//! Anything turned by hand on an axle: a game's valve or wheel, a switch's knob, a lever, rigged to
//! an object ([`OBJ_VALVE`]) so any game with one works the same way. The game reads how far it's
//! turned (its state) and works its own mechanism from that.
//!
//! It turns on an axle fixed in the world: its centre and axis are the object's spawn pose (axis
//! +Z), its rim `ObjectDesc::radius` out, its own weight `mass` (a ring's inertia, m r^2), and the
//! turning it resists `ObjectDesc::friction` (N m, static and sliding alike, as an old stiff valve
//! or a free-running wheel), scaled by the player's stiffness setting. A hand reaching it takes it
//! where it is, and turns it one of two ways:
//!
//! * **cranked** (a wheel): the hand pulls its point on the rim towards the controller like an
//!   arm would, up to [`ARM_FORCE`];
//! * **twisted** (a knob, `JOINT_TWIST`, or anything smaller than [`TWIST_RADIUS`]): the wrist
//!   turns it as far as the hand has turned about the axle since it took hold, up to
//!   [`WRIST_TORQUE`].
//!
//! Either way each hand adds its own: a light one follows the hand, a stiff one needs both hands
//! or won't give, and the hand stays on it (the ghost shows where the controller is). It may turn
//! within a range only (`turn_min`..`turn_max`, a switch's quarter turn), clicking into either end
//! and holding there against up to `detent` (N m). Let go, it spins on until its friction stops
//! it. One geared to something the game moves at its own pace (a gate it winds up) turns no faster
//! than that (`ObjectDesc::fire_rate`, rad/s).

use glam::{Quat, Vec3};

/// How hard an arm pulls the hand's point on the rim towards the controller, N per metre apart,
/// and most a hand can pull, N.
pub const ARM_STIFFNESS: f32 = 600.0;
pub const ARM_FORCE: f32 = 180.0;
/// How hard a wrist twists a knob towards where the hand has turned, N m per radian, and most a
/// wrist can twist, N m (about what a firm grip turns a stiff tap with).
pub const WRIST_STIFFNESS: f32 = 60.0;
pub const WRIST_TORQUE: f32 = 6.0;
/// A knob: anything no bigger than this round (metres) is twisted, not cranked.
pub const TWIST_RADIUS: f32 = 0.06;
/// Within this far (radians) of a click it pulls into it, and how stiffly (N m per radian).
const DETENT_ZONE: f32 = 0.3;
const DETENT_SPRING: f32 = 40.0;
/// How far from the rim (metres) a palm takes hold of it.
pub const REACH: f32 = 0.1;
const SUBSTEPS: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct Valve {
    /// The axle: centre, and the rotation whose +Z is the axis (the wheel at angle 0).
    pub centre: Vec3,
    pub base: Quat,
    pub radius: f32,
    pub inertia: f32,
    pub friction: f32,
    /// Turned this far about the axis (radians, unbounded), and how fast.
    pub angle: f32,
    pub vel: f32,
    /// The fastest it turns, rad/s (0: as fast as it's pushed).
    pub max_speed: f32,
    /// The range it turns in (`min == max`: no limit), and how hard its clicks at both ends hold.
    pub min: f32,
    pub max: f32,
    pub detent: f32,
    /// Twisted by the wrist (a knob), not cranked round its rim.
    pub twist: bool,
}

/// What one hand does to it this step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pull {
    /// Cranked: its place on the rim (angle, wheel frame), pulled towards the controller there.
    Rim { at: f32, toward: Vec3 },
    /// Twisted: the angle the wrist turns it towards.
    Twist { to: f32 },
}

/// A hand on a valve: where on the rim (its angle in the wheel's own frame) and how the hand sits
/// on it (its rotation in the wheel's frame); and where the hand was when it took hold, and when
/// (it goes onto the rim over the grab transition).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValveGrip {
    pub id: u32,
    pub at: f32,
    pub rot: Quat,
    pub from: (Vec3, Quat),
    pub since: f64,
    /// Twisting: how far it was turned when the hand took hold, and how far the hand has turned
    /// about the axle since (unwrapped), and its last reading.
    pub turn0: f32,
    pub twisted: f32,
    pub wrist: f32,
}

impl Valve {
    pub fn new(centre: Vec3, base: Quat, radius: f32, mass: f32, friction: f32) -> Valve {
        let radius = radius.max(0.02);
        Valve {
            centre,
            base,
            radius,
            inertia: (mass.max(0.1) * radius * radius).max(1e-4),
            friction: friction.max(0.0),
            angle: 0.0,
            vel: 0.0,
            max_speed: 0.0,
            min: 0.0,
            max: 0.0,
            detent: 0.0,
            twist: radius <= TWIST_RADIUS,
        }
    }

    /// Turning only between `min` and `max` (radians; equal: no limit), clicking into both ends
    /// and holding there against up to `detent` N m.
    pub fn with_range(self, min: f32, max: f32, detent: f32) -> Valve {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        Valve { min, max, detent: detent.max(0.0), ..self }
    }

    pub fn limited(&self) -> bool {
        self.max > self.min
    }

    /// The hand's turn about the axle (radians, its rotation's twist about it).
    pub fn wrist(&self, hand: Quat) -> f32 {
        let a = self.axis();
        let q = if hand.w < 0.0 { -hand } else { hand };
        2.0 * Vec3::new(q.x, q.y, q.z).dot(a).atan2(q.w)
    }

    /// The wheel's rotation now.
    pub fn rot(&self) -> Quat {
        self.base * Quat::from_rotation_z(self.angle)
    }

    pub fn axis(&self) -> Vec3 {
        self.base * Vec3::Z
    }

    /// The rim at angle `at` (wheel frame), in the world.
    pub fn rim(&self, at: f32) -> Vec3 {
        self.centre + self.rot() * Vec3::new(self.radius * at.cos(), self.radius * at.sin(), 0.0)
    }

    /// The rim's nearest point to `p`: its angle (wheel frame) and how far `p` is from it.
    pub fn nearest(&self, p: Vec3) -> (f32, f32) {
        let local = self.rot().inverse() * (p - self.centre);
        let at = local.y.atan2(local.x);
        let on = Vec3::new(self.radius * at.cos(), self.radius * at.sin(), 0.0);
        (at, local.distance(on))
    }

    /// One step: each gripping hand pulls or twists (each its own: two hands, twice the force),
    /// its clicks pull it in, friction holds or drags, and its range stops it. `stiffness` scales
    /// its friction and clicks (the player's setting: 1 as the game made it).
    pub fn step(&mut self, pulls: &[Pull], dt: f32, stiffness: f32) {
        let h = dt / SUBSTEPS as f32;
        let axis = self.axis();
        let (friction, detent) = (self.friction * stiffness, self.detent * stiffness);
        // An arm, and a wrist, are damped springs: critically damped on its own inertia.
        let m_eff = self.inertia / (self.radius * self.radius);
        let damp = 2.0 * (ARM_STIFFNESS * m_eff).sqrt();
        let wrist_damp = 2.0 * (WRIST_STIFFNESS * self.inertia).sqrt();
        for _ in 0..SUBSTEPS {
            let mut torque = 0.0;
            for pull in pulls {
                torque += match *pull {
                    Pull::Rim { at, toward } => {
                        let p = self.rim(at);
                        let r = p - self.centre;
                        let v = axis.cross(r) * self.vel;
                        let f = ((toward - p) * ARM_STIFFNESS - v * damp).clamp_length_max(ARM_FORCE);
                        r.cross(f).dot(axis)
                    }
                    Pull::Twist { to } => ((to - self.angle) * WRIST_STIFFNESS - self.vel * wrist_damp).clamp(-WRIST_TORQUE, WRIST_TORQUE),
                };
            }
            // Its clicks: near an end, pulled into it (and held there against up to `detent`).
            if self.limited() && detent > 0.0 {
                let zone = DETENT_ZONE.min((self.max - self.min) / 3.0);
                for end in [self.min, self.max] {
                    let off = self.angle - end;
                    if off.abs() < zone {
                        torque += (-off * DETENT_SPRING - self.vel * wrist_damp).clamp(-detent, detent);
                    }
                }
            }
            // Friction: holds still against less than it, drags against motion.
            if self.vel.abs() < 1e-3 && torque.abs() <= friction {
                self.vel = 0.0;
                continue;
            }
            let dir = if self.vel.abs() >= 1e-3 { self.vel.signum() } else { torque.signum() };
            let v = self.vel + (torque - dir * friction) * h / self.inertia;
            // (Friction stops it; it doesn't turn it back.)
            self.vel = if self.vel != 0.0 && v.signum() != self.vel.signum() && torque.abs() <= friction { 0.0 } else { v };
            if self.max_speed > 0.0 {
                self.vel = self.vel.clamp(-self.max_speed, self.max_speed);
            }
            self.angle += self.vel * h;
            // Its stops.
            if self.limited() && (self.angle < self.min || self.angle > self.max) {
                self.angle = self.angle.clamp(self.min, self.max);
                self.vel = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(friction: f32) -> Valve {
        Valve::new(Vec3::new(0.0, 1.2, 1.0), Quat::IDENTITY, 0.2, 2.0, friction)
    }

    /// Hands that take the rim and go `turn` radians round with it (anticlockwise about +Z) at a
    /// quarter turn a second, then hold there a third of a second.
    fn turn(v: &mut Valve, hands: &[f32], turn: f32) {
        let start: Vec<f32> = hands.to_vec();
        let steps = (turn / std::f32::consts::FRAC_PI_2 * 90.0) as usize;
        for k in 1..=steps + 30 {
            let t = (k as f32 / steps as f32).min(1.0) * turn / std::f32::consts::FRAC_PI_2;
            let pulls: Vec<Pull> = start
                .iter()
                .map(|&a| {
                    let want = a + t * std::f32::consts::FRAC_PI_2;
                    Pull::Rim { at: a, toward: v.centre + v.base * Vec3::new(v.radius * want.cos(), v.radius * want.sin(), 0.0) }
                })
                .collect();
            v.step(&pulls, 1.0 / 90.0, 1.0);
        }
    }

    #[test]
    fn a_light_valve_follows_the_hand_round() {
        let mut v = wheel(0.5);
        turn(&mut v, &[std::f32::consts::FRAC_PI_2], std::f32::consts::FRAC_PI_2);
        assert!((v.angle - std::f32::consts::FRAC_PI_2).abs() < 0.1, "a quarter turn with the hand: {}", v.angle);
    }

    #[test]
    fn a_stiff_valve_wont_give_to_one_hand_but_does_to_two() {
        // 45 N m on a 20 cm rim. Each hand pulls round, its controller 25 cm ahead of where it holds
        // along the rim (150 N): one hand manages 30 N m, two 60.
        let lead = |v: &mut Valve, hands: &[f32]| {
            for _ in 0..90 {
                let pulls: Vec<Pull> = hands.iter().map(|&a| Pull::Rim { at: a, toward: v.rim(a) + v.axis().cross(v.rim(a) - v.centre).normalize() * 0.25 }).collect();
                v.step(&pulls, 1.0 / 90.0, 1.0);
            }
        };
        let mut v = wheel(45.0);
        lead(&mut v, &[std::f32::consts::FRAC_PI_2]);
        assert!(v.angle.abs() < 1e-3, "one hand can't move it: {}", v.angle);
        let mut v = wheel(45.0);
        lead(&mut v, &[std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2]);
        assert!(v.angle > 1.0, "two hands turn it: {}", v.angle);
    }

    #[test]
    fn let_go_it_spins_on_until_friction_stops_it() {
        let mut v = wheel(0.2);
        v.vel = 6.0;
        v.step(&[], 0.1, 1.0);
        assert!(v.vel > 4.0 && v.vel < 6.0, "slowing: {}", v.vel);
        for _ in 0..400 {
            v.step(&[], 1.0 / 90.0, 1.0);
        }
        assert_eq!(v.vel, 0.0, "stopped");
        let a = v.angle;
        v.step(&[], 1.0, 1.0);
        assert_eq!(v.angle, a, "and stays");
    }

    #[test]
    fn a_geared_wheel_turns_no_faster_than_its_gearing() {
        // A hand going round a quarter turn a second on a wheel geared to half that: it lags.
        let mut v = wheel(0.5);
        v.max_speed = std::f32::consts::FRAC_PI_4;
        turn(&mut v, &[0.0], std::f32::consts::TAU);
        assert!(v.vel.abs() <= v.max_speed + 1e-5);
        assert!(v.angle > 1.0 && v.angle < std::f32::consts::TAU * 0.8, "behind the hand: {}", v.angle);
    }

    /// A switch's knob: 3 cm round, a quarter turn between its clicks.
    fn knob() -> Valve {
        Valve::new(Vec3::new(0.0, 1.4, 0.5), Quat::IDENTITY, 0.03, 0.2, 0.05).with_range(0.0, std::f32::consts::FRAC_PI_2, 0.8)
    }

    /// The wrist turns `by` over `steps` steps, then holds there.
    fn twist(v: &mut Valve, hands: usize, by: f32, steps: usize, stiffness: f32) {
        let from = v.angle;
        for k in 1..=steps + 60 {
            let to = from + by * (k.min(steps) as f32 / steps as f32);
            v.step(&vec![Pull::Twist { to }; hands], 1.0 / 90.0, stiffness);
        }
    }

    #[test]
    fn a_knob_is_twisted_and_stops_at_its_ends() {
        let mut v = knob();
        assert!(v.twist, "a knob that small is twisted, not cranked");
        // Turned a little: its click pulls it back to off.
        twist(&mut v, 1, 0.15, 20, 1.0);
        let mut free = v;
        for _ in 0..90 {
            free.step(&[], 1.0 / 90.0, 1.0);
        }
        assert!(free.angle.abs() < 0.02, "let go short of half way, it clicks back off: {}", free.angle);
        // Turned on, past the end: it stops at the end and clicks in there.
        twist(&mut v, 1, 2.0, 60, 1.0);
        assert!((v.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-4 && v.vel == 0.0, "at its stop: {}", v.angle);
        for _ in 0..90 {
            v.step(&[], 1.0 / 90.0, 1.0);
        }
        assert!((v.angle - std::f32::consts::FRAC_PI_2).abs() < 0.02, "and stays on: {}", v.angle);
    }

    #[test]
    fn a_stiff_knob_wants_both_hands_and_the_setting_scales_it() {
        // 9 N m of stiffness: more than one wrist's 6 N m, less than two.
        let stiff = Valve { friction: 9.0, ..knob() };
        let mut one = stiff;
        twist(&mut one, 1, 1.0, 45, 1.0);
        assert!(one.angle.abs() < 1e-3, "one hand can't twist it: {}", one.angle);
        let mut two = stiff;
        twist(&mut two, 2, 1.0, 45, 1.0);
        assert!(two.angle > 0.8, "two hands do: {}", two.angle);
        let mut eased = stiff;
        twist(&mut eased, 1, 1.0, 45, 0.5);
        assert!(eased.angle > 0.8, "at half the stiffness one does: {}", eased.angle);
    }

    #[test]
    fn the_wrist_reading_is_the_hands_turn_about_the_axle() {
        let v = knob();
        let hand = Quat::from_rotation_z(0.7) * Quat::from_rotation_x(0.3);
        assert!((v.wrist(hand) - 0.7).abs() < 0.05, "{}", v.wrist(hand));
        assert!(v.wrist(Quat::from_rotation_y(1.0)).abs() < 1e-5, "turning about another axis isn't twisting it");
    }

    #[test]
    fn the_nearest_rim_point() {
        let v = wheel(1.0);
        let (at, d) = v.nearest(v.centre + Vec3::new(0.0, 0.25, 0.03));
        assert!((at - std::f32::consts::FRAC_PI_2).abs() < 1e-4 && (d - (0.05f32.powi(2) + 0.03f32.powi(2)).sqrt()).abs() < 1e-4);
        assert!(v.rim(at).distance(v.centre + Vec3::new(0.0, 0.2, 0.0)) < 1e-5);
    }
}
