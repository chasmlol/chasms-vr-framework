//! A mount's reins, drawn by the runtime when the game's mount has none of its own (`RIDE_BITS`):
//! straps of the game's own leather (`RIDE_LEATHER`: a model's material, cut from a stretch of its
//! texture), drawn with the body as what the hands hold is ([`HeldModel::load_bent`]), so the
//! game's depth hides them behind its world and its lights light them. Each runs from a ring of the
//! bit to the hand holding it (or, let go, back to where the rider holds it), hanging by its slack,
//! and lies on the mount's neck and head wherever it would pass through them (`RIDE_NECK`); with
//! both held, their bight hangs between the hands. One hand alone holds both, as on a western
//! saddle.
//!
//! A strap is a chain of cross-sections, each a part of the model (one of its bones): placed where
//! its share of the strap's length is along its curve, turned along the curve, the strap's flat
//! carried along from the bit without twisting (parallel transport). Without the leather (a game
//! that doesn't name it, or it didn't load), the same curves are drawn as flat ribbons of its
//! colour.

use glam::{Mat4, Quat, Vec3};
use rustvr_avatar::{HeldDraw, HeldModel};
use rustvr_gpu::Gpu;
use rustvr_protocol::*;

use crate::compositor::Ribbon;

/// Each rein's cross-sections (bit to hand), and the bight's.
const REIN_RINGS: usize = 24;
const BIGHT_RINGS: usize = 14;
/// The strap (metres): a rein's leather, 16 mm wide and 4 mm thick; and how long its model is laid
/// out (the texture runs along that much; a rein stretches to its length as it's drawn).
const WIDTH: f32 = 0.016;
const THICK: f32 = 0.004;
const REIN_LENGTH: f32 = 1.2;
const BIGHT_LENGTH: f32 = 0.6;
/// Slack: a rein at rest is this much longer than its way from the bit over the neck to where it's
/// held (a share), and hangs this deep for it (a share of a free rope's); the bight's spare length
/// between two hands (metres).
const SLACK: f32 = 0.12;
const SAG: f32 = 0.7;
const BIGHT: f32 = 0.25;
/// How far off the neck a strap lies (metres): its half thickness, and the little the neck's
/// ellipses fall inside the game's own surface. And how many times a curve is laid on the neck and
/// smoothed.
const CLEAR: f32 = 0.01;
const LAY_PASSES: usize = 4;

/// The reins this frame.
#[derive(Default)]
pub struct Reins {
    /// The leather (its model's name and the stretch of its texture) and the reins' model made of
    /// it (None: it didn't load, said once).
    model: Option<(String, [f32; 4], Option<HeldModel>)>,
    /// Each cross-section's place (left rein, right rein, the bight; None: not drawn).
    parts: Vec<Option<Mat4>>,
    /// Without the leather: the curves as ribbons.
    ribbons: Vec<Ribbon>,
}

impl Reins {
    /// Where the reins are this frame: `held`, which rein each hand holds; `hands`, the hands'
    /// grips (tracking space); `light`, the game's (for the ribbons).
    pub fn update(&mut self, gpu: &Gpu, r: &Riding, held: [Option<usize>; 2], hands: [Vec3; 2], light: Option<&LightHint>) {
        self.parts.clear();
        self.ribbons.clear();
        if r.flags & RIDE_BITS == 0 || !r.mounted() {
            return;
        }
        let curves = curves(r, held, hands);
        if let Some(leather) = r.reins_leather() {
            if self.model.as_ref().is_none_or(|(name, uv, _)| name != leather || *uv != r.reins_uv) {
                let model = load(gpu, leather, r.reins_uv).map_err(|e| tracing::warn!("reins: their leather {leather}: {e:#} (drawn as ribbons)")).ok();
                if model.is_some() {
                    tracing::info!("reins: of {leather}'s leather, cut from {:?} of its texture", r.reins_uv);
                }
                self.model = Some((leather.to_owned(), r.reins_uv, model));
            }
        } else {
            self.model = None;
        }
        if self.model.as_ref().is_some_and(|(_, _, m)| m.is_some()) {
            let laid = [(REIN_RINGS, REIN_LENGTH), (REIN_RINGS, REIN_LENGTH), (BIGHT_RINGS, BIGHT_LENGTH)];
            for (curve, (rings, length)) in curves.iter().zip(laid) {
                match curve {
                    Some((points, flat)) => place(&mut self.parts, &resample(points, rings), *flat, length),
                    None => self.parts.extend(std::iter::repeat_n(None, rings)),
                }
            }
        } else {
            let lit = light
                .map(|l| Vec3::from_array(l.ambient) + Vec3::from_array(l.sun_color) * l.sun_dir[1].max(0.0))
                .filter(|l| l.max_element() > 1e-3)
                .unwrap_or(Vec3::ONE);
            let color = (Vec3::new(r.reins_color[0], r.reins_color[1], r.reins_color[2]) * lit).clamp(Vec3::ZERO, Vec3::ONE).extend(1.0);
            for (points, _) in curves.iter().flatten() {
                self.ribbons.extend(points.windows(2).map(|w| Ribbon { a: w[0], b: w[1], width: WIDTH, color }));
            }
        }
    }

    /// The reins as a held model to draw with the body, when they're of the game's leather.
    pub fn draw(&self) -> Option<HeldDraw<'_>> {
        let model = self.model.as_ref()?.2.as_ref()?;
        (!self.parts.is_empty()).then(|| HeldDraw { model, parts: &self.parts, key: "reins" })
    }

    /// The reins as flat ribbons, when they aren't of the game's leather.
    pub fn ribbons(&self) -> Vec<Ribbon> {
        self.ribbons.clone()
    }
}

/// The reins' curves (tracking space) and the way each one's flat faces at its start: the left
/// rein, the right, and their bight between two hands.
type Curve = Option<(Vec<Vec3>, Vec3)>;

fn curves(r: &Riding, held: [Option<usize>; 2], hands: [Vec3; 2]) -> [Curve; 3] {
    let v = |p: [f32; 4]| Vec3::new(p[0], p[1], p[2]);
    let neck = r.neck();
    let bits = [v(r.bits[0]), v(r.bits[1])];
    let holding: Vec<usize> = (0..2).filter(|&h| held[h].is_some()).collect();
    let rein = |side: usize| -> Curve {
        let (bit, rest) = (bits[side], v(r.reins[side]));
        // Its length: its way from the bit along the neck to where it's held, and its slack.
        let way = length(&lay(&straight(bit, rest, REIN_RINGS), neck, true));
        let length = way * (1.0 + SLACK);
        // To the hand holding it; let go, back to where it's held. One hand alone holds both: each
        // comes along its own side of the neck to where it's held, then up to the hand.
        let points = match holding.as_slice() {
            [h] => {
                let mut along = hanging(bit, rest, length, REIN_RINGS);
                along.extend(hanging(rest, hands[*h], rest.distance(hands[*h]) * (1.0 + SLACK), REIN_RINGS / 3).into_iter().skip(1));
                lay(&along, neck, false)
            }
            _ => match (0..2).find(|&h| held[h] == Some(side)) {
                Some(h) => lay(&hanging(bit, hands[h], length, REIN_RINGS), neck, false),
                None => lay(&hanging(bit, rest, length, REIN_RINGS), neck, true),
            },
        };
        // (Its flat against the face at the bit: out from the head.)
        Some((points, (bits[side] - bits[1 - side]).normalize_or(Vec3::X)))
    };
    let bight = match holding.as_slice() {
        [a, b] => {
            let (a, b) = (hands[*a], hands[*b]);
            Some((lay(&hanging(a, b, a.distance(b) + BIGHT, BIGHT_RINGS), neck, false), Vec3::Y))
        }
        _ => None,
    };
    [rein(0), rein(1), bight]
}

/// `n` points from `a` to `b`.
fn straight(a: Vec3, b: Vec3, n: usize) -> Vec<Vec3> {
    (0..n).map(|k| a.lerp(b, k as f32 / (n - 1) as f32)).collect()
}

/// `n` points from `a` to `b` of a strap `length` long, hanging by its slack (a parabola as deep as
/// `SAG` of a free rope's).
fn hanging(a: Vec3, b: Vec3, length: f32, n: usize) -> Vec<Vec3> {
    let chord = a.distance(b);
    let sag = 0.5 * (length * length - chord * chord).max(0.0).sqrt() * SAG;
    (0..n)
        .map(|k| {
            let t = k as f32 / (n - 1) as f32;
            a.lerp(b, t) - Vec3::Y * sag * 4.0 * t * (1.0 - t)
        })
        .collect()
}

/// A curve laid on the neck: what passes through it put on it, smoothed, and laid again. Its ends
/// stay (the bit, a hand), but for a free end (`free_end`: a rein let go), laid on the neck too.
fn lay(points: &[Vec3], neck: &[NeckSection], free_end: bool) -> Vec<Vec3> {
    let mut p = points.to_vec();
    let n = p.len();
    if neck.len() < 2 || n < 3 {
        return p;
    }
    let last = if free_end { n } else { n - 1 };
    for pass in 0..=LAY_PASSES {
        for q in &mut p[1..last] {
            *q = off_neck(*q, neck);
        }
        if pass == LAY_PASSES {
            break;
        }
        let was = p.clone();
        for k in 1..n - 1 {
            p[k] = was[k] * 0.5 + (was[k - 1] + was[k + 1]) * 0.25;
        }
    }
    p
}

/// `p` put on the neck and head (`CLEAR` off them) if it's inside them: between two of their
/// cross-sections, inside the ellipse there (the two blended), it goes out from the ellipse's middle
/// to its edge (deep inside, over the top). Below the ellipse's middle the neck goes straight down
/// (the throat, and the chest under it, are wider than the ellipse): there it goes out to its side.
fn off_neck(mut p: Vec3, neck: &[NeckSection]) -> Vec3 {
    let v = |a: [f32; 4]| Vec3::new(a[0], a[1], a[2]);
    for w in neck.windows(2) {
        let (ca, cb) = (v(w[0].centre), v(w[1].centre));
        let axis = cb - ca;
        if axis.length_squared() < 1e-8 {
            continue;
        }
        let t = (p - ca).dot(axis) / axis.length_squared();
        if !(0.0..=1.0).contains(&t) {
            continue;
        }
        let c = ca.lerp(cb, t);
        let (side, up) = (v(w[0].side).lerp(v(w[1].side), t), v(w[0].up).lerp(v(w[1].up), t));
        let (half_w, half_h) = (side.length() + CLEAR, up.length() + CLEAR);
        let (sx, uy) = (side.normalize_or_zero(), up.normalize_or_zero());
        let (x, y) = ((p - c).dot(sx), (p - c).dot(uy));
        if y < 0.0 {
            if x.abs() < half_w {
                p += sx * (half_w.copysign(x) - x);
            }
            continue;
        }
        let e = (x / half_w).powi(2) + (y / half_h).powi(2);
        if e >= 1.0 {
            continue;
        }
        if e < 1e-4 {
            p += uy * (half_h - y);
        } else {
            let k = 1.0 / e.sqrt() - 1.0;
            p += (sx * x + uy * y) * k;
        }
    }
    p
}

fn length(points: &[Vec3]) -> f32 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// `n` points evenly along a curve.
fn resample(points: &[Vec3], n: usize) -> Vec<Vec3> {
    let total = length(points).max(1e-6);
    let mut out = Vec::with_capacity(n);
    let (mut seg, mut walked) = (0, 0.0);
    for k in 0..n {
        let want = total * k as f32 / (n - 1) as f32;
        while seg + 2 < points.len() && walked + points[seg].distance(points[seg + 1]) < want {
            walked += points[seg].distance(points[seg + 1]);
            seg += 1;
        }
        let d = points[seg].distance(points[seg + 1]).max(1e-6);
        out.push(points[seg].lerp(points[seg + 1], ((want - walked) / d).clamp(0.0, 1.0)));
    }
    out
}

/// A strap's cross-sections placed along its curve (`flat`: the way its flat faces at the start):
/// each one's bone space (the strap laid out straight along +Z, `length` long) to tracking space.
fn place(parts: &mut Vec<Option<Mat4>>, points: &[Vec3], flat: Vec3, length: f32) {
    let n = points.len();
    let tangent = |k: usize| (points[(k + 1).min(n - 1)] - points[k.saturating_sub(1)]).normalize_or(Vec3::Z);
    let mut t = tangent(0);
    let mut up = (flat - t * flat.dot(t)).normalize_or((Vec3::Y - t * t.y).normalize_or(Vec3::X));
    for (k, &p) in points.iter().enumerate() {
        let tk = tangent(k);
        up = (Quat::from_rotation_arc(t, tk) * up).normalize();
        up = (up - tk * up.dot(tk)).normalize_or(up);
        t = tk;
        let side = up.cross(t);
        let laid = length * k as f32 / (n - 1) as f32;
        parts.push(Some(Mat4::from_cols(side.extend(0.0), up.extend(0.0), t.extend(0.0), p.extend(1.0)) * Mat4::from_translation(-Vec3::Z * laid)));
    }
}

/// The reins' model, of the leather of `leather` (`<game>/<asset key>`: its first mesh's material),
/// cut from `uv` of its texture: the left rein, the right and the bight, laid out straight along
/// +Z, each cross-section its own bone.
fn load(gpu: &Gpu, leather: &str, uv: [f32; 4]) -> anyhow::Result<HeldModel> {
    use anyhow::Context;
    let (game, key) = rustvr_guns::desc::parse_model_name(leather).with_context(|| format!("{leather:?} isn't <game>/<asset key>"))?;
    let dir = rustvr_asset::default_root().join(game);
    let source = rustvr_asset::load_model(&dir, key)?;
    let first = source.meshes.first().context("it has no meshes")?;
    anyhow::ensure!(!first.texture.is_empty(), "its first mesh has no texture");
    // (The game's vertex colour on its leather, as it's drawn on the model.)
    let ao = (first.vertices.iter().map(|v| v.ao as u32).sum::<u32>() / first.vertices.len().max(1) as u32) as u8;
    let mut mesh = rustvr_asset::Mesh { texture: first.texture.clone(), material: first.material.clone(), name: "reins".into(), ..Default::default() };
    let mut bone = 0;
    for (rings, laid) in [(REIN_RINGS, REIN_LENGTH), (REIN_RINGS, REIN_LENGTH), (BIGHT_RINGS, BIGHT_LENGTH)] {
        strap(&mut mesh, bone, rings, laid, uv, ao);
        bone += rings;
    }
    let model = rustvr_asset::Model { source: format!("the reins, of {leather}"), meshes: vec![mesh], ..Default::default() };
    HeldModel::load_bent(gpu, &dir, &model)
}

/// A strap of `rings` cross-sections (bones `first`..), `laid` long along +Z: a flat band `WIDTH`
/// by `THICK`, its faces and edges cut from `uv` of the texture (across u, along v), the texture
/// mirrored at each end of its stretch so it runs on without a seam.
fn strap(mesh: &mut rustvr_asset::Mesh, first: usize, rings: usize, laid: f32, uv: [f32; 4], ao: u8) {
    let [u0, v0, u1, v1] = uv;
    // (The texture's stretch is as long as the strap is wide times its own length over its width.)
    let tile = WIDTH * ((v1 - v0) / (u1 - u0).max(1e-6)).abs();
    let edge = (u1 - u0) * THICK / WIDTH;
    let (w, h) = (WIDTH * 0.5, THICK * 0.5);
    // Each face: its two corners (across, up), their u, and its normal.
    let faces: [([f32; 2], [f32; 2], [f32; 2], [f32; 3]); 4] = [
        ([-w, h], [w, h], [u0, u1], [0.0, 1.0, 0.0]),
        ([w, h], [w, -h], [u1 - edge, u1], [1.0, 0.0, 0.0]),
        ([w, -h], [-w, -h], [u1, u0], [0.0, -1.0, 0.0]),
        ([-w, -h], [-w, h], [u0, u0 + edge], [-1.0, 0.0, 0.0]),
    ];
    let base = mesh.vertices.len() as u32;
    for k in 0..rings {
        let z = laid * k as f32 / (rings - 1) as f32;
        let along = (z / tile).rem_euclid(2.0);
        let v = v0 + (v1 - v0) * (1.0 - (along - 1.0).abs());
        for (a, b, u, n) in faces {
            for (corner, u) in [(a, u[0]), (b, u[1])] {
                mesh.vertices.push(rustvr_asset::Vertex { pos: [corner[0], corner[1], z], normal: n, uv: [u, v], bones: [(first + k) as u8, 0, 0, 0], weights: [255, 0, 0, 0], ao });
            }
        }
    }
    let per = (faces.len() * 2) as u32;
    for k in 0..rings as u32 - 1 {
        for f in 0..faces.len() as u32 {
            let (a, b) = (base + k * per + f * 2, base + k * per + f * 2 + 1);
            let (c, d) = (a + per, b + per);
            mesh.indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }
}
