//! Swimming by the arms, for any game whose adapter says where its water is
//! (`rustvr_protocol::WaterState`). The game decides when the player swims and how fast they may;
//! the runtime turns the player's strokes into the velocity the game's swimming moves them at
//! (`Intents::swim`). Research and design: the Oblivion adapter's `docs/research/swimming.md`.
//!
//! * **A hand pushes the water** when it's under the surface, empty (holding nothing, not
//!   gripping) and moving with its palm leading: how much goes with how squarely the palm meets its
//!   motion, so a hand brought back edge-on, or back of the hand first, pushes nothing, and any
//!   stroke works (a crawl, a breaststroke, a dog paddle, sculling, a push down to rise). Its
//!   velocity is measured against the head, so walking about the room or turning on the spot pushes
//!   nothing; under a dead zone (a hand reaching about, or held still) it pushes nothing either.
//! * **The strokes accelerate the player**, and the water's drag glides the speed out over about a
//!   second: each stroke surges, then coasts. A steady, firm stroke reaches the game's full swim
//!   speed, a gentle one about half, and nothing goes faster than the game's. So a slow swimmer in
//!   the game (low skill, heavy armour) moves slower for the same stroke.
//! * **Comfort**: the strokes only ever push; they never turn the player. At the surface the game
//!   holds the player up (pushing up there does nothing, and a stroke that takes them a little under
//!   is floated back up by the game); under water there's no buoyancy: a still swimmer hangs where
//!   they are, as the game has them.
//! * **Feel**: a pushing hand buzzes with the water's resistance; a hand slapping into it, a pulse.

use glam::{Quat, Vec3};
use rustvr_ik::{HandOffsets, Side};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::rig::WorldMap;

/// The hands' velocity is smoothed over a few frames (seconds, e-folding): a controller's samples
/// jitter, a stroke doesn't.
const VELOCITY_TAU: f32 = 0.03;
/// Below this (m/s, against the head) a hand isn't stroking.
const DEAD_ZONE: f32 = 0.25;
/// How squarely the palm has to lead its motion (the cosine between its normal and the motion) to
/// start pushing, and to push fully.
const PADDLE: [f32; 2] = [0.15, 0.6];
/// A hand is in the water from its palm's last 2 cm above the surface to 4 cm under.
const WET: [f32; 2] = [-0.02, 0.04];
/// The palm's centre in the hand's canonical frame (`rustvr_ik`: +Y along the fingers).
const PALM: Vec3 = Vec3::new(0.0, 0.05, 0.0);
/// The water's drag: the swimming speed glides out by e over this long (seconds). Long enough that
/// a steady stroke's surges blend into a swim rather than lurching, short enough to stop soon.
const GLIDE: f32 = 1.4;
/// The strokes' push (both hands', m/s past the dead zone, averaged over the stroke) that holds the
/// game's full swim speed against the drag.
const FULL_PUSH: f32 = 0.9;
/// A grip squeezed past this is a fist (or a hold): it doesn't swim.
const FIST: f32 = 0.6;
/// A hand going into the water faster than this (m/s, downwards) slaps it: a pulse.
const SLAP: f32 = 0.8;
/// While a hand pushes, its buzz is renewed this often (seconds), each pulse a little longer.
const BUZZ_EVERY: f64 = 0.03;

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
    /// Each hand's velocity against the head (smoothed), and where the hand, the head and its yaw
    /// were last frame.
    hand_vel: [Vec3; 2],
    last: Option<([Vec3; 2], Vec3, f32)>,
    wet: [bool; 2],
    buzzed: [f64; 2],
    pub pulses: Vec<Pulse>,
    said: bool,
    /// When the strokes were last logged.
    logged: f64,
}

fn yaw_of(q: Quat) -> f32 {
    let f = q * Vec3::Z;
    f.x.atan2(f.z)
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
        let head_rot = Quat::from_array(f.hmd.rot).normalize();
        let yaw = yaw_of(head_rot);
        let side = [Side::Left, Side::Right];
        let palms: [Option<(Vec3, Vec3)>; 2] = std::array::from_fn(|i| {
            let c = &f.hands[i];
            (c.connected != 0 && c.grip.valid != 0).then(|| {
                let grip = rustvr_ik::Pose { pos: Vec3::from_array(c.grip.pos), rot: Quat::from_array(c.grip.rot).normalize() };
                let hand = offsets.hand_frame(side[i], grip);
                // (The right palm faces the hand frame's +X, the left's -X.)
                let normal = hand.rot * if i == 0 { Vec3::NEG_X } else { Vec3::X };
                (hand.pos + hand.rot * PALM, normal)
            })
        });
        // Each hand's velocity against the head: the head's own movement, and the hand's going
        // round with the head's turning, taken out.
        let pos = palms.map(|p| p.map_or(head, |p| p.0));
        if let Some((was, was_head, was_yaw)) = self.last {
            let head_vel = (head - was_head) / dt;
            let turn = (yaw - was_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            let spin = Vec3::new(0.0, turn / dt, 0.0);
            let a = 1.0 - (-dt / VELOCITY_TAU).exp();
            for i in 0..2 {
                let raw = (pos[i] - was[i]) / dt - head_vel - spin.cross(pos[i] - head);
                let raw = if raw.is_finite() && palms[i].is_some() { raw } else { Vec3::ZERO };
                self.hand_vel[i] += (raw - self.hand_vel[i]) * a;
            }
        }
        self.last = Some((pos, head, yaw));

        if !swimming || !inp.enabled || inp.paused || full <= 0.0 {
            self.velocity = Vec3::ZERO;
            self.effort = 0.0;
            self.wet = [false; 2];
            return SwimOut { swimming, full_speed: full, ..Default::default() };
        }
        if !self.said {
            self.said = true;
            tracing::info!("swim: the arms swim (the game's full speed {full:.2} m/s)");
        }
        let surface = surface.unwrap_or(f32::MAX);
        let mut push = Vec3::ZERO;
        for i in 0..2 {
            let Some((palm, normal)) = palms[i] else { continue };
            let depth = surface - palm.y;
            let wet = smoothstep(WET[0], WET[1], depth);
            let v = self.hand_vel[i];
            // A hand slapping into the water.
            let now_wet = wet > 0.5;
            if now_wet && !self.wet[i] && -v.y > SLAP {
                self.pulses.push((i, 0.04, 120.0, (-v.y / 3.0).clamp(0.3, 1.0)));
            }
            self.wet[i] = now_wet;
            let fist = f.hands[i].squeeze > FIST;
            let speed = v.length();
            if wet <= 0.0 || inp.busy[i] || fist || speed <= DEAD_ZONE {
                continue;
            }
            let dir = v / speed;
            let paddle = smoothstep(PADDLE[0], PADDLE[1], dir.dot(normal));
            let p = -dir * (speed - DEAD_ZONE) * paddle * wet;
            push += p;
            // The water's resistance in the hand.
            let strength = p.length();
            if strength > 0.15 && inp.now - self.buzzed[i] >= BUZZ_EVERY {
                self.buzzed[i] = inp.now;
                self.pulses.push((i, BUZZ_EVERY as f32 * 1.6, 70.0, (0.12 + 0.3 * (strength / 1.5).min(1.0)) * paddle));
            }
        }
        // Strokes accelerate, the water's drag glides it out: at a steady push of FULL_PUSH the two
        // balance at the game's full speed.
        let drag = 1.0 / GLIDE;
        let gain = drag * full / (FULL_PUSH / inp.ease.clamp(0.25, 4.0));
        self.velocity += (push * gain - self.velocity * drag) * dt;
        // (Pushing up with the head out of the water does nothing: the game holds the player at its
        // surface, and brings them back to it after a stroke takes them a little under.)
        self.velocity = self.velocity.clamp_length_max(full);
        if !self.velocity.is_finite() {
            self.velocity = Vec3::ZERO;
        }
        let effort = (push.length() / FULL_PUSH * inp.ease).min(1.0);
        self.effort += (effort - self.effort) * (1.0 - (-dt / 0.25).exp());
        if inp.now - self.logged > 1.0 && (self.effort > 0.02 || self.velocity.length() > 0.05) {
            self.logged = inp.now;
            let depth = palms.map(|p| p.map(|p| surface - p.0.y));
            tracing::info!(
                "swim: {:.2?} m/s ({:.0}% of the game's {full:.2}), effort {:.2}; the hands {:.2?} m under, at {:.2?} m/s, palms {:.2?}; the surface at {surface:.2} in tracking space, the head at {:.2}",
                self.velocity,
                100.0 * self.velocity.length() / full,
                self.effort,
                depth,
                self.hand_vel,
                palms,
                head.y,
            );
        }
        SwimOut { velocity: self.velocity, effort: self.effort, swimming, full_speed: full }
    }
}
