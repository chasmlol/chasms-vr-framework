//! The arena's fighters' brains: how each attacks, defends, takes a hit and arms itself,
//! in its stances (one-handed, dual wield, sword and shield), at Easy, Medium and Hard
//! difficulty. A brain animates its body
//! (`crate::anim`'s pose, which the body's pins follow: `crate::ragdoll`), so everything it does is
//! physical: its swings are its weapon swung by its arm, which the player's weapon meets, blocks and
//! deflects; its guard is its weapon or shield put in the way of the player's.
//!
//! * **Stances**: one-handed (a weapon, the other hand free), dual wield (a weapon
//!   in each hand, alternating) and sword and shield.
//! * **Attacking**: from its stance's moves, those its difficulty allows (Easy
//!   only plain swings; Hard thrusts and lunges too), the one whose reach fits where the player will
//!   be; at most so often (Easy every 2 to 3 s, Medium 0.5 to 1, Hard 0 to 0.5). A move is a wind-up,
//!   a strike and a recovery; its weapon hurts only between the strike's start and end (its
//!   damage window). It swings slower with a heavier weapon (`1.5 / mass`, down to 70%).
//! * **Parried** (its swing met the player's weapon): it
//!   rolls the dice: a chained follow-up (a combo), nothing (the swing plays back half a second and
//!   it carries on), or a recoil (a full stagger), the odds by difficulty and how many it has chained.
//! * **Defending**: it watches the player's weapon; on a course for it within
//!   reach, it puts its guard in the way (its weapon across the blow, or its shield), blending there
//!   at 7 a second, its arm pinned three times as stiff. Danger makes it hold off attacking.
//!   A blocked blow it may riposte.
//! * **Disarmed**, it goes for the nearest weapon lying loose and picks it
//!   up; with none, it flees (its only fear: unarmed with nothing to take up).

use glam::{Quat, Vec3};
use rustvr_melee::body::Doing;

use crate::anim::{self, Pose};

/// How hard a fighter is: its difficulty tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Easy,
    Medium,
    Hard,
}

/// What a tier changes (every field that differs between the tiers).
#[derive(Clone, Copy, Debug)]
pub struct Difficulty {
    /// Its moves are filtered by this (`difficulty`: Easy 1, Medium 2, Hard 3).
    pub level: u32,
    /// Seconds between attacks (least and most).
    pub between: (f32, f32),
    /// Its swings' speed.
    pub speed: f32,
    /// It keeps this far off while it waits its turn (its safe distance, m).
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
    /// Strafing round the player, every so often (least and most, s).
    pub strafe: (f32, f32),
}

impl Tier {
    pub fn difficulty(self) -> Difficulty {
        match self {
            Tier::Easy => Difficulty { level: 1, between: (2.0, 3.0), speed: 1.0, safe: 3.75, cancel: 0.05, roll: (0.30, 0.35), roll_per_chain: (0.10, 0.15), riposte_delay: (1.5, 2.0), chain_speed: -0.05, recovery: 0.9, recoil_cooldown: 1.0, strafe: (2.0, 6.0) },
            Tier::Medium => Difficulty { level: 2, between: (0.5, 1.0), speed: 1.05, safe: 3.5, cancel: 0.08, roll: (0.10, 0.15), roll_per_chain: (0.05, 0.12), riposte_delay: (0.5, 1.2), chain_speed: -0.02, recovery: 1.0, recoil_cooldown: 1.5, strafe: (1.0, 4.0) },
            Tier::Hard => Difficulty { level: 3, between: (0.0, 0.5), speed: 1.05, safe: 3.25, cancel: 0.10, roll: (0.0, 0.05), roll_per_chain: (0.03, 0.11), riposte_delay: (0.5, 1.0), chain_speed: -0.01, recovery: 1.1, recoil_cooldown: 2.0, strafe: (0.0, 2.0) },
        }
    }
}

/// The stances: one-handed, dual wield, sword and shield.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    OneHanded,
    DualWield,
    SwordShield,
}

/// A key of a move: when (its share of the move), where the wrist is (its own frame, facing +Z),
/// which way the blade points and which way its edge faces.
#[derive(Clone, Copy, Debug)]
struct Key {
    t: f32,
    wrist: Vec3,
    blade: Vec3,
    edge: Vec3,
}

const fn k(t: f32, wrist: [f32; 3], blade: [f32; 3], edge: [f32; 3]) -> Key {
    Key { t, wrist: Vec3::from_array(wrist), blade: Vec3::from_array(blade), edge: Vec3::from_array(edge) }
}

/// A move: its keys for the right hand (the left mirrors them), how long it
/// takes, when it can hurt (its share: `start`, `end`), how far it reaches (from the feet, m), how
/// far it steps in, the difficulty it needs, and the move it can chain into.
pub struct Move {
    pub name: &'static str,
    keys: &'static [Key],
    pub seconds: f32,
    pub start: f32,
    pub end: f32,
    pub range: f32,
    pub step: f32,
    pub difficulty: u32,
    pub chain: Option<usize>,
    pub thrust: bool,
}

/// The one-handed moves: swings from the right, from the left and from above at arm's reach,
/// their stepping versions, and a thrust (Hard).
pub static MOVES: [Move; 7] = [
    Move {
        name: "swing from the right",
        keys: &[
            k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
            k(0.35, [0.5, 1.5, -0.05], [0.35, 0.55, -0.75], [-0.5, 0.0, 0.85]),
            k(0.55, [0.15, 1.35, 0.4], [-0.2, 0.1, 1.0], [-1.0, 0.0, -0.2]),
            k(0.75, [-0.3, 1.15, 0.3], [-0.85, -0.15, 0.5], [-0.5, -0.1, -0.85]),
            k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
        ],
        seconds: 1.3,
        start: 0.4,
        end: 0.72,
        range: 1.1,
        step: 0.0,
        difficulty: 1,
        chain: Some(1),
        thrust: false,
    },
    Move {
        name: "swing from the left",
        keys: &[
            k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
            k(0.35, [-0.25, 1.45, 0.05], [-0.5, 0.55, -0.65], [0.6, 0.0, 0.8]),
            k(0.55, [0.05, 1.3, 0.45], [0.3, 0.05, 0.95], [1.0, 0.0, -0.2]),
            k(0.75, [0.45, 1.15, 0.25], [0.85, -0.2, 0.45], [0.45, -0.1, -0.85]),
            k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
        ],
        seconds: 1.3,
        start: 0.4,
        end: 0.72,
        range: 1.05,
        step: 0.0,
        difficulty: 1,
        chain: Some(2),
        thrust: false,
    },
    Move {
        name: "swing from above",
        keys: &[
            k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
            k(0.38, [0.25, 1.85, -0.05], [0.0, 0.5, -0.85], [0.0, 0.85, 0.5]),
            k(0.58, [0.15, 1.35, 0.45], [0.0, -0.2, 1.0], [0.0, -1.0, -0.2]),
            k(0.76, [0.1, 1.0, 0.35], [0.0, -0.85, 0.5], [0.0, -0.5, -0.85]),
            k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
        ],
        seconds: 1.35,
        start: 0.42,
        end: 0.74,
        range: 1.1,
        step: 0.0,
        difficulty: 1,
        chain: Some(0),
        thrust: false,
    },
    Move {
        name: "stepping swing from the right",
        keys: &[
            k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
            k(0.35, [0.5, 1.5, -0.05], [0.35, 0.55, -0.75], [-0.5, 0.0, 0.85]),
            k(0.55, [0.15, 1.35, 0.4], [-0.2, 0.1, 1.0], [-1.0, 0.0, -0.2]),
            k(0.75, [-0.3, 1.15, 0.3], [-0.85, -0.15, 0.5], [-0.5, -0.1, -0.85]),
            k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
        ],
        seconds: 1.5,
        start: 0.4,
        end: 0.72,
        range: 1.95,
        step: 0.85,
        difficulty: 2,
        chain: Some(1),
        thrust: false,
    },
    Move {
        name: "stepping swing from above",
        keys: &[
            k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
            k(0.38, [0.25, 1.85, -0.05], [0.0, 0.5, -0.85], [0.0, 0.85, 0.5]),
            k(0.58, [0.15, 1.35, 0.45], [0.0, -0.2, 1.0], [0.0, -1.0, -0.2]),
            k(0.76, [0.1, 1.0, 0.35], [0.0, -0.85, 0.5], [0.0, -0.5, -0.85]),
            k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
        ],
        seconds: 1.55,
        start: 0.42,
        end: 0.74,
        range: 2.0,
        step: 0.9,
        difficulty: 2,
        chain: Some(0),
        thrust: false,
    },
    Move {
        name: "thrust",
        keys: &[
            k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
            k(0.35, [0.3, 1.3, -0.1], [0.0, 0.05, 1.0], [-1.0, 0.0, 0.0]),
            k(0.55, [0.12, 1.32, 0.62], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]),
            k(0.75, [0.2, 1.3, 0.3], [0.0, 0.1, 1.0], [-1.0, 0.0, 0.0]),
            k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
        ],
        seconds: 1.1,
        start: 0.4,
        end: 0.66,
        range: 1.15,
        step: 0.0,
        difficulty: 3,
        chain: Some(0),
        thrust: true,
    },
    Move {
        name: "lunging thrust",
        keys: &[
            k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
            k(0.35, [0.3, 1.3, -0.1], [0.0, 0.05, 1.0], [-1.0, 0.0, 0.0]),
            k(0.55, [0.12, 1.32, 0.62], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]),
            k(0.75, [0.2, 1.3, 0.3], [0.0, 0.1, 1.0], [-1.0, 0.0, 0.0]),
            k(1.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]),
        ],
        seconds: 1.4,
        start: 0.4,
        end: 0.66,
        range: 2.1,
        step: 1.0,
        difficulty: 3,
        chain: Some(1),
        thrust: true,
    },
];

/// The guard: where the weapon hand rests (its own frame), the blade up and forward.
const GUARD: Key = k(0.0, [0.3, 1.25, 0.25], [0.0, 0.6, 0.8], [-1.0, 0.0, 0.0]);
/// A shield's guard: in front of the chest, its face forward.
const SHIELD_GUARD: Key = k(0.0, [-0.2, 1.25, 0.35], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);

impl Move {
    /// Where the wrist is and how the blade lies at `t` (0..1), for hand `hand` (the left mirrored).
    fn at(&self, t: f32, hand: usize) -> Key {
        let t = t.clamp(0.0, 1.0);
        let i = self.keys.iter().rposition(|k| k.t <= t).unwrap_or(0).min(self.keys.len() - 2);
        let (a, b) = (self.keys[i], self.keys[i + 1]);
        let s = smooth((t - a.t) / (b.t - a.t).max(1e-3));
        mirror(
            Key { t, wrist: a.wrist.lerp(b.wrist, s), blade: a.blade.lerp(b.blade, s).normalize_or(a.blade), edge: a.edge.lerp(b.edge, s).normalize_or(a.edge) },
            hand,
        )
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

/// A swing under way: which move, when it started, how fast, which hand; played back from then
/// (a recoil or a "nothing") at half speed.
#[derive(Clone, Copy, Debug)]
pub struct Swing {
    pub mv: usize,
    pub t0: f64,
    pub speed: f32,
    pub hand: usize,
    /// Played back (at -0.5 speed for 0.2 s), from this share of it, since then.
    pub back: Option<(f32, f64)>,
    /// A feint: at the top of its wind-up it turns into another move.
    pub feint: bool,
}

impl Swing {
    /// Its share of the move now (0..1).
    pub fn progress(&self, now: f64) -> f32 {
        match self.back {
            Some((from, since)) => (from - 0.5 * (now - since) as f32 / MOVES[self.mv].seconds).max(0.0),
            None => ((now - self.t0) as f32 * self.speed / MOVES[self.mv].seconds).clamp(0.0, 1.0),
        }
    }

    /// Its weapon can hurt now (between the move's start and end, going forward).
    pub fn hurting(&self, now: f64) -> bool {
        let t = self.progress(now);
        let m = &MOVES[self.mv];
        self.back.is_none() && t >= m.start && t <= m.end
    }
}

/// What a brain sees of the player: the head, and each of their weapons (where its blade's two ends
/// are, how fast its tip moves).
#[derive(Clone, Copy, Debug, Default)]
pub struct Threat {
    pub a: Vec3,
    pub b: Vec3,
    pub vel: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct Sight {
    pub feet: Vec3,
    pub vel: Vec3,
    pub threats: Vec<Threat>,
    /// Weapons lying loose (their ids and where).
    pub loose: Vec<(usize, Vec3)>,
}

/// What a hand holds, as a brain cares: how far its blade reaches from the fist (m), how heavy it
/// is (kg), and whether it's a shield.
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub reach: f32,
    pub mass: f32,
    pub shield: bool,
}

/// What it's up to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Not yet roused (the player isn't near).
    Waiting,
    /// Closing in or keeping its distance, strafing.
    Fighting,
    /// Going for a weapon lying loose (an item index).
    Fetching(usize),
    /// Unarmed with nothing to take up: running.
    Fleeing,
}

pub struct Brain {
    pub tier: Tier,
    pub stance: Stance,
    pub mode: Mode,
    pub swing: Option<Swing>,
    /// When it may attack next, and when it may next riposte or recoil.
    next_attack: f64,
    next_riposte: f64,
    last_recoil: f64,
    /// How many swings it has chained.
    pub chains: u32,
    /// How threatened it feels (0..1, falling 2 a second), and when it was last hurt.
    pub danger: f32,
    hurt_at: f64,
    /// Its guard: where its weapon hand (or shield) goes, blended at 7 a second, and how far up it is
    /// (the block weight).
    guard: Key,
    pub block: f32,
    /// Strafing: which way, until when.
    strafe: (f32, f64),
    /// Which hand swings next (dual wield alternates), and the move it last made.
    next_hand: usize,
    last_move: Option<usize>,
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
/// Its danger: the incoming weapon's speed over the stance's incoming-speed range (1H 2.5 to 6,
/// dual 3 to 8, shield 1.75 to 5 m/s), doubled on course for it; falling 2 a second.
const DANGER_FALL: f32 = 2.0;
/// Walking and strafing speeds (m/s).
const WALK: f32 = 1.4;
const STRAFE: f32 = 0.8;
const RUN: f32 = 3.0;
/// A thing within an arm's reach of its hand it can pick up.
const REACH: f32 = 0.45;
/// The reach the moves' ranges are for: an arming sword's blade (m).
const SWORD_REACH: f32 = 0.8;

impl Brain {
    pub fn new(tier: Tier, stance: Stance, seed: u32) -> Brain {
        Brain {
            tier,
            stance,
            mode: Mode::Waiting,
            swing: None,
            next_attack: 0.0,
            next_riposte: 0.0,
            last_recoil: -10.0,
            chains: 0,
            danger: 0.0,
            hurt_at: -10.0,
            guard: GUARD,
            block: 0.0,
            strafe: (1.0, 0.0),
            next_hand: 1,
            last_move: None,
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

    /// It's swinging (its swing's damage window aside).
    pub fn attacking(&self) -> bool {
        self.swing.is_some_and(|s| s.back.is_none())
    }

    /// Its swing is a feint (for the game's look).
    pub fn feinting(&self) -> bool {
        self.swing.is_some_and(|s| s.feint)
    }

    pub fn doing(&self, now: f64) -> Doing {
        match (self.mode, self.swing) {
            (Mode::Waiting, _) => Doing::Idle,
            (_, Some(s)) if s.back.is_none() && s.progress(now) > 0.0 => Doing::Attacking,
            _ => Doing::Combat,
        }
    }

    /// It was hurt this much of its health: a heavy enough hit cancels its attack
    /// (its cancel share), and being hurt makes it fight back rather than defend.
    pub fn hurt(&mut self, share: f32, now: f64) {
        self.hurt_at = now;
        if share >= self.tier.difficulty().cancel && self.swing.is_some_and(|s| s.back.is_none()) {
            self.swing = None;
        }
        if self.mode == Mode::Waiting {
            self.mode = Mode::Fighting;
        }
    }

    /// Its swing met the player's weapon (as the attacker): it rolls for
    /// a chained follow-up, nothing, or a recoil. Returns whether it recoils (a full stagger).
    pub fn parried(&mut self, now: f64) -> bool {
        let Some(s) = self.swing.filter(|s| s.back.is_none()) else { return false };
        let d = self.tier.difficulty();
        let c = self.chains as f32;
        let r = self.rand();
        let chain_from = d.roll.1 + c * d.roll_per_chain.1;
        let nothing_from = d.roll.0 + c * d.roll_per_chain.0;
        let can_chain = MOVES[s.mv].chain.is_some() && now >= self.next_riposte;
        if r > chain_from && can_chain && chain_from < 1.0 {
            // A combo: this swing stops, its chain starts a moment later, a little slower.
            let mv = MOVES[s.mv].chain.unwrap();
            self.chains += 1;
            let speed = (s.speed * (1.0 + d.chain_speed * self.chains as f32)).max(0.5);
            self.swing = Some(Swing { mv, t0: now + 0.2, speed, hand: s.hand, back: None, feint: false });
            self.next_riposte = now + self.range(d.riposte_delay) as f64;
            tracing::info!("brain: parried, chains into {} ({} chained)", MOVES[mv].name, self.chains);
            false
        } else if r >= nothing_from || now - self.last_recoil < d.recoil_cooldown as f64 {
            // Nothing: the swing plays back at half speed for a moment, then it carries on.
            let t = s.progress(now);
            self.swing = Some(Swing { back: Some((t, now)), ..s });
            tracing::info!("brain: parried, its swing turned back");
            false
        } else {
            self.last_recoil = now;
            self.swing = None;
            self.chains = 0;
            tracing::info!("brain: parried, it recoils");
            true
        }
    }

    /// It blocked the player's blow: a riposte maybe (the blocker's roll, with no chains yet).
    pub fn blocked(&mut self, now: f64) {
        if self.swing.is_some() || now < self.next_riposte || self.block < 0.5 {
            return;
        }
        let d = self.tier.difficulty();
        let r = self.rand();
        if r > d.roll.1 {
            self.next_attack = now;
            self.next_riposte = now + self.range(d.riposte_delay) as f64;
            self.chains = 0;
            tracing::info!("brain: blocked, ripostes");
        }
    }

    /// One frame: what it does, and the pose its body is pinned to; the root moves by `walk` (its
    /// own frame's velocity) and turns to face `face`.
    #[allow(clippy::too_many_arguments)]
    pub fn think(&mut self, now: f64, dt: f32, at: Vec3, face: Quat, hands: [Option<Hold>; 2], sight: &Sight, staggered: bool) -> Thought {
        let d = self.tier.difficulty();
        let to = sight.feet - at;
        let flat = Vec3::new(to.x, 0.0, to.z);
        let dist = flat.length();
        let local = face.inverse();
        let armed = hands.iter().any(|h| h.is_some_and(|h| !h.shield));
        // Roused by the player near.
        if self.mode == Mode::Waiting && dist < ROUSE {
            self.mode = Mode::Fighting;
            self.next_attack = now + self.range(d.between) as f64;
        }
        if !armed && self.mode != Mode::Waiting {
            self.swing = None;
            // The nearest loose weapon not near the player (within 30 m, not within 4 m of an
            // enemy), else away.
            let pick = sight.loose.iter().filter(|(_, p)| p.distance(sight.feet) > 1.5).min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)));
            self.mode = match pick {
                Some(&(k, _)) => Mode::Fetching(k),
                None => Mode::Fleeing,
            };
        } else if armed && matches!(self.mode, Mode::Fetching(_) | Mode::Fleeing) {
            self.mode = Mode::Fighting;
        }
        // Danger: the player's fastest weapon coming its way.
        let mut incoming: Option<(Vec3, Vec3, f32)> = None;
        let chest = at + Vec3::Y * 1.25;
        let (lo, hi) = match self.stance {
            Stance::OneHanded => (2.5, 6.0),
            Stance::DualWield => (3.0, 8.0),
            Stance::SwordShield => (1.75, 5.0),
        };
        for t in &sight.threats {
            let speed = t.vel.length();
            let (near, _) = closest(t.a, t.b, chest, chest);
            if t.a.lerp(t.b, near).distance(chest) > DEFEND || speed < 1.0 {
                continue;
            }
            // On course: its tip's way passes near the body within half a second.
            let ahead = t.b + t.vel * 0.5;
            let (s, _) = closest(t.b, ahead, chest - Vec3::Y * 0.4, chest + Vec3::Y * 0.5);
            let passes = t.b.lerp(ahead, s).distance(chest.with_y(t.b.lerp(ahead, s).y.clamp(chest.y - 0.4, chest.y + 0.5)));
            let on_course = passes < 0.6;
            let level = ((speed - lo) / (hi - lo)).clamp(0.0, 1.0) * if on_course { 2.0 } else { 1.0 };
            self.danger = self.danger.max(level.min(1.0));
            if incoming.is_none_or(|(_, _, p)| passes < p) {
                incoming = Some((t.b.lerp(ahead, s), t.vel, passes));
            }
        }
        self.danger = (self.danger - DANGER_FALL * dt).max(0.0);
        // The guard: towards where the blow will cross (in its own frame), and the block weight by
        // how near its way passes.
        let want_block = match incoming {
            Some((_, _, passes)) => ((BLOCK_FAR - passes) / (BLOCK_FAR - BLOCK_NEAR)).clamp(0.0, 1.0),
            None => 0.0,
        };
        let can_guard = !staggered && self.swing.is_none_or(|s| s.back.is_some()) && matches!(self.mode, Mode::Fighting);
        let want_block = if can_guard { want_block } else { 0.0 };
        self.block += (want_block - self.block).clamp(-BLOCK_SPEED * dt, BLOCK_SPEED * dt);
        let shield_hand = hands.iter().position(|h| h.is_some_and(|h| h.shield));
        let guard_target = match (incoming, shield_hand) {
            (Some((cross, vel, _)), Some(_)) => {
                // The shield to the crossing point, its face to the blow.
                let c = local * (cross - at);
                let c = Vec3::new(c.x.clamp(-0.45, 0.35), c.y.clamp(0.9, 1.75), 0.38);
                let face_to = (-(local * vel)).with_y(0.0).normalize_or(Vec3::Z);
                Key { t: 0.0, wrist: c, blade: Vec3::Y, edge: face_to }
            }
            (Some((cross, vel, _)), None) => {
                // The blade across the blow: up when it comes from the side, level when from above.
                let c = local * (cross - at);
                let v = (local * vel).normalize_or(Vec3::NEG_Z);
                let side = if c.x >= 0.0 { 1.0 } else { -1.0 };
                let wrist = Vec3::new((c.x * 0.6).clamp(-0.3, 0.45), c.y.clamp(1.0, 1.7) - 0.15, 0.38);
                let blade = if v.y.abs() > 0.6 { Vec3::new(-side, 0.25, 0.3).normalize() } else { Vec3::new(side * 0.15, 1.0, 0.25).normalize() };
                Key { t: 0.0, wrist, blade, edge: (-v).normalize_or(Vec3::Z) }
            }
            _ => match shield_hand {
                Some(_) => SHIELD_GUARD,
                None => GUARD,
            },
        };
        let g = (GUARD_SPEED * dt).min(1.0);
        self.guard = Key { t: 0.0, wrist: self.guard.wrist.lerp(guard_target.wrist, g), blade: self.guard.blade.lerp(guard_target.blade, g).normalize_or(GUARD.blade), edge: self.guard.edge.lerp(guard_target.edge, g).normalize_or(GUARD.edge) };
        // Attacking: when it's allowed, in reach of a move, and not too much in danger (it skips an
        // attack with a chance the danger level, unless it was just hurt).
        let mut walk = Vec3::ZERO;
        let mut look = flat;
        match self.mode {
            Mode::Waiting => look = Vec3::ZERO,
            Mode::Fleeing => {
                walk = local * -flat.normalize_or_zero() * RUN;
                look = -flat;
            }
            Mode::Fetching(k) => {
                if let Some(&(_, p)) = sight.loose.iter().find(|(i, _)| *i == k) {
                    let to = (p - at).with_y(0.0);
                    walk = local * to.normalize_or_zero() * WALK.min(to.length() * 2.0);
                    look = to;
                }
            }
            Mode::Fighting => {
                // A feint at the top of its wind-up: the swing turns into another from there.
                if let Some(s) = self.swing.filter(|s| s.feint && s.back.is_none() && s.progress(now) >= 0.33) {
                    let next = (s.mv + 1 + (self.rand() * 2.0) as usize) % 3;
                    tracing::info!("brain: feints: {} turns into {}", MOVES[s.mv].name, MOVES[next].name);
                    self.swing = Some(Swing { mv: next, t0: now - (MOVES[next].seconds * 0.2 / s.speed) as f64, feint: false, ..s });
                }
                if let Some(s) = self.swing
                    && s.progress(now) >= 1.0
                {
                    self.swing = None;
                    self.next_attack = now + self.range(d.between) as f64;
                    self.chains = 0;
                }
                let in_danger = self.danger > 0.0 && now - self.hurt_at > 0.75 && self.rand() < self.danger * dt * 4.0;
                if self.swing.is_none() && !staggered && now >= self.next_attack && !in_danger {
                    // The move whose reach fits where the player will be at its strike.
                    // (The moves' ranges are where the blade lands with an arming sword's reach; a longer
                    // or shorter blade shifts them by the difference.)
                    let weapon_reach = hands.iter().flatten().filter(|h| !h.shield).map(|h| h.reach).fold(0.0f32, f32::max) - SWORD_REACH;
                    let hand = self.attack_hand(hands);
                    let ahead = dist + (sight.vel.dot(-flat.normalize_or_zero())) * -0.3;
                    // It weighs each move that fits by how well it fits, the last one used at 1%, and
                    // picks at random by weight.
                    let fits: Vec<(usize, f32)> = MOVES
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.difficulty <= d.level && (!m.thrust || hands[hand].is_some_and(|h| h.reach > 0.5)))
                        .map(|(i, m)| (i, (m.range + weapon_reach * 0.75 - ahead).abs()))
                        .filter(|(_, miss)| *miss < 0.45)
                        .map(|(i, miss)| (i, (1.0 - miss / 0.45) * if Some(i) == self.last_move { 0.01 } else { 1.0 }))
                        .collect();
                    let total: f32 = fits.iter().map(|(_, w)| w).sum();
                    let mut pick = self.rand() * total;
                    let best = fits.iter().find(|(_, w)| {
                        pick -= w;
                        pick <= 0.0
                    }).or(fits.last()).copied();
                    if let Some((mv, _)) = best {
                        let mass = hands[hand].map_or(1.5, |h| h.mass);
                        let speed = d.speed * (1.5 / mass.max(0.1)).clamp(0.7, 1.0);
                        // A feint now and then (Medium one swing in ten, Hard one in five): plain
                        // fighters never feint; the user asked for it (our call, `docs/MELEE.md`).
                        let feint = self.rand() < match self.tier {
                            Tier::Easy => 0.0,
                            Tier::Medium => 0.1,
                            Tier::Hard => 0.2,
                        };
                        self.swing = Some(Swing { mv, t0: now, speed, hand, back: None, feint });
                        self.last_move = Some(mv);
                        self.chains = 1;
                        tracing::info!("brain: attacks: {} with the {} hand at {dist:.2} m", MOVES[mv].name, if hand == 0 { "left" } else { "right" });
                    }
                }
                // Keep its distance while it waits its turn, strafing; close in when it may strike.
                let want = if self.swing.is_some() || now >= self.next_attack - 0.3 { 1.0 } else { d.safe.min(2.2) };
                let towards = flat.normalize_or_zero();
                let gap = dist - want;
                if now > self.strafe.1 {
                    let side = if self.rand() < 0.5 { -1.0 } else { 1.0 };
                    self.strafe = (side, now + self.range(d.strafe).max(1.0) as f64);
                }
                let sideways = Vec3::Y.cross(towards) * self.strafe.0;
                let close = towards * gap.clamp(-1.0, 1.0) * WALK;
                let strafe = if gap.abs() < 0.6 && self.swing.is_none() { sideways * STRAFE } else { Vec3::ZERO };
                walk = local * (close + strafe);
                // A stepping move steps in through its strike.
                if let Some(s) = self.swing {
                    let m = &MOVES[s.mv];
                    let t = s.progress(now);
                    if m.step > 0.0 && t > 0.25 && t < m.end && s.back.is_none() {
                        walk = Vec3::Z * m.step / (m.seconds * (m.end - 0.25)) * s.speed;
                    } else {
                        walk = Vec3::ZERO;
                    }
                }
            }
        }
        if staggered {
            walk = Vec3::ZERO;
        }
        // The pose: its stance, its arms where its swing, guard or reach has them, its legs stepping
        // as it walks.
        let mut pose = Pose::default();
        let crouch = 0.04;
        pose.pelvis = Vec3::new(0.0, -crouch, 0.0);
        for leg in 0..2 {
            pose.turn[anim::THIGH[leg]] = Quat::from_rotation_x(-0.12);
            pose.turn[anim::SHIN[leg]] = Quat::from_rotation_x(0.2);
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
                pose.turn[anim::SHIN[leg]] = Quat::from_rotation_x(0.2 + (s.max(0.0)) * stride);
            }
        }
        // The body turns into a swing a little.
        if let Some(s) = self.swing {
            let key = MOVES[s.mv].at(s.progress(now), s.hand);
            pose.turn[anim::TORSO] = Quat::from_rotation_y(-key.wrist.x * 0.5) * Quat::from_rotation_x(0.1);
        }
        let mut attacking_hand = None;
        for hand in 0..2 {
            let Some(Hold { shield, .. }) = hands[hand] else {
                // An empty hand: down at its side, or reaching for what it fetches.
                let rest = mirror(k(0.0, [0.28, 0.95, 0.1], [0.0, -0.3, 1.0], [0.0, 1.0, 0.0]), hand);
                let key = match self.mode {
                    Mode::Fetching(k) if hand == 1 => sight.loose.iter().find(|(i, _)| *i == k).map(|(_, p)| {
                        let l = local * (*p - at);
                        Key { t: 0.0, wrist: Vec3::new(l.x.clamp(-0.1, 0.5), l.y.clamp(0.15, 1.0) + 0.05, l.z.clamp(0.1, 0.6)), blade: Vec3::new(0.0, -0.2, 1.0), edge: Vec3::Y }
                    }),
                    _ => None,
                }
                .unwrap_or(rest);
                arm(&mut pose, hand, key);
                continue;
            };
            let key = match self.swing {
                Some(s) if s.hand == hand => {
                    attacking_hand = Some(hand);
                    MOVES[s.mv].at(s.progress(now), hand)
                }
                _ if shield || self.stance != Stance::SwordShield => {
                    // The hand that guards: towards the blow by the block weight; the other at rest.
                    let guarding = shield || shield_hand.is_none() && hand == self.guard_hand(hands);
                    let rest = mirror(if shield { SHIELD_GUARD } else { GUARD }, if shield { 1 } else { hand });
                    if guarding {
                        let b = self.block;
                        let g = if shield { self.guard } else { mirror(self.guard, if hand == 1 { 1 } else { 0 }) };
                        Key { t: 0.0, wrist: rest.wrist.lerp(g.wrist, b), blade: rest.blade.lerp(g.blade, b).normalize_or(rest.blade), edge: rest.edge.lerp(g.edge, b).normalize_or(rest.edge) }
                    } else {
                        rest
                    }
                }
                _ => mirror(GUARD, hand),
            };
            arm(&mut pose, hand, key);
        }
        let face_to = if look.length() > 0.1 { Some(look) } else { None };
        Thought { pose, walk, face_to, attacking: attacking_hand, guarding: self.block > 0.3 }
    }

    /// The hand that swings: dual wield alternates; otherwise the weapon hand.
    fn attack_hand(&mut self, hands: [Option<Hold>; 2]) -> usize {
        let weapons: Vec<usize> = (0..2).filter(|&h| hands[h].is_some_and(|x| !x.shield)).collect();
        if weapons.len() == 2 {
            self.next_hand ^= 1;
            return self.next_hand;
        }
        weapons.first().copied().unwrap_or(1)
    }

    fn guard_hand(&self, hands: [Option<Hold>; 2]) -> usize {
        (0..2).rev().find(|&h| hands[h].is_some()).unwrap_or(1)
    }
}

/// What it does this frame.
pub struct Thought {
    pub pose: Pose,
    /// How it walks (its own frame, m/s), and where it would face (world, flat).
    pub walk: Vec3,
    pub face_to: Option<Vec3>,
    /// Which hand swings, and whether it guards.
    pub attacking: Option<usize>,
    pub guarding: bool,
}

/// The arm of `hand` posed so its wrist is at `key.wrist` and the weapon in it lies as `key` says
/// (the weapon's +Z along the blade, +X along its edge, held with the hand's frame).
fn arm(pose: &mut Pose, hand: usize, key: Key) {
    let blade = key.blade.normalize_or(Vec3::Z);
    let edge = (key.edge - blade * key.edge.dot(blade)).normalize_or(blade.any_orthonormal_vector());
    let rot = Quat::from_mat3(&glam::Mat3::from_cols(edge, blade.cross(edge), blade)).normalize();
    pose.arm(hand, key.wrist, rot, 0.0);
}

/// The closest points between segments: (s, t) along each.
fn closest(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (f32, f32) {
    let d1 = p1 - p0;
    let d2 = q1 - q0;
    let r = p0 - q0;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    if a <= 1e-9 || e <= 1e-9 {
        return (0.0, 0.0);
    }
    let c = d1.dot(r);
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
