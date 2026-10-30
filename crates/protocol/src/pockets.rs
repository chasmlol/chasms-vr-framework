//! Pockets: places on the game's creatures (a belt pouch, a purse, a coat's inner pocket, a bag on
//! the back) that the player's hand reaches into, and what's in them: for picking pockets by hand,
//! and for planting things on people.
//!
//! The game stays the source of truth for what's in them (its own inventory, as its own
//! pickpocketing lists it), whether a take works (its own chance, rolled as the thing comes out),
//! what being caught does (its crime, its reaction) and what may be planted. The runtime only
//! decides that the hand reached in, which thing the fingers closed on, and when it came out.
//!
//! * [`Pockets`] (game -> runtime, every frame while there are any): each pocket where it is on its
//!   creature (world space, from the creature's own bones), what's in it (each thing's template
//!   object, for its model and weight in the hand, and how it feels to the fingers) and how aware
//!   its creature is of the player; and the game's answers to the attempts ([`PocketOutcome`]).
//! * [`PocketEvents`] (runtime -> game, a ring): a hand going into a creature's pockets and coming
//!   away (the game's pickpocketing of it opening and closing), a thing pulled out of one (the
//!   attempt), a thing let go in one (planting), what became of a thing taken (put in a body
//!   slot, or let go in the world), and which thing the fingers are on (for the game's card of it).
//!
//! All positions are world space, metres, the protocol's axes.

use bytemuck::{Pod, Zeroable};

/// After the body's slots' edits, the last of the chained regions before it.
pub const OFF_POCKETS: usize = (crate::OFF_SLOT_EDITS + size_of::<crate::SlotEdits>()).next_multiple_of(64);
pub const MAX_POCKETS: usize = 32;
pub const MAX_POCKET_ITEMS: usize = 12;
pub const MAX_POCKET_OUTCOMES: usize = 16;

// ---- how a thing feels to the fingers (the tick a hand feels as it comes on it) ---------------
/// Cloth, leather, food, herbs.
pub const FEEL_SOFT: u32 = 0;
/// Coins: a purse.
pub const FEEL_COINS: u32 = 1;
/// Small metal: a key, a lockpick, a ring.
pub const FEEL_KEY: u32 = 2;
/// A bottle, a gem, a soul gem.
pub const FEEL_GLASS: u32 = 3;
/// A book, a note, a scroll.
pub const FEEL_PAPER: u32 = 4;
/// Anything hard and bigger: a weapon, armour, a tool.
pub const FEEL_HARD: u32 = 5;

/// One thing in a pocket.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct PocketItem {
    /// The game's id for it while it's in this pocket (passed back when it's taken).
    pub id: u32,
    /// The template object ([`crate::OBJF_TEMPLATE`]) the hand takes out a copy of: the game's
    /// model ([`crate::OBJF_RUNTIME_DRAWS`]), its shape and its weight.
    pub object: u32,
    pub feel: u32, // FEEL_*
    /// How many come out together (a purse's coins, all of them).
    pub count: u32,
}

/// The game won't have this creature robbed now (it caught the player before): a hand finds it shut.
pub const POCKET_SHUT: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Pocket {
    /// Non-zero and stable while it's there.
    pub id: u32,
    /// The creature it's on ([`crate::Actor::id`]), and the part of it it rides on (`n`: part
    /// `n - 1` in [`crate::Actor::parts`]; 0: none in particular).
    pub actor: u32,
    pub part: u32,
    pub flags: u32, // POCKET_*
    /// Its opening, and how far round it a hand is in it.
    pub center: [f32; 3],
    pub radius: f32,
    /// Which way its opening faces (out of the body).
    pub mouth: [f32; 3],
    /// Things in use are `items[..count]`.
    pub count: u32,
    /// How aware its creature is of the player now, by the game's own detection: 0 not at all, 1
    /// it sees them plainly.
    pub awareness: f32,
    pub _pad: [u32; 3],
    pub items: [PocketItem; MAX_POCKET_ITEMS],
}

// ---- the game's answers ------------------------------------------------------------------------
/// The take worked: the thing in the hand is the player's now.
pub const PO_TAKEN: u32 = 1;
/// It didn't: the thing stays where it was (the hand's copy goes) and the game reacts.
pub const PO_CAUGHT: u32 = 2;
/// The thing let go in the pocket went into it.
pub const PO_PLANTED: u32 = 3;
/// The game won't have it planted: it stays out (it falls from the pocket).
pub const PO_REFUSED: u32 = 4;
/// The taken thing let go in the world is the game's own thing there now (the runtime's copy goes).
pub const PO_DROPPED: u32 = 5;

/// The game's answer to one event.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct PocketOutcome {
    /// Which event it answers: its number in the ring (see [`PocketEvents::written`]) plus one.
    pub event: u64,
    /// The object the event was about ([`PocketEvent::object`]).
    pub object: u32,
    pub outcome: u32, // PO_*
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Pockets {
    /// Seqlock word.
    pub seq: u32,
    /// Pockets in use are `pockets[..count]`.
    pub count: u32,
    /// The game frame they're from.
    pub frame: u64,
    /// Answers in use are `outcomes[..outcome_count]`: the latest ones, oldest first (the runtime
    /// takes those it hasn't had yet, by their events' numbers).
    pub outcome_count: u32,
    pub _pad: [u32; 11],
    pub pockets: [Pocket; MAX_POCKETS],
    pub outcomes: [PocketOutcome; MAX_POCKET_OUTCOMES],
}

impl Pockets {
    pub fn pockets(&self) -> &[Pocket] {
        &self.pockets[..(self.count as usize).min(MAX_POCKETS)]
    }

    pub fn outcomes(&self) -> &[PocketOutcome] {
        &self.outcomes[..(self.outcome_count as usize).min(MAX_POCKET_OUTCOMES)]
    }

    /// Keeps `o` as the newest answer (the oldest goes when it's full).
    pub fn push_outcome(&mut self, o: PocketOutcome) {
        let n = (self.outcome_count as usize).min(MAX_POCKET_OUTCOMES);
        if n == MAX_POCKET_OUTCOMES {
            self.outcomes.copy_within(1.., 0);
            self.outcomes[n - 1] = o;
        } else {
            self.outcomes[n] = o;
            self.outcome_count = n as u32 + 1;
        }
    }
}

impl Pocket {
    pub fn items(&self) -> &[PocketItem] {
        &self.items[..(self.count as usize).min(MAX_POCKET_ITEMS)]
    }
}

// ---- what the hands did ------------------------------------------------------------------------
pub const OFF_POCKET_EVENTS: usize = (OFF_POCKETS + size_of::<Pockets>()).next_multiple_of(64);
pub const POCKET_EVENT_SLOTS: usize = 64;

/// A hand has been in `actor`'s pockets a moment (`pocket` the one it's in), or is taking a thing
/// out of one or planting one, or found them shut: the game's pickpocketing of it opens (shut, the
/// game's refusal). A quicker look is only feeling, and the game has no part in it.
pub const PE_OPEN: u32 = 1;
/// The hands have been away from all of `actor`'s pockets a moment (or its pockets went), nothing
/// waiting on the game: its pickpocketing closes.
pub const PE_CLOSE: u32 = 2;
/// Thing `item` pulled out of `pocket` by `hand`, as `object` (the copy in the hand): the attempt.
/// `speed`: how fast the hand came out (m/s).
pub const PE_TAKE: u32 = 3;
/// `object` let go in `pocket` (a thing to take, or one taken): planting it.
pub const PE_PLANT: u32 = 4;
/// A taken thing (`object`, the player's now) let go in the world at `pos` / `rot`, moving at
/// `vel`: the game puts its own thing there.
pub const PE_DROP: u32 = 5;
/// A taken thing (`object`) put in a body slot: the player keeps it (it's theirs already).
pub const PE_STOW: u32 = 6;
/// What `hand` is on in the pockets changed: it's in `pocket` of `actor` (0: in none), its
/// fingers on thing `item`, whose template is `object` (0: on nothing, as in an empty or shut
/// pocket, or holding a thing). The game shows its card of the thing the fingers are on.
pub const PE_FEEL: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct PocketEvent {
    pub kind: u32, // PE_*
    pub actor: u32,
    pub pocket: u32,
    pub item: u32,
    pub object: u32,
    /// 0 left, 1 right.
    pub hand: u32,
    pub _p0: [u32; 2],
    pub pos: [f32; 3],
    pub speed: f32,
    pub rot: [f32; 4],
    pub vel: [f32; 3],
    pub _p1: f32,
}

/// Runtime -> game: `written` events so far; event `n` is in slot `n % POCKET_EVENT_SLOTS`. The
/// game keeps how many it has read (and skips ahead if it fell more than a ring behind).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PocketEvents {
    pub written: u64,
    pub _pad: [u32; 14],
    pub events: [PocketEvent; POCKET_EVENT_SLOTS],
}

pub const OFF_POCKETS_END: usize = OFF_POCKET_EVENTS + size_of::<PocketEvents>();

const _: () = {
    assert!(size_of::<PocketItem>() == 16);
    assert!(size_of::<Pocket>() == 64 + 16 * MAX_POCKET_ITEMS);
    assert!(size_of::<PocketOutcome>() == 16);
    assert!(size_of::<PocketEvent>() == 80);
    assert!(OFF_POCKETS.is_multiple_of(64));
    assert!(OFF_POCKETS_END <= crate::OFF_HUD_LAYOUT);
};
