//! Swimming by the arms, for any game whose adapter says where its water is
//! (`rustvr_protocol::WaterState`). The game decides when the player swims; the runtime turns the
//! player's hands in its water into the velocity the game's swimming moves them at
//! (`Intents::swim`). Research and design: the Oblivion adapter's `docs/research/swimming.md`.
//!
//! It's only the physics of hands in water. A hand under the surface pushes the water one way and
//! the swimmer the other, as hard as the square of its speed: a hand pushed hard and fast shoves the
//! swimmer along, one brought back slowly hardly holds them back, and one reaching forward again
//! fingertips first slices through the water. So swimming is pushing hard and coming back slowly or
//! slicing, in any direction, and every stroke (breaststroke, crawl, dog paddle, treading water)
//! works because that's how the water works, not as a gesture. The water's drag glides the
//! swimmer's speed out.
//!
//! * The hands' velocity is measured against the head, so walking about the room pushes nothing.
//!   A hand holding something doesn't swim.
//! * The game's swim speed (skills, attributes, load) scales the push, and the strokes may go up to
//!   a few times faster than the game's own swimmer.
//! * **Comfort**: the strokes only ever push; they never turn the player. At the surface the game
//!   holds the player up (pushing up there does nothing), and a stroke's little drift down is held
//!   there too. Under water there's no buoyancy: a still swimmer hangs where they are, as the game
//!   has them.
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
/// How hard the water pushes back on a hand: the swimmer's acceleration (in the game's full swim
/// speeds a second) for each (m/s)² of a hand's speed. Two hands pulled back at 1.2 m/s for a third
/// of a second, then reaching forward again at 0.4 m/s, surge the swimmer to about 2.4 times the
/// game's swim speed each stroke and glide to about its speed; one pull at 1 m/s carries them about
/// 5 m.
const STRENGTH: f32 = 1.5;
/// A hand moving the way its fingers point slices through the water: the water pushes back on that
/// part of its motion this much of fully. Pulled back wrist first it doesn't slice: a swimmer holds
/// the hand flat across a pull, which a hand round a controller can't show.
const SLICE: f32 = 0.1;
/// The water's drag on the swimmer: their speed glides out by e over this long (seconds), and
/// sooner when fast (`DRAG` more for each m/s). From 3 m/s a swimmer glides about 6 m, still at
/// 1.7 m/s a second on.
const GLIDE: f32 = 3.0;
const DRAG: f32 = 0.1;
/// No stroke drives the swimmer past this many times the game's full swim speed.
const MAX_SPEED: f32 = 4.0;
/// At the surface (the head out of the water) a stroke must take the player down faster than this
/// (m/s) to dive: below it they stay at the surface. Every real stroke has a little up and down in
/// it, and the game floats a swimmer only once they stop.
const SURFACE_HOLD: f32 = 0.2;
/// A hand is in the water from its palm's last 2 cm above the surface to 4 cm under.
const WET: [f32; 2] = [-0.02, 0.04];
/// The palm's centre in the hand's canonical frame (`rustvr_ik`: +Y along the fingers).
const PALM: Vec3 = Vec3::new(0.0, 0.05, 0.0);
/// A hand faster than this (m/s) is the tracking jumping, not an arm.
const JUMP: f32 = 10.0;
/// A hand going into the water faster than this (m/s, downwards) slaps it: a pulse.
const SLAP: f32 = 0.8;
/// While a hand pushes, its buzz is renewed this often (seconds), each pulse a little longer.
const BUZZ_EVERY: f64 = 0.03;
/// A hand moving faster than this (m/s) through the water buzzes with its resistance.
const BUZZ_FROM: f32 = 0.4;
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
    /// The arms swim (the setting), and how much stronger a stroke is (1: as designed).
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
    /// Each hand's velocity against the head (smoothed), and where the hands and the head were last
    /// frame.
    hand_vel: [Vec3; 2],
    last: Option<([Option<Vec3>; 2], Vec3)>,
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
        // Each hand's palm, and the way its fingers point.
        let hands: [Option<(Vec3, Vec3)>; 2] = std::array::from_fn(|i| {
            let c = &f.hands[i];
            (c.connected != 0 && c.grip.valid != 0).then(|| {
                let grip = rustvr_ik::Pose { pos: Vec3::from_array(c.grip.pos), rot: Quat::from_array(c.grip.rot).normalize() };
                let hand = offsets.hand_frame(side[i], grip);
                (hand.pos + hand.rot * PALM, hand.rot * Vec3::Y)
            })
        });
        let palms = hands.map(|h| h.map(|(palm, _)| palm));
        // Each hand's velocity against the head: the head's own movement taken out, so walking
        // about the room pushes nothing. (Not its turning: looking about swings the hands round the
        // head, not through the water.) Only a hand tracked last frame too has moved: one coming
        // back into tracking has only jumped there.
        if let Some((was, was_head)) = self.last {
            let head_vel = (head - was_head) / dt;
            let a = 1.0 - (-dt / VELOCITY_TAU).exp();
            for i in 0..2 {
                let raw = match (palms[i], was[i]) {
                    (Some(p), Some(w)) => (p - w) / dt - head_vel,
                    _ => Vec3::ZERO,
                };
                if raw.is_finite() && raw.length() < JUMP {
                    self.hand_vel[i] += (raw - self.hand_vel[i]) * a;
                }
            }
        }
        self.last = Some((palms, head));

        self.heart(water, inp.now, inp.paused);
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
        // Each hand in the water pushes the swimmer the other way, as hard as the square of its
        // speed; the water's drag glides their speed out.
        let strength = STRENGTH * full * inp.ease.clamp(0.25, 4.0);
        let mut accel = -self.velocity * (1.0 / GLIDE + DRAG * self.velocity.length());
        let mut stroking = 0.0;
        for i in 0..2 {
            let Some((palm, fingers)) = hands[i] else { continue };
            let wet = smoothstep(WET[0], WET[1], surface - palm.y);
            let v = self.hand_vel[i];
            // A hand slapping into the water.
            let now_wet = wet > 0.5;
            if now_wet && !self.wet[i] && -v.y > SLAP {
                self.pulses.push((i, 0.04, 120.0, (-v.y / 3.0).clamp(0.3, 1.0)));
            }
            self.wet[i] = now_wet;
            if wet <= 0.0 || inp.busy[i] {
                continue;
            }
            // Fingertips first, a hand slices through the water: only the rest of its motion, across
            // the hand, meets it fully.
            let along = v.dot(fingers);
            let push = if along > 0.0 {
                let across = v - fingers * along;
                across * across.length() + fingers * along * along * SLICE
            } else {
                v * v.length()
            };
            accel -= push * strength * wet;
            // The water's resistance in the hand (as fast as a hand meeting it fully would go).
            let speed = push.length().sqrt() * wet;
            stroking += speed;
            if speed > BUZZ_FROM && inp.now - self.buzzed[i] >= BUZZ_EVERY {
                self.buzzed[i] = inp.now;
                self.pulses.push((i, BUZZ_EVERY as f32 * 1.6, 70.0, 0.12 + 0.3 * (speed / 2.0).min(1.0)));
            }
        }
        self.velocity = (self.velocity + accel * dt).clamp_length_max(full * MAX_SPEED);
        if !self.velocity.is_finite() {
            self.velocity = Vec3::ZERO;
        }
        // (Pushing up with the head out of the water does nothing: the game holds the player at its
        // surface.)
        let mut velocity = self.velocity;
        if water.is_some_and(|w| w.flags & WATER_HEAD_UNDER == 0) {
            velocity.y = (velocity.y + SURFACE_HOLD).min(0.0);
        }
        // How hard the arms work: both hands at 0.75 m/s is all they've got.
        let effort = (stroking / 1.5).min(1.0);
        self.effort += (effort - self.effort) * (1.0 - (-dt / 0.25).exp());
        if inp.now - self.logged > 1.0 && (self.effort > 0.02 || self.velocity.length() > 0.05) {
            self.logged = inp.now;
            let depth = palms.map(|p| p.map(|p| surface - p.y));
            // (As the swimmer has them: ahead of the face, level, up, to the right.)
            let face = Quat::from_array(f.hmd.rot).normalize() * Vec3::Z;
            let ahead = Vec3::new(face.x, 0.0, face.z).normalize_or_zero();
            let right = Vec3::new(ahead.z, 0.0, -ahead.x);
            let felt = |v: Vec3| Vec3::new(v.dot(ahead), v.y, v.dot(right));
            // (How much of each hand's motion goes the way its fingers point: 1 slicing, -1 wrist first.)
            let first: [Option<f32>; 2] = std::array::from_fn(|i| hands[i].map(|(_, f)| self.hand_vel[i].normalize_or_zero().dot(f)));
            tracing::info!(
                "swim: {:.2?} m/s ahead, up, right ({:.0}% of the game's {full:.2}), effort {:.2}; the hands {:.2?} m under, at {:.2?} m/s, fingertips first {:.2?}",
                felt(self.velocity),
                100.0 * self.velocity.length() / full,
                self.effort,
                depth,
                self.hand_vel.map(felt),
                first,
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
