//! Paints egui output into a D3D11 render target (menus, the test range's HUD).
//!
//! egui produces sRGB, premultiplied-alpha colours. We paint into an 8-bit UNORM target without
//! any sRGB conversion ("gamma space"), which is what egui expects, and leave the result
//! premultiplied so it can be composited as a panel.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::pipeline::{self, Blend, ConstantBuffer, DynBuffer};
use crate::shader::{Program, element};
use crate::{Gpu, TexDesc, Texture};

const SHADER: &str = r#"
cbuffer Screen : register(b0) { float2 size_points; float2 pad; };
Texture2D tex : register(t0);
SamplerState samp : register(s0);
struct VSIn { float2 pos : POSITION; float2 uv : TEXCOORD0; float4 color : COLOR0; };
struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 color : COLOR0; };
VSOut VSMain(VSIn i) {
    VSOut o;
    o.pos = float4(2.0 * i.pos.x / size_points.x - 1.0, 1.0 - 2.0 * i.pos.y / size_points.y, 0.0, 1.0);
    o.uv = i.uv;
    o.color = i.color;
    return o;
}
float4 PSMain(VSOut i) : SV_Target { return i.color * tex.Sample(samp, i.uv); }
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vtx {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ScreenCb {
    size_points: [f32; 2],
    pad: [f32; 2],
}

pub struct EguiPainter {
    program: Program,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    depth: ID3D11DepthStencilState,
    sampler: ID3D11SamplerState,
    cb: ConstantBuffer<ScreenCb>,
    vb: DynBuffer,
    ib: DynBuffer,
    textures: HashMap<egui::TextureId, Texture>,
}

impl EguiPainter {
    pub fn new(gpu: &Gpu) -> Result<EguiPainter> {
        let layout = [
            element(b"POSITION\0", 0, DXGI_FORMAT_R32G32_FLOAT, 0),
            element(b"TEXCOORD\0", 0, DXGI_FORMAT_R32G32_FLOAT, 8),
            element(b"COLOR\0", 0, DXGI_FORMAT_R8G8B8A8_UNORM, 16),
        ];
        Ok(EguiPainter {
            program: Program::new(gpu, SHADER, "egui", &layout)?,
            blend: pipeline::blend_state(gpu, Blend::Premultiplied)?,
            raster: pipeline::raster_state(gpu, false, true)?,
            depth: pipeline::depth_state(gpu, false, false)?,
            sampler: pipeline::sampler(gpu, true, false)?,
            cb: ConstantBuffer::new(gpu)?,
            vb: DynBuffer::vertices(),
            ib: DynBuffer::indices(),
            textures: HashMap::new(),
        })
    }

    /// Applies texture changes, paints `primitives` into `target` (cleared to transparent first if
    /// `clear`), then frees textures egui no longer needs.
    pub fn paint(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        width_px: u32,
        height_px: u32,
        pixels_per_point: f32,
        primitives: &[egui::ClippedPrimitive],
        delta: &mut egui::TexturesDelta,
        clear: bool,
    ) -> Result<()> {
        for (id, patches) in &delta.set {
            for patch in patches {
                self.set_texture(gpu, *id, patch)?;
            }
        }
        let ctx = &gpu.ctx;
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            if clear {
                ctx.ClearRenderTargetView(target, &[0.0, 0.0, 0.0, 0.0]);
            }
        }
        pipeline::viewport(ctx, width_px, height_px);
        self.cb.update(ctx, &ScreenCb { size_points: [width_px as f32 / pixels_per_point, height_px as f32 / pixels_per_point], pad: [0.0; 2] });
        self.program.bind(ctx);
        unsafe {
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.OMSetBlendState(&self.blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth, 0);
            ctx.RSSetState(&self.raster);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        }

        // One vertex/index upload for the whole frame; each mesh draws its range.
        let mut vertices: Vec<Vtx> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut draws = Vec::new();
        for p in primitives {
            let egui::epaint::Primitive::Mesh(mesh) = &p.primitive else { continue };
            if mesh.indices.is_empty() {
                continue;
            }
            let base = vertices.len() as u32;
            let first = indices.len() as u32;
            vertices.extend(mesh.vertices.iter().map(|v| Vtx {
                pos: [v.pos.x, v.pos.y],
                uv: [v.uv.x, v.uv.y],
                color: v.color.to_array(),
            }));
            indices.extend(mesh.indices.iter().map(|i| i + base));
            draws.push((p.clip_rect, mesh.texture_id, first, mesh.indices.len() as u32));
        }
        if draws.is_empty() {
            self.free(delta);
            return Ok(());
        }
        self.vb.write(gpu, bytemuck::cast_slice(&vertices))?;
        self.ib.write(gpu, bytemuck::cast_slice(&indices))?;
        unsafe {
            let stride = size_of::<Vtx>() as u32;
            let offset = 0u32;
            ctx.IASetVertexBuffers(0, 1, Some(&self.vb.buf.clone()), Some(&stride), Some(&offset));
            ctx.IASetIndexBuffer(self.ib.buf.as_ref(), DXGI_FORMAT_R32_UINT, 0);
        }
        for (clip, tex, first, count) in draws {
            let Some(t) = self.textures.get(&tex) else { continue };
            let r = RECT {
                left: (clip.min.x * pixels_per_point).floor().max(0.0) as i32,
                top: (clip.min.y * pixels_per_point).floor().max(0.0) as i32,
                right: (clip.max.x * pixels_per_point).ceil().min(width_px as f32) as i32,
                bottom: (clip.max.y * pixels_per_point).ceil().min(height_px as f32) as i32,
            };
            if r.right <= r.left || r.bottom <= r.top {
                continue;
            }
            unsafe {
                ctx.RSSetScissorRects(Some(&[r]));
                ctx.PSSetShaderResources(0, Some(std::slice::from_ref(&t.srv)));
                ctx.DrawIndexed(count, first, 0);
            }
        }
        self.free(delta);
        Ok(())
    }

    fn free(&mut self, delta: &mut egui::TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
        // egui insists every delta is consumed.
        delta.clear();
    }

    fn set_texture(&mut self, gpu: &Gpu, id: egui::TextureId, delta: &egui::epaint::ImageDelta) -> Result<()> {
        let egui::ImageData::Color(image) = &delta.image;
        let [w, h] = image.size;
        let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        match delta.pos {
            None => {
                let desc = TexDesc { srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM), rtv: None, ..TexDesc::target(w as u32, h as u32, DXGI_FORMAT_R8G8B8A8_UNORM) };
                let t = Texture::new(gpu, &desc)?;
                upload_region(gpu, &t, 0, 0, w as u32, h as u32, &pixels);
                self.textures.insert(id, t);
            }
            Some([x, y]) => {
                if let Some(t) = self.textures.get(&id) {
                    upload_region(gpu, t, x as u32, y as u32, w as u32, h as u32, &pixels);
                }
            }
        }
        Ok(())
    }
}

fn upload_region(gpu: &Gpu, t: &Texture, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
    let b = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
    unsafe { gpu.ctx.UpdateSubresource(&t.tex, 0, Some(&b), rgba.as_ptr().cast(), w * 4, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_a_label() {
        let gpu = Gpu::new().unwrap();
        let mut painter = EguiPainter::new(&gpu).unwrap();
        let target = Texture::new(&gpu, &TexDesc::target(256, 128, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256.0, 128.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 30.0)), 0.0, egui::Color32::RED);
            ui.label("RustVR");
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        painter.paint(&gpu, target.rtv.as_ref().unwrap(), 256, 128, out.pixels_per_point, &prims, &mut out.textures_delta, true).unwrap();
        let px = target.read_rgba8(&gpu).unwrap();
        let at = |x: usize, y: usize| &px[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4];
        assert_eq!(at(30, 25), &[255, 0, 0, 255], "the red box is painted");
        assert_eq!(at(250, 120)[3], 0, "untouched pixels stay transparent");
        let opaque = px.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
        assert!(opaque > 50 * 30, "the label adds pixels beyond the box ({opaque})");
    }
}
