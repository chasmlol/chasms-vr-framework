//! The game's 2D HUD and menus, described piece by piece, so the runtime can place each piece the
//! way native VR games do instead of hanging the whole flat layer in front of the face.
//!
//! The adapter still renders its HUD and menus into the one HUD texture (`TGT_HUD`). This table only
//! says which rectangle of it is what. A piece on its own has a role, which says where it goes:
//! subtitles on a caption strip low in view, prompts just above them, the ammo counter on the held
//! gun, status on the off-hand wrist, full-screen effects on the face, and the crosshair dropped (the
//! gun's own sights aim). Anything the table doesn't cover is drawn as before (one panel), unless the
//! adapter says otherwise (`HUDL_REST_HIDDEN`). A game that never writes the table keeps the single
//! panel.
//!
//! A game's menus are pieces too, in **groups**: a menu is cut into the parts that matter (its list,
//! its tabs, its buttons...), and each group floats in the world as one cluster, anchored where it
//! belongs: beside the thing it's about (a chest's list beside the chest, a merchant's wares beside
//! the merchant, dialogue at the speaker), in front of the player (their own inventory), or popped up
//! where they look (a question). The laser points at the pieces that take it ([`HUDE_POINT`]), and
//! where it points is where the game's own cursor goes: the game keeps its selection, scrolling and
//! clicks. Nothing is drawn but the game's own UI: a piece is a rectangle of what the game drew.
//!
//! A group can be held in the hands too ([`ANCHOR_HELD`]): a book's pages lying on the book a hand
//! holds, turned by the other hand, or a map held like paper between them, panned by a hand's grip.
//!
//! The game stays the source of truth for *when* a piece shows: the runtime draws a rectangle
//! every frame, so it appears and fades exactly as the game draws and fades it.

use bytemuck::{Pod, Zeroable};

/// The layout lives in the mapping's last 4 KB.
pub const OFF_HUD_LAYOUT: usize = crate::MAPPING_BYTES - 0x1000;
pub const MAX_HUD_ELEMENTS: usize = 48;
pub const MAX_HUD_GROUPS: usize = 8;

// ---- element roles: where the runtime puts each piece (see `crates/runtime/src/panels.rs`) ------
/// Spoken lines: a caption strip low in view (13 degrees under gaze), lazily following the head.
pub const HUD_SUBTITLES: u32 = 1;
/// Hints and tutorial prompts: a row just under gaze (6 degrees), lazily following the head.
pub const HUD_HINTS: u32 = 2;
/// The ammo counter: on the held gun's left side, facing the eye.
pub const HUD_AMMO: u32 = 3;
/// Status (health, filters, item counts): on the back of the off-hand wrist.
pub const HUD_STATUS: u32 = 4;
/// Full-screen effects (damage, fades, a gas mask's glass): fixed to the face, covering the view.
pub const HUD_OVERLAY: u32 = 5;
/// Button prompts and notifications ("A Continue", "+ammo"): the caption strip's right end.
pub const HUD_PROMPTS: u32 = 6;
/// Things the game puts at the screen centre (use prompts, markers): at gaze, a little below.
pub const HUD_CENTER: u32 = 7;
/// Not drawn at all (e.g. the crosshair).
pub const HUD_HIDDEN: u32 = 8;
/// What the game says about the thing in a hand (its own panel of a thing's name and numbers): over
/// the thing a hand holds or pulls, turned to the eyes, fading with it, each hand its own (drawn
/// from pictures of it: [`HudLayout::held`]); with nothing in a hand, where it is on the flat
/// screen. A game with this piece has its own card for what's held, so the runtime draws none of its
/// own ([`crate::ItemInfos`]).
pub const HUD_HELD: u32 = 9;
/// A piece of a menu: placed by its group (the element's `group`), not by its role.
pub const HUD_MENU: u32 = 10;
/// The compass (a heading strip): low in front of the body, lazily following the head's yaw, where
/// a glance down finds it.
pub const HUD_COMPASS: u32 = 11;
/// What's said about a foe (its health): over the thing the layout's `target` says, facing the eye.
pub const HUD_TARGET: u32 = 12;
/// What the eyes are on (a crosshair, what it can activate: "Open Door", a name): where the head
/// looks, its `offset` from that point (the group's units), smoothed so it doesn't shake with the
/// head, and out at the distance of the thing it's about (the layout's `gaze`), as big to the eye
/// wherever it is. Pieces drawn there are small, so it stays out of the way.
pub const HUD_GAZE: u32 = 13;
/// What a hand points at that it can use (the game's card of it: "Locked Gate", a name): over that
/// hand ([`HudElement::hand`]), turned to the eyes, as a held thing's card is over it.
pub const HUD_POINTED: u32 = 14;
/// What's in what a hand points at, to take there ([`crate::GS_QUICK_LOOT`]): at that thing
/// ([`HudElement::at`]), over it, or under it when it's over the middle of the view (a shelf), on its
/// near side, turned to the eyes and as big to them as the game's text there.
pub const HUD_LOOT: u32 = 15;

// ---- element flags ------------------------------------------------------------------------------
/// The laser points at it: where it hits is where the game's cursor goes (and the trigger clicks).
pub const HUDE_POINT: u32 = 1 << 0;
/// Its edges fade out over a few texels, so a piece cut from a bigger picture (the game's parchment)
/// doesn't end in a hard line.
pub const HUDE_SOFT: u32 = 1 << 1;
/// It's solid where the game drew it: a page (the game's parchment) is never seen through, whatever
/// coverage the game's blending left in the HUD texture's alpha. Where it drew half or more it's
/// opaque, in its own colour; fainter edges (a torn edge, a letter's) stay soft.
pub const HUDE_SOLID: u32 = 1 << 2;
/// Its coverage is its colour's: the game draws its colour there but not its coverage (Oblivion's
/// book and map pages, added as light over what's there), so what it drew is solid and what it left
/// black (the HUD texture's clear) is clear.
pub const HUDE_OPAQUE: u32 = 1 << 3;

// ---- group anchors: where a group floats (see `crates/runtime/src/hud.rs`) ----------------------
/// Beside a thing in the world (`HudGroup::at`, `radius`): a container, a merchant, a speaker. At the
/// thing's distance (within the laser's comfortable range), on its `side`, under the eyes, facing
/// them; it stays put in the world while the menu is open.
pub const ANCHOR_WORLD: u32 = 1;
/// In front of the player, where they faced when it opened: their own menus (inventory, magic, the
/// map, the journal, pause). It comes round after a big turn.
pub const ANCHOR_PLAYER: u32 = 2;
/// A question or a count over another menu: in front of the eyes where they look, a little nearer
/// than what's behind it.
pub const ANCHOR_POPUP: u32 = 3;
/// On a thing in the hands (its [`HudLayout::held`]): a book's pages on the book, a map held like
/// paper. It goes where the hands take it, every frame, and it's drawn with the body (a hand in
/// front of a page covers it; the fingers behind a sheet don't show through it). There's no laser
/// on it: the free hand's fingertip presses its buttons (`Intents::cursor_uv`; in a menu the game's
/// cursor and click). A game may hold one with no menu open (a book read as it plays on).
pub const ANCHOR_HELD: u32 = 4;
/// Over a view, as a title screen in the world: `HudGroup::at` is what the view is of (far off,
/// in the world, metres). The group floats out over the view a few metres away, beside its subject
/// on `side` (so it never covers it), a little under the horizon, and stays put in the world while
/// the player looks round.
pub const ANCHOR_VISTA: u32 = 5;

// ---- how a group lies on what holds it (`HudHeld::flags`) ----------------------------------------
/// On the thing's broad face, the one turned to the eyes as it appears (a book's cover): its middle
/// on the face's, upright along the face's longer side, a few millimetres out from it.
pub const HELD_FACE: u32 = 1 << 0;
/// Held like a sheet of paper: the hand at its middle, the sheet a hand's depth in front of it
/// (no finger holding it comes through) turned to the eyes. The other
/// hand's grip near either side spreads it between the two (the points of it the hands took stay in
/// them, within reason): pulling them apart makes it bigger.
pub const HELD_SHEET: u32 = 1 << 1;
/// Its pages turn by hand: the free hand's grip near them, moved across them (a page every few
/// centimetres), presses [`crate::Action::PageNext`] (leftwards) or
/// [`crate::Action::PagePrev`].
pub const HELD_PAGES: u32 = 1 << 2;
/// The free hand's grip on its face drags what's on it: the game's cursor goes where the hand is on
/// it, its button down, as the hand moves (a map's pan), until the grip opens.
pub const HELD_DRAG: u32 = 1 << 3;
/// Its thing comes to the hand (`hand`) if no hand has it: a book read where it lies (activated on a
/// shelf) flies into the off hand, as the gloves would bring it, and opens there.
pub const HELD_BRING: u32 = 1 << 4;
/// [`HudHeld::centre`] says where the thing's faces are centred.
pub const HELD_CENTRE: u32 = 1 << 5;
/// On the thing but turned to the eyes from its middle, upright, just in front of it as the eyes
/// see it (instead of [`HELD_FACE`]): however the hand turns it, it's read face on (a book's pages
/// over the book).
pub const HELD_FACING: u32 = 1 << 6;
/// [`HudHeld::hand`]: whichever hand the laser doesn't come from (the player's off hand).
pub const HELD_OFF_HAND: u32 = 2;

// ---- which side of its thing a group goes (`HudGroup::side`) --------------------------------------
pub const SIDE_RIGHT: u32 = 0;
pub const SIDE_LEFT: u32 = 1;
/// Under it (a speaker's line under their face).
pub const SIDE_BELOW: u32 = 2;
/// In front of it, between it and the eyes.
pub const SIDE_FRONT: u32 = 3;
/// Over it (a speaker's name over their head).
pub const SIDE_ABOVE: u32 = 4;

// ---- layout flags -------------------------------------------------------------------------------
/// Don't draw what no element covers.
pub const HUDL_REST_HIDDEN: u32 = 1 << 0;
/// The HUD texture's colour is added to the view (it has no usable alpha): every element is
/// blended additively.
pub const HUDL_ADDITIVE: u32 = 1 << 1;
/// The HUD texture's alpha is the game's coverage (its colour premultiplied by it, over clear): every
/// element is drawn over the view as the game drew it, its art as opaque as the game made it.
pub const HUDL_ALPHA: u32 = 1 << 2;
/// The runtime's own settings float beside this menu (the game's pause menu).
pub const HUDL_SETTINGS: u32 = 1 << 3;


#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct HudElement {
    /// `HUD_*`; 0 marks an unused entry.
    pub role: u32,
    pub flags: u32,
    /// The rectangle of the HUD texture: u0, v0, u1, v1 (0..1, top-left origin).
    pub uv: [f32; 4],
    /// Size relative to the role's default, or to its group's size (0 means 1).
    pub scale: f32,
    /// The group it floats in (`HudLayout::groups[group - 1]`); 0: none, placed by its role.
    pub group: u32,
    /// Where it floats in its group: its centre from the group's centre, x right and y up, in the
    /// group's units (the HUD texture's height at the group's size), and z towards the eye (a piece
    /// lifted over the rest).
    pub offset: [f32; 3],
    /// How opaque it's drawn (0 means 1).
    pub opacity: f32,
    /// For [`HUD_POINTED`]: the hand it's beside ([`crate::HAND_LEFT`], [`crate::HAND_RIGHT`]).
    pub hand: u32,
    /// For [`HUD_LOOT`]: the thing it's of, its centre in the world (the adapter's world space,
    /// metres).
    pub at: [f32; 3],
}

/// Where a group of pieces floats.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct HudGroup {
    /// `ANCHOR_*`.
    pub anchor: u32,
    /// `SIDE_*`: which side of its thing (`ANCHOR_WORLD`).
    pub side: u32,
    /// The thing it's beside: its centre in the world (the adapter's world space, metres), how far
    /// it reaches round that across, and up and down (metres; 0: as far as across).
    pub at: [f32; 3],
    pub radius: f32,
    /// The adapter's name for what the group is (a menu and its thing): a new one is placed afresh,
    /// the same one stays where it was.
    pub id: u32,
    pub height: f32,
}

/// What a group anchored in the hands ([`ANCHOR_HELD`]) is held by, and how it lies there.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct HudHeld {
    /// The thing it's on ([`crate::ObjectDesc::id`]: the runtime has where it is), or 0: in a hand
    /// with nothing under it (`hand`).
    pub object: u32,
    /// With no object: the hand it's in ([`crate::HAND_LEFT`], [`crate::HAND_RIGHT`], or
    /// [`HELD_OFF_HAND`]).
    pub hand: u32,
    pub flags: u32, // HELD_*
    /// Pieces wholly to one side of the group's middle (x = 0) tip towards the eye about it by this
    /// much (radians): an open book's two pages.
    pub fold: f32,
    /// With [`HELD_CENTRE`]: the middle of what's seen of the thing (its model's, in its own space),
    /// where its faces are centred instead of on its box (a collision box can be off its model).
    pub centre: [f32; 3],
    pub _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HudLayout {
    /// Seqlock word; bump through `Link::write_hud_layout`.
    pub seq: u32,
    /// Entries in use are `elements[..count]`.
    pub count: u32,
    pub flags: u32, // HUDL_*
    /// Groups in use are `groups[..group_count]`.
    pub group_count: u32,
    /// How tall a line of the game's ordinary text is, as a part of the HUD texture's height (0: not
    /// known): the runtime sizes groups so it reads at a comfortable angle.
    pub text_height: f32,
    /// What [`HUD_TARGET`] pieces float over: a point in the world (metres), if `has_target`.
    pub target: [f32; 3],
    pub has_target: u32,
    /// What [`HUD_GAZE`] pieces are about (the thing looked at): its centre in the world (metres)
    /// and how far it reaches round that (0: nothing, or not said). They float just in front of it.
    pub gaze: [f32; 3],
    pub gaze_radius: f32,
    /// The laser's colours in the game's own palette (its cursor's, its UI's): the beam's bright
    /// thread and the glow round it and its dot, each `0xRRGGBBAA` in sRGB (0: the runtime's own).
    pub pointer: [u32; 2],
    /// What the game's card ([`HUD_HELD`]) is of now, once it shows it: a thing in a hand, by the id
    /// its card is known by ([`crate::ItemInfos`]: the thing's own, or its template's). 0: none (it's
    /// of what's looked at, or still laying out a new thing: a game takes a moment, and meanwhile
    /// it's of what it was). The runtime keeps a picture of the card for each thing it's been of and
    /// draws each hand's from its thing's: both hands have theirs, though the card is of one thing
    /// at a time.
    pub card_of: u32,
    pub groups: [HudGroup; MAX_HUD_GROUPS],
    pub elements: [HudElement; MAX_HUD_ELEMENTS],
    /// What holds each group anchored in the hands (`held[group - 1]`, for [`ANCHOR_HELD`]).
    pub held: [HudHeld; MAX_HUD_GROUPS],
}

impl HudLayout {
    pub fn entries(&self) -> &[HudElement] {
        &self.elements[..(self.count as usize).min(MAX_HUD_ELEMENTS)]
    }

    pub fn group_entries(&self) -> &[HudGroup] {
        &self.groups[..(self.group_count as usize).min(MAX_HUD_GROUPS)]
    }

    /// The group an element floats in, if any.
    pub fn group_of(&self, e: &HudElement) -> Option<&HudGroup> {
        (e.group as usize).checked_sub(1).and_then(|i| self.group_entries().get(i))
    }

    /// Adds a group (if there's room); its number for [`HudElement::group`].
    pub fn add_group(&mut self, g: HudGroup) -> u32 {
        let n = self.group_count as usize;
        if n >= MAX_HUD_GROUPS {
            return 0;
        }
        self.groups[n] = g;
        self.group_count += 1;
        self.group_count
    }

    /// Adds a group held in the hands ([`ANCHOR_HELD`], held as `held` says); its number.
    pub fn add_held(&mut self, mut g: HudGroup, held: HudHeld) -> u32 {
        g.anchor = ANCHOR_HELD;
        let n = self.add_group(g);
        if n > 0 {
            self.held[n as usize - 1] = held;
        }
        n
    }

    /// What holds group `g` (one of [`HudLayout::group_entries`]), if it's held in the hands.
    pub fn held_of(&self, group: u32) -> Option<&HudHeld> {
        let i = (group as usize).checked_sub(1)?;
        self.group_entries().get(i).filter(|g| g.anchor == ANCHOR_HELD).map(|_| &self.held[i])
    }

    /// Adds an element (if there's room).
    pub fn push(&mut self, e: HudElement) {
        let n = self.count as usize;
        if n < MAX_HUD_ELEMENTS {
            self.elements[n] = e;
            self.count += 1;
        }
    }

    /// A layout of `elements` (at most [`MAX_HUD_ELEMENTS`]) with `flags`.
    pub fn new(flags: u32, elements: &[HudElement]) -> HudLayout {
        let mut l = HudLayout::zeroed();
        l.flags = flags;
        let n = elements.len().min(MAX_HUD_ELEMENTS);
        l.elements[..n].copy_from_slice(&elements[..n]);
        l.count = n as u32;
        l
    }
}

impl HudElement {
    pub fn new(role: u32, uv: [f32; 4]) -> HudElement {
        HudElement { role, uv, ..Default::default() }
    }

    /// A piece of group `group` (from [`HudLayout::add_group`]), its centre at `offset` in the group.
    pub fn piece(group: u32, uv: [f32; 4], offset: [f32; 3], flags: u32) -> HudElement {
        HudElement { role: HUD_MENU, flags, uv, group, offset, ..Default::default() }
    }
}

const _: () = {
    use std::mem::size_of;
    assert!(size_of::<HudElement>() == 64);
    assert!(size_of::<HudGroup>() == 32);
    assert!(size_of::<HudHeld>() == 32);
    assert!(size_of::<HudLayout>() == 64 + 32 * MAX_HUD_GROUPS + 64 * MAX_HUD_ELEMENTS + 32 * MAX_HUD_GROUPS);
    assert!(size_of::<HudLayout>() <= 0x1000);
    assert!(OFF_HUD_LAYOUT.is_multiple_of(64));
    assert!(crate::OFF_KICKS + size_of::<crate::KickTable>() <= OFF_HUD_LAYOUT);
    assert!(crate::OFF_SOUNDS + size_of::<crate::SoundPack>() <= OFF_HUD_LAYOUT);
    assert!(OFF_HUD_LAYOUT + size_of::<HudLayout>() <= crate::MAPPING_BYTES);
};
