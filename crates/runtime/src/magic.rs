//! The hands' magic, after Skyrim VR and the mods its players swear by (what they do and why, and
//! this design: the Oblivion adapter's `docs/design/spells.md`). The game says what spells the
//! player has and what each costs and does ([`Spells`]); the runtime has the hands hold them, charge
//! them and aim them, and sends the casts back ([`CE_CAST`]) for the game to cast its own way.
//!
//! * **A spell in each hand** (Skyrim VR's own): either empty hand holds one of the player's spells,
//!   its art small in the palm (the game's, [`Casters`]), smaller as the player's magicka runs down
//!   (Magic Improvements). The art shows once the trigger has charged it, and stays [`SHOWN`]
//!   seconds after: a hand only holding a spell shows nothing (turned about, it'd flicker round the
//!   hand's edges). A hand that takes something keeps its spell and has it back when it lets
//!   go (as VRIK puts a spell back). At first the off hand holds the game's selected spell and the
//!   main hand none, so its trigger stays the game's Activate. A spell picked in the game's own
//!   interface (its magic menu, a quick key) goes into the off hand.
//! * **The spell wheel** (Spell Wheel VR, [`crate::spell_wheel`]): the main hand's B held opens a
//!   ring of the player's spells; either hand's pointer on one and its trigger, or B let go with the
//!   main hand on one, puts it in that hand.
//! * **Charge and release** (Skyrim VR; ISPVR's feel): the trigger pulled charges the hand's spell
//!   over the game's casting time. It's felt growing, silent and slow at first and then a sharp ramp
//!   (a pulse every 100 ms down to every 20 ms, as strong as the charge to the sixth), then a faint
//!   tick every 50 ms while it's held ready; let go when it's ready and it's cast, with a full burst.
//!   Let go sooner and nothing happens (no magicka spent). A quick tap with nothing charged is the
//!   game's Activate, so a hand holding a spell still opens doors and talks. A spell the game won't
//!   cast now (too little magicka, a power used today) doesn't charge: a long dull pulse (PLANCK's
//!   failure), and the game says why ([`CE_CAST_REFUSED`]). After a cast the hand waits out the rest
//!   of the game's cast before it charges again.
//! * **Aim along the arm**: a target spell leaves the palm along the arm held out, from the shoulder
//!   through the palm (the headset's body tracking's shoulder, else one set off the head), as a
//!   hand held out towards something points at it whatever its wrist does (the hand-tracking
//!   pointer Meta's own headsets use is the same line). The controller's own pointing ray (its aim
//!   pose, the laser's) rides on how the wrist is cocked: an open casting hand's is cocked up, and
//!   the spell went high. With the hand in at the shoulder there's no arm to go by, and the
//!   controller's ray takes over. Palm casting (Magic Improvements', which some Skyrim VR lists
//!   ship) is a setting: `palm_tilt` degrees from that way towards the palm's face. The aim is
//!   smoothed over the last moment, a little longer for a harder spell (40 ms for the easiest, 120
//!   ms for the hardest: enough to take out the trigger's jolt; Magic Improvements' own, up to half
//!   a second, trails behind a quick point), and helped onto a creature in a gentle cone (Spell
//!   Auto-Aim VR's missiles: 10 degrees up or down, 15 across), foes first for a harmful spell.
//! * **Touch** spells go into the first creature the charged hand touches, at once; let go in the
//!   air, the game's own touch takes what's within its reach along the aim. **Self** spells go off
//!   wherever the hand is.
//! * **Things that cast** ([`OBJF_CASTS`], a staff): the trigger charges it at its tip over its own
//!   time, and let go it's cast along it, with the same help onto a creature.

use std::collections::VecDeque;

use glam::{Mat3, Quat, Vec3};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::combat::{Pulse, closest_segments};
use crate::rig::{Rig, RigOutput, WorldMap};
use crate::spell_wheel::{Pick, Wheel, WheelInput};

/// The trigger charges past this and lets go below that.
const TRIGGER_ON: f32 = 0.5;
const TRIGGER_OFF: f32 = 0.25;
/// Let go this soon with nothing charged: a tap, the game's Activate (seconds).
const TAP: f64 = 0.25;
/// The palm: this far off the middle of the fist (the controller's grip), m; the art sits in it.
const PALM: f32 = 0.035;
/// A spell's art stays in the hand this long after the trigger last held it (seconds).
pub const SHOWN: f64 = 30.0;
/// A shoulder off the head when the headset doesn't track it (m, the right one; the head's yaw):
/// to the side, below the eyes, a little behind them (an adult's).
const SHOULDER: Vec3 = Vec3::new(0.18, -0.22, -0.08);
/// A palm nearer its shoulder than the first (m) casts where the controller points, one beyond the
/// second along the arm; between, a blend of the two.
const ARM: (f32, f32) = (0.15, 0.3);
/// A casting time when the game gives none (seconds).
const CAST_TIME: f32 = 0.5;
/// ISPVR's charge: a pulse every `PULSE_SLOW` seconds at the start down to every `PULSE_FAST` when
/// it's full, `PULSE_LENGTH` long, as strong as the charge to the sixth. Held ready, a tick every
/// `READY_TICK` (ISPVR's 0.01 is below what a Touch controller can be felt to do: this is the
/// faintest it can).
const PULSE_SLOW: f64 = 0.1;
const PULSE_FAST: f64 = 0.02;
const PULSE_LENGTH: f32 = 0.012;
const READY_TICK: f64 = 0.05;
const READY_AMPLITUDE: f32 = 0.05;
/// ISPVR's release: ten full pulses 10 ms apart.
const RELEASE: Pulse = Pulse { hand: 0, seconds: 0.1, frequency: 250.0, amplitude: 1.0 };
/// PLANCK's failure: a long dull pulse.
const FAILED: Pulse = Pulse { hand: 0, seconds: 0.3, frequency: 60.0, amplitude: 0.5 };
/// The aim's smoothing by the spell's level (Novice .. Master): the last moment's way, as seconds.
/// Weighted as Magic Improvements weights it (heavier for harder spells), but short: its own (10 to
/// 40 frames at 90 a second) trails behind a hand pointed quickly and let go.
const SMOOTHING: [f64; 5] = [0.04, 0.06, 0.08, 0.10, 0.12];
/// Spell Auto-Aim VR's missile cone (degrees up or down, across) and its reach (Skyrim's 2000
/// units, in metres).
const ASSIST_UP: f32 = 10.0;
const ASSIST_ACROSS: f32 = 15.0;
const ASSIST_RANGE: f32 = 28.0;
/// A charged hand touching a body: within this of a capsule's surface (m).
const TOUCH: f32 = 0.08;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Phase {
    /// The trigger let go.
    #[default]
    Idle,
    /// The trigger pulled at `since`: charging, then charged.
    Charging { since: f64 },
    /// The trigger still held after a cast, a tap or a failure: nothing until it's let go.
    Spent,
}

#[derive(Debug, Default)]
struct Hand {
    /// The spell it holds ([`Spell::id`]; 0: none).
    spell: u32,
    phase: Phase,
    charge: f32,
    next_pulse: f64,
    /// It charges again from then (the rest of the game's cast after a release).
    recover_until: f64,
    /// Its spell's art shows until then ([`SHOWN`] after the trigger last held it).
    shown_until: f64,
    /// Where it's aimed lately (world), for the smoothing: when, which way.
    aims: VecDeque<(f64, Vec3)>,
    /// The palm last frame (world) and how fast it moves.
    palm: Option<Vec3>,
    vel: Vec3,
    /// What the hand's casting was (`CAST_*`), since when.
    state: u32,
    since: f64,
}

/// What a hand casts with this frame.
#[derive(Clone, Copy, Debug)]
enum Source {
    Spell(Spell),
    Thing(ObjectDesc),
}

/// A spell in a palm, for a game with no casting art of its own (the runtime's glow).
#[derive(Clone, Copy, Debug)]
pub struct Glow {
    /// The palm (tracking space).
    pub at: Vec3,
    pub color: [f32; 3],
    /// 0 held, growing to 1 charged.
    pub charge: f32,
    pub state: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MagicDebug {
    pub casts: u32,
    pub failures: u32,
    pub taps: u32,
    pub picks: u32,
    pub last: Option<CombatEvent>,
}

pub struct MagicInput<'a> {
    pub rig: &'a Rig,
    pub out: &'a RigOutput,
    pub map: &'a WorldMap,
    pub frame: &'a HmdFrame,
    /// The objects the game offers (what a held thing casts).
    pub objects: &'a ObjectTable,
    /// The game's creatures (`crate::combat`'s table).
    pub actors: &'a [Actor],
    pub settings: &'a crate::settings::Magic,
    pub dominant: usize,
    pub now: f64,
    pub dt: f32,
    /// Menus, pauses, loading: nothing is cast, the wheel shuts.
    pub paused: bool,
    /// On a mount: the game casts nothing from the saddle.
    pub riding: bool,
}

pub struct Magic {
    table: Box<Spells>,
    hands: [Hand; 2],
    /// The table's count of the player's own picks, as last seen (None: not yet).
    picks_seen: Option<u32>,
    /// The off hand has been given the game's selected spell.
    started: bool,
    pub wheel: Wheel,
    /// This frame's: each hand's casting for the game ([`Casters`]), the haptics, the casts and
    /// picks for the game, the game's Activate (a tapped spell hand), the runtime's own glows.
    pub casting: [Caster; 2],
    pub pulses: Vec<Pulse>,
    pub events: Vec<CombatEvent>,
    pub activate: bool,
    pub glows: Vec<Glow>,
    pub debug: MagicDebug,
}

impl Default for Magic {
    fn default() -> Self {
        Magic {
            table: bytemuck::zeroed_box(),
            hands: Default::default(),
            picks_seen: None,
            started: false,
            wheel: Wheel::default(),
            casting: Default::default(),
            pulses: Vec::new(),
            events: Vec::new(),
            activate: false,
            glows: Vec::new(),
            debug: MagicDebug::default(),
        }
    }
}

/// The aim smoothing's window for a spell of `level` (seconds).
pub fn smoothing(level: u32) -> f64 {
    SMOOTHING[(level as usize).min(SMOOTHING.len() - 1)]
}

/// ISPVR's charge pulse at `charge` (0..1): seconds to the next, and how strong.
pub fn charge_pulse(charge: f32) -> (f64, f32) {
    let c = charge.clamp(0.0, 1.0);
    (PULSE_SLOW + (PULSE_FAST - PULSE_SLOW) * c as f64, c.powi(6))
}

/// Which way a hand's palm faces, and its fingers point (tracking space), from its pose: the
/// controller's grip, +Z forward along the fingers, +Y out of the top of the fist, the palm facing
/// +X on the left hand and -X on the right.
pub fn palm_frame(rot: Quat, hand: usize) -> (Vec3, Vec3) {
    (rot * Vec3::X * if hand == 0 { 1.0 } else { -1.0 }, rot * Vec3::Z)
}

/// Where a hand's shoulder is (tracking space): the headset's body tracking's, else set off the
/// head ([`SHOULDER`]).
pub fn shoulder(frame: &HmdFrame, hand: usize) -> Vec3 {
    if let Some(s) = frame.shoulders[hand] {
        return Vec3::from_array(s);
    }
    let ahead = Quat::from_array(frame.hmd.rot) * Vec3::Z;
    let yaw = Quat::from_rotation_y(ahead.x.atan2(ahead.z));
    let side = if hand == 0 { -SHOULDER.x } else { SHOULDER.x };
    Vec3::from_array(frame.hmd.pos) + yaw * Vec3::new(side, SHOULDER.y, SHOULDER.z)
}

/// The way a hand casts (tracking space): along its arm, from `shoulder` through `palm`; with the
/// palm in at the shoulder ([`ARM`]), where its controller points (`aim`: its aim pose, +Z where it
/// points). Then turned `palm_tilt` degrees towards the palm's face (the grip's, [`palm_frame`]).
pub fn hand_aim(shoulder: Vec3, palm: Vec3, aim: Quat, grip: Quat, hand: usize, palm_tilt: f32) -> Vec3 {
    let ray = (aim * Vec3::Z).normalize_or(Vec3::Z);
    let arm = palm - shoulder;
    let out = ((arm.length() - ARM.0) / (ARM.1 - ARM.0)).clamp(0.0, 1.0);
    let point = ray.lerp(arm.normalize_or(ray), out).normalize_or(ray);
    let (face, _) = palm_frame(grip, hand);
    let side = (face - point * face.dot(point)).normalize_or_zero();
    let a = palm_tilt.clamp(0.0, 90.0).to_radians();
    (point * a.cos() + side * a.sin()).normalize_or(point)
}

impl Magic {
    /// The player's spells, as the game wrote them (once a frame; what's unchanged stays).
    #[cfg(windows)]
    pub fn read(&mut self, link: &rustvr_protocol::link::Link) {
        link.read_spells_into(&mut self.table);
    }

    /// For tests and the range: the table as given.
    pub fn set_table(&mut self, spells: &Spells) {
        *self.table = *spells;
    }

    pub fn table(&self) -> &Spells {
        &self.table
    }

    /// The spell each hand holds (0: none).
    pub fn held(&self) -> [u32; 2] {
        [self.hands[0].spell, self.hands[1].spell]
    }

    /// Puts `spell` (0: none) in `hand`.
    pub fn hold(&mut self, hand: usize, spell: u32) {
        let h = &mut self.hands[hand];
        if h.spell != spell {
            h.spell = spell;
            h.phase = if h.phase == Phase::Idle { Phase::Idle } else { Phase::Spent };
            h.charge = 0.0;
            h.aims.clear();
        }
    }

    /// The hands whose trigger is the magic's this frame (not the game's "use"): a free hand that
    /// holds a spell, and both while the wheel is open.
    pub fn spell_hands(&self, rig: &Rig, out: &RigOutput) -> [bool; 2] {
        let open = self.wheel.is_open();
        std::array::from_fn(|h| open || (self.hands[h].spell != 0 && self.table.get(self.hands[h].spell).is_some() && rig.held(h).is_none() && !out.hand_busy[h]))
    }

    /// One frame of both hands' magic: [`Magic::casting`], [`Magic::events`], [`Magic::pulses`] and
    /// the rest are this frame's.
    pub fn update(&mut self, inp: &MagicInput) {
        self.pulses.clear();
        self.events.clear();
        self.glows.clear();
        self.activate = false;
        self.casting = Default::default();
        self.keep_hands(inp.dominant);
        // The spell wheel: what the hands pick in it goes into them (and the game is told).
        let picks = self.wheel.update(&WheelInput { frame: inp.frame, spells: &self.table, held: self.held(), dominant: inp.dominant, now: inp.now, enabled: !inp.paused && self.table.count > 0 });
        for p in picks {
            self.pick(p);
        }
        for p in self.wheel.pulses.drain(..) {
            self.pulses.push(p);
        }
        let game_art = self.table.flags & SPELLS_GAME_ART != 0;
        for h in 0..2 {
            self.hand(inp, h, game_art);
        }
    }

    /// What the wheel picked: into the hand, and (a spell) the game's selected one.
    fn pick(&mut self, p: Pick) {
        tracing::info!("magic: the {} hand takes {} from the wheel", if p.hand == 0 { "left" } else { "right" }, self.table.get(p.spell).map_or("nothing".to_owned(), |s| s.name().to_owned()));
        self.hold(p.hand, p.spell);
        // (Picked with its trigger: that pull doesn't charge it.)
        self.hands[p.hand].phase = Phase::Spent;
        self.debug.picks += 1;
        if p.spell != 0 {
            self.events.push(CombatEvent { kind: CE_SELECT, what: p.spell, hand: p.hand as u32, ..Default::default() });
        }
    }

    /// The hands' spells as the game's list changes: the off hand starts with the game's selected
    /// spell; a pick in the game's own interface goes into the off hand; a spell the player no longer
    /// has leaves the hand (the off hand falls back on the selected one).
    fn keep_hands(&mut self, dominant: usize) {
        let off = dominant ^ 1;
        if self.table.count == 0 {
            return;
        }
        let selected = self.table.selected().map(|s| s.id);
        if !self.started {
            if let Some(id) = selected {
                self.started = true;
                self.hold(off, id);
                tracing::info!("magic: the off hand starts with {}", self.table.get(id).map_or("", |s| s.name()));
            }
        }
        let picks = self.table.picks;
        if let Some(seen) = self.picks_seen
            && seen != picks
            && self.table.get(self.table.picked).is_some()
        {
            let id = self.table.picked;
            tracing::info!("magic: picked in the game's interface: {} into the off hand", self.table.get(id).map_or("", |s| s.name()));
            self.hold(off, id);
        }
        self.picks_seen = Some(picks);
        for h in 0..2 {
            let s = self.hands[h].spell;
            if s != 0 && self.table.get(s).is_none() {
                let back = if h == off { selected.unwrap_or(0) } else { 0 };
                tracing::info!("magic: the {} hand's spell {s:#x} is gone from the player's list", if h == 0 { "left" } else { "right" });
                self.hold(h, back);
            }
        }
    }

    fn hand(&mut self, inp: &MagicInput, h: usize, game_art: bool) {
        let hs = inp.out.hands[h];
        let (hp, hq) = (Vec3::from_array(hs.pose.pos), Quat::from_array(hs.pose.rot));
        // The palm (tracking: off the middle of the fist its way), then the world; how fast it goes.
        let (face, fingers) = palm_frame(hq, h);
        let palm_t = hp + face * PALM;
        let palm = inp.map.point(palm_t);
        {
            let c = &mut self.hands[h];
            if let Some(last) = c.palm {
                c.vel = (palm - last) / inp.dt.max(1e-4);
            }
            c.palm = Some(palm);
        }
        // What it casts with: a held thing that casts, else (free) the spell it holds.
        let held = inp.rig.held(h);
        let thing = held.and_then(|id| inp.objects.descs[..(inp.objects.count as usize).min(MAX_OBJECTS)].iter().find(|d| d.id == id && d.flags & OBJF_CASTS != 0 && d.casts != 0).copied());
        let free = held.is_none() && !inp.out.hand_busy[h];
        let spell = self.table.get(self.hands[h].spell).copied();
        let source = match (thing, spell) {
            (Some(d), _) => Some(Source::Thing(d)),
            (None, Some(s)) if free => Some(Source::Spell(s)),
            _ => None,
        };
        // Where it comes from (world, the way it points), and where its art goes (tracking: off the
        // palm and out of it, or the thing's tip and along it).
        let emitter = match source {
            Some(Source::Thing(d)) => held.and_then(|id| inp.rig.object_world_pose(id)).map(|(p, q)| {
                let tip = p + q * Vec3::from_array(d.muzzle);
                (tip, q * Vec3::Z, Pose { pos: inp.map.inv_point(tip).to_array(), valid: 1, rot: inp.map.inv_rot(q).to_array() })
            }),
            Some(Source::Spell(_)) => {
                // The art's +Z out of the palm, +Y along the fingers.
                let rot = Quat::from_mat3(&Mat3::from_cols(fingers.cross(face), fingers, face)).normalize();
                let pointing = Quat::from_array(inp.frame.hands[h].aim.rot);
                let aim = inp.map.rot(Quat::IDENTITY) * hand_aim(shoulder(inp.frame, h), palm_t, pointing, hq, h, inp.settings.palm_tilt);
                Some((palm, aim, Pose { pos: palm_t.to_array(), valid: 1, rot: rot.to_array() }))
            }
            None => None,
        };
        let trigger = inp.frame.hands[h].trigger;
        let wheel = self.wheel.is_open();
        let c = &mut self.hands[h];
        // (Nothing to cast, a pause, the wheel up: the charge goes; a trigger held waits to be let go.)
        let (Some(source), Some((from, along, at)), false) = (source, emitter, inp.paused) else {
            c.charge = 0.0;
            c.phase = if trigger > TRIGGER_OFF { Phase::Spent } else { Phase::Idle };
            c.aims.clear();
            c.state = CAST_NONE;
            return;
        };
        let (id, cast_time, recover, delivery, level, flags, color, name) = match source {
            Source::Spell(s) => (
                s.id,
                if s.cast_time > 0.01 { s.cast_time } else if self.table.cast_time > 0.01 { self.table.cast_time } else { CAST_TIME },
                if s.recover > 0.0 { s.recover } else { self.table.recover.max(0.0) },
                s.delivery,
                s.level,
                s.flags,
                s.color,
                s.name().to_owned(),
            ),
            Source::Thing(d) => (d.casts, if d.cast_time > 0.01 { d.cast_time } else { CAST_TIME }, self.table.recover.max(0.0), DELIVER_TARGET, 2, SPELL_CASTABLE | SPELL_HARMFUL, [0.8, 0.7, 1.0], name_str(&d.name).to_owned()),
        };
        // The aim, smoothed by the spell's level.
        c.aims.push_back((inp.now, along));
        let window = if inp.settings.aim_smoothing { smoothing(level) } else { 0.0 };
        while c.aims.len() > 1 && c.aims.front().is_some_and(|&(t, _)| inp.now - t > window) {
            c.aims.pop_front();
        }
        let smooth = c.aims.iter().fold(Vec3::ZERO, |a, &(_, d)| a + d).normalize_or(along);
        let mut cast = None;
        let blocked = wheel || inp.riding;
        if trigger < TRIGGER_OFF {
            if let Phase::Charging { since } = c.phase {
                if c.charge >= 1.0 && !blocked {
                    cast = Some(None);
                } else if inp.now - since < TAP {
                    self.activate = true;
                    self.debug.taps += 1;
                }
            }
            c.phase = Phase::Idle;
            c.charge = 0.0;
        } else if trigger > TRIGGER_ON && c.phase == Phase::Idle && !blocked && inp.now >= c.recover_until {
            if flags & SPELL_CASTABLE != 0 {
                c.phase = Phase::Charging { since: inp.now };
                c.charge = 0.0;
                c.next_pulse = inp.now;
            } else {
                // (The game says why, its own way.)
                c.phase = Phase::Spent;
                self.debug.failures += 1;
                if let Source::Spell(_) = source {
                    self.events.push(CombatEvent { kind: CE_CAST_REFUSED, what: id, hand: h as u32, ..Default::default() });
                }
                self.pulses.push(Pulse { hand: h, ..FAILED });
                tracing::info!("magic: the {} hand pulls on {name}, which the game won't cast now", if h == 0 { "left" } else { "right" });
            }
        }
        if blocked && matches!(c.phase, Phase::Charging { .. }) {
            c.phase = Phase::Spent;
            c.charge = 0.0;
        }
        // Touching a body: a charged touch spell goes into it at once.
        let mut touched = None;
        if let (Phase::Charging { .. }, DELIVER_TOUCH) = (c.phase, delivery) {
            c.charge = (c.charge + inp.dt / cast_time.max(0.01)).min(1.0);
            touched = touching(inp.actors, from);
            if c.charge >= 1.0 && touched.is_some() {
                cast = Some(touched.map(|(id, _)| id));
            }
        } else if let Phase::Charging { .. } = c.phase {
            c.charge = (c.charge + inp.dt / cast_time.max(0.01)).min(1.0);
        }
        if let Phase::Charging { .. } = c.phase {
            c.shown_until = inp.now + SHOWN;
        }
        // The charge felt (ISPVR's curve), a spike as it fills, a faint tick while it's held ready.
        if let (Phase::Charging { .. }, true, None) = (c.phase, inp.settings.charge_haptics, cast) {
            if inp.now >= c.next_pulse {
                let full = c.charge >= 1.0;
                let (gap, amplitude) = if full { (READY_TICK, READY_AMPLITUDE) } else { charge_pulse(c.charge) };
                c.next_pulse = inp.now + gap;
                if amplitude > 0.003 {
                    self.pulses.push(Pulse { hand: h, seconds: PULSE_LENGTH, frequency: 160.0, amplitude });
                }
            }
        }
        let state = match c.phase {
            Phase::Charging { .. } if c.charge >= 1.0 => CAST_READY,
            Phase::Charging { .. } => CAST_CHARGING,
            _ if matches!(source, Source::Spell(_)) && inp.now < c.shown_until => CAST_READIED,
            _ => CAST_NONE,
        };
        if state != c.state {
            c.state = state;
            c.since = inp.now;
        }
        let magicka = if self.table.magicka_max > 0.0 { (self.table.magicka / self.table.magicka_max).clamp(0.0, 1.0) } else { 1.0 };
        let mut out = Caster {
            state,
            flags: if matches!(source, Source::Thing(_)) { CASTF_OBJECT } else { 0 },
            spell: id,
            object: if let Source::Thing(d) = source { d.id } else { 0 },
            charge: if state == CAST_READIED { magicka } else { c.charge },
            time: (inp.now - c.since) as f32,
            at,
            ..Default::default()
        };
        // Which way it goes, and onto whom.
        let head_rot = inp.map.rot(Quat::from_array(inp.frame.hmd.rot));
        let (dir, onto) = match delivery {
            DELIVER_SELF => (head_rot * Vec3::Z, 0),
            // (Into what it touched: towards it.)
            DELIVER_TOUCH => touched.map_or((smooth, 0), |(id, on)| ((on - from).normalize_or(smooth), id)),
            _ if inp.settings.aim_assist => assist(inp.actors, from, smooth, inp.map.world_per_meter, flags & SPELL_HARMFUL != 0),
            _ => (smooth, 0),
        };
        out.aim = inp.map.inv_rot(Quat::IDENTITY).mul_vec3(dir).normalize_or_zero().to_array();
        if !game_art && state != CAST_NONE {
            self.glows.push(Glow { at: Vec3::from_array(at.pos), color, charge: if state == CAST_READIED { 0.0 } else { c.charge }, state });
        }
        let Some(touch) = cast else {
            self.casting[h] = out;
            return;
        };
        let actor = touch.unwrap_or(onto);
        let touched_flag = if touch.is_some() || (delivery == DELIVER_TOUCH && onto != 0) { CEF_TOUCHED } else { 0 };
        c.phase = Phase::Spent;
        c.charge = 0.0;
        c.recover_until = inp.now + recover as f64;
        let speed = c.vel.length();
        out.flags |= CASTF_CAST;
        self.casting[h] = out;
        let ev = CombatEvent { kind: CE_CAST, flags: touched_flag, actor, what: id, object: out.object, hand: h as u32, strength: 1.0, speed, point: from.to_array(), dir: dir.to_array(), ..Default::default() };
        self.debug.casts += 1;
        self.debug.last = Some(ev);
        // (How far up it went, and the controller's own ray and the head: degrees.)
        let up = |v: Vec3| v.normalize_or_zero().y.clamp(-1.0, 1.0).asin().to_degrees();
        let ray = Quat::from_array(inp.frame.hands[h].aim.rot) * Vec3::Z;
        let head = Quat::from_array(inp.frame.hmd.rot) * Vec3::Z;
        tracing::info!(
            "magic: cast {name} from the {} hand{} at {from:.2?} towards {dir:.2?} (on {actor:#x}); up {:.0} deg, the controller's ray {:.0} ({:.0} off), the head {:.0}",
            if h == 0 { "left" } else { "right" },
            if out.object != 0 { "'s staff" } else { "" },
            up(dir),
            up(ray),
            inp.map.rot(Quat::IDENTITY).mul_vec3(ray).normalize_or_zero().dot(dir.normalize_or_zero()).clamp(-1.0, 1.0).acos().to_degrees(),
            up(head),
        );
        self.pulses.push(Pulse { hand: h, ..RELEASE });
        self.events.push(ev);
    }
}

/// The living creature a palm at `palm` (world) touches, if any, and the point of it touched.
fn touching(actors: &[Actor], palm: Vec3) -> Option<(u32, Vec3)> {
    actors.iter().filter(|a| a.flags & ACT_ALIVE != 0).find_map(|a| {
        a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)].iter().find_map(|p| {
            let (pa, pb) = (Vec3::from_array(p.a), Vec3::from_array(p.b));
            let (_, t) = closest_segments(palm, palm, pa, pb);
            let on = pa.lerp(pb, t);
            (palm.distance(on) <= p.radius + TOUCH).then_some((a.id, on))
        })
    })
}

/// Spell Auto-Aim VR's cone: the way from `from` (world) to the living creature whose body (the
/// middle of a torso part) is nearest the aim `dir`, within [`ASSIST_UP`] degrees up or down and
/// [`ASSIST_ACROSS`] across and [`ASSIST_RANGE`] metres, and its id; a harmful spell takes a foe
/// before anyone else. Else `dir` itself.
pub fn assist(actors: &[Actor], from: Vec3, dir: Vec3, world_per_meter: f32, harmful: bool) -> (Vec3, u32) {
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    let pitch = |v: Vec3| v.y.clamp(-1.0, 1.0).asin();
    let mut best: Option<(bool, f32, Vec3, u32)> = None;
    for a in actors {
        if a.flags & ACT_ALIVE == 0 || a.count == 0 {
            continue;
        }
        let parts = &a.parts[..(a.count as usize).min(MAX_ACTOR_PARTS)];
        let torso: Vec<_> = parts.iter().filter(|p| p.kind == PART_TORSO).collect();
        let aimed = if torso.is_empty() { vec![&parts[0]] } else { torso };
        for part in aimed {
            let c = (Vec3::from_array(part.a) + Vec3::from_array(part.b)) * 0.5;
            let to = c - from;
            let d = to.length() / world_per_meter.max(1e-6);
            if !(0.3..=ASSIST_RANGE).contains(&d) {
                continue;
            }
            let to = to.normalize();
            let across = flat(to).dot(flat(dir)).clamp(-1.0, 1.0).acos().to_degrees();
            let up = (pitch(to) - pitch(dir)).abs().to_degrees();
            if across > ASSIST_ACROSS || up > ASSIST_UP {
                continue;
            }
            let score = (across / ASSIST_ACROSS).powi(2) + (up / ASSIST_UP).powi(2);
            let foe = harmful && a.flags & ACT_HOSTILE != 0;
            if best.is_none_or(|(bf, bs, ..)| (foe && !bf) || (foe == bf && score < bs)) {
                best = Some((foe, score, to, a.id));
            }
        }
    }
    match best {
        Some((_, _, d, id)) => (d, id),
        None => (dir, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_charge_is_felt_late_then_sharply() {
        let (gap0, a0) = charge_pulse(0.0);
        let (_, a_half) = charge_pulse(0.5);
        let (gap1, a1) = charge_pulse(1.0);
        assert!((gap0 - 0.1).abs() < 1e-9 && (gap1 - 0.02).abs() < 1e-9);
        assert!(a0 == 0.0 && a_half < 0.02 && (a1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_hand_casts_along_the_arm_held_out() {
        // The arm held out ahead and a little down, the wrist cocked up (the controller pointing
        // well above it): the spell goes along the arm.
        let shoulder = Vec3::new(0.18, 1.5, 0.0);
        let way = Vec3::new(0.0, -0.2, 1.0).normalize();
        let palm = shoulder + way * 0.55;
        let cocked = Quat::from_rotation_x(-0.8);
        assert!((hand_aim(shoulder, palm, cocked, cocked, 1, 0.0) - way).length() < 1e-4);
        // The hand in at the shoulder: where the controller points.
        let aim = hand_aim(shoulder, shoulder + Vec3::new(0.0, 0.0, 0.1), cocked, cocked, 1, 0.0);
        assert!((aim - cocked * Vec3::Z).length() < 1e-4, "{aim:?}");
        // Palm casting: turned towards the palm (the left hand's to its right, +X; the right's -X).
        let ahead = Vec3::new(0.0, 1.5, 0.6);
        assert!(hand_aim(Vec3::new(0.0, 1.5, 0.0), ahead, Quat::IDENTITY, Quat::IDENTITY, 0, 60.0).x > 0.8);
        assert!(hand_aim(Vec3::new(0.0, 1.5, 0.0), ahead, Quat::IDENTITY, Quat::IDENTITY, 1, 60.0).x < -0.8);
    }

    #[test]
    fn a_shoulder_off_the_head_unless_tracked() {
        let mut f = HmdFrame::default();
        f.hmd.pos = [0.0, 1.7, 0.0];
        f.hmd.rot = Quat::IDENTITY.to_array();
        let (l, r) = (shoulder(&f, 0), shoulder(&f, 1));
        assert!(l.x < -0.1 && r.x > 0.1 && (l.y - 1.48).abs() < 1e-4 && l.z < 0.0, "{l:?} {r:?}");
        // Turned to face +X: the right shoulder is behind (-Z).
        f.hmd.rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2).to_array();
        assert!(shoulder(&f, 1).z < -0.1, "{:?}", shoulder(&f, 1));
        f.shoulders[1] = Some([0.3, 1.4, 0.1]);
        assert_eq!(shoulder(&f, 1), Vec3::new(0.3, 1.4, 0.1));
    }

    #[test]
    fn the_cone_takes_a_foe_near_the_aim_and_not_one_far_off() {
        let mut a: Actor = bytemuck::Zeroable::zeroed();
        a.id = 7;
        a.flags = ACT_ALIVE | ACT_HOSTILE;
        a.count = 1;
        a.parts[0] = ActorPart { a: [0.5, 1.0, 10.0], b: [0.5, 1.6, 10.0], radius: 0.2, kind: PART_TORSO, ..Default::default() };
        let from = Vec3::new(0.0, 1.3, 0.0);
        let (d, id) = assist(&[a], from, Vec3::Z, 1.0, true);
        assert_eq!(id, 7);
        assert!(d.x > 0.0);
        // 45 degrees off: not helped.
        let (_, id) = assist(&[a], from, Vec3::new(-1.0, 0.0, 1.0).normalize(), 1.0, true);
        assert_eq!(id, 0);
    }
}
