//! Placing the game's 2D HUD in VR, piece by piece.
//!
//! A flat game draws its HUD over the whole screen. Hung in front of the face as one sheet, it
//! covers the view and chases the head; none of the well-liked VR shooters do that (see
//! `research_notes/VR throwables and UI/ui.md`). When the adapter describes its HUD
//! ([`HudLayout`]: which rectangle of the HUD texture is the subtitles, the hints, the ammo
//! counter...), each piece goes where native VR games put it:
//!
//! | Role | Where |
//! |---|---|
//! | subtitles | a caption strip 13 degrees under gaze, 1.5 m away, at most ~50 degrees wide |
//! | hints | a row just above the subtitles (6 degrees under gaze) |
//! | prompts | the caption strip's right end |
//! | ammo | on the held gun's left side, facing you (Half-Life: Alyx); hidden with no gun |
//! | status | on the back of the off-hand wrist, shown while you look at it |
//! | overlay | fixed to the face, over the whole view (damage, fades) |
//! | centre and the rest | the flat screen, lazily following the head |
//! | hidden | not drawn (the crosshair: the gun's sights aim) |
//! | gaze | where the head looks, fixed to it, 1.6 m out (what looking there does: "Open Door") |
//! | compass | low in front of the body, lazily following its turn |
//! | target | over the foe it's about |
//!
//! Several status pieces stack up the forearm from the wrist, the first the size the settings say
//! and the others to its scale; several hints stack upwards.
//!
//! Everything that follows the head does so lazily: it stays put while the head turns less than
//! the dead zone (20 degrees), then glides back in front (0.15 s) and stops. It never rolls.
//!
//! The game still decides when a piece shows: its rectangle is drawn every frame, so a piece
//! appears and fades exactly as the game draws it. Without a layout the whole HUD is one flat
//! screen, as before.
//!
//! The compass goes low in front of the body (26 degrees under the horizon, lazily following the
//! head's turn), where a glance down finds it without it sitting in the view; what's said about a
//! foe (its health) floats over the foe.
//!
//! **Menus** come as groups of pieces (`HudGroup`): the parts of a menu that matter, cut out of the
//! game's own screen. Each group floats where it belongs ([`HudPlacer::place_groups`]):
//!
//! | Anchor | Where |
//! |---|---|
//! | a thing in the world | at its distance (0.9 to 1.4 m: the laser's comfortable range), 10 degrees under the eyes, beside it on its side wherever it would otherwise cover it (never more than 40 degrees round) |
//! | the player | 1.1 m ahead of where they faced, 8 degrees under the eyes; it comes round after a turn of 65 degrees |
//! | a pop-up | 0.8 m ahead of where the eyes look, nearer than what's behind it |
//!
//! Once placed a group stays put in the world (it's placed again only when its thing moves, or the
//! head walks away from where it was, gliding over 0.3 s). Its pieces lie on an upright cylinder
//! round the eye it was placed for, each turned to face it, so a wide menu keeps its text at one
//! distance. Every group is sized from the game's own text (`HudLayout::text_height`) so a line of
//! it is `menu_text_deg` degrees tall wherever the group is.

use glam::{Mat4, Quat, Vec3};
use rustvr_protocol::*;

use crate::panels::Panel;
use crate::rig::WorldMap;
use crate::settings::Settings;

/// One piece of the HUD in the world: a rectangle of the HUD texture on a panel.
#[derive(Clone, Copy, Debug)]
pub struct HudPiece {
    pub panel: Panel,
    /// u0, v0, u1, v1 of the HUD texture.
    pub uv: [f32; 4],
    pub opacity: f32,
    pub role: u32,
    /// `HUDE_*`: whether the laser points at it, whether its edges fade.
    pub flags: u32,
}

impl HudPiece {
    /// Where a ray hits it: the distance and the point of the HUD texture it hits.
    pub fn hit(&self, origin: Vec3, dir: Vec3) -> Option<(f32, glam::Vec2)> {
        let (t, p) = self.panel.hit(origin, dir)?;
        let uv = glam::Vec2::new(self.uv[0] + (self.uv[2] - self.uv[0]) * p.x, self.uv[1] + (self.uv[3] - self.uv[1]) * p.y);
        Some((t, uv))
    }
}

/// What the pieces can attach to this frame (tracking space).
#[derive(Clone, Copy, Debug, Default)]
pub struct Attach {
    /// The held gun: weapon -> tracking, and its line of sight (point, direction).
    pub gun: Option<(Mat4, Vec3, Vec3)>,
    /// The off hand's controller grip pose.
    pub off_hand: Option<(Vec3, Quat)>,
    /// Where the card of the thing in a hand goes (over it, eased), and how strongly it shows.
    pub held: Option<(Vec3, f32)>,
    /// Tracking space -> the game's world (for the things pieces float over).
    pub map: Option<WorldMap>,
}

/// How wide the game's own card of a thing in a hand is (metres).
pub const HELD_WIDTH: f32 = 0.2;

/// A head-following anchor with a dead zone: it holds still while the head turns less than `dead`
/// away from it, then catches up until it's within a couple of degrees, and stops.
#[derive(Clone, Copy, Debug, Default)]
struct Lazy {
    angle: f32,
    moving: bool,
}

impl Lazy {
    fn update(&mut self, target: f32, dead: f32, tau: f32, dt: f32) -> f32 {
        let d = crate::wrap_angle(target - self.angle);
        if d.abs() > dead {
            self.moving = true;
        }
        if self.moving {
            self.angle += d * (1.0 - (-dt / tau.max(1e-3)).exp());
            if crate::wrap_angle(target - self.angle).abs() < 2f32.to_radians() {
                self.moving = false;
            }
        }
        self.angle
    }
}

/// A group of a menu's pieces as it was placed in the world (tracking space).
#[derive(Clone, Copy, Debug)]
struct Placed {
    id: u32,
    anchor: u32,
    /// Where its thing was when it was placed.
    thing: Option<Vec3>,
    /// The eye it was placed for: its pieces lie on an upright cylinder round it, facing it.
    eye: Vec3,
    /// The direction of the group's origin from the eye, the origin's height and the cylinder's
    /// radius.
    yaw: f32,
    height: f32,
    distance: f32,
    /// Metres per group unit (the HUD texture's height at the group's size).
    unit: f32,
}

impl Placed {
    /// Part of the way from `self` to `to`.
    fn lerp(&self, to: &Placed, t: f32) -> Placed {
        Placed {
            eye: self.eye.lerp(to.eye, t),
            yaw: self.yaw + crate::wrap_angle(to.yaw - self.yaw) * t,
            height: self.height + (to.height - self.height) * t,
            distance: self.distance + (to.distance - self.distance) * t,
            unit: self.unit + (to.unit - self.unit) * t,
            ..*to
        }
    }
}

/// A group gliding from where it was to where it's placed again.
#[derive(Clone, Copy, Debug)]
struct Glide {
    from: Placed,
    t: f32,
}

/// How long a group glides to where it's placed again (seconds).
const GLIDE: f32 = 0.3;

/// Where the head-following pieces are anchored (yaw and pitch), and where the menus' groups were
/// placed, kept between frames.
#[derive(Default)]
pub struct HudPlacer {
    anchor: Option<(Lazy, Lazy)>,
    groups: Vec<(Placed, Option<Glide>)>,
}

/// The extent of group `group`'s pieces (x0, y0, x1, y1 in its units, y up).
fn footprint(layout: &HudLayout, group: u32, aspect: f32) -> Option<[f32; 4]> {
    let mut b: Option<[f32; 4]> = None;
    for e in layout.entries().iter().filter(|e| e.group == group) {
        let (w, h) = piece_size(e, aspect);
        let r = [e.offset[0] - w * 0.5, e.offset[1] - h * 0.5, e.offset[0] + w * 0.5, e.offset[1] + h * 0.5];
        b = Some(match b {
            None => r,
            Some(b) => [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])],
        });
    }
    b
}

/// A piece's size in its group's units (the HUD texture's height).
fn piece_size(e: &HudElement, aspect: f32) -> (f32, f32) {
    let scale = if e.scale > 0.0 { e.scale } else { 1.0 };
    ((e.uv[2] - e.uv[0]).max(1e-4) * aspect * scale, (e.uv[3] - e.uv[1]).max(1e-4) * scale)
}

/// Metres per unit of the game's UI (the HUD texture's height) per metre away: a line of the
/// game's text (`text_height` of the texture) is `menu_text_deg` degrees tall.
fn unit_per_metre(s: &Settings, layout: &HudLayout) -> f32 {
    let text = if layout.text_height > 0.0 { layout.text_height } else { 0.025 };
    s.ui.menu_text_deg.to_radians().tan() / text
}

impl HudPlacer {
    /// Forget the anchor (the HUD went away): it starts in front of the head next time.
    pub fn reset(&mut self) {
        self.anchor = None;
    }

    /// Places the menus' groups (the layout's pieces that have one) for this frame. `aspect`: the
    /// HUD texture's width / height; `map`: tracking -> the game's world, for the things groups go
    /// beside.
    pub fn place_groups(&mut self, s: &Settings, head_pos: Vec3, head_rot: Quat, dt: f32, aspect: f32, layout: &HudLayout, map: Option<&WorldMap>) -> Vec<HudPiece> {
        let groups = layout.group_entries();
        self.groups.retain(|(p, _)| groups.iter().any(|g| g.id == p.id && g.anchor == p.anchor));
        let fwd = head_rot * Vec3::Z;
        let head_yaw = fwd.x.atan2(fwd.z);
        let head_pitch = -(fwd.y.clamp(-1.0, 1.0)).asin();
        let per_metre = unit_per_metre(s, layout);
        let mut out = Vec::new();
        for (i, g) in groups.iter().enumerate() {
            let n = i as u32 + 1;
            let Some(fp) = footprint(layout, n, aspect) else { continue };
            let thing = map.map(|m| m.inv_point(Vec3::from_array(g.at)));
            let slot = self.groups.iter().position(|(p, _)| p.id == g.id && p.anchor == g.anchor);
            let stale = slot.map(|k| &self.groups[k].0).is_none_or(|p| match g.anchor {
                ANCHOR_WORLD => thing.zip(p.thing).is_some_and(|(t, then)| t.distance(then) > 0.3) || head_pos.distance(p.eye) > 0.8,
                ANCHOR_PLAYER => crate::wrap_angle(head_yaw - p.yaw).abs() > 65f32.to_radians() || head_pos.distance(p.eye) > 0.8,
                _ => false,
            });
            if stale {
                let placed = place_group(g, fp, thing, head_pos, head_yaw, head_pitch, per_metre);
                match slot {
                    // (Placed again: it glides there from where it was.)
                    Some(k) => {
                        let (old, glide) = self.groups[k];
                        let from = glide.map_or(old, |gl| gl.from.lerp(&old, (gl.t / GLIDE).min(1.0)));
                        self.groups[k] = (placed, Some(Glide { from, t: 0.0 }));
                    }
                    None => self.groups.push((placed, None)),
                }
            }
            let k = self.groups.iter().position(|(p, _)| p.id == g.id && p.anchor == g.anchor).unwrap();
            let (to, glide) = &mut self.groups[k];
            let p = match glide {
                Some(gl) => {
                    gl.t += dt;
                    let t = (gl.t / GLIDE).min(1.0);
                    let p = gl.from.lerp(to, t * t * (3.0 - 2.0 * t));
                    if t >= 1.0 {
                        *glide = None;
                    }
                    p
                }
                None => *to,
            };
            for e in layout.entries().iter().filter(|e| e.group == n) {
                let (w, h) = piece_size(e, aspect);
                let (w, h) = (w * p.unit, h * p.unit);
                // Round the cylinder by its offset (an arc), up by its height, nearer by its lift.
                let r = (p.distance - e.offset[2] * p.unit).max(0.3);
                let yaw = p.yaw + e.offset[0] * p.unit / p.distance;
                let y = p.height + e.offset[1] * p.unit;
                let c = Vec3::new(p.eye.x + yaw.sin() * r, y, p.eye.z + yaw.cos() * r);
                let pitch = (p.eye.y - y).atan2(r);
                let mut panel = Panel::facing(c, yaw, pitch, w, w / h.max(1e-4));
                panel.height = h;
                let opacity = if e.opacity > 0.0 { e.opacity } else { 1.0 } * s.ui.menu_opacity;
                out.push(HudPiece { panel, uv: e.uv, opacity, role: e.role, flags: e.flags });
            }
        }
        out
    }

    /// Places this frame's pieces. `layout`: what the adapter said its HUD is (None: one flat
    /// screen); `aspect`: the HUD texture's width / height.
    pub fn place(&mut self, s: &Settings, head_pos: Vec3, head_rot: Quat, dt: f32, aspect: f32, layout: Option<&HudLayout>, at: &Attach) -> Vec<HudPiece> {
        let fwd = head_rot * Vec3::Z;
        let yaw = fwd.x.atan2(fwd.z);
        let pitch = -(fwd.y.clamp(-1.0, 1.0)).asin();
        let u = &s.ui;
        let (ay, ap) = match (&mut self.anchor, u.hud_follow_head) {
            (None, _) => {
                self.anchor = Some((Lazy { angle: yaw, moving: false }, Lazy { angle: pitch, moving: false }));
                (yaw, pitch)
            }
            (Some((ly, lp)), true) => {
                let dead = u.hud_deadzone_deg.to_radians();
                (ly.update(yaw, dead, u.hud_catchup, dt), lp.update(pitch, dead * 0.75, u.hud_catchup, dt))
            }
            (Some((ly, lp)), false) => (ly.angle, lp.angle),
        };
        // The flat screen, as the game drew it (the centre and any piece without a place of its own).
        // Sizes are for a 16:9 layer and kept by height: a game's UI scales with its screen's
        // height, so a narrower layer (a game drawing at an eye's shape) keeps its text's size.
        let by_height = (aspect / (16.0 / 9.0)).min(1.0);
        let screen = Panel::facing(head_pos + Panel::dir(ay, ap) * u.hud_distance, ay, ap, u.hud_size * by_height, aspect);
        let full = [0.0, 0.0, 1.0, 1.0];
        let Some(layout) = layout.filter(|l| l.count > 0 && u.split_hud) else {
            return vec![HudPiece { panel: screen, uv: full, opacity: 1.0, role: 0, flags: 0 }];
        };
        let mut out = Vec::new();
        if layout.flags & HUDL_REST_HIDDEN == 0 {
            out.push(HudPiece { panel: screen, uv: full, opacity: 1.0, role: 0, flags: 0 });
        }
        // The caption strip: the whole layer would be `caption_width_deg` wide at its distance.
        let cd = u.caption_distance;
        let strip_full = 2.0 * cd * (u.caption_width_deg.to_radians() * 0.5).tan() * by_height;
        let strip_max = 2.0 * cd * (u.caption_max_deg.to_radians() * 0.5).tan();
        let per_metre = unit_per_metre(s, layout);
        // (The status pieces share one size, the first's as the settings say; each stacks on the
        // last.)
        let status_unit = layout.entries().iter().find(|e| e.group == 0 && e.role == HUD_STATUS).map(|e| u.status_width / ((e.uv[2] - e.uv[0]).max(1e-4) * aspect));
        let (mut status_stack, mut hints_stack) = (0.0f32, 0.0f32);
        for e in layout.entries().iter().filter(|e| e.group == 0) {
            let scale = if e.scale > 0.0 { e.scale } else { 1.0 };
            let (du, dv) = ((e.uv[2] - e.uv[0]).max(1e-4), (e.uv[3] - e.uv[1]).max(1e-4));
            // A rectangle of the layer `full_w` metres wide (for the whole layer), no wider than `max_w`.
            let sized = |full_w: f32, max_w: f32| {
                let w = (du * full_w * scale).min(max_w);
                (w, w * dv / du / aspect)
            };
            let strip = |yaw_off: f32, pitch_off: f32| {
                let (y, p) = (ay + yaw_off.to_radians(), ap + pitch_off.to_radians());
                let (w, h) = sized(strip_full, strip_max);
                let mut panel = Panel::facing(head_pos + Panel::dir(y, p) * cd, y, p, w, aspect);
                panel.height = h;
                panel
            };
            let piece = match e.role {
                HUD_SUBTITLES => Some((strip(0.0, u.caption_pitch_deg), 1.0)),
                HUD_HINTS => {
                    let mut panel = strip(0.0, u.caption_pitch_deg - 7.0);
                    panel.center += panel.up * hints_stack;
                    hints_stack += panel.height + 0.01;
                    Some((panel, 1.0))
                }
                HUD_PROMPTS => Some((strip(u.caption_max_deg * 0.5 + 6.0, u.caption_pitch_deg), 1.0)),
                HUD_AMMO => at.gun.map(|(gun, sight, dir)| {
                    // Beside the receiver on the gun's left, facing the eye.
                    let left = -(gun.x_axis.truncate().normalize_or(Vec3::X));
                    let up = gun.y_axis.truncate().normalize_or(Vec3::Y);
                    let c = sight + dir * 0.06 + left * 0.07 - up * 0.02;
                    let (w, h) = sized(u.ammo_width / du, f32::MAX);
                    (facing_eye(c, head_pos, w, h), 1.0)
                }),
                HUD_STATUS => at.off_hand.map(|(p, q)| {
                    // On the back of the wrist (a little up the forearm from the grip), and the next
                    // further up it.
                    let unit = status_unit.unwrap_or(1.0) * scale;
                    let (w, h) = (du * aspect * unit, dv * unit);
                    let c = p + q * Vec3::new(0.0, 0.035, -0.09 - status_stack - h * 0.5);
                    status_stack += h + 0.008;
                    let to = (c - head_pos).normalize_or(Vec3::Z);
                    let off = to.angle_between(fwd).to_degrees();
                    let opacity = 1.0 - ((off - 30.0) / 15.0).clamp(0.0, 1.0);
                    (facing_eye(c, head_pos, w, h), opacity)
                }),
                HUD_OVERLAY => {
                    let mut panel = Panel::facing(head_pos + fwd * 1.0, yaw, pitch, 2.0 * 1.0 * 1.25f32 * du, aspect);
                    panel.height = panel.width * dv / du / aspect;
                    Some((panel, 1.0))
                }
                HUD_HELD if at.held.is_some() => at.held.map(|(c, opacity)| {
                    let (w, h) = sized(HELD_WIDTH / du, f32::MAX);
                    (facing_eye(c, head_pos, w, h), opacity)
                }),
                HUD_COMPASS => {
                    // Low in front of the body: the head's lazy turn, a fixed angle under the
                    // horizon (not the head's pitch: looking down finds it).
                    const DISTANCE: f32 = 1.2;
                    let pitch = 26f32.to_radians();
                    let unit = DISTANCE * per_metre * scale;
                    let (w, h) = (du * aspect * unit, dv * unit);
                    let mut panel = Panel::facing(head_pos + Panel::dir(ay, pitch) * DISTANCE, ay, pitch, w, aspect);
                    panel.height = h;
                    Some((panel, 1.0))
                }
                HUD_GAZE => {
                    // Where the head looks, fixed to it (no roll), as big as the game's text there.
                    const DISTANCE: f32 = 1.6;
                    let unit = DISTANCE * per_metre * scale;
                    let (w, h) = (du * aspect * unit, dv * unit);
                    let rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
                    let c = head_pos + rot * Vec3::new(e.offset[0] * unit, e.offset[1] * unit, DISTANCE);
                    let mut panel = Panel::facing(c, yaw, pitch, w, aspect);
                    panel.height = h;
                    Some((panel, 1.0))
                }
                HUD_TARGET => at.map.filter(|_| layout.has_target != 0).map(|m| {
                    // Over the foe, as big as the game's text there would be.
                    let c = m.inv_point(Vec3::from_array(layout.target));
                    let unit = c.distance(head_pos).max(0.5) * per_metre * scale;
                    (facing_eye(c, head_pos, du * aspect * unit, dv * unit), 1.0)
                }),
                HUD_HIDDEN => None,
                // The centre, and anything else: where it was on the flat screen.
                _ => {
                    let c = screen.point(glam::Vec2::new((e.uv[0] + e.uv[2]) * 0.5, (e.uv[1] + e.uv[3]) * 0.5));
                    let mut panel = screen;
                    panel.center = c;
                    panel.width = screen.width * du;
                    panel.height = screen.height * dv;
                    Some((panel, 1.0))
                }
            };
            if let Some((panel, opacity)) = piece
                && opacity > 0.01
            {
                out.push(HudPiece { panel, uv: e.uv, opacity, role: e.role, flags: e.flags });
            }
        }
        out
    }
}

/// Where a group goes when it's first placed (or placed again): see the module's table.
fn place_group(g: &HudGroup, fp: [f32; 4], thing: Option<Vec3>, eye: Vec3, head_yaw: f32, head_pitch: f32, per_metre: f32) -> Placed {
    let deg = |d: f32| d.to_radians();
    // Where the group's middle goes: its direction from the eye (yaw, and pitch under the eye's
    // level) and its distance.
    let (yaw, pitch, distance) = match (g.anchor, thing) {
        (ANCHOR_WORLD, Some(t)) => {
            let v = t - eye;
            let flat = Vec3::new(v.x, 0.0, v.z).length().max(0.1);
            let yaw_t = v.x.atan2(v.z);
            let distance = (flat - g.radius * 0.5).clamp(0.9, 1.4);
            let unit = distance * per_metre;
            // How far round the group and the thing reach from their middles (angles).
            let (half_w, half_h) = ((fp[2] - fp[0]) * 0.5 * unit, (fp[3] - fp[1]) * 0.5 * unit);
            let (bw, bh) = ((half_w / distance).atan(), (half_h / distance).atan());
            let tall = if g.height > 0.0 { g.height } else { g.radius };
            let (top, bottom) = ((eye.y - (t.y + tall)).atan2(flat), (eye.y - (t.y - tall)).atan2(flat));
            let alpha = (g.radius / flat).atan().min(deg(60.0));
            match g.side {
                SIDE_BELOW => (yaw_t, bottom + bh + deg(1.0), distance),
                SIDE_ABOVE => (yaw_t, top - bh - deg(1.0), distance),
                SIDE_FRONT => (yaw_t, (eye.y - t.y).atan2(flat), (flat - g.radius - 0.1).clamp(0.8, 1.4)),
                side => {
                    let pitch = deg(10.0);
                    // Beside it, if it would cover it (a chest on the floor is well under a list at
                    // this height; a merchant's face isn't).
                    let covers = pitch - bh < bottom && pitch + bh > top;
                    let sign = if side == SIDE_LEFT { -1.0 } else { 1.0 };
                    let yaw = if covers { yaw_t + sign * (alpha + bw + deg(2.0)) } else { yaw_t };
                    let off = crate::wrap_angle(yaw - head_yaw).clamp(-deg(40.0), deg(40.0));
                    (head_yaw + off, pitch, distance)
                }
            }
        }
        (ANCHOR_POPUP, _) => (head_yaw, head_pitch.clamp(-deg(5.0), deg(25.0)), 0.8),
        // The player's own menus, and a thing the runtime can't place (no world mapping).
        _ => (head_yaw, deg(8.0), 1.1),
    };
    let unit = distance * per_metre;
    // The group's middle there; its origin is offset from its middle.
    let (cx, cy) = ((fp[0] + fp[2]) * 0.5, (fp[1] + fp[3]) * 0.5);
    let height = eye.y - distance * pitch.tan() - cy * unit;
    Placed { id: g.id, anchor: g.anchor, thing, eye, yaw: yaw - cx * unit / distance, height, distance, unit }
}

/// A `w` x `h` panel at `c`, upright, turned to face the eye at `eye`.
fn facing_eye(c: Vec3, eye: Vec3, w: f32, h: f32) -> Panel {
    let to_eye = eye - c;
    let yaw = (-to_eye.x).atan2(-to_eye.z);
    let flat = Vec3::new(to_eye.x, 0.0, to_eye.z).length();
    let pitch = to_eye.y.atan2(flat);
    let mut p = Panel::facing(c, yaw, pitch, w, w / h.max(1e-4));
    p.height = h;
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metro() -> HudLayout {
        HudLayout::new(
            HUDL_REST_HIDDEN,
            &[
                HudElement::new(HUD_SUBTITLES, [0.15, 0.80, 0.85, 0.93]),
                HudElement::new(HUD_HINTS, [0.15, 0.62, 0.85, 0.80]),
                HudElement::new(HUD_AMMO, [0.78, 0.80, 1.0, 1.0]),
                HudElement::new(HUD_STATUS, [0.0, 0.80, 0.22, 1.0]),
                HudElement::new(HUD_CENTER, [0.25, 0.25, 0.75, 0.62]),
            ],
        )
    }

    fn angle_below(head: Vec3, p: &Panel) -> f32 {
        let d = (p.center - head).normalize();
        (-d.y).asin().to_degrees()
    }

    #[test]
    fn without_a_layout_the_hud_is_one_screen() {
        let s = Settings::default();
        let mut h = HudPlacer::default();
        let head = Vec3::new(0.0, 1.6, 0.0);
        let p = h.place(&s, head, Quat::IDENTITY, 0.011, 16.0 / 9.0, None, &Attach::default());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].uv, [0.0, 0.0, 1.0, 1.0]);
        assert!(p[0].panel.center.abs_diff_eq(head + Vec3::Z * s.ui.hud_distance, 1e-4));
    }

    #[test]
    fn subtitles_sit_low_and_hints_above_them() {
        let s = Settings::default();
        let mut h = HudPlacer::default();
        let head = Vec3::new(0.0, 1.6, 0.0);
        let l = metro();
        let p = h.place(&s, head, Quat::IDENTITY, 0.011, 16.0 / 9.0, Some(&l), &Attach::default());
        let sub = p.iter().find(|p| p.role == HUD_SUBTITLES).unwrap();
        let hint = p.iter().find(|p| p.role == HUD_HINTS).unwrap();
        assert!((angle_below(head, &sub.panel) - s.ui.caption_pitch_deg).abs() < 0.5);
        assert!(angle_below(head, &hint.panel) < angle_below(head, &sub.panel));
        assert!(((sub.panel.center - head).length() - s.ui.caption_distance).abs() < 1e-3);
        // Never wider than the cap.
        let width_deg = 2.0 * (sub.panel.width * 0.5 / s.ui.caption_distance).atan().to_degrees();
        assert!(width_deg <= s.ui.caption_max_deg + 0.1, "{width_deg}");
        // No gun, no off hand: no ammo or status pieces.
        assert!(!p.iter().any(|p| p.role == HUD_AMMO || p.role == HUD_STATUS));
        // The rest is hidden: nothing covers the whole layer.
        assert!(!p.iter().any(|p| p.uv == [0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn the_strip_stays_put_for_small_turns_and_comes_back_for_big_ones() {
        let s = Settings::default();
        let mut h = HudPlacer::default();
        let head = Vec3::new(0.0, 1.6, 0.0);
        let l = metro();
        let at = Attach::default();
        let first = h.place(&s, head, Quat::IDENTITY, 0.011, 16.0 / 9.0, Some(&l), &at)[0].panel.center;
        let small = Quat::from_rotation_y(10f32.to_radians());
        for _ in 0..90 {
            h.place(&s, head, small, 0.011, 16.0 / 9.0, Some(&l), &at);
        }
        let after_small = h.place(&s, head, small, 0.011, 16.0 / 9.0, Some(&l), &at)[0].panel.center;
        assert!(first.abs_diff_eq(after_small, 1e-4), "a 10 degree glance leaves it where it was");
        let big = Quat::from_rotation_y(60f32.to_radians());
        for _ in 0..90 {
            h.place(&s, head, big, 0.011, 16.0 / 9.0, Some(&l), &at);
        }
        let c = h.place(&s, head, big, 0.011, 16.0 / 9.0, Some(&l), &at)[0].panel.center - head;
        let yaw = c.x.atan2(c.z).to_degrees();
        assert!((yaw - 60.0).abs() < 3.0, "turning 60 degrees brings it round in a second: {yaw}");
    }

    #[test]
    fn ammo_goes_on_the_gun_and_status_shows_only_when_looked_at() {
        let s = Settings::default();
        let mut h = HudPlacer::default();
        let head = Vec3::new(0.0, 1.6, 0.0);
        let l = metro();
        let gun = Mat4::from_translation(Vec3::new(0.15, 1.35, 0.35));
        let sight = Vec3::new(0.15, 1.40, 0.30);
        let wrist_ahead = (Vec3::new(-0.05, 1.45, 0.35), Quat::IDENTITY);
        let at = Attach { gun: Some((gun, sight, Vec3::Z)), off_hand: Some(wrist_ahead), held: None, map: None };
        let p = h.place(&s, head, Quat::IDENTITY, 0.011, 16.0 / 9.0, Some(&l), &at);
        let ammo = p.iter().find(|p| p.role == HUD_AMMO).unwrap();
        assert!(ammo.panel.center.distance(sight) < 0.15 && ammo.panel.center.x < sight.x, "{:?}", ammo.panel.center);
        assert!((ammo.panel.width - s.ui.ammo_width).abs() < 1e-4);
        assert!(p.iter().any(|p| p.role == HUD_STATUS), "a wrist held up in front is shown");
        let wrist_down = (Vec3::new(-0.25, 0.9, 0.1), Quat::IDENTITY);
        let at = Attach { off_hand: Some(wrist_down), ..at };
        let p = h.place(&s, head, Quat::IDENTITY, 0.011, 16.0 / 9.0, Some(&l), &at);
        assert!(!p.iter().any(|p| p.role == HUD_STATUS), "a hand at the side isn't");
    }
}
