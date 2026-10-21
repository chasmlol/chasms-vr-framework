//! Gun recoil, after Escape from Tarkov's (`NewRecoilShotEffect` and `NewRotationRecoilProcess`;
//! the user's Rust port is the reference), adapted to a gun in a tracked hand.
//!
//! Each shot drives three layers, all as offsets of the gun from the hand that holds it (the hand
//! drawn on the gun goes with it; the controller doesn't):
//!
//! * **Snap**: a fast, stiff flip and push back: the violent first ~50 ms of a shot.
//! * **Rotation process** (EFT's hand rotation recoil): the shot's kick leans a random angle off
//!   straight up (wider as a burst goes on), the first shot of a burst kicks hardest, and the
//!   return spring is weak while you keep firing, so full auto climbs. After a few shots it goes
//!   into *stable mode*: the climb plateaus and wanders around a stable point. When you stop, the
//!   gun comes back down along a curved, sideways return, and settles a little up and off where
//!   it started: you pull it back on target yourself. That rest offset fades over a few seconds.
//! * **Position**: the gun drives back along the barrel and lifts slightly, then springs forward.
//!
//! A second hand and a shouldered stock tame the rotation and speed the return, as EFT's mount
//! multipliers do. The springs run at a fixed 240 Hz so the feel doesn't change with frame rate,
//! and the randomness is a per-gun seeded generator, so tests are repeatable.

use glam::{Quat, Vec2, Vec3};
use rustvr_protocol::*;

/// The fixed step the springs run at.
const STEP: f32 = 1.0 / 240.0;

/// What a gun's recoil is like (from its descriptor, with defaults by type).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecoilParams {
    /// One shot's kick: muzzle climb (rad), sideways spread (rad), push back (m).
    pub kick: Vec3,
    /// Share of each kick that stays while firing on full auto (0..1).
    pub climb: f32,
    /// Return speed after a shot (1/s): the rotation spring's natural frequency.
    pub ret: f32,
    /// How far from straight up a shot's kick may lean, either way (rad).
    pub angle: f32,
    /// Share of the climb carried by the fast snap.
    pub snap: f32,
}

impl RecoilParams {
    pub fn of(d: &ObjectDesc) -> RecoilParams {
        let pistol = d.weapon_flags & WPN_PISTOL != 0;
        let or = |v: f32, default: f32| if v > 0.0 { v } else { default };
        RecoilParams {
            kick: Vec3::new(or(d.recoil_pitch, if pistol { 10.0 } else { 6.0 }).to_radians(), d.recoil_yaw.max(0.0).to_radians(), or(d.recoil_back, if pistol { 0.025 } else { 0.02 })),
            climb: or(d.recoil_climb, if pistol { 0.4 } else { 0.65 }).clamp(0.0, 0.95),
            ret: or(d.recoil_return, if pistol { 14.0 } else { 9.0 }),
            angle: or(d.recoil_angle, if pistol { 8.0 } else { 12.0 }).to_radians(),
            snap: if pistol { 0.55 } else { 0.4 },
        }
    }
}

/// How the gun is held for one shot: multipliers on the rotation kick, the push back and the
/// return speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold {
    pub rotation: f32,
    pub position: f32,
    pub ret: f32,
}

impl Hold {
    pub const ONE_HAND: Hold = Hold { rotation: 1.0, position: 1.0, ret: 1.0 };
    pub const TWO_HANDS: Hold = Hold { rotation: 0.55, position: 0.8, ret: 1.35 };
    pub const SHOULDERED: Hold = Hold { rotation: 0.4, position: 0.6, ret: 1.6 };
}

impl Default for Hold {
    fn default() -> Self {
        Hold::ONE_HAND
    }
}

/// Shots this close together are one burst.
const BURST_GAP: f32 = 0.25;
/// From this shot of a burst on, the climb plateaus (EFT's `RecoilStableIndexShot`).
const STABLE_SHOT: u32 = 4;
/// Each stable-mode shot leans this much wider (EFT's `RecoilStableAngleIncreaseStep`), up to the max.
const STABLE_ANGLE_STEP: f32 = 2.5;
const STABLE_ANGLE_MAX: f32 = 20.0;
/// The first shot of a burst kicks harder (EFT's shot groups).
const FIRST_SHOT: f32 = 1.25;
/// The rest offset left after a burst fades with this time constant (s).
const REST_FADE: f32 = 2.5;
/// Impulse calibration of the two rotation layers (see `kick`).
const MAIN_GAIN: f32 = 1.6;
const SNAP_GAIN: f32 = 1.6;

#[derive(Clone, Copy, Debug)]
pub struct Recoil {
    /// The current offset: x = muzzle climb (rad), y = sideways (rad), z = push back (m).
    pub offset: Vec3,
    /// Roll (rad) and lift (m), kept apart from the main three.
    pub roll: f32,
    pub lift: f32,
    params: RecoilParams,
    hold: Hold,
    // rotation process: pitch, yaw, roll
    rot: Vec3,
    rot_vel: Vec3,
    // snap: pitch, back
    snap: Vec2,
    snap_vel: Vec2,
    // position: back, lift
    pos: Vec2,
    pos_vel: Vec2,
    /// Where the rotation settles (the after-recoil offset), pitch and yaw.
    rest: Vec2,
    rest_target: Vec2,
    shot_index: u32,
    since_shot: f32,
    stable: Option<Vec2>,
    stable_lean: f32,
    return_side: f32,
    returning: bool,
    acc: f32,
    rng: u32,
}

impl Default for Recoil {
    fn default() -> Self {
        Recoil::new(0x9E37_79B9)
    }
}

impl Recoil {
    pub fn new(seed: u32) -> Recoil {
        Recoil {
            offset: Vec3::ZERO,
            roll: 0.0,
            lift: 0.0,
            params: RecoilParams { kick: Vec3::ZERO, climb: 0.65, ret: 9.0, angle: 0.2, snap: 0.4 },
            hold: Hold::ONE_HAND,
            rot: Vec3::ZERO,
            rot_vel: Vec3::ZERO,
            snap: Vec2::ZERO,
            snap_vel: Vec2::ZERO,
            pos: Vec2::ZERO,
            pos_vel: Vec2::ZERO,
            rest: Vec2::ZERO,
            rest_target: Vec2::ZERO,
            shot_index: 0,
            since_shot: 10.0,
            stable: None,
            stable_lean: 0.0,
            return_side: 1.0,
            returning: false,
            acc: 0.0,
            rng: seed | 1,
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.rand()
    }

    /// Consecutive shots in the current burst.
    pub fn burst(&self) -> u32 {
        self.shot_index
    }

    /// One shot.
    pub fn kick(&mut self, d: &ObjectDesc, hold: Hold) {
        let p = RecoilParams::of(d);
        self.params = p;
        self.hold = hold;
        if self.since_shot > BURST_GAP {
            self.shot_index = 0;
            self.stable = None;
            self.stable_lean = 0.0;
        }
        self.shot_index += 1;
        self.since_shot = 0.0;
        self.returning = false;
        let first = if self.shot_index == 1 { FIRST_SHOT } else { 1.0 };
        // Follow-up shots of a burst add less climb than the first (each still snaps).
        let follow = match self.shot_index {
            1 => 1.0,
            2..=3 => 0.75,
            _ => 0.6,
        };
        // EFT: the kick's direction leans a random angle off straight up, wider in stable mode.
        if self.shot_index >= STABLE_SHOT {
            if self.stable.is_none() {
                self.stable = Some(self.rot.truncate());
            }
            self.stable_lean = (self.stable_lean + STABLE_ANGLE_STEP).min(STABLE_ANGLE_MAX);
        }
        let lean_max = p.angle + self.stable_lean.to_radians();
        let lean = self.range(-lean_max, lean_max);
        let power = p.kick.x * self.range(0.85, 1.15) * first * hold.rotation;
        let up = power * lean.cos();
        let side = power * lean.sin() + p.kick.y * self.range(-1.0, 1.0) * hold.rotation;
        // Rotation process: an impulse sized so that, with the snap on top, one shot flips the gun
        // about its full per-shot kick (calibrated: the post-shot spring is soft and well damped,
        // so the layers peak at different times).
        let w = p.ret * hold.ret;
        let gain = w / 0.46 * MAIN_GAIN;
        let main = (1.0 - p.snap) * follow;
        let roll = power * 0.15 * self.range(-1.0, 1.0);
        self.rot_vel += Vec3::new(up * main, side * main, roll) * gain;
        // Snap: the fast flip and the push back.
        let ws = std::f32::consts::TAU * 6.0;
        let back = p.kick.z * self.range(0.9, 1.1) * hold.position;
        self.snap_vel += Vec2::new(up * p.snap, 0.0) * (ws / 0.46 * SNAP_GAIN);
        let wp = std::f32::consts::TAU * 5.0;
        self.pos_vel += Vec2::new(back, back * 0.2) * (wp / 0.5);
        // Stable mode: the plateau creeps up a little and wanders with each shot.
        if let Some(s) = &mut self.stable {
            s.x += up * 0.12;
            s.y += side * 0.3;
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.acc += dt.clamp(0.0, 0.1);
        while self.acc >= STEP {
            self.acc -= STEP;
            self.step(STEP);
        }
        self.offset = Vec3::new(self.rot.x + self.snap.x, self.rot.y, self.pos.x + self.snap.y);
        self.roll = self.rot.z;
        self.lift = self.pos.y;
    }

    fn step(&mut self, h: f32) {
        let p = self.params;
        self.since_shot += h;
        let firing = self.since_shot < BURST_GAP;
        let w = p.ret * self.hold.ret;
        // EFT's return speed curve: weak right after a shot (so a burst climbs), full by ~0.35 s.
        let t = (self.since_shot / 0.35).clamp(0.0, 1.0);
        let curve = t * t * (3.0 - 2.0 * t);
        let k = w * w * (1.0 - p.climb * (1.0 - curve));
        let c = 2.0 * 0.7 * w;
        // In stable mode the burst hovers around its stable point instead of climbing on.
        let rest = match self.stable {
            Some(s) if firing => s,
            _ => self.rest,
        };
        let stiff = if self.stable.is_some() && firing { w * w } else { k };
        let to_rest = self.rot - rest.extend(0.0);
        let acc = -Vec3::new(stiff, stiff, w * w) * to_rest - c * self.rot_vel;
        self.rot_vel += acc * h;
        // The return trajectory: coming back down, the gun drifts to one side on a curve.
        if !firing && self.rot_vel.x < 0.0 {
            if !self.returning {
                self.returning = true;
                self.return_side = if self.rot.y.abs() > 1e-4 { self.rot.y.signum() } else if self.rand() < 0.5 { -1.0 } else { 1.0 };
                // EFT's after-recoil offset: where this burst leaves the gun.
                let peak = self.rot.x.max(0.0);
                let up = self.range(0.06, 0.16);
                let side = self.range(0.2, 0.5);
                self.rest_target = Vec2::new(peak * up, self.rot.y * side);
                self.stable = None;
                self.stable_lean = 0.0;
            }
            self.rot_vel.y += self.return_side * self.rot_vel.x.abs() * 0.25 * h * w;
        }
        self.rot += self.rot_vel * h;
        // The rest point eases to the burst's offset, which itself fades away.
        self.rest += (self.rest_target - self.rest) * (1.0 - (-h / 0.15).exp());
        self.rest_target *= (-h / REST_FADE).exp();
        // Snap and position: stiff, quick springs.
        let ws = std::f32::consts::TAU * 6.0;
        let a = -ws * ws * self.snap - 2.0 * 0.6 * ws * self.snap_vel;
        self.snap_vel += a * h;
        self.snap += self.snap_vel * h;
        let wp = std::f32::consts::TAU * 5.0;
        let a = -wp * wp * self.pos - 2.0 * 0.5 * wp * self.pos_vel;
        self.pos_vel += a * h;
        self.pos += self.pos_vel * h;
        // Never flip the gun over, however long the burst.
        self.rot = self.rot.clamp(Vec3::new(-0.3, -0.6, -0.4), Vec3::new(1.2, 0.6, 0.4));
    }

    /// The offset as a transform in weapon space about `pivot` (the grip): (rotation, translation).
    pub fn transform(&self, pivot: Vec3) -> (Quat, Vec3) {
        let r = Quat::from_rotation_y(self.offset.y) * Quat::from_rotation_x(-self.offset.x) * Quat::from_rotation_z(self.roll);
        let t = pivot - r * pivot + Vec3::new(0.0, self.lift, -self.offset.z);
        (r, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn rifle() -> ObjectDesc {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.weapon_flags = WPN_AUTOMATIC;
        d.recoil_pitch = 7.0;
        d.recoil_yaw = 1.8;
        d.recoil_back = 0.025;
        d
    }

    fn pistol() -> ObjectDesc {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.weapon_flags = WPN_PISTOL;
        d.recoil_pitch = 12.0;
        d.recoil_back = 0.03;
        d
    }

    /// Runs `seconds`, firing at `rate` per second for the first `shots` shots; returns the pitch
    /// (degrees) every frame.
    fn fire(d: &ObjectDesc, hold: Hold, shots: u32, rate: f32, seconds: f32) -> (Vec<f32>, Recoil) {
        let mut r = Recoil::new(7);
        let dt = 1.0 / 90.0;
        let mut out = Vec::new();
        let mut next = 0.0f32;
        let mut fired = 0;
        let mut t = 0.0f32;
        while t < seconds {
            if fired < shots && t >= next {
                r.kick(d, hold);
                fired += 1;
                next += 1.0 / rate;
            }
            r.update(dt);
            out.push(r.offset.x.to_degrees());
            t += dt;
        }
        (out, r)
    }

    #[test]
    fn a_single_shot_kicks_hard_and_snaps_back_close_to_where_it_was() {
        let d = rifle();
        let (pitch, r) = fire(&d, Hold::ONE_HAND, 1, 1.0, 1.0);
        let peak = pitch.iter().cloned().fold(0.0, f32::max);
        assert!((6.0..12.0).contains(&peak), "the muzzle flips about the gun's 7 degrees (first shot harder): {peak:.1}");
        let at = |s: f32| pitch[(s * 90.0) as usize];
        assert!(at(0.6).abs() < 1.5, "mostly back by 0.6 s: {:.2}", at(0.6));
        let (_, mut r) = (pitch, r);
        for _ in 0..(6 * 90) {
            r.update(1.0 / 90.0);
        }
        assert!(r.offset.x.to_degrees().abs() < 0.1, "and fully re-centred after a few seconds: {:.3}", r.offset.x.to_degrees());
    }

    #[test]
    fn it_settles_a_little_off_target_for_you_to_correct() {
        let d = rifle();
        let (pitch, _) = fire(&d, Hold::ONE_HAND, 3, 8.0, 1.2);
        let settled = pitch[(1.1 * 90.0) as usize];
        assert!(settled > 0.15, "the after-recoil offset leaves the muzzle up: {settled:.2} degrees");
    }

    #[test]
    fn full_auto_climbs_then_plateaus() {
        let d = rifle();
        let (pitch, _) = fire(&d, Hold::ONE_HAND, 12, 10.0, 1.4);
        let at_shot = |n: f32| pitch[((n - 0.5) / 10.0 * 90.0) as usize];
        let peak = pitch.iter().cloned().fold(0.0, f32::max);
        assert!(peak > 12.0 && peak < 45.0, "a one-handed burst climbs well past one shot: {peak:.1}");
        assert!(at_shot(4.0) > at_shot(1.0) + 3.0, "it walks up through the first shots: {:.1} -> {:.1}", at_shot(1.0), at_shot(4.0));
        assert!((at_shot(11.0) - at_shot(7.0)).abs() < 6.0, "then plateaus in stable mode: {:.1} vs {:.1}", at_shot(7.0), at_shot(11.0));
    }

    #[test]
    fn bursts_wander_sideways() {
        let d = rifle();
        let mut r = Recoil::new(3);
        let mut yaw_range = (0.0f32, 0.0f32);
        for i in 0..120 {
            if i % 9 == 0 && i < 108 {
                r.kick(&d, Hold::ONE_HAND);
            }
            r.update(1.0 / 90.0);
            yaw_range = (yaw_range.0.min(r.offset.y), yaw_range.1.max(r.offset.y));
        }
        assert!((yaw_range.1 - yaw_range.0).to_degrees() > 1.0, "the muzzle wanders sideways: {:?}", yaw_range);
    }

    #[test]
    fn two_hands_and_a_stock_tame_it() {
        let d = rifle();
        let peak = |hold| fire(&d, hold, 10, 10.0, 1.2).0.into_iter().fold(0.0, f32::max);
        let (one, two, stock) = (peak(Hold::ONE_HAND), peak(Hold::TWO_HANDS), peak(Hold::SHOULDERED));
        assert!(two < one * 0.7 && stock < two, "one hand {one:.1}, two {two:.1}, shouldered {stock:.1}");
    }

    #[test]
    fn pistols_snap_and_recover_faster() {
        let (p, _) = fire(&pistol(), Hold::ONE_HAND, 1, 1.0, 1.0);
        let peak = p.iter().cloned().fold(0.0, f32::max);
        assert!(peak > 10.0, "a big flip: {peak:.1}");
        assert!(p[(0.35 * 90.0) as usize].abs() < peak * 0.25, "and quickly back");
    }

    #[test]
    fn the_push_back_is_felt() {
        let mut r = Recoil::new(1);
        r.kick(&rifle(), Hold::ONE_HAND);
        let mut back = 0.0f32;
        for _ in 0..20 {
            r.update(1.0 / 90.0);
            back = back.max(r.offset.z);
        }
        assert!(back > 0.018, "the gun drives back about 2.5 cm: {back:.3}");
    }

    #[test]
    fn it_is_repeatable() {
        let a = fire(&rifle(), Hold::ONE_HAND, 8, 10.0, 1.0).0;
        let b = fire(&rifle(), Hold::ONE_HAND, 8, 10.0, 1.0).0;
        assert_eq!(a, b);
    }
}
