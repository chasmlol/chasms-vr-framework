//! Gamebryo's per-pixel lighting, as Oblivion's shaders light the player's skin, armour and
//! weapons, on the CPU: the reference the avatar's shader mirrors (`HLSL` here, `PSGamebryo` in
//! `render.rs`).
//!
//! The same terms as Oblivion's own shader passes for its shader model 2 path, the
//! `ShadowLightShader` (`SLS20xx`) and `SkinShader` (`SKIN20xx`) passes, which the game draws one
//! after another into its display-range back buffer. Summed, each fragment is:
//!
//! * per light, its diffuse: `saturate(N·L · colour)` (the skin's: `min(colour·max(N·L, 0) +
//!   ½·colour·(1 − max(N·V, 0))³, 1)`, a rim towards grazing angles), times its attenuation
//!   (a point light's `saturate(1 − d²/R²)`);
//! * the light: `saturate(ambient + emittance·glow + Σ diffuse)`;
//! * the colour: `base · vertex colour · light`;
//! * per light, its specular added: `saturate(colour · N.a · (N·H)^glossiness · soft(N·L) · att)`,
//!   where `soft` fades the highlight out past the terminator (`N·L + ½` below `N·L = 0.2`);
//! * the fog: `lerp(colour, fog, amount)`.
//!
//! Two of the game's other shaders take the same lights:
//!
//! * the hair's (`HAIR2000`): each light's `colour · (max(N·L, 0) + (1 − max(N·V, 0))³)`, its rim
//!   at full strength and nothing capped; its base tinted, `base · 2 · (0.5 + mask · (tint − 0.5))`
//!   (the mask the vertex colour's green); its layer map laid over the base by its alpha; each
//!   light's anisotropic highlight added, `(tint' · ½ + 0.15) · mask · colour · aniso(N·L, N·H).a`
//!   (`tint'` the tint as the mask has it, `mask` the normal map's alpha, `aniso` the game's
//!   highlight table, `textures\effects\aniso.dds`). Its height map's parallax is left out;
//! * the head's FaceGen blend (`SKIN2001`): its colour `4 · detail · (base + 2 · (morphs − 0.5))`
//!   before the skin's lighting ([`face`]).
//!
//! N is the normal map's (`2·rgb − 1` in the tangent frame: tangent along +u, binormal along +v of
//! the texture coordinates, as the meshes' own tangent space data has them), its alpha the specular
//! mask. Everything is in the display's 0..1 range, as the game's 8-bit back buffer holds it.

use glam::{Vec3, Vec4};
use rustvr_protocol::{GL_GB_DIRECTIONAL, GL_GB_POINT, GameLight};

/// Below this `N·L` the highlight fades (`SLS2037`: `c0.y`).
pub const SOFT_EDGE: f32 = 0.2;

/// A fragment as the game's shaders see it.
#[derive(Clone, Copy, Debug)]
pub struct Fragment {
    /// Where it is, and the eye (one space: the lights' positions are in it too).
    pub p: Vec3,
    pub eye: Vec3,
    /// The normal from the normal map, in that space (unit).
    pub n: Vec3,
    /// The base texture times the vertex colour (rgb).
    pub base: Vec3,
    /// The normal map's alpha: the specular mask.
    pub spec_mask: f32,
    /// The glow map's colour times the emittance colour (0 without one).
    pub glow: Vec3,
}

/// The material's terms.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub skin: bool,
    /// The hair's lighting (its own shader's).
    pub hair: bool,
    /// The specular power (the material's glossiness); `None`: no specular.
    pub glossiness: Option<f32>,
}

/// A light's attenuation and the direction towards it, or `None` if it doesn't reach.
pub fn reach(l: &GameLight, p: Vec3) -> Option<(Vec3, f32)> {
    let at = Vec3::new(l.pos[0], l.pos[1], l.pos[2]);
    match l.kind {
        GL_GB_DIRECTIONAL => Some((at.normalize_or_zero(), 1.0)),
        GL_GB_POINT => {
            let d = at - p;
            let att = (1.0 - d.length_squared() * l.pos[3]).clamp(0.0, 1.0);
            (att > 0.0).then(|| (d.normalize_or_zero(), att))
        }
        _ => None,
    }
}

/// `soft(N·L)`: the highlight past the terminator.
pub fn soft(nl: f32) -> f32 {
    if nl <= SOFT_EDGE { (nl + 0.5).max(0.0) } else { 1.0 }
}

/// One light's diffuse and specular for the fragment.
pub fn light(f: &Fragment, s: &Surface, l: &GameLight) -> (Vec3, Vec3) {
    let Some((to, att)) = reach(l, f.p) else { return (Vec3::ZERO, Vec3::ZERO) };
    let c = Vec3::new(l.color[0], l.color[1], l.color[2]);
    let nl = f.n.dot(to);
    let v = (f.eye - f.p).normalize_or_zero();
    let diffuse = if s.hair {
        c * (nl.max(0.0) + (1.0 - f.n.dot(v).max(0.0)).powi(3))
    } else if s.skin {
        let rim = (1.0 - f.n.dot(v).max(0.0)).powi(3);
        (c * nl.max(0.0) + 0.5 * rim * c).min(Vec3::ONE)
    } else {
        (c * nl).clamp(Vec3::ZERO, Vec3::ONE)
    };
    let spec = match s.glossiness {
        Some(power) => {
            let h = (to + v).normalize_or_zero();
            let k = f.spec_mask * f.n.dot(h).clamp(0.0, 1.0).powf(power) * soft(nl);
            (c * k * att).clamp(Vec3::ZERO, Vec3::ONE)
        }
        None => Vec3::ZERO,
    };
    (diffuse * att, spec)
}

/// The fragment's colour (display range) under `ambient` and `lights`, before the fog.
pub fn shade(f: &Fragment, s: &Surface, ambient: Vec3, lights: &[GameLight]) -> Vec3 {
    let (mut d, mut sp) = (Vec3::ZERO, Vec3::ZERO);
    for l in lights {
        let (ld, ls) = light(f, s, l);
        d += ld;
        sp += ls;
    }
    let lit = (ambient + f.glow + d).clamp(Vec3::ZERO, Vec3::ONE);
    (f.base * lit + sp).clamp(Vec3::ZERO, Vec3::ONE)
}

/// The head's colour by the game's FaceGen blend: the base, moved by the texture morphs' texture
/// (`morphs`, about 0.5), under the detail map (`detail`, about 0.25: a 4× modulate).
pub fn face(base: Vec3, morphs: Vec3, detail: Vec3) -> Vec3 {
    4.0 * detail * (base + 2.0 * (morphs - Vec3::splat(0.5)))
}

/// The hair's base colour: tinted by `tint` where `mask` (the vertex colour's green) says.
pub fn hair_tint(base: Vec3, tint: Vec3, mask: f32) -> Vec3 {
    base * 2.0 * (Vec3::splat(0.5) + mask * (tint - Vec3::splat(0.5)))
}

/// The fog over a colour at `distance` (`fog_fade`: amount = saturate(distance·y + x)).
pub fn fogged(c: Vec3, distance: f32, fog: Vec4, fade: Vec4) -> Vec3 {
    if fade.y == 0.0 {
        return c;
    }
    let k = (distance * fade.y + fade.x).clamp(0.0, 1.0);
    c.lerp(fog.truncate(), k)
}

/// The shader's half of the above (used by `PSGamebryo`).
pub const HLSL: &str = r#"
#define GB_LIT 8
#define GB_SKIN 9
#define GB_HAIR 10
#define GB_DIRECTIONAL 16
#define GB_POINT 17

// The meshes' tangent frame (tangent along +u, binormal along +v), rebuilt from the fragment's
// derivatives, square to the normal n.
void gb_frame(float3 dpx, float3 dpy, float2 dux, float2 duy, float3 n, out float3 t, out float3 b) {
    float det = dux.x * duy.y - duy.x * dux.y;
    float3 dpdu = dpx * duy.y - dpy * dux.y;
    float3 dpdv = dpy * dux.x - dpx * duy.x;
    if (det < 0) { dpdu = -dpdu; dpdv = -dpdv; }
    t = dpdu - n * dot(n, dpdu);
    b = dpdv - n * dot(n, dpdv);
    float3 any = abs(n.y) < 0.99 ? float3(0, 1, 0) : float3(1, 0, 0);
    t = dot(t, t) > 1e-24 ? normalize(t) : normalize(cross(any, n));
    b = dot(b, b) > 1e-24 ? normalize(b) : cross(n, t);
}

float gb_soft(float nl) { return nl <= 0.2 ? max(nl + 0.5, 0) : 1; }

// One light's diffuse (added to d) and specular (added to s) at p, normal n, towards the eye v.
// Whether a light reaches p: the direction towards it and its attenuation.
bool gb_reach(MLight l, float3 p, out float3 to, out float att) {
    to = 0;
    att = 1;
    if (l.info.x == GB_DIRECTIONAL) {
        to = normalize(l.pos.xyz);
    } else if (l.info.x == GB_POINT) {
        float3 dd = l.pos.xyz - p;
        att = saturate(1 - dot(dd, dd) * l.pos.w);
        if (att <= 0) return false;
        to = normalize(dd);
    } else {
        return false;
    }
    return true;
}

void gb_light(MLight l, float3 p, float3 n, float3 v, bool skin, bool hair, float gloss, float mask, inout float3 d, inout float3 s) {
    float3 to;
    float att;
    if (!gb_reach(l, p, to, att)) return;
    float3 c = l.color.rgb;
    float nl = dot(n, to);
    float3 dif;
    if (hair) {
        float k = 1 - max(dot(n, v), 0);
        dif = c * (max(nl, 0) + k * k * k);
    } else if (skin) {
        float k = 1 - max(dot(n, v), 0);
        dif = min(c * max(nl, 0) + 0.5 * k * k * k * c, 1);
    } else {
        dif = saturate(nl * c);
    }
    d += dif * att;
    if (gloss > 0) {
        float3 h = normalize(to + v);
        float k = mask * pow(saturate(dot(n, h)), gloss) * gb_soft(nl);
        s += saturate(c * k * att);
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn sun(towards: Vec3, c: f32) -> GameLight {
        GameLight { pos: [towards.x, towards.y, towards.z, 0.0], color: [c, c, c, 1.0], kind: GL_GB_DIRECTIONAL, ..Default::default() }
    }

    fn frag(n: Vec3) -> Fragment {
        Fragment { p: Vec3::ZERO, eye: Vec3::new(0.0, 0.0, 1.0), n, base: Vec3::splat(0.8), spec_mask: 1.0, glow: Vec3::ZERO }
    }

    #[test]
    fn the_base_pass() {
        // SLS2000: saturate(saturate(N·L · c) + ambient) · base.
        let f = frag(Vec3::Z);
        let s = Surface { skin: false, hair: false, glossiness: None };
        let l = sun(Vec3::new(0.0, 0.6, 0.8), 0.5);
        let c = shade(&f, &s, Vec3::splat(0.2), &[l]);
        assert!((c.x - 0.8 * (0.5 * 0.8 + 0.2)).abs() < 1e-5);
        // Lit past 1 it holds at the base colour.
        let c = shade(&f, &s, Vec3::splat(0.9), &[sun(Vec3::Z, 1.0)]);
        assert!((c.x - 0.8).abs() < 1e-5);
    }

    #[test]
    fn skin_has_a_rim() {
        // Seen edge on, lit from behind: the rim alone, half the light's colour.
        let mut f = frag(Vec3::X);
        f.eye = Vec3::new(0.0, 0.0, 1.0);
        let skin = Surface { skin: true, hair: false, glossiness: None };
        let plain = Surface { skin: false, hair: false, glossiness: None };
        let l = sun(-Vec3::X, 1.0);
        assert_eq!(shade(&f, &plain, Vec3::ZERO, &[l]), Vec3::ZERO);
        assert!((shade(&f, &skin, Vec3::ZERO, &[l]).x - 0.8 * 0.5).abs() < 1e-5);
    }

    #[test]
    fn a_point_light_fades_out_at_its_radius() {
        let f = frag(Vec3::Z);
        let s = Surface { skin: false, hair: false, glossiness: None };
        let at = |d: f32| GameLight { pos: [0.0, 0.0, d, 1.0 / 4.0], color: [1.0; 4], kind: GL_GB_POINT, ..Default::default() };
        assert!(shade(&f, &s, Vec3::ZERO, &[at(1.0)]).x > 0.5);
        assert_eq!(shade(&f, &s, Vec3::ZERO, &[at(2.0)]), Vec3::ZERO);
    }

    #[test]
    fn the_highlight_fades_past_the_terminator() {
        assert_eq!(soft(0.5), 1.0);
        assert!((soft(0.1) - 0.6).abs() < 1e-6);
        assert_eq!(soft(-0.6), 0.0);
        // Straight back to the eye: the mask times the light.
        let f = frag(Vec3::Z);
        let s = Surface { skin: false, hair: false, glossiness: Some(30.0) };
        let (_, sp) = light(&f, &s, &sun(Vec3::Z, 0.5));
        assert!((sp.x - 0.5).abs() < 1e-5);
    }
}
