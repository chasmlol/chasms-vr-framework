//! The game's 2D HUD, described piece by piece, so the runtime can place each piece the way native
//! VR games do instead of hanging the whole flat layer in front of the face.
//!
//! The adapter still renders its HUD into the one HUD texture (`TGT_HUD`). This table only says
//! which rectangle of it is what: subtitles go on a caption strip low in view, prompts just above
//! them, the ammo counter on the held gun, status on the off-hand wrist, full-screen effects stay
//! on the face, and the crosshair is dropped (the gun's own sights aim). Anything the table doesn't
//! cover is drawn as before (one panel), unless the adapter says otherwise (`HUDL_REST_HIDDEN`).
//! A game that never writes the table keeps the single panel.
//!
//! The game stays the source of truth for *when* a piece shows: the runtime draws a rectangle
//! every frame, so it appears and fades exactly as the game draws and fades it.

use bytemuck::{Pod, Zeroable};

/// The layout lives in the mapping's last 4 KB.
pub const OFF_HUD_LAYOUT: usize = crate::MAPPING_BYTES - 0x1000;
pub const MAX_HUD_ELEMENTS: usize = 32;

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

// ---- layout flags -------------------------------------------------------------------------------
/// Don't draw what no element covers.
pub const HUDL_REST_HIDDEN: u32 = 1 << 0;
/// The HUD texture's colour is added to the view (it has no usable alpha): every element is
/// blended additively.
pub const HUDL_ADDITIVE: u32 = 1 << 1;
/// The game's card ([`HUD_HELD`]) isn't of the thing in a hand yet: a game takes a moment to show a
/// new thing on it, and meanwhile it's of whatever it was (what was looked at). It isn't drawn over
/// the hand till it is.
pub const HUDL_HELD_PENDING: u32 = 1 << 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct HudElement {
    /// `HUD_*`; 0 marks an unused entry.
    pub role: u32,
    pub flags: u32,
    /// The rectangle of the HUD texture: u0, v0, u1, v1 (0..1, top-left origin).
    pub uv: [f32; 4],
    /// Size relative to the role's default (0 means 1).
    pub scale: f32,
    pub _pad: [u32; 9],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HudLayout {
    /// Seqlock word; bump through `Link::write_hud_layout`.
    pub seq: u32,
    /// Entries in use are `elements[..count]`.
    pub count: u32,
    pub flags: u32, // HUDL_*
    pub _pad: [u32; 13],
    pub elements: [HudElement; MAX_HUD_ELEMENTS],
}

impl HudLayout {
    pub fn entries(&self) -> &[HudElement] {
        &self.elements[..(self.count as usize).min(MAX_HUD_ELEMENTS)]
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
    assert!(size_of::<HudLayout>() == 64 + 64 * MAX_HUD_ELEMENTS);
    assert!(OFF_HUD_LAYOUT.is_multiple_of(64));
    assert!(crate::OFF_KICKS + size_of::<crate::KickTable>() <= OFF_HUD_LAYOUT);
    assert!(crate::OFF_SOUNDS + size_of::<crate::SoundPack>() <= OFF_HUD_LAYOUT);
    assert!(OFF_HUD_LAYOUT + size_of::<HudLayout>() <= crate::MAPPING_BYTES);
};
