//! Body-relative zones: holsters (both hips, both shoulders behind the head), the grenade pouches
//! (either side of the sternum and on the belt, torso-anchored as the research recommends: Pavlov's
//! vest, a Resident Evil VR mod's chest zone, HL:A's wrist pockets) and the magazine pouches
//! (chest and belt front, plus any empty slot).
//!
//! The zones hang off a body anchor whose yaw follows the head with a 0.5 s lag and freezes while
//! a hand is in or approaching a zone, so a slot never slides away mid-reach (the shock2quest fix
//! for drifting hip slots).
//!
//! Holsters are deliberately tight and gated (players complained of guns holstering themselves
//! when let go of near the body): the hips are 13 cm spheres at the belt, and the back slots only
//! count once the hand is behind the head.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use super::filters::alpha;

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

/// Belt height below the eyes for a player with this standing eye height.
fn belt(eye_height: f32) -> f32 {
    -0.55 * (eye_height / 1.70).clamp(0.6, 1.3)
}

/// Holster slots, indexed by `SLOT_*`.
pub fn slot_zones(eye_height: f32, dominant: usize) -> [Zone; SLOT_COUNT_BODY] {
    let belt = belt(eye_height);
    let back = |side: f32| Zone { offset: Vec3::new(0.17 * side, -0.08, -0.12), radius: 0.2, behind: Some(-0.03) };
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

#[derive(Clone, Copy, Debug)]
pub struct BodyAnchor {
    pub yaw: f32,
    pub head: Vec3,
    init: bool,
}

impl Default for BodyAnchor {
    fn default() -> Self {
        BodyAnchor { yaw: 0.0, head: Vec3::ZERO, init: false }
    }
}

impl BodyAnchor {
    /// Follows the head (yaw only), unless `frozen` (a hand is reaching into a zone).
    pub fn update(&mut self, head_pos: Vec3, head_rot: Quat, dt: f32, frozen: bool) {
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
    }

    pub fn rot(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }

    /// A zone's centre in tracking space.
    pub fn zone_center(&self, z: &Zone) -> Vec3 {
        self.head + self.rot() * z.offset
    }

    /// A tracking-space point in the anchor frame.
    pub fn local(&self, p: Vec3) -> Vec3 {
        self.rot().inverse() * (p - self.head)
    }

    pub fn contains(&self, z: &Zone, p: Vec3) -> bool {
        p.distance(self.zone_center(z)) < z.radius && z.behind.is_none_or(|b| self.local(p).z < b)
    }

    /// How a holstered object sits in a slot (tracking space): on the hips barrel down, on the
    /// back diagonally behind the shoulder, barrel up; upright in a chest pouch; point down on the
    /// belt.
    pub fn slot_pose(&self, slot: u32, zone: &Zone) -> (Vec3, Quat) {
        let c = self.zone_center(zone);
        let r = self.rot();
        let local = match slot {
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
        a.update(head, Quat::IDENTITY, 0.011, false);
        a.update(head, Quat::from_rotation_y(1.0), 0.011, false);
        assert!(a.yaw > 0.0 && a.yaw < 0.1, "lags behind a fast head turn: {}", a.yaw);
        for _ in 0..300 {
            a.update(head, Quat::from_rotation_y(1.0), 0.011, false);
        }
        assert!((a.yaw - 1.0).abs() < 0.01, "catches up");
        let before = a.yaw;
        a.update(head, Quat::from_rotation_y(2.0), 0.5, true);
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
        a.update(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY, 0.011, false);
        let z = slot_zones(1.7, 1)[SLOT_BACK_RIGHT as usize];
        assert!(a.contains(&z, Vec3::new(0.18, 1.62, -0.15)), "over the shoulder, behind the head");
        assert!(!a.contains(&z, Vec3::new(0.2, 1.62, 0.05)), "beside the head in front isn't the back");
    }

    #[test]
    fn grenade_pouches_sit_on_the_torso_clear_of_the_magazines_and_holsters() {
        let mut a = BodyAnchor::default();
        a.update(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY, 0.011, false);
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
        a.update(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY, 0.011, false);
        let z = ammo_zones(1.7, 1);
        assert!(a.contains(&z[0], Vec3::new(0.0, 1.38, 0.15)), "chest");
        assert!(a.contains(&z[1], Vec3::new(-0.1, 1.22, 0.15)), "belt front, off-hand side");
        assert!(!z.iter().any(|z| a.contains(z, Vec3::new(0.0, 1.65, 0.15))), "a hand at the face isn't reaching for ammo");
    }
}
