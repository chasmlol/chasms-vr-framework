//! Riding: a mount the player rides (a horse, or whatever creature the game lets a player get on),
//! and the mounts near the player they could get on.
//!
//! The game stays the source of truth for the mount: its model and animations, its speeds and how
//! fast it turns, its rules on whose mount it is (getting on someone else's can be a crime),
//! getting on and off, its sounds. The runtime is the rider: their hands on the reins, their body
//! in the saddle, what they see.
//!
//! * [`Ride`] (game -> runtime, every game frame): the mounts near the player, with where each one's
//!   saddle is (a hand takes hold of it to get on), and the mount the player rides: where the game
//!   seats the rider (the hips, the knees and the feet, as its own riding animation has them, so
//!   they move with the gait), where the reins are, the gait it's going at and how many it has, and
//!   whether the rider is getting on or off.
//! * [`RtRide`] (runtime -> game, every runtime frame): what the rider does with it: the gait asked
//!   for and how much of its pace, the steering, getting on (which mount) and getting off. The
//!   adapter gives these to the game as its own controls (its mount's own acceleration, turning and
//!   speeds then do the rest).
//!
//! Positions are in tracking space, mapped by the adapter with the mapping it renders that game
//! frame with (as [`crate::OS_TRACKING`] objects are): the saddle stays under the rider however the
//! mount moves, because the adapter carries the view with the mount (its turns as well as its
//! travel: the adapter SDK's `TrackingMapper::carry_turn`).

use bytemuck::{Pod, Zeroable};

use crate::Pose;

/// After the magic's regions ([`crate::magic`]), the region before it.
pub const OFF_RIDE: usize = crate::OFF_MAGIC_END.next_multiple_of(64);
pub const OFF_RT_RIDE: usize = (OFF_RIDE + size_of::<Ride>()).next_multiple_of(64);
pub const OFF_RIDE_END: usize = OFF_RT_RIDE + size_of::<RtRide>();
pub const MAX_MOUNTS: usize = 8;
/// The most cross-sections of a mount's neck and head ([`Riding::neck`]).
pub const NECK_SECTIONS: usize = 10;

/// The mount is the player's own.
pub const MOUNT_OWNED: u32 = 1 << 0;
/// The mount is someone else's: getting on it is a crime (the game asks first).
pub const MOUNT_CRIME: u32 = 1 << 1;

/// A mount near the player that they could get on.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Mount {
    /// The game's id for it, passed back in [`RtRide::mount`].
    pub id: u32,
    pub flags: u32, // MOUNT_*
    pub _pad: [u32; 2],
    /// Its saddle (tracking space): from its front (the pommel) to its back, and how far round
    /// that line a hand takes hold of it.
    pub saddle_a: [f32; 3],
    pub radius: f32,
    pub saddle_b: [f32; 3],
    pub _p0: f32,
}

/// Seated and riding: the rider has the reins.
pub const RIDE_MOUNTED: u32 = 1 << 0;
/// Getting on: the game's own animation is putting the rider in the saddle.
pub const RIDE_GETTING_ON: u32 = 1 << 1;
/// Getting off: the game's own animation is taking the rider down.
pub const RIDE_GETTING_OFF: u32 = 1 << 2;
/// The knees, ankles and toes are the game's riding pose's.
pub const RIDE_LEGS: u32 = 1 << 3;
/// The reins are where the game's rider holds them.
pub const RIDE_REINS: u32 = 1 << 4;
/// The bits are given: the runtime draws the reins from them to the hands (a game whose mount has
/// reins of its own leaves this off).
pub const RIDE_BITS: u32 = 1 << 5;
/// The mount's neck and head are given ([`Riding::neck`]): the reins lie on them.
pub const RIDE_NECK: u32 = 1 << 6;
/// The reins' leather is given ([`Riding::reins_leather`]): they're drawn in the game's own.
pub const RIDE_LEATHER: u32 = 1 << 7;

/// A cross-section of a mount's neck or head (tracking space): an ellipse round the top of it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct NeckSection {
    /// Its middle (xyz).
    pub centre: [f32; 4],
    /// Across it to its edge, and up from its middle to its top (xyz: each as long as the ellipse's
    /// half width and half height).
    pub side: [f32; 4],
    pub up: [f32; 4],
}

/// The mount the player rides, as the game has it (tracking space).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Riding {
    /// The mount's [`Mount::id`].
    pub id: u32,
    pub flags: u32, // RIDE_*
    /// The gait the mount is going at: -1 backing, 0 standing, then each of its gaits forward from
    /// the slowest (`gaits` of them: a horse in Oblivion walks and gallops).
    pub gait: i32,
    pub gaits: u32,
    /// How fast it goes (metres a second) and turns (radians a second, positive to the right).
    pub speed: f32,
    pub turn: f32,
    pub _pad: [f32; 2],
    /// Where the rider sits: the saddle's seat (the lowest of its top), turned as the mount is (+Z
    /// its forward, +Y its up). It moves with the mount's gait as the game animates it.
    pub seat: Pose,
    /// Where the rider's knees, ankles and the balls of their feet go [left, right] (xyz;
    /// `RIDE_LEGS`): astride the mount, round its body.
    pub knees: [[f32; 4]; 2],
    pub ankles: [[f32; 4]; 2],
    pub toes: [[f32; 4]; 2],
    /// Where the reins are held [left, right]: the game's rider's hands (xyz; `RIDE_REINS`).
    pub reins: [[f32; 4]; 2],
    /// The front of the saddle (its pommel, or horn) a hand can hold (xyz).
    pub pommel: [f32; 4],
    /// Where the reins run from [left, right]: the rings of the bit in the mount's mouth (xyz;
    /// `RIDE_BITS`).
    pub bits: [[f32; 4]; 2],
    /// The reins' colour (linear RGB; `RIDE_BITS`): the game's own leather's, for when they can't
    /// be drawn in it.
    pub reins_color: [f32; 4],
    /// The reins' leather (`RIDE_LEATHER`): the model (`<game>/<asset key>` in the runtime's asset
    /// cache, NUL-padded) whose first mesh's material they're made of (its texture, its normal map,
    /// how it shines), and the stretch of that texture they're cut from (u0, v0, u1, v1: a strap
    /// running along v).
    pub reins_leather: [u8; 32],
    pub reins_uv: [f32; 4],
    /// The mount's neck and head from its withers to its muzzle, in order (`RIDE_NECK`): the reins
    /// lie on them and never pass through them. `neck[..neck_count]`.
    pub neck_count: u32,
    pub _pad2: [u32; 3],
    pub neck: [NeckSection; NECK_SECTIONS],
}

impl Riding {
    pub fn mounted(&self) -> bool {
        self.flags & RIDE_MOUNTED != 0
    }

    /// Getting on or off: the game's animation has the rider.
    pub fn moving_on_or_off(&self) -> bool {
        self.flags & (RIDE_GETTING_ON | RIDE_GETTING_OFF) != 0
    }

    /// The neck's cross-sections given (`RIDE_NECK`).
    pub fn neck(&self) -> &[NeckSection] {
        if self.flags & RIDE_NECK == 0 { &[] } else { &self.neck[..(self.neck_count as usize).min(NECK_SECTIONS)] }
    }

    /// The reins' leather's model (`RIDE_LEATHER`), if given.
    pub fn reins_leather(&self) -> Option<&str> {
        let n = self.reins_leather.iter().position(|&b| b == 0).unwrap_or(self.reins_leather.len());
        std::str::from_utf8(&self.reins_leather[..n]).ok().filter(|s| self.flags & RIDE_LEATHER != 0 && !s.is_empty())
    }

    /// Names the reins' leather's model (cut to fit) and sets `RIDE_LEATHER`.
    pub fn set_reins_leather(&mut self, name: &str) {
        self.reins_leather = [0; 32];
        let n = name.len().min(31);
        self.reins_leather[..n].copy_from_slice(&name.as_bytes()[..n]);
        self.flags |= RIDE_LEATHER;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Ride {
    /// Seqlock word.
    pub seq: u32,
    /// Mounts in use are `mounts[..count]`.
    pub count: u32,
    /// The game frame it's from.
    pub frame: u64,
    pub _pad: [u32; 12],
    /// The mount the player rides (`riding.flags` 0: none).
    pub riding: Riding,
    pub mounts: [Mount; MAX_MOUNTS],
}

impl Default for Ride {
    fn default() -> Self {
        Zeroable::zeroed()
    }
}

impl Ride {
    pub fn mounts(&self) -> &[Mount] {
        &self.mounts[..(self.count as usize).min(MAX_MOUNTS)]
    }

    /// Adds a mount (false: no room).
    pub fn push_mount(&mut self, m: Mount) -> bool {
        let n = self.count as usize;
        if n >= MAX_MOUNTS {
            return false;
        }
        self.mounts[n] = m;
        self.count += 1;
        true
    }
}

/// The runtime is riding: `gait`, `pace` and `steer` are the rider's.
pub const RR_RIDING: u32 = 1 << 0;
/// A hand holds the left rein, the right rein.
pub const RR_REIN_LEFT: u32 = 1 << 1;
pub const RR_REIN_RIGHT: u32 = 1 << 2;
/// The stick rides (rather than the reins).
pub const RR_STICK: u32 = 1 << 3;
/// Going forward at [`RtRide::speed`] (rather than a gait at a pace): the adapter gives the game
/// whichever of its gaits, at whatever pace, makes that speed.
pub const RR_SPEED: u32 = 1 << 4;

/// What the rider does with the mount (runtime -> game).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct RtRide {
    /// Seqlock word.
    pub seq: u32,
    pub flags: u32, // RR_*
    /// The runtime frame it's from.
    pub frame_index: u64,
    /// The gait asked for (as [`Riding::gait`]) and how much of that gait's pace (0..1: the
    /// stick's push; the reins ask for all of it).
    pub gait: i32,
    pub pace: f32,
    /// Steering: -1 hard left .. 1 hard right, as hard as the game's mount turns.
    pub steer: f32,
    /// Get on the mount `mount` ([`Mount::id`]): `mount_seq` goes up by one each time.
    pub mount: u32,
    pub mount_seq: u32,
    /// Get off: goes up by one each time.
    pub dismount_seq: u32,
    /// Going forward (`RR_SPEED`, `gait` 1): how fast, as a share of the mount's fastest gait's
    /// full speed (0..1).
    pub speed: f32,
    pub _pad: [u32; 5],
}

const _: () = {
    assert!(size_of::<Mount>() == 48);
    assert!(size_of::<Riding>() == 32 + 32 + 16 * 12 + 32 + 16 + 16 + 48 * NECK_SECTIONS);
    assert!(size_of::<RtRide>() == 64);
    assert!(OFF_RIDE.is_multiple_of(64) && OFF_RT_RIDE.is_multiple_of(64));
    assert!(OFF_RIDE >= crate::OFF_MAGIC_END);
    assert!(OFF_RIDE_END <= crate::OFF_HUD_LAYOUT);
};
