//! Gore: the blood the player's blows and the creatures' stumps spill, and the limbs they lose.
//!
//! The game says what its blood looks like ([`BloodTable`]: its own textures per kind), which
//! creature bleeds which ([`GoreBodies`]), where its creatures have stumps, and when one loses a
//! limb ([`GoreEvent`]: the adapter decides that with its game's numbers, by
//! [`rustvr_protocol::severs`]). The runtime makes the blood:
//!
//! * **Wounds.** A clean hit sprays from where the blade met the body, as hard as it hit: a slash
//!   fans along the blade's way and out of the cut, a stab jets back out of the hole, a blunt blow
//!   spatters. Then the wound bleeds a few seconds where it is on the body part, following it.
//! * **Stumps** pump at a heart's beat, each pulse a jet out of the stump that weakens over half a
//!   minute into a flow and then drips (Skyrim's dismemberment mods spray "until the body is
//!   drained"). The severed piece bleeds from its own cut, weaker. The piece is the adapter's prop,
//!   simulated by the rig: it leaves the body at half the body part's speed and pushed off the
//!   blade (3 m/s).
//! * **Blades** carry what they cut through (splats along the edge, as deep as it went in) and drip
//!   from their lowest wet point until they dry.
//!
//! Every drop is a point with gravity and air drag in a pool of [`MAX_DROPS`] (the oldest go
//! first). Drops are collided with the world's static collision (the game's, the rig's physics) by
//! a budget of ray casts a frame ([`PROBES`]): each live drop is probed in turn from where it was
//! last probed to where it is now, so a drop is probed every few frames when there are many; and
//! with the player's hands, body and held blades by distance. A drop that lands becomes a decal
//! where it lands (a splat, streaked along its way when it hits at a slant), joins a pool on a
//! floor (pools grow from what lands in them), or on anything steep may start a run: a point that
//! slides down the surface, probed every few frames, leaving a drip's streak and dropping off
//! edges. Decals are pooled ([`MAX_DECALS`], the oldest go first, pools last) and dry over minutes;
//! they don't fade. Fine mist (no collision, short-lived) goes with every spray, for the look of a
//! lot of blood at the cost of none.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use rustvr_avatar::gore::{self as draw, DECAL_DRIP, DECAL_SPLAT, DECAL_SPLATTER, DECAL_STAIN, GoreBlood, GoreCap, GoreDecal, GoreDrop, GoreSplat};
use rustvr_protocol::*;

use crate::rig::{Rig, RigOutput, WorldMap};

/// The most drops, decals and runs at once.
pub const MAX_DROPS: usize = draw::MAX_DROPS;
pub const MAX_DECALS: usize = draw::MAX_DECALS;
pub const MAX_RUNNERS: usize = 96;
/// Ray casts against the world a frame, shared by every drop in the air.
pub const PROBES: usize = 160;
/// Splats kept on the body and on each held thing.
const MAX_BODY_SPLATS: usize = 20;
const MAX_HELD_SPLATS: usize = 28;

const GRAVITY: f32 = 9.81;
/// A heartbeat (s): the stumps' pulses, and how long each spurt lasts (the pressure rises and falls
/// through it, so its drops land in a line out from the stump).
const BEAT: f64 = 0.78;
const SPURT: f64 = 0.3;
/// How long a stump pumps, flows and drips (s), and how fast its pulses weaken (e-folding, s).
const STUMP_PUMPS: f64 = 14.0;
const STUMP_FLOWS: f64 = 45.0;
const STUMP_DRIPS: f64 = 120.0;
const STUMP_FADE: f64 = 6.0;
/// The severed piece's own cut: shorter, weaker.
const PIECE_PUMPS: f64 = 5.0;
const PIECE_DRIPS: f64 = 30.0;
const PIECE_FADE: f64 = 2.5;
/// Seconds a decal takes to dry (a pool longer), and a blade or a hand.
const DRY: f32 = 150.0;
const POOL_DRY: f32 = 400.0;
const HELD_DRY: f32 = 90.0;
/// Projected decals reach this far either side of the surface they were put on (m): the world's
/// collision and the game's drawn surface differ by a few centimetres.
const DECAL_DEPTH: f32 = 0.07;
/// A hand's reach for drops (m, round the palm).
const HAND_RADIUS: f32 = 0.07;

/// What the rest of the runtime gives the gore each frame.
pub struct GoreInput<'a> {
    pub out: &'a RigOutput,
    pub map: &'a WorldMap,
    /// The player's body (tracking space), for blood that lands on it.
    pub joints: &'a [Pose; JOINT_COUNT],
    /// This frame's hits (`CE_HIT`), and the combat they came from (the creatures, the blades).
    pub hits: &'a [CombatEvent],
    pub combat: &'a crate::combat::Combat,
    pub now: f64,
    pub dt: f32,
    pub paused: bool,
}

#[derive(Clone, Copy, Debug)]
struct Drop {
    pos: Vec3,
    vel: Vec3,
    size: f32,
    blood: u32,
    /// Mist: no collision, it fades.
    mist: bool,
    life: f32,
    age: f32,
    /// Where it was last probed against the world.
    probed: Vec3,
}

#[derive(Clone, Copy, Debug)]
struct Decal {
    center: Vec3,
    normal: Vec3,
    u: Vec3,
    hu: f32,
    hv: f32,
    tex: u32,
    tile: u32,
    blood: u32,
    born: f64,
    wet: f32,
    /// Pools: the blood in it (m³ of drops) and the radius it spreads to.
    volume: f32,
    grow_to: f32,
}

#[derive(Clone, Copy, Debug)]
struct Runner {
    pos: Vec3,
    normal: Vec3,
    start: Vec3,
    speed: f32,
    width: f32,
    volume: f32,
    blood: u32,
    decal: usize,
    born: f64,
    probe_in: u32,
}

/// A wound bleeding on a creature's body part: where on it (the part's frame) and which way out.
#[derive(Clone, Copy, Debug)]
struct Wound {
    actor: u32,
    part: usize,
    at: Vec3,
    out: Vec3,
    blood: u32,
    born: f64,
    until: f64,
    rate: f32,
}

/// A severed piece (the adapter's prop) and its cut.
#[derive(Clone, Copy, Debug)]
struct Piece {
    object: u32,
    cap: Vec3,
    cap_out: Vec3,
    radius: f32,
    blood: u32,
    born: f64,
    /// Its push off the body, until the rig has it loose.
    kick: Option<Vec3>,
    next_beat: f64,
}

/// Blood on the player's body: on a joint (its frame), or on a held thing (its object space).
#[derive(Clone, Copy, Debug)]
struct Splat {
    at: Vec3,
    normal: Vec3,
    radius: f32,
    wet: f32,
    born: f64,
}

#[derive(Clone, Copy, Debug)]
struct StumpTime {
    born: f64,
    next_beat: f64,
    seen: f64,
    /// The spurt under way: when it started, which way, how many drops a second at its height.
    spurt: Option<(f64, Vec3, f32)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GoreStats {
    pub drops: usize,
    pub decals: usize,
    pub pools: usize,
    pub runners: usize,
    pub probes: usize,
    pub severs: u32,
    pub wounds: u32,
}

pub struct Gore {
    bloods: Box<BloodTable>,
    bodies: Box<GoreBodies>,
    events_read: u64,
    looks: Vec<GoreBlood>,
    drops: Vec<Drop>,
    next_probe: usize,
    decals: Vec<Decal>,
    runners: Vec<Runner>,
    wounds: Vec<Wound>,
    pieces: Vec<Piece>,
    stumps: HashMap<(u32, u32), StumpTime>,
    body_splats: Vec<(usize, Splat)>,
    held_splats: HashMap<u32, Vec<Splat>>,
    held_drip: HashMap<u32, f64>,
    splat_blood: u32,
    rng: u64,
    pub stats: GoreStats,
    logged: f64,
    dt: f32,
    /// The hand of the last hit on each creature, and when (for the feel of a limb coming off).
    last_hit: HashMap<u32, (usize, f64)>,
    /// Haptic pulses this frame.
    pub pulses: Vec<crate::combat::Pulse>,
    // This frame's draw lists.
    out_drops: Vec<GoreDrop>,
    out_decals: Vec<GoreDecal>,
    out_caps: Vec<GoreCap>,
    out_splats: Vec<GoreSplat>,
}

impl Default for Gore {
    fn default() -> Self {
        Gore {
            bloods: bytemuck::zeroed_box(),
            bodies: bytemuck::zeroed_box(),
            events_read: 0,
            looks: Vec::new(),
            drops: Vec::with_capacity(MAX_DROPS),
            next_probe: 0,
            decals: Vec::with_capacity(MAX_DECALS),
            runners: Vec::new(),
            wounds: Vec::new(),
            pieces: Vec::new(),
            stumps: HashMap::new(),
            body_splats: Vec::new(),
            held_splats: HashMap::new(),
            held_drip: HashMap::new(),
            splat_blood: 0,
            rng: 0x9E37_79B9_7F4A_7C15,
            stats: GoreStats::default(),
            logged: 0.0,
            dt: 1.0 / 90.0,
            last_hit: HashMap::new(),
            pulses: Vec::new(),
            out_drops: Vec::new(),
            out_decals: Vec::new(),
            out_caps: Vec::new(),
            out_splats: Vec::new(),
        }
    }
}

/// A unit vector within `deg` degrees of `dir`, at random.
fn cone(dir: Vec3, deg: f32, r1: f32, r2: f32) -> Vec3 {
    let d = dir.normalize_or(Vec3::Y);
    let a = d.any_orthonormal_vector();
    let b = d.cross(a);
    let theta = deg.to_radians() * r1.sqrt();
    let phi = r2 * std::f32::consts::TAU;
    (d * theta.cos() + (a * phi.cos() + b * phi.sin()) * theta.sin()).normalize_or(d)
}

impl Gore {
    fn rand(&mut self) -> f32 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.rand()
    }

    /// The tables and events the game wrote, once a frame.
    #[cfg(windows)]
    pub fn read(&mut self, link: &rustvr_protocol::link::Link) -> Vec<GoreEvent> {
        if link.read_bloods_into(&mut self.bloods) {
            let n = (self.bloods.count as usize).min(MAX_BLOODS);
            let looks: Vec<GoreBlood> = self.bloods.bloods[..n]
                .iter()
                .map(|b| GoreBlood {
                    splat: Blood::texture(&b.splat).to_owned(),
                    splatter: Blood::texture(&b.splatter).to_owned(),
                    stain: Blood::texture(&b.stain).to_owned(),
                    drip: Blood::texture(&b.drip).to_owned(),
                    drop: Blood::texture(&b.drop).to_owned(),
                    mist: Blood::texture(&b.mist).to_owned(),
                    flesh: Blood::texture(&b.flesh).to_owned(),
                    atlas: b.atlas.max(1),
                    glow: b.flags & BLOOD_GLOW != 0,
                    dust: b.flags & BLOOD_DUST != 0,
                })
                .collect();
            if looks != self.looks {
                tracing::info!("gore: the game's blood: {}", looks.iter().map(|l| format!("{} ({})", l.splat, if l.dust { "dust" } else { "liquid" })).collect::<Vec<_>>().join(", "));
                self.looks = looks;
            }
        }
        link.read_gore_bodies_into(&mut self.bodies);
        link.read_gore_events(&mut self.events_read)
    }

    fn blood(&self, i: u32) -> Option<&Blood> {
        self.bloods.bloods[..(self.bloods.count as usize).min(MAX_BLOODS)].get(i as usize)
    }

    fn liquid(&self, i: u32) -> bool {
        self.blood(i).is_some_and(|b| b.flags & BLOOD_DUST == 0)
    }

    fn body(&self, actor: u32) -> Option<&GoreBody> {
        self.bodies.bodies[..(self.bodies.count as usize).min(MAX_GORE_BODIES)].iter().find(|b| b.id == actor)
    }

    /// Drops thrown out from `at` along `dir` within `spread` degrees, `speed` m/s (a range), `size`
    /// m (a range); and `mist` fine drops with them.
    #[allow(clippy::too_many_arguments)]
    fn spray(&mut self, at: Vec3, dir: Vec3, spread: f32, speed: (f32, f32), size: (f32, f32), count: usize, mist: usize, blood: u32, base_vel: Vec3) {
        let dust = !self.liquid(blood);
        for k in 0..count + mist {
            let is_mist = k >= count || dust;
            let (r1, r2) = (self.rand(), self.rand());
            let d = cone(dir, spread * if is_mist { 1.4 } else { 1.0 }, r1, r2);
            let s = self.range(speed.0, speed.1) * if is_mist { 0.8 } else { 1.0 };
            let sz = if is_mist { self.range(0.0005, 0.0012) } else { self.range(size.0, size.1) };
            let life = if is_mist { self.range(0.2, 0.55) } else { 6.0 };
            self.push_drop(Drop { pos: at, vel: base_vel + d * s, size: sz, blood, mist: is_mist, life, age: 0.0, probed: at });
        }
    }

    fn push_drop(&mut self, d: Drop) {
        if !d.pos.is_finite() || !d.vel.is_finite() {
            return;
        }
        if self.drops.len() >= MAX_DROPS {
            // The oldest goes (mist first: it's going anyway).
            let i = self.drops.iter().position(|x| x.mist).unwrap_or(0);
            self.drops.swap_remove(i);
        }
        self.drops.push(d);
    }

    /// One frame. `rig` gives the world's collision and takes the severed pieces' pushes.
    pub fn update(&mut self, rig: &mut Rig, inp: &GoreInput, events: &[GoreEvent]) {
        let now = inp.now;
        let dt = inp.dt.clamp(0.0, 0.05);
        self.dt = dt;
        self.pulses.clear();
        for ev in events {
            if ev.kind == GE_SEVER {
                self.sever(ev, now);
            }
        }
        for hit in inp.hits.iter().filter(|e| e.kind == CE_HIT) {
            self.wound(rig, inp, hit);
        }
        if inp.paused {
            self.build(rig, inp);
            return;
        }
        // The pieces: pushed off once the rig has them loose.
        for p in &mut self.pieces {
            if let Some(dv) = p.kick
                && rig.kick(p.object, dv)
            {
                tracing::info!("gore: the piece {:#x} leaves the body at {:.2} m/s", p.object, dv.length());
                p.kick = None;
            }
        }
        self.pieces.retain(|p| now - p.born < 600.0 && (p.kick.is_some() || rig.object_world_pose(p.object).is_some()));
        self.stumps_bleed(rig, now);
        self.wounds_bleed(inp, now);
        self.blades_drip(rig, inp, now, dt);
        self.fly(rig, inp, dt, now);
        self.run(rig, dt, now);
        self.dry(dt);
        self.build(rig, inp);
        if now - self.logged > 5.0 && (self.stats.drops > 0 || !self.decals.is_empty()) {
            self.logged = now;
            let s = &self.stats;
            tracing::info!("gore: {} drops, {} decals ({} pools), {} runs, {} probes this frame, {} wounds and {} limbs so far, {} splats on the player", s.drops, s.decals, s.pools, s.runners, s.probes, s.wounds, s.severs, self.out_splats.len());
        }
    }

    fn sever(&mut self, ev: &GoreEvent, now: f64) {
        self.stats.severs += 1;
        let at = Vec3::from_array(ev.at);
        let out = Vec3::from_array(ev.out).normalize_or(Vec3::Y);
        let amount = self.blood(ev.blood).map_or(1.0, |b| b.amount.max(0.1));
        tracing::info!("gore: {:#x} lost limb {:#x} at {at:.2?}, the piece {:#x} (blood {})", ev.actor, ev.limb, ev.object, ev.blood);
        self.stumps.insert((ev.actor, ev.limb), StumpTime { born: now, next_beat: now + 0.15, seen: now, spurt: None });
        // The blow's hand feels it go through.
        if let Some(&(hand, at)) = self.last_hit.get(&ev.actor)
            && now - at < 0.5
        {
            self.pulses.push(crate::combat::Pulse { hand, seconds: 0.16, frequency: 55.0, amplitude: 1.0 });
        }
        // The cut's gush: out of the stump, and from the piece's side back the other way.
        let vel = Vec3::from_array(ev.vel);
        let n = (90.0 * amount) as usize;
        self.spray(at, out, 28.0, (1.5, 4.5), (0.0015, 0.0045), n, n, ev.blood, vel * 0.3);
        self.spray(at, -out, 35.0, (0.8, 2.5), (0.0015, 0.004), n / 3, n / 3, ev.blood, vel * 0.3);
        if ev.object != 0 {
            // (Half the part's speed, and pushed off the cut; the blade's way
            // carries it on, a head a little up.)
            let push = Vec3::from_array(ev.push).normalize_or_zero() * ev.push_speed.clamp(0.0, 8.0) * 0.18;
            let lift = if ev.limb & 0xFF == LIMB_HEAD { Vec3::Y * 0.6 } else { Vec3::ZERO };
            let kick = (vel * 0.5 + out * 1.2 + push + lift).clamp_length_max(3.5);
            self.pieces.push(Piece {
                object: ev.object,
                cap: Vec3::from_array(ev.cap),
                cap_out: Vec3::from_array(ev.cap_out).normalize_or(Vec3::NEG_Y),
                radius: ev.radius,
                blood: ev.blood,
                born: now,
                kick: Some(kick),
                next_beat: now + 0.3,
            });
        }
    }

    /// A hit: the spray from it, the wound it leaves bleeding, and the blood on the blade.
    fn wound(&mut self, rig: &Rig, inp: &GoreInput, ev: &CombatEvent) {
        self.last_hit.retain(|_, (_, t)| inp.now - *t < 2.0);
        self.last_hit.insert(ev.actor, ((ev.hand as usize).min(1), inp.now));
        let Some(body) = self.body(ev.actor).copied() else { return };
        let blood = body.blood;
        if blood == u32::MAX || self.blood(blood).is_none() {
            return;
        }
        self.stats.wounds += 1;
        let point = Vec3::from_array(ev.point);
        let way = Vec3::from_array(ev.dir).normalize_or(Vec3::Y);
        // Out of the body there: from the hit part's axis.
        let part = inp.combat.actors().iter().find(|a| a.id == ev.actor).and_then(|a| a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)].iter().position(|p| p.game_part == ev.part).map(|i| (a, i)));
        let out = part
            .map(|(a, i)| {
                let p = &a.parts[i];
                let (pa, pb) = (Vec3::from_array(p.a), Vec3::from_array(p.b));
                let t = ((point - pa).dot(pb - pa) / (pb - pa).length_squared().max(1e-6)).clamp(0.0, 1.0);
                (point - pa.lerp(pb, t)).normalize_or(-way)
            })
            .unwrap_or(-way);
        // Armour bleeds less through it.
        let through = match ev.material {
            MAT_FLESH => 1.0,
            MAT_LEATHER => 0.8,
            MAT_CHAINMAIL => 0.55,
            MAT_PLATE => 0.4,
            _ => 0.6,
        };
        let amount = self.blood(blood).map_or(1.0, |b| b.amount.max(0.1));
        let s = ev.strength.clamp(0.05, 1.5);
        let k = s * through * amount;
        let glancing = ev.flags & CEF_GLANCING != 0;
        match ev.what {
            DMG_SLASH if !glancing => {
                // A fan along the blade's way and out of the cut, and the cut's lip back at the blade.
                let dir = (out * 0.65 + way * 0.75).normalize_or(out);
                let n = (35.0 + 90.0 * k) as usize;
                self.spray(point, dir, 30.0, (1.2, 2.0 + 3.5 * s), (0.0012, 0.0035), n, n, blood, Vec3::ZERO);
                self.spray(point, (out - way * 0.3).normalize_or(out), 45.0, (0.5, 1.6), (0.001, 0.003), n / 3, n / 4, blood, Vec3::ZERO);
            }
            DMG_PIERCE if !glancing => {
                let n = (25.0 + 60.0 * k) as usize;
                self.spray(point, (out * 0.7 - way * 0.5).normalize_or(out), 16.0, (1.0, 1.5 + 2.5 * s), (0.0015, 0.004), n, n / 2, blood, Vec3::ZERO);
            }
            _ => {
                let n = (8.0 + 30.0 * k) as usize;
                self.spray(point, out, 55.0, (0.6, 1.0 + 1.5 * s), (0.001, 0.003), n, n, blood, Vec3::ZERO);
            }
        }
        // The wound bleeds on where it is on the body part.
        if let Some((a, i)) = part
            && self.liquid(blood)
            && !glancing
        {
            let p = &a.parts[i];
            let (fp, fq) = crate::rig::embed::part_frame(Vec3::from_array(p.a), Vec3::from_array(p.b));
            let inv = fq.inverse();
            self.wounds.retain(|w| inp.now < w.until);
            if self.wounds.len() >= 48 {
                self.wounds.remove(0);
            }
            self.wounds.push(Wound { actor: ev.actor, part: i, at: inv * (point - fp), out: inv * out, blood, born: inp.now, until: inp.now + 3.0 + 6.0 * k as f64, rate: 20.0 + 50.0 * k });
        }
        // On the blade, as far as it went in (a stab its point, a cut along its edge); a fist's
        // knuckles.
        if glancing || !self.liquid(blood) {
            return;
        }
        self.splat_blood = blood;
        if ev.object == 0 {
            let h = (ev.hand as usize).min(1);
            self.splat_body(inp, if h == 0 { Joint::LeftHand as usize } else { Joint::RightHand as usize }, inp.map.inv_point(point), 0.035, out, inp.now);
            return;
        }
        // (In the weapon's own space as the combat judged the blow: where the hand swung it, which a
        // fast swing's body lags behind.)
        let Some((wp, wq)) = rig.swing_pose(ev.object) else { return };
        let inv = wq.inverse();
        let depth = part.map_or(0.08, |(a, i)| a.parts[i].radius * 1.4);
        let blades = inp.combat.blades(ev.object);
        let blade = blades.iter().min_by(|x, y| {
            let d = |b: &(Vec3, Vec3, u32)| {
                let (s, _) = crate::combat::closest_segments(b.0, b.1, point, point);
                b.0.lerp(b.1, s).distance(point)
            };
            d(x).total_cmp(&d(y))
        });
        let list = self.held_splats.entry(ev.object).or_default();
        let mut add = |at: Vec3, r: f32| {
            if list.len() >= MAX_HELD_SPLATS
                && let Some(i) = list.iter().enumerate().min_by(|a, b| a.1.radius.total_cmp(&b.1.radius)).map(|(i, _)| i)
            {
                list.remove(i);
            }
            list.push(Splat { at: inv * (at - wp), normal: inv * out, radius: r, wet: 1.0, born: inp.now });
        };
        match blade {
            Some(&(a, b, kind)) => {
                let (s, _) = crate::combat::closest_segments(a, b, point, point);
                let len = a.distance(b).max(1e-3);
                let on = a.lerp(b, s);
                if kind == DMG_PIERCE {
                    // From the point back as deep as it went.
                    let back = (a - b).normalize_or_zero();
                    let n = ((depth / 0.025) as usize).clamp(1, 6);
                    for j in 0..n {
                        add(b + back * (j as f32 * 0.025), 0.03);
                    }
                } else {
                    // Along the edge as far as the body it went through, either side of where it
                    // met (a blade is bloodied as far as it went in).
                    let along = (b - a) / len;
                    let n = ((depth * 2.0 / 0.025) as usize).clamp(3, 10);
                    for j in 0..n {
                        let off = (j as f32 - (n as f32 - 1.0) * 0.5) * 0.025;
                        add(on + along * off, 0.03 + 0.012 * s.min(1.0));
                    }
                }
            }
            None => add(point, 0.03),
        }
    }

    fn splat_body(&mut self, inp: &GoreInput, joint: usize, at_tracking: Vec3, radius: f32, normal_world: Vec3, now: f64) {
        let j = &inp.joints[joint];
        if j.valid == 0 {
            return;
        }
        let (jp, jq) = (Vec3::from_array(j.pos), Quat::from_array(j.rot).normalize());
        let inv = jq.inverse();
        let n = inp.map.inv_rot(Quat::IDENTITY) * normal_world;
        if self.body_splats.len() >= MAX_BODY_SPLATS {
            self.body_splats.remove(0);
        }
        self.body_splats.push((joint, Splat { at: inv * (at_tracking - jp), normal: inv * n, radius, wet: 1.0, born: now }));
    }

    fn stumps_bleed(&mut self, rig: &Rig, now: f64) {
        let bodies: Vec<GoreBody> = self.bodies.bodies[..(self.bodies.count as usize).min(MAX_GORE_BODIES)].to_vec();
        for b in &bodies {
            if b.blood == u32::MAX || !self.liquid(b.blood) {
                continue;
            }
            let amount = self.blood(b.blood).map_or(1.0, |x| x.amount.max(0.1));
            for s in &b.stumps[..(b.count as usize).min(MAX_STUMPS)] {
                let key = (b.id, s.limb);
                let st = *self.stumps.entry(key).or_insert(StumpTime { born: now, next_beat: now, seen: now, spurt: None });
                let t = now - st.born;
                let at = Vec3::from_array(s.at);
                let out = Vec3::from_array(s.out).normalize_or(Vec3::Y);
                let k = (-t / STUMP_FADE).exp() as f32;
                let width = (s.radius / 0.05).clamp(0.5, 2.0);
                let mut next = st.next_beat;
                let mut spurt = st.spurt;
                if t < STUMP_PUMPS && now >= next {
                    // A pulse: the arteries' spurt, a jet out of the stump.
                    next = now + BEAT * self.range(0.92, 1.08) as f64;
                    let n = ((50.0 + 90.0 * width) * k * amount) as f32 + 8.0;
                    spurt = Some((now, cone(out, 8.0, self.rand(), self.rand()), n / SPURT as f32));
                }
                if let Some((from, dir, rate)) = spurt {
                    // Through the spurt the pressure rises and falls: the drops leave faster then
                    // slower, and land in a line.
                    let f = ((now - from) / SPURT) as f32;
                    if f >= 1.0 {
                        spurt = None;
                    } else {
                        let pressure = (f * std::f32::consts::PI).sin();
                        let n = self.count(rate * (0.4 + 0.6 * pressure));
                        let speed = (0.7 + 3.4 * k) * (0.35 + 0.65 * pressure);
                        let wobble = cone(dir, 4.0, self.rand(), self.rand());
                        self.spray(at, wobble, 5.0 + 4.0 * width, (speed * 0.85, speed * 1.1), (0.0025, 0.006 * width.sqrt()), n, n / 2, b.blood, Vec3::ZERO);
                    }
                }
                if spurt.is_none() && t < STUMP_FLOWS {
                    // The flow between and after: drops welling over the cut's edge.
                    let rate = if t < STUMP_PUMPS { 25.0 * k + 10.0 } else { 14.0 * (1.0 - (t - STUMP_PUMPS) / (STUMP_FLOWS - STUMP_PUMPS)) as f32 + 1.5 };
                    let n = self.count(rate * width);
                    for _ in 0..n {
                        let off = cone(out, 80.0, self.rand(), self.rand()) * s.radius * 0.8;
                        self.spray(at + off, out, 50.0, (0.05, 0.35), (0.0025, 0.005), 1, 0, b.blood, Vec3::ZERO);
                    }
                } else if t < STUMP_DRIPS {
                    let n = self.count(0.8);
                    for _ in 0..n {
                        self.spray(at, out, 60.0, (0.0, 0.1), (0.002, 0.0035), 1, 0, b.blood, Vec3::ZERO);
                    }
                }
                if let Some(e) = self.stumps.get_mut(&key) {
                    e.next_beat = next;
                    e.seen = now;
                    e.spurt = spurt;
                }
            }
        }
        // Stumps the game no longer has (the body gone) are forgotten after a while.
        self.stumps.retain(|_, s| now - s.seen < 30.0);
        // The pieces' own cuts.
        let pieces = self.pieces.clone();
        for (i, p) in pieces.iter().enumerate() {
            let Some((pp, pq)) = rig.object_world_pose(p.object) else { continue };
            if !self.liquid(p.blood) {
                continue;
            }
            let t = now - p.born;
            let at = pp + pq * p.cap;
            let out = (pq * p.cap_out).normalize_or(Vec3::NEG_Y);
            let k = (-t / PIECE_FADE).exp() as f32;
            if t < PIECE_PUMPS && now >= p.next_beat {
                self.pieces[i].next_beat = now + BEAT;
                let n = (25.0 * k) as usize + 3;
                self.spray(at, out, 18.0, (0.6 + 1.5 * k, 1.0 + 2.5 * k), (0.0015, 0.0035), n, n / 2, p.blood, Vec3::ZERO);
            } else if t < PIECE_DRIPS {
                let n = self.count(if t < PIECE_PUMPS { 12.0 } else { 3.0 * (1.0 - (t / PIECE_DRIPS) as f32) + 0.3 });
                for _ in 0..n {
                    let off = cone(out, 80.0, self.rand(), self.rand()) * p.radius * 0.7;
                    self.spray(at + off, out, 60.0, (0.02, 0.25), (0.0015, 0.003), 1, 0, p.blood, Vec3::ZERO);
                }
            }
        }
    }

    /// How many of something that comes `rate` a second this frame (at random, on average right).
    fn count(&mut self, rate: f32) -> usize {
        let x = rate.max(0.0) * self.dt;
        let whole = x.floor();
        whole as usize + (self.rand() < x - whole) as usize
    }

    fn wounds_bleed(&mut self, inp: &GoreInput, now: f64) {
        let wounds = self.wounds.clone();
        for w in &wounds {
            if now >= w.until {
                continue;
            }
            let Some(a) = inp.combat.actors().iter().find(|a| a.id == w.actor) else { continue };
            let Some(p) = a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)].get(w.part) else { continue };
            let (fp, fq) = crate::rig::embed::part_frame(Vec3::from_array(p.a), Vec3::from_array(p.b));
            let at = fp + fq * w.at;
            let out = fq * w.out;
            let k = 1.0 - ((now - w.born) / (w.until - w.born)) as f32;
            let n = self.count(w.rate * k);
            for _ in 0..n {
                self.spray(at, out, 40.0, (0.1, 0.6 * k + 0.1), (0.0012, 0.003), 1, 0, w.blood, Vec3::ZERO);
            }
        }
        self.wounds.retain(|w| now < w.until);
    }

    /// Wet blades drip from their lowest bloody point.
    fn blades_drip(&mut self, rig: &Rig, inp: &GoreInput, now: f64, dt: f32) {
        let ids: Vec<u32> = self.held_splats.keys().copied().collect();
        for id in ids {
            let Some((wp, wq)) = rig.object_world_pose(id) else {
                self.held_splats.remove(&id);
                continue;
            };
            let list = self.held_splats.get_mut(&id).unwrap();
            for s in list.iter_mut() {
                s.wet = (s.wet - dt / HELD_DRY).max(0.0);
            }
            list.retain(|s| now - s.born < 1200.0);
            let Some(low) = list.iter().filter(|s| s.wet > 0.35).map(|s| (wp + wq * s.at, s.wet)).min_by(|a, b| a.0.y.total_cmp(&b.0.y)) else { continue };
            let next = *self.held_drip.entry(id).or_insert(now);
            if now >= next {
                let gap = 0.35 + (1.0 - low.1) as f64 * 2.5;
                let jitter = self.range(0.6, 1.4) as f64;
                self.held_drip.insert(id, now + gap * jitter);
                let blood = self.splat_blood;
                let size = self.range(0.0018, 0.0032);
                self.push_drop(Drop { pos: low.0 - Vec3::Y * 0.005, vel: Vec3::ZERO, size, blood, mist: false, life: 6.0, age: 0.0, probed: low.0 });
            }
        }
        // The body's splats dry too.
        for (_, s) in &mut self.body_splats {
            s.wet = (s.wet - dt / HELD_DRY).max(0.0);
        }
        self.body_splats.retain(|(_, s)| now - s.born < 1200.0);
        let _ = inp;
    }

    /// The drops fly: gravity, drag, and what they meet.
    fn fly(&mut self, rig: &Rig, inp: &GoreInput, dt: f32, now: f64) {
        // The player's hands (world) and held blades, for the drops that meet them.
        let hands: Vec<(usize, Vec3)> = (0..2)
            .filter(|&h| inp.out.hands[h].flags & HAND_TRACKED != 0)
            .map(|h| {
                let hs = &inp.out.hands[h];
                (h, inp.map.point(Vec3::from_array(hs.pose.pos) + Quat::from_array(hs.pose.rot) * Vec3::Y * 0.05))
            })
            .collect();
        let held: Vec<(u32, Vec3, Vec3)> = (0..2).filter_map(|h| rig.held(h)).flat_map(|id| inp.combat.blades(id).into_iter().map(move |(a, b, _)| (id, a, b))).collect();
        let mut landed = Vec::new();
        let mut on_hand = Vec::new();
        let mut on_blade = Vec::new();
        for (i, d) in self.drops.iter_mut().enumerate() {
            d.age += dt;
            // Air drag: fine drops slow quickly.
            let drag = if d.mist { 3.0 } else { 0.25 + 0.0008 / d.size.max(1e-4) * 0.1 };
            d.vel.y -= GRAVITY * dt;
            d.vel *= (1.0 - drag * dt).max(0.0);
            d.pos += d.vel * dt;
            if d.mist {
                continue;
            }
            if let Some(&(h, _)) = hands.iter().find(|(_, p)| p.distance(d.pos) < HAND_RADIUS) {
                on_hand.push((i, h));
                continue;
            }
            if let Some(&(id, ..)) = held.iter().find(|(_, a, b)| {
                let (s, _) = crate::combat::closest_segments(*a, *b, d.pos, d.pos);
                a.lerp(*b, s).distance(d.pos) < 0.02
            }) {
                on_blade.push((i, id));
            }
        }
        // Probes against the world, in turn.
        let n = self.drops.len();
        let mut probes = 0;
        if n > 0 {
            let mut k = 0;
            while probes < PROBES && k < n {
                let i = (self.next_probe + k) % n;
                k += 1;
                let d = self.drops[i];
                if d.mist {
                    continue;
                }
                let way = d.pos - d.probed;
                let len = way.length();
                if len < 1e-4 {
                    continue;
                }
                probes += 1;
                let dir = way / len;
                match rig.world_hit(d.probed, dir, len) {
                    Some(hit) => landed.push((i, d.probed + dir * hit.distance, hit.normal.normalize_or(Vec3::Y))),
                    None => self.drops[i].probed = d.pos,
                }
            }
            self.next_probe = (self.next_probe + k) % n.max(1);
        }
        self.stats.probes = probes;
        let mut gone = vec![false; n];
        for &(i, h) in &on_hand {
            let d = self.drops[i];
            gone[i] = true;
            if self.liquid(d.blood) {
                self.splat_blood = d.blood;
                let hs = &inp.out.hands[h];
                let at = inp.map.inv_point(d.pos);
                let _ = hs;
                self.splat_body(inp, if h == 0 { Joint::LeftHand as usize } else { Joint::RightHand as usize }, at, (d.size * 6.0).clamp(0.01, 0.03), -d.vel.normalize_or(Vec3::Y), now);
            }
        }
        for &(i, id) in &on_blade {
            if gone[i] {
                continue;
            }
            let d = self.drops[i];
            gone[i] = true;
            if let (true, Some((wp, wq))) = (self.liquid(d.blood), rig.swing_pose(id)) {
                let inv = wq.inverse();
                let at = inv * (d.pos - wp);
                let r = (d.size * 5.0).clamp(0.008, 0.02);
                let list = self.held_splats.entry(id).or_default();
                // On blood already there it adds to it; else a splat of its own (the smallest goes
                // for it when the blade's full).
                if let Some(s) = list.iter_mut().find(|s| s.at.distance(at) < s.radius) {
                    s.radius = (s.radius * s.radius + 0.3 * r * r).sqrt().min(0.05);
                    s.wet = 1.0;
                } else {
                    if list.len() >= MAX_HELD_SPLATS
                        && let Some(i) = list.iter().enumerate().min_by(|a, b| a.1.radius.total_cmp(&b.1.radius)).map(|(i, _)| i)
                    {
                        list.remove(i);
                    }
                    list.push(Splat { at, normal: inv * -d.vel.normalize_or(Vec3::Y), radius: r, wet: 1.0, born: now });
                }
            }
        }
        for &(i, at, normal) in &landed {
            if gone[i] {
                continue;
            }
            gone[i] = true;
            let d = self.drops[i];
            self.land(at, normal, d.vel, d.size, d.blood, now);
        }
        // Old or far fallen ones go.
        let mut i = 0;
        let mut j = 0;
        self.drops.retain(|d| {
            let keep = !gone[i] && d.age < d.life && d.pos.y > -2000.0;
            i += 1;
            if keep {
                j += 1;
            }
            keep
        });
        let _ = j;
        if self.next_probe >= self.drops.len() {
            self.next_probe = 0;
        }
    }

    /// A drop meets the world at `at` (`normal` out of it): a splat, a pool, a run.
    fn land(&mut self, at: Vec3, normal: Vec3, vel: Vec3, size: f32, blood: u32, now: f64) {
        let speed = vel.length();
        let volume = size * size * size;
        let dust = !self.liquid(blood);
        let floor = normal.y > 0.75;
        // Into a pool it lands in.
        if floor && !dust
            && let Some(p) = self.decals.iter_mut().rev().take(300).find(|p| p.tex == DECAL_STAIN && p.blood == blood && p.center.distance(at) < p.hu.max(0.05) * 1.1 && p.normal.dot(normal) > 0.8)
        {
            p.volume += volume;
            p.grow_to = pool_radius(p.volume);
            p.wet = 1.0;
            return;
        }
        // (A drop spreads as it hits: a few times its size, more the faster it lands.)
        let r = (size * (8.0 + 1.2 * speed)).clamp(0.012, 0.14) * if dust { 1.5 } else { 1.0 };
        // Close to a splat already there: it wets that one and adds to it.
        let roll = self.rand();
        if let Some(s) = self.decals.iter_mut().rev().take(250).find(|s| matches!(s.tex, DECAL_SPLAT | DECAL_SPLATTER) && s.blood == blood && s.center.distance(at) < (s.hv + r) * 0.6 && s.normal.dot(normal) > 0.8) {
            s.wet = 1.0;
            let grow = |x: f32| (x * x + 0.25 * r * r).sqrt().min(0.16);
            s.hu = grow(s.hu);
            s.hv = grow(s.hv);
            s.volume += volume;
            if floor && !dust && s.volume > 7e-8 && roll < 0.5 {
                // Enough lands here: it's a pool now.
                let v = s.volume;
                let c = s.center;
                self.add_decal(Decal { center: c, normal, u: normal.any_orthonormal_vector(), hu: 0.03, hv: 0.03, tex: DECAL_STAIN, tile: 0, blood, born: now, wet: 1.0, volume: v, grow_to: pool_radius(v) });
            }
            return;
        }
        // A fast slanting hit splatters along its way (the splatter's streaks run along +u from
        // where it struck); anything else is one of the splat atlas's splats, turned at random.
        let tangential = vel - normal * vel.dot(normal);
        let ts = tangential.length();
        let tiles = self.looks.get(blood as usize).map_or(1, |l| l.atlas.max(1) * l.atlas.max(1));
        let tile = (self.rand() * tiles as f32) as u32 % tiles;
        if ts > 2.0 && speed > 2.5 && !dust {
            let u = tangential / ts;
            let hu = (r * (1.2 + 0.35 * ts)).min(0.25);
            self.add_decal(Decal { center: at + u * hu * 0.6, normal, u, hu, hv: hu * 0.45, tex: DECAL_SPLATTER, tile: 0, blood, born: now, wet: 1.0, volume, grow_to: 0.0 });
        } else {
            let a = normal.any_orthonormal_vector();
            let ang = self.rand() * std::f32::consts::TAU;
            let u = Quat::from_axis_angle(normal, ang) * a;
            self.add_decal(Decal { center: at, normal, u, hu: r, hv: r, tex: DECAL_SPLAT, tile, blood, born: now, wet: 1.0, volume, grow_to: 0.0 });
        }
        // Down a wall or a slope, a big enough drop runs.
        if !dust && normal.y < 0.6 && normal.y > -0.5 && size > 0.0022 && self.runners.len() < MAX_RUNNERS && self.rand() < 0.45 {
            let width = (size * 2.2).clamp(0.003, 0.009);
            let decal = self.add_decal(Decal { center: at, normal, u: Vec3::NEG_Y, hu: width, hv: width, tex: DECAL_DRIP, tile: 0, blood, born: now, wet: 1.0, volume: 0.0, grow_to: 0.0 });
            self.runners.push(Runner { pos: at, normal, start: at, speed: 0.0, width, volume: volume * 40.0, blood, decal, born: now, probe_in: 3 });
        }
    }

    fn add_decal(&mut self, d: Decal) -> usize {
        if self.decals.len() < MAX_DECALS {
            self.decals.push(d);
            return self.decals.len() - 1;
        }
        // Full: the oldest splat goes (a pool only when there's nothing else); a decal a run is
        // still drawing stays.
        let busy: Vec<usize> = self.runners.iter().map(|r| r.decal).collect();
        let oldest = |want_pool: bool, list: &[Decal]| list.iter().enumerate().filter(|(i, x)| (x.tex == DECAL_STAIN) == want_pool && !busy.contains(i)).min_by(|a, b| a.1.born.total_cmp(&b.1.born)).map(|(i, _)| i);
        let i = oldest(false, &self.decals).or_else(|| oldest(true, &self.decals)).unwrap_or(0);
        self.decals[i] = d;
        i
    }

    /// Runs slide down what they're on.
    fn run(&mut self, rig: &Rig, dt: f32, now: f64) {
        let mut fell = Vec::new();
        let viscosity = |g: &Gore, b: u32| g.blood(b).map_or(1.0, |x| x.viscosity.max(0.2));
        for i in 0..self.runners.len() {
            let mut r = self.runners[i];
            let g = Vec3::NEG_Y;
            let down = g - r.normal * g.dot(r.normal);
            let steep = down.length();
            if steep < 0.25 || r.volume <= 0.0 || now - r.born > 9.0 {
                r.volume = 0.0;
                self.runners[i] = r;
                continue;
            }
            let dir = down / steep;
            // Blood creeps down a wall at a few centimetres a second, faster on the steep and while
            // there's a lot of it.
            let terminal = 0.09 * steep * (r.volume / 2e-7).clamp(0.2, 1.6) / viscosity(self, r.blood);
            r.speed += (terminal - r.speed) * (dt * 3.0).min(1.0);
            let step = dir * r.speed * dt;
            r.pos += step;
            r.volume -= step.length() * 4e-7;
            r.probe_in = r.probe_in.saturating_sub(1);
            if r.probe_in == 0 {
                r.probe_in = 4;
                // Still on it? Follow its curve; off its edge, it falls.
                match rig.world_hit(r.pos + r.normal * 0.02, -r.normal, 0.06) {
                    Some(hit) => {
                        r.pos = r.pos + r.normal * 0.02 - r.normal * hit.distance;
                        let n = hit.normal.normalize_or(r.normal);
                        if n.dot(r.normal) < 0.85 {
                            // It bends: a new streak from here.
                            r.start = r.pos;
                            r.decal = self.add_decal(Decal { center: r.pos, normal: n, u: dir, hu: r.width, hv: r.width, tex: DECAL_DRIP, tile: 0, blood: r.blood, born: now, wet: 1.0, volume: 0.0, grow_to: 0.0 });
                        }
                        r.normal = n;
                    }
                    None => {
                        fell.push((r.pos + dir * 0.01, dir * r.speed, r.blood));
                        r.volume = 0.0;
                    }
                }
            }
            // Its streak: from where it started to where it is.
            if let Some(d) = self.decals.get_mut(r.decal).filter(|d| d.tex == DECAL_DRIP) {
                let span = r.pos - r.start;
                let len = span.length();
                d.center = (r.start + r.pos) * 0.5;
                d.normal = r.normal;
                d.u = if len > 1e-4 { span / len } else { dir };
                d.hu = len * 0.5 + r.width;
                d.hv = r.width;
                d.wet = 1.0;
            }
            self.runners[i] = r;
        }
        self.runners.retain(|r| r.volume > 0.0);
        for (at, vel, blood) in fell {
            let size = self.range(0.002, 0.0035);
            self.push_drop(Drop { pos: at, vel, size, blood, mist: false, life: 6.0, age: 0.0, probed: at });
        }
    }

    fn dry(&mut self, dt: f32) {
        for d in &mut self.decals {
            let dry = if d.tex == DECAL_STAIN { POOL_DRY } else { DRY };
            d.wet = (d.wet - dt / dry).max(0.0);
            if d.tex == DECAL_STAIN && d.hu < d.grow_to {
                // A pool spreads to what's in it.
                let r = d.hu + (d.grow_to - d.hu) * (dt * 0.6).min(1.0);
                d.hu = r;
                d.hv = r;
            }
        }
    }

    /// This frame's draw lists.
    fn build(&mut self, rig: &Rig, inp: &GoreInput) {
        self.out_drops.clear();
        for d in &self.drops {
            let alpha = if d.mist { (1.0 - d.age / d.life).clamp(0.0, 1.0) * 0.7 } else { 1.0 };
            self.out_drops.push(GoreDrop { pos: d.pos.to_array(), size: d.size, vel: d.vel.to_array(), alpha, blood: d.blood, mist: d.mist as u32 as f32, _p: [0.0; 2] });
        }
        self.out_decals.clear();
        for d in &self.decals {
            let v = d.normal.cross(d.u).normalize_or_zero();
            self.out_decals.push(GoreDecal {
                center: d.center.to_array(),
                tile: d.tile,
                u: (d.u * d.hu).to_array(),
                wet: d.wet,
                v: (v * d.hv).to_array(),
                tex: d.tex,
                n: (d.normal * DECAL_DEPTH).to_array(),
                blood: d.blood,
            });
        }
        self.out_caps.clear();
        for b in &self.bodies.bodies[..(self.bodies.count as usize).min(MAX_GORE_BODIES)] {
            if b.blood == u32::MAX {
                continue;
            }
            for s in &b.stumps[..(b.count as usize).min(MAX_STUMPS)] {
                self.out_caps.push(GoreCap { center: s.at, radius: s.radius, out: s.out, blood: b.blood });
            }
        }
        for p in &self.pieces {
            if let Some((pp, pq)) = rig.object_world_pose(p.object) {
                self.out_caps.push(GoreCap { center: (pp + pq * p.cap).to_array(), radius: p.radius, out: (pq * p.cap_out).to_array(), blood: p.blood });
            }
        }
        self.out_caps.truncate(draw::MAX_CAPS);
        // The player's: the body's splats on its joints, the held things' on them (tracking).
        self.out_splats.clear();
        for (j, s) in &self.body_splats {
            let jt = &inp.joints[*j];
            if jt.valid == 0 {
                continue;
            }
            let (jp, jq) = (Vec3::from_array(jt.pos), Quat::from_array(jt.rot).normalize());
            self.out_splats.push(GoreSplat { pos: (jp + jq * s.at).to_array(), radius: s.radius, normal: (jq * s.normal).to_array(), wet: s.wet });
        }
        for (id, list) in &self.held_splats {
            let Some((wp, wq)) = rig.object_world_pose(*id) else { continue };
            for s in list {
                let p = inp.map.inv_point(wp + wq * s.at);
                let n = inp.map.inv_rot(wq) * s.normal;
                self.out_splats.push(GoreSplat { pos: p.to_array(), radius: s.radius, normal: n.to_array(), wet: s.wet });
            }
        }
        self.out_splats.truncate(draw::MAX_SPLATS);
        self.stats.drops = self.drops.len();
        self.stats.decals = self.decals.len();
        self.stats.pools = self.decals.iter().filter(|d| d.tex == DECAL_STAIN).count();
        self.stats.runners = self.runners.len();
    }

    /// What to draw (the eye's world -> tracking still to set).
    pub fn frame(&self) -> rustvr_avatar::gore::GoreFrame<'_> {
        rustvr_avatar::gore::GoreFrame {
            bloods: &self.looks,
            drops: &self.out_drops,
            decals: &self.out_decals,
            caps: &self.out_caps,
            splats: &self.out_splats,
            splat_blood: self.splat_blood,
            world_to_tracking: glam::Mat4::IDENTITY,
        }
    }

    /// Anything to draw at all.
    pub fn active(&self) -> bool {
        !self.looks.is_empty() && (!self.out_drops.is_empty() || !self.out_decals.is_empty() || !self.out_caps.is_empty() || !self.out_splats.is_empty())
    }
}

/// How far a pool of `volume` (m³) spreads: a film a millimetre and a half thick, at most 45 cm
/// across.
fn pool_radius(volume: f32) -> f32 {
    (volume / (std::f32::consts::PI * 0.0015)).sqrt().clamp(0.03, 0.45) * 3.0
}
