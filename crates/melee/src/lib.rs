//! The melee rules and numbers (`docs/MELEE.md`): how a contact is judged (the damager's angles,
//! the material's minimum speeds), what it does (the damage curves, the material's multiplier, the
//! body part's, the push levels that stagger and knock down), when a blade goes in and how deep,
//! and when an edge cuts a limb off.
//!
//! The runtime judges what the player's hands and weapons did with these ([`judge`]), and reports
//! it; a game takes the hit and applies its own rules, or these ([`damage`], [`body`]) when it has
//! none of its own (the test range).

use glam::Vec3;
use rustvr_protocol::*;

pub mod body;
pub mod modifiers;

pub use modifiers::*;

/// A curve of two keys, as a damager's damage by speed: flat before the first key and after the
/// second, a cubic between them with the keys' slopes (the first's leaving, the second's arriving;
/// zero slopes ease in and out).
pub fn curve(c: [f32; 4], slopes: [f32; 2], x: f32) -> f32 {
    let [x0, y0, x1, y1] = c;
    if x1 <= x0 {
        return y0;
    }
    if x <= x0 {
        return y0;
    }
    if x >= x1 {
        return y1;
    }
    let span = x1 - x0;
    let t = (x - x0) / span;
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * y0 + h10 * slopes[0] * span + h01 * y1 + h11 * slopes[1] * span
}

/// A damager's angle limits by kind (degrees: across the blade against its edge, along it against
/// the blade's plane, the surface's normal): a sword's edge 35, 90, 60; an axe's 45, 80, 80; a
/// point 35, 35, 60. Blunt has none but the normal's, 60.
pub fn angles(b: &MeleeBlade) -> (f32, f32, f32) {
    let or = |v: f32, d: f32| if v > 0.0 { v } else { d };
    let (h, v, n) = match (b.kind, b.flags & DMGF_AXE != 0) {
        (DMG_SLASH, true) => (45.0, 80.0, 80.0),
        (DMG_SLASH, false) => (35.0, 90.0, 60.0),
        (DMG_PIERCE, _) => (35.0, 35.0, 60.0),
        _ => (180.0, 180.0, 60.0),
    };
    (or(b.horizontal, h), or(b.vertical, v), or(b.normal, n))
}

/// The damage table a blade is judged by: its own, or its kind's (a sword's slash, its point's
/// stab, a mace's blunt, a fist's punch).
pub fn modifier_of(b: &MeleeBlade) -> u32 {
    if b.modifier != 0 {
        return b.modifier;
    }
    match b.kind {
        DMG_SLASH if b.flags & DMGF_AXE != 0 => MOD_SLASH_AXE,
        DMG_SLASH => MOD_SLASH,
        DMG_PIERCE => MOD_PIERCE,
        DMG_PUNCH => MOD_PUNCH,
        _ => MOD_BLUNT,
    }
}

/// A blade's damage curve: its own, or its kind's (`SwordSlash1H`'s 2 -> 1, 20 -> 2; a stab's
/// 0 -> 1, 20 -> 10, rising fast; a mace's 2 -> 1, 20 -> 5).
pub fn curve_of(b: &MeleeBlade) -> ([f32; 4], [f32; 2]) {
    if b.curve != [0.0; 4] {
        return (b.curve, b.slopes);
    }
    match b.kind {
        DMG_SLASH if b.flags & DMGF_AXE != 0 => ([2.0, 1.0, 20.0, 3.0], [0.0; 2]),
        DMG_SLASH => ([2.0, 1.0, 20.0, 2.0], [0.0; 2]),
        DMG_PIERCE => (SWORD_PIERCE_CURVE, SWORD_PIERCE_SLOPES),
        _ => ([2.0, 1.0, 20.0, 5.0], [0.0; 2]),
    }
}

/// A sword's point's curve: 0 -> 1 leaving at 0.908 a m/s, 20 -> 10 arriving at 0.0229 (1.88 at
/// 1 m/s, 4.9 at 5, 8.5 at 12); a dagger's point's 0 -> 1 at 0.541, 20 -> 5 at 0.0434.
pub const SWORD_PIERCE_CURVE: [f32; 4] = [0.0, 1.0, 20.0, 10.0];
pub const SWORD_PIERCE_SLOPES: [f32; 2] = [0.908_29, 0.022_876];
pub const DAGGER_PIERCE_CURVE: [f32; 4] = [0.0, 1.0, 20.0, 5.0];
pub const DAGGER_PIERCE_SLOPES: [f32; 2] = [0.541_345, 0.043_378];

/// A damager meeting something, all in one frame (world space).
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// Along the damager, towards its point (`up` in the damager's frame).
    pub along: Vec3,
    /// The way an edge cuts (`forward`); a point's is `along`.
    pub edge: Vec3,
    /// The impact velocity: the damager's at the contact, against what it meets's there.
    pub vel: Vec3,
    /// Out of the surface it meets.
    pub normal: Vec3,
    /// What it meets (`MAT_*`), and the weapon's tier (0..5: common ones are 2).
    pub material: u32,
    pub tier: u32,
    /// Thrown (the damager's throw multiplier, and the thrown push levels).
    pub thrown: bool,
    /// Filled with fire (the fire-imbued rows come first in every table).
    pub fire: bool,
}

/// How a contact was judged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Judged {
    /// The damage it does before the body part's multiplier (the damager's damage curve at the
    /// impact speed, times the damager's tier and the material's multiplier, and thrown,
    /// the damager's throw multiplier).
    pub damage: f32,
    /// What kind of damage it did (a bad angle falls back to blunt).
    pub kind: u32,
    /// `CEF_*`: glancing, penetrated, power (a push level of 2 or more).
    pub flags: u32,
    /// The push level, 0..4 (the highest of the material's thresholds the speed passes).
    pub push: u32,
    /// The speed it hit at.
    pub speed: f32,
    /// It goes in: how deep it may go (metres; the material's cap and the damager's own depth), and
    /// how much the material multiplies the dampers it slides against.
    pub depth: f32,
    pub damper: f32,
}

/// Judges a damager's contact: the angles (an edge must move along its cutting way across the blade
/// and meet the surface square enough; a point must go in point first), the material's minimum
/// speed for the damage type, then the damage, the push level and whether it goes in. A bad angle
/// is a weak blunt hit if the damager falls back to one ([`DMGF_BLUNT_BAD`]: a sword's flat), else
/// nothing.
pub fn judge(b: &MeleeBlade, c: &Contact) -> Option<Judged> {
    let speed = c.vel.length();
    if speed < 1e-3 {
        return None;
    }
    let dir = c.vel / speed;
    let (h, v, n) = angles(b);
    let within = |a: Vec3, b: Vec3, deg: f32| deg >= 180.0 || a.dot(b) >= deg.to_radians().cos() - 1e-6;
    // The normal: the way in against the damager's own way (either way round counts).
    let way = if b.kind == DMG_PIERCE { c.along } else { c.edge };
    let normal_ok = within(-c.normal, way, n) || (b.kind == DMG_SLASH && b.flags & DMGF_BOTH_WAYS != 0 && within(-c.normal, -way, n)) || b.kind != DMG_SLASH && b.kind != DMG_PIERCE;
    let clean = match b.kind {
        DMG_SLASH => {
            // Across the blade (the velocity off its length) against the edge; along it, in the
            // blade's own plane.
            let across = (dir - c.along * dir.dot(c.along)).normalize_or_zero();
            let side = c.along.cross(c.edge).normalize_or_zero();
            let in_plane = (dir - side * dir.dot(side)).normalize_or_zero();
            let ways: &[f32] = if b.flags & DMGF_BOTH_WAYS != 0 { &[1.0, -1.0] } else { &[1.0] };
            ways.iter().any(|&s| {
                let e = c.edge * s;
                within(across, e, h) && within(in_plane, e, v) && (within(-c.normal, e, n))
            })
        }
        DMG_PIERCE => {
            let side = c.along.cross(c.edge).normalize_or(c.along.any_orthonormal_vector());
            let up = side.cross(c.along).normalize_or_zero();
            let h_plane = (dir - up * dir.dot(up)).normalize_or_zero();
            let v_plane = (dir - side * dir.dot(side)).normalize_or_zero();
            within(h_plane, c.along, h) && within(v_plane, c.along, v) && within(-c.normal, c.along, n)
        }
        // A hand (a punch) hits square enough: its way within the normal's angle of the face.
        DMG_PUNCH => within(-c.normal, dir, n),
        _ => normal_ok,
    };
    let (modifier, kind, mut flags, base) = if clean {
        let (cv, slopes) = curve_of(b);
        (modifier_of(b), b.kind, 0, curve(cv, slopes, speed))
    } else if b.flags & DMGF_BLUNT_BAD != 0 && matches!(b.kind, DMG_SLASH | DMG_PIERCE) {
        // The fallback: a light blunt with a fixed damage (a sword's 0.5, a two-hander's 1).
        (MOD_BLUNT_LIGHT, DMG_BLUNT, CEF_GLANCING, BAD_ANGLE_DAMAGE)
    } else {
        return None;
    };
    let row = modifiers::row(modifier, c.material, c.tier, c.fire)?;
    // Fast enough for the material: its minimum, or (a damager that goes in, a table that lets it)
    // the lower of that and the speed it goes in at.
    let pen_ok = clean && b.depth > 0.0 && row.pen_speed.is_finite();
    let needed = if pen_ok { row.min_speed.min(row.pen_speed) } else { row.min_speed };
    if speed < needed {
        return None;
    }
    let tier = if b.tier > 0.0 { b.tier } else { 1.0 };
    let thrown = if c.thrown && b.thrown > 0.0 { b.thrown } else { 1.0 };
    let damage = if speed >= row.min_speed { base * tier * row.damage * thrown } else { 0.0 };
    let push = row.push_level(speed, c.thrown);
    if push >= 2 {
        flags |= CEF_POWER;
    }
    let (depth, damper) = if pen_ok && speed >= row.pen_speed {
        flags |= CEF_PENETRATED;
        (b.depth.min(row.depth_cap), row.damper)
    } else {
        (0.0, 1.0)
    };
    Some(Judged { damage, kind, flags, push, speed, depth, damper })
}

/// A bad angle's blunt damage (a sword's, 0.5).
pub const BAD_ANGLE_DAMAGE: f32 = 0.5;

/// Whether a cut that went in severs the limb it's in, as the melee judges it:
/// the damager cuts limbs off ([`MeleeBlade::dismember`]), the hit
/// is faster than that times the weapon's tier factor (2.5 / 1.5 / 1 / 0.9 / 0.8 / 0.8 for tiers
/// 0..5), within `SLICE_BAND` of the joint, and the swing square enough to the limb (the blade's
/// cutting way within its horizontal limit of square: |cos| < the horizontal angle
/// 35 / 90).
pub fn severs(b: &MeleeBlade, j: &Judged, tier: u32, limb_axis: Vec3, edge: Vec3, from_joint: f32) -> bool {
    if b.dismember <= 0.0 || j.flags & CEF_PENETRATED == 0 || j.kind != DMG_SLASH {
        return false;
    }
    let factor = [2.5, 1.5, 1.0, 0.9, 0.8, 0.8][(tier as usize).min(5)];
    if j.speed <= b.dismember * factor {
        return false;
    }
    from_joint <= SLICE_BAND && limb_axis.normalize_or_zero().dot(edge.normalize_or_zero()).abs() < 35.0 / 90.0
}

/// How near a severable joint (metres) a cut must land to sever it.
pub const SLICE_BAND: f32 = 0.04;
/// What a severed piece flies off with: its speed halved, plus 3 m/s along the limb (3.5 a head).
pub const SLICE_KEEP: f32 = 0.5;
pub const SLICE_PUSH: f32 = 3.0;
pub const SLICE_PUSH_HEAD: f32 = 3.5;
/// A damager that just cut through something can't go in again for this long (s).
pub const SLICE_NO_PENETRATION: f64 = 1.0;
/// The same collider isn't hit again by a damager within this (s).
pub const HIT_DELAY: f64 = 0.1;
/// A contact's effects are as loud and bright as its speed between these (m/s): its intensity,
/// 0..1.
pub const INTENSITY_FROM: f32 = 0.25;
pub const INTENSITY_FULL: f32 = 15.0;

pub fn intensity(speed: f32) -> f32 {
    ((speed - INTENSITY_FROM) / (INTENSITY_FULL - INTENSITY_FROM)).clamp(0.0, 1.0)
}

/// Metal sparks show only above this intensity (about 7.6 m/s).
pub const SPARKS_FROM: f32 = 0.5;

/// The push a point pressed slowly into flesh needs to go in (the pressure curve
/// for swords and daggers: 5 at a standstill, 1.5 drawn at 0.5 m/s, 0.25 at 1 m/s; a velocity change
/// a step, m/s), before the material's pressure multiplier: the faster it's drawn, the less it takes.
pub fn pressure_needed(sliding: f32) -> f32 {
    if sliding <= 0.5 {
        curve([0.0, 5.0, 0.5, 1.5], [0.0, -7.124_971], sliding)
    } else {
        curve([0.5, 1.5, 1.0, 0.25], [-7.124_971, 0.0], sliding)
    }
}
