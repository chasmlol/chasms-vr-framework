//! Fingers closing on what the hand holds.
//!
//! A hand holding something it isn't drawn in a game's own pose for wraps its fingers round the
//! thing's surface ([`Grasp`]): each finger bends in from straight, all its joints together, and a
//! segment that meets the surface stops there (with the joints before it) while the joints beyond
//! it carry on, the way a finger folds round a handle or over an edge. A finger that's in the thing
//! straight (a thumb under a palm laid flat on a crate) first bends back at its base until it's
//! out, as far as a finger can. The fingers are the drawn
//! hand's own (its model's joints, lengths and bending axes), so they land on the surface whatever
//! hand the model has. The surface is a few solids in tracking space: the part of the thing the
//! hand has hold of (a handle's capsule), or the whole thing's collision shape.

use glam::{Quat, Vec3, Vec4};

/// One solid of what a hand holds, in tracking space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Solid {
    Sphere { centre: Vec3, radius: f32 },
    /// A handle, a shaft: the segment `a`..`b`, `radius` thick.
    Capsule { a: Vec3, b: Vec3, radius: f32 },
    Box { centre: Vec3, rot: Quat, half: Vec3 },
    /// A convex hull: the planes of its faces, [`Grasp::planes`]`[first..first + count]`.
    Hull { first: u32, count: u32 },
}

/// What a hand's fingers close on this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grasp {
    pub solids: Vec<Solid>,
    /// Hull faces: `n.xyz` the outward normal, `w` with `n · p + w = 0` on the face.
    pub planes: Vec<Vec4>,
    /// The fingers that close on it (bit `f`: thumb 0 .. little 4); the others keep their curls
    /// (a trigger finger on its trigger).
    pub fingers: u8,
    /// How far the fingers have closed on it from their curls: 0 not at all .. 1 on it. It eases in
    /// as the hand takes hold and out as it lets go.
    pub weight: f32,
}

/// Every finger.
pub const ALL_FINGERS: u8 = 0b11111;

impl Solid {
    /// Signed distance from `p` to the surface (negative inside). A hull's is exact inside and on
    /// its faces, and a little short off its edges and corners.
    fn distance(&self, planes: &[Vec4], p: Vec3) -> f32 {
        match *self {
            Solid::Sphere { centre, radius } => p.distance(centre) - radius,
            Solid::Capsule { a, b, radius } => {
                let ab = b - a;
                let t = if ab.length_squared() > 1e-12 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                p.distance(a + ab * t) - radius
            }
            Solid::Box { centre, rot, half } => {
                let q = (rot.inverse() * (p - centre)).abs() - half;
                q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
            }
            Solid::Hull { first, count } => {
                let (first, count) = (first as usize, count as usize);
                planes.get(first..first + count).unwrap_or(&[]).iter().map(|n| n.truncate().dot(p) + n.w).fold(f32::NEG_INFINITY, f32::max)
            }
        }
    }

    /// Moved by `m`: (translation, rotation), applied as `m.0 + m.1 * p`.
    fn moved(&self, m: (Vec3, Quat)) -> Solid {
        let at = |p: Vec3| m.0 + m.1 * p;
        match *self {
            Solid::Sphere { centre, radius } => Solid::Sphere { centre: at(centre), radius },
            Solid::Capsule { a, b, radius } => Solid::Capsule { a: at(a), b: at(b), radius },
            Solid::Box { centre, rot, half } => Solid::Box { centre: at(centre), rot: (m.1 * rot).normalize(), half },
            hull => hull,
        }
    }
}

impl Grasp {
    /// Signed distance from `p` to the nearest solid's surface (infinite with none).
    pub fn distance(&self, p: Vec3) -> f32 {
        self.solids.iter().map(|s| s.distance(&self.planes, p)).fold(f32::INFINITY, f32::min)
    }

    /// The same surface moved by `m` (translation, rotation): `m.0 + m.1 * p`.
    pub fn moved(&self, m: (Vec3, Quat)) -> Grasp {
        let planes = self
            .planes
            .iter()
            .map(|n| {
                let normal = m.1 * n.truncate();
                // A point on the plane, moved, keeps the plane through it.
                normal.extend(n.w - normal.dot(m.0))
            })
            .collect();
        Grasp { solids: self.solids.iter().map(|s| s.moved(m)).collect(), planes, ..*self }
    }
}

/// One finger as the wrap sees it: where each of its segments is for a set of per-joint curls.
pub(crate) trait Chain {
    /// How many segments it has (1..=3).
    fn segments(&self) -> usize;
    /// The segments (start, end) in tracking space for these curls (0 straight .. 1 a fist's;
    /// the base's below 0 bent back).
    fn place(&self, curls: [f32; 3]) -> [(Vec3, Vec3); 3];
    /// How far its base can bend back (a curl, at least 0).
    fn lift(&self) -> f32 {
        0.0
    }
}

/// Steps a finger takes from straight to a fist, and halvings that find where it meets the surface
/// between two steps.
const STEPS: usize = 20;
const HALVINGS: usize = 6;
/// Where along a segment it's checked against the surface (0 its joint, 1 its end).
const SAMPLES: [f32; 6] = [0.1, 0.28, 0.46, 0.64, 0.82, 1.0];
/// How far along a segment from its joint the knuckle's crease reaches: nearer the joint the
/// segment may come that much closer to the surface (it folds round an edge it's on).
const CREASE: f32 = 0.5;
/// Steps a finger in the thing takes bending back out of it.
const LIFT_STEPS: usize = 12;
/// A finger is in the thing when its skin is this much of its thickness under the surface.
const SUNK: f32 = 0.5;

/// Each joint's curl for a finger closed on `grasp`, `radius` thick: bent in from straight, joints
/// stopping from the base out as their segments meet the surface. A finger in the surface when
/// straight bends back at its base until it's out (as far as [`Chain::lift`]); a segment on it
/// then stays there; a finger that meets nothing makes a fist.
pub(crate) fn wrap(chain: &impl Chain, grasp: &Grasp, radius: f32) -> [f32; 3] {
    let n = chain.segments().clamp(1, 3);
    // Segment k's nearest to the surface (its centre line).
    let nearest = |curls: [f32; 3], k: usize| {
        let (a, b) = chain.place(curls)[k];
        SAMPLES.iter().map(|&t| grasp.distance(a.lerp(b, t))).fold(f32::INFINITY, f32::min)
    };
    // Segment k touches: within the finger's thickness of the surface anywhere along it (less
    // near its joint, where the knuckle's crease lets it fold close round an edge).
    let touches = |curls: [f32; 3], k: usize| {
        let (a, b) = chain.place(curls)[k];
        SAMPLES.iter().any(|&t| grasp.distance(a.lerp(b, t)) < radius * (t / CREASE).min(1.0))
    };
    // How deep the finger's skin is under the surface at its deepest.
    let sunk = |curls: [f32; 3]| (0..n).map(|k| radius - nearest(curls, k)).fold(f32::NEG_INFINITY, f32::max);
    // Where it closes from: straight, or bent back out of the thing.
    let mut start = [0.0f32; 3];
    if sunk(start) > SUNK * radius && chain.lift() > 0.0 {
        let mut best = (sunk(start), 0.0);
        for step in 1..=LIFT_STEPS {
            let back = -chain.lift() * step as f32 / LIFT_STEPS as f32;
            let d = sunk([back, 0.0, 0.0]);
            if d < best.0 {
                best = (d, back);
            }
            if d <= SUNK * radius {
                break;
            }
        }
        start[0] = best.1;
    }
    let mut curls = start;
    // Joints 0..held stop where they are.
    let mut held = 0;
    // Already on it, straight.
    while held < n && touches(curls, held) {
        held += 1;
    }
    // The joints from `held` on, `t` of the way from where they start to a fist's.
    let at = |base: [f32; 3], held: usize, t: f32| {
        let mut c = base;
        for k in held..3 {
            c[k] = start[k] + (1.0 - start[k]) * t;
        }
        c
    };
    let mut t = 0.0f32;
    for step in 1..=STEPS {
        if held >= n {
            break;
        }
        let next = step as f32 / STEPS as f32;
        let tried = at(curls, held, next);
        let Some(k) = (held..n).find(|&k| touches(tried, k)) else {
            curls = tried;
            t = next;
            continue;
        };
        // Between `t` (clear) and `next` (on it): close in on where segment k meets the surface.
        let (mut lo, mut hi) = (t, next);
        for _ in 0..HALVINGS {
            let mid = 0.5 * (lo + hi);
            if (held..n).any(|j| touches(at(curls, held, mid), j)) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        curls = at(curls, held, lo);
        t = lo;
        // The joints up to the segment that touched stop; any segment past it that also touches
        // now stops its joints too.
        held = k + 1;
        while held < n && touches(curls, held) {
            held += 1;
        }
    }
    if held < n {
        curls = at(curls, held, 1.0).map(|c| c.min(1.0));
    }
    curls
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A finger of three segments lying along +Y from `base`, bending about +X (towards -Z).
    struct Line {
        base: Vec3,
        lengths: [f32; 3],
    }

    impl Chain for Line {
        fn segments(&self) -> usize {
            3
        }
        fn place(&self, curls: [f32; 3]) -> [(Vec3, Vec3); 3] {
            let full = [1.4, 1.7, 1.1];
            let mut q = Quat::IDENTITY;
            let mut at = self.base;
            std::array::from_fn(|k| {
                q *= Quat::from_rotation_x(-curls[k] * full[k]);
                let end = at + q * Vec3::Y * self.lengths[k];
                let seg = (at, end);
                at = end;
                seg
            })
        }
    }

    fn finger() -> Line {
        Line { base: Vec3::ZERO, lengths: [0.045, 0.028, 0.022] }
    }

    #[test]
    fn a_finger_wraps_a_handle_and_rests_on_it() {
        // A 1.5 cm handle across the finger, 2.5 cm below it and 4 cm out.
        let g = Grasp { solids: vec![Solid::Capsule { a: Vec3::new(-0.1, 0.04, -0.025), b: Vec3::new(0.1, 0.04, -0.025), radius: 0.015 }], fingers: ALL_FINGERS, weight: 1.0, ..Default::default() };
        let r = 0.008;
        let f = finger();
        let curls = wrap(&f, &g, r);
        let segs = f.place(curls);
        // Nothing goes into it, and the finger is on it.
        let closest = segs.iter().flat_map(|&(a, b)| (0..=20).map(move |i| a.lerp(b, i as f32 / 20.0))).map(|p| g.distance(p)).fold(f32::INFINITY, f32::min);
        assert!(closest > r - 0.0015 && closest < r + 0.002, "resting on it: {closest}");
        // The base stopped early and the tip curled on round it.
        assert!(curls[0] < curls[2], "{curls:?}");
    }

    #[test]
    fn a_finger_folds_over_an_edge() {
        // A box whose top is just under the first segment and ends 4 cm out.
        let g = Grasp { solids: vec![Solid::Box { centre: Vec3::new(0.0, 0.0, -0.057), rot: Quat::IDENTITY, half: Vec3::new(0.2, 0.04, 0.05) }], fingers: ALL_FINGERS, weight: 1.0, ..Default::default() };
        let f = finger();
        let curls = wrap(&f, &g, 0.008);
        assert!(curls[0] < 0.05, "the first segment stays flat on top: {curls:?}");
        assert!(curls[1] > 0.3, "the rest bends down over the edge: {curls:?}");
        let (_, tip) = f.place(curls)[2];
        assert!(tip.z < -0.02, "the tip comes down the far side: {tip:?}");
    }

    /// The same finger, its base able to bend back.
    struct Lifting(Line);

    impl Chain for Lifting {
        fn segments(&self) -> usize {
            3
        }
        fn place(&self, curls: [f32; 3]) -> [(Vec3, Vec3); 3] {
            self.0.place(curls)
        }
        fn lift(&self) -> f32 {
            0.5
        }
    }

    #[test]
    fn a_finger_in_the_thing_bends_back_out_onto_it() {
        // A slab whose top is 2 mm under the straight finger's middle: 6 mm of it lies in it.
        let g = Grasp { solids: vec![Solid::Box { centre: Vec3::new(0.0, 0.1, -0.052), rot: Quat::IDENTITY, half: Vec3::new(0.2, 0.2, 0.05) }], fingers: ALL_FINGERS, weight: 1.0, ..Default::default() };
        let r = 0.008;
        let f = Lifting(finger());
        let curls = wrap(&f, &g, r);
        assert!(curls[0] < -0.05, "the base bends back: {curls:?}");
        let deepest = f.place(curls).iter().flat_map(|&(a, b)| (1..=10).map(move |i| a.lerp(b, i as f32 / 10.0))).map(|p| g.distance(p)).fold(f32::INFINITY, f32::min);
        assert!(deepest > r * (1.0 - SUNK) - 1e-3, "out of it, on it: {deepest}");
        // Unable to bend back, it stays as it was (straight, in it).
        assert_eq!(wrap(&f.0, &g, r)[0], 0.0);
    }

    #[test]
    fn nothing_to_hold_makes_a_fist_and_a_moved_grasp_moves() {
        let f = finger();
        assert_eq!(wrap(&f, &Grasp::default(), 0.008), [1.0; 3]);
        let g = Grasp { solids: vec![Solid::Sphere { centre: Vec3::ZERO, radius: 0.1 }], planes: vec![Vec4::new(0.0, 1.0, 0.0, -0.5)], ..Default::default() };
        let m = g.moved((Vec3::new(1.0, 0.0, 0.0), Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)));
        assert!(m.distance(Vec3::new(1.0, 0.0, 0.0)) < -0.09);
        // The plane y = 0.5 turned a quarter about Z and moved 1 along X: x = 0.5.
        let n = m.planes[0];
        assert!((n.truncate().dot(Vec3::new(0.5, 3.0, 0.0)) + n.w).abs() < 1e-5, "{n:?}");
    }
}
