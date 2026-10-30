//! Tracking space -> game world, applied every game frame with the game's current anchor.
//!
//! Recentring, turning, the vertical anchor smoothing, scripted-camera following, and roomscale
//! (the body follows the head; the body's movement is absorbed into the origin so the view stays
//! where the head really is; beyond the limit the view is held back).
//!
//! It lives in the adapter, not the runtime, because it must use the anchor of the frame being
//! rendered: doing it a frame late in the runtime would make the view jitter while walking.
//!
//! A game's body is often wider than the player's (Oblivion's reaches 30 cm round them), so it
//! stops well short of a table or a wall. Pushed on against it, the view carries on inside the
//! game's body ([`TrackingMapper::set_body_radius`]) until the player's own front is where its edge
//! is: up against the table, never through anything the game's body keeps out.

use glam::{Mat4, Quat, Vec2, Vec3};
use rustvr_protocol::*;

use crate::math::*;

/// How far ahead of the eyes the player's own front is (metres): the view leading a game's body
/// stays this far inside its edge. A head's breadth: the runtime fades the view out as a head this
/// round goes into a wall, so up against one it doesn't.
pub const EYE_TO_FRONT: f32 = 0.12;
/// A push (the stick, or the body walked after the head) of at least this much of a full one that
/// moves the game's body slower than this along it (metres a second) is held up, and the view goes
/// on inside the body at this speed for a full push.
const LEAD_PUSH: f32 = 0.2;
const HELD_UP_SPEED: f32 = 0.1;
const LEAD_SPEED: f32 = 1.0;

/// Where the game's camera is this frame, in world metres (canonical axes, see [`crate::Basis`]).
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    /// The game's eye position as it would render it (with bob, shake and animations).
    pub eye: Vec3,
    /// The eye without bob, shake or animation, if the game has it; otherwise the same as `eye`.
    pub base_eye: Vec3,
    /// The game's own look yaw, used when recentring so the view faces where the game faces.
    pub look_yaw: f32,
    /// The game's camera is being driven by an animation (ladder, cutscene, being grabbed).
    pub scripted: bool,
}

/// The mapping for one game frame.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Tracking space -> world, including the world scale.
    pub tracking_to_world: Mat4,
    /// World yaw of tracking-space forward.
    pub body_yaw: f32,
    pub world_per_meter: f32,
    /// Head pose in the world.
    pub head_pos: Vec3,
    pub head_rot: Quat,
    /// Look angles for the game: yaw from +Z towards +X, pitch positive looking down.
    pub look_yaw: f32,
    pub look_pitch: f32,
    pub look_dir: Vec3,
    /// World point the tracking origin maps onto (the smoothed game eye anchor, and where the view
    /// leads the game's body from it).
    pub anchor: Vec3,
    pub room_offset: Vec2,
    pub scripted: bool,
}

impl View {
    pub fn point(&self, p: Vec3) -> Vec3 {
        self.tracking_to_world.transform_point3(p)
    }

    /// Rotates a tracking-space direction into the world (no scale).
    pub fn dir(&self, d: Vec3) -> Vec3 {
        Quat::from_rotation_y(self.body_yaw) * d
    }

    pub fn rot(&self, q: Quat) -> Quat {
        Quat::from_rotation_y(self.body_yaw) * q
    }

    /// A tracking-space pose in the world.
    pub fn pose(&self, p: &Pose) -> (Vec3, Quat) {
        (self.point(pose_pos(p)), self.rot(pose_rot(p)))
    }

    /// World view matrix for an eye pose (as given in [`RtFrame::eyes`]).
    pub fn eye_view(&self, eye: &Pose) -> Mat4 {
        let (p, r) = self.pose(eye);
        view_matrix(p, r)
    }

    /// The gun in the world. The world scale moves it with the hand but doesn't resize it.
    pub fn weapon_to_world(&self, intents: &Intents) -> Mat4 {
        let w2t = mat(&intents.weapon_to_tracking);
        let pos = self.point(w2t.w_axis.truncate());
        let mut m = Mat4::from_quat(Quat::from_rotation_y(self.body_yaw)) * w2t;
        m.w_axis = pos.extend(1.0);
        m
    }

    /// Converts a world point back into tracking space.
    pub fn to_tracking(&self, p: Vec3) -> Vec3 {
        self.tracking_to_world.inverse().transform_point3(p)
    }

    /// This mapping as a slot reports it ([`rustvr_protocol::SlotMap`]): the tracking point the
    /// anchor is (origin), the yaw, the anchor and the scale.
    pub fn slot_map(&self) -> rustvr_protocol::SlotMap {
        rustvr_protocol::SlotMap {
            origin: self.to_tracking(self.anchor).to_array(),
            body_yaw: self.body_yaw,
            anchor: self.anchor.to_array(),
            world_per_meter: self.world_per_meter,
        }
    }
}

pub struct TrackingMapper {
    active: bool,
    origin: Vec3,
    body_yaw: f32,
    applied_recenter: u32,
    applied_turn: f32,
    ref_smooth_y: f32,
    ref_init: bool,
    last_ref: Option<Vec3>,
    follow_end: f64,
    recenter_pending: bool,
    last: Option<View>,
    /// How far the view may lead the game's body (metres, along the floor), and where it leads it
    /// now (the world, along the floor).
    lead_max: f32,
    lead: Vec3,
}

impl Default for TrackingMapper {
    fn default() -> Self {
        TrackingMapper {
            active: false,
            origin: Vec3::new(0.0, 1.7, 0.0),
            body_yaw: 0.0,
            applied_recenter: 0,
            applied_turn: 0.0,
            ref_smooth_y: 0.0,
            ref_init: false,
            last_ref: None,
            follow_end: -1.0,
            recenter_pending: true,
            last: None,
            lead_max: 0.0,
            lead: Vec3::ZERO,
        }
    }
}

impl TrackingMapper {
    /// Recentre on the next update (e.g. after a level load).
    pub fn recenter(&mut self) {
        self.recenter_pending = true;
    }

    /// The game's body is round, `radius` (metres) from its middle (the anchor) at its edge. While
    /// a push can't move it, the view goes on inside it towards what's holding it up, as far as the
    /// player's own front ([`EYE_TO_FRONT`] ahead of their eyes) reaching its edge. Never further:
    /// whatever the game's body keeps out, the view stays out of. 0 (the default): the view stays on
    /// the body.
    pub fn set_body_radius(&mut self, radius: f32) {
        self.lead_max = (radius - EYE_TO_FRONT).max(0.0);
        self.lead = self.lead.clamp_length_max(self.lead_max);
    }

    pub fn last_view(&self) -> Option<View> {
        self.last
    }

    /// Maps this frame. `now` is any monotonic clock in seconds; `dt` the game's frame time.
    /// Returns None while the runtime doesn't want VR (then render the flat game as usual).
    pub fn update(&mut self, frame: &RtFrame, anchor: &Anchor, now: f64, dt: f32) -> Option<View> {
        let want = frame.flags & RT_ACTIVE != 0 && frame.hmd.valid != 0;
        if !want {
            self.active = false;
            self.ref_init = false;
            self.last_ref = None;
            self.last = None;
            return None;
        }
        let trk = &frame.tracking;
        if !self.active {
            // First frame in VR: face where the game faces, and don't replay old turns.
            self.recenter_pending = true;
            self.applied_turn = trk.turn_total;
            self.applied_recenter = trk.recenter_seq;
            self.active = true;
        }
        if trk.recenter_seq != self.applied_recenter {
            self.applied_recenter = trk.recenter_seq;
            self.recenter_pending = true;
        }
        let hmd_pos = pose_pos(&frame.hmd);
        let hmd_rot = pose_rot(&frame.hmd);
        if std::mem::take(&mut self.recenter_pending) {
            self.origin = hmd_pos;
            self.body_yaw = anchor.look_yaw - yaw_of_quat(hmd_rot);
            self.last_ref = None;
        }
        self.body_yaw = wrap_angle(self.body_yaw + wrap_angle(trk.turn_total - self.applied_turn));
        self.applied_turn = trk.turn_total;

        // Anchor: the game eye without bob or shake. A large gap between the two means a scripted
        // camera animation is moving the view, which we follow (if allowed).
        let scripted = anchor.scripted || anchor.eye.distance(anchor.base_eye) > 0.25;
        let follow_game_camera = trk.flags & TRK_HEAD_BOB != 0 || (scripted && trk.flags & TRK_CAMERA_ANIMS != 0);
        let mut r = if follow_game_camera { anchor.eye } else { anchor.base_eye };
        r.y += trk.height_comp;
        if !self.ref_init {
            self.ref_smooth_y = r.y;
            self.ref_init = true;
        }
        // Smooth the vertical anchor (stairs, crouch transitions): ~0.35 per frame at 90 Hz.
        let alpha = 1.0 - (-dt.max(0.0) / 0.025).exp();
        self.ref_smooth_y += (r.y - self.ref_smooth_y) * alpha;
        r.y = self.ref_smooth_y + trk.height_offset;

        let wpm = 1.0 / trk.world_scale.max(0.1);
        // The lead: a push the game's body was held up on (it moved slower than that along it)
        // carries the view on inside the body. A jump of the body (a load, a door), a recentre or
        // the game's own camera puts the view back on it.
        let mut led = Vec3::ZERO;
        match self.last_ref {
            Some(last) if self.lead_max > 0.0 && !scripted && (r - last).length() < 0.25 => {
                // (The game turns a push by its own heading: +Y ahead, +X to the right.)
                let [mx, my] = frame.intents.move_xy;
                let (s, c) = anchor.look_yaw.sin_cos();
                let push = Vec3::new(s, 0.0, c) * my + Vec3::new(c, 0.0, -s) * mx;
                let m = push.length();
                if m > LEAD_PUSH && dt > 0.0 {
                    let d = push / m;
                    if Vec3::new(r.x - last.x, 0.0, r.z - last.z).dot(d) / dt < HELD_UP_SPEED {
                        let lead = (self.lead + d * (LEAD_SPEED * m.min(1.0) * dt)).clamp_length_max(self.lead_max);
                        led = lead - self.lead;
                        self.lead = lead;
                    }
                }
            }
            _ => self.lead = Vec3::ZERO,
        }
        // While the body is walked after the head, the view is the head's: the lead is taken off
        // the head's way from the body instead, as the body's own movement is below.
        if trk.flags & (TRK_ROOMSCALE | TRK_FOLLOW) == TRK_ROOMSCALE | TRK_FOLLOW {
            let shift_t = (Quat::from_rotation_y(-self.body_yaw) * led) / wpm;
            self.origin.x += shift_t.x;
            self.origin.z += shift_t.z;
        }
        let mut room_offset = Vec2::ZERO;
        if trk.flags & TRK_ROOMSCALE == 0 {
            self.origin.x = hmd_pos.x;
            self.origin.z = hmd_pos.z;
        } else {
            let yaw = Quat::from_rotation_y(self.body_yaw);
            let off_t = Vec3::new(hmd_pos.x - self.origin.x, 0.0, hmd_pos.z - self.origin.z);
            let mut off_w = yaw * off_t * wpm;
            let mut len = off_w.length();
            if trk.flags & TRK_FOLLOW != 0 {
                self.follow_end = now;
            }
            // While the body is walked after the head, its movement toward the head is moved into
            // the origin so the view doesn't move a second time.
            if let Some(last) = self.last_ref
                && !scripted && now - self.follow_end < 0.3 && len > 1e-4 {
                    let moved = Vec3::new(r.x - last.x, 0.0, r.z - last.z);
                    let dir = off_w / len;
                    let along = moved.dot(dir).clamp(0.0, len);
                    if moved.length() < 0.25 && along > 0.0 {
                        let shift_t = (Quat::from_rotation_y(-self.body_yaw) * (dir * along)) / wpm;
                        self.origin.x += shift_t.x;
                        self.origin.z += shift_t.z;
                        off_w -= dir * along;
                        len -= along;
                    }
                }
            let limit = trk.room_limit.max(0.05);
            if len > limit {
                let k = 1.0 - limit / len;
                self.origin.x += (hmd_pos.x - self.origin.x) * k;
                self.origin.z += (hmd_pos.z - self.origin.z) * k;
                off_w *= limit / len;
            }
            room_offset = Vec2::new(off_w.x, off_w.z);
        }
        self.last_ref = Some(r);
        let r = r + self.lead;

        let t2w = Mat4::from_translation(r)
            * Mat4::from_rotation_y(self.body_yaw)
            * Mat4::from_scale(Vec3::splat(wpm))
            * Mat4::from_translation(-self.origin);
        let head_rot = Quat::from_rotation_y(self.body_yaw) * hmd_rot;
        let head_pos = t2w.transform_point3(hmd_pos);
        let look_dir = head_rot * Vec3::Z;
        let view = View {
            tracking_to_world: t2w,
            body_yaw: self.body_yaw,
            world_per_meter: wpm,
            head_pos,
            head_rot,
            look_yaw: yaw_of_dir(look_dir),
            look_pitch: (-look_dir.y.clamp(-1.0, 1.0).asin()).clamp(-1.13, 1.13),
            look_dir,
            anchor: r,
            room_offset,
            scripted,
        };
        self.last = Some(view);
        Some(view)
    }

    /// What to report in [`GameState::tracking`].
    pub fn state(&self) -> TrackingState {
        let Some(v) = self.last else { return TrackingState::default() };
        TrackingState {
            origin: self.origin.to_array(),
            body_yaw: self.body_yaw,
            room_offset: v.room_offset.to_array(),
            world_per_meter: v.world_per_meter,
            applied_recenter: self.applied_recenter,
            look_yaw: v.look_yaw,
            look_pitch: v.look_pitch,
            applied_turn: self.applied_turn,
            _pad: 0.0,
            anchor: v.anchor.to_array(),
            _pad2: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn frame(hmd: Vec3, yaw: f32) -> RtFrame {
        let mut f = RtFrame::zeroed();
        f.flags = RT_ACTIVE;
        f.hmd = pose(hmd, Quat::from_rotation_y(yaw));
        f.tracking.world_scale = 1.0;
        f.tracking.room_limit = 0.6;
        f.tracking.flags = TRK_ROOMSCALE;
        f
    }

    fn anchor(eye: Vec3, yaw: f32) -> Anchor {
        Anchor { eye, base_eye: eye, look_yaw: yaw, scripted: false }
    }

    #[test]
    fn recentre_puts_the_head_on_the_game_eye() {
        let mut m = TrackingMapper::default();
        let game_eye = Vec3::new(10.0, 1.7, -4.0);
        let v = m.update(&frame(Vec3::new(0.3, 1.6, 0.2), 0.4), &anchor(game_eye, 1.0), 0.0, 1.0 / 90.0).unwrap();
        assert!(v.head_pos.abs_diff_eq(game_eye, 1e-4), "{:?}", v.head_pos);
        assert!((v.look_yaw - 1.0).abs() < 1e-4, "faces the game's direction");
    }

    #[test]
    fn turning_rotates_the_body() {
        let mut m = TrackingMapper::default();
        let mut f = frame(Vec3::new(0.0, 1.6, 0.0), 0.0);
        m.update(&f, &anchor(Vec3::ZERO, 0.0), 0.0, 0.01).unwrap();
        f.tracking.turn_total = 0.5;
        let v = m.update(&f, &anchor(Vec3::ZERO, 0.0), 0.01, 0.01).unwrap();
        assert!((v.body_yaw - 0.5).abs() < 1e-5);
        let v = m.update(&f, &anchor(Vec3::ZERO, 0.0), 0.02, 0.01).unwrap();
        assert!((v.body_yaw - 0.5).abs() < 1e-5, "a turn is applied once");
    }

    #[test]
    fn walking_in_the_room_moves_the_view_until_the_limit() {
        let mut m = TrackingMapper::default();
        let eye = Vec3::new(0.0, 1.7, 0.0);
        m.update(&frame(Vec3::new(0.0, 1.7, 0.0), 0.0), &anchor(eye, 0.0), 0.0, 0.01).unwrap();
        let v = m.update(&frame(Vec3::new(0.0, 1.7, 0.4), 0.0), &anchor(eye, 0.0), 0.01, 0.01).unwrap();
        assert!((v.head_pos.z - 0.4).abs() < 1e-4, "head moved 40 cm forward: {:?}", v.head_pos);
        assert!((v.room_offset.y - 0.4).abs() < 1e-4);
        let v = m.update(&frame(Vec3::new(0.0, 1.7, 1.5), 0.0), &anchor(eye, 0.0), 0.02, 0.01).unwrap();
        assert!((v.head_pos.z - 0.6).abs() < 1e-4, "held back at the 60 cm limit: {:?}", v.head_pos);
    }

    #[test]
    fn following_body_is_absorbed() {
        let mut m = TrackingMapper::default();
        let mut f = frame(Vec3::new(0.0, 1.7, 0.0), 0.0);
        m.update(&f, &anchor(Vec3::new(0.0, 1.7, 0.0), 0.0), 0.0, 0.01).unwrap();
        f.hmd.pos = [0.0, 1.7, 0.3];
        let before = m.update(&f, &anchor(Vec3::new(0.0, 1.7, 0.0), 0.0), 0.01, 0.01).unwrap();
        // The runtime starts walking the game body toward the head; the game moves it 10 cm.
        f.tracking.flags |= TRK_FOLLOW;
        let after = m.update(&f, &anchor(Vec3::new(0.0, 1.7, 0.1), 0.0), 0.02, 0.01).unwrap();
        assert!(after.head_pos.abs_diff_eq(before.head_pos, 1e-4), "the view stays put: {:?} vs {:?}", after.head_pos, before.head_pos);
        assert!((after.room_offset.y - 0.2).abs() < 1e-4, "the offset shrinks: {:?}", after.room_offset);
    }

    #[test]
    fn weapon_keeps_its_size_in_a_scaled_world() {
        let mut m = TrackingMapper::default();
        let mut f = frame(Vec3::new(0.0, 1.7, 0.0), 0.0);
        f.tracking.world_scale = 2.0;
        let v = m.update(&f, &anchor(Vec3::ZERO, 0.0), 0.0, 0.01).unwrap();
        f.intents.weapon_to_tracking = Mat4::from_translation(Vec3::new(0.0, 1.2, 0.4)).to_cols_array();
        let w = v.weapon_to_world(&f.intents);
        assert!((w.x_axis.length() - 1.0).abs() < 1e-5, "not resized");
        assert!(w.w_axis.truncate().abs_diff_eq(v.point(Vec3::new(0.0, 1.2, 0.4)), 1e-5));
    }
}
