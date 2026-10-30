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
/// the thing a hand holds or pulls, turned to the eyes, fading with it; with nothing in a hand,
/// where it is on the flat screen. A game with this piece has its own card for what's held, so the
/// runtime draws none of its own ([`crate::ItemInfos`]).
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

// ---- element flags ------------------------------------------------------------------------------
/// The laser points at it: where it hits is where the game's cursor goes (and the trigger clicks).
pub const HUDE_POINT: u32 = 1 << 0;
/// Its edges fade out over a few texels, so a piece cut from a bigger picture (the game's parchment)
/// doesn't end in a hard line.
pub const HUDE_SOFT: u32 = 1 << 1;

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
    pub _pad: [u32; 4],
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
    pub _pad: [u32; 1],
    pub groups: [HudGroup; MAX_HUD_GROUPS],
    pub elements: [HudElement; MAX_HUD_ELEMENTS],
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

/// Metro 2033 Redux's UI layer in pieces, for a layer `aspect` (width / height) wide: what its
/// adapter sends, and what the test range sends standing in for it. Measured on Redux's UI layer:
/// the game lays its HUD out relative to the screen's height, so at the eyes' nearly square aspect
/// the subtitles wrap narrower and the ammo block (about 0.30 heights wide) reaches further in from
/// the bottom-right corner, at least to where the subtitles end (74 % across).
pub fn metro_2033_redux_layout(aspect: f32) -> HudLayout {
    let a = (1.0 - 0.30 / aspect.max(0.5)).max(0.74);
    HudLayout::new(
        HUDL_REST_HIDDEN,
        &[
            HudElement::new(HUD_SUBTITLES, [0.12, 0.805, a, 0.935]),
            HudElement::new(HUD_HINTS, [0.12, 0.64, 0.88, 0.805]),
            HudElement::new(HUD_AMMO, [a, 0.805, 1.0, 1.0]),
            HudElement::new(HUD_STATUS, [0.0, 0.805, 0.12, 1.0]),
            HudElement::new(HUD_PROMPTS, [0.12, 0.935, a, 1.0]),
            // Everything above, where the flat screen had it.
            HudElement::new(HUD_CENTER, [0.0, 0.0, 1.0, 0.64]),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metros_pieces_tile_without_overlapping() {
        for aspect in [16.0 / 9.0, 2552.0 / 2536.0, 0.9] {
            let l = metro_2033_redux_layout(aspect);
            let e = l.entries();
            for (i, a) in e.iter().enumerate() {
                assert!(a.uv[0] < a.uv[2] && a.uv[1] < a.uv[3]);
                for b in &e[i + 1..] {
                    let overlap = a.uv[0] < b.uv[2] && b.uv[0] < a.uv[2] && a.uv[1] < b.uv[3] && b.uv[1] < a.uv[3];
                    assert!(!overlap, "{a:?} overlaps {b:?}");
                }
            }
        }
        // The ammo reaches in further on a narrower layer, but never into the subtitles.
        let ammo = |a: f32| metro_2033_redux_layout(a).entries().iter().find(|e| e.role == HUD_AMMO).unwrap().uv[0];
        assert!(ammo(16.0 / 9.0) > 0.8 && (ammo(1.07) - 0.74).abs() < 1e-6);
    }
}

const _: () = {
    use std::mem::size_of;
    assert!(size_of::<HudElement>() == 64);
    assert!(size_of::<HudGroup>() == 32);
    assert!(size_of::<HudLayout>() == 64 + 32 * MAX_HUD_GROUPS + 64 * MAX_HUD_ELEMENTS);
    assert!(size_of::<HudLayout>() <= 0x1000);
    assert!(OFF_HUD_LAYOUT.is_multiple_of(64));
    assert!(crate::OFF_KICKS + size_of::<crate::KickTable>() <= OFF_HUD_LAYOUT);
    assert!(crate::OFF_SOUNDS + size_of::<crate::SoundPack>() <= OFF_HUD_LAYOUT);
    assert!(OFF_HUD_LAYOUT + size_of::<HudLayout>() <= crate::MAPPING_BYTES);
};
