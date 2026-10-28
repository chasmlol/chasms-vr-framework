//! The framework view: instead of the game's picture, the world as the runtime's physics has it.
//! The game's collision around the player, the game's props as their own convex collision, the
//! runtime's objects, the palms and the fingers, each in a colour of its own (see
//! `Rig::physics_triangles`), shaded and outlined, in tracking space. A hotkey swaps between this
//! and the game's picture (`Mechanics::framework_view`), to see what the hands are really touching.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use rustvr_gpu::pipeline::{self, ConstantBuffer, DynBuffer};
use rustvr_gpu::shader::{Program, element};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::{Fov, Pose};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32G32B32_FLOAT;

use crate::backend::EyeTarget;
use crate::compositor::{projection, view_matrix};

const SHADER: &str = r#"
cbuffer Draw : register(b0) {
    float4x4 view_proj;
    float4 eye;     // xyz: the eye (tracking space); w: 1 for the outline pass
};
struct VIn { float3 pos : POSITION; float3 normal : NORMAL; float3 color : COLOR; };
struct VOut { float4 pos : SV_Position; float3 world : TEXCOORD0; float3 normal : NORMAL; float3 color : COLOR; };
VOut VSMain(VIn v) {
    VOut o;
    o.pos = mul(view_proj, float4(v.pos, 1));
    o.world = v.pos;
    o.normal = v.normal;
    o.color = v.color;
    return o;
}
float4 PSMain(VOut i) : SV_Target {
    if (eye.w > 0) return float4(i.color * 0.25, 1);
    // A headlamp: faces towards the eye bright, edge-on dark, a little from above.
    float3 v = normalize(eye.xyz - i.world);
    float3 n = normalize(i.normal);
    float lit = 0.25 + 0.6 * abs(dot(n, v)) + 0.15 * saturate(n.y);
    return float4(i.color * lit, 1);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawCb {
    view_proj: [f32; 16],
    eye: [f32; 4],
}

pub struct PhysicsView {
    program: Program,
    vertices: Vec<Vertex>,
    vb: DynBuffer,
    /// Drawn over everything (the guns' working parts).
    over: Vec<Vertex>,
    over_vb: DynBuffer,
    no_depth: ID3D11DepthStencilState,
    cb: ConstantBuffer<DrawCb>,
    depth: Vec<Texture>,
    solid: ID3D11RasterizerState,
    outline: ID3D11RasterizerState,
    /// Draw the outlines alone, bright (shapes over the game's picture, which shows through).
    pub wireframe_only: bool,
    depth_test: ID3D11DepthStencilState,
    opaque: ID3D11BlendState,
}

impl PhysicsView {
    pub fn new(gpu: &Gpu) -> Result<PhysicsView> {
        let layout = [
            element(b"POSITION\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0),
            element(b"NORMAL\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 12),
            element(b"COLOR\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 24),
        ];
        let raster = |fill: D3D11_FILL_MODE, bias: i32| -> Result<ID3D11RasterizerState> {
            let desc = D3D11_RASTERIZER_DESC {
                FillMode: fill,
                CullMode: D3D11_CULL_NONE,
                FrontCounterClockwise: false.into(),
                DepthBias: bias,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: if bias != 0 { -1.0 } else { 0.0 },
                DepthClipEnable: true.into(),
                ScissorEnable: false.into(),
                MultisampleEnable: false.into(),
                AntialiasedLineEnable: false.into(),
            };
            let mut s = None;
            unsafe { gpu.device.CreateRasterizerState(&desc, Some(&mut s))? };
            Ok(s.unwrap())
        };
        Ok(PhysicsView {
            program: Program::new(gpu, SHADER, "physics_view", &layout)?,
            vertices: Vec::new(),
            vb: DynBuffer::vertices(),
            over: Vec::new(),
            over_vb: DynBuffer::vertices(),
            no_depth: pipeline::depth_state(gpu, false, false)?,
            cb: ConstantBuffer::new(gpu)?,
            depth: Vec::new(),
            solid: raster(D3D11_FILL_SOLID, 0)?,
            outline: raster(D3D11_FILL_WIREFRAME, -200)?,
            wireframe_only: false,
            depth_test: pipeline::depth_state(gpu, true, true)?,
            opaque: pipeline::blend_state(gpu, pipeline::Blend::Opaque)?,
        })
    }

    /// This frame's triangles (tracking space) and their colours, and those drawn over them.
    pub fn set_triangles(&mut self, gpu: &Gpu, tris: impl IntoIterator<Item = ([Vec3; 3], [f32; 3])>, over: impl IntoIterator<Item = ([Vec3; 3], [f32; 3])>) -> Result<()> {
        let push = |v: &mut Vec<Vertex>, (t, color): ([Vec3; 3], [f32; 3])| {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().to_array();
            v.extend(t.map(|p| Vertex { pos: p.to_array(), normal: n, color }));
        };
        self.vertices.clear();
        self.over.clear();
        tris.into_iter().for_each(|t| push(&mut self.vertices, t));
        over.into_iter().for_each(|t| push(&mut self.over, t));
        self.vb.write(gpu, bytemuck::cast_slice(&self.vertices))?;
        self.over_vb.write(gpu, bytemuck::cast_slice(&self.over))
    }

    /// Draws them into one eye's target, over whatever is there.
    pub fn draw(&mut self, gpu: &Gpu, target: &EyeTarget, eye: &Pose, fov: &Fov) -> Result<()> {
        if self.vertices.is_empty() {
            return Ok(());
        }
        // One depth buffer per target size (the eyes, a desktop view): the last two kept.
        let i = match self.depth.iter().position(|d| (d.width, d.height) == (target.width, target.height)) {
            Some(i) => i,
            None => {
                if self.depth.len() >= 2 {
                    self.depth.remove(0);
                }
                self.depth.push(Texture::new(gpu, &TexDesc::depth(target.width, target.height))?);
                self.depth.len() - 1
            }
        };
        let dsv = self.depth[i].dsv.clone();
        let ctx = &gpu.ctx;
        let view_proj: Mat4 = projection(fov) * view_matrix(eye);
        let mut cb = DrawCb { view_proj: view_proj.to_cols_array(), eye: [eye.pos[0], eye.pos[1], eye.pos[2], 0.0] };
        unsafe {
            if let Some(dsv) = &dsv {
                ctx.ClearDepthStencilView(dsv, D3D11_CLEAR_DEPTH.0 as u32, 1.0, 0);
            }
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), dsv.as_ref());
            ctx.OMSetDepthStencilState(&self.depth_test, 0);
            ctx.OMSetBlendState(&self.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetVertexBuffers(0, 1, Some(&self.vb.buf.clone()), Some(&(size_of::<Vertex>() as u32)), Some(&0));
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
        }
        pipeline::viewport(ctx, target.width, target.height);
        self.program.bind(ctx);
        let n = self.vertices.len() as u32;
        // Solid, then the outlines on top (or bright outlines alone, over something else).
        let passes = if self.wireframe_only { vec![(&self.outline, 0.0)] } else { vec![(&self.solid, 0.0), (&self.outline, 1.0)] };
        for (raster, outline) in passes {
            cb.eye[3] = outline;
            self.cb.update(ctx, &cb);
            unsafe {
                ctx.RSSetState(raster);
                ctx.Draw(n, 0);
            }
        }
        if !self.over.is_empty() {
            cb.eye[3] = 0.0;
            self.cb.update(ctx, &cb);
            unsafe {
                ctx.OMSetDepthStencilState(&self.no_depth, 0);
                ctx.RSSetState(&self.solid);
                ctx.IASetVertexBuffers(0, 1, Some(&self.over_vb.buf.clone()), Some(&(size_of::<Vertex>() as u32)), Some(&0));
                ctx.Draw(self.over.len() as u32, 0);
            }
        }
        unsafe { ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

    #[test]
    fn a_box_ahead_is_drawn_in_the_middle_of_the_view() {
        let gpu = Gpu::new().unwrap();
        let tex = Texture::new(&gpu, &TexDesc::target(128, 128, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
        let target = EyeTarget { tex: tex.tex.clone(), rtv: tex.rtv.clone().unwrap(), width: 128, height: 128 };
        unsafe { gpu.ctx.ClearRenderTargetView(&target.rtv, &[0.0; 4]) };
        // A 40 cm cube 2 m ahead (+Z in tracking space), as triangles of its faces.
        let c = Vec3::new(0.0, 0.0, 2.0);
        let corner = |i: usize| c + Vec3::new(if i & 1 != 0 { 0.2 } else { -0.2 }, if i & 2 != 0 { 0.2 } else { -0.2 }, if i & 4 != 0 { 0.2 } else { -0.2 });
        let faces = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
        let tris: Vec<([Vec3; 3], [f32; 3])> =
            faces.iter().flat_map(|f| [([corner(f[0]), corner(f[1]), corner(f[2])], [1.0, 0.5, 0.1]), ([corner(f[0]), corner(f[2]), corner(f[3])], [1.0, 0.5, 0.1])]).collect();
        let mut view = PhysicsView::new(&gpu).unwrap();
        view.set_triangles(&gpu, tris, []).unwrap();
        let eye = Pose { pos: [0.0; 3], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
        let fov = Fov { left: -0.8, right: 0.8, up: 0.8, down: -0.8 };
        view.draw(&gpu, &target, &eye, &fov).unwrap();
        let px = tex.read_rgba8(&gpu).unwrap();
        let at = |x: usize, y: usize| &px[(y * 128 + x) * 4..(y * 128 + x) * 4 + 3];
        assert!(at(64, 64)[0] > 60, "the cube in the middle: {:?}", at(64, 64));
        assert_eq!(at(4, 4), [0, 0, 0], "nothing in the corner");
    }
}
