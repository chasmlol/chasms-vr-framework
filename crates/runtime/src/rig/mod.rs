//! The player rig: physics hands and everything they do with the game's objects. Grabbing,
//! two-handed guns, throwing, grenades and throwing knives, holsters and pouches, the ammo pouch,
//! physical reloads, recoil and gravity gloves, simulated with Rapier in the game's world.
//!
//! The research's rule ("pin the hands, cheat everything else") shapes it:
//!
//! * Hands follow the controllers exactly in free space. Walls stop them, and a ghost hand then
//!   shows where the real hand is. They snap back past a 0.7 m leash.
//! * Grabs are generous and reliable. There's a 7 cm reach from the palm, the grip has press/release
//!   hysteresis, guns snap into the hand the way they're held, and the ammo pouch already knows
//!   your gun.
//! * Held objects collide with the world but soften against it instead of fighting it. Throws use
//!   the hand's speed at its peak before the release, not the release frame (`throwing`), and
//!   grenades and knives get the help the research recommends (`throwable`).
//! * Guns keep their own state (magazine, chamber, slide). The game only sees each shot.
//!
//! Physics runs in world metres. Held and holstered objects are reported in tracking space, so the
//! adapter maps them with the same view as the hands and they never trail behind while walking.

pub mod actions;
pub mod feedback;
pub mod filters;
pub mod fingers;
pub mod valve;
pub mod kick;
pub mod physics;
pub mod recoil;
pub mod throwable;
pub mod throwing;
pub mod weapon;
pub mod zones;

use glam::{EulerRot, Mat3, Quat, Vec3};
use rapier3d::prelude::{ColliderHandle, RigidBodyHandle};
use rustvr_protocol::*;
use std::collections::{BTreeMap, HashMap};

use crate::backend::HmdFrame;
use crate::settings::{GrenadeArming, Settings};
use actions::{Chamber, Cycle, CycleEvent, HingeEvent};
use feedback::{Cue, Feedback};
use filters::OneEuro;
use physics::{Physics, groups_free, groups_game_prop, groups_hand, groups_held, groups_none};
use throwable::{Flight, Fuse, KnifeMode, ThrowState};
use throwing::{ThrowClass, ThrowHistory};
use weapon::{Fire, HandleGrab, WeaponState};
use zones::{BodyAnchor, Zone, ammo_zones, slot_fits, slot_zones};

/// Tracking space -> world, as the adapter last reported it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldMap {
    pub origin: Vec3,
    pub body_yaw: f32,
    pub world_per_meter: f32,
    pub anchor: Vec3,
}

impl WorldMap {
    pub const IDENTITY: WorldMap = WorldMap { origin: Vec3::ZERO, body_yaw: 0.0, world_per_meter: 1.0, anchor: Vec3::ZERO };

    pub fn from_state(t: &TrackingState) -> WorldMap {
        WorldMap {
            origin: Vec3::from_array(t.origin),
            body_yaw: t.body_yaw,
            world_per_meter: if t.world_per_meter > 0.0 { t.world_per_meter } else { 1.0 },
            anchor: Vec3::from_array(t.anchor),
        }
    }

    /// A slot's mapping (where an eye's picture was in the world).
    pub fn from_slot(m: &SlotMap) -> WorldMap {
        WorldMap { origin: Vec3::from_array(m.origin), body_yaw: m.body_yaw, world_per_meter: if m.world_per_meter > 0.0 { m.world_per_meter } else { 1.0 }, anchor: Vec3::from_array(m.anchor) }
    }

    /// Tracking -> world as a matrix.
    pub fn matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_translation(self.anchor) * glam::Mat4::from_rotation_y(self.body_yaw) * glam::Mat4::from_scale(Vec3::splat(self.world_per_meter)) * glam::Mat4::from_translation(-self.origin)
    }

    fn yaw(&self) -> Quat {
        Quat::from_rotation_y(self.body_yaw)
    }

    pub fn point(&self, p: Vec3) -> Vec3 {
        self.anchor + self.yaw() * ((p - self.origin) * self.world_per_meter)
    }

    pub fn rot(&self, q: Quat) -> Quat {
        self.yaw() * q
    }

    pub fn vel(&self, v: Vec3) -> Vec3 {
        self.yaw() * v * self.world_per_meter
    }

    pub fn inv_point(&self, w: Vec3) -> Vec3 {
        self.origin + self.yaw().inverse() * (w - self.anchor) / self.world_per_meter
    }

    pub fn inv_rot(&self, q: Quat) -> Quat {
        self.yaw().inverse() * q
    }
}

/// Where an object is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Holder {
    Free,
    Held(usize),
    Holstered(u32),
    InWeapon(u32),
    /// Flying to a hand (gravity gloves): which hand, launch time, launch distance.
    Pulled { hand: usize, t0: f64, dur: f32, gravity: f32 },
    /// Gliding back to its holster after being let go.
    Returning { slot: u32, t0: f64, from: (Vec3, Quat) },
    /// Stuck in something (a knife's blade, a sticky bomb): this pose relative to object `parent`
    /// (0: the world, and the pose is the world's).
    Stuck { parent: u32, rel: (Vec3, Quat) },
    /// Went off: inert until the game puts it back.
    Gone,
}

/// How a held object sits in the hand.
#[derive(Clone, Copy, Debug)]
enum Attach {
    /// Guns: grip on the palm, barrel along the controller's aim.
    Gun,
    /// Anything else: this pose relative to the controller grip (grab where you touched it).
    Fixed(Vec3, Quat),
}

struct Obj {
    desc: ObjectDesc,
    /// The desc it's drawn with (itself, or its template).
    draw_desc: u32,
    body: RigidBodyHandle,
    collider: ColliderHandle,
    holder: Holder,
    attach: Attach,
    /// Ease into the hand from here (pose, start time).
    /// Gliding into the hand that took it: from where it was, since when, and how far it goes.
    blend: Option<(Vec3, Quat, f64, f32)>,
    spawn_seq: u32,
    spawned: bool,
    free_since: f64,
    home_slot: Option<u32>,
    rounds: u32,
    weapon: Option<WeaponState>,
    hovered: bool,
    /// Ignore the hands until then (just thrown).
    hands_off_until: f64,
    /// A hand has had it (held or pulled): a game's prop the runtime draws stays the runtime's to
    /// draw from then on ([`OS_RUNTIME_DRAWN`]).
    handled: bool,
    /// Touching the world or another object (a held one knocking into something is felt).
    in_contact: bool,
    /// Held: where the player's walk and turn take it this step, and the share of its velocity the
    /// last one gave it (`physics::Drive::carry`, `carried`).
    carry: Option<(Vec3, Quat)>,
    carried: (Vec3, Vec3),
    /// Held: going through the world for now (`SLIP`).
    through: bool,
    /// Velocity after the last step, for impact sounds.
    prev_vel: Vec3,
    /// A magazine just taken out of a gun can't seat again until it has left the well.
    seat_armed: bool,
    next_impact: f64,
    /// Grenades, knives and other throwables.
    throw: Option<ThrowState>,
    /// What a hanging object hangs from (`OBJF_HANGING`): the fixed anchor its joint holds.
    hang: Option<RigidBodyHandle>,
    /// A valve's wheel ([`OBJ_VALVE`]), and the part of it a hand takes (points in the wheel's
    /// frame; empty: its rim), from the model the runtime draws it with ([`Rig::set_valve_shape`]).
    valve: Option<valve::Valve>,
    valve_shape: Vec<Vec3>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hover {
    None,
    Object(u32),
    Slot(u32),
    Ammo(u32),
    Foregrip(u32),
    Handle(u32),
    /// The other hand's grenade's pin.
    Pin(u32),
    /// The other hand's gun's open barrels (or swung-out cylinder), to swing them shut (or further).
    Barrels(u32),
    /// A live round in one of the other hand's gun's open chambers, to pull out.
    Chamber(u32, usize),
    /// A valve's rim.
    Valve(u32),
}

/// A hand holding a hinged action's barrels: which gun, where on them it took them (weapon space,
/// shut) and how far open they were then.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BarrelGrab {
    gun: u32,
    at: Vec3,
    angle: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Glove {
    Idle,
    Hover(u32),
    Locked(u32),
    Flight(u32),
}

struct Hand {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    /// Thumb to pinky: kinematic capsules that push loose things ([`fingers`]), where each was
    /// last step and how fast it moved.
    fingers: [RigidBodyHandle; 5],
    finger_at: [Vec3; 5],
    finger_vel: [Vec3; 5],
    /// The fingers push things only while the hand has nothing (holding, steering or working
    /// something, they're on it); and they knock loose things about only while it isn't about to
    /// take one (they'd nudge it out of the grab).
    fingers_on: bool,
    fingers_solid: bool,
    grip_down: bool,
    grip_toggled: bool,
    prev_buttons: u32,
    trigger: f32,
    holding: Option<u32>,
    support: Option<u32>,
    handle: Option<u32>,
    hover: Hover,
    glove: Glove,
    /// A target the gloves are about to take (or none), and since when.
    glove_pending: Option<(Option<u32>, f64)>,
    /// When the grip let go of a locked target (it holds on a moment).
    glove_released: Option<f64>,
    /// When a locked target next strains towards the hand (0: it hasn't yet).
    glove_hop: f64,
    history: ThrowHistory,
    /// Pulling the other hand's grenade's pin: which, and where the hand took it.
    pin: Option<(u32, Vec3)>,
    /// Holding the other hand's gun's open barrels.
    barrels: Option<BarrelGrab>,
    /// On a valve's rim.
    valve: Option<valve::ValveGrip>,
    /// The controller's turn rate (world, rad/s) from its last two rotations: the wrist's flicks
    /// (whatever a heavy gun in it manages to follow).
    turn_rate: Vec3,
    trigger_was: f32,
    /// The hardest squeeze since the grip closed, and whether an easing grip let a throw go (it
    /// must open past the release threshold before it can grab again).
    squeeze_peak: f32,
    eased: bool,
    euro: OneEuro,
    blocked: bool,
    /// Controller grip target in the world, and the aim rotation.
    target: (Vec3, Quat),
    aim_rot: Quat,
    /// Tracking-space controller grip pose.
    grip_t: (Vec3, Quat),
    aim_t: (Vec3, Quat),
    vel_t: Vec3,
    ang_t: Vec3,
    leashed_until: f64,
    /// Letting go of something that held the hand away from its controller (a valve's rim): it
    /// goes back to the controller from there (position, rotation), since when, over how long.
    returning: Option<(Vec3, Quat, f64, f32)>,
}

/// What the rest of the runtime needs from the rig this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct RigOutput {
    pub hands: [HandState; 2],
    /// The controller pose to draw a ghost hand at, when the physics hand is held back.
    pub ghosts: [Option<Pose>; 2],
    /// The gun in the firing hand (or any held gun), tracking space: for aiming down sights,
    /// scopes and the adapter's legacy weapon fields.
    pub primary_weapon: Option<RigWeapon>,
    /// Hands busy with an object (their buttons belong to the rig, not the game).
    pub hand_busy: [bool; 2],
    /// Hands pointing at something they could grab or pull (the trigger shouldn't "use").
    pub hand_targeting: [bool; 2],
    /// Objects flying to each hand (gravity gloves), tracking space.
    pub pulled: [Option<Vec3>; 2],
    /// How each hand's fingers wrap what it holds.
    pub holds: [rustvr_ik::HoldShape; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct RigWeapon {
    pub id: u32,
    pub weapon_to_tracking: glam::Mat4,
    /// Weapon-space line of sight (point and direction) and muzzle.
    pub sight: Vec3,
    pub muzzle: Vec3,
    pub two_handed: bool,
    pub scoped: bool,
    /// A laser fitted and on (`WPN_LASER`): where it shines from (weapon space, along +Z).
    pub laser: Option<Vec3>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RigDebug {
    pub grabs: u32,
    pub throws: u32,
    pub holstered: u32,
    pub drawn: u32,
    pub mags_spawned: u32,
    pub seats: u32,
    pub ejects: u32,
    pub racks: u32,
    pub shots: u32,
    /// Shots that kicked the way the gun's game makes it (its kicks).
    pub game_kicks: u32,
    pub dry_fires: u32,
    pub pulls: u32,
    pub catches: u32,
    pub returns: u32,
    pub last_throw_speed: f32,
    /// The last throw's gain and how long before the release its peak was (ms).
    pub last_throw_gain: f32,
    pub last_throw_peak_ms: f32,
    pub blocked_frames: u32,
    pub pins: u32,
    pub arms: u32,
    pub detonations: u32,
    pub knife_throws: u32,
    pub sticks: u32,
    /// Where the last grenade went off, and where the last knife's tip went in (tracking space).
    pub last_detonation: Option<Vec3>,
    pub last_stick: Option<Vec3>,
    /// Hinged actions opened and latched shut; rounds pushed into chambers and pulled out; cases
    /// (or rounds) thrown out of chambers.
    pub opens: u32,
    pub closes: u32,
    pub loads: u32,
    pub unloads: u32,
    pub case_ejects: u32,
    /// Bolt and pump strokes (back and home again, each a round chambered or not).
    pub cycles: u32,
    /// Air guns: pump strokes, trigger pulls without the pressure, and the last shot's strength.
    pub pumps: u32,
    pub hisses: u32,
    pub last_strength: f32,
}

/// A push from the game (its explosion, a bullet), applied on the next step.
#[derive(Clone, Copy, Debug)]
enum Push {
    Blast { at: Vec3, radius: f32, impulse: f32 },
    One { id: u32, impulse: Vec3, at: Vec3 },
}

pub struct RigInput<'a> {
    pub frame: &'a HmdFrame,
    pub map: WorldMap,
    pub dt: f32,
    pub now: f64,
    pub settings: &'a Settings,
    /// Calibrated standing eye height (zones scale with it).
    pub eye_height: f32,
    /// The game's body is crouched (the game's recoil has crouched kicks).
    pub crouched: bool,
    /// The game's menu is open: hands go idle.
    pub paused: bool,
    pub infinite_ammo: bool,
}

const SPAWN_BASE: u32 = 0x8000_0000;
/// A hand inside one of the game's props asks for this many m/s per metre of overlap, on top of
/// its own speed into it, and never more than `PUSH_MAX` in all.
const PUSH_OUT: f32 = 10.0;
const PUSH_MAX: f32 = 4.0;
/// Where the shoulder is from the eyes (for the gloves' reach): this far down, and out to the side.
const SHOULDER_DROP: f32 = 0.25;
const SHOULDER_OUT: f32 = 0.18;
/// Faster than this (m/s), a hand pushes the way it moves.
const PUSH_MOVING: f32 = 0.2;
const LEASH: f32 = 0.7;
/// A turn (radians) or a move (metres) of the player's mapping in one step past which it's a snap
/// turn or a teleport, not a walk (as `crate::locomotion` tells them apart).
const SNAP_TURN: f32 = 0.3;
const SNAP_MOVE: f32 = 1.0;
/// A held thing the world holds back further than this from the hand (metres: a fence the walk
/// carries it along, a doorway it catches in) goes through the world until it's back in the hand
/// (within `SLIP_BACK`) and clear of it: it's never dragged along behind and then jumps back.
const SLIP: f32 = 0.1;
const SLIP_BACK: f32 = 0.02;
/// The least a held thing speeds up and spins up, however heavy (m/s^2, rad/s^2).
const MIN_HELD_ACCEL: f32 = 15.0;
const MIN_HELD_ANG_ACCEL: f32 = 40.0;
const BLOCKED: f32 = 0.04;
const FOREGRIP_REACH: f32 = 0.14;
/// The support hand lets go of the foregrip when its controller is this far from the gun's line.
const FOREGRIP_BREAK: f32 = 0.3;
/// Letting go of something faster than this throws it rather than holstering it.
const HOLSTER_MAX_SPEED: f32 = 1.2;
/// A throw lets go when the grip eases this far off its hardest squeeze while the hand swings
/// faster than this (m/s).
const THROW_EASE: f32 = 0.25;
const THROW_EASE_SPEED: f32 = 1.5;
const HANDLE_REACH: f32 = 0.09;
/// A hand this near open barrels' middle (or the foregrip on them) takes them; this near an open
/// chamber holding a live round, pulls it out.
const BARREL_REACH: f32 = 0.1;
const CHAMBER_REACH: f32 = 0.035;
/// A round goes into an open chamber when its base is this near the chamber's mouth and it points
/// within this of the chamber's axis (easy, the default; strict mode half that).
const ROUND_SEAT: f32 = 0.05;
const ROUND_SEAT_ANGLE: f32 = 50.0;
/// A break action has to be open this far (or most of the way, if it opens less) to load it.
const LOAD_OPEN: f32 = 35.0;
/// A gun fired with its sights this close to the eye is aimed (the game's aimed recoil).
const AIMED_SIGHT: f32 = 0.15;
const RETURN_TIME: f64 = 0.3;

pub struct Rig {
    pub physics: Physics,
    objects: BTreeMap<u32, Obj>,
    /// Where the game says its own simulated objects are (`OBJF_GAME_SIMULATED`, world space).
    game_poses: HashMap<u32, (Vec3, Quat)>,
    /// Where the hands push the game's own free objects after this step (see [`GamePush`]).
    pub game_pushes: Vec<GamePush>,
    /// The game's convex collision for its objects ([`SHAPE_HULL`]), object space.
    game_hulls: HashMap<u32, Vec<Vec3>>,
    hands: Option<[Hand; 2]>,
    anchor: BodyAnchor,
    slots: [Option<u32>; SLOT_COUNT_BODY],
    next_spawn: u32,
    templates: Vec<ObjectDesc>,
    pub feedback: Vec<Feedback>,
    pub debug: RigDebug,
    table_seq: u32,
    /// Controller grip pose -> drawn hand (the IK's), for the support hand's default hold.
    offsets: rustvr_ik::HandOffsets,
    map: WorldMap,
    now: f64,
    pub frame_index: u64,
    /// The guns' own recoil (the adapter's kick table), by gun.
    kicks: HashMap<u32, Vec<KickDesc>>,
    pushes: Vec<Push>,
}

impl Default for Rig {
    fn default() -> Self {
        Rig {
            physics: Physics::default(),
            objects: BTreeMap::new(),
            hands: None,
            anchor: BodyAnchor::default(),
            slots: [None; SLOT_COUNT_BODY],
            next_spawn: SPAWN_BASE,
            game_poses: HashMap::new(),
            game_pushes: Vec::new(),
            game_hulls: HashMap::new(),
            templates: Vec::new(),
            feedback: Vec::new(),
            debug: RigDebug::default(),
            table_seq: 0,
            offsets: rustvr_ik::BodyParams::default().hand_offsets,
            map: WorldMap::IDENTITY,
            now: 0.0,
            frame_index: 0,
            kicks: HashMap::new(),
            pushes: Vec::new(),
        }
    }
}

fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::from_array(a)
}

fn q4(a: [f32; 4]) -> Quat {
    physics::quat(a)
}

fn pose(p: Vec3, q: Quat) -> Pose {
    Pose { pos: p.to_array(), valid: 1, rot: q.normalize().to_array() }
}

fn compose(a: (Vec3, Quat), b: (Vec3, Quat)) -> (Vec3, Quat) {
    (a.0 + a.1 * b.0, (a.1 * b.1).normalize())
}

fn inverse(a: (Vec3, Quat)) -> (Vec3, Quat) {
    let ri = a.1.inverse();
    (ri * -a.0, ri)
}

/// How a magazine from the pouch sits in the hand (grip-pose space): its insertion axis (+Y) along
/// the fist's axis (+Z), and its top this far out past the thumb.
pub fn held_mag_rot() -> Quat {
    Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)
}
pub const HELD_MAG_TOP: Vec3 = Vec3::new(0.0, 0.0, 0.06);

/// How a round from the pouch (a magazine of one) sits in the hand: nose first along the fist's
/// axis (+Z, out past the thumb), its base just inside the fist, ready to push into a chamber.
pub fn held_round_rot() -> Quat {
    Quat::IDENTITY
}
pub const HELD_ROUND_BASE: Vec3 = Vec3::new(0.0, 0.0, -0.005);

/// A magazine of one round is a round.
fn is_round(d: &ObjectDesc) -> bool {
    d.kind == OBJ_MAGAZINE && d.capacity == 1
}

/// A quaternion field, if it's set (all zero means "not authored").
fn authored(a: [f32; 4]) -> Option<Quat> {
    let q = Quat::from_array(a);
    (q.is_finite() && q.length_squared() > 1e-6).then(|| q.normalize())
}

/// A right-hand grip rotation as the left hand would hold the same gun (mirror across the
/// weapon's YZ plane; both hands' grip X axes point the same way on a grip, so this is exact).
fn mirror_grip(q: Quat) -> Quat {
    Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
}

/// Where the support hand holds the gun (weapon space): the game's own place for it. It locks there.
fn foregrip_point(d: &ObjectDesc, _local: Vec3) -> Vec3 {
    v3(d.foregrip)
}

/// How far a hand (or what it takes) goes in one grab transition: further takes longer in
/// proportion, so it never moves faster than it does over this.
pub const TRAVEL_REACH: f32 = 0.2;

/// How long a hand's transition over `distance` metres takes: the grab transition setting, longer
/// in proportion beyond [`TRAVEL_REACH`] (0: at once, whatever the distance).
pub fn travel_time(transition: f32, distance: f32) -> f32 {
    transition.max(0.0) * (distance / TRAVEL_REACH).max(1.0)
}

fn smooth(x: f64) -> f32 {
    let x = x.clamp(0.0, 1.0) as f32;
    x * x * (3.0 - 2.0 * x)
}

const GRAVITY: f32 = 9.81;

/// Half-Life: Alyx's gravity gloves, from its convars (`vr_hand_pull_*`, `vr_gg_*` in
/// `server.dll`; Source units are inches, here in metres).
pub mod alyx {
    /// `vr_hand_pull_cone` 0.965: the cone you point into (cos of 15 degrees) ...
    pub const CONE_COS: f32 = 0.965;
    /// Only what you're looking at: `vr_gg_fov_max_for_target` 60 and `..._vert_for_target` 70
    /// (degrees, the window's full width and height), centred `vr_gg_fov_center_vert_offset` -100
    /// at 500 below the line of sight (server.dll `0x3F2EA0` draws it: tan(fov / 2) * 500 either
    /// side of the point 500 ahead, the offset along the view's up).
    pub const VIEW_HALF_WIDE: f32 = 30.0;
    pub const VIEW_HALF_HIGH: f32 = 35.0;
    pub const VIEW_DOWN: f32 = -100.0 / 500.0;
    /// ... starting `vr_hand_pull_coneoffset` -10 (25 cm) behind the hand, out to
    /// `vr_hand_pull_conelength` 400 (10 m, the range setting's default).
    pub const CONE_BACK: f32 = 0.254;
    pub const RANGE: f32 = 10.16;
    /// The current target holds on out to 20 degrees (so it doesn't flicker at the edge).
    pub const HOLD_COS: f32 = 0.94;
    /// `vr_hand_pull_mindist` 12: nothing closer (that's in reach).
    pub const MIN_DIST: f32 = 0.3048;
    /// `vr_gg_target_select_time` / `vr_gg_target_change_time`: 0.1 s pointed at a thing takes it.
    pub const SELECT_TIME: f64 = 0.1;
    pub const CHANGE_TIME: f64 = 0.1;
    /// `vr_gg_lock_release_delay` 0.25 s: a locked target lets go this long after the grip does
    /// (and only then: wherever the hand turns meanwhile, the grip holds the lock).
    pub const LOCK_RELEASE: f64 = 0.25;
    /// Locked, it hops up (m/s) and keeps straining towards the hand every so often.
    pub const LOCK_HOP: f32 = 1.2;
    pub const STRAIN_HOP: f32 = 0.6;
    pub const STRAIN_EVERY: f64 = 0.5;
    /// `vr_gg_pull_gesture_speed` 5: the wrist flick, rad/s.
    pub const FLICK_WRIST: f32 = 5.0;
    /// `vr_hand_pull_traveltime_min` 0.6 s at `..._min_distance` 100 (2.5 m), `..._max` 0.8 s at
    /// `..._max_distance` 400 (10 m).
    pub const TRAVEL: (f32, f32, f32, f32) = (0.6, 0.8, 2.54, 10.16);
    /// `vr_hand_pull_angle_near` 20 degrees at `..._near_distance` 100 (2.5 m), `..._far` 10 at
    /// `..._far_distance` 250 (6.35 m): how far above the straight line it's thrown.
    pub const ANGLE: (f32, f32, f32, f32) = (20.0, 10.0, 2.54, 6.35);
    /// `vr_hand_pull_dynamic_min_gravity_scale` 0.5, `..._min_time_of_flight` 0.5 s.
    pub const MIN_GRAVITY: f32 = 0.5;
    pub const MIN_FLIGHT: f32 = 0.5;
    /// `vr_hand_pull_acceleration_start_time` 0.6 / `..._shutoff_time` 0.05: it steers after the
    /// moving hand from 0.6 s before it arrives until 0.05 s before, at up to
    /// `vr_hand_pull_acceleration_max` 500 (12.7 m/s^2).
    pub const HOMING_START: f32 = 0.6;
    pub const HOMING_STOP: f32 = 0.05;
    pub const HOMING_MAX: f32 = 12.7;
}

/// Alyx's throw for a pull from `d` metres: (flight time, gravity on the way, m/s^2). The time and
/// the angle above the straight line come from the distance; gravity bends the flight into that
/// angle, eased to no less than half (a near pull then flies a little longer).
pub fn alyx_pull(d: f32) -> (f32, f32) {
    let lerp = |(a, b, x0, x1): (f32, f32, f32, f32)| a + (b - a) * ((d - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let mut t = lerp(alyx::TRAVEL);
    let tan = lerp(alyx::ANGLE).to_radians().tan();
    let need = |t: f32| 2.0 * d * tan / (t * t);
    let g_min = alyx::MIN_GRAVITY * GRAVITY;
    let mut g = need(t);
    if g < g_min {
        t = (2.0 * d * tan / g_min).sqrt().max(alyx::MIN_FLIGHT);
        g = need(t).max(g_min);
    }
    (t, g.min(GRAVITY))
}

impl Rig {
    pub fn has_objects(&self) -> bool {
        !self.objects.is_empty()
    }

    // ---- the game's object table -----------------------------------------------------------------
    /// Applies the adapter's object table if it changed (`seq` differs from the last one applied).
    /// Takes the guns' own recoil from the adapter's kick table (the kicks playing stop).
    pub fn set_kicks(&mut self, table: &KickTable) {
        self.kicks.clear();
        for k in table.entries().iter().filter(|k| k.object != 0) {
            self.kicks.entry(k.object).or_default().push(*k);
        }
        for (id, o) in &mut self.objects {
            if let Some(w) = &mut o.weapon {
                w.game = kick::GameRecoil::new(id.wrapping_mul(2246822519));
            }
        }
    }

    /// The gun's own recoil, if the game gave one.
    pub fn kicks(&self, id: u32) -> &[KickDesc] {
        self.kicks.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Where the game's own simulated objects are now (`OBJF_GAME_SIMULATED`): the free ones follow.
    pub fn set_game_poses(&mut self, poses: &GamePoses) {
        self.game_poses.clear();
        for p in &poses.poses[..(poses.count as usize).min(MAX_OBJECTS)] {
            if p.id != 0 && p.pose.valid != 0 {
                self.game_poses.insert(p.id, (v3(p.pose.pos), q4(p.pose.rot)));
            }
        }
    }

    /// The physics world as world triangles, coloured by what each thing is: the game's collision
    /// grey, its props orange and what it moves itself magenta, the runtime's loose objects green and held
    /// ones cyan, the palms blue and the fingers light blue (dim while they touch nothing). For the
    /// framework view.
    pub fn physics_triangles(&self, out: &mut Vec<([Vec3; 3], [f32; 3])>) {
        let mut tints: HashMap<ColliderHandle, [f32; 3]> = HashMap::new();
        for o in self.objects.values() {
            let c = if o.desc.flags & OBJF_GAME_MOVED != 0 {
                [0.9, 0.3, 0.85]
            } else if o.desc.flags & OBJF_GAME_SIMULATED != 0 && o.holder == Holder::Free {
                [1.0, 0.55, 0.15]
            } else if matches!(o.holder, Holder::Free | Holder::Stuck { .. }) {
                [0.3, 0.85, 0.35]
            } else {
                [0.2, 0.9, 0.9]
            };
            tints.insert(o.collider, c);
        }
        // Left out: the hands' and fingers' colliders (the body is drawn there), and the guns and
        // magazines the runtime draws itself (drawn as their low-poly selves instead; a game's prop
        // it draws while in a hand is drawn here as its collision, like any other).
        let mut skip: std::collections::HashSet<ColliderHandle> = std::collections::HashSet::new();
        for h in self.hands.iter().flatten() {
            skip.insert(h.collider);
            skip.extend(h.fingers.iter().filter_map(|f| self.physics.collider_of(*f)));
        }
        skip.extend(self.objects.values().filter(|o| o.desc.flags & OBJF_RUNTIME_DRAWS != 0 && o.desc.kind != OBJ_PROP).map(|o| o.collider));
        self.physics.surface_triangles(
            |h, c| (!skip.contains(&h)).then(|| tints.get(&h).copied().unwrap_or(if c.parent().is_some() { [0.75, 0.75, 0.75] } else { [0.55, 0.57, 0.62] })),
            out,
        );
    }

    /// The guns' working parts, from the game's rig, as small cubes (world space) to draw over the
    /// framework view: where it's held, where its magazine goes, its charging handle (where it is
    /// now: it moves as it's racked), muzzle and sight.
    pub fn mechanism_markers(&self, out: &mut Vec<([Vec3; 3], [f32; 3])>) {
        for (id, o) in &self.objects {
            if o.desc.kind != OBJ_WEAPON {
                continue;
            }
            let Some((p, q)) = self.obj_pose(*id) else { continue };
            let d = &o.desc;
            let marks: [(Vec3, [f32; 3]); 6] = [
                (v3(d.grip), [0.2, 0.45, 1.0]),
                (v3(d.foregrip), [0.25, 0.9, 0.35]),
                (v3(d.mag_well), [1.0, 0.85, 0.15]),
                (self.handle_now(*id), [1.0, 0.5, 0.1]),
                (v3(d.muzzle), [1.0, 0.2, 0.15]),
                (v3(d.sight), [0.95, 0.95, 0.95]),
            ];
            for (at, color) in marks {
                if at != Vec3::ZERO {
                    marker_cube(p + q * at, q, 0.009, color, out);
                }
            }
        }
    }

    /// The game's convex collision for its objects: kept for the ones still to come, and given to
    /// the ones already here whose hull is new.
    pub fn set_game_hulls(&mut self, hulls: &GameHulls) {
        for h in &hulls.hulls[..(hulls.count as usize).min(MAX_GAME_HULLS)] {
            let points: Vec<Vec3> = h.points[..(h.count as usize).min(MAX_HULL_POINTS)].iter().map(|p| Vec3::new(p[0], p[1], p[2])).filter(|p| p.is_finite()).collect();
            if h.id == 0 || points.len() < 4 || self.game_hulls.get(&h.id) == Some(&points) {
                continue;
            }
            if let Some(o) = self.objects.get(&h.id)
                && o.desc.shape == SHAPE_HULL
            {
                let (c, mass) = (o.collider, o.desc.mass);
                self.physics.set_hull(c, &points, mass);
            }
            self.game_hulls.insert(h.id, points);
        }
    }

    pub fn sync(&mut self, table: &ObjectTable) {
        if table.seq == self.table_seq {
            return;
        }
        self.table_seq = table.seq;
        let mut seen = Vec::new();
        let descs = &table.descs[..(table.count as usize).min(MAX_OBJECTS)];
        // Templates first: a gun loads itself from its magazine template as it's added.
        self.templates = descs.iter().filter(|d| d.id != 0 && d.flags & OBJF_TEMPLATE != 0).copied().collect();
        for d in descs {
            if d.id == 0 || d.flags & OBJF_TEMPLATE != 0 {
                continue;
            }
            seen.push(d.id);
            match self.objects.get_mut(&d.id) {
                Some(o) => {
                    o.desc = *d;
                    if let Some(t) = &mut o.throw {
                        t.flags = d.throw_flags;
                    }
                    if o.spawn_seq != d.spawn_seq {
                        o.spawn_seq = d.spawn_seq;
                        // Put back by the game (a grenade that went off, say): safe again.
                        if let Some(t) = &mut o.throw {
                            t.reset();
                        }
                        self.place(d.id, v3(d.spawn_pose.pos), q4(d.spawn_pose.rot));
                        self.start_in_slot(d);
                        // The game's ammunition: its magazine holds what the game's does.
                        if d.kind == OBJ_WEAPON && d.flags & OBJF_GAME_AMMO != 0 {
                            self.load_game_magazine(d.id);
                        }
                    }
                }
                None => {
                    self.add(*d, d.id, false, v3(d.spawn_pose.pos), q4(d.spawn_pose.rot));
                    self.start_in_slot(d);
                }
            }
        }
        let gone: Vec<u32> = self.objects.iter().filter(|(id, o)| !o.spawned && !seen.contains(id)).map(|(id, _)| *id).collect();
        for id in gone {
            self.remove(id);
        }
    }

    fn add(&mut self, d: ObjectDesc, draw_desc: u32, spawned: bool, pos: Vec3, rot: Quat) {
        let (body, collider) = self.physics.add_object(&d, pos, rot, self.game_hulls.get(&d.id).map(Vec::as_slice));
        let weapon = (d.kind == OBJ_WEAPON).then(|| {
            let mut w = WeaponState::for_desc(&d);
            w.recoil = recoil::Recoil::new(d.id.wrapping_mul(2654435761));
            w.game = kick::GameRecoil::new(d.id.wrapping_mul(2246822519));
            w
        });
        let id = d.id;
        self.objects.insert(
            id,
            Obj {
                desc: d,
                draw_desc,
                body,
                collider,
                holder: Holder::Free,
                attach: Attach::Fixed(Vec3::ZERO, Quat::IDENTITY),
                blend: None,
                spawn_seq: d.spawn_seq,
                spawned,
                free_since: self.now,
                home_slot: None,
                rounds: if d.kind == OBJ_MAGAZINE { d.rounds.min(d.capacity.max(d.rounds)) } else { 0 },
                weapon,
                hovered: false,
                hands_off_until: 0.0,
                handled: false,
                in_contact: false,
                carry: None,
                carried: (Vec3::ZERO, Vec3::ZERO),
                through: false,
                prev_vel: Vec3::ZERO,
                next_impact: 0.0,
                seat_armed: true,
                throw: ThrowState::new(&d),
                hang: None,
                valve: (d.kind == OBJ_VALVE).then(|| {
                    let v = valve::Valve::new(pos, rot, d.radius, d.mass, d.friction).with_range(d.turn_min, d.turn_max, d.detent);
                    valve::Valve { max_speed: d.fire_rate.max(0.0), twist: v.twist || d.joint_flags & JOINT_TWIST != 0, ..v }
                }),
                valve_shape: Vec::new(),
            },
        );
        // A valve's wheel turns on its axle, never anywhere else.
        if d.kind == OBJ_VALVE {
            self.physics.set_kinematic(body, true);
        }
        // Hung from its point, where it is now; damped like a cable or a chain settling.
        if d.flags & OBJF_HANGING != 0 {
            let anchor = self.physics.hang(body, v3(d.handle));
            self.physics.set_damping(body, 0.4, 1.5);
            self.objects.get_mut(&id).unwrap().hang = Some(anchor);
        }
        // A gun arrives loaded: a seated magazine from its template, or rounds held internally (a
        // gun loaded into its chambers has them all loaded already).
        if d.kind == OBJ_WEAPON && d.weapon_flags & WPN_LOADS_ROUNDS != 0 {
            // A magazine that stays in it, full: one of its rounds chambered.
            if let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) {
                w.internal = Some(d.capacity.max(1) - w.chambered as u32);
            }
        } else if d.kind == OBJ_WEAPON && d.weapon_flags & WPN_LOADS_CHAMBERS == 0 {
            match self.template_for(d.ammo_type) {
                // With the game's ammunition, the rounds the game's gun holds.
                Some(_) if d.flags & OBJF_GAME_AMMO != 0 => self.load_game_magazine(id),
                Some(t) => {
                    let mag = self.spawn_mag(&t, t.capacity, pos, rot);
                    self.seat(mag, id, false);
                }
                None => {
                    if let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) {
                        w.internal = Some(d.capacity.max(1));
                    }
                }
            }
        }
    }

    /// Objects the game hands the player at the start go straight into their body slot.
    fn start_in_slot(&mut self, d: &ObjectDesc) {
        let slot = d.start_slot.wrapping_sub(1);
        if (slot as usize) < SLOT_COUNT_BODY && self.slots[slot as usize].is_none_or(|s| s == d.id) {
            if let Some(h) = &mut self.hands {
                for h in h.iter_mut().filter(|h| h.holding == Some(d.id)) {
                    h.holding = None;
                }
            }
            self.holster(d.id, slot);
        }
    }

    fn remove(&mut self, id: u32) {
        if let Some(o) = self.objects.remove(&id) {
            self.physics.remove(o.body);
            if let Some(a) = o.hang {
                self.physics.remove(a);
            }
            for s in self.slots.iter_mut() {
                if *s == Some(id) {
                    *s = None;
                }
            }
            if let Some(hands) = &mut self.hands {
                for h in hands.iter_mut() {
                    if h.holding == Some(id) {
                        h.holding = None;
                    }
                    if h.support == Some(id) {
                        h.support = None;
                    }
                    if h.handle == Some(id) {
                        h.handle = None;
                    }
                    if h.pin.is_some_and(|(g, _)| g == id) {
                        h.pin = None;
                    }
                    if h.barrels.is_some_and(|b| b.gun == id) {
                        h.barrels = None;
                    }
                }
            }
            if let Some(w) = o.weapon {
                if let Some(m) = w.mag {
                    self.remove(m);
                }
            }
        }
    }

    fn place(&mut self, id: u32, pos: Vec3, rot: Quat) {
        let Some(o) = self.objects.get(&id) else { return };
        let body = o.body;
        for s in self.slots.iter_mut().filter(|s| **s == Some(id)) {
            *s = None;
        }
        self.free_body(id, groups_free());
        self.physics.set_pose(body, pos, rot);
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
    }

    /// A loose dynamic body again: gravity, normal damping, colliding with everything.
    fn free_body(&mut self, id: u32, groups: rapier3d::prelude::InteractionGroups) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        o.through = false;
        o.holder = Holder::Free;
        o.free_since = self.now;
        o.blend = None;
        self.physics.set_kinematic(o.body, false);
        self.physics.set_gravity(o.body, 1.0);
        self.physics.set_angular_damping(o.body, 0.2);
        self.physics.set_groups(o.collider, groups);
    }

    fn template_for(&self, ammo_type: u32) -> Option<ObjectDesc> {
        self.templates.iter().find(|t| t.kind == OBJ_MAGAZINE && t.ammo_type == ammo_type).copied()
    }

    /// A copy of the magazine template `t` holding `rounds` (at most its capacity).
    fn spawn_mag(&mut self, t: &ObjectDesc, rounds: u32, pos: Vec3, rot: Quat) -> u32 {
        let id = self.next_spawn;
        self.next_spawn = self.next_spawn.wrapping_add(1).max(SPAWN_BASE);
        let mut d = *t;
        d.id = id;
        d.flags &= !OBJF_TEMPLATE;
        d.rounds = rounds.min(t.capacity);
        self.add(d, t.id, true, pos, rot);
        id
    }

    /// How many rounds a magazine for gun `wid` holds as it comes out of the pouch: a full one,
    /// or with the game's ammunition ([`OBJF_GAME_AMMO`]) what the game's own reload would load
    /// now (its template's `rounds`; 0: the pouch is empty).
    fn pouch_rounds(&self, wid: u32, t: &ObjectDesc) -> u32 {
        match self.objects.get(&wid) {
            Some(o) if o.desc.flags & OBJF_GAME_AMMO != 0 => t.rounds.min(t.capacity),
            _ => t.capacity,
        }
    }

    /// A magazine-fed gun with the game's ammunition ([`OBJF_GAME_AMMO`]), as it arrives or the
    /// game puts it back: it holds every round the game's gun does (`desc.rounds`). A self-loading
    /// gun has one of them chambered and the rest in its seated magazine (none: both empty); a gun
    /// worked by hand has them all in its magazine. Without a seated magazine, one from its
    /// template goes in.
    fn load_game_magazine(&mut self, wid: u32) {
        let Some(o) = self.objects.get(&wid) else { return };
        let (d, mag) = (o.desc, o.weapon.as_ref().and_then(|w| w.mag));
        if d.weapon_flags & (WPN_LOADS_ROUNDS | WPN_LOADS_CHAMBERS) != 0 {
            return;
        }
        let in_mag = if d.action == ACTION_SELF_LOADING {
            if let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) {
                w.chambered = d.rounds > 0;
            }
            d.rounds.saturating_sub(1)
        } else {
            d.rounds
        };
        match mag {
            Some(m) => {
                if let Some(m) = self.objects.get_mut(&m) {
                    m.rounds = in_mag.min(m.desc.capacity);
                }
            }
            None => {
                if let Some(t) = self.template_for(d.ammo_type) {
                    let (pos, rot) = self.obj_pose(wid).unwrap_or((Vec3::ZERO, Quat::IDENTITY));
                    let mag = self.spawn_mag(&t, in_mag, pos, rot);
                    self.seat(mag, wid, false);
                }
            }
        }
    }

    // ---- the frame ------------------------------------------------------------------------------
    pub fn update(&mut self, inp: &RigInput) -> RigOutput {
        self.now = inp.now;
        let prev = self.map;
        self.map = inp.map;
        // What the hands hold goes where the player goes (walking, turning): a thing's weight
        // limits how fast a hand swings it, not how fast the player walks with it. It's carried
        // as a velocity (`Drive::carry`), so a wall the walk takes it into stops it rather than it
        // jumping into the wall and being thrown back out.
        let carrying = prev != WorldMap::IDENTITY && prev != inp.map && prev.world_per_meter == inp.map.world_per_meter;
        let turn = inp.map.yaw() * prev.yaw().inverse();
        let carry = |p: Vec3, q: Quat| (inp.map.point(prev.inv_point(p)), (turn * q).normalize());
        // A snap turn or a teleport isn't a walk: what the hands hold, and the hands, go with it at
        // once (as a velocity, a 45 degree snap in one step is past any speed the physics will
        // take: the gun lagged off the hand, which let go, and was flung through the world).
        let head = v3(inp.frame.hmd.pos);
        let jumped = carrying
            && (turn.angle_between(Quat::IDENTITY) > SNAP_TURN || inp.map.point(head).distance(prev.point(head)) > SNAP_MOVE * inp.map.world_per_meter);
        if jumped {
            for o in self.objects.values_mut().filter(|o| matches!(o.holder, Holder::Held(_))) {
                let (p, q) = self.physics.pose(o.body);
                let (cp, cq) = carry(p, q);
                self.physics.set_pose(o.body, cp, cq);
                let (v, w) = self.physics.velocity(o.body);
                self.physics.set_velocity(o.body, turn * v, turn * w);
                if let Some((bp, bq, t0, d)) = o.blend {
                    let (bp, bq) = carry(bp, bq);
                    o.blend = Some((bp, bq, t0, d));
                }
                (o.carry, o.carried) = (None, (Vec3::ZERO, Vec3::ZERO));
            }
            for h in self.hands.iter().flatten() {
                for b in std::iter::once(h.body).chain(h.fingers) {
                    let (p, q) = self.physics.pose(b);
                    let (cp, cq) = carry(p, q);
                    self.physics.set_pose(b, cp, cq);
                    let (v, w) = self.physics.velocity(b);
                    self.physics.set_velocity(b, turn * v, turn * w);
                }
            }
        }
        for o in self.objects.values_mut().filter(|o| !jumped && matches!(o.holder, Holder::Held(_))) {
            o.carry = carrying.then(|| {
                // Its own swing turns with the player too.
                let (v, w) = self.physics.velocity(o.body);
                self.physics.set_velocity(o.body, turn * v, turn * w);
                o.carried = (turn * o.carried.0, turn * o.carried.1);
                let (p, q) = self.physics.pose(o.body);
                carry(p, q)
            });
            if carrying && let Some((bp, bq, t0, d)) = o.blend {
                let (bp, bq) = carry(bp, bq);
                o.blend = Some((bp, bq, t0, d));
            }
        }
        self.frame_index = inp.frame.index;
        self.feedback.clear();
        let dt = inp.dt.clamp(1e-4, 0.05);
        let s = inp.settings;
        let dom = s.dominant();
        self.ensure_hands(inp.frame);

        // Controllers -> world targets.
        {
            let map = self.map;
            let hands = self.hands.as_mut().unwrap();
            for (i, h) in hands.iter_mut().enumerate() {
                let c = &inp.frame.hands[i];
                let turned = (q4(c.grip.rot) * h.grip_t.1.inverse()).normalize();
                let turned = if turned.w < 0.0 { -turned } else { turned };
                h.turn_rate = map.rot(Quat::IDENTITY) * turned.to_scaled_axis() / dt;
                h.grip_t = (v3(c.grip.pos), q4(c.grip.rot));
                h.aim_t = (v3(c.aim.pos), q4(c.aim.rot));
                h.vel_t = v3(c.vel);
                h.ang_t = v3(c.ang_vel);
                h.target = (map.point(h.grip_t.0), map.rot(h.grip_t.1));
                h.aim_rot = map.rot(h.aim_t.1);
                h.trigger = c.trigger;
                h.history.push(inp.now, h.grip_t.0, Some(h.vel_t), h.ang_t, map);
            }
        }

        // The body anchor (zones) follows the head, frozen while a hand reaches into a zone.
        let head_t = (v3(inp.frame.hmd.pos), q4(inp.frame.hmd.rot));
        let zones = slot_zones(inp.eye_height, dom);
        let ammo = ammo_zones(inp.eye_height, dom);
        let near_zone = (0..2).any(|i| {
            let p = self.hands.as_ref().unwrap()[i].grip_t.0;
            zones.iter().chain(ammo.iter()).any(|z| p.distance(self.anchor.zone_center(z)) < z.radius + 0.1)
        });
        self.anchor.update(head_t.0, head_t.1, dt, near_zone);

        if !inp.paused {
            for i in 0..2 {
                self.update_hover(i, s, &zones, &ammo);
            }
            for i in 0..2 {
                self.hand_input(i, inp, &zones, dom);
            }
            for i in 0..2 {
                self.gloves(i, inp);
            }
            self.weapons(inp, dom);
            self.throwables();
        } else {
            // The game is paused: so are the fuses.
            for t in self.objects.values_mut().filter_map(|o| o.throw.as_mut()) {
                if let Fuse::Lit { at } = &mut t.fuse {
                    *at += dt as f64;
                }
            }
        }
        self.drive(inp, dt, &zones);
        self.place_fingers(inp);
        self.apply_pushes();
        self.physics.set_dt(dt);
        self.physics.step();
        self.after_step(inp);
        self.game_pushes = self.pushes();
        self.output(inp, dom)
    }

    /// The fingers onto each physics hand, curled as the controller's trigger (the index) and grip
    /// (the rest) are held.
    fn place_fingers(&mut self, inp: &RigInput) {
        let Some(hands) = &mut self.hands else { return };
        let dt = inp.dt.max(1e-4);
        for (i, h) in hands.iter_mut().enumerate() {
            let side = if i == 0 { rustvr_ik::Side::Left } else { rustvr_ik::Side::Right };
            let (gp, gq) = self.physics.pose(h.body);
            let hand = self.offsets.hand_frame(side, rustvr_ik::Pose { pos: gp, rot: gq });
            let c = &inp.frame.hands[i];
            let grip = c.squeeze.clamp(0.0, 1.0);
            let curls = [grip * 0.6, c.trigger.clamp(0.0, 1.0), grip, grip, grip];
            h.fingers_on = h.holding.is_none() && h.support.is_none() && h.handle.is_none() && h.barrels.is_none() && h.pin.is_none() && h.valve.is_none();
            let solid = h.fingers_on && h.hover == Hover::None && matches!(h.glove, Glove::Idle);
            if solid != h.fingers_solid {
                h.fingers_solid = solid;
                for f in h.fingers {
                    if let Some(c) = self.physics.collider_of(f) {
                        self.physics.set_groups(c, if solid { physics::groups_finger(i) } else { groups_none() });
                    }
                }
            }
            for (k, (a, b)) in fingers::segments((hand.pos, hand.rot), i == 1, curls).into_iter().enumerate() {
                let (at, rot, half) = fingers::capsule_pose(a, b);
                let f = h.fingers[k];
                self.physics.set_capsule_half_height(f, half);
                self.physics.move_kinematic(f, at, rot);
                h.finger_vel[k] = (at - h.finger_at[k]) / dt;
                h.finger_at[k] = at;
            }
        }
    }

    /// The part a hand takes of the valves described by `desc` (the game's description id): `shape`,
    /// points of the part that turns in the wheel's frame (the model the runtime draws it with).
    pub fn set_valve_shape(&mut self, desc: u32, shape: &[Vec3]) {
        for o in self.objects.values_mut().filter(|o| o.desc.id == desc && o.valve.is_some()) {
            o.valve_shape = shape.to_vec();
        }
    }

    /// Turns the valves: each by the hands on it (cranking its rim towards their controllers, or
    /// twisting it as far as their wrists have turned) against its friction (`stiffness`: the
    /// player's setting), and puts it and those hands where it's turned to (a hand that's just
    /// taken hold on its way there over `transition` seconds).
    fn valves(&mut self, dt: f32, transition: f32, stiffness: f32) {
        let ids: Vec<u32> = self.objects.iter().filter(|(_, o)| o.valve.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            let Some(v) = self.objects[&id].valve else { continue };
            // A wrist's turn about the axle since it took hold (unwrapped, so turning past half a
            // turn goes on rather than flipping round).
            let mut pulls = Vec::new();
            let mut grips: Vec<(usize, valve::ValveGrip)> = Vec::new();
            for (i, h) in self.hands.iter_mut().flatten().enumerate() {
                let Some(g) = h.valve.as_mut().filter(|g| g.id == id) else { continue };
                if v.twist {
                    let w = v.wrist(h.target.1);
                    let d = (w - g.wrist + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    g.twisted += d;
                    g.wrist = w;
                    pulls.push(valve::Pull::Twist { to: g.turn0 + g.twisted });
                } else {
                    pulls.push(valve::Pull::Rim { at: g.at, toward: h.target.0 });
                }
                grips.push((i, *g));
            }
            let o = self.objects.get_mut(&id).unwrap();
            let v = o.valve.as_mut().unwrap();
            v.step(&pulls, dt, stiffness);
            let (v, body) = (*v, o.body);
            self.physics.move_kinematic(body, v.centre, v.rot());
            for (i, g) in grips {
                let h = &self.hands.as_ref().unwrap()[i];
                let b = h.body;
                let (p, q) = (v.point(g.at), (v.rot() * g.rot).normalize());
                let time = travel_time(transition, g.from.0.distance(p));
                let k = if time > 1e-3 { smooth((self.now - g.since) / time as f64) } else { 1.0 };
                self.physics.set_pose(b, g.from.0.lerp(p, k), g.from.1.slerp(q, k).normalize());
                self.physics.set_velocity(b, Vec3::ZERO, Vec3::ZERO);
            }
        }
    }

    /// The palms and fingers in the game's own free props, as pushes for the game: at each prop,
    /// the deepest touch, asking the point to move along with the hand at least as fast as the
    /// hand moves (plus enough to undo the overlap within a few frames).
    fn pushes(&self) -> Vec<GamePush> {
        let Some(hands) = &self.hands else { return Vec::new() };
        let mut out = Vec::new();
        for (id, o) in &self.objects {
            let game_free = o.desc.flags & OBJF_GAME_SIMULATED != 0 && o.holder == Holder::Free && self.now - o.free_since > GAME_HANDBACK as f64;
            if !game_free {
                continue;
            }
            let mut best: Option<(f32, Vec3, Vec3, f32)> = None;
            for h in hands {
                let palm_vel = self.physics.velocity(h.body).0;
                let fingers = h.fingers.iter().zip(h.finger_vel).filter(|_| h.fingers_on).map(|(f, v)| (self.physics.collider_of(*f), v));
                let palm = (h.holding.is_none() && h.support.is_none()).then_some((Some(h.collider), palm_vel));
                let pushers = palm.into_iter().chain(fingers);
                for (c, vel) in pushers {
                    let Some((point, normal, depth)) = c.and_then(|c| self.physics.overlap(c, o.collider)) else { continue };
                    // A moving hand pushes the way it moves (deep in a thin cable, the shortest
                    // way out can be backwards); one held still, straight out of it.
                    let (normal, speed) = match vel.try_normalize() {
                        Some(dir) if vel.length() > PUSH_MOVING => (dir, vel.length() + depth * PUSH_OUT),
                        _ => (normal, depth * PUSH_OUT),
                    };
                    let speed = speed.min(PUSH_MAX);
                    if best.is_none_or(|b| depth > b.0) {
                        best = Some((depth, point, normal, speed));
                    }
                }
            }
            if let Some((_, p, n, speed)) = best
                && out.len() < MAX_GAME_PUSHES
            {
                out.push(GamePush { id: *id, point: p.extend(1.0).to_array(), normal: n.extend(speed).to_array(), ..Default::default() });
            }
        }
        out
    }

    fn ensure_hands(&mut self, f: &HmdFrame) {
        if self.hands.is_some() {
            return;
        }
        let mk = |p: &mut Physics, i: usize| {
            let g = &f.hands[i].grip;
            let (body, collider) = p.add_hand(i, v3(g.pos), q4(g.rot));
            let fingers = std::array::from_fn(|_| p.add_finger(0.02));
            Hand {
                body,
                collider,
                fingers,
                finger_at: [v3(g.pos); 5],
                finger_vel: [Vec3::ZERO; 5],
                fingers_on: false,
                fingers_solid: false,
                grip_down: false,
                grip_toggled: false,
                prev_buttons: 0,
                trigger: 0.0,
                holding: None,
                support: None,
                handle: None,
                hover: Hover::None,
                glove: Glove::Idle,
                glove_pending: None,
                glove_released: None,
                glove_hop: 0.0,
                history: ThrowHistory::default(),
                pin: None,
                barrels: None,
                valve: None,
                turn_rate: Vec3::ZERO,
                trigger_was: 0.0,
                squeeze_peak: 0.0,
                eased: false,
                euro: OneEuro::new(1.0, 8.0, 1.0),
                blocked: false,
                target: (Vec3::ZERO, Quat::IDENTITY),
                aim_rot: Quat::IDENTITY,
                grip_t: (Vec3::ZERO, Quat::IDENTITY),
                aim_t: (Vec3::ZERO, Quat::IDENTITY),
                vel_t: Vec3::ZERO,
                ang_t: Vec3::ZERO,
                leashed_until: 0.0,
                returning: None,
            }
        };
        let l = mk(&mut self.physics, 0);
        let r = mk(&mut self.physics, 1);
        self.hands = Some([l, r]);
    }

    fn emit(&mut self, cue: Cue, hand: Option<usize>, object: Option<u32>, pos_world: Vec3, intensity: f32) {
        let pos = self.map.inv_point(pos_world);
        self.feedback.push(Feedback { cue, hand, object, pos, intensity });
    }

    fn obj_pose(&self, id: u32) -> Option<(Vec3, Quat)> {
        self.objects.get(&id).map(|o| self.physics.pose(o.body))
    }

    /// How far a hinged gun's barrels are open (radians, by size; 0 shut or for any other gun).
    fn hinge_angle(&self, id: u32) -> f32 {
        self.objects.get(&id).and_then(|o| o.weapon.as_ref()?.hinge).map_or(0.0, |h| h.angle)
    }

    /// Where the gun's handle is now (weapon space): a bolt lifted and back, a pump or an air gun's
    /// pump back, a slide at rest (a self-loading gun's handle is grabbed at rest).
    fn handle_now(&self, id: u32) -> Vec3 {
        let Some(o) = self.objects.get(&id) else { return Vec3::ZERO };
        let home = v3(o.desc.handle);
        match &o.weapon {
            Some(w) if w.cycle.is_some() => w.cycle.unwrap().knob(&o.desc, home),
            Some(w) if w.air.is_some() => home - Vec3::Z * w.slide * o.desc.handle_travel,
            _ => home,
        }
    }

    /// Where a hand holding these barrels is drawn (weapon space): its grab point turned with them.
    fn barrel_point(&self, g: &BarrelGrab) -> Vec3 {
        let Some(o) = self.objects.get(&g.gun) else { return g.at };
        actions::on_barrels(&o.desc, self.hinge_angle(g.gun), g.at)
    }

    /// The foregrip of a break action ahead of its hinge is on its barrels.
    fn foregrip_on_barrels(d: &ObjectDesc) -> bool {
        d.action == ACTION_BREAK && d.flags & OBJF_TWO_HANDED != 0 && d.foregrip[2] > d.handle[2]
    }

    /// The weapon in the other hand, if any.
    fn other_weapon(&self, hand: usize) -> Option<u32> {
        let id = self.hands.as_ref()?[hand ^ 1].holding?;
        (self.objects.get(&id)?.desc.kind == OBJ_WEAPON).then_some(id)
    }

    // ---- what an empty hand would grab ------------------------------------------------------------
    fn update_hover(&mut self, i: usize, s: &Settings, zones: &[Zone; SLOT_COUNT_BODY], ammo: &[Zone; 2]) {
        let hands = self.hands.as_ref().unwrap();
        let h = &hands[i];
        if h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.pin.is_some() || h.barrels.is_some() || h.valve.is_some() {
            self.hands.as_mut().unwrap()[i].hover = Hover::None;
            return;
        }
        let palm = h.target.0;
        let palm_t = h.grip_t.0;
        let mut hover = Hover::None;
        // 0. The other hand's grenade: its pin (the pin scheme).
        if let Some(gid) = self.hands.as_ref().unwrap()[i ^ 1].holding.filter(|_| s.interaction.grenade_arming == GrenadeArming::Pin) {
            let o = &self.objects[&gid];
            if o.throw.as_ref().is_some_and(|t| t.has_pin() && t.fuse == Fuse::Safe) {
                let (gp, gr) = self.physics.pose(o.body);
                if palm.distance(gp + gr * v3(o.desc.handle)) < throwable::PIN_REACH {
                    hover = Hover::Pin(gid);
                }
            }
        }
        // 1. The other hand's gun: its charging handle, then its foregrip (anywhere along it).
        if let (Hover::None, Some(wid)) = (hover, self.other_weapon(i)) {
            let o = &self.objects[&wid];
            let (wp, wr) = self.physics.pose(o.body);
            let handle = wp + wr * self.handle_now(wid);
            let fore = wp + wr * foregrip_point(&o.desc, wr.inverse() * (palm - wp));
            let w = o.weapon.as_ref();
            let needs_rack = w.is_some_and(|w| !w.chambered || w.slide_locked || w.spent);
            let open = w.and_then(|w| w.hinge).filter(|h| !h.latched).map(|h| h.angle);
            // An open hinged action: a live round in a chamber (to pull out), or the barrels.
            let chamber = open.and_then(|angle| {
                let w = w?;
                (0..w.chambers.len())
                    .filter(|&k| w.chambers[k] == Chamber::Live)
                    .map(|k| (k, (wp + wr * actions::on_barrels(&o.desc, angle, actions::chamber_mouth(&o.desc, k))).distance(palm)))
                    .filter(|(_, d)| *d < CHAMBER_REACH)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(k, _)| k)
            });
            let barrels = open.is_some_and(|angle| {
                let h = v3(o.desc.handle);
                let mid = wp + wr * actions::on_barrels(&o.desc, angle, h + actions::barrels_centre(&o.desc));
                let fore_on = Self::foregrip_on_barrels(&o.desc).then(|| wp + wr * actions::on_barrels(&o.desc, angle, v3(o.desc.foregrip)));
                palm.distance(mid) < BARREL_REACH || fore_on.is_some_and(|f| palm.distance(f) < FOREGRIP_REACH)
            });
            if let Some(k) = chamber {
                hover = Hover::Chamber(wid, k);
            } else if barrels {
                hover = Hover::Barrels(wid);
            } else if o.desc.handle_travel > 0.0 && palm.distance(handle) < HANDLE_REACH * if needs_rack { 1.3 } else { 1.0 } {
                hover = Hover::Handle(wid);
            } else if o.desc.flags & OBJF_TWO_HANDED != 0 && open.is_none() && palm.distance(fore) < FOREGRIP_REACH {
                hover = Hover::Foregrip(wid);
            }
        }
        // 2. Holsters, then the ammo pouch (it beats nearby objects when the other hand has a gun).
        if hover == Hover::None {
            for (slot, z) in zones.iter().enumerate() {
                if self.slots[slot].is_some() && self.anchor.contains(z, palm_t) {
                    hover = Hover::Slot(slot as u32);
                    break;
                }
            }
        }
        // Magazines (unlimited) from the chest and belt pouches, or any empty slot, whenever the
        // other hand holds a magazine-fed gun.
        if hover == Hover::None {
            if let Some(wid) = self.other_weapon(i) {
                // (A magazine that stays in it takes rounds from the pouch one at a time.)
                let o = &self.objects[&wid];
                let wants = o.weapon.as_ref().is_some_and(|w| w.internal.is_none()) || o.desc.weapon_flags & WPN_LOADS_ROUNDS != 0;
                let in_pouch = ammo.iter().any(|z| self.anchor.contains(z, palm_t))
                    || zones.iter().enumerate().any(|(slot, z)| self.slots[slot].is_none() && self.anchor.contains(z, palm_t));
                if wants && in_pouch {
                    hover = Hover::Ammo(wid);
                }
            }
        }
        // A valve's rim within reach.
        if hover == Hover::None {
            let rim = self.objects.iter().filter_map(|(id, o)| Some((*id, o.valve.as_ref()?.nearest_on(&o.valve_shape, palm).1))).filter(|(_, d)| *d < valve::REACH).min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((id, _)) = rim {
                hover = Hover::Valve(id);
            }
        }
        // 3. The nearest free object within reach of the palm (7 cm from its surface).
        if hover == Hover::None {
            let reach = s.interaction.grab_radius;
            let prev = match h.hover {
                Hover::Object(id) => Some(id),
                _ => None,
            };
            let mut best: Option<(u32, f32)> = None;
            for (id, o) in &self.objects {
                if o.desc.flags & OBJF_GRABBABLE == 0 || !matches!(o.holder, Holder::Free | Holder::Pulled { .. } | Holder::Stuck { .. }) {
                    continue;
                }
                let d = self.physics.distance_to(o.collider, palm);
                // Hysteresis: keep the current choice unless another is clearly closer.
                let d = if Some(*id) == prev { d - 0.02 } else { d };
                let mut limit = if Some(*id) == prev { reach * 1.5 } else { reach };
                // Something flying to this hand can be caught from further away.
                if matches!(o.holder, Holder::Pulled { hand, .. } if hand == i) {
                    limit = limit.max(s.interaction.glove_catch_radius);
                }
                if d < limit && best.is_none_or(|b| d < b.1) {
                    best = Some((*id, d));
                }
            }
            if let Some((id, _)) = best {
                hover = Hover::Object(id);
            }
        }
        // A tick as the hand enters a holster or pouch, so you can find them without looking.
        let before = self.hands.as_ref().unwrap()[i].hover;
        if matches!(hover, Hover::Slot(_) | Hover::Ammo(_)) && before != hover {
            self.emit(Cue::ZoneTick, Some(i), None, palm, 1.0);
        }
        let hands = self.hands.as_mut().unwrap();
        hands[i].hover = hover;
    }

    // ---- grip, buttons ------------------------------------------------------------------------------
    fn hand_input(&mut self, i: usize, inp: &RigInput, zones: &[Zone; SLOT_COUNT_BODY], _dom: usize) {
        let s = &inp.settings.interaction;
        let c = &inp.frame.hands[i];
        // Something to throw in a swinging hand (not a gun or a magazine, which you hold on to).
        let throwing = {
            let h = &self.hands.as_ref().unwrap()[i];
            h.holding.and_then(|id| self.objects.get(&id)).is_some_and(|o| o.desc.kind != OBJ_WEAPON && o.desc.kind != OBJ_MAGAZINE) && h.history.recent().length() > THROW_EASE_SPEED
        };
        let (pressed, released) = {
            let h = &mut self.hands.as_mut().unwrap()[i];
            let analog = c.squeeze;
            let mut down = if h.grip_down { analog > s.grip_release } else { analog > s.grip_press && !h.eased };
            if analog <= s.grip_release {
                h.eased = false;
            }
            // Deck's release: mid-swing the grip lets go as it eases a quarter off its hardest
            // squeeze, not only once it's nearly open (a late release is the top throwing complaint).
            if h.grip_down {
                h.squeeze_peak = h.squeeze_peak.max(analog);
                if down && !s.grip_toggle && throwing && analog < h.squeeze_peak * (1.0 - THROW_EASE) {
                    down = false;
                    h.eased = true;
                }
            } else {
                h.squeeze_peak = analog;
            }
            let was = h.grip_down;
            h.grip_down = down;
            let edge_down = down && !was;
            let edge_up = !down && was;
            if s.grip_toggle {
                // Toggle: each press flips holding, so tired hands can relax.
                if edge_down {
                    h.grip_toggled = !h.grip_toggled;
                    (h.grip_toggled, !h.grip_toggled)
                } else {
                    (false, false)
                }
            } else {
                (edge_down, edge_up)
            }
        };
        let buttons = c.buttons;
        let prev = self.hands.as_ref().unwrap()[i].prev_buttons;
        self.hands.as_mut().unwrap()[i].prev_buttons = buttons;
        let tapped = |bit: u32| buttons & bit != 0 && prev & bit == 0;
        let trigger_was = std::mem::replace(&mut self.hands.as_mut().unwrap()[i].trigger_was, c.trigger);
        let pulled = c.trigger > 0.8 && trigger_was <= 0.8;

        if released {
            self.release(i, inp, zones);
        }
        if pressed {
            self.grab(i, inp);
        }
        // A grenade in the hand: the one-handed button arms it, B cooks a held lever.
        if let Some(id) = self.hands.as_ref().unwrap()[i].holding {
            self.grenade_buttons(i, id, tapped(CTRL_A) || pulled, tapped(CTRL_B), inp.settings);
        }
        // Buttons on the gun hand: A/X ejects the magazine, B/Y drops a locked slide.
        if let Some(id) = self.hands.as_ref().unwrap()[i].holding {
            if self.objects.get(&id).is_some_and(|o| o.desc.kind == OBJ_WEAPON) {
                // A hinged action: B (or A, when there's no magazine to drop) is the lever that
                // lets the barrels go; A drops a revolver's speed loader once its cylinder is out.
                let hinge = self.objects[&id].weapon.as_ref().and_then(|w| w.hinge);
                match hinge {
                    Some(h) => {
                        let has_mag = self.objects[&id].weapon.as_ref().is_some_and(|w| w.mag.is_some());
                        if tapped(CTRL_B) || (tapped(CTRL_A) && (h.latched || !has_mag)) {
                            self.open_action(id, i);
                        } else if tapped(CTRL_A) {
                            self.eject(id, i);
                        }
                    }
                    None => {
                        if tapped(CTRL_A) {
                            self.eject(id, i);
                        }
                        if tapped(CTRL_B) {
                            self.slide_release(id, i);
                        }
                    }
                }
            }
        }
    }

    fn grab(&mut self, i: usize, inp: &RigInput) {
        let hover = self.hands.as_ref().unwrap()[i].hover;
        let (palm, grip_rot) = self.hands.as_ref().unwrap()[i].target;
        match hover {
            Hover::None => {
                // A locked gravity-glove target is handled by the gloves.
                match self.hands.as_ref().unwrap()[i].glove {
                    Glove::Hover(id) => {
                        self.set_glove(i, Glove::Locked(id));
                        self.emit(Cue::GloveLock, Some(i), Some(id), palm, 1.0);
                    }
                    // Gripping again within the lock's grace: still locked.
                    Glove::Locked(_) => self.hands.as_mut().unwrap()[i].glove_released = None,
                    _ => {}
                }
            }
            Hover::Object(id) => {
                // Gripped as it flies in from the gloves: a catch (it may still be half a metre out).
                let caught = self.objects.get(&id).is_some_and(|o| matches!(o.holder, Holder::Pulled { .. }));
                self.take(id, i, caught)
            }
            Hover::Valve(id) => {
                // The hand goes to the turning part (or the rim) where it's nearest and holds on
                // there, as it sits.
                let Some(o) = self.objects.get(&id) else { return };
                let Some(v) = o.valve else { return };
                let (at, _) = v.nearest_on(&o.valve_shape, palm);
                let from = self.physics.pose(self.hands.as_ref().unwrap()[i].body);
                let now = self.now;
                let h = &mut self.hands.as_mut().unwrap()[i];
                let wrist = v.wrist(h.target.1);
                h.valve = Some(valve::ValveGrip { id, at, rot: v.rot().inverse() * grip_rot, from, since: now, turn0: v.angle, twisted: 0.0, wrist });
                let c = h.collider;
                self.physics.set_groups(c, groups_none());
                self.emit(Cue::Grab, Some(i), Some(id), v.point(at), 0.7);
            }
            Hover::Slot(slot) => {
                if let Some(id) = self.slots[slot as usize].take() {
                    self.take(id, i, true);
                    self.debug.drawn += 1;
                    self.emit(Cue::HolsterOut, Some(i), Some(id), palm, 1.0);
                }
            }
            Hover::Ammo(wid) => {
                let ammo_type = self.objects[&wid].desc.ammo_type;
                // (With the game's ammunition an empty pouch gives nothing.)
                match self.template_for(ammo_type).map(|t| (self.pouch_rounds(wid, &t), t)) {
                    Some((rounds, t)) if rounds > 0 => {
                        let id = self.spawn_mag(&t, rounds, palm, grip_rot);
                        self.debug.mags_spawned += 1;
                        self.take(id, i, true);
                        self.emit(Cue::Pouch, Some(i), Some(wid), palm, 1.0);
                    }
                    _ => self.emit(Cue::Fail, Some(i), None, palm, 1.0),
                }
            }
            Hover::Foregrip(wid) => {
                let h = &mut self.hands.as_mut().unwrap()[i];
                h.support = Some(wid);
                h.euro.reset();
                self.emit(Cue::Grab, Some(i), Some(wid), palm, 0.7);
            }
            Hover::Handle(wid) => {
                // Where the knob is from the hand (weapon space), so the handle follows the hand
                // wherever the gun goes.
                let knob = self.handle_now(wid);
                let (wp, wr) = self.obj_pose(wid).unwrap_or((Vec3::ZERO, Quat::IDENTITY));
                let offset = knob - wr.inverse() * (palm - wp);
                if let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) {
                    w.handle = Some(HandleGrab { hand: i, start: palm, offset });
                }
                self.hands.as_mut().unwrap()[i].handle = Some(wid);
                self.emit(Cue::Grab, Some(i), Some(wid), palm, 0.6);
            }
            Hover::Barrels(wid) => {
                let (wp, wr) = self.obj_pose(wid).unwrap_or((Vec3::ZERO, Quat::IDENTITY));
                let angle = self.hinge_angle(wid);
                let d = self.objects[&wid].desc;
                // Where on the barrels (shut) the hand has them.
                let at = actions::on_barrels(&d, -angle, wr.inverse() * (palm - wp));
                self.hands.as_mut().unwrap()[i].barrels = Some(BarrelGrab { gun: wid, at, angle });
                self.emit(Cue::Grab, Some(i), Some(wid), palm, 0.6);
            }
            Hover::Chamber(wid, k) => {
                // A live round out of an open chamber, into the hand.
                let ammo_type = self.objects[&wid].desc.ammo_type;
                if let Some(t) = self.template_for(ammo_type) {
                    if let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) {
                        w.chambers[k] = Chamber::Empty;
                    }
                    let id = self.spawn_mag(&t, t.capacity, palm, grip_rot);
                    self.take(id, i, true);
                    self.debug.unloads += 1;
                    self.emit(Cue::ShellOut, Some(i), Some(wid), palm, 1.0);
                }
            }
            Hover::Pin(gid) => {
                self.hands.as_mut().unwrap()[i].pin = Some((gid, palm));
                self.emit(Cue::Grab, Some(i), Some(gid), palm, 0.4);
            }
        }
        let _ = inp;
    }

    /// Puts object `id` in hand `i`. `snap`: ease it into the hand's natural hold (guns always do).
    /// Testing (`RUSTVR_TEST_TAKE`): the nearest of the game's loose props within 6 m of hand `i`
    /// (`ours`: only one the runtime moves itself, not one the game simulates), taken into it as
    /// the gloves' catch would.
    pub fn test_take_nearest(&mut self, i: usize, ours: bool) -> Option<u32> {
        let palm = self.hands.as_ref()?[i].target.0;
        let (id, _) = self
            .objects
            .iter()
            .filter(|(_, o)| o.holder == Holder::Free && o.desc.kind == OBJ_PROP && (!ours || o.desc.flags & OBJF_GAME_SIMULATED == 0))
            .map(|(&id, o)| (id, self.physics.pose(o.body).0.distance(palm)))
            .filter(|&(_, d)| d < 6.0 * self.map.world_per_meter)
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        self.take(id, i, true);
        Some(id)
    }

    fn take(&mut self, id: u32, i: usize, snap: bool) {
        let (palm, grip_rot) = self.hands.as_ref().unwrap()[i].target;
        // Out of the other hand if it was there.
        let other = i ^ 1;
        if self.hands.as_ref().unwrap()[other].holding == Some(id) {
            self.hands.as_mut().unwrap()[other].holding = None;
        }
        let Some(o) = self.objects.get_mut(&id) else { return };
        let (pos, rot) = self.physics.pose(o.body);
        let gun = o.desc.kind == OBJ_WEAPON;
        let hold = authored(o.desc.grip_rot).filter(|_| o.desc.kind == OBJ_THROWABLE);
        o.attach = if gun {
            Attach::Gun
        } else if let Some(g) = hold {
            // A throwable the game rigged sits in the hand the way it's held (a knife by its
            // handle, a grenade with its pin to the other hand), whichever way it was picked up.
            let g = if i == 0 { mirror_grip(g) } else { g };
            let rel = inverse((v3(o.desc.grip), g));
            Attach::Fixed(rel.0, rel.1)
        } else if snap && is_round(&o.desc) {
            // A round sits nose first along the fist's axis, its base in the fist.
            let rot = held_round_rot();
            Attach::Fixed(HELD_ROUND_BASE - rot * v3(o.desc.mag_well), rot)
        } else if snap && o.desc.kind == OBJ_MAGAZINE {
            // A fresh magazine sits in the fist along the grip pose's +Z (the fist's axis, little
            // finger to thumb), its top 6 cm out past the thumb, ready to push into a well.
            let rot = held_mag_rot();
            let top = v3(o.desc.mag_well);
            Attach::Fixed(HELD_MAG_TOP - rot * top, rot)
        } else {
            // Caught or drawn (`snap`): its nearest surface in the palm, as it's turned (it was
            // caught up to `glove_catch_radius` away). Picked up, it stays where the hand took it.
            let at = if snap { pos + (palm - self.physics.closest_point(o.collider, palm)) } else { pos };
            let rel = compose(inverse((palm, grip_rot)), (at, rot));
            Attach::Fixed(rel.0, rel.1)
        };
        o.blend = Some((pos, rot, self.now, pos.distance(palm)));
        if let Holder::Holstered(slot) = o.holder {
            o.home_slot = Some(slot);
        }
        // Not yet moving with the player: it's taken up from where it was.
        (o.carry, o.carried) = (None, (Vec3::ZERO, Vec3::ZERO));
        o.through = false;
        o.holder = Holder::Held(i);
        o.handled = true;
        o.hovered = false;
        if let Some(t) = &mut o.throw {
            t.flight = None;
            t.thrown_at = None;
        }
        self.physics.set_kinematic(o.body, false);
        self.physics.set_gravity(o.body, 1.0);
        self.physics.set_angular_damping(o.body, 0.2);
        self.physics.set_groups(o.collider, groups_held(i));
        let h = &mut self.hands.as_mut().unwrap()[i];
        h.holding = Some(id);
        h.glove = Glove::Idle;
        self.physics.set_groups(h.collider, groups_none());
        self.debug.grabs += 1;
        self.emit(Cue::Grab, Some(i), Some(id), palm, 1.0);
    }

    fn release(&mut self, i: usize, inp: &RigInput, zones: &[Zone; SLOT_COUNT_BODY]) {
        let s = &inp.settings.interaction;
        let (palm, _) = self.hands.as_ref().unwrap()[i].target;
        let palm_t = self.hands.as_ref().unwrap()[i].grip_t.0;
        // Support grip / handle / pin / glove lock end with the grip.
        {
            let h = &mut self.hands.as_mut().unwrap()[i];
            if h.barrels.take().is_some() {
                let c = h.collider;
                self.physics.set_groups(c, groups_hand(i));
                return;
            }
            if h.support.take().is_some() || h.pin.take().is_some() {
                return;
            }
            if h.valve.take().is_some() {
                // Back to the controller from the rim, at a hand's pace, through whatever's between.
                let (b, target) = (h.body, h.target.0);
                let from = self.physics.pose(b);
                let time = travel_time(s.grab_transition, from.0.distance(target));
                self.hands.as_mut().unwrap()[i].returning = Some((from.0, from.1, self.now, time));
                return;
            }
            if let Glove::Locked(_) = h.glove {
                h.glove_released = Some(self.now);
            }
        }
        if let Some(wid) = self.hands.as_mut().unwrap()[i].handle.take() {
            if let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) {
                if w.let_go_handle() {
                    self.emit(Cue::SlideForward, Some(i), Some(wid), palm, 1.0);
                }
            }
            return;
        }
        let Some(id) = self.hands.as_mut().unwrap()[i].holding.take() else { return };
        self.physics.set_groups(self.hands.as_ref().unwrap()[i].collider, groups_hand(i));
        // The other hand was supporting it: it takes the gun.
        if self.hands.as_ref().unwrap()[i ^ 1].support == Some(id) {
            self.hands.as_mut().unwrap()[i ^ 1].support = None;
            self.take(id, i ^ 1, true);
            return;
        }
        let o = &self.objects[&id];
        let (desc, body) = (o.desc, o.body);
        let class = o.throw.as_ref().map(|t| t.class()).unwrap_or(ThrowClass::Generic);
        // Never stowed armed (or with the pin out).
        let stowable = o.throw.as_ref().is_none_or(|t| t.stowable());
        let holsterable = desc.flags & OBJF_HOLSTERABLE != 0 && stowable;
        let hand_speed = self.hands.as_ref().unwrap()[i].history.recent().length();
        // Into a holster or pouch the hand is in, let go of gently (a swing past it throws instead).
        if holsterable && hand_speed < HOLSTER_MAX_SPEED {
            for (slot, z) in zones.iter().enumerate() {
                let free = self.slots[slot].is_none_or(|s| s == id);
                if free && slot_fits(slot as u32, &desc) && self.anchor.contains(z, palm_t) {
                    self.holster(id, slot as u32);
                    self.emit(Cue::HolsterIn, Some(i), Some(id), palm, 1.0);
                    return;
                }
            }
        }
        // Throw (or drop) with the hand's motion at its peak before the release.
        let rel = self.hands.as_ref().unwrap()[i].history.release(self.now);
        let (pos, rot) = self.physics.pose(body);
        // Heavy things leave the hand slower: a rifle at about 80% of the hand's speed.
        let heavy = 1.0 / (1.0 + 0.15 * s.weight * desc.mass.max(0.0));
        let l = throwing::launch(&rel, class, pos - palm, heavy, s.throw_scale);
        let speed = l.lin.length();
        // Guns that came from a holster glide back to it when simply let go (H3VR "harnessing"):
        // the weapon-swapping fix players asked Titanfall 2 VR for.
        let o = self.objects.get_mut(&id).unwrap();
        if o.desc.kind == OBJ_WEAPON && s.auto_holster && speed < 1.5 {
            let slot = o.home_slot.or_else(|| self.slots.iter().position(|x| x.is_none()).map(|x| x as u32));
            if let Some(slot) = slot.filter(|s| self.slots[*s as usize].is_none_or(|x| x == id)) {
                let from = self.physics.pose(o.body);
                o.holder = Holder::Returning { slot, t0: self.now, from };
                o.home_slot = Some(slot);
                self.slots[slot as usize] = Some(id);
                self.physics.set_kinematic(o.body, true);
                self.physics.set_groups(o.collider, groups_none());
                self.debug.returns += 1;
                self.emit(Cue::Release, Some(i), Some(id), palm, 0.6);
                return;
            }
        }
        // A throw leaves from where the object was at the peak, carried along it to now (unless
        // that's far, or through the world).
        let mut at = pos;
        if l.thrown {
            let p = throwing::backdated(rel.peak_pos + (pos - palm), l.lin, rel.t_peak, self.now);
            let d = p - pos;
            if d.length() < 0.35 && (d.length() < 1e-3 || self.physics.ray_world(pos, d, d.length() + 0.05).is_none()) {
                at = p;
            }
        }
        o.hands_off_until = self.now + if speed > 2.0 { 0.2 } else { 0.1 };
        let mut rot = rot;
        let mut ang = l.ang;
        let mut cue = Cue::Release;
        let mut lit = false;
        let mut knife = false;
        if let Some(t) = &mut o.throw {
            // A held lever flies off as it leaves the hand: the fuse starts.
            lit = t.let_go(self.now);
            if l.thrown {
                t.thrown_at = Some(self.now);
                t.flight = Some(Flight { hand: i, t0: self.now, knife: None });
                if t.is_grenade() {
                    cue = Cue::GrenadeThrow;
                }
            }
            if t.is_knife() && l.thrown {
                let (tip, flat) = (rot * t.blade, rot * t.flat);
                let dir = l.lin / speed.max(1e-4);
                let mode = match throwable::knife_mode(l.ang, flat) {
                    KnifeMode::Spin(rate) => {
                        // Spin only about the flat, nudged so the tip comes round at the target.
                        let reach = self.physics.ray_world(at + rot * t.tip, dir, 30.0).unwrap_or(8.0);
                        let f = (dir - flat * dir.dot(flat)).normalize_or(dir);
                        let tp = (tip - flat * tip.dot(flat)).normalize_or(tip);
                        let angle = flat.dot(f.cross(tp)).atan2(f.dot(tp));
                        let rate = throwable::spin_to_land(rate, angle, reach / speed.max(1.0));
                        ang = flat * rate;
                        KnifeMode::Spin(rate)
                    }
                    KnifeMode::Dart => {
                        // Tip first, once, then kept on its flight (see `drive`).
                        rot = (Quat::from_rotation_arc(tip, dir) * rot).normalize();
                        ang = throwable::dart_spin(rot * t.blade, dir, Vec3::ZERO);
                        KnifeMode::Dart
                    }
                };
                t.flight = Some(Flight { hand: i, t0: self.now, knife: Some(mode) });
                cue = Cue::KnifeThrow;
                knife = true;
            }
        }
        if knife {
            self.debug.knife_throws += 1;
        }
        self.free_body(id, groups_held(i)); // no hand contact for a moment
        if at != pos || rot != self.physics.pose(body).1 {
            self.physics.set_pose(body, at, rot);
        }
        if knife {
            // It keeps its spin (or its dart's alignment) in the air.
            self.physics.set_angular_damping(body, 0.0);
        }
        self.physics.set_velocity(body, l.lin, ang);
        if l.thrown && speed > 1.5 {
            self.debug.throws += 1;
            self.debug.last_throw_speed = speed;
            self.debug.last_throw_gain = l.gain;
            self.debug.last_throw_peak_ms = ((self.now - rel.t_peak) * 1000.0) as f32;
            tracing::debug!(
                "rig: throw {speed:.2} m/s (peak {:.2}, at release {:.2}, {:.0} ms before it), gain {:.2}, spin {:.1} rad/s",
                rel.peak_speed,
                rel.release_speed,
                self.debug.last_throw_peak_ms,
                l.gain,
                ang.length()
            );
        }
        self.emit(cue, Some(i), Some(id), palm, if l.thrown { 1.0 } else { 0.7 });
        if lit {
            self.debug.arms += 1;
            self.emit(Cue::GrenadeArm, None, Some(id), at, 0.8);
        }
    }

    fn holster(&mut self, id: u32, slot: u32) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        o.holder = Holder::Holstered(slot);
        o.home_slot = Some(slot);
        self.slots[slot as usize] = Some(id);
        self.physics.set_kinematic(o.body, true);
        self.physics.set_groups(o.collider, groups_none());
        self.debug.holstered += 1;
    }

    // ---- guns ------------------------------------------------------------------------------------
    fn mag_rounds(&mut self, weapon: u32) -> Option<&mut u32> {
        let w = self.objects.get(&weapon)?.weapon.as_ref()?;
        if w.internal.is_some() {
            return self.objects.get_mut(&weapon)?.weapon.as_mut()?.internal.as_mut();
        }
        let mag = w.mag?;
        Some(&mut self.objects.get_mut(&mag)?.rounds)
    }

    fn with_rounds<R>(&mut self, weapon: u32, f: impl FnOnce(&mut WeaponState, &ObjectDesc, &mut u32) -> R) -> Option<R> {
        let mut rounds = self.mag_rounds(weapon).map(|r| *r).unwrap_or(0);
        let o = self.objects.get_mut(&weapon)?;
        let desc = o.desc;
        let w = o.weapon.as_mut()?;
        let r = f(w, &desc, &mut rounds);
        if let Some(slot) = self.mag_rounds(weapon) {
            *slot = rounds;
        }
        Some(r)
    }

    fn eject(&mut self, wid: u32, hand: usize) {
        let swing = self.objects.get(&wid).is_some_and(|o| o.desc.action == ACTION_SWING);
        let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) else { return };
        let Some(mag) = w.mag.take() else { return };
        // A revolver's chambered round is one of its cylinder's: it goes out with the loader.
        if swing && std::mem::take(&mut w.chambered)
            && let Some(m) = self.objects.get_mut(&mag)
        {
            m.rounds = (m.rounds + 1).min(m.desc.capacity).max(m.rounds);
        }
        let (wp, wr) = self.obj_pose(wid).unwrap();
        let (wv, _) = self.physics.velocity(self.objects[&wid].body);
        self.debug.ejects += 1;
        self.emit(Cue::MagEject, Some(hand), Some(wid), wp, 1.0);
        // The other hand waiting under the gun takes the magazine straight out.
        let well = wp + wr * v3(self.objects[&wid].desc.mag_well);
        let other = &self.hands.as_ref().unwrap()[hand ^ 1];
        let free_hand = other.holding.is_none() && other.support.is_none() && other.handle.is_none();
        if let Some(m) = self.objects.get_mut(&mag) {
            m.seat_armed = false;
        }
        if free_hand && other.grip_down && other.target.0.distance(well) < 0.2 {
            if let Some(m) = self.objects.get_mut(&mag) {
                m.holder = Holder::Free;
            }
            self.physics.set_kinematic(self.objects[&mag].body, false);
            self.take(mag, hand ^ 1, false);
            return;
        }
        if let Some(body) = self.objects.get(&mag).map(|m| m.body) {
            self.free_body(mag, groups_free());
            self.physics.set_velocity(body, wv - wr * Vec3::Y * 1.2, Vec3::ZERO);
        }
    }

    /// A hinged action's latch lets go (the lever under the thumb, a flick): the barrels start to
    /// fall open. A support hand on a break action's foregrip now has the barrels.
    fn open_action(&mut self, wid: u32, hand: usize) {
        let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) else { return };
        let Some(h) = &mut w.hinge else { return };
        if !h.unlatch() {
            return;
        }
        self.opened(wid, Some(hand));
    }

    /// The cue for a hinged action that just let go, and the support hand taking its barrels.
    fn opened(&mut self, wid: u32, hand: Option<usize>) {
        let d = self.objects[&wid].desc;
        let p = self.obj_pose(wid).map_or(Vec3::ZERO, |(p, q)| p + q * v3(d.handle));
        self.debug.opens += 1;
        self.emit(if d.action == ACTION_SWING { Cue::SwingOpen } else { Cue::BreakOpen }, hand, Some(wid), p, 1.0);
        if Self::foregrip_on_barrels(&d) {
            for i in 0..2 {
                if self.hands.as_ref().unwrap()[i].support == Some(wid) {
                    let h = &mut self.hands.as_mut().unwrap()[i];
                    h.support = None;
                    h.barrels = Some(BarrelGrab { gun: wid, at: v3(d.foregrip), angle: 0.0 });
                }
            }
        }
    }

    fn slide_release(&mut self, wid: u32, hand: usize) {
        let released = self.with_rounds(wid, |w, _, r| w.release_slide(r)).unwrap_or(false);
        if released {
            let p = self.obj_pose(wid).unwrap().0;
            self.emit(Cue::SlideForward, Some(hand), Some(wid), p, 1.0);
        }
    }

    fn seat(&mut self, mag: u32, wid: u32, feedback: bool) {
        let slam = true;
        let Some(m) = self.objects.get_mut(&mag) else { return };
        m.holder = Holder::InWeapon(wid);
        m.blend = None;
        self.physics.set_kinematic(m.body, true);
        self.physics.set_groups(m.collider, groups_none());
        if let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) {
            w.mag = Some(mag);
        }
        let chambered = self.with_rounds(wid, |w, d, r| w.seated(d, r, slam)).unwrap_or(false);
        if feedback {
            self.debug.seats += 1;
            let p = self.obj_pose(wid).unwrap().0;
            let holder = match self.objects[&wid].holder {
                Holder::Held(h) => Some(h),
                _ => None,
            };
            self.emit(Cue::MagSeat, holder.map(|h| h ^ 1), Some(wid), p, 1.0);
            if let Some(h) = holder {
                self.emit(Cue::MagSeat, Some(h), Some(wid), p, 0.6);
            }
            if chambered {
                self.emit(Cue::SlideForward, holder, Some(wid), p, 0.8);
            }
        }
    }

    /// Weapon world pose: grip on the palm, barrel along the aim, steered by the support hand,
    /// kicked by recoil.
    fn gun_target(&mut self, id: u32, hand: usize, settings: &Settings, dt: f32) -> (Vec3, Quat) {
        let w = &settings.weapon;
        let map = self.map;
        let hands = self.hands.as_mut().unwrap();
        let (palm, _) = hands[hand].target;
        let aim = hands[hand].aim_rot;
        let o = &self.objects[&id];
        let grip = v3(o.desc.grip);
        let offset_rot = Quat::from_euler(EulerRot::YXZ, w.yaw.to_radians(), w.pitch.to_radians(), w.roll.to_radians());
        let (mut pos, mut rot) = match authored(o.desc.grip_rot) {
            // The gun is held the way it was rigged: its grip frame is the controller's grip pose.
            Some(g) => {
                let g = if hand == 0 { mirror_grip(g) } else { g };
                let hand_rot = (hands[hand].target.1 * offset_rot).normalize();
                let rot = (hand_rot * g.inverse()).normalize();
                (palm + hand_rot * Vec3::from_array(w.offset) - rot * grip, rot)
            }
            // Unrigged: grip on the palm, barrel along the controller's aim ray.
            None => {
                let rot = (aim * offset_rot).normalize();
                (palm + aim * Vec3::from_array(w.offset) - rot * grip, rot)
            }
        };
        // Two-handed: pivot about the firing hand so the foregrip line runs through the (steadied)
        // support hand, wherever along it the hand is (twice: the closest point moves as it turns).
        // Steadied in the player's own space, where only the controller moves: in the world the
        // walk moves it too, and the filter lagging the walk swung the gun (and the support hand
        // on it) whenever the walk set off, stopped or turned.
        let support = if hands[hand ^ 1].support == Some(id) { Some(hand ^ 1) } else { None };
        if let Some(sh) = support {
            let raw = hands[sh].grip_t.0;
            let steady = map.point(hands[sh].euro.filter(raw, dt));
            let grip_w = pos + rot * grip;
            for _ in 0..2 {
                let on_line = pos + rot * foregrip_point(&o.desc, rot.inverse() * (steady - pos));
                let cur = (on_line - grip_w).normalize_or_zero();
                let want = (steady - grip_w).normalize_or_zero();
                if cur.length_squared() > 0.5 && want.length_squared() > 0.5 && (steady - grip_w).length() > 0.08 {
                    let q = Quat::from_rotation_arc(cur, want);
                    rot = (q * rot).normalize();
                    pos = grip_w - rot * grip;
                }
            }
        }
        // Virtual stock: shouldered two-handed long guns aim along the line from the shoulder to the
        // support hand, so the gun steadies like a real one braced against you.
        if settings.interaction.virtual_stock && support.is_some() && o.desc.weapon_flags & WPN_STOCK != 0 {
            let shoulder = self.shoulder(hand);
            let grip_w = pos + rot * grip;
            if grip_w.distance(shoulder) < 0.3 {
                let steady = self.hands.as_ref().unwrap()[hand ^ 1].target.0;
                let want = (steady - shoulder).normalize_or_zero();
                let cur = rot * Vec3::Z;
                if want.length_squared() > 0.5 {
                    let q = Quat::from_rotation_arc(cur, want);
                    rot = (q * rot).normalize();
                    pos = grip_w - rot * grip;
                }
            }
        }
        let o = &self.objects[&id];
        if let Some(wst) = &o.weapon {
            let (rr, rt) = wst.recoil.transform(grip);
            pos += rot * rt;
            rot = (rot * rr).normalize();
            let i = &settings.interaction;
            let (gr, gt) = wst.game.transform(self.kicks(id), i.game_recoil_turn, i.game_recoil_push);
            pos += rot * gt;
            rot = (rot * gr).normalize();
        }
        (pos, rot)
    }

    /// The support hand's controller grip rotation in weapon space: the gun's rigged one (authored
    /// for the left hand, mirrored for the right), or palm up under the barrel, fingers across it
    /// and the thumb towards the muzzle.
    fn support_rot(&self, d: &ObjectDesc, hand: usize) -> Quat {
        if let Some(q) = authored(d.support_rot) {
            return if hand == 1 { mirror_grip(q) } else { q };
        }
        let side = if hand == 0 { rustvr_ik::Side::Left } else { rustvr_ik::Side::Right };
        let fingers = if hand == 0 { Vec3::X } else { -Vec3::X };
        let hand_rot = look_rotation_y(fingers, Vec3::Z);
        let o = self.offsets.hand_frame(side, rustvr_ik::Pose::IDENTITY);
        (hand_rot * o.rot.inverse()).normalize()
    }

    /// How braced a held long gun is against the firing shoulder, 0..1: its stock's end (the back of
    /// the gun) within 20 cm of the shoulder is fully braced, past 38 cm not at all. Generous on
    /// purpose (the shoulder is only estimated from the head): no need to line it up.
    fn brace(&self, id: u32, hand: usize) -> f32 {
        let Some(o) = self.objects.get(&id) else { return 0.0 };
        let (p, q) = self.physics.pose(o.body);
        let d = &o.desc;
        let back = p + q * Vec3::new(d.center[0], d.center[1], d.center[2] - d.half_extents[2]);
        let dist = back.distance(self.shoulder(hand)).min((p + q * v3(d.grip)).distance(self.shoulder(hand)) - 0.12);
        1.0 - smooth(((dist - 0.2) / 0.18) as f64)
    }

    /// How much harder than the game's a shot kicks for the way the gun is held (see
    /// [`kick::HoldScale`]): a long gun braced at the shoulder with both hands is the game's; off
    /// the shoulder it kicks `recoil_unbraced` times as hard, in one hand `recoil_one_hand` (half
    /// way back if that hand has the stock at the shoulder). Pistols are the game's in one hand and
    /// steadier in two. The push grows half as much as the turn.
    fn hold_scale(&self, id: u32, hand: usize, two: bool, s: &crate::settings::Interaction) -> kick::HoldScale {
        let Some(o) = self.objects.get(&id) else { return kick::HoldScale::GAME };
        let long = o.desc.weapon_flags & WPN_STOCK != 0 || o.desc.flags & OBJF_TWO_HANDED != 0;
        let turn = if !long {
            if two { 0.75 } else { 1.0 }
        } else {
            let b = self.brace(id, hand);
            let loose = if two { s.recoil_unbraced } else { s.recoil_one_hand };
            let braced = if two { 1.0 } else { 0.5 * (1.0 + s.recoil_one_hand) };
            loose + (braced - loose) * b
        };
        kick::HoldScale { turn, push: 1.0 + (turn - 1.0) * 0.5 }
    }

    /// A held gun's grip is up at the firing shoulder (a shouldered stock takes the recoil).
    fn shouldered(&self, id: u32, hand: usize) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let (p, q) = self.physics.pose(o.body);
        (p + q * v3(o.desc.grip)).distance(self.shoulder(hand)) < 0.3
    }

    /// The firing-side shoulder in the world, estimated from the head: 18 cm to the side, 22 cm down,
    /// 6 cm back, along the body's yaw.
    fn shoulder(&self, hand: usize) -> Vec3 {
        let side = if hand == 1 { 1.0 } else { -1.0 };
        let local = Vec3::new(0.18 * side, -0.22, -0.06);
        let t = self.anchor.head + self.anchor.rot() * local;
        self.map.point(t)
    }

    fn weapons(&mut self, inp: &RigInput, _dom: usize) {
        let dt = inp.dt;
        let ids: Vec<u32> = self.objects.iter().filter(|(_, o)| o.weapon.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            let holder = self.objects[&id].holder;
            let kicks = self.kicks.get(&id).map(Vec::as_slice).unwrap_or(&[]);
            if let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) {
                w.recoil.update(dt);
                w.game.update(dt, kicks);
            }
            let Holder::Held(hand) = holder else { continue };
            let pull = self.hands.as_ref().unwrap()[hand].trigger;
            let two = self.hands.as_ref().unwrap()[hand ^ 1].support == Some(id);
            let now = self.now;
            let fired = self.with_rounds(id, |w, d, r| w.trigger(d, pull, now, r)).unwrap_or(Fire::None);
            let (wp, wr) = self.obj_pose(id).unwrap();
            let d = self.objects[&id].desc;
            match fired {
                Fire::Shot | Fire::LastShot => {
                    let stocked = settings_stock(inp.settings, &d) && two && self.shouldered(id, hand);
                    let hold = if stocked { recoil::Hold::SHOULDERED } else if two { recoil::Hold::TWO_HANDS } else { recoil::Hold::ONE_HAND };
                    // The game's own recoil when it has one: aimed when shouldered or with the
                    // sights at the eye.
                    let game = inp.settings.interaction.game_recoil && !self.kicks(id).is_empty();
                    let head = self.map.point(self.anchor.head);
                    let aimed = self.shouldered(id, hand) || (wp + wr * v3(d.sight)).distance(head) < AIMED_SIGHT;
                    let stance = kick::Stance { aimed, crouched: inp.crouched };
                    let scale = self.hold_scale(id, hand, two, &inp.settings.interaction);
                    let kicks = self.kicks.get(&id).map(Vec::as_slice).unwrap_or(&[]);
                    if let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) {
                        if game {
                            w.game.kick(kicks, stance, scale);
                            self.debug.game_kicks += 1;
                        } else {
                            w.recoil.kick(&d, hold);
                        }
                    }
                    self.debug.shots += 1;
                    let cue = if d.weapon_flags & WPN_PISTOL != 0 { Cue::ShotPistol } else { Cue::ShotRifle };
                    let muzzle = wp + wr * v3(d.muzzle);
                    self.emit(cue, Some(hand), Some(id), muzzle, 1.0);
                    if two {
                        self.emit(cue, Some(hand ^ 1), Some(id), muzzle, 0.5);
                    }
                    if fired == Fire::LastShot {
                        self.emit(Cue::LastRound, None, Some(id), wp, 1.0);
                    }
                    if let Some(w) = self.objects[&id].weapon.as_ref().filter(|w| w.air.is_some()) {
                        self.debug.last_strength = w.last_strength;
                    }
                }
                Fire::Empty => {
                    self.debug.dry_fires += 1;
                    self.emit(Cue::EmptyClick, Some(hand), Some(id), wp, 1.0);
                }
                Fire::NoAir => {
                    self.debug.hisses += 1;
                    self.emit(Cue::AirHiss, Some(hand), Some(id), wp + wr * v3(d.muzzle), 1.0);
                }
                Fire::None => {}
            }
            // A hinged action: its barrels swing on the gun.
            self.hinge_frame(id, hand, dt);
            // A bolt or a pump worked by the other hand, or an air gun's pump.
            let manual = self.objects[&id].weapon.as_ref().is_some_and(|w| w.cycle.is_some() || w.air.is_some());
            if manual {
                self.cycle_frame(id);
            }
            // Racking: the other hand on the charging handle pulls it back along the barrel.
            let grab = self.objects[&id].weapon.as_ref().unwrap().handle.filter(|_| !manual);
            if let Some(g) = grab {
                let hand_pos = self.hands.as_ref().unwrap()[g.hand].target.0;
                let pull_m = (g.start - hand_pos).dot(wr * Vec3::Z).max(0.0);
                let travel = d.handle_travel;
                let (racked, forward) = self.with_rounds(id, |w, _, r| w.pull_handle(pull_m, travel, r)).unwrap_or((false, false));
                if racked {
                    self.debug.racks += 1;
                    self.emit(Cue::SlideBack, Some(g.hand), Some(id), wp, 1.0);
                }
                if forward {
                    self.emit(Cue::SlideForward, Some(g.hand), Some(id), wp, 1.0);
                }
                // Pulling the hand away past the handle's travel lets go of it.
                if hand_pos.distance(g.start) > travel.max(0.06) + 0.25 {
                    self.hands.as_mut().unwrap()[g.hand].handle = None;
                    if let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) {
                        w.let_go_handle();
                    }
                }
            }
            // A held magazine near this gun's magazine well seats itself; a round near an open
            // chamber goes in.
            if let Some(mag) = self.hands.as_ref().unwrap()[hand ^ 1].holding {
                if d.weapon_flags & (WPN_LOADS_CHAMBERS | WPN_LOADS_ROUNDS) != 0 {
                    self.try_load(mag, id, inp.settings.interaction.easy_mag, hand ^ 1);
                } else {
                    self.try_seat(mag, id, inp.settings.interaction.easy_mag, hand ^ 1);
                }
            }
        }
    }

    /// A held hinged gun's barrels for one frame: their weight swings them (gravity in the gun's
    /// frame), a flick of the gun lets them go or throws them shut, a hand holding them carries
    /// them. Opened far enough the spent cases fly out; back home, they latch.
    fn hinge_frame(&mut self, id: u32, hand: usize, dt: f32) {
        let Some(o) = self.objects.get(&id) else { return };
        let Some(h) = o.weapon.as_ref().and_then(|w| w.hinge) else { return };
        let d = o.desc;
        let (wp, wr) = self.physics.pose(o.body);
        // The flick is the wrist's: the controller's turn about the hinge (a heavy gun lags it).
        let ang = self.hands.as_ref().unwrap()[hand].turn_rate;
        let (axis, sign) = actions::hinge_axis(&d);
        let flick = sign * (wr.inverse() * ang).dot(axis);
        let gravity = wr.inverse() * Vec3::new(0.0, -9.81, 0.0);
        let pull = actions::gravity_pull(&d, h.angle, gravity);
        // A hand on the barrels (the other one: the gun hand holds the grip).
        let grab = (0..2).find_map(|i| self.hands.as_ref().unwrap()[i].barrels.filter(|b| b.gun == id).map(|b| (i, b)));
        let carried = grab.map(|(i, b)| {
            let palm = self.hands.as_ref().unwrap()[i].target.0;
            let took = actions::on_barrels(&d, b.angle, b.at);
            actions::hand_angle(&d, took, b.angle, wr.inverse() * (palm - wp))
        });
        let mut events = Vec::new();
        let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) else { return };
        let Some(hinge) = &mut w.hinge else { return };
        hinge.step(dt, d.action_angle, pull, flick, carried, &mut events);
        w.slide = hinge.fraction(d.action_angle);
        for e in events {
            match e {
                HingeEvent::Unlatched => self.opened(id, Some(hand)),
                HingeEvent::Eject => {
                    // The extractor throws the spent cases out (live rounds stay, lifted).
                    let angle = self.hinge_angle(id);
                    let out = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()).map(|w| w.throw_out_cases()).unwrap_or_default();
                    for k in out {
                        self.debug.case_ejects += 1;
                        let at = wp + wr * actions::on_barrels(&d, angle, actions::chamber_mouth(&d, k));
                        self.emit(Cue::CaseEject, None, Some(id), at, 0.8);
                    }
                }
                HingeEvent::Latched => {
                    self.debug.closes += 1;
                    let at = wp + wr * v3(d.handle);
                    self.emit(if d.action == ACTION_SWING { Cue::SwingClose } else { Cue::BreakClose }, Some(hand), Some(id), at, 1.0);
                    // A hand that pushed them home lets go of them, or holds the gun's foregrip
                    // again if that's where it had them.
                    if let Some((i, b)) = grab {
                        let h = &mut self.hands.as_mut().unwrap()[i];
                        h.barrels = None;
                        if Self::foregrip_on_barrels(&d) && b.at.distance(v3(d.foregrip)) < FOREGRIP_REACH {
                            h.support = Some(id);
                            h.euro.reset();
                        } else {
                            let c = h.collider;
                            self.physics.set_groups(c, groups_hand(i));
                        }
                    }
                }
            }
        }
    }

    /// The other hand working a bolt or a pump (or an air gun's pump) for one frame: the handle goes
    /// where the hand takes it along its path, and the stroke throws the case out and chambers the
    /// next round (or builds the pressure).
    fn cycle_frame(&mut self, id: u32) {
        let Some(o) = self.objects.get(&id) else { return };
        let Some(w) = o.weapon.as_ref() else { return };
        let Some(g) = w.handle else { return };
        let d = o.desc;
        let (wp, wr) = self.physics.pose(o.body);
        let hand_pos = self.hands.as_ref().unwrap()[g.hand].target.0;
        let knob = wr.inverse() * (hand_pos - wp) + g.offset;
        let home = v3(d.handle);
        let (lift, back) = Cycle::from_knob(&d, home, knob);
        let at = wp + wr * self.handle_now(id);
        if w.air.is_some() {
            let gain = d.pump_pressure;
            let stroke = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()).is_some_and(|w| w.pump(back, gain));
            if stroke {
                self.debug.pumps += 1;
                self.emit(Cue::Pump, Some(g.hand), Some(id), at, 1.0);
            }
        } else {
            let lifts = d.action == ACTION_BOLT && d.action_angle.abs() > 1e-3;
            let mut events = Vec::new();
            let thrown = self.with_rounds(id, |w, _, r| w.work(lift, back, lifts, r, &mut events)).flatten();
            let pump = d.action == ACTION_PUMP;
            for e in events {
                let cue = match e {
                    CycleEvent::Unlocked => Cue::BoltUp,
                    CycleEvent::Back => {
                        self.debug.racks += 1;
                        if pump { Cue::PumpBack } else { Cue::BoltBack }
                    }
                    CycleEvent::Home => {
                        self.debug.cycles += 1;
                        if pump { Cue::PumpForward } else { Cue::BoltForward }
                    }
                    CycleEvent::Locked => Cue::BoltDown,
                };
                self.emit(cue, Some(g.hand), Some(id), at, 1.0);
            }
            if thrown.is_some() {
                self.debug.case_ejects += 1;
                self.emit(Cue::CaseEject, None, Some(id), at, 0.8);
            }
        }
        // Pulling the hand well away from the handle lets go of it (it stays where it is).
        let knob_now = wp + wr * self.handle_now(id);
        if hand_pos.distance(knob_now) > 0.25 {
            self.hands.as_mut().unwrap()[g.hand].handle = None;
            if let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) {
                w.let_go_handle();
            }
        }
    }

    /// A held round pushed into one of an open gun's empty chambers, or through the port of a
    /// magazine that stays in the gun: its base to the mouth, pointing along the bore. It goes in
    /// (and out of the hand) the moment it's there.
    fn try_load(&mut self, round: u32, wid: u32, easy: bool, round_hand: usize) {
        let (Some(r), Some(w)) = (self.objects.get(&round), self.objects.get(&wid)) else { return };
        if !is_round(&r.desc) || r.desc.ammo_type != w.desc.ammo_type {
            return;
        }
        let Some(ws) = w.weapon.as_ref() else { return };
        let d = w.desc;
        // A magazine that stays in the gun: one more through its port while there's room (at any
        // time). Chambers: an empty one, with the action open.
        let tube = d.weapon_flags & WPN_LOADS_ROUNDS != 0;
        let angle = ws.hinge.map_or(0.0, |h| if h.latched { 0.0 } else { h.angle });
        let open = ws.hinge.is_some_and(|h| !h.latched && h.angle >= LOAD_OPEN.to_radians().min(0.6 * d.action_angle.abs())) || ws.cycle.is_some_and(|c| c.back > 0.8);
        let room = ws.internal.unwrap_or(0) + (ws.chambered as u32) < d.capacity;
        if if tube { !room } else { !open } {
            return;
        }
        let (rp, rq) = self.physics.pose(r.body);
        let (wp, wr) = self.physics.pose(w.body);
        let base = rp + rq * v3(r.desc.mag_well);
        let along = rq * Vec3::Z;
        let bore = wr * actions::hinge_rot(&d, angle) * Vec3::Z;
        let (reach, max_angle) = if easy { (ROUND_SEAT, ROUND_SEAT_ANGLE) } else { (ROUND_SEAT * 0.5, ROUND_SEAT_ANGLE * 0.5) };
        if along.angle_between(bore) > max_angle.to_radians() {
            return;
        }
        let mouths: Vec<(usize, Vec3)> = if tube {
            vec![(0, wp + wr * v3(d.mag_well))]
        } else {
            (0..ws.chambers.len()).filter(|&k| ws.chambers[k] == Chamber::Empty).map(|k| (k, wp + wr * actions::on_barrels(&d, angle, actions::chamber_mouth(&d, k)))).collect()
        };
        let best = mouths.into_iter().map(|(k, at)| (k, at, at.distance(base))).filter(|(_, _, dist)| *dist < reach).min_by(|a, b| a.2.total_cmp(&b.2));
        let Some((k, mouth, _)) = best else { return };
        let needs_rack = d.weapon_flags & WPN_NEEDS_RACK != 0;
        let loaded = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()).is_some_and(|ws| {
            if !tube {
                return ws.load_chamber(k);
            }
            // In through the port; a gun that cycles itself with its chamber empty takes it
            // straight in (a revolving shotgun's cylinder turns it under the hammer).
            *ws.internal.get_or_insert(0) += 1;
            if !ws.chambered && !needs_rack && ws.cycle.is_none() {
                ws.internal = ws.internal.map(|n| n - 1);
                ws.chambered = true;
            }
            true
        });
        if loaded {
            let h = &mut self.hands.as_mut().unwrap()[round_hand];
            h.holding = None;
            let c = h.collider;
            self.physics.set_groups(c, groups_hand(round_hand));
            self.remove(round);
            self.debug.loads += 1;
            self.emit(Cue::ShellIn, Some(round_hand), Some(wid), mouth, 1.0);
            let holder = match self.objects[&wid].holder {
                Holder::Held(h) => Some(h),
                _ => None,
            };
            if let Some(h) = holder {
                self.emit(Cue::ShellIn, Some(h), Some(wid), mouth, 0.5);
            }
        }
    }

    fn try_seat(&mut self, mag: u32, wid: u32, easy: bool, mag_hand: usize) {
        let (Some(m), Some(w)) = (self.objects.get(&mag), self.objects.get(&wid)) else { return };
        if m.desc.kind != OBJ_MAGAZINE || m.desc.ammo_type != w.desc.ammo_type {
            return;
        }
        if w.weapon.as_ref().is_none_or(|ws| ws.mag.is_some() || ws.internal.is_some()) {
            return;
        }
        // A swing-out cylinder takes its speed loader only while it's out.
        let hinge = w.weapon.as_ref().and_then(|ws| ws.hinge);
        if hinge.is_some_and(|h| h.latched || h.angle < LOAD_OPEN.to_radians().min(0.6 * w.desc.action_angle.abs())) {
            return;
        }
        let turn = actions::hinge_rot(&w.desc, hinge.map_or(0.0, |h| h.angle));
        let (mp, mr) = self.physics.pose(m.body);
        let (wp, wr) = self.physics.pose(w.body);
        let top = mp + mr * v3(m.desc.mag_well);
        let well = wp + wr * actions::on_barrels(&w.desc, hinge.map_or(0.0, |h| h.angle), v3(w.desc.mag_well));
        if !m.seat_armed {
            if top.distance(well) > 0.12 {
                self.objects.get_mut(&mag).unwrap().seat_armed = true;
            }
            return;
        }
        let angle = (mr * Vec3::Y).angle_between(wr * turn * Vec3::Y);
        // Easy (default) 9 cm / 45 degrees, strict 4 cm / 25 degrees.
        let (radius, max_angle) = if easy { (0.09, 45f32.to_radians()) } else { (0.04, 25f32.to_radians()) };
        if top.distance(well) < radius && angle < max_angle {
            self.hands.as_mut().unwrap()[mag_hand].holding = None;
            self.physics.set_groups(self.hands.as_ref().unwrap()[mag_hand].collider, groups_hand(mag_hand));
            self.seat(mag, wid, true);
        }
    }

    // ---- throwables -----------------------------------------------------------------------------------
    /// The buttons on a hand holding a grenade: `arm` (A or a trigger pull) arms it one-handed,
    /// where that's how it arms (the button scheme, no pin, or the other hand busy with a gun);
    /// `cook` (B) pops a held lever so the fuse burns in the hand.
    fn grenade_buttons(&mut self, i: usize, id: u32, arm: bool, cook: bool, settings: &Settings) {
        let other_gun = self.other_weapon(i).is_some();
        let Some(o) = self.objects.get_mut(&id) else { return };
        let Some(t) = o.throw.as_mut().filter(|t| t.is_grenade()) else { return };
        let one_handed = settings.interaction.grenade_arming == GrenadeArming::Button || !t.has_pin() || other_gun;
        let had_pin = t.has_pin() && t.fuse == Fuse::Safe;
        let lit = if arm && one_handed && t.fuse == Fuse::Safe {
            t.arm(self.now)
        } else if cook && t.fuse == Fuse::PinOut {
            t.arm(self.now)
        } else {
            false
        };
        if lit {
            let p = self.physics.pose(o.body).0;
            if had_pin {
                self.debug.pins += 1;
                self.emit(Cue::GrenadePin, Some(i), Some(id), p, 0.7);
            }
            self.debug.arms += 1;
            self.emit(Cue::GrenadeArm, Some(i), Some(id), p, 1.0);
        }
    }

    /// Pins being pulled, fuses burning and ticking.
    fn throwables(&mut self) {
        let now = self.now;
        for i in 0..2 {
            let Some((gid, start)) = self.hands.as_ref().unwrap()[i].pin else { continue };
            let safe = self.objects.get(&gid).is_some_and(|o| o.holder == Holder::Held(i ^ 1) && o.throw.as_ref().is_some_and(|t| t.fuse == Fuse::Safe));
            if !safe {
                self.hands.as_mut().unwrap()[i].pin = None;
                continue;
            }
            let palm = self.hands.as_ref().unwrap()[i].target.0;
            let o = self.objects.get_mut(&gid).unwrap();
            let travel = if o.desc.handle_travel > 0.0 { o.desc.handle_travel } else { throwable::DEFAULT_PIN_TRAVEL };
            if palm.distance(start) < travel {
                continue;
            }
            // Out it comes: two crisp clicks, in both hands; without a lever the fuse is lit.
            let lit = o.throw.as_mut().unwrap().pull_pin(now);
            let p = self.physics.pose(o.body).0;
            self.hands.as_mut().unwrap()[i].pin = None;
            self.debug.pins += 1;
            self.emit(Cue::GrenadePin, Some(i), Some(gid), palm, 1.0);
            self.emit(Cue::GrenadePin, Some(i ^ 1), Some(gid), p, 0.7);
            if lit {
                self.debug.arms += 1;
                self.emit(Cue::GrenadeArm, Some(i ^ 1), Some(gid), p, 1.0);
            }
        }
        let lit: Vec<u32> = self.objects.iter().filter(|(_, o)| o.throw.as_ref().is_some_and(|t| t.lit())).map(|(id, _)| *id).collect();
        for id in lit {
            let o = self.objects.get_mut(&id).unwrap();
            let t = o.throw.as_mut().unwrap();
            if t.due(now) {
                self.detonate(id);
                continue;
            }
            if let Some(k) = t.tick(now) {
                let hand = if let Holder::Held(h) = o.holder { Some(h) } else { None };
                let p = self.physics.pose(o.body).0;
                self.emit(Cue::GrenadeTick, hand, Some(id), p, k);
            }
        }
    }

    /// A grenade goes off: out of any hand, inert, and the game told (its `shots`).
    fn detonate(&mut self, id: u32) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        let Some(t) = o.throw.as_mut() else { return };
        t.went_off();
        o.holder = Holder::Gone;
        let (body, collider) = (o.body, o.collider);
        let (p, _) = self.physics.pose(body);
        self.physics.set_kinematic(body, true);
        self.physics.set_groups(collider, groups_none());
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        for s in self.slots.iter_mut().filter(|s| **s == Some(id)) {
            *s = None;
        }
        let hands = self.hands.as_mut().unwrap();
        for (i, h) in hands.iter_mut().enumerate() {
            if h.holding == Some(id) {
                h.holding = None;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
            if h.pin.is_some_and(|(g, _)| g == id) {
                h.pin = None;
            }
        }
        self.debug.detonations += 1;
        self.debug.last_detonation = Some(self.map.inv_point(p));
        self.emit(Cue::Explosion, None, Some(id), p, 1.0);
        for i in 0..2 {
            let d = self.hands.as_ref().unwrap()[i].target.0.distance(p);
            let k = (1.0 - d / 15.0).clamp(0.0, 1.0);
            self.emit(Cue::ExplosionRumble, Some(i), Some(id), p, k * k);
        }
    }

    /// A thrown knife (tip first) or sticky bomb meets something at `hit` going at `v`: it stays
    /// there, in the tip-forward pose, riding with what it's in.
    fn stick(&mut self, id: u32, hit: Vec3, v: Vec3, into: rapier3d::prelude::ColliderHandle, hand: usize) {
        let parent = if self.physics.is_world(into) { 0 } else { self.objects.iter().find(|(_, o)| o.collider == into).map(|(id, _)| *id).unwrap_or(0) };
        let Some(o) = self.objects.get(&id) else { return };
        let Some(t) = o.throw.as_ref() else { return };
        let (_, q) = self.physics.pose(o.body);
        let dir = v.normalize_or(Vec3::Z);
        let (pos, rot, tip) = if t.is_knife() {
            let rot = (Quat::from_rotation_arc((q * t.blade).normalize(), dir) * q).normalize();
            let tip = hit + dir * throwable::STICK_DEPTH;
            (tip - rot * t.tip, rot, tip)
        } else {
            // A sticky bomb: against the surface where it touched.
            let r = o.desc.half_extents.iter().fold(f32::MAX, |a, b| a.min(*b)).max(o.desc.radius).max(0.01);
            let at = hit - dir * r;
            (at, q, at)
        };
        let rel = match self.obj_pose(parent).filter(|_| parent != 0) {
            Some(pp) => compose(inverse(pp), (pos, rot)),
            None => (pos, rot),
        };
        let o = self.objects.get_mut(&id).unwrap();
        o.holder = Holder::Stuck { parent, rel };
        if let Some(t) = o.throw.as_mut() {
            t.flight = None;
            t.thrown_at = None;
        }
        let (body, collider) = (o.body, o.collider);
        self.physics.set_kinematic(body, true);
        self.physics.set_pose(body, pos, rot);
        self.physics.move_kinematic(body, pos, rot);
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        self.physics.set_groups(collider, groups_free());
        self.physics.set_angular_damping(body, 0.2);
        self.debug.sticks += 1;
        self.debug.last_stick = Some(self.map.inv_point(tip));
        self.emit(Cue::KnifeStick, Some(hand), Some(id), hit, 1.0);
    }

    /// A throw in the air: a dart keeps its tip on its flight, a spinner its spin, and a knife or
    /// sticky bomb sticks when it meets something the right way (swept ahead of the step, so it
    /// doesn't bounce first).
    fn fly(&mut self, id: u32, dt: f32, assist: bool) {
        let o = &self.objects[&id];
        let Some(t) = o.throw.as_ref() else { return };
        let Some(fl) = t.flight else { return };
        let (p, q) = self.physics.pose(o.body);
        let (v, w) = self.physics.velocity(o.body);
        let (body, collider) = (o.body, o.collider);
        let (sticky, knife) = (t.is_sticky(), t.is_knife());
        let (tip, blade, flat) = (t.tip, t.blade, t.flat);
        let done = self.now - fl.t0 > throwable::FLIGHT_TIME || (v.length() < 0.5 && self.now - fl.t0 > 0.15);
        if done {
            if let Some(t) = self.objects.get_mut(&id).and_then(|o| o.throw.as_mut()) {
                t.flight = None;
            }
            self.physics.set_angular_damping(body, 0.2);
            return;
        }
        if (knife && fl.knife.is_some()) || sticky {
            let from = if knife { p + q * tip } else { p };
            let reach = v.length() * dt * 1.25 + 0.02;
            if let Some((toi, c, _)) = self.physics.ray_cast(from, v, reach, collider) {
                if sticky || throwable::sticks(q * blade, v, fl.knife, assist) {
                    self.stick(id, from + v.normalize_or_zero() * toi, v, c, fl.hand);
                    return;
                }
                // It glanced off: no more help, it tumbles.
                if let Some(t) = self.objects.get_mut(&id).and_then(|o| o.throw.as_mut()) {
                    t.flight = Some(Flight { knife: None, ..fl });
                }
                self.physics.set_angular_damping(body, 0.2);
                return;
            }
        }
        match fl.knife {
            Some(KnifeMode::Dart) if v.length() > 1.0 => self.physics.set_velocity(body, v, throwable::dart_spin(q * blade, v, w)),
            Some(KnifeMode::Spin(rate)) => self.physics.set_velocity(body, v, q * flat * rate),
            _ => {}
        }
    }

    /// The game's explosion at `at` pushes the loose objects within `radius` (`impulse` N s at
    /// the centre, falling off linearly), on the next step.
    pub fn blast(&mut self, at: Vec3, radius: f32, impulse: f32) {
        if at.is_finite() && radius > 0.0 && impulse.is_finite() {
            self.pushes.push(Push::Blast { at, radius, impulse });
        }
    }

    /// A push on one loose object (N s at world point `at`), on the next step.
    pub fn impulse(&mut self, id: u32, impulse: Vec3, at: Vec3) {
        if impulse.is_finite() && at.is_finite() {
            self.pushes.push(Push::One { id, impulse: impulse.clamp_length_max(50.0), at });
        }
    }

    fn apply_pushes(&mut self) {
        for push in std::mem::take(&mut self.pushes) {
            match push {
                Push::Blast { at, radius, impulse } => {
                    for o in self.objects.values().filter(|o| matches!(o.holder, Holder::Free | Holder::Pulled { .. })) {
                        let (p, q) = self.physics.pose(o.body);
                        let c = p + q * v3(o.desc.center);
                        let d = c - at;
                        let k = 1.0 - d.length() / radius;
                        if k > 0.0 {
                            // Out from the blast and a little up (it's on the ground).
                            let dir = (d.normalize_or(Vec3::Y) + Vec3::Y * 0.35).normalize();
                            self.physics.apply_impulse(o.body, dir * impulse * k, c);
                        }
                    }
                }
                Push::One { id, impulse, at } => {
                    if let Some(o) = self.objects.get(&id).filter(|o| matches!(o.holder, Holder::Free | Holder::Pulled { .. })) {
                        self.physics.apply_impulse(o.body, impulse, at);
                    }
                }
            }
        }
    }

    // ---- gravity gloves (Half-Life: Alyx's) -----------------------------------------------------------
    /// Where a hand reaches the gloves: along the arm, from the shoulder out through the hand (a
    /// hand reaching for a thing beyond it, held however it's naturally held). Alyx aims along
    /// one direction, an attachment authored on its hand model
    /// (`vr_controller_grabbity_gun_forward_pull`, server.dll `0xD5D0E0`): a reach, not the
    /// controller's nose or the palm's normal.
    fn glove_dir(&self, i: usize, head: (Vec3, Quat)) -> Vec3 {
        let h = &self.hands.as_ref().unwrap()[i];
        // The shoulder: below the eyes and out to the hand's side, turned with the head's yaw.
        let fwd = (head.1 * Vec3::Z).with_y(0.0).normalize_or(Vec3::Z);
        let right = Vec3::Y.cross(fwd).normalize_or(Vec3::X);
        let shoulder = head.0 - Vec3::Y * SHOULDER_DROP + right * if i == 0 { -SHOULDER_OUT } else { SHOULDER_OUT };
        (h.target.0 - shoulder).normalize_or(fwd)
    }

    /// Whether `p` is in Alyx's targeting window: within `VIEW_HALF_*` of the point a little below
    /// where the head looks.
    fn in_view(head: (Vec3, Quat), p: Vec3) -> bool {
        let local = head.1.inverse() * (p - head.0);
        if local.z <= 0.0 {
            return false;
        }
        let x = (local.x / local.z).atan().to_degrees().abs();
        let y = (local.y / local.z - alyx::VIEW_DOWN).atan().to_degrees().abs();
        x <= alyx::VIEW_HALF_WIDE && y <= alyx::VIEW_HALF_HIGH
    }

    /// The object the gloves would take: the free, pullable one nearest the middle of Alyx's cone
    /// (15 degrees about either direction, from 25 cm behind the hand, out to `range`), with a clear
    /// line to it. The current target keeps hold out to 20 degrees.
    fn glove_target(&self, origin: Vec3, dir: Vec3, head: (Vec3, Quat), current: Option<u32>, range: f32) -> Option<u32> {
        let mut best: Option<(u32, f32)> = None;
        for (id, o) in &self.objects {
            let pullable = o.desc.flags & OBJF_PULLABLE != 0 || (o.desc.flags & OBJF_GRABBABLE != 0 && o.desc.mass <= 8.0);
            if !pullable || o.holder != Holder::Free {
                continue;
            }
            let (p, _) = self.physics.pose(o.body);
            let to = p - origin;
            let d = to.length();
            if d < alyx::MIN_DIST || d > range {
                continue;
            }
            if !Self::in_view(head, p) {
                continue;
            }
            let cos = dir.dot((p - (origin - dir * alyx::CONE_BACK)).normalize_or_zero());
            let limit = if Some(*id) == current { alyx::HOLD_COS } else { alyx::CONE_COS };
            if cos < limit || self.physics.ray_world(origin, to, d - 0.05).is_some() {
                continue;
            }
            let score = (1.0 - cos) + d * 0.001;
            if best.is_none_or(|b| score < b.1) {
                best = Some((*id, score));
            }
        }
        best.map(|b| b.0)
    }

    fn gloves(&mut self, i: usize, inp: &RigInput) {
        let s = &inp.settings.interaction;
        let now = self.now;
        let h = &self.hands.as_ref().unwrap()[i];
        if !s.gravity_gloves || h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.barrels.is_some() || h.valve.is_some() {
            if !matches!(h.glove, Glove::Flight(_)) {
                self.set_glove(i, Glove::Idle);
            }
            return;
        }
        let origin = h.target.0;
        let head = (inp.map.point(v3(inp.frame.hmd.pos)), inp.map.rot(q4(inp.frame.hmd.rot)));
        let dir = self.glove_dir(i, head);
        match h.glove {
            Glove::Idle | Glove::Hover(_) => {
                if h.hover != Hover::None {
                    self.set_glove(i, Glove::Idle);
                    return;
                }
                let current = if let Glove::Hover(id) = h.glove { Some(id) } else { None };
                let best = self.glove_target(origin, dir, head, current, s.glove_range);
                let h = &mut self.hands.as_mut().unwrap()[i];
                if best == current {
                    h.glove_pending = None;
                    return;
                }
                // Alyx takes a new target (or a change, or none) once it's held for 0.1 s.
                let since = match h.glove_pending {
                    Some((p, t)) if p == best => t,
                    _ => {
                        h.glove_pending = Some((best, now));
                        now
                    }
                };
                let wait = if current.is_none() { alyx::SELECT_TIME } else { alyx::CHANGE_TIME };
                if now - since >= wait {
                    h.glove_pending = None;
                    match best {
                        Some(id) => {
                            self.emit(Cue::GloveHover, Some(i), Some(id), origin, 0.6);
                            self.set_glove(i, Glove::Hover(id));
                        }
                        None => self.set_glove(i, Glove::Idle),
                    }
                }
            }
            Glove::Locked(id) => {
                let Some(o) = self.objects.get(&id).filter(|o| o.holder == Holder::Free) else {
                    self.set_glove(i, Glove::Idle);
                    return;
                };
                let body = o.body;
                let (p, _) = self.physics.pose(body);
                let to = (p - origin).normalize_or_zero();
                // Locked in while the grip holds: let go for 0.25 s, it drops the lock.
                if h.glove_released.is_some_and(|t| now - t > alyx::LOCK_RELEASE) {
                    self.set_glove(i, Glove::Idle);
                    return;
                }
                // Locked on, it strains towards the hand: a hop now and then while it rests.
                if now >= h.glove_hop {
                    let (v, w) = self.physics.velocity(body);
                    if v.length() < 0.3 {
                        let first = h.glove_hop == 0.0;
                        let up = if first { alyx::LOCK_HOP } else { alyx::STRAIN_HOP };
                        let toward = Vec3::new(-to.x, 0.0, -to.z) * up * 0.2;
                        let spin = Vec3::new(to.z, 0.0, -to.x) * if first { 3.0 } else { 1.0 };
                        self.physics.set_velocity(body, v + Vec3::Y * up + toward, w + spin);
                    }
                    self.hands.as_mut().unwrap()[i].glove_hop = now + alyx::STRAIN_EVERY;
                }
                // The flick, whichever way the hand faces now: it yanks back from the thing, in
                // towards you, or up; or the wrist flicks back.
                let h = &self.hands.as_ref().unwrap()[i];
                let v = h.history.recent();
                let ang = self.map.vel(h.ang_t) / self.map.world_per_meter.max(1e-6);
                let flat = Vec3::new(to.x, 0.0, to.z).normalize_or_zero();
                let wrist = ang.dot(flat.cross(Vec3::Y));
                let head = inp.map.point(v3(inp.frame.hmd.pos));
                let inward = (head - origin).normalize_or_zero();
                let yank = (-to).dot(v).max(inward.dot(v)).max(v.y * 0.8);
                if yank > s.glove_flick_speed || wrist > alyx::FLICK_WRIST {
                    let palm = origin;
                    let (dur, g) = alyx_pull(p.distance(palm));
                    let o = self.objects.get_mut(&id).unwrap();
                    o.holder = Holder::Pulled { hand: i, t0: now, dur, gravity: g };
                    o.handled = true;
                    // (A game's own prop followed the game until now: the runtime's while it flies.)
                    self.physics.set_kinematic(body, false);
                    self.physics.set_gravity(body, g / GRAVITY);
                    self.physics.set_angular_damping(body, 8.0);
                    let launch = (palm - p) / dur + Vec3::Y * (0.5 * g * dur);
                    let (_, w) = self.physics.velocity(body);
                    self.physics.set_velocity(body, launch, w);
                    self.set_glove(i, Glove::Flight(id));
                    self.debug.pulls += 1;
                    self.emit(Cue::GloveFlick, Some(i), Some(id), origin, 1.0);
                }
            }
            Glove::Flight(id) => {
                let Some(o) = self.objects.get(&id) else {
                    self.set_glove(i, Glove::Idle);
                    return;
                };
                if !matches!(o.holder, Holder::Pulled { .. }) {
                    self.set_glove(i, Glove::Idle);
                }
            }
        }
    }

    fn set_glove(&mut self, i: usize, g: Glove) {
        let h = &mut self.hands.as_mut().unwrap()[i];
        if !matches!(g, Glove::Locked(_)) {
            h.glove_released = None;
            h.glove_hop = 0.0;
        }
        h.glove = g;
    }

    // ---- driving the bodies --------------------------------------------------------------------------
    fn drive(&mut self, inp: &RigInput, dt: f32, zones: &[Zone; SLOT_COUNT_BODY]) {
        let s = inp.settings;
        // Where the player's eyes are: a hand (or what it holds) left behind snaps back to its
        // controller only when the head could reach it there.
        let head = inp.map.point(v3(inp.frame.hmd.pos));
        let ids: Vec<u32> = self.objects.keys().copied().collect();
        for id in &ids {
            let holder = self.objects[id].holder;
            match holder {
                Holder::Held(hand) => {
                    let target = match self.objects[id].attach {
                        Attach::Gun => self.gun_target(*id, hand, s, dt),
                        Attach::Fixed(p, q) => compose(self.hands.as_ref().unwrap()[hand].target, (p, q)),
                    };
                    let o = &self.objects[id];
                    let target = match o.blend {
                        Some((bp, bq, t0, d)) if self.now - t0 < travel_time(s.interaction.grab_transition, d) as f64 => {
                            let k = smooth((self.now - t0) / travel_time(s.interaction.grab_transition, d) as f64);
                            (bp.lerp(target.0, k), bq.slerp(target.1, k))
                        }
                        _ => target,
                    };
                    // Weight: heavy things accelerate less (Godot's 15 -> 300 N idea, scaled by the
                    // weight setting).
                    let two = self.hands.as_ref().unwrap()[hand ^ 1].support == Some(*id);
                    let mass = o.desc.mass.max(0.05);
                    // How hard you can move it: a 3.5 kg rifle speeds up at about 30 m/s^2 and spins
                    // at about 60 rad/s^2 in one hand (a quarter turn takes a third of a second),
                    // twice and three times that in two; a pistol nearly and a bottle fully keeps up.
                    // However heavy (a game's prop can weigh 80 kg), it still follows the hand, a
                    // little behind: a hand can't feel a weight it can't lift.
                    let heft = (1.0 + 2.0 * s.interaction.weight * mass).powi(2);
                    let max_accel = (600.0 * if two { 2.2 } else { 1.0 } / heft).max(MIN_HELD_ACCEL);
                    let max_ang_accel = (1200.0 * if two { 3.0 } else { 1.0 } / heft).max(MIN_HELD_ANG_ACCEL);
                    // Where the hand holds it: the point driven onto the hand, the one it turns about.
                    let pivot = match o.attach {
                        Attach::Gun => v3(o.desc.grip),
                        Attach::Fixed(p, q) => inverse((p, q)).0,
                    };
                    let (now_pos, now_rot) = self.physics.pose(o.body);
                    // Held back by the world too far: through it until it's back in the hand.
                    let behind = (now_pos + now_rot * pivot).distance(target.0 + target.1 * pivot);
                    let collider = o.collider;
                    if !o.through && behind > SLIP {
                        self.physics.set_groups(collider, physics::groups_held_through(hand));
                        self.objects.get_mut(id).unwrap().through = true;
                    } else if o.through && behind < SLIP_BACK && !self.physics.in_world(collider) {
                        self.physics.set_groups(collider, groups_held(hand));
                        self.objects.get_mut(id).unwrap().through = false;
                    }
                    let o = &self.objects[id];
                    let (mut carry, mut carried) = (o.carry, o.carried);
                    if now_pos.distance(target.0) > LEASH && self.physics.reachable(head, target.0) {
                        // Stuck behind something while the hand moved on: back into the hand.
                        self.physics.set_pose(o.body, target.0, target.1);
                        self.physics.set_velocity(o.body, Vec3::ZERO, Vec3::ZERO);
                        (carry, carried) = (None, (Vec3::ZERO, Vec3::ZERO));
                    }
                    // Pressed into the world it stays as stiff: the world stops it where it is (a
                    // softer hold only lags it behind and lets it jump on once it's clear).
                    let drive = physics::Drive { pivot, carry, carried, gain: 1.0, max_accel, ang_gain: 1.0, max_ang_accel };
                    let body = o.body;
                    let carried = self.physics.track(body, target.0, target.1, dt, &drive);
                    self.objects.get_mut(id).unwrap().carried = carried;
                    self.physics.set_gravity(body, 0.0);
                }
                Holder::Holstered(slot) => {
                    let (pt, rt) = self.anchor.slot_pose(slot, &zones[slot as usize]);
                    let grip = v3(self.objects[id].desc.grip);
                    let (p, q) = (self.map.point(pt), self.map.rot(rt));
                    let o = &self.objects[id];
                    self.physics.move_kinematic(o.body, p - q * grip, q);
                }
                Holder::Returning { slot, t0, from } => {
                    let (pt, rt) = self.anchor.slot_pose(slot, &zones[slot as usize]);
                    let grip = v3(self.objects[id].desc.grip);
                    let q = self.map.rot(rt);
                    let p = self.map.point(pt) - q * grip;
                    let k = smooth((self.now - t0) / RETURN_TIME);
                    let o = self.objects.get_mut(id).unwrap();
                    let body = o.body;
                    if k >= 1.0 {
                        o.holder = Holder::Holstered(slot);
                    }
                    self.physics.move_kinematic(body, from.0.lerp(p, k), from.1.slerp(q, k));
                }
                Holder::InWeapon(wid) => {
                    let Some((p, q)) = self.seated_pose(*id, wid) else { continue };
                    self.physics.move_kinematic(self.objects[id].body, p, q);
                }
                Holder::Pulled { hand, t0, dur, gravity } => {
                    let palm = self.hands.as_ref().unwrap()[hand].target.0;
                    let o = &self.objects[id];
                    let (p, _) = self.physics.pose(o.body);
                    let elapsed = (self.now - t0) as f32;
                    let total = dur;
                    let grip_held = self.hands.as_ref().unwrap()[hand].grip_down;
                    if p.distance(palm) < s.interaction.glove_catch_radius && grip_held && elapsed > 0.05 {
                        self.debug.catches += 1;
                        self.take(*id, hand, true);
                        self.emit(Cue::GloveCatch, Some(hand), Some(*id), palm, 1.0);
                        self.set_glove(hand, Glove::Idle);
                        continue;
                    }
                    // Alyx: it steers after the moving hand on the way (not right at the end).
                    let tau = total - elapsed;
                    if tau > alyx::HOMING_STOP && tau <= alyx::HOMING_START {
                        let (v, w) = self.physics.velocity(o.body);
                        let need = (palm - p - v * tau) * (2.0 / (tau * tau));
                        let extra = (need + Vec3::Y * gravity).clamp_length_max(alyx::HOMING_MAX);
                        self.physics.set_velocity(o.body, v + extra * dt, w);
                    }
                    if elapsed > total + 0.6 {
                        self.free_body(*id, groups_free());
                        self.set_glove(hand, Glove::Idle);
                    }
                }
                Holder::Free => {
                    let o = &self.objects[id];
                    if o.hands_off_until > 0.0 && self.now > o.hands_off_until {
                        self.physics.set_groups(o.collider, groups_free());
                        self.objects.get_mut(id).unwrap().hands_off_until = 0.0;
                    }
                    if self.objects[id].throw.as_ref().is_some_and(|t| t.flight.is_some()) {
                        self.fly(*id, dt, s.interaction.knife_assist);
                    }
                    // The game's own prop: where the game has it, once it's had it back.
                    let o = &self.objects[id];
                    if o.desc.flags & OBJF_GAME_SIMULATED != 0
                        && self.now - o.free_since > GAME_HANDBACK as f64
                        && let Some(&(p, q)) = self.game_poses.get(id)
                    {
                        let (body, collider) = (o.body, o.collider);
                        self.physics.set_kinematic(body, true);
                        self.physics.set_groups(collider, groups_game_prop());
                        self.physics.move_kinematic(body, p, q);
                    }
                }
                // A knife in a loose object rides with it; if that's gone, it falls.
                Holder::Stuck { parent, rel } if parent != 0 => match self.obj_pose(parent) {
                    Some(pp) => {
                        let (p, q) = compose(pp, rel);
                        self.physics.move_kinematic(self.objects[id].body, p, q);
                    }
                    None => self.free_body(*id, groups_free()),
                },
                Holder::Stuck { .. } | Holder::Gone => {}
            }
        }
        self.valves(dt, s.interaction.grab_transition, s.interaction.mechanism_stiffness);
        // Empty hands follow their controllers; past the leash they snap back, unless the
        // controller is through the world (a wall, the floor): then the hand stays against it.
        let now = self.now;
        let hands = self.hands.as_mut().unwrap();
        for (i, h) in hands.iter_mut().enumerate() {
            if h.holding.is_some() || h.support.is_some() || h.barrels.is_some() || h.valve.is_some() {
                continue;
            }
            // Gliding back to the controller after letting go of a valve.
            if let Some((p0, q0, t0, time)) = h.returning {
                let k = if time > 1e-3 { smooth((now - t0) / time as f64) } else { 1.0 };
                if k < 1.0 {
                    self.physics.set_pose(h.body, p0.lerp(h.target.0, k), q0.slerp(h.target.1, k).normalize());
                    self.physics.set_velocity(h.body, Vec3::ZERO, Vec3::ZERO);
                    continue;
                }
                h.returning = None;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
            let (pos, _) = self.physics.pose(h.body);
            if pos.distance(h.target.0) > LEASH && self.physics.reachable(head, h.target.0) {
                self.physics.set_pose(h.body, h.target.0, h.target.1);
                self.physics.set_velocity(h.body, Vec3::ZERO, Vec3::ZERO);
                h.leashed_until = now + 0.1;
                self.physics.set_groups(h.collider, groups_none());
            } else if h.leashed_until > 0.0 && now > h.leashed_until {
                h.leashed_until = 0.0;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
            if s.interaction.physics_hands {
                self.physics.track(h.body, h.target.0, h.target.1, dt, &physics::Drive::FREE);
            } else {
                self.physics.set_pose(h.body, h.target.0, h.target.1);
                self.physics.set_groups(h.collider, groups_none());
            }
        }
    }

    /// Where magazine (or round) `id` seated in weapon `wid` is, the weapon where it is now: on its
    /// mag well (in a swung-out cylinder, swung out with it).
    fn seated_pose(&self, id: u32, wid: u32) -> Option<(Vec3, Quat)> {
        let (wp, wr) = self.obj_pose(wid)?;
        let (w, o) = (self.objects.get(&wid)?, self.objects.get(&id)?);
        let angle = self.hinge_angle(wid);
        let turn = actions::hinge_rot(&w.desc, angle);
        let h = v3(w.desc.handle);
        let offset = v3(w.desc.mag_well) - v3(o.desc.mag_well);
        let local = if angle > 0.0 { h + turn * (offset - h) } else { offset };
        let rot = if angle > 0.0 { (wr * turn).normalize() } else { wr };
        Some((wp + wr * local, rot))
    }

    fn after_step(&mut self, inp: &RigInput) {
        // What's seated in a gun goes where the gun is after the step: put there before it, it
        // trails the gun by however far the step moved it (a walk's few centimetres: a magazine
        // shaking against its gun on the move).
        let seated: Vec<(u32, u32)> = self.objects.iter().filter_map(|(id, o)| if let Holder::InWeapon(w) = o.holder { Some((*id, w)) } else { None }).collect();
        for (id, wid) in seated {
            if let Some((p, q)) = self.seated_pose(id, wid) {
                self.physics.set_pose(self.objects[&id].body, p, q);
            }
        }
        let ids: Vec<u32> = self.objects.keys().copied().collect();
        let mut impacts = Vec::new();
        for id in ids {
            let o = &self.objects[&id];
            let contact = matches!(o.holder, Holder::Held(_)) && self.physics.in_contact(o.collider);
            let (vel, _) = self.physics.velocity(o.body);
            let (pos, _) = self.physics.pose(o.body);
            // Something hit something: a sudden change of velocity (a free object landing or
            // bouncing), or a held one starting to touch the world.
            let change = (vel - o.prev_vel).length();
            // (Not one of the game's own props the runtime only follows: the game moves it, at the
            // game's pace, and sounds its own knocks.)
            let following = o.desc.flags & OBJF_GAME_SIMULATED != 0 && o.holder == Holder::Free && self.now - o.free_since > GAME_HANDBACK as f64;
            let hit = match o.holder {
                Holder::Free | Holder::Pulled { .. } => !following && change > 1.2 && o.prev_vel.length() > 1.0,
                Holder::Held(_) => contact && !o.in_contact && o.prev_vel.length() > 0.4,
                _ => false,
            };
            if hit && self.now >= o.next_impact {
                let hand = if let Holder::Held(h) = o.holder { Some(h) } else { None };
                let strength = (change.max(o.prev_vel.length()) / 6.0).clamp(0.05, 1.0);
                impacts.push((id, hand, pos, strength * (o.desc.mass / 2.0).clamp(0.4, 1.0)));
            }
            let now = self.now;
            let free = matches!(o.holder, Holder::Free | Holder::Pulled { .. });
            if free && o.throw.as_ref().is_some_and(|t| t.impact_due(now, change)) {
                self.detonate(id);
                continue;
            }
            let o = self.objects.get_mut(&id).unwrap();
            if hit {
                o.next_impact = now + 0.12;
            }
            // A throw is over once it comes to rest.
            if let Some(t) = &mut o.throw
                && t.thrown_at.is_some_and(|t0| now - t0 > 0.3)
                && vel.length() < 0.25
            {
                t.thrown_at = None;
            }
            o.prev_vel = vel;
            o.in_contact = contact;
            // Loose runtime-spawned magazines tidy themselves away.
            if o.spawned && o.holder == Holder::Free && self.now - o.free_since > 10.0 {
                self.remove(id);
            }
        }
        for (id, hand, pos, strength) in impacts {
            self.emit(Cue::Impact, hand, Some(id), pos, strength);
        }
        // A support hand locks onto the gun's foregrip line, where along it the controller is; it
        // lets go if the controller wanders off.
        for i in 0..2 {
            let Some(wid) = self.hands.as_ref().unwrap()[i].support else { continue };
            let Some(o) = self.objects.get(&wid) else { continue };
            let (wp, wr) = self.physics.pose(o.body);
            let target = self.hands.as_ref().unwrap()[i].target.0;
            let p = foregrip_point(&o.desc, wr.inverse() * (target - wp));
            let on_gun = wp + wr * p;
            if on_gun.distance(target) > FOREGRIP_BREAK {
                self.hands.as_mut().unwrap()[i].support = None;
                self.physics.set_groups(self.hands.as_ref().unwrap()[i].collider, groups_hand(i));
                self.emit(Cue::Release, Some(i), Some(wid), target, 0.6);
                continue;
            }
            let rot = (wr * self.support_rot(&o.desc, i)).normalize();
            let h = &self.hands.as_ref().unwrap()[i];
            let (body, collider) = (h.body, h.collider);
            self.physics.set_groups(collider, groups_none());
            self.physics.set_pose(body, on_gun, rot);
            self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        }
        // A hand holding open barrels sits where it took them, turning with them; pulled 30 cm away
        // from them it lets go.
        for i in 0..2 {
            let Some(b) = self.hands.as_ref().unwrap()[i].barrels else { continue };
            let Some((wp, wr)) = self.obj_pose(b.gun) else { continue };
            let on = wp + wr * self.barrel_point(&b);
            let h = &self.hands.as_ref().unwrap()[i];
            let (body, collider, target) = (h.body, h.collider, h.target);
            if on.distance(target.0) > FOREGRIP_BREAK {
                self.hands.as_mut().unwrap()[i].barrels = None;
                self.physics.set_groups(collider, groups_hand(i));
                self.emit(Cue::Release, Some(i), Some(b.gun), target.0, 0.6);
                continue;
            }
            self.physics.set_groups(collider, groups_none());
            self.physics.set_pose(body, on, target.1);
            self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        }
        // A holding hand sits on its object: where the object is, minus the hold offset.
        let hands = self.hands.as_mut().unwrap();
        for (i, h) in hands.iter_mut().enumerate() {
            if let Some(id) = h.holding {
                if let Some(o) = self.objects.get(&id) {
                    let op = self.physics.pose(o.body);
                    let hold = match o.attach {
                        Attach::Gun => {
                            let grip = v3(o.desc.grip);
                            match authored(o.desc.grip_rot) {
                                // Rigged: the hand is the gun's grip frame.
                                Some(g) => (op.0 + op.1 * grip, (op.1 * if i == 0 { mirror_grip(g) } else { g }).normalize()),
                                // The palm is at the grip, oriented like the controller relative to the aim.
                                None => {
                                    let rel = h.aim_rot.inverse() * h.target.1;
                                    (op.0 + op.1 * grip, (op.1 * rel).normalize())
                                }
                            }
                        }
                        Attach::Fixed(p, q) => compose(op, inverse((p, q))),
                    };
                    self.physics.set_pose(h.body, hold.0, hold.1);
                    self.physics.set_velocity(h.body, Vec3::ZERO, Vec3::ZERO);
                    h.blocked = hold.0.distance(h.target.0) > BLOCKED * 2.0;
                }
            } else if h.support.is_some() || h.barrels.is_some() {
                h.blocked = false;
            } else {
                let (pos, _) = self.physics.pose(h.body);
                h.blocked = inp.settings.interaction.physics_hands && pos.distance(h.target.0) > BLOCKED;
            }
            if h.blocked {
                self.debug.blocked_frames += 1;
            }
        }
    }

    // ---- what the adapter and the renderer get ----------------------------------------------------------
    fn output(&mut self, inp: &RigInput, dom: usize) -> RigOutput {
        let mut out = RigOutput::default();
        let map = self.map;
        let hands = self.hands.as_ref().unwrap();
        for (i, h) in hands.iter().enumerate() {
            let (p, q) = self.physics.pose(h.body);
            let (pt, qt) = (map.inv_point(p), map.inv_rot(q));
            out.hands[i] = HandState {
                pose: pose(pt, qt),
                flags: HAND_TRACKED
                    | if h.blocked { HAND_BLOCKED } else { 0 }
                    | if h.holding.is_some() || h.support.is_some() || h.barrels.is_some() { HAND_HOLDING } else { 0 },
                holding: h.holding.or(h.support).or(h.barrels.map(|b| b.gun)).unwrap_or(0),
                _pad: [0; 2],
            };
            if h.blocked && inp.settings.interaction.ghost_hands {
                out.ghosts[i] = Some(pose(h.grip_t.0, h.grip_t.1));
            }
            out.hand_busy[i] = h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.pin.is_some() || h.barrels.is_some();
            out.holds[i] = match h.holding.and_then(|id| self.objects.get(&id)) {
                Some(o) if o.desc.kind == OBJ_WEAPON => rustvr_ik::HoldShape::PistolGrip,
                Some(o) if is_round(&o.desc) => rustvr_ik::HoldShape::Small,
                Some(o) if o.desc.kind == OBJ_MAGAZINE => rustvr_ik::HoldShape::Magazine,
                // A knife's handle in the fist.
                Some(o) if o.throw.as_ref().is_some_and(|t| t.is_knife()) => rustvr_ik::HoldShape::Foregrip,
                Some(o) if o.desc.half_extents.iter().fold(0.0f32, |a, b| a.max(*b)) < 0.07 => rustvr_ik::HoldShape::Small,
                Some(_) => rustvr_ik::HoldShape::Large,
                None if h.support.is_some() || h.handle.is_some() || h.barrels.is_some() => rustvr_ik::HoldShape::Foregrip,
                None if h.pin.is_some() => rustvr_ik::HoldShape::Small,
                None => rustvr_ik::HoldShape::None,
            };
            if let Glove::Flight(id) = h.glove {
                out.pulled[i] = self.obj_pose(id).map(|(p, _)| map.inv_point(p));
            }
            out.hand_targeting[i] = h.hover != Hover::None || matches!(h.glove, Glove::Hover(_) | Glove::Locked(_) | Glove::Flight(_));
        }
        // Primary gun: the firing hand's, else any held gun.
        let gun_in = |hand: usize| hands[hand].holding.filter(|id| self.objects.get(id).is_some_and(|o| o.desc.kind == OBJ_WEAPON));
        if let Some(id) = gun_in(dom).or_else(|| gun_in(dom ^ 1)) {
            let o = &self.objects[&id];
            let (p, q) = self.physics.pose(o.body);
            let m = glam::Mat4::from_rotation_translation(map.inv_rot(q), map.inv_point(p));
            let held_by = if hands[dom].holding == Some(id) { dom } else { dom ^ 1 };
            out.primary_weapon = Some(RigWeapon {
                id,
                weapon_to_tracking: m,
                sight: v3(o.desc.sight),
                muzzle: v3(o.desc.muzzle),
                two_handed: hands[held_by ^ 1].support == Some(id),
                scoped: o.desc.flags & OBJF_SCOPED != 0,
                laser: (o.desc.weapon_flags & WPN_LASER != 0).then(|| v3(o.desc.laser)),
            });
        }
        let _ = inp;
        out
    }

    /// Fills the adapter's object state table.
    pub fn write_states(&self, out: &mut ObjectStates) {
        let map = self.map;
        out.count = 0;
        out.frame_index = self.frame_index;
        let hands = self.hands.as_ref();
        let hovered: Vec<u32> = hands
            .map(|h| {
                h.iter()
                    .filter_map(|h| match (h.hover, h.glove) {
                        (Hover::Object(id) | Hover::Valve(id), _) => Some(id),
                        (_, Glove::Hover(id) | Glove::Locked(id)) => Some(id),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (id, o) in self.objects.iter().take(MAX_OBJECTS) {
            let (p, q) = self.physics.pose(o.body);
            let (lv, av) = self.physics.velocity(o.body);
            // A valve turns on its axle at its own speed (its body is only moved there).
            let (lv, av) = o.valve.map_or((lv, av), |v| (Vec3::ZERO, v.axis() * v.vel));
            let tracking = matches!(o.holder, Holder::Held(_) | Holder::Holstered(_) | Holder::Returning { .. });
            let tracking = tracking || matches!(o.holder, Holder::InWeapon(w) if self.objects.get(&w).is_some_and(|w| matches!(w.holder, Holder::Held(_) | Holder::Holstered(_) | Holder::Returning { .. })));
            let pose_out = if tracking { pose(map.inv_point(p), map.inv_rot(q)) } else { pose(p, q) };
            let mut flags = OS_SIMULATED | if tracking { OS_TRACKING } else { 0 };
            match o.holder {
                Holder::Held(0) => flags |= OS_HELD_LEFT,
                Holder::Held(_) => flags |= OS_HELD_RIGHT,
                Holder::Holstered(_) | Holder::Returning { .. } => flags |= OS_HOLSTERED,
                Holder::InWeapon(_) => flags |= OS_IN_WEAPON,
                Holder::Pulled { .. } => flags |= OS_PULLED,
                Holder::Stuck { .. } => flags |= OS_STUCK,
                Holder::Free | Holder::Gone => {}
            }
            if let Some(t) = &o.throw {
                flags |= t.flags_out();
            }
            if hovered.contains(id) {
                flags |= OS_HOVERED;
            }
            // A game's prop a hand has had: drawn by the runtime from then on (the game hides its).
            if o.desc.kind == OBJ_PROP && o.desc.flags & OBJF_RUNTIME_DRAWS != 0 && (o.handled || matches!(o.holder, Holder::Held(_) | Holder::Pulled { .. })) {
                flags |= OS_RUNTIME_DRAWN;
            }
            // The hands on a valve's rim have it.
            for (i, h) in hands.into_iter().flatten().enumerate() {
                if h.valve.is_some_and(|g| g.id == *id) {
                    flags |= if i == 0 { OS_HELD_LEFT } else { OS_HELD_RIGHT };
                }
            }
            if o.spawned {
                flags |= OS_SPAWNED;
            }
            let (rounds, shots, chambered, slide) = match &o.weapon {
                Some(w) => {
                    let rounds = w.internal.or_else(|| w.mag.and_then(|m| self.objects.get(&m)).map(|m| m.rounds)).unwrap_or(0);
                    if w.slide_locked {
                        flags |= OS_SLIDE_LOCKED;
                    }
                    if !w.action_closed() {
                        flags |= OS_ACTION_OPEN;
                    }
                    (rounds, w.shots, w.chamber_bits(), w.slide)
                }
                // A valve: how far it's turned (radians about its axis).
                None => (o.rounds, o.throw.as_ref().map(|t| t.detonations).unwrap_or(0), 0, o.valve.map_or(0.0, |v| v.angle)),
            };
            // An air gun's pressure, a bolt's lift; a grenade's fuse.
            let fuse = match &o.weapon {
                Some(w) => w.air.map(|a| a.pressure).or(w.cycle.map(|c| c.lift)).unwrap_or(0.0),
                None => o.throw.as_ref().map(|t| t.fuse_left(self.now)).unwrap_or(0.0),
            };
            let slot = match o.holder {
                Holder::Holstered(s) | Holder::Returning { slot: s, .. } => s,
                _ => 0,
            };
            let parent = match o.holder {
                Holder::InWeapon(w) => w,
                Holder::Stuck { parent, .. } => parent,
                _ => 0,
            };
            out.states[out.count as usize] = ObjectState {
                id: *id,
                flags,
                slot,
                parent,
                pose: pose_out,
                lin_vel: if tracking { (map.inv_rot(Quat::IDENTITY) * lv / map.world_per_meter).to_array() } else { lv.to_array() },
                rounds,
                ang_vel: if tracking { (map.inv_rot(Quat::IDENTITY) * av).to_array() } else { av.to_array() },
                shots,
                chambered,
                slide,
                desc: o.draw_desc,
                fuse,
            };
            out.count += 1;
        }
    }

    /// Sight line of a weapon, for tests and the debug overlay.
    pub fn weapon_rounds(&self, id: u32) -> Option<(u32, bool, bool)> {
        let o = self.objects.get(&id)?;
        let w = o.weapon.as_ref()?;
        let rounds = w.internal.or_else(|| w.mag.and_then(|m| self.objects.get(&m)).map(|m| m.rounds))?;
        Some((rounds, w.chambered, w.slide_locked))
    }

    /// A gun's own state (its chambers, action and pressure), for tests.
    pub fn weapon_state(&self, id: u32) -> Option<&WeaponState> {
        self.objects.get(&id)?.weapon.as_ref()
    }

    pub fn object_world_pose(&self, id: u32) -> Option<(Vec3, Quat)> {
        self.obj_pose(id)
    }

    pub fn held(&self, hand: usize) -> Option<u32> {
        self.hands.as_ref()?[hand].holding
    }

    pub fn holstered(&self, slot: u32) -> Option<u32> {
        self.slots[slot as usize]
    }

    pub fn mag_of(&self, weapon: u32) -> Option<u32> {
        self.objects.get(&weapon)?.weapon.as_ref()?.mag
    }

    pub fn is_free(&self, id: u32) -> bool {
        self.objects.get(&id).is_some_and(|o| o.holder == Holder::Free)
    }

    pub fn anchor(&self) -> &BodyAnchor {
        &self.anchor
    }
}

/// An object as a scripted test sees it: where it is (tracking space) and its key points (object space).
#[derive(Clone, Copy, Debug)]
pub struct ObjSnap {
    pub id: u32,
    pub desc: u32,
    pub kind: u32,
    /// `OS_*`
    pub flags: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub grip: Vec3,
    pub sight: Vec3,
    pub foregrip: Vec3,
    pub mag_well: Vec3,
    pub handle: Vec3,
    pub half_extents: Vec3,
    pub center: Vec3,
    pub rounds: u32,
    /// The rig: the controller grip rotation on the gun (None: aim-ray guns), and the support hand's.
    pub grip_rot: Option<Quat>,
    pub support_rot: Option<Quat>,
    /// How many recoil kicks its game gave it (0: the runtime's own recoil).
    pub kicks: usize,
    /// Throwables: `THROW_*`, the tip (knives, object space) and the seconds left on its fuse (an
    /// air gun's pressure, a bolt's lift).
    pub throw_flags: u32,
    pub muzzle: Vec3,
    pub fuse: f32,
    /// Guns: how they're worked (`ACTION_*`, `WPN_*`, the action's angle, the chambers' spacing and
    /// count), and what the action and the chambers are doing (`ObjectState::slide`, `chambered`).
    pub action: u32,
    pub weapon_flags: u32,
    pub action_angle: f32,
    pub chamber_step: f32,
    pub capacity: u32,
    pub handle_travel: f32,
    pub slide: f32,
    pub chambered: u32,
}

impl ObjSnap {
    /// An object-space point in tracking space.
    pub fn at(&self, p: Vec3) -> Vec3 {
        self.pos + self.rot * p
    }
}

impl Rig {
    /// Every object in tracking space, from the states just written.
    pub fn snapshot(&self, states: &ObjectStates) -> Vec<ObjSnap> {
        let map = self.map;
        states.states[..(states.count as usize).min(MAX_OBJECTS)]
            .iter()
            .filter_map(|s| {
                let o = self.objects.get(&s.id)?;
                let (p, q) = self.physics.pose(o.body);
                let d = &o.desc;
                Some(ObjSnap {
                    id: s.id,
                    desc: s.desc,
                    kind: d.kind,
                    flags: s.flags,
                    pos: map.inv_point(p),
                    rot: map.inv_rot(q),
                    grip_rot: authored(d.grip_rot),
                    support_rot: authored(d.support_rot),
                    grip: v3(d.grip),
                    sight: v3(d.sight),
                    foregrip: v3(d.foregrip),
                    mag_well: v3(d.mag_well),
                    handle: v3(d.handle),
                    half_extents: v3(d.half_extents),
                    center: v3(d.center),
                    rounds: s.rounds,
                    kicks: self.kicks(s.id).len(),
                    throw_flags: d.throw_flags,
                    muzzle: v3(d.muzzle),
                    fuse: s.fuse,
                    action: d.action,
                    weapon_flags: d.weapon_flags,
                    action_angle: d.action_angle,
                    chamber_step: d.chamber_step,
                    capacity: d.capacity,
                    handle_travel: d.handle_travel,
                    slide: s.slide,
                    chambered: s.chambered,
                })
            })
            .collect()
    }
}

/// The rotation whose +Y is `y` and whose +Z is as close to `z` as possible.
fn look_rotation_y(y: Vec3, z: Vec3) -> Quat {
    let y = y.normalize_or(Vec3::Y);
    let x = y.cross(z).normalize_or(Vec3::X);
    let z = x.cross(y);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

fn settings_stock(s: &Settings, d: &ObjectDesc) -> bool {
    s.interaction.virtual_stock && d.weapon_flags & WPN_STOCK != 0
}

/// Rotation whose +Z is `fwd` with +Y as close to `up` as possible.
pub fn look_rotation(fwd: Vec3, up: Vec3) -> Quat {
    let z = fwd.normalize_or(Vec3::Z);
    let x = up.cross(z).normalize_or(Vec3::X);
    let y = z.cross(x);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

#[cfg(test)]
mod tests;

/// A small cube (half size `h`) at `at`, turned by `q`, as outward triangles.
fn marker_cube(at: Vec3, q: Quat, h: f32, color: [f32; 3], out: &mut Vec<([Vec3; 3], [f32; 3])>) {
    let c = |x: f32, y: f32, z: f32| at + q * (Vec3::new(x, y, z) * h);
    // Each face: its normal axis and sign; corners wound outwards.
    for (axis, s) in [(0, 1.0f32), (0, -1.0), (1, 1.0), (1, -1.0), (2, 1.0), (2, -1.0)] {
        let p = |u: f32, v: f32| {
            let mut a = [0.0f32; 3];
            a[axis] = s;
            a[(axis + 1) % 3] = u;
            a[(axis + 2) % 3] = v;
            c(a[0], a[1], a[2])
        };
        let (a, b, cc, d) = (p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0));
        if s > 0.0 {
            out.push(([a, b, cc], color));
            out.push(([a, cc, d], color));
        } else {
            out.push(([a, cc, b], color));
            out.push(([a, d, cc], color));
        }
    }
}
