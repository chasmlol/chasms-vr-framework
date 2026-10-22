//! Hands drawn in a game's own pose on what they hold (the protocol's hand pose table).
//!
//! A [`HandPoseDesc`] is in object space. [`held_hands`] puts the poses of whatever the hands hold
//! into tracking space the way the runtime's rig holds it: the firing hand's pose on the grip, the
//! support hand's slid along the foregrip line to where the rig put that hand, and either mirrored
//! for the other hand. The runtime and the tests both go through it.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

/// A hand drawn in a game's own pose this frame (tracking space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldHand {
    /// The hand joint's canonical frame: where the wrist is and how the hand is turned.
    pub wrist: Pose,
    /// Finger segment rotations relative to the hand frame (as in [`HandPoseDesc::fingers`]).
    pub fingers: [Quat; 15],
    /// The index finger's segments at a full trigger pull, when it rests on a trigger.
    pub index_pulled: Option<[Quat; 3]>,
    /// How far the trigger is pulled, 0..1.
    pub trigger: f32,
    /// The game's hand size (wrist to middle-finger base, metres); 0 keeps the avatar's.
    pub size: f32,
    /// Where each finger's last segment reaches, in the hand's frame (see
    /// [`HandPoseDesc::reach`]): a point, and how far out along the segment it is.
    pub reach: [Option<(Vec3, f32)>; 5],
    /// The index's reach point at a full pull, when it rests on a trigger.
    pub index_reach_pulled: Option<(Vec3, f32)>,
    /// How far the hand has gone into this pose from its own (curled fingers, its own size): 1 all
    /// the way, less while it takes hold or lets go.
    pub weight: f32,
}

impl HeldHand {
    /// As the protocol carries it to an adapter that draws the player ([`RtPlayerDraw`]).
    pub fn to_drawn(&self) -> DrawnHand {
        let q = |r: Quat| r.to_array();
        let reach = |r: (Vec3, f32)| [r.0.x, r.0.y, r.0.z, r.1];
        let mut d: DrawnHand = bytemuck::Zeroable::zeroed();
        d.flags = DH_HELD;
        d.trigger = self.trigger;
        d.size = self.size;
        d.weight = self.weight;
        d.wrist = self.wrist;
        d.fingers = self.fingers.map(q);
        if let Some(p) = self.index_pulled {
            d.flags |= DH_INDEX_PULLED;
            d.index_pulled = p.map(q);
        }
        for (k, r) in self.reach.iter().enumerate() {
            if let Some(r) = r {
                d.reach_mask |= 1 << k;
                d.reach[k] = reach(*r);
            }
        }
        if let Some(r) = self.index_reach_pulled {
            d.flags |= DH_INDEX_REACH_PULLED;
            d.index_reach_pulled = reach(r);
        }
        d
    }

    /// The hand an adapter was given ([`HeldHand::to_drawn`]); None when it isn't held.
    pub fn from_drawn(d: &DrawnHand) -> Option<HeldHand> {
        if d.flags & DH_HELD == 0 {
            return None;
        }
        let q = |a: [f32; 4]| Quat::from_array(a);
        let reach = |a: [f32; 4]| (Vec3::new(a[0], a[1], a[2]), a[3]);
        Some(HeldHand {
            wrist: d.wrist,
            fingers: d.fingers.map(q),
            index_pulled: (d.flags & DH_INDEX_PULLED != 0).then(|| d.index_pulled.map(q)),
            trigger: d.trigger,
            size: d.size,
            reach: std::array::from_fn(|k| (d.reach_mask & (1 << k) != 0).then(|| reach(d.reach[k]))),
            index_reach_pulled: (d.flags & DH_INDEX_REACH_PULLED != 0).then(|| reach(d.index_reach_pulled)),
            weight: if d.weight > 0.0 && d.weight.is_finite() { d.weight.min(1.0) } else { 1.0 },
        })
    }

    fn pull(&self) -> f32 {
        if self.trigger.is_finite() { self.trigger.clamp(0.0, 1.0) } else { 0.0 }
    }

    /// The finger rotations to draw: the index blended towards its pulled pose by the trigger.
    pub fn finger_rotations(&self) -> [Quat; 15] {
        let mut out = self.fingers;
        if let Some(pulled) = self.index_pulled {
            let t = self.pull();
            for k in 0..3 {
                out[3 + k] = self.fingers[3 + k].slerp(pulled[k], t).normalize();
            }
        }
        out
    }

    /// The reach points to draw: the index's blended towards its pulled one by the trigger.
    pub fn reach_points(&self) -> [Option<(Vec3, f32)>; 5] {
        let mut out = self.reach;
        if let (Some((rest, along)), Some((pulled, _))) = (self.reach[1], self.index_reach_pulled) {
            out[1] = Some((rest.lerp(pulled, self.pull()), along));
        }
        out
    }

    /// A pose (object space) on an object at `object` (tracking space) for hand `side`: mirrored
    /// through the object's plane `x = mirror_x` when the pose was made for the other hand, and
    /// slid `slide` metres along the object's +Z.
    pub fn place(desc: &HandPoseDesc, object: (Vec3, Quat), side: usize, mirror_x: f32, slide: f32, trigger: f32) -> HeldHand {
        let mirror = desc.side as usize != side;
        let m = |q: Quat| if mirror { Quat::from_xyzw(q.x, -q.y, -q.z, q.w) } else { q };
        let mut p = Vec3::from_array(desc.wrist.pos);
        if mirror {
            p.x = 2.0 * mirror_x - p.x;
        }
        p.z += slide;
        let wrist_rot = m(quat(desc.wrist.rot));
        let (op, oq) = object;
        let fingers = desc.fingers.map(|f| m(quat(f)));
        let index_pulled = (desc.flags & HPOSE_TRIGGER != 0).then(|| desc.index_pulled.map(|f| m(quat(f))));
        // A point in the hand's frame: the mirror image's frame mirrors it across its own YZ plane.
        let point = |r: [f32; 4]| (r[3] > 0.0 && r[3].is_finite()).then(|| (Vec3::new(if mirror { -r[0] } else { r[0] }, r[1], r[2]), r[3]));
        HeldHand {
            reach: desc.reach.map(point),
            index_reach_pulled: if desc.flags & HPOSE_TRIGGER != 0 { point(desc.index_reach_pulled) } else { None },
            wrist: Pose { pos: (op + oq * p).to_array(), valid: 1, rot: (oq * wrist_rot).normalize().to_array() },
            fingers,
            index_pulled,
            trigger,
            size: desc.size.max(0.0),
            weight: 1.0,
        }
    }
}

/// Which hands hold an object with one of the game's poses, and where those poses are this frame.
///
/// * `grips`: each hand's controller grip pose as the rig holds it (tracking space; the firing
///   hand on the object's `grip`, a support hand on its foregrip line), None if not tracked;
/// * `hands`: the rig's hand states (`HAND_HOLDING` and what each holds);
/// * `states`: the object states the rig just wrote (held objects in tracking space);
/// * `objects`, `poses`: the adapter's tables;
/// * `triggers`: each controller's trigger.
pub fn held_hands(
    grips: [Option<(Vec3, Quat)>; 2],
    hands: &[HandState; 2],
    states: &ObjectStates,
    objects: &ObjectTable,
    poses: &HandPoseTable,
    triggers: [f32; 2],
) -> [Option<HeldHand>; 2] {
    let states = &states.states[..(states.count as usize).min(MAX_OBJECTS)];
    let descs = &objects.descs[..(objects.count as usize).min(MAX_OBJECTS)];
    [0, 1].map(|side| {
        let h = &hands[side];
        if h.flags & HAND_HOLDING == 0 || h.holding == 0 {
            return None;
        }
        let (grip, _) = grips[side]?;
        let s = states.iter().find(|s| s.id == h.holding && s.flags & OS_TRACKING != 0)?;
        let d = descs.iter().find(|d| d.id == s.desc && d.id != 0)?;
        if d.kind != OBJ_WEAPON {
            return None;
        }
        let object = (Vec3::from_array(s.pose.pos), quat(s.pose.rot));
        let firing = s.flags & if side == 0 { OS_HELD_LEFT } else { OS_HELD_RIGHT } != 0;
        if firing {
            let pose = poses.find(d.grip_pose)?;
            Some(HeldHand::place(pose, object, side, d.grip[0], 0.0, triggers[side]))
        } else {
            let pose = poses.find(d.support_pose)?;
            Some(HeldHand::place(pose, object, side, d.foregrip[0], support_slide(d, object, grip), 0.0))
        }
    })
}

/// How far along the barrel from the game's support pose the rig holds a support hand: always
/// none (the support hand locks to the game's own place on the gun).
pub fn support_slide(_d: &ObjectDesc, _object: (Vec3, Quat), _grip: Vec3) -> f32 {
    0.0
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if !q.is_finite() || q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    /// What an adapter that draws the player gets is the hand the runtime posed.
    #[test]
    fn a_held_hand_crosses_to_the_adapter_unchanged() {
        let h = HeldHand {
            wrist: Pose { pos: [0.1, 1.3, 0.4], valid: 1, rot: Quat::from_rotation_x(0.3).to_array() },
            fingers: std::array::from_fn(|k| Quat::from_rotation_z(-0.05 * k as f32)),
            index_pulled: Some([Quat::from_rotation_z(-0.9); 3]),
            trigger: 0.4,
            size: 0.095,
            reach: [None, Some((Vec3::new(0.01, 0.08, 0.0), 0.7)), None, None, Some((Vec3::new(-0.02, 0.06, 0.01), 0.5))],
            index_reach_pulled: None,
            weight: 0.4,
        };
        assert_eq!(HeldHand::from_drawn(&h.to_drawn()), Some(h));
        assert_eq!(HeldHand::from_drawn(&DrawnHand::zeroed()), None);
    }

    fn pose_desc(id: u32, side: u32) -> HandPoseDesc {
        let mut p = HandPoseDesc::zeroed();
        p.id = id;
        p.side = side;
        p.size = 0.1;
        p.wrist = Pose { pos: [0.03, -0.02, -0.05], valid: 1, rot: Quat::from_rotation_y(0.4).to_array() };
        p.fingers[3] = Quat::from_rotation_z(-0.5).to_array();
        p.index_pulled = [Quat::from_rotation_z(-0.9).to_array(); 3];
        p.flags = HPOSE_TRIGGER;
        p
    }

    #[test]
    fn mirroring_reflects_the_hand_through_the_grip_plane() {
        let p = pose_desc(1, HAND_RIGHT as u32);
        let at = (Vec3::ZERO, Quat::IDENTITY);
        let right = HeldHand::place(&p, at, 1, 0.01, 0.0, 0.0);
        let left = HeldHand::place(&p, at, 0, 0.01, 0.0, 0.0);
        let (r, l) = (Vec3::from_array(right.wrist.pos), Vec3::from_array(left.wrist.pos));
        assert!((r.x + l.x - 0.02).abs() < 1e-6 && (r.y - l.y).abs() < 1e-6 && (r.z - l.z).abs() < 1e-6);
        // The mirrored hand frame: Y and Z reflected, X reflected and flipped (X = Y x Z).
        let (rq, lq) = (Quat::from_array(right.wrist.rot), Quat::from_array(left.wrist.rot));
        let m = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        assert!((lq * Vec3::Y).abs_diff_eq(m(rq * Vec3::Y), 1e-5));
        assert!((lq * Vec3::Z).abs_diff_eq(m(rq * Vec3::Z), 1e-5));
        assert!((lq * Vec3::X).abs_diff_eq(-m(rq * Vec3::X), 1e-5));
        // Fingers bend the mirror way in the mirrored hand's own frame.
        let (fr, fl) = (right.fingers[3], left.fingers[3]);
        assert!(((rq * fr) * Vec3::Y).abs_diff_eq(m((lq * fl) * Vec3::Y), 1e-5));
    }

    #[test]
    fn the_index_follows_the_trigger() {
        let p = pose_desc(1, HAND_RIGHT as u32);
        let mut h = HeldHand::place(&p, (Vec3::ZERO, Quat::IDENTITY), 1, 0.0, 0.0, 0.0);
        let rest = h.finger_rotations()[3];
        h.trigger = 1.0;
        let pulled = h.finger_rotations()[3];
        h.trigger = 0.5;
        let half = h.finger_rotations()[3];
        assert!(rest.angle_between(Quat::from_rotation_z(-0.5)) < 1e-5);
        assert!(pulled.angle_between(Quat::from_rotation_z(-0.9)) < 1e-5);
        assert!((half.angle_between(rest) - 0.2).abs() < 1e-4);
        assert_eq!(h.finger_rotations()[6], Quat::IDENTITY, "the other fingers stay");
    }

    #[test]
    fn held_hands_follow_the_rig() {
        let mut objects: Box<ObjectTable> = bytemuck::zeroed_box();
        let mut states: Box<ObjectStates> = bytemuck::zeroed_box();
        let mut poses: Box<HandPoseTable> = bytemuck::zeroed_box();
        let d = &mut objects.descs[0];
        d.id = 5;
        d.kind = OBJ_WEAPON;
        d.foregrip = [0.0, -0.01, 0.3];
        d.grip_pose = 11;
        d.support_pose = 12;
        objects.count = 1;
        poses.poses[0] = pose_desc(11, HAND_RIGHT as u32);
        poses.poses[1] = pose_desc(12, HAND_LEFT as u32);
        poses.count = 2;
        let gun = (Vec3::new(0.1, 1.4, 0.3), Quat::from_rotation_y(0.3));
        states.states[0] = ObjectState {
            id: 5,
            flags: OS_SIMULATED | OS_TRACKING | OS_HELD_RIGHT,
            pose: Pose { pos: gun.0.to_array(), valid: 1, rot: gun.1.to_array() },
            desc: 5,
            ..ObjectState::zeroed()
        };
        states.count = 1;
        let holding = HandState { flags: HAND_TRACKED | HAND_HOLDING, holding: 5, ..Default::default() };
        // The support hand's controller 2 cm back along the foregrip line.
        let support_grip = gun.0 + gun.1 * Vec3::new(0.0, -0.01, 0.28);
        let grips = [Some((support_grip, Quat::IDENTITY)), Some((gun.0, gun.1))];
        let held = held_hands(grips, &[holding, holding], &states, &objects, &poses, [0.9, 0.7]);
        let [Some(l), Some(r)] = held else { panic!("both hands posed: {held:?}") };
        assert_eq!(r.trigger, 0.7, "the firing hand's trigger");
        assert_eq!(l.trigger, 0.0);
        // The support hand stays where the game holds the gun: it doesn't slide with the controller.
        let want = gun.0 + gun.1 * Vec3::new(0.03, -0.02, -0.05);
        assert!(Vec3::from_array(l.wrist.pos).distance(want) < 1e-5, "on the game's pose, not slid back: {:?}", l.wrist.pos);
        let want = gun.0 + gun.1 * Vec3::new(0.03, -0.02, -0.05);
        assert!(Vec3::from_array(r.wrist.pos).distance(want) < 1e-5);
        // Not holding, or no pose: the curls.
        let none = held_hands(grips, &[HandState::default(), holding], &states, &objects, &poses, [0.0; 2]);
        assert!(none[0].is_none() && none[1].is_some());
        objects.descs[0].grip_pose = 0;
        assert!(held_hands(grips, &[holding, holding], &states, &objects, &poses, [0.0; 2])[1].is_none());
    }
}
