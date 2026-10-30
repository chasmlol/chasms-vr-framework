//! The player's body as the game makes it up: the models (from an asset cache, `<game>/<key>`) the
//! runtime's avatar is put together from, for a game whose player is made of parts: a race's skin,
//! the armour and clothes it wears. Every part is skinned to the same skeleton (the first model
//! named, whose bones are the body's), so the avatar is their meshes side by side; a texture swap
//! puts the race's own skin on parts made with a generic one.
//!
//! The adapter writes it when the body changes (a new race, something put on or taken off); the
//! runtime builds the new avatar off the frame and swaps it in. Nothing written: the runtime's own
//! body.

use bytemuck::{Pod, Zeroable};

pub const OFF_AVATAR: usize = crate::OFF_COMBAT_END.next_multiple_of(64);
pub const MAX_AVATAR_PARTS: usize = 24;
pub const MAX_AVATAR_SWAPS: usize = 32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct AvatarDesc {
    /// Seqlock word (each write is a new body).
    pub seq: u32,
    pub count: u32,
    pub swap_count: u32,
    /// The body's height over the skeleton's as modelled (a race's scale), 0 or 1: as modelled.
    pub scale: f32,
    /// The skeleton every part is skinned to, `<game>/<key>` (UTF-8, NUL-terminated).
    pub skeleton: [u8; 32],
    /// The parts, `<game>/<key>`, each skinned to the skeleton's bones in its order.
    pub parts: [[u8; 32]; MAX_AVATAR_PARTS],
    /// Texture swaps: (the key a part's mesh names, the key drawn instead).
    pub swaps: [[[u8; 16]; 2]; MAX_AVATAR_SWAPS],
}

impl AvatarDesc {
    pub fn part(&self, i: usize) -> &str {
        crate::name_str(&self.parts[i])
    }

    pub fn swap(&self, i: usize) -> (&str, &str) {
        (crate::name_str(&self.swaps[i][0]), crate::name_str(&self.swaps[i][1]))
    }

    /// Adds a part (false when full).
    pub fn push_part(&mut self, name: &str) -> bool {
        let i = self.count as usize;
        if i >= MAX_AVATAR_PARTS {
            return false;
        }
        crate::set_name(&mut self.parts[i], name);
        self.count += 1;
        true
    }

    /// Adds a texture swap (false when full).
    pub fn push_swap(&mut self, from: &str, to: &str) -> bool {
        let i = self.swap_count as usize;
        if i >= MAX_AVATAR_SWAPS {
            return false;
        }
        crate::set_name(&mut self.swaps[i][0], from);
        crate::set_name(&mut self.swaps[i][1], to);
        self.swap_count += 1;
        true
    }
}

/// The player's face, for the parts of the body that have face morphs (a FaceGen head and the
/// parts made to its shape: ears, eyes, mouth, teeth, hair; the cache's `morphs/<key>.rvf`): where
/// it is in FaceGen's face space, the detail map laid over its skin, and its hair's colour.
///
/// The adapter writes it whenever it changes (a slider in the game's character creator moved); the
/// runtime moves the morphed parts' vertices and remakes the face's texture off the frame, without
/// putting the body together again. Nothing written: the parts as modelled.
pub const OFF_FACE: usize = (crate::OFF_ITEM_INFO + size_of::<crate::ItemInfos>()).next_multiple_of(64);
pub const FACE_GEOMETRY_SYMMETRIC: usize = 50;
pub const FACE_GEOMETRY_ASYMMETRIC: usize = 30;
pub const FACE_TEXTURE_SYMMETRIC: usize = 50;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FaceDesc {
    /// Seqlock word (each write is a new face).
    pub seq: u32,
    pub flags: u32, // FACE_*
    /// The face's coordinates: the symmetric and asymmetric geometry, the symmetric texture.
    pub geometry_symmetric: [f32; FACE_GEOMETRY_SYMMETRIC],
    pub geometry_asymmetric: [f32; FACE_GEOMETRY_ASYMMETRIC],
    pub texture_symmetric: [f32; FACE_TEXTURE_SYMMETRIC],
    /// The hair's tint as the game's hair shader takes it (display 0..1 rgb; a unused).
    pub hair_color: [f32; 4],
    /// The face's detail map (Oblivion's age map, `4 · detail · colour` over the skin's colour), on
    /// the meshes whose base texture is `detail_on`, with the normal map it brings (drawn instead
    /// of theirs). Texture keys; empty: none.
    pub detail_on: [u8; 16],
    pub detail: [u8; 16],
    pub detail_normal: [u8; 16],
    /// The parts that are the eyes (left, right; `<game>/<key>`, empty: none): each turns as one
    /// about its centre, to look where the eyes look (a mirror's reflection looks back at you).
    pub eyes: [[u8; 32]; 2],
}

/// [`FaceDesc`] holds a face (else the parts are drawn as modelled).
pub const FACE_GIVEN: u32 = 1 << 0;

impl FaceDesc {
    pub fn detail(&self) -> (&str, &str, &str) {
        (crate::name_str(&self.detail_on), crate::name_str(&self.detail), crate::name_str(&self.detail_normal))
    }

    pub fn eye(&self, side: usize) -> &str {
        crate::name_str(&self.eyes[side])
    }
}

const _: () = {
    assert!(size_of::<AvatarDesc>() == 16 + 32 + 32 * MAX_AVATAR_PARTS + 32 * MAX_AVATAR_SWAPS);
    assert!(OFF_AVATAR + size_of::<AvatarDesc>() <= crate::OFF_HUD_LAYOUT);
    assert!(OFF_FACE + size_of::<FaceDesc>() <= crate::OFF_HUD_LAYOUT);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_and_swaps_read_back() {
        let mut d: AvatarDesc = Zeroable::zeroed();
        crate::set_name(&mut d.skeleton, "oblivion/m00000001");
        assert!(d.push_part("oblivion/m12345678"));
        assert!(d.push_swap("t00000001", "t00000002"));
        assert_eq!(d.part(0), "oblivion/m12345678");
        assert_eq!(d.swap(0), ("t00000001", "t00000002"));
        for _ in 1..MAX_AVATAR_PARTS {
            assert!(d.push_part("x"));
        }
        assert!(!d.push_part("one too many"));
    }
}
