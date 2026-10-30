//! Cards made of a game's own interface: a small panel the runtime draws beside one of the game's
//! objects (what a bench would make, say), put together from the game's own pieces: its menu art
//! cut out (a texture, and the part of it), and lines of text in its own bitmap font, colour and
//! wrap, where the game's own menu would have them.
//!
//! A card is laid out in its own units (the game's interface units: Oblivion's menus are 960
//! high), `size` across and down, every piece a rectangle of it. The textures and fonts come from
//! the runtime's asset cache by name (`<game>/<key>`: a texture's, or a font's, see
//! `rustvr_asset::Font`). The runtime puts the card over its `anchor`, turned to the eyes, fading
//! in when it comes and out when it goes, and paints it again whenever its `revision` changes. A
//! card anchored on a sheet ([`crate::OBJF_SHEET`]) is that sheet's face instead: drawn on it, in
//! the world's depth and light, all of the card over all of the face.
//!
//! The adapter writes the table whenever a card comes, goes or changes.

use bytemuck::{Pod, Zeroable};

pub const OFF_CARDS: usize = (crate::OFF_STATIONS + size_of::<crate::Receptacles>()).next_multiple_of(64);
/// (Two to a bench, say: what it would make, and a page of what to make.)
pub const MAX_CARDS: usize = 8;
pub const MAX_CARD_PIECES: usize = 24;

/// A piece of the game's art: the part `uv` of the texture `asset`, stretched over `rect`.
pub const PIECE_IMAGE: u32 = 1;
/// A line of the game's text: `text` in the font `asset`, `color`, from `rect`'s top left (its
/// width the wrap width; 0: no wrap).
pub const PIECE_TEXT: u32 = 2;

/// Text: centred across `rect` (else from its left).
pub const PIECEF_CENTRE: u32 = 1 << 0;
/// Text: to `rect`'s right edge.
pub const PIECEF_RIGHT: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct CardPiece {
    pub kind: u32,  // PIECE_*
    pub flags: u32, // PIECEF_*
    pub _pad: [u32; 2],
    /// Where on the card: x, y (from the top left), width, height (card units).
    pub rect: [f32; 4],
    /// Images: the part of the texture, u0, v0, u1, v1 (0..1, top left origin).
    pub uv: [f32; 4],
    /// RGBA 0..1 as the game gives them (sRGB): an image's tint, a text's colour.
    pub color: [f32; 4],
    /// The texture's or the font's name, `<game>/<key>` (UTF-8, NUL-terminated).
    pub asset: [u8; 32],
    /// UTF-8, NUL-terminated.
    pub text: [u8; 96],
}

impl CardPiece {
    pub fn image(asset: &str, rect: [f32; 4], uv: [f32; 4]) -> CardPiece {
        let mut p = CardPiece::zeroed();
        p.kind = PIECE_IMAGE;
        p.rect = rect;
        p.uv = uv;
        p.color = [1.0; 4];
        crate::set_name(&mut p.asset, asset);
        p
    }

    pub fn text(font: &str, text: &str, rect: [f32; 4], color: [f32; 4], flags: u32) -> CardPiece {
        let mut p = CardPiece::zeroed();
        p.kind = PIECE_TEXT;
        p.flags = flags;
        p.rect = rect;
        p.color = color;
        crate::set_name(&mut p.asset, font);
        crate::set_name(&mut p.text, text);
        p
    }

    pub fn asset(&self) -> &str {
        crate::name_str(&self.asset)
    }

    pub fn text_str(&self) -> &str {
        crate::name_str(&self.text)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Card {
    /// The object it's over ([`crate::ObjectDesc::id`]); 0 marks an unused entry.
    pub anchor: u32,
    pub flags: u32,
    /// Changes whenever its pieces do.
    pub revision: u32,
    /// Pieces in use are `pieces[..count]`, drawn in order.
    pub count: u32,
    /// Its width and height (card units).
    pub size: [f32; 2],
    /// How wide it is in the world (metres; 0: the runtime's own size for a card).
    pub world_width: f32,
    pub _pad: u32,
    pub pieces: [CardPiece; MAX_CARD_PIECES],
}

impl Card {
    pub fn new(anchor: u32, size: [f32; 2]) -> Card {
        let mut c = Card::zeroed();
        c.anchor = anchor;
        c.size = size;
        c
    }

    pub fn pieces(&self) -> &[CardPiece] {
        &self.pieces[..(self.count as usize).min(MAX_CARD_PIECES)]
    }

    /// Adds a piece (false when full).
    pub fn push(&mut self, piece: CardPiece) -> bool {
        let n = self.count as usize;
        if n >= MAX_CARD_PIECES {
            return false;
        }
        self.pieces[n] = piece;
        self.count += 1;
        true
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Cards {
    /// Seqlock word.
    pub seq: u32,
    /// Cards in use are `cards[..count]` (with gaps where `anchor` is 0).
    pub count: u32,
    pub _pad: [u32; 14],
    pub cards: [Card; MAX_CARDS],
}

impl Cards {
    pub fn entries(&self) -> &[Card] {
        &self.cards[..(self.count as usize).min(MAX_CARDS)]
    }
}

const _: () = {
    assert!(size_of::<CardPiece>() == 64 + 32 + 96);
    assert!(size_of::<Card>() == 32 + size_of::<CardPiece>() * MAX_CARD_PIECES);
    assert!(OFF_CARDS.is_multiple_of(64));
    assert!(OFF_CARDS + size_of::<Cards>() <= crate::OFF_HUD_LAYOUT);
};

