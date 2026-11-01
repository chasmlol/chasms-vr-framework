//! The range's melee weapons, one of each kind, on the melee rack at your left (`arenas::RACK`),
//! and the ones the arena's fighters carry. Our own shapes (boxes) and numbers: each weapon's mass,
//! its handles (a main grip, a second hand's, the blade for half-swording, a long shaft that
//! slides), and its damagers' numbers at the weapon's tier (2: the common weapons, 4: the bladed
//! spear).
//!
//! Every weapon's blade runs along +Z from its handle (the guard, or where a haft meets its head, at
//! 0); an edge cuts towards ±X. A shield faces +Z, its grip a bar behind the boss along Y.

use glam::{Mat4, Quat, Vec3};
use rustvr_melee::*;
use rustvr_protocol::*;

use crate::content::MeshData;
use crate::throwables::Throwable;

const STEEL: [u8; 4] = [190, 192, 198, 255];
const DARK: [u8; 4] = [95, 98, 104, 255];
const BRASS: [u8; 4] = [150, 120, 60, 255];
const LEATHER: [u8; 4] = [60, 40, 30, 255];
const WOOD: [u8; 4] = [120, 86, 52, 255];

/// A weapon: what the range draws and registers, and its handles and damagers for the runtime.
pub struct Arm {
    pub thing: Throwable,
    pub melee: MeleeWeapon,
}

/// A damager's numbers, at a weapon's tier: (kind, flags, tier multiplier, velocity
/// curve, penetration damage, dampers held in / held out / free, dismemberment speed, its angles
/// and its tiers' multipliers, thrown multiplier, its bad angle's table and damage, its shove on a
/// body that's down, its effects' speeds after a damaging blow).
#[derive(Clone, Copy)]
struct Dmg {
    skewer: f32,
    kind: u32,
    flags: u32,
    modifier: u32,
    tier: f32,
    curve: [f32; 4],
    slopes: [f32; 2],
    pen_damage: f32,
    dampers: [f32; 3],
    dismember: f32,
    cut_angles: [f32; 2],
    cut_tiers: [f32; 6],
    thrown: f32,
    bad: (u32, f32),
    push: [f32; 4],
    intensity: [f32; 2],
}

/// A one-handed sword's slash (tier 2: 0.9; damage by speed 2 -> 1, 20 -> 2; penetration 15;
/// held in 10000, out 100, free 100; dismemberment 12 m/s, within 35 deg of square to the edge, by
/// tier x2.5, 1.5, 1, 0.9, 0.8, 0.8; slides).
/// Draw cuts (pressure allowed); at a bad angle, a light blunt blow of 0.5.
const SWORD_SLASH: Dmg = Dmg { skewer: 0.0, kind: DMG_SLASH, flags: DMGF_BOTH_WAYS | DMGF_SLIDES | DMGF_BLUNT_BAD | DMGF_PRESSURE, modifier: MOD_SLASH, tier: 0.9, curve: [2.0, 1.0, 20.0, 2.0], slopes: [0.0; 2], pen_damage: 15.0, dampers: [10000.0, 100.0, 100.0], dismember: 12.0, cut_angles: [35.0, 90.0], cut_tiers: [2.5, 1.5, 1.0, 0.9, 0.8, 0.8], thrown: 1.0, bad: (MOD_BLUNT_LIGHT, 0.5), push: [0.0; 4], intensity: [0.0; 2] };
/// A two-handed sword's slash (tier 2: 1.1; dismemberment 11, by tier x2, 1.5, 1, 0.9, 0.8, 0.8;
/// the heavier slash table; at a bad angle a blunt blow of 1).
const SWORD_SLASH_2H: Dmg = Dmg { modifier: MOD_SLASH_HEAVY, tier: 1.1, dismember: 11.0, cut_tiers: [2.0, 1.5, 1.0, 0.9, 0.8, 0.8], bad: (MOD_BLUNT, 1.0), ..SWORD_SLASH };
/// A sword's thrust (tier any: 1; 0 -> 1, 20 -> 10; penetration 5; held in 1000, out 500,
/// free 1250; pressure stabs; skewers; thrown x1.5).
const SWORD_PIERCE: Dmg = Dmg { skewer: 15.0, kind: DMG_PIERCE, flags: DMGF_PRESSURE, modifier: MOD_PIERCE, tier: 1.0, curve: rustvr_melee::SWORD_PIERCE_CURVE, slopes: rustvr_melee::SWORD_PIERCE_SLOPES, pen_damage: 5.0, dampers: [1000.0, 500.0, 1250.0], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 1.5, bad: (0, 0.0), push: [0.0; 4], intensity: [0.0; 2] };
/// A dagger's stab (0 -> 1, 20 -> 5; penetration 7.5; held in 500, out 250, free 500;
/// thrown x3).
const DAGGER_PIERCE: Dmg = Dmg { skewer: 7.5, kind: DMG_PIERCE, flags: DMGF_PRESSURE, modifier: MOD_PIERCE_FINE, tier: 1.0, curve: rustvr_melee::DAGGER_PIERCE_CURVE, slopes: rustvr_melee::DAGGER_PIERCE_SLOPES, pen_damage: 7.5, dampers: [500.0, 250.0, 500.0], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 3.0, bad: (0, 0.0), push: [0.0; 4], intensity: [0.0; 2] };
/// A dagger's slash (tier 2: 0.5; penetration 5; dismemberment 13, within 35 deg of square to
/// the edge and 60 to the blade; at a bad angle a very light blunt blow of 0.5).
const DAGGER_SLASH: Dmg = Dmg { modifier: MOD_SLASH_WEAK, tier: 0.5, pen_damage: 5.0, dismember: 13.0, cut_angles: [35.0, 60.0], bad: (MOD_BLUNT_VERY_LIGHT, 0.5), ..SWORD_SLASH };
/// A one-handed axe's slash (tier 2: 0.9; 2 -> 1, 20 -> 3; penetration 25; held in 0, out 10000,
/// free 5000; dismemberment 9, within 45 deg of square to the edge and 80 to the blade; doesn't
/// slide; draw cuts; at a bad angle a blunt blow of 2; shoves a body that's down with 50 for
/// 0.15 s).
const AXE_SLASH: Dmg = Dmg { skewer: 0.0, kind: DMG_SLASH, flags: DMGF_AXE | DMGF_BLUNT_BAD | DMGF_PRESSURE, modifier: MOD_SLASH_AXE, tier: 0.9, curve: [2.0, 1.0, 20.0, 3.0], slopes: [0.0; 2], pen_damage: 25.0, dampers: [0.0, 10000.0, 5000.0], dismember: 9.0, cut_angles: [45.0, 80.0], cut_tiers: [2.5, 1.5, 1.0, 0.9, 0.8, 0.8], thrown: 1.0, bad: (MOD_BLUNT, 2.0), push: [50.0, 0.15, 2.0, 0.5], intensity: [0.0; 2] };
/// A one-handed mace's head (tier 2: 2.4; 2 -> 1, 20 -> 5; a body that's down shoved with 105 for
/// 0.15 s; its effects 6 to 16 m/s), a two-handed one's (tier 2: 2.7; 200 for 0.2 s).
const MACE: Dmg = Dmg { skewer: 0.0, kind: DMG_BLUNT, flags: 0, modifier: MOD_BLUNT, tier: 2.4, curve: [2.0, 1.0, 20.0, 5.0], slopes: [0.0; 2], pen_damage: 0.0, dampers: [0.0; 3], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 1.0, bad: (0, 0.0), push: [105.0, 0.15, 2.0, 0.5], intensity: [6.0, 16.0] };
const MACE_2H: Dmg = Dmg { tier: 2.7, push: [200.0, 0.2, 2.0, 0.5], ..MACE };
/// A light handle's blow (light blunt: 2 -> 1, 20 -> 2; thrown x2.3; a body that's down pushed by
/// an impulse of 30 for 0.1 s, the part hit only) and a heavy one's (2 -> 1, 20 -> 5; thrown x2; 65
/// for 0.1 s, x1 and x0.2): pommels, hafts, staves; their effects 4 to 16 m/s.
const HANDLE_LIGHT: Dmg = Dmg { skewer: 0.0, kind: DMG_BLUNT, flags: DMGF_PUSH_IMPULSE, modifier: MOD_BLUNT_LIGHT, tier: 1.0, curve: [2.0, 1.0, 20.0, 2.0], slopes: [0.0; 2], pen_damage: 0.0, dampers: [0.0; 3], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 2.3, bad: (0, 0.0), push: [30.0, 0.1, 1.0, 0.0], intensity: [4.0, 16.0] };
const HANDLE_HEAVY: Dmg = Dmg { flags: 0, modifier: MOD_BLUNT, curve: [2.0, 1.0, 20.0, 5.0], thrown: 2.0, push: [65.0, 0.1, 1.0, 0.2], ..HANDLE_LIGHT };
/// A shield's bash (tier 2: 0.95; 2 -> 1, 20 -> 5; thrown x1; a body that's down shoved
/// with 400 for 0.15 s).
const SHIELD: Dmg = Dmg { tier: 0.95, thrown: 1.0, push: [400.0, 0.15, 2.0, 0.5], intensity: [0.0; 2], ..HANDLE_HEAVY };

fn blade(d: Dmg, a: Vec3, b: Vec3, radius: f32, edge: Vec3, depth: f32) -> MeleeBlade {
    MeleeBlade {
        kind: d.kind,
        flags: d.flags,
        depth,
        radius,
        a: a.to_array(),
        tier: d.tier,
        b: b.to_array(),
        dismember: d.dismember,
        dismember_angles: d.cut_angles,
        dismember_tiers: d.cut_tiers,
        bad_modifier: d.bad.0,
        bad_damage: d.bad.1,
        push: d.push,
        intensity: d.intensity,
        edge: edge.to_array(),
        thrown: d.thrown,
        penetration_damage: d.pen_damage,
        curve: d.curve,
        slopes: d.slopes,
        modifier: d.modifier,
        skewer: d.skewer,
        damper_in: d.dampers[0],
        damper_out: d.dampers[1],
        damper_free: d.dampers[2],
        ..Default::default()
    }
}

/// A blade whose cut runs `length` along its edge before it comes off the end, shorter than the
/// edge it hits with.
fn sliding(mut b: MeleeBlade, length: f32) -> MeleeBlade {
    b.slide_length = length;
    b
}

fn handle(a: Vec3, b: Vec3, radius: f32, flags: u32, rot: Quat) -> MeleeHandle {
    MeleeHandle { a: a.to_array(), radius, b: b.to_array(), flags, rot: rot.to_array(), ..Default::default() }
}

/// A handle of grip kind `kind` (`GRIP_*`), and the handles a hand sliding off its `b` end and its
/// `a` end goes on to.
fn kind_of(mut h: MeleeHandle, kind: u32, past_b: Option<usize>, past_a: Option<usize>) -> MeleeHandle {
    h.kind = kind;
    h.links = past_b.map_or(0, |n| n as u32 + 1) | past_a.map_or(0, |n| n as u32 + 1) << 8;
    h
}

/// The fist round a handle along `axis` (little finger to thumb), the back of the hand to `back`.
fn fist(axis: Vec3, back: Vec3) -> Quat {
    rustvr_guns::fist(axis, back)
}

/// A melee weapon's object: its collider is cut from its model's parts (`throwables` makes them
/// hulls), so only its bounding box and the hand's main grip are said here.
fn thing(key: &str, name: &str, parts: Vec<MeshData>, mass: f32, grip: [Vec3; 2], radius: f32, grip_rot: Quat, lying: (Quat, f32)) -> Throwable {
    let (lo, hi) = parts.iter().filter_map(|m| m.bounds(Mat4::IDENTITY)).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)));
    let mut d: ObjectDesc = bytemuck::Zeroable::zeroed();
    d.kind = OBJ_MELEE;
    d.flags = OBJF_GRABBABLE | OBJF_PULLABLE | OBJF_HOLSTERABLE;
    d.shape = SHAPE_BOX;
    d.half_extents = ((hi - lo) * 0.5).to_array();
    d.center = ((hi + lo) * 0.5).to_array();
    d.mass = mass;
    d.grip = ((grip[0] + grip[1]) * 0.5).to_array();
    d.grip_rot = grip_rot.to_array();
    d.grip_line = [grip[0].to_array(), grip[1].to_array()];
    d.grip_radius = radius;
    d.muzzle = [0.0, 0.0, hi.z];
    set_name(&mut d.name, name);
    Throwable { key: key.into(), name: name.into(), parts, fix: Mat4::IDENTITY, from_game: false, pin_parts: Vec::new(), lever_parts: Vec::new(), desc: d, blast: None, fuse_at: None, led: None, lying }
}

/// Lying flat on a table, its flat (+Y) up, its blade along +X.
fn flat() -> Quat {
    Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
}

fn melee(flags: u32, handles: &[MeleeHandle], blades: &[MeleeBlade], balance: Vec3) -> MeleeWeapon {
    let mut w: MeleeWeapon = bytemuck::Zeroable::zeroed();
    w.flags = flags;
    w.handles = handles.len().min(MAX_HANDLES) as u32;
    w.blades = blades.len().min(MAX_BLADES) as u32;
    w.handle[..w.handles as usize].copy_from_slice(&handles[..w.handles as usize]);
    w.blade[..w.blades as usize].copy_from_slice(&blades[..w.blades as usize]);
    w.balance = balance.to_array();
    // The common weapons are tier 2.
    w.tier = 2;
    w.throw_mult = 1.0;
    w
}

/// A weapon that flies point first when thrown within `angle` degrees of its flight, turned along
/// it at `rate` (3 most; a staff's 5).
fn flies(mut w: MeleeWeapon, angle: f32, rate: f32) -> MeleeWeapon {
    w.flags |= MW_FLIES;
    w.fly_angle = angle;
    w.fly_rate = rate;
    w
}

/// The arming sword: 1.25 kg, a 72 cm blade. Handles: its grip (a light handle), the
/// pommel and the ricasso for a second hand, and the blade for half-swording, reached by sliding up
/// off the grip.
pub fn sword() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.022, -0.003, 0.0), Vec3::new(0.022, 0.003, 0.72), STEEL);
    m.add_box(Vec3::new(-0.012, -0.0025, 0.72), Vec3::new(0.012, 0.0025, 0.8), STEEL);
    m.add_box(Vec3::new(-0.08, -0.012, -0.012), Vec3::new(0.08, 0.012, 0.0), BRASS);
    m.add_box(Vec3::new(-0.014, -0.014, -0.11), Vec3::new(0.014, 0.014, -0.012), LEATHER);
    m.add_box(Vec3::new(-0.022, -0.022, -0.135), Vec3::new(0.022, 0.022, -0.11), BRASS);
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("builtin_sword", "Arming sword", vec![m], 1.25, [Vec3::new(0.0, 0.0, -0.11), Vec3::new(0.0, 0.0, -0.012)], 0.014, grip, (flat(), 0.012));
    let melee = melee(
        0,
        &[
            kind_of(handle(Vec3::new(0.0, 0.0, -0.11), Vec3::new(0.0, 0.0, -0.012), 0.014, HANDLE_MAIN | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip), GRIP_LIGHT, Some(2), None),
            kind_of(handle(Vec3::new(0.0, 0.0, -0.135), Vec3::new(0.0, 0.0, -0.11), 0.022, HANDLE_REVERSIBLE, grip), GRIP_SUPPORTING, None, None),
            kind_of(handle(Vec3::new(0.0, 0.0, 0.25), Vec3::new(0.0, 0.0, 0.6), 0.02, HANDLE_BLADE | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip), GRIP_HALF_SWORD, None, Some(0)),
        ],
        &[
            sliding(blade(SWORD_SLASH, Vec3::new(0.0, 0.0, 0.03), Vec3::new(0.0, 0.0, 0.79), 0.025, Vec3::X, 0.01), 0.63),
            blade(SWORD_PIERCE, Vec3::new(0.0, 0.0, 0.7), Vec3::new(0.0, 0.0, 0.8), 0.015, Vec3::X, 0.64),
            blade(HANDLE_HEAVY, Vec3::new(0.0, 0.0, -0.135), Vec3::new(0.0, 0.0, -0.11), 0.025, Vec3::ZERO, 0.0),
        ],
        Vec3::new(0.0, 0.0, 0.12),
    );
    Arm { thing, melee }
}

/// The dagger: 0.65 kg, a 22 cm blade; held either way round.
pub fn dagger() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.016, -0.0025, 0.0), Vec3::new(0.016, 0.0025, 0.18), STEEL);
    m.add_box(Vec3::new(-0.008, -0.002, 0.18), Vec3::new(0.008, 0.002, 0.22), STEEL);
    m.add_box(Vec3::new(-0.04, -0.01, -0.01), Vec3::new(0.04, 0.01, 0.0), BRASS);
    m.add_box(Vec3::new(-0.013, -0.013, -0.1), Vec3::new(0.013, 0.013, -0.01), LEATHER);
    m.add_box(Vec3::new(-0.017, -0.017, -0.115), Vec3::new(0.017, 0.017, -0.1), BRASS);
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("kit_dagger", "Dagger", vec![m], 0.65, [Vec3::new(0.0, 0.0, -0.1), Vec3::new(0.0, 0.0, -0.01)], 0.013, grip, (flat(), 0.017));
    let melee = melee(
        0,
        &[handle(Vec3::new(0.0, 0.0, -0.1), Vec3::new(0.0, 0.0, -0.01), 0.013, HANDLE_MAIN | HANDLE_REVERSIBLE, grip)],
        &[
            sliding(blade(DAGGER_SLASH, Vec3::new(0.0, 0.0, 0.01), Vec3::new(0.0, 0.0, 0.215), 0.018, Vec3::X, 0.01), 0.208),
            blade(DAGGER_PIERCE, Vec3::new(0.0, 0.0, 0.17), Vec3::new(0.0, 0.0, 0.22), 0.01, Vec3::X, 0.22),
            blade(HANDLE_LIGHT, Vec3::new(0.0, 0.0, -0.115), Vec3::new(0.0, 0.0, -0.1), 0.02, Vec3::ZERO, 0.0),
        ],
        Vec3::new(0.0, 0.0, 0.02),
    );
    // (A dagger flies point first thrown within 145 degrees of it: almost always.)
    let mut melee = flies(melee, 145.0, 3.0);
    // (The dagger's drag is 0.4.)
    melee.drag = [0.4, 0.05];
    Arm { thing, melee }
}

/// The longsword: 2 kg, a 92 cm blade, a grip for both hands (a heavy handle).
pub fn longsword() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.024, -0.0035, 0.0), Vec3::new(0.024, 0.0035, 0.84), STEEL);
    m.add_box(Vec3::new(-0.013, -0.003, 0.84), Vec3::new(0.013, 0.003, 0.93), STEEL);
    m.add_box(Vec3::new(-0.11, -0.013, -0.014), Vec3::new(0.11, 0.013, 0.0), DARK);
    m.add_box(Vec3::new(-0.015, -0.015, -0.25), Vec3::new(0.015, 0.015, -0.014), LEATHER);
    m.add_box(Vec3::new(-0.026, -0.026, -0.28), Vec3::new(0.026, 0.026, -0.25), DARK);
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("kit_longsword", "Longsword", vec![m], 2.0, [Vec3::new(0.0, 0.0, -0.25), Vec3::new(0.0, 0.0, -0.014)], 0.015, grip, (flat(), 0.026));
    let melee = melee(
        0,
        &[
            kind_of(handle(Vec3::new(0.0, 0.0, -0.25), Vec3::new(0.0, 0.0, -0.014), 0.015, HANDLE_MAIN | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip), GRIP_HEAVY, Some(1), None),
            kind_of(handle(Vec3::new(0.0, 0.0, 0.3), Vec3::new(0.0, 0.0, 0.75), 0.02, HANDLE_BLADE | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip), GRIP_HALF_SWORD, None, Some(0)),
        ],
        &[
            sliding(blade(SWORD_SLASH, Vec3::new(0.0, 0.0, 0.04), Vec3::new(0.0, 0.0, 0.92), 0.027, Vec3::X, 0.015), 0.85),
            blade(SWORD_PIERCE, Vec3::new(0.0, 0.0, 0.82), Vec3::new(0.0, 0.0, 0.93), 0.016, Vec3::X, 0.86),
            blade(HANDLE_HEAVY, Vec3::new(0.0, 0.0, -0.28), Vec3::new(0.0, 0.0, -0.25), 0.028, Vec3::ZERO, 0.0),
        ],
        Vec3::new(0.0, 0.0, 0.1),
    );
    Arm { thing, melee }
}

/// The mace: 1.5 kg, a flanged head on a 55 cm haft.
pub fn mace() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.014, -0.014, -0.15), Vec3::new(0.014, 0.014, 0.4), WOOD);
    m.add_box(Vec3::new(-0.018, -0.018, -0.17), Vec3::new(0.018, 0.018, -0.15), DARK);
    for k in 0..4 {
        let q = Quat::from_rotation_z(k as f32 * std::f32::consts::FRAC_PI_4);
        m.add_obox(Vec3::new(0.0, 0.0, 0.46), Vec3::new(0.055, 0.008, 0.06), q, DARK);
    }
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("kit_mace", "Mace", vec![m], 1.5, [Vec3::new(0.0, 0.0, -0.15), Vec3::new(0.0, 0.0, 0.0)], 0.014, grip, (flat(), 0.055));
    let melee = melee(
        0,
        &[handle(Vec3::new(0.0, 0.0, -0.15), Vec3::new(0.0, 0.0, 0.3), 0.014, HANDLE_MAIN | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip)],
        &[blade(MACE, Vec3::new(0.0, 0.0, 0.41), Vec3::new(0.0, 0.0, 0.51), 0.06, Vec3::ZERO, 0.0), blade(HANDLE_LIGHT, Vec3::new(0.0, 0.0, -0.17), Vec3::new(0.0, 0.0, -0.15), 0.02, Vec3::ZERO, 0.0)],
        Vec3::new(0.0, 0.0, 0.32),
    );
    Arm { thing, melee }
}

/// The axe: 1.2 kg, a bearded head on a 60 cm haft, its edge towards +X.
pub fn axe() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.015, -0.012, -0.2), Vec3::new(0.015, 0.012, 0.42), WOOD);
    m.add_box(Vec3::new(-0.02, -0.014, 0.33), Vec3::new(0.09, 0.014, 0.42), DARK);
    m.add_box(Vec3::new(0.09, -0.004, 0.28), Vec3::new(0.12, 0.004, 0.44), STEEL);
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("kit_axe", "Axe", vec![m], 1.2, [Vec3::new(0.0, 0.0, -0.2), Vec3::new(0.0, 0.0, 0.25)], 0.015, grip, (flat(), 0.014));
    let melee = melee(
        0,
        &[handle(Vec3::new(0.0, 0.0, -0.2), Vec3::new(0.0, 0.0, 0.28), 0.015, HANDLE_MAIN | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip)],
        &[
            sliding(blade(AXE_SLASH, Vec3::new(0.115, 0.0, 0.285), Vec3::new(0.115, 0.0, 0.435), 0.01, Vec3::X, 0.08), 0.15),
            blade(HANDLE_LIGHT, Vec3::new(0.0, 0.0, -0.2), Vec3::new(0.0, 0.0, -0.17), 0.02, Vec3::ZERO, 0.0),
        ],
        Vec3::new(0.02, 0.0, 0.25),
    );
    Arm { thing, melee }
}

/// The bladed spear (tier 4): 1.65 kg, a 25 cm leaf point on a 1.75 m shaft that the hands slide
/// along (one long heavy handle).
pub fn spear() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.014, -0.014, -1.5), Vec3::new(0.014, 0.014, 0.25), WOOD);
    m.add_box(Vec3::new(-0.022, -0.004, 0.25), Vec3::new(0.022, 0.004, 0.44), STEEL);
    m.add_box(Vec3::new(-0.01, -0.003, 0.44), Vec3::new(0.01, 0.003, 0.5), STEEL);
    m.add_box(Vec3::new(-0.017, -0.017, -1.52), Vec3::new(0.017, 0.017, -1.5), DARK);
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("kit_spear", "Spear", vec![m], 1.65, [Vec3::new(0.0, 0.0, -1.45), Vec3::new(0.0, 0.0, 0.2)], 0.014, grip, (flat(), 0.017));
    let pierce = Dmg { tier: 1.0, ..SWORD_PIERCE };
    let slash = Dmg { tier: 1.3, ..SWORD_SLASH_2H };
    let mut melee = melee(
        0,
        &[kind_of(handle(Vec3::new(0.0, 0.0, -1.45), Vec3::new(0.0, 0.0, 0.2), 0.014, HANDLE_MAIN | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip), GRIP_HEAVY, None, None)],
        &[
            blade(pierce, Vec3::new(0.0, 0.0, 0.42), Vec3::new(0.0, 0.0, 0.5), 0.012, Vec3::ZERO, 0.35),
            blade(slash, Vec3::new(0.0, 0.0, 0.27), Vec3::new(0.0, 0.0, 0.44), 0.02, Vec3::X, 0.02),
            blade(HANDLE_HEAVY, Vec3::new(0.0, 0.0, -1.52), Vec3::new(0.0, 0.0, -1.48), 0.02, Vec3::ZERO, 0.0),
        ],
        Vec3::new(0.0, 0.0, -0.45),
    );
    // (The bladed spear is tier 4.)
    melee.tier = 4;
    // (A spear flies point first thrown within 60 degrees of it.)
    let mut melee = flies(melee, 60.0, 3.0);
    // (The spear's drag is 0.4.)
    melee.drag = [0.4, 0.05];
    Arm { thing, melee }
}

/// The round shield: 3 kg, 64 cm across, its grip a bar behind the boss; its rim and face bludgeon
/// (`SHIELD`).
pub fn shield() -> Arm {
    let mut m = MeshData::default();
    for k in 0..8 {
        let q = Quat::from_rotation_z(k as f32 * std::f32::consts::PI / 8.0);
        m.add_obox(Vec3::ZERO, Vec3::new(0.32, 0.13, 0.012), q, WOOD);
    }
    m.add_obox(Vec3::new(0.0, 0.0, 0.015), Vec3::new(0.07, 0.07, 0.015), Quat::IDENTITY, DARK);
    m.add_box(Vec3::new(-0.014, -0.06, -0.06), Vec3::new(0.014, 0.06, -0.035), LEATHER);
    m.add_box(Vec3::new(-0.014, -0.075, -0.06), Vec3::new(0.014, -0.06, -0.012), DARK);
    m.add_box(Vec3::new(-0.014, 0.06, -0.06), Vec3::new(0.014, 0.075, -0.012), DARK);
    // The fist up the bar, its back to the shield's face.
    let grip = fist(Vec3::Y, Vec3::Z);
    let thing = thing("kit_shield", "Round shield", vec![m], 3.0, [Vec3::new(0.0, -0.06, -0.047), Vec3::new(0.0, 0.06, -0.047)], 0.014, grip, (Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2), 0.06));
    let melee = melee(
        MW_SHIELD,
        &[kind_of(handle(Vec3::new(0.0, -0.06, -0.047), Vec3::new(0.0, 0.06, -0.047), 0.014, HANDLE_MAIN, grip), GRIP_SHIELD, None, None)],
        &[blade(SHIELD, Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.03), 0.3, Vec3::ZERO, 0.0)],
        Vec3::ZERO,
    );
    // (The round shield's drag is 0.4.)
    let mut melee = melee;
    melee.drag = [0.4, 0.05];
    Arm { thing, melee }
}

/// The staff: 1.5 kg, 1.8 m, held anywhere along it and slid through the hands; both ends bludgeon
/// (`HANDLE_HEAVY`).
pub fn staff() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.016, -0.016, -0.9), Vec3::new(0.016, 0.016, 0.9), WOOD);
    m.add_box(Vec3::new(-0.02, -0.02, 0.84), Vec3::new(0.02, 0.02, 0.9), BRASS);
    m.add_box(Vec3::new(-0.02, -0.02, -0.9), Vec3::new(0.02, 0.02, -0.84), BRASS);
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("kit_staff", "Staff", vec![m], 1.5, [Vec3::new(0.0, 0.0, -0.8), Vec3::new(0.0, 0.0, 0.8)], 0.016, grip, (flat(), 0.02));
    let melee = melee(
        MW_STAFF,
        &[kind_of(handle(Vec3::new(0.0, 0.0, -0.8), Vec3::new(0.0, 0.0, 0.8), 0.016, HANDLE_MAIN | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip), GRIP_HEAVY, None, None)],
        &[blade(HANDLE_HEAVY, Vec3::new(0.0, 0.0, 0.8), Vec3::new(0.0, 0.0, 0.9), 0.025, Vec3::ZERO, 0.0), blade(HANDLE_HEAVY, Vec3::new(0.0, 0.0, -0.9), Vec3::new(0.0, 0.0, -0.8), 0.025, Vec3::ZERO, 0.0)],
        Vec3::ZERO,
    );
    // (The staff flies end first within 30 degrees, turned along its flight at 5.)
    let melee = flies(melee, 30.0, 5.0);
    Arm { thing, melee }
}

/// The maul: 8 kg, a heavy head on a 1.1 m haft: what weight feels like.
pub fn maul() -> Arm {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.018, -0.018, -0.75), Vec3::new(0.018, 0.018, 0.3), WOOD);
    m.add_box(Vec3::new(-0.13, -0.06, 0.3), Vec3::new(0.13, 0.06, 0.42), DARK);
    let grip = fist(Vec3::Z, Vec3::Y);
    let thing = thing("kit_maul", "Maul", vec![m], 8.0, [Vec3::new(0.0, 0.0, -0.72), Vec3::new(0.0, 0.0, 0.2)], 0.018, grip, (flat(), 0.06));
    let melee = melee(
        0,
        &[kind_of(handle(Vec3::new(0.0, 0.0, -0.72), Vec3::new(0.0, 0.0, 0.2), 0.018, HANDLE_MAIN | HANDLE_SLIDE | HANDLE_REVERSIBLE, grip), GRIP_HEAVY, None, None)],
        &[blade(MACE_2H, Vec3::new(-0.13, 0.0, 0.36), Vec3::new(0.13, 0.0, 0.36), 0.065, Vec3::ZERO, 0.0), blade(HANDLE_HEAVY, Vec3::new(0.0, 0.0, -0.75), Vec3::new(0.0, 0.0, -0.7), 0.025, Vec3::ZERO, 0.0)],
        Vec3::new(0.0, 0.0, 0.28),
    );
    Arm { thing, melee }
}

/// Every weapon the range has, in the rack's order (the arming sword first: the scenarios take it
/// from the throwables' table).
pub fn all() -> Vec<Arm> {
    vec![sword(), dagger(), longsword(), mace(), axe(), spear(), shield(), staff(), maul()]
}

/// Where each weapon lies on the rack (x along it, from its left end; the long ones along its
/// length instead of across it).
pub fn rack_places() -> Vec<(&'static str, Vec3, Quat)> {
    let (lo, hi) = crate::arenas::RACK;
    let top = hi.y;
    let z = (lo.z + hi.z) * 0.5;
    // The rack runs along X at your left: short weapons lie across it, blades away, handles to
    // its front edge (+Z, towards where you start); the long ones lie along it.
    let across = Quat::from_rotation_y(std::f32::consts::PI);
    let along = flat();
    let a = crate::arenas::ARENA_TABLE;
    let b = crate::arenas::BENCH_TABLE;
    let leftward = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    // (Blades pointing away from the arena's start, flat side up.)
    let toward = Quat::IDENTITY;
    vec![
        ("kit_dagger", Vec3::new(lo.x + 0.15, top, z), across),
        ("kit_mace", Vec3::new(lo.x + 0.35, top, z), across),
        ("kit_axe", Vec3::new(lo.x + 0.55, top, z), across),
        ("kit_longsword", Vec3::new(lo.x + 0.8, top, z - 0.1), across),
        ("kit_maul", Vec3::new(lo.x + 1.1, top, z), across),
        ("kit_shield", Vec3::new(lo.x + 1.55, top, z), Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        ("kit_spear", Vec3::new((lo.x + hi.x) * 0.5, top, lo.z + 0.08), along),
        ("kit_staff", Vec3::new((lo.x + hi.x) * 0.5 + 0.1, top, hi.z - 0.08), along),
        // The arena's table at your left as you go in: a sword, a dagger and a longsword, blades
        // away and handles towards you at its near edge, and a shield beside them.
        ("builtin_sword", Vec3::new(a.1.x - 0.1, a.1.y, a.0.z + 0.15), toward),
        ("kit_dagger", Vec3::new(a.1.x - 0.25, a.1.y, a.0.z + 0.13), toward),
        ("kit_longsword", Vec3::new(a.1.x - 0.42, a.1.y, a.0.z + 0.3), toward),
        ("kit_shield", Vec3::new(a.0.x + 0.35, a.1.y, a.0.z + 0.6), Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        // The bench's table at your left: a dagger, a sword and a mace across it, handles towards
        // you over its near edge, heads and blades away to the left.
        ("kit_dagger", Vec3::new(b.1.x - 0.12, b.1.y, b.0.z + 0.3), leftward),
        ("builtin_sword", Vec3::new(b.1.x - 0.13, b.1.y, b.0.z + 0.6), leftward),
        ("kit_mace", Vec3::new(b.1.x - 0.17, b.1.y, b.0.z + 0.9), leftward),
    ]
}
