//! Valves and wheels: a game's turnable mechanism, rigged to an object ([`OBJ_VALVE`]) so any game
//! with one works the same way.
//!
//! A valve is a wheel on an axle fixed in the world: its centre and axis are the object's spawn
//! pose (axis +Z), its rim `ObjectDesc::radius` out, its own weight `mass` (a ring's inertia,
//! m r^2), and the turning it resists `ObjectDesc::friction` (N m, static and sliding alike, as an
//! old stiff valve or a free-running wheel). A hand reaching the rim takes it where it is; gripped,
//! the hand pulls its point on the rim towards the controller like an arm would, up to
//! [`ARM_FORCE`]: a light wheel follows the hand round, a stiff one needs both hands or won't
//! give, and the hand stays on the rim either way (the ghost shows where the controller is). Let
//! go, it spins on until its friction stops it.

use glam::{Quat, Vec3};

/// How hard an arm pulls the hand's point on the rim towards the controller, N per metre apart,
/// and most a hand can pull, N.
pub const ARM_STIFFNESS: f32 = 600.0;
pub const ARM_FORCE: f32 = 180.0;
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
}

/// A hand on a valve: where on the rim (its angle in the wheel's own frame) and how the hand sits
/// on it (its rotation in the wheel's frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValveGrip {
    pub id: u32,
    pub at: f32,
    pub rot: Quat,
}

impl Valve {
    pub fn new(centre: Vec3, base: Quat, radius: f32, mass: f32, friction: f32) -> Valve {
        let radius = radius.max(0.02);
        Valve { centre, base, radius, inertia: (mass.max(0.1) * radius * radius).max(1e-4), friction: friction.max(0.0), angle: 0.0, vel: 0.0 }
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

    /// One step: each gripping hand (its place on the rim, and where its controller is) pulls,
    /// friction holds or drags.
    pub fn step(&mut self, pulls: &[(f32, Vec3)], dt: f32) {
        let h = dt / SUBSTEPS as f32;
        let axis = self.axis();
        // An arm is a damped spring: critically damped on the rim's own inertia.
        let m_eff = self.inertia / (self.radius * self.radius);
        let damp = 2.0 * (ARM_STIFFNESS * m_eff).sqrt();
        for _ in 0..SUBSTEPS {
            let mut torque = 0.0;
            for &(at, target) in pulls {
                let p = self.rim(at);
                let r = p - self.centre;
                let v = axis.cross(r) * self.vel;
                let f = ((target - p) * ARM_STIFFNESS - v * damp).clamp_length_max(ARM_FORCE);
                torque += r.cross(f).dot(axis);
            }
            // Friction: holds still against less than it, drags against motion.
            if self.vel.abs() < 1e-3 && torque.abs() <= self.friction {
                self.vel = 0.0;
                continue;
            }
            let dir = if self.vel.abs() >= 1e-3 { self.vel.signum() } else { torque.signum() };
            let v = self.vel + (torque - dir * self.friction) * h / self.inertia;
            // (Friction stops it; it doesn't turn it back.)
            self.vel = if self.vel != 0.0 && v.signum() != self.vel.signum() && torque.abs() <= self.friction { 0.0 } else { v };
            self.angle += self.vel * h;
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
            let pulls: Vec<(f32, Vec3)> = start
                .iter()
                .map(|&a| {
                    let want = a + t * std::f32::consts::FRAC_PI_2;
                    (a, v.centre + v.base * Vec3::new(v.radius * want.cos(), v.radius * want.sin(), 0.0))
                })
                .collect();
            v.step(&pulls, 1.0 / 90.0);
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
                let pulls: Vec<(f32, Vec3)> = hands.iter().map(|&a| (a, v.rim(a) + v.axis().cross(v.rim(a) - v.centre).normalize() * 0.25)).collect();
                v.step(&pulls, 1.0 / 90.0);
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
        v.step(&[], 0.1);
        assert!(v.vel > 4.0 && v.vel < 6.0, "slowing: {}", v.vel);
        for _ in 0..400 {
            v.step(&[], 1.0 / 90.0);
        }
        assert_eq!(v.vel, 0.0, "stopped");
        let a = v.angle;
        v.step(&[], 1.0);
        assert_eq!(v.angle, a, "and stays");
    }

    #[test]
    fn the_nearest_rim_point() {
        let v = wheel(1.0);
        let (at, d) = v.nearest(v.centre + Vec3::new(0.0, 0.25, 0.03));
        assert!((at - std::f32::consts::FRAC_PI_2).abs() < 1e-4 && (d - (0.05f32.powi(2) + 0.03f32.powi(2)).sqrt()).abs() < 1e-4);
        assert!(v.rim(at).distance(v.centre + Vec3::new(0.0, 0.2, 0.0)) < 1e-5);
    }
}
