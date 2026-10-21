//! The shared textures adapters render into: created here, opened by name in the game.

use anyhow::Result;
use rustvr_gpu::texture::color_family;
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::link::Security;
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM};

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

impl Spec {
    /// From the adapter's request, the headset's recommended size and the settings.
    pub fn new(req: &TargetRequest, recommended: (u32, u32), s: &Settings) -> Spec {
        let (eye_w, eye_h) = if req.eye_w > 0 && req.eye_h > 0 {
            (req.eye_w, req.eye_h)
        } else {
            let scale = s.vr.render_scale.clamp(0.25, 3.0);
            let (mut w, mut h) = ((recommended.0 as f32 * scale) as u32, (recommended.1 as f32 * scale) as u32);
            let cap = s.vr.max_eye_size;
            if cap > 0 && w.max(h) > cap {
                let k = cap as f32 / w.max(h) as f32;
                (w, h) = ((w as f32 * k) as u32, (h as f32 * k) as u32);
            }
            (w.max(16), h.max(16))
        };
        let fmt = if req.color_format == 0 { DXGI_FORMAT_R8G8B8A8_UNORM } else { DXGI_FORMAT(req.color_format as i32) };
        let (typeless, _, _) = color_family(fmt);
        let (aux_w, aux_h) = if req.aux_size > 0 { (req.aux_size, req.aux_size) } else { (eye_w, eye_h) };
        Spec {
            eye_w,
            eye_h,
            color_format: typeless.0,
            flags: req.flags & (TGT_DEPTH | TGT_HUD | TGT_AUX),
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
}

impl Slot {
    fn all(&self) -> impl Iterator<Item = &Texture> {
        self.eyes.iter().chain(self.depth.iter().flatten()).chain(self.hud.iter()).chain(self.aux.iter())
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
            slots.push(Slot { eyes, depth, hud, aux });
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
