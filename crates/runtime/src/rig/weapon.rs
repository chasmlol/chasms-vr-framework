//! A gun's own state, owned by the runtime: magazine, chamber, slide, shots and recoil, and the
//! action of a gun that doesn't cycle itself (`super::actions`).
//!
//! Chamber state is independent of the magazine (Alyx): pulling the magazine leaves a round
//! chambered, firing the last round locks the slide back on guns that do, and a fresh magazine
//! needs a rack (or a slide release) before the next shot. A bolt or pump action leaves the case
//! in the chamber until it's worked; a gun loaded into its chambers fires them in order; a hinged
//! action fires only shut; an air gun only with pressure.

use glam::Vec3;
use rustvr_protocol::*;

use super::actions::{Air, Chamber, Cycle, Hinge, chamber_bits};
pub use super::recoil::Recoil;


#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandleGrab {
    pub hand: usize,
    /// Where the hand was (world) when it took hold.
    pub start: Vec3,
    /// Where the knob is from the hand (weapon space): the handle of a bolt, a pump or an air gun's
    /// pump goes where the hand takes it however the gun moves.
    pub offset: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct WeaponState {
    /// Seated magazine (object id).
    pub mag: Option<u32>,
    /// Guns without a magazine object: rounds held internally.
    pub internal: Option<u32>,
    pub chambered: bool,
    /// 0 forward .. 1 fully back.
    pub slide: f32,
    pub slide_locked: bool,
    pub shots: u32,
    next_shot: f64,
    trigger_was: bool,
    pub recoil: Recoil,
    /// The game's own recoil, when it gave one (see [`super::kick`]).
    pub game: super::kick::GameRecoil,
    pub handle: Option<HandleGrab>,
    racked_this_pull: bool,
    /// Bolt and pump actions: a spent case sits in the chamber (until the action is worked).
    pub spent: bool,
    /// Guns loaded round by round into their chambers ([`WPN_LOADS_CHAMBERS`]): each chamber.
    pub chambers: Vec<Chamber>,
    /// Break and swing actions: the barrels (or the cylinder) on their hinge.
    pub hinge: Option<Hinge>,
    /// Bolt and pump actions: the handle along its path.
    pub cycle: Option<Cycle>,
    /// Air guns: the pressure; and the last shot's strength (the pressure it was fired at).
    pub air: Option<Air>,
    pub last_strength: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fire {
    None,
    Shot,
    /// Last round in the magazine just went.
    LastShot,
    Empty,
    /// An air gun without the pressure to fire: it hisses.
    NoAir,
}

impl WeaponState {
    pub fn new(chambered: bool) -> WeaponState {
        WeaponState { chambered, ..Default::default() }
    }

    /// A gun as it's handed over: chambered (or every chamber loaded), its action shut; an air gun
    /// without air (it has to be pumped).
    pub fn for_desc(d: &ObjectDesc) -> WeaponState {
        let mut w = WeaponState::new(true);
        if d.weapon_flags & WPN_LOADS_CHAMBERS != 0 {
            w.chambers = vec![Chamber::Live; d.capacity.clamp(1, 16) as usize];
            w.chambered = false;
        }
        // A revolver's chambers are its cylinder's, the speed loader's rounds: the one under the
        // hammer is chambered as the loader goes in, not as well as them.
        if d.action == ACTION_SWING && d.weapon_flags & WPN_LOADS_CHAMBERS == 0 {
            w.chambered = false;
        }
        match d.action {
            ACTION_BREAK => w.hinge = Some(Hinge::shut()),
            ACTION_SWING => w.hinge = Some(Hinge::swing_out()),
            ACTION_BOLT | ACTION_PUMP => w.cycle = Some(Cycle::default()),
            _ => {}
        }
        if d.weapon_flags & WPN_PNEUMATIC != 0 {
            w.air = Some(Air::default());
        }
        w
    }

    /// The action is shut and locked: it can fire.
    pub fn action_closed(&self) -> bool {
        self.hinge.is_none_or(|h| h.latched) && self.cycle.is_none_or(|c| c.closed())
    }

    /// `ObjectState::chambered`: the chambers' live rounds and spent cases.
    pub fn chamber_bits(&self) -> u32 {
        if !self.chambers.is_empty() {
            return chamber_bits(&self.chambers);
        }
        self.chambered as u32 | (self.spent as u32) << CHAMBER_SPENT_SHIFT
    }

    /// A live round in any chamber.
    pub fn any_live(&self) -> bool {
        self.chambered || self.chambers.contains(&Chamber::Live)
    }

    /// Trigger handling for one frame. `rounds` is the seated magazine's (or internal) count.
    pub fn trigger(&mut self, d: &ObjectDesc, pull: f32, now: f64, rounds: &mut u32) -> Fire {
        let down = pull > if self.trigger_was { 0.4 } else { 0.6 };
        let pressed = down && !self.trigger_was;
        self.trigger_was = down;
        if !down {
            return Fire::None;
        }
        let auto = d.weapon_flags & WPN_AUTOMATIC != 0;
        if !(pressed || auto) || now < self.next_shot {
            return Fire::None;
        }
        // An open action doesn't fire (the trigger does nothing at all).
        if !self.action_closed() {
            return Fire::None;
        }
        let rate = if d.fire_rate > 0.0 { d.fire_rate } else { 8.0 };
        // Loaded into its chambers: the next live one fires, and its case stays.
        if !self.chambers.is_empty() {
            let Some(k) = self.chambers.iter().position(|c| *c == Chamber::Live) else {
                return if pressed { Fire::Empty } else { Fire::None };
            };
            self.next_shot = now + 1.0 / rate as f64;
            self.shots += 1;
            self.chambers[k] = Chamber::Spent;
            return if self.chambers.contains(&Chamber::Live) { Fire::Shot } else { Fire::LastShot };
        }
        let manual = self.cycle.is_some();
        // A hand on the charging handle holds the gun off (an air gun's pump only while it's out).
        let racking = self.handle.is_some() && !manual && !(self.air.is_some() && self.slide < 0.1);
        if !self.chambered || self.slide_locked || racking {
            // A dry click only on the press, not every auto cycle.
            return if pressed { Fire::Empty } else { Fire::None };
        }
        // An air gun needs its pressure; a shot's strength is the pressure it's fired at.
        if let Some(air) = &mut self.air {
            match air.shoot(d.shot_pressure, d.min_pressure) {
                Some(strength) => self.last_strength = strength,
                None => return if pressed { Fire::NoAir } else { Fire::None },
            }
        }
        self.next_shot = now + 1.0 / rate as f64;
        self.shots += 1;
        self.chambered = false;
        if manual {
            // Worked by hand: the case stays until the bolt or the pump takes it out.
            self.spent = true;
            return if *rounds == 0 { Fire::LastShot } else { Fire::Shot };
        }
        if *rounds > 0 {
            *rounds -= 1;
            self.chambered = true;
            if *rounds == 0 { Fire::LastShot } else { Fire::Shot }
        } else {
            if d.weapon_flags & WPN_SLIDE_LOCKS != 0 {
                self.slide_locked = true;
                self.slide = 1.0;
            }
            Fire::LastShot
        }
    }

    /// A magazine went in. Returns true if that also chambered a round (slide release by
    /// slamming the magazine, or guns that don't need racking).
    pub fn seated(&mut self, d: &ObjectDesc, rounds: &mut u32, slam_release: bool) -> bool {
        let release = self.slide_locked && slam_release;
        let auto_chamber = d.weapon_flags & WPN_NEEDS_RACK == 0 && !self.chambered;
        if (release || auto_chamber) && *rounds > 0 && !self.chambered {
            *rounds -= 1;
            self.chambered = true;
            self.slide_locked = false;
            self.slide = 0.0;
            return true;
        }
        false
    }

    /// Slide release button: drops a locked slide, chambering from the magazine.
    pub fn release_slide(&mut self, rounds: &mut u32) -> bool {
        if !self.slide_locked {
            return false;
        }
        self.slide_locked = false;
        self.slide = 0.0;
        if *rounds > 0 && !self.chambered {
            *rounds -= 1;
            self.chambered = true;
        }
        true
    }

    /// The charging handle / slide is at `pull` metres back (of `travel`). Returns (racked now,
    /// returned forward now).
    pub fn pull_handle(&mut self, pull: f32, travel: f32, rounds: &mut u32) -> (bool, bool) {
        let travel = if travel > 0.0 { travel } else { 0.06 };
        let s = (pull / travel).clamp(0.0, 1.0);
        let was = self.slide;
        self.slide = if self.slide_locked { s.max(1.0) } else { s };
        let mut racked = false;
        let mut forward = false;
        // Auto Hand's slider: the end event fires at 90% of travel.
        if s >= 0.9 && !self.racked_this_pull {
            self.racked_this_pull = true;
            racked = true;
            self.chambered = false; // a chambered round is ejected
            self.slide_locked = false;
            if *rounds > 0 {
                *rounds -= 1;
                self.chambered = true;
            }
        }
        if s < 0.2 && was >= 0.2 && self.racked_this_pull {
            self.racked_this_pull = false;
            forward = true;
        }
        (racked, forward)
    }

    /// A bolt or pump action's handle, where the hand has it (`lift`, `back`: 0..1, see
    /// [`Cycle::drive`]): at the back of the stroke whatever is in the chamber flies out (returned
    /// as `Some(live)`: a live round or, false, a spent case), and on the way home the next round
    /// goes in from the magazine. Pushes what happened onto `events`.
    pub fn work(&mut self, lift: f32, back: f32, lifts: bool, rounds: &mut u32, events: &mut Vec<super::actions::CycleEvent>) -> Option<bool> {
        let Some(c) = &mut self.cycle else { return None };
        let from = events.len();
        c.drive(lift, back, lifts, events);
        self.slide = c.back;
        let mut out = None;
        for e in events[from..].iter() {
            match e {
                super::actions::CycleEvent::Back => {
                    if self.chambered || self.spent {
                        out = Some(self.chambered);
                    }
                    self.chambered = false;
                    self.spent = false;
                }
                super::actions::CycleEvent::Home => {
                    if !self.chambered && *rounds > 0 {
                        *rounds -= 1;
                        self.chambered = true;
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// An air gun's pump at `back` (0 home .. 1 out): a whole stroke adds `gain`. True at the end of
    /// a stroke.
    pub fn pump(&mut self, back: f32, gain: f32) -> bool {
        self.slide = back.clamp(0.0, 1.0);
        self.air.as_mut().is_some_and(|a| a.pump(back, gain))
    }

    /// The cases in the chambers fly out (the action opened): which chambers they were in.
    pub fn throw_out_cases(&mut self) -> Vec<usize> {
        let mut out = Vec::new();
        for (k, c) in self.chambers.iter_mut().enumerate() {
            if *c == Chamber::Spent {
                *c = Chamber::Empty;
                out.push(k);
            }
        }
        out
    }

    /// A round goes into chamber `k`, if it's empty and the action is open.
    pub fn load_chamber(&mut self, k: usize) -> bool {
        let open = self.hinge.is_some_and(|h| !h.latched) || self.cycle.is_some_and(|c| !c.closed());
        match self.chambers.get_mut(k) {
            Some(c) if *c == Chamber::Empty && open => {
                *c = Chamber::Live;
                true
            }
            _ => false,
        }
    }

    /// The hand let go of the handle: it springs forward.
    pub fn let_go_handle(&mut self) -> bool {
        self.handle = None;
        // A bolt or a pump stays where the hand left it (no spring); an air gun's pump goes home.
        if self.cycle.is_some() {
            return false;
        }
        if let Some(air) = &mut self.air {
            // Let go of out, the pump goes home without its stroke.
            air.abandon();
            self.slide = 0.0;
            return false;
        }
        let forward = self.racked_this_pull || self.slide > 0.2;
        self.racked_this_pull = false;
        if !self.slide_locked {
            self.slide = 0.0;
        }
        forward
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn pistol() -> ObjectDesc {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.weapon_flags = WPN_SLIDE_LOCKS | WPN_NEEDS_RACK | WPN_PISTOL;
        d.fire_rate = 10.0;
        d.recoil_pitch = 8.0;
        d.recoil_back = 0.01;
        d
    }

    #[test]
    fn semi_auto_fires_once_per_pull_and_locks_on_empty() {
        let d = pistol();
        let mut w = WeaponState { chambered: true, ..Default::default() };
        let mut rounds = 2;
        assert_eq!(w.trigger(&d, 1.0, 0.0, &mut rounds), Fire::Shot);
        assert_eq!(w.trigger(&d, 1.0, 0.2, &mut rounds), Fire::None, "held: no second shot");
        w.trigger(&d, 0.0, 0.3, &mut rounds);
        assert_eq!(w.trigger(&d, 1.0, 0.4, &mut rounds), Fire::LastShot);
        assert_eq!(rounds, 0);
        assert!(w.chambered, "the last round from the magazine is now chambered");
        w.trigger(&d, 0.0, 0.5, &mut rounds);
        assert_eq!(w.trigger(&d, 1.0, 0.6, &mut rounds), Fire::LastShot, "the chambered round fires");
        assert!(w.slide_locked);
        w.trigger(&d, 0.0, 0.7, &mut rounds);
        assert_eq!(w.trigger(&d, 1.0, 0.8, &mut rounds), Fire::Empty);
        assert_eq!(w.shots, 3);
    }

    #[test]
    fn fresh_magazine_and_slide_release() {
        let d = pistol();
        let mut w = WeaponState { slide_locked: true, ..Default::default() };
        let mut rounds = 12;
        assert!(!w.seated(&d, &mut rounds, false), "no slam release: still locked");
        assert!(w.release_slide(&mut rounds));
        assert!(w.chambered && rounds == 11);
        let mut w = WeaponState { slide_locked: true, ..Default::default() };
        let mut rounds = 12;
        assert!(w.seated(&d, &mut rounds, true), "slamming it in drops the slide");
    }

    #[test]
    fn racking_chambers_once_per_pull() {
        let mut w = WeaponState::default();
        let mut rounds = 5;
        let (r, _) = w.pull_handle(0.03, 0.06, &mut rounds);
        assert!(!r);
        let (r, _) = w.pull_handle(0.058, 0.06, &mut rounds);
        assert!(r && w.chambered && rounds == 4);
        let (r, _) = w.pull_handle(0.06, 0.06, &mut rounds);
        assert!(!r, "holding it back doesn't rack again");
        let (_, fwd) = w.pull_handle(0.0, 0.06, &mut rounds);
        assert!(fwd);
        let (r, _) = w.pull_handle(0.06, 0.06, &mut rounds);
        assert!(r && rounds == 3, "a second pull ejects the chambered round and chambers another");
    }

    fn bolt_rifle() -> ObjectDesc {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.action = ACTION_BOLT;
        d.action_angle = 1.2;
        d.handle_travel = 0.08;
        d.weapon_flags = WPN_NEEDS_RACK | WPN_STOCK;
        d.fire_rate = 2.0;
        d
    }

    #[test]
    fn a_bolt_action_fires_once_then_needs_its_bolt_worked() {
        use crate::rig::actions::CycleEvent as E;
        let d = bolt_rifle();
        let mut w = WeaponState::for_desc(&d);
        let mut rounds = 4;
        assert_eq!(w.trigger(&d, 1.0, 0.0, &mut rounds), Fire::Shot);
        w.trigger(&d, 0.0, 1.0, &mut rounds);
        assert_eq!(w.trigger(&d, 1.0, 1.1, &mut rounds), Fire::Empty, "the case is still in the chamber");
        assert!(w.spent && !w.chambered && rounds == 4);
        assert_eq!(w.chamber_bits(), 1 << CHAMBER_SPENT_SHIFT);
        w.trigger(&d, 0.0, 1.2, &mut rounds);
        let mut ev = Vec::new();
        assert_eq!(w.work(1.0, 0.0, true, &mut rounds, &mut ev), None, "lifted");
        assert_eq!(w.trigger(&d, 1.0, 1.3, &mut rounds), Fire::None, "a lifted bolt doesn't fire");
        w.trigger(&d, 0.0, 1.4, &mut rounds);
        assert_eq!(w.work(1.0, 1.0, true, &mut rounds, &mut ev), Some(false), "back: the case flies out");
        assert!(!w.spent && w.slide == 1.0);
        assert_eq!(w.work(1.0, 0.0, true, &mut rounds, &mut ev), None);
        assert!(w.chambered && rounds == 3, "home: the next round went in");
        w.work(0.0, 0.0, true, &mut rounds, &mut ev);
        assert_eq!(ev, vec![E::Unlocked, E::Back, E::Home, E::Locked]);
        assert_eq!(w.trigger(&d, 1.0, 1.5, &mut rounds), Fire::Shot, "worked, it fires again");
        assert_eq!(w.shots, 2);
    }

    #[test]
    fn a_double_barrel_fires_each_chamber_once_and_reloads_them_open() {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.action = ACTION_BREAK;
        d.action_angle = 1.0;
        d.capacity = 2;
        d.weapon_flags = WPN_LOADS_CHAMBERS;
        d.fire_rate = 5.0;
        let mut w = WeaponState::for_desc(&d);
        let mut none = 0;
        assert_eq!(w.chamber_bits(), 0b11, "arrives with both barrels loaded");
        assert_eq!(w.trigger(&d, 1.0, 0.0, &mut none), Fire::Shot, "one barrel per pull");
        w.trigger(&d, 0.0, 0.5, &mut none);
        assert_eq!(w.trigger(&d, 1.0, 1.0, &mut none), Fire::LastShot);
        w.trigger(&d, 0.0, 1.5, &mut none);
        assert_eq!(w.trigger(&d, 1.0, 2.0, &mut none), Fire::Empty, "both spent");
        assert_eq!(w.chamber_bits(), 0b11 << CHAMBER_SPENT_SHIFT);
        assert!(!w.load_chamber(0), "shut, nothing goes in");
        w.hinge.as_mut().unwrap().unlatch();
        assert_eq!(w.throw_out_cases(), vec![0, 1]);
        assert!(w.load_chamber(1) && !w.load_chamber(1), "one round per chamber");
        assert!(w.load_chamber(0));
        w.trigger(&d, 0.0, 2.5, &mut none);
        assert_eq!(w.trigger(&d, 1.0, 3.0, &mut none), Fire::None, "open, it doesn't fire");
        *w.hinge.as_mut().unwrap() = crate::rig::actions::Hinge::shut();
        w.trigger(&d, 0.0, 3.5, &mut none);
        assert_eq!(w.trigger(&d, 1.0, 4.0, &mut none), Fire::Shot);
        assert_eq!(w.shots, 3);
    }

    #[test]
    fn an_air_gun_hisses_until_it_is_pumped() {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.weapon_flags = WPN_PNEUMATIC | WPN_AUTOMATIC;
        d.pump_pressure = 0.25;
        d.shot_pressure = 0.1;
        d.min_pressure = 0.15;
        d.fire_rate = 6.0;
        let mut w = WeaponState::for_desc(&d);
        let mut rounds = 15;
        assert_eq!(w.trigger(&d, 1.0, 0.0, &mut rounds), Fire::NoAir, "no air, no shot");
        assert_eq!(w.shots, 0);
        w.trigger(&d, 0.0, 0.1, &mut rounds);
        for _ in 0..2 {
            w.pump(1.0, d.pump_pressure);
            assert!(w.pump(0.0, d.pump_pressure));
        }
        assert_eq!(w.trigger(&d, 1.0, 0.2, &mut rounds), Fire::Shot);
        assert!((w.last_strength - 0.5).abs() < 1e-6, "fired at half pressure");
        assert_eq!(w.trigger(&d, 1.0, 0.4, &mut rounds), Fire::Shot);
        assert_eq!(w.trigger(&d, 1.0, 0.6, &mut rounds), Fire::Shot);
        assert_eq!(w.trigger(&d, 1.0, 0.8, &mut rounds), Fire::Shot, "0.2 left: still enough");
        assert_eq!(w.trigger(&d, 1.0, 1.0, &mut rounds), Fire::None, "below the minimum on full auto: quiet");
        assert_eq!(w.shots, 4);
        assert!((w.last_strength - 0.2).abs() < 1e-5, "the last one weak");
    }
}
