//! Anything worked by hand on a hinge or a slide: a game's valve or wheel, a switch's knob, a lever,
//! a door's leaf, a lid, a drawer, a button, a pull chain, rigged to an object ([`OBJ_VALVE`]) so any
//! game with one works the same way. The game reads how far it's turned (its state) and works its
//! own mechanism from that.
//!
//! It turns on an axle fixed in the world: its centre and axis are the object's spawn pose (axis
//! +Z), its rim `ObjectDesc::radius` out, its own weight `mass` (a ring's inertia, m r^2, or the
//! inertia of the part's own shape about the axle when the runtime has it: a door is a slab turned
//! about one edge), and the turning it resists `ObjectDesc::friction` (N m, static and sliding alike,
//! as an old stiff valve or a free-running wheel), scaled by the player's stiffness setting. Or it
//! slides along the axis instead (`JOINT_SLIDE`: a drawer, a button), the same in metres and
//! newtons. It's simulated exactly, one degree of freedom, rather than as a joint in the physics:
//! it can't come off its hinge, stretch, or be flung off. A hand reaching it takes it where it is:
//! on its rim, or, when the runtime has the model of the part that turns (a game's switch handle,
//! its wheel), anywhere on that part ([`Valve::nearest_on`]), or, a part the game draws (a door's
//! leaf), anywhere on its collider, just where the hand is. It turns it one of two ways:
//!
//! * **cranked** (a wheel, a door): the hand pulls its point towards the controller like an arm
//!   would, up to [`ARM_FORCE`] once it's stretched [`ARM_STRETCH`]; so the leverage is the arm's
//!   (near a door's hinge it hardly moves it, at its edge it swings it). Past that stretch the body
//!   pulls (the player leans, or walks off with it): the point comes along with the hand, wherever
//!   the part can take it, rather than the arm stretching on;
//! * **twisted** (a knob, `JOINT_TWIST`, anything smaller than [`TWIST_RADIUS`], or anything held
//!   within that of its axle: a switch's handle by its middle): the wrist turns it as far as the
//!   hand has turned about the axle since it took hold, up to [`WRIST_TORQUE`].
//!
//! A part that swings through the room (`JOINT_PANEL`, a door's leaf) is pushed by a hand pressed
//! on it too: the arm's push where it presses ([`Pull::Press`]), never a pull, so a door can be
//! shoved open with a palm or the back of a hand; and the body's, pressed in past the arm's stretch.
//!
//! One the game moves itself (`OBJF_GAME_MOVED`: a valve on a gate the game may let down, a door an
//! NPC opens) follows the game's pose of it ([`Valve::follow`]) whenever no hand is turning it.
//!
//! Either way each hand adds its own: a light one follows the hand, a stiff one needs both arms or
//! the body, and the hand stays on it (the ghost shows where the controller is) until its
//! controller is [`BREAK`] away. It may turn within a range only (`turn_min`..`turn_max`, a
//! switch's quarter turn, a door's swing), clicking into either end and holding there against up
//! to `detent` (N m). Its own weight may move it (`JOINT_GRAVITY`: a lid falls shut), a spring may
//! pull it back to where it rests (a button, a pull chain: `joint_spring`), and damping slows it
//! (`joint_damping`: the air a door swings through). Let go, it moves on until its friction stops
//! it. One geared to something the game moves at its own pace (a gate it winds up) turns no faster
//! than that (`ObjectDesc::fire_rate`, rad/s).

use glam::{Quat, Vec3};

/// Most an arm pulls the hand's point towards the controller (N), and how far it stretches doing it
/// (metres): its pull grows with the stretch, its stiffness (N a metre) the one over the other.
/// Past that the body pulls, without limit: the point comes along within the stretch of the
/// controller. (Without it a heavy door trailed a walking hand by a quarter of a metre: there's
/// no weight in a controller to feel, only the drawn hand left behind it.)
pub const ARM_FORCE: f32 = 180.0;
pub const ARM_STRETCH: f32 = 0.05;
pub const ARM_STIFFNESS: f32 = ARM_FORCE / ARM_STRETCH;
/// How hard a wrist twists a knob towards where the hand has turned, N m per radian, and most a
/// wrist can twist, N m (about what a firm grip turns a stiff tap with).
pub const WRIST_STIFFNESS: f32 = 60.0;
pub const WRIST_TORQUE: f32 = 6.0;
/// A knob: anything no bigger than this round (metres), or held no further than this from its axle,
/// is twisted, not cranked.
pub const TWIST_RADIUS: f32 = 0.06;
/// Within this far (radians) of a click it pulls into it, and how stiffly (N m per radian).
const DETENT_ZONE: f32 = 0.3;
const DETENT_SPRING: f32 = 40.0;
/// The fastest (rad/s) one the game moves goes where the game has it.
pub const GAME_FOLLOW: f32 = 6.0;
/// How far from the rim (metres) a palm takes hold of it.
pub const REACH: f32 = 0.1;
/// A hand held on it lets go once its controller is this far (metres) from where it holds it: the
/// way the part can't go (off its hinge, past its stops), the body can't bring it. Sooner than an
/// empty hand's leash (0.7 m), as the constrained grips of other games let go sooner than free ones
/// (SteamVR's drives at 20 cm, Godot XR's handles at 30, and the rig's own foregrip at 30): along
/// the way it goes, the body brings it, so nothing is leaned on with a stretched arm.
pub const BREAK: f32 = 0.3;
const SUBSTEPS: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct Valve {
    /// The axle: centre, and the rotation whose +Z is the axis (the wheel at angle 0).
    pub centre: Vec3,
    pub base: Quat,
    pub radius: f32,
    /// Its inertia about the axle (kg m^2); a slide's, its mass (kg).
    pub inertia: f32,
    pub friction: f32,
    /// Turned this far about the axis (radians, unbounded), or slid along it (metres), and how
    /// fast.
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
    /// Slides along its axis instead of turning about it.
    pub slide: bool,
    /// Its weight (N, the world's way down; zero unless its weight moves it) and where it acts:
    /// the centre of mass, in the wheel's own frame.
    pub weight: Vec3,
    pub com: Vec3,
    /// What pulls it back to 0 (N m a radian, a slide's N a metre), and what slows it as it moves
    /// (N m s a radian, N s a metre).
    pub spring: f32,
    pub damping: f32,
}

/// What one hand does to it this step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pull {
    /// Cranked: where the hand holds it (the wheel's own frame), pulled towards the controller (where
    /// it is at the end of the step, and how fast it's going: world), and how far the point may lag
    /// along the way it moves before the body pulls (at least [`ARM_STRETCH`]; more while a hand
    /// that took it from further off hasn't come nearer, so taking it doesn't jerk it).
    Rim { at: Vec3, toward: Vec3, vel: Vec3, slack: f32 },
    /// Twisted: the angle the wrist turns it towards.
    Twist { to: f32 },
    /// Pressed on (a palm on a door's leaf): where (the wheel's own frame), the way it's pushed
    /// (world), how deep the controller is in it past the surface at the end of the step and how fast
    /// it's going on in (m/s along `normal`): the arm pushes, never pulls.
    Press { at: Vec3, normal: Vec3, depth: f32, vel: f32 },
}

/// A hand on a valve: where it holds it (a point in the wheel's own frame) and how the hand sits
/// on it (its rotation in the wheel's frame); and where the hand was when it took hold, and when
/// (it goes onto the valve over the grab transition).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValveGrip {
    pub id: u32,
    pub at: Vec3,
    pub rot: Quat,
    pub from: (Vec3, Quat),
    pub since: f64,
    /// Twisting: how far it was turned when the hand took hold, and how far the hand has turned
    /// about the axle since (unwrapped), and its last reading.
    pub turn0: f32,
    pub twisted: f32,
    pub wrist: f32,
    /// Cranked: how far its point may lag its controller before the body pulls (see [`Pull::Rim`]).
    pub slack: f32,
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
            slide: false,
            weight: Vec3::ZERO,
            com: Vec3::ZERO,
            spring: 0.0,
            damping: 0.0,
        }
    }

    /// Sliding along its axis rather than turning about it: `inertia` is its mass.
    pub fn sliding(self, mass: f32) -> Valve {
        Valve { slide: true, twist: false, inertia: mass.max(0.02), ..self }
    }

    /// Turning only between `min` and `max` (radians; equal: no limit), clicking into both ends
    /// and holding there against up to `detent` N m.
    pub fn with_range(self, min: f32, max: f32, detent: f32) -> Valve {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        Valve { min, max, detent: detent.max(0.0), ..self }
    }

    /// Whether a hand holding it at `at` (its own frame) twists it rather than cranks it: a knob,
    /// or anything held near its axle (where an arm has no leverage, a wrist has).
    pub fn twisted_at(&self, at: Vec3) -> bool {
        !self.slide && (self.twist || at.truncate().length() <= TWIST_RADIUS)
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
        if self.slide { self.base } else { self.base * Quat::from_rotation_z(self.angle) }
    }

    /// Where its frame's origin is now (a slide's moves along the axis).
    pub fn origin(&self) -> Vec3 {
        if self.slide { self.centre + self.axis() * self.angle } else { self.centre }
    }

    pub fn axis(&self) -> Vec3 {
        self.base * Vec3::Z
    }

    /// The rim at angle `at` (wheel frame): in the wheel's frame, and in the world.
    pub fn ring(&self, at: f32) -> Vec3 {
        Vec3::new(self.radius * at.cos(), self.radius * at.sin(), 0.0)
    }

    pub fn rim(&self, at: f32) -> Vec3 {
        self.point(self.ring(at))
    }

    /// A point of the wheel (its own frame) in the world, as it's turned now.
    pub fn point(&self, local: Vec3) -> Vec3 {
        self.origin() + self.rot() * local
    }

    /// A world point in the wheel's own frame, as it's turned now.
    pub fn local(&self, p: Vec3) -> Vec3 {
        self.rot().inverse() * (p - self.origin())
    }

    /// How fast its point `local` (its own frame) moves now (world).
    pub fn point_vel(&self, local: Vec3) -> Vec3 {
        self.moves(local) * self.vel
    }

    /// How its point `local` moves (world) for each radian it turns (a slide's, each metre).
    pub fn moves(&self, local: Vec3) -> Vec3 {
        if self.slide { self.axis() } else { self.axis().cross(self.point(local) - self.centre) }
    }

    /// How far `toward` (world) is ahead of its point `local` along the way that point moves (behind:
    /// less than 0).
    pub fn lead(&self, local: Vec3, toward: Vec3) -> f32 {
        (toward - self.point(local)).dot(self.moves(local).normalize_or_zero())
    }

    /// What a force `f` (world) at its point `local` does to it: the torque about the axle (a
    /// slide's, the force along it).
    pub fn drive(&self, local: Vec3, f: Vec3) -> f32 {
        if self.slide { f.dot(self.axis()) } else { (self.point(local) - self.centre).cross(f).dot(self.axis()) }
    }

    /// How heavy it is at its point `local` to a hand pushing it there the way it moves: its
    /// inertia over the lever arm squared (no nearer the axle than a quarter of its rim), a slide's
    /// its mass.
    pub fn mass_at(&self, local: Vec3) -> f32 {
        if self.slide {
            return self.inertia;
        }
        let r = local.truncate().length().max(self.radius * 0.25).max(0.02);
        self.inertia / (r * r)
    }

    /// The rim's nearest point to `p` (the wheel's frame) and how far `p` is from it.
    pub fn nearest(&self, p: Vec3) -> (Vec3, f32) {
        let local = self.local(p);
        let on = self.ring(local.y.atan2(local.x));
        (on, local.distance(on))
    }

    /// The nearest of `shape`'s points to `p` (the turning part, in the wheel's frame) and how far
    /// `p` is from it; with no shape, the rim's.
    pub fn nearest_on(&self, shape: &[Vec3], p: Vec3) -> (Vec3, f32) {
        let local = self.local(p);
        shape.iter().map(|&q| (q, q.distance(local))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or_else(|| self.nearest(p))
    }

    /// Towards the game's pose of it (`rot`, its rotation in the world): turned as far as that is
    /// about its axle (the nearest way round), no faster than [`GAME_FOLLOW`], within its range.
    pub fn follow(&mut self, rot: Quat, dt: f32) {
        let rel = self.base.inverse() * rot;
        let raw = 2.0 * rel.z.atan2(rel.w);
        let off = (raw - self.angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        let step = off.clamp(-GAME_FOLLOW * dt, GAME_FOLLOW * dt);
        self.angle += step;
        if self.limited() {
            self.angle = self.angle.clamp(self.min, self.max);
        }
        self.vel = if dt > 0.0 { step / dt } else { 0.0 };
    }

    /// A slide towards the game's pose of it (`pos`, where the game has its frame's origin): as far
    /// along its axis, no faster than [`GAME_FOLLOW`] (metres a second), within its range.
    pub fn follow_slide(&mut self, pos: Vec3, dt: f32) {
        let raw = (pos - self.centre).dot(self.axis());
        let step = (raw - self.angle).clamp(-GAME_FOLLOW * dt, GAME_FOLLOW * dt);
        self.angle += step;
        if self.limited() {
            self.angle = self.angle.clamp(self.min, self.max);
        }
        self.vel = if dt > 0.0 { step / dt } else { 0.0 };
    }

    /// One step: each gripping hand pulls or twists (each its own: two hands, twice the force),
    /// each pressing one pushes, its weight, spring and clicks pull it, friction holds or drags,
    /// the body brings each hand's point along past the arm's stretch, and its range stops it.
    /// `stiffness` scales its friction and clicks (the player's setting: 1 as the game made it).
    pub fn step(&mut self, pulls: &[Pull], dt: f32, stiffness: f32) {
        let h = dt / SUBSTEPS as f32;
        let (friction, detent) = (self.friction * stiffness, self.detent * stiffness);
        // An arm, and a wrist, are damped springs: critically damped on what they move, the arm's
        // damping on how fast it stretches (so it keeps up with a hand that moves on steadily).
        let wrist_damp = 2.0 * (WRIST_STIFFNESS * self.inertia).sqrt();
        // Where each pressed point was as the step began (the press's depth is as it ends: it's
        // less as the point gives way).
        let pressed: Vec<Vec3> = pulls.iter().map(|p| if let Pull::Press { at, .. } = *p { self.point(at) } else { Vec3::ZERO }).collect();
        for k in 0..SUBSTEPS {
            // (The controllers are where they are at the end of the step: this far before that.)
            let early = dt - (k + 1) as f32 * h;
            let mut torque = 0.0;
            for (pull, from) in pulls.iter().zip(&pressed) {
                torque += match *pull {
                    Pull::Rim { at, toward, vel, .. } => {
                        let damp = 2.0 * (ARM_STIFFNESS * self.mass_at(at)).sqrt();
                        let toward = toward - vel * early;
                        let f = ((toward - self.point(at)) * ARM_STIFFNESS - (self.point_vel(at) - vel) * damp).clamp_length_max(ARM_FORCE);
                        self.drive(at, f)
                    }
                    Pull::Twist { to } => ((to - self.angle) * WRIST_STIFFNESS - self.vel * wrist_damp).clamp(-WRIST_TORQUE, WRIST_TORQUE),
                    Pull::Press { at, normal, depth, vel } => {
                        // (Moving away from the hand faster than it presses, it's pushed less: the
                        // hand never holds it back.)
                        let damp = 2.0 * (ARM_STIFFNESS * self.mass_at(at)).sqrt();
                        let depth = depth - vel * early - (self.point(at) - *from).dot(normal);
                        let f = (depth * ARM_STIFFNESS - (self.point_vel(at).dot(normal) - vel) * damp).clamp(0.0, ARM_FORCE);
                        self.drive(at, normal * f)
                    }
                };
            }
            // Its weight, its spring and its damping.
            if self.weight != Vec3::ZERO {
                torque += self.drive(self.com, self.weight);
            }
            torque -= self.spring * self.angle + self.damping * self.vel;
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
            let start = self.angle;
            if self.vel.abs() < 1e-3 && torque.abs() <= friction {
                self.vel = 0.0;
            } else {
                let dir = if self.vel.abs() >= 1e-3 { self.vel.signum() } else { torque.signum() };
                let v = self.vel + (torque - dir * friction) * h / self.inertia;
                // (Friction stops it; it doesn't turn it back.)
                self.vel = if self.vel != 0.0 && v.signum() != self.vel.signum() && torque.abs() <= friction { 0.0 } else { v };
                if self.max_speed > 0.0 {
                    self.vel = self.vel.clamp(-self.max_speed, self.max_speed);
                }
                self.angle += self.vel * h;
            }
            for (pull, from) in pulls.iter().zip(&pressed) {
                self.body_pull(pull, *from, early, h, start);
            }
            // Its stops.
            if self.limited() && (self.angle < self.min || self.angle > self.max) {
                self.angle = self.angle.clamp(self.min, self.max);
                self.vel = 0.0;
            }
        }
    }

    /// Past an arm's stretch the body pulls (or presses on in): the hand's point is brought along
    /// the way it moves to within the stretch of its controller (`early` seconds before the end of
    /// the step), and goes on at least as fast as the controller goes that way. No faster than its
    /// gearing, all told this substep (from `start`: a wheel geared to a gate stays behind, until the
    /// hand comes off it), and only the way it moves (pulled off its hinge, it stays: the hand comes
    /// off).
    fn body_pull(&mut self, pull: &Pull, from: Vec3, early: f32, h: f32, start: f32) {
        let (at, gap, along, speed) = match *pull {
            Pull::Rim { at, toward, vel, slack } => {
                let m = self.moves(at).normalize_or_zero();
                let lead = (toward - vel * early - self.point(at)).dot(m);
                (at, lead.abs() - slack.max(ARM_STRETCH), m * lead.signum(), vel.dot(m) * lead.signum())
            }
            Pull::Press { at, normal, depth, vel } => (at, depth - vel * early - (self.point(at) - from).dot(normal) - ARM_STRETCH, normal, vel),
            Pull::Twist { .. } => return,
        };
        // How far it turns (or slides) for the point to go `gap` the way the body takes it: as far
        // as the point goes that way, when the way it moves is off it (a palm pressed slantwise).
        let m = self.moves(at);
        let (reach, square) = (m.dot(along), m.length_squared());
        if gap <= 0.0 || reach.abs() <= 1e-4 || square < 1e-8 {
            return;
        }
        let dir = reach.signum();
        let mut turn = gap * reach.abs() / square;
        if self.max_speed > 0.0 {
            turn = turn.min((self.max_speed * h - (self.angle - start) * dir).max(0.0));
        }
        self.angle += turn * dir;
        // (As fast as the controller goes that way, and never away from it.)
        let mut follow = speed.max(0.0) * reach.abs() / square;
        if self.max_speed > 0.0 {
            follow = follow.min(self.max_speed);
        }
        if self.vel * dir < follow {
            self.vel = follow * dir;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One the game moves goes where the game has it, the nearest way round and no faster than it
    /// may.
    #[test]
    fn a_valve_the_game_moves_follows_it() {
        let mut v = Valve::new(Vec3::ZERO, Quat::IDENTITY, 0.3, 3.0, 4.0).with_range(0.0, 5.5, 0.0);
        v.angle = 3.0;
        // The game has it at 2.5 (turned back: its gate let down).
        for _ in 0..90 {
            v.follow(Quat::from_rotation_z(2.5), 1.0 / 90.0);
        }
        assert!((v.angle - 2.5).abs() < 1e-4, "{}", v.angle);
        // Past half a turn on: unwrapped (5.0 rad, not -1.28).
        for _ in 0..90 {
            v.follow(Quat::from_rotation_z(5.0), 1.0 / 90.0);
        }
        assert!((v.angle - 5.0).abs() < 1e-4, "{}", v.angle);
        // Not faster than it may.
        v.follow(Quat::from_rotation_z(3.0), 0.1);
        assert!((v.angle - (5.0 - GAME_FOLLOW * 0.1)).abs() < 1e-4);
    }

    /// A switch's handle sticking out one way from the hub: the hand takes it on the handle, not
    /// on a circle round the hub; and holds that point of it as it turns.
    #[test]
    fn a_part_is_taken_where_it_is() {
        let mut v = Valve::new(Vec3::new(0.0, 1.2, 1.0), Quat::IDENTITY, 0.13, 0.5, 0.4);
        let handle: Vec<Vec3> = (0..14).map(|k| Vec3::new(k as f32 * 0.01, 0.0, 0.02)).collect();
        let (at, d) = v.nearest_on(&handle, v.centre + Vec3::new(0.1, 0.01, 0.03));
        assert!(at.distance(Vec3::new(0.1, 0.0, 0.02)) < 1e-6 && d < 0.02, "on the handle: {at} {d}");
        // Across the hub from the handle, nothing to take (the rim would be right there).
        let (_, d) = v.nearest_on(&handle, v.centre + Vec3::new(-0.13, 0.0, 0.02));
        assert!(d > REACH, "nothing there: {d}");
        v.angle = std::f32::consts::FRAC_PI_2;
        assert!(v.point(at).distance(v.centre + Vec3::new(0.0, 0.1, 0.02)) < 1e-5, "turned a quarter with it");
    }

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
            let rate = if k <= steps { std::f32::consts::FRAC_PI_2 } else { 0.0 };
            let pulls: Vec<Pull> = start
                .iter()
                .map(|&a| {
                    let want = a + t * std::f32::consts::FRAC_PI_2;
                    let toward = v.centre + v.base * Vec3::new(v.radius * want.cos(), v.radius * want.sin(), 0.0);
                    let vel = v.base * Vec3::new(-want.sin(), want.cos(), 0.0) * v.radius * rate;
                    Pull::Rim { at: v.ring(a), toward, vel, slack: ARM_STRETCH }
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
    fn a_stiff_valve_wont_give_to_one_arm_but_does_to_two_or_the_body() {
        // 45 N m on a 20 cm rim. Each hand pulls round, its controller 4 cm ahead of where it holds
        // along the rim and going round with it (144 N, within the arm's stretch): one arm manages
        // 29 N m, two 58.
        let lead = |v: &mut Valve, hands: &[f32]| {
            for _ in 0..90 {
                let pulls: Vec<Pull> = hands
                    .iter()
                    .map(|&a| {
                        let ahead = v.axis().cross(v.rim(a) - v.centre).normalize() * 0.04;
                        Pull::Rim { at: v.ring(a), toward: v.rim(a) + ahead, vel: v.point_vel(v.ring(a)), slack: ARM_STRETCH }
                    })
                    .collect();
                v.step(&pulls, 1.0 / 90.0, 1.0);
            }
        };
        let mut v = wheel(45.0);
        lead(&mut v, &[std::f32::consts::FRAC_PI_2]);
        assert!(v.angle.abs() < 1e-3, "one arm can't move it: {}", v.angle);
        let mut v = wheel(45.0);
        lead(&mut v, &[std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2]);
        assert!(v.angle > 1.0, "two arms turn it: {}", v.angle);
        // One hand going on round, a quarter turn: past the arm's stretch the body brings it round
        // with it, a stretch behind.
        let mut v = wheel(45.0);
        turn(&mut v, &[std::f32::consts::FRAC_PI_2], std::f32::consts::FRAC_PI_2);
        let behind = (ARM_STRETCH / v.radius).asin();
        assert!((v.angle - (std::f32::consts::FRAC_PI_2 - behind)).abs() < 0.01, "the body turns it: {}", v.angle);
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

    /// A switch's long handle is cranked by its end and twisted by its middle.
    #[test]
    fn a_handle_held_by_its_middle_is_twisted() {
        let v = Valve::new(Vec3::ZERO, Quat::IDENTITY, 0.13, 0.5, 0.4);
        assert!(!v.twist);
        assert!(!v.twisted_at(Vec3::new(0.12, 0.0, 0.02)), "by its end: cranked");
        assert!(v.twisted_at(Vec3::new(0.03, 0.01, 0.04)), "by its middle: twisted");
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
        let at = at.y.atan2(at.x);
        assert!((at - std::f32::consts::FRAC_PI_2).abs() < 1e-4 && (d - (0.05f32.powi(2) + 0.03f32.powi(2)).sqrt()).abs() < 1e-4);
        assert!(v.rim(at).distance(v.centre + Vec3::new(0.0, 0.2, 0.0)) < 1e-5);
    }
}
