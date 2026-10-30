//! Pictures of a game's own card of a thing in a hand ([`HUD_HELD`]), one for each thing it's been
//! of. The game's card is of one thing at a time, so each hand's card is drawn from a picture of it,
//! cut from the game's HUD while the card was of that hand's thing ([`HudLayout::held`]): both
//! hands have theirs, in the game's own art.

use anyhow::{Context, Result};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::{HUD_HELD, HudLayout};
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, ID3D11ShaderResourceView};

/// How many pictures are kept: the things in both hands, and a few let go of (fading, or coming
/// back to a hand) before they're dropped, the longest unseen first.
const KEEP: usize = 6;

struct Picture {
    thing: u32,
    tex: Texture,
    /// Its share of the HUD's width (the card's part of it).
    du: f32,
    /// When it was last cut or drawn (frames).
    used: u64,
}

#[derive(Default)]
pub struct HeldCards {
    pictures: Vec<Picture>,
    frame: u64,
    said: bool,
}

impl HeldCards {
    /// Once a frame, before the pictures are cut or drawn.
    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// The game's HUD this frame (`hud`, its layout `layout`): while its card is of a thing in a hand,
    /// that thing's picture, cut from it again.
    pub fn cut(&mut self, gpu: &Gpu, hud: &Texture, layout: &HudLayout) {
        let Some(e) = layout.entries().iter().find(|e| e.role == HUD_HELD).filter(|_| layout.card_of != 0) else { return };
        if let Err(err) = self.cut_part(gpu, hud, e.uv, layout.card_of)
            && !std::mem::replace(&mut self.said, true)
        {
            tracing::warn!("held cards: the game's card couldn't be cut from its HUD: {err:#}");
        }
    }

    fn cut_part(&mut self, gpu: &Gpu, hud: &Texture, uv: [f32; 4], thing: u32) -> Result<()> {
        let px = |v: f32, n: u32| (v.clamp(0.0, 1.0) * n as f32).round() as u32;
        let (x0, y0, x1, y1) = (px(uv[0], hud.width), px(uv[1], hud.height), px(uv[2], hud.width), px(uv[3], hud.height));
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let frame = self.frame;
        let at = self.pictures.iter().position(|p| p.thing == thing);
        if at.is_none_or(|i| (self.pictures[i].tex.width, self.pictures[i].tex.height) != (w, h)) {
            // A new picture (or the card's part changed size): the HUD's own format and view of it.
            let mut view = Default::default();
            unsafe { hud.srv.as_ref().context("the HUD has no view")?.GetDesc(&mut view) };
            let tex = Texture::new(gpu, &TexDesc { width: w, height: h, format: hud.format, rtv: None, srv: Some(view.Format), dsv: None, mips: 1 })?;
            let picture = Picture { thing, tex, du: uv[2] - uv[0], used: frame };
            match at {
                Some(i) => self.pictures[i] = picture,
                None => {
                    if self.pictures.len() >= KEEP
                        && let Some(oldest) = (0..self.pictures.len()).min_by_key(|&k| self.pictures[k].used)
                    {
                        self.pictures.remove(oldest);
                    }
                    self.pictures.push(picture);
                }
            }
        }
        let p = self.pictures.iter_mut().find(|p| p.thing == thing).expect("made above");
        p.du = uv[2] - uv[0];
        p.used = frame;
        let region = D3D11_BOX { left: x0, top: y0, front: 0, right: x1, bottom: y1, back: 1 };
        unsafe { gpu.ctx.CopySubresourceRegion(&p.tex.tex, 0, 0, 0, 0, &hud.tex, 0, Some(&region)) };
        Ok(())
    }

    /// The picture of the card of `thing` is drawn this frame (it's kept).
    pub fn touch(&mut self, thing: u32) {
        let frame = self.frame;
        if let Some(p) = self.pictures.iter_mut().find(|p| p.thing == thing) {
            p.used = frame;
        }
    }

    /// The picture of the card of `thing`, if there's one: its view, its share of the HUD's width,
    /// and its width / height.
    pub fn picture(&self, thing: u32) -> Option<(&ID3D11ShaderResourceView, f32, f32)> {
        let p = self.pictures.iter().find(|p| p.thing == thing)?;
        Some((p.tex.srv.as_ref()?, p.du, p.tex.width as f32 / p.tex.height.max(1) as f32))
    }
}
