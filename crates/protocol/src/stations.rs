//! Things the hands make things in: a bowl (a mortar, a pot) that keeps what's let go into it, and
//! what grinds in it (a pestle).
//!
//! The adapter says which of its objects ([`crate::ObjectDesc`]) is a bowl and how it's shaped, how
//! much it holds, which of its objects may go in now (the game's rules: what it takes, how many,
//! never the same kind twice) and what grinds in it. The runtime does the hands' part:
//!
//! * Something that may go in, let go over the bowl's mouth, goes in. The bowl keeps it, laid on its
//!   floor beside what's there already, and carries it with it however it's moved; a grab takes it
//!   out again. Its state is [`OS_IN_RECEPTACLE`] with the bowl as its `parent`, its pose where it
//!   lies in the bowl (in tracking space while the bowl is held), and `fuse` how far it's ground,
//!   0 .. 1.
//! * The bowl's collision is a bowl (a floor and a ring of wall, `radius` and `depth` inside and
//!   `wall` thick), so hands and what they hold go into it.
//! * With [`REC_GRIND`], its grinder's tip (`tip`, `tip_radius`, in the grinder's own object space)
//!   pressed low in the bowl and worked about grinds what's in it: every metre the tip goes along
//!   the floor grinds each thing `1 / grind_work` of the way. The bowl's own state has the share of
//!   all it holds that's ground in `slide` (0 .. 1) and the grinding strokes so far in `shots`
//!   (each increase one stroke, for the game's sound), and the hand on the grinder feels it. The
//!   grinder rests in the bowl (`rest_pos`, `rest_rot`) until a hand takes it, and goes back there
//!   when it's let go over the mouth.
//! * A thing in a bowl bigger than its mouth is drawn shrunk to fit, and smaller as it's ground:
//!   its state's `slide` is the size to draw it at (1: its own).
//!
//! The adapter writes the table whenever any of that changes; a bowl not in it is an ordinary thing.

use bytemuck::{Pod, Zeroable};

pub const OFF_STATIONS: usize = crate::OFF_POCKETS_END.next_multiple_of(64);
pub const MAX_RECEPTACLES: usize = 8;
/// The objects that may go into one bowl now, at most.
pub const MAX_ACCEPTS: usize = 40;

/// What's in it is ground by its grinder.
pub const REC_GRIND: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Receptacle {
    /// The bowl ([`crate::ObjectDesc::id`]); 0 marks an unused entry.
    pub id: u32,
    pub flags: u32, // REC_*
    /// How many things it holds at most.
    pub capacity: u32,
    /// The object that grinds in it (0: none).
    pub grinder: u32,
    /// The middle of its inside floor (object space); its mouth is straight up from there, along
    /// the object's +Y.
    pub floor: [f32; 3],
    /// The mouth's inner radius (metres).
    pub radius: f32,
    /// From the floor up to the rim.
    pub depth: f32,
    /// How thick its wall is (its collision).
    pub wall: f32,
    /// Metres of grinding (the tip pressed along the floor) each thing takes to be ground.
    pub grind_work: f32,
    pub _p0: f32,
    /// The grinder's tip (the grinder's own object space) and how big it is.
    pub tip: [f32; 3],
    pub tip_radius: f32,
    /// Where the grinder rests in it (its pose in the bowl's object space): where it starts, and
    /// where it goes back to when let go over the mouth.
    pub rest_pos: [f32; 3],
    pub _p2: f32,
    pub rest_rot: [f32; 4],
    /// Entries in use are `accepts[..accept_count]`.
    pub accept_count: u32,
    pub _p1: [u32; 3],
    /// What may go in now (object ids).
    pub accepts: [u32; MAX_ACCEPTS],
}

impl Receptacle {
    pub fn accepts(&self) -> &[u32] {
        &self.accepts[..(self.accept_count as usize).min(MAX_ACCEPTS)]
    }

    /// Lets `id` go in (false when the list is full).
    pub fn push_accept(&mut self, id: u32) -> bool {
        let n = self.accept_count as usize;
        if n >= MAX_ACCEPTS {
            return false;
        }
        self.accepts[n] = id;
        self.accept_count += 1;
        true
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Receptacles {
    /// Seqlock word.
    pub seq: u32,
    /// Entries in use are `entries[..count]` (with gaps where `id` is 0).
    pub count: u32,
    pub _pad: [u32; 14],
    pub entries: [Receptacle; MAX_RECEPTACLES],
}

impl Receptacles {
    pub fn entries(&self) -> &[Receptacle] {
        &self.entries[..(self.count as usize).min(MAX_RECEPTACLES)]
    }

    pub fn get(&self, id: u32) -> Option<&Receptacle> {
        self.entries().iter().find(|r| r.id == id && id != 0)
    }
}

const _: () = {
    assert!(size_of::<Receptacle>() == 112 + 4 * MAX_ACCEPTS);
    assert!(OFF_STATIONS.is_multiple_of(64));
};
