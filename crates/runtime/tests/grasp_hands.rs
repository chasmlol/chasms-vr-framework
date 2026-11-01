//! The drawn fingers close round the handles the rig puts a hand on (a sword's grip, a gun's pistol
//! grip, a handguard), measured on the mannequin.

use glam::{Mat3, Quat, Vec3};
use rustvr_avatar::grasp::ALL_FINGERS;
use rustvr_avatar::{Grasp, Solid};
use rustvr_ik::{BodyParams, Pose as IkPose, Side};
use rustvr_protocol::Pose;

/// The hand joint (canonical frame) for a controller grip pose, as the body solver puts it.
fn hand_for(side: Side, grip: (Vec3, Quat)) -> Pose {
    let h = BodyParams::default().hand_offsets.hand_frame(side, IkPose { pos: grip.0, rot: grip.1 });
    Pose { pos: h.pos.to_array(), valid: 1, rot: h.rot.to_array() }
}

/// A fist round a tube along `tube`, the back of the hand towards `back`.
fn fist(tube: Vec3, back: Vec3) -> Quat {
    let z = tube.normalize();
    let x = (back - z * back.dot(z)).normalize();
    Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)).normalize()
}

fn models() -> Vec<(String, Option<Vec<rustvr_asset::Bone>>)> {
    vec![("mannequin".to_string(), None)]
}

#[test]
fn fingers_close_round_a_handle_the_hand_is_on() {
    let joints = rustvr_avatar::pose::t_pose(1.75);
    // The right hand round a handle along its fist, 1.4 cm and 1.6 cm thick (a sword's grip, a
    // pistol grip), held out in front.
    let at = Vec3::new(0.25, 1.2, 0.4);
    let rot = fist(Vec3::Z, Vec3::X);
    for radius in [0.014, 0.016] {
        let z = rot * Vec3::Z * 0.05;
        let g = Grasp { solids: vec![Solid::Capsule { a: at - z, b: at + z, radius }], fingers: ALL_FINGERS, weight: 1.0, ..Default::default() };
        for (name, bones) in models() {
            let curls = rustvr_avatar::grasp_curls(bones.as_deref(), &joints, 1, hand_for(Side::Right, (at, rot)), &g).unwrap();
            eprintln!("{name}, a {:.1} cm handle: {:?}", radius * 200.0, curls.map(|c| c.map(|v| (v * 100.0).round() / 100.0)));
            for f in 1..5 {
                let mean = curls[f].iter().sum::<f32>() / 3.0;
                assert!(mean > 0.35, "{name}: finger {f} closes round it: {:?}", curls[f]);
            }
        }
    }
}
