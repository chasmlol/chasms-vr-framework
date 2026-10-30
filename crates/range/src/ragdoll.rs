//! The range's creatures, simulated (Rapier) as ragdolls held up by pins: a body of thirteen parts
//! on limited joints (pelvis, torso, head, both arms in three, both legs in two), each pulled towards
//! where its animation has it (`crate::anim`) by its pins: position 1000 N/m, damper 50, at most
//! 1000 N; rotation 800 N m/rad, damper 50, at most 100 N m. Standing, gravity is off its parts (it
//! stands on its locomotion capsule); its arms pin twice as stiff and ten times as hard while it
//! swings, three times while it guards (`ATTACK_ARM`, `GUARD_ARM`).
//!
//! It has no balance controller, only states that hits and handling change.
//! * **Standing**: pinned to its animation.
//! * **Destabilised** (knocked off its feet): the position pins let go, the rotation pins go weak
//!   (spring x0.5, damper x0.1; x0.1 once on the ground), gravity is back and it falls. Once its
//!   pelvis has moved slower than 1 m/s for half a second on the ground it gets up, its pins ramping
//!   back over three seconds.
//! * **Limp**: knocked out (blunt to the head) or dead: no pins at all.
//! Its mass: 65 kg standing, 130 while a hand holds it, 227.5 limp or down.
//!
//! The range is the game here: hits land on it by the melee rules (`rustvr_melee`: the part's
//! multiplier, armour as each part's material, health 50, push levels that stagger and floor it, the
//! knockout, a cut through a limb), and it bleeds where it was hit. It holds weapons (the arena's
//! fighters) on fixed joints at its hands, which the player's weapons meet, push and wrench away.
//! A brain (`crate::brain`) animates a fighter; a training dummy just stands.

use glam::{Mat4, Quat, Vec3};
use rapier3d::prelude::*;
use rustvr_melee::body::{Doing, Part as BodyPart, Reaction};
use rustvr_protocol::*;

use crate::anim::{self, PARTS, Pose};

type V3 = Vec3;
type Q = Quat;

pub(crate) fn to_v(v: V3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}
pub(crate) fn from_v(v: Vector) -> V3 {
    V3::new(v.x, v.y, v.z)
}
pub(crate) fn to_pose(p: V3, q: Q) -> rapier3d::math::Pose {
    rapier3d::math::Pose::from_parts(to_v(p), Rotation::from_xyzw(q.x, q.y, q.z, q.w))
}
pub(crate) fn from_pose(p: &rapier3d::math::Pose) -> (V3, Q) {
    (from_v(p.translation), Q::from_xyzw(p.rotation.x, p.rotation.y, p.rotation.z, p.rotation.w))
}

/// Its standing pins: position (spring, damper, most force), then rotation's.
const PIN_SPRING: f32 = 1000.0;
const PIN_DAMPER: f32 = 50.0;
const PIN_MAX: f32 = 1000.0;
const PIN_TURN_SPRING: f32 = 800.0;
const PIN_TURN_DAMPER: f32 = 50.0;
const PIN_TURN_MAX: f32 = 100.0;
/// Down, its rotation pins weaken (spring x0.5, damper x0.1), and more on the ground (spring x0.1).
const LOOSE_TURN_SPRING: f32 = 0.5;
const LOOSE_TURN_DAMPER: f32 = 0.1;
const GROUND_TURN_SPRING: f32 = 0.1;
/// Swinging, its arms' pins (spring x2, damper x0, force x10); guarding (spring x3, damper x1,
/// force x30).
pub const ATTACK_ARM: (f32, f32, f32) = (2.0, 0.0, 10.0);
pub const GUARD_ARM: (f32, f32, f32) = (3.0, 1.0, 30.0);
/// Down long enough and still enough, it gets up (slower than 1 m/s for 0.5 s), its pins ramping
/// back over 3 s.
const STILL_SPEED: f32 = 1.0;
const STILL_TIME: f64 = 0.5;
const RISE_TIME: f64 = 3.0;
/// Its masses (kg): standing, handled, ragdolled.
const MASS_STANDING: f32 = 65.0;
const MASS_HANDLED: f32 = 130.0;
const MASS_RAGDOLLED: f32 = 227.5;
/// The physics' fixed step.
const STEP: f32 = 1.0 / 120.0;
/// A hand's hold: the spring pulling the point it has to where the hand would have it (N/m, and
/// N s/m on the point's speed): the player's hand on a creature (5000, damper x0.5).
const GRAB_SPRING: f32 = 5000.0;
const GRAB_DAMPER: f32 = 50.0;
/// A standing creature lifted by its neck or torso this far above its stance (metres:
/// 0.05, legs 0.1) is destabilised.
const LIFT: f32 = 0.05;
const LIFT_LEGS: f32 = 0.1;
/// Let go while moving faster than this (m/s), it's knocked over.
const THROWN_SPEED: f32 = 2.0;
/// A weapon the player holds pulled this far from its holder's chest (m) for this long (s) is
/// wrenched from its hand.
const WRENCH_REACH: f32 = 0.95;
const WRENCH_TIME: f64 = 0.25;
/// A hand on the throat squeezing past this chokes (one weapon dropped after 2 s,
/// both after 5); it hurts at 5 a second (that it hurts at all is
/// our call, so a choke can kill).
const CHOKE_SQUEEZE: f32 = 0.5;
const CHOKE_DPS: f32 = 5.0;
/// A held creature that's awake struggles: a try every 2 to 3 s, 10% at first, 5% more each time,
/// at most 50%; a try that works wrenches its held part away.
const STRUGGLE_EVERY: f64 = 2.5;
const STRUGGLE_JERK: f32 = 5.0;

/// On fire: 2 damage every half second, and dead once its fire has run its course (s).
const BURN_DPS: f32 = 4.0;
const BURN_DEATH: f64 = 4.0;

/// The most a push may give a part at once (N s).
const PUSH_MAX: f32 = 60.0;
/// A part slamming into something this much faster than it was going is a slam (m/s), and does
/// slam damage.
const SLAM_FROM: f32 = 4.0;

/// One part: a capsule from `a` to `b` (the person's space, standing, feet at the origin, facing
/// +Z), its radius, its share of the mass, its kind, and the part it hangs from with its joint's
/// limits about X, Y and Z (radians).
struct PartSpec {
    a: V3,
    b: V3,
    radius: f32,
    mass: f32,
    kind: u32,
    limits: [[f32; 2]; 3],
}

const fn v(x: f32, y: f32, z: f32) -> V3 {
    V3::new(x, y, z)
}

/// A 1.8 m person in `anim`'s layout. Limits: a knee and an elbow bend one way, a hip mostly
/// forward, the shoulders nearly anywhere, a wrist a little.
fn body() -> [PartSpec; PARTS] {
    use std::f32::consts::FRAC_PI_2 as RIGHT;
    let arm = |s: f32| {
        [
            PartSpec { a: v(0.21 * s, 1.4, 0.0), b: v(0.25 * s, 1.13, 0.0), radius: 0.05, mass: 2.2, kind: PART_ARM, limits: [[-2.0, 2.0], [-RIGHT, RIGHT], [-2.0, 2.0]] },
            PartSpec { a: v(0.25 * s, 1.11, 0.0), b: v(0.27 * s, 0.9, 0.025), radius: 0.045, mass: 1.4, kind: PART_ARM, limits: [[-2.4, 0.05], [-0.3, 0.3], [-0.2, 0.2]] },
            PartSpec { a: v(0.27 * s, 0.86, 0.03), b: v(0.27 * s, 0.78, 0.03), radius: 0.042, mass: 0.4, kind: PART_HAND, limits: [[-1.2, 1.2], [-1.2, 1.2], [-0.8, 0.8]] },
        ]
    };
    let leg = |s: f32| {
        [
            PartSpec { a: v(0.1 * s, 0.88, 0.0), b: v(0.1 * s, 0.52, 0.0), radius: 0.075, mass: 8.5, kind: PART_LEG, limits: [[-1.8, 0.5], [-0.6, 0.6], [-0.6, 0.6]] },
            PartSpec { a: v(0.1 * s, 0.48, 0.0), b: v(0.1 * s, 0.07, 0.0), radius: 0.055, mass: 4.5, kind: PART_LEG, limits: [[-0.05, 2.4], [-0.1, 0.1], [-0.1, 0.1]] },
        ]
    };
    let [lu, lf, lh] = arm(-1.0);
    let [ru, rf, rh] = arm(1.0);
    let [lt, ls] = leg(-1.0);
    let [rt, rs] = leg(1.0);
    [
        // Pelvis, across the hips.
        PartSpec { a: v(-0.08, 0.97, 0.0), b: v(0.08, 0.97, 0.0), radius: 0.12, mass: 11.0, kind: PART_TORSO, limits: [[0.0; 2]; 3] },
        // Torso, belly to the top of the chest.
        PartSpec { a: v(0.0, 1.1, 0.0), b: v(0.0, 1.4, 0.0), radius: 0.15, mass: 24.0, kind: PART_TORSO, limits: [[-0.5, 0.5], [-0.6, 0.6], [-0.4, 0.4]] },
        // Head and neck (the neck its lower part).
        PartSpec { a: v(0.0, 1.5, 0.01), b: v(0.0, 1.66, 0.01), radius: 0.1, mass: 5.5, kind: PART_HEAD, limits: [[-0.7, 0.7], [-1.0, 1.0], [-0.6, 0.6]] },
        lu,
        lf,
        lh,
        ru,
        rf,
        rh,
        lt,
        ls,
        rt,
        rs,
    ]
}

struct Part {
    body: RigidBodyHandle,
    /// The joint to its parent (gone once it's cut off).
    joint: Option<ImpulseJointHandle>,
    /// Its rest centre in the person's space (its capsule along the body's local frame there).
    rest: V3,
    a: V3,
    b: V3,
    radius: f32,
    kind: u32,
    /// What covers it (`MAT_*`): flesh, or armour.
    material: u32,
    mass: f32,
    /// Its speed last step, for slams.
    was: V3,
}

/// Its balance: what holds it up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Balance {
    Standing,
    /// Knocked off its feet since then; still (on the ground, slow) since then.
    Down { since: f64, still: Option<f64> },
    /// Getting up since then.
    Rising(f64),
    /// No pins: knocked out until then (or dead: forever).
    Limp(f64),
}

/// A stagger: how far it reels, which way (its own frame), since when.
#[derive(Clone, Copy, Debug)]
pub struct Stagger {
    pub recoil: f32,
    pub dir: V3,
    pub since: f64,
    pub head: bool,
}

/// A wound where it was hit, on its part (that part's frame): a cut, a stab or a bruise, how big.
#[derive(Clone, Copy, Debug)]
pub struct Wound {
    pub part: usize,
    pub at: V3,
    pub normal: V3,
    pub kind: u32,
    pub size: f32,
}

pub struct Person {
    pub id: u32,
    /// Where it stands (its feet) and which way it faces: where its animation is.
    pub at: (V3, Q),
    parts: Vec<Part>,
    pub balance: Balance,
    /// The animation its pins pull it towards (in its own frame), and how its arms are pinned (1:
    /// standing, or the swing or guard multipliers).
    pub pose: Pose,
    pub arms: [(f32, f32, f32); 2],
    pub health: f32,
    pub alive: bool,
    pub stagger: Option<Stagger>,
    /// When it was last hit, and when its eyes last screwed up in pain.
    pub hurt_at: f64,
    pub wounds: Vec<Wound>,
    /// What each hand holds (an index into the creatures' items).
    pub hands: [Option<usize>; 2],
    /// It's a brute in heavy armour (a Heavy brain: a step tougher to stagger).
    pub heavy: bool,
    /// Its brain's view of what it's doing, for the push table.
    pub doing: Doing,
    /// Which parts were cut off (bits by part index).
    pub severed: u32,
    /// A hand holds it.
    handled: bool,
    /// Heat from a fire-filled blade (0..100, cooling 4 a second alive, 8 dead), and on fire since
    /// then; shocked until then.
    pub heat: f32,
    pub burning: Option<f64>,
    pub shocked: f64,
    /// Choked by a hand since then; when it next tries to struggle free, and how often it has.
    choked_since: Option<f64>,
    choked_by: Option<usize>,
    next_struggle: f64,
    struggles: u32,
}

/// A weapon or shield a creature holds (or dropped): its body in the creatures' world, the object
/// the game registered it as, its shape (boxes, its own space) and where a hand holds it.
pub struct Item {
    pub id: u32,
    body: RigidBodyHandle,
    joint: Option<ImpulseJointHandle>,
    /// Who holds it: (person index, hand).
    pub holder: Option<(usize, usize)>,
    /// Its hold in the hand's frame (the item's pose relative to the hand part).
    grip: (V3, Q),
    pub shield: bool,
    /// The player's hands have it: the runtime moves it (it follows the runtime's pose).
    pub players: bool,
    /// Its blades, for the brains (its own space): (a, b).
    pub blade: (V3, V3),
    pub mass: f32,
    /// The player has been pulling it out past its holder's reach since then.
    wrench: Option<f64>,
}

/// A weapon or shield a creature can hold: its object id, mass, collision, grip and blade.
#[derive(Clone, Debug)]
pub struct ItemSpec {
    pub id: u32,
    pub mass: f32,
    /// Its collision (boxes: centre, half extents, its own space).
    pub boxes: Vec<(V3, V3)>,
    /// Where the hand holds it, its own space (the handle's middle), and the fist's turn.
    pub grip: V3,
    pub shield: bool,
    pub blade: (V3, V3),
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
    throat: bool,
}

/// What happened to the creatures this frame, for the game's side (blood, sounds, the arena).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Happened {
    /// A limb (and what hangs from it) came off creature `id` at `at` (world), the stump's way out.
    Severed { id: u32, part: usize, at: V3, out: V3, radius: f32 },
    /// It was hurt (a grunt), it died, it was knocked out.
    Hurt { id: u32, at: V3 },
    Died { id: u32, at: V3 },
    KnockedOut { id: u32, at: V3 },
    /// It let go of its weapon (hit on the hand, stabbed through the arm, floored while guarding).
    Disarmed { id: u32, item: u32 },
    /// It was knocked off its feet.
    Floored { id: u32, at: V3 },
}

pub struct Dummies {
    world: PhysicsWorld,
    pub people: Vec<Person>,
    pub items: Vec<Item>,
    /// What the hands hold this frame, and the grabs they last let go of (each hand's serial).
    held: Vec<Held>,
    released: [u32; 2],
    /// Time not yet stepped.
    owed: f32,
    pub time: f64,
    pub happened: Vec<Happened>,
}

/// The turn `q` as a rotation vector, the short way round.
fn shortest(q: Q) -> V3 {
    let q = q.normalize();
    let q = if q.w < 0.0 { -q } else { q };
    q.to_scaled_axis()
}

/// Every person's parts and what it holds are in their own collision group: a body doesn't collide
/// with itself, only with the world and other bodies.
fn group(k: usize) -> InteractionGroups {
    let g = Group::from_bits_truncate(1 << (1 + k % 30));
    InteractionGroups::new(g, Group::ALL ^ g, InteractionTestMode::And)
}

/// A loose item: everything.
fn loose() -> InteractionGroups {
    InteractionGroups::new(Group::GROUP_32, Group::ALL, InteractionTestMode::And)
}

impl Dummies {
    /// Creatures among the range's static boxes (`world`: min, max).
    pub fn new(world: &[(V3, V3)]) -> Dummies {
        let mut w = PhysicsWorld::new();
        w.gravity = Vector::new(0.0, -9.81, 0.0);
        for (lo, hi) in world {
            let c = (*lo + *hi) * 0.5;
            let h = (*hi - *lo) * 0.5;
            w.insert_collider(ColliderBuilder::cuboid(h.x, h.y, h.z).translation(to_v(c)).friction(0.8), None);
        }
        Dummies { world: w, people: Vec::new(), items: Vec::new(), held: Vec::new(), released: [0; 2], owed: 0.0, time: 0.0, happened: Vec::new() }
    }

    /// A person standing at `foot` facing `face`, with id `id` and each part covered by `armour`
    /// (`MAT_*` by part, flesh where not said).
    pub fn spawn(&mut self, id: u32, foot: V3, face: Q, armour: &[(usize, u32)]) -> usize {
        let k = self.people.len();
        let spec = body();
        let mut parts: Vec<Part> = Vec::new();
        for (i, p) in spec.iter().enumerate() {
            // Each body's frame at rest is the person's (turned to face): its capsule placed in it.
            let rest = (p.a + p.b) * 0.5;
            let (a, b) = (p.a - rest, p.b - rest);
            let rb = RigidBodyBuilder::dynamic().pose(to_pose(foot + face * rest, face)).gravity_scale(0.0).linear_damping(0.1).angular_damping(0.4).ccd_enabled(true);
            let half = ((b - a).length() * 0.5).max(0.001);
            let along = Quat::from_rotation_arc(V3::Y, (b - a).normalize_or(V3::Y));
            let collider = ColliderBuilder::capsule_y(half, p.radius).position(to_pose(V3::ZERO, along)).mass(p.mass).friction(0.7).collision_groups(group(k));
            let (body, _) = self.world.insert(rb, collider);
            let joint = anim::PARENT[i].map(|parent| {
                let pr = parts[parent].rest;
                let pivot = anim::pivot(i);
                let mut j = SphericalJointBuilder::new().local_anchor1(to_v(pivot - pr)).local_anchor2(to_v(pivot - rest)).contacts_enabled(false);
                for (axis, l) in [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ].into_iter().zip(p.limits) {
                    j = j.limits(axis, l);
                }
                self.world.insert_impulse_joint(parts[parent].body, body, j)
            });
            let material = armour.iter().find(|(part, _)| *part == i).map_or(MAT_FLESH, |(_, m)| *m);
            parts.push(Part { body, joint, rest, a, b, radius: p.radius, kind: p.kind, material, mass: p.mass, was: V3::ZERO });
        }
        self.people.push(Person {
            id,
            at: (foot, face),
            parts,
            balance: Balance::Standing,
            pose: Pose::default(),
            arms: [(1.0, 1.0, 1.0); 2],
            health: rustvr_melee::body::HEALTH,
            alive: true,
            stagger: None,
            hurt_at: -10.0,
            wounds: Vec::new(),
            hands: [None, None],
            heavy: false,
            doing: Doing::Idle,
            severed: 0,
            handled: false,
            heat: 0.0,
            burning: None,
            shocked: -10.0,
            choked_since: None,
            choked_by: None,
            next_struggle: 0.0,
            struggles: 0,
        });
        k
    }

    /// Item `spec` into person `who`'s hand `hand`, held where its grip is (a fixed joint): its handle
    /// along the fist (+Z of the hand's frame), its blade or face as `turn` turns it there.
    pub fn arm(&mut self, who: usize, hand: usize, spec: &ItemSpec, turn: Q) -> usize {
        let hand_body = self.people[who].parts[anim::HAND[hand]].body;
        let (hpos, hrot) = from_pose(self.world.bodies[hand_body].position());
        // The grip a little into the fist from the hand's middle.
        let at_hand = V3::new(0.0, -0.02, 0.0);
        let grip = (at_hand - turn * spec.grip, turn);
        let pose = (hpos + hrot * grip.0, (hrot * grip.1).normalize());
        let k = self.items.len();
        let body = self.item_body(spec, pose, group(who));
        let joint = Some(self.fix(hand_body, body, grip));
        self.items.push(Item { id: spec.id, body, joint, holder: Some((who, hand)), grip, shield: spec.shield, players: false, blade: spec.blade, mass: spec.mass, wrench: None });
        self.people[who].hands[hand] = Some(k);
        k
    }

    fn item_body(&mut self, spec: &ItemSpec, (p, q): (V3, Q), groups: InteractionGroups) -> RigidBodyHandle {
        let rb = RigidBodyBuilder::dynamic().pose(to_pose(p, q)).linear_damping(0.4).angular_damping(0.05).ccd_enabled(true).build();
        let body = self.world.bodies.insert(rb);
        let each = spec.mass / spec.boxes.len().max(1) as f32;
        for (c, h) in &spec.boxes {
            let col = ColliderBuilder::cuboid(h.x, h.y, h.z).translation(to_v(*c)).mass(each).friction(0.6).collision_groups(groups);
            self.world.colliders.insert_with_parent(col, body, &mut self.world.bodies);
        }
        body
    }

    fn fix(&mut self, hand: RigidBodyHandle, item: RigidBodyHandle, grip: (V3, Q)) -> ImpulseJointHandle {
        let j = FixedJointBuilder::new().local_frame1(to_pose(grip.0, grip.1)).local_frame2(to_pose(V3::ZERO, Q::IDENTITY)).contacts_enabled(false);
        self.world.insert_impulse_joint(hand, item, j)
    }

    /// Person `who` lets go of what hand `hand` holds: it falls loose.
    pub fn drop_item(&mut self, who: usize, hand: usize) -> Option<u32> {
        let k = self.people[who].hands[hand].take()?;
        let item = &mut self.items[k];
        item.holder = None;
        let (body, joint) = (item.body, item.joint.take());
        if let Some(j) = joint {
            self.world.remove_impulse_joint(j);
        }
        let cols: Vec<ColliderHandle> = self.world.bodies[body].colliders().to_vec();
        for c in cols {
            self.world.colliders[c].set_collision_groups(loose());
        }
        Some(self.items[k].id)
    }

    /// Person `who` takes loose item `k` into hand `hand`, where it is (its hold the one it was
    /// made with): the brain brought the hand to it.
    pub fn pick_up(&mut self, who: usize, hand: usize, k: usize) {
        if self.items[k].holder.is_some() || self.items[k].players || self.people[who].hands[hand].is_some() {
            return;
        }
        let grip = self.items[k].grip;
        let hp = self.people[who].parts[anim::HAND[hand]].body;
        let (hpos, hrot) = from_pose(self.world.bodies[hp].position());
        let body = self.items[k].body;
        self.world.bodies[body].set_position(to_pose(hpos + hrot * grip.0, (hrot * grip.1).normalize()), true);
        self.world.bodies[body].set_linvel(Vector::ZERO, true);
        let cols: Vec<ColliderHandle> = self.world.bodies[body].colliders().to_vec();
        for c in cols {
            self.world.colliders[c].set_collision_groups(group(who));
        }
        self.items[k].joint = Some(self.fix(hp, body, grip));
        self.items[k].holder = Some((who, hand));
        self.people[who].hands[hand] = Some(k);
    }

    /// The runtime's hands have item `id` (`players`), or let it go with its pose and velocity: the
    /// game takes it back where the runtime left it.
    pub fn players_item(&mut self, id: u32, held: Option<(V3, Q)>, vel: (V3, V3)) {
        let Some(k) = self.items.iter().position(|i| i.id == id) else { return };
        let body = self.items[k].body;
        match held {
            Some((p, q)) => {
                if !self.items[k].players {
                    self.items[k].players = true;
                    self.items[k].wrench = None;
                }
                // Followed where the runtime has it: its holder's hand is dragged along with it
                // (the player wrestling it away).
                let rb = &mut self.world.bodies[body];
                rb.set_body_type(RigidBodyType::KinematicPositionBased, true);
                rb.set_next_kinematic_position(to_pose(p, q));
                // Pulled out past its holder's reach (the arm stretched from its chest) for a
                // moment, it lets go: the player has wrenched it away (`docs/MELEE.md`).
                if let Some((who, hand)) = self.items[k].holder {
                    let chest = from_pose(self.world.bodies[self.people[who].parts[anim::TORSO].body].position()).0;
                    let stretched = p.distance(chest) > WRENCH_REACH;
                    let now = self.time;
                    match (stretched, self.items[k].wrench) {
                        (true, None) => self.items[k].wrench = Some(now),
                        (true, Some(t)) if now - t > WRENCH_TIME => {
                            let id = self.people[who].id;
                            if let Some(item) = self.drop_item(who, hand) {
                                tracing::info!("range: {id:#x}'s {item:#x} wrenched from its hand");
                                self.happened.push(Happened::Disarmed { id, item });
                            }
                        }
                        (false, _) => self.items[k].wrench = None,
                        _ => {}
                    }
                }
            }
            None if self.items[k].players => {
                self.items[k].players = false;
                let rb = &mut self.world.bodies[body];
                rb.set_body_type(RigidBodyType::Dynamic, true);
                rb.set_linvel(to_v(vel.0), true);
                rb.set_angvel(to_v(vel.1), true);
            }
            None => {}
        }
    }

    /// One frame of `dt` seconds: the pins, then the physics at its fixed step.
    pub fn step(&mut self, dt: f32) {
        self.owed = (self.owed + dt.clamp(0.0, 0.1)).min(0.1);
        while self.owed >= STEP {
            self.owed -= STEP;
            self.time += STEP as f64;
            self.balance();
            self.burn();
            self.pin();
            self.hold();
            self.world.integration_parameters.dt = STEP;
            self.world.step();
            self.slams();
        }
    }

    /// The states that change by themselves: down and still long enough, it gets up; up, it
    /// stands; knocked out long enough, it comes to (on the ground).
    fn balance(&mut self) {
        let now = self.time;
        for p in &mut self.people {
            let pelvis = &self.world.bodies[p.parts[anim::PELVIS].body];
            let speed = from_v(pelvis.linvel()).length();
            let (ppos, _) = from_pose(pelvis.position());
            let grounded = ppos.y < 0.5;
            p.balance = match p.balance {
                Balance::Down { since, still } if p.alive && !p.handled => {
                    let still = if speed < STILL_SPEED && grounded { Some(still.unwrap_or(now)) } else { None };
                    match still {
                        Some(t) if now - t > STILL_TIME && now - since > 1.0 => {
                            // Up where it lies, facing the way its body points.
                            let (tpos, trot) = from_pose(self.world.bodies[p.parts[anim::TORSO].body].position());
                            let fwd = trot * V3::Z;
                            let flat = V3::new(fwd.x, 0.0, fwd.z).normalize_or(V3::Z);
                            let up = trot * V3::Y;
                            let face = if up.y.abs() < 0.7 { Quat::from_rotation_arc(V3::Z, V3::new(up.x, 0.0, up.z).normalize_or(V3::Z)) } else { Quat::from_rotation_arc(V3::Z, flat) };
                            p.at = (V3::new(tpos.x, 0.0, tpos.z), face);
                            Balance::Rising(now)
                        }
                        _ => Balance::Down { since, still },
                    }
                }
                Balance::Rising(t) if now - t > RISE_TIME => Balance::Standing,
                Balance::Limp(until) if p.alive && now > until => Balance::Down { since: now, still: None },
                b => b,
            };
        }
    }

    /// Each part pulled towards its animation, as hard as the person's state allows.
    fn pin(&mut self) {
        let now = self.time;
        for p in &mut self.people {
            let shocked = p.shocked > now;
            let (stance, gravity, loose) = match p.balance {
                // Shocked: it shakes on weak pins (it can't fight or stand up).
                Balance::Standing if shocked => (0.35 + 0.15 * ((now * 40.0).sin() as f32), 0.5, 0.2),
                Balance::Standing => (1.0, 0.0, 0.0),
                Balance::Down { still, .. } => (0.0, 1.0, if still.is_some() { GROUND_TURN_SPRING } else { LOOSE_TURN_SPRING }),
                Balance::Rising(t) => {
                    let k = smooth(((now - t) / RISE_TIME) as f32);
                    (k, 1.0 - k, GROUND_TURN_SPRING * (1.0 - k))
                }
                Balance::Limp(_) => (0.0, 1.0, 0.0),
            };
            // Its masses: 65 kg standing, 130 handled, 227.5 down or limp.
            let total = if p.handled { MASS_HANDLED } else if stance < 0.5 { MASS_RAGDOLLED } else { MASS_STANDING };
            let base: f32 = p.parts.iter().map(|q| q.mass).sum();
            let (foot, face) = p.at;
            let mut pose = p.pose;
            // Reeling: the torso and head lean away from the blow, the pelvis gives a little.
            if let Some(s) = p.stagger {
                let k = (((now - s.since) / 0.6) as f32).clamp(0.0, 1.0);
                let lean = s.recoil * (1.0 - k) * (std::f32::consts::PI * k).sin().max(0.0) * 0.6 + s.recoil * (1.0 - k) * 0.2;
                let axis = V3::Y.cross(s.dir).normalize_or(V3::X);
                let turn = Quat::from_axis_angle(axis, lean);
                pose.turn[anim::TORSO] = (turn * pose.turn[anim::TORSO]).normalize();
                if s.head {
                    pose.turn[anim::HEAD] = (Quat::from_axis_angle(axis, lean) * pose.turn[anim::HEAD]).normalize();
                }
                pose.pelvis += s.dir * 0.08 * s.recoil * (1.0 - k);
            }
            let rest: [V3; PARTS] = std::array::from_fn(|i| p.parts[i].rest);
            let targets = pose.targets(foot, face, &rest);
            let poses: Vec<(V3, Q, V3, V3)> = p
                .parts
                .iter()
                .map(|part| {
                    let rb = &self.world.bodies[part.body];
                    let (pos, rot) = from_pose(rb.position());
                    (pos, rot, from_v(rb.linvel()), from_v(rb.angvel()))
                })
                .collect();
            // Each part's mass and its inertia (the mean of its principal moments), what its pins act on.
            let masses: Vec<(f32, f32)> = p
                .parts
                .iter()
                .map(|part| {
                    let rb = &self.world.bodies[part.body];
                    let i = from_v(rb.mass_properties().local_mprops.principal_inertia());
                    (rb.mass().max(0.05), ((i.x + i.y + i.z) / 3.0).max(1e-4))
                })
                .collect();
            let mut torques = vec![V3::ZERO; p.parts.len()];
            let mut forces = vec![V3::ZERO; p.parts.len()];
            for i in 0..p.parts.len() {
                let cut = p.severed & (1 << i) != 0;
                let (pos, rot, vel, ang) = poses[i];
                // The arms pin harder while it swings or guards.
                let arm = (0..2).find(|&h| [anim::UPPER_ARM[h], anim::FOREARM[h], anim::HAND[h]].contains(&i)).map_or((1.0, 1.0, 1.0), |h| p.arms[h]);
                let (target, trot) = targets[i];
                if !cut && stance > 0.0 {
                    let (m, inertia) = masses[i];
                    let f = implicit(m, PIN_SPRING * arm.0 * stance, PIN_DAMPER * arm.1 * stance, target - pos, vel, STEP);
                    forces[i] = f.clamp_length_max(PIN_MAX * arm.2 * stance);
                    let t = implicit(inertia, PIN_TURN_SPRING * arm.0 * stance, PIN_TURN_DAMPER * arm.1 * stance, shortest(trot * rot.inverse()), ang, STEP);
                    torques[i] += t.clamp_length_max(PIN_TURN_MAX * arm.2 * stance);
                }
                // Down: each part keeps its pose on its parent, weakly (the loose rotation pins).
                if let Some(parent) = anim::PARENT[i].filter(|_| !cut && loose > 0.0) {
                    let (_, prot, _, pang) = poses[parent];
                    let want = prot * pose.turn[i];
                    let t = implicit(masses[i].1, PIN_TURN_SPRING * loose, PIN_TURN_DAMPER * LOOSE_TURN_DAMPER, shortest(want * rot.inverse()), ang - pang, STEP).clamp_length_max(PIN_TURN_MAX);
                    torques[i] += t;
                    torques[parent] -= t;
                }
            }
            for (i, part) in p.parts.iter().enumerate() {
                let rb = &mut self.world.bodies[part.body];
                rb.reset_forces(false);
                rb.reset_torques(false);
                let cut = p.severed & (1 << i) != 0;
                rb.set_gravity_scale(if cut { 1.0 } else { gravity }, true);
                rb.set_additional_mass(part.mass * (total / base - 1.0).max(0.0), true);
                rb.add_force(to_v(forces[i]), true);
                rb.add_torque(to_v(torques[i]), true);
            }
        }
    }

    /// A body part hitting the world (or another) much faster than it was going hurts it (the
    /// ragdoll damager on itself: nothing at 4 m/s, 12.5 at 12, times the part's multiplier).
    fn slams(&mut self) {
        for k in 0..self.people.len() {
            if matches!(self.people[k].balance, Balance::Standing) {
                for part in &mut self.people[k].parts {
                    part.was = from_v(self.world.bodies[part.body].linvel());
                }
                continue;
            }
            let mut hurt = 0.0;
            let mut at = V3::ZERO;
            for part in &mut self.people[k].parts {
                let rb = &self.world.bodies[part.body];
                let v = from_v(rb.linvel());
                let change = (part.was - v).length();
                if change > SLAM_FROM {
                    hurt += rustvr_melee::body::slam_damage(change) * BodyPart::of(part.kind, false).damage();
                    at = from_v(rb.translation());
                }
                part.was = v;
            }
            if hurt > 0.0 {
                self.hurt(k, hurt, at);
            }
        }
    }

    /// The runtime's hands on the creatures: each held point pulled to where its hand would have it
    /// (applied every step with the pins); a standing creature lifted by its neck, torso or legs
    /// goes down; a held one stays down; let go fast, it's knocked over and takes the throw.
    pub fn grab(&mut self, grabs: &CreatureGrabs) {
        self.held.clear();
        for p in &mut self.people {
            p.handled = false;
        }
        for (hand, g) in grabs.grabs.iter().enumerate() {
            let Some(k) = self.people.iter().position(|p| p.id == g.actor) else { continue };
            let Some(part) = self.people[k].parts.get(g.part as usize) else { continue };
            // The point is in the part's frame as the runtime has it: its capsule's start, turned as
            // its body is (see `bodies`).
            let local = part.a + V3::from_array(g.point);
            if g.flags & GRAB_HELD != 0 {
                self.held.push(Held { id: g.actor, part: g.part as usize, local, target: V3::from_array(g.target), force: g.force, throat: g.flags & GRAB_THROAT != 0 });
                self.people[k].handled = true;
                continue;
            }
            if g.serial != 0 && self.released[hand] != g.serial {
                self.released[hand] = g.serial;
                let (pos, rot) = from_pose(self.world.bodies[part.body].position());
                let throw = V3::from_array(g.vel);
                if g.flags & GRAB_SLIPPED == 0 && throw.length() > 0.3 {
                    let at = pos + rot * local;
                    self.speed_up(k, g.part as usize, at, throw.normalize(), throw.length());
                    // Let go fast, it's thrown off its feet.
                    if throw.length() > THROWN_SPEED {
                        self.floor(k, at);
                    }
                }
            }
        }
        let now = self.time;
        // Choked: weapons dropped after 2 and 5 seconds, and hurt.
        for (hand, g) in grabs.grabs.iter().enumerate() {
            let held = g.flags & GRAB_HELD != 0 && g.flags & GRAB_THROAT != 0 && g.squeeze > CHOKE_SQUEEZE;
            let Some(k) = self.people.iter().position(|p| p.id == g.actor) else { continue };
            if !held {
                if self.people[k].choked_by == Some(hand) {
                    self.people[k].choked_by = None;
                }
                continue;
            }
            let p = &mut self.people[k];
            let since = *p.choked_since.get_or_insert(now);
            p.choked_by = Some(hand);
            let (choking, id) = (now - since, p.id);
            for (after, h) in [(2.0, 1), (5.0, 0)] {
                if choking > after
                    && let Some(item) = self.drop_item(k, h)
                {
                    self.happened.push(Happened::Disarmed { id, item });
                }
            }
            let at = self.part_pose(k, anim::HEAD).0;
            self.hurt(k, CHOKE_DPS * STEP.max(1.0 / 90.0), at);
        }
        for p in &mut self.people {
            if p.choked_by.is_none() {
                p.choked_since = None;
            }
        }
        // Struggling: an awake creature held tries to break free now and then.
        for h in self.held.clone() {
            let Some(k) = self.people.iter().position(|p| p.id == h.id) else { continue };
            let p = &mut self.people[k];
            if !p.alive || matches!(p.balance, Balance::Limp(_)) {
                continue;
            }
            if now < p.next_struggle {
                continue;
            }
            p.next_struggle = now + STRUGGLE_EVERY + ((now * 3.7).fract() - 0.5);
            let chance = (0.1 + 0.05 * p.struggles as f32).min(0.5);
            p.struggles += 1;
            if (now * 7.31).fract() as f32 >= chance {
                continue;
            }
            p.struggles = 0;
            let rb = &mut self.world.bodies[p.parts[h.part].body];
            let (pos, rot) = from_pose(rb.position());
            let away = (pos + rot * h.local - h.target).normalize_or(V3::Y);
            let jerk = away * STRUGGLE_JERK * rb.mass();
            rb.apply_impulse(to_v(jerk), true);
            tracing::info!("range: {:#x} struggles free", p.id);
        }
        for h in self.held.clone() {
            let Some(k) = self.people.iter().position(|p| p.id == h.id) else { continue };
            let part = &self.people[k].parts[h.part];
            let (pos, rot) = from_pose(self.world.bodies[part.body].position());
            let lifted = (h.target - (pos + rot * h.local)).y;
            let lift = if part.kind == PART_LEG { LIFT_LEGS } else { LIFT };
            let by = matches!(part.kind, PART_HEAD | PART_TORSO | PART_LEG) && (h.throat || part.kind != PART_HEAD);
            if lifted > lift && by && self.people[k].balance == Balance::Standing {
                self.floor(k, pos);
            }
            if let Balance::Down { since, .. } = self.people[k].balance {
                self.people[k].balance = Balance::Down { since: since.max(now - 0.5), still: None };
            }
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

    /// Knocked off its feet: it falls, and lets go of a weapon it was guarding with.
    pub fn floor(&mut self, k: usize, at: V3) {
        let p = &mut self.people[k];
        if !matches!(p.balance, Balance::Standing | Balance::Rising(_)) {
            return;
        }
        p.balance = Balance::Down { since: self.time, still: None };
        p.stagger = None;
        let id = p.id;
        self.happened.push(Happened::Floored { id, at });
    }

    /// The runtime's pushes on their parts and on what they hold: each point made to move along its
    /// direction at least so fast (what's lacking added, at the part's mass).
    pub fn push(&mut self, pushes: &GamePushes) {
        for g in &pushes.pushes[..(pushes.count as usize).min(MAX_GAME_PUSHES)] {
            let point = V3::new(g.point[0], g.point[1], g.point[2]);
            let dir = V3::new(g.normal[0], g.normal[1], g.normal[2]).normalize_or_zero();
            if g.part == 0 {
                // A creature's weapon or shield, pushed by the player's.
                if let Some(item) = self.items.iter().find(|i| i.id == g.id && !i.players) {
                    let rb = &mut self.world.bodies[item.body];
                    let at = from_v(rb.velocity_at_point(to_v(point))).dot(dir);
                    let lacking = g.normal[3] - at;
                    if lacking > 0.0 {
                        let impulse = (dir * lacking * rb.mass()).clamp_length_max(PUSH_MAX);
                        rb.apply_impulse_at_point(to_v(impulse), to_v(point), true);
                    }
                }
                continue;
            }
            let Some(k) = self.people.iter().position(|p| p.id == g.id) else { continue };
            self.speed_up(k, g.part as usize - 1, point, dir, g.normal[3]);
        }
    }

    fn speed_up(&mut self, k: usize, part: usize, point: V3, dir: V3, speed: f32) {
        let Some(part) = self.people[k].parts.get(part) else { return };
        let rb = &mut self.world.bodies[part.body];
        let at = from_v(rb.velocity_at_point(to_v(point))).dot(dir);
        let lacking = speed - at;
        if lacking <= 0.0 {
            return;
        }
        let impulse = (dir * lacking * rb.mass()).clamp_length_max(PUSH_MAX);
        rb.apply_impulse_at_point(to_v(impulse), to_v(point), true);
    }

    /// Damage to person `k` (its health; dead at nothing).
    pub fn hurt(&mut self, k: usize, damage: f32, at: V3) {
        let p = &mut self.people[k];
        if !p.alive || damage <= 0.0 {
            return;
        }
        p.health -= damage;
        p.hurt_at = self.time;
        let id = p.id;
        if p.health <= 0.0 {
            self.kill(k, at);
        } else {
            self.happened.push(Happened::Hurt { id, at });
        }
    }

    pub fn kill(&mut self, k: usize, at: V3) {
        let p = &mut self.people[k];
        if !p.alive {
            return;
        }
        p.alive = false;
        p.health = 0.0;
        p.balance = Balance::Limp(f64::INFINITY);
        p.stagger = None;
        let id = p.id;
        self.happened.push(Happened::Died { id, at });
        // Its hands open.
        for h in 0..2 {
            if let Some(item) = self.drop_item(k, h) {
                self.happened.push(Happened::Disarmed { id, item });
            }
        }
    }

    /// Heat from a fire-filled blade: at 100 it catches fire.
    pub fn heat(&mut self, k: usize, heat: f32, at: V3) {
        let p = &mut self.people[k];
        if p.burning.is_some() {
            return;
        }
        p.heat = (p.heat + heat).min(100.0);
        if p.heat >= 100.0 {
            p.burning = Some(self.time);
            tracing::info!("range: {:#x} catches fire", p.id);
            let _ = at;
        }
    }

    /// Shocked for `seconds`: its arms and legs go stiff and useless (it can't fight or stand up).
    pub fn shock(&mut self, k: usize, seconds: f64) {
        let p = &mut self.people[k];
        p.shocked = p.shocked.max(self.time + seconds);
        p.stagger = None;
        tracing::info!("range: {:#x} shocked for {seconds:.0} s", p.id);
    }

    /// Burning and cooling, once a step: a body on fire takes 2 every half second, and dies 85% of
    /// the way through its fire's course (about 4 s); heat not yet alight cools at 4 a second (8
    /// dead).
    fn burn(&mut self) {
        let now = self.time;
        for k in 0..self.people.len() {
            let p = &mut self.people[k];
            match p.burning {
                Some(since) => {
                    let at = from_pose(self.world.bodies[p.parts[anim::TORSO].body].position()).0;
                    if p.alive && now - since > BURN_DEATH {
                        self.kill(k, at);
                    } else if p.alive {
                        self.hurt(k, BURN_DPS * STEP, at);
                    }
                }
                None => p.heat = (p.heat - if p.alive { 4.0 } else { 8.0 } * STEP).max(0.0),
            }
        }
    }

    /// Whether person `k` is shocked now.
    pub fn is_shocked(&self, k: usize) -> bool {
        self.people[k].shocked > self.time
    }

    /// Knocked out for `seconds`: limp, eyes shut.
    pub fn knock_out(&mut self, k: usize, seconds: f64, at: V3) {
        let p = &mut self.people[k];
        if !p.alive {
            return;
        }
        p.balance = Balance::Limp(self.time + seconds);
        p.stagger = None;
        let id = p.id;
        self.happened.push(Happened::KnockedOut { id, at });
        for h in 0..2 {
            if let Some(item) = self.drop_item(k, h) {
                self.happened.push(Happened::Disarmed { id, item });
            }
        }
    }

    /// A hit's push on person `k`'s part (its push level and the hit-reaction table): a stagger
    /// away from it, or knocked off its feet; and the hit's own shove (its impulse along the way it
    /// went, as the mace and shield damagers add one).
    pub fn react(&mut self, k: usize, part: usize, level: u32, dir: V3, point: V3, shove: f32) {
        let now = self.time;
        let p = &self.people[k];
        let kind = p.parts.get(part).map_or(PART_TORSO, |q| q.kind);
        if let Some(q) = p.parts.get(part) {
            let rb = &mut self.world.bodies[q.body];
            rb.apply_impulse_at_point(to_v(dir * shove), to_v(point), true);
        }
        if !p.alive || !matches!(p.balance, Balance::Standing | Balance::Rising(_)) {
            return;
        }
        let body = BodyPart::of(kind, false);
        let doing = match (p.stagger, p.doing) {
            (Some(s), _) if now - s.since < 0.6 && s.recoil >= 1.0 => Doing::FullyStaggered,
            (Some(s), _) if now - s.since < 0.6 => Doing::Staggered,
            (_, d) => d,
        };
        let r = rustvr_melee::body::reaction(level, body, doing, p.heavy);
        // Getting up, any push puts it back down.
        if matches!(p.balance, Balance::Rising(_)) && level > 0 {
            self.people[k].balance = Balance::Down { since: now, still: None };
            return;
        }
        match r {
            Reaction::None => {}
            Reaction::Destabilize => self.floor(k, point),
            r => {
                let face = self.people[k].at.1;
                let local = (face.inverse() * dir).with_y(0.0).normalize_or(-V3::Z);
                self.people[k].stagger = Some(Stagger { recoil: r.recoil(), dir: local, since: now, head: body == BodyPart::Head });
            }
        }
    }

    /// Cuts person `k`'s part `part` off at its joint (and what hangs from it): its joint gone, its
    /// pins and its children's with it, flung off at half its speed plus 3 m/s along the limb (3.5 a
    /// head). Any part cut off kills.
    pub fn sever(&mut self, k: usize, part: usize, blade_dir: V3) -> bool {
        if part == anim::PELVIS || part == anim::TORSO || self.people[k].severed & (1 << part) != 0 {
            return false;
        }
        let Some(joint) = self.people[k].parts[part].joint.take() else { return false };
        self.world.remove_impulse_joint(joint);
        // It and every part below it.
        let mut cut = 1u32 << part;
        for i in 0..PARTS {
            let mut at = i;
            while let Some(parent) = anim::PARENT[at] {
                if parent == part {
                    cut |= 1 << i;
                }
                at = parent;
            }
        }
        let p = &mut self.people[k];
        p.severed |= cut;
        let id = p.id;
        let q = &p.parts[part];
        let rb = &mut self.world.bodies[q.body];
        let (pos, rot) = from_pose(rb.position());
        let joint_at = pos + rot * q.a;
        let out = (rot * (q.b - q.a)).normalize_or(blade_dir);
        let push = if q.kind == PART_HEAD { rustvr_melee::SLICE_PUSH_HEAD } else { rustvr_melee::SLICE_PUSH };
        let v = from_v(rb.linvel()) * rustvr_melee::SLICE_KEEP + (out + blade_dir * 0.5).normalize_or(out) * push;
        rb.set_linvel(to_v(v), true);
        let radius = q.radius;
        // What a cut-off hand held goes with it (still in its fist).
        self.happened.push(Happened::Severed { id, part, at: joint_at, out, radius });
        let at = joint_at;
        self.kill(k, at);
        true
    }

    /// A wound on person `k`'s part `part` where the hit landed (world): a cut, a stab or a bruise,
    /// as big as the hit was hard (0.1 to 0.18 m, a stab 0.06 to 0.12).
    pub fn wound(&mut self, k: usize, part: usize, point: V3, kind: u32, intensity: f32) {
        let p = &mut self.people[k];
        let Some(q) = p.parts.get(part) else { return };
        let (pos, rot) = from_pose(self.world.bodies[q.body].position());
        let local = rot.inverse() * (point - pos);
        let axis_t = ((local - q.a).dot(q.b - q.a) / (q.b - q.a).length_squared().max(1e-6)).clamp(0.0, 1.0);
        let on = q.a.lerp(q.b, axis_t);
        let normal = (local - on).normalize_or(V3::Z);
        let at = on + normal * q.radius;
        let (lo, hi) = if kind == DMG_PIERCE { (0.06, 0.12) } else { (0.1, 0.18) };
        let size = lo + (hi - lo) * intensity.clamp(0.0, 1.0);
        p.wounds.push(Wound { part, at, normal, kind, size });
        if p.wounds.len() > 24 {
            p.wounds.remove(0);
        }
    }

    /// The people as the game's creatures, every part a capsule where it is now.
    pub fn actors(&self) -> Actors {
        let mut t: Actors = bytemuck::Zeroable::zeroed();
        for (k, p) in self.people.iter().take(MAX_ACTORS).enumerate() {
            let a = &mut t.actors[k];
            a.id = p.id;
            a.flags = if p.alive { ACT_ALIVE } else { 0 } | ACT_HOSTILE | if matches!(p.balance, Balance::Down { .. } | Balance::Limp(_)) { ACT_DOWN } else { 0 };
            a.health = (p.health / rustvr_melee::body::HEALTH).clamp(0.0, 1.0);
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

    /// The bodies of the people (beside [`Dummies::actors`], in its order): each part's rotation,
    /// speed and mass, and whether it's down.
    pub fn bodies(&self) -> ActorBodies {
        let mut t: ActorBodies = bytemuck::Zeroable::zeroed();
        for (k, p) in self.people.iter().take(MAX_ACTORS).enumerate() {
            let b = &mut t.bodies[k];
            b.id = p.id;
            let down = matches!(p.balance, Balance::Down { .. } | Balance::Limp(_));
            let conscious = p.alive && !matches!(p.balance, Balance::Limp(_));
            b.flags = BODY_GRABBABLE | if conscious { BODY_CONSCIOUS } else { 0 } | if down { BODY_RAGDOLL } else { 0 };
            b.mass = if down { MASS_RAGDOLLED } else { MASS_STANDING };
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

    /// What the creatures hold, for the runtime: each item where it is now, and how it moves.
    pub fn held_items(&self, attacking: impl Fn(usize) -> bool, blocking: impl Fn(usize) -> bool, feinting: impl Fn(usize) -> bool) -> (ActorItems, Vec<(u32, V3, Q)>) {
        let mut t: ActorItems = bytemuck::Zeroable::zeroed();
        let mut poses = Vec::new();
        for item in &self.items {
            let rb = &self.world.bodies[item.body];
            let (p, q) = from_pose(rb.position());
            if !item.players {
                poses.push((item.id, p, q));
            }
            let Some((who, hand)) = item.holder else { continue };
            let n = t.count as usize;
            if n >= MAX_ACTOR_ITEMS {
                continue;
            }
            t.items[n] = ActorItem {
                id: item.id,
                actor: self.people[who].id,
                hand: hand as u32,
                flags: if attacking(who) && !item.shield { AI_ATTACKING } else { 0 }
                    | if blocking(who) { AI_BLOCKING } else { 0 }
                    | if item.shield { AI_SHIELD } else { 0 }
                    | if feinting(who) && !item.shield { AI_FEINT } else { 0 },
                vel: from_v(rb.linvel()).to_array(),
                ang_vel: from_v(rb.angvel()).to_array(),
                ..Default::default()
            };
            t.count += 1;
        }
        (t, poses)
    }

    /// Person `k` goes (its body's parts and joints out of the world): the people after it move up
    /// one, and what they hold with them.
    pub fn remove(&mut self, k: usize) {
        let p = self.people.remove(k);
        for part in &p.parts {
            self.world.remove_body(part.body);
        }
        for item in &mut self.items {
            if let Some((who, hand)) = item.holder {
                if who == k {
                    item.holder = None;
                } else if who > k {
                    item.holder = Some((who - 1, hand));
                }
            }
        }
    }

    /// Where item `k` is (world), and how its blade's tip moves.
    pub fn item_pose(&self, k: usize) -> (V3, Q) {
        from_pose(self.world.bodies[self.items[k].body].position())
    }

    /// Where person `k`'s part `part` is (world).
    pub fn part_pose(&self, k: usize, part: usize) -> (V3, Q) {
        from_pose(self.world.bodies[self.people[k].parts[part].body].position())
    }

    /// Person `k`'s part `part`'s capsule (world ends, radius).
    pub fn capsule(&self, k: usize, part: usize) -> (V3, V3, f32) {
        let q = &self.people[k].parts[part];
        let (pos, rot) = self.part_pose(k, part);
        (pos + rot * q.a, pos + rot * q.b, q.radius)
    }

    pub fn part_kind(&self, k: usize, part: usize) -> u32 {
        self.people[k].parts.get(part).map_or(PART_TORSO, |q| q.kind)
    }

    pub fn find(&self, id: u32) -> Option<usize> {
        self.people.iter().position(|p| p.id == id)
    }

    /// Every part as a box round its capsule (the range's unit cube scaled), with its colour, its
    /// wounds and its eyes: (model, tint).
    pub fn draws(&self) -> Vec<(Mat4, [f32; 4])> {
        let mut out = Vec::new();
        for p in &self.people {
            for (i, part) in p.parts.iter().enumerate() {
                let (pos, rot) = from_pose(self.world.bodies[part.body].position());
                let (a, b) = (pos + rot * part.a, pos + rot * part.b);
                let along = Quat::from_rotation_arc(V3::Y, (b - a).normalize_or(rot * V3::Y));
                let size = V3::new(2.0 * part.radius, (b - a).length() + 2.0 * part.radius, 2.0 * part.radius);
                let tint = match part.material {
                    MAT_LEATHER => [0.45, 0.3, 0.18, 1.0],
                    MAT_CHAINMAIL => [0.55, 0.57, 0.6, 1.0],
                    MAT_PLATE => [0.78, 0.8, 0.84, 1.0],
                    _ if part.kind == PART_HEAD || part.kind == PART_HAND => [0.86, 0.68, 0.55, 1.0],
                    _ => [0.77, 0.66, 0.38, 1.0],
                };
                // Paler as it bleeds out; grey dead; charring as it burns, and glowing hot.
                let k = if p.alive { 0.75 + 0.25 * (p.health / rustvr_melee::body::HEALTH).clamp(0.0, 1.0) } else { 0.6 };
                let char = p.burning.map_or(0.0, |t| ((self.time - t) / 10.0).clamp(0.0, 0.85) as f32);
                let hot = (p.heat / 100.0) * 0.4;
                let tint = [(tint[0] * k * (1.0 - char) + hot).min(1.0), tint[1] * k * (1.0 - char) + hot * 0.3, tint[2] * k * (1.0 - char), 1.0];
                out.push((Mat4::from_scale_rotation_translation(size, along, (a + b) * 0.5), tint));
                // Its eyes: open, screwed up in pain for a moment after a hit, shut knocked out or
                // dead, or mid-blink.
                if i == anim::HEAD {
                    let shut = !p.alive || matches!(p.balance, Balance::Limp(_));
                    let pain = self.time - p.hurt_at < 0.75;
                    let h = if shut { 0.004 } else if pain { 0.008 } else { 0.022 };
                    for s in [-1.0f32, 1.0] {
                        let at = pos + rot * (part.b * 0.25 + V3::new(0.035 * s, 0.02, part.radius * 0.95));
                        out.push((Mat4::from_scale_rotation_translation(V3::new(0.025, h, 0.012), rot, at), [0.08, 0.06, 0.05, 1.0]));
                    }
                }
            }
            // Its wounds: dark red on the part where it was cut or stabbed, purple where bruised
            // (a red and a blue channel: blood and the wound's look).
            for w in &p.wounds {
                let part = &p.parts[w.part];
                let (pos, rot) = from_pose(self.world.bodies[part.body].position());
                let at = pos + rot * (w.at + w.normal * 0.004);
                let turn = rot * Quat::from_rotation_arc(V3::Z, w.normal);
                let (shape, tint) = match w.kind {
                    DMG_SLASH => (V3::new(0.012, w.size, 0.004), [0.45, 0.03, 0.03, 1.0]),
                    DMG_PIERCE => (V3::new(w.size * 0.3, w.size * 0.3, 0.004), [0.3, 0.01, 0.01, 1.0]),
                    _ => (V3::new(w.size * 0.6, w.size * 0.6, 0.003), [0.35, 0.15, 0.35, 1.0]),
                };
                out.push((Mat4::from_scale_rotation_translation(shape, turn, at), tint));
            }
        }
        out
    }

    /// Where each held or loose item is, for drawing (its object id, pose).
    pub fn item_draws(&self) -> Vec<(u32, Mat4)> {
        self.items.iter().filter(|i| !i.players).map(|i| {
            let (p, q) = from_pose(self.world.bodies[i.body].position());
            (i.id, Mat4::from_rotation_translation(q, p))
        }).collect()
    }

    /// Each creature's blood and stumps (`GoreBodies`), for the runtime's blood.
    pub fn gore(&self) -> GoreBodies {
        let mut t: GoreBodies = bytemuck::Zeroable::zeroed();
        for (k, p) in self.people.iter().take(MAX_GORE_BODIES).enumerate() {
            let g = &mut t.bodies[k];
            g.id = p.id;
            g.blood = 0;
            g.flags = if p.alive { GB_ALIVE } else { 0 };
            for (i, part) in p.parts.iter().enumerate() {
                if p.severed & (1 << i) == 0 || anim::PARENT[i].is_some_and(|parent| p.severed & (1 << parent) != 0) || g.count as usize >= MAX_STUMPS {
                    continue;
                }
                let parent = anim::PARENT[i].unwrap_or(0);
                let (pos, rot) = from_pose(self.world.bodies[p.parts[parent].body].position());
                let pivot = anim::pivot(i);
                let at = pos + rot * (pivot - p.parts[parent].rest);
                let out = (rot * (pivot - p.parts[parent].rest - (p.parts[parent].a + p.parts[parent].b) * 0.0)).normalize_or(V3::Y);
                g.stumps[g.count as usize] = Stump { at: at.to_array(), radius: part.radius, out: out.to_array(), limb: limb_of(i) };
                g.count += 1;
            }
        }
        t.count = self.people.len().min(MAX_GORE_BODIES) as u32;
        t
    }
}

/// The gore's limb for a part.
pub fn limb_of(part: usize) -> u32 {
    let left = if [3, 4, 5, 9, 10].contains(&part) { LIMB_LEFT } else { 0 };
    left | match part {
        anim::HEAD => LIMB_HEAD,
        3 | 6 => LIMB_UPPER_ARM,
        4 | 7 => LIMB_FOREARM,
        5 | 8 => LIMB_HAND,
        9 | 11 => LIMB_THIGH,
        _ => LIMB_CALF,
    }
}

/// A pin's pull as a joint drive pulls (implicitly: stable however stiff the spring, however light
/// the part): the force (or torque) that takes `m` at velocity `vel`, `err` off its target, to
/// where a spring `k` and damper `c` would have it at the end of a step of `dt`.
fn implicit(m: f32, k: f32, c: f32, err: V3, vel: V3, dt: f32) -> V3 {
    let next = (vel * m + err * k * dt) / (m + c * dt + k * dt * dt);
    (next - vel) * m / dt
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
