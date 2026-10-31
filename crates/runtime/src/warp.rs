//! Moves a game's eye picture to where it's seen now: the head where it is at this headset frame,
//! and the player where the game has walked or turned them since, by the picture's depth.
//!
//! A game's picture is shown a frame or more after the frame it was drawn for (more when the game
//! is slower than the headset, and the same picture again when no new one has come). The headset
//! turns a picture for the head's turn since, but not for the head's step, nor for the player's
//! walk (that moves the world, not the head): the world would lag and step while the hands, drawn
//! by the runtime every frame, don't. Here each picture is laid over a grid, each grid point pushed
//! out to its depth, carried from where that eye was in the world (`SlotInfo::eye_map`) to where it
//! is now, and drawn from the eye's pose now: every headset frame shows the world where it is, as
//! a game drawing at the headset's rate would. A grid cell across a jump in depth (a near thing's
//! edge) can't follow the edge inside it: each of its pixels looks along its own line of sight
//! through the picture's depth, near where the grid put it, for the surface it really sees, so
//! edges stay as sharp as the game drew them. Where something near hid something far and the move
//! uncovers it, the other eye's picture fills it (it saw round the near thing's edge from 6 cm
//! away): what's left of a stretched cell goes behind everything real and shows only where nothing
//! else does. Past the picture's edge, its edge is drawn on outwards.
//!
//! It also writes what the runtime's body draw needs of the game's picture as it now shows: the
//! depth (metres along the new eye's view), and the game's bloom and lens moved with it.
//!
//! It costs the game little of the GPU: the grid is as fine as the headset's pixels (not the
//! game's, often larger), the eye's own picture goes first so the other eye's is only drawn where
//! it's seen, and the bloom and lens are moved only for a game that has them.

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
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT};

use crate::backend::{DepthTarget, EyeTarget};
use crate::compositor::{FAR, NEAR, projection, view_matrix};

const SHADER: &str = r#"
#ifndef BLOOM
#define BLOOM 0
#endif
cbuffer Warp : register(b0) {
    float4x4 proj;      // the target's view space -> its clip space
    float4x4 to_view;   // the source eye's view space (tracking metres) -> the target's view space
    float4x4 from_target; // the target's view space -> the source eye's (the edge fill's)
    float4x4 inv_proj;  // the target's clip space -> its view space
    float4 src_tan;     // the source picture's left, right, up, down tangents
    float4 grid;        // x: cells across, y: cells down, z: metres per depth unit (0: no depth), w: farthest point (metres)
    float4 src_info;    // x, y: the source picture's size (pixels), z: how much farther it's put (1: as it is)
    float4 target;      // x, y: the target's size (pixels)
};
Texture2D src : register(t0);
Texture2D<float> src_depth : register(t1);
Texture2D src_bloom : register(t2);
Texture2D src_lens : register(t3);
SamplerState samp : register(s0);
SamplerState pt : register(s1);

// (The picture's coordinates go across the screen as its pixels did: no perspective.)
struct WOut { float4 pos : SV_Position; noperspective float2 uv : TEXCOORD0; float vz : TEXCOORD1; noperspective float sd : TEXCOORD2; };

WOut VSMain(uint id : SV_VertexID) {
    uint across = (uint)grid.x + 1;
    float2 uv = float2(id % across, id / across) / grid.xy;
    float d = src_depth.SampleLevel(pt, uv, 0) * grid.z;
    float2 t = float2(lerp(src_tan.x, src_tan.y, uv.x), lerp(src_tan.z, src_tan.w, uv.y));
    // Nothing drawn, or past the far plane: a direction (the sky, the far distance).
    bool far = !(d > 0) || d > grid.w;
    float4 p = far ? float4(t, 1, 0) : float4(t * d, d, 1);
    float4 v = mul(to_view, p);
    // (The other eye's picture a little farther along each ray: where both show the same thing,
    // this eye's own wins.)
    if (!far) v.xyz *= src_info.z;
    WOut o;
    o.pos = mul(proj, v);
    if (far) o.pos.z = o.pos.w * 0.99999;
    o.uv = uv;
    o.vz = far ? 0 : v.z;
    o.sd = far ? grid.w : d;
    return o;
}

// What's written: the colour, the view depth, and (a game with them) the bloom and lens.
#if BLOOM
#define TARGETS float4 color : SV_Target0; float z : SV_Target1; float4 bloom : SV_Target2; float4 lens : SV_Target3;
#else
#define TARGETS float4 color : SV_Target0; float z : SV_Target1;
#endif
struct POut { TARGETS };
struct PDOut { TARGETS float depth : SV_Depth; };

// The picture over a target pixel's footprint (`dx`, `dy`: how far its `uv` goes a target pixel
// across and down). A game drawn sharper than the headset shows it (the render scale) has more
// pixels than the target: one tap a target pixel would skip the ones between, and fine detail (far
// branches, fences, the far distance) would flicker in and out of the taps as the head moves. Taps
// spread over the footprint average them, as a mip level would. (One tap at the headset's own
// sharpness or below.)
float3 footprint(float2 uv, float2 dx, float2 dy) {
    float s = max(length(dx * src_info.xy), length(dy * src_info.xy));
    int n = (int)clamp(ceil(s - 0.01), 1, 4);
    float3 c = 0;
    [loop] for (int y = 0; y < n; y++) {
        [loop] for (int x = 0; x < n; x++) {
            float2 o = (float2(x, y) + 0.5) / n - 0.5;
            c += src.SampleLevel(samp, uv + o.x * dx + o.y * dy, 0).rgb;
        }
    }
    return c / (n * n);
}

// The picture at `uv` into `o` (the bloom and lens are soft: one tap).
#if BLOOM
#define SAMPLE(o, uv, dx, dy) o.color = float4(footprint(uv, dx, dy), 1); o.bloom = src_bloom.SampleLevel(samp, uv, 0); o.lens = src_lens.SampleLevel(samp, uv, 0);
#else
#define SAMPLE(o, uv, dx, dy) o.color = float4(footprint(uv, dx, dy), 1);
#endif

// The source picture's point at `uv`, `d` metres along its view; and where a point is in it.
float3 src_point(float2 uv, float d) {
    return float3(lerp(src_tan.x, src_tan.y, uv.x), lerp(src_tan.z, src_tan.w, uv.y), 1) * d;
}
float2 src_uv(float3 p) {
    float2 t = p.xy / p.z;
    return float2((t.x - src_tan.x) / (src_tan.y - src_tan.x), (t.y - src_tan.z) / (src_tan.w - src_tan.z));
}
// The picture's depth at `uv` (metres; nothing drawn, or past the far plane: very far).
float depth_at(float2 uv) {
    int2 c = clamp(int2(uv * src_info.xy), int2(0, 0), int2(src_info.xy) - 1);
    float d = src_depth.Load(int3(c, 0)) * grid.z;
    return (d > 0 && d <= grid.w) ? d : 1e6;
}

// How far a misplaced pixel looks either side of where the grid put it, along its line of sight:
// this many steps of `STEP` source pixels (a near thing's edge moves up to some 20 pixels more
// than what's behind it over a slow game's frame or two).
#define SEARCH 16
#define STEP 1.5
// 1 / the nearest a line of sight is followed from (5 cm).
#define WMAX 20.0

// A cell stretched across a jump in the picture's depth (a near thing's edge over what's far behind
// it, pulled apart by the move) is not a surface; and a pixel whose own depth isn't the cell's (the
// edge of a near thing inside a cell whose corners are all on what's behind it, or the other way
// round) was moved as something it isn't.
bool astray(WOut i) {
    if (grid.z <= 0) return false;
    float2 px = i.uv * src_info.xy;
    float dd = length(float2(ddx(i.sd), ddy(i.sd)));
    float dpx = max(length(ddx(px)) + length(ddy(px)), 1e-3);
    if (dd / dpx > 0.06 * max(i.sd, 0.05)) return true;
    float own = depth_at(i.uv);
    float cell = i.sd >= grid.w ? 1e6 : i.sd;
    if ((own >= 1e6) != (cell >= 1e6)) return true;
    return own < 1e6 && abs(own - cell) > 0.05 * cell + 0.01;
}

// The eye's own picture. Each pixel the grid moved astray finds what it really sees: along its own
// line of sight (in the source picture, a line through where the grid put it), the nearest place
// the line meets the picture's depth.
PDOut PSMain(WOut i) {
    PDOut o;
    float2 uv = i.uv;
    // (The footprint from where the grid put the pixel, smooth across the cells; taken before
    // anything branches.)
    float2 dx = ddx(i.uv), dy = ddy(i.uv);
    o.z = i.vz;
    o.depth = i.pos.z;
    if (astray(i)) {
        // This pixel's line of sight in the source's view space: C + t D, t its depth in the
        // target's view (the other eye's put farther: undone here, done again on what's found).
        float2 ndc = float2(i.pos.x / target.x * 2 - 1, 1 - i.pos.y / target.y * 2);
        float4 tv = mul(inv_proj, float4(ndc, 1, 1));
        float3 ray = tv.xyz / tv.w;
        ray /= ray.z;
        float3 C = mul(from_target, float4(0, 0, 0, 1)).xyz;
        float3 D = mul(from_target, float4(ray, 0)).xyz;
        // Stepped by a source pixel at a time (in 1 / t, which moves along the line evenly), from
        // near to far round where the grid put it: no nearer than 5 cm (`WMAX`), out to the far
        // distance (w = 0).
        float w0 = 1 / max(i.vz / src_info.z, 1 / WMAX);
        float2 s0 = src_uv(C + D / w0);
        float2 s1 = src_uv(C + D / (w0 * 1.01));
        float px_per_w = length((s1 - s0) * src_info.xy) / (w0 * 0.01);
        float prev_w = 0, prev_r = 0, prev_z = 0;
        bool have_prev = false, found = false, past = false, still = false, have_far = false;
        float2 far_uv = i.uv;
        float hit_w = 0;
        if (px_per_w * WMAX < 1) {
            // The line of sight hardly crosses the picture (the eye has hardly moved since it was
            // drawn): what it sees is what's at the one place it crosses, whatever its depth.
            still = true;
        }
        float step_w = min(STEP / max(px_per_w, 1e-6), WMAX / (2 * SEARCH));
        float w_hi = min(w0 + SEARCH * step_w, WMAX);
        [loop] for (int k = 0; k <= 2 * SEARCH && !still; k++) {
            float w = w_hi - k * step_w;
            if (w <= 1e-4) {
                past = true;
                break;
            }
            float3 p = C + D / w;
            if (p.z <= 0.01) continue;
            float2 s = src_uv(p);
            float r = p.z - depth_at(s);
            // (In front of what the picture shows there, then behind it: the surface is between,
            // unless what the picture shows jumped nearer from one step to the next: the line only
            // passed behind a nearer thing's edge, and goes on past it. A surface seen edge-on
            // changes depth fast but smoothly, and is met.)
            float z = p.z - r;
            if (have_prev && prev_r < 0 && r >= 0) {
                float z_prev = prev_z;
                if (z < 0.85 * z_prev - 0.02) {
                    // Behind a nearer thing's edge here: what it hides is most likely what the line
                    // saw just before, carried on behind it (the far side, not the near thing).
                    // (A pixel and a half back from the edge, clear of its blend.)
                    if (!have_far) {
                        float2 before = src_uv(C + D / prev_w);
                        float2 back = before - s;
                        far_uv = before + back / max(length(back * src_info.xy), 1e-3) * 1.5;
                    }
                    have_far = true;
                } else {
                    float lo = w, hi = prev_w;
                    [unroll] for (int b = 0; b < 5; b++) {
                        float mid = 0.5 * (lo + hi);
                        float3 q = C + D / mid;
                        if (q.z - depth_at(src_uv(q)) >= 0) lo = mid; else hi = mid;
                    }
                    hit_w = hi;
                    found = true;
                    break;
                }
            }
            prev_z = z;
            prev_w = w;
            prev_r = r;
            have_prev = true;
        }
        if (still) {
            uv = s0;
            float z = depth_at(s0);
            if (z >= 1e6) {
                o.depth = 0.99999;
                o.z = 0;
            } else {
                float t = max((z - C.z) / max(D.z, 1e-4), 0.01);
                float4 c = mul(proj, float4(ray * t * src_info.z, 1));
                o.depth = c.z / c.w;
                o.z = t * src_info.z;
            }
        } else if (found) {
            float t = 1 / hit_w;
            uv = src_uv(C + D * t);
            float4 c = mul(proj, float4(ray * t * src_info.z, 1));
            o.depth = c.z / c.w;
            o.z = t * src_info.z;
        } else if ((past || (have_prev && prev_r < 0)) && D.z > 0 && depth_at(src_uv(D)) >= 1e6) {
            // In front of everything along the way, and nothing at all straight on: the far
            // distance (the sky).
            uv = src_uv(D);
            o.depth = 0.99999;
            o.z = 0;
        } else {
            // Nothing found (hidden from this eye when it was drawn): behind everything real, so the
            // other eye's picture shows here where it saw the far side, and this only where nothing
            // does, as the far side carried on behind the near thing.
            if (have_far) uv = far_uv;
            o.depth = 0.999998;
        }
    }
    SAMPLE(o, uv, dx, dy)
    return o;
}

// The other eye's picture, drawn after: only where the eye's own has nothing real (its edges, what
// a move uncovers); its own strays are left out.
POut PSOther(WOut i) {
    float2 dx = ddx(i.uv), dy = ddy(i.uv);
    if (astray(i)) discard;
    POut o;
    o.z = i.vz;
    SAMPLE(o, i.uv, dx, dy)
    return o;
}

// Past the picture's edges (a head turned further than it was drawn wide): each direction looked
// up in the picture as a direction alone, its edge drawn on outwards, and no depth (nothing the
// body hides behind).
struct FOut { float4 pos : SV_Position; float2 ndc : TEXCOORD0; };
FOut VSFill(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    FOut o;
    o.pos = float4(uv * float2(2, -2) + float2(-1, 1), 1, 1);
    o.ndc = o.pos.xy;
    return o;
}
POut PSFill(FOut i) {
    float4 v = mul(inv_proj, float4(i.ndc, 1, 1));
    float3 d = mul(from_target, float4(v.xyz / v.w, 0)).xyz;
    POut o;
    o.z = 0;
#if BLOOM
    o.bloom = 0;
    o.lens = 0;
#endif
    if (d.z <= 1e-4) {
        o.color = float4(0.004, 0.004, 0.005, 1);
        return o;
    }
    float2 t = d.xy / d.z;
    float2 uv = saturate(float2((t.x - src_tan.x) / (src_tan.y - src_tan.x), (t.y - src_tan.z) / (src_tan.w - src_tan.z)));
    o.color = float4(src.SampleLevel(samp, uv, 0).rgb, 1);
    return o;
}

// The headset's depth for one eye: the view depth (`src_depth`, metres times `grid.z` along the
// eye's view, 0 where nothing was drawn) as the compositor's projection keeps it.
float PSHeadsetDepth(FOut i) : SV_Depth {
    uint w, h;
    src_depth.GetDimensions(w, h);
    float2 uv = float2(i.ndc.x * 0.5 + 0.5, 0.5 - i.ndc.y * 0.5);
    float z = src_depth.Load(int3(min(uint2(uv * float2(w, h)), uint2(w - 1, h - 1)), 0)) * grid.z;
    if (!(z > 0)) return 1;
    // (D3D's depth for the projection's near and far planes: `grid.w` near, `src_tan.x` far.)
    float n = grid.w, f = src_tan.x;
    return saturate(f / (f - n) * (1 - n / max(z, n)));
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WarpCb {
    proj: [f32; 16],
    to_view: [f32; 16],
    from_target: [f32; 16],
    inv_proj: [f32; 16],
    src_tan: [f32; 4],
    grid: [f32; 4],
    src: [f32; 4],
    target: [f32; 4],
}

/// How much farther along each ray the other eye's picture is put (its depth's share): where both
/// eyes show the same surface, the eye's own picture is the one seen.
const OTHER_EYE_BACK: f32 = 1.01;

/// Grid points every this many of the target's pixels (the source's, scaled to them).
pub const CELL_PIXELS: u32 = 4;
/// Points farther than this (metres) are drawn as directions (the projection's far plane is 200 m).
const FARTHEST: f32 = 190.0;

/// One eye picture to move.
#[derive(Clone, Copy)]
pub struct WarpSource<'a> {
    pub color: &'a ID3D11ShaderResourceView,
    /// Its depth: distance along its view, in units of `metres_per_unit` metres; 0: nothing drawn.
    /// None: no depth (the picture is turned only, as if everything were far).
    pub depth: Option<&'a ID3D11ShaderResourceView>,
    /// The game's bloom and lens over it ([`rustvr_avatar::DepthInput`]), moved with it.
    pub bloom: Option<&'a ID3D11ShaderResourceView>,
    pub lens: Option<&'a ID3D11ShaderResourceView>,
    pub width: u32,
    pub height: u32,
    /// The tracking-space eye it was rendered from, and its field of view.
    pub pose: Pose,
    pub fov: Fov,
    /// Its tracking space -> the target's (the player walked or turned in between).
    pub to_target: Mat4,
    pub metres_per_unit: f32,
}

/// What [`Warp::draw`] wrote besides the colour, for a target its size: the depth seen from its
/// eye (metres along its view, 0 where the picture doesn't reach), and the bloom and lens.
pub struct Post {
    pub depth: ID3D11ShaderResourceView,
    pub bloom: ID3D11ShaderResourceView,
    pub lens: ID3D11ShaderResourceView,
}

/// The depth buffer and the seen-from-here depth, bloom and lens for one target size.
struct Targets {
    width: u32,
    height: u32,
    depth: Texture,
    view_z: Texture,
    bloom: Texture,
    lens: Texture,
}

/// One way of drawing (`BLOOM`: with the game's bloom and lens, or without).
struct Programs {
    own: Program,
    other: Program,
    fill: Program,
}

impl Programs {
    fn new(gpu: &Gpu, bloom: bool) -> Result<Programs> {
        let src = format!("#define BLOOM {}\n{SHADER}", bloom as u32);
        Ok(Programs {
            own: Program::with_entries(gpu, &src, "warp", "VSMain", "PSMain", &[])?,
            other: Program::with_entries(gpu, &src, "warp", "VSMain", "PSOther", &[])?,
            fill: Program::with_entries(gpu, &src, "warp", "VSFill", "PSFill", &[])?,
        })
    }
}

pub struct Warp {
    /// Without the bloom and lens, and with them.
    programs: [Programs; 2],
    headset_depth: Program,
    cb: ConstantBuffer<WarpCb>,
    linear: ID3D11SamplerState,
    point: ID3D11SamplerState,
    depth_test: ID3D11DepthStencilState,
    no_depth: ID3D11DepthStencilState,
    write_depth: ID3D11DepthStencilState,
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
        // (Every pixel written, whatever was there: the headset's depth.)
        let write_depth = {
            let desc = D3D11_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D11_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D11_COMPARISON_ALWAYS,
                StencilEnable: false.into(),
                StencilReadMask: 0xFF,
                StencilWriteMask: 0xFF,
                FrontFace: D3D11_DEPTH_STENCILOP_DESC::default(),
                BackFace: D3D11_DEPTH_STENCILOP_DESC::default(),
            };
            let mut s = None;
            unsafe { gpu.device.CreateDepthStencilState(&desc, Some(&mut s))? };
            s.unwrap()
        };
        Ok(Warp {
            programs: [Programs::new(gpu, false)?, Programs::new(gpu, true)?],
            headset_depth: Program::with_entries(gpu, SHADER, "warp", "VSFill", "PSHeadsetDepth", &[])?,
            cb: ConstantBuffer::new(gpu)?,
            linear: pipeline::sampler(gpu, true, false)?,
            point: pipeline::sampler(gpu, false, false)?,
            depth_test: pipeline::depth_state(gpu, true, true)?,
            no_depth: pipeline::depth_state(gpu, false, false)?,
            write_depth,
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
        let tex = |f| Texture::new(gpu, &TexDesc::target(width, height, f));
        self.targets.push(Targets {
            width,
            height,
            depth: Texture::new(gpu, &TexDesc::depth(width, height))?,
            view_z: tex(DXGI_FORMAT_R32_FLOAT)?,
            bloom: tex(DXGI_FORMAT_R16G16B16A16_FLOAT)?,
            lens: tex(DXGI_FORMAT_R16G16B16A16_FLOAT)?,
        });
        Ok(self.targets.len() - 1)
    }

    /// What [`Warp::draw`] wrote for a target this size besides the colour. Valid after the draw,
    /// until the next.
    pub fn post(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<Post> {
        let i = self.targets(gpu, width, height)?;
        let t = &self.targets[i];
        Ok(Post { depth: t.view_z.srv.clone().unwrap(), bloom: t.bloom.srv.clone().unwrap(), lens: t.lens.srv.clone().unwrap() })
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

    /// Draws `sources` into `target` as seen from `eye` with `fov`: the last one (the eye's own
    /// picture) with its edge drawn on outwards, then the others (the other eye's) where it has
    /// nothing real: its edges, and what the move uncovers.
    pub fn draw(&mut self, gpu: &Gpu, target: &EyeTarget, eye: &Pose, fov: &Fov, sources: &[WarpSource]) -> Result<()> {
        let ctx = &gpu.ctx;
        let ti = self.targets(gpu, target.width, target.height)?;
        let t = &self.targets[ti];
        let dsv = t.depth.dsv.clone().unwrap();
        // (The bloom and lens only for a game that has them.)
        let bloom = sources.iter().any(|s| s.bloom.is_some() || s.lens.is_some());
        let rtvs = [Some(target.rtv.clone()), t.view_z.rtv.clone(), t.bloom.rtv.clone(), t.lens.rtv.clone()];
        let rtvs = &rtvs[..if bloom { 4 } else { 2 }];
        let proj = projection(fov);
        let inv_proj = proj.inverse();
        unsafe {
            for rtv in rtvs.iter().skip(1).flatten() {
                ctx.ClearRenderTargetView(rtv, &[0.0; 4]);
            }
            ctx.OMSetRenderTargets(Some(rtvs), &dsv);
            ctx.OMSetBlendState(&self.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.RSSetState(&self.raster);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.VSSetSamplers(0, Some(&[Some(self.linear.clone()), Some(self.point.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.linear.clone()), Some(self.point.clone())]));
        }
        pipeline::viewport(ctx, target.width, target.height);
        let cb_for = |s: &WarpSource, cells: (u32, u32), back: f32| {
            let to_view = source_to_target_view(&s.pose, s.to_target, eye);
            WarpCb {
                proj: proj.to_cols_array(),
                to_view: to_view.to_cols_array(),
                from_target: to_view.inverse().to_cols_array(),
                inv_proj: inv_proj.to_cols_array(),
                src_tan: [s.fov.left, s.fov.right, s.fov.up, s.fov.down],
                grid: [cells.0 as f32, cells.1 as f32, if s.depth.is_some() { s.metres_per_unit } else { 0.0 }, FARTHEST],
                src: [s.width as f32, s.height as f32, back, 0.0],
                target: [target.width as f32, target.height as f32, 0.0, 0.0],
            }
        };
        let programs = &self.programs[bloom as usize];
        // The edge drawn on, behind everything.
        if let Some(s) = sources.last() {
            self.cb.update(ctx, &cb_for(s, (1, 1), 1.0));
            programs.fill.bind(ctx);
            unsafe {
                ctx.OMSetDepthStencilState(&self.no_depth, 0);
                ctx.PSSetShaderResources(0, Some(&[Some(s.color.clone())]));
                ctx.Draw(3, 0);
                ctx.OMSetDepthStencilState(&self.depth_test, 0);
                ctx.ClearDepthStencilView(&dsv, D3D11_CLEAR_DEPTH.0 as u32, 1.0, 0);
            }
        }
        // The eye's own picture, then the other's where it has nothing real.
        let order = sources.iter().rev().enumerate();
        let mut grids = Vec::new();
        for (k, s) in order {
            // (Grid points a few of the target's pixels apart, however large the game's picture.)
            let scale = (s.width as f32 / target.width.max(1) as f32).max(1.0);
            let cell = ((CELL_PIXELS as f32 * scale).round() as u32).max(1);
            let cells = (s.width.div_ceil(cell).max(1), s.height.div_ceil(cell).max(1));
            grids.push((k, cells));
        }
        for (k, cells) in grids {
            let s = &sources[sources.len() - 1 - k];
            let own = k == 0;
            let (ib, count) = self.grid(gpu, cells)?;
            let programs = &self.programs[bloom as usize];
            (if own { &programs.own } else { &programs.other }).bind(ctx);
            self.cb.update(ctx, &cb_for(s, cells, if own { 1.0 } else { OTHER_EYE_BACK }));
            let srvs = [Some(s.color.clone()), s.depth.cloned(), s.bloom.cloned(), s.lens.cloned()];
            unsafe {
                ctx.VSSetShaderResources(0, Some(&srvs[..2]));
                ctx.PSSetShaderResources(0, Some(&srvs));
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

    /// Writes one eye's depth for the headset (`target`) from a view depth (`depth`: along the eye's
    /// view, `metres_per_unit` metres a unit, 0 where nothing is), every pixel, as the compositor's
    /// projection keeps it. Then the runtime's body goes in where it's nearer.
    pub fn headset_depth(&mut self, gpu: &Gpu, target: &DepthTarget, depth: Option<&ID3D11ShaderResourceView>, metres_per_unit: f32) {
        let ctx = &gpu.ctx;
        let cb = WarpCb { src_tan: [FAR, 0.0, 0.0, 0.0], grid: [0.0, 0.0, if depth.is_some() { metres_per_unit } else { 0.0 }, NEAR], ..WarpCb::zeroed() };
        self.cb.update(ctx, &cb);
        unsafe {
            ctx.OMSetRenderTargets(None, &target.dsv);
            ctx.OMSetDepthStencilState(&self.write_depth, 0);
            ctx.OMSetBlendState(&self.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.RSSetState(&self.raster);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
        }
        pipeline::viewport(ctx, target.width, target.height);
        self.headset_depth.bind(ctx);
        unsafe {
            ctx.PSSetShaderResources(1, Some(&[depth.cloned()]));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(1, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
    }
}

