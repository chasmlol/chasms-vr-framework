//! Throwables: grenades and throwing knives, the hand side of them (the game owns what happens
//! when one goes off or hits). From `docs/design/vr-throwables-and-ui/throwables.md`:
//!
//! * **Grenades** ([`THROW_GRENADE`]). The fuse is the game's (`ObjectDesc::fuse`), else 4 s (real
//!   UZRGM and M67 fuses, HL:A and Metro Awakening's). Two ways to arm, a setting:
//!   - *pin* (the default, the realistic scheme of Pavlov, H3VR and Into the Radius): the other hand
//!     takes the pin ([`THROW_PIN`]: its ring at `handle`) and pulls it `handle_travel` (5 cm)
//!     away. With a lever ([`THROW_LEVER`]) the spoon stays held while the grip is: the fuse starts
//!     when you let go (a throw or a drop) or when B pops it in your hand (cooking). Without one (a
//!     lit fuse, Metro's dynamite) it starts as the pin comes out;
//!   - *button* (one-handed, HL:A's priming button): the trigger or A on the holding hand arms it
//!     and the fuse starts. It's also how a pinned grenade arms while the other hand holds a gun
//!     (never two hands when one is busy), and how one without a pin arms.
//!
//!   An armed grenade can be held (cooked), re-gripped or caught: the fuse keeps running. It ticks
//!   (a sound, and a faint pulse in the hand holding it) once a second, four times a second in the
//!   last 1.5 s. It can't be stowed. When the fuse runs out, or on impact for impact grenades
//!   ([`THROW_IMPACT`], after `impact_delay`, only once thrown), it goes off: `ObjectState::shots`
//!   counts it, it's reported spent ([`OS_SPENT`]) and inert until the game respawns it.
//!   [`THROW_GAME_FUSE`] leaves the timing to the game.
//! * **Throwing knives** ([`THROW_KNIFE`]: the blade from `grip` to its tip at `muzzle`). At the
//!   release the spin about the blade's flat picks the flight: past 8 rad/s it *spins* end over end
//!   (that spin only, at most 40 rad/s, nudged so the tip comes round to the front where it's going
//!   to hit), otherwise it flies as a *dart*: turned tip-first once, then kept pointing along its
//!   flight (16/s, Meta's dart) with almost no spin. It sticks if the tip meets something at
//!   1 m/s or more within 60 degrees of its flight (HVR's stab test; 72 with the assist on). A
//!   spinning knife is judged in its tip-forward pose whenever its tip is within 105 degrees of the
//!   front (the spin is for show, the hit uses the tip). A stuck
//!   knife ([`OS_STUCK`], `parent` the object it's in) rides with what it's in and can be pulled out.
//! * **Sticky** throwables ([`THROW_STICKY`]) stick to whatever they hit once thrown.
//! * Anything else of kind [`OBJ_THROWABLE`] throws like any prop.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use super::throwing::ThrowClass;

pub const DEFAULT_FUSE: f32 = 4.0;
pub const DEFAULT_PIN_TRAVEL: f32 = 0.05;
/// The other hand takes the pin within this of its ring.
pub const PIN_REACH: f32 = 0.08;
/// Fuse ticks: every second, then every quarter second for the last `FAST_FOR` seconds.
pub const TICK_SLOW: f32 = 1.0;
pub const TICK_FAST: f32 = 0.25;
pub const FAST_FOR: f32 = 1.5;
/// An impact fuse goes off on a change of velocity this big.
pub const IMPACT_SPEED: f32 = 2.0;
/// Knives: spin about the blade's flat past this (rad/s) flies end over end.
pub const KNIFE_SPIN_MODE: f32 = 8.0;
/// A dart turns its tip towards its flight at this rate (1/s), and keeps no more spin than this.
pub const KNIFE_ALIGN_RATE: f32 = 16.0;
pub const KNIFE_DART_SPIN: f32 = 5.0;
/// The stick test: at least this fast, the tip within this of the flight (cosines).
pub const STICK_SPEED: f32 = 1.0;
pub const STICK_COS: f32 = 0.5;
pub const STICK_COS_ASSIST: f32 = 0.3;
/// A spinning knife's tip counts as forward within 90 + 15 degrees of its flight.
pub const SPIN_WINDOW_COS: f32 = -0.2588;
/// How deep a stuck blade goes in past the surface.
pub const STICK_DEPTH: f32 = 0.025;
/// A throw's flight assist (alignment, the stick sweep) gives up after this long.
pub const FLIGHT_TIME: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fuse {
    /// The pin is in.
    Safe,
    /// The pin is out but the lever is held: nothing's running yet.
    PinOut,
    /// Burning since then.
    Lit { at: f64 },
    /// It went off.
    Spent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KnifeMode {
    Dart,
    /// End over end at this rate (rad/s) about the blade's flat.
    Spin(f32),
}

/// A throw in the air (assisted until it lands, sticks or gives up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    pub hand: usize,
    pub t0: f64,
    pub knife: Option<KnifeMode>,
}

#[derive(Clone, Debug)]
pub struct ThrowState {
    pub flags: u32, // THROW_*
    pub fuse: Fuse,
    /// Seconds from lighting to going off (0: none, it waits for an impact or the game).
    pub fuse_len: f32,
    pub impact_delay: f32,
    /// When it was last thrown (cleared once it comes to rest).
    pub thrown_at: Option<f64>,
    pub flight: Option<Flight>,
    next_tick: f64,
    /// Times it went off (`ObjectState::shots`).
    pub detonations: u32,
    /// Object space: the blade's axis (grip to tip) and its flat's (the spin axis).
    pub blade: Vec3,
    pub flat: Vec3,
    pub tip: Vec3,
}

impl ThrowState {
    pub fn new(d: &ObjectDesc) -> Option<ThrowState> {
        if d.kind != OBJ_THROWABLE {
            return None;
        }
        let impact = d.throw_flags & THROW_IMPACT != 0;
        let fuse_len = if d.throw_flags & THROW_GAME_FUSE != 0 {
            0.0
        } else if d.fuse > 0.0 && d.fuse.is_finite() {
            d.fuse
        } else if impact || d.fuse < 0.0 {
            0.0
        } else {
            DEFAULT_FUSE
        };
        let tip = Vec3::from_array(d.muzzle);
        let blade = (tip - Vec3::from_array(d.grip)).normalize_or(Vec3::Z);
        let flat = (Vec3::X - blade * blade.x).normalize_or(Vec3::Y);
        Some(ThrowState {
            flags: d.throw_flags,
            fuse: Fuse::Safe,
            fuse_len,
            impact_delay: d.impact_delay.max(0.0),
            thrown_at: None,
            flight: None,
            next_tick: 0.0,
            detonations: 0,
            blade,
            flat,
            tip,
        })
    }

    pub fn is_grenade(&self) -> bool {
        self.flags & THROW_GRENADE != 0
    }

    pub fn is_knife(&self) -> bool {
        self.flags & THROW_KNIFE != 0
    }

    pub fn is_sticky(&self) -> bool {
        self.flags & THROW_STICKY != 0
    }

    pub fn has_pin(&self) -> bool {
        self.is_grenade() && self.flags & THROW_PIN != 0
    }

    pub fn class(&self) -> ThrowClass {
        if self.is_grenade() {
            ThrowClass::Grenade
        } else if self.is_knife() {
            ThrowClass::Knife
        } else {
            ThrowClass::Generic
        }
    }

    pub fn lit(&self) -> bool {
        matches!(self.fuse, Fuse::Lit { .. })
    }

    /// The pin is out (lit or with its lever held).
    pub fn pin_out(&self) -> bool {
        matches!(self.fuse, Fuse::PinOut | Fuse::Lit { .. })
    }

    pub fn spent(&self) -> bool {
        self.fuse == Fuse::Spent
    }

    /// May go into a body slot: never armed (or with its pin out).
    pub fn stowable(&self) -> bool {
        !self.pin_out() && !self.spent()
    }

    /// Seconds left on a burning fuse the runtime times (0 otherwise).
    pub fn fuse_left(&self, now: f64) -> f32 {
        match self.fuse {
            Fuse::Lit { at } if self.fuse_len > 0.0 => (self.fuse_len - (now - at) as f32).max(0.0),
            _ => 0.0,
        }
    }

    fn light(&mut self, now: f64) {
        self.fuse = Fuse::Lit { at: now };
        self.next_tick = now;
    }

    /// The pin came out. True if that lit the fuse (no lever to hold).
    pub fn pull_pin(&mut self, now: f64) -> bool {
        if self.fuse != Fuse::Safe || !self.is_grenade() {
            return false;
        }
        if self.flags & THROW_LEVER != 0 {
            self.fuse = Fuse::PinOut;
            false
        } else {
            self.light(now);
            true
        }
    }

    /// Armed in the hand: the one-handed button, or B popping a held lever (cooking). True if the
    /// fuse lit.
    pub fn arm(&mut self, now: f64) -> bool {
        if !self.is_grenade() {
            return false;
        }
        match self.fuse {
            Fuse::Safe | Fuse::PinOut => {
                self.light(now);
                true
            }
            _ => false,
        }
    }

    /// Let go of (thrown or dropped): a held lever flies off and the fuse starts. True if it lit.
    pub fn let_go(&mut self, now: f64) -> bool {
        if self.fuse == Fuse::PinOut {
            self.light(now);
            true
        } else {
            false
        }
    }

    /// The runtime's fuse has run out.
    pub fn due(&self, now: f64) -> bool {
        matches!(self.fuse, Fuse::Lit { at } if self.fuse_len > 0.0 && now - at >= self.fuse_len as f64)
    }

    /// An impact grenade hit something hard enough after its delay.
    pub fn impact_due(&self, now: f64, change: f32) -> bool {
        self.flags & THROW_IMPACT != 0
            && self.flags & THROW_GAME_FUSE == 0
            && self.lit()
            && self.thrown_at.is_some_and(|t| now - t >= self.impact_delay as f64)
            && change >= IMPACT_SPEED
    }

    /// A tick is due: its intensity (louder near the end).
    pub fn tick(&mut self, now: f64) -> Option<f32> {
        let Fuse::Lit { at } = self.fuse else { return None };
        if now + 1e-6 < self.next_tick {
            return None;
        }
        let left = self.fuse_left(now);
        let fast = self.fuse_len > 0.0 && left <= FAST_FOR + 1e-4;
        let mut next = now + if fast { TICK_FAST } else { TICK_SLOW } as f64;
        if !fast && self.fuse_len > 0.0 {
            // The fast ticks start on time, not at the next slow one.
            let fast_from = at + (self.fuse_len - FAST_FOR) as f64;
            next = next.min(fast_from.max(now + TICK_FAST as f64));
        }
        self.next_tick = next;
        Some(if fast { 1.0 } else { 0.6 })
    }

    pub fn went_off(&mut self) {
        self.fuse = Fuse::Spent;
        self.detonations += 1;
        self.flight = None;
        self.thrown_at = None;
    }

    /// The game put it back (respawned it): safe again.
    pub fn reset(&mut self) {
        self.fuse = Fuse::Safe;
        self.flight = None;
        self.thrown_at = None;
    }

    pub fn flags_out(&self) -> u32 {
        let mut f = 0;
        if self.lit() {
            f |= OS_ARMED;
        }
        if self.pin_out() {
            f |= OS_PIN_PULLED;
        }
        if self.thrown_at.is_some() {
            f |= OS_THROWN;
        }
        if self.spent() {
            f |= OS_SPENT;
        }
        f
    }
}

/// How a knife leaves the hand, from its spin (world) about its flat (world).
pub fn knife_mode(ang: Vec3, flat: Vec3) -> KnifeMode {
    let about = ang.dot(flat);
    if about.abs() > KNIFE_SPIN_MODE { KnifeMode::Spin(about.clamp(-super::throwing::MAX_SPIN_KNIFE, super::throwing::MAX_SPIN_KNIFE)) } else { KnifeMode::Dart }
}

/// The spin near `rate` (same way round) that brings the tip round to the front after `time`
/// seconds, from `angle` (radians, the tip's angle from its flight about the flat, in the spin's
/// sense): exactly tip first when that's a change of no more than half, else just inside the
/// tip-forward window (75 of its 105 degrees), whichever is the smaller change.
pub fn spin_to_land(rate: f32, angle: f32, time: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if time <= 0.05 || rate.abs() < 1e-3 {
        return rate;
    }
    let phi = rate * time;
    let end = (angle + phi + PI).rem_euclid(TAU) - PI;
    let same_way = |p: f32| p.signum() == phi.signum() && p.abs() > 1e-3;
    // Exactly tip first, a whole number of turns.
    let exact = [-TAU, 0.0, TAU].map(|k| phi + k - end).into_iter().filter(|p| same_way(*p) && (p - phi).abs() <= phi.abs() * 0.5).min_by(|a, b| (a - phi).abs().total_cmp(&(b - phi).abs()));
    if let Some(p) = exact {
        return p / time;
    }
    let window = 75f32.to_radians();
    if end.abs() <= window {
        return rate;
    }
    let mut best: Option<f32> = None;
    for target in [window, -window] {
        for k in [-TAU, 0.0, TAU] {
            let p = phi + target + k - end;
            if same_way(p) && best.is_none_or(|b| (p - phi).abs() < (b - phi).abs()) {
                best = Some(p);
            }
        }
    }
    best.map(|p| p / time).unwrap_or(rate)
}

/// Does the blade go in? `tip`: the tip's direction (world), `v` the velocity just before the hit.
/// A spinning knife is judged tip-forward whenever its tip is round to the front.
pub fn sticks(tip: Vec3, v: Vec3, mode: Option<KnifeMode>, assist: bool) -> bool {
    let speed = v.length();
    if speed < STICK_SPEED {
        return false;
    }
    let cos = tip.normalize_or_zero().dot(v / speed);
    match mode {
        Some(KnifeMode::Spin(_)) if cos >= SPIN_WINDOW_COS => true,
        _ => cos >= if assist { STICK_COS_ASSIST } else { STICK_COS },
    }
}

/// The angular velocity that keeps a dart's tip on its flight: turn `tip` (world) towards `v`
/// at [`KNIFE_ALIGN_RATE`], keeping no more than [`KNIFE_DART_SPIN`] of the spin about the blade.
pub fn dart_spin(tip: Vec3, v: Vec3, ang: Vec3) -> Vec3 {
    let (t, d) = (tip.normalize_or_zero(), v.normalize_or_zero());
    if d == Vec3::ZERO || t == Vec3::ZERO {
        return ang;
    }
    let roll = (ang.dot(t)).clamp(-KNIFE_DART_SPIN, KNIFE_DART_SPIN);
    let q = Quat::from_rotation_arc(t, d);
    let (axis, angle) = q.to_axis_angle();
    let turn = if angle.is_finite() && axis.is_finite() { axis * angle * KNIFE_ALIGN_RATE } else { Vec3::ZERO };
    turn + t * roll
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn grenade(flags: u32, fuse: f32) -> ThrowState {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_THROWABLE;
        d.throw_flags = THROW_GRENADE | flags;
        d.fuse = fuse;
        ThrowState::new(&d).unwrap()
    }

    #[test]
    fn the_fuse_is_the_games_else_four_seconds() {
        assert_eq!(grenade(0, 0.0).fuse_len, DEFAULT_FUSE);
        assert_eq!(grenade(0, 3.2).fuse_len, 3.2);
        assert_eq!(grenade(THROW_IMPACT, 0.0).fuse_len, 0.0, "impact grenades wait for the hit");
        assert_eq!(grenade(THROW_GAME_FUSE, 3.0).fuse_len, 0.0, "the game times it");
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_PROP;
        assert!(ThrowState::new(&d).is_none());
    }

    #[test]
    fn a_lever_holds_the_fuse_until_let_go_or_cooked() {
        let mut g = grenade(THROW_PIN | THROW_LEVER, 4.0);
        assert!(g.stowable());
        assert!(!g.pull_pin(1.0), "the spoon is held");
        assert_eq!(g.fuse, Fuse::PinOut);
        assert!(!g.stowable(), "never stowed with the pin out");
        assert!(!g.due(100.0), "nothing runs while the lever's held");
        assert!(g.let_go(2.0), "letting go starts it");
        assert!(!g.due(5.9) && g.due(6.0), "four seconds from the release");
        assert!((g.fuse_left(5.0) - 1.0).abs() < 1e-5);
        // Cooking: B pops the lever in the hand.
        let mut c = grenade(THROW_PIN | THROW_LEVER, 4.0);
        c.pull_pin(0.0);
        assert!(c.arm(1.0) && c.lit());
        assert!(!c.let_go(2.0), "already burning");
        assert!(c.due(5.0));
    }

    #[test]
    fn a_lit_fuse_starts_with_the_pin_and_ticks_faster_at_the_end() {
        let mut g = grenade(THROW_PIN, 4.0);
        assert!(g.pull_pin(10.0), "no lever: pulling it lights it");
        let mut ticks = Vec::new();
        let mut t = 10.0;
        while t < 14.0 {
            if g.tick(t).is_some() {
                ticks.push(t - 10.0);
            }
            t += 1.0 / 90.0;
        }
        let slow = ticks.iter().filter(|t| **t < 2.4).count();
        let fast = ticks.iter().filter(|t| **t >= 2.5).count();
        assert!((2..=3).contains(&slow) && fast >= 5, "ticks at {ticks:?}");
        g.went_off();
        assert!(g.spent() && g.detonations == 1 && g.tick(15.0).is_none());
        assert!(g.flags_out() & OS_SPENT != 0);
        g.reset();
        assert_eq!(g.fuse, Fuse::Safe);
    }

    #[test]
    fn impact_grenades_need_a_throw_and_a_hard_hit() {
        let mut g = grenade(THROW_PIN | THROW_IMPACT, 0.0);
        g.impact_delay = 0.2;
        g.pull_pin(0.0);
        assert!(!g.impact_due(1.0, 5.0), "dropped, not thrown");
        g.thrown_at = Some(1.0);
        assert!(!g.impact_due(1.1, 5.0), "too soon");
        assert!(!g.impact_due(1.5, 1.0), "too soft");
        assert!(g.impact_due(1.5, 5.0));
        let mut safe = grenade(THROW_IMPACT, 0.0);
        safe.thrown_at = Some(0.0);
        assert!(!safe.impact_due(5.0, 9.0), "never before it's armed");
    }

    #[test]
    fn knives_spin_or_fly_as_darts() {
        let flat = Vec3::X;
        assert_eq!(knife_mode(Vec3::new(3.0, 1.0, 0.0), flat), KnifeMode::Dart);
        assert_eq!(knife_mode(Vec3::new(-12.0, 0.0, 0.0), flat), KnifeMode::Spin(-12.0));
        assert_eq!(knife_mode(Vec3::new(90.0, 0.0, 0.0), flat), KnifeMode::Spin(40.0), "capped");
        // Quantised so the tip comes round: from 0 rad at 12 rad/s over 0.5 s (6 rad) -> one turn.
        let w = spin_to_land(12.0, 0.0, 0.5);
        assert!((w * 0.5 - std::f32::consts::TAU).abs() < 1e-4, "{w}");
        assert_eq!(spin_to_land(12.0, 0.0, 0.01), 12.0, "nothing to do for an instant hit");
        // 139 degrees round by the hit, too far to make a whole turn: slowed to just inside the
        // window, the same way round.
        let w = spin_to_land(-13.5, 0.0, 0.18);
        assert!(w < 0.0 && (w * 0.18).abs() <= 75f32.to_radians() + 1e-3 && w > -9.0, "{w}");
    }

    #[test]
    fn the_stick_test_wants_the_tip_first() {
        let v = Vec3::new(0.0, 0.0, 8.0);
        assert!(sticks(Vec3::Z, v, Some(KnifeMode::Dart), false));
        assert!(!sticks(Vec3::new(0.0, 1.0, 0.3), v, Some(KnifeMode::Dart), false), "flat on");
        assert!(sticks(Vec3::new(0.0, 1.0, 0.4), v, Some(KnifeMode::Dart), true), "the assist forgives 72 degrees");
        assert!(!sticks(-Vec3::Z, v, Some(KnifeMode::Dart), true), "handle first");
        assert!(!sticks(Vec3::Z, Vec3::new(0.0, 0.0, 0.5), Some(KnifeMode::Dart), true), "too slow");
        // Spinning: anywhere round the front counts (the tip-forward pose), never the back.
        assert!(sticks(Vec3::new(0.0, 1.0, 0.1), v, Some(KnifeMode::Spin(20.0)), false));
        assert!(!sticks(Vec3::new(0.0, 0.3, -1.0), v, Some(KnifeMode::Spin(20.0)), false));
    }

    #[test]
    fn a_dart_turns_its_tip_into_its_flight() {
        let w = dart_spin(Vec3::Y, Vec3::Z, Vec3::new(0.0, 30.0, 0.0));
        // Y -> Z is a turn about +X; the 30 rad/s roll about the blade is cut to 5.
        assert!(w.x > 20.0 && (w.y - KNIFE_DART_SPIN).abs() < 1e-4, "{w:?}");
    }
}
