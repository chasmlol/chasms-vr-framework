//! The spine hangs from the head: the neck pivot is rigidly attached to the HMD, and the chain
//! below it pitches with how far and how the head has dropped (Parger et al. 2018, FRIK, VRIK).

use glam::{Quat, Vec3};

use crate::math::{frame_yz, smoothstep, wrap_angle, yaw_dir, yaw_rot, UP};
use crate::skeleton::Skeleton;
use crate::BodyParams;

/// Share of the spine pitch each segment takes, hips -> spine up to upper chest -> neck: the
/// upper back curls a little more than the pelvis.
const PITCH_SHARE: [f32; 4] = [0.85, 0.95, 1.05, 1.15];
/// Share of the torso's twist over the root each joint takes, hips up to upper chest.
const TWIST_SHARE: [f32; 4] = [0.3, 0.55, 0.8, 1.0];
/// The neck bends halfway between the upper chest and the head.
const NECK_FOLLOW: f32 = 0.5;
/// Pitch limits: a little lean back when looking up, folded over at the other end.
pub const MIN_PITCH: f32 = -15.0 * std::f32::consts::PI / 180.0;
pub const MAX_PITCH: f32 = 115.0 * std::f32::consts::PI / 180.0;

/// Spine pitch (radians, positive bends forward) for an eye height `eyes_y`, a standing eye height
/// `h0` and a head pitch (radians, positive looking down).
pub fn target_pitch(p: &BodyParams, skel: &Skeleton, eyes_y: f32, h0: f32, head_pitch: f32) -> f32 {
    let h = eyes_y.max(p.min_head_height * skel.scale);
    let drop = ((h0 - h) / h0).clamp(0.0, 1.0);
    // Crouch versus bend: the drop bends the spine only as far as the gaze is down.
    let gaze = smoothstep(p.bend_gaze_start_deg, p.bend_gaze_full_deg, head_pitch.to_degrees());
    // Parger / FRIK: pitch = drop/h0 × base + k × head pitch.
    let bend = drop * p.spine_bend_deg.to_radians() * gaze + p.spine_bend_head_pitch * head_pitch;
    // VRIK moves the body back as the head drops; with the head fixed that leans the spine.
    let crouch = (p.pelvis_back * drop).min(1.0).asin();
    (bend + crouch * (1.0 - gaze)).clamp(MIN_PITCH, MAX_PITCH)
}

/// Hips, spine, chest, upper chest, neck, head: positions and canonical frames.
#[derive(Clone, Copy, Debug)]
pub struct Chain {
    pub pos: [Vec3; 6],
    pub rot: [Quat; 6],
    /// Yaw of each joint from the hips to the upper chest.
    pub yaw: [f32; 4],
}

impl Chain {
    pub fn hips(&self) -> Vec3 {
        self.pos[0]
    }

    pub fn upper_chest(&self) -> (Vec3, Quat) {
        (self.pos[3], self.rot[3])
    }

    /// Moves the hips down by `dy` (up when negative) and spreads the change over the spine, so it
    /// stretches or compresses instead of the feet leaving the floor. The upper chest stays: the
    /// shoulders, and with them the arms, are the player's own, which a walking gait (or a stretch
    /// for the feet) mustn't move.
    pub fn shift_hips(&mut self, dy: f32) {
        for (i, share) in [1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0].into_iter().enumerate() {
            self.pos[i].y -= dy * share;
        }
        self.refresh_frames();
    }

    fn refresh_frames(&mut self) {
        for i in 0..4 {
            let up = self.pos[i + 1] - self.pos[i];
            self.rot[i] = frame_yz(up, yaw_dir(self.yaw[i]), UP);
        }
    }
}

pub struct ChainInput {
    pub head_joint: Vec3,
    pub head_rot: Quat,
    pub torso_yaw: f32,
    pub root_yaw: f32,
    pub pitch: f32,
}

pub fn build(skel: &Skeleton, input: &ChainInput) -> Chain {
    let twist = wrap_angle(input.torso_yaw - input.root_yaw);
    let yaw = TWIST_SHARE.map(|s| input.root_yaw + twist * s);
    // Segment directions from each joint up to the next, pitched forward about their own right.
    let dirs: [Vec3; 4] =
        std::array::from_fn(|i| yaw_rot(yaw[i]) * (Quat::from_rotation_x(input.pitch * PITCH_SHARE[i]) * UP));
    let head_up = input.head_rot * UP;
    let neck_dir = dirs[3].lerp(head_up, NECK_FOLLOW).normalize_or(dirs[3]);
    let neck = input.head_joint - neck_dir * skel.neck;

    let mut pos = [Vec3::ZERO; 6];
    pos[5] = input.head_joint;
    pos[4] = neck;
    for i in (0..4).rev() {
        pos[i] = pos[i + 1] - dirs[i] * skel.spine[i];
    }
    let head_fwd = input.head_rot * Vec3::Z;
    let neck_fwd = yaw_dir(yaw[3]).lerp(head_fwd, NECK_FOLLOW);
    let mut chain = Chain { pos, rot: [Quat::IDENTITY; 6], yaw };
    chain.refresh_frames();
    chain.rot[4] = frame_yz(neck_dir, neck_fwd, yaw_dir(yaw[3]));
    chain.rot[5] = input.head_rot;
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Calibration;

    #[test]
    fn crouch_and_bend_are_told_apart() {
        let p = BodyParams::default();
        let s = Skeleton::new(&p, Calibration::default());
        let level = target_pitch(&p, &s, 1.2, 1.7, 0.0);
        let looking_down = target_pitch(&p, &s, 1.2, 1.7, 60f32.to_radians());
        assert!(level.to_degrees() < 15.0, "{}", level.to_degrees());
        assert!(looking_down.to_degrees() > 40.0, "{}", looking_down.to_degrees());
        // Standing tall, level: upright.
        assert!(target_pitch(&p, &s, 1.7, 1.7, 0.0).abs() < 1e-6);
        // The minimum head height caps the drop.
        assert_eq!(target_pitch(&p, &s, 0.2, 1.7, 1.0), target_pitch(&p, &s, 0.8, 1.7, 1.0));
    }

    #[test]
    fn chain_keeps_bone_lengths() {
        let p = BodyParams::default();
        let s = Skeleton::new(&p, Calibration::default());
        let input = ChainInput {
            head_joint: Vec3::new(0.0, 1.3, 0.3),
            head_rot: Quat::from_rotation_x(1.0),
            torso_yaw: 0.3,
            root_yaw: -0.2,
            pitch: 0.9,
        };
        let c = build(&s, &input);
        for i in 0..4 {
            assert!((c.pos[i].distance(c.pos[i + 1]) - s.spine[i]).abs() < 1e-5);
            assert!((c.rot[i] * UP - (c.pos[i + 1] - c.pos[i]).normalize()).length() < 1e-5);
        }
        assert!((c.pos[4].distance(c.pos[5]) - s.neck).abs() < 1e-5);
        // Bent forward: the hips are behind the neck.
        assert!(c.hips().z < c.pos[4].z - 0.3);
    }
}
