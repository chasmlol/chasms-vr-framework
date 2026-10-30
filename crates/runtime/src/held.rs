//! The place the player was in, held while the game loads another ([`SLOT_HELD`]), and the game's
//! own fade from it when the new place comes ([`SlotInfo::held_over`]).
//!
//! A game can't draw its world while it loads: the old place is gone, the new one isn't there yet.
//! Its adapter gives the last picture it drew of it again, with the eye poses, fields of view and
//! world mappings it was drawn with, and the runtime draws it through the warp (`crate::warp`) every
//! headset frame from where the head is now, by its depth: the place stays round the player,
//! leaning and turning look right, and the headset is never left without a picture of the game's.
//! It's held in tracking space: the player's walk isn't carried on over it, and the game moving its
//! player to the new place (a teleport, as the game sees a door) doesn't move it.
//!
//! The first held picture is copied into textures of the runtime's own: the game's next pictures (the
//! new place, drawn under the game's fade before it shows) come into the same slots. While the game
//! says how much of its fade there still is, the held picture is drawn that much over the new one:
//! where the flat game fades the new place in from black, the headset fades it in from the place the
//! player was in. With no fade said, it hands over in [`HANDOVER`] seconds.

use glam::Mat4;
use rustvr_gpu::texture::color_family;
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_FLOAT;

use crate::targets::Slot;
use crate::warp::WarpSource;

/// How long the place held hands over to the new one when the game says nothing of a fade (s).
pub const HANDOVER: f32 = 0.5;

/// The held picture: each eye's colour and depth, and how it was drawn.
struct Copy {
    eyes: [Texture; 2],
    depth: Option<[Texture; 2]>,
    info: SlotInfo,
}

#[derive(Default)]
pub struct HeldWorld {
    copy: Option<Copy>,
    /// The game has said how much of its fade there is since the place was held.
    faded: bool,
    /// How much of the held place still shows, handing over by time.
    left: f32,
}

impl HeldWorld {
    /// A picture of the place to hold is shown (its slot locked): copied the first time it's seen.
    pub fn keep(&mut self, gpu: &Gpu, slot: &Slot, info: &SlotInfo) {
        self.faded = false;
        self.left = 1.0;
        let same = self.copy.as_ref().is_some_and(|c| bytemuck::bytes_of(&c.info.eye_pose) == bytemuck::bytes_of(&info.eye_pose) && c.eyes[0].width == slot.eyes[0].width && c.eyes[0].height == slot.eyes[0].height);
        if same {
            return;
        }
        let made = (|| -> anyhow::Result<Copy> {
            let (w, h) = (slot.eyes[0].width, slot.eyes[0].height);
            // (Reused when it's the same size: a load after a load.)
            let reuse = self.copy.take().filter(|c| (c.eyes[0].width, c.eyes[0].height) == (w, h) && c.eyes[0].format == slot.eyes[0].format);
            let (eyes, depth) = match reuse {
                Some(c) => (c.eyes, c.depth),
                None => {
                    let (typeless, _, srgb) = color_family(slot.eyes[0].format);
                    let colour = TexDesc { width: w, height: h, format: typeless, rtv: None, srv: Some(srgb), dsv: None, mips: 1 };
                    let eyes = [Texture::new(gpu, &colour)?, Texture::new(gpu, &colour)?];
                    let depth = match slot.depth.as_ref() {
                        Some(d) => {
                            let desc = TexDesc::target(d[0].width, d[0].height, DXGI_FORMAT_R32_FLOAT);
                            Some([Texture::new(gpu, &desc)?, Texture::new(gpu, &desc)?])
                        }
                        None => None,
                    };
                    (eyes, depth)
                }
            };
            unsafe {
                for e in 0..2 {
                    gpu.ctx.CopyResource(&eyes[e].tex, &slot.eyes[e].tex);
                    if let (Some(d), Some(from)) = (depth.as_ref(), slot.depth.as_ref()) {
                        gpu.ctx.CopyResource(&d[e].tex, &from[e].tex);
                    }
                }
            }
            Ok(Copy { eyes, depth, info: *info })
        })();
        match made {
            Ok(c) => {
                tracing::info!("runtime: the place the player was in is held while the game loads ({}x{} an eye{})", c.eyes[0].width, c.eyes[0].height, if info.flags & SLOT_DEPTH != 0 { ", with its depth" } else { "" });
                self.copy = Some(c);
            }
            Err(e) => tracing::warn!("runtime: holding the game's last picture: {e:#}"),
        }
    }

    /// There's a held place to show.
    pub fn has(&self) -> bool {
        self.copy.is_some()
    }

    /// How much of the held place to draw over the game's live picture `info` this frame (0: none,
    /// and it's let go of once the handover is done).
    pub fn over(&mut self, info: &SlotInfo, dt: f32) -> f32 {
        if self.copy.is_none() {
            return 0.0;
        }
        let said = info.held_over.clamp(0.0, 1.0);
        if said > 0.0 {
            self.faded = true;
            self.left = said;
            return said;
        }
        // (The game's fade done, or none said: by time, from where it was.)
        if !self.faded {
            self.left -= dt / HANDOVER;
        } else {
            self.left = 0.0;
        }
        if self.left <= 0.0 {
            tracing::info!("runtime: the new place has taken over from the one held");
            self.copy = None;
            self.faded = false;
            return 0.0;
        }
        self.left
    }

    /// Eye `k` of the held picture for the warp: drawn from where that eye was, fixed in tracking
    /// space (`wpm`: the world's metres a tracking metre when its mapping isn't said).
    pub fn source(&self, k: usize, wpm: f32) -> Option<WarpSource<'_>> {
        let c = self.copy.as_ref()?;
        let then = c.info.eye_map[k];
        let per = if then.world_per_meter > 0.0 { then.world_per_meter } else { wpm };
        Some(WarpSource {
            color: c.eyes[k].srv.as_ref()?,
            depth: c.depth.as_ref().filter(|_| c.info.flags & SLOT_DEPTH != 0).and_then(|d| d[k].srv.as_ref()),
            bloom: None,
            lens: None,
            width: c.eyes[k].width,
            height: c.eyes[k].height,
            pose: c.info.eye_pose[k],
            fov: c.info.fov[k],
            to_target: Mat4::IDENTITY,
            metres_per_unit: 1.0 / per.max(1e-6),
        })
    }
}
