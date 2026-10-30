//! A card beside a thing in the hand (or flying to it, or under the fingers in a pocket): what the
//! game says it is ([`ItemInfos`]): its name and how many, its numbers as the game words them
//! (damage, armour, weight, value...), and whether taking it is stealing. It floats just over the
//! thing, turned to the eyes, follows it, and fades in as it's taken and out as it's let go.

use anyhow::Result;
use glam::{Quat, Vec3};
use rustvr_gpu::{EguiPainter, Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

use crate::panels::Panel;
use crate::rig::WorldMap;

const WIDTH_PX: u32 = 720;
const HEIGHT_PX: u32 = 360;
const PIXELS_PER_POINT: f32 = 2.5;
/// The card's width in the world (metres); its height is half that.
pub const WIDTH: f32 = 0.18;
/// How far over the thing's top its bottom edge is, and how far towards the eyes (metres): just over
/// it, in front of it.
const ABOVE: f32 = 0.015;
const TOWARDS: f32 = 0.04;
/// Seconds to fade in and out; how quickly it catches up with the thing it's over.
const FADE_IN: f32 = 0.12;
const FADE_OUT: f32 = 0.2;
const FOLLOW: f32 = 0.04;
/// How far over a hand in a pocket its top is, for the card of the thing under its fingers.
const FELT_TOP: f32 = 0.05;

pub struct ItemCard {
    ctx: egui::Context,
    painter: EguiPainter,
    tex: Texture,
    infos: Box<ItemInfos>,
    /// What's painted on it now (painted again only when that changes).
    painted: Option<ItemInfo>,
    /// The thing it's over, and the point just over it that the card stands on (tracking space,
    /// eased); the id the game's card of it is known by.
    shown: Option<u32>,
    at: Option<Vec3>,
    key: Option<u32>,
    opacity: f32,
}

/// The thing to show hand `hand`'s card for: one it holds or pulls (`hand_of`: which hand has a
/// thing) that the game says what it is (itself, or the template it's a copy of: a thing out of a
/// pocket): the one shown already while the hand still has it, else one held, else the one its
/// fingers are on in a pocket (`felt`: its template, and where the hand is), else one flying in.
/// Where it is (tracking space) and how far the top of it, as it's turned now, is over that.
#[allow(clippy::too_many_arguments)]
pub fn pick(shown: Option<u32>, states: &ObjectStates, objects: &ObjectTable, infos: &ItemInfos, map: Option<&WorldMap>, hand: usize, hand_of: impl Fn(&ObjectState) -> Option<usize>, felt: Option<(u32, Vec3)>) -> Option<(u32, Vec3, f32)> {
    let states = &states.states[..(states.count as usize).min(MAX_OBJECTS)];
    let descs = &objects.descs[..(objects.count as usize).min(MAX_OBJECTS)];
    let place = |s: &ObjectState| -> Option<(u32, Vec3, f32)> {
        info_of(infos, s)?;
        let p = Vec3::from_array(s.pose.pos);
        let tracking = s.flags & OS_TRACKING != 0;
        let at = if tracking { p } else { map?.inv_point(p) };
        let d = descs.iter().find(|d| d.id == s.desc)?;
        let world_per_metre = if tracking { 1.0 } else { map.map_or(1.0, |m| m.world_per_meter) };
        // The highest corner of its box as it's turned now (a plate held flat is low; on its edge,
        // high), not the ball round it: the card sits just over what's there.
        let q = Quat::from_array(s.pose.rot);
        let q = if tracking { q } else { map.map_or(q, |m| m.inv_rot(q)) };
        let (c, h) = (Vec3::from_array(d.center), Vec3::from_array(d.half_extents));
        let corner = |k: u32| c + h * Vec3::new(if k & 1 == 0 { -1.0 } else { 1.0 }, if k & 2 == 0 { -1.0 } else { 1.0 }, if k & 4 == 0 { -1.0 } else { 1.0 });
        let top = (0..8).map(|k| (q * corner(k)).y).fold(f32::MIN, f32::max);
        Some((s.id, at, top / world_per_metre.max(1e-6)))
    };
    let held = |s: &&ObjectState| s.flags & (OS_HELD_LEFT | OS_HELD_RIGHT) != 0 && hand_of(s) == Some(hand);
    let pulled = |s: &&ObjectState| s.flags & OS_PULLED != 0 && hand_of(s) == Some(hand);
    if let Some(id) = shown
        && let Some(s) = states.iter().find(|s| s.id == id).filter(|s| held(s) || pulled(s))
        && let Some(p) = place(s)
    {
        return Some(p);
    }
    // (A hand in a pocket: the card over the hand, as over a thing its size.)
    let felt = || felt.filter(|(t, _)| infos.get(*t).is_some()).map(|(t, at)| (t, at, FELT_TOP));
    states.iter().filter(held).find_map(place).or_else(felt).or_else(|| states.iter().filter(pulled).find_map(place))
}

/// What the game says a thing is: the thing shown, by its own card or its template's; or a template
/// (a thing felt in a pocket).
fn info_for<'a>(infos: &'a ItemInfos, states: &ObjectStates, id: u32) -> Option<&'a ItemInfo> {
    infos.get(info_key(infos, states, id))
}

/// The id the game's card of a thing is known by: its own, or its template's (a thing out of a
/// pocket); a template's is its own.
fn info_key(infos: &ItemInfos, states: &ObjectStates, id: u32) -> u32 {
    let states = &states.states[..(states.count as usize).min(MAX_OBJECTS)];
    states.iter().find(|s| s.id == id).map_or(id, |s| if infos.get(s.id).is_some() || s.desc == 0 { s.id } else { s.desc })
}

/// What the game says a thing is: its own card, or its template's.
fn info_of<'a>(infos: &'a ItemInfos, s: &ObjectState) -> Option<&'a ItemInfo> {
    infos.get(s.id).or_else(|| infos.get(s.desc).filter(|_| s.desc != 0))
}

impl ItemCard {
    pub fn new(gpu: &Gpu) -> Result<ItemCard> {
        let desc = TexDesc {
            width: WIDTH_PX,
            height: HEIGHT_PX,
            format: DXGI_FORMAT_R8G8B8A8_TYPELESS,
            rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM),
            srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
            dsv: None,
            mips: 1,
        };
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        Ok(ItemCard {
            ctx,
            painter: EguiPainter::new(gpu)?,
            tex: Texture::new(gpu, &desc)?,
            infos: bytemuck::zeroed_box(),
            painted: None,
            shown: None,
            at: None,
            key: None,
            opacity: 0.0,
        })
    }

    pub fn srv(&self) -> &ID3D11ShaderResourceView {
        self.tex.srv.as_ref().unwrap()
    }

    /// What the game says its things are (once a frame).
    pub fn read(&mut self, link: &rustvr_protocol::link::Link) {
        if !link.read_item_infos_into(&mut self.infos) {
            self.infos.count = 0;
        }
    }

    /// This frame's card of hand `hand`'s thing ([`pick`]): where it goes and how strongly it shows
    /// (None: not shown). `head`: the eyes' middle (tracking space).
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, gpu: &Gpu, states: &ObjectStates, objects: &ObjectTable, map: Option<&WorldMap>, hand: usize, hand_of: impl Fn(&ObjectState) -> Option<usize>, felt: Option<(u32, Vec3)>, head: Vec3, dt: f32) -> Result<Option<(Panel, f32)>> {
        let now = pick(self.shown, states, objects, &self.infos, map, hand, hand_of, felt);
        match now {
            Some((id, at, top)) => {
                if self.shown != Some(id) {
                    self.at = None;
                }
                self.shown = Some(id);
                self.key = Some(info_key(&self.infos, states, id));
                let toward = (head - at).normalize_or(Vec3::Z);
                let want = at + Vec3::Y * (top + ABOVE) + toward * TOWARDS;
                let k = 1.0 - (-dt / FOLLOW).exp();
                self.at = Some(self.at.map_or(want, |a| a.lerp(want, k)));
                self.opacity = (self.opacity + dt / FADE_IN).min(1.0);
                let info = *info_for(&self.infos, states, id).unwrap();
                if self.painted.is_none_or(|p| bytemuck::bytes_of(&p) != bytemuck::bytes_of(&info)) {
                    self.paint(gpu, &info)?;
                    self.painted = Some(info);
                }
            }
            None => {
                self.opacity = (self.opacity - dt / FADE_OUT).max(0.0);
                if self.opacity == 0.0 {
                    self.shown = None;
                    self.at = None;
                    self.key = None;
                }
            }
        }
        let Some(at) = self.over() else { return Ok(None) };
        // (Standing on the point: its middle half its height over it.)
        let at = at + Vec3::Y * WIDTH * HEIGHT_PX as f32 / WIDTH_PX as f32 * 0.5;
        let d = (at - head).normalize_or(Vec3::Z);
        let (yaw, pitch) = (d.x.atan2(d.z), -(d.y.clamp(-1.0, 1.0)).asin());
        let eased = self.opacity * self.opacity * (3.0 - 2.0 * self.opacity);
        Ok(Some((Panel::facing(at, yaw, pitch, WIDTH, WIDTH_PX as f32 / HEIGHT_PX as f32), eased)))
    }

    /// The point just over the shown thing the card stands on (tracking space), while it shows: a
    /// game's own card stands there too.
    pub fn over(&self) -> Option<Vec3> {
        self.at.filter(|_| self.opacity > 0.0 && self.painted.is_some())
    }

    /// The id the game's card of the shown thing is known by, while it shows.
    pub fn key(&self) -> Option<u32> {
        self.key.filter(|_| self.over().is_some())
    }

    fn paint(&mut self, gpu: &Gpu, info: &ItemInfo) -> Result<()> {
        let size = egui::vec2(WIDTH_PX as f32 / PIXELS_PER_POINT, HEIGHT_PX as f32 / PIXELS_PER_POINT);
        self.ctx.set_pixels_per_point(PIXELS_PER_POINT);
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
        let gold = egui::Color32::from_rgb(232, 199, 122);
        let dim = egui::Color32::from_rgb(170, 158, 130);
        let red = egui::Color32::from_rgb(225, 70, 60);
        let stolen = info.flags & ITEM_STOLEN != 0;
        let mut out = self.ctx.run_ui(input, |ui| {
            let frame = egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(16, 14, 11, 225))
                .stroke(egui::Stroke::new(1.5, if stolen { red } else { gold.gamma_multiply(0.8) }))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(12, 9));
            frame.show(ui, |ui| {
                ui.set_width(size.x - 24.0 - 3.0);
                ui.horizontal(|ui| {
                    let name = if info.count > 1 { format!("{} ({})", info.name(), info.count) } else { info.name().to_string() };
                    ui.label(egui::RichText::new(name).size(17.0).strong().color(gold));
                    if stolen {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new("STOLEN").size(11.0).strong().color(red));
                        });
                    }
                });
                let mut tags = Vec::new();
                if info.flags & ITEM_ENCHANTED != 0 {
                    tags.push("Enchanted");
                }
                if info.flags & ITEM_QUEST != 0 {
                    tags.push("Quest item");
                }
                if !tags.is_empty() {
                    ui.label(egui::RichText::new(tags.join(" · ")).size(11.0).italics().color(egui::Color32::from_rgb(150, 170, 230)));
                }
                ui.add_space(3.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, gold.gamma_multiply(0.35));
                ui.add_space(5.0);
                let stats: Vec<(&str, &str)> = info.stats().collect();
                let columns = if stats.len() > 4 { 3 } else { stats.len().max(1) };
                let cell = ui.available_width() / columns as f32;
                for row in stats.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (label, value) in row {
                            ui.allocate_ui_with_layout(egui::vec2(cell, 34.0), egui::Layout::top_down(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(egui::RichText::new(label.to_uppercase()).size(9.5).color(dim));
                                ui.label(egui::RichText::new(*value).size(15.0).strong().color(egui::Color32::from_rgb(240, 236, 226)));
                            });
                        }
                    });
                }
            });
        });
        let prims = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        self.painter.paint(gpu, self.tex.rtv.as_ref().unwrap(), WIDTH_PX, HEIGHT_PX, out.pixels_per_point, &prims, &mut out.textures_delta, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn tables() -> (Box<ObjectStates>, Box<ObjectTable>, Box<ItemInfos>) {
        let mut states: Box<ObjectStates> = bytemuck::zeroed_box();
        let mut objects: Box<ObjectTable> = bytemuck::zeroed_box();
        let mut infos: Box<ItemInfos> = bytemuck::zeroed_box();
        for (i, (id, flags)) in [(1, 0), (2, OS_PULLED), (3, OS_HELD_RIGHT | OS_TRACKING), (4, OS_HELD_LEFT | OS_TRACKING)].into_iter().enumerate() {
            let mut d = ObjectDesc::zeroed();
            d.id = id;
            d.half_extents = [0.1, 0.1, 0.0];
            objects.descs[i] = d;
            states.states[i] = ObjectState { id, desc: id, flags, pose: Pose { pos: [id as f32, 1.0, 0.0], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] }, ..ObjectState::zeroed() };
            if id != 4 {
                infos.infos[infos.count as usize] = ItemInfo::new(id, "thing");
                infos.count += 1;
            }
        }
        states.count = 4;
        objects.count = 4;
        (states, objects, infos)
    }

    #[test]
    fn the_card_is_for_a_held_thing_the_game_names_then_one_flying_in() {
        let (mut states, objects, infos) = tables();
        let right = |s: &ObjectState| if s.flags & OS_HELD_LEFT != 0 { Some(0) } else { Some(1) };
        let map = WorldMap::IDENTITY;
        // Held (3) beats pulled (2); the held one without a name (4) has none.
        let (id, at, top) = pick(None, &states, &objects, &infos, Some(&map), 1, right, None).unwrap();
        assert_eq!(id, 3);
        assert_eq!(at, Vec3::new(3.0, 1.0, 0.0));
        assert!((top - 0.1).abs() < 1e-6);
        // Shown already and still pulled: it stays.
        assert_eq!(pick(Some(2), &states, &objects, &infos, Some(&map), 1, right, None).unwrap().0, 2);
        // Nothing held: the one flying in.
        states.states[2].flags = 0;
        assert_eq!(pick(Some(3), &states, &objects, &infos, Some(&map), 1, right, None).unwrap().0, 2);
        states.states[1].flags = 0;
        assert!(pick(None, &states, &objects, &infos, Some(&map), 1, right, None).is_none());
    }
}
