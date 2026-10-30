//! Combat: what the player's hands do to the game's creatures, judged from what they physically do.
//! The game says where its creatures are ([`Actors`]), what each melee weapon's blade is
//! ([`MeleeTable`]) and what spells the player has ([`Spells`]). The runtime sends back hits,
//! blocks and casts ([`CombatEvent`]), and the game does the rest: damage, stagger, magicka,
//! effects, projectiles.
//!
//! **Melee**: a weapon's damagers are swept from where they were last frame to where they are now,
//! and a sweep that meets a creature's body capsule is a contact. Then:
//!
//! * the contact's impact velocity (the blade's, relative to the body) must reach the material's
//!   minimum for the damage type;
//! * an edge must be moving along its cutting direction, within 35 degrees across the blade, and
//!   meet the surface within 60 degrees (an axe's: 45 and 80);
//! * a point must be moving along the blade within 35 degrees.
//!
//! A blade at a bad angle is a weak, glancing blunt hit. Each creature takes one hit per swing from
//! each weapon, and a blade is judged as it meets the creature: one that went in too slowly or at a
//! bad angle doesn't hit later from inside it, only once it has come out and meets it again.
//!
//! **The swing is the weapon's** (`docs/research/melee.md`): a held weapon is judged on how the rig
//! moves it (`rig::Swing`: as fast as its weight and its inertia round the hand let the hand swing
//! it, its spin included), not on where the controller is, so a heavy weapon can't be flicked. And
//! the arm must be behind it: a hit counts for no more than the hand's own speed against the game's
//! swing's ([`MeleeDesc::grip_speed`]), so a still hand spinning a blade with the wrist is a weak
//! hit. The strength is the speed at the contact over a full hit's there (the game's own swing of
//! that weapon, [`Damager::full_a`]); past the power speed ([`MeleeDesc::power`]) it's a power hit.
//! A swing is the weapon moving past half a full hit's speed until it slows again: one that swung
//! as hard as a full hit and hit nothing is a [`CE_SWING`]; its direction in the player's frame goes
//! with it and with its hits (the game picks its kind of attack by it).
//!
//! A clean cut goes on through the creature it cut ([`Combat::passes`]: its body doesn't stop that
//! weapon until it's out). A hard chop of an axe's edge, or a stab of a point, stays in what it
//! went into (a creature, or the world's wood and earth): [`Combat::embeds`], held there by the rig
//! (`rig::embed`). An empty fist punches. A blade on a creature's guard ([`PART_SHIELD`]: its
//! shield or weapon) while it holds it up is a hit on its guard ([`CEF_GUARD`]), the game's block to
//! decide; otherwise the guard only stops it.
//!
//! **Blocks and parries.** The player's weapon meeting a creature's attacking weapon (or put across
//! its blow's way) is a block, and a parry when the guard's momentum into the blow is at least the
//! blow's (it struck the blow aside rather than being struck). The blow pushes the guard either way
//! ([`Combat::blows`]: the impulse of the two weapons' collision, their masses the game's); a heavy
//! blow (a power attack) landing on the player shoves whatever the hands hold the same way.
//!
//! **Magic**: the off hand, empty, is the spell hand while the game has a spell ready. Holding its
//! trigger charges the spell over the game's casting time; the charge is felt (and seen) growing,
//! with a tick when it's ready. Then the spell is delivered:
//!
//! * a spell on the player is released anywhere;
//! * a touch spell goes into the first creature the charged hand touches, or wherever it's
//!   released;
//! * a target spell goes where the hand points when the trigger is released: the line from the
//!   shoulder (raised to the eyes) through the palm. A throw bends it towards the throw. The
//!   creature nearest that line within a few degrees is aimed at exactly.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::rig::embed;
use crate::rig::{Rig, RigOutput, WorldMap};

/// Each damager is followed at this many points along it.
const SAMPLES: usize = 5;
/// A creature takes one hit per swing from a weapon: none for this long after one.
const HIT_COOLDOWN: f64 = 0.35;
/// One block per attack.
const BLOCK_COOLDOWN: f64 = 0.8;
/// A fist: a sphere this far along the hand from its joint, this big; closed past this squeeze.
const FIST_FORWARD: f32 = 0.08;
const FIST_RADIUS: f32 = 0.05;
const FIST_SQUEEZE: f32 = 0.6;
/// The damagers' angle limits (degrees): a sword's, an axe's.
const SLASH_ACROSS_DEG: f32 = 35.0;
const PIERCE_DEG: f32 = 35.0;
const NORMAL_DEG: f32 = 60.0;
const AXE_ACROSS_DEG: f32 = 45.0;
const AXE_NORMAL_DEG: f32 = 80.0;
/// An axe needs this much (m/s) whatever it meets.
const AXE_MIN_SPEED: f32 = 3.0;
/// A glancing blow's strength, against a clean one's.
const GLANCING: f32 = 0.3;
/// The hand's own speed a full-strength hit needs (m/s; PLANCK's `hitRequiredHandSpeedRoomspace`):
/// the arm behind it, not a wrist flick.
pub const DEFAULT_GRIP_SPEED: f32 = 1.0;
/// A hit needs the arm behind it moving at least this fast (m/s), and the blade moving of itself as
/// fast as the least its kind of hit needs: a tap moves the arm, but a blade resting on a creature,
/// or a creature walking into a still blade, isn't the player's hit.
const ARM_MIN: f32 = 0.5 * DEFAULT_GRIP_SPEED;
/// The hand's speed is its fastest in the last moment, fading over this long (s): the hand leads
/// the blade, its peak a little before the blade's.
const HAND_PEAK_TIME: f32 = 0.15;
/// A swing: the weapon moving past this share of a full hit's speed, until it slows below
/// `SWING_END` of its peak.
const SWING_START: f32 = 0.5;
const SWING_END: f32 = 0.6;
/// A swing doesn't start again within this long of the last one ending (s): the blade catching up
/// with the hand after a hit stopped it, or bouncing back, is no new swing.
const SWING_REST: f64 = 0.25;
/// A thrust: the point moving within this of the way the blade points (degrees).
const THRUST_DEG: f32 = 40.0;
/// The arm behind a blow or a guard weighs about this (kg; a forearm and hand): it swings the
/// creature's weapon (or is the blow itself, a creature's claws, a fist), and braces the player's
/// guard (whose own weapon the rig moves; the arm is its hold, which takes the guard back after).
const ARM_MASS: f32 = 1.5;
/// How far a blade a creature's body (or a guard) stops is held off it by its own thickness, past
/// the damager's radius (m): it's still touching.
const CONTACT_SKIN: f32 = 0.015;
/// The chest, this far under the eyes (metres): where a blow's way ends.
const CHEST_BELOW_EYES: f32 = 0.35;
/// A guard closer to the chest than this is the body itself being hit, not a block (metres).
const BODY_SPARE: f32 = 0.15;
/// The trigger charges past this and releases below that.
const TRIGGER_ON: f32 = 0.6;
const TRIGGER_OFF: f32 = 0.3;
/// A hand moving faster than this as the trigger goes is a throw.
const THROW_SPEED: f32 = 1.0;
/// Aim assist: a creature within this angle of the aim line and this far (a homing bolt could take
/// 40 degrees; a game whose bolts fly straight wants it tighter).
const AIM_ASSIST_DEG: f32 = 15.0;
const AIM_ASSIST_RANGE: f32 = 25.0;
/// The shoulder the aim line starts at: this far to the hand's side of the head (m).
const SHOULDER_SIDE: f32 = 0.18;
/// A charged hand touching a body: within this of a capsule's surface (m).
const TOUCH: f32 = 0.08;

/// The impact speed (m/s) a damage type needs against a material (the damage tables' four: flesh,
/// leather, chainmail, plate; the rest as the nearest).
pub fn min_speed(kind: u32, material: u32) -> f32 {
    let m = match material {
        MAT_FLESH | MAT_ETHEREAL => 0,
        MAT_LEATHER | MAT_WOOD => 1,
        MAT_CHAINMAIL | MAT_BONE => 2,
        _ => 3,
    };
    let table: [f32; 4] = match kind {
        DMG_SLASH => [1.5, 1.5, 3.0, 4.0],
        DMG_PIERCE => [1.0, 1.0, 1.0, 1.0],
        DMG_PUNCH => [2.0, 3.0, 4.5, 4.5],
        _ => [1.0, 4.0, 4.0, 4.0],
    };
    table[m]
}

/// The speed (m/s) a full-strength hit has, and the speed a power hit (the game's stagger) needs.
fn full_and_power(kind: u32) -> (f32, f32) {
    match kind {
        DMG_SLASH => (FULL_SLASH_SPEED, 8.0),
        DMG_PIERCE => (5.0, 6.0),
        DMG_PUNCH => (6.0, 6.0),
        _ => (8.0, 7.5),
    }
}

/// One damager meeting a body, all in world space.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    pub kind: u32,
    pub flags: u32, // DMGF_*
    /// The damager's direction along its length (towards a stab's point).
    pub along: Vec3,
    /// Slash: the direction it cuts in.
    pub edge: Vec3,
    /// The impact velocity: the damager's, relative to the body.
    pub vel: Vec3,
    /// Out of the body's surface at the contact.
    pub normal: Vec3,
    pub material: u32,
    /// A full hit's speed at this point (m/s; 0: the runtime's own for the kind), and a power hit's
    /// over it (0: the runtime's own).
    pub full: f32,
    pub power: f32,
    /// The arm behind it: the hand's own speed (m/s; infinite: not judged). A hit counts for no more
    /// than the arm gives it (a full one wants [`DEFAULT_GRIP_SPEED`], PLANCK's), and a power hit
    /// wants the hand moving as the game's own swing moves it (`grip`, m/s; 0: the runtime's own).
    pub arm: f32,
    pub grip: f32,
}

impl Contact {
    /// A contact judged on its speed alone (a full arm behind it, the runtime's own speeds).
    pub fn new(kind: u32, flags: u32, along: Vec3, edge: Vec3, vel: Vec3, normal: Vec3, material: u32) -> Contact {
        Contact { kind, flags, along, edge, vel, normal, material, full: 0.0, power: 0.0, arm: f32::INFINITY, grip: 0.0 }
    }
}

/// The speeds (m/s) of a full hit and of a power hit at a point whose full hit is `full` (0: the
/// runtime's own for `kind`), a power hit `power` times faster (0: the runtime's own).
pub fn full_and_power_at(kind: u32, full: f32, power: f32) -> (f32, f32) {
    let (f, p) = full_and_power(kind);
    let full = if full > 0.1 { full } else { f };
    (full, full * if power > 1.0 { power } else { p / f })
}

/// A hit's strength (0..1.5, 1 a full-strength hit) and its `CEF_*` flags, or None if it's no hit.
pub fn judge(c: &Contact) -> Option<(f32, u32)> {
    let speed = c.vel.length();
    if speed < 1e-3 {
        return None;
    }
    let dir = c.vel / speed;
    let within = |a: Vec3, b: Vec3, deg: f32| a.dot(b) >= deg.to_radians().cos();
    let into_surface = within(-c.normal, dir, 90.0 - 1e-3);
    let clean = match c.kind {
        DMG_SLASH => {
            // Across the blade: the velocity without its part along the blade, against the edge.
            let across = (dir - c.along * dir.dot(c.along)).normalize_or_zero();
            let ways: &[f32] = if c.flags & DMGF_BOTH_WAYS != 0 { &[1.0, -1.0] } else { &[1.0] };
            let (across_deg, normal_deg) = if c.flags & DMGF_AXE != 0 { (AXE_ACROSS_DEG, AXE_NORMAL_DEG) } else { (SLASH_ACROSS_DEG, NORMAL_DEG) };
            ways.iter().any(|&s| {
                let e = c.edge * s;
                within(across, e, across_deg) && within(-c.normal, e, normal_deg)
            })
        }
        DMG_PIERCE => within(dir, c.along, PIERCE_DEG) && within(-c.normal, c.along, NORMAL_DEG),
        _ => into_surface,
    };
    let (kind, strength_k, mut flags) = if clean {
        (c.kind, 1.0, 0)
    } else if matches!(c.kind, DMG_SLASH | DMG_PIERCE) && into_surface {
        (DMG_BLUNT, GLANCING, CEF_GLANCING)
    } else {
        return None;
    };
    let min = if clean && c.flags & DMGF_AXE != 0 { AXE_MIN_SPEED } else { min_speed(kind, c.material) };
    if speed < min {
        return None;
    }
    let (full, power) = full_and_power_at(c.kind, c.full, c.power);
    // As strong as its speed there says, and no stronger than the arm behind it.
    let strength = ((((speed - min) / (full - min).max(0.5)).clamp(0.0, 1.5)).min(c.arm / DEFAULT_GRIP_SPEED) * strength_k).max(0.05);
    if clean && speed >= power && c.arm >= c.grip.max(DEFAULT_GRIP_SPEED) {
        flags |= CEF_POWER;
    }
    if c.kind == DMG_PIERCE && clean && speed >= 2.0 {
        flags |= CEF_PENETRATED;
    }
    Some((strength, flags))
}

/// A capsule `a`..`b` of `radius` as the box around it (12 triangles), for drawing.
fn capsule_box(a: Vec3, b: Vec3, radius: f32, color: [f32; 3], out: &mut Vec<([Vec3; 3], [f32; 3])>) {
    let axis = (b - a).normalize_or(Vec3::Y);
    let (u, v) = (axis.any_orthonormal_vector() * radius, axis.cross(axis.any_orthonormal_vector()) * radius);
    let (a, b) = (a - axis * radius, b + axis * radius);
    let corner = |end: Vec3, i: i32, j: i32| end + u * i as f32 + v * j as f32;
    let ring = |end: Vec3| [corner(end, -1, -1), corner(end, 1, -1), corner(end, 1, 1), corner(end, -1, 1)];
    let (ra, rb) = (ring(a), ring(b));
    for k in 0..4 {
        let m = (k + 1) % 4;
        out.push(([ra[k], ra[m], rb[m]], color));
        out.push(([ra[k], rb[m], rb[k]], color));
    }
    out.push(([ra[0], ra[2], ra[1]], color));
    out.push(([ra[0], ra[3], ra[2]], color));
    out.push(([rb[0], rb[1], rb[2]], color));
    out.push(([rb[0], rb[2], rb[3]], color));
}

/// Testing (`RUSTVR_DEBUG_COMBAT=1`): every contact is logged with how it was judged.
fn debug_combat() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTVR_DEBUG_COMBAT").is_ok_and(|v| v == "1"))
}

/// The closest points between segments `p0..p1` and `q0..q1`: (s, t) along each, 0..1.
pub fn closest_segments(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (f32, f32) {
    let d1 = p1 - p0;
    let d2 = q1 - q0;
    let r = p0 - q0;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    if a <= 1e-9 && e <= 1e-9 {
        return (0.0, 0.0);
    }
    if a <= 1e-9 {
        return (0.0, (f / e).clamp(0.0, 1.0));
    }
    let c = d1.dot(r);
    if e <= 1e-9 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = d1.dot(d2);
    let denom = a * e - b * b;
    let mut s = if denom > 1e-9 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (s, t)
}

/// A haptic pulse for a hand.
#[derive(Clone, Copy, Debug)]
pub struct Pulse {
    pub hand: usize,
    pub seconds: f32,
    pub frequency: f32,
    pub amplitude: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CombatDebug {
    pub hits: u32,
    pub blocks: u32,
    pub casts: u32,
    pub last: Option<CombatEvent>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Caster {
    charging: bool,
    charge: f32,
    ready: bool,
    /// The palm last frame (world) and the hand's velocity.
    palm: Option<Vec3>,
    vel: Vec3,
    cooldown_until: f64,
    /// A spell went off and the trigger is still held: the next one waits for it to be let go.
    held_after_cast: bool,
}

/// A hand's spell, as it's being charged (for drawing it in the hand).
#[derive(Clone, Copy, Debug)]
pub struct Charging {
    pub spell: u32,
    pub charge: f32,
    pub color: [f32; 3],
    /// The palm, tracking space.
    pub palm: Vec3,
}

/// A held weapon's swing as it goes: from the weapon moving past `SWING_START` of a full hit's
/// speed until it slows again.
#[derive(Clone, Copy, Debug, Default)]
struct SwingTrack {
    on: bool,
    /// Its fastest so far (a full hit's speed is 1, the arm behind it counted), whether that was as
    /// fast as a power hit, and which way it went then (`SWING_*`).
    peak: f32,
    power: bool,
    dir: u32,
    /// The first creature it hit (0: none yet).
    hit: u32,
    /// When the last one ended.
    ended: f64,
}

pub struct Combat {
    actors: Box<Actors>,
    melee: Box<MeleeTable>,
    spells: Box<Spells>,
    /// Each held weapon's damager points last frame (world), by object (0, 1: the fists).
    prev: HashMap<u32, Vec<[Vec3; SAMPLES]>>,
    /// Each body part's centre last frame, by (actor, part index).
    prev_parts: HashMap<(u32, usize), Vec3>,
    /// Each attacking creature's blade last frame (its two ends): how fast its blow comes.
    prev_blades: HashMap<u32, (Vec3, Vec3)>,
    /// The creatures as of the game frame they were last published in, and when that came: they move
    /// once a game frame, so they're timed by the game's frames, not the runtime's. Their parts' and
    /// blades' velocities as of then.
    actors_frame: (u64, f64),
    part_vel: HashMap<(u32, usize), Vec3>,
    blade_vel: HashMap<u32, (Vec3, Vec3)>,
    /// Each creature's count of blows landed on the player, as last seen.
    blows_seen: HashMap<u32, u32>,
    /// The damagers in a creature last frame (actor, object, damager): judged as they went in, not
    /// again until they've come out.
    touching: std::collections::HashSet<(u32, u32, usize)>,
    touching_now: std::collections::HashSet<(u32, u32, usize)>,
    hit_until: HashMap<(u32, u32), f64>,
    block_until: HashMap<u32, f64>,
    /// Each hand's speed (its controller's, m/s): its fastest in the last moment.
    hand_speed: [f32; 2],
    swings: HashMap<u32, SwingTrack>,
    casters: [Caster; 2],
    pub pulses: Vec<Pulse>,
    pub charging: [Option<Charging>; 2],
    pub debug: CombatDebug,
    /// Blades that went in and stay in, this frame (for the rig).
    pub embeds: Vec<embed::Request>,
    /// Weapons that cut clean into a creature this frame and go on through it (for the rig):
    /// (object, actor).
    pub passes: Vec<(u32, u32)>,
    /// Blows on what the hands hold this frame (for the rig): (object, impulse N s, where; world).
    pub blows: Vec<(u32, Vec3, Vec3)>,
}

impl Default for Combat {
    fn default() -> Self {
        Combat {
            actors: bytemuck::zeroed_box(),
            melee: bytemuck::zeroed_box(),
            spells: bytemuck::zeroed_box(),
            prev: HashMap::new(),
            prev_parts: HashMap::new(),
            prev_blades: HashMap::new(),
            actors_frame: (0, 0.0),
            part_vel: HashMap::new(),
            blade_vel: HashMap::new(),
            blows_seen: HashMap::new(),
            touching: Default::default(),
            touching_now: Default::default(),
            hit_until: HashMap::new(),
            block_until: HashMap::new(),
            hand_speed: [0.0; 2],
            swings: HashMap::new(),
            casters: Default::default(),
            pulses: Vec::new(),
            charging: [None, None],
            debug: CombatDebug::default(),
            embeds: Vec::new(),
            passes: Vec::new(),
            blows: Vec::new(),
        }
    }
}

pub struct CombatInput<'a> {
    pub rig: &'a Rig,
    pub out: &'a RigOutput,
    pub map: &'a WorldMap,
    pub frame: &'a HmdFrame,
    pub dominant: usize,
    pub now: f64,
    pub dt: f32,
    /// Menus, pauses, loading: nothing is judged.
    pub paused: bool,
}

/// A world-space capsule of a creature: its id, the part's index, the part.
struct Body<'a> {
    actor: &'a Actor,
    index: usize,
    part: &'a ActorPart,
    vel: Vec3,
}

/// One damager as it hits this frame: what it is, where its points were and are, how fast each moves
/// as the weapon swings, and what a full hit there takes.
struct Hitter<'a> {
    hand: usize,
    object: u32,
    damager: usize,
    kind: u32,
    flags: u32,
    radius: f32,
    /// Along it (towards a stab's point), and the way its edge cuts.
    along: Vec3,
    edge: Vec3,
    prev: &'a [Vec3; SAMPLES],
    now: &'a [Vec3; SAMPLES],
    vel: [Vec3; SAMPLES],
    /// A full hit's speed at its two ends (0: the runtime's own), a power hit's over it (0: the
    /// runtime's own), and the arm behind it and the arm a power hit wants ([`Contact::arm`]).
    full: (f32, f32),
    power: f32,
    arm: f32,
    grip: f32,
    /// The player's frame for the swing's way: right and forward (level).
    frame: (Vec3, Vec3),
}

/// Which way a point moving at `vel` on a blade running `along` swings, in the player's frame
/// (`SWING_*`): a thrust if it goes point first and forward, else down, up, left or right, whichever
/// it goes most.
pub fn swing_dir(vel: Vec3, along: Vec3, frame: (Vec3, Vec3)) -> u32 {
    let Some(dir) = vel.try_normalize() else { return SWING_NONE };
    let (right, fwd) = frame;
    if dir.dot(along) >= THRUST_DEG.to_radians().cos() && dir.dot(fwd) > 0.0 {
        return SWING_THRUST;
    }
    let (x, y) = (dir.dot(right), dir.y);
    if y.abs() >= x.abs() {
        if y < 0.0 { SWING_DOWN } else { SWING_UP }
    } else if x < 0.0 {
        SWING_LEFT
    } else {
        SWING_RIGHT
    }
}

impl Combat {
    /// Testing (`RUSTVR_DEBUG_COMBAT=1`): what's judged, as world-space triangles with colours:
    /// every creature's body parts (green; red while it attacks, and its blade yellow), and each
    /// held weapon's damagers where they were last judged (edges orange, points magenta, blunt
    /// heads cyan, guards blue).
    pub fn debug_shapes(&self, out: &mut Vec<([Vec3; 3], [f32; 3])>) {
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        for a in &self.actors.actors[..n] {
            let attacking = a.flags & ACT_ATTACKING != 0;
            for p in &a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)] {
                let c = if attacking { [0.9, 0.2, 0.2] } else { [0.2, 0.9, 0.3] };
                capsule_box(Vec3::from_array(p.a), Vec3::from_array(p.b), p.radius, c, out);
            }
            if attacking {
                capsule_box(Vec3::from_array(a.weapon_a), Vec3::from_array(a.weapon_b), 0.02, [0.95, 0.9, 0.2], out);
            }
        }
        for (id, points) in &self.prev {
            let Some(desc) = self.melee_desc(*id) else { continue };
            for (d, p) in desc.damagers[..(desc.count as usize).min(MAX_DAMAGERS)].iter().zip(points) {
                let c = if d.flags & DMGF_GUARD != 0 {
                    [0.2, 0.4, 1.0]
                } else {
                    match d.kind {
                        DMG_SLASH => [1.0, 0.55, 0.1],
                        DMG_PIERCE => [1.0, 0.2, 0.9],
                        _ => [0.2, 0.9, 0.95],
                    }
                };
                capsule_box(p[0], p[SAMPLES - 1], d.radius.max(0.008), c, out);
            }
        }
    }

    /// Every creature's body parts (world): ((actor, part index), (a, b)).
    pub fn actor_parts(&self) -> Vec<((u32, usize), (Vec3, Vec3, f32))> {
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        self.actors.actors[..n]
            .iter()
            .flat_map(|a| a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)].iter().enumerate().map(move |(i, p)| ((a.id, i), (Vec3::from_array(p.a), Vec3::from_array(p.b), p.radius))))
            .collect()
    }

    /// The tables the game wrote (reads the link once a frame; what's unchanged stays).
    #[cfg(windows)]
    pub fn read(&mut self, link: &rustvr_protocol::link::Link) {
        link.read_actors_into(&mut self.actors);
        link.read_melee_into(&mut self.melee);
        link.read_spells_into(&mut self.spells);
    }

    /// For tests: the tables as given.
    pub fn set_tables(&mut self, actors: &Actors, melee: &MeleeTable, spells: &Spells) {
        *self.actors = *actors;
        *self.melee = *melee;
        *self.spells = *spells;
    }

    /// The hand that casts this frame (its trigger is the spell's, not the game's "use").
    pub fn spell_hands(&self, dominant: usize) -> [bool; 2] {
        let off = dominant ^ 1;
        let ready = self.selected().is_some();
        [off == 0 && ready, off == 1 && ready]
    }

    fn selected(&self) -> Option<&Spell> {
        self.spells.spells[..(self.spells.count as usize).min(MAX_SPELLS)].iter().find(|s| s.flags & SPELL_SELECTED != 0)
    }

    fn melee_desc(&self, id: u32) -> Option<&MeleeDesc> {
        self.melee.weapons[..(self.melee.count as usize).min(MAX_MELEE)].iter().find(|w| w.id == id && w.count > 0)
    }

    /// How strong the player's arms are, as the game says (1: the runtime's own).
    pub fn arm(&self) -> f32 {
        if self.melee.arm > 0.05 { self.melee.arm } else { 1.0 }
    }

    /// One frame: hits, blocks, swings and casts (each returned for the game, and counted).
    pub fn update(&mut self, inp: &CombatInput) -> Vec<CombatEvent> {
        self.pulses.clear();
        self.embeds.clear();
        self.passes.clear();
        self.blows.clear();
        let mut events = Vec::new();
        let dt = inp.dt.max(1e-4);
        // The hands' own speeds (the arm behind a swing), their fastest in the last moment.
        let fade = (-dt / HAND_PEAK_TIME).exp();
        for h in 0..2 {
            let v = Vec3::from_array(inp.frame.hands[h].vel).length();
            self.hand_speed[h] = if v.is_finite() { v.max(self.hand_speed[h] * fade) } else { 0.0 };
        }
        // The bodies, with how fast each part moves: measured from one game frame to the next (the
        // game moves them once a frame, at its own pace), kept between.
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        let actors = self.actors.actors;
        let fresh = self.actors.frame != self.actors_frame.0;
        let since = (inp.now - self.actors_frame.1).clamp(1e-3, 0.25) as f32;
        if fresh {
            self.actors_frame = (self.actors.frame, inp.now);
            let mut parts_now = HashMap::new();
            let mut blades_now = HashMap::new();
            self.part_vel.clear();
            self.blade_vel.clear();
            for a in &actors[..n] {
                for (i, p) in a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)].iter().enumerate() {
                    let c = (Vec3::from_array(p.a) + Vec3::from_array(p.b)) * 0.5;
                    if let Some(v) = self.prev_parts.get(&(a.id, i)).map(|&q| (c - q) / since).filter(|v| v.length() < 30.0) {
                        self.part_vel.insert((a.id, i), v);
                    }
                    parts_now.insert((a.id, i), c);
                }
                if a.flags & ACT_ATTACKING != 0 {
                    let (wa, wb) = (Vec3::from_array(a.weapon_a), Vec3::from_array(a.weapon_b));
                    blades_now.insert(a.id, (wa, wb));
                    if let Some(&(pa, pb)) = self.prev_blades.get(&a.id) {
                        let (va, vb) = ((wa - pa) / since, (wb - pb) / since);
                        // (Taken against the player's arm, as the game says its swings are.)
                        let k = if self.actors.blow_scale > 0.01 { self.actors.blow_scale } else { 1.0 };
                        if va.length() < 60.0 && vb.length() < 60.0 {
                            self.blade_vel.insert(a.id, (va * k, vb * k));
                        }
                    }
                }
            }
            self.prev_parts = parts_now;
            self.prev_blades = blades_now;
        }
        let mut bodies = Vec::new();
        for a in &actors[..n] {
            for (i, p) in a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)].iter().enumerate() {
                let vel = self.part_vel.get(&(a.id, i)).copied().unwrap_or(Vec3::from_array(a.velocity));
                bodies.push(Body { actor: a, index: i, part: p, vel });
            }
        }
        // How fast each attacking creature's blade moves (its two ends).
        let blades = self.blade_vel.clone();
        // The player's frame (level): which way a swing goes.
        let head_rot = inp.map.rot(Quat::from_array(inp.frame.hmd.rot));
        let fwd = (head_rot * Vec3::Z * Vec3::new(1.0, 0.0, 1.0)).normalize_or(Vec3::Z);
        let right = (head_rot * Vec3::X * Vec3::new(1.0, 0.0, 1.0)).normalize_or(Vec3::X);
        let frame = (right, fwd);
        let mut seen = HashMap::new();
        for h in 0..2 {
            let held = inp.rig.held(h);
            let weapon = held.and_then(|id| Some((id, *self.melee_desc(id)?)));
            if let Some((id, desc)) = weapon {
                // Where the weapon is and how it swings: as the rig moves it (as fast as its weight
                // lets the hand swing it, before anything stopped it). Stuck in something, it doesn't.
                let swing = inp.rig.swing(id);
                let Some((wp, wq)) = swing.map(|s| (s.pos, s.rot)).or_else(|| inp.rig.swing_pose(id)) else { continue };
                let damagers = &desc.damagers[..(desc.count as usize).min(MAX_DAMAGERS)];
                let now: Vec<[Vec3; SAMPLES]> = damagers
                    .iter()
                    .map(|d| {
                        let (a, b) = (Vec3::from_array(d.a), Vec3::from_array(d.b));
                        std::array::from_fn(|k| wp + wq * a.lerp(b, k as f32 / (SAMPLES - 1) as f32))
                    })
                    .collect();
                let vels: Vec<[Vec3; SAMPLES]> = now.iter().map(|pts| pts.map(|p| swing.map_or(Vec3::ZERO, |s| s.velocity_at(p)))).collect();
                // The arm behind it: this hand's speed (or the other's, with both on it); a power
                // hit wants it as fast as the game's own swing moves its grip.
                let grip = desc.grip_speed.max(DEFAULT_GRIP_SPEED);
                let other = if inp.rig.two_hands_on(id) { self.hand_speed[h ^ 1] } else { 0.0 };
                let arm = self.hand_speed[h].max(other);
                // Its fastest point this frame, against a full hit there.
                let mut fastest = (0.0f32, Vec3::ZERO, Vec3::ZERO, f32::INFINITY);
                for (di, d) in damagers.iter().enumerate() {
                    if d.flags & DMGF_GUARD != 0 {
                        continue;
                    }
                    let along = (wq * (Vec3::from_array(d.b) - Vec3::from_array(d.a))).normalize_or_zero();
                    for k in 0..SAMPLES {
                        let t = k as f32 / (SAMPLES - 1) as f32;
                        let (full, power) = full_and_power_at(d.kind, d.full_a + (d.full_b - d.full_a) * t, desc.power);
                        let r = (vels[di][k].length() / full).min(arm / DEFAULT_GRIP_SPEED);
                        if r > fastest.0 {
                            // (A power swing wants the arm too.)
                            fastest = (r, vels[di][k], along, if arm >= grip { power / full } else { f32::INFINITY });
                        }
                    }
                }
                if !inp.paused {
                    self.track_swing(h, id, fastest, frame, inp.now, &mut events);
                }
                let prev = self.prev.get(&id).filter(|v| v.len() == now.len()).cloned();
                if let (Some(prev), false) = (prev, inp.paused) {
                    for (di, d) in damagers.iter().enumerate() {
                        if d.flags & DMGF_GUARD != 0 {
                            continue;
                        }
                        let along = (wq * Vec3::from_array(d.b) - wq * Vec3::from_array(d.a)).normalize_or_zero();
                        let edge = (wq * Vec3::from_array(d.edge)).normalize_or_zero();
                        let hitter = Hitter {
                            hand: h,
                            object: id,
                            damager: di,
                            kind: d.kind,
                            flags: d.flags,
                            radius: d.radius,
                            along,
                            edge,
                            prev: &prev[di],
                            now: &now[di],
                            vel: vels[di],
                            full: (d.full_a, d.full_b),
                            power: desc.power,
                            arm,
                            grip,
                            frame,
                        };
                        let hit = self.sweep(inp, &bodies, &hitter, &mut events);
                        // A clean cut goes on through what it cut (an axe's chop stays in, below).
                        if let Some((actor, ..)) = hit
                            && d.kind == DMG_SLASH
                            && d.flags & DMGF_AXE == 0
                        {
                            self.passes.push((id, actor));
                        }
                        // An axe's edge chopping in, or a point stabbing in, hard enough: it stays.
                        let stab = d.kind == DMG_PIERCE;
                        let chop = d.kind == DMG_SLASH && d.flags & DMGF_AXE != 0;
                        if !(stab || chop) || self.embeds.iter().any(|e| e.object == id) {
                            continue;
                        }
                        let (way, speed_needed, depth, max_depth) = if stab {
                            (along, embed::STAB_SPEED, (d.depth * 0.5).max(0.03), if d.depth > 0.0 { d.depth } else { embed::STAB_MAX_DEPTH })
                        } else {
                            (edge, embed::AXE_SPEED, embed::AXE_DEPTH, embed::AXE_MAX_DEPTH)
                        };
                        let local = |p: Vec3| {
                            // The point of the damager nearest where it met.
                            let (a, b) = (Vec3::from_array(d.a), Vec3::from_array(d.b));
                            let l = wq.inverse() * (p - wp);
                            let t = if (b - a).length_squared() > 1e-9 { ((l - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                            a.lerp(b, t)
                        };
                        if let Some((actor, part, point, vel, material)) = hit {
                            if vel.length() >= speed_needed && biteable(material) {
                                self.embeds.push(embed::Request { object: id, into: embed::Into::Actor { id: actor, part }, point, dir: way, sample: local(point), depth, max_depth });
                            }
                            continue;
                        }
                        if let Some((point, _)) = self.bite_world(inp, &hitter, dt, speed_needed) {
                            self.embeds.push(embed::Request { object: id, into: embed::Into::World, point, dir: way, sample: local(point), depth, max_depth });
                            continue;
                        }
                        if let Some((thing, point)) = Self::bite_thing(inp, &hitter, dt, speed_needed) {
                            self.embeds.push(embed::Request { object: id, into: embed::Into::Object { id: thing }, point, dir: way, sample: local(point), depth, max_depth });
                        }
                    }
                    self.blocks(inp, h, id, &desc, &now, &vels, &blades, &mut events);
                }
                seen.insert(id, now);
            } else if held.is_none() {
                // An empty hand closed into a fist punches.
                let hs = &inp.out.hands[h];
                let fist = inp.frame.hands[h].squeeze > FIST_SQUEEZE && hs.flags & HAND_TRACKED != 0;
                let (hp, hq) = (Vec3::from_array(hs.pose.pos), Quat::from_array(hs.pose.rot));
                let k = inp.map.point(hp + hq * Vec3::Y * FIST_FORWARD);
                let now = [[k; SAMPLES]];
                let key = h as u32;
                if fist && !inp.paused
                    && let Some(prev) = self.prev.get(&key).cloned()
                {
                    let along = inp.map.rot(hq) * Vec3::Y;
                    let vel = [(now[0][0] - prev[0][0]) / dt; SAMPLES];
                    let hitter = Hitter {
                        hand: h,
                        object: 0,
                        damager: h,
                        kind: DMG_PUNCH,
                        flags: 0,
                        radius: FIST_RADIUS,
                        along,
                        edge: along,
                        prev: &prev[0],
                        now: &now[0],
                        vel,
                        full: (0.0, 0.0),
                        power: 0.0,
                        arm: f32::INFINITY,
                        grip: 0.0,
                        frame,
                    };
                    self.sweep(inp, &bodies, &hitter, &mut events);
                }
                seen.insert(key, now.to_vec());
            }
        }
        // Swings in hands that let go are over.
        self.swings.retain(|id, _| seen.contains_key(id));
        self.prev = seen;
        self.touching = std::mem::take(&mut self.touching_now);
        if !inp.paused {
            self.landed(inp, &blades);
        }
        self.magic(inp, &bodies, &mut events);
        self.hit_until.retain(|_, &mut t| t > inp.now);
        self.block_until.retain(|_, &mut t| t > inp.now);
        events
    }

    /// A held weapon's swing this frame, its fastest point at `fastest` (its share of a full hit,
    /// that point's velocity, the blade's way there, and a power hit's share): a swing goes on until
    /// it slows; one that swung as hard as a full hit is the game's attack ([`CE_SWING`]), whatever
    /// it hit.
    #[allow(clippy::too_many_arguments)]
    fn track_swing(&mut self, hand: usize, object: u32, fastest: (f32, Vec3, Vec3, f32), frame: (Vec3, Vec3), now: f64, events: &mut Vec<CombatEvent>) {
        let (ratio, vel, along, power) = fastest;
        let t = self.swings.entry(object).or_default();
        if !t.on {
            if ratio >= SWING_START && now - t.ended >= SWING_REST {
                *t = SwingTrack { on: true, peak: ratio, power: ratio >= power, dir: swing_dir(vel, along, frame), hit: 0, ended: t.ended };
            }
            return;
        }
        if ratio > t.peak {
            t.peak = ratio;
            t.dir = swing_dir(vel, along, frame);
            t.power |= ratio >= power;
        }
        if ratio >= t.peak * SWING_END && ratio >= SWING_START * 0.5 {
            return;
        }
        let done = std::mem::take(t);
        t.ended = now;
        if done.peak < 1.0 {
            return;
        }
        let ev = CombatEvent {
            kind: CE_SWING,
            flags: if done.power { CEF_POWER } else { 0 },
            actor: done.hit,
            object,
            hand: hand as u32,
            strength: done.peak,
            swing: done.dir,
            ..Default::default()
        };
        if debug_combat() {
            tracing::info!("combat: a swing of {object:#x} peaked at {:.2} of a full hit{} going {} (hit {:#x})", done.peak, if done.power { ", a power swing" } else { "" }, done.dir, done.hit);
        }
        events.push(ev);
    }

    /// One damager's sweep (its sample points from `prev` to `now`) against every body.
    /// A clean hit: (actor, part index, where, the impact velocity, the part's material).
    fn sweep(&mut self, inp: &CombatInput, bodies: &[Body], hit: &Hitter, events: &mut Vec<CombatEvent>) -> Option<(u32, usize, Vec3, Vec3, u32)> {
        let (prev, now, object) = (hit.prev, hit.now, hit.object);
        // The earliest contact of any sample with any body (a creature it's still in from last
        // frame is no new contact: it stays in, unjudged).
        let mut best: Option<(f32, usize, usize, Vec3, Vec3, Vec3)> = None;
        for body in bodies {
            // A creature's guard (its shield, its weapon) is hit only while it holds it up (a
            // swinging weapon meeting the blade is a block, `Combat::blocks`); otherwise it only
            // stops the blade.
            let guard = body.part.kind == PART_SHIELD;
            if guard && (body.actor.flags & ACT_BLOCKING == 0 || body.actor.flags & ACT_ATTACKING != 0) {
                continue;
            }
            let key = (body.actor.id, object);
            let (a, b) = (Vec3::from_array(body.part.a), Vec3::from_array(body.part.b));
            // (A blade the creature's body stops is held off its surface by its own thickness.)
            let reach = body.part.radius + hit.radius + CONTACT_SKIN;
            let contact = |k: usize| {
                let (s, t) = closest_segments(prev[k], now[k], a, b);
                let p = prev[k].lerp(now[k], s);
                let on_axis = a.lerp(b, t);
                (s, p, on_axis, p.distance(on_axis) <= reach)
            };
            let entry = (body.actor.id, object, hit.damager);
            if self.touching.contains(&entry) {
                if (0..SAMPLES).any(|k| contact(k).3) {
                    self.touching_now.insert(entry);
                }
                continue;
            }
            if self.hit_until.get(&key).is_some_and(|&t| t > inp.now) {
                continue;
            }
            for k in 0..SAMPLES {
                let (s, p, on_axis, touches) = contact(k);
                if touches && best.is_none_or(|(bs, ..)| s < bs) {
                    let vel = hit.vel[k] - body.vel;
                    let normal = (p - on_axis).normalize_or(-vel.normalize_or_zero());
                    best = Some((s, bodies.iter().position(|x| std::ptr::eq(x, body)).unwrap(), k, p, vel, normal));
                }
            }
        }
        let (_, bi, k, point, vel, normal) = best?;
        let body = &bodies[bi];
        let guard = body.part.kind == PART_SHIELD;
        self.touching_now.insert((body.actor.id, object, hit.damager));
        // Only a swing hits: the arm moving, the blade going as fast as its kind of hit ever needs.
        if hit.arm < ARM_MIN || hit.vel[k].length() < min_speed(hit.kind, MAT_FLESH) {
            return None;
        }
        let t = k as f32 / (SAMPLES - 1) as f32;
        let contact = Contact {
            kind: hit.kind,
            flags: hit.flags,
            along: hit.along,
            edge: hit.edge,
            vel,
            normal,
            material: body.part.material,
            full: hit.full.0 + (hit.full.1 - hit.full.0) * t,
            power: hit.power,
            arm: hit.arm,
            grip: hit.grip,
        };
        let judged = judge(&contact);
        if debug_combat() {
            let dir = vel.normalize_or_zero();
            let across = (dir - hit.along * dir.dot(hit.along)).normalize_or_zero();
            let deg = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
            tracing::info!(
                "combat: contact kind {} with {:#x} part {} ({}): {:.1} m/s (a full hit {:.1}, the hand {:.1} m/s), across the blade vs its edge {:.0} deg, into the surface vs the edge {:.0} deg, along the blade vs the way it moves {:.0} deg -> {judged:?}",
                hit.kind,
                body.actor.id,
                body.index,
                body.part.kind,
                vel.length(),
                full_and_power_at(hit.kind, contact.full, hit.power).0,
                hit.arm,
                deg(across, hit.edge),
                deg(-normal, hit.edge),
                deg(dir, hit.along)
            );
        }
        let (strength, mut cef) = judged?;
        if guard {
            cef |= CEF_GUARD;
        }
        self.hit_until.insert((body.actor.id, object), inp.now + HIT_COOLDOWN);
        // A stab is a thrust whichever way the arm went; otherwise the way the blade went.
        let swing = if hit.kind == DMG_PIERCE && cef & CEF_GLANCING == 0 { SWING_THRUST } else { swing_dir(hit.vel[k], hit.along, hit.frame) };
        if let Some(t) = self.swings.get_mut(&object).filter(|t| t.on && t.hit == 0) {
            t.hit = body.actor.id;
        }
        let ev = CombatEvent {
            kind: CE_HIT,
            flags: cef,
            actor: body.actor.id,
            part: body.part.game_part,
            object,
            what: if cef & CEF_GLANCING != 0 { DMG_BLUNT } else { hit.kind },
            hand: hit.hand as u32,
            material: body.part.material,
            strength,
            speed: vel.length(),
            swing,
            point: point.to_array(),
            dir: vel.normalize_or_zero().to_array(),
            ..Default::default()
        };
        self.debug.hits += 1;
        self.debug.last = Some(ev);
        self.pulses.push(Pulse { hand: hit.hand, seconds: 0.05 + 0.07 * strength.min(1.0), frequency: 110.0, amplitude: (0.35 + 0.55 * strength).min(1.0) });
        events.push(ev);
        (cef & (CEF_GLANCING | CEF_GUARD) == 0).then_some((body.actor.id, body.index, point, vel, body.part.material))
    }

    /// A damager's sweep against the world's static collision: where it chopped (an axe's edge,
    /// leading, into the surface: the axe's angle limits) or stabbed (along the blade) into
    /// something it bites, at least `speed` fast: (where, its material). Each point goes the way it
    /// was swung (the world may have stopped it short of the surface).
    fn bite_world(&self, inp: &CombatInput, hit: &Hitter, dt: f32, speed: f32) -> Option<(Vec3, u32)> {
        let within = |a: Vec3, b: Vec3, deg: f32| a.dot(b) >= deg.to_radians().cos();
        for k in 0..SAMPLES {
            let way = hit.vel[k] * dt;
            let len = way.length();
            if len < 1e-4 || len / dt < speed {
                continue;
            }
            let Some(w) = inp.rig.world_hit(hit.prev[k], way, len + CONTACT_SKIN) else { continue };
            let dir = way / len;
            // (What the game didn't say the material of, a blade bites as wood.)
            let material = w.material.unwrap_or(MAT_WOOD);
            if !biteable(material) {
                return None;
            }
            let clean = if hit.kind == DMG_PIERCE {
                within(dir, hit.along, PIERCE_DEG) && within(-w.normal, hit.along, NORMAL_DEG)
            } else {
                let across = (dir - hit.along * dir.dot(hit.along)).normalize_or_zero();
                let (across_deg, normal_deg) = if hit.flags & DMGF_AXE != 0 { (AXE_ACROSS_DEG, AXE_NORMAL_DEG) } else { (SLASH_ACROSS_DEG, NORMAL_DEG) };
                within(across, hit.edge, across_deg) && within(-w.normal, hit.edge, normal_deg)
            };
            return clean.then_some((hit.prev[k] + dir * w.distance, material));
        }
        None
    }

    /// A damager's sweep against the rig's loose things (a crate, a barrel): where it chopped or
    /// stabbed into one (as [`Combat::bite_world`] judges it), at least `speed` fast: (the thing,
    /// where).
    fn bite_thing(inp: &CombatInput, hit: &Hitter, dt: f32, speed: f32) -> Option<(u32, Vec3)> {
        let within = |a: Vec3, b: Vec3, deg: f32| a.dot(b) >= deg.to_radians().cos();
        for k in 0..SAMPLES {
            let way = hit.vel[k] * dt;
            let len = way.length();
            if len < 1e-4 || len / dt < speed {
                continue;
            }
            let Some((thing, dist, normal)) = inp.rig.thing_hit(hit.prev[k], way, len + CONTACT_SKIN, hit.object) else { continue };
            let dir = way / len;
            let clean = if hit.kind == DMG_PIERCE {
                within(dir, hit.along, PIERCE_DEG) && within(-normal, hit.along, NORMAL_DEG)
            } else {
                let across = (dir - hit.along * dir.dot(hit.along)).normalize_or_zero();
                let (across_deg, normal_deg) = if hit.flags & DMGF_AXE != 0 { (AXE_ACROSS_DEG, AXE_NORMAL_DEG) } else { (SLASH_ACROSS_DEG, NORMAL_DEG) };
                within(across, hit.edge, across_deg) && within(-normal, hit.edge, normal_deg)
            };
            return clean.then_some((thing, hit.prev[k] + dir * dist));
        }
        None
    }

    /// The weapon meeting a creature's attacking weapon, or put across the blow's way to the
    /// player: from its weapon to the player's chest (a game whose blows land by reach, where the
    /// weapon never touches the player, is blocked by a guard between them). A parry when the guard
    /// goes into the blow with at least the blow's momentum (it strikes the blow aside), a block
    /// otherwise; either way the blow pushes the guard ([`Combat::blows`]).
    #[allow(clippy::too_many_arguments)]
    fn blocks(&mut self, inp: &CombatInput, hand: usize, object: u32, desc: &MeleeDesc, now: &[[Vec3; SAMPLES]], vels: &[[Vec3; SAMPLES]], blades: &HashMap<u32, (Vec3, Vec3)>, events: &mut Vec<CombatEvent>) {
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        let chest = inp.map.point(Vec3::from_array(inp.frame.hmd.pos)) - Vec3::Y * CHEST_BELOW_EYES;
        let m_guard = inp.rig.mass_of(object).max(0.1);
        for actor in &self.actors.actors[..n] {
            if actor.flags & ACT_ATTACKING == 0 || self.block_until.get(&actor.id).is_some_and(|&t| t > inp.now) {
                continue;
            }
            let (wa, wb) = (Vec3::from_array(actor.weapon_a), Vec3::from_array(actor.weapon_b));
            // The blow's way: the weapon, then on to the player (short of the body itself).
            let to_chest = chest - wb;
            let reach_end = wb + to_chest * (1.0 - (BODY_SPARE / to_chest.length().max(1e-3)).min(1.0));
            let ways = [(wa, wb), (wb, reach_end)];
            let (va, vb) = blades.get(&actor.id).copied().unwrap_or((Vec3::ZERO, Vec3::ZERO));
            for (di, d) in desc.damagers[..now.len()].iter().enumerate() {
                let (a, b) = (now[di][0], now[di][SAMPLES - 1]);
                let Some((p, t, on_blade)) = ways.iter().enumerate().find_map(|(w, &(wa, wb))| {
                    let (s, t) = closest_segments(a, b, wa, wb);
                    let (p, q) = (a.lerp(b, s), wa.lerp(wb, t));
                    (p.distance(q) <= actor.weapon_radius + d.radius + CONTACT_SKIN).then_some((p, t, w == 0))
                }) else {
                    continue;
                };
                let s = (p - a).dot(b - a) / (b - a).length_squared().max(1e-8);
                let k = ((s.clamp(0.0, 1.0) * (SAMPLES - 1) as f32).round() as usize).min(SAMPLES - 1);
                let guard = vels[di][k];
                // The blow there (on its blade, or the blade's tip on its way on to the player), and
                // the way it comes (towards the player when it's hardly moving).
                let blow = if on_blade { va.lerp(vb, t) } else { vb };
                let way = blow.try_normalize().unwrap_or_else(|| (chest - wb).normalize_or(Vec3::Y));
                let m_blow = actor.weapon_mass.max(0.0) + ARM_MASS;
                let (into_guard, into_blow) = (blow.dot(way).max(0.0), (-guard.dot(way)).max(0.0));
                // Struck aside: the guard swung across its blade (the arm behind it), with at least
                // the momentum the blow has against the way the guard goes (none, struck from the
                // side; all of it, met head on).
                let blade = (wb - wa).normalize_or_zero();
                let across = guard - blade * guard.dot(blade);
                let against = (-blow.dot(across.normalize_or_zero())).max(0.0);
                let parry = across.length() >= DEFAULT_GRIP_SPEED && (m_guard + ARM_MASS) * across.length() >= m_blow * against;
                // The two meet: the guard takes their collision's impulse along the blow.
                let closing = (blow - guard).dot(way);
                if closing > 0.0 {
                    let mu = m_guard * m_blow / (m_guard + m_blow);
                    self.blows.push((object, way * mu * closing, p));
                }
                self.block_until.insert(actor.id, inp.now + BLOCK_COOLDOWN);
                let ev = CombatEvent {
                    kind: CE_BLOCK,
                    flags: if parry { CEF_PARRY } else { 0 },
                    actor: actor.id,
                    object,
                    hand: hand as u32,
                    // How far the guard beat the blow (1: an even meeting).
                    strength: if into_guard > 1e-3 { (m_guard * into_blow / (m_blow * into_guard)).min(1.5) } else { 1.5 },
                    speed: closing.max(0.0),
                    point: p.to_array(),
                    dir: way.to_array(),
                    ..Default::default()
                };
                if debug_combat() {
                    tracing::info!(
                        "combat: {:#x}'s blow ({:.1} kg at {:.1} m/s) met by the guard ({m_guard:.1} kg at {into_blow:.1} m/s into it, {:.1} across its blade): {}",
                        actor.id,
                        m_blow,
                        into_guard,
                        across.length(),
                        if parry { "a parry" } else { "a block" }
                    );
                }
                self.debug.blocks += 1;
                self.debug.last = Some(ev);
                self.pulses.push(Pulse { hand, seconds: 0.12, frequency: 70.0, amplitude: 1.0 });
                if inp.rig.two_hands_on(object) {
                    self.pulses.push(Pulse { hand: hand ^ 1, seconds: 0.12, frequency: 70.0, amplitude: 0.8 });
                }
                events.push(ev);
                break;
            }
        }
    }

    /// Blows that landed on the player (the game's own hits, the count each creature sends): one the
    /// guard didn't meet is felt in both hands, and a heavy one (a power attack) shoves what they
    /// hold, as a blow on the guard would.
    fn landed(&mut self, inp: &CombatInput, blades: &HashMap<u32, (Vec3, Vec3)>) {
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        let chest = inp.map.point(Vec3::from_array(inp.frame.hmd.pos)) - Vec3::Y * CHEST_BELOW_EYES;
        for actor in &self.actors.actors[..n] {
            let Some(was) = self.blows_seen.insert(actor.id, actor.blows) else { continue };
            if actor.blows == was || self.block_until.get(&actor.id).is_some_and(|&t| t > inp.now) {
                continue;
            }
            let power = actor.flags & ACT_POWER_ATTACK != 0;
            for h in 0..2 {
                self.pulses.push(Pulse { hand: h, seconds: if power { 0.2 } else { 0.08 }, frequency: 60.0, amplitude: if power { 1.0 } else { 0.5 } });
            }
            if !power {
                continue;
            }
            let wb = Vec3::from_array(actor.weapon_b);
            let blow = blades.get(&actor.id).map(|&(_, vb)| vb).filter(|v| v.length() > 0.5).unwrap_or_else(|| (chest - wb).normalize_or(Vec3::Y) * 3.0);
            let m_blow = actor.weapon_mass.max(0.0) + ARM_MASS;
            for h in 0..2 {
                let Some(id) = inp.rig.held(h) else { continue };
                let Some(at) = inp.rig.swing(id).map(|s| s.com) else { continue };
                let m = inp.rig.mass_of(id).max(0.1);
                self.blows.push((id, blow * (m * m_blow / (m + m_blow)), at));
            }
            if debug_combat() {
                tracing::info!("combat: {:#x}'s power attack landed on the player at {:.1} m/s: what the hands hold is shoved", actor.id, blow.length());
            }
        }
        let actors = &self.actors.actors[..n];
        self.blows_seen.retain(|id, _| actors.iter().any(|a| a.id == *id));
    }

    fn magic(&mut self, inp: &CombatInput, bodies: &[Body], events: &mut Vec<CombatEvent>) {
        self.charging = [None, None];
        let spell = self.selected().copied();
        let hands = self.spell_hands(inp.dominant);
        let cast_time = if self.spells.cast_time > 0.05 { self.spells.cast_time } else { 0.5 };
        for h in 0..2 {
            let hs = inp.out.hands[h];
            let (hp, hq) = (Vec3::from_array(hs.pose.pos), Quat::from_array(hs.pose.rot));
            // The palm: along the hand from its joint (tracking), then the world.
            let palm_t = hp + hq * Vec3::Y * 0.06;
            let palm = inp.map.point(palm_t);
            let c = &mut self.casters[h];
            if let Some(last) = c.palm {
                c.vel = (palm - last) / inp.dt.max(1e-4);
            }
            c.palm = Some(palm);
            let free = inp.rig.held(h).is_none() && !inp.out.hand_busy[h] && !inp.out.hand_targeting[h];
            let (Some(spell), true, true, false) = (spell, hands[h], free, inp.paused) else {
                c.charging = false;
                c.charge = 0.0;
                c.ready = false;
                continue;
            };
            let trigger = inp.frame.hands[h].trigger;
            if trigger < TRIGGER_OFF {
                c.held_after_cast = false;
            }
            if !c.charging {
                if trigger > TRIGGER_ON && !c.held_after_cast && inp.now >= c.cooldown_until && spell.flags & SPELL_CASTABLE != 0 {
                    c.charging = true;
                    c.charge = 0.0;
                    c.ready = false;
                }
                continue;
            }
            c.charge = (c.charge + inp.dt / cast_time).min(1.0);
            if c.charge >= 1.0 && !c.ready {
                c.ready = true;
                self.pulses.push(Pulse { hand: h, seconds: 0.03, frequency: 250.0, amplitude: 1.0 });
            } else if !c.ready {
                self.pulses.push(Pulse { hand: h, seconds: inp.dt * 1.5, frequency: 160.0, amplitude: 0.05 + 0.25 * c.charge });
            }
            self.charging[h] = Some(Charging { spell: spell.id, charge: c.charge, color: spell.color, palm: palm_t });
            // A charged touch spell goes into the first body the hand touches.
            let touched = bodies
                .iter()
                .find(|b| {
                    let (a, bb) = (Vec3::from_array(b.part.a), Vec3::from_array(b.part.b));
                    let (_, t) = closest_segments(palm, palm, a, bb);
                    palm.distance(a.lerp(bb, t)) <= b.part.radius + TOUCH
                })
                .map(|b| b.actor.id);
            let released = trigger < TRIGGER_OFF;
            let touch_now = spell.delivery == DELIVER_TOUCH && c.ready && touched.is_some();
            if !(released || touch_now) {
                continue;
            }
            let c = &mut self.casters[h];
            c.charging = false;
            if !c.ready {
                continue; // let go too soon: nothing
            }
            let vel = c.vel;
            let (dir, actor) = match spell.delivery {
                DELIVER_SELF => (inp.map.rot(Quat::from_array(inp.frame.hmd.rot)) * Vec3::Z, 0),
                DELIVER_TOUCH => (vel.normalize_or(inp.map.rot(hq) * Vec3::Y), touched.unwrap_or(0)),
                _ => self.aim(inp, h, palm, vel),
            };
            self.casters[h].cooldown_until = inp.now + 0.25;
            self.casters[h].held_after_cast = !released;
            let ev = CombatEvent {
                kind: CE_CAST,
                actor,
                what: spell.id,
                hand: h as u32,
                strength: self.casters[h].charge,
                speed: self.casters[h].vel.length(),
                point: palm.to_array(),
                dir: dir.to_array(),
                ..Default::default()
            };
            self.debug.casts += 1;
            self.debug.last = Some(ev);
            tracing::info!("combat: cast {} from the {} hand at {palm:.2?} towards {dir:.2?} (on {actor})", spell.name(), if h == 0 { "left" } else { "right" });
            self.pulses.push(Pulse { hand: h, seconds: 0.08, frequency: 90.0, amplitude: 0.8 });
            events.push(ev);
        }
    }

    /// Where a target spell goes from the palm (world): the line from the shoulder raised to the
    /// eyes through the palm, bent towards a throw, and onto a creature near it.
    fn aim(&self, inp: &CombatInput, hand: usize, palm: Vec3, vel: Vec3) -> (Vec3, u32) {
        let head_rot = inp.map.rot(Quat::from_array(inp.frame.hmd.rot));
        let head = inp.map.point(Vec3::from_array(inp.frame.hmd.pos));
        let side = if hand == 0 { -1.0 } else { 1.0 };
        let right = (head_rot * Vec3::X * Vec3::new(1.0, 0.0, 1.0)).normalize_or(Vec3::X);
        let shoulder = head + right * side * SHOULDER_SIDE;
        let mut dir = (palm - shoulder).normalize_or(head_rot * Vec3::Z);
        if vel.length() > THROW_SPEED {
            dir = (dir * 0.7 + vel.normalize() * 0.3).normalize();
        }
        // Aim assist: the creature whose body (the middle of a torso part: a chest or a pelvis) is
        // nearest the line, within a few degrees.
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        let mut best: Option<(f32, Vec3, u32)> = None;
        for a in &self.actors.actors[..n] {
            if a.flags & ACT_ALIVE == 0 || a.count == 0 {
                continue;
            }
            let parts = &a.parts[..a.count as usize];
            let torso = parts.iter().filter(|p| p.kind == PART_TORSO);
            let aimed: Vec<_> = if torso.clone().next().is_some() { torso.collect() } else { vec![&parts[0]] };
            for part in aimed {
                let c = (Vec3::from_array(part.a) + Vec3::from_array(part.b)) * 0.5;
                let to = c - palm;
                let d = to.length();
                if d < 0.3 || d > AIM_ASSIST_RANGE {
                    continue;
                }
                let angle = to.normalize().dot(dir).clamp(-1.0, 1.0).acos();
                if angle <= AIM_ASSIST_DEG.to_radians() && best.is_none_or(|(b, ..)| angle < b) {
                    best = Some((angle, to.normalize(), a.id));
                }
            }
        }
        match best {
            Some((_, d, id)) => (d, id),
            None => (dir, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slash(vel: Vec3, normal: Vec3) -> Option<(f32, u32)> {
        // A blade along +Y cutting towards +X.
        judge(&Contact::new(DMG_SLASH, 0, Vec3::Y, Vec3::X, vel, normal, MAT_FLESH))
    }

    #[test]
    fn an_edge_cuts_along_its_edge_and_slaps_with_its_flat() {
        // Edge first into a body on its +X side: a clean cut.
        let (s, f) = slash(Vec3::X * 6.0, -Vec3::X).unwrap();
        assert!(s > 0.5 && f & CEF_GLANCING == 0, "{s} {f:#x}");
        // The flat (moving along +Z): a glancing blow.
        let (s, f) = slash(Vec3::Z * 6.0, -Vec3::Z).unwrap();
        assert!(s < 0.5 && f & CEF_GLANCING != 0, "{s} {f:#x}");
        // Too slow for flesh (1.5 m/s): nothing.
        assert!(slash(Vec3::X * 1.0, -Vec3::X).is_none());
        // Fast: a power hit.
        assert!(slash(Vec3::X * 9.0, -Vec3::X).unwrap().1 & CEF_POWER != 0);
    }

    #[test]
    fn plate_needs_a_harder_cut_and_a_point_must_go_in_point_first() {
        let plate = |v: f32| judge(&Contact::new(DMG_SLASH, 0, Vec3::Y, Vec3::X, Vec3::X * v, -Vec3::X, MAT_PLATE));
        assert!(plate(3.0).is_none() && plate(5.0).is_some());
        let stab = |vel: Vec3| judge(&Contact::new(DMG_PIERCE, 0, Vec3::Y, Vec3::X, vel, -Vec3::Y, MAT_FLESH));
        assert!(stab(Vec3::Y * 3.0).unwrap().1 & CEF_PENETRATED != 0);
        // Sideways with the point: a glancing blow.
        assert!(stab(Vec3::new(3.0, 0.5, 0.0)).is_none_or(|(_, f)| f & CEF_GLANCING != 0));
    }

    #[test]
    fn segments_meet_where_they_cross() {
        let (s, t) = closest_segments(Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, -1.0, 0.5), Vec3::new(0.0, 1.0, 0.5));
        assert!((s - 0.5).abs() < 1e-5 && (t - 0.5).abs() < 1e-5);
    }
}
