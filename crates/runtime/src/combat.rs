//! Combat: what the player's hands do to the game's creatures, judged from what they physically do.
//! The game says where its creatures are ([`Actors`]), what each melee weapon's blade is
//! ([`MeleeTable`]). The runtime sends back hits and blocks ([`CombatEvent`]), and the game does
//! the rest: damage, stagger, sounds.
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
//! **Magic** is `crate::magic`'s: the hands hold the player's spells ([`Spells`]), charge them and
//! cast them, and it takes the creatures from here.

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
/// Above this the parry is a parry: the player's weapon moving into the attack (m/s).
const PARRY_SPEED: f32 = 2.0;
/// The chest, this far under the eyes (metres): where a blow's way ends.
const CHEST_BELOW_EYES: f32 = 0.35;
/// A guard closer to the chest than this is the body itself being hit, not a block (metres).
const BODY_SPARE: f32 = 0.15;
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
    pub last: Option<CombatEvent>,
}

pub struct Combat {
    actors: Box<Actors>,
    melee: Box<MeleeTable>,
    /// Each held weapon's damager points last frame (world), by object (0, 1: the fists).
    prev: HashMap<u32, Vec<[Vec3; SAMPLES]>>,
    /// Each body part's centre last frame, by (actor, part index).
    prev_parts: HashMap<(u32, usize), Vec3>,
    /// The damagers in a creature last frame (actor, object, damager): judged as they went in, not
    /// again until they've come out.
    touching: std::collections::HashSet<(u32, u32, usize)>,
    touching_now: std::collections::HashSet<(u32, u32, usize)>,
    hit_until: HashMap<(u32, u32), f64>,
    block_until: HashMap<u32, f64>,
    pub pulses: Vec<Pulse>,
    pub debug: CombatDebug,
    /// Blades that went in and stay in, this frame (for the rig).
    pub embeds: Vec<embed::Request>,
}

impl Default for Combat {
    fn default() -> Self {
        Combat {
            actors: bytemuck::zeroed_box(),
            melee: bytemuck::zeroed_box(),
            prev: HashMap::new(),
            prev_parts: HashMap::new(),
            touching: Default::default(),
            touching_now: Default::default(),
            hit_until: HashMap::new(),
            block_until: HashMap::new(),
            pulses: Vec::new(),
            debug: CombatDebug::default(),
            embeds: Vec::new(),
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
    }

    /// For tests: the tables as given.
    pub fn set_tables(&mut self, actors: &Actors, melee: &MeleeTable) {
        *self.actors = *actors;
        *self.melee = *melee;
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
                                self.embeds.push(embed::Request { object: id, into: embed::Into::Actor { id: actor, part }, point, dir: way, sample: local(point), depth, max_depth });
                            }
                            continue;
                        }
                        if let Some((point, _)) = self.bite_world(inp, d.kind, d.flags, frame, edge, &prev[di], &now[di], dt, speed_needed) {
                            self.embeds.push(embed::Request { object: id, into: embed::Into::World, point, dir: way, sample: local(point), depth, max_depth });
                            continue;
                        }
                        if let Some((thing, point)) = Self::bite_thing(inp, id, d.kind, d.flags, frame, edge, &prev[di], &now[di], dt, speed_needed) {
                            self.embeds.push(embed::Request { object: id, into: embed::Into::Object { id: thing }, point, dir: way, sample: local(point), depth, max_depth });
                        }
                    }
                    self.blocks(inp, h, id, &desc, &prev, &now, dt, &mut events);
                }
                seen.insert(id, now);
            } else if held.is_none() && inp.rig.holds_creature(h).is_none() {
                // An empty hand closed into a fist punches (not one closed on a creature it holds).
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
                    self.sweep(inp, &bodies, h, 0, h, DMG_PUNCH, 0, FIST_RADIUS, along, along, &prev[0], &now[0], dt, &mut events);
                }
                seen.insert(key, now.to_vec());
            }
        }
        self.prev = seen;
        self.touching = std::mem::take(&mut self.touching_now);
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
    /// something it bites, at least `speed` fast: (where, its material).
    #[allow(clippy::too_many_arguments)]
    fn bite_world(&self, inp: &CombatInput, kind: u32, flags: u32, along: Vec3, edge: Vec3, prev: &[Vec3; SAMPLES], now: &[Vec3; SAMPLES], dt: f32, speed: f32) -> Option<(Vec3, u32)> {
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
            return clean.then_some((prev[k] + dir * hit.distance, material));
        }
        None
    }

    /// A damager's sweep against the rig's loose things (a crate, a barrel): where it chopped or
    /// stabbed into one (as [`Combat::bite_world`] judges it), at least `speed` fast: (the thing,
    /// where).
    #[allow(clippy::too_many_arguments)]
    fn bite_thing(inp: &CombatInput, weapon: u32, kind: u32, flags: u32, along: Vec3, edge: Vec3, prev: &[Vec3; SAMPLES], now: &[Vec3; SAMPLES], dt: f32, speed: f32) -> Option<(u32, Vec3)> {
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
            return clean.then_some((thing, prev[k] + dir * dist));
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
