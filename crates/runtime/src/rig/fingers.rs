//! The fingers as colliders: five capsules on each physics hand, so a hand brushes and pushes
//! loose things (a hanging cable, a bottle) without grabbing them.
//!
//! Each finger is one straight capsule from its knuckle to its tip, in the hand's canonical frame
//! (`rustvr_ik`: +Y along the fingers, +Z on the thumb side; the palm faces +X on the right hand,
//! -X on the left), placed on the hand joint the same offsets put the drawn hand on. A curl turns
//! the finger in towards the palm and shortens it to the chord a bent finger spans. The sizes are
//! an adult hand's, the thumb's as the drawn hands' (Metro's): out in front of the palm, folding
//! across it towards the little finger. The drawn hands' own vary by model; these only decide what
//! the fingers touch.

use glam::{Quat, Vec3};

/// Capsule radius (a finger's half thickness).
pub const RADIUS: f32 = 0.009;

/// Thumb, index, middle, ring, pinky: (knuckle in the right hand's frame, the left's its mirror
/// image; straight direction; length).
const FINGERS: [([f32; 3], [f32; 3], f32); 5] = [
    ([0.004, 0.025, 0.015], [0.28, 0.76, 0.58], 0.10),
    ([0.0, 0.095, 0.026], [0.0, 1.0, 0.0], 0.075),
    ([0.0, 0.098, 0.008], [0.0, 1.0, 0.0], 0.085),
    ([0.0, 0.095, -0.010], [0.0, 1.0, 0.0], 0.08),
    ([0.0, 0.088, -0.027], [0.0, 1.0, 0.0], 0.065),
];
/// The palm: a box from the wrist to the knuckles, as wide as the knuckles and as thick as a hand
/// (its centre in the hand frame, its half sizes along the frame's axes). The fist's grip axis runs
/// just off its face, so what the hand holds there sits in it.
const PALM: ([f32; 3], [f32; 3]) = ([0.0, 0.05, 0.0], [0.014, 0.046, 0.038]);

/// The palm's box for a hand joint at `hand` (position, the canonical frame's rotation): its
/// centre, rotation and half sizes.
pub fn palm(hand: (Vec3, Quat)) -> (Vec3, Quat, Vec3) {
    let (c, half) = PALM;
    (hand.0 + hand.1 * Vec3::from_array(c), hand.1, Vec3::from_array(half))
}

/// How far each finger bends back past straight at most, pressed flat on something (a curl): the
/// thumb a long way (it lies back beside the hand).
pub const BACK: [f32; 5] = [1.2, 0.35, 0.35, 0.35, 0.35];

/// How far a finger curled `curl` (0 straight .. 1 a fist's) is pushed open by what it's pressed on,
/// at most: a relaxed finger all the way (and back past straight), a clenched one only a little.
pub fn give(curl: f32) -> f32 {
    let c = curl.clamp(0.0, 1.0);
    1.2 * (1.0 - 0.75 * c * c)
}

/// How a full curl turns a finger (the thumb less), radians; and a curl back past straight (at
/// the knuckle, as the drawn hand's).
const CURL: [f32; 5] = [1.0, 1.9, 1.9, 1.9, 1.9];
const CURL_BACK: [f32; 5] = [0.45, 1.3, 1.3, 1.3, 1.3];
/// How much shorter a full curl makes a finger's span (the thumb's hardly).
const SHORTEN: [f32; 5] = [0.15, 0.35, 0.35, 0.35, 0.35];

/// The five fingers' segments (knuckle, tip) in the world, for a hand joint at `hand` (position,
/// the canonical frame's rotation) on the `right` hand or the left, with each finger's curl (0
/// straight .. 1 a fist; down to -[`BACK`] bent back).
pub fn segments(hand: (Vec3, Quat), right: bool, curls: [f32; 5]) -> [(Vec3, Vec3); 5] {
    let (pos, rot) = hand;
    let palm = if right { Vec3::X } else { Vec3::NEG_X };
    let side = |v: [f32; 3]| if right { Vec3::from_array(v) } else { Vec3::new(-v[0], v[1], v[2]) };
    std::array::from_fn(|i| {
        let (k, d, len) = FINGERS[i];
        let c = curls[i].clamp(-BACK[i], 1.0);
        let straight = side(d).normalize();
        // Turned in towards the palm about the axis across the finger (the thumb across it,
        // towards the little finger, more than into it).
        let toward = if i == 0 { (palm * 0.6 - Vec3::Z).normalize() } else { palm };
        let axis = straight.cross(toward).normalize_or(Vec3::Z);
        let turn = if c >= 0.0 { c * CURL[i] } else { c * CURL_BACK[i] };
        let dir = Quat::from_axis_angle(axis, turn) * straight;
        let chord = len * (1.0 - SHORTEN[i] * c.max(0.0));
        let a = pos + rot * side(k);
        (a, a + rot * (dir * chord))
    })
}

/// A capsule's place for a segment: its centre and the rotation taking +Y along it, and its half
/// height (the straight part).
pub fn capsule_pose(a: Vec3, b: Vec3) -> (Vec3, Quat, f32) {
    let d = b - a;
    let rot = Quat::from_rotation_arc(Vec3::Y, d.normalize_or(Vec3::Y));
    ((a + b) * 0.5, rot, (d.length() * 0.5 - RADIUS).max(0.002))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_fingers_reach_ahead_and_a_fist_curls_into_the_palm() {
        let hand = (Vec3::ZERO, Quat::IDENTITY);
        let open = segments(hand, true, [0.0; 5]);
        // The middle finger's tip: knuckle 9.8 cm out, plus its 8.5 cm.
        assert!((open[2].1 - Vec3::new(0.0, 0.183, 0.008)).length() < 1e-4, "{:?}", open[2].1);
        let fist = segments(hand, true, [1.0; 5]);
        for i in 1..5 {
            assert!(fist[i].1.y < open[i].1.y - 0.05, "finger {i} curls back");
            assert!(fist[i].1.x > 0.03, "towards the right palm (+X): {:?}", fist[i].1);
        }
        let left = segments(hand, false, [1.0; 5]);
        assert!(left[2].1.x < -0.03, "the left palm faces -X: {:?}", left[2].1);
    }

    #[test]
    fn a_capsule_spans_its_segment() {
        let (c, q, h) = capsule_pose(Vec3::ZERO, Vec3::new(0.0, 0.0, 0.1));
        assert!((c - Vec3::new(0.0, 0.0, 0.05)).length() < 1e-6);
        assert!((q * Vec3::Y - Vec3::Z).length() < 1e-5);
        assert!((h - (0.05 - RADIUS)).abs() < 1e-6);
    }
}
