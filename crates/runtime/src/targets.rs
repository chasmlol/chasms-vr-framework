//! The shared textures adapters render into: created here, opened by name in the game.

use anyhow::Result;
use rustvr_gpu::texture::color_family;
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::link::Security;
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM};

use crate::settings::Settings;

/// What to create; compared with the current one to decide whether to recreate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub eye_w: u32,
    pub eye_h: u32,
    /// TYPELESS storage format.
    pub color_format: i32,
    pub flags: u32,
    pub hud_w: u32,
    pub hud_h: u32,
    pub aux_w: u32,
    pub aux_h: u32,
}

/// The most overscan there is (degrees). The eye grows fast with it, the outer edge of a wide
/// headset's eye nearing 90 degrees: on a Quest through Virtual Desktop (54 degrees out) 11 is a
/// 4669x5026 eye, 20 a 7249x7898 one (2.4 times the pixels), 30 an 11673x12468 one (6.2 times),
/// for an edge a 128 degree desktop view only reaches in a fast turn (it needs about 10 still, 18
/// behind a turning head).
pub const MAX_OVERSCAN: f32 = 20.0;

/// The field of view the game draws each eye with, from the headset's (tangents, as [`Fov`] has
/// them): `overscan` degrees wider all round (the setting, `vr.overscan`). The headset turns each
/// picture to where the head is when it shows it, a frame or two on (more over a stream, Virtual
/// Desktop's): without some, a turning head sees round the picture's edge, undrawn there, the edges
/// flickering and smearing as it turns. More lets the desktop view go wider on the aiming eye alone.
pub fn render_fov(headset: [Fov; 2], overscan: f32) -> [Fov; 2] {
    let m = overscan.clamp(0.0, MAX_OVERSCAN).to_radians();
    // (`sign`: the side's: left and down are negative tangents.)
    let wider = |t: f32, sign: f32| sign * ((sign * t).atan() + m).min(80f32.to_radians()).tan();
    headset.map(|f| Fov { left: wider(f.left, -1.0), right: wider(f.right, 1.0), up: wider(f.up, 1.0), down: wider(f.down, -1.0) })
}

/// How much wider and taller (as tangent spans) the game's eyes are drawn than the headset's view:
/// the eye size grows with them ([`eye_size`]), so the picture stays as sharp.
pub fn render_scale(headset: [Fov; 2], drawn: [Fov; 2]) -> (f32, f32) {
    let k = |e: usize| {
        let (h, d) = (headset[e], drawn[e]);
        ((d.right - d.left) / (h.right - h.left).max(1e-3), (d.up - d.down) / (h.up - h.down).max(1e-3))
    };
    let (a, b) = (k(0), k(1));
    // (Rounded, so the size doesn't change with every frame's tiny differences.)
    let r = |x: f32| (x.max(1.0) * 50.0).round() / 50.0;
    (r(a.0.max(b.0)), r(a.1.max(b.1)))
}

/// How much wider and taller (tangent spans) `drawn` is than `headset`, unrounded: both eyes'
/// most.
fn widening(headset: [Fov; 2], drawn: [Fov; 2]) -> (f32, f32) {
    let k = |e: usize| {
        let (h, d) = (headset[e], drawn[e]);
        ((d.right - d.left) / (h.right - h.left).max(1e-3), (d.up - d.down) / (h.up - h.down).max(1e-3))
    };
    let (a, b) = (k(0), k(1));
    (a.0.max(b.0), a.1.max(b.1))
}

/// The overscan a game drawing at a size it can't change (`room`: its eyes' size over the size the
/// headset's view wants at the supersampling, each way) shows at full sharpness: `wanted`, or as
/// much of it as its pixels go round (none when they don't go round the headset's own view).
/// The rest comes when the game starts again at the size [`eye_size`] asks for.
pub fn fitting_overscan(headset: [Fov; 2], wanted: f32, room: (f32, f32)) -> f32 {
    let fits = |o: f32| {
        let (w, h) = widening(headset, render_fov(headset, o));
        w <= room.0 + 1e-3 && h <= room.1 + 1e-3
    };
    if wanted <= 0.0 || fits(wanted) {
        return wanted.max(0.0);
    }
    let (mut lo, mut hi) = (0.0f32, wanted);
    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        if fits(mid) { lo = mid } else { hi = mid }
    }
    lo
}

/// The longest side a game's eye can be (pixels): Direct3D 11's largest texture.
pub const MAX_SIDE: u32 = 16384;

/// The per-eye resolution games render at: the headset's own (its recommended size) times the
/// supersampling (`vr.render_scale`, each way), grown by `wider` (how much wider and taller the
/// game draws than the headset shows, overscanned: [`render_scale`]) so the overscan adds pixels
/// round the edges at the same sharpness. Nothing else caps it but the largest texture there is.
pub fn eye_size(recommended: (u32, u32), wider: (f32, f32), s: &Settings) -> (u32, u32) {
    let scale = s.vr.render_scale.clamp(0.25, 3.0);
    let (mut w, mut h) = (recommended.0 as f32 * scale * wider.0.max(1.0), recommended.1 as f32 * scale * wider.1.max(1.0));
    if w.max(h) > MAX_SIDE as f32 {
        let k = MAX_SIDE as f32 / w.max(h);
        (w, h) = (w * k, h * k);
    }
    ((w as u32).max(16), (h as u32).max(16))
}

impl Spec {
    /// From the adapter's request, the headset's recommended size, how much wider than the headset
    /// the eyes are drawn ([`render_scale`]) and the settings.
    pub fn new(req: &TargetRequest, recommended: (u32, u32), wider: (f32, f32), s: &Settings) -> Spec {
        let (eye_w, eye_h) = if req.eye_w > 0 && req.eye_h > 0 { (req.eye_w, req.eye_h) } else { eye_size(recommended, wider, s) };
        let fmt = if req.color_format == 0 { DXGI_FORMAT_R8G8B8A8_UNORM } else { DXGI_FORMAT(req.color_format as i32) };
        let (typeless, _, _) = color_family(fmt);
        let (aux_w, aux_h) = if req.aux_size > 0 { (req.aux_size, req.aux_size) } else { (eye_w, eye_h) };
        Spec {
            eye_w,
            eye_h,
            color_format: typeless.0,
            flags: req.flags & (TGT_DEPTH | TGT_HUD | TGT_AUX | TGT_LIGHTS | TGT_BLOOM | TGT_AUX_DEPTH),
            hud_w: if req.hud_w > 0 { req.hud_w } else { 1920 },
            hud_h: if req.hud_h > 0 { req.hud_h } else { 1080 },
            aux_w,
            aux_h,
        }
    }
}

pub struct Slot {
    pub eyes: [Texture; 2],
    pub depth: Option<[Texture; 2]>,
    pub hud: Option<Texture>,
    pub aux: Option<Texture>,
    /// The aux view's depth (linear metres: [`TGT_AUX_DEPTH`]).
    pub aux_depth: Option<Texture>,
    pub lights: Option<Texture>,
    pub bloom: Option<[Texture; 2]>,
    pub lens: Option<[Texture; 2]>,
}

impl Slot {
    fn all(&self) -> impl Iterator<Item = &Texture> {
        self.eyes
            .iter()
            .chain(self.depth.iter().flatten())
            .chain(self.hud.iter())
            .chain(self.aux.iter())
            .chain(self.aux_depth.iter())
            .chain(self.lights.iter())
            .chain(self.bloom.iter().flatten())
            .chain(self.lens.iter().flatten())
    }

    /// Takes every texture's keyed mutex; on failure releases what it took.
    pub fn acquire(&self, timeout_ms: u32) -> bool {
        let all: Vec<&Texture> = self.all().collect();
        for (i, t) in all.iter().enumerate() {
            if !t.acquire(timeout_ms) {
                for t in &all[..i] {
                    t.release();
                }
                return false;
            }
        }
        true
    }

    pub fn release(&self) {
        for t in self.all() {
            t.release();
        }
    }
}

pub struct SharedTargets {
    pub spec: Spec,
    pub generation: u32,
    pub slots: Vec<Slot>,
}

impl SharedTargets {
    pub fn create(gpu: &Gpu, mapping: &str, generation: u32, spec: Spec, sec: Option<&Security>) -> Result<SharedTargets> {
        let sa = sec.map(|s| s.attributes());
        let (typeless, unorm, srgb) = color_family(DXGI_FORMAT(spec.color_format));
        let color = |w, h| TexDesc { width: w, height: h, format: typeless, rtv: Some(unorm), srv: Some(srgb), dsv: None, mips: 1 };
        let make = |kind: TexKind, slot: u32, desc: TexDesc| {
            Texture::new_shared(gpu, &desc, &texture_name(mapping, generation, kind, slot), sa.as_ref())
        };
        let mut slots = Vec::new();
        for s in 0..SLOT_COUNT as u32 {
            let eyes = [make(TexKind::EyeLeft, s, color(spec.eye_w, spec.eye_h))?, make(TexKind::EyeRight, s, color(spec.eye_w, spec.eye_h))?];
            let depth = if spec.flags & TGT_DEPTH != 0 {
                let d = TexDesc::target(spec.eye_w, spec.eye_h, DXGI_FORMAT_R32_FLOAT);
                Some([make(TexKind::DepthLeft, s, d)?, make(TexKind::DepthRight, s, d)?])
            } else {
                None
            };
            let hud = if spec.flags & TGT_HUD != 0 { Some(make(TexKind::Hud, s, color(spec.hud_w, spec.hud_h))?) } else { None };
            let aux = if spec.flags & TGT_AUX != 0 { Some(make(TexKind::Aux, s, color(spec.aux_w, spec.aux_h))?) } else { None };
            let aux_depth = if spec.flags & TGT_AUX_DEPTH != 0 { Some(make(TexKind::AuxDepth, s, TexDesc::target(spec.aux_w, spec.aux_h, DXGI_FORMAT_R32_FLOAT))?) } else { None };
            let lights = if spec.flags & TGT_LIGHTS != 0 {
                let d = TexDesc { width: LIGHT_ATLAS_W, height: LIGHT_ATLAS_H, format: DXGI_FORMAT_R8G8B8A8_UNORM, rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM), srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM), dsv: None, mips: 1 };
                Some(make(TexKind::Lights, s, d)?)
            } else {
                None
            };
            let (bloom, lens) = if spec.flags & TGT_BLOOM != 0 {
                let (w, h) = bloom_size(spec.eye_w, spec.eye_h);
                let d = TexDesc::target(w, h, DXGI_FORMAT_R16G16B16A16_FLOAT);
                (
                    Some([make(TexKind::BloomLeft, s, d)?, make(TexKind::BloomRight, s, d)?]),
                    Some([make(TexKind::LensLeft, s, d)?, make(TexKind::LensRight, s, d)?]),
                )
            } else {
                (None, None)
            };
            slots.push(Slot { eyes, depth, hud, aux, aux_depth, lights, bloom, lens });
        }
        Ok(SharedTargets { spec, generation, slots })
    }

    pub fn info(&self) -> Targets {
        Targets {
            seq: 0,
            generation: self.generation,
            eye_w: self.spec.eye_w,
            eye_h: self.spec.eye_h,
            color_format: self.spec.color_format as u32,
            flags: self.spec.flags,
            hud_w: self.spec.hud_w,
            hud_h: self.spec.hud_h,
            aux_w: self.spec.aux_w,
            aux_h: self.spec.aux_h,
            slots: SLOT_COUNT as u32,
            _pad: 0,
        }
    }
}

