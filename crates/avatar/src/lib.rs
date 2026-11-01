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
//! * **Joints that keep their volume:** every vertex on two or more bones is skinned about its
//!   optimised centre of rotation ([`cor`]), so elbows don't pinch and wrists don't twist into a
//!   ribbon.
//! * **Armour:** a game's marks ([`rustvr_asset::armour`]) make parts of its armour rigid plates,
//!   each riding one bone or several with shares ([`armour`], [`compose`], `docs/ARMOUR.md`);
//!   `rustvr-armour` previews them through a sweep of poses.
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
//!   and colour grading, written as the code values the game would display ([`shading`]). Other
//!   meshes, and everything when the game doesn't say, keep the runtime's own look.
//!
//! * **Held objects:** what the hands hold (a gun, its magazine) can be drawn here too
//!   ([`HeldModel`], [`Avatar::draw_with`]): rigid parts of a game model, each where the frame puts
//!   it, in the body's depth (fingers wrap a grip) and through exactly the same shading (the game's
//!   materials and lighting). Meshes whose shading isn't reproduced here (a sight's lens) are
//!   left to a caller's special draw ([`SpecialDraw`]).
//!
//! **Joint rotations** are read as the orientation of each joint's canonical frame in tracking
//! space (the protocol's convention in `world.rs`), so a player standing in the reference T-pose
//! has exactly [`pose::reference_frame`] on every joint, and a hand's +Y points along its fingers
//! with +Z on the thumb side. [`pose`] builds such poses by hand for previews and tests.

#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

pub mod armour;
pub mod cor;
pub mod facegen;
pub mod sculpt;
pub mod fit;
pub mod gamebryo;
pub mod gore;
pub mod grasp;
pub mod held;
mod names;
pub mod pose;
mod procedural;
mod render;
mod rig;
pub mod shading;

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
pub use rig::{GameHand, bind_joints, finger_reach, game_hand, rest_pose};

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
/// The game's water near the player ([`Avatar::set_water`]): its surface's height in tracking
/// space, its colour (as the game's shaders take it) and how far through it light goes (metres).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Water {
    pub surface: f32,
    pub color: [f32; 3],
    pub visibility: f32,
}

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
    /// Models from an asset cache folder (e.g. `local_assets/oblivion`): `body` for the full
    /// body, and optionally first-person arms (`hands`) for the hands and arms views and the ghost
    /// hands.
    Model { dir: PathBuf, body: String, hands: Option<String> },
    /// A model given directly, with the textures its meshes and materials name (by key), and how
    /// it's skinned beyond its weights (None: worked out from them).
    Built { model: asset::Model, textures: HashMap<String, asset::Texture>, skinning: Option<Skin> },
    /// A body put together from a game's parts ([`compose`]): as [`AvatarSource::Built`] (its
    /// textures shared with the parts' cache), with what each mesh draws beside its material (a
    /// face's textures, the hair's tint), by mesh, and how it's skinned beyond its weights.
    Composed { model: asset::Model, textures: HashMap<String, Arc<asset::Texture>>, extras: Vec<MeshExtras>, skinning: Option<Skin> },
}

/// How a model is skinned beyond its weights.
#[derive(Clone, Debug, Default)]
pub struct Skin {
    /// Each vertex's centre of rotation.
    pub centres: Centres,
    /// Bones that are copies of others bound in another pose (copy, of): see `armour.rs`.
    pub copies: Vec<(usize, usize)>,
}

/// Each mesh's vertices' centres of rotation (`cor.rs`): `[x, y, z, w]`, w 0 for a vertex skinned
/// by linear blending, else 1 plus its plate's layer (`armour.rs`) for one turned about (x, y, z);
/// the body under what's worn (see [`compose`]) 0.25 or 0.75 for the same.
pub type Centres = Vec<Vec<[f32; 4]>>;

/// How a body put together by [`compose`] is skinned.
#[derive(Clone, Copy)]
pub enum Skinning<'a> {
    /// The game's weights, blended linearly (as the game skins them).
    Linear,
    /// The game's weights, with joints that keep their volume (`cor.rs`).
    Centres,
    /// That, and the plates the marks give each piece (`armour.rs`).
    Marked(&'a asset::armour::ArmourFile),
}

/// What a mesh of a body put together from a game's parts draws beside its model's material.
#[derive(Clone, Debug, Default)]
pub struct MeshExtras {
    /// A FaceGen face's texture morphs' texture ([`facegen::morph_texture`]), laid over the base
    /// colour as the game's FaceGen blend does, with the face's detail map (a texture key) over it.
    pub face: Option<asset::Texture>,
    pub face_detail: Option<String>,
    /// The hair's tint ([`asset::Shader::GamebryoHair`]).
    pub hair_tint: Option<[f32; 3]>,
    /// A FaceGen part's shape as the face moves it ([`facegen::FaceShape`]): the mesh is drawn
    /// again as the face changes live ([`Avatar::set_face_shape`]).
    pub shape: Option<Arc<facegen::FaceShape>>,
}

/// The game's lighting for this frame, to shade the body exactly as the game shades its own
/// first-person model (see [`shading`]).
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
        let (body, arms) = Self::characters(gpu, source)?;
        Ok(Avatar { renderer, body, arms })
    }

    /// Draws another body from now on (the game's, put together again: something put on, a face
    /// changed), keeping the renderer (its shaders take a second to compile: a character creator's
    /// every change would hold the frame that long).
    pub fn set_source(&mut self, gpu: &Gpu, source: AvatarSource) -> Result<()> {
        (self.body, self.arms) = Self::characters(gpu, source)?;
        Ok(())
    }

    /// The body (and first-person arms) a source makes, the mannequin when its model fails.
    fn characters(gpu: &Gpu, source: AvatarSource) -> Result<(render::Character, Option<render::Character>)> {
        let (body, arms) = match source {
            AvatarSource::Procedural => (None, None),
            AvatarSource::Built { model, textures, skinning } => {
                let textures: HashMap<String, Arc<asset::Texture>> = textures.into_iter().map(|(k, t)| (k, Arc::new(t))).collect();
                let (centres, copies) = skinning.map_or((None, Vec::new()), |s| (Some(s.centres), s.copies));
                let mut source = from_model(model, centres, |key| textures.get(key).cloned());
                source.copies = copies;
                let body = render::Character::new(gpu, source);
                (body.map_err(|e| tracing::warn!("avatar: {e:#}; drawing the mannequin instead")).ok(), None)
            }
            AvatarSource::Composed { model, textures, extras, skinning } => {
                let (centres, copies) = skinning.map_or((None, Vec::new()), |s| (Some(s.centres), s.copies));
                let mut source = from_model_with(model, extras, centres, |key| textures.get(key).cloned());
                source.copies = copies;
                let body = render::Character::new(gpu, source);
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
        Ok((body, arms))
    }

    /// Draws the hands this size: wrist to the middle finger's knuckle, metres (None: the model's
    /// at the body's scale).
    pub fn set_hand_length(&mut self, length: Option<f32>) {
        self.body.rig.set_hand_length(length);
        if let Some(a) = &mut self.arms {
            a.rig.set_hand_length(length);
        }
    }

    /// Where the eyes look from now on (tracking space; None: straight ahead, as the head looks):
    /// a model's eye bones turn to it, as far as eyes turn. A mirror's reflection looks back at the
    /// wearer's own reflected eyes.
    pub fn set_gaze(&mut self, at: Option<glam::Vec3>) {
        self.body.gaze = at;
        if let Some(a) = &mut self.arms {
            a.gaze = at;
        }
    }

    /// Draws `face` on the body's FaceGen parts from now on, without putting it together again (a
    /// face shaped by hand, live; None: the face it was put together with).
    pub fn set_face_shape(&mut self, face: Option<Arc<facegen::FaceNow>>) {
        self.body.face = face;
    }

    /// Where the bone `name` is as `frame` poses the body (with its head, as a mirror shows it): its
    /// skinning matrix, bind model space -> tracking space.
    pub fn bone_skin(&mut self, frame: &AvatarFrame, name: &str) -> Option<Mat4> {
        self.renderer.bone_skin(frame, &mut self.body, name)
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

    /// The game's water where the player is, from now on (None: none): its surface is in the
    /// game's depth, but the body under it is seen through it, the water's colour over it.
    pub fn set_water(&mut self, water: Option<Water>) {
        self.renderer.set_water(water);
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

    /// The game's effect shaders over the body and hands from now on (a spell's shader on the
    /// player), each drawn over them as it draws over what's held.
    pub fn set_body_overlays(&mut self, overlays: Vec<OverlayDraw>) {
        self.renderer.set_body_overlays(overlays);
    }

    /// The sheets on what the hands hold from now on (none: none), drawn with the player.
    pub fn set_sheets(&mut self, sheets: Vec<Sheet>) {
        self.renderer.set_sheets(sheets);
    }

    /// Draws the body into `target` (an sRGB view of one eye's image, `width` x `height`) as seen
    /// from `eye` with `fov`. With `depth`, the body is hidden behind the game's geometry;
    /// without, it's drawn on top.
    /// The blood to draw into the next eye ([`gore`]): `None` for none.
    pub fn set_gore(&mut self, gpu: &Gpu, frame: Option<&gore::GoreFrame>) {
        self.renderer.set_gore(gpu, frame);
    }

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
    /// materials don't reproduce ([`shading::Class::of`] has none, in a model that has the game's
    /// materials).
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

/// A sheet of a texture on something the hands hold (tracking space): a book's pages, a map, cut
/// from the game's own UI. Drawn with the player, in its depth, after what the hands hold: a hand in
/// front of a page covers it, and the fingers behind a sheet are hidden by it. The texture is
/// premultiplied (its colour times its alpha, over nothing), drawn as it is.
#[derive(Clone, Debug)]
pub struct Sheet {
    pub srv: ID3D11ShaderResourceView,
    /// Its middle, and half its width and half its height along it (right and up as it's read).
    pub center: glam::Vec3,
    pub half_x: glam::Vec3,
    pub half_y: glam::Vec3,
    /// The part of the texture on it: u0, v0, u1, v1 (top-left origin).
    pub uv: [f32; 4],
    pub opacity: f32,
    /// Its edges fade out over this part of its half-width and half-height (0: a hard edge).
    pub soft: [f32; 2],
    /// Its coverage is its colour's, whatever the texture's alpha says (a game that draws a page's
    /// colour but not its coverage): solid where something was drawn, clear where it's black.
    pub opaque: bool,
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
    /// What it is, for the special draws (`<game folder>/<asset key>`).
    pub key: &'a str,
    /// The game's effect shader over it (an enchanted weapon's shimmer).
    pub overlay: Option<OverlayDraw>,
    /// How far a model that bends is bent ([`HeldModel::load_bent`]: 0 its own shape, 1 the bent
    /// one; a bow drawn); 0 for any other.
    pub bend: f32,
}

/// The game's effect shader over a held model ([`rustvr_protocol::Overlay`]): its fill and edge,
/// its texture when it has one (raw, as the game samples it) and how long it has run (seconds:
/// its pulses and its texture's scroll).
#[derive(Clone)]
pub struct OverlayDraw {
    pub overlay: rustvr_protocol::Overlay,
    pub texture: Option<ID3D11ShaderResourceView>,
    pub time: f32,
}

/// A texture from an asset cache folder (`dir`, by key) as the game samples it (raw), for an
/// [`OverlayDraw`].
pub fn overlay_texture(gpu: &Gpu, dir: &std::path::Path, key: &str) -> Result<ID3D11ShaderResourceView> {
    let t = asset::load_texture(dir, key)?;
    Ok(render::upload_texture(gpu, &t)?.linear)
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
        Self::load_bent(gpu, dir, model, None, parts)
    }

    /// The same, for a model that bends as it's used (a bow as its string is drawn): `bent` is the
    /// same meshes and vertices, in the same order, bent all the way (the game's morph target); it's
    /// drawn as far between the two as each draw's [`HeldDraw::bend`] says.
    pub fn load_bent(gpu: &Gpu, dir: &std::path::Path, model: &asset::Model, bent: Option<&asset::Model>, parts: &[HeldPart]) -> Result<HeldModel> {
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
            let Some(source) = from_model(one, None, &mut texture).meshes.pop() else { continue };
            // Shaders not reproduced here.
            let special = game_materials && shading::Class::of(m.material.shader).is_none();
            // Each vertex's offset to the bent shape, where there's one with the same vertices.
            let offsets = bent.and_then(|b| b.meshes.get(p.mesh)).filter(|b| b.vertices.len() == m.vertices.len()).map(|b| {
                p.vertices
                    .iter()
                    .map(|&i| {
                        let (r, t) = (&m.vertices[i as usize], &b.vertices[i as usize]);
                        let d = |a: [f32; 3], b: [f32; 3]| [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                        [d(r.pos, t.pos), d(r.normal, t.normal)]
                    })
                    .collect::<Vec<_>>()
            });
            meshes.push((source, HeldMeshInfo { part: p.part, source: p.mesh, name: m.name.clone(), special }, offsets));
        }
        render::HeldModel::new(gpu, meshes)
    }

    /// A model that bends along its bones (a strap, a rope) from an asset cache folder (`dir`, its
    /// textures by key): its meshes whole, each vertex moving with its own bones as weighted, and a
    /// part for each bone ([`HeldDraw::parts`] places them: bone space -> tracking space). Drawn
    /// with the body as anything held is: in its depth, with the game's materials and lighting; the
    /// game's surface hides it only once it's `depth_slack` (metres) behind it (a strap lying on the
    /// game's own model). (Not [`HeldModel::load_bent`]'s bend: that's a morph, a bow drawn.)
    pub fn load_strap(gpu: &Gpu, dir: &std::path::Path, model: &asset::Model, depth_slack: f32) -> Result<HeldModel> {
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
            let Some(mut source) = from_model(one, None, &mut texture).meshes.pop() else { continue };
            source.depth_slack = depth_slack;
            meshes.push((source, HeldMeshInfo { part: 0, source: i, name: m.name.clone(), special: false }));
        }
        render::HeldModel::strap(gpu, meshes)
    }

    /// A sheet ([`rustvr_protocol::OBJF_SHEET`]): a page on the +Y face of a thin box (`centre`,
    /// `half`: its collision, object space), its face the texture `face` (its sRGB-decoding and its
    /// raw view: a card the runtime paints) with its top towards +Z, and its back on -Y the part `uv`
    /// of the cache's texture `back` (the page's paper, the other side of it). Shaded as Gamebryo
    /// shades the game's own things (matte, no highlight, as paper is), the card's clear parts cut
    /// out.
    pub fn sheet(gpu: &Gpu, face: (ID3D11ShaderResourceView, ID3D11ShaderResourceView), back: Option<(asset::Texture, [f32; 4])>, centre: glam::Vec3, half: glam::Vec3) -> Result<HeldModel> {
        // A hair over the box, so the page isn't lost in a table it lies on.
        const OVER: f32 = 0.0005;
        let placeholder = Arc::new(asset::Texture { version: 0, source: "sheet face".into(), width: 1, height: 1, format: asset::TexFormat::Rgba8, mips: vec![vec![0; 4]], faces: 1 });
        let quad = |y: f32, uv: [f32; 4]| -> Vec<asset::Vertex> {
            let n = if y > 0.0 { [0.0, 1.0, 0.0] } else { [0.0, -1.0, 0.0] };
            [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
                .map(|(x, z): (f32, f32)| asset::Vertex {
                    pos: (centre + glam::Vec3::new(x * half.x, y, z * half.z)).to_array(),
                    normal: n,
                    // Across with +X, down the card with -Z.
                    uv: [uv[0] + (x + 1.0) * 0.5 * (uv[2] - uv[0]), uv[1] + (1.0 - z) * 0.5 * (uv[3] - uv[1])],
                    bones: [0; 4],
                    weights: [255, 0, 0, 0],
                    ao: 255,
                })
                .to_vec()
        };
        let paper = || SourceMaterial {
            class: shading::Class::GamebryoLit,
            params: [0.0; 4],
            color: [0.0; 4],
            alpha_test: true,
            emissive: 0.0,
            fresnel: false,
            bump: None,
            detail: None,
            env: None,
            face: None,
            face_detail: None,
            hair_tint: None,
        };
        let mesh = |name: &str, vertices: Vec<asset::Vertex>, texture: Arc<asset::Texture>| SourceMesh {
            name: name.into(),
            vertices,
            // (Wound to face along its normal when it's loaded.)
            indices: vec![0, 1, 2, 0, 2, 3],
            texture: Some(texture),
            tint: [1.0; 3],
            material: Some(paper()),
            depth_slack: 0.0,
            centres: Vec::new(),
            shape: None,
        };
        let mut meshes = vec![mesh("sheet face", quad(half.y + OVER, [0.0, 0.0, 1.0, 1.0]), placeholder.clone())];
        if let Some((t, uv)) = back {
            meshes.push(mesh("sheet back", quad(-half.y - OVER, uv), Arc::new(t)));
        }
        let meshes = meshes.into_iter().enumerate().map(|(k, m)| {
            let name = m.name.clone();
            (m, HeldMeshInfo { part: 0, source: k, name, special: false })
        });
        render::HeldModel::with_views(gpu, meshes.collect(), &[(placeholder, render::GpuTex { srgb: face.0, linear: face.1 })])
    }
}

/// A model as the renderer takes it: a skeleton and meshes, with their textures.
pub(crate) struct Source {
    pub bones: Vec<asset::Bone>,
    pub meshes: Vec<SourceMesh>,
    /// Bones that copy others ([`Skin::copies`]).
    pub copies: Vec<(usize, usize)>,
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
    /// Its vertices' centres of rotation ([`Centres`]); empty: not worked out yet.
    pub centres: Vec<[f32; 4]>,
    /// A FaceGen part's shape as the face moves it: its vertices are rewritten as the face changes.
    pub shape: Option<Arc<facegen::FaceShape>>,
}

pub(crate) struct SourceMaterial {
    pub class: shading::Class,
    pub params: [f32; 4],
    pub color: [f32; 4],
    pub alpha_test: bool,
    /// The emissive classes' multiplier (2 when the asset doesn't say).
    pub emissive: f32,
    /// The grazing-angle term is on (the deferred model's fresnel).
    pub fresnel: bool,
    pub bump: Option<Arc<asset::Texture>>,
    pub detail: Option<Arc<asset::Texture>>,
    pub env: Option<Arc<asset::Texture>>,
    /// A FaceGen face's texture morphs' texture and detail map ([`MeshExtras`]).
    pub face: Option<Arc<asset::Texture>>,
    pub face_detail: Option<Arc<asset::Texture>>,
    /// The hair's tint ([`asset::Shader::GamebryoHair`]).
    pub hair_tint: Option<[f32; 3]>,
}

/// How far inside its own skin the body under what's worn is drawn (metres, along its normals): more
/// than the vanilla gloves' fingers and hands are slimmer than the bare hand (up to 3.5 mm and,
/// but for a few corners, 6 mm), so its outline never shows round theirs.
const UNDER_INSET: f32 = 0.005;

/// A body put together from a game's parts (see [`rustvr_protocol::AvatarDesc`]): the skeleton
/// model's bones, and the meshes of every part (each skinned to those bones in their order), from
/// the asset cache folder `dir`, with the texture swaps applied (a part's texture or bump map
/// named first is drawn with the second, when the cache has it). A part made for another skeleton
/// (a body or an armour made for one race's skeleton, worn by another's) is put on this one by its
/// bones' names, as the game itself does ([`rebind`]); a part with bones the skeleton hasn't is left
/// out.
///
/// `skinning` says how it's skinned: with [`Skinning::Marked`], each part the marks name (by
/// the model path the cache's index gives its key) gets its plates. A part named with
/// [`rustvr_protocol::UNDER`] after its key is the body under what's worn: it goes after the others,
/// its joints rounded on its own (the bare body's), and the renderer draws it a little away from the
/// eye, so it shows only where nothing worn is in front of it.
///
/// With a `face` ([`rustvr_protocol::FaceDesc`]), the parts with face morphs in the cache (a FaceGen
/// head and the parts made to it: `morphs/<key>.rvf`) are moved to it first ([`facegen`]), the head
/// gets its texture morphs and detail map, and the hair its tint.
pub fn compose(dir: &std::path::Path, skeleton: &str, parts: &[String], swaps: &[(String, String)], face: Option<&facegen::Face>, skinning: Skinning) -> Result<AvatarSource> {
    let skel = cached::model(dir, skeleton).with_context(|| format!("the skeleton {skeleton}"))?;
    // (A swap to a texture the cache doesn't have keeps the part's own.)
    let have = |key: &str| dir.join("textures").join(format!("{key}.{}", asset::TEXTURE_EXT)).is_file();
    let swap = |k: &mut String| {
        if let Some((_, to)) = swaps.iter().find(|(from, to)| from == k && have(to)) {
            *k = to.clone();
        }
    };
    // The parts' names, for their marks.
    let names: HashMap<String, String> = match skinning {
        Skinning::Marked(_) => asset::load_index(dir).map(|i| i.items.into_iter().map(|i| (i.key, i.name)).collect()).unwrap_or_default(),
        _ => HashMap::new(),
    };
    let mut model = asset::Model { version: asset::FORMAT_VERSION, source: format!("{skeleton} with {} parts", parts.len()), bones: skel.bones.clone(), ..Default::default() };
    let mut extras: Vec<MeshExtras> = Vec::new();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut plates: Vec<armour::Plate> = Vec::new();
    let rest = rig::rest_pose(&model.bones);
    // (What's worn first, then the body under it, from this mesh on.)
    let (worn, under): (Vec<&String>, Vec<&String>) = parts.iter().partition(|k| !k.ends_with(rustvr_protocol::UNDER));
    let mut first_under = usize::MAX;
    let mut face_parts = 0usize;
    for (key, is_under) in worn.into_iter().map(|k| (k.as_str(), false)).chain(under.into_iter().map(|k| (k.trim_end_matches(rustvr_protocol::UNDER), true))) {
        if is_under && first_under == usize::MAX {
            first_under = model.meshes.len();
        }
        let part = match cached::model(dir, key) {
            Ok(p) => (*p).clone(),
            Err(e) => {
                tracing::warn!("avatar: part {key}: {e:#}");
                continue;
            }
        };
        let mut part = part;
        // The face, on a part made to it (before the part is put on this skeleton: its morphs are
        // in the part's own bind pose).
        let mut face_tex = None;
        // (Its shape with no face on it and each coordinate's move, kept for changing the face live.)
        let mut shape: Option<(usize, facegen::FaceShape)> = None;
        if let Some(f) = face
            && asset::has_morphs(dir, key)
        {
            match cached::morphs(dir, key) {
                Ok(m) => {
                    if let Some(mesh) = part.meshes.get_mut(m.mesh as usize) {
                        let rest = mesh.vertices.iter().map(|v| v.pos).collect();
                        facegen::morph_mesh(mesh, &m, &f.coords);
                        let fit = |set: &Vec<Vec<[f32; 3]>>| set.iter().map(|d| (0..mesh.vertices.len()).map(|i| d.get(i).copied().unwrap_or([0.0; 3])).collect()).collect();
                        shape = Some((m.mesh as usize, facegen::FaceShape { part: face_parts, rest, symmetric: fit(&m.symmetric), asymmetric: fit(&m.asymmetric) }));
                        face_parts += 1;
                    }
                    face_tex = m.texture.as_ref().map(|t| (m.mesh as usize, facegen::morph_texture(t, &f.coords.texture_symmetric)));
                }
                Err(e) => tracing::warn!("avatar: part {key}'s face morphs: {e:#}"),
            }
        }
        if part.bones.len() != model.bones.len() || part.bones.iter().zip(&model.bones).any(|(a, b)| a.name != b.name) {
            // (The face's moves go onto the skeleton as the vertices they move do.)
            if let Some((mesh, s)) = shape.as_mut()
                && let Ok(moves) = rebind_moves(&part, &model.bones)
                && let Some(moves) = moves.get(*mesh)
            {
                for (i, m) in moves.iter().enumerate() {
                    let Some(m) = m else { continue };
                    if let Some(p) = s.rest.get_mut(i) {
                        *p = m.transform_point3(glam::Vec3::from_array(*p)).to_array();
                    }
                    for d in s.symmetric.iter_mut().chain(s.asymmetric.iter_mut()).filter_map(|d| d.get_mut(i)) {
                        *d = m.transform_vector3(glam::Vec3::from_array(*d)).to_array();
                    }
                }
            }
            if let Err(e) = rebind(&mut part, &model.bones) {
                tracing::warn!("avatar: part {key} isn't skinned to the skeleton {skeleton}: {e:#}: left out");
                continue;
            }
        }
        let mut shape = shape.map(|(mesh, s)| (mesh, Arc::new(s)));
        for k in 0..3 {
            lo[k] = lo[k].min(part.bounds_min[k]);
            hi[k] = hi[k].max(part.bounds_max[k]);
        }
        // An eye: a bone of its own at its centre, on the bone it rides, so it turns as one.
        if let Some(side) = face.and_then(|f| f.eyes.iter().position(|e| e.as_deref() == Some(key))) {
            eye_bone(&mut model.bones, &mut part, side);
        }
        if let (Skinning::Marked(marks), Some(name), false) = (skinning, names.get(key), is_under)
            && let Some(piece) = marks.piece(name)
        {
            let mut marked = armour::plates(piece, &part, &rest);
            armour::cut_seams(&mut part, &mut marked.plates);
            for p in &marked.problems {
                tracing::warn!("avatar: {name}: {p}");
            }
            let list: Vec<String> = marked.plates.iter().map(|p| format!("{} ({}) on {}", p.name, p.side.name(), armour::riders(p, &part.bones))).collect();
            tracing::info!("avatar: {name} ({}): {} plates: {}", piece.name, marked.plates.len(), list.join("; "));
            let first = model.meshes.len();
            plates.extend(marked.plates.into_iter().map(|p| armour::Plate { mesh: p.mesh + first, ..p }));
        }
        for (i, mut m) in part.meshes.into_iter().enumerate() {
            swap(&mut m.texture);
            swap(&mut m.material.bump);
            swap(&mut m.material.detail);
            let mut x = MeshExtras::default();
            if shape.as_ref().is_some_and(|(mesh, _)| *mesh == i) {
                x.shape = shape.take().map(|(_, s)| s);
            }
            if let Some(f) = face {
                if face_tex.as_ref().is_some_and(|(mesh, _)| *mesh == i) {
                    x.face = face_tex.take().map(|(_, t)| t);
                }
                // The face's detail map (the game's age map) over the head's skin, and its normal map.
                if let Some((on, detail, normal)) = &f.detail
                    && *on == m.texture
                {
                    if !detail.is_empty() && have(detail) {
                        x.face_detail = Some(detail.clone());
                    }
                    if !normal.is_empty() && have(normal) {
                        m.material.bump = normal.clone();
                    }
                }
                if m.material.shader == asset::Shader::GamebryoHair {
                    x.hair_tint = Some(f.hair);
                }
            }
            model.meshes.push(m);
            extras.push(x);
        }
    }
    anyhow::ensure!(!model.meshes.is_empty(), "no part of the body could be loaded");
    (model.bounds_min, model.bounds_max) = (lo, hi);
    // How each vertex is skinned: the soft ones about their centres (found from the game's own
    // weights), then each plate's as one piece.
    let mut centres: Centres = model.meshes.iter().map(|m| vec![[0.0; 4]; m.vertices.len()]).collect();
    let first_under = first_under.min(model.meshes.len());
    if !matches!(skinning, Skinning::Linear) {
        // (What's worn and the body under it each on its own: the body under it, as bare, mustn't
        // change how the armour bends.)
        for range in [0..first_under, first_under..model.meshes.len()] {
            let meshes: Vec<(&[asset::Vertex], &[u32])> = model.meshes[range.clone()].iter().map(|m| (m.vertices.as_slice(), m.indices.as_slice())).collect();
            for (out, found) in centres[range].iter_mut().zip(cor::centres(&meshes)) {
                for (c, f) in out.iter_mut().zip(found) {
                    if let Some(p) = f {
                        *c = [p.x, p.y, p.z, 1.0];
                    }
                }
            }
        }
    }
    // The body under what's worn is flagged as such: 0.25 for a vertex blended linearly, 0.75 for
    // one turned about its centre. It's drawn a little inside its own skin, so its outline stays
    // within the armour's: a glove's fingers are often the bare hand's own, and others are a few
    // millimetres slimmer than it, where the bare hand's edge showed round every glove's.
    for c in centres[first_under..].iter_mut().flatten() {
        c[3] = if c[3] < 0.5 { 0.25 } else { 0.75 };
    }
    for m in &mut model.meshes[first_under..] {
        for v in &mut m.vertices {
            v.pos = (glam::Vec3::from_array(v.pos) - glam::Vec3::from_array(v.normal) * UNDER_INSET).to_array();
        }
    }
    // Plates are shaped at rest, so they ride their riders bound at rest: a copy of each rider
    // that moves between its bind pose and rest (an arm's bones), the copy below it.
    let mut copies: Vec<(usize, usize)> = Vec::new();
    // (The skeleton's bones, as `rest` has them: the eyes' own bones come after.)
    let skeleton_bones = rest.len();
    for p in &mut plates {
        for k in 0..4 {
            let b = p.bones[k] as usize;
            if p.weights[k] == 0 || b >= skeleton_bones || Mat4::from_cols_array(&model.bones[b].model_matrix).abs_diff_eq(rest[b], 1e-5) {
                continue;
            }
            let copy = match copies.iter().find(|c| c.1 == b) {
                Some(c) => c.0,
                None => {
                    let original = &model.bones[b];
                    let (_, rot, pos) = rest[b].to_scale_rotation_translation();
                    let bone = asset::Bone { name: format!("{} at rest", original.name), parent: b as i32, local_pos: pos.to_array(), local_rot: rot.to_array(), model_matrix: rest[b].to_cols_array() };
                    model.bones.push(bone);
                    copies.push((model.bones.len() - 1, b));
                    model.bones.len() - 1
                }
            };
            match u8::try_from(copy) {
                Ok(c) => p.bones[k] = c,
                Err(_) => tracing::warn!("avatar: past 256 bones: the plate {} rides {} as bound", p.name, model.bones[b].name),
            }
        }
    }
    for p in &plates {
        let mesh = &mut model.meshes[p.mesh];
        for (&i, &(pos, normal)) in p.vertices.iter().zip(&p.shape) {
            let v = &mut mesh.vertices[i as usize];
            (v.bones, v.weights) = (p.bones, p.weights);
            (v.pos, v.normal) = (pos.to_array(), normal.to_array());
            centres[p.mesh][i as usize] = [p.pivot.x, p.pivot.y, p.pivot.z, 1.0 + p.layer as f32];
        }
    }
    let mut textures = HashMap::new();
    for (m, x) in model.meshes.iter().zip(&extras) {
        for key in [&m.texture, &m.material.bump, &m.material.detail, &m.material.env].into_iter().chain(x.face_detail.as_ref()) {
            if key.is_empty() || textures.contains_key(key) {
                continue;
            }
            match cached::texture(dir, key) {
                Ok(t) => {
                    textures.insert(key.clone(), t);
                }
                Err(e) => tracing::warn!("avatar: texture {key}: {e:#}"),
            }
        }
    }
    Ok(AvatarSource::Composed { model, textures, extras, skinning: Some(Skin { centres, copies }) })
}

/// A body's face on its own ([`face_source`]): the FaceGen parts of a body put together with a face
/// (the head and what's made to it: ears, mouth, teeth, tongue, eyes, hair), each mesh with its
/// shape ([`MeshExtras::shape`]), its textures and shading, in the body's model space (bind pose);
/// and the bone that carries the head. For a copy of the head to shape by hand ([`HeldModel::face`]),
/// and the shapes for finding where a hand is on it.
#[derive(Clone)]
pub struct FaceSource {
    pub meshes: Vec<(asset::Mesh, MeshExtras)>,
    pub textures: HashMap<String, Arc<asset::Texture>>,
    /// The head's bone (the one most of the face's vertices ride), its name and bind pose (model
    /// space).
    pub head: (String, Mat4),
}

impl FaceSource {
    /// The face's shapes, by part ([`facegen::FaceShape::part`]), each with its mesh's triangles.
    pub fn shapes(&self) -> Vec<(Arc<facegen::FaceShape>, &[u32])> {
        let mut out: Vec<(Arc<facegen::FaceShape>, &[u32])> = self.meshes.iter().filter_map(|(m, x)| x.shape.clone().map(|s| (s, m.indices.as_slice()))).collect();
        out.sort_by_key(|(s, _)| s.part);
        out
    }
}

/// The face of a body [`compose`] put together with one, if it has one.
pub fn face_source(source: &AvatarSource) -> Option<FaceSource> {
    let AvatarSource::Composed { model, textures, extras, .. } = source else { return None };
    let meshes: Vec<(asset::Mesh, MeshExtras)> = model.meshes.iter().zip(extras).filter(|(_, x)| x.shape.is_some()).map(|(m, x)| (m.clone(), x.clone())).collect();
    if meshes.is_empty() {
        return None;
    }
    let mut weight = vec![0u64; model.bones.len()];
    for v in meshes.iter().flat_map(|(m, _)| &m.vertices) {
        for k in 0..4 {
            if let Some(w) = weight.get_mut(v.bones[k] as usize) {
                *w += v.weights[k] as u64;
            }
        }
    }
    let head = (0..weight.len()).max_by_key(|&b| weight[b])?;
    let bone = &model.bones[head];
    let keys: std::collections::HashSet<&String> = meshes.iter().flat_map(|(m, x)| [&m.texture, &m.material.bump, &m.material.detail, &m.material.env].into_iter().chain(x.face_detail.as_ref())).collect();
    let textures = textures.iter().filter(|(k, _)| keys.contains(k)).map(|(k, t)| (k.clone(), t.clone())).collect();
    Some(FaceSource { meshes, textures, head: (bone.name.clone(), Mat4::from_cols_array(&bone.model_matrix)) })
}

impl HeldModel {
    /// A face on its own ([`FaceSource`]) as a held model of one part, in the body's model space:
    /// drawn as the body draws it (its textures, the game's FaceGen blend, the hair's tint), its
    /// FaceGen parts reshaped live ([`HeldModel::set_face`]).
    pub fn face(gpu: &Gpu, face: &FaceSource) -> Result<HeldModel> {
        let model = asset::Model { source: "a face".into(), meshes: face.meshes.iter().map(|(m, _)| m.clone()).collect(), ..Default::default() };
        let extras = face.meshes.iter().map(|(_, x)| x.clone()).collect();
        let source = from_model_with(model, extras, None, |key| face.textures.get(key).cloned());
        let meshes = source
            .meshes
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let name = m.name.clone();
                (m, HeldMeshInfo { part: 0, source: i, name, special: false }, None)
            })
            .collect();
        render::HeldModel::new(gpu, meshes)
    }
}

/// What [`compose`] loads from an asset cache folder, kept as loaded: a character creator puts the
/// same parts together again at every slider's move, and the files are big (a head's texture morphs
/// are ten megabytes). Forgotten all at once past a few hundred things.
mod cached {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    type Map<T> = Mutex<Option<HashMap<(PathBuf, String), Arc<T>>>>;
    static MODELS: Map<asset::Model> = Mutex::new(None);
    static MORPHS: Map<asset::FaceMorphs> = Mutex::new(None);
    static TEXTURES: Map<asset::Texture> = Mutex::new(None);
    const KEPT: usize = 400;

    fn get<T>(map: &Map<T>, dir: &Path, key: &str, load: impl FnOnce() -> Result<T>) -> Result<Arc<T>> {
        let k = (dir.to_path_buf(), key.to_owned());
        if let Some(v) = map.lock().unwrap().as_ref().and_then(|m| m.get(&k)) {
            return Ok(v.clone());
        }
        let v = Arc::new(load()?);
        let mut m = map.lock().unwrap();
        let m = m.get_or_insert_with(HashMap::new);
        if m.len() >= KEPT {
            m.clear();
        }
        m.insert(k, v.clone());
        Ok(v)
    }

    pub fn model(dir: &Path, key: &str) -> Result<Arc<asset::Model>> {
        get(&MODELS, dir, key, || asset::load_model(dir, key))
    }

    pub fn morphs(dir: &Path, key: &str) -> Result<Arc<asset::FaceMorphs>> {
        get(&MORPHS, dir, key, || asset::load_morphs(dir, key))
    }

    pub fn texture(dir: &Path, key: &str) -> Result<Arc<asset::Texture>> {
        get(&TEXTURES, dir, key, || asset::load_texture(dir, key))
    }
}

/// Gives an eye (a rigid part on one bone: the head) a bone of its own at its centre, named as
/// rigs name eyes (`LeftEye`, `RightEye`: the rig turns them to look), its vertices moved onto it.
fn eye_bone(bones: &mut Vec<asset::Bone>, part: &mut asset::Model, side: usize) {
    use glam::{Mat4, Vec3};
    let verts = || part.meshes.iter().flat_map(|m| m.vertices.iter());
    let Some(first) = verts().next() else { return };
    let parent = first.bones[(0..4).max_by_key(|&k| first.weights[k]).unwrap_or(0)] as usize;
    let Some(p) = bones.get(parent) else { return };
    let (lo, hi) = verts().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| (lo.min(Vec3::from_array(v.pos)), hi.max(Vec3::from_array(v.pos))));
    let center = (lo + hi) * 0.5;
    let parent_bind = Mat4::from_cols_array(&p.model_matrix);
    let (_, rot, _) = parent_bind.to_scale_rotation_translation();
    let bind = Mat4::from_rotation_translation(rot, center);
    let local = parent_bind.inverse() * bind;
    let index = bones.len();
    if index > u8::MAX as usize {
        return;
    }
    bones.push(asset::Bone {
        name: ["LeftEye", "RightEye"][side].into(),
        parent: parent as i32,
        local_pos: local.w_axis.truncate().to_array(),
        local_rot: glam::Quat::IDENTITY.to_array(),
        model_matrix: bind.to_cols_array(),
    });
    for m in &mut part.meshes {
        for v in &mut m.vertices {
            (v.bones, v.weights) = ([index as u8, 0, 0, 0], [255, 0, 0, 0]);
        }
    }
    part.bones = bones.clone();
}

/// Puts a part skinned to another skeleton (`part.bones`) on `bones` by the bones' names: each
/// vertex goes where its bones take it from the part's bind pose to this skeleton's (the weighted
/// sum of each bone's move, as the game skins a part to whatever skeleton it's on), and its bone
/// indices become this skeleton's. An error when a bone the part is weighted to isn't in it.
pub fn rebind(part: &mut asset::Model, bones: &[asset::Bone]) -> Result<()> {
    use glam::Vec3;
    let by_name: HashMap<String, usize> = bones.iter().enumerate().map(|(i, b)| (b.name.to_ascii_lowercase(), i)).collect();
    let to: Vec<Option<usize>> = part.bones.iter().map(|b| by_name.get(&b.name.to_ascii_lowercase()).copied()).collect();
    let moves = rebind_moves(part, bones)?;
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (m, moves) in part.meshes.iter_mut().zip(moves) {
        for (v, sum) in m.vertices.iter_mut().zip(moves) {
            if let Some(sum) = sum {
                for k in 0..4 {
                    if v.weights[k] > 0 {
                        let t = to.get(v.bones[k] as usize).copied().flatten().context("a bone the skeleton hasn't")?;
                        v.bones[k] = u8::try_from(t).context("a bone past the 256th")?;
                    }
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

/// How [`rebind`] moves each vertex of a part onto the skeleton `bones` (by mesh, then vertex): the
/// weighted sum of its bones' moves off the part's bind pose onto the skeleton's (None: a vertex
/// no bone moves). An error when a bone a vertex is weighted to isn't the skeleton's.
pub fn rebind_moves(part: &asset::Model, bones: &[asset::Bone]) -> Result<Vec<Vec<Option<glam::Mat4>>>> {
    use glam::Mat4;
    let by_name: HashMap<String, usize> = bones.iter().enumerate().map(|(i, b)| (b.name.to_ascii_lowercase(), i)).collect();
    let to: Vec<Option<usize>> = part.bones.iter().map(|b| by_name.get(&b.name.to_ascii_lowercase()).copied()).collect();
    // Each of the part's bones' move: off its bind pose, onto this skeleton's.
    let moves: Vec<Option<Mat4>> = part
        .bones
        .iter()
        .zip(&to)
        .map(|(b, t)| t.map(|t| Mat4::from_cols_array(&bones[t].model_matrix) * Mat4::from_cols_array(&b.model_matrix).inverse()))
        .collect();
    part.meshes
        .iter()
        .map(|m| {
            m.vertices
                .iter()
                .map(|v| {
                    let total: u32 = v.weights.iter().map(|&w| w as u32).sum();
                    if total == 0 {
                        return Ok(None);
                    }
                    let mut sum = Mat4::ZERO;
                    for k in 0..4 {
                        if v.weights[k] == 0 {
                            continue;
                        }
                        let b = v.bones[k] as usize;
                        let Some(mv) = moves.get(b).copied().flatten() else {
                            anyhow::bail!("the skeleton has no {}", part.bones.get(b).map_or("(bone)", |b| b.name.as_str()));
                        };
                        sum += mv * (v.weights[k] as f32 / total as f32);
                    }
                    Ok(Some(sum))
                })
                .collect()
        })
        .collect()
}

/// The skeleton of the built-in mannequin (Unity humanoid names, bound in the reference T-pose),
/// for building models in memory ([`AvatarSource::Built`]).
pub fn mannequin_skeleton() -> Vec<asset::Bone> {
    procedural::mannequin().bones
}

/// Eye overlay shells (an `eyeballenv`, the cornea, tearline and occlusion meshes of Unreal
/// and Character Creator heads) are blended over the eyes in their games; drawn opaque they hide
/// the iris, so they're left out.
fn is_eye_overlay(mesh_name: &str) -> bool {
    let n = mesh_name.to_ascii_lowercase();
    (n.contains("eye") && n.contains("env")) || ["cornea", "tearline", "occlusion", "lacrimal"].iter().any(|k| n.contains(k))
}

fn load_model(dir: &std::path::Path, key: &str) -> Result<Source> {
    let model = asset::load_model(dir, key)?;
    let mut cache: HashMap<String, Option<Arc<asset::Texture>>> = HashMap::new();
    Ok(from_model(model, None, |key| {
        cache
            .entry(key.to_string())
            .or_insert_with(|| asset::load_texture(dir, key).map_err(|e| tracing::warn!("avatar: texture {key}: {e:#}")).ok().map(Arc::new))
            .clone()
    }))
}

/// A model with its textures looked up by key, and its vertices' centres of rotation (None: not
/// worked out yet).
fn from_model(model: asset::Model, centres: Option<Centres>, texture: impl FnMut(&str) -> Option<Arc<asset::Texture>>) -> Source {
    let n = model.meshes.len();
    from_model_with(model, vec![MeshExtras::default(); n], centres, texture)
}

/// [`from_model`], with what each mesh draws beside its material ([`MeshExtras`], by mesh).
fn from_model_with(model: asset::Model, extras: Vec<MeshExtras>, centres: Option<Centres>, mut texture: impl FnMut(&str) -> Option<Arc<asset::Texture>>) -> Source {
    let mut get = |key: &str| if key.is_empty() { None } else { texture(key) };
    let mut centres = centres.map(Vec::into_iter);
    let meshes = model
        .meshes
        .into_iter()
        .zip(extras.into_iter().chain(std::iter::repeat_with(MeshExtras::default)))
        .map(|(m, x)| (centres.as_mut().and_then(|c| c.next()).unwrap_or_default(), m, x))
        .filter(|(_, m, _)| !m.indices.is_empty() && !is_eye_overlay(&m.name))
        .map(|(centres, m, x)| {
            let texture = get(&m.texture);
            let material = shading::Class::of(m.material.shader).and_then(|class| {
                let mat = &m.material;
                let (bump, detail, env) = (get(&mat.bump), get(&mat.detail), get(&mat.env));
                let (need_bump, need_detail, need_env) = class.needs();
                let missing = texture.is_none() || (need_bump && bump.is_none()) || (need_detail && detail.is_none()) || (need_env && env.is_none());
                if missing {
                    tracing::debug!("avatar: mesh {}: {class:?} without all its textures; drawn with the base texture alone", m.name);
                    return None;
                }
                let emissive = if mat.emissive > 0.0 { mat.emissive } else { 2.0 };
                let face_detail = x.face_detail.as_deref().and_then(&mut get);
                Some(SourceMaterial {
                    class,
                    params: mat.params,
                    color: mat.color,
                    alpha_test: mat.alpha_test,
                    emissive,
                    fresnel: mat.fresnel,
                    bump,
                    detail,
                    env,
                    face: x.face.clone().map(Arc::new),
                    face_detail,
                    hair_tint: x.hair_tint,
                })
            });
            SourceMesh { name: m.name, vertices: m.vertices, indices: m.indices, texture, tint: [1.0; 3], material, depth_slack: 0.0, centres, shape: x.shape }
        })
        .collect();
    Source { bones: model.bones, meshes, copies: Vec::new() }
}
