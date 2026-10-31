//! The arena's fighters' brains: how each attacks, defends, takes a hit, arms itself and acts when
//! held, in stances by what its hands hold, at five tiers of difficulty. A brain animates its
//! body (`crate::anim`'s pose, which the body's pins follow: `crate::ragdoll`), so everything it does
//! is physical: its swings are its weapon swung by its arm, which the player's weapon meets, blocks
//! and deflects; its guard is its weapon or shield put in the way of the player's.
//!
//! * **Stances**, by what its hands hold: one-handed (a weapon, the other hand free), dual wield (a
//!   weapon in each hand) and sword and shield. Each stance stands in one of three idles (simple,
//!   thrusting, high), and an idle has its own opening moves.
//! * **Attacking**: the stance's moves, each with its own numbers (where it lands, when it hurts,
//!   how long it takes, how far it carries the body forward), those its difficulty allows, the one
//!   that best fits where the player will be at its peak; no sooner than its delay after its last
//!   attack's end (Easy 2 to 3 s, Medium 0.5 to 1, Hard none), no more of the wave at the player at
//!   once than its tier allows (`melee_max`), and held off by danger. It swings slower with a
//!   heavier weapon (`1.5 / mass`, down to 70%).
//! * **Parried** (its swing met the player's weapon in its damage window): a roll: a chained
//!   follow-up (a combo), nothing (the swing plays back for 0.2 s and it carries on), or a recoil (a
//!   full stagger), the odds by difficulty and how many it has chained.
//! * **Defending**: it watches the player's weapons; on a course for it within reach, it puts its
//!   guard in the way, blending there at 7 a second, its arm pinned three times as stiff; when its
//!   guard doesn't cover the blow (or the blow is a thrust and it holds no shield), it dodges back.
//!   Danger makes it hold off attacking. A blow it blocked may knock it about (a heavy weapon's
//!   push), or it ripostes, does nothing, or recoils.
//! * **Disarmed**, it goes for the nearest weapon lying loose and not near the player, stoops and
//!   picks it up; with none, it runs for the way out and is gone (its only fear: unarmed with
//!   nothing to take up).
//! * **Grappled**: held standing it stops fighting, turns to the hand holding it, steps after its
//!   head and crouches as it's pressed down; a held arm goes where the hand has it, and held by the
//!   throat it claws at its neck.

use glam::{Quat, Vec2, Vec3};
use rustvr_melee::body::Doing;

use crate::anim::{self, Pose};

/// How hard a fighter is: `Easy`, `Medium`, `Hard`, and the armoured `MediumHeavy` and
/// `HardHeavy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Easy,
    Medium,
    Hard,
    MediumHeavy,
    HardHeavy,
}

/// What a tier changes (every field that differs between the tiers).
#[derive(Clone, Copy, Debug)]
pub struct Difficulty {
    /// Its moves are filtered by this (`difficulty`: Easy 1, Medium 2, Hard 3).
    pub level: u32,
    /// Seconds after its last attack's end before the next (checked only when its least is over 0,
    /// so Hard's 0 to 0.5 never holds it back).
    pub between: (f32, f32),
    /// Its swings' speed.
    pub speed: f32,
    /// At most this many of its side at the player at once, the rest keeping this far off (m).
    pub melee_max: usize,
    pub safe: f32,
    /// A hit doing this share of its health cancels its attack.
    pub cancel: f32,
    /// The parry roll: under the first it recoils, under the second it does nothing, over that it
    /// chains or ripostes; both raised this much per swing chained.
    pub roll: (f32, f32),
    pub roll_per_chain: (f32, f32),
    /// Between ripostes (s), and each chained one slower by this.
    pub riposte_delay: (f32, f32),
    pub chain_speed: f32,
    /// Its recovery from a stagger, and between recoils (s).
    pub recovery: f32,
    pub recoil_cooldown: f32,
    /// Strafing round the player, every so often (least and most, s), backing off and strafing
    /// this much slower than walking.
    pub strafe: (f32, f32),
    pub backward: f32,
    pub sideways: f32,
    /// Its chance to dodge by the share of its health lost, at none, 60% and all of it (straight
    /// between), and how fast it dodges.
    pub dodge: [f32; 3],
    pub dodge_speed: f32,
    /// The push level a hit on its hand needs to disarm it, and a hit on an arm whose weapon the
    /// player holds.
    pub disarm: u32,
    /// Its push tables are the armoured brains', a step tougher.
    pub heavy: bool,
}

impl Tier {
    pub fn difficulty(self) -> Difficulty {
        let easy = Difficulty {
            level: 1,
            between: (2.0, 3.0),
            speed: 1.0,
            melee_max: 1,
            safe: 3.75,
            cancel: 0.05,
            roll: (0.30, 0.35),
            roll_per_chain: (0.10, 0.15),
            riposte_delay: (1.5, 2.0),
            chain_speed: -0.05,
            recovery: 0.9,
            recoil_cooldown: 1.0,
            strafe: (2.0, 6.0),
            backward: 0.7,
            sideways: 0.8,
            dodge: [0.5, 1.0, 0.0],
            dodge_speed: 1.0,
            disarm: 2,
            heavy: false,
        };
        let medium = Difficulty {
            level: 2,
            between: (0.5, 1.0),
            speed: 1.05,
            melee_max: 2,
            safe: 3.5,
            cancel: 0.08,
            roll: (0.10, 0.15),
            roll_per_chain: (0.05, 0.12),
            riposte_delay: (0.5, 1.2),
            chain_speed: -0.02,
            recovery: 1.0,
            recoil_cooldown: 1.5,
            strafe: (1.0, 4.0),
            backward: 0.8,
            sideways: 0.9,
            ..easy
        };
        let hard = Difficulty {
            level: 3,
            between: (0.0, 0.5),
            melee_max: 3,
            safe: 3.25,
            cancel: 0.10,
            roll: (0.0, 0.05),
            roll_per_chain: (0.03, 0.11),
            riposte_delay: (0.5, 1.0),
            chain_speed: -0.01,
            recovery: 1.1,
            recoil_cooldown: 2.0,
            strafe: (0.0, 2.0),
            backward: 0.9,
            sideways: 1.0,
            ..medium
        };
        // The armoured brains: their tier's, swinging at 1.0, dodging less and slower, disarmed
        // only at push level 3, and a step tougher to stagger.
        let armoured = |d: Difficulty| Difficulty { speed: 1.0, dodge: [0.0, 0.6, 0.0], dodge_speed: 0.85, disarm: 3, heavy: true, ..d };
        match self {
            Tier::Easy => easy,
            Tier::Medium => medium,
            Tier::Hard => hard,
            Tier::MediumHeavy => armoured(medium),
            Tier::HardHeavy => armoured(hard),
        }
    }

    /// Its push table (Hard's is Medium's).
    pub fn table(self) -> rustvr_melee::body::Table {
        use rustvr_melee::body::Table;
        match self {
            Tier::Easy => Table::Easy,
            Tier::Medium | Tier::Hard => Table::Medium,
            Tier::MediumHeavy => Table::MediumHeavy,
            Tier::HardHeavy => Table::HardHeavy,
        }
    }

    /// How often it feints (the user asked for feints, `docs/MELEE.md`).
    fn feints(self) -> f32 {
        match self {
            Tier::Easy => 0.0,
            Tier::Medium | Tier::MediumHeavy => 0.1,
            Tier::Hard | Tier::HardHeavy => 0.2,
        }
    }
}

/// A fighter's stance, by what is in the off hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    OneHanded,
    DualWield,
    SwordShield,
}

/// The idle a stance stands in (`SimpleStance`, `ThrustingStance`, `HighStance`): its guard, and
/// which moves it can open with from there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    Simple,
    Thrusting,
    High,
}

/// The idle a move leaves it in: the simple one, another one, or any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    Simple,
    Other,
    Any,
}

/// Which hand a move swings: the left, the right, or both at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Both,
}

impl Side {
    /// Hand `hand` (0 left, 1 right) swings in it.
    pub fn has(self, hand: usize) -> bool {
        matches!((self, hand), (Side::Both, _) | (Side::Left, 0) | (Side::Right, 1))
    }
}

/// A key of a move's shape: when (its share of the shape), where the wrist is (the person's frame,
/// facing +Z), which way the blade points and which way its edge faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    t: f32,
    wrist: Vec3,
    blade: Vec3,
    edge: Vec3,
}

const fn k(t: f32, wrist: [f32; 3], blade: [f32; 3], edge: [f32; 3]) -> Key {
    Key { t, wrist: Vec3::from_array(wrist), blade: Vec3::from_array(blade), edge: Vec3::from_array(edge) }
}

impl Key {
    /// The weapon's turn its blade and edge make (its +Z along the blade, +X along its edge).
    fn turn(&self) -> Quat {
        let blade = self.blade.normalize_or(Vec3::Z);
        let edge = (self.edge - blade * self.edge.dot(blade)).normalize_or(blade.any_orthonormal_vector());
        Quat::from_mat3(&glam::Mat3::from_cols(edge, blade.cross(edge), blade)).normalize()
    }

    fn with_turn(self, q: Quat) -> Key {
        Key { blade: q * Vec3::Z, edge: q * Vec3::X, ..self }
    }

    /// Partway to `to`: its wrist along the way, its weapon turned the short way round (blending
    /// its blade's direction instead would flip it over as it passes half way between two far
    /// apart).
    fn lerp(self, to: Key, s: f32) -> Key {
        Key { t: to.t, wrist: self.wrist.lerp(to.wrist, s), ..self }.with_turn(self.turn().slerp(to.turn(), s))
    }
}

/// The guards of the idles (the right hand's; the left mirrors them): the blade up and forward, held
/// level to thrust, raised high.
const GUARD: Key = k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]);
const GUARD_THRUST: Key = k(0.0, [0.25, 1.15, 0.22], [0.0, 0.12, 1.0], [-1.0, 0.0, 0.0]);
const GUARD_HIGH: Key = k(0.0, [0.3, 1.5, 0.15], [0.05, 0.85, 0.45], [-1.0, 0.0, 0.0]);
/// A shield's guard: in front of the chest, its face forward.
const SHIELD_GUARD: Key = k(0.0, [0.2, 1.25, 0.35], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
/// An empty hand at its side.
const REST: Key = k(0.0, [0.28, 0.95, 0.1], [0.0, -0.3, 1.0], [0.0, 1.0, 0.0]);

impl Idle {
    fn guard(self) -> Key {
        match self {
            Idle::Simple => GUARD,
            Idle::Thrusting => GUARD_THRUST,
            Idle::High => GUARD_HIGH,
        }
    }
}

/// A move's shape: the right hand's keys through it (the left mirrors them), and where in them it's
/// wound up, strikes and has followed through (each move's own marks are laid onto these).
pub struct Shape {
    keys: &'static [Key],
    marks: [f32; 3],
    /// The keys it passes through at speed (its strikes); it comes to rest at the others (its guard,
    /// the top of its wind-up, the end of its follow-through).
    strikes: &'static [usize],
}

/// A swing from the right across to the left.
static RTL: Shape = Shape {
    keys: &[
        GUARD,
        k(0.35, [0.5, 1.5, -0.05], [0.35, 0.55, -0.75], [-0.5, 0.0, 0.85]),
        k(0.55, [0.12, 1.35, 0.5], [-0.25, 0.1, 1.0], [-1.0, 0.0, -0.25]),
        k(0.75, [-0.3, 1.15, 0.3], [-0.85, -0.15, 0.5], [-0.5, -0.1, -0.85]),
        k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};
/// From the left across to the right.
static LTR: Shape = Shape {
    keys: &[
        GUARD,
        k(0.35, [-0.25, 1.45, 0.05], [-0.5, 0.55, -0.65], [0.6, 0.0, 0.8]),
        k(0.55, [0.05, 1.3, 0.5], [0.3, 0.05, 0.95], [1.0, 0.0, -0.2]),
        k(0.75, [0.45, 1.15, 0.25], [0.85, -0.2, 0.45], [0.45, -0.1, -0.85]),
        k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};
/// From above on the right down to the left.
static TTL: Shape = Shape {
    keys: &[
        GUARD_HIGH,
        k(0.35, [0.35, 1.85, -0.05], [0.25, 0.5, -0.8], [-0.2, 0.85, 0.45]),
        k(0.55, [0.12, 1.4, 0.5], [-0.3, -0.15, 0.95], [-0.5, -0.8, -0.2]),
        k(0.75, [-0.25, 1.0, 0.35], [-0.7, -0.6, 0.35], [-0.45, -0.75, -0.45]),
        k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};
/// From above on the left down to the right.
static TTR: Shape = Shape {
    keys: &[
        GUARD_HIGH,
        k(0.35, [-0.1, 1.85, -0.05], [-0.25, 0.5, -0.8], [0.2, 0.85, 0.45]),
        k(0.55, [0.15, 1.4, 0.5], [0.3, -0.15, 0.95], [0.5, -0.8, -0.2]),
        k(0.75, [0.45, 1.0, 0.3], [0.7, -0.6, 0.35], [0.45, -0.75, -0.45]),
        k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};
/// Low from the right across to the left, rising.
static LOW_RTL: Shape = Shape {
    keys: &[
        GUARD,
        k(0.35, [0.5, 1.0, 0.0], [0.5, -0.3, -0.8], [-0.6, 0.2, 0.75]),
        k(0.55, [0.1, 1.05, 0.5], [-0.3, -0.25, 0.95], [-1.0, 0.1, -0.2]),
        k(0.75, [-0.3, 1.15, 0.3], [-0.85, 0.1, 0.5], [-0.5, 0.1, -0.85]),
        k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};
/// Low from the left across to the right.
static LOW_LTR: Shape = Shape {
    keys: &[
        GUARD,
        k(0.35, [-0.2, 1.0, 0.05], [-0.5, -0.3, -0.75], [0.6, 0.2, 0.75]),
        k(0.55, [0.1, 1.05, 0.5], [0.3, -0.25, 0.95], [1.0, 0.1, -0.2]),
        k(0.75, [0.45, 1.1, 0.25], [0.85, 0.1, 0.45], [0.45, 0.1, -0.85]),
        k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};
/// Point first, straight out.
static THRUST: Shape = Shape {
    keys: &[
        GUARD_THRUST,
        k(0.35, [0.3, 1.3, -0.1], [0.0, 0.05, 1.0], [-1.0, 0.0, 0.0]),
        k(0.55, [0.12, 1.32, 0.56], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]),
        k(0.75, [0.2, 1.3, 0.3], [0.0, 0.1, 1.0], [-1.0, 0.0, 0.0]),
        k(1.0, [0.25, 1.15, 0.22], [0.0, 0.12, 1.0], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};
/// The one-handed combo: from the right across, then back from the left.
static COMBO: Shape = Shape {
    keys: &[
        GUARD,
        k(0.16, [0.5, 1.5, -0.05], [0.35, 0.55, -0.75], [-0.5, 0.0, 0.85]),
        k(0.3, [0.12, 1.35, 0.5], [-0.25, 0.1, 1.0], [-1.0, 0.0, -0.25]),
        k(0.42, [-0.3, 1.4, 0.15], [-0.6, 0.5, -0.6], [0.6, 0.0, 0.8]),
        k(0.56, [0.05, 1.3, 0.5], [0.3, 0.05, 0.95], [1.0, 0.0, -0.2]),
        k(0.7, [0.45, 1.15, 0.25], [0.85, -0.2, 0.45], [0.45, -0.1, -0.85]),
        k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
    ],
    marks: [0.16, 0.42, 0.7],
    strikes: &[2, 4],
};
/// A shield shoved out from the chest (its face forward).
static PUSH: Shape = Shape {
    keys: &[
        SHIELD_GUARD,
        k(0.35, [0.22, 1.25, 0.15], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        k(0.55, [0.12, 1.3, 0.56], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        k(0.75, [0.15, 1.28, 0.45], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        k(1.0, [0.2, 1.25, 0.35], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ],
    marks: [0.35, 0.55, 0.75],
    strikes: &[2],
};

impl Shape {
    /// Where the wrist is and how the blade lies at `t` (0..1), for hand `hand` (the left mirrored):
    /// a cubic through the keys, coming to rest at each but its strikes, which it passes through at
    /// speed (the way between its neighbours), so a blow doesn't stop where it lands.
    fn at(&self, t: f32, hand: usize) -> Key {
        let t = t.clamp(0.0, 1.0);
        let n = self.keys.len();
        let i = self.keys.iter().rposition(|k| k.t <= t).unwrap_or(0).min(n - 2);
        let (a, b) = (self.keys[i], self.keys[i + 1]);
        let s = ((t - a.t) / (b.t - a.t).max(1e-3)).clamp(0.0, 1.0);
        let strike = |j: usize| self.strikes.contains(&j);
        let slope = |j: usize| if strike(j) { (self.keys[(j + 1).min(n - 1)].wrist - self.keys[j.saturating_sub(1)].wrist) * 0.5 } else { Vec3::ZERO };
        let (s2, s3) = (s * s, s * s * s);
        let wrist = (2.0 * s3 - 3.0 * s2 + 1.0) * a.wrist + (s3 - 2.0 * s2 + s) * slope(i) + (3.0 * s2 - 2.0 * s3) * b.wrist + (s3 - s2) * slope(i + 1);
        // The weapon turns the short way from key to key, eased the same way.
        let ease = (s3 - 2.0 * s2 + s) * f32::from(u8::from(strike(i))) + (3.0 * s2 - 2.0 * s3) + (s3 - s2) * f32::from(u8::from(strike(i + 1)));
        mirror(Key { t, wrist, ..a }.with_turn(a.turn().slerp(b.turn(), ease)), hand)
    }
}

/// A move and its numbers.
pub struct Move {
    pub name: &'static str,
    shape: &'static Shape,
    pub side: Side,
    /// Where it lands (least and most, m between its feet and the target's, past its weapon's
    /// reach when `reach`).
    pub range: (f32, f32),
    reach: bool,
    /// It sweeps through (a thrust, a shield push): it fits a target nearer than its far reach.
    sweep: bool,
    /// A shield push's shove on what it meets (m/s; 0 none).
    pub push: f32,
    /// When it can hurt, when it peaks and when it stops (shares of it: `start`, `peak`, `end`).
    pub start: f32,
    pub peak: f32,
    pub end: f32,
    /// Its clip's length (s) and its own speed.
    pub seconds: f32,
    speed: f32,
    /// How far it carries its body forward (its root's curve, m): the furthest, when (a share of
    /// it), and where it ends.
    root: (f32, f32, f32),
    pub difficulty: u32,
    weight: f32,
    skip: f32,
    /// Parried, it may chain into another.
    pub chains: bool,
    from: Option<Idle>,
    then: Then,
}

/// A move is wound up this far through to its blow's start.
const WOUND_UP: f32 = 0.6;

/// The chains' and ripostes' length: the most common swing's 1.33 s, leaning in as the plain swings
/// do.
const CLIP: f32 = 1.33;
const LEAN: (f32, f32, f32) = (0.22, 0.4, 0.0);

#[allow(clippy::too_many_arguments)]
const fn mv(
    name: &'static str,
    shape: &'static Shape,
    side: Side,
    range: (f32, f32),
    reach: bool,
    sweep: bool,
    push: f32,
    (start, peak, end): (f32, f32, f32),
    seconds: f32,
    speed: f32,
    root: (f32, f32, f32),
    difficulty: u32,
    weight: f32,
    skip: f32,
    chains: bool,
    from: Option<Idle>,
    then: Then,
) -> Move {
    Move { name, shape, side, range, reach, sweep, push, start, peak, end, seconds, speed, root, difficulty, weight, skip, chains, from, then }
}

/// The one-handed stance's moves: swings from the right, the left and above, their stepping and
/// lunging versions, a thrust and a lunging one, and the combo; its counters, and a riposte for
/// each guard.
pub static ONE_OPENING: [Move; 12] = [
    mv("SwingRightToLeft", &RTL, Side::Right, (1.05, 1.05), true, false, 0.0, (0.237, 0.325, 0.500), 1.33, 1.0, (0.24, 0.40, 0.00), 1, 1.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("SwingRightToLeftStep", &RTL, Side::Right, (2.0, 2.0), true, false, 0.0, (0.287, 0.412, 0.550), 1.33, 1.0, (0.62, 0.41, 0.35), 1, 1.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("SwingLeftToRight", &LTR, Side::Right, (1.05, 1.05), true, false, 0.0, (0.233, 0.344, 0.500), 1.50, 1.0, (0.22, 0.34, 0.00), 1, 1.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("SwingLeftToRightStep", &LTR, Side::Right, (2.0, 2.0), true, false, 0.0, (0.187, 0.321, 0.491), 1.87, 1.0, (1.07, 0.56, 1.03), 1, 1.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("SwingCombo", &COMBO, Side::Right, (2.0, 2.0), true, false, 0.0, (0.168, 0.460, 0.571), 2.68, 1.0, (1.39, 0.78, 1.25), 2, 1.0, 0.0, false, Some(Idle::Simple), Then::Simple),
    mv("Thrust", &THRUST, Side::Right, (1.0, 1.2), true, true, 0.0, (0.233, 0.467, 0.550), 1.00, 1.0, (0.25, 0.48, 0.00), 3, 1.0, 0.0, true, Some(Idle::Thrusting), Then::Other),
    mv("LungeThrust", &THRUST, Side::Right, (2.0, 2.2), true, true, 0.0, (0.141, 0.341, 0.400), 1.42, 1.0, (1.25, 1.00, 1.25), 3, 1.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
    mv("SwingTopToLeft", &TTL, Side::Right, (1.1, 1.1), true, false, 0.0, (0.150, 0.250, 0.375), 1.33, 1.0, (0.30, 0.35, 0.00), 1, 1.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("SwingTopToLeftStep", &TTL, Side::Right, (1.9, 1.9), true, false, 0.0, (0.169, 0.347, 0.551), 1.97, 1.0, (1.83, 0.84, 1.79), 2, 1.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("LungeSwingTopToLeft", &TTL, Side::Right, (2.55, 2.55), true, false, 0.0, (0.267, 0.362, 0.543), 1.93, 1.0, (1.07, 0.41, 0.73), 1, 1.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("SwingTopToRight", &TTR, Side::Right, (1.1, 1.1), true, false, 0.0, (0.138, 0.263, 0.400), 1.33, 1.0, (0.24, 0.27, 0.00), 1, 1.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("LungeSwingTopToRight", &TTR, Side::Right, (2.55, 2.55), true, false, 0.0, (0.235, 0.341, 0.455), 2.20, 1.0, (1.16, 0.36, 0.73), 1, 1.0, 0.0, true, Some(Idle::High), Then::Simple),
];
static ONE_CHAIN: [Move; 5] = [
    mv("CounterSwingRight", &RTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.380, 0.460, 0.620), CLIP, 1.0, LEAN, 1, 1.0, 0.0, true, None, Then::Other),
    mv("CounterSwingLeft", &LTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.250, 0.360, 0.530), CLIP, 1.0, LEAN, 1, 1.0, 0.0, true, None, Then::Other),
    mv("CounterSwingHighRight", &TTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.300, 0.400, 0.600), CLIP, 1.0, LEAN, 2, 1.0, 0.0, true, None, Then::Other),
    mv("CounterSwingHighLeft", &TTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.250, 0.350, 0.450), CLIP, 1.0, LEAN, 2, 1.0, 0.0, true, None, Then::Other),
    mv("CounterThrust", &THRUST, Side::Right, (0.0, 0.0), true, false, 0.0, (0.435, 0.635, 0.706), CLIP, 0.8, LEAN, 3, 1.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
];
static ONE_RIPOSTE: [Move; 6] = [
    mv("RiposteHighRight", &TTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.125, 0.250, 0.437), CLIP, 1.0, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteHighLeft", &TTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.144, 0.233, 0.367), CLIP, 0.9, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteRight", &RTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.224, 0.303, 0.500), CLIP, 0.9, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteLeft", &LTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.194, 0.265, 0.449), CLIP, 1.0, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteLowRight", &LOW_RTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.224, 0.3178, 0.505), CLIP, 0.9, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteLowLeft", &LOW_LTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.153, 0.243, 0.333), CLIP, 0.9, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
];
/// Dual wield: each hand's swings mid, low and high, its thrusts, and the cross slash with both;
/// chains alternate hands.
static DUAL_OPENING: [Move; 9] = [
    mv("MidLeftSwing", &RTL, Side::Left, (1.35, 1.45), true, false, 0.0, (0.386, 0.543, 0.700), 1.17, 0.85, (0.88, 0.66, 0.36), 1, 1.0, 0.0, true, None, Then::Any),
    mv("MidRightSwing", &RTL, Side::Right, (0.95, 1.05), true, false, 0.0, (0.257, 0.333, 0.524), 1.75, 0.85, (0.21, 0.31, 0.00), 1, 1.0, 0.0, true, None, Then::Any),
    mv("LowLeftSwing", &LOW_RTL, Side::Left, (1.35, 1.45), true, false, 0.0, (0.312, 0.475, 0.600), 1.33, 0.85, (0.94, 0.50, 0.10), 1, 1.0, 0.0, true, Some(Idle::Simple), Then::Any),
    mv("LowRightSwing", &LOW_RTL, Side::Right, (1.05, 1.15), true, false, 0.0, (0.250, 0.370, 0.500), 1.67, 0.85, (0.28, 0.36, 0.01), 1, 1.0, 0.0, true, Some(Idle::Simple), Then::Any),
    mv("ThrustLeft", &THRUST, Side::Left, (1.25, 1.35), true, true, 0.0, (0.275, 0.525, 0.650), 1.33, 0.85, (0.75, 0.54, 0.0), 2, 1.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
    mv("ThrustRight", &THRUST, Side::Right, (1.25, 1.35), true, true, 0.0, (0.225, 0.375, 0.500), 1.33, 0.85, (0.45, 0.40, 0.0), 2, 1.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
    mv("HighLeftSwing", &TTL, Side::Left, (1.35, 1.45), true, false, 0.0, (0.357, 0.543, 0.714), 1.17, 0.85, (0.91, 0.61, 0.16), 1, 1.0, 0.0, true, Some(Idle::High), Then::Any),
    mv("HighRightSwing", &TTL, Side::Right, (0.8, 0.9), true, false, 0.0, (0.386, 0.514, 0.671), 1.17, 0.85, (0.25, 0.69, 0.04), 1, 1.0, 0.0, true, Some(Idle::High), Then::Any),
    mv("CrossSlash", &RTL, Side::Both, (1.45, 1.5), true, false, 0.0, (0.250, 0.362, 0.512), 1.33, 0.8, (0.81, 0.56, 0.00), 2, 1.0, 0.15, true, Some(Idle::Simple), Then::Other),
];
static DUAL_CHAIN: [Move; 9] = [
    mv("ChainMidLeftSwing", &RTL, Side::Left, (1.05, 1.15), true, false, 0.0, (0.386, 0.543, 0.700), CLIP, 0.85, LEAN, 1, 1.0, 0.0, true, None, Then::Other),
    mv("ChainMidRightSwing", &RTL, Side::Right, (1.05, 1.15), true, false, 0.0, (0.257, 0.333, 0.524), CLIP, 0.85, LEAN, 1, 1.0, 0.0, true, None, Then::Other),
    mv("ChainLowLeftSwing", &LOW_RTL, Side::Left, (1.05, 1.15), true, false, 0.0, (0.278, 0.489, 0.567), CLIP, 0.85, LEAN, 2, 1.0, 0.0, true, None, Then::Other),
    mv("ChainLowRightSwing", &LOW_RTL, Side::Right, (1.05, 1.15), true, false, 0.0, (0.250, 0.370, 0.500), CLIP, 0.85, LEAN, 2, 1.0, 0.0, true, None, Then::Other),
    mv("ChainThrustLeft", &THRUST, Side::Left, (1.2, 1.3), true, true, 0.0, (0.275, 0.525, 0.650), CLIP, 0.85, LEAN, 3, 1.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
    mv("ChainThrustRight", &THRUST, Side::Right, (1.2, 1.3), true, true, 0.0, (0.225, 0.375, 0.500), CLIP, 0.8, LEAN, 3, 1.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
    mv("ChainHighLeftSwing", &TTL, Side::Left, (1.0, 1.1), true, false, 0.0, (0.357, 0.543, 0.714), CLIP, 0.85, LEAN, 1, 1.0, 0.0, true, None, Then::Other),
    mv("ChainHighRightSwing", &TTL, Side::Right, (0.8, 0.9), true, false, 0.0, (0.386, 0.514, 0.671), CLIP, 0.85, LEAN, 1, 1.0, 0.0, true, None, Then::Other),
    mv("ChainCrossSlash", &RTL, Side::Both, (1.3, 1.3), true, false, 0.0, (0.250, 0.362, 0.512), CLIP, 0.8, LEAN, 2, 1.0, 0.15, true, Some(Idle::Simple), Then::Other),
];
static DUAL_RIPOSTE: [Move; 6] = [
    mv("RiposteHighRight", &TTL, Side::Both, (0.0, 0.0), true, false, 0.0, (0.100, 0.470, 0.570), CLIP, 0.85, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteHighLeft", &TTR, Side::Both, (0.0, 0.0), true, false, 0.0, (0.153, 0.408, 0.510), CLIP, 0.85, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteRight", &RTL, Side::Both, (0.0, 0.0), true, false, 0.0, (0.155, 0.636, 0.727), CLIP, 0.85, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteLeft", &LTR, Side::Both, (0.0, 0.0), true, false, 0.0, (0.184, 0.408, 0.510), CLIP, 0.85, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteLowRight", &LOW_RTL, Side::Both, (0.0, 0.0), true, false, 0.0, (0.136, 0.636, 0.727), CLIP, 0.85, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
    mv("RiposteLowLeft", &LOW_LTR, Side::Both, (0.0, 0.0), true, false, 0.0, (0.184, 0.408, 0.561), CLIP, 0.85, LEAN, 0, 1.0, 0.0, true, None, Then::Simple),
];
/// Sword and shield: light and heavy swings, overhead ones, thrusts, and the shield's three pushes
/// (shoving at 5, 7 and 9 m/s) and its two chained ones.
static SHIELD_OPENING: [Move; 13] = [
    mv("SwingLeftToRightLight", &LTR, Side::Right, (1.1, 1.1), true, false, 0.0, (0.250, 0.325, 0.575), 1.33, 1.0, (0.39, 0.43, 0.00), 1, 4.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("SwingRightToLeftLight", &RTL, Side::Right, (1.1, 1.1), true, false, 0.0, (0.229, 0.429, 0.600), 1.17, 1.0, (0.30, 0.40, 0.00), 1, 4.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("SwingLeftToRight", &LTR, Side::Right, (1.45, 1.45), true, false, 0.0, (0.145, 0.273, 0.455), 1.83, 1.0, (0.83, 0.56, 0.00), 1, 4.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("SwingRightToLeft", &RTL, Side::Right, (1.45, 1.45), true, false, 0.0, (0.200, 0.345, 0.436), 1.83, 1.0, (0.83, 0.35, 0.03), 1, 3.0, 0.0, true, Some(Idle::Simple), Then::Simple),
    mv("ThrustLight", &THRUST, Side::Right, (0.95, 1.05), true, true, 0.0, (0.200, 0.444, 0.511), 1.50, 1.0, (0.43, 0.49, 0.00), 3, 3.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
    mv("ThrustHeavy", &THRUST, Side::Right, (1.25, 1.45), true, true, 0.0, (0.180, 0.400, 0.460), 1.67, 1.0, (0.83, 0.40, 0.0), 3, 3.0, 0.0, true, Some(Idle::Thrusting), Then::Any),
    mv("OverheadSwingLeftToRightLight", &TTR, Side::Right, (1.1, 1.1), true, false, 0.0, (0.200, 0.325, 0.500), 1.33, 1.0, (0.35, 0.50, 0.0), 1, 3.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("OverheadSwingRightToLeftLight", &TTL, Side::Right, (1.1, 1.1), true, false, 0.0, (0.220, 0.380, 0.440), 1.67, 1.0, (0.35, 0.38, 0.0), 1, 3.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("OverheadSwingLeftToRight", &TTR, Side::Right, (1.6, 1.6), true, false, 0.0, (0.218, 0.345, 0.455), 1.83, 1.0, (0.86, 0.53, 0.0), 1, 3.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("OverheadSwingRightToLeft", &TTL, Side::Right, (1.6, 1.6), true, false, 0.0, (0.273, 0.436, 0.491), 1.83, 1.0, (0.86, 0.55, 0.00), 1, 3.0, 0.0, true, Some(Idle::High), Then::Simple),
    mv("PushCloseLight", &PUSH, Side::Left, (0.8, 1.13), false, true, 5.0, (0.275, 0.562, 0.800), 1.33, 1.0, (1.08, 0.58, 0.75), 1, 1.0, 0.3, false, None, Then::Any),
    mv("PushMidAverage", &PUSH, Side::Left, (1.13, 1.46), false, true, 7.0, (0.275, 0.562, 0.800), 1.33, 1.0, (1.08, 0.58, 0.75), 2, 1.0, 0.3, false, None, Then::Any),
    mv("PushFarHeavy", &PUSH, Side::Left, (1.46, 1.8), false, true, 9.0, (0.275, 0.562, 0.800), 1.33, 1.0, (1.08, 0.58, 0.75), 3, 1.0, 0.3, false, None, Then::Any),
];
static SHIELD_CHAIN: [Move; 2] = [
    mv("ChainPushMidAverage", &PUSH, Side::Left, (0.1, 2.0), false, true, 5.0, (0.275, 0.562, 0.800), CLIP, 1.15, LEAN, 1, 1.0, 0.6, false, None, Then::Any),
    mv("ChainPushFarHeavy", &PUSH, Side::Left, (0.1, 2.0), false, true, 7.0, (0.275, 0.562, 0.800), CLIP, 1.15, LEAN, 2, 1.0, 0.6, false, None, Then::Any),
];
static SHIELD_RIPOSTE: [Move; 6] = [
    mv("RiposteRight", &RTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.144, 0.256, 0.478), CLIP, 1.0, LEAN, 1, 1.0, 0.0, false, None, Then::Simple),
    mv("RiposteLeft", &LTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.211, 0.289, 0.444), CLIP, 1.0, LEAN, 1, 1.0, 0.0, false, None, Then::Simple),
    mv("RiposteHighRight", &TTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.156, 0.267, 0.378), CLIP, 1.0, LEAN, 1, 1.0, 0.0, false, None, Then::Simple),
    mv("RiposteHighLeft", &TTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.200, 0.300, 0.400), CLIP, 1.0, LEAN, 1, 1.0, 0.0, false, None, Then::Simple),
    mv("RiposteLowRight", &LOW_RTL, Side::Right, (0.0, 0.0), true, false, 0.0, (0.089, 0.244, 0.400), CLIP, 1.0, LEAN, 2, 1.0, 0.0, false, None, Then::Simple),
    mv("RiposteLowLeft", &LOW_LTR, Side::Right, (0.0, 0.0), true, false, 0.0, (0.222, 0.333, 0.444), CLIP, 1.0, LEAN, 2, 1.0, 0.0, false, None, Then::Simple),
];

impl Stance {
    /// What its hands hold makes its stance, by the off hand: a shield beside the weapon, a second
    /// weapon, or nothing.
    pub fn of(hands: [Option<Hold>; 2]) -> Stance {
        let weapons = hands.iter().flatten().filter(|h| !h.shield).count();
        if hands.iter().flatten().any(|h| h.shield) {
            Stance::SwordShield
        } else if weapons == 2 {
            Stance::DualWield
        } else {
            Stance::OneHanded
        }
    }

    fn opening(self) -> &'static [Move] {
        match self {
            Stance::OneHanded => &ONE_OPENING,
            Stance::DualWield => &DUAL_OPENING,
            Stance::SwordShield => &SHIELD_OPENING,
        }
    }

    fn chain(self) -> &'static [Move] {
        match self {
            Stance::OneHanded => &ONE_CHAIN,
            Stance::DualWield => &DUAL_CHAIN,
            Stance::SwordShield => &SHIELD_CHAIN,
        }
    }

    fn riposte(self, name: &str) -> Option<&'static Move> {
        let all: &'static [Move] = match self {
            Stance::OneHanded => &ONE_RIPOSTE,
            Stance::DualWield => &DUAL_RIPOSTE,
            Stance::SwordShield => &SHIELD_RIPOSTE,
        };
        all.iter().find(|m| m.name == name)
    }

    /// Its chains alternate hands (the dual wield's).
    fn alternates(self) -> bool {
        self == Stance::DualWield
    }
}

impl Move {
    /// Where the wrist is at `t` (its share, 0..1) for hand `hand`: its shape, wound up a little
    /// before its blow starts to hurt (ours), striking at its
    /// peak, followed through at its end.
    fn key(&self, t: f32, hand: usize) -> Key {
        let [w, p, e] = self.shape.marks;
        let marks = [(0.0, 0.0), (self.start * WOUND_UP, w), (self.peak, p), (self.end, e), (1.0, 1.0)];
        let t = t.clamp(0.0, 1.0);
        let i = marks.windows(2).position(|w| t <= w[1].0).unwrap_or(3);
        let ((a0, b0), (a1, b1)) = (marks[i], marks[i + 1]);
        self.shape.at(b0 + (b1 - b0) * ((t - a0) / (a1 - a0).max(1e-4)).clamp(0.0, 1.0), hand)
    }

    /// How far forward its body has come at `t` (m): its root's curve, eased to its furthest and on
    /// to where it ends.
    fn root(&self, t: f32) -> f32 {
        let (far, at, end) = self.root;
        let t = t.clamp(0.0, 1.0);
        if t <= at { far * smooth(t / at.max(1e-3)) } else { far + (end - far) * smooth((t - at) / (1.0 - at).max(1e-3)) }
    }
}

fn mirror(k: Key, hand: usize) -> Key {
    if hand == 1 {
        return k;
    }
    let m = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
    Key { t: k.t, wrist: m(k.wrist), blade: m(k.blade), edge: m(k.edge) }
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A swing under way: which move, when it started, how fast it plays; played back (at -0.5 speed
/// for 0.2 s) from a share of it, since then, before it's dropped.
#[derive(Clone, Copy)]
pub struct Swing {
    pub mv: &'static Move,
    pub t0: f64,
    /// Its tier's speed times the move's own, slower for a chain and a heavy weapon.
    pub speed: f32,
    pub back: Option<(f32, f64)>,
    /// A feint: at the top of its wind-up it turns into another move.
    pub feint: bool,
}

impl Swing {
    /// How long it takes (s).
    fn length(&self) -> f32 {
        self.mv.seconds / self.speed.max(0.1)
    }

    /// Its share of the move now (0..1).
    pub fn progress(&self, now: f64) -> f32 {
        match self.back {
            Some((from, since)) => (from - PLAYBACK * (now - since) as f32 / self.length()).max(0.0),
            None => ((now - self.t0) as f32 / self.length()).clamp(0.0, 1.0),
        }
    }

    /// Its weapon can hurt now (between the move's start and end, going forward).
    pub fn hurting(&self, now: f64) -> bool {
        let t = self.progress(now);
        self.back.is_none() && now >= self.t0 && t >= self.mv.start && t <= self.mv.end
    }
}

/// A stopped swing plays back at this speed for this long before it's dropped (-0.5, 0.2 s); a
/// chain starts once it has.
const PLAYBACK: f32 = 0.5;
const WAIT: f64 = 0.2;
/// An arm breaking off what it did blends to what it does next over this long (s): the 0.2 s wait
/// before a chain, and the same otherwise.
const BLEND: f64 = 0.2;

/// One of the player's weapons as a brain sees it: its blade's ends, how fast its tip moves, and
/// whether it can stab (its point is a forward pierce damager).
#[derive(Clone, Copy, Debug, Default)]
pub struct Threat {
    pub a: Vec3,
    pub b: Vec3,
    pub vel: Vec3,
    pub point: bool,
}

/// What a brain sees: the player (their feet, how they move, their weapons and the longest reach
/// among them), weapons lying loose (item indices and where), the others of its side (their feet,
/// and how many are at the player already), and the ways out.
#[derive(Clone, Debug, Default)]
pub struct Sight {
    pub feet: Vec3,
    pub vel: Vec3,
    pub threats: Vec<Threat>,
    pub reach: f32,
    pub loose: Vec<(usize, Vec3)>,
    pub allies: Vec<Vec3>,
    pub engaged: usize,
    pub exits: Vec<Vec3>,
}

/// What a hand holds, as a brain cares: how far its blade reaches from the fist (m), how heavy it
/// is (kg), whether it's a shield, whether the player has hold of it too, and where its blade is
/// (world).
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub reach: f32,
    pub mass: f32,
    pub shield: bool,
    pub players: bool,
    pub blade: (Vec3, Vec3),
}

/// A hand on it, by one of its handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Neck,
    Mouth,
    Skull,
    Torso,
    Shoulder,
    Wrist,
    Hand,
    Legs,
}

/// Held standing: by which handle (on which arm or leg, for theirs), where the hand would have the
/// point it holds (world) and how high that point is standing (m), and where the one holding it is.
#[derive(Clone, Copy, Debug)]
pub struct Grappled {
    pub handle: Handle,
    pub limb: usize,
    pub grip: Vec3,
    pub stand: f32,
    pub grabber: Vec3,
}

/// The state of its body, as its brain needs it.
#[derive(Clone, Copy, Debug)]
pub struct Me {
    pub at: Vec3,
    pub face: Quat,
    pub hands: [Option<Hold>; 2],
    /// Where its head is (world).
    pub head: Vec3,
    /// Reeling, shocked or burning (it can't attack, guard or dodge), or getting up.
    pub staggered: bool,
    pub rising: bool,
    /// The share of its health it has lost (its dodges), and a hand holding it.
    pub lost: f32,
    pub held: Option<Grappled>,
}

/// What it's up to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Not yet roused (the player isn't near).
    Waiting,
    /// Closing in or keeping its distance, strafing.
    Fighting,
    /// Going for a weapon lying loose (an item index), and picking it up since then.
    Fetching(usize, Option<f64>),
    /// Unarmed with nothing to take up: running for the way out (there), and gone once there.
    Fleeing(Vec3),
    Gone,
}

/// A dodge under way: back, ducking a high blow or leaping a low one, since then.
#[derive(Clone, Copy, Debug)]
struct Dodge {
    low: f32,
    high: f32,
    since: f64,
}

pub struct Brain {
    pub tier: Tier,
    pub stance: Stance,
    pub mode: Mode,
    pub swing: Option<Swing>,
    /// A swing to start then (a chain after the parried swing's playback, an attack after its guard
    /// comes down), whether it's a chain, and its speed.
    queued: Option<(&'static Move, f64, bool)>,
    /// The idle it stands in, and when it next changes (every 5 to 10 s).
    idle: Idle,
    idle_until: f64,
    /// When its last attack ends as planned when it started, and the delay after that.
    attack_end: f64,
    attack_delay: f32,
    /// Ripostes and chains wait this long after then, and when it last recoiled.
    riposte_at: f64,
    riposte_delay: f32,
    last_recoil: f64,
    /// How many swings it has chained, and whether what its last swing met was a shield.
    pub chains: u32,
    hit_shield: bool,
    /// How threatened it feels (0..1, falling 2 a second), when that last rose, and when it was last
    /// hurt.
    pub danger: f32,
    danger_rose: f64,
    hurt_at: f64,
    /// Its guard: where its weapon hand (or shield) goes, blended at 7 a second, which guard pose
    /// that is (side, height), and how far up it is (the block weight).
    guard: Key,
    guard_dir: Vec2,
    pub block: f32,
    /// Strafing: which way, until when.
    strafe: (f32, f64),
    /// The move it last opened with (picked again at 1%).
    last_move: Option<&'static str>,
    /// Its next brain cycle (every 0.1 s near the player, up to 1 s at 20 m).
    cycle: f64,
    dodge: Option<Dodge>,
    next_dodge: f64,
    dodge_wait: Option<f64>,
    /// How far forward its swing's root has carried it so far (m).
    root: f32,
    /// Each hand's key last frame, and the key to blend from since then.
    keys: [Key; 2],
    blend: [Option<(Key, f64)>; 2],
    /// Grappled, its last escape try (for the escape's twist).
    pub escaping: Option<f64>,
    /// Its body and what it saw last frame (for a block's roll between frames).
    seen: Option<(Me, Sight)>,
    rng: u32,
}

/// Its guard blends this fast (7/s); the block weight 10/s, full when the blow's way passes within
/// 0.5 m, nothing beyond 2 m.
const GUARD_SPEED: f32 = 7.0;
const BLOCK_SPEED: f32 = 10.0;
const BLOCK_NEAR: f32 = 0.5;
const BLOCK_FAR: f32 = 2.0;
/// It's roused by the player within this (m), and defends within 5 m.
const ROUSE: f32 = 7.0;
const DEFEND: f32 = 5.0;
/// The danger falls 2 a second (all of it in 0.5 s); being hurt this recently (s) makes it attack
/// back regardless.
const DANGER_FALL: f32 = 2.0;
const HURT_IGNORES_DANGER: f64 = 0.75;
/// It attacks only a target within this of its facing (degrees), and a move fits within twice this
/// of its range.
const ATTACK_ANGLE: f32 = 45.0;
const RANGE_DELTA: f32 = 0.2;
/// Its weapon's reach is its blade's from the fist times this; it keeps this far past the player's
/// reach while it waits to strike (m, the player's own reach counted to at most that past their
/// arm's length).
const REACH_RATIO: f32 = 0.75;
const KEEP: f32 = 0.5;
const ARMS: f32 = 0.6;
/// Waiting its turn beyond `melee_max`, it keeps from the safe distance plus this to this much
/// further (m).
const SAFE_OFFSET: f32 = 0.25;
const SAFE_THICKNESS: f32 = 2.0;
/// Walking (m/s), strafing at this share of it, running.
const WALK: f32 = 1.4;
const STRAFE_RATIO: f32 = 0.75;
const RUN: f32 = 3.0;
/// Its dodges (a blow due within 0.5 s, after a 0.06 s wait, 0.5 to 1 s apart): a metre back, over
/// 0.6 s, split by height at 0.85 of the way from feet to hips and 0.64 from hips to head.
const HIT_SOON: f32 = 0.5;
const DODGE_WAIT: f64 = 0.06;
const DODGE_COOLDOWN: (f32, f32) = (0.5, 1.0);
const DODGE_BACK: f32 = 1.0;
const DODGE_TIME: f64 = 0.6;
/// Unarmed, it takes up a weapon no nearer the player than this, within this (m), once at its side:
/// stooping to it over 0.75 s, taking it at 1.05 s.
const AVOID: f32 = 4.0;
const SEARCH: f32 = 30.0;
const PICK_FROM: f32 = 0.7;
const STOOP_TIME: f64 = 0.75;
const TAKE_AT: f64 = 1.05;
const PICK_GIVE_UP: f64 = 1.8;
/// A thing within this of its hand it can take up (m).
const REACH: f32 = 0.45;
/// Held, its feet step after its head at three times its forward speed, a wrist's after its body at
/// once, and its height follows the hand down to half.
const STEP_AFTER: f32 = 3.0;

impl Brain {
    pub fn new(tier: Tier, stance: Stance, seed: u32) -> Brain {
        Brain {
            tier,
            stance,
            mode: Mode::Waiting,
            swing: None,
            queued: None,
            idle: Idle::Simple,
            idle_until: 0.0,
            attack_end: 0.0,
            attack_delay: 0.0,
            riposte_at: 0.0,
            riposte_delay: 0.0,
            last_recoil: -10.0,
            chains: 0,
            hit_shield: false,
            danger: 0.0,
            danger_rose: -10.0,
            hurt_at: -10.0,
            guard: GUARD,
            guard_dir: Vec2::ZERO,
            block: 0.0,
            strafe: (1.0, 0.0),
            last_move: None,
            cycle: 0.0,
            dodge: None,
            next_dodge: 0.0,
            dodge_wait: None,
            root: 0.0,
            keys: [mirror(REST, 0), REST],
            blend: [None; 2],
            escaping: None,
            seen: None,
            rng: seed.wrapping_mul(2_654_435_761).max(1),
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift: a fighter's own dice.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32) / (u32::MAX as f32)
    }

    fn range(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * self.rand()
    }

    /// It's attacking (from the attack's start, a delay before it included, to its end; not while a
    /// stopped swing plays back).
    pub fn attacking(&self) -> bool {
        self.queued.is_some() || self.swing.is_some_and(|s| s.back.is_none())
    }

    /// The hands its swing hurts with now.
    pub fn hurting(&self, now: f64) -> [bool; 2] {
        [0, 1].map(|h| self.swing.is_some_and(|s| s.hurting(now) && s.mv.side.has(h)))
    }

    /// Its swing is a feint (for the game's look).
    pub fn feinting(&self) -> bool {
        self.swing.is_some_and(|s| s.feint)
    }

    /// Its swing is one of the shield's pushes, and how hard it shoves (m/s).
    pub fn pushing(&self, now: f64) -> Option<f32> {
        self.swing.filter(|s| s.hurting(now) && s.mv.push > 0.0).map(|s| s.mv.push)
    }

    /// What it's up to, for `RUSTVR_DEBUG_BRAIN`'s log.
    pub fn describe(&self, now: f64) -> String {
        let swing = self.swing.map_or(String::from("-"), |s| format!("{} {:.2}{}", s.mv.name, s.progress(now), if s.back.is_some() { " back" } else { "" }));
        format!("{:?} {:?} {:?}, swing {swing}, danger {:.2}, block {:.2}{}", self.mode, self.stance, self.idle, self.danger, self.block, if self.dodge.is_some() { ", dodging" } else { "" })
    }

    pub fn doing(&self) -> Doing {
        match self.mode {
            Mode::Waiting => Doing::Idle,
            _ if self.attacking() => Doing::Attacking,
            _ => Doing::Combat,
        }
    }

    /// Its swing breaks off (a recoil, a cancel, a stagger, a block): played back for 0.2 s, then
    /// dropped.
    fn break_off(&mut self, now: f64) {
        self.queued = None;
        if let Some(s) = self.swing.filter(|s| s.back.is_none()) {
            self.swing = Some(Swing { back: Some((s.progress(now), now)), ..s });
        }
    }

    /// Its attack stops at once (with nothing played back), and any it had waiting with it.
    fn stop(&mut self, now: f64) {
        self.queued = None;
        self.drop_swing(now);
    }

    /// Its swing is dropped: its arms blend from where they are to what comes next (a chain waiting
    /// for it to have played back is kept).
    fn drop_swing(&mut self, now: f64) {
        if let Some(s) = self.swing.take() {
            for h in 0..2 {
                if s.mv.side.has(h) {
                    self.blend[h] = Some((self.keys[h], now));
                }
            }
        }
    }

    /// Its brain goes off (out cold, or down): whatever it was swinging stops, and its guard drops.
    pub fn sleep(&mut self, now: f64) {
        self.stop(now);
        self.dodge = None;
        self.dodge_wait = None;
        self.block = 0.0;
        self.seen = None;
    }

    /// It was hurt this much of its health: a heavy enough hit stops its attack, and being hurt
    /// makes it fight back rather than defend.
    pub fn hurt(&mut self, share: f32, now: f64) {
        self.hurt_at = now;
        if share >= self.tier.difficulty().cancel && self.attacking() {
            tracing::info!("brain: hurt for {:.0}% of its health: its attack stops", share * 100.0);
            self.stop(now);
        }
        if self.mode == Mode::Waiting {
            self.mode = Mode::Fighting;
        }
    }

    /// Its swing met the player's weapon, and it rolls: a chained follow-up after 0.2 s, nothing (it
    /// plays back and stops), or a recoil (a full stagger, at most once a cooldown). Against a
    /// shield it never chains. Returns whether it recoils.
    pub fn parried(&mut self, now: f64, shield: bool) -> bool {
        let Some(s) = self.swing.filter(|s| s.back.is_none()) else { return false };
        let d = self.tier.difficulty();
        self.hit_shield = shield;
        let c = self.chains as f32;
        let r = self.rand();
        let chain_from = d.roll.1 + c * d.roll_per_chain.1;
        let nothing_from = d.roll.0 + c * d.roll_per_chain.0;
        let can_chain = chain_from < 1.0 && (now - self.riposte_at > self.riposte_delay as f64 || now < self.riposte_at) && s.mv.chains && !shield;
        if r > chain_from && can_chain && let Some(next) = self.pick_chain(s.mv) {
            // A combo: this swing plays back, and its chain starts once it has.
            self.chains += 1;
            self.break_off(now);
            self.queued = Some((next, now + WAIT, true));
            let length = next.seconds / (next.speed * d.speed * (1.0 + d.chain_speed * self.chains as f32)).max(0.1);
            self.riposte_at = now + (1.1 * length).min(d.riposte_delay.0) as f64;
            self.riposte_delay = self.range(d.riposte_delay);
            tracing::info!("brain: parried, chains into {} ({} chained)", next.name, self.chains);
            false
        } else if r >= nothing_from {
            // Nothing: the swing plays back a moment, and it carries on.
            self.break_off(now);
            tracing::info!("brain: parried, its swing turned back");
            false
        } else if now - self.last_recoil <= d.recoil_cooldown as f64 {
            // A recoil too soon after the last: its attack just stops.
            self.stop(now);
            tracing::info!("brain: parried, its attack stops (a recoil too soon after the last)");
            false
        } else {
            self.last_recoil = now;
            self.chains = 0;
            self.break_off(now);
            tracing::info!("brain: parried, it recoils");
            true
        }
    }

    /// A chain for move `last`: any of its stance's chains its difficulty allows from its idle, by
    /// weight; dual wield switches hands.
    fn pick_chain(&mut self, last: &'static Move) -> Option<&'static Move> {
        let level = self.tier.difficulty().level;
        let idle = self.idle;
        let alternate = self.stance.alternates();
        let fits = |m: &Move| m.difficulty <= level && m.from.is_none_or(|i| i == idle);
        let other = |m: &Move| match last.side {
            Side::Left => m.side != Side::Left,
            Side::Right => m.side != Side::Right,
            Side::Both => m.side != Side::Both,
        };
        let pool: Vec<&'static Move> = self.stance.chain().iter().filter(|m| fits(m) && (!alternate || other(m))).collect();
        let pool = if pool.is_empty() { self.stance.chain().iter().filter(|m| fits(m)).collect() } else { pool };
        self.weighted(&pool)
    }

    /// One of `pool` at random by weight, each skipped by its own chance first.
    fn weighted(&mut self, pool: &[&'static Move]) -> Option<&'static Move> {
        let weights: Vec<f32> = pool.iter().map(|m| if m.skip > 0.0 && self.rand() <= m.skip { 0.0 } else { m.weight }).collect();
        let total: f32 = weights.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let mut pick = self.rand() * total;
        pool.iter().zip(&weights).find(|(_, w)| {
            pick -= **w;
            pick <= 0.0
        }).map(|(m, _)| *m).or(pool.last().copied())
    }

    /// It blocked the player's blow, with no push passed on (a heavy weapon's is the ragdoll's): over
    /// 1 m/s, with a shield a counter three times in ten; otherwise, out of the shield stance, the
    /// riposte roll with no chains yet (its guard well up: a riposte, nothing, or a recoil) or, half
    /// up, a recoil. Returns whether it recoils.
    pub fn blocked(&mut self, now: f64, speed: f32, shield: bool) -> bool {
        let Some((me, sight)) = self.seen.clone() else { return false };
        let (me, sight) = (&me, &sight);
        if !self.defending(me, sight) {
            return false;
        }
        self.next_dodge = self.next_dodge.max(now + DODGE_COOLDOWN.0 as f64);
        if speed <= 1.0 {
            return false;
        }
        let d = self.tier.difficulty();
        self.chains = 0;
        let block = self.block;
        if shield && self.rand() <= SHIELD_COUNTER {
            if !self.block_riposte(now) {
                // No riposte from a neutral guard: its best opening move, after its guard comes down.
                let delay = if block >= 0.5 { 0.1 * block * 1.5 } else { 0.0 };
                if let Some(m) = self.pick_opening(me, sight) {
                    self.queued = Some((m, now + delay as f64, false));
                    tracing::info!("brain: blocked with its shield, counters with {}", m.name);
                }
            }
            return false;
        }
        if self.stance == Stance::SwordShield {
            return false;
        }
        if block < RIPOSTE_BLOCK {
            return self.recoil(now, "blocked half-guarded, it recoils");
        }
        let r = self.rand();
        let can = d.roll.1 < 1.0 && (now - self.riposte_at > self.riposte_delay as f64 || now < self.riposte_at) && !self.hit_shield;
        if r > d.roll.1 && can {
            // (A guard with no riposte of its own, the neutral one, does nothing.)
            self.block_riposte(now);
            return false;
        }
        if r >= d.roll.0 {
            tracing::info!("brain: blocked, nothing");
            return false;
        }
        self.recoil(now, "blocked, it recoils")
    }

    fn recoil(&mut self, now: f64, why: &str) -> bool {
        if now - self.last_recoil <= self.tier.difficulty().recoil_cooldown as f64 {
            return false;
        }
        self.last_recoil = now;
        tracing::info!("brain: {why}");
        true
    }

    /// Its riposte off a block: the riposte of the guard pose it's most in (at least a third of it;
    /// the neutral guard has none), straight away.
    fn block_riposte(&mut self, now: f64) -> bool {
        let poses = [(Vec2::new(-1.0, 1.0), "RiposteHighLeft"), (Vec2::new(1.0, 1.0), "RiposteHighRight"), (Vec2::new(-1.0, 0.0), "RiposteLeft"), (Vec2::ZERO, ""), (Vec2::new(1.0, 0.0), "RiposteRight"), (Vec2::new(-1.0, -1.0), "RiposteLowLeft"), (Vec2::new(1.0, -1.0), "RiposteLowRight")];
        let weights: Vec<f32> = poses.iter().map(|(p, _)| (1.0 - p.distance(self.guard_dir)).max(0.0)).collect();
        let total: f32 = weights.iter().sum::<f32>().max(1e-3);
        let (best, w) = weights.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, w)| (i, w / total)).unwrap_or((3, 1.0));
        let Some(m) = (w >= 0.333).then(|| self.stance.riposte(poses[best].1)).flatten() else { return false };
        self.queued = Some((m, now, false));
        tracing::info!("brain: blocked, ripostes with {}", m.name);
        true
    }

    /// It may dodge: the player within its reach of defence, and it neither attacking, dodging,
    /// reeling, getting up nor held (unarmed, it still dodges).
    fn can_dodge(&self, me: &Me, sight: &Sight) -> bool {
        matches!(self.mode, Mode::Fighting | Mode::Fetching(..) | Mode::Fleeing(_)) && !self.attacking() && self.dodge.is_none() && !me.staggered && !me.rising && me.held.is_none() && (sight.feet - me.at).with_y(0.0).length() <= DEFEND
    }

    /// It's defending with a weapon: fighting, and free to dodge.
    fn defending(&self, me: &Me, sight: &Sight) -> bool {
        self.mode == Mode::Fighting && self.can_dodge(me, sight)
    }

    /// It's defending, as it was last frame (a blow is blocked only then).
    pub fn defends(&self) -> bool {
        self.seen.as_ref().is_some_and(|(me, sight)| self.defending(me, sight))
    }

    /// The opening move that best fits where the player will be at its peak: from its idle, its
    /// difficulty allows, within 0.12 to 0.28 m of its range (or, sweeping through, reaching past
    /// them), weighed by how well it fits, the last one at 1%.
    fn pick_opening(&mut self, me: &Me, sight: &Sight) -> Option<&'static Move> {
        let d = self.tier.difficulty();
        let idle = self.idle;
        let at = me.at.with_y(0.0);
        let target = sight.feet.with_y(0.0);
        let vel = sight.vel.with_y(0.0);
        let mut fits: Vec<(&'static Move, f32)> = Vec::new();
        for m in self.stance.opening() {
            if m.difficulty > d.level || m.from.is_some_and(|i| i != idle) || !self.can_swing(m, me.hands) {
                continue;
            }
            if m.skip > 0.0 && self.rand() <= m.skip {
                continue;
            }
            let speed = self.speed_of(m, me.hands, 1);
            let length = m.seconds / speed;
            let reach = if m.reach { weapon_reach(m.side, me.hands) } else { 0.0 };
            let ahead = |share: f32| (target + vel * share * length).distance(at) - reach;
            let (start, peak, end) = (ahead(m.start), ahead(m.peak), ahead(m.end));
            let further = m.range.1 > peak;
            let delta = (m.range.0 - peak).abs();
            let window = RANGE_DELTA * 2.0 * (if end < start { 0.6 } else { 0.4 } + if further { 0.1 } else { -0.1 });
            let good = delta <= window;
            if !good && !(further && m.sweep) {
                continue;
            }
            let ratio = if Some(m.name) == self.last_move { 0.01 } else if good { 1.0 - delta / window } else { m.range.0 / peak.max(1e-3) };
            fits.push((m, m.weight * ratio));
        }
        let total: f32 = fits.iter().map(|(_, w)| w).sum();
        if total <= 0.0 {
            return None;
        }
        let mut pick = self.rand() * total;
        fits.iter().find(|(_, w)| {
            pick -= w;
            pick <= 0.0
        }).or(fits.last()).map(|(m, _)| *m)
    }

    /// Its hands can swing `m`: a weapon in each hand it swings with (a shield for its pushes), and a
    /// point for a thrust.
    fn can_swing(&self, m: &Move, hands: [Option<Hold>; 2]) -> bool {
        (0..2).filter(|&h| m.side.has(h)).all(|h| hands[h].is_some_and(|x| if m.push > 0.0 { x.shield } else { !x.shield && (!std::ptr::eq(m.shape, &THRUST) || x.reach > 0.5) }))
    }

    /// The speed for move `m` with `chains` chained: its own, its tier's, slower each chain, and
    /// slower with a heavier weapon (`1.5 / mass`, down to 70%: both hands', the slower).
    fn speed_of(&self, m: &Move, hands: [Option<Hold>; 2], chains: u32) -> f32 {
        let d = self.tier.difficulty();
        let mass = (0..2).filter(|&h| m.side.has(h)).map(|h| (1.5 / hands[h].map_or(1.5, |x| x.mass).max(0.1)).clamp(0.7, 1.0)).fold(1.0f32, f32::min);
        (m.speed * d.speed * (1.0 + d.chain_speed * chains as f32) * mass).max(0.1)
    }

    /// Starts move `m` now: its arms blend into it; its planned end and the delay after it set when
    /// it may next attack, and when it may next riposte.
    fn start(&mut self, m: &'static Move, now: f64, chain: bool, hands: [Option<Hold>; 2]) {
        let d = self.tier.difficulty();
        if !chain {
            self.chains = 1;
        }
        let speed = self.speed_of(m, hands, self.chains);
        let feint = !chain && self.rand() < self.tier.feints();
        let swing = Swing { mv: m, t0: now, speed, back: None, feint };
        let length = swing.length() as f64;
        self.attack_end = now + length;
        self.attack_delay = self.range(d.between);
        if !chain {
            self.riposte_at = now + length;
            self.riposte_delay = self.range(d.riposte_delay);
        }
        for h in 0..2 {
            if m.side.has(h) {
                self.blend[h] = Some((self.keys[h], now));
            }
        }
        self.swing = Some(swing);
        self.root = 0.0;
        self.hit_shield = false;
        if !chain {
            self.last_move = Some(m.name);
        }
    }

    /// One frame: what it does, and the pose its body is pinned to; its feet move by `walk` (its own
    /// frame's velocity) and it turns to face `face_to`.
    pub fn think(&mut self, now: f64, dt: f32, me: &Me, sight: &Sight) -> Thought {
        self.seen = Some((*me, sight.clone()));
        let d = self.tier.difficulty();
        let at = me.at;
        let face = me.face;
        let to = sight.feet - at;
        let flat = Vec3::new(to.x, 0.0, to.z);
        let dist = flat.length();
        let local = face.inverse();
        let hands = me.hands;
        self.stance = Stance::of(hands);
        let armed = hands.iter().any(|h| h.is_some_and(|h| !h.shield));
        let mut shout = false;
        // Roused by the player near.
        if self.mode == Mode::Waiting && dist < ROUSE {
            self.mode = Mode::Fighting;
        }
        // Unarmed: the nearest loose weapon not near the player (within 30 m, not within 4 m of
        // them), else the nearest way out.
        if !armed && !matches!(self.mode, Mode::Waiting | Mode::Gone) {
            self.stop(now);
            let pick = sight.loose.iter().filter(|(_, p)| p.distance(sight.feet) > AVOID && p.distance(at) < SEARCH).min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at))).map(|&(k, _)| k);
            self.mode = match (pick, self.mode) {
                (Some(k), Mode::Fetching(was, since)) if was == k => Mode::Fetching(k, since),
                (Some(k), _) => Mode::Fetching(k, None),
                (None, Mode::Fleeing(exit)) => Mode::Fleeing(exit),
                (None, _) => {
                    let exit = sight.exits.iter().copied().min_by(|a, b| a.distance(at).total_cmp(&b.distance(at))).unwrap_or(at - flat.normalize_or(Vec3::Z) * 5.0);
                    tracing::info!("brain: unarmed with nothing to take up: flees for {exit:.1}");
                    Mode::Fleeing(exit)
                }
            };
        } else if armed && matches!(self.mode, Mode::Fetching(..) | Mode::Fleeing(_)) {
            self.mode = Mode::Fighting;
        }
        let held = me.held;
        let busy = me.staggered || me.rising || held.is_some();
        if busy {
            // Reeling, getting up or held, it neither swings, guards nor dodges; any stagger stops its
            // attack, played back 0.2 s.
            if me.staggered || held.is_some() {
                if self.attacking() {
                    tracing::info!("brain: {}: its attack stops", if me.staggered { "staggered" } else { "held" });
                }
                self.break_off(now);
            }
            self.queued = None;
            self.dodge = None;
            self.dodge_wait = None;
        }
        // A stopped swing's playback ends: it's dropped, and its arms blend from there.
        if let Some(s) = self.swing
            && let Some((_, since)) = s.back
            && now - since >= WAIT
        {
            self.drop_swing(now);
        }
        // A swing ends; the idle it leaves it in.
        if let Some(s) = self.swing
            && s.back.is_none()
            && s.progress(now) >= 1.0
        {
            self.idle = self.next_idle(s.mv.then, hands);
            self.swing = None;
            self.chains = 0;
        }
        // Its idle changes now and then (the first at once).
        if self.mode == Mode::Fighting && self.swing.is_none() && self.queued.is_none() && now >= self.idle_until {
            self.idle = self.next_idle(if self.idle_until == 0.0 { Then::Any } else { Then::Other }, hands);
            self.idle_until = now + self.range(IDLE_EVERY) as f64;
        }
        // A queued swing starts.
        if let Some((m, when, chain)) = self.queued
            && now >= when
        {
            self.queued = None;
            if self.swing.is_some_and(|s| s.back.is_some()) {
                self.drop_swing(now);
            }
            if self.swing.is_none() && self.can_swing(m, hands) {
                if chain {
                    tracing::info!("brain: its chain {} begins", m.name);
                }
                self.start(m, now, chain, hands);
                shout = true;
            }
        }
        // A feint at the top of its wind-up: the swing turns into another of its hand's moves.
        if let Some(s) = self.swing.filter(|s| s.feint && s.back.is_none() && s.progress(now) >= s.mv.start * 0.8) {
            let level = d.level;
            let pool: Vec<&'static Move> = self.stance.opening().iter().filter(|m| m.name != s.mv.name && m.side == s.mv.side && m.difficulty <= level && m.push == 0.0).collect();
            if let Some(next) = self.weighted(&pool) {
                tracing::info!("brain: feints: {} turns into {}", s.mv.name, next.name);
                for h in 0..2 {
                    if next.side.has(h) {
                        self.blend[h] = Some((self.keys[h], now));
                    }
                }
                let back = (next.seconds / s.speed) as f64 * next.start as f64 * 0.6;
                self.swing = Some(Swing { mv: next, t0: now - back, feint: false, ..s });
            } else {
                self.swing = Some(Swing { feint: false, ..s });
            }
        }
        // Danger: the player's weapons coming its way.
        let chest = at + Vec3::Y * 1.25;
        let (lo, hi) = match self.stance {
            Stance::OneHanded => (2.5, 6.0),
            Stance::DualWield => (3.0, 8.0),
            Stance::SwordShield => (1.75, 5.0),
        };
        let mut incoming: Option<Incoming> = None;
        for t in &sight.threats {
            let speed = t.vel.length();
            let (near, _) = closest(t.a, t.b, chest, chest);
            let from = t.a.lerp(t.b, near).distance(chest);
            if from > DEFEND || speed < 1.0 {
                continue;
            }
            // On course: its tip's way passes near the body within half a second (within its
            // defence capsule's 0.6 m).
            let ahead = t.b + t.vel * HIT_SOON;
            let (s, _) = closest(t.b, ahead, chest - Vec3::Y * 0.6, chest + Vec3::Y * 0.5);
            let cross = t.b.lerp(ahead, s);
            let passes = cross.distance(chest.with_y(cross.y.clamp(chest.y - 0.6, chest.y + 0.5)));
            let on_course = passes < 0.6;
            let level = (((speed - lo) / (hi - lo)).clamp(0.0, 1.0) * if on_course { 2.0 } else { 1.0 }).min(1.0);
            if level > self.danger {
                self.danger = level;
                self.danger_rose = now;
            }
            // A thrust: its point driven along its blade (its motion over 0.1 along it).
            let thrust = t.point && (t.b - t.a).normalize_or_zero().dot(t.vel / speed) > 0.1;
            if incoming.is_none_or(|i| passes < i.passes) {
                incoming = Some(Incoming { cross, vel: t.vel, passes, on_course, soon: from / speed, thrust, tip: t.b });
            }
        }
        self.danger = (self.danger - DANGER_FALL * dt).max(0.0);
        // The guard: towards where the blow will cross (in its own frame), and the block weight by
        // how near its way passes.
        let on_guard = self.defending(me, sight) && armed;
        let want_block = match (incoming, on_guard) {
            (Some(i), true) => ((BLOCK_FAR - i.passes) / (BLOCK_FAR - BLOCK_NEAR)).clamp(0.0, 1.0),
            _ => 0.0,
        };
        self.block += (want_block - self.block).clamp(-BLOCK_SPEED * dt, BLOCK_SPEED * dt);
        let shield_hand = (0..2).find(|&h| hands[h].is_some_and(|x| x.shield));
        let guard_hand = match (shield_hand, incoming) {
            (Some(h), _) => h,
            (None, Some(i)) if hands[0].is_some() && hands[1].is_some() => usize::from((local * (i.cross - at)).x >= 0.0),
            _ => (0..2).rev().find(|&h| hands[h].is_some()).unwrap_or(1),
        };
        let rest = mirror(if shield_hand.is_some() { SHIELD_GUARD } else { self.idle.guard() }, guard_hand);
        let (guard_target, dir_target) = match (incoming.filter(|_| on_guard), shield_hand) {
            (Some(i), Some(_)) => {
                // The shield to the crossing point, its face to the blow.
                let c = local * (i.cross - at);
                let c = Vec3::new(c.x.clamp(-0.45, 0.35), c.y.clamp(0.9, 1.75), 0.38);
                let face_to = (-(local * i.vel)).with_y(0.0).normalize_or(Vec3::Z);
                (Key { t: 0.0, wrist: c, blade: Vec3::Y, edge: face_to }, Vec2::new(c.x / 0.6, (c.y - GUARD_CENTRE) / 0.6).clamp_length_max(1.0))
            }
            (Some(i), None) => {
                // The blade across the blow: up when it comes from the side, level when from above.
                let c = local * (i.cross - at);
                let v = (local * i.vel).normalize_or(Vec3::NEG_Z);
                let side = if c.x >= 0.0 { 1.0 } else { -1.0 };
                let x = if guard_hand == 1 { (c.x * 0.6).clamp(-0.3, 0.45) } else { (c.x * 0.6).clamp(-0.45, 0.3) };
                let wrist = Vec3::new(x, c.y.clamp(1.0, 1.7) - 0.15, 0.38);
                let blade = if v.y.abs() > 0.6 { Vec3::new(-side, 0.25, 0.3).normalize() } else { Vec3::new(side * 0.15, 1.0, 0.25).normalize() };
                // Its guard pose (its side and height round its defence's centre, the one-handed
                // stance snapping fully to a side above it).
                let mut dir = Vec2::new(c.x, c.y - GUARD_CENTRE).normalize_or_zero();
                if self.stance == Stance::OneHanded && dir.y > 0.0 {
                    dir.x = if dir.x >= 0.0 { 1.0 } else { -1.0 };
                }
                (Key { t: 0.0, wrist, blade, edge: (-v).normalize_or(Vec3::Z) }, dir)
            }
            _ => (rest, Vec2::ZERO),
        };
        let g = (GUARD_SPEED * dt).min(1.0);
        self.guard = self.guard.lerp(guard_target, g);
        self.guard_dir += (dir_target - self.guard_dir).clamp_length_max(GUARD_SPEED * dt);
        // Dodging: a blow on course, soon, that its guard doesn't cover (or a thrust, which it won't
        // take on a sword), by its chance at what it's lost of its health; back a metre if there's
        // room.
        if let Some(i) = incoming.filter(|_| self.can_dodge(me, sight)) {
            let soon = i.soon <= HIT_SOON + DODGE_WAIT as f32;
            let covered = soon && armed && (shield_hand.is_some() || !i.thrust) && match shield_hand {
                Some(_) => self.block >= RIPOSTE_BLOCK,
                None => hands.iter().flatten().any(|h| guards(h.blade, i.tip, i.vel, chest)),
            };
            if !covered && i.on_course && (soon || self.dodge_wait.is_some()) {
                if self.dodge_wait.is_none() && self.rand() <= dodge_chance(d.dodge, me.lost) {
                    self.dodge_wait = Some(now);
                }
                if let Some(t) = self.dodge_wait
                    && now - t >= DODGE_WAIT
                {
                    self.dodge_wait = None;
                    let room = at - face * Vec3::Z * DODGE_BACK;
                    let free = room.x > crate::content::ROOM_MIN.x + 0.3 && room.x < crate::content::ROOM_MAX.x - 0.3 && room.z > crate::content::ROOM_MIN.z + 0.3 && room.z < crate::content::ROOM_MAX.z - 0.3;
                    let grabbed = hands.iter().flatten().any(|h| h.players);
                    if i.soon <= HIT_SOON && now >= self.next_dodge && free && !grabbed {
                        let y = i.cross.y - at.y;
                        let (low, high) = (y < 0.97 * 0.85, y > 0.97 + (1.58 - 0.97) * 0.64);
                        self.dodge = Some(Dodge { low: if low { 1.0 } else { 0.0 }, high: if high { 1.0 } else { 0.0 }, since: now });
                        self.next_dodge = now + self.range(DODGE_COOLDOWN) as f64;
                        tracing::info!("brain: dodges back {}{}", if low { "(low)" } else if high { "(high)" } else { "" }, if i.thrust { " from a thrust" } else { "" });
                    }
                }
            } else if !i.on_course {
                self.dodge_wait = None;
            }
        }
        if self.dodge.is_some_and(|x| now - x.since > DODGE_TIME / d.dodge_speed as f64) {
            self.dodge = None;
        }
        // Attacking, once a brain cycle: allowed by its delay, the player in front and in reach,
        // fewer of its side at them than its tier allows and none of them in its way, and not too
        // much in danger (it skips an attack if the danger rose in the last 0.2 s, or by the
        // danger's chance, unless it was just hurt).
        let cooled = d.between.0 <= 0.0 || now >= self.attack_end + self.attack_delay as f64;
        let engaged = sight.engaged < d.melee_max;
        let mut walk = Vec3::ZERO;
        let mut look = flat;
        if now >= self.cycle {
            self.cycle = now + (0.1 + 0.9 * ((dist - 2.0) / 18.0).clamp(0.0, 1.0)) as f64;
            let facing = (face * Vec3::Z).with_y(0.0).angle_between(flat).to_degrees() <= ATTACK_ANGLE;
            let reach = max_reach(hands);
            let in_way = sight.allies.iter().any(|&a| {
                let l = local * (a - at);
                (a - at).with_y(0.0).length() < reach || l.z > 0.0 && l.z < dist && l.x.abs() < reach
            });
            let danger = self.danger > 0.0 && now - self.hurt_at > HURT_IGNORES_DANGER && (now - self.danger_rose < 0.2 || self.rand() <= self.danger);
            if self.mode == Mode::Fighting && armed && !busy && self.dodge.is_none() && self.swing.is_none() && self.queued.is_none() && cooled && engaged && facing && !in_way && !danger
                && let Some(m) = self.pick_opening(me, sight)
            {
                // It lowers a raised guard first (0.15 s times the block weight).
                let delay = if self.block >= 0.5 { 0.1 * self.block * 1.5 } else { 0.0 };
                self.queued = Some((m, now + delay as f64, false));
                tracing::info!("brain: attacks: {} at {dist:.2} m{}", m.name, if delay > 0.0 { " (its guard coming down)" } else { "" });
            }
        }
        match self.mode {
            Mode::Waiting | Mode::Gone => look = Vec3::ZERO,
            Mode::Fleeing(exit) => {
                let to = (exit - at).with_y(0.0);
                walk = local * to.normalize_or_zero() * RUN.min(to.length() * 3.0);
                look = to;
                if to.length() < 0.5 {
                    tracing::info!("brain: reaches the way out and is gone");
                    self.mode = Mode::Gone;
                }
            }
            Mode::Fetching(k, since) => {
                if let Some(&(_, p)) = sight.loose.iter().find(|(i, _)| *i == k) {
                    let to = (p - at).with_y(0.0);
                    look = to;
                    match since {
                        None if to.length() > PICK_FROM => walk = local * to.normalize_or_zero() * WALK.min(to.length() * 2.0),
                        None => self.mode = Mode::Fetching(k, Some(now)),
                        Some(t) if now - t > PICK_GIVE_UP => self.mode = Mode::Fetching(k, None),
                        Some(_) => {}
                    }
                }
            }
            Mode::Fighting => {
                // Spacing: engaged (no more of its side at the player than its tier allows) it waits
                // past the player's reach and steps in to strike once its delay is up; otherwise it
                // keeps its safe distance; either way it strafes round them.
                let my_reach = weapon_reach(Side::Both, hands);
                let (near, far) = self.melee_band(hands);
                let (lo, hi) = if engaged { let lo = near.max(KEEP + my_reach + sight.reach.min(KEEP) + ARMS) + RANGE_DELTA; (lo, lo + (far - near)) } else { (d.safe + SAFE_OFFSET, d.safe + SAFE_OFFSET + SAFE_THICKNESS) };
                let towards = flat.normalize_or_zero();
                let speed = if engaged && cooled && dist < hi + 1.0 {
                    // In to strike: forward to its nearest move's range, back if nearer.
                    if dist > near { WALK * if dist > lo { 0.75 } else { 0.5 } } else { -WALK * 0.5 * d.backward }
                } else if dist > hi {
                    WALK
                } else if dist < lo {
                    -WALK * d.backward
                } else {
                    0.0
                };
                if now > self.strafe.1 {
                    let side = if self.rand() < 0.5 { -1.0 } else { 1.0 };
                    self.strafe = (side, now + self.range(d.strafe).max(0.1) as f64);
                }
                // Round the player clear of its own side: away from the nearest one in its reach.
                let across = Vec3::Y.cross(towards);
                let crowd = sight.allies.iter().copied().filter(|a| (*a - at).with_y(0.0).length() < max_reach(hands)).min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)));
                if let Some(a) = crowd {
                    self.strafe.0 = if (a - at).dot(across) > 0.0 { -1.0 } else { 1.0 };
                }
                let sideways = across * self.strafe.0 * WALK * STRAFE_RATIO * d.sideways;
                let strafe = if crowd.is_some() || dist > lo - 0.3 && dist < hi + 0.3 && !(engaged && cooled) { sideways } else { Vec3::ZERO };
                walk = local * (towards * speed + strafe);
            }
        }
        // A swing carries it forward on its root (it doesn't walk while it swings).
        if let Some(s) = self.swing {
            let z = s.mv.root(s.progress(now));
            walk = if s.back.is_none() { Vec3::Z * (z - self.root) / dt.max(1e-4) } else { Vec3::ZERO };
            self.root = z;
        }
        if self.dodge.is_some() {
            walk = Vec3::NEG_Z * DODGE_BACK / DODGE_TIME as f32 * d.dodge_speed;
        }
        if me.staggered || me.rising || self.queued.is_some() && self.swing.is_none() {
            walk = Vec3::ZERO;
        }
        // Held standing: it stops, turns to whoever holds it, and its feet step after its head (its
        // body for a wrist).
        let mut crouch: f32 = 0.04;
        if let Some(g) = held {
            walk = Vec3::ZERO;
            let head = me.head.with_y(at.y);
            let (step, threshold) = match g.handle {
                Handle::Neck | Handle::Mouth | Handle::Skull | Handle::Torso | Handle::Shoulder => (STEP_AFTER, 0.0),
                Handle::Wrist => (1.0, 0.2),
                Handle::Hand | Handle::Legs => (0.0, 0.0),
            };
            let off = head - at;
            if step > 0.0 && off.length() > threshold {
                walk = local * off * WALK * step;
            }
            let front = (local * (g.grabber - at)).z > 0.0;
            look = match g.handle {
                Handle::Neck | Handle::Mouth | Handle::Skull | Handle::Torso | Handle::Shoulder => (g.grabber - at).with_y(0.0) * if front { 1.0 } else { -1.0 },
                _ => Vec3::ZERO,
            };
            // Its height follows the hand: pressed down, it crouches.
            if matches!(g.handle, Handle::Neck | Handle::Mouth | Handle::Skull | Handle::Torso | Handle::Shoulder) {
                let ratio = ((g.grip.y - at.y) / g.stand.max(0.3)).clamp(0.5, 1.0);
                crouch = crouch.max((1.0 - ratio) * 0.97);
            }
        }
        // The pose: its stance, its arms where its swing, guard, reach or hold has them, its legs
        // stepping as it walks.
        let mut pose = Pose::default();
        let fetch = match self.mode {
            Mode::Fetching(k, Some(t)) => sight.loose.iter().find(|(i, _)| *i == k).map(|&(_, p)| (p, ((now - t) / STOOP_TIME).min(1.0) as f32)),
            _ => None,
        };
        // Stooping for a weapon on the floor: it squats and bends over it.
        let stoop = fetch.map_or(0.0, |(_, s)| smooth(s));
        if stoop > 0.0 {
            crouch = crouch.max(0.4 * stoop);
        }
        squat(&mut pose, crouch);
        if stoop > 0.0 {
            bend(&mut pose, 1.1 * stoop);
        }
        let speed = walk.length();
        if speed > 0.1 {
            let phase = (now as f32 * (3.0 + speed * 2.0)).sin();
            let stride = (speed * 0.35).min(0.5);
            for leg in 0..2 {
                let s = if leg == 0 { phase } else { -phase };
                let dir = walk.with_y(0.0).normalize_or(Vec3::Z);
                let axis = Vec3::Y.cross(dir).normalize_or(Vec3::X);
                pose.turn[anim::THIGH[leg]] = Quat::from_axis_angle(axis, s * stride) * pose.turn[anim::THIGH[leg]];
                pose.turn[anim::SHIN[leg]] = Quat::from_rotation_x(0.2 + (s.max(0.0)) * stride) * pose.turn[anim::SHIN[leg]];
            }
        }
        // The body turns into a swing a little; a dodge leans it back, ducks or springs it.
        if let Some(s) = self.swing {
            let key = s.mv.key(s.progress(now), if s.mv.side == Side::Left { 0 } else { 1 });
            pose.turn[anim::TORSO] = Quat::from_rotation_y(-key.wrist.x * 0.5) * Quat::from_rotation_x(0.1) * pose.turn[anim::TORSO];
        }
        if let Some(x) = self.dodge {
            let k = (((now - x.since) / (DODGE_TIME / d.dodge_speed as f64)) as f32).clamp(0.0, 1.0);
            let k = (std::f32::consts::PI * k).sin();
            pose.turn[anim::TORSO] = Quat::from_rotation_x(-0.35 * k * (1.0 - x.high) + 0.3 * k * x.high) * pose.turn[anim::TORSO];
            if x.high > 0.0 {
                squat(&mut pose, 0.04 + 0.3 * k);
            }
            if x.low > 0.0 {
                pose.pelvis.y += 0.1 * k;
            }
        }
        // Struggling free: a twist of its body.
        if let Some(t) = self.escaping.filter(|&t| now - t < 0.6) {
            let k = (std::f32::consts::PI * ((now - t) / 0.6) as f32).sin();
            pose.turn[anim::TORSO] = Quat::from_rotation_y(0.5 * k) * pose.turn[anim::TORSO];
        }
        let mut attacking = [false; 2];
        let mut guarding = [false; 2];
        for hand in 0..2 {
            let key = match hands[hand] {
                None => {
                    // An empty hand: down at its side, reaching for what it fetches, or at its throat
                    // while a hand chokes it.
                    match (fetch, held) {
                        (Some((p, s)), _) if hand == 1 => {
                            let l = local * (p - at);
                            let reach = Key { t: 0.0, wrist: Vec3::new(l.x.clamp(-0.1, 0.5), l.y.max(0.05) + 0.06, l.z.clamp(0.1, 0.7)), blade: Vec3::new(0.0, -0.2, 1.0), edge: Vec3::Y };
                            mirror(REST, hand).lerp(reach, smooth(s))
                        }
                        (_, Some(g)) if g.handle == Handle::Neck => mirror(k(0.0, [0.07, 1.47, 0.13], [0.0, 0.2, 1.0], [-1.0, 0.0, 0.0]), hand),
                        _ => mirror(REST, hand),
                    }
                }
                Some(Hold { shield, .. }) => match self.swing {
                    Some(s) if s.mv.side.has(hand) => {
                        attacking[hand] = s.back.is_none();
                        s.mv.key(s.progress(now), hand)
                    }
                    _ if hand == guard_hand && on_guard => {
                        // The hand that guards: towards the blow by the block weight.
                        guarding[hand] = self.block > 0.3;
                        let _ = shield;
                        rest.lerp(self.guard, self.block)
                    }
                    _ if shield => mirror(SHIELD_GUARD, hand),
                    _ => mirror(self.idle.guard(), hand),
                },
            };
            // A held arm goes where the hand holding it has it (its wrist pulled to the grip: wholly
            // for the hand and wrist, a quarter for the shoulder).
            let key = match held {
                Some(g) if g.limb == hand && matches!(g.handle, Handle::Hand | Handle::Wrist | Handle::Shoulder) => {
                    let weight = if g.handle == Handle::Shoulder { 0.25 } else { 1.0 };
                    attacking[hand] = false;
                    guarding[hand] = false;
                    Key { wrist: key.wrist.lerp(local * (g.grip - at), weight), ..key }
                }
                _ => key,
            };
            // Blending from where it broke off.
            let key = match self.blend[hand] {
                Some((from, since)) if now - since < BLEND => from.lerp(key, smooth(((now - since) / BLEND) as f32)),
                _ => {
                    self.blend[hand] = None;
                    key
                }
            };
            self.keys[hand] = key;
            arm(&mut pose, hand, key);
        }
        // A held leg goes where the hand has it.
        if let Some(g) = held.filter(|g| g.handle == Handle::Legs) {
            aim_leg(&mut pose, g.limb, local * (g.grip - at));
        }
        let face_to = if look.length() > 0.1 { Some(look) } else { None };
        Thought { pose, walk, face_to, attacking, guarding, shout }
    }

    /// The idle a move leaves it in: the simple one, or one of the others it has a move from.
    fn next_idle(&mut self, then: Then, hands: [Option<Hold>; 2]) -> Idle {
        let level = self.tier.difficulty().level;
        let usable = |i: Idle, b: &Brain| b.stance.opening().iter().any(|m| m.from == Some(i) && m.difficulty <= level && b.can_swing(m, hands));
        let mut pool: Vec<Idle> = [Idle::Simple, Idle::Thrusting, Idle::High].into_iter().filter(|&i| usable(i, self)).collect();
        if then == Then::Other {
            pool.retain(|&i| i != self.idle);
        }
        if then == Then::Simple || pool.is_empty() {
            return Idle::Simple;
        }
        let pick = ((self.rand() * pool.len() as f32) as usize).min(pool.len() - 1);
        pool[pick]
    }

    /// The distances its idle's moves land at, from nearest to furthest (each move's own range,
    /// past its weapon's reach).
    fn melee_band(&self, hands: [Option<Hold>; 2]) -> (f32, f32) {
        let level = self.tier.difficulty().level;
        let mut band: Option<(f32, f32)> = None;
        for m in self.stance.opening().iter().filter(|m| m.difficulty <= level && m.from.is_none_or(|i| i == self.idle) && self.can_swing(m, hands)) {
            let reach = if m.reach { weapon_reach(m.side, hands) } else { 0.0 };
            let (a, b) = (m.range.0 + reach, m.range.1 + reach);
            band = Some(band.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b))));
        }
        band.unwrap_or((1.5, 2.0))
    }
}

/// It changes its idle this often (s).
const IDLE_EVERY: (f32, f32) = (5.0, 10.0);

/// The height its guard poses are measured round (person space: its defence capsule's centre
/// raised 0.35).
const GUARD_CENTRE: f32 = 1.15;
/// A guard at least this far up ripostes; a shield counters this often.
const RIPOSTE_BLOCK: f32 = 0.5;
const SHIELD_COUNTER: f32 = 0.3;

/// The fastest of the player's blows coming at it: where its way crosses, how fast, how near it
/// passes, whether it's on course, how soon it gets there (s), whether it's a thrust, and its tip.
#[derive(Clone, Copy, Debug)]
struct Incoming {
    cross: Vec3,
    vel: Vec3,
    passes: f32,
    on_course: bool,
    soon: f32,
    thrust: bool,
    tip: Vec3,
}

/// Its reach with its arm (its weapon's, its arm's length and half the width of its shoulders):
/// none of its side may be nearer, or in its way, when it attacks.
fn max_reach(hands: [Option<Hold>; 2]) -> f32 {
    weapon_reach(Side::Both, hands) + ARMS + SHOULDERS * 0.5
}
const SHOULDERS: f32 = 0.4;

/// Its weapons' reach for a move with `side` (the blade's from the fist times `REACH_RATIO`).
fn weapon_reach(side: Side, hands: [Option<Hold>; 2]) -> f32 {
    (0..2).filter(|&h| side.has(h)).filter_map(|h| hands[h].filter(|x| !x.shield)).map(|x| x.reach * REACH_RATIO).fold(0.0, f32::max)
}

/// Its dodge chance at the share `lost` of its health: straight through its keys at none, 60% and
/// all of it.
fn dodge_chance(curve: [f32; 3], lost: f32) -> f32 {
    let lost = lost.clamp(0.0, 1.0);
    if lost <= 0.6 { curve[0] + (curve[1] - curve[0]) * lost / 0.6 } else { curve[1] + (curve[2] - curve[1]) * (lost - 0.6) / 0.4 }
}

/// Its own blade `blade` (world) is in the way of a blow whose tip at `tip` moves at `vel`, before it
/// reaches the body at `chest` (a capsule cast along the blow, a hair thick, out to twice the
/// distance: its weapon met before its body covers it).
fn guards(blade: (Vec3, Vec3), tip: Vec3, vel: Vec3, chest: Vec3) -> bool {
    let reach = tip.distance(chest) * 2.0;
    let end = tip + vel.normalize_or_zero() * reach;
    let (s, t) = closest(tip, end, blade.0, blade.1);
    let meets = tip.lerp(end, s).distance(blade.0.lerp(blade.1, t)) < 0.06;
    let (body, _) = closest(tip, end, chest - Vec3::Y * 0.6, chest + Vec3::Y * 0.5);
    meets && s <= body + 0.05
}

/// What it does this frame.
pub struct Thought {
    pub pose: Pose,
    /// How it walks (its own frame, m/s), and where it would face (world, flat).
    pub walk: Vec3,
    pub face_to: Option<Vec3>,
    /// Which hands swing and which guard (their pins), and whether an attack began (its shout).
    pub attacking: [bool; 2],
    pub guarding: [bool; 2],
    pub shout: bool,
}

/// Standing out cold (its brain off): its legs as they stand, its arms down.
pub fn dazed() -> Pose {
    let mut pose = Pose::default();
    squat(&mut pose, 0.04);
    for hand in 0..2 {
        arm(&mut pose, hand, mirror(REST, hand));
    }
    pose
}

/// The arm of `hand` posed so its wrist is at `key.wrist` and the weapon in it lies as `key` says
/// (the weapon's +Z along the blade, +X along its edge, held with the hand's frame).
fn arm(pose: &mut Pose, hand: usize, key: Key) {
    let blade = key.blade.normalize_or(Vec3::Z);
    let edge = (key.edge - blade * key.edge.dot(blade)).normalize_or(blade.any_orthonormal_vector());
    let rot = Quat::from_mat3(&glam::Mat3::from_cols(edge, blade.cross(edge), blade)).normalize();
    pose.arm(hand, key.wrist, rot, 0.0);
}

/// Both legs bent so the pelvis is `drop` lower with the feet where they stand: each thigh forward
/// and its shin back twice as far (a knee over its foot).
fn squat(pose: &mut Pose, drop: f32) {
    let leg = anim::hip(0).y - 0.07;
    let bend = (1.0 - drop / leg).clamp(-1.0, 1.0).acos();
    pose.pelvis.y = -drop;
    for l in 0..2 {
        pose.turn[anim::THIGH[l]] = Quat::from_rotation_x(-bend);
        pose.turn[anim::SHIN[l]] = Quat::from_rotation_x(2.0 * bend);
    }
}

/// Bent over at the waist by `angle` (radians): the pelvis tips forward as far as the torso's joint
/// leaves to it, the thighs kept where they were.
fn bend(pose: &mut Pose, angle: f32) {
    let torso = angle.min(0.5);
    let pelvis = angle - torso;
    pose.turn[anim::PELVIS] = Quat::from_rotation_x(pelvis) * pose.turn[anim::PELVIS];
    pose.turn[anim::TORSO] = Quat::from_rotation_x(torso) * pose.turn[anim::TORSO];
    for l in 0..2 {
        pose.turn[anim::THIGH[l]] = Quat::from_rotation_x(-pelvis) * pose.turn[anim::THIGH[l]];
    }
}

/// Leg `leg` pointed (straight) at `target` (person space) from its hip.
fn aim_leg(pose: &mut Pose, leg: usize, target: Vec3) {
    let hip = anim::hip(leg) + pose.pelvis;
    let rest = (anim::knee(leg) - anim::hip(leg)).normalize();
    let want = (target - hip).normalize_or(rest);
    pose.turn[anim::THIGH[leg]] = Quat::from_rotation_arc(rest, want);
    pose.turn[anim::SHIN[leg]] = Quat::IDENTITY;
}

/// The closest points between segments: (s, t) along each.
fn closest(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (f32, f32) {
    let d1 = p1 - p0;
    let d2 = q1 - q0;
    let r = p0 - q0;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    if a <= 1e-9 && e <= 1e-9 {
        return (0.0, 0.0);
    }
    if a <= 1e-9 {
        return (0.0, (f / e).clamp(0.0, 1.0));
    }
    let c = d1.dot(r);
    if e <= 1e-9 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = d1.dot(d2);
    let denom = a * e - b * b;
    let s = if denom > 1e-9 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let t = ((b * s + f) / e).clamp(0.0, 1.0);
    (((b * t - c) / a).clamp(0.0, 1.0), t)
}

/// Within reach of its hand.
pub fn within_reach(hand: Vec3, p: Vec3) -> bool {
    hand.distance(p) < REACH
}

/// It may take up its fetched weapon now (1.05 s into stooping for it).
pub fn taking(mode: Mode, now: f64) -> Option<usize> {
    match mode {
        Mode::Fetching(k, Some(t)) if now - t >= TAKE_AT => Some(k),
        _ => None,
    }
}
