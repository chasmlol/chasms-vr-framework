//! The range's creatures' animation: where each part of a body would be, which the body's pins pull
//! it towards (`crate::ragdoll`), so the body follows its animation as an active ragdoll.
//!
//! A body's parts hang from each other at their joints (`ragdoll::body`'s layout, a person 1.8 m
//! tall facing +Z with its feet at the origin). A [`Pose`] turns each part about its joint, from
//! its parent; [`Pose::targets`] puts the whole body in the world (where it stands and faces) and
//! gives every part's place and turn there. The arms are posed by where the hands should be
//! ([`arm`]): a two-bone reach from the shoulder, the elbow out and down, the wrist turning the hand
//! (and what it holds) as asked, within its limits.

use glam::{Quat, Vec3};

/// The parts, in `ragdoll::body`'s order.
pub const PELVIS: usize = 0;
pub const TORSO: usize = 1;
pub const HEAD: usize = 2;
pub const UPPER_ARM: [usize; 2] = [3, 6];
pub const FOREARM: [usize; 2] = [4, 7];
pub const HAND: [usize; 2] = [5, 8];
pub const THIGH: [usize; 2] = [9, 11];
pub const SHIN: [usize; 2] = [10, 12];
pub const PARTS: usize = 13;

/// Each part's parent (the pelvis has none).
pub const PARENT: [Option<usize>; PARTS] = [None, Some(PELVIS), Some(TORSO), Some(TORSO), Some(3), Some(4), Some(TORSO), Some(6), Some(7), Some(PELVIS), Some(9), Some(PELVIS), Some(11)];

/// The side of `x`: -1 left (the body's left is -X, facing +Z), 1 right.
pub fn side(hand: usize) -> f32 {
    if hand == 0 { -1.0 } else { 1.0 }
}

/// Where the joints are, standing at rest (person space).
pub fn shoulder(hand: usize) -> Vec3 {
    Vec3::new(0.2 * side(hand), 1.42, 0.0)
}
pub fn elbow(hand: usize) -> Vec3 {
    Vec3::new(0.25 * side(hand), 1.12, 0.0)
}
pub fn wrist(hand: usize) -> Vec3 {
    Vec3::new(0.27 * side(hand), 0.86, 0.03)
}
pub const NECK: Vec3 = Vec3::new(0.0, 1.5, 0.0);
pub const WAIST: Vec3 = Vec3::new(0.0, 1.03, 0.0);
pub fn hip(leg: usize) -> Vec3 {
    Vec3::new(0.1 * side(leg), 0.93, 0.0)
}
pub fn knee(leg: usize) -> Vec3 {
    Vec3::new(0.1 * side(leg), 0.5, 0.0)
}

/// Where each part turns about its parent (person space, at rest).
pub fn pivot(part: usize) -> Vec3 {
    match part {
        PELVIS => WAIST,
        TORSO => WAIST,
        HEAD => NECK,
        3 | 6 => shoulder(if part == 3 { 0 } else { 1 }),
        4 | 7 => elbow(if part == 4 { 0 } else { 1 }),
        5 | 8 => wrist(if part == 5 { 0 } else { 1 }),
        9 | 11 => hip(if part == 9 { 0 } else { 1 }),
        _ => knee(if part == 10 { 0 } else { 1 }),
    }
}

/// The length of an upper arm (shoulder to elbow) and a forearm (elbow to wrist).
pub fn upper_len(hand: usize) -> f32 {
    shoulder(hand).distance(elbow(hand))
}
pub fn fore_len(hand: usize) -> f32 {
    elbow(hand).distance(wrist(hand))
}

/// A body's pose: each part turned about its joint, from its parent (the pelvis from where the body
/// stands), and the pelvis raised or lowered (a crouch) and moved over the feet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub turn: [Quat; PARTS],
    pub pelvis: Vec3,
}

impl Default for Pose {
    fn default() -> Self {
        Pose { turn: [Quat::IDENTITY; PARTS], pelvis: Vec3::ZERO }
    }
}

impl Pose {
    /// Every part's place (its rest centre `rest[i]` carried along) and turn in the world, for a
    /// body standing at `at` (its feet) facing `face`.
    pub fn targets(&self, at: Vec3, face: Quat, rest: &[Vec3; PARTS]) -> [(Vec3, Quat); PARTS] {
        // Each part's transform from its rest place (person space) to where the pose has it.
        let mut xf: [(Vec3, Quat); PARTS] = [(Vec3::ZERO, Quat::IDENTITY); PARTS];
        for i in 0..PARTS {
            let (pp, pq) = match PARENT[i] {
                Some(p) => xf[p],
                None => (self.pelvis, Quat::IDENTITY),
            };
            // Turned about its joint: x -> pivot + turn * (x - pivot), after the parent's.
            let pv = pivot(i);
            let q = (pq * self.turn[i]).normalize();
            let t = pp + pq * pv - q * pv;
            xf[i] = (t, q);
        }
        std::array::from_fn(|i| {
            let (t, q) = xf[i];
            (at + face * (t + q * rest[i]), (face * q).normalize())
        })
    }

    /// Where a joint ends up (person space) under this pose: `p` on part `part`.
    pub fn point(&self, part: usize, p: Vec3) -> Vec3 {
        let mut chain = vec![part];
        while let Some(parent) = PARENT[*chain.last().unwrap()] {
            chain.push(parent);
        }
        let mut t = self.pelvis;
        let mut q = Quat::IDENTITY;
        for &i in chain.iter().rev() {
            let pv = pivot(i);
            let nq = (q * self.turn[i]).normalize();
            t = t + q * pv - nq * pv;
            q = nq;
        }
        t + q * p
    }

    /// The turn part `part` has in person space (its parent's turns and its own).
    pub fn turn_of(&self, part: usize) -> Quat {
        let mut q = self.turn[part];
        let mut at = part;
        while let Some(p) = PARENT[at] {
            q = self.turn[p] * q;
            at = p;
        }
        q.normalize()
    }

    /// Arm `hand` posed so its wrist is at `wrist_at` and the hand turned `hand_rot` (person space,
    /// the hand's own turn from rest), the elbow bending out and down (`swivel` radians round the
    /// shoulder-wrist line from straight down-and-out). Out of reach, it reaches as far as it can.
    pub fn arm(&mut self, hand: usize, wrist_at: Vec3, hand_rot: Quat, swivel: f32) {
        let torso = self.turn_of(TORSO);
        // The shoulder where the torso has it, and the wrist in the torso's own (rest) frame.
        let sh = self.point(UPPER_ARM[hand], shoulder(hand));
        let (lu, lf) = (upper_len(hand), fore_len(hand));
        let to = torso.inverse() * (wrist_at - sh);
        let d = to.length().clamp(0.05, lu + lf - 1e-3);
        let dir = to.normalize_or(Vec3::NEG_Y);
        // The elbow's bend (law of cosines) and how far off the line it sits.
        let cos_u = ((lu * lu + d * d - lf * lf) / (2.0 * lu * d)).clamp(-1.0, 1.0);
        let off = cos_u.acos();
        // The elbow sits off the line towards "down and out", turned `swivel` round it.
        let out = Vec3::new(side(hand), 0.0, 0.0);
        let hint = (Vec3::NEG_Y * 0.7 + out * 0.7 - Vec3::Z * 0.2).normalize();
        let perp = (hint - dir * hint.dot(dir)).try_normalize().unwrap_or_else(|| dir.any_orthonormal_vector());
        let perp = Quat::from_axis_angle(dir, swivel * side(hand)) * perp;
        let upper_dir = (dir * cos_u + perp * off.sin()).normalize();
        let elbow_at = upper_dir * lu;
        let fore_dir = (dir * d - elbow_at).normalize_or(dir);
        // The upper arm hangs along -Y at rest (shoulder to elbow), the forearm likewise.
        let rest_u = (elbow(hand) - shoulder(hand)).normalize();
        let rest_f = (wrist(hand) - elbow(hand)).normalize();
        // Turned so it points along upper_dir, its roll keeping the elbow's hinge across the bend.
        let q_u = aim(rest_u, upper_dir, Vec3::X * side(hand), upper_dir.cross(fore_dir).normalize_or(Vec3::X));
        self.turn[UPPER_ARM[hand]] = q_u;
        // The forearm only bends at the elbow (about its hinge) from the upper arm.
        let local_fore = (q_u.inverse() * fore_dir).normalize();
        self.turn[FOREARM[hand]] = Quat::from_rotation_arc(rest_f, local_fore);
        // The hand: whatever turn is left to reach `hand_rot` (person space), from the forearm.
        let fore_world = (torso * q_u * self.turn[FOREARM[hand]]).normalize();
        let want = torso * hand_rot;
        self.turn[HAND[hand]] = limit((fore_world.inverse() * want).normalize(), WRIST_LIMIT);
    }
}

/// How far a wrist turns from straight (radians).
const WRIST_LIMIT: f32 = 1.3;

/// A turn that takes `from` onto `to` and, about it, `from_side` as near `to_side` as it can.
fn aim(from: Vec3, to: Vec3, from_side: Vec3, to_side: Vec3) -> Quat {
    let q = Quat::from_rotation_arc(from, to);
    let s = q * from_side;
    let s = (s - to * s.dot(to)).normalize_or_zero();
    let t = (to_side - to * to_side.dot(to)).normalize_or_zero();
    if s == Vec3::ZERO || t == Vec3::ZERO {
        return q;
    }
    let angle = s.dot(t).clamp(-1.0, 1.0).acos() * s.cross(t).dot(to).signum();
    (Quat::from_axis_angle(to, angle) * q).normalize()
}

/// `q` turned no further than `max` radians from straight.
fn limit(q: Quat, max: f32) -> Quat {
    let q = if q.w < 0.0 { -q } else { q };
    let (axis, angle) = q.to_axis_angle();
    if angle <= max || !axis.is_finite() { q } else { Quat::from_axis_angle(axis, max) }
}
