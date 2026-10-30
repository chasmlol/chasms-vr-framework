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
//!   near them takes the rein on its side, and takes it up: the rider draws it in to where they
//!   hold it (a hand comes back from the neck to the lap), so until the hand comes to rest (or for
//!   [`SETTLE_MAX`] at most) the rein follows it and does nothing, and where it stops is the rein's
//!   rest (a soft tick says so). A trigger squeezed with the grip as the rein is taken isn't a
//!   spur. From there,
//!   in the rider's own frame (tracking space turned to the mount as it was when the reins were
//!   taken, so the view turning with the mount in steps never moves them):
//!   - one rein pulled back further than the other, or both moved to one side (laid on the neck,
//!     or a rein opened out), turns the mount that way, finely, measured against the mount itself
//!     (not from where the reins were taken: hands centred over its mane and level go straight,
//!     wherever they were when they took the reins): the hands' middle across from the mane past
//!     [`NECK_DEAD`], one ahead of the other past [`LEVEL_DEAD`]; gently at first, as hard as the
//!     game turns it at `rein_travel` past that, smoothed over [`STEER_SMOOTH`];
//!   - its speed is a share of its full gallop, and fluid: a flick of the reins (a hand snapped
//!     down and stopped, either or both: the whip of Rival Stars and DovaVR's stroke) or the
//!     trigger of a hand on a rein (a squeeze of the heels) spurs it, as hard as it's done (the
//!     flick's speed, the squeeze's depth): it surges by that much, up to a full gallop for the
//!     hardest (rising at [`ACCEL`]), and the surge fades slowly (over `spur_fade`) to `cruise` of
//!     it: a light spur from a stand walks it slowly; to keep it at full, keep spurring hard. Reins
//!     held back are given by a spur (where the hands are is their rest from then on);
//!   - both pulled back slow it, as hard as they're pulled: nothing at the dead zone (`rein_dead`:
//!     hands resting on the reins don't slow it), [`BRAKE`] a second at `rein_pull`. Where it's slowed to when they're eased, it stays (a walk, a stand).
//!     Backing up is asked for on its own, from a stand: the reins eased and pulled right back
//!     again, and held for [`BACK_AFTER`]; eased, it stands;
//!
//!   One hand alone holds both reins, as on a western saddle: to the side it turns, back it slows.
//!   What the hands did and what the mount was told goes into the log twice a second while a rein
//!   is held, and each time the reins start or stop turning it.
//! * **Let go** of the reins, the mount keeps its speed for `coast` seconds, then eases off at
//!   [`LET_GO_EASE`] to a stand.
//! * **The stick** rides too: pushed ahead or to a side, the mount goes at the push (it walks; with
//!   sprint, the fastest gait) and the sideways push steers, so a hard turn keeps it going; pulled
//!   back (within 60° of straight back), it backs up. The turning stick steers alone. Let go, the
//!   mount stops, as with the game's own controls.
//! * **Getting off**: holding the trigger of a hand off the reins ([`DISMOUNT_HOLD`]), or a hand
//!   holding the pommel while the head leans well out over one side ([`LEAN_OUT`]: swinging a leg
//!   off), asks the game to take the rider down ([`RtRide::dismount_seq`]).
//!
//! The rest of riding is elsewhere: the view turning with the mount (the adapter SDK's
//! `TrackingMapper::carry_turn`, smoothly or in steps), the vignette and the fade while getting on
//! and off (the session's comfort), the body in the saddle (`crate::body`, `rustvr_ik`), the reins
//! drawn (`crate::reins`).

use glam::{Quat, Vec2, Vec3};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::settings::Settings;

/// A hand takes a rein this close to where the game's rider holds it (metres).
const REIN_REACH: f32 = 0.15;
/// Steering is measured against the mount: the hands' middle this far across from its mane, or one
/// hand this far ahead of the other, doesn't steer (metres: hands held naturally over the withers
/// are this near centred and level). Smoothed over `STEER_SMOOTH` (seconds).
pub const NECK_DEAD: f32 = 0.03;
pub const LEVEL_DEAD: f32 = 0.025;
pub const STEER_SMOOTH: f32 = 0.08;
/// A rein's rest is what a pull is measured from (where the hand took it up): a hand within
/// `STEER_DEAD` of it carries it along `REST_FOLLOW` slowly (seconds to close most of the way), so
/// a resting hand's drift is taken up and a deliberate move (a few tenths of a second) isn't.
pub const STEER_DEAD: f32 = 0.015;
pub const REST_FOLLOW: f32 = 1.5;
/// A full pull begun at a stand and held this long backs the mount up (seconds).
pub const BACK_AFTER: f64 = 1.2;
/// The rider's speed (a share of the mount's full gallop): a spur's surge rises to it at `ACCEL` a
/// second (from a stand to full in two seconds; the game's own acceleration comes on top); reins
/// pulled right back take `BRAKE` a second off it (less as they're pulled less: from a full gallop
/// to a stand in a second and a half); let go, past the coast, it eases off at `LET_GO_EASE` a
/// second. Under `STAND` it stands.
pub const ACCEL: f32 = 0.5;
pub const BRAKE: f32 = 0.7;
pub const LET_GO_EASE: f32 = 0.2;
const STAND: f32 = 0.02;
/// The reins' log while they're held, this often (seconds).
const LOG_EVERY: f64 = 0.5;
/// Taking up a rein: the hand has come to rest when it's moved slower than `SETTLE_STILL` (m/s) for
/// `SETTLE_HOLD` (seconds), at least `SETTLE_MIN` after taking it; or `SETTLE_MAX` after, whatever
/// it does. A trigger in the first `TAKE_TRIGGER` is the grip's squeeze, not a spur.
const SETTLE_STILL: f32 = 0.15;
const SETTLE_HOLD: f64 = 0.25;
const SETTLE_MIN: f64 = 0.3;
pub const SETTLE_MAX: f64 = 2.5;
const TAKE_TRIGGER: f64 = 0.4;
/// The reins turn the mount (for the log) past this share of a full turn.
const LOG_TURNING: f32 = 0.05;
/// A flick: a rein hand snapped down faster than this (m/s), then stopped (slower than
/// `FLICK_STOP` down) within `FLICK_WINDOW`, `FLICK_TRAVEL` or more lower; as hard as a spur gets
/// at `FLICK_HARD` (m/s at its fastest).
const FLICK_DOWN: f32 = 1.1;
const FLICK_HARD: f32 = 3.5;
/// A squeeze of a rein hand's trigger: past `SQUEEZE` (of its travel), as deep as it goes in
/// `SQUEEZE_TIME` (seconds): that deep, that hard a spur.
const SQUEEZE: f32 = 0.3;
const SQUEEZE_TIME: f64 = 0.12;
/// The lightest spur adds this much of a full gallop (a slow walk, from a stand); the hardest, all
/// of it.
const SPUR_LEAST: f32 = 0.1;
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
/// A stride's beat: the seat came down at least this far from its last height (metres).
const STRIDE_DROP: f32 = 0.01;
/// Asking to get on again is held off this long (seconds): the game's own check (a crime's
/// question) is still up.
const MOUNT_AGAIN: f64 = 1.5;

/// A rein in a hand: which (0 left, 1 right), its rest (tracking space: where the hand took it up),
/// when it was taken, and whether it's taken up yet (the hand come to rest).
#[derive(Clone, Copy, Debug)]
struct Rein {
    side: usize,
    rest: Vec3,
    taken_at: f64,
    settled: bool,
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
    /// The gait asked for (as `Riding::gait`: by the reins -1 backing, 0 standing, 1 going).
    gait: i32,
    /// By the reins: the speed asked for, where the last spur's surge stands (fading to the
    /// cruise), and the cruise (each a share of the mount's full gallop); whether the reins are
    /// slowing it.
    speed: f32,
    surge: f32,
    cruise: f32,
    braking: bool,
    /// The reins pulled right back since, and whether the pull began with the mount standing (the
    /// reins eased since it stopped): held, it backs.
    pulling: Option<f64>,
    from_stand: bool,
    /// The reins' last log, and which way they last turned the mount (-1, 0, 1).
    logged_at: f64,
    reins_turning: i32,
    /// The reins let go at.
    let_go_at: Option<f64>,
    stick_was: bool,
    spur_at: f64,
    /// Each hand: its last place (for the flick's speed), a down stroke started (when, how high),
    /// and since when it's been still (taking up a rein).
    last_pos: [Option<Vec3>; 2],
    still_since: [Option<f64>; 2],
    flick: [Option<(f64, f32, f32)>; 2],
    /// A rein hand's trigger squeezed since, and how deep yet.
    squeeze: [Option<(f64, f32)>; 2],
    /// The reins' steering, smoothed.
    steer: f32,
    grip_was: [bool; 2],
    trig_was: [bool; 2],
    /// The trigger held since, and whether this hold already asked to get off.
    trig_since: [Option<f64>; 2],
    hold_asked: [bool; 2],
    lean_since: Option<f64>,
    on_pommel: bool,
    mount_at: f64,
    mounts_seen: u32,
    was_mounted: bool,
    /// The seat's height and which way it last moved, for the stride in the hands.
    seat_y: f32,
    seat_top: f32,
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

    /// Which rein (0 left, 1 right) each hand holds.
    pub fn reins_held(&self) -> [Option<usize>; 2] {
        self.reins.map(|r| r.map(|r| r.side))
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
        let speed: [f32; 2] = std::array::from_fn(|h| self.last_pos[h].map_or(0.0, |p| pos(h).distance(p) / dt));

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
                        self.reins[h] = Some(Rein { side, rest: pos(h), taken_at: now, settled: false });
                        self.still_since[h] = None;
                        // (A trigger squeezed as the rein's taken is let go before it spurs.)
                        self.squeeze[h] = Some((f64::INFINITY, 0.0));
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

        // Taking up a rein: it follows the hand (and does nothing) until the hand comes to rest.
        for &h in &held {
            if speed[h] >= SETTLE_STILL {
                self.still_since[h] = None;
            }
            let Some(rein) = self.reins[h].as_mut().filter(|r| !r.settled) else { continue };
            rein.rest = pos(h);
            let still = speed[h] < SETTLE_STILL && now - *self.still_since[h].get_or_insert(now) >= SETTLE_HOLD;
            let since = now - rein.taken_at;
            if (since >= SETTLE_MIN && still) || since >= SETTLE_MAX {
                rein.settled = true;
                self.haptics.push((h, 0.03, 90.0, 0.35));
                tracing::info!("riding: the {} rein taken up, {:.1} s after taking it", side_name(h), since);
            }
        }

        // Where the reins are from their rest, in the rider's frame (x right, y up, z ahead). A hand
        // inside the dead zone carries its rest along: what a resting hand drifts is taken up.
        let turn = Quat::from_rotation_y(-self.frame_yaw);
        let dead_zone = s.rein_dead.clamp(0.01, 0.15);
        let follow = 1.0 - (-dt / REST_FOLLOW).exp();
        for &h in &held {
            if let Some(rein) = self.reins[h].as_mut() {
                let m = turn * (pos(h) - rein.rest);
                if m.x.abs() < STEER_DEAD && m.z.abs() < STEER_DEAD {
                    rein.rest += (pos(h) - rein.rest) * follow;
                }
            }
        }
        let reins_now = self.reins;
        let moved = |h: usize| reins_now[h].map_or(Vec3::ZERO, |rein| turn * (pos(h) - rein.rest));
        let on_side = |side: usize| (0..2).find(|&h| reins_now[h].is_some_and(|r| r.side == side));
        // Where a hand is against the mount: across from its mane (right +) and ahead along it, from
        // the middle of where its rider holds the reins.
        let mane = (reins[0] + reins[1]) * 0.5;
        let across = Vec3::new(reins[1].x - reins[0].x, 0.0, reins[1].z - reins[0].z).normalize_or(Vec3::X);
        let ahead = across.cross(Vec3::Y);
        let on_mount = |h: usize| ((pos(h) - mane).dot(across), (pos(h) - mane).dot(ahead));
        // A move past a dead zone as a share of a full turn: gently at first, all of it
        // `rein_travel` past the dead zone.
        let travel = s.rein_travel.max(0.03);
        let share = |d: f32, dead: f32| {
            let x = ((d.abs() - dead) / travel).clamp(0.0, 1.0);
            d.signum() * (0.5 * x + 0.5 * x * x)
        };
        let (rein_steer, pull) = match (on_side(0), on_side(1)) {
            (Some(lh), Some(rh)) => {
                let (l, rt) = (on_mount(lh), on_mount(rh));
                // The right rein back further turns right, as do both reins moved right.
                (share(l.1 - rt.1, LEVEL_DEAD) + share(0.5 * (l.0 + rt.0), NECK_DEAD), -moved(lh).z.max(moved(rh).z))
            }
            (Some(h), None) | (None, Some(h)) => (share(on_mount(h).0, NECK_DEAD), -moved(h).z),
            _ => (0.0, 0.0),
        };
        // (Smoothed: a trembling hand's turn doesn't jitter.)
        self.steer += (rein_steer - self.steer) * (1.0 - (-dt / STEER_SMOOTH).exp());
        let rein_steer = if held.is_empty() { 0.0 } else { self.steer };
        let mut steer = rein_steer;

        // A flick of a rein hand, or a squeeze of its trigger: a spur, as hard as it's done (0..1).
        let mut spur: Option<(&str, f32)> = None;
        let mut spurred = |how: &'static str, hard: f32| {
            if spur.is_none_or(|(_, was)| hard > was) {
                spur = Some((how, hard));
            }
        };
        for &h in &held {
            // (A trigger squeezed with the grip as the rein is taken isn't a spur.)
            let trigger = if live(h) { f.hands[h].trigger } else { 0.0 };
            let taking = self.reins[h].is_some_and(|r| now - r.taken_at < TAKE_TRIGGER);
            match self.squeeze[h] {
                None if trigger > SQUEEZE && !taking => self.squeeze[h] = Some((now, trigger)),
                Some((t, deepest)) if now - t >= SQUEEZE_TIME => {
                    let hard = ((deepest.max(trigger) - SQUEEZE) / (1.0 - SQUEEZE)).clamp(0.0, 1.0);
                    spurred("the trigger", hard);
                    tracing::info!("riding: a squeeze of the {} hand's trigger ({:.0}% of it: {:.0}% of the hardest spur)", side_name(h), deepest.max(trigger) * 100.0, hard * 100.0);
                    // (Held, it's one squeeze: the next starts once it's let go.)
                    self.squeeze[h] = Some((f64::INFINITY, 0.0));
                }
                Some((t, deepest)) if t.is_finite() => self.squeeze[h] = Some((t, deepest.max(trigger))),
                Some(_) if trigger < SQUEEZE * 0.5 => self.squeeze[h] = None,
                _ => {}
            }
            if s.flick {
                match self.flick[h] {
                    None if velocity[h] < -FLICK_DOWN => self.flick[h] = Some((now, pos(h).y, -velocity[h])),
                    Some((t, _, _)) if now - t > FLICK_WINDOW => self.flick[h] = None,
                    Some((_, y0, fastest)) if velocity[h] > -FLICK_STOP => {
                        if y0 - pos(h).y >= FLICK_TRAVEL {
                            let hard = ((fastest - FLICK_DOWN) / (FLICK_HARD - FLICK_DOWN)).clamp(0.0, 1.0);
                            spurred("a flick", hard);
                            tracing::info!("riding: a flick of the reins ({} hand, {:.2} m at {:.1} m/s: {:.0}% of the hardest spur)", side_name(h), y0 - pos(h).y, fastest, hard * 100.0);
                        }
                        self.flick[h] = None;
                    }
                    Some((t, y0, fastest)) => self.flick[h] = Some((t, y0, fastest.max(-velocity[h]))),
                    _ => {}
                }
            }
        }
        for h in 0..2 {
            if self.reins[h].is_none() {
                self.flick[h] = None;
                self.squeeze[h] = None;
            }
        }

        // The speed from the reins: a spur surges it to a full gallop, which fades to a cruise; a
        // pull takes it down as hard as it's pulled, and where it's eased it stays.
        let was = self.gait;
        let mut why = "";
        if !held.is_empty() {
            self.let_go_at = None;
            let full = s.rein_pull.max(dead_zone + 0.03);
            // (How hard the reins are pulled: nothing at the dead zone, all of it at `rein_pull`.)
            let brake = ((pull - dead_zone) / (full - dead_zone)).clamp(0.0, 1.0);
            if let Some((how, hard)) = spur.filter(|_| now - self.spur_at > SPUR_COOLDOWN) {
                // A spur is always heard: reins held back are given, and where the hands are now
                // is their rest. It surges as hard as it's done, and settles to its share of that.
                self.spur_at = now;
                self.surge = (self.surge.max(self.speed) + SPUR_LEAST + (1.0 - SPUR_LEAST) * hard).min(1.0);
                self.cruise = self.cruise.max(self.surge * s.cruise.clamp(0.05, 1.0));
                self.gait = self.gait.max(0);
                why = how;
                if self.pulling.take().is_some() || brake > 0.0 {
                    for &h in &held {
                        if let Some(rein) = self.reins[h].as_mut() {
                            rein.rest = pos(h);
                        }
                    }
                    tracing::info!("riding: spurred with the reins held back ({:.2} m): the reins given", pull);
                }
                self.braking = false;
                tracing::info!("riding: {} ({:.0}% of the hardest): a surge to {:.2} of a gallop (from {:.2}), settling at {:.2}", how, hard * 100.0, self.surge, self.speed, self.cruise);
                for &h in &held {
                    self.haptics.push((h, 0.08, 80.0, 0.8));
                }
            } else if brake > 0.0 && self.gait >= 0 {
                // (Off the surge and the cruise both: where it's slowed to, it stays.)
                self.surge = (self.surge - BRAKE * brake * dt).max(0.0);
                self.cruise = self.cruise.min(self.surge);
                self.speed = self.speed.min(self.surge);
                if !self.braking && self.speed > STAND {
                    self.braking = true;
                    tracing::info!("riding: the reins slow it ({:.2} m back: {:.0}% of the hardest), from {:.2} of a gallop", pull, brake * 100.0, self.speed);
                    for &h in &held {
                        self.haptics.push((h, 0.04, 60.0, 0.4));
                    }
                }
            }
            if brake <= 0.0 && self.braking {
                self.braking = false;
                tracing::info!("riding: the reins eased: it keeps {:.2} of a gallop", self.speed);
            }
            // Backing up: from a stand, a full pull begun there and held.
            if brake >= 0.9 {
                match self.pulling {
                    None => {
                        self.pulling = Some(now);
                        self.from_stand = self.speed < STAND && self.gait == 0;
                        if self.from_stand {
                            tracing::info!("riding: the reins pulled right back at a stand ({:.2} m): held, it backs", pull);
                        }
                    }
                    Some(since) if self.from_stand && self.gait == 0 && now - since >= BACK_AFTER => {
                        self.gait = -1;
                        why = "the reins held back at a stand";
                    }
                    _ => {}
                }
            } else if brake < 0.5 && self.pulling.take().is_some() && self.gait < 0 {
                self.gait = 0;
                why = "the reins eased";
            }
        } else {
            self.pulling = None;
            self.braking = false;
            if self.gait < 0 {
                self.gait = 0;
            }
            let since = *self.let_go_at.get_or_insert(now);
            if now - since >= s.coast.max(0.0) as f64 {
                self.surge = (self.surge - LET_GO_EASE * dt).max(0.0);
                self.cruise = self.cruise.min(self.surge);
            }
        }
        // The surge fades to the cruise; the speed rises to it at `ACCEL` and follows it down.
        self.surge = self.cruise + (self.surge - self.cruise).max(0.0) * (-dt / s.spur_fade.max(0.5)).exp();
        self.speed = if self.surge > self.speed { (self.speed + ACCEL * dt).min(self.surge) } else { self.surge };
        if self.gait >= 0 {
            self.gait = (self.speed > STAND) as i32;
        }

        // The stick rides too: pushed, its gait and pace (and, let go, the mount stops, as the
        // game's own controls do); its sideways push, or the turning stick's, steers (the turning
        // stick alone leaves the gait as it is).
        let stick = Vec2::from_array(inp.move_xy);
        let dom = inp.settings.dominant();
        let turning = deadzone(f.hands[dom].stick[0], 0.25);
        let stick_on = stick.length() > STICK;
        if stick_on {
            out.flags |= RR_STICK;
            // (Ahead or to a side goes at the push, so a hard turn keeps going; back, within 60°
            // of straight back, backs up.)
            let push = stick.length().min(1.0);
            let (gait, pace) = if stick.y < -0.5 * stick.length() { (-1, push) } else { (if inp.sprint { gaits } else { 1 }, push) };
            self.gait = gait;
            out.gait = gait;
            out.pace = pace;
            steer = stick.x;
            // (The stick has the mount: what the reins asked of it is over.)
            (self.speed, self.surge, self.cruise) = (0.0, 0.0, 0.0);
        } else {
            if self.stick_was {
                self.gait = 0;
            }
            out.gait = self.gait;
            out.pace = if self.gait != 0 { 1.0 } else { 0.0 };
            if self.gait > 0 {
                out.flags |= RR_SPEED;
                out.speed = self.speed;
            }
        }
        if turning != 0.0 {
            out.flags |= RR_STICK;
            steer = if stick_on { steer + turning } else { turning };
        }
        let by_stick = stick_on || self.stick_was;
        self.stick_was = stick_on;
        out.steer = steer.clamp(-1.0, 1.0);

        // What the hands did and what the mount was told: twice a second while a rein is held,
        // and whenever the reins start turning it, change the way they turn it, or stop.
        let reins_way = if rein_steer.abs() > LOG_TURNING && !stick_on { rein_steer.signum() as i32 } else { 0 };
        let reins_now = self.reins;
        let hand = |h: usize| match reins_now[h] {
            Some(r) => {
                let m = turn * (pos(h) - r.rest);
                let (x, z) = on_mount(h);
                format!("{} hand ({} rein) {:+.2} across the mane {:+.2} ahead, {:+.2} back from its rest", side_name(h), side_name(r.side), x, z, -m.z)
            }
            None => format!("{} hand free", side_name(h)),
        };
        if !held.is_empty() && (reins_way != self.reins_turning || now - self.logged_at >= LOG_EVERY) {
            if reins_way != self.reins_turning {
                match reins_way {
                    0 => tracing::info!("riding: the reins stop turning the mount"),
                    t => tracing::info!("riding: the reins turn the mount {} ({:.2} of a full turn)", if t < 0 { "left" } else { "right" }, rein_steer.abs()),
                }
            }
            tracing::info!(
                "riding: reins: {}, {}; pull {:.2} (slows from {:.2}, hardest at {:.2}){} -> gait {} speed {:.2} (surge {:.2}, settling at {:.2}) steer {:+.2}{}",
                hand(0),
                hand(1),
                pull,
                dead_zone,
                s.rein_pull.max(dead_zone + 0.03),
                if self.braking { ", slowing it" } else { "" },
                out.gait,
                if out.flags & RR_SPEED != 0 { out.speed } else { out.pace },
                self.surge,
                self.cruise,
                out.steer,
                if out.flags & RR_STICK != 0 { " (the stick)" } else { "" }
            );
            self.logged_at = now;
        }
        self.reins_turning = if held.is_empty() { 0 } else { reins_way };
        if self.gait != was {
            let why = if by_stick {
                "the stick"
            } else if !why.is_empty() {
                why
            } else if !held.is_empty() {
                "the reins"
            } else {
                "the reins let go"
            };
            tracing::info!("riding: gait {} -> {} ({})", was, self.gait, why);
            if self.gait < was {
                for &h in &held {
                    self.haptics.push((h, 0.05, 60.0, 0.5));
                }
            }
            if self.gait == 0 && !by_stick {
                (self.speed, self.surge, self.cruise) = (0.0, 0.0, 0.0);
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
        if on_pommel != self.on_pommel {
            self.on_pommel = on_pommel;
            tracing::info!("riding: a hand {} the pommel (the head {:.2} m to the side of the seat)", if on_pommel { "on" } else { "off" }, lean);
        }
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

        // The stride, felt in the hands on the reins: a beat each time the seat comes down from a
        // stride's height (not the seat's every little move).
        let y = r.seat.pos[1];
        if (y - self.seat_y).abs() > 1e-4 {
            let falling = y < self.seat_y;
            if self.seat_falling && !falling {
                let dropped = self.seat_top - self.seat_y;
                if dropped >= STRIDE_DROP && s.stride_haptics && r.gait != 0 && r.speed.abs() > 0.3 {
                    let amp = 0.12 + 0.2 * (r.gait.unsigned_abs() as f32 / gaits as f32);
                    for &h in &held {
                        self.haptics.push((h, 0.03, 60.0, amp));
                    }
                }
            }
            if !self.seat_falling && falling {
                self.seat_top = self.seat_y;
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

}

fn side_name(h: usize) -> &'static str {
    if h == 0 { "left" } else { "right" }
}

fn quat(q: [f32; 4]) -> Quat {
    let q = Quat::from_array(q);
    if q.length_squared() < 1e-8 { Quat::IDENTITY } else { q.normalize() }
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
