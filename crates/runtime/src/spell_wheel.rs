//! The spell wheel (Spell Wheel VR, Skyrim VR's most-installed mod): the player's spells in a ring
//! ahead of them, to put one in either hand without a menu (`crate::magic`).
//!
//! * **Opened** by the main hand's B held, at the height of the chest a little ahead, facing the
//!   eyes; let go of B and it shuts.
//! * **Pointed at** (Spell Wheel VR puts the hand in an orb; here a pointer, as the game's menus are
//!   worked): each hand's pointer on the wheel lights the spell its way from the middle (a slice of
//!   the ring, not the icon itself, and past the ring's edge too: it's quick), with a tick in that
//!   hand as it moves onto another. A hand's trigger on a spell puts it in that hand (Skyrim VR's "the trigger of the hand
//!   you want it in"); B let go with the main hand on one puts it in the main hand. The middle is an
//!   empty hand (its trigger the game's Activate again).
//! * **What's in it**: the spells and powers the game lists, the ones on its quick keys first,
//!   twelve to a page (the arrows at its foot turn the page); each its icon (the game's art), dim
//!   when the game won't cast it now, a mark on the side of the hand that holds it; the one
//!   pointed at named in the middle, with its cost, in the game's font.

use anyhow::Result;
use glam::{Quat, Vec3};
use rustvr_gpu::{EguiPainter, Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

use crate::backend::HmdFrame;
use crate::combat::Pulse;
use crate::game_cards::{Assets, GameCards, text_mesh};
use crate::panels::Panel;

/// Spells on a page.
pub const PER_PAGE: usize = 12;
/// The wheel's size (metres across), how far ahead of the eyes and under them.
const WIDTH: f32 = 0.44;
const AHEAD: f32 = 0.42;
const BELOW: f32 = 0.2;
/// Its layout, in its own units (1000 across): the ring's radius, an icon (and one pointed at),
/// the middle's radius, the disc's edge, how far past it a pointer still takes its slice (a slice is
/// a direction from the middle, as a marking menu's: overshooting is fine), the page arrows.
const UNITS: f32 = 1000.0;
const RING: f32 = 330.0;
const ICON: f32 = 112.0;
const ICON_HOVER: f32 = 146.0;
const MIDDLE: f32 = 140.0;
const REACH: f32 = 480.0;
const SLICE_REACH: f32 = 1100.0;
const ARROW_AT: [(f32, f32); 2] = [(110.0, 900.0), (890.0, 900.0)];
const ARROW_RADIUS: f32 = 75.0;
/// Its texture's size.
const PX: u32 = 768;
/// A trigger pulled past this picks.
const TRIGGER: f32 = 0.6;
/// The tick as a pointer moves onto another entry, and the pick's.
const TICK: Pulse = Pulse { hand: 0, seconds: 0.01, frequency: 200.0, amplitude: 0.3 };
const PICKED: Pulse = Pulse { hand: 0, seconds: 0.04, frequency: 160.0, amplitude: 0.7 };

/// What a pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Entry {
    Spell(u32),
    /// The middle: an empty hand.
    Empty,
    Prev,
    Next,
}

/// A hand took a spell from the wheel (0: an empty hand).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub hand: usize,
    pub spell: u32,
}

pub struct WheelInput<'a> {
    pub frame: &'a HmdFrame,
    pub spells: &'a Spells,
    /// The spell each hand holds.
    pub held: [u32; 2],
    pub dominant: usize,
    pub now: f64,
    /// The wheel may be up (in the world, not paused, the game has spells).
    pub enabled: bool,
}

struct Open {
    /// Where it is (tracking space).
    panel: Panel,
    page: usize,
    hover: [Option<Entry>; 2],
    /// Where each pointer meets it (tracking space).
    hits: [Option<Vec3>; 2],
}

/// The wheel's art, painted whenever what it shows changes.
struct Art {
    ctx: egui::Context,
    painter: EguiPainter,
    assets: Assets,
    tex: Texture,
    painted: Option<u64>,
}

#[derive(Default)]
pub struct Wheel {
    open: Option<Open>,
    b_was: bool,
    trigger_was: [bool; 2],
    /// This frame's ticks in the hands.
    pub pulses: Vec<Pulse>,
    /// What's on it: the spells in the order shown.
    order: Vec<Spell>,
    held: [u32; 2],
    font: String,
    back: String,
    magicka: f32,
    art: Option<Art>,
}

/// The spells in the order the wheel shows them: the quick keys' first, then the game's order.
pub fn order(spells: &Spells) -> Vec<Spell> {
    let (mut first, rest): (Vec<Spell>, Vec<Spell>) = spells.entries().iter().copied().partition(|s| s.flags & SPELL_FAVOURITE != 0);
    first.extend(rest);
    first
}

/// The pages `n` spells take.
pub fn pages(n: usize) -> usize {
    n.div_ceil(PER_PAGE).max(1)
}

/// Where the `i`th of `n` on a page sits (wheel units): from the top, clockwise.
fn slot(i: usize, n: usize) -> (f32, f32) {
    let a = i as f32 / n.max(1) as f32 * std::f32::consts::TAU;
    (UNITS * 0.5 + RING * a.sin(), UNITS * 0.5 - RING * a.cos())
}

/// Where a pointer from `o` along `d` meets the wheel's plane, on it or past its edge: how far, and
/// where in its units (0..1 across its face, top left; beyond it outside).
fn plane_hit(panel: &Panel, o: Vec3, d: Vec3) -> Option<(f32, (f32, f32))> {
    let n = panel.normal();
    let denom = d.dot(n);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (panel.center - o).dot(n) / denom;
    if t <= 0.0 || t > 3.0 {
        return None;
    }
    let p = o + d * t - panel.center;
    Some((t, (p.dot(panel.right) / panel.width + 0.5, 0.5 - p.dot(panel.up) / panel.height)))
}

/// What's at `(x, y)` (wheel units) on a page of `n` spells (`ids`), with `pages` pages.
pub fn entry_at(x: f32, y: f32, ids: &[u32], pages: usize) -> Option<Entry> {
    if pages > 1 {
        for (k, &(ax, ay)) in ARROW_AT.iter().enumerate() {
            if (x - ax).hypot(y - ay) <= ARROW_RADIUS {
                return Some(if k == 0 { Entry::Prev } else { Entry::Next });
            }
        }
    }
    let (dx, dy) = (x - UNITS * 0.5, y - UNITS * 0.5);
    let r = dx.hypot(dy);
    if r < MIDDLE {
        return Some(Entry::Empty);
    }
    if r > SLICE_REACH || ids.is_empty() {
        return None;
    }
    // The slice its way from the middle: angle from the top, clockwise.
    let a = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
    let n = ids.len();
    let i = ((a / std::f32::consts::TAU * n as f32).round() as usize) % n;
    Some(Entry::Spell(ids[i]))
}

impl Wheel {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The spells on the page shown.
    fn page_ids(&self, page: usize) -> Vec<u32> {
        self.order.iter().skip(page * PER_PAGE).take(PER_PAGE).map(|s| s.id).collect()
    }

    /// One frame: open, point, pick, shut. Returns what the hands took.
    pub fn update(&mut self, inp: &WheelInput) -> Vec<Pick> {
        self.pulses.clear();
        let mut picks = Vec::new();
        let dom = inp.dominant;
        let b = inp.frame.hands[dom].buttons & CTRL_B != 0 && inp.frame.hands[dom].connected != 0;
        let pressed = b && !self.b_was;
        self.b_was = b;
        let triggers: [bool; 2] = std::array::from_fn(|h| inp.frame.hands[h].trigger > TRIGGER);
        let pulled: [bool; 2] = std::array::from_fn(|h| triggers[h] && !self.trigger_was[h]);
        self.trigger_was = triggers;
        if !inp.enabled {
            self.open = None;
            return picks;
        }
        self.order = order(inp.spells);
        self.held = inp.held;
        self.font = inp.spells.font().to_owned();
        self.back = inp.spells.back().to_owned();
        self.magicka = if inp.spells.magicka_max > 0.0 { inp.spells.magicka / inp.spells.magicka_max } else { 1.0 };
        if pressed && self.open.is_none() && !self.order.is_empty() {
            let head = Vec3::from_array(inp.frame.hmd.pos);
            let look = Quat::from_array(inp.frame.hmd.rot) * Vec3::Z;
            let ahead = Vec3::new(look.x, 0.0, look.z).normalize_or(Vec3::Z);
            let center = head + ahead * AHEAD - Vec3::Y * BELOW;
            let d = (center - head).normalize_or(Vec3::Z);
            let panel = Panel::facing(center, d.x.atan2(d.z), -(d.y.clamp(-1.0, 1.0)).asin(), WIDTH, 1.0);
            // (It opens on the page of the main hand's spell.)
            let page = self.order.iter().position(|s| s.id == inp.held[dom]).map_or(0, |i| i / PER_PAGE);
            self.open = Some(Open { panel, page, hover: [None; 2], hits: [None; 2] });
            tracing::info!("magic: the spell wheel opens ({} spells, {} pages)", self.order.len(), pages(self.order.len()));
        }
        let n_pages = pages(self.order.len());
        let Some(mut open) = self.open.take() else { return picks };
        open.page = open.page.min(n_pages - 1);
        let ids = self.page_ids(open.page);
        for h in 0..2 {
            let c = &inp.frame.hands[h];
            let (o, d) = (Vec3::from_array(c.aim.pos), Quat::from_array(c.aim.rot) * Vec3::Z);
            let hit = (c.connected != 0).then(|| plane_hit(&open.panel, o, d)).flatten();
            open.hits[h] = hit.map(|(t, _)| o + d * t);
            let now = hit.and_then(|(_, (u, v))| entry_at(u * UNITS, v * UNITS, &ids, n_pages));
            if now.is_some() && now != open.hover[h] {
                self.pulses.push(Pulse { hand: h, ..TICK });
            }
            open.hover[h] = now;
        }
        let mut shut = false;
        for h in 0..2 {
            if !pulled[h] {
                continue;
            }
            match open.hover[h] {
                Some(Entry::Spell(id)) => {
                    picks.push(Pick { hand: h, spell: id });
                    shut = true;
                }
                Some(Entry::Empty) => {
                    picks.push(Pick { hand: h, spell: 0 });
                    shut = true;
                }
                Some(Entry::Prev) => open.page = (open.page + n_pages - 1) % n_pages,
                Some(Entry::Next) => open.page = (open.page + 1) % n_pages,
                None => {}
            }
        }
        if !b && !shut {
            match open.hover[dom] {
                Some(Entry::Spell(id)) => picks.push(Pick { hand: dom, spell: id }),
                Some(Entry::Empty) => picks.push(Pick { hand: dom, spell: 0 }),
                _ => {}
            }
            shut = true;
        }
        for p in &picks {
            self.pulses.push(Pulse { hand: p.hand, ..PICKED });
        }
        if shut {
            tracing::info!("magic: the spell wheel shuts");
        } else {
            self.open = Some(open);
        }
        picks
    }

    /// Each hand's pointer while the wheel is up (tracking space): from, to, whether it's on it.
    pub fn pointers(&self, frame: &HmdFrame) -> Vec<(Vec3, Vec3, bool)> {
        let Some(open) = &self.open else { return Vec::new() };
        (0..2)
            .filter(|&h| frame.hands[h].connected != 0)
            .map(|h| {
                let c = &frame.hands[h];
                let o = Vec3::from_array(c.aim.pos);
                let d = Quat::from_array(c.aim.rot) * Vec3::Z;
                match open.hits[h] {
                    Some(p) => (o, p, true),
                    None => (o, o + d * 0.5, false),
                }
            })
            .collect()
    }

    /// Paints the wheel again if what it shows changed (before it's drawn, [`Wheel::shown`]).
    pub fn paint(&mut self, gpu: &Gpu) -> Result<()> {
        let Some(open) = &self.open else { return Ok(()) };
        if self.art.is_none() {
            let desc = TexDesc { width: PX, height: PX, format: DXGI_FORMAT_R8G8B8A8_TYPELESS, rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM), srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), dsv: None, mips: 1 };
            self.art = Some(Art { ctx: egui::Context::default(), painter: EguiPainter::new(gpu)?, assets: Assets::default(), tex: Texture::new(gpu, &desc)?, painted: None });
        }
        let key = {
            use std::hash::{Hash, Hasher};
            let mut k = std::collections::hash_map::DefaultHasher::new();
            open.page.hash(&mut k);
            open.hover.hash(&mut k);
            self.held.hash(&mut k);
            ((self.magicka * 20.0) as i32).hash(&mut k);
            for s in &self.order {
                (s.id, s.flags, s.cost as i32).hash(&mut k);
            }
            k.finish()
        };
        let (page, hover) = (open.page, open.hover);
        let art = self.art.as_mut().unwrap();
        if art.painted != Some(key) {
            let shown: Vec<Spell> = self.order.iter().skip(page * PER_PAGE).take(PER_PAGE).copied().collect();
            paint(gpu, art, &shown, page, pages(self.order.len()), hover, self.held, &self.font, &self.back)?;
            art.painted = Some(key);
        }
        Ok(())
    }

    /// The wheel to draw this frame, if it's up: where (tracking space), and its art.
    pub fn shown(&self) -> Option<(Panel, &ID3D11ShaderResourceView)> {
        let open = self.open.as_ref()?;
        let art = self.art.as_ref().filter(|a| a.painted.is_some())?;
        art.tex.srv.as_ref().map(|srv| (open.panel, srv))
    }
}

#[allow(clippy::too_many_arguments)]
fn paint(gpu: &Gpu, art: &mut Art, shown: &[Spell], page: usize, n_pages: usize, hover: [Option<Entry>; 2], held: [u32; 2], font: &str, back: &str) -> Result<()> {
    let scale = PX as f32 / UNITS;
    art.ctx.set_pixels_per_point(1.0);
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(PX as f32, PX as f32))), ..Default::default() };
    let (ctx, assets) = (art.ctx.clone(), &mut art.assets);
    let at = |x: f32, y: f32| egui::pos2(x * scale, y * scale);
    let mut out = ctx.run_ui(input, |ui| {
        let p = ui.painter().clone();
        let centre = at(UNITS * 0.5, UNITS * 0.5);
        // The ground: the game's art when it gives it, else a dark disc.
        match (!back.is_empty()).then(|| GameCards::image(&ctx, assets, back)).flatten() {
            Some(t) => {
                p.image(t.id(), egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(PX as f32, PX as f32)), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::from_white_alpha(235));
            }
            None => {
                p.circle_filled(centre, REACH * scale, egui::Color32::from_rgba_unmultiplied(12, 10, 8, 200));
            }
        }
        p.circle_stroke(centre, (RING + ICON * 0.75) * scale, egui::Stroke::new(2.0, egui::Color32::from_rgba_unmultiplied(200, 180, 140, 90)));
        p.circle_stroke(centre, (RING - ICON * 0.75) * scale, egui::Stroke::new(2.0, egui::Color32::from_rgba_unmultiplied(200, 180, 140, 90)));
        // The middle: an empty hand.
        let middle_on = hover.contains(&Some(Entry::Empty));
        p.circle_filled(centre, MIDDLE * 0.8 * scale, egui::Color32::from_rgba_unmultiplied(40, 34, 26, if middle_on { 240 } else { 150 }));
        p.circle_stroke(centre, MIDDLE * 0.8 * scale, egui::Stroke::new(if middle_on { 5.0 } else { 2.0 }, egui::Color32::from_rgba_unmultiplied(230, 210, 170, if middle_on { 255 } else { 120 })));
        let words = |p: &egui::Painter, assets: &mut Assets, text: &str, y: f32, size: f32, color: egui::Color32| {
            let r = [0.0, y, UNITS, size * 1.4];
            match (!font.is_empty()).then(|| GameCards::font(&ctx, assets, font)).flatten() {
                Some(f) => {
                    let k = size / f.font.height.max(1.0);
                    // (The game's font at its own size, scaled to `size`: laid out in units over `k`.)
                    let r = [0.0, y / k, UNITS / k, size * 1.4 / k];
                    p.add(egui::Shape::mesh(text_mesh(f, text, r, PIECEF_CENTRE, scale * k, color)));
                }
                None => {
                    p.text(at(UNITS * 0.5, r[1] + size * 0.6), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(size * scale), color);
                }
            }
        };
        // The spells round the ring.
        let ink = egui::Color32::from_rgb(235, 220, 190);
        for (i, s) in shown.iter().enumerate() {
            let (x, y) = slot(i, shown.len());
            let on = hover.contains(&Some(Entry::Spell(s.id)));
            let size = if on { ICON_HOVER } else { ICON };
            let castable = s.flags & SPELL_CASTABLE != 0;
            let alpha = if castable { 255 } else { 130 };
            let rect = egui::Rect::from_center_size(at(x, y), egui::vec2(size * scale, size * scale));
            let c = s.color.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
            match (!s.icon().is_empty()).then(|| GameCards::image(&ctx, assets, s.icon())).flatten() {
                Some(t) => {
                    p.image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::from_white_alpha(alpha));
                }
                None => {
                    p.circle_filled(at(x, y), size * 0.45 * scale, egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], alpha));
                    let initial: String = s.name().chars().take(1).collect();
                    p.text(at(x, y), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(size * 0.5 * scale), egui::Color32::from_black_alpha(alpha));
                }
            }
            if on {
                p.circle_stroke(at(x, y), size * 0.62 * scale, egui::Stroke::new(5.0, egui::Color32::from_rgb(255, 240, 200)));
            }
            // A mark on the side of each hand that holds it.
            for (h, &id) in held.iter().enumerate() {
                if id == s.id {
                    let side = if h == 0 { -1.0 } else { 1.0 };
                    p.circle_filled(at(x + side * size * 0.58, y + size * 0.42), 14.0 * scale, egui::Color32::from_rgb(c[0].max(140), c[1].max(140), c[2].max(140)));
                }
            }
        }
        // What a pointer is on, named in the middle with its cost.
        let named = hover.iter().flatten().next().copied();
        if let Some(Entry::Spell(id)) = named
            && let Some(s) = shown.iter().find(|s| s.id == id)
        {
            words(&p, assets, s.name(), 440.0, 40.0, ink);
            if s.flags & SPELL_POWER == 0 {
                words(&p, assets, &format!("{}", s.cost.round() as i32), 505.0, 34.0, ink.gamma_multiply(0.8));
            }
        }
        if n_pages > 1 {
            for (k, &(x, y)) in ARROW_AT.iter().enumerate() {
                let on = hover.contains(&Some(if k == 0 { Entry::Prev } else { Entry::Next }));
                let dir = if k == 0 { -1.0 } else { 1.0 };
                let s = ARROW_RADIUS * 0.55 * scale;
                let tip = at(x + dir * ARROW_RADIUS * 0.5, y);
                let pts = vec![tip, egui::pos2(tip.x - dir * s * 1.6, tip.y - s), egui::pos2(tip.x - dir * s * 1.6, tip.y + s)];
                p.add(egui::Shape::convex_polygon(pts, if on { egui::Color32::from_rgb(255, 240, 200) } else { ink.gamma_multiply(0.6) }, egui::Stroke::NONE));
            }
            words(&p, assets, &format!("{} / {}", page + 1, n_pages), 30.0, 36.0, ink.gamma_multiply(0.8));
        }
    });
    let prims = art.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
    let rtv = art.tex.rtv.clone().unwrap();
    art.painter.paint(gpu, &rtv, PX, PX, out.pixels_per_point, &prims, &mut out.textures_delta, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_takes_the_slice_its_way_from_the_middle() {
        let ids = [1, 2, 3, 4];
        // Straight up from the middle: the first; to the right: the second; the middle: empty.
        assert_eq!(entry_at(500.0, 250.0, &ids, 1), Some(Entry::Spell(1)));
        assert_eq!(entry_at(700.0, 520.0, &ids, 1), Some(Entry::Spell(2)));
        assert_eq!(entry_at(510.0, 490.0, &ids, 1), Some(Entry::Empty));
        // Past the ring its way: still that slice; far off: nothing. The arrows only with more than a
        // page.
        assert_eq!(entry_at(500.0, -100.0, &ids, 1), Some(Entry::Spell(1)));
        assert_eq!(entry_at(500.0, -900.0, &ids, 1), None);
        assert_eq!(entry_at(110.0, 900.0, &ids, 2), Some(Entry::Prev));
        assert_eq!(pages(25), 3);
    }
}
