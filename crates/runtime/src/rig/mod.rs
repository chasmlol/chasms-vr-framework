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
pub mod bow;
pub mod creatures;
pub mod embed;
pub mod feedback;
pub mod filters;
pub mod fingers;
pub mod grips;
pub mod valve;
pub mod kick;
pub mod physics;
pub mod pickpocket;
pub mod pockets;
pub mod receptacle;
pub mod recoil;
pub mod sheet;
pub mod throwable;
pub mod throwing;
pub mod weapon;
pub mod zones;

use glam::{EulerRot, Mat3, Quat, Vec3};
use rapier3d::prelude::{ColliderHandle, RigidBodyHandle};
use rustvr_protocol::*;
use std::collections::{BTreeMap, HashMap};

use crate::backend::HmdFrame;
use crate::settings::{BodySlot, GrenadeArming, Settings};
use actions::{Chamber, Cycle, CycleEvent, HingeEvent};
use feedback::{Cue, Feedback};
use filters::OneEuro;
use physics::{Physics, groups_free, groups_game_prop, groups_hand, groups_held, groups_none};
use pickpocket::{InPocket, Pocketed};
use receptacle::{Bowl, Grind};
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
    /// A thing to take ([`OBJF_LOOT`]) put in this body slot: the game's to take.
    Stowed(u32),
    /// An arrow on a bow's string ([`bow`]): it goes where the bow and the draw put it (the hand
    /// drawing it, if any, holds it too).
    Nocked { bow: u32 },
    /// Shown still at slot `0` of the player's things while their editor is open: what it gives
    /// first, where it is ([`pockets`]).
    Shown(usize),
    /// In a bowl ([`receptacle`]): lying at this pose in it (the bowl's object space). `grinder`:
    /// the bowl's grinder resting in it, not one of the things in it.
    InBowl { bowl: u32, at: Vec3, rot: Quat, grinder: bool },
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
    /// Things that open ([`OBJF_OPENS`]): times the other hand opened or shut it.
    opens: u32,
    /// Touching the world or another object (a held one knocking into something is felt).
    in_contact: bool,
    /// Held: where the player's walk and turn take it this step, and the share of its velocity the
    /// last one gave it (`physics::Drive::carry`, `carried`).
    carry: Option<(Vec3, Quat)>,
    carried: (Vec3, Vec3),
    /// Held: where the hand would have it (its drive's target, as the hand swings it), and how far
    /// its hold is behind that (world).
    aim: Option<(Vec3, Quat)>,
    lag: Vec3,
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
    /// Stuck in something it chopped or stabbed into ([`embed`]).
    embed: Option<embed::Embedded>,
    /// A bow's string and the arrow on it ([`bow`]).
    bow: Option<bow::BowState>,
    /// A thing drawn from one of the body's slots ([`pockets`]): the runtime's copy of one of the
    /// player's things.
    drawn: Option<pockets::Drawn>,
    /// Shown at a slot while the editor is open: the stack ([`InvItem::id`]) it shows.
    shown: Option<u32>,
    /// A copy of a thing a hand took out of one of the game's creatures' pockets ([`pickpocket`]).
    pocketed: Option<Pocketed>,
    /// In a bowl: how far it's ground (0..1), how much it's shrunk to fit, and where it fell into
    /// its place from (world pose) and when.
    ground: f32,
    fit: f32,
    glide: Option<(Vec3, Quat, f64)>,
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
    /// The quiver, while the other hand holds this bow: an arrow for it.
    Quiver(u32),
    /// The string of the other hand's bow, an arrow resting on it: draw it.
    String(u32),
    /// A part of one of the game's creatures (its id, the part's index).
    Creature(u32, usize),
    /// One of the body's slots that hand out the player's things ([`pockets`]).
    Pocket(usize),
    /// In one of the game's creatures' pockets (its id).
    CreaturePocket(u32),
}

/// A hand holding a hinged action's barrels: which gun, where on them it took them (weapon space,
/// shut) and how far open they were then.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BarrelGrab {
    gun: u32,
    at: Vec3,
    angle: f32,
}

/// Where on a thing a hand has it: anywhere (the fingers close on the thing itself), or on one of
/// its handles ([`grips`]: the fingers close round the handle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum On {
    #[default]
    Free,
    /// Its own grip: a gun's pistol grip, a sword's handle.
    Grip,
    /// The handle a second hand takes: a gun's handguard.
    Foregrip,
}

/// A second hand on what the other hand holds: which thing, the hand's hold on it (its controller
/// grip pose in the thing's space) and where on it that is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Support {
    id: u32,
    at: Vec3,
    rot: Quat,
    on: On,
    /// Where the hold is from the hand's controller (the controller's own space): the hand was
    /// stopped short of where the controller went, and stays so (0 on a handle).
    reach: Vec3,
}

/// A hand's fingers on what it holds, for the drawn hand (see [`Rig::grasps`]).
#[derive(Clone, Debug, Default)]
struct GraspState {
    /// What it has hold of (the thing, where on it), and since when.
    key: Option<(u32, On)>,
    since: f64,
    /// The last grasp in the hand's own space (its controller grip pose), and how far closed.
    last: Option<(rustvr_avatar::Grasp, f32)>,
    /// Let go: since when, from how far closed, on what (hand space).
    fading: Option<(f64, f32, rustvr_avatar::Grasp)>,
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
    /// The index finger's tip (world), where the drawn one's is: what touches a sheet ([`sheet`]).
    index_tip: Vec3,
    /// The fingers as the controller has them, eased as the drawn hand's are (the same solver on
    /// the same input), this frame's: the capsules are where the drawn fingers are (a lifted thumb
    /// or index straight).
    finger_solver: rustvr_ik::FingerSolver,
    drawn_curls: [f32; 5],
    /// The fingers push things only while the hand has nothing (holding, steering or working
    /// something, they're on it); and they knock loose things about only while it isn't about to
    /// take one (they'd nudge it out of the grab).
    fingers_on: bool,
    grip_down: bool,
    grip_toggled: bool,
    prev_buttons: u32,
    trigger: f32,
    /// The grip's squeeze (0..1) as the controller has it.
    squeeze: f32,
    holding: Option<u32>,
    /// Where on what it holds it has it.
    on: On,
    /// Taken anywhere: the drawn hand's place on it (its grip pose in the thing's space), where the
    /// hand was when it took it (the thing is held from the controller as it was then; neither
    /// moves). None: on its grip, as the attach has it.
    hold: Option<(Vec3, Quat)>,
    /// What it held or had hold of last frame.
    was_on: Option<u32>,
    /// Just let go of this, which the other hand still holds: it comes out of it rather than being
    /// put out of it (else what the other hand holds is solid to it), until it's clear of it.
    passing: Option<u32>,
    /// Empty: where it was sent last frame and where its controller was then (its catching up is
    /// eased from there).
    reach_from: Option<(Vec3, Vec3)>,
    /// Empty, resting on what the other hand holds (that thing's id): it goes along with it.
    rests_on: Option<u32>,
    /// Empty and stuck on the wrong side of something (its controller clear through it): finding
    /// its way round.
    round: Round,
    /// Loose things it's held back by (pinned where they are, a gun on a table): solid to it while
    /// it goes round them.
    pinned: Vec<ColliderHandle>,
    /// Its wrist turned off the controller's to roll round an edge, and where that's easing to.
    tilt: Quat,
    tilt_to: Quat,
    /// How far each finger (thumb .. little) is pushed open by what the empty hand is pressed on
    /// (0 or less), and the fingers' curls with it.
    give: [f32; 5],
    curls: [f32; 5],
    /// Empty: its fingers as the controller has them, and as far open as they'd be pushed.
    held_curls: [f32; 5],
    open: [f32; 5],
    support: Option<Support>,
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
    /// On a creature's part ([`creatures`]).
    creature: Option<creatures::Hold>,
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
    /// How fast the target moves in the world (the player's walking with it), from its last two
    /// places; zero across a jump (a snap turn, a teleport).
    target_vel: Vec3,
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
    grasp: GraspState,
    /// Laid slowly on something it could take: it rests on it without shoving it (see
    /// [`Rig::rest_on`]).
    yielding: bool,
    /// Pressed on a part that swings through the room last frame (a door's leaf it's pushing):
    /// gripping takes it, however fast it's swinging away from the palm.
    pressing: Option<u32>,
    /// The grip closed in a slot that hands out the player's things: which, when, where the hand
    /// was (tracking space), and what it drew: held there still, it opens the slot's editor.
    pocket_press: Option<(usize, f64, Vec3, Option<u32>)>,
    /// The grip as it was last frame, for placing a slot by hand (which works in menus too).
    place_grip: bool,
    /// In one of the game's creatures' pockets, on one of its things ([`pickpocket`]); when its
    /// creature's watching heart next beats in it, and how aware the creature was last frame.
    pocket: Option<InPocket>,
    heartbeat: f64,
    aware: f32,
    /// What it was on in the pockets when the game was last told ([`Rig::felt`]), and since when.
    felt: [u32; 4],
    felt_since: f64,
    /// Let go while the game decided on the thing it took out of a pocket: let go once it has.
    let_go: bool,
}

impl Hand {
    /// What it supports with the other hand's (see [`Support`]).
    fn supporting(&self) -> Option<u32> {
        self.support.map(|s| s.id)
    }
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
    /// How far each empty hand's fingers (thumb .. little) are pushed open by what it's pressed on
    /// (0 or less: added to their curls).
    pub finger_give: [[f32; 5]; 2],
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
    /// Held blades that chopped or stabbed into something and stayed in.
    pub embeds: u32,
    /// Things to take put in a body slot ([`OBJF_LOOT`]).
    pub stowed: u32,
    /// The player's things drawn from the body's slots, put back, and used (eaten, drunk, read, put
    /// on a weapon).
    pub pocket_draws: u32,
    pub pocket_returns: u32,
    pub pocket_uses: u32,
    /// Things gone into a bowl, and grinding strokes in one ([`receptacle`]).
    pub into_bowls: u32,
    pub grind_strokes: u32,
    pub spills: u32,
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
    /// Arrows taken from the quiver, nocked and loosed, and the last loose's strength.
    pub quiver_draws: u32,
    pub nocks: u32,
    pub looses: u32,
    pub last_loose: f32,
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
    /// The player's arm, shoulder to grip (metres; 0: unknown): a bow's full draw fits it.
    pub arm_length: f32,
}

/// An arrow loosed from a bow this frame ([`Rig::loosed`]): for the game to fire its own
/// ([`CE_LOOSE`]). World space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loose {
    pub bow: u32,
    /// The arrow's template (what it is).
    pub arrow: u32,
    /// The string hand.
    pub hand: usize,
    /// The arrow's tip, and its way along the bow.
    pub point: Vec3,
    pub dir: Vec3,
    /// The draw, 0..1 of the full draw.
    pub strength: f32,
}

const SPAWN_BASE: u32 = 0x8000_0000;
/// A hand inside one of the game's props asks for this many m/s per metre of overlap, on top of
/// its own speed into it, and never more than `PUSH_MAX` in all.
const PUSH_OUT: f32 = 10.0;
const PUSH_MAX: f32 = 4.0;
/// Empty hand `i`'s fingers (`h`): each eased (`ease` of the way, at most `step` a frame) towards
/// where it fits with the hand at `aim` (position, rotation): as curled as the controller has it,
/// or the nearest it gets to that either way without going into what's solid to the hand (pushed
/// open as far as it gives, or curled in; `passing`: the thing it's coming out of). Never into
/// anything with the hand where it is (`now`): else the nearest it can get to within the step that
/// isn't.
#[allow(clippy::too_many_arguments)]
fn fit_fingers(physics: &Physics, offsets: &rustvr_ik::HandOffsets, i: usize, h: &mut Hand, aim: (Vec3, Quat), now: (Vec3, Quat), passing: Option<ColliderHandle>, ease: f32, step: f32) {
    let solid = physics::solid_for_hand(i);
    let side = if i == 0 { rustvr_ik::Side::Left } else { rustvr_ik::Side::Right };
    let joint = offsets.hand_frame(side, rustvr_ik::Pose::IDENTITY);
    let (held, open) = (h.held_curls, h.open);
    for f in 0..5 {
        let fits_at = |at: (Vec3, Quat), c: f32| {
            let mut curls = held;
            curls[f] = c;
            let (a, b) = fingers::segments((joint.pos, joint.rot), i == 1, curls)[f];
            !physics.finger_in(at.0 + at.1 * a, at.0 + at.1 * b, fingers::RADIUS, solid, passing)
        };
        let fits = |c: f32| fits_at(aim, c);
        let curl = if fits(held[f]) {
            held[f]
        } else {
            // Out from where the controller has it, a step either way at a time, then closer in.
            let mut found = None;
            'out: for k in 1..=FIT_STEPS {
                let d = k as f32 / FIT_STEPS as f32 * (1.0 - open[f]);
                for c in [held[f] - d, held[f] + d] {
                    if (open[f]..=1.0).contains(&c) && fits(c) {
                        found = Some((c, (c - held[f]).signum() * -d / k as f32));
                        break 'out;
                    }
                }
            }
            match found {
                Some((free, back)) => {
                    // `free` fits, `free + back` (a step back towards the controller's) didn't.
                    let (mut ok, mut not) = (free, free + back);
                    for _ in 0..4 {
                        let mid = 0.5 * (ok + not);
                        if fits(mid) {
                            ok = mid;
                        } else {
                            not = mid;
                        }
                    }
                    ok
                }
                None => open[f],
            }
        };
        // Eased towards it, at most a step a frame...
        let mut give = h.give[f] + ((curl - held[f] - h.give[f]) * ease).clamp(-step, step);
        // ...never into anything where the hand is now: else the nearest within the step that isn't.
        if !fits_at(now, held[f] + give) {
            let lo = (h.give[f] - step).max(open[f] - held[f]);
            let hi = (h.give[f] + step).min(1.0 - held[f]);
            let mut best: Option<f32> = None;
            for k in 0..=FIT_STEPS {
                let g = lo + (hi - lo) * k as f32 / FIT_STEPS as f32;
                if fits_at(now, held[f] + g) && best.is_none_or(|b| (g - give).abs() < (b - give).abs()) {
                    best = Some(g);
                }
            }
            if let Some(b) = best {
                give = b;
            }
        }
        h.give[f] = give;
        h.curls[f] = held[f] + give;
    }
}

/// Steps a finger pushed aside takes looking for room, out from its curl either way.
const FIT_STEPS: usize = 10;
/// An empty hand stuck on the wrong side of something, finding its way round it.
#[derive(Clone, Copy, Debug, Default)]
enum Round {
    #[default]
    No,
    /// Trying ways along the surface it's stuck on (`normal`): from where it is (`from`) towards its
    /// controller (`toward`), the ways tried so far and the best (how far round, which way).
    Looking { from: Vec3, toward: Vec3, normal: Vec3, dirs: [Vec3; PROBE_DIRS], tried: usize, best: Option<(f32, Vec3)> },
    /// Going round that way since `since`; where it was last frame and how long it's hardly moved.
    Going { dir: Vec3, normal: Vec3, toward: Vec3, since: f64, last: Vec3, still: f32 },
    /// No way round within reach (a wall, the floor): it rests where it is and looks again after.
    NoWay { until: f64 },
}

/// Stuck: held back this far (m) and getting no nearer, its controller clear (this much room round
/// it) on the far side of what's in the way.
const STUCK: f32 = 0.05;
const STUCK_CLEAR: f32 = 0.02;
/// Looking for the way round: this many ways along the surface (a few a frame), each walked this
/// far a step at a time, a little off the surface (so it doesn't catch on it).
const PROBE_DIRS: usize = 12;
const PROBE_PER_FRAME: usize = 3;
const PROBE_STEPS: usize = 5;
const PROBE_STEP: f32 = 0.08;
const PROBE_LIFT: f32 = 0.15;
/// Going round: it heads this far ahead that way at a time; the controller moving this far (m)
/// starts the looking over; hardly moving (less than this a frame) this long (s), or going round
/// this long at all, it gives up for `NO_WAY_WAIT`.
const ROUND_AHEAD: f32 = 0.12;
const ROUND_REPLAN: f32 = 0.15;
const ROUND_STILL_MOVE: f32 = 0.003;
const ROUND_STILL: f32 = 0.25;
const ROUND_MAX: f32 = 2.0;
const NO_WAY_WAIT: f64 = 0.5;
/// How far the wrist rolls (radians) to get over an edge it's caught on.
const ROUND_TILT: f32 = 0.45;
/// An empty hand this far short of where it was sent (m) is held back by something loose; pressed
/// on such a thing that won't move, it presses on at most this fast (m/s).
const PINNED: f32 = 0.005;
const PRESS_SPEED: f32 = 0.6;
/// An empty hand held back this far from its controller (m) is pressed on something; held back
/// at all (beyond rounding), it catches up eased. Moving freely it follows its controller exactly.
const PRESSED: f32 = 0.01;
const HELD_BACK: f32 = 0.002;
/// An empty hand this close to what the other hand holds (m) rests on it.
const RESTING: f32 = 0.006;
/// Pressed on something, a hand turns with its controller this far behind (s).
const PRESSED_TURN_TIME: f32 = 0.06;
/// Fingers follow the controller and give way over about this long (s), and move at most this
/// fast (curl a second: from straight to a fist in a fifth of a second).
const FINGER_EASE_TIME: f32 = 0.06;
const FINGER_SPEED: f32 = 5.0;
/// An empty hand held back catches up with its controller over about this long (s), once what
/// held it back is out of the way: quickly (settled in about a tenth of a second), but a glide,
/// not a jump.
const CATCH_UP_TIME: f32 = 0.045;
/// The most a blade stuck in a loose thing gives it at once, dragging it (N s).
const DRAG_IMPULSE_MAX: f32 = 20.0;
/// A held thing pressed into a creature asks for this many m/s per metre the hand would have it
/// go further.
const PUSH_LAG: f32 = 8.0;
/// Where the shoulder is from the eyes (for the gloves' reach): this far down, and out to the side.
const SHOULDER_DROP: f32 = 0.25;
const SHOULDER_OUT: f32 = 0.18;
/// Faster than this (m/s), a hand pushes the way it moves.
const PUSH_MOVING: f32 = 0.2;
/// A hand squeezing its grip less than this is open (it shoves; squeezing more, it's taking hold or
/// a fist).
const OPEN_HAND: f32 = 0.3;
const LEASH: f32 = 0.7;
/// How near (metres) a hand must be to a part that swings through the room to be pressing on it.
const PRESS_GAP: f32 = 0.01;
/// How wide the player's body is to a door swinging at them, and how far down from the head it
/// goes (the capsule's).
const BODY_RADIUS: f32 = 0.2;
const BODY_LENGTH: f32 = 1.25;
/// A turn (radians) or a move (metres) of the player's mapping in one step past which it's a snap
/// turn or a teleport, not a walk (as `crate::locomotion` tells them apart).
const SNAP_TURN: f32 = 0.3;
const SNAP_MOVE: f32 = 1.0;
/// The least a held thing speeds up and spins up, however heavy (m/s^2, rad/s^2).
const MIN_HELD_ACCEL: f32 = 15.0;
const MIN_HELD_ANG_ACCEL: f32 = 40.0;
const BLOCKED: f32 = 0.04;
const FOREGRIP_REACH: f32 = 0.14;
/// The palm, from the controller's grip pose: this far towards the back of the hand (metres). A
/// thing taken anywhere is brought onto it, no further than `PALM_REACH` for a second hand.
const PALM: f32 = 0.02;
const PALM_REACH: f32 = 0.1;
/// A hand this near what the other hand holds (metres, from its grip pose to the surface) has its
/// palm on it.
const ON_IT: f32 = 0.035;
/// ... and this near a book the other hand holds ([`OBJF_OPENS`]): a hand reaching for its cover
/// to open it may be stopped a little short of it by the pages over it.
const ON_COVER: f32 = 0.08;
/// A hand takes a thing it's on: its palm or a finger touching it, or all but (metres).
const TOUCH: f32 = 0.03;
/// A hand slower than this (m/s) near a thing it could take is reaching for it, not swatting it.
const REACH_SPEED: f32 = 0.8;
/// Two hands closer than this on a thing (metres) don't aim it.
const TWO_HANDS_APART: f32 = 0.06;
/// How long the fingers take to close on what the hand takes, or to open off it (seconds).
const GRASP_TIME: f64 = 0.12;
/// The support hand lets go of the foregrip when its controller is this far from the gun's line.
const FOREGRIP_BREAK: f32 = 0.3;
/// A controller that moves faster than this (m/s) in the world has jumped (the player snap-turned
/// or was moved), not moved: no hand goes that fast.
const TARGET_JUMP: f32 = 15.0;
/// Letting go of something faster than this throws it rather than holstering it.
const HOLSTER_MAX_SPEED: f32 = 1.2;
/// Letting go over a bowl's mouth faster than this (m/s) flings it on, past the bowl.
const BOWL_MAX_SPEED: f32 = 2.5;
/// How long a thing let go into a bowl takes to fall into its place there (seconds).
const BOWL_FALL: f64 = 0.18;
/// How often the hand on a grinder feels its grinding (seconds between pulses).
const GRIND_FEEL: f64 = 0.045;
/// A bowl whose mouth faces lower than this (its up's height, -1 upside down) spills, and one
/// facing up less than `UPRIGHT` takes nothing in.
const SPILL: f32 = -0.3;
const UPRIGHT: f32 = 0.5;
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
    /// The creatures' body parts this frame (world), what a blade stuck in one rides with.
    actor_parts: HashMap<(u32, usize), (Vec3, Vec3, f32)>,
    /// Those parts in the physics: kinematic capsules following them (half length, radius), that
    /// the hands and what they hold meet ([`Rig::follow_actors`]).
    proxies: HashMap<(u32, usize), (RigidBodyHandle, ColliderHandle, f32, f32)>,
    /// What blades stuck in creatures drag them by this frame (pushes for the game).
    drags: Vec<GamePush>,
    /// What the game says about its creatures' bodies (for the hands that take hold of them).
    bodies: HashMap<u32, creatures::Body>,
    /// How strong the game says the player is (1: an average person).
    game_strength: f32,
    /// Each hand's grab of a creature, for the game ([`CreatureGrabs`]), and the last number given
    /// to one.
    grabs: [CreatureGrab; 2],
    grab_serial: u32,
    /// Where the game says its own simulated objects are (`OBJF_GAME_SIMULATED`, world space).
    game_poses: HashMap<u32, (Vec3, Quat)>,
    /// Where the hands push the game's own free objects after this step (see [`GamePush`]).
    pub game_pushes: Vec<GamePush>,
    /// The game's convex collision for its objects ([`SHAPE_HULL`]), object space.
    game_hulls: HashMap<u32, Vec<Vec<Vec3>>>,
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
    /// What each hand's fingers close on this frame (tracking space).
    grasps: [Option<rustvr_avatar::Grasp>; 2],
    /// How the game's bows bend and their arrows fly ([`Bows`]).
    bows: Box<Bows>,
    /// The arrows loosed this frame, for the game.
    pub loosed: Vec<Loose>,
    /// Hands turning a held book's pages (its grip on them): they take nothing, and their gloves
    /// pull nothing.
    page_hands: [bool; 2],
    /// The player's things as the game keeps them, and how many of its taken-back list were seen.
    inventory: Box<Inventory>,
    taken_seen: Option<u32>,
    /// The body's slots that hand them out (the player's, or the game's own), and their zones this
    /// frame.
    pockets: Vec<BodySlot>,
    pocket_zones: Vec<pockets::SlotZones>,
    /// A slot being placed by hand: the next squeeze puts it there.
    placing: Option<usize>,
    /// The slot picked in the slots' editor, while it's open.
    editing: Option<usize>,
    /// How the hand holds the things drawn from the slots, by model (None: its model couldn't be
    /// read, so it's held as it came out).
    holds: HashMap<String, Option<pockets::Hold>>,
    /// What the player did with the slots this frame, for the runtime to keep.
    pub pocket_events: Vec<PocketEvent>,
    /// The game's creatures' pockets now (world), and its latest answers ([`Rig::set_creature_pockets`]).
    creature_pockets: Vec<Pocket>,
    pocket_answers: Vec<PocketOutcome>,
    /// What the hands did with them this frame, for the game, numbered from `pocket_next` on (the
    /// ring's next number: the session pushes them in order).
    pub pickpocket_events: Vec<rustvr_protocol::PocketEvent>,
    pocket_next: u64,
    /// The creatures whose pickpocketing is open, and when a hand was last near their pockets.
    pocket_sessions: BTreeMap<u32, f64>,
    /// The creatures a hand is feeling in the pockets of, not open yet: since when, and when a hand
    /// was last in them ([`pickpocket::OPEN_AFTER`]).
    pocket_looks: BTreeMap<u32, (f64, f64)>,
    /// The bowls the hands make things in ([`Receptacles`]), by the bowl's id, as last read; and
    /// each one's grinding.
    bowls: BTreeMap<u32, Receptacle>,
    bowls_seq: u32,
    grinds: HashMap<u32, Grind>,
    /// When each hand last felt its grinding.
    grind_felt: [f64; 2],
    /// The sheets' touches ([`sheet`]).
    touches: HashMap<u32, sheet::Touches>,
}

/// Something the player did with the body's slots that the runtime keeps (in its settings) or
/// passes on (to the game's editor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PocketEvent {
    /// A grip held still in slot `0`: open its editor.
    Edit(usize),
    /// Slot `0` placed by hand: its new middle (the body anchor's frame, for a player whose eyes
    /// are 1.7 m up).
    Placed(usize, Vec3),
    /// A thing (the game's key for it) put back in slot `0`.
    PutBack(usize, u32),
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
            actor_parts: HashMap::new(),
            proxies: HashMap::new(),
            drags: Vec::new(),
            bodies: HashMap::new(),
            game_strength: 1.0,
            grabs: [CreatureGrab::default(); 2],
            grab_serial: 0,
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
            grasps: [None, None],
            bows: bytemuck::zeroed_box(),
            loosed: Vec::new(),
            page_hands: [false; 2],
            inventory: bytemuck::zeroed_box(),
            taken_seen: None,
            pockets: Vec::new(),
            pocket_zones: Vec::new(),
            placing: None,
            editing: None,
            holds: HashMap::new(),
            pocket_events: Vec::new(),
            creature_pockets: Vec::new(),
            pocket_answers: Vec::new(),
            pickpocket_events: Vec::new(),
            pocket_next: 0,
            pocket_sessions: BTreeMap::new(),
            pocket_looks: BTreeMap::new(),
            bowls: BTreeMap::new(),
            bowls_seq: 0,
            grinds: HashMap::new(),
            grind_felt: [0.0; 2],
            touches: HashMap::new(),
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

/// A grasp in the world, in tracking space.
fn grasp_in_tracking(map: &WorldMap, g: rustvr_avatar::Grasp) -> rustvr_avatar::Grasp {
    use rustvr_avatar::Solid;
    let k = 1.0 / map.world_per_meter;
    let solids = g
        .solids
        .iter()
        .map(|s| match *s {
            Solid::Sphere { centre, radius } => Solid::Sphere { centre: map.inv_point(centre), radius: radius * k },
            Solid::Capsule { a, b, radius } => Solid::Capsule { a: map.inv_point(a), b: map.inv_point(b), radius: radius * k },
            Solid::Box { centre, rot, half } => Solid::Box { centre: map.inv_point(centre), rot: map.inv_rot(rot), half: half * k },
            hull => hull,
        })
        .collect();
    let turn = map.inv_rot(Quat::IDENTITY);
    let planes = g
        .planes
        .iter()
        .map(|n| {
            let normal = turn * n.truncate();
            normal.extend(-normal.dot(map.inv_point(n.truncate() * -n.w)))
        })
        .collect();
    rustvr_avatar::Grasp { solids, planes, ..g }
}

/// How far a hand (or what it takes) goes in one grab transition: further takes longer in
/// proportion, so it never moves faster than it does over this.
pub const TRAVEL_REACH: f32 = 0.2;

/// How long a hand's transition over `distance` metres takes: the grab transition setting, longer
/// in proportion beyond [`TRAVEL_REACH`] (0: at once, whatever the distance).
/// Valve `v` as its description `d` has it: its range and clicks, its top speed, how a hand turns
/// it, whether it slides, its spring and damping.
fn valve_of(d: &ObjectDesc, v: valve::Valve) -> valve::Valve {
    let v = v.with_range(d.turn_min, d.turn_max, d.detent);
    let v = valve::Valve {
        max_speed: d.fire_rate.max(0.0),
        twist: (v.radius <= valve::TWIST_RADIUS || d.joint_flags & JOINT_TWIST != 0) && d.joint_flags & JOINT_SLIDE == 0,
        spring: d.joint_spring.max(0.0),
        damping: d.joint_damping.max(0.0),
        ..v
    };
    if d.joint_flags & JOINT_SLIDE != 0 { v.sliding(if v.slide { v.inertia } else { d.mass }) } else { valve::Valve { slide: false, ..v } }
}

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
        // Each object's convex parts (an entry each), in order.
        let mut parts: BTreeMap<u32, Vec<Vec<Vec3>>> = BTreeMap::new();
        for h in &hulls.hulls[..(hulls.count as usize).min(MAX_GAME_HULLS)] {
            let points: Vec<Vec3> = h.points[..(h.count as usize).min(MAX_HULL_POINTS)].iter().map(|p| Vec3::new(p[0], p[1], p[2])).filter(|p| p.is_finite()).collect();
            if h.id != 0 && points.len() >= 4 {
                parts.entry(h.id).or_default().push(points);
            }
        }
        for (id, parts) in parts {
            if self.game_hulls.get(&id) == Some(&parts) {
                continue;
            }
            if let Some(o) = self.objects.get(&id)
                && o.desc.shape == SHAPE_HULL
                && !self.bowls.contains_key(&id)
            {
                let (c, mass) = (o.collider, o.desc.mass);
                self.physics.set_hulls(c, &parts, mass);
            }
            self.game_hulls.insert(id, parts);
            self.weigh_valve(id);
        }
    }

    /// A valve's inertia about its axle, its centre of mass and its weight from its collider: the
    /// game's own shape of the part that moves (a door's leaf turned about its edge), at its mass.
    /// A wheel without one keeps a ring's.
    fn weigh_valve(&mut self, id: u32) {
        let Some(o) = self.objects.get(&id) else { return };
        let Some(mut v) = o.valve else { return };
        let (d, collider) = (o.desc, o.collider);
        if d.shape == SHAPE_HULL
            && let Some((mass, com, inertia)) = self.physics.inertia_about(collider, v.origin(), v.axis())
        {
            // (Where it is now: the frame its centre of mass is kept in turns with it.)
            v.com = v.local(com);
            v.inertia = if v.slide { mass } else { inertia.max(1e-4) };
        }
        v.weight = if d.joint_flags & JOINT_GRAVITY != 0 { Vec3::new(0.0, -9.81 * d.mass.max(0.0), 0.0) } else { Vec3::ZERO };
        self.objects.get_mut(&id).unwrap().valve = Some(v);
    }

    pub fn sync(&mut self, table: &ObjectTable) {
        if table.seq == self.table_seq {
            return;
        }
        self.table_seq = table.seq;
        let descs = &table.descs[..(table.count as usize).min(MAX_OBJECTS)];
        // Templates first: a gun loads itself from its magazine template as it's added.
        self.templates = descs.iter().filter(|d| d.id != 0 && d.flags & OBJF_TEMPLATE != 0).copied().collect();
        // Arrows of a kind just put on the bow from a slot (their copy in a hand as the game
        // equipped them): one of the bow's own takes the copy's place there, as a weapon does.
        let arrows: Vec<ObjectDesc> = self.templates.iter().filter(|t| t.kind == OBJ_ARROW && t.replaces != 0).copied().collect();
        for t in arrows {
            if let Some(i) = self.hands.as_ref().and_then(|h| h.iter().position(|h| h.holding == Some(t.replaces))) {
                self.remove(t.replaces);
                self.draw_arrow(i, &t, t.id);
                tracing::info!("rig: one of the bow's arrows takes the place of its copy {:#x} in the {} hand", t.replaces, if i == 0 { "left" } else { "right" });
            }
        }
        // (A driven thing is only drawn: no body, nothing the hands meet.)
        let seen: Vec<u32> = descs.iter().filter(|d| d.id != 0 && d.flags & OBJF_TEMPLATE == 0 && d.kind != OBJ_DRIVEN).map(|d| d.id).collect();
        // What's gone goes first (a holster it was in is free for what's new), and so does what
        // came back as something else (a sword become a bow): it's made again.
        let gone: Vec<u32> = self
            .objects
            .iter()
            .filter(|(id, o)| !o.spawned && (!seen.contains(id) || descs.iter().any(|d| d.id == **id && d.kind != o.desc.kind)))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.remove(id);
        }
        for d in descs {
            if !seen.contains(&d.id) {
                continue;
            }
            match self.objects.get_mut(&d.id) {
                Some(o) => {
                    o.desc = *d;
                    if let Some(t) = &mut o.throw {
                        t.flags = d.throw_flags;
                    }
                    // A valve the game changes as it goes (a door's latch, its lock): its range,
                    // stiffness, clicks and spring now, where it's turned to kept.
                    if let Some(v) = &mut o.valve {
                        let (angle, vel, inertia, com, weight) = (v.angle, v.vel, v.inertia, v.com, v.weight);
                        let mut n = valve_of(d, valve::Valve { friction: d.friction.max(0.0), ..*v });
                        n.inertia = inertia;
                        (n.com, n.weight) = (com, if d.joint_flags & JOINT_GRAVITY != 0 { weight } else { Vec3::ZERO });
                        n.angle = if n.limited() { angle.clamp(n.min, n.max) } else { angle };
                        n.vel = if n.angle != angle { 0.0 } else { vel };
                        *v = n;
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
                        self.replace(d.id, d.replaces);
                        self.conjure(d.id, d.into_hand);
                    }
                }
                None => {
                    self.add(*d, d.id, false, v3(d.spawn_pose.pos), q4(d.spawn_pose.rot));
                    self.start_in_slot(d);
                    self.replace(d.id, d.replaces);
                    self.conjure(d.id, d.into_hand);
                }
            }
        }
    }

    fn add(&mut self, d: ObjectDesc, draw_desc: u32, spawned: bool, pos: Vec3, rot: Quat) {
        let (body, collider) = self.physics.add_object(&d, pos, rot, self.game_hulls.get(&d.id).map(Vec::as_slice).unwrap_or(&[]));
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
                opens: 0,
                in_contact: false,
                carry: None,
                carried: (Vec3::ZERO, Vec3::ZERO),
                aim: None,
                lag: Vec3::ZERO,
                prev_vel: Vec3::ZERO,
                next_impact: 0.0,
                seat_armed: true,
                throw: ThrowState::new(&d),
                hang: None,
                valve: (d.kind == OBJ_VALVE).then(|| valve_of(&d, valve::Valve::new(pos, rot, d.radius, d.mass, d.friction))),
                valve_shape: Vec::new(),
                embed: None,
                bow: (d.kind == OBJ_BOW).then(bow::BowState::default),
                drawn: None,
                shown: None,
                pocketed: None,
                ground: 0.0,
                fit: 1.0,
                glide: None,
            },
        );
        // A bowl's collision is the bowl; its grinder starts resting in it.
        if self.bowls.contains_key(&id) {
            self.shape_bowl(id);
        }
        if let Some(bowl) = self.bowls.values().find(|r| r.grinder == id).map(|r| r.id) {
            self.rest_grinder(id, bowl);
        }
        // A valve's wheel turns on its axle, never anywhere else.
        if d.kind == OBJ_VALVE {
            self.physics.set_kinematic(body, true);
            // A part that swings through the room is a wall to the hands and what they hold.
            if d.joint_flags & JOINT_PANEL != 0 {
                self.physics.set_groups(collider, physics::groups_panel());
            }
            self.weigh_valve(id);
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

    /// The game's own thing taking the place of the runtime's copy of it a hand holds (a weapon
    /// drawn from one of the body's slots, equipped by the game): into that hand by its grip, from
    /// where the copy was, the copy gone.
    fn replace(&mut self, id: u32, copy: u32) {
        let Some(i) = self.hands.as_ref().and_then(|h| h.iter().position(|h| copy != 0 && h.holding == Some(copy))) else { return };
        let Some(at) = self.obj_pose(copy) else { return };
        self.remove(copy);
        for s in self.slots.iter_mut().filter(|s| **s == Some(id)) {
            *s = None;
        }
        let Some(body) = self.objects.get(&id).map(|o| o.body) else { return };
        self.physics.set_pose(body, at.0, at.1);
        self.take(id, i, true);
        tracing::info!("rig: {id:#x} takes the place of its copy {copy:#x} in the {} hand", if i == 0 { "left" } else { "right" });
    }

    /// A thing the game made in a hand (`into`: 1 the left, 2 the right; a bound weapon the hand
    /// conjured): in that hand at once, by its grip, out of the slot it would start in. Only an empty
    /// hand takes it.
    fn conjure(&mut self, id: u32, into: u32) {
        let i = match into {
            1 => 0,
            2 => 1,
            _ => return,
        };
        let Some(hands) = self.hands.as_ref() else { return };
        if hands[i].holding.is_some() || hands[i].support.is_some() {
            tracing::info!("rig: {id:#x} would be made in the {} hand, which holds something", if i == 0 { "left" } else { "right" });
            return;
        }
        let (palm, rot) = hands[i].target;
        let Some(body) = self.objects.get(&id).map(|o| o.body) else { return };
        for s in self.slots.iter_mut().filter(|s| **s == Some(id)) {
            *s = None;
        }
        self.physics.set_pose(body, palm, rot);
        self.take(id, i, true);
        tracing::info!("rig: {id:#x} made in the {} hand", if i == 0 { "left" } else { "right" });
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
        // What was in it falls out; what it was in lays out what's left.
        let inside: Vec<u32> = self.objects.iter().filter(|(_, o)| matches!(o.holder, Holder::InBowl { bowl, .. } if bowl == id)).map(|(i, _)| *i).collect();
        for i in inside {
            self.free_body(i, groups_free());
        }
        let was_in = self.objects.get(&id).and_then(|o| if let Holder::InBowl { bowl, grinder: false, .. } = o.holder { Some(bowl) } else { None });
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
                    if h.supporting() == Some(id) {
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
            // An arrow off its string; a bow's arrow falls.
            if let Holder::Nocked { bow } = o.holder
                && let Some(b) = self.objects.get_mut(&bow).and_then(|b| b.bow.as_mut())
            {
                (b.nocked, b.drawing) = (None, None);
            }
            if let Some(a) = o.bow.and_then(|b| b.nocked) {
                self.free_body(a, groups_free());
            }
        }
        if let Some(bowl) = was_in {
            self.arrange(bowl);
        }
    }

    // ---- bowls -----------------------------------------------------------------------------------

    /// The bowls the hands make things in ([`Receptacles`], the adapter's): their collision made
    /// a bowl's, what's in one no longer a bowl loose again, a grinder no hand has had resting in
    /// its bowl, and what may no longer go into one out of it.
    pub fn set_receptacles(&mut self, table: &Receptacles) {
        if table.seq == self.bowls_seq {
            return;
        }
        self.bowls_seq = table.seq;
        let new: BTreeMap<u32, Receptacle> = table.entries().iter().filter(|r| r.id != 0).map(|r| (r.id, *r)).collect();
        let gone: Vec<u32> = self.bowls.keys().filter(|id| !new.contains_key(id)).copied().collect();
        let changed: Vec<u32> = new.iter().filter(|(id, r)| self.bowls.get(id).is_none_or(|o| Bowl::of(o) != Bowl::of(r))).map(|(id, _)| *id).collect();
        self.bowls = new;
        for id in gone {
            self.unbowl(id);
        }
        for id in changed {
            self.shape_bowl(id);
        }
        let rests: Vec<(u32, u32)> = self.bowls.values().filter(|r| r.grinder != 0).map(|r| (r.grinder, r.id)).collect();
        for (g, bowl) in rests {
            if self.objects.get(&g).is_some_and(|o| o.holder == Holder::Free && !o.handled) {
                self.rest_grinder(g, bowl);
            }
        }
        let out: Vec<(u32, u32)> = self
            .objects
            .iter()
            .filter_map(|(id, o)| match o.holder {
                Holder::InBowl { bowl, grinder: false, .. } if self.bowls.get(&bowl).is_some_and(|r| !r.accepts().contains(id)) => Some((*id, bowl)),
                _ => None,
            })
            .collect();
        for (id, bowl) in out {
            self.free_body(id, groups_free());
            self.arrange(bowl);
        }
    }

    /// A bowl's collision: its floor and its wall ([`Bowl::parts`]), from the bottom of its box.
    fn shape_bowl(&mut self, id: u32) {
        let (Some(r), Some(o)) = (self.bowls.get(&id), self.objects.get(&id)) else { return };
        let bottom = o.desc.center[1] - o.desc.half_extents[1];
        let parts = Bowl::of(r).parts(bottom);
        let (c, mass) = (o.collider, o.desc.mass);
        self.physics.set_boxes(c, &parts, mass);
    }

    /// `id` is no longer a bowl: what's in it is loose, and its collision is its own again.
    fn unbowl(&mut self, id: u32) {
        let inside: Vec<u32> = self.objects.iter().filter(|(_, o)| matches!(o.holder, Holder::InBowl { bowl, .. } if bowl == id)).map(|(i, _)| *i).collect();
        for i in inside {
            self.free_body(i, groups_free());
        }
        if let Some(o) = self.objects.get(&id) {
            let (c, d) = (o.collider, o.desc);
            let hulls = self.game_hulls.get(&id).cloned().unwrap_or_default();
            self.physics.set_object_shape(c, &d, &hulls);
        }
        self.grinds.remove(&id);
    }

    /// A point in the world in a bowl's object space (metres).
    fn in_bowl_space(&self, bowl: u32, at: Vec3) -> Option<Vec3> {
        let (bp, bq) = self.obj_pose(bowl)?;
        Some(bq.inverse() * (at - bp) / self.map.world_per_meter.max(1e-6))
    }

    /// The bowl `id` would go into if let go of at `at` (world; `above`, `slack`: how far over the
    /// rim and out past it its mouth still takes it): one that takes it and has room for it.
    fn bowl_for(&self, id: u32, at: Vec3, above: f32, slack: f32) -> Option<u32> {
        self.bowls.values().find_map(|r| {
            if r.id == id || !r.accepts().contains(&id) || self.contents(r.id).len() >= r.capacity as usize || !self.upright(r.id) {
                return None;
            }
            let local = self.in_bowl_space(r.id, at)?;
            Bowl::of(r).in_mouth(local, above, slack).then_some(r.id)
        })
    }

    /// A bowl the right way up enough to take things (its mouth facing up more than sideways).
    fn upright(&self, bowl: u32) -> bool {
        self.obj_pose(bowl).is_some_and(|(_, q)| (q * Vec3::Y).y > UPRIGHT)
    }

    /// What's in a bowl (not its grinder), in the order it went in.
    fn contents(&self, bowl: u32) -> Vec<u32> {
        let mut v: Vec<(f64, u32)> =
            self.objects.iter().filter(|(_, o)| matches!(o.holder, Holder::InBowl { bowl: b, grinder: false, .. } if b == bowl)).map(|(id, o)| (o.free_since, *id)).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// Into a bowl: out of the world's way (nothing meets it), laid on its floor with the rest.
    fn into_bowl(&mut self, id: u32, bowl: u32) {
        let Some(bq) = self.obj_pose(bowl).map(|p| p.1) else { return };
        let now = self.now;
        let Some(o) = self.objects.get_mut(&id) else { return };
        let (p, q) = self.physics.pose(o.body);
        o.holder = Holder::InBowl { bowl, at: Vec3::ZERO, rot: (bq.inverse() * q).normalize(), grinder: false };
        // (When it went in: its place in the order.)
        o.free_since = now;
        o.glide = Some((p, q, now));
        o.embed = None;
        let (body, collider) = (o.body, o.collider);
        self.physics.set_kinematic(body, true);
        self.physics.set_groups(collider, groups_none());
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        self.debug.into_bowls += 1;
        self.arrange(bowl);
    }

    /// Lays what's in a bowl on its floor: alone in the middle, else round it, each with its
    /// lowest point on the floor as it lies, shrunk to fit and as it's ground.
    fn arrange(&mut self, bowl: u32) {
        let Some(r) = self.bowls.get(&bowl) else { return };
        let b = Bowl::of(r);
        let inside = self.contents(bowl);
        let n = inside.len();
        for (k, id) in inside.into_iter().enumerate() {
            let o = self.objects.get_mut(&id).unwrap();
            let Holder::InBowl { rot, .. } = o.holder else { continue };
            let (centre, half) = (v3(o.desc.center), v3(o.desc.half_extents));
            o.fit = receptacle::fit(&b, half);
            let scale = receptacle::drawn_scale(o.fit, o.ground);
            // Its box as it lies: how far it reaches down from its middle.
            let down: f32 = [Vec3::X, Vec3::Y, Vec3::Z].iter().zip(half.to_array()).map(|(a, h)| (rot * *a).y.abs() * h).sum();
            let mid = rot * centre * scale;
            let place = b.place(k, n);
            let at = Vec3::new(place.x - mid.x, place.y + down * scale - mid.y, place.z - mid.z);
            o.holder = Holder::InBowl { bowl, at, rot, grinder: false };
            // What a hand reaching in meets of it: a ball as big as it's drawn, about its middle.
            let (c, r) = (o.collider, half.max_element() * scale);
            self.physics.set_ball(c, rot * centre * scale, r.max(0.008));
        }
    }

    /// Out of its bowl (taken, or loose): its own collision again (a ball as it's drawn while in).
    fn out_of_bowl(&mut self, id: u32) {
        let Some(o) = self.objects.get(&id).filter(|o| matches!(o.holder, Holder::InBowl { grinder: false, .. })) else { return };
        let (c, d) = (o.collider, o.desc);
        let hulls = self.game_hulls.get(&id).cloned().unwrap_or_default();
        self.physics.set_object_shape(c, &d, &hulls);
    }

    /// The bowl's grinder lies where it rests in it.
    fn rest_grinder(&mut self, id: u32, bowl: u32) {
        let Some(r) = self.bowls.get(&bowl) else { return };
        let (at, rot) = (v3(r.rest_pos), q4(r.rest_rot));
        let now = self.now;
        let Some(o) = self.objects.get_mut(&id) else { return };
        let (p, q) = self.physics.pose(o.body);
        o.holder = Holder::InBowl { bowl, at, rot, grinder: true };
        o.glide = o.handled.then_some((p, q, now));
        let (body, collider) = (o.body, o.collider);
        self.physics.set_kinematic(body, true);
        self.physics.set_groups(collider, groups_none());
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
    }

    /// Where a thing in a bowl is now: on the bowl where it lies, falling into its place at first.
    fn in_bowl_pose(&self, id: u32) -> Option<(Vec3, Quat)> {
        let o = self.objects.get(&id)?;
        let Holder::InBowl { bowl, at, rot, .. } = o.holder else { return None };
        let (bp, bq) = self.obj_pose(bowl)?;
        let to = (bp + bq * at * self.map.world_per_meter, (bq * rot).normalize());
        Some(match o.glide {
            Some((p, q, t0)) if self.now - t0 < BOWL_FALL => {
                let k = smooth((self.now - t0) / BOWL_FALL);
                (p.lerp(to.0, k), q.slerp(to.1, k))
            }
            _ => to,
        })
    }

    /// One step of the bowls, after the physics: what fell into one goes in, what's in one goes
    /// where the bowl is now, and the grinders grind.
    fn bowls(&mut self, dt: f32) {
        if self.bowls.is_empty() {
            return;
        }
        // Loose and falling into a bowl's mouth (let go over it from higher up, knocked in): in.
        let falling: Vec<(u32, Vec3)> = self
            .objects
            .iter()
            .filter(|(_, o)| o.holder == Holder::Free && (o.desc.flags & OBJF_GAME_SIMULATED == 0 || self.now - o.free_since <= GAME_HANDBACK as f64))
            .map(|(id, o)| (*id, self.physics.pose(o.body).0))
            .collect();
        for (id, at) in falling {
            if let Some(bowl) = self.bowl_for(id, at, 0.0, 0.0) {
                self.into_bowl(id, bowl);
            }
        }
        // A bowl turned over spills what's in it (and its grinder).
        let over: Vec<u32> = self.bowls.keys().copied().filter(|b| self.obj_pose(*b).is_some_and(|(_, q)| (q * Vec3::Y).y < SPILL)).collect();
        for b in over {
            let spilt: Vec<u32> = self.objects.iter().filter(|(_, o)| matches!(o.holder, Holder::InBowl { bowl, .. } if bowl == b)).map(|(id, _)| *id).collect();
            for id in spilt {
                self.free_body(id, groups_free());
                // (A grinder that fell out stays where it falls.)
                if let Some(o) = self.objects.get_mut(&id) {
                    o.handled = true;
                }
                self.debug.spills += 1;
            }
        }
        let inside: Vec<u32> = self.objects.iter().filter(|(_, o)| matches!(o.holder, Holder::InBowl { .. })).map(|(id, _)| *id).collect();
        for id in inside {
            match self.in_bowl_pose(id) {
                Some((p, q)) => {
                    let body = self.objects[&id].body;
                    self.physics.set_pose(body, p, q);
                    self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
                }
                None => self.free_body(id, groups_free()),
            }
        }
        // Grinding: the grinder's tip pressed low in its bowl and worked about.
        let grinding: Vec<Receptacle> = self.bowls.values().filter(|r| r.flags & REC_GRIND != 0 && r.grinder != 0).copied().collect();
        let wpm = self.map.world_per_meter;
        for r in grinding {
            let Some(g) = self.objects.get(&r.grinder) else { continue };
            let hand = if let Holder::Held(h) = g.holder { Some(h) } else { None };
            let (gp, gq) = self.physics.pose(g.body);
            let tip_world = gp + gq * v3(r.tip) * wpm;
            let tip = hand.and_then(|_| self.in_bowl_space(r.id, tip_world));
            let aimed = hand.and(g.aim).and_then(|(ap, aq)| self.in_bowl_space(r.id, ap + aq * v3(r.tip) * wpm));
            let (done, stroke) = self.grinds.entry(r.id).or_default().step(&Bowl::of(&r), tip, aimed, r.tip_radius, dt);
            let Some(hand) = hand.filter(|_| done > 0.0) else { continue };
            let inside = self.contents(r.id);
            let work = (r.grind_work.max(0.01) * inside.len() as f32).max(0.01);
            for id in &inside {
                let o = self.objects.get_mut(id).unwrap();
                o.ground = (o.ground + done / work).min(1.0);
            }
            if !inside.is_empty() {
                self.arrange(r.id);
            }
            let speed = self.grinds[&r.id].speed;
            if stroke {
                self.debug.grind_strokes += 1;
                self.emit(Cue::GrindStroke, Some(hand), Some(r.id), tip_world, 1.0);
            } else if self.now - self.grind_felt[hand] > GRIND_FEEL {
                self.grind_felt[hand] = self.now;
                self.emit(Cue::Grind, Some(hand), Some(r.id), tip_world, (speed / receptacle::MAX_SPEED).clamp(0.0, 1.0));
            }
        }
    }

    /// The fingertips on the sheets ([`sheet`]): one coming down on a face is felt in its hand, and
    /// lifting off again it's a touch.
    fn sheets(&mut self) {
        self.touches.retain(|id, _| self.objects.get(id).is_some_and(|o| o.desc.flags & OBJF_SHEET != 0));
        let Some(hands) = &self.hands else { return };
        let wpm = self.map.world_per_meter.max(1e-6);
        let mut felt = Vec::new();
        for (id, o) in self.objects.iter().filter(|(_, o)| o.desc.flags & OBJF_SHEET != 0) {
            let (p, q) = self.physics.pose(o.body);
            let (half, centre) = (v3(o.desc.half_extents), v3(o.desc.center));
            let t = self.touches.entry(*id).or_default();
            for (i, h) in hands.iter().enumerate() {
                // (A hand holding or working anything has its fingers on that, not free to touch.)
                let tip = h.fingers_on.then(|| q.inverse() * (h.index_tip - p) / wpm - centre);
                if t.step(i, tip, half) == sheet::Tap::Down {
                    felt.push((i, *id, h.index_tip));
                }
            }
        }
        for (hand, id, at) in felt {
            self.emit(Cue::Touch, Some(hand), Some(id), at, 1.0);
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
        self.out_of_bowl(id);
        let Some(o) = self.objects.get_mut(&id) else { return };
        o.holder = Holder::Free;
        o.free_since = self.now;
        o.blend = None;
        self.physics.set_kinematic(o.body, false);
        self.physics.set_gravity(o.body, 1.0);
        self.physics.set_angular_damping(o.body, 0.2);
        // (A sheet lets the hands through, whatever else is free.)
        let groups = if o.desc.flags & OBJF_SHEET != 0 { physics::groups_sheet() } else { groups };
        self.physics.set_groups(o.collider, groups);
    }

    fn template_for(&self, ammo_type: u32) -> Option<ObjectDesc> {
        self.templates.iter().find(|t| t.kind == OBJ_MAGAZINE && t.ammo_type == ammo_type).copied()
    }

    /// The arrows a bow of `ammo_type` shoots (their template).
    fn arrow_template(&self, ammo_type: u32) -> Option<ObjectDesc> {
        self.templates.iter().find(|t| t.kind == OBJ_ARROW && t.ammo_type == ammo_type).copied()
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
        self.pocket_events.clear();
        let dt = inp.dt.clamp(1e-4, 0.05);
        let s = inp.settings;
        let dom = s.dominant();
        self.ensure_hands(inp.frame);
        self.follow_actors();
        self.drags.clear();

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
                let target = (map.point(h.grip_t.0), map.rot(h.grip_t.1));
                let moved = (target.0 - h.target.0) / dt.max(1e-4);
                h.target_vel = if moved.length() < TARGET_JUMP * map.world_per_meter { moved } else { Vec3::ZERO };
                h.target = target;
                h.aim_rot = map.rot(h.aim_t.1);
                h.trigger = c.trigger;
                h.squeeze = c.squeeze;
                h.history.push(inp.now, h.grip_t.0, Some(h.vel_t), h.ang_t, map);
            }
        }

        // The body anchor (zones) follows the head, frozen while a hand reaches into a zone.
        let head_t = (v3(inp.frame.hmd.pos), q4(inp.frame.hmd.rot));
        let zones = slot_zones(inp.eye_height, dom);
        let ammo = ammo_zones(inp.eye_height, dom);
        // Each hand's quiver, while the other holds a bow: for a game that hands out its own things
        // from the body's slots, its arrows are in whichever slot they're kept in instead.
        let quivers = [0, 1].map(|i| self.other_bow(i).filter(|_| self.pockets.is_empty()).map(|_| zones::quiver_zones(inp.eye_height, i, s.archery.quiver)).unwrap_or_default());
        self.pocket_zones = self.pockets.iter().map(|p| pockets::zones(p, inp.eye_height)).collect();
        let near_zone = (0..2).any(|i| {
            let p = self.hands.as_ref().unwrap()[i].grip_t.0;
            zones.iter().chain(ammo.iter()).chain(quivers[i].iter()).chain(self.pocket_zones.iter().map(|z| &z.take)).any(|z| self.anchor.distance(z, p) < z.radius + 0.1)
        });
        self.anchor.update(head_t.0, head_t.1, inp.eye_height, dt, near_zone);
        self.place_by_hand(inp);
        self.loosed.clear();

        if !inp.paused {
            self.pocket_frame(inp, &zones);
            for i in 0..2 {
                self.update_hover(i, s, &zones, &ammo, &quivers[i]);
            }
            self.rest_on();
            for i in 0..2 {
                self.hand_input(i, inp, &zones, dom);
            }
            self.pocket_holds(inp);
            self.use_drawn(inp, dt);
            for i in 0..2 {
                if !self.page_hands[i] {
                    self.gloves(i, inp);
                }
            }
            self.weapons(inp, dom);
            self.bows(inp);
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
        self.bowls(dt);
        self.sheets();
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
            // The hand joint in the hand body's own space (the body is the controller's grip).
            let hand = self.offsets.hand_frame(side, rustvr_ik::Pose::IDENTITY);
            // As the drawn hand's are, from the controller; an empty hand's as what it's pressed
            // on pushes them open.
            let curls = if h.reach_from.is_some() { h.curls } else { h.drawn_curls };
            h.fingers_on = h.holding.is_none() && h.support.is_none() && h.handle.is_none() && h.barrels.is_none() && h.pin.is_none() && h.valve.is_none() && h.creature.is_none();
            // The fingers are the hand's own collider (curled as the controller has them): what
            // the hand meets, they meet too, and nothing goes through them.
            let local = fingers::segments((hand.pos, hand.rot), i == 1, curls);
            self.physics.set_hand_shape(h.collider, fingers::palm((hand.pos, hand.rot)), &local);
            let segs = local.map(|(a, b)| (gp + gq * a, gp + gq * b));
            for (k, (a, b)) in segs.into_iter().enumerate() {
                let (at, rot, half) = fingers::capsule_pose(a, b);
                let f = h.fingers[k];
                self.physics.set_capsule_half_height(f, half);
                self.physics.move_kinematic(f, at, rot);
                h.finger_vel[k] = (at - h.finger_at[k]) / dt;
                h.finger_at[k] = at;
            }
            h.index_tip = segs[1].1;
        }
    }

    /// The part a hand takes of the valves described by `desc` (the game's description id): `shape`,
    /// points of the part that turns in the wheel's frame (the model the runtime draws it with).
    pub fn set_valve_shape(&mut self, desc: u32, shape: &[Vec3]) {
        for o in self.objects.values_mut().filter(|o| o.desc.id == desc && o.valve.is_some()) {
            o.valve_shape = shape.to_vec();
        }
    }

    /// Whether a hand takes valve `o` anywhere on its collider (the game draws it, and its
    /// collider is the game's own shape of the part: a door's leaf), not on its rim or its model.
    fn taken_anywhere(o: &Obj) -> bool {
        o.valve_shape.is_empty() && (o.desc.shape == SHAPE_HULL || o.desc.joint_flags & JOINT_PANEL != 0)
    }

    /// Turns the valves: each by the hands on it (cranking it towards their controllers, or
    /// twisting it as far as their wrists have turned) and the empty hands pressed on it (a door's
    /// leaf) against its friction (`stiffness`: the player's setting), and puts it and those hands
    /// where it's turned to (a hand that's just taken hold on its way there over `transition`
    /// seconds). A hand whose controller is [`valve::BREAK`] from where it holds lets go. A part
    /// that swings through the room stops on the player's body (`body`: the capsule from the head
    /// down, world) and the game's creatures.
    fn valves(&mut self, dt: f32, transition: f32, stiffness: f32, body: (Vec3, Vec3, f32)) {
        let ids: Vec<u32> = self.objects.iter().filter(|(_, o)| o.valve.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            let Some(v) = self.objects[&id].valve else { continue };
            let (collider, panel) = (self.objects[&id].collider, self.objects[&id].desc.joint_flags & JOINT_PANEL != 0);
            // A wrist's turn about the axle since it took hold (unwrapped, so turning past half a
            // turn goes on rather than flipping round).
            let mut pulls = Vec::new();
            let mut grips: Vec<(usize, valve::ValveGrip)> = Vec::new();
            let mut broke = Vec::new();
            for (i, h) in self.hands.iter_mut().flatten().enumerate() {
                let Some(g) = h.valve.as_mut().filter(|g| g.id == id) else { continue };
                if h.target.0.distance(v.point(g.at)) > valve::BREAK * self.map.world_per_meter {
                    broke.push(i);
                    continue;
                }
                if v.twisted_at(g.at) {
                    let w = v.wrist(h.target.1);
                    let d = (w - g.wrist + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    g.twisted += d;
                    g.wrist = w;
                    pulls.push(valve::Pull::Twist { to: g.turn0 + g.twisted });
                } else {
                    // (The slack a hand that took it from further off had closes as it comes nearer.)
                    g.slack = g.slack.min(v.lead(g.at, h.target.0).abs()).max(valve::ARM_STRETCH * self.map.world_per_meter);
                    pulls.push(valve::Pull::Rim { at: g.at, toward: h.target.0, vel: h.target_vel, slack: g.slack });
                }
                grips.push((i, *g));
            }
            for i in broke {
                self.let_go_valve(i, transition);
                let at = self.physics.pose(self.hands.as_ref().unwrap()[i].body).0;
                self.emit(Cue::Release, Some(i), Some(id), at, 0.6);
            }
            // The empty hands pressed on it (resting on its surface, their controllers past it):
            // each pushes it there, as far as its controller is past the hand.
            if panel && let Some(hands) = &mut self.hands {
                for h in hands.iter_mut() {
                    if h.pressing == Some(id) {
                        h.pressing = None;
                    }
                    if h.holding.is_some() || h.support.is_some() || h.valve.is_some() || h.barrels.is_some() || h.pin.is_some() || h.returning.is_some() {
                        continue;
                    }
                    let Some((at, normal, gap)) = self.physics.touch(h.collider, collider, PRESS_GAP * self.map.world_per_meter) else { continue };
                    let depth = (h.target.0 - self.physics.pose(h.body).0).dot(normal) - gap.max(0.0);
                    if depth > 0.0 {
                        pulls.push(valve::Pull::Press { at: v.local(at), normal, depth: depth / self.map.world_per_meter, vel: h.target_vel.dot(normal) / self.map.world_per_meter });
                        h.pressing = Some(id);
                    }
                }
            }
            // One the game moves itself (a valve on the gate it winds, which the game may let down;
            // a door an NPC opens): with no hand turning it, where the game has it.
            let game = (grips.is_empty() && self.objects[&id].desc.flags & OBJF_GAME_MOVED != 0).then(|| self.game_poses.get(&id).copied()).flatten();
            let o = self.objects.get_mut(&id).unwrap();
            let v = o.valve.as_mut().unwrap();
            let before = *v;
            match game {
                Some((p, _)) if v.slide => v.follow_slide(p, dt),
                Some((_, q)) => v.follow(q, dt),
                None => v.step(&pulls, dt, stiffness),
            }
            // A door doesn't push its way through the player or a creature: it stops on them (if
            // it's in one already, it may move on out).
            if panel && game.is_none() && v.angle != before.angle {
                let parts: Vec<(Vec3, Vec3, f32)> = std::iter::once(body).chain(self.actor_parts.values().copied()).collect();
                let hits = |at: &valve::Valve| parts.iter().any(|&(a, b, r)| self.physics.hits_capsule(collider, (at.origin(), at.rot()), a, b, r));
                if hits(v) && !hits(&before) {
                    v.angle = before.angle;
                    v.vel = 0.0;
                }
            }
            let (v, body_h) = (*v, o.body);
            // Coming hard against a stop is felt in the hands on it.
            let stopped = v.limited() && v.vel == 0.0 && before.vel.abs() > 0.2 && (v.angle <= v.min || v.angle >= v.max);
            self.physics.move_kinematic(body_h, v.origin(), v.rot());
            for &(i, g) in &grips {
                let h = &self.hands.as_ref().unwrap()[i];
                let b = h.body;
                let (p, q) = (v.point(g.at), (v.rot() * g.rot).normalize());
                let time = travel_time(transition, g.from.0.distance(p));
                let k = if time > 1e-3 { smooth((self.now - g.since) / time as f64) } else { 1.0 };
                self.physics.set_pose(b, g.from.0.lerp(p, k), g.from.1.slerp(q, k).normalize());
                self.physics.set_velocity(b, Vec3::ZERO, Vec3::ZERO);
            }
            if stopped {
                let speed = before.vel.abs() * if v.slide { 1.0 } else { v.radius.max(0.1) };
                for &(i, g) in &grips {
                    self.emit(Cue::Stop, Some(i), Some(id), v.point(g.at), (speed / 2.0).min(1.0));
                }
            }
        }
    }

    /// Hand `i` lets go of the valve it's on, and goes back to its controller from there, at a
    /// hand's pace (over `transition` seconds for a short way), through whatever's between.
    fn let_go_valve(&mut self, i: usize, transition: f32) {
        let h = &mut self.hands.as_mut().unwrap()[i];
        if h.valve.take().is_none() {
            return;
        }
        let (b, target) = (h.body, h.target.0);
        let from = self.physics.pose(b);
        let time = travel_time(transition, from.0.distance(target));
        self.hands.as_mut().unwrap()[i].returning = Some((from.0, from.1, self.now, time));
    }

    /// The palms and fingers in the game's own free props, as pushes for the game: at each prop,
    /// the deepest touch, asking the point to move along with the hand at least as fast as the
    /// hand moves (plus enough to undo the overlap within a few frames).
    fn pushes(&self) -> Vec<GamePush> {
        let Some(hands) = &self.hands else { return Vec::new() };
        let mut out = self.actor_pushes();
        for (id, o) in &self.objects {
            let constrained = o.desc.flags & OBJF_CONSTRAINED != 0;
            let game_free = o.desc.flags & OBJF_GAME_SIMULATED != 0 && o.holder == Holder::Free && (constrained || self.now - o.free_since > GAME_HANDBACK as f64);
            if !game_free {
                continue;
            }
            let mut best: Option<(f32, Vec3, Vec3, f32)> = None;
            // Solid to the hands (on the game's constraints): each empty hand resting on it pushes
            // it as fast as its controller goes on into it, and more the deeper its controller is.
            if constrained {
                for h in hands.iter().filter(|h| h.holding.is_none() && h.support.is_none() && h.valve.is_none()) {
                    let Some((point, normal, gap)) = self.physics.touch(h.collider, o.collider, PRESS_GAP * self.map.world_per_meter) else { continue };
                    let depth = (h.target.0 - self.physics.pose(h.body).0).dot(normal) - gap.max(0.0);
                    if depth > 0.0 && best.is_none_or(|b| depth > b.0) {
                        best = Some((depth, point, normal, (h.target_vel.dot(normal).max(0.0) + depth * PUSH_OUT).min(PUSH_MAX)));
                    }
                }
            }
            for h in hands.iter().filter(|_| !constrained) {
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

    /// The game's creatures' parts as kinematic capsules where the game has them now (made as they
    /// appear, gone when they go): what the hands and what they hold meet.
    fn follow_actors(&mut self) {
        let gone: Vec<(u32, usize)> = self.proxies.keys().filter(|k| !self.actor_parts.contains_key(k)).copied().collect();
        for k in gone {
            if let Some((b, ..)) = self.proxies.remove(&k) {
                self.physics.remove(b);
            }
        }
        for (&k, &(a, b, radius)) in &self.actor_parts {
            let half = 0.5 * a.distance(b);
            let (mid, rot) = ((a + b) * 0.5, Quat::from_rotation_arc(Vec3::Y, (b - a).normalize_or(Vec3::Y)));
            match self.proxies.get(&k) {
                Some(&(body, _, h, r)) if (h - half).abs() < 0.005 && (r - radius).abs() < 0.002 => self.physics.move_kinematic(body, mid, rot),
                old => {
                    if let Some(&(body, ..)) = old {
                        self.physics.remove(body);
                    }
                    let (body, c) = self.physics.add_actor_part(mid, rot, half, radius);
                    self.proxies.insert(k, (body, c, half, radius));
                }
            }
        }
    }

    /// What the hands, what they hold and loose things push the game's creatures by: each part
    /// something is pressed into, at the contact, the way it presses (its own speed into the part,
    /// and what the hand would have it go further), and the blades stuck in them, dragged. The
    /// hardest for each part.
    fn actor_pushes(&self) -> Vec<GamePush> {
        let Some(hands) = &self.hands else { return Vec::new() };
        let mut best: HashMap<(u32, u32), GamePush> = HashMap::new();
        let mut keep = |g: GamePush| {
            let e = best.entry((g.id, g.part)).or_insert(g);
            if g.normal[3] > e.normal[3] {
                *e = g;
            }
        };
        let quiet: [Option<u32>; 2] = [0, 1].map(|i| self.in_pocket(i).map(|(actor, _)| actor));
        for (&(actor, part), &(_, proxy, ..)) in &self.proxies {
            for c in self.physics.touching(proxy) {
                // A hand in one of its pockets is light: it pushes nothing of it.
                if hands.iter().zip(quiet).any(|(h, q)| h.collider == c && q == Some(actor)) {
                    continue;
                }
                // Who's pressing, and how far behind where it's driven it is (what it presses with).
                // (Only an open hand shoves: one closing is taking hold, a fist punches, the game's
                // own blow.)
                let pusher = hands.iter().find(|h| h.collider == c).map(|h| (h.body, h.target.0 - self.physics.pose(h.body).0, Some(if h.squeeze < OPEN_HAND { self.map.vel(h.vel_t) } else { Vec3::ZERO }))).or_else(|| {
                    self.objects.values().find(|o| o.collider == c).map(|o| (o.body, if matches!(o.holder, Holder::Held(_)) { o.lag } else { Vec3::ZERO }, None))
                });
                let Some((body, lag, controller)) = pusher else { continue };
                let Some((point, normal, depth)) = self.physics.overlap(c, proxy).or_else(|| {
                    // Resting against it: the nearest points.
                    let p = self.physics.closest_point(proxy, self.physics.pose(body).0);
                    let n = (p - self.physics.pose(body).0).normalize_or_zero();
                    (n != Vec3::ZERO).then_some((p, n, 0.0))
                }) else {
                    continue;
                };
                let vel = self.physics.velocity(body).0;
                let speed = (vel.dot(normal).max(0.0) + lag.dot(normal).max(0.0) * PUSH_LAG + depth * PUSH_OUT).min(PUSH_MAX);
                // (A hand stopped on the body has lost its speed: its controller's is how hard it
                // came.)
                let moving = controller.unwrap_or(vel).dot(normal).max(0.0);
                if speed > 0.05 {
                    keep(GamePush { id: actor, part: part as u32 + 1, moving, point: point.extend(1.0).to_array(), normal: normal.extend(speed).to_array(), ..Default::default() });
                }
            }
        }
        for g in &self.drags {
            keep(*g);
        }
        best.into_values().collect()
    }

    fn ensure_hands(&mut self, f: &HmdFrame) {
        if self.hands.is_some() {
            return;
        }
        let palms = [rustvr_ik::Side::Left, rustvr_ik::Side::Right].map(|side| {
            let hand = self.offsets.hand_frame(side, rustvr_ik::Pose::IDENTITY);
            fingers::palm((hand.pos, hand.rot))
        });
        let mk = |p: &mut Physics, i: usize| {
            let g = &f.hands[i].grip;
            let (body, collider) = p.add_hand(i, v3(g.pos), q4(g.rot), palms[i]);
            let fingers = std::array::from_fn(|_| p.add_finger(0.02));
            Hand {
                body,
                collider,
                fingers,
                finger_at: [v3(g.pos); 5],
                finger_vel: [Vec3::ZERO; 5],
                index_tip: v3(g.pos),
                finger_solver: rustvr_ik::FingerSolver::new(),
                drawn_curls: [0.0; 5],
                fingers_on: false,
                grip_down: false,
                grip_toggled: false,
                prev_buttons: 0,
                trigger: 0.0,
                squeeze: 0.0,
                holding: None,
                on: On::Free,
                hold: None,
                was_on: None,
                passing: None,
                reach_from: None,
                rests_on: None,
                round: Round::No,
                pinned: Vec::new(),
                tilt: Quat::IDENTITY,
                tilt_to: Quat::IDENTITY,
                give: [0.0; 5],
                curls: [0.0; 5],
                held_curls: [0.0; 5],
                open: [0.0; 5],
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
                creature: None,
                turn_rate: Vec3::ZERO,
                trigger_was: 0.0,
                squeeze_peak: 0.0,
                eased: false,
                euro: OneEuro::new(1.0, 8.0, 1.0),
                blocked: false,
                target: (Vec3::ZERO, Quat::IDENTITY),
                target_vel: Vec3::ZERO,
                aim_rot: Quat::IDENTITY,
                grip_t: (Vec3::ZERO, Quat::IDENTITY),
                aim_t: (Vec3::ZERO, Quat::IDENTITY),
                vel_t: Vec3::ZERO,
                ang_t: Vec3::ZERO,
                leashed_until: 0.0,
                returning: None,
                grasp: GraspState::default(),
                yielding: false,
                pressing: None,
                pocket_press: None,
                place_grip: false,
                pocket: None,
                heartbeat: 0.0,
                aware: 0.0,
                felt: [0; 4],
                felt_since: 0.0,
                let_go: false,
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

    /// The bow the other hand holds by its grip, if any (held anywhere else it can't be drawn).
    fn other_bow(&self, hand: usize) -> Option<u32> {
        let h = &self.hands.as_ref()?[hand ^ 1];
        let id = h.holding.filter(|_| h.on == On::Grip)?;
        (self.objects.get(&id)?.desc.kind == OBJ_BOW).then_some(id)
    }

    // ---- the game's creatures' pockets ([`pickpocket`]) -------------------------------------------------
    /// The game's creatures' pockets now (world space) and its answers to what the hands did with
    /// them; `next`: the number the next pocket event will have in the ring (the session pushes this
    /// frame's [`Rig::pickpocket_events`] in order after the update).
    pub fn set_creature_pockets(&mut self, table: &Pockets, next: u64) {
        let creatures = |p: &[Pocket]| p.iter().map(|p| p.actor).collect::<std::collections::BTreeSet<u32>>();
        if creatures(table.pockets()) != creatures(&self.creature_pockets) {
            tracing::info!("rig: {} pockets on {:x?}", table.pockets().len(), creatures(table.pockets()));
        }
        self.creature_pockets.clear();
        self.creature_pockets.extend_from_slice(table.pockets());
        self.pocket_answers.clear();
        self.pocket_answers.extend_from_slice(table.outcomes());
        self.pocket_next = next;
        self.pickpocket_events.clear();
    }

    /// The pocket hand `i` is in (its creature and its id), its fingers on its things or holding one
    /// still in it: what it does there is the pocket's, not a grab of the creature's body.
    pub fn in_pocket(&self, i: usize) -> Option<(u32, u32)> {
        let h = &self.hands.as_ref()?[i];
        if let Some(p) = h.pocket {
            return Some((p.actor, p.pocket));
        }
        match self.objects.get(&h.holding?)?.pocketed? {
            Pocketed::Inside { pocket, actor, .. } => Some((actor, pocket)),
            _ => None,
        }
    }

    /// What hand `i` is on in the pockets: the creature, the pocket, the thing its fingers are on and
    /// that thing's template (0s where there's none: out of the pockets, holding a thing, or in a
    /// pocket with nothing to feel).
    fn felt(&self, i: usize) -> [u32; 4] {
        let Some((actor, pocket)) = self.in_pocket(i) else { return [0; 4] };
        let h = &self.hands.as_ref().unwrap()[i];
        let open = self.pocket(pocket).filter(|pk| pk.flags & POCKET_SHUT == 0);
        let thing = h.pocket.filter(|_| h.holding.is_none()).zip(open).and_then(|(p, pk)| pk.items().get(p.index).copied());
        thing.map_or([actor, pocket, 0, 0], |t| [actor, pocket, t.id, t.object])
    }

    /// The thing hand `i`'s fingers are on in a pocket (its template) and where the hand is (world),
    /// for the card of it.
    pub fn feeling(&self, i: usize) -> Option<(u32, Vec3)> {
        let h = &self.hands.as_ref()?[i];
        (h.felt[3] != 0).then(|| (h.felt[3], self.physics.pose(h.body).0))
    }

    /// The hand pulling object `id` to it (the gloves), if one is.
    pub fn puller(&self, id: u32) -> Option<usize> {
        match self.objects.get(&id)?.holder {
            Holder::Pulled { hand, .. } => Some(hand),
            _ => None,
        }
    }

    /// How far hand `i` is from a point: from the nearest of it, its palm or a finger.
    fn hand_reach(&self, i: usize) -> impl Fn(Vec3) -> f32 + '_ {
        let c = self.hands.as_ref().map(|h| h[i].collider);
        move |p: Vec3| c.map_or(f32::MAX, |c| self.physics.closest_point(c, p).distance(p))
    }

    /// The pocket nearest hand `i` and where its middle is from the nearest of the hand (world
    /// metres), for the log.
    pub fn nearest_pocket(&self, i: usize) -> Option<(u32, Vec3)> {
        let c = self.hands.as_ref()?[i].collider;
        let from = |p: &Pocket| Vec3::from_array(p.center) - self.physics.closest_point(c, Vec3::from_array(p.center));
        self.creature_pockets.iter().map(|p| (p.id, from(p))).min_by(|a, b| a.1.length().total_cmp(&b.1.length()))
    }

    fn pocket(&self, id: u32) -> Option<&Pocket> {
        self.creature_pockets.iter().find(|p| p.id == id)
    }

    /// An event for the game: its number in the ring.
    fn pocket_event(&mut self, ev: rustvr_protocol::PocketEvent) -> u64 {
        let n = self.pocket_next + self.pickpocket_events.len() as u64;
        self.pickpocket_events.push(ev);
        n
    }

    /// The event for something done with object `id` by hand `i` (where it is, how fast).
    fn pocket_event_of(&self, kind: u32, actor: u32, pocket: u32, item: u32, id: u32, i: usize) -> rustvr_protocol::PocketEvent {
        let (pos, rot) = self.obj_pose(id).unwrap_or((Vec3::ZERO, Quat::IDENTITY));
        let vel = self.objects.get(&id).map_or(Vec3::ZERO, |o| self.physics.velocity(o.body).0);
        let speed = self.hands.as_ref().map_or(0.0, |h| h[i].history.recent().length());
        rustvr_protocol::PocketEvent { kind, actor, pocket, item, object: id, hand: i as u32, pos: pos.to_array(), speed, rot: rot.to_array(), vel: vel.to_array(), ..Default::default() }
    }

    /// A copy of template `t` (a thing in a pocket), at `pos`.
    fn spawn_copy(&mut self, t: &ObjectDesc, pos: Vec3, rot: Quat) -> u32 {
        let id = self.next_spawn;
        self.next_spawn = self.next_spawn.wrapping_add(1).max(SPAWN_BASE);
        let mut d = *t;
        d.id = id;
        d.flags &= !OBJF_TEMPLATE;
        self.add(d, t.id, true, pos, rot);
        id
    }

    /// A copy of a pocket's thing goes from wherever it is (a hand holding it lets go of nothing).
    fn drop_copy(&mut self, id: u32) {
        if let Some(hands) = &mut self.hands {
            for (i, h) in hands.iter_mut().enumerate() {
                if h.holding == Some(id) {
                    h.holding = None;
                    h.let_go = false;
                    self.physics.set_groups(h.collider, groups_hand(i));
                }
            }
        }
        self.remove(id);
    }

    /// Once a frame, before the hands' input: the game's answers; the hands in the creatures'
    /// pockets, feeling what's there and a thing held in one coming out of it (the attempt); the
    /// creatures' pickpocketing opening as a hand goes in and closing once the hands are away.
    fn pocket_frame(&mut self, inp: &RigInput, zones: &[Zone; SLOT_COUNT_BODY]) {
        self.pocket_answers();
        let scale = self.map.world_per_meter;
        let now = self.now;
        for i in 0..2 {
            let h = &self.hands.as_ref().unwrap()[i];
            let palm = self.physics.pose(h.body).0;
            let busy = h.support.is_some() || h.handle.is_some() || h.pin.is_some() || h.barrels.is_some() || h.valve.is_some();
            let (holding, before, let_go) = (h.holding, h.pocket, h.let_go);
            if let Some(id) = holding {
                self.hands.as_mut().unwrap()[i].pocket = None;
                if let Some(Pocketed::Inside { pocket, actor, item }) = self.objects.get(&id).and_then(|o| o.pocketed) {
                    match self.pocket(pocket).copied() {
                        // Its pocket's gone (the creature out of reach, the player not sneaking):
                        // it's back where it was, nothing tried.
                        None => {
                            tracing::info!("rig: pocket {pocket:#x} went with {id:#x} still in it: put back");
                            self.drop_copy(id);
                        }
                        // Pulled out: the attempt, for the game to decide (its pickpocketing open
                        // first, if the look was quick).
                        Some(p) if pickpocket::out_of(&p, self.hand_reach(i), scale) => {
                            self.open_pocket_session(&p, i, palm);
                            let ev = self.pocket_event_of(PE_TAKE, actor, pocket, item, id, i);
                            let n = self.pocket_event(ev);
                            let o = self.objects.get_mut(&id).unwrap();
                            o.pocketed = Some(Pocketed::Deciding { event: n, since: now });
                            let c = o.collider;
                            self.physics.set_groups(c, groups_held(i));
                            tracing::info!("rig: {id:#x} (thing {item:#x}) pulled out of pocket {pocket:#x} of {actor:#x}: event {n}");
                            self.emit(Cue::PickpocketOut, Some(i), Some(id), palm, 1.0);
                        }
                        _ => {}
                    }
                }
                // Let go while the game decided: now that it has, it's let go of.
                let deciding = self.objects.get(&id).is_some_and(|o| matches!(o.pocketed, Some(Pocketed::Deciding { .. })));
                if let_go && !deciding && self.hands.as_ref().unwrap()[i].holding == Some(id) {
                    self.hands.as_mut().unwrap()[i].let_go = false;
                    self.release(i, inp, zones);
                }
                // A thing to plant brought into a pocket: a brush (the creature's pickpocketing
                // opens as it's let go there, or after a moment there, as for an empty hand).
                let plantable = self.objects.get(&id).is_some_and(|o| o.desc.flags & OBJF_LOOT != 0 || o.pocketed == Some(Pocketed::Taken));
                let into = plantable.then(|| pickpocket::pocket_at(&self.creature_pockets, self.hand_reach(i), before.map(|p| p.pocket), scale)).flatten().and_then(|p| self.pocket(p).copied());
                if let Some(pk) = into.filter(|_| self.hands.as_ref().unwrap()[i].holding == Some(id)) {
                    if before.is_none_or(|b| b.pocket != pk.id) {
                        self.emit(Cue::PickpocketIn, Some(i), None, palm, 0.6);
                    }
                    self.hands.as_mut().unwrap()[i].pocket = Some(InPocket { pocket: pk.id, actor: pk.actor, index: 0, moved: 0.0, last: palm });
                }
            } else if busy {
                self.hands.as_mut().unwrap()[i].pocket = None;
            } else {
                let in_now = pickpocket::pocket_at(&self.creature_pockets, self.hand_reach(i), before.map(|p| p.pocket), scale);
                match (before, in_now.and_then(|id| self.pocket(id).copied())) {
                    (Some(mut p), Some(pk)) if p.pocket == pk.id => {
                        // On along it: the next thing, felt.
                        if let Some(k) = p.feel_along(palm, pk.items().len(), scale).filter(|_| pk.flags & POCKET_SHUT == 0) {
                            self.emit(Cue::PocketFeel(pk.items[k].feel), Some(i), None, palm, 1.0);
                        }
                        self.hands.as_mut().unwrap()[i].pocket = Some(p);
                    }
                    (_, Some(pk)) => {
                        // In: a brush, and the first thing (or nothing, or a refusal: the game's,
                        // at once). Its pickpocketing opens after a moment of this.
                        self.hands.as_mut().unwrap()[i].pocket = Some(InPocket { pocket: pk.id, actor: pk.actor, index: 0, moved: 0.0, last: palm });
                        self.emit(Cue::PickpocketIn, Some(i), None, palm, 0.6);
                        if pk.flags & POCKET_SHUT != 0 {
                            self.emit(Cue::PocketShut, Some(i), None, palm, 1.0);
                            self.open_pocket_session(&pk, i, palm);
                        } else if let Some(first) = pk.items().first() {
                            self.emit(Cue::PocketFeel(first.feel), Some(i), None, palm, 1.0);
                        } else {
                            self.emit(Cue::PocketEmpty, Some(i), None, palm, 1.0);
                        }
                    }
                    _ => self.hands.as_mut().unwrap()[i].pocket = None,
                }
            }
            // Its creature sees the player: a slow heartbeat in the hand in its pocket, strong as it
            // first notices them.
            let aware = self.in_pocket(i).and_then(|(_, p)| self.pocket(p)).map_or(0.0, |p| p.awareness);
            let h = &self.hands.as_ref().unwrap()[i];
            let (was, next) = (h.aware, h.heartbeat);
            if aware >= pickpocket::WATCHED && (was < pickpocket::WATCHED || now >= next) {
                self.emit(Cue::PocketWatched, Some(i), None, palm, if was < pickpocket::WATCHED { 1.0 } else { 0.4 });
                self.hands.as_mut().unwrap()[i].heartbeat = now + pickpocket::HEARTBEAT;
            }
            self.hands.as_mut().unwrap()[i].aware = aware;
            // What it's on, told to the game as it changes: the game shows its card of the thing the
            // fingers are on, before they close on it.
            let felt = self.felt(i);
            if felt != self.hands.as_ref().unwrap()[i].felt {
                let [actor, pocket, item, object] = felt;
                self.pocket_event(rustvr_protocol::PocketEvent { kind: PE_FEEL, actor, pocket, item, object, hand: i as u32, pos: palm.to_array(), ..Default::default() });
                let h = &mut self.hands.as_mut().unwrap()[i];
                h.felt = felt;
                h.felt_since = now;
            }
        }
        // A moment feeling about in a creature's pockets opens its pickpocketing (the game's menu:
        // closed having taken nothing, its rummaging check); a brush past them is only feeling.
        for i in 0..2 {
            let Some((actor, pocket)) = self.in_pocket(i).filter(|(a, _)| !self.pocket_sessions.contains_key(a)) else { continue };
            let since = {
                let look = self.pocket_looks.entry(actor).or_insert((now, now));
                look.1 = now;
                look.0
            };
            if now - since >= pickpocket::OPEN_AFTER
                && let Some(pk) = self.pocket(pocket).copied()
            {
                let palm = self.physics.pose(self.hands.as_ref().unwrap()[i].body).0;
                self.open_pocket_session(&pk, i, palm);
            }
        }
        let sessions = &self.pocket_sessions;
        self.pocket_looks.retain(|actor, (_, last)| !sessions.contains_key(actor) && now - *last <= pickpocket::SESSION_GAP);
        // The creatures' pickpocketing closes once no hand has been near their pockets a moment.
        let near: Vec<u32> = self
            .pocket_sessions
            .keys()
            .copied()
            .filter(|&actor| (0..2).any(|i| pickpocket::near_actor(&self.creature_pockets, actor, self.hand_reach(i), scale) || self.in_pocket(i).is_some_and(|(a, _)| a == actor)))
            .collect();
        let mut closed = Vec::new();
        for (actor, last) in self.pocket_sessions.iter_mut() {
            if near.contains(actor) {
                *last = now;
            } else if now - *last > pickpocket::SESSION_GAP {
                closed.push(*actor);
            }
        }
        for actor in closed {
            self.pocket_sessions.remove(&actor);
            let n = self.pocket_event(rustvr_protocol::PocketEvent { kind: PE_CLOSE, actor, ..Default::default() });
            tracing::info!("rig: the hands are away from {actor:#x}'s pockets: its pickpocketing closes (event {n})");
        }
    }

    /// A hand (`i`, at `palm`) in pocket `pk` a moment, taking or planting a thing there, or finding
    /// it shut: its creature's pickpocketing opens, unless it's open.
    fn open_pocket_session(&mut self, pk: &Pocket, i: usize, palm: Vec3) {
        if self.pocket_sessions.contains_key(&pk.actor) {
            return;
        }
        self.pocket_sessions.insert(pk.actor, self.now);
        let ev = rustvr_protocol::PocketEvent { kind: PE_OPEN, actor: pk.actor, pocket: pk.id, hand: i as u32, pos: palm.to_array(), ..Default::default() };
        let n = self.pocket_event(ev);
        tracing::info!("rig: the {} hand in pocket {:#x} of {:#x} ({} things{}): its pickpocketing opens (event {n})", if i == 0 { "left" } else { "right" }, pk.id, pk.actor, pk.count, if pk.flags & POCKET_SHUT != 0 { ", shut" } else { "" });
    }

    /// The game's answers: a thing pulled out is the player's or goes, one dropped is the game's;
    /// unanswered too long, as if it went (a take) or was done (a drop).
    fn pocket_answers(&mut self) {
        let now = self.now;
        let mut gone = Vec::new();
        for (&id, o) in self.objects.iter_mut() {
            let (event, since) = match o.pocketed {
                Some(Pocketed::Deciding { event, since } | Pocketed::Dropping { event, since }) => (event, since),
                _ => continue,
            };
            let answer = self.pocket_answers.iter().find(|a| a.event == event + 1 && a.object == id).map(|a| a.outcome);
            match (o.pocketed, answer) {
                (Some(Pocketed::Deciding { .. }), Some(PO_TAKEN)) => {
                    o.pocketed = Some(Pocketed::Taken);
                    tracing::info!("rig: the game let {id:#x} be taken (event {event})");
                }
                (Some(Pocketed::Deciding { .. }), Some(_)) => {
                    let hand = match o.holder {
                        Holder::Held(h) => Some(h),
                        _ => None,
                    };
                    gone.push((id, hand, true));
                    tracing::info!("rig: {id:#x} wasn't taken unnoticed (event {event}): it stays where it was");
                }
                (Some(Pocketed::Dropping { .. }), Some(_)) => gone.push((id, None, false)),
                (_, None) if now - since > pickpocket::ANSWER_WAIT => {
                    tracing::warn!("rig: no answer from the game for {id:#x} (event {event}) in {:.0} s: it goes", pickpocket::ANSWER_WAIT);
                    gone.push((id, None, false));
                }
                _ => {}
            }
        }
        for (id, hand, caught) in gone {
            let at = self.obj_pose(id).map_or(Vec3::ZERO, |p| p.0);
            self.drop_copy(id);
            if caught {
                self.emit(Cue::PocketCaught, hand, None, at, 1.0);
            }
        }
    }

    /// Hand `i` closes on the thing it's on in pocket `pid`: a copy of it in the hand, still its
    /// creature's while it's in the pocket.
    fn take_from_pocket(&mut self, i: usize, pid: u32) {
        let Some(p) = self.pocket(pid).copied() else { return };
        let (palm, rot) = self.hands.as_ref().unwrap()[i].target;
        if p.flags & POCKET_SHUT != 0 || p.items().is_empty() {
            self.emit(if p.flags & POCKET_SHUT != 0 { Cue::PocketShut } else { Cue::PocketEmpty }, Some(i), None, palm, 1.0);
            return;
        }
        let k = self.hands.as_ref().unwrap()[i].pocket.map_or(0, |x| x.index).min(p.items().len() - 1);
        let item = p.items[k];
        let Some(t) = self.templates.iter().find(|t| t.id == item.object).copied() else {
            tracing::warn!("rig: pocket {pid:#x}'s thing {:#x} has no template {:#x}", item.id, item.object);
            self.emit(Cue::Fail, Some(i), None, palm, 1.0);
            return;
        };
        let id = self.spawn_copy(&t, palm, rot);
        self.take(id, i, true);
        let o = self.objects.get_mut(&id).unwrap();
        o.pocketed = Some(Pocketed::Inside { pocket: pid, actor: p.actor, item: item.id });
        // In the pocket it meets nothing (it's in among the creature's things).
        let c = o.collider;
        self.physics.set_groups(c, groups_none());
        self.hands.as_mut().unwrap()[i].pocket = None;
        tracing::info!("rig: the {} hand closes on thing {:#x} ({k} of {}) in pocket {pid:#x} of {:#x}: {id:#x}", if i == 0 { "left" } else { "right" }, item.id, p.count, p.actor);
    }

    // ---- what an empty hand would grab ------------------------------------------------------------
    fn update_hover(&mut self, i: usize, s: &Settings, zones: &[Zone; SLOT_COUNT_BODY], ammo: &[Zone; 2], quiver: &[Zone]) {
        if self.page_hands[i] {
            // (Its grip is on a held book's pages: it takes nothing, not even the book.)
            if self.hands.as_ref().unwrap()[i].glove != Glove::Idle {
                self.set_glove(i, Glove::Idle);
            }
            self.hands.as_mut().unwrap()[i].hover = Hover::None;
            return;
        }
        let hands = self.hands.as_ref().unwrap();
        let h = &hands[i];
        if h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.pin.is_some() || h.barrels.is_some() || h.valve.is_some() || h.creature.is_some() {
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
        // The other hand's bow: the arrow resting on its string, by its nock (to draw it), or
        // anywhere else on it (to take it off).
        if let (Hover::None, Some(bid)) = (hover, self.other_bow(i)) {
            let b = &self.objects[&bid];
            if let Some(st) = b.bow.filter(|st| st.drawing.is_none())
                && let Some(arrow) = st.nocked.and_then(|a| self.objects.get(&a))
            {
                let (bp, bq) = self.physics.pose(b.body);
                let at = bp + bq * bow::nock(&b.desc, st.draw);
                if self.pinch(i).distance(at) < bow::NOCK_REACH * self.map.world_per_meter {
                    hover = Hover::String(bid);
                } else if self.physics.distance_to(arrow.collider, palm) < ON_IT * self.map.world_per_meter {
                    hover = Hover::Object(st.nocked.unwrap());
                }
            }
        }
        // 1. The other hand's gun: its charging handle, then its handguard (anywhere along it).
        if let (Hover::None, Some(wid)) = (hover, self.other_weapon(i)) {
            let o = &self.objects[&wid];
            let (wp, wr) = self.physics.pose(o.body);
            let handle = wp + wr * self.handle_now(wid);
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
            } else if open.is_none() && self.handle_hold(i, wid, On::Foregrip).is_some() {
                hover = Hover::Foregrip(wid);
            }
        }
        // What the other hand holds, at its grip when the other hand isn't on it (taking a gun held
        // anywhere by its pistol grip), or with the hand on it: both hands hold it (before the
        // pouches and holsters a gun held at the chest is in front of).
        if hover == Hover::None
            && let Some(id) = self.hands.as_ref().unwrap()[i ^ 1].holding
            && let Some(o) = self.objects.get(&id)
            && o.desc.flags & OBJF_GRABBABLE != 0
        {
            let at_grip = self.hands.as_ref().unwrap()[i ^ 1].on != On::Grip && self.handle_hold(i, id, On::Grip).is_some();
            let reach = if o.desc.flags & OBJF_OPENS != 0 { ON_COVER } else { ON_IT };
            if at_grip || self.physics.distance_to(o.collider, palm) < reach * self.map.world_per_meter {
                hover = Hover::Object(id);
            }
        }
        // A pocket of one of the game's creatures the hand's in: its things come before the
        // player's own holsters behind it, and before anything of the creature's body.
        if hover == Hover::None
            && let Some(p) = h.pocket
        {
            hover = Hover::CreaturePocket(p.pocket);
        }
        // 2. Holsters and the slots that hand out the player's things, then the ammo pouch (it beats
        // nearby objects when the other hand has a gun). A hand at a slot's middle takes from it,
        // holsters first (a hip with a sword in it gives the sword); one only deeper, toward the
        // body, from the slot whose reach it's furthest into (pressed against the belly beside the
        // potions, the potions, not the sword at the hip).
        if hover == Hover::None {
            let anchor = &self.anchor;
            let holsters = zones.iter().enumerate().filter(|(slot, _)| self.slots[*slot].is_some()).map(|(slot, z)| (Hover::Slot(slot as u32), z));
            let pockets = self.pocket_zones.iter().enumerate().map(|(k, z)| (Hover::Pocket(k), &z.take));
            let all: Vec<(Hover, &Zone)> = holsters.chain(pockets).collect();
            let deepest = || {
                all.iter()
                    .filter(|(_, z)| anchor.contains(z, palm_t))
                    .min_by(|a, b| (anchor.distance(a.1, palm_t) / a.1.radius).total_cmp(&(anchor.distance(b.1, palm_t) / b.1.radius)))
            };
            if let Some((h, _)) = all.iter().find(|(_, z)| anchor.at_middle(z, palm_t)).or_else(deepest) {
                hover = *h;
            }
        }
        // Arrows from the quiver whenever the other hand holds a bow by its grip.
        if hover == Hover::None
            && let Some(bid) = self.other_bow(i)
            && quiver.iter().any(|z| self.anchor.contains(z, palm_t))
        {
            hover = Hover::Quiver(bid);
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
        // What the other hand holds, the hand on it: both hands hold it (and once the other lets go,
        // this one does: from hand to hand).
        let hand_c = self.hands.as_ref().unwrap()[i].collider;
        if hover == Hover::None
            && let Some(id) = self.hands.as_ref().unwrap()[i ^ 1].holding
            && self.objects.get(&id).is_some_and(|o| o.desc.flags & OBJF_GRABBABLE != 0 && self.physics.distance_between(hand_c, o.collider) < TOUCH)
        {
            hover = Hover::Object(id);
        }
        // A valve's rim within reach; a part of one the game draws (a door's leaf, the game's own
        // shape of it) anywhere the hand is on it, as a thing is taken.
        if hover == Hover::None {
            let pressing = self.hands.as_ref().unwrap()[i].pressing;
            let near = |(id, o): (&u32, &Obj)| -> Option<f32> {
                let v = o.valve.as_ref()?;
                if Self::taken_anywhere(o) {
                    let d = self.physics.distance_between(hand_c, o.collider);
                    return (d < TOUCH || pressing == Some(*id)).then_some(d);
                }
                let d = v.nearest_on(&o.valve_shape, palm).1;
                (d < valve::REACH).then_some(d)
            };
            let rim = self.objects.iter().filter_map(|(id, o)| Some((*id, near((id, o))?))).min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((id, _)) = rim {
                hover = Hover::Valve(id);
            }
        }
        // 3. The nearest free thing the hand is on (the palm or a finger touching it, or all but),
        // or one flying to it within the gloves' catch.
        if hover == Hover::None {
            let reach = TOUCH;
            let prev = match h.hover {
                Hover::Object(id) => Some(id),
                _ => None,
            };
            let mut best: Option<(u32, f32)> = None;
            // A hand reaching into a bowl with something in it is after what's in it (not the bowl,
            // or its grinder lying in it).
            let reaching_into = self.bowls.values().find(|r| self.in_bowl_space(r.id, palm).is_some_and(|p| Bowl::of(r).in_mouth(p, 0.02, 0.0)) && !self.contents(r.id).is_empty()).map(|r| r.id);
            for (id, o) in &self.objects {
                if o.desc.flags & OBJF_GRABBABLE == 0 || !matches!(o.holder, Holder::Free | Holder::Pulled { .. } | Holder::Stuck { .. } | Holder::InBowl { .. }) {
                    continue;
                }
                if reaching_into.is_some_and(|b| !matches!(o.holder, Holder::InBowl { bowl, grinder: false, .. } if bowl == b)) {
                    continue;
                }
                // What's in a bowl is taken out by a hand reaching into it (above it, the hand is
                // on the bowl's grinder or the bowl).
                if let Holder::InBowl { bowl, grinder: false, .. } = o.holder
                    && !self.bowls.get(&bowl).zip(self.in_bowl_space(bowl, palm)).is_some_and(|(r, p)| Bowl::of(r).in_mouth(p, 0.02, 0.0))
                {
                    continue;
                }
                let pulled = matches!(o.holder, Holder::Pulled { hand, .. } if hand == i);
                // Something flying to this hand is caught from further away (from the palm).
                let d = if pulled { self.physics.distance_to(o.collider, palm) } else { self.physics.distance_between(hand_c, o.collider) };
                // Hysteresis: keep the current choice unless another is clearly closer.
                let d = if Some(*id) == prev { d - 0.01 } else { d };
                let mut limit = if Some(*id) == prev { reach * 1.5 } else { reach };
                if pulled {
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
        // 4. A creature's part the hand is on (the palm or a finger touching it, or all but): any
        // part of anyone the game lets the hands take hold of.
        if hover == Hover::None {
            let prev = match h.hover {
                Hover::Creature(a, p) => Some((a, p)),
                _ => None,
            };
            let mut best: Option<((u32, usize), f32)> = None;
            for (&k, &(_, c, ..)) in &self.proxies {
                if !self.bodies.get(&k.0).is_some_and(|b| b.grabbable()) {
                    continue;
                }
                let d = self.physics.distance_between(hand_c, c);
                let (d, limit) = if Some(k) == prev { (d - 0.01, TOUCH * 1.5) } else { (d, TOUCH) };
                if d < limit && best.is_none_or(|b| d < b.1) {
                    best = Some((k, d));
                }
            }
            if let Some(((actor, part), _)) = best {
                hover = Hover::Creature(actor, part);
            }
        }
        // A tick as the hand enters a holster or pouch, so you can find them without looking.
        let before = self.hands.as_ref().unwrap()[i].hover;
        if matches!(hover, Hover::Slot(_) | Hover::Ammo(_) | Hover::Quiver(_)) && before != hover {
            self.emit(Cue::ZoneTick, Some(i), None, palm, 1.0);
        }
        // (A slot of the player's things ticks fainter when there's nothing in it.)
        if let (Hover::Pocket(k), true) = (hover, before != hover) {
            let empty = self.pockets.get(k).is_none_or(|p| pockets::contents(p, &self.inventory, &self.drawn_out()).is_empty());
            self.emit(Cue::ZoneTick, Some(i), None, palm, if empty { 0.35 } else { 1.0 });
        }
        let hands = self.hands.as_mut().unwrap();
        hands[i].hover = hover;
    }

    /// A hand closing in slowly on a thing it could take rests on it rather than shoving it: it stops
    /// at its surface (the thing lying on a table would squirt out from under it; a gun's grip is
    /// inside the gun, and the controller goes on in to it). Swept fast, a hand pushes as ever.
    fn rest_on(&mut self) {
        let Some(hands) = self.hands.as_mut() else { return };
        for h in hands.iter_mut() {
            let reaching = matches!(h.hover, Hover::Object(_)) && h.history.recent().length() < REACH_SPEED;
            if reaching != h.yielding {
                h.yielding = reaching;
                self.physics.set_yielding(h.body, reaching);
            }
        }
    }

    // ---- grip, buttons ------------------------------------------------------------------------------
    fn hand_input(&mut self, i: usize, inp: &RigInput, zones: &[Zone; SLOT_COUNT_BODY], _dom: usize) {
        let s = &inp.settings.interaction;
        let c = &inp.frame.hands[i];
        // Something to throw in a swinging hand (not a gun or a magazine, which you hold on to).
        let throwing = {
            let h = &self.hands.as_ref().unwrap()[i];
            // (Nor an arrow: brought to the string fast, or drawing it, it's held on to.)
            h.holding.and_then(|id| self.objects.get(&id)).is_some_and(|o| !matches!(o.desc.kind, OBJ_WEAPON | OBJ_MAGAZINE | OBJ_ARROW)) && h.history.recent().length() > THROW_EASE_SPEED
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
            self.hands.as_mut().unwrap()[i].pocket_press = None;
        }
        if pressed {
            let hover = self.hands.as_ref().unwrap()[i].hover;
            self.grab(i, inp);
            if let Hover::Pocket(k) = hover {
                let now = self.now;
                let h = &mut self.hands.as_mut().unwrap()[i];
                h.pocket_press = Some((k, now, h.grip_t.0, h.holding));
            }
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
            // The other hand's grip on a held book opens it (or shuts it): it stays in the hand
            // that has it, and this one stays free to turn its pages.
            Hover::Object(id) if self.objects.get(&id).is_some_and(|o| o.holder == Holder::Held(i ^ 1) && o.desc.flags & OBJF_OPENS != 0) => {
                let o = self.objects.get_mut(&id).unwrap();
                o.opens = o.opens.wrapping_add(1);
                tracing::info!("rig: {id:#x} opened or shut by the other hand ({})", o.opens);
                self.emit(Cue::Grab, Some(i), Some(id), palm, 0.4);
            }
            Hover::Object(id) if self.objects.get(&id).is_some_and(|o| o.holder == Holder::Held(i ^ 1)) => self.second_hand(i, id),
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
                let from = self.physics.pose(self.hands.as_ref().unwrap()[i].body);
                // (Taken anywhere: held where the hand is, as it's turned, so neither moves.)
                let anywhere = Self::taken_anywhere(o);
                let at = if anywhere { v.local(from.0) } else { v.nearest_on(&o.valve_shape, palm).0 };
                let grip_rot = if anywhere { from.1 } else { grip_rot };
                let now = self.now;
                let h = &mut self.hands.as_mut().unwrap()[i];
                let wrist = v.wrist(h.target.1);
                let slack = v.lead(at, h.target.0).abs().max(valve::ARM_STRETCH * self.map.world_per_meter);
                h.valve = Some(valve::ValveGrip { id, at, rot: v.rot().inverse() * grip_rot, from, since: now, turn0: v.angle, twisted: 0.0, wrist, slack });
                let c = h.collider;
                self.physics.set_groups(c, groups_none());
                tracing::info!("rig: hand {i} takes valve {id:#x} at {at:.3?} (its frame), turned {:.3}", v.angle);
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
            Hover::Foregrip(wid) => self.second_hand(i, wid),
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
            Hover::Quiver(bid) => {
                // An arrow for the bow, while the game has one to give (its count, less the arrows
                // already out of the quiver: in a hand, on the string, dropped).
                let b = &self.objects[&bid].desc;
                let game = b.flags & OBJF_GAME_AMMO != 0;
                match self.arrow_template(b.ammo_type) {
                    Some(t) if !game || t.rounds > self.arrows_out(t.id) => self.draw_arrow(i, &t, bid),
                    _ => self.emit(Cue::Fail, Some(i), None, palm, 1.0),
                }
            }
            Hover::String(bid) => self.take_string(i, bid),
            Hover::Creature(actor, part) => self.take_creature(i, actor, part),
            Hover::Pocket(k) => self.draw_pocket(i, k),
            Hover::CreaturePocket(pid) => self.take_from_pocket(i, pid),
        }
        let _ = inp;
    }

    /// Hand `i` takes the string of the bow the other hand holds, by the arrow resting on it: it
    /// draws it from here.
    fn take_string(&mut self, i: usize, bid: u32) {
        let aim = self.aim_in_grip(i);
        let Some(arrow) = self.objects.get(&bid).and_then(|b| b.bow?.nocked) else { return };
        let Some(a) = self.objects.get_mut(&arrow) else { return };
        // (Held as it comes out of the quiver once it's off the string.)
        let (p, q) = bow::held_arrow(&a.desc, aim);
        a.attach = Attach::Fixed(p, q);
        a.handled = true;
        let now = self.now;
        if let Some(b) = self.objects.get_mut(&bid).and_then(|b| b.bow.as_mut()) {
            b.drawing = Some(i);
            b.drawn_at = now;
        }
        let at = self.pinch(i);
        let h = &mut self.hands.as_mut().unwrap()[i];
        h.holding = Some(arrow);
        h.on = On::Free;
        h.hold = None;
        h.glove = Glove::Idle;
        let c = h.collider;
        self.physics.set_groups(c, groups_none());
        self.emit(Cue::Grab, Some(i), Some(bid), at, 0.5);
    }

    /// Where hand `i`'s controller points (its aim pose), turned into its grip pose.
    fn aim_in_grip(&self, i: usize) -> Quat {
        let h = &self.hands.as_ref().unwrap()[i];
        (h.target.1.inverse() * h.aim_rot).normalize()
    }

    /// Where hand `i`'s fingers pinch an arrow's nock and the string, from its controller (world).
    fn pinch(&self, i: usize) -> Vec3 {
        let (p, q) = self.hands.as_ref().unwrap()[i].target;
        p + q * bow::PINCH * self.map.world_per_meter
    }

    /// The arrows of template `t` out of the quiver (in a hand, on a string, lying about).
    fn arrows_out(&self, t: u32) -> u32 {
        self.objects.values().filter(|o| o.spawned && o.draw_desc == t && o.desc.kind == OBJ_ARROW).count() as u32
    }

    /// Puts object `id` in hand `i`. `snap`: ease it into the hand's natural hold (guns always do).
    /// Testing (`RUSTVR_TEST_TAKE`): the nearest of the game's loose props within 6 m of hand `i`
    /// (`ours`: only one the runtime moves itself, not one the game simulates), taken into it as
    /// the gloves' catch would.
    pub fn test_take_nearest(&mut self, i: usize, ours: bool) -> Option<u32> {
        let palm = self.hands.as_ref()?[i].target.0;
        let nearest = self.objects.iter().filter(|(_, o)| o.desc.kind == OBJ_PROP).map(|(&id, o)| (id, o.holder, self.physics.pose(o.body).0.distance(palm))).min_by(|a, b| a.2.total_cmp(&b.2));
        tracing::info!("rig: test take: the palm at {palm:.2?}; the nearest object {nearest:?}");
        let (id, _) = self
            .objects
            .iter()
            .filter(|(_, o)| o.holder == Holder::Free && o.desc.kind == OBJ_PROP && (!ours || o.desc.flags & OBJF_GAME_SIMULATED == 0))
            .map(|(&id, o)| (id, self.physics.pose(o.body).0.distance(palm)))
            .filter(|&(_, d)| d < 12.0 * self.map.world_per_meter)
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        self.take(id, i, true);
        Some(id)
    }

    /// Object `id`, lying free, to hand `i` as the gloves' catch brings it (a book read where it
    /// lies, to be held open in the hand). False if it isn't free (or the hand is full).
    pub fn bring(&mut self, id: u32, i: usize) -> bool {
        let free = self.objects.get(&id).is_some_and(|o| o.holder == Holder::Free);
        let empty = self.hands.as_ref().is_some_and(|h| h[i].holding.is_none() && h[i].support.is_none());
        if free && empty {
            self.take(id, i, true);
        }
        free && empty
    }

    /// The hands turning a held book's pages (their grip on them), from now on: they take nothing,
    /// and their gloves pull nothing, until they're done.
    pub fn set_page_hands(&mut self, hands: [bool; 2]) {
        self.page_hands = hands;
    }

    /// Object `id`, held by the other hand, into hand `i`'s palm (a sheet spread between both hands,
    /// let go by the one that had it: the other has it now).
    pub fn give(&mut self, id: u32, i: usize) {
        if self.objects.get(&id).is_some_and(|o| o.holder == Holder::Held(i ^ 1)) {
            self.take(id, i, true);
        }
    }

    /// Testing (`RUSTVR_TEST_TAKE=<hand>:<id>`): object `id`, wherever it is, into hand `i` as the
    /// gloves' catch would take it.
    pub fn test_take(&mut self, i: usize, id: u32) -> Option<u32> {
        let o = self.objects.get(&id).filter(|o| o.holder == Holder::Free)?;
        tracing::info!("rig: test take: {id:#x} from {:.2?}", self.physics.pose(o.body).0);
        self.take(id, i, true);
        Some(id)
    }

    fn take(&mut self, id: u32, i: usize, snap: bool) {
        let (palm, grip_rot) = self.hands.as_ref().unwrap()[i].target;
        // A hand holds one thing: whatever else it had is let go (it falls), never left held by a
        // hand that has something else.
        if let Some(before) = self.hands.as_ref().unwrap()[i].holding.filter(|&b| b != id) {
            self.hands.as_mut().unwrap()[i].holding = None;
            self.free_body(before, groups_free());
            tracing::warn!("rig: {before:#x} let go: the hand took {id:#x}");
        }
        // Out of the other hand if it was there.
        let other = i ^ 1;
        if self.hands.as_ref().unwrap()[other].holding == Some(id) {
            self.hands.as_mut().unwrap()[other].holding = None;
        }
        if self.hands.as_ref().unwrap()[other].supporting() == Some(id) {
            let h = &mut self.hands.as_mut().unwrap()[other];
            h.support = None;
            let c = h.collider;
            self.physics.set_groups(c, groups_hand(other));
        }
        let Some(o) = self.objects.get(&id) else { return };
        // Off the string it's on.
        if let Holder::Nocked { bow } = o.holder
            && let Some(b) = self.objects.get_mut(&bow).and_then(|b| b.bow.as_mut())
        {
            (b.nocked, b.drawing, b.draw) = (None, None, 0.0);
        }
        let Some(o) = self.objects.get(&id) else { return };
        let (pos, rot) = self.physics.pose(o.body);
        let d = o.desc;
        let gun = d.kind == OBJ_WEAPON;
        // Where the hand is on it (its controller grip pose in the thing's space).
        let local = (rot.inverse() * (palm - pos), (rot.inverse() * grip_rot).normalize());
        let mut drawn = None;
        // Its grip, if the hand took it there: a gun's pistol grip locks on, a handle takes the
        // hand anywhere along it. Drawn from a holster or caught (`snap`), a gun or a rigged
        // weapon goes into the hand by its grip whatever.
        let rigged = gun || (authored(d.grip_rot).is_some() && matches!(d.kind, OBJ_THROWABLE | OBJ_MELEE | OBJ_BOW));
        let hold = self.handle_of(&d, i, On::Grip).and_then(|h| match h.take(local.0, local.1) {
            Some(hold) => Some(hold),
            None if snap && rigged => Some((h.at, h.rot)),
            None => None,
        });
        let (attach, on) = if let Some(hold) = hold {
            if gun {
                (Attach::Gun, On::Grip)
            } else {
                let rel = inverse(hold);
                (Attach::Fixed(rel.0, rel.1), On::Grip)
            }
        } else if snap && is_round(&d) {
            // A round sits nose first along the fist's axis, its base in the fist.
            let rot = held_round_rot();
            (Attach::Fixed(HELD_ROUND_BASE - rot * v3(d.mag_well), rot), On::Free)
        } else if snap && d.kind == OBJ_ARROW {
            // An arrow from the quiver: its nock between the index and the thumb, pointing where
            // the controller points.
            let (p, q) = bow::held_arrow(&d, self.aim_in_grip(i));
            (Attach::Fixed(p, q), On::Free)
        } else if snap && d.kind == OBJ_MAGAZINE {
            // A fresh magazine sits in the fist along the grip pose's +Z (the fist's axis, little
            // finger to thumb), its top 6 cm out past the thumb, ready to push into a well.
            let rot = held_mag_rot();
            let top = v3(d.mag_well);
            (Attach::Fixed(HELD_MAG_TOP - rot * top, rot), On::Free)
        } else if snap {
            // Caught from the gloves (it was up to `glove_catch_radius` away): its surface into the
            // palm.
            let shift = self.palm_on(i, o.collider, (palm, grip_rot), false) - palm;
            let rel = compose(inverse((palm, grip_rot)), (pos - shift, rot));
            (Attach::Fixed(rel.0, rel.1), On::Free)
        } else {
            // Anywhere: held just as it is (from the controller as it is now), the hand drawn just
            // where it is (where the world stopped it): neither moves, and the fingers close on it
            // from where they are.
            let rel = compose(inverse((palm, grip_rot)), (pos, rot));
            let hand = self.physics.pose(self.hands.as_ref().unwrap()[i].body);
            drawn = Some(compose(inverse((pos, rot)), hand));
            (Attach::Fixed(rel.0, rel.1), On::Free)
        };
        // Where it goes in the hand: it eases there (nowhere, taken anywhere with the hand where
        // the controller is).
        let to = match attach {
            Attach::Fixed(p, q) => compose((palm, grip_rot), (p, q)).0,
            Attach::Gun => palm,
        };
        if let Some(Holder::InBowl { bowl, grinder, .. }) = self.objects.get(&id).map(|o| o.holder) {
            tracing::info!("rig: the {} hand takes {id:#x} out of the bowl {bowl:#x}{}: the hand at {:.3?} in it", if i == 0 { "left" } else { "right" }, if grinder { " (its grinder)" } else { "" }, self.in_bowl_space(bowl, palm));
        }
        self.out_of_bowl(id);
        let o = self.objects.get_mut(&id).unwrap();
        o.attach = attach;
        o.blend = Some((pos, rot, self.now, pos.distance(to)));
        if let Holder::Holstered(slot) = o.holder {
            o.home_slot = Some(slot);
        }
        // Not yet moving with the player: it's taken up from where it was.
        (o.carry, o.carried) = (None, (Vec3::ZERO, Vec3::ZERO));
        o.holder = Holder::Held(i);
        o.handled = true;
        o.hovered = false;
        if let Some(t) = &mut o.throw {
            t.flight = None;
            t.thrown_at = None;
        }
        // (One on the game's constraints stays where the game has it: the game pulls it to the hand.)
        self.physics.set_kinematic(o.body, o.desc.flags & OBJF_CONSTRAINED != 0);
        self.physics.set_gravity(o.body, 1.0);
        self.physics.set_angular_damping(o.body, 0.2);
        self.physics.set_groups(o.collider, groups_held(i));
        let h = &mut self.hands.as_mut().unwrap()[i];
        h.holding = Some(id);
        h.on = on;
        h.hold = drawn;
        h.glove = Glove::Idle;
        self.physics.set_groups(h.collider, groups_none());
        self.debug.grabs += 1;
        tracing::info!("rig: hand {i} takes {id:#x} (kind {}, {on:?})", d.kind);
        self.emit(Cue::Grab, Some(i), Some(id), palm, 1.0);
    }

    /// Hand `i` takes hold of what the other hand holds: both hands have it. On the thing's own grip
    /// when the other hand isn't (a gun's pistol grip, a sword's handle), this hand holds it there
    /// and the other holds on where it is; anywhere else this is the second hand, on the handle a
    /// second hand takes (a handguard, anywhere along it) or where it took it.
    fn second_hand(&mut self, i: usize, id: u32) {
        let other = i ^ 1;
        if self.hands.as_ref().unwrap()[other].holding != Some(id) {
            return;
        }
        let Some(o) = self.objects.get(&id) else { return };
        let (op, oq) = self.physics.pose(o.body);
        let (collider, gun) = (o.collider, o.desc.kind == OBJ_WEAPON);
        let palm = self.hands.as_ref().unwrap()[i].target.0;
        let other_on = self.hands.as_ref().unwrap()[other].on;
        if other_on != On::Grip
            && let Some(hold) = self.handle_hold(i, id, On::Grip)
        {
            let theirs = self.drawn_hold(id, other);
            let reach = self.reach_of(other, (op, oq), theirs);
            {
                let h = &mut self.hands.as_mut().unwrap()[other];
                h.holding = None;
                h.hold = None;
                h.support = Some(Support { id, at: theirs.0, rot: theirs.1, on: other_on, reach });
                h.euro.reset();
            }
            let now = self.now;
            let o = self.objects.get_mut(&id).unwrap();
            let rel = inverse(hold);
            o.attach = if gun { Attach::Gun } else { Attach::Fixed(rel.0, rel.1) };
            o.holder = Holder::Held(i);
            o.blend = Some((op, oq, now, palm.distance(op + oq * hold.0)));
            self.physics.set_groups(collider, groups_held(i));
            let h = &mut self.hands.as_mut().unwrap()[i];
            h.holding = Some(id);
            h.on = On::Grip;
            h.hold = None;
            h.glove = Glove::Idle;
            let c = h.collider;
            self.physics.set_groups(c, groups_none());
            self.debug.grabs += 1;
            self.emit(Cue::Grab, Some(i), Some(id), palm, 1.0);
            return;
        }
        let (at, rot, on) = match self.handle_hold(i, id, On::Foregrip) {
            Some((at, rot)) => (at, rot, On::Foregrip),
            None => {
                // Anywhere: just where the hand is drawn on it now (neither moves).
                let (hp, hq) = self.physics.pose(self.hands.as_ref().unwrap()[i].body);
                (oq.inverse() * (hp - op), (oq.inverse() * hq).normalize(), On::Free)
            }
        };
        let reach = if on == On::Free { self.reach_of(i, (op, oq), (at, rot)) } else { Vec3::ZERO };
        let h = &mut self.hands.as_mut().unwrap()[i];
        h.support = Some(Support { id, at, rot, on, reach });
        h.glove = Glove::Idle;
        h.euro.reset();
        let c = h.collider;
        self.physics.set_groups(c, groups_none());
        self.emit(Cue::Grab, Some(i), Some(id), palm, 0.7);
    }

    /// Where hand `i` is drawn on thing `id` it holds (its grip pose in the thing's space).
    fn drawn_hold(&self, id: u32, i: usize) -> (Vec3, Quat) {
        self.hands.as_ref().and_then(|h| h[i].hold).unwrap_or_else(|| self.hold_of(id, i))
    }

    /// Where a hold (`at` on a thing at `thing`) is from hand `i`'s controller, in its own space.
    fn reach_of(&self, i: usize, thing: (Vec3, Quat), at: (Vec3, Quat)) -> Vec3 {
        let (cp, cq) = self.hands.as_ref().unwrap()[i].target;
        cq.inverse() * (thing.0 + thing.1 * at.0 - cp)
    }

    /// The handle `which` of a thing (`d`) for hand `i` ([`grips`]).
    fn handle_of(&self, d: &ObjectDesc, i: usize, which: On) -> Option<grips::Handle> {
        // (A prop has no handedness: the left hand holds it the same way round, its palm on the
        // same side, the hold turned half round the fist's axis rather than mirrored.)
        let grip_rot = authored(d.grip_rot).map(|g| match (i, d.kind) {
            (1, _) => g,
            (_, OBJ_PROP) => (g * Quat::from_rotation_z(std::f32::consts::PI)).normalize(),
            _ => mirror_grip(g),
        });
        match which {
            On::Grip => grips::grip(d, grip_rot),
            On::Foregrip => grips::foregrip(d, self.support_rot(d, i), grip_rot),
            On::Free => None,
        }
    }

    /// The hold hand `i` takes on thing `id`'s handle `which`, if it's near it now (thing space).
    fn handle_hold(&self, i: usize, id: u32, which: On) -> Option<(Vec3, Quat)> {
        let o = self.objects.get(&id)?;
        let (op, oq) = self.physics.pose(o.body);
        let (palm, rot) = self.hands.as_ref()?[i].target;
        self.handle_of(&o.desc, i, which)?.take(oq.inverse() * (palm - op), (oq.inverse() * rot).normalize())
    }

    /// Where hand `i` holds thing `id` that it holds (its controller grip pose in the thing's space).
    fn hold_of(&self, id: u32, i: usize) -> (Vec3, Quat) {
        let o = &self.objects[&id];
        match o.attach {
            Attach::Gun => {
                let grip = v3(o.desc.grip);
                match authored(o.desc.grip_rot) {
                    Some(g) => (grip, if i == 0 { mirror_grip(g) } else { g }),
                    None => {
                        let h = &self.hands.as_ref().unwrap()[i];
                        (grip, (h.aim_rot.inverse() * h.target.1).normalize())
                    }
                }
            }
            Attach::Fixed(p, q) => inverse((p, q)),
        }
    }

    /// Where hand `i`'s grip pose (`grip`, world) goes for its palm to rest on collider `c`: along
    /// to the surface nearest the palm (`limit`: no further than [`PALM_REACH`]).
    fn palm_on(&self, i: usize, c: ColliderHandle, grip: (Vec3, Quat), limit: bool) -> Vec3 {
        let back = if i == 1 { Vec3::X } else { Vec3::NEG_X };
        let palm = grip.0 + grip.1 * back * (PALM * self.map.world_per_meter);
        let shift = self.physics.surface_point(c, palm) - palm;
        let shift = if limit { shift.clamp_length_max(PALM_REACH * self.map.world_per_meter) } else { shift };
        grip.0 + shift
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
            if h.creature.is_some() {
                let throw = h.history.release(self.now).lin;
                self.let_go_creature(i, Some(throw));
                return;
            }
            if h.valve.is_some() {
                // Back to the controller from the rim, at a hand's pace, through whatever's between.
                self.let_go_valve(i, s.grab_transition);
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
        let pocketed = self.objects.get(&id).and_then(|o| o.pocketed);
        // Out of a pocket, and the game deciding on it: the hand holds on until it has (a moment).
        if matches!(pocketed, Some(Pocketed::Deciding { .. })) {
            let h = &mut self.hands.as_mut().unwrap()[i];
            h.holding = Some(id);
            h.let_go = true;
            return;
        }
        self.physics.set_groups(self.hands.as_ref().unwrap()[i].collider, groups_hand(i));
        // The string let go: the arrow is loosed, or (short of a shot) stays on the string.
        if let Holder::Nocked { bow } = self.objects[&id].holder {
            self.let_string_go(bow, id, i);
            return;
        }
        // On the game's constraints: the game's again at once (it was never anywhere else), and
        // the hand goes back to its controller from it, as off a valve.
        if self.objects[&id].desc.flags & OBJF_CONSTRAINED != 0 {
            let o = self.objects.get_mut(&id).unwrap();
            o.holder = Holder::Free;
            o.free_since = self.now;
            o.aim = None;
            let h = &mut self.hands.as_mut().unwrap()[i];
            h.hold = None;
            let (b, target) = (h.body, h.target.0);
            let from = self.physics.pose(b);
            let time = travel_time(s.grab_transition, from.0.distance(target));
            self.hands.as_mut().unwrap()[i].returning = Some((from.0, from.1, self.now, time));
            self.emit(Cue::Release, Some(i), Some(id), palm, 0.6);
            return;
        }
        // Let go still in its pocket: back where it was, nothing tried.
        if matches!(pocketed, Some(Pocketed::Inside { .. })) {
            self.remove(id);
            self.emit(Cue::Release, Some(i), None, palm, 0.4);
            return;
        }
        // Let go of while it's stuck in something: it stays in.
        if self.objects[&id].embed.is_some() {
            let o = self.objects.get_mut(&id).unwrap();
            o.holder = Holder::Free;
            o.free_since = self.now;
            self.emit(Cue::Release, Some(i), Some(id), palm, 0.6);
            return;
        }
        // The other hand has it too: it holds it now, where it has it (from hand to hand).
        if let Some(sup) = self.hands.as_ref().unwrap()[i ^ 1].support.filter(|s| s.id == id) {
            let (p, q) = self.physics.pose(self.objects[&id].body);
            let h = &mut self.hands.as_mut().unwrap()[i ^ 1];
            h.support = None;
            h.holding = Some(id);
            h.on = sup.on;
            // Held from its controller just as it is now, the hand drawn where it is on it: neither
            // moves as it's handed over.
            let ctrl = h.target;
            h.hold = Some((sup.at, sup.rot));
            let o = self.objects.get_mut(&id).unwrap();
            let rel = compose(inverse(ctrl), (p, q));
            o.attach = Attach::Fixed(rel.0, rel.1);
            o.holder = Holder::Held(i ^ 1);
            o.blend = None;
            let c = o.collider;
            self.physics.set_groups(c, groups_held(i ^ 1));
            self.emit(Cue::Release, Some(i), Some(id), palm, 0.5);
            return;
        }
        let o = &self.objects[&id];
        let (desc, body) = (o.desc, o.body);
        // An arrow let go of gently where arrows come from goes back in: the quiver, or any of the
        // body's slots for a game that hands its arrows out from them, as a thing drawn from one
        // goes back in any.
        let arrows_back = if self.pockets.is_empty() {
            zones::quiver_zones(inp.eye_height, i, inp.settings.archery.quiver).iter().any(|z| self.anchor.contains(z, palm_t))
        } else {
            self.pocket_zones.iter().any(|z| self.anchor.contains(&z.put, palm_t))
        };
        if desc.kind == OBJ_ARROW && o.spawned && self.hands.as_ref().unwrap()[i].history.recent().length() < HOLSTER_MAX_SPEED && arrows_back {
            tracing::info!("rig: an arrow {id:#x} back in the quiver");
            self.remove(id);
            self.emit(Cue::HolsterIn, Some(i), None, palm, 0.6);
            return;
        }
        let class = o.throw.as_ref().map(|t| t.class()).unwrap_or(ThrowClass::Generic);
        // Never stowed armed (or with the pin out).
        let stowable = o.throw.as_ref().is_none_or(|t| t.stowable());
        let holsterable = desc.flags & OBJF_HOLSTERABLE != 0 && stowable;
        let hand_speed = self.hands.as_ref().unwrap()[i].history.recent().length();
        // A thing drawn from a slot, let go of gently in any slot (one of the player's things', or a
        // holster): back in the inventory it never left.
        if let Some(d) = self.objects[&id].drawn
            && hand_speed < HOLSTER_MAX_SPEED
        {
            let pocket = self.pocket_zones.iter().position(|z| self.anchor.contains(&z.put, palm_t));
            if pocket.is_some() || zones.iter().any(|z| self.anchor.contains(z, palm_t)) {
                self.remove(id);
                self.debug.pocket_returns += 1;
                tracing::info!("rig: {id:#x} put back (in slot {pocket:?})");
                self.pocket_events.push(PocketEvent::PutBack(pocket.unwrap_or(d.slot), d.key));
                self.emit(Cue::PocketIn, Some(i), None, palm, 1.0);
                return;
            }
        }
        // Let go gently in one of the game's creatures' pockets: planted (a thing to take, or one taken
        // from a pocket), if the game will have it. One taken goes into the pocket (it's the
        // player's still if the game won't: it was never anywhere else); a thing of the game's is let
        // go there as anywhere, and the game takes it if it's planted.
        let taken = pocketed == Some(Pocketed::Taken);
        let plant = (desc.flags & OBJF_LOOT != 0 || taken)
            .then(|| pickpocket::pocket_at(&self.creature_pockets, self.hand_reach(i), self.hands.as_ref().unwrap()[i].pocket.map(|p| p.pocket), self.map.world_per_meter))
            .flatten()
            .and_then(|pid| self.pocket(pid).copied())
            .filter(|p| p.flags & POCKET_SHUT == 0 && hand_speed < HOLSTER_MAX_SPEED);
        if let Some(p) = plant {
            self.open_pocket_session(&p, i, palm);
            let ev = self.pocket_event_of(PE_PLANT, p.actor, p.id, 0, id, i);
            let n = self.pocket_event(ev);
            tracing::info!("rig: {id:#x} let go in pocket {:#x} of {:#x}: planting it (event {n})", p.id, p.actor);
            self.emit(Cue::PocketPlant, Some(i), Some(id), palm, 1.0);
            if taken {
                self.remove(id);
                return;
            }
        }
        // A thing taken from a pocket, let go gently in a body slot (a holster, or one of the player's
        // things'): kept (it's the player's).
        if taken
            && hand_speed < HOLSTER_MAX_SPEED
            && (zones.iter().any(|z| self.anchor.contains(z, palm_t)) || self.pocket_zones.iter().any(|z| self.anchor.contains(&z.put, palm_t)))
        {
            let ev = self.pocket_event_of(PE_STOW, 0, 0, 0, id, i);
            self.pocket_event(ev);
            self.remove(id);
            self.emit(Cue::HolsterIn, Some(i), None, palm, 1.0);
            return;
        }
        // A thing to take, let go of gently in any of the body's slots: stowed, for the game.
        if desc.flags & OBJF_LOOT != 0
            && plant.is_none()
            && hand_speed < HOLSTER_MAX_SPEED
            && let Some(slot) = zones
                .iter()
                .position(|z| self.anchor.contains(z, palm_t))
                .or_else(|| self.pocket_zones.iter().position(|z| self.anchor.contains(&z.put, palm_t)).map(|k| SLOT_COUNT_BODY + k))
        {
            self.stow(id, slot as u32);
            self.emit(Cue::PocketIn, Some(i), Some(id), palm, 1.0);
            return;
        }
        // Let go of over a bowl that takes it (not flung at it): in. A bowl's grinder let go with its
        // tip over the mouth rests in it again.
        if hand_speed < BOWL_MAX_SPEED {
            let (p, q) = self.physics.pose(body);
            if let Some(bowl) = self.bowl_for(id, p, receptacle::MOUTH_ABOVE, receptacle::MOUTH_SLACK) {
                self.into_bowl(id, bowl);
                self.emit(Cue::Release, Some(i), Some(id), palm, 0.6);
                return;
            }
            let home = self.bowls.values().find(|r| r.grinder == id).copied();
            if let Some(r) = home
                && let Some(tip) = self.in_bowl_space(r.id, p + q * v3(r.tip) * self.map.world_per_meter)
                && Bowl::of(&r).in_mouth(tip, receptacle::MOUTH_ABOVE * 0.5, receptacle::MOUTH_SLACK)
            {
                self.rest_grinder(id, r.id);
                self.emit(Cue::Release, Some(i), Some(id), palm, 0.6);
                return;
            }
        }
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
        // Kept on the body (a map): back to the slot it was drawn from, however it's let go.
        if o.desc.flags & OBJF_KEPT != 0
            && let Some(slot) = o.home_slot.filter(|&s| self.slots[s as usize].is_none_or(|x| x == id))
        {
            let from = self.physics.pose(o.body);
            o.holder = Holder::Returning { slot, t0: self.now, from };
            self.slots[slot as usize] = Some(id);
            self.physics.set_kinematic(o.body, true);
            self.physics.set_groups(o.collider, groups_none());
            self.debug.returns += 1;
            self.emit(Cue::Release, Some(i), Some(id), palm, 0.6);
            return;
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
        // A thing taken from a pocket let go in the world: the game's own thing there, where it's
        // let go and moving as it goes (the copy flies on until the game has it).
        if taken {
            let ev = self.pocket_event_of(PE_DROP, 0, 0, 0, id, i);
            let n = self.pocket_event(ev);
            if let Some(o) = self.objects.get_mut(&id) {
                o.pocketed = Some(Pocketed::Dropping { event: n, since: self.now });
            }
        }
        if lit {
            self.debug.arms += 1;
            self.emit(Cue::GrenadeArm, None, Some(id), at, 0.8);
        }
    }

    // ---- the slots that hand out the player's things ([`pockets`]) ------------------------------
    /// The player's things as the game keeps them, whenever the adapter rewrites them: the copies
    /// the game has taken back (used, or dropped into the world) go, and so does a copy of a stack
    /// that's gone (all of it dropped from the game's own menu, say).
    pub fn set_inventory(&mut self, inv: &Inventory) {
        *self.inventory = *inv;
        // (What was taken before this runtime started isn't ours: its numbers may come round again.)
        let mut seen = self.taken_seen.unwrap_or(inv.taken_count);
        for id in inv.taken_since(&mut seen) {
            if self.objects.get(&id).is_some_and(|o| o.drawn.is_some()) {
                tracing::info!("rig: {id:#x} taken back by the game");
                self.remove(id);
            }
        }
        self.taken_seen = Some(seen);
        let gone: Vec<u32> = self.objects.iter().filter(|(_, o)| o.drawn.is_some_and(|d| inv.item(d.item).is_none())).map(|(id, _)| *id).collect();
        for id in gone {
            tracing::info!("rig: {id:#x}'s stack is gone from the inventory: it goes too");
            self.remove(id);
        }
    }

    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// The body's slots in use (the player's, or the game's own).
    pub fn set_pockets(&mut self, slots: &[BodySlot]) {
        if self.pockets != slots {
            self.pockets = slots.to_vec();
            self.placing = self.placing.filter(|&k| k < slots.len());
        }
    }

    /// Whether how the hand holds a thing of model `model` drawn from a slot is known yet.
    pub fn has_hold(&self, model: &str) -> bool {
        self.holds.contains_key(model)
    }

    /// How the hand holds a thing of model `model` drawn from a slot ([`pockets::hold`]).
    pub fn set_hold(&mut self, model: &str, hold: Option<pockets::Hold>) {
        self.holds.insert(model.to_owned(), hold);
    }

    /// Slot `slot` is to be placed by hand (None: not any more): the next squeeze puts it there.
    pub fn place_pocket(&mut self, slot: Option<usize>) {
        self.placing = slot.filter(|&k| k < self.pockets.len());
    }

    pub fn placing(&self) -> Option<usize> {
        self.placing
    }

    /// The sound set of a thing drawn from a slot (or shown at one), as the game's inventory has a
    /// thing of its kind sound ([`InvItem::sound`]).
    pub fn drawn_sound(&self, id: u32) -> Option<&str> {
        let o = self.objects.get(&id)?;
        let item = o.drawn.map(|d| d.item).or(o.shown)?;
        self.inventory.items().iter().find(|i| i.id == item).map(|i| i.sound()).filter(|s| !s.is_empty())
    }

    /// How many of each stack are out of the slots (in hands, or let go and not yet taken back).
    pub fn drawn_out(&self) -> HashMap<u32, u32> {
        let mut out = HashMap::new();
        for d in self.objects.values().filter_map(|o| o.drawn) {
            *out.entry(d.item).or_insert(0) += 1;
        }
        out
    }

    /// What each slot gives now (its first thing, and how many it has to give in all) and where it
    /// is (tracking space).
    pub fn pockets_now(&self) -> Vec<SlotNow> {
        let out = self.drawn_out();
        self.pockets
            .iter()
            .zip(&self.pocket_zones)
            .map(|(p, z)| {
                let has = pockets::contents(p, &self.inventory, &out);
                let count = has.iter().map(|i| i.count - out.get(&i.id).copied().unwrap_or(0).min(i.count)).sum();
                SlotNow { item: has.first().map_or(0, |i| i.id), count, at: self.anchor.zone_center(&z.take).to_array(), _pad: [0; 3] }
            })
            .collect()
    }

    /// While the slots' editor is open, where each slot of the player's things is and how far it
    /// reaches out (tracking space: its middle, its taking radius), and whether it's the one picked
    /// in the editor; nothing once it closes. The picked slot's first thing shows there too
    /// ([`Rig::show_pockets`]), but it can be inside the drawn body (a hip's) or out of sight.
    pub fn pocket_marks(&self) -> Vec<(Vec3, f32, bool)> {
        let Some(picked) = self.editing else { return Vec::new() };
        self.pocket_zones.iter().enumerate().map(|(k, z)| (self.anchor.zone_center(&z.take), z.take.radius, k == picked)).collect()
    }

    /// The things drawn from the slots, as the runtime describes them (for drawing: they're in no
    /// adapter's table).
    pub fn drawn_descs(&self) -> Vec<ObjectDesc> {
        self.objects.values().filter(|o| o.drawn.is_some() || o.shown.is_some()).map(|o| o.desc).collect()
    }

    /// While the slots' editor is open, the first thing of the slot picked in it (`picked`) shown
    /// still at its place on the body, the game's own model, so the player sees where it is (and
    /// what it gives); none once it closes. One at a time: the chest's slots are close under the
    /// eyes, and a scroll there is big. Not a thing the hands can take.
    pub fn show_pockets(&mut self, picked: Option<usize>) {
        self.editing = picked;
        let out = self.drawn_out();
        let want: Vec<Option<InvItem>> =
            self.pockets.iter().enumerate().map(|(k, p)| if Some(k) == picked { pockets::contents(p, &self.inventory, &out).first().map(|i| **i) } else { None }).collect();
        let shown: Vec<(u32, usize, u32)> = self.objects.iter().filter_map(|(id, o)| match (o.holder, o.shown) {
            (Holder::Shown(k), Some(item)) => Some((*id, k, item)),
            _ => None,
        }).collect();
        for &(id, k, item) in &shown {
            if want.get(k).copied().flatten().is_none_or(|w| w.id != item) {
                self.remove(id);
            }
        }
        for (k, item) in want.iter().enumerate() {
            let Some(item) = item else { continue };
            if shown.iter().any(|&(_, sk, si)| sk == k && si == item.id) {
                continue;
            }
            let id = self.next_spawn;
            self.next_spawn = self.next_spawn.wrapping_add(1).max(SPAWN_BASE);
            let mut d = pockets::object_desc(item, id);
            d.flags &= !(OBJF_GRABBABLE | OBJF_PULLABLE);
            let at = self.pocket_zones.get(k).map_or(Vec3::ZERO, |z| self.map.point(self.anchor.zone_center(&z.take)));
            self.add(d, id, true, at, self.map.rot(self.anchor.rot()));
            if let Some(o) = self.objects.get_mut(&id) {
                o.holder = Holder::Shown(k);
                o.shown = Some(item.id);
                o.handled = true;
                let (body, collider) = (o.body, o.collider);
                self.physics.set_kinematic(body, true);
                self.physics.set_groups(collider, groups_none());
            }
        }
    }

    /// Hand `i` reaches into slot `k`: its first thing into the fist (the runtime's copy of it), or,
    /// empty, what the slot does then.
    /// One of the bow's arrows (`t`, its template) into hand `i`, held to be nocked: from the
    /// quiver, or from a slot the player keeps arrows in. `of`: the object whose sound it makes
    /// coming out.
    fn draw_arrow(&mut self, i: usize, t: &ObjectDesc, of: u32) {
        let (palm, grip_rot) = self.hands.as_ref().unwrap()[i].target;
        // (Out of the quiver already as it's held, so it doesn't swing into the hand.)
        let (p, q) = compose((palm, grip_rot), bow::held_arrow(t, self.aim_in_grip(i)));
        let id = self.spawn_mag(t, 1, p, q);
        self.debug.quiver_draws += 1;
        self.take(id, i, true);
        self.emit(Cue::Quiver, Some(i), Some(of), palm, 1.0);
    }

    fn draw_pocket(&mut self, i: usize, k: usize) {
        let (palm, grip_rot) = self.hands.as_ref().unwrap()[i].target;
        let Some(slot) = self.pockets.get(k) else { return };
        let out = self.drawn_out();
        let has = pockets::contents(slot, &self.inventory, &out);
        tracing::info!(
            "rig: slot {k} ({}) has {}",
            slot.name,
            has.iter().take(4).map(|i| format!("{} x{} (restores {:?}, value {}, power {})", i.name(), i.count, i.restores, i.value, i.power)).collect::<Vec<_>>().join(", ")
        );
        let Some(item) = has.first().map(|i| **i) else {
            if slot.empty != crate::settings::SlotEmpty::Silent {
                self.emit(Cue::Fail, Some(i), None, palm, 0.6);
            }
            return;
        };
        let name = slot.name.clone();
        // An arrow of the kind on the bow, from whichever slot it's kept in: one of the bow's own,
        // as the quiver gives them, while the game has one to give. (Another kind comes out as
        // itself, and the game puts it on the bow: its own then takes the copy's place, `sync`.)
        if item.flags & (INV_ARROW | INV_EQUIPPED) == INV_ARROW | INV_EQUIPPED
            && let Some(t) = self.templates.iter().find(|t| t.kind == OBJ_ARROW).copied()
        {
            if t.rounds > self.arrows_out(t.id) {
                self.draw_arrow(i, &t, t.id);
                tracing::info!("rig: one of the bow's {} drawn from slot {k} ({name}) into the {} hand", item.name(), if i == 0 { "left" } else { "right" });
            } else {
                self.emit(Cue::Fail, Some(i), None, palm, 0.6);
            }
            return;
        }
        let id = self.next_spawn;
        self.next_spawn = self.next_spawn.wrapping_add(1).max(SPAWN_BASE);
        let desc = pockets::object_desc(&item, id);
        let (desc, pos, rot) = match self.holds.get(item.model()).copied().flatten() {
            // Rigged: in the hand where it's made to be held, the fingers round its handle (the
            // left hand the same way round, its palm on the same side: `handle_of`).
            Some(h) => {
                let held = if i == 1 { h.rot } else { (h.rot * Quat::from_rotation_z(std::f32::consts::PI)).normalize() };
                let rot = (grip_rot * held.inverse()).normalize();
                (pockets::rigged(desc, &h), palm - rot * h.grip, rot)
            }
            // Its middle in the fist; a big thing's side against the palm, the rest of it out past
            // the fingers.
            None => {
                let wpm = self.map.world_per_meter;
                let rot = (grip_rot * pockets::hold_rot(&item)).normalize();
                let to_palm = grip_rot * if i == 1 { Vec3::X } else { Vec3::NEG_X };
                let across = (rot.inverse() * to_palm).abs().dot(v3(item.half_extents));
                let centre = palm - to_palm * ((across - 0.025) * wpm).max(0.0);
                (desc, centre - rot * v3(item.center), rot)
            }
        };
        self.add(desc, id, true, pos, rot);
        if let Some(o) = self.objects.get_mut(&id) {
            o.drawn = Some(pockets::Drawn::new(&item, k));
        }
        // Held just where it came out: the hand stays where it is, the fingers close on it.
        self.take(id, i, false);
        self.debug.pocket_draws += 1;
        self.emit(Cue::PocketOut, Some(i), Some(id), palm, 1.0);
        // (Where the hand was, from the eyes in the body's frame: where players reach for a slot.)
        let reached = self.anchor.local(self.hands.as_ref().unwrap()[i].grip_t.0);
        tracing::info!("rig: {} ({} of them) drawn from slot {k} ({name}) into the {} hand as {id:#x}, reached at {reached:.2?}", item.name(), item.count, if i == 0 { "left" } else { "right" });
    }

    /// A grip held still in a slot (`Holsters::edit_hold`) asks for the slot's editor; what it drew
    /// goes back.
    fn pocket_holds(&mut self, inp: &RigInput) {
        let hold = inp.settings.holsters.edit_hold as f64;
        for i in 0..2 {
            let h = &self.hands.as_ref().unwrap()[i];
            let Some((k, t0, at, drew)) = h.pocket_press else { continue };
            let (down, now_at, holding) = (h.grip_down, h.grip_t.0, h.holding);
            if !down || now_at.distance(at) > pockets::EDIT_STILL || hold <= 0.0 {
                self.hands.as_mut().unwrap()[i].pocket_press = None;
                continue;
            }
            if self.now - t0 < hold {
                continue;
            }
            self.hands.as_mut().unwrap()[i].pocket_press = None;
            if let Some(id) = drew.filter(|&id| holding == Some(id)) {
                self.remove(id);
            }
            self.pocket_events.push(PocketEvent::Edit(k));
            let p = self.map.point(now_at);
            // (Felt, not heard: the editor opens with a sound of its own, the game's.)
            self.emit(Cue::Use, Some(i), None, p, 1.0);
            tracing::info!("rig: the grip held in slot {k}: its editor");
        }
    }

    /// A slot being placed by hand goes where the next squeeze is (in menus too: the editor's Move).
    fn place_by_hand(&mut self, inp: &RigInput) {
        let press = inp.settings.interaction.grip_press;
        let k = (inp.eye_height / 1.70).clamp(0.6, 1.3);
        for i in 0..2 {
            let down = inp.frame.hands[i].squeeze > press;
            let h = &mut self.hands.as_mut().unwrap()[i];
            let pressed = down && !h.place_grip;
            h.place_grip = down;
            let at = h.grip_t.0;
            if let (true, Some(slot)) = (pressed, self.placing) {
                self.placing = None;
                let local = self.anchor.local(at) / k;
                self.pocket_events.push(PocketEvent::Placed(slot, local));
                let w = self.map.point(at);
                self.emit(Cue::HolsterIn, Some(i), None, w, 1.0);
                tracing::info!("rig: slot {slot} placed by hand at {local:.3?}");
            }
        }
    }

    /// Things drawn from the slots, used where they're used: at the mouth (eaten, drunk), up in
    /// front of the eyes (read), on the weapon the other hand holds (a poison). Held there a moment
    /// (`Holsters::mouth_time`, `read_time`, `coat_time`), with bites or gulps felt on the way,
    /// then used once, for the game to do (its state's `shots`); never while the hand is moving
    /// fast or held back from its controller. A use the game doesn't take back in time was refused
    /// (its potion limit, a quest item: it says so itself): the thing has to leave and come back to
    /// be tried again.
    fn use_drawn(&mut self, inp: &RigInput, dt: f32) {
        let h = &inp.settings.holsters;
        let head = (v3(inp.frame.hmd.pos), q4(inp.frame.hmd.rot));
        let mouth = self.map.point(pockets::mouth(head));
        let wpm = self.map.world_per_meter;
        for i in 0..2 {
            let hands = self.hands.as_ref().unwrap();
            let Some(id) = hands[i].holding else { continue };
            let (speed, blocked) = (hands[i].history.recent().length(), hands[i].blocked);
            let weapon = hands[i ^ 1].holding.and_then(|o| self.objects.get(&o)).filter(|o| matches!(o.desc.kind, OBJ_MELEE | OBJ_WEAPON)).map(|o| o.collider);
            let Some(o) = self.objects.get(&id) else { continue };
            let Some(mut d) = o.drawn else { continue };
            let (collider, (pos, _)) = (o.collider, self.physics.pose(o.body));
            if let Some(t) = d.used_at {
                if self.now - t < pockets::USE_WAIT {
                    continue;
                }
                d.used_at = None;
                d.armed = false;
                self.emit(Cue::Fail, Some(i), Some(id), pos, 0.5);
            }
            let (time, cue) = if d.flags & (INV_EAT | INV_DRINK) != 0 && self.physics.distance_to(collider, mouth) < pockets::MOUTH_REACH * wpm {
                (h.mouth_time, if d.flags & INV_EAT != 0 { Cue::Bite } else { Cue::Gulp })
            } else if d.flags & INV_READ != 0 && pockets::readable(head, self.map.inv_point(pos)) {
                (h.read_time, Cue::Hold)
            } else if d.flags & INV_COAT != 0 && weapon.is_some_and(|w| self.physics.distance_between(collider, w) < pockets::COAT_REACH * wpm) {
                (h.coat_time, Cue::Hold)
            } else {
                (-1.0, Cue::Hold)
            };
            if time < 0.0 {
                d.armed = true;
            }
            if time < 0.0 || speed > pockets::USE_SPEED || blocked || !d.armed {
                d.held = 0.0;
                d.next_bite = 0.0;
            } else {
                d.held += dt;
                if d.held >= d.next_bite {
                    self.emit(cue, Some(i), Some(id), pos, 1.0);
                    d.next_bite += pockets::BITE_EVERY;
                }
                if d.held >= time.max(0.05) {
                    d.uses += 1;
                    d.used_at = Some(self.now);
                    d.held = 0.0;
                    d.next_bite = 0.0;
                    self.debug.pocket_uses += 1;
                    self.emit(Cue::Use, Some(i), Some(id), pos, 1.0);
                    tracing::info!("rig: {id:#x} (stack {:#x}) used ({cue:?}), for the game", d.item);
                }
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.drawn = Some(d);
            }
        }
    }

    /// Into the player's things: out of the world and the hands, until the game takes it.
    fn stow(&mut self, id: u32, slot: u32) {

        let Some(o) = self.objects.get_mut(&id) else { return };
        o.holder = Holder::Stowed(slot);
        o.embed = None;
        let (body, collider) = (o.body, o.collider);
        self.physics.set_kinematic(body, true);
        self.physics.set_groups(collider, groups_none());
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        self.debug.stowed += 1;
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
                let h = &mut self.hands.as_mut().unwrap()[i];
                if let Some(sup) = h.support.filter(|s| s.id == wid && s.on == On::Foregrip) {
                    h.support = None;
                    h.barrels = Some(BarrelGrab { gun: wid, at: sup.at, angle: 0.0 });
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
        let hands = self.hands.as_ref().unwrap();
        let (palm, _) = hands[hand].target;
        let aim = hands[hand].aim_rot;
        let o = &self.objects[&id];
        let grip = v3(o.desc.grip);
        let stocked = o.desc.weapon_flags & WPN_STOCK != 0;
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
        // Two-handed: pivoting on the firing hand, the support hand aims it.
        let support = hands[hand ^ 1].supporting().filter(|s| *s == id);
        if support.is_some() {
            (pos, rot) = self.two_handed(id, hand, grip, (pos, rot), dt);
        }
        // Virtual stock: shouldered two-handed long guns aim along the line from the shoulder to the
        // support hand, so the gun steadies like a real one braced against you.
        if settings.interaction.virtual_stock && support.is_some() && stocked {
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

    /// A thing hand `hand` holds at `pivot` (thing space), put at `pose` by that hand alone, turned
    /// about the hold so that the other hand's hold on it lies towards the other hand (steadied):
    /// two hands aim it. Steadied in the player's own space, where only the controller moves: in
    /// the world the walk moves it too, and the filter lagging the walk swung the gun (and the
    /// support hand on it) whenever the walk set off, stopped or turned.
    fn two_handed(&mut self, id: u32, hand: usize, pivot: Vec3, pose: (Vec3, Quat), dt: f32) -> (Vec3, Quat) {
        let map = self.map;
        let h = &mut self.hands.as_mut().unwrap()[hand ^ 1];
        let Some(sup) = h.support.filter(|s| s.id == id) else { return pose };
        let raw = h.grip_t.0;
        let steady = map.point(h.euro.filter(raw, dt)) + h.target.1 * sup.reach;
        let (pos, rot) = pose;
        let pivot_w = pos + rot * pivot;
        let cur = (pos + rot * sup.at - pivot_w).normalize_or_zero();
        let want = (steady - pivot_w).normalize_or_zero();
        // (Hands too near together don't aim it: their line jitters.)
        let apart = (sup.at - pivot).length() > TWO_HANDS_APART && (steady - pivot_w).length() > TWO_HANDS_APART;
        if !apart || cur.length_squared() < 0.5 || want.length_squared() < 0.5 {
            return pose;
        }
        let rot = (Quat::from_rotation_arc(cur, want) * rot).normalize();
        (pivot_w - rot * pivot, rot)
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
            // Held anywhere but its grip, the finger isn't on the trigger.
            let on_grip = matches!(self.objects[&id].attach, Attach::Gun);
            let pull = if on_grip { self.hands.as_ref().unwrap()[hand].trigger } else { 0.0 };
            let two = self.hands.as_ref().unwrap()[hand ^ 1].supporting() == Some(id);
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
                        let rot = self.support_rot(&d, i);
                        let h = &mut self.hands.as_mut().unwrap()[i];
                        h.barrels = None;
                        if Self::foregrip_on_barrels(&d) && b.at.distance(v3(d.foregrip)) < FOREGRIP_REACH {
                            h.support = Some(Support { id, at: b.at, rot, on: On::Foregrip, reach: Vec3::ZERO });
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

    // ---- bows ------------------------------------------------------------------------------------
    /// The game's bows ([`Bows`]): how each bends and how its arrows fly.
    pub fn set_bows(&mut self, table: &Bows) {
        *self.bows = *table;
    }

    /// The bows, once a frame: an arrow brought to the string of a bow held by its grip goes on it;
    /// a bow that isn't so held lets its arrow go (back into the quiver when it's put away); a bow
    /// just loosed rings in the hand as its limbs spring back.
    fn bows(&mut self, inp: &RigInput) {
        let ids: Vec<u32> = self.objects.iter().filter(|(_, o)| o.bow.is_some()).map(|(id, _)| *id).collect();
        for bid in ids {
            let (holder, st, d) = {
                let o = &self.objects[&bid];
                (o.holder, o.bow.unwrap(), o.desc)
            };
            let hands = self.hands.as_ref().unwrap();
            let bow_hand = match holder {
                Holder::Held(h) if hands[h].holding == Some(bid) && hands[h].on == On::Grip => Some(h),
                _ => None,
            };
            let Some(bh) = bow_hand else {
                if let Some(a) = st.nocked {
                    self.unnock(a, matches!(holder, Holder::Holstered(_) | Holder::Returning { .. }));
                }
                continue;
            };
            let sh = bh ^ 1;
            // An arrow for it in the other hand, its nock brought to the string (pointing anywhere
            // but back at the archer): on it goes.
            if st.nocked.is_none()
                && let Some(aid) = hands[sh].holding
                && let Some(a) = self.objects.get(&aid).filter(|a| a.desc.kind == OBJ_ARROW && a.desc.ammo_type == d.ammo_type && a.holder == Holder::Held(sh))
            {
                let (bp, bq) = self.physics.pose(self.objects[&bid].body);
                let (ap, aq) = self.physics.pose(a.body);
                let string = bp + bq * bow::nock(&d, 0.0);
                let nock_at = ap + aq * v3(a.desc.mag_well);
                if (self.now * 2.0).floor() != ((self.now - inp.dt as f64) * 2.0).floor() {
                    tracing::debug!("rig: the arrow's nock {:.3} m from the string ({nock_at:.2?}, {string:.2?}), pointing {:.2} along the bow", nock_at.distance(string), (aq * Vec3::Z).dot(bq * Vec3::Z));
                }
                if nock_at.distance(string) < bow::NOCK_REACH * self.map.world_per_meter && (aq * Vec3::Z).dot(bq * Vec3::Z) > -0.3 {
                    self.nock(bid, aid, sh);
                }
            }
            // Just loosed: a pulse in the bow hand at each swing of its limbs, as strong as the swing.
            if let (Some((at, from)), Some(b)) = (st.loosed, self.bows.get(bid).copied()) {
                let t = (self.now - at) as f32;
                let was = t - inp.dt;
                for k in b.spring[..(b.spring_count as usize).min(BOW_SPRING_KEYS)].iter().skip(1) {
                    if k[0] > was && k[0] <= t && (k[1] * from).abs() > 0.03 {
                        let (bp, _) = self.physics.pose(self.objects[&bid].body);
                        self.emit(Cue::BowSpring, Some(bh), Some(bid), bp, (k[1] * from).abs() * 2.5);
                    }
                }
            }
        }
    }

    /// Arrow `aid` in hand `hand` goes onto bow `bid`'s string, eased on from where it is; the hand
    /// holds the string now.
    fn nock(&mut self, bid: u32, aid: u32, hand: usize) {
        let (Some((bp, bq)), Some((ap, aq))) = (self.obj_pose(bid), self.obj_pose(aid)) else { return };
        let from = (bq.inverse() * (ap - bp), (bq.inverse() * aq).normalize());
        let now = self.now;
        let Some(a) = self.objects.get_mut(&aid) else { return };
        a.holder = Holder::Nocked { bow: bid };
        let (body, collider) = (a.body, a.collider);
        self.physics.set_kinematic(body, true);
        self.physics.set_groups(collider, groups_none());
        let d = self.objects[&bid].desc;
        if let Some(b) = self.objects.get_mut(&bid).and_then(|b| b.bow.as_mut()) {
            *b = bow::BowState { nocked: Some(aid), drawing: Some(hand), nocked_at: now, drawn_at: now, nocked_from: Some(from), shots: b.shots, loosed: b.loosed, full: b.full, ..Default::default() };
        }
        self.debug.nocks += 1;
        tracing::info!("rig: an arrow {aid:#x} nocked on {bid:#x} by hand {hand}");
        let at = bp + bq * bow::nock(&d, 0.0);
        self.emit(Cue::BowNock, Some(hand), Some(bid), at, 1.0);
        self.emit(Cue::BowNock, Some(hand ^ 1), Some(bid), at, 0.6);
    }

    /// The arrow on a string comes off it: into the hand drawing it if there is one, else (`stow`)
    /// back into the quiver, or falling where it is.
    fn unnock(&mut self, aid: u32, stow: bool) {
        let Some(a) = self.objects.get(&aid) else { return };
        let Holder::Nocked { bow } = a.holder else { return };
        if let Some(b) = self.objects.get_mut(&bow).and_then(|b| b.bow.as_mut()) {
            (b.nocked, b.drawing, b.draw) = (None, None, 0.0);
        }
        let hand = self.hands.as_ref().unwrap().iter().position(|h| h.holding == Some(aid));
        match hand {
            Some(i) => {
                let now = self.now;
                let a = self.objects.get_mut(&aid).unwrap();
                let (p, q) = self.physics.pose(a.body);
                a.holder = Holder::Held(i);
                a.blend = Some((p, q, now, 0.1));
                let (body, collider) = (a.body, a.collider);
                self.physics.set_kinematic(body, false);
                self.physics.set_groups(collider, groups_held(i));
            }
            None if stow => self.remove(aid),
            None => self.free_body(aid, groups_free()),
        }
    }

    /// The string hand lets go of the string drawn on bow `bid`: past the least draw the arrow is
    /// loosed along the bow (the game fires its own, [`Rig::loosed`]) and the bow springs back;
    /// short of it, the arrow stays on the string, which goes back to rest.
    fn let_string_go(&mut self, bid: u32, aid: u32, i: usize) {
        let Some((st, d, holder)) = self.objects.get(&bid).and_then(|b| Some((b.bow?, b.desc, b.holder))) else { return };
        let bow_hand = if let Holder::Held(h) = holder { h } else { i ^ 1 };
        let (Some((bp, bq)), Some((ap, aq))) = (self.obj_pose(bid), self.obj_pose(aid)) else { return };
        let string = bp + bq * bow::nock(&d, st.draw);
        let least = self.bows.get(bid).map(|b| b.min_draw).filter(|m| *m > 0.0).unwrap_or(bow::MIN_DRAW);
        let strength = st.strength();
        if strength < least {
            if let Some(b) = self.objects.get_mut(&bid).and_then(|b| b.bow.as_mut()) {
                (b.drawing, b.draw, b.ticked) = (None, 0.0, 0.0);
            }
            self.emit(Cue::Release, Some(i), Some(bid), string, 0.4);
            return;
        }
        let a = &self.objects[&aid];
        let point = ap + aq * v3(a.desc.muzzle);
        let dir = (aq * Vec3::Z).normalize();
        let arrow = a.draw_desc;
        let bend = st.bend(&d, self.bows.get(bid), self.now);
        self.loosed.push(Loose { bow: bid, arrow, hand: i, point, dir, strength });
        let now = self.now;
        if let Some(b) = self.objects.get_mut(&bid).and_then(|b| b.bow.as_mut()) {
            (b.nocked, b.drawing, b.draw, b.ticked) = (None, None, 0.0, 0.0);
            b.loosed = Some((now, bend));
            b.shots += 1;
        }
        // The runtime's arrow is gone: the game flies its own from where it was.
        self.remove(aid);
        self.debug.looses += 1;
        self.debug.last_loose = strength;
        tracing::info!("rig: an arrow loosed from {bid:#x} at {:.0}% draw, from {point:.2?} along {dir:.3?}", strength * 100.0);
        self.emit(Cue::BowRelease, Some(bow_hand), Some(bid), bp, strength.max(0.4));
        self.emit(Cue::StringSnap, Some(i), Some(bid), string, strength.max(0.4));
    }

    /// Where bow `id`, held by hand `hand` at `pivot` (its hold, bow space) and put at `alone` by
    /// that hand, goes: turned to the string hand while one draws it (eased in as the hand takes
    /// the string), its string drawn back to that hand up to the full draw. The draw ticks in both
    /// hands as it moves, and strains at full draw.
    fn bow_target(&mut self, id: u32, hand: usize, pivot: Vec3, alone: (Vec3, Quat), inp: &RigInput) -> (Vec3, Quat) {
        let Some((mut st, d)) = self.objects.get(&id).and_then(|o| Some((o.bow?, o.desc))) else { return alone };
        st.full = bow::full_draw(&d, inp.arm_length, inp.settings.archery.fit_draw);
        let Some(sh) = st.drawing else {
            self.objects.get_mut(&id).unwrap().bow = Some(st);
            return alone;
        };
        let pinch = self.pinch(sh);
        let ((_, aimed), draw) = bow::aim(&d, hand, alone, pivot, pinch, st.full);
        // Eased in as the hand takes the string, then exact.
        let k = smooth((self.now - st.drawn_at) / bow::ALIGN_TIME);
        let g = alone.0 + alone.1 * pivot;
        let rot = alone.1.slerp(aimed, k).normalize();
        st.draw = draw;
        let strength = st.strength();
        let ticks = ((st.draw - st.ticked).abs() / bow::TICK) as u32;
        if ticks > 0 {
            st.ticked = st.draw;
        }
        // At full draw a low strain now and then; drawn on past it, harder and more often the
        // tighter it gets, until it's nearly a steady rumble.
        let tight = bow::strain(st.draw, st.full);
        let strain = strength >= bow::STRAIN_AT && self.now >= st.next_strain;
        if strain {
            st.next_strain = self.now + (0.1 + 0.4 * bow::noise(self.frame_index ^ id as u64) as f64) * (1.0 - 0.75 * tight as f64);
        }
        self.objects.get_mut(&id).unwrap().bow = Some(st);
        let at = g + rot * (bow::nock(&d, draw) - pivot);
        if ticks > 0 {
            self.emit(Cue::BowCreak, Some(sh), Some(id), at, strength);
            self.emit(Cue::BowCreak, Some(hand), Some(id), at, strength);
        }
        if strain {
            self.emit(Cue::BowStrain, Some(sh), Some(id), at, tight);
            self.emit(Cue::BowStrain, Some(hand), Some(id), at, 0.8 * tight);
        }
        (g - rot * pivot, rot)
    }

    /// Where arrow `id`, in hand `hand` at `alone` (where the hand alone puts it, world), goes as
    /// its nock nears the string of a bow the other hand holds: turned about its nock, still in the
    /// fingers, to lie along the bow as it will on the string, from none at [`bow::ALIGN_REACH`] to
    /// all of it at [`bow::NOCK_REACH`], where it goes on. So it arrives at the string already lined
    /// up, as the Longbow's does, and going on only slides it the last few centimetres. None: no
    /// bow near.
    fn arrow_target(&self, id: u32, hand: usize, alone: (Vec3, Quat)) -> Option<(Vec3, Quat)> {
        let a = self.objects.get(&id).filter(|a| a.desc.kind == OBJ_ARROW)?;
        let bh = hand ^ 1;
        let h = &self.hands.as_ref()?[bh];
        let bid = h.holding.filter(|_| h.on == On::Grip)?;
        let b = self.objects.get(&bid)?;
        if b.bow.is_none_or(|st| st.nocked.is_some()) || b.desc.ammo_type != a.desc.ammo_type {
            return None;
        }
        let (bp, bq) = self.physics.pose(b.body);
        let string = bp + bq * bow::nock(&b.desc, 0.0);
        let well = v3(a.desc.mag_well);
        let nock = alone.0 + alone.1 * well;
        let wpm = self.map.world_per_meter;
        let near = smooth(((bow::ALIGN_REACH * wpm - nock.distance(string)) / ((bow::ALIGN_REACH - bow::NOCK_REACH) * wpm)) as f64);
        // (Only an arrow pointing the bow's way, as one must to go on: one pointing back at the
        // archer isn't spun round.)
        let along = (bq * (bow::rest(&b.desc, bh) - bow::nock(&b.desc, 0.0))).normalize_or(bq * Vec3::Z);
        let k = near * smooth((((alone.1 * Vec3::Z).dot(bq * Vec3::Z) + 0.3) / 0.5) as f64);
        if k <= 0.0 {
            return None;
        }
        let lined = (Quat::from_rotation_arc(alone.1 * Vec3::Z, along) * alone.1).normalize();
        let rot = alone.1.slerp(lined, k).normalize();
        Some((nock - rot * well, rot))
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

    /// A loose object set moving `dv` faster (m/s, world), on the next step: a push of its own mass
    /// times that, at its centre (a severed limb leaving the body). False if it isn't loose yet.
    pub fn kick(&mut self, id: u32, dv: Vec3) -> bool {
        let Some(o) = self.objects.get(&id).filter(|o| matches!(o.holder, Holder::Free)) else { return false };
        if !dv.is_finite() {
            return false;
        }
        let (p, q) = self.physics.pose(o.body);
        let at = p + q * v3(o.desc.center);
        self.pushes.push(Push::One { id, impulse: dv * o.desc.mass.max(0.01), at });
        true
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
            // (Not one on the game's constraints: it can't come away to the hand.)
            let pullable = o.desc.flags & OBJF_PULLABLE != 0 || (o.desc.flags & OBJF_GRABBABLE != 0 && o.desc.mass <= 8.0);
            if !pullable || o.holder != Holder::Free || o.desc.flags & OBJF_CONSTRAINED != 0 {
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
        if !s.gravity_gloves || h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.barrels.is_some() || h.valve.is_some() || h.creature.is_some() {
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

    // ---- creatures held (creatures.rs) ---------------------------------------------------------------
    /// What the game says about its creatures' bodies this frame ([`ActorBodies`], beside their
    /// parts in [`Actors`]): which the hands may take hold of, how each part turns, and how strong
    /// the player is.
    pub fn set_actor_bodies(&mut self, actors: &[Actor], bodies: &ActorBodies) {
        self.bodies.clear();
        for a in actors {
            self.bodies.insert(a.id, creatures::Body::from(a, bodies.get(a.id)));
        }
        self.game_strength = if bodies.strength > 0.0 && bodies.strength.is_finite() { bodies.strength } else { 1.0 };
    }

    /// Each hand's grab of a creature, for the game (see [`CreatureGrab`]).
    pub fn creature_grabs(&self) -> &[CreatureGrab; 2] {
        &self.grabs
    }

    /// The creature hand `hand` holds, if any.
    pub fn holds_creature(&self, hand: usize) -> Option<u32> {
        self.hands.as_ref()?[hand].creature.map(|c| c.actor)
    }

    /// Where part `part` of creature `actor` is now: its frame (see [`creatures::frame`]).
    fn part_frame(&self, actor: u32, part: usize) -> Option<(Vec3, Quat)> {
        let &(a, b, _) = self.actor_parts.get(&(actor, part))?;
        let rot = self.bodies.get(&actor).and_then(|b| b.parts.get(part)).and_then(|p| p.rot);
        Some(creatures::frame(a, b, rot))
    }

    /// Hand `i` takes hold of part `part` of creature `actor` where it is on it: neither the hand
    /// nor the body moves, and the fingers close on it from where they are.
    fn take_creature(&mut self, i: usize, actor: u32, part: usize) {
        let (Some(frame), Some(&(a, b, radius))) = (self.part_frame(actor, part), self.actor_parts.get(&(actor, part))) else { return };
        let at = self.physics.pose(self.hands.as_ref().unwrap()[i].body);
        let (on, _) = creatures::surface(a, b, radius, at.0);
        let kind = self.bodies.get(&actor).and_then(|b| b.parts.get(part)).map_or(PART_TORSO, |p| p.kind);
        let throat = creatures::on_throat(kind, a, b, on);
        self.grab_serial += 1;
        let inv = inverse(frame);
        let hold = creatures::Hold { actor, part, hand: compose(inv, at), point: inv.0 + inv.1 * on, throat, serial: self.grab_serial, since: self.now, slipping: None, strained: 0.0 };
        self.grabs[i] = CreatureGrab {
            serial: hold.serial,
            flags: GRAB_HELD | if throat { GRAB_THROAT } else { 0 },
            actor,
            part: part as u32,
            point: hold.point.to_array(),
            target: on.to_array(),
            target_rot: frame.1.to_array(),
            ..Default::default()
        };
        let h = &mut self.hands.as_mut().unwrap()[i];
        h.creature = Some(hold);
        h.returning = None;
        let c = h.collider;
        self.physics.set_groups(c, groups_none());
        self.debug.grabs += 1;
        tracing::info!("rig: the {} hand takes hold of creature {actor:#x}'s part {part} at {on:.2?}{}", if i == 0 { "left" } else { "right" }, if throat { ", by the throat" } else { "" });
        self.emit(Cue::Grab, Some(i), None, on, 0.8);
    }

    /// Hand `i` lets go of the creature it holds, with the throw (none: its grip slipped, or the
    /// creature went): its grab's last word for the game. The hand is on the body's surface where it
    /// let go, and a free hand again from there: held back by the body if its controller is in it,
    /// catching up eased as a hand held back does (never a jump, never through the body).
    fn let_go_creature(&mut self, i: usize, throw: Option<Vec3>) {
        let Some(hold) = self.hands.as_mut().unwrap()[i].creature.take() else { return };
        let g = &mut self.grabs[i];
        g.flags = (g.flags & !GRAB_HELD) | if throw.is_none() { GRAB_SLIPPED } else { 0 };
        g.vel = throw.unwrap_or(Vec3::ZERO).to_array();
        let h = &mut self.hands.as_mut().unwrap()[i];
        let from = self.physics.pose(h.body);
        h.reach_from = Some((from.0, h.target.0));
        h.blocked = false;
        let c = h.collider;
        self.physics.set_groups(c, groups_hand(i));
        tracing::info!(
            "rig: the {} hand lets go of creature {:#x}{}",
            if i == 0 { "left" } else { "right" },
            hold.actor,
            match throw {
                Some(v) => format!(", thrown at {:.2} m/s", v.length()),
                None => ": its grip slipped".into(),
            }
        );
        self.emit(Cue::Release, Some(i), None, from.0, 0.6);
    }

    /// The hands on creatures: each where it holds its part now (the body's weight holds it back
    /// from its controller), and its grab for the game: where the controller would have the point
    /// it holds, how hard it pulls and how hard it squeezes. Pulled past its reach for a moment, its
    /// grip slips.
    fn creature_holds(&mut self, inp: &RigInput) {
        let s = &inp.settings.interaction;
        let force = s.creature_strength * self.game_strength;
        let map = self.map;
        for i in 0..2 {
            let Some(hold) = self.hands.as_ref().unwrap()[i].creature else { continue };
            let Some(frame) = self.part_frame(hold.actor, hold.part) else {
                self.let_go_creature(i, None);
                continue;
            };
            let both = self.hands.as_ref().unwrap()[i ^ 1].creature.is_some_and(|c| c.actor == hold.actor);
            let now = self.now;
            let h = &mut self.hands.as_mut().unwrap()[i];
            let at = compose(frame, hold.hand);
            // Where the controller would have the part: the hold carried by it.
            let want = compose(h.target, inverse(hold.hand));
            let target = want.0 + want.1 * hold.point;
            let held = frame.0 + frame.1 * hold.point;
            let apart = h.target.0.distance(at.0);
            h.blocked = apart > BLOCKED;
            let slipping = (apart > creatures::REACH * map.world_per_meter).then(|| hold.slipping.unwrap_or(now));
            // The body's weight, felt: the further it holds the hand back, the harder.
            let strain = (apart > creatures::STRAIN_FROM * map.world_per_meter && now - hold.strained >= creatures::STRAIN_EVERY)
                .then(|| ((apart / map.world_per_meter - creatures::STRAIN_FROM) / (creatures::STRAIN_FULL - creatures::STRAIN_FROM)).clamp(0.0, 1.0));
            if let Some(c) = h.creature.as_mut() {
                c.slipping = slipping;
                if strain.is_some() {
                    c.strained = now;
                }
            }
            self.physics.set_pose(h.body, at.0, at.1);
            self.physics.set_velocity(h.body, Vec3::ZERO, Vec3::ZERO);
            self.grabs[i] = CreatureGrab {
                serial: hold.serial,
                flags: GRAB_HELD | if hold.throat { GRAB_THROAT } else { 0 } | if both { GRAB_TWO_HANDS } else { 0 },
                actor: hold.actor,
                part: hold.part as u32,
                point: hold.point.to_array(),
                force,
                target: target.to_array(),
                squeeze: creatures::squeeze_past(inp.frame.hands[i].squeeze, s.grip_press),
                target_rot: want.1.to_array(),
                vel: map.vel(h.vel_t).to_array(),
                stretch: held.distance(target),
                ang_vel: (map.rot(Quat::IDENTITY) * h.ang_t).to_array(),
                _p0: 0.0,
            };
            if let Some(k) = strain {
                self.emit(Cue::Strain, Some(i), None, at.0, k);
            }
            if slipping.is_some_and(|t| now - t > creatures::SLIP_TIME) {
                self.let_go_creature(i, None);
            }
        }
    }

    // ---- blades stuck in things (embed.rs) -----------------------------------------------------------
    /// The creatures' body parts this frame (world), for what's stuck in them.
    pub fn set_actor_parts(&mut self, parts: impl IntoIterator<Item = ((u32, usize), (Vec3, Vec3, f32))>) {
        self.actor_parts.clear();
        self.actor_parts.extend(parts);
    }

    /// The world's static collision: where a ray meets it, and what it's made of.
    /// The first of the rig's loose things (not `skip`, nothing in a hand) along a ray: (its id,
    /// how far, the surface's normal).
    pub fn thing_hit(&self, origin: Vec3, dir: Vec3, max: f32, skip: u32) -> Option<(u32, f32, Vec3)> {
        let skip_c = self.objects.get(&skip).map(|o| o.collider)?;
        let (d, c, n) = self.physics.ray_cast(origin, dir, max, skip_c)?;
        let (id, _) = self.objects.iter().find(|(_, o)| o.collider == c && matches!(o.holder, Holder::Free | Holder::Stuck { .. }))?;
        Some((*id, d, n))
    }

    pub fn world_hit(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<physics::WorldHit> {
        self.physics.world_hit(origin, dir, max)
    }

    /// The arrow a hand is drawing on a held bow: the bow, the arrow's tip and its way (world),
    /// and the draw's strength.
    pub fn drawn_arrow(&self) -> Option<(u32, Vec3, Vec3, f32)> {
        self.objects.iter().find_map(|(id, o)| {
            let st = o.bow.filter(|st| st.drawing.is_some() && matches!(o.holder, Holder::Held(_)))?;
            let a = self.objects.get(&st.nocked?)?;
            let (p, q) = self.physics.pose(a.body);
            Some((*id, p + q * v3(a.desc.muzzle), q * Vec3::Z, st.strength()))
        })
    }

    fn embed_parent(&self, into: embed::Into) -> Option<(Vec3, Quat)> {
        match into {
            embed::Into::World => Some((Vec3::ZERO, Quat::IDENTITY)),
            embed::Into::Actor { id, part } => self.actor_parts.get(&(id, part)).map(|&(a, b, _)| embed::part_frame(a, b)),
            embed::Into::Object { id } => self.obj_pose(id),
        }
    }

    /// A held object chopped or stabbed into something (the combat's): it stays in there.
    pub fn embed(&mut self, r: &embed::Request) {
        let Some(o) = self.objects.get(&r.object) else { return };
        let Holder::Held(hand) = o.holder else { return };
        if o.embed.is_some() {
            return;
        }
        let Some(parent) = self.embed_parent(r.into) else { return };
        // Turned as it was swung (as the blow was judged: what stopped it may already have tipped
        // it over), placed so the point that met the surface is at it, then in.
        let Some((_, q)) = self.swing_pose(r.object) else { return };
        let surface = (r.point - q * r.sample, q);
        let rel = compose(inverse(parent), surface);
        let axis = parent.1.inverse() * r.dir;
        let pos = surface.0 + r.dir * r.depth;
        let (body, collider) = (o.body, o.collider);
        let at = inverse(parent).0 + parent.1.inverse() * r.point;
        self.objects.get_mut(&r.object).unwrap().embed = Some(embed::Embedded { into: r.into, at, rel, axis, depth: r.depth, max_depth: r.max_depth });
        self.physics.set_kinematic(body, true);
        self.physics.set_groups(collider, groups_none());
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        self.physics.set_pose(body, pos, q);
        self.debug.embeds += 1;
        self.emit(Cue::KnifeStick, Some(hand), Some(r.object), r.point, 1.0);
    }

    /// Out of what it was stuck in: free to move again (in the hand, or falling).
    fn unembed(&mut self, id: u32) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        o.embed = None;
        let (body, collider, holder) = (o.body, o.collider, o.holder);
        self.physics.set_kinematic(body, false);
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        match holder {
            Holder::Held(hand) => self.physics.set_groups(collider, groups_held(hand)),
            _ => self.free_body(id, groups_free()),
        }
    }

    /// Where it is while stuck: at the surface, `depth` in.
    fn embedded_pose(&self, e: &embed::Embedded) -> Option<(Vec3, Quat)> {
        let parent = self.embed_parent(e.into)?;
        let surface = compose(parent, e.rel);
        Some((surface.0 + parent.1 * e.axis * e.depth, surface.1))
    }

    /// A held object stuck in something, the hand at `target`: false once it's out.
    fn hold_embedded(&mut self, id: u32, hand: usize, target: (Vec3, Quat), dt: f32) -> bool {
        let o = &self.objects[&id];
        let Some(e) = o.embed else { return false };
        let Some(parent) = self.embed_parent(e.into) else {
            self.unembed(id);
            return false;
        };
        let Some(now) = self.embedded_pose(&e) else { return false };
        let pivot = match o.attach {
            Attach::Gun => v3(o.desc.grip),
            Attach::Fixed(p, q) => inverse((p, q)).0,
        };
        let axis = parent.1 * e.axis;
        let d = (target.0 + target.1 * pivot) - (now.0 + now.1 * pivot);
        let along = d.dot(axis);
        let across = (d - axis * along).length();
        // In a creature, what the blade can't follow (across its way in, or out faster than it
        // slides) drags the creature by where it went in.
        let strain = d - axis * along.clamp(-embed::SLIDE_SPEED * dt, embed::SLIDE_SPEED * dt);
        if let embed::Into::Actor { id: actor, part } = e.into
            && strain.length() > 0.005
        {
            let entry = parent.0 + parent.1 * e.at;
            let speed = (strain.length() * embed::DRAG_RATE).min(PUSH_MAX);
            self.drags.push(GamePush { id: actor, part: part as u32 + 1, point: entry.extend(1.0).to_array(), normal: strain.normalize().extend(speed).to_array(), ..Default::default() });
        }
        // In a loose thing, the same, on the thing itself: what its point there lacks of the pull.
        if let embed::Into::Object { id: thing } = e.into
            && strain.length() > 0.005
            && let Some(t) = self.objects.get(&thing)
        {
            let entry = parent.0 + parent.1 * e.at;
            let dir = strain.normalize();
            let want = (strain.length() * embed::DRAG_RATE).min(PUSH_MAX);
            let (body, mass) = (t.body, t.desc.mass.max(0.05));
            let lacking = want - self.physics.velocity(body).0.dot(dir);
            if lacking > 0.0 {
                self.physics.apply_impulse(body, dir * (lacking * mass).min(DRAG_IMPULSE_MAX), entry);
            }
        }
        match embed::step(e.depth, e.max_depth, along, across, dt) {
            embed::Step::Out => {
                self.unembed(id);
                self.emit(Cue::Release, Some(hand), Some(id), now.0, 0.8);
                false
            }
            embed::Step::Stay(depth) => {
                let o = self.objects.get_mut(&id).unwrap();
                if let Some(e) = o.embed.as_mut() {
                    e.depth = depth;
                }
                let body = o.body;
                let surface = compose(parent, e.rel);
                self.physics.move_kinematic(body, surface.0 + axis * depth, surface.1);
                true
            }
        }
    }

    /// A let-go object stuck in something: it rides with it; if that's gone, it falls.
    fn rest_embedded(&mut self, id: u32) {
        let Some(e) = self.objects[&id].embed else { return };
        match self.embedded_pose(&e) {
            Some((p, q)) => self.physics.move_kinematic(self.objects[&id].body, p, q),
            None => self.unembed(id),
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
        let mut pulled_off = Vec::new();
        for id in &ids {
            let holder = self.objects[id].holder;
            match holder {
                Holder::Held(hand) => {
                    let target = match self.objects[id].attach {
                        Attach::Gun => self.gun_target(*id, hand, s, dt),
                        Attach::Fixed(p, q) => {
                            let alone = compose(self.hands.as_ref().unwrap()[hand].target, (p, q));
                            // A bow turns to the hand drawing its string, and an arrow nearing a
                            // bow's string turns to lie along the bow.
                            if self.objects[id].bow.is_some() {
                                self.bow_target(*id, hand, inverse((p, q)).0, alone, inp)
                            } else if let Some(t) = self.arrow_target(*id, hand, alone) {
                                t
                            } else {
                                self.two_handed(*id, hand, inverse((p, q)).0, alone, dt)
                            }
                        }
                    };
                    let o = &self.objects[id];
                    let target = match o.blend {
                        Some((bp, bq, t0, d)) if self.now - t0 < travel_time(s.interaction.grab_transition, d) as f64 => {
                            let k = smooth((self.now - t0) / travel_time(s.interaction.grab_transition, d) as f64);
                            (bp.lerp(target.0, k), bq.slerp(target.1, k))
                        }
                        _ => target,
                    };
                    // On the game's constraints (a chain's link): where the game has it, the hand on
                    // it there; where the hand would have it goes to the game, which pulls it there.
                    // Pulled where the game's physics won't let it go, the hand comes off it.
                    if self.objects[id].desc.flags & OBJF_CONSTRAINED != 0 {
                        let o = self.objects.get_mut(id).unwrap();
                        let pivot = match o.attach {
                            Attach::Gun => v3(o.desc.grip),
                            Attach::Fixed(p, q) => inverse((p, q)).0,
                        };
                        let body = o.body;
                        o.aim = Some(target);
                        if let Some(&(p, q)) = self.game_poses.get(id) {
                            self.physics.move_kinematic(body, p, q);
                        }
                        let (now_pos, now_rot) = self.physics.pose(body);
                        let o = self.objects.get_mut(id).unwrap();
                        o.lag = (target.0 + target.1 * pivot) - (now_pos + now_rot * pivot);
                        if o.lag.length() > valve::BREAK * self.map.world_per_meter {
                            pulled_off.push(hand);
                        }
                        continue;
                    }
                    // Stuck in something: it stays, sliding along the way in as the hand pushes
                    // and pulls, until it's pulled out.
                    if self.objects[id].embed.is_some() && self.hold_embedded(*id, hand, target, dt) {
                        continue;
                    }
                    let o = &self.objects[id];
                    // Weight: heavy things accelerate less (Godot's 15 -> 300 N idea, scaled by the
                    // weight setting).
                    let two = self.hands.as_ref().unwrap()[hand ^ 1].supporting() == Some(*id);
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
                    {
                        let o = self.objects.get_mut(id).unwrap();
                        o.aim = Some(target);
                        o.lag = (target.0 + target.1 * pivot) - (now_pos + now_rot * pivot);
                    }
                    // (Held back by the world, it stays held back: nothing a hand holds goes
                    // through anything.)
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
                Holder::Shown(k) => {
                    // Its middle at the slot's, turned with the body (`pockets::shown_rot`).
                    let Some(z) = self.pocket_zones.get(k) else { continue };
                    let (pt, rt) = (self.anchor.zone_center(&z.take), self.anchor.rot() * pockets::shown_rot(self.objects[id].desc.half_extents));
                    let (p, q) = (self.map.point(pt), self.map.rot(rt));
                    let o = &self.objects[id];
                    self.physics.move_kinematic(o.body, p - q * v3(o.desc.center), q);
                }
                Holder::Holstered(slot) => {
                    let (pt, rt) = self.anchor.slot_pose(slot, &zones[slot as usize], self.objects[id].desc.flags & OBJF_UPRIGHT != 0);
                    let rt = self.slung(*id, rt);
                    let grip = v3(self.objects[id].desc.grip);
                    let (p, q) = (self.map.point(pt), self.map.rot(rt));
                    let o = &self.objects[id];
                    self.physics.move_kinematic(o.body, p - q * grip, q);
                }
                Holder::Returning { slot, t0, from } => {
                    let (pt, rt) = self.anchor.slot_pose(slot, &zones[slot as usize], self.objects[id].desc.flags & OBJF_UPRIGHT != 0);
                    let rt = self.slung(*id, rt);
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
                    // (A hand that has taken something else on the way doesn't catch it: it falls
                    // by the hand once its flight is over.)
                    let empty = self.hands.as_ref().unwrap()[hand].holding.is_none();
                    if p.distance(palm) < s.interaction.glove_catch_radius && grip_held && empty && elapsed > 0.05 {
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
                Holder::Free if self.objects[id].embed.is_some() => self.rest_embedded(*id),
                Holder::Free => {
                    let o = &self.objects[id];
                    if o.hands_off_until > 0.0 && self.now > o.hands_off_until {
                        self.physics.set_groups(o.collider, physics::groups_free_of(&o.desc));
                        self.objects.get_mut(id).unwrap().hands_off_until = 0.0;
                    }
                    if self.objects[id].throw.as_ref().is_some_and(|t| t.flight.is_some()) {
                        self.fly(*id, dt, s.interaction.knife_assist);
                    }
                    // The game's own prop: where the game has it, once it's had it back (one on the
                    // game's constraints at once: it never flew off in the runtime). One on its
                    // constraints is solid to the hands, as a door's leaf is: they rest on it and
                    // push it, rather than going into it.
                    let o = &self.objects[id];
                    let constrained = o.desc.flags & OBJF_CONSTRAINED != 0;
                    if o.desc.flags & OBJF_GAME_SIMULATED != 0
                        && (constrained || self.now - o.free_since > GAME_HANDBACK as f64)
                        && let Some(&(p, q)) = self.game_poses.get(id)
                    {
                        let (body, collider) = (o.body, o.collider);
                        self.physics.set_kinematic(body, true);
                        self.physics.set_groups(collider, if constrained { physics::groups_panel() } else if self.bowls.contains_key(id) { physics::groups_game_bowl() } else { groups_game_prop() });
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
                // (An arrow on a string goes with its bow after the step; a thing in a bowl where the
                // bowl is, `bowls`.)
                Holder::Stuck { .. } | Holder::Gone | Holder::Stowed(_) | Holder::Nocked { .. } | Holder::InBowl { .. } => {}
            }
        }
        for hand in pulled_off {
            self.release(hand, inp, zones);
        }
        // The player's body, for the doors to stop on: from just under the head down past the hips.
        let body = {
            let head = v3(inp.frame.hmd.pos);
            (self.map.point(head - Vec3::Y * 0.15), self.map.point(head - Vec3::Y * BODY_LENGTH), BODY_RADIUS * self.map.world_per_meter)
        };
        self.valves(dt, s.interaction.grab_transition, s.interaction.mechanism_stiffness, body);
        self.creature_holds(inp);
        // Empty hands follow their controllers; past the leash they snap back, unless the
        // controller is through the world (a wall, the floor): then the hand stays against it.
        let now = self.now;
        let hands = self.hands.as_mut().unwrap();
        let holding = [hands[0].holding, hands[1].holding];
        for (i, h) in hands.iter_mut().enumerate() {
            // (A thumb or index lifted off the controls stands up only a little: its capsule stays
            // straight, which would only catch on what the hand rests against.)
            let fingers = rustvr_ik::FingerInput::from_hand(&inp.frame.hands[i], inp.frame.fingers[i]);
            let mut drawn = h.finger_solver.update(dt, &fingers, rustvr_ik::HoldShape::None).curl;
            drawn[0] = drawn[0].max(0.0);
            drawn[1] = drawn[1].max(0.0);
            h.drawn_curls = drawn;
            let was_on = std::mem::replace(&mut h.was_on, h.holding.or(h.support.as_ref().map(|s| s.id)));
            if h.holding.is_some() || h.support.is_some() || h.barrels.is_some() || h.valve.is_some() || h.creature.is_some() {
                h.reach_from = None;
                h.give = [0.0; 5];
                h.round = Round::No;
                h.pinned.clear();
                h.tilt = Quat::IDENTITY;
                h.tilt_to = Quat::IDENTITY;
                continue;
            }
            // Just let go of what the other hand still holds: it comes out of it, not knocked off.
            if was_on.is_some() && was_on == holding[i ^ 1] {
                h.passing = was_on;
            }
            // Gliding back to the controller after letting go of a valve.
            if let Some((p0, q0, t0, time)) = h.returning {
                let k = if time > 1e-3 { smooth((now - t0) / time as f64) } else { 1.0 };
                if k < 1.0 {
                    self.physics.set_pose(h.body, p0.lerp(h.target.0, k), q0.slerp(h.target.1, k).normalize());
                    self.physics.set_velocity(h.body, Vec3::ZERO, Vec3::ZERO);
                    h.reach_from = None;
                    continue;
                }
                h.returning = None;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
            let (pos, _) = self.physics.pose(h.body);
            if pos.distance(h.target.0) > LEASH && self.physics.reachable(head, h.target.0) {
                h.leashed_until = now + 0.1;
                self.physics.set_groups(h.collider, groups_none());
            }
            // Snapped back, it's on its controller for a moment, through whatever's between.
            if h.leashed_until > 0.0 && now <= h.leashed_until {
                self.physics.set_pose(h.body, h.target.0, h.target.1);
                self.physics.set_velocity(h.body, Vec3::ZERO, Vec3::ZERO);
                h.reach_from = None;
                h.give = [0.0; 5];
                continue;
            } else if h.leashed_until > 0.0 {
                h.leashed_until = 0.0;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
            if s.interaction.physics_hands {
                // Always turned as the controller is (a touch never tips it: a thing taken is held as
                // the hand is turned), and sent as far towards it as it gets: stopped on the world
                // and on what the other hand holds, sliding on along them. Never driven into them,
                // so a controller pushed on through a wall leaves the hand resting on it, calm.
                let passing = h.passing.filter(|id| holding[i ^ 1] == Some(*id)).and_then(|id| self.objects.get(&id)).map(|o| o.collider);
                if passing.is_none_or(|c| self.physics.distance_between(h.collider, c) > 0.0) {
                    h.passing = None;
                }
                // Loose things it's held back by (it didn't get where it was sent, pressed on them):
                // solid to it while it goes round them.
                if h.reach_from.is_some_and(|(sent, _)| pos.distance(sent) > PINNED) {
                    for c in self.physics.touching(h.collider) {
                        if !h.pinned.contains(&c) {
                            h.pinned.push(c);
                        }
                    }
                } else if !matches!(h.round, Round::Going { .. }) {
                    h.pinned.clear();
                }
                let pinned = h.pinned.clone();
                let going = matches!(h.round, Round::Going { .. });
                let solid = physics::Solid::hand(i, if going { &pinned } else { &[] });
                let side = if i == 0 { rustvr_ik::Side::Left } else { rustvr_ik::Side::Right };
                let joint = self.offsets.hand_frame(side, rustvr_ik::Pose::IDENTITY);
                let local = |curls: [f32; 5]| fingers::segments((joint.pos, joint.rot), i == 1, curls);
                let palm = fingers::palm((joint.pos, joint.rot));
                // Pressed on something, it turns with the controller a touch behind, so the
                // controller's tremor doesn't jolt it about against what it's on; free, exactly
                // with it.
                let (_, turned) = self.physics.pose(h.body);
                // (How far it was held back last frame: from where its controller was then, or short
                // of where it was sent. Moving freely, nothing: it follows the controller exactly.)
                let behind = h.reach_from.map_or(0.0, |(sent, was)| sent.distance(was).max(pos.distance(sent)));
                let pressed = behind > PRESSED;
                // (Rolled off the controller's turn while going round an edge, easing back after.)
                h.tilt = h.tilt.slerp(h.tilt_to, 1.0 - (-dt / PRESSED_TURN_TIME).exp()).normalize();
                if h.tilt_to == Quat::IDENTITY && h.tilt.angle_between(Quat::IDENTITY) < 0.01 {
                    h.tilt = Quat::IDENTITY;
                }
                let aim = (h.tilt * h.target.1).normalize();
                let rot = if pressed { turned.slerp(aim, 1.0 - (-dt / PRESSED_TURN_TIME).exp()).normalize() } else { aim };
                // Its fingers as the controller has them (eased, as the drawn hand's are), and as
                // far open as they'd be pushed (a relaxed hand's back past straight, a clenched
                // fist's hardly).
                let ease = 1.0 - (-dt / FINGER_EASE_TIME).exp();
                let drawn = h.drawn_curls;
                let held: [f32; 5] = if h.reach_from.is_some() { std::array::from_fn(|f| h.held_curls[f] + (drawn[f] - h.held_curls[f]) * ease) } else { drawn };
                h.held_curls = held;
                h.open = std::array::from_fn(|f| (held[f] - fingers::give(held[f])).max(-fingers::BACK[f]));
                // Where the palm alone gets to: the fingers bend to make room for it there, eased
                // and never faster than a finger moves, and never into anything where the hand is.
                self.physics.set_hand_shape(h.collider, palm, &[]);
                let mut intent = self.physics.reach(h.collider, rot, h.target.0, solid, passing);
                // Stuck on the wrong side of something (held back, getting no nearer, its controller
                // clear through on the far side): it finds the way round and goes round, as a hand
                // would, rather than waiting to be pulled through.
                {
                    let pinned_solid = physics::Solid::hand(i, &pinned);
                    let palm_box = rapier3d::parry::shape::Cuboid::new(physics::to_v(palm.2));
                    let palm_at = |p: Vec3, q: Quat| (p + q * palm.0, (q * palm.1).normalize());
                    let stuck = intent.distance(h.target.0) > STUCK && intent.distance(h.target.0) > pos.distance(h.target.0) - 0.002;
                    let through = || !self.physics.ball_in(h.target.0, STUCK_CLEAR, pinned_solid);
                    match h.round {
                        Round::No if stuck && through() => {
                            // Which way it's held back: the surface it's on, towards the controller.
                            let toward = h.target.0;
                            if let Some((_, n)) = self.physics.sweep(&palm_box, palm_at(intent, rot), toward - intent, pinned_solid).filter(|(_, n)| n.length_squared() > 0.5) {
                                let along = toward - intent;
                                let t0 = (along - n * along.dot(n)).try_normalize().unwrap_or_else(|| n.any_orthonormal_vector());
                                let dirs = std::array::from_fn(|k| (Quat::from_axis_angle(n, k as f32 * std::f32::consts::TAU / PROBE_DIRS as f32) * t0 + n * PROBE_LIFT).normalize());
                                h.round = Round::Looking { from: intent, toward, normal: n, dirs, tried: 0, best: None };
                            }
                        }
                        Round::Looking { from, toward, normal, dirs, tried, best } => {
                            if !stuck || toward.distance(h.target.0) > ROUND_REPLAN {
                                h.round = Round::No;
                            } else {
                                // A few ways a frame: along the surface a step at a time, until a
                                // straight way to the controller opens (the shortest way round wins).
                                let mut best = best;
                                for &dir in dirs.iter().skip(tried).take(PROBE_PER_FRAME) {
                                    let mut p = from;
                                    let mut walked = 0.0;
                                    for _ in 0..PROBE_STEPS {
                                        let step = dir * PROBE_STEP;
                                        let got = self.physics.sweep(&palm_box, palm_at(p, rot), step, pinned_solid).map_or(1.0, |(t, _)| t);
                                        p += step * got;
                                        walked += PROBE_STEP * got;
                                        if self.physics.sweep(&palm_box, palm_at(p, rot), toward - p, pinned_solid).is_none() {
                                            let cost = walked + p.distance(toward);
                                            if best.is_none_or(|(c, _)| cost < c) {
                                                best = Some((cost, dir));
                                            }
                                            break;
                                        }
                                        if got < 0.99 {
                                            break;
                                        }
                                    }
                                }
                                let tried = (tried + PROBE_PER_FRAME).min(PROBE_DIRS);
                                h.round = match (tried == PROBE_DIRS, best) {
                                    (false, _) => Round::Looking { from, toward, normal, dirs, tried, best },
                                    (true, Some((_, dir))) => Round::Going { dir, normal, toward, since: now, last: pos, still: 0.0 },
                                    (true, None) => Round::NoWay { until: now + NO_WAY_WAIT },
                                };
                            }
                        }
                        Round::Going { dir, normal, toward, since, last, still } => {
                            // Clear to go straight there now, the controller somewhere else, or no
                            // getting anywhere: done.
                            let clear = self.physics.sweep(&palm_box, palm_at(pos, rot), h.target.0 - pos, pinned_solid).is_none();
                            let still = if pos.distance(last) < ROUND_STILL_MOVE { still + dt } else { 0.0 };
                            if clear || toward.distance(h.target.0) > ROUND_REPLAN {
                                h.round = Round::No;
                                h.tilt_to = Quat::IDENTITY;
                            } else if still > ROUND_STILL || now - since > ROUND_MAX as f64 {
                                h.round = Round::NoWay { until: now + NO_WAY_WAIT };
                                h.tilt_to = Quat::IDENTITY;
                            } else {
                                // On along the surface that way (sliding round what it meets).
                                let ahead = pos + dir * ROUND_AHEAD;
                                intent = self.physics.reach(h.collider, rot, ahead, solid, passing);
                                // Caught on an edge: the wrist rolls over it, if that gets it on.
                                if intent.distance(pos) < ROUND_STILL_MOVE {
                                    let axis = normal.cross(dir).normalize_or_zero();
                                    for turn in [ROUND_TILT, -ROUND_TILT] {
                                        let tilt = Quat::from_axis_angle(axis, turn);
                                        let rolled = self.physics.reach(h.collider, (tilt * rot).normalize(), ahead, solid, passing);
                                        if rolled.distance(pos) > intent.distance(pos) + ROUND_STILL_MOVE {
                                            intent = rolled;
                                            h.tilt_to = (tilt * h.tilt_to).normalize();
                                            break;
                                        }
                                    }
                                }
                                h.round = Round::Going { dir, normal, toward, since, last: pos, still };
                            }
                        }
                        Round::NoWay { until } if now > until => h.round = Round::No,
                        Round::No | Round::NoWay { .. } => {}
                    }
                }
                fit_fingers(&self.physics, &self.offsets, i, h, (intent, rot), (pos, rot), passing, ease, FINGER_SPEED * dt);
                // The hand with its fingers as they are now, as far towards there as it gets: they
                // hold it back until they've given way, never pushed into what they're pressed on.
                self.physics.set_hand_shape(h.collider, palm, &local(h.curls));
                let want = self.physics.reach(h.collider, rot, intent, solid, passing);
                // Held back, it catches up eased, never all at once (slid off an edge, round a
                // corner, off something loose it was pressed on): from where it is, as far as its
                // controller moved and the rest a little at a time (swept again, so it goes round
                // what's in the way). Pressed on a loose thing that can't go anywhere, it presses
                // on gently rather than being driven into it.
                let to = match h.reach_from {
                    Some((_, was)) if behind > HELD_BACK && want.distance(pos) > h.target.0.distance(was) + 1e-4 => {
                        let (moved, len) = (h.target.0.distance(was), want.distance(pos));
                        let catch = 1.0 - (-dt / CATCH_UP_TIME).exp();
                        // (Held back by something loose that won't move, it presses on it gently,
                        // never hard enough to be pushed through it.)
                        let extra = (len - moved) * catch;
                        let extra = if pinned.is_empty() { extra } else { extra.min(PRESS_SPEED * dt) };
                        let goal = pos + (want - pos) * ((moved + extra) / len);
                        self.physics.reach(h.collider, rot, goal, solid, passing)
                    }
                    _ => want,
                };
                // Resting on what the other hand holds, it goes along with it: where it's put
                // against it now, carried as that moves this step (else that moving under it would
                // shove it about every frame).
                let other = holding[i ^ 1].and_then(|id| self.objects.get(&id).map(|o| (id, o.body, o.collider)));
                h.rests_on = other.filter(|&(_, _, c)| self.physics.gap_at(h.collider, (to, rot), c) < RESTING).map(|(id, ..)| id);
                let to = match other.filter(|&(id, ..)| h.rests_on == Some(id)) {
                    Some((_, body, _)) => {
                        let (op, oq) = self.physics.pose(body);
                        let (v, w) = self.physics.velocity(body);
                        let next = (op + v * dt, (Quat::from_scaled_axis(w * dt) * oq).normalize());
                        next.0 + next.1 * (oq.inverse() * (to - op))
                    }
                    None => to,
                };
                h.reach_from = Some((to, h.target.0));
                self.physics.track(h.body, to, rot, dt, &physics::Drive::FREE);
                self.physics.set_rotation(h.body, rot);
            } else {
                self.physics.set_pose(h.body, h.target.0, h.target.1);
                self.physics.set_groups(h.collider, groups_none());
                h.reach_from = None;
                h.give = [0.0; 5];
            }
        }
    }

    /// How thing `id` hangs in a holster turned `rot` (tracking space): as it's made to, except a
    /// bow, which is slung limbs along where a long gun's barrel goes (its length is its limbs').
    fn slung(&self, id: u32, rot: Quat) -> Quat {
        match self.objects.get(&id) {
            Some(o) if o.desc.kind == OBJ_BOW => (rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)).normalize(),
            _ => rot,
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


    /// Where arrow `id` on bow `bid`'s string lies (world): on the string as drawn, eased on from
    /// where it was as it went on.
    fn nocked_pose(&self, id: u32, bid: u32) -> Option<(Vec3, Quat)> {
        let (b, a) = (self.objects.get(&bid)?, self.objects.get(&id)?);
        let st = b.bow?;
        let hand = match b.holder {
            Holder::Held(h) => h,
            _ => 1,
        };
        let (bp, bq) = self.physics.pose(b.body);
        let on = bow::nocked_pose(&b.desc, &a.desc, hand, st.draw);
        let local = match st.nocked_from {
            Some((fp, fq)) if self.now - st.nocked_at < bow::NOCK_TIME => {
                let k = smooth((self.now - st.nocked_at) / bow::NOCK_TIME);
                (fp.lerp(on.0, k), fq.slerp(on.1, k).normalize())
            }
            _ => on,
        };
        Some(compose((bp, bq), local))
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
        // An arrow on a string lies where the bow (after the step) and the draw put it, eased on
        // from where it was as it went on.
        let nocked: Vec<(u32, u32)> = self.objects.iter().filter_map(|(id, o)| if let Holder::Nocked { bow } = o.holder { Some((*id, bow)) } else { None }).collect();
        for (id, bid) in nocked {
            if let Some((p, q)) = self.nocked_pose(id, bid) {
                self.physics.set_pose(self.objects[&id].body, p, q);
                self.physics.set_velocity(self.objects[&id].body, Vec3::ZERO, Vec3::ZERO);
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
            let following = o.desc.flags & OBJF_GAME_SIMULATED != 0 && o.holder == Holder::Free && (o.desc.flags & OBJF_CONSTRAINED != 0 || self.now - o.free_since > GAME_HANDBACK as f64);
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
        // A second hand stays where it took hold (a handguard, anywhere along it, or anywhere on
        // the thing); it lets go if the controller wanders off.
        for i in 0..2 {
            let Some(sup) = self.hands.as_ref().unwrap()[i].support else { continue };
            let Some(o) = self.objects.get(&sup.id) else { continue };
            let (wp, wr) = self.physics.pose(o.body);
            let target = self.hands.as_ref().unwrap()[i].target.0;
            let on_it = wp + wr * sup.at;
            let ctrl = self.hands.as_ref().unwrap()[i].target;
            if on_it.distance(ctrl.0 + ctrl.1 * sup.reach) > FOREGRIP_BREAK * self.map.world_per_meter {
                self.hands.as_mut().unwrap()[i].support = None;
                self.physics.set_groups(self.hands.as_ref().unwrap()[i].collider, groups_hand(i));
                self.emit(Cue::Release, Some(i), Some(sup.id), target, 0.6);
                continue;
            }
            let h = &self.hands.as_ref().unwrap()[i];
            let (body, collider) = (h.body, h.collider);
            self.physics.set_groups(collider, groups_none());
            self.physics.set_pose(body, on_it, (wr * sup.rot).normalize());
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
                    let nocked = matches!(o.holder, Holder::Nocked { .. });
                    let hold = match o.attach {
                        // Drawing a string: the fingers on the arrow's nock, the hand turned as its
                        // controller is (the string holds where it is, not how it's turned).
                        _ if nocked => {
                            let at = op.0 + op.1 * v3(o.desc.mag_well);
                            (at - h.target.1 * bow::PINCH * self.map.world_per_meter, h.target.1)
                        }
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
                        Attach::Fixed(p, q) => h.hold.map_or_else(|| compose(op, inverse((p, q))), |at| compose(op, at)),
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
            out.finger_give[i] = h.give;
            out.hands[i] = HandState {
                pose: pose(pt, qt),
                flags: HAND_TRACKED
                    | if h.blocked { HAND_BLOCKED } else { 0 }
                    | if h.holding.is_some() || h.support.is_some() || h.barrels.is_some() { HAND_HOLDING } else { 0 },
                holding: h.holding.or(h.supporting()).or(h.barrels.map(|b| b.gun)).unwrap_or(0),
                _pad: [0; 2],
            };
            if h.blocked && inp.settings.interaction.ghost_hands {
                out.ghosts[i] = Some(pose(h.grip_t.0, h.grip_t.1));
            }
            // (A hand on a door or a lever too: its trigger, squeezed with the grip, would use what
            // the eyes are on, the game's Activate on the lever it's already pulling.)
            out.hand_busy[i] = h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.pin.is_some() || h.barrels.is_some() || h.creature.is_some() || h.valve.is_some();
            out.holds[i] = match h.holding.and_then(|id| self.objects.get(&id)) {
                Some(o) if matches!(o.attach, Attach::Gun) => rustvr_ik::HoldShape::PistolGrip,
                Some(o) if o.desc.kind == OBJ_WEAPON => rustvr_ik::HoldShape::Foregrip,
                // A melee weapon's handle in the fist, and a bow's grip.
                Some(o) if o.desc.kind == OBJ_MELEE || o.desc.kind == OBJ_BOW => rustvr_ik::HoldShape::Foregrip,
                // An arrow pinched by its nock (drawing a string too).
                Some(o) if o.desc.kind == OBJ_ARROW => rustvr_ik::HoldShape::Small,
                Some(o) if is_round(&o.desc) => rustvr_ik::HoldShape::Small,
                Some(o) if o.desc.kind == OBJ_MAGAZINE => rustvr_ik::HoldShape::Magazine,
                // A knife's handle in the fist.
                Some(o) if o.throw.as_ref().is_some_and(|t| t.is_knife()) => rustvr_ik::HoldShape::Foregrip,
                Some(o) if o.desc.half_extents.iter().fold(0.0f32, |a, b| a.max(*b)) < 0.07 => rustvr_ik::HoldShape::Small,
                Some(_) => rustvr_ik::HoldShape::Large,
                None if h.support.is_some() || h.handle.is_some() || h.barrels.is_some() => rustvr_ik::HoldShape::Foregrip,
                None if h.pin.is_some() => rustvr_ik::HoldShape::Small,
                None => match h.creature.and_then(|c| self.actor_parts.get(&(c.actor, c.part))) {
                    Some(&(_, _, radius)) => creatures::hold_shape(radius),
                    None => rustvr_ik::HoldShape::None,
                },
            };
            if let Glove::Flight(id) = h.glove {
                out.pulled[i] = self.obj_pose(id).map(|(p, _)| map.inv_point(p));
            }
            // (A hand on a person still has the trigger's use: touch someone's shoulder to talk.)
            out.hand_targeting[i] = !matches!(h.hover, Hover::None | Hover::Creature(..)) || matches!(h.glove, Glove::Hover(_) | Glove::Locked(_) | Glove::Flight(_));
        }
        // Primary gun: the firing hand's, else any held gun.
        let gun_in = |hand: usize| hands[hand].holding.filter(|id| self.objects.get(id).is_some_and(|o| matches!(o.attach, Attach::Gun)));
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
                two_handed: hands[held_by ^ 1].supporting() == Some(id),
                scoped: o.desc.flags & OBJF_SCOPED != 0,
                laser: (o.desc.weapon_flags & WPN_LASER != 0).then(|| v3(o.desc.laser)),
            });
        }
        let _ = inp;
        self.update_grasps(&out);
        out
    }

    /// What each hand's fingers close on: the handle it's on, or the whole thing where it has it
    /// anywhere (tracking space), closing over [`GRASP_TIME`] as it takes hold and opening as it
    /// lets go.
    fn update_grasps(&mut self, out: &RigOutput) {
        let now_g: [Option<((u32, On), rustvr_avatar::Grasp)>; 2] = [0, 1].map(|i| self.grasp_now(i));
        let (map, now) = (self.map, self.now);
        let Some(hands) = self.hands.as_mut() else { return };
        for (i, g) in now_g.into_iter().enumerate() {
            let hp = out.hands[i].pose;
            let hand = (v3(hp.pos), q4(hp.rot));
            let st = &mut hands[i].grasp;
            self.grasps[i] = match g {
                Some((key, g)) => {
                    // Something new (not the same thing in the other hand's place, or elsewhere on it).
                    if st.key.map(|k| k.0) != Some(key.0) {
                        st.since = now;
                        st.fading = None;
                    }
                    st.key = Some(key);
                    let mut g = grasp_in_tracking(&map, g);
                    g.weight = smooth((now - st.since) / GRASP_TIME);
                    st.last = Some((g.moved(inverse(hand)), g.weight));
                    Some(g)
                }
                None => {
                    st.key = None;
                    if let Some((last, w)) = st.last.take() {
                        st.fading = Some((now, w, last));
                    }
                    match &st.fading {
                        Some((t0, w0, local)) if now - t0 < GRASP_TIME => {
                            let mut g = local.moved(hand);
                            g.weight = w0 * (1.0 - smooth((now - t0) / GRASP_TIME));
                            Some(g)
                        }
                        _ => {
                            st.fading = None;
                            None
                        }
                    }
                }
            };
        }
    }

    /// What hand `i`'s fingers close on now (world space), and which of them: the handle it's on,
    /// or the whole thing where it has it anywhere. A gun held by its grip keeps the index on the
    /// trigger.
    fn grasp_now(&self, i: usize) -> Option<((u32, On), rustvr_avatar::Grasp)> {
        let h = &self.hands.as_ref()?[i];
        if let Some(c) = h.creature {
            // Round the part it's on, and what of the creature is beside it (the neck's and the
            // torso's capsules at the throat).
            let (hp, _) = self.physics.pose(h.body);
            let mut g = rustvr_avatar::Grasp { fingers: rustvr_avatar::grasp::ALL_FINGERS, ..Default::default() };
            for (&(actor, part), &(a, b, radius)) in &self.actor_parts {
                if actor != c.actor || creatures::surface(a, b, radius, hp).0.distance(hp) >= creatures::GRASP_NEAR {
                    continue;
                }
                // On the throat, round the neck itself.
                let kind = self.bodies.get(&actor).and_then(|b| b.parts.get(part)).map_or(PART_TORSO, |p| p.kind);
                match creatures::neck(kind, a, b).filter(|_| c.throat) {
                    Some((na, nb)) => g.solids.push(rustvr_avatar::Solid::Capsule { a: na, b: nb, radius: creatures::THROAT_RADIUS * self.map.world_per_meter }),
                    None => g.solids.push(rustvr_avatar::Solid::Capsule { a, b, radius }),
                }
            }
            return Some(((c.actor, On::Free), g));
        }
        let (id, on) = match (h.holding, h.support) {
            (Some(id), _) => (id, h.on),
            (None, Some(sup)) => (sup.id, sup.on),
            _ => return None,
        };
        let o = self.objects.get(&id)?;
        let firing = h.holding == Some(id) && matches!(o.attach, Attach::Gun);
        let mut g = rustvr_avatar::Grasp { fingers: if firing { rustvr_avatar::grasp::ALL_FINGERS & !(1 << 1) } else { rustvr_avatar::grasp::ALL_FINGERS }, ..Default::default() };
        match self.handle_of(&o.desc, i, on).filter(|h| h.grasped) {
            Some(handle) => {
                let (op, oq) = self.physics.pose(o.body);
                g.solids.push(rustvr_avatar::Solid::Capsule { a: op + oq * handle.axis.0, b: op + oq * handle.axis.1, radius: handle.radius * self.map.world_per_meter });
            }
            // Anywhere (or held its way, with no handle): round the thing itself.
            None => self.physics.grasp_solids(o.collider, &mut g),
        }
        Some(((id, on), g))
    }

    /// Where hand `i`'s index fingertip is (world), as the drawn hand has it (straight when the
    /// finger's lifted off the trigger): what presses the buttons of a menu held in the other hand.
    pub fn index_tip(&self, i: usize) -> Option<Vec3> {
        Some(self.hands.as_ref()?[i].index_tip)
    }

    /// Testing: hand `i`'s physical fingers (each one's ends, world).
    pub fn debug_fingers(&self, i: usize) -> Vec<(Vec3, Vec3)> {
        let Some(h) = self.hands.as_ref().map(|h| &h[i]) else { return Vec::new() };
        h.fingers.iter().map(|&f| {
            let (p, q) = self.physics.pose(f);
            (p - q * Vec3::Y * 0.04, p + q * Vec3::Y * 0.04)
        }).collect()
    }

    /// Testing: how far hand `i`'s collider (palm and fingers) is from thing `id`'s.
    pub fn debug_hand_gap(&self, i: usize, id: u32) -> f32 {
        let (Some(h), Some(o)) = (self.hands.as_ref().map(|h| &h[i]), self.objects.get(&id)) else { return f32::MAX };
        self.physics.distance_between(h.collider, o.collider)
    }

    /// What each hand's fingers close on this frame (tracking space), if anything.
    pub fn grasps(&self) -> &[Option<rustvr_avatar::Grasp>; 2] {
        &self.grasps
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
            // (One on the game's constraints a hand holds: where the hand would have it, for the
            // game to pull it there.)
            let held_constrained = matches!(o.holder, Holder::Held(_)) && o.desc.flags & OBJF_CONSTRAINED != 0;
            let (p, q) = o.aim.filter(|_| held_constrained).unwrap_or_else(|| self.physics.pose(o.body));
            let (lv, av) = self.physics.velocity(o.body);
            // A valve turns on its axle at its own speed (its body is only moved there), or slides.
            let (lv, av) = o.valve.map_or((lv, av), |v| if v.slide { (v.axis() * v.vel, Vec3::ZERO) } else { (Vec3::ZERO, v.axis() * v.vel) });
            let tracking = matches!(o.holder, Holder::Held(_) | Holder::Holstered(_) | Holder::Returning { .. } | Holder::Shown(_));
            // (What's in a gun, on a bow's string or in a bowl goes with it.)
            let tracking = tracking
                || matches!(o.holder, Holder::InWeapon(w) | Holder::Nocked { bow: w } | Holder::InBowl { bowl: w, .. } if self.objects.get(&w).is_some_and(|w| matches!(w.holder, Holder::Held(_) | Holder::Holstered(_) | Holder::Returning { .. })));
            let pose_out = if tracking { pose(map.inv_point(p), map.inv_rot(q)) } else { pose(p, q) };
            let mut flags = OS_SIMULATED | if tracking { OS_TRACKING } else { 0 };
            match o.holder {
                Holder::Held(0) => flags |= OS_HELD_LEFT,
                Holder::Held(_) => flags |= OS_HELD_RIGHT,
                Holder::Holstered(_) | Holder::Returning { .. } => flags |= OS_HOLSTERED,
                Holder::InWeapon(_) => flags |= OS_IN_WEAPON,
                Holder::Pulled { .. } => flags |= OS_PULLED,
                Holder::Stuck { .. } => flags |= OS_STUCK,
                Holder::Stowed(_) => flags |= OS_STOWED,
                Holder::Nocked { .. } => flags |= OS_IN_WEAPON,
                Holder::InBowl { grinder: false, .. } => flags |= OS_IN_RECEPTACLE,
                Holder::Free | Holder::Gone | Holder::Shown(_) | Holder::InBowl { .. } => {}
            }
            if let Some(t) = &o.throw {
                flags |= t.flags_out();
            }
            if hovered.contains(id) {
                flags |= OS_HOVERED;
            }
            // A game's prop a hand has had: drawn by the runtime from then on (the game hides its).
            // One it always draws (a piece it split off the game's model), always.
            let stowed = matches!(o.holder, Holder::Stowed(_));
            let always = o.desc.flags & OBJF_DRAWN_ALWAYS != 0;
            if o.desc.kind == OBJ_PROP && o.desc.flags & OBJF_RUNTIME_DRAWS != 0 && !stowed && (always || o.handled || matches!(o.holder, Holder::Held(_) | Holder::Pulled { .. })) {
                flags |= OS_RUNTIME_DRAWN;
            }
            // The hands on a valve's rim have it, and the hand drawing a string its arrow.
            for (i, h) in hands.into_iter().flatten().enumerate() {
                if h.valve.is_some_and(|g| g.id == *id) || (h.holding == Some(*id) && matches!(o.holder, Holder::Nocked { .. })) {
                    flags |= if i == 0 { OS_HELD_LEFT } else { OS_HELD_RIGHT };
                }
            }
            if o.spawned {
                flags |= OS_SPAWNED;
            }
            if o.drawn.is_some() {
                flags |= OS_INVENTORY;
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
                // A bow: an arrow on its string or not, its shots, how far it's bent.
                None if o.bow.is_some() => {
                    let b = o.bow.unwrap();
                    (b.nocked.is_some() as u32, b.shots, 0, b.bend(&o.desc, self.bows.get(*id), self.now))
                }
                // A valve: how far it's turned (radians about its axis).
                // (Grenades: times they went off; things that open: times they were opened or shut.)
                None => (o.rounds, o.throw.as_ref().map(|t| t.detonations).unwrap_or(o.opens), 0, o.valve.map_or(0.0, |v| v.angle)),
            };
            // An air gun's pressure, a bolt's lift; a grenade's fuse.
            let fuse = match &o.weapon {
                Some(w) => w.air.map(|a| a.pressure).or(w.cycle.map(|c| c.lift)).unwrap_or(0.0),
                // A bow's draw (the strength a release would loose with).
                None if o.bow.is_some() => o.bow.unwrap().strength(),
                None => o.throw.as_ref().map(|t| t.fuse_left(self.now)).unwrap_or(0.0),
            };
            let slot = match o.holder {
                Holder::Holstered(s) | Holder::Returning { slot: s, .. } | Holder::Stowed(s) => s,
                _ => 0,
            };
            let parent = match o.holder {
                _ if o.drawn.is_some() => o.drawn.unwrap().item,
                Holder::InWeapon(w) | Holder::Nocked { bow: w } | Holder::InBowl { bowl: w, .. } => w,
                Holder::Stuck { parent, .. } => parent,
                _ => 0,
            };
            let shots = o.drawn.map_or(shots, |d| d.uses);

            // A thing in a bowl: how far it's ground, and how big to draw it. A bowl: how much of what's
            // in it is ground, and its grinding strokes.
            let (fuse, slide, shots) = match o.holder {
                Holder::InBowl { grinder: false, .. } => (o.ground, receptacle::drawn_scale(o.fit, o.ground), shots),
                _ if self.bowls.contains_key(id) => {
                    let inside = self.contents(*id);
                    let share = if inside.is_empty() { 0.0 } else { inside.iter().map(|i| self.objects[i].ground).sum::<f32>() / inside.len() as f32 };
                    (fuse, share, self.grinds.get(id).map_or(0, |g| g.strokes))
                }
                _ => match self.touches.get(id) {
                    // A sheet: its touches, and where the last one was on its card.
                    Some(t) => (t.at.1, t.at.0, t.count),
                    None => (fuse, slide, shots),
                },
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

    /// Where a held thing is swung to: where the hand drives it (as the hand swings it, before
    /// anything stops it); anything else where it is. A blow is judged by the swing (the velocity
    /// it meets the body with, before the body stops it).
    pub fn swing_pose(&self, id: u32) -> Option<(Vec3, Quat)> {
        let o = self.objects.get(&id)?;
        match (o.holder, o.aim, o.embed) {
            (Holder::Held(_), Some(aim), None) => Some(aim),
            _ => self.obj_pose(id),
        }
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
