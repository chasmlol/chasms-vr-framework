//! Stick figures of the key poses, written to `captures/ik/` for looking at (front and side
//! views). The checks here are only that each pose solves; the behaviour is tested in `body.rs`.

mod common;

use common::*;
use glam::Vec3;
use rustvr_ik::Side;

#[test]
fn key_poses() {
    for name in POSES {
        let i = named_pose(name);
        let o = settle(&mut solver(), &i, 180);
        assert!(all_finite(&o), "{name}");
        dump(name, &o, i.head.pos);
    }
}

#[test]
fn walking_frames() {
    let mut s = solver();
    let mut i = input(standing_head(), Some(hanging_grip(Side::Left)), Some(hanging_grip(Side::Right)));
    i.locomotion = Vec3::new(0.0, 0.0, 1.4);
    settle(&mut s, &i, 90);
    for k in 0..6 {
        let o = settle(&mut s, &i, 8);
        dump(&format!("walk_{k}"), &o, i.head.pos);
    }
}
