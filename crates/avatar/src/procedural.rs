//! The built-in avatar, for games without a model: a low-poly mannequin (rounded torso pieces,
//! capsule limbs, ball joints) with articulated hands (a palm, three-segment fingers and a
//! thumb).
//!
//! It is an ordinary skinned model: a skeleton with Unity humanoid bone names, bound in the
//! protocol's reference T-pose, with every vertex rigidly on one bone. So it goes through exactly
//! the same mapping and retargeting as a game's model.

use glam::{Mat4, Quat, Vec3};
use rustvr_asset as asset;
use rustvr_protocol::{JOINT_COUNT, Joint};

use crate::pose::{reference_frame, reference_position};
use crate::{Source, SourceMesh};

const HEIGHT: f32 = 1.75;

const JOINT_NAMES: [&str; JOINT_COUNT] = [
    "Hips",
    "Spine",
    "Chest",
    "UpperChest",
    "Neck",
    "Head",
    "LeftShoulder",
    "LeftUpperArm",
    "LeftLowerArm",
    "LeftHand",
    "RightShoulder",
    "RightUpperArm",
    "RightLowerArm",
    "RightHand",
    "LeftUpperLeg",
    "LeftLowerLeg",
    "LeftFoot",
    "LeftToes",
    "RightUpperLeg",
    "RightLowerLeg",
    "RightFoot",
    "RightToes",
];
const FINGER_NAMES: [&str; 5] = ["Thumb", "Index", "Middle", "Ring", "Little"];
const SEGMENT_NAMES: [&str; 3] = ["Proximal", "Intermediate", "Distal"];

/// Knuckles in hand space (along the fingers, towards the thumb), metres. The middle knuckle is on
/// the hand's axis and the index and little knuckles are level with it, so the hand's canonical
/// frame is exactly the reference one.
const KNUCKLES: [[f32; 2]; 4] = [[0.094, 0.020], [0.098, 0.0], [0.094, -0.019], [0.087, -0.036]];
const FINGER_LENGTHS: [[f32; 3]; 5] = [
    [0.040, 0.031, 0.026],
    [0.042, 0.025, 0.021],
    [0.046, 0.029, 0.022],
    [0.043, 0.027, 0.021],
    [0.034, 0.021, 0.019],
];
const FINGER_RADII: [f32; 5] = [0.0112, 0.0088, 0.0091, 0.0086, 0.0076];

/// Linear colours: a cool grey body, slate joints, light hands (so they read in dark games).
const BODY: [f32; 3] = [0.30, 0.34, 0.41];
const JOINTS: [f32; 3] = [0.10, 0.11, 0.13];
const HANDS: [f32; 3] = [0.61, 0.56, 0.50];

pub fn mannequin() -> Source {
    let mut bones = Vec::new();
    for j in Joint::ALL {
        let parent = j.parent().map_or(-1, |p| p as i32);
        bones.push(bone(JOINT_NAMES[j as usize], parent, reference_position(j, HEIGHT), reference_frame(j)));
    }
    let mut body = Mesh::default();
    let mut joints = Mesh::default();
    let mut hands = Mesh::default();

    // Torso, neck and head.
    let at = |j: Joint| reference_position(j, HEIGHT);
    let b = |j: Joint| j as usize;
    // The torso in four overlapping rounded pieces (height, half width, half depth, centre z),
    // so it bends at the waist and chest without opening gaps.
    let torso = |rows: &[[f32; 4]]| rows.iter().map(|&[y, w, d, z]| (Vec3::new(0.0, y, z), Vec3::X * w, Vec3::Z * d)).collect::<Vec<_>>();
    body.loft(&torso(&[[0.855, 0.085, 0.070, 0.0], [0.885, 0.140, 0.098, -0.005], [0.94, 0.163, 0.110, -0.008], [1.00, 0.155, 0.104, -0.008], [1.05, 0.138, 0.094, -0.005]]), 2.6, b(Joint::Hips));
    body.loft(&torso(&[[1.03, 0.134, 0.092, -0.004], [1.08, 0.128, 0.089, 0.0], [1.13, 0.136, 0.094, 0.004], [1.17, 0.143, 0.098, 0.006]]), 2.6, b(Joint::Spine));
    body.loft(&torso(&[[1.15, 0.140, 0.097, 0.006], [1.20, 0.154, 0.108, 0.010], [1.26, 0.164, 0.115, 0.010], [1.32, 0.170, 0.112, 0.004]]), 2.6, b(Joint::Chest));
    body.loft(&torso(&[[1.30, 0.171, 0.111, 0.004], [1.36, 0.180, 0.106, -0.004], [1.405, 0.170, 0.094, -0.010], [1.44, 0.120, 0.074, -0.014], [1.458, 0.064, 0.056, -0.014]]), 2.6, b(Joint::UpperChest));
    body.capsule(Vec3::new(0.0, 1.43, -0.012), Vec3::new(0.0, 1.565, 0.0), 0.054, 0.050, b(Joint::Neck));
    body.ellipsoid(Vec3::new(0.0, 1.648, 0.012), [0.082, 0.112, 0.098], b(Joint::Head));
    // A nose, so the head shows which way it faces.
    body.ellipsoid(Vec3::new(0.0, 1.640, 0.103), [0.012, 0.020, 0.014], b(Joint::Head));

    for (side, x) in [(0usize, -1.0f32), (1, 1.0)] {
        let [shoulder, upper, lower, hand] = [
            [Joint::LeftShoulder, Joint::LeftUpperArm, Joint::LeftLowerArm, Joint::LeftHand],
            [Joint::RightShoulder, Joint::RightUpperArm, Joint::RightLowerArm, Joint::RightHand],
        ][side];
        let [thigh, calf, foot, toes] = [
            [Joint::LeftUpperLeg, Joint::LeftLowerLeg, Joint::LeftFoot, Joint::LeftToes],
            [Joint::RightUpperLeg, Joint::RightLowerLeg, Joint::RightFoot, Joint::RightToes],
        ][side];
        body.capsule(Vec3::new(x * 0.04, 1.41, 0.0), at(upper), 0.035, 0.035, b(shoulder));
        body.capsule(at(upper), at(lower), 0.050, 0.040, b(upper));
        body.capsule(at(lower), at(hand), 0.040, 0.029, b(lower));
        body.capsule(at(thigh), at(calf), 0.075, 0.054, b(thigh));
        body.capsule(at(calf), at(foot), 0.052, 0.036, b(calf));
        // Ball joints, a little proud of the limbs. (The hips' are inside the pelvis.)
        joints.sphere(at(upper), 0.057, b(upper));
        joints.sphere(at(lower), 0.046, b(lower));
        joints.sphere(at(calf), 0.061, b(calf));
        joints.sphere(at(foot), 0.043, b(foot));

        // Foot and toes: rounded sections along +Z (z, centre height, half width, half height).
        let section = |z: f32, y: f32, w: f32, h: f32| (Vec3::new(x * 0.09, y, z), Vec3::X * w, Vec3::Y * h);
        body.loft(
            &[
                section(-0.062, 0.045, 0.026, 0.030),
                section(-0.045, 0.050, 0.033, 0.046),
                section(-0.010, 0.055, 0.038, 0.052),
                section(0.040, 0.042, 0.044, 0.036),
                section(0.100, 0.028, 0.047, 0.024),
                section(0.136, 0.022, 0.046, 0.020),
            ],
            3.0,
            b(foot),
        );
        body.loft(
            &[section(0.130, 0.022, 0.045, 0.020), section(0.175, 0.018, 0.042, 0.016), section(0.198, 0.016, 0.034, 0.012), section(0.207, 0.016, 0.020, 0.008)],
            3.0,
            b(toes),
        );

        // The hand, in hand space: u along the fingers, t towards the thumb, d out of the back of
        // the hand (the palm faces -X on the left hand and +X on the right).
        let frame = reference_frame(hand);
        let wrist = at(hand);
        let (u, t) = (frame * Vec3::Y, frame * Vec3::Z);
        let d = if side == 0 { frame * Vec3::X } else { -(frame * Vec3::X) };
        let p = |pu: f32, pt: f32, pd: f32| wrist + u * pu + t * pt + d * pd;
        let palm = |pu: f32, ct: f32, cd: f32, wt: f32, wd: f32| (p(pu, ct, cd), t * wt, d * wd);
        hands.loft(
            &[
                palm(-0.014, -0.006, -0.002, 0.022, 0.015),
                palm(0.004, -0.006, -0.002, 0.027, 0.0165),
                palm(0.030, -0.007, -0.003, 0.034, 0.016),
                palm(0.060, -0.008, -0.003, 0.040, 0.0145),
                palm(0.086, -0.008, -0.002, 0.041, 0.012),
                palm(0.099, -0.008, -0.001, 0.035, 0.008),
            ],
            3.0,
            b(hand),
        );
        // The ball of the thumb.
        hands.ellipsoid_axes(p(0.032, 0.015, -0.008), [t * 0.017, u * 0.030, d * 0.012], b(hand));

        let side_name = ["Left", "Right"][side];
        for f in 0..5 {
            let (start, dir) = if f == 0 {
                (p(0.024, 0.022, -0.011), (u * 0.80 + t * 0.42 - d * 0.42).normalize())
            } else {
                let [ku, kt] = KNUCKLES[f - 1];
                (p(ku, kt, 0.0), u)
            };
            let rot = crate::pose::frame(dir, t);
            let mut pivot = start;
            let mut parent = b(hand) as i32;
            for k in 0..3 {
                let index = bones.len();
                bones.push(bone(&format!("{side_name}{}{}", FINGER_NAMES[f], SEGMENT_NAMES[k]), parent, pivot, rot));
                let len = FINGER_LENGTHS[f][k];
                let r = FINGER_RADII[f] * [1.0, 0.93, 0.86][k];
                let r_end = if k == 2 { r * 0.85 } else { r * 0.96 };
                // Segments end a little short of the next knuckle; the caps overlap there.
                hands.capsule(pivot, pivot + dir * (len - r_end * 0.4), r, r_end, index);
                pivot += dir * len;
                parent = index as i32;
            }
        }
    }

    let meshes = [("body", body, BODY), ("joints", joints, JOINTS), ("hands", hands, HANDS)]
        .into_iter()
        .map(|(name, m, tint)| SourceMesh { name: name.into(), vertices: m.vertices, indices: m.indices, texture: None, tint, material: None, depth_slack: 0.0 })
        .collect();
    Source { bones, meshes }
}

/// A bind-pose bone at `pos` with frame `rot` (the rig only reads the model-space matrix).
fn bone(name: &str, parent: i32, pos: Vec3, rot: Quat) -> asset::Bone {
    asset::Bone { name: name.into(), parent, local_pos: [0.0; 3], local_rot: [0.0, 0.0, 0.0, 1.0], model_matrix: Mat4::from_rotation_translation(rot, pos).to_cols_array() }
}

/// Closed shapes, each rigid on one bone, with smooth normals and outward (D3D front) faces.
#[derive(Default)]
struct Mesh {
    vertices: Vec<asset::Vertex>,
    indices: Vec<u32>,
}

impl Mesh {
    fn sphere(&mut self, c: Vec3, r: f32, bone: usize) {
        self.ellipsoid(c, [r, r, r], bone);
    }

    /// Axis-aligned ellipsoid with these half extents.
    fn ellipsoid(&mut self, c: Vec3, half: [f32; 3], bone: usize) {
        self.ellipsoid_axes(c, [Vec3::X * half[0], Vec3::Y * half[1], Vec3::Z * half[2]], bone);
    }

    /// Ellipsoid with half axes `a` (the poles are along `a[1]`).
    fn ellipsoid_axes(&mut self, c: Vec3, a: [Vec3; 3], bone: usize) {
        const RINGS: usize = 12;
        let rings: Vec<(Vec3, Vec3, Vec3)> = (1..RINGS)
            .map(|i| {
                let th = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / RINGS as f32;
                (c + a[1] * th.sin(), a[0] * th.cos(), a[2] * th.cos())
            })
            .collect();
        self.tube(&rings, 2.0, c - a[1], c + a[1], bone);
    }

    /// A capsule from `a` to `b` whose radius goes from `ra` to `rb`.
    fn capsule(&mut self, a: Vec3, b: Vec3, ra: f32, rb: f32, bone: usize) {
        const CAP: usize = 4;
        let axis = (b - a).normalize();
        let e1 = axis.any_orthonormal_vector();
        let e2 = axis.cross(e1);
        let mut rings = Vec::new();
        for i in 1..=CAP {
            let th = -std::f32::consts::FRAC_PI_2 * (1.0 - i as f32 / CAP as f32);
            rings.push((a + axis * (ra * th.sin()), e1 * (ra * th.cos()), e2 * (ra * th.cos())));
        }
        for i in 0..CAP {
            let th = std::f32::consts::FRAC_PI_2 * i as f32 / CAP as f32;
            rings.push((b + axis * (rb * th.sin()), e1 * (rb * th.cos()), e2 * (rb * th.cos())));
        }
        self.tube(&rings, 2.0, a - axis * ra, b + axis * rb, bone);
    }

    /// Rounded-rectangle sections (centre, half axis, half axis), flat-capped at both ends.
    fn loft(&mut self, sections: &[(Vec3, Vec3, Vec3)], squareness: f32, bone: usize) {
        let start = sections[0].0;
        let end = sections[sections.len() - 1].0;
        self.tube(sections, squareness, start, end, bone);
    }

    /// Rings of superellipses (exponent `p`: 2 is an ellipse) joined into a tube, closed by a fan
    /// to `start` and `end`.
    fn tube(&mut self, rings: &[(Vec3, Vec3, Vec3)], p: f32, start: Vec3, end: Vec3, bone: usize) {
        const AROUND: usize = 18;
        let first_index = self.indices.len();
        let base = self.vertices.len() as u32;
        let vertex = |pos: Vec3| asset::Vertex { pos: pos.to_array(), normal: [0.0; 3], uv: [0.0; 2], bones: [bone as u8, 0, 0, 0], weights: [255, 0, 0, 0], ao: 255 };
        let curve = |v: f32| v.signum() * v.abs().powf(2.0 / p);
        self.vertices.push(vertex(start));
        for &(c, a, b) in rings {
            for k in 0..AROUND {
                let phi = std::f32::consts::TAU * k as f32 / AROUND as f32;
                self.vertices.push(vertex(c + a * curve(phi.cos()) + b * curve(phi.sin())));
            }
        }
        self.vertices.push(vertex(end));
        let ring = |r: usize, k: usize| base + 1 + (r * AROUND + k % AROUND) as u32;
        let last = base + 1 + (rings.len() * AROUND) as u32;
        for k in 0..AROUND {
            self.indices.extend([base, ring(0, k), ring(0, k + 1)]);
            for r in 0..rings.len() - 1 {
                self.indices.extend([ring(r, k), ring(r + 1, k), ring(r + 1, k + 1), ring(r, k), ring(r + 1, k + 1), ring(r, k + 1)]);
            }
            self.indices.extend([ring(rings.len() - 1, k), last, ring(rings.len() - 1, k + 1)]);
        }
        // Wind outwards: a closed surface whose faces point out has a positive signed volume.
        let tris = &mut self.indices[first_index..];
        let pos = |i: u32| Vec3::from_array(self.vertices[i as usize].pos) - start;
        let volume: f32 = tris.chunks(3).map(|t| pos(t[0]).dot(pos(t[1]).cross(pos(t[2])))).sum();
        if volume < 0.0 {
            for t in tris.chunks_mut(3) {
                t.swap(1, 2);
            }
        }
        let mut normals = vec![Vec3::ZERO; self.vertices.len() - base as usize];
        for t in self.indices[first_index..].chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from_array(self.vertices[i as usize].pos));
            let n = (b - a).cross(c - a);
            for &i in t {
                normals[(i - base) as usize] += n;
            }
        }
        for (v, n) in self.vertices[base as usize..].iter_mut().zip(normals) {
            v.normal = n.normalize_or(Vec3::Y).to_array();
        }
    }
}
