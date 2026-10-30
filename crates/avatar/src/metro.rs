//! Metro 2033 Redux's shading of its first-person model, on the CPU: the reference the avatar's
//! shader mirrors line for line (`METRO_HLSL` in `render.rs`), and what the tests check against
//! the game's own numbers.
//!
//! Everything here follows the game's shaders (docs/research/metro-viewmodel-lighting.md,
//! measured on the game): the G-buffer pixel shaders (section 2.4), the light shaders (3.3),
//! the ambient/combine pass (4.2) and the tonemap and grading (5.5). The game evaluates them as a
//! deferred renderer; the avatar evaluates the same terms for its own fragment in one pass:
//!
//! 1. [`prologue`] and [`finish`]: the G-buffer values the game would have written for the
//!    fragment ([`GBuffer`]): its lighting position (view space, z squashed), its normal, gloss,
//!    albedo and specular intensity, and its class (the skin flag, emissive).
//! 2. [`light`] for each of the game's lights that [`reaches`] it, with the light's texture term
//!    from the light atlas ([`atlas_term`]).
//! 3. [`combine`]: spherical harmonics, flat ambient, occlusion, the skin's subsurface tint.
//! 4. [`tonemap`] and [`grade`]: the game's tone curve and colour grading, giving the code value
//!    the game would have put in its back buffer, and [`display_to_linear`] for writing that code
//!    value through an sRGB render target.
//!
//! Space: the game camera's view space (x right, y up, z forward, metres). The fragment's real
//! position there is `pv`; the lighting position has z multiplied by
//! [`GameLighting::depth_squash`], as the game does for its first-person model.
//!
//! What the game computes from data the runtime doesn't have:
//! * The screen-space reflection term of the combine (the previous frame's blur buffer): 0.
//! * The skin's blurred red light (`aoR`, the SSAO pass's z): the fragment's own red diffuse light,
//!   which makes the subsurface term's `max(D.r, 2·aoR²)` exactly `D.r` (the blur's local value).
//! * The light buffer's chroma subsampling (YCoCg in a checkerboard): the full colour is used.
//! * Fog: zero at the distances the first-person model is lit at (33 m before it starts).

use glam::{Vec2, Vec3, Vec4, Vec4Swizzles};
use rustvr_asset::Shader;
use rustvr_protocol::{
    GL_AMBIENT, GL_PROJECTED, GL_PROJECTED_OCCLUDED, GL_SHADOWED, GL_SHADOWED_PLAIN, GLF_REACHES_VIEWMODEL, GLF_TILE, GameLight, GameLighting,
    LIGHT_ATLAS_H, LIGHT_ATLAS_W, LIGHT_TILE, LIGHT_TILE_DEPTH, LIGHT_TILE_EXTENT, LIGHT_TILE_SLICES, LUT_SIZE,
};

/// `envmap_color` (a frame constant): the environment map's tint, the same value wherever
/// measured. Used when the adapter leaves [`GameLighting::envmap_color`] zero.
pub const ENVMAP_COLOR: Vec3 = Vec3::new(0.5706, 0.6588, 0.75);
/// `fade_params.z`: the detail strength fades with the squared view distance.
pub const FADE_Z: f32 = 0.000178;
/// The height map's neighbour offset, in uv (fixed, whatever the texture's size).
pub const BUMP_STEP: f32 = 1.0 / 2048.0;
/// The game's luminance weights.
pub const LUM: Vec3 = Vec3::new(0.3, 0.48, 0.22);
/// How far in front of the camera a light's volume must contain the pixel's ray (metres).
pub const REACH_DEPTH: f32 = 0.1;

/// The game's shading classes (its G-buffer and forward pixel shaders).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Class {
    /// The plain surface shader (and its alpha-tested variants).
    Plain = 1,
    /// Metal: the environment baked into the albedo.
    MetalEnv = 2,
    /// Cloth and skin: Lambert, subsurface tint, grey specular, grazing sheen.
    ClothSkin = 3,
    /// Glossy glass.
    GlossyGlass = 4,
    /// Emissive (a brighter one is the same ×3).
    Emissive = 5,
    /// Forward: unlit, blended.
    GlassTint = 6,
    /// Forward: the environment reflection, blended.
    GlassReflect = 7,
    /// Not Metro's: Gamebryo's per-pixel lighting (`gamebryo.rs`, Oblivion's `ShadowLightShader`).
    GamebryoLit = 8,
    /// Gamebryo's skin (Oblivion's `SkinShader`).
    GamebryoSkin = 9,
    /// Gamebryo's hair (Oblivion's `HairShader`).
    GamebryoHair = 10,
}

impl Class {
    pub fn of(shader: Shader) -> Option<Class> {
        Some(match shader {
            // The collimator's lens and reticle are `collimator.rs`'s.
            Shader::Unknown | Shader::GlassMultiply | Shader::Distort | Shader::Reticle => return None,
            Shader::Plain => Class::Plain,
            Shader::MetalEnv => Class::MetalEnv,
            Shader::ClothSkin => Class::ClothSkin,
            Shader::GlossyGlass => Class::GlossyGlass,
            Shader::Emissive => Class::Emissive,
            Shader::GlassTint => Class::GlassTint,
            Shader::GlassReflect => Class::GlassReflect,
            Shader::GamebryoLit => Class::GamebryoLit,
            Shader::GamebryoSkin => Class::GamebryoSkin,
            Shader::GamebryoHair => Class::GamebryoHair,
        })
    }

    /// Gamebryo's (drawn by `PSGamebryo`, with Gamebryo's lighting).
    pub fn gamebryo(self) -> bool {
        matches!(self, Class::GamebryoLit | Class::GamebryoSkin | Class::GamebryoHair)
    }

    /// Drawn blended after the opaque classes (the game's forward pass).
    pub fn forward(self) -> bool {
        matches!(self, Class::GlassTint | Class::GlassReflect)
    }

    /// The textures it samples beside the base map: (bump, detail, environment).
    pub fn needs(self) -> (bool, bool, bool) {
        match self {
            Class::Plain => (true, true, false),
            Class::MetalEnv | Class::GlossyGlass => (true, true, true),
            // `use_env` is a permutation of the skin shader (the glove's has it): without a cube, the
            // shader without the environment term.
            Class::ClothSkin => (true, true, false),
            Class::Emissive => (false, true, false),
            Class::GlassTint => (false, false, false),
            Class::GlassReflect => (false, false, true),
            // (The normal and glow maps when the mesh has them.)
            Class::GamebryoLit | Class::GamebryoSkin | Class::GamebryoHair => (false, false, false),
        }
    }
}

/// The fragment as the game's pixel shaders see it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fragment {
    /// The real view-space position (the game's `v2.xyz`).
    pub pv: Vec3,
    /// The interpolated vertex normal (view space, not renormalised: `v3`).
    pub nv: Vec3,
    /// `v1.z`: the detail/parallax strength, `saturate(1 − |pv|²·fade)·surf_params.w`.
    pub k: f32,
    /// `v1.w`: the vertex normal's weight against the bump (`surf_params.z`).
    pub nw: f32,
    /// `v2.w`: vertex AO × the per-object "hemi" occlusion.
    pub hemi_ao: f32,
}

impl Fragment {
    /// The vertex shader's outputs for a vertex at `pv` with unit normal `nv`, for a material
    /// with `surf_params` = `params`.
    pub fn new(pv: Vec3, nv: Vec3, params: Vec4, hemi_ao: f32) -> Fragment {
        Fragment { pv, nv, k: (1.0 - pv.length_squared() * FADE_Z).clamp(0.0, 1.0) * params.w, nw: params.z, hemi_ao }
    }
}

/// The cotangent frame the game builds from screen derivatives (`ddx/ddy` of the view position
/// and the uv), normalised so the longer of the two has unit length. Any pair of screen offsets
/// gives the same frame (up to its sign when they swap handedness).
pub fn cotangent_frame(dpx: Vec3, dpy: Vec3, dux: Vec2, duy: Vec2, nv: Vec3) -> (Vec3, Vec3) {
    let r1 = dpy.cross(nv);
    let r2 = nv.cross(dpx);
    let t = r1 * dux.x + r2 * duy.x;
    let b = r1 * dux.y + r2 * duy.y;
    let s = 1.0 / t.length_squared().max(b.length_squared()).sqrt();
    (t * s, b * s)
}

/// The one-step parallax: the uv everything after it samples at, from the height at the
/// fragment's own uv.
pub fn parallax(uv: Vec2, t: Vec3, b: Vec3, f: &Fragment, height: f32) -> Vec2 {
    let vt = Vec3::new(t.dot(f.pv), b.dot(f.pv), f.nv.dot(f.pv));
    let e = -Vec2::new(vt.x, vt.y) / vt.length();
    uv + e * (height * 0.01 - 0.005) * f.k.clamp(0.0, 1.0)
}

/// The uvs the bump map is sampled at around `uv` (after the parallax): left (the full texel, for
/// the detail selector, specular and height), right, up and down (height only).
pub fn bump_taps(uv: Vec2) -> [Vec2; 4] {
    let d = BUMP_STEP;
    [uv - Vec2::new(d, 0.0), uv + Vec2::new(d, 0.0), uv - Vec2::new(0.0, d), uv + Vec2::new(0.0, d)]
}

/// What the pixel shader read from its textures (all linear: the game's are UNORM).
#[derive(Clone, Copy, Debug, Default)]
pub struct Texels {
    pub base: Vec4,
    /// The bump map at the left tap: r = the detail selector, g = specular, a = height.
    pub bump_l: Vec4,
    /// Heights at the right, up and down taps.
    pub height_r: f32,
    pub height_u: f32,
    pub height_d: f32,
    /// The bump map's green with mip bias +3 at the fragment's uv.
    pub gloss: f32,
    /// The detail map at uv × `surf_params.xy`.
    pub detail: Vec3,
    /// The environment cube in the direction [`reflection`] gave.
    pub env: Vec3,
}

/// What the game's G-buffer would hold for the fragment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GBuffer {
    /// The lighting position: view space with z squashed (RT0.xyz).
    pub p: Vec3,
    /// ±vertex AO·hemi; negative marks the skin class (RT0.w).
    pub w: f32,
    /// Unit normal (RT1.xyz); zero for emissive.
    pub n: Vec3,
    pub gloss: f32,
    /// Albedo (RT2.rgb); for emissive the emitted colour.
    pub albedo: Vec3,
    /// Specular intensity (RT2.a).
    pub spec: f32,
    pub emissive: bool,
}

/// The bumped normal (before any class adjustments), shared by the G-buffer classes.
pub fn bumped_normal(t: Vec3, b: Vec3, f: &Fragment, x: &Texels, class: Class) -> Vec3 {
    let n = (b * (x.height_u - x.height_d) + t * (x.bump_l.w - x.height_r) + f.nv * f.nw).normalize();
    if class == Class::GlossyGlass { Vec3::new(n.x, n.y, n.z * 0.5).normalize() } else { n }
}

/// The view-space reflection the environment cube is sampled with (rotate it to the game's world
/// to sample): `n` is the bumped normal for the G-buffer classes, the vertex normal for glass.
pub fn reflection(pv: Vec3, n: Vec3) -> Vec3 {
    let vn = pv.normalize();
    vn - 2.0 * vn.dot(n) * n
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// The material constants the pixel shaders take beside the fragment's, and the permutation
/// flags that pick among a class's shaders.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    /// `surf_color.rgb`.
    pub color: Vec3,
    /// The emissive classes' fixed multiplier (2; 6 for the game's brighter emissive shader).
    pub emissive: f32,
    /// `use_env`: [`Class::ClothSkin`] lerps towards the environment's grey; without it
    /// the albedo is kept as it is.
    pub env: bool,
    /// `use_fresnel`: [`Class::ClothSkin`]'s grazing sheen, and [`Class::Plain`]'s
    /// sky-tinted grazing term (the game's `deffer_*/use_fresnel=1` shaders; the plain one is without).
    pub fresnel: bool,
}

impl Default for Surface {
    fn default() -> Self {
        Surface { color: Vec3::ONE, emissive: 2.0, env: true, fresnel: true }
    }
}

/// The frame's constants the G-buffer shaders read.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub depth_squash: f32,
    /// `envmap_color` (a frame constant).
    pub envmap_color: Vec3,
    /// The world's up direction in view space (`m_iV[1]`, the game's view-to-world row for y).
    pub up: Vec3,
}

impl Default for Frame {
    fn default() -> Self {
        Frame { depth_squash: 0.04, envmap_color: ENVMAP_COLOR, up: Vec3::Y }
    }
}

/// The frame's environment tint: the game's, or [`ENVMAP_COLOR`] when the adapter didn't fill it.
pub fn envmap_color(l: &GameLighting) -> Vec3 {
    let c = Vec3::new(l.envmap_color[0], l.envmap_color[1], l.envmap_color[2]);
    if c == Vec3::ZERO { ENVMAP_COLOR } else { c }
}

/// The fresnel term of the game's `use_fresnel` opaque shaders: towards grazing angles the albedo
/// goes to its square root tinted by a sky colour that follows the normal's world height.
pub fn fresnel_tint(a: Vec3, n: Vec3, c: f32, up: Vec3) -> Vec3 {
    let h = (up.dot(n) + 0.5).clamp(0.0, 1.0);
    let tint = h * Vec3::new(0.1885, 0.529, 0.888) + Vec3::new(0.8, 0.635, 0.435);
    let f = (c + 0.675).clamp(0.0, 1.0);
    f * (Vec3::new(a.x.sqrt(), a.y.sqrt(), a.z.sqrt()) * tint - a) + a
}

/// The G-buffer the game writes for the fragment: `n` from [`bumped_normal`], `x` the texels
/// (`env` sampled along [`reflection`]`(pv, n)`).
pub fn gbuffer(class: Class, f: &Fragment, n: Vec3, x: &Texels, surface: &Surface, frame: &Frame) -> GBuffer {
    let color = surface.color;
    let (depth_squash, envmap_color) = (frame.depth_squash, frame.envmap_color);
    if class == Class::Emissive {
        let e = x.base.xyz() * (x.detail.dot(Vec3::splat(0.666667)) * surface.emissive);
        let p = f.pv + f.nv.normalize() * 0.025;
        return GBuffer { p: Vec3::new(p.x, p.y, p.z * depth_squash), w: f.hemi_ao, n: Vec3::ZERO, gloss: 0.5, albedo: e, spec: 0.0, emissive: true };
    }
    let p = f.pv + n * x.bump_l.w * 0.05;
    let p = Vec3::new(p.x, p.y, p.z * depth_squash);
    let g = x.gloss;
    let sel = x.bump_l.x * 2.0 - 1.0;
    let dsel = x.detail.dot(Vec3::new(sel.max(0.0), 1.0 - sel.abs(), (-sel).max(0.0)));
    let dm = (dsel - 0.5) * f.k + 0.5;
    let vn = f.pv.normalize();
    let c = vn.dot(n);
    let mut out = GBuffer { p, w: f.hemi_ao, n, gloss: g, albedo: Vec3::ZERO, spec: 0.0, emissive: false };
    match class {
        Class::Plain => {
            let albedo = x.base.xyz() * dm * color * 2.0;
            out.albedo = if surface.fresnel { fresnel_tint(albedo, n, c, frame.up) } else { albedo };
            out.spec = 2.0 * x.bump_l.y * dm;
        }
        Class::MetalEnv => {
            let mut albedo = x.base.xyz() * dm * color * 2.0;
            let spec = 2.0 * x.bump_l.y * dm;
            let a = (c + 1.0).clamp(0.0, 1.0);
            let xx = a * (2.0 * a - 1.0).abs() * g.min(spec);
            let y = 1.0 - ((3.0 * a - 1.0).abs() - a).abs();
            let z = 1.0 - y.abs();
            albedo += xx * albedo * (2.0 * z - 1.0);
            out.albedo = albedo * (Vec3::ONE + g * (x.env - Vec3::splat(0.5)));
            out.spec = ((g + spec) * y.abs() * 0.5).max(spec);
        }
        Class::ClothSkin => {
            let albedo = x.base.xyz() * dm * color * 2.0;
            let spec = 2.0 * x.bump_l.y * dm;
            let ff = (c + 0.675).clamp(0.0, 1.0);
            let e = x.env.dot(Vec3::splat(0.33));
            let a = if surface.env { (albedo + (e * envmap_color - albedo) * (spec * f.k)).clamp(Vec3::ZERO, Vec3::ONE) } else { albedo };
            let s1 = smooth((2.0 * (ff - 0.5)).max(0.0));
            let s2 = smooth((2.0 * ff).min(1.0));
            out.albedo = if surface.fresnel { a + 0.5 * (s2 - s1) * (Vec3::new(a.x.sqrt(), a.y.sqrt(), a.z.sqrt()) - a) } else { a };
            out.spec = spec;
            out.w = -f.hemi_ao;
        }
        Class::GlossyGlass => {
            let base = x.base.xyz();
            let q = 6.0 * x.bump_l.y / (6.0 * x.bump_l.y + 0.5);
            let albedo = (base + q * (base * base - base)) * dm * color * 2.0;
            let spec = 2.0 * q * dm;
            let a = (c + 1.0).clamp(0.0, 1.0);
            let spec2 = a * 0.3 + spec;
            out.albedo = x.env * envmap_color * spec2 * a + albedo;
            out.spec = spec2;
        }
        Class::Emissive | Class::GlassTint | Class::GlassReflect | Class::GamebryoLit | Class::GamebryoSkin | Class::GamebryoHair => unreachable!("not a G-buffer class"),
    }
    out
}

/// The forward glass passes' colour (HDR, before the tonemap) and opacity. `nv` is the vertex
/// normal; `x.env` sampled along [`reflection`]`(pv, normalize(nv))`.
pub fn glass(class: Class, x: &Texels, envmap_color: Vec3) -> Vec4 {
    match class {
        Class::GlassTint => x.base,
        // The previous frame's blur-buffer reflection the game adds isn't available: 0.
        _ => (x.env * envmap_color * (1.0 - x.base.w) + x.base.xyz()).extend(x.base.w),
    }
}

/// Whether a light with [`GLF_REACHES_VIEWMODEL`] lights the fragment: the game's stencil test
/// passes iff the point [`REACH_DEPTH`] in front of the camera along the pixel's ray is inside the
/// light's volume. Only the volume's radius is known, so this is the sphere test at that point.
pub fn reaches(pv: Vec3, l: &GameLight) -> bool {
    if l.flags & GLF_REACHES_VIEWMODEL == 0 {
        return false;
    }
    let q = pv * (REACH_DEPTH / pv.z.max(0.1 * pv.length()).max(1e-6));
    let d = q - Vec3::new(l.pos[0], l.pos[1], l.pos[2]);
    d.length_squared() * l.pos[3] < 1.0
}

/// The light's texture term for a fragment at lighting position `p` (rgb: the light map, a: the
/// shadow), 1 for lights without a tile. `fetch(x, y)` returns the atlas texel at integer
/// coordinates (RGBA, 0..1).
pub fn atlas_term(l: &GameLight, p: Vec3, depth_squash: f32, fetch: &dyn Fn(u32, u32) -> Vec4) -> Vec4 {
    if l.flags & GLF_TILE == 0 {
        return Vec4::ONE;
    }
    let [(a, ka), (b, kb)] = atlas_taps(l.tile, p, depth_squash);
    bilinear(a, fetch) * ka + bilinear(b, fetch) * kb
}

/// The two slices' sample points in the atlas (texel units) and their weights. The layout
/// ([`LIGHT_TILE`]): texel i of a tile's row holds view x = −E + (i + ½)·2E/T (likewise y, row 0
/// at +y), and slice s the squashed depth s/(S − 1)·`depth_squash`·[`LIGHT_TILE_DEPTH`].
pub fn atlas_taps(tile: u32, p: Vec3, depth_squash: f32) -> [(Vec2, f32); 2] {
    let n = LIGHT_TILE as f32;
    let slices = LIGHT_TILE_SLICES as f32;
    let e = LIGHT_TILE_EXTENT;
    let tx = ((p.x + e) / (2.0 * e) * n).clamp(0.5, n - 0.5);
    let ty = ((e - p.y) / (2.0 * e) * n).clamp(0.5, n - 0.5);
    let s = (p.z / (depth_squash * LIGHT_TILE_DEPTH).max(1e-9) * (slices - 1.0)).clamp(0.0, slices - 1.0);
    let s0 = s.floor().min(slices - 2.0);
    let k = s - s0;
    let row = tile as f32 * n + ty;
    [(Vec2::new(s0 * n + tx, row), 1.0 - k), (Vec2::new((s0 + 1.0) * n + tx, row), k)]
}

fn bilinear(at: Vec2, fetch: &dyn Fn(u32, u32) -> Vec4) -> Vec4 {
    let t = at - 0.5;
    let i = t.floor();
    let f = t - i;
    let get = |dx: f32, dy: f32| {
        let x = (i.x + dx).clamp(0.0, LIGHT_ATLAS_W as f32 - 1.0) as u32;
        let y = (i.y + dy).clamp(0.0, LIGHT_ATLAS_H as f32 - 1.0) as u32;
        fetch(x, y)
    };
    let top = get(0.0, 0.0).lerp(get(1.0, 0.0), f.x);
    let bottom = get(0.0, 1.0).lerp(get(1.0, 1.0), f.x);
    top.lerp(bottom, f.y)
}

/// The cubic the game uses for diffuse light on everything but skin.
pub fn diffuse_curve(x: f32) -> f32 {
    1.214295 * x * x * x - 2.153095 * x * x + 1.958771 * x + 0.016965
}

/// One light's diffuse and specular contributions (rgb), `term` its [`atlas_term`].
pub fn light(g: &GBuffer, l: &GameLight, term: Vec4) -> (Vec3, Vec3) {
    let lpos = Vec3::new(l.pos[0], l.pos[1], l.pos[2]);
    let lc = Vec3::new(l.color[0], l.color[1], l.color[2]);
    let lv = g.p - lpos;
    let mut att = (1.0 - lv.length_squared() * l.pos[3]).clamp(0.0, 1.0);
    if l.kind == GL_AMBIENT {
        return ((g.w.abs() + 0.05) * att * lc, Vec3::ZERO);
    }
    match l.kind {
        GL_PROJECTED_OCCLUDED => att *= g.w.abs(),
        GL_SHADOWED | GL_SHADOWED_PLAIN => att *= term.w,
        _ => {}
    }
    let ld = lv.normalize();
    let v = -g.p.normalize();
    let h = (v - ld).normalize();
    let ndotl = (-ld.dot(g.n)).clamp(0.0, 1.0);
    let ndoth = h.dot(g.n).clamp(0.0, 1.0);
    let n = g.gloss * g.gloss * 128.0 + l.params[0];
    let spec = (n + 8.0) * ndoth.powf(n) * 0.039789;
    let diff = if g.w < 0.0 { ndotl } else { diffuse_curve(ndotl) };
    let mut d = diff * att * lc;
    let mut ss = spec * att * l.color[3];
    if matches!(l.kind, GL_PROJECTED | GL_PROJECTED_OCCLUDED | GL_SHADOWED) {
        d *= term.xyz();
        ss *= term.xyz().dot(LUM);
    }
    (d, ss * lc)
}

/// The sum of every light reaching the fragment: (diffuse, specular).
pub fn lights(g: &GBuffer, pv: Vec3, lighting: &GameLighting, fetch: &dyn Fn(u32, u32) -> Vec4) -> (Vec3, Vec3) {
    let mut d = Vec3::ZERO;
    let mut s = Vec3::ZERO;
    for l in &lighting.lights[..(lighting.count as usize).min(lighting.lights.len())] {
        if reaches(pv, l) {
            let (ld, ls) = light(g, l, atlas_term(l, g.p, lighting.depth_squash, fetch));
            d += ld;
            s += ls;
        }
    }
    (d, s)
}

/// The spherical harmonics' irradiance for view-space normal `n` (the game's L0 + L1).
pub fn sh_irradiance(sh: &[[f32; 4]; 3], n: Vec3) -> Vec3 {
    let e = |c: &[f32; 4]| 0.886227 * c[0] + 1.023328 * (n.x * c[3] + n.y * c[1] + n.z * c[2]);
    Vec3::new(e(&sh[0]), e(&sh[1]), e(&sh[2]))
}

/// The combine pass: the lit HDR colour from the G-buffer, the lights' sums `d` and `s`, the
/// screen-space occlusion `ao`, the skin's blurred red light `ao_r` (the SSAO pass's z) and the
/// screen-space reflection `refl`.
pub fn combine(g: &GBuffer, d: Vec3, s: Vec3, sh: &[[f32; 4]; 3], ambient: Vec4, ao: f32, ao_r: f32, refl: Vec3) -> Vec3 {
    if g.emissive {
        return g.albedo;
    }
    let skin = g.w < 0.0;
    let aot = g.w.abs() * ao;
    let mut d = d;
    if skin {
        let dm = Vec3::new(d.x.max(2.0 * ao_r * ao_r), d.y, d.z);
        let l = dm.dot(LUM);
        let ll = l * ao + 0.01;
        d = Vec3::new(ll, 0.01 * ll, 0.01 * ll).lerp(dm, (6.0 * l * ao + 0.1).clamp(0.0, 1.0));
    }
    let e = sh_irradiance(sh, g.n);
    let amb = Vec3::new(ambient.x, ambient.y, ambient.z) * (0.25 + 0.75 * aot);
    let mut spec = s + refl * ambient.w;
    if skin {
        spec = Vec3::splat(spec.dot(LUM));
    }
    ((e * aot + d + amb) * g.albedo + spec * g.spec) * ao
}

/// The `ao_r` the combine gets when the light isn't blurred: the fragment's own red diffuse light.
pub fn local_ao_r(d: Vec3) -> f32 {
    (0.5 * d.x.max(0.0)).sqrt()
}

/// The game's tone curve: Hable's (Uncharted 2) with the standard constants, normalised so 1 maps
/// to 1. Bloom is 0 for the runtime's own model.
pub fn tonemap(x: f32) -> f32 {
    let x = x.max(0.0);
    4.531915 * ((x * (0.15 * x + 0.05) + 0.004) / (x * (0.15 * x + 0.5) + 0.06) - 0.066667)
}

/// Samples the grading table (`LUT_SIZE`³ RGBA8, r fastest) at `c` with clamped trilinear
/// filtering and no half-texel remap, as the game samples it.
pub fn lut_sample(lut: &[u32], c: Vec3) -> Vec4 {
    let n = LUT_SIZE as f32;
    let t = c * n - 0.5;
    let i = t.floor();
    let f = t - i;
    let texel = |x: f32, y: f32, z: f32| {
        let idx = |v: f32| v.clamp(0.0, n - 1.0) as usize;
        let v = lut[(idx(z) * LUT_SIZE + idx(y)) * LUT_SIZE + idx(x)];
        Vec4::new((v & 0xFF) as f32, ((v >> 8) & 0xFF) as f32, ((v >> 16) & 0xFF) as f32, (v >> 24) as f32) / 255.0
    };
    let plane = |z: f32| {
        let a = texel(i.x, i.y, z).lerp(texel(i.x + 1.0, i.y, z), f.x);
        let b = texel(i.x, i.y + 1.0, z).lerp(texel(i.x + 1.0, i.y + 1.0, z), f.x);
        a.lerp(b, f.y)
    };
    plane(i.z).lerp(plane(i.z + 1.0), f.z)
}

/// The final pass on a tonemapped colour: the grading table (`None`: none this frame) and the
/// back buffer's clamp. The result is the code value the game would display (0..1).
pub fn grade(tm: Vec3, lut: Option<&[u32]>) -> Vec3 {
    let s = tm.clamp(Vec3::ZERO, Vec3::ONE);
    let out = match lut {
        Some(lut) => {
            let l = lut_sample(lut, Vec3::new(tm.z, tm.y, tm.x));
            s + 2.0 * Vec3::new(l.z, l.y, l.x) - Vec3::ONE
        }
        None => s,
    };
    out.clamp(Vec3::ZERO, Vec3::ONE)
}

/// The HDR colour to the code value the game would display.
pub fn post(hdr: Vec3, lut: Option<&[u32]>) -> Vec3 {
    grade(Vec3::new(tonemap(hdr.x), tonemap(hdr.y), tonemap(hdr.z)), lut)
}

/// What to write to an sRGB render target so that it stores `code` (the game's back buffer has no
/// sRGB encoding: its numbers are the displayed code values).
pub fn display_to_linear(code: f32) -> f32 {
    let c = code.clamp(0.0, 1.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// The whole chain for one opaque fragment: its displayed code value.
/// `up`: the world's up direction in the game camera's view space.
pub fn shade(class: Class, f: &Fragment, n: Vec3, x: &Texels, surface: &Surface, up: Vec3, lighting: &GameLighting, fetch: &dyn Fn(u32, u32) -> Vec4) -> Vec3 {
    let frame = Frame { depth_squash: lighting.depth_squash, envmap_color: envmap_color(lighting), up };
    let g = gbuffer(class, f, n, x, surface, &frame);
    let (d, s) = if g.emissive { (Vec3::ZERO, Vec3::ZERO) } else { lights(&g, f.pv, lighting, fetch) };
    let hdr = combine(&g, d, s, &lighting.sh, Vec4::from_array(lighting.ambient), lighting.ssao, local_ao_r(d), Vec3::ZERO);
    post(hdr, (lighting.lut_valid != 0).then_some(&lighting.lut[..]))
}

/// The same model in HLSL, function for function (the avatar's shader includes it). Pure
/// functions only: the shader samples the textures and hands the texels in, as the CPU side does.
pub const HLSL: &str = r#"
#define M_PLAIN 1
#define M_METAL_ENV 2
#define M_CLOTH_SKIN 3
#define M_GLOSSY_GLASS 4
#define M_EMISSIVE 5
#define M_GLASS_TINT 6
#define M_GLASS_REFLECT 7
#define M_GL_OMNI 0
#define M_GL_PROJECTED 1
#define M_GL_PROJECTED_OCCLUDED 2
#define M_GL_SHADOWED 3
#define M_GL_SHADOWED_PLAIN 4
#define M_GL_AMBIENT 5
#define M_GLF_REACHES 1
#define M_GLF_TILE 2
static const float3 M_LUM = float3(0.3, 0.48, 0.22);
static const float M_FADE_Z = 0.000178;
static const float M_BUMP_STEP = 1.0 / 2048.0;
static const float M_REACH_DEPTH = 0.1;
static const float M_TILE = 64.0;
static const float M_SLICES = 4.0;
static const float M_TILE_EXTENT = 1.2;
static const float M_TILE_DEPTH = 1.0;

struct MFrag { float3 pv; float3 nv; float k; float nw; float hemi_ao; };
struct MTex { float4 base; float4 bump_l; float height_r; float height_u; float height_d; float gloss; float3 detail; float3 env; };
struct MG { float3 p; float w; float3 n; float gloss; float3 albedo; float spec; bool emissive; };
struct MLight { float4 pos; float4 color; float4 params; uint4 info; };   // info: kind, flags, tile

MFrag m_frag(float3 pv, float3 nv, float4 params, float hemi_ao) {
    MFrag f;
    f.pv = pv; f.nv = nv;
    f.k = saturate(1 - dot(pv, pv) * M_FADE_Z) * params.w;
    f.nw = params.z;
    f.hemi_ao = hemi_ao;
    return f;
}

void m_frame(float3 dpx, float3 dpy, float2 dux, float2 duy, float3 nv, out float3 t, out float3 b) {
    float3 r1 = cross(dpy, nv), r2 = cross(nv, dpx);
    t = r1 * dux.x + r2 * duy.x;
    b = r1 * dux.y + r2 * duy.y;
    float s = rsqrt(max(dot(t, t), dot(b, b)));
    t *= s; b *= s;
}

float2 m_parallax(float2 uv, float3 t, float3 b, MFrag f, float h) {
    float3 vt = float3(dot(t, f.pv), dot(b, f.pv), dot(f.nv, f.pv));
    float2 e = -vt.xy * rsqrt(dot(vt, vt));
    return uv + e * (h * 0.01 - 0.005) * saturate(f.k);
}

float3 m_bumped_normal(float3 t, float3 b, MFrag f, MTex x, uint cls) {
    float3 n = normalize(b * (x.height_u - x.height_d) + t * (x.bump_l.w - x.height_r) + f.nv * f.nw);
    if (cls == M_GLOSSY_GLASS) n = normalize(float3(n.xy, n.z * 0.5));
    return n;
}

float3 m_reflection(float3 pv, float3 n) {
    float3 vn = normalize(pv);
    return vn - 2 * dot(vn, n) * n;
}

float m_smooth(float t) { return t * t * (3 - 2 * t); }

float3 m_fresnel_tint(float3 a, float3 n, float c, float3 up) {
    float h = saturate(dot(up, n) + 0.5);
    float3 tint = h * float3(0.1885, 0.529, 0.888) + float3(0.8, 0.635, 0.435);
    float f = saturate(c + 0.675);
    return f * (sqrt(a) * tint - a) + a;
}

// opts: x use_env (skin), y use_fresnel.
MG m_gbuffer(uint cls, MFrag f, float3 n, MTex x, float3 color, float emissive, float2 opts, float squash, float3 envc, float3 up) {
    MG o;
    o.emissive = false; o.w = f.hemi_ao; o.n = n; o.gloss = x.gloss; o.spec = 0; o.albedo = 0;
    if (cls == M_EMISSIVE) {
        float3 p = f.pv + normalize(f.nv) * 0.025;
        o.p = float3(p.xy, p.z * squash);
        o.n = 0; o.gloss = 0.5; o.emissive = true;
        o.albedo = x.base.rgb * (dot(x.detail, 0.666667) * emissive);
        return o;
    }
    float3 p = f.pv + n * x.bump_l.w * 0.05;
    o.p = float3(p.xy, p.z * squash);
    float g = x.gloss;
    float sel = x.bump_l.x * 2 - 1;
    float dsel = dot(x.detail, float3(max(sel, 0), 1 - abs(sel), max(-sel, 0)));
    float dm = (dsel - 0.5) * f.k + 0.5;
    float c = dot(normalize(f.pv), n);
    if (cls == M_GLOSSY_GLASS) {
        float3 base = x.base.rgb;
        float q = 6 * x.bump_l.y / (6 * x.bump_l.y + 0.5);
        float3 albedo = (base + q * (base * base - base)) * dm * color * 2;
        float spec = 2 * q * dm;
        float a = saturate(c + 1);
        float spec2 = a * 0.3 + spec;
        o.albedo = x.env * envc * spec2 * a + albedo;
        o.spec = spec2;
        return o;
    }
    float3 albedo = x.base.rgb * dm * color * 2;
    float spec = 2 * x.bump_l.y * dm;
    if (cls == M_METAL_ENV) {
        float a = saturate(c + 1);
        float xx = a * abs(2 * a - 1) * min(g, spec);
        float y = 1 - abs(abs(3 * a - 1) - a);
        float z = 1 - abs(y);
        albedo += xx * albedo * (2 * z - 1);
        o.albedo = albedo * (1 + g * (x.env - 0.5));
        o.spec = max((g + spec) * abs(y) * 0.5, spec);
    } else if (cls == M_CLOTH_SKIN) {
        float ff = saturate(c + 0.675);
        float e = dot(x.env, 0.33);
        float3 a = opts.x != 0 ? saturate(albedo + (e * envc - albedo) * (spec * f.k)) : albedo;
        float s1 = m_smooth(max(2 * (ff - 0.5), 0));
        float s2 = m_smooth(min(2 * ff, 1));
        o.albedo = opts.y != 0 ? a + 0.5 * (s2 - s1) * (sqrt(a) - a) : a;
        o.spec = spec;
        o.w = -f.hemi_ao;
    } else {
        o.albedo = opts.y != 0 ? m_fresnel_tint(albedo, n, c, up) : albedo;
        o.spec = spec;
    }
    return o;
}

float4 m_glass(uint cls, MTex x, float3 envc) {
    if (cls == M_GLASS_TINT) return x.base;
    return float4(x.env * envc * (1 - x.base.a) + x.base.rgb, x.base.a);
}

bool m_reaches(float3 pv, MLight l) {
    if ((l.info.y & M_GLF_REACHES) == 0) return false;
    float3 q = pv * (M_REACH_DEPTH / max(max(pv.z, 0.1 * length(pv)), 1e-6));
    float3 d = q - l.pos.xyz;
    return dot(d, d) * l.pos.w < 1;
}

// The two slices' sample points (atlas texels) and the second one's weight.
void m_atlas_taps(uint tile, float3 p, float squash, out float2 a, out float2 b, out float k) {
    float tx = clamp((p.x + M_TILE_EXTENT) / (2 * M_TILE_EXTENT) * M_TILE, 0.5, M_TILE - 0.5);
    float ty = clamp((M_TILE_EXTENT - p.y) / (2 * M_TILE_EXTENT) * M_TILE, 0.5, M_TILE - 0.5);
    float s = clamp(p.z / max(squash * M_TILE_DEPTH, 1e-9) * (M_SLICES - 1), 0, M_SLICES - 1);
    float s0 = min(floor(s), M_SLICES - 2);
    k = s - s0;
    float row = tile * M_TILE + ty;
    a = float2(s0 * M_TILE + tx, row);
    b = float2((s0 + 1) * M_TILE + tx, row);
}

float m_diffuse_curve(float x) { return 1.214295 * x * x * x - 2.153095 * x * x + 1.958771 * x + 0.016965; }

void m_light(MG g, MLight l, float4 term, out float3 d, out float3 s) {
    float3 lv = g.p - l.pos.xyz;
    float att = saturate(1 - dot(lv, lv) * l.pos.w);
    uint kind = l.info.x;
    s = 0;
    if (kind == M_GL_AMBIENT) { d = (abs(g.w) + 0.05) * att * l.color.rgb; return; }
    if (kind == M_GL_PROJECTED_OCCLUDED) att *= abs(g.w);
    if (kind == M_GL_SHADOWED || kind == M_GL_SHADOWED_PLAIN) att *= term.a;
    float3 ld = normalize(lv);
    float3 v = -normalize(g.p);
    float3 h = normalize(v - ld);
    float ndotl = saturate(-dot(ld, g.n));
    float ndoth = saturate(dot(h, g.n));
    float n = g.gloss * g.gloss * 128 + l.params.x;
    float spec = (n + 8) * pow(ndoth, n) * 0.039789;
    float diff = g.w < 0 ? ndotl : m_diffuse_curve(ndotl);
    d = diff * att * l.color.rgb;
    float ss = spec * att * l.color.w;
    if (kind == M_GL_PROJECTED || kind == M_GL_PROJECTED_OCCLUDED || kind == M_GL_SHADOWED) {
        d *= term.rgb;
        ss *= dot(term.rgb, M_LUM);
    }
    s = ss * l.color.rgb;
}

float3 m_sh(float4 r, float4 gg, float4 b, float3 n) {
    float3 c0 = float3(r.x, gg.x, b.x);
    return 0.886227 * c0 + 1.023328 * (n.x * float3(r.w, gg.w, b.w) + n.y * float3(r.y, gg.y, b.y) + n.z * float3(r.z, gg.z, b.z));
}

float3 m_combine(MG g, float3 d, float3 s, float4 sh_r, float4 sh_g, float4 sh_b, float4 ambient, float ao, float ao_r, float3 refl) {
    if (g.emissive) return g.albedo;
    bool skin = g.w < 0;
    float aot = abs(g.w) * ao;
    if (skin) {
        float3 dm = float3(max(d.r, 2 * ao_r * ao_r), d.g, d.b);
        float l = dot(dm, M_LUM);
        float ll = l * ao + 0.01;
        d = lerp(float3(ll, 0.01 * ll, 0.01 * ll), dm, saturate(6 * l * ao + 0.1));
    }
    float3 e = m_sh(sh_r, sh_g, sh_b, g.n);
    float3 amb = ambient.rgb * (0.25 + 0.75 * aot);
    float3 spec = s + refl * ambient.w;
    if (skin) spec = dot(spec, M_LUM);
    return ((e * aot + d + amb) * g.albedo + spec * g.spec) * ao;
}

float m_local_ao_r(float3 d) { return sqrt(0.5 * max(d.r, 0)); }

float3 m_tonemap(float3 x) {
    x = max(x, 0);
    return 4.531915 * ((x * (0.15 * x + 0.05) + 0.004) / (x * (0.15 * x + 0.5) + 0.06) - 0.066667);
}

// The graded code value from the tonemapped colour and the table's sample at tm.bgr.
float3 m_grade(float3 tm, float4 l, bool lut) {
    float3 o = saturate(tm);
    if (lut) o += 2 * l.bgr - 1;
    return saturate(o);
}

float3 m_display_to_linear(float3 c) {
    c = saturate(c);
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}
"#;

#[cfg(test)]
#[allow(clippy::excessive_precision)] // the measured numbers, as they were printed
mod tests {
    //! Against the game's own numbers: its shaders' values measured on the game at the gun
    //! and glove pixels (docs section 2.3, 3.4, 4.2, 5.5).
    use super::*;
    use rustvr_protocol::{GL_OMNI, LUT_SIZE};

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    fn assert_vec(what: &str, got: &[f32], want: &[f32], tol: f32) {
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!(close(*g, *w, tol), "{what}[{i}]: {got:?}, the game has {want:?}");
        }
    }

    /// The game's light buffer encoding at a pixel of parity `even`: (Y, Co or Cg).
    fn ycocg(rgb: Vec3, even: bool) -> [f32; 2] {
        let y = 0.25 * rgb.x + 0.5 * rgb.y + 0.25 * rgb.z;
        let c = if even { 0.5 * rgb.x - 0.5 * rgb.z } else { -0.25 * rgb.x + 0.5 * rgb.y - 0.25 * rgb.z };
        [y, c]
    }

    fn game_light(kind: u32, pos: [f32; 4], color: [f32; 4], exponent: f32) -> GameLight {
        GameLight { pos, color, params: [exponent, 0.0, 0.0, 0.0], kind, flags: GLF_REACHES_VIEWMODEL, tile: 0, _pad: 0 }
    }

    /// The gun's top (metal + env), measured: the derivatives, the interpolants and
    /// every texture read, through to the G-buffer it wrote.
    #[test]
    fn gun_top_gbuffer_matches_the_trace() {
        let pv = Vec3::new(0.09407908, -0.06716754, 0.4606582);
        let nv = Vec3::new(0.03007909, 0.5975537, -0.7978349);
        let f = Fragment { pv, nv, k: 1.499939, nw: 0.2026667, hemi_ao: 0.3608987 };
        // ddy(Pv).zxy, ddx(Pv).yzx, ddy(uv), ddx(uv) as traced.
        let dpy = Vec3::new(-2.089143e-05, -9.679794e-05, -0.000102222);
        let dpx = Vec3::new(0.0001165047, -3.427267e-06, 2.354383e-05);
        let duy = Vec2::new(0.0004532933, -7.390976e-05);
        let dux = Vec2::new(1.603365e-05, 0.0004122853);
        let (t, b) = cotangent_frame(dpx, dpy, dux, duy, nv);
        assert_vec("T", &t.to_array(), &[0.1308504, -0.7608977, -0.5649557], 2e-4);
        assert_vec("B", &b.to_array(), &[0.999535, -0.0216543, 0.02146496], 2e-4);
        let uv = parallax(Vec2::new(0.9896335, 0.678193), t, b, &f, 0.4997482);
        assert_vec("parallax uv", &uv.to_array(), &[0.9896324, 0.6781936], 2e-6);
        let x = Texels {
            base: Vec4::new(0.2162509, 0.2145266, 0.2162509, 1.0),
            bump_l: Vec4::new(1.0, 0.2685588, 0.0, 0.498497),
            height_r: 0.4993057,
            height_u: 0.4980392,
            height_d: 0.5002365,
            gloss: 0.2079042,
            detail: Vec3::new(0.5675746, 0.4762036, 0.5532311),
            env: Vec3::new(0.8721904, 0.8946822, 0.9290456),
        };
        let n = bumped_normal(t, b, &f, &x, Class::MetalEnv);
        let g = gbuffer(Class::MetalEnv, &f, n, &x, &Surface::default(), &Frame::default());
        // o0, o1, o2 of the trace (o2 is the shader's output; the stored value went through the
        // 8-bit sRGB target).
        assert_vec("RT0", &[g.p.x, g.p.y, g.p.z, g.w], &[0.09454692, -0.05215192, 0.01763078, 0.3608987], 2e-5);
        assert_vec("RT1", &[g.n.x, g.n.y, g.n.z, g.gloss], &[0.01877024, 0.6024356, -0.7979467, 0.2079042], 2e-4);
        assert_vec("RT2", &[g.albedo.x, g.albedo.y, g.albedo.z, g.spec], &[0.2792766, 0.2782523, 0.2823407, 0.3229998], 2e-4);
        assert!(!g.emissive);
    }

    /// The glove (cloth/skin): the class flags the pixel as skin, and its
    /// G-buffer, from the traced texture reads.
    #[test]
    fn glove_gbuffer_matches_the_trace() {
        let t = trace_inputs("eid95_glove_mid.txt");
        let Some(t) = t else { return };
        let g = gbuffer(Class::ClothSkin, &t.0, t.1, &t.2, &Surface::default(), &Frame::default());
        assert_vec("RT0", &[g.p.x, g.p.y, g.p.z, g.w], &t.3[0], 2e-4);
        assert_vec("RT1", &[g.n.x, g.n.y, g.n.z, g.gloss], &t.3[1], 2e-4);
        assert_vec("RT2", &[g.albedo.x, g.albedo.y, g.albedo.z, g.spec], &t.3[2], 2e-4);
    }

    /// The glove's measured inputs, read from their folder when it's there (it's git-ignored):
    /// the fragment, the bumped normal, the texels and the traced outputs.
    fn trace_inputs(name: &str) -> Option<(Fragment, Vec3, Texels, [[f32; 4]; 3])> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local_assets/metro/traces").join(name);
        let text = std::fs::read_to_string(&path).ok().or_else(|| {
            eprintln!("no {}: skipped", path.display());
            None
        })?;
        let line_regs = |line: usize| -> Vec<(String, Vec<f32>)> {
            let prefix = format!("{line}: ");
            let l = text.lines().find(|l| l.starts_with(&prefix)).unwrap_or_else(|| panic!("line {line} of {name}"));
            l[prefix.len()..]
                .split(") ")
                .filter_map(|r| {
                    let (k, v) = r.split_once("=(")?;
                    Some((k.trim().to_string(), v.trim_end_matches(')').split(',').filter_map(|x| x.trim().parse().ok()).collect()))
                })
                .collect()
        };
        let reg = |line: usize, r: &str| -> Vec<f32> { line_regs(line).into_iter().find(|(k, _)| k == r).unwrap_or_else(|| panic!("{r} at line {line} of {name}")).1 };
        let input = |r: &str| -> Vec<f32> {
            let l = text.lines().find(|l| l.starts_with(&format!("INPUT {r}="))).unwrap();
            l.split_once("=(").unwrap().1.trim_end_matches(')').split(',').map(|x| x.trim().parse().unwrap()).collect()
        };
        let v1 = input("v1");
        let v2 = input("v2");
        let v3 = input("v3");
        let f = Fragment { pv: Vec3::new(v2[0], v2[1], v2[2]), nv: Vec3::new(v3[0], v3[1], v3[2]), k: v1[2], nw: v1[3], hemi_ao: v2[3] };
        // The glove shader's texture reads, as measured (read n on line n + 1).
        let bl = reg(32, "r3");
        let base = reg(51, "r1");
        let detail = reg(53, "r2");
        let env = reg(75, "r0");
        let x = Texels {
            base: Vec4::new(base[0], base[1], base[2], 1.0),
            bump_l: Vec4::new(bl[0], bl[1], 0.0, bl[2]),
            height_r: reg(35, "r2")[2],
            height_u: reg(31, "r0")[3],
            height_d: reg(34, "r1")[3],
            gloss: reg(49, "r0")[3],
            detail: Vec3::new(detail[0], detail[1], detail[2]),
            env: Vec3::new(env[0], env[1], env[2]),
        };
        let o = |o: &str| -> [f32; 4] {
            let mut last = None;
            for l in text.lines() {
                if let Some((_, rest)) = l.split_once(": ")
                    && let Some(v) = rest.split(") ").find_map(|r| r.trim().strip_prefix(&format!("{o}=(")))
                {
                    let v: Vec<f32> = v.trim_end_matches(')').split(',').map(|x| x.trim().parse().unwrap()).collect();
                    last = Some([v[0], v[1], v[2], v[3]]);
                }
            }
            last.unwrap()
        };
        let o1 = o("o1");
        Some((f, Vec3::new(o1[0], o1[1], o1[2]), x, [o("o0"), o1, o("o2")]))
    }

    /// The light shaders at the measured pixels (docs 3.3): the game's light
    /// buffer holds diffuse (Y, chroma) in .xy and specular in .zw at the pixel's parity.
    #[test]
    fn lights_match_the_traces() {
        let gun = GBuffer {
            p: Vec3::new(0.09454346, -0.05212402, 0.0176239),
            w: 0.3608398,
            n: Vec3::new(0.01876831, 0.6020508, -0.7978516),
            gloss: 0.2078857,
            ..Default::default()
        };
        let glove = GBuffer {
            p: Vec3::new(0.0539856, -0.0947876, 0.02427673),
            w: -0.4145508,
            n: Vec3::new(0.3334961, 0.918457, -0.2120361),
            gloss: 0.199585,
            ..Default::default()
        };
        let even = |x: u32, y: u32| (x & 1) == (y & 1);
        let cases = [
            // A projected, occluded light, with its light-map texel.
            (&gun, even(2118, 1869), game_light(GL_PROJECTED_OCCLUDED, [-0.4846802, -1.951338, 1.008202, 0.04340278], [0.2333333, 0.05460973, 0.0, 2.0], 1.0), Vec4::new(0.8715495, 0.8658732, 0.8715495, 1.0), [0.000358516, -0.000130914, 0.0, 0.0]),
            // The camera-fixed omni on the glove (skin: Lambert).
            (&glove, even(1595, 2031), game_light(GL_OMNI, [0.1251373, 0.1115303, 0.2874527, 3.014081], [0.1, 0.3, 0.4, 2.0], 1.0), Vec4::ONE, [0.08200923, -0.0447323, 0.02535563, -0.01383034]),
            // The "ambient" omni.
            (&gun, even(2118, 1869), game_light(GL_AMBIENT, [-0.7256012, 1.918453, 1.044464, 0.06781684], [0.2411765, 0.08680503, 0.0, 2.0], 32.0), Vec4::ONE, [0.02639379, -0.004299401, 0.0, 0.0]),
            // The shadowed spot, all 12 shadow taps lit.
            (&gun, even(2118, 1869), game_light(GL_SHADOWED, [-0.7256012, 1.918453, 1.044464, 0.007535205], [0.9647059, 0.3472201, 0.0, 2.0], 50.0), Vec4::new(0.5899596, 0.5890593, 0.5899596, 1.0), [0.0619105, -0.01013089, 0.0, 0.0]),
        ];
        for (i, (g, even, l, term, want)) in cases.into_iter().enumerate() {
            let (d, s) = light(g, &l, term);
            let got = [ycocg(d, even), ycocg(s, even)].concat();
            assert_vec(&format!("light {i}"), &got, &want, 2e-6);
        }
    }

    /// The combine at the three measured pixels (docs 4.2): the light buffer
    /// decoded at the pixel, the SSAO taps' result, and the HDR colour the game wrote.
    #[test]
    fn combine_matches_the_traces() {
        let sh = [
            [0.08189845, 0.03958863, -0.0004416184, 0.02013502],
            [0.09168029, 0.06594336, 0.0004253049, 0.02170763],
            [0.08516866, 0.08406597, 0.002326976, 0.02088051],
        ];
        let ambient = Vec4::new(0.05882353, 0.05882353, 0.05882353, 1.0);
        #[rustfmt::skip]
        let cases = [
            // glove_mid (1595, 2031)
            ([0.0539856, -0.0947876, 0.02427673, -0.4145508], [0.3334961, 0.918457, -0.2120361], [0.4179688, 0.3867188, 0.3417969, 0.2901961],
             0.9729004, 0.2163696, [0.0988731045, 0.1137733155, 0.1196250645], [0.01005578475, 0.02680945525, 0.03770470475], [0.08133407, 0.07392557, 0.06899935]),
            // gun_top (2118, 1869)
            ([0.09454346, -0.05212402, 0.0176239, 0.3608398], [0.01876831, 0.6020508, -0.7978516], [0.2539062, 0.25, 0.25, 0.3215686],
             1.0, 0.3168945, [0.2067642225, 0.07759857, 0.0054702775], [0.0, 0.0, 0.0], [0.06922758, 0.03805677, 0.02036983]),
            // glove_lowleft (1121, 2430)
            ([-0.02435303, -0.1117554, 0.01954651, -0.4294434], [-0.1967773, 0.9492188, -0.2437744], [0.4335938, 0.4121094, 0.3671875, 0.05490196],
             0.9729004, 0.4571533, [0.412719695, 0.20126343, 0.086120645], [0.08276113666666667, 0.090934758, 0.07936350733333332], [0.2137656, 0.1221624, 0.0699334]),
        ];
        for (i, (p, n, a, ao, ao_r, d, s, want)) in cases.into_iter().enumerate() {
            let g = GBuffer { p: Vec3::new(p[0], p[1], p[2]), w: p[3], n: Vec3::from_array(n), gloss: 0.0, albedo: Vec3::new(a[0], a[1], a[2]), spec: a[3], emissive: false };
            let lit = combine(&g, Vec3::from_array(d), Vec3::from_array(s), &sh, ambient, ao, ao_r, Vec3::ZERO);
            assert_vec(&format!("combine {i}"), &lit.to_array(), &want, 2e-6);
        }
    }

    /// The final pass at the gun's top (docs 5.5): c = 0.0712 → tm = 0.0892 → 0.0853 with the
    /// frame's uniform grading table (127 everywhere).
    #[test]
    fn tonemap_and_grading_match_the_trace() {
        assert!(close(tonemap(1.0), 1.0, 1e-5), "normalised: {}", tonemap(1.0));
        assert!(close(tonemap(0.0712), 0.0892, 1e-4), "{}", tonemap(0.0712));
        let lut = vec![0x7F7F_7F7Fu32; LUT_SIZE * LUT_SIZE * LUT_SIZE];
        let out = grade(Vec3::splat(tonemap(0.0712)), Some(&lut));
        assert!(close(out.x, 0.0853, 1e-4), "{out}");
        // The final pass, measured on the gun's top: the blurred colour plus bloom going into the
        // curve, the curve with 0.125·bloom added, and the graded output.
        let x = Vec3::new(0.07122708, 0.03860796, 0.02006738);
        let bloom = Vec3::new(1.540191e-05, 7.38636e-06, 3.557201e-06);
        let tm = Vec3::new(tonemap(x.x), tonemap(x.y), tonemap(x.z)) + bloom;
        assert_vec("tm", &tm.to_array(), &[0.08923776, 0.04856736, 0.02527284], 2e-6);
        assert_vec("out", &grade(tm, Some(&lut)).to_array(), &[0.08531618, 0.04464579, 0.02135122], 2e-6);
    }

    /// The table is sampled at (b, g, r) and applied as `.bgr`: a table that is neutral but for
    /// its red-axis texels shifts only red.
    #[test]
    fn grading_table_axes() {
        let n = LUT_SIZE;
        let mut lut = vec![0u32; n * n * n];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    // Neutral (128 → +1/255) everywhere, with the table's own red (low byte)
                    // following its x axis.
                    let r = 112 + 2 * x as u32;
                    lut[(z * n + y) * n + x] = 0xFF80_8000 | r;
                }
            }
        }
        // x axis = tm.b; red byte = L.r, applied to out.b.
        let tm = Vec3::new(0.2, 0.4, 0.8);
        let out = grade(tm, Some(&lut));
        let l = lut_sample(&lut, Vec3::new(tm.z, tm.y, tm.x));
        assert!(close(out.z, tm.z + 2.0 * l.x - 1.0, 1e-6));
        assert!(close(out.x, tm.x + 2.0 * 128.0 / 255.0 - 1.0, 1e-6));
    }

    #[test]
    fn display_values_round_trip_through_srgb() {
        for code in [0.0f32, 0.02, 0.0853, 0.5, 1.0] {
            let lin = display_to_linear(code);
            let back = if lin <= 0.0031308 { lin * 12.92 } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 };
            assert!(close(back, code, 1e-5), "{code} -> {lin} -> {back}");
        }
    }

    /// The camera-fixed light (radius 0.576 m at (0.125, 0.112, 0.287)) reaches everything in front
    /// of the camera; a light 3 m to the side doesn't; mode-B lights never do.
    #[test]
    fn which_lights_reach() {
        let fixed = game_light(GL_OMNI, [0.1251373, 0.1115303, 0.2874527, 3.014081], [0.1, 0.3, 0.4, 2.0], 1.0);
        let far = game_light(GL_OMNI, [3.0, 0.0, 0.5, 1.0], [1.0; 4], 1.0);
        let mut mode_b = fixed;
        mode_b.flags = 0;
        for pv in [Vec3::new(0.09, -0.07, 0.46), Vec3::new(-0.2, -0.3, 0.5), Vec3::new(0.1, -0.4, 0.25)] {
            assert!(reaches(pv, &fixed), "{pv}");
            assert!(!reaches(pv, &far));
            assert!(!reaches(pv, &mode_b));
        }
    }

    #[test]
    fn atlas_taps_cover_the_tile() {
        // The view axis at the middle of the depth range: the tile's centre, between slices 1 and 2.
        let [(a, ka), (b, kb)] = atlas_taps(3, Vec3::new(0.0, 0.0, 0.02), 0.04);
        assert!(close(a.x, 64.0 + 32.0, 1e-4) && close(b.x, 128.0 + 32.0, 1e-4), "{a} {b}");
        assert!(close(a.y, 3.0 * 64.0 + 32.0, 1e-4));
        assert!(close(ka, 0.5, 1e-5) && close(kb, 0.5, 1e-5));
        // Texel centres hold the points the protocol says: +y is up the tile (towards row 0),
        // outside the extent clamps to the edge texel, slice 0 is at the camera.
        let e = LIGHT_TILE_EXTENT;
        let [(a, ka), _] = atlas_taps(0, Vec3::new(-e + 2.5 * 2.0 * e / 64.0, e - 7.5 * 2.0 * e / 64.0, 0.0), 0.04);
        assert!(close(a.x, 2.5, 1e-3) && close(a.y, 7.5, 1e-3) && close(ka, 1.0, 1e-6), "{a}");
        let [(a, _), _] = atlas_taps(0, Vec3::new(10.0, 0.3, 0.0), 0.04);
        assert!(close(a.x, 63.5, 1e-4) && close(a.y, 24.0, 1e-4), "{a}");
        // The last slice at depth_squash · LIGHT_TILE_DEPTH.
        let [(_, ka), (b, kb)] = atlas_taps(0, Vec3::new(0.0, 0.0, 0.04 * LIGHT_TILE_DEPTH), 0.04);
        assert!(close(b.x, 3.0 * 64.0 + 32.0, 1e-4) && close(kb, 1.0, 1e-6) && close(ka, 0.0, 1e-6));
    }
}
