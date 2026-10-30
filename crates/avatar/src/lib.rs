//! Draws the player's own body and hands over the game's eye images.
//!
//! The runtime solves the player's body (22 joints in tracking space, see
//! [`rustvr_protocol::RtBody`]) and draws it here, so every game gets the same articulated hands
//! whatever the adapter can do. The game draws the world and the guns; this draws the body over the
//! game's picture, hidden behind the game's own geometry using its depth buffer.
//!
//! * **Models:** any humanoid skinned model in the asset cache format ([`rustvr_asset`]) is
//!   retargeted onto the joints (see `rig.rs`): Biped, Mixamo, Unreal, Unity and Rigify names
//!   are recognised. Without one (or if it fails to load) a built-in mannequin is drawn instead.
//! * **Visibility:** hands only, hands and arms, or the full body. In first person the head (and
//!   everything under it) collapses onto the neck so the wearer never sees inside it.
//! * **Ghost hands:** where a physics hand is held back by the world, a translucent hand shows
//!   where the controller really is.
//! * **Held poses:** a hand holding a gun the game has its own hand pose for ([`HeldHand`], from
//!   the protocol's hand pose table) is drawn exactly in that pose instead of the curls: the wrist
//!   where the game's is, every finger joint as the game has it, the hand scaled to the game's
//!   hand size, and the index finger curling towards its pulled pose with the trigger.
//!   [`game_hand`] takes such a pose from a posed model (a game's first-person arms).
//!
//! Drawing happens in tracking space with each eye's pose and FOV, like the rest of the
//! compositor, into an sRGB render target (lighting is linear).
//!
//! * **The game's shading:** when the game publishes its lighting ([`GameShading`]) and a mesh
//!   carries the game's material, the mesh is shaded exactly as the game shades its own
//!   first-person model: its G-buffer terms, its lights, ambient and occlusion, its tone curve
//!   and colour grading, written as the code values the game would display ([`metro`]). Other
//!   meshes, and everything when the game doesn't say, keep the runtime's own look.
//!
//! * **Held objects:** what the hands hold (a gun, its magazine) can be drawn here too
//!   ([`HeldModel`], [`Avatar::draw_with`]): rigid parts of a game model, each where the frame puts
//!   it, in the body's depth (fingers wrap a grip) and through exactly the same shading (the game's
//!   materials and lighting). Meshes whose shading isn't reproduced here (a collimator's lens and
//!   reticle) are left to a caller's special draw ([`SpecialDraw`]); [`collimator::Collimator`]
//!   is the one for a collimator sight.
//!
//! **Joint rotations** are read as the orientation of each joint's canonical frame in tracking
//! space (the protocol's convention in `world.rs`), so a player standing in the reference T-pose
//! has exactly [`pose::reference_frame`] on every joint, and a hand's +Y points along its fingers
//! with +Z on the thumb side. [`pose`] builds such poses by hand for previews and tests.

#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

pub mod collimator;
pub mod fit;
pub mod gamebryo;
pub mod grasp;
pub mod held;
pub mod metro;
mod names;
pub mod pose;
mod procedural;
mod render;
mod rig;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Quat;
use rustvr_asset as asset;
use rustvr_gpu::Gpu;
use rustvr_protocol::{Fingers, Fov, GameLighting, HandState, JOINT_COUNT, LightHint, Pose};
use glam::Mat4;
use windows::Win32::Graphics::Direct3D11::{ID3D11RenderTargetView, ID3D11ShaderResourceView};

pub use grasp::{Grasp, Solid};
pub use held::{HeldHand, held_hands};
pub use render::HeldModel;
pub use rig::{GameHand, bind_joints, finger_reach, game_hand};

/// Each finger's joint curls (thumb .. little; 0 straight .. 1 a fist's) the hand `side` of a model
/// with `bones` (None: the built-in mannequin) closes to on `grasp`, its hand joint at `hand` (the
/// canonical frame, tracking space), the body at `joints`. For checking grasps without drawing.
pub fn grasp_curls(bones: Option<&[asset::Bone]>, joints: &[Pose; JOINT_COUNT], side: usize, hand: Pose, grasp: &Grasp) -> Result<[[f32; 3]; 5]> {
    let mannequin;
    let bones = match bones {
        Some(b) => b,
        None => {
            mannequin = procedural::mannequin().bones;
            &mannequin
        }
    };
    Ok(rig::Rig::new(bones, &[])?.grasp_curls(joints, side, hand, grasp))
}

/// Each finger's segments (thumb .. little; (start, end), tracking space) of hand `side` bent to
/// `curls`, as [`grasp_curls`] places them. For checking grasps without drawing.
pub fn finger_segments(bones: Option<&[asset::Bone]>, joints: &[Pose; JOINT_COUNT], side: usize, hand: Pose, curls: [[f32; 3]; 5]) -> Result<[Vec<(glam::Vec3, glam::Vec3)>; 5]> {
    let mannequin;
    let bones = match bones {
        Some(b) => b,
        None => {
            mannequin = procedural::mannequin().bones;
            &mannequin
        }
    };
    Ok(rig::Rig::new(bones, &[])?.finger_segments(joints, side, hand, curls))
}

/// Everything about the player's body for one frame (tracking space).
pub struct AvatarFrame<'a> {
    pub joints: &'a [Pose; JOINT_COUNT],
    pub fingers: [Fingers; 2],
    /// Where the hands are drawn (the hand joint's frame); these win over the hand joints, so a
    /// hand is always exactly where the runtime says even when the arm can't quite reach. (The
    /// flags aren't needed: ghosts are given below.)
    pub hands: [HandState; 2],
    /// Controller poses to draw as translucent ghost hands (while a physics hand is blocked).
    pub ghosts: [Option<Pose>; 2],
    /// Hands holding something in the game's own pose for it: drawn exactly so (winning over
    /// `hands` and `fingers`); None keeps the curls.
    pub held: [Option<HeldHand>; 2],
    /// Hands holding something not in a game's pose: their fingers close on it.
    pub grasp: [Option<&'a Grasp>; 2],
    /// `BODYVIS_HANDS`, `BODYVIS_ARMS` or `BODYVIS_FULL`.
    pub visibility: u32,
    /// The game's light near the player, with `sun_dir` already in tracking space. All zero
    /// means the game didn't say: a neutral default is used.
    pub light: LightHint,
    /// Drawing for the wearer's own eyes: hide the head.
    pub first_person: bool,
    /// The plain look: one colour, no textures (the framework view).
    pub plain: bool,
}

/// A picture the game drew the player into, inside its own frame: the player's colours there are
/// the game's (its light, fog, tone curve and grading), taken by [`Avatar::draw_reprojected`].
pub struct PlayerSource<'a> {
    pub color: &'a ID3D11ShaderResourceView,
    /// `R32_FLOAT`: the view depth (metres) of the player where it shows in `color`, 0 elsewhere
    /// ([`Avatar::draw_view_depth`]).
    pub player_depth: &'a ID3D11ShaderResourceView,
    /// The eye it was drawn from.
    pub eye: Pose,
    pub fov: Fov,
}

/// The game's depth for the eye being drawn (and its bloom).
pub struct DepthInput<'a> {
    /// `R32_FLOAT` linear depth along the view axis, in game units. 0 means nothing was drawn.
    pub srv: &'a ID3D11ShaderResourceView,
    pub units_per_meter: f32,
    /// The game's post chain's light and lens over the same picture (any size), so the player
    /// drawn over the finished picture gets them too: `bloom` rgb is added before the tone curve
    /// (and an eighth of it after), `lens` rgb after it; their alphas are the lens's warp (x, y):
    /// the player shows at `uv` as drawn at `uv + warp` ([`rustvr_protocol::TGT_BLOOM`]).
    pub bloom: Option<&'a ID3D11ShaderResourceView>,
    pub lens: Option<&'a ID3D11ShaderResourceView>,
}

pub enum AvatarSource {
    /// The built-in mannequin.
    Procedural,
    /// Models from an asset cache folder (e.g. `local_assets/metro2033`): `body` for the full
    /// body, and optionally first-person arms (`hands`) for the hands and arms views and the ghost
    /// hands.
    Model { dir: PathBuf, body: String, hands: Option<String> },
    /// A model given directly, with the textures its meshes and materials name (by key).
    Built { model: asset::Model, textures: HashMap<String, asset::Texture> },
}

/// The game's lighting for this frame, to shade the body exactly as the game shades its own
/// first-person model (see [`metro`]).
pub struct GameShading<'a> {
    pub lighting: &'a GameLighting,
    /// Tracking space to the game's world (the orientation its environment maps are in).
    pub world_rot: Quat,
    /// The light atlas ([`rustvr_protocol::TexKind::Lights`]), when the adapter fills one.
    pub atlas: Option<&'a ID3D11ShaderResourceView>,
}

pub struct Avatar {
    renderer: render::Renderer,
    body: render::Character,
    /// First-person arms, when the source has them.
    arms: Option<render::Character>,
}

impl Avatar {
    /// Loads the avatar. A model that fails to load (or isn't a humanoid) is replaced by the
    /// mannequin, with a warning.
    pub fn new(gpu: &Gpu, source: AvatarSource) -> Result<Avatar> {
        let renderer = render::Renderer::new(gpu)?;
        let (body, arms) = match source {
            AvatarSource::Procedural => (None, None),
            AvatarSource::Built { model, textures } => {
                let textures: HashMap<String, Arc<asset::Texture>> = textures.into_iter().map(|(k, t)| (k, Arc::new(t))).collect();
                let body = render::Character::new(gpu, from_model(model, |key| textures.get(key).cloned()));
                (body.map_err(|e| tracing::warn!("avatar: {e:#}; drawing the mannequin instead")).ok(), None)
            }
            AvatarSource::Model { dir, body, hands } => {
                let body = load_model(&dir, &body).and_then(|s| render::Character::new(gpu, s)).with_context(|| format!("avatar body {body}"));
                let arms = hands.map(|key| load_model(&dir, &key).and_then(|s| render::Character::new(gpu, s)).with_context(|| format!("avatar hands {key}")));
                let body = body.map_err(|e| tracing::warn!("{e:#}; drawing the mannequin instead")).ok();
                let arms = arms.and_then(|a| a.map_err(|e| tracing::warn!("{e:#}; using the body's own arms")).ok());
                (body, arms)
            }
        };
        let body = match body {
            Some(b) => b,
            None => render::Character::new(gpu, procedural::mannequin()).context("the built-in mannequin")?,
        };
        Ok(Avatar { renderer, body, arms })
    }

    /// Draws the hands this size: wrist to the middle finger's knuckle, metres (None: the model's
    /// at the body's scale).
    pub fn set_hand_length(&mut self, length: Option<f32>) {
        self.body.rig.set_hand_length(length);
        if let Some(a) = &mut self.arms {
            a.rig.set_hand_length(length);
        }
    }

    /// The model's hands' size (wrist to the middle finger's knuckle, metres, its bind pose).
    pub fn model_hand_length(&self) -> f32 {
        let [l, r] = self.body.rig.hand_size();
        l.max(r)
    }

    /// The forearm's share of the arm (shoulder to wrist) in the model drawn: its proportions, for
    /// the body solver to keep.
    pub fn forearm_share(&self) -> Option<f32> {
        self.body.rig.forearm_share()
    }

    /// Draw into the game's own HDR picture before its post chain (an adapter drawing the player
    /// inside the game's frame, SkyCraft's way): the game's shading writes HDR colour, and view depth
    /// in alpha as the game's combine does; the game's post chain tone maps and grades it. Takes
    /// effect at the next [`Avatar::set_game_shading`].
    pub fn set_in_frame(&mut self, in_frame: bool) {
        self.renderer.set_in_frame(in_frame);
    }

    /// Testing: draw the body in one flat colour and what it holds in another (linear; HDR in the
    /// game's frame), so a picture of them can be found exactly in each frame; None: as they are.
    pub fn set_flat(&mut self, colors: Option<([f32; 3], [f32; 3])>) {
        self.renderer.set_flat(colors);
    }

    /// Shades the body with the game's own lighting model from now on (None: the runtime's own
    /// look). Call once a frame, before drawing the eyes.
    pub fn set_game_shading(&mut self, gpu: &Gpu, shading: Option<&GameShading>) {
        self.renderer.set_shading(gpu, shading);
    }

    /// The held gun's laser from now on (None: none), drawn with the player.
    pub fn set_laser(&mut self, gpu: &Gpu, laser: Option<LaserBeam>) {
        self.renderer.set_laser(gpu, laser);
    }

    /// Draws the body into `target` (an sRGB view of one eye's image, `width` x `height`) as seen
    /// from `eye` with `fov`. With `depth`, the body is hidden behind the game's geometry;
    /// without, it's drawn on top.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        frame: &AvatarFrame,
        depth: Option<DepthInput>,
    ) -> Result<()> {
        self.renderer.draw(gpu, target, width, height, eye, fov, frame, depth, &mut self.body, self.arms.as_mut(), true, &[], None)
    }

    /// [`Avatar::draw`], with what the hands hold (`held`, in the same depth and shading), and
    /// `special` called for each held mesh a special draw takes, after the opaque ones (the eye's
    /// target and the avatar's depth buffer bound; it may change any state). Without `body` only
    /// the held objects are drawn.
    pub fn draw_with(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        frame: &AvatarFrame,
        depth: Option<DepthInput>,
        body: bool,
        held: &[HeldDraw],
        special: Option<&mut dyn FnMut(&SpecialDraw)>,
    ) -> Result<()> {
        self.renderer.draw(gpu, target, width, height, eye, fov, frame, depth, &mut self.body, self.arms.as_mut(), body, held, special)
    }

    /// The view depth (metres) of the player `frame` poses (with `held`) where it shows from `eye`,
    /// hidden behind the game's world (`depth`): into `target` (`R32_FLOAT`, `width` x `height`), 0
    /// where the player isn't. Drawn for a picture the game drew the player into, it tells which of
    /// its pixels are the player's (see [`Avatar::draw_reprojected`]).
    pub fn draw_view_depth(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        frame: &AvatarFrame,
        depth: Option<DepthInput>,
        held: &[HeldDraw],
    ) -> Result<()> {
        self.renderer.draw_view_depth(gpu, target, width, height, eye, fov, frame, depth, &mut self.body, self.arms.as_mut(), held)
    }

    /// Writes the depth of the player `frame` poses (with `held`), seen from `eye`, into the game's
    /// own depth buffer (`dsv`, `width` x `height`) as the game's projection `proj` (its own near and
    /// far planes) stores it, where it's nearer than what's there. An adapter drawing the player in
    /// the game's frame does this after the colour, so the game's later passes (its volumetric
    /// light, particles, glass) stop at the player as they do at the game's own things.
    pub fn draw_game_depth(
        &mut self,
        gpu: &Gpu,
        dsv: &windows::Win32::Graphics::Direct3D11::ID3D11DepthStencilView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        proj: Mat4,
        frame: &AvatarFrame,
        held: &[HeldDraw],
    ) -> Result<()> {
        self.renderer.draw_game_depth(gpu, dsv, width, height, eye, fov, proj, frame, &mut self.body, self.arms.as_mut(), held)
    }

    /// The player the game drew inside its own frame, where it is now: drawn posed as `now` (with
    /// each of `held`'s parts where they are now), every point in the colour the game's pictures
    /// (`sources`) have it in, the game having drawn it posed as `then` (each held object's parts
    /// where the second of its pair says). So the game's own pixels of the player (its lighting,
    /// fog, bloom, tone curve and grading) follow the player at every headset frame. Hidden behind
    /// the world as it shows now (`depth`).
    pub fn draw_reprojected(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        now: &AvatarFrame,
        then: &AvatarFrame,
        depth: Option<DepthInput>,
        held: &[(HeldDraw, &[Option<Mat4>])],
        sources: &[PlayerSource],
    ) -> Result<()> {
        self.renderer.draw_reprojected(gpu, target, width, height, eye, fov, now, then, depth, &mut self.body, self.arms.as_mut(), held, sources)
    }

    /// The skin this would draw for `frame`, posed on the CPU: every vertex (tracking space) with
    /// the name of the bone carrying most of it. For measuring contact (a finger on a trigger) in
    /// tests and tools; the arms model in the arms and hands views, the body in the full view.
    pub fn skin(&mut self, frame: &AvatarFrame) -> Vec<(glam::Vec3, &str)> {
        self.renderer.skin(frame, &mut self.body, self.arms.as_mut())
    }
}

/// One of a [`HeldModel`]'s meshes.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldMeshInfo {
    /// The part it moves with (its place in [`HeldDraw::parts`]).
    pub part: usize,
    /// The mesh of the game's model it was cut from, and that mesh's name.
    pub source: usize,
    pub name: String,
    /// Taken by a special draw ([`SpecialDraw`]): the game shades it with a shader this crate's
    /// materials don't reproduce ([`metro::Class::of`] has none, in a model that has the game's
    /// materials), or it is part of a collimator ([`collimator::takes`]; [`collimator::Collimator`]
    /// is the special draw that draws it).
    pub special: bool,
}

/// One of a [`HeldModel`]'s parts, from a mesh of the game's model: its triangles (`indices`) over
/// some of that mesh's vertices (`vertices`, their indices in the mesh).
pub struct HeldPart {
    pub part: usize,
    pub mesh: usize,
    pub vertices: Vec<u32>,
    pub indices: Vec<u32>,
}

/// A gun's laser (tracking space): its beam from `from` to `to`, and its dot there when it hits
/// something (`hit`). Drawn with the player, in the same depth (the gun hides it) and with the
/// game's look on its light (its tone curve and grading, its fog, and a glow round the dot for
/// the bloom the game would give it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaserBeam {
    pub from: glam::Vec3,
    pub to: glam::Vec3,
    pub hit: bool,
}

/// A held model to draw this eye: each of its parts' model space -> tracking space (None: not
/// drawn).
pub struct HeldDraw<'a> {
    pub model: &'a HeldModel,
    pub parts: &'a [Option<Mat4>],
    /// What it is, for the special draws (e.g. `metro2033/wpn_2012`).
    pub key: &'a str,
}

/// A held mesh a caller draws itself (see [`HeldMeshInfo::special`]), between the opaque meshes
/// and the ghosts, into the eye's target with the avatar's depth buffer bound.
pub struct SpecialDraw<'a> {
    pub gpu: &'a Gpu,
    /// The held model's key ([`HeldDraw::key`]) and the mesh.
    pub key: &'a str,
    pub mesh: &'a HeldMeshInfo,
    /// The mesh's model space -> tracking space (its part's place).
    pub model: Mat4,
    /// Tracking space -> clip space as the avatar draws (left-handed, D3D depth), and its near and
    /// far planes (metres).
    pub view_proj: Mat4,
    pub near_far: (f32, f32),
    pub eye: &'a Pose,
    pub fov: &'a Fov,
    pub width: u32,
    pub height: u32,
    /// The game's linear depth and its units per metre, when the game gave it.
    pub game_depth: Option<(&'a ID3D11ShaderResourceView, f32)>,
    /// The game's lighting is set for this frame ([`Avatar::set_game_shading`]).
    pub shading: bool,
}

impl HeldModel {
    /// A held model from a game model in an asset cache folder (`dir`, its textures by key), in
    /// `parts`. Meshes with the game's materials whose shader isn't reproduced here are marked
    /// [`HeldMeshInfo::special`].
    pub fn load(gpu: &Gpu, dir: &std::path::Path, model: &asset::Model, parts: &[HeldPart]) -> Result<HeldModel> {
        let mut cache: HashMap<String, Option<Arc<asset::Texture>>> = HashMap::new();
        let mut texture = |key: &str| -> Option<Arc<asset::Texture>> {
            if key.is_empty() {
                return None;
            }
            cache
                .entry(key.to_string())
                .or_insert_with(|| asset::load_texture(dir, key).map_err(|e| tracing::warn!("avatar: texture {key}: {e:#}")).ok().map(Arc::new))
                .clone()
        };
        let game_materials = model.meshes.iter().any(|m| m.material.shader != asset::Shader::Unknown);
        let mut meshes = Vec::new();
        for p in parts {
            let m = model.meshes.get(p.mesh).with_context(|| format!("{}: no mesh {}", model.source, p.mesh))?;
            let vertices = p.vertices.iter().map(|&i| m.vertices.get(i as usize).copied().with_context(|| format!("{}: mesh {} has no vertex {i}", model.source, p.mesh))).collect::<Result<Vec<_>>>()?;
            anyhow::ensure!(p.indices.iter().all(|&i| (i as usize) < vertices.len()), "{}: a part of mesh {} indexes past its vertices", model.source, p.mesh);
            let mesh = asset::Mesh { texture: m.texture.clone(), vertices, indices: p.indices.clone(), name: m.name.clone(), material: m.material.clone() };
            let one = asset::Model { source: model.source.clone(), meshes: vec![mesh], ..Default::default() };
            let Some(source) = from_model(one, &mut texture).meshes.pop() else { continue };
            // Shaders not reproduced here, and the whole of a collimator (its reticle, the lens's
            // reflection, pane and refraction, which the game draws in that order: `collimator`).
            let special = game_materials && (metro::Class::of(m.material.shader).is_none() || collimator::takes(model, p.mesh));
            meshes.push((source, HeldMeshInfo { part: p.part, source: p.mesh, name: m.name.clone(), special }));
        }
        render::HeldModel::new(gpu, meshes)
    }

    /// A model that bends (a strap, a rope) from an asset cache folder (`dir`, its textures by
    /// key): its meshes whole, each vertex moving with its own bones as weighted, and a part for
    /// each bone ([`HeldDraw::parts`] places them: bone space -> tracking space). Drawn with the
    /// body as anything held is: in its depth, with the game's materials and lighting; the game's
    /// surface hides it only once it's `depth_slack` (metres) behind it (a strap lying on the
    /// game's own model).
    pub fn load_bent(gpu: &Gpu, dir: &std::path::Path, model: &asset::Model, depth_slack: f32) -> Result<HeldModel> {
        let mut cache: HashMap<String, Option<Arc<asset::Texture>>> = HashMap::new();
        let mut texture = |key: &str| -> Option<Arc<asset::Texture>> {
            if key.is_empty() {
                return None;
            }
            cache
                .entry(key.to_string())
                .or_insert_with(|| asset::load_texture(dir, key).map_err(|e| tracing::warn!("avatar: texture {key}: {e:#}")).ok().map(Arc::new))
                .clone()
        };
        let mut meshes = Vec::new();
        for (i, m) in model.meshes.iter().enumerate() {
            let one = asset::Model { source: model.source.clone(), meshes: vec![m.clone()], ..Default::default() };
            let Some(mut source) = from_model(one, &mut texture).meshes.pop() else { continue };
            source.depth_slack = depth_slack;
            meshes.push((source, HeldMeshInfo { part: 0, source: i, name: m.name.clone(), special: false }));
        }
        render::HeldModel::bent(gpu, meshes)
    }
}

/// A model as the renderer takes it: a skeleton and meshes, with their textures.
pub(crate) struct Source {
    pub bones: Vec<asset::Bone>,
    pub meshes: Vec<SourceMesh>,
}

pub(crate) struct SourceMesh {
    pub name: String,
    pub vertices: Vec<asset::Vertex>,
    pub indices: Vec<u32>,
    pub texture: Option<Arc<asset::Texture>>,
    /// Linear colour multiplied with the texture.
    pub tint: [f32; 3],
    /// How the game shades it, when that's known and its textures are all there.
    pub material: Option<SourceMaterial>,
    /// How far behind the game's surface it may be before it's hidden (metres, on top of the
    /// body's own allowance): 0 but for a strap lying on the game's model.
    pub depth_slack: f32,
}

pub(crate) struct SourceMaterial {
    pub class: metro::Class,
    pub params: [f32; 4],
    pub color: [f32; 4],
    pub alpha_test: bool,
    /// The emissive classes' multiplier (2 when the asset doesn't say).
    pub emissive: f32,
    /// The `use_fresnel` permutation (see [`metro::Surface::fresnel`]).
    pub fresnel: bool,
    pub bump: Option<Arc<asset::Texture>>,
    pub detail: Option<Arc<asset::Texture>>,
    pub env: Option<Arc<asset::Texture>>,
}

/// A body put together from a game's parts (see [`rustvr_protocol::AvatarDesc`]): the skeleton
/// model's bones, and the meshes of every part (each skinned to those bones in their order), from
/// the asset cache folder `dir`, with the texture swaps applied (a part's texture or bump map
/// named first is drawn with the second, when the cache has it). A part made for another skeleton
/// (a body or an armour made for one race's skeleton, worn by another's) is put on this one by its
/// bones' names, as the game itself does ([`rebind`]); a part with bones the skeleton hasn't is left
/// out.
pub fn compose(dir: &std::path::Path, skeleton: &str, parts: &[String], swaps: &[(String, String)]) -> Result<AvatarSource> {
    let skel = asset::load_model(dir, skeleton).with_context(|| format!("the skeleton {skeleton}"))?;
    // (A swap to a texture the cache doesn't have keeps the part's own.)
    let have = |key: &str| dir.join("textures").join(format!("{key}.{}", asset::TEXTURE_EXT)).is_file();
    let swap = |k: &mut String| {
        if let Some((_, to)) = swaps.iter().find(|(from, to)| from == k && have(to)) {
            *k = to.clone();
        }
    };
    let mut model = asset::Model { version: asset::FORMAT_VERSION, source: format!("{skeleton} with {} parts", parts.len()), bones: skel.bones, ..Default::default() };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for key in parts {
        let part = match asset::load_model(dir, key) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("avatar: part {key}: {e:#}");
                continue;
            }
        };
        let mut part = part;
        if part.bones.len() != model.bones.len() || part.bones.iter().zip(&model.bones).any(|(a, b)| a.name != b.name) {
            if let Err(e) = rebind(&mut part, &model.bones) {
                tracing::warn!("avatar: part {key} isn't skinned to the skeleton {skeleton}: {e:#}: left out");
                continue;
            }
        }
        for k in 0..3 {
            lo[k] = lo[k].min(part.bounds_min[k]);
            hi[k] = hi[k].max(part.bounds_max[k]);
        }
        for mut m in part.meshes {
            swap(&mut m.texture);
            swap(&mut m.material.bump);
            model.meshes.push(m);
        }
    }
    anyhow::ensure!(!model.meshes.is_empty(), "no part of the body could be loaded");
    (model.bounds_min, model.bounds_max) = (lo, hi);
    let mut textures = HashMap::new();
    for m in &model.meshes {
        for key in [&m.texture, &m.material.bump, &m.material.detail, &m.material.env] {
            if key.is_empty() || textures.contains_key(key) {
                continue;
            }
            match asset::load_texture(dir, key) {
                Ok(t) => {
                    textures.insert(key.clone(), t);
                }
                Err(e) => tracing::warn!("avatar: texture {key}: {e:#}"),
            }
        }
    }
    Ok(AvatarSource::Built { model, textures })
}

/// Puts a part skinned to another skeleton (`part.bones`) on `bones` by the bones' names: each
/// vertex goes where its bones take it from the part's bind pose to this skeleton's (the weighted
/// sum of each bone's move, as the game skins a part to whatever skeleton it's on), and its bone
/// indices become this skeleton's. An error when a bone the part is weighted to isn't in it.
pub fn rebind(part: &mut asset::Model, bones: &[asset::Bone]) -> Result<()> {
    use glam::{Mat4, Vec3};
    let by_name: HashMap<String, usize> = bones.iter().enumerate().map(|(i, b)| (b.name.to_ascii_lowercase(), i)).collect();
    let to: Vec<Option<usize>> = part.bones.iter().map(|b| by_name.get(&b.name.to_ascii_lowercase()).copied()).collect();
    // Each of the part's bones' move: off its bind pose, onto this skeleton's.
    let moves: Vec<Option<Mat4>> = part
        .bones
        .iter()
        .zip(&to)
        .map(|(b, t)| t.map(|t| Mat4::from_cols_array(&bones[t].model_matrix) * Mat4::from_cols_array(&b.model_matrix).inverse()))
        .collect();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for m in &mut part.meshes {
        for v in &mut m.vertices {
            let total: u32 = v.weights.iter().map(|&w| w as u32).sum();
            if total > 0 {
                let mut sum = Mat4::ZERO;
                for k in 0..4 {
                    if v.weights[k] == 0 {
                        continue;
                    }
                    let b = v.bones[k] as usize;
                    let (Some(mv), Some(t)) = (moves.get(b).copied().flatten(), to.get(b).copied().flatten()) else {
                        anyhow::bail!("the skeleton has no {}", part.bones.get(b).map_or("(bone)", |b| b.name.as_str()));
                    };
                    sum += mv * (v.weights[k] as f32 / total as f32);
                    v.bones[k] = u8::try_from(t).context("a bone past the 256th")?;
                }
                let p = sum.transform_point3(Vec3::from_array(v.pos));
                v.pos = p.to_array();
                v.normal = sum.transform_vector3(Vec3::from_array(v.normal)).normalize_or_zero().to_array();
            }
            lo = lo.min(Vec3::from_array(v.pos));
            hi = hi.max(Vec3::from_array(v.pos));
        }
    }
    part.bones = bones.to_vec();
    if lo.x <= hi.x {
        (part.bounds_min, part.bounds_max) = (lo.to_array(), hi.to_array());
    }
    Ok(())
}

/// The skeleton of the built-in mannequin (Unity humanoid names, bound in the reference T-pose),
/// for building models in memory ([`AvatarSource::Built`]).
pub fn mannequin_skeleton() -> Vec<asset::Bone> {
    procedural::mannequin().bones
}

/// Eye overlay shells (Metro's `eyeballenv`, the cornea, tearline and occlusion meshes of Unreal
/// and Character Creator heads) are blended over the eyes in their games; drawn opaque they hide
/// the iris, so they're left out.
fn is_eye_overlay(mesh_name: &str) -> bool {
    let n = mesh_name.to_ascii_lowercase();
    (n.contains("eye") && n.contains("env")) || ["cornea", "tearline", "occlusion", "lacrimal"].iter().any(|k| n.contains(k))
}

fn load_model(dir: &std::path::Path, key: &str) -> Result<Source> {
    let model = asset::load_model(dir, key)?;
    let mut cache: HashMap<String, Option<Arc<asset::Texture>>> = HashMap::new();
    Ok(from_model(model, |key| {
        cache
            .entry(key.to_string())
            .or_insert_with(|| asset::load_texture(dir, key).map_err(|e| tracing::warn!("avatar: texture {key}: {e:#}")).ok().map(Arc::new))
            .clone()
    }))
}

/// A model with its textures looked up by key.
fn from_model(model: asset::Model, mut texture: impl FnMut(&str) -> Option<Arc<asset::Texture>>) -> Source {
    let mut get = |key: &str| if key.is_empty() { None } else { texture(key) };
    let meshes = model
        .meshes
        .into_iter()
        .filter(|m| !m.indices.is_empty() && !is_eye_overlay(&m.name))
        .map(|m| {
            let texture = get(&m.texture);
            let material = metro::Class::of(m.material.shader).and_then(|class| {
                let mat = &m.material;
                let (bump, detail, env) = (get(&mat.bump), get(&mat.detail), get(&mat.env));
                let (need_bump, need_detail, need_env) = class.needs();
                let missing = texture.is_none() || (need_bump && bump.is_none()) || (need_detail && detail.is_none()) || (need_env && env.is_none());
                if missing {
                    tracing::debug!("avatar: mesh {}: {class:?} without all its textures; drawn with the base texture alone", m.name);
                    return None;
                }
                let emissive = if mat.emissive > 0.0 { mat.emissive } else { 2.0 };
                Some(SourceMaterial { class, params: mat.params, color: mat.color, alpha_test: mat.alpha_test, emissive, fresnel: mat.fresnel, bump, detail, env })
            });
            SourceMesh { name: m.name, vertices: m.vertices, indices: m.indices, texture, tint: [1.0; 3], material, depth_slack: 0.0 }
        })
        .collect();
    Source { bones: model.bones, meshes }
}
