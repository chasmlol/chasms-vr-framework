//! The body in water: it floats from the head instead of standing on a floor. The legs hang from
//! the hips and kick (a slow treading kick at rest, a flutter kick swimming), the feet pointed,
//! and the body leans into the way it swims: forward, the hips trail behind and the legs stream out
//! behind them, as a swimmer's do; backing off, it leans back a little. The arms stay the player's
//! own. Going into the water and out of it, the swimming pose eases in and out (`weight`), and the
//! feet are kept under the body meanwhile so they're where they should be when it stands again.
//!
//! Kick rates and depths are a recreational swimmer's (est.): treading water's eggbeater about once
//! a second with the knees well bent, a flutter kick under twice a second from the hips with the
//! knees nearly straight.

use std::f32::consts::{PI, TAU};

use glam::{Quat, Vec3};

use crate::Swimming;
use crate::legs::LegPose;
use crate::math::{frame_yz, smoothing, smoothstep, yaw_dir};
use crate::skeleton::Skeleton;

/// The swimming pose eases in and out over about this long (seconds, e-folding).
const BLEND_TAU: f32 = 0.18;
/// How fast the swimming speed and effort the pose reads follow the game's (seconds).
const FOLLOW_TAU: f32 = 0.35;
/// The body's lean (radians, the spine's pitch: positive leans forward, the hips going back):
/// treading water nearly upright, swimming forward at full speed well out behind, backing off a
/// little back.
const TREAD_LEAN: f32 = 10.0 * PI / 180.0;
const SWIM_LEAN: f32 = 62.0 * PI / 180.0;
const BACK_LEAN: f32 = -12.0 * PI / 180.0;
/// Kicks a second, treading and swimming at full speed.
const TREAD_RATE: f32 = 0.9;
const SWIM_RATE: f32 = 1.7;
/// How far each thigh swings either side (radians), how bent the knees are and how much more they
/// bend in the kick's recovery, treading and swimming.
const TREAD_SWING: f32 = 0.22;
const SWIM_SWING: f32 = 0.3;
const TREAD_KNEE: [f32; 2] = [0.75, 0.45];
const SWIM_KNEE: [f32; 2] = [0.15, 0.4];
/// The thighs a little apart, and how far the feet are pointed past standing (radians).
const SPLAY: f32 = 4.0 * PI / 180.0;
const TREAD_POINT: f32 = 0.6;
const SWIM_POINT: f32 = 1.2;
/// Resting in the water, the kick keeps this share of its depth (a body treading water).
const REST_KICK: f32 = 0.35;

#[derive(Clone, Copy, Debug, Default)]
pub struct SwimState {
    /// How far the body is in its swimming pose, 0 (standing) .. 1.
    pub weight: f32,
    /// The kick's phase (radians).
    phase: f32,
    /// How fast the body swims (a share of the game's full speed): ahead of the torso (negative
    /// backing off) and in all.
    forward: f32,
    speed: f32,
    /// How hard the body works: the arms' effort or its speed, whichever's more.
    activity: f32,
}

impl SwimState {
    /// One frame: towards the swimming pose while `swim` is given, back to standing after.
    pub fn update(&mut self, dt: f32, swim: Option<&Swimming>, torso_yaw: f32) {
        let target = if swim.is_some() { 1.0 } else { 0.0 };
        self.weight += (target - self.weight) * smoothing(dt, BLEND_TAU);
        if self.weight < 1e-3 && swim.is_none() {
            self.weight = 0.0;
        }
        let (forward, speed, effort) = match swim {
            Some(s) if s.full_speed > 1e-3 && s.velocity.is_finite() => {
                let full = s.full_speed;
                ((s.velocity.dot(yaw_dir(torso_yaw)) / full).clamp(-1.0, 1.0), (s.velocity.length() / full).min(1.0), s.effort.clamp(0.0, 1.0))
            }
            _ => (0.0, 0.0, 0.0),
        };
        let a = smoothing(dt, FOLLOW_TAU);
        self.forward += (forward - self.forward) * a;
        self.speed += (speed - self.speed) * a;
        self.activity += (effort.max(speed) - self.activity) * a;
        let rate = TREAD_RATE + (SWIM_RATE - TREAD_RATE) * self.speed;
        self.phase = (self.phase + TAU * rate * dt * (0.6 + 0.4 * self.activity)).rem_euclid(TAU);
    }

    /// The spine's pitch for the swimming pose.
    pub fn lean(&self) -> f32 {
        TREAD_LEAN + (SWIM_LEAN - TREAD_LEAN) * smoothstep(0.0, 0.8, self.forward) + (BACK_LEAN - TREAD_LEAN) * smoothstep(0.0, 0.6, -self.forward)
    }

    /// Both legs floating from the hip joints (`hips` the hips joint's canonical frame).
    pub fn legs(&self, skel: &Skeleton, hips: Quat, hip_joints: [Vec3; 2]) -> [LegPose; 2] {
        let down = -(hips * Vec3::Y);
        let fwd = hips * Vec3::Z;
        let right = hips * Vec3::X;
        let s = self.speed;
        let swing = (TREAD_SWING + (SWIM_SWING - TREAD_SWING) * s) * (REST_KICK + (1.0 - REST_KICK) * self.activity);
        let knee = [TREAD_KNEE[0] + (SWIM_KNEE[0] - TREAD_KNEE[0]) * s, TREAD_KNEE[1] + (SWIM_KNEE[1] - TREAD_KNEE[1]) * s];
        let point = TREAD_POINT + (SWIM_POINT - TREAD_POINT) * s;
        std::array::from_fn(|i| {
            // The legs kick in turn: half a cycle apart.
            let phase = self.phase + PI * i as f32;
            let side = if i == 0 { -1.0 } else { 1.0 };
            let theta = swing * phase.sin();
            // The thigh swings fore and aft of hanging, a little out to its side.
            let out = Quat::from_axis_angle(fwd, side * SPLAY);
            let thigh = out * (down * theta.cos() + fwd * theta.sin());
            let knee_fwd = out * (fwd * theta.cos() - down * theta.sin());
            // The knee bends most as the leg comes back up (the recovery), least on the kick.
            let kappa = knee[0] + knee[1] * (0.5 + 0.5 * (phase + 0.5 * PI).sin());
            let shin = thigh * kappa.cos() - knee_fwd * kappa.sin();
            let shin_fwd = knee_fwd * kappa.cos() + thigh * kappa.sin();
            let hip = hip_joints[i];
            let k = hip + thigh * skel.thigh;
            let ankle = k + shin * skel.shin;
            // The foot pointed: turned from standing (toes ahead, its top up the shin) down the shin.
            let toes_dir = shin_fwd * point.cos() + shin * point.sin();
            let top = -shin * point.cos() + shin_fwd * point.sin();
            let toes = ankle + toes_dir * skel.toe_forward + top * (skel.toe_height - skel.ankle_height);
            let rot = [
                frame_yz(thigh, knee_fwd, fwd),
                frame_yz(shin, shin_fwd, fwd),
                frame_yz(toes_dir, top, -right.cross(toes_dir)),
                frame_yz(toes_dir, top, -right.cross(toes_dir)),
            ];
            LegPose { hip, knee: k, ankle, toes, rot }
        })
    }
}

/// A leg part way (`w`, 0..1) from one pose to another.
pub fn blend(a: &LegPose, b: &LegPose, w: f32) -> LegPose {
    if w <= 0.0 {
        return *a;
    }
    if w >= 1.0 {
        return *b;
    }
    LegPose {
        hip: a.hip.lerp(b.hip, w),
        knee: a.knee.lerp(b.knee, w),
        ankle: a.ankle.lerp(b.ankle, w),
        toes: a.toes.lerp(b.toes, w),
        rot: std::array::from_fn(|k| a.rot[k].slerp(b.rot[k], w).normalize()),
    }
}

