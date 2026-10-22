//! A collimator sight's lens and reticle, drawn as Metro 2033 Redux draws them, collimated for
//! each eye (docs/research/metro-collimator.md).
//!
//! **What the game does** (measured on the game, the Kalash 2012's `att_kollimator`):
//! after the deferred lighting, four forward draws over the HDR picture, in this order:
//!
//! 1. The **reticle**: a 4.4 mm quad 2.3 cm behind the lens, blended
//!    `SrcAlpha/InvSrcAlpha` with its texel (`rgb = base.rgb, a = base.a`): the red ring in the
//!    sight's atlas `wpn34_kollimator`, whose alpha peaks at 0.5.
//! 2. The lens's **reflecting surface**: the environment cube along the view
//!    reflected off the lens (× `envmap_color`, × (1 − base.a)) plus the base texel, plus a
//!    screen-space reflection of the previous frame's blurred picture; blended with base.a.
//! 3. The **lens pane** (`multiply2x`): what's behind × 2·base.rgb (blend
//!    `DestColor/SrcColor`): a yellowish vignette (×1.74, ×1.57, ×0.84 at the centre, ×0.69 at
//!    the rim) that tints everything seen through the sight, the reticle included.
//! 4. The **refraction**: screen-space offsets into a distortion buffer, which
//!    the final pass samples the whole picture through (`uv += dudv·0.035`).
//!
//! Then the final pass tonemaps (Hable, normalised to 1), adds bloom and grades.
//!
//! **What this draws in VR**: the same four steps with the same numbers, in each eye, over the
//! eye's picture. Two things change, both forced by the headset:
//!
//! * The reticle is **collimated**: a collimator's reticle is an image at optical infinity, so
//!   both eyes must see it in the same direction (the sight's axis, which is where the runtime
//!   aims the game's shots) wherever they are behind the lens. The flat game can simply put a quad
//!   2.3 cm behind the lens; in stereo that quad sits 0.6 m from the eyes and the two eyes disagree
//!   about where it points. Rule: **the reticle is the game's quad as the game's aimed camera sees
//!   it, along each pixel's own direction**: for a pixel of the lens pane whose ray from the eye
//!   has direction `d` (sight space), the reticle texel is where the ray from the aimed camera
//!   point `E = centre − D·axis` with that same direction meets the quad (`D`: the aimed camera's
//!   distance from the quad, the extractor's `Material::params.x`, from the gun's aim motion).
//!   Every eye position gives the same texel for the same direction: optical infinity, with the
//!   angular size the game gives the reticle when it aims. It is seen only through the pane,
//!   as through a real collimator's window. [`ReticleMode::Quad`] draws the game's flat quad
//!   instead (the same formula with `E` the eye itself), for comparing with the game's frames.
//! * The **picture behind the lens** is the eye's picture (the game's world and what the runtime
//!   drew), which holds display code values, not the game's HDR: it is brought back to HDR by
//!   inverting the game's grading and tone curve exactly ([`inverse_post`]), the four steps run
//!   on that, and the result goes back through the game's post ([`crate::metro::post`]). Exact
//!   up to 8-bit quantisation for what the runtime drew; for the world, the game's bloom is
//!   already in the picture, so it gets tinted by the pane too (the game adds it after). With the
//!   game's own pre-post picture and bloom ([`Scene`], an adapter's job) it is the game's chain.
//!   The reflection's screen-space term reads the eye's own picture (4×4 average) in place of the
//!   game's previous-frame blur buffer.
//!
//! Everything else is Metro's own arithmetic, as the game's shaders do it, with their numbers,
//! measured on the game and checked against its render targets pass by pass.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4, Vec4Swizzles};
use rustvr_asset as asset;
use rustvr_gpu::pipeline::{self, ConstantBuffer, static_buffer};
use rustvr_gpu::shader::{Program, element};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::LUT_SIZE;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::metro::{self, LUM};
use crate::{GameShading, SpecialDraw};

/// The final pass's refraction scale: `uv += dudv.xy · 0.035`.
pub const REFRACTION_SCALE: f32 = 0.035;
/// The screen-space reflection's mapping of the reflected direction (step 2's).
const SSR_SCALE: f32 = 0.543478;
const SSR_FADE: f32 = 1.086957;

// ---- the CPU reference ------------------------------------------------------------------------

/// How the reticle is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReticleMode {
    /// At optical infinity along the sight's axis, as the game's aimed camera sees it (VR).
    #[default]
    Collimated,
    /// The game's own quad behind the lens, seen from the eye (flat; for comparing with the game's frames).
    Quad,
}

/// The reticle quad: its centre, plane and texture mapping (the reticle mesh's sight space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReticleQuad {
    pub centre: Vec3,
    /// Unit normal of its plane, pointing away from the eye (the sight's axis).
    pub axis: Vec3,
    /// In-plane unit axes and the half extents along them.
    pub ex: Vec3,
    pub ey: Vec3,
    pub half: Vec2,
    /// uv = `uv_centre + uv_x·q.x + uv_y·q.y` for in-plane offsets `q` (metres).
    pub uv_centre: Vec2,
    pub uv_x: Vec2,
    pub uv_y: Vec2,
    /// The game's aimed camera's distance from the quad's plane (metres; 0: unknown).
    pub aimed_distance: f32,
}

impl ReticleQuad {
    /// From the reticle mesh: its vertices' bounds, normals and uvs (an affine fit, exact for the
    /// game's quad).
    pub fn from_mesh(m: &asset::Mesh) -> Option<ReticleQuad> {
        let used: Vec<&asset::Vertex> = m.indices.iter().map(|&i| &m.vertices[i as usize]).collect();
        if used.len() < 3 {
            return None;
        }
        // The axis is the quad's plane normal from its positions (its vertex normals are 8-bit,
        // 0.2° off), pointing away from the side its normals face (the camera's).
        let n = used.iter().fold(Vec3::ZERO, |a, v| a + Vec3::from_array(v.normal));
        let mut plane = Vec3::ZERO;
        for t in m.indices.as_chunks::<3>().0 {
            let p = |k: usize| Vec3::from_array(m.vertices[t[k] as usize].pos);
            let c = (p(1) - p(0)).cross(p(2) - p(0));
            plane += if c.dot(plane) < 0.0 { -c } else { c };
        }
        let plane = plane.try_normalize()?;
        let axis = if plane.dot(n) > 0.0 { -plane } else { plane };
        let ex = (Vec3::X - axis * axis.dot(Vec3::X)).try_normalize()?;
        let ey = axis.cross(ex);
        let (lo, hi) = used.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), v| (lo.min(Vec3::from_array(v.pos)), hi.max(Vec3::from_array(v.pos))));
        let centre = (lo + hi) * 0.5;
        // Least squares for uv = c + A·q over the vertices (q in-plane offsets).
        let rows: Vec<(Vec3, Vec2)> = used.iter().map(|v| {
            let d = Vec3::from_array(v.pos) - centre;
            (Vec3::new(1.0, d.dot(ex), d.dot(ey)), Vec2::from_array(v.uv))
        }).collect();
        let mut ata = Mat3::ZERO;
        let mut atu = Vec3::ZERO;
        let mut atv = Vec3::ZERO;
        for (r, uv) in &rows {
            ata += Mat3::from_cols(*r * r.x, *r * r.y, *r * r.z);
            atu += *r * uv.x;
            atv += *r * uv.y;
        }
        if ata.determinant().abs() < 1e-18 {
            return None;
        }
        let inv = ata.inverse();
        let (su, sv) = (inv * atu, inv * atv);
        let half = Vec2::new(rows.iter().map(|(r, _)| r.y.abs()).fold(0.0, f32::max), rows.iter().map(|(r, _)| r.z.abs()).fold(0.0, f32::max));
        Some(ReticleQuad {
            centre,
            axis,
            ex,
            ey,
            half,
            uv_centre: Vec2::new(su.x, sv.x),
            uv_x: Vec2::new(su.y, sv.y),
            uv_y: Vec2::new(su.z, sv.z),
            aimed_distance: m.material.params[0],
        })
    }

    /// The point the reticle is seen from: the game's aimed camera (collimated) or the eye.
    pub fn viewpoint(&self, mode: ReticleMode, eye: Vec3) -> Vec3 {
        match mode {
            ReticleMode::Collimated => self.centre - self.axis * self.aimed_distance,
            ReticleMode::Quad => eye,
        }
    }

    /// The reticle's texture coordinate for a ray of direction `dir` (sight space) from the eye,
    /// or None where the ray misses the quad.
    pub fn uv(&self, mode: ReticleMode, eye: Vec3, dir: Vec3) -> Option<Vec2> {
        let from = self.viewpoint(mode, eye);
        let den = dir.dot(self.axis);
        if den <= 1e-6 {
            return None;
        }
        let l = from + dir * ((self.centre - from).dot(self.axis) / den) - self.centre;
        let q = Vec2::new(l.dot(self.ex), l.dot(self.ey));
        if q.x.abs() > self.half.x || q.y.abs() > self.half.y {
            return None;
        }
        Some(self.uv_centre + self.uv_x * q.x + self.uv_y * q.y)
    }

    /// The reticle's angular half-size (radians) once collimated.
    pub fn angular_half(&self) -> Vec2 {
        Vec2::new((self.half.x / self.aimed_distance).atan(), (self.half.y / self.aimed_distance).atan())
    }
}

/// Step 1, the reticle: blended over `dst` with its texel's alpha.
pub fn reticle(dst: Vec3, texel: Vec4) -> Vec3 {
    dst.lerp(texel.xyz(), texel.w)
}

/// Step 2, the lens's reflecting surface: its colour and opacity. `r` is the reflected
/// view direction in the eye's view space (x right, y up, z forward; the game's `R` before
/// `m_iV`), `env` the environment cube's texel along it (in the game's world), `blur` the
/// reflection source at [`ssr_uv`] (the game's previous-frame blur buffer, `rgb / a`), `aspect`
/// the eye's width / height.
pub fn reflection(texel: Vec4, env: Vec3, envmap_color: Vec3, blur: Vec3, r: Vec3, aspect: f32) -> Vec4 {
    let mut c = env * envmap_color * (1.0 - texel.w) + texel.xyz();
    let b = blur.clamp(Vec3::ZERO, Vec3::ONE);
    let s = Vec2::new(r.x / r.z, r.y / r.z);
    let fade = (1.0 - Vec2::new(s.x * SSR_FADE, s.y * aspect).length()).max(0.0).sqrt();
    c += fade * b * b.dot(LUM);
    c.extend(texel.w)
}

/// Where step 2 samples the reflection source (uv over the eye's picture).
pub fn ssr_uv(r: Vec3, aspect: f32) -> Vec2 {
    Vec2::new(r.x / r.z * SSR_SCALE + 0.5, r.y / r.z * aspect * -0.5 + 0.5)
}

/// Step 3, the lens pane (blend `DestColor/SrcColor`): `dst · 2·texel.rgb`.
pub fn pane(dst: Vec3, texel: Vec4) -> Vec3 {
    2.0 * texel.xyz() * dst
}

/// The refraction's vertex term: `saturate(1.2·|dot(normalize(P), N)| − 0.1)` with
/// `P`, `N` the vertex's view-space position and unit normal.
pub fn refraction_facing(p: Vec3, n: Vec3) -> f32 {
    (1.2 * p.normalize().dot(n).abs() - 0.1).clamp(0.0, 1.0)
}

/// Step 4, the refraction: the offset it writes (`xy`, in the units the final pass
/// scales by [`REFRACTION_SCALE`]) and its blend opacity. `z`: the fragment's view depth;
/// `facing`: [`refraction_facing`]; `scene_z`: the scene's view depth at the offset pixel
/// (nearest of the 2×2 there), None when unknown (taken as far).
pub fn refraction(texel: Vec4, z: f32, facing: f32, scene_z: Option<f32>) -> (Vec2, f32) {
    let k = (4.0 / z).clamp(0.0, 2.0);
    let d = (Vec2::new(texel.x, texel.y) * 2.0 - Vec2::ONE) * k * (facing + 0.5);
    let f = scene_z.map_or(1.0, |s| (10.0 * (s - z) + 0.01).clamp(0.0, 1.0));
    (d * f, (1.25 - facing).clamp(0.0, 1.0) * texel.w)
}

/// The inverse of the game's tone curve ([`metro::tonemap`]) on [0, 1]: the HDR value it maps to
/// `tm` (1 and above: 1, the brightest the game displays).
pub fn tonemap_inverse(tm: f32) -> f32 {
    if tm >= 1.0 {
        return 1.0;
    }
    let y = tm / 4.531915 + 0.066667;
    let (a, b, c) = (0.15 * (y - 1.0), 0.5 * y - 0.05, 0.06 * y - 0.004);
    let d = (b * b - 4.0 * a * c).max(0.0);
    (2.0 * c / (-b + d.sqrt())).max(0.0)
}

/// The HDR colour the game's post ([`metro::post`] without bloom) turned into the displayed
/// `code`: its grading undone by fixed-point iteration (the table is smooth: 4 steps), then the
/// tone curve.
pub fn inverse_post(code: Vec3, lut: Option<&[u32]>) -> Vec3 {
    let mut tm = code;
    if let Some(lut) = lut {
        for _ in 0..4 {
            let l = metro::lut_sample(lut, Vec3::new(tm.z, tm.y, tm.x));
            tm = code - (2.0 * Vec3::new(l.z, l.y, l.x) - Vec3::ONE);
        }
    }
    let tm = tm.clamp(Vec3::ZERO, Vec3::ONE);
    Vec3::new(tonemap_inverse(tm.x), tonemap_inverse(tm.y), tonemap_inverse(tm.z))
}

/// The game's final pass for a lens pixel: the refracted HDR `c` with the game's bloom term
/// `bloom` (its `t_bloom_b` × lens dirt, 0 without), tonemapped and graded: the displayed code.
pub fn post_with_bloom(c: Vec3, bloom: Vec3, lut: Option<&[u32]>) -> Vec3 {
    let x = (c + bloom).max(Vec3::ZERO);
    let tm = Vec3::new(metro::tonemap(x.x), metro::tonemap(x.y), metro::tonemap(x.z)) + 0.125 * bloom;
    metro::grade(tm, lut)
}

// ---- the collimator's meshes ------------------------------------------------------------------

/// Which of a model's meshes a collimator draws: its pane ([`asset::Shader::GlassMultiply`]), its
/// reticle ([`asset::Shader::Reticle`]), the reflecting glass and refraction over the pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Parts {
    pub pane: usize,
    pub reticle: usize,
    pub reflect: Vec<usize>,
    pub distort: Vec<usize>,
}

/// The collimator in `model` (the first pane with a reticle behind it), if it has one.
pub fn parts(model: &asset::Model) -> Option<Parts> {
    use asset::Shader;
    let bounds = |m: &asset::Mesh| {
        m.indices.iter().map(|&i| Vec3::from_array(m.vertices[i as usize].pos)).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(p), hi.max(p)))
    };
    let reticle = model.meshes.iter().position(|m| m.material.shader == Shader::Reticle && !m.indices.is_empty())?;
    let (rlo, rhi) = bounds(&model.meshes[reticle]);
    let pane = model.meshes.iter().position(|m| {
        if m.material.shader != Shader::GlassMultiply || m.indices.is_empty() {
            return false;
        }
        let (lo, hi) = bounds(m);
        rlo.x >= lo.x && rhi.x <= hi.x && rlo.y >= lo.y && rhi.y <= hi.y
    })?;
    let (plo, phi) = bounds(&model.meshes[pane]);
    // Over the pane: its outline (with a millimetre for rounding) and within 3 cm of it.
    let over = |m: &asset::Mesh| {
        let (lo, hi) = bounds(m);
        let e = 0.001;
        lo.x >= plo.x - e && hi.x <= phi.x + e && lo.y >= plo.y - e && hi.y <= phi.y + e && lo.z > plo.z - 0.03 && hi.z < phi.z + 0.03
    };
    let of = |s: Shader| model.meshes.iter().enumerate().filter(|(_, m)| m.material.shader == s && !m.indices.is_empty() && over(m)).map(|(i, _)| i).collect::<Vec<_>>();
    Some(Parts { pane, reticle, reflect: of(Shader::GlassReflect), distort: of(Shader::Distort) })
}

/// Whether the collimator draw takes mesh `index` of `model` (the avatar leaves it to the
/// special draw): the game draws a collimator's reticle, reflection, pane and refraction in that
/// order, so all of them are drawn together.
pub fn takes(model: &asset::Model, index: usize) -> bool {
    parts(model).is_some_and(|p| p.pane == index || p.reticle == index || p.reflect.contains(&index) || p.distort.contains(&index))
}

// ---- the GPU side -----------------------------------------------------------------------------

const SHADER: &str = r#"
cbuffer Coll : register(b0) {
    float4x4 view_proj;   // tracking -> clip
    float4x4 model;       // sight (model) space -> tracking
    float4x4 view;        // tracking -> the eye's view space
    float4 world_rot[3];  // tracking -> the game's world (rows)
    float4 eye_t;         // the eye, tracking space
    float4 eye_m;         // the eye, model space
    float4 from_m;        // the point the reticle is seen from (model space)
    float4 ret_centre;
    float4 ret_axis;
    float4 ret_ex;
    float4 ret_ey;
    float4 ret_half;      // xy
    float4 ret_uv;        // xy: uv at the centre; zw unused
    float4 ret_uv_m;      // (du/dq.x, dv/dq.x, du/dq.y, dv/dq.y)
    float4 target;        // w, h, 1/w, 1/h
    float4 envc;          // rgb: envmap_color; w: grading table valid
    float4 depth;         // x: game depth bound, y: metres per game unit, z: bias, w: bias per metre
    float4 flags;         // x: the picture holds code values (sRGB target); y: scene HDR given; z: scene bloom given; w: scene's later passes given
    float4 quarter_size;  // the reflection source's size, 1/size
};
Texture2D base_tex : register(t0);
TextureCube env_tex : register(t1);
Texture2D<float4> pic : register(t2);
Texture2D<float4> hdr_tex : register(t3);
Texture2D<float4> dudv_tex : register(t4);
Texture2D<float4> quarter : register(t5);
Texture3D grade_tex : register(t6);
Texture2D<float> game_depth : register(t7);
Texture2D<float4> scene_hdr : register(t8);
Texture2D<float4> scene_bloom : register(t9);
Texture2D<float4> scene_after : register(t10);
SamplerState wrap_tri : register(s0);
SamplerState wrap_aniso : register(s1);
SamplerState clamp_tri : register(s2);

struct VSIn { float3 pos : POSITION; float3 normal : NORMAL; float2 uv : TEXCOORD0; };
struct VSOut {
    float4 pos : SV_Position;
    float3 mpos : TEXCOORD0;
    float3 tpos : TEXCOORD1;
    float3 vpos : TEXCOORD2;
    float3 tnrm : TEXCOORD3;
    float2 uv : TEXCOORD4;
    float facing : TEXCOORD5;
};

VSOut VSMesh(VSIn i) {
    VSOut o;
    float4 t = mul(model, float4(i.pos, 1));
    o.pos = mul(view_proj, t);
    o.mpos = i.pos;
    o.tpos = t.xyz;
    o.vpos = mul(view, t).xyz;
    o.tnrm = normalize(mul((float3x3)model, i.normal));
    o.uv = i.uv;
    // The refraction's vertex term: saturate(1.2 |dot(normalize(P), N)| - 0.1), view space.
    float3 vn = mul((float3x3)view, o.tnrm);
    o.facing = saturate(1.2 * abs(dot(normalize(o.vpos), vn)) - 0.1);
    return o;
}

float4 VSFull(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

float3 m_linear_to_display(float3 c) {
    c = saturate(c);
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1 / 2.4) - 0.055;
}

// The game's tone curve undone on [0, 1].
float3 tonemap_inverse(float3 tm) {
    float3 y = tm / 4.531915 + 0.066667;
    float3 a = 0.15 * (y - 1), b = 0.5 * y - 0.05, c = 0.06 * y - 0.004;
    float3 x = max(2 * c / (-b + sqrt(max(b * b - 4 * a * c, 0))), 0);
    return tm >= 1 ? 1 : x;
}

// The HDR colour behind a displayed code value: the grading undone, then the tone curve.
float3 inverse_post(float3 code) {
    float3 tm = code;
    if (envc.w > 0) {
        [unroll] for (int k = 0; k < 4; k++) {
            float4 l = grade_tex.SampleLevel(clamp_tri, tm.bgr, 0);
            tm = code - (2 * l.bgr - 1);
        }
    }
    return tonemap_inverse(saturate(tm));
}

float3 picture_hdr(int2 p) {
    if (flags.y > 0) return scene_hdr.Load(int3(p, 0)).rgb;
    float3 c = pic.Load(int3(p, 0)).rgb;
    return inverse_post(flags.x > 0 ? c : m_linear_to_display(c));
}

// The game's depth at a target pixel (metres; 0: nothing drawn or no depth).
float game_z(int2 p) {
    if (depth.x == 0) return 0;
    uint w, h;
    game_depth.GetDimensions(w, h);
    int2 q = clamp(int2(float2(p) * float2(w, h) * target.zw), 0, int2(w, h) - 1);
    return game_depth.Load(int3(q, 0)) * depth.y;
}

// Behind the game's own surface (a gun pushed into a wall): hidden, as the avatar hides it.
bool behind_game(float4 svpos, float z) {
    float g = game_z(int2(svpos.xy));
    return g > 0 && z > g + depth.z + depth.w * g;
}

// Pass 1: the picture behind the lens, in HDR (alpha 0: untouched).
float4 PSDecode(float4 svpos : SV_Position) : SV_Target {
    return float4(picture_hdr(int2(svpos.xy)), 0);
}

// The reflection source: a 4x4 average of the picture in HDR.
float4 PSQuarter(float4 svpos : SV_Position) : SV_Target {
    int2 o = int2(svpos.xy) * 4;
    int2 last = int2(target.xy) - 1;
    float3 s = 0;
    [unroll] for (int y = 0; y < 4; y++)
        [unroll] for (int x = 0; x < 4; x++)
            s += picture_hdr(min(o + int2(x, y), last));
    return float4(s / 16, 1);
}

// Step 1 through the lens pane: the reticle texel along this pixel's direction.
float4 PSReticle(VSOut i) : SV_Target {
    float3 dir = normalize(i.mpos - eye_m.xyz);
    float3 from = from_m.xyz;
    float den = dot(dir, ret_axis.xyz);
    float3 l = from + dir * (dot(ret_centre.xyz - from, ret_axis.xyz) / max(den, 1e-6)) - ret_centre.xyz;
    float2 q = float2(dot(l, ret_ex.xyz), dot(l, ret_ey.xyz));
    float2 uv = ret_uv.xy + ret_uv_m.xy * q.x + ret_uv_m.zw * q.y;
    float4 b = base_tex.SampleGrad(wrap_tri, uv, ddx(uv), ddy(uv));
    bool inside = den > 1e-6 && all(abs(q) <= ret_half.xy);
    clip(inside && !behind_game(i.pos, i.vpos.z) ? 1 : -1);
    return b;
}

// Step 2: the lens's reflecting surface.
float4 PSReflect(VSOut i) : SV_Target {
    float3 n = normalize(i.tnrm);
    float3 v = normalize(i.tpos - eye_t.xyz);
    float3 r = v - 2 * dot(v, n) * n;
    float3 w = float3(dot(world_rot[0].xyz, r), dot(world_rot[1].xyz, r), dot(world_rot[2].xyz, r));
    float3 rv = mul((float3x3)view, r);
    float3 env = env_tex.Sample(clamp_tri, w).rgb * envc.rgb;
    float4 b = base_tex.Sample(wrap_tri, i.uv);
    float3 c = env * (1 - b.a) + b.rgb;
    float aspect = target.x / target.y;
    float2 s = rv.xy / rv.z;
    float2 suv = float2(s.x * 0.543478 + 0.5, s.y * aspect * -0.5 + 0.5);
    float fade = sqrt(max(1 - length(float2(s.x * 1.086957, s.y * aspect)), 0));
    float3 bl = saturate(quarter.SampleLevel(clamp_tri, suv, 0).rgb);
    c += fade * bl * dot(bl, float3(0.3, 0.48, 0.22));
    clip(behind_game(i.pos, i.vpos.z) ? -1 : 1);
    return float4(c, b.a);
}

// Step 3, the lens pane: blended DestColor/SrcColor, i.e. dst * 2 base.
float4 PSPane(VSOut i) : SV_Target {
    float4 b = base_tex.Sample(wrap_tri, i.uv);
    clip(behind_game(i.pos, i.vpos.z) ? -1 : 1);
    return float4(b.rgb, 1);
}

// Step 4: the refraction offset and its opacity.
float4 PSDistort(VSOut i) : SV_Target {
    float z = i.vpos.z;
    float k = clamp(4 / z, 0, 2);
    float4 t = base_tex.Sample(wrap_aniso, i.uv);
    float2 d = (t.xy * 2 - 1) * k * (i.facing + 0.5);
    float f = 1;
    if (depth.x > 0) {
        int2 p = int2(i.pos.xy + d * 0.035 * target.xy - 0.5);
        float g0 = game_z(p), g1 = game_z(p + int2(1, 0)), g2 = game_z(p + int2(0, 1)), g3 = game_z(p + int2(1, 1));
        float zs = min(min(g0 > 0 ? g0 : 1e9, g1 > 0 ? g1 : 1e9), min(g2 > 0 ? g2 : 1e9, g3 > 0 ? g3 : 1e9));
        f = saturate(10 * (zs - z) + 0.01);
    }
    clip(behind_game(i.pos, z) ? -1 : 1);
    return float4(f * d, 0, saturate(1.25 - i.facing) * t.w);
}

// The game's final pass over the lens: refracted, bloom (when given), tone curve, grading.
float4 PSResolve(float4 svpos : SV_Position) : SV_Target {
    int2 p = int2(svpos.xy);
    float4 h = hdr_tex.Load(int3(p, 0));
    float4 d = dudv_tex.Load(int3(p, 0));
    if (h.a <= 0 && d.a <= 0) discard;
    float2 uv = svpos.xy * target.zw + d.xy * 0.035;
    float3 c = hdr_tex.SampleLevel(clamp_tri, uv, 0).rgb;
    if (flags.w > 0) c += scene_after.SampleLevel(clamp_tri, uv, 0).rgb;
    float3 bloom = flags.z > 0 ? scene_bloom.SampleLevel(clamp_tri, uv, 0).rgb : 0;
    float3 tm = m_tonemap(c + bloom) + 0.125 * bloom;
    float4 l = grade_tex.SampleLevel(clamp_tri, tm.bgr, 0);
    return float4(m_display_to_linear(m_grade(tm, l, envc.w > 0)), 1);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CollCb {
    view_proj: [f32; 16],
    model: [f32; 16],
    view: [f32; 16],
    world_rot: [[f32; 4]; 3],
    eye_t: [f32; 4],
    eye_m: [f32; 4],
    from_m: [f32; 4],
    ret_centre: [f32; 4],
    ret_axis: [f32; 4],
    ret_ex: [f32; 4],
    ret_ey: [f32; 4],
    ret_half: [f32; 4],
    ret_uv: [f32; 4],
    ret_uv_m: [f32; 4],
    target: [f32; 4],
    envc: [f32; 4],
    depth: [f32; 4],
    flags: [f32; 4],
    quarter_size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

struct GpuMesh {
    vb: ID3D11Buffer,
    ib: ID3D11Buffer,
    count: u32,
    /// Its vertices (sight space), for the screen rectangle.
    points: Vec<Vec3>,
}

/// A loaded collimator: its meshes, textures and reticle.
struct Loaded {
    parts: Parts,
    pane: GpuMesh,
    reflect: Vec<GpuMesh>,
    distort: Vec<GpuMesh>,
    reticle: ReticleQuad,
    base: ID3D11ShaderResourceView,
    env: Option<ID3D11ShaderResourceView>,
}

/// The game's picture behind the lens before its post chain, when an adapter can give it (the
/// exact path; see the module docs): its HDR colour before the first-person glass (`RGBA16F`,
/// the eye's size), what the game's later forward passes add over the glass (HDR, the eye's
/// size: particles, volumetric light), and its bloom term (the final pass's `t_bloom_b` × lens
/// dirt; any size, sampled over the eye).
#[derive(Clone)]
pub struct Scene {
    pub hdr: ID3D11ShaderResourceView,
    pub after: Option<ID3D11ShaderResourceView>,
    pub bloom: Option<ID3D11ShaderResourceView>,
}

/// Scratch targets for one eye size and picture format.
struct Scratch {
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    picture: Texture,
    hdr: Texture,
    dudv: Texture,
    quarter: Texture,
}

/// Draws collimators: give [`Collimator::draw`] to the avatar as its special draw.
pub struct Collimator {
    root: PathBuf,
    models: HashMap<String, Option<Arc<Loaded>>>,
    mode: ReticleMode,
    reticle_shown: bool,
    decode: Program,
    quarter: Program,
    reticle: Program,
    reflect: Program,
    pane: Program,
    distort: Program,
    resolve: Program,
    cb: ConstantBuffer<CollCb>,
    lut: ID3D11Texture3D,
    lut_srv: ID3D11ShaderResourceView,
    envc: Vec3,
    lut_valid: bool,
    world_rot: Quat,
    scene: Option<Scene>,
    scratch: Option<Scratch>,
    samplers: [ID3D11SamplerState; 3],
    blend_alpha: ID3D11BlendState,
    blend_mul2: ID3D11BlendState,
    blend_opaque: ID3D11BlendState,
    /// The same blends leaving alpha alone (in the game's frame alpha is the view depth).
    blend_alpha_rgb: ID3D11BlendState,
    blend_mul2_rgb: ID3D11BlendState,
    /// Drawing into the game's own HDR picture ([`Collimator::set_in_frame`]).
    in_frame: bool,
    cull_back: ID3D11RasterizerState,
    full: ID3D11RasterizerState,
    depth_test: ID3D11DepthStencilState,
    depth_off: ID3D11DepthStencilState,
}

impl Collimator {
    /// Loads models from the asset cache under `root` (per game: `<root>/<game>`), by the held
    /// draws' keys (`<game>/<asset key>`).
    pub fn new(gpu: &Gpu, root: PathBuf) -> Result<Collimator> {
        let layout = [
            element(b"POSITION\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0),
            element(b"NORMAL\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 12),
            element(b"TEXCOORD\0", 0, DXGI_FORMAT_R32G32_FLOAT, 24),
        ];
        let source = format!("{}{SHADER}", metro::HLSL);
        let mesh = |ps: &str| Program::with_entries(gpu, &source, "collimator", "VSMesh", ps, &layout);
        let full = |ps: &str| Program::with_entries(gpu, &source, "collimator", "VSFull", ps, &[]);
        let (lut, lut_srv) = lut_texture(gpu)?;
        Ok(Collimator {
            root,
            models: HashMap::new(),
            mode: ReticleMode::Collimated,
            reticle_shown: true,
            decode: full("PSDecode")?,
            quarter: full("PSQuarter")?,
            reticle: mesh("PSReticle")?,
            reflect: mesh("PSReflect")?,
            pane: mesh("PSPane")?,
            distort: mesh("PSDistort")?,
            resolve: full("PSResolve")?,
            cb: ConstantBuffer::new(gpu)?,
            lut,
            lut_srv,
            envc: metro::ENVMAP_COLOR,
            lut_valid: false,
            world_rot: Quat::IDENTITY,
            scene: None,
            scratch: None,
            samplers: [
                sampler(gpu, D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_TEXTURE_ADDRESS_WRAP)?,
                sampler(gpu, D3D11_FILTER_ANISOTROPIC, D3D11_TEXTURE_ADDRESS_WRAP)?,
                sampler(gpu, D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_TEXTURE_ADDRESS_CLAMP)?,
            ],
            blend_alpha: blend(gpu, D3D11_BLEND_SRC_ALPHA, D3D11_BLEND_INV_SRC_ALPHA, D3D11_COLOR_WRITE_ENABLE_ALL)?,
            blend_mul2: blend(gpu, D3D11_BLEND_DEST_COLOR, D3D11_BLEND_SRC_COLOR, D3D11_COLOR_WRITE_ENABLE_ALL)?,
            blend_alpha_rgb: blend(gpu, D3D11_BLEND_SRC_ALPHA, D3D11_BLEND_INV_SRC_ALPHA, RGB)?,
            blend_mul2_rgb: blend(gpu, D3D11_BLEND_DEST_COLOR, D3D11_BLEND_SRC_COLOR, RGB)?,
            in_frame: false,
            blend_opaque: pipeline::blend_state(gpu, pipeline::Blend::Opaque)?,
            cull_back: pipeline::raster_state(gpu, true, false)?,
            full: pipeline::raster_state(gpu, false, true)?,
            depth_test: pipeline::depth_state(gpu, true, false)?,
            depth_off: pipeline::depth_state(gpu, false, false)?,
        })
    }

    /// How the reticle is placed (collimated unless a test asks for the game's flat quad).
    pub fn set_mode(&mut self, mode: ReticleMode) {
        self.mode = mode;
    }

    /// Leaves the reticle out (for measuring it: tests and tools); it is drawn by default.
    pub fn set_reticle_shown(&mut self, shown: bool) {
        self.reticle_shown = shown;
    }

    /// The game's lighting this frame, as [`crate::Avatar::set_game_shading`] takes it: the
    /// environment tint and grading table, and the turn to the game's world for its cube.
    pub fn set_shading(&mut self, gpu: &Gpu, shading: Option<&GameShading>) {
        let Some(s) = shading else {
            self.lut_valid = false;
            self.envc = metro::ENVMAP_COLOR;
            return;
        };
        self.envc = metro::envmap_color(s.lighting);
        self.world_rot = s.world_rot;
        self.lut_valid = s.lighting.lut_valid != 0;
        if self.lut_valid {
            let row = (LUT_SIZE * 4) as u32;
            unsafe { gpu.ctx.UpdateSubresource(&self.lut, 0, None, s.lighting.lut.as_ptr().cast(), row, row * LUT_SIZE as u32) };
        }
    }

    /// Draw into the game's own HDR picture, before its post chain (an adapter drawing the player
    /// in the game's frame): the game's forward passes for the sight run straight on the bound
    /// target, as the game runs them, and the game's post chain does the rest. The refraction
    /// (step 4) is left out: it goes through the game's own distortion buffer, which isn't hooked.
    pub fn set_in_frame(&mut self, in_frame: bool) {
        self.in_frame = in_frame;
    }

    /// The game's own picture before its post chain, when there is one (see [`Scene`]).
    pub fn set_scene(&mut self, scene: Option<Scene>) {
        self.scene = scene;
    }

    /// The collimator of a held model (`<game>/<asset key>`), loaded once.
    fn model(&mut self, gpu: &Gpu, key: &str) -> Option<Arc<Loaded>> {
        if let Some(m) = self.models.get(key) {
            return m.clone();
        }
        let loaded = load(gpu, &self.root, key).map_err(|e| tracing::warn!("collimator: {key}: {e:#}")).ok().flatten().map(Arc::new);
        self.models.insert(key.to_string(), loaded.clone());
        loaded
    }

    /// The avatar's special draw: on a collimator's pane, draws the whole collimator (its other
    /// meshes' calls do nothing). The eye's target and the avatar's depth buffer are bound.
    pub fn draw(&mut self, sd: &SpecialDraw) {
        let gpu = sd.gpu;
        let Some(m) = self.model(gpu, sd.key) else { return };
        if sd.mesh.source != m.parts.pane {
            return;
        }
        if let Err(e) = self.draw_collimator(gpu, sd, &m) {
            tracing::warn!("collimator: {}: {e:#}", sd.key);
        }
    }

    fn draw_collimator(&mut self, gpu: &Gpu, sd: &SpecialDraw, m: &Loaded) -> Result<()> {
        let ctx = &gpu.ctx;
        let (w, h) = (sd.width, sd.height);
        // The eye's target and depth, as bound.
        let mut rtvs = [None];
        let mut dsv = None;
        unsafe { ctx.OMGetRenderTargets(Some(&mut rtvs), Some(&mut dsv)) };
        let rtv = rtvs[0].take().context("no target bound")?;
        let (target_tex, rtv_format) = unsafe {
            let res = rtv.GetResource()?;
            let tex: ID3D11Texture2D = res.cast()?;
            let mut d = D3D11_RENDER_TARGET_VIEW_DESC::default();
            rtv.GetDesc(&mut d);
            (tex, d.Format)
        };
        let format = unsafe {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            target_tex.GetDesc(&mut d);
            if (d.Width, d.Height) != (w, h) {
                bail!("target {}x{} for an eye of {w}x{h}", d.Width, d.Height);
            }
            d.Format
        };
        if self.in_frame && format != DXGI_FORMAT_R16G16B16A16_FLOAT {
            bail!("the game's HDR picture is format {format:?}, not RGBA16F");
        }
        if !self.in_frame && rustvr_gpu::texture::bytes_per_pixel(format) != 4 {
            bail!("unsupported eye target format {format:?}");
        }
        self.ensure_scratch(gpu, w, h, format)?;
        let s = self.scratch.as_ref().unwrap();

        // Where it lands on the eye (with the refraction's reach, at most 0.105 of the picture).
        let Some(rect) = screen_rect(sd.view_proj * sd.model, [std::slice::from_ref(&m.pane), &m.reflect[..], &m.distort[..]], w, h) else { return Ok(()) };

        let view = Mat4::from_rotation_translation(quat(sd.eye.rot), Vec3::from_array(sd.eye.pos)).inverse();
        let eye_t = Vec3::from_array(sd.eye.pos);
        let eye_m = sd.model.inverse().transform_point3(eye_t);
        let r = &m.reticle;
        let collimated = self.mode == ReticleMode::Collimated;
        if collimated && r.aimed_distance <= 0.0 {
            tracing::warn!("collimator: {}: the reticle's aimed distance is unknown (re-extract the gun)", sd.key);
        }
        let from = r.viewpoint(self.mode, eye_m);
        let world = Mat3::from_quat(self.world_rot);
        let (game_depth, units) = match sd.game_depth {
            Some((srv, upm)) => (Some(srv.clone()), upm),
            None => (None, 1.0),
        };
        let cb = CollCb {
            view_proj: sd.view_proj.to_cols_array(),
            model: sd.model.to_cols_array(),
            view: view.to_cols_array(),
            world_rot: [0, 1, 2].map(|i| world.row(i).extend(0.0).to_array()),
            eye_t: eye_t.extend(1.0).to_array(),
            eye_m: eye_m.extend(1.0).to_array(),
            from_m: from.extend(1.0).to_array(),
            ret_centre: r.centre.extend(1.0).to_array(),
            ret_axis: r.axis.extend(0.0).to_array(),
            ret_ex: r.ex.extend(0.0).to_array(),
            ret_ey: r.ey.extend(0.0).to_array(),
            ret_half: [r.half.x, r.half.y, 0.0, 0.0],
            ret_uv: [r.uv_centre.x, r.uv_centre.y, 0.0, 0.0],
            ret_uv_m: [r.uv_x.x, r.uv_x.y, r.uv_y.x, r.uv_y.y],
            target: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32],
            envc: self.envc.extend(self.lut_valid as u32 as f32).to_array(),
            depth: match game_depth {
                Some(_) => [1.0, 1.0 / units.max(1e-6), crate::render::DEPTH_BIAS, crate::render::DEPTH_BIAS_PER_METER],
                None => [0.0; 4],
            },
            flags: [
                is_srgb(rtv_format) as u32 as f32,
                // In the game's frame the copy of its HDR picture is the scene.
                (self.scene.is_some() || self.in_frame) as u32 as f32,
                self.scene.as_ref().is_some_and(|s| s.bloom.is_some()) as u32 as f32,
                self.scene.as_ref().is_some_and(|s| s.after.is_some()) as u32 as f32,
            ],
            quarter_size: [s.quarter.width as f32, s.quarter.height as f32, 1.0 / s.quarter.width as f32, 1.0 / s.quarter.height as f32],
        };
        self.cb.update(ctx, &cb);
        if self.in_frame {
            self.draw_in_frame(ctx, m, &target_tex, &rtv, dsv.as_ref(), rect, w, h, game_depth.as_ref());
            return Ok(());
        }

        unsafe {
            // The picture as it is now (the game's, and what the avatar drew).
            ctx.CopySubresourceRegion(&s.picture.tex, 0, 0, 0, 0, &target_tex, 0, None);
            ctx.OMSetRenderTargets(None, None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.samplers[0].clone()), Some(self.samplers[1].clone()), Some(self.samplers[2].clone())]));
            let scene = self.scene.clone();
            ctx.PSSetShaderResources(
                0,
                Some(&[
                    Some(m.base.clone()),
                    m.env.clone(),
                    s.picture.srv.clone(),
                    None,
                    None,
                    None,
                    Some(self.lut_srv.clone()),
                    game_depth.clone(),
                    scene.as_ref().map(|s| s.hdr.clone()),
                    scene.as_ref().and_then(|s| s.bloom.clone()),
                    scene.as_ref().and_then(|s| s.after.clone()),
                ]),
            );
            ctx.OMSetBlendState(&self.blend_opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_off, 0);
            ctx.RSSetState(&self.full);

            // 1. The picture behind the lens in HDR, over the rectangle; the refraction cleared.
            ctx.ClearRenderTargetView(s.dudv.rtv.as_ref().unwrap(), &[0.0; 4]);
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&s.hdr.rtv)), None);
            pipeline::viewport(ctx, w, h);
            ctx.RSSetScissorRects(Some(&[rect]));
            self.decode.bind(ctx);
            ctx.Draw(3, 0);
            // The reflection source, over the whole eye.
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&s.quarter.rtv)), None);
            pipeline::viewport(ctx, s.quarter.width, s.quarter.height);
            ctx.RSSetScissorRects(Some(&[RECT { left: 0, top: 0, right: s.quarter.width as i32, bottom: s.quarter.height as i32 }]));
            self.quarter.bind(ctx);
            ctx.Draw(3, 0);

            // 2-4. The game's passes, into the HDR picture (depth-tested against the avatar's).
            pipeline::viewport(ctx, w, h);
            ctx.RSSetState(&self.cull_back);
            ctx.OMSetDepthStencilState(&self.depth_test, 0);
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&s.hdr.rtv)), dsv.as_ref());
            ctx.PSSetShaderResources(5, Some(std::slice::from_ref(&s.quarter.srv)));
            ctx.OMSetBlendState(&self.blend_alpha, Some(&[0.0; 4]), 0xFFFF_FFFF);
            if self.reticle_shown {
                self.reticle.bind(ctx);
                draw_mesh(ctx, &m.pane);
            }
            self.reflect.bind(ctx);
            for g in &m.reflect {
                draw_mesh(ctx, g);
            }
            ctx.OMSetBlendState(&self.blend_mul2, Some(&[0.0; 4]), 0xFFFF_FFFF);
            self.pane.bind(ctx);
            draw_mesh(ctx, &m.pane);
            ctx.OMSetBlendState(&self.blend_alpha, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&s.dudv.rtv)), dsv.as_ref());
            self.distort.bind(ctx);
            for g in &m.distort {
                draw_mesh(ctx, g);
            }

            // 5. Back into the eye: the final pass over what changed.
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.OMSetBlendState(&self.blend_opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_off, 0);
            ctx.RSSetState(&self.full);
            ctx.RSSetScissorRects(Some(&[rect]));
            ctx.PSSetShaderResources(3, Some(&[s.hdr.srv.clone(), s.dudv.srv.clone()]));
            self.resolve.bind(ctx);
            ctx.Draw(3, 0);

            ctx.PSSetShaderResources(0, Some(&[None, None, None, None, None, None, None, None, None, None, None]));
            ctx.OMSetRenderTargets(Some(&[Some(rtv)]), dsv.as_ref());
        }
        Ok(())
    }

    /// Steps 1-3 straight into the game's HDR picture (see [`Collimator::set_in_frame`]): the
    /// reflection's source is a copy of the picture as it is now.
    fn draw_in_frame(
        &self,
        ctx: &ID3D11DeviceContext,
        m: &Loaded,
        target_tex: &ID3D11Texture2D,
        rtv: &ID3D11RenderTargetView,
        dsv: Option<&ID3D11DepthStencilView>,
        rect: RECT,
        w: u32,
        h: u32,
        game_depth: Option<&ID3D11ShaderResourceView>,
    ) {
        let s = self.scratch.as_ref().unwrap();
        unsafe {
            ctx.CopySubresourceRegion(&s.picture.tex, 0, 0, 0, 0, target_tex, 0, None);
            ctx.OMSetRenderTargets(None, None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.samplers[0].clone()), Some(self.samplers[1].clone()), Some(self.samplers[2].clone())]));
            ctx.PSSetShaderResources(
                0,
                Some(&[Some(m.base.clone()), m.env.clone(), None, None, None, None, Some(self.lut_srv.clone()), game_depth.cloned(), s.picture.srv.clone()]),
            );
            // The reflection source, over the whole eye.
            ctx.OMSetBlendState(&self.blend_opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_off, 0);
            ctx.RSSetState(&self.full);
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&s.quarter.rtv)), None);
            pipeline::viewport(ctx, s.quarter.width, s.quarter.height);
            ctx.RSSetScissorRects(Some(&[RECT { left: 0, top: 0, right: s.quarter.width as i32, bottom: s.quarter.height as i32 }]));
            self.quarter.bind(ctx);
            ctx.Draw(3, 0);

            // The game's passes, into its picture (depth-tested against the avatar's).
            pipeline::viewport(ctx, w, h);
            ctx.RSSetScissorRects(Some(&[rect]));
            ctx.RSSetState(&self.cull_back);
            ctx.OMSetDepthStencilState(&self.depth_test, 0);
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), dsv);
            ctx.PSSetShaderResources(5, Some(std::slice::from_ref(&s.quarter.srv)));
            ctx.OMSetBlendState(&self.blend_alpha_rgb, Some(&[0.0; 4]), 0xFFFF_FFFF);
            if self.reticle_shown {
                self.reticle.bind(ctx);
                draw_mesh(ctx, &m.pane);
            }
            self.reflect.bind(ctx);
            for g in &m.reflect {
                draw_mesh(ctx, g);
            }
            ctx.OMSetBlendState(&self.blend_mul2_rgb, Some(&[0.0; 4]), 0xFFFF_FFFF);
            self.pane.bind(ctx);
            draw_mesh(ctx, &m.pane);
            ctx.PSSetShaderResources(0, Some(&[None, None, None, None, None, None, None, None, None]));
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), dsv);
        }
    }

    fn ensure_scratch(&mut self, gpu: &Gpu, w: u32, h: u32, format: DXGI_FORMAT) -> Result<()> {
        if self.scratch.as_ref().is_some_and(|s| s.width == w && s.height == h && s.format == format) {
            return Ok(());
        }
        let picture = if format == DXGI_FORMAT_R16G16B16A16_FLOAT {
            Texture::new(gpu, &TexDesc { width: w, height: h, format, rtv: None, srv: Some(format), dsv: None, mips: 1 })?
        } else {
            let (typeless, unorm, _) = rustvr_gpu::texture::color_family(format);
            Texture::new(gpu, &TexDesc { width: w, height: h, format: typeless, rtv: None, srv: Some(unorm), dsv: None, mips: 1 })?
        };
        let hdr = Texture::new(gpu, &TexDesc::target(w, h, DXGI_FORMAT_R16G16B16A16_FLOAT))?;
        let dudv = Texture::new(gpu, &TexDesc::target(w, h, DXGI_FORMAT_R16G16B16A16_FLOAT))?;
        let quarter = Texture::new(gpu, &TexDesc::target(w.div_ceil(4), h.div_ceil(4), DXGI_FORMAT_R16G16B16A16_FLOAT))?;
        self.scratch = Some(Scratch { width: w, height: h, format, picture, hdr, dudv, quarter });
        Ok(())
    }
}

/// The screen rectangle the collimator's meshes cover (pixels), grown by the refraction's
/// largest reach (0.035 × 3 of the picture), or None when it's wholly off the eye. A mesh with a
/// vertex behind the eye takes the whole eye.
fn screen_rect(mvp: Mat4, meshes: [&[GpuMesh]; 3], w: u32, h: u32) -> Option<RECT> {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    let mut whole = false;
    for list in meshes {
        for m in list {
            for p in &m.points {
                let c = mvp * p.extend(1.0);
                if c.w <= 1e-4 {
                    whole = true;
                    continue;
                }
                let s = Vec2::new((c.x / c.w * 0.5 + 0.5) * w as f32, (0.5 - c.y / c.w * 0.5) * h as f32);
                lo = lo.min(s);
                hi = hi.max(s);
            }
        }
    }
    if whole {
        return Some(RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 });
    }
    if lo.x > hi.x {
        return None;
    }
    let reach = Vec2::new(w as f32, h as f32) * (REFRACTION_SCALE * 3.0) + Vec2::splat(2.0);
    let (lo, hi) = ((lo - reach).max(Vec2::ZERO), (hi + reach).min(Vec2::new(w as f32, h as f32)));
    if lo.x >= hi.x || lo.y >= hi.y {
        return None;
    }
    Some(RECT { left: lo.x.floor() as i32, top: lo.y.floor() as i32, right: hi.x.ceil() as i32, bottom: hi.y.ceil() as i32 })
}

fn is_srgb(f: DXGI_FORMAT) -> bool {
    matches!(f, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB)
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

fn draw_mesh(ctx: &ID3D11DeviceContext, m: &GpuMesh) {
    let stride = size_of::<Vertex>() as u32;
    unsafe {
        ctx.IASetVertexBuffers(0, 1, Some(&Some(m.vb.clone())), Some(&stride), Some(&0));
        ctx.IASetIndexBuffer(&m.ib, DXGI_FORMAT_R32_UINT, 0);
        ctx.DrawIndexed(m.count, 0, 0);
    }
}

/// Loads the collimator of `key` (`<game>/<asset key>`) from the cache, if its model has one.
fn load(gpu: &Gpu, root: &std::path::Path, key: &str) -> Result<Option<Loaded>> {
    let (game, item) = key.split_once('/').with_context(|| format!("{key:?} isn't <game>/<asset key>"))?;
    let dir = root.join(game);
    let model = asset::load_model(&dir, item)?;
    let Some(parts) = parts(&model) else { return Ok(None) };
    let reticle = ReticleQuad::from_mesh(&model.meshes[parts.reticle]).context("the reticle isn't a quad")?;
    let mesh = |i: usize| gpu_mesh(gpu, &model.meshes[i]);
    let pane = mesh(parts.pane)?;
    let reflect = parts.reflect.iter().map(|&i| mesh(i)).collect::<Result<Vec<_>>>()?;
    let distort = parts.distort.iter().map(|&i| mesh(i)).collect::<Result<Vec<_>>>()?;
    let tex = |k: &str| -> Result<ID3D11ShaderResourceView> {
        let t = asset::load_texture(&dir, k).with_context(|| format!("texture {k}"))?;
        Ok(crate::render::upload_texture(gpu, &t)?.linear)
    };
    let base_key = &model.meshes[parts.pane].texture;
    let base = tex(base_key)?;
    let env_key = parts.reflect.iter().map(|&i| &model.meshes[i].material.env).find(|k| !k.is_empty());
    let env = match env_key {
        Some(k) => Some(tex(k)?),
        None => None,
    };
    tracing::info!(
        "collimator: {key}: pane {}, reticle {} ({:.1} x {:.1} mm, aimed from {:.4} m: {:.3}° across), reflection {:?}, refraction {:?}",
        parts.pane,
        parts.reticle,
        reticle.half.x * 2000.0,
        reticle.half.y * 2000.0,
        reticle.aimed_distance,
        reticle.angular_half().x.to_degrees() * 2.0,
        parts.reflect,
        parts.distort
    );
    Ok(Some(Loaded { parts, pane, reflect, distort, reticle, base, env }))
}

/// A mesh's vertices and its triangles wound as D3D's front faces (clockwise on screen): flipped
/// when most wind against their normals, as the avatar does (`render::fix_winding`).
fn gpu_mesh(gpu: &Gpu, m: &asset::Mesh) -> Result<GpuMesh> {
    let p = |i: u32| Vec3::from_array(m.vertices[i as usize].pos);
    let n = |i: u32| Vec3::from_array(m.vertices[i as usize].normal);
    let (mut with, mut against) = (0, 0);
    for t in m.indices.chunks(3) {
        let d = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).dot(n(t[0]) + n(t[1]) + n(t[2]));
        if d > 0.0 {
            with += 1;
        } else if d < 0.0 {
            against += 1;
        }
    }
    let mut indices = m.indices.clone();
    if against > with {
        for t in indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
    let vertices: Vec<Vertex> = m.vertices.iter().map(|v| Vertex { pos: v.pos, normal: v.normal, uv: v.uv }).collect();
    Ok(GpuMesh {
        vb: static_buffer(gpu, bytemuck::cast_slice(&vertices), D3D11_BIND_VERTEX_BUFFER)?,
        ib: static_buffer(gpu, bytemuck::cast_slice(&indices), D3D11_BIND_INDEX_BUFFER)?,
        count: indices.len() as u32,
        points: m.indices.iter().map(|&i| p(i)).collect(),
    })
}

/// Blending for the game's forward passes: colour as given, alpha the max (marks what changed).
const RGB: D3D11_COLOR_WRITE_ENABLE = D3D11_COLOR_WRITE_ENABLE(D3D11_COLOR_WRITE_ENABLE_RED.0 | D3D11_COLOR_WRITE_ENABLE_GREEN.0 | D3D11_COLOR_WRITE_ENABLE_BLUE.0);

fn blend(gpu: &Gpu, src: D3D11_BLEND, dst: D3D11_BLEND, mask: D3D11_COLOR_WRITE_ENABLE) -> Result<ID3D11BlendState> {
    let rt = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: src,
        DestBlend: dst,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: D3D11_BLEND_ONE,
        BlendOpAlpha: D3D11_BLEND_OP_MAX,
        RenderTargetWriteMask: mask.0 as u8,
    };
    let desc = D3D11_BLEND_DESC { AlphaToCoverageEnable: false.into(), IndependentBlendEnable: false.into(), RenderTarget: [rt; 8] };
    let mut s = None;
    unsafe { gpu.device.CreateBlendState(&desc, Some(&mut s))? };
    s.context("no blend state")
}

fn sampler(gpu: &Gpu, filter: D3D11_FILTER, addr: D3D11_TEXTURE_ADDRESS_MODE) -> Result<ID3D11SamplerState> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: filter,
        AddressU: addr,
        AddressV: addr,
        AddressW: addr,
        MipLODBias: 0.0,
        MaxAnisotropy: 16,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let mut s = None;
    unsafe { gpu.device.CreateSamplerState(&desc, Some(&mut s))? };
    s.context("no sampler")
}

/// The grading table (16³ RGBA8), neutral until the game sends one.
fn lut_texture(gpu: &Gpu) -> Result<(ID3D11Texture3D, ID3D11ShaderResourceView)> {
    let n = LUT_SIZE as u32;
    let desc = D3D11_TEXTURE3D_DESC {
        Width: n,
        Height: n,
        Depth: n,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let neutral = vec![0x8080_8080u32; LUT_SIZE * LUT_SIZE * LUT_SIZE];
    let init = D3D11_SUBRESOURCE_DATA { pSysMem: neutral.as_ptr().cast(), SysMemPitch: n * 4, SysMemSlicePitch: n * n * 4 };
    unsafe {
        let mut tex = None;
        gpu.device.CreateTexture3D(&desc, Some(&init), Some(&mut tex))?;
        let tex = tex.context("no grading table")?;
        let mut srv = None;
        gpu.device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
        Ok((tex, srv.context("no grading table view")?))
    }
}

#[cfg(test)]
#[allow(clippy::excessive_precision)] // the measured numbers, as they were printed
mod tests {
    //! Against the game's own numbers: its shaders' values measured on the game at the
    //! lens's pixels, and its render targets
    //! around each draw (docs/research/metro-collimator.md section 3).
    use super::*;

    fn close3(got: Vec3, want: Vec3, tol: f32) -> bool {
        (got - want).abs().max_element() <= tol
    }

    /// The reticle at pixel (1858, 1261): the texel (0.942519, 0, 0, 0.356756) over the picture's
    /// (0.0419006, 0.0155335, 0.0042076) gives the target's (0.363037, 0.009995, 0.002708).
    #[test]
    fn reticle_blend_matches_the_frame() {
        let got = reticle(Vec3::new(0.04190063, 0.01553345, 0.00420761), Vec4::new(0.942519248, 0.0, 0.0, 0.356755912));
        assert!(close3(got, Vec3::new(0.3630371, 0.00999451, 0.00270844), 2e-4), "{got}");
    }

    /// The reflection at pixel (1858, 1261), with the game's own inputs: o = (0.410043, 0.587218, 0.605875).
    #[test]
    fn reflection_matches_the_trace() {
        let blur = Vec3::new(0.0013256073, 0.000665664673, 0.000275373459) / 0.0105667114;
        let r = Vec3::new(0.0435538143, 0.0259476453, -0.998714387);
        let aspect = 0.0003943218 / 0.0003918495;
        let o = reflection(
            Vec4::new(0.283802539, 0.450309008, 0.440329581, 0.146791786),
            Vec3::new(0.240863666, 0.235538259, 0.255786985),
            Vec3::new(0.5705883, 0.6588236, 0.75),
            blur,
            r,
            aspect,
        );
        assert!(close3(o.xyz(), Vec3::new(0.410043001, 0.587217987, 0.605874836), 1e-5), "{o}");
        assert!((o.w - 0.146791786).abs() < 1e-7);
        let uv = ssr_uv(r, aspect);
        assert!((uv - Vec2::new(0.476298988, 0.513072491)).length() < 1e-5, "{uv}");
    }

    /// The pane at pixel (1858, 1261): 2 × (0.870588, 0.785580, 0.419608) × 1794's
    /// (0.382324, 0.109192, 0.111084) = the target's (0.665527, 0.171509, 0.093201).
    #[test]
    fn pane_matches_the_frame() {
        let got = pane(Vec3::new(0.38232422, 0.10919189, 0.11108398), Vec4::new(0.870588243, 0.785580218, 0.419607848, 0.498008698));
        assert!(close3(got, Vec3::new(0.66552734, 0.17150879, 0.09320068), 3e-4), "{got}");
    }

    /// The refraction at pixel (1868, 1268): view depth 0.661576, facing 1, the texel (0.505882, 0.498039):
    /// the offset (0.0352943, -0.0117647), opacity 0.25 × texel.a.
    #[test]
    fn refraction_matches_the_trace() {
        let (d, a) = refraction(Vec4::new(0.505882382, 0.498039216, 0.0, 0.5), 0.66157639, 1.0, Some(5.0));
        assert!((d - Vec2::new(0.0352942944, -0.0117647052)).length() < 1e-6, "{d}");
        assert!((a - 0.125).abs() < 1e-6);
        // The scene in front of the lens: no offset.
        let (d, _) = refraction(Vec4::new(0.9, 0.9, 0.0, 1.0), 0.66, 1.0, Some(0.5));
        assert!(d.length() < 1e-3, "{d}");
    }

    #[test]
    fn the_post_is_inverted() {
        let lut: Vec<u32> = vec![0x7F7F_7F7F; LUT_SIZE * LUT_SIZE * LUT_SIZE];
        // (The frame's table takes 1/255 off: what displays as 0 can't be told apart.)
        for hdr in [0.0, 0.003, 0.02, 0.0712, 0.3, 0.6655, 0.95] {
            let x = Vec3::splat(hdr);
            for lut in [None, Some(&lut[..])] {
                let code = metro::post(x, lut);
                if code.x == 0.0 && hdr > 0.0 || lut.is_some() && code.x == 0.0 {
                    continue;
                }
                let back = inverse_post(code, lut);
                assert!((back - x).abs().max_element() < 1e-4 * (1.0 + hdr * 10.0), "{hdr}: {code} -> {back}");
            }
        }
        // The game's gun top, measured: HDR 0.0712 displays as 0.0853 (docs 5.5).
        assert!((metro::post(Vec3::splat(0.0712), Some(&lut)).x - 0.0853).abs() < 5e-4);
    }

    /// The collimated reticle: from any eye, a direction gives the same texel; from the aimed
    /// camera it is exactly the game's quad.
    #[test]
    fn collimation() {
        let q = ReticleQuad {
            centre: Vec3::new(0.097, 0.0, 0.7514),
            axis: Vec3::Z,
            ex: Vec3::X,
            ey: Vec3::Y,
            half: Vec2::splat(0.0022),
            uv_centre: Vec2::new(0.7297, 0.4778),
            uv_x: Vec2::new(7.88, 0.0),
            uv_y: Vec2::new(0.0, -7.88),
            aimed_distance: 0.5637,
        };
        let aim = q.centre - Vec3::Z * q.aimed_distance;
        let dir = Vec3::new(0.001, -0.0015, 1.0).normalize();
        let a = q.uv(ReticleMode::Collimated, aim + Vec3::new(0.032, 0.0, 0.0), dir).unwrap();
        let b = q.uv(ReticleMode::Collimated, aim - Vec3::new(0.032, 0.01, 0.05), dir).unwrap();
        let flat = q.uv(ReticleMode::Quad, aim, dir).unwrap();
        assert!((a - b).length() < 1e-7 && (a - flat).length() < 1e-6, "{a} {b} {flat}");
        // Along the axis: the quad's centre.
        assert!((q.uv(ReticleMode::Collimated, Vec3::new(0.2, 0.1, 0.0), Vec3::Z).unwrap() - q.uv_centre).length() < 1e-6);
        // 0.4472° across.
        assert!((q.angular_half().x.to_degrees() * 2.0 - 0.4472).abs() < 1e-3, "{}", q.angular_half().x.to_degrees() * 2.0);
    }
}
