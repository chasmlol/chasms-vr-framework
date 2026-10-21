//! What the range is made of: the room, the targets, and the guns (Metro 2033's own, from the
//! local asset cache, or simple built-in ones when that cache doesn't exist).

use glam::{Mat3, Mat4, Quat, Vec3};
use rustvr_asset as asset;
use rustvr_avatar::fit::{Surface, Trigger};
use rustvr_protocol::*;

/// One vertex of range geometry (left-handed metres).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

#[derive(Clone, Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Texture key in the asset cache (Metro models), or a built-in texture name.
    pub texture: Option<String>,
}

impl MeshData {
    /// An axis-aligned box, faces wound clockwise seen from outside (D3D front faces).
    pub fn add_box(&mut self, min: Vec3, max: Vec3, color: [u8; 4]) {
        let c = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, max.y, max.z),
            Vec3::new(min.x, max.y, max.z),
        ];
        let size = max - min;
        // (corners, normal, uv scale) per face; corners go bottom-left, top-left, top-right, bottom-right
        // as seen from outside, which is clockwise in a left-handed space.
        let faces: [([usize; 4], Vec3, (f32, f32)); 6] = [
            ([0, 3, 2, 1], -Vec3::Z, (size.x, size.y)),
            ([5, 6, 7, 4], Vec3::Z, (size.x, size.y)),
            ([4, 7, 3, 0], -Vec3::X, (size.z, size.y)),
            ([1, 2, 6, 5], Vec3::X, (size.z, size.y)),
            ([3, 7, 6, 2], Vec3::Y, (size.x, size.z)),
            ([4, 0, 1, 5], -Vec3::Y, (size.x, size.z)),
        ];
        for (idx, n, (su, sv)) in faces {
            let base = self.vertices.len() as u32;
            let uvs = [[0.0, sv], [0.0, 0.0], [su, 0.0], [su, sv]];
            for k in 0..4 {
                self.vertices.push(Vertex { pos: c[idx[k]].to_array(), normal: n.to_array(), uv: uvs[k], color });
            }
            self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    /// A box of half size `half` centred at `c`, turned by `rot`.
    pub fn add_obox(&mut self, c: Vec3, half: Vec3, rot: Quat, color: [u8; 4]) {
        let start = self.vertices.len();
        self.add_box(-half, half, color);
        for v in &mut self.vertices[start..] {
            v.pos = (c + rot * Vec3::from_array(v.pos)).to_array();
            v.normal = (rot * Vec3::from_array(v.normal)).to_array();
        }
    }

    /// Bounds of the vertices after `m`, if there are any.
    pub fn bounds(&self, m: Mat4) -> Option<(Vec3, Vec3)> {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for v in &self.vertices {
            let p = m.transform_point3(Vec3::from_array(v.pos));
            min = min.min(p);
            max = max.max(p);
        }
        (min.x <= max.x).then_some((min, max))
    }

    /// A rounded box (a squircle in cross-section, flat ends) centred at `c`, turned by `rot`: its
    /// section is `half.x` by `half.y`, its length runs along local +Z (`half.z` each way). Grips and
    /// handguards shaped like a real gun's.
    pub fn add_rounded_prism(&mut self, c: Vec3, half: Vec3, rot: Quat, color: [u8; 4]) {
        const N: usize = 24;
        let ring: Vec<(Vec3, Vec3)> = (0..N)
            .map(|i| {
                let t = i as f32 / N as f32 * std::f32::consts::TAU;
                let (s, co) = t.sin_cos();
                // Superellipse of power 4: |x|^4 + |y|^4 = 1.
                let f = |v: f32| v.signum() * v.abs().sqrt();
                let p = Vec3::new(half.x * f(co), half.y * f(s), 0.0);
                let n = Vec3::new(co.signum() * co.abs().powf(1.5) / half.x, s.signum() * s.abs().powf(1.5) / half.y, 0.0).normalize_or(Vec3::X);
                (p, n)
            })
            .collect();
        let at = |p: Vec3| c + rot * p;
        let tri = |m: &mut MeshData, v: [(Vec3, Vec3); 3]| {
            let base = m.vertices.len() as u32;
            let (a, b, cc) = (at(v[0].0), at(v[1].0), at(v[2].0));
            let n = rot * (v[0].1 + v[1].1 + v[2].1);
            // Front faces as `add_box` winds them: (b - a) x (c - a) points out.
            let order = if (b - a).cross(cc - a).dot(n) >= 0.0 { [0, 1, 2] } else { [0, 2, 1] };
            for k in order {
                m.vertices.push(Vertex { pos: at(v[k].0).to_array(), normal: (rot * v[k].1).normalize().to_array(), uv: [0.0, 0.0], color });
            }
            m.indices.extend_from_slice(&[base, base + 1, base + 2]);
        };
        let z = |p: Vec3, z: f32| p + Vec3::Z * z;
        for i in 0..N {
            let (p0, n0) = ring[i];
            let (p1, n1) = ring[(i + 1) % N];
            let (lo0, hi0, lo1, hi1) = (z(p0, -half.z), z(p0, half.z), z(p1, -half.z), z(p1, half.z));
            tri(self, [(lo0, n0), (hi0, n0), (hi1, n1)]);
            tri(self, [(lo0, n0), (hi1, n1), (lo1, n1)]);
            tri(self, [(z(Vec3::ZERO, half.z), Vec3::Z), (hi0, Vec3::Z), (hi1, Vec3::Z)]);
            tri(self, [(z(Vec3::ZERO, -half.z), -Vec3::Z), (lo1, -Vec3::Z), (lo0, -Vec3::Z)]);
        }
    }

    /// A rectangle facing -Z (towards a player looking +Z), centred at `c`.
    pub fn add_plate(&mut self, c: Vec3, half_w: f32, half_h: f32, color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        let n = -Vec3::Z;
        let corners = [(-half_w, -half_h, 0.0, 1.0), (-half_w, half_h, 0.0, 0.0), (half_w, half_h, 1.0, 0.0), (half_w, -half_h, 1.0, 1.0)];
        for (x, y, u, v) in corners {
            self.vertices.push(Vertex { pos: [c.x + x, c.y + y, c.z], normal: n.to_array(), uv: [u, v], color });
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

pub struct Target {
    pub center: Vec3,
    pub radius: f32,
}

/// Room bounds the player's feet stay inside.
pub const ROOM_MIN: Vec3 = Vec3::new(-9.5, 0.0, -4.5);
pub const ROOM_MAX: Vec3 = Vec3::new(9.5, 6.0, 24.5);

/// The armory tables (min, max): one at your right hand where you start (deep enough for the
/// rigged long guns side by side), one long one ahead (reach it with a step, or pull things off
/// it with the gravity gloves).
pub const TABLE_NEAR: (Vec3, Vec3) = (Vec3::new(0.40, 0.0, 0.30), Vec3::new(1.50, 0.85, 1.42));
pub const TABLE_LONG: (Vec3, Vec3) = (Vec3::new(-3.0, 0.0, 2.2), Vec3::new(3.0, 0.85, 2.9));
/// A small table at your left hand with things to throw.
pub const TABLE_SIDE: (Vec3, Vec3) = (Vec3::new(-0.95, 0.0, 0.25), Vec3::new(-0.45, 0.85, 0.75));
/// The armory bench behind you (turn round and step back): Metro's other rigged guns lie on it
/// side by side, barrels to the back wall, butts towards you.
pub const TABLE_ARMORY: (Vec3, Vec3) = (Vec3::new(-3.6, 0.0, -2.75), Vec3::new(3.6, 0.85, -0.95));

/// Solid furniture (min, max), drawn and sent to the runtime's physics as collision boxes.
pub fn furniture() -> Vec<(Vec3, Vec3, [u8; 4])> {
    let wood = [110, 85, 60, 255];
    let mut v = vec![(TABLE_NEAR.0, TABLE_NEAR.1, wood), (TABLE_LONG.0, TABLE_LONG.1, wood), (TABLE_SIDE.0, TABLE_SIDE.1, wood), (TABLE_ARMORY.0, TABLE_ARMORY.1, wood)];
    for x in [-3.0f32, 3.0] {
        v.push((Vec3::new(x - 0.05, 0.0, 3.0), Vec3::new(x + 0.05, 1.1, 22.0), [80, 90, 100, 255]));
    }
    // Crates for depth cues.
    for (x, z) in [(-6.0f32, 6.0f32), (-7.0, 14.0), (6.5, 9.0), (7.0, 18.0)] {
        v.push((Vec3::new(x - 0.6, 0.0, z - 0.6), Vec3::new(x + 0.6, 1.2, z + 0.6), [120, 100, 70, 255]));
    }
    v
}

/// The room's shell (min, max): floor, walls and ceiling, for collision.
pub fn shell() -> Vec<(Vec3, Vec3)> {
    let t = 0.2;
    vec![
        (Vec3::new(-10.0, -t, -5.0), Vec3::new(10.0, 0.0, 25.0)),
        (Vec3::new(-10.0 - t, 0.0, -5.0), Vec3::new(-10.0, 6.0, 25.0)),
        (Vec3::new(10.0, 0.0, -5.0), Vec3::new(10.0 + t, 6.0, 25.0)),
        (Vec3::new(-10.0, 0.0, 25.0), Vec3::new(10.0, 6.0, 25.0 + t)),
        (Vec3::new(-10.0, 0.0, -5.0 - t), Vec3::new(10.0, 6.0, -5.0)),
        (Vec3::new(-10.0, 6.0, -5.0), Vec3::new(10.0, 6.3, 25.0)),
    ]
}

/// The room: floor, walls, ceiling beams, tables. Built-in texture "grid" on walls and floor.
pub fn room() -> (MeshData, MeshData) {
    let mut grid = MeshData { texture: Some("grid".into()), ..Default::default() };
    let t = 0.2;
    grid.add_box(Vec3::new(-10.0, -t, -5.0), Vec3::new(10.0, 0.0, 25.0), [150, 150, 150, 255]); // floor
    grid.add_box(Vec3::new(-10.0 - t, 0.0, -5.0), Vec3::new(-10.0, 6.0, 25.0), [170, 165, 160, 255]);
    grid.add_box(Vec3::new(10.0, 0.0, -5.0), Vec3::new(10.0 + t, 6.0, 25.0), [170, 165, 160, 255]);
    grid.add_box(Vec3::new(-10.0, 0.0, 25.0), Vec3::new(10.0, 6.0, 25.0 + t), [160, 160, 170, 255]);
    grid.add_box(Vec3::new(-10.0, 0.0, -5.0 - t), Vec3::new(10.0, 6.0, -5.0), [160, 160, 170, 255]);
    let mut plain = MeshData::default();
    plain.add_box(Vec3::new(-10.0, 6.0, -5.0), Vec3::new(10.0, 6.3, 25.0), [60, 60, 66, 255]); // ceiling
    for z in [0.0f32, 6.0, 12.0, 18.0, 24.0] {
        plain.add_box(Vec3::new(-10.0, 5.6, z - 0.15), Vec3::new(10.0, 6.0, z + 0.15), [90, 80, 70, 255]);
    }
    for (min, max, color) in furniture() {
        plain.add_box(min, max, color);
    }
    (grid, plain)
}

/// Target plates: straight ahead at 10 m (eye height, the scenarios aim at it), plus more.
pub fn targets() -> Vec<Target> {
    vec![
        Target { center: Vec3::new(0.0, 1.6, 10.0), radius: 0.35 },
        Target { center: Vec3::new(-1.5, 1.4, 5.0), radius: 0.3 },
        Target { center: Vec3::new(1.5, 1.8, 15.0), radius: 0.45 },
        Target { center: Vec3::new(0.0, 1.5, 22.0), radius: 0.6 },
        Target { center: Vec3::new(-6.0, 1.6, 12.0), radius: 0.4 },
        Target { center: Vec3::new(6.0, 1.2, 8.0), radius: 0.4 },
    ]
}

pub fn target_mesh(t: &Target) -> MeshData {
    let mut m = MeshData { texture: Some("target".into()), ..Default::default() };
    m.add_plate(t.center, t.radius, t.radius, [255, 255, 255, 255]);
    m.add_box(t.center + Vec3::new(-0.03, -t.center.y, 0.02), t.center + Vec3::new(0.03, -t.radius, 0.06), [70, 70, 70, 255]);
    m
}

/// A gun: its meshes, where the hands, sights and magazine go on it, and how it fires.
///
/// Weapon space is left-handed metres with the line of sight along +Z (which is the barrel give or
/// take the fraction of a degree a model's sights are tilted by), up +Y and right +X. `parts` are
/// in the model's own space and drawn with `fix` (model -> weapon space).
pub struct Weapon {
    pub name: String,
    /// Stable id: a Metro gun's asset key, or `builtin_rifle`, `builtin_rifle_scoped`,
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
    /// The game's own hands on it, [support (left), firing (right)]: Metro's first-person arms as
    /// they hold this gun (or, for the built-in guns, the Kalash's carried over). See [`GunHand`].
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
    /// ...and the trigger blade's surface (weapon-space triangles).
    #[allow(dead_code)]
    pub trigger_blade: Vec<[Vec3; 3]>,
    /// Metro's first-person arms posed holding it, and the same arms at rest (the game's hands
    /// are taken from these; the built-in guns borrow the Kalash's).
    pub game_arms: Option<std::sync::Arc<GameArms>>,
    /// What it fires (the game's ammo class, `5.45x39`), and the magazine it takes: guns with the
    /// same magazine take each other's (the runtime's `ammo_type`, see `physical::ammo_types`).
    pub caliber: String,
    pub magazine: String,
    /// How it's worked between shots and loaded, and the parts that move for it.
    pub action: WeaponAction,
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
}

impl PartsState {
    /// As the gun lies on a table: shut, loaded, the gauge where it is.
    #[allow(dead_code)] // (the rig renders use it)
    pub fn at_rest(w: &Weapon) -> PartsState {
        let loaded = (1u32 << w.action.chambers.len().max(1)) - 1;
        PartsState { chambered: loaded, ..Default::default() }
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
        match self.action.chamber_parts.first() {
            Some(c) if self.mag_parts.is_empty() => c,
            _ => &self.mag_parts,
        }
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
/// (Metro's own fire motions leave the Kalash's trigger finger still; the Bastard's flexes it by
/// about 30°, 40° and 65° at the shot, with the whole grip squeezing.)
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
        let mut g = rustvr_avatar::game_hand(&arms.bones, rest.map(|r| r.bones.as_slice()), side).map_err(|e| tracing::warn!("range: {}: {e:#}", arms.source)).ok()?;
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
                "range: hand {side} fitted: moved {:.1} mm (palm {} -> {} mm in); fingers {} -> {} mm in; joints opened {:?} deg; trigger pad {:?} -> {:?} mm off",
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

/// The first-person arms the extractor poses for every gun (`metro-extract` bakes each gun's arms
/// idle into a copy of these): the rest pose the finger frames are carried from.
const ARMS_REST: &str = "hands_basic_suit";

/// A grip or foregrip frame: a point and the controller grip rotation there.
fn grip_frame(at: [f32; 3], rot: Option<Quat>) -> Option<(Vec3, Quat)> {
    Some((Vec3::from_array(at), rot?))
}

/// Gives the built-in guns the Kalash's own hands (when Metro's are there): Metro's arms as they
/// hold the Kalash, carried over from the Kalash's grip frame (`grip`, `grip_rot`) to each gun's
/// for the firing hand and from foregrip frame to foregrip frame for the support hand, then fitted
/// to that gun (its grip and handguard are made like the Kalash's, so little changes).
pub fn share_game_hands(weapons: &mut [Weapon]) {
    let Some(ak) = weapons.iter().find(|w| w.key == "wpn_ak" && w.game_arms.is_some()) else { return };
    let (Some(arms), Some(ak_grip), Some(ak_fore)) = (ak.game_arms.clone(), grip_frame(ak.rig.grip, ak.grip_rot), grip_frame(ak.rig.foregrip, ak.support_rot)) else { return };
    let ak_fix = ak.fix;
    for w in weapons.iter_mut().filter(|w| !w.from_game && w.rigged()) {
        for side in [1, 0] {
            let (from, to) = match side {
                1 => (ak_grip, grip_frame(w.rig.grip, w.grip_rot)),
                _ if !w.pistol => (ak_fore, grip_frame(w.rig.foregrip, w.support_rot)),
                _ => continue,
            };
            let Some(to) = to else { continue };
            let r = (to.1 * from.1.inverse()).normalize();
            let carry = Mat4::from_rotation_translation(r, to.0 - r * from.0) * ak_fix;
            let posed = transformed(&arms.posed, carry);
            w.game_hands[side] = GunHand::from_arms(&posed, arms.rest.as_ref(), side, Mat4::IDENTITY, None);
        }
    }
}

/// Where a controller's grip pose is when the hand drawn on it is `hand` (the inverse of the IK's
/// grip -> hand mapping): (position, rotation) in the same space.
pub fn controller_on(hand: &GunHand, side: usize) -> (Vec3, Quat) {
    let o = rustvr_ik::BodyParams::default().hand_offsets.grip_to_hand[side];
    let rot = (hand.wrist.1 * o.rot.inverse()).normalize();
    (hand.wrist.0 - rot * o.pos, rot)
}

/// A model moved rigidly by `m`.
fn transformed(model: &asset::Model, m: Mat4) -> asset::Model {
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
/// [`TRIGGER_TWIST`] about the grip: found on Metro's AK in `tests/rig.rs` (the pad on the trigger's
/// face, the fist round the grip; the Kalash's own trigger sits there too). The built-in guns put
/// their triggers here, so the Kalash's hand carried over to them finds it.
pub const TRIGGER_FROM_PALM: Vec3 = Vec3::new(0.0, 0.020, 0.074);
/// The firing hand's turn about the grip (see [`pistol_grip`]): the index finger crosses the
/// trigger's face instead of running past it.
pub const TRIGGER_TWIST: f32 = -15.0;

/// The support hand cradling a handguard (from Metro's AK arms, `hands_ak`'s bind pose): the
/// handguard in the palm, the fingers curled up its right side, the thumb along its left; the
/// fist's tube points up, forward and to the left, the palm faces up and a little right.
pub fn cradle() -> Quat {
    fist(CRADLE_TUBE, CRADLE_PALM)
}

/// The Kalash's own rig (from [`metro_rigs`]) in its weapon space: its line-of-sight rotation, and
/// its grip and foregrip frames (point and controller grip rotation, as `rigged_metro_weapon`
/// builds them).
fn kalash() -> (&'static MetroRig, Quat, (Vec3, Quat), (Vec3, Quat)) {
    let r = metro_rigs().iter().find(|r| r.key == "wpn_ak").expect("content/rigs/metro2033.toml rigs the Kalash (wpn_ak)");
    let rot = Quat::from_rotation_arc((r.front_sight - r.rear_sight).normalize(), Vec3::Z);
    let fix = Mat4::from_quat(rot) * Mat4::from_translation(-r.grip);
    let grip = (fix.transform_point3(r.grip), (rot * pistol_grip(raked(r.grip_rake_deg), r.grip_twist_deg)).normalize());
    let fore = (fix.transform_point3(r.foregrip.unwrap_or(r.grip)), (rot * fist(r.support_tube, r.support_palm)).normalize());
    (r, rot, grip, fore)
}

/// Carries something placed relative to frame `from` over to frame `to` (weapon space).
fn carry(from: (Vec3, Quat), to: (Vec3, Quat), p: Vec3, q: Quat) -> (Vec3, Quat) {
    let r = (to.1 * from.1.inverse()).normalize();
    (to.0 + r * (p - from.0), (r * q).normalize())
}

/// The Kalash's pistol grip (a rounded block of its measured section, raked as the fist round it)
/// on a gun whose firing hand's grip frame is (`grip`, `grip_rot`): (centre, half size across, front
/// to back and along, rotation). The built-in guns' grips are made like it, so the Kalash's own hand
/// fits them.
fn kalash_grip(grip: Vec3, grip_rot: Quat) -> (Vec3, Vec3, Quat) {
    let (r, rot, from, _) = kalash();
    let fix = Mat4::from_quat(rot) * Mat4::from_translation(-r.grip);
    let block = fist(from.1 * Vec3::Z, Vec3::X);
    let (c, q) = carry(from, (grip, grip_rot), fix.transform_point3(r.grip_block.0), block);
    (c, r.grip_block.1, q)
}

/// Built-in rifle: barrel along +Z, a pistol grip raked 20°, a charging handle on the right and
/// iron sights (rear notch and front post) the line of sight runs through.
pub fn builtin_rifle(name: &str, scoped: bool) -> Weapon {
    let mut m = MeshData::default();
    let metal = [55, 58, 62, 255];
    let wood = [120, 78, 45, 255];
    m.add_box(Vec3::new(-0.025, 0.0, -0.06), Vec3::new(0.025, 0.075, 0.26), metal); // receiver
    m.add_box(Vec3::new(-0.011, 0.025, 0.26), Vec3::new(0.011, 0.047, 0.66), metal); // barrel
    // The handguard: the Kalash's lower handguard's section (3.2 cm wide, its bottom 2.8 cm under
    // the bore), so the Kalash's support hand cradles it as it does its own.
    m.add_rounded_prism(Vec3::new(0.0, 0.034, 0.36), Vec3::new(0.016, 0.026, 0.10), Quat::IDENTITY, wood);
    m.add_box(Vec3::new(-0.02, -0.02, -0.30), Vec3::new(0.02, 0.06, -0.06), wood); // stock
    // Pistol grip under the receiver, the trigger 3.5 cm ahead of its front edge.
    let rake = 20f32;
    let tube = raked(rake);
    let grip_rot = pistol_grip(tube, TRIGGER_TWIST);
    // The palm centre on the grip's axis, just over 4 cm below the receiver.
    let grip = Vec3::new(0.0, -0.042, -0.0073);
    // The grip itself is the Kalash's (its section and rake round the fist), run up into the
    // receiver.
    let (grip_centre, grip_half, grip_q) = kalash_grip(grip, grip_rot);
    m.add_rounded_prism(grip_centre + grip_q * Vec3::Z * 0.01, grip_half + Vec3::Z * 0.01, grip_q, wood);
    let trigger = grip + TRIGGER_FROM_PALM;
    m.add_box(Vec3::new(-0.004, -0.044, 0.012), Vec3::new(0.004, -0.038, 0.104), metal); // trigger guard
    m.add_box(Vec3::new(-0.004, -0.044, 0.098), Vec3::new(0.004, 0.0, 0.104), metal);
    let mut blade = MeshData::default();
    blade.add_obox(trigger + Vec3::new(0.0, 0.004, -0.003), Vec3::new(0.003, 0.017, 0.003), Quat::from_rotation_x(-12f32.to_radians()), metal); // trigger
    // It pivots at its top, in the receiver.
    let trigger_pivot = trigger + Vec3::new(0.0, 0.021, -0.004);
    if !scoped {
        // Rear notch (shoulders up to the line of sight at y 0.10) and front post (tip on it).
        m.add_box(Vec3::new(-0.012, 0.075, 0.0), Vec3::new(-0.0025, 0.10, 0.012), metal);
        m.add_box(Vec3::new(0.0025, 0.075, 0.0), Vec3::new(0.012, 0.10, 0.012), metal);
        m.add_box(Vec3::new(-0.0025, 0.075, 0.0), Vec3::new(0.0025, 0.096, 0.012), metal);
        m.add_box(Vec3::new(-0.003, 0.047, 0.552), Vec3::new(0.003, 0.10, 0.566), metal);
    } else {
        m.add_box(Vec3::new(-0.003, 0.047, 0.552), Vec3::new(0.003, 0.085, 0.566), metal); // low post
        m.add_box(Vec3::new(-0.02, 0.09, 0.02), Vec3::new(0.02, 0.13, 0.26), [30, 30, 32, 255]); // scope tube
        m.add_box(Vec3::new(-0.012, 0.075, 0.08), Vec3::new(0.012, 0.09, 0.2), metal); // mount
    }
    let mut mag = MeshData::default();
    mag.add_box(Vec3::new(-0.016, -0.17, 0.11), Vec3::new(0.016, 0.0, 0.18), [80, 70, 50, 255]);
    let mut handle = MeshData::default();
    handle.add_box(Vec3::new(0.025, 0.045, 0.15), Vec3::new(0.05, 0.06, 0.17), metal);
    let handle_at = Vec3::new(0.042, 0.052, 0.16);
    let (sight, front) = if scoped { (Vec3::new(0.0, 0.11, 0.03), Vec3::new(0.0, 0.11, 0.25)) } else { (Vec3::new(0.0, 0.10, 0.006), Vec3::new(0.0, 0.10, 0.559)) };
    let rig = WeaponRig {
        id: if scoped { 2 } else { 1 },
        flags: RIG_HAS_SIGHT | RIG_HAS_MUZZLE | RIG_HAS_MAG_WELL | RIG_TWO_HANDED | RIG_HAS_CHARGING_HANDLE | RIG_SIGHT_LINE,
        grip: grip.to_array(),
        charging_handle: handle_at.to_array(),
        // The support palm under the handguard where the Kalash's is under its own: 3.6 cm under the
        // bore and a little left of it.
        foregrip: [-0.002, 0.0003, 0.36],
        sight: sight.to_array(),
        muzzle: [0.0, 0.036, 0.66],
        mag_well: [0.0, 0.0, 0.145],
        sight_line_origin: sight.to_array(),
        sight_line_dir: [0.0, 0.0, 1.0],
        ..Default::default()
    };
    Weapon {
        name: name.into(),
        key: if scoped { "builtin_rifle_scoped" } else { "builtin_rifle" }.into(),
        pistol: false,
        stock: true,
        handle: Some((handle_at, 0.08)),
        slide_locks: false,
        trigger_blade: triangles(&blade, Mat4::IDENTITY),
        trigger_parts: vec![3],
        trigger_pivot: Some((trigger_pivot, TRIGGER_TRAVEL / (trigger - trigger_pivot).length())),
        parts: vec![m, mag, handle, blade],
        fix: Mat4::IDENTITY,
        hands: Vec::new(),
        rig,
        mag_parts: vec![1],
        bolt_parts: vec![2],
        mag_center: Vec3::new(0.0, -0.085, 0.145),
        mag_size: 30,
        rounds_per_second: 10.0,
        automatic: true,
        ads_zoom: if scoped { 4.0 } else { 1.1 },
        from_game: false,
        grip_rot: Some(grip_rot),
        // The Kalash's own support hold (its hand is carried over, see `share_game_hands`).
        support_rot: Some(cradle()),
        // The handguard runs 0.26..0.46: the palm stays a hand's half-width inside it.
        recoil: [6.5, 1.6, 0.022],
        recoil_feel: [0.6, 9.0, 12.0],
        kicks: Vec::new(),
        game_hands: [None; 2],
        mass: 3.5,
        mag_mass: 0.45,
        left_side_up: false,
        sights: Some((sight, front)),
        trigger: Some(trigger),
        grip_block: Some((grip_centre, grip_half)),
        game_arms: None,
        caliber: "5.56x45".into(),
        // Each built-in gun has a magazine of its own.
        magazine: if scoped { "builtin_rifle_scoped" } else { "builtin_rifle" }.into(),
        action: WeaponAction::default(),
    }
}

/// Built-in pistol: barrel along +Z, a grip raked 18° with the magazine inside it, a slide (with
/// the sights) to rack.
pub fn builtin_pistol() -> Weapon {
    let metal = [45, 47, 52, 255];
    let polymer = [30, 30, 32, 255];
    let rake = 18f32;
    let tube = raked(rake);
    let grip_rot = pistol_grip(tube, TRIGGER_TWIST);
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.013, -0.005, -0.04), Vec3::new(0.013, 0.012, 0.12), polymer); // frame
    // The palm centre on the grip's axis, under the frame as the Kalash's is under its receiver
    // (its trigger finger then runs along the frame).
    let grip = Vec3::new(0.0, -0.040, -0.0275);
    // The grip is the Kalash's (its section and rake round the fist), run up to the frame.
    let (kalash_centre, grip_half, grip_q) = kalash_grip(grip, grip_rot);
    let axis = grip_q * Vec3::Z;
    let up = 0.0145;
    let grip_centre = kalash_centre + axis * up;
    let grip_half = grip_half + Vec3::Z * up;
    m.add_rounded_prism(grip_centre, grip_half, grip_q, polymer);
    let trigger = grip + TRIGGER_FROM_PALM;
    m.add_box(Vec3::new(-0.003, -0.041, -0.004), Vec3::new(0.003, -0.035, 0.08), polymer); // trigger guard
    m.add_box(Vec3::new(-0.003, -0.041, 0.074), Vec3::new(0.003, 0.0, 0.08), polymer);
    let mut blade = MeshData::default();
    blade.add_obox(trigger + Vec3::new(0.0, 0.0055, -0.003), Vec3::new(0.0025, 0.0125, 0.0025), Quat::from_rotation_x(-10f32.to_radians()), metal); // trigger
    let trigger_pivot = trigger + Vec3::new(0.0, 0.017, -0.004);
    let mut slide = MeshData::default();
    slide.add_box(Vec3::new(-0.014, 0.012, -0.045), Vec3::new(0.014, 0.045, 0.14), metal);
    slide.add_box(Vec3::new(-0.0025, 0.045, 0.125), Vec3::new(0.0025, 0.053, 0.133), metal); // front sight
    slide.add_box(Vec3::new(-0.011, 0.045, -0.042), Vec3::new(-0.002, 0.053, -0.034), metal); // rear notch
    slide.add_box(Vec3::new(0.002, 0.045, -0.042), Vec3::new(0.011, 0.053, -0.034), metal);
    slide.add_box(Vec3::new(-0.002, 0.045, -0.042), Vec3::new(0.002, 0.049, -0.034), metal);
    // The magazine inside the grip (a slim single stack), its base plate under it.
    let mut mag = MeshData::default();
    let mag_half = Vec3::new(grip_half.x - 0.0025, grip_half.y - 0.004, grip_half.z - 0.002);
    let mag_centre = grip_centre - axis * 0.004;
    mag.add_obox(mag_centre, mag_half, grip_q, [70, 70, 75, 255]);
    mag.add_obox(mag_centre - axis * (mag_half.z + 0.002), Vec3::new(grip_half.x + 0.001, grip_half.y + 0.001, 0.003), grip_q, [70, 70, 75, 255]); // base plate
    let mag_top = mag_centre + axis * mag_half.z;
    let (sight, front) = (Vec3::new(0.0, 0.053, -0.038), Vec3::new(0.0, 0.053, 0.129));
    let handle_at = Vec3::new(0.0, 0.03, -0.035);
    let rig = WeaponRig {
        id: 3,
        flags: RIG_HAS_SIGHT | RIG_HAS_MUZZLE | RIG_HAS_MAG_WELL | RIG_HAS_CHARGING_HANDLE | RIG_SIGHT_LINE,
        grip: grip.to_array(),
        charging_handle: handle_at.to_array(),
        foregrip: grip.to_array(),
        sight: sight.to_array(),
        muzzle: [0.0, 0.03, 0.14],
        mag_well: mag_top.to_array(),
        sight_line_origin: sight.to_array(),
        sight_line_dir: [0.0, 0.0, 1.0],
        ..Default::default()
    };
    Weapon {
        name: "Range pistol".into(),
        key: "builtin_pistol".into(),
        pistol: true,
        stock: false,
        handle: Some((handle_at, 0.045)),
        slide_locks: true,
        trigger_blade: triangles(&blade, Mat4::IDENTITY),
        trigger_parts: vec![3],
        trigger_pivot: Some((trigger_pivot, TRIGGER_TRAVEL / (trigger - trigger_pivot).length())),
        parts: vec![m, mag, slide, blade],
        fix: Mat4::IDENTITY,
        hands: Vec::new(),
        rig,
        mag_parts: vec![1],
        bolt_parts: vec![2],
        mag_center: mag_centre,
        mag_size: 15,
        rounds_per_second: 12.0,
        automatic: false,
        ads_zoom: 1.0,
        from_game: false,
        grip_rot: Some(grip_rot),
        support_rot: None,
        recoil: [12.0, 3.0, 0.03],
        recoil_feel: [0.4, 14.0, 8.0],
        kicks: Vec::new(),
        game_hands: [None; 2],
        mass: 0.9,
        mag_mass: 0.25,
        left_side_up: false,
        sights: Some((sight, front)),
        trigger: Some(trigger),
        grip_block: Some((grip_centre, grip_half)),
        game_arms: None,
        caliber: "9x19".into(),
        magazine: "builtin_pistol".into(),
        action: WeaponAction::default(),
    }
}

/// A Metro 2033 gun rigged by hand: its entry in `content/rigs/metro2033.toml` (see
/// `rustvr_asset::rigs` and docs/RIGGING.md), in vectors. Every point is in the model's own space
/// (Metro's viewmodel space, as `cargo run -p rustvr-asset --example rig_probe` prints it); the
/// loader turns them into weapon space with the gun's `fix`.
struct MetroRig {
    key: String,
    name: Option<String>,
    /// One-handed (see `rigs::Hold`).
    pistol: bool,
    /// Shouldered (a rifle's hold, not a heavy gun's handles).
    stock: bool,
    caliber: String,
    /// The magazine it takes: guns naming the same one take each other's.
    magazine: String,
    /// Top of the rear sight notch's shoulders, and the front post's tip: weapon +Z is the line
    /// through them.
    rear_sight: Vec3,
    front_sight: Vec3,
    /// The firing hand's grip pose origin (palm centre) on the pistol grip's axis, the grip's rake
    /// and an extra turn of the hand about it.
    grip: Vec3,
    grip_rake_deg: f32,
    grip_twist_deg: f32,
    /// The trigger finger's pad on the trigger's face.
    trigger: Vec3,
    /// The pistol grip's centre, and its half size across, front to back (square to its axis) and
    /// along it.
    grip_block: (Vec3, Vec3),
    /// The support hand's grip pose origin (its palm centre, cradling the handguard) and its fist's
    /// tube and palm normal (see [`fist`]); used only without Metro's arms.
    foregrip: Option<Vec3>,
    support_tube: Vec3,
    support_palm: Vec3,
    /// Where the magazine's top sits when it's in.
    mag_well: Vec3,
    muzzle: Vec3,
    /// Charging handle (bolt, slide, pump, lever) knob, and how far it travels back.
    handle: Option<(Vec3, f32)>,
    /// The trigger's bone (its blade's skin).
    trigger_bone: String,
    /// Bones (with their children) whose skin is the magazine, and the bolt or charging handle.
    mag_bones: Vec<String>,
    bolt_bones: Vec<String>,
    /// Meshes (by index in the model) that are the magazine too.
    mag_meshes: Vec<usize>,
    /// Bones whose skin isn't drawn.
    hidden_bones: Vec<String>,
    hidden_meshes: Vec<usize>,
    recoil: [f32; 3],
    /// Recoil character: climb kept on full auto, return (1/s), lean (degrees).
    recoil_feel: [f32; 3],
    rounds_per_second: f32,
    automatic: bool,
    slide_locks: bool,
    mag_size: Option<u32>,
    mass: f32,
    mag_mass: f32,
    left_side_up: bool,
    ads_zoom: f32,
    /// How it's worked (see `rigs::Action`), and the parts that move for it (model space).
    action: asset::rigs::Action,
    hinge: Option<Vec3>,
    open_deg: f32,
    barrel_bones: Vec<String>,
    chamber_bones: Vec<String>,
    bolt_turn: Option<[f32; 2]>,
    handle_swing: Option<asset::rigs::HandleSwing>,
    pressure: Option<asset::rigs::Pressure>,
}

/// The cradle's fist (see [`cradle`]), for rigs that don't give their own.
const CRADLE_TUBE: Vec3 = Vec3::new(-0.705, 0.331, 0.627);
const CRADLE_PALM: Vec3 = Vec3::new(0.391, 0.919, -0.045);

impl MetroRig {
    fn new(g: &asset::rigs::GunRig) -> MetroRig {
        let v = Vec3::from_array;
        MetroRig {
            key: g.key.clone(),
            name: g.name.clone(),
            pistol: g.hold == asset::rigs::Hold::Pistol,
            stock: g.hold == asset::rigs::Hold::Rifle,
            caliber: g.caliber.clone(),
            magazine: g.magazine.clone().unwrap_or_else(|| g.key.clone()),
            rear_sight: v(g.rear_sight),
            front_sight: v(g.front_sight),
            grip: v(g.grip),
            grip_rake_deg: g.grip_rake_deg,
            grip_twist_deg: g.grip_twist_deg,
            trigger: v(g.trigger),
            grip_block: (v(g.grip_block.centre), v(g.grip_block.half)),
            foregrip: g.foregrip.map(v),
            support_tube: g.support_tube.map_or(CRADLE_TUBE, v),
            support_palm: g.support_palm.map_or(CRADLE_PALM, v),
            mag_well: v(g.mag_well),
            muzzle: v(g.muzzle),
            handle: g.handle.map(|h| (v(h), g.handle_travel)),
            trigger_bone: g.trigger_bone.clone(),
            mag_bones: g.mag_bones.clone(),
            bolt_bones: g.bolt_bones.clone(),
            mag_meshes: g.mag_meshes.clone(),
            hidden_bones: g.hidden_bones.clone(),
            hidden_meshes: g.hidden_meshes.clone(),
            recoil: g.recoil,
            recoil_feel: g.recoil_feel,
            rounds_per_second: g.rounds_per_second,
            automatic: g.automatic,
            slide_locks: g.slide_locks,
            mag_size: g.mag_size,
            mass: g.mass,
            mag_mass: g.mag_mass,
            left_side_up: g.left_side_up,
            ads_zoom: g.ads_zoom.unwrap_or(1.1),
            action: g.action,
            hinge: g.hinge.map(v),
            open_deg: g.open_deg,
            barrel_bones: g.barrel_bones.clone(),
            chamber_bones: g.chamber_bones.clone(),
            bolt_turn: g.bolt_turn,
            handle_swing: g.handle_swing,
            pressure: g.pressure.clone(),
        }
    }
}

/// Metro 2033's rigs (`content/rigs/metro2033.toml`, or `RUSTVR_RIGS`'s), read once.
fn metro_rigs() -> &'static [MetroRig] {
    static RIGS: std::sync::OnceLock<Vec<MetroRig>> = std::sync::OnceLock::new();
    RIGS.get_or_init(|| {
        let file = asset::rigs::load("metro2033").unwrap_or_else(|e| {
            tracing::warn!("range: {e:#}: using the compiled-in rigs");
            asset::rigs::builtin("metro2033").unwrap_or_default()
        });
        file.guns.iter().map(MetroRig::new).collect()
    })
}

/// Metro 2033's guns from `local_assets/metro2033` (extracted by `metro-extract`), if present.
/// The ones with a rig (see [`metro_rigs`]) are properly rigged; the rest keep the extractor's
/// guessed points (legacy mode only).
pub fn metro_weapons(root: &std::path::Path) -> Vec<Weapon> {
    let dir = root.join("metro2033");
    let Ok(index) = asset::load_index(&dir) else { return Vec::new() };
    let rest = asset::load_model(&dir, ARMS_REST).ok();
    let mut out = Vec::new();
    for item in index.items.iter().filter(|i| i.kind == asset::ItemKind::Weapon) {
        let Some(rig) = item.rig else { continue };
        let model = match asset::load_model(&dir, &item.key) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("range: {}: {e:#}", item.key);
                continue;
            }
        };
        let arms = item.hands.as_ref().and_then(|k| asset::load_model(&dir, k).ok());
        let hands = arms.as_ref().map(model_meshes).unwrap_or_default();
        let id = out.len() as u64 + 100;
        // The game's own recoil, for every gun that has it (rigged or not).
        let kicks = match asset::load_recoil(&dir, &item.key) {
            Ok(r) => r.kicks,
            Err(e) => {
                tracing::info!("range: {} has no recoil of its own ({e:#}): the runtime's", item.key);
                Vec::new()
            }
        };
        if let Some(r) = metro_rigs().iter().find(|r| r.key == item.key) {
            // The arms posed for this gun (not the plain suit every unposed gun falls back to).
            let posed = arms.as_ref().filter(|_| item.hands.as_deref() != Some(ARMS_REST));
            let mut w = rigged_metro_weapon(r, item, &model, hands, posed, rest.as_ref(), id);
            w.kicks = kicks;
            out.push(w);
            continue;
        }
        // Turn the model so its barrel points along +Z, with the grip at the origin.
        let fwd = Vec3::from_array(rig.forward).normalize_or(Vec3::Z);
        let rot = Quat::from_rotation_arc(fwd, Vec3::Z);
        let grip = Vec3::from_array(rig.grip);
        let fix = Mat4::from_quat(rot) * Mat4::from_translation(-grip);
        let p = |v: [f32; 3]| fix.transform_point3(Vec3::from_array(v)).to_array();
        let wr = WeaponRig {
            id,
            flags: RIG_HAS_SIGHT | RIG_HAS_MUZZLE | RIG_HAS_MAG_WELL | RIG_TWO_HANDED,
            grip: p(rig.grip),
            foregrip: p(rig.foregrip),
            sight: p(rig.sight),
            muzzle: p(rig.muzzle),
            mag_well: p(rig.mag_well),
            ..Default::default()
        };
        let scoped = item.key.contains("vsv") || item.key.contains("scope") || item.key.contains("optic") || item.key.contains("sniper");
        let pistol = item.key.contains("revolver") || item.key.contains("pistol");
        out.push(Weapon {
            name: item.name.clone(),
            key: item.key.clone(),
            pistol,
            stock: !pistol,
            handle: None,
            slide_locks: false,
            parts: model_meshes(&model),
            fix,
            hands,
            mag_center: Vec3::from_array(wr.mag_well) - Vec3::new(0.0, 0.08, 0.0),
            rig: wr,
            mag_parts: Vec::new(),
            bolt_parts: Vec::new(),
            trigger_parts: Vec::new(),
            trigger_pivot: None,
            mag_size: item.mag_size.unwrap_or(30).max(1),
            rounds_per_second: 9.0,
            automatic: !item.key.contains("revolver") && !item.key.contains("duplet") && !item.key.contains("helsing"),
            ads_zoom: if scoped { 4.0 } else { 1.1 },
            from_game: true,
            grip_rot: None,
            support_rot: None,
            recoil: if pistol { [12.0, 3.0, 0.03] } else { [6.5, 1.6, 0.022] },
            recoil_feel: [0.0; 3],
            kicks,
            game_hands: [None; 2],
            mass: if pistol { 1.0 } else { 3.5 },
            mag_mass: 0.4,
            left_side_up: false,
            sights: None,
            trigger: None,
            grip_block: None,
            trigger_blade: Vec::new(),
            game_arms: None,
            caliber: String::new(),
            magazine: item.key.clone(),
            action: WeaponAction::default(),
        });
    }
    out
}

fn rigged_metro_weapon(r: &MetroRig, item: &asset::Item, model: &asset::Model, hands: Vec<MeshData>, arms: Option<&asset::Model>, rest: Option<&asset::Model>, id: u64) -> Weapon {
    // Weapon space: the line of sight along +Z, the firing grip at the origin.
    let rot = Quat::from_rotation_arc((r.front_sight - r.rear_sight).normalize(), Vec3::Z);
    let fix = Mat4::from_quat(rot) * Mat4::from_translation(-r.grip);
    let p = |v: Vec3| fix.transform_point3(v);
    let mag_bones: Vec<&str> = r.mag_bones.iter().map(String::as_str).collect();
    let bolt_bones: Vec<&str> = r.bolt_bones.iter().map(String::as_str).collect();
    let trigger_bone = [r.trigger_bone.as_str()];
    let hidden: Vec<&str> = r.hidden_bones.iter().map(String::as_str).collect();
    let barrel_bones: Vec<&str> = r.barrel_bones.iter().map(String::as_str).collect();
    let gauge_bone: Vec<&str> = r.pressure.as_ref().and_then(|p| p.gauge_bone.as_deref()).into_iter().collect();
    let chamber_bones: Vec<[&str; 1]> = r.chamber_bones.iter().map(|b| [b.as_str()]).collect();
    // The groups, in order: magazine, bolt, trigger, hidden, barrels, gauge, then each chamber's
    // round. (A bone goes to the group that names it or its nearest named ancestor: rounds under
    // the barrels are rounds.)
    let mut list = vec![
        Group { bones: &mag_bones, meshes: &r.mag_meshes },
        Group { bones: &bolt_bones, meshes: &[] },
        Group { bones: &trigger_bone, meshes: &[] },
        Group { bones: &hidden, meshes: &r.hidden_meshes },
        Group { bones: &barrel_bones, meshes: &[] },
        Group { bones: &gauge_bone, meshes: &[] },
    ];
    list.extend(chamber_bones.iter().map(|b| Group { bones: b, meshes: &[] }));
    let mut groups = split_by_bones(model, &list).into_iter();
    let mut parts = groups.next().unwrap_or_default();
    let take = |parts: &mut Vec<MeshData>, g: Vec<MeshData>| {
        let range: Vec<usize> = (parts.len()..parts.len() + g.len()).collect();
        parts.extend(g);
        range
    };
    let mag_parts = take(&mut parts, groups.next().unwrap_or_default());
    let bolt_parts = take(&mut parts, groups.next().unwrap_or_default());
    let trigger_parts = take(&mut parts, groups.next().unwrap_or_default());
    let _hidden = groups.next();
    let barrel_parts = take(&mut parts, groups.next().unwrap_or_default());
    let gauge_parts = take(&mut parts, groups.next().unwrap_or_default());
    let chamber_parts: Vec<Vec<usize>> = groups.map(|g| take(&mut parts, g)).collect();
    // How it's worked, in weapon space.
    let muzzle_w = p(r.muzzle);
    let mut action = WeaponAction { barrel_parts, chamber_parts, ..Default::default() };
    action.kind = match r.action {
        asset::rigs::Action::Auto => ACTION_SELF_LOADING,
        asset::rigs::Action::Bolt => ACTION_BOLT,
        asset::rigs::Action::Pump => ACTION_PUMP,
        asset::rigs::Action::Break => ACTION_BREAK,
        asset::rigs::Action::Swing => ACTION_SWING,
    };
    if let Some(h) = r.hinge {
        action.hinge = p(h);
        action.open = r.open_deg.to_radians();
    }
    action.chambers = r.chamber_bones.iter().map(|b| bone_position(model, b).map_or(p(r.mag_well), p)).collect();
    action.bolt_turn = r.bolt_turn.map(|t| t.map(f32::to_radians));
    action.lever = r.handle_swing.map(|s| (p(Vec3::from_array(s.pivot)), s.deg.to_radians()));
    if let Some(pr) = &r.pressure {
        action.air = Some([pr.pump, pr.shot, pr.min]);
        action.gauge = pr.gauge_bone.as_ref().and_then(|b| {
            let bone = model.bones.iter().find(|x| &x.name == b)?;
            let m = Mat4::from_cols_array(&bone.model_matrix);
            Some(Gauge {
                parts: gauge_parts.clone(),
                at: p(m.w_axis.truncate()),
                axis: fix.transform_vector3(m.x_axis.truncate()).normalize_or(Vec3::X),
                turn: pr.gauge_turn.map(|t| t.map(f32::to_radians)),
                slide: pr.gauge_slide,
            })
        });
    }
    // A bolt's knob is grabbed where it is closed (the model may show it turned).
    let handle = r.handle.map(|(h, travel)| {
        let at = p(h);
        let closed = action.bolt_turn.map_or(0.0, |t| t[0]);
        let axis = Vec3::new(muzzle_w.x, muzzle_w.y, at.z);
        (axis + Quat::from_rotation_z(closed) * (at - axis), travel)
    });
    // The trigger swings about its bone (in the receiver): a full pull carries its face back by
    // the trigger travel.
    let trigger_pivot = bone_position(model, &r.trigger_bone).map(|b| {
        let pivot = p(b);
        (pivot, TRIGGER_TRAVEL / (p(r.trigger) - pivot).length().max(0.005))
    });
    let mag_center = parts_bounds(&parts, &mag_parts, fix).map(|(a, b)| (a + b) * 0.5).unwrap_or(p(r.mag_well) - Vec3::Y * 0.08);
    let sight = p(r.rear_sight);
    // The game's own hands on it, exactly as Metro's arms hold it (no fitting: the game is the
    // truth). Where the controllers go follows from them through the one controller-to-hand
    // mapping (the IK's `HandOffsets`), so the drawn hands land exactly on Metro's. A pistol is
    // held in the firing hand alone.
    let game_hands = [0, 1].map(|side| arms.filter(|_| side == 1 || !r.pistol).and_then(|a| GunHand::from_arms(a, rest, side, fix, None)));
    let measured_grip = (p(r.grip), (rot * pistol_grip(raked(r.grip_rake_deg), r.grip_twist_deg)).normalize());
    let measured_fore = r.foregrip.map(|f| (p(f), (rot * fist(r.support_tube, r.support_palm)).normalize()));
    let (grip, grip_rot) = game_hands[1].as_ref().map_or(measured_grip, |h| controller_on(h, 1));
    let (foregrip, support_rot) = match (&game_hands[0], measured_fore) {
        _ if r.pistol => (grip, None),
        (Some(l), _) => {
            let (at, q) = controller_on(l, 0);
            (at, Some(q))
        }
        (None, Some((at, q))) => (at, Some(q)),
        (None, None) => (grip, None),
    };
    let rig = WeaponRig {
        id,
        flags: RIG_HAS_SIGHT
            | RIG_HAS_MUZZLE
            | RIG_HAS_MAG_WELL
            | RIG_SIGHT_LINE
            | if r.pistol { 0 } else { RIG_TWO_HANDED }
            | if r.handle.is_some() { RIG_HAS_CHARGING_HANDLE } else { 0 },
        grip: grip.to_array(),
        foregrip: foregrip.to_array(),
        sight: sight.to_array(),
        muzzle: p(r.muzzle).to_array(),
        mag_well: p(r.mag_well).to_array(),
        sight_line_origin: sight.to_array(),
        sight_line_dir: [0.0, 0.0, 1.0],
        charging_handle: handle.map_or(grip, |(h, _)| h).to_array(),
        ..Default::default()
    };
    Weapon {
        name: r.name.clone().unwrap_or_else(|| item.name.clone()),
        key: item.key.clone(),
        pistol: r.pistol,
        stock: r.stock,
        handle,
        slide_locks: r.slide_locks,
        parts,
        fix,
        hands,
        rig,
        mag_parts,
        bolt_parts,
        trigger_parts,
        trigger_pivot,
        mag_center,
        mag_size: r.mag_size.or(item.mag_size).unwrap_or(30).max(1),
        rounds_per_second: r.rounds_per_second,
        automatic: r.automatic,
        ads_zoom: r.ads_zoom,
        from_game: true,
        grip_rot: Some(grip_rot),
        support_rot,
        recoil: r.recoil,
        recoil_feel: r.recoil_feel,
        kicks: Vec::new(),
        // Metro's arms for this gun hold it in their pose (its arms idle, baked by the extractor):
        // the same space as the gun's model, so `fix` takes them into weapon space too.
        game_hands,
        mass: r.mass,
        mag_mass: r.mag_mass,
        left_side_up: r.left_side_up,
        sights: Some((sight, p(r.front_sight))),
        trigger: Some(p(r.trigger)),
        grip_block: Some((p(r.grip_block.0), r.grip_block.1)),
        trigger_blade: bone_triangles(model, &r.trigger_bone, fix),
        game_arms: arms.map(|a| std::sync::Arc::new(GameArms { posed: a.clone(), rest: rest.cloned() })),
        caliber: r.caliber.clone(),
        magazine: r.magazine.clone(),
        action,
    }
}

/// A bone's model-space position, by name.
fn bone_position(model: &asset::Model, name: &str) -> Option<Vec3> {
    let b = model.bones.iter().find(|b| b.name == name)?;
    Some(Mat4::from_cols_array(&b.model_matrix).w_axis.truncate())
}

/// A mesh's triangles after `m`.
pub fn triangles(mesh: &MeshData, m: Mat4) -> Vec<[Vec3; 3]> {
    mesh.indices.as_chunks::<3>().0.iter().map(|t| t.map(|i| m.transform_point3(Vec3::from_array(mesh.vertices[i as usize].pos)))).collect()
}

/// The triangles of a skinned model's part on `bone` (at least two of their vertices carried
/// mostly by it), after `m`.
fn bone_triangles(model: &asset::Model, bone: &str, m: Mat4) -> Vec<[Vec3; 3]> {
    bone_triangles_with_normals(model, bone).into_iter().map(|(p, _)| p.map(|p| m.transform_point3(p))).collect()
}

/// [`bone_triangles`] in model space, with their vertex normals.
fn bone_triangles_with_normals(model: &asset::Model, bone: &str) -> Vec<([Vec3; 3], [Vec3; 3])> {
    let Some(b) = model.bones.iter().position(|x| x.name == bone) else { return Vec::new() };
    let on = |v: &asset::Vertex| {
        let k = (0..4).max_by_key(|&k| v.weights[k]).unwrap_or(0);
        v.weights[k] > 0 && v.bones[k] as usize == b
    };
    let mut out = Vec::new();
    for mesh in &model.meshes {
        for t in mesh.indices.as_chunks::<3>().0 {
            let v = t.map(|i| &mesh.vertices[i as usize]);
            if v.iter().filter(|v| on(v)).count() >= 2 {
                out.push((v.map(|v| Vec3::from_array(v.pos)), v.map(|v| Vec3::from_array(v.normal))));
            }
        }
    }
    out
}

/// Bounds of some of `parts` in weapon space.
pub fn parts_bounds(parts: &[MeshData], which: &[usize], fix: Mat4) -> Option<(Vec3, Vec3)> {
    which.iter().filter_map(|&i| parts[i].bounds(fix)).reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
}

/// A bone name matches a rig's: exactly, or up to a trailing `*`.
fn bone_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

/// A group of a gun's skin: the named bones (and everything under them), and whole meshes (by
/// their index in the model).
struct Group<'a> {
    bones: &'a [&'a str],
    meshes: &'a [usize],
}

/// Splits a skinned model by the bones that carry its skin: a triangle goes to the first group
/// whose meshes it's in or that owns at least two of its vertices (each vertex belongs to its
/// heaviest bone; a group is the named bones and everything under them), else to the body.
/// Returns the body, then each group, as one mesh per source mesh (and texture) that has
/// triangles there.
fn split_by_bones(m: &asset::Model, groups: &[Group]) -> Vec<Vec<MeshData>> {
    // Which group (1-based, 0 = body) each bone belongs to.
    let mut bone_group = vec![0usize; m.bones.len()];
    for (b, bone_group) in bone_group.iter_mut().enumerate() {
        let mut at = Some(b);
        while let Some(i) = at {
            if let Some(g) = groups.iter().position(|g| g.bones.iter().any(|p| bone_matches(p, &m.bones[i].name))) {
                *bone_group = g + 1;
                break;
            }
            at = usize::try_from(m.bones[i].parent).ok().filter(|&p| p < m.bones.len());
        }
    }
    let vertex_group = |v: &asset::Vertex| {
        let k = (0..4).max_by_key(|&k| v.weights[k]).unwrap_or(0);
        if v.weights[k] == 0 { 0 } else { bone_group.get(v.bones[k] as usize).copied().unwrap_or(0) }
    };
    let mut out: Vec<Vec<MeshData>> = vec![Vec::new(); groups.len() + 1];
    for (index, mesh) in m.meshes.iter().enumerate().filter(|(_, mesh)| !mesh.indices.is_empty()) {
        let texture = if mesh.texture.is_empty() { None } else { Some(mesh.texture.clone()) };
        let mut split: Vec<MeshData> = vec![MeshData { texture: texture.clone(), ..Default::default() }; groups.len() + 1];
        let mut remap: Vec<Vec<u32>> = vec![vec![u32::MAX; mesh.vertices.len()]; groups.len() + 1];
        let whole = groups.iter().position(|g| g.meshes.contains(&index)).map(|g| g + 1);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let g3 = tri.map(|i| vertex_group(&mesh.vertices[i as usize]));
            let g = whole.unwrap_or_else(|| (1..=groups.len()).find(|&g| g3.iter().filter(|&&x| x == g).count() >= 2).unwrap_or(0));
            for &i in tri {
                let slot = &mut remap[g][i as usize];
                if *slot == u32::MAX {
                    let v = &mesh.vertices[i as usize];
                    *slot = split[g].vertices.len() as u32;
                    split[g].vertices.push(Vertex { pos: v.pos, normal: v.normal, uv: v.uv, color: [255, 255, 255, 255] });
                }
                split[g].indices.push(*slot);
            }
        }
        for (g, part) in split.into_iter().enumerate() {
            if !part.indices.is_empty() {
                out[g].push(part);
            }
        }
    }
    out
}

fn model_meshes(m: &asset::Model) -> Vec<MeshData> {
    m.meshes
        .iter()
        .filter(|mesh| !mesh.indices.is_empty())
        .map(|mesh| MeshData {
            vertices: mesh
                .vertices
                .iter()
                .map(|v| Vertex { pos: v.pos, normal: v.normal, uv: v.uv, color: [255, 255, 255, 255] })
                .collect(),
            indices: mesh.indices.clone(),
            texture: if mesh.texture.is_empty() { None } else { Some(mesh.texture.clone()) },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metro() -> Vec<Weapon> {
        let w = metro_weapons(&asset::default_root());
        if w.is_empty() {
            eprintln!("no Metro 2033 assets: only the built-in guns are checked");
        }
        w
    }

    fn all() -> Vec<Weapon> {
        let mut w = metro();
        w.extend([builtin_rifle("Range rifle", false), builtin_rifle("Range rifle (scoped)", true), builtin_pistol()]);
        w
    }

    #[test]
    fn the_magazine_is_its_own_small_part() {
        for w in all().iter().filter(|w| w.rigged()) {
            let tris = |which: &[usize]| which.iter().map(|&i| w.parts[i].indices.len() / 3).sum::<usize>();
            if !w.action.chambers.is_empty() {
                // Loaded round by round: each chamber's round is its own small part, its base at
                // the chamber's mouth.
                assert_eq!(w.action.chambers.len(), w.action.chamber_parts.len(), "{}", w.key);
                for (k, (mouth, parts)) in w.action.chambers.iter().zip(&w.action.chamber_parts).enumerate() {
                    let (lo, hi) = parts_bounds(&w.parts, parts, w.fix).unwrap_or_else(|| panic!("{}: chamber {k} has no round", w.key));
                    assert!(tris(parts) > 0 && (hi - lo).max_element() < 0.12, "{}: chamber {k}'s round {lo}..{hi}", w.key);
                    let d = mouth.clamp(lo, hi).distance(*mouth);
                    assert!(d < 0.005, "{}: chamber {k}'s mouth {mouth} is {d} m from its round {lo}..{hi}", w.key);
                }
                assert!(w.mag_parts.is_empty(), "{}: chambers and a magazine", w.key);
                continue;
            }
            if w.mag_parts.is_empty() {
                // (A gun whose rounds stay in it: the flamethrower's fuel.)
                assert!(w.from_game, "{}: no magazine part", w.key);
                continue;
            }
            let body: Vec<usize> = (0..w.parts.len()).filter(|i| !w.mag_parts.contains(i) && !w.bolt_parts.contains(i)).collect();
            let (mag, gun) = (tris(&w.mag_parts), tris(&body));
            // (Metro's magazines carry their rounds, so triangles say little: their size does.)
            assert!(mag > 0 && mag < gun, "{}: magazine {mag} triangles, gun {gun}", w.key);
            let (lo, hi) = parts_bounds(&w.parts, &w.mag_parts, w.fix).unwrap();
            let (glo, ghi) = parts_bounds(&w.parts, &body, w.fix).unwrap();
            let (size, gun_size) = (hi - lo, ghi - glo);
            let volume = |v: Vec3| v.x * v.y * v.z;
            // (A pistol's magazine fills its grip; darts and drums run long.)
            let (long, share) = if w.pistol { (0.45, 0.45) } else { (0.75, 0.3) };
            assert!(size.z < gun_size.z * long && size.max_element() < 0.5, "{}: magazine {size} in a gun {gun_size}", w.key);
            assert!(volume(size) < volume(gun_size) * share, "{}: magazine {size} in a gun {gun_size}", w.key);
            // The well is at the magazine's top: on its bounds, not floating away from it.
            let well = Vec3::from_array(w.rig.mag_well);
            let d = well.clamp(lo, hi).distance(well);
            assert!(d < 0.01, "{}: mag well {well} is {d} m from the magazine {lo}..{hi}", w.key);
            if matches!(w.key.as_str(), "wpn_ak" | "wpn_ubludok") {
                assert!(!w.bolt_parts.is_empty(), "{}: no bolt part", w.key);
            }
            if w.from_game && !w.bolt_parts.is_empty() {
                assert!(w.handle.is_some(), "{}: a bolt without a handle", w.key);
                let bolt = tris(&w.bolt_parts);
                assert!(bolt > 0 && bolt * 2 < gun, "{}: bolt {bolt} triangles", w.key);
            }
        }
    }

    #[test]
    fn rigged_guns_have_authored_hands_and_a_true_sight_line() {
        let guns = all();
        let rigged: Vec<&Weapon> = guns.iter().filter(|w| w.rigged()).collect();
        assert!(rigged.len() >= 3);
        if guns.iter().any(|w| w.from_game) {
            for key in ["wpn_ak", "wpn_ubludok"] {
                assert!(rigged.iter().any(|w| w.key == key), "{key} is rigged");
            }
        }
        for w in rigged {
            let q = w.grip_rot.unwrap();
            assert!((q.length() - 1.0).abs() < 1e-4);
            // A heavy gun's handles are whatever the game makes them (a spade grip, a handlebar, a
            // side handle): its hands are checked against the game's (tests/rig.rs), not these.
            if !w.stock && !w.pistol {
                continue;
            }
            // The fist runs up the pistol grip (bottom back), the back of the right hand to the right.
            let tube = q * Vec3::Z;
            // (Raked anywhere from upright to a straight stock's wrist: the game's hand decides.)
            assert!(tube.y > 0.5 && tube.z > -0.1 && tube.z < 0.87, "{}: grip tube {tube}", w.key);
            assert!((q * Vec3::X).x > 0.7, "{}: the back of the hand faces {}", w.key, q * Vec3::X);
            if !w.pistol {
                // The support hand cradles the handguard: palm up (a left hand's grip +X is out of
                // its palm), the fist's tube pointing forward (and up and left).
                let s = w.support_rot.unwrap();
                assert!((s * Vec3::X).y > 0.7, "{}: support palm up", w.key);
                assert!((s * Vec3::Z).z > 0.4 && (s * Vec3::Z).x < 0.0, "{}: support fist {}", w.key, s * Vec3::Z);
            }
            // The line of sight (along +Z through `sight`) passes through both sights.
            let (rear, front) = w.sights.unwrap();
            let sight = Vec3::from_array(w.rig.sight);
            for p in [rear, front] {
                let off = (p - sight).truncate().length();
                assert!(off < 5e-4, "{}: line of sight misses a sight by {off} m", w.key);
            }
            assert!(front.z - rear.z > 0.1, "{}", w.key);
            // The trigger is ahead of the grip, near the top of the fist.
            let t = w.trigger.unwrap() - Vec3::from_array(w.rig.grip);
            assert!(t.z > 0.03 && t.z < 0.09 && t.y > -0.03 && t.y < 0.06, "{}: trigger at {t} from the palm", w.key);
            if let Some((h, travel)) = w.handle {
                // The handle is on the gun, and racks a believable distance (the Preved's long bolt
                // and the Tihar's pump lever the furthest, 15.7 and 17.6 cm).
                assert!(h.length() < 0.9 && (0.015..=0.2).contains(&travel), "{}: handle {h} travel {travel}", w.key);
            }
        }
    }
}

