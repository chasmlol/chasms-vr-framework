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

pub mod feedback;
pub mod filters;
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
use feedback::{Cue, Feedback};
use filters::OneEuro;
use physics::{Physics, groups_free, groups_hand, groups_held, groups_none};
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
    blend: Option<(Vec3, Quat, f64)>,
    spawn_seq: u32,
    spawned: bool,
    free_since: f64,
    home_slot: Option<u32>,
    rounds: u32,
    weapon: Option<WeaponState>,
    hovered: bool,
    /// Ignore the hands until then (just thrown).
    hands_off_until: f64,
    /// Touching the world or another object (held objects soften against it).
    in_contact: bool,
    /// Velocity after the last step, for impact sounds.
    prev_vel: Vec3,
    /// A magazine just taken out of a gun can't seat again until it has left the well.
    seat_armed: bool,
    next_impact: f64,
    /// Grenades, knives and other throwables.
    throw: Option<ThrowState>,
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
const LEASH: f32 = 0.7;
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
/// A gun fired with its sights this close to the eye is aimed (the game's aimed recoil).
const AIMED_SIGHT: f32 = 0.15;
const SNAP_BLEND: f64 = 0.12;
const RETURN_TIME: f64 = 0.3;

pub struct Rig {
    pub physics: Physics,
    objects: BTreeMap<u32, Obj>,
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
    /// `vr_hand_pull_detach_cone` 0.5: a locked target lets go when you point 60 degrees away ...
    pub const DETACH_COS: f32 = 0.5;
    /// ... or `vr_gg_lock_release_delay` 0.25 s after the grip lets go.
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
        let (body, collider) = self.physics.add_object(&d, pos, rot);
        let weapon = (d.kind == OBJ_WEAPON).then(|| {
            let mut w = WeaponState::new(true);
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
                in_contact: false,
                prev_vel: Vec3::ZERO,
                next_impact: 0.0,
                seat_armed: true,
                throw: ThrowState::new(&d),
            },
        );
        // A gun arrives loaded: a seated magazine from its template, or rounds held internally.
        if d.kind == OBJ_WEAPON {
            match self.template_for(d.ammo_type) {
                Some(t) => {
                    let mag = self.spawn_mag(&t, pos, rot);
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

    fn spawn_mag(&mut self, t: &ObjectDesc, pos: Vec3, rot: Quat) -> u32 {
        let id = self.next_spawn;
        self.next_spawn = self.next_spawn.wrapping_add(1).max(SPAWN_BASE);
        let mut d = *t;
        d.id = id;
        d.flags &= !OBJF_TEMPLATE;
        d.rounds = t.capacity;
        self.add(d, t.id, true, pos, rot);
        id
    }

    // ---- the frame ------------------------------------------------------------------------------
    pub fn update(&mut self, inp: &RigInput) -> RigOutput {
        self.now = inp.now;
        self.map = inp.map;
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
        self.apply_pushes();
        self.physics.set_dt(dt);
        self.physics.step();
        self.after_step(inp);
        self.output(inp, dom)
    }

    fn ensure_hands(&mut self, f: &HmdFrame) {
        if self.hands.is_some() {
            return;
        }
        let mk = |p: &mut Physics, i: usize| {
            let g = &f.hands[i].grip;
            let (body, collider) = p.add_hand(i, v3(g.pos), q4(g.rot));
            Hand {
                body,
                collider,
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
            }
        };
        let l = mk(&mut self.physics, 0);
        let r = mk(&mut self.physics, 1);
        self.hands = Some([l, r]);
    }

    fn emit(&mut self, cue: Cue, hand: Option<usize>, pos_world: Vec3, intensity: f32) {
        let pos = self.map.inv_point(pos_world);
        self.feedback.push(Feedback { cue, hand, pos, intensity });
    }

    fn obj_pose(&self, id: u32) -> Option<(Vec3, Quat)> {
        self.objects.get(&id).map(|o| self.physics.pose(o.body))
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
        if h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.pin.is_some() {
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
            let handle = wp + wr * v3(o.desc.handle);
            let fore = wp + wr * foregrip_point(&o.desc, wr.inverse() * (palm - wp));
            let needs_rack = o.weapon.as_ref().is_some_and(|w| !w.chambered || w.slide_locked);
            if o.desc.handle_travel > 0.0 && palm.distance(handle) < HANDLE_REACH * if needs_rack { 1.3 } else { 1.0 } {
                hover = Hover::Handle(wid);
            } else if o.desc.flags & OBJF_TWO_HANDED != 0 && palm.distance(fore) < FOREGRIP_REACH {
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
                let wants = self.objects[&wid].weapon.as_ref().is_some_and(|w| w.internal.is_none());
                let in_pouch = ammo.iter().any(|z| self.anchor.contains(z, palm_t))
                    || zones.iter().enumerate().any(|(slot, z)| self.slots[slot].is_none() && self.anchor.contains(z, palm_t));
                if wants && in_pouch {
                    hover = Hover::Ammo(wid);
                }
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
            self.emit(Cue::ZoneTick, Some(i), palm, 1.0);
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
                if tapped(CTRL_A) {
                    self.eject(id, i);
                }
                if tapped(CTRL_B) {
                    self.slide_release(id, i);
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
                        self.emit(Cue::GloveLock, Some(i), palm, 1.0);
                    }
                    // Gripping again within the lock's grace: still locked.
                    Glove::Locked(_) => self.hands.as_mut().unwrap()[i].glove_released = None,
                    _ => {}
                }
            }
            Hover::Object(id) => self.take(id, i, false),
            Hover::Slot(slot) => {
                if let Some(id) = self.slots[slot as usize].take() {
                    self.take(id, i, true);
                    self.debug.drawn += 1;
                    self.emit(Cue::HolsterOut, Some(i), palm, 1.0);
                }
            }
            Hover::Ammo(wid) => {
                let ammo_type = self.objects[&wid].desc.ammo_type;
                match self.template_for(ammo_type) {
                    Some(t) => {
                        let id = self.spawn_mag(&t, palm, grip_rot);
                        self.debug.mags_spawned += 1;
                        self.take(id, i, true);
                        self.emit(Cue::ZoneTick, Some(i), palm, 1.0);
                    }
                    None => self.emit(Cue::Fail, Some(i), palm, 1.0),
                }
            }
            Hover::Foregrip(wid) => {
                let h = &mut self.hands.as_mut().unwrap()[i];
                h.support = Some(wid);
                h.euro.reset();
                self.emit(Cue::Grab, Some(i), palm, 0.7);
            }
            Hover::Handle(wid) => {
                if let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) {
                    w.handle = Some(HandleGrab { hand: i, start: palm });
                }
                self.hands.as_mut().unwrap()[i].handle = Some(wid);
                self.emit(Cue::Grab, Some(i), palm, 0.6);
            }
            Hover::Pin(gid) => {
                self.hands.as_mut().unwrap()[i].pin = Some((gid, palm));
                self.emit(Cue::Grab, Some(i), palm, 0.4);
            }
        }
        let _ = inp;
    }

    /// Puts object `id` in hand `i`. `snap`: ease it into the hand's natural hold (guns always do).
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
        } else if snap && o.desc.kind == OBJ_MAGAZINE {
            // A fresh magazine sits in the fist along the grip pose's +Z (the fist's axis, little
            // finger to thumb), its top 6 cm out past the thumb, ready to push into a well.
            let rot = held_mag_rot();
            let top = v3(o.desc.mag_well);
            Attach::Fixed(HELD_MAG_TOP - rot * top, rot)
        } else {
            let rel = compose(inverse((palm, grip_rot)), (pos, rot));
            Attach::Fixed(rel.0, rel.1)
        };
        o.blend = Some((pos, rot, self.now));
        if let Holder::Holstered(slot) = o.holder {
            o.home_slot = Some(slot);
        }
        o.holder = Holder::Held(i);
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
        self.emit(Cue::Grab, Some(i), palm, 1.0);
    }

    fn release(&mut self, i: usize, inp: &RigInput, zones: &[Zone; SLOT_COUNT_BODY]) {
        let s = &inp.settings.interaction;
        let (palm, _) = self.hands.as_ref().unwrap()[i].target;
        let palm_t = self.hands.as_ref().unwrap()[i].grip_t.0;
        // Support grip / handle / pin / glove lock end with the grip.
        {
            let h = &mut self.hands.as_mut().unwrap()[i];
            if h.support.take().is_some() || h.pin.take().is_some() {
                return;
            }
            if let Glove::Locked(_) = h.glove {
                h.glove_released = Some(self.now);
            }
        }
        if let Some(wid) = self.hands.as_mut().unwrap()[i].handle.take() {
            if let Some(w) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()) {
                if w.let_go_handle() {
                    self.emit(Cue::SlideForward, Some(i), palm, 1.0);
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
                    self.emit(Cue::HolsterIn, Some(i), palm, 1.0);
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
                self.emit(Cue::Release, Some(i), palm, 0.6);
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
        self.emit(cue, Some(i), palm, if l.thrown { 1.0 } else { 0.7 });
        if lit {
            self.debug.arms += 1;
            self.emit(Cue::GrenadeArm, None, at, 0.8);
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
        let Some(mag) = self.objects.get_mut(&wid).and_then(|o| o.weapon.as_mut()).and_then(|w| w.mag.take()) else { return };
        let (wp, wr) = self.obj_pose(wid).unwrap();
        let (wv, _) = self.physics.velocity(self.objects[&wid].body);
        self.debug.ejects += 1;
        self.emit(Cue::MagEject, Some(hand), wp, 1.0);
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

    fn slide_release(&mut self, wid: u32, hand: usize) {
        let released = self.with_rounds(wid, |w, _, r| w.release_slide(r)).unwrap_or(false);
        if released {
            let p = self.obj_pose(wid).unwrap().0;
            self.emit(Cue::SlideForward, Some(hand), p, 1.0);
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
            self.emit(Cue::MagSeat, holder.map(|h| h ^ 1), p, 1.0);
            if let Some(h) = holder {
                self.emit(Cue::MagSeat, Some(h), p, 0.6);
            }
            if chambered {
                self.emit(Cue::SlideForward, holder, p, 0.8);
            }
        }
    }

    /// Weapon world pose: grip on the palm, barrel along the aim, steered by the support hand,
    /// kicked by recoil.
    fn gun_target(&mut self, id: u32, hand: usize, settings: &Settings, dt: f32) -> (Vec3, Quat) {
        let w = &settings.weapon;
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
        let support = if hands[hand ^ 1].support == Some(id) { Some(hand ^ 1) } else { None };
        if let Some(sh) = support {
            let raw = hands[sh].target.0;
            let steady = hands[sh].euro.filter(raw, dt);
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
                    self.emit(cue, Some(hand), muzzle, 1.0);
                    if two {
                        self.emit(cue, Some(hand ^ 1), muzzle, 0.5);
                    }
                    if fired == Fire::LastShot {
                        self.emit(Cue::LastRound, None, wp, 1.0);
                    }
                }
                Fire::Empty => {
                    self.debug.dry_fires += 1;
                    self.emit(Cue::EmptyClick, Some(hand), wp, 1.0);
                }
                Fire::None => {}
            }
            // Racking: the other hand on the charging handle pulls it back along the barrel.
            let grab = self.objects[&id].weapon.as_ref().unwrap().handle;
            if let Some(g) = grab {
                let hand_pos = self.hands.as_ref().unwrap()[g.hand].target.0;
                let pull_m = (g.start - hand_pos).dot(wr * Vec3::Z).max(0.0);
                let travel = d.handle_travel;
                let (racked, forward) = self.with_rounds(id, |w, _, r| w.pull_handle(pull_m, travel, r)).unwrap_or((false, false));
                if racked {
                    self.debug.racks += 1;
                    self.emit(Cue::SlideBack, Some(g.hand), wp, 1.0);
                }
                if forward {
                    self.emit(Cue::SlideForward, Some(g.hand), wp, 1.0);
                }
                // Pulling the hand away past the handle's travel lets go of it.
                if hand_pos.distance(g.start) > travel.max(0.06) + 0.25 {
                    self.hands.as_mut().unwrap()[g.hand].handle = None;
                    if let Some(w) = self.objects.get_mut(&id).and_then(|o| o.weapon.as_mut()) {
                        w.let_go_handle();
                    }
                }
            }
            // A held magazine near this gun's magazine well seats itself.
            if let Some(mag) = self.hands.as_ref().unwrap()[hand ^ 1].holding {
                self.try_seat(mag, id, inp.settings.interaction.easy_mag, hand ^ 1);
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
        let (mp, mr) = self.physics.pose(m.body);
        let (wp, wr) = self.physics.pose(w.body);
        let top = mp + mr * v3(m.desc.mag_well);
        let well = wp + wr * v3(w.desc.mag_well);
        if !m.seat_armed {
            if top.distance(well) > 0.12 {
                self.objects.get_mut(&mag).unwrap().seat_armed = true;
            }
            return;
        }
        let angle = (mr * Vec3::Y).angle_between(wr * Vec3::Y);
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
                self.emit(Cue::GrenadePin, Some(i), p, 0.7);
            }
            self.debug.arms += 1;
            self.emit(Cue::GrenadeArm, Some(i), p, 1.0);
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
            self.emit(Cue::GrenadePin, Some(i), palm, 1.0);
            self.emit(Cue::GrenadePin, Some(i ^ 1), p, 0.7);
            if lit {
                self.debug.arms += 1;
                self.emit(Cue::GrenadeArm, Some(i ^ 1), p, 1.0);
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
                self.emit(Cue::GrenadeTick, hand, p, k);
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
        self.emit(Cue::Explosion, None, p, 1.0);
        for i in 0..2 {
            let d = self.hands.as_ref().unwrap()[i].target.0.distance(p);
            let k = (1.0 - d / 15.0).clamp(0.0, 1.0);
            self.emit(Cue::ExplosionRumble, Some(i), p, k * k);
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
        self.emit(Cue::KnifeStick, Some(hand), hit, 1.0);
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
    /// Where a hand can point the gloves: along the controller's aim, or out of the palm (a hand
    /// reaching out, palm towards the thing). OpenXR's grip +X leaves the left palm and enters the
    /// right one.
    fn glove_dirs(&self, i: usize) -> [Vec3; 2] {
        let h = &self.hands.as_ref().unwrap()[i];
        let palm = if i == 0 { Vec3::X } else { -Vec3::X };
        [h.aim_rot * Vec3::Z, h.target.1 * palm]
    }

    /// The object the gloves would take: the free, pullable one nearest the middle of Alyx's cone
    /// (15 degrees about either direction, from 25 cm behind the hand, out to `range`), with a clear
    /// line to it. The current target keeps hold out to 20 degrees.
    fn glove_target(&self, origin: Vec3, dirs: [Vec3; 2], current: Option<u32>, range: f32) -> Option<u32> {
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
            let cos = dirs.iter().map(|&dir| dir.dot((p - (origin - dir * alyx::CONE_BACK)).normalize_or_zero())).fold(-1.0, f32::max);
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
        if !s.gravity_gloves || h.holding.is_some() || h.support.is_some() || h.handle.is_some() {
            if !matches!(h.glove, Glove::Flight(_)) {
                self.set_glove(i, Glove::Idle);
            }
            return;
        }
        let origin = h.target.0;
        let dirs = self.glove_dirs(i);
        match h.glove {
            Glove::Idle | Glove::Hover(_) => {
                if h.hover != Hover::None {
                    self.set_glove(i, Glove::Idle);
                    return;
                }
                let current = if let Glove::Hover(id) = h.glove { Some(id) } else { None };
                let best = self.glove_target(origin, dirs, current, s.glove_range);
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
                            self.emit(Cue::GloveHover, Some(i), origin, 0.6);
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
                // Pointing well away (60 degrees), or the grip let go for 0.25 s: it drops the lock.
                let released = h.glove_released.is_some_and(|t| now - t > alyx::LOCK_RELEASE);
                if released || dirs.iter().all(|d| d.dot(to) < alyx::DETACH_COS) {
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
                // The flick: the hand yanks back towards you or up, or the wrist flicks back.
                let h = &self.hands.as_ref().unwrap()[i];
                let v = h.history.recent();
                let ang = self.map.vel(h.ang_t) / self.map.world_per_meter.max(1e-6);
                let flat = Vec3::new(to.x, 0.0, to.z).normalize_or_zero();
                let wrist = ang.dot(flat.cross(Vec3::Y));
                if (-to).dot(v).max(v.y * 0.8) > s.glove_flick_speed || wrist > alyx::FLICK_WRIST {
                    let palm = origin;
                    let (dur, g) = alyx_pull(p.distance(palm));
                    let o = self.objects.get_mut(&id).unwrap();
                    o.holder = Holder::Pulled { hand: i, t0: now, dur, gravity: g };
                    self.physics.set_gravity(body, g / GRAVITY);
                    self.physics.set_angular_damping(body, 8.0);
                    let launch = (palm - p) / dur + Vec3::Y * (0.5 * g * dur);
                    let (_, w) = self.physics.velocity(body);
                    self.physics.set_velocity(body, launch, w);
                    self.set_glove(i, Glove::Flight(id));
                    self.debug.pulls += 1;
                    self.emit(Cue::GloveFlick, Some(i), origin, 1.0);
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
                        Some((bp, bq, t0)) if self.now - t0 < SNAP_BLEND => {
                            let k = smooth((self.now - t0) / SNAP_BLEND);
                            (bp.lerp(target.0, k), bq.slerp(target.1, k))
                        }
                        _ => target,
                    };
                    // Weight: heavy things accelerate less (Godot's 15 -> 300 N idea, scaled by the
                    // weight setting); stiffness drops to 5% while pressing into the world (HIGGS).
                    let two = self.hands.as_ref().unwrap()[hand ^ 1].support == Some(*id);
                    let mass = o.desc.mass.max(0.05);
                    // How hard you can move it: a 3.5 kg rifle speeds up at about 30 m/s^2 and spins
                    // at about 60 rad/s^2 in one hand (a quarter turn takes a third of a second),
                    // twice and three times that in two; a pistol nearly and a bottle fully keeps up.
                    let heft = (1.0 + 2.0 * s.interaction.weight * mass).powi(2);
                    let max_accel = 600.0 * if two { 2.2 } else { 1.0 } / heft;
                    let max_ang_accel = 1200.0 * if two { 3.0 } else { 1.0 } / heft;
                    let gain = if o.in_contact { 0.05 } else { 1.0 };
                    let ang_gain = 1.0f32;
                    let (now_pos, _) = self.physics.pose(o.body);
                    if now_pos.distance(target.0) > LEASH {
                        // Stuck behind something while the hand moved on: back into the hand.
                        self.physics.set_pose(o.body, target.0, target.1);
                        self.physics.set_velocity(o.body, Vec3::ZERO, Vec3::ZERO);
                    }
                    self.physics.track(o.body, target.0, target.1, dt, gain, max_accel, ang_gain, max_ang_accel);
                    self.physics.set_gravity(o.body, 0.0);
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
                    let Some((wp, wr)) = self.obj_pose(wid) else { continue };
                    let w = &self.objects[&wid];
                    let o = &self.objects[id];
                    let p = wp + wr * (v3(w.desc.mag_well) - v3(o.desc.mag_well));
                    self.physics.move_kinematic(o.body, p, wr);
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
                        self.emit(Cue::GloveCatch, Some(hand), palm, 1.0);
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
        // Empty hands follow their controllers; past the leash they snap back.
        let now = self.now;
        let hands = self.hands.as_mut().unwrap();
        for (i, h) in hands.iter_mut().enumerate() {
            if h.holding.is_some() || h.support.is_some() {
                continue;
            }
            let (pos, _) = self.physics.pose(h.body);
            if pos.distance(h.target.0) > LEASH {
                self.physics.set_pose(h.body, h.target.0, h.target.1);
                self.physics.set_velocity(h.body, Vec3::ZERO, Vec3::ZERO);
                h.leashed_until = now + 0.1;
                self.physics.set_groups(h.collider, groups_none());
            } else if h.leashed_until > 0.0 && now > h.leashed_until {
                h.leashed_until = 0.0;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
            if s.interaction.physics_hands {
                self.physics.track(h.body, h.target.0, h.target.1, dt, 1.0, 1e5, 1.0, 1e5);
            } else {
                self.physics.set_pose(h.body, h.target.0, h.target.1);
                self.physics.set_groups(h.collider, groups_none());
            }
        }
    }

    fn after_step(&mut self, inp: &RigInput) {
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
            let hit = match o.holder {
                Holder::Free | Holder::Pulled { .. } => change > 1.2 && o.prev_vel.length() > 1.0,
                Holder::Held(_) => contact && !o.in_contact && o.prev_vel.length() > 0.4,
                _ => false,
            };
            if hit && self.now >= o.next_impact {
                let hand = if let Holder::Held(h) = o.holder { Some(h) } else { None };
                let strength = (change.max(o.prev_vel.length()) / 6.0).clamp(0.05, 1.0);
                impacts.push((hand, pos, strength * (o.desc.mass / 2.0).clamp(0.4, 1.0)));
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
        for (hand, pos, strength) in impacts {
            self.emit(Cue::Impact, hand, pos, strength);
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
                self.emit(Cue::Release, Some(i), target, 0.6);
                continue;
            }
            let rot = (wr * self.support_rot(&o.desc, i)).normalize();
            let h = &self.hands.as_ref().unwrap()[i];
            let (body, collider) = (h.body, h.collider);
            self.physics.set_groups(collider, groups_none());
            self.physics.set_pose(body, on_gun, rot);
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
            } else if h.support.is_some() {
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
                    | if h.holding.is_some() || h.support.is_some() { HAND_HOLDING } else { 0 },
                holding: h.holding.or(h.support).unwrap_or(0),
                _pad: [0; 2],
            };
            if h.blocked {
                out.ghosts[i] = Some(pose(h.grip_t.0, h.grip_t.1));
            }
            out.hand_busy[i] = h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.pin.is_some();
            out.holds[i] = match h.holding.and_then(|id| self.objects.get(&id)) {
                Some(o) if o.desc.kind == OBJ_WEAPON => rustvr_ik::HoldShape::PistolGrip,
                Some(o) if o.desc.kind == OBJ_MAGAZINE => rustvr_ik::HoldShape::Magazine,
                // A knife's handle in the fist.
                Some(o) if o.throw.as_ref().is_some_and(|t| t.is_knife()) => rustvr_ik::HoldShape::Foregrip,
                Some(o) if o.desc.half_extents.iter().fold(0.0f32, |a, b| a.max(*b)) < 0.07 => rustvr_ik::HoldShape::Small,
                Some(_) => rustvr_ik::HoldShape::Large,
                None if h.support.is_some() || h.handle.is_some() => rustvr_ik::HoldShape::Foregrip,
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
                        (Hover::Object(id), _) => Some(id),
                        (_, Glove::Hover(id) | Glove::Locked(id)) => Some(id),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (id, o) in self.objects.iter().take(MAX_OBJECTS) {
            let (p, q) = self.physics.pose(o.body);
            let (lv, av) = self.physics.velocity(o.body);
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
            if o.spawned {
                flags |= OS_SPAWNED;
            }
            let (rounds, shots, chambered, slide) = match &o.weapon {
                Some(w) => {
                    let rounds = w.internal.or_else(|| w.mag.and_then(|m| self.objects.get(&m)).map(|m| m.rounds)).unwrap_or(0);
                    if w.slide_locked {
                        flags |= OS_SLIDE_LOCKED;
                    }
                    (rounds, w.shots, w.chambered as u32, w.slide)
                }
                None => (o.rounds, o.throw.as_ref().map(|t| t.detonations).unwrap_or(0), 0, 0.0),
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
                fuse: o.throw.as_ref().map(|t| t.fuse_left(self.now)).unwrap_or(0.0),
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
    /// Throwables: `THROW_*`, the tip (knives, object space) and the seconds left on its fuse.
    pub throw_flags: u32,
    pub muzzle: Vec3,
    pub fuse: f32,
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
