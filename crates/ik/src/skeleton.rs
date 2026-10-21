//! The reference skeleton: a generic humanoid scaled to the player's calibration.

use glam::{Quat, Vec3};

use crate::{BodyParams, Calibration, Side};

/// Eye height of the reference skeleton; `scale` is the player's eye height over this.
pub const REF_EYE_HEIGHT: f32 = 1.70;

/// Legs are made slightly longer than the standing hip-to-ankle distance so a standing body keeps
/// a few degrees of knee bend (FRIK's leg slack "to preserve slight knee bend"); a dead-straight
/// knee has no defined bend direction.
const LEG_SLACK: f32 = 0.998;

/// Lengths of one player's body, in metres.
#[derive(Clone, Copy, Debug)]
pub struct Skeleton {
    pub scale: f32,
    pub eye_height: f32,
    /// Eyes -> head joint, in head (HMD) space.
    pub eye_to_head: Vec3,
    /// Head joint -> neck joint.
    pub neck: f32,
    /// Hips -> spine, spine -> chest, chest -> upper chest, upper chest -> neck.
    pub spine: [f32; 4],
    /// Shoulder line above the upper chest joint.
    pub shoulder_up: f32,
    /// Clavicle roots either side of the midline.
    pub clavicle_root: f32,
    /// Half the distance between the shoulder joints.
    pub shoulder_half: f32,
    pub upper_arm: f32,
    pub lower_arm: f32,
    /// Hip joints either side of, and below, the hips joint.
    pub hip_half: f32,
    pub hip_down: f32,
    pub thigh: f32,
    pub shin: f32,
    /// Ankle above the sole.
    pub ankle_height: f32,
    /// Ball of the foot (the toes joint) ahead of the ankle, and its height above the sole.
    pub toe_forward: f32,
    pub toe_height: f32,
    /// Hip joints above the seat when sitting.
    pub sit_hip_height: f32,
}

impl Skeleton {
    pub fn new(params: &BodyParams, calibration: Calibration) -> Self {
        let eye_height = calibration.eye_height;
        let s = eye_height / REF_EYE_HEIGHT;
        // The neck pivot direction comes from the legacy Oculus neck model (7.5 cm down, 8.05 cm
        // back from the eyes); its length is Parger's 0.13 m neck-to-head distance.
        let eye_to_head = Vec3::new(0.0, -0.075, -0.0805).normalize() * params.neck_to_eyes * s;
        let neck = 0.11 * s;
        let spine = [0.12 * s, 0.13 * s, 0.13 * s, 0.14 * s];
        let hip_down = 0.06 * s;
        let ankle_height = 0.08 * s;
        // Standing, everything hangs from the eyes: the hip joints end up this high.
        let hip_joint_y = eye_height + eye_to_head.y - neck - spine.iter().sum::<f32>() - hip_down;
        let leg = (hip_joint_y - ankle_height).max(0.2 * s) / LEG_SLACK;

        // Parger's calibration: arm segment = (span - shoulder width)/4 with the span measured
        // controller to controller. Our forearm ends at the wrist, so the palm-to-wrist reach of
        // both hands comes off the span first: in the T-pose the right controller points forward
        // palm down (grip +X up), and the wrist sits that far inside the palm centre along +X.
        let right = params.hand_offsets.grip_to_hand[Side::Right as usize];
        let wrist_reach = (-(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * right.pos).x).max(0.0);
        let shoulder_width = params.shoulder_width * s;
        let segment = ((calibration.arm_span - 2.0 * wrist_reach - shoulder_width) / 4.0).clamp(0.12, 0.6);

        Skeleton {
            scale: s,
            eye_height,
            eye_to_head,
            neck,
            spine,
            shoulder_up: 0.08 * s,
            clavicle_root: 0.025 * s,
            shoulder_half: shoulder_width * 0.5,
            upper_arm: segment,
            lower_arm: segment,
            hip_half: 0.09 * s,
            hip_down,
            thigh: 0.49 * leg,
            shin: 0.51 * leg,
            ankle_height,
            toe_forward: 0.13 * s,
            toe_height: 0.02 * s,
            sit_hip_height: 0.09 * s,
        }
    }

    pub fn arm_length(&self) -> f32 {
        self.upper_arm + self.lower_arm
    }

    pub fn spine_length(&self) -> f32 {
        self.spine.iter().sum()
    }

    /// Eye height when sitting upright on a seat `seat_height` high: the virtual standing height
    /// the spine formulas use in seated mode.
    pub fn seated_eye_height(&self, seat_height: f32) -> f32 {
        let hips_to_eyes = -self.eye_to_head.y + self.neck + self.spine_length() + self.hip_down;
        seat_height * self.scale + self.sit_hip_height + hips_to_eyes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_body_is_plausible() {
        let s = Skeleton::new(&BodyParams::default(), Calibration::default());
        assert!((s.scale - 1.0).abs() < 1e-6);
        // About 0.6 m shoulder to wrist and 0.84 m of leg for a 1.70 m eye height.
        assert!((0.55..0.68).contains(&s.arm_length()), "{}", s.arm_length());
        assert!((0.8..0.9).contains(&(s.thigh + s.shin)), "{}", (s.thigh + s.shin));
        assert!((s.eye_to_head.length() - 0.13).abs() < 1e-5);
        let seated = s.seated_eye_height(0.45);
        assert!((1.15..1.4).contains(&seated), "{seated}");
    }
}
