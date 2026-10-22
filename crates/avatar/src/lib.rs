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
//! **Joint rotations** are read as the orientation of each joint's canonical frame in tracking
//! space (the protocol's convention in `world.rs`), so a player standing in the reference T-pose
//! has exactly [`pose::reference_frame`] on every joint, and a hand's +Y points along its fingers
//! with +Z on the thumb side. [`pose`] builds such poses by hand for previews and tests.

#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

pub mod fit;
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
use windows::Win32::Graphics::Direct3D11::{ID3D11RenderTargetView, ID3D11ShaderResourceView};

pub use held::{HeldHand, held_hands};
pub use rig::{GameHand, bind_joints, finger_reach, game_hand};

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
    /// `BODYVIS_HANDS`, `BODYVIS_ARMS` or `BODYVIS_FULL`.
    pub visibility: u32,
    /// The game's light near the player, with `sun_dir` already in tracking space. All zero
    /// means the game didn't say: a neutral default is used.
    pub light: LightHint,
    /// Drawing for the wearer's own eyes: hide the head.
    pub first_person: bool,
}

/// The game's depth for the eye being drawn.
pub struct DepthInput<'a> {
    /// `R32_FLOAT` linear depth along the view axis, in game units. 0 means nothing was drawn.
    pub srv: &'a ID3D11ShaderResourceView,
    pub units_per_meter: f32,
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

    /// Shades the body with the game's own lighting model from now on (None: the runtime's own
    /// look). Call once a frame, before drawing the eyes.
    pub fn set_game_shading(&mut self, gpu: &Gpu, shading: Option<&GameShading>) {
        self.renderer.set_shading(gpu, shading);
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
        self.renderer.draw(gpu, target, width, height, eye, fov, frame, depth, &mut self.body, self.arms.as_mut())
    }

    /// The skin this would draw for `frame`, posed on the CPU: every vertex (tracking space) with
    /// the name of the bone carrying most of it. For measuring contact (a finger on a trigger) in
    /// tests and tools; the arms model in the arms and hands views, the body in the full view.
    pub fn skin(&mut self, frame: &AvatarFrame) -> Vec<(glam::Vec3, &str)> {
        self.renderer.skin(frame, &mut self.body, self.arms.as_mut())
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
            SourceMesh { name: m.name, vertices: m.vertices, indices: m.indices, texture, tint: [1.0; 3], material }
        })
        .collect();
    Source { bones: model.bones, meshes }
}
