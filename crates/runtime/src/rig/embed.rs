//! A blade in something, as the melee's penetration has it:
//! a held axe that chops in edge first, a point that stabs in along the blade, or an edge that bites
//! (a cut goes in 3 mm and slides along the edge), hard enough and into something it bites, stays
//! in. The combat finds where ([`Request`]); the rig holds it there ([`Embedded`]): it rides with what
//! it's in (the world, or a creature's body part), and the hand's pull moves it along the way it went
//! in against its dampers (no spring: a damper going in, another coming out, a third with no hand
//! on it), and a cut along its edge against a light one. Pulled back out past where it went in, or a
//! cut slid off the end of its blade, it's out; across its way it can't move, so pulling it across
//! drags what it's in. Let go, it stays in, to be taken again.

use glam::{Quat, Vec3};

/// Stuck in this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Into {
    World,
    /// One of the rig's loose things ([`rustvr_protocol::ObjectDesc::id`]): it's dragged along.
    Object { id: u32 },
    /// A creature's body part ([`rustvr_protocol::Actor`] id, part index).
    Actor { id: u32, part: usize },
}

/// What a blade going in does on its way, for the game.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wounding {
    /// Which damager, its kind (`DMG_*`) and the weapon's tier.
    pub blade: u32,
    pub kind: u32,
    /// What its going in does once it's past `deep` metres (a cut's at once), and what its point
    /// coming out the far side does past `through` (0: nothing).
    pub damage: f32,
    pub deep: f32,
    pub skewer: f32,
    pub through: f32,
}

/// The combat's: object `object` (held) went into `into`.
#[derive(Clone, Copy, Debug)]
pub struct Request {
    pub object: u32,
    pub into: Into,
    /// Where it met the surface (world).
    pub point: Vec3,
    /// The way it went in (world, unit): an axe's edge, a point's blade.
    pub dir: Vec3,
    /// The point of the object (its own space) that met the surface.
    pub sample: Vec3,
    /// How far it went in, and how far it can.
    pub depth: f32,
    pub max_depth: f32,
    /// The dampers it moves along its way against (N s/m): a hand's going in, a hand's coming out,
    /// and with no hand on it (the blade's, times the material's multiplier).
    pub dampers: [f32; 3],
    /// A cut slides along its edge (world, unit), this far either way before it comes off the end.
    pub slide: Option<(Vec3, f32)>,
    pub wounding: Wounding,
}

/// An object stuck in something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Embedded {
    pub into: Into,
    /// Where it went in, in what it's in's frame.
    pub at: Vec3,
    /// The object as it was at the surface (no way in), in what it's in's frame.
    pub rel: (Vec3, Quat),
    /// The way in, in that frame.
    pub axis: Vec3,
    pub depth: f32,
    pub max_depth: f32,
    pub dampers: [f32; 3],
    /// A cut's way along its edge in that frame (zero: it doesn't slide), how far it has slid and
    /// how far it may.
    pub slide_axis: Vec3,
    pub slid: f32,
    pub slide_max: f32,
    pub wounding: Wounding,
    /// The deepest it has been, and whether its going in and its coming out the far side have been
    /// told.
    pub deepest: f32,
    pub wounded: bool,
    pub skewered: bool,
}

/// Pulled back this far out past the surface (metres): it's out.
pub const EXIT: f32 = 0.012;
/// The hand this far across the way it went in (metres): yanked out. (Short of that, a creature
/// it's in is dragged along: the blade is jointed to the body, not torn out.)
pub const TEAR: f32 = 0.5;
/// What it's in is dragged at this many m/s per metre the hand pulls it off its way in.
pub const DRAG_RATE: f32 = 12.0;
/// A cut slides along its edge against this (N s/m).
pub const SLIDE_DAMPER: f32 = 100.0;

/// How fast (m/s) an axe's edge must chop, and a point stab, to stay in, when the game gives no
/// damage tables (the material minimums: a pierce 1 m/s into anything, an axe's slash 3).
pub const AXE_SPEED: f32 = 3.0;
pub const STAB_SPEED: f32 = 1.0;
/// How deep an axe bites, and how deep it can go; a point's default reach in.
pub const AXE_DEPTH: f32 = 0.03;
pub const AXE_MAX_DEPTH: f32 = 0.05;
pub const STAB_MAX_DEPTH: f32 = 0.12;
/// A sword's stab dampers when the game gives none (held in 1000, out 500, free 1250).
pub const DAMPERS: [f32; 3] = [1000.0, 500.0, 1250.0];

/// A body part's frame: at `a`, its +Y along to `b`.
pub fn part_frame(a: Vec3, b: Vec3) -> (Vec3, Quat) {
    (a, Quat::from_rotation_arc(Vec3::Y, (b - a).normalize_or(Vec3::Y)))
}

/// One step of a held object in something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Still in: how deep, and how far along its cut.
    Stay(f32, f32),
    Out,
}

/// One step: the hand pulls it `along` metres further in (negative: out) than it is, `slide` metres
/// along its edge, `across` metres off its way in, with the hand's joint (`spring` N/m, at most
/// `max_force` N). The penetration joint has no spring of its own, only dampers, so it moves at
/// the hand's pull over the damper (going in, or coming out), as far as it can go.
#[allow(clippy::too_many_arguments)]
pub fn step(e: &Embedded, along: f32, slide: f32, across: f32, spring: f32, max_force: f32, held: bool, dt: f32) -> Step {
    if across > TEAR {
        return Step::Out;
    }
    let damper = |going_in: bool| (if !held { e.dampers[2] } else if going_in { e.dampers[0] } else { e.dampers[1] }).max(1.0);
    let pull = (along * spring).clamp(-max_force, max_force);
    // (No faster than closing what's between the hand and it in one step.)
    let moved = (pull / damper(pull > 0.0) * dt).clamp(-along.abs(), along.abs());
    let d = (e.depth + moved).min(e.max_depth);
    if d < -EXIT {
        return Step::Out;
    }
    let mut slid = e.slid;
    if e.slide_axis != Vec3::ZERO {
        let pull = (slide * spring).clamp(-max_force, max_force);
        slid += (pull / SLIDE_DAMPER * dt).clamp(-slide.abs(), slide.abs());
        if slid.abs() > e.slide_max {
            return Step::Out;
        }
    }
    Step::Stay(d, slid)
}
