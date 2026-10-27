//! Behaviour of the full-body solver through its public API.

mod common;

use common::*;
use glam::{Quat, Vec3};
use rustvr_ik::{BodyOutput, BodyParams, BodySolver, Calibration, HandOffsets, Pose, Side, TrackedInput};
use rustvr_protocol::Joint;

/// Angle between two rotations, accurate for tiny angles (unlike acos of the dot product).
fn angle_deg(a: Quat, b: Quat) -> f32 {
    let d = a.inverse() * b;
    (2.0 * Vec3::new(d.x, d.y, d.z).length().min(1.0).asin()).to_degrees()
}

fn sides() -> [(Side, Joint, Joint, Joint, Joint); 2] {
    [
        (Side::Left, Joint::LeftShoulder, Joint::LeftUpperArm, Joint::LeftLowerArm, Joint::LeftHand),
        (Side::Right, Joint::RightShoulder, Joint::RightUpperArm, Joint::RightLowerArm, Joint::RightHand),
    ]
}

/// The hand joint must be exactly `HandOffsets::hand_frame` of the grip.
fn assert_hands_pinned(o: &BodyOutput, i: &TrackedInput, what: &str) {
    let offsets = HandOffsets::default();
    for (side, _, _, _, hand) in sides() {
        let Some(grip) = i.grips[side as usize] else { continue };
        let want = offsets.hand_frame(side, grip);
        let got = o.joints[hand as usize];
        assert!(got.pos.distance(want.pos) < 1e-3, "{what} {side:?}: hand {} from target", got.pos.distance(want.pos));
        assert!(angle_deg(got.rot, want.rot) < 0.05, "{what} {side:?}: hand rotation off");
        assert_eq!(o.hand_targets[side as usize], got);
    }
}

fn assert_on_floor(o: &BodyOutput, what: &str) {
    for h in sole_heights(o) {
        assert!(h > -1e-3, "{what}: a foot is {h} m below the floor");
    }
}

// ---- standing --------------------------------------------------------------------------------

#[test]
fn standing_neutral_is_plausible() {
    let i = named_pose("neutral");
    let o = settle(&mut solver(), &i, 90);
    assert!(all_finite(&o));
    assert!(chest_yaw(&o).abs() < 3.0, "chest yaw {}", chest_yaw(&o));
    assert!(spine_lean(&o) < 3.0, "lean {}", spine_lean(&o));
    assert!(angle_deg(o.joints[Joint::Hips as usize].rot, Quat::IDENTITY) < 5.0);
    for (side, _, upper, _, _) in sides() {
        // Arms hang: upper arms point down, elbows a little bent, both hands reached.
        assert!(axes(&o, upper).1.dot(Vec3::NEG_Y) > 0.9, "{side:?} upper arm");
        assert!(elbow_flexion(&o, side) < 45.0, "{side:?} elbow {}", elbow_flexion(&o, side));
        assert!(knee_flexion(&o, side) < 12.0, "{side:?} knee {}", knee_flexion(&o, side));
        assert!(o.reached[side as usize]);
    }
    // Feet 0.3 m apart under the hips, flat on the floor.
    let (l, r) = (pos(&o, Joint::LeftFoot), pos(&o, Joint::RightFoot));
    assert!((l.distance(r) - 0.3).abs() < 0.03, "feet {} apart", l.distance(r));
    let mid = (l + r) * 0.5;
    let hips = pos(&o, Joint::Hips);
    assert!(Vec3::new(mid.x - hips.x, 0.0, mid.z - hips.z).length() < 0.1);
    for h in sole_heights(&o) {
        assert!(h.abs() < 1e-3, "sole {h}");
    }
    assert!((o.scale - 1.0).abs() < 1e-6);
    assert_hands_pinned(&o, &i, "neutral");
}

#[test]
fn tpose_gives_the_reference_frames() {
    let mut s = solver();
    let first = settle(&mut s, &named_pose("neutral"), 10);
    let offsets = HandOffsets::default();
    // Grips placed so the hand joints are exactly the reference frames, arms straight out.
    let grip = |side: Side, hand: Joint, shoulder: Joint| {
        let out = if side == Side::Left { -1.0 } else { 1.0 };
        let wrist = pos(&first, shoulder) + Vec3::X * out * first.arm_length * 0.99;
        let local = offsets.grip_to_hand[side as usize];
        let rot = reference_frame(hand) * local.rot.inverse();
        Pose { pos: wrist - rot * local.pos, rot }
    };
    let i = input(
        standing_head(),
        Some(grip(Side::Left, Joint::LeftHand, Joint::LeftUpperArm)),
        Some(grip(Side::Right, Joint::RightHand, Joint::RightUpperArm)),
    );
    let o = settle(&mut s, &i, 120);
    for j in Joint::ALL {
        let err = angle_deg(o.joints[j as usize].rot, reference_frame(j));
        // Elbows keep 10° of bend at full reach and knees a few degrees of slack, so the bones
        // either side of them lean by about half that.
        let limit = match j {
            Joint::LeftHand | Joint::RightHand => 0.05,
            Joint::LeftUpperArm | Joint::LeftLowerArm | Joint::RightUpperArm | Joint::RightLowerArm => 10.0,
            Joint::LeftShoulder | Joint::RightShoulder => 6.0,
            Joint::LeftUpperLeg | Joint::LeftLowerLeg | Joint::RightUpperLeg | Joint::RightLowerLeg => 6.0,
            _ => 2.0,
        };
        assert!(err < limit, "{j:?}: {err:.2}° from the reference frame");
    }
    assert!(o.reached[0] && o.reached[1]);
    assert_hands_pinned(&o, &i, "T-pose");
    dump("tpose", &o, i.head.pos);
}

#[test]
fn hands_land_exactly_on_their_targets() {
    let mut s = solver();
    // Deterministic wandering hands, mostly within reach, with spinning wrists.
    let mut seed = 0x1234_5678u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let mut reached = 0;
    for frame in 0..900 {
        let t = frame as f32 * DT;
        let mut grips = [None; 2];
        for (k, g) in grips.iter_mut().enumerate() {
            let side = if k == 0 { -1.0 } else { 1.0 };
            let p = Vec3::new(side * (0.25 + 0.2 * (t * 1.3 + k as f32).sin()), 1.15 + 0.35 * (t * 0.9 + k as f32).cos(), 0.25 + 0.25 * (t * 1.1).sin());
            let rot = Quat::from_euler(glam::EulerRot::YXZ, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0, rnd() * 3.0 - 1.5);
            *g = Some(Pose { pos: p, rot });
        }
        let i = TrackedInput { grips, ..input(head_about_neck((t * 0.7).sin(), 0.3 * t.cos()), None, None) };
        let o = s.solve(&i);
        assert!(all_finite(&o));
        // The rule: the hand is on the controller whether or not the arm reached.
        assert_hands_pinned(&o, &i, &format!("frame {frame}"));
        reached += o.reached.iter().filter(|r| **r).count();
    }
    assert!(reached > 1200, "most of these targets are reachable ({reached})");
}

#[test]
fn head_joint_follows_the_hmd() {
    let mut s = solver();
    let mut offset = None;
    for k in 0..40 {
        let k = k as f32;
        let h = Pose {
            pos: Vec3::new(0.1 * k.sin(), 1.5 + 0.2 * k.cos(), 0.05 * k),
            rot: Quat::from_euler(glam::EulerRot::YXZ, k * 0.4, (k * 0.7).sin(), (k * 1.3).sin() * 0.4),
        };
        let o = s.solve(&input(h, Some(hanging_grip(Side::Left)), None));
        let head = o.joints[Joint::Head as usize];
        assert!(angle_deg(head.rot, h.rot) < 0.01, "head rotation is the HMD's");
        // The head joint is the neck pivot, rigidly attached to the HMD 0.13 m away.
        let local = h.rot.inverse() * (head.pos - h.pos);
        assert!((local.length() - 0.13).abs() < 1e-4);
        if let Some(first) = offset {
            assert!(local.distance(first) < 1e-4);
        }
        offset = Some(local);
    }
}

#[test]
fn output_converts_to_the_protocol() {
    let o = settle(&mut solver(), &named_pose("rifle"), 30);
    let p = o.to_protocol();
    for (a, b) in o.joints.iter().zip(p.iter()) {
        assert_eq!(a.pos.to_array(), b.pos);
        assert_eq!(a.rot.to_array(), b.rot);
        assert_eq!(b.valid, 1);
    }
}

// ---- torso and spine ---------------------------------------------------------------------------

/// Runs `seconds` of input from `f(t)` (t from 0 to 1 over the run) and returns every output.
fn run(s: &mut BodySolver, seconds: f32, f: impl Fn(f32) -> TrackedInput) -> Vec<BodyOutput> {
    let frames = (seconds / DT).round() as usize;
    (0..frames).map(|k| s.solve(&f(k as f32 / (frames - 1).max(1) as f32))).collect()
}

#[test]
fn turning_the_head_with_the_hands_still_barely_turns_the_torso() {
    for hands in ["neutral", "two_hand_hip"] {
        for turn in [60.0, -60.0] {
            let still = named_pose(hands);
            let mut s = solver();
            let before = chest_yaw(&settle(&mut s, &still, 90));
            let look = |t: f32| TrackedInput { head: head_about_neck(deg(turn) * t.min(1.0), 0.0), ..still };
            run(&mut s, 0.5, look);
            let o = settle(&mut s, &look(1.0), 90);
            let yaw = chest_yaw(&o) - before;
            // FRIK's 0.7 blend leaves the torso about a third of a head turn, a little more when
            // the hands already turn it.
            assert!(yaw.abs() < 0.45 * 60.0, "{hands}, head {turn}°: chest turned {yaw:.1}°");
            assert!(yaw * turn >= 0.0, "a little towards the head if at all");
            assert!(yaw_deg(axes(&o, Joint::Hips).2).abs() < 10.0, "hips stay put");
            assert_hands_pinned(&o, &look(1.0), hands);
        }
    }
}

/// `pose` turned about the vertical through the eyes.
fn turned(pose: &TrackedInput, yaw: f32) -> TrackedInput {
    let q = Quat::from_rotation_y(yaw);
    let eyes = pose.head.pos;
    let turn = |p: Pose| Pose { pos: eyes + q * (p.pos - eyes), rot: q * p.rot };
    TrackedInput { head: turn(pose.head), grips: pose.grips.map(|g| g.map(turn)), ..*pose }
}

#[test]
fn turning_the_whole_body_turns_the_torso_and_feet() {
    for name in ["neutral", "two_hand_hip", "rifle"] {
        let pose = named_pose(name);
        let mut s = solver();
        settle(&mut s, &pose, 60);
        run(&mut s, 1.0, |t| turned(&pose, deg(90.0) * t));
        let o = settle(&mut s, &turned(&pose, deg(90.0)), 120);
        let chest = chest_yaw(&o);
        assert!((chest - 90.0).abs() < 15.0, "{name}: chest at {chest:.1}°");
        let hips = yaw_deg(axes(&o, Joint::Hips).2);
        assert!(hips > 55.0, "{name}: hips at {hips:.1}°");
        for foot in [Joint::LeftFoot, Joint::RightFoot] {
            let yaw = yaw_deg(axes(&o, foot).1);
            assert!(yaw > 40.0, "{name}: {foot:?} still faces {yaw:.1}°");
        }
        assert_on_floor(&o, name);
    }
}

#[test]
fn hands_behind_the_back_dont_flip_the_torso() {
    let mut s = solver();
    let front = named_pose("neutral");
    settle(&mut s, &front, 60);
    let behind = |t: f32| {
        // Round the hips to behind the back.
        let a = deg(100.0) * t;
        let grip = |side: f32| {
            let p = Vec3::new(side * 0.24 * a.cos().max(0.4), 0.9, -0.03 - 0.25 * a.sin());
            Some(Pose { pos: p, rot: Quat::from_rotation_y(-side * a) })
        };
        input(standing_head(), grip(-1.0), grip(1.0))
    };
    let mut outs = run(&mut s, 1.0, behind);
    outs.extend(run(&mut s, 1.0, |_| behind(1.0)));
    outs.extend(run(&mut s, 1.0, |t| behind(1.0 - t)));
    for (k, o) in outs.iter().enumerate() {
        let yaw = chest_yaw(o);
        assert!(yaw.abs() < 30.0, "frame {k}: chest yaw {yaw:.1}°");
    }
}

#[test]
fn crouching_with_a_level_gaze_bends_the_knees() {
    let mut s = solver();
    let standing = settle(&mut s, &named_pose("neutral"), 60);
    let crouch = named_pose("crouch");
    let outs = run(&mut s, 1.0, |t| {
        let stand = named_pose("neutral");
        TrackedInput {
            head: Pose { pos: stand.head.pos.lerp(crouch.head.pos, t), rot: Quat::IDENTITY },
            grips: [0, 1].map(|k| Some(Pose { pos: stand.grips[k].unwrap().pos.lerp(crouch.grips[k].unwrap().pos, t), rot: Quat::IDENTITY })),
            ..stand
        }
    });
    let o = settle(&mut s, &crouch, 60);
    assert!(spine_lean(&o) < 20.0, "spine leans {:.1}°", spine_lean(&o));
    for side in [Side::Left, Side::Right] {
        assert!(knee_flexion(&o, side) > 50.0, "{side:?} knee {:.1}°", knee_flexion(&o, side));
    }
    assert!(pos(&o, Joint::Hips).y < pos(&standing, Joint::Hips).y - 0.35);
    let (hips, neck) = (pos(&o, Joint::Hips), pos(&o, Joint::Neck));
    assert!((hips.z - neck.z).abs() < 0.15, "hips stay under the head");
    for h in sole_heights(&o) {
        assert!(h.abs() < 5e-3, "feet planted ({h})");
    }
    for o in &outs {
        assert_on_floor(o, "crouching");
    }
    assert_hands_pinned(&o, &crouch, "crouch");
}

#[test]
fn bending_over_pitches_the_spine_and_pushes_the_hips_back() {
    let mut s = solver();
    let standing = settle(&mut s, &named_pose("neutral"), 60);
    let bend = named_pose("bend_over");
    run(&mut s, 1.0, |t| {
        let stand = named_pose("neutral");
        TrackedInput { head: Pose { pos: stand.head.pos.lerp(bend.head.pos, t), rot: Quat::IDENTITY.slerp(bend.head.rot, t) }, ..bend }
    });
    let o = settle(&mut s, &bend, 90);
    assert!(spine_lean(&o) > 40.0, "spine leans only {:.1}°", spine_lean(&o));
    let hips = pos(&o, Joint::Hips);
    assert!(hips.z < pos(&standing, Joint::Hips).z - 0.1, "hips moved back to {:.2}", hips.z);
    assert!(hips.z < pos(&o, Joint::Head).z - 0.3);
    assert_on_floor(&o, "bent over");
    assert_hands_pinned(&o, &bend, "bend");
}

#[test]
fn head_too_high_keeps_the_feet_planted() {
    let mut s = solver();
    let stand = named_pose("neutral");
    let before = settle(&mut s, &stand, 120);
    let tall = TrackedInput { head: Pose { pos: stand.head.pos + Vec3::Y * 0.15, ..stand.head }, ..stand };
    let o = settle(&mut s, &tall, 60);
    for (foot, toes) in [(Joint::LeftFoot, Joint::LeftToes), (Joint::RightFoot, Joint::RightToes)] {
        // On tiptoe at most: the ball of the foot stays on the floor where it was.
        assert!((pos(&o, toes).y - 0.02).abs() < 2e-3, "{toes:?} at {}", pos(&o, toes).y);
        let moved = pos(&o, toes) - pos(&before, toes);
        assert!(Vec3::new(moved.x, 0.0, moved.z).length() < 0.02, "{foot:?} slid");
        assert!(pos(&o, foot).y >= pos(&before, foot).y - 1e-4);
    }
    assert_on_floor(&o, "tall");
    // The legs still connect hip to foot.
    let thigh = pos(&before, Joint::LeftUpperLeg).distance(pos(&before, Joint::LeftLowerLeg));
    assert!((pos(&o, Joint::LeftUpperLeg).distance(pos(&o, Joint::LeftLowerLeg)) - thigh).abs() < thigh * 0.03);
}

// ---- arms ------------------------------------------------------------------------------------

/// Where the elbow sits relative to the shoulder -> wrist line (perpendicular offset).
fn elbow_offset(o: &BodyOutput, upper: Joint, lower: Joint, hand: Joint) -> Vec3 {
    let (s, e, w) = (pos(o, upper), pos(o, lower), pos(o, hand));
    let axis = (w - s).normalize();
    e - (s + axis * (e - s).dot(axis))
}

/// Elbow inside the ribcage (a box around the upper chest)?
fn elbow_in_torso(o: &BodyOutput, lower: Joint) -> bool {
    let chest = o.joints[Joint::UpperChest as usize];
    let l = chest.rot.inverse() * (pos(o, lower) - chest.pos);
    l.x.abs() < 0.12 && l.z > -0.11 && l.z < 0.1 && l.y > -0.55 && l.y < 0.08
}

#[test]
fn elbows_point_out_and_down_for_gun_poses() {
    // (pose, arms to check, how far up the elbow may point as a fraction of its offset)
    let cases: [(&str, &[Side], f32); 6] = [
        ("hip_fire", &[Side::Right], 0.0),
        ("two_hand_hip", &[Side::Left, Side::Right], 0.0),
        ("ads_pistol", &[Side::Left, Side::Right], 0.5),
        ("rifle", &[Side::Left], 0.0),
        ("rifle", &[Side::Right], 0.6),
        ("reach_hip", &[Side::Right], 0.3),
    ];
    for (name, arms, max_up) in cases {
        let i = named_pose(name);
        let o = settle(&mut solver(), &i, 120);
        for &side in arms {
            let (_, _, upper, lower, hand) = sides()[side as usize];
            let e = elbow_offset(&o, upper, lower, hand);
            let outward = axes(&o, Joint::UpperChest).0 * if side == Side::Left { -1.0 } else { 1.0 };
            let (up, out) = (e.y / e.length(), e.dot(outward) / e.length());
            assert!(up <= max_up, "{name} {side:?}: elbow points up ({up:.2}, offset {e})");
            assert!(out > -0.35, "{name} {side:?}: elbow points into the body ({out:.2})");
            assert!(!elbow_in_torso(&o, lower), "{name} {side:?}: elbow inside the ribcage");
            assert!(pos(&o, lower).y < pos(&o, upper).y + 0.15, "{name} {side:?}: elbow above the shoulder");
        }
        assert_hands_pinned(&o, &i, name);
    }
    // Reaching back over the shoulder the elbow comes up and forward, but never through the body.
    let o = settle(&mut solver(), &named_pose("reach_shoulder"), 120);
    let e = elbow_offset(&o, Joint::RightUpperArm, Joint::RightLowerArm, Joint::RightHand);
    assert!(!elbow_in_torso(&o, Joint::RightLowerArm));
    assert!(e.dot(axes(&o, Joint::UpperChest).0) > 0.0 || e.z > 0.0, "elbow out or forward: {e}");
    assert!(e.y < 0.8 * e.length(), "elbow not straight up: {e}");
}

#[test]
fn elbow_flexion_stays_inside_the_clamps() {
    let mut s = solver();
    let mut checked = 0;
    for x in [-0.3, 0.0, 0.2, 0.45, 0.7] {
        for y in [0.7, 1.0, 1.3, 1.6, 1.9] {
            for z in [-0.35, 0.0, 0.25, 0.5, 0.8] {
                for rot in [Quat::IDENTITY, Quat::from_rotation_x(deg(-65.0)), Quat::from_rotation_z(deg(80.0))] {
                    let grip = Pose { pos: Vec3::new(x, y, z), rot };
                    let i = input(standing_head(), Some(hanging_grip(Side::Left)), Some(grip));
                    let o = settle(&mut s, &i, 3);
                    let flex = elbow_flexion(&o, Side::Right);
                    assert!((9.5..=145.5).contains(&flex), "hand at {}: elbow at {flex:.1} deg", grip.pos);
                    assert_hands_pinned(&o, &i, "sweep");
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 375);
}

#[test]
fn out_of_reach_hands_stay_on_the_controllers() {
    let mut s = solver();
    let neutral = settle(&mut s, &named_pose("neutral"), 30);
    let upper = neutral.arm_length * 0.5;
    for distance in [1.0, 4.0, 100.0] {
        let far = Pose { pos: Vec3::new(0.2, 1.4, distance), rot: Quat::IDENTITY };
        let i = input(standing_head(), Some(hanging_grip(Side::Left)), Some(far));
        let o = settle(&mut s, &i, 30);
        assert!(all_finite(&o));
        assert!(!o.reached[1], "{distance} m away is out of reach");
        assert!(o.reached[0]);
        assert_hands_pinned(&o, &i, "far");
        // The arm stretches a little at most; the hand is left floating beyond that.
        let bone = pos(&o, Joint::RightUpperArm).distance(pos(&o, Joint::RightLowerArm));
        assert!(bone <= upper * 1.05 + 1e-4, "upper arm {bone} vs {upper}");
    }
}

#[test]
fn a_lost_hand_eases_down_to_the_side() {
    let mut s = solver();
    let held = input(standing_head(), Some(hanging_grip(Side::Left)), Some(gun_grip(Vec3::new(0.1, 1.3, 0.35), Vec3::Z, PISTOL)));
    let before = settle(&mut s, &held, 60);
    let lost = TrackedInput { grips: [held.grips[0], None], ..held };
    let first = s.solve(&lost);
    let moved = pos(&first, Joint::RightHand).distance(pos(&before, Joint::RightHand));
    assert!(moved < 0.05, "the hand eased instead of snapping ({moved} m)");
    let o = settle(&mut s, &lost, 90);
    assert!(pos(&o, Joint::RightHand).y < pos(&o, Joint::RightUpperArm).y - 0.45, "hangs at the side");
    assert!(o.reached[1] && all_finite(&o));
    // Tracking back: the hand is on the controller again at once.
    let back = s.solve(&held);
    assert_hands_pinned(&back, &held, "re-acquired");
}

// ---- legs ------------------------------------------------------------------------------------

/// Feet in the air: the ball of the foot a centimetre off the floor (a heel raised on tiptoe
/// doesn't count).
fn lifted(o: &BodyOutput) -> [bool; 2] {
    [Joint::LeftToes, Joint::RightToes].map(|f| pos(o, f).y > 0.02 * o.scale + 0.01)
}

#[test]
fn walking_in_the_room_steps_and_ends_under_the_body() {
    let mut s = solver();
    let start = named_pose("neutral");
    settle(&mut s, &start, 60);
    let walk = |t: f32| {
        let d = Vec3::Z * t;
        TrackedInput {
            head: Pose { pos: start.head.pos + d, ..start.head },
            grips: start.grips.map(|g| g.map(|g| Pose { pos: g.pos + d, ..g })),
            ..start
        }
    };
    let mut outs = run(&mut s, 1.25, walk);
    outs.extend(run(&mut s, 1.5, |_| walk(1.0)));
    let mut steps = 0;
    let mut was = [false; 2];
    for o in &outs {
        let now = lifted(o);
        steps += (0..2).filter(|&k| now[k] && !was[k]).count();
        was = now;
        assert_on_floor(o, "walking");
    }
    assert!(steps >= 2, "only {steps} steps for a metre");
    let o = outs.last().unwrap();
    assert_eq!(lifted(o), [false, false], "standing still at the end");
    let (l, r) = (pos(o, Joint::LeftFoot), pos(o, Joint::RightFoot));
    let mid = (l + r) * 0.5;
    let hips = pos(o, Joint::Hips);
    assert!(Vec3::new(mid.x - hips.x, 0.0, mid.z - hips.z).length() < 0.12, "feet at {mid}, hips at {hips}");
    assert!(l.distance(r) > 0.2, "feet apart");
}

#[test]
fn stick_locomotion_walks_with_alternating_feet() {
    let mut s = solver();
    let stand = named_pose("neutral");
    settle(&mut s, &stand, 60);
    let walking = TrackedInput { locomotion: Vec3::new(0.0, 0.0, 1.4), ..stand };
    let outs = run(&mut s, 4.0, |_| walking);
    let mut lift_offs = Vec::new();
    let mut was = [false; 2];
    for (k, o) in outs.iter().enumerate() {
        let now = lifted(o);
        assert!(!(now[0] && now[1]), "frame {k}: both feet in the air");
        for f in 0..2 {
            if now[f] && !was[f] {
                lift_offs.push((k, f));
            }
        }
        was = now;
        assert_on_floor(o, "stick walking");
        assert_hands_pinned(o, &walking, "stick walking");
    }
    assert!(lift_offs.len() >= 6, "only {} steps in 4 s", lift_offs.len());
    for w in lift_offs.windows(2) {
        assert_ne!(w[0].1, w[1].1, "the same foot stepped twice: {lift_offs:?}");
    }
    // FRIK's step time at walking speed is 0.5 s (clamped to 0.28-0.5 s).
    let span = (lift_offs.last().unwrap().0 - lift_offs[1].0) as f32 * DT / (lift_offs.len() - 2) as f32;
    assert!((0.28..=0.55).contains(&span), "a step every {span:.2} s");
    // Stopping: the gait finishes its step and the feet come back under the body.
    let o = settle(&mut s, &stand, 150);
    assert_eq!(lifted(&o), [false, false]);
    let mid = (pos(&o, Joint::LeftFoot) + pos(&o, Joint::RightFoot)) * 0.5;
    let hips = pos(&o, Joint::Hips);
    assert!(Vec3::new(mid.x - hips.x, 0.0, mid.z - hips.z).length() < 0.15, "feet at {mid}");
}

#[test]
fn seated_mode_sits() {
    let i = named_pose("seated");
    let o = settle(&mut solver(), &i, 120);
    assert!(spine_lean(&o) < 10.0, "sitting upright, not crouched ({:.1} deg)", spine_lean(&o));
    for side in [Side::Left, Side::Right] {
        let knee = knee_flexion(&o, side);
        assert!((70.0..=120.0).contains(&knee), "{side:?} knee {knee:.1} deg");
    }
    let hips = pos(&o, Joint::Hips);
    assert!((0.45..0.7).contains(&hips.y), "hips at {}", hips.y);
    for foot in [Joint::LeftFoot, Joint::RightFoot] {
        assert!(pos(&o, foot).z > hips.z + 0.2, "feet in front of the seat");
    }
    for h in sole_heights(&o) {
        assert!(h.abs() < 2e-3);
    }
}

// ---- calibration -----------------------------------------------------------------------------

fn bone(o: &BodyOutput, a: Joint, b: Joint) -> f32 {
    pos(o, a).distance(pos(o, b))
}

#[test]
fn calibration_scales_bone_lengths() {
    let measure = |c: Calibration| {
        let mut s = BodySolver::new(BodyParams::default(), c);
        let i = input(head(Vec3::new(0.0, c.eye_height, 0.0), 0.0, 0.0), None, None);
        settle(&mut s, &i, 60)
    };
    let base = measure(Calibration::default());
    for eye in [1.5, 1.9] {
        let o = measure(Calibration { eye_height: eye, arm_span: 1.7 });
        let ratio = eye / 1.7;
        assert!((o.scale - ratio).abs() < 1e-5);
        for (a, b) in [(Joint::LeftUpperLeg, Joint::LeftLowerLeg), (Joint::Spine, Joint::Chest), (Joint::Neck, Joint::Head)] {
            let r = bone(&o, a, b) / bone(&base, a, b);
            assert!((r - ratio).abs() < 0.01, "{a:?}: {r} vs {ratio}");
        }
        assert!((pos(&o, Joint::Hips).y / pos(&base, Joint::Hips).y - ratio).abs() < 0.01);
    }
    // Arm span sets the arm, (span - shoulders - 2 wrist reaches) / 4 per segment (Parger).
    let mut last = 0.0;
    for span in [1.5, 1.7, 2.0] {
        let o = measure(Calibration { eye_height: 1.7, arm_span: span });
        let upper = bone(&o, Joint::RightUpperArm, Joint::RightLowerArm);
        assert!((upper - o.arm_length * 0.5).abs() < 1e-3, "unstretched at rest");
        assert!(o.arm_length > last + 0.04, "span {span}: arm {}", o.arm_length);
        last = o.arm_length;
    }
}

#[test]
fn calibration_steps_measure_the_player() {
    let mut s = solver();
    s.calibrate_standing(Pose { pos: Vec3::new(0.3, 1.82, -0.2), rot: Quat::IDENTITY });
    assert_eq!(s.calibration().eye_height, 1.82);
    let head = Pose { pos: Vec3::new(0.0, 1.64, 0.0), rot: Quat::IDENTITY };
    let grip = |x: f32| Pose { pos: Vec3::new(x, 1.4, -0.09), rot: Quat::IDENTITY };
    s.calibrate_tpose(head, grip(-0.8), grip(0.8));
    assert_eq!(s.calibration(), Calibration { eye_height: 1.64, arm_span: 1.6 });
    // Nonsense is clamped or ignored.
    s.set_calibration(Calibration { eye_height: f32::NAN, arm_span: 50.0 });
    assert_eq!(s.calibration().eye_height, 1.64);
    assert!(s.calibration().arm_span <= 2.6);
    let o = settle(&mut s, &named_pose("neutral"), 10);
    assert!(all_finite(&o));
}

// ---- robustness and speed ----------------------------------------------------------------------

#[test]
fn degenerate_inputs_never_produce_nan() {
    let h = standing_head();
    let at = |p: Vec3| Some(Pose { pos: p, rot: Quat::IDENTITY });
    let nan = Vec3::splat(f32::NAN);
    let mut cases = vec![
        ("hands at the eyes", input(h, at(h.pos), at(h.pos))),
        ("hands together", input(h, at(Vec3::new(0.0, 1.3, 0.2)), at(Vec3::new(0.0, 1.3, 0.2)))),
        ("hands on the shoulders", input(h, at(Vec3::new(-0.155, 1.44, -0.095)), at(Vec3::new(0.155, 1.44, -0.095)))),
        ("no hands", input(h, None, None)),
        ("hands 100 m away", input(h, at(Vec3::splat(100.0)), at(Vec3::splat(-100.0)))),
        ("hands under the floor", input(h, at(Vec3::new(-0.2, -5.0, 0.0)), at(Vec3::new(0.2, -5.0, 0.0)))),
        ("NaN grip", input(h, at(nan), Some(Pose { pos: Vec3::ZERO, rot: Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0) }))),
        ("zero quaternion head", input(Pose { pos: h.pos, rot: Quat::from_xyzw(0.0, 0.0, 0.0, 0.0) }, None, None)),
        ("NaN head", input(Pose { pos: nan, rot: Quat::from_xyzw(f32::NAN, 0.0, 0.0, 0.0) }, at(Vec3::ONE), None)),
        ("upside down", input(Pose { pos: h.pos, rot: Quat::from_rotation_z(deg(180.0)) }, None, None)),
        ("looking straight down", input(head(h.pos, 0.0, deg(90.0)), None, None)),
        ("looking straight up", input(head(h.pos, 0.0, deg(-90.0)), None, None)),
        (
            "lying on the floor",
            input(head(Vec3::new(0.0, 0.1, 0.0), 0.0, deg(-90.0)), at(Vec3::new(-0.6, 0.1, 0.0)), at(Vec3::new(0.6, 0.1, 0.0))),
        ),
        ("head on the ceiling", input(head(Vec3::new(0.0, 3.0, 0.0), 0.0, 0.0), None, None)),
    ];
    let mut seated_low = input(head(Vec3::new(0.0, 0.1, 0.0), 0.0, 0.0), None, None);
    seated_low.seated = true;
    cases.push(("seated on the floor", seated_low));
    for dt in [0.0, 1e6, f32::NAN, -1.0, f32::INFINITY] {
        cases.push(("odd dt", TrackedInput { dt, ..named_pose("rifle") }));
    }
    for v in [Vec3::splat(1000.0), nan] {
        cases.push(("odd locomotion", TrackedInput { locomotion: v, ..named_pose("neutral") }));
    }
    // Each case on a fresh solver, and all of them in sequence on one.
    let mut shared = solver();
    for (name, i) in &cases {
        let mut s = solver();
        for _ in 0..5 {
            assert!(all_finite(&s.solve(i)), "{name}");
            assert!(all_finite(&shared.solve(i)), "{name} (after other cases)");
        }
        let usable = |g: &Option<Pose>| g.is_some_and(|g| g.pos.is_finite() && g.rot.is_finite() && g.rot.length() > 0.5);
        if i.grips.iter().all(usable) {
            assert_hands_pinned(&s.solve(i), i, name);
        }
    }
}

#[test]
fn zero_dt_repeats_the_frame() {
    let mut s = solver();
    settle(&mut s, &named_pose("rifle"), 30);
    let still = TrackedInput { dt: 0.0, ..named_pose("two_hand_hip") };
    let a = s.solve(&still);
    let b = s.solve(&still);
    assert_eq!(a, b);
}

#[test]
fn a_solve_takes_well_under_a_tenth_of_a_millisecond() {
    let mut s = solver();
    let poses: Vec<TrackedInput> = POSES.iter().map(|n| named_pose(n)).collect();
    let frames = 20_000;
    let start = std::time::Instant::now();
    let mut checksum = 0.0;
    for k in 0..frames {
        let mut i = poses[(k / 60) % poses.len()];
        i.head = Pose { pos: i.head.pos + Vec3::new(0.0, 0.0, (k as f32 * 0.01).sin() * 0.2), ..i.head };
        checksum += s.solve(&i).joints[Joint::RightLowerArm as usize].pos.x;
    }
    let per = start.elapsed().as_secs_f64() / frames as f64;
    println!("one body solve: {:.1} us (checksum {checksum})", per * 1e6);
    assert!(per < 1e-4, "{:.1} us per solve", per * 1e6);
}

#[test]
fn stick_walking_leaves_the_arms_where_they_are() {
    // Holding a gun with both hands, head and hands still: the stick walks on and stops. What the
    // player sees of their arms (shoulders, elbows) mustn't move with the gait: in VR, walking with
    // the stick isn't the body moving.
    let mut s = solver();
    let stand = named_pose("two_hand_hip");
    let still = settle(&mut s, &stand, 120);
    let walking = TrackedInput { locomotion: Vec3::new(0.0, 0.0, 1.4), ..stand };
    let watched = [Joint::LeftUpperArm, Joint::RightUpperArm, Joint::LeftLowerArm, Joint::RightLowerArm, Joint::UpperChest];
    let mut worst = (0.0f32, Joint::UpperChest);
    let mut check = |o: &BodyOutput| {
        for j in watched {
            let d = pos(o, j).distance(pos(&still, j));
            if d > worst.0 {
                worst = (d, j);
            }
        }
    };
    for o in run(&mut s, 3.0, |_| walking) {
        check(&o);
    }
    for o in run(&mut s, 1.5, |_| stand) {
        check(&o);
    }
    eprintln!("stick walking: the arms moved {:.1} cm at most ({:?})", worst.0 * 100.0, worst.1);
    assert!(worst.0 < 0.01, "stick walking moved {:?} {:.1} cm", worst.1, worst.0 * 100.0);
}

#[test]
fn changing_direction_on_the_stick_leaves_the_arms_where_they_are() {
    // As above, but the stick swings round: forward, then strafing, then back the other way.
    let mut s = solver();
    let stand = named_pose("two_hand_hip");
    let still = settle(&mut s, &stand, 120);
    let watched = [Joint::LeftUpperArm, Joint::RightUpperArm, Joint::LeftLowerArm, Joint::RightLowerArm, Joint::UpperChest];
    let mut worst = (0.0f32, Joint::UpperChest);
    for dir in [Vec3::Z, Vec3::X, -Vec3::X, -Vec3::Z, Vec3::new(1.0, 0.0, 1.0).normalize()] {
        let walking = TrackedInput { locomotion: dir * 1.4, ..stand };
        for o in run(&mut s, 1.2, |_| walking) {
            for j in watched {
                let d = pos(&o, j).distance(pos(&still, j));
                if d > worst.0 {
                    worst = (d, j);
                }
            }
        }
    }
    eprintln!("changing direction: the arms moved {:.1} cm at most ({:?})", worst.0 * 100.0, worst.1);
    assert!(worst.0 < 0.01, "changing direction moved {:?} {:.1} cm", worst.1, worst.0 * 100.0);
}
