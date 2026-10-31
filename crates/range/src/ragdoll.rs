//! The range's creatures, simulated (Rapier) as ragdolls held up by pins: a body of thirteen parts
//! on limited joints (pelvis, torso, head, both arms in three, both legs in two), each pulled towards
//! where its animation has it (`crate::anim`) by its pins: position 1000 N/m, damper 50, at most
//! 1000 N; rotation 800 N m/rad, damper 50, at most 100 N m. Standing, gravity is off its parts (it
//! stands on its locomotion capsule); its arms pin twice as stiff and ten times as hard while it
//! swings, three times while it guards (`ATTACK_ARM`, `GUARD_ARM`).
//!
//! It has no balance controller, only states that hits and handling change.
//! * **Standing**: pinned to its animation.
//! * **Down** (knocked off its feet): the position pins let go, the rotation pins go weak (spring
//!   x0.5, damper x0.1; x0.1 once on the ground), gravity is back and it falls. Once its pelvis has
//!   moved slower than 1 m/s for half a second on the ground it gets up, its pins ramping back over
//!   three seconds.
//! * **Limp**: dead, or knocked out while already down: no pins at all. Knocked out standing (its
//!   brain off, its eyes shut for 6 s), it stays on its feet.
//!
//! Its mass: 65 kg standing, 130 while a hand holds it, 227.5 limp or down.
//!
//! The player's hands hold it by the point they grab: each held point pulled to where the hand has
//! it on the player's grip (5000 N/m, 50 against the hand's own motion, at most 3000 N; turning
//! 1000, 50, at most 250 N m), by one of its handles (the neck, the mouth, the skull, the torso, a
//! shoulder, a wrist, a hand, a leg) with that handle's rules for lifting it, flooring it, and how
//! it struggles free.
//!
//! The range is the game here: hits land on it by the melee rules (`rustvr_melee`: the part's
//! multiplier, armour as each part's material, health 50, push levels that stagger and floor it, the
//! knockout, a cut through a limb), and it bleeds where it was hit. It holds weapons (the arena's
//! fighters) on fixed joints at its hands, which the player's weapons meet, push and wrench away.
//! A brain (`crate::brain`) animates a fighter; a training dummy just stands.

use std::collections::HashSet;

use glam::{Mat4, Quat, Vec3};
use rapier3d::prelude::*;
use rustvr_melee::body::{Doing, Part as BodyPart, Reaction};
use rustvr_protocol::*;

use crate::anim::{self, PARTS, Pose};
use crate::brain::{Grappled, Handle};

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
/// A hand's hold on a creature (the player's grip, with its handles' multipliers): the point it has
/// pulled to where the hand would have it at 5000 N/m, damped at 50 N s/m against the hand's own
/// motion, at most the hand's pull (3000 N); the part turned to the hand's turn at
/// 1000 N m a radian, damped at 50, at most 250 N m.
const GRAB_SPRING: f32 = 5000.0;
const GRAB_DAMPER: f32 = 50.0;
const GRAB_TURN_SPRING: f32 = 1000.0;
const GRAB_TURN_DAMPER: f32 = 50.0;
const GRAB_TURN_MAX: f32 = 250.0;
/// A standing creature's root raised this far above its stance by a hand (metres: 0.05; a leg, its
/// own part, 0.1) falls (a neck's or a leg's hold) or the hand comes off (the others').
const LIFT: f32 = 0.05;
const LIFT_LEGS: f32 = 0.1;
/// Let go by its last hand while the part held moves faster than this (m/s), it's knocked over:
/// thrown, on its own momentum.
const THROWN_SPEED: f32 = 2.0;
/// A neck or torso hold tightens the head on its neck (swing 20 degrees, twist 20 one way and 45
/// the other): the nod (radians), and the turn.
const HELD_NOD: [f32; 2] = [-0.349, 0.349];
const HELD_TURN: [f32; 2] = [-0.349, 0.785];
/// A weapon the player holds pulled this far from its holder's chest (m) for this long (s) is
/// wrenched from its hand.
const WRENCH_REACH: f32 = 0.95;
const WRENCH_TIME: f64 = 0.25;
/// A hand on the throat chokes (one weapon dropped after 2 s if it holds two, both after 5); it
/// hurts, 5 every 1 s give or take 0.3, so a choke can kill.
const CHOKE_ONE: f64 = 2.0;
const CHOKE_BOTH: f64 = 5.0;
const CHOKE_DAMAGE: f32 = 5.0;
const CHOKE_EVERY: f64 = 1.0;
const CHOKE_VARY: f64 = 0.3;
/// A held creature that's awake struggles: a try at once, then each 2 to 3 s after its escape's
/// animation (1 s); the chance 10% at first, 5% more each failed try, 10% less held in two hands, at
/// most 50%, back to 10% when it's hurt; a try that works forces the hand off after the handle's
/// escape delay (0.5 s).
const ESCAPE_FIRST: f32 = 0.1;
const ESCAPE_STEP: f32 = 0.05;
const ESCAPE_TWO: f32 = 0.1;
const ESCAPE_MAX: f32 = 0.5;
const ESCAPE_EVERY: (f64, f64) = (2.0, 3.0);
const ESCAPE_ANIM: f64 = 1.0;
const ESCAPE_DELAY: f64 = 0.5;
/// Its eyes blink (0.5 s, every 2.5 to 5 s), shut for the blink's middle.
const BLINK: f64 = 0.5;
const BLINK_EVERY: (f64, f64) = (2.5, 5.0);

/// On fire: 2 damage every half second, and dead once its fire has run its course (s).
const BURN_DPS: f32 = 4.0;
const BURN_DEATH: f64 = 4.0;

/// The most a push may give a part at once (N s).
const PUSH_MAX: f32 = 60.0;
/// A push given as an acceleration is applied once a frame and lands in one physics step (a frame
/// at the headset's rate): we take 90 Hz's (s).
const ADD_FORCE_STEP: f32 = 1.0 / 90.0;
/// A body part slams into the same thing again only after this (s).
const SLAM_AGAIN: f64 = 0.1;
/// A body's damage against another creature it slams into, by speed (0 at a standstill, 5 at
/// 12 m/s, its keys' slopes 0.282 and 0.830).
const BODY_CURVE: [f32; 4] = [0.0, 0.0, 12.0, 5.0];
const BODY_SLOPES: [f32; 2] = [0.282_455_6, 0.829_812_8];

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
    collider: ColliderHandle,
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
    /// Its speed before the last step (a slam's impact speed), and when it last slammed.
    was: V3,
    slammed: f64,
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
    pub look: Look,
    pub size: f32,
    /// How strongly it shows (0..1), and when it was made (a burn glows as it cools).
    pub strength: f32,
    pub since: f64,
}

/// A mark's look, as its wound masks paint them: on flesh a cut, a stab or a bruise (red, its
/// blood, with the wound's own look); a burn where the blade was filled with fire; on armour a blow
/// that went through marks it as on flesh, and one that didn't scratches it (an edge) or scuffs it
/// (a blunt one): only the mask's wear channel, no blood.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    Cut,
    Stab,
    Bruise,
    Burn,
    Scratch,
    Scuff,
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
    /// Its brain's push table (Easy, Medium and Hard, and the Heavy brains' in heavy armour, a step
    /// tougher to stagger).
    pub table: rustvr_melee::body::Table,
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
    /// Knocked out: its brain off and its eyes shut until it wakes.
    pub stun: Option<Stun>,
    /// Its eyes last blinked then, and blink next then.
    blink: f64,
    next_blink: f64,
    /// Its voice is cut (a stab through its throat), and a hand is over its mouth.
    pub silenced: bool,
    pub muffled: bool,
    /// What last hurt it (`DMG_*`, 0 a choke) and where, for its death's line.
    pub last_hit: (u32, BodyPart),
    /// Choked by a hand since then, and its choke's next hurt.
    choked_since: Option<f64>,
    next_choke: f64,
    /// Struggling free: its next try, its failed tries, its last try (for the brain's twist), and a
    /// hold it has broken free of (forced off then: its serial).
    next_struggle: f64,
    struggles: u32,
    pub struggled: f64,
    escape: Option<(f64, u32)>,
    /// Its head is held tight on its neck (a neck or torso hold).
    tight: bool,
}

/// Knocked out until then; extended once by a push since.
#[derive(Clone, Copy, Debug)]
pub struct Stun {
    pub wake: f64,
    extended: bool,
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

/// A hand holding a part: its hold's serial, the point it has (the part's body's own space), where
/// the hand would have it and how it would have the part turned, how the hand moves, how hard it
/// pulls, and by which of its handles it holds.
#[derive(Clone, Copy, Debug)]
struct Held {
    id: u32,
    serial: u32,
    part: usize,
    local: V3,
    target: V3,
    target_rot: Q,
    hand_vel: V3,
    hand_spin: V3,
    force: f32,
    handle: Handle,
}

/// A shove on a body that isn't standing, a moment long: the blow at a point on a part (its own
/// space), for every part of its body, until then.
#[derive(Clone, Copy, Debug)]
struct Shove {
    id: u32,
    part: usize,
    local: V3,
    /// The blow's velocity times its push (an acceleration, or with `impulse` an impulse, per m/s of
    /// the blow, each second), times this on the part hit and on the rest.
    push: V3,
    part_x: f32,
    other_x: f32,
    impulse: bool,
    until: f64,
}

/// A part of person `k` slamming into something (another person's part, or the world) at `at`, at
/// `vel` against it.
struct Slam {
    k: usize,
    part: usize,
    other: Option<(usize, usize)>,
    at: V3,
    vel: V3,
}

/// What happened to the creatures this frame, for the game's side (blood, sounds, the arena).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Happened {
    /// A limb (and what hangs from it) came off creature `id` at `at` (world), the stump's way out.
    Severed { id: u32, part: usize, at: V3, out: V3, radius: f32 },
    /// It was hurt (a grunt, if `voice`), it died, it was knocked out.
    Hurt { id: u32, at: V3, voice: bool },
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
    /// What the hands hold this frame, the grabs they last let go of and held last frame (each
    /// hand's serial), and the holds the creatures have broken (struggled free of, or lifted off a
    /// handle that won't take it), which they ignore until the runtime lets go.
    held: Vec<Held>,
    released: [u32; 2],
    holding: [u32; 2],
    freed: Vec<u32>,
    /// The shoves under way.
    shoves: Vec<Shove>,
    /// The parts' contacts last step (a slam is a new one).
    touching: HashSet<(ColliderHandle, ColliderHandle)>,
    rng: u32,
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
        Dummies {
            world: w,
            people: Vec::new(),
            items: Vec::new(),
            held: Vec::new(),
            released: [0; 2],
            holding: [0; 2],
            freed: Vec::new(),
            shoves: Vec::new(),
            touching: HashSet::new(),
            rng: 0x9e37_79b9,
            owed: 0.0,
            time: 0.0,
            happened: Vec::new(),
        }
    }

    /// A person standing at `foot` facing `face`, with id `id` and each part covered by `armour`
    /// (`MAT_*` by part, flesh where not said).
    pub fn spawn(&mut self, id: u32, foot: V3, face: Q, armour: &[(usize, u32)]) -> usize {
        let k = self.people.len();
        let spec = body();
        // 65 kg standing, shared among the parts as our body's proportions have it.
        let scale = MASS_STANDING / spec.iter().map(|p| p.mass).sum::<f32>();
        let mut parts: Vec<Part> = Vec::new();
        for (i, p) in spec.iter().enumerate() {
            // Each body's frame at rest is the person's (turned to face): its capsule placed in it.
            let rest = (p.a + p.b) * 0.5;
            let (a, b) = (p.a - rest, p.b - rest);
            let rb = RigidBodyBuilder::dynamic().pose(to_pose(foot + face * rest, face)).gravity_scale(0.0).linear_damping(0.1).angular_damping(0.4).ccd_enabled(true);
            let half = ((b - a).length() * 0.5).max(0.001);
            let along = Quat::from_rotation_arc(V3::Y, (b - a).normalize_or(V3::Y));
            let mass = p.mass * scale;
            let collider = ColliderBuilder::capsule_y(half, p.radius).position(to_pose(V3::ZERO, along)).mass(mass).friction(0.7).collision_groups(group(k));
            let (body, collider) = self.world.insert(rb, collider);
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
            parts.push(Part { body, collider, joint, rest, a, b, radius: p.radius, kind: p.kind, material, mass, was: V3::ZERO, slammed: -1.0 });
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
            table: rustvr_melee::body::Table::Medium,
            doing: Doing::Idle,
            severed: 0,
            handled: false,
            heat: 0.0,
            burning: None,
            shocked: -10.0,
            stun: None,
            blink: -10.0,
            next_blink: self.time + 1.0 + (id % 7) as f64 * 0.5,
            silenced: false,
            muffled: false,
            last_hit: (0, BodyPart::Torso),
            choked_since: None,
            next_choke: 0.0,
            next_struggle: 0.0,
            struggles: 0,
            struggled: -10.0,
            escape: None,
            tight: false,
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
            self.shoves();
            self.remember();
            self.world.integration_parameters.dt = STEP;
            self.world.step();
            self.slams();
        }
    }

    /// The states that change by themselves: down and still long enough, it gets up (not while a
    /// hand holds it, it's shocked or it's out cold); up, it stands; out cold long enough, it comes
    /// to (on the ground, if it went limp); and its eyes blink.
    fn balance(&mut self) {
        let now = self.time;
        let mut rng = self.rng;
        for p in &mut self.people {
            let pelvis = &self.world.bodies[p.parts[anim::PELVIS].body];
            let speed = from_v(pelvis.linvel()).length();
            let (ppos, _) = from_pose(pelvis.position());
            let grounded = ppos.y < 0.5;
            if p.stun.is_some_and(|s| now > s.wake) {
                p.stun = None;
                tracing::info!("range: {:#x} comes to", p.id);
            }
            if now >= p.next_blink {
                p.blink = now;
                p.next_blink = now + BLINK_EVERY.0 + (BLINK_EVERY.1 - BLINK_EVERY.0) * next(&mut rng) as f64;
            }
            let may_rise = p.alive && !p.handled && p.shocked <= now && p.stun.is_none();
            p.balance = match p.balance {
                Balance::Down { since, still } if may_rise => {
                    let still = if speed < STILL_SPEED && grounded { Some(still.unwrap_or(now)) } else { None };
                    match still {
                        Some(t) if now - t > STILL_TIME => {
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
                Balance::Down { since, .. } => Balance::Down { since, still: None },
                Balance::Rising(t) if now - t > RISE_TIME => Balance::Standing,
                Balance::Limp(until) if p.alive && now > until => Balance::Down { since: now, still: None },
                b => b,
            };
        }
        self.rng = rng;
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
            // Its masses: 130 kg handled, else from 227.5 ragdolled to 65 standing by how hard its
            // position pins hold it.
            let total = if p.handled { MASS_HANDLED } else { MASS_RAGDOLLED + (MASS_STANDING - MASS_RAGDOLLED) * stance.clamp(0.0, 1.0) };
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
            // Each part's mass and its inertia (the mean of its principal moments), what its pins act
            // on; a hand's with what it holds welded to it (an NPC's grip is a locked joint: one body
            // its drive moves, so a heavy weapon doesn't whip a light hand about).
            let mut masses: Vec<(f32, f32)> = p
                .parts
                .iter()
                .map(|part| {
                    let rb = &self.world.bodies[part.body];
                    let i = from_v(rb.mass_properties().local_mprops.principal_inertia());
                    (rb.mass().max(0.05), ((i.x + i.y + i.z) / 3.0).max(1e-4))
                })
                .collect();
            for h in 0..2 {
                let Some(item) = p.hands[h].map(|k| &self.items[k]).filter(|it| !it.players) else { continue };
                let (hand, held) = (&self.world.bodies[p.parts[anim::HAND[h]].body], &self.world.bodies[item.body]);
                let i = from_v(held.mass_properties().local_mprops.principal_inertia());
                let lever = from_v(held.center_of_mass()).distance(from_v(hand.translation()));
                masses[anim::HAND[h]].0 += held.mass();
                masses[anim::HAND[h]].1 += (i.x + i.y + i.z) / 3.0 + held.mass() * lever * lever;
            }
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

    /// Each part's speed before the step: a slam's impact speed.
    fn remember(&mut self) {
        for p in &mut self.people {
            for part in &mut p.parts {
                part.was = from_v(self.world.bodies[part.body].linvel());
            }
        }
    }

    /// A body that isn't standing hurts itself on what it slams into (only while it's down or dead):
    /// at a new contact's impact speed (the two bodies' speeds against each other before the step),
    /// against the world by `slam_damage` (nothing at 4 m/s, 12.5 at 12), against another creature by
    /// [`BODY_CURVE`] (0 to 5 at 12), each by its own armour's row of the ragdoll table there
    /// (flesh from 5 m/s at x1, leather 8 at x0.1, chainmail 6 at x0.75, plate 7 at x0.2; a head's
    /// from 3, 4, 4 and 5) and its part's multiplier. A standing creature it slams into takes the same
    /// curve's damage, and its push.
    fn slams(&mut self) {
        let now = self.time;
        let mut owner = std::collections::HashMap::new();
        for (k, p) in self.people.iter().enumerate() {
            for (i, part) in p.parts.iter().enumerate() {
                owner.insert(part.collider, (k, i));
            }
        }
        let mut touching = HashSet::new();
        let mut slams: Vec<Slam> = Vec::new();
        for pair in self.world.contact_pairs() {
            if !pair.has_any_active_contact() {
                continue;
            }
            let (c1, c2) = (pair.collider1, pair.collider2);
            let key = if c1.into_raw_parts() <= c2.into_raw_parts() { (c1, c2) } else { (c2, c1) };
            touching.insert(key);
            if self.touching.contains(&key) {
                continue;
            }
            let Some((_, contact)) = pair.find_deepest_contact() else { continue };
            let (p1, q1) = from_pose(self.world.colliders[c1].position());
            let at = p1 + q1 * from_v(contact.local_p1);
            // Each body's speed (the bodies' own, not their spin at the contact): a part's from
            // before the step, a loose thing's now, the world's nothing.
            let speed_of = |c: ColliderHandle| -> V3 {
                match owner.get(&c) {
                    Some(&(k, i)) => self.people[k].parts[i].was,
                    None => self.world.colliders[c].parent().map_or(V3::ZERO, |b| from_v(self.world.bodies[b].linvel())),
                }
            };
            for (me, other) in [(c1, c2), (c2, c1)] {
                let Some(&(k, i)) = owner.get(&me) else { continue };
                if matches!(self.people[k].balance, Balance::Standing | Balance::Rising(_)) && self.people[k].alive {
                    continue;
                }
                let vel = speed_of(me) - speed_of(other);
                slams.push(Slam { k, part: i, other: owner.get(&other).copied(), at, vel });
            }
        }
        self.touching = touching;
        for Slam { k, part: i, other, at, vel } in slams {
            let speed = vel.length();
            if self.people[k].parts[i].slammed > now - SLAM_AGAIN {
                continue;
            }
            let part = &self.people[k].parts[i];
            let (kind, material) = (part.kind, part.material);
            let table = if kind == PART_HEAD { rustvr_melee::MOD_RAGDOLL_HEAD } else { rustvr_melee::MOD_RAGDOLL };
            let Some(row) = rustvr_melee::row(table, material, 0, false).or_else(|| rustvr_melee::row(rustvr_melee::MOD_RAGDOLL, material, 0, false)) else { continue };
            if speed < row.min_speed {
                continue;
            }
            self.people[k].parts[i].slammed = now;
            let base = match other {
                Some(_) => rustvr_melee::curve(BODY_CURVE, BODY_SLOPES, speed),
                None => rustvr_melee::body::slam_damage(speed),
            };
            let damage = base * row.damage * BodyPart::of(kind, false).damage();
            if damage > 0.0 {
                tracing::info!("range: {:#x}'s part {i} slams into {} at {speed:.1} m/s: {damage:.1}", self.people[k].id, if other.is_some() { "a body" } else { "the world" });
                self.hurt(k, damage, at);
            }
            // A standing creature it slams into is hurt by it too, and pushed.
            if let Some((o, j)) = other
                && matches!(self.people[o].balance, Balance::Standing | Balance::Rising(_))
            {
                self.hurt(o, base, at);
                self.react(o, j, row.push_level(speed, false), vel.normalize_or_zero(), at, false);
            }
        }
    }

    /// The runtime's hands on the creatures: each held point pulled to where its hand would have it
    /// (every step, with the pins), by the handle it's on. Held standing, a creature is floored at
    /// once in the states its handle says, falls or shakes the hand off when it's lifted off its
    /// feet, and falls when its head is pressed below its hips; held by the throat it's choked;
    /// awake, it struggles free now and then. Let go of by its last hand while the part held moves
    /// fast, it's thrown down.
    pub fn grab(&mut self, grabs: &CreatureGrabs) {
        let now = self.time;
        self.held.clear();
        for p in &mut self.people {
            p.handled = false;
            p.muffled = false;
        }
        // The holds broken that the runtime has since let go of are done with.
        self.freed.retain(|s| grabs.grabs.iter().any(|g| g.serial == *s && g.flags & GRAB_HELD != 0));
        for (hand, g) in grabs.grabs.iter().enumerate() {
            let Some(k) = self.find(g.actor) else { continue };
            let Some(part) = self.people[k].parts.get(g.part as usize) else { continue };
            // The point is in the part's frame as the runtime has it: its capsule's start, turned as
            // its body is (see `bodies`).
            let local = part.a + V3::from_array(g.point);
            let (body, (pos, rot)) = (part.body, from_pose(self.world.bodies[part.body].position()));
            if g.flags & GRAB_HELD != 0 {
                let handle = handle_of(g.part as usize, part, g.flags & GRAB_THROAT != 0, local);
                let new = self.holding[hand] != g.serial;
                self.holding[hand] = g.serial;
                if self.freed.contains(&g.serial) {
                    continue;
                }
                let turn = Quat::from_array(g.target_rot);
                let target_rot = if turn.length_squared() > 0.5 && turn.is_finite() { turn.normalize() } else { rot };
                let first = !self.held.iter().any(|h| h.id == g.actor);
                self.held.push(Held { id: g.actor, serial: g.serial, part: g.part as usize, local, target: V3::from_array(g.target), target_rot, hand_vel: V3::from_array(g.vel), hand_spin: V3::from_array(g.ang_vel), force: g.force, handle });
                let p = &mut self.people[k];
                if new {
                    tracing::info!("range: {:#x} held by its {handle:?}", p.id);
                    // An escape is tried at once when a first hand takes hold.
                    if first && !p.handled {
                        p.next_struggle = now;
                    }
                }
                p.handled = true;
                p.muffled |= handle == Handle::Mouth;
                continue;
            }
            self.holding[hand] = 0;
            if g.serial != 0 && self.released[hand] != g.serial {
                self.released[hand] = g.serial;
                let other = grabs.grabs[hand ^ 1];
                if other.flags & GRAB_HELD == 0 || other.actor != g.actor {
                    let speed = from_v(self.world.bodies[body].linvel()).length();
                    self.people[k].struggles = 0;
                    if speed > THROWN_SPEED && self.people[k].alive {
                        tracing::info!("range: {:#x} let go of at {speed:.1} m/s: thrown down", self.people[k].id);
                        self.floor(k, pos);
                    }
                }
            }
        }
        let mut rng = self.rng;
        for k in 0..self.people.len() {
            let id = self.people[k].id;
            let holds: Vec<Held> = self.held.iter().filter(|h| h.id == id).copied().collect();
            // A neck or torso hold tightens its head on its neck.
            let tight = holds.iter().any(|h| matches!(h.handle, Handle::Neck | Handle::Torso));
            if tight != self.people[k].tight {
                self.neck_limits(k, tight);
            }
            self.choke(k, holds.iter().any(|h| h.handle == Handle::Neck), &mut rng);
            let p = &self.people[k];
            if holds.is_empty() || !p.alive || p.balance != Balance::Standing {
                continue;
            }
            // Forced down: held by the neck or torso while idle, reeling fully or shocked; by a
            // leg while idle, attacking, reeling at all or shocked; by a wrist while shocked: down at
            // once.
            let reeling = p.stagger.filter(|s| now - s.since < 0.6);
            let fully = reeling.is_some_and(|s| s.recoil >= 1.0);
            let shocked = p.shocked > now;
            let forced = holds.iter().find(|h| match h.handle {
                Handle::Neck | Handle::Torso => p.doing == Doing::Idle || fully || shocked,
                Handle::Legs => p.doing == Doing::Idle || p.doing == Doing::Attacking || reeling.is_some() || shocked,
                Handle::Wrist => shocked,
                _ => false,
            });
            let feet = p.at.0;
            if let Some(h) = forced {
                tracing::info!("range: {id:#x} taken by its {:?} {}: down", h.handle, if shocked { "shocked" } else if p.doing == Doing::Idle { "unawares" } else { "reeling" });
                let at = self.part_pose(k, h.part).0;
                self.floor(k, at);
                continue;
            }
            // Lifted off its feet: its root (a leg its own part) above its stance by `LIFT`, a
            // neck's or a leg's hold floors it, the others' come off.
            let mut lifted = None;
            for h in &holds {
                let (part, offset) = match h.handle {
                    Handle::Hand => continue,
                    Handle::Legs => (h.part, LIFT_LEGS),
                    _ => (anim::PELVIS, LIFT),
                };
                let height = self.part_pose(k, part).0.y - feet.y;
                if height > self.people[k].parts[part].rest.y + offset {
                    lifted = Some(*h);
                    break;
                }
            }
            if let Some(h) = lifted {
                let at = self.part_pose(k, h.part).0;
                if matches!(h.handle, Handle::Neck | Handle::Legs) {
                    tracing::info!("range: {id:#x} lifted by its {:?}: down", h.handle);
                    self.floor(k, at);
                } else {
                    tracing::info!("range: {id:#x} lifted by its {:?}: the hand comes off", h.handle);
                    self.freed.push(holds[0].serial);
                }
                continue;
            }
            // Its head pressed below its hips' standing height, it falls.
            let head = self.part_pose(k, anim::HEAD).0;
            if head.y - feet.y < self.people[k].parts[anim::PELVIS].rest.y {
                tracing::info!("range: {id:#x} pressed down below its hips: down");
                self.floor(k, head);
                continue;
            }
            // Struggling free, awake and not reeling.
            let p = &mut self.people[k];
            if let Some((at, serial)) = p.escape
                && now >= at
            {
                p.escape = None;
                tracing::info!("range: {id:#x} breaks free of the hand");
                self.freed.push(serial);
                continue;
            }
            if p.stun.is_some() || reeling.is_some() || p.escape.is_some() || now < p.next_struggle {
                continue;
            }
            let chance = (ESCAPE_FIRST + ESCAPE_STEP * p.struggles as f32 - if holds.len() > 1 { ESCAPE_TWO } else { 0.0 }).min(ESCAPE_MAX);
            let pick = holds[((next(&mut rng) * holds.len() as f32) as usize).min(holds.len() - 1)];
            p.struggled = now;
            p.next_struggle = now + ESCAPE_ANIM + ESCAPE_EVERY.0 + (ESCAPE_EVERY.1 - ESCAPE_EVERY.0) * next(&mut rng) as f64;
            if next(&mut rng) < chance {
                p.escape = Some((now + ESCAPE_DELAY, pick.serial));
                p.struggles = 0;
                tracing::info!("range: {id:#x} struggles ({:.0}%): it works", chance * 100.0);
            } else {
                p.struggles += 1;
                tracing::info!("range: {id:#x} struggles ({:.0}%)", chance * 100.0);
            }
        }
        self.rng = rng;
    }

    /// A hand on its throat (one is enough): after 2 s it lets go of the weapon in its left hand if
    /// it holds one in each, after 5 s of both; and it's hurt, 5 every second or so of the game's
    /// time, once however many hands are on it, with no grunt (its throat is held).
    fn choke(&mut self, k: usize, throat: bool, rng: &mut u32) {
        let now = self.time;
        let p = &mut self.people[k];
        if !throat || !p.alive {
            p.choked_since = None;
            return;
        }
        let since = match p.choked_since {
            Some(t) => t,
            None => {
                p.choked_since = Some(now);
                p.next_choke = now + CHOKE_EVERY + CHOKE_VARY * (2.0 * next(rng) as f64 - 1.0);
                now
            }
        };
        let id = p.id;
        let both = p.hands.iter().all(|h| h.is_some());
        let drops: &[usize] = if now - since > CHOKE_BOTH { &[0, 1] } else if now - since > CHOKE_ONE && both { &[0] } else { &[] };
        for &h in drops {
            if let Some(item) = self.drop_item(k, h) {
                tracing::info!("range: {id:#x}, choked, lets go of {item:#x}");
                self.happened.push(Happened::Disarmed { id, item });
            }
        }
        if now >= self.people[k].next_choke {
            self.people[k].next_choke = now + CHOKE_EVERY + CHOKE_VARY * (2.0 * next(rng) as f64 - 1.0);
            self.people[k].last_hit = (0, BodyPart::Neck);
            let at = self.part_pose(k, anim::HEAD).0;
            self.hurt_with(k, CHOKE_DAMAGE, at, false);
            tracing::info!("range: {id:#x} choked: {:.0} left", self.people[k].health.max(0.0));
        }
    }

    /// The held points pulled where the hands would have them and turned as they'd have them (each
    /// physics step): the grip on the part held, its damping against the hand's own motion, solved
    /// implicitly as the pins are.
    fn hold(&mut self) {
        for h in &self.held {
            let Some(p) = self.people.iter().find(|p| p.id == h.id) else { continue };
            let rb = &mut self.world.bodies[p.parts[h.part].body];
            let (pos, rot) = from_pose(rb.position());
            let at = pos + rot * h.local;
            let vel = from_v(rb.velocity_at_point(to_v(at)));
            let m = rb.mass().max(0.05);
            let f = implicit(m, GRAB_SPRING, GRAB_DAMPER, h.target - at, vel - h.hand_vel, STEP).clamp_length_max(h.force);
            rb.add_force_at_point(to_v(f), to_v(at), true);
            let i = from_v(rb.mass_properties().local_mprops.principal_inertia());
            let inertia = ((i.x + i.y + i.z) / 3.0).max(1e-4);
            let t = implicit(inertia, GRAB_TURN_SPRING, GRAB_TURN_DAMPER, shortest(h.target_rot * rot.inverse()), from_v(rb.angvel()) - h.hand_spin, STEP).clamp_length_max(GRAB_TURN_MAX);
            rb.add_torque(to_v(t), true);
        }
    }

    /// Its head held tight on its neck (a neck or torso hold), or loose again.
    fn neck_limits(&mut self, k: usize, tight: bool) {
        let p = &mut self.people[k];
        p.tight = tight;
        let Some(j) = p.parts[anim::HEAD].joint else { return };
        let rest = body()[anim::HEAD].limits;
        let (nod, turn) = if tight { (HELD_NOD, HELD_TURN) } else { (rest[0], rest[1]) };
        if let Some(joint) = self.world.impulse_joints.get_mut(j, true) {
            joint.data.set_limits(JointAxis::AngX, nod);
            joint.data.set_limits(JointAxis::AngY, turn);
        }
    }

    /// How a hand holds person `k`, standing, for its brain (`Grappled`): by which handle,
    /// on which limb, where the hand would have the point it holds and how high that point stands,
    /// and the one holding it at `grabber`.
    pub fn hold_on(&self, k: usize, grabber: V3) -> Option<Grappled> {
        let p = &self.people[k];
        let h = self.held.iter().find(|h| h.id == p.id)?;
        let limb = usize::from(![3, 4, 5, 9, 10].contains(&h.part));
        Some(Grappled { handle: h.handle, limb, grip: h.target, stand: p.parts[h.part].rest.y + h.local.y, grabber })
    }

    /// A shove: a blow on a body that isn't standing (down, or dead) pushes it for a moment: the
    /// blow's velocity times the weapon's push (`force`: an acceleration a second per m/s, or with
    /// `impulse` an impulse), for `seconds`, times `part_x` on the part hit and `other_x` on the rest
    /// of its body, at the point hit. As an acceleration it doesn't care for the body's mass: about
    /// 0.33 of the blow's speed for a punch (100 for 0.15 s, x2).
    #[allow(clippy::too_many_arguments)]
    pub fn shove(&mut self, k: usize, part: usize, point: V3, vel: V3, force: f32, seconds: f32, part_x: f32, other_x: f32, impulse: bool) {
        let p = &self.people[k];
        let standing = p.alive && matches!(p.balance, Balance::Standing | Balance::Rising(_));
        if force <= 0.0 || seconds <= 0.0 || standing {
            return;
        }
        let Some(q) = p.parts.get(part) else { return };
        let (pos, rot) = from_pose(self.world.bodies[q.body].position());
        let gain = if impulse { force * seconds } else { force * seconds * ADD_FORCE_STEP };
        tracing::info!("range: {:#x} down, shoved on part {part} at {:.1} m/s ({force} for {seconds} s: {:.2} of it{})", p.id, vel.length(), gain * part_x, if impulse { " a kg" } else { "" });
        self.shoves.push(Shove { id: p.id, part, local: rot.inverse() * (point - pos), push: vel * force, part_x, other_x, impulse, until: self.time + seconds as f64 });
    }

    /// The shoves under way, each step: every part of the body it's still joined to pushed at the
    /// point hit, by the blow times its share (a cut-off piece is its own).
    fn shoves(&mut self) {
        let now = self.time;
        self.shoves.retain(|s| s.until >= now);
        for s in &self.shoves {
            let Some(p) = self.people.iter().find(|p| p.id == s.id) else { continue };
            let (pos, rot) = from_pose(self.world.bodies[p.parts[s.part].body].position());
            let at = pos + rot * s.local;
            let off = p.severed & (1 << s.part) != 0;
            for (i, q) in p.parts.iter().enumerate() {
                let x = if i == s.part { s.part_x } else { s.other_x };
                if x <= 0.0 || (p.severed & (1 << i) != 0) != off {
                    continue;
                }
                let rb = &mut self.world.bodies[q.body];
                if s.impulse {
                    rb.apply_impulse_at_point(to_v(s.push * x * STEP), to_v(at), true);
                } else {
                    // As an acceleration at a point: its speed changed, and its spin by the lever
                    // to the point, neither weighed by its mass (a light hand isn't sent spinning).
                    let dv = s.push * x * STEP * ADD_FORCE_STEP;
                    let lever = at - from_v(rb.translation());
                    rb.set_linvel(rb.linvel() + to_v(dv), true);
                    rb.set_angvel(rb.angvel() + to_v(lever.cross(dv)), true);
                }
            }
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
                        let impulse = impulse_for(rb, point, dir, lacking).clamp_length_max(PUSH_MAX);
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
        let impulse = impulse_for(rb, point, dir, lacking).clamp_length_max(PUSH_MAX);
        rb.apply_impulse_at_point(to_v(impulse), to_v(point), true);
    }

    /// Damage to person `k` (its health; dead at nothing), with a grunt.
    pub fn hurt(&mut self, k: usize, damage: f32, at: V3) {
        self.hurt_with(k, damage, at, true);
    }

    /// Damage to person `k`, with a grunt or without. Hurt, its struggles start over.
    fn hurt_with(&mut self, k: usize, damage: f32, at: V3, voice: bool) {
        let p = &mut self.people[k];
        if !p.alive || damage <= 0.0 {
            return;
        }
        p.health -= damage;
        p.hurt_at = self.time;
        p.struggles = 0;
        let id = p.id;
        if p.health <= 0.0 {
            self.kill(k, at);
        } else {
            self.happened.push(Happened::Hurt { id, at, voice });
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

    /// Knocked out: its brain off and its eyes shut for 6 s, its weapons kept; it goes limp only if
    /// it was down already, or out cold already; knocked out again while out, it's out 6 s from then.
    pub fn knock_out(&mut self, k: usize, at: V3) {
        let now = self.time;
        let p = &mut self.people[k];
        if !p.alive {
            return;
        }
        let wake = now + rustvr_melee::body::KNOCKOUT_TIME as f64;
        let limp = p.stun.is_some() || matches!(p.balance, Balance::Down { .. } | Balance::Limp(_));
        let stun = match p.stun {
            Some(s) if s.wake >= wake => s,
            _ => Stun { wake, extended: false },
        };
        p.stun = Some(stun);
        if limp {
            p.balance = Balance::Limp(stun.wake);
        }
        p.stagger = None;
        let id = p.id;
        self.happened.push(Happened::KnockedOut { id, at });
    }

    /// A hit's push on person `k`'s part (by its push table): a stagger away from it, or knocked off
    /// its feet. Already down it takes no push; getting up, any push puts it down again; shocked, a
    /// blow (not the shock's own, `lightning`) pushes a level harder and any stagger floors it; out
    /// cold, it reels fully at least and its stun lasts on, once. Returns what it did.
    pub fn react(&mut self, k: usize, part: usize, level: u32, dir: V3, point: V3, lightning: bool) -> Reaction {
        let now = self.time;
        let p = &self.people[k];
        if !p.alive || !matches!(p.balance, Balance::Standing | Balance::Rising(_)) {
            return Reaction::None;
        }
        let id = p.id;
        if matches!(p.balance, Balance::Rising(_)) {
            tracing::info!("range: {id:#x} pushed getting up: down again");
            self.people[k].balance = Balance::Down { since: now, still: None };
            self.happened.push(Happened::Floored { id, at: point });
            return Reaction::Destabilize;
        }
        let shocked = p.shocked > now;
        if shocked && lightning {
            return Reaction::None;
        }
        let level = if shocked { level + 1 } else { level };
        let kind = p.parts.get(part).map_or(PART_TORSO, |q| q.kind);
        let body = BodyPart::of(kind, false);
        let doing = match (p.stagger, p.doing) {
            (Some(s), _) if now - s.since < 0.6 && s.recoil >= 1.0 => Doing::FullyStaggered,
            (Some(s), _) if now - s.since < 0.6 => Doing::Staggered,
            (_, d) => d,
        };
        let left = [anim::UPPER_ARM[0], anim::FOREARM[0], anim::HAND[0], anim::THIGH[0], anim::SHIN[0]].contains(&part);
        let mut r = rustvr_melee::body::reaction(level, body, left, doing, p.table);
        if let Some(s) = p.stun {
            r = r.max(Reaction::Full);
            if !s.extended {
                let wake = now + rustvr_melee::body::KNOCKOUT_TIME as f64;
                self.people[k].stun = Some(Stun { wake, extended: true });
            }
        }
        if shocked && r != Reaction::None {
            r = Reaction::Destabilize;
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
        r
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
        let _ = blade_dir;
        let v = from_v(rb.linvel()) * rustvr_melee::SLICE_KEEP + out * push;
        rb.set_linvel(to_v(v), true);
        let radius = q.radius;
        // What a cut-off hand held goes with it (still in its fist).
        self.happened.push(Happened::Severed { id, part, at: joint_at, out, radius });
        let at = joint_at;
        self.kill(k, at);
        true
    }

    /// A mark on person `k`'s part `part` where a blow landed (world), of damage kind `kind`, for what
    /// the part is (flesh, or the armour over it): as big and as strong as the blow was hard
    /// (`intensity`, 0..1: from the mark's smallest and weakest to its largest and strongest), or at
    /// the size and full strength of a blow that went in (`went_in`). A blow that did no damage
    /// (`damaging` false) leaves nothing on flesh and a scratch or a scuff on armour; one with a
    /// fire-filled blade burns.
    #[allow(clippy::too_many_arguments)]
    pub fn wound(&mut self, k: usize, part: usize, point: V3, kind: u32, intensity: f32, damaging: bool, went_in: bool, fire: bool) {
        let time = self.time;
        let p = &mut self.people[k];
        let Some(q) = p.parts.get(part) else { return };
        let armour = matches!(q.material, MAT_LEATHER | MAT_CHAINMAIL | MAT_PLATE);
        let i = intensity.clamp(0.0, 1.0);
        let lerp = |lo: f32, hi: f32| lo + (hi - lo) * i;
        // (size, strength) by its mask: a cut 0.1..0.18 (gone in: 0.18), a stab 0.06..0.12
        // (0.15), a bruise 0.1..0.18 at 0.1..0.5 on flesh, a dent 0.1..0.16 on mail and plate.
        let (look, size, strength) = match (kind, damaging, armour) {
            (DMG_PIERCE, true, _) => (Look::Stab, if went_in { 0.15 } else { lerp(0.06, 0.12) }, if went_in { 1.0 } else { i }),
            (DMG_BLUNT | DMG_PUNCH, true, false) => (Look::Bruise, lerp(0.1, 0.18), lerp(0.1, 0.5)),
            (DMG_BLUNT | DMG_PUNCH, true, true) if q.material == MAT_LEATHER => (Look::Bruise, lerp(0.06, 0.12), i),
            (DMG_BLUNT | DMG_PUNCH, true, true) => (Look::Scuff, lerp(0.1, 0.16), i),
            (_, true, _) => (Look::Cut, if went_in { 0.18 } else { lerp(0.1, 0.18) }, if went_in { 1.0 } else { i }),
            (_, false, false) => return,
            (DMG_BLUNT | DMG_PUNCH, false, true) => (Look::Scuff, lerp(0.1, 0.16), i),
            (DMG_PIERCE, false, true) => (Look::Scratch, lerp(0.06, 0.12), i),
            (_, false, true) => (Look::Scratch, lerp(0.1, 0.18), i),
        };
        let look = if fire && damaging { Look::Burn } else { look };
        let (pos, rot) = from_pose(self.world.bodies[q.body].position());
        let local = rot.inverse() * (point - pos);
        let axis_t = ((local - q.a).dot(q.b - q.a) / (q.b - q.a).length_squared().max(1e-6)).clamp(0.0, 1.0);
        let on = q.a.lerp(q.b, axis_t);
        let normal = (local - on).normalize_or(V3::Z);
        let at = on + normal * q.radius;
        p.wounds.push(Wound { part, at, normal, look, size, strength, since: time });
        // (At most 100, the oldest reused.)
        if p.wounds.len() > 100 {
            p.wounds.remove(0);
        }
    }

    /// Where a line from `from` (world) along `dir` comes out of person `k`'s part `part` again: a
    /// point run through it, out the far side.
    pub fn exit_point(&self, k: usize, part: usize, from: V3, dir: V3) -> Option<V3> {
        let (a, b, r) = self.capsule(k, part);
        let dir = dir.normalize_or_zero();
        let inside = |p: V3| {
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            p.distance(a + ab * t) < r
        };
        // Into it first (the blow met its surface), then on until it's out.
        let mut d = 0.0;
        let mut was_in = false;
        while d < 1.0 {
            let p = from + dir * d;
            if inside(p) {
                was_in = true;
            } else if was_in {
                return Some(p);
            }
            d += 0.005;
        }
        None
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
            let conscious = p.alive && !matches!(p.balance, Balance::Limp(_)) && p.stun.is_none();
            b.flags = BODY_GRABBABLE | if conscious { BODY_CONSCIOUS } else { 0 } | if down { BODY_RAGDOLL } else { 0 };
            b.mass = if p.handled { MASS_HANDLED } else if down { MASS_RAGDOLLED } else { MASS_STANDING };
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

    /// What the creatures hold, for the runtime: each item where it is now, how it moves, and
    /// whether its hand swings it (`attacking`: by person and hand), guards with it (`blocking`) or
    /// feints; nothing of that for what the player's hand has hold of too.
    pub fn held_items(&self, attacking: impl Fn(usize, usize) -> bool, blocking: impl Fn(usize, usize) -> bool, feinting: impl Fn(usize) -> bool) -> (ActorItems, Vec<(u32, V3, Q)>) {
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
                flags: if !item.players && attacking(who, hand) { AI_ATTACKING } else { 0 }
                    | if !item.players && blocking(who, hand) { AI_BLOCKING } else { 0 }
                    | if item.shield { AI_SHIELD } else { 0 }
                    | if !item.players && feinting(who) && !item.shield { AI_FEINT } else { 0 },
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

    /// What covers person `k`'s part `part` (`MAT_*`).
    pub fn part_material(&self, k: usize, part: usize) -> u32 {
        self.people[k].parts.get(part).map_or(MAT_FLESH, |q| q.material)
    }

    /// It reels (a stagger of `recoil`, away along `dir`), as a parry's recoil makes it, outside its
    /// push table.
    pub fn reel(&mut self, k: usize, recoil: f32, dir: V3) {
        let now = self.time;
        let p = &mut self.people[k];
        if !p.alive || p.balance != Balance::Standing {
            return;
        }
        let local = (p.at.1.inverse() * dir).with_y(0.0).normalize_or(-V3::Z);
        p.stagger = Some(Stagger { recoil, dir: local, since: now, head: false });
    }

    /// Every part of person `k` given `impulse` (N s, each part the whole of it).
    pub fn fling(&mut self, k: usize, impulse: V3) {
        for part in &self.people[k].parts {
            self.world.bodies[part.body].apply_impulse(to_v(impulse), true);
        }
    }

    /// A roll of the creatures' dice, 0..1.
    pub fn chance(&mut self) -> f32 {
        next(&mut self.rng)
    }

    pub fn find(&self, id: u32) -> Option<usize> {
        self.people.iter().position(|p| p.id == id)
    }

    /// A part's own colour: its armour's, or skin, or the dummies' canvas.
    fn part_tint(part: &Part) -> [f32; 3] {
        match part.material {
            MAT_LEATHER => [0.45, 0.3, 0.18],
            MAT_CHAINMAIL => [0.55, 0.57, 0.6],
            MAT_PLATE => [0.78, 0.8, 0.84],
            _ if part.kind == PART_HEAD || part.kind == PART_HAND => [0.86, 0.68, 0.55],
            _ => [0.77, 0.66, 0.38],
        }
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
                let t = Self::part_tint(part);
                let tint = [t[0], t[1], t[2], 1.0];
                // Paler as it bleeds out; grey dead; charring as it burns, and glowing hot.
                let k = if p.alive { 0.75 + 0.25 * (p.health / rustvr_melee::body::HEALTH).clamp(0.0, 1.0) } else { 0.6 };
                let char = p.burning.map_or(0.0, |t| ((self.time - t) / 10.0).clamp(0.0, 0.85) as f32);
                let hot = (p.heat / 100.0) * 0.4;
                let tint = [(tint[0] * k * (1.0 - char) + hot).min(1.0), tint[1] * k * (1.0 - char) + hot * 0.3, tint[2] * k * (1.0 - char), 1.0];
                out.push((Mat4::from_scale_rotation_translation(size, along, (a + b) * 0.5), tint));
                // Its eyes: open, screwed up in pain for a moment after a hit, shut knocked out or
                // dead, or mid-blink.
                if i == anim::HEAD {
                    let blinking = self.time - p.blink > BLINK * 0.25 && self.time - p.blink < BLINK * 0.75;
                    let shut = !p.alive || matches!(p.balance, Balance::Limp(_)) || p.stun.is_some() || blinking;
                    let pain = self.time - p.hurt_at < 0.75;
                    let h = if shut { 0.004 } else if pain { 0.008 } else { 0.022 };
                    for s in [-1.0f32, 1.0] {
                        let at = pos + rot * (part.b * 0.25 + V3::new(0.035 * s, 0.02, part.radius * 0.95));
                        out.push((Mat4::from_scale_rotation_translation(V3::new(0.025, h, 0.012), rot, at), [0.08, 0.06, 0.05, 1.0]));
                    }
                }
            }
            // Its marks (red its blood, the rest the wound's look), as strong as they were made over
            // what's there: dark red cuts and stabs, purple bruises, charred burns glowing as they
            // cool (their glow gone over 5 s), and on armour bright scratches and dull scuffs.
            for w in &p.wounds {
                let part = &p.parts[w.part];
                let (pos, rot) = from_pose(self.world.bodies[part.body].position());
                let at = pos + rot * (w.at + w.normal * 0.004);
                let turn = rot * Quat::from_rotation_arc(V3::Z, w.normal);
                let base = Self::part_tint(part);
                let glow = (1.0 - (self.time - w.since) / 5.0).clamp(0.0, 1.0) as f32;
                let (shape, colour) = match w.look {
                    Look::Cut => (V3::new(0.012, w.size, 0.004), [0.45, 0.03, 0.03]),
                    Look::Stab => (V3::new(w.size * 0.3, w.size * 0.3, 0.004), [0.3, 0.01, 0.01]),
                    Look::Bruise => (V3::new(w.size * 0.6, w.size * 0.6, 0.003), [0.35, 0.15, 0.35]),
                    Look::Burn => (V3::new(w.size * 0.4, w.size, 0.004), [0.08 + 0.9 * glow, 0.05 + 0.3 * glow, 0.03]),
                    Look::Scratch => (V3::new(0.006, w.size, 0.002), base.map(|c| (c + 0.25).min(1.0))),
                    Look::Scuff => (V3::new(w.size * 0.5, w.size * 0.5, 0.002), base.map(|c| c * 0.6)),
                };
                let s = w.strength.clamp(0.0, 1.0);
                let tint = [base[0] + (colour[0] - base[0]) * s, base[1] + (colour[1] - base[1]) * s, base[2] + (colour[2] - base[2]) * s, 1.0];
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

/// The handle a hold at `local` (the part's body's own space) on part `i` is on: the neck by the
/// throat, the mouth on the face's lower front, the skull above, the torso and hips, a shoulder on
/// an upper arm, a wrist on a forearm, a hand, a leg.
fn handle_of(i: usize, part: &Part, throat: bool, local: V3) -> Handle {
    match part.kind {
        _ if throat => Handle::Neck,
        PART_HEAD if local.z > part.radius * 0.5 && local.y < 0.03 => Handle::Mouth,
        PART_HEAD => Handle::Skull,
        PART_ARM if anim::UPPER_ARM.contains(&i) => Handle::Shoulder,
        PART_ARM => Handle::Wrist,
        PART_HAND => Handle::Hand,
        PART_LEG | PART_FOOT => Handle::Legs,
        _ => Handle::Torso,
    }
}

/// The creatures' own dice (xorshift): 0..1.
fn next(rng: &mut u32) -> f32 {
    let mut x = *rng;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *rng = x;
    (x as f32) / (u32::MAX as f32)
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

/// The impulse along `dir` (a unit vector) at `point` that changes `rb`'s speed there along it by
/// `dv`: the body's mass as it's felt at the point (turning as well as going), so a light blade
/// pushed at its tip isn't flung off faster than it was asked to go.
fn impulse_for(rb: &RigidBody, point: V3, dir: V3, dv: f32) -> V3 {
    let props = rb.mass_properties();
    let turn_axis = (point - from_v(props.world_com)).cross(dir);
    let turning = from_v(props.effective_world_inv_inertia * to_v(turn_axis));
    let felt = props.effective_inv_mass.x.max(0.0) + turn_axis.dot(turning);
    dir * dv / felt.max(1e-6)
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
