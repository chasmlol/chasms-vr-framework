//! The player as the runtime posed it for a frame, for an adapter that draws the player inside the
//! game's own frame.
//!
//! This is SkyCraft's way of putting its player into Skyrim: Minecraft owns the player and sends
//! its posed body every frame (`RenAvatar`), and Skyrim's plugin draws it with Skyrim's camera into
//! Skyrim's scene, before Skyrim's post-processing, so the game's own fog, bloom, tone curve and
//! grading treat it as one of its own objects. Here the runtime owns the player (body, hands, what
//! they hold) and writes [`RtPlayerDraw`] every step; an adapter that sets [`crate::GS_DRAWS_BODY`]
//! draws it (with `rustvr_avatar`, from the same assets) into the game's HDR picture and depth, and
//! the runtime draws no body of its own.
//!
//! Everything is in tracking space, as [`RtBody`] is; the adapter draws it from the eye the game
//! frame was rendered for.

use bytemuck::{Pod, Zeroable};

use crate::{Pose, RtBody};

pub const OFF_PLAYER_DRAW: usize = 0x004C_0000;
/// Objects drawn with the player at once (a gun, its magazines).
pub const DRAWN_OBJECTS_MAX: usize = 6;
/// Moving parts per drawn object (a gun's body, trigger, bolt, rounds, glass...).
pub const DRAWN_PARTS_MAX: usize = 128;

/// [`RtPlayerDraw::flags`]: a posed player is there.
pub const PD_VALID: u32 = 1 << 0;

/// [`DrawnHand::flags`]: the hand is drawn in a game's own pose for what it holds.
pub const DH_HELD: u32 = 1 << 0;
/// [`DrawnHand::index_pulled`] holds the index finger at a full trigger pull.
pub const DH_INDEX_PULLED: u32 = 1 << 1;
/// [`DrawnHand::index_reach_pulled`] is given.
pub const DH_INDEX_REACH_PULLED: u32 = 1 << 2;

/// A hand drawn in a game's own pose (the avatar's `HeldHand`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DrawnHand {
    pub flags: u32, // DH_*
    /// Bit k: `reach[k]` is given.
    pub reach_mask: u32,
    /// How far the trigger is pulled, 0..1.
    pub trigger: f32,
    /// The game's hand size (metres); 0 keeps the avatar's.
    pub size: f32,
    /// The hand joint's canonical frame.
    pub wrist: Pose,
    /// Finger segment rotations relative to the hand frame (quaternions, x y z w).
    pub fingers: [[f32; 4]; 15],
    pub index_pulled: [[f32; 4]; 3],
    /// Where each finger's last segment reaches in the hand's frame (xyz), and how far along it (w).
    pub reach: [[f32; 4]; 5],
    pub index_reach_pulled: [f32; 4],
}

/// An object drawn with the player: its model and each part's place.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DrawnObject {
    /// The model, `<game>/<asset key>` (as [`crate::ObjectDesc::name`] gives it), NUL-padded.
    pub name: [u8; 32],
    pub part_count: u32,
    pub _pad: [u32; 3],
    /// Bit k: part k is drawn.
    pub visible: [u32; DRAWN_PARTS_MAX / 32],
    /// Each part's model space -> tracking space, as the first three rows of the matrix.
    pub parts: [[[f32; 4]; 3]; DRAWN_PARTS_MAX],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RtPlayerDraw {
    /// Seqlock word.
    pub seq: u32,
    pub flags: u32, // PD_*
    /// The runtime frame ([`crate::RtFrame::frame_index`]) the player was posed for.
    pub frame_index: u64,
    pub body: RtBody,
    /// Controller poses to draw as ghost hands (`valid` 0: none).
    pub ghosts: [Pose; 2],
    pub hands: [DrawnHand; 2],
    pub object_count: u32,
    pub _pad: [u32; 3],
    pub objects: [DrawnObject; DRAWN_OBJECTS_MAX],
}

impl Default for RtPlayerDraw {
    fn default() -> Self {
        Zeroable::zeroed()
    }
}

impl DrawnObject {
    pub fn set_part(&mut self, k: usize, m: Option<[[f32; 4]; 3]>) {
        if k >= DRAWN_PARTS_MAX {
            return;
        }
        let (w, b) = (k / 32, 1u32 << (k % 32));
        match m {
            Some(rows) => {
                self.parts[k] = rows;
                self.visible[w] |= b;
            }
            None => self.visible[w] &= !b,
        }
    }

    pub fn part(&self, k: usize) -> Option<[[f32; 4]; 3]> {
        (k < DRAWN_PARTS_MAX && self.visible[k / 32] & (1 << (k % 32)) != 0).then(|| self.parts[k])
    }
}

const _: () = {
    assert!(size_of::<DrawnHand>() == 16 + 32 + 16 * (15 + 3 + 5 + 1));
    assert!(OFF_PLAYER_DRAW.is_multiple_of(64));
    assert!(OFF_PLAYER_DRAW + size_of::<RtPlayerDraw>() <= crate::hud::OFF_HUD_LAYOUT);
    assert!(crate::lighting::OFF_GAME_LIGHTING + size_of::<crate::GameLighting>() <= OFF_PLAYER_DRAW);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_keep_their_visibility() {
        let mut o: DrawnObject = Zeroable::zeroed();
        let m = [[1.0, 0.0, 0.0, 0.5], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        o.set_part(0, Some(m));
        o.set_part(77, Some(m));
        o.set_part(DRAWN_PARTS_MAX, Some(m));
        assert_eq!(o.part(0), Some(m));
        assert_eq!(o.part(77), Some(m));
        assert_eq!(o.part(1), None);
        o.set_part(77, None);
        assert_eq!(o.part(77), None);
    }
}
