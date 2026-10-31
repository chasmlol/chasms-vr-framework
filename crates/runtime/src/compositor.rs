//! Draws each eye: the game's image (or the runtime's own empty room when there's no game picture),
//! then the runtime's layers on top: the scope lens, panels (HUD, menus), the laser pointer and
//! controller markers. Everything the runtime draws lives in tracking space and is drawn with the
//! eye pose the picture underneath is seen from (the game's own, or this headset frame's when the
//! game's picture has been moved to it, `crate::warp`), so the headset's reprojection treats it
//! all as one picture.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer};
use rustvr_gpu::shader::Program;
use rustvr_gpu::Gpu;
use rustvr_protocol::{Fov, Pose};
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP};
use windows::Win32::Graphics::Direct3D11::*;

use crate::backend::EyeTarget;
use crate::panels::Panel;

const SHADER: &str = r#"
cbuffer Draw : register(b0) {
    float4x4 view_proj;      // tracking space -> clip
    float4x4 inv_view_proj;
    float4 center;           // quad centre
    float4 axis_x;           // half width (towards u = 1)
    float4 axis_y;           // half height (towards v = 0)
    float4 color;            // premultiplied, linear
    float4 uv_rect;          // u0 v0 u1 v1
    float4 params;           // x mode, y reticle, z opacity, w 1: added to what's behind
    float4x4 view_proj_src;  // the re-projected picture's eye
};
Texture2D tex : register(t0);
SamplerState samp : register(s0);

struct VOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float2 local : TEXCOORD1; };

VOut VSFull(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VOut o;
    o.pos = float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
    o.uv = uv;
    o.local = uv;
    return o;
}

float4 PSBlit(VOut i) : SV_Target { return float4(tex.Sample(samp, i.uv).rgb, 1); }

// Another camera's view of the game's picture: each pixel's direction looked up in one eye's image
// (`view_proj` here is that eye's; the camera stands where it was drawn from, so a turn of the
// camera is exact), else in the other's (`eye_b`), else dark. The other eye stands 6 cm away: where
// its depth is there, each ray is followed out through it to the surface it shows, so near things
// line up across the join (no seam); without it, by direction alone (far things don't move).
cbuffer Reproject : register(b1) {
    float4x4 eye_b;
    float4 cam_pos;   // xyz: the camera's position (where the first eye's picture was drawn from)
    float4 b_pos;     // xyz: the other eye's
    float4 b_fwd;     // xyz: the other eye's forward
    float4 b_depth;   // x: metres per unit of its depth, y: 1 when its depth is there
};
Texture2D tex_b : register(t1);
Texture2D<float> depth_b : register(t3);

// The game's fog where it measures its distance in clip space, as Gamebryo's shaders do: the
// length of a point's clip-space x, y and depth, not how far away it is. That grows towards the
// edges of an eye's frustum, so the same far hill is foggier near the edge of one eye's picture
// than in the middle of the other's, and the join between them shows as a step in the fog. Where
// the other eye's picture fills in, its fog is made the first eye's (`refog_on`: 1 to; the game's
// colour in its display's values; amount = saturate(distance·y + x), in the game's units of
// distance, `b_depth.x` metres each).
cbuffer Refog : register(b3) {
    float4 refog_color;
    float4 refog_fade;
    float4 refog_on;
};
float3 display_of(float3 c) {
    c = max(c, 0);
    return lerp(1.055 * pow(c, 1 / 2.4) - 0.055, c * 12.92, step(c, 0.0031308));
}
float3 linear_of(float3 c) {
    c = max(c, 0);
    return lerp(pow((c + 0.055) / 1.055, 2.4), c / 12.92, step(c, 0.04045));
}
// How far the game measured a point `r` metres along `u` for its fog, seen from the eye of `m` (its
// view-projection; the eyes are 6 cm apart, nothing next to the distances fog is seen at).
float fog_amount(float4x4 m, float3 u, float r) {
    float4 c = mul(m, float4(u, 0));
    return saturate(length(float3(c.xy, c.w)) * r / max(b_depth.x, 1e-6) * refog_fade.y + refog_fade.x);
}
// The other eye's colour `c`, `r` metres along `u`, with the first eye's fog instead of its own.
float3 refog(float3 c, float3 u, float r) {
    if (refog_on.x == 0 || r <= 0) return c;
    float fa = fog_amount(view_proj_src, u, r);
    float fb = fog_amount(eye_b, u, r);
    if (fb > 0.995) return c;
    float3 d = display_of(c);
    // (Thicker: more of the fog over it. Thinner: some of its fog taken back off, no more than
    // fourfold, which lifts the dark and the noise as much.)
    d = fa >= fb ? lerp(d, refog_color.rgb, (fa - fb) / (1 - fb)) : refog_color.rgb + (d - refog_color.rgb) * min((1 - fa) / (1 - fb), 4);
    return linear_of(d);
}

float depth_b_at(float2 uv) {
    uint w, h;
    depth_b.GetDimensions(w, h);
    return depth_b.Load(int3(min(uint2(saturate(uv) * float2(w, h)), uint2(w - 1, h - 1)), 0)) * b_depth.x;
}
float2 b_uv(float3 p, out bool in_view) {
    float4 c = mul(eye_b, float4(p, 1));
    float2 n = c.xy / max(c.w, 1e-6);
    in_view = c.w > 0 && abs(n.x) <= 1 && abs(n.y) <= 1;
    return float2(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5);
}
// Where the ray from the camera along `d` meets what the other eye shows: how far (`t`, metres) and
// where in its picture (`uv`). A step that lands far behind a surface has passed behind something
// nearer the other eye than the camera's line: it goes on past it.
bool march_b(float3 d, out float2 uv, out float t) {
    uv = 0;
    t = 0;
    if (b_depth.y == 0) return false;
    float prev = 0.03;
    [loop] for (int i = 0; i < 48; i++) {
        float ti = 0.03 * pow(3000.0, i / 47.0);
        float3 p = cam_pos.xyz + d * ti;
        bool in_view;
        float2 u = b_uv(p, in_view);
        if (in_view) {
            float z = depth_b_at(u);
            float behind = dot(p - b_pos.xyz, b_fwd.xyz) - z;
            if (z > 0 && behind >= 0) {
                if (behind < 0.05 + 0.15 * z + (ti - prev)) {
                    float lo = prev, hi = ti;
                    [unroll] for (int k = 0; k < 6; k++) {
                        float mid = 0.5 * (lo + hi);
                        float3 q = cam_pos.xyz + d * mid;
                        bool v;
                        float2 uq = b_uv(q, v);
                        float zq = depth_b_at(uq);
                        if (zq > 0 && dot(q - b_pos.xyz, b_fwd.xyz) >= zq) hi = mid; else lo = mid;
                    }
                    bool v;
                    t = hi;
                    uv = b_uv(cam_pos.xyz + d * hi, v);
                    return true;
                }
            }
        }
        prev = ti;
    }
    return false;
}
bool eye_uv(float4x4 m, float3 d, out float2 uv) {
    float4 c = mul(m, float4(d, 0));
    float2 n = c.xy / max(c.w, 1e-6);
    uv = float2(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5);
    return c.w > 0 && abs(n.x) <= 1 && abs(n.y) <= 1;
}
float3 eye_sample(Texture2D t, float4x4 m, float3 d, out bool ok) {
    float2 uv;
    ok = eye_uv(m, d, uv);
    return t.SampleLevel(samp, uv, 0).rgb;
}
float4 PSReproject(VOut i) : SV_Target {
    float2 ndc = float2(i.uv.x * 2 - 1, 1 - i.uv.y * 2);
    float4 a = mul(inv_view_proj, float4(ndc, 0, 1)); a /= a.w;
    float4 b = mul(inv_view_proj, float4(ndc, 1, 1)); b /= b.w;
    float3 d = normalize(b.xyz - a.xyz);
    bool ok;
    float3 c = eye_sample(tex, view_proj_src, d, ok);
    if (ok) return float4(c, 1);
    float2 uv;
    float t;
    if (march_b(d, uv, t)) return float4(refog(tex_b.SampleLevel(samp, uv, 0).rgb, d, t), 1);
    // (By direction: as far along this ray as the other eye's depth there, measured along its
    // forward. The sky has none, and keeps its colour.)
    if (eye_uv(eye_b, d, uv)) return float4(refog(tex_b.SampleLevel(samp, uv, 0).rgb, d, depth_b_at(uv) / max(dot(d, b_fwd.xyz), 1e-3)), 1);
    return float4(0.004, 0.004, 0.005, 1);
}

// The game's depth, bloom and lens for another camera at the eye a picture was drawn from (the
// desktop view's): each pixel's direction looked up in that eye's (`view_proj_src`), else the
// other's (`eye_b`), the depth turned onto this camera's axis (as far along the ray, measured along
// its forward). What the runtime draws over the picture hides behind what the game drew, and gets
// its light and lens, as in the headset. 0 where neither eye shows it (nothing drawn there).
cbuffer ReprojectPost : register(b2) {
    float4 fwd_a;
    float4 fwd_b;
    float4 fwd_cam;
};
Texture2D<float> depth_a : register(t2);
Texture2D bloom_a : register(t4);
Texture2D bloom_b : register(t5);
Texture2D lens_a : register(t6);
Texture2D lens_b : register(t7);
struct PostOut { float depth : SV_Target0; float4 bloom : SV_Target1; float4 lens : SV_Target2; };
float load_depth(Texture2D<float> t, float2 uv) {
    uint w, h;
    t.GetDimensions(w, h);
    return t.Load(int3(min(uint2(uv * float2(w, h)), uint2(w - 1, h - 1)), 0));
}
PostOut PSReprojectPost(VOut i) {
    float2 ndc = float2(i.uv.x * 2 - 1, 1 - i.uv.y * 2);
    float4 a = mul(inv_view_proj, float4(ndc, 0, 1)); a /= a.w;
    float4 b = mul(inv_view_proj, float4(ndc, 1, 1)); b /= b.w;
    float3 d = normalize(b.xyz - a.xyz);
    PostOut o;
    o.depth = 0;
    o.bloom = 0;
    o.lens = 0;
    float2 uv;
    float along = dot(d, fwd_cam.xyz);
    if (eye_uv(view_proj_src, d, uv)) {
        float z = load_depth(depth_a, uv);
        o.depth = z > 0 ? z * along / max(dot(d, fwd_a.xyz), 1e-3) : 0;
        o.bloom = bloom_a.SampleLevel(samp, uv, 0);
        o.lens = lens_a.SampleLevel(samp, uv, 0);
    } else {
        float t;
        if (march_b(d, uv, t)) {
            // (Its depth along this camera's axis, in the game's units as the picture's.)
            o.depth = t * along / max(b_depth.x, 1e-6);
            o.bloom = bloom_b.SampleLevel(samp, uv, 0);
            o.lens = lens_b.SampleLevel(samp, uv, 0);
        } else if (eye_uv(eye_b, d, uv)) {
            float z = load_depth(depth_b, uv);
            o.depth = z > 0 ? z * along / max(dot(d, fwd_b.xyz), 1e-3) : 0;
            o.bloom = bloom_b.SampleLevel(samp, uv, 0);
            o.lens = lens_b.SampleLevel(samp, uv, 0);
        }
    }
    return o;
}

// The runtime's own room: a floor grid at the real floor with a ring around the play-space centre.
float4 PSVoid(VOut i) : SV_Target {
    float2 ndc = float2(i.uv.x * 2 - 1, 1 - i.uv.y * 2);
    float4 a = mul(inv_view_proj, float4(ndc, 0, 1)); a /= a.w;
    float4 b = mul(inv_view_proj, float4(ndc, 1, 1)); b /= b.w;
    float3 d = normalize(b.xyz - a.xyz);
    float3 sky = lerp(float3(0.010, 0.012, 0.018), float3(0.05, 0.06, 0.09), saturate(d.y * 2 + 0.3));
    if (d.y > -1e-3) return float4(sky, 1);
    float t = -a.y / d.y;
    float3 p = a.xyz + d * t;
    float2 g = abs(frac(p.xz + 0.5) - 0.5) / max(fwidth(p.xz), 1e-4);
    float grid = 1 - saturate(min(g.x, g.y));
    float fade = saturate(1 - t / 25);
    float r = length(p.xz);
    float ring = 1 - saturate(abs(r - 1.0) / max(fwidth(r), 1e-4) * 0.5);
    float3 floorc = float3(0.012, 0.014, 0.018) + grid * fade * float3(0.05, 0.16, 0.22) + ring * fade * float3(0.30, 0.20, 0.05);
    return float4(lerp(sky, floorc, fade * 0.9 + 0.1), 1);
}

// Comfort: a vignette that closes in around the centre of view (the head's forward, `center`), and a
// fade to black. color = (strength, aperture, feather, fade). Angles, not screen positions, so
// both eyes darken the same way.
float4 PSComfort(VOut i) : SV_Target {
    float2 ndc = float2(i.uv.x * 2 - 1, 1 - i.uv.y * 2);
    float4 a = mul(inv_view_proj, float4(ndc, 0, 1)); a /= a.w;
    float4 b = mul(inv_view_proj, float4(ndc, 1, 1)); b /= b.w;
    float3 d = normalize(b.xyz - a.xyz);
    float angle = acos(clamp(dot(d, normalize(center.xyz)), -1, 1));
    float r = angle / radians(55.0);
    float v = smoothstep(color.y - color.z, color.y + color.z, r) * color.x;
    float dark = 1 - (1 - v) * (1 - color.w);
    return float4(0, 0, 0, saturate(dark));
}

VOut VSQuad(uint id : SV_VertexID) {
    float2 c = float2(id & 1, id >> 1);
    float2 s = c * 2 - 1;
    float3 p = center.xyz + axis_x.xyz * s.x - axis_y.xyz * s.y;
    VOut o;
    o.pos = mul(view_proj, float4(p, 1));
    o.uv = lerp(uv_rect.xy, uv_rect.zw, c);
    o.local = s;
    return o;
}

// The texture over this pixel's whole footprint: a panel seen smaller than its texture (a game's UI
// drawn at the eye's resolution and shown a few degrees across) is averaged over the texels it
// covers rather than sampled at one, so text doesn't shimmer. (The texture has no smaller copies of
// itself to sample.)
float4 sample_footprint(float2 uv) {
    uint w, h;
    tex.GetDimensions(w, h);
    float2 dx = ddx(uv), dy = ddy(uv);
    float2 size = float2(w, h);
    float n = clamp(ceil(max(length(dx * size), length(dy * size))), 1, 4);
    float4 acc = 0;
    [loop] for (float a = 0; a < n; a++) {
        [loop] for (float b = 0; b < n; b++) {
            acc += tex.SampleLevel(samp, uv + dx * ((a + 0.5) / n - 0.5) + dy * ((b + 0.5) / n - 0.5), 0);
        }
    }
    return acc / (n * n);
}

float4 PSQuad(VOut i) : SV_Target {
    int mode = (int)params.x;
    if (mode == 1) return color;
    if (mode == 2) {
        float r = length(i.local);
        if (r > 1) discard;
        return color * saturate((1 - r) * 3);
    }
    if (mode == 4) {
        // A glow: light added to what's behind, bright at its heart and fading to nothing.
        float r = length(i.local);
        if (r > 1) discard;
        float g = pow(saturate(1 - r), 2.5) + 0.6 * saturate(1 - r * 3);
        return float4(color.rgb * g, 0);
    }
    if (mode == 5) {
        // A beam of light (the laser): a bright thread along its middle (`color`) in a glow
        // (`uv_rect`) that fades to nothing at its sides, added to what's behind (the thread a
        // little opaque too, so it reads over a pale page). It comes in from the hand, fades out
        // along its length when it points at nothing (`params.z`), and motes of light run out along
        // it (`params.y`: seconds; `params.w`: its length in metres).
        float across = saturate(1 - abs(i.local.x));
        float d = (1 - i.local.y) * 0.5 * params.w;
        float ends = saturate(d / 0.05) * lerp(1, saturate(1 - d / max(params.w, 1e-3)), params.z);
        float m = frac(d / 0.25 - params.y * 4);
        float mote = pow(saturate(1 - abs(m - 0.5) * 8), 2);
        float thread = pow(across, 6);
        float3 c = (color.rgb * thread + uv_rect.rgb * across * across * 0.35) * (1 + 1.5 * mote) * ends;
        return float4(c, thread * 0.5 * ends);
    }
    if (mode == 6) {
        // A volume of space (a body slot's): a sphere seen as light, faint in its middle and
        // brightest at its rim, as a soap bubble is, its edge smoothed over a pixel.
        float r = length(i.local);
        if (r > 1) discard;
        float g = (0.12 + 0.88 * pow(r, 8)) * saturate((1 - r) / max(fwidth(r), 1e-4));
        return float4(color.rgb * g, 0);
    }
    if (mode == 3) {
        float r = length(i.local);
        if (r > 1) discard;
        float3 c = tex.Sample(samp, i.uv).rgb;
        if (params.y > 0) {
            float w = 0.008;
            bool crossline = (abs(i.local.x) < w || abs(i.local.y) < w) && r > 0.06;
            bool centre_dot = params.y > 1 && r < 0.015;
            if (crossline || centre_dot) c = float3(0.01, 0.01, 0.01);
        }
        float vignette = saturate((1 - r) * 12);
        return float4(c * vignette, 1);
    }
    float4 t = sample_footprint(i.uv);
    // Solid (`color.z`): covered half or more is opaque, in its own colour (premultiplied again by
    // the coverage it's given); fainter edges stay soft.
    if (color.z > 0.5) {
        float a = saturate(t.a * 2);
        t = float4(t.rgb * (a / max(t.a, 1e-3)), a);
    }
    t *= params.z;
    // A soft edge: faded out over the last part of the quad (`color.xy` of its half-width and
    // half-height).
    if (color.x > 0) t *= smoothstep(0, color.x, 1 - abs(i.local.x)) * smoothstep(0, color.y, 1 - abs(i.local.y));
    // Added to what's behind (a texture whose alpha means nothing): premultiplied with no coverage.
    if (params.w > 0.5) t.a = 0;
    return t;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawCb {
    view_proj: [f32; 16],
    inv_view_proj: [f32; 16],
    center: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    color: [f32; 4],
    uv_rect: [f32; 4],
    params: [f32; 4],
    view_proj_src: [f32; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ReprojectCb {
    eye_b: [f32; 16],
    cam_pos: [f32; 4],
    b_pos: [f32; 4],
    b_fwd: [f32; 4],
    b_depth: [f32; 4],
}

impl ReprojectCb {
    /// The other eye (`second`: its view-projection, pose and depth, if it has one) for a camera at
    /// `cam`; `metres`: metres per unit of the game's depth.
    fn new(cam: Vec3, second: Option<(Mat4, &Pose)>, has_depth: bool, metres: f32) -> Self {
        let (vp, pos, fwd) = match second {
            Some((vp, p)) => (vp, Vec3::from_array(p.pos), quat(p.rot) * Vec3::Z),
            None => (Mat4::ZERO, Vec3::ZERO, Vec3::Z),
        };
        ReprojectCb {
            eye_b: vp.to_cols_array(),
            cam_pos: cam.extend(1.0).to_array(),
            b_pos: pos.extend(1.0).to_array(),
            b_fwd: fwd.extend(0.0).to_array(),
            b_depth: [metres, if has_depth && second.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0],
        }
    }
}

/// The game's fog for [`Background::Reprojected`]'s other eye, when the game measures its distance
/// in clip space (Gamebryo's shaders: see `refog` in the shader).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipFog {
    /// Its colour, in the game's display's values.
    pub color: [f32; 3],
    /// Its amount = saturate(distance·`fade[1]` + `fade[0]`), the distance in the game's world
    /// units (the depth's).
    pub fade: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RefogCb {
    color: [f32; 4],
    fade: [f32; 4],
    on: [f32; 4],
}

impl RefogCb {
    fn new(fog: Option<&ClipFog>) -> Self {
        match fog {
            Some(f) => RefogCb { color: [f.color[0], f.color[1], f.color[2], 0.0], fade: [f.fade[0], f.fade[1], 0.0, 0.0], on: [1.0, 0.0, 0.0, 0.0] },
            None => RefogCb::zeroed(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ReprojectPostCb {
    fwd_a: [f32; 4],
    fwd_b: [f32; 4],
    fwd_cam: [f32; 4],
}

/// One eye's depth (`R32_FLOAT`, linear along its view axis, game units), its bloom and lens (as
/// [`rustvr_avatar::DepthInput`] has them), and where its picture was drawn from.
pub struct PostSource<'a> {
    pub depth: &'a ID3D11ShaderResourceView,
    pub bloom: Option<&'a ID3D11ShaderResourceView>,
    pub lens: Option<&'a ID3D11ShaderResourceView>,
    pub pose: Pose,
    pub fov: Fov,
}

/// Another camera's depth, bloom and lens ([`Compositor::reproject_post`]): the camera's size.
pub struct PostTargets {
    pub depth: rustvr_gpu::Texture,
    pub bloom: rustvr_gpu::Texture,
    pub lens: rustvr_gpu::Texture,
}

impl PostTargets {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Result<PostTargets> {
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32_FLOAT};
        let tex = |f| rustvr_gpu::Texture::new(gpu, &rustvr_gpu::TexDesc::target(width, height, f));
        Ok(PostTargets { depth: tex(DXGI_FORMAT_R32_FLOAT)?, bloom: tex(DXGI_FORMAT_R16G16B16A16_FLOAT)?, lens: tex(DXGI_FORMAT_R16G16B16A16_FLOAT)? })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.depth.width, self.depth.height)
    }
}

/// What's behind everything in a view.
pub enum Background<'a> {
    /// The game's picture for this eye, as it was rendered.
    Game(&'a ID3D11ShaderResourceView),
    /// The runtime's own room.
    Void,
    /// Another camera's view of the game's picture, the camera where `first` was drawn from: from
    /// `first` (its eye's pose and field of view), else from `second` where `first` doesn't reach,
    /// followed out along each ray through its depth (`second_depth`, `metres` per unit) when it
    /// has one.
    Reprojected {
        first: (&'a ID3D11ShaderResourceView, Pose, Fov),
        second: Option<(&'a ID3D11ShaderResourceView, Pose, Fov)>,
        second_depth: Option<&'a ID3D11ShaderResourceView>,
        metres: f32,
        /// The game's fog, when it measures its distance in clip space: `second`'s picture is
        /// refogged as `first`'s would be where it fills in, so the two meet without a step.
        fog: Option<ClipFog>,
    },
    /// What's in the target already (the game's picture moved there, `crate::warp`).
    Keep,
}

const MODE_TEXTURE: f32 = 0.0;
const MODE_SOLID: f32 = 1.0;
const MODE_DOT: f32 = 2.0;
const MODE_GLOW: f32 = 4.0;
const MODE_BEAM: f32 = 5.0;
const MODE_BUBBLE: f32 = 6.0;
const MODE_LENS: f32 = 3.0;

/// One textured panel to draw.
pub struct PanelLayer<'a> {
    pub panel: Panel,
    pub srv: &'a ID3D11ShaderResourceView,
    pub opacity: f32,
    /// The part of the texture shown: u0, v0, u1, v1.
    pub uv: [f32; 4],
    /// Its colour is added to what's behind it (its alpha is ignored).
    pub additive: bool,
    /// Its edges fade out over this part of its half-width and half-height (0: a hard edge).
    pub soft: [f32; 2],
    /// Solid where its texture is half covered or more (`HUDE_SOLID`).
    pub solid: bool,
}

impl<'a> PanelLayer<'a> {
    /// The whole texture on `panel`.
    pub fn whole(panel: Panel, srv: &'a ID3D11ShaderResourceView) -> Self {
        PanelLayer { panel, srv, opacity: 1.0, uv: [0.0, 0.0, 1.0, 1.0], additive: false, soft: [0.0; 2], solid: false }
    }
}

pub struct LensLayer<'a> {
    pub pos: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub radius: f32,
    pub srv: &'a ID3D11ShaderResourceView,
    /// Width / height of the aux image (the lens shows its centre square).
    pub aspect: f32,
    pub reticle: u32,
}

/// Light gathered at a point (a spell held ready in a palm): a camera-facing glow, added to what's
/// behind it.
#[derive(Clone, Copy, Debug)]
pub struct Glow {
    pub at: Vec3,
    pub radius: f32,
    /// Linear colour, any brightness.
    pub color: Vec3,
}

/// A volume of space shown (a body slot's, while the slots' editor is open): a sphere of light,
/// faint in its middle and brightest at its rim, added to what's behind it, so it shows over the
/// drawn body a slot is partly inside.
#[derive(Clone, Copy, Debug)]
pub struct Bubble {
    pub at: Vec3,
    pub radius: f32,
    /// Linear colour, any brightness.
    pub color: Vec3,
}

pub struct Laser {
    pub origin: Vec3,
    pub end: Vec3,
    /// It hits a panel (draw the dot).
    pub hit: bool,
    /// The trigger is held: the dot pressed in.
    pub down: bool,
    /// Its colours (linear): the bright thread along its middle, and the glow round it and its dot.
    pub core: Vec3,
    pub glow: Vec3,
    /// Seconds (any start), for the light running along it.
    pub time: f32,
}

impl Laser {
    /// The runtime's own colours, for a game that doesn't give its own (`HudLayout::pointer`).
    pub const CORE: Vec3 = Vec3::new(0.6, 0.9, 1.0);
    pub const GLOW: Vec3 = Vec3::new(0.25, 0.75, 1.0);
}

/// A colour given as `0xRRGGBBAA` in sRGB, in linear light (its alpha left out).
pub fn srgb(rgba: u32) -> Vec3 {
    let f = |c: u32| {
        let c = (c & 0xFF) as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    Vec3::new(f(rgba >> 24), f(rgba >> 16), f(rgba >> 8))
}

#[derive(Default)]
pub struct Layers<'a> {
    pub lens: Option<LensLayer<'a>>,
    /// Panels in the world (a game's menus floating there): drawn before the body, which is nearer
    /// (the hands are never covered by a menu they're in front of).
    pub world_panels: Vec<PanelLayer<'a>>,
    pub panels: Vec<PanelLayer<'a>>,
    /// Panels only the headset shows: never the desktop view, never a recording (OBS's window).
    pub private: Vec<PanelLayer<'a>>,
    pub laser: Option<Laser>,
    /// Straps drawn as flat ribbons turned to the eye (tracking space): a mount's reins.
    pub ribbons: Vec<Ribbon>,
    /// Controller grip poses to mark (when the game isn't drawing hands).
    pub markers: Vec<(Vec3, Quat)>,
    pub glows: Vec<Glow>,
    pub bubbles: Vec<Bubble>,
    pub comfort: Comfort,
}

/// A straight piece of a strap: from `a` to `b`, `width` across, of one colour (linear RGBA).
#[derive(Clone, Copy, Debug)]
pub struct Ribbon {
    pub a: Vec3,
    pub b: Vec3,
    pub width: f32,
    pub color: Vec4,
}

/// The comfort overlay for this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Comfort {
    /// 0 = no vignette.
    pub vignette: f32,
    /// Clear radius at full strength, as a fraction of 55 degrees from the centre of view.
    pub aperture: f32,
    pub feather: f32,
    /// Head forward (tracking space): the vignette's centre.
    pub forward: Vec3,
    /// 0..1 black.
    pub fade: f32,
}

pub struct Compositor {
    full_blit: Program,
    full_void: Program,
    full_comfort: Program,
    full_reproject: Program,
    reproject_cb: ConstantBuffer<ReprojectCb>,
    refog_cb: ConstantBuffer<RefogCb>,
    full_post: Program,
    post_cb: ConstantBuffer<ReprojectPostCb>,
    quad: Program,
    opaque: ID3D11BlendState,
    premul: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    no_depth: ID3D11DepthStencilState,
    sampler: ID3D11SamplerState,
    cb: ConstantBuffer<DrawCb>,
}

pub const NEAR: f32 = 0.03;
pub const FAR: f32 = 200.0;

impl Compositor {
    pub fn new(gpu: &Gpu) -> Result<Compositor> {
        Ok(Compositor {
            full_blit: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSBlit", &[])?,
            full_void: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSVoid", &[])?,
            full_comfort: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSComfort", &[])?,
            full_reproject: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSReproject", &[])?,
            reproject_cb: ConstantBuffer::new(gpu)?,
            refog_cb: ConstantBuffer::new(gpu)?,
            full_post: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSReprojectPost", &[])?,
            post_cb: ConstantBuffer::new(gpu)?,
            quad: Program::with_entries(gpu, SHADER, "compositor", "VSQuad", "PSQuad", &[])?,
            opaque: pipeline::blend_state(gpu, Blend::Opaque)?,
            premul: pipeline::blend_state(gpu, Blend::Premultiplied)?,
            raster: pipeline::raster_state(gpu, false, false)?,
            no_depth: pipeline::depth_state(gpu, false, false)?,
            sampler: pipeline::sampler(gpu, true, false)?,
            cb: ConstantBuffer::new(gpu)?,
        })
    }

    fn bind(&self, gpu: &Gpu, target: &EyeTarget) {
        let ctx = &gpu.ctx;
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
            ctx.RSSetState(&self.raster);
            ctx.OMSetDepthStencilState(&self.no_depth, 0);
            ctx.OMSetBlendState(&self.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.IASetInputLayout(None);
        }
        pipeline::viewport(ctx, target.width, target.height);
    }

    /// Draws one eye. `game` is the game's eye image (sRGB view), or None for the runtime's room.
    /// `between` runs after the picture and before the overlays (the player's body goes there),
    /// with the eye's target bound; it may change any pipeline state.
    pub fn draw_eye(
        &self,
        gpu: &Gpu,
        target: &EyeTarget,
        eye: &Pose,
        fov: &Fov,
        game: Option<&ID3D11ShaderResourceView>,
        layers: &Layers,
        between: &mut dyn FnMut(&Gpu),
    ) {
        let background = game.map_or(Background::Void, Background::Game);
        self.draw_view(gpu, target, eye, fov, background, layers, true, between);
    }

    /// The game's depth, bloom and lens as another camera at the eye `first` was drawn from sees
    /// them (`eye`, `fov`; the desktop view's), into `out`: what the runtime draws over the
    /// reprojected picture hides behind what the game drew and gets its light, as in the headset.
    pub fn reproject_post(&self, gpu: &Gpu, out: &PostTargets, eye: &Pose, fov: &Fov, first: &PostSource, second: Option<&PostSource>, metres: f32) {
        let ctx = &gpu.ctx;
        let vp = projection(fov) * view_matrix(eye);
        let mut cb = DrawCb::zeroed();
        cb.view_proj = vp.to_cols_array();
        cb.inv_view_proj = vp.inverse().to_cols_array();
        cb.view_proj_src = (projection(&first.fov) * view_matrix(&first.pose)).to_cols_array();
        let b = second.map(|s| (projection(&s.fov) * view_matrix(&s.pose), &s.pose));
        let fwd = |p: &Pose| (quat(p.rot) * Vec3::Z).extend(0.0).to_array();
        let post = ReprojectPostCb { fwd_a: fwd(&first.pose), fwd_b: second.map_or([0.0; 4], |s| fwd(&s.pose)), fwd_cam: fwd(eye) };
        let rtvs = [out.depth.rtv.clone(), out.bloom.rtv.clone(), out.lens.rtv.clone()];
        unsafe {
            for rtv in rtvs.iter().flatten() {
                ctx.ClearRenderTargetView(rtv, &[0.0; 4]);
            }
            ctx.OMSetRenderTargets(Some(&rtvs), None);
            ctx.RSSetState(&self.raster);
            ctx.OMSetDepthStencilState(&self.no_depth, 0);
            ctx.OMSetBlendState(&self.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.cb.update(ctx, &cb);
            self.reproject_cb.update(ctx, &ReprojectCb::new(Vec3::from_array(first.pose.pos), b, true, metres));
            self.post_cb.update(ctx, &post);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone()), Some(self.reproject_cb.buf.clone()), Some(self.post_cb.buf.clone())]));
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            pipeline::viewport(ctx, out.depth.width, out.depth.height);
            self.full_post.bind(ctx);
            let srvs = [
                Some(first.depth.clone()),
                second.map(|s| s.depth.clone()),
                first.bloom.cloned(),
                second.and_then(|s| s.bloom.cloned()),
                first.lens.cloned(),
                second.and_then(|s| s.lens.cloned()),
            ];
            ctx.PSSetShaderResources(2, Some(&srvs));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(2, Some(&[None, None, None, None, None, None]));
            ctx.OMSetRenderTargets(None, None);
        }
    }

    /// [`Compositor::draw_eye`] for any camera, with any [`Background`], and with or without the
    /// comfort vignette and fade (`comfort`: a desktop view has none of the headset's).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_view(
        &self,
        gpu: &Gpu,
        target: &EyeTarget,
        eye: &Pose,
        fov: &Fov,
        background: Background,
        layers: &Layers,
        comfort: bool,
        between: &mut dyn FnMut(&Gpu),
    ) {
        let ctx = &gpu.ctx;
        let view_proj = projection(fov) * view_matrix(eye);
        let mut cb = DrawCb::zeroed();
        cb.view_proj = view_proj.to_cols_array();
        cb.inv_view_proj = view_proj.inverse().to_cols_array();
        cb.uv_rect = [0.0, 0.0, 1.0, 1.0];
        self.bind(gpu, target);
        let keep = matches!(background, Background::Keep);
        unsafe {
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            match background {
                Background::Keep => {}
                Background::Game(srv) => {
                    self.cb.update(ctx, &cb);
                    self.full_blit.bind(ctx);
                    ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
                }
                Background::Void => {
                    self.cb.update(ctx, &cb);
                    self.full_void.bind(ctx);
                    ctx.PSSetShaderResources(0, Some(&[None]));
                }
                Background::Reprojected { first, second, second_depth, metres, fog } => {
                    let mut c = cb;
                    c.view_proj_src = (projection(&first.2) * view_matrix(&first.1)).to_cols_array();
                    self.cb.update(ctx, &c);
                    // (Nothing to fall back on: a matrix that puts everything behind it.)
                    let b = second.as_ref().map(|(_, pose, fov)| (projection(fov) * view_matrix(pose), pose));
                    self.reproject_cb.update(ctx, &ReprojectCb::new(Vec3::from_array(first.1.pos), b, second_depth.is_some(), metres));
                    self.refog_cb.update(ctx, &RefogCb::new(fog.as_ref().filter(|_| second.is_some())));
                    ctx.PSSetConstantBuffers(1, Some(&[Some(self.reproject_cb.buf.clone())]));
                    ctx.PSSetConstantBuffers(3, Some(&[Some(self.refog_cb.buf.clone())]));
                    self.full_reproject.bind(ctx);
                    ctx.PSSetShaderResources(0, Some(&[Some(first.0.clone()), second.map(|s| s.0.clone()), None, second_depth.cloned()]));
                }
            }
            if !keep {
                ctx.Draw(3, 0);
            }
            ctx.PSSetShaderResources(0, Some(&[None, None, None, None]));
        }
        if !layers.world_panels.is_empty() {
            self.bind(gpu, target);
            unsafe {
                ctx.OMSetBlendState(&self.premul, Some(&[0.0; 4]), 0xFFFF_FFFF);
                ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            }
            self.quad.bind(ctx);
            for p in &layers.world_panels {
                self.draw_panel(gpu, &cb, p);
            }
            unsafe { ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST) };
        }
        self.cb.update(ctx, &cb);
        between(gpu);
        self.bind(gpu, target);
        unsafe {
            ctx.OMSetBlendState(&self.premul, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        }
        self.quad.bind(ctx);
        let eye_pos = Vec3::from_array(eye.pos);
        let eye_rot = quat(eye.rot);

        if let Some(l) = &layers.lens {
            let (u0, u1) = if l.aspect > 1.0 { (0.5 - 0.5 / l.aspect, 0.5 + 0.5 / l.aspect) } else { (0.0, 1.0) };
            let (v0, v1) = if l.aspect < 1.0 { (0.5 - 0.5 * l.aspect, 0.5 + 0.5 * l.aspect) } else { (0.0, 1.0) };
            let mut c = cb;
            c.center = l.pos.extend(1.0).to_array();
            c.axis_x = (l.right * l.radius).extend(0.0).to_array();
            c.axis_y = (l.up * l.radius).extend(0.0).to_array();
            c.uv_rect = [u0, v0, u1, v1];
            c.params = [MODE_LENS, l.reticle as f32, 1.0, 0.0];
            self.draw_quad(gpu, &c, Some(l.srv));
        }
        // (The headset's own, `private`, only in the headset: `comfort` is only for it.)
        for p in layers.panels.iter().chain(layers.private.iter().filter(|_| comfort)) {
            self.draw_panel(gpu, &cb, p);
        }
        for (pos, rot) in &layers.markers {
            self.draw_ribbon(gpu, &cb, eye_pos, *pos, *pos + *rot * Vec3::Z * 0.08, 0.012, Vec4::new(0.35, 0.55, 0.65, 0.9));
            self.draw_dot(gpu, &cb, eye_rot, *pos, 0.022, Vec4::new(0.5, 0.7, 0.8, 0.95));
        }
        for g in &layers.glows {
            let mut c = cb;
            c.center = g.at.extend(1.0).to_array();
            c.axis_x = (eye_rot * Vec3::X * g.radius).extend(0.0).to_array();
            c.axis_y = (eye_rot * Vec3::Y * g.radius).extend(0.0).to_array();
            c.color = g.color.extend(0.0).to_array();
            c.params = [MODE_GLOW, 0.0, 1.0, 0.0];
            self.draw_quad(gpu, &c, None);
        }
        for r in &layers.ribbons {
            self.draw_ribbon(gpu, &cb, eye_pos, r.a, r.b, r.width, r.color);
        }
        for b in &layers.bubbles {
            let mut c = cb;
            c.center = b.at.extend(1.0).to_array();
            c.axis_x = (eye_rot * Vec3::X * b.radius).extend(0.0).to_array();
            c.axis_y = (eye_rot * Vec3::Y * b.radius).extend(0.0).to_array();
            c.color = b.color.extend(0.0).to_array();
            c.params = [MODE_BUBBLE, 0.0, 1.0, 0.0];
            self.draw_quad(gpu, &c, None);
        }
        if let Some(l) = &layers.laser {
            // A thread of light in a glow, fainter and fading out when it points at nothing.
            let k = if l.hit { 1.0 } else { 0.5 };
            self.draw_beam(gpu, &cb, eye_pos, l, 0.012, k);
            if l.hit {
                // Where it points: the light it casts, and a dot like the game's own cursor (a dark
                // edge round the glow's colour, a bright heart), pressed in while the trigger's held.
                let r = if l.down { 0.75 } else { 1.0 };
                let mut c = cb;
                c.center = l.end.extend(1.0).to_array();
                c.axis_x = (eye_rot * Vec3::X * 0.025 * r).extend(0.0).to_array();
                c.axis_y = (eye_rot * Vec3::Y * 0.025 * r).extend(0.0).to_array();
                c.color = (l.glow * 0.6).extend(0.0).to_array();
                c.params = [MODE_GLOW, 0.0, 1.0, 0.0];
                self.draw_quad(gpu, &c, None);
                self.draw_dot(gpu, &cb, eye_rot, l.end, 0.0055 * r, Vec4::new(0.01, 0.006, 0.0, 0.85));
                self.draw_dot(gpu, &cb, eye_rot, l.end, 0.0042 * r, l.glow.extend(1.0));
                self.draw_dot(gpu, &cb, eye_rot, l.end, 0.002 * r, l.core.extend(1.0));
            }
        }
        let k = &layers.comfort;
        if comfort && (k.vignette > 0.003 || k.fade > 0.003) {
            let mut c = cb;
            c.center = k.forward.normalize_or(Vec3::Z).extend(0.0).to_array();
            c.color = [k.vignette.clamp(0.0, 1.0), k.aperture, k.feather.max(0.01), k.fade.clamp(0.0, 1.0)];
            self.cb.update(ctx, &c);
            unsafe {
                ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                self.full_comfort.bind(ctx);
                ctx.PSSetShaderResources(0, Some(&[None]));
                ctx.Draw(3, 0);
            }
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
    }

    fn draw_panel(&self, gpu: &Gpu, cb: &DrawCb, p: &PanelLayer) {
        let mut c = *cb;
        c.center = p.panel.center.extend(1.0).to_array();
        c.axis_x = (p.panel.right * p.panel.width * 0.5).extend(0.0).to_array();
        c.axis_y = (p.panel.up * p.panel.height * 0.5).extend(0.0).to_array();
        c.params = [MODE_TEXTURE, 0.0, p.opacity, if p.additive { 1.0 } else { 0.0 }];
        c.color = [p.soft[0], p.soft[1], if p.solid { 1.0 } else { 0.0 }, 0.0];
        c.uv_rect = p.uv;
        self.draw_quad(gpu, &c, Some(p.srv));
    }

    fn draw_quad(&self, gpu: &Gpu, cb: &DrawCb, srv: Option<&ID3D11ShaderResourceView>) {
        self.cb.update(&gpu.ctx, cb);
        unsafe {
            gpu.ctx.PSSetShaderResources(0, Some(&[srv.cloned()]));
            gpu.ctx.Draw(4, 0);
        }
    }

    /// A camera-facing strip from `a` to `b`.
    fn draw_ribbon(&self, gpu: &Gpu, base: &DrawCb, eye: Vec3, a: Vec3, b: Vec3, width: f32, color: Vec4) {
        let mid = (a + b) * 0.5;
        let along = (b - a) * 0.5;
        if along.length_squared() < 1e-10 {
            return;
        }
        let side = along.cross(eye - mid).normalize_or_zero() * width * 0.5;
        let mut c = *base;
        c.center = mid.extend(1.0).to_array();
        c.axis_x = side.extend(0.0).to_array();
        c.axis_y = along.extend(0.0).to_array();
        c.color = color.to_array();
        c.params = [MODE_SOLID, 0.0, 1.0, 0.0];
        self.draw_quad(gpu, &c, None);
    }

    /// The laser's beam, `width` wide, turned to the eye, its light `k` as strong.
    fn draw_beam(&self, gpu: &Gpu, base: &DrawCb, eye: Vec3, l: &Laser, width: f32, k: f32) {
        let mid = (l.origin + l.end) * 0.5;
        let along = (l.end - l.origin) * 0.5;
        if along.length_squared() < 1e-10 {
            return;
        }
        let side = along.cross(eye - mid).normalize_or_zero() * width * 0.5;
        let mut c = *base;
        c.center = mid.extend(1.0).to_array();
        c.axis_x = side.extend(0.0).to_array();
        c.axis_y = along.extend(0.0).to_array();
        c.color = (l.core * k).extend(1.0).to_array();
        c.uv_rect = (l.glow * k).extend(0.0).to_array();
        c.params = [MODE_BEAM, l.time, if l.hit { 0.0 } else { 1.0 }, along.length() * 2.0];
        self.draw_quad(gpu, &c, None);
    }

    fn draw_dot(&self, gpu: &Gpu, base: &DrawCb, eye_rot: Quat, at: Vec3, radius: f32, color: Vec4) {
        let mut c = *base;
        c.center = at.extend(1.0).to_array();
        c.axis_x = (eye_rot * Vec3::X * radius).extend(0.0).to_array();
        c.axis_y = (eye_rot * Vec3::Y * radius).extend(0.0).to_array();
        c.color = color.to_array();
        c.params = [MODE_DOT, 0.0, 1.0, 0.0];
        self.draw_quad(gpu, &c, None);
    }
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

pub fn view_matrix(eye: &Pose) -> Mat4 {
    Mat4::from_rotation_translation(quat(eye.rot), Vec3::from_array(eye.pos)).inverse()
}

/// Left-handed off-centre projection with D3D depth, from FOV tangents.
pub fn projection(fov: &Fov) -> Mat4 {
    let w = fov.right - fov.left;
    let h = fov.up - fov.down;
    let range = FAR / (FAR - NEAR);
    Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new(-(fov.left + fov.right) / w, -(fov.up + fov.down) / h, range, 1.0),
        Vec4::new(0.0, 0.0, -range * NEAR, 0.0),
    )
}

/// Screen UV (0..1, top-left origin) of a tracking-space point seen from an eye, if in front.
pub fn project(eye: &Pose, fov: &Fov, p: Vec3) -> Option<Vec2> {
    let c = projection(fov) * view_matrix(eye) * p.extend(1.0);
    if c.w <= 0.0 {
        return None;
    }
    Some(Vec2::new(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustvr_gpu::{TexDesc, Texture};
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

    /// Gamebryo's fog for a point `r` along `dir` from an eye: by the length of its clip-space x, y
    /// and depth (Oblivion's `SLS2000.vso`: `length(mul(ModelViewProj, v).xyz)`).
    fn gamebryo_fog(pose: &Pose, fov: &Fov, dir: Vec3, r: f32, far: f32) -> f32 {
        let c = projection(fov) * view_matrix(pose) * dir.extend(0.0) * r;
        (Vec3::new(c.x, c.y, c.w).length() / far).clamp(0.0, 1.0)
    }

    #[test]
    fn the_other_eyes_picture_takes_the_first_eyes_fog() {
        let gpu = Gpu::new().unwrap();
        // Oblivion's eyes in the playtest (its log's frustums: the headset's and 11 degrees of
        // overscan), the right eye the desktop view's; a world of far hills all round, fogged
        // linearly from the eye out to `far`.
        let (l, r, u, d) = (-2.1376505, 1.2318016, 1.4244225, -2.2386353);
        let eyes = [
            (Pose { pos: [-0.032, 0.0, 0.0], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] }, Fov { left: l, right: r, up: u, down: d }),
            (Pose { pos: [0.032, 0.0, 0.0], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] }, Fov { left: -r, right: -l, up: u, down: d }),
        ];
        let (base, fog) = (Vec3::new(0.2, 0.5, 0.2), Vec3::new(0.7, 0.75, 0.8));
        let check = |hills: f32, far: f32| -> (f32, f32) {
            let size = 256u32;
            let mut pictures = Vec::new();
            for (pose, fov) in &eyes {
                let (mut colour, mut depth) = (Vec::new(), Vec::new());
                for y in 0..size {
                    for x in 0..size {
                        let n = Vec2::new((x as f32 + 0.5) / size as f32 * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / size as f32 * 2.0);
                        let tan = Vec2::new((n.x * (fov.right - fov.left) + fov.left + fov.right) / 2.0, (n.y * (fov.up - fov.down) + fov.up + fov.down) / 2.0);
                        let dir = tan.extend(1.0).normalize();
                        let c = base.lerp(fog, gamebryo_fog(pose, fov, dir, hills, far));
                        colour.extend([c.x, c.y, c.z, 1.0].map(|v| (v * 255.0).round() as u8));
                        depth.extend_from_slice(&(hills * dir.z).to_ne_bytes());
                    }
                }
                let colour = Texture::upload(&gpu, size, size, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &[&colour]).unwrap();
                let depth = Texture::upload(&gpu, size, size, DXGI_FORMAT_R32_FLOAT, &[&depth]).unwrap();
                pictures.push((colour, depth));
            }
            // The desktop view: 128 degrees across from the right eye, looking ahead.
            let (w, h) = (512u32, 288u32);
            let out = Texture::new(&gpu, &TexDesc::target(w, h, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)).unwrap();
            let target = EyeTarget { tex: out.tex.clone(), rtv: out.rtv.clone().unwrap(), width: w, height: h };
            let tan = 64f32.to_radians().tan();
            let view = Fov { left: -tan, right: tan, up: tan * h as f32 / w as f32, down: -tan * h as f32 / w as f32 };
            let mut errs = (0.0f32, 0.0f32);
            for refog in [false, true] {
                let background = Background::Reprojected {
                    first: (pictures[1].0.srv.as_ref().unwrap(), eyes[1].0, eyes[1].1),
                    second: Some((pictures[0].0.srv.as_ref().unwrap(), eyes[0].0, eyes[0].1)),
                    second_depth: pictures[0].1.srv.as_ref(),
                    metres: 1.0,
                    fog: refog.then_some(ClipFog { color: fog.to_array(), fade: [0.0, 1.0 / far] }),
                };
                Compositor::new(&gpu).unwrap().draw_view(&gpu, &target, &eyes[1].0, &view, background, &Layers::default(), false, &mut |_| {});
                let px = out.read_rgba8(&gpu).unwrap();
                // Along the middle row, the fog each pixel shows against the right eye's, carried on
                // past its edge.
                let mut worst = 0.0f32;
                for x in 0..w {
                    let ndc = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                    let dir = Vec3::new(ndc * tan, 0.0, 1.0).normalize();
                    let at = ((h / 2 * w + x) * 4) as usize;
                    let shown = (px[at + 2] as f32 / 255.0 - base.z) / (fog.z - base.z);
                    let want = gamebryo_fog(&eyes[1].0, &eyes[1].1, dir, hills, far);
                    worst = worst.max((shown - want).abs());
                }
                if refog { errs.1 = worst } else { errs.0 = worst }
            }
            errs
        };
        // Hills far off (found by direction in the left eye's picture) and near (followed out
        // through its depth).
        for (hills, far) in [(500.0, 1000.0), (40.0, 80.0)] {
            let (before, after) = check(hills, far);
            assert!(before > 0.06, "the left eye's fog differs at {hills} m: by {before}");
            assert!(after < 0.02, "the fog carries on past the right eye's edge at {hills} m: off by {after}");
        }
    }
}
