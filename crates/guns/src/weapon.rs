//! A gun as RustVR holds it: weapon space, its parts and how they move, where the hands go (the
//! game's own hands when it has them) and how it fires.

use glam::{Mat3, Mat4, Quat, Vec3};
use rustvr_asset as asset;
use rustvr_avatar::fit::{Surface, Trigger};
use rustvr_protocol::*;

use crate::mesh::MeshData;

/// A gun: its meshes, where the hands, sights and magazine go on it, and how it fires.
///
/// Weapon space is left-handed metres with the line of sight along +Z (which is the barrel give or
/// take the fraction of a degree a model's sights are tilted by), up +Y and right +X. `parts` are
/// in the model's own space and drawn with `fix` (model -> weapon space).
pub struct Weapon {
    pub name: String,
    /// Stable id: a game gun's asset key, or `builtin_rifle`, `builtin_rifle_scoped`,
    /// `builtin_pistol`.
    pub key: String,
    /// One-handed (holsters on the hip).
    pub pistol: bool,
    /// Shouldered (`WPN_STOCK`): a rifle; not a pistol or a heavy gun held by its handles at the
    /// hip.
    pub stock: bool,
    /// Slide or charging handle you pull to chamber: grab point (weapon space) and travel.
    pub handle: Option<(Vec3, f32)>,
    /// Slide locks back on the last round.
    pub slide_locks: bool,
    /// Model space; drawn with weapon_to_world * fix.
    pub parts: Vec<MeshData>,
    /// Model space -> weapon space (the line of sight onto +Z, the firing grip to the origin).
    pub fix: Mat4,
    pub hands: Vec<MeshData>,
    /// Weapon space (legacy mode hands this to the runtime).
    pub rig: WeaponRig,
    /// The magazine's meshes in `parts` (one per texture). Physical mode draws them as the
    /// magazine object; legacy mode hides them while the magazine is out.
    pub mag_parts: Vec<usize>,
    /// The bolt carrier, slide or charging handle in `parts`: drawn moved back along -Z by the
    /// runtime's `slide` times the handle travel.
    pub bolt_parts: Vec<usize>,
    /// The trigger in `parts`, drawn swung back about its pivot (weapon space, about +X) by the
    /// holding hand's trigger times the angle (radians) of a full pull: with the trigger finger.
    pub trigger_parts: Vec<usize>,
    /// Glass and lenses (the rig's `hidden_meshes`: a gauge's glass, a reflex sight's lens and
    /// reticle), model space like `parts`, riding on the gun's body (drawn with `fix`). Not among
    /// `parts`: the range doesn't draw them (it can't draw them see-through); the runtime draws them
    /// with the game's forward shading or a special draw.
    pub glass: Vec<MeshData>,
    pub trigger_pivot: Option<(Vec3, f32)>,
    /// Centre of the magazine in weapon space (where the hand holds a fresh one).
    pub mag_center: Vec3,
    pub mag_size: u32,
    pub rounds_per_second: f32,
    pub automatic: bool,
    /// Zoom while aiming down the sights (>1.4 opens a magnified scope).
    pub ads_zoom: f32,
    pub from_game: bool,
    /// The firing hand's controller grip rotation in weapon space, with the grip pose's origin (the
    /// palm centre) at `rig.grip`: the drawn hand then wraps the pistol grip with its index finger
    /// on the trigger. `Some` marks a properly rigged gun; `None` leaves the runtime aiming the
    /// barrel along the controller's ray.
    pub grip_rot: Option<Quat>,
    /// Two-handed guns: the support hand's controller grip rotation in weapon space while it holds
    /// the foregrip line.
    pub support_rot: Option<Quat>,
    /// Recoil per shot: muzzle climb (degrees), sideways scatter (degrees), push back (metres).
    pub recoil: [f32; 3],
    /// Recoil character (`ObjectDesc::recoil_climb`, `recoil_return`, `recoil_angle`): how much of
    /// each kick stays on full auto (0..1), how fast the gun settles (1/s), and how far from
    /// straight up a kick may lean (degrees). Zero: the runtime's default.
    pub recoil_feel: [f32; 3],
    /// The game's own recoil (its model space), when the extractor found it: the runtime plays
    /// these instead of its own (see `rustvr_asset::Recoil`).
    pub kicks: Vec<asset::Kick>,
    /// The game's own hands on it, [support (left), firing (right)]: the game's first-person arms
    /// as they hold this gun (or, for the built-in guns, a game gun's carried over). See [`GunHand`].
    pub game_hands: [Option<GunHand>; 2],
    /// Kilograms: the gun without its magazine, and a loaded magazine.
    pub mass: f32,
    pub mag_mass: f32,
    /// Lies on a table with its left side up (a magazine sticking out of its left side points up).
    pub left_side_up: bool,
    /// What the rig is checked against (weapon space): the rear sight (top of its notch's
    /// shoulders, or an optic's axis) and the front sight's tip, which the line of sight passes
    /// through...
    #[allow(dead_code)] // (read by the tests)
    pub sights: Option<(Vec3, Vec3)>,
    /// ...and where the trigger finger's pad rests on the trigger...
    #[allow(dead_code)]
    pub trigger: Option<Vec3>,
    /// ...and the pistol grip as a block: its centre (weapon space) and half size across (the back
    /// of the hand's way), front to back and along its axis (the firing hand's grip frame).
    #[allow(dead_code)]
    pub grip_block: Option<(Vec3, Vec3)>,
    /// ...and the handguard the support hand takes anywhere along: its axis, end to end (weapon
    /// space), and its radius. None: the runtime's default along the bore through `foregrip`.
    pub handguard: Option<(Vec3, Vec3, f32)>,
    /// ...and the trigger blade's surface (weapon-space triangles).
    #[allow(dead_code)]
    pub trigger_blade: Vec<[Vec3; 3]>,
    /// The game's first-person arms posed holding it, and the same arms at rest (the game's hands
    /// are taken from these; the built-in guns borrow a game gun's).
    pub game_arms: Option<std::sync::Arc<GameArms>>,
    /// What it fires (the game's ammo class, `5.45x39`), and the magazine it takes: guns with the
    /// same magazine take each other's (the runtime's `ammo_type`, see `physical::ammo_types`).
    pub caliber: String,
    pub magazine: String,
    /// How it's worked between shots and loaded, and the parts that move for it.
    pub action: WeaponAction,
    /// Where a laser fitted to it shines from (weapon space, along +Z): the game's mount for its
    /// laser attachment. Whether one is fitted is the game's to say.
    pub laser: Option<Vec3>,
}

/// How a gun that doesn't cycle itself is worked (the protocol's `ACTION_*`, `WPN_PNEUMATIC`,
/// `WPN_LOADS_CHAMBERS`), and which of its parts move for it. Points are weapon space.
#[derive(Clone, Debug, Default)]
pub struct WeaponAction {
    /// `ACTION_*`.
    pub kind: u32,
    /// Break and swing actions: a point on the hinge's pin, and how far it opens (radians, signed:
    /// about +X tipping the muzzle down, or about +Z swinging the top left); the parts that turn.
    pub hinge: Vec3,
    pub open: f32,
    pub barrel_parts: Vec<usize>,
    /// Loaded round by round: each chamber's mouth (its round's base), and its round's parts.
    pub chambers: Vec<Vec3>,
    pub chamber_parts: Vec<Vec<usize>>,
    /// A magazine that stays in it, loaded one round at a time: each of its rounds' parts (in the
    /// order they're loaded and fired), and the port they go in at (the first's base).
    pub round_parts: Vec<Vec<usize>>,
    pub port: Option<Vec3>,
    /// Bolt actions: the bolt's turn about the bore (the line through the muzzle along +Z) closed
    /// and open, radians from the model's pose.
    pub bolt_turn: Option<[f32; 2]>,
    /// A handle that's a lever (`bolt_parts` are it): its pivot, and its swing at full travel
    /// (radians about +X, positive its far end down and back).
    pub lever: Option<(Vec3, f32)>,
    /// Air guns: what a pump stroke adds, a shot spends, the least it fires with; and the gauge.
    pub air: Option<[f32; 3]>,
    pub gauge: Option<Gauge>,
}

/// An air gun's pressure gauge: its parts, turned about (or slid along) an axis through a point
/// between two settings, empty and full (radians or metres, from the model's pose).
#[derive(Clone, Debug, Default)]
pub struct Gauge {
    pub parts: Vec<usize>,
    pub at: Vec3,
    pub axis: Vec3,
    pub turn: Option<[f32; 2]>,
    pub slide: Option<[f32; 2]>,
}

impl WeaponAction {
    pub fn hinged(&self) -> bool {
        matches!(self.kind, ACTION_BREAK | ACTION_SWING)
    }

    /// The barrels' turn, open by `fraction` of the way: a point `p` on them goes to `m * p`.
    pub fn hinge_matrix(&self, fraction: f32) -> Mat4 {
        let axis = if self.kind == ACTION_SWING { Vec3::Z } else { Vec3::X };
        let q = Quat::from_axis_angle(axis, self.open * fraction.clamp(0.0, 1.0));
        Mat4::from_translation(self.hinge) * Mat4::from_quat(q) * Mat4::from_translation(-self.hinge)
    }
}

/// How a gun's moving parts sit, from what the runtime reports (`ObjectState`): the action's
/// travel (`slide`), what's in the chambers (`chambered`), a bolt's lift or an air gun's pressure
/// (`fuse`), and the holding hand's trigger.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartsState {
    pub slide: f32,
    pub chambered: u32,
    pub fuse: f32,
    pub trigger: f32,
    /// Rounds in a magazine that stays in the gun (`ObjectState::rounds`).
    pub rounds: u32,
}

impl PartsState {
    /// As the gun lies on a table: shut, loaded, the gauge where it is.
    #[allow(dead_code)] // (the rig renders use it)
    pub fn at_rest(w: &Weapon) -> PartsState {
        let loaded = (1u32 << w.action.chambers.len().max(1)) - 1;
        PartsState { chambered: loaded, rounds: w.action.round_parts.len().saturating_sub(1) as u32, ..Default::default() }
    }
}

impl Weapon {
    /// Where each of the gun's parts is drawn for `s` (weapon space; multiply by the gun's pose and
    /// `fix` is in it), or None for a part that isn't drawn now: the magazine (its own object
    /// draws it) and the rounds of empty chambers.
    pub fn part_matrices(&self, s: &PartsState) -> Vec<Option<Mat4>> {
        let fin = |x: f32| if x.is_finite() { x } else { 0.0 };
        let (slide, fuse, pull) = (fin(s.slide).clamp(0.0, 1.0), fin(s.fuse).clamp(0.0, 1.0), fin(s.trigger).clamp(0.0, 1.0));
        let a = &self.action;
        let travel = self.handle.map(|(_, t)| t).unwrap_or(0.0);
        // The bolt or handle: a lever swings, a bolt turns about the bore then slides, anything
        // else slides straight back.
        let bolt = if let Some((pivot, swing)) = a.lever {
            let angle = self.handle.map_or(swing * slide, |(h, t)| lever_angle(h, pivot, swing, slide * t));
            Mat4::from_translation(pivot) * Mat4::from_rotation_x(angle) * Mat4::from_translation(-pivot)
        } else {
            let back = Mat4::from_translation(Vec3::new(0.0, 0.0, -travel * slide));
            match a.bolt_turn {
                Some([closed, open]) => {
                    let m = Vec3::from_array(self.rig.muzzle);
                    let axis = Vec3::new(m.x, m.y, 0.0);
                    let turn = closed + (open - closed) * fuse;
                    back * Mat4::from_translation(axis) * Mat4::from_rotation_z(turn) * Mat4::from_translation(-axis)
                }
                None => back,
            }
        };
        let trigger = match self.trigger_pivot {
            Some((at, angle)) => Mat4::from_translation(at) * Mat4::from_rotation_x(angle * pull) * Mat4::from_translation(-at),
            None => Mat4::IDENTITY,
        };
        let barrels = if a.hinged() { a.hinge_matrix(slide) } else { Mat4::IDENTITY };
        let gauge = a.gauge.as_ref().map(|g| match (g.turn, g.slide) {
            (Some([lo, hi]), _) => Mat4::from_translation(g.at) * Mat4::from_quat(Quat::from_axis_angle(g.axis, lo + (hi - lo) * fuse)) * Mat4::from_translation(-g.at),
            (_, Some([lo, hi])) => Mat4::from_translation(g.axis * (lo + (hi - lo) * fuse)),
            _ => Mat4::IDENTITY,
        });
        (0..self.parts.len())
            .map(|i| {
                if self.mag_parts.contains(&i) {
                    return None;
                }
                if let Some(k) = a.chamber_parts.iter().position(|c| c.contains(&i)) {
                    let full = s.chambered & (1 << k | 1 << (k as u32 + CHAMBER_SPENT_SHIFT)) != 0;
                    return full.then(|| barrels * self.fix);
                }
                // A magazine that stays in it shows the rounds it holds (with the chambered one).
                if let Some(k) = a.round_parts.iter().position(|r| r.contains(&i)) {
                    let held = s.rounds + (s.chambered & CHAMBER_LIVE_MASK != 0) as u32;
                    return ((k as u32) < held).then_some(self.fix);
                }
                Some(match () {
                    _ if self.bolt_parts.contains(&i) => bolt,
                    _ if self.trigger_parts.contains(&i) => trigger,
                    _ if a.barrel_parts.contains(&i) => barrels,
                    _ if a.gauge.as_ref().is_some_and(|g| g.parts.contains(&i)) => gauge.unwrap_or(Mat4::IDENTITY),
                    _ => Mat4::IDENTITY,
                } * self.fix)
            })
            .collect()
    }

    /// The parts a magazine of this gun is drawn with: its magazine's, or for a gun loaded round by
    /// round, its first chamber's round.
    pub fn round_parts(&self) -> &[usize] {
        match self.action.chamber_parts.first().or(self.action.round_parts.first()) {
            Some(c) if self.mag_parts.is_empty() => c,
            _ => &self.mag_parts,
        }
    }

    /// Loaded one round at a time (into its chambers or its magazine): the pouch hands it rounds,
    /// and this is where a round's base sits in it (the first chamber's mouth, the port).
    pub fn round_base(&self) -> Option<Vec3> {
        self.action.chambers.first().copied().or(self.action.port)
    }
}

/// How far a lever about `pivot` (weapon space, about +X) is swung (radians, 0..`swing`) for its
/// handle at `handle` to have come `back` metres back.
pub fn lever_angle(handle: Vec3, pivot: Vec3, swing: f32, back: f32) -> f32 {
    // The handle goes back steadily as the lever swings (over its range).
    let sign = if swing < 0.0 { -1.0 } else { 1.0 };
    let z = |a: f32| (pivot + Quat::from_rotation_x(sign * a) * (handle - pivot)).z;
    let want = handle.z - back.max(0.0);
    let (mut lo, mut hi) = (0.0, swing.abs());
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if z(mid) > want { lo = mid } else { hi = mid }
    }
    sign * 0.5 * (lo + hi)
}

/// A game's first-person arms posed holding a gun (in the gun's model space), and at rest.
pub struct GameArms {
    pub posed: asset::Model,
    pub rest: Option<asset::Model>,
}

/// A hand in a game's own pose on a gun, in weapon space (the protocol's `HandPoseDesc`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunHand {
    /// The hand joint's canonical frame: the wrist, and how the hand is turned.
    pub wrist: (Vec3, Quat),
    /// Each finger segment's canonical rotation relative to the hand (thumb .. little, three each).
    pub fingers: [Quat; 15],
    /// Firing hands: the index finger's segments at a full trigger pull.
    pub index_pulled: Option<[Quat; 3]>,
    /// Wrist to the middle finger's base of the hand that took the pose, metres.
    pub size: f32,
    /// How the game's pose was fitted to the gun (see [`rustvr_avatar::fit`]): per finger, the
    /// degrees each joint was unbent (+) or bent (-), knuckle first.
    #[allow(dead_code)]
    pub fitted: [[f32; 3]; 5],
    /// Where each finger's last segment reaches in the hand's frame, and how far out along it
    /// (the fingertips; the trigger finger's contact with the trigger), for other hands to reach
    /// (`HandPoseDesc::reach`); and the trigger finger's at a full pull.
    pub reach: [Option<(Vec3, f32)>; 5],
    pub index_reach_pulled: Option<(Vec3, f32)>,
}

/// How far the trigger finger's contact with the trigger travels back at a full pull, metres.
pub const TRIGGER_TRAVEL: f32 = 0.008;

/// Without a trigger to fit to: the turns the trigger finger takes from resting to a full pull
/// (radians at its three joints, knuckle first).
pub const TRIGGER_PULL: [f32; 3] = [0.08, 0.16, 0.11];

impl GunHand {
    /// The pose of `side` (0 left, 1 right) in a posed first-person arms model `arms` that holds
    /// the gun in its own model space (`rest`: the same arms unposed), moved into weapon space by
    /// `fix`. A right hand's index finger gets its pulled pose.
    ///
    /// With `fit` (the gun's surface and, for the firing hand, its trigger's face, in the arms'
    /// model space), the pose is fitted for close viewing: fingertips the game sank into the gun
    /// come back out to its surface, and the index finger's pad goes onto the trigger.
    pub fn from_arms(arms: &asset::Model, rest: Option<&asset::Model>, side: usize, fix: Mat4, fit: Option<(&Surface, Option<Trigger>)>) -> Option<GunHand> {
        let mut g = rustvr_avatar::game_hand(&arms.bones, rest.map(|r| r.bones.as_slice()), side).map_err(|e| tracing::warn!("guns: {}: {e:#}", arms.source)).ok()?;
        let mut fitted = [[0.0; 3]; 5];
        let mut index_pulled = None;
        let mut contact = None;
        if let Some((surface, trigger)) = fit {
            // The firing hand keeps the game's wrist (it decides how the gun sits in the hand); a
            // support hand may move a little out of the handguard.
            let max_move = if side == 1 { 0.0 } else { SUPPORT_MAX_MOVE };
            let (f, report) = rustvr_avatar::fit::fit_hand(arms, &g, side, surface, trigger, FIT_SLACK, max_move);
            let mm = |d: f32| format!("{:.1}", d * 1000.0);
            tracing::debug!(
                "guns: hand {side} fitted: moved {:.1} mm (palm {} -> {} mm in); fingers {} -> {} mm in; joints opened {:?} deg; trigger pad {:?} -> {:?} mm off",
                report.moved.length() * 1000.0,
                mm(report.palm_before),
                mm(report.palm_after),
                report.depth_before.map(mm).join("/"),
                report.depth_after.map(mm).join("/"),
                report.opened.map(|j| j.map(|a| a.to_degrees().round() as i32)),
                report.pad_before.map(mm),
                report.pad_after.map(mm)
            );
            fitted = report.opened.map(|j| j.map(f32::to_degrees));
            index_pulled = report.index_pulled;
            contact = report.index_contact;
            g = f;
        }
        let (_, fix_rot, _) = fix.to_scale_rotation_translation();
        let wrist = (fix.transform_point3(g.wrist.0), (fix_rot * g.wrist.1).normalize());
        let index_pulled = if side == 1 { index_pulled.or_else(|| Some(pulled_index(&g.fingers, side))) } else { None };
        // The fingertips, and where the trigger finger touches the trigger (released and pulled).
        let tip = |f: usize| rustvr_avatar::fit::tip_length(arms, side, f);
        let along = |f: usize| match (f, contact, tip(f)) {
            (1, Some(c), Some(t)) if t > 1e-3 => (c / t).clamp(0.05, 1.0),
            _ => 1.0,
        };
        let reach = std::array::from_fn(|f| rustvr_avatar::finger_reach(arms, &g, side, f, along(f)).map(|p| (p, along(f))));
        let index_reach_pulled = index_pulled.and_then(|q| {
            let mut pulled = g;
            pulled.fingers[3..6].copy_from_slice(&q);
            rustvr_avatar::finger_reach(arms, &pulled, side, 1, along(1)).map(|p| (p, along(1)))
        });
        Some(GunHand { wrist, fingers: g.fingers, index_pulled, size: g.size, fitted, reach, index_reach_pulled })
    }
}

/// The index finger (segments 3..6 of `fingers`) flexed by [`TRIGGER_PULL`] at each joint, about
/// each segment's own bending axis (+Z), towards the palm.
fn pulled_index(fingers: &[Quat; 15], side: usize) -> [Quat; 3] {
    // The palm faces the hand's +X on the right hand and -X on the left: flexing turns +Y towards it.
    let sign = if side == 1 { -1.0 } else { 1.0 };
    let mut parent = Quat::IDENTITY;
    let mut parent_rest = Quat::IDENTITY;
    let mut out = [Quat::IDENTITY; 3];
    for k in 0..3 {
        let rest = fingers[3 + k];
        let local = parent_rest.inverse() * rest;
        let bent = (parent * local * Quat::from_rotation_z(sign * TRIGGER_PULL[k])).normalize();
        out[k] = bent;
        parent = bent;
        parent_rest = rest;
    }
    out
}

/// How far the skin may sink into a gun before a finger is opened (flesh gives a little), metres.
const FIT_SLACK: f32 = 0.0012;
/// How far a support hand may be moved out of the handguard it sank into, metres.
const SUPPORT_MAX_MOVE: f32 = 0.012;


/// Where a controller's grip pose is when the hand drawn on it is `hand` (the inverse of the IK's
/// grip -> hand mapping): (position, rotation) in the same space.
pub fn controller_on(hand: &GunHand, side: usize) -> (Vec3, Quat) {
    let o = rustvr_ik::BodyParams::default().hand_offsets.grip_to_hand[side];
    let rot = (hand.wrist.1 * o.rot.inverse()).normalize();
    (hand.wrist.0 - rot * o.pos, rot)
}

/// A model moved rigidly by `m`.
pub fn transformed(model: &asset::Model, m: Mat4) -> asset::Model {
    let mut out = model.clone();
    for b in &mut out.bones {
        b.model_matrix = (m * Mat4::from_cols_array(&b.model_matrix)).to_cols_array();
    }
    for mesh in &mut out.meshes {
        for v in &mut mesh.vertices {
            v.pos = m.transform_point3(Vec3::from_array(v.pos)).to_array();
            v.normal = m.transform_vector3(Vec3::from_array(v.normal)).normalize_or_zero().to_array();
        }
    }
    out
}

impl Weapon {
    /// Properly rigged: the hands have authored places on it (physical mode only lays these out).
    pub fn rigged(&self) -> bool {
        self.grip_rot.is_some()
    }
}

/// The controller grip rotation for a fist round a tube along `tube` (little finger to thumb: the
/// grip pose's +Z), with the grip pose's +X as near `x` as the tube allows. +X is the palm normal
/// of OpenXR's grip pose: out of the back of a right hand, out of the palm of a left one.
pub fn fist(tube: Vec3, x: Vec3) -> Quat {
    let z = tube.normalize();
    let x = (x - z * x.dot(z)).normalize();
    Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)).normalize()
}

/// The firing hand on a pistol grip whose axis runs along `tube` (bottom to top): the back of the
/// right hand to the gun's right, turned `twist` degrees about the grip (negative turns the
/// knuckles towards the gun's left, so the index finger crosses the trigger's face).
pub fn pistol_grip(tube: Vec3, twist_deg: f32) -> Quat {
    (Quat::from_axis_angle(tube.normalize(), twist_deg.to_radians()) * fist(tube, Vec3::X)).normalize()
}

/// A pistol grip's axis raked `deg` back from vertical (the bottom further back).
pub fn raked(deg: f32) -> Vec3 {
    let r = deg.to_radians();
    Vec3::new(0.0, r.cos(), r.sin())
}

/// Where the trigger finger's pad rests relative to the firing hand's grip pose origin, for the
/// avatar's hand (`rustvr_ik`'s default `HandOffsets` and its `PistolGrip` curls) turned
/// [`TRIGGER_TWIST`] about the grip (the pad on the trigger's face, the fist round the grip). The
/// built-in guns put their triggers here, so a game gun's hand carried over to them finds it.
pub const TRIGGER_FROM_PALM: Vec3 = Vec3::new(0.0, 0.020, 0.074);
/// The firing hand's turn about the grip (see [`pistol_grip`]): the index finger crosses the
/// trigger's face instead of running past it.
pub const TRIGGER_TWIST: f32 = -15.0;

/// The support hand cradling a handguard: the handguard in the palm, the fingers curled up its
/// right side, the thumb along its left; the fist's tube points up, forward and to the left, the
/// palm faces up and a little right.
pub fn cradle() -> Quat {
    fist(CRADLE_TUBE, CRADLE_PALM)
}

/// The cradle's fist (see [`cradle`]), for rigs that don't give their own.
pub const CRADLE_TUBE: Vec3 = Vec3::new(-0.705, 0.331, 0.627);
pub const CRADLE_PALM: Vec3 = Vec3::new(0.391, 0.919, -0.045);

/// A mesh's triangles after `m`.
pub fn triangles(mesh: &MeshData, m: Mat4) -> Vec<[Vec3; 3]> {
    mesh.indices.as_chunks::<3>().0.iter().map(|t| t.map(|i| m.transform_point3(Vec3::from_array(mesh.vertices[i as usize].pos)))).collect()
}

/// Bounds of some of `parts` in weapon space.
pub fn parts_bounds(parts: &[MeshData], which: &[usize], fix: Mat4) -> Option<(Vec3, Vec3)> {
    which.iter().filter_map(|&i| parts[i].bounds(fix)).reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
}
