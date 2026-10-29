//! Retargeting: drives any humanoid skeleton from the protocol's 22 joints and finger curls.
//!
//! At load time every bone the names identify ([`crate::names`]) is mapped to a protocol joint,
//! and its *canonical frame in the bind pose* is computed from the bind positions alone: Y along
//! the bone towards the child joint, Z forward (the body's forward, or the thumb side for hands,
//! or up for feet), X = Y x Z. That's the same frame the protocol puts on its joints, so each
//! frame
//!
//! ```text
//! posed_rotation = joint_rotation * inverse(canonical_bind) * bind_rotation
//! posed_position = joint_position
//! ```
//!
//! turns the bone from where it is in the bind pose to where the joint is now, whatever local axes
//! the rig's author picked. Everything else follows:
//!
//! * fingers curl about their own flex axis (measured from the bind pose, so curl 0 is straight
//!   and 1 is a fist whatever the bind hand looked like) and splay about the palm normal;
//! * twist bones along the forearm and shin take a share of the hand's (foot's) twist, by how far
//!   along the segment their skin sits;
//! * other bones (helpers, face, clothing, attachments) stay rigid with their parent;
//! * the whole model is scaled uniformly so its segment lengths match the joints, since the model
//!   is whoever the artist made and the joints are the real player.

use anyhow::{Result, bail};
use glam::{Mat3, Mat4, Quat, Vec3, Vec4};
use rustvr_asset as asset;
use rustvr_protocol::{Fingers, JOINT_COUNT, Joint, Pose};

use crate::grasp::{self, Grasp};
use crate::names::{self, Class, Limb};

/// Most bones a model may have: vertex bone indices are bytes.
pub const MAX_BONES: usize = 256;

/// Full flex (radians at curl 1) of each finger's segments, base first. The base of the thumb is
/// added to its rest pose; every other angle replaces the bind pose's bend.
const CURL: [[f32; 3]; 5] = [
    [0.45, 0.60, 0.80], // thumb: its base towards the palm, then the two knuckles
    [1.22, 1.66, 1.05], // index
    [1.36, 1.75, 1.13],
    [1.43, 1.75, 1.08],
    [1.54, 1.66, 1.05], // little
];
/// How far each finger's base bends back from straight at most (a curl: a finger pressed flat on
/// something, a thumb moved off it).
const LIFT: [f32; 5] = [1.6, 0.35, 0.35, 0.35, 0.35];
/// Spread at splay 1 (radians towards the thumb side) of each finger's base. Fingers close up as
/// they curl into a fist.
const SPREAD: [f32; 5] = [0.35, 0.21, 0.03, -0.14, -0.31];

/// Visibility sets: what part of the body each draw shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    Full,
    /// Hands, forearms and upper arms (cut just below the shoulder).
    Arms,
    /// Hands and wrists (cut a little way up the forearm).
    Hands,
    /// One hand and its wrist (a ghost).
    Hand(usize),
}

/// Which part of the body a bone (and the skin on it) belongs to, for [`Show`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Body,
    UpperArm(usize),
    LowerArm(usize),
    Hand(usize),
}

#[derive(Clone, Copy, Debug)]
enum Drive {
    /// Rotation and position from a protocol joint.
    Joint(Joint),
    /// Rigid with its parent; a root that isn't a body part rides with the anchor joint.
    Follow,
    /// One segment of a finger, curled from [`Fingers`].
    Finger { side: usize, finger: usize, segment: usize },
    /// Rigid with its parent plus `share` of the twist between the `segment` joint and the
    /// `driver` joint below it, about the segment's axis.
    Twist { segment: Joint, driver: Joint, share: f32 },
}

struct Bone {
    parent: Option<usize>,
    bind_pos: Vec3,
    bind_rot: Quat,
    /// Bind pose relative to the parent.
    local_pos: Vec3,
    local_rot: Quat,
    inv_bind: Mat4,
    drive: Drive,
    part: Part,
    /// The head or anything under it (face, hair, hats): collapsed for the wearer.
    in_head: bool,
}

/// A finger's (up to) three curling segments.
pub(crate) struct Finger {
    pub(crate) count: usize,
    /// Bind positions of the segments' pivots.
    pub(crate) pivots: [Vec3; 3],
    /// Each segment's canonical frame in the bind pose (see [`segment_frame`]).
    pub(crate) canon: [Quat; 3],
    /// How far the last segment reaches from its pivot (to the fingertip), along its +Y.
    pub(crate) tip_len: f32,
    /// Bend towards the palm is a positive rotation about this (bind space).
    flex_axis: Vec3,
    /// How far each joint is already bent in the bind pose.
    bind_flex: [f32; 3],
    /// Spread towards the thumb is a positive rotation about this (the palm normal).
    splay_axis: Vec3,
    bind_splay: f32,
}

/// The rig's per-frame inputs.
pub struct PoseInput<'a> {
    pub joints: &'a [Pose; JOINT_COUNT],
    /// Where the hands are drawn, overriding the hand joints (the hand is pinned, the arm bends).
    pub hands: [Option<Pose>; 2],
    pub fingers: &'a [Fingers; 2],
    /// Hands in a game's own pose: their fingers come from it instead of the curls.
    pub held: [Option<PosedFingers>; 2],
    /// Hands holding something (not in a game's pose): their fingers close on it.
    pub grasp: [Option<&'a Grasp>; 2],
    /// Collapse the head for the wearer's own eyes.
    pub chop_head: bool,
}

/// A whole hand's finger pose: every segment's canonical frame relative to the hand's (thumb,
/// index, middle, ring, little; three segments each, base first), and the size of the hand it was
/// made on (wrist to middle-finger base; 0 keeps the model's).
///
/// `reach`: for each finger, a point its last segment should touch, in the hand's frame (metres,
/// for a hand of `size`), and where along that segment the point is (0 its pivot, 1 the
/// fingertip). A hand of other proportions than the one the pose was made on bends its fingers a
/// little to get there, so its fingertips (and the trigger finger's contact) land where the
/// game's do.
///
/// `weight`: how far the hand has gone into this pose from its own (curled fingers, its own size):
/// 1 all the way, less while it takes hold of something or lets go (see [`Rig::pose`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PosedFingers {
    pub rel: [Quat; 15],
    pub size: f32,
    pub reach: [Option<(Vec3, f32)>; 5],
    pub weight: f32,
}

/// Results of [`Rig::pose`], allocated once and reused.
pub struct Posed {
    pub rot: Vec<Quat>,
    pub pos: Vec<Vec3>,
    /// Skinning matrices: bind model space -> tracking space.
    pub skin: Vec<Mat4>,
    pub scale: f32,
}

pub struct Rig {
    bones: Vec<Bone>,
    /// Parents before children.
    order: Vec<usize>,
    joint_bone: [Option<usize>; JOINT_COUNT],
    /// Inverse canonical bind frame of each mapped joint (model space).
    inv_canonical: [Quat; JOINT_COUNT],
    canonical: [Quat; JOINT_COUNT],
    fingers: [[Option<Finger>; 5]; 2],
    /// Each hand's size in the bind pose: wrist to the middle finger's base.
    hand_size: [f32; 2],
    head: Option<usize>,
    /// Joint pairs (child, parent) that are both mapped: their lengths give the scale.
    scale_pairs: Vec<(Joint, Joint)>,
    /// The player's hands' size (wrist to the middle finger's knuckle, metres): the hands are drawn
    /// that size rather than the model's at the body's scale. None: the model's.
    hand_length: Option<f32>,
}

impl Rig {
    /// Builds the rig for a skeleton. `meshes` (vertex lists) place the twist bones' shares.
    pub fn new(bones: &[asset::Bone], meshes: &[&[asset::Vertex]]) -> Result<Rig> {
        if bones.is_empty() {
            bail!("the model has no skeleton");
        }
        if bones.len() > MAX_BONES {
            bail!("{} bones (at most {MAX_BONES})", bones.len());
        }
        let n = bones.len();
        let parents: Vec<Option<usize>> =
            bones.iter().enumerate().map(|(i, b)| usize::try_from(b.parent).ok().filter(|&p| p < n && p != i)).collect();
        let tree = Tree::new(parents);
        let classes: Vec<Option<Class>> = bones.iter().map(|b| names::classify(&b.name)).collect();
        let map = Mapping::new(&tree, &classes);

        let world: Vec<Mat4> = bones.iter().map(|b| Mat4::from_cols_array(&b.model_matrix)).collect();
        let bind: Vec<(Quat, Vec3)> = world
            .iter()
            .map(|m| {
                let (_, r, t) = m.to_scale_rotation_translation();
                (r.normalize(), t)
            })
            .collect();
        let pos = |b: usize| bind[b].1;

        for side in 0..2 {
            for (j, what) in [(HAND[side], "hand"), (LOWER_ARM[side], "forearm"), (UPPER_ARM[side], "upper arm")] {
                if map.joints[j as usize].is_none() {
                    bail!("no {} {what} bone", ["left", "right"][side]);
                }
            }
        }

        let canonical = canonical_frames(&map, &tree, &pos);
        let mut inv_canonical = [Quat::IDENTITY; JOINT_COUNT];
        for j in 0..JOINT_COUNT {
            inv_canonical[j] = canonical[j].inverse();
        }

        // Drives.
        let mut drive = vec![Drive::Follow; n];
        for j in Joint::ALL {
            if let Some(b) = map.joints[j as usize] {
                drive[b] = Drive::Joint(j);
            }
        }
        for side in 0..2 {
            for f in 0..5 {
                for (segment, &b) in map.fingers[side][f].iter().enumerate() {
                    drive[b] = Drive::Finger { side, finger: f, segment };
                }
            }
        }
        // Twist bones along the forearm (driven by the hand) and the shin (by the foot); the
        // share is how far along the segment the skin weighted to the bone sits.
        let joint_of = |b: usize| match drive[b] {
            Drive::Joint(j) => Some(j),
            _ => None,
        };
        let mut shares = vec![None; n];
        for &b in &map.twists {
            let Some(seg_bone) = tree.ancestors(b).find(|&a| joint_of(a).is_some()) else { continue };
            let segment = joint_of(seg_bone).unwrap();
            let driver = match segment {
                Joint::LeftLowerArm => Joint::LeftHand,
                Joint::RightLowerArm => Joint::RightHand,
                Joint::LeftLowerLeg => Joint::LeftFoot,
                Joint::RightLowerLeg => Joint::RightFoot,
                _ => continue,
            };
            let Some(driver_bone) = map.joints[driver as usize] else { continue };
            let (a, z) = (pos(seg_bone), pos(driver_bone));
            let share = skin_centroid(meshes, b).map_or(0.5, |c| ((c - a).dot(z - a) / (z - a).length_squared().max(1e-8)).clamp(0.0, 1.0));
            shares[b] = Some((segment, driver, share));
        }
        for &b in &map.twists {
            let Some((segment, driver, share)) = shares[b] else { continue };
            // A twist bone under another one already turns with it.
            let inherited = tree.ancestors(b).find_map(|a| shares[a].filter(|s| s.0 == segment)).map_or(0.0, |s| s.2);
            drive[b] = Drive::Twist { segment, driver, share: share - inherited };
            tracing::debug!("rig: twist bone '{}' takes {:.0}% of the {driver:?}'s roll", bones[b].name, share * 100.0);
        }

        // Parts, for showing only the arms or hands.
        let part_of_bone = |b: usize| {
            for side in 0..2 {
                for (j, part) in [(HAND[side], Part::Hand(side)), (LOWER_ARM[side], Part::LowerArm(side)), (UPPER_ARM[side], Part::UpperArm(side))] {
                    if map.joints[j as usize] == Some(b) {
                        return Some(part);
                    }
                }
            }
            None
        };
        let head = map.joints[Joint::Head as usize];
        let out_bones: Vec<Bone> = (0..n)
            .map(|b| {
                let (rot, p) = bind[b];
                let (local_rot, local_pos) = match tree.parents[b] {
                    Some(pa) => {
                        let inv = bind[pa].0.inverse();
                        (inv * rot, inv * (p - bind[pa].1))
                    }
                    None => (rot, p),
                };
                let part = std::iter::once(b).chain(tree.ancestors(b)).find_map(part_of_bone).unwrap_or(Part::Body);
                let in_head = head.is_some_and(|h| h == b || tree.ancestors(b).any(|a| a == h));
                Bone { parent: tree.parents[b], bind_pos: p, bind_rot: rot, local_pos, local_rot, inv_bind: world[b].inverse(), drive: drive[b], part, in_head }
            })
            .collect();

        let mut fingers: [[Option<Finger>; 5]; 2] = Default::default();
        let mut hand_size = [0.0; 2];
        for side in 0..2 {
            for f in 0..5 {
                fingers[side][f] = finger_chain(&map, side, f, canonical[HAND[side] as usize], &bind);
            }
            hand_size[side] = size_of_hand(&map, side, &pos);
        }
        tips_from_skin(&mut fingers, &map, meshes);

        // The body's size from its trunk and legs: the arms' segments are fitted one by one (the
        // player's arms aren't the model's proportions), so they don't make the whole model
        // bigger or smaller. (A model of the arms alone is sized by them.)
        let pairs: Vec<(Joint, Joint)> = Joint::ALL
            .into_iter()
            .filter_map(|j| Some((j, j.parent()?)))
            .filter(|&(j, p)| map.joints[j as usize].is_some() && map.joints[p as usize].is_some())
            .collect();
        let fitted = |j: Joint| LOWER_ARM.contains(&j) || HAND.contains(&j);
        let trunk: Vec<(Joint, Joint)> = pairs.iter().copied().filter(|&(j, _)| !fitted(j)).collect();
        let scale_pairs = if trunk.is_empty() { pairs } else { trunk };

        Ok(Rig {
            bones: out_bones,
            order: tree.order,
            joint_bone: map.joints,
            inv_canonical,
            canonical,
            fingers,
            hand_size,
            head,
            scale_pairs,
            hand_length: None,
        })
    }

    /// Each hand's size in the bind pose (wrist to the middle finger's base, metres).
    pub fn hand_size(&self) -> [f32; 2] {
        self.hand_size
    }

    /// Draws the hands this size (wrist to the middle finger's knuckle, metres; None: the model's
    /// at the body's scale).
    pub fn set_hand_length(&mut self, length: Option<f32>) {
        self.hand_length = length.filter(|l| *l > 0.02 && l.is_finite());
    }

    /// A hand's own scale, holding nothing in a game's pose: the player's hand's size, or the
    /// body's scale `s`.
    fn own_hand_scale(&self, side: usize, s: f32) -> f32 {
        match self.hand_length {
            Some(l) if self.hand_size[side] > 1e-3 => l / self.hand_size[side],
            _ => s,
        }
    }

    pub fn posed(&self) -> Posed {
        let n = self.bones.len();
        Posed { rot: vec![Quat::IDENTITY; n], pos: vec![Vec3::ZERO; n], skin: vec![Mat4::IDENTITY; n], scale: 1.0 }
    }

    /// The forearm's share of the arm (shoulder to wrist) in the model's bind pose, both arms'
    /// average: the arm's proportions, for a solver to keep (so the arm lengthens evenly, not one
    /// segment more than the other).
    pub fn forearm_share(&self) -> Option<f32> {
        let len = |a: Joint, b: Joint| Some(self.bones[self.joint_bone[a as usize]?].bind_pos.distance(self.bones[self.joint_bone[b as usize]?].bind_pos));
        let shares: Vec<f32> = (0..2)
            .filter_map(|side| {
                let (upper, fore) = (len(UPPER_ARM[side], LOWER_ARM[side])?, len(LOWER_ARM[side], HAND[side])?);
                (upper + fore > 1e-4).then(|| fore / (upper + fore))
            })
            .collect();
        (!shares.is_empty()).then(|| shares.iter().sum::<f32>() / shares.len() as f32)
    }

    /// The canonical frame of `joint` in the bind pose (model space), if the model has it.
    #[cfg(test)]
    pub fn canonical_bind(&self, joint: Joint) -> Option<(Vec3, Quat)> {
        let b = self.joint_bone[joint as usize]?;
        Some((self.bones[b].bind_pos, self.canonical[joint as usize]))
    }

    /// Uniform scale that fits the model's segment lengths to the joints'.
    fn scale(&self, joints: &[Pose; JOINT_COUNT]) -> f32 {
        let mut now = 0.0;
        let mut bind = 0.0;
        for &(j, p) in &self.scale_pairs {
            let (a, b) = (&joints[j as usize], &joints[p as usize]);
            if a.valid == 0 || b.valid == 0 {
                continue;
            }
            now += Vec3::from_array(a.pos).distance(Vec3::from_array(b.pos));
            bind += self.bones[self.joint_bone[j as usize].unwrap()].bind_pos.distance(self.bones[self.joint_bone[p as usize].unwrap()].bind_pos);
        }
        if now > 1e-4 && bind > 1e-4 { now / bind } else { 1.0 }
    }

    /// Poses every bone and fills the skinning matrices.
    pub fn pose(&self, input: &PoseInput, out: &mut Posed) {
        let s = self.scale(input.joints);
        out.scale = s;
        let joint = |j: Joint| -> Option<(Vec3, Quat)> {
            let override_hand = match j {
                Joint::LeftHand => input.hands[0],
                Joint::RightHand => input.hands[1],
                _ => None,
            };
            let p = override_hand.unwrap_or(input.joints[j as usize]);
            (p.valid != 0).then(|| (Vec3::from_array(p.pos), quat(p.rot)))
        };
        // Roots that aren't body parts (Biped's `bip01`, Metro's locators) ride with the hips, or
        // whichever mapped joint comes first.
        let anchor = ANCHORS.iter().find_map(|&j| Some((self.joint_bone[j as usize]?, joint(j)?, j)));
        let (anchor_rot, anchor_pos, anchor_bind) = match anchor {
            Some((b, (p, r), j)) => (r * self.inv_canonical[j as usize], p, self.bones[b].bind_pos),
            None => (Quat::IDENTITY, Vec3::ZERO, Vec3::ZERO),
        };

        // A hand in a game's pose is drawn at that hand's size (the hand and everything on it).
        let hand_scale: [f32; 2] = [0, 1].map(|side| match &input.held[side] {
            Some(h) if h.size > 1e-3 && self.hand_size[side] > 1e-3 => h.size / self.hand_size[side],
            _ => self.own_hand_scale(side, s),
        });
        let scale_of = |b: usize| match self.bones[b].part {
            Part::Hand(side) => hand_scale[side],
            _ => s,
        };

        // Curled fingers: bind-space rotations and pivots, moved with the hand below.
        let mut finger_rot = [[[Quat::IDENTITY; 3]; 5]; 2];
        let mut finger_pos = [[[Vec3::ZERO; 3]; 5]; 2];
        // Posed fingers: tracking-space rotations and pivots, final.
        let mut posed_fingers = [false; 2];
        for side in 0..2 {
            let held = match (&input.held[side], joint(HAND[side]), self.joint_bone[HAND[side] as usize]) {
                (Some(h), Some(at), Some(hb)) => Some((h, at, hb)),
                _ => None,
            };
            posed_fingers[side] = held.is_some();
            for f in 0..5 {
                let Some(chain) = &self.fingers[side][f] else { continue };
                match held {
                    Some((h, (hand_pos, hand_rot), hb)) => {
                        let k_s = hand_scale[side];
                        let delta = hand_rot * self.inv_canonical[HAND[side] as usize];
                        let mut frames = [Quat::IDENTITY; 3];
                        let mut pivots = [Vec3::ZERO; 3];
                        let mut pos = hand_pos + delta * ((chain.pivots[0] - self.bones[hb].bind_pos) * k_s);
                        for k in 0..chain.count {
                            frames[k] = (hand_rot * h.rel[f * 3 + k]).normalize();
                            if k > 0 {
                                pos += frames[k - 1] * chain.canon[k - 1].inverse() * ((chain.pivots[k] - chain.pivots[k - 1]) * k_s);
                            }
                            pivots[k] = pos;
                        }
                        if let Some((target, along)) = h.reach[f] {
                            reach(side, f, chain.count, &mut frames, &mut pivots, chain.tip_len * k_s * along, hand_pos + hand_rot * target, hand_rot);
                        }
                        for k in 0..chain.count {
                            // The bone turns from its bind canonical frame onto the posed one.
                            finger_rot[side][f][k] = frames[k] * chain.canon[k].inverse();
                            finger_pos[side][f][k] = pivots[k];
                        }
                    }
                    None => {
                        let own = &input.fingers[side];
                        // (Pressed flat on something, a finger bends back at its base.)
                        let c = own.curl[f];
                        let mut curls = [c.clamp(-LIFT[f], 1.0), c.clamp(0.0, 1.0), c.clamp(0.0, 1.0)];
                        let mut splay = own.splay;
                        // Holding something: the finger closes on it, as far as the grasp has.
                        let grasp = input.grasp[side].filter(|g| g.fingers & (1 << f) != 0 && g.weight > 0.0);
                        if let (Some(g), Some(hand)) = (grasp, joint(HAND[side])) {
                            let wrapped = self.wrap(side, f, hand, hand_scale[side], g);
                            let w = g.weight.min(1.0);
                            for (c, t) in curls.iter_mut().zip(wrapped) {
                                *c += (t - *c) * w;
                            }
                            splay += (WRAP_SPLAY - splay) * w;
                        }
                        chain.bend(f, curls, splay, &mut finger_rot[side][f], &mut finger_pos[side][f]);
                    }
                }
            }
        }

        for &b in &self.order {
            let bone = &self.bones[b];
            let k_s = scale_of(b);
            let follow = |out: &Posed| match bone.parent {
                Some(p) => (out.rot[p] * bone.local_rot, out.pos[p] + out.rot[p] * (bone.local_pos * k_s)),
                None => (anchor_rot * bone.bind_rot, anchor_pos + anchor_rot * ((bone.bind_pos - anchor_bind) * s)),
            };
            let (rot, pos) = match bone.drive {
                Drive::Joint(j) => match joint(j) {
                    Some((p, r)) => (r * self.inv_canonical[j as usize] * bone.bind_rot, p),
                    None => follow(out),
                },
                Drive::Follow => follow(out),
                Drive::Finger { side, finger, segment } if posed_fingers[side] => {
                    (finger_rot[side][finger][segment] * bone.bind_rot, finger_pos[side][finger][segment])
                }
                Drive::Finger { side, finger, segment } => {
                    let hand = self.joint_bone[HAND[side] as usize].unwrap();
                    let delta = out.rot[hand] * self.bones[hand].bind_rot.inverse();
                    let q = finger_rot[side][finger][segment];
                    let p = finger_pos[side][finger][segment];
                    (delta * q * bone.bind_rot, out.pos[hand] + delta * ((p - self.bones[hand].bind_pos) * k_s))
                }
                Drive::Twist { segment, driver, share } => {
                    let (rot, pos) = follow(out);
                    match (joint(segment), joint(driver)) {
                        (Some((_, a)), Some((_, d))) => {
                            // The driver's turn relative to the segment, in the segment's bind
                            // canonical frame, where the segment runs along +Y.
                            let rel = a.inverse() * d * self.inv_canonical[driver as usize] * self.canonical[segment as usize];
                            let axis = a * Vec3::Y;
                            (Quat::from_axis_angle(axis, share * twist_about(rel, Vec3::Y)) * rot, pos)
                        }
                        _ => (rot, pos),
                    }
                }
            };
            out.rot[b] = rot;
            out.pos[b] = pos;
        }

        let chop = if input.chop_head { self.head.map(|h| out.pos[h]) } else { None };
        for (b, bone) in self.bones.iter().enumerate() {
            out.skin[b] = match chop {
                // Everything under the head shrinks onto the top of the neck: the wearer never sees
                // the inside of their own head, and there's no hole in the neck.
                Some(at) if bone.in_head => Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, at.extend(1.0)),
                _ => Mat4::from_scale_rotation_translation(Vec3::splat(scale_of(b)), out.rot[b], out.pos[b]) * bone.inv_bind,
            };
        }

        // A hand part way into a held pose (or out of one): its bones between the pose and the
        // hand's own (its curled fingers at its own size, the wrist where the hand is), by weight.
        for side in 0..2 {
            let Some(w) = input.held[side].map(|h| h.weight).filter(|w| *w < 1.0 - 1e-4) else { continue };
            let w = w.max(0.0);
            let mut held = input.held;
            held[side] = None;
            let mut own = self.posed();
            self.pose(&PoseInput { held, ..*input }, &mut own);
            let k = hand_scale[side] * w + self.own_hand_scale(side, own.scale) * (1.0 - w);
            for b in (0..self.bones.len()).filter(|&b| self.bones[b].part == Part::Hand(side)) {
                out.rot[b] = own.rot[b].slerp(out.rot[b], w).normalize();
                out.pos[b] = own.pos[b].lerp(out.pos[b], w);
                out.skin[b] = Mat4::from_scale_rotation_translation(Vec3::splat(k), out.rot[b], out.pos[b]) * self.bones[b].inv_bind;
            }
        }

        // Each arm segment's skin spans its two joints exactly: turned onto the line from one to
        // the next and scaled evenly along it. The arm is the player's, not the model's (and a hand
        // can be where the arm doesn't quite reach): the difference is spread along the whole bone
        // rather than tearing the skin open at the elbow or the wrist.
        for side in 0..2 {
            for (part, from, to) in [(Part::UpperArm(side), UPPER_ARM[side], LOWER_ARM[side]), (Part::LowerArm(side), LOWER_ARM[side], HAND[side])] {
                let (Some(a), Some(b), Some((_, r))) = (self.joint_bone[from as usize], self.joint_bone[to as usize], joint(from)) else { continue };
                let length = self.bones[a].bind_pos.distance(self.bones[b].bind_pos) * s;
                let Some(fit) = segment_fit(out.pos[a], r * Vec3::Y, out.pos[b], length) else { continue };
                for (k, bone) in self.bones.iter().enumerate() {
                    if bone.part == part {
                        out.skin[k] = fit * out.skin[k];
                    }
                }
            }
        }
    }

    /// Per-bone visibility half-spaces in bind space for `show`: a vertex's visibility is the
    /// skin-weighted sum of `dot(plane.xyz, bind_position) + plane.w` over its bones, and only
    /// skin where that's non-negative is drawn. Hidden parts are -1. Every shown bone of an arm
    /// carries the same plane across the limb (the signed distance from the cut, in metres), so
    /// skin blended between the hand and the forearm is still cut exactly at the plane: the cut
    /// is clean and nothing stretches.
    pub fn visibility(&self, show: Show) -> Vec<[f32; 4]> {
        const SHOWN: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
        const HIDDEN: [f32; 4] = [0.0, 0.0, 0.0, -1.0];
        let cut = |from: Joint, to: Joint, at: f32| {
            let a = self.bones[self.joint_bone[from as usize].unwrap()].bind_pos;
            let b = self.bones[self.joint_bone[to as usize].unwrap()].bind_pos;
            let d = (b - a).normalize_or(Vec3::Y);
            let p = a.lerp(b, at);
            [d.x, d.y, d.z, -d.dot(p)]
        };
        let sides = |side: usize| match show {
            Show::Hand(s) => s == side,
            _ => true,
        };
        self.bones
            .iter()
            .map(|bone| match (show, bone.part) {
                (Show::Full, _) => SHOWN,
                (Show::Arms, Part::Hand(side) | Part::LowerArm(side) | Part::UpperArm(side)) => cut(UPPER_ARM[side], LOWER_ARM[side], 0.2),
                (Show::Hands | Show::Hand(_), Part::Hand(side) | Part::LowerArm(side)) if sides(side) => cut(LOWER_ARM[side], HAND[side], 0.78),
                _ => HIDDEN,
            })
            .collect()
    }
}

/// How far apart the fingers spread while they close on something (as [`Fingers::splay`]).
const WRAP_SPLAY: f32 = 0.1;
/// A finger's half thickness as a share of the hand's size (wrist to the middle finger's base):
/// 9 mm on a 9.5 cm hand.
const FINGER_THICKNESS: f32 = 0.095;

/// A finger of the posed hand, for [`grasp::wrap`]: its segments in tracking space for a set of
/// per-joint curls, as [`Rig::pose`] will put them.
struct PosedChain<'a> {
    finger: &'a Finger,
    index: usize,
    splay: f32,
    /// The hand bone: bind space `x` goes to `origin + turn * ((x - bind_hand) * scale)`.
    origin: Vec3,
    turn: Quat,
    bind_hand: Vec3,
    scale: f32,
}

impl Rig {
    /// Finger `f` of hand `side` closed on `g`, the hand joint at `hand` (tracking space) drawn at
    /// `scale`: each joint's curl.
    fn wrap(&self, side: usize, f: usize, hand: (Vec3, Quat), scale: f32, g: &Grasp) -> [f32; 3] {
        let (Some(chain), Some(hb)) = (&self.fingers[side][f], self.joint_bone[HAND[side] as usize]) else { return [0.0; 3] };
        let posed = PosedChain {
            finger: chain,
            index: f,
            splay: WRAP_SPLAY,
            origin: hand.0,
            turn: hand.1 * self.inv_canonical[HAND[side] as usize],
            bind_hand: self.bones[hb].bind_pos,
            scale,
        };
        grasp::wrap(&posed, g, FINGER_THICKNESS * self.hand_size[side] * scale)
    }

    /// Each finger's joint curls (thumb .. little) hand `side` closes to on `g`, its hand joint at
    /// `hand` (the canonical frame, tracking space) and the body at `joints`: for tests.
    pub fn grasp_curls(&self, joints: &[Pose; JOINT_COUNT], side: usize, hand: Pose, g: &Grasp) -> [[f32; 3]; 5] {
        let scale = self.scale(joints);
        std::array::from_fn(|f| self.wrap(side, f, (Vec3::from_array(hand.pos), quat(hand.rot)), scale, g))
    }

    /// Each finger's segments (thumb .. little; (start, end) in tracking space, as many as it has)
    /// bent to `curls`, hand `side`'s joint at `hand` and the body at `joints`: for tests.
    pub fn finger_segments(&self, joints: &[Pose; JOINT_COUNT], side: usize, hand: Pose, curls: [[f32; 3]; 5]) -> [Vec<(Vec3, Vec3)>; 5] {
        use grasp::Chain;
        let scale = self.scale(joints);
        let hand = (Vec3::from_array(hand.pos), quat(hand.rot));
        std::array::from_fn(|f| {
            let (Some(chain), Some(hb)) = (&self.fingers[side][f], self.joint_bone[HAND[side] as usize]) else { return Vec::new() };
            let posed = PosedChain { finger: chain, index: f, splay: WRAP_SPLAY, origin: hand.0, turn: hand.1 * self.inv_canonical[HAND[side] as usize], bind_hand: self.bones[hb].bind_pos, scale };
            posed.place(curls[f])[..chain.count].to_vec()
        })
    }
}

impl grasp::Chain for PosedChain<'_> {
    fn segments(&self) -> usize {
        self.finger.count
    }

    fn place(&self, curls: [f32; 3]) -> [(Vec3, Vec3); 3] {
        let c = self.finger;
        let (mut rot, mut pos) = ([Quat::IDENTITY; 3], [Vec3::ZERO; 3]);
        c.bend(self.index, curls, self.splay, &mut rot, &mut pos);
        let at = |x: Vec3| self.origin + self.turn * ((x - self.bind_hand) * self.scale);
        let mut out = [(Vec3::ZERO, Vec3::ZERO); 3];
        for k in 0..c.count {
            let a = at(pos[k]);
            let b = if k + 1 < c.count { at(pos[k + 1]) } else { a + self.turn * (rot[k] * (c.canon[k] * Vec3::Y)) * (c.tip_len * self.scale) };
            out[k] = (a, b);
        }
        for k in c.count..3 {
            out[k] = (out[c.count - 1].1, out[c.count - 1].1);
        }
        out
    }

    fn lift(&self) -> f32 {
        LIFT[self.index]
    }
}

impl Finger {
    /// Bind-space rotation and pivot of each segment, every joint curled `curls[k]` (0 straight .. 1
    /// a fist's), the finger spread by `splay` (less as its base curls).
    fn bend(&self, finger: usize, curls: [f32; 3], splay: f32, rot: &mut [Quat; 3], pos: &mut [Vec3; 3]) {
        let splay = splay.clamp(0.0, 1.0) * (1.0 - curls[0].clamp(0.0, 1.0));
        let mut q = Quat::IDENTITY;
        for k in 0..self.count {
            // (The base may bend back: a finger pressed flat, a thumb lifted off.)
            let curl = curls[k].clamp(if k == 0 { -LIFT[finger] } else { 0.0 }, 1.0);
            let flex = if k == 0 && finger == 0 {
                curl * CURL[0][0]
            } else {
                curl * CURL[finger][k] - self.bind_flex[k]
            };
            let mut delta = Quat::from_axis_angle(self.flex_axis, flex);
            if k == 0 {
                let spread = if finger == 0 { (splay - 0.3) * SPREAD[0] } else { splay * SPREAD[finger] - self.bind_splay };
                delta = Quat::from_axis_angle(self.splay_axis, spread) * delta;
                pos[0] = self.pivots[0];
            } else {
                pos[k] = pos[k - 1] + q * (self.pivots[k] - self.pivots[k - 1]);
            }
            q *= delta;
            rot[k] = q;
        }
    }
}

/// Joints, in order of preference, that bones without a parent or a part ride with.
const ANCHORS: [Joint; 6] = [Joint::Hips, Joint::Spine, Joint::Chest, Joint::UpperChest, Joint::Neck, Joint::Head];
pub(crate) const HAND: [Joint; 2] = [Joint::LeftHand, Joint::RightHand];
const LOWER_ARM: [Joint; 2] = [Joint::LeftLowerArm, Joint::RightLowerArm];
const UPPER_ARM: [Joint; 2] = [Joint::LeftUpperArm, Joint::RightUpperArm];

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

/// What takes a limb segment's skin, drawn from `start` along `dir` for `length`, onto the line
/// from `start` to `end`: turned onto it, then scaled along it to its length, and its thickness by
/// the square root of that (a longer arm a little stronger, not lanky; nor as thick as it is long).
/// None when there's no line.
fn segment_fit(start: Vec3, dir: Vec3, end: Vec3, length: f32) -> Option<Mat4> {
    let to = end - start;
    let distance = to.length();
    let dir = dir.try_normalize()?;
    if distance < 1e-4 || length < 1e-4 || !distance.is_finite() {
        return None;
    }
    let d = to / distance;
    let k = (distance / length).clamp(0.5, 2.0);
    let t = k.sqrt().clamp(0.9, 1.2);
    // t across the bone, k along it.
    let along = Mat3::from_cols(Vec3::X * t + d * (d.x * (k - t)), Vec3::Y * t + d * (d.y * (k - t)), Vec3::Z * t + d * (d.z * (k - t)));
    let turn = Mat3::from_quat(Quat::from_rotation_arc(dir, d));
    Some(Mat4::from_translation(start) * Mat4::from_mat3(along * turn) * Mat4::from_translation(-start))
}

/// The angle of `q`'s twist about the unit `axis` (swing-twist decomposition), in -pi..pi.
pub fn twist_about(q: Quat, axis: Vec3) -> f32 {
    let along = Vec3::new(q.x, q.y, q.z).dot(axis);
    let a = 2.0 * along.atan2(q.w);
    if a > std::f32::consts::PI {
        a - std::f32::consts::TAU
    } else if a < -std::f32::consts::PI {
        a + std::f32::consts::TAU
    } else {
        a
    }
}

/// Signed angle from `u` to `v` about `axis`, measured in the plane across the axis.
fn signed_angle(u: Vec3, v: Vec3, axis: Vec3) -> f32 {
    let u = u - axis * u.dot(axis);
    let v = v - axis * v.dot(axis);
    axis.dot(u.cross(v)).atan2(u.dot(v))
}

/// A rotation with Y along `y` and Z as close to `z` as possible (`fallback` if `z` is along `y`).
fn frame(y: Vec3, z: Vec3, fallback: Vec3) -> Quat {
    let y = y.normalize_or(Vec3::Y);
    let mut zz = z - y * z.dot(y);
    if zz.length_squared() < 1e-6 {
        zz = fallback - y * fallback.dot(y);
    }
    let zz = zz.normalize_or(y.any_orthonormal_vector());
    Quat::from_mat3(&Mat3::from_cols(y.cross(zz), y, zz))
}

/// Where the skin weighted to `bone` sits in the bind pose (weighted mean), if any.
fn skin_centroid(meshes: &[&[asset::Vertex]], bone: usize) -> Option<Vec3> {
    let mut sum = Vec3::ZERO;
    let mut weight = 0.0;
    for v in meshes.iter().flat_map(|m| m.iter()) {
        for k in 0..4 {
            if v.bones[k] as usize == bone && v.weights[k] > 0 {
                let w = v.weights[k] as f32;
                sum += Vec3::from_array(v.pos) * w;
                weight += w;
            }
        }
    }
    (weight > 0.0).then(|| sum / weight)
}

/// The bind canonical frame of every mapped joint (identity where the model lacks the joint).
fn canonical_frames(map: &Mapping, tree: &Tree, pos: &dyn Fn(usize) -> Vec3) -> [Quat; JOINT_COUNT] {
    use Joint::*;
    let at = |j: Joint| map.joints[j as usize].map(pos);
    let up = Vec3::Y;
    // The body's right, from whichever left/right pair the model has.
    let right = [(LeftUpperLeg, RightUpperLeg), (LeftUpperArm, RightUpperArm), (LeftShoulder, RightShoulder), (LeftHand, RightHand)]
        .into_iter()
        .find_map(|(l, r)| Some(at(r)? - at(l)?))
        .map(|r| Vec3::new(r.x, 0.0, r.z))
        .unwrap_or(Vec3::X);
    let forward = right.cross(up).normalize_or(Vec3::Z);
    let first = |js: &[Joint]| js.iter().find_map(|&j| at(j));
    let level = |d: Vec3, fallback: Vec3| Vec3::new(d.x, 0.0, d.z).try_normalize().unwrap_or(fallback);

    let mut out = [Quat::IDENTITY; JOINT_COUNT];
    for j in Joint::ALL {
        let Some(p) = at(j) else { continue };
        let towards = |child: Option<Vec3>, fallback: Vec3| child.map(|c| c - p).filter(|d| d.length_squared() > 1e-8).unwrap_or(fallback);
        out[j as usize] = match j {
            Hips => frame(towards(first(&[Spine, Chest, UpperChest, Neck, Head]), up), forward, -up),
            Spine => frame(towards(first(&[Chest, UpperChest, Neck, Head]), up), forward, -up),
            Chest => frame(towards(first(&[UpperChest, Neck, Head]), up), forward, -up),
            UpperChest => frame(towards(first(&[Neck, Head]), up), forward, -up),
            Neck => frame(towards(at(Head), up), forward, -up),
            // The head has no child joint: its frame is the model's upright, facing forward.
            Head => frame(up, forward, -up),
            LeftShoulder | RightShoulder | LeftUpperArm | RightUpperArm | LeftLowerArm | RightLowerArm => {
                let child = Joint::ALL.into_iter().find(|c| c.parent() == Some(j)).and_then(at);
                frame(towards(child, if matches!(j, LeftShoulder | LeftUpperArm | LeftLowerArm) { -right } else { right }), forward, -up)
            }
            LeftHand | RightHand => {
                let side = if j == LeftHand { 0 } else { 1 };
                let base = |f: usize| map.fingers[side][f].first().map(|&b| pos(b));
                let lower = at(LOWER_ARM[side]).unwrap_or(p);
                let tip = base(2).or_else(|| [1, 3, 4].into_iter().find_map(base));
                let y = towards(tip, p - lower);
                let thumb_side = match (base(1), base(4)) {
                    (Some(i), Some(l)) => i - l,
                    _ => base(0).map(|t| t - p).unwrap_or(forward),
                };
                frame(y, thumb_side, forward)
            }
            LeftUpperLeg | RightUpperLeg | LeftLowerLeg | RightLowerLeg => {
                let child = Joint::ALL.into_iter().find(|c| c.parent() == Some(j)).and_then(at);
                frame(towards(child, -up), forward, up)
            }
            // Feet are level in the protocol's frames ("towards the toes" along the ground), so
            // the direction to the toes is flattened: a bind ankle sits higher than the ball of
            // the foot, and following that slope would lift the toes off the floor.
            LeftFoot | RightFoot => {
                let toes = if j == LeftFoot { LeftToes } else { RightToes };
                frame(level(towards(at(toes), forward), forward), up, forward)
            }
            LeftToes | RightToes => {
                // A tip bone under the toes, if the rig has one.
                let b = map.joints[j as usize].unwrap();
                let tip = (0..tree.parents.len()).find(|&c| tree.parents[c] == Some(b)).map(pos);
                frame(level(towards(tip, forward), forward), up, forward)
            }
        };
    }
    out
}

/// Bind-space data for curling one finger.
pub(crate) fn finger_chain(map: &Mapping, side: usize, f: usize, hand_frame: Quat, bind: &[(Quat, Vec3)]) -> Option<Finger> {
    let segs = &map.fingers[side][f];
    if segs.is_empty() {
        return None;
    }
    let mut pivots = [Vec3::ZERO; 3];
    for (k, &b) in segs.iter().enumerate() {
        pivots[k] = bind[b].1;
    }
    let count = segs.len();
    // Each segment's direction: to the next pivot, then to the tip bone if there is one, else the
    // last segment continues the way its bone's local axes say.
    let mut dirs = [Vec3::ZERO; 3];
    for k in 0..count {
        let next = if k + 1 < count { Some(pivots[k + 1]) } else { map.finger_tips[side][f].map(|t| bind[t].1) };
        dirs[k] = match next {
            Some(n) if n.distance_squared(pivots[k]) > 1e-10 => (n - pivots[k]).normalize(),
            _ if k > 0 => {
                let along = bind[segs[k - 1]].0.inverse() * dirs[k - 1];
                (bind[segs[k]].0 * along).normalize()
            }
            _ => hand_frame * Vec3::Y,
        };
    }
    let hand_y = hand_frame * Vec3::Y;
    let hand_x = hand_frame * Vec3::X;
    let hand_z = hand_frame * Vec3::Z;
    // The palm faces -X on the left hand and +X on the right (the protocol's hand frames).
    let palm = if side == 0 { -hand_x } else { hand_x };
    let flex_axis = if f == 0 {
        // The thumb's knuckles fold it across the palm, towards the little finger, more than into
        // the palm.
        dirs[0].cross((palm * 0.6 - hand_z).normalize()).normalize_or(hand_z)
    } else {
        dirs[0].cross(palm).normalize_or(hand_z)
    };
    let mut bind_flex = [0.0; 3];
    let mut canon = [Quat::IDENTITY; 3];
    for k in 0..count {
        let before = if k == 0 { hand_y } else { dirs[k - 1] };
        bind_flex[k] = signed_angle(before, dirs[k], flex_axis);
        canon[k] = segment_frame(f, dirs[k], hand_frame);
    }
    // The fingertip, until the skin says (see `skin_tip`): a little short of the segment before.
    let tip_len = if count > 1 { 0.8 * pivots[count - 1].distance(pivots[count - 2]) } else { 0.02 };
    Some(Finger { count, pivots, canon, tip_len, flex_axis, bind_flex, splay_axis: hand_x, bind_splay: signed_angle(hand_y, dirs[0], hand_x) })
}

/// How far the skin carried by `bone` (mostly) reaches from `pivot` along `dir`: a fingertip.
fn skin_tip(meshes: &[&[asset::Vertex]], bone: usize, pivot: Vec3, dir: Vec3) -> Option<f32> {
    let mut far: Option<f32> = None;
    for v in meshes.iter().flat_map(|m| m.iter()) {
        let total: u32 = v.weights.iter().map(|&w| w as u32).sum();
        let on: u32 = (0..4).filter(|&k| v.bones[k] as usize == bone).map(|k| v.weights[k] as u32).sum();
        if total > 0 && on * 2 > total {
            let d = (Vec3::from_array(v.pos) - pivot).dot(dir);
            far = Some(far.map_or(d, |f| f.max(d)));
        }
    }
    far.filter(|&d| d > 1e-3)
}

/// Each finger chain's fingertip from the skin, where the skin has one.
fn tips_from_skin(fingers: &mut [[Option<Finger>; 5]; 2], map: &Mapping, meshes: &[&[asset::Vertex]]) {
    for side in 0..2 {
        for f in 0..5 {
            let Some(c) = fingers[side][f].as_mut() else { continue };
            let last = c.count - 1;
            if let Some(t) = skin_tip(meshes, map.fingers[side][f][last], c.pivots[last], c.canon[last] * Vec3::Y) {
                c.tip_len = t;
            }
        }
    }
}

/// A finger segment's canonical frame (`rustvr_protocol::HandPoseDesc`'s convention) for a
/// segment running along `dir` on a hand whose canonical frame is `hand`: +Y along the segment;
/// for the four fingers +Z as near the hand's thumb side (their bending axis) as the segment
/// allows, for the thumb +X as near the hand's +X. A flat hand's segments are the hand's frame.
pub(crate) fn segment_frame(finger: usize, dir: Vec3, hand: Quat) -> Quat {
    if finger == 0 {
        let x = hand * Vec3::X;
        frame(dir, x.cross(dir), hand * Vec3::Z)
    } else {
        frame(dir, hand * Vec3::Z, hand * Vec3::Y)
    }
}

/// The most a posed finger's joints turn to reach their point (radians, each joint, in all), and
/// the most its knuckle spreads.
const REACH_TURN: f32 = 0.6;
const REACH_SPREAD: f32 = 0.25;

/// Bends a posed finger (segment `frames` and `pivots`, tracking space) so the point `along`
/// metres out along its last segment comes to `target`: cyclic coordinate descent, each joint
/// about its own bending axis (the four fingers' +Z, the thumb's whichever way it needs) and the
/// knuckle also spreading about the palm's normal, every turn limited so the game's pose stays.
fn reach(side: usize, finger: usize, count: usize, frames: &mut [Quat; 3], pivots: &mut [Vec3; 3], along: f32, target: Vec3, hand: Quat) {
    let last = count - 1;
    let tip = |frames: &[Quat; 3], pivots: &[Vec3; 3]| pivots[last] + frames[last] * Vec3::Y * along;
    let mut turned = [0.0f32; 3];
    let mut spread = 0.0f32;
    // Turns segments k.. about `axis` through pivot k.
    let turn = |frames: &mut [Quat; 3], pivots: &mut [Vec3; 3], k: usize, r: Quat| {
        for j in k..count {
            frames[j] = (r * frames[j]).normalize();
            if j > k {
                pivots[j] = pivots[k] + r * (pivots[j] - pivots[k]);
            }
        }
    };
    let palm_normal = hand * Vec3::X * if side == 1 { 1.0 } else { -1.0 };
    for _ in 0..10 {
        if tip(frames, pivots).distance(target) < 2e-4 {
            break;
        }
        for k in (0..count).rev() {
            let (a, b) = (tip(frames, pivots) - pivots[k], target - pivots[k]);
            if finger == 0 {
                // The thumb: the shortest turn, however it's limited.
                let (Some(a), Some(b)) = (a.try_normalize(), b.try_normalize()) else { continue };
                let r = Quat::from_rotation_arc(a, b);
                let (axis, angle) = r.to_axis_angle();
                let room = (REACH_TURN - turned[k]).max(0.0);
                let angle = angle.min(room);
                turned[k] += angle;
                turn(frames, pivots, k, Quat::from_axis_angle(axis, angle));
            } else {
                let axis = frames[k] * Vec3::Z;
                let angle = signed_angle(a, b, axis);
                let angle = (turned[k] + angle).clamp(-REACH_TURN, REACH_TURN) - turned[k];
                turned[k] += angle;
                turn(frames, pivots, k, Quat::from_axis_angle(axis, angle));
                if k == 0 {
                    let (a, b) = (tip(frames, pivots) - pivots[0], target - pivots[0]);
                    let angle = signed_angle(a, b, palm_normal);
                    let angle = (spread + angle).clamp(-REACH_SPREAD, REACH_SPREAD) - spread;
                    spread += angle;
                    turn(frames, pivots, 0, Quat::from_axis_angle(palm_normal, angle));
                }
            }
        }
    }
}

/// Where on finger `f` of hand `side` the point `along` (a fraction of its last segment's reach,
/// 1 the fingertip) is, for the posed model `model` in the finger pose of `hand` (as
/// [`game_hand`] took it, perhaps refitted): in the hand's frame, metres. What a pose's `reach`
/// points are made of.
pub fn finger_reach(model: &asset::Model, hand: &GameHand, side: usize, f: usize, along: f32) -> Option<Vec3> {
    let skel = Skeleton::new(&model.bones);
    let hb = skel.map.joints[HAND[side] as usize]?;
    let h = skel.canonical[HAND[side] as usize];
    let mut chain = finger_chain(&skel.map, side, f, h, &skel.bind)?;
    let meshes: Vec<&[asset::Vertex]> = model.meshes.iter().map(|m| m.vertices.as_slice()).collect();
    let last = chain.count - 1;
    if let Some(t) = skin_tip(&meshes, skel.map.fingers[side][f][last], chain.pivots[last], chain.canon[last] * Vec3::Y) {
        chain.tip_len = t;
    }
    // In the hand's frame, the knuckle where the model has it and the segments as the pose turns
    // them.
    let mut p = h.inverse() * (chain.pivots[0] - skel.bind[hb].1);
    for k in 1..chain.count {
        p += hand.fingers[f * 3 + k - 1] * Vec3::Y * chain.pivots[k].distance(chain.pivots[k - 1]);
    }
    Some(p + hand.fingers[f * 3 + last] * Vec3::Y * (chain.tip_len * along))
}

/// How far finger `f`'s last segment reaches (to its fingertip) in `model`, metres.
pub fn tip_length(model: &asset::Model, side: usize, f: usize) -> Option<f32> {
    let skel = Skeleton::new(&model.bones);
    let chain = finger_chain(&skel.map, side, f, skel.canonical[HAND[side] as usize], &skel.bind)?;
    let meshes: Vec<&[asset::Vertex]> = model.meshes.iter().map(|m| m.vertices.as_slice()).collect();
    let last = chain.count - 1;
    skin_tip(&meshes, skel.map.fingers[side][f][last], chain.pivots[last], chain.canon[last] * Vec3::Y).or(Some(chain.tip_len))
}

/// Wrist to the base of the middle finger (or the nearest other finger) in the bind pose.
fn size_of_hand(map: &Mapping, side: usize, pos: &dyn Fn(usize) -> Vec3) -> f32 {
    let Some(hand) = map.joints[HAND[side] as usize] else { return 0.0 };
    [2, 1, 3, 4].into_iter().find_map(|f| map.fingers[side][f].first()).map_or(0.0, |&b| pos(b).distance(pos(hand)))
}

/// A hand's pose taken from a posed model (the first-person arms holding a gun, say): the hand
/// joint's canonical frame and every finger segment's canonical rotation relative to it, in the
/// model's own space. See [`game_hand`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameHand {
    /// Wrist position and the hand's canonical frame (model space).
    pub wrist: (Vec3, Quat),
    /// Finger segments relative to the hand frame: thumb, index, middle, ring, little, three each
    /// from the knuckle out. Segments the model lacks are the identity.
    pub fingers: [Quat; 15],
    /// Wrist to the middle finger's base, metres.
    pub size: f32,
}

/// One skeleton's mapping and bind data.
pub(crate) struct Skeleton {
    pub(crate) names: Vec<String>,
    pub(crate) map: Mapping,
    pub(crate) bind: Vec<(Quat, Vec3)>,
    pub(crate) canonical: [Quat; JOINT_COUNT],
}

impl Skeleton {
    pub(crate) fn new(bones: &[asset::Bone]) -> Skeleton {
        let n = bones.len();
        let parents: Vec<Option<usize>> =
            bones.iter().enumerate().map(|(i, b)| usize::try_from(b.parent).ok().filter(|&p| p < n && p != i)).collect();
        let tree = Tree::new(parents);
        let classes: Vec<Option<Class>> = bones.iter().map(|b| names::classify(&b.name)).collect();
        let map = Mapping::new(&tree, &classes);
        let bind: Vec<(Quat, Vec3)> = bones
            .iter()
            .map(|b| {
                let (_, r, t) = Mat4::from_cols_array(&b.model_matrix).to_scale_rotation_translation();
                (r.normalize(), t)
            })
            .collect();
        let canonical = canonical_frames(&map, &tree, &|b| bind[b].1);
        Skeleton { names: bones.iter().map(|b| b.name.clone()).collect(), map, bind, canonical }
    }
}

/// The joints that draw a model exactly in its bind pose (model space): each mapped joint at its
/// bone's bind position, turned to its canonical frame there. A first-person arms model baked in a
/// game's pose (relative to the game's camera) drawn with these, moved by the camera's pose, is
/// where the game drew it.
pub fn bind_joints(bones: &[asset::Bone]) -> [Pose; JOINT_COUNT] {
    let s = Skeleton::new(bones);
    std::array::from_fn(|j| match s.map.joints[j] {
        Some(b) => Pose { pos: s.bind[b].1.to_array(), valid: 1, rot: s.canonical[j].to_array() },
        None => Pose { pos: [0.0; 3], valid: 0, rot: [0.0, 0.0, 0.0, 1.0] },
    })
}

/// The pose of hand `side` (0 left, 1 right) in a posed model: a game's first-person arms as it
/// draws them holding something. Frames are found from the bones' positions the same way the
/// avatar finds its own (see the module docs and [`segment_frame`]).
///
/// `rest`, when given, is the same skeleton in its rest pose (the arms before the animation was
/// baked in): each finger segment's frame is then carried from the rest pose by its bone's own
/// rotation, which keeps each finger's roll exactly as the animation has it (a curled finger's
/// roll can't be read from positions alone). Bones are matched by name.
pub fn game_hand(posed: &[asset::Bone], rest: Option<&[asset::Bone]>, side: usize) -> Result<GameHand> {
    if posed.is_empty() {
        bail!("the model has no skeleton");
    }
    let p = Skeleton::new(posed);
    let Some(hb) = p.map.joints[HAND[side] as usize] else { bail!("no {} hand bone", ["left", "right"][side]) };
    let hand = p.canonical[HAND[side] as usize];
    let r = rest.filter(|r| !r.is_empty()).map(Skeleton::new);
    let mut fingers = [Quat::IDENTITY; 15];
    for f in 0..5 {
        let Some(chain) = finger_chain(&p.map, side, f, hand, &p.bind) else { continue };
        // The same finger in the rest pose, segment for segment by name.
        let rest_chain = r.as_ref().and_then(|r| {
            let same = p.map.fingers[side][f].iter().zip(&r.map.fingers[side][f]).all(|(&a, &b)| p.names[a] == r.names[b]);
            let rh = r.canonical[HAND[side] as usize];
            same.then(|| finger_chain(&r.map, side, f, rh, &r.bind)).flatten().map(|c| (r, c))
        });
        for k in 0..chain.count {
            let b = p.map.fingers[side][f][k];
            let frame_k = match &rest_chain {
                Some((r, rc)) if k < rc.count => {
                    let rb = r.map.fingers[side][f][k];
                    let carried = p.bind[b].0 * r.bind[rb].0.inverse() * rc.canon[k];
                    frame(chain.canon[k] * Vec3::Y, carried * Vec3::Z, carried * Vec3::X)
                }
                _ => chain.canon[k],
            };
            fingers[f * 3 + k] = (hand.inverse() * frame_k).normalize();
        }
    }
    Ok(GameHand { wrist: (p.bind[hb].1, hand), fingers, size: size_of_hand(&p.map, side, &|b| p.bind[b].1) })
}

/// Parent links with depth order and ancestor walks.
struct Tree {
    parents: Vec<Option<usize>>,
    depth: Vec<usize>,
    order: Vec<usize>,
}

impl Tree {
    fn new(parents: Vec<Option<usize>>) -> Tree {
        let n = parents.len();
        let mut depth = vec![0; n];
        for b in 0..n {
            // Bounded walk: a broken file with a parent loop can't hang us.
            let mut d = 0;
            let mut p = parents[b];
            while let Some(q) = p {
                d += 1;
                if d > n {
                    break;
                }
                p = parents[q];
            }
            depth[b] = d;
        }
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&b| depth[b]);
        Tree { parents, depth, order }
    }

    fn ancestors(&self, b: usize) -> impl Iterator<Item = usize> + '_ {
        let mut p = self.parents[b];
        let mut steps = 0;
        std::iter::from_fn(move || {
            let cur = p?;
            steps += 1;
            if steps > self.parents.len() {
                return None;
            }
            p = self.parents[cur];
            Some(cur)
        })
    }

    fn is_ancestor(&self, a: usize, b: usize) -> bool {
        self.ancestors(b).any(|x| x == a)
    }
}

/// Which bone plays which part.
pub(crate) struct Mapping {
    pub(crate) joints: [Option<usize>; JOINT_COUNT],
    /// Up to three curling segments per finger, base first.
    pub(crate) fingers: [[Vec<usize>; 5]; 2],
    /// A bone past the last segment (for its direction), if the rig has one.
    finger_tips: [[Option<usize>; 5]; 2],
    twists: Vec<usize>,
}

impl Mapping {
    fn new(tree: &Tree, classes: &[Option<Class>]) -> Mapping {
        let n = classes.len();
        let shallowest = |pred: &dyn Fn(usize) -> bool| (0..n).filter(|&b| pred(b)).min_by_key(|&b| (tree.depth[b], b));
        let is = |b: usize, c: Class| classes[b] == Some(c);
        let mut joints = [None; JOINT_COUNT];
        use Joint::*;

        let hips = shallowest(&|b| is(b, Class::Hips));
        let under = |root: Option<usize>, b: usize| root.is_none_or(|r| tree.is_ancestor(r, b));
        let neck = shallowest(&|b| is(b, Class::Neck) && under(hips, b));
        let head = shallowest(&|b| is(b, Class::Head) && under(neck.or(hips), b));
        joints[Hips as usize] = hips;
        joints[Neck as usize] = neck;
        joints[Head as usize] = head;

        // The spine is whatever lies between the hips and the neck.
        let chain: Vec<usize> = match (hips, neck) {
            (Some(h), Some(nk)) if tree.is_ancestor(h, nk) => {
                let mut c: Vec<usize> = tree.ancestors(nk).take_while(|&a| a != h).collect();
                c.reverse();
                c
            }
            _ => {
                let mut c: Vec<usize> = (0..n).filter(|&b| is(b, Class::Spine) && under(hips, b)).collect();
                c.sort_by_key(|&b| tree.depth[b]);
                c
            }
        };
        match chain.len() {
            0 => {}
            1 => joints[Chest as usize] = Some(chain[0]),
            2 => {
                joints[Spine as usize] = Some(chain[0]);
                joints[UpperChest as usize] = Some(chain[1]);
            }
            len => {
                joints[Spine as usize] = Some(chain[0]);
                joints[Chest as usize] = Some(chain[(len - 1) / 2]);
                joints[UpperChest as usize] = Some(chain[len - 1]);
            }
        }

        let twist = |b: usize| is(b, Class::Twist);
        let limb = |side: usize, l: Limb| move |b: usize| is(b, Class::Limb(side, l));
        // A named bone above `below`, or failing that the nearest non-twist bone above it.
        let above = |named: &dyn Fn(usize) -> bool, below: Option<usize>, guess: bool| -> Option<usize> {
            let below = below?;
            tree.ancestors(below).find(|&a| named(a)).or_else(|| if guess { tree.ancestors(below).find(|&a| !twist(a)) } else { None })
        };
        let arms = [[LeftShoulder, LeftUpperArm, LeftLowerArm, LeftHand], [RightShoulder, RightUpperArm, RightLowerArm, RightHand]];
        let legs = [[LeftUpperLeg, LeftLowerLeg, LeftFoot, LeftToes], [RightUpperLeg, RightLowerLeg, RightFoot, RightToes]];
        for side in 0..2 {
            let hand = shallowest(&|b| limb(side, Limb::Hand)(b));
            let lower = above(&limb(side, Limb::LowerArm), hand, true);
            let upper = above(&limb(side, Limb::UpperArm), lower, true);
            let shoulder = above(&limb(side, Limb::Shoulder), upper, false);
            for (j, b) in arms[side].into_iter().zip([shoulder, upper, lower, hand]) {
                joints[j as usize] = b;
            }
            let foot = shallowest(&|b| limb(side, Limb::Foot)(b));
            let lower_leg = above(&limb(side, Limb::LowerLeg), foot, true);
            let upper_leg = above(&limb(side, Limb::UpperLeg), lower_leg, true);
            let toes = foot.and_then(|f| shallowest(&|b| limb(side, Limb::Toes)(b) && tree.is_ancestor(f, b)));
            for (j, b) in legs[side].into_iter().zip([upper_leg, lower_leg, foot, toes]) {
                joints[j as usize] = b;
            }
        }
        // A guessed bone may not play two parts (e.g. a hand parented straight to the chest).
        for j in 0..JOINT_COUNT {
            if let Some(b) = joints[j]
                && joints[..j].contains(&Some(b))
            {
                joints[j] = None;
            }
        }

        let mut fingers: [[Vec<usize>; 5]; 2] = Default::default();
        let mut finger_tips = [[None; 5]; 2];
        for side in 0..2 {
            let Some(hand) = joints[HAND[side] as usize] else { continue };
            for f in 0..5 {
                let mut bones: Vec<usize> = (0..n)
                    .filter(|&b| classes[b] == Some(Class::Finger(side, f, false)) && tree.is_ancestor(hand, b))
                    .collect();
                bones.sort_by_key(|&b| (tree.depth[b], b));
                // Keep a single chain: each segment under the previous one.
                let mut chain: Vec<usize> = Vec::new();
                for b in bones {
                    if chain.last().is_none_or(|&prev| tree.is_ancestor(prev, b)) {
                        chain.push(b);
                    }
                }
                finger_tips[side][f] = chain.get(3).copied();
                chain.truncate(3);
                fingers[side][f] = chain;
            }
        }
        let twists = (0..n).filter(|&b| twist(b)).collect();
        Mapping { joints, fingers, finger_tips, twists }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose;

    /// A skeleton from (name, parent, position, rotation) rows.
    fn skeleton(rows: &[(&str, i32, Vec3, Quat)]) -> Vec<asset::Bone> {
        rows.iter()
            .map(|&(name, parent, p, r)| asset::Bone {
                name: name.into(),
                parent,
                model_matrix: Mat4::from_rotation_translation(r, p).to_cols_array(),
                ..Default::default()
            })
            .collect()
    }

    /// An A-pose Biped-style skeleton facing +Z with arbitrary (rig-specific) bone axes.
    fn a_pose_biped() -> Vec<asset::Bone> {
        let odd = Quat::from_euler(glam::EulerRot::XYZ, 0.3, -1.1, 0.7);
        let mut rows: Vec<(&str, i32, Vec3, Quat)> = vec![
            ("bip01", -1, Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY),
            ("bip01_pelvis", 0, Vec3::new(0.0, 1.0, 0.0), odd),
            ("bip01_spine", 1, Vec3::new(0.0, 1.1, 0.0), odd),
            ("bip01_spine1", 2, Vec3::new(0.0, 1.25, 0.01), odd),
            ("bip01_spine2", 3, Vec3::new(0.0, 1.4, 0.0), odd),
            ("bip01_neck", 4, Vec3::new(0.0, 1.55, -0.01), odd),
            ("bip01_head", 5, Vec3::new(0.0, 1.63, 0.0), odd),
        ];
        // Arms 40 degrees down from horizontal; hands palm down-ish with fingers along the arm.
        let down = 40f32.to_radians();
        let names = [
            ["bip01_l_clavicle", "bip01_l_upperarm", "bip01_l_forearm", "bip01_l_hand", "bip01_l_finger1", "bip01_l_finger4", "bip01_l_finger2", "bip01_l_finger0"],
            ["bip01_r_clavicle", "bip01_r_upperarm", "bip01_r_forearm", "bip01_r_hand", "bip01_r_finger1", "bip01_r_finger4", "bip01_r_finger2", "bip01_r_finger0"],
        ];
        for (side, x) in [(0usize, -1.0f32), (1, 1.0)] {
            let dir = Vec3::new(x * down.cos(), -down.sin(), 0.0);
            let n = &names[side];
            let base = rows.len() as i32;
            let shoulder = Vec3::new(x * 0.03, 1.5, 0.0);
            let upper = shoulder + Vec3::new(x * 0.15, 0.0, 0.0);
            let elbow = upper + dir * 0.3;
            let wrist = elbow + dir * 0.26;
            rows.push((n[0], 4, shoulder, odd));
            rows.push((n[1], base, upper, odd));
            rows.push((n[2], base + 1, elbow, odd));
            rows.push((n[3], base + 2, wrist, odd));
            // Knuckles: index forward (+Z, the thumb side), little behind.
            rows.push((n[4], base + 3, wrist + dir * 0.09 + Vec3::Z * 0.025, odd));
            rows.push((n[5], base + 3, wrist + dir * 0.08 - Vec3::Z * 0.025, odd));
            rows.push((n[6], base + 3, wrist + dir * 0.095, odd));
            rows.push((n[7], base + 3, wrist + dir * 0.02 + Vec3::Z * 0.02 + Vec3::new(0.0, -0.01, 0.0), odd));
        }
        for (side, x) in [(0usize, -1.0f32), (1, 1.0)] {
            let base = rows.len() as i32;
            let names = [["bip01_l_thigh", "bip01_l_calf", "bip01_l_foot", "bip01_l_toe0"], ["bip01_r_thigh", "bip01_r_calf", "bip01_r_foot", "bip01_r_toe0"]][side];
            rows.push((names[0], 1, Vec3::new(x * 0.1, 0.95, 0.0), odd));
            rows.push((names[1], base, Vec3::new(x * 0.1, 0.5, 0.01), odd));
            rows.push((names[2], base + 1, Vec3::new(x * 0.1, 0.08, -0.01), odd));
            rows.push((names[3], base + 2, Vec3::new(x * 0.1, 0.01, 0.13), odd));
        }
        skeleton(&rows)
    }

    #[test]
    fn canonical_frames_from_an_a_pose() {
        let bones = a_pose_biped();
        let rig = Rig::new(&bones, &[]).unwrap();
        let axes = |j: Joint| {
            let (p, q) = rig.canonical_bind(j).unwrap();
            (p, Mat3::from_quat(q))
        };
        for j in Joint::ALL {
            let (_, m) = axes(j);
            assert!((m.determinant() - 1.0).abs() < 1e-4, "{j:?} is a rotation");
            assert!(m.x_axis.abs_diff_eq(m.y_axis.cross(m.z_axis), 1e-4), "{j:?}: X = Y x Z");
        }
        // Limb bones point at their child joints; Z is forward (or the thumb side, or up).
        for (j, child) in [
            (Joint::LeftUpperArm, Joint::LeftLowerArm),
            (Joint::RightLowerArm, Joint::RightHand),
            (Joint::LeftShoulder, Joint::LeftUpperArm),
            (Joint::RightUpperLeg, Joint::RightLowerLeg),
            (Joint::Hips, Joint::Spine),
            (Joint::Neck, Joint::Head),
        ] {
            let (p, m) = axes(j);
            let (c, _) = axes(child);
            assert!(m.y_axis.dot((c - p).normalize()) > 0.9999, "{j:?} Y towards {child:?}");
        }
        for j in [Joint::Hips, Joint::Chest, Joint::LeftUpperArm, Joint::RightLowerArm, Joint::LeftUpperLeg, Joint::Head] {
            assert!(axes(j).1.z_axis.dot(Vec3::Z) > 0.99, "{j:?} Z forward");
        }
        // Feet are level: Y towards the toes along the ground, Z straight up.
        let (foot, m) = axes(Joint::LeftFoot);
        let to_toes = axes(Joint::LeftToes).0 - foot;
        assert!(m.y_axis.abs_diff_eq(Vec3::new(to_toes.x, 0.0, to_toes.z).normalize(), 1e-5));
        for j in [Joint::LeftFoot, Joint::RightToes] {
            assert!(axes(j).1.z_axis.abs_diff_eq(Vec3::Y, 1e-5), "{j:?} Z up");
        }
        for j in [Joint::LeftHand, Joint::RightHand] {
            let (_, m) = axes(j);
            assert!(m.z_axis.dot(Vec3::Z) > 0.99, "{j:?} Z towards the thumb side");
        }
        // An A-pose arm hangs: its canonical Y is 40 degrees below horizontal.
        assert!((axes(Joint::LeftUpperArm).1.y_axis.dot(-Vec3::Y) - 40f32.to_radians().sin()).abs() < 1e-4);
    }

    /// Joints that put every mapped bone exactly where it is in the bind pose, scaled by `k` and
    /// moved by `t`.
    fn bind_joints(rig: &Rig, k: f32, t: Mat4) -> [Pose; JOINT_COUNT] {
        let (_, tr, tt) = t.to_scale_rotation_translation();
        let mut joints = [Pose::default(); JOINT_COUNT];
        for j in Joint::ALL {
            if let Some((p, q)) = rig.canonical_bind(j) {
                joints[j as usize] = Pose { pos: (tr * (p * k) + tt).to_array(), valid: 1, rot: (tr * q).to_array() };
            }
        }
        joints
    }

    /// Every skinning matrix is `want`, except the thumbs': their spread is added to the bind
    /// pose around splay 0.3, so splay 0 moves them.
    fn assert_skin_is(rig: &Rig, posed: &Posed, want: Mat4) {
        for (b, m) in posed.skin.iter().enumerate() {
            if !matches!(rig.bones[b].drive, Drive::Finger { finger: 0, .. }) {
                assert!(m.abs_diff_eq(want, 1e-4), "bone {b} ({:?}): {m:?} vs {want:?}", rig.bones[b].drive);
            }
        }
    }

    #[test]
    fn identity_pose_reproduces_the_bind_pose_up_to_scale() {
        for bones in [a_pose_biped(), crate::procedural::mannequin().bones] {
            let rig = Rig::new(&bones, &[]).unwrap();
            let mut posed = rig.posed();
            for (k, t) in [(1.0, Mat4::IDENTITY), (0.8, Mat4::from_rotation_translation(Quat::from_rotation_y(0.7), Vec3::new(0.3, 0.0, -2.0)))] {
                let joints = bind_joints(&rig, k, t);
                // Finger bones are left out: curl 0 straightens them whatever the bind hand did.
                let fingers = [Fingers::default(); 2];
                rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held: [None, None], grasp: [None, None], chop_head: false }, &mut posed);
                assert!((posed.scale - k).abs() < 1e-4, "scale {}", posed.scale);
                let want = t * Mat4::from_scale(Vec3::splat(k));
                for (b, bone) in rig.bones.iter().enumerate() {
                    if !matches!(bone.drive, Drive::Finger { .. }) {
                        assert!(posed.skin[b].abs_diff_eq(want, 1e-4), "bone {b} ({:?})", bone.drive);
                    }
                }
            }
        }
    }

    #[test]
    fn an_arm_longer_than_the_models_stretches_evenly_never_at_the_joints() {
        // The mannequin in its bind pose, then its right hand moved 12 cm further out along the arm
        // (the player's arm is longer, or the hand's past where the arm reaches): the forearm's
        // skin spans elbow to hand, its wrist end on the hand and its elbow end on the elbow, and
        // a point half way along it half way between.
        let source = crate::procedural::mannequin();
        let rig = Rig::new(&source.bones, &[]).unwrap();
        let mut joints = bind_joints(&rig, 1.0, Mat4::IDENTITY);
        let (elbow, wrist) = (Vec3::from_array(joints[Joint::RightLowerArm as usize].pos), Vec3::from_array(joints[Joint::RightHand as usize].pos));
        let out = (wrist - elbow).normalize();
        let hand = wrist + out * 0.12;
        joints[Joint::RightHand as usize].pos = hand.to_array();
        let fingers = [Fingers::default(); 2];
        let mut posed = rig.posed();
        rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held: [None, None], grasp: [None, None], chop_head: false }, &mut posed);
        let fore = rig.joint_bone[Joint::RightLowerArm as usize].unwrap();
        let skin = |p: Vec3| posed.skin[fore].transform_point3(p);
        assert!(skin(elbow).distance(elbow) < 1e-4, "the elbow end stays on the elbow: {:?}", skin(elbow));
        assert!(skin(wrist).distance(hand) < 1e-4, "the wrist end is on the hand: {:?} for {hand:?}", skin(wrist));
        let mid = skin((elbow + wrist) * 0.5);
        assert!(mid.distance((elbow + hand) * 0.5) < 1e-4, "stretched evenly: {mid:?}");
        // A little stronger, as the square root of how much longer (at most a fifth).
        let side = out.any_orthonormal_vector() * 0.03;
        let across = skin(elbow + side).distance(elbow) / 0.03;
        let k = elbow.distance(hand) / elbow.distance(wrist);
        assert!((across - k.sqrt().clamp(0.9, 1.2)).abs() < 1e-3, "{across} for {k}");
        // The body's size is its trunk's, and the upper arm, its joints where they were, is as it
        // was.
        assert!((posed.scale - 1.0).abs() < 1e-4, "scale {}", posed.scale);
        let upper = rig.joint_bone[Joint::RightUpperArm as usize].unwrap();
        assert!(posed.skin[upper].abs_diff_eq(Mat4::IDENTITY, 1e-4));
    }

    #[test]
    fn straight_fingers_reproduce_a_straight_bind_hand() {
        // The mannequin's fingers are straight in its bind pose, so curl 0 changes nothing.
        let source = crate::procedural::mannequin();
        let rig = Rig::new(&source.bones, &[]).unwrap();
        let mut posed = rig.posed();
        let joints = bind_joints(&rig, 1.0, Mat4::IDENTITY);
        let fingers = [Fingers { splay: 0.0, ..Default::default() }; 2];
        rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held: [None, None], grasp: [None, None], chop_head: false }, &mut posed);
        assert_skin_is(&rig, &posed, Mat4::IDENTITY);
    }

    #[test]
    fn the_reference_pose_drives_the_mannequin_to_its_bind_pose() {
        // The mannequin is bound in the reference pose of a 1.75 m person.
        let source = crate::procedural::mannequin();
        let rig = Rig::new(&source.bones, &[]).unwrap();
        let mut posed = rig.posed();
        let joints = pose::t_pose(1.75);
        let fingers = [Fingers::default(); 2];
        rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held: [None, None], grasp: [None, None], chop_head: false }, &mut posed);
        assert_skin_is(&rig, &posed, Mat4::IDENTITY);
    }

    #[test]
    fn a_hand_going_into_a_held_pose_blends_from_its_own() {
        let source = crate::procedural::mannequin();
        let rig = Rig::new(&source.bones, &[]).unwrap();
        let joints = pose::t_pose(1.75);
        let fingers = [Fingers::default(); 2];
        let pose_with = |held: [Option<PosedFingers>; 2]| {
            let mut posed = rig.posed();
            rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held, grasp: [None, None], chop_head: false }, &mut posed);
            posed
        };
        // A fist the game holds the right hand in.
        let fist = PosedFingers { rel: [Quat::from_rotation_z(-1.2); 15], size: 0.0, reach: [None; 5], weight: 1.0 };
        let own = pose_with([None, None]);
        let full = pose_with([None, Some(fist)]);
        let none = pose_with([None, Some(PosedFingers { weight: 0.0, ..fist })]);
        let half = pose_with([None, Some(PosedFingers { weight: 0.5, ..fist })]);
        let hand: Vec<usize> = (0..rig.bones.len()).filter(|&b| rig.bones[b].part == Part::Hand(1)).collect();
        let moved = |a: &Posed, b: &Posed| hand.iter().map(|&k| a.rot[k].angle_between(b.rot[k])).fold(0.0f32, f32::max);
        assert!(moved(&own, &full) > 0.5, "the held pose differs from the hand's own");
        assert!(moved(&own, &none) < 1e-4, "at weight 0 the hand is its own");
        let (to_own, to_full) = (moved(&half, &own), moved(&half, &full));
        assert!(to_own > 0.2 && to_full > 0.2, "half way between: {to_own} {to_full}");
    }

    #[test]
    fn fingers_curl_into_the_palm() {
        let source = crate::procedural::mannequin();
        let rig = Rig::new(&source.bones, &[]).unwrap();
        let mut posed = rig.posed();
        let joints = pose::t_pose(1.75);
        for side in 0..2 {
            let hand = rig.joint_bone[HAND[side] as usize].unwrap();
            // Palms face down in the reference pose.
            let palm = -Vec3::Y;
            for f in 0..5 {
                let last = rig.fingers[side][f].as_ref().unwrap().count - 1;
                let tip = (0..rig.bones.len())
                    .find(|&b| matches!(rig.bones[b].drive, Drive::Finger { side: s, finger, segment } if s == side && finger == f && segment == last))
                    .unwrap();
                let mut at = |curl: f32| {
                    let mut fingers = [Fingers::default(); 2];
                    fingers[side].curl[f] = curl;
                    rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held: [None, None], grasp: [None, None], chop_head: false }, &mut posed);
                    posed.pos[tip] - posed.pos[hand]
                };
                let (open, closed) = (at(0.0), at(1.0));
                assert!(closed.dot(palm) > open.dot(palm) + 0.01, "side {side} finger {f}: towards the palm");
                assert!(closed.length() < open.length(), "side {side} finger {f}: tip comes back towards the wrist");
            }
        }
    }

    #[test]
    fn fingers_close_round_a_held_handle_and_rest_on_it() {
        use crate::grasp::{ALL_FINGERS, Solid};
        let source = crate::procedural::mannequin();
        let rig = Rig::new(&source.bones, &[]).unwrap();
        let joints = pose::t_pose(1.75);
        let fingers = [Fingers::default(); 2];
        let side = 1;
        let bone = |f: usize, k: usize| {
            (0..rig.bones.len()).find(|&b| matches!(rig.bones[b].drive, Drive::Finger { side: s, finger, segment } if s == side && finger == f && segment == k)).unwrap()
        };
        let pose_with = |grasp: Option<&Grasp>| {
            let mut posed = rig.posed();
            rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held: [None, None], grasp: [None, grasp], chop_head: false }, &mut posed);
            posed
        };
        let open = pose_with(None);
        // A 3 cm handle across the hand, under the middle of the middle finger's first segment, its
        // axis 2.5 cm below it (palms face down in the reference pose).
        let palm = -Vec3::Y;
        let (base, mid) = (open.pos[bone(2, 0)], open.pos[bone(2, 1)]);
        let across = (mid - base).normalize().cross(palm).normalize();
        let c = base.lerp(mid, 0.5) + palm * 0.025;
        let g = Grasp { solids: vec![Solid::Capsule { a: c - across * 0.12, b: c + across * 0.12, radius: 0.015 }], fingers: ALL_FINGERS, weight: 1.0, ..Default::default() };
        let held = pose_with(Some(&g));
        for f in 1..5 {
            let count = rig.fingers[side][f].as_ref().unwrap().count;
            let last = bone(f, count - 1);
            // The longer fingers reach past it and close round it (the little finger's middle
            // segment is on it already: only its tip comes round).
            if f < 4 {
                assert!(held.pos[last].dot(palm) > open.pos[last].dot(palm) + 0.015, "finger {f} closes round it");
            }
            // Every joint on it, none in it.
            let d: Vec<f32> = (1..count).map(|k| g.distance(held.pos[bone(f, k)])).collect();
            assert!(d.iter().all(|&d| d > 0.004) && d.iter().any(|&d| d < 0.02), "finger {f}'s joints rest on the handle: {d:?}");
        }
        // Half way in: half way between.
        let half = pose_with(Some(&Grasp { weight: 0.5, ..g.clone() }));
        let tip = |p: &Posed| p.pos[bone(2, 2)].dot(palm);
        assert!(tip(&half) > tip(&open) + 0.005 && tip(&half) < tip(&held) - 0.005);
    }

    /// `rest` with an extra rotation (in each bone's own frame, from `delta` by name) on some
    /// bones, everything below following: the skeleton posed the way an animation poses it.
    fn posed_bones(rest: &[asset::Bone], delta: &dyn Fn(&str) -> Quat) -> Vec<asset::Bone> {
        let world: Vec<Mat4> = rest.iter().map(|b| Mat4::from_cols_array(&b.model_matrix)).collect();
        let n = rest.len();
        let tree = Tree::new(rest.iter().map(|b| usize::try_from(b.parent).ok().filter(|&p| p < n)).collect());
        let mut posed = world.clone();
        for &b in &tree.order {
            let (base, local) = match tree.parents[b] {
                Some(p) => (posed[p], world[p].inverse() * world[b]),
                None => (Mat4::IDENTITY, world[b]),
            };
            posed[b] = base * local * Mat4::from_quat(delta(&rest[b].name));
        }
        rest.iter().zip(posed).map(|(b, m)| asset::Bone { model_matrix: m.to_cols_array(), ..b.clone() }).collect()
    }

    /// A grip: fingers bent at every joint and rolled a little, the thumb across, the hand turned.
    fn gripping(name: &str) -> Quat {
        let n = name.to_ascii_lowercase();
        if n.ends_with("hand") {
            Quat::from_euler(glam::EulerRot::XYZ, 0.3, -0.5, 0.2)
        } else if n.contains("thumb") {
            Quat::from_euler(glam::EulerRot::XYZ, 0.25, 0.4, -0.3)
        } else if ["index", "middle", "ring", "little"].iter().any(|f| n.contains(f)) {
            Quat::from_euler(glam::EulerRot::XYZ, 0.9, 0.15, -0.1)
        } else {
            Quat::IDENTITY
        }
    }

    #[test]
    fn a_game_hand_pose_is_drawn_exactly() {
        let rest = crate::procedural::mannequin().bones;
        let rig = Rig::new(&rest, &[]).unwrap();
        let posed_model = posed_bones(&rest, &gripping);
        let world = |b: &asset::Bone| Mat4::from_cols_array(&b.model_matrix);
        let mut posed = rig.posed();
        for side in 0..2 {
            for carried in [true, false] {
                let g = game_hand(&posed_model, carried.then_some(rest.as_slice()), side).unwrap();
                assert!((g.size - rig.hand_size()[side]).abs() < 1e-5, "the same hand, the same size");
                let joints = bind_joints(&rig, 1.0, Mat4::IDENTITY);
                let mut hands = [None, None];
                hands[side] = Some(Pose { pos: g.wrist.0.to_array(), valid: 1, rot: g.wrist.1.to_array() });
                let mut held = [None, None];
                held[side] = Some(PosedFingers { rel: g.fingers, size: g.size, reach: [None; 5], weight: 1.0 });
                let fingers = [Fingers::default(); 2];
                rig.pose(&PoseInput { joints: &joints, hands, fingers: &fingers, held, grasp: [None, None], chop_head: false }, &mut posed);
                // Every bone of that hand lands exactly where the posed model has it (carried
                // through the rest pose, the fingers' roll too).
                for b in (0..rig.bones.len()).filter(|&b| rig.bones[b].part == Part::Hand(side)) {
                    let (_, wr, wp) = world(&posed_model[b]).to_scale_rotation_translation();
                    assert!(posed.pos[b].distance(wp) < 1e-5, "side {side} bone {} at {}, posed {wp}", rest[b].name, posed.pos[b]);
                    let err = posed.rot[b].angle_between(wr).to_degrees();
                    // From positions alone (not carried) a segment's direction is exact, but its roll
                    // is a guess: only the positions are checked then.
                    if carried || bone_is_hand(&rig, side, b) {
                        // (f32 quaternion angles are only good to a few hundredths of a degree.)
                        assert!(err < 0.1, "side {side} bone {}: {err}° off (carried {carried})", rest[b].name);
                    }
                }
            }
        }
    }

    fn bone_is_hand(rig: &Rig, side: usize, b: usize) -> bool {
        rig.joint_bone[HAND[side] as usize] == Some(b)
    }

    #[test]
    fn a_game_hand_is_drawn_at_its_size() {
        let rest = crate::procedural::mannequin().bones;
        let rig = Rig::new(&rest, &[]).unwrap();
        let posed_model = posed_bones(&rest, &gripping);
        let g = game_hand(&posed_model, Some(&rest), 1).unwrap();
        let joints = bind_joints(&rig, 1.0, Mat4::IDENTITY);
        let fingers = [Fingers::default(); 2];
        let hand = rig.joint_bone[HAND[1] as usize].unwrap();
        let tip = |k: f32, posed: &mut Posed| {
            let held = [None, Some(PosedFingers { rel: g.fingers, size: g.size * k, reach: [None; 5], weight: 1.0 })];
            let hands = [None, Some(Pose { pos: g.wrist.0.to_array(), valid: 1, rot: g.wrist.1.to_array() })];
            rig.pose(&PoseInput { joints: &joints, hands, fingers: &fingers, held, grasp: [None, None], chop_head: false }, posed);
            let index = rig.fingers[1][1].as_ref().unwrap().count - 1;
            let b = (0..rig.bones.len()).find(|&b| matches!(rig.bones[b].drive, Drive::Finger { side: 1, finger: 1, segment } if segment == index)).unwrap();
            (posed.pos[b] - posed.pos[hand], posed.skin[b].transform_vector3(Vec3::X).length())
        };
        let mut posed = rig.posed();
        let (one, s1) = tip(1.0, &mut posed);
        let (bigger, s2) = tip(1.25, &mut posed);
        assert!(bigger.abs_diff_eq(one * 1.25, 1e-5), "the hand grows about the wrist: {one} -> {bigger}");
        assert!((s1 - 1.0).abs() < 1e-5 && (s2 - 1.25).abs() < 1e-5, "and its skin with it");
        // The forearm keeps the body's size.
        let forearm = rig.joint_bone[LOWER_ARM[1] as usize].unwrap();
        assert!((posed.skin[forearm].transform_vector3(Vec3::X).length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_head_collapses_for_the_wearer_but_not_the_neck() {
        let bones = a_pose_biped();
        let rig = Rig::new(&bones, &[]).unwrap();
        let mut posed = rig.posed();
        let joints = bind_joints(&rig, 1.0, Mat4::IDENTITY);
        let fingers = [Fingers::default(); 2];
        rig.pose(&PoseInput { joints: &joints, hands: [None, None], fingers: &fingers, held: [None, None], grasp: [None, None], chop_head: true }, &mut posed);
        let head = rig.joint_bone[Joint::Head as usize].unwrap();
        let neck = rig.joint_bone[Joint::Neck as usize].unwrap();
        let p = posed.skin[head].transform_point3(Vec3::new(0.0, 1.8, 0.1));
        assert!(p.abs_diff_eq(posed.pos[head], 1e-5), "head skin lands on the head pivot");
        assert!(posed.skin[neck].abs_diff_eq(Mat4::IDENTITY, 1e-4), "the neck stays");
    }

    #[test]
    fn hands_view_cuts_blended_skin_at_the_wrist() {
        let rig = Rig::new(&a_pose_biped(), &[]).unwrap();
        let planes = rig.visibility(Show::Hands);
        let bone = |j: Joint| rig.joint_bone[j as usize].unwrap();
        let (elbow, wrist) = (rig.bones[bone(Joint::RightLowerArm)].bind_pos, rig.bones[bone(Joint::RightHand)].bind_pos);
        let vis = |p: Vec3, weights: &[(usize, f32)]| -> f32 {
            weights.iter().map(|&(b, w)| w * (Vec3::from_slice(&planes[b][..3]).dot(p) + planes[b][3])).sum()
        };
        // Skin halfway down the forearm, half on the hand bone (as some rigs twist it), is hidden;
        // the same blend just past the cut near the wrist is shown.
        let blend = [(bone(Joint::RightLowerArm), 0.5), (bone(Joint::RightHand), 0.5)];
        assert!(vis(elbow.lerp(wrist, 0.5), &blend) < 0.0);
        assert!(vis(elbow.lerp(wrist, 0.9), &blend) > 0.0);
        assert!(vis(wrist, &[(bone(Joint::RightHand), 1.0)]) > 0.0);
        let left_wrist = rig.bones[bone(Joint::LeftHand)].bind_pos;
        assert!(vis(left_wrist, &[(bone(Joint::LeftHand), 1.0)]) > 0.0, "both hands");
        assert!(vis(elbow, &[(bone(Joint::Chest), 1.0)]) < 0.0, "not the body");
    }

    #[test]
    fn twist_decomposition() {
        let axis = Vec3::Y;
        // A swing about an axis across the twist axis doesn't change the twist, on either side.
        let swing = Quat::from_axis_angle(Vec3::new(1.0, 0.0, 0.5).normalize(), 0.6);
        for angle in [-2.5f32, -0.4, 0.0, 0.9, 3.0] {
            let twist = Quat::from_axis_angle(axis, angle);
            assert!((twist_about(twist, axis) - angle).abs() < 1e-5);
            assert!((twist_about(-twist, axis) - angle).abs() < 1e-5, "sign of the quaternion doesn't matter");
            assert!((twist_about(swing * twist, axis) - angle).abs() < 1e-4);
            assert!((twist_about(twist * swing, axis) - angle).abs() < 1e-4);
        }
    }

    #[test]
    fn maps_a_mixamo_skeleton() {
        let names = [
            ("mixamorig:Hips", -1),
            ("mixamorig:Spine", 0),
            ("mixamorig:Spine1", 1),
            ("mixamorig:Spine2", 2),
            ("mixamorig:Neck", 3),
            ("mixamorig:Head", 4),
            ("mixamorig:HeadTop_End", 5),
            ("mixamorig:LeftShoulder", 3),
            ("mixamorig:LeftArm", 7),
            ("mixamorig:LeftForeArm", 8),
            ("mixamorig:LeftHand", 9),
            ("mixamorig:LeftHandIndex1", 10),
            ("mixamorig:LeftHandIndex2", 11),
            ("mixamorig:LeftHandIndex3", 12),
            ("mixamorig:LeftHandIndex4", 13),
            ("mixamorig:RightShoulder", 3),
            ("mixamorig:RightArm", 15),
            ("mixamorig:RightForeArm", 16),
            ("mixamorig:RightHand", 17),
            ("mixamorig:LeftUpLeg", 0),
            ("mixamorig:LeftLeg", 19),
            ("mixamorig:LeftFoot", 20),
            ("mixamorig:LeftToeBase", 21),
        ];
        let parents: Vec<Option<usize>> = names.iter().map(|&(_, p)| usize::try_from(p).ok()).collect();
        let classes: Vec<Option<Class>> = names.iter().map(|(n, _)| names::classify(n)).collect();
        let tree = Tree::new(parents);
        let map = Mapping::new(&tree, &classes);
        let want = [
            (Joint::Hips, 0),
            (Joint::Spine, 1),
            (Joint::Chest, 2),
            (Joint::UpperChest, 3),
            (Joint::Neck, 4),
            (Joint::Head, 5),
            (Joint::LeftShoulder, 7),
            (Joint::LeftUpperArm, 8),
            (Joint::LeftLowerArm, 9),
            (Joint::LeftHand, 10),
            (Joint::RightHand, 18),
            (Joint::LeftUpperLeg, 19),
            (Joint::LeftLowerLeg, 20),
            (Joint::LeftFoot, 21),
            (Joint::LeftToes, 22),
        ];
        for (j, b) in want {
            assert_eq!(map.joints[j as usize], Some(b), "{j:?}");
        }
        assert_eq!(map.fingers[0][1], vec![11, 12, 13]);
        assert_eq!(map.finger_tips[0][1], Some(14));
        assert_eq!(map.joints[Joint::RightUpperLeg as usize], None);
    }
}
