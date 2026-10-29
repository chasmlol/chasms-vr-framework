//! Magic as the game draws it: where the player's casting is, for the game to draw its own casting
//! art there, and the game's own effect shaders on what the runtime draws.
//!
//! * [`Casters`] (runtime -> game, every frame): each hand's casting, as [`crate::combat`]'s magic
//!   has it: a spell readied in the palm (the finger on the trigger), charging (the trigger
//!   pulled), charged and ready; or a held thing that casts ([`crate::OBJF_CASTS`], a staff) doing
//!   the same at its tip. With where its art goes, in tracking space (so it never lags the hand) and
//!   which way it would go. A game with its own casting art draws it there and says so
//!   ([`crate::SPELLS_GAME_ART`]): the runtime then draws none of its own.
//! * [`Overlays`] (game -> runtime, when they change): the game's effect shader on something the
//!   runtime draws (an enchanted weapon's shimmer, a spell's shader on the player's own body): a
//!   texture scrolled over the model's surface, coloured and pulsing, brighter at its edges, over
//!   the model as the game draws its own.

use bytemuck::{Pod, Zeroable};

pub const OFF_CASTERS: usize = (crate::OFF_ITEM_INFO + size_of::<crate::ItemInfos>()).next_multiple_of(64);

/// Nothing in the hand.
pub const CAST_NONE: u32 = 0;
/// The spell is readied (the finger rests on the trigger, or pulls it a little): its art in the palm,
/// small.
pub const CAST_READIED: u32 = 1;
/// The trigger is pulled: the spell is charging (`charge` grows to 1 over the game's casting time).
pub const CAST_CHARGING: u32 = 2;
/// Charged: let go (or touch, for a touch spell) and it's cast.
pub const CAST_READY: u32 = 3;

/// It casts from the thing the hand holds (`object`), not from the palm.
pub const CASTF_OBJECT: u32 = 1 << 0;
/// It was just cast (this frame's event): the art goes, the game's own cast takes over.
pub const CASTF_CAST: u32 = 1 << 1;

/// One hand's casting this frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct Caster {
    pub state: u32, // CAST_*
    pub flags: u32, // CASTF_*
    /// What it casts: the spell's [`crate::Spell::id`], or what the held thing casts
    /// ([`crate::ObjectDesc::casts`]).
    pub spell: u32,
    /// The held thing it casts from ([`CASTF_OBJECT`]), else 0.
    pub object: u32,
    /// How charged, 0..1 (1: ready).
    pub charge: f32,
    /// Seconds in this state.
    pub time: f32,
    pub _pad: [u32; 2],
    /// Where its art goes (tracking space): just off the palm, +Z out of it and +Y along the fingers;
    /// or the held thing's tip, +Z the way it casts.
    pub at: crate::Pose,
    /// The way it would go if it were cast now (tracking space, a unit vector).
    pub aim: [f32; 3],
    pub _p0: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Casters {
    /// Seqlock word.
    pub seq: u32,
    pub _p0: u32,
    /// The runtime frame it's from.
    pub frame: u64,
    pub _pad: [u32; 12],
    pub hands: [Caster; 2],
}

// ---- overlays -------------------------------------------------------------------------------------
pub const OFF_OVERLAYS: usize = (OFF_CASTERS + size_of::<Casters>()).next_multiple_of(64);
pub const MAX_OVERLAYS: usize = 16;

/// The edge (where the surface turns away from the eye) shows: `edge` coloured, `edge_falloff`
/// sharp.
pub const OVL_EDGE: u32 = 1 << 0;
/// The fill shows (a colour, times `texture` when it has one).
pub const OVL_FILL: u32 = 1 << 1;
/// The edge is where the surface faces the eye, not where it turns away.
pub const OVL_EDGE_INVERSE: u32 = 1 << 2;
/// Added to what's there (else blended over it by its alpha).
pub const OVL_ADDITIVE: u32 = 1 << 3;

/// The game's effect shader on something the runtime draws: over its model's surface, each point
/// `fill · texture(uv + scroll · t)` and `edge · (1 − |n·v|)^edge_falloff` (`|n·v|` with
/// [`OVL_EDGE_INVERSE`]), each by its alpha (pulsing `alpha · (1 + pulse[0] · sin(2π · pulse[1] ·
/// t))`, at most 1), blended over it or added ([`OVL_ADDITIVE`]). Colours in the display's range,
/// as the game blends them. Without a texture the fill is its colour.
/// [`Overlay::object`]: the player's own body and hands (a spell's shader on the player: healing's
/// glow, a shield's flames, being burned).
pub const OVERLAY_BODY: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Overlay {
    /// What it's on: an [`crate::ObjectDesc::id`], or the player's body ([`OVERLAY_BODY`]).
    pub object: u32,
    pub flags: u32, // OVL_*
    pub _p0: [u32; 2],
    /// The texture, `<game>/<asset key>` in the runtime's asset cache (as an object's model is
    /// named). UTF-8, NUL-terminated.
    pub texture: [u8; 64],
    /// The fill: colour (0..1) and alpha.
    pub fill: [f32; 4],
    /// The fill's alpha pulse: amplitude (a share of it) and frequency (Hz).
    pub fill_pulse: [f32; 2],
    /// How fast the texture moves over the surface, in its own units (U, V) a second.
    pub scroll: [f32; 2],
    /// The edge: colour (0..1) and alpha.
    pub edge: [f32; 4],
    pub edge_pulse: [f32; 2],
    /// How sharply the edge falls off towards the middle (an exponent: 1 soft, higher sharper).
    pub edge_falloff: f32,
    pub _p1: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Overlays {
    /// Seqlock word.
    pub seq: u32,
    pub count: u32,
    pub _pad: [u32; 14],
    pub overlays: [Overlay; MAX_OVERLAYS],
}

pub const OFF_MAGIC_END: usize = OFF_OVERLAYS + size_of::<Overlays>();

const _: () = {
    assert!(size_of::<Caster>() == 80);
    assert!(size_of::<Overlay>() == 144);
    assert!(OFF_CASTERS.is_multiple_of(64));
    assert!(OFF_MAGIC_END <= crate::OFF_HUD_LAYOUT);
};

impl Overlay {
    pub fn texture(&self) -> &str {
        let n = self.texture.iter().position(|&c| c == 0).unwrap_or(self.texture.len());
        std::str::from_utf8(&self.texture[..n]).unwrap_or("")
    }

    pub fn set_texture(&mut self, s: &str) {
        crate::set_name(&mut self.texture, s);
    }
}

impl Overlays {
    pub fn entries(&self) -> &[Overlay] {
        &self.overlays[..(self.count as usize).min(MAX_OVERLAYS)]
    }

    pub fn get(&self, object: u32) -> Option<&Overlay> {
        self.entries().iter().find(|o| o.object == object)
    }
}
