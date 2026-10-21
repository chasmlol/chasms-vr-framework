//! The protocol's joint convention in code, and hand-built body poses.
//!
//! [`reference_frame`] is each joint's canonical frame in the reference T-pose (facing +Z, palms
//! down). A joint's rotation in [`rustvr_protocol::RtBody::joints`] is the orientation of that
//! frame in tracking space, so a player standing in the reference pose has exactly these
//! rotations.
//!
//! The poses here (T-pose, arms down, looking down, holding a gun, reaching forward, crouching) are built
//! directly from a simple proportioned skeleton, with a two-bone solve for the limbs. They exist
//! for previews and tests that need a body without the full IK solver.

use glam::{Mat3, Quat, Vec3};
use rustvr_protocol::{Fingers, HAND_TRACKED, HandState, JOINT_COUNT, Joint, Pose};

/// The canonical frame of `joint` in the reference T-pose:
///
/// * spine chain: X right, Y up, Z forward;
/// * arms and hands: Y along the bone (outwards), Z forward (the thumb side), X = Y x Z;
/// * legs: Y down the bone, Z forward (the knee), X = Y x Z;
/// * feet and toes: Y forward (towards the toes), Z up, X = Y x Z.
pub fn reference_frame(joint: Joint) -> Quat {
    use Joint::*;
    let m = match joint {
        Hips | Spine | Chest | UpperChest | Neck | Head => Mat3::IDENTITY,
        LeftShoulder | LeftUpperArm | LeftLowerArm | LeftHand => Mat3::from_cols(Vec3::Y, -Vec3::X, Vec3::Z),
        RightShoulder | RightUpperArm | RightLowerArm | RightHand => Mat3::from_cols(-Vec3::Y, Vec3::X, Vec3::Z),
        LeftUpperLeg | LeftLowerLeg | RightUpperLeg | RightLowerLeg => Mat3::from_cols(-Vec3::X, -Vec3::Y, Vec3::Z),
        LeftFoot | LeftToes | RightFoot | RightToes => Mat3::from_cols(-Vec3::X, Vec3::Z, Vec3::Y),
    };
    Quat::from_mat3(&m)
}

/// Joint positions of a 1.75 m person in the reference pose (feet on the floor at the origin).
const REFERENCE: [[f32; 3]; JOINT_COUNT] = [
    [0.0, 0.97, 0.0],       // hips
    [0.0, 1.07, 0.0],       // spine
    [0.0, 1.19, 0.0],       // chest
    [0.0, 1.31, 0.0],       // upper chest
    [0.0, 1.46, 0.0],       // neck
    [0.0, 1.55, 0.0],       // head (top of the neck)
    [-0.025, 1.425, 0.0],   // left shoulder (clavicle)
    [-0.175, 1.425, 0.0],   // left upper arm (shoulder joint)
    [-0.465, 1.425, 0.0],   // left lower arm (elbow)
    [-0.715, 1.425, 0.0],   // left hand (wrist)
    [0.025, 1.425, 0.0],    // right shoulder
    [0.175, 1.425, 0.0],    // right upper arm
    [0.465, 1.425, 0.0],    // right lower arm
    [0.715, 1.425, 0.0],    // right hand
    [-0.09, 0.92, 0.0],     // left upper leg (hip joint)
    [-0.09, 0.50, 0.0],     // left lower leg (knee)
    [-0.09, 0.08, 0.0],     // left foot (ankle)
    [-0.09, 0.02, 0.135],   // left toes (ball of the foot)
    [0.09, 0.92, 0.0],      // right upper leg
    [0.09, 0.50, 0.0],      // right lower leg
    [0.09, 0.08, 0.0],      // right foot
    [0.09, 0.02, 0.135],    // right toes
];
const REFERENCE_HEIGHT: f32 = 1.75;

/// The centre eye relative to the head joint, in the head's frame (reference proportions).
const EYE_FROM_HEAD: Vec3 = Vec3::new(0.0, 0.095, 0.085);

/// Joint position of a person of `height` metres in the reference pose.
pub fn reference_position(joint: Joint, height: f32) -> Vec3 {
    Vec3::from_array(REFERENCE[joint as usize]) * (height / REFERENCE_HEIGHT)
}

/// Where the eyes are for this head joint pose (between the eyes, looking along the head's +Z).
pub fn eye(head: &Pose, height: f32) -> Pose {
    let rot = quat(head.rot);
    let pos = Vec3::from_array(head.pos) + rot * EYE_FROM_HEAD * (height / REFERENCE_HEIGHT);
    Pose { pos: pos.to_array(), valid: 1, rot: rot.to_array() }
}

/// The drawn hands at the hand joints (as the runtime reports them when nothing holds them back).
pub fn hands_at_joints(joints: &[Pose; JOINT_COUNT]) -> [HandState; 2] {
    let hand = |j: Joint| HandState { pose: joints[j as usize], flags: HAND_TRACKED, ..Default::default() };
    [hand(Joint::LeftHand), hand(Joint::RightHand)]
}

pub fn open_hand() -> Fingers {
    Fingers { curl: [0.0; 5], splay: 0.4, _pad: [0.0; 2] }
}

pub fn relaxed_hand() -> Fingers {
    Fingers { curl: [0.15, 0.2, 0.25, 0.3, 0.35], splay: 0.2, _pad: [0.0; 2] }
}

pub fn fist() -> Fingers {
    Fingers { curl: [1.0; 5], splay: 0.0, _pad: [0.0; 2] }
}

/// Firing hand on a pistol grip: index finger along the trigger, the rest wrapped round.
pub fn trigger_grip() -> Fingers {
    Fingers { curl: [0.65, 0.35, 0.85, 0.9, 0.9], splay: 0.0, _pad: [0.0; 2] }
}

/// Degrees a relaxed neck leans forward, with the head level on top of it. (In the reference
/// T-pose the neck is upright; standing people, and IK placing the neck below and behind the
/// headset, lean it forward. Models are usually bound that way too.)
const NECK_LEAN: f32 = 12.0;

pub fn t_pose(height: f32) -> [Pose; JOINT_COUNT] {
    Builder::new(height).joints
}

/// Standing with the arms hanging at the sides, palms in.
pub fn arms_down(height: f32) -> [Pose; JOINT_COUNT] {
    looking_down(height, 0.0)
}

/// Standing with the arms at the sides, looking down by `degrees` (the neck and head share the
/// bend).
pub fn looking_down(height: f32, degrees: f32) -> [Pose; JOINT_COUNT] {
    let mut b = Builder::new(height);
    b.spine([0.0, 0.0, 0.0, 0.0, NECK_LEAN + 0.45 * degrees, -NECK_LEAN + 0.55 * degrees]);
    b.legs_straight();
    let palm_in = frame(-Vec3::Y, Vec3::Z);
    for (side, x) in [(0, -1.0f32), (1, 1.0)] {
        let shoulder = b.pos(arm(side, 1));
        let wrist = shoulder + Vec3::new(x * 0.06, -0.53, 0.07) * b.k;
        b.arm(side, wrist, palm_in, Vec3::new(x * 0.3, 0.0, -1.0));
    }
    b.joints
}

/// Holding a rifle at chest height: the right hand on the pistol grip, the left on the foregrip,
/// head tipped down along the sights.
pub fn holding_gun(height: f32) -> [Pose; JOINT_COUNT] {
    let mut b = Builder::new(height);
    b.spine([0.0, 2.0, 2.0, 1.0, NECK_LEAN + 4.0, -NECK_LEAN + 8.0]);
    b.legs_straight();
    b.gun_hands(Vec3::ZERO);
    b.joints
}

/// Both arms stretched forward at shoulder height, palms down. `reach` is how far in front of
/// each shoulder the wrist is (left, right; 0.53 m is a straight arm at the reference size).
pub fn reaching_forward(height: f32, reach: [f32; 2]) -> [Pose; JOINT_COUNT] {
    let mut b = Builder::new(height);
    b.spine([0.0, 0.0, 0.0, 0.0, NECK_LEAN, -NECK_LEAN]);
    b.legs_straight();
    for (side, x) in [(0, -1.0f32), (1, 1.0)] {
        let shoulder = b.pos(arm(side, 1));
        let wrist = shoulder + Vec3::new(-x * 0.04, -0.06, reach[side]) * b.k;
        // Palms down with the fingers forward puts both thumbs on the inside.
        let rot = frame(Vec3::Z, Vec3::new(-x, 0.0, 0.0));
        b.arm(side, wrist, rot, Vec3::new(x, -0.6, -0.2));
    }
    b.joints
}

/// Crouched low with the feet flat, leaning forward, gun held ready.
pub fn crouch(height: f32) -> [Pose; JOINT_COUNT] {
    let mut b = Builder::new(height);
    let drop = 0.40 * b.k;
    b.hips_offset = Vec3::new(0.0, -drop, -0.10 * b.k);
    // 34 degrees of lean at the chest; the neck takes some back and the head the rest.
    b.spine([22.0, 6.0, 4.0, 2.0, -14.0, -16.0]);
    for (side, x) in [(0, -1.0f32), (1, 1.0)] {
        let ankle = Vec3::new(x * 0.13, 0.08, 0.06) * b.k;
        b.leg(side, ankle, Vec3::new(x * 0.35, 0.0, 1.0), frame(Vec3::new(x * 0.15, 0.0, 1.0).normalize(), Vec3::Y));
    }
    let chest = b.pos(Joint::UpperChest) - reference_position(Joint::UpperChest, b.height);
    b.gun_hands(chest);
    b.joints
}

/// A rotation whose Y axis is `y` and Z axis is `z` (orthogonalised against Y).
pub fn frame(y: Vec3, z: Vec3) -> Quat {
    let y = y.normalize();
    let z = (z - y * z.dot(y)).normalize();
    Quat::from_mat3(&Mat3::from_cols(y.cross(z), y, z))
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

const ARMS: [[Joint; 4]; 2] = [
    [Joint::LeftShoulder, Joint::LeftUpperArm, Joint::LeftLowerArm, Joint::LeftHand],
    [Joint::RightShoulder, Joint::RightUpperArm, Joint::RightLowerArm, Joint::RightHand],
];
const LEGS: [[Joint; 4]; 2] = [
    [Joint::LeftUpperLeg, Joint::LeftLowerLeg, Joint::LeftFoot, Joint::LeftToes],
    [Joint::RightUpperLeg, Joint::RightLowerLeg, Joint::RightFoot, Joint::RightToes],
];
const SPINE: [Joint; 6] = [Joint::Hips, Joint::Spine, Joint::Chest, Joint::UpperChest, Joint::Neck, Joint::Head];

fn arm(side: usize, i: usize) -> Joint {
    ARMS[side][i]
}

/// Builds a pose joint by joint: the spine by forward kinematics, the limbs with a two-bone solve.
struct Builder {
    height: f32,
    /// Size relative to the reference person.
    k: f32,
    hips_offset: Vec3,
    joints: [Pose; JOINT_COUNT],
}

impl Builder {
    fn new(height: f32) -> Builder {
        let mut joints = [Pose::IDENTITY; JOINT_COUNT];
        for j in Joint::ALL {
            joints[j as usize] = Pose { pos: reference_position(j, height).to_array(), valid: 1, rot: reference_frame(j).to_array() };
        }
        Builder { height, k: height / REFERENCE_HEIGHT, hips_offset: Vec3::ZERO, joints }
    }

    fn pos(&self, j: Joint) -> Vec3 {
        Vec3::from_array(self.joints[j as usize].pos)
    }

    fn rot(&self, j: Joint) -> Quat {
        Quat::from_array(self.joints[j as usize].rot)
    }

    fn set(&mut self, j: Joint, pos: Vec3, rot: Quat) {
        self.joints[j as usize] = Pose { pos: pos.to_array(), valid: 1, rot: rot.to_array() };
    }

    fn reference_offset(&self, child: Joint, parent: Joint) -> Vec3 {
        reference_position(child, self.height) - reference_position(parent, self.height)
    }

    /// Bends the spine chain forward: each joint pitches by `pitch_deg` relative to its parent
    /// (negative tips back up). The shoulders follow the upper chest.
    fn spine(&mut self, pitch_deg: [f32; 6]) {
        let mut rot = Quat::IDENTITY;
        let mut pos = reference_position(Joint::Hips, self.height) + self.hips_offset;
        for (i, j) in SPINE.into_iter().enumerate() {
            if i > 0 {
                pos += rot * self.reference_offset(j, SPINE[i - 1]);
            }
            // A positive rotation about +X turns +Y (up) towards +Z (forward).
            rot *= Quat::from_rotation_x(pitch_deg[i].to_radians());
            self.set(j, pos, rot * reference_frame(j));
        }
        let chest = self.rot(Joint::UpperChest);
        let chest_pos = self.pos(Joint::UpperChest);
        for side in 0..2 {
            let shoulder = chest_pos + chest * self.reference_offset(arm(side, 0), Joint::UpperChest);
            self.set(arm(side, 0), shoulder, chest * reference_frame(arm(side, 0)));
            let upper = shoulder + chest * self.reference_offset(arm(side, 1), arm(side, 0));
            self.set(arm(side, 1), upper, chest * reference_frame(arm(side, 1)));
        }
    }

    /// Places the wrist at `wrist` with the hand frame `hand`, the elbow pointing towards `pole`.
    fn arm(&mut self, side: usize, wrist: Vec3, hand: Quat, pole: Vec3) {
        let [_, upper_j, lower_j, hand_j] = ARMS[side];
        let shoulder = self.pos(upper_j);
        let l1 = self.reference_offset(lower_j, upper_j).length();
        let l2 = self.reference_offset(hand_j, lower_j).length();
        let (elbow, wrist, bend) = two_bone(shoulder, l1, l2, wrist, pole);
        // The elbow is the hinge: its axis is X of both arm frames. With the elbow pointing back
        // (-Z in the reference) that's bend x bone.
        let up = (elbow - shoulder).normalize();
        let fore = (wrist - elbow).normalize();
        let hinge = bend.cross(up).normalize();
        self.set(upper_j, shoulder, Quat::from_mat3(&Mat3::from_cols(hinge, up, hinge.cross(up))));
        self.set(lower_j, elbow, Quat::from_mat3(&Mat3::from_cols(hinge, fore, hinge.cross(fore))));
        self.set(hand_j, wrist, hand);
    }

    /// Places the ankle with the foot frame `foot`, the knee pointing towards `pole`.
    fn leg(&mut self, side: usize, ankle: Vec3, pole: Vec3, foot: Quat) {
        let [upper_j, lower_j, foot_j, toes_j] = LEGS[side];
        let hips = self.rot(Joint::Hips);
        let hip = self.pos(Joint::Hips) + hips * self.reference_offset(upper_j, Joint::Hips);
        let l1 = self.reference_offset(lower_j, upper_j).length();
        let l2 = self.reference_offset(foot_j, lower_j).length();
        let (knee, ankle, bend) = two_bone(hip, l1, l2, ankle, pole);
        // The knee points along +Z of the leg frames, so the hinge (X) is bone x bend.
        let thigh = (knee - hip).normalize();
        let shin = (ankle - knee).normalize();
        let hinge = thigh.cross(bend).normalize();
        self.set(upper_j, hip, Quat::from_mat3(&Mat3::from_cols(hinge, thigh, hinge.cross(thigh))));
        self.set(lower_j, knee, Quat::from_mat3(&Mat3::from_cols(hinge, shin, hinge.cross(shin))));
        self.set(foot_j, ankle, foot);
        let toes = ankle + foot * reference_frame(foot_j).inverse() * self.reference_offset(toes_j, foot_j);
        self.set(toes_j, toes, foot);
    }

    fn legs_straight(&mut self) {
        for (side, x) in [(0, -1.0f32), (1, 1.0)] {
            let ankle = reference_position(LEGS[side][2], self.height) + Vec3::new(x * 0.01, 0.0, 0.0);
            self.leg(side, ankle, Vec3::Z, reference_frame(LEGS[side][2]));
        }
    }

    /// The rifle hold, shifted by `offset` (the chest's movement from standing).
    fn gun_hands(&mut self, offset: Vec3) {
        // Right hand on the pistol grip: fingers forward and a little down, thumb up, palm left.
        let grip = Vec3::new(0.12, 1.29, 0.36) * self.k + offset;
        let grip_rot = frame(Vec3::new(0.0, -0.42, 0.9), Vec3::Y);
        self.arm(1, grip, grip_rot, Vec3::new(0.6, -0.8, -0.3));
        // Left hand under the handguard: fingers forward-right, thumb up, palm right.
        let fore = Vec3::new(0.0, 1.25, 0.60) * self.k + offset;
        let fore_rot = frame(Vec3::new(0.5, -0.2, 0.85), Vec3::Y);
        self.arm(0, fore, fore_rot, Vec3::new(-0.5, -0.9, -0.2));
    }
}

/// Two-bone solve from `root` towards `target` with segment lengths `l1`, `l2`, bending towards
/// `pole`. Returns the middle joint, the end (on the target when it's in reach) and the bend
/// direction (unit, perpendicular to root -> end).
fn two_bone(root: Vec3, l1: f32, l2: f32, target: Vec3, pole: Vec3) -> (Vec3, Vec3, Vec3) {
    let to = target - root;
    let reach = (l1 + l2) * 0.999;
    let dist = to.length().clamp(1e-4, reach);
    let dir = to.normalize_or(Vec3::Z);
    let bend = (pole - dir * pole.dot(dir)).normalize_or(dir.any_orthonormal_vector());
    let cos_a = ((l1 * l1 + dist * dist - l2 * l2) / (2.0 * l1 * dist)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).sqrt();
    let mid = root + dir * (l1 * cos_a) + bend * (l1 * sin_a);
    let end = root + dir * dist;
    (mid, end, bend)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_frames_follow_the_convention() {
        for j in Joint::ALL {
            let m = Mat3::from_quat(reference_frame(j));
            assert!((m.determinant() - 1.0).abs() < 1e-5);
            assert!(m.x_axis.abs_diff_eq(m.y_axis.cross(m.z_axis), 1e-5), "{j:?}: X = Y x Z");
            // Y points at the child joint in the reference pose.
            let child = Joint::ALL.into_iter().find(|c| c.parent() == Some(j) && !matches!(c, Joint::LeftShoulder | Joint::RightShoulder | Joint::LeftUpperLeg | Joint::RightUpperLeg));
            if let Some(c) = child {
                let d = (reference_position(c, 1.75) - reference_position(j, 1.75)).normalize();
                assert!(m.y_axis.dot(d) > 0.9, "{j:?}: Y towards {c:?}");
            }
        }
        let palm_down_left = Mat3::from_quat(reference_frame(Joint::LeftHand));
        assert!(palm_down_left.z_axis.abs_diff_eq(Vec3::Z, 1e-6), "thumb forward");
    }

    #[test]
    fn poses_keep_bone_lengths_and_frames() {
        for joints in [arms_down(1.8), looking_down(1.8, 60.0), holding_gun(1.8), reaching_forward(1.8, [0.53, 0.4]), crouch(1.8)] {
            for j in Joint::ALL {
                let p = &joints[j as usize];
                let m = Mat3::from_quat(Quat::from_array(p.rot));
                assert!((m.determinant() - 1.0).abs() < 1e-4, "{j:?}");
                let Some(parent) = j.parent() else { continue };
                let want = (reference_position(j, 1.8) - reference_position(parent, 1.8)).length();
                let got = (Vec3::from_array(p.pos) - Vec3::from_array(joints[parent as usize].pos)).length();
                assert!((want - got).abs() < 1e-3, "{j:?}: {got} vs {want}");
            }
            // Limb Y axes point at their child joint.
            for chain in ARMS.iter().chain(LEGS.iter()) {
                for w in chain.windows(2).skip(if chain[0] == Joint::LeftShoulder || chain[0] == Joint::RightShoulder { 1 } else { 0 }).take(2) {
                    let y = Quat::from_array(joints[w[0] as usize].rot) * Vec3::Y;
                    let d = (Vec3::from_array(joints[w[1] as usize].pos) - Vec3::from_array(joints[w[0] as usize].pos)).normalize();
                    assert!(y.dot(d) > 0.999, "{:?}", w[0]);
                }
            }
        }
    }
}
