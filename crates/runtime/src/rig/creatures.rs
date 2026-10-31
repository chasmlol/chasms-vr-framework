//! Hands on the game's creatures: a hand whose grip closes on any part of one (an arm, a leg, the
//! head, the throat, the torso; a dead body by anything) takes hold of it where it touched it, and
//! moves it with the game's own physics ([`rustvr_protocol::grab`]).
//!
//! The runtime has the hand's side, as Skyrim's HIGGS does (see
//! `docs/design/npcs.md` in the Oblivion adapter's repository):
//!
//! * The hand is drawn on the part where it took hold, wherever the game has the part, so the
//!   body's weight holds it back (a light arm follows the hand closely, a body hangs from it and
//!   lags, a heavy creature barely moves) and it never goes into the body. The ghost hand shows the
//!   controller while they're apart.
//! * The game is told where the hand would have the point it holds (the controller carrying the
//!   hold), how hard the hand can pull (the strength setting times the game's own strength), how
//!   hard it squeezes a throat, and when it lets go, the throw: the hand's speed at its peak.
//! * Past the arm's reach from the controller the grip slips, rather than the hand stretching.
//! * Let go, the hand is a free hand again where it is, on the body's surface: held back by the
//!   body if the controller is in it, catching up eased, never through it.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

/// A hand pulled this far from its controller by what it holds (metres) loses its grip, after
/// [`SLIP_TIME`] seconds of it: an arm's reach (HIGGS lets go at 0.7 m).
pub const REACH: f32 = 0.6;
pub const SLIP_TIME: f64 = 0.15;
/// The throat: the lower part of the head's capsule (from the neck, this share of it), or the top
/// of the torso's (this far from its end at the neck, metres).
const THROAT_SHARE: f32 = 0.45;
const THROAT_TORSO: f32 = 0.1;
/// A capsule thinner than this (metres) is held in the fist (an arm, a neck); thicker, with the
/// fingers spread over it (a torso, a head).
const THIN: f32 = 0.07;
/// A throat is this thick (metres): the fingers close round the neck, not the head's capsule
/// (which runs on up through the crown, as wide as the skull).
pub const THROAT_RADIUS: f32 = 0.055;
/// Capsules of the creature this near the hand (metres) are what its fingers close on.
pub const GRASP_NEAR: f32 = 0.12;
/// A hand held back further than this from its controller (metres) feels the body's weight: a
/// strain every so often (seconds), full at [`STRAIN_FULL`] metres.
pub const STRAIN_FROM: f32 = 0.04;
pub const STRAIN_EVERY: f64 = 0.05;
pub const STRAIN_FULL: f32 = 0.3;

/// What the game says about one creature's body ([`ActorBody`]), and its parts' kinds.
#[derive(Clone, Debug, Default)]
pub struct Body {
    pub flags: u32,
    pub mass: f32,
    pub parts: Vec<Part>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Part {
    pub kind: u32,
    pub rot: Option<Quat>,
    pub mass: f32,
    pub vel: Vec3,
}

impl Body {
    pub fn from(actor: &Actor, body: Option<&ActorBody>) -> Body {
        let n = (actor.count as usize).min(MAX_ACTOR_PARTS);
        let parts = (0..n)
            .map(|i| {
                let b = body.map(|b| b.parts[i]).unwrap_or_default();
                let q = Quat::from_array(b.rot);
                Part { kind: actor.parts[i].kind, rot: (q.length_squared() > 0.5 && q.is_finite()).then(|| q.normalize()), mass: b.mass, vel: Vec3::from_array(b.vel) }
            })
            .collect();
        Body { flags: body.map_or(0, |b| b.flags), mass: body.map_or(0.0, |b| b.mass), parts }
    }

    pub fn grabbable(&self) -> bool {
        self.flags & BODY_GRABBABLE != 0
    }
}

/// A part's frame: its capsule's `a`, turned as its bone is (or, unknown, along the capsule).
pub fn frame(a: Vec3, b: Vec3, rot: Option<Quat>) -> (Vec3, Quat) {
    match rot {
        Some(q) => (a, q),
        None => crate::rig::embed::part_frame(a, b),
    }
}

/// The point of a capsule's surface nearest `p` (world), and its normal there.
pub fn surface(a: Vec3, b: Vec3, radius: f32, p: Vec3) -> (Vec3, Vec3) {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-9 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    let axis = a + ab * t;
    let n = (p - axis).try_normalize().unwrap_or_else(|| ab.normalize_or(Vec3::Y).any_orthonormal_vector());
    (axis + n * radius, n)
}

/// Whether a hold at `p` (world) on a part of `kind` from `a` to `b` is on the throat.
/// The neck of a part of `kind` from `a` to `b`, if it has one: the stretch of it a throat hold is
/// on.
pub fn neck(kind: u32, a: Vec3, b: Vec3) -> Option<(Vec3, Vec3)> {
    match kind {
        PART_HEAD => Some((a, a.lerp(b, THROAT_SHARE))),
        PART_TORSO => Some((b - (b - a).normalize_or_zero() * THROAT_TORSO, b)),
        _ => None,
    }
}

pub fn on_throat(kind: u32, a: Vec3, b: Vec3, p: Vec3) -> bool {
    let ab = b - a;
    let len = ab.length();
    if len < 1e-4 {
        return false;
    }
    let along = (p - a).dot(ab) / len;
    match kind {
        // The head's capsule runs from the neck up through the crown.
        PART_HEAD => along < len * THROAT_SHARE,
        // The torso's from the pelvis up to the neck.
        PART_TORSO => along > len - THROAT_TORSO,
        _ => false,
    }
}

/// The fingers' shape round a capsule this thick.
pub fn hold_shape(radius: f32) -> rustvr_ik::HoldShape {
    if radius < THIN { rustvr_ik::HoldShape::Foregrip } else { rustvr_ik::HoldShape::Large }
}

/// A hand's hold on a creature's part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold {
    pub actor: u32,
    pub part: usize,
    /// The hand's grip pose in the part's frame, as it was when it took hold.
    pub hand: (Vec3, Quat),
    /// The point held: where the palm is on the part's surface, in its frame.
    pub point: Vec3,
    pub throat: bool,
    pub serial: u32,
    pub since: f64,
    /// Pulled past its reach since then.
    pub slipping: Option<f64>,
    /// When the strain was last felt.
    pub strained: f64,
}

/// How hard a hand squeezing `squeeze` (0..1) grips past its hold, with the grip closing at `press`.
pub fn squeeze_past(squeeze: f32, press: f32) -> f32 {
    ((squeeze - press) / (1.0 - press).max(1e-3)).clamp(0.0, 1.0)
}

