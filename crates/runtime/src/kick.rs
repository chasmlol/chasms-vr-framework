//! Kicks with untracked feet: the movement stick clicked, walking no faster than 3 m/s, kicks at
//! what the eyes look at within 2 m (a 0.2 m sphere cast from the eyes), with the foot on its side
//! (the right if it's to the right of the body's way, else the left). The foot folds up to its
//! hip's place at 0.8 of the legs' length over 0.2 s, drives out 1.2 legs' lengths from the hip
//! towards the target over another 0.2 s (no target: straight ahead at the legs' height, held
//! between the floor and 1.1 legs high), stays out 0.1 s unless it met something, and comes back
//! over 0.4 s. Only while it drives out and stays does it hit, with the Kick damager (the combat's).

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use crate::backend::HmdFrame;

/// The kick's timing (s) and its reach, as shares of the legs' length.
pub const FOLD: f64 = 0.2;
pub const EXTEND: f64 = 0.2;
pub const STAY: f64 = 0.1;
pub const RETURN: f64 = 0.4;
pub const FOLD_RATIO: f32 = 0.8;
pub const EXTEND_RATIO: f32 = 1.2;
pub const GROUND_HEIGHT_RATIO: f32 = 1.0;
pub const MAX_HEIGHT_RATIO: f32 = 1.1;
/// Its target: a sphere this wide cast this far from the eyes (m); not kicking walking faster than
/// this (m/s).
pub const RAY_RADIUS: f32 = 0.2;
pub const RAY_LENGTH: f32 = 2.0;
pub const MAX_WALK: f32 = 3.0;

/// A kick under way (tracking space).
#[derive(Clone, Copy, Debug)]
pub struct Kick {
    /// The foot (0 left, 1 right), when it started, where the foot was then, where it's kicked at.
    pub foot: usize,
    pub since: f64,
    pub from: Vec3,
    pub target: Vec3,
    /// When it met something (its stay cut short), and where it got to.
    pub met: Option<f64>,
    pub out: Vec3,
}

#[derive(Default)]
pub struct Kicks {
    pub kick: Option<Kick>,
    stick_was: bool,
}

pub struct KickInput<'a> {
    pub frame: &'a HmdFrame,
    /// The hand whose stick moves the player.
    pub stick_hand: usize,
    pub now: f64,
    /// The player's body last frame (tracking space), and its floor's height there.
    pub joints: &'a [Pose; JOINT_COUNT],
    pub floor: f32,
    /// How fast the player walks (m/s).
    pub walking: f32,
    /// What there is to kick at: the creatures' parts (tracking space capsules).
    pub targets: &'a [(Vec3, Vec3, f32)],
    pub paused: bool,
}

/// Where a kick has its foot (the ankle, tracking space), and whether it's kicking now.
#[derive(Clone, Copy, Debug, Default)]
pub struct KickOut {
    pub ankles: [Option<Vec3>; 2],
    /// The foot hitting now: which, and its sole's way (the kick's way).
    pub striking: Option<(usize, Vec3)>,
}

impl Kicks {
    pub fn update(&mut self, inp: &KickInput) -> KickOut {
        let down = inp.frame.hands[inp.stick_hand].buttons & CTRL_STICK_CLICK != 0;
        let clicked = down && !self.stick_was;
        self.stick_was = down;
        let pos = |j: Joint| Vec3::from_array(inp.joints[j as usize].pos);
        let hips = [pos(Joint::LeftUpperLeg), pos(Joint::RightUpperLeg)];
        // (The legs' length: the hips' height above the floor.)
        let legs = ((hips[0].y + hips[1].y) * 0.5 - inp.floor).max(0.3);
        if clicked && self.kick.is_none() && !inp.paused && inp.walking <= MAX_WALK {
            // (The body's joints look along their +Z.)
            let head = &inp.joints[Joint::Head as usize];
            let (eye, look) = (Vec3::from_array(head.pos), Quat::from_array(head.rot).normalize() * Vec3::Z);
            let hips_rot = Quat::from_array(inp.joints[Joint::Hips as usize].rot).normalize();
            let fwd = (hips_rot * Vec3::Z).with_y(0.0).normalize_or((look).with_y(0.0).normalize_or(Vec3::Z));
            // The nearest of what the eyes' sphere meets, else straight ahead.
            let hit = inp
                .targets
                .iter()
                .filter_map(|&(a, b, r)| {
                    let (s, t) = crate::combat::closest_segments(eye, eye + look * RAY_LENGTH, a, b);
                    let (p, q) = (eye + look * RAY_LENGTH * s, a.lerp(b, t));
                    (p.distance(q) <= r + RAY_RADIUS).then_some((s, q + (p - q).normalize_or_zero() * r))
                })
                .min_by(|x, y| x.0.total_cmp(&y.0))
                .map(|(_, p)| p);
            let side = |to: Vec3| if fwd.cross(to.with_y(0.0)).y < 0.0 { 1 } else { 0 };
            let foot = match hit {
                Some(p) => side(p - (hips[0] + hips[1]) * 0.5),
                None => side(look),
            };
            // (No target: ahead of its hip, the legs' height up, 1.2 legs out.)
            let target = hit.unwrap_or_else(|| hips[foot].with_y(inp.floor) + Vec3::Y * legs * GROUND_HEIGHT_RATIO + fwd * legs * EXTEND_RATIO);
            let from = pos(if foot == 0 { Joint::LeftFoot } else { Joint::RightFoot });
            tracing::info!("kick: the {} foot kicks at {target:.2}", if foot == 0 { "left" } else { "right" });
            self.kick = Some(Kick { foot, since: inp.now, from, target, met: None, out: from });
        }
        let Some(k) = self.kick.as_mut() else { return KickOut::default() };
        let t = inp.now - k.since;
        let hip = hips[k.foot];
        let fold = hip.with_y(inp.floor + legs * FOLD_RATIO);
        let aim = k.target.with_y(k.target.y.clamp(inp.floor, inp.floor + legs * MAX_HEIGHT_RATIO));
        let reach = hip + (aim - hip).normalize_or(Vec3::Z) * legs * EXTEND_RATIO;
        let mut out = KickOut::default();
        let ankle = if t < FOLD {
            k.from.lerp(fold, (t / FOLD) as f32)
        } else if t < FOLD + EXTEND && k.met.is_none() {
            out.striking = Some((k.foot, (reach - fold).normalize_or(Vec3::Z)));
            k.out = fold.lerp(reach, ((t - FOLD) / EXTEND) as f32);
            k.out
        } else {
            // Out: stays a moment (unless it met something), then back.
            let back_from = k.met.unwrap_or(k.since + FOLD + EXTEND + STAY);
            if inp.now < back_from {
                out.striking = Some((k.foot, (reach - fold).normalize_or(Vec3::Z)));
                k.out
            } else {
                let r = ((inp.now - back_from) / RETURN) as f32;
                if r >= 1.0 {
                    self.kick = None;
                    return out;
                }
                let rest = pos(if k.foot == 0 { Joint::LeftFoot } else { Joint::RightFoot });
                k.out.lerp(rest, r)
            }
        };
        out.ankles[k.foot] = Some(ankle);
        out
    }

    /// The kicking foot met something: it stops there and comes back.
    pub fn met(&mut self, now: f64) {
        if let Some(k) = self.kick.as_mut()
            && k.met.is_none()
        {
            k.met = Some(now);
        }
    }
}
