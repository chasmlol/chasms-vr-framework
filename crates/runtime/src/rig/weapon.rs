//! A gun's own state, owned by the runtime: magazine, chamber, slide, shots and recoil.
//!
//! Chamber state is independent of the magazine (Alyx): pulling the magazine leaves a round
//! chambered, firing the last round locks the slide back on guns that do, and a fresh magazine
//! needs a rack (or a slide release) before the next shot.

use glam::Vec3;
use rustvr_protocol::*;

pub use super::recoil::Recoil;


#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandleGrab {
    pub hand: usize,
    /// Where the hand was (world) when it took hold.
    pub start: Vec3,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fire {
    None,
    Shot,
    /// Last round in the magazine just went.
    LastShot,
    Empty,
}

impl WeaponState {
    pub fn new(chambered: bool) -> WeaponState {
        WeaponState { chambered, ..Default::default() }
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
        if !self.chambered || self.slide_locked || self.handle.is_some() {
            // A dry click only on the press, not every auto cycle.
            return if pressed { Fire::Empty } else { Fire::None };
        }
        let rate = if d.fire_rate > 0.0 { d.fire_rate } else { 8.0 };
        self.next_shot = now + 1.0 / rate as f64;
        self.shots += 1;
        self.chambered = false;
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

    /// The hand let go of the handle: it springs forward.
    pub fn let_go_handle(&mut self) -> bool {
        self.handle = None;
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
}
