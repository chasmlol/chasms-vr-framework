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
    // (The limits are strict: under the angle.)
    let within = |a: Vec3, b: Vec3, deg: f32| deg >= 180.0 || a.dot(b) > deg.to_radians().cos();
    // The normal: the way in against the damager's own way, either way round (against its forward
    // or its backward).
    let way = if b.kind == DMG_PIERCE { c.along } else { c.edge };
    let normal_ok = within(-c.normal, way, n) || within(-c.normal, -way, n);
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
                within(across, e, h) && within(in_plane, e, v) && normal_ok
            })
        }
        DMG_PIERCE => {
            let side = c.along.cross(c.edge).normalize_or(c.along.any_orthonormal_vector());
            let up = side.cross(c.along).normalize_or_zero();
            let h_plane = (dir - up * dir.dot(up)).normalize_or_zero();
            let v_plane = (dir - side * dir.dot(side)).normalize_or_zero();
            within(h_plane, c.along, h) && within(v_plane, c.along, v) && normal_ok
        }
        // A blunt damager (a mace's head, a pommel, a fist) hits from any way at all.
        _ => true,
    };
    let (modifier, kind, mut flags, base) = if clean {
        let (cv, slopes) = curve_of(b);
        (modifier_of(b), b.kind, 0, curve(cv, slopes, speed))
    } else if b.flags & DMGF_BLUNT_BAD != 0 && matches!(b.kind, DMG_SLASH | DMG_PIERCE) {
        // The fallback: a blunt blow of a fixed damage by the damager's own table (a sword's
        // light blunt 0.5, a dagger's very light 0.5, a two-hander's blunt 1, an axe's blunt 2).
        let (modifier, damage) = bad_angle(b);
        (modifier, DMG_BLUNT, CEF_GLANCING, damage)
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

/// A bad angle's blunt damage where the blade gives none (a sword's, 0.5).
pub const BAD_ANGLE_DAMAGE: f32 = 0.5;

/// The table a blade's bad-angle blow is judged by and its damage: its own, or a sword's light
/// blunt 0.5.
pub fn bad_angle(b: &MeleeBlade) -> (u32, f32) {
    if b.bad_modifier != 0 { (b.bad_modifier, if b.bad_damage > 0.0 { b.bad_damage } else { BAD_ANGLE_DAMAGE }) } else { (MOD_BLUNT_LIGHT, BAD_ANGLE_DAMAGE) }
}

/// How loud and bright a blow's effects are, 0..1: by the damager's own speeds after a blow that
/// did damage ([`MeleeBlade::intensity`]), else the contact's (0.25 to 15).
pub fn blow_intensity(b: &MeleeBlade, speed: f32, damaging: bool) -> f32 {
    let [lo, hi] = b.intensity;
    if damaging && hi > lo { ((speed - lo) / (hi - lo)).clamp(0.0, 1.0) } else { intensity(speed) }
}

/// Where a part of a human comes off: its cut lies `height` metres along its way from its joint,
/// and that way takes `adjust` of its parent's (0 its own, 1 its parent's). `None`: it never comes
/// off (the torso; a thigh). The protocol's kinds with their parents' tell the parts apart: an arm
/// hanging from the torso is an upper arm, from an arm a forearm; a leg from the torso a thigh,
/// from a leg a shin. A head comes off by its neck: the neck is cut 2.8 cm up from its base and
/// the head goes, and a creature's head part here starts at the neck.
pub fn slicing(kind: u32, parent: Option<u32>) -> Option<(f32, f32)> {
    match (kind, parent) {
        (PART_HEAD, _) => Some((0.028, 0.0)),
        (PART_ARM, Some(PART_TORSO)) => Some((0.0, 0.35)),
        (PART_ARM, _) | (PART_HAND, _) => Some((0.0, 0.5)),
        (PART_LEG, Some(PART_LEG)) => Some((0.0, 0.5)),
        // 3 cm up the shin from the ankle, square to the shin.
        (PART_FOOT, _) => Some((-0.03, 1.0)),
        _ => None,
    }
}

/// The part each of a creature's parts hangs from (`parts`: each capsule's ends, radius and kind;
/// `a` the end it hangs from): the part nearest its joint of those a part of its kind can hang from
/// (a head, an arm or a leg from the torso; a forearm from an arm, a hand from an arm, a shin from a
/// leg, a foot from a leg).
pub fn parents(parts: &[(Vec3, Vec3, f32, u32)]) -> Vec<Option<usize>> {
    parts
        .iter()
        .enumerate()
        .map(|(i, &(a, _, _, kind))| {
            let from: &[u32] = match kind {
                PART_HEAD | PART_TAIL => &[PART_TORSO],
                PART_ARM => &[PART_TORSO, PART_ARM],
                PART_HAND => &[PART_ARM],
                PART_LEG => &[PART_TORSO, PART_LEG],
                PART_FOOT => &[PART_LEG],
                _ => &[],
            };
            parts
                .iter()
                .enumerate()
                .filter(|&(j, p)| j != i && from.contains(&p.3))
                .map(|(j, &(pa, pb, ..))| (j, a.distance(nearest_on(pa, pb, a))))
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(j, _)| j)
        })
        .collect()
}

/// The point of segment `a`..`b` nearest `p`.
fn nearest_on(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-9 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

/// Where part `i`'s joint is. A part that carries its parent's line on (a forearm, a hand, a shin, a
/// foot, the head on the spine) meets it halfway between its `a` and its parent's `b` (a game's
/// capsules may stop short of the joint, or run on past it to cover what's drawn there); one at its
/// parent's side (an arm, a leg on the torso), and one with no parent, at its `a`.
fn joint_of(parts: &[(Vec3, Vec3, f32, u32)], i: usize, parent: Option<usize>) -> Vec3 {
    let (a, _, _, kind) = parts[i];
    match parent.map(|p| (parts[p].1, parts[p].3)) {
        Some((pb, pk)) if matches!((kind, pk), (PART_ARM | PART_HAND, PART_ARM) | (PART_LEG | PART_FOOT, PART_LEG) | (PART_HEAD, PART_TORSO)) => (a + pb) * 0.5,
        _ => a,
    }
}

/// Which part a cut that went in takes off, as the melee decides it:
/// the damager cuts limbs off ([`MeleeBlade::dismember`]), faster than that
/// times its factor at the weapon's tier ([`MeleeBlade::dismember_tiers`]), and the cut fits a part
/// that comes off ([`slicing`]): the part hit (`hit`), else its parent, else one of its children.
/// It fits within [`SLICE_WIDTH`] of the part's cut along its way, and the way square enough both to
/// the damager's cutting way (`forward`: an edge's facing, a point's way in) and to its length
/// (`up`), each within its horizontal / vertical angle of square
/// ([`MeleeBlade::dismember_angles`]; their cosines are compared with the angle over 90).
#[allow(clippy::too_many_arguments)]
pub fn sever_part(b: &MeleeBlade, j: &Judged, tier: u32, parts: &[(Vec3, Vec3, f32, u32)], hit: usize, point: Vec3, forward: Vec3, up: Vec3) -> Option<usize> {
    if b.dismember <= 0.0 || j.flags & CEF_PENETRATED == 0 || hit >= parts.len() {
        return None;
    }
    let tiers = if b.dismember_tiers == [0.0; 6] { [2.5, 1.5, 1.0, 0.9, 0.8, 0.8] } else { b.dismember_tiers };
    if j.speed <= b.dismember * tiers[(tier as usize).min(5)] {
        return None;
    }
    let [h, v] = if b.dismember_angles == [0.0; 2] { [35.0, 90.0] } else { b.dismember_angles };
    let parent = parents(parts);
    let fits = |i: usize| -> bool {
        let Some((height, adjust)) = slicing(parts[i].3, parent[i].map(|p| parts[p].3)) else { return false };
        let way = |k: usize| (parts[k].1 - parts[k].0).normalize_or_zero();
        let dir = way(i).lerp(parent[i].map_or(way(i), way), adjust);
        let at = joint_of(parts, i, parent[i]) + dir * height;
        (point - at).dot(dir.normalize_or_zero()).abs() < SLICE_WIDTH && dir.dot(up).abs() < v / 90.0 && dir.dot(forward).abs() < h / 90.0
    };
    if fits(hit) {
        return Some(hit);
    }
    if let Some(p) = parent[hit].filter(|&p| fits(p)) {
        return Some(p);
    }
    (0..parts.len()).find(|&c| parent[c] == Some(hit) && fits(c))
}

/// How near a part's cut (metres, along its way) a blow must land to take it off (0.04 on every
/// part).
pub const SLICE_WIDTH: f32 = 0.04;
/// What a severed piece flies off with: its speed halved, plus 3 m/s along the limb (3.5 a head).
pub const SLICE_KEEP: f32 = 0.5;
pub const SLICE_PUSH: f32 = 3.0;
pub const SLICE_PUSH_HEAD: f32 = 3.5;
/// A damager that just cut through something can't go in again for this long (s).
pub const SLICE_NO_PENETRATION: f64 = 1.0;
/// A point is through the far side once it's more than this deep (metres: the skewer test, with
/// its entry carried out past the part).
pub const SKEWER_DEPTH: f32 = 0.05;
/// The same collider isn't hit again by a damager within this (s).
pub const HIT_DELAY: f64 = 0.1;
/// A contact's effects are as loud and bright as its speed between these (m/s): its intensity,
/// 0..1.
pub const INTENSITY_FROM: f32 = 0.25;
pub const INTENSITY_FULL: f32 = 15.0;

pub fn intensity(speed: f32) -> f32 {
    ((speed - INTENSITY_FROM) / (INTENSITY_FULL - INTENSITY_FROM)).clamp(0.0, 1.0)
}

/// How loud a contact that does nothing sounds, by its intensity: the "soft" layer, the only one
/// such a contact plays (its volume curve 0 -> 0, 0.2 -> 1, 1 -> 0: a light touch is heard, a hard
/// one is the hit's own clip).
pub fn soft_volume(intensity: f32) -> f32 {
    let k = intensity.clamp(0.0, 1.0);
    if k < 0.2 { k / 0.2 } else { (1.0 - k) / 0.8 }
}

/// Metal sparks show only above this intensity (about 7.6 m/s).
pub const SPARKS_FROM: f32 = 0.5;

/// The push a damager pressed on a body needs to go in (a velocity change a step, m/s), by how fast
/// it moves against it (m/s), before the material's pressure multiplier: the faster it's drawn the
/// less it takes. A sword's or a dagger's
/// point: 5 at a standstill, 1.5 at 0.5 m/s, 0.25 at 1; their edges' the same to 0.35; an axe's 5 to
/// 0.5 at 2 m/s.
pub fn pressure_needed(b: &MeleeBlade, speed: f32) -> f32 {
    let end = match b.kind {
        DMG_SLASH if b.flags & DMGF_AXE != 0 => return curve([0.0, 5.0, 2.0, 0.5], [0.0; 2], speed),
        DMG_SLASH => 0.35,
        _ => 0.25,
    };
    if speed <= 0.5 {
        curve([0.0, 5.0, 0.5, 1.5], [0.0, -7.124_971], speed)
    } else {
        curve([0.5, 1.5, 1.0, end], [-7.124_971, 0.0], speed)
    }
}

/// A draw cut: an edge pressed on goes in only while it's drawn along its length, the way it's
/// drawn within this of its length (|cos| at least 1 - 0.2).
pub const PRESSURE_MAX_DOT: f32 = 0.2;

/// Whether `dir` lies within a damager's angles of its `forward`: in the plane across its `up`
/// (horizontal) and in its own plane (vertical), each under its limit (degrees; 180 or more: any).
pub fn within_angles(dir: Vec3, forward: Vec3, up: Vec3, horizontal: f32, vertical: f32) -> bool {
    let d = dir.normalize_or_zero();
    let plane = forward.cross(up).normalize_or_zero();
    let in_plane = (d - plane * d.dot(plane)).normalize_or_zero();
    let across = (d - up * d.dot(up)).normalize_or_zero();
    let angle = |a: Vec3| a.dot(forward).clamp(-1.0, 1.0).acos().to_degrees();
    (horizontal >= 180.0 || angle(across) < horizontal) && (vertical >= 180.0 || angle(in_plane) < vertical)
}
