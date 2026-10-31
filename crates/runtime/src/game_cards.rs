//! Cards of the game's own interface beside its objects ([`Cards`]): what a bench would make, say,
//! put together from the game's own pieces, its menu art and its bitmap fonts, from the asset cache
//! (`rustvr_asset::Texture`, `rustvr_asset::Font`).
//!
//! Each card is painted (with egui) into a texture of its own whenever its revision changes: every
//! image piece the part of its texture stretched over its rectangle, every text piece laid out the
//! way the game lays out text, glyph by glyph along the line (the space before each, its width,
//! the space after it), its top that far over the baseline, the words wrapped at the piece's
//! width. It floats over its object, turned to the eyes, fading in as it comes and out as it goes;
//! a card on a sheet ([`OBJF_SHEET`]) is that sheet's face instead, drawn on it with the held
//! things ([`GameCards::faces`], `crate::held_objects`).

use std::collections::HashMap;

use anyhow::Result;
use glam::Vec3;
use rustvr_gpu::{EguiPainter, Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

use crate::panels::Panel;
use crate::rig::WorldMap;

/// Pixels a card unit is painted at, and the largest a card's texture is.
const PIXELS_PER_UNIT: f32 = 1.5;
const MAX_PX: u32 = 1024;
/// A card's width in the world when the game doesn't say (metres).
pub const WIDTH: f32 = 0.26;
/// How far over its object's top it floats, and towards the eyes (metres).
const ABOVE: f32 = 0.08;
const TOWARDS: f32 = 0.03;
/// Seconds to fade in and out; how quickly it catches up with its object.
const FADE_IN: f32 = 0.2;
const FADE_OUT: f32 = 0.3;
const FOLLOW: f32 = 0.06;

/// A game font, ready to lay text out with: its glyphs and its atlas.
pub(crate) struct LoadedFont {
    pub(crate) font: rustvr_asset::Font,
    pub(crate) atlas: egui::TextureHandle,
    /// The tallest glyph's top over the baseline: where a line's baseline is under its top.
    pub(crate) ascent: f32,
}

/// One card on show (or fading).
struct Shown {
    tex: Texture,
    /// Its texture's raw view (what it was painted, unconverted: as the game's own textures are
    /// read where its lighting shades a sheet), and which texture it is (a new one when its size
    /// changes).
    raw: ID3D11ShaderResourceView,
    generation: u64,
    px: (u32, u32),
    painted: Option<u32>,
    /// Where it is (tracking space, eased) and how strongly it shows.
    at: Option<Vec3>,
    opacity: f32,
    card: Card,
    present: bool,
}

#[derive(Default)]
pub(crate) struct Assets {
    images: HashMap<String, Option<egui::TextureHandle>>,
    fonts: HashMap<String, Option<LoadedFont>>,
}

pub struct GameCards {
    ctx: egui::Context,
    painter: EguiPainter,
    table: Box<Cards>,
    shown: HashMap<u32, Shown>,
    assets: Assets,
    /// The sheets the cards are on this frame (their objects' ids).
    sheets: Vec<u32>,
    generations: u64,
}

/// A sheet's face this frame ([`OBJF_SHEET`]): its card's texture (sRGB-decoding and raw views, and
/// which texture it is), and the card's first piece (the cache's texture and the part of it: its
/// paper) for the back of it.
pub struct Face {
    pub anchor: u32,
    pub srgb: ID3D11ShaderResourceView,
    pub raw: ID3D11ShaderResourceView,
    pub generation: u64,
    pub back: Option<(String, [f32; 4])>,
}

/// An object's place (tracking space) and how far its top is over its middle, from its state.
pub fn place_of(id: u32, states: &ObjectStates, objects: &ObjectTable, map: Option<&WorldMap>) -> Option<(Vec3, f32)> {
    let s = states.states[..(states.count as usize).min(MAX_OBJECTS)].iter().find(|s| s.id == id)?;
    let p = Vec3::from_array(s.pose.pos);
    let at = if s.flags & OS_TRACKING != 0 { p } else { map?.inv_point(p) };
    let d = objects.descs[..(objects.count as usize).min(MAX_OBJECTS)].iter().find(|d| d.id == s.desc)?;
    let wpm = map.map_or(1.0, |m| m.world_per_meter).max(1e-6);
    Some((at, (d.center[1] + d.half_extents[1]).max(0.02) / wpm))
}

/// Text in the game's 8-bit characters (Latin-1, as Oblivion's): what isn't one is left out.
pub(crate) fn bytes_of(text: &str) -> Vec<u8> {
    text.chars().filter_map(|c| u8::try_from(c as u32).ok()).collect()
}

/// The lines `text` wraps into at `width` (0: one line), each with its width: broken at spaces,
/// as the game wraps.
pub(crate) fn wrap(font: &rustvr_asset::Font, text: &[u8], width: f32) -> Vec<(Vec<u8>, f32)> {
    let measure = |w: &[u8]| w.iter().map(|&c| font.advance(c)).sum::<f32>();
    let mut lines: Vec<(Vec<u8>, f32)> = Vec::new();
    let mut line: Vec<u8> = Vec::new();
    for word in text.split(|&c| c == b' ') {
        let mut next = line.clone();
        if !next.is_empty() {
            next.push(b' ');
        }
        next.extend_from_slice(word);
        if width > 0.0 && !line.is_empty() && measure(&next) > width {
            let w = measure(&line);
            lines.push((std::mem::take(&mut line), w));
            line.extend_from_slice(word);
        } else {
            line = next;
        }
    }
    let w = measure(&line);
    lines.push((line, w));
    lines
}

impl GameCards {
    pub fn new(gpu: &Gpu) -> Result<GameCards> {
        Ok(GameCards {
            ctx: egui::Context::default(),
            painter: EguiPainter::new(gpu)?,
            table: bytemuck::zeroed_box(),
            shown: HashMap::new(),
            assets: Assets::default(),
            sheets: Vec::new(),
            generations: 0,
        })
    }

    /// The game's cards (once a frame).
    pub fn read(&mut self, link: &rustvr_protocol::link::Link) {
        if !link.read_cards_into(&mut self.table) {
            self.table.count = 0;
        }
    }

    /// This frame's cards: each one's panel, how strongly it shows and its texture (the cards on
    /// sheets aren't among them: [`GameCards::faces`]). `head`: the eyes' middle (tracking space).
    pub fn update(&mut self, gpu: &Gpu, states: &ObjectStates, objects: &ObjectTable, map: Option<&WorldMap>, head: Vec3, dt: f32) -> Result<Vec<(Panel, f32, u32)>> {
        for s in self.shown.values_mut() {
            s.present = false;
        }
        self.sheets = objects.descs[..(objects.count as usize).min(MAX_OBJECTS)].iter().filter(|d| d.id != 0 && d.flags & OBJF_SHEET != 0).map(|d| d.id).collect();
        let cards: Vec<Card> = self.table.entries().iter().filter(|c| c.anchor != 0 && c.count > 0).copied().collect();
        for card in cards {
            // (A card too big for the largest texture at that is painted smaller, its shape kept.)
            let k = PIXELS_PER_UNIT.min(MAX_PX as f32 / card.size[0].max(1.0)).min(MAX_PX as f32 / card.size[1].max(1.0));
            let px = ((card.size[0] * k).clamp(16.0, MAX_PX as f32) as u32, (card.size[1] * k).clamp(16.0, MAX_PX as f32) as u32);
            if self.shown.get(&card.anchor).is_none_or(|s| s.px != px) {
                let desc = TexDesc {
                    width: px.0,
                    height: px.1,
                    format: DXGI_FORMAT_R8G8B8A8_TYPELESS,
                    rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM),
                    srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
                    dsv: None,
                    mips: 1,
                };
                let old = self.shown.remove(&card.anchor);
                let tex = Texture::new(gpu, &desc)?;
                let raw = tex.view(gpu, DXGI_FORMAT_R8G8B8A8_UNORM)?;
                self.generations += 1;
                self.shown.insert(
                    card.anchor,
                    Shown {
                        tex,
                        raw,
                        generation: self.generations,
                        px,
                        painted: None,
                        at: old.as_ref().and_then(|o| o.at),
                        opacity: old.map_or(0.0, |o| o.opacity),
                        card,
                        present: true,
                    },
                );
            }
            let s = self.shown.get_mut(&card.anchor).unwrap();
            s.card = card;
            s.present = true;
        }
        let mut out = Vec::new();
        let ids: Vec<u32> = self.shown.keys().copied().collect();
        for id in ids {
            // A sheet's is there, whole, as long as the sheet is: it's the page.
            let sheet = self.sheets.contains(&id);
            let place = if sheet { None } else { place_of(id, states, objects, map) };
            let s = self.shown.get_mut(&id).unwrap();
            let showing = s.present && (sheet || place.is_some());
            s.opacity = if sheet {
                showing as u32 as f32
            } else if showing {
                (s.opacity + dt / FADE_IN).min(1.0)
            } else {
                (s.opacity - dt / FADE_OUT).max(0.0)
            };
            if !showing && s.opacity == 0.0 {
                self.shown.remove(&id);
                continue;
            }
            let width = if s.card.world_width > 0.0 { s.card.world_width } else { WIDTH };
            let aspect = s.px.0 as f32 / s.px.1 as f32;
            if let Some((at, top)) = place {
                let toward = (head - at).normalize_or(Vec3::Z);
                let want = at + Vec3::Y * (top + ABOVE + width / aspect * 0.5) + toward * TOWARDS;
                let k = 1.0 - (-dt / FOLLOW).exp();
                s.at = Some(s.at.map_or(want, |a| a.lerp(want, k)));
            }
            if s.painted != Some(s.card.revision) {
                let card = s.card;
                let (rtv, px) = (s.tex.rtv.clone().unwrap(), s.px);
                self.paint(gpu, &card, &rtv, px)?;
                self.shown.get_mut(&id).unwrap().painted = Some(card.revision);
            }
            if sheet {
                continue;
            }
            let s = &self.shown[&id];
            let Some(at) = s.at else { continue };
            let d = (at - head).normalize_or(Vec3::Z);
            let (yaw, pitch) = (d.x.atan2(d.z), -(d.y.clamp(-1.0, 1.0)).asin());
            let eased = s.opacity * s.opacity * (3.0 - 2.0 * s.opacity);
            out.push((Panel::facing(at, yaw, pitch, width, aspect), eased, id));
        }
        Ok(out)
    }

    /// The faces of the sheets the cards are on, painted.
    pub fn faces(&self) -> Vec<Face> {
        self.shown
            .iter()
            .filter(|(id, s)| self.sheets.contains(id) && s.present && s.painted.is_some())
            .filter_map(|(id, s)| {
                let back = s.card.pieces().first().filter(|p| p.kind == PIECE_IMAGE).map(|p| (p.asset().to_owned(), p.uv));
                Some(Face { anchor: *id, srgb: s.tex.srv.clone()?, raw: s.raw.clone(), generation: s.generation, back })
            })
            .collect()
    }

    /// The texture of the card over `anchor`.
    pub fn srv(&self, anchor: u32) -> Option<&ID3D11ShaderResourceView> {
        self.shown.get(&anchor).and_then(|s| s.tex.srv.as_ref())
    }

    /// An image piece's texture, loaded from the asset cache the first time it's asked for (one
    /// that won't load is said once).
    pub(crate) fn image(ctx: &egui::Context, assets: &mut Assets, name: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = assets.images.get(name) {
            return t.clone();
        }
        let loaded = (|| -> Result<egui::TextureHandle> {
            let (game, key) = name.split_once('/').ok_or_else(|| anyhow::anyhow!("{name:?} isn't <game>/<key>"))?;
            let t = rustvr_asset::load_texture(&rustvr_asset::default_root().join(game), key)?;
            Ok(ctx.load_texture(name, color_image(&t)?, egui::TextureOptions::LINEAR))
        })();
        let t = match loaded {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!("cards: can't load the game's {name}: {e:#}");
                None
            }
        };
        assets.images.insert(name.to_owned(), t.clone());
        t
    }

    /// A text piece's font, loaded (with its atlas) the first time.
    pub(crate) fn font<'a>(ctx: &egui::Context, assets: &'a mut Assets, name: &str) -> Option<&'a LoadedFont> {
        if !assets.fonts.contains_key(name) {
            let loaded = (|| -> Result<LoadedFont> {
                let (game, key) = name.split_once('/').ok_or_else(|| anyhow::anyhow!("{name:?} isn't <game>/<key>"))?;
                let dir = rustvr_asset::default_root().join(game);
                let font = rustvr_asset::load_font(&dir, key)?;
                let atlas = rustvr_asset::load_texture(&dir, &font.atlas)?;
                let atlas = ctx.load_texture(format!("{name}#atlas"), color_image(&atlas)?, egui::TextureOptions::LINEAR);
                let ascent = font.glyphs.iter().map(|g| g.top).fold(0.0f32, f32::max);
                Ok(LoadedFont { font, atlas, ascent })
            })();
            let f = match loaded {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::warn!("cards: can't load the game's font {name}: {e:#}");
                    None
                }
            };
            assets.fonts.insert(name.to_owned(), f);
        }
        assets.fonts.get(name).and_then(|f| f.as_ref())
    }

    fn paint(&mut self, gpu: &Gpu, card: &Card, rtv: &windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView, px: (u32, u32)) -> Result<()> {
        let scale = px.0 as f32 / card.size[0].max(1.0);
        self.ctx.set_pixels_per_point(1.0);
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(px.0 as f32, px.1 as f32))), ..Default::default() };
        let (ctx, assets) = (self.ctx.clone(), &mut self.assets);
        let mut out = ctx.run_ui(input, |ui| {
            let painter = ui.painter().clone();
            for piece in card.pieces() {
                let r = piece.rect;
                let rect = egui::Rect::from_min_size(egui::pos2(r[0] * scale, r[1] * scale), egui::vec2(r[2] * scale, r[3] * scale));
                let c = piece.color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                let tint = egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
                match piece.kind {
                    PIECE_IMAGE => {
                        if let Some(t) = Self::image(&ctx, assets, piece.asset()) {
                            let uv = egui::Rect::from_min_max(egui::pos2(piece.uv[0], piece.uv[1]), egui::pos2(piece.uv[2], piece.uv[3]));
                            painter.image(t.id(), rect, uv, tint);
                        }
                    }
                    PIECE_TEXT => {
                        let Some(f) = Self::font(&ctx, assets, piece.asset()) else { continue };
                        painter.add(egui::Shape::mesh(text_mesh(f, piece.text_str(), r, piece.flags, scale, tint)));
                    }
                    _ => {}
                }
            }
        });
        let prims = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        self.painter.paint(gpu, rtv, px.0, px.1, out.pixels_per_point, &prims, &mut out.textures_delta, true)
    }
}

/// `text` laid out the way the game lays out text, glyph by glyph along the line (the space before
/// each, its width, the space after it), its top that far over the baseline, wrapped at `r`'s width
/// (0: one line), from `r`'s top left or centred or to its right ([`PIECEF_CENTRE`],
/// [`PIECEF_RIGHT`]); `r` in card units, painted at `scale` pixels a unit.
pub(crate) fn text_mesh(f: &LoadedFont, text: &str, r: [f32; 4], flags: u32, scale: f32, tint: egui::Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(f.atlas.id());
    let mut top = r[1];
    for (line, w) in wrap(&f.font, &bytes_of(text), r[2]) {
        let mut x = if flags & PIECEF_CENTRE != 0 {
            r[0] + (r[2] - w) * 0.5
        } else if flags & PIECEF_RIGHT != 0 {
            r[0] + r[2] - w
        } else {
            r[0]
        };
        let baseline = top + f.ascent;
        for c in line {
            let Some(g) = f.font.glyphs.get(c as usize) else { continue };
            x += g.before;
            if g.width > 0.0 && g.height > 0.0 {
                let at = egui::Rect::from_min_size(egui::pos2(x * scale, (baseline - g.top) * scale), egui::vec2(g.width * scale, g.height * scale));
                let uv = egui::Rect::from_min_max(egui::pos2(g.uv[0], g.uv[1]), egui::pos2(g.uv[2], g.uv[3]));
                mesh.add_rect_with_uv(at, uv, tint);
            }
            x += g.width + g.after;
        }
        top += f.font.height;
    }
    mesh
}

/// A cache texture as egui's image: 8-bit RGBA only (the extractor writes interface art so).
pub(crate) fn color_image(t: &rustvr_asset::Texture) -> Result<egui::ColorImage> {
    anyhow::ensure!(t.format == rustvr_asset::TexFormat::Rgba8, "{} isn't RGBA ({:?})", t.source, t.format);
    let px = t.mips.first().ok_or_else(|| anyhow::anyhow!("{} has no pixels", t.source))?;
    anyhow::ensure!(px.len() >= (t.width * t.height * 4) as usize, "{} is short", t.source);
    Ok(egui::ColorImage::from_rgba_unmultiplied([t.width as usize, t.height as usize], &px[..(t.width * t.height * 4) as usize]))
}
