//! Where the game has the player at any moment. Its states come at its own pace (Metro steps once
//! an eye pair, 50 to 90 times a second, unevenly), each stamped with when the game's clock stood
//! where it has the player (`GameState::time_qpc`). The walk and turn between the last two steps
//! are carried on to when a headset frame is shown, so the world moves smoothly under a walking,
//! turning player instead of in the game's steps: still for a frame, then a jump. Where a new step
//! disagrees with what was being shown (the player stopped, turned harder, the game hitched past
//! `MAX_AHEAD`), the difference is glided out over `SETTLE` rather than jumped.

use glam::Vec3;

use crate::rig::WorldMap;

/// At most this far past the game's last step (seconds): a game that has stopped stepping (a
/// pause, a hitch) isn't walked on and on. (Metro's camera is a step behind its clock and a pair
/// of eyes is 10 to 25 ms: shown 25 to 50 ms after.)
const MAX_AHEAD: f64 = 0.15;
/// How fast a disagreement between what was shown and a new step fades (seconds, e-folding).
const SETTLE: f64 = 0.06;
/// Further than this in one step (metres) or turned more (radians) is a jump (a snap turn, a
/// teleport, a level), not a speed.
const JUMP: f32 = 1.0;
const JUMP_TURN: f32 = 0.3;

#[derive(Clone, Copy, Debug, Default)]
pub struct Locomotion {
    /// The performance counter's ticks a second.
    freq: f64,
    /// The newest step: when, and the mapping it gave.
    last: Option<(i64, WorldMap)>,
    /// The walk (world units a second: where tracking's origin goes in the world) and the turn
    /// (radians a second) between the last steps.
    vel: Vec3,
    turn: f32,
    /// What was being carried on before the newest step came (its step, walk and turn), until a
    /// frame has been placed from the new one; and whether the newest step was a jump.
    before: Option<((i64, WorldMap), Vec3, f32)>,
    jumped: bool,
    /// What's still being glided out (world units, radians), as of `settle_at`.
    settle: (Vec3, f32),
    settle_at: i64,
}

impl Locomotion {
    pub fn new() -> Locomotion {
        let mut freq = 0i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut freq);
        }
        Locomotion { freq: freq.max(1) as f64, ..Default::default() }
    }

    pub fn reset(&mut self) {
        *self = Locomotion { freq: self.freq, ..Default::default() };
    }

    /// The game's state: `qpc` when its clock stood where `map` has the player (0: not known, and
    /// the mapping is taken as it is).
    pub fn see(&mut self, qpc: i64, map: WorldMap) {
        if qpc <= 0 {
            self.reset();
            return;
        }
        match self.last {
            Some((t, was)) if qpc > t => {
                if self.before.is_none() {
                    self.before = Some(((t, was), self.vel, self.turn));
                }
                let dt = ((qpc - t) as f64 / self.freq) as f32;
                // Where tracking's origin is in the world now against where it was: the walk (a
                // step in the room moves the origin and the anchor together, and isn't one).
                let moved = map.anchor - was.point(map.origin);
                let turn = (map.body_yaw - was.body_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                if moved.length() > JUMP * map.world_per_meter || turn.abs() > JUMP_TURN || dt > 0.25 || map.world_per_meter != was.world_per_meter {
                    self.vel = Vec3::ZERO;
                    self.turn = 0.0;
                    self.jumped = true;
                } else {
                    // The step's own speed: both its distance and its time are the game's clock's,
                    // so an even walk is even however unevenly the game steps.
                    self.vel = moved / dt;
                    self.turn = turn / dt;
                }
                self.last = Some((qpc, map));
            }
            // The same step again (Metro's second eye): the newest mapping (the head moved in the
            // room), the step's time.
            Some((t, _)) if qpc == t => self.last = Some((t, map)),
            _ => {
                self.vel = Vec3::ZERO;
                self.turn = 0.0;
                self.jumped = true;
                self.last = Some((qpc, map));
            }
        }
    }

    /// Step `step` walked `vel` and turned `turn` on to `qpc`.
    fn carried(&self, (t, m): (i64, WorldMap), vel: Vec3, turn: f32, qpc: i64) -> WorldMap {
        let ahead = ((qpc - t) as f64 / self.freq).clamp(0.0, MAX_AHEAD) as f32;
        WorldMap { anchor: m.anchor + vel * ahead, body_yaw: m.body_yaw + turn * ahead, ..m }
    }

    /// What's still to glide out at `qpc`.
    fn settling(&self, qpc: i64) -> (Vec3, f32) {
        let k = (-(((qpc - self.settle_at).max(0) as f64 / self.freq) / SETTLE)).exp() as f32;
        (self.settle.0 * k, self.settle.1 * k)
    }

    /// The mapping at `qpc` (when a headset frame is shown): the last step's, walked and turned on
    /// from when the game's clock stood there, plus what's still gliding out. Without a step (or a
    /// time) to go from, `fallback`.
    pub fn at(&mut self, qpc: i64, fallback: WorldMap) -> WorldMap {
        let Some(last) = self.last else { return fallback };
        if qpc <= 0 {
            return last.1;
        }
        let now = self.carried(last, self.vel, self.turn, qpc);
        // A new step since the frame before: whatever it disagrees with what was being shown is
        // glided out from here (unless it's a jump, which is shown as one).
        if let Some((step, vel, turn)) = self.before.take() {
            let jumped = std::mem::take(&mut self.jumped);
            let (lin, yaw) = self.settling(qpc);
            let was = self.carried(step, vel, turn, qpc);
            let was = WorldMap { anchor: was.anchor + lin, body_yaw: was.body_yaw + yaw, ..was };
            let off = (was.point(now.origin) - now.anchor, (was.body_yaw - now.body_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI);
            self.settle = if jumped || off.0.length() > JUMP * now.world_per_meter || off.1.abs() > JUMP_TURN { (Vec3::ZERO, 0.0) } else { off };
            self.settle_at = qpc;
        }
        let (lin, yaw) = self.settling(qpc);
        WorldMap { anchor: now.anchor + lin, body_yaw: now.body_yaw + yaw, ..now }
    }

    /// The walk (world units a second) and the turn (radians a second).
    pub fn walk(&self) -> (Vec3, f32) {
        (self.vel, self.turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(z: f32, yaw: f32) -> WorldMap {
        WorldMap { origin: Vec3::new(0.0, 1.7, 0.0), body_yaw: yaw, world_per_meter: 1.0, anchor: Vec3::new(10.0, 1.7, z) }
    }

    /// Steps at an uneven pace, walking at 2.5 m/s: in between, the mapping goes on at the walk,
    /// so frames shown at an even pace see the world move evenly.
    #[test]
    fn between_uneven_steps_the_walk_goes_on_evenly() {
        let mut l = Locomotion { freq: 1000.0, ..Default::default() };
        let mut t = 0i64;
        for (i, gap) in [11, 17, 9, 14, 20, 8, 12].iter().enumerate() {
            t += gap;
            l.see(t, map(2.5 * t as f32 / 1000.0, 0.0));
            for ahead in [0, 4, 8] {
                let z = l.at(t + ahead, map(0.0, 0.0)).anchor.z;
                let want = 2.5 * (t + ahead) as f32 / 1000.0;
                // (Once what the start of the walk left to glide out has gone.)
                if i > 4 {
                    assert!((z - want).abs() < 1e-3, "at {} ms: {z} for {want}", t + ahead);
                }
            }
        }
        // A pause: not walked on past MAX_AHEAD.
        let z = l.at(t + 500, map(0.0, 0.0)).anchor.z;
        assert!((z - 2.5 * (t as f32 / 1000.0 + MAX_AHEAD as f32)).abs() < 1e-3);
    }

    /// Walking at 3.75 m/s, then stopped dead: the frames (every 8 ms) never jump back; the world
    /// settles where the game has it.
    #[test]
    fn a_sudden_stop_glides_rather_than_jumps() {
        let mut l = Locomotion { freq: 1000.0, ..Default::default() };
        let (mut t, mut z) = (0i64, 0.0f32);
        let mut shown = Vec::new();
        for k in 0..60 {
            if k % 2 == 0 {
                // A step every 16 ms; the walk stops after the 12th.
                let walking = k < 24;
                t += 16;
                if walking {
                    z += 3.75 * 0.016;
                }
                l.see(t, map(z, 0.0));
            }
            shown.push(l.at(t + 40 + (k % 2) * 8, map(0.0, 0.0)).anchor.z);
        }
        let worst_back = shown.windows(2).map(|w| w[0] - w[1]).fold(0.0f32, f32::max);
        assert!(worst_back < 0.03, "back {worst_back:.3} m in a frame at worst");
        assert!((shown.last().unwrap() - z).abs() < 5e-3, "settles where the game has it: {} for {z}", shown.last().unwrap());
    }

    #[test]
    fn a_turn_goes_on_and_a_snap_turn_is_not_a_speed() {
        let mut l = Locomotion { freq: 1000.0, ..Default::default() };
        for k in 1..=6 {
            l.see(k * 10, map(0.0, 1.5 * (k * 10) as f32 / 1000.0));
        }
        let yaw = l.at(70, map(0.0, 0.0)).body_yaw;
        // (Less what the start of the turn left to glide out.)
        assert!((yaw - 0.105).abs() < 0.01, "{yaw}");
        // A snap turn of 45 degrees: the mapping jumps with it and nothing is carried on.
        l.see(80, map(0.0, 0.09 + 0.785));
        assert_eq!(l.walk(), (Vec3::ZERO, 0.0));
        assert!((l.at(90, map(0.0, 0.0)).body_yaw - (0.09 + 0.785)).abs() < 1e-5);
    }

    #[test]
    fn the_second_eye_and_unknown_times_carry_nothing_on() {
        let mut l = Locomotion { freq: 1000.0, ..Default::default() };
        l.see(10, map(0.0, 0.0));
        l.see(20, map(0.025, 0.0));
        // The second eye: the same step, the head a little further on in the room.
        let mut m = map(0.025, 0.0);
        m.origin.x += 0.01;
        m.anchor.x += 0.01;
        l.see(20, m);
        assert!((l.at(20, map(0.0, 0.0)).origin.x - 0.01).abs() < 1e-6);
        assert!((l.walk().0.z - 2.5).abs() < 1e-3, "{:?}", l.walk());
        // A game that doesn't stamp its states: its mapping as it is.
        l.see(0, map(3.0, 0.0));
        assert_eq!(l.at(50, map(3.0, 0.0)).anchor.z, 3.0);
    }
}
