//! Pockets on the game's creatures (`rustvr_protocol::pockets`): a hand reaching into one, feeling
//! what's there, closing on one thing and pulling it out, or letting a thing go in one.
//!
//! The research (`docs/research/pickpocket.md` in a game's adapter, Oblivion's first) shapes it:
//!
//! * A pocket is found by touch: a soft brush as the hand goes in, then a tick for the thing it's on,
//!   each kind of thing its own (coins, a key, glass, paper, cloth, something hard). Every few
//!   centimetres the hand moves in the pocket it's on the next thing, another tick: a fast rummage
//!   skips past things, a slow hand finds the one it wants. An empty pocket is one dull brush, a
//!   shut one a refusal: who carries what is never a mystery (Thief VR's satchels that couldn't be
//!   picked were its players' top complaint).
//! * Closing the hand chooses (it's locked from then on, Alyx's way), and only pulling it out
//!   commits: let go inside, and it's back, nothing tried (Kingdom Come's way out). The game rolls
//!   as it comes out, and its answer decides whether the thing stays in the hand.
//! * Time never stops: no menu, the creature goes on moving, and the pocket with it.
//!
//! What's here is the pure part (which pocket a hand is in, the feel of things, the stages a thing
//! taken goes through); the rig ([`super::Rig`]) does the hands.

use glam::Vec3;
use rustvr_protocol::*;

/// How far a hand moves in a pocket from one thing to the next (metres).
pub const NEXT_THING: f32 = 0.04;
/// A hand in a pocket stays in it this much past its edge (so the edge doesn't flicker), and a thing
/// held in it comes out once the palm is this far past it.
pub const EDGE: f32 = 0.02;
pub const OUT: f32 = 0.03;
/// A creature's pickpocketing stays open while a hand is this near one of its pockets (past its
/// edge), and closes once none has been for [`SESSION_GAP`] seconds.
pub const NEAR: f32 = 0.12;
pub const SESSION_GAP: f64 = 0.75;
/// How long the runtime waits for the game's answer before giving up on it (seconds): a take
/// unanswered is as if it failed (the thing stays where it was), a drop as if done.
pub const ANSWER_WAIT: f64 = 3.0;
/// Its creature sees the player at least this much ([`Pocket::awareness`]): the hand in its pocket
/// feels a slow heartbeat, this often (seconds).
pub const WATCHED: f32 = 0.66;
pub const HEARTBEAT: f64 = 0.9;

/// The pocket (of `pockets`) a palm at `palm` is in: the one whose middle it's nearest, within its
/// radius (`current`, the one it's in already, a little further). `scale`: world metres a metre.
pub fn pocket_at(pockets: &[Pocket], palm: Vec3, current: Option<u32>, scale: f32) -> Option<u32> {
    pockets
        .iter()
        .filter_map(|p| {
            let d = palm.distance(Vec3::from_array(p.center));
            let reach = (p.radius + if Some(p.id) == current { EDGE } else { 0.0 }) * scale;
            (d < reach).then_some((p.id, d / reach))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// The palm's out of pocket `p`: past its edge by [`OUT`].
pub fn out_of(p: &Pocket, palm: Vec3, scale: f32) -> bool {
    palm.distance(Vec3::from_array(p.center)) > (p.radius + OUT) * scale
}

/// Near any of `actor`'s pockets (its pickpocketing stays open).
pub fn near_actor(pockets: &[Pocket], actor: u32, palm: Vec3, scale: f32) -> bool {
    pockets.iter().any(|p| p.actor == actor && palm.distance(Vec3::from_array(p.center)) < (p.radius + NEAR) * scale)
}

/// The tick a finger feels coming on a thing (seconds, frequency Hz, amplitude): coins a bright
/// jingle, a key a sharp click, glass a brief high ring, paper a soft rasp, cloth a dull push,
/// something hard a firm knock.
pub fn feel(kind: u32) -> (f32, f32, f32) {
    match kind {
        FEEL_COINS => (0.025, 320.0, 0.5),
        FEEL_KEY => (0.012, 260.0, 0.55),
        FEEL_GLASS => (0.008, 420.0, 0.45),
        FEEL_PAPER => (0.035, 110.0, 0.3),
        FEEL_HARD => (0.03, 150.0, 0.6),
        _ => (0.04, 70.0, 0.3),
    }
}

/// A hand in a pocket, its fingers on one of its things.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InPocket {
    pub pocket: u32,
    pub actor: u32,
    /// The thing it's on (an index into the pocket's things), and how far the hand has moved since
    /// it came on it; where it was last frame (world).
    pub index: usize,
    pub moved: f32,
    pub last: Vec3,
}

impl InPocket {
    /// The hand moved to `palm`: on to the next thing every [`NEXT_THING`] (of `count`), which it
    /// returns when it moves on.
    pub fn feel_along(&mut self, palm: Vec3, count: usize, scale: f32) -> Option<usize> {
        self.moved += palm.distance(self.last) / scale.max(1e-6);
        self.last = palm;
        if count == 0 || self.moved < NEXT_THING {
            return None;
        }
        self.moved = 0.0;
        self.index = (self.index + 1) % count;
        Some(self.index)
    }
}

/// A thing a hand took out of a pocket (a copy of its template), as it goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pocketed {
    /// In the hand, still in the pocket: still its creature's.
    Inside { pocket: u32, actor: u32, item: u32 },
    /// Pulled out: the game is deciding (the take's event number, since when).
    Deciding { event: u64, since: f64 },
    /// The game let it be taken: the player's.
    Taken,
    /// Let go in the world: the game is putting its own thing there (the drop's event, since when).
    Dropping { event: u64, since: f64 },
}
