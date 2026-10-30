//! Riding a mount (`rustvr_protocol::ride`): the rider's side of it, for any game. The game has the
//! mount (its model, its gaits and their speeds, how it turns, whose it is, getting on and off, its
//! sounds); the runtime has the rider. Why it works this way is in the Oblivion adapter's
//! `docs/research/riding.md` (what players of VR riding like and hate, and what keeps it
//! comfortable).
//!
//! * **Getting on**: a free hand closing on a mount's saddle ([`Mount::saddle_a`] to `saddle_b`)
//!   asks the game to put the rider on it ([`RtRide::mount`]), the game's own way: its checks, its
//!   crime if it's someone else's, its animation, its sounds.
//! * **The reins** are where the game's rider holds them ([`Riding::reins`]). A free hand gripping
//!   near them takes the rein on its side, and where it took it is that rein's rest. From there,
//!   in the rider's own frame (tracking space turned to the mount as it was when the reins were
//!   taken, so the view turning with the mount in steps never moves them):
//!   - one rein pulled back further than the other, or both moved to one side (laid on the neck,
//!     or a rein opened out), turns the mount that way, as hard as the game turns it at
//!     `rein_travel`;
//!   - both pulled back past `rein_pull` slow it a gait; held, a gait every [`PULL_REPEAT`] to a
//!     stand, then it backs up;
//!   - a flick of the reins (a hand snapped down and stopped, either or both: the whip of Rival
//!     Stars and DovaVR's stroke) or the trigger of a hand on a rein (a squeeze of the heels)
//!     spurs it on a gait, up to its fastest.
//!
//!   One hand alone holds both reins, as on a western saddle: to the side it turns, back it slows.
//! * **Let go** of the reins, the mount keeps its gait for `coast` seconds, then slows a gait every
//!   [`COAST_STEP`] to a stand (the game's own slowing from one gait to the next).
//! * **The stick** rides too: pushed, it's the gait (forward walks, at the push; with sprint the
//!   fastest; back backs up), and its sideways push (or the turning stick's) steers. Let go, the
//!   mount stops, as with the game's own controls.
//! * **Getting off**: holding the trigger of a hand off the reins ([`DISMOUNT_HOLD`]), or a hand
//!   holding the pommel while the head leans well out over one side ([`LEAN_OUT`]: swinging a leg
//!   off), asks the game to take the rider down ([`RtRide::dismount_seq`]).
//!
//! The rest of riding is elsewhere: the view turning with the mount (the adapter SDK's
//! `TrackingMapper::carry_turn`, smoothly or in steps), the vignette and the fade while getting on
//! and off (the session's comfort), the body in the saddle (`crate::body`, `rustvr_ik`).

use glam::{Quat, Vec2, Vec3};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::settings::Settings;

/// A hand takes a rein this close to where the game's rider holds it (metres).
const REIN_REACH: f32 = 0.15;
/// A rein moved less than this from its rest does nothing (metres): a resting hand drifts.
const DEAD: f32 = 0.025;
/// Held pulled back, the mount slows another gait this often (seconds), and once standing backs
/// up after this long.
pub const PULL_REPEAT: f64 = 0.9;
const BACK_AFTER: f64 = 1.0;
/// Let go of the reins, past the coast the mount slows a gait this often (seconds).
pub const COAST_STEP: f64 = 1.5;
/// A flick: a rein hand snapped down faster than this (m/s), then stopped (slower than
/// `FLICK_STOP` down) within `FLICK_WINDOW`, `FLICK_TRAVEL` or more lower.
const FLICK_DOWN: f32 = 1.1;
const FLICK_STOP: f32 = 0.25;
const FLICK_WINDOW: f64 = 0.3;
const FLICK_TRAVEL: f32 = 0.04;
/// Spurs closer together than this are one (both hands flick at once).
const SPUR_COOLDOWN: f64 = 0.35;
/// Getting off: a free hand's trigger held this long (seconds); or the head this far out to one
/// side of the seat (metres) with a hand on the pommel, for `LEAN_HOLD`.
pub const DISMOUNT_HOLD: f64 = 0.6;
pub const LEAN_OUT: f32 = 0.35;
const LEAN_HOLD: f64 = 0.3;
const POMMEL_REACH: f32 = 0.15;
/// The stick rides past this push.
const STICK: f32 = 0.15;
/// Asking to get on again is held off this long (seconds): the game's own check (a crime's
/// question) is still up.
const MOUNT_AGAIN: f64 = 1.5;

/// A rein in a hand: which (0 left, 1 right) and where the hand took it (tracking space).
#[derive(Clone, Copy, Debug)]
struct Rein {
    side: usize,
    rest: Vec3,
}

pub struct RidingInput<'a> {
    pub frame: &'a HmdFrame,
    pub ride: &'a Ride,
    pub settings: &'a Settings,
    pub now: f64,
    /// The hands holding something (the rig's): they don't take a rein or a saddle.
    pub busy: [bool; 2],
    /// The walking stick's push (`Intents::move_xy`, past its dead zone) and sprint held.
    pub move_xy: [f32; 2],
    pub sprint: bool,
}

/// A haptic pulse: hand, seconds, frequency, amplitude.
pub type Pulse = (usize, f32, f32, f32);

#[derive(Default)]
pub struct Riding {
    reins: [Option<Rein>; 2],
    /// The rider's frame: the mount's yaw in tracking space when the first rein was taken.
    frame_yaw: f32,
    /// The gait asked for (as `Riding::gait`).
    gait: i32,
    /// Both reins pulled back since; when the next gait down is; when the mount stood.
    pulling: Option<f64>,
    pull_step_at: f64,
    stood_at: f64,
    /// The reins let go at; when the next gait down is.
    let_go_at: Option<f64>,
    coast_step_at: f64,
    stick_was: bool,
    spur_at: f64,
    /// Each hand: its last place (for the flick's speed), a down stroke started (when, how high).
    last_pos: [Option<Vec3>; 2],
    flick: [Option<(f64, f32)>; 2],
    grip_was: [bool; 2],
    trig_was: [bool; 2],
    /// The trigger held since, and whether this hold already asked to get off.
    trig_since: [Option<f64>; 2],
    hold_asked: [bool; 2],
    lean_since: Option<f64>,
    mount_at: f64,
    mounts_seen: u32,
    was_mounted: bool,
    /// The seat's height and which way it last moved, for the stride in the hands.
    seat_y: f32,
    seat_falling: bool,
    out: RtRide,
    pub haptics: Vec<Pulse>,
}

impl Riding {
    pub fn new() -> Riding {
        Riding { mount_at: f64::MIN, spur_at: f64::MIN, ..Default::default() }
    }

    /// The gait asked for now.
    pub fn gait(&self) -> i32 {
        self.gait
    }

    /// One runtime frame: what the rider does with the mount.
    pub fn update(&mut self, inp: &RidingInput) -> RtRide {
        self.haptics.clear();
        let f = inp.frame;
        let now = inp.now;
        let dt = f.dt.max(1e-4);
        let r = &inp.ride.riding;
        let s = &inp.settings.riding;
        let ia = &inp.settings.interaction;
        let mut out = self.out;
        out.frame_index = f.index;
        out.flags = 0;
        let pos = |h: usize| Vec3::from_array(f.hands[h].grip.pos);
        let live = |h: usize| f.hands[h].connected != 0 && f.hands[h].grip.valid != 0;
        let gripped: [bool; 2] = std::array::from_fn(|h| live(h) && f.hands[h].squeeze > if self.grip_was[h] { ia.grip_release } else { ia.grip_press });
        let triggered: [bool; 2] = std::array::from_fn(|h| live(h) && f.hands[h].trigger > if self.trig_was[h] { 0.4 } else { 0.6 });
        let velocity: [f32; 2] = std::array::from_fn(|h| self.last_pos[h].map_or(0.0, |p| (pos(h).y - p.y) / dt));

        if !r.mounted() {
            // Getting on: a free hand closing on a mount's saddle.
            if inp.ride.count != self.mounts_seen {
                self.mounts_seen = inp.ride.count;
                tracing::info!("riding: {} mounts near", inp.ride.count);
            }
            if !r.moving_on_or_off() && now - self.mount_at > MOUNT_AGAIN {
                for h in 0..2 {
                    if !gripped[h] || self.grip_was[h] || inp.busy[h] {
                        if gripped[h] && !self.grip_was[h] {
                            tracing::info!("riding: the {} hand gripped, holding something", side_name(h));
                        }
                        continue;
                    }
                    if let Some(d) = inp.ride.mounts().iter().map(|m| segment_distance(pos(h), Vec3::from_array(m.saddle_a), Vec3::from_array(m.saddle_b)) - m.radius).min_by(f32::total_cmp) {
                        tracing::info!("riding: the {} hand gripped at {:.2?}, {:.2} m past the nearest saddle's reach", side_name(h), pos(h), d);
                    }
                    let near = inp.ride.mounts().iter().map(|m| (m, segment_distance(pos(h), Vec3::from_array(m.saddle_a), Vec3::from_array(m.saddle_b)))).filter(|(m, d)| *d <= m.radius).min_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((m, d)) = near {
                        out.mount = m.id;
                        out.mount_seq = out.mount_seq.wrapping_add(1);
                        self.mount_at = now;
                        self.haptics.push((h, 0.06, 150.0, 0.7));
                        tracing::info!("riding: the {} hand took the saddle of mount {:#x} ({:.2} m from its line{}): getting on", side_name(h), m.id, d, if m.flags & MOUNT_CRIME != 0 { ", someone else's" } else { "" });
                        break;
                    }
                }
            }
            if self.was_mounted {
                tracing::info!("riding: off the mount");
            }
            let haptics = std::mem::take(&mut self.haptics);
            *self = Riding { out, mount_at: self.mount_at, mounts_seen: self.mounts_seen, grip_was: gripped, trig_was: triggered, last_pos: [0, 1].map(|h| live(h).then(|| pos(h))), haptics, ..Riding::new() };
            out.gait = 0;
            out.pace = 0.0;
            out.steer = 0.0;
            self.out = out;
            return out;
        }
        if !self.was_mounted {
            tracing::info!("riding: in the saddle of mount {:#x} ({} gaits)", r.id, r.gaits);
            self.was_mounted = true;
            self.gait = 0;
        }
        out.flags |= RR_RIDING;
        let gaits = r.gaits.max(1) as i32;
        let fwd = quat(r.seat.rot) * Vec3::Z;
        let mount_yaw = fwd.x.atan2(fwd.z);
        let reins = [Vec3::new(r.reins[0][0], r.reins[0][1], r.reins[0][2]), Vec3::new(r.reins[1][0], r.reins[1][1], r.reins[1][2])];

        // Taking the reins and letting them go.
        for h in 0..2 {
            match self.reins[h] {
                Some(_) if !gripped[h] => {
                    self.reins[h] = None;
                    tracing::info!("riding: the {} hand let go of the reins", side_name(h));
                }
                None if gripped[h] && !inp.busy[h] && r.flags & RIDE_REINS != 0 => {
                    let other = self.reins[1 - h].map(|o| o.side);
                    let take = (0..2).filter(|&side| Some(side) != other).map(|side| (side, reins[side].distance(pos(h)))).filter(|(_, d)| *d <= REIN_REACH).min_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((side, d)) = take {
                        if self.reins[1 - h].is_none() {
                            self.frame_yaw = mount_yaw;
                        }
                        self.reins[h] = Some(Rein { side, rest: pos(h) });
                        self.haptics.push((h, 0.04, 120.0, 0.5));
                        tracing::info!("riding: the {} hand took the {} rein ({:.2} m from it)", side_name(h), side_name(side), d);
                    }
                }
                _ => {}
            }
        }
        let held: Vec<usize> = (0..2).filter(|&h| self.reins[h].is_some()).collect();
        for &h in &held {
            out.flags |= if self.reins[h].unwrap().side == 0 { RR_REIN_LEFT } else { RR_REIN_RIGHT };
        }

        // Where the reins are from their rest, in the rider's frame (x right, y up, z ahead).
        let turn = Quat::from_rotation_y(-self.frame_yaw);
        let moved = |h: usize| self.reins[h].map_or(Vec3::ZERO, |rein| turn * (pos(h) - rein.rest));
        let travel = s.rein_travel.max(0.03);
        let (mut steer, pull) = match held.as_slice() {
            [_, _] => {
                let (l, rt) = (moved(0), moved(1));
                // The right rein back further turns right, as do both reins moved right.
                ((dead(rt.z - l.z) * -1.0 + dead(0.5 * (l.x + rt.x))) / travel, -l.z.max(rt.z))
            }
            [h] => {
                let m = moved(*h);
                (dead(m.x) / travel, -m.z)
            }
            _ => (0.0, 0.0),
        };

        // A flick of a rein hand, or the trigger of one: a spur.
        let mut spur = false;
        for &h in &held {
            if triggered[h] && !self.trig_was[h] {
                spur = true;
            }
            if s.flick {
                match self.flick[h] {
                    None if velocity[h] < -FLICK_DOWN => self.flick[h] = Some((now, pos(h).y)),
                    Some((t, _)) if now - t > FLICK_WINDOW => self.flick[h] = None,
                    Some((_, y0)) if velocity[h] > -FLICK_STOP => {
                        if y0 - pos(h).y >= FLICK_TRAVEL {
                            spur = true;
                            tracing::info!("riding: a flick of the reins ({} hand, {:.2} m)", side_name(h), y0 - pos(h).y);
                        }
                        self.flick[h] = None;
                    }
                    _ => {}
                }
            }
        }
        for h in 0..2 {
            if self.reins[h].is_none() {
                self.flick[h] = None;
            }
        }

        // The gait from the reins.
        let was = self.gait;
        if !held.is_empty() {
            self.let_go_at = None;
            let on = s.rein_pull.max(0.02);
            if pull > on {
                match self.pulling {
                    None => {
                        self.pulling = Some(now);
                        self.slow(now);
                    }
                    Some(_) if now >= self.pull_step_at && self.gait > 0 => self.slow(now),
                    Some(_) if self.gait == 0 && now - self.stood_at >= BACK_AFTER => self.gait = -1,
                    _ => {}
                }
            } else if pull < on * 0.5 && self.pulling.take().is_some() && self.gait < 0 {
                self.gait = 0;
            }
            if spur && self.pulling.is_none() && now - self.spur_at > SPUR_COOLDOWN {
                self.spur_at = now;
                self.gait = (self.gait.max(0) + 1).min(gaits);
                for &h in &held {
                    self.haptics.push((h, 0.08, 80.0, 0.8));
                }
            }
        } else {
            self.pulling = None;
            if self.gait < 0 {
                self.gait = 0;
            }
            let since = *self.let_go_at.get_or_insert_with(|| {
                self.coast_step_at = now + s.coast.max(0.0) as f64;
                now
            });
            if self.gait > 0 && now >= self.coast_step_at && now - since >= s.coast.max(0.0) as f64 {
                self.gait -= 1;
                self.coast_step_at = now + COAST_STEP;
            }
        }

        // The stick rides too (and, let go, stops the mount as the game's own controls do).
        let stick = Vec2::from_array(inp.move_xy);
        let dom = inp.settings.dominant();
        let turning = deadzone(f.hands[dom].stick[0], 0.25);
        let stick_on = stick.length() > STICK || turning != 0.0;
        if stick_on {
            out.flags |= RR_STICK;
            let (gait, pace) = if stick.y > STICK {
                (if inp.sprint { gaits } else { 1 }, stick.y.min(1.0))
            } else if stick.y < -STICK {
                (-1, (-stick.y).min(1.0))
            } else {
                (0, 0.0)
            };
            self.gait = gait;
            out.gait = gait;
            out.pace = pace;
            steer = stick.x + turning;
        } else {
            if self.stick_was {
                self.gait = 0;
            }
            out.gait = self.gait;
            out.pace = if self.gait != 0 { 1.0 } else { 0.0 };
        }
        self.stick_was = stick_on;
        out.steer = steer.clamp(-1.0, 1.0);
        if self.gait != was {
            tracing::info!("riding: gait {} -> {} ({})", was, self.gait, if stick_on { "the stick" } else if spur { "spurred on" } else if !held.is_empty() { "the reins pulled" } else { "the reins let go" });
            if self.gait < was {
                for &h in &held {
                    self.haptics.push((h, 0.05, 60.0, 0.5));
                }
            }
        }

        // Getting off: a free hand's trigger held, or the pommel held and the head leaning out.
        let mut off = false;
        for h in 0..2 {
            let free = self.reins[h].is_none() && !inp.busy[h];
            if triggered[h] && free {
                let since = *self.trig_since[h].get_or_insert(now);
                if now - since >= DISMOUNT_HOLD && !self.hold_asked[h] {
                    self.hold_asked[h] = true;
                    off = true;
                    tracing::info!("riding: the {} hand's trigger held: getting off", side_name(h));
                }
            } else {
                self.trig_since[h] = None;
                self.hold_asked[h] = false;
            }
        }
        let pommel = Vec3::new(r.pommel[0], r.pommel[1], r.pommel[2]);
        let on_pommel = (0..2).any(|h| gripped[h] && self.reins[h].is_none() && pos(h).distance(pommel) <= POMMEL_REACH);
        let seat = Vec3::from_array(r.seat.pos);
        let lean = (Quat::from_rotation_y(-mount_yaw) * (Vec3::from_array(f.hmd.pos) - seat)).x;
        if on_pommel && lean.abs() >= LEAN_OUT {
            let since = *self.lean_since.get_or_insert(now);
            if now - since >= LEAN_HOLD {
                self.lean_since = Some(f64::INFINITY);
                off = true;
                tracing::info!("riding: leaning out {:.2} m to the {} with a hand on the pommel: getting off", lean.abs(), if lean < 0.0 { "left" } else { "right" });
            }
        } else {
            self.lean_since = None;
        }
        if off {
            out.dismount_seq = out.dismount_seq.wrapping_add(1);
            self.gait = 0;
            for h in 0..2 {
                self.haptics.push((h, 0.06, 100.0, 0.6));
            }
        }

        // The stride, felt in the hands on the reins: a beat each time the seat comes down.
        let y = r.seat.pos[1];
        if (y - self.seat_y).abs() > 1e-4 {
            let falling = y < self.seat_y;
            if self.seat_falling && !falling && s.stride_haptics && r.gait != 0 && r.speed.abs() > 0.3 {
                let amp = 0.12 + 0.2 * (r.gait.unsigned_abs() as f32 / gaits as f32);
                for &h in &held {
                    self.haptics.push((h, 0.03, 60.0, amp));
                }
            }
            self.seat_falling = falling;
            self.seat_y = y;
        }

        self.grip_was = gripped;
        self.trig_was = triggered;
        self.last_pos = [0, 1].map(|h| live(h).then(|| pos(h)));
        self.out = out;
        out
    }

    /// Down a gait (not past standing), and the next one after `PULL_REPEAT` if the pull's held.
    fn slow(&mut self, now: f64) {
        if self.gait > 0 {
            self.gait -= 1;
            if self.gait == 0 {
                self.stood_at = now;
            }
        }
        self.pull_step_at = now + PULL_REPEAT;
    }
}

fn side_name(h: usize) -> &'static str {
    if h == 0 { "left" } else { "right" }
}

fn quat(q: [f32; 4]) -> Quat {
    let q = Quat::from_array(q);
    if q.length_squared() < 1e-8 { Quat::IDENTITY } else { q.normalize() }
}

/// A rein's move past the dead zone (its sign kept).
fn dead(x: f32) -> f32 {
    x.signum() * (x.abs() - DEAD).max(0.0)
}

fn deadzone(x: f32, dz: f32) -> f32 {
    if x.abs() < dz { 0.0 } else { x.signum() * (x.abs() - dz) / (1.0 - dz) }
}

/// The distance from `p` to the segment `a`..`b`.
fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-8 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}
