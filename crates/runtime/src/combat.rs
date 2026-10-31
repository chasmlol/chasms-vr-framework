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
//! A blade at a bad angle is a weak, glancing blunt hit. The speed sets the strength, and fast
//! enough is a power hit (the game's stagger). Each creature takes one hit per swing from each
//! weapon, and a blade is judged as it meets the creature: one that went in too slowly or at a bad
//! angle doesn't hit later from inside it, only once it has come out and meets it again.
//!
//! A hard chop of an axe's edge, or a stab of a point, stays in what it went into (a creature, or
//! the world's wood and earth): [`Combat::embeds`], held there by the rig (`rig::embed`). An empty fist punches. The player's weapon meeting a creature's attacking weapon is a
//! block, and a parry when it's moving into it.
//!
//! **Magic**: the off hand, empty, is the spell hand while the game has a spell ready. A finger
//! resting on its trigger (or pulling it a little) readies the spell; pulling it charges the spell
//! over the game's casting time; the charge is felt growing (faint and slow at first, then a sharp
//! ramp, as Skyrim VR's best spellcasting mod does it), with a spike when it's ready and a soft tick
//! while it's held ready. A held thing that casts ([`OBJF_CASTS`], a staff) is the same in whichever
//! hand holds it, cast from its tip along its length. Each hand's casting goes to the game
//! ([`Casters`]) for its own casting art; the runtime draws a glow of its own only for a game that
//! has none ([`SPELLS_GAME_ART`]). Then the spell is delivered:
//!
//! * a spell on the player is released anywhere;
//! * a touch spell goes into the first creature the charged hand touches, or wherever it's
//!   released;
//! * a target spell goes where the hand points when the trigger is released: the line from the
//!   shoulder (raised to the eyes) through the palm (a staff's: along the staff from its tip). A
//!   throw bends it towards the throw. The creature nearest that line within a few degrees is aimed
//!   at exactly.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::rig::embed;
use crate::rig::{Rig, RigOutput, WorldMap};

/// Each damager is followed at this many points along it.
const SAMPLES: usize = 12;
/// A creature takes one hit per swing from a weapon: none for this long after one.
const HIT_COOLDOWN: f64 = 0.35;
/// One block per attack.
const BLOCK_COOLDOWN: f64 = 0.8;
/// A fist: a sphere this far along the hand from its joint, this big; closed past this squeeze.
/// What a weapon is made of, for its sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stuff {
    Metal,
    Wood,
    Flesh,
}

/// How long a hit's buzz runs (s).
const HIT_BUZZ: f32 = 0.3;

/// A contact is kept this many frames after it's lost: one that comes and goes is one hit.
const CONTACT_FRAMES: u64 = 5;

/// Which of a weapon's collider groups a damager is on: its edges and point share the blade's, each
/// blunt damager (a pommel, a haft, a head) is its own.
fn group_of(b: &MeleeBlade, blade: usize) -> usize {
    if matches!(b.kind, DMG_SLASH | DMG_PIERCE) { 0 } else { blade + 1 }
}

/// How a stuck blade rocks (metres, radians): its own, or its kind's (an axe's 0.12 m, any other's
/// a sword's 0.1; 30 degrees).
fn rock_of(b: &MeleeBlade) -> (f32, f32) {
    let (depth, angle) = if b.rock != [0.0; 2] {
        (b.rock[0], b.rock[1])
    } else if b.kind == DMG_SLASH && b.flags & DMGF_AXE != 0 {
        (0.12, embed::ROCK.1)
    } else {
        embed::ROCK
    };
    (depth, angle.to_radians())
}

/// A hand's Punch damager (the player's hands: their palm and fingers, in the drawn hand's grip
/// space, the fingers out along +Z): judged by the Punch table and curve (2 m/s: 1, 20 m/s: 5),
/// within 60 degrees of square, never going in, twice its damage thrown.
fn punch() -> MeleeBlade {
    let mut b: MeleeBlade = bytemuck::Zeroable::zeroed();
    b.kind = DMG_PUNCH;
    b.modifier = rustvr_melee::modifiers::MOD_PUNCH;
    b.radius = PUNCH_RADIUS;
    b.a = [0.0, 0.0, -0.02];
    b.b = [0.0, 0.0, 0.1];
    b.tier = 1.0;
    b.thrown = 2.0;
    b.normal = 60.0;
    b.curve = [2.0, 1.0, 20.0, 5.0];
    b
}

/// A foot's Kick damager (the Kick table, 2 -> 1, 20 -> 5, x2 thrown, any way; a body that's down
/// shoved with 400 for 0.15 s, x2 and x0.5; its effects 2 to 4 m/s).
fn kick() -> MeleeBlade {
    let mut b: MeleeBlade = bytemuck::Zeroable::zeroed();
    b.kind = DMG_BLUNT;
    b.modifier = rustvr_melee::modifiers::MOD_KICK;
    b.radius = KICK_RADIUS;
    b.tier = 1.0;
    b.thrown = 2.0;
    b.curve = [2.0, 1.0, 20.0, 5.0];
    b.push = [400.0, 0.15, 2.0, 0.5];
    b.intensity = [2.0, 4.0];
    b
}

/// How thick a foot is to its Kick damager (m).
const KICK_RADIUS: f32 = 0.05;

/// How thick a hand is to its Punch damager (m).
const PUNCH_RADIUS: f32 = 0.045;
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
/// Above this the parry is a parry: the player's weapon moving into the attack (m/s).
const PARRY_SPEED: f32 = 2.0;
/// A creature's swing met by the player's weapon moving at least this fast is parried (m/s).
const PARRY_MIN_SPEED: f32 = 1.0;
/// A creature's damager strikes the same part of the player at most this often (s; it remembers
/// only the part it last hit).
const STRUCK_COOLDOWN: f64 = 0.1;
/// A creature's blow the player's weapon or shield met doesn't go on into the player for this
/// long (s): the guard stopped it (the creature's weapon follows the game's pose, which may carry on
/// a moment).
const STRUCK_AFTER_BLOCK: f64 = 0.4;
/// The chest, this far under the eyes (metres): where a blow's way ends.
const CHEST_BELOW_EYES: f32 = 0.35;
/// A guard closer to the chest than this is the body itself being hit, not a block (metres).
const BODY_SPARE: f32 = 0.15;
/// The trigger charges past this and releases below that; a finger on it (or a pull past
/// `TRIGGER_READY`) readies the spell.
const TRIGGER_ON: f32 = 0.6;
const TRIGGER_OFF: f32 = 0.3;
const TRIGGER_READY: f32 = 0.05;
/// A spell's art sits this far off the palm (m).
const PALM_OUT: f32 = 0.06;
/// The charge's haptics (after ISPVR): a pulse every `CHARGE_GAP_SLOW` seconds at the start down to
/// every `CHARGE_GAP_FAST` when it's full, as strong as `CHARGE_FAINT + CHARGE_STRONG * charge^4`;
/// held ready, a soft tick every `READY_TICK` seconds.
const CHARGE_GAP_SLOW: f64 = 0.1;
const CHARGE_GAP_FAST: f64 = 0.02;
const CHARGE_FAINT: f32 = 0.06;
const CHARGE_STRONG: f32 = 0.6;
const READY_TICK: f64 = 0.3;
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

/// The player's body as capsules a creature's blow can land on (world: ends, radius, `PART_*`), from
/// the solved body's joints (tracking space): the head, the torso, the arms and the legs.
pub fn player_capsules(joints: &[Pose; JOINT_COUNT], map: &WorldMap) -> Vec<(Vec3, Vec3, f32, u32)> {
    let at = |j: Joint| map.point(Vec3::from_array(joints[j as usize].pos));
    let w = map.world_per_meter;
    let up = (at(Joint::Head) - at(Joint::Neck)).normalize_or(Vec3::Y);
    let mut out = vec![(at(Joint::Neck), at(Joint::Head) + up * 0.08 * w, 0.1 * w, PART_HEAD), (at(Joint::Hips), at(Joint::UpperChest), 0.15 * w, PART_TORSO)];
    for (a, b, r, kind) in [
        (Joint::LeftUpperArm, Joint::LeftLowerArm, 0.05, PART_ARM),
        (Joint::LeftLowerArm, Joint::LeftHand, 0.045, PART_ARM),
        (Joint::RightUpperArm, Joint::RightLowerArm, 0.05, PART_ARM),
        (Joint::RightLowerArm, Joint::RightHand, 0.045, PART_ARM),
        (Joint::LeftUpperLeg, Joint::LeftLowerLeg, 0.07, PART_LEG),
        (Joint::LeftLowerLeg, Joint::LeftFoot, 0.055, PART_LEG),
        (Joint::RightUpperLeg, Joint::RightLowerLeg, 0.07, PART_LEG),
        (Joint::RightLowerLeg, Joint::RightFoot, 0.055, PART_LEG),
    ] {
        out.push((at(a), at(b), r * w, kind));
    }
    out
}

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
        DMG_SLASH => (7.0, 8.0),
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
    let (full, power) = full_and_power(c.kind);
    let strength = (((speed - min) / (full - min).max(0.5)).clamp(0.0, 1.5) * strength_k).max(0.05);
    if clean && speed >= power {
        flags |= CEF_POWER;
    }
    if c.kind == DMG_PIERCE && clean && speed >= 2.0 {
        flags |= CEF_PENETRATED;
    }
    Some((strength, flags))
}

/// Where a point swept from `p0` to `p1` first comes within `reach` of the segment `a`..`b`: (its
/// share of the sweep, the point, the nearest point on the segment, whether it touches at all).
fn entry(p0: Vec3, p1: Vec3, a: Vec3, b: Vec3, reach: f32) -> (f32, Vec3, Vec3, bool) {
    let (s, t) = closest_segments(p0, p1, a, b);
    let (deep, axis) = (p0.lerp(p1, s), a.lerp(b, t));
    if deep.distance(axis) > reach {
        return (s, deep, axis, false);
    }
    // Back from the deepest point to where it came in: halving the gap between a share that
    // touches and one before it that doesn't.
    let near = |u: f32| {
        let p = p0.lerp(p1, u);
        let (_, t) = closest_segments(p, p, a, b);
        (p, a.lerp(b, t))
    };
    let (mut lo, mut hi) = (0.0f32, s);
    if near(0.0).0.distance(near(0.0).1) <= reach {
        let (p, q) = near(0.0);
        return (0.0, p, q, true);
    }
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        let (p, q) = near(mid);
        if p.distance(q) <= reach { hi = mid } else { lo = mid }
    }
    let (p, q) = near(hi);
    (hi, p, q, true)
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
    /// What it was doing (`CAST_*`), since when; the next haptic pulse of the charge.
    state: u32,
    since: f64,
    next_pulse: f64,
}

/// What a hand casts with: the spell selected, from the palm; or a held thing's own magic, from its
/// tip.
#[derive(Clone, Copy, Debug)]
enum Source {
    Spell(Spell),
    Thing(ObjectDesc),
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

pub struct Combat {
    actors: Box<Actors>,
    melee: Box<MeleeTable>,
    spells: Box<Spells>,
    /// The game's melee weapons with their full melee numbers ([`MeleeWeapons`]), what its
    /// creatures hold ([`ActorItems`]) and its rules for the player ([`MeleeRules`]).
    pub weapons: Box<MeleeWeapons>,
    pub items: Box<ActorItems>,
    pub rules: Box<MeleeRules>,
    /// The weapons a hand's spell has filled, by id (`crate::rig::melee`'s imbuing), and which
    /// spells are fire (the damage tables' fire rows).
    pub imbues: HashMap<u32, Imbue>,
    /// Each drawn hand as it sits on its controller's grip (tracking space), from when it was last
    /// free: a punch is swept where the controller takes it.
    hand_on_grip: [Option<(Vec3, Quat)>; 2],
    pub fire_spells: Vec<u32>,
    /// The hits heard this frame: (cue, where (world), how hard 0..1, the weapon).
    pub sounds: Vec<(crate::rig::feedback::Cue, Vec3, f32, u32)>,
    /// The creatures' swinging weapons' blades last frame (world), and when each may strike the
    /// player again.
    item_prev: HashMap<u32, Vec<(Vec3, Vec3)>>,
    /// Each creature's damager's cooldown on each of the player's parts (item, damager, part).
    struck_until: HashMap<(u32, usize, usize), f64>,
    /// The player's parts last frame (their motion takes off a blow's speed).
    player_prev: Vec<(Vec3, Vec3)>,
    /// Each held weapon's damager points last frame (world), by object (0, 1: the fists).
    prev: HashMap<u32, Vec<[Vec3; SAMPLES]>>,
    /// Each body part's centre last frame, by (actor, part index).
    prev_parts: HashMap<(u32, usize), Vec3>,
    /// The damagers in a creature last frame (actor, object, damager): judged as they went in, not
    /// again until they've come out.
    touching: std::collections::HashSet<(u32, u32, usize)>,
    touching_now: std::collections::HashSet<(u32, u32, usize)>,
    hit_until: HashMap<(u32, u32), f64>,
    /// The contacts: one for each of a weapon's collider groups (its edges and point; each blunt
    /// part) against each body part, kept [`CONTACT_FRAMES`] frames after they're last touching
    /// (by the frame they last touched: (creature, part, weapon, hand, group)); a contact is one
    /// hit, however long it lasts.
    contacts: HashMap<(u32, usize, u32, usize, usize), u64>,
    /// Each damager's cooldown (weapon, damager, hand): the part it last hit, and until when it
    /// can't hit that part again (one slot: only the part it last hit).
    cooldowns: HashMap<(u32, usize, usize), ((u32, usize), f64)>,
    frame: u64,
    block_until: HashMap<u32, f64>,
    casters: [Caster; 2],
    pub pulses: Vec<Pulse>,
    pub charging: [Option<Charging>; 2],
    /// Each hand's casting this frame, for the game to draw its art ([`Casters`]).
    pub casting: [rustvr_protocol::Caster; 2],
    pub debug: CombatDebug,
    /// Blades that went in and stay in, this frame (for the rig).
    pub embeds: Vec<embed::Request>,
    /// Weapons that cut a limb off this frame, and whose: they carry on through it (for the rig).
    pub cuts: Vec<(u32, u32)>,
    /// A kicking foot met something this frame (it stops there).
    pub kicked: bool,
    /// When each damager (weapon, blade) last cut something off.
    sliced: HashMap<(u32, usize), f64>,
}

impl Default for Combat {
    fn default() -> Self {
        Combat {
            actors: bytemuck::zeroed_box(),
            melee: bytemuck::zeroed_box(),
            spells: bytemuck::zeroed_box(),
            weapons: bytemuck::zeroed_box(),
            items: bytemuck::zeroed_box(),
            rules: bytemuck::zeroed_box(),
            imbues: HashMap::new(),
            hand_on_grip: [None; 2],
            fire_spells: Vec::new(),
            item_prev: HashMap::new(),
            struck_until: HashMap::new(),
            player_prev: Vec::new(),
            sounds: Vec::new(),
            prev: HashMap::new(),
            prev_parts: HashMap::new(),
            touching: Default::default(),
            touching_now: Default::default(),
            hit_until: HashMap::new(),
            contacts: HashMap::new(),
            cooldowns: HashMap::new(),
            frame: 0,
            block_until: HashMap::new(),
            casters: Default::default(),
            pulses: Vec::new(),
            charging: [None, None],
            casting: Default::default(),
            debug: CombatDebug::default(),
            embeds: Vec::new(),
            cuts: Vec::new(),
            kicked: false,
            sliced: HashMap::new(),
        }
    }
}

pub struct CombatInput<'a> {
    pub rig: &'a Rig,
    pub out: &'a RigOutput,
    pub map: &'a WorldMap,
    pub frame: &'a HmdFrame,
    /// The objects the game offers (what a held thing casts).
    pub objects: &'a ObjectTable,
    pub dominant: usize,
    pub now: f64,
    pub dt: f32,
    /// Menus, pauses, loading: nothing is judged.
    pub paused: bool,
    /// The player's body as capsules (world: ends, radius, `PART_*`), for the creatures' blows.
    pub player: &'a [(Vec3, Vec3, f32, u32)],
    /// A foot kicking now ([`crate::kick`]): which, its ankle and toes (world), and the kick's way.
    pub kick: Option<(usize, Vec3, Vec3, Vec3)>,
}

/// A damager's hit on a body part: whose, which part (its index, kind and radius), where, how
/// fast, out of which surface, and how it was judged.
struct JudgedHit {
    actor: u32,
    index: usize,
    part_kind: u32,
    /// The part's capsule (world).
    a: Vec3,
    b: Vec3,
    radius: f32,
    point: Vec3,
    #[allow(dead_code)]
    vel: Vec3,
    #[allow(dead_code)]
    normal: Vec3,
    judged: rustvr_melee::Judged,
}

/// A world-space capsule of a creature: its id, the part's index, the part.
struct Body<'a> {
    actor: &'a Actor,
    index: usize,
    part: &'a ActorPart,
    vel: Vec3,
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

    /// The ids of the game's spells with any of `flags` (`SPELL_*`).
    pub fn spells_with(&self, flags: u32) -> Vec<u32> {
        self.spells.spells[..(self.spells.count as usize).min(MAX_SPELLS)].iter().filter(|s| s.flags & flags != 0).map(|s| s.id).collect()
    }

    /// Spell `id`'s flags (`SPELL_*`; 0: no such spell).
    pub fn spell_flags(&self, id: u32) -> u32 {
        self.spells.spells[..(self.spells.count as usize).min(MAX_SPELLS)].iter().find(|s| s.id == id).map_or(0, |s| s.flags)
    }

    /// The game's own index for creature `actor`'s part `index` (as [`ActorPart::game_part`]).
    pub fn game_part(&self, actor: u32, index: usize) -> u32 {
        self.actors().iter().find(|a| a.id == actor).and_then(|a| a.parts.get(index)).map_or(index as u32, |p| p.game_part)
    }

    /// The creatures as the game last said (for the gore: whose body a hit drew blood from).
    pub fn actors(&self) -> &[Actor] {
        &self.actors.actors[..(self.actors.count as usize).min(MAX_ACTORS)]
    }

    /// A held weapon's cutting and stabbing damagers where they were judged this frame (world):
    /// each one's ends, and its kind.
    pub fn blades(&self, object: u32) -> Vec<(Vec3, Vec3, u32)> {
        let (Some(desc), Some(points)) = (self.melee_desc(object), self.prev.get(&object)) else { return Vec::new() };
        desc.damagers[..(desc.count as usize).min(MAX_DAMAGERS)]
            .iter()
            .zip(points)
            .filter(|(d, _)| d.flags & DMGF_GUARD == 0 && matches!(d.kind, DMG_SLASH | DMG_PIERCE))
            .map(|(d, p)| (p[0], p[SAMPLES - 1], d.kind))
            .collect()
    }

    /// The tables the game wrote (reads the link once a frame; what's unchanged stays).
    #[cfg(windows)]
    pub fn read(&mut self, link: &rustvr_protocol::link::Link) {
        link.read_actors_into(&mut self.actors);
        link.read_melee_into(&mut self.melee);
        link.read_spells_into(&mut self.spells);
        link.read_melee_weapons_into(&mut self.weapons);
        link.read_actor_items_into(&mut self.items);
        link.read_melee_rules_into(&mut self.rules);
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

    /// One frame: hits, blocks and casts (each returned for the game, and counted).
    pub fn update(&mut self, inp: &CombatInput) -> Vec<CombatEvent> {
        self.pulses.clear();
        self.embeds.clear();
        let mut events = Vec::new();
        let dt = inp.dt.max(1e-4);
        // The bodies, with how fast each part moves.
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        let actors = self.actors.actors;
        let mut bodies = Vec::new();
        let mut parts_now = HashMap::new();
        for a in &actors[..n] {
            for (i, p) in a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)].iter().enumerate() {
                let c = (Vec3::from_array(p.a) + Vec3::from_array(p.b)) * 0.5;
                let vel = self.prev_parts.get(&(a.id, i)).map(|&q| (c - q) / dt).filter(|v| v.length() < 30.0).unwrap_or(Vec3::from_array(a.velocity));
                parts_now.insert((a.id, i), c);
                bodies.push(Body { actor: a, index: i, part: p, vel });
            }
        }
        self.prev_parts = parts_now;
        let mut seen = HashMap::new();
        for h in 0..2 {
            let held = inp.rig.held(h);
            // A weapon with its full melee numbers is judged by them.
            if let Some(id) = held
                && self.judge_weapon(inp, &bodies, h, id, dt, &mut events, &mut seen)
            {
                continue;
            }
            let weapon = held.and_then(|id| Some((id, *self.melee_desc(id)?, inp.rig.swing_pose(id)?)));
            if let Some((id, desc, (p, q))) = weapon {
                // The weapon's damagers in the world now (the rig's physics is in the world).
                let (wp, wq) = (p, q);
                let now: Vec<[Vec3; SAMPLES]> = desc.damagers[..(desc.count as usize).min(MAX_DAMAGERS)]
                    .iter()
                    .map(|d| {
                        let (a, b) = (Vec3::from_array(d.a), Vec3::from_array(d.b));
                        std::array::from_fn(|k| wp + wq * a.lerp(b, k as f32 / (SAMPLES - 1) as f32))
                    })
                    .collect();
                let prev = self.prev.get(&id).filter(|v| v.len() == now.len()).cloned();
                if let (Some(prev), false) = (prev, inp.paused) {
                    for (di, d) in desc.damagers[..now.len()].iter().enumerate() {
                        if d.flags & DMGF_GUARD != 0 {
                            continue;
                        }
                        let frame = (wq * Vec3::from_array(d.b) - wq * Vec3::from_array(d.a)).normalize_or_zero();
                        let edge = (wq * Vec3::from_array(d.edge)).normalize_or_zero();
                        let hit = self.sweep(inp, &bodies, h, id, di, d.kind, d.flags, d.radius, frame, edge, &prev[di], &now[di], dt, &mut events);
                        // An axe's edge chopping in, or a point stabbing in, hard enough: it stays.
                        let stab = d.kind == DMG_PIERCE;
                        let chop = d.kind == DMG_SLASH && d.flags & DMGF_AXE != 0;
                        if !(stab || chop) || self.embeds.iter().any(|e| e.object == id) {
                            continue;
                        }
                        let (way, speed_needed, depth, max_depth) = if stab {
                            (frame, embed::STAB_SPEED, (d.depth * 0.5).max(0.03), if d.depth > 0.0 { d.depth } else { embed::STAB_MAX_DEPTH })
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
                                self.embeds.push(embed::Request { object: id, into: embed::Into::Actor { id: actor, part }, point, dir: way, sample: local(point), depth, max_depth, dampers: embed::DAMPERS, slide: None, wounding: Default::default(), rock: (embed::ROCK.0, embed::ROCK.1.to_radians()) });
                            }
                            continue;
                        }
                        if let Some((point, ..)) = self.bite_world(inp, d.kind, d.flags, frame, edge, &prev[di], &now[di], dt, speed_needed) {
                            self.embeds.push(embed::Request { object: id, into: embed::Into::World, point, dir: way, sample: local(point), depth, max_depth, dampers: embed::DAMPERS, slide: None, wounding: Default::default(), rock: (embed::ROCK.0, embed::ROCK.1.to_radians()) });
                            continue;
                        }
                        if let Some((thing, point, _)) = Self::bite_thing(inp, id, d.kind, d.flags, frame, edge, &prev[di], &now[di], dt, speed_needed) {
                            self.embeds.push(embed::Request { object: id, into: embed::Into::Object { id: thing }, point, dir: way, sample: local(point), depth, max_depth, dampers: embed::DAMPERS, slide: None, wounding: Default::default(), rock: (embed::ROCK.0, embed::ROCK.1.to_radians()) });
                        }
                    }
                    self.blocks(inp, h, id, &desc, &prev, &now, dt, &mut events);
                }
                seen.insert(id, now);
            } else if held.is_none() && !inp.out.hand_busy[h] {
                // An empty hand hits open or closed alike (a fist doesn't hit harder): its palm and
                // fingers carry Punch.
                let hs = &inp.out.hands[h];
                let (hp, hq) = (Vec3::from_array(hs.pose.pos), Quat::from_array(hs.pose.rot));
                // Swept where the controller takes it (the hand meets things at the speed it was
                // going, not stopped on them as the drawn hand is): the hand as it sits on its
                // controller, from when it was last free.
                let grip = &inp.frame.hands[h].grip;
                let (gp, gq) = (Vec3::from_array(grip.pos), Quat::from_array(grip.rot));
                if inp.out.ghosts[h].is_none() {
                    self.hand_on_grip[h] = Some((gq.inverse() * (hp - gp), gq.inverse() * hq));
                }
                let (hp, hq) = self.hand_on_grip[h].map_or((hp, hq), |(p, q)| (gp + gq * p, gq * q));
                let blade = punch();
                let now: [Vec3; SAMPLES] = std::array::from_fn(|k| inp.map.point(hp + hq * Vec3::from_array(blade.a).lerp(Vec3::from_array(blade.b), k as f32 / (SAMPLES - 1) as f32)));
                let key = h as u32;
                if hs.flags & HAND_TRACKED != 0
                    && !inp.paused
                    && let Some(prev) = self.prev.get(&key).filter(|p| p.len() == 1).cloned()
                {
                    let along = inp.map.rot(hq) * Vec3::Z;
                    self.judge_sweep(inp, &bodies, h, 0, h, &blade, 1, along, along, &prev[0], &now, dt, false, &[], &mut events);
                }
                seen.insert(key, vec![now]);
            }
        }
        // Thrown weapons are judged as they fly (with the thrown multipliers and pushes), as the
        // hand that threw them.
        for (id, hand) in inp.rig.thrown_melee() {
            self.judge_weapon(inp, &bodies, hand, id, dt, &mut events, &mut seen);
        }
        // A kicking foot (as "hand" 2 or 3, no object), from its ankle to its toes.
        self.kicked = false;
        if let Some((foot, ankle, toes, way)) = inp.kick {
            let blade = kick();
            let now: [Vec3; SAMPLES] = std::array::from_fn(|k| ankle.lerp(toes, k as f32 / (SAMPLES - 1) as f32));
            let key = 2 + foot as u32;
            if !inp.paused
                && let Some(prev) = self.prev.get(&key).filter(|p| p.len() == 1).cloned()
                && self.judge_sweep(inp, &bodies, 2 + foot, 0, 2 + foot, &blade, 1, way, way, &prev[0], &now, dt, false, &[], &mut events).is_some()
            {
                self.kicked = true;
            }
            seen.insert(key, vec![now]);
        }
        self.prev = seen;
        self.touching = std::mem::take(&mut self.touching_now);
        self.frame += 1;
        let frame = self.frame;
        self.contacts.retain(|_, &mut last| frame - last <= CONTACT_FRAMES);
        let now = inp.now;
        self.cooldowns.retain(|_, &mut (_, until)| until > now);
        if !inp.paused {
            self.struck(inp, dt, &mut events);
        }
        self.struck_until.retain(|_, &mut t| t > inp.now);
        self.magic(inp, &bodies, &mut events);
        self.hit_until.retain(|_, &mut t| t > inp.now);
        self.block_until.retain(|_, &mut t| t > inp.now);
        events
    }

    /// One damager's sweep (its sample points from `prev` to `now`) against every body.
    #[allow(clippy::too_many_arguments)]
    /// A clean hit: (actor, part index, where, the impact velocity, the part's material).
    fn sweep(&mut self, inp: &CombatInput, bodies: &[Body], hand: usize, object: u32, damager: usize, kind: u32, flags: u32, radius: f32, along: Vec3, edge: Vec3, prev: &[Vec3; SAMPLES], now: &[Vec3; SAMPLES], dt: f32, events: &mut Vec<CombatEvent>) -> Option<(u32, usize, Vec3, Vec3, u32)> {
        // The earliest contact of any sample with any body (a creature it's still in from last
        // frame is no new contact: it stays in, unjudged).
        let mut best: Option<(f32, usize, Vec3, Vec3, Vec3)> = None;
        for body in bodies {
            let key = (body.actor.id, object);
            let (a, b) = (Vec3::from_array(body.part.a), Vec3::from_array(body.part.b));
            let reach = body.part.radius + radius;
            let contact = |k: usize| {
                let (s, t) = closest_segments(prev[k], now[k], a, b);
                let p = prev[k].lerp(now[k], s);
                let on_axis = a.lerp(b, t);
                (s, p, on_axis, p.distance(on_axis) <= reach)
            };
            let entry = (body.actor.id, object, damager);
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
                    let vel = (now[k] - prev[k]) / dt - body.vel;
                    let normal = (p - on_axis).normalize_or(-vel.normalize_or_zero());
                    best = Some((s, bodies.iter().position(|x| std::ptr::eq(x, body)).unwrap(), p, vel, normal));
                }
            }
        }
        let (_, bi, point, vel, normal) = best?;
        let body = &bodies[bi];
        self.touching_now.insert((body.actor.id, object, damager));
        let contact = Contact { kind, flags, along, edge, vel, normal, material: body.part.material };
        let judged = judge(&contact);
        if debug_combat() {
            let dir = vel.normalize_or_zero();
            let across = (dir - along * dir.dot(along)).normalize_or_zero();
            let deg = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
            tracing::info!(
                "combat: contact kind {kind} with {:#x} part {} ({}): {:.1} m/s, across the blade vs its edge {:.0} deg, into the surface vs the edge {:.0} deg, along the blade vs the way it moves {:.0} deg -> {judged:?}",
                body.actor.id,
                body.index,
                body.part.kind,
                vel.length(),
                deg(across, edge),
                deg(-normal, edge),
                deg(dir, along)
            );
        }
        let (strength, cef) = judged?;
        self.hit_until.insert((body.actor.id, object), inp.now + HIT_COOLDOWN);
        let ev = CombatEvent {
            kind: CE_HIT,
            flags: cef,
            actor: body.actor.id,
            part: body.part.game_part,
            object,
            what: if cef & CEF_GLANCING != 0 { DMG_BLUNT } else { kind },
            hand: hand as u32,
            material: body.part.material,
            strength,
            speed: vel.length(),
            point: point.to_array(),
            dir: vel.normalize_or_zero().to_array(),
            ..Default::default()
        };
        self.debug.hits += 1;
        self.debug.last = Some(ev);
        self.pulses.push(Pulse { hand, seconds: 0.05 + 0.07 * strength.min(1.0), frequency: 110.0, amplitude: (0.35 + 0.55 * strength).min(1.0) });
        events.push(ev);
        (cef & CEF_GLANCING == 0).then_some((body.actor.id, body.index, point, vel, body.part.material))
    }

    /// A damager's sweep against the world's static collision: where it chopped (an axe's edge,
    /// leading, into the surface: the axe's angle limits) or stabbed (along the blade) into
    /// something it bites, at least `speed` fast: (where, its material, how fast it went in).
    #[allow(clippy::too_many_arguments)]
    fn bite_world(&self, inp: &CombatInput, kind: u32, flags: u32, along: Vec3, edge: Vec3, prev: &[Vec3; SAMPLES], now: &[Vec3; SAMPLES], dt: f32, speed: f32) -> Option<(Vec3, u32, f32)> {
        let within = |a: Vec3, b: Vec3, deg: f32| a.dot(b) >= deg.to_radians().cos();
        for k in 0..SAMPLES {
            let way = now[k] - prev[k];
            let len = way.length();
            if len < 1e-4 || len / dt < speed {
                continue;
            }
            let Some(hit) = inp.rig.world_hit(prev[k], way, len) else { continue };
            let dir = way / len;
            // (What the game didn't say the material of, a blade bites as wood.)
            let material = hit.material.unwrap_or(MAT_WOOD);
            if !biteable(material) {
                return None;
            }
            let clean = if kind == DMG_PIERCE {
                within(dir, along, PIERCE_DEG) && within(-hit.normal, along, NORMAL_DEG)
            } else {
                let across = (dir - along * dir.dot(along)).normalize_or_zero();
                let (across_deg, normal_deg) = if flags & DMGF_AXE != 0 { (AXE_ACROSS_DEG, AXE_NORMAL_DEG) } else { (SLASH_ACROSS_DEG, NORMAL_DEG) };
                within(across, edge, across_deg) && within(-hit.normal, edge, normal_deg)
            };
            return clean.then_some((prev[k] + dir * hit.distance, material, len / dt));
        }
        None
    }

    /// A damager's sweep against the rig's loose things (a crate, a barrel): where it chopped or
    /// stabbed into one (as [`Combat::bite_world`] judges it), at least `speed` fast: (the thing,
    /// where, how fast it went in).
    #[allow(clippy::too_many_arguments)]
    fn bite_thing(inp: &CombatInput, weapon: u32, kind: u32, flags: u32, along: Vec3, edge: Vec3, prev: &[Vec3; SAMPLES], now: &[Vec3; SAMPLES], dt: f32, speed: f32) -> Option<(u32, Vec3, f32)> {
        let within = |a: Vec3, b: Vec3, deg: f32| a.dot(b) >= deg.to_radians().cos();
        for k in 0..SAMPLES {
            let way = now[k] - prev[k];
            let len = way.length();
            if len < 1e-4 || len / dt < speed {
                continue;
            }
            let Some((thing, dist, normal)) = inp.rig.thing_hit(prev[k], way, len, weapon) else { continue };
            let dir = way / len;
            let clean = if kind == DMG_PIERCE {
                within(dir, along, PIERCE_DEG) && within(-normal, along, NORMAL_DEG)
            } else {
                let across = (dir - along * dir.dot(along)).normalize_or_zero();
                let (across_deg, normal_deg) = if flags & DMGF_AXE != 0 { (AXE_ACROSS_DEG, AXE_NORMAL_DEG) } else { (SLASH_ACROSS_DEG, NORMAL_DEG) };
                within(across, edge, across_deg) && within(-normal, edge, normal_deg)
            };
            return clean.then_some((thing, prev[k] + dir * dist, len / dt));
        }
        None
    }

    /// The weapon meeting a creature's attacking weapon, or put across the blow's way to the
    /// player: from its weapon to the player's chest (a game whose blows land by reach, where the
    /// weapon never touches the player, is blocked by a guard between them).
    #[allow(clippy::too_many_arguments)]
    fn blocks(&mut self, inp: &CombatInput, hand: usize, object: u32, desc: &MeleeDesc, prev: &[[Vec3; SAMPLES]], now: &[[Vec3; SAMPLES]], dt: f32, events: &mut Vec<CombatEvent>) {
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        let chest = inp.map.point(Vec3::from_array(inp.frame.hmd.pos)) - Vec3::Y * CHEST_BELOW_EYES;
        for actor in &self.actors.actors[..n] {
            if actor.flags & ACT_ATTACKING == 0 || self.block_until.get(&actor.id).is_some_and(|&t| t > inp.now) {
                continue;
            }
            let (wa, wb) = (Vec3::from_array(actor.weapon_a), Vec3::from_array(actor.weapon_b));
            // The blow's way: the weapon, then on to the player (short of the body itself).
            let to_chest = chest - wb;
            let reach_end = wb + to_chest * (1.0 - (BODY_SPARE / to_chest.length().max(1e-3)).min(1.0));
            let ways = [(wa, wb), (wb, reach_end)];
            for (di, d) in desc.damagers[..now.len()].iter().enumerate() {
                let (a, b) = (now[di][0], now[di][SAMPLES - 1]);
                let Some((p, q)) = ways.iter().map(|&(wa, wb)| {
                    let (s, t) = closest_segments(a, b, wa, wb);
                    (a.lerp(b, s), wa.lerp(wb, t))
                }).find(|(p, q)| p.distance(*q) <= actor.weapon_radius + d.radius) else {
                    continue;
                };
                let s = (p - a).dot(b - a) / (b - a).length_squared().max(1e-8);
                let k = ((s.clamp(0.0, 1.0) * (SAMPLES - 1) as f32).round() as usize).min(SAMPLES - 1);
                let vel = (now[di][k] - prev[di][k]) / dt;
                let parry = vel.dot((q - p).normalize_or_zero()) > PARRY_SPEED || vel.length() > PARRY_SPEED * 1.5;
                self.block_until.insert(actor.id, inp.now + BLOCK_COOLDOWN);
                let ev = CombatEvent {
                    kind: CE_BLOCK,
                    flags: if parry { CEF_PARRY } else { 0 },
                    actor: actor.id,
                    object,
                    hand: hand as u32,
                    strength: (vel.length() / 6.0).min(1.5),
                    speed: vel.length(),
                    point: p.to_array(),
                    dir: vel.normalize_or_zero().to_array(),
                    ..Default::default()
                };
                self.debug.blocks += 1;
                self.debug.last = Some(ev);
                self.pulses.push(Pulse { hand, seconds: 0.12, frequency: 70.0, amplitude: 1.0 });
                events.push(ev);
                break;
            }
        }
    }

    // ---- the full melee (a weapon the game describes in [`MeleeWeapons`]) -----------------------------
    /// A held weapon the game describes with its full melee numbers: each of its damagers swept from
    /// last frame to where the hand swings it this frame against every body part, each contact
    /// judged (`rustvr_melee::judge`: the impact velocity against the damager's angles and the
    /// material's minimum, then its curve, tier and the material's multiplier); a blow that goes in
    /// stays in ([`embed`]). False: it isn't one (the older table's path judges it).
    #[allow(clippy::too_many_arguments)]
    fn judge_weapon(&mut self, inp: &CombatInput, bodies: &[Body], hand: usize, id: u32, dt: f32, events: &mut Vec<CombatEvent>, seen: &mut HashMap<u32, Vec<[Vec3; SAMPLES]>>) -> bool {
        // A thrown one sweeps from where it was before the physics stepped to where it would have
        // gone (`rig::melee`'s `Thrown`); a held one from where it was last frame.
        let sweep = inp.rig.thrown_sweep(id);
        let (Some(w), Some((wp, wq))) = (self.weapons.get(id).copied(), sweep.map(|s| s.1).or_else(|| inp.rig.swing_pose(id))) else { return false };
        let blades = w.blades();
        let samples = |(p, q): (Vec3, Quat)| -> Vec<[Vec3; SAMPLES]> {
            blades
                .iter()
                .map(|b| {
                    let (a, bb) = (Vec3::from_array(b.a), Vec3::from_array(b.b));
                    std::array::from_fn(|k| p + q * a.lerp(bb, k as f32 / (SAMPLES - 1) as f32))
                })
                .collect()
        };
        let now = samples((wp, wq));
        let prev = match sweep {
            Some((from, _)) => Some(samples(from)),
            None => self.prev.get(&id).filter(|v| v.len() == now.len()).cloned(),
        };
        seen.insert(id, now.clone());
        let Some(prev) = prev else { return true };
        if inp.paused {
            return true;
        }
        let two = inp.rig.two_handed_melee(id);
        let embedded = inp.rig.embedded(id);
        // A contact's damagers are tried point first, then edge, then blunt: the first that hits a
        // body takes it.
        let mut order: Vec<usize> = (0..blades.len()).collect();
        order.sort_by_key(|&k| match blades[k].kind {
            DMG_PIERCE => 0,
            DMG_SLASH => 1,
            _ => 2,
        });
        let mut struck: Vec<(u32, usize)> = Vec::new();
        for bi in order {
            let b = blades[bi];
            let along = (wq * (Vec3::from_array(b.b) - Vec3::from_array(b.a))).normalize_or_zero();
            let edge = if b.kind == DMG_PIERCE { along } else { (wq * Vec3::from_array(b.edge)).normalize_or_zero() };
            let Some(hit) = self.judge_sweep(inp, bodies, hand, id, bi, &b, w.tier, along, edge, &prev[bi], &now[bi], dt, two, &struck, events) else {
                // A point or an axe meeting the world's wood (or a loose thing) bites in as fast as
                // the damage tables let it into wood (a stab 3 m/s, an axe 5).
                let axe = b.kind == DMG_SLASH && b.flags & DMGF_AXE != 0;
                if (b.kind == DMG_PIERCE || axe) && b.depth > 0.0 && !embedded && !self.embeds.iter().any(|e| e.object == id) {
                    let row = rustvr_melee::row(rustvr_melee::modifier_of(&b), MAT_WOOD, w.tier, false);
                    let speed = row.map_or(f32::INFINITY, |r| r.pen_speed) * inp.map.world_per_meter;
                    let way = if axe { edge } else { along };
                    let (a, bb) = (Vec3::from_array(b.a), Vec3::from_array(b.b));
                    let local = |p: Vec3| {
                        let l = wq.inverse() * (p - wp);
                        let t = if (bb - a).length_squared() > 1e-9 { ((l - a).dot(bb - a) / (bb - a).length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                        a.lerp(bb, t)
                    };
                    let dampers = [b.damper_in, b.damper_out, b.damper_free].map(|d| d * row.map_or(1.0, |r| r.damper));
                    let max_depth = b.depth.min(row.map_or(0.0, |r| r.depth_cap));
                    // In as far as its momentum carries it against the damper going in (as into a
                    // body; a thrown point all the way).
                    let thrown = inp.rig.held(hand) != Some(id);
                    let mass = inp.rig.mass_of(id);
                    let request = |into, point, speed: f32| {
                        let depth = if thrown && b.kind == DMG_PIERCE { max_depth } else { (mass * speed / dampers[0].max(1.0)).min(max_depth) };
                        embed::Request { object: id, into, point, dir: way, sample: local(point), depth, max_depth, dampers, slide: None, wounding: Default::default(), rock: rock_of(&b) }
                    };
                    if let Some((point, _, v)) = self.bite_world(inp, b.kind, b.flags, along, edge, &prev[bi], &now[bi], dt, speed) {
                        self.embeds.push(request(embed::Into::World, point, v / inp.map.world_per_meter));
                    } else if let Some((thing, point, v)) = Self::bite_thing(inp, id, b.kind, b.flags, along, edge, &prev[bi], &now[bi], dt, speed) {
                        self.embeds.push(request(embed::Into::Object { id: thing }, point, v / inp.map.world_per_meter));
                    }
                }
                continue;
            };
            struck.push((hit.actor, hit.index));
            // Gone in: it stays in (a cut 3 mm and sliding along its edge, an axe and a point as
            // deep as they go), unless it's already in this creature; one already in another runs
            // on into it.
            if hit.judged.flags & CEF_PENETRATED != 0 && !inp.rig.embedded_in(id, hit.actor) && !self.embeds.iter().any(|e| e.object == id) {
                self.embeds.push(self.judge_embed(inp, id, &b, &hit, along, edge, (wp, wq), inp.rig.held(hand) != Some(id)));
            }
        }
        // (A thrown one meeting a fighter's blade just glances off it.)
        if inp.rig.held(hand) == Some(id) {
            let desc = Self::as_desc(&w);
            self.blocks(inp, hand, id, &desc, &prev, &now, dt, events);
            self.item_blocks(inp, hand, id, &desc, &prev, &now, dt, events);
        }
        true
    }

    /// What a hit sounds like (by the two materials and the damage type): metal on metal clashes, a
    /// blade into wood chops, flesh is cut, stabbed or thumped.
    fn hit_cue(kind: u32, material: u32, source: Stuff) -> crate::rig::feedback::Cue {
        use crate::rig::feedback::Cue;
        match (source, material, kind) {
            // Metal on metal clashes; wood or a fist on it only knocks.
            (Stuff::Metal, MAT_CHAINMAIL | MAT_PLATE | MAT_STONE, _) => Cue::Clash,
            (_, MAT_CHAINMAIL | MAT_PLATE | MAT_STONE, _) => Cue::HitBlunt,
            (Stuff::Metal, MAT_WOOD | MAT_BONE, DMG_SLASH | DMG_PIERCE) => Cue::Chop,
            (_, _, DMG_SLASH) => Cue::HitCut,
            (_, _, DMG_PIERCE) => Cue::HitStab,
            _ => Cue::HitBlunt,
        }
    }

    /// What a weapon is made of, for its sounds (the hit's source material): a hand's flesh, a
    /// staff's wood, any other weapon's metal.
    fn source_of(&self, object: u32) -> Stuff {
        match self.weapons.get(object) {
            _ if object == 0 => Stuff::Flesh,
            Some(w) if w.flags & MW_STAFF != 0 => Stuff::Wood,
            _ => Stuff::Metal,
        }
    }

    /// The older table's damagers for a weapon described in [`MeleeWeapons`] (for the blocks).
    fn as_desc(w: &MeleeWeapon) -> MeleeDesc {
        let mut d = MeleeDesc { id: w.id, ..Default::default() };
        for b in w.blades().iter().take(MAX_DAMAGERS) {
            d.damagers[d.count as usize] = Damager { kind: b.kind, flags: b.flags, depth: b.depth, radius: b.radius, a: b.a, b: b.b, edge: b.edge, ..Default::default() };
            d.count += 1;
        }
        d
    }

    /// One damager's sweep against every body: the earliest new contact, judged and reported.
    #[allow(clippy::too_many_arguments)]
    fn judge_sweep(&mut self, inp: &CombatInput, bodies: &[Body], hand: usize, object: u32, blade: usize, b: &MeleeBlade, tier: u32, along: Vec3, edge: Vec3, prev: &[Vec3; SAMPLES], now: &[Vec3; SAMPLES], dt: f32, two: bool, struck: &[(u32, usize)], events: &mut Vec<CombatEvent>) -> Option<JudgedHit> {
        let w = inp.map.world_per_meter;
        let mut best: Option<(f32, usize, Vec3, Vec3, Vec3)> = None;
        for (bi, body) in bodies.iter().enumerate() {
            let (a, bb) = (Vec3::from_array(body.part.a), Vec3::from_array(body.part.b));
            let reach = body.part.radius + b.radius * w;
            // Where along its sweep the sample first touches the capsule (not where it's deepest:
            // a fast blade passes right through a limb in a frame, and the surface it met is
            // where it went in).
            let contact = |k: usize| entry(prev[k], now[k], a, bb, reach);
            let entry = (body.actor.id, body.index, object, hand, group_of(b, blade));
            // Still in contact (or lost it only a few frames ago): no new hit (a contact is one hit
            // until it's been lost 5 frames), but a point pressed on, or an edge drawn, may go in
            // by pressure.
            if self.contacts.get(&entry).is_some_and(|&last| self.frame - last <= CONTACT_FRAMES) {
                if (0..SAMPLES).any(|k| contact(k).3) {
                    self.contacts.insert(entry, self.frame);
                    // A point pushed in, or an edge pressed on and drawn, goes in by pressure
                    // (`Self::pressure`). (Only a hand pushes: a thrown blade that didn't go in rests
                    // where it is; one already in this creature is in; and another of its damagers
                    // that struck the part this frame took it.)
                    let cooling = self.cooldowns.get(&(object, blade, hand)).is_some_and(|&(part, until)| part == (body.actor.id, body.index) && until > inp.now);
                    let taken = struck.contains(&(body.actor.id, body.index));
                    if matches!(b.kind, DMG_PIERCE | DMG_SLASH) && b.flags & DMGF_PRESSURE != 0 && !inp.rig.embedded_in(object, body.actor.id) && inp.rig.held(hand) == Some(object) && !cooling && !taken {
                        let k = SAMPLES - 1;
                        let (_, p, on_axis, _) = contact(k);
                        let normal = (p - on_axis).normalize_or(-along);
                        if let Some(hit) = self.pressure(inp, object, b, tier, along, edge, normal, body, p, (now[k] - prev[k]) / dt - body.vel) {
                            self.cooldowns.insert((object, blade, hand), ((body.actor.id, body.index), inp.now + rustvr_melee::HIT_DELAY));
                            return Some(hit);
                        }
                    }
                }
                continue;
            }
            // (The same damager doesn't hit the part it last hit again within its delay; another
            // of the weapon's damagers that hit the part this frame took it.)
            if struck.contains(&(body.actor.id, body.index)) || self.cooldowns.get(&(object, blade, hand)).is_some_and(|&(part, until)| part == (body.actor.id, body.index) && until > inp.now) {
                continue;
            }
            for k in 0..SAMPLES {
                let (s, p, on_axis, touches) = contact(k);
                if touches && best.is_none_or(|(bs, ..)| s < bs) {
                    let vel = (now[k] - prev[k]) / dt - body.vel;
                    let normal = (p - on_axis).normalize_or(-vel.normalize_or_zero());
                    best = Some((s, bi, p, vel, normal));
                }
            }
        }
        let (_, bi, point, vel, normal) = best?;
        let body = &bodies[bi];
        self.contacts.insert((body.actor.id, body.index, object, hand, group_of(b, blade)), self.frame);
        // A blow on an arm or a hand that went through the creature's shield on its way there is
        // the shield's (a ray back along the blade's way since last frame, from a little past where
        // it met to a little behind where it was).
        if matches!(body.part.kind, PART_ARM | PART_HAND) {
            let way = (vel + body.vel) * dt;
            let back = -way.normalize_or_zero();
            let from = point - back * 0.1 * w;
            let shields = self.items.entries().iter().filter(|i| i.actor == body.actor.id && i.flags & AI_SHIELD != 0);
            if back != Vec3::ZERO && shields.into_iter().any(|i| inp.rig.ray_meets(from, back, way.length() + 0.2 * w, i.id)) {
                return None;
            }
        }
        // (A hand's own blow, object 0, is never thrown.)
        let thrown = object != 0 && !inp.rig.held(hand).is_some_and(|h| h == object);
        let contact = rustvr_melee::Contact { along, edge, vel: vel / w, normal, material: body.part.material, tier, thrown, fire: self.imbued_with_fire(object) };
        let judged = rustvr_melee::judge(b, &contact);
        if debug_combat() {
            let dir = vel.normalize_or_zero();
            let across = (dir - along * dir.dot(along)).normalize_or_zero();
            let deg = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
            tracing::info!(
                "combat: blade {blade} (kind {}) of {object:#x} met {:#x} part {} (kind {}, material {}): {:.1} m/s, across the blade vs its edge {:.0} deg, into the surface vs the edge {:.0} deg, along vs its way {:.0} deg -> {judged:?}",
                b.kind,
                body.actor.id,
                body.index,
                body.part.kind,
                body.part.material,
                vel.length() / w,
                deg(across, edge),
                deg(-normal, edge),
                deg(dir, along)
            );
        }
        let Some(mut judged) = judged else {
            // A contact that does nothing (too slow for what it met, or at a bad angle with no
            // blunt to fall back on) is still heard, softly, and it scratches or scuffs armour (a
            // touch: its soft sound, its mark on the armour).
            let speed = vel.length() / w;
            if speed >= rustvr_melee::INTENSITY_FROM {
                let k = rustvr_melee::intensity(speed);
                self.sounds.push((Self::hit_cue(b.kind, body.part.material, self.source_of(object)), point, rustvr_melee::soft_volume(k), object));
                events.push(CombatEvent {
                    kind: CE_TOUCH,
                    actor: body.actor.id,
                    part: body.part.game_part,
                    object,
                    what: b.kind,
                    hand: hand as u32,
                    material: body.part.material,
                    strength: k,
                    speed,
                    point: point.to_array(),
                    blade: blade as u32,
                    dir: vel.normalize_or_zero().to_array(),
                    ..Default::default()
                });
            }
            return None;
        };
        self.cooldowns.insert((object, blade, hand), ((body.actor.id, body.index), inp.now + rustvr_melee::HIT_DELAY));
        // Just after it cut something off, the damager doesn't go in: it carries on through.
        if self.sliced.get(&(object, blade)).is_some_and(|&t| inp.now - t < rustvr_melee::SLICE_NO_PENETRATION) {
            judged.flags &= !CEF_PENETRATED;
            judged.depth = 0.0;
        }
        let mut flags = judged.flags | if two { CEF_TWO_HANDED } else { 0 } | if thrown { CEF_THROWN } else { 0 };
        // Gone in fast and square enough at a joint, it cuts through: the part it takes off (the one
        // hit, or the one beside it whose joint it crossed) is the one told, and the game cuts it
        // off. It doesn't stay in: it carries on at the speed it met the limb at.
        let mut part = body.part;
        let parts: Vec<(Vec3, Vec3, f32, u32)> = body.actor.parts[..(body.actor.count as usize).min(MAX_ACTOR_PARTS)].iter().map(|p| (Vec3::from_array(p.a), Vec3::from_array(p.b), p.radius, p.kind)).collect();
        let forward = if b.kind == DMG_PIERCE { along } else { edge };
        let up = if b.kind == DMG_PIERCE { edge.reject_from(along).normalize_or_zero() } else { along };
        if let Some(cut) = rustvr_melee::sever_part(b, &judged, tier, &parts, body.index, point, forward, up) {
            flags = (flags | CEF_SEVER) & !CEF_PENETRATED;
            judged.flags &= !CEF_PENETRATED;
            part = &body.actor.parts[cut];
            self.sliced.insert((object, blade), inp.now);
            self.cuts.push((object, body.actor.id));
        }
        let spell = self.imbued(object).map_or(0, |i| i.spell);
        if spell != 0 {
            flags |= CEF_IMBUED;
        }
        let ev = CombatEvent {
            kind: CE_HIT,
            flags,
            actor: body.actor.id,
            part: part.game_part,
            object,
            what: judged.kind,
            hand: hand as u32,
            material: body.part.material,
            strength: Self::strength_of(judged.kind, judged.speed),
            speed: judged.speed,
            depth: judged.depth,
            damage: judged.damage,
            point: point.to_array(),
            blade: blade as u32,
            dir: vel.normalize_or_zero().to_array(),
            spell,
        };
        tracing::info!(
            "combat: {object:#x}'s {} hit {:#x} part {} at {:.1} m/s: {:.2} damage{}{}{}",
            match judged.kind {
                DMG_SLASH => "edge",
                DMG_PIERCE => "point",
                _ => "blunt",
            },
            body.actor.id,
            body.index,
            judged.speed,
            judged.damage,
            if flags & CEF_PENETRATED != 0 { ", in" } else { "" },
            if flags & CEF_SEVER != 0 { ", severing" } else { "" },
            if flags & CEF_GLANCING != 0 { " (glancing)" } else { "" }
        );
        self.debug.hits += 1;
        self.debug.last = Some(ev);
        // The hit felt in the hand(s) holding it, as loud as its sound (the buzz follows the hit
        // sound: 0.3 s, at the sound's volume, 0.2 to 1 over the blow's intensity).
        let k = rustvr_melee::blow_intensity(b, judged.speed, judged.damage > 0.0);
        for h in [hand, hand ^ 1] {
            if (h == hand || two) && h < 2 {
                self.pulses.push(Pulse { hand: h, seconds: HIT_BUZZ, frequency: if judged.kind == DMG_BLUNT { 70.0 } else { 120.0 }, amplitude: 0.2 + 0.8 * k });
            }
        }
        self.sounds.push((Self::hit_cue(judged.kind, body.part.material, self.source_of(object)), point, k, object));
        events.push(ev);
        Some(JudgedHit { actor: body.actor.id, index: body.index, part_kind: body.part.kind, a: Vec3::from_array(body.part.a), b: Vec3::from_array(body.part.b), radius: body.part.radius, point, vel, normal, judged })
    }

    /// A damager pressed on a body (in contact since last frame) goes in once the hand pushes it
    /// hard enough for how fast it moves against it: a point pushed in (a pressure stab), an edge
    /// drawn along its length (a draw cut). The push into the body must
    /// lie within the damager's angles of its way in (a point's, or the edge facing the body), as a
    /// blow's does; an edge's draw must run along its length. No damage as it starts; its wound
    /// when it's in.
    #[allow(clippy::too_many_arguments)]
    fn pressure(&mut self, inp: &CombatInput, object: u32, b: &MeleeBlade, tier: u32, along: Vec3, edge: Vec3, normal: Vec3, body: &Body, point: Vec3, vel: Vec3) -> Option<JudgedHit> {
        let w = inp.map.world_per_meter;
        let row = rustvr_melee::row(rustvr_melee::modifier_of(b), body.part.material, tier, self.imbued_with_fire(object))?;
        if row.pressure <= 0.0 || !row.pen_speed.is_finite() || b.depth <= 0.0 {
            return None;
        }
        let (h, v, _) = rustvr_melee::angles(b);
        let into = -normal;
        let (forward, up) = match b.kind {
            DMG_PIERCE => (along, edge.reject_from(along).normalize_or(along.any_orthonormal_vector())),
            // (A double edge presses with whichever faces the body.)
            _ => (if edge.dot(into) < 0.0 && b.flags & DMGF_BOTH_WAYS != 0 { -edge } else { edge }, along),
        };
        if !rustvr_melee::within_angles(into, forward, up, h, v) {
            return None;
        }
        let speed = vel.length() / w;
        if b.kind == DMG_SLASH && (vel.normalize_or_zero().dot(along)).abs() < 1.0 - rustvr_melee::PRESSURE_MAX_DOT {
            return None;
        }
        // Pushed into the body: the hand's joint's pull that way, a step's worth over the weapon's
        // mass (the contact's impulse over the mass).
        let (lag, spring, max_force) = inp.rig.melee_pull(object)?;
        let push = (lag.dot(into) * spring).clamp(0.0, max_force);
        let kick = push * inp.dt.max(1e-4) / inp.rig.mass_of(object);
        if kick < rustvr_melee::pressure_needed(b, speed) * row.pressure {
            return None;
        }
        let judged = rustvr_melee::Judged { damage: 0.0, kind: b.kind, flags: CEF_PENETRATED | CEF_PRESSURE, push: 0, speed, depth: b.depth.min(row.depth_cap), damper: row.damper };
        tracing::info!("combat: {object:#x}'s {} pressed into {:#x} part {} ({push:.0} N)", if b.kind == DMG_SLASH { "edge, drawn," } else { "point" }, body.actor.id, body.index);
        Some(JudgedHit { actor: body.actor.id, index: body.index, part_kind: body.part.kind, a: Vec3::from_array(body.part.a), b: Vec3::from_array(body.part.b), radius: body.part.radius, point, vel, normal, judged })
    }

    /// The stay in a body a blow that went in makes.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn judge_embed(&self, inp: &CombatInput, id: u32, b: &MeleeBlade, hit: &JudgedHit, along: Vec3, edge: Vec3, (wp, wq): (Vec3, Quat), thrown: bool) -> embed::Request {
        let w = inp.map.world_per_meter;
        let (a, bb) = (Vec3::from_array(b.a), Vec3::from_array(b.b));
        let l = wq.inverse() * (hit.point - wp);
        let t = if (bb - a).length_squared() > 1e-9 { ((l - a).dot(bb - a) / (bb - a).length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        let sample = a.lerp(bb, t);
        let axe = b.kind == DMG_SLASH && b.flags & DMGF_AXE != 0;
        // An edge goes in the way it cut: its back edge's way when the blow went more than 90
        // degrees off its front's.
        let cut = if hit.vel.dot(edge) < 0.0 { -edge } else { edge };
        let length = a.distance(bb) / w;
        let (dir, slide) = match b.kind {
            DMG_PIERCE => (along, None),
            _ if axe => (cut, None),
            _ => (cut, (b.flags & DMGF_SLIDES != 0).then(|| (along, (t - 0.5) * length, if b.slide_length > 0.0 { b.slide_length * 0.5 } else { length * 0.5 }))),
        };
        let defaults = if axe { [0.0, 10000.0, 5000.0] } else { embed::DAMPERS };
        let dampers = [b.damper_in, b.damper_out, b.damper_free];
        let dampers: [f32; 3] = std::array::from_fn(|k| if dampers.iter().any(|&d| d > 0.0) { dampers[k] } else { defaults[k] } * hit.judged.damper);
        let max_depth = hit.judged.depth.max(0.003);
        // Its swing carries it in a little (its momentum against the damper going in), the hand's
        // push the rest. A thrown point goes in freely (no damper going in for its first half
        // second or second): all the way.
        let mass = inp.rig.mass_of(id);
        let first = if thrown && b.kind == DMG_PIERCE { max_depth } else { (mass * hit.judged.speed / dampers[0].max(1.0)).min(max_depth) };
        let throat = crate::rig::creatures::on_throat(hit.part_kind, hit.a, hit.b, hit.point);
        let part = rustvr_melee::body::Part::of(hit.part_kind, throat);
        // A point comes out the far side once the blade's point that met the surface is through the
        // part (a skewer: more than 5 cm in, its entry carried past the part's far side); only
        // points skewer.
        let through = if b.kind == DMG_PIERCE { embed::chord(hit.a, hit.b, hit.radius, hit.point, dir, 4.0 * hit.radius) / w } else { 0.0 };
        let wounding = embed::Wounding {
            blade: self.blade_index(id, b),
            kind: b.kind,
            damage: b.penetration_damage,
            deep: if b.kind == DMG_PIERCE { part.deep().0 } else { 0.0 },
            skewer: if b.kind == DMG_PIERCE { b.skewer } else { 0.0 },
            through: through.max(rustvr_melee::SKEWER_DEPTH),
        };
        let into = embed::Into::Actor { id: hit.actor, part: hit.index };
        embed::Request { object: id, into, point: hit.point, dir, sample, depth: first, max_depth, dampers, slide, wounding, rock: rock_of(b) }
    }

    fn blade_index(&self, id: u32, b: &MeleeBlade) -> u32 {
        self.weapons.get(id).and_then(|w| w.blades().iter().position(|x| x == b)).unwrap_or(0) as u32
    }

    /// The spell weapon `id` is filled with, if a hand is imbuing it (or did).
    fn imbued(&self, id: u32) -> Option<Imbue> {
        self.imbues.get(&id).copied().filter(|i| i.charge > 0.0)
    }

    fn imbued_with_fire(&self, id: u32) -> bool {
        self.imbued(id).is_some_and(|i| self.fire_spells.contains(&i.spell))
    }

    /// A hit's strength for a game that scales its own damage by it (1: a full-strength blow), from
    /// the speed past the kind's least.
    fn strength_of(kind: u32, speed: f32) -> f32 {
        let (full, _) = full_and_power(kind);
        let least = match kind {
            DMG_PIERCE => 1.0,
            DMG_SLASH => 3.0,
            _ => 1.0,
        };
        ((speed - least) / (full - least).max(0.5)).clamp(0.05, 1.5)
    }

    // ---- the creatures' weapons ([`ActorItems`]) ------------------------------------------------------
    /// A creature's weapon's blades (world: each damager's ends, radius, and the damager), where the
    /// rig has it now.
    fn item_blades(&self, inp: &CombatInput, id: u32) -> Vec<(Vec3, Vec3, f32, MeleeBlade)> {
        let (Some(w), Some((p, q))) = (self.weapons.get(id), inp.rig.object_world_pose(id)) else { return Vec::new() };
        let wpm = inp.map.world_per_meter;
        w.blades().iter().map(|b| (p + q * Vec3::from_array(b.a), p + q * Vec3::from_array(b.b), b.radius * wpm, *b)).collect()
    }

    /// The player's weapon meeting a creature's: its swing parried, or its guard blocking the
    /// player's blow. Both are told to the game (`CE_BLOCK`, `what` the
    /// creature's weapon), which rolls the creature's dice; `CEF_PARRY` when the player's weapon met
    /// it moving faster than [`PARRY_MIN_SPEED`], 1 m/s.
    #[allow(clippy::too_many_arguments)]
    fn item_blocks(&mut self, inp: &CombatInput, hand: usize, object: u32, desc: &MeleeDesc, prev: &[[Vec3; SAMPLES]], now: &[[Vec3; SAMPLES]], dt: f32, events: &mut Vec<CombatEvent>) {
        let items: Vec<ActorItem> = self.items.entries().to_vec();
        for item in items {
            if self.block_until.get(&item.id).is_some_and(|&t| t > inp.now) {
                continue;
            }
            let theirs = self.item_blades(inp, item.id);
            'mine: for (di, d) in desc.damagers[..now.len()].iter().enumerate() {
                let (a, b) = (now[di][0], now[di][SAMPLES - 1]);
                for &(ta, tb, tr, _) in &theirs {
                    let (s, t) = closest_segments(a, b, ta, tb);
                    let (p, q) = (a.lerp(b, s), ta.lerp(tb, t));
                    if p.distance(q) > tr + d.radius * inp.map.world_per_meter + 0.01 {
                        continue;
                    }
                    let k = ((s * (SAMPLES - 1) as f32).round() as usize).min(SAMPLES - 1);
                    let mine = (now[di][k] - prev[di][k]) / dt;
                    let speed = (mine - Vec3::from_array(item.vel)).length() / inp.map.world_per_meter;
                    let parry = mine.length() / inp.map.world_per_meter > PARRY_MIN_SPEED;
                    self.block_until.insert(item.id, inp.now + BLOCK_COOLDOWN);
                    let ev = CombatEvent {
                        kind: CE_BLOCK,
                        flags: if parry { CEF_PARRY } else { 0 },
                        actor: item.actor,
                        object,
                        what: item.id,
                        hand: hand as u32,
                        material: MAT_PLATE,
                        strength: (speed / 6.0).min(1.5),
                        speed,
                        point: p.to_array(),
                        dir: mine.normalize_or_zero().to_array(),
                        ..Default::default()
                    };
                    tracing::info!("combat: {object:#x} met {:#x}'s {:#x} at {speed:.1} m/s ({})", item.actor, item.id, if item.flags & AI_ATTACKING != 0 { "its swing" } else { "its guard" });
                    self.debug.blocks += 1;
                    self.debug.last = Some(ev);
                    // The clash in the hand: as hard as it was.
                    let k = rustvr_melee::intensity(speed);
                    self.pulses.push(Pulse { hand, seconds: 0.06 + 0.1 * k, frequency: 80.0, amplitude: (0.4 + 0.6 * k).min(1.0) });
                    self.sounds.push((crate::rig::feedback::Cue::Clash, p, k.max(0.1), object));
                    events.push(ev);
                    break 'mine;
                }
            }
        }
    }

    /// The creatures' weapons swinging at the player (`AI_ATTACKING`: in its blow's damage window):
    /// each blade swept from last frame to now against the player's body, judged as any contact but
    /// with no angle test (a creature's blow needn't land square), told to the game (`CE_STRUCK`).
    fn struck(&mut self, inp: &CombatInput, dt: f32, events: &mut Vec<CombatEvent>) {
        let items: Vec<ActorItem> = self.items.entries().iter().filter(|i| i.flags & AI_ATTACKING != 0).copied().collect();
        let mut now_blades: HashMap<u32, Vec<(Vec3, Vec3, f32, MeleeBlade)>> = HashMap::new();
        for item in &items {
            now_blades.insert(item.id, self.item_blades(inp, item.id));
        }
        // The player's own parts' motion since last frame: a blow is judged by how fast it meets
        // them, not how fast it moves.
        let player_vel: Vec<Vec3> = inp.player.iter().enumerate().map(|(i, &(ca, cb, ..))| match self.player_prev.get(i) {
            Some(&(pa, pb)) if dt > 0.0 => ((ca + cb) - (pa + pb)) * 0.5 / dt,
            _ => Vec3::ZERO,
        }).collect();
        for item in items {
            let blades = &now_blades[&item.id];
            let Some(prev) = self.item_prev.get(&item.id).filter(|p| p.len() == blades.len()).cloned() else { continue };
            // (A blow the player's weapon or shield just stopped doesn't go on into the player.)
            if self.block_until.get(&item.id).is_some_and(|&t| t - BLOCK_COOLDOWN + STRUCK_AFTER_BLOCK > inp.now) {
                continue;
            }
            let tier = self.weapons.get(item.id).map_or(2, |w| w.tier);
            'blades: for (bi, &(a, b, r, blade)) in blades.iter().enumerate() {
                let (pa, pb) = prev[bi];
                for (pi, &(ca, cb, cr, kind)) in inp.player.iter().enumerate() {
                    if self.struck_until.get(&(item.id, bi, pi)).is_some_and(|&t| t > inp.now) {
                        continue;
                    }
                    // The sweeps of points all along it against the capsule.
                    for k in 0..SAMPLES {
                        let f = k as f32 / (SAMPLES - 1) as f32;
                        let (p0, p1) = (pa.lerp(pb, f), a.lerp(b, f));
                        let (s, t) = closest_segments(p0, p1, ca, cb);
                        let (p, q) = (p0.lerp(p1, s), ca.lerp(cb, t));
                        if p.distance(q) > r + cr {
                            continue;
                        }
                        // How fast it moves there, as the game has it move (its own physics': the
                        // game's poses come at its own pace, so their steps would jump), against
                        // the player's part.
                        let centre = inp.rig.object_world_pose(item.id).map_or(p, |(c, _)| c);
                        let vel = (Vec3::from_array(item.vel) + Vec3::from_array(item.ang_vel).cross(p - centre) - player_vel[pi]) / inp.map.world_per_meter;
                        let normal = (p - q).normalize_or(-vel.normalize_or_zero());
                        let along = (b - a).normalize_or_zero();
                        let mut free = blade;
                        free.horizontal = 180.0;
                        free.vertical = 180.0;
                        free.normal = 180.0;
                        let contact = rustvr_melee::Contact { along, edge: along, vel, normal, material: MAT_FLESH, tier, thrown: false, fire: false };
                        let Some(j) = rustvr_melee::judge(&free, &contact) else { continue };
                        self.struck_until.insert((item.id, bi, pi), inp.now + STRUCK_COOLDOWN);
                        let ev = CombatEvent { kind: CE_STRUCK, flags: j.flags, actor: item.actor, part: kind, object: item.id, what: j.kind, material: MAT_FLESH, strength: Self::strength_of(j.kind, j.speed), speed: j.speed, damage: j.damage, point: p.to_array(), blade: bi as u32, dir: vel.normalize_or_zero().to_array(), ..Default::default() };
                        tracing::info!("combat: struck by {:#x}'s {:#x} at {:.1} m/s on the player's part kind {kind}", item.actor, item.id, j.speed);
                        // Felt in both hands: the player was hit.
                        for h in 0..2 {
                            self.pulses.push(Pulse { hand: h, seconds: 0.15, frequency: 60.0, amplitude: 1.0 });
                        }
                        self.sounds.push((Self::hit_cue(j.kind, MAT_FLESH, Stuff::Metal), p, rustvr_melee::intensity(j.speed), item.id));
                        events.push(ev);
                        break 'blades;
                    }
                }
            }
        }
        self.item_prev = now_blades.into_iter().map(|(id, v)| (id, v.into_iter().map(|(a, b, ..)| (a, b)).collect())).collect();
        self.player_prev = inp.player.iter().map(|&(a, b, ..)| (a, b)).collect();
    }

    fn magic(&mut self, inp: &CombatInput, bodies: &[Body], events: &mut Vec<CombatEvent>) {
        self.charging = [None, None];
        self.casting = Default::default();
        let spell = self.selected().copied();
        let hands = self.spell_hands(inp.dominant);
        let spell_time = if self.spells.cast_time > 0.05 { self.spells.cast_time } else { 0.5 };
        let game_art = self.spells.flags & SPELLS_GAME_ART != 0;
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
            // What it casts with: a held thing that casts, else (the spell hand, empty) the spell.
            let held = inp.rig.held(h);
            let thing = held.and_then(|id| inp.objects.descs[..(inp.objects.count as usize).min(MAX_OBJECTS)].iter().find(|d| d.id == id && d.flags & OBJF_CASTS != 0 && d.casts != 0).copied());
            let free = held.is_none() && !inp.out.hand_busy[h] && (!inp.out.hand_targeting[h] || inp.out.hand_glove_hovers[h]);
            let source = match (thing, spell) {
                (Some(d), _) => Some(Source::Thing(d)),
                (None, Some(s)) if hands[h] && free && s.flags & SPELL_CASTABLE != 0 => Some(Source::Spell(s)),
                _ => None,
            };
            // Where it comes from (world, and the way it points), and where its art goes (tracking:
            // off the palm and out of it, or the thing's tip and along it).
            let emitter = match source {
                Some(Source::Thing(d)) => held.and_then(|id| inp.rig.object_world_pose(id)).map(|(p, q)| {
                    let tip = p + q * Vec3::from_array(d.muzzle);
                    (tip, q * Vec3::Z, Pose { pos: inp.map.inv_point(tip).to_array(), valid: 1, rot: inp.map.inv_rot(q).to_array() })
                }),
                Some(Source::Spell(_)) => {
                    // The palm faces the hand's -X (left) or +X (right) in its joint's frame.
                    let out = hq * Vec3::X * if h == 0 { -1.0 } else { 1.0 };
                    let fingers = hq * Vec3::Y;
                    let rot = Quat::from_mat3(&glam::Mat3::from_cols(fingers.cross(out), fingers, out)).normalize();
                    Some((palm, inp.map.rot(hq) * Vec3::Y, Pose { pos: (palm_t + out * PALM_OUT).to_array(), valid: 1, rot: rot.to_array() }))
                }
                None => None,
            };
            let c = &mut self.casters[h];
            let (Some(source), Some((from, along, at)), false) = (source, emitter, inp.paused) else {
                c.charging = false;
                c.charge = 0.0;
                c.ready = false;
                c.state = CAST_NONE;
                continue;
            };
            let (id, cast_time, delivery, name) = match source {
                Source::Spell(s) => (s.id, spell_time, s.delivery, s.name().to_owned()),
                Source::Thing(d) => (d.casts, if d.cast_time > 0.05 { d.cast_time } else { spell_time }, DELIVER_TARGET, name_str(&d.name).to_owned()),
            };
            let trigger = inp.frame.hands[h].trigger;
            let finger_on = inp.frame.hands[h].buttons & CTRL_TOUCH_TRIGGER != 0;
            if trigger < TRIGGER_OFF {
                c.held_after_cast = false;
            }
            if !c.charging && trigger > TRIGGER_ON && !c.held_after_cast && inp.now >= c.cooldown_until {
                c.charging = true;
                c.charge = 0.0;
                c.ready = false;
                c.next_pulse = inp.now;
            }
            let mut cast = None;
            if c.charging {
                c.charge = (c.charge + inp.dt / cast_time).min(1.0);
                if c.charge >= 1.0 && !c.ready {
                    c.ready = true;
                    c.next_pulse = inp.now + READY_TICK;
                    self.pulses.push(Pulse { hand: h, seconds: 0.03, frequency: 250.0, amplitude: 1.0 });
                } else if inp.now >= c.next_pulse {
                    let (seconds, amplitude) = if c.ready {
                        c.next_pulse = inp.now + READY_TICK;
                        (0.015, 0.1)
                    } else {
                        c.next_pulse = inp.now + CHARGE_GAP_SLOW + (CHARGE_GAP_FAST - CHARGE_GAP_SLOW) * c.charge as f64;
                        (0.012, CHARGE_FAINT + CHARGE_STRONG * c.charge.powi(4))
                    };
                    self.pulses.push(Pulse { hand: h, seconds, frequency: 160.0, amplitude });
                }
                if !game_art {
                    let color = match source {
                        Source::Spell(s) => s.color,
                        Source::Thing(_) => [0.8, 0.7, 1.0],
                    };
                    self.charging[h] = Some(Charging { spell: id, charge: c.charge, color, palm: Vec3::from_array(at.pos) });
                }
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
                let touch_now = delivery == DELIVER_TOUCH && c.ready && touched.is_some();
                if released || touch_now {
                    c.charging = false;
                    // (Let go too soon: nothing.)
                    if c.ready {
                        cast = Some((released, touched));
                    }
                }
            }
            let state = if c.charging {
                if c.ready { CAST_READY } else { CAST_CHARGING }
            } else if (finger_on || trigger > TRIGGER_READY) && !c.held_after_cast && cast.is_none() {
                CAST_READIED
            } else {
                CAST_NONE
            };
            if state != c.state {
                c.state = state;
                c.since = inp.now;
            }
            let charge = c.charge;
            let vel = c.vel;
            let mut out = rustvr_protocol::Caster {
                state,
                flags: if matches!(source, Source::Thing(_)) { CASTF_OBJECT } else { 0 },
                spell: id,
                object: if let Source::Thing(d) = source { d.id } else { 0 },
                charge: if state == CAST_READIED { 0.0 } else { charge },
                time: (inp.now - c.since) as f32,
                at,
                ..Default::default()
            };
            let aim = match source {
                Source::Thing(_) => self.assist(along, from),
                Source::Spell(s) => match s.delivery {
                    DELIVER_SELF => (inp.map.rot(Quat::from_array(inp.frame.hmd.rot)) * Vec3::Z, 0),
                    DELIVER_TOUCH => (vel.normalize_or(inp.map.rot(hq) * Vec3::Y), 0),
                    _ => self.aim(inp, h, palm, vel),
                },
            };
            out.aim = (inp.map.inv_rot(Quat::IDENTITY) * aim.0).normalize_or_zero().to_array();
            let Some((released, touched)) = cast else {
                self.casting[h] = out;
                continue;
            };
            let (dir, actor) = if delivery == DELIVER_TOUCH { (aim.0, touched.unwrap_or(0)) } else { aim };
            let c = &mut self.casters[h];
            c.cooldown_until = inp.now + 0.25;
            c.held_after_cast = !released;
            out.flags |= CASTF_CAST;
            self.casting[h] = out;
            let ev = CombatEvent {
                kind: CE_CAST,
                actor,
                what: id,
                object: out.object,
                hand: h as u32,
                strength: charge,
                speed: vel.length(),
                point: from.to_array(),
                dir: dir.to_array(),
                ..Default::default()
            };
            self.debug.casts += 1;
            self.debug.last = Some(ev);
            tracing::info!("combat: cast {name} from the {} hand{} at {from:.2?} towards {dir:.2?} (on {actor})", if h == 0 { "left" } else { "right" }, if out.object != 0 { "'s staff" } else { "" });
            self.pulses.push(Pulse { hand: h, seconds: 0.1, frequency: 90.0, amplitude: 1.0 });
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
        self.assist(dir, palm)
    }

    /// Aim assist: the way to the creature whose body (the middle of a torso part: a chest or a
    /// pelvis) is nearest the line from `palm` along `dir`, within a few degrees, and its id; else
    /// `dir` itself.
    fn assist(&self, dir: Vec3, palm: Vec3) -> (Vec3, u32) {
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
        judge(&Contact { kind: DMG_SLASH, flags: 0, along: Vec3::Y, edge: Vec3::X, vel, normal, material: MAT_FLESH })
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
        let plate = |v: f32| judge(&Contact { kind: DMG_SLASH, flags: 0, along: Vec3::Y, edge: Vec3::X, vel: Vec3::X * v, normal: -Vec3::X, material: MAT_PLATE });
        assert!(plate(3.0).is_none() && plate(5.0).is_some());
        let stab = |vel: Vec3| judge(&Contact { kind: DMG_PIERCE, flags: 0, along: Vec3::Y, edge: Vec3::X, vel, normal: -Vec3::Y, material: MAT_FLESH });
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
