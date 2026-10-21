//! Guns that don't cycle themselves: the moving parts the player's hands work (DESIGN.md §6b), the
//! way Half-Life: Alyx, H3VR and Pavlov do them.
//!
//! * **Hinged actions** ([`Hinge`]): a break action's barrels tipping down about their hinge pin,
//!   or a barrel or cylinder swinging out sideways about a pin along the bore. Unlatched (the thumb
//!   on the lever, a flick of the gun, the other hand pulling the barrels), they fall open under
//!   their own weight, a pendulum on the gun: how the gun is held decides where they hang (with the
//!   stock lifted up to them, the muzzle down and back, they swing shut). Opened far enough, the
//!   spent cases fly out. A flick the other way or the other hand pushing them closes them, and
//!   they latch.
//! * **Manual cycling** ([`Cycle`]): a bolt lifted, pulled back, pushed home and turned down, or a
//!   pump's forend pulled back and pushed forward, after every shot. The handle follows the hand
//!   along its L-shaped path (a bolt only slides once it's turned up, and only turns down once it's
//!   home); the case flies out at the back of the stroke and the next round goes in on the way home.
//!   Let go of, it stays where it is.
//! * **Chambers** ([`Chamber`]): guns loaded round by round straight into their chambers (a double
//!   barrel's two), fired in order, their cases left in them until the action opens.
//! * **Air** ([`Air`]): an air gun's pressure, built by strokes of its pump and spent by shots;
//!   below its minimum the trigger only hisses.
//!
//! All of it is plain state, no physics: the rig (`mod.rs`) feeds it the hands, the gun's attitude
//! and the buttons, and each state machine is tested here on its own.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

/// A flick of the gun this fast about its hinge (rad/s) in the opening direction (a break action
/// snapped muzzle-down) lets the latch go...
pub const FLICK_OPEN: f32 = 6.0;
/// ...and one this fast the other way (snapped muzzle-up) throws open barrels shut: they swing home
/// at [`FLICK_CLOSE_SPEED`] whatever their weight does.
pub const FLICK_CLOSE: f32 = 5.0;
pub const FLICK_CLOSE_SPEED: f32 = 12.0;
/// The swing loses this much of its speed per second (the hinge's friction).
const HINGE_DAMPING: f32 = 4.0;
/// The push open as the latch lets go (the hinge's spring), rad/s.
const UNLATCH_KICK: f32 = 1.5;
/// Open this far (or 80% of the way, if the action opens less), the spent cases fly out.
pub const EJECT_ANGLE: f32 = 75.0 * std::f32::consts::PI / 180.0;
/// It latches when it comes back within this of shut (radians: the catch takes it from there)
/// having been open more than [`OPENED`]...
const LATCH: f32 = 2.0 * std::f32::consts::PI / 180.0;
const OPENED: f32 = 5.0 * std::f32::consts::PI / 180.0;
/// ...and re-latches by itself if it never opened (held shut by its weight) for this long (s).
const RELATCH: f32 = 0.3;
/// Just latched, a flick doesn't open it again for this long (s): the wrist coming back down after
/// the flick that shut it.
const FLICK_REARM: f32 = 0.4;

/// A hinged action's barrels (or cylinder): how far they're turned about the hinge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hinge {
    /// How far open, radians (0 shut .. the size of the gun's `action_angle`).
    pub angle: f32,
    /// The swing, rad/s (positive opening).
    pub vel: f32,
    pub latched: bool,
    /// They've opened past [`OPENED`] since the latch let go, and the cases flew out on this opening.
    opened: bool,
    ejected: bool,
    /// Flicked shut: swinging home.
    flung: bool,
    /// How long it's been unlatched without opening, and how long latched.
    shut_for: f32,
    latched_for: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HingeEvent {
    Unlatched,
    /// Open far enough for the spent cases to fly out.
    Eject,
    Latched,
}

impl Hinge {
    pub fn shut() -> Hinge {
        Hinge { angle: 0.0, vel: 0.0, latched: true, opened: false, ejected: false, flung: false, shut_for: 0.0, latched_for: FLICK_REARM }
    }

    /// How far open, 0 shut .. 1 fully (of `max`, the gun's `action_angle`).
    pub fn fraction(&self, max: f32) -> f32 {
        (self.angle / max.abs().max(1e-3)).clamp(0.0, 1.0)
    }

    /// Lets the latch go (the lever, a flick, a hand pulling the barrels). False if it was open.
    pub fn unlatch(&mut self) -> bool {
        if !self.latched {
            return false;
        }
        self.latched = false;
        self.opened = false;
        self.ejected = false;
        self.flung = false;
        self.shut_for = 0.0;
        self.vel = self.vel.max(UNLATCH_KICK);
        true
    }

    /// One step of `dt`. `max`: how far it opens (radians, by its size); `pull`: the angular
    /// acceleration gravity gives the barrels about the hinge (rad/s², positive opening, see
    /// [`gravity_pull`]); `flick`: the gun's angular velocity about the hinge (rad/s, positive in
    /// the opening direction); `hand`: the angle a hand holding them puts them at.
    pub fn step(&mut self, dt: f32, max: f32, pull: f32, flick: f32, hand: Option<f32>, events: &mut Vec<HingeEvent>) {
        let max = max.abs();
        if self.latched {
            self.latched_for += dt;
            if flick > FLICK_OPEN && self.latched_for > FLICK_REARM && self.unlatch() {
                events.push(HingeEvent::Unlatched);
            } else {
                return;
            }
        }
        match hand {
            // A hand on them carries them, exactly.
            Some(target) => {
                let target = target.clamp(0.0, max);
                self.vel = (target - self.angle) / dt.max(1e-4);
                self.angle = target;
                self.flung = false;
            }
            None => {
                if flick < -FLICK_CLOSE && self.opened {
                    self.flung = true;
                }
                if self.flung {
                    self.vel = -FLICK_CLOSE_SPEED;
                } else {
                    self.vel += pull * dt;
                    self.vel *= (-HINGE_DAMPING * dt).exp();
                }
                self.angle += self.vel * dt;
            }
        }
        // The stop: a small bounce off it.
        if self.angle >= max {
            self.angle = max;
            if self.vel > 0.0 {
                self.vel *= -0.2;
            }
        }
        if self.angle > OPENED {
            self.opened = true;
        }
        if !self.ejected && self.angle >= EJECT_ANGLE.min(0.8 * max) {
            self.ejected = true;
            events.push(HingeEvent::Eject);
        }
        if self.angle <= 0.0 {
            self.angle = 0.0;
            self.vel = self.vel.max(0.0);
        }
        // Back home after opening: it latches (a hand, a flick, its weight). Never opened (held
        // shut by its own weight as the lever went): the latch catches again a moment later.
        self.shut_for = if !self.opened && self.angle < OPENED { self.shut_for + dt } else { 0.0 };
        if (self.opened && self.angle <= LATCH) || (self.shut_for > RELATCH && hand.is_none()) {
            let was_open = self.opened;
            *self = Hinge::shut();
            self.latched_for = 0.0;
            if was_open {
                events.push(HingeEvent::Latched);
            }
        }
    }
}

/// The weapon-space axis a hinged action turns about (break: +X, swing: +Z), and the sign of its
/// opening (`action_angle`'s).
pub fn hinge_axis(d: &ObjectDesc) -> (Vec3, f32) {
    let axis = if d.action == ACTION_SWING { Vec3::Z } else { Vec3::X };
    (axis, if d.action_angle < 0.0 { -1.0 } else { 1.0 })
}

/// The rotation of the barrels (or cylinder) open by `angle` (radians, by its size), in weapon
/// space about the hinge: a point `p` on them goes to `hinge + rot * (p - hinge)`.
pub fn hinge_rot(d: &ObjectDesc, angle: f32) -> Quat {
    let (axis, sign) = hinge_axis(d);
    Quat::from_axis_angle(axis, sign * angle)
}

/// A weapon-space point on the barrels, open by `angle`.
pub fn on_barrels(d: &ObjectDesc, angle: f32, p: Vec3) -> Vec3 {
    let h = Vec3::from_array(d.handle);
    h + hinge_rot(d, angle) * (p - h)
}

/// Where the moving part's weight is, from the hinge (weapon space, shut): a break action's barrels
/// half way to the muzzle; a swinging barrel or cylinder on the bore beside its pin.
pub fn barrels_centre(d: &ObjectDesc) -> Vec3 {
    let (h, m) = (Vec3::from_array(d.handle), Vec3::from_array(d.muzzle));
    if d.action == ACTION_SWING { Vec3::new(m.x - h.x, m.y - h.y, 0.0) } else { (m - h) * 0.5 }
}

/// Gravity's angular acceleration on hinged barrels open by `angle` (rad/s², positive opening):
/// a point weight at [`barrels_centre`] swinging about the hinge, `gravity` in weapon space.
pub fn gravity_pull(d: &ObjectDesc, angle: f32, gravity: Vec3) -> f32 {
    let (axis, sign) = hinge_axis(d);
    let r = hinge_rot(d, angle) * barrels_centre(d);
    let r = r - axis * r.dot(axis);
    let len2 = r.length_squared();
    if len2 < 1e-6 {
        return 0.0;
    }
    sign * r.cross(gravity).dot(axis) / len2
}

/// The angle (by its size, 0..`max`) a hand at `hand` (weapon space) carries hinged barrels to when
/// it took them at `took` with them open by `took_angle`: the hand's turn about the hinge since.
pub fn hand_angle(d: &ObjectDesc, took: Vec3, took_angle: f32, hand: Vec3) -> f32 {
    let (axis, sign) = hinge_axis(d);
    let h = Vec3::from_array(d.handle);
    let flat = |p: Vec3| {
        let v = p - h;
        v - axis * v.dot(axis)
    };
    let (a, b) = (flat(took), flat(hand));
    if a.length_squared() < 1e-6 || b.length_squared() < 1e-6 {
        return took_angle;
    }
    let turn = a.cross(b).dot(axis).atan2(a.dot(b));
    took_angle + sign * turn
}

/// A bolt turned up this far (of its lift) is out of its locking lugs and slides.
const UNLOCKED: f32 = 0.85;

/// Manual cycling: a bolt's or a pump's handle, along its path.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cycle {
    /// How far the handle's lifted (0 down .. 1 up; a pump's stays 0) and back (0 home .. 1).
    pub lift: f32,
    pub back: f32,
    /// It went fully back on this stroke (the case is out): the next round goes in on the way home.
    stroked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleEvent {
    /// A bolt turned up out of its locking lugs.
    Unlocked,
    /// At the back: whatever was in the chamber flies out.
    Back,
    /// Home again after a full stroke: the next round goes in.
    Home,
    /// A bolt turned down: locked, it fires.
    Locked,
}

impl Cycle {
    /// A handle lifted and back this far (for where it would be).
    pub fn at(lift: f32, back: f32) -> Cycle {
        Cycle { lift, back, stroked: false }
    }

    /// Closed (and locked): it can fire. (Within a tenth of home: the last few millimetres the bolt
    /// seats itself.)
    pub fn closed(&self) -> bool {
        self.lift <= 0.1 && self.back <= 0.1
    }

    /// The hand has the handle where `lift` and `back` (0..1 each) would put it. A bolt that lifts
    /// (`lifts`) only slides once it's turned up and only turns down once it's home.
    pub fn drive(&mut self, lift: f32, back: f32, lifts: bool, events: &mut Vec<CycleEvent>) {
        let (lift, back) = (lift.clamp(0.0, 1.0), back.clamp(0.0, 1.0));
        let (was_lift, was_back) = (self.lift, self.back);
        if lifts {
            // Out of home it stays turned up; up, it slides.
            self.lift = if self.back > 0.02 { 1.0 } else { lift };
            if self.lift >= UNLOCKED {
                self.lift = if back > 0.02 { 1.0 } else { self.lift };
                self.back = back;
            } else {
                self.back = 0.0;
            }
        } else {
            self.lift = 0.0;
            self.back = back;
        }
        if lifts && was_lift < UNLOCKED && self.lift >= UNLOCKED {
            events.push(CycleEvent::Unlocked);
        }
        if !self.stroked && was_back < 0.9 && self.back >= 0.9 {
            self.stroked = true;
            events.push(CycleEvent::Back);
        }
        if self.stroked && was_back > 0.1 && self.back <= 0.1 {
            self.stroked = false;
            events.push(CycleEvent::Home);
        }
        if lifts && was_lift > 0.1 && self.lift <= 0.1 {
            events.push(CycleEvent::Locked);
        }
    }

    /// The handle's lift and pull for a hand that has its knob at weapon-space `knob`: its turn
    /// about the bore (the line through `muzzle` along +Z) from the knob's closed place `home`, as
    /// a fraction of `action_angle`, and how far it's back of it along -Z, of `handle_travel`.
    pub fn from_knob(d: &ObjectDesc, home: Vec3, knob: Vec3) -> (f32, f32) {
        let axis = Vec3::new(d.muzzle[0], d.muzzle[1], 0.0);
        let flat = |p: Vec3| Vec3::new(p.x, p.y, 0.0) - axis;
        let (a, b) = (flat(home), flat(knob));
        let lift = if d.action_angle.abs() > 1e-3 && a.length_squared() > 1e-6 && b.length_squared() > 1e-6 {
            a.cross(b).z.atan2(a.dot(b)) / d.action_angle
        } else {
            0.0
        };
        let travel = if d.handle_travel > 0.0 { d.handle_travel } else { 0.06 };
        (lift, (home.z - knob.z) / travel)
    }

    /// Where the knob is (weapon space): `home` turned up by the lift about the bore, then back.
    pub fn knob(&self, d: &ObjectDesc, home: Vec3) -> Vec3 {
        let axis = Vec3::new(d.muzzle[0], d.muzzle[1], home.z);
        let turned = axis + Quat::from_rotation_z(d.action_angle * self.lift) * (home - axis);
        turned - Vec3::Z * self.back * d.handle_travel
    }
}

/// One chamber of a gun loaded round by round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Chamber {
    #[default]
    Empty,
    Live,
    Spent,
}

/// `ObjectState::chambered` for a set of chambers (see `CHAMBER_SPENT_SHIFT`).
pub fn chamber_bits(chambers: &[Chamber]) -> u32 {
    chambers.iter().take(16).enumerate().fold(0, |bits, (k, c)| match c {
        Chamber::Live => bits | 1 << k,
        Chamber::Spent => bits | 1 << (k as u32 + CHAMBER_SPENT_SHIFT),
        Chamber::Empty => bits,
    })
}

/// Where chamber `k`'s mouth is on the barrels (weapon space, shut).
pub fn chamber_mouth(d: &ObjectDesc, k: usize) -> Vec3 {
    Vec3::from_array(d.mag_well) + Vec3::X * d.chamber_step * k as f32
}

/// An air gun's pressure.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Air {
    /// 0 empty .. 1 full.
    pub pressure: f32,
    /// The pump went all the way back on this stroke.
    stroked: bool,
}

impl Air {
    /// The pump is at `back` (0 home .. 1 fully out): a whole stroke (out and home again) adds
    /// `gain`. True at the end of a stroke.
    pub fn pump(&mut self, back: f32, gain: f32) -> bool {
        if back >= 0.9 {
            self.stroked = true;
        }
        if self.stroked && back <= 0.15 {
            self.stroked = false;
            self.pressure = (self.pressure + gain).min(1.0);
            return true;
        }
        false
    }

    /// The pump let go of out: no stroke.
    pub fn abandon(&mut self) {
        self.stroked = false;
    }

    /// A shot needs `min` and spends `cost`: the pressure it fires at (its strength), or None if
    /// there isn't enough.
    pub fn shoot(&mut self, cost: f32, min: f32) -> Option<f32> {
        if self.pressure < min.max(1e-3) {
            return None;
        }
        let strength = self.pressure;
        self.pressure = (self.pressure - cost).max(0.0);
        Some(strength)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    const DT: f32 = 1.0 / 90.0;

    /// A double barrel: the hinge 4 cm under the bore at the breech, the muzzle 40 cm ahead,
    /// opening 60 degrees.
    fn double() -> ObjectDesc {
        let mut d = ObjectDesc::zeroed();
        d.kind = OBJ_WEAPON;
        d.action = ACTION_BREAK;
        d.action_angle = 60f32.to_radians();
        d.handle = [0.0, -0.04, 0.2];
        d.muzzle = [0.0, 0.0, 0.6];
        d.mag_well = [0.02, 0.0, 0.19];
        d.chamber_step = -0.04;
        d.capacity = 2;
        d.weapon_flags = WPN_LOADS_CHAMBERS;
        d
    }

    fn level() -> Vec3 {
        Vec3::new(0.0, -9.81, 0.0)
    }

    fn run(h: &mut Hinge, d: &ObjectDesc, frames: usize, gravity: Vec3, flick: f32, hand: Option<f32>) -> Vec<HingeEvent> {
        let mut ev = Vec::new();
        for _ in 0..frames {
            let pull = gravity_pull(d, h.angle, gravity);
            h.step(DT, d.action_angle, pull, flick, hand, &mut ev);
        }
        ev
    }

    #[test]
    fn unlatched_barrels_fall_open_under_their_weight_and_throw_the_cases_out() {
        let d = double();
        let mut h = Hinge::shut();
        assert!(run(&mut h, &d, 30, level(), 0.0, None).is_empty(), "latched, nothing moves");
        assert_eq!(h.angle, 0.0);
        assert!(h.unlatch());
        let ev = run(&mut h, &d, 90, level(), 0.0, None);
        assert!(h.fraction(d.action_angle) > 0.97, "fallen fully open with the gun level: {:.1} deg", h.angle.to_degrees());
        assert_eq!(ev, vec![HingeEvent::Eject], "the cases flew out once");
        assert!(!h.latched);
    }

    #[test]
    fn where_they_hang_is_where_gravity_puts_them() {
        let d = double();
        // Their weight is 2 cm over the hinge 20 cm out: plumb is 95.7 degrees from shut. With the
        // muzzle 45 degrees down they hang at 50.7 (short of the 60 degree stop).
        let plumb = 90.0 + (0.02f32 / 0.2).atan().to_degrees();
        let g = Quat::from_rotation_x(45f32.to_radians()).inverse() * level();
        let mut h = Hinge::shut();
        h.unlatch();
        run(&mut h, &d, 400, g, 0.0, None);
        assert!((h.angle.to_degrees() - (plumb - 45.0)).abs() < 2.0, "hangs at {:.1} deg", h.angle.to_degrees());
        // The stock lifted up to them (the muzzle down and back): they swing shut and latch.
        let g = Quat::from_rotation_x(110f32.to_radians()).inverse() * level();
        let ev = run(&mut h, &d, 200, g, 0.0, None);
        assert!(h.latched && ev.contains(&HingeEvent::Latched), "{h:?} {ev:?}");
    }

    #[test]
    fn a_flick_opens_and_the_other_way_snaps_them_shut() {
        let d = double();
        let mut h = Hinge::shut();
        let ev = run(&mut h, &d, 1, level(), FLICK_OPEN + 1.0, None);
        assert_eq!(ev, vec![HingeEvent::Unlatched]);
        run(&mut h, &d, 90, level(), 0.0, None);
        assert!(h.fraction(d.action_angle) > 0.9);
        let ev = run(&mut h, &d, 3, level(), -(FLICK_CLOSE + 1.0), None);
        let ev2 = run(&mut h, &d, 30, level(), 0.0, None);
        assert!(h.latched, "flicked shut: {h:?}");
        assert!(ev.contains(&HingeEvent::Latched) || ev2.contains(&HingeEvent::Latched));
        // The wrist coming back down right after doesn't open it again.
        let mut h = Hinge { latched_for: 0.0, ..Hinge::shut() };
        assert!(run(&mut h, &d, 3, level(), FLICK_OPEN + 2.0, None).is_empty() && h.latched);
        // A slow tilt isn't a flick.
        let ev = run(&mut h, &d, 10, level(), FLICK_OPEN * 0.5, None);
        assert!(ev.is_empty() && h.latched);
    }

    #[test]
    fn a_hand_on_the_barrels_carries_them_and_pushing_them_home_latches() {
        let d = double();
        let mut h = Hinge::shut();
        h.unlatch();
        run(&mut h, &d, 10, level(), 0.0, Some(30f32.to_radians()));
        assert!((h.angle.to_degrees() - 30.0).abs() < 0.1);
        let mut ev = Vec::new();
        for k in 0..30 {
            let a = 30f32.to_radians() * (1.0 - k as f32 / 29.0);
            h.step(DT, d.action_angle, gravity_pull(&d, h.angle, level()), 0.0, Some(a), &mut ev);
        }
        assert!(h.latched && ev == vec![HingeEvent::Latched], "{h:?} {ev:?}");
    }

    #[test]
    fn a_lever_pressed_with_the_muzzle_down_catches_again() {
        let d = double();
        let mut h = Hinge::shut();
        h.unlatch();
        let g = Quat::from_rotation_x(110f32.to_radians()).inverse() * level();
        let ev = run(&mut h, &d, 60, g, 0.0, None);
        assert!(h.latched && ev.is_empty(), "held shut by their weight, it latches quietly: {h:?} {ev:?}");
    }

    #[test]
    fn a_hand_turning_about_the_hinge_turns_the_barrels() {
        let d = double();
        let took = Vec3::new(0.0, -0.05, 0.4);
        // The hand swung 20 degrees down about the hinge.
        let h = Vec3::from_array(d.handle);
        let to = h + Quat::from_rotation_x(20f32.to_radians()) * (took - h);
        let a = hand_angle(&d, took, 0.0, to);
        assert!((a.to_degrees() - 20.0).abs() < 0.01, "{}", a.to_degrees());
        // The barrels' points follow: the muzzle goes down.
        let m = on_barrels(&d, a, Vec3::from_array(d.muzzle));
        assert!(m.y < d.muzzle[1] - 0.1, "{m}");
        // A swing action turns about +Z: positive swings its top to the left.
        let mut s = d;
        s.action = ACTION_SWING;
        let top = on_barrels(&s, 90f32.to_radians(), Vec3::new(0.0, 0.0, 0.3));
        assert!(top.x < -0.03 && top.y.abs() < 0.045, "{top}");
    }

    #[test]
    fn a_bolt_turns_up_slides_back_comes_home_and_turns_down() {
        let mut c = Cycle::default();
        let mut ev = Vec::new();
        // Pulling straight back without lifting: the lugs hold it.
        c.drive(0.0, 0.8, true, &mut ev);
        assert_eq!((c.lift, c.back), (0.0, 0.0));
        assert!(ev.is_empty() && c.closed());
        c.drive(1.0, 0.0, true, &mut ev);
        assert_eq!(ev, vec![CycleEvent::Unlocked]);
        assert!(!c.closed());
        c.drive(1.0, 0.5, true, &mut ev);
        c.drive(0.3, 1.0, true, &mut ev);
        assert_eq!(c.lift, 1.0, "back, it can't turn down");
        assert_eq!(ev.last(), Some(&CycleEvent::Back));
        c.drive(1.0, 0.0, true, &mut ev);
        assert_eq!(ev.last(), Some(&CycleEvent::Home));
        c.drive(0.0, 0.0, true, &mut ev);
        assert_eq!(ev, vec![CycleEvent::Unlocked, CycleEvent::Back, CycleEvent::Home, CycleEvent::Locked]);
        assert!(c.closed());
        // Half a stroke and back: nothing goes in.
        let mut ev = Vec::new();
        c.drive(1.0, 0.0, true, &mut ev);
        c.drive(1.0, 0.6, true, &mut ev);
        c.drive(1.0, 0.0, true, &mut ev);
        assert_eq!(ev, vec![CycleEvent::Unlocked]);
    }

    #[test]
    fn a_pump_strokes_back_and_forward() {
        let mut c = Cycle::default();
        let mut ev = Vec::new();
        for b in [0.3, 0.95, 0.5, 0.05] {
            c.drive(0.7, b, false, &mut ev);
        }
        assert_eq!(c.lift, 0.0, "a pump doesn't turn");
        assert_eq!(ev, vec![CycleEvent::Back, CycleEvent::Home]);
        assert!(c.closed());
    }

    #[test]
    fn the_knob_maps_to_lift_and_pull_and_back() {
        let mut d = ObjectDesc::zeroed();
        d.action = ACTION_BOLT;
        d.action_angle = 90f32.to_radians();
        d.handle_travel = 0.08;
        d.muzzle = [0.0, 0.0, 1.0];
        let home = Vec3::new(0.09, 0.0, 0.5);
        // Turned up (the right-side knob to the top), then back 4 cm.
        let up = Vec3::new(0.0, 0.09, 0.46);
        let (lift, back) = Cycle::from_knob(&d, home, up);
        assert!((lift - 1.0).abs() < 1e-3 && (back - 0.5).abs() < 1e-3, "{lift} {back}");
        let c = Cycle { lift: 1.0, back: 0.5, stroked: false };
        assert!(c.knob(&d, home).distance(up) < 1e-4);
    }

    #[test]
    fn chambers_report_live_and_spent() {
        let ch = [Chamber::Spent, Chamber::Live];
        assert_eq!(chamber_bits(&ch), 0b10 | 1 << CHAMBER_SPENT_SHIFT);
        let d = double();
        assert!((chamber_mouth(&d, 1).x - (-0.02)).abs() < 1e-6);
    }

    #[test]
    fn air_builds_with_whole_strokes_and_shots_spend_it() {
        let mut a = Air::default();
        assert_eq!(a.shoot(0.1, 0.15), None, "no pressure, no shot");
        assert!(!a.pump(0.5, 0.25) && !a.pump(0.0, 0.25), "half a stroke pumps nothing");
        assert!(!a.pump(1.0, 0.25));
        assert!(a.pump(0.0, 0.25));
        assert!((a.pressure - 0.25).abs() < 1e-6);
        for _ in 0..6 {
            a.pump(1.0, 0.25);
            a.pump(0.0, 0.25);
        }
        assert_eq!(a.pressure, 1.0, "full stays full");
        assert_eq!(a.shoot(0.1, 0.15), Some(1.0), "a full-strength shot");
        let mut shots = 1;
        while a.shoot(0.1, 0.15).is_some() {
            shots += 1;
        }
        assert_eq!(shots, 9, "until it's below the minimum: {}", a.pressure);
        assert!(a.pressure < 0.15);
    }
}
