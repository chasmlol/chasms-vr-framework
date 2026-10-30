//! The range's creatures: training dummies built as people and simulated (Rapier) as ragdolls
//! held up by pins:
//! eleven parts on limited joints, each part pulled towards its stance by springs (its standing
//! pin forces: position 1000 N/m, damper 50, at most 1000 N; rotation 800 N m/rad, damper 50, at
//! most 100 N m), with gravity off while it stands, so a hit knocks it about and it comes back. A
//! hard hit destabilises it: the position pins let go, the rotation pins go weak (spring
//! x0.5, damper x0.1), gravity is back and it falls; a few seconds later it stands back up.
//!
//! The range is the game here: the runtime gets the dummies as its creatures every frame (capsules
//! on their parts, [`Actors`], and their bodies, [`ActorBodies`]), and they're pushed by what the
//! player's hands do to them (the runtime's [`GamePushes`] naming a part), hit by its blows
//! ([`CombatEvent`]) and held by its hands ([`CreatureGrabs`]): the point a hand has is pulled to
//! where the hand would have it by a stiff spring, as for the player's hand on a creature
//! (5000 N/m), no harder than the hand pulls. A standing dummy lifted by its head or torso is destabilised, as when its
//! hips leave the ground; a held one can't get up; let go, the part takes the throw.

use glam::{Mat4, Quat, Vec3};
use rapier3d::prelude::*;
use rustvr_protocol::*;

type V3 = Vec3;
type Q = Quat;

fn to_v(v: V3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}
fn from_v(v: Vector) -> V3 {
    V3::new(v.x, v.y, v.z)
}
fn to_pose(p: V3, q: Q) -> rapier3d::math::Pose {
    rapier3d::math::Pose::from_parts(to_v(p), Rotation::from_xyzw(q.x, q.y, q.z, q.w))
}
fn from_pose(p: &rapier3d::math::Pose) -> (V3, Q) {
    (from_v(p.translation), Q::from_xyzw(p.rotation.x, p.rotation.y, p.rotation.z, p.rotation.w))
}

/// Its standing pins: position (spring, damper, most force), then rotation's.
const PIN_SPRING: f32 = 1000.0;
const PIN_DAMPER: f32 = 50.0;
const PIN_MAX: f32 = 1000.0;
const PIN_TURN_SPRING: f32 = 800.0;
const PIN_TURN_DAMPER: f32 = 50.0;
const PIN_TURN_MAX: f32 = 100.0;
/// Destabilised (the rotation pins' spring and damper multipliers).
const LOOSE_TURN_SPRING: f32 = 0.5;
const LOOSE_TURN_DAMPER: f32 = 0.1;
/// How long a destabilised dummy stays down, and how long it takes to stand back up.
const DOWN_TIME: f64 = 2.5;
const RISE_TIME: f64 = 1.2;
/// A push or hit asking a part for this much more speed (m/s) destabilises it.
const KNOCKDOWN_SPEED: f32 = 3.0;
/// A hit's impulse (N s) at full strength.
const HIT_IMPULSE: f32 = 9.0;
/// The most a push may give a part at once (N s).
const PUSH_MAX: f32 = 60.0;
/// The physics' fixed step.
const STEP: f32 = 1.0 / 120.0;
/// A hand's hold: the spring pulling the point it has to where the hand would have it (N/m, and
/// N s/m on the point's speed).
const GRAB_SPRING: f32 = 5000.0;
const GRAB_DAMPER: f32 = 100.0;
/// A standing dummy lifted by its head or torso this far above its stance (metres) is destabilised.
const LIFT: f32 = 0.12;
/// A person's mass (kg; the parts' masses add up to it).
const PERSON_KG: f32 = 75.0;

/// One part: a capsule from `a` to `b` (the person's space, standing, feet at the origin, facing
/// +Z), its radius, mass, kind and material, and the part it hangs from (with the joint's point and
/// its limits about X, Y and Z, radians, in the person's space).
struct PartSpec {
    a: V3,
    b: V3,
    radius: f32,
    mass: f32,
    kind: u32,
    material: u32,
    parent: Option<(usize, V3, [[f32; 2]; 3])>,
}

const fn v(x: f32, y: f32, z: f32) -> V3 {
    V3::new(x, y, z)
}

/// A 1.8 m person, 75 kg. Limits: a knee and an elbow bend one way about X (the shin back, the
/// forearm forward), a hip mostly forward, the shoulders nearly anywhere.
fn body() -> Vec<PartSpec> {
    use std::f32::consts::FRAC_PI_2 as RIGHT;
    let mut out = vec![
        // 0 pelvis, across the hips.
        PartSpec { a: v(-0.08, 0.97, 0.0), b: v(0.08, 0.97, 0.0), radius: 0.12, mass: 11.0, kind: PART_TORSO, material: MAT_LEATHER, parent: None },
        // 1 torso, belly to the top of the chest.
        PartSpec { a: v(0.0, 1.1, 0.0), b: v(0.0, 1.4, 0.0), radius: 0.15, mass: 24.0, kind: PART_TORSO, material: MAT_LEATHER, parent: Some((0, v(0.0, 1.03, 0.0), [[-0.5, 0.5], [-0.5, 0.5], [-0.4, 0.4]])) },
        // 2 head and neck.
        PartSpec { a: v(0.0, 1.58, 0.01), b: v(0.0, 1.66, 0.01), radius: 0.1, mass: 5.5, kind: PART_HEAD, material: MAT_FLESH, parent: Some((1, v(0.0, 1.5, 0.0), [[-0.7, 0.7], [-1.0, 1.0], [-0.6, 0.6]])) },
    ];
    for side in [-1.0f32, 1.0] {
        let s = |x: f32| x * side;
        let n = out.len();
        out.push(PartSpec { a: v(s(0.21), 1.4, 0.0), b: v(s(0.25), 1.13, 0.0), radius: 0.05, mass: 2.2, kind: PART_ARM, material: MAT_LEATHER, parent: Some((1, v(s(0.2), 1.42, 0.0), [[-2.0, 2.0], [-RIGHT, RIGHT], [-2.0, 2.0]])) });
        out.push(PartSpec { a: v(s(0.25), 1.11, 0.0), b: v(s(0.27), 0.84, 0.03), radius: 0.045, mass: 1.8, kind: PART_ARM, material: MAT_FLESH, parent: Some((n, v(s(0.25), 1.12, 0.0), [[-2.4, 0.05], [-0.2, 0.2], [-0.2, 0.2]])) });
    }
    for side in [-1.0f32, 1.0] {
        let s = |x: f32| x * side;
        let n = out.len();
        out.push(PartSpec { a: v(s(0.1), 0.88, 0.0), b: v(s(0.1), 0.52, 0.0), radius: 0.075, mass: 8.5, kind: PART_LEG, material: MAT_LEATHER, parent: Some((0, v(s(0.1), 0.93, 0.0), [[-1.8, 0.5], [-0.6, 0.6], [-0.6, 0.6]])) });
        out.push(PartSpec { a: v(s(0.1), 0.48, 0.0), b: v(s(0.1), 0.07, 0.0), radius: 0.055, mass: 4.5, kind: PART_LEG, material: MAT_LEATHER, parent: Some((n, v(s(0.1), 0.5, 0.0), [[-0.05, 2.4], [-0.1, 0.1], [-0.1, 0.1]])) });
    }
    out
}

struct Part {
    body: RigidBodyHandle,
    /// The part it hangs from.
    parent: Option<usize>,
    /// Its rest pose in the person's space (the capsule along the body's local frame there).
    rest: V3,
    /// The capsule's ends in the body's own space, and its radius.
    a: V3,
    b: V3,
    radius: f32,
    kind: u32,
    material: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Standing,
    /// Knocked down since then: the pins weak, gravity on.
    Down(f64),
    /// Getting up since then: the pins coming back.
    Rising(f64),
}

struct Person {
    id: u32,
    /// Where it stands (its feet) and which way it faces.
    at: (V3, Q),
    parts: Vec<Part>,
    state: State,
}

/// A hand holding a part: the point it has (the part's body's own space), where the hand would have
/// it, and how hard it pulls.
#[derive(Clone, Copy, Debug)]
struct Held {
    id: u32,
    part: usize,
    local: V3,
    target: V3,
    force: f32,
}

pub struct Dummies {
    world: PhysicsWorld,
    people: Vec<Person>,
    /// What the hands hold this frame, and the grabs they last let go of (each hand's serial).
    held: Vec<Held>,
    released: [u32; 2],
    /// Time not yet stepped.
    owed: f32,
    time: f64,
}

/// The turn `q` as a rotation vector, the short way round.
fn shortest(q: Q) -> V3 {
    let q = q.normalize();
    let q = if q.w < 0.0 { -q } else { q };
    q.to_scaled_axis()
}

/// Every person's parts are in their own collision group: they don't collide with each other.
fn group(k: usize) -> InteractionGroups {
    let g = Group::from_bits_truncate(1 << (1 + k % 30));
    InteractionGroups::new(g, Group::ALL ^ g, InteractionTestMode::And)
}

impl Dummies {
    /// Dummies standing at `places` (feet, facing), ids from `first_id`, among the range's static
    /// boxes (`world`: min, max).
    pub fn new(places: &[(V3, Q)], first_id: u32, world: &[(V3, V3)]) -> Dummies {
        let mut w = PhysicsWorld::new();
        w.gravity = Vector::new(0.0, -9.81, 0.0);
        for (lo, hi) in world {
            let c = (*lo + *hi) * 0.5;
            let h = (*hi - *lo) * 0.5;
            w.insert_collider(ColliderBuilder::cuboid(h.x, h.y, h.z).translation(to_v(c)).friction(0.8), None);
        }
        let spec = body();
        let mut people = Vec::new();
        for (k, &(foot, face)) in places.iter().enumerate() {
            let mut parts: Vec<Part> = Vec::new();
            for p in &spec {
                // Each body's frame at rest is the person's (turned to face): its capsule placed in it.
                let rest = (p.a + p.b) * 0.5;
                let (a, b) = (p.a - rest, p.b - rest);
                let body = RigidBodyBuilder::dynamic().pose(to_pose(foot + face * rest, face)).gravity_scale(0.0).linear_damping(0.1).angular_damping(0.4).ccd_enabled(true);
                let half = ((b - a).length() * 0.5).max(0.001);
                let along = Quat::from_rotation_arc(V3::Y, (b - a).normalize_or(V3::Y));
                let collider = ColliderBuilder::capsule_y(half, p.radius).position(to_pose(V3::ZERO, along)).mass(p.mass).friction(0.7).collision_groups(group(k));
                let (body, _) = w.insert(body, collider);
                if let Some((parent, joint_at, limits)) = p.parent {
                    let pr = parts[parent].rest;
                    let mut j = SphericalJointBuilder::new().local_anchor1(to_v(joint_at - pr)).local_anchor2(to_v(joint_at - rest)).contacts_enabled(false);
                    for (axis, l) in [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ].into_iter().zip(limits) {
                        j = j.limits(axis, l);
                    }
                    w.insert_impulse_joint(parts[parent].body, body, j);
                }
                parts.push(Part { body, parent: p.parent.map(|(k, ..)| k), rest, a, b, radius: p.radius, kind: p.kind, material: p.material });
            }
            people.push(Person { id: first_id + k as u32, at: (foot, face), parts, state: State::Standing });
        }
        Dummies { world: w, people, held: Vec::new(), released: [0; 2], owed: 0.0, time: 0.0 }
    }

    /// One frame of `dt` seconds: the pins, then the physics at its fixed step.
    pub fn step(&mut self, dt: f32) {
        self.owed = (self.owed + dt.clamp(0.0, 0.1)).min(0.1);
        while self.owed >= STEP {
            self.owed -= STEP;
            self.time += STEP as f64;
            self.pin();
            self.hold();
            self.world.integration_parameters.dt = STEP;
            self.world.step();
        }
    }

    /// The runtime's hands on the dummies: each held point pulled to where its hand would have it
    /// (applied every step with the pins); a standing dummy lifted by its head or torso goes down; a
    /// held one stays down; a part let go of takes the hand's throw.
    pub fn grab(&mut self, grabs: &CreatureGrabs) {
        self.held.clear();
        for (hand, g) in grabs.grabs.iter().enumerate() {
            let Some(p) = self.people.iter().position(|p| p.id == g.actor) else { continue };
            let Some(part) = self.people[p].parts.get(g.part as usize) else { continue };
            // The point is in the part's frame as the runtime has it: its capsule's start, turned as
            // its body is (see `bodies`).
            let local = part.a + V3::from_array(g.point);
            if g.flags & GRAB_HELD != 0 {
                self.held.push(Held { id: g.actor, part: g.part as usize, local, target: V3::from_array(g.target), force: g.force });
                continue;
            }
            if g.serial != 0 && self.released[hand] != g.serial {
                self.released[hand] = g.serial;
                let (pos, rot) = from_pose(self.world.bodies[part.body].position());
                let throw = V3::from_array(g.vel);
                if g.flags & GRAB_SLIPPED == 0 && throw.length() > 0.3 {
                    self.speed_up(g.actor, g.part as usize, pos + rot * local, throw.normalize(), throw.length());
                }
            }
        }
        let now = self.time;
        for h in &self.held {
            let Some(p) = self.people.iter_mut().find(|p| p.id == h.id) else { continue };
            let part = &p.parts[h.part];
            let (pos, rot) = from_pose(self.world.bodies[part.body].position());
            let lifted = (h.target - (pos + rot * h.local)).y > LIFT && matches!(part.kind, PART_HEAD | PART_TORSO);
            p.state = match p.state {
                State::Standing if lifted => State::Down(now),
                // (Held, it stays down.)
                State::Down(_) => State::Down(now),
                s => s,
            };
        }
    }

    /// The held points pulled where the hands would have them (each physics step).
    fn hold(&mut self) {
        for h in &self.held {
            let Some(p) = self.people.iter().find(|p| p.id == h.id) else { continue };
            let rb = &mut self.world.bodies[p.parts[h.part].body];
            let (pos, rot) = from_pose(rb.position());
            let at = pos + rot * h.local;
            let vel = from_v(rb.velocity_at_point(to_v(at)));
            let f = (GRAB_SPRING * (h.target - at) - GRAB_DAMPER * vel).clamp_length_max(h.force);
            rb.add_force_at_point(to_v(f), to_v(at), true);
        }
    }

    /// The bodies of the people (beside [`Dummies::actors`], in its order): each part's rotation,
    /// speed and mass, and whether it's down.
    pub fn bodies(&self) -> ActorBodies {
        let mut t: ActorBodies = bytemuck::Zeroable::zeroed();
        for (k, p) in self.people.iter().take(MAX_ACTORS).enumerate() {
            let b = &mut t.bodies[k];
            b.id = p.id;
            b.flags = BODY_GRABBABLE | BODY_CONSCIOUS | if matches!(p.state, State::Down(_)) { BODY_RAGDOLL } else { 0 };
            b.mass = PERSON_KG;
            for (i, part) in p.parts.iter().take(MAX_ACTOR_PARTS).enumerate() {
                let rb = &self.world.bodies[part.body];
                let (_, rot) = from_pose(rb.position());
                b.parts[i] = PartBody { rot: rot.to_array(), vel: from_v(rb.linvel()).to_array(), mass: rb.mass() };
            }
        }
        t.count = self.people.len().min(MAX_ACTORS) as u32;
        t.strength = 1.0;
        t
    }

    /// Each part pulled towards its stance, as hard as the person's state allows.
    fn pin(&mut self) {
        let now = self.time;
        for p in &mut self.people {
            // Down long enough: up again; up: standing.
            p.state = match p.state {
                State::Down(t) if now - t > DOWN_TIME => State::Rising(now),
                State::Rising(t) if now - t > RISE_TIME => State::Standing,
                s => s,
            };
            // Standing, every part is pinned to its stance in the world (where it stands, upright).
            // Down, only to its pose on the part it hangs from, weakly: the body keeps its shape
            // loosely and falls as a whole. Rising, the stance comes back.
            let (stance, gravity) = match p.state {
                State::Standing => (1.0, 0.0),
                State::Down(_) => (0.0, 1.0),
                State::Rising(t) => {
                    let k = ((now - t) / RISE_TIME).clamp(0.0, 1.0) as f32;
                    (k, 1.0 - k)
                }
            };
            let loose = 1.0 - stance;
            let (foot, face) = p.at;
            let poses: Vec<(V3, Q, V3, V3)> = p
                .parts
                .iter()
                .map(|part| {
                    let rb = &self.world.bodies[part.body];
                    let (pos, rot) = from_pose(rb.position());
                    (pos, rot, from_v(rb.linvel()), from_v(rb.angvel()))
                })
                .collect();
            let mut torques = vec![V3::ZERO; p.parts.len()];
            for (i, part) in p.parts.iter().enumerate() {
                let (pos, rot, vel, ang) = poses[i];
                let target = foot + face * part.rest;
                let force = (PIN_SPRING * (target - pos) - PIN_DAMPER * vel) * stance;
                torques[i] += (PIN_TURN_SPRING * shortest(face * rot.inverse()) - PIN_TURN_DAMPER * ang) * stance;
                // Its pose on its parent (the same turn as the parent's at rest).
                if let Some(parent) = part.parent {
                    let (_, prot, _, pang) = poses[parent];
                    let t = (PIN_TURN_SPRING * LOOSE_TURN_SPRING * shortest(prot * rot.inverse()) - PIN_TURN_DAMPER * LOOSE_TURN_DAMPER * (ang - pang)) * loose;
                    let t = t.clamp_length_max(PIN_TURN_MAX);
                    torques[i] += t;
                    torques[parent] -= t;
                }
                let rb = &mut self.world.bodies[part.body];
                rb.reset_forces(false);
                rb.set_gravity_scale(gravity, true);
                rb.add_force(to_v(force.clamp_length_max(PIN_MAX * stance)), true);
            }
            for (part, t) in p.parts.iter().zip(torques) {
                let rb = &mut self.world.bodies[part.body];
                rb.reset_torques(false);
                rb.add_torque(to_v(t.clamp_length_max(PIN_TURN_MAX * (1.0 + loose))), true);
            }
        }
    }

    /// The runtime's pushes on their parts: each point made to move along its direction at least
    /// so fast (what's lacking added, at the part's mass). A hard one knocks the person down.
    pub fn push(&mut self, pushes: &GamePushes) {
        for g in &pushes.pushes[..(pushes.count as usize).min(MAX_GAME_PUSHES)] {
            if g.part == 0 {
                continue;
            }
            let point = V3::new(g.point[0], g.point[1], g.point[2]);
            let dir = V3::new(g.normal[0], g.normal[1], g.normal[2]).normalize_or_zero();
            self.speed_up(g.id, g.part as usize - 1, point, dir, g.normal[3]);
        }
    }

    /// A hit (`CE_HIT`): its impulse along the way it went, at full strength `HIT_IMPULSE`; a power
    /// hit knocks it down.
    pub fn hit(&mut self, ev: &CombatEvent) {
        let Some(p) = self.people.iter_mut().find(|p| p.id == ev.actor) else { return };
        let Some(part) = p.parts.get(ev.part as usize) else { return };
        let dir = V3::from_array(ev.dir).normalize_or_zero();
        let impulse = dir * HIT_IMPULSE * ev.strength.clamp(0.0, 1.5);
        self.world.bodies[part.body].apply_impulse_at_point(to_v(impulse), to_v(V3::from_array(ev.point)), true);
        if ev.flags & CEF_POWER != 0 {
            p.state = State::Down(self.time);
        }
    }

    fn speed_up(&mut self, id: u32, part: usize, point: V3, dir: V3, speed: f32) {
        let now = self.time;
        let Some(p) = self.people.iter_mut().find(|p| p.id == id) else { return };
        let Some(part) = p.parts.get(part) else { return };
        let rb = &mut self.world.bodies[part.body];
        let at = from_v(rb.velocity_at_point(to_v(point))).dot(dir);
        let lacking = speed - at;
        if lacking <= 0.0 {
            return;
        }
        let impulse = (dir * lacking * rb.mass()).clamp_length_max(PUSH_MAX);
        rb.apply_impulse_at_point(to_v(impulse), to_v(point), true);
        if lacking > KNOCKDOWN_SPEED && p.state == State::Standing {
            p.state = State::Down(now);
        }
    }

    /// The people as the game's creatures, every part a capsule where it is now.
    pub fn actors(&self) -> Actors {
        let mut t: Actors = bytemuck::Zeroable::zeroed();
        for (k, p) in self.people.iter().take(MAX_ACTORS).enumerate() {
            let a = &mut t.actors[k];
            a.id = p.id;
            a.flags = ACT_ALIVE | ACT_HOSTILE;
            a.health = 1.0;
            a.velocity = from_v(self.world.bodies[p.parts[0].body].linvel()).to_array();
            for (i, part) in p.parts.iter().take(MAX_ACTOR_PARTS).enumerate() {
                let (pos, rot) = from_pose(self.world.bodies[part.body].position());
                a.parts[i] = ActorPart { a: (pos + rot * part.a).to_array(), radius: part.radius, b: (pos + rot * part.b).to_array(), kind: part.kind, material: part.material, game_part: i as u32, _pad: [0; 2] };
            }
            a.count = p.parts.len().min(MAX_ACTOR_PARTS) as u32;
        }
        t.count = self.people.len().min(MAX_ACTORS) as u32;
        t
    }

    /// Every part as a box round its capsule (the range's unit cube scaled), with its colour:
    /// (model, tint).
    pub fn draws(&self) -> Vec<(Mat4, [f32; 4])> {
        let mut out = Vec::new();
        for p in &self.people {
            for part in &p.parts {
                let (pos, rot) = from_pose(self.world.bodies[part.body].position());
                let (a, b) = (pos + rot * part.a, pos + rot * part.b);
                let along = Quat::from_rotation_arc(V3::Y, (b - a).normalize_or(rot * V3::Y));
                let size = V3::new(2.0 * part.radius, (b - a).length() + 2.0 * part.radius, 2.0 * part.radius);
                let tint = if part.kind == PART_HEAD { [0.28, 0.22, 0.16, 1.0] } else { [0.77, 0.66, 0.38, 1.0] };
                out.push((Mat4::from_scale_rotation_translation(size, along, (a + b) * 0.5), tint));
            }
        }
        out
    }

    /// Whether person `id` is down (knocked over), for tests.
    #[cfg(test)]
    pub fn is_down(&self, id: u32) -> bool {
        self.people.iter().any(|p| p.id == id && matches!(p.state, State::Down(_)))
    }

    /// Where person `id`'s part `part` is (its capsule's middle), for tests.
    #[cfg(test)]
    pub fn part_at(&self, id: u32, part: usize) -> Option<V3> {
        let p = self.people.iter().find(|p| p.id == id)?;
        let part = p.parts.get(part)?;
        let (pos, rot) = from_pose(self.world.bodies[part.body].position());
        Some(pos + rot * (part.a + part.b) * 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> Vec<(V3, V3)> {
        vec![(V3::new(-10.0, -1.0, -10.0), V3::new(10.0, 0.0, 10.0))]
    }

    #[test]
    fn a_dummy_stands_takes_a_push_and_comes_back() {
        let face = Quat::from_rotation_y(std::f32::consts::PI);
        let mut d = Dummies::new(&[(V3::new(0.0, 0.0, 1.0), face)], 7001, &floor());
        for _ in 0..120 {
            d.step(1.0 / 90.0);
        }
        let chest = d.part_at(7001, 1).unwrap();
        assert!((chest.y - 1.25).abs() < 0.05, "stands: chest at {chest:?}");
        // A shove on the chest (2 m/s): it rocks back, and comes back to its stance.
        let mut p: GamePushes = bytemuck::Zeroable::zeroed();
        p.pushes[0] = GamePush { id: 7001, part: 2, point: (chest).extend(1.0).to_array(), normal: [0.0, 0.0, 1.0, 2.0], ..Default::default() };
        p.count = 1;
        d.push(&p);
        let mut furthest = 0.0f32;
        for _ in 0..30 {
            d.step(1.0 / 90.0);
            furthest = furthest.max(d.part_at(7001, 1).unwrap().z - chest.z);
        }
        assert!(furthest > 0.03, "rocked back: {furthest}");
        assert!(!d.is_down(7001), "a shove isn't a knockdown");
        for _ in 0..180 {
            d.step(1.0 / 90.0);
        }
        assert!(d.part_at(7001, 1).unwrap().distance(chest) < 0.03, "back in its stance");
    }

    #[test]
    fn a_power_hit_knocks_it_down_and_it_gets_up() {
        let mut d = Dummies::new(&[(V3::ZERO, Quat::IDENTITY)], 7, &floor());
        for _ in 0..60 {
            d.step(1.0 / 90.0);
        }
        let head = d.part_at(7, 2).unwrap();
        let ev = CombatEvent { kind: CE_HIT, flags: CEF_POWER, actor: 7, part: 1, strength: 1.2, point: [0.0, 1.3, 0.1], dir: [0.0, 0.0, -1.0], ..Default::default() };
        d.hit(&ev);
        assert!(d.is_down(7));
        for _ in 0..180 {
            d.step(1.0 / 90.0);
        }
        assert!(d.part_at(7, 2).unwrap().y < head.y - 0.5, "fallen: head at {:?}", d.part_at(7, 2));
        // Down 2.5 s, up over 1.2 s.
        for _ in 0..300 {
            d.step(1.0 / 90.0);
        }
        assert!(!d.is_down(7));
        assert!(d.part_at(7, 2).unwrap().distance(head) < 0.08, "standing again: head at {:?}", d.part_at(7, 2));
        // Published as one creature, eleven parts.
        let a = d.actors();
        assert_eq!((a.count, a.actors[0].count), (1, 11));
    }
}
