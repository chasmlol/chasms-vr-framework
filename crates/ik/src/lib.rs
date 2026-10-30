//! Full-body IK for a VR player: a whole body from the head and two controllers, calibrated to
//! the player, plus finger curls from the controllers' buttons and touch sensors.
//!
//! The rule from the research (`reports/VR player controller design.md`, "Three tracked points
//! can carry a body only if the body absorbs every error"): **the hands are pinned exactly to the
//! controllers.** The body bends, stretches, clamps or reports that it couldn't reach
//! ([`BodyOutput::reached`], so the runtime can fade the arm), and never pulls a hand away.
//!
//! # Spaces and frames
//!
//! Input and output are in tracking space: left-handed metres, X right, Y up, Z forward, real
//! floor at Y = 0. Output joints follow `rustvr_protocol`'s convention: 22 humanoid joints, each
//! rotation being the *absolute* orientation of the joint's canonical frame in tracking space, so
//! `rot * Y` points along the bone towards the child joint (or the fingers) and `rot * Z` is the
//! canonical forward:
//!
//! * spine chain (hips .. head): Y up the spine, Z forward; identity standing straight facing +Z;
//! * arms and hands: Y along the bone towards the fingers, Z forward in the T-pose (the thumb side
//!   for hands), X = Y × Z; in the T-pose the left arm is (X, Y, Z) = (+Y, -X, +Z) and the right
//!   arm (-Y, +X, +Z);
//! * legs: Y down the bone towards the foot, Z forward (the knee);
//! * feet and toes: Y forward towards the toes, Z up.
//!
//! The head joint's rotation is the HMD's rotation, and each hand joint is exactly
//! [`HandOffsets::hand_frame`] of its grip.
//!
//! # The solve
//!
//! Per frame, in VRIK's and FRIK's order:
//!
//! 1. **Head.** The head joint is the neck pivot, rigidly 0.13 m behind and below the eyes
//!    (Parger et al. 2018), turning with the HMD.
//! 2. **Torso yaw** from the sum of the normalized head -> hand directions (Parger), with hands
//!    near the face, above the head or crossed losing their say (FRIK), clamped to 80° of the head
//!    and blended at 0.7 (VRArmIK, FRIK), with VRArmIK's hands-behind flip detector. The feet and
//!    hips follow with VRIK's dead zone (25° standing, 10° moving).
//! 3. **Spine** pitch from the head drop and head pitch (Parger, FRIK), told apart into crouching
//!    (level gaze: the knees bend) and bending over (looking down: the spine bends, the hips go
//!    back).
//! 4. **Feet**: VRIK-style balance steps standing, FRIK's velocity-driven gait under stick
//!    locomotion, a sitting pose when seated. Legs are two-bone IK; feet stay planted on the floor
//!    and rise onto the toes before the hips are pulled down.
//! 5. **Arms**: FRIK's clavicle reach, an elbow swivel from the hand's position and twist
//!    (VRArmIK/Parger), smoothed (only the swivel), and an analytic two-bone solve with FRIK's
//!    10°–145° flexion clamps. The wrist lands exactly on the hand target.
//!
//! In water ([`TrackedInput::swim`]) the body floats instead (`swim`): the legs hang from the hips
//! and kick, and the body leans into the way it swims.
//!
//! Every constant is in [`BodyParams`], with its source. All filters are α = 1 − e^(−dt/τ) with
//! dt clamped to three nominal frames.

mod arm;
mod fingers;
mod legs;
mod math;
mod params;
mod skeleton;
mod solver;
mod spine;
mod swim;
mod torso;

pub use fingers::{FingerInput, FingerSolver, HoldShape};
pub use params::{BodyParams, HandGeometry, HandOffsets};
pub use skeleton::{NECK_LEAN, REF_EYE_HEIGHT};
pub use solver::BodySolver;

use glam::{Quat, Vec3};
use rustvr_protocol::JOINT_COUNT;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: Vec3,
    pub rot: Quat,
}

impl Pose {
    pub const IDENTITY: Pose = Pose { pos: Vec3::ZERO, rot: Quat::IDENTITY };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left = 0,
    Right = 1,
}

/// One frame of tracking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackedInput {
    /// Seconds since the last solve.
    pub dt: f32,
    /// HMD (eye-centre) pose in tracking space.
    pub head: Pose,
    /// Controller grip poses [left, right]; `None` = not tracked.
    pub grips: [Option<Pose>; 2],
    /// Velocity (m/s, tracking space) of artificial stick movement this frame.
    pub locomotion: Vec3,
    pub seated: bool,
    /// Each elbow where the headset tracks it [left, right] (a Quest's body tracking), tracking
    /// space; `None` = not tracked: the arm's own model places it.
    pub elbows: [Option<Vec3>; 2],
    /// Each shoulder joint where the headset tracks it [left, right], tracking space: with both,
    /// the chest faces square to the line between them rather than as the hands and head suggest.
    pub shoulders: [Option<Vec3>; 2],
    /// The player is swimming: the body floats.
    pub swim: Option<Swimming>,
}

/// A swimming body: how it moves through the water.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swimming {
    /// Its velocity through the water (tracking space, m/s).
    pub velocity: Vec3,
    /// The game's full swimming speed (m/s): how fast it swims is the velocity against this.
    pub full_speed: f32,
    /// How hard the arms are stroking, 0..1.
    pub effort: f32,
}

/// A seat on a mount, as the game poses its own rider on it (tracking space): the body sits there
/// instead of standing ([`BodySolver::ride`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saddle {
    /// The point the rider sits on (the saddle's seat), turned as the mount is (+Z its forward, +Y
    /// up): the hip joints are a sitting body's height over it.
    pub seat: Pose,
    /// Where the rider's knee, ankle and ball of the foot go (astride the mount), [left, right];
    /// None: the legs hang from the seat.
    pub legs: Option<[[Vec3; 3]; 2]>,
}

/// The two measurements the body is scaled from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibration {
    /// Standing eye height above the floor, metres.
    pub eye_height: f32,
    /// Grip-to-grip distance in a T-pose, metres.
    pub arm_span: f32,
}

impl Default for Calibration {
    fn default() -> Self {
        Calibration { eye_height: 1.70, arm_span: 1.70 }
    }
}

/// A pose held to measure the arms: the head and both grips (tracking space), both arms straight
/// (out to the sides, or out in front).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StraightArms {
    pub head: Pose,
    pub grips: [Pose; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyOutput {
    /// Canonical joint frames in tracking space, indexed by `rustvr_protocol::Joint as usize`.
    pub joints: [Pose; JOINT_COUNT],
    /// Avatar scale against the reference skeleton (eye height over [`REF_EYE_HEIGHT`]).
    pub scale: f32,
    /// Shoulder to wrist, metres.
    pub arm_length: f32,
    /// Where the hand joints were asked to go (from the grips; for an untracked hand, its resting
    /// place). The hand joints are always exactly here.
    pub hand_targets: [Pose; 2],
    /// False when an arm couldn't reach its hand naturally: the bones stretched, or the hand was
    /// left floating at the controller beyond the stretch (or tracking looked lost).
    pub reached: [bool; 2],
}

impl BodyOutput {
    pub fn to_protocol(&self) -> [rustvr_protocol::Pose; JOINT_COUNT] {
        self.joints.map(|j| rustvr_protocol::Pose { pos: j.pos.to_array(), valid: 1, rot: j.rot.to_array() })
    }
}
