//! The shading models the avatar draws a game's own materials with ([`Class`]): a deferred
//! renderer's (the classes up to [`Class::GlassReflect`], evaluated per fragment in one pass by
//! [`HLSL`]) and Gamebryo's forward per-pixel lighting (`gamebryo.rs`). Space: the game camera's
//! view space (x right, y up, z forward, metres).

use glam::Vec3;
use rustvr_asset::Shader;
use rustvr_protocol::GameLighting;

/// The deferred model's environment tint when the adapter leaves
/// [`GameLighting::envmap_color`] zero.
pub const ENVMAP_COLOR: Vec3 = Vec3::new(0.5706, 0.6588, 0.75);

/// The shading classes: the deferred model's G-buffer and forward ones, then Gamebryo's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Class {
    /// Opaque, no environment term (alpha-tested or not).
    Plain = 1,
    /// Metal: the environment baked into the albedo.
    MetalEnv = 2,
    /// Cloth and skin: Lambert, subsurface tint, grey specular, grazing sheen.
    ClothSkin = 3,
    /// Glossy glass.
    GlossyGlass = 4,
    /// Emissive, unlit.
    Emissive = 5,
    /// Forward glass: unlit, blended.
    GlassTint = 6,
    /// Forward glass: the environment reflection, blended.
    GlassReflect = 7,
    /// Gamebryo's per-pixel lighting (`gamebryo.rs`, Oblivion's `ShadowLightShader`).
    GamebryoLit = 8,
    /// Gamebryo's skin (Oblivion's `SkinShader`).
    GamebryoSkin = 9,
    /// Gamebryo's hair (Oblivion's `HairShader`).
    GamebryoHair = 10,
}

impl Class {
    pub fn of(shader: Shader) -> Option<Class> {
        Some(match shader {
            // Left to a caller's special draw.
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
            // The environment term is optional on cloth and skin: without a cube, it's left out.
            Class::ClothSkin => (true, true, false),
            Class::Emissive => (false, true, false),
            Class::GlassTint => (false, false, false),
            Class::GlassReflect => (false, false, true),
            // (The normal and glow maps when the mesh has them.)
            Class::GamebryoLit | Class::GamebryoSkin | Class::GamebryoHair => (false, false, false),
        }
    }
}

/// The frame's environment tint: the game's, or [`ENVMAP_COLOR`] when the adapter didn't fill it.
pub fn envmap_color(l: &GameLighting) -> Vec3 {
    let c = Vec3::new(l.envmap_color[0], l.envmap_color[1], l.envmap_color[2]);
    if c == Vec3::ZERO { ENVMAP_COLOR } else { c }
}

/// The deferred model, per fragment: the G-buffer terms, each light's, the ambient and the tone
/// curve and grading, as `render.rs`'s `PSDeferred` uses them.
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
