//! The desktop view's bob (`settings::DesktopBob`): its steadied camera (`crate::spectator`) bobs
//! as a flat game's does while the player is moved by the stick or the game, so a recording looks
//! like walking and not a floating camera. The headset's view never bobs: comfort comes first, and
//! the player's real head moves as it does. Walking in the room adds none either: the real head's
//! own movement is in the picture already (`Locomotion::walk` leaves the room's steps out).
//!
//! Two bobs, as the player chooses:
//! - **The game's own** (`rustvr_protocol::ViewBob`), as its adapter gives it: an offset and a turn
//!   in the view's own frame, shown as the game has it (Oblivion's: its first-person animations
//!   moving its camera node). Where the game gives none, the runtime's.
//! - **The runtime's own**, the same for every game, made the way good flat games make theirs (the
//!   bob tied to the footsteps, not a sine wave on a timer) and spectator cameras keep theirs
//!   readable (small, eased, the horizon level):
//!   - steps at the walk's own pace: people take longer and quicker steps the faster they go;
//!   - the view drops at each footfall and rises over the foot, the drop sharper the faster (a
//!     runner lands on each step, a walker rolls over it), and sways a little over each foot in
//!     turn, with a slight nod down as it drops;
//!   - all of it as big as the speed makes it, eased in and out as the walk starts and stops (the
//!     steps keep their pace as they fade, rather than freezing mid-step);
//!   - a soft dip on landing from a fall or a jump, as hard as the landing, on a spring;
//!   - smaller and slower while crouched; in water no steps, a slow float with the strokes.
//!
//! Either is damped while the view is narrowed for aiming, as a flat game's view steadies down its
//! sights.

use glam::{Quat, Vec3};
use rustvr_protocol::{BOB_GIVEN, Pose, ViewBob};

use crate::rig::WorldMap;
use crate::settings::{BobMode, DesktopBob};

/// Slower than this (metres a second), no steps; from `FULL_STEPS` on, steps as big as the speed
/// makes them. (A drift on the stick's edge doesn't bob.)
const STILL: f32 = 0.15;
const FULL_STEPS: f32 = 0.9;
/// How long the speed takes to ease (seconds, e-folding): the steps fade in and out with it.
const SPEED_EASE: f32 = 0.18;
/// How long crouching, swimming, a change between the game's bob and the runtime's, and the bob
/// turned on or off take to ease (seconds, e-folding).
const EASE: f32 = 0.25;
/// From walking to running (metres a second): the footfall's drop goes from a walker's roll to a
/// runner's landing, and the sway narrows.
const RUN_FROM: f32 = 1.8;
const RUN_TO: f32 = 3.6;
/// The sway over each foot against the drop and rise: a walker's, a runner's (runners sway less).
const SWAY_WALK: f32 = 0.5;
const SWAY_RUN: f32 = 0.3;
/// The nod down as the view drops (radians a metre: a quarter of a degree a centimetre).
const NOD: f32 = 0.45;
/// Crouched: steps this much of their size, this much of their pace.
const CROUCH_SIZE: f32 = 0.6;
const CROUCH_PACE: f32 = 0.85;
/// Swimming: a float this much of a step's size, at this many a second (a stroke's pace).
const SWIM_SIZE: f32 = 0.4;
const SWIM_PACE: f32 = 0.65;
/// Falling faster than this (metres a second) is off the ground (walking down stairs or a slope is
/// slower); slowed to this from it, a landing.
const FALLING: f32 = -2.2;
const LANDED: f32 = -0.6;
/// A landing pushes the view down at this share of how fast it fell, as measured (over the last
/// `RISE_OVER`: a jump landing at 4 m/s measures about 3; 0.6 m/s, about 2.5 cm down), and no
/// harder than `DIP_MOST` (about 4 cm).
const DIP_PER_FALL: f32 = 0.2;
const DIP_MOST: f32 = 1.0;
/// The dip's spring: how fast it swings back (radians a second: the bottom 0.1 s after landing) and
/// how damped (1: it doesn't come back past where it was; a little under: a soft give).
const DIP_SPRING: f32 = 11.0;
const DIP_DAMPING: f32 = 0.7;
/// How fast the vertical speed eases (seconds, e-folding).
const RISE_EASE: f32 = 0.04;
/// The walk is measured from the game's own steps over this long (seconds), up and down over the
/// shorter `RISE_OVER` (a landing is sudden). Not step by step: a game's step and the time it's
/// stamped with can be a frame apart (the range's are), and one step's walk is then off many times
/// over (5 to 70 m/s, walking at 1.4); over a quarter of a second it's off by a few per cent.
const WALK_OVER: f64 = 0.25;
const RISE_OVER: f64 = 0.1;
/// The game's steps kept to measure it by (at 120 a second, a quarter of a second's).
const KEPT: usize = 32;
/// A step further than this (metres) is the mapping jumping (a teleport, a recentre), not a walk.
const JUMP: f32 = 1.0;
/// No new step for this long (seconds): the game has stopped (paused, loading), and so has the walk.
const STOPPED: f32 = 0.25;

/// What moves the player this frame.
#[derive(Clone, Copy, Debug)]
pub struct BobInput {
    /// The game's newest step: when its clock stood there (seconds: [`seconds`]) and where it has
    /// the player (tracking space into its world). The walk by the stick or the game is measured
    /// from these: where tracking's origin goes in the world, which the room's own steps don't move.
    pub step: Option<(f64, WorldMap)>,
    pub crouched: bool,
    pub swimming: bool,
    /// The game's own bob this frame (`GameState::view_bob`).
    pub game: ViewBob,
    /// The game's bob is in the headset's picture already (the player keeps it there:
    /// `Comfort::head_bob`), so the desktop view, which shows that picture, has it too.
    pub in_picture: bool,
    /// The desktop view's field of view against its setting (1: as set; less while it's narrowed
    /// for aiming).
    pub narrowed: f32,
}

/// The bob this frame, to move the desktop view's camera by ([`Bobbed::apply`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bobbed {
    /// Where it moves the camera (metres): in the camera's level frame (x right, y straight up, z
    /// forward: the runtime's own), and in its own frame (the game's, as the game's camera has it).
    pub level: Vec3,
    pub view: Vec3,
    /// How it turns the camera after where it looks.
    pub rot: Quat,
}

impl Default for Bobbed {
    fn default() -> Self {
        Bobbed { level: Vec3::ZERO, view: Vec3::ZERO, rot: Quat::IDENTITY }
    }
}

impl Bobbed {
    /// The camera `cam` (tracking space) moved and turned by the bob.
    pub fn apply(&self, cam: Pose) -> Pose {
        let rot = Quat::from_array(cam.rot);
        // (Its level frame from its right, which stays level with the horizon held level, and still
        // gives a heading looking straight up or down.)
        let right = rot * Vec3::X;
        let level = Quat::from_rotation_y((-right.z).atan2(right.x));
        let pos = Vec3::from_array(cam.pos) + level * self.level + rot * self.view;
        Pose { pos: pos.to_array(), rot: (rot * self.rot).normalize().to_array(), ..cam }
    }
}

/// The desktop view's bob, frame to frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bob {
    /// The game's last steps (`KEPT`, the newest at `newest`), how long since a new one came
    /// (seconds), and the walk measured from them (metres a second, the world's axes).
    kept: [Option<(f64, WorldMap)>; KEPT],
    newest: usize,
    since_step: f32,
    walk: Vec3,
    /// The walk's speed, eased (metres a second), and the steps taken (one for each foot, counted
    /// over a stride of two).
    speed: f32,
    steps: f32,
    /// How far crouched, and how far swimming, eased (0..1).
    crouch: f32,
    swim: f32,
    /// The vertical speed, eased (metres a second, up positive), and the fastest fall since the
    /// player left the ground (0 on it).
    rise: f32,
    fall: f32,
    /// The landing's dip, on its spring: how far down (metres, down negative) and how fast.
    dip: f32,
    dip_vel: f32,
    /// How far the game's own bob is shown rather than the runtime's (0..1, eased across a change),
    /// and how far the bob is on at all (0..1: off fades it out).
    game: f32,
    on: f32,
    /// What's shown, as last said in the log; and since it was last said how it bobbed (seconds,
    /// lowest and highest, furthest to the side, metres; the fastest walk and its steps a second).
    said: Option<Shown>,
    noted: f32,
    low: f32,
    high: f32,
    side: f32,
    fastest: (f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shown {
    None,
    Runtimes,
    Games,
    InPicture,
}

impl Bob {
    /// This frame's bob, `dt` seconds after the last.
    pub fn update(&mut self, i: &BobInput, dt: f32, s: &DesktopBob) -> Bobbed {
        let dt = dt.clamp(0.0, 0.1);
        let ease = |tau: f32| 1.0 - (-dt / tau).exp();
        let given = i.game.flags & BOB_GIVEN != 0;
        let shown = match s.mode {
            BobMode::Off => Shown::None,
            BobMode::Game if given && i.in_picture => Shown::InPicture,
            BobMode::Game if given => Shown::Games,
            // (The runtime's own on a picture that bobs with the game's already would bob twice.)
            _ if given && i.in_picture => Shown::InPicture,
            _ => Shown::Runtimes,
        };
        if self.said != Some(shown) {
            self.said = Some(shown);
            tracing::info!(
                "desktop view: {}",
                match shown {
                    Shown::None => "no bob",
                    Shown::Runtimes => "the runtime's own bob as the player walks",
                    Shown::Games => "the game's own bob",
                    Shown::InPicture => "no bob of its own: the game's is in the headset's picture already (Comfort: camera bob)",
                }
            );
        }
        self.on += (if matches!(shown, Shown::Runtimes | Shown::Games) { 1.0 } else { 0.0 } - self.on) * ease(EASE);
        self.game += (if shown == Shown::Games { 1.0 } else { 0.0 } - self.game) * ease(EASE);

        // ---- the runtime's own: steps at the walk's pace
        let walk = self.measure(i.step, dt);
        let flat = Vec3::new(walk.x, 0.0, walk.z).length();
        self.speed += (flat - self.speed) * ease(SPEED_EASE);
        self.crouch += (i.crouched as u8 as f32 - self.crouch) * ease(EASE);
        self.swim += (i.swimming as u8 as f32 - self.swim) * ease(EASE);
        let v = self.speed;
        let pace = mix(steps_per_second(v) * mix(1.0, CROUCH_PACE, self.crouch), SWIM_PACE, self.swim);
        self.steps = (self.steps + pace * dt).rem_euclid(2.0);
        let size = drop(v) * smoothstep(STILL, FULL_STEPS, v) * mix(1.0, CROUCH_SIZE, self.crouch) * mix(1.0, SWIM_SIZE, self.swim);
        let run = smoothstep(RUN_FROM, RUN_TO, v) * (1.0 - self.swim);
        let p = self.steps.fract();
        // Lowest at each footfall (p = 0), highest over the foot (p = 0.5): a walker's even roll, a
        // runner's sharp landing and round flight.
        let shape = mix(-(std::f32::consts::TAU * p).cos(), 2.0 * (std::f32::consts::PI * p).sin().abs() - 1.0, run);
        let up = size * shape;
        // Over each foot in turn at the top of its step (a stride: two steps); none in water.
        let sway = size * mix(SWAY_WALK, SWAY_RUN, run) * (1.0 - self.swim) * (std::f32::consts::PI * self.steps).sin();

        // ---- a landing's dip
        self.rise += (walk.y - self.rise) * ease(RISE_EASE);
        if self.rise < FALLING && self.swim < 0.5 {
            self.fall = self.fall.min(self.rise);
        } else if self.fall < 0.0 && self.rise > LANDED {
            let push = (-self.fall * DIP_PER_FALL).min(DIP_MOST);
            self.dip_vel -= push;
            tracing::info!("desktop view: a landing at {:.1} m/s, the view dips (pushed {push:.2} m/s)", -self.fall);
            self.fall = 0.0;
        }
        // (In small steps: the spring stays steady however long the frame.)
        let n = (dt / 0.01).ceil().max(1.0);
        for _ in 0..n as u32 {
            let h = dt / n;
            self.dip_vel += (-DIP_SPRING * DIP_SPRING * self.dip - 2.0 * DIP_DAMPING * DIP_SPRING * self.dip_vel) * h;
            self.dip += self.dip_vel * h;
        }

        // ---- shown: the game's or the runtime's, as strong as set, damped while aiming
        let k = self.on * s.strength.max(0.0) * i.narrowed.clamp(0.0, 1.0).powi(3);
        let ours = (1.0 - self.game) * k;
        let theirs = self.game * k;
        let drop_all = up + self.dip;
        let game_rot = Quat::from_array(i.game.rot);
        let game_turn = if game_rot.length_squared() > 1e-6 {
            let (axis, angle) = game_rot.normalize().to_axis_angle();
            Quat::from_axis_angle(axis, angle * theirs)
        } else {
            Quat::IDENTITY
        };
        let out = Bobbed {
            level: Vec3::new(sway, drop_all, 0.0) * ours,
            view: Vec3::from_array(i.game.offset) * theirs,
            rot: Quat::from_rotation_x(-drop_all * NOD * ours) * game_turn,
        };
        self.note(&out, shown, pace, dt);
        out
    }

    /// The walk by the stick or the game from the game's steps, `step` the newest: where tracking's
    /// origin went in the world over the last `WALK_OVER` (up and down over `RISE_OVER`), metres a
    /// second. (Locomotion's formula, over more than one step.)
    fn measure(&mut self, step: Option<(f64, WorldMap)>, dt: f32) -> Vec3 {
        self.since_step += dt;
        let Some((t, now)) = step else {
            self.kept = Default::default();
            return Vec3::ZERO;
        };
        let moved = |then: &WorldMap| (now.anchor - then.point(now.origin)) / now.world_per_meter.max(1e-6);
        match self.kept[self.newest] {
            Some((last, _)) if t <= last => {}
            Some((_, then)) if moved(&then).length() > JUMP || then.world_per_meter != now.world_per_meter => {
                self.kept = Default::default();
                self.kept[self.newest] = Some((t, now));
                self.since_step = 0.0;
                self.walk = Vec3::ZERO;
            }
            _ => {
                self.newest = (self.newest + 1) % KEPT;
                self.kept[self.newest] = Some((t, now));
                self.since_step = 0.0;
                // The newest step against the latest kept at least `over` before it (or the oldest).
                let over = |over: f64| {
                    let then = (1..KEPT).map(|k| self.kept[(self.newest + KEPT - k) % KEPT]).take_while(Option::is_some).flatten().find(|&(at, _)| t - at >= over);
                    let then = then.or_else(|| (1..KEPT).rev().find_map(|k| self.kept[(self.newest + KEPT - k) % KEPT]));
                    then.filter(|&(at, _)| t > at).map_or(Vec3::ZERO, |(at, m)| moved(&m) / (t - at) as f32)
                };
                let (flat, rise) = (over(WALK_OVER), over(RISE_OVER));
                self.walk = Vec3::new(flat.x, rise.y, flat.z);
            }
        }
        if self.since_step > STOPPED {
            self.walk = Vec3::ZERO;
        }
        self.walk
    }

    /// Says in the log every five seconds how it bobbed (while it did).
    fn note(&mut self, out: &Bobbed, shown: Shown, pace: f32, dt: f32) {
        let moved = out.level + out.view;
        self.low = self.low.min(moved.y);
        self.high = self.high.max(moved.y);
        self.side = self.side.max(moved.x.abs());
        if self.speed > self.fastest.0 {
            self.fastest = (self.speed, pace);
        }
        self.noted += dt;
        if self.noted < 5.0 {
            return;
        }
        if self.high - self.low > 1e-3 {
            let whose = match shown {
                Shown::Games => "the game's own".to_owned(),
                _ => format!("the runtime's: at most {:.1} m/s, {:.1} steps a second", self.fastest.0, self.fastest.1),
            };
            tracing::info!("desktop view: bobbed {:.1} to {:.1} cm up, {:.1} cm to the side in {:.0} s ({whose})", self.low * 100.0, self.high * 100.0, self.side * 100.0, self.noted);
        }
        (self.noted, self.low, self.high, self.side, self.fastest) = (0.0, 0.0, 0.0, 0.0, (0.0, 0.0));
    }
}

/// A game's step's time (`GameState::time_qpc`, the performance counter's ticks) in seconds.
pub fn seconds(qpc: i64) -> f64 {
    static FREQ: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut f);
        }
        f.max(1) as f64
    });
    qpc as f64 / freq
}

/// Steps a second at a walk's speed (metres a second), as people walk: 1.5 at a slow 0.6 m/s, 2 at
/// an easy 1.4, 2.4 at a brisk 2.2 (walkers' footfalls measured on a treadmill), and a runner's 3 at
/// most. (Quicker and longer steps the faster: a bob on a timer of its own, as Quake's and
/// Half-Life's, slides under the feet.)
fn steps_per_second(speed: f32) -> f32 {
    (1.16 + 0.56 * speed).min(3.0)
}

/// How far the view drops and rises either side of its middle with each step at a speed (metres a
/// second): 0.8 cm at a walk, 1.8 at a run, 2.2 at most. A walker's body rises and falls 2 to 5 cm
/// a step, more the faster; on a screen a camera moving as much reads as a jolt, so about half of it.
fn drop(speed: f32) -> f32 {
    (0.002 + 0.004 * speed).min(0.022)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 90.0;

    /// The player as the game has them: walked at `vel` (metres a second), a step a frame.
    #[derive(Default)]
    struct Player {
        t: f64,
        at: Vec3,
    }

    impl Player {
        fn step(&mut self, vel: Vec3) -> BobInput {
            self.t += DT as f64;
            self.at += vel * DT;
            let map = WorldMap { origin: Vec3::new(0.0, 1.7, 0.0), body_yaw: 0.0, world_per_meter: 1.0, anchor: self.at + Vec3::Y * 1.7 };
            BobInput { step: Some((self.t, map)), crouched: false, swimming: false, game: ViewBob::default(), in_picture: false, narrowed: 1.0 }
        }
    }

    fn generic() -> DesktopBob {
        DesktopBob { mode: BobMode::Generic, strength: 1.0 }
    }

    /// The heights over `seconds` walked at `vel`, after a second to settle; `with` changes each
    /// frame's input.
    fn heights(b: &mut Bob, p: &mut Player, vel: Vec3, s: &DesktopBob, seconds: f32, with: impl Fn(&mut BobInput)) -> Vec<f32> {
        let frame = |b: &mut Bob, p: &mut Player| {
            let mut i = p.step(vel);
            with(&mut i);
            b.update(&i, DT, s).level.y
        };
        for _ in 0..90 {
            frame(b, p);
        }
        (0..(seconds / DT) as usize).map(|_| frame(b, p)).collect()
    }

    fn walked(speed: f32, s: &DesktopBob, seconds: f32) -> Vec<f32> {
        heights(&mut Bob::default(), &mut Player::default(), Vec3::Z * speed, s, seconds, |_| {})
    }

    fn highest(h: &[f32]) -> f32 {
        h.iter().cloned().fold(f32::MIN, f32::max)
    }

    #[test]
    fn a_walk_bobs_at_its_steps_pace_and_a_run_harder_and_quicker() {
        let s = generic();
        let lows = |h: &[f32]| h.windows(3).filter(|w| w[1] < w[0] && w[1] <= w[2]).count();
        let span = |h: &[f32]| highest(h) - h.iter().cloned().fold(f32::MAX, f32::min);
        let (walk, run) = (walked(1.4, &s, 4.0), walked(4.0, &s, 4.0));
        // About 2 footfalls a second walking, 3 running.
        assert!((7..=8).contains(&lows(&walk)), "{} footfalls walking", lows(&walk));
        assert!((11..=13).contains(&lows(&run)), "{} footfalls running", lows(&run));
        assert!((span(&walk) - 0.016).abs() < 0.002, "walking {:.4} m top to bottom", span(&walk));
        assert!((span(&run) - 0.036).abs() < 0.003, "running {:.4} m top to bottom", span(&run));
    }

    /// The range's steps: each moved a frame's walk, stamped as much as 8 ms off when it was.
    #[test]
    fn the_walk_is_measured_however_unevenly_the_steps_are_stamped() {
        let mut b = Bob::default();
        let mut p = Player::default();
        for k in 0..360 {
            let mut i = p.step(Vec3::Z * 1.4);
            if let Some((t, _)) = &mut i.step {
                *t += [0.0, 0.008, -0.004, 0.006][k % 4];
            }
            b.update(&i, DT, &generic());
            if k > 90 {
                assert!((b.speed - 1.4).abs() < 0.1, "{:.2} m/s at {k}", b.speed);
            }
        }
    }

    #[test]
    fn standing_still_and_off_are_still_and_a_stop_fades_out() {
        assert!(walked(0.0, &generic(), 1.0).iter().all(|h| h.abs() < 1e-6));
        assert!(walked(1.4, &DesktopBob { mode: BobMode::Off, strength: 1.0 }, 1.0).iter().all(|h| h.abs() < 1e-4));
        // Walking, then stopped: the bob fades rather than stopping dead.
        let (mut b, mut p) = (Bob::default(), Player::default());
        heights(&mut b, &mut p, Vec3::Z * 1.4, &generic(), 1.0, |_| {});
        let after: Vec<f32> = (0..90).map(|_| b.update(&p.step(Vec3::ZERO), DT, &generic()).level.y.abs()).collect();
        assert!(after[..10].iter().cloned().fold(0.0, f32::max) > 1e-3, "still bobbing just after");
        assert!(after[80..].iter().all(|&h| h < 2e-4), "settled a second after");
        // A game that stops stepping (paused): its walk stops too.
        heights(&mut b, &mut p, Vec3::Z * 1.4, &generic(), 1.0, |_| {});
        let paused = p.step(Vec3::Z * 1.4);
        for _ in 0..90 {
            b.update(&paused, DT, &generic());
        }
        assert!(b.speed < 0.05, "{} m/s paused", b.speed);
    }

    #[test]
    fn a_landing_dips_and_comes_back() {
        let s = generic();
        let (mut b, mut p) = (Bob::default(), Player::default());
        heights(&mut b, &mut p, Vec3::ZERO, &s, 0.5, |_| {});
        for _ in 0..30 {
            b.update(&p.step(Vec3::Y * -4.0), DT, &s);
        }
        let after: Vec<f32> = (0..60).map(|_| b.update(&p.step(Vec3::ZERO), DT, &s).level.y).collect();
        let lowest = after.iter().cloned().fold(f32::MAX, f32::min);
        assert!((-0.045..-0.02).contains(&lowest), "dips {lowest:.3} m");
        assert!(after.last().unwrap().abs() < 0.003, "back by {:.3}", after.last().unwrap());
        // Walking down a slope isn't a fall.
        let (mut b, mut p) = (Bob::default(), Player::default());
        heights(&mut b, &mut p, Vec3::new(0.0, -1.0, 1.4), &s, 1.0, |_| {});
        let h = heights(&mut b, &mut p, Vec3::Z * 1.4, &s, 1.0, |_| {});
        assert!(h.iter().all(|h| h.abs() < 0.009));
    }

    #[test]
    fn the_games_own_shows_as_given_unless_the_picture_has_it() {
        let s = DesktopBob { mode: BobMode::Game, strength: 1.0 };
        let given = ViewBob { flags: BOB_GIVEN, offset: [0.0, -0.01, 0.0], rot: [0.0, 0.0, 0.0, 1.0] };
        let (mut b, mut p) = (Bob::default(), Player::default());
        let mut frame = |in_picture: bool| {
            let mut i = p.step(Vec3::Z * 1.4);
            (i.game, i.in_picture) = (given, in_picture);
            b.update(&i, DT, &s)
        };
        for _ in 0..180 {
            frame(false);
        }
        let shown = frame(false);
        assert!((shown.view.y + 0.01).abs() < 1e-4 && shown.level.length() < 1e-4, "{shown:?}");
        // Kept in the headset (and so in the picture): nothing added.
        for _ in 0..180 {
            frame(true);
        }
        let shown = frame(true);
        assert!(shown.view.length() < 1e-4 && shown.level.length() < 1e-4);
        // No game's bob given: the runtime's.
        assert!(highest(&walked(1.4, &s, 2.0)) > 0.005);
    }

    #[test]
    fn aiming_damps_it() {
        let s = generic();
        let free = highest(&walked(1.4, &s, 2.0));
        let aimed = highest(&heights(&mut Bob::default(), &mut Player::default(), Vec3::Z * 1.4, &s, 2.0, |i| i.narrowed = 65.0 / 105.0));
        assert!((aimed / free - 0.237).abs() < 0.02, "{aimed} against {free}");
    }

    #[test]
    fn the_bob_moves_the_camera_level_and_turns_it_after_its_look() {
        let b = Bobbed { level: Vec3::new(0.01, -0.02, 0.0), view: Vec3::ZERO, rot: Quat::from_rotation_x(0.01) };
        let look = Quat::from_rotation_y(1.0) * Quat::from_rotation_x(0.6);
        let p = b.apply(Pose { pos: [0.0, 1.7, 0.0], valid: 1, rot: look.to_array() });
        let moved = Vec3::from_array(p.pos) - Vec3::new(0.0, 1.7, 0.0);
        assert!((moved.y + 0.02).abs() < 1e-5, "straight down, looking down too: {moved:?}");
        assert!((moved - Quat::from_rotation_y(1.0) * Vec3::new(0.01, -0.02, 0.0)).length() < 1e-5);
        assert!(Quat::from_array(p.rot).angle_between(look * Quat::from_rotation_x(0.01)) < 1e-5);
    }
}
