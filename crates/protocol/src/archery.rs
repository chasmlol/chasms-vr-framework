//! Bows: what the object table can't say about a bow ([`crate::OBJ_BOW`]) and the arrows it
//! shoots ([`crate::OBJ_ARROW`]): its model bent at full draw, how its limbs spring back after a
//! shot, and how the game's own arrow flies, for the runtime to bend the bow as the string is drawn
//! and to show where an arrow will land. The shot itself is the game's: a [`crate::CE_LOOSE`]
//! combat event gives it the arrow's place, its way and the draw, and the game fires its own arrow.
//!
//! The adapter writes the table with its objects, when they change; a bow without an entry is
//! drawn straight and has no crosshair.

use bytemuck::{Pod, Zeroable};

/// High in the free space before the HUD layout, clear of the regions that grow from the combat
/// tables.
pub const OFF_BOWS: usize = 0x0051_8000;
pub const MAX_BOWS: usize = 8;
pub const BOW_SPRING_KEYS: usize = 16;

/// [`BowDesc::flags`]: the game plays the bow's own draw and release sounds (the runtime plays
/// none of its own for them).
pub const BOWF_GAME_SOUNDS: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BowDesc {
    /// The bow's [`crate::ObjectDesc::id`]; 0 marks an empty entry.
    pub id: u32,
    pub flags: u32, // BOWF_*
    /// Keys in use of `spring`.
    pub spring_count: u32,
    pub _pad: u32,
    /// The bow's model bent as the game bends it at its full draw, `<game>/<key>`: the same meshes
    /// and vertices, in the same order, as the model its object names. The runtime draws the bow
    /// that far between the two as its string is drawn of the bow's travel
    /// ([`crate::ObjectDesc::handle_travel`]), so the string is where the hand has it.
    pub bent: [u8; 32],
    /// How the limbs spring back after a shot, as the game animates it: `[seconds after the
    /// release, bend]` in time order (a bend as the draw's: 1 full, 0 at rest, below 0 past rest the
    /// other way), straight lines between; after the last, at rest. None: at rest at once.
    pub spring: [[f32; 2]; BOW_SPRING_KEYS],
    /// How the game's arrow flies loosed at no draw (`[0]`) and at the bow's full draw (`[1]`), in
    /// a straight line between with the draw: its speed (m/s) and the gravity pulling it down
    /// (m/s^2).
    pub speed: [f32; 2],
    pub gravity: [f32; 2],
    /// How far the game's arrow flies at most (metres; 0: as far as the crosshair looks).
    pub range: f32,
    /// The least draw that looses an arrow (share of the full draw; 0: the runtime's own).
    pub min_draw: f32,
    pub _p0: [f32; 2],
    /// The game's own crosshair, a texture in the runtime's asset cache (`<game>/<key>`; empty:
    /// none), shown where an arrow will land when the player asks for it.
    pub crosshair: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Bows {
    /// Seqlock word.
    pub seq: u32,
    /// Entries in use are `bows[..count]`.
    pub count: u32,
    pub _pad: [u32; 14],
    pub bows: [BowDesc; MAX_BOWS],
}

impl BowDesc {
    /// The limbs' bend `t` seconds after a shot ([`BowDesc::spring`]).
    pub fn spring_at(&self, t: f32) -> f32 {
        let k = &self.spring[..(self.spring_count as usize).min(BOW_SPRING_KEYS)];
        let Some(last) = k.last() else { return 0.0 };
        if t >= last[0] {
            return 0.0;
        }
        let i = k.partition_point(|x| x[0] <= t);
        if i == 0 {
            return k[0][1];
        }
        let (a, b) = (k[i - 1], k[i]);
        a[1] + (b[1] - a[1]) * ((t - a[0]) / (b[0] - a[0]).max(1e-6))
    }

    /// How the game's arrow flies at a draw of `draw` (0..1 of full): speed (m/s), gravity (m/s^2).
    pub fn flight(&self, draw: f32) -> (f32, f32) {
        let d = draw.clamp(0.0, 1.0);
        (self.speed[0] + (self.speed[1] - self.speed[0]) * d, self.gravity[0] + (self.gravity[1] - self.gravity[0]) * d)
    }
}

impl Bows {
    pub fn get(&self, id: u32) -> Option<&BowDesc> {
        if id == 0 {
            return None;
        }
        self.bows[..(self.count as usize).min(MAX_BOWS)].iter().find(|b| b.id == id)
    }
}

const _: () = {
    assert!(size_of::<BowDesc>() == 16 + 32 + 8 * BOW_SPRING_KEYS + 32 + 32);
    assert!(OFF_BOWS.is_multiple_of(64));
    assert!(crate::OFF_ITEM_INFO + size_of::<crate::ItemInfos>() <= OFF_BOWS);
    assert!(OFF_BOWS + size_of::<Bows>() <= crate::hud::OFF_HUD_LAYOUT);
};
