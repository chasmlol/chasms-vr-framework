//! Flat panels in tracking space: the game's menu, its HUD, and the runtime's own settings menu,
//! plus pointing at them with the laser.

use glam::{Quat, Vec2, Vec3};

use crate::settings::Settings;

#[derive(Clone, Copy, Debug)]
pub struct Panel {
    pub center: Vec3,
    /// Unit vectors along the panel's width (left -> right) and height (bottom -> top).
    pub right: Vec3,
    pub up: Vec3,
    pub width: f32,
    pub height: f32,
}

impl Panel {
    /// Facing direction (towards the viewer).
    pub fn normal(&self) -> Vec3 {
        self.up.cross(self.right).normalize()
    }

    /// Where a ray hits the panel: distance and UV (0..1, top-left origin).
    pub fn hit(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec2)> {
        let n = self.normal();
        let denom = dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (self.center - origin).dot(n) / denom;
        if t <= 0.0 {
            return None;
        }
        let p = origin + dir * t - self.center;
        let u = p.dot(self.right) / self.width + 0.5;
        let v = 0.5 - p.dot(self.up) / self.height;
        if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) { Some((t, Vec2::new(u, v))) } else { None }
    }

    /// The point at a UV.
    pub fn point(&self, uv: Vec2) -> Vec3 {
        self.center + self.right * (uv.x - 0.5) * self.width + self.up * (0.5 - uv.y) * self.height
    }

    /// The direction `yaw` to the right of +Z and `pitch` below the horizon.
    pub fn dir(yaw: f32, pitch: f32) -> Vec3 {
        Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Vec3::Z
    }

    /// An upright panel at `center` facing back along (`yaw`, `pitch`), `width` wide.
    pub fn facing(center: Vec3, yaw: f32, pitch: f32, width: f32, aspect: f32) -> Panel {
        let rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
        Panel { center, right: rot * Vec3::X, up: rot * Vec3::Y, width, height: width / aspect.max(0.1) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelId {
    GameMenu,
    Hud,
    RuntimeMenu,
}

#[derive(Default)]
pub struct Layout {
    menu_anchor: Option<(Vec3, f32)>,
    hud: Option<(f32, f32)>,
}

pub struct Frame {
    pub game_menu: Option<Panel>,
    pub hud: Option<Panel>,
    pub runtime_menu: Option<Panel>,
}

impl Layout {
    /// Places this frame's panels. `menu`: the game shows a menu; `hud`: it has a HUD layer;
    /// `runtime_menu`: the runtime's settings panel is open. The menus stand before `menu_eye` (the
    /// head at its upright height), the HUD before the head.
    pub fn update(
        &mut self,
        s: &Settings,
        head_pos: Vec3,
        menu_eye: Vec3,
        head_rot: Quat,
        dt: f32,
        menu: Option<f32>,
        hud: Option<f32>,
        runtime_menu: Option<f32>,
    ) -> Frame {
        let fwd = head_rot * Vec3::Z;
        let yaw = fwd.x.atan2(fwd.z);
        let mut out = Frame { game_menu: None, hud: None, runtime_menu: None };

        // Menus: world-locked in front of the user's facing, re-anchored when they turn away
        // (more than 70 degrees) from it.
        if menu.is_some() || runtime_menu.is_some() {
            let re_anchor = match self.menu_anchor {
                None => true,
                Some((_, a)) => crate::wrap_angle(yaw - a).abs() > 70f32.to_radians(),
            };
            if re_anchor {
                let d = s.ui.menu_distance;
                self.menu_anchor = Some((menu_eye + Vec3::new(yaw.sin() * d, 0.0, yaw.cos() * d), yaw));
            }
            let (pos, a) = self.menu_anchor.unwrap();
            let right = Vec3::new(a.cos(), 0.0, -a.sin());
            if let Some(aspect) = menu {
                out.game_menu = Some(Panel::facing(pos, a, 0.0, s.ui.menu_size, aspect));
                if let Some(side_aspect) = runtime_menu {
                    // The settings panel floats to the right of the game's menu, turned toward you.
                    let side_w = s.ui.menu_size * 0.55;
                    let c = pos + right * (s.ui.menu_size * 0.5 + side_w * 0.5 + 0.1) - Vec3::new(a.sin(), 0.0, a.cos()) * 0.25;
                    out.runtime_menu = Some(Panel::facing(c, a + 20f32.to_radians(), 0.0, side_w, side_aspect));
                }
            } else if let Some(side_aspect) = runtime_menu {
                out.runtime_menu = Some(Panel::facing(pos, a, 0.0, s.ui.menu_size * 0.6, side_aspect));
            }
        } else {
            self.menu_anchor = None;
        }

        // HUD: in front of the head, lazily following it (or fixed once placed).
        if let Some(aspect) = hud {
            let pitch = -(fwd.y.clamp(-1.0, 1.0)).asin();
            let (hy, hp) = match self.hud {
                Some((hy, hp)) if s.ui.hud_follow_head => {
                    let k = 1.0 - (-dt / 0.25).exp();
                    (hy + crate::wrap_angle(yaw - hy) * k, hp + (pitch - hp) * k)
                }
                Some(v) => v,
                None => (yaw, pitch),
            };
            self.hud = Some((hy, hp));
            let rot = Quat::from_rotation_y(hy) * Quat::from_rotation_x(hp);
            let center = head_pos + rot * Vec3::new(0.0, 0.0, s.ui.hud_distance);
            out.hud = Some(Panel::facing(center, hy, hp, s.ui.hud_size, aspect));
        } else {
            self.hud = None;
        }
        out
    }
}

impl Frame {
    /// The nearest panel the ray points at (menus only: the HUD isn't interactive).
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<(PanelId, f32, Vec2)> {
        let mut best: Option<(PanelId, f32, Vec2)> = None;
        for (id, p) in [(PanelId::GameMenu, self.game_menu), (PanelId::RuntimeMenu, self.runtime_menu)] {
            if let Some(p) = p
                && let Some((t, uv)) = p.hit(origin, dir)
                    && best.is_none_or(|b| t < b.1) {
                        best = Some((id, t, uv));
                    }
        }
        best
    }

    pub fn get(&self, id: PanelId) -> Option<&Panel> {
        match id {
            PanelId::GameMenu => self.game_menu.as_ref(),
            PanelId::Hud => self.hud.as_ref(),
            PanelId::RuntimeMenu => self.runtime_menu.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laser_hits_the_menu_where_it_points() {
        let s = Settings::default();
        let mut l = Layout::default();
        let head = Vec3::new(0.0, 1.6, 0.0);
        let f = l.update(&s, head, head, Quat::IDENTITY, 0.01, Some(16.0 / 9.0), None, None);
        let menu = f.game_menu.unwrap();
        assert!(menu.center.abs_diff_eq(Vec3::new(0.0, 1.6, 2.2), 1e-5));
        let (id, t, uv) = f.pick(head, Vec3::Z).unwrap();
        assert_eq!(id, PanelId::GameMenu);
        assert!((t - 2.2).abs() < 1e-4 && uv.abs_diff_eq(Vec2::splat(0.5), 1e-4));
        // Pointing at the top-left corner region.
        let corner = menu.point(Vec2::new(0.1, 0.1));
        let (_, _, uv) = f.pick(head, (corner - head).normalize()).unwrap();
        assert!(uv.abs_diff_eq(Vec2::new(0.1, 0.1), 1e-4), "{uv:?}");
    }

    #[test]
    fn menu_reanchors_only_after_turning_away() {
        let s = Settings::default();
        let mut l = Layout::default();
        let head = Vec3::new(0.0, 1.6, 0.0);
        let a = l.update(&s, head, head, Quat::IDENTITY, 0.01, Some(1.5), None, None).game_menu.unwrap();
        let b = l.update(&s, head, head, Quat::from_rotation_y(0.5), 0.01, Some(1.5), None, None).game_menu.unwrap();
        assert!(a.center.abs_diff_eq(b.center, 1e-5), "a small turn keeps it put");
        let c = l.update(&s, head, head, Quat::from_rotation_y(1.6), 0.01, Some(1.5), None, None).game_menu.unwrap();
        assert!(c.center.x > 2.0, "turning 90 degrees brings it round: {:?}", c.center);
    }

    #[test]
    fn hud_lags_behind_a_head_turn() {
        let s = Settings::default();
        let mut l = Layout::default();
        let head = Vec3::new(0.0, 1.6, 0.0);
        l.update(&s, head, head, Quat::IDENTITY, 0.01, None, Some(1.6), None);
        let h = l.update(&s, head, head, Quat::from_rotation_y(1.0), 0.011, None, Some(1.6), None).hud.unwrap();
        let yaw = (h.center - head).x.atan2((h.center - head).z);
        assert!(yaw > 0.0 && yaw < 0.2, "follows smoothly, not instantly: {yaw}");
    }
}
