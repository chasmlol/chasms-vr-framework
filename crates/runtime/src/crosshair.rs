//! The bow's crosshair (the Hands tab's option, off by default; the acclaimed VR bows have none,
//! and the arrow already leaves exactly along the drawn arrow): the game's own crosshair image
//! where a drawn arrow would land if it were loosed now. Its flight is the game's: from the arrow's
//! tip along the bow, at the speed and under the gravity the game gives its arrow at this draw
//! ([`BowDesc::flight`]), traced against the world's collision and the creatures out to the game's
//! range. It faces the eyes and stays the size the game's own crosshair is in view, at any
//! distance; it fades in as the string is drawn and out as it's let go.

use anyhow::Result;
use glam::{Quat, Vec3};
use rustvr_gpu::{Gpu, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::panels::Panel;
use crate::rig::{Rig, WorldMap};

/// Its size in view (radians across): the flat game's crosshair, 37 of 1280 pixels of a 91 degree
/// view, is about this.
const SIZE: f32 = 0.042;
/// Seconds of flight per step of the trace, and the longest flight traced.
const STEP: f32 = 1.0 / 60.0;
const LONGEST: f32 = 6.0;
/// How far it's traced when the game gives no range (metres).
const RANGE: f32 = 150.0;
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

/// Where an arrow loosed from `from` along `dir` at `speed` (m/s) under `gravity` (m/s^2, down)
/// lands within `range` metres: the first point of its flight in `hit` (a segment of the flight:
/// how far along it the arrow first meets something, 0..1).
pub fn landing(from: Vec3, dir: Vec3, speed: f32, gravity: f32, range: f32, hit: impl Fn(Vec3, Vec3) -> Option<f32>) -> Option<Vec3> {
    if speed.is_nan() || speed <= 0.1 {
        return None;
    }
    let (mut p, mut v) = (from, dir.normalize_or(Vec3::Z) * speed);
    let (mut t, mut flown) = (0.0, 0.0);
    while t < LONGEST && flown < range {
        let next = p + v * STEP + Vec3::NEG_Y * (0.5 * gravity * STEP * STEP);
        if let Some(k) = hit(p, next) {
            return Some(p.lerp(next, k));
        }
        flown += p.distance(next);
        v += Vec3::NEG_Y * gravity * STEP;
        p = next;
        t += STEP;
    }
    None
}

/// The first point along the segment `a`..`b` (0..1) inside a capsule (`p`..`q`, `r` thick).
fn capsule_hit(a: Vec3, b: Vec3, p: Vec3, q: Vec3, r: f32) -> Option<f32> {
    let (s, t) = crate::combat::closest_segments(a, b, p, q);
    let (x, y) = (a.lerp(b, s), p.lerp(q, t));
    (x.distance(y) < r).then_some(s)
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
    /// draws the string of a held bow the game describes, where its arrow would land. `head`: the
    /// headset's pose (tracking space).
    pub fn update(&mut self, gpu: &Gpu, on: bool, rig: &Rig, bows: &Bows, map: Option<&WorldMap>, head: (Vec3, Quat), dt: f32) -> Option<(Panel, f32)> {
        let drawn = rig.drawn_arrow().filter(|_| on);
        let land = match (drawn, map) {
            (Some((bow, tip, dir, strength)), Some(map)) => bows.get(bow).and_then(|b| {
                let name = name_str(&b.crosshair);
                if name.is_empty() {
                    return None;
                }
                if let Err(e) = self.load(gpu, name) {
                    tracing::warn!("runtime: no crosshair for the bow: {e:#}");
                }
                let (speed, gravity) = b.flight(strength);
                let range = if b.range > 1.0 { b.range } else { RANGE };
                let parts = rig.actor_capsules();
                let hit = |a: Vec3, c: Vec3| {
                    let len = a.distance(c);
                    let world = rig.world_hit(a, c - a, len).map(|h| h.distance / len.max(1e-6));
                    let actor = parts.iter().filter_map(|&(p, q, r)| capsule_hit(a, c, p, q, r)).reduce(f32::min);
                    match (world, actor) {
                        (Some(w), Some(a)) => Some(w.min(a)),
                        (w, a) => w.or(a),
                    }
                };
                landing(tip, dir, speed, gravity, range, hit).map(|p| map.inv_point(p))
            }),
            _ => None,
        };
        let step = dt / FADE;
        self.opacity = if land.is_some() { (self.opacity + step).min(1.0) } else { (self.opacity - step).max(0.0) };
        if land.is_some() {
            self.at = land;
        }
        let at = self.at.filter(|_| self.opacity > 0.0 && self.srv().is_some())?;
        // Facing the eyes, upright as the head is, a touch nearer than the surface it's on, the
        // size in view the game's own is.
        let (eye, rot) = head;
        let to = at - eye;
        let dist = to.length().max(0.2);
        let fwd = to / dist;
        let right = (rot * Vec3::Y).cross(fwd).normalize_or(rot * Vec3::X);
        let up = fwd.cross(right).normalize_or(rot * Vec3::Y);
        let near = dist - (0.02 * dist).clamp(0.01, 0.3);
        let width = 2.0 * near * (SIZE * 0.5).tan();
        let panel = Panel { center: eye + fwd * near, right, up, width, height: width };
        Some((panel, self.opacity))
    }
}
