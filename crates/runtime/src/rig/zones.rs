//! Body-relative zones: holsters (both hips, both shoulders behind the head), the grenade pouches
//! (either side of the sternum and on the belt, torso-anchored as the research recommends: Pavlov's
//! vest, a Resident Evil VR mod's chest zone, HL:A's wrist pockets) and the magazine pouches
//! (chest and belt front, plus any empty slot).
//!
//! The zones hang off a body anchor whose yaw follows the head with a 0.5 s lag and freezes while
//! a hand is in or approaching a zone, so a slot never slides away mid-reach (the shock2quest fix
//! for drifting hip slots). They hang from the eyes as they'd be looking level
//! over the neck the head turns on, not from the eyes themselves: looking down at a hip swings the
//! eyes some 10 cm forward and down, and a zone that went with them was out in front of the body,
//! away from the hand reaching for where it was.
//!
//! A zone's middle sits a little out from the body, and a hand counts there or deeper: anywhere
//! from its middle in toward the body's middle line (`DEPTH` at most), so a hand pressed against
//! the body at a slot is in the slot.
//!
//! Holsters are deliberately tight and gated (players complained of guns holstering themselves
//! when let go of near the body): the hips are 13 cm spheres at the belt (and in from them), and
//! the back slots only count once the hand is behind the eyes ([`BEHIND`]).

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use super::filters::alpha;
use crate::settings::Quiver;

/// The over-the-shoulder slots count only with the hand behind this (the anchor frame's z, from
/// the eyes): anywhere beside or behind the head, never in front of the face (food at the mouth).
pub const BEHIND: f32 = 0.0;

#[derive(Clone, Copy, Debug)]
pub struct Zone {
    /// Offset in the anchor frame (x right, y up, z forward) relative to the eyes.
    pub offset: Vec3,
    pub radius: f32,
    /// The hand must also be behind this local z (back slots: behind the head).
    pub behind: Option<f32>,
}

impl Zone {
    const fn new(offset: Vec3, radius: f32) -> Zone {
        Zone { offset, radius, behind: None }
    }
}

/// The eyes to the neck pivot the head turns on (head space, for a player of the reference eye
/// height): the IK body's (`rustvr_ik`'s skeleton: the legacy Oculus neck model's direction, 7.5 cm
/// down and 8.05 cm back, at Parger's 0.13 m).
const EYES_TO_NECK: Vec3 = Vec3::new(0.0, -0.0886, -0.0951);

/// How much deeper than a zone's middle, toward the body's middle line, a hand is still in it (for
/// a player of the reference eye height): from the hips' and the chest's middles, about to the
/// body.
const DEPTH: f32 = 0.12;

/// A player's size against the reference body (eyes 1.70 m up).
pub fn scale(eye_height: f32) -> f32 {
    (eye_height / 1.70).clamp(0.6, 1.3)
}

/// Belt height below the eyes for a player with this standing eye height.
fn belt(eye_height: f32) -> f32 {
    -0.55 * (eye_height / 1.70).clamp(0.6, 1.3)
}

/// Holster slots, indexed by `SLOT_*`.
pub fn slot_zones(eye_height: f32, dominant: usize) -> [Zone; SLOT_COUNT_BODY] {
    let belt = belt(eye_height);
    // (The back slots are the biggest, from above the shoulder down behind it: a hand over the
    // shoulder is out of sight, and the arm's reach behind the head varies the most from player to
    // player.)
    let back = |side: f32| Zone { offset: Vec3::new(0.16 * side, -0.12, -0.12), radius: 0.28, behind: Some(BEHIND) };
    // Grenade pouches high on the chest either side of the sternum (clear of the magazine pouch
    // in its middle), and one on the belt front on the weapon hand's side (the magazine belt
    // pouch is on the other).
    let chest = |side: f32| Zone::new(Vec3::new(0.125 * side, -0.29 * (eye_height / 1.70).clamp(0.6, 1.3), 0.11), 0.085);
    let dom = if dominant == 0 { -1.0 } else { 1.0 };
    [
        Zone::new(Vec3::new(0.19, belt, 0.0), 0.13),  // right hip
        Zone::new(Vec3::new(-0.19, belt, 0.0), 0.13), // left hip
        back(1.0),                                     // over the right shoulder
        back(-1.0),                                    // over the left shoulder
        chest(-1.0),                                   // chest pouch, left
        chest(1.0),                                    // chest pouch, right
        Zone::new(Vec3::new(0.1 * dom, belt + 0.03, 0.15), 0.085), // belt pouch
    ]
}

/// The pouches only take small things (grenades, knives); the hips and the back take anything
/// holsterable.
pub fn slot_fits(slot: u32, d: &ObjectDesc) -> bool {
    match slot {
        SLOT_CHEST_LEFT | SLOT_CHEST_RIGHT | SLOT_BELT => d.kind == OBJ_THROWABLE || d.half_extents.iter().fold(0.0f32, |a, b| a.max(*b)) <= 0.08,
        _ => true,
    }
}

/// Magazine pouches: the chest and the belt front on the off-hand side (empty slots count too, see
/// `Rig::update_hover`).
pub fn ammo_zones(eye_height: f32, dominant: usize) -> [Zone; 2] {
    let off = if dominant == 1 { -1.0 } else { 1.0 };
    [
        Zone::new(Vec3::new(0.0, -0.34 * (eye_height / 1.70).clamp(0.6, 1.3), 0.13), 0.16),
        Zone::new(Vec3::new(0.1 * off, belt(eye_height) + 0.05, 0.13), 0.13),
    ]
}

/// Where a hand takes an arrow from the quiver while the other holds a bow (`string`: the hand
/// taking it): over that hand's shoulder behind the head (where Oblivion wears its quiver, the
/// arrows at the right shoulder), at that hand's hip (for a headset that loses the hand behind the
/// head), or both. The quiver is a set of zones arrows come from, so a holster for arrows can be
/// another.
pub fn quiver_zones(eye_height: f32, string: usize, place: Quiver) -> Vec<Zone> {
    let z = slot_zones(eye_height, string);
    let (back, hip) = if string == 1 { (SLOT_BACK_RIGHT, SLOT_HIP_RIGHT) } else { (SLOT_BACK_LEFT, SLOT_HIP_LEFT) };
    // A little more generous than the holsters: an arrow is reached for often, fast, unseen.
    let shoulder = Zone { radius: 0.22, ..z[back as usize] };
    let hip = Zone { radius: 0.15, ..z[hip as usize] };
    match place {
        Quiver::Shoulder => vec![shoulder],
        Quiver::Hip => vec![hip],
        Quiver::Both => vec![shoulder, hip],
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BodyAnchor {
    pub yaw: f32,
    /// The eyes (tracking space).
    pub head: Vec3,
    /// The eyes as they'd be looking level ahead over the neck they turn on (tracking space): what
    /// the zones hang from, still while the head looks down or tilts.
    pub level: Vec3,
    /// The player's size against the reference body ([`scale`]).
    pub scale: f32,
    init: bool,
}

impl Default for BodyAnchor {
    fn default() -> Self {
        BodyAnchor { yaw: 0.0, head: Vec3::ZERO, level: Vec3::ZERO, scale: 1.0, init: false }
    }
}

impl BodyAnchor {
    /// Follows the head (yaw only), unless `frozen` (a hand is reaching into a zone).
    pub fn update(&mut self, head_pos: Vec3, head_rot: Quat, eye_height: f32, dt: f32, frozen: bool) {
        let f = head_rot * Vec3::Z;
        let head_yaw = f.x.atan2(f.z);
        if !self.init {
            self.yaw = head_yaw;
            self.init = true;
        } else if !frozen {
            let d = crate::wrap_angle(head_yaw - self.yaw);
            self.yaw = crate::wrap_angle(self.yaw + d * alpha(dt, 0.5));
        }
        self.head = head_pos;
        self.scale = scale(eye_height);
        let neck = EYES_TO_NECK * self.scale;
        self.level = head_pos + head_rot * neck - self.rot() * neck;
    }

    pub fn rot(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }

    /// A zone's centre in tracking space.
    pub fn zone_center(&self, z: &Zone) -> Vec3 {
        self.level + self.rot() * z.offset
    }

    /// A tracking-space point in the anchor frame.
    pub fn local(&self, p: Vec3) -> Vec3 {
        self.rot().inverse() * (p - self.level)
    }

    /// How far a tracking-space point is from a zone's reach: the line from its middle in toward
    /// the body's middle line (under the neck, at the zone's height), [`DEPTH`] long at most.
    pub fn distance(&self, z: &Zone, p: Vec3) -> f32 {
        let a = z.offset;
        let inward = Vec3::new(0.0, a.y, EYES_TO_NECK.z * self.scale) - a;
        let b = a + inward.clamp_length_max(DEPTH * self.scale);
        let l = self.local(p);
        let ab = b - a;
        let t = if ab.length_squared() > 1e-8 { ((l - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        l.distance(a + ab * t)
    }

    /// A point is in a zone: at its middle or deeper toward the body, and behind the head where
    /// that's asked.
    pub fn contains(&self, z: &Zone, p: Vec3) -> bool {
        self.distance(z, p) < z.radius && z.behind.is_none_or(|b| self.local(p).z < b)
    }

    /// A point is in a zone's middle, not only deeper (a hand there is in it before a nearer
    /// zone's reach into the body).
    pub fn at_middle(&self, z: &Zone, p: Vec3) -> bool {
        p.distance(self.zone_center(z)) < z.radius && z.behind.is_none_or(|b| self.local(p).z < b)
    }

    /// How a holstered object sits in a slot (tracking space): on the hips barrel down, on the
    /// back diagonally behind the shoulder, barrel up; upright in a chest pouch; point down on the
    /// belt. One that hangs `upright` (a torch) sits at the hip with its +Z up, leaning forward and
    /// out.
    pub fn slot_pose(&self, slot: u32, zone: &Zone, upright: bool) -> (Vec3, Quat) {
        let c = self.zone_center(zone);
        let r = self.rot();
        let local = match slot {
            // (Upright, its top leans forward and out, clear of the body and the face.)
            SLOT_HIP_RIGHT if upright => Quat::from_rotation_z(-0.45) * Quat::from_rotation_x(-1.1),
            SLOT_HIP_LEFT if upright => Quat::from_rotation_z(0.45) * Quat::from_rotation_x(-1.1),
            SLOT_BACK_RIGHT => Quat::from_rotation_z(0.5) * Quat::from_rotation_x(-1.25),
            SLOT_BACK_LEFT => Quat::from_rotation_z(-0.5) * Quat::from_rotation_x(-1.25),
            SLOT_CHEST_LEFT | SLOT_CHEST_RIGHT => Quat::IDENTITY,
            SLOT_BELT => Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            _ => Quat::from_rotation_x(std::f32::consts::FRAC_PI_2 * 0.9),
        };
        (c, r * local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_lags_the_head_and_freezes() {
        let mut a = BodyAnchor::default();
        let head = Vec3::new(0.0, 1.7, 0.0);
        a.update(head, Quat::IDENTITY, 1.7, 0.011, false);
        a.update(head, Quat::from_rotation_y(1.0), 1.7, 0.011, false);
        assert!(a.yaw > 0.0 && a.yaw < 0.1, "lags behind a fast head turn: {}", a.yaw);
        for _ in 0..300 {
            a.update(head, Quat::from_rotation_y(1.0), 1.7, 0.011, false);
        }
        assert!((a.yaw - 1.0).abs() < 0.01, "catches up");
        let before = a.yaw;
        a.update(head, Quat::from_rotation_y(2.0), 1.7, 0.5, true);
        assert_eq!(a.yaw, before, "frozen while reaching into a zone");
    }

    #[test]
    fn hips_sit_below_the_eyes_and_scale_with_height() {
        let z = slot_zones(1.70, 1);
        assert!((z[SLOT_HIP_RIGHT as usize].offset.y + 0.55).abs() < 1e-5);
        let tall = slot_zones(1.90, 1);
        assert!(tall[SLOT_HIP_RIGHT as usize].offset.y < -0.6);
        assert!(z[SLOT_BACK_RIGHT as usize].offset.x > 0.0 && z[SLOT_BACK_LEFT as usize].offset.x < 0.0);
    }

    #[test]
    fn back_slots_need_the_hand_behind_the_head() {
        let mut a = BodyAnchor::default();
        a.update(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY, 1.7, 0.011, false);
        let z = slot_zones(1.7, 1)[SLOT_BACK_RIGHT as usize];
        assert!(a.contains(&z, Vec3::new(0.18, 1.62, -0.15)), "over the shoulder, behind the head");
        assert!(!a.contains(&z, Vec3::new(0.2, 1.62, 0.05)), "beside the head in front isn't the back");
    }

    #[test]
    fn grenade_pouches_sit_on_the_torso_clear_of_the_magazines_and_holsters() {
        let mut a = BodyAnchor::default();
        a.update(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY, 1.7, 0.011, false);
        let z = slot_zones(1.7, 1);
        let ammo = ammo_zones(1.7, 1);
        let (l, r) = (&z[SLOT_CHEST_LEFT as usize], &z[SLOT_CHEST_RIGHT as usize]);
        assert!(a.contains(l, Vec3::new(-0.125, 1.41, 0.11)) && a.contains(r, Vec3::new(0.125, 1.41, 0.11)));
        assert!(!a.contains(l, Vec3::new(0.0, 1.36, 0.13)) && !a.contains(r, Vec3::new(0.0, 1.36, 0.13)), "the sternum is the magazine pouch's");
        assert!(a.contains(&ammo[0], Vec3::new(0.0, 1.36, 0.13)));
        let belt = &z[SLOT_BELT as usize];
        assert!(belt.offset.x > 0.0, "on the weapon hand's side");
        assert!(slot_zones(1.7, 0)[SLOT_BELT as usize].offset.x < 0.0, "left-handed: the left");
        let hip = a.zone_center(&z[SLOT_HIP_RIGHT as usize]);
        assert!(!a.contains(belt, hip) && !a.contains(&z[SLOT_HIP_RIGHT as usize], a.zone_center(belt)), "apart from the hip holster");
        // Only small things go in the pouches.
        let mut d = bytemuck::Zeroable::zeroed();
        let small: &mut ObjectDesc = &mut d;
        small.kind = OBJ_WEAPON;
        small.half_extents = [0.02, 0.07, 0.12];
        assert!(!slot_fits(SLOT_CHEST_LEFT, small) && slot_fits(SLOT_HIP_RIGHT, small), "a pistol goes on the hip");
        small.kind = OBJ_THROWABLE;
        assert!(slot_fits(SLOT_BELT, small), "a knife goes in the belt");
    }

    #[test]
    fn pouches_are_at_the_chest_and_belt_not_the_face() {
        let mut a = BodyAnchor::default();
        a.update(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY, 1.7, 0.011, false);
        let z = ammo_zones(1.7, 1);
        assert!(a.contains(&z[0], Vec3::new(0.0, 1.38, 0.15)), "chest");
        assert!(a.contains(&z[1], Vec3::new(-0.1, 1.22, 0.15)), "belt front, off-hand side");
        assert!(!z.iter().any(|z| a.contains(z, Vec3::new(0.0, 1.65, 0.15))), "a hand at the face isn't reaching for ammo");
    }
}
