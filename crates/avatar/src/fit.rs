//! Fitting a game's hand pose to the thing it holds, for close-up viewing.
//!
//! A game poses its first-person hands for its own camera: from there Metro's Kalash hand looks
//! perfect, but up close (VR) its trigger finger is hooked a little too far round the trigger (the
//! pad 5 mm past the blade's far side) and the other fingertips sink half a centimetre into the
//! far side of the grip, where the game's camera never looks. [`fit_hand`] keeps the game's pose and
//! makes the smallest changes that fix that, measured on the pose's own skin (the posed model's
//! mesh):
//!
//! * a hand whose palm and knuckles sank into what it holds may be moved straight out of it, up to
//!   the caller's limit (the range moves a support hand at most 12 mm and keeps the firing hand's
//!   wrist exactly where the game has it);
//! * each finger whose tip (its outer two segments) goes more than `slack` into the surface is
//!   opened just until it no longer does: its three joints unbent together, the middle one most,
//!   or the knuckle most when that frees it sooner (the thumb may fold in instead);
//! * with a trigger, the point of the index finger's last segment nearest to it (its pad, or the
//!   crease of its last joint when the game hooks the finger round the trigger) is put against the
//!   blade, turning the knuckle least; and a pulled pose is solved the same way, that point carried
//!   back with the trigger.
//!
//! Fingers turn only about their own bending axis (each segment's +Z, see [`crate::rig`]) and the
//! thumb about one axis across the palm, so a finger keeps the game's spread and roll. Small,
//! bounded corrections only: what they can't fix (a knuckle the game put inside the gun) is left
//! as the game has it, and reported.

use glam::{Quat, Vec3};
use rustvr_asset as asset;

pub use crate::rig::tip_length;
use crate::rig::{GameHand, HAND, Skeleton, finger_chain};

/// A closed-ish mesh to keep the skin out of: triangles with outward normals.
pub struct Surface {
    tris: Vec<Tri>,
}

#[derive(Clone, Copy)]
struct Tri {
    p: [Vec3; 3],
    n: Vec3,
    lo: Vec3,
    hi: Vec3,
}

impl Surface {
    /// Triangles (positions) with their vertices' normals, which say which side is out.
    pub fn new(tris: impl IntoIterator<Item = ([Vec3; 3], [Vec3; 3])>) -> Surface {
        let tris = tris
            .into_iter()
            .filter_map(|(p, n)| {
                let face = (p[1] - p[0]).cross(p[2] - p[0]);
                let normal = n[0] + n[1] + n[2];
                let face = face.try_normalize()?;
                let n = if face.dot(normal) < 0.0 { -face } else { face };
                Some(Tri { p, n, lo: p[0].min(p[1]).min(p[2]), hi: p[0].max(p[1]).max(p[2]) })
            })
            .collect();
        Surface { tris }
    }

    /// The triangles within the box (lo, hi).
    pub fn near(&self, lo: Vec3, hi: Vec3) -> Surface {
        Surface { tris: self.tris.iter().filter(|t| t.hi.cmpge(lo).all() && t.lo.cmple(hi).all()).copied().collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// How far `p` is inside the surface (0 when outside, or more than `reach` from it).
    ///
    /// Inside means behind the nearest triangles. Game meshes are open in places (a receiver's
    /// underside, a hollow shroud) and have thin parts (a trigger blade), so a point is only taken
    /// to be inside when that's clear: behind the triangle whose face (not an edge) is nearest, or,
    /// nearest to an edge or corner, behind every triangle meeting there (and at least two do: a
    /// lone edge is the rim of an opening).
    pub fn depth(&self, p: Vec3, reach: f32) -> f32 {
        self.inside(p, reach).map_or(0.0, |(d, _)| d)
    }

    /// [`Surface::depth`] and the way out (the nearest triangles' normal), when `p` is inside.
    pub fn inside(&self, p: Vec3, reach: f32) -> Option<(f32, Vec3)> {
        let r = Vec3::splat(reach);
        let mut best = f32::MAX;
        // (distance, side, nearest on the face itself, normal)
        let mut near: Vec<(f32, f32, bool, Vec3)> = Vec::new();
        for t in &self.tris {
            if p.cmplt(t.lo - r).any() || p.cmpgt(t.hi + r).any() {
                continue;
            }
            let (q, on_face) = closest_on_triangle_where(p, &t.p);
            let d = q.distance(p);
            if d > reach || d > best + 2e-4 {
                continue;
            }
            if d < best {
                best = d;
                near.retain(|x| x.0 <= best + 2e-4);
            }
            near.push((d, (p - q).dot(t.n), on_face, t.n));
        }
        let faces: Vec<&(f32, f32, bool, Vec3)> = near.iter().filter(|x| x.2).collect();
        let inside = if !faces.is_empty() { faces.iter().all(|x| x.1 < 0.0) } else { near.len() >= 2 && near.iter().all(|x| x.1 < 0.0) };
        let out = near.iter().fold(Vec3::ZERO, |a, x| a + x.3).normalize_or_zero();
        (inside && !near.is_empty()).then_some((best, out))
    }

    /// The nearest point of the surface to `p`.
    pub fn closest(&self, p: Vec3) -> Option<Vec3> {
        self.tris.iter().map(|t| closest_on_triangle(p, &t.p)).min_by(|a, b| a.distance_squared(p).total_cmp(&b.distance_squared(p)))
    }

    /// The triangles facing within 60 degrees of `dir`.
    pub fn facing(&self, dir: Vec3) -> Surface {
        Surface { tris: self.tris.iter().filter(|t| t.n.dot(dir) > 0.5).copied().collect() }
    }

    /// Distance from `p` to the nearest triangle (`f32::MAX` with none).
    pub fn distance(&self, p: Vec3) -> f32 {
        self.tris.iter().map(|t| closest_on_triangle(p, &t.p).distance(p)).fold(f32::MAX, f32::min)
    }
}

/// The closest point to `p` on triangle `t` (Ericson, Real-Time Collision Detection 5.1.5).
pub fn closest_on_triangle(p: Vec3, t: &[Vec3; 3]) -> Vec3 {
    closest_on_triangle_where(p, t).0
}

/// [`closest_on_triangle`], and whether that point is inside the face (not on an edge or corner).
fn closest_on_triangle_where(p: Vec3, t: &[Vec3; 3]) -> (Vec3, bool) {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, false);
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, false);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return (a + ab * (d1 / (d1 - d3)), false);
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, false);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return (a + ac * (d2 / (d2 - d6)), false);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return (b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))), false);
    }
    let denom = 1.0 / (va + vb + vc);
    (a + ab * (vb * denom) + ac * (vc * denom), true)
}

/// What [`fit_hand`] did.
#[derive(Clone, Debug, Default)]
pub struct FitReport {
    /// How far the whole hand was moved to bring its palm out of the surface (metres).
    pub moved: Vec3,
    /// The palm's deepest skin point inside the surface before and after (metres).
    pub palm_before: f32,
    pub palm_after: f32,
    /// Per finger (thumb .. little): the deepest skin point inside the surface before and after
    /// (metres), and how far its three joints were unbent (radians, + opens).
    pub depth_before: [f32; 5],
    pub depth_after: [f32; 5],
    pub opened: [[f32; 3]; 5],
    /// The index pad's distance from the trigger before and after (metres).
    pub pad_before: Option<f32>,
    pub pad_after: Option<f32>,
    /// The index finger at a full trigger pull: its segments relative to the hand (like
    /// [`GameHand::fingers`]), the point that touches the trigger having moved with it.
    pub index_pulled: Option<[Quat; 3]>,
    /// Where along its last segment (from its pivot, metres) the index touches the trigger.
    pub index_contact: Option<f32>,
}

/// A trigger to put the index finger on: its blade, and how far (and which way) the point the
/// finger touches it at travels at a full pull.
#[derive(Clone, Copy)]
pub struct Trigger<'a> {
    pub blade: &'a Surface,
    pub pull: Vec3,
}

/// How a finger's opening is shared between its joints (knuckle first): mostly the middle joint,
/// so the fingertip comes back out of what it sank into; or, when the finger is in to its middle
/// joint, mostly the knuckle.
const OPEN_SHARES: [[f32; 3]; 2] = [[0.25, 0.45, 0.30], [0.6, 0.25, 0.15]];
/// The most a finger is opened, in total over its joints (radians): the game's pose stays; what
/// this can't fix is left (and reported).
const MAX_OPEN: [f32; 5] = [0.45, 0.8, 0.8, 0.8, 0.8];

/// One finger of the posed model, re-posed by turning its joints.
struct Chain {
    count: usize,
    /// Pivots and canonical frames in the posed model (model space).
    pivots: [Vec3; 3],
    frames: [Quat; 3],
    /// Each joint's bending axis in its segment's frame: turning about it bends the segment
    /// towards the palm (the fingers about their own +Z or -Z, the thumb across the palm).
    axes: [Vec3; 3],
    /// Each skin vertex's position and its weights on the three segments (the rest of its
    /// weight stays on the hand).
    verts: Vec<(Vec3, [f32; 3])>,
}

impl Chain {
    /// Joint `k`'s flexion turn about its bending axis: `+` bends towards the palm.
    fn flex(&self, k: usize, a: f32) -> Quat {
        Quat::from_axis_angle(self.axes[k], a)
    }

    /// The segments' frames and pivots with each joint bent `bend[k]` more.
    fn posed(&self, bend: [f32; 3]) -> ([Quat; 3], [Vec3; 3]) {
        let mut frames = self.frames;
        let mut pivots = self.pivots;
        let mut parent_old = Quat::IDENTITY;
        let mut parent_new = Quat::IDENTITY;
        for k in 0..self.count {
            let local = if k == 0 { self.frames[0] } else { parent_old.inverse() * self.frames[k] };
            let new = if k == 0 { local * self.flex(0, bend[0]) } else { parent_new * local * self.flex(k, bend[k]) };
            if k > 0 {
                let delta = parent_new * parent_old.inverse();
                pivots[k] = pivots[k - 1] + delta * (self.pivots[k] - self.pivots[k - 1]);
            }
            parent_old = self.frames[k];
            parent_new = new.normalize();
            frames[k] = parent_new;
        }
        (frames, pivots)
    }

    /// The finger's skin with each joint bent `bend[k]` more.
    fn skin(&self, bend: [f32; 3]) -> impl Iterator<Item = Vec3> + '_ {
        let (frames, pivots) = self.posed(bend);
        let deltas: [Quat; 3] = [0, 1, 2].map(|k| frames[k] * self.frames[k].inverse());
        self.verts.iter().map(move |(v, w)| {
            let mut p = *v * (1.0 - w[0] - w[1] - w[2]);
            for k in 0..self.count {
                if w[k] > 0.0 {
                    p += (pivots[k] + deltas[k] * (*v - self.pivots[k])) * w[k];
                }
            }
            p
        })
    }

    fn deepest(&self, bend: [f32; 3], surface: &Surface) -> f32 {
        self.skin(bend).map(|p| surface.depth(p, 0.02)).fold(0.0, f32::max)
    }

    /// The deepest point of the finger's outer two segments (what opening it can bring out; its
    /// base sits where the hand puts it).
    fn deepest_tip(&self, bend: [f32; 3], surface: &Surface) -> f32 {
        self.skin(bend).zip(&self.verts).filter(|(_, (_, w))| w[1] + w[2] > 0.5).map(|(p, _)| surface.depth(p, 0.02)).fold(0.0, f32::max)
    }
}

/// Skin vertices carried mostly (over `min`) by `bones`, with their weights on each of them.
fn skin_on(arms: &asset::Model, bones: &[usize], min: f32) -> Vec<(Vec3, [f32; 3])> {
    let mut out = Vec::new();
    for mesh in &arms.meshes {
        for v in &mesh.vertices {
            let total: f32 = v.weights.iter().map(|&w| w as f32).sum::<f32>().max(1.0);
            let mut w = [0.0f32; 3];
            for k in 0..4 {
                if let Some(i) = bones.iter().position(|&b| b == v.bones[k] as usize) {
                    w[i.min(2)] += v.weights[k] as f32 / total;
                }
            }
            if w.iter().sum::<f32>() > min {
                out.push((Vec3::from_array(v.pos), w));
            }
        }
    }
    out
}

fn bounds(points: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    points.fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(p), hi.max(p)))
}

/// The pose `hand` of hand `side` in the posed model `arms` (as [`crate::game_hand`] took it, in
/// the same space as `surface` and `trigger`), fitted to them; see the module docs. `max_move`:
/// how far the whole hand may be moved to bring its palm and knuckles out of the surface (0 keeps
/// the game's wrist).
pub fn fit_hand(arms: &asset::Model, hand: &GameHand, side: usize, surface: &Surface, trigger: Option<Trigger>, slack: f32, max_move: f32) -> (GameHand, FitReport) {
    let skel = Skeleton::new(&arms.bones);
    let mut out = *hand;
    let mut report = FitReport::default();
    let h = hand.wrist.1;
    let Some(hand_bone) = skel.map.joints[HAND[side] as usize] else { return (out, report) };

    // The palm and the fingers' first segments: moved (a little) straight out of what they sank
    // into.
    let mut body: Vec<usize> = vec![hand_bone];
    body.extend((1..5).filter_map(|f| skel.map.fingers[side][f].first().copied()));
    let palm: Vec<Vec3> = skin_on(arms, &body, 0.6).into_iter().map(|(v, _)| v).collect();
    let mut moved = Vec3::ZERO;
    if !palm.is_empty() {
        let (lo, hi) = bounds(palm.iter().copied());
        let near = surface.near(lo - Vec3::splat(0.03), hi + Vec3::splat(0.03));
        let deepest = |t: Vec3| palm.iter().map(|&p| near.depth(p + t, 0.02)).fold(0.0, f32::max);
        report.palm_before = deepest(Vec3::ZERO);
        if report.palm_before > slack && max_move > 0.0 {
            // Straight out along the way out of what it's in (on average), or failing that (a hand
            // round a tube: the ways out cancel) along whichever of 26 directions frees it soonest.
            let mut push = Vec3::ZERO;
            for &p in &palm {
                if let Some((d, way_out)) = near.inside(p, 0.02) {
                    push += way_out * d;
                }
            }
            let mut ways = vec![push.normalize_or_zero()];
            for x in -1..=1 {
                for y in -1..=1 {
                    for z in -1..=1 {
                        if (x, y, z) != (0, 0, 0) {
                            ways.push(Vec3::new(x as f32, y as f32, z as f32).normalize());
                        }
                    }
                }
            }
            let mut best = (report.palm_before, Vec3::ZERO);
            for way in ways.into_iter().filter(|w| w.length_squared() > 0.5) {
                for i in 1..=12 {
                    let t = way * (max_move * i as f32 / 12.0);
                    let d = deepest(t);
                    if d < best.0 - 1e-4 && (best.0 > slack || t.length() < best.1.length()) {
                        best = (d, t);
                    }
                    if d <= slack {
                        break;
                    }
                }
            }
            moved = best.1;
        }
        report.palm_after = deepest(moved);
        // Only worth it if it clearly helps.
        if report.palm_after > report.palm_before * 0.7 {
            moved = Vec3::ZERO;
            report.palm_after = report.palm_before;
        }
        report.moved = moved;
        out.wrist.0 += moved;
    }

    // The palm faces the hand's +X on the right hand, -X on the left.
    let palm_dir = h * if side == 1 { Vec3::X } else { -Vec3::X };
    for f in 0..5 {
        let Some(chain) = finger_chain(&skel.map, side, f, skel.canonical[HAND[side] as usize], &skel.bind) else { continue };
        let frames: [Quat; 3] = [0, 1, 2].map(|k| (h * hand.fingers[f * 3 + k]).normalize());
        // Bending turns a finger's segments about their own +Z (towards the palm: -Z on the right
        // hand); the thumb folds across the palm towards the little finger, about one axis for all
        // three (as the avatar curls it).
        let axes = if f == 0 {
            let axis = (frames[0] * Vec3::Y).cross((palm_dir * 0.6 - h * Vec3::Z).normalize()).normalize_or(h * Vec3::Z);
            frames.map(|q| q.inverse() * axis)
        } else {
            [if side == 1 { -Vec3::Z } else { Vec3::Z }; 3]
        };
        let c = Chain {
            count: chain.count,
            pivots: chain.pivots.map(|p| p + moved),
            frames,
            axes,
            verts: skin_on(arms, &skel.map.fingers[side][f], 0.3).into_iter().map(|(v, w)| (v + moved, w)).collect(),
        };
        if c.verts.is_empty() {
            continue;
        }
        // Only the surface this finger could reach.
        let (lo, hi) = bounds(c.verts.iter().map(|v| v.0));
        let near = surface.near(lo - Vec3::splat(0.04), hi + Vec3::splat(0.04));
        let mut bend = [0.0f32; 3];
        report.depth_before[f] = c.deepest(bend, &near);
        let tip_before = c.deepest_tip(bend, &near);
        if f == 1
            && let Some(trigger) = trigger.filter(|t| !t.blade.is_empty())
        {
            let pulled;
            (bend, pulled) = put_pad_on(&c, trigger, &mut report);
            let (frames, _) = c.posed(pulled);
            report.index_pulled = Some(frames.map(|q| (h.inverse() * q).normalize()));
        } else if tip_before > slack {
            // The least opening that brings the fingertip out (the thumb may need to fold in
            // instead): or, if nothing within reach does, the one that gets it furthest out.
            let directions: &[f32] = if f == 0 { &[-1.0, 1.0] } else { &[-1.0] };
            let mut best = (tip_before, 0.0f32, [0.0f32; 3]);
            let ways = directions.iter().flat_map(|&d| OPEN_SHARES.iter().map(move |s| (d, s)));
            'search: for (dir, share) in ways {
                let steps = 40;
                for i in 1..=steps {
                    let t = MAX_OPEN[f] * i as f32 / steps as f32;
                    let b = share.map(|s| dir * s * t);
                    let d = c.deepest_tip(b, &near);
                    if d <= slack {
                        if best.0 > slack || t < best.1 {
                            best = (d, t, b);
                        }
                        continue 'search;
                    }
                    if best.0 > slack && d < best.0 - 1e-4 {
                        best = (d, t, b);
                    }
                }
            }
            // Turning that hardly helps isn't kept.
            if best.0 < tip_before * 0.8 {
                bend = best.2;
            }
        }
        report.depth_after[f] = c.deepest(bend, &near);
        report.opened[f] = bend.map(|b| -b);
        let (frames, _) = c.posed(bend);
        for k in 0..c.count {
            out.fingers[f * 3 + k] = (h.inverse() * frames[k]).normalize();
        }
    }
    (out, report)
}

/// Bends the index's three joints (as little as possible) so its pad rests on the trigger: on the
/// point of the blade nearest to where the game has the pad. Returns that, and the bend at a full
/// pull (the contact carried back with the trigger).
fn put_pad_on(c: &Chain, trigger: Trigger, report: &mut FitReport) -> ([f32; 3], [f32; 3]) {
    let none = ([0.0; 3], [0.0; 3]);
    let blade = trigger.blade;
    let last = c.count - 1;
    // Where the finger meets the trigger: the point of the last segment's palm side (the way it
    // bends) nearest to the trigger in the game's pose. That's the pad when the game rests the pad
    // on the trigger, the crease of the last joint when it hooks the finger round it.
    let palm = c.axes[last].cross(Vec3::Y).normalize_or(Vec3::X);
    let local: Vec<Vec3> = c.verts.iter().filter(|(_, w)| w[last] > 0.5).map(|(v, _)| c.frames[last].inverse() * (*v - c.pivots[last])).collect();
    let world = |u: Vec3| c.pivots[last] + c.frames[last] * u;
    let gap = |u: &Vec3| blade.closest(world(*u)).map_or(f32::MAX, |t| t.distance(world(*u)));
    let Some(pad_local) = local.iter().copied().filter(|u| u.dot(palm) > 0.0).min_by(|a, b| gap(a).total_cmp(&gap(b))) else { return none };
    let pad_at = |bend: [f32; 3]| {
        let (frames, pivots) = c.posed(bend);
        pivots[last] + frames[last] * pad_local
    };
    let Some(first) = blade.closest(pad_at([0.0; 3])) else { return none };
    report.pad_before = Some(pad_at([0.0; 3]).distance(first));
    report.index_contact = Some(pad_local.y);
    // Least squares on the pad's miss plus a little for every radian turned, by Levenberg-Marquardt
    // over the joints; the target follows the pad over the face. Turning the knuckle costs most
    // (it swings the whole finger off the frame), the last joint least (a game's trigger finger is
    // usually just hooked too far round the trigger).
    const REG: Vec3 = Vec3::new(0.02, 0.008, 0.003);
    // The bend from `from` that brings the contact to `target`, changing it as little as possible.
    let solve = |from: [f32; 3], target: Vec3| -> [f32; 3] {
        let residual = |b: [f32; 3]| (pad_at(b) - target, (Vec3::from_array(b) - Vec3::from_array(from)) * REG);
        let cost = |b: [f32; 3]| {
            let (r, g) = residual(b);
            r.length_squared() + g.length_squared()
        };
        let mut b = from;
        let mut mu = 1e-4f32;
        for _ in 0..60 {
            let (r, _) = residual(b);
            let base = cost(b);
            let mut cols = [Vec3::ZERO; 3];
            for k in 0..c.count {
                let mut e = b;
                e[k] += 1e-3;
                cols[k] = (pad_at(e) - pad_at(b)) / 1e-3;
            }
            let j = glam::Mat3::from_cols(cols[0], cols[1], cols[2]);
            let jt = j.transpose();
            let a = jt * j + glam::Mat3::from_diagonal(REG * REG + Vec3::splat(mu));
            let rhs = -(jt * r) - (Vec3::from_array(b) - Vec3::from_array(from)) * (REG * REG);
            if a.determinant().abs() < 1e-18 {
                break;
            }
            let step = a.inverse() * rhs;
            let mut next = b;
            for k in 0..c.count {
                // (Unbent at most 20 degrees past the game's pose: no hyperextended joints.)
                next[k] = (b[k] + step[k]).clamp(-0.35, 1.2);
            }
            if cost(next) < base {
                b = next;
                mu *= 0.3;
            } else {
                mu *= 10.0;
            }
            if step.length() < 1e-5 {
                break;
            }
        }
        b
    };
    let mut b = [0.0f32; 3];
    let mut target = first;
    for _round in 0..4 {
        b = solve(b, target);
        match blade.closest(pad_at(b)) {
            Some(t) if t.distance(target) > 1e-4 => target = t,
            _ => break,
        }
    }
    report.pad_after = blade.closest(pad_at(b)).map(|t| t.distance(pad_at(b)));
    // A full pull: the contact goes back with the trigger.
    let pulled = solve(b, pad_at(b) + trigger.pull);
    (b, pulled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube from -0.5 to 0.5 as a surface.
    fn cube() -> Surface {
        let mut tris = Vec::new();
        for axis in 0..3 {
            for s in [-1.0f32, 1.0] {
                let n = Vec3::from_array(std::array::from_fn(|i| if i == axis { s } else { 0.0 }));
                let (u, v) = (Vec3::from_array(std::array::from_fn(|i| if i == (axis + 1) % 3 { 0.5 } else { 0.0 })), Vec3::from_array(std::array::from_fn(|i| if i == (axis + 2) % 3 { 0.5 } else { 0.0 })));
                let c = n * 0.5;
                let q = [c - u - v, c + u - v, c + u + v, c - u + v];
                tris.push(([q[0], q[1], q[2]], [n; 3]));
                tris.push(([q[0], q[2], q[3]], [n; 3]));
            }
        }
        Surface::new(tris)
    }

    /// A box from `lo` to `hi` as a surface.
    fn block(lo: Vec3, hi: Vec3) -> Surface {
        let c = cube();
        let (size, mid) = (hi - lo, (lo + hi) * 0.5);
        Surface { tris: c.tris.iter().map(|t| {
            let p = t.p.map(|p| mid + p * size);
            Tri { p, n: t.n, lo: p[0].min(p[1]).min(p[2]), hi: p[0].max(p[1]).max(p[2]) }
        }).collect() }
    }

    /// The mannequin as a posed model (its bind pose: arms out, palms down, fingers straight).
    fn mannequin_model() -> asset::Model {
        let m = crate::procedural::mannequin();
        let meshes = m.meshes.into_iter().map(|s| asset::Mesh { texture: String::new(), vertices: s.vertices, indices: s.indices, name: s.name, material: Default::default() }).collect();
        asset::Model { version: asset::FORMAT_VERSION, bones: m.bones, meshes, ..Default::default() }
    }

    /// The tip of the right index finger: its last segment's skin, lowest point (palm side).
    fn index_tip(model: &asset::Model) -> (Vec3, f32) {
        let skel = Skeleton::new(&model.bones);
        let last = *skel.map.fingers[1][1].last().unwrap();
        let pts: Vec<Vec3> = skin_on(model, &[last], 0.6).into_iter().map(|(v, _)| v).collect();
        let centre = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
        // (Under the middle of the segment.)
        let low = pts.iter().filter(|p| (p.x - centre.x).abs() < 0.01).map(|p| p.y).fold(f32::MAX, f32::min);
        (centre, low)
    }

    #[test]
    fn a_fingertip_in_a_block_comes_out_and_the_index_finds_a_trigger() {
        let model = mannequin_model();
        let hand = crate::game_hand(&model.bones, None, 1).unwrap();
        let (tip, low) = index_tip(&model);
        // A block under the right index fingertip, 3 mm into its skin (the palm faces down).
        let top = low + 0.003;
        let under = block(Vec3::new(tip.x - 0.03, top - 0.05, tip.z - 0.006), Vec3::new(tip.x + 0.03, top, tip.z + 0.006));
        let (fitted, report) = fit_hand(&model, &hand, 1, &under, None, 0.001, 0.0);
        assert!(report.depth_before[1] > 0.0025, "{report:?}");
        assert!(report.depth_after[1] <= 0.0011, "opened out: {report:?}");
        assert!(report.opened[1].iter().all(|&a| a >= 0.0) && report.opened[1][1] > 0.0, "{report:?}");
        assert_eq!(fitted.fingers[6..], hand.fingers[6..], "the other fingers stay");
        assert_eq!(fitted.wrist, hand.wrist, "the wrist stays");
        // A trigger post 6 mm under the pad: the index goes down to it, and gets a pulled pose.
        let top = low - 0.006;
        let blade = block(Vec3::new(tip.x - 0.003, top - 0.03, tip.z - 0.004), Vec3::new(tip.x + 0.003, top, tip.z + 0.004));
        let trigger = Trigger { blade: &blade, pull: Vec3::new(-0.008, 0.0, 0.0) };
        let (_, report) = fit_hand(&model, &hand, 1, &Surface { tris: Vec::new() }, Some(trigger), 0.001, 0.0);
        let (before, after) = (report.pad_before.unwrap(), report.pad_after.unwrap());
        assert!(before > 0.005 && after < before * 0.6, "{report:?}");
        assert!(report.opened[1].iter().all(|&a| a <= 0.351), "no joint unbent past 20 degrees: {report:?}");
        assert!(report.index_pulled.is_some());
    }

    #[test]
    fn depth_is_inside_only() {
        let c = cube();
        assert!((c.depth(Vec3::new(0.45, 0.0, 0.0), 0.2) - 0.05).abs() < 1e-5);
        assert_eq!(c.depth(Vec3::new(0.55, 0.0, 0.0), 0.2), 0.0);
        // Just past a corner: outside, though it's behind some faces.
        assert_eq!(c.depth(Vec3::new(0.52, 0.52, 0.0), 0.2), 0.0);
        assert!((c.depth(Vec3::new(0.48, 0.47, 0.1), 0.2) - 0.02).abs() < 1e-5);
        assert_eq!(c.depth(Vec3::ZERO, 0.2), 0.0, "deeper than the reach reads as outside");
    }
}
