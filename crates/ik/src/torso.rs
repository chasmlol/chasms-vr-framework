//! Torso yaw from the hands, clamped by the head (Parger et al. 2018, VRArmIK, FRIK), and the
//! root (feet and hips) yaw that trails it with a dead zone (VRIK).

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::Vec3;

use crate::BodyParams;
use crate::math::{flat, smoothing, smoothstep, wrap_angle, yaw_dir, yaw_of, Follow};

/// Parger settles the both-hands-behind ambiguity with the head: a hand estimate this far from
/// the head's yaw is taken to be the flipped one.
const FLIP_BY_HEAD: f32 = 120.0 * PI / 180.0;
/// Hands straight above or below the head joint have no usable horizontal direction.
const MIN_HORIZONTAL: [f32; 2] = [0.05, 0.15];

#[derive(Clone, Copy, Debug)]
pub struct TorsoYaw {
    pub yaw: f32,
    /// The yaw as it follows its target (a critically damped follower: no jolt as it starts).
    follow: Follow,
    /// Last hand yaw with a usable magnitude, for the flip detector.
    prev_raw: Option<f32>,
    flipped: bool,
}

impl TorsoYaw {
    pub fn new(yaw: f32) -> Self {
        TorsoYaw { yaw, follow: Follow::new(yaw), prev_raw: None, flipped: false }
    }

    /// `eyes` and `head_joint` are the HMD and neck-pivot positions, `head_yaw` the head's facing,
    /// `hands` the tracked palm positions [left, right], `tracked` the chest's yaw where the
    /// headset's body tracking has it (it then wins over the hands'), `learned` the learned prior's.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        p: &BodyParams,
        dt: f32,
        scale: f32,
        eyes: Vec3,
        head_joint: Vec3,
        head_yaw: f32,
        hands: [Option<Vec3>; 2],
        tracked: Option<f32>,
        learned: Option<f32>,
    ) -> f32 {
        let (target, tau) = match hands {
            _ if tracked.is_some() => {
                self.prev_raw = None;
                (tracked.unwrap_or(head_yaw), p.torso_yaw_tau)
            }
            [Some(l), Some(r)] => {
                let offset = self.hand_offset(p, scale, eyes, head_joint, head_yaw, l, r);
                // The learned prior's chest, as far as it's trusted (`BodyParams::prior_chest`).
                let target = head_yaw + offset;
                let target = learned.map_or(target, |y| target + wrap_angle(y - target) * p.prior_chest);
                (target, p.torso_yaw_tau)
            }
            // One hand alone can't tell a turned torso from a hand held out to the side, so with
            // fewer than two hands the torso just trails the head.
            _ => {
                self.prev_raw = None;
                self.flipped = false;
                (head_yaw, p.torso_yaw_tau_head_only)
            }
        };
        self.follow.value = self.yaw;
        self.yaw = wrap_angle(self.follow.step(wrap_angle(target - self.yaw), dt, tau));
        // Parger keeps the head within 90° of the neck whatever the filter is doing.
        let off = wrap_angle(self.yaw - head_yaw);
        if off.abs() > FRAC_PI_2 {
            self.yaw = wrap_angle(head_yaw + FRAC_PI_2.copysign(off));
            self.follow.speed = 0.0;
        }
        self.yaw
    }

    /// How far the hands turn the torso from the head, already clamped and blended.
    #[allow(clippy::too_many_arguments)]
    fn hand_offset(
        &mut self,
        p: &BodyParams,
        scale: f32,
        eyes: Vec3,
        head_joint: Vec3,
        head_yaw: f32,
        left: Vec3,
        right: Vec3,
    ) -> f32 {
        let head_right = {
            let f = yaw_dir(head_yaw);
            Vec3::new(f.z, 0.0, -f.x)
        };
        let weight = |hand: Vec3| {
            // FRIK: nothing from a hand at the face, less from a hand above the head.
            let near = smoothstep(p.face_radius * scale, 2.0 * p.face_radius * scale, hand.distance(eyes));
            let above = 1.0 - ((hand.y - eyes.y).max(0.0) / (p.above_head_range * scale)).min(1.0);
            let horizontal = flat(hand - head_joint).length() / scale;
            near * above * smoothstep(MIN_HORIZONTAL[0], MIN_HORIZONTAL[1], horizontal)
        };
        // FRIK: crossed hands (left of the body's midline to the right of the right hand).
        let crossing = (left - right).dot(head_right).max(0.0);
        let uncrossed = 1.0 - (crossing / (p.hand_cross_range * scale)).min(1.0);

        // Parger: the sum of the normalized head -> hand directions.
        let dir = |hand: Vec3| flat(hand - head_joint).normalize_or_zero();
        let mut sum = dir(left) + dir(right);
        // With the hands level at the sides those directions cancel; the perpendicular of the
        // line between the hands still points forward then, as long as they aren't crossed. It
        // only fills in where the sum is weak, so it doesn't skew a two-handed hold.
        let span = flat(right - left);
        let lateral = span.dot(head_right);
        let line_weight = 2.0 * smoothstep(0.1 * scale, 0.4 * scale, lateral) * (1.0 - 0.5 * sum.length()).max(0.0);
        sum += Vec3::new(-span.z, 0.0, span.x).normalize_or_zero() * line_weight;

        let magnitude = sum.length();
        // FRIK zeroes the hand yaw when either hand is unreliable rather than letting the other
        // hand alone steer the torso, so the weights gate the confidence.
        let weights = weight(left).min(weight(right));
        let confidence = smoothstep(0.2, 0.8, magnitude) * weights * uncrossed;
        if magnitude < 0.2 {
            self.prev_raw = None;
            return 0.0;
        }
        let raw = yaw_of(sum);
        // VRArmIK's flip detector: both hands going behind the body swings the sum through zero,
        // so its yaw jumps by about 180° in one step. It only reads hands well out from under the
        // head and away from the face: bent over with the hands under it (sweeping, picking
        // something up), their directions are noise whose jumps it took for the hands going behind
        // the back, and it turned the chest round backwards, the shoulders leaping.
        let steady = weights > 0.5;
        if steady {
            if let Some(prev) = self.prev_raw {
                let jump = (raw - prev).rem_euclid(TAU).to_degrees();
                if (p.flip_jump_min_deg..=p.flip_jump_max_deg).contains(&jump) {
                    self.flipped = !self.flipped;
                }
            }
        }
        self.prev_raw = steady.then_some(raw);
        let mut offset = wrap_angle(raw + if self.flipped { PI } else { 0.0 } - head_yaw);
        if offset.abs() > FLIP_BY_HEAD {
            if steady {
                self.flipped = !self.flipped;
            }
            offset = wrap_angle(offset + PI);
        }
        let clamp = p.torso_yaw_clamp_deg.to_radians();
        p.torso_yaw_blend * confidence * offset.clamp(-clamp, clamp)
    }
}

/// The feet and hips heading: it turns only by however far the torso leaves the dead zone.
#[derive(Clone, Copy, Debug)]
pub struct RootYaw {
    pub yaw: f32,
    dead_zone: f32,
}

impl RootYaw {
    pub fn new(yaw: f32, p: &BodyParams) -> Self {
        RootYaw { yaw, dead_zone: p.root_dead_zone_standing_deg.to_radians() }
    }

    pub fn update(&mut self, p: &BodyParams, dt: f32, torso_yaw: f32, moving: bool) -> f32 {
        let target = if moving { p.root_dead_zone_moving_deg } else { p.root_dead_zone_standing_deg };
        // VRIK Animated blends between its moving and standing limits over 0.2 s.
        self.dead_zone += (target.to_radians() - self.dead_zone) * smoothing(dt, 0.2);
        let off = wrap_angle(torso_yaw - self.yaw);
        if off.abs() > self.dead_zone {
            self.yaw = wrap_angle(self.yaw + off - self.dead_zone.copysign(off));
        }
        self.yaw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(t: &mut TorsoYaw, head_yaw: f32, hands: [Option<Vec3>; 2]) -> f32 {
        let p = BodyParams::default();
        let eyes = Vec3::new(0.0, 1.7, 0.0);
        let head_joint = eyes + crate::math::yaw_rot(head_yaw) * Vec3::new(0.0, -0.09, -0.095);
        for _ in 0..360 {
            t.update(&p, 1.0 / 90.0, 1.0, eyes, head_joint, head_yaw, hands, None, None);
        }
        t.yaw
    }

    #[test]
    fn hands_at_the_sides_face_forward() {
        let mut t = TorsoYaw::new(0.3);
        let hands = [Some(Vec3::new(-0.25, 0.85, -0.05)), Some(Vec3::new(0.25, 0.85, -0.05))];
        assert!(settle(&mut t, 0.0, hands).abs() < 0.02);
    }

    #[test]
    fn one_hand_follows_the_head() {
        let mut t = TorsoYaw::new(0.0);
        let yaw = settle(&mut t, 0.8, [None, Some(Vec3::new(0.25, 1.0, 0.3))]);
        assert!((yaw - 0.8).abs() < 0.05, "{yaw}");
    }

    #[test]
    fn root_turns_only_past_the_dead_zone() {
        let p = BodyParams::default();
        let mut r = RootYaw::new(0.0, &p);
        assert_eq!(r.update(&p, 0.01, 20f32.to_radians(), false), 0.0);
        let yaw = r.update(&p, 0.01, 40f32.to_radians(), false);
        assert!((yaw.to_degrees() - 15.0).abs() < 0.1, "{}", yaw.to_degrees());
    }
}
