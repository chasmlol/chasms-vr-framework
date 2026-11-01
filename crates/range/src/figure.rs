//! A game's own figure on one of the range's people (the duel ring's duelist): Oblivion's male body
//! in its iron armour, from the user's own extracted files in `local_assets/oblivion/` (never
//! committed; without them the person is drawn as its parts). It's put together as the runtime puts
//! the player's body together (`rustvr_avatar::compose`: the game's skeleton and the parts worn on
//! it), posed every frame from the person's thirteen physical parts through the avatar's
//! retargeting (`rustvr_avatar::rig`: each part's place and turn as the protocol's joints), skinned
//! on the CPU by the game's own weights, and drawn by the range's renderer like the rest of the room
//! (lit and in its depth). Its animation is ours: whatever the ragdoll does (its keyframes, its hits,
//! its falls), the figure does.

use std::path::Path;

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec3};
use rustvr_asset as asset;
use rustvr_avatar::rig::{PoseInput, Posed, Rig};
use rustvr_protocol::{Fingers, JOINT_COUNT, Joint, Pose};

use crate::anim;
use crate::content::{MeshData, Vertex};
use crate::ragdoll::Dummies;

/// The figure, by its files' names in the game: the skeleton, and what's worn on it (the head bare).
const SKELETON: &str = r"characters\_male\skeleton.nif";
const WORN: [&str; 7] = [
    r"characters\imperial\headhuman.nif",
    r"characters\imperial\eyelefthuman.nif",
    r"characters\imperial\eyerighthuman.nif",
    r"armor\iron\m\cuirass.nif",
    r"armor\iron\m\gauntlets.nif",
    r"armor\iron\m\greaves.nif",
    r"armor\iron\m\boots.nif",
];

pub struct Figure {
    model: asset::Model,
    rig: Rig,
    posed: Posed,
    /// Each mesh's vertices posed this frame (the renderer's), reused.
    skinned: Vec<Vec<Vertex>>,
}

impl Figure {
    /// The figure from the game's extracted files in `dir` (`local_assets/oblivion`), and its meshes
    /// as the renderer takes them (at rest; their textures as `oblivion/<key>`).
    pub fn load(dir: &Path) -> Result<(Figure, Vec<MeshData>)> {
        let index = asset::load_index(dir).with_context(|| format!("the asset index in {}", dir.display()))?;
        let key = |name: &str| index.items.iter().find(|i| i.name.eq_ignore_ascii_case(name)).map(|i| i.key.clone());
        let skeleton = key(SKELETON).with_context(|| format!("{SKELETON} isn't extracted"))?;
        let parts: Vec<String> = WORN.iter().filter_map(|n| key(n)).collect();
        anyhow::ensure!(!parts.is_empty(), "none of the figure's parts are extracted");
        let source = rustvr_avatar::compose(dir, &skeleton, &parts, &[], None, rustvr_avatar::Skinning::Linear)?;
        let rustvr_avatar::AvatarSource::Composed { model, .. } = source else { anyhow::bail!("the figure didn't compose") };
        let rig = Rig::new(&model.bones, &model.meshes.iter().map(|m| m.vertices.as_slice()).collect::<Vec<_>>())?;
        let posed = rig.posed();
        let meshes: Vec<MeshData> = model
            .meshes
            .iter()
            .map(|m| MeshData {
                vertices: m.vertices.iter().map(|v| Vertex { pos: v.pos, normal: v.normal, uv: v.uv, color: [255; 4] }).collect(),
                indices: m.indices.clone(),
                texture: (!m.texture.is_empty()).then(|| format!("oblivion/{}", m.texture)),
                source: None,
            })
            .collect();
        let skinned = meshes.iter().map(|m| m.vertices.clone()).collect();
        tracing::info!("range: the duelist's figure: {} meshes, {} vertices, from {} parts", meshes.len(), meshes.iter().map(|m| m.vertices.len()).sum::<usize>(), parts.len());
        Ok((Figure { model, rig, posed, skinned }, meshes))
    }

    /// The figure posed as person `k` stands now: each mesh's vertices (world).
    pub fn pose(&mut self, d: &Dummies, k: usize) -> &[Vec<Vertex>] {
        let joints = joints(d, k);
        let fingers: [Fingers; 2] = [rustvr_avatar::pose::relaxed_hand(), rustvr_avatar::pose::fist()];
        let input = PoseInput { joints: &joints, hands: [None; 2], fingers: &fingers, held: [None; 2], grasp: [None; 2], chop_head: false };
        self.rig.pose(&input, &mut self.posed);
        for (mesh, out) in self.model.meshes.iter().zip(&mut self.skinned) {
            for (v, o) in mesh.vertices.iter().zip(out.iter_mut()) {
                let mut m = Mat4::ZERO;
                let mut total = 0.0;
                for i in 0..4 {
                    let w = v.weights[i] as f32 / 255.0;
                    if w > 0.0 && let Some(s) = self.posed.skin.get(v.bones[i] as usize) {
                        m += *s * w;
                        total += w;
                    }
                }
                // (A vertex on no bone rides the first.)
                let m = if total > 0.0 { m * (1.0 / total) } else { self.posed.skin.first().copied().unwrap_or(Mat4::IDENTITY) };
                o.pos = m.transform_point3(Vec3::from_array(v.pos)).to_array();
                o.normal = m.transform_vector3(Vec3::from_array(v.normal)).normalize_or_zero().to_array();
            }
        }
        &self.skinned
    }
}

/// Where a joint is on the range's body at rest (its own space, standing, facing +Z), the part
/// that carries it, and its canonical frame there (the protocol's: the spine's Y up and Z forward;
/// a limb's Y along it and Z forward, the thumb's side for a hand; a foot's Y forward and Z up).
fn layout(j: Joint) -> (usize, Vec3, Quat) {
    use rustvr_avatar::pose::frame;
    let (y, z) = (Vec3::Y, Vec3::Z);
    let limb = |from: Vec3, to: Vec3| frame(to - from, z);
    let side = |s: usize| if s == 0 { -1.0 } else { 1.0 };
    let arm = |s: usize, which: usize| -> (usize, Vec3, Quat) {
        let (sh, el, wr) = (anim::shoulder(s), anim::elbow(s), anim::wrist(s));
        match which {
            0 => (anim::TORSO, Vec3::new(0.03 * side(s), sh.y, 0.0), frame(Vec3::X * side(s), z)),
            1 => (anim::UPPER_ARM[s], sh, limb(sh, el)),
            2 => (anim::FOREARM[s], el, limb(el, wr)),
            _ => (anim::HAND[s], wr, frame(-y, z)),
        }
    };
    let leg = |s: usize, which: usize| -> (usize, Vec3, Quat) {
        let (hip, knee) = (anim::hip(s), anim::knee(s));
        match which {
            0 => (anim::THIGH[s], hip, frame(-y, z)),
            1 => (anim::SHIN[s], knee, frame(-y, z)),
            2 => (anim::SHIN[s], Vec3::new(hip.x, 0.08, 0.0), frame(z, y)),
            _ => (anim::SHIN[s], Vec3::new(hip.x, 0.02, 0.135), frame(z, y)),
        }
    };
    match j {
        Joint::Hips => (anim::PELVIS, Vec3::new(0.0, 0.97, 0.0), Quat::IDENTITY),
        Joint::Spine => (anim::TORSO, Vec3::new(0.0, 1.08, 0.0), Quat::IDENTITY),
        Joint::Chest => (anim::TORSO, Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY),
        Joint::UpperChest => (anim::TORSO, Vec3::new(0.0, 1.32, 0.0), Quat::IDENTITY),
        Joint::Neck => (anim::TORSO, Vec3::new(0.0, 1.46, 0.0), Quat::IDENTITY),
        Joint::Head => (anim::HEAD, Vec3::new(0.0, 1.53, 0.0), Quat::IDENTITY),
        Joint::LeftShoulder => arm(0, 0),
        Joint::LeftUpperArm => arm(0, 1),
        Joint::LeftLowerArm => arm(0, 2),
        Joint::LeftHand => arm(0, 3),
        Joint::RightShoulder => arm(1, 0),
        Joint::RightUpperArm => arm(1, 1),
        Joint::RightLowerArm => arm(1, 2),
        Joint::RightHand => arm(1, 3),
        Joint::LeftUpperLeg => leg(0, 0),
        Joint::LeftLowerLeg => leg(0, 1),
        Joint::LeftFoot => leg(0, 2),
        Joint::LeftToes => leg(0, 3),
        Joint::RightUpperLeg => leg(1, 0),
        Joint::RightLowerLeg => leg(1, 1),
        Joint::RightFoot => leg(1, 2),
        Joint::RightToes => leg(1, 3),
    }
}

/// Person `k`'s joints where its parts are now (world), as the protocol has a body's.
pub fn joints(d: &Dummies, k: usize) -> [Pose; JOINT_COUNT] {
    std::array::from_fn(|i| {
        let j = Joint::ALL[i];
        let (part, at, canonical) = layout(j);
        let (pp, pq) = d.part_pose(k, part);
        let pos = pp + pq * (at - d.part_rest(k, part));
        Pose { pos: pos.to_array(), valid: 1, rot: (pq * canonical).normalize().to_array() }
    })
}
