//! Where on a thing a hand takes hold of it.
//!
//! Anywhere: a thing taken where it has no handle stays as the hand found it, its surface brought
//! onto the palm, and the fingers close on whatever is there. Near one of its handles it goes into
//! the hand the way it's made to be held:
//! * a gun's pistol grip (`grip`, `grip_rot`) locks the hand on: the gun aims where the hand
//!   points, the index finger on the trigger;
//! * a handle ([`ObjectDesc::grip_line`]: a sword's grip, an axe's haft, a knife's handle) takes
//!   the hand anywhere along it, turned as the thing is held there, or the other way round it (a
//!   reversed grip);
//! * a two-handed gun's handguard ([`ObjectDesc::foregrip_line`]) takes the support hand anywhere
//!   along it, as the gun's support hold (`foregrip`, `support_rot`) slid along.
//!
//! Every hold is the hand's controller grip pose in the thing's space.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

/// How near a gun's pistol grip (metres, from the hand's grip pose to the gun's) a hand that takes
/// the gun must be for the grip to lock it on.
pub const LOCK_REACH: f32 = 0.1;
/// How near a handle's line a hand must take it to go on.
pub const HANDLE_REACH: f32 = 0.07;
/// The pistol grip's length the fingers close round (a hand's width), and a handle's thickness
/// when the game doesn't say.
const GRIP_LENGTH: f32 = 0.09;
const GRIP_RADIUS: f32 = 0.016;
/// A melee weapon's or a knife's handle when the game doesn't say: this far either way of `grip`.
const HANDLE_HALF: f32 = 0.03;
/// A handguard when the game doesn't say: this far either way of `foregrip` along the bore, and its
/// thickness.
const HANDGUARD_HALF: f32 = 0.05;
const HANDGUARD_RADIUS: f32 = 0.02;

/// A place on a thing a hand is made to hold it by (object space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handle {
    /// The hand's hold there: its controller grip pose, before any slide.
    pub at: Vec3,
    pub rot: Quat,
    /// The way the hold slides along the handle (unit), and how far either way of `at` it goes;
    /// 0 both ways: it locks onto `at`.
    pub dir: Vec3,
    pub min: f32,
    pub max: f32,
    /// What the fingers close round: the handle's axis, end to end, and its radius.
    pub axis: (Vec3, Vec3),
    pub radius: f32,
    /// Held the other way round it too (the fist's axis reversed along it).
    pub reversible: bool,
    /// The fingers close round the handle (false: there's no handle, round the thing itself: a
    /// grenade held its way).
    pub grasped: bool,
    /// How near the hand must take it.
    pub reach: f32,
}

impl Handle {
    /// The hold a hand at `pos` turned `rot` (its controller grip pose, object space) takes on this
    /// handle, if it's near enough: the nearest place along it, turned as it's held there (or the
    /// other way round, for a hand that comes at it reversed).
    pub fn take(&self, pos: Vec3, rot: Quat) -> Option<(Vec3, Quat)> {
        let along = (pos - self.at).dot(self.dir).clamp(self.min, self.max);
        let at = self.at + self.dir * along;
        if !(at.distance(pos) <= self.reach) {
            return None;
        }
        let reversed = self.reversible && (rot * Vec3::Z).dot(self.rot * Vec3::Z) < 0.0;
        let rot = if reversed { (self.rot * Quat::from_rotation_x(std::f32::consts::PI)).normalize() } else { self.rot };
        Some((at, rot))
    }

    /// The hold a hand at `pos` turned `rot` takes on this handle closing where it is (a melee
    /// weapon's haft, a second hand on it): the nearest place along it, turned as the hand is turned,
    /// its fist's axis laid along the handle whichever way it already runs. Nothing moves to fit the
    /// hand but the hand itself onto the handle; None if it isn't near enough.
    pub fn take_turned(&self, pos: Vec3, rot: Quat) -> Option<(Vec3, Quat)> {
        let along = (pos - self.at).dot(self.dir).clamp(self.min, self.max);
        let at = self.at + self.dir * along;
        if !(at.distance(pos) <= self.reach) {
            return None;
        }
        let fist = rot * Vec3::Z;
        let axis = if fist.dot(self.dir) >= 0.0 { self.dir } else { -self.dir };
        Some((at, (Quat::from_rotation_arc(fist, axis) * rot).normalize()))
    }

    /// Where along the handle a hand at `pos` would be (clamped to its ends), for a hold sliding along it.
    pub fn slide_to(&self, pos: Vec3) -> Vec3 {
        self.at + self.dir * (pos - self.at).dot(self.dir).clamp(self.min, self.max)
    }

    /// A hold at `at` is on this handle (anywhere along it).
    pub fn holds(&self, at: Vec3) -> bool {
        let along = (at - self.at).dot(self.dir);
        along >= self.min - 1e-3 && along <= self.max + 1e-3 && at.distance(self.at + self.dir * along) < 1e-3
    }
}

fn line(a: [[f32; 3]; 2]) -> Option<(Vec3, Vec3)> {
    let (a, b) = (Vec3::from_array(a[0]), Vec3::from_array(a[1]));
    (a.is_finite() && b.is_finite() && a.distance(b) > 1e-3).then_some((a, b))
}

/// A hold at `at` turned `rot`, sliding along `axis` (whose own ends bound the slide).
fn along(at: Vec3, rot: Quat, axis: (Vec3, Vec3), radius: f32, reversible: bool) -> Handle {
    let dir = (axis.1 - axis.0).normalize();
    let (a, b) = ((axis.0 - at).dot(dir), (axis.1 - at).dot(dir));
    Handle { at, rot, dir, min: a.min(b), max: a.max(b), axis, radius, reversible, grasped: true, reach: HANDLE_REACH }
}

fn radius_or(r: f32, default: f32) -> f32 {
    if r.is_finite() && r > 1e-3 { r } else { default }
}

/// The handle the holding hand takes it by, for hand `hand` (0 left, 1 right): `rot` is the hold's
/// rotation at `grip` for that hand (its rigged `grip_rot`, mirrored for the left; None: not
/// rigged).
pub fn grip(d: &ObjectDesc, rot: Option<Quat>) -> Option<Handle> {
    let at = Vec3::from_array(d.grip);
    let radius = radius_or(d.grip_radius, GRIP_RADIUS);
    if d.kind == OBJ_WEAPON {
        // The pistol grip: locked on. (Unrigged, the fist's axis is taken as up the gun.)
        let rot = rot.unwrap_or(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2));
        let z = rot * Vec3::Z * (0.5 * GRIP_LENGTH);
        return Some(Handle { at, rot, dir: Vec3::Z, min: 0.0, max: 0.0, axis: (at - z, at + z), radius, reversible: false, grasped: true, reach: LOCK_REACH });
    }
    let knife = d.kind == OBJ_THROWABLE && d.throw_flags & THROW_KNIFE != 0;
    let handled = d.kind == OBJ_MELEE || knife;
    if let Some(axis) = line(d.grip_line) {
        let rot = rot.unwrap_or_else(|| fist_along(axis.1 - axis.0));
        return Some(along(at, rot, axis, radius, true));
    }
    let rot = rot?;
    if handled {
        let z = rot * Vec3::Z * HANDLE_HALF;
        return Some(along(at, rot, (at - z, at + z), radius, true));
    }
    // A grenade (or anything else rigged): always held that way, wherever it was taken.
    Some(Handle { at, rot, dir: Vec3::Z, min: 0.0, max: 0.0, axis: (at, at), radius, reversible: false, grasped: false, reach: f32::INFINITY })
}

/// The handle a second hand takes it by while the other holds it: a two-handed gun's handguard
/// (`rot`: the support hold's rotation at `foregrip`, for that hand), or a handle long enough for
/// two hands (the same as [`grip`], `grip_rot` the hold's rotation for that hand).
pub fn foregrip(d: &ObjectDesc, rot: Quat, grip_rot: Option<Quat>) -> Option<Handle> {
    if d.kind == OBJ_WEAPON {
        if d.flags & OBJF_TWO_HANDED == 0 {
            return None;
        }
        let at = Vec3::from_array(d.foregrip);
        let (axis, radius) = match line(d.foregrip_line) {
            Some(axis) => (axis, radius_or(d.foregrip_radius, HANDGUARD_RADIUS)),
            None => ((at - Vec3::Z * HANDGUARD_HALF, at + Vec3::Z * HANDGUARD_HALF), HANDGUARD_RADIUS),
        };
        return Some(Handle { reversible: false, ..along(at, rot, axis, radius, false) });
    }
    grip(d, grip_rot).filter(|h| h.max > h.min)
}

/// A fist round a handle running along `dir` (little finger to thumb), turned any way about it.
fn fist_along(dir: Vec3) -> Quat {
    let z = dir.normalize_or(Vec3::Z);
    let x = z.any_orthonormal_vector();
    Quat::from_mat3(&glam::Mat3::from_cols(x, z.cross(x), z)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn sword() -> ObjectDesc {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_MELEE;
        d.grip = [0.0, 0.0, -0.06];
        d.grip_line = [[0.0, 0.0, -0.11], [0.0, 0.0, -0.012]];
        d.grip_radius = 0.014;
        d
    }

    #[test]
    fn a_handle_takes_the_hand_anywhere_along_it() {
        let rot = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2) * Quat::from_rotation_y(0.0);
        let h = grip(&sword(), Some(rot)).unwrap();
        // Near the pommel, 3 cm off the axis: on the handle there, as it's held.
        let (at, q) = h.take(Vec3::new(0.03, 0.0, -0.1), rot).unwrap();
        assert!(at.distance(Vec3::new(0.0, 0.0, -0.1)) < 1e-5, "{at:?}");
        assert!(q.abs_diff_eq(rot, 1e-5), "{q:?}");
        // Past its end: at the end.
        let (at, _) = h.take(Vec3::new(0.0, 0.0, 0.0), rot).unwrap();
        assert!(at.distance(Vec3::new(0.0, 0.0, -0.012)) < 1e-5, "{at:?}");
        // Too far off it: not on it.
        assert!(h.take(Vec3::new(0.1, 0.0, -0.06), rot).is_none());
        // Come at it the other way round: reversed, the fist's axis the other way along it.
        let back = (rot * Quat::from_rotation_y(std::f32::consts::PI)).normalize();
        let (_, q) = h.take(Vec3::new(0.0, 0.01, -0.05), back).unwrap();
        assert!((q * Vec3::Z).dot(rot * Vec3::Z) < -0.99);
        assert!(h.holds(Vec3::new(0.0, 0.0, -0.05)) && !h.holds(Vec3::new(0.0, 0.0, -0.2)));
    }

    #[test]
    fn a_pistol_grip_locks_and_a_handguard_slides() {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.flags = OBJF_TWO_HANDED;
        d.grip = [0.0, -0.05, 0.0];
        d.foregrip = [0.0, -0.03, 0.4];
        let g = grip(&d, Some(Quat::IDENTITY)).unwrap();
        let (at, _) = g.take(Vec3::new(0.0, 0.0, 0.03), Quat::IDENTITY).unwrap();
        assert_eq!(at, Vec3::new(0.0, -0.05, 0.0), "locked on");
        assert!(g.take(Vec3::new(0.0, 0.0, 0.2), Quat::IDENTITY).is_none(), "the receiver is not the grip");
        let f = foregrip(&d, Quat::IDENTITY, None).unwrap();
        let (at, _) = f.take(Vec3::new(0.0, -0.02, 0.43), Quat::IDENTITY).unwrap();
        assert!(at.distance(Vec3::new(0.0, -0.03, 0.43)) < 1e-5, "slid along: {at:?}");
        let (at, _) = f.take(Vec3::new(0.0, -0.03, 0.5), Quat::IDENTITY).unwrap();
        assert!(at.distance(Vec3::new(0.0, -0.03, 0.45)) < 1e-5, "to the handguard's end: {at:?}");
        // A one-handed gun has no handguard.
        d.flags = 0;
        assert!(foregrip(&d, Quat::IDENTITY, None).is_none());
    }
}
