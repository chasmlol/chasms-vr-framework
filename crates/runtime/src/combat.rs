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
//!   meet the surface within 60 degrees;
//! * a point must be moving along the blade within 35 degrees.
//!
//! A blade at a bad angle is a weak, glancing blunt hit. The speed sets the strength, and fast
//! enough is a power hit (the game's stagger). Each creature takes one hit per swing from each
//! weapon. An empty fist punches. The player's weapon meeting a creature's attacking weapon is a
//! block, and a parry when it's moving into it.
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
/// The damagers' angle limits (degrees).
const SLASH_ACROSS_DEG: f32 = 35.0;
const PIERCE_DEG: f32 = 35.0;
const NORMAL_DEG: f32 = 60.0;
/// A glancing blow's strength, against a clean one's.
const GLANCING: f32 = 0.3;
/// Above this the parry is a parry: the player's weapon moving into the attack (m/s).
const PARRY_SPEED: f32 = 2.0;
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
            ways.iter().any(|&s| {
                let e = c.edge * s;
                within(across, e, SLASH_ACROSS_DEG) && within(-c.normal, e, NORMAL_DEG)
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
    let min = min_speed(kind, c.material);
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

pub struct Combat {
    actors: Box<Actors>,
    melee: Box<MeleeTable>,
    spells: Box<Spells>,
    /// Each held weapon's damager points last frame (world), by object (0, 1: the fists).
    prev: HashMap<u32, Vec<[Vec3; SAMPLES]>>,
    /// Each body part's centre last frame, by (actor, part index).
    prev_parts: HashMap<(u32, usize), Vec3>,
    hit_until: HashMap<(u32, u32), f64>,
    block_until: HashMap<u32, f64>,
    casters: [Caster; 2],
    pub pulses: Vec<Pulse>,
    pub charging: [Option<Charging>; 2],
    pub debug: CombatDebug,
}

impl Default for Combat {
    fn default() -> Self {
        Combat {
            actors: bytemuck::zeroed_box(),
            melee: bytemuck::zeroed_box(),
            spells: bytemuck::zeroed_box(),
            prev: HashMap::new(),
            prev_parts: HashMap::new(),
            hit_until: HashMap::new(),
            block_until: HashMap::new(),
            casters: Default::default(),
            pulses: Vec::new(),
            charging: [None, None],
            debug: CombatDebug::default(),
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

impl Combat {
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

    /// One frame: hits, blocks and casts (each returned for the game, and counted).
    pub fn update(&mut self, inp: &CombatInput) -> Vec<CombatEvent> {
        self.pulses.clear();
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
            let weapon = held.and_then(|id| Some((id, *self.melee_desc(id)?, inp.rig.object_world_pose(id)?)));
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
                        self.sweep(inp, &bodies, h, id, d.kind, d.flags, d.radius, frame, edge, &prev[di], &now[di], dt, &mut events);
                    }
                    self.blocks(inp, h, id, &desc, &prev, &now, dt, &mut events);
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
                    self.sweep(inp, &bodies, h, 0, DMG_PUNCH, 0, FIST_RADIUS, along, along, &prev[0], &now[0], dt, &mut events);
                }
                seen.insert(key, now.to_vec());
            }
        }
        self.prev = seen;
        self.magic(inp, &bodies, &mut events);
        self.hit_until.retain(|_, &mut t| t > inp.now);
        self.block_until.retain(|_, &mut t| t > inp.now);
        events
    }

    /// One damager's sweep (its sample points from `prev` to `now`) against every body.
    #[allow(clippy::too_many_arguments)]
    fn sweep(&mut self, inp: &CombatInput, bodies: &[Body], hand: usize, object: u32, kind: u32, flags: u32, radius: f32, along: Vec3, edge: Vec3, prev: &[Vec3; SAMPLES], now: &[Vec3; SAMPLES], dt: f32, events: &mut Vec<CombatEvent>) {
        // The earliest contact of any sample with any body.
        let mut best: Option<(f32, usize, Vec3, Vec3, Vec3)> = None;
        for body in bodies {
            let key = (body.actor.id, object);
            if self.hit_until.get(&key).is_some_and(|&t| t > inp.now) {
                continue;
            }
            let (a, b) = (Vec3::from_array(body.part.a), Vec3::from_array(body.part.b));
            let reach = body.part.radius + radius;
            for k in 0..SAMPLES {
                let (s, t) = closest_segments(prev[k], now[k], a, b);
                let p = prev[k].lerp(now[k], s);
                let on_axis = a.lerp(b, t);
                if p.distance(on_axis) <= reach && best.is_none_or(|(bs, ..)| s < bs) {
                    let vel = (now[k] - prev[k]) / dt - body.vel;
                    let normal = (p - on_axis).normalize_or(-vel.normalize_or_zero());
                    best = Some((s, bodies.iter().position(|x| std::ptr::eq(x, body)).unwrap(), p, vel, normal));
                }
            }
        }
        let Some((_, bi, point, vel, normal)) = best else { return };
        let body = &bodies[bi];
        let contact = Contact { kind, flags, along, edge, vel, normal, material: body.part.material };
        let Some((strength, cef)) = judge(&contact) else { return };
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
        let _ = body.index;
        self.debug.hits += 1;
        self.debug.last = Some(ev);
        self.pulses.push(Pulse { hand, seconds: 0.05 + 0.07 * strength.min(1.0), frequency: 110.0, amplitude: (0.35 + 0.55 * strength).min(1.0) });
        events.push(ev);
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
        // Aim assist: the creature whose middle is nearest the line, within a few degrees.
        let n = (self.actors.count as usize).min(MAX_ACTORS);
        let mut best: Option<(f32, Vec3, u32)> = None;
        for a in &self.actors.actors[..n] {
            if a.flags & ACT_ALIVE == 0 || a.count == 0 {
                continue;
            }
            let torso = a.parts[..a.count as usize].iter().find(|p| p.kind == PART_TORSO).unwrap_or(&a.parts[0]);
            let c = (Vec3::from_array(torso.a) + Vec3::from_array(torso.b)) * 0.5;
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
