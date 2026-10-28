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
pub const MAX_AVATAR_SWAPS: usize = 16;

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

const _: () = {
    assert!(size_of::<AvatarDesc>() == 16 + 32 + 32 * MAX_AVATAR_PARTS + 32 * MAX_AVATAR_SWAPS);
    assert!(OFF_AVATAR + size_of::<AvatarDesc>() <= crate::OFF_HUD_LAYOUT);
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
