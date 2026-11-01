//! The range's duelist: one fighter for the duel (`docs/DUEL.md`; its timings in
//! `rustvr_melee::duel`), standing in the duel ring (`arenas::DUEL`). Its swings are our own
//! keyframes, posed through the arm's reach (`anim::Pose::arm`) and followed by its ragdoll's pins:
//! a slow wind-up the player can read, a release that hurts what it meets, a recovery. Its swings
//! come from four sides: its right, its left, over the top and from under.
//!
//! It reads the player's blade as a person would: it sees it [`tune::REACTION`] late, judges when it
//! will reach it from how it was closing then (its speed and how that was changing), and opens its
//! parry so the blow lands early in the window, its blade put across the blow's way. A blade that
//! slows on the way (the player turning away: a drag) comes after the window has closed; one that
//! speeds up (turning into it: an accel) comes before it has seen it. Once open, a parry can't be
//! timed again: one that met nothing recovers before another.
//!
//! The rules are the player's: a parry of its own (the runtime's `CEF_PARRIED`) gives it the
//! riposte, a quick wind-up straight into a swing; its swing parried by the player (`CEF_PARRY`)
//! staggers it, unable to swing until it's over and to parry until [`tune::STAGGER_PARRY`]; a hit
//! while it winds up holds the wind-up back a moment (a flinch), and while it releases does nothing
//! to the swing.

use std::collections::VecDeque;

use glam::{Quat, Vec3};
use rustvr_melee::duel as tune;

use crate::anim::{self, Pose};
use crate::brain::Sight;

/// Where it swings from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// From its right, cutting across to its left.
    Right,
    /// From its left (a backhand), across to its right.
    Left,
    /// From above, down.
    Over,
    /// From below, up.
    Under,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Right, Side::Left, Side::Over, Side::Under];

    pub fn parse(s: &str) -> Option<Side> {
        Side::ALL.into_iter().find(|side| side.name() == s)
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Right => "right",
            Side::Left => "left",
            Side::Over => "over",
            Side::Under => "under",
        }
    }

    /// Its keys: wound up, striking, followed through; and the torso's turn (radians, to its right)
    /// at each. Short arcs, so the blade comes on slowly enough to read (its tip at most about 6 m/s
    /// over the release).
    fn keys(self) -> ([Key; 3], [f32; 3]) {
        match self {
            Side::Right => (
                [k([0.38, 1.5, 0.05], [0.4, 0.85, -0.3], [-0.6, 0.3, 0.75]), k([0.1, 1.33, 0.52], [-0.3, 0.05, 1.0], [-1.0, 0.0, -0.3]), k([-0.2, 1.18, 0.4], [-0.7, -0.15, 0.65], [-0.65, -0.1, -0.7])],
                [0.4, 0.0, -0.3],
            ),
            Side::Left => (
                [k([-0.15, 1.48, 0.1], [-0.4, 0.85, -0.3], [0.6, 0.3, 0.75]), k([0.05, 1.32, 0.52], [0.3, 0.05, 0.95], [1.0, 0.0, -0.3]), k([0.4, 1.18, 0.35], [0.7, -0.2, 0.6], [0.65, -0.1, -0.7])],
                [-0.4, 0.0, 0.3],
            ),
            Side::Over => (
                [k([0.2, 1.8, 0.0], [0.05, 0.85, -0.45], [0.0, 0.5, 0.85]), k([0.12, 1.42, 0.5], [0.0, -0.25, 1.0], [0.0, -1.0, -0.25]), k([0.1, 1.05, 0.45], [0.0, -0.75, 0.6], [0.0, -0.6, -0.75])],
                [0.15, 0.0, -0.1],
            ),
            Side::Under => (
                [k([0.35, 0.92, 0.05], [0.3, -0.75, -0.35], [0.0, -0.8, 0.5]), k([0.12, 1.12, 0.5], [0.0, 0.25, 1.0], [0.0, 1.0, -0.25]), k([0.0, 1.45, 0.4], [-0.1, 0.75, 0.6], [0.0, 0.6, -0.75])],
                [0.3, 0.0, -0.2],
            ),
        }
    }
}

/// Where the sword hand is and how the sword lies (its own space: facing +Z, its right +X): the
/// wrist, the blade's way and its edge's.
#[derive(Clone, Copy, Debug)]
struct Key {
    wrist: Vec3,
    blade: Vec3,
    edge: Vec3,
}

const fn k(wrist: [f32; 3], blade: [f32; 3], edge: [f32; 3]) -> Key {
    Key { wrist: Vec3::from_array(wrist), blade: Vec3::from_array(blade), edge: Vec3::from_array(edge) }
}

impl Key {
    /// The hand's turn that lies the sword so (the weapon's +Z along the blade, +X along its edge).
    fn turn(&self) -> Quat {
        let blade = self.blade.normalize_or(Vec3::Z);
        let edge = (self.edge - blade * self.edge.dot(blade)).normalize_or(blade.any_orthonormal_vector());
        Quat::from_mat3(&glam::Mat3::from_cols(edge, blade.cross(edge), blade)).normalize()
    }

    /// Partway to `to`: the wrist along the way, the sword turned the short way round.
    fn lerp(self, to: Key, s: f32) -> Key {
        let q = self.turn().slerp(to.turn(), s);
        Key { wrist: self.wrist.lerp(to.wrist, s), blade: q * Vec3::Z, edge: q * Vec3::X }
    }
}

/// Its guard: the sword up and forward, before it.
const GUARD: Key = k([0.2, 1.22, 0.33], [0.1, 0.8, 0.6], [-1.0, 0.0, 0.1]);
/// Knocked back off a parry: the sword thrown up and back, the hand pulled in.
const RECOIL: Key = k([0.3, 1.45, 0.08], [0.35, 0.9, -0.25], [-0.6, 0.2, 0.75]);
/// Its free hand, low at its side.
const OFF_HAND: Vec3 = Vec3::new(-0.26, 1.0, 0.16);
/// How low it stands, and how far its blade's middle is from its wrist (m).
const CROUCH: f32 = 0.06;
const MID_BLADE: f32 = 0.4;
/// It steps in as it winds up while further than this from the player (m), and lunges as it
/// releases at this speed (m/s) while further than [`LUNGE_TO`]. Driven for a test, it keeps this
/// far off between its swings.
const STEP_IN: f32 = 1.2;
const LUNGE: f32 = 0.6;
const LUNGE_TO: f32 = 0.95;
const TEST_SPACING: f32 = 1.45;
/// A blade's hits on it within this long are one blow (s); a blow landing on it in its guard
/// brings its next swing this soon at the latest (s).
const ONE_BLOW: f64 = 0.4;
const ANSWER: f64 = 0.4;
/// What it guards with its parries (its own space): its chest to its head, this thick (m).
const BODY: (Vec3, Vec3, f32) = (Vec3::new(0.0, 0.95, 0.0), Vec3::new(0.0, 1.75, 0.0), 0.25);

/// What it's doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Guard,
    /// Winding up a swing from `side` (a riposte's quicker), from where its sword was; held back by
    /// flinches so far.
    WindUp { side: Side, riposte: bool, from: KeyAt, held: f64 },
    Release { side: Side, hit: bool },
    /// Coming back to its guard from where its sword was, until then.
    Recover { from: KeyAt, until: f64 },
    /// Its parry: its sword across the blow's way, open until then.
    Parry { key: KeyAt, until: f64 },
    /// After a parry that met nothing, until then.
    ParryRecover { from: KeyAt, until: f64 },
    /// Its swing parried: knocked back, until then.
    Staggered { from: KeyAt, until: f64 },
}

/// A key and the torso's turn with it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyAt {
    wrist: Vec3,
    blade: Vec3,
    edge: Vec3,
    twist: f32,
}

impl KeyAt {
    fn of(k: Key, twist: f32) -> KeyAt {
        KeyAt { wrist: k.wrist, blade: k.blade, edge: k.edge, twist }
    }

    fn key(&self) -> Key {
        Key { wrist: self.wrist, blade: self.blade, edge: self.edge }
    }

    fn lerp(self, to: KeyAt, s: f32) -> KeyAt {
        KeyAt::of(self.key().lerp(to.key(), s), self.twist + (to.twist - self.twist) * s)
    }
}

/// How a duelist is driven: by its own will, or (the tests) swinging only when the player raises the
/// off hand over the head (from that side), or never swinging but to riposte.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drive {
    Free,
    Cue(Side),
    Guard,
}

/// What it saw of the player's blade at a moment: how far it was from reaching it (m), how fast it
/// was closing (m/s), where on it it would meet, which way it was going and lay (its own space).
#[derive(Clone, Copy, Debug)]
struct Seen {
    t: f64,
    gap: f32,
    closing: f32,
    at: Vec3,
    vel: Vec3,
    along: Vec3,
}

/// What the duelist does with its body this frame.
pub struct Think {
    pub pose: Pose,
    /// Its walk (its own space, m/s) and which way it turns to face (world).
    pub walk: Vec3,
    pub face_to: Option<Vec3>,
    /// Its sword hurts what it meets (its release).
    pub attacking: bool,
    /// How its sword lies (its own space): as keyed, past what its wrist would turn to.
    pub sword: Quat,
}

pub struct Duelist {
    pub person: usize,
    pub id: u32,
    pub drive: Drive,
    state: State,
    since: f64,
    /// Its sword as last posed (for moving on from where it was).
    last: KeyAt,
    next_swing: f64,
    last_side: Option<Side>,
    /// What it's seen of the player's blade, and how late it sees it now.
    seen: VecDeque<Seen>,
    reaction: f64,
    rng: u32,
    /// The cue (the tests) was up last frame.
    cue_was: bool,
    /// The last blow that landed on it: by what, when (a blow lands once).
    last_blow: Option<(u32, f64)>,
}

impl Duelist {
    pub fn new(person: usize, id: u32, drive: Drive, now: f64) -> Duelist {
        Duelist { person, id, drive, state: State::Guard, since: now, last: KeyAt::of(GUARD, 0.0), next_swing: now + 2.0, last_side: None, seen: VecDeque::new(), reaction: tune::REACTION, rng: 0x9E37_79B9 ^ id, cue_was: false, last_blow: None }
    }

    fn roll(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn set(&mut self, state: State, now: f64) {
        tracing::info!("duelist {:#x}: {} (was {})", self.id, Self::name(&state), Self::name(&self.state));
        self.state = state;
        self.since = now;
    }

    fn name(s: &State) -> String {
        match s {
            State::Guard => "guard".into(),
            State::WindUp { side, riposte, .. } => format!("winds up from its {}{}", side.name(), if *riposte { " (a riposte)" } else { "" }),
            State::Release { side, .. } => format!("releases from its {}", side.name()),
            State::Recover { .. } => "recovers".into(),
            State::Parry { .. } => "parries".into(),
            State::ParryRecover { .. } => "recovers from its parry".into(),
            State::Staggered { .. } => "staggered".into(),
        }
    }

    /// Its sword hurts what it meets (its release).
    pub fn attacking(&self) -> bool {
        matches!(self.state, State::Release { .. })
    }

    /// Its parry's window is open.
    pub fn parrying(&self) -> bool {
        matches!(self.state, State::Parry { .. })
    }

    /// Down, out cold or dead: whatever it was doing stops.
    pub fn sleep(&mut self, now: f64) {
        if self.state != State::Guard {
            self.state = State::Guard;
            self.since = now;
        }
        self.seen.clear();
    }

    /// The runtime's word on a clash with its sword: `parried` (its parry turned the player's blow:
    /// its riposte), `its_swing_parried` (the player parried it: staggered), or neither (it beat the
    /// player's blade aside: its swing goes on).
    pub fn clashed(&mut self, now: f64, parried: bool, its_swing_parried: bool) {
        if parried && matches!(self.state, State::Parry { .. }) {
            let side = self.nearest_side();
            tracing::info!("duelist {:#x}: parried the player's blow; its riposte from its {}", self.id, side.name());
            let from = self.last;
            self.set(State::WindUp { side, riposte: true, from, held: 0.0 }, now);
        } else if its_swing_parried && matches!(self.state, State::Release { .. } | State::WindUp { .. }) {
            tracing::info!("duelist {:#x}: its swing parried: staggered {:.2} s", self.id, tune::STAGGER);
            let from = self.last;
            self.set(State::Staggered { from, until: now + tune::STAGGER }, now);
        } else if !parried && !its_swing_parried {
            tracing::info!("duelist {:#x}: beat the player's blade aside", self.id);
        }
    }

    /// Whether a blow of `object`'s lands on it now: one a swing (the player's blade meeting its
    /// parts one after another in a swing is one blow).
    pub fn takes(&mut self, object: u32, now: f64) -> bool {
        if self.last_blow.is_some_and(|(o, t)| o == object && now - t < ONE_BLOW) {
            return false;
        }
        self.last_blow = Some((object, now));
        true
    }

    /// Its sword struck the player.
    pub fn struck(&mut self) {
        if let State::Release { side, .. } = self.state {
            self.state = State::Release { side, hit: true };
        }
    }

    /// A blow landed on it: winding up, it flinches (its wind-up held back); anything else goes on.
    pub fn hurt(&mut self, now: f64) {
        if let State::WindUp { side, riposte, from, held } = self.state {
            tracing::info!("duelist {:#x}: hit winding up: it flinches", self.id);
            self.state = State::WindUp { side, riposte, from, held: (held + tune::FLINCH).min(now - self.since) };
        } else if let State::Release { .. } = self.state {
            tracing::info!("duelist {:#x}: hit as it swings: its swing goes on", self.id);
        } else {
            tracing::info!("duelist {:#x}: hit (it {})", self.id, Self::name(&self.state));
            // It answers a blow that lands: a swing back soon.
            if self.state == State::Guard {
                self.next_swing = self.next_swing.min(now + ANSWER);
            }
        }
    }

    /// The side whose wind-up is nearest where its sword is.
    fn nearest_side(&self) -> Side {
        let at = self.last.wrist;
        Side::ALL.into_iter().min_by(|a, b| a.keys().0[0].wrist.distance(at).total_cmp(&b.keys().0[0].wrist.distance(at))).unwrap_or(Side::Right)
    }

    /// One frame: what it sees (the player, `sight`; its own feet and facing; the off hand raised
    /// for a cue), what it does.
    pub fn think(&mut self, now: f64, dt: f32, at: Vec3, face: Quat, sight: &Sight, cue: bool) -> Think {
        let mine = |p: Vec3| face.inverse() * (p - at);
        let to_player = (sight.feet - at).with_y(0.0);
        let dist = to_player.length();
        let free = self.drive == Drive::Free;
        // (Driven for a test, it keeps to where it was put; free, to its blade's reach.)
        let spacing = if free { tune::SPACING } else { TEST_SPACING };
        self.look(now, dt, sight, &mine);
        let threat = self.threat(now);
        let mut walk = Vec3::ZERO;
        let mut attacking = false;
        // Its parry: as the blow it sees coming is due, when it may.
        // (Early in its own wind-up it may break off to parry, when it fights of its own will.)
        let may_parry = match self.state {
            State::Guard => true,
            State::WindUp { riposte, held, .. } => !riposte && free && now - self.since - held < tune::WINDUP * 0.5,
            State::Staggered { .. } => now - self.since >= tune::STAGGER_PARRY,
            _ => false,
        };
        if may_parry && let Some((when, contact, vel, along)) = threat {
            if now >= when - tune::PARRY_LEAD {
                let key = Self::parry_key(contact, vel, along);
                tracing::info!("duelist {:#x}: sees a blow due in {:.2} s ({:.2} s late): parries across it at {:.2?}", self.id, when - now, self.reaction, contact);
                self.set(State::Parry { key, until: now + tune::PARRY_WINDOW }, now);
                self.reaction = tune::REACTION + tune::REACTION_SPREAD * (2.0 * self.roll() as f64 - 1.0);
            }
        }
        let cue_now = cue && !self.cue_was;
        self.cue_was = cue;
        let s = (now - self.since) as f32;
        let ease = |x: f32| x * x * (3.0 - 2.0 * x);
        let key = match self.state {
            State::Guard => {
                let guard = KeyAt::of(GUARD, 0.0);
                let k = self.last.lerp(guard, (dt * 6.0).min(1.0));
                if dist > spacing + 0.15 {
                    walk = Vec3::Z * tune::WALK;
                } else if dist < spacing - 0.3 {
                    walk = -Vec3::Z * tune::WALK * 0.6;
                }
                // (A blow coming too soon to parry doesn't hold its own swing back.)
                let held_back = threat.is_some_and(|(when, ..)| when - now > tune::PARRY_LEAD + 0.25);
                let swing = match self.drive {
                    Drive::Free => (now >= self.next_swing && dist <= tune::SWING_RANGE && !held_back).then(|| self.pick_side()),
                    Drive::Cue(side) => cue_now.then_some(side),
                    Drive::Guard => None,
                };
                if let Some(side) = swing {
                    let from = k;
                    self.set(State::WindUp { side, riposte: false, from, held: 0.0 }, now);
                }
                k
            }
            State::WindUp { side, riposte, from, held } => {
                let long = if riposte { tune::RIPOSTE_WINDUP } else { tune::WINDUP };
                let t = ((now - self.since - held) / long).clamp(0.0, 1.0) as f32;
                let (keys, twist) = side.keys();
                let k = from.lerp(KeyAt::of(keys[0], twist[0]), ease(t));
                // (It steps into its blow as it winds up, and lunges as it releases.)
                if dist > STEP_IN {
                    walk = Vec3::Z * tune::WALK * 0.7;
                }
                if now - self.since - held >= long {
                    self.last_side = Some(side);
                    self.set(State::Release { side, hit: false }, now);
                }
                k
            }
            State::Release { side, hit } => {
                let t = (s / tune::RELEASE as f32).clamp(0.0, 1.0);
                let (keys, twist) = side.keys();
                let (a, b, c) = (KeyAt::of(keys[0], twist[0]), KeyAt::of(keys[1], twist[1]), KeyAt::of(keys[2], twist[2]));
                // Gathering speed to the strike half way, slowing after it.
                let u = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
                let k = if u < 0.5 { a.lerp(b, u * 2.0) } else { b.lerp(c, (u - 0.5) * 2.0) };
                attacking = true;
                if dist > LUNGE_TO {
                    walk = Vec3::Z * LUNGE;
                }
                if t >= 1.0 {
                    let rest = if hit { tune::RECOVERY } else { tune::MISS_RECOVERY };
                    self.set(State::Recover { from: k, until: now + rest }, now);
                }
                k
            }
            State::Recover { from, until } => {
                let long = (until - self.since).max(0.05) as f32;
                let k = from.lerp(KeyAt::of(GUARD, 0.0), ease((s / long).clamp(0.0, 1.0)));
                if dist < spacing - 0.2 {
                    walk = -Vec3::Z * tune::WALK * 0.6;
                }
                if now >= until {
                    let pause = tune::PAUSE.0 + (tune::PAUSE.1 - tune::PAUSE.0) * self.roll() as f64;
                    self.next_swing = now + pause;
                    self.set(State::Guard, now);
                }
                k
            }
            State::Parry { key, until } => {
                let k = self.last.lerp(key, (dt * 14.0).min(1.0));
                if now >= until {
                    tracing::info!("duelist {:#x}: its parry met nothing", self.id);
                    self.set(State::ParryRecover { from: k, until: now + tune::PARRY_RECOVERY }, now);
                }
                k
            }
            State::ParryRecover { from, until } => {
                let long = (until - self.since).max(0.05) as f32;
                let k = from.lerp(KeyAt::of(GUARD, 0.0), ease((s / long).clamp(0.0, 1.0)));
                if now >= until {
                    self.set(State::Guard, now);
                }
                k
            }
            State::Staggered { from, until } => {
                let long = (until - self.since).max(0.05) as f32;
                let u = (s / long).clamp(0.0, 1.0);
                // Thrown back fast, then gathering itself back to its guard.
                let recoil = KeyAt::of(RECOIL, 0.15);
                let k = if u < 0.3 { from.lerp(recoil, ease(u / 0.3)) } else { recoil.lerp(KeyAt::of(GUARD, 0.0), ease((u - 0.3) / 0.7)) };
                if dist < spacing {
                    walk = -Vec3::Z * 0.4;
                }
                if now >= until {
                    self.next_swing = now + tune::PAUSE.0;
                    self.set(State::Guard, now);
                }
                k
            }
        };
        self.last = key;
        let mut pose = Pose::default();
        crouch(&mut pose, CROUCH);
        let torso = Quat::from_rotation_y(key.twist);
        pose.turn[anim::TORSO] = torso;
        pose.arm(1, key.wrist, key.key().turn(), 0.0);
        pose.arm(0, OFF_HAND, Quat::IDENTITY, 0.0);
        Think { pose, walk, face_to: (dist > 0.05).then_some(to_player), attacking, sword: (torso * key.key().turn()).normalize() }
    }

    /// A side to swing from: any but the last one twice running.
    fn pick_side(&mut self) -> Side {
        let n = (self.roll() * 4.0) as usize % 4;
        let side = Side::ALL[n];
        if Some(side) == self.last_side { Side::ALL[(n + 1 + (self.roll() * 3.0) as usize % 3) % 4] } else { side }
    }

    /// Its sword across a blow coming at `contact` (its own space) moving `vel` with its blade lying
    /// `along`: square to the blade's way and its swing, its edge to it, the blade's middle where the
    /// blow will be, held within its guard.
    fn parry_key(contact: Vec3, vel: Vec3, along: Vec3) -> KeyAt {
        let p = Vec3::new(contact.x.clamp(-0.5, 0.55), contact.y.clamp(0.95, 1.85), 0.42);
        let v = vel.normalize_or(-Vec3::Z);
        let mut d = along.cross(v).normalize_or(Vec3::Y);
        // Upright when it can be; lying across, the hilt on its right.
        if d.y.abs() > 0.4 {
            d *= d.y.signum();
        } else if d.x > 0.0 {
            d = -d;
        }
        let edge = -(v - d * v.dot(d)).normalize_or(Vec3::Z);
        KeyAt::of(Key { wrist: p - d * MID_BLADE, blade: d, edge }, -p.x * 0.4)
    }

    /// What it sees of the player's blades now (its own space), kept a second.
    fn look(&mut self, now: f64, dt: f32, sight: &Sight, mine: &impl Fn(Vec3) -> Vec3) {
        let mut best: Option<Seen> = None;
        for t in &sight.threats {
            let (a, b) = (mine(t.a), mine(t.b));
            let vel = mine(t.b + t.vel) - b;
            let (s, u) = closest(a, b, BODY.0, BODY.1);
            let p = a.lerp(b, s);
            let gap = (p.distance(BODY.0.lerp(BODY.1, u)) - BODY.2).max(0.0);
            if best.is_none_or(|b| gap < b.gap) {
                best = Some(Seen { t: now, gap, closing: 0.0, at: p, vel, along: (b - a).normalize_or(Vec3::Z) });
            }
        }
        let Some(mut seen) = best else {
            self.seen.clear();
            return;
        };
        if let Some(last) = self.seen.back() {
            let raw = (last.gap - seen.gap) / dt.max(1e-3);
            seen.closing = last.closing + (raw - last.closing) * 0.5;
        }
        self.seen.push_back(seen);
        while self.seen.front().is_some_and(|s| now - s.t > 1.0) {
            self.seen.pop_front();
        }
    }

    /// A blow coming at it, as it sees it now (`reaction` late): when it will land, where (its own
    /// space), how it moves and lies. Judged from how fast it was closing then, and how that was
    /// changing (a swing gathers speed).
    fn threat(&self, now: f64) -> Option<(f64, Vec3, Vec3, Vec3)> {
        let then = now - self.reaction;
        let i = self.seen.iter().rposition(|s| s.t <= then)?;
        let s = self.seen[i];
        let before = self.seen.iter().take(i).rev().find(|b| s.t - b.t >= 0.04)?;
        if s.gap > tune::THREAT_REACH || s.closing < tune::THREAT_SPEED {
            return None;
        }
        let gather = ((s.closing - before.closing) / (s.t - before.t) as f32).clamp(0.0, 25.0);
        let c = s.closing;
        let t = if gather > 1e-3 { (-c + (c * c + 2.0 * gather * s.gap).sqrt()) / gather } else { s.gap / c };
        if !t.is_finite() || t as f64 > tune::THREAT_TIME {
            return None;
        }
        let at = s.at + s.vel * t;
        Some((s.t + t as f64, at, s.vel, s.along))
    }
}

/// Both legs bent so the pelvis is `drop` lower, the feet where they stand.
fn crouch(pose: &mut Pose, drop: f32) {
    let leg = anim::hip(0).y - 0.07;
    let bend = (1.0 - drop / leg).clamp(-1.0, 1.0).acos();
    pose.pelvis.y = -drop;
    for l in 0..2 {
        pose.turn[anim::THIGH[l]] = Quat::from_rotation_x(-bend);
        pose.turn[anim::SHIN[l]] = Quat::from_rotation_x(2.0 * bend);
    }
}

/// The closest points between segments: (s, t) along each.
fn closest(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (f32, f32) {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    if a <= 1e-9 {
        return (0.0, if e > 1e-9 { (f / e).clamp(0.0, 1.0) } else { 0.0 });
    }
    let c = d1.dot(r);
    if e <= 1e-9 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = d1.dot(d2);
    let denom = a * e - b * b;
    let s = if denom > 1e-9 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let t = ((b * s + f) / e).clamp(0.0, 1.0);
    (((b * t - c) / a).clamp(0.0, 1.0), t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Threat;

    /// A blade at `tip` (its own space) moving `vel`, as the player's.
    fn sight_of(at: Vec3, face: Quat, tip: Vec3, vel: Vec3) -> Sight {
        let w = |p: Vec3| at + face * p;
        Sight { feet: w(Vec3::new(0.0, 0.0, 1.6)), threats: vec![Threat { a: w(tip + Vec3::new(0.0, 0.0, 0.7)), b: w(tip), vel: face * vel, point: false }], ..Default::default() }
    }

    /// A swing that comes on steadily is parried in time; the same swing dragged (slowed for a moment
    /// after the duelist judged it, then on at speed) lands after its window has closed.
    #[test]
    fn a_drag_comes_after_the_parry() {
        let (at, face) = (Vec3::ZERO, Quat::IDENTITY);
        let dt = 1.0 / 90.0;
        for drag in [false, true] {
            let mut d = Duelist::new(0, 1, Drive::Guard, 0.0);
            // Swung in from its left at 4 m/s, close in front of it; dragged, slowed to 0.8 m/s for
            // 0.3 m on the way.
            let mut tip = Vec3::new(-2.5, 1.4, 0.15);
            let mut opened = None;
            let mut landed = None;
            for n in 0..200 {
                let now = n as f64 * dt as f64;
                let speed = if drag && tip.x > -1.0 && tip.x < -0.7 { 0.8 } else { 4.0 };
                let vel = Vec3::new(speed, 0.0, 0.0);
                tip += vel * dt;
                d.think(now, dt, at, face, &sight_of(at, face, tip, vel), false);
                if d.parrying() && opened.is_none() {
                    opened = Some(now);
                }
                if landed.is_none() && tip.x >= -0.2 {
                    landed = Some(now);
                }
            }
            let (opened, landed) = (opened.expect("it parries"), landed.expect("the blow lands"));
            let into = landed - opened;
            if drag {
                assert!(into > tune::PARRY_WINDOW, "dragged, it lands {into:.2} s after the parry opened: after its window");
            } else {
                assert!((0.0..=tune::PARRY_WINDOW).contains(&into), "steady, it lands {into:.2} s into the window");
            }
        }
    }
}
