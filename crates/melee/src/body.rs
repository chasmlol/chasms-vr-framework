//! A human's body: its health, what each part multiplies damage by, how deep a point must go to
//! do a part its deep damage, which parts come off, how hits stagger and floor it, and what the
//! player takes.

use rustvr_protocol::*;

/// A human's health (50), and the player's (100).
pub const HEALTH: f32 = 50.0;
pub const PLAYER_HEALTH: f32 = 100.0;

/// Where on the body a hit lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Head,
    Neck,
    Torso,
    Arm,
    Leg,
    Hand,
    Foot,
}

impl Part {
    /// The part of `kind` (`PART_*`), the throat taken as the neck.
    pub fn of(kind: u32, throat: bool) -> Part {
        match kind {
            PART_HEAD if throat => Part::Neck,
            PART_HEAD => Part::Head,
            PART_ARM => Part::Arm,
            PART_HAND => Part::Hand,
            PART_LEG => Part::Leg,
            PART_FOOT => Part::Foot,
            _ => Part::Torso,
        }
    }

    /// The hit's damage multiplier.
    pub fn damage(self) -> f32 {
        match self {
            Part::Head => 3.0,
            Part::Neck | Part::Torso => 1.0,
            Part::Arm | Part::Leg => 0.5,
            Part::Hand | Part::Foot => 0.25,
        }
    }

    /// How deep a point must go for its deep damage (metres), and that damage's multiplier (a head
    /// or a neck x32, the torso x2).
    pub fn deep(self) -> (f32, f32) {
        match self {
            Part::Head => (0.04, 32.0),
            Part::Neck => (0.02, 32.0),
            Part::Torso => (0.1, 2.0),
            Part::Arm | Part::Leg => (0.1, 0.5),
            Part::Hand => (0.04, 0.25),
            Part::Foot => (0.1, 0.25),
        }
    }

    /// A cut's going in multiplies its penetration damage by this (its slash multiplier:
    /// the neck's 32 makes a cut throat fatal).
    pub fn cut(self) -> f32 {
        match self {
            Part::Head => 2.0,
            Part::Neck => 32.0,
            Part::Torso => 1.0,
            Part::Arm | Part::Leg => 0.5,
            Part::Hand | Part::Foot => 0.25,
        }
    }

    /// A point coming out the far side multiplies its skewer damage by this
    /// (by part).
    pub fn skewer(self) -> f32 {
        match self {
            Part::Head | Part::Neck => 4.0,
            Part::Torso => 3.0,
            Part::Arm | Part::Leg => 0.5,
            Part::Hand | Part::Foot => 0.25,
        }
    }

    /// It can be cut off (the head by its neck, the arms, the legs; never the torso).
    pub fn severable(self) -> bool {
        !matches!(self, Part::Torso)
    }

    /// The hit-reaction table's harsher rows are for these parts.
    pub fn head_or_legs(self) -> bool {
        matches!(self, Part::Head | Part::Neck | Part::Leg | Part::Foot)
    }
}

/// What a hit's push does to a creature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reaction {
    None,
    /// A stagger, as far as `recoil()` (0.2, 0.6, 1).
    Light,
    Medium,
    Full,
    /// Knocked off its feet: its ragdoll falls.
    Destabilize,
}

impl Reaction {
    /// How far it reels (light 0.2, medium 0.6, full 1).
    pub fn recoil(self) -> f32 {
        match self {
            Reaction::None => 0.0,
            Reaction::Light => 0.2,
            Reaction::Medium => 0.6,
            Reaction::Full | Reaction::Destabilize => 1.0,
        }
    }
}

/// What the creature was doing when the hit landed (the table's columns).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doing {
    /// No target: unaware, or a training dummy.
    Idle,
    Combat,
    /// Reeling from a light or medium stagger, or from a full one.
    Staggered,
    FullyStaggered,
    Attacking,
    AttackingAirborne,
}

/// The push table for the Medium and Hard humans (Hard's is Medium's);
/// `heavy`: the armoured ones' (`MediumHeavy`, `HardHeavy`), a step tougher.
pub fn reaction(level: u32, part: Part, doing: Doing, heavy: bool) -> Reaction {
    use Doing::*;
    use Reaction::*;
    let hl = part.head_or_legs();
    let hands = part == Part::Hand;
    let r = match (level.min(4), hl) {
        (0, _) => match doing {
            Idle => Light,
            Attacking if hands => Light,
            _ => None,
        },
        (1, false) => match doing {
            Idle | Staggered | FullyStaggered | Attacking => Medium,
            Combat => Light,
            AttackingAirborne => None,
        },
        (1, true) => match doing {
            Combat => Medium,
            AttackingAirborne => Destabilize,
            _ => Full,
        },
        (2, false) => match doing {
            Idle | FullyStaggered => Full,
            Combat => Light,
            Staggered | Attacking => Medium,
            AttackingAirborne => Destabilize,
        },
        (2, true) => match doing {
            Combat | Attacking => Full,
            _ => Destabilize,
        },
        (3, false) => match doing {
            Combat => Medium,
            AttackingAirborne => Destabilize,
            _ => Full,
        },
        (3, true) => match doing {
            Combat => Full,
            _ => Destabilize,
        },
        _ => Destabilize,
    };
    if !heavy {
        return r;
    }
    // The armoured: L1 and L2 while reeling only light; L2 on the body medium idle, light in an
    // attack; L2 on the head or legs full idle and reeling, medium fighting, down only airborne.
    match (level.min(4), hl, doing) {
        (1 | 2, false, Staggered) => Light,
        (2, false, Idle) => Medium,
        (2, false, Attacking) => Light,
        (2, true, Idle | Staggered | FullyStaggered) => Full,
        (2, true, Combat | Attacking) => Medium,
        _ => r,
    }
}

/// What a hit does to the player (5 at least, 15 at most: a missed parry never fells them at once),
/// times the weapon's tier's multiplier (1 to 2).
pub fn player_damage(damage: f32, tier: u32) -> f32 {
    let mult = [1.0, 1.0, 1.25, 1.5, 1.75, 2.0][(tier as usize).min(5)];
    (damage * mult).clamp(5.0, 15.0)
}

/// What a body slamming into the world does to itself (nothing at 4 m/s, 12.5 at 12, straight
/// between, and flat past its last key), before its part's multiplier: creatures take no fall
/// damage but this.
pub fn slam_damage(speed: f32) -> f32 {
    ((speed - 4.0) / 8.0 * 12.5).clamp(0.0, 12.5)
}

/// What a blow does to the player's part of kind `kind` (`PART_*`) before it's clamped: the
/// player's head and neck x1.5, hands and feet x0.5, the rest x1.
pub fn player_part(kind: u32) -> f32 {
    match kind {
        PART_HEAD => 1.5,
        PART_HAND | PART_FOOT => 0.5,
        _ => 1.0,
    }
}

/// The player's fall (nothing at 12 m/s down, 200 at 20).
pub fn fall_damage(speed_down: f32) -> f32 {
    ((speed_down - 12.0) * 25.0).max(0.0)
}

/// A knockout's length (s): the standard stun from a blow (6), times the material's
/// `Row::knockout_time`.
pub const KNOCKOUT_TIME: f32 = 6.0;
