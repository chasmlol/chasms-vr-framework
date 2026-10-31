//! RustVR shared-memory protocol: the VR runtime (`rustvr.exe`) <-> a game adapter (a DLL inside
//! the game, or the test range).
//!
//! This crate is the single source of truth for the byte layout. `include/rustvr_protocol.h` is
//! generated from it (see `tests/header.rs`), so C++ and C# adapters share the exact same layout.
//! Change anything here and bump [`VERSION`].
//!
//! # Who owns what
//!
//! The runtime owns the player's body: head, hands, what they hold, aiming, gestures, turning and
//! comfort. It runs entirely in the headset's **tracking space**. The game owns the world and the
//! outcomes: position after collision, damage, ammo, AI. The adapter SDK (`rustvr-adapter`) maps
//! tracking space into the game world each game frame (recentre, turning, roomscale, world scale),
//! so the runtime never needs the game's coordinates and the game never needs VR maths.
//!
//! # Spaces and units
//!
//! Everything is left-handed metres with X right, Y up and Z forward (the DirectX/Unity/4A
//! convention). Quaternions are `[x, y, z, w]`. Matrices are column-major `[f32; 16]` (glam's
//! layout): `m[12..15]` is the translation.
//!
//! * **Tracking space**: the headset's standing space. Y = 0 is the real floor.
//! * **Weapon space**: a gun's own model space, barrel along +Z. Adapters describe the gun with a
//!   [`WeaponRig`] in this space; the runtime answers with [`Intents::weapon_to_tracking`].
//!
//! # Mapping layout
//!
//! One file mapping (default name [`DEFAULT_MAPPING`]) created by the runtime:
//!
//! | Offset | Region | Direction | Access |
//! |---|---|---|---|
//! | [`OFF_HEADER`] | [`Header`] | both | atomics |
//! | [`OFF_SUBMIT`] | [`SubmitCtl`] | both | atomics (triple buffer) |
//! | [`OFF_TARGETS`] | [`Targets`] | runtime -> adapter | seqlock |
//! | [`OFF_RT_FRAME`] | [`RtFrame`] | runtime -> adapter | seqlock |
//! | [`OFF_GAME_STATE`] | [`GameState`] | adapter -> runtime | seqlock |
//! | [`OFF_SLOTS`] | [`SlotInfo`] x [`SLOT_COUNT`] | adapter -> runtime | owned by the slot holder |
//! | [`OFF_EVENTS`] | [`Event`] ring | adapter -> runtime | SPSC ring |
//! | [`OFF_BODY`] .. [`OFF_KICKS`] | the player rig's regions: see [`world`] | both | seqlock, SPSC ring |
//! | [`OFF_SOUNDS`] | [`SoundPack`]: the game's own sounds (see [`sound`]) | adapter -> runtime | seqlock |
//! | [`OFF_CASTERS`], [`OFF_OVERLAYS`] | [`Casters`] (runtime -> adapter), [`Overlays`] (adapter -> runtime): see [`magic`] | | seqlock |
//! | [`OFF_RIDE`], [`OFF_RT_RIDE`] | [`Ride`], [`RtRide`]: the mount the player rides (see [`ride`]) | both | seqlock |
//! | [`OFF_GORE`] | the game's blood, its creatures' stumps, limbs cut off (see [`gore`]) | adapter -> runtime | seqlock, SPSC ring |
//! | [`OFF_ACTOR_BODIES`], [`OFF_CREATURE_GRABS`] | [`ActorBodies`] (adapter -> runtime), [`CreatureGrabs`] (runtime -> adapter): see [`grab`] | | seqlock |
//! | [`OFF_BOWS`] | [`Bows`]: the bows' bend and their arrows' flight (see [`archery`]) | adapter -> runtime | seqlock |
//! | [`OFF_VOICE_SCENE`] .. [`OFF_VOICE_STREAM`] | [`VoiceScene`], [`VoiceEvents`] (adapter -> voice plugin), [`VoiceLines`], [`VoiceStream`] (voice plugin -> adapter): talking to the game's characters (see [`voice`]) | | seqlock, rings |
//!
//! The regions after the combat tables each follow the one before, 64-byte aligned: the avatar,
//! the item cards, the face, the magic, the ride, the gore, the creature grabs and the bows (a new
//! one goes after the last). Every region is listed in address order in `LAYOUT`, which the build
//! checks: none overlaps the next, and all lie inside [`MAPPING_BYTES`].
//!
//! # Frames
//!
//! 1. The runtime gets the predicted poses for the next headset frame and writes [`RtFrame`], then
//!    signals the `<mapping>_frame` event.
//! 2. The adapter maps tracking space into its world, renders both eyes (and the HUD, and a scope
//!    view if asked) into the textures of its private *back* slot, fills that slot's [`SlotInfo`]
//!    with the poses it actually used, and publishes the slot (see [`SubmitCtl`]). It then signals
//!    `<mapping>_submit`.
//! 3. The runtime takes the newest published slot, draws its own layers (panels, laser, scope lens)
//!    with the same poses and hands the result to the headset.
//!
//! Textures are D3D11 shared textures (NT handles, keyed mutex, key 0 used as a plain mutex by
//! both sides), created by the runtime and opened by name: see [`texture_name`].

#![allow(clippy::too_many_arguments)]

use bytemuck::{Pod, Zeroable};

#[cfg(windows)]
pub mod link;
pub mod archery;
pub mod avatar;
pub mod combat;
pub mod gore;
pub mod grab;
pub mod hud;
pub mod items;
pub mod sound;
pub mod world;
pub mod lighting;
pub mod magic;
pub mod player;
pub mod ride;
pub mod voice;

pub use archery::*;
pub use avatar::*;
pub use combat::*;
pub use gore::*;
pub use grab::*;
pub use hud::*;
pub use items::*;
pub use sound::*;
pub use world::*;
pub use lighting::*;
pub use magic::*;
pub use player::*;
pub use ride::*;
pub use voice::*;

pub const MAGIC: u32 = 0x5256_5552; // "RUVR"
pub const VERSION: u32 = 13;
pub const DEFAULT_MAPPING: &str = "Local\\RustVR_v13";
pub const MAPPING_BYTES: usize = 0x0052_0000;

// ---- region offsets ---------------------------------------------------------------------------
pub const OFF_HEADER: usize = 0x0000;
pub const OFF_SUBMIT: usize = 0x0080;
pub const OFF_TARGETS: usize = 0x0100;
pub const OFF_RT_FRAME: usize = 0x0200;
pub const OFF_GAME_STATE: usize = 0x0C00;
pub const OFF_SLOTS: usize = 0x1000;
pub const OFF_EVENTS: usize = 0x2000;

/// Number of texture slots in the triple buffer.
pub const SLOT_COUNT: usize = 3;
pub const SLOT_STRIDE: usize = 0x100;

/// Events ring: `u64` head (producer) at +0x00, `u64` tail (consumer) at +0x40, entries at +0x80.
pub const EVENT_RING_ENTRIES: usize = 256;
pub const RING_HEAD_OFF: usize = 0x00;
pub const RING_TAIL_OFF: usize = 0x40;
pub const RING_DATA_OFF: usize = 0x80;

/// A connected side is alive while its heartbeat is at most this old.
pub const HEARTBEAT_TIMEOUT_MS: u64 = 2000;

// ---- header -----------------------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Header {
    pub magic: u32,
    pub version: u32,
    pub runtime_pid: u32,
    pub adapter_pid: u32,
    /// `GetTickCount64()` at the runtime's / adapter's latest frame.
    pub runtime_heartbeat_ms: u64,
    pub adapter_heartbeat_ms: u64,
    /// Bumped by the adapter each time it (re)connects. The runtime resets the triple buffer and
    /// answers by copying it into `runtime_ack_epoch`; the adapter submits only after that.
    pub adapter_epoch: u32,
    pub runtime_ack_epoch: u32,
    pub mapping_bytes: u32,
    pub _pad: u32,
}

// ---- triple buffer ----------------------------------------------------------------------------
/// `state` bits 0-1: index of the *middle* slot. Bit 2 ([`SUBMIT_DIRTY`]): the middle slot holds a
/// frame the runtime hasn't taken yet.
///
/// * Adapter (writer) renders into its private back slot, then `back = xchg(state, back | DIRTY) & 3`.
/// * Runtime (reader), when `state & DIRTY`: `front = xchg(state, front) & 3`, then reads `front`.
///
/// After a (re)connect handshake the adapter's back slot is 0, the middle is 1 and the runtime's
/// front is 2.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SubmitCtl {
    pub state: u32,
    pub _pad: u32,
    /// Frames published by the adapter since it connected.
    pub published: u64,
}
pub const SUBMIT_DIRTY: u32 = 1 << 2;

// ---- texture targets --------------------------------------------------------------------------
/// Shared textures the runtime created for the adapter to render into (seqlock).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Targets {
    pub seq: u32,
    /// Textures are recreated (with new names) whenever this changes. 0: none yet.
    pub generation: u32,
    pub eye_w: u32,
    pub eye_h: u32,
    /// `DXGI_FORMAT` the colour textures were created with (a TYPELESS format; the adapter views
    /// them as UNORM, the runtime as UNORM_SRGB).
    pub color_format: u32,
    pub flags: u32, // TGT_*
    pub hud_w: u32,
    pub hud_h: u32,
    pub aux_w: u32,
    pub aux_h: u32,
    pub slots: u32,
    pub _pad: u32,
}
pub const TGT_DEPTH: u32 = 1 << 0;
pub const TGT_HUD: u32 = 1 << 1;
pub const TGT_AUX: u32 = 1 << 2;
/// The light atlas (`LIGHT_ATLAS_W` x `LIGHT_ATLAS_H` RGBA8, see [`lighting`]).
pub const TGT_LIGHTS: u32 = 1 << 3;
/// The game's post chain's light and lens per eye, for the player the runtime draws over the game's
/// finished picture to get them too. Two `R16G16B16A16_FLOAT` textures per eye ([`bloom_size`]):
///
/// * **bloom** ([`TexKind::BloomLeft`]): rgb, light the game adds to each pixel before its tone
///   curve (its bloom, through the lens dirt), and an eighth of after; a, the lens's warp in x;
/// * **lens** ([`TexKind::LensLeft`]): rgb, light added after the tone curve (a gas mask visor's
///   sheen); a, the warp in y.
///
/// The warp (in texture coordinates, at the screen position) is where the picture there comes from
/// (a visor's glass, heat haze): the game shows at `uv` what it drew at `uv + warp(uv)`.
pub const TGT_BLOOM: u32 = 1 << 4;

/// The aux view's depth ([`TexKind::AuxDepth`]: `R32_FLOAT`, linear metres along its view axis), as
/// the mirror's world needs it ([`MirrorView`]).
pub const TGT_AUX_DEPTH: u32 = 1 << 5;

/// The bloom and lens textures' size for eyes `w` x `h`: half each way (bloom is blurry; the lens
/// dirt it shows and the visor's warp less so).
pub fn bloom_size(w: u32, h: u32) -> (u32, u32) {
    ((w / 2).max(1), (h / 2).max(1))
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexKind {
    EyeLeft = 0,
    EyeRight = 1,
    /// `R32_FLOAT` linear view depth in metres (or the encoding in [`SlotInfo::depth_flags`]).
    DepthLeft = 2,
    DepthRight = 3,
    /// Premultiplied-alpha 2D layer: the game's HUD, or its whole picture while a menu is open.
    Hud = 4,
    /// Extra camera view, e.g. a magnified scope picture.
    Aux = 5,
    /// The light atlas: the game's light-map and shadow terms per light (see [`lighting`]).
    Lights = 6,
    /// The game's bloom (and the lens's warp in x) for each eye (see [`TGT_BLOOM`]).
    BloomLeft = 7,
    BloomRight = 8,
    /// The lens's light (and its warp in y) for each eye (see [`TGT_BLOOM`]).
    LensLeft = 9,
    LensRight = 10,
    /// The aux view's depth (see [`TGT_AUX_DEPTH`]).
    AuxDepth = 11,
}

impl TexKind {
    pub const ALL: [TexKind; 12] = [
        TexKind::EyeLeft,
        TexKind::EyeRight,
        TexKind::DepthLeft,
        TexKind::DepthRight,
        TexKind::Hud,
        TexKind::Aux,
        TexKind::Lights,
        TexKind::BloomLeft,
        TexKind::BloomRight,
        TexKind::LensLeft,
        TexKind::LensRight,
        TexKind::AuxDepth,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            TexKind::EyeLeft => "eyeL",
            TexKind::EyeRight => "eyeR",
            TexKind::DepthLeft => "depthL",
            TexKind::DepthRight => "depthR",
            TexKind::Hud => "hud",
            TexKind::Aux => "aux",
            TexKind::Lights => "lights",
            TexKind::BloomLeft => "bloomL",
            TexKind::BloomRight => "bloomR",
            TexKind::LensLeft => "lensL",
            TexKind::LensRight => "lensR",
            TexKind::AuxDepth => "auxDepth",
        }
    }
}

/// Name of a shared texture: `<mapping>_g<generation>_<kind>_<slot>`, opened with
/// `ID3D11Device1::OpenSharedResourceByName`.
pub fn texture_name(mapping: &str, generation: u32, kind: TexKind, slot: u32) -> String {
    format!("{mapping}_g{generation}_{}_{slot}", kind.tag())
}

/// Named auto-reset events: the runtime signals `frame` after each [`RtFrame`], the adapter signals
/// `submit` after each published slot.
pub fn event_name(mapping: &str, which: &str) -> String {
    format!("{mapping}_{which}")
}

// ---- shared small types -----------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Pose {
    pub pos: [f32; 3],
    /// Non-zero when tracked.
    pub valid: u32,
    pub rot: [f32; 4],
}

impl Pose {
    pub const IDENTITY: Pose = Pose { pos: [0.0; 3], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
}

/// Field of view as tangents of the half-angles: `left` and `down` are negative.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Fov {
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Controller {
    /// Palm centre.
    pub grip: Pose,
    /// Pointing ray, forward = +Z.
    pub aim: Pose,
    pub vel: [f32; 3],
    pub connected: u32,
    pub ang_vel: [f32; 3],
    pub buttons: u32, // CTRL_*
    pub trigger: f32,
    pub squeeze: f32,
    pub stick: [f32; 2],
}
pub const CTRL_A: u32 = 1 << 0; // A / X
pub const CTRL_B: u32 = 1 << 1; // B / Y
pub const CTRL_STICK_CLICK: u32 = 1 << 2;
pub const CTRL_MENU: u32 = 1 << 3;
pub const CTRL_TRIGGER_CLICK: u32 = 1 << 4;
pub const CTRL_SQUEEZE_CLICK: u32 = 1 << 5;
pub const CTRL_TOUCH_TRIGGER: u32 = 1 << 6;
pub const CTRL_TOUCH_A: u32 = 1 << 7;
pub const CTRL_TOUCH_B: u32 = 1 << 8;
pub const CTRL_TOUCH_STICK: u32 = 1 << 9;
/// The thumb rests on the thumbrest (Touch controllers).
pub const CTRL_TOUCH_REST: u32 = 1 << 10;

pub const HAND_LEFT: usize = 0;
pub const HAND_RIGHT: usize = 1;

// ---- runtime -> adapter, per headset frame ----------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RtFrame {
    pub seq: u32,
    pub flags: u32, // RT_*
    /// Increases by one per headset frame.
    pub frame_index: u64,
    /// Predicted display time of this frame, nanoseconds on the runtime's clock.
    pub display_time_ns: i64,
    /// Seconds since the previous frame (fixed 1/rate in lockstep tests).
    pub dt: f32,
    pub refresh_hz: f32,
    pub hmd: Pose,
    pub eyes: [Pose; 2],
    pub fov: [Fov; 2],
    pub hands: [Controller; 2],
    pub tracking: TrackingControl,
    pub intents: Intents,
    /// The headset's per-eye resolution (its recommended size times the render-scale setting): a
    /// game that renders the eyes itself sizes its frame to match.
    pub eye_size: [u32; 2],
    /// The mirror the player has up, and the camera the game's world in it is rendered from.
    pub mirror: MirrorView,
}

/// A mirror standing in the world (the runtime's `mirror`): where its glass is, and the camera to
/// render the game's world in it from. The camera is the head reflected in the glass's plane,
/// looking straight through the glass at the player, its frustum through the glass's corners and
/// its near plane on the glass (so what stands behind the mirror is clipped). Each eye's
/// reflection is drawn by the runtime from that eye reflected, with this picture (and its depth)
/// moved there.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct MirrorView {
    pub flags: u32, // MIR_*
    /// The near plane's distance from the camera (metres): the glass.
    pub near: f32,
    /// [`RtFrame::frame_index`] the camera is for (low 32 bits), as the slot's `aux_frame` says.
    pub frame: u32,
    /// How many times the mirror has been summoned or sent away with the game's own effect (the
    /// game's mirror spell: [`EV_MIRROR_CAST`]). When it changes, the game plays its effect at the
    /// glass and says when the effect's moment comes ([`EV_MIRROR_HIT`] with this count): the glass
    /// appears, or goes, then.
    pub summons: u32,
    /// The camera, tracking space, and its frustum.
    pub camera: Pose,
    pub fov: Fov,
    /// The glass: its centre, and its half extents to the right (as seen from in front) and up
    /// (tracking space, metres; w unused).
    pub center: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
}
/// The mirror is up.
pub const MIR_SHOWN: u32 = 1 << 0;
/// Render the game's world from [`MirrorView::camera`] into the aux texture, with its depth into
/// the aux depth ([`TGT_AUX_DEPTH`]), for this frame.
pub const MIR_RENDER: u32 = 1 << 1;
/// Tracking is valid and the runtime wants the game in VR.
pub const RT_ACTIVE: u32 = 1 << 0;
pub const RT_USER_PRESENT: u32 = 1 << 1;
/// Poses come from the simulated headset (tests).
pub const RT_SIMULATED: u32 = 1 << 2;
/// The runtime waits for every frame (tests): the adapter should render exactly once per
/// `frame_index`, using `dt` as its time step.
pub const RT_LOCKSTEP: u32 = 1 << 3;
/// The runtime's own menu has the controllers: the game should ignore gameplay intents.
pub const RT_RUNTIME_MENU: u32 = 1 << 4;
/// The headset's system dashboard is open.
pub const RT_DASHBOARD: u32 = 1 << 5;
/// The runtime draws the player's body and hands over the game's picture (the game shouldn't draw
/// its own first-person hands or arms).
pub const RT_DRAWS_BODY: u32 = 1 << 6;
/// The player has simple voice commands on: the adapter listens for its game's own phrases (a
/// menu's words, "what have you got for sale") with the system's speech recognition, no servers.
pub const RT_VOICE_COMMANDS: u32 = 1 << 7;

/// How the adapter maps tracking space into the game world. The mapping itself lives in the
/// adapter SDK because it must use the game's *current* anchor every game frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct TrackingControl {
    /// Recentre when this changes: the current head maps onto the game's eye, facing the game's
    /// look direction.
    pub recenter_seq: u32,
    pub flags: u32, // TRK_*
    /// Total turning (radians, wrapped to +-pi) requested so far; the adapter applies the change
    /// since the last value it saw.
    pub turn_total: f32,
    /// >1 makes the world feel bigger.
    pub world_scale: f32,
    /// Metres added to the eye height (setting).
    pub height_offset: f32,
    /// Metres added while the player is physically crouched and the game crouches too, so the
    /// view stays at the real head height.
    pub height_comp: f32,
    /// How far (metres) the head may move away from the game body before the view is held back.
    pub room_limit: f32,
    /// What carries the player (a mount: `TrackingMapper::carry_turn`) turns the view with it in
    /// steps of this many radians, each as one turn (0: smoothly, as it turns).
    pub carry_step: f32,
}
/// The head may move away from the game body (roomscale).
pub const TRK_ROOMSCALE: u32 = 1 << 0;
/// The runtime is walking the game body after the head: absorb that movement into the origin.
pub const TRK_FOLLOW: u32 = 1 << 1;
/// Keep the game's camera bob and shake.
pub const TRK_HEAD_BOB: u32 = 1 << 2;
/// Follow scripted camera animations (ladders, cutscenes).
pub const TRK_CAMERA_ANIMS: u32 = 1 << 3;

/// What the player is doing, worked out by the runtime from their body. All poses are in tracking
/// space; the adapter SDK converts them into the world.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Intents {
    /// Stick movement: x right, y forward, relative to the look direction, magnitude 0..1.
    pub move_xy: [f32; 2],
    /// Firing-hand trigger, 0..1.
    pub fire: f32,
    pub flags: u32, // INT_*
    /// Bit per [`Action`]: held right now.
    pub held: u64,
    /// Per [`Action`]: increases by one per press (wraps). Compare with the last value you saw, so a
    /// press is never missed even if the game skips frames.
    pub presses: [u16; 64],
    /// Weapon space -> tracking space: the gun in the player's hand (rigid, times the weapon scale).
    pub weapon_to_tracking: [f32; 16],
    /// Line of sight: shots start here and travel along `aim_dir`.
    pub sight_origin: [f32; 3],
    /// The game's aim zoom the runtime used (base FOV / current FOV).
    pub zoom: f32,
    pub aim_dir: [f32; 3],
    /// Metres between the sight line and the nearest eye (small = looking down the sights).
    pub sight_to_eye: f32,
    pub muzzle: [f32; 3],
    pub _pad0: f32,
    /// Scope camera to render into the aux texture while [`INT_SCOPE`] is set.
    pub scope_cam: Pose,
    pub scope_vfov_deg: f32,
    pub scope_aspect: f32,
    /// Render the scope view once every this many frames.
    pub scope_every: u32,
    pub _pad1: u32,
    /// Laser pointer on the game's menu panel, 0..1 (top-left origin), while [`INT_CURSOR`].
    pub cursor_uv: [f32; 2],
    /// Scrolling the game's menu (the dominant stick): notches a second, up positive, as a mouse
    /// wheel turns.
    pub scroll: f32,
    pub _pad2: f32,
}
pub const INT_WEAPON: u32 = 1 << 0;
pub const INT_TWO_HANDED: u32 = 1 << 1;
/// Aiming down the sights (sights at the eye, or the grip held).
pub const INT_ADS: u32 = 1 << 2;
pub const INT_SCOPE: u32 = 1 << 3;
/// The game's menu is shown as a panel: gameplay input is off, the cursor drives the menu.
pub const INT_MENU_MODE: u32 = 1 << 4;
pub const INT_CURSOR: u32 = 1 << 5;
pub const INT_CURSOR_DOWN: u32 = 1 << 6;
/// Manual reload: the magazine is out of the gun (hide it on the model).
pub const INT_MAG_OUT: u32 = 1 << 7;
/// Manual reload: the off hand holds a fresh magazine (draw one there).
pub const INT_HOLDING_MAG: u32 = 1 << 8;

/// Game actions the runtime can ask for. The adapter maps each to the game's own input.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Fire = 0,
    AimDownSights = 1,
    Jump = 2,
    /// Toggle crouch (stick click, or the player ducking / standing up).
    Crouch = 3,
    Sprint = 4,
    Use = 5,
    /// Use button held (Metro: journal and map).
    UseHold = 6,
    Reload = 7,
    /// Reload button held (Metro: change gas-mask filter).
    ReloadHold = 8,
    NextWeapon = 9,
    PrevWeapon = 10,
    Menu = 11,
    Flashlight = 12,
    /// Flashlight button held (Metro: charge the battery).
    FlashlightHold = 13,
    Throw = 14,
    Melee = 15,
    /// Off-hand wrist turned up in front of the eyes.
    Watch = 16,
    /// Grip squeezed with a hand at the eyes (Metro: gas mask on or off).
    GestureFace = 17,
    /// ... on top of the head (Metro: night vision).
    GestureHeadTop = 18,
    /// ... on the chest (Metro: medkit).
    GestureChest = 19,
    /// ... over the shoulder (Metro: next weapon).
    GestureShoulder = 20,
    /// ... at the hip (Metro: knife).
    GestureHip = 21,
    MenuAccept = 22,
    MenuBack = 23,
    MenuUp = 24,
    MenuDown = 25,
    MenuLeft = 26,
    MenuRight = 27,
    /// Grip held, hand flicked up (Titanfall 2 VR's titan abilities; any game's ability slots).
    AbilityUp = 28,
    AbilityDown = 29,
    AbilityLeft = 30,
    AbilityRight = 31,
    /// The weapon wheel is open (selecting by hand position).
    WeaponWheel = 32,
    /// The radial action menu is open.
    Inventory = 33,
    /// Grip squeezed with a hand at the mouth (Metro: the gas mask's filter changed).
    GestureMouth = 34,
    /// An open hand swept across the eyes (Metro: the gas mask wiped).
    GestureWipe = 35,
    /// Adapter-defined actions, picked from the radial menu.
    Custom0 = 40,
    Custom1 = 41,
    Custom2 = 42,
    Custom3 = 43,
    Custom4 = 44,
    Custom5 = 45,
    Custom6 = 46,
    Custom7 = 47,
}
/// One past the highest [Action] value (some values in between are unused).
pub const ACTION_COUNT: usize = 48;

impl Action {
    pub const ALL: &'static [Action] = &[
        Action::Fire,
        Action::AimDownSights,
        Action::Jump,
        Action::Crouch,
        Action::Sprint,
        Action::Use,
        Action::UseHold,
        Action::Reload,
        Action::ReloadHold,
        Action::NextWeapon,
        Action::PrevWeapon,
        Action::Menu,
        Action::Flashlight,
        Action::FlashlightHold,
        Action::Throw,
        Action::Melee,
        Action::Watch,
        Action::GestureFace,
        Action::GestureHeadTop,
        Action::GestureChest,
        Action::GestureShoulder,
        Action::GestureHip,
        Action::MenuAccept,
        Action::MenuBack,
        Action::MenuUp,
        Action::MenuDown,
        Action::MenuLeft,
        Action::MenuRight,
        Action::AbilityUp,
        Action::AbilityDown,
        Action::AbilityLeft,
        Action::AbilityRight,
        Action::WeaponWheel,
        Action::Inventory,
        Action::GestureMouth,
        Action::GestureWipe,
        Action::Custom0,
        Action::Custom1,
        Action::Custom2,
        Action::Custom3,
        Action::Custom4,
        Action::Custom5,
        Action::Custom6,
        Action::Custom7,
    ];

    pub fn bit(self) -> u64 {
        1u64 << (self as u32)
    }

    pub fn name(self) -> &'static str {
        match self {
            Action::Fire => "fire",
            Action::AimDownSights => "aim_down_sights",
            Action::Jump => "jump",
            Action::Crouch => "crouch",
            Action::Sprint => "sprint",
            Action::Use => "use",
            Action::UseHold => "use_hold",
            Action::Reload => "reload",
            Action::ReloadHold => "reload_hold",
            Action::NextWeapon => "next_weapon",
            Action::PrevWeapon => "prev_weapon",
            Action::Menu => "menu",
            Action::Flashlight => "flashlight",
            Action::FlashlightHold => "flashlight_hold",
            Action::Throw => "throw",
            Action::Melee => "melee",
            Action::Watch => "watch",
            Action::GestureFace => "gesture_face",
            Action::GestureHeadTop => "gesture_head_top",
            Action::GestureChest => "gesture_chest",
            Action::GestureShoulder => "gesture_shoulder",
            Action::GestureHip => "gesture_hip",
            Action::MenuAccept => "menu_accept",
            Action::MenuBack => "menu_back",
            Action::MenuUp => "menu_up",
            Action::MenuDown => "menu_down",
            Action::MenuLeft => "menu_left",
            Action::MenuRight => "menu_right",
            Action::AbilityUp => "ability_up",
            Action::AbilityDown => "ability_down",
            Action::AbilityLeft => "ability_left",
            Action::AbilityRight => "ability_right",
            Action::WeaponWheel => "weapon_wheel",
            Action::Inventory => "inventory",
            Action::GestureMouth => "gesture_mouth",
            Action::GestureWipe => "gesture_wipe",
            Action::Custom0 => "custom0",
            Action::Custom1 => "custom1",
            Action::Custom2 => "custom2",
            Action::Custom3 => "custom3",
            Action::Custom4 => "custom4",
            Action::Custom5 => "custom5",
            Action::Custom6 => "custom6",
            Action::Custom7 => "custom7",
        }
    }
}

// ---- adapter -> runtime, per game frame -------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GameState {
    pub seq: u32,
    pub flags: u32, // GS_*
    pub frame: u64,
    /// When the game's clock stood where `tracking` has the player (QueryPerformanceCounter ticks;
    /// 0: not known): the runtime carries the walk and turn on from it to when a frame is shown.
    pub time_qpc: i64,
    pub tracking: TrackingState,
    /// The game's current eye height above the player's feet, metres.
    pub eye_above_feet: f32,
    /// The game's aim zoom: base FOV / current FOV (1 when not zoomed).
    pub game_zoom: f32,
    pub rig: WeaponRig,
    pub request: TargetRequest,
    /// The game's lighting near the player, so what the runtime draws (hands, body) matches it.
    pub light: LightHint,
    /// Adapter-defined counters (diagnostics and tests): see the adapter's docs.
    pub stats: [u32; 24],
    pub stats_f: [f32; 16],
    /// Adapter name, UTF-8, NUL-terminated. Also selects the runtime's settings profile.
    pub name: [u8; 32],
    /// The part of the game's menu picture its menu is in (u0, v0, u1, v1, 0..1 from the top left;
    /// all zero: the whole picture), for a menu shown in the world ([`GS_MENU_IN_WORLD`]).
    pub menu_uv: [f32; 4],
}
pub const GS_IN_GAME: u32 = 1 << 0;
/// A game menu is open: the runtime shows the HUD texture as a big panel and points a laser at it.
pub const GS_MENU: u32 = 1 << 1;
pub const GS_LOADING: u32 = 1 << 2;
/// The game's camera is being driven by an animation or cutscene.
pub const GS_SCRIPTED: u32 = 1 << 3;
pub const GS_DEAD: u32 = 1 << 4;
pub const GS_CROUCHED: u32 = 1 << 5;
pub const GS_HAS_WEAPON: u32 = 1 << 6;
pub const GS_PAUSED: u32 = 1 << 7;
/// The game draws the player itself (from [RtPlayerDraw]: body, hands and what the runtime draws
/// of what they hold, inside the game's own frame); the runtime doesn't.
pub const GS_DRAWS_BODY: u32 = 1 << 8;
/// Magazines from the ammo pouch never run out.
pub const GS_INFINITE_AMMO: u32 = 1 << 9;
/// The game wants the runtime's player controller (hands, body) even with no objects registered:
/// what the player sees of themself is the runtime's, drawn over the game's picture.
pub const GS_PLAYER_RIG: u32 = 1 << 10;
/// The open menu is one the player stays in the world for (a character creator): the runtime keeps
/// the game's stereo picture and the body around its panel ([`GameState::menu_uv`]).
pub const GS_MENU_IN_WORLD: u32 = 1 << 11;
/// The game wants the mirror up (its character creator is open): the runtime stands it in front of
/// the player, and puts it away when this goes, unless the player had called it up.
pub const GS_MIRROR: u32 = 1 << 12;
/// The open menu is one the player can turn in (a conversation, its pieces with the speaker in the
/// world): the turning stick's left and right turn them as out of a menu, not the menu's.
pub const GS_MENU_TURNS: u32 = 1 << 13;

/// Linear-light colours.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct LightHint {
    /// World direction towards the sun (or the main light).
    pub sun_dir: [f32; 3],
    pub _p0: f32,
    pub sun_color: [f32; 3],
    pub _p1: f32,
    pub ambient: [f32; 3],
    pub _p2: f32,
}

/// The adapter's tracking -> world mapping as it was last applied (written by the adapter SDK).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct TrackingState {
    /// The tracking-space point that maps onto the game's eye anchor.
    pub origin: [f32; 3],
    /// World yaw (radians) of tracking-space forward.
    pub body_yaw: f32,
    /// Head offset from the game body in tracking XZ (metres), after the roomscale limit.
    pub room_offset: [f32; 2],
    pub world_per_meter: f32,
    pub applied_recenter: u32,
    /// The look angles given to the game (radians).
    pub look_yaw: f32,
    pub look_pitch: f32,
    pub applied_turn: f32,
    pub _pad: f32,
    /// World point (metres) the origin maps onto this frame: with origin, ody_yaw and
    /// world_per_meter it gives the full tracking -> world mapping.
    pub anchor: [f32; 3],
    pub _pad2: f32,
}

/// Points on the held gun in weapon space (metres, barrel +Z).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct WeaponRig {
    /// Changes whenever the gun (or its optic) changes.
    pub id: u64,
    pub flags: u32, // RIG_*
    pub _pad: u32,
    /// Firing hand's palm.
    pub grip: [f32; 3],
    pub _p0: f32,
    /// Off hand's grip for two-handed aiming.
    pub foregrip: [f32; 3],
    pub _p1: f32,
    pub sight: [f32; 3],
    pub _p2: f32,
    pub muzzle: [f32; 3],
    pub _p3: f32,
    pub mag_well: [f32; 3],
    pub _p4: f32,
    /// The exact line of sight, when the adapter knows it ([`RIG_SIGHT_LINE`]).
    pub sight_line_origin: [f32; 3],
    pub _p5: f32,
    pub sight_line_dir: [f32; 3],
    pub _p6: f32,
    /// Where the off hand grabs to rack the gun after a new magazine ([`RIG_HAS_CHARGING_HANDLE`]).
    pub charging_handle: [f32; 3],
    pub _p7: f32,
}
pub const RIG_HAS_SIGHT: u32 = 1 << 0;
pub const RIG_HAS_MUZZLE: u32 = 1 << 1;
pub const RIG_SIGHT_LINE: u32 = 1 << 2;
pub const RIG_HAS_MAG_WELL: u32 = 1 << 3;
/// The gun can be held with two hands.
pub const RIG_TWO_HANDED: u32 = 1 << 4;
pub const RIG_HAS_CHARGING_HANDLE: u32 = 1 << 5;

/// What the adapter wants to render into.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct TargetRequest {
    /// Colour `DXGI_FORMAT` (any member of the R8G8B8A8 or B8G8R8A8 family; 0 = R8G8B8A8).
    pub color_format: u32,
    pub flags: u32, // TGT_*
    /// Eye size the adapter needs (e.g. its back buffer). 0: the runtime picks from the headset.
    pub eye_w: u32,
    pub eye_h: u32,
    pub hud_w: u32,
    pub hud_h: u32,
    /// Square size of the aux (scope) texture.
    pub aux_size: u32,
    pub _pad: u32,
}

// ---- slots ------------------------------------------------------------------------------------
/// Describes one published texture slot. Written by the adapter before it publishes the slot,
/// read by the runtime after it takes it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SlotInfo {
    /// [`RtFrame::frame_index`] the eyes were rendered for.
    pub frame_index: u64,
    /// [`Targets::generation`] the textures belong to.
    pub generation: u32,
    pub flags: u32, // SLOT_*
    /// Tracking-space eye poses and FOVs the images were rendered with.
    pub eye_pose: [Pose; 2],
    pub fov: [Fov; 2],
    pub depth_near: f32,
    pub depth_far: f32,
    pub depth_flags: u32, // DEPTH_*
    /// Frame index the aux view was last rendered for.
    pub aux_frame: u32,
    /// Tracking-space camera the aux view was rendered from.
    pub aux_pose: Pose,
    /// Width / height of the HUD content.
    pub hud_aspect: f32,
    /// The HUD layout that says what this picture's HUD is (its `seq`, as
    /// [`crate::link::Link::hud_layout_seq`] gave it after it was written; 0: not said). The runtime cuts
    /// a picture's pieces by the layout it was drawn with, never one a frame older or newer.
    pub hud_layout: u32,
    pub _pad: [u32; 2],
    /// Where each eye's picture was in the world when it was rendered: the tracking -> world
    /// mapping it was drawn with. With the depth, the runtime moves the picture to where the head
    /// and the walking player are when it's shown.
    pub eye_map: [SlotMap; 2],
    /// The player the adapter drew into both eyes inside the game's own frame: the
    /// [`crate::RtPlayerDraw::frame_index`] it drew (0: none). Knowing the pose the player was drawn
    /// in, the runtime moves the player's own pixels to where the player is when the picture is
    /// shown.
    pub player_frame: u64,
    /// The object states the game's held things were placed by in both eyes (the runtime frame of
    /// [`crate::ObjectStates::frame_index`]; 0: none): the runtime moves their pixels with the hands
    /// as it does the player's.
    pub states_frame: u64,
}

/// A tracking -> world mapping (as [`TrackingState`] gives it): world = anchor + yaw(body_yaw) *
/// (tracking - origin) * world_per_meter. All zero: not given.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct SlotMap {
    pub origin: [f32; 3],
    pub body_yaw: f32,
    pub anchor: [f32; 3],
    pub world_per_meter: f32,
}

pub const SLOT_STEREO: u32 = 1 << 0;
pub const SLOT_HUD: u32 = 1 << 1;
/// The HUD texture holds the game's whole picture (a menu or loading screen) instead of an overlay.
pub const SLOT_MENU: u32 = 1 << 2;
pub const SLOT_AUX: u32 = 1 << 3;
pub const SLOT_DEPTH: u32 = 1 << 4;
/// Both eyes' bloom and lens textures hold the game's for their pictures.
pub const SLOT_BLOOM: u32 = 1 << 5;
/// The aux view is the mirror's world ([`MirrorView`], the camera of the frame `aux_frame`), with its
/// depth in the aux depth (linear metres).
pub const SLOT_MIRROR: u32 = 1 << 6;

/// Depth holds linear view distance in metres.
pub const DEPTH_LINEAR: u32 = 1 << 0;
/// Depth holds a reversed-Z projection's depth (near = 1).
pub const DEPTH_REVERSED: u32 = 1 << 1;

// ---- events -----------------------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Event {
    pub kind: u32, // EV_*
    pub a: u32,
    pub f: [f32; 6],
}
/// Vibrate: `a` = [`HAPTIC_LEFT`]..[`HAPTIC_OFF_HAND`], f0 seconds, f1 frequency (Hz), f2 amplitude 0..1.
pub const EV_HAPTIC: u32 = 1;
/// The game's own rumble (recoil, damage): f0 strength 0..1.
pub const EV_RUMBLE: u32 = 2;
/// Ask the runtime to recentre (e.g. after a level load).
pub const EV_RECENTER: u32 = 3;
/// The game's explosion pushes what the runtime simulates (its loose objects): f0..f2 the blast's
/// world position (metres), f3 its radius, f4 the impulse (N s) an object at the centre gets,
/// falling off to nothing at the radius.
pub const EV_BLAST: u32 = 4;
/// A push on one object the runtime simulates (a bullet hitting it): `a` its id, f0..f2 the world
/// impulse (N s), f3..f5 the world point it acts at.
pub const EV_IMPULSE: u32 = 5;
/// The game fired its held gun, `a` times since the last one (it counts its own rounds): in
/// legacy mode (the gun glued to the hand) the runtime kicks it with the game's recoil, the kicks
/// registered for [`KICK_OBJECT_HELD`].
pub const EV_GAME_SHOTS: u32 = 6;
/// The game's mirror spell was cast. A mirror the player is looking at is sent away; otherwise a
/// mirror is summoned in front of them (where the gesture would stand it). Either way the runtime
/// counts a summons ([`MirrorView::summons`]) and waits for the game's effect ([`EV_MIRROR_HIT`]).
pub const EV_MIRROR_CAST: u32 = 7;
/// The game's effect for the mirror's summons `a` ([`MirrorView::summons`]) reached its moment:
/// the glass appears (or goes) now, as a summoned creature appears at its effect's "Hit".
pub const EV_MIRROR_HIT: u32 = 8;

pub const HAPTIC_LEFT: u32 = 0;
pub const HAPTIC_RIGHT: u32 = 1;
pub const HAPTIC_FIRING_HAND: u32 = 2;
pub const HAPTIC_OFF_HAND: u32 = 3;

// ---- layout checks ----------------------------------------------------------------------------
const _: () = {
    assert!(size_of::<Header>() == 48);
    assert!(size_of::<SubmitCtl>() == 16);
    assert!(size_of::<Targets>() == 48);
    assert!(size_of::<Pose>() == 32);
    assert!(size_of::<Fov>() == 16);
    assert!(size_of::<Controller>() == 112);
    assert!(size_of::<TrackingControl>() == 32);
    assert!(size_of::<Intents>() == 328);
    assert!(size_of::<RtFrame>() == 752 + 112);
    assert!(size_of::<MirrorView>() == 112);
    assert!(size_of::<TrackingState>() == 64);
    assert!(size_of::<WeaponRig>() == 144);
    assert!(size_of::<TargetRequest>() == 32);
    assert!(size_of::<GameState>() == 528);
    assert!(size_of::<LightHint>() == 48);
    assert!(size_of::<SlotInfo>() == 256);
    assert!(size_of::<SlotMap>() == 32);
    assert!(size_of::<Event>() == 32);
    assert!(OFF_HEADER + size_of::<Header>() <= OFF_SUBMIT);
    assert!(OFF_SUBMIT + size_of::<SubmitCtl>() <= OFF_TARGETS);
    assert!(OFF_TARGETS + size_of::<Targets>() <= OFF_RT_FRAME);
    assert!(OFF_RT_FRAME + size_of::<RtFrame>() <= OFF_GAME_STATE);
    assert!(OFF_GAME_STATE + size_of::<GameState>() <= OFF_SLOTS);
    assert!(size_of::<SlotInfo>() <= SLOT_STRIDE);
    assert!(OFF_SLOTS + SLOT_STRIDE * SLOT_COUNT <= OFF_EVENTS);
    assert!(OFF_EVENTS + RING_DATA_OFF + EVENT_RING_ENTRIES * size_of::<Event>() <= MAPPING_BYTES);
    assert!(EVENT_RING_ENTRIES.is_power_of_two());
    assert!(ACTION_COUNT <= 64);
    // Seqlocked structs are copied 4 bytes at a time, after their leading `seq` word.
    assert!(size_of::<RtFrame>().is_multiple_of(4) && size_of::<GameState>().is_multiple_of(4) && size_of::<Targets>().is_multiple_of(4));
};

// ---- the whole mapping --------------------------------------------------------------------------
/// Every region of the mapping in address order, (offset, bytes): each ends before the next starts,
/// and the last inside the mapping, as the build checks below. A region added goes here too.
const LAYOUT: &[(usize, usize)] = &[
    (OFF_HEADER, size_of::<Header>()),
    (OFF_SUBMIT, size_of::<SubmitCtl>()),
    (OFF_TARGETS, size_of::<Targets>()),
    (OFF_RT_FRAME, size_of::<RtFrame>()),
    (OFF_GAME_STATE, size_of::<GameState>()),
    (OFF_SLOTS, SLOT_STRIDE * SLOT_COUNT),
    (OFF_EVENTS, RING_DATA_OFF + EVENT_RING_ENTRIES * size_of::<Event>()),
    (OFF_BODY, size_of::<RtBody>()),
    (OFF_OBJECTS, size_of::<ObjectTable>()),
    (OFF_OBJECT_STATES, size_of::<ObjectStates>()),
    (OFF_HAND_POSES, size_of::<HandPoseTable>()),
    (OFF_COLLISION, COLLISION_RING_BYTES),
    (OFF_KICKS, size_of::<KickTable>()),
    (OFF_SOUNDS, size_of::<SoundPack>()),
    (OFF_GAME_LIGHTING, size_of::<GameLighting>()),
    (OFF_PLAYER_DRAW, size_of::<RtPlayerDraw>()),
    (OFF_GAME_POSES, size_of::<GamePoses>()),
    (OFF_GAME_PUSHES, size_of::<GamePushes>()),
    (OFF_GAME_HULLS, size_of::<GameHulls>()),
    (OFF_ACTORS, size_of::<Actors>()),
    (OFF_MELEE, size_of::<MeleeTable>()),
    (OFF_SPELLS, size_of::<Spells>()),
    (OFF_COMBAT_EVENTS, size_of::<CombatEvents>()),
    (OFF_AVATAR, size_of::<AvatarDesc>()),
    (OFF_ITEM_INFO, size_of::<ItemInfos>()),
    (OFF_FACE, size_of::<FaceDesc>()),
    (OFF_CASTERS, size_of::<Casters>()),
    (OFF_OVERLAYS, size_of::<Overlays>()),
    (OFF_RIDE, size_of::<Ride>()),
    (OFF_RT_RIDE, size_of::<RtRide>()),
    (OFF_BLOODS, size_of::<BloodTable>()),
    (OFF_GORE_BODIES, size_of::<GoreBodies>()),
    (OFF_GORE_EVENTS, size_of::<GoreEvents>()),
    (OFF_ACTOR_BODIES, size_of::<ActorBodies>()),
    (OFF_CREATURE_GRABS, size_of::<CreatureGrabs>()),
    (OFF_BOWS, size_of::<Bows>()),
    (OFF_VOICE_SCENE, size_of::<VoiceScene>()),
    (OFF_VOICE_EVENTS, size_of::<VoiceEvents>()),
    (OFF_VOICE_LINES, size_of::<VoiceLines>()),
    (OFF_VOICE_STREAM, size_of::<VoiceStream>()),
    (OFF_HUD_LAYOUT, size_of::<HudLayout>()),
];

const _: () = {
    let mut i = 1;
    while i < LAYOUT.len() {
        assert!(LAYOUT[i - 1].0 + LAYOUT[i - 1].1 <= LAYOUT[i].0, "two regions of the mapping overlap");
        i += 1;
    }
    let (last, bytes) = LAYOUT[LAYOUT.len() - 1];
    assert!(last + bytes <= MAPPING_BYTES);
};

/// The test range's [`GameState::stats`] / [`GameState::stats_f`] layout. The runtime's headless
/// scenarios check these to see what the game actually did.
pub mod range_stats {
    pub const SHOTS: usize = 0;
    pub const HITS: usize = 1;
    pub const RELOADS: usize = 2;
    pub const WEAPON: usize = 3;
    pub const AMMO: usize = 4;
    pub const CROUCHED: usize = 5;
    pub const JUMPS: usize = 6;
    pub const GESTURE_FACE: usize = 7;
    pub const GESTURE_HEAD_TOP: usize = 8;
    pub const GESTURE_CHEST: usize = 9;
    pub const GESTURE_SHOULDER: usize = 10;
    pub const GESTURE_HIP: usize = 11;
    pub const MENU_TOGGLES: usize = 12;
    pub const MENU_CLICKS: usize = 13;
    pub const THROWS: usize = 14;
    pub const SWITCHES: usize = 15;
    pub const MELEES: usize = 16;
    pub const ABILITY_UP: usize = 17;
    pub const ABILITY_DOWN: usize = 18;
    pub const ABILITY_LEFT: usize = 19;
    pub const ABILITY_RIGHT: usize = 20;
    pub const USES: usize = 21;
    /// Physical mode: objects the runtime reports.
    pub const OBJECTS: usize = 22;
    /// Physical mode: 1 + the index of the gun in a hand (0 = none).
    pub const HELD: usize = 23;

    pub const F_FEET_X: usize = 0;
    pub const F_FEET_Z: usize = 1;
    /// Distance (m) from the last hit to the target's centre.
    pub const F_LAST_HIT_ERROR: usize = 2;
    pub const F_LOOK_YAW: usize = 3;
    /// Angle (degrees) between the last shot and the runtime's aim direction.
    pub const F_SHOT_AIM_ERROR: usize = 4;
    pub const F_FRAME_MS: usize = 5;
    pub const F_SCOPE_FRAMES: usize = 6;
    /// Rounds in a full magazine of the held weapon.
    pub const F_MAG_SIZE: usize = 7;
    /// Physical mode: grenades that went off, the target plates their blasts reached, and knives
    /// stuck in a target plate.
    pub const F_EXPLOSIONS: usize = 8;
    pub const F_BLAST_HITS: usize = 9;
    pub const F_KNIFE_HITS: usize = 10;
    /// Visual effects alive (muzzle flashes, smoke, tracers, bullet holes, casings, debris).
    pub const F_FX_LIVE: usize = 11;
    /// The runtime's melee hits on the dummy, those that were power hits, its casts, and those cast
    /// at the dummy (`range/src/combat.rs`).
    pub const F_MELEE_HITS: usize = 12;
    pub const F_MELEE_POWER: usize = 13;
    pub const F_CASTS: usize = 14;
    pub const F_CASTS_AT_DUMMY: usize = 15;
}

/// Reads a NUL-terminated UTF-8 name field.
pub fn name_str(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).unwrap_or("")
}

/// Writes `s` into a fixed name field, truncated on a character boundary and NUL-terminated.
pub fn set_name(dst: &mut [u8], s: &str) {
    dst.fill(0);
    let mut n = s.len().min(dst.len().saturating_sub(1));
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    dst[..n].copy_from_slice(&s.as_bytes()[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let mut buf = [0u8; 8];
        set_name(&mut buf, "metro2033redux");
        assert_eq!(name_str(&buf), "metro20");
        set_name(&mut buf, "range");
        assert_eq!(name_str(&buf), "range");
    }

    #[test]
    fn texture_names_are_unique() {
        let mut all = std::collections::HashSet::new();
        for kind in TexKind::ALL {
            for slot in 0..SLOT_COUNT as u32 {
                assert!(all.insert(texture_name(DEFAULT_MAPPING, 7, kind, slot)));
            }
        }
    }

    #[test]
    fn action_bits_are_unique_and_in_range() {
        let mut seen = 0u64;
        for a in Action::ALL {
            assert!((*a as usize) < ACTION_COUNT);
            assert_eq!(a.bit(), 1 << (*a as u32));
            assert_eq!(seen & a.bit(), 0, "{a:?} reuses a bit");
            seen |= a.bit();
            assert!(!a.name().is_empty());
        }
    }
}
