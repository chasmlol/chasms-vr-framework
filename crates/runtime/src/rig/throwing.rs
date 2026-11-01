//! Release velocity: what a throw leaves the hand with. Built from the research in
//! `docs/design/vr-throwables-and-ui/throwables.md` (section 10):
//!
//! * **History** (Alyx, the Valve patent): 0.3 s of controller samples per hand, in *tracking*
//!   space (a snap turn or a teleport doesn't pollute it), the controller's own velocity where the
//!   runtime gives one (IMU-fused, OpenXR's) and differenced poses otherwise. Locomotion is added
//!   only at the release.
//! * **Estimate** (Half-Life: Alyx, "averages the 3 frames bracketing the peak"): the peak controller
//!   speed in the 0.12 s before the release (no lever term, so a wrist flick can't fake it; on a
//!   plateau, its middle), the mean of the samples within 17 ms of it, its direction refined by a
//!   least-squares line through the
//!   positions around the peak (Deck, Schoneveld), and the spin averaged over twice that. A hand
//!   that slows before letting go still throws at its peak; a late grip release doesn't matter.
//! * **Backdating** (the Valve patent): the object leaves from where it was at the peak, carried on
//!   along the throw to now (up to 0.15 s), unless the world is in the way.
//! * **Gain** (Valve's speed curve, quakevr): 1 below 1.5 m/s rising smoothly to 1.25 by 6 m/s
//!   (1.35 for grenades: players undershoot, and short grenades are the top complaint), capped at
//!   25 m/s (knives 30). Under 1 m/s it's a drop: no gain, no knife alignment, a quarter of the spin.
//! * **Spin**: 0.75 of the hand's (between Auto Hand's 0.5 and XRI's 1), capped at 20 rad/s (40 for
//!   a spinning knife). The wrist's spin adds `0.8 * w x r` at the object once it passes 3 rad/s
//!   (HVR's gate, ramped in), so flicks help without turning the hand into a catapult.
//!
//! (The release itself: mid-swing the rig lets go as the grip eases a quarter off its hardest
//! squeeze, Deck's fix for late releases; see `Rig::hand_input`.)

use glam::Vec3;
use std::collections::VecDeque;

use super::WorldMap;

/// Seconds of history kept per hand (at least 64 samples at 90 Hz, 0.3 s at 200 Hz).
pub const HISTORY: f64 = 0.3;
pub const MAX_SAMPLES: usize = 96;
/// Look-back before the release for the peak (Alyx: 10 frames at 90 Hz).
pub const WINDOW: f64 = 0.12;
/// Samples within this of the peak make the linear estimate (3 frames at 90 Hz) ...
pub const PEAK_SPAN: f64 = 0.017;
/// ... and within this the spin (it's noisier).
pub const SPIN_SPAN: f64 = 0.034;
/// The positions from this long before the peak to `FIT_AFTER` after it give the direction.
pub const FIT_BEFORE: f64 = 0.04;
pub const FIT_AFTER: f64 = 0.01;
/// Slower than this at release is a drop, not a throw.
pub const DROP_SPEED: f32 = 1.0;
/// The gain ramps in between these speeds (m/s).
pub const GAIN_FROM: f32 = 1.5;
pub const GAIN_FULL: f32 = 6.0;
pub const GAIN_GENERIC: f32 = 1.25;
pub const GAIN_GRENADE: f32 = 1.35;
pub const MAX_SPEED: f32 = 25.0;
pub const MAX_SPEED_KNIFE: f32 = 30.0;
pub const SPIN_SCALE: f32 = 0.75;
pub const MAX_SPIN: f32 = 20.0;
pub const MAX_SPIN_KNIFE: f32 = 40.0;
/// The wrist's spin carries the object round (`w x r`) this much, above `LEVER_FROM` rad/s.
pub const LEVER_GAIN: f32 = 0.8;
pub const LEVER_FROM: f32 = 3.0;
/// The object leaves from where it was at the peak, no more than this long ago.
pub const MAX_BACKDATE: f64 = 0.15;

#[derive(Clone, Copy, Debug)]
struct Sample {
    t: f64,
    /// Tracking space.
    pos: Vec3,
    vel: Vec3,
    ang: Vec3,
    /// Tracking -> world when it was taken (for locomotion).
    map: WorldMap,
}

/// One hand's recent motion.
#[derive(Default, Clone, Debug)]
pub struct ThrowHistory {
    samples: VecDeque<Sample>,
}

/// The hand's motion at a release (world space, the current tracking -> world mapping).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Release {
    /// Linear velocity at the controller (m/s), locomotion included.
    pub lin: Vec3,
    /// Angular velocity (rad/s).
    pub ang: Vec3,
    /// When the peak was, and how fast the controller was going then and at the release.
    pub t_peak: f64,
    pub peak_speed: f32,
    pub release_speed: f32,
    /// The controller at the peak (world).
    pub peak_pos: Vec3,
    /// The world velocity locomotion added.
    pub loco: Vec3,
}

impl Release {
    pub fn speed(&self) -> f32 {
        self.lin.length()
    }

    /// Fast enough to be a throw (not a drop).
    pub fn is_throw(&self) -> bool {
        self.speed() >= DROP_SPEED
    }
}

impl ThrowHistory {
    /// Adds the controller's tracking-space pose at `t`. `vel`: the runtime's own velocity, when it
    /// gives one (None, or zero: differenced from the poses).
    pub fn push(&mut self, t: f64, pos: Vec3, vel: Option<Vec3>, ang: Vec3, map: WorldMap) {
        let differenced = match self.samples.back() {
            Some(p) if t > p.t + 1e-4 => (pos - p.pos) / (t - p.t) as f32,
            Some(p) if t <= p.t => return, // a repeated frame
            Some(p) => p.vel,
            None => Vec3::ZERO,
        };
        let vel = vel.filter(|v| v.length_squared() > 0.0 && v.is_finite()).unwrap_or(differenced);
        self.samples.push_back(Sample { t, pos, vel, ang, map });
        while self.samples.len() > MAX_SAMPLES || self.samples.front().is_some_and(|s| t - s.t > HISTORY) {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// The hand's velocity relative to the body right now, in world axes (no locomotion): the
    /// peak of the last 50 ms (for flicks and telling a gentle let-go from a swing).
    pub fn recent(&self) -> Vec3 {
        let Some(last) = self.samples.back() else { return Vec3::ZERO };
        let peak = self.samples.iter().rev().take_while(|s| last.t - s.t <= 0.05).max_by(|a, b| a.vel.length_squared().total_cmp(&b.vel.length_squared()));
        peak.map(|s| last.map.vel(s.vel) / last.map.world_per_meter.max(1e-6)).unwrap_or(Vec3::ZERO)
    }

    /// The estimate for a release at `t_rel` (see the module docs), frozen there.
    pub fn release(&self, t_rel: f64) -> Release {
        let Some(last) = self.samples.iter().rev().find(|s| s.t <= t_rel + 1e-6).or(self.samples.back()) else { return Release::default() };
        let now = last.map;
        let window: Vec<&Sample> = self.samples.iter().filter(|s| s.t <= t_rel + 1e-6 && t_rel - s.t <= WINDOW + 1e-6).collect();
        if window.is_empty() {
            return Release { t_peak: last.t, ..Default::default() };
        }
        // The peak: the fastest stretch of three frames (on a plateau its middle, so the frames
        // bracketing it are all at speed), then the fastest frame.
        let span_speed = |c: &Sample| {
            let near: Vec<f32> = window.iter().filter(|s| (s.t - c.t).abs() <= PEAK_SPAN + 1e-6).map(|s| s.vel.length()).collect();
            near.iter().sum::<f32>() / near.len().max(1) as f32
        };
        let peak = *window
            .iter()
            .map(|s| (s, span_speed(s), s.vel.length()))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
            .map(|(s, _, _)| s)
            .unwrap();
        let mean = |span: f64, f: &dyn Fn(&Sample) -> Vec3| {
            let near: Vec<&&Sample> = window.iter().filter(|s| (s.t - peak.t).abs() <= span + 1e-6).collect();
            near.iter().map(|s| f(s)).sum::<Vec3>() / near.len().max(1) as f32
        };
        let mut lin = mean(PEAK_SPAN, &|s| s.vel);
        let ang = mean(SPIN_SPAN, &|s| s.ang);
        // The direction from a straight line through the positions around the peak (robust to a
        // noisy sample); the speed stays the peak's.
        let fit: Vec<&Sample> = self.samples.iter().filter(|s| s.t >= peak.t - FIT_BEFORE - 1e-6 && s.t <= (peak.t + FIT_AFTER).min(t_rel) + 1e-6).collect();
        if fit.len() >= 3 {
            let tm = fit.iter().map(|s| s.t).sum::<f64>() / fit.len() as f64;
            let pm = fit.iter().map(|s| s.pos).sum::<Vec3>() / fit.len() as f32;
            let (mut num, mut den) = (Vec3::ZERO, 0.0f32);
            for s in &fit {
                let dt = (s.t - tm) as f32;
                num += (s.pos - pm) * dt;
                den += dt * dt;
            }
            if den > 1e-9 {
                let slope = num / den;
                if slope.length() > 0.5 && slope.angle_between(lin) < 45f32.to_radians() {
                    lin = slope.normalize() * lin.length();
                }
            }
        }
        // Locomotion: how fast the mapping carried a fixed tracking point (the hand) through the
        // world over the last 0.1 s, skipping turns and teleports.
        let mut loco = Vec3::ZERO;
        let mut n = 0;
        let recent: Vec<&Sample> = self.samples.iter().filter(|s| s.t <= t_rel + 1e-6 && t_rel - s.t <= 0.1).collect();
        for w in recent.windows(2) {
            let (a, b) = (w[0], w[1]);
            let dt = (b.t - a.t) as f32;
            if dt <= 1e-4 || (a.map.body_yaw - b.map.body_yaw).abs() > 1e-4 {
                continue;
            }
            let v = (b.map.point(last.pos) - a.map.point(last.pos)) / dt;
            if v.length() < 10.0 {
                loco += v;
                n += 1;
            }
        }
        if n > 0 {
            loco /= n as f32;
        }
        Release {
            lin: now.vel(lin) + loco,
            ang: now.rot(glam::Quat::IDENTITY) * ang,
            t_peak: peak.t,
            peak_speed: peak.vel.length() * now.world_per_meter,
            release_speed: last.vel.length() * now.world_per_meter,
            peak_pos: now.point(peak.pos),
            loco,
        }
    }
}

/// What's being thrown, for its gain and limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThrowClass {
    Generic,
    Grenade,
    Knife,
}

/// The throw's speed gain at release speed `speed` (1 for drops and gentle tosses).
pub fn gain(class: ThrowClass, speed: f32) -> f32 {
    let g = if class == ThrowClass::Grenade { GAIN_GRENADE } else { GAIN_GENERIC };
    let x = ((speed - GAIN_FROM) / (GAIN_FULL - GAIN_FROM)).clamp(0.0, 1.0);
    1.0 + (g - 1.0) * x * x * (3.0 - 2.0 * x)
}

/// What the object leaves the hand with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Launch {
    pub lin: Vec3,
    pub ang: Vec3,
    pub gain: f32,
    /// A real throw (not a drop).
    pub thrown: bool,
}

/// The launch for a release: `r` from the controller to the object's centre (world), `heavy` the
/// weight's share of the speed (1 = none), `scale` the player's throw-strength setting.
pub fn launch(rel: &Release, class: ThrowClass, r: Vec3, heavy: f32, scale: f32) -> Launch {
    let speed = rel.speed();
    if speed < DROP_SPEED {
        return Launch { lin: rel.lin * heavy, ang: rel.ang * 0.25, gain: 1.0, thrown: false };
    }
    let w = rel.ang.length();
    let lever_k = LEVER_GAIN * ((w - (LEVER_FROM - 1.0)) / 2.0).clamp(0.0, 1.0);
    let lever = (rel.ang.cross(r) * lever_k).clamp_length_max(5.0);
    let v = rel.lin + lever;
    let g = gain(class, v.length());
    let max = if class == ThrowClass::Knife { MAX_SPEED_KNIFE } else { MAX_SPEED };
    let lin = (v * g * heavy * scale).clamp_length_max(max);
    let max_spin = if class == ThrowClass::Knife { MAX_SPIN_KNIFE } else { MAX_SPIN };
    Launch { lin, ang: (rel.ang * SPIN_SCALE).clamp_length_max(max_spin), gain: g, thrown: true }
}

/// Where the object leaves from: where it was at the peak (`at_peak`), carried on along the
/// throw with gravity to `now` (no more than [`MAX_BACKDATE`] later).
pub fn backdated(at_peak: Vec3, v: Vec3, t_peak: f64, now: f64) -> Vec3 {
    let age = (now - t_peak).clamp(0.0, MAX_BACKDATE) as f32;
    at_peak + v * age - Vec3::Y * (0.5 * super::GRAVITY * age * age)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 90.0;

    /// A history of a hand moving along `dir` at these speeds, one per frame (no runtime velocity).
    fn swing(speeds: &[f32], dir: Vec3) -> (ThrowHistory, f64) {
        let mut h = ThrowHistory::default();
        let mut p = Vec3::new(0.2, 1.3, 0.1);
        let mut t = 0.0;
        h.push(t, p, None, Vec3::ZERO, WorldMap::IDENTITY);
        for s in speeds {
            t += DT;
            p += dir.normalize() * *s * DT as f32;
            h.push(t, p, None, Vec3::ZERO, WorldMap::IDENTITY);
        }
        (h, t)
    }

    #[test]
    fn a_fast_flick_throws_at_its_peak_speed_and_direction() {
        let dir = Vec3::new(0.0, 0.4, 1.0);
        let (h, t) = swing(&[1.0, 3.0, 6.0, 8.0, 9.0, 9.0], dir);
        let r = h.release(t);
        assert!((r.speed() - 9.0).abs() < 0.5, "peak speed: {}", r.speed());
        assert!(r.lin.angle_between(dir) < 0.05, "along the swing: {:?}", r.lin);
        let l = launch(&r, ThrowClass::Generic, Vec3::ZERO, 1.0, 1.0);
        assert!(l.thrown && (l.gain - GAIN_GENERIC).abs() < 1e-3, "a hard throw gets the full gain: {}", l.gain);
        let g = launch(&r, ThrowClass::Grenade, Vec3::ZERO, 1.0, 1.0);
        assert!(g.lin.length() > l.lin.length(), "grenades get more");
        assert!(launch(&r, ThrowClass::Generic, Vec3::ZERO, 1.0, 10.0).lin.length() <= MAX_SPEED + 1e-3, "capped");
    }

    #[test]
    fn a_slow_lob_keeps_its_speed() {
        let dir = Vec3::new(0.0, 1.0, 1.0);
        let (h, t) = swing(&[0.5, 1.2, 2.0, 2.4, 2.5, 2.5, 2.5], dir);
        let r = h.release(t);
        assert!((r.speed() - 2.5).abs() < 0.15, "{}", r.speed());
        let l = launch(&r, ThrowClass::Generic, Vec3::ZERO, 1.0, 1.0);
        assert!(l.thrown && l.gain < 1.05, "barely any gain for a lob: {}", l.gain);
        assert!(l.lin.angle_between(dir) < 0.05);
    }

    #[test]
    fn letting_go_of_a_slow_hand_is_a_drop() {
        let (h, t) = swing(&[0.2, 0.4, 0.5, 0.5], Vec3::new(0.3, -1.0, 0.2));
        let r = h.release(t);
        assert!(!r.is_throw(), "{}", r.speed());
        let mut r2 = r;
        r2.ang = Vec3::new(4.0, 0.0, 0.0);
        let l = launch(&r2, ThrowClass::Knife, Vec3::new(0.0, 0.0, 0.1), 1.0, 1.0);
        assert!(!l.thrown && l.gain == 1.0, "no gain");
        assert!((l.ang.length() - 1.0).abs() < 1e-4, "a quarter of the spin: {:?}", l.ang);
        assert!((l.lin - r.lin).length() < 1e-5, "no lever on a drop");
    }

    #[test]
    fn a_hand_that_slows_before_letting_go_still_throws_at_its_peak() {
        // Up to 6 m/s, then braking to 1 m/s over the 4 frames (44 ms) before the grip opens.
        let dir = Vec3::Z;
        let (h, t) = swing(&[2.0, 4.0, 6.0, 6.0, 6.0, 4.5, 3.0, 2.0, 1.0], dir);
        let r = h.release(t);
        assert!(r.release_speed < 1.5, "the hand had slowed: {}", r.release_speed);
        assert!(r.speed() > 5.5, "thrown at the peak: {}", r.speed());
        assert!(t - r.t_peak > 0.04 && t - r.t_peak < 0.1, "peak {} ms before the release", (t - r.t_peak) * 1000.0);
        // It leaves from where it was at the peak, carried along the throw.
        let p = backdated(r.peak_pos, r.lin, r.t_peak, t);
        assert!(p.z > r.peak_pos.z + 0.2, "carried on to now: {p:?} from {:?}", r.peak_pos);
    }

    #[test]
    fn a_peak_older_than_the_window_doesnt_count() {
        // A fast swing, then 0.2 s nearly still before letting go: that's a drop.
        let mut speeds = vec![3.0, 6.0, 6.0, 3.0];
        speeds.extend(std::iter::repeat_n(0.3, 18));
        let (h, t) = swing(&speeds, Vec3::Z);
        assert!(!h.release(t).is_throw());
    }

    #[test]
    fn the_runtime_velocity_wins_over_differencing() {
        let mut h = ThrowHistory::default();
        for k in 0..8 {
            // Poses that barely move (a late, smoothed pose) while the IMU says 7 m/s.
            h.push(k as f64 * DT, Vec3::new(0.0, 1.0, k as f32 * 0.001), Some(Vec3::new(0.0, 0.0, 7.0)), Vec3::ZERO, WorldMap::IDENTITY);
        }
        assert!((h.release(7.0 * DT).speed() - 7.0).abs() < 0.3);
    }

    #[test]
    fn turning_doesnt_pollute_the_throw_and_walking_adds_to_it() {
        let mut h = ThrowHistory::default();
        let mut map = WorldMap::IDENTITY;
        for k in 0..10 {
            let t = k as f64 * DT;
            // A 45 degree snap turn half-way through, and the body walking +Z at 2 m/s throughout.
            if k == 5 {
                map.body_yaw = std::f32::consts::FRAC_PI_4;
            }
            map.anchor = Vec3::new(0.0, 0.0, 2.0 * t as f32);
            h.push(t, Vec3::new(0.0, 1.2, 0.2 + 4.0 * t as f32), None, Vec3::ZERO, map);
        }
        let r = h.release(9.0 * DT);
        // 4 m/s along tracking +Z, turned 45 degrees into the world, plus the walk.
        let want = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4) * Vec3::new(0.0, 0.0, 4.0) + Vec3::new(0.0, 0.0, 2.0);
        assert!((r.lin - want).length() < 0.3, "{:?} vs {want:?}", r.lin);
        assert!((r.loco - Vec3::new(0.0, 0.0, 2.0)).length() < 0.1, "locomotion {:?}", r.loco);
    }

    #[test]
    fn the_wrist_adds_its_lever_only_when_it_really_spins() {
        let r = Release { lin: Vec3::new(0.0, 0.0, 4.0), ang: Vec3::new(1.5, 0.0, 0.0), ..Default::default() };
        let arm = Vec3::new(0.0, 0.2, 0.0);
        let slow = launch(&r, ThrowClass::Generic, arm, 1.0, 1.0);
        assert!((slow.lin.normalize() - Vec3::Z).length() < 1e-4, "a slow wrist adds nothing");
        let r = Release { ang: Vec3::new(10.0, 0.0, 0.0), ..r };
        let fast = launch(&r, ThrowClass::Generic, arm, 1.0, 1.0);
        // w x r = (10,0,0) x (0,0.2,0) = (0,0,2): forward.
        assert!(fast.lin.z > slow.lin.z + 1.5, "a flicked wrist throws harder: {:?} vs {:?}", fast.lin, slow.lin);
        assert!(fast.ang.length() <= MAX_SPIN + 1e-4 && (fast.ang.length() - 7.5).abs() < 1e-3, "0.75 of the spin");
    }

    use glam::Quat;
}
