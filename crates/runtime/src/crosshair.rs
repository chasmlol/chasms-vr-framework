//! The bow's crosshair (the Hands tab's option, off by default; the acclaimed VR bows have none,
//! and the arrow already leaves exactly along the drawn arrow): the game's own crosshair image
//! straight out along a drawn arrow, as the flat game's crosshair shows where the view points. It's
//! a guide to where the bow points, not where the arrow lands: it leaves out the arrow's drop, and
//! it's never put on what's in the way, so it sits at the same distance out whatever is there. It
//! faces the eyes, half the size in view the game's own crosshair is and half seen through, and it
//! fades in as the string is drawn and out as it's let go.

use anyhow::Result;
use glam::{Quat, Vec3};
use rustvr_gpu::{Gpu, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::panels::Panel;
use crate::rig::{Rig, WorldMap};

/// Its size in view (radians across): half the flat game's crosshair, which is 37 of 1280 pixels
/// of a 91 degree view.
const SIZE: f32 = 0.021;
/// How strongly it shows at most: half seen through, a guide rather than a sight.
const OPACITY: f32 = 0.5;
/// How far out along the arrow it sits (metres of the player's space). Its size in view is the
/// same at any distance, so this only sets where the eyes meet on it: far enough out that it
/// reads as the bow's direction, and its parallax with the eyes is a few centimetres at a shot's
/// distance.
const DISTANCE: f32 = 20.0;
const FADE: f32 = 0.1;

#[derive(Default)]
pub struct Crosshair {
    /// The crosshair image and the key it was loaded from.
    tex: Option<(String, Texture)>,
    failed: Option<String>,
    opacity: f32,
    /// Where it was last shown (tracking space), kept while it fades out.
    at: Option<Vec3>,
}

/// An asset texture's format as Direct3D reads it (colour in sRGB).
fn dxgi(f: rustvr_asset::TexFormat) -> DXGI_FORMAT {
    use rustvr_asset::TexFormat as F;
    match f {
        F::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        F::Bc1 => DXGI_FORMAT_BC1_UNORM_SRGB,
        F::Bc2 => DXGI_FORMAT_BC2_UNORM_SRGB,
        F::Bc3 => DXGI_FORMAT_BC3_UNORM_SRGB,
        F::Bc4 => DXGI_FORMAT_BC4_UNORM,
        F::Bc5 => DXGI_FORMAT_BC5_UNORM,
        F::Bc7 => DXGI_FORMAT_BC7_UNORM_SRGB,
    }
}

impl Crosshair {
    pub fn srv(&self) -> Option<&ID3D11ShaderResourceView> {
        self.tex.as_ref().and_then(|(_, t)| t.srv.as_ref())
    }

    /// The game's crosshair image `name` (`<game>/<key>`), loaded once.
    fn load(&mut self, gpu: &Gpu, name: &str) -> Result<()> {
        if self.tex.as_ref().is_some_and(|(n, _)| n == name) || self.failed.as_deref() == Some(name) {
            return Ok(());
        }
        let (game, key) = rustvr_guns::desc::parse_model_name(name).ok_or_else(|| anyhow::anyhow!("{name:?} isn't <game>/<key>"))?;
        let t = match rustvr_asset::load_texture(&rustvr_asset::default_root().join(game), key) {
            Ok(t) => t,
            Err(e) => {
                self.failed = Some(name.to_string());
                return Err(e);
            }
        };
        let mips: Vec<&[u8]> = t.mips.iter().map(Vec::as_slice).collect();
        let tex = Texture::upload(gpu, t.width, t.height, dxgi(t.format), &mips)?;
        tracing::info!("runtime: the bow's crosshair is {name} ({}x{})", t.width, t.height);
        self.tex = Some((name.to_string(), tex));
        Ok(())
    }

    /// This frame's crosshair panel (tracking space) and how strongly it shows, if any: while a hand
    /// draws the string of a held bow the game describes, straight out along its arrow. `head`: the
    /// headset's pose (tracking space).
    pub fn update(&mut self, gpu: &Gpu, on: bool, rig: &Rig, bows: &Bows, map: Option<&WorldMap>, head: (Vec3, Quat), dt: f32) -> Option<(Panel, f32)> {
        let drawn = rig.drawn_arrow().filter(|_| on);
        let ahead = match (drawn, map) {
            (Some((bow, tip, dir, _)), Some(map)) => bows.get(bow).and_then(|b| {
                let name = name_str(&b.crosshair);
                if name.is_empty() {
                    return None;
                }
                if let Err(e) = self.load(gpu, name) {
                    tracing::warn!("runtime: no crosshair for the bow: {e:#}");
                }
                // (The arrow is in the world's space, and the distance is the player's.)
                Some(map.inv_point(tip + dir.normalize_or(Vec3::Z) * DISTANCE * map.world_per_meter))
            }),
            _ => None,
        };
        let step = OPACITY * dt / FADE;
        self.opacity = if ahead.is_some() { (self.opacity + step).min(OPACITY) } else { (self.opacity - step).max(0.0) };
        if ahead.is_some() {
            self.at = ahead;
        }
        let at = self.at.filter(|_| self.opacity > 0.0 && self.srv().is_some())?;
        // Facing the eyes, upright as the head is, the size in view it's meant to be.
        let (eye, rot) = head;
        let to = at - eye;
        let dist = to.length().max(0.2);
        let fwd = to / dist;
        let right = (rot * Vec3::Y).cross(fwd).normalize_or(rot * Vec3::X);
        let up = fwd.cross(right).normalize_or(rot * Vec3::Y);
        let width = 2.0 * dist * (SIZE * 0.5).tan();
        let panel = Panel { center: eye + fwd * dist, right, up, width, height: width };
        Some((panel, self.opacity))
    }
}
