//! The per-frame body solve, in VRIK's and FRIK's order: head, torso yaw, spine, feet and legs,
//! then arms last so nothing after them can move a hand.

use glam::{Quat, Vec3};
use rustvr_protocol::{JOINT_COUNT, Joint};

use crate::arm::{self, ArmInput, ArmState};
use crate::legs::{self, Feet, FeetInput};
use crate::math::{clamp_dt, flat, frame_yz, sanitize_pos, sanitize_rot, smoothing, wrap_angle, yaw_of, UP};
use crate::skeleton::Skeleton;
use crate::spine::{self, ChainInput, MAX_PITCH};
use crate::torso::{RootYaw, TorsoYaw};
use crate::{BodyOutput, BodyParams, Calibration, Pose, Saddle, Side, StraightArms, TrackedInput};

/// In a saddle the chest turns with the head this far at most either way (radians): looking
/// round from a horse's back turns the shoulders a little, the hips stay with the mount.
const SADDLE_TWIST: f32 = 50.0 * std::f32::consts::PI / 180.0;
/// Smoothing of the centre-of-mass and head velocities the stepping logic reads (est.).
const VELOCITY_TAU: f32 = 0.1;
/// An untracked hand eases from where it was lost to hanging at the side (est.).
const LOST_HAND_TAU: f32 = 0.15;
/// Calibration limits: anything outside is a bad measurement.
const EYE_HEIGHT_RANGE: [f32; 2] = [0.9, 2.4];
const ARM_SPAN_RANGE: [f32; 2] = [0.9, 2.6];

#[derive(Clone, Copy, Debug)]
struct State {
    torso: TorsoYaw,
    root: RootYaw,
    pitch: f32,
    feet: Feet,
    arms: [ArmState; 2],
    com: Vec3,
    com_vel: Vec3,
    eyes: Vec3,
    head_speed: f32,
    /// How far the hips are pulled down to keep the feet planted (falls back slowly).
    hip_drop: f32,
    /// Each hand's last target in upper chest space, which an untracked hand eases on from.
    loose_hand: [Pose; 2],
}

pub struct BodySolver {
    params: BodyParams,
    calibration: Calibration,
    skel: Skeleton,
    state: Option<State>,
    /// Riding: the seat the body sits in instead of standing ([`BodySolver::ride`]).
    saddle: Option<Saddle>,
}

impl BodySolver {
    pub fn new(params: BodyParams, calibration: Calibration) -> Self {
        let calibration = sanitize_calibration(calibration, Calibration::default());
        BodySolver { skel: Skeleton::new(&params, calibration), params, calibration, state: None, saddle: None }
    }

    /// Riding: the body sits in this saddle (the game's own seat on its mount) from the next solve
    /// until it's taken away (None: standing again, the feet under the head).
    pub fn ride(&mut self, saddle: Option<Saddle>) {
        if saddle.is_none() && self.saddle.is_some() {
            self.state = None;
        }
        self.saddle = saddle.filter(|s| s.seat.pos.is_finite() && s.seat.rot.is_finite() && s.legs.is_none_or(|l| l.iter().flatten().all(|v| v.is_finite())));
    }

    pub fn calibration(&self) -> Calibration {
        self.calibration
    }

    /// The drawn model's arm proportions (its forearm's share of the arm): the arm's length stays
    /// the player's, shared out as the model's.
    pub fn set_forearm_share(&mut self, share: f32) {
        if (self.params.forearm_share - share).abs() > 1e-4 {
            self.params.forearm_share = share;
            self.skel = Skeleton::new(&self.params, self.calibration);
        }
    }

    pub fn set_calibration(&mut self, c: Calibration) {
        self.calibration = sanitize_calibration(c, self.calibration);
        self.skel = Skeleton::new(&self.params, self.calibration);
    }

    /// The arm span (as [`Calibration::arm_span`] takes it) at which the arms just come straight in
    /// every pose given: as this solver lays the arm out from where the shoulders rest, each arm
    /// reaching its length plus the eased last stretch to straight (`BodyParams::arm_ease`), the
    /// shoulder's share, so it's straight just where the player's is. None when no span in range
    /// does it.
    pub fn straight_arm_span(&self, poses: &[StraightArms]) -> Option<f32> {
        let slack = self.params.arm_ease;
        if poses.is_empty() {
            return None;
        }
        let eye = self.calibration.eye_height;
        // How far the least stretched arm reaches past its length (a share of it), at a span.
        let reach = |span: f32| {
            let mut least = f32::INFINITY;
            for pose in poses {
                // Standing straight: the head at the eye height (the arms only care where the
                // hands are from it).
                let lift = Vec3::new(0.0, eye - pose.head.pos.y, 0.0);
                // (The shoulders at rest: the arm as long as reaches the hand from there.)
                let params = BodyParams { shoulder_reach: [0.0; 5], ..self.params };
                let mut s = BodySolver::new(params, Calibration { eye_height: eye, arm_span: span });
                let input = TrackedInput {
                    dt: self.params.nominal_dt,
                    head: Pose { pos: pose.head.pos + lift, rot: pose.head.rot },
                    grips: pose.grips.map(|g| Some(Pose { pos: g.pos + lift, rot: g.rot })),
                    locomotion: Vec3::ZERO,
                    seated: false,
                    elbows: [None; 2],
                };
                let mut out = s.solve(&input);
                for _ in 0..30 {
                    out = s.solve(&input);
                }
                for (shoulder, hand) in [(Joint::LeftUpperArm, Joint::LeftHand), (Joint::RightUpperArm, Joint::RightHand)] {
                    let d = out.joints[shoulder as usize].pos.distance(out.joints[hand as usize].pos);
                    least = least.min(d / out.arm_length - 1.0);
                }
            }
            least
        };
        // A longer arm reaches less far past its length.
        let [mut lo, mut hi] = ARM_SPAN_RANGE;
        if reach(lo) < slack || reach(hi) > slack {
            return None;
        }
        for _ in 0..20 {
            let mid = 0.5 * (lo + hi);
            if reach(mid) > slack { lo = mid } else { hi = mid }
        }
        Some(lo)
    }

    /// "Stand straight and look ahead": the eye height.
    pub fn calibrate_standing(&mut self, head: Pose) {
        self.set_calibration(Calibration { eye_height: head.pos.y, ..self.calibration });
    }

    /// A standing T-pose (Parger calibrates from one): eye height from the head and arm span from
    /// the grips.
    pub fn calibrate_tpose(&mut self, head: Pose, left: Pose, right: Pose) {
        self.set_calibration(Calibration { eye_height: head.pos.y, arm_span: left.pos.distance(right.pos) });
    }

    pub fn solve(&mut self, input: &TrackedInput) -> BodyOutput {
        let out = self.solve_frame(input);
        if out.joints.iter().all(|j| j.pos.is_finite() && j.rot.is_finite()) {
            return out;
        }
        // Inputs are sanitized, so this is a solver bug; start over rather than hand NaN on.
        self.state = None;
        let neutral = TrackedInput {
            dt: 0.0,
            head: Pose { pos: Vec3::new(0.0, self.skel.eye_height, 0.0), rot: Quat::IDENTITY },
            grips: [None, None],
            locomotion: Vec3::ZERO,
            seated: false,
            elbows: [None, None],
        };
        let out = self.solve_frame(&neutral);
        self.state = None;
        out
    }

    fn solve_frame(&mut self, input: &TrackedInput) -> BodyOutput {
        let saddle = self.saddle;
        let p = &self.params;
        let skel = &self.skel;
        let s = skel.scale;
        let dt = clamp_dt(input.dt, p.nominal_dt * p.max_dt_frames);

        // ---- head ----
        let head_rot = sanitize_rot(input.head.rot);
        let eyes = sanitize_pos(input.head.pos, Vec3::new(0.0, skel.eye_height, 0.0));
        let head_joint = eyes + head_rot * skel.eye_to_head;
        let fwd = head_rot * Vec3::Z;
        // Facing: looking straight down, the top of the head still points the way you face.
        let facing = flat(fwd - head_rot * UP * fwd.y);
        let head_pitch = (-fwd.y).clamp(-1.0, 1.0).asin();
        let grips = input.grips.map(|g| {
            g.filter(|g| g.pos.is_finite() && g.rot.is_finite())
                .map(|g| Pose { pos: g.pos, rot: sanitize_rot(g.rot) })
        });
        let locomotion = flat(sanitize_pos(input.locomotion, Vec3::ZERO));

        let state = self.state.get_or_insert_with(|| {
            let yaw = if facing.length_squared() > 1e-8 { yaw_of(facing) } else { 0.0 };
            State {
                torso: TorsoYaw::new(yaw),
                root: RootYaw::new(yaw, p),
                pitch: 0.0,
                feet: Feet::new(p, skel, head_joint, yaw),
                arms: [ArmState::default(); 2],
                com: eyes,
                com_vel: Vec3::ZERO,
                eyes,
                head_speed: 0.0,
                hip_drop: 0.0,
                loose_hand: [Side::Left, Side::Right].map(|side| rest_hand(skel, side)),
            }
        });
        let head_yaw = if facing.length_squared() > 1e-8 { yaw_of(facing) } else { state.torso.yaw };
        let velocity_a = smoothing(dt, VELOCITY_TAU);
        if dt > 0.0 {
            let speed = flat(eyes - state.eyes).length() / dt;
            state.head_speed += (speed - state.head_speed) * velocity_a;
        }
        state.eyes = eyes;
        let moving = locomotion.length() > p.moving_speed || state.head_speed > p.moving_speed || state.feet.walking();

        let hip_joints = |c: &spine::Chain| {
            [-1.0, 1.0].map(|sign| c.pos[0] + c.rot[0] * Vec3::new(sign * skel.hip_half, -skel.hip_down, 0.0))
        };
        let (chain, legs) = match saddle {
            // In a saddle: the hips where the game seats its rider (turned as the mount is), the
            // spine from them to the head wherever it is, the chest turning part way with the head
            // as the rider looks round, the legs astride to the game's riding pose.
            Some(saddle) => {
                let fwd = flat(saddle.seat.rot * Vec3::Z);
                let mount_yaw = if fwd.length_squared() > 1e-8 { yaw_of(fwd) } else { head_yaw };
                let torso_yaw = mount_yaw + wrap_angle(head_yaw - mount_yaw).clamp(-SADDLE_TWIST, SADDLE_TWIST);
                // (On their feet again, the body starts from here.)
                state.torso = TorsoYaw::new(torso_yaw);
                state.root = RootYaw::new(mount_yaw, p);
                state.feet = Feet::new(p, skel, head_joint, mount_yaw);
                state.pitch = 0.0;
                state.hip_drop = 0.0;
                let up = (saddle.seat.rot * UP).normalize_or(UP);
                // (The hip joints sit over the seat, the hips joint over them.)
                let hips = saddle.seat.pos + up * (skel.sit_hip_height + skel.hip_down);
                let input = ChainInput { head_joint, head_rot, torso_yaw, root_yaw: mount_yaw, pitch: 0.0 };
                let chain = spine::seated(skel, &input, hips, up, saddle.seat.rot * Vec3::Z);
                let joints = hip_joints(&chain);
                let legs = [0, 1].map(|i| legs::solve_saddle_leg(p, skel, joints[i], &saddle, i));
                (chain, legs)
            }
            None => {
            // ---- torso ----
            let palms = grips.map(|g| g.map(|g| g.pos));
            let torso_yaw = state.torso.update(p, dt, s, eyes, head_joint, head_yaw, palms);
            let root_yaw = state.root.update(p, dt, torso_yaw, moving);

            // ---- spine ----
            let h0 = if input.seated { skel.seated_eye_height(p.seat_height) } else { skel.eye_height };
            // The drop is the neck's, not the eyes': nodding swings the eyes down about the neck
            // without the body going anywhere. This is the eye height with a level gaze.
            let level_eyes_y = head_joint.y - skel.eye_to_head.y;
            let target = spine::target_pitch(p, skel, level_eyes_y, h0, head_pitch);
            state.pitch += (target - state.pitch) * smoothing(dt, p.spine_tau);
            let chain_at = |pitch| spine::build(skel, &ChainInput { head_joint, head_rot, torso_yaw, root_yaw, pitch });
            let mut chain = chain_at(state.pitch);

            // ---- feet ----
            let mut com = chain.hips() + head_joint;
            let mut weight = 2.0;
            for palm in palms.iter().flatten() {
                // VRIK's centre of mass: pelvis and head 1 each, hands 0.2.
                com += *palm * 0.2;
                weight += 0.2;
            }
            let com = com / weight;
            if dt > 0.0 {
                state.com_vel = state.com_vel.lerp((com - state.com) / dt, velocity_a);
            }
            state.com = com;
            let seated_hips = input.seated.then(|| {
                let [l, r] = hip_joints(&chain);
                (l + r) * 0.5
            });
            let feet_input = FeetInput { com, com_vel: state.com_vel, root_yaw, locomotion, seated_hips };
            state.feet.update(p, skel, dt, &feet_input);
            let lifts = state.feet.feet.map(|f| f.lift(p, s));

            // ---- hips: keep both feet planted ----
            let hips_range = |c: &spine::Chain| {
                let joints = hip_joints(c);
                let mut range = (f32::MIN, f32::MAX);
                for i in 0..2 {
                    let (lo, hi) = legs::hip_height_range(p, skel, joints[i], &state.feet.feet[i], lifts[i]);
                    let offset = joints[i].y - c.hips().y;
                    range = (range.0.max(lo - offset), range.1.min(hi - offset));
                }
                range
            };
            let (low, _) = hips_range(&chain);
            if chain.hips().y < low {
                // Too low for the knees: bend the spine further (that raises the hips), then squash it.
                let (mut lo, mut hi) = (state.pitch, MAX_PITCH);
                if chain_at(hi).hips().y >= low {
                    for _ in 0..12 {
                        let mid = 0.5 * (lo + hi);
                        if chain_at(mid).hips().y >= low { hi = mid } else { lo = mid }
                    }
                }
                state.pitch = hi;
                chain = chain_at(hi);
                let short = low - chain.hips().y;
                if short > 0.0 {
                    chain.shift_hips(-short);
                }
            }
            let (low, high) = hips_range(&chain);
            // Too high to reach the feet: pull the hips down (the spine stretches). The pull is held and
            // released slowly so a walking gait doesn't bob the body every step.
            let excess = (chain.hips().y - high).max(0.0);
            state.hip_drop = if excess >= state.hip_drop {
                excess
            } else {
                state.hip_drop + (excess - state.hip_drop) * smoothing(dt, p.hip_release_tau)
            };
            let drop = state.hip_drop.min((chain.hips().y - low).max(0.0));
            if drop > 0.0 {
                chain.shift_hips(drop);
            }

            // ---- legs ----
            let hips_fwd = chain.rot[0] * Vec3::Z;
            let joints_at_hips = hip_joints(&chain);
            let legs = [0, 1].map(|i| legs::solve_leg(p, skel, joints_at_hips[i], &state.feet.feet[i], lifts[i], hips_fwd));
                (chain, legs)
            }
        };

        // ---- arms ----
        let (chest_pos, chest_rot) = chain.upper_chest();
        let mut targets = [Pose { pos: Vec3::ZERO, rot: Quat::IDENTITY }; 2];
        let arms: [arm::ArmPose; 2] = std::array::from_fn(|i| {
            let side = [Side::Left, Side::Right][i];
            let local = match grips[i] {
                Some(grip) => {
                    let world = p.hand_offsets.hand_frame(side, grip);
                    Pose { pos: chest_rot.inverse() * (world.pos - chest_pos), rot: chest_rot.inverse() * world.rot }
                }
                None => {
                    let rest = rest_hand(skel, side);
                    let a = smoothing(dt, LOST_HAND_TAU);
                    let last = state.loose_hand[i];
                    Pose { pos: last.pos.lerp(rest.pos, a), rot: last.rot.slerp(rest.rot, a).normalize() }
                }
            };
            state.loose_hand[i] = local;
            let target = match grips[i] {
                // A tracked hand goes exactly where the controller says, not through chest space.
                Some(grip) => p.hand_offsets.hand_frame(side, grip),
                None => Pose { pos: chest_pos + chest_rot * local.pos, rot: (chest_rot * local.rot).normalize() },
            };
            targets[i] = target;
            // (A tracked elbow that isn't a number is no elbow.)
            let elbow = input.elbows[i].filter(|e| e.is_finite());
            let input = ArmInput { side, chest_pos, chest_rot, target, elbow };
            arm::solve(p, skel, dt, &mut state.arms[i], &input)
        });

        // ---- output ----
        let mut frames = [Pose { pos: Vec3::ZERO, rot: Quat::IDENTITY }; JOINT_COUNT];
        let chain_joints = [Joint::Hips, Joint::Spine, Joint::Chest, Joint::UpperChest, Joint::Neck, Joint::Head];
        for (k, j) in chain_joints.into_iter().enumerate() {
            frames[j as usize] = Pose { pos: chain.pos[k], rot: chain.rot[k] };
        }
        let arm_joints = [
            [Joint::LeftShoulder, Joint::LeftUpperArm, Joint::LeftLowerArm, Joint::LeftHand],
            [Joint::RightShoulder, Joint::RightUpperArm, Joint::RightLowerArm, Joint::RightHand],
        ];
        for (i, a) in arms.iter().enumerate() {
            let pos = [a.clavicle_root, a.shoulder, a.elbow, targets[i].pos];
            for k in 0..4 {
                frames[arm_joints[i][k] as usize] = Pose { pos: pos[k], rot: a.rot[k] };
            }
        }
        let leg_joints = [
            [Joint::LeftUpperLeg, Joint::LeftLowerLeg, Joint::LeftFoot, Joint::LeftToes],
            [Joint::RightUpperLeg, Joint::RightLowerLeg, Joint::RightFoot, Joint::RightToes],
        ];
        for (i, l) in legs.iter().enumerate() {
            let pos = [l.hip, l.knee, l.ankle, l.toes];
            for k in 0..4 {
                frames[leg_joints[i][k] as usize] = Pose { pos: pos[k], rot: l.rot[k] };
            }
        }
        let joints = frames.map(|f| Pose { pos: f.pos, rot: f.rot.normalize() });
        let hand_targets = [Joint::LeftHand, Joint::RightHand].map(|j| joints[j as usize]);
        BodyOutput {
            joints,
            scale: s,
            arm_length: skel.arm_length(),
            hand_targets,
            reached: [arms[0].reached, arms[1].reached],
        }
    }
}

/// A hand hanging relaxed at the side, in upper chest space: fingers down, thumb forward.
fn rest_hand(skel: &Skeleton, side: Side) -> Pose {
    let sign = if side == Side::Left { -1.0 } else { 1.0 };
    let pos = Vec3::new(sign * (skel.shoulder_half + 0.03 * skel.scale), skel.shoulder_up - skel.arm_length() * 0.99, 0.03 * skel.scale);
    Pose { pos, rot: frame_yz(-UP, Vec3::Z, Vec3::Z) }
}

fn sanitize_calibration(c: Calibration, fallback: Calibration) -> Calibration {
    let pick = |v: f32, old: f32, [lo, hi]: [f32; 2]| if v.is_finite() { v.clamp(lo, hi) } else { old };
    Calibration {
        eye_height: pick(c.eye_height, fallback.eye_height, EYE_HEIGHT_RANGE),
        arm_span: pick(c.arm_span, fallback.arm_span, ARM_SPAN_RANGE),
    }
}
