//! Swimming by the arms, for any game whose adapter says where its water is
//! (`rustvr_protocol::WaterState`). The game decides when the player swims; the runtime turns the
//! player's hands in its water into the velocity the game's swimming moves them at
//! (`Intents::swim`). Research and design: the Oblivion adapter's `docs/design/swimming.md`.
//!
//! A hand catches the water and the swimmer pulls themself past it, as in the well-liked VR swimmers
//! (Freediver: Triton Down, after Lone Echo's zero gravity): the grip squeezed with a hand under
//! the surface holds the water there, and pulling that hand carries the swimmer the other way, a few
//! times as fast as the hand goes. Let go and the swimmer glides on while the hand reaches out
//! again, free. That's how a swimmer feels the water: the hand anchored, the body pulled past it. A
//! controller can't show the palm that catches the water and the one that slices back through it,
//! so the grip says which (pure hand physics, with the recovery as quick as the pull, barely moved:
//! the playtests' builds 1 to 5).
//!
//! * Any stroke works, in any direction: a breaststroke or a crawl ahead, a pull up to dive, a push
//!   down to rise, to the side. The water's drag glides the swimmer out over several seconds.
//! * The hands holding the water make one stroke together: their velocities' sum over two (one
//!   hand alone pulls half as hard). A breaststroke's sweeps out and in cancel in it, leaving its
//!   pull back (a hand's own way, the sweep out, hid the pull in playtest 8). The stroke pulls only
//!   while it goes on away from where it started (or turned back), once 10 cm from there:
//!   squeezing the grip a little before the reach ends, or letting it go a little after the hands
//!   start back, slips, as the hands go back over their own stroke. (Without that, those hands
//!   moving forward threw the swimmer backwards: playtest 7.)
//! * Light strokes swim well: the pull's pace grows with the hands' speed less and less (to the 0.4th
//!   power), so they needn't push hard (playtest 10).
//! * A stroke never takes the glide's speed unless it's a push back against it (that's how to stop
//!   and go back): more push than sweep, the hands going that way at least half as far as they move
//!   apart or together. The sweep out of the next breaststroke goes a little forward, and slowed
//!   the swimmer at every stroke (playtest 9), but its hands spread wide; a push back goes with
//!   them together. (A stroke back had to be 20 cm long to count instead, and only slowed the
//!   swimmer: too hard to go back, playtest 11.) Otherwise a pull speeds the glide up to its pace
//!   along it and turns it its way, as a swimmer's streamlined body follows the stroke, so uneven
//!   hands don't leave the swimmer drifting sideways.
//! * The hands' velocity is measured against the head, so walking about the room pushes nothing.
//!   A hand holding something doesn't swim.
//! * The game's swim speed (skills, attributes, load) scales the pull, and the strokes may go up to
//!   a few times faster than the game's own swimmer.
//! * **Comfort**: the motion is the hands' own, pulled for; the strokes never turn the player. At
//!   the surface the game holds the player up (pushing up there does nothing), and a stroke's little
//!   drift down is held there too. Under water there's no buoyancy: a still swimmer hangs where they
//!   are, as the game has them.
//! * **Feel**: a hand catching the water gives a pulse, and buzzes as it pulls (not as it slips); a
//!   hand slapping into it, a pulse. Holding the breath with little of it left, the heart beats in
//!   both hands, faster and harder as it runs out (the game's own meter and gasp say so too): time to
//!   come up.

use glam::{Quat, Vec3};
use rustvr_ik::{HandOffsets, Side};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::rig::WorldMap;

/// The hands' velocity is smoothed over a few frames (seconds, e-folding): a controller's samples
/// jitter, a stroke doesn't.
const VELOCITY_TAU: f32 = 0.03;
/// A hand catches the water with the grip squeezed past `CATCH[0]`, and lets go below `CATCH[1]`.
const CATCH: [f32; 2] = [0.6, 0.35];
/// A pull with the hands at 1 m/s carries the swimmer this many of the game's full swim speeds, and
/// its pace grows with the hands' speed to the `PULL_CURVE` power: hands at 0.3 m/s still carry
/// them 1.9 game speeds, at 2 m/s 4. At the surface a light breaststroke (the hands a little out
/// and back) swims about 1.7 times the game's speed, a relaxed one (wide out, back and in over most
/// of a second) 2.6 times, a firm one 3 times, however the grip is timed.
const PULL: f32 = 3.0;
const PULL_CURVE: f32 = 0.4;
/// A stroke pulls once the hands are this far (m) from the point of the stroke they're furthest
/// from: a grip squeezed while the reach is still ending moves them less than that.
const STROKE_FROM: f32 = 0.1;
/// A stroke back against the glide (more than 120 degrees from it, while gliding faster than
/// `GLIDING` m/s) counts only while it's gone at least this share of how far the hands have moved
/// apart or together: a breaststroke's sweep out goes a little forward, its hands spreading wide.
const PUSH_NOT_SWEEP: f32 = 0.5;
const GLIDING: f32 = 1.0;
/// A stroke's points are kept this far apart (m), and no more of them than this.
const STROKE_STEP: f32 = 0.01;
const STROKE_POINTS: usize = 400;
/// How quickly a pull takes the swimmer up to its pace (seconds, e-folding): about as fast as the
/// hands, so the water holds them. And how quickly it turns the glide its way.
const CATCH_TAU: f32 = 0.08;
const STEER_TAU: f32 = 0.25;
/// The water's drag on the swimmer: their speed glides out by e over this long (seconds), and
/// sooner when fast (`DRAG` more for each m/s). From 5 m/s a swimmer is still at 3.8 m/s a second
/// on and 2.3 m/s three seconds on.
const GLIDE: f32 = 6.0;
const DRAG: f32 = 0.025;
/// No stroke drives the swimmer past this many times the game's full swim speed.
const MAX_SPEED: f32 = 4.0;
/// At the surface (the head out of the water) a stroke must take the player down faster than this
/// (m/s) to dive: below it they stay at the surface. Every real stroke has a little up and down in
/// it, and the game floats a swimmer only once they stop.
const SURFACE_HOLD: f32 = 0.2;
/// A hand is in the water with its palm this far under the surface (m).
const WET: f32 = 0.01;
/// The palm's centre in the hand's canonical frame (`rustvr_ik`: +Y along the fingers).
const PALM: Vec3 = Vec3::new(0.0, 0.05, 0.0);
/// A hand faster than this (m/s) is the tracking jumping, not an arm.
const JUMP: f32 = 10.0;
/// A hand going into the water faster than this (m/s, downwards) slaps it: a pulse.
const SLAP: f32 = 0.8;
/// While a hand pulls, its buzz is renewed this often (seconds), each pulse a little longer.
const BUZZ_EVERY: f64 = 0.03;
/// A hand pulling faster than this (m/s) buzzes with the water's resistance.
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
    /// The hands holding the water, and how far each has gone since it caught it; how far their
    /// stroke (their sum over two) has gone, and its points on the way; how fast it pulls.
    caught: [bool; 2],
    reach: [Vec3; 2],
    drawn: Vec3,
    stroke: Vec<Vec3>,
    pulling: f32,
    buzzed: [f64; 2],
    pub pulses: Vec<Pulse>,
    said: bool,
    /// When the strokes were last logged.
    logged: f64,
    /// When the heart next beats (and the beat's second thump), while the breath runs low.
    beat: f64,
    thump: Option<(f64, f32)>,
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
            self.caught = [false; 2];
            self.reach = [Vec3::ZERO; 2];
            self.drawn = Vec3::ZERO;
            self.stroke.clear();
            self.pulling = 0.0;
            return SwimOut { swimming, full_speed: full, ..Default::default() };
        }
        if !self.said {
            self.said = true;
            tracing::info!("swim: the arms swim (the game's full speed {full:.2} m/s)");
        }
        let surface = surface.unwrap_or(f32::MAX);
        let at_surface = water.is_some_and(|w| w.flags & WATER_HEAD_UNDER == 0);
        // The hands holding the water: under the surface, the grip squeezed, holding nothing else.
        let mut pull = Vec3::ZERO;
        let mut holding = 0;
        for i in 0..2 {
            let Some(palm) = palms[i] else {
                self.caught[i] = false;
                self.reach[i] = Vec3::ZERO;
                continue;
            };
            let wet = surface - palm.y > WET;
            let v = self.hand_vel[i];
            // A hand slapping into the water.
            if wet && !self.wet[i] && -v.y > SLAP {
                self.pulses.push((i, 0.04, 120.0, (-v.y / 3.0).clamp(0.3, 1.0)));
            }
            self.wet[i] = wet;
            let caught = wet && !inp.busy[i] && f.hands[i].squeeze > CATCH[self.caught[i] as usize];
            // The water caught in the hand.
            if caught && !self.caught[i] {
                self.pulses.push((i, 0.03, 90.0, 0.35));
            }
            self.caught[i] = caught;
            if caught {
                pull += v;
                holding += 1;
                self.reach[i] += v * dt;
            } else {
                self.reach[i] = Vec3::ZERO;
            }
        }
        // The hands holding the water stroke together: their velocities' sum over two. It pulls the
        // way it goes on away from the point of the stroke that's furthest from it; back over its
        // own stroke, it slips.
        self.pulling = 0.0;
        let mut stroking = 0.0;
        if holding == 0 {
            self.drawn = Vec3::ZERO;
            self.stroke.clear();
        } else {
            let both = pull / 2.0;
            self.drawn += both * dt;
            let at = self.drawn;
            if self.stroke.last().is_none_or(|p| p.distance(at) > STROKE_STEP) && self.stroke.len() < STROKE_POINTS {
                self.stroke.push(at);
            }
            let from = self.stroke.iter().copied().max_by(|a, b| a.distance_squared(at).total_cmp(&b.distance_squared(at))).unwrap_or(at);
            let gone = at - from;
            // It carries the swimmer the other way, at a pace that grows with its speed less and
            // less. (At the surface its rise goes into the surface.)
            let hands = both.length();
            let mut to = -both.normalize_or_zero() * PULL * hands.powf(PULL_CURVE) * full * inp.ease.clamp(0.25, 4.0);
            if at_surface {
                to.y = to.y.min(0.0);
            }
            let pace = to.length();
            let speed = self.velocity.length();
            let gliding = speed > GLIDING;
            let against = gliding && to.dot(self.velocity) < -0.5 * pace * speed;
            // (Against the glide, a push: a sweep's hands move apart or together more.)
            let push = !against || holding < 2 || gone.length() >= PUSH_NOT_SWEEP * (self.reach[0] - self.reach[1]).length();
            if pace > 1e-3 && gone.length() >= STROKE_FROM && push && both.dot(gone) > 0.0 {
                self.pulling = both.length();
                let way = to / pace;
                let catch = 1.0 - (-dt / CATCH_TAU).exp();
                if !gliding {
                    // From (nearly) still: the stroke's way, at its pace.
                    if self.velocity.dot(way) < pace {
                        self.velocity += (to - self.velocity) * catch;
                    }
                } else if against {
                    // Back against the glide: it slows the swimmer, as hard as it's strong against
                    // their speed (nearly still, it takes them its way).
                    self.velocity *= 1.0 - catch * (pace / speed).min(1.0);
                } else {
                    // Otherwise it never takes speed: it speeds the glide up to its pace along it
                    // (a pull slower than that slips), and turns it its way, as fast as it's strong
                    // against the swimmer's speed.
                    let dir = self.velocity / speed;
                    let ahead = to.dot(dir);
                    let speed = if ahead > speed { speed + (ahead - speed) * catch } else { speed };
                    let turn = (1.0 - (-dt / STEER_TAU).exp()) * (pace / speed).min(1.0);
                    self.velocity = (dir + (way - dir) * turn).try_normalize().unwrap_or(dir) * speed;
                }
                // The water's resistance in the hands pulling.
                for i in (0..2).filter(|&i| self.caught[i]) {
                    let speed = self.hand_vel[i].length();
                    stroking += speed;
                    if speed > BUZZ_FROM && inp.now - self.buzzed[i] >= BUZZ_EVERY {
                        self.buzzed[i] = inp.now;
                        self.pulses.push((i, BUZZ_EVERY as f32 * 1.6, 70.0, 0.12 + 0.3 * (speed / 2.0).min(1.0)));
                    }
                }
            }
        }
        // The water's drag glides them out.
        self.velocity -= self.velocity * (1.0 / GLIDE + DRAG * self.velocity.length()) * dt;
        self.velocity = self.velocity.clamp_length_max(full * MAX_SPEED);
        if !self.velocity.is_finite() {
            self.velocity = Vec3::ZERO;
        }
        // (Pushing up with the head out of the water does nothing: the game holds the player at its
        // surface, and a stroke's rise there goes into it, not on into the glide.)
        let mut velocity = self.velocity;
        if at_surface {
            self.velocity.y = self.velocity.y.min(0.0);
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
            tracing::info!(
                "swim: {:.2?} m/s ahead, up, right ({:.0}% of the game's {full:.2}), effort {:.2}; the hands {:.2?} m under, at {:.2?} m/s, holding the water {:?}, pulling at {:.2?} m/s",
                felt(self.velocity),
                100.0 * self.velocity.length() / full,
                self.effort,
                depth,
                self.hand_vel.map(felt),
                self.caught,
                self.pulling,
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
