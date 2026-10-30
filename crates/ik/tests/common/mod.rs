//! Shared helpers for the body tests: poses for a standing player, frame maths in the protocol's
//! convention, and a stick-figure PNG dump (front and side views) for looking at the results.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::PathBuf;

use glam::{Mat3, Quat, Vec3};
use rustvr_ik::{BodyOutput, BodyParams, BodySolver, Calibration, Pose, Side, TrackedInput};
use rustvr_protocol::Joint;

pub const DT: f32 = 1.0 / 90.0;
pub const EYE: f32 = 1.70;

pub fn deg(d: f32) -> f32 {
    d.to_radians()
}

/// Head at standing eye height, turned by `yaw` and pitched down by `pitch`.
pub fn head(pos: Vec3, yaw: f32, pitch: f32) -> Pose {
    Pose { pos, rot: Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) }
}

pub fn standing_head() -> Pose {
    head(Vec3::new(0.0, EYE, 0.0), 0.0, 0.0)
}

/// Eyes to the default skeleton's neck pivot, in head space (0.13 m, Parger).
pub fn eye_to_neck() -> Vec3 {
    Vec3::new(0.0, -0.075, -0.0805).normalize() * 0.13
}

/// A head turned about the standing neck pivot, the way a real head turns (the eyes swing).
pub fn head_about_neck(yaw: f32, pitch: f32) -> Pose {
    let pivot = Vec3::new(0.0, EYE, 0.0) + eye_to_neck();
    let rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
    Pose { pos: pivot - rot * eye_to_neck(), rot }
}

/// Handle angle of a pistol grip above the barrel: a real pistol's grip sits about 20° off
/// vertical, which keeps the wrist straight with the barrel along the forearm.
pub const PISTOL: f32 = 65.0;
/// A vertical foregrip.
pub const FOREGRIP: f32 = 85.0;

/// Rotation pointing +Z along `dir` without roll.
pub fn aim(dir: Vec3) -> Quat {
    let d = dir.normalize();
    Quat::from_rotation_y(d.x.atan2(d.z)) * Quat::from_rotation_x(-d.y.clamp(-1.0, 1.0).asin())
}

/// A controller held like a gun aimed along `dir`: the grip's +Z (along the handle) leans up
/// from the barrel by `handle` degrees.
pub fn gun_grip(pos: Vec3, dir: Vec3, handle: f32) -> Pose {
    Pose { pos, rot: aim(dir) * Quat::from_rotation_x(-deg(handle)) }
}

/// A fist hanging at the side, the controller pointing forward.
pub fn hanging_grip(side: Side) -> Pose {
    let x = if side == Side::Left { -0.22 } else { 0.22 };
    Pose { pos: Vec3::new(x, 0.76, -0.03), rot: Quat::IDENTITY }
}

/// The named poses the gallery draws and the gun-pose tests check.
pub fn named_pose(name: &str) -> TrackedInput {
    let h = standing_head();
    let fwd = Vec3::Z;
    let hang = |side| Some(hanging_grip(side));
    match name {
        "neutral" => input(h, hang(Side::Left), hang(Side::Right)),
        "hip_fire" => input(h, hang(Side::Left), Some(gun_grip(Vec3::new(0.2, 1.05, 0.38), fwd, PISTOL))),
        "two_hand_hip" => input(
            h,
            Some(gun_grip(Vec3::new(-0.05, 1.08, 0.42), fwd, FOREGRIP)),
            Some(gun_grip(Vec3::new(0.15, 1.02, 0.2), fwd, PISTOL)),
        ),
        "ads_pistol" => input(
            h,
            Some(gun_grip(Vec3::new(-0.01, 1.56, 0.5), fwd, PISTOL)),
            Some(gun_grip(Vec3::new(0.02, 1.58, 0.52), fwd, PISTOL)),
        ),
        "rifle" => input(
            h,
            Some(gun_grip(Vec3::new(-0.03, 1.47, 0.62), fwd, FOREGRIP)),
            Some(gun_grip(Vec3::new(0.1, 1.5, 0.24), fwd, PISTOL)),
        ),
        "reach_shoulder" => input(
            h,
            hang(Side::Left),
            Some(gun_grip(Vec3::new(0.16, 1.6, -0.06), Vec3::new(0.0, 1.0, -0.3), 0.0)),
        ),
        "reach_hip" => input(h, hang(Side::Left), Some(gun_grip(Vec3::new(0.24, 0.95, -0.02), Vec3::new(0.0, -0.3, 1.0), 70.0))),
        "crouch" => {
            let low = |side| {
                let g = hanging_grip(side);
                Some(Pose { pos: g.pos + Vec3::new(0.0, -0.5, 0.25), rot: g.rot })
            };
            input(head(Vec3::new(0.0, 1.2, 0.0), 0.0, 0.0), low(Side::Left), low(Side::Right))
        }
        "bend_over" => input(
            head(Vec3::new(0.0, 1.15, 0.35), 0.0, deg(70.0)),
            Some(Pose { pos: Vec3::new(-0.15, 0.55, 0.45), rot: Quat::IDENTITY }),
            Some(Pose { pos: Vec3::new(0.15, 0.55, 0.45), rot: Quat::IDENTITY }),
        ),
        "no_hands" => input(h, None, None),
        "seated" => {
            let mut i = input(head(Vec3::new(0.0, 1.3, 0.0), 0.0, 0.0), None, None);
            i.seated = true;
            i
        }
        _ => panic!("no pose {name}"),
    }
}

pub const POSES: [&str; 11] = [
    "neutral",
    "hip_fire",
    "two_hand_hip",
    "ads_pistol",
    "rifle",
    "reach_shoulder",
    "reach_hip",
    "crouch",
    "bend_over",
    "no_hands",
    "seated",
];

pub fn input(head: Pose, left: Option<Pose>, right: Option<Pose>) -> TrackedInput {
    TrackedInput { dt: DT, head, grips: [left, right], locomotion: Vec3::ZERO, seated: false, elbows: [None; 2], swim: None }
}

pub fn solver() -> BodySolver {
    BodySolver::new(BodyParams::default(), Calibration::default())
}

/// Runs `frames` frames of the same input and returns the last output.
pub fn settle(s: &mut BodySolver, i: &TrackedInput, frames: usize) -> BodyOutput {
    let mut out = s.solve(i);
    for _ in 1..frames {
        out = s.solve(i);
    }
    out
}

pub fn pos(o: &BodyOutput, j: Joint) -> Vec3 {
    o.joints[j as usize].pos
}

/// A joint's canonical frame in the reference T-pose facing +Z, as `rustvr_avatar::pose::
/// reference_frame` encodes it (absolute orientations in tracking space).
pub fn reference_frame(j: Joint) -> Quat {
    use Joint::*;
    let m = match j {
        Hips | Spine | Chest | UpperChest | Neck | Head => Mat3::IDENTITY,
        LeftShoulder | LeftUpperArm | LeftLowerArm | LeftHand => Mat3::from_cols(Vec3::Y, -Vec3::X, Vec3::Z),
        RightShoulder | RightUpperArm | RightLowerArm | RightHand => Mat3::from_cols(-Vec3::Y, Vec3::X, Vec3::Z),
        LeftUpperLeg | LeftLowerLeg | RightUpperLeg | RightLowerLeg => Mat3::from_cols(-Vec3::X, -Vec3::Y, Vec3::Z),
        LeftFoot | LeftToes | RightFoot | RightToes => Mat3::from_cols(-Vec3::X, Vec3::Z, Vec3::Y),
    };
    Quat::from_mat3(&m)
}

/// A joint's canonical axes in tracking space: (X, Y, Z).
pub fn axes(o: &BodyOutput, j: Joint) -> (Vec3, Vec3, Vec3) {
    let q = o.joints[j as usize].rot;
    (q * Vec3::X, q * Vec3::Y, q * Vec3::Z)
}

pub fn yaw_deg(v: Vec3) -> f32 {
    v.x.atan2(v.z).to_degrees()
}

/// Forward of the upper chest, as a yaw in degrees.
pub fn chest_yaw(o: &BodyOutput) -> f32 {
    yaw_deg(axes(o, Joint::UpperChest).2)
}

/// Angle of the hips -> neck line from vertical, degrees.
pub fn spine_lean(o: &BodyOutput) -> f32 {
    (pos(o, Joint::Neck) - pos(o, Joint::Hips)).angle_between(Vec3::Y).to_degrees()
}

/// Elbow flexion from the bone frames (0 = straight), degrees.
pub fn elbow_flexion(o: &BodyOutput, side: Side) -> f32 {
    let (upper, lower) = match side {
        Side::Left => (Joint::LeftUpperArm, Joint::LeftLowerArm),
        Side::Right => (Joint::RightUpperArm, Joint::RightLowerArm),
    };
    axes(o, upper).1.angle_between(axes(o, lower).1).to_degrees()
}

pub fn knee_flexion(o: &BodyOutput, side: Side) -> f32 {
    let (hip, knee, ankle) = match side {
        Side::Left => (Joint::LeftUpperLeg, Joint::LeftLowerLeg, Joint::LeftFoot),
        Side::Right => (Joint::RightUpperLeg, Joint::RightLowerLeg, Joint::RightFoot),
    };
    (pos(o, knee) - pos(o, hip)).angle_between(pos(o, ankle) - pos(o, knee)).to_degrees()
}

pub fn all_finite(o: &BodyOutput) -> bool {
    o.joints.iter().chain(o.hand_targets.iter()).all(|j| j.pos.is_finite() && j.rot.is_finite())
        && o.scale.is_finite()
        && o.arm_length.is_finite()
}

/// The lowest point of each foot (sole under the ankle or under the ball) above the floor, given
/// the default skeleton's 8 cm ankle and 2 cm ball heights scaled by `o.scale`.
pub fn sole_heights(o: &BodyOutput) -> [f32; 2] {
    let s = o.scale;
    let sole = |foot: Joint, toes: Joint| (pos(o, foot).y - 0.08 * s).min(pos(o, toes).y - 0.02 * s);
    [sole(Joint::LeftFoot, Joint::LeftToes), sole(Joint::RightFoot, Joint::RightToes)]
}

// ---- stick-figure PNGs ----------------------------------------------------------------------

const VIEW: usize = 420;
const PX_PER_M: f32 = 180.0;

struct Canvas {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Canvas { w, h, rgb: vec![255; w * h * 3] }
    }

    fn dot(&mut self, x: i32, y: i32, c: [u8; 3], r: i32) {
        for dy in -r..=r {
            for dx in -r..=r {
                let (px, py) = (x + dx, y + dy);
                if px >= 0 && py >= 0 && (px as usize) < self.w && (py as usize) < self.h {
                    let i = (py as usize * self.w + px as usize) * 3;
                    self.rgb[i..i + 3].copy_from_slice(&c);
                }
            }
        }
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 3], r: i32) {
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as i32).max(1);
        for k in 0..=n {
            let t = k as f32 / n as f32;
            self.dot((a.0 + (b.0 - a.0) * t) as i32, (a.1 + (b.1 - a.1) * t) as i32, c, r);
        }
    }
}

fn colour(j: Joint) -> [u8; 3] {
    use Joint::*;
    match j {
        LeftShoulder | LeftUpperArm | LeftLowerArm | LeftHand | LeftUpperLeg | LeftLowerLeg | LeftFoot | LeftToes => {
            [40, 90, 220]
        }
        RightShoulder | RightUpperArm | RightLowerArm | RightHand | RightUpperLeg | RightLowerLeg | RightFoot
        | RightToes => [220, 50, 40],
        _ => [30, 30, 30],
    }
}

/// Writes `captures/ik/<name>.png`: front view (+X to the right) on the left, side view (the
/// player's forward, +Z, to the right) on the right, both centred on the hips. Grid
/// lines every 0.5 m; the floor is the bottom grey line; green crosses are the hand targets and
/// the purple dot is the HMD.
pub fn dump(name: &str, o: &BodyOutput, eyes: Vec3) {
    let mut c = Canvas::new(VIEW * 2, VIEW);
    let centre = pos(o, Joint::Hips);
    let floor_y = VIEW as f32 - 20.0;
    let project = |p: Vec3, view: usize| -> (f32, f32) {
        let h = if view == 0 { p.x - centre.x } else { p.z - centre.z };
        (view as f32 * VIEW as f32 + VIEW as f32 * 0.5 + h * PX_PER_M, floor_y - p.y * PX_PER_M)
    };
    for view in 0..2 {
        for k in -4..=4 {
            let x = view as f32 * VIEW as f32 + VIEW as f32 * 0.5 + k as f32 * 0.5 * PX_PER_M;
            c.line((x, 0.0), (x, floor_y), [225, 225, 225], 0);
        }
        for k in 0..5 {
            let y = floor_y - k as f32 * 0.5 * PX_PER_M;
            let x0 = view as f32 * VIEW as f32;
            c.line((x0, y), (x0 + VIEW as f32 - 1.0, y), if k == 0 { [150, 150, 150] } else { [225, 225, 225] }, 0);
        }
        for j in Joint::ALL {
            if let Some(parent) = j.parent() {
                c.line(project(pos(o, parent), view), project(pos(o, j), view), colour(j), 1);
            }
        }
        for j in Joint::ALL {
            let (x, y) = project(pos(o, j), view);
            c.dot(x as i32, y as i32, [0, 0, 0], 2);
        }
        for t in o.hand_targets {
            let (x, y) = project(t.pos, view);
            c.line((x - 6.0, y - 6.0), (x + 6.0, y + 6.0), [20, 170, 60], 0);
            c.line((x - 6.0, y + 6.0), (x + 6.0, y - 6.0), [20, 170, 60], 0);
        }
        let (x, y) = project(eyes, view);
        c.dot(x as i32, y as i32, [150, 40, 170], 3);
    }
    c.line((VIEW as f32, 0.0), (VIEW as f32, VIEW as f32 - 1.0), [0, 0, 0], 0);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../captures/ik");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), c.w as u32, c.h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&c.rgb).unwrap();
}
