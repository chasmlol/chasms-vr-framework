//! A blade in something (penetration): a held axe that chops in edge first, a point that stabs in
//! along the blade, or an edge that bites (a cut goes in 3 mm and slides along the edge), hard
//! enough and into something it bites, stays in. The combat finds where ([`Request`]); the rig
//! holds it there ([`Embedded`]): it rides with what it's in (the world, or a creature's body part,
//! turning as the part turns), and the hand's pull moves it along the way it went in against the
//! dampers (no spring: a damper going in, another coming out, a third with no hand on it), and a
//! cut along its edge against a light one. While it's shallow it rocks on where it went in, up to
//! 30 degrees, stiffening to rigid as it goes deeper; it can't turn about its way in, so twisting
//! it turns the part. Pulled back out past where it went in, or a cut slid off the end of its
//! blade, it's out; across its way it can't move, so pulling it across drags what it's in (it's
//! never torn out sideways: an arm's length off, the hand lets go of it instead). Let go, it stays
//! in, to be taken again.

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
    /// A cut slides along its edge (world, unit) from where it met the surface along the blade
    /// (metres from the edge's middle), this far either way of its middle before it comes off the
    /// end.
    pub slide: Option<(Vec3, f32, f32)>,
    pub wounding: Wounding,
    /// How it rocks (see [`Embedded::rock`]: metres, radians).
    pub rock: (f32, f32),
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
    /// A cut's way along its edge in that frame (zero: it doesn't slide), how far it has slid, where
    /// along its edge it went in (metres from the edge's middle), and how far either way of the
    /// middle it may be before it's off the end.
    pub slide_axis: Vec3,
    pub slid: f32,
    pub slide_from: f32,
    pub slide_max: f32,
    pub wounding: Wounding,
    /// The deepest it has been, and whether its going in and its coming out the far side have been
    /// told.
    pub deepest: f32,
    pub wounded: bool,
    pub skewered: bool,
    /// How it rocks: rigid from this deep (metres, its short depth), and up to this far (radians) at
    /// the surface; and how far it's rocked now, about where it went in (in what it's in's frame).
    pub rock: (f32, f32),
    pub tilt: Quat,
}

impl Embedded {
    /// The most it may rock at its depth now (radians: the angular limit, its short depth's angle
    /// eased to none at the short depth).
    pub fn rock_limit(&self) -> f32 {
        if self.rock.0 <= 0.0 {
            return 0.0;
        }
        self.rock.1 * (1.0 - self.depth / self.rock.0).clamp(0.0, 1.0)
    }
}

/// Pulled back out past the surface, it's out (at a depth of 0, falling).
pub const EXIT: f32 = 0.0;
/// A cut slides along its edge against this (N s/m).
pub const SLIDE_DAMPER: f32 = 100.0;
/// The rocking where the blade gives none: a sword's 0.1 m and 30 degrees.
pub const ROCK: (f32, f32) = (0.1, 30.0);
/// What a stuck blade's weight pulls it with, a kilogram (N, world: the physics' gravity).
pub const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);
/// A creature's part the game gives no mass for (kg: a 65 kg human over its 11 parts).
pub const PART_MASS: f32 = 6.0;

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
/// along its edge, with the hand's joint (`spring` N/m, at most `max_force` N); `held`: a hand is on
/// it (else `along` is what its own weight would carry it, over the step). The penetration joint has
/// no spring of its own, only dampers, so it moves at the pull over the damper (a hand's going in, or
/// coming out; with none, its own), as far as it can go.
#[allow(clippy::too_many_arguments)]
pub fn step(e: &Embedded, along: f32, slide: f32, spring: f32, max_force: f32, held: bool, dt: f32) -> Step {
    let damper = |going_in: bool| (if !held { e.dampers[2] } else if going_in { e.dampers[0] } else { e.dampers[1] }).max(1.0);
    let pull = (along * spring).clamp(-max_force, max_force);
    // (No faster than closing what's between the hand and it in one step.)
    let moved = (pull / damper(pull > 0.0) * dt).clamp(-along.abs(), along.abs());
    let d = (e.depth + moved).min(e.max_depth);
    if d < -EXIT && moved < 0.0 {
        return Step::Out;
    }
    let mut slid = e.slid;
    if e.slide_axis != Vec3::ZERO {
        let pull = (slide * spring).clamp(-max_force, max_force);
        slid += (pull / SLIDE_DAMPER * dt).clamp(-slide.abs(), slide.abs());
        // (Off the end of its edge: its anchor past half its penetration length.)
        if (e.slide_from + slid).abs() > e.slide_max {
            return Step::Out;
        }
    }
    Step::Stay(d, slid)
}

/// `q` split into its swing (turning `axis` itself) and its twist (about `axis`): `q = swing * twist`.
pub fn swing_twist(q: Quat, axis: Vec3) -> (Quat, Quat) {
    let along = axis * Vec3::new(q.x, q.y, q.z).dot(axis);
    let twist = Quat::from_xyzw(along.x, along.y, along.z, q.w);
    let twist = if twist.length_squared() > 1e-12 { twist.normalize() } else { Quat::IDENTITY };
    ((q * twist.inverse()).normalize(), twist)
}

/// `q` turned no further than `max` radians (about its own axis).
pub fn clamp_turn(q: Quat, max: f32) -> Quat {
    let (axis, angle) = q.to_axis_angle();
    let angle = if angle > std::f32::consts::PI { angle - std::f32::consts::TAU } else { angle };
    if angle.abs() <= max {
        return q;
    }
    Quat::from_axis_angle(axis, max.copysign(angle))
}

/// How far a line from `from` along `dir` (unit) runs inside the capsule `a`..`b` of `radius` before
/// it's out of it (the way a point goes in to where it comes out the far side), at most `most`.
pub fn chord(a: Vec3, b: Vec3, radius: f32, from: Vec3, dir: Vec3, most: f32) -> f32 {
    let inside = |p: Vec3| {
        let ab = b - a;
        let t = if ab.length_squared() > 1e-9 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        p.distance(a + ab * t) < radius
    };
    // In from its surface (where the point met it), then on until it's out.
    let step = (radius * 0.05).max(1e-4);
    let mut d = 0.0;
    let mut was_in = false;
    while d < most {
        let p = from + dir * d;
        if inside(p) {
            was_in = true;
        } else if was_in {
            return d;
        }
        d += step;
    }
    most
}
