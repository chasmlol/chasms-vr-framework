//! A small forward renderer for the range: lit, textured meshes into an eye (or scope) target.
//!
//! The adapter views the shared eye textures as UNORM, like a game's back buffer, so the pixel
//! shader writes gamma-encoded colour; the runtime reads them back as sRGB.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use rustvr_asset as asset;
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer, static_buffer};
use rustvr_gpu::shader::{Program, element};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use std::collections::HashMap;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::content::{MeshData, Vertex};

const SHADER: &str = r#"
cbuffer Frame : register(b0) { float4x4 view_proj; float4 eye_pos; float4 sun_dir; float4 fog; float4 view_fwd; };
cbuffer Object : register(b1) { float4x4 model; float4 tint; float4 params; };
Texture2D tex : register(t0);
SamplerState samp : register(s0);
struct VSIn { float3 pos : POSITION; float3 normal : NORMAL; float2 uv : TEXCOORD0; float4 color : COLOR0; };
struct VSOut { float4 pos : SV_Position; float3 world : TEXCOORD1; float3 normal : NORMAL; float2 uv : TEXCOORD0; float4 color : COLOR0; };
VSOut VSMain(VSIn i) {
    VSOut o;
    float4 w = mul(model, float4(i.pos, 1));
    o.world = w.xyz;
    o.pos = mul(view_proj, w);
    o.normal = mul((float3x3)model, i.normal);
    o.uv = i.uv;
    o.color = i.color;
    return o;
}
struct PSOut { float4 color : SV_Target0; float depth : SV_Target1; };
PSOut PSMain(VSOut i, bool front : SV_IsFrontFace) {
    float3 base = pow(i.color.rgb, 2.2) * tint.rgb;   // vertex colours are sRGB
    if (params.x > 0.5) base *= tex.Sample(samp, i.uv).rgb;
    float3 n = normalize(i.normal) * (front ? 1 : -1);
    float diffuse = saturate(dot(n, -sun_dir.xyz));
    float hemi = 0.5 + 0.5 * n.y;
    float3 c = base * (0.18 + 0.22 * hemi + 0.75 * diffuse) + base * params.y;
    float dist = distance(i.world, eye_pos.xyz);
    c = lerp(c, fog.rgb, saturate(dist * fog.w));
    PSOut o;
    o.color = float4(pow(saturate(c), 1.0 / 2.2), 1);
    // Linear depth along the view axis, for the runtime to hide the player's body behind the world.
    o.depth = dot(i.world - eye_pos.xyz, view_fwd.xyz);
    return o;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameCb {
    view_proj: [f32; 16],
    eye_pos: [f32; 4],
    sun_dir: [f32; 4],
    fog: [f32; 4],
    view_fwd: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjectCb {
    model: [f32; 16],
    tint: [f32; 4],
    params: [f32; 4],
}

pub struct Mesh {
    vb: ID3D11Buffer,
    ib: ID3D11Buffer,
    count: u32,
    texture: Option<String>,
}

pub struct Renderer {
    program: Program,
    frame_cb: ConstantBuffer<FrameCb>,
    object_cb: ConstantBuffer<ObjectCb>,
    raster: ID3D11RasterizerState,
    raster_two_sided: ID3D11RasterizerState,
    depth_state: ID3D11DepthStencilState,
    blend: ID3D11BlendState,
    sampler: ID3D11SamplerState,
    textures: HashMap<String, Texture>,
    depth: HashMap<(u32, u32), Texture>,
    asset_dir: std::path::PathBuf,
}

/// One thing to draw this frame.
pub struct Draw<'a> {
    pub mesh: &'a Mesh,
    pub model: Mat4,
    pub tint: [f32; 4],
    pub emissive: f32,
    /// Draw both sides (models from games, whose winding we don't control).
    pub two_sided: bool,
}

impl Renderer {
    pub fn new(gpu: &Gpu, asset_dir: std::path::PathBuf) -> Result<Renderer> {
        let layout = [
            element(b"POSITION\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0),
            element(b"NORMAL\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 12),
            element(b"TEXCOORD\0", 0, DXGI_FORMAT_R32G32_FLOAT, 24),
            element(b"COLOR\0", 0, DXGI_FORMAT_R8G8B8A8_UNORM, 32),
        ];
        let mut r = Renderer {
            program: Program::new(gpu, SHADER, "range", &layout)?,
            frame_cb: ConstantBuffer::new(gpu)?,
            object_cb: ConstantBuffer::new(gpu)?,
            raster: pipeline::raster_state(gpu, true, false)?,
            raster_two_sided: pipeline::raster_state(gpu, false, false)?,
            depth_state: pipeline::depth_state(gpu, true, true)?,
            blend: pipeline::blend_state(gpu, Blend::Opaque)?,
            sampler: pipeline::sampler(gpu, true, true)?,
            textures: HashMap::new(),
            depth: HashMap::new(),
            asset_dir,
        };
        r.textures.insert("grid".into(), builtin_texture(gpu, grid_pixels)?);
        r.textures.insert("target".into(), builtin_texture(gpu, target_pixels)?);
        Ok(r)
    }

    pub fn upload(&mut self, gpu: &Gpu, m: &MeshData) -> Result<Mesh> {
        if let Some(t) = &m.texture {
            self.ensure_texture(gpu, t);
        }
        Ok(Mesh {
            vb: static_buffer(gpu, bytemuck::cast_slice(&m.vertices), D3D11_BIND_VERTEX_BUFFER)?,
            ib: static_buffer(gpu, bytemuck::cast_slice(&m.indices), D3D11_BIND_INDEX_BUFFER)?,
            count: m.indices.len() as u32,
            texture: m.texture.clone().filter(|t| self.textures.contains_key(t)),
        })
    }

    /// Loads a game texture from the asset cache (once); a missing one leaves the mesh untextured.
    fn ensure_texture(&mut self, gpu: &Gpu, key: &str) {
        if self.textures.contains_key(key) {
            return;
        }
        let tex = asset::load_texture(&self.asset_dir.join("metro2033"), key).and_then(|t| {
            let format = match t.format {
                asset::TexFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                asset::TexFormat::Bc1 => DXGI_FORMAT_BC1_UNORM_SRGB,
                asset::TexFormat::Bc2 => DXGI_FORMAT_BC2_UNORM_SRGB,
                asset::TexFormat::Bc3 => DXGI_FORMAT_BC3_UNORM_SRGB,
                asset::TexFormat::Bc4 => DXGI_FORMAT_BC4_UNORM,
                asset::TexFormat::Bc5 => DXGI_FORMAT_BC5_UNORM,
                asset::TexFormat::Bc7 => DXGI_FORMAT_BC7_UNORM_SRGB,
            };
            let mips: Vec<&[u8]> = t.mips.iter().map(|m| m.as_slice()).collect();
            Texture::upload(gpu, t.width, t.height, format, &mips)
        });
        match tex {
            Ok(t) => {
                self.textures.insert(key.to_owned(), t);
            }
            Err(e) => tracing::debug!("range: texture {key}: {e:#}"),
        }
    }

    /// Renders `draws` into `target` from a camera (`view_proj`, eye position and forward in the
    /// world). With `linear_depth`, also writes each pixel's distance along the view axis there
    /// (0 where nothing was drawn).
    pub fn render(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        linear_depth: Option<&ID3D11RenderTargetView>,
        w: u32,
        h: u32,
        view_proj: Mat4,
        eye: Vec3,
        fwd: Vec3,
        draws: &[Draw],
    ) -> Result<()> {
        if let std::collections::hash_map::Entry::Vacant(e) = self.depth.entry((w, h)) {
            e.insert(Texture::new(gpu, &TexDesc::depth(w, h))?);
        }
        let depth = self.depth.get(&(w, h)).unwrap().dsv.clone().unwrap();
        let ctx = &gpu.ctx;
        let fog = [0.20f32, 0.22, 0.25];
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone()), linear_depth.cloned()]), &depth);
            let clear = [fog[0].powf(1.0 / 2.2), fog[1].powf(1.0 / 2.2), fog[2].powf(1.0 / 2.2), 1.0];
            ctx.ClearRenderTargetView(target, &clear);
            if let Some(d) = linear_depth {
                ctx.ClearRenderTargetView(d, &[0.0; 4]);
            }
            ctx.ClearDepthStencilView(&depth, D3D11_CLEAR_DEPTH.0, 1.0, 0);
            ctx.OMSetDepthStencilState(&self.depth_state, 0);
            ctx.OMSetBlendState(&self.blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone()), Some(self.object_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone()), Some(self.object_cb.buf.clone())]));
        }
        pipeline::viewport(ctx, w, h);
        self.program.bind(ctx);
        let sun = Vec3::new(-0.4, -1.0, 0.6).normalize();
        self.frame_cb.update(
            ctx,
            &FrameCb {
                view_proj: view_proj.to_cols_array(),
                eye_pos: eye.extend(1.0).to_array(),
                sun_dir: sun.extend(0.0).to_array(),
                fog: [fog[0], fog[1], fog[2], 0.012],
                view_fwd: fwd.normalize_or(Vec3::Z).extend(0.0).to_array(),
            },
        );
        for d in draws {
            let tex = d.mesh.texture.as_ref().and_then(|t| self.textures.get(t));
            self.object_cb.update(
                ctx,
                &ObjectCb { model: d.model.to_cols_array(), tint: d.tint, params: [tex.is_some() as u32 as f32, d.emissive, 0.0, 0.0] },
            );
            unsafe {
                ctx.RSSetState(if d.two_sided { &self.raster_two_sided } else { &self.raster });
                ctx.PSSetShaderResources(0, Some(&[tex.and_then(|t| t.srv.clone())]));
                let stride = size_of::<Vertex>() as u32;
                let offset = 0u32;
                ctx.IASetVertexBuffers(0, 1, Some(&Some(d.mesh.vb.clone())), Some(&stride), Some(&offset));
                ctx.IASetIndexBuffer(&d.mesh.ib, DXGI_FORMAT_R32_UINT, 0);
                ctx.DrawIndexed(d.mesh.count, 0, 0);
            }
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}

fn builtin_texture(gpu: &Gpu, f: fn(u32, u32) -> [u8; 4]) -> Result<Texture> {
    let n = 256u32;
    let mut levels: Vec<Vec<u8>> = Vec::new();
    let mut size = n;
    while size >= 1 {
        let mut px = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                px.extend_from_slice(&f(x * n / size, y * n / size));
            }
        }
        levels.push(px);
        if size == 1 {
            break;
        }
        size /= 2;
    }
    let mips: Vec<&[u8]> = levels.iter().map(|l| l.as_slice()).collect();
    Texture::upload(gpu, n, n, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &mips)
}

/// One-metre tiles with a thin line (the UVs are in metres).
fn grid_pixels(x: u32, y: u32) -> [u8; 4] {
    let line = x < 4 || y < 4;
    let check = ((x / 128) + (y / 128)).is_multiple_of(2);
    let v = if line { 90 } else if check { 200 } else { 185 };
    [v, v, v, 255]
}

/// Concentric red and white rings.
fn target_pixels(x: u32, y: u32) -> [u8; 4] {
    let dx = x as f32 - 127.5;
    let dy = y as f32 - 127.5;
    let r = (dx * dx + dy * dy).sqrt() / 128.0;
    if r > 1.0 {
        return [40, 40, 40, 255];
    }
    if r < 0.12 {
        return [230, 190, 30, 255];
    }
    if ((r * 5.0) as u32).is_multiple_of(2) { [200, 30, 30, 255] } else { [235, 235, 235, 255] }
}
