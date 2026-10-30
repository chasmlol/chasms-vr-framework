//! Blood, drawn: what the runtime's gore (`rustvr-runtime`'s `gore.rs`) makes, over the game's
//! picture with its depth, lit as the body is lit.
//!
//! * **Decals** on the game's world: each a box projected onto whatever the game drew inside it
//!   (its depth gives the surface, so a splat lies on the real wall, not the collision under it),
//!   textured with the game's own blood decal, stain or drip. Drawn before the body, so the player's
//!   hands hide them.
//! * **Caps** on stumps: a dome of the limb's size over the cut, the game's own flesh on it.
//! * **Drops** in the air: sprites stretched along their flight, the game's blood particle on
//!   them, softened where they meet the game's surfaces. Drawn after the body, in its depth.
//! * **Splats** on the player's body and what the hands hold: spheres where blood got on them,
//!   given to the body's own shader (it reddens and wets what's inside them).
//!
//! Each list is one instanced draw per kind of blood (its textures), with the instances in a
//! structured buffer. Everything comes in world space with the eye's world -> tracking matrix, so
//! blood stays where the game's picture has the world.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use rustvr_asset as asset;
use rustvr_gpu::Gpu;
use rustvr_gpu::pipeline::{self, ConstantBuffer};
use rustvr_gpu::shader::Program;
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_SRV_DIMENSION_BUFFER};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;

use crate::render::GpuTex;

/// The most of each the renderer takes in a frame (the simulation's own caps are these).
pub const MAX_DROPS: usize = 3072;
pub const MAX_DECALS: usize = 640;
pub const MAX_CAPS: usize = 48;
pub const MAX_SPLATS: usize = 48;

/// A drop in the air (world space).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GoreDrop {
    pub pos: [f32; 3],
    /// Its radius (m).
    pub size: f32,
    /// How it's moving (m/s): it's drawn stretched along this.
    pub vel: [f32; 3],
    pub alpha: f32,
    /// Its blood (an index into [`GoreFrame::bloods`]).
    pub blood: u32,
    /// 0 a drop, 1 mist (finer, fainter).
    pub mist: f32,
    pub _p: [f32; 2],
}

/// Which of a blood's textures a decal shows.
pub const DECAL_SPLAT: u32 = 0;
pub const DECAL_SPLATTER: u32 = 1;
pub const DECAL_STAIN: u32 = 2;
pub const DECAL_DRIP: u32 = 3;

/// A decal on the world (world space): the box `center ± u ± v ± n` (half axes; `n` along the
/// surface's normal, out of it), its texture across `u` and `v`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GoreDecal {
    pub center: [f32; 3],
    /// Which of the splat atlas's tiles (splats only).
    pub tile: u32,
    pub u: [f32; 3],
    /// 1 fresh and wet .. 0 dry.
    pub wet: f32,
    pub v: [f32; 3],
    pub tex: u32, // DECAL_*
    pub n: [f32; 3],
    pub blood: u32,
}

/// A stump's cap (world space): a dome `radius` across over the cut at `center`, bulging along
/// `out`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GoreCap {
    pub center: [f32; 3],
    pub radius: f32,
    pub out: [f32; 3],
    pub blood: u32,
}

/// Blood on the player or what the hands hold (tracking space): inside `radius` of `pos`, wet as
/// `wet` (0 dry, dark).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GoreSplat {
    pub pos: [f32; 3],
    pub radius: f32,
    /// The surface's normal where it landed (the texture is laid across it).
    pub normal: [f32; 3],
    pub wet: f32,
}

/// One kind of blood's look: the game's textures (`<game>/<asset key>`; empty: none), as the
/// protocol's [`rustvr_protocol::Blood`] names them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GoreBlood {
    pub splat: String,
    pub splatter: String,
    pub stain: String,
    pub drip: String,
    pub drop: String,
    pub mist: String,
    pub flesh: String,
    /// The splat texture's atlas (n x n splats).
    pub atlas: u32,
    /// It glows (drawn unlit).
    pub glow: bool,
    /// Dust, not a liquid: never wet.
    pub dust: bool,
}

/// What to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct GoreFrame<'a> {
    pub bloods: &'a [GoreBlood],
    pub drops: &'a [GoreDrop],
    pub decals: &'a [GoreDecal],
    pub caps: &'a [GoreCap],
    pub splats: &'a [GoreSplat],
    /// The blood the splats are (the last to get on the player).
    pub splat_blood: u32,
    /// World -> tracking for this eye (the mapping its picture of the world was drawn with).
    pub world_to_tracking: Mat4,
}

// ---- the GPU side -----------------------------------------------------------------------------------

/// The decals, drops and caps (`gore` in the avatar's shader source, after its own).
pub(crate) const HLSL: &str = r#"
// ---- Blood (gore.rs) ----
struct GDrop { float3 pos; float size; float3 vel; float alpha; uint blood; float mist; float2 _p; };
struct GDecal { float3 center; uint tile; float3 u; float wet; float3 v; uint tex; float3 n; uint blood; };
struct GCap { float3 center; float radius; float3 outv; uint blood; };
cbuffer Gore : register(b7) {
    float4x4 g_world;   // world -> tracking (this eye's picture of the world)
    float4 g_info;      // x: the game's lights, y: unlit (it glows), z: dust, w: the first instance
    float4 g_tex;       // x: the texture is there, y: its atlas (tiles a side), z: its shape (0 its alpha, 1 its red, 2 its alpha in the blood's colour), w: a drop's stretch (s)
    float4 g_color;     // the blood's colour (the game's texture's, display range)
};
StructuredBuffer<GDrop> g_drops : register(t16);
StructuredBuffer<GDecal> g_decals : register(t17);
StructuredBuffer<GCap> g_caps : register(t18);
Texture2D g_tex0 : register(t19);

float3 g_tw(float3 p) { return mul(g_world, float4(p, 1)).xyz; }
float3 g_tv(float3 v) { return mul((float3x3)g_world, v); }

// Lit as the body is: the game's lights when it gives them, wet blood's highlight.
float3 g_lit(float3 p, float3 n, float3 base, float wet) {
    if (g_info.y > 0) return base;
    float3 v = normalize(eye_pos.xyz - p);
    float gloss = lerp(6, 70, wet);
    float mask = 0.15 + 0.75 * wet;
    float3 d = 0, s = 0;
    if (g_info.x > 0) {
        uint count = (uint)envc.w;
        for (uint k = 0; k < count; k++) gb_light(lights[k], p, n, v, false, gloss, mask, d, s);
        return saturate(base * saturate(amb.rgb + d) + s);
    }
    float3 l = normalize(sun_dir.xyz);
    float sp = pow(saturate(dot(n, normalize(l + v))), gloss) * mask;
    return saturate(base * saturate(ambient.rgb + sun_color.rgb * saturate(dot(n, l))) + sun_color.rgb * sp);
}

float3 g_fog(float3 c, float3 p) {
    if (g_info.x > 0 && fog_fade.y != 0) c = lerp(c, fog_color.rgb, saturate(length(eye_pos.xyz - p) * fog_fade.y + fog_fade.x));
    return c;
}

// The game's depth where this pixel is (metres along the view), 0 where it drew nothing.
float g_game_depth(float4 svpos) {
    if (depth.x == 0) return 0;
    uint w, h;
    game_depth.GetDimensions(w, h);
    int2 px = int2(svpos.xy * float2(w, h) / target_size.xy);
    return game_depth.Load(int3(min(px, int2(w, h) - 1), 0)) * depth.y;
}

// Blood dries dark, towards brown.
float3 g_dry(float3 c, float wet) { return lerp(c * float3(0.42, 0.36, 0.34), c, saturate(wet * 1.4)); }

float4 g_out(float3 display, float a) { return float4(m_display_to_linear(display) * a, a); }

// ---- decals: the box's far side, each pixel the game's surface inside the box ----
struct DecalOut {
    float4 pos : SV_Position;
    float3 world : TEXCOORD0;
    nointerpolation float3 c : TEXCOORD1;
    nointerpolation float3 u : TEXCOORD2;
    nointerpolation float3 v : TEXCOORD3;
    nointerpolation float3 n : TEXCOORD4;
    nointerpolation float3 face : TEXCOORD5;
    nointerpolation float3 info : TEXCOORD6;   // x: wet, y: tile
};

static const uint G_QUAD[6] = { 0, 1, 2, 2, 1, 3 };

DecalOut VSDecal(uint vid : SV_VertexID, uint inst : SV_InstanceID) {
    GDecal d = g_decals[inst + (uint)g_info.w];
    float3 ax[3] = { g_tv(d.u), g_tv(d.v), g_tv(d.n) };
    uint face = vid / 6;
    uint q = G_QUAD[vid % 6];
    float s = (q & 1) ? 1 : -1, t = (q & 2) ? 1 : -1;
    uint a = face / 2;
    float sign = (face & 1) ? 1 : -1;
    float3 A = ax[a], B = ax[(a + 1) % 3], C = ax[(a + 2) % 3];
    float3 c = g_tw(d.center);
    float3 p = c + A * sign + B * s + C * t;
    DecalOut o;
    o.pos = mul(view_proj, float4(p, 1));
    o.world = p;
    o.c = c; o.u = ax[0]; o.v = ax[1]; o.n = ax[2];
    o.face = A * sign;
    o.info = float3(d.wet, d.tile, 0);
    return o;
}

float4 PSDecal(DecalOut i) : SV_Target {
    float g = g_game_depth(i.pos);
    float zb = dot(view_z.xyz, i.world) + view_z.w;
    float3 sp = eye_pos.xyz + (i.world - eye_pos.xyz) * (g / max(zb, 1e-4));
    float3 dpx = ddx(sp), dpy = ddy(sp);
    // Only the box's far side draws (each pixel once, from inside it too).
    if (dot(i.face, eye_pos.xyz - i.world) > 0 || g <= 0) discard;
    float3 d = sp - i.c;
    float3 l = float3(dot(d, i.u) / dot(i.u, i.u), dot(d, i.v) / dot(i.v, i.v), dot(d, i.n) / dot(i.n, i.n));
    if (any(abs(l) > 1)) discard;
    float3 sn = normalize(cross(dpy, dpx));
    if (dot(sn, eye_pos.xyz - sp) < 0) sn = -sn;
    // Not on what faces away from where it was thrown at.
    float facing = dot(sn, normalize(i.n));
    if (facing < 0.2) discard;
    float2 uv = l.xy * 0.5 + 0.5;
    uint tiles = (uint)max(g_tex.y, 1);
    uint tile = (uint)i.info.y;
    uv = (uv + float2(tile % tiles, (tile / tiles) % tiles)) / tiles;
    float4 t = g_tex0.Sample(clamp_samp, uv);
    // (A run's streak is the arena's drips, red on black; a splatter is the dungeons' streaks, in
    // this blood's colour; a splat or a stain is the texture as it is.)
    float a = g_tex.z > 0.5 && g_tex.z < 1.5 ? saturate(t.r * 4) : t.a;
    float3 base = g_tex.z > 0.5 ? g_color.rgb * (g_tex.z > 1.5 ? 0.75 + 0.5 * saturate(dot(t.rgb, 3)) : 1) : t.rgb;
    float wet = g_info.z > 0 ? 0 : i.info.x;
    base = g_dry(base, wet);
    // (Fading out at the box's depth, and on surfaces at a slant.)
    a *= saturate((1 - abs(l.z)) * 4) * saturate((facing - 0.2) * 4);
    if (a < 0.01) discard;
    float3 col = g_fog(g_lit(sp, sn, base, wet), sp);
    return g_out(col, a);
}

// ---- drops: sprites drawn out along their flight ----
struct DropOut {
    float4 pos : SV_Position;
    float3 world : TEXCOORD0;
    float2 uv : TEXCOORD1;
    float view_z : TEXCOORD2;
    nointerpolation float2 info : TEXCOORD3;   // x: alpha, y: size
    float3 n : TEXCOORD4;
};

DropOut VSDrop(uint vid : SV_VertexID, uint inst : SV_InstanceID) {
    GDrop d = g_drops[inst + (uint)g_info.w];
    float3 p = g_tw(d.pos);
    float3 vel = g_tv(d.vel);
    float3 f = normalize(p - eye_pos.xyz);
    float3 along = vel - f * dot(vel, f);
    float speed = length(along);
    float3 other = abs(f.y) < 0.95 ? float3(0, 1, 0) : float3(1, 0, 0);
    along = speed > 1e-4 ? along / speed : normalize(cross(f, other));
    float3 side = normalize(cross(f, along));
    uint q = G_QUAD[vid % 6];
    float2 c = float2((q & 1) ? 1 : -1, (q & 2) ? 1 : -1);
    // (Drawn a little bigger than they are: a drop of blood catches the light.)
    float r = d.size * (d.mist > 0 ? 5.0 : 2.2);
    float len = r + speed * g_tex.w;
    float3 w = p + side * c.x * r + along * c.y * len;
    DropOut o;
    o.pos = mul(view_proj, float4(w, 1));
    o.world = w;
    o.uv = c * 0.5 + 0.5;
    o.view_z = dot(view_z.xyz, w) + view_z.w;
    o.info = float2(d.alpha, r);
    o.n = normalize(-f + side * c.x * 0.6);
    return o;
}

float4 PSDrop(DropOut i) : SV_Target {
    if (behind_game(i.pos, i.view_z)) discard;
    float4 t;
    if (g_tex.x > 0) {
        t = g_tex0.Sample(clamp_samp, i.uv);
    } else {
        float2 q = i.uv * 2 - 1;
        t = float4(g_color.rgb, saturate((1 - dot(q, q)) * 3));
    }
    float a = t.a * i.info.x;
    // Soft where it meets the game's surfaces.
    float g = g_game_depth(i.pos);
    if (g > 0) a *= saturate((g - i.view_z) / max(i.info.y * 4, 0.003));
    if (a < 0.01) discard;
    float3 col = g_fog(g_lit(i.world, normalize(i.n), t.rgb, g_info.z > 0 ? 0 : 1), i.world);
    return g_out(col, a);
}

// ---- caps: a dome over a stump's cut, the game's flesh across it ----
#define CAP_RINGS 4
#define CAP_SEGS 14
struct CapOut {
    float4 pos : SV_Position;
    float3 world : TEXCOORD0;
    float3 n : TEXCOORD1;
    float2 uv : TEXCOORD2;
    float view_z : TEXCOORD3;
};

static const uint2 G_CAP_QUAD[6] = { uint2(0, 0), uint2(1, 0), uint2(0, 1), uint2(0, 1), uint2(1, 0), uint2(1, 1) };

CapOut VSCap(uint vid : SV_VertexID, uint inst : SV_InstanceID) {
    GCap c = g_caps[inst + (uint)g_info.w];
    uint quad = vid / 6;
    uint2 q = G_CAP_QUAD[vid % 6];
    uint seg = quad % CAP_SEGS + q.x, ring = quad / CAP_SEGS + q.y;
    float phi = seg * 6.2831853 / CAP_SEGS;
    float rr = 1 - (float)ring / CAP_RINGS;            // 1 at the rim, 0 at the top
    float lift = sqrt(saturate(1 - rr * rr));
    float3 o = normalize(g_tv(c.outv));
    float3 other = abs(o.y) < 0.95 ? float3(0, 1, 0) : float3(1, 0, 0);
    float3 a = normalize(cross(o, other)), b = cross(o, a);
    float r = c.radius * length(g_tv(float3(1, 0, 0)));
    float2 xy = float2(cos(phi), sin(phi)) * rr;
    // A low dome, its rim a little inside the stump.
    float3 p = g_tw(c.center) + (a * xy.x + b * xy.y) * r * 1.05 + o * r * (0.35 * lift - 0.12);
    CapOut co;
    co.pos = mul(view_proj, float4(p, 1));
    co.world = p;
    co.n = normalize(a * xy.x + b * xy.y + o * (lift * 1.6 + 0.2));
    co.uv = xy * 0.5 + 0.5;
    co.view_z = dot(view_z.xyz, p) + view_z.w;
    return co;
}

float4 PSCap(CapOut i) : SV_Target {
    if (behind_game(i.pos, i.view_z)) discard;
    float3 base = g_tex.x > 0 ? g_tex0.Sample(samp, i.uv).rgb : g_color.rgb;
    float3 col = g_fog(g_lit(i.world, normalize(i.n), base, 0.8), i.world);
    return float4(m_display_to_linear(col), 1);
}
"#;

const CAP_RINGS: usize = 4;
const CAP_SEGS: usize = 14;

/// The constants of a gore draw (`cbuffer Gore`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GoreCb {
    world: [f32; 16],
    info: [f32; 4],
    tex: [f32; 4],
    color: [f32; 4],
}

/// The splats on the player for the body's shader (`cbuffer Splats`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct SplatCb {
    pub pos: [[f32; 4]; MAX_SPLATS],
    pub nrm: [[f32; 4]; MAX_SPLATS],
    /// x: how many, y: the splat texture is bound, z: its atlas.
    pub info: [f32; 4],
    /// The blood's colour (display range).
    pub color: [f32; 4],
}

/// A growable structured buffer, rewritten each frame.
struct Instances {
    buf: Option<ID3D11Buffer>,
    srv: Option<ID3D11ShaderResourceView>,
    capacity: usize,
    stride: usize,
}

impl Instances {
    fn new(stride: usize) -> Self {
        Instances { buf: None, srv: None, capacity: 0, stride }
    }

    fn write(&mut self, gpu: &Gpu, bytes: &[u8]) -> Result<()> {
        let count = bytes.len() / self.stride;
        if count == 0 {
            return Ok(());
        }
        if self.buf.is_none() || count > self.capacity {
            let cap = count.next_power_of_two().max(64);
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: (cap * self.stride) as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
                StructureByteStride: self.stride as u32,
            };
            let mut b = None;
            unsafe { gpu.device.CreateBuffer(&desc, None, Some(&mut b))? };
            let b = b.unwrap();
            let sd = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_UNKNOWN,
                ViewDimension: D3D11_SRV_DIMENSION_BUFFER,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: D3D11_BUFFER_SRV { Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: 0 }, Anonymous2: D3D11_BUFFER_SRV_1 { NumElements: cap as u32 } } },
            };
            let mut v = None;
            unsafe { gpu.device.CreateShaderResourceView(&b, Some(&sd), Some(&mut v))? };
            self.buf = Some(b);
            self.srv = v;
            self.capacity = cap;
        }
        pipeline::write_dynamic(&gpu.ctx, self.buf.as_ref().unwrap(), bytes);
        Ok(())
    }
}

/// A texture as loaded: its views, and its colour (the mean of its pixels by their alpha, display
/// range) for what's drawn plain.
struct Loaded {
    tex: GpuTex,
    color: [f32; 3],
}

/// The renderer's gore: its programs, buffers and the game's textures.
pub(crate) struct GoreGpu {
    decal: Program,
    drop: Program,
    cap: Program,
    cb: ConstantBuffer<GoreCb>,
    pub(crate) splat_cb: ConstantBuffer<SplatCb>,
    drops: Instances,
    decals: Instances,
    caps: Instances,
    textures: HashMap<String, Option<Loaded>>,
    root: PathBuf,
    no_depth: ID3D11DepthStencilState,
    /// This eye's lists in draws: (blood, which texture, first, count).
    decal_groups: Vec<(u32, u32, usize, usize)>,
    drop_groups: Vec<(u32, bool, usize, usize)>,
    cap_groups: Vec<(u32, usize, usize)>,
    bloods: Vec<GoreBlood>,
    world: Mat4,
    pub(crate) splat_tex: Option<ID3D11ShaderResourceView>,
    pub(crate) active: bool,
}

impl GoreGpu {
    pub(crate) fn new(gpu: &Gpu, source: &str) -> Result<GoreGpu> {
        let no_depth = {
            let desc = D3D11_DEPTH_STENCIL_DESC { DepthEnable: false.into(), DepthWriteMask: D3D11_DEPTH_WRITE_MASK_ZERO, DepthFunc: D3D11_COMPARISON_ALWAYS, ..Default::default() };
            let mut s = None;
            unsafe { gpu.device.CreateDepthStencilState(&desc, Some(&mut s))? };
            s.unwrap()
        };
        Ok(GoreGpu {
            decal: Program::with_entries(gpu, source, "gore", "VSDecal", "PSDecal", &[])?,
            drop: Program::with_entries(gpu, source, "gore", "VSDrop", "PSDrop", &[])?,
            cap: Program::with_entries(gpu, source, "gore", "VSCap", "PSCap", &[])?,
            cb: ConstantBuffer::new(gpu)?,
            splat_cb: ConstantBuffer::new(gpu)?,
            drops: Instances::new(size_of::<GoreDrop>()),
            decals: Instances::new(size_of::<GoreDecal>()),
            caps: Instances::new(size_of::<GoreCap>()),
            textures: HashMap::new(),
            root: asset::default_root(),
            no_depth,
            decal_groups: Vec::new(),
            drop_groups: Vec::new(),
            cap_groups: Vec::new(),
            bloods: Vec::new(),
            world: Mat4::IDENTITY,
            splat_tex: None,
            active: false,
        })
    }

    /// A texture by its name (`<game>/<key>`), loaded once.
    fn texture(&mut self, gpu: &Gpu, name: &str) -> Option<&Loaded> {
        if name.is_empty() {
            return None;
        }
        if !self.textures.contains_key(name) {
            let (game, key) = name.split_once('/').unwrap_or(("", name));
            let loaded = asset::load_texture(&self.root.join(game), key)
                .and_then(|t| Ok(Loaded { color: mean_color(&t), tex: crate::render::upload_texture(gpu, &t)? }))
                .map_err(|e| tracing::warn!("gore: texture {name}: {e:#}"))
                .ok();
            if let Some(l) = &loaded {
                tracing::info!("gore: texture {name}: its colour {:.2?}", l.color);
            }
            self.textures.insert(name.to_owned(), loaded);
        }
        self.textures.get(name).and_then(Option::as_ref)
    }

    /// This eye's blood: uploaded and sorted into draws.
    pub(crate) fn set(&mut self, gpu: &Gpu, frame: Option<&GoreFrame>) {
        self.active = false;
        let Some(f) = frame else { return };
        if f.bloods.is_empty() {
            return;
        }
        self.bloods = f.bloods.to_vec();
        self.world = f.world_to_tracking;
        let mut decals: Vec<GoreDecal> = f.decals.iter().take(MAX_DECALS).copied().collect();
        decals.sort_by_key(|d| (d.blood, d.tex));
        let mut drops: Vec<GoreDrop> = f.drops.iter().take(MAX_DROPS).copied().collect();
        drops.sort_by_key(|d| (d.blood, d.mist > 0.0));
        let mut caps: Vec<GoreCap> = f.caps.iter().take(MAX_CAPS).copied().collect();
        caps.sort_by_key(|c| c.blood);
        self.decal_groups = groups(&decals, |d| (d.blood, d.tex)).into_iter().map(|((b, t), s, n)| (b, t, s, n)).collect();
        self.drop_groups = groups(&drops, |d| (d.blood, d.mist > 0.0)).into_iter().map(|((b, m), s, n)| (b, m, s, n)).collect();
        self.cap_groups = groups(&caps, |c| c.blood);
        let ok = self.decals.write(gpu, bytemuck::cast_slice(&decals)).and(self.drops.write(gpu, bytemuck::cast_slice(&drops))).and(self.caps.write(gpu, bytemuck::cast_slice(&caps)));
        if let Err(e) = ok {
            tracing::warn!("gore: {e:#}");
            return;
        }
        // The player's splats: the blood last on them, the game's splat across each.
        let blood = self.bloods.get(f.splat_blood as usize).cloned();
        let atlas = blood.as_ref().map_or(1, |b| b.atlas.max(1));
        let (splat_tex, color) = match blood.as_ref().and_then(|b| self.texture(gpu, &b.splat).map(|l| (l.tex.linear.clone(), l.color))) {
            Some((t, c)) => (Some(t), c),
            None => (None, [0.35, 0.02, 0.02]),
        };
        let mut s: SplatCb = Zeroable::zeroed();
        let n = f.splats.len().min(MAX_SPLATS);
        for (i, sp) in f.splats[..n].iter().enumerate() {
            s.pos[i] = [sp.pos[0], sp.pos[1], sp.pos[2], sp.radius];
            s.nrm[i] = [sp.normal[0], sp.normal[1], sp.normal[2], sp.wet];
        }
        s.info = [n as f32, splat_tex.is_some() as u32 as f32, atlas as f32, 0.0];
        s.color = [color[0], color[1], color[2], 1.0];
        self.splat_cb.update(&gpu.ctx, &s);
        self.splat_tex = splat_tex;
        self.active = true;
    }

    /// No splats on the player (no blood this frame).
    pub(crate) fn clear_splats(&mut self, ctx: &ID3D11DeviceContext) {
        self.splat_cb.update(ctx, &Zeroable::zeroed());
        self.splat_tex = None;
    }

    #[allow(clippy::too_many_arguments)]
    fn constants(&self, gpu: &Gpu, blood: u32, first: usize, tex: bool, color: [f32; 3], kind: u32, shading: bool) {
        let b = self.bloods.get(blood as usize);
        let atlas = if kind == DECAL_SPLAT { b.map_or(1, |b| b.atlas.max(1)) } else { 1 };
        let cb = GoreCb {
            world: self.world.to_cols_array(),
            info: [shading as u32 as f32, b.is_some_and(|b| b.glow) as u32 as f32, b.is_some_and(|b| b.dust) as u32 as f32, first as f32],
            tex: [tex as u32 as f32, atlas as f32, match kind { DECAL_DRIP => 1.0, DECAL_SPLATTER => 2.0, _ => 0.0 }, 0.02],
            color: [color[0], color[1], color[2], 1.0],
        };
        self.cb.update(&gpu.ctx, &cb);
    }

    /// The texture a decal of `kind` takes (the splat's when the blood has none of its own).
    fn decal_texture(b: &GoreBlood, kind: u32) -> (&str, u32) {
        let want = match kind {
            DECAL_SPLATTER => &b.splatter,
            DECAL_STAIN => &b.stain,
            DECAL_DRIP => &b.drip,
            _ => &b.splat,
        };
        if want.is_empty() { (&b.splat, DECAL_SPLAT) } else { (want, kind) }
    }

    /// Binds the instances' common state.
    fn begin(&self, ctx: &ID3D11DeviceContext, two_sided: &ID3D11RasterizerState) {
        unsafe {
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetConstantBuffers(7, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(7, Some(&[Some(self.cb.buf.clone())]));
            ctx.RSSetState(two_sided);
        }
    }

    /// The decals and the caps over the game's picture, before the body (its hands hide them). The
    /// frame's constants and the game's depth are bound (the avatar's `bind_common`).
    pub(crate) fn draw_world(&mut self, gpu: &Gpu, shading: bool, premultiplied: &ID3D11BlendState, two_sided: &ID3D11RasterizerState, depth_write: &ID3D11DepthStencilState) {
        if !self.active || (self.decal_groups.is_empty() && self.cap_groups.is_empty()) {
            return;
        }
        let ctx = gpu.ctx.clone();
        self.begin(&ctx, two_sided);
        // Decals: blended, without depth (each pixel is the game's own surface).
        if !self.decal_groups.is_empty() {
            unsafe {
                ctx.OMSetBlendState(premultiplied, Some(&[0.0; 4]), 0xFFFF_FFFF);
                ctx.OMSetDepthStencilState(&self.no_depth, 0);
                ctx.VSSetShader(&self.decal.vs, None);
                ctx.PSSetShader(&self.decal.ps, None);
                ctx.VSSetShaderResources(17, Some(&[self.decals.srv.clone()]));
            }
            for (blood, kind, first, count) in self.decal_groups.clone() {
                let Some(b) = self.bloods.get(blood as usize).cloned() else { continue };
                let (name, kind) = Self::decal_texture(&b, kind);
                let Some((srv, color)) = self.texture(gpu, name).map(|l| (l.tex.linear.clone(), l.color)) else { continue };
                // (A run's streak and a splatter are their blood's colour: the splat's.)
                let color = if matches!(kind, DECAL_DRIP | DECAL_SPLATTER) { self.texture(gpu, &b.splat).map_or(color, |l| l.color) } else { color };
                self.constants(gpu, blood, first, true, color, kind, shading);
                unsafe {
                    ctx.PSSetShaderResources(19, Some(&[Some(srv)]));
                    ctx.DrawInstanced(36, count as u32, 0, 0);
                }
            }
        }
        // Caps: solid, in the avatar's depth.
        if !self.cap_groups.is_empty() {
            unsafe {
                ctx.OMSetBlendState(None, Some(&[0.0; 4]), 0xFFFF_FFFF);
                ctx.OMSetDepthStencilState(depth_write, 0);
                ctx.VSSetShader(&self.cap.vs, None);
                ctx.PSSetShader(&self.cap.ps, None);
                ctx.VSSetShaderResources(18, Some(&[self.caps.srv.clone()]));
            }
            for (blood, first, count) in self.cap_groups.clone() {
                let Some(b) = self.bloods.get(blood as usize).cloned() else { continue };
                let flesh = self.texture(gpu, &b.flesh).map(|l| (l.tex.linear.clone(), l.color));
                let splat = self.texture(gpu, &b.splat).map_or([0.35, 0.02, 0.02], |l| l.color);
                self.constants(gpu, blood, first, flesh.is_some(), flesh.as_ref().map_or(splat, |f| f.1), u32::MAX, shading);
                unsafe {
                    ctx.PSSetShaderResources(19, Some(&[flesh.map(|f| f.0)]));
                    ctx.DrawInstanced((CAP_RINGS * CAP_SEGS * 6) as u32, count as u32, 0, 0);
                }
            }
        }
        unsafe {
            ctx.VSSetShaderResources(17, Some(&[None, None]));
            ctx.PSSetShaderResources(19, Some(&[None]));
        }
    }

    /// The drops, after the body, in its depth (hands hide what's behind them).
    pub(crate) fn draw_drops(&mut self, gpu: &Gpu, shading: bool, premultiplied: &ID3D11BlendState, two_sided: &ID3D11RasterizerState, depth_test: &ID3D11DepthStencilState) {
        if !self.active || self.drop_groups.is_empty() {
            return;
        }
        let ctx = gpu.ctx.clone();
        self.begin(&ctx, two_sided);
        unsafe {
            ctx.OMSetBlendState(premultiplied, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(depth_test, 0);
            ctx.VSSetShader(&self.drop.vs, None);
            ctx.PSSetShader(&self.drop.ps, None);
            ctx.VSSetShaderResources(16, Some(&[self.drops.srv.clone()]));
        }
        for (blood, mist, first, count) in self.drop_groups.clone() {
            let Some(b) = self.bloods.get(blood as usize).cloned() else { continue };
            let name = if mist && !b.mist.is_empty() { b.mist.clone() } else { b.drop.clone() };
            let tex = self.texture(gpu, &name).map(|l| (l.tex.linear.clone(), l.color));
            // (Without the game's particle a drop is plain, its blood's colour: the splat's.)
            let splat = self.texture(gpu, &b.splat).map_or([0.35, 0.02, 0.02], |l| l.color);
            self.constants(gpu, blood, first, tex.is_some(), tex.as_ref().map_or(splat, |t| t.1), u32::MAX, shading);
            unsafe {
                ctx.PSSetShaderResources(19, Some(&[tex.map(|t| t.0)]));
                ctx.DrawInstanced(6, count as u32, 0, 0);
            }
        }
        unsafe {
            ctx.VSSetShaderResources(16, Some(&[None]));
            ctx.PSSetShaderResources(19, Some(&[None]));
        }
    }
}

/// Runs of equal keys in a sorted list: (key, first, count).
fn groups<T, K: PartialEq + Copy>(list: &[T], key: impl Fn(&T) -> K) -> Vec<(K, usize, usize)> {
    let mut out: Vec<(K, usize, usize)> = Vec::new();
    for (i, x) in list.iter().enumerate() {
        let k = key(x);
        match out.last_mut() {
            Some(last) if last.0 == k => last.2 += 1,
            _ => out.push((k, i, 1)),
        }
    }
    out
}

/// A texture's colour: the mean of its pixels weighted by their alpha (display range), from a mip
/// no bigger than 32 across (block-compressed ones decoded).
fn mean_color(t: &asset::Texture) -> [f32; 3] {
    let levels = t.mips.len() / t.faces.max(1) as usize;
    let level = (0..levels).find(|&l| (t.width >> l).max(1) <= 32).unwrap_or(levels.saturating_sub(1));
    let (w, h) = ((t.width >> level).max(1) as usize, (t.height >> level).max(1) as usize);
    let Some(data) = t.mips.get(level) else { return [0.35, 0.02, 0.02] };
    let mut pixels: Vec<[f32; 4]> = Vec::new();
    match t.format {
        asset::TexFormat::Rgba8 => pixels.extend(data.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]].map(|x| x as f32 / 255.0))),
        asset::TexFormat::Bc1 | asset::TexFormat::Bc2 | asset::TexFormat::Bc3 => {
            let block = if t.format == asset::TexFormat::Bc1 { 8 } else { 16 };
            let blocks = w.div_ceil(4) * h.div_ceil(4);
            for b in data.chunks_exact(block).take(blocks) {
                let (alpha, colour) = if block == 16 { (&b[..8], &b[8..]) } else { (&b[..0], b) };
                let c565 = |v: u16| [((v >> 11) & 31) as f32 / 31.0, ((v >> 5) & 63) as f32 / 63.0, (v & 31) as f32 / 31.0];
                let c0v = u16::from_le_bytes([colour[0], colour[1]]);
                let c1v = u16::from_le_bytes([colour[2], colour[3]]);
                let (c0, c1) = (c565(c0v), c565(c1v));
                let mix = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
                let bits = u32::from_le_bytes([colour[4], colour[5], colour[6], colour[7]]);
                let four = block == 16 || c0v > c1v;
                for k in 0..16 {
                    let (rgb, mut a) = match ((bits >> (2 * k)) & 3, four) {
                        (0, _) => (c0, 1.0),
                        (1, _) => (c1, 1.0),
                        (2, true) => (mix(c0, c1, 1.0 / 3.0), 1.0),
                        (3, true) => (mix(c0, c1, 2.0 / 3.0), 1.0),
                        (2, false) => (mix(c0, c1, 0.5), 1.0),
                        _ => ([0.0; 3], 0.0),
                    };
                    if t.format == asset::TexFormat::Bc2 {
                        a = ((alpha[k / 2] >> ((k % 2) * 4)) & 15) as f32 / 15.0;
                    } else if t.format == asset::TexFormat::Bc3 {
                        let (a0, a1) = (alpha[0] as f32 / 255.0, alpha[1] as f32 / 255.0);
                        let abits = u64::from_le_bytes([alpha[2], alpha[3], alpha[4], alpha[5], alpha[6], alpha[7], 0, 0]);
                        a = match ((abits >> (3 * k)) & 7) as usize {
                            0 => a0,
                            1 => a1,
                            i if alpha[0] > alpha[1] => (a0 * (8 - i) as f32 + a1 * (i - 1) as f32) / 7.0,
                            6 => 0.0,
                            7 => 1.0,
                            i => (a0 * (6 - i) as f32 + a1 * (i - 1) as f32) / 5.0,
                        };
                    }
                    pixels.push([rgb[0], rgb[1], rgb[2], a]);
                }
            }
        }
        _ => {}
    }
    let (mut sum, mut wsum) = ([0.0f32; 3], 0.0f32);
    for p in &pixels {
        for c in 0..3 {
            sum[c] += p[c] * p[3];
        }
        wsum += p[3];
    }
    if wsum < 1e-4 { [0.35, 0.02, 0.02] } else { sum.map(|x| x / wsum) }
}
