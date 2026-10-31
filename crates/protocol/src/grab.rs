//! Grabbing the game's creatures: a hand takes hold of any part of one (an arm, a leg, the head,
//! the throat, the torso; a dead body by anything) and moves it, with the game's own physics.
//!
//! The runtime decides the hand's side: where it took hold, where the hand would have that point
//! now, how hard it can pull, how hard it squeezes a throat, and the throw when it lets go. It draws
//! the hand on the part where the game has it, so the hand is held back by the body's weight and
//! never passes through it. The game decides the body's side with its own physics and rules: how the
//! body follows (a limb pulled while the rest animates, a body knocked down and hanging, a dead body
//! dragged), what a choke does, and how the creature reacts.
//!
//! * [`ActorBodies`] (game -> runtime, every frame with [`crate::Actors`], in the same order): what
//!   grabbing needs beyond the capsules: each part's rotation, velocity and mass, the whole body's
//!   mass, whether it's all physics now, and how strong the player is.
//! * [`CreatureGrabs`] (runtime -> game, every frame): each hand's grab.
//!
//! All positions are world space, metres, the protocol's axes.

use bytemuck::{Pod, Zeroable};

use crate::{MAX_ACTOR_PARTS, MAX_ACTORS};

/// After the gore's regions ([`crate::gore`]), the region before it.
pub const OFF_ACTOR_BODIES: usize = crate::OFF_GORE_END.next_multiple_of(64);

/// One part's body: [`crate::Actor::parts`]' part of the same index.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct PartBody {
    /// The part's own rotation (any frame that turns with it: its bone's), so what's held turns
    /// with it. The part's frame is this rotation about its capsule's `a`. Zero: unknown (the
    /// runtime makes a frame from the capsule alone, which doesn't turn about it).
    pub rot: [f32; 4],
    /// How fast it moves (m/s).
    pub vel: [f32; 3],
    /// Its mass (kg; 0: unknown).
    pub mass: f32,
}

/// The hands may take hold of it.
pub const BODY_GRABBABLE: u32 = 1 << 0;
/// All of it is physics now (dead, knocked down, lifted off its feet): nothing of it stands.
pub const BODY_RAGDOLL: u32 = 1 << 1;
/// Alive and awake enough to struggle (not dead, not knocked out).
pub const BODY_CONSCIOUS: u32 = 1 << 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ActorBody {
    /// The creature's [`crate::Actor::id`].
    pub id: u32,
    pub flags: u32, // BODY_*
    /// The whole body's mass (kg).
    pub mass: f32,
    /// A hold on it the creature broke free of: that grab's
    /// [`CreatureGrab::serial`], and the hand lets go of it, with no throw. 0: none. (One a frame:
    /// a game freeing it of both hands names the other next frame.)
    pub let_go: u32,
    pub parts: [PartBody; MAX_ACTOR_PARTS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ActorBodies {
    /// Seqlock word.
    pub seq: u32,
    pub count: u32,
    /// The game frame they're from.
    pub frame: u64,
    /// How strong the player is, by the game's own measure: 1 for an average person (the hand's
    /// pull is the runtime's strength setting times this).
    pub strength: f32,
    pub _pad: [u32; 11],
    pub bodies: [ActorBody; MAX_ACTORS],
}

pub const OFF_CREATURE_GRABS: usize = (OFF_ACTOR_BODIES + size_of::<ActorBodies>()).next_multiple_of(64);

/// Held now. Without it the grab has been let go of: its last word, with the throw (`vel`), kept
/// until the hand takes hold of something else.
pub const GRAB_HELD: u32 = 1 << 0;
/// On the throat: `squeeze` chokes.
pub const GRAB_THROAT: u32 = 1 << 1;
/// The other hand holds the same creature.
pub const GRAB_TWO_HANDS: u32 = 1 << 2;
/// The grip slipped (the hand was pulled past its reach): let go without a throw.
pub const GRAB_SLIPPED: u32 = 1 << 3;
/// Held by telekinesis, from afar: the game holds the point on the telekinesis drive for bodies
/// (2000 N/m, 100 N s/m, at most 2500 N; the neck's turn x5), not the hand's; let go of with a
/// throw, the part held gets 30 m/s of it and the rest of the body 8. Two hands holding one
/// creature this way (with [`GRAB_TWO_HANDS`]): its limbs that can tear off do past 3000 N on
/// their joints (over the time scale).
pub const GRAB_TK: u32 = 1 << 4;

/// One hand's grab of a creature's part.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct CreatureGrab {
    /// A new number for each hold a hand takes (0: this hand hasn't held a creature yet).
    pub serial: u32,
    pub flags: u32, // GRAB_*
    /// The creature's [`crate::Actor::id`], and the part held (its index in its parts).
    pub actor: u32,
    pub part: u32,
    /// The point held: where the palm is on the part, in the part's frame (see [`PartBody::rot`]).
    pub point: [f32; 3],
    /// The most the hand pulls with (N).
    pub force: f32,
    /// Where the hand would have that point now (where the controller carries it).
    pub target: [f32; 3],
    /// How hard the hand squeezes past its hold (0..1): on the throat, how hard it chokes.
    pub squeeze: f32,
    /// How the hand would have the part turned now (its frame's rotation).
    pub target_rot: [f32; 4],
    /// The hand's velocity (m/s); let go, the throw's: the hand's speed at its peak before the
    /// release.
    pub vel: [f32; 3],
    /// Where the point held is now, from where the hand would have it (metres): how far the body
    /// holds the hand back.
    pub stretch: f32,
    /// The hand's spin (rad/s).
    pub ang_vel: [f32; 3],
    pub _p0: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CreatureGrabs {
    /// Seqlock word.
    pub seq: u32,
    pub _pad: [u32; 15],
    /// The left hand's, the right hand's.
    pub grabs: [CreatureGrab; 2],
}

pub const OFF_GRAB_END: usize = OFF_CREATURE_GRABS + size_of::<CreatureGrabs>();

const _: () = {
    assert!(size_of::<PartBody>() == 32);
    assert!(size_of::<ActorBody>() == 16 + 32 * MAX_ACTOR_PARTS);
    assert!(size_of::<CreatureGrab>() == 96);
    assert!(OFF_ACTOR_BODIES >= crate::OFF_GORE_END);
    assert!(OFF_GRAB_END <= crate::OFF_HUD_LAYOUT);
};

impl ActorBodies {
    /// The body of creature `id`, if the game sent one.
    pub fn get(&self, id: u32) -> Option<&ActorBody> {
        self.bodies[..(self.count as usize).min(MAX_ACTORS)].iter().find(|b| b.id == id)
    }
}
