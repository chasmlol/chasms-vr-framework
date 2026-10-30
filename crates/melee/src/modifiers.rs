//! Damage types against materials: our damage tables, their un-imbued rows, and the fire-imbued rows
//! that come first for a weapon filled with fire. A row says how fast a hit must be to count
//! against that material, what it multiplies the damage by, whether and how fast it goes in (how
//! deep at most, how hard it slides), and the speeds of its five push levels. A pairing with no row
//! does nothing (a blunt hit on wood or stone).
//!
//! The table's materials are the protocol's: flesh (and ethereal), leather, chainmail, plate (and the
//! world's metal and stone, which the tables list with plate), wood (and bone: hard, but cut and stabbed
//! like wood here), and earth (sand and dirt).

use rustvr_protocol::*;

/// The damage tables.
pub const MOD_SLASH: u32 = 1;
pub const MOD_SLASH_WEAK: u32 = 2;
pub const MOD_SLASH_HEAVY: u32 = 3;
pub const MOD_SLASH_AXE: u32 = 4;
pub const MOD_PIERCE: u32 = 5;
pub const MOD_PIERCE_FINE: u32 = 6;
pub const MOD_BLUNT: u32 = 7;
pub const MOD_BLUNT_LIGHT: u32 = 8;
pub const MOD_BLUNT_VERY_LIGHT: u32 = 9;
pub const MOD_PUNCH: u32 = 10;
pub const MOD_KICK: u32 = 11;
pub const MOD_ENERGY: u32 = 12;
/// A body slamming into something (flesh: what a fall or a throw does to it).
pub const MOD_RAGDOLL: u32 = 13;

const INF: f32 = f32::INFINITY;

/// One row of a table, for a material and a weapon tier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    /// The speed a hit needs to count at all (m/s), and the damage's multiplier.
    pub min_speed: f32,
    pub damage: f32,
    /// The speed it goes in at (infinite: it doesn't), the most it goes in (metres; infinite: as
    /// deep as the damager's own depth), and its dampers' multiplier.
    pub pen_speed: f32,
    pub depth_cap: f32,
    pub damper: f32,
    /// A pressure stab's force multiplier (0: none).
    pub pressure: f32,
    /// The push levels' speeds, and a thrown thing's (infinite: no such level).
    pub push: [f32; 5],
    pub push_thrown: [f32; 5],
    /// Knocks out above this speed (and thrown, above `knockout_thrown`; infinite: never), for this
    /// long against the standard 6 s stun.
    pub knockout: f32,
    pub knockout_thrown: f32,
    pub knockout_time: f32,
}

impl Row {
    const fn hit(min_speed: f32, damage: f32, push: [f32; 5]) -> Row {
        Row { min_speed, damage, pen_speed: INF, depth_cap: INF, damper: 1.0, pressure: 0.0, push, push_thrown: push, knockout: INF, knockout_thrown: INF, knockout_time: 0.0 }
    }

    const fn goes_in(self, pen_speed: f32, damper: f32, depth_cap: f32, pressure: f32) -> Row {
        Row { pen_speed, damper, depth_cap, pressure, ..self }
    }

    const fn thrown(self, push_thrown: [f32; 5]) -> Row {
        Row { push_thrown, ..self }
    }

    const fn knocks_out(self, at: f32, thrown: f32, time: f32) -> Row {
        Row { knockout: at, knockout_thrown: thrown, knockout_time: time, ..self }
    }

    /// The push level at this speed: the highest level whose speed it passes (the first speed
    /// passed is level 0, which pushes nothing).
    pub fn push_level(&self, speed: f32, thrown: bool) -> u32 {
        let levels = if thrown { &self.push_thrown } else { &self.push };
        levels.iter().rposition(|&v| speed > v).map_or(0, |k| k as u32)
    }
}

/// Push levels, padded with "never".
const fn p1(a: f32) -> [f32; 5] {
    [a, INF, INF, INF, INF]
}
const fn p2(a: f32, b: f32) -> [f32; 5] {
    [a, b, INF, INF, INF]
}
const NONE: [f32; 5] = [INF; 5];

/// The material's class in the tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mat {
    Flesh,
    Leather,
    Chainmail,
    Plate,
    Wood,
    Earth,
}

fn class(material: u32) -> Mat {
    match material {
        MAT_LEATHER => Mat::Leather,
        MAT_CHAINMAIL => Mat::Chainmail,
        MAT_PLATE | MAT_STONE => Mat::Plate,
        MAT_WOOD | MAT_BONE => Mat::Wood,
        MAT_EARTH => Mat::Earth,
        _ => Mat::Flesh,
    }
}

/// The row of table `modifier` for a hit on `material` by a weapon of `tier` (0..5), filled with
/// fire or not; None: there's none (the hit does nothing).
pub fn row(modifier: u32, material: u32, tier: u32, fire: bool) -> Option<Row> {
    let m = class(material);
    let t = tier.min(6);
    if fire && let Some(r) = fire_row(modifier, m) {
        return Some(r);
    }
    use Mat::*;
    Some(match (modifier, m) {
        // Slash: a sword's edge bites flesh at 3 m/s and goes in 3 mm (it rides the skin and
        // slides); leather halves it or more; chainmail, wood and plate stop it.
        (MOD_SLASH, Flesh) => Row::hit(3.0, 1.0, p2(3.0, 8.0)).goes_in(3.0, 1.0, 0.003, 1.0),
        (MOD_SLASH, Leather) => match t {
            0 => Row::hit(3.0, 0.25, p2(3.0, 9.0)),
            1 | 2 => Row::hit(3.0, 0.3, p2(3.0, 9.0)).goes_in(8.0, 3.0, 0.003, 0.0),
            _ => Row::hit(3.0, 0.35, p2(3.0, 9.0)).goes_in(5.0, 1.0, 0.003, 0.0),
        },
        (MOD_SLASH, Chainmail | Wood) => Row::hit(8.0, 0.1, p2(12.0, 15.0)),
        (MOD_SLASH, Plate) => Row::hit(8.0, 0.2, p2(8.0, 12.0)),
        (MOD_SLASH_WEAK, Flesh) => Row::hit(3.0, 1.0, p1(6.0)).goes_in(6.0, 1.0, 0.003, 1.0),
        (MOD_SLASH_WEAK, Leather) => Row::hit(6.0, [0.25, 0.3, 0.3, 0.35, 0.35, 0.35, 0.35][t as usize], p1(6.0)),
        (MOD_SLASH_WEAK, Chainmail | Wood) => Row::hit(12.0, 0.1, p1(12.0)),
        (MOD_SLASH_WEAK, Plate) => Row::hit(15.0, 0.2, p1(15.0)),
        (MOD_SLASH_HEAVY, Flesh) => Row::hit(3.0, 1.0, [3.0, 6.0, 9.0, 20.0, 24.0]).goes_in(3.0, 1.0, 0.003, 1.0),
        (MOD_SLASH_HEAVY, Leather) => match t {
            0 => Row::hit(3.0, 0.25, [3.0, 6.0, 10.0, 20.0, 24.0]),
            1 | 2 => Row::hit(3.0, 0.3, [3.0, 6.0, 10.0, 20.0, 24.0]).goes_in(6.0, 1.0, 0.003, 0.0),
            _ => Row::hit(3.0, 0.35, [3.0, 6.0, 10.0, 20.0, 24.0]).goes_in(3.0, 1.0, 0.003, 1.0),
        },
        (MOD_SLASH_HEAVY, Chainmail | Wood) => Row::hit(8.0, 0.1, [12.0, 15.0, 18.0, 20.0, 24.0]).thrown([12.0, 15.0, 18.0, 20.0, 14.0]),
        (MOD_SLASH_HEAVY, Plate) => Row::hit(6.0, 0.25, [7.0, 12.0, 15.0, 20.0, 24.0]),
        // An axe bites everything but plate, and sinks (no depth cap); better axes slide in easier.
        (MOD_SLASH_AXE, Flesh) => Row::hit(3.0, 1.0, [3.0, 6.0, 9.0, 14.0, 24.0]).goes_in(6.0, [6.0, 1.5, 1.5, 1.0, 1.0, 1.0, 1.0][t as usize], INF, 1.0),
        (MOD_SLASH_AXE, Leather) => match t {
            0 => Row::hit(4.0, 0.8, [4.0, 7.0, 10.0, 16.0, 24.0]),
            1 | 2 => Row::hit(3.0, 0.8, [4.0, 7.0, 10.0, 16.0, 24.0]).thrown([6.0, 7.0, 10.0, 16.0, 24.0]).goes_in(10.0, 1.6, INF, 1.0),
            _ => Row::hit(3.0, 0.8, [4.0, 7.0, 10.0, 16.0, 24.0]).goes_in(8.0, 1.4, INF, 1.0),
        },
        (MOD_SLASH_AXE, Chainmail) => match t {
            0 => Row::hit(4.0, 0.8, [4.0, 8.0, 12.0, 20.0, 24.0]),
            1 | 2 => Row::hit(3.0, 0.8, [4.0, 8.0, 12.0, 20.0, 24.0]).thrown([6.0, 8.0, 12.0, 20.0, 24.0]).goes_in(12.0, 2.0, INF, 0.0),
            _ => Row::hit(3.0, 0.8, [4.0, 8.0, 12.0, 20.0, 24.0]).thrown([6.0, 8.0, 12.0, 20.0, 24.0]).goes_in(10.0, 1.8, INF, 0.0),
        },
        (MOD_SLASH_AXE, Plate) => Row::hit(3.0, 0.8, [9.0, 12.0, 15.0, 20.0, 24.0]),
        (MOD_SLASH_AXE, Wood | Earth) => Row::hit(3.0, 0.8, NONE).goes_in(5.0, 0.5, INF, 1.0),
        // A stab goes into flesh at 1 m/s, as deep as the blade; into leather and chainmail
        // harder (chainmail only 20 cm), into wood at 3, into plate and stone almost never.
        (MOD_PIERCE, Flesh) => {
            let damper = [3.0, 1.5, 1.0, 0.75, 0.5, 0.5, 0.5][t as usize];
            let push = if t >= 3 { p2(6.0, 18.0) } else { p2(6.0, 15.0) };
            Row::hit(1.0, 1.0, push).thrown(p2(3.0, 6.0)).goes_in(if t == 1 { 0.5 } else { 1.0 }, damper, INF, 2.0)
        }
        (MOD_PIERCE, Leather) => match t {
            0 => Row::hit(3.0, 1.0, p2(6.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(6.0, 6.0, 0.1, 0.0),
            1 | 2 => Row::hit(3.0, 1.0, p2(6.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(3.0, 2.5, INF, 1.0),
            _ => Row::hit(3.0, 1.0, p2(6.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(3.0, 1.25, INF, 1.0),
        },
        (MOD_PIERCE, Chainmail) => match t {
            0 => Row::hit(3.0, 0.1, p2(9.0, 18.0)).thrown(p2(3.0, 6.0)),
            1 | 2 => Row::hit(3.0, 1.0, p2(9.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(3.0, 6.0, 0.2, 0.0),
            _ => Row::hit(3.0, 1.0, p2(9.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(3.0, 4.0, 0.2, 0.0),
        },
        (MOD_PIERCE, Wood) => Row::hit(3.0, 1.0, p1(9.0)).goes_in(3.0, 2.0, INF, 0.0),
        (MOD_PIERCE, Plate) => Row::hit(12.0, 0.05, NONE).goes_in(50.0, 12.0, INF, 0.0),
        (MOD_PIERCE, Earth) => Row::hit(6.0, 1.0, NONE).goes_in(6.0, 1.0, 0.3, 0.0),
        (MOD_PIERCE_FINE, Flesh) => {
            let damper = [3.0, 2.0, 1.5, 1.0, 0.75, 0.75, 0.75][t as usize];
            let push = if t >= 3 { p2(6.0, 18.0) } else { p2(6.0, 15.0) };
            Row::hit(1.0, 1.0, push).thrown(p2(3.0, 6.0)).goes_in(1.0, damper, INF, 1.0)
        }
        (MOD_PIERCE_FINE, Leather) => match t {
            0 => Row::hit(2.0, 0.1, p2(6.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(4.0, 6.0, 0.1, 0.0),
            1 | 2 => Row::hit(2.0, 1.0, p2(6.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(2.0, 3.0, INF, 1.0),
            _ => Row::hit(2.0, 1.0, p2(6.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(2.0, 1.25, INF, 1.0),
        },
        (MOD_PIERCE_FINE, Chainmail) => match t {
            0 => Row::hit(3.0, 0.1, p2(9.0, 18.0)).thrown(p2(3.0, 6.0)),
            1 | 2 => Row::hit(3.0, 1.0, p2(9.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(3.0, 3.0, 0.5, 0.0),
            _ => Row::hit(3.0, 1.0, p2(9.0, 18.0)).thrown(p2(3.0, 6.0)).goes_in(3.0, 2.0, 0.5, 1.0),
        },
        (MOD_PIERCE_FINE, Wood) => Row::hit(3.0, 1.0, p1(9.0)).goes_in(3.0, 2.0, INF, 0.0),
        (MOD_PIERCE_FINE, Plate) => Row::hit(28.0, 0.05, NONE),
        (MOD_PIERCE_FINE, Earth) => Row::hit(3.0, 1.0, NONE).goes_in(3.0, 1.0, 0.3, 1.0),
        // Blunt: flesh from 1 m/s; armour from 4 at half or two-thirds. These rows carry the
        // knockout's speeds (see `docs/MELEE.md`).
        (MOD_BLUNT, Flesh) => Row::hit(1.0, 1.0, [4.0, 7.5, 9.0, 12.0, 24.0]).knocks_out(4.0, 4.0, 1.0),
        (MOD_BLUNT, Leather) => Row::hit(4.0, 0.5, [8.0, 10.0, 12.0, 16.0, 24.0]).knocks_out(14.0, 16.0, 0.5),
        (MOD_BLUNT, Chainmail) => Row::hit(4.0, 0.5, [8.0, 10.0, 12.0, 16.0, 24.0]).knocks_out(12.0, 12.0, 0.8),
        (MOD_BLUNT, Plate) if t == 0 => Row::hit(4.0, 0.5, [9.0, 13.0, 16.0, 19.0, 24.0]).knocks_out(14.0, 14.0, 1.2),
        (MOD_BLUNT, Plate) => Row::hit(4.0, 0.66, [9.0, 11.0, 13.0, 16.0, 24.0]).knocks_out(14.0, 14.0, 1.2),
        (MOD_BLUNT_LIGHT, Flesh) => Row::hit(4.0, 1.0, [7.0, 9.0, 12.0, 24.0, 28.0]).thrown([7.0, 9.0, 10.0, 24.0, 28.0]),
        (MOD_BLUNT_LIGHT, Leather) => Row::hit(6.0, 0.5, [12.0, 16.0, 18.0, 24.0, 28.0]),
        (MOD_BLUNT_LIGHT, Chainmail) => Row::hit(6.0, 0.5, [12.0, 14.0, 16.0, 24.0, 28.0]),
        (MOD_BLUNT_LIGHT, Plate) => Row::hit(4.0, 0.66, [14.0, 16.0, 20.0, 24.0, 24.0]).thrown([14.0, 16.0, 20.0, 28.0, 28.0]),
        (MOD_BLUNT_VERY_LIGHT, Flesh) => Row::hit(3.0, 1.0, p1(7.0)),
        (MOD_BLUNT_VERY_LIGHT, Leather | Chainmail) => Row::hit(6.0, 0.5, p1(12.0)),
        (MOD_BLUNT_VERY_LIGHT, Plate) => Row::hit(6.0, 0.75, p1(14.0)),
        // A fist from 2 m/s at half; a kick from 1. Thrown, neither pushes (a thrown fist is a
        // thrown body).
        (MOD_PUNCH, Flesh) => Row::hit(2.0, 0.5, [2.0, 4.0, 6.0, 12.0, 24.0]).thrown(NONE),
        (MOD_PUNCH, Leather) => Row::hit(3.0, 0.4, [3.0, 9.0, 12.0, 18.0, 24.0]).thrown(NONE),
        (MOD_PUNCH, Chainmail) => Row::hit(4.5, 0.3, [3.0, 9.0, 12.0, 24.0, 26.0]).thrown(NONE),
        (MOD_PUNCH, Plate) => Row::hit(4.5, 0.2, [4.0, 9.0, 20.0, 24.0, 26.0]).thrown(NONE),
        (MOD_KICK, Flesh) => Row::hit(1.0, 1.0, [1.0, 1.5, 2.0, 3.0, 18.0]).thrown(NONE).knocks_out(6.0, 6.0, 1.0),
        (MOD_KICK, Leather) => Row::hit(3.0, 0.4, [1.0, 2.0, 3.0, 4.0, 18.0]).thrown(NONE).knocks_out(14.0, 16.0, 0.5),
        (MOD_KICK, Chainmail) => Row::hit(2.0, 0.75, [1.0, 2.0, 4.0, 6.0, 18.0]).thrown(NONE).knocks_out(12.0, 12.0, 0.8),
        (MOD_KICK, Plate) => Row::hit(6.0, 1.5, [1.0, 2.0, 5.0, 7.0, 18.0]).thrown(NONE).knocks_out(14.0, 14.0, 1.2),
        (MOD_ENERGY, _) => Row::hit(1.0, 1.0, [1.0, 2.0, 4.0, 6.0, 24.0]).knocks_out(6.0, 6.0, 1.0),
        // A body slamming into the world or another (falls, throws): from 5 m/s.
        (MOD_RAGDOLL, _) => Row::hit(5.0, 1.0, [3.0, 4.5, 6.0, 12.0, 24.0]).knocks_out(3.0, INF, 1.0),
        _ => return None,
    })
}

/// The fire-imbued rows, checked first for a weapon filled with fire: a burning blade bites at half
/// the speed and sinks easier.
fn fire_row(modifier: u32, m: Mat) -> Option<Row> {
    use Mat::*;
    Some(match (modifier, m) {
        (MOD_SLASH, Flesh) => Row::hit(1.5, 1.0, p2(3.0, 8.0)).goes_in(1.5, 0.5, 0.003, 1.0),
        (MOD_SLASH, Leather) => Row::hit(1.5, 0.5, p2(3.0, 9.0)).goes_in(3.0, 1.0, 0.003, 1.0),
        (MOD_SLASH, Chainmail | Wood) => Row::hit(3.0, 0.5, p2(12.0, 15.0)).goes_in(3.0, 1.0, 0.003, 1.0),
        (MOD_SLASH, Plate) => Row::hit(4.0, 0.5, p2(8.0, 12.0)).goes_in(4.0, 1.0, 0.003, 1.0),
        (MOD_SLASH_HEAVY, Flesh) => Row::hit(1.5, 1.0, [3.0, 6.0, 9.0, 20.0, 24.0]).goes_in(1.5, 0.5, 0.003, 1.0),
        (MOD_SLASH_AXE, Flesh) => Row::hit(3.0, 1.0, [3.0, 6.0, 9.0, 14.0, 24.0]).goes_in(3.0, 0.9, INF, 1.0),
        (MOD_SLASH_AXE, Plate) => Row::hit(3.0, 0.8, [9.0, 12.0, 15.0, 20.0, 24.0]).goes_in(4.5, 3.0, INF, 1.0),
        (MOD_PIERCE | MOD_PIERCE_FINE, Flesh) => Row::hit(1.0, 1.0, p2(6.0, 15.0)).goes_in(1.0, 1.0, INF, 1.0),
        (MOD_PIERCE | MOD_PIERCE_FINE, Chainmail) => Row::hit(1.0, 1.0, p2(9.0, 18.0)).goes_in(1.0, 4.0, INF, 1.0),
        (MOD_PIERCE | MOD_PIERCE_FINE, Plate) => Row::hit(1.0, 1.0, NONE).goes_in(1.0, 8.0, INF, 1.0),
        (MOD_BLUNT, Flesh) => Row::hit(1.0, 1.2, [4.0, 7.5, 9.0, 12.0, 24.0]).knocks_out(4.0, 4.0, 1.5),
        (MOD_BLUNT, Leather | Chainmail) => Row::hit(4.0, 0.6, [8.0, 10.0, 12.0, 16.0, 24.0]),
        (MOD_BLUNT, Plate) => Row::hit(4.0, 0.75, [9.0, 11.0, 13.0, 16.0, 24.0]),
        _ => return None,
    })
}
