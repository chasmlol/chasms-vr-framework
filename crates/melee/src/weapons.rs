//! Ready-made damagers and handles, as the range's weapons carry them (their numbers, their
//! parts), for a game to describe its melee weapons with ([`MeleeWeapon`]): the test range's own
//! weapons, and a game's weapons where it has no such numbers of its own (Oblivion's, by its weapon
//! types). Every number is our own tuning.

use bytemuck::Zeroable;
use glam::{Quat, Vec3};

use crate::*;

/// A damager's numbers, at a weapon's tier: (kind, flags, tier multiplier, velocity
/// curve, penetration damage, dampers held in / held out / free, dismemberment speed, its angles
/// and its tiers' multipliers, thrown multiplier, its bad angle's table and damage, its shove on a
/// body that's down, its effects' speeds after a damaging blow).
#[derive(Clone, Copy, Debug)]
pub struct Dmg {
    pub skewer: f32,
    pub kind: u32,
    pub flags: u32,
    pub modifier: u32,
    pub tier: f32,
    pub curve: [f32; 4],
    pub slopes: [f32; 2],
    pub pen_damage: f32,
    pub dampers: [f32; 3],
    pub dismember: f32,
    pub cut_angles: [f32; 2],
    pub cut_tiers: [f32; 6],
    pub thrown: f32,
    pub bad: (u32, f32),
    pub push: [f32; 4],
    pub intensity: [f32; 2],
}

/// A one-handed sword's slash (tier 2: 0.9; damage by speed 2 -> 1, 20 -> 2; penetration 15;
/// held in 10000, out 100, free 100; dismemberment 12 m/s, within 35 deg of square to the edge, by
/// tier x2.5, 1.5, 1, 0.9, 0.8, 0.8; slides).
/// Draw cuts (pressure allowed); at a bad angle, a light blunt blow of 0.5.
pub const SWORD_SLASH: Dmg = Dmg { skewer: 0.0, kind: DMG_SLASH, flags: DMGF_BOTH_WAYS | DMGF_SLIDES | DMGF_BLUNT_BAD | DMGF_PRESSURE, modifier: MOD_SLASH, tier: 0.9, curve: [2.0, 1.0, 20.0, 2.0], slopes: [0.0; 2], pen_damage: 15.0, dampers: [10000.0, 100.0, 100.0], dismember: 12.0, cut_angles: [35.0, 90.0], cut_tiers: [2.5, 1.5, 1.0, 0.9, 0.8, 0.8], thrown: 1.0, bad: (MOD_BLUNT_LIGHT, 0.5), push: [0.0; 4], intensity: [0.0; 2] };
/// A two-handed sword's slash (tier 2: 1.1; dismemberment 11, by tier x2, 1.5, 1, 0.9, 0.8, 0.8;
/// the heavier slash table; at a bad angle a blunt blow of 1).
pub const SWORD_SLASH_2H: Dmg = Dmg { modifier: MOD_SLASH_HEAVY, tier: 1.1, dismember: 11.0, cut_tiers: [2.0, 1.5, 1.0, 0.9, 0.8, 0.8], bad: (MOD_BLUNT, 1.0), ..SWORD_SLASH };
/// A sword's point (tier any: 1; 0 -> 1, 20 -> 10; penetration 5; held in 1000, out 500,
/// free 1250; pressure stabs; skewers; thrown x1.5).
pub const SWORD_PIERCE: Dmg = Dmg { skewer: 15.0, kind: DMG_PIERCE, flags: DMGF_PRESSURE, modifier: MOD_PIERCE, tier: 1.0, curve: SWORD_PIERCE_CURVE, slopes: SWORD_PIERCE_SLOPES, pen_damage: 5.0, dampers: [1000.0, 500.0, 1250.0], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 1.5, bad: (0, 0.0), push: [0.0; 4], intensity: [0.0; 2] };
/// A dagger's point (0 -> 1, 20 -> 5; penetration 7.5; held in 500, out 250, free 500;
/// thrown x3).
pub const DAGGER_PIERCE: Dmg = Dmg { skewer: 7.5, kind: DMG_PIERCE, flags: DMGF_PRESSURE, modifier: MOD_PIERCE_FINE, tier: 1.0, curve: DAGGER_PIERCE_CURVE, slopes: DAGGER_PIERCE_SLOPES, pen_damage: 7.5, dampers: [500.0, 250.0, 500.0], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 3.0, bad: (0, 0.0), push: [0.0; 4], intensity: [0.0; 2] };
/// A dagger's edge (tier 2: 0.5; penetration 5; dismemberment 13, within 35 deg of square to
/// the edge and 60 to the blade; at a bad angle a very light blunt blow of 0.5).
pub const DAGGER_SLASH: Dmg = Dmg { modifier: MOD_SLASH_WEAK, tier: 0.5, pen_damage: 5.0, dismember: 13.0, cut_angles: [35.0, 60.0], bad: (MOD_BLUNT_VERY_LIGHT, 0.5), ..SWORD_SLASH };
/// An axe's edge (tier 2: 0.9; 2 -> 1, 20 -> 3; penetration 25; held in 0, out 10000, free
/// 5000; dismemberment 9, within 45 deg of square to the edge and 80 to the blade; doesn't slide;
/// draw cuts; at a bad angle a blunt blow of 2; shoves a body that's down with 50 for 0.15 s).
pub const AXE_SLASH: Dmg = Dmg { skewer: 0.0, kind: DMG_SLASH, flags: DMGF_AXE | DMGF_BLUNT_BAD | DMGF_PRESSURE, modifier: MOD_SLASH_AXE, tier: 0.9, curve: [2.0, 1.0, 20.0, 3.0], slopes: [0.0; 2], pen_damage: 25.0, dampers: [0.0, 10000.0, 5000.0], dismember: 9.0, cut_angles: [45.0, 80.0], cut_tiers: [2.5, 1.5, 1.0, 0.9, 0.8, 0.8], thrown: 1.0, bad: (MOD_BLUNT, 2.0), push: [50.0, 0.15, 2.0, 0.5], intensity: [0.0; 2] };
/// A mace's head (tier 2: 2.4; 2 -> 1, 20 -> 5; a body that's down shoved with 105 for 0.15 s;
/// its effects 6 to 16 m/s), a two-handed one's (tier 2: 2.7; 200 for 0.2 s).
pub const MACE: Dmg = Dmg { skewer: 0.0, kind: DMG_BLUNT, flags: 0, modifier: MOD_BLUNT, tier: 2.4, curve: [2.0, 1.0, 20.0, 5.0], slopes: [0.0; 2], pen_damage: 0.0, dampers: [0.0; 3], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 1.0, bad: (0, 0.0), push: [105.0, 0.15, 2.0, 0.5], intensity: [6.0, 16.0] };
pub const MACE_2H: Dmg = Dmg { tier: 2.7, push: [200.0, 0.2, 2.0, 0.5], ..MACE };
/// A light handle's blow (light blunt: 2 -> 1, 20 -> 2; thrown x2.3; a body that's down pushed by
/// an impulse of 30 for 0.1 s, the part hit only) and a heavy one's (2 -> 1, 20 -> 5; thrown x2; 65
/// for 0.1 s, x1 and x0.2): pommels, hafts, staves; their effects 4 to 16 m/s.
pub const HANDLE_LIGHT: Dmg = Dmg { skewer: 0.0, kind: DMG_BLUNT, flags: DMGF_PUSH_IMPULSE, modifier: MOD_BLUNT_LIGHT, tier: 1.0, curve: [2.0, 1.0, 20.0, 2.0], slopes: [0.0; 2], pen_damage: 0.0, dampers: [0.0; 3], dismember: 0.0, cut_angles: [0.0; 2], cut_tiers: [0.0; 6], thrown: 2.3, bad: (0, 0.0), push: [30.0, 0.1, 1.0, 0.0], intensity: [4.0, 16.0] };
pub const HANDLE_HEAVY: Dmg = Dmg { flags: 0, modifier: MOD_BLUNT, curve: [2.0, 1.0, 20.0, 5.0], thrown: 2.0, push: [65.0, 0.1, 1.0, 0.2], ..HANDLE_LIGHT };
/// A shield's rim (tier 2: 0.95; 2 -> 1, 20 -> 5; thrown x1; a body that's down shoved
/// with 400 for 0.15 s).
pub const SHIELD: Dmg = Dmg { tier: 0.95, thrown: 1.0, push: [400.0, 0.15, 2.0, 0.5], intensity: [0.0; 2], ..HANDLE_HEAVY };

pub fn blade(d: Dmg, a: Vec3, b: Vec3, radius: f32, edge: Vec3, depth: f32) -> MeleeBlade {
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
pub fn sliding(mut b: MeleeBlade, length: f32) -> MeleeBlade {
    b.slide_length = length;
    b
}

pub fn handle(a: Vec3, b: Vec3, radius: f32, flags: u32, rot: Quat) -> MeleeHandle {
    MeleeHandle { a: a.to_array(), radius, b: b.to_array(), flags, rot: rot.to_array(), ..Default::default() }
}

/// A handle of grip kind `kind` (`GRIP_*`), and the handles a hand sliding off its `b` end and its
/// `a` end goes on to.
pub fn kind_of(mut h: MeleeHandle, kind: u32, past_b: Option<usize>, past_a: Option<usize>) -> MeleeHandle {
    h.kind = kind;
    h.links = past_b.map_or(0, |n| n as u32 + 1) | past_a.map_or(0, |n| n as u32 + 1) << 8;
    h
}


pub fn melee(flags: u32, handles: &[MeleeHandle], blades: &[MeleeBlade], balance: Vec3) -> MeleeWeapon {
    let mut w = MeleeWeapon::zeroed();
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
pub fn flies(mut w: MeleeWeapon, angle: f32, rate: f32) -> MeleeWeapon {
    w.flags |= MW_FLIES;
    w.fly_angle = angle;
    w.fly_rate = rate;
    w
}


/// A hand's Punch damager (the player's hands: their palm and fingers, in the drawn hand's grip
/// space, the fingers out along +Z): judged by the Punch table and curve (2 m/s: 1, 20 m/s: 5),
/// within 60 degrees of square, never going in, twice its damage thrown; `radius` thick.
pub fn punch(radius: f32) -> MeleeBlade {
    let mut b = MeleeBlade::zeroed();
    b.kind = DMG_PUNCH;
    b.modifier = MOD_PUNCH;
    b.radius = radius;
    b.a = [0.0, 0.0, -0.02];
    b.b = [0.0, 0.0, 0.1];
    b.tier = 1.0;
    b.thrown = 2.0;
    b.normal = 60.0;
    b.curve = [2.0, 1.0, 20.0, 5.0];
    b
}

/// A foot's kick damager (the kick table, 2 -> 1, 20 -> 5, x2 thrown, any
/// way; a body that's down shoved with 400 for 0.15 s, x2 and x0.5; its effects 2 to 4 m/s),
/// `radius` thick.
pub fn kick(radius: f32) -> MeleeBlade {
    let mut b = MeleeBlade::zeroed();
    b.kind = DMG_BLUNT;
    b.modifier = MOD_KICK;
    b.radius = radius;
    b.tier = 1.0;
    b.thrown = 2.0;
    b.curve = [2.0, 1.0, 20.0, 5.0];
    b.push = [400.0, 0.15, 2.0, 0.5];
    b.intensity = [2.0, 4.0];
    b
}
