//! A blade in something, as the melee's penetration has it:
//! a held axe that chops in edge first, or a point that stabs in along the blade, hard enough and
//! into something it bites (wood, earth, flesh: [`rustvr_protocol::biteable`]), stays in. The
//! combat finds where ([`Request`]); the rig holds it there ([`Embedded`]): it rides with what it's
//! in (the world, or a creature's body part), slides along the way it went in as the hand pushes
//! and pulls ([`step`]), and comes out when it's pulled back out past where it went in, or yanked
//! hard across. Let go, it stays in, to be taken again.

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
}

/// Pulled back this far out past the surface (metres): it's out.
pub const EXIT: f32 = 0.012;
/// The hand this far across the way it went in (metres): yanked out. (Short of that, a creature
/// it's in is dragged along: the blade is jointed to the body, not torn out.)
pub const TEAR: f32 = 0.5;
/// What it's in is dragged at this many m/s per metre the hand pulls it off its way in.
pub const DRAG_RATE: f32 = 12.0;
/// It slides along the way in at most this fast (m/s): the hold of what it's in.
pub const SLIDE_SPEED: f32 = 0.9;

/// How fast (m/s) an axe's edge must chop, and a point stab, to stay in.
/// (The materials' minimums: a pierce 1 m/s into anything, an axe's slash 3.)
pub const AXE_SPEED: f32 = 3.0;
pub const STAB_SPEED: f32 = 1.0;
/// How deep an axe bites, and how deep it can go; a point's default reach in.
pub const AXE_DEPTH: f32 = 0.03;
pub const AXE_MAX_DEPTH: f32 = 0.05;
pub const STAB_MAX_DEPTH: f32 = 0.12;

/// A body part's frame: at `a`, its +Y along to `b`.
pub fn part_frame(a: Vec3, b: Vec3) -> (Vec3, Quat) {
    (a, Quat::from_rotation_arc(Vec3::Y, (b - a).normalize_or(Vec3::Y)))
}

/// One step of a held object in something: the hand is `along` metres further in (negative: out)
/// than the object's hold, and `across` metres off the way in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Stay(f32),
    Out,
}

pub fn step(depth: f32, max_depth: f32, along: f32, across: f32, dt: f32) -> Step {
    if across > TEAR {
        return Step::Out;
    }
    let slide = along.clamp(-SLIDE_SPEED * dt, SLIDE_SPEED * dt);
    let d = (depth + slide).min(max_depth);
    if d < -EXIT { Step::Out } else { Step::Stay(d) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_slides_and_comes_out_backwards() {
        // Pushed in: deeper, at most its depth.
        assert_eq!(step(0.03, 0.05, 0.5, 0.0, 1.0), Step::Stay(0.05));
        // Pulled a little: slides out, at its pace.
        match step(0.03, 0.05, -0.2, 0.0, 0.01) {
            Step::Stay(d) => assert!((d - (0.03 - SLIDE_SPEED * 0.01)).abs() < 1e-6),
            s => panic!("{s:?}"),
        }
        // Pulled back past the surface: out.
        assert_eq!(step(-0.01, 0.05, -0.2, 0.0, 0.01), Step::Out);
        // Yanked across: out.
        assert_eq!(step(0.03, 0.05, 0.0, 0.6, 0.01), Step::Out);
        // Pulled across short of that, it stays in (what it's in is dragged instead).
        assert_eq!(step(0.03, 0.05, 0.0, 0.3, 0.01), Step::Stay(0.03));
    }

    #[test]
    fn a_part_frame_points_along_it() {
        let (p, q) = part_frame(Vec3::ZERO, Vec3::X * 2.0);
        assert_eq!(p, Vec3::ZERO);
        assert!((q * Vec3::Y - Vec3::X).length() < 1e-5);
    }
}
