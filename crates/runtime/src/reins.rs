//! A mount's reins, drawn by the runtime when the game's mount has none of its own (`RIDE_BITS`):
//! straps of the game's own leather (`RIDE_LEATHER`: a model's material, cut from a stretch of its
//! texture), drawn with the body as what the hands hold is ([`HeldModel::load_strap`]), so the
//! game's depth hides them behind its world and its lights light them. Each runs from a ring of the
//! bit to the hand holding it (or, let go, back to where the rider holds it), hanging by its slack,
//! and lies on the mount's neck and head wherever it would pass through them (`RIDE_NECK`); with
//! both held, their bight hangs between the hands. One hand alone holds both, as on a western
//! saddle. A rein runs through the fist as a rider's does: in at the little finger, out over the
//! index under the thumb, where the bight hangs from ([`FIST`]).
//!
//! Each rein is a rope ([`Rope`]; the bight just hangs between the fists): a chain of points carried
//! from frame to frame, swinging under
//! gravity (and settling over [`SWING`]), each a fixed share of its length from the next, held at
//! its ends (the bit and a hand, or where the rider holds it let go) and kept out of the neck. Its
//! shape only ever moves on from the last frame's, so it can't jump. Out of the neck, above the
//! middle of its ellipse, a point goes straight out from that middle (a strap crossing to a hand on
//! the other side goes over the top); below it, straight out sideways on the side it's on, which
//! it learns only while it's out of the neck (so it never flips across). The neck's ends and bends
//! are rounded (`CAP`). Near a hand (`HAND_FADE`) a rope is kept out of the neck less and less: a
//! hand in the horse (the hands go through it) keeps its rein in it there, hidden by the game's
//! depth, not fought over. The game's depth hides the reins a little later than the body
//! ([`DEPTH_SLACK`]): a strap lying on the horse's skin doesn't flicker against it.
//!
//! A strap is a chain of cross-sections, each a part of the model (one of its bones): placed where
//! its share of the strap's length is along its curve, turned along the curve, the strap's flat
//! carried along from the bit without twisting (parallel transport). Without the leather (a game
//! that doesn't name it, or it didn't load), the same curves are drawn as flat ribbons of its
//! colour.
//!
//! The reins are laid once a frame, where the game has the mount. A game picture moved on to where
//! the head is now (the session's warp) moves the mount in it with the rest of its world, so each
//! picture's reins ([`Reins::place`]) go with it at the mount's end (the bit, where they're held),
//! less and less towards the hands, which stay in the hands.

use glam::{Mat4, Quat, Vec3};
use rustvr_avatar::{HeldDraw, HeldModel};
use rustvr_gpu::Gpu;
use rustvr_protocol::*;

use crate::compositor::Ribbon;

/// Half a fist's width (metres): a rein goes into it this far down the grip's axis from the palm's
/// middle (the little finger's side, -Z: the grip's fingers lie along -Y, its thumb along +Z), and
/// the bight comes out of it this far up (over the index, under the thumb).
const FIST: f32 = 0.035;
/// Each rein's cross-sections (bit to hand), and the bight's; and the points of their ropes.
const REIN_RINGS: usize = 24;
const BIGHT_RINGS: usize = 14;
const REIN_POINTS: usize = 28;
const BIGHT_POINTS: usize = 14;
/// A rope's swing: gravity (m/s²), how long its swinging takes to die away most of the way
/// (seconds: leather reins settle at once; and a point on the neck stops there, as leather on a
/// horse's coat does), and how many times a frame its points are put their length apart and out of
/// the neck. A frame longer than `LONGEST` (seconds) counts as that long; an
/// end that moves further than `TELEPORT` in a frame (metres) starts the rope afresh.
const GRAVITY: f32 = 9.8;
pub const SWING: f32 = 0.05;
const ITERATIONS: usize = 12;
const LONGEST: f32 = 0.05;
const TELEPORT: f32 = 0.5;
/// A rope's ends further apart than it's long: it's that much longer than they're apart (a hand
/// slides along it).
const TAUT_SLIP: f32 = 0.03;
/// How fast a rope's points may slide on their own (m/s), beyond where its ends carry them: a rein
/// tipping over the crest slips over it, not across in a frame.
const SLIDE: f32 = 1.5;
/// The strap (metres): a rein's leather, 16 mm wide and 4 mm thick; and how long its model is laid
/// out (the texture runs along that much; a rein stretches to its length as it's drawn).
const WIDTH: f32 = 0.016;
const THICK: f32 = 0.004;
const REIN_LENGTH: f32 = 1.2;
const BIGHT_LENGTH: f32 = 0.6;
/// Slack: a rein at rest is this much longer than its way from the bit over the neck to where it's
/// held (a share), and hangs this deep for it (a share of a free rope's); the bight's spare length
/// between two hands (metres).
const SLACK: f32 = 0.06;
const SAG: f32 = 0.7;
/// How gently a strap's last slack goes as it's pulled taut (metres).
const TAUT: f32 = 0.02;
const BIGHT: f32 = 0.25;
/// How far off the neck a strap lies (metres): its half thickness, and the little the neck's
/// ellipses fall inside the game's own surface. And how many times a curve is laid on the neck and
/// smoothed.
const CLEAR: f32 = 0.01;
const LAY_PASSES: usize = 4;
/// Past the neck's first and last cross-sections, and round the outside of its bends, it rounds off
/// over this (metres) rather than stopping square; and within this of a hand along a rein, the rein
/// is laid on it less and less (none at the hand).
const CAP: f32 = 0.1;
const HAND_FADE: f32 = 0.12;
/// How far behind the game's surface the reins may be before it hides them (metres): the skin they
/// lie on is the game's, a pixel's depth on a neck seen along it spans centimetres, and the neck's
/// ellipses fall inside it in places; the neck between the eye and the far rein is a quarter of a
/// metre through.
const DEPTH_SLACK: f32 = 0.03;

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
    /// This frame's straps laid, for each picture's place of them ([`Reins::place`]); their ropes,
    /// and which rein each hand held when they were made.
    laid: [Curve; 3],
    ropes: [Option<Rope>; 3],
    last_held: [Option<usize>; 2],
}

impl Reins {
    /// Where the reins are this frame (`dt` seconds after the last): `held`, which rein each hand
    /// holds; `hands`, the hands' grips (tracking space: the palm's middle, turned); `light`, the
    /// game's (for the ribbons).
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, gpu: &Gpu, r: &Riding, held: [Option<usize>; 2], hands: [(Vec3, Quat); 2], light: Option<&LightHint>, dt: f32) {
        self.parts.clear();
        self.ribbons.clear();
        self.laid = [None, None, None];
        if r.flags & RIDE_BITS == 0 || !r.mounted() {
            self.ropes = [None, None, None];
            return;
        }
        // (A rein taken or let go: the ropes are new.)
        if held != self.last_held {
            self.ropes = [None, None, None];
            self.last_held = held;
        }
        self.laid = straps(r, held, hands, &mut self.ropes, dt.clamp(0.0, LONGEST));
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
            self.parts = parts(&self.laid);
        } else {
            let lit = light
                .map(|l| Vec3::from_array(l.ambient) + Vec3::from_array(l.sun_color) * l.sun_dir[1].max(0.0))
                .filter(|l| l.max_element() > 1e-3)
                .unwrap_or(Vec3::ONE);
            let color = (Vec3::new(r.reins_color[0], r.reins_color[1], r.reins_color[2]) * lit).clamp(Vec3::ZERO, Vec3::ONE).extend(1.0);
            for (points, _, _) in self.laid.iter().flatten() {
                self.ribbons.extend(points.windows(2).map(|w| Ribbon { a: w[0], b: w[1], width: WIDTH, color }));
            }
        }
    }

    /// The reins for a picture whose mount is `picture` from where the game had it (tracking space
    /// to tracking space: the picture's mapping then, to the one it's shown with): at the mount's
    /// end they go with it, less and less towards a hand. The identity: as the game had it.
    pub fn place(&mut self, picture: Mat4) {
        if self.model.as_ref().is_none_or(|(_, _, m)| m.is_none()) || self.laid.iter().all(Option::is_none) {
            return;
        }
        if picture == Mat4::IDENTITY {
            self.parts = parts(&self.laid);
            return;
        }
        let moved: [Curve; 3] = std::array::from_fn(|i| {
            self.laid[i].as_ref().map(|(points, flat, ends)| {
                let total = length(points).max(1e-6);
                let mut walked = 0.0;
                let carried = points
                    .iter()
                    .enumerate()
                    .map(|(k, &q)| {
                        if k > 0 {
                            walked += q.distance(points[k - 1]);
                        }
                        let share = ends[0] as u32 as f32 + (ends[1] as u32 as f32 - ends[0] as u32 as f32) * walked / total;
                        q + (picture.transform_point3(q) - q) * share
                    })
                    .collect();
                (carried, picture.transform_vector3(*flat), *ends)
            })
        });
        self.parts = parts(&moved);
    }

    /// The reins as a held model to draw with the body, when they're of the game's leather.
    pub fn draw(&self) -> Option<HeldDraw<'_>> {
        let model = self.model.as_ref()?.2.as_ref()?;
        (!self.parts.is_empty()).then(|| HeldDraw { model, parts: &self.parts, key: "reins", overlay: None, bend: 0.0 })
    }

    /// The reins as flat ribbons, when they aren't of the game's leather.
    pub fn ribbons(&self) -> Vec<Ribbon> {
        self.ribbons.clone()
    }
}

/// A strap laid (tracking space): its points, the way its flat faces at its start, and whether its
/// start and its end are the mount's (the bit, where it's held) rather than a hand's.
type Curve = Option<(Vec<Vec3>, Vec3, [bool; 2])>;

/// A strap as a rope: its points now and a frame ago (their swing), each one's side of the neck
/// across its right (-1, 1; 0 not known), and its length.
struct Rope {
    x: Vec<Vec3>,
    was: Vec<Vec3>,
    sides: Vec<f32>,
    length: f32,
}

impl Rope {
    /// A rope `length` long from `from` to `to`, hanging by its slack, of `n` points on `side`.
    fn new(from: Vec3, to: Vec3, length: f32, n: usize, side: f32) -> Rope {
        let x = hanging(from, to, length, n);
        Rope { was: x.clone(), x, sides: vec![side; n], length }
    }

    /// A frame of `dt` seconds: its ends at `from` and `to` (`hands`: which of them are hands), its
    /// points swung on by their own way and gravity, then a few times over each put its share of the
    /// length from the next and out of the neck (towards a hand, less and less); then each learns
    /// its side, where it's out of the neck. Never shorter than its ends are apart: a hand further
    /// off than the rein reaches slides along it, as a rider's does, rather than snapping it taut.
    #[allow(clippy::too_many_arguments)]
    fn step(&mut self, from: Vec3, to: Vec3, hands: [bool; 2], dt: f32, neck: &[NeckSection], right: Vec3) {
        let n = self.x.len();
        let before = self.x.clone();
        let carried = (from - before[0], to - before[n - 1]);
        let keep = (-dt / SWING).exp();
        for k in 1..n - 1 {
            let way = (self.x[k] - self.was[k]) * keep;
            self.was[k] = self.x[k];
            self.x[k] += way - Vec3::Y * GRAVITY * dt * dt;
        }
        (self.x[0], self.x[n - 1]) = (from, to);
        (self.was[0], self.was[n - 1]) = (from, to);
        self.length = self.length.max(from.distance(to) * (1.0 + TAUT_SLIP));
        let seg = self.length / (n - 1) as f32;
        let weight: Vec<f32> = (0..n)
            .map(|k| {
                let fade = |d: f32| {
                    let x = (d / HAND_FADE).min(1.0);
                    x * x * (3.0 - 2.0 * x)
                };
                let mut w = 1.0;
                if hands[0] {
                    w *= fade(k as f32 * seg);
                }
                if hands[1] {
                    w *= fade((n - 1 - k) as f32 * seg);
                }
                // (Each pass's share of it, so that all of them come to that much.)
                1.0 - (1.0 - w).powf(1.0 / ITERATIONS as f32)
            })
            .collect();
        let mut touching = vec![false; n];
        for _ in 0..ITERATIONS {
            for k in 0..n - 1 {
                let d = self.x[k + 1] - self.x[k];
                let len = d.length();
                if len < 1e-6 {
                    continue;
                }
                let fix = d * ((len - seg) / len);
                // (Its ends stay where they're held.)
                let (a, b) = (if k == 0 { 0.0 } else { 1.0 }, if k + 1 == n - 1 { 0.0 } else { 1.0 });
                if a + b > 0.0 {
                    self.x[k] += fix * (a / (a + b));
                    self.x[k + 1] -= fix * (b / (a + b));
                }
            }
            for k in 1..n - 1 {
                let q = self.x[k];
                let on = off_neck(q, neck, right, self.sides[k]);
                if on != q {
                    touching[k] = true;
                    self.x[k] = q + (on - q) * weight[k];
                }
            }
        }
        // (A point slides on its own only so fast; on the neck it stops there: its swing is gone.)
        for k in 1..n - 1 {
            let expected = before[k] + carried.0.lerp(carried.1, k as f32 / (n - 1) as f32);
            let own = self.x[k] - expected;
            if own.length() > SLIDE * dt {
                self.x[k] = expected + own.normalize() * SLIDE * dt;
            }
            if touching[k] {
                self.was[k] = self.x[k];
            }
        }
        for (q, side) in self.x.iter().zip(self.sides.iter_mut()) {
            if let Some(now) = side_of(*q, neck, right) {
                *side = now;
            }
        }
    }
}

/// Each cross-section's place along the curves (None: a strap not drawn).
fn parts(curves: &[Curve; 3]) -> Vec<Option<Mat4>> {
    let mut out = Vec::new();
    let laid = [(REIN_RINGS, REIN_LENGTH), (REIN_RINGS, REIN_LENGTH), (BIGHT_RINGS, BIGHT_LENGTH)];
    for (curve, (rings, length)) in curves.iter().zip(laid) {
        match curve {
            Some((points, flat, _)) => place(&mut out, &resample(points, rings), *flat, length),
            None => out.extend(std::iter::repeat_n(None, rings)),
        }
    }
    out
}

/// The left rein, the right, and their bight between two hands, as their ropes (`ropes`, made
/// afresh when there's none or an end leapt) have them after a frame of `dt` seconds.
fn straps(r: &Riding, held: [Option<usize>; 2], hands: [(Vec3, Quat); 2], ropes: &mut [Option<Rope>; 3], dt: f32) -> [Curve; 3] {
    let v = |p: [f32; 4]| Vec3::new(p[0], p[1], p[2]);
    // (Where a rein goes into a fist, and the bight comes out of it.)
    let into = |h: usize| hands[h].0 + hands[h].1 * Vec3::new(0.0, 0.0, -FIST);
    let out_of = |h: usize| hands[h].0 + hands[h].1 * Vec3::new(0.0, 0.0, FIST);
    let neck = r.neck();
    let bits = [v(r.bits[0]), v(r.bits[1])];
    // (The mount's right, across it.)
    let right = (bits[1] - bits[0]).normalize_or(Vec3::X);
    let holding: Vec<usize> = (0..2).filter(|&h| held[h].is_some()).collect();
    let mut out: [Curve; 3] = [None, None, None];
    for side in 0..2 {
        let (bit, rest) = (bits[side], v(r.reins[side]));
        let own = if side == 0 { -1.0 } else { 1.0 };
        // To the hand holding it (one hand alone holds both); let go, to where it's held, put on the
        // neck.
        let hand = match holding.as_slice() {
            [h] => Some(*h),
            _ => (0..2).find(|&h| held[h] == Some(side)),
        };
        let end = hand.map_or_else(|| off_neck(rest, neck, right, own), into);
        let fresh = |rope: &Rope| rope.x[0].distance(bit) > TELEPORT || rope.x[rope.x.len() - 1].distance(end) > TELEPORT;
        if ropes[side].as_ref().is_none_or(fresh) {
            // Its length: its way from the bit along the neck to where it's held (or to the hand,
            // if that's further), and its slack.
            let way = length(&lay(&straight(bit, rest, REIN_POINTS), neck, right, own)).max(bit.distance(end));
            ropes[side] = Some(Rope::new(bit, end, way * (1.0 + SLACK), REIN_POINTS, own));
        }
        let rope = ropes[side].as_mut().unwrap();
        rope.step(bit, end, [false, hand.is_some()], dt, neck, right);
        // (Its flat against the face at the bit: out from the head.)
        out[side] = Some((rope.x.clone(), (bits[side] - bits[1 - side]).normalize_or(Vec3::X), [true, hand.is_none()]));
    }
    match holding.as_slice() {
        [a, b] => {
            // The bight just hangs between the fists by its slack (the reins slide through the
            // hands, so it keeps it): laid on the neck as the reins are, it was pushed up over the
            // neck's rounded end behind the withers, where there's no neck.
            let (a, b) = (out_of(*a), out_of(*b));
            ropes[2] = None;
            out[2] = Some((hanging(a, b, a.distance(b) + BIGHT, BIGHT_POINTS), Vec3::Y, [false, false]));
        }
        _ => ropes[2] = None,
    }
    out
}

/// `n` points from `a` to `b`.
fn straight(a: Vec3, b: Vec3, n: usize) -> Vec<Vec3> {
    (0..n).map(|k| a.lerp(b, k as f32 / (n - 1) as f32)).collect()
}

/// `n` points from `a` to `b` of a strap `length` long, hanging by its slack (a parabola as deep as
/// `SAG` of a free rope's). Pulled nearly taut, its last slack goes out of it gently (`TAUT`), not
/// all at once, as a hand trembling at the end of it would make it.
fn hanging(a: Vec3, b: Vec3, length: f32, n: usize) -> Vec<Vec3> {
    let chord = a.distance(b);
    let slack = (length - chord).max(0.0);
    let slack = (slack * slack + TAUT * TAUT).sqrt() - TAUT;
    let sag = 0.5 * (slack * (2.0 * chord + slack)).sqrt() * SAG;
    (0..n)
        .map(|k| {
            let t = k as f32 / (n - 1) as f32;
            a.lerp(b, t) - Vec3::Y * sag * 4.0 * t * (1.0 - t)
        })
        .collect()
}

/// A curve laid on the neck (its way along it, for a rope's length): what passes through it put on
/// it (on `side`), smoothed, and laid again; its first point stays (the bit).
fn lay(points: &[Vec3], neck: &[NeckSection], right: Vec3, side: f32) -> Vec<Vec3> {
    let mut p = points.to_vec();
    let n = p.len();
    if neck.len() < 2 || n < 3 {
        return p;
    }
    for pass in 0..=LAY_PASSES {
        for q in p.iter_mut().skip(1) {
            *q = off_neck(*q, neck, right, side);
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

/// The way to a point's side of the neck across `right` (-1, 1), if it's out of the neck; None in
/// it.
fn side_of(p: Vec3, neck: &[NeckSection], right: Vec3) -> Option<f32> {
    let mut nearest = (f32::MAX, 0.0);
    for (c, sx, x, across, y, half_h, half_w, _, rounded) in sections(p, neck) {
        let inside = if rounded { (x / half_w).powi(2) + (y / half_h).powi(2) < 1.0 - 1e-3 } else { y < half_h && x.abs() < across - 1e-4 };
        if inside {
            return None;
        }
        let d = p.distance(c);
        if d < nearest.0 {
            nearest = (d, x * sx.dot(right).signum());
        }
    }
    if nearest.0 == f32::MAX {
        let v = |a: [f32; 4]| Vec3::new(a[0], a[1], a[2]);
        nearest.1 = neck.iter().map(|n| p - v(n.centre)).min_by(|a, b| a.length_squared().total_cmp(&b.length_squared())).map_or(0.0, |d| d.dot(right));
    }
    Some(if nearest.1 >= 0.0 { 1.0 } else { -1.0 })
}

/// `p` put on the neck and head (`CLEAR` off them) if it's in them. Above the ellipse's middle,
/// straight out from its middle to its edge: a strap's points go out where they are, and one
/// crossing to a hand on the other side goes over the top. Below it, straight out sideways to the
/// surface on `side` (across `right`; 0: the nearer), never across. In the rounded ends (past the
/// end sections, round the outside of a bend) the ellipse is all there is, and every point goes
/// straight out from its middle: a strap slides round an end, not into a wall.
fn off_neck(mut p: Vec3, neck: &[NeckSection], right: Vec3, side: f32) -> Vec3 {
    for (_, sx, x, across, y, half_h, half_w, uy, rounded) in sections(p, neck) {
        if rounded {
            let e = (x / half_w).powi(2) + (y / half_h).powi(2);
            if e < 1.0 {
                p += (sx * x + uy * y) * (1.0 / e.sqrt().max(1e-3) - 1.0);
            }
            continue;
        }
        if y >= half_h || x.abs() >= across {
            continue;
        }
        if y > 0.0 {
            let k = 1.0 / ((x / half_w).powi(2) + (y / half_h).powi(2)).sqrt().max(1e-3) - 1.0;
            p += (sx * x + uy * y) * k;
        } else {
            let way = if side != 0.0 { side * sx.dot(right).signum() } else if x >= 0.0 { 1.0 } else { -1.0 };
            p += sx * (way * across - x);
        }
    }
    p
}

/// The neck's cross-sections round `p`: for each pair of them it's between (the two blended), and
/// past the pair's ends within `CAP` (the nearer one shrunk smoothly to nothing, its top kept on the
/// crest: the neck's ends and the outside of its bends rounded off), the middle, the way across,
/// `p` across from the middle, the neck's half width at `p`'s height (`CLEAR` off it: the
/// ellipse's above its middle, all of it below: the throat, and the chest under it, are wider than
/// the ellipse), `p` up from the middle, the half height, the half width, the way up, and whether
/// it's in a rounded end. The neck is the ellipse above its middle and goes straight down below it;
/// a rounded end is the ellipse alone.
#[allow(clippy::type_complexity)]
fn sections(p: Vec3, neck: &[NeckSection]) -> impl Iterator<Item = (Vec3, Vec3, f32, f32, f32, f32, f32, Vec3, bool)> + '_ {
    let v = |a: [f32; 4]| Vec3::new(a[0], a[1], a[2]);
    neck.windows(2).filter_map(move |w| {
        let (ca, cb) = (v(w[0].centre), v(w[1].centre));
        let axis = cb - ca;
        let along = axis.length();
        if along < 1e-4 {
            return None;
        }
        let t = (p - ca).dot(axis) / (along * along);
        let past = if t < 0.0 { -t * along } else { (t - 1.0).max(0.0) * along };
        if past >= CAP {
            return None;
        }
        let s = past / CAP;
        let round = 1.0 - s * s * (3.0 - 2.0 * s);
        let t = t.clamp(0.0, 1.0);
        let (side, up) = (v(w[0].side).lerp(v(w[1].side), t), v(w[0].up).lerp(v(w[1].up), t));
        let c = ca.lerp(cb, t) + up * (1.0 - round);
        let (half_w, half_h) = ((side.length() + CLEAR) * round, (up.length() + CLEAR) * round);
        let (sx, uy) = (side.normalize_or_zero(), up.normalize_or_zero());
        let (x, y) = ((p - c).dot(sx), (p - c).dot(uy));
        let across = if y > 0.0 { half_w * (1.0 - (y / half_h).powi(2)).max(0.0).sqrt() } else { half_w };
        Some((c, sx, x, across, y, half_h, half_w, uy, past > 0.0))
    })
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
    HeldModel::load_strap(gpu, &dir, &model, DEPTH_SLACK)
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
    type Face = ([f32; 2], [f32; 2], [f32; 2], [f32; 3]);
    let faces: [Face; 4] = [
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
