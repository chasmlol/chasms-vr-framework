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
//! **Joint rotations** are read as the orientation of each joint's canonical frame in tracking
//! space (the protocol's convention in `world.rs`), so a player standing in the reference T-pose
//! has exactly [`pose::reference_frame`] on every joint, and a hand's +Y points along its fingers
//! with +Z on the thumb side. [`pose`] builds such poses by hand for previews and tests.

#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

pub mod fit;
pub mod held;
mod names;
pub mod pose;
mod procedural;
mod render;
mod rig;

use std::path::PathBuf;

use anyhow::{Context, Result};
use rustvr_asset as asset;
use rustvr_gpu::Gpu;
use rustvr_protocol::{Fingers, Fov, HandState, JOINT_COUNT, LightHint, Pose};
use windows::Win32::Graphics::Direct3D11::{ID3D11RenderTargetView, ID3D11ShaderResourceView};

pub use held::{HeldHand, held_hands};
pub use rig::{GameHand, finger_reach, game_hand};

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
    pub texture: Option<asset::Texture>,
    /// Linear colour multiplied with the texture.
    pub tint: [f32; 3],
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
    let meshes = model
        .meshes
        .into_iter()
        .filter(|m| !m.indices.is_empty() && !is_eye_overlay(&m.name))
        .map(|m| {
            let texture = (!m.texture.is_empty())
                .then(|| asset::load_texture(dir, &m.texture).map_err(|e| tracing::warn!("avatar: texture {}: {e:#}", m.texture)).ok())
                .flatten();
            SourceMesh { name: m.name, vertices: m.vertices, indices: m.indices, texture, tint: [1.0; 3] }
        })
        .collect();
    Ok(Source { bones: model.bones, meshes })
}
