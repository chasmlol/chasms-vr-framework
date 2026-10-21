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
//!
//! Everything that follows the head does so lazily: it stays put while the head turns less than
//! the dead zone (20 degrees), then glides back in front (0.15 s) and stops. It never rolls.
//!
//! The game still decides when a piece shows: its rectangle is drawn every frame, so a piece
//! appears and fades exactly as the game draws it. Without a layout the whole HUD is one flat
//! screen, as before.

use glam::{Mat4, Quat, Vec3};
use rustvr_protocol::*;

use crate::panels::Panel;
use crate::settings::Settings;

/// One piece of the HUD in the world: a rectangle of the HUD texture on a panel.
#[derive(Clone, Copy, Debug)]
pub struct HudPiece {
    pub panel: Panel,
    /// u0, v0, u1, v1 of the HUD texture.
    pub uv: [f32; 4],
    pub opacity: f32,
    pub role: u32,
}

/// What the pieces can attach to this frame (tracking space).
#[derive(Clone, Copy, Debug, Default)]
pub struct Attach {
    /// The held gun: weapon -> tracking, and its line of sight (point, direction).
    pub gun: Option<(Mat4, Vec3, Vec3)>,
    /// The off hand's controller grip pose.
    pub off_hand: Option<(Vec3, Quat)>,
}

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

/// Where the head-following pieces are anchored (yaw and pitch), kept between frames.
#[derive(Default)]
pub struct HudPlacer {
    anchor: Option<(Lazy, Lazy)>,
}

impl HudPlacer {
    /// Forget the anchor (the HUD went away): it starts in front of the head next time.
    pub fn reset(&mut self) {
        self.anchor = None;
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
            return vec![HudPiece { panel: screen, uv: full, opacity: 1.0, role: 0 }];
        };
        let mut out = Vec::new();
        if layout.flags & HUDL_REST_HIDDEN == 0 {
            out.push(HudPiece { panel: screen, uv: full, opacity: 1.0, role: 0 });
        }
        // The caption strip: the whole layer would be `caption_width_deg` wide at its distance.
        let cd = u.caption_distance;
        let strip_full = 2.0 * cd * (u.caption_width_deg.to_radians() * 0.5).tan() * by_height;
        let strip_max = 2.0 * cd * (u.caption_max_deg.to_radians() * 0.5).tan();
        for e in layout.entries() {
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
                HUD_HINTS => Some((strip(0.0, u.caption_pitch_deg - 7.0), 1.0)),
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
                    // On the back of the wrist (a little up the forearm from the grip).
                    let c = p + q * Vec3::new(0.0, 0.035, -0.09);
                    let (w, h) = sized(u.status_width / du, f32::MAX);
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
                out.push(HudPiece { panel, uv: e.uv, opacity, role: e.role });
            }
        }
        out
    }
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
        let at = Attach { gun: Some((gun, sight, Vec3::Z)), off_hand: Some(wrist_ahead) };
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
