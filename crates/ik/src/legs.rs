//! Feet and legs. Feet are anchored to the world: while standing they step when balance needs it
//! (VRIK's procedural footsteps), under stick locomotion a velocity-driven gait takes over (FRIK's
//! phase generator), and seated they rest in front of the seat. Legs are two-bone IK to the
//! planted feet; a foot out of reach rises onto its toes before the hips come down (VRIK plants
//! the toes, not the ankles).

use std::f32::consts::PI;

use glam::{Quat, Vec3};

use crate::arm::two_bone;
use crate::math::{flat, frame_yz, smoothing, wrap_angle, yaw_dir, UP};
use crate::skeleton::Skeleton;
use crate::BodyParams;

/// The knee keeps a sliver of bend so its direction stays defined.
const KNEE_MIN_FLEXION: f32 = 3.0 * PI / 180.0;
/// VRIK lets a new step start once the other foot is 80% through its own.
const OVERLAP: f32 = 0.8;
/// VRIK biases the tie-break towards the left foot by lowering its threshold to 90%.
const LEFT_BIAS: [f32; 2] = [0.9, 1.0];
/// Below this centre-of-mass speed the body counts as still, for settling steps (est.).
const STILL_SPEED: f32 = 0.1;
/// Seated feet rest this far ahead of the hip joints, as a fraction of the thigh, and glide to
/// new spots with this time constant (est.).
const SEATED_REACH: f32 = 0.9;
const SEATED_TAU: f32 = 0.15;

#[derive(Clone, Copy, Debug)]
struct Step {
    from: Vec3,
    from_yaw: f32,
    progress: f32,
    duration: f32,
    length: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Foot {
    /// The floor point under the ankle.
    pub pos: Vec3,
    pub yaw: f32,
    step: Option<Step>,
}

impl Foot {
    /// Height off the floor. The foot is down again by the time the next step may start (VRIK's
    /// 80%), so two feet are never in the air together; it slides the last few centimetres.
    pub fn lift(&self, p: &BodyParams, scale: f32) -> f32 {
        match self.step {
            Some(s) => {
                let height = p.step_height * scale * (s.length / (0.25 * scale)).clamp(0.3, 1.0);
                (PI * (s.progress / OVERLAP).min(1.0)).sin() * height
            }
            None => 0.0,
        }
    }
}

pub struct FeetInput {
    /// Centre of mass and its velocity (tracking space).
    pub com: Vec3,
    pub com_vel: Vec3,
    pub root_yaw: f32,
    /// Horizontal stick-locomotion velocity.
    pub locomotion: Vec3,
    /// Seated: the hip joints' midpoint.
    pub seated_hips: Option<Vec3>,
}

#[derive(Clone, Copy, Debug)]
pub struct Feet {
    pub feet: [Foot; 2],
    /// Stick gait: the foot swinging next, while walking.
    gait: Option<usize>,
    still_time: f32,
}

impl Feet {
    pub fn new(p: &BodyParams, skel: &Skeleton, center: Vec3, yaw: f32) -> Self {
        let foot = |i| Foot { pos: spot(skel, flat(center), yaw, i, p.foot_spacing), yaw, step: None };
        Feet { feet: [foot(0), foot(1)], gait: None, still_time: 0.0 }
    }

    pub fn walking(&self) -> bool {
        self.gait.is_some()
    }

    pub fn update(&mut self, p: &BodyParams, skel: &Skeleton, dt: f32, input: &FeetInput) {
        if let Some(hips) = input.seated_hips {
            self.gait = None;
            for (i, foot) in self.feet.iter_mut().enumerate() {
                let ahead = flat(hips) + yaw_dir(input.root_yaw) * skel.thigh * SEATED_REACH;
                let target = spot(skel, ahead, input.root_yaw, i, p.foot_spacing * 0.8);
                let a = smoothing(dt, SEATED_TAU);
                foot.pos = foot.pos.lerp(target, a);
                foot.yaw = wrap_angle(foot.yaw + wrap_angle(input.root_yaw - foot.yaw) * a);
                foot.step = None;
            }
            return;
        }
        // Feet are planted in the world, so under stick locomotion they slide back in tracking
        // space.
        let v = flat(input.locomotion);
        for foot in &mut self.feet {
            if foot.step.is_none() {
                foot.pos -= v * dt;
            }
        }
        let speed = v.length();
        // A balance step in progress finishes before the gait starts.
        let stepping = self.feet.iter().any(|f| f.step.is_some());
        if self.gait.is_none() && !stepping && speed >= p.gait_start_speed {
            // Start with the foot furthest behind along the travel direction.
            let behind = |f: &Foot| -(f.pos - flat(input.com)).dot(v);
            self.gait = Some(if behind(&self.feet[1]) > behind(&self.feet[0]) { 1 } else { 0 });
        }
        if self.gait.is_some() {
            self.update_gait(p, skel, dt, input, speed);
        } else {
            self.update_balance(p, skel, dt, input);
        }
    }

    /// FRIK's phase generator: feet swing alternately, each landing half a step ahead of the body
    /// so it slides back through the stance to half a step behind.
    fn update_gait(&mut self, p: &BodyParams, skel: &Skeleton, dt: f32, input: &FeetInput, speed: f32) {
        let Some(swing) = self.gait else { return };
        let v = flat(input.locomotion);
        // FRIK: step time = clamp(cos(speed / 140 units/s)), about 2 m/s at 70 units per metre.
        let duration = (speed / 2.0).cos().clamp(p.gait_step_time_min, p.gait_step_time_max);
        let other = self.feet[1 - swing].pos;
        let foot = &mut self.feet[swing];
        let step = foot.step.get_or_insert(Step {
            from: foot.pos,
            from_yaw: foot.yaw,
            progress: 0.0,
            duration,
            length: speed * duration,
        });
        step.duration = duration;
        step.length = speed * duration;
        step.progress += dt / duration;
        let ahead = flat(input.com) + v * (duration * 0.5);
        let target = spot(skel, ahead, input.root_yaw, swing, p.foot_spacing * p.gait_spacing);
        let target = cross_guard(p, skel, target, other, input.root_yaw, swing);
        if advance(foot, target, input.root_yaw) {
            self.gait = if speed >= p.gait_stop_speed { Some(1 - swing) } else { None };
        }
    }

    /// VRIK's balance steps: a foot steps when the predicted centre of mass leaves it behind, or
    /// the root has turned away from it.
    fn update_balance(&mut self, p: &BodyParams, skel: &Skeleton, dt: f32, input: &FeetInput) {
        let s = skel.scale;
        // VRIK: velocity prediction clamped to 0.4 m/s, times 0.4.
        let com = input.com + input.com_vel.clamp_length_max(0.4) * 0.4;
        let ground = flat(com);
        if flat(input.com_vel).length() < STILL_SPEED {
            self.still_time += dt;
        } else {
            self.still_time = 0.0;
        }
        let targets: [Vec3; 2] = std::array::from_fn(|i| {
            let t = spot(skel, ground, input.root_yaw, i, p.foot_spacing);
            cross_guard(p, skel, t, self.feet[1 - i].pos, input.root_yaw, i)
        });
        for (i, foot) in self.feet.iter_mut().enumerate() {
            if let Some(step) = foot.step.as_mut() {
                step.progress += dt / step.duration;
                advance(foot, targets[i], input.root_yaw);
            }
        }

        let busy = self.feet.iter().any(|f| f.step.is_some_and(|s| s.progress < OVERLAP));
        if busy {
            return;
        }
        // VRIK: the threshold shrinks as the centre of mass tips over the feet.
        let support = (self.feet[0].pos + self.feet[1].pos) * 0.5;
        let lean = flat(com - support).length().atan2((com.y - support.y).max(0.1)).to_degrees();
        let threshold = p.step_threshold * s * (1.0 - 0.9 * (lean * 0.015).clamp(0.0, 1.0));
        let settled = self.still_time >= p.settle_delay;
        let mut best: Option<(usize, f32)> = None;
        for (i, foot) in self.feet.iter().enumerate() {
            if foot.step.is_some() {
                continue;
            }
            let dist = flat(targets[i] - foot.pos).length();
            let limit = threshold * LEFT_BIAS[i];
            let turned = wrap_angle(input.root_yaw - foot.yaw).abs() > p.step_angle_deg.to_radians();
            let need = if dist > limit || turned {
                1.0 + dist / limit
            } else if settled && dist > p.settle_distance * s {
                dist / limit
            } else {
                continue;
            };
            if best.is_none_or(|(_, n)| need > n) {
                best = Some((i, need));
            }
        }
        if let Some((i, need)) = best {
            let foot = &mut self.feet[i];
            // Urgent steps are quicker (VRIK's 3-4.5 per second, deterministic here).
            let urgency = (need - 1.0).clamp(0.0, 1.0);
            let duration = p.step_time_max + (p.step_time_min - p.step_time_max) * urgency;
            let length = flat(targets[i] - foot.pos).length();
            foot.step = Some(Step { from: foot.pos, from_yaw: foot.yaw, progress: 0.0, duration, length });
        }
    }
}

/// Moves a stepping foot along its step towards `target`, which re-tracks every frame. Returns
/// true when it lands.
fn advance(foot: &mut Foot, target: Vec3, yaw: f32) -> bool {
    let Some(step) = foot.step else { return false };
    let t = step.progress.min(1.0);
    // VRIK's InOutSine step interpolation.
    let ease = 0.5 - 0.5 * (PI * t).cos();
    foot.pos = step.from.lerp(target, ease);
    foot.yaw = wrap_angle(step.from_yaw + wrap_angle(yaw - step.from_yaw) * ease);
    if step.progress >= 1.0 {
        foot.pos = target;
        foot.yaw = yaw;
        foot.step = None;
        return true;
    }
    false
}

/// A foot's spot beside `center`: left (0) or right (1) of it, `spacing` apart.
fn spot(skel: &Skeleton, center: Vec3, yaw: f32, side: usize, spacing: f32) -> Vec3 {
    let right = right_of(yaw);
    let sign = if side == 0 { -1.0 } else { 1.0 };
    flat(center) + right * (sign * spacing * 0.5 * skel.scale)
}

fn right_of(yaw: f32) -> Vec3 {
    let f = yaw_dir(yaw);
    Vec3::new(f.z, 0.0, -f.x)
}

/// VRIK's leg-crossing guard: a step never lands across the other foot, nor within
/// `leg_cross_guard` of it; such targets are pushed out sideways.
fn cross_guard(p: &BodyParams, skel: &Skeleton, target: Vec3, other: Vec3, yaw: f32, side: usize) -> Vec3 {
    let outward = right_of(yaw) * if side == 0 { -1.0 } else { 1.0 };
    let rel = flat(target - other);
    let lateral = rel.dot(outward).max(0.0);
    let along = rel - outward * rel.dot(outward);
    let guard = p.leg_cross_guard * skel.scale;
    let needed = (guard * guard - along.length_squared()).max(0.0).sqrt();
    flat(other) + along + outward * lateral.max(needed)
}

#[derive(Clone, Copy, Debug)]
pub struct LegPose {
    pub hip: Vec3,
    pub knee: Vec3,
    pub ankle: Vec3,
    pub toes: Vec3,
    /// Canonical frames: upper leg, lower leg, foot, toes.
    pub rot: [Quat; 4],
}

/// The ankle and the ball of the foot for a foot `lift` off the floor, with the heel raised by
/// `heel_lift` radians about the ball.
fn ankle_at(skel: &Skeleton, foot: &Foot, lift: f32, heel_lift: f32) -> (Vec3, Vec3) {
    let fwd = yaw_dir(foot.yaw);
    let ball = foot.pos + fwd * skel.toe_forward + UP * (skel.toe_height + lift);
    let flat_ankle = foot.pos + UP * (skel.ankle_height + lift);
    let pitch = Quat::from_axis_angle(right_of(foot.yaw), heel_lift);
    (ball + pitch * (flat_ankle - ball), ball)
}

/// The hip joint heights (lowest, highest) from which this foot can stay planted: the knee bends
/// no further than its limit, and stretches no further than heel lift plus `leg_stretch`.
pub fn hip_height_range(p: &BodyParams, skel: &Skeleton, hip: Vec3, foot: &Foot, lift: f32) -> (f32, f32) {
    let (a, b) = (skel.thigh, skel.shin);
    let fold = p.knee_max_flexion_deg.to_radians();
    let closest = (a * a + b * b + 2.0 * a * b * fold.cos()).sqrt();
    let (flat_ankle, _) = ankle_at(skel, foot, lift, 0.0);
    let horizontal = flat(hip - flat_ankle).length();
    let low = flat_ankle.y + (closest * closest - horizontal * horizontal).max(0.0).sqrt();
    let (raised, _) = ankle_at(skel, foot, lift, p.heel_lift_max_deg.to_radians());
    let reach = (a + b) * p.leg_stretch;
    let horizontal = flat(hip - raised).length();
    let high = raised.y + (reach * reach - horizontal * horizontal).max(0.0).sqrt();
    (low, high.max(low))
}

pub fn solve_leg(p: &BodyParams, skel: &Skeleton, hip: Vec3, foot: &Foot, lift: f32, hips_fwd: Vec3) -> LegPose {
    let (a, b) = (skel.thigh, skel.shin);
    let reach = (a * a + b * b + 2.0 * a * b * KNEE_MIN_FLEXION.cos()).sqrt();
    // Rise onto the toes only as far as needed to reach the ankle.
    let max_lift = p.heel_lift_max_deg.to_radians();
    let mut heel = 0.0;
    if ankle_at(skel, foot, lift, 0.0).0.distance(hip) > reach {
        let (mut lo, mut hi) = (0.0, max_lift);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if ankle_at(skel, foot, lift, mid).0.distance(hip) > reach { lo = mid } else { hi = mid }
        }
        heel = hi;
    }
    let (ankle, toes) = ankle_at(skel, foot, lift, heel);
    let fwd = yaw_dir(foot.yaw);
    // VRIK bendToTargetWeight 0.5: the knee points between the foot and the hips.
    let knee_dir = (fwd + flat(hips_fwd).normalize_or(fwd)).normalize_or(fwd);
    let flex = [KNEE_MIN_FLEXION, p.knee_max_flexion_deg.to_radians()];
    // Feet stay planted: the leg stretches as far as it must (the hips have already come down
    // as far as the spine allows).
    let solved = two_bone(hip, ankle, [a, b], knee_dir, flex, f32::MAX);
    let thigh_dir = (solved.elbow - hip).normalize_or(-UP);
    let upper = frame_yz(thigh_dir, knee_dir, fwd);
    let lower = frame_yz(solved.forearm, knee_dir, fwd);
    let pitch = Quat::from_axis_angle(right_of(foot.yaw), heel);
    let foot_rot = frame_yz(pitch * fwd, pitch * UP, UP);
    let toes_rot = frame_yz(fwd, UP, UP);
    LegPose { hip, knee: solved.elbow, ankle, toes, rot: [upper, lower, foot_rot, toes_rot] }
}

/// A rider's leg astride a mount: from its hip joint to the ankle where the game's riding pose has
/// it (in the stirrup), the knee bent the way the game's is (out round the mount's side). A
/// shorter leg than the game rider's hangs just short of it; without the game's pose (`legs`
/// None), the leg hangs from the hip down the mount's side.
pub fn solve_saddle_leg(p: &BodyParams, skel: &Skeleton, hip: Vec3, saddle: &crate::Saddle, side: usize) -> LegPose {
    let (a, b) = (skel.thigh, skel.shin);
    let up = (saddle.seat.rot * UP).normalize_or(UP);
    let fwd = (saddle.seat.rot * Vec3::Z).normalize_or(Vec3::Z);
    let out = (saddle.seat.rot * Vec3::X) * if side == 0 { -1.0 } else { 1.0 };
    let [knee_at, ankle_at, toes_at] = match saddle.legs {
        Some(l) => l[side],
        None => {
            let ankle = hip + out * (0.12 * skel.scale) + fwd * (0.1 * skel.scale) - up * ((a + b) * 0.9);
            [hip + fwd * a + out * (0.15 * skel.scale), ankle, ankle + fwd * skel.toe_forward]
        }
    };
    let flex = [KNEE_MIN_FLEXION, p.knee_max_flexion_deg.to_radians()];
    // (Never stretched: a leg that doesn't reach the stirrup hangs short of it.)
    let solved = two_bone(hip, ankle_at, [a, b], knee_at - hip, flex, 1.0);
    let ankle = solved.elbow + solved.forearm * b;
    let thigh_dir = (solved.elbow - hip).normalize_or(-up);
    let knee_dir = (knee_at - hip).reject_from((ankle_at - hip).normalize_or(-up)).normalize_or(fwd);
    let upper = frame_yz(thigh_dir, knee_dir, fwd);
    let lower = frame_yz(solved.forearm, knee_dir, fwd);
    let foot_fwd = (toes_at - ankle_at).normalize_or(fwd);
    let toes = ankle + foot_fwd * skel.toe_forward.hypot(skel.ankle_height - skel.toe_height);
    let foot_rot = frame_yz(foot_fwd, up, fwd);
    LegPose { hip, knee: solved.elbow, ankle, toes, rot: [upper, lower, foot_rot, foot_rot] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Calibration;

    #[test]
    fn heel_lift_keeps_the_toes_down() {
        let p = BodyParams::default();
        let s = Skeleton::new(&p, Calibration::default());
        let foot = Foot { pos: Vec3::new(0.1, 0.0, 0.0), yaw: 0.0, step: None };
        // Hip joint a few centimetres too high for a flat foot.
        let hip = Vec3::new(0.1, (s.thigh + s.shin) + s.ankle_height + 0.03, 0.0);
        let leg = solve_leg(&p, &s, hip, &foot, 0.0, Vec3::Z);
        assert!((leg.toes.y - s.toe_height).abs() < 1e-5, "toes stay on the floor");
        assert!(leg.ankle.y > s.ankle_height + 0.01, "heel up");
        assert!((leg.knee.distance(hip) - s.thigh).abs() < 1e-3);
        assert!((leg.knee.distance(leg.ankle) - s.shin).abs() < 2e-3);
    }

    #[test]
    fn guard_keeps_feet_apart() {
        let p = BodyParams::default();
        let s = Skeleton::new(&p, Calibration::default());
        // A right-foot target across the left foot is pushed back out to the right.
        let t = cross_guard(&p, &s, Vec3::new(-0.2, 0.0, 0.0), Vec3::ZERO, 0.0, 1);
        assert!(t.x >= p.leg_cross_guard - 1e-5, "{t}");
        // Far ahead it may be narrow.
        let t = cross_guard(&p, &s, Vec3::new(0.05, 0.0, 0.5), Vec3::ZERO, 0.0, 1);
        assert!((t - Vec3::new(0.05, 0.0, 0.5)).length() < 1e-5);
    }

    #[test]
    fn hip_range_brackets_standing() {
        let p = BodyParams::default();
        let s = Skeleton::new(&p, Calibration::default());
        let foot = Foot { pos: Vec3::ZERO, yaw: 0.0, step: None };
        let standing = s.ankle_height + (s.thigh + s.shin) * 0.998;
        let (lo, hi) = hip_height_range(&p, &s, Vec3::new(0.0, standing, 0.0), &foot, 0.0);
        assert!(lo < 0.5 && hi > standing, "{lo} {hi}");
    }
}
