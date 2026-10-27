//! Moves a game's eye picture to where it's seen now: the head where it is at this headset frame,
//! and the player where the game has walked or turned them since, by the picture's depth.
//!
//! A game that can't render both eyes at the headset's rate (Metro renders one eye per game frame,
//! so a pair every two) shows a picture 20-30 ms old, and the same picture again when no new pair
//! has come. The headset turns a picture for the head's turn since, but not for the player's walk
//! (that moves the world, not the head), so the world steps while the hands, drawn by the runtime
//! every frame, don't. Here each picture is laid over a grid, each grid point pushed out to its
//! depth, carried from where that eye was in the world (`SlotInfo::eye_map`) to where it is now,
//! and drawn from the eye's pose now: every headset frame shows the world where it is, as the
//! framework view does. Where something near hides something far, the far side is stretched over
//! the gap (a few centimetres at most: a frame or two of movement).
//!
//! It also writes the depth seen from the new eye (metres along its view), for the runtime's body
//! to hide behind the game's walls.
//!
//! A picture the game drew the player into (`crate::in_frame`) is moved without the player's
//! pixels ([`WarpSource::mask`]): the player is moved on its own, where it is now. So are the
//! things of the game's a hand holds ([`WarpSource::props`]): each moved again on its own with the
//! hand ([`WarpSource::only`], its motion as `to_target`), from the game's own pixels and depth.

use std::collections::HashMap;

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use rustvr_gpu::pipeline::{self, ConstantBuffer};
use rustvr_gpu::shader::Program;
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::{Fov, Pose};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT};

use crate::backend::EyeTarget;
use crate::compositor::{projection, view_matrix};

const SHADER: &str = r#"
cbuffer Warp : register(b0) {
    float4x4 to_clip;   // the source eye's view space (tracking metres) -> the target's clip space
    float4x4 to_view;   // the source eye's view space -> the target's view space
    float4 src_tan;     // the source picture's left, right, up, down tangents
    float4 grid;        // x: cells across, y: cells down, z: metres per depth unit, w: farthest point (metres)
    float4 opts;        // x: 1 to leave out the pixels src_mask has (> 0: the player's), y: 1 to leave out
                        // the ones src_props marks, z: > 0 to keep only the ones it marks so
    float4 rect;        // the part of the picture drawn (uv: left, top, right, bottom)
};
Texture2D src : register(t0);
Texture2D<float> src_depth : register(t1);
Texture2D<float> src_mask : register(t2);
Texture2D<float> src_props : register(t3);
SamplerState samp : register(s0);
SamplerState pt : register(s1);

// (The picture's coordinates go across the screen as its pixels did: no perspective.)
struct WOut { float4 pos : SV_Position; noperspective float2 uv : TEXCOORD0; float vz : TEXCOORD1; };

WOut VSMain(uint id : SV_VertexID) {
    uint across = (uint)grid.x + 1;
    float2 uv = rect.xy + float2(id % across, id / across) / grid.xy * (rect.zw - rect.xy);
    float d = src_depth.SampleLevel(pt, uv, 0) * grid.z;
    float2 t = float2(lerp(src_tan.x, src_tan.y, uv.x), lerp(src_tan.z, src_tan.w, uv.y));
    // Nothing drawn, or past the far plane: a direction (the sky, the far distance).
    bool far = !(d > 0) || d > grid.w;
    float4 p = far ? float4(t, 1, 0) : float4(t * d, d, 1);
    WOut o;
    o.pos = mul(to_clip, p);
    if (far) o.pos.z = o.pos.w * 0.99999;
    o.uv = uv;
    o.vz = far ? 100000.0 : mul(to_view, p).z;
    return o;
}

struct POut { float4 color : SV_Target0; float z : SV_Target1; };
// The player's pixels and the one around them (the game's anti-aliasing blends the player's edge
// into its neighbours).
bool near_player(float2 uv) {
    uint w, h;
    src_mask.GetDimensions(w, h);
    int2 c = int2(uv * float2(w, h));
    [unroll] for (int k = 0; k < 9; k++) {
        int2 q = clamp(c + int2(k % 3 - 1, k / 3 - 1), 0, int2(w - 1, h - 1));
        if (src_mask.Load(int3(q, 0)) > 0) return true;
    }
    return false;
}

// A held thing's pixels and the one around them.
bool near_prop(float2 uv) {
    uint w, h;
    src_props.GetDimensions(w, h);
    int2 c = int2(uv * float2(w, h));
    [unroll] for (int k = 0; k < 9; k++) {
        int2 q = clamp(c + int2(k % 3 - 1, k / 3 - 1), 0, int2(w - 1, h - 1));
        if (src_props.Load(int3(q, 0)) > 0) return true;
    }
    return false;
}

POut PSMain(WOut i) {
    if (opts.x > 0 && near_player(i.uv)) discard;
    if (opts.y > 0 && near_prop(i.uv)) discard;
    if (opts.z > 0) {
        uint w, h;
        src_props.GetDimensions(w, h);
        if (abs(src_props.Load(int3(min(uint2(i.uv * float2(w, h)), uint2(w - 1, h - 1)), 0)) - opts.z) > 0.25) discard;
    }
    POut o;
    o.color = float4(src.Sample(samp, i.uv).rgb, 1);
    o.z = i.vz;
    return o;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WarpCb {
    to_clip: [f32; 16],
    to_view: [f32; 16],
    src_tan: [f32; 4],
    grid: [f32; 4],
    opts: [f32; 4],
    rect: [f32; 4],
}

/// Grid points every this many of the source's pixels.
pub const CELL_PIXELS: u32 = 6;
/// Points farther than this (metres) are drawn as directions (the projection's far plane is 200 m).
const FARTHEST: f32 = 190.0;

/// One eye picture to move.
#[derive(Clone, Copy)]
pub struct WarpSource<'a> {
    pub color: &'a ID3D11ShaderResourceView,
    /// Its depth: distance along its view, in units of `metres_per_unit` metres; 0: nothing drawn.
    pub depth: &'a ID3D11ShaderResourceView,
    pub width: u32,
    pub height: u32,
    /// The tracking-space eye it was rendered from, and its field of view.
    pub pose: Pose,
    pub fov: Fov,
    /// Its tracking space -> the target's (the player walked or turned in between).
    pub to_target: Mat4,
    pub metres_per_unit: f32,
    /// Pixels left out where this is above 0 (`R32_FLOAT`: the player's depth in the picture).
    pub mask: Option<&'a ID3D11ShaderResourceView>,
    /// The held things' pixels (`R32_FLOAT`, each marked with its number): left out, unless `only`.
    pub props: Option<&'a ID3D11ShaderResourceView>,
    /// > 0: only the pixels `props` marks with this number are drawn (one held thing, moved with
    /// the hand: `to_target` is its motion).
    pub only: f32,
    /// The part of the picture drawn (uv: left, top, right, bottom), and a grid point every this
    /// many of its pixels.
    pub rect: [f32; 4],
    pub cell: u32,
}

impl WarpSource<'_> {
    /// All of the picture, a grid point every [`CELL_PIXELS`].
    pub const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
}

/// The depth and the seen-from-here depth for one target size.
struct Targets {
    width: u32,
    height: u32,
    depth: Texture,
    view_z: Texture,
}

pub struct Warp {
    program: Program,
    cb: ConstantBuffer<WarpCb>,
    linear: ID3D11SamplerState,
    point: ID3D11SamplerState,
    depth_test: ID3D11DepthStencilState,
    raster: ID3D11RasterizerState,
    opaque: ID3D11BlendState,
    /// Index buffers by grid size (cells across, down): the triangle count.
    grids: HashMap<(u32, u32), (ID3D11Buffer, u32)>,
    targets: Vec<Targets>,
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if !q.is_finite() || q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

/// The source eye's view space -> the target's view space, for a picture from `src` (tracking,
/// before `to_target`) seen from `eye`.
pub fn source_to_target_view(src: &Pose, to_target: Mat4, eye: &Pose) -> Mat4 {
    view_matrix(eye) * to_target * Mat4::from_rotation_translation(quat(src.rot), Vec3::from_array(src.pos))
}

impl Warp {
    pub fn new(gpu: &Gpu) -> Result<Warp> {
        let raster = {
            let desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            };
            let mut s = None;
            unsafe { gpu.device.CreateRasterizerState(&desc, Some(&mut s))? };
            s.unwrap()
        };
        Ok(Warp {
            program: Program::new(gpu, SHADER, "warp", &[])?,
            cb: ConstantBuffer::new(gpu)?,
            linear: pipeline::sampler(gpu, true, false)?,
            point: pipeline::sampler(gpu, false, false)?,
            depth_test: pipeline::depth_state(gpu, true, true)?,
            raster,
            opaque: pipeline::blend_state(gpu, pipeline::Blend::Opaque)?,
            grids: HashMap::new(),
            targets: Vec::new(),
        })
    }

    fn targets(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<usize> {
        if let Some(i) = self.targets.iter().position(|t| (t.width, t.height) == (width, height)) {
            return Ok(i);
        }
        // The eyes and a desktop view: the last three sizes kept.
        if self.targets.len() >= 3 {
            self.targets.remove(0);
        }
        self.targets.push(Targets {
            width,
            height,
            depth: Texture::new(gpu, &TexDesc::depth(width, height))?,
            view_z: Texture::new(gpu, &TexDesc::target(width, height, DXGI_FORMAT_R32_FLOAT))?,
        });
        Ok(self.targets.len() - 1)
    }

    /// What [`Warp::draw`] writes for a target this size: the depth seen from its eye (metres along
    /// its view, 0 where the picture doesn't reach). Valid after the draw, until the next.
    pub fn view_depth(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<ID3D11ShaderResourceView> {
        let i = self.targets(gpu, width, height)?;
        Ok(self.targets[i].view_z.srv.clone().unwrap())
    }

    fn grid(&mut self, gpu: &Gpu, cells: (u32, u32)) -> Result<(ID3D11Buffer, u32)> {
        if let Some(g) = self.grids.get(&cells) {
            return Ok(g.clone());
        }
        let (nx, ny) = cells;
        let mut idx: Vec<u32> = Vec::with_capacity((nx * ny * 6) as usize);
        for j in 0..ny {
            for i in 0..nx {
                let v0 = j * (nx + 1) + i;
                let (v1, v2) = (v0 + 1, v0 + nx + 1);
                idx.extend([v0, v1, v2, v1, v2 + 1, v2]);
            }
        }
        let buf = pipeline::static_buffer(gpu, bytemuck::cast_slice(&idx), D3D11_BIND_INDEX_BUFFER)?;
        let g = (buf, idx.len() as u32);
        self.grids.insert(cells, g.clone());
        Ok(g)
    }

    /// Draws `sources` into `target` as seen from `eye` with `fov`: each over the last where it
    /// reaches (put the eye's own picture last, the other's first to fill the edges).
    pub fn draw(&mut self, gpu: &Gpu, target: &EyeTarget, eye: &Pose, fov: &Fov, sources: &[WarpSource]) -> Result<()> {
        let ctx = &gpu.ctx;
        let ti = self.targets(gpu, target.width, target.height)?;
        let (dsv, zrtv) = (self.targets[ti].depth.dsv.clone().unwrap(), self.targets[ti].view_z.rtv.clone().unwrap());
        let proj = projection(fov);
        unsafe {
            ctx.ClearRenderTargetView(&target.rtv, &[0.004, 0.004, 0.005, 1.0]);
            ctx.ClearRenderTargetView(&zrtv, &[0.0; 4]);
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone()), Some(zrtv.clone())]), &dsv);
            ctx.OMSetDepthStencilState(&self.depth_test, 0);
            ctx.OMSetBlendState(&self.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.RSSetState(&self.raster);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.VSSetSamplers(0, Some(&[Some(self.linear.clone()), Some(self.point.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.linear.clone()), Some(self.point.clone())]));
        }
        pipeline::viewport(ctx, target.width, target.height);
        self.program.bind(ctx);
        for s in sources {
            let [u0, v0, u1, v1] = s.rect;
            let (pw, ph) = (((u1 - u0) * s.width as f32).max(1.0) as u32, ((v1 - v0) * s.height as f32).max(1.0) as u32);
            let cell = s.cell.max(1);
            let cells = (pw.div_ceil(cell).max(1), ph.div_ceil(cell).max(1));
            let (ib, count) = self.grid(gpu, cells)?;
            let to_view = source_to_target_view(&s.pose, s.to_target, eye);
            let cb = WarpCb {
                to_clip: (proj * to_view).to_cols_array(),
                to_view: to_view.to_cols_array(),
                src_tan: [s.fov.left, s.fov.right, s.fov.up, s.fov.down],
                grid: [cells.0 as f32, cells.1 as f32, s.metres_per_unit, FARTHEST],
                opts: [s.mask.is_some() as u32 as f32, (s.props.is_some() && s.only <= 0.0) as u32 as f32, s.only, 0.0],
                rect: s.rect,
            };
            self.cb.update(ctx, &cb);
            unsafe {
                ctx.ClearDepthStencilView(&dsv, D3D11_CLEAR_DEPTH.0 as u32, 1.0, 0);
                ctx.VSSetShaderResources(0, Some(&[Some(s.color.clone()), Some(s.depth.clone())]));
                ctx.PSSetShaderResources(0, Some(&[Some(s.color.clone()), Some(s.depth.clone()), s.mask.cloned(), s.props.cloned()]));
                ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
                ctx.IASetIndexBuffer(&ib, DXGI_FORMAT_R32_UINT, 0);
                ctx.DrawIndexed(count, 0, 0);
            }
        }
        unsafe {
            ctx.VSSetShaderResources(0, Some(&[None, None]));
            ctx.PSSetShaderResources(0, Some(&[None, None, None, None]));
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

    /// A picture of a near red wall over a far blue one, the eye then stepped to the side: the near
    /// wall moves across the view more than the far one, by what parallax says.
    #[test]
    fn near_things_move_more_than_far_ones_when_the_eye_steps() {
        let gpu = Gpu::new().unwrap();
        let n = 64u32;
        // Left half: red at 1 m; right half: blue at 20 m.
        let mut color = vec![0u8; (n * n * 4) as usize];
        let mut depth = vec![0f32; (n * n) as usize];
        for y in 0..n {
            for x in 0..n {
                let i = (y * n + x) as usize;
                let near = x < n / 2;
                color[i * 4..i * 4 + 4].copy_from_slice(if near { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
                depth[i] = if near { 1.0 } else { 20.0 };
            }
        }
        let src = Texture::upload(&gpu, n, n, DXGI_FORMAT_R8G8B8A8_UNORM, &[&color]).unwrap();
        let src_depth = Texture::upload(&gpu, n, n, DXGI_FORMAT_R32_FLOAT, &[bytemuck::cast_slice(&depth)]).unwrap();
        let out = Texture::new(&gpu, &TexDesc { rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), ..TexDesc::target(n, n, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB) }).unwrap();
        let target = EyeTarget { tex: out.tex.clone(), rtv: out.rtv.clone().unwrap(), width: n, height: n };
        let fov = Fov { left: -1.0, right: 1.0, up: 1.0, down: -1.0 };
        let eye = Pose { pos: [0.0; 3], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
        let mut warp = Warp::new(&gpu).unwrap();
        let source = |to_target: Mat4| WarpSource {
            color: src.srv.as_ref().unwrap(),
            depth: src_depth.srv.as_ref().unwrap(),
            width: n,
            height: n,
            pose: eye,
            fov,
            to_target,
            metres_per_unit: 1.0,
            mask: None,
            props: None,
            only: 0.0,
            rect: WarpSource::WHOLE,
            cell: CELL_PIXELS,
        };
        // Where the red-blue edge is across the middle row.
        let edge = |warp: &mut Warp, to_target: Mat4| -> f32 {
            warp.draw(&gpu, &target, &eye, &fov, &[source(to_target)]).unwrap();
            let px = out.read_rgba8(&gpu).unwrap();
            let row = (n / 2) as usize;
            (0..n as usize).find(|&x| px[(row * n as usize + x) * 4 + 2] > 128).unwrap_or(n as usize) as f32
        };
        let still = edge(&mut warp, Mat4::IDENTITY);
        assert!((still - 32.0).abs() <= 1.0, "unmoved, the edge stays in the middle: {still}");
        // The world 40 cm to the left of the eye now (the eye stepped right): the near wall's edge
        // moves left by 0.4 / 1 of the half-width (13 px), the far one's by a twentieth of that;
        // the gap between is the far side stretched (the edge lands half way across it).
        let moved = edge(&mut warp, Mat4::from_translation(Vec3::new(-0.4, 0.0, 0.0)));
        assert!(moved < still - 4.0 && moved > still - 13.0, "the near red wall slid left over the blue: {still} -> {moved}");
    }
}
