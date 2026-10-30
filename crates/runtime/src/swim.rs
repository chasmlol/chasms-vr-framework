//! Swimming by the arms, for any game whose adapter says where its water is
//! (`rustvr_protocol::WaterState`). The game decides when the player swims and how fast they may;
//! the runtime turns the player's strokes into the velocity the game's swimming moves them at
//! (`Intents::swim`). Research and design: the Oblivion adapter's `docs/research/swimming.md`.
//!
//! * **A hand pushes the water** when it's under the surface and empty (holding nothing, not
//!   gripping), as a hand in water does: the faster it moves through the water, the harder (with
//!   the square of its speed), and the swimmer goes the other way. Any stroke works (a crawl, a
//!   breaststroke, a dog paddle, a push down to rise, up to dive), a small one for a little move and
//!   a hard fast one to shoot along. The palm doesn't decide it: a hand round a controller's handle
//!   faces the other hand whichever way it sweeps. The hand's velocity is measured against the
//!   head, so walking about the room pushes nothing, and under a dead zone (a hand drifting, or
//!   reaching about) it pushes nothing either.
//! * **The arms steer**, in any direction, whatever the head does: a stroke drives the swimmer the
//!   way it pushes the water, so a breaststroke done downwards dives. A stroke's stronger part (the
//!   pull) sets the way; a hand going that way through the water (the recovery reaching out again,
//!   or hands held still in a glide) slices through it, and any other way it meets the water. So
//!   turn and stroke a new way, and the old way's momentum dies in the hands instead of drifting the
//!   swimmer off. A recovery as quick as its pull doesn't turn the swimmer round; a stroke clearly
//!   harder than the recent ones does, or any stroke after a pause. Gliding out, the swimmer slows
//!   with the water's drag (the faster, the sooner).
//! * **Faster than the game's swimmer.** The game's swim speed (skills, attributes, load) is the
//!   strokes' scale: a firm stroke surges the swimmer to about two and a half times it, a gentle one
//!   to about the game's own, and nothing goes past four times it.
//! * **Comfort**: the strokes only ever push; they never turn the player. At the surface the game
//!   holds the player up (pushing up there does nothing), and a stroke's little drift down is held
//!   there too: diving takes a stroke that goes down. Under water there's no buoyancy: a still
//!   swimmer hangs where they are, as the game has them.
//! * **Feel**: a pushing hand buzzes with the water's resistance; a hand slapping into it, a pulse.
//!   Holding the breath with little of it left, the heart beats in both hands, faster and harder as
//!   it runs out (the game's own meter and gasp say so too): time to come up.

use glam::{Quat, Vec3};
use rustvr_ik::{HandOffsets, Side};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::rig::WorldMap;

/// The hands' velocity is smoothed over a few frames (seconds, e-folding): a controller's samples
/// jitter, a stroke doesn't.
const VELOCITY_TAU: f32 = 0.03;
/// Below this (m/s, against the head) a hand isn't stroking: it drifts, or reaches about.
const DEAD_ZONE: f32 = 0.1;
/// Through the water the strokes' way (a stroke's recovery reaching out again, or the hands held
/// still in a glide) a hand slices: only this share of its speed that way meets the water. Any
/// other way (pulled back, swept, the swimmer drifting off to a side) meets it fully.
const RECOVERY: f32 = 0.1;
/// The strokes' way is set by the strongest of them. A stroke at least `STEER[0]` as strong as the
/// recent strongest steers it (pushing about that way), `STEER[1]` turns it round (pushing against
/// it), and in between by how far it turns. So a recovery, even one as quick as its pull, doesn't
/// turn the swimmer round, and a stroke meant to does.
const STEER: [f32; 2] = [0.5, 1.3];
/// The recent strongest stroke fades by e over this long (seconds): after a pause, any stroke sets
/// the way.
const STRONGEST_FADE: f32 = 3.0;
/// The strokes' way turns to a new stroke's over this long (seconds, e-folding).
const TURN: f32 = 0.12;
/// A hand is in the water from its palm's last 2 cm above the surface to 4 cm under.
const WET: [f32; 2] = [-0.02, 0.04];
/// The palm's centre in the hand's canonical frame (`rustvr_ik`: +Y along the fingers).
const PALM: Vec3 = Vec3::new(0.0, 0.05, 0.0);
/// A hand stroking this fast (m/s past the dead zone) moves the water at the game's full swim
/// speed: the strokes are made as strong as the game's swimmer, so skill, attributes and load still
/// count. A firm stroke (a hand at 1.2 m/s) moves the water at about four times the game's speed,
/// and held, it drives the swimmer at two thirds to three quarters of that (the water slips past
/// the hands): a firm breaststroke surges to about two and a half times the game's speed, and glides
/// to about one between strokes.
const HAND_FULL: f32 = 0.28;
/// How hard the water pushes back on a hand moving through it (per metre: with the square of its
/// speed through the water, the swimmer's mass one). It decides how soon a stroke gets the swimmer
/// going: a firm one is at speed in about a quarter of a second.
const HAND_DRAG: f32 = 0.55;
/// The swimmer's own drag (per metre, with the square of their speed), a quarter of a hand's: a
/// stroke held drives the swimmer at two thirds of the speed its hand moves the water, or three
/// quarters with both hands. Gliding at speed, it slows them over a couple of metres.
const BODY_DRAG: f32 = 0.14;
/// The water's drag at walking speeds and less: the speed glides out by e over this long (seconds),
/// so a little stroke's move (a hand at 0.3 m/s: half a metre) stops soon after it.
const GLIDE: f32 = 2.0;
/// No stroke drives the swimmer past this many times the game's full swim speed.
const MAX_SPEED: f32 = 4.0;
/// Each frame's strokes are worked out in steps no longer than this (seconds): the hands' drag
/// changes fast with the speed it gives.
const STEP: f32 = 1.0 / 120.0;
/// At the surface (the head out of the water) a stroke must take the player down faster than
/// `SURFACE_HOLD[0]` m/s and `SURFACE_HOLD[1]` of their speed to dive (a stroke about 7 degrees
/// down): below it they stay at the surface. Every real stroke has a little up and down in it, and
/// the game floats a swimmer only once they stop: a long swim at the surface sank them.
const SURFACE_HOLD: [f32; 2] = [0.1, 0.12];
/// A grip squeezed past this is a fist (or a hold): it doesn't swim.
const FIST: f32 = 0.6;
/// A hand going into the water faster than this (m/s, downwards) slaps it: a pulse.
const SLAP: f32 = 0.8;
/// While a hand pushes, its buzz is renewed this often (seconds), each pulse a little longer.
const BUZZ_EVERY: f64 = 0.03;
/// Under the water with less breath than this left, the heart beats in the hands: a beat every
/// `HEART[1]` seconds at the warning's start, `HEART[2]` at none left.
const HEART: [f32; 3] = [0.3, 1.1, 0.5];

/// What the strokes did this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct SwimOut {
    /// The velocity the strokes give the player (tracking space, m/s), while swimming.
    pub velocity: Vec3,
    /// How hard the arms are working, 0..1.
    pub effort: f32,
    /// The game has the player swimming.
    pub swimming: bool,
    /// The game's full swim speed (m/s).
    pub full_speed: f32,
}

/// One hand's pulse for the backend: (hand, seconds, frequency, amplitude).
pub type Pulse = (usize, f32, f32, f32);

pub struct SwimInput<'a> {
    pub frame: &'a HmdFrame,
    pub water: Option<WaterState>,
    /// Tracking space -> the game's world now.
    pub map: Option<WorldMap>,
    /// Hands holding something, or otherwise busy with the rig (their pushes aren't strokes).
    pub busy: [bool; 2],
    pub paused: bool,
    /// The arms swim (the setting), and how much easier a stroke is (1: as designed).
    pub enabled: bool,
    pub ease: f32,
    pub now: f64,
}

#[derive(Default)]
pub struct Swim {
    offsets: Option<HandOffsets>,
    /// The swimming velocity (tracking space, m/s).
    velocity: Vec3,
    effort: f32,
    /// The strokes' drive (their way, and how strong, smoothed), its way, and the recent strongest
    /// stroke's strength (m/s of hand past the dead zone).
    drive: Vec3,
    way: Vec3,
    strongest: f32,
    /// Each hand's velocity against the head (smoothed), and where the hands and the head were last
    /// frame.
    hand_vel: [Vec3; 2],
    last: Option<([Vec3; 2], Vec3)>,
    wet: [bool; 2],
    buzzed: [f64; 2],
    pub pulses: Vec<Pulse>,
    said: bool,
    /// When the strokes were last logged.
    logged: f64,
    /// When the heart next beats (and the beat's second thump), while the breath runs low.
    beat: f64,
    thump: Option<(f64, f32)>,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Swim {
    pub fn update(&mut self, inp: &SwimInput) -> SwimOut {
        self.pulses.clear();
        let f = inp.frame;
        let dt = f.dt.clamp(1e-4, 0.1);
        let offsets = *self.offsets.get_or_insert_with(|| rustvr_ik::BodyParams::default().hand_offsets);
        let water = inp.water.filter(|w| w.flags & WATER_HERE != 0 && w.flags & WATER_WALKING == 0);
        let swimming = water.is_some_and(|w| w.flags & WATER_SWIMMING != 0);
        // (The game's speed is the world's; the strokes are tracking space's, a world scale apart.)
        let full = water.map_or(0.0, |w| w.swim_speed.max(0.0)) / inp.map.map_or(1.0, |m| m.world_per_meter);
        // The surface in tracking space (the mapping turns about the vertical only).
        let surface = match (water, inp.map) {
            (Some(w), Some(m)) => Some(m.origin.y + (w.surface - m.anchor.y) / m.world_per_meter),
            _ => None,
        };
        let head = Vec3::from_array(f.hmd.pos);
        let side = [Side::Left, Side::Right];
        let palms: [Option<Vec3>; 2] = std::array::from_fn(|i| {
            let c = &f.hands[i];
            (c.connected != 0 && c.grip.valid != 0).then(|| {
                let grip = rustvr_ik::Pose { pos: Vec3::from_array(c.grip.pos), rot: Quat::from_array(c.grip.rot).normalize() };
                let hand = offsets.hand_frame(side[i], grip);
                hand.pos + hand.rot * PALM
            })
        });
        // Each hand's velocity against the head: the head's own movement taken out, so walking
        // about the room pushes nothing. (Not its turning: looking about swings the hands round the
        // head, not through the water.)
        let pos = palms.map(|p| p.unwrap_or(head));
        if let Some((was, was_head)) = self.last {
            let head_vel = (head - was_head) / dt;
            let a = 1.0 - (-dt / VELOCITY_TAU).exp();
            for i in 0..2 {
                let raw = (pos[i] - was[i]) / dt - head_vel;
                let raw = if raw.is_finite() && palms[i].is_some() { raw } else { Vec3::ZERO };
                self.hand_vel[i] += (raw - self.hand_vel[i]) * a;
            }
        }
        self.last = Some((pos, head));

        self.heart(water, inp.now, inp.paused);
        if !swimming || !inp.enabled || inp.paused || full <= 0.0 {
            self.velocity = Vec3::ZERO;
            self.effort = 0.0;
            (self.drive, self.way, self.strongest) = (Vec3::ZERO, Vec3::ZERO, 0.0);
            self.wet = [false; 2];
            return SwimOut { swimming, full_speed: full, ..Default::default() };
        }
        if !self.said {
            self.said = true;
            tracing::info!("swim: the arms swim (the game's full speed {full:.2} m/s)");
        }
        let surface = surface.unwrap_or(f32::MAX);
        let scale = full / HAND_FULL * inp.ease.clamp(0.25, 4.0);
        // Each hand's stroke (against the head, past the dead zone), and how far it's in the water.
        let mut strokes = [(Vec3::ZERO, 0.0f32); 2];
        for i in 0..2 {
            let Some(palm) = palms[i] else { continue };
            let wet = smoothstep(WET[0], WET[1], surface - palm.y);
            let v = self.hand_vel[i];
            // A hand slapping into the water.
            let now_wet = wet > 0.5;
            if now_wet && !self.wet[i] && -v.y > SLAP {
                self.pulses.push((i, 0.04, 120.0, (-v.y / 3.0).clamp(0.3, 1.0)));
            }
            self.wet[i] = now_wet;
            if wet <= 0.0 || inp.busy[i] || f.hands[i].squeeze > FIST {
                continue;
            }
            let speed = v.length();
            let h = if speed > DEAD_ZONE { v * ((speed - DEAD_ZONE) / speed) } else { Vec3::ZERO };
            strokes[i] = (h, wet);
        }
        // The strokes' way: where they'd push the swimmer from still water (the hands' speeds
        // squared, as the water's push goes), the stronger strokes steering it.
        let push = strokes.iter().map(|&(h, wet)| -h * h.length() * wet).sum::<Vec3>();
        let strength = push.length().sqrt();
        let dir = push.normalize_or_zero();
        self.strongest *= (-dt / STRONGEST_FADE).exp();
        let agree = if self.way == Vec3::ZERO { 1.0 } else { dir.dot(self.way) };
        if strength > 0.05 && strength >= self.strongest * (STEER[0] + (STEER[1] - STEER[0]) * (1.0 - agree) / 2.0) {
            self.drive += (dir * strength - self.drive) * (1.0 - (-dt / TURN).exp());
            self.way = self.drive.normalize_or_zero();
            // (A stroke turning the way round is measured against the old way's strongest until it
            // has.)
            if dir.dot(self.way) > 0.0 {
                self.strongest = self.strongest.max(strength);
            }
        }
        // Each hand moves through the water at the swimmer's speed and its stroke's (made as strong
        // as the game's swimmer), and the water pushes back on it: the swimmer goes the other way.
        // The strokes' way it slices through; any other way it holds the swimmer back, which is what
        // stops them drifting off the way they went before. The swimmer's own drag glides their
        // speed out.
        let way = self.way;
        let through = |velocity: Vec3, h: Vec3| {
            let w = velocity + h * scale;
            w - way * w.dot(way).max(0.0) * (1.0 - RECOVERY)
        };
        let steps = (dt / STEP).ceil().clamp(1.0, 12.0);
        for _ in 0..steps as usize {
            let mut force = -self.velocity * (self.velocity.length() * BODY_DRAG + 1.0 / GLIDE);
            for &(h, wet) in &strokes {
                if wet > 0.0 {
                    let w = through(self.velocity, h);
                    force -= w * w.length() * HAND_DRAG * wet;
                }
            }
            self.velocity += force * (dt / steps);
        }
        self.velocity = self.velocity.clamp_length_max(full * MAX_SPEED);
        if !self.velocity.is_finite() {
            self.velocity = Vec3::ZERO;
        }
        // The water's resistance in a stroking hand.
        for (i, &(h, wet)) in strokes.iter().enumerate() {
            let w = through(self.velocity, h).length() * wet;
            if h != Vec3::ZERO && w > 0.3 && inp.now - self.buzzed[i] >= BUZZ_EVERY {
                self.buzzed[i] = inp.now;
                self.pulses.push((i, BUZZ_EVERY as f32 * 1.6, 70.0, 0.12 + 0.3 * (w / (3.0 * full)).min(1.0)));
            }
        }
        // (Pushing up with the head out of the water does nothing: the game holds the player at its
        // surface.)
        let mut velocity = self.velocity;
        if water.is_some_and(|w| w.flags & WATER_HEAD_UNDER == 0) {
            let flat = velocity.x.hypot(velocity.z);
            velocity.y = (velocity.y + SURFACE_HOLD[0] + SURFACE_HOLD[1] * flat).min(0.0);
        }
        // How hard the arms work: both hands stroking at 0.75 m/s past the dead zone is all they've got.
        let effort = (strokes.iter().map(|&(h, wet)| h.length() * wet).sum::<f32>() / 1.5).min(1.0);
        self.effort += (effort - self.effort) * (1.0 - (-dt / 0.25).exp());
        if inp.now - self.logged > 1.0 && (self.effort > 0.02 || self.velocity.length() > 0.05) {
            self.logged = inp.now;
            let depth = palms.map(|p| p.map(|p| surface - p.y));
            // (As the swimmer has them: ahead of the face, level, up, to the right.)
            let face = Quat::from_array(f.hmd.rot).normalize() * Vec3::Z;
            let ahead = Vec3::new(face.x, 0.0, face.z).normalize_or_zero();
            let right = Vec3::new(ahead.z, 0.0, -ahead.x);
            let felt = |v: Vec3| Vec3::new(v.dot(ahead), v.y, v.dot(right));
            tracing::info!(
                "swim: {:.2?} m/s ahead, up, right ({:.0}% of the game's {full:.2}), the strokes' way {:.2?} (the strongest {:.2} m/s), effort {:.2}; the hands {:.2?} m under, at {:.2?} m/s",
                felt(self.velocity),
                100.0 * self.velocity.length() / full,
                felt(self.way),
                self.strongest,
                self.effort,
                depth,
                self.hand_vel.map(felt),
            );
        }
        SwimOut { velocity, effort: self.effort, swimming, full_speed: full }
    }

    /// The heart in the hands while the breath runs low under the water: a beat (two thumps, the
    /// second softer) every second or so, faster and harder as it runs out.
    fn heart(&mut self, water: Option<WaterState>, now: f64, paused: bool) {
        if let Some((at, amp)) = self.thump
            && now >= at
        {
            self.thump = None;
            for h in 0..2 {
                self.pulses.push((h, 0.05, 55.0, amp));
            }
        }
        let low = water.filter(|w| w.flags & WATER_HEAD_UNDER != 0 && w.breath < HEART[0] && !paused);
        let Some(w) = low else {
            self.beat = 0.0;
            return;
        };
        if self.beat == 0.0 {
            tracing::info!("swim: breath {:.2} left under the water: the heart beats in the hands", w.breath);
        }
        let left = (w.breath / HEART[0]).clamp(0.0, 1.0);
        if now >= self.beat {
            let amp = 0.35 + 0.45 * (1.0 - left);
            for h in 0..2 {
                self.pulses.push((h, 0.06, 55.0, amp));
            }
            self.thump = Some((now + 0.15, amp * 0.6));
            self.beat = now + (HEART[2] + (HEART[1] - HEART[2]) * left) as f64;
        }
    }
}
