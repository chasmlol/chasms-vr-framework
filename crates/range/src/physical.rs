//! Physical mode: the range's guns, magazines and props are objects the runtime's hands pick up,
//! holster, reload and fire. The range registers them (with its static collision, and the game's
//! own hand poses on the guns: Metro's first-person arms, see `content::GunHand`), then each frame
//! draws them where the runtime says they are, the trigger of a held gun following its hand's, and
//! turns their shot counters into bullets. Other modules add their own objects to the table
//! (`add_objects`: the throwables, which `throwables` draws).
//!
//! The range makes no sound itself: it hands the runtime Metro's own sounds (the sound pack
//! `metro-extract` writes to `local_assets/metro2033/sounds`, see [`sound_pack`]), each gun and
//! throwable the set it sounds like, and the runtime plays them, its shots and explosions
//! included, as it would for any game that doesn't play its own.

use bytemuck::Zeroable;
use glam::{Mat3, Mat4, Quat, Vec3};
use rustvr_adapter::View;
use rustvr_protocol::*;
use std::collections::HashMap;

use crate::content::{self, Weapon};

/// Desc ids: weapon `i` is `WEAPON_BASE + i`, its magazine template `MAG_BASE + i`.
pub const WEAPON_BASE: u32 = 1;
pub const MAG_BASE: u32 = 1000;
pub const PROP_BASE: u32 = 2000;
/// (Throwables are THROW_BASE 3000 on.)
pub const VALVE_BASE: u32 = 4000;

/// Hand pose ids (the hand pose table): weapon `i`'s firing hand, and its support hand.
pub fn grip_pose_id(i: usize) -> u32 {
    2 * i as u32 + 1
}
pub fn support_pose_id(i: usize) -> u32 {
    2 * i as u32 + 2
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Weapon(usize),
    /// A magazine for weapon `i`, drawn with that weapon's magazine part (or a box).
    Mag(usize),
    Prop { color: [f32; 4] },
    /// Throwable `i` (drawn by `throwables`).
    Throwable(usize),
    /// Valve `i` of `content::valves` (drawn with its wheel).
    Valve(usize),
}

/// A bullet to fire: world origin and direction, and the gun (weapon index, world pose and
/// velocity) it came from.
pub struct Shot {
    pub origin: Vec3,
    pub dir: Vec3,
    pub weapon: usize,
    pub gun: (Vec3, Quat),
    pub vel: Vec3,
}

/// A case (or a round) the runtime threw out of a gun: out of an open chamber (its mouth and the
/// way it points, world) or, without one, out of the ejection port.
pub struct Ejection {
    pub weapon: usize,
    pub gun: (Vec3, Quat),
    pub vel: Vec3,
    pub chamber: Option<(Vec3, Vec3)>,
}

pub struct Armory {
    pub table: Box<ObjectTable>,
    /// The guns' own recoil (see `kick_descs`).
    pub kicks: Box<KickTable>,
    /// The game's own hands on the guns (Metro's arms, see `content::GunHand`).
    pub poses: Box<HandPoseTable>,
    /// Metro's sounds, when extracted, and each gun's set (see [`sound_pack`]).
    pub sounds: Box<SoundPack>,
    pub kinds: HashMap<u32, Kind>,
    descs: HashMap<u32, ObjectDesc>,
    pub states: Box<ObjectStates>,
    shots_seen: HashMap<u32, u32>,
    /// What each gun's chambers held last time (`ObjectState::chambered`).
    chambers_seen: HashMap<u32, u32>,
    collision: Vec<ColBox>,
    /// How many of `collision` the runtime has (the ring can fill up mid-way).
    sent: usize,
    /// The objects' convex parts (guns, blades, grenades: cut from their models), their collision.
    pub hulls: Box<GameHulls>,
    pub registered: bool,
    /// The table changed (an object respawned): send it again.
    dirty: bool,
}

/// The most convex parts a gun's collision is cut into.
const GUN_PARTS: usize = 8;

/// What a draw list needs: (mesh id, model, tint, emissive, two-sided).
pub type DrawItem = (usize, Mat4, [f32; 4], f32, bool);

/// Mesh ids for weapon parts in the physical draw list (see `to_draws`).
pub fn part_id(weapon: usize, part: usize) -> usize {
    10_000 + weapon * 100 + part
}

use rustvr_guns::desc::{bounds_of, drawn_bounds, weapon_bounds};

/// The rotation of a gun lying on its side with the barrel along `barrel`: its right side up, or
/// its left side up when `left_side_up` (a magazine sticking out of the left side then points up).
fn lying_rot(barrel: Vec3, left_side_up: bool) -> Quat {
    let x = if left_side_up { -Vec3::Y } else { Vec3::Y };
    let z = barrel.normalize();
    Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z))
}

/// A gun lying on its side on a table top at height `top`, barrel along +X, its (drawn) length
/// centred on `x` and its near edge (towards -Z) at `front`. Returns the pose and the far edge.
fn lying(w: &Weapon, x: f32, front: f32, top: f32) -> (Pose, f32) {
    let rot = lying_rot(Vec3::X, w.left_side_up);
    let (cmin, cmax) = weapon_bounds(w);
    let (dmin, dmax) = drawn_bounds(w);
    // World extents of the weapon-space bounds under `rot` (axis-aligned either way).
    let world = |min: Vec3, max: Vec3| {
        let corners = (0..8).map(|k| rot * Vec3::new([min.x, max.x][k & 1], [min.y, max.y][(k >> 1) & 1], [min.z, max.z][k >> 2]));
        bounds_of(corners).unwrap()
    };
    let (collider_lo, _) = world(cmin, cmax);
    let (drawn_lo, drawn_hi) = world(dmin, dmax);
    // The collider rests on the table; the drawn gun (magazine and all) sets the footprint.
    let pos = Vec3::new(x - 0.5 * (drawn_lo.x + drawn_hi.x), top + 0.01 - collider_lo.y, front - drawn_lo.z);
    (Pose { pos: pos.to_array(), valid: 1, rot: rot.to_array() }, pos.z + drawn_hi.z)
}

/// The rigged guns' places on the near table, front to back, in `order` (indices into
/// `weapons`): (index, spawn pose).
pub fn table_layout(weapons: &[&Weapon], order: &[usize]) -> Vec<(usize, Pose)> {
    let (min, max) = content::TABLE_NEAR;
    let mut front = min.z + 0.03;
    let mut out = Vec::new();
    for &i in order {
        let (pose, back) = lying(weapons[i], 0.5 * (min.x + max.x), front, max.y);
        out.push((i, pose));
        front = back + 0.025;
    }
    out
}

/// How far apart the guns on the armory bench lie (their drawn footprints), and how far in from
/// its edges.
const ARMORY_GAP: f32 = 0.03;
const ARMORY_MARGIN: f32 = 0.05;

/// The guns' places on the armory bench behind you, left to right in `order` (indices into
/// `weapons`): each on its side, barrel to the back wall, its butt a little in from the bench's
/// near edge. (index, spawn pose).
pub fn armory_layout(weapons: &[&Weapon], order: &[usize]) -> Vec<(usize, Pose)> {
    let (min, max) = content::TABLE_ARMORY;
    let barrel = -Vec3::Z;
    let mut x = min.x + ARMORY_MARGIN;
    let mut out = Vec::new();
    for &i in order {
        let w = weapons[i];
        let rot = lying_rot(barrel, w.left_side_up);
        let (lo, hi) = world_bounds(rot, drawn_bounds(w));
        // The butt (the far end from the barrel) at the near edge.
        let (pose, right) = lying_from(w, barrel, x, max.z - ARMORY_MARGIN - (hi.z - lo.z), max.y);
        if right > max.x - ARMORY_MARGIN {
            tracing::warn!("range: {} doesn't fit on the armory bench", w.key);
        }
        out.push((i, pose));
        x = right + ARMORY_GAP;
    }
    out
}

/// Axis-aligned bounds of weapon-space bounds turned by `rot`.
fn world_bounds(rot: Quat, (min, max): (Vec3, Vec3)) -> (Vec3, Vec3) {
    let corners = (0..8).map(|k| rot * Vec3::new([min.x, max.x][k & 1], [min.y, max.y][(k >> 1) & 1], [min.z, max.z][k >> 2]));
    bounds_of(corners).unwrap()
}

/// A gun lying on its side on a table top at height `top` with its barrel along `barrel`, the
/// corner of its drawn footprint towards -X and -Z at (`x`, `z`). Returns the pose and the
/// footprint's far edge along X.
fn lying_from(w: &Weapon, barrel: Vec3, x: f32, z: f32, top: f32) -> (Pose, f32) {
    let rot = lying_rot(barrel, w.left_side_up);
    let (collider_lo, _) = world_bounds(rot, weapon_bounds(w));
    let (drawn_lo, drawn_hi) = world_bounds(rot, drawn_bounds(w));
    // The collider rests on the table; the drawn gun (magazine and all) sets the footprint.
    let pos = Vec3::new(x - drawn_lo.x, top + 0.01 - collider_lo.y, z - drawn_lo.z);
    (Pose { pos: pos.to_array(), valid: 1, rot: rot.to_array() }, pos.x + drawn_hi.x)
}

pub fn weapon_desc(i: usize, w: &Weapon, spawn: Pose, start_slot: u32) -> ObjectDesc {
    let mut d = rustvr_guns::desc::weapon_desc(WEAPON_BASE + i as u32, w, [support_pose_id(i), grip_pose_id(i)]);
    d.spawn_seq = 1;
    d.spawn_pose = spawn;
    d.start_slot = start_slot;
    d.ammo_type = i as u32 + 1;
    d
}

/// A gun's own recoil as kick table entries (see `rustvr_guns::desc::kick_descs`).
pub fn kick_descs(object: u32, w: &Weapon) -> Vec<KickDesc> {
    rustvr_guns::desc::kick_descs(object, w)
}

/// The guns' kicks as the kick table can hold them (`capacity`, the protocol's `MAX_KICKS`): every
/// gun keeps one kick of each kind it has (each layer and `when`: the game's recoil for every way
/// of firing), then the guns get their other variants while they fit, those in `first` (object
/// ids) before the rest. Returned grouped by gun, in `per_gun`'s order.
pub fn fit_kicks(per_gun: Vec<(u32, Vec<KickDesc>)>, first: &[u32], capacity: usize) -> Vec<KickDesc> {
    // Each gun's kicks split into one of each kind and the rest.
    let split: Vec<(u32, Vec<KickDesc>, Vec<KickDesc>)> = per_gun
        .into_iter()
        .map(|(id, kicks)| {
            let (mut one, mut rest) = (Vec::new(), Vec::new());
            for k in kicks {
                if one.iter().any(|o: &KickDesc| o.layer == k.layer && o.when == k.when) {
                    rest.push(k);
                } else {
                    one.push(k);
                }
            }
            (id, one, rest)
        })
        .collect();
    let mut left = capacity.saturating_sub(split.iter().map(|(_, one, _)| one.len()).sum());
    let all: usize = split.iter().map(|(_, one, rest)| one.len() + rest.len()).sum();
    // Who gets their variants: `first`, then everyone else, each wholly or not at all.
    let mut order: Vec<usize> = (0..split.len()).collect();
    order.sort_by_key(|&g| first.iter().position(|&id| id == split[g].0).unwrap_or(usize::MAX));
    let mut full = vec![false; split.len()];
    for g in order {
        if split[g].2.len() <= left {
            left -= split[g].2.len();
            full[g] = true;
        }
    }
    let out: Vec<KickDesc> = split.iter().zip(&full).flat_map(|((_, one, rest), &full)| one.iter().chain(if full { rest.as_slice() } else { &[] }).copied()).take(capacity).collect();
    if out.len() < all {
        tracing::info!("range: {} of the guns' {all} recoil kicks fit the kick table ({capacity}); the rest keep one of each kind", out.len());
    }
    out
}

pub use rustvr_guns::desc::pose_desc;

/// The runtime's ammo types (`ObjectDesc::ammo_type`: guns and magazines with the same one fit
/// together), by weapon: one per kind of magazine (`Weapon::magazine`), so guns that take the same
/// magazine take each other's. Each says what it fires too: `caliber << 8 | magazine`, both
/// numbered from 1 in the order they first appear.
pub fn ammo_types(weapons: &[&Weapon]) -> Vec<u32> {
    let mut calibers: Vec<&str> = Vec::new();
    let mut mags: Vec<(&str, u32)> = Vec::new();
    weapons
        .iter()
        .map(|w| {
            if let Some((_, t)) = mags.iter().find(|(m, _)| *m == w.magazine) {
                return *t;
            }
            let c = match calibers.iter().position(|c| *c == w.caliber) {
                Some(c) => c,
                None => {
                    calibers.push(&w.caliber);
                    calibers.len() - 1
                }
            } as u32
                + 1;
            let t = c << 8 | (mags.iter().filter(|(_, t)| t >> 8 == c).count() as u32 + 1);
            mags.push((&w.magazine, t));
            t
        })
        .collect()
}

/// A gun's magazine template (see `rustvr_guns::desc::mag_desc`).
fn mag_desc(i: usize, w: &Weapon) -> ObjectDesc {
    let mut d = rustvr_guns::desc::mag_desc(MAG_BASE + i as u32, w).expect("a gun that takes magazines or rounds");
    d.ammo_type = i as u32 + 1;
    d
}

fn prop(id: u32, name: &str, at: Vec3, half: Vec3, mass: f32) -> ObjectDesc {
    let mut d = ObjectDesc::zeroed();
    d.id = id;
    d.kind = OBJ_PROP;
    d.flags = OBJF_GRABBABLE | if mass <= 8.0 { OBJF_PULLABLE } else { 0 };
    d.spawn_seq = 1;
    d.spawn_pose = Pose { pos: (at + Vec3::Y * (half.y + 0.005)).to_array(), valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
    d.shape = SHAPE_BOX;
    d.mass = mass;
    d.half_extents = half.to_array();
    set_name(&mut d.name, name);
    d
}

fn col_box(id: u32, min: Vec3, max: Vec3) -> ColBox {
    ColBox { id, center: ((min + max) * 0.5).to_array(), half_extents: ((max - min) * 0.5).to_array(), rot: [0.0, 0.0, 0.0, 1.0], ..Default::default() }
}

/// The sound set a gun uses in Metro's sound pack: a Metro gun its own (its asset key is the
/// game's entity name, `wpn_ak`), the built-in guns the nearest Metro has (the Kalash for the
/// rifles, the revolver for the pistol).
pub fn sound_set(w: &Weapon) -> &str {
    match w.key.as_str() {
        "builtin_rifle" | "builtin_rifle_scoped" => "wpn_ak",
        "builtin_pistol" => "wpn_revolver",
        key => key,
    }
}

/// The sound pack to publish: Metro's (`<assets>/metro2033/sounds`) if it has been extracted, with
/// each registered gun's set; none otherwise (the runtime's own sounds). No flags: the range plays
/// no sounds of its own, so the runtime plays every shot.
pub fn sound_pack(assets: &std::path::Path, guns: &[(u32, &Weapon)]) -> SoundPack {
    let dir = assets.join("metro2033").join("sounds");
    if !dir.join("sounds.toml").is_file() {
        return SoundPack::zeroed();
    }
    let mut pack = SoundPack::new(&dir.to_string_lossy());
    for (id, w) in guns {
        pack.add_object(*id, sound_set(w), 0);
    }
    pack
}

/// Front-to-back order on the near table: the plain rifle nearest (the scenarios reach for it),
/// then Metro's Kalash and Bastard, then the scoped rifle. Every other gun goes on the armory
/// bench (`None`). Guns named in `front` (the range's `--table`) go nearest instead, followed by
/// the plain rifle and the Kalash only (to leave them room).
fn table_rank(w: &Weapon, front: &[String]) -> Option<u32> {
    if let Some(k) = front.iter().position(|k| *k == w.key) {
        return Some(k as u32);
    }
    let first = front.len() as u32;
    match w.key.as_str() {
        "builtin_rifle" => Some(first),
        "wpn_ak" => Some(first + 1),
        "wpn_ubludok" if front.is_empty() => Some(2),
        "builtin_rifle_scoped" if front.is_empty() => Some(4),
        _ => None,
    }
}

/// The pistol that starts in the right hip holster: the range's own, else the first rigged one.
fn hip_pistol(weapons: &[&Weapon]) -> Option<usize> {
    let pistols = || (0..weapons.len()).filter(|&i| weapons[i].rigged() && weapons[i].pistol);
    pistols().find(|&i| weapons[i].key == "builtin_pistol").or_else(|| pistols().next())
}

impl Armory {
    /// Lays out the range: the properly rigged guns (see [`Weapon::rigged`]) on the table at your
    /// right hand (the plain rifle nearest) and the pistol on your hip; things to throw around on
    /// the long table ahead and the floor. Guns that aren't rigged stay out (legacy mode only).
    #[allow(dead_code)] // (the tests lay out the default range)
    pub fn new(weapons: &[&Weapon]) -> Armory {
        Armory::with_front(weapons, &[])
    }

    /// [`Armory::new`], the guns named in `front` (asset keys) laid nearest on the near table.
    pub fn with_front(weapons: &[&Weapon], front: &[String]) -> Armory {
        let table_rank = |w: &Weapon| table_rank(w, front);
        let mut table: Box<ObjectTable> = bytemuck::zeroed_box();
        let mut poses: Box<HandPoseTable> = bytemuck::zeroed_box();
        let mut kicks: Box<KickTable> = bytemuck::zeroed_box();
        let mut kick_list = Vec::new();
        let mut guns = Vec::new();
        let mut kinds = HashMap::new();
        let mut descs = Vec::new();
        let mut pose_descs = Vec::new();
        let long_top = content::TABLE_LONG.1.y;
        let hip = hip_pistol(weapons);
        let rigged = || (0..weapons.len()).filter(|&i| weapons[i].rigged() && Some(i) != hip);
        let mut on_table: Vec<usize> = rigged().filter(|&i| table_rank(weapons[i]).is_some()).collect();
        on_table.sort_by_key(|&i| table_rank(weapons[i]));
        // The armory bench, grouped by what they fire, the pistols last.
        let mut on_bench: Vec<usize> = rigged().filter(|&i| table_rank(weapons[i]).is_none()).collect();
        on_bench.sort_by(|&a, &b| {
            let key = |i: usize| (weapons[i].pistol, weapons[i].caliber.clone(), weapons[i].key.clone());
            key(a).cmp(&key(b))
        });
        let mut places: HashMap<usize, (Pose, u32)> = table_layout(weapons, &on_table).into_iter().chain(armory_layout(weapons, &on_bench)).map(|(i, p)| (i, (p, 0))).collect();
        if let Some(i) = hip {
            // The hip pistol starts in the right hip holster; its spawn pose is on the right end
            // of the long table.
            let (pose, _) = lying(weapons[i], 1.3, content::TABLE_LONG.0.z + 0.2, long_top);
            places.insert(i, (pose, SLOT_HIP_RIGHT + 1));
        }
        let ammo = ammo_types(weapons);
        let mut templates = Vec::new();
        for (i, w) in weapons.iter().enumerate() {
            let Some(&(spawn, slot)) = places.get(&i) else { continue };
            let mut d = weapon_desc(i, w, spawn, slot);
            d.ammo_type = ammo[i];
            for (id, side) in [(d.grip_pose, HAND_RIGHT), (d.support_pose, HAND_LEFT)] {
                if let (true, Some(h)) = (id != 0, &w.game_hands[side]) {
                    pose_descs.push(pose_desc(id, side, h));
                }
            }
            kick_list.push((d.id, kick_descs(d.id, w)));
            guns.push((d.id, *w));
            descs.push(d);
            kinds.insert(WEAPON_BASE + i as u32, Kind::Weapon(i));
            // One magazine template per kind of magazine, from the first gun laid out that takes it.
            // (A gun without one, the flamethrower, holds its rounds itself.)
            if !templates.contains(&ammo[i]) && !w.round_parts().is_empty() {
                templates.push(ammo[i]);
                let mut m = mag_desc(i, w);
                m.ammo_type = ammo[i];
                descs.push(m);
                kinds.insert(MAG_BASE + i as u32, Kind::Mag(i));
            }
        }
        let side_top = content::TABLE_SIDE.1.y;
        let props = [
            ("bottle", Vec3::new(-0.75, long_top, 2.45), Vec3::new(0.035, 0.12, 0.035), 0.6, [0.35, 0.6, 0.35, 1.0]),
            ("bottle", Vec3::new(-0.6, long_top, 2.45), Vec3::new(0.035, 0.12, 0.035), 0.6, [0.55, 0.4, 0.25, 1.0]),
            ("can", Vec3::new(-0.45, long_top, 2.45), Vec3::new(0.033, 0.06, 0.033), 0.35, [0.8, 0.15, 0.12, 1.0]),
            ("can", Vec3::new(-0.32, long_top, 2.45), Vec3::new(0.033, 0.06, 0.033), 0.35, [0.85, 0.85, 0.9, 1.0]),
            ("box", Vec3::new(0.6, long_top, 2.55), Vec3::new(0.1, 0.1, 0.1), 2.0, [0.6, 0.45, 0.3, 1.0]),
            ("crate", Vec3::new(-1.45, 0.0, 1.3), Vec3::new(0.22, 0.22, 0.22), 6.0, [0.55, 0.42, 0.25, 1.0]),
            ("crate", Vec3::new(-1.55, 0.45, 1.35), Vec3::new(0.15, 0.15, 0.15), 3.0, [0.6, 0.5, 0.3, 1.0]),
            ("barrel", Vec3::new(-2.1, 0.0, 0.6), Vec3::new(0.25, 0.4, 0.25), 25.0, [0.25, 0.3, 0.45, 1.0]),
            // Things to throw on the side table at your left hand.
            ("bottle", Vec3::new(-0.55, side_top, 0.36), Vec3::new(0.035, 0.12, 0.035), 0.6, [0.35, 0.6, 0.35, 1.0]),
            ("can", Vec3::new(-0.68, side_top, 0.36), Vec3::new(0.033, 0.06, 0.033), 0.35, [0.8, 0.15, 0.12, 1.0]),
            ("box", Vec3::new(-0.75, side_top, 0.6), Vec3::new(0.08, 0.08, 0.08), 1.5, [0.6, 0.45, 0.3, 1.0]),
            ("bottle", Vec3::new(-2.0, long_top, 2.3), Vec3::new(0.035, 0.12, 0.035), 0.6, [0.35, 0.6, 0.35, 1.0]),
            ("can", Vec3::new(2.0, long_top, 2.3), Vec3::new(0.033, 0.06, 0.033), 0.35, [0.8, 0.15, 0.12, 1.0]),
        ];
        for (k, (name, at, half, mass, color)) in props.into_iter().enumerate() {
            let id = PROP_BASE + k as u32;
            let mut d = prop(id, name, at, half, mass);
            // Bottles, boxes, crates and barrels break; cans don't.
            if name != "can" {
                d.flags |= OBJF_BREAKABLE;
            }
            descs.push(d);
            kinds.insert(id, Kind::Prop { color });
        }
        // The valves on the board, their axles out of it towards you.
        for (k, v) in content::valves().iter().enumerate() {
            let id = VALVE_BASE + k as u32;
            let mut d = ObjectDesc::zeroed();
            d.id = id;
            d.kind = OBJ_VALVE;
            d.spawn_seq = 1;
            d.spawn_pose = Pose { pos: v.centre.to_array(), valid: 1, rot: Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2).to_array() };
            d.radius = v.radius;
            d.half_extents = [v.radius, v.radius, 0.02];
            d.mass = 1.5 + 8.0 * v.radius;
            d.friction = v.friction;
            set_name(&mut d.name, &format!("valve {:.0} cm, {} N m", v.radius * 100.0, v.friction));
            descs.push(d);
            kinds.insert(id, Kind::Valve(k));
        }
        for (k, d) in descs.iter().take(MAX_OBJECTS).enumerate() {
            table.descs[k] = *d;
        }
        table.count = descs.len().min(MAX_OBJECTS) as u32;
        table.seq = 1;
        for (k, p) in pose_descs.iter().take(MAX_HAND_POSES).enumerate() {
            poses.poses[k] = *p;
        }
        poses.count = pose_descs.len().min(MAX_HAND_POSES) as u32;
        poses.seq = 1;
        // The near table's guns (the scenarios fire those) keep every variant first.
        let first: Vec<u32> = on_table.iter().map(|&i| WEAPON_BASE + i as u32).collect();
        let kick_list = fit_kicks(kick_list, &first, MAX_KICKS);
        for (k, d) in kick_list.iter().enumerate() {
            kicks.kicks[k] = *d;
        }
        kicks.count = kick_list.len() as u32;
        kicks.seq = 1;

        let mut collision = Vec::new();
        let mut id = 1;
        for (min, max) in content::shell() {
            collision.push(col_box(id, min, max));
            id += 1;
        }
        for (min, max, _) in content::furniture() {
            collision.push(col_box(id, min, max));
            id += 1;
        }
        for t in content::targets() {
            collision.push(col_box(id, t.center - Vec3::new(t.radius, t.radius, 0.0), t.center + Vec3::new(t.radius, t.radius, 0.02)));
            id += 1;
        }
        let sounds = Box::new(sound_pack(&rustvr_asset::default_root(), &guns));
        // The guns' convex parts, cut from their models (each on its own thread).
        let laid: Vec<(u32, &Weapon)> = guns.iter().map(|(id, w)| (*id, *w)).collect();
        let parts: Vec<(u32, Vec<Vec<Vec3>>)> = std::thread::scope(|s| {
            let jobs: Vec<_> = laid.iter().map(|&(id, w)| s.spawn(move || (id, rustvr_guns::hulls::weapon_parts(w, GUN_PARTS)))).collect();
            jobs.into_iter().filter_map(|j| j.join().ok()).collect()
        });
        Armory {
            table,
            kicks,
            poses,
            sounds,
            kinds,
            descs: descs.iter().map(|d| (d.id, *d)).collect(),
            states: bytemuck::zeroed_box(),
            shots_seen: HashMap::new(),
            chambers_seen: HashMap::new(),
            collision,
            sent: 0,
            hulls: bytemuck::zeroed_box(),
            registered: false,
            dirty: false,
        }
        .with_hulls(&parts)
    }

    fn with_hulls(mut self, parts: &[(u32, Vec<Vec<Vec3>>)]) -> Armory {
        for (id, p) in parts {
            self.add_hulls(*id, p);
        }
        self
    }

    /// Object `id`'s convex parts (object space): its collision from now on (before registering).
    pub fn add_hulls(&mut self, id: u32, parts: &[Vec<Vec3>]) {
        let entries = rustvr_guns::hulls::hull_entries(id, parts);
        let n = self.hulls.count as usize;
        if entries.is_empty() || n + entries.len() > MAX_GAME_HULLS {
            if !entries.is_empty() {
                tracing::warn!("range: the hull table is full: object {id} keeps its box");
            }
            return;
        }
        for (k, h) in entries.iter().enumerate() {
            self.hulls.hulls[n + k] = *h;
        }
        self.hulls.count = (n + entries.len()) as u32;
        for d in self.table.descs[..self.table.count as usize].iter_mut().filter(|d| d.id == id) {
            d.shape = SHAPE_HULL;
        }
        if let Some(d) = self.descs.get_mut(&id) {
            d.shape = SHAPE_HULL;
        }
    }

    /// Objects added by other modules (`add_objects`) that sound like one of the pack's sets
    /// (the throwables, see `throwables::sound_set`): nothing without a pack. Before registering.
    pub fn add_sound_sets(&mut self, sets: &[(u32, &str)]) {
        if self.sounds.path().is_empty() {
            return;
        }
        for (id, set) in sets {
            if !self.sounds.add_object(*id, set, 0) {
                tracing::warn!("range: no room for object {id}'s sound set {set}");
            }
        }
    }

    /// Adds objects of other kinds (before registering).
    pub fn add_objects(&mut self, items: &[(ObjectDesc, Kind)]) {
        for (d, kind) in items {
            let n = self.table.count as usize;
            if n >= MAX_OBJECTS {
                tracing::warn!("range: the object table is full, {} left out", name_str(&d.name));
                break;
            }
            self.table.descs[n] = *d;
            self.table.count += 1;
            self.kinds.insert(d.id, *kind);
            self.descs.insert(d.id, *d);
        }
    }

    /// More static collision (min, max boxes).
    pub fn add_collision(&mut self, boxes: &[(Vec3, Vec3)]) {
        let mut id = self.collision.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        for (min, max) in boxes {
            self.collision.push(col_box(id, *min, *max));
            id += 1;
        }
    }

    /// Puts object `id` back where it started (its spawn pose or slot): the game respawning it.
    pub fn respawn(&mut self, id: u32) {
        let n = self.table.count as usize;
        if let Some(d) = self.table.descs[..n].iter_mut().find(|d| d.id == id) {
            d.spawn_seq = d.spawn_seq.wrapping_add(1).max(1);
            self.dirty = true;
        }
    }

    /// The colour and size (half extents) a prop is drawn at, for its pieces when it breaks.
    pub fn prop_look(&self, id: u32) -> Option<([f32; 4], Vec3)> {
        let d = self.descs.get(&id)?;
        match self.kinds.get(&id)? {
            Kind::Prop { color } => Some((*color, Vec3::from_array(d.half_extents))),
            _ => Some(([0.6, 0.6, 0.6, 1.0], Vec3::from_array(d.half_extents).max(Vec3::splat(0.03)))),
        }
    }

    /// The ids of every object of a kind (`OBJ_*`).
    pub fn ids_of_kind(&self, kind: u32) -> Vec<u32> {
        self.table.descs[..(self.table.count as usize).min(MAX_OBJECTS)].iter().filter(|d| d.id != 0 && d.kind == kind).map(|d| d.id).collect()
    }

    pub fn desc(&self, id: u32) -> Option<&ObjectDesc> {
        self.descs.get(&id)
    }

    /// Which throwable a desc is.
    pub fn throwable(&self, desc: u32) -> Option<usize> {
        match self.kinds.get(&desc) {
            Some(Kind::Throwable(i)) => Some(*i),
            _ => None,
        }
    }

    /// (Re-)registers everything with a newly connected runtime.
    pub fn register(&mut self, adapter: &rustvr_adapter::Adapter) {
        if self.registered && self.dirty {
            self.table.seq = self.table.seq.wrapping_add(1).max(1);
            if adapter.write_objects(&self.table) {
                self.dirty = false;
            }
        }
        if !self.registered {
            self.table.seq = self.table.seq.wrapping_add(1).max(1);
            // The poses first: when the runtime sees the objects, their poses are there.
            self.poses.seq = self.poses.seq.wrapping_add(1).max(1);
            self.kicks.seq = self.kicks.seq.wrapping_add(1).max(1);
            // The hulls before the objects: their colliders are made with them.
            self.hulls.seq = self.hulls.seq.wrapping_add(1).max(1);
            let written = adapter.write_game_hulls(&self.hulls) && adapter.write_hand_poses(&self.poses) && adapter.write_kicks(&self.kicks) && adapter.write_objects(&self.table);
            if written && adapter.set_sound_pack(&self.sounds) {
                self.registered = true;
                self.shots_seen.clear();
                self.chambers_seen.clear();
                tracing::info!("range: registered {} objects, {} hand poses and {} recoil kicks", self.table.count, self.poses.count, self.kicks.count);
                match self.sounds.path() {
                    "" => tracing::info!("range: no sound pack (run metro-extract): the runtime's own sounds"),
                    dir => tracing::info!("range: Metro's sounds from {dir}, {} objects with their own", self.sounds.count),
                }
            }
        }
        while self.sent < self.collision.len() && adapter.push_collision(COL_BOX, bytemuck::bytes_of(&self.collision[self.sent])) {
            self.sent += 1;
        }
    }

    /// The runtime went away: register everything again when one is back.
    pub fn reset(&mut self) {
        self.registered = false;
        self.sent = 0;
        self.shots_seen.clear();
        self.chambers_seen.clear();
        self.states.count = 0;
    }

    /// The object states, read fresh.
    pub fn update(&mut self, adapter: &rustvr_adapter::Adapter) -> bool {
        adapter.read_object_states(&mut self.states)
    }

    pub fn states(&self) -> &[ObjectState] {
        &self.states.states[..(self.states.count as usize).min(MAX_OBJECTS)]
    }

    /// Where an object is in the world (held ones are reported in tracking space).
    pub fn world_pose(s: &ObjectState, v: &View) -> (Vec3, Quat) {
        let p = Vec3::from_array(s.pose.pos);
        let q = Quat::from_array(s.pose.rot).normalize();
        if s.flags & OS_TRACKING != 0 { (v.point(p), v.rot(q)) } else { (p, q) }
    }

    /// New shots fired by the runtime since the last call, as world rays along the line of sight.
    pub fn shots(&mut self, v: &View) -> Vec<Shot> {
        let mut out = Vec::new();
        for s in &self.states.states[..(self.states.count as usize).min(MAX_OBJECTS)] {
            let Some(&Kind::Weapon(weapon)) = self.kinds.get(&s.desc) else { continue };
            let seen = self.shots_seen.entry(s.id).or_insert(s.shots);
            let new = s.shots.wrapping_sub(*seen).min(4);
            *seen = s.shots;
            if new == 0 {
                continue;
            }
            let (p, q) = Self::world_pose(s, v);
            let d = &self.descs[&s.desc];
            let vel = Vec3::from_array(s.lin_vel);
            let vel = if s.flags & OS_TRACKING != 0 { v.dir(vel) } else { vel };
            for _ in 0..new {
                out.push(Shot { origin: p + q * Vec3::from_array(d.sight), dir: q * Vec3::Z, weapon, gun: (p, q), vel });
            }
        }
        out
    }

    /// Cases (and rounds) the runtime threw out of the guns' chambers since the last call: a break
    /// action's spent cases as it opens, whatever a bolt or a pump takes out at the back of its
    /// stroke. (A live round pulled out of an open chamber by hand is in the hand: not one of them.)
    pub fn ejections(&mut self, v: &View, weapons: &[&Weapon]) -> Vec<Ejection> {
        let mut out = Vec::new();
        for s in &self.states.states[..(self.states.count as usize).min(MAX_OBJECTS)] {
            let Some(&Kind::Weapon(weapon)) = self.kinds.get(&s.desc) else { continue };
            let was = self.chambers_seen.insert(s.id, s.chambered).unwrap_or(s.chambered);
            if s.flags & OS_ACTION_OPEN == 0 || was == s.chambered {
                continue;
            }
            let w = weapons[weapon];
            let (p, q) = Self::world_pose(s, v);
            let vel = Vec3::from_array(s.lin_vel);
            let vel = if s.flags & OS_TRACKING != 0 { v.dir(vel) } else { vel };
            let a = &w.action;
            if !a.chambers.is_empty() {
                let turn = if a.hinged() { a.hinge_matrix(s.slide) } else { Mat4::IDENTITY };
                for (k, mouth) in a.chambers.iter().enumerate() {
                    let spent = 1u32 << (k as u32 + CHAMBER_SPENT_SHIFT);
                    if was & spent != 0 && s.chambered & (spent | 1 << k) == 0 {
                        let at = p + q * turn.transform_point3(*mouth);
                        let along = q * turn.transform_vector3(Vec3::Z);
                        out.push(Ejection { weapon, gun: (p, q), vel, chamber: Some((at, along)) });
                    }
                }
            } else if matches!(a.kind, ACTION_BOLT | ACTION_PUMP) && was & (1 | 1 << CHAMBER_SPENT_SHIFT) != 0 && s.chambered & (1 | 1 << CHAMBER_SPENT_SHIFT) == 0 {
                out.push(Ejection { weapon, gun: (p, q), vel, chamber: None });
            }
        }
        out
    }

    /// The gun in a hand (dominant first): weapon index, rounds in the magazine, rounds in the chambers.
    pub fn held_gun(&self, dominant: usize) -> Option<(usize, u32, u32)> {
        let flag = |h: usize| if h == 0 { OS_HELD_LEFT } else { OS_HELD_RIGHT };
        [dominant, dominant ^ 1].into_iter().find_map(|h| {
            self.states().iter().find(|s| s.flags & flag(h) != 0).and_then(|s| match self.kinds.get(&s.desc) {
                Some(Kind::Weapon(i)) => Some((*i, s.rounds, (s.chambered & CHAMBER_LIVE_MASK).count_ones())),
                _ => None,
            })
        })
    }

    /// Has the runtime draw the rigged Metro guns and their magazines (`OBJF_RUNTIME_DRAWS`, by
    /// their model's name: `rustvr_guns::desc::model_name`) instead of the range, as a game adapter
    /// does. Before the table is registered.
    pub fn let_runtime_draw(&mut self, weapons: &[&Weapon]) {
        let count = (self.table.count as usize).min(MAX_OBJECTS);
        for d in self.table.descs[..count].iter_mut() {
            let w = match self.kinds.get(&d.id) {
                Some(Kind::Weapon(i)) | Some(Kind::Mag(i)) => weapons[*i],
                _ => continue,
            };
            if !w.from_game || !w.rigged() {
                continue;
            }
            set_name(&mut d.name, &rustvr_guns::desc::model_name("metro2033", &w.key));
            d.flags |= OBJF_RUNTIME_DRAWS;
            self.descs.insert(d.id, *d);
        }
    }

    /// Draw items for every object the range draws (not the ones the runtime does). `triggers`:
    /// each controller's trigger (a held gun's trigger follows its hand's).
    pub fn draws(&self, v: &View, weapons: &[&Weapon], triggers: [f32; 2], skip: &dyn Fn(&ObjectState) -> bool, glow_of: &dyn Fn(u32) -> f32) -> Vec<DrawItem> {
        let mut out = Vec::new();
        for s in self.states() {
            let Some(kind) = self.kinds.get(&s.desc) else { continue };
            if skip(s) {
                continue;
            }
            if self.descs.get(&s.desc).is_some_and(|d| d.flags & OBJF_RUNTIME_DRAWS != 0) || s.flags & OS_BROKEN != 0 {
                continue;
            }
            let (p, q) = Self::world_pose(s, v);
            let model = Mat4::from_rotation_translation(q, p);
            // (Hovered, or a blade filled with a spell: it glows.)
            let glow = if s.flags & OS_HOVERED != 0 { 0.25 } else { 0.0f32 }.max(glow_of(s.id));
            match *kind {
                Kind::Weapon(i) => {
                    let w = &weapons[i];
                    // The parts the runtime moves: the bolt, slide or handle riding back (a bolt
                    // turned up first, a lever swung), the barrels or cylinder open on their hinge
                    // with the rounds in the chambers, the gauge showing the pressure, and the
                    // trigger swung back under the finger pulling it.
                    let pull = match s.flags & (OS_HELD_LEFT | OS_HELD_RIGHT) {
                        OS_HELD_LEFT => triggers[0],
                        OS_HELD_RIGHT => triggers[1],
                        _ => 0.0,
                    };
                    let state = content::PartsState { slide: s.slide, chambered: s.chambered, fuse: s.fuse, trigger: pull, rounds: s.rounds };
                    for (part, m) in w.part_matrices(&state).into_iter().enumerate() {
                        if let Some(m) = m {
                            out.push((part_id(i, part), model * m, [1.0; 4], glow, w.from_game));
                        }
                    }
                }
                Kind::Mag(i) => {
                    let w = &weapons[i];
                    if !w.round_parts().is_empty() {
                        for &part in w.round_parts() {
                            out.push((part_id(i, part), model * w.fix, [1.0; 4], glow, w.from_game));
                        }
                    } else if s.flags & OS_IN_WEAPON == 0 {
                        // The game's model already has its magazine: only loose ones are drawn.
                        let d = &self.descs[&s.desc];
                        let m = model * Mat4::from_translation(Vec3::from_array(d.center)) * Mat4::from_scale(Vec3::from_array(d.half_extents) * 2.0);
                        out.push((200, m, [0.3, 0.27, 0.22, 1.0], glow, false));
                    }
                }
                Kind::Prop { color } => {
                    let d = &self.descs[&s.desc];
                    let m = model * Mat4::from_translation(Vec3::from_array(d.center)) * Mat4::from_scale(Vec3::from_array(d.half_extents) * 2.0);
                    out.push((200, m, color, glow, false));
                }
                Kind::Valve(k) => out.push((600 + k, model, [1.0; 4], glow, false)),
                Kind::Throwable(_) => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtins() -> Vec<Weapon> {
        vec![content::builtin_rifle("Range rifle", false), content::builtin_rifle("Range rifle (scoped)", true), content::builtin_pistol()]
    }

    /// Everything the range loads: Metro's guns (when extracted) and the built-in ones.
    fn everything() -> Vec<Weapon> {
        let mut w = content::metro_weapons(&rustvr_asset::default_root());
        w.extend(builtins());
        content::share_game_hands(&mut w);
        w
    }

    fn weapon_descs(a: &Armory) -> Vec<ObjectDesc> {
        a.table.descs[..a.table.count as usize].iter().filter(|d| d.kind == OBJ_WEAPON).copied().collect()
    }

    #[test]
    fn the_armory_registers_guns_mags_and_props() {
        let weapons = builtins();
        let a = Armory::new(&weapons.iter().collect::<Vec<_>>());
        let descs = &a.table.descs[..a.table.count as usize];
        assert_eq!(descs.iter().filter(|d| d.kind == OBJ_WEAPON).count(), 3);
        assert_eq!(descs.iter().filter(|d| d.flags & OBJF_TEMPLATE != 0).count(), 3, "a magazine template per gun");
        let pistol = descs.iter().find(|d| d.kind == OBJ_WEAPON && d.weapon_flags & WPN_PISTOL != 0).unwrap();
        assert_eq!(pistol.start_slot, SLOT_HIP_RIGHT + 1, "the pistol starts on the hip");
        assert!(descs.iter().any(|d| d.kind == OBJ_PROP && d.flags & OBJF_PULLABLE == 0), "the barrel is too heavy to pull");
        // Rifles rest on the near table: their lowest point is just above its top.
        let rifle = descs.iter().find(|d| d.id == WEAPON_BASE).unwrap();
        let rot = Quat::from_array(rifle.spawn_pose.rot);
        let low = Vec3::from_array(rifle.spawn_pose.pos).y + (rot * Vec3::from_array(rifle.center)).y - rifle.half_extents[0];
        assert!((low - content::TABLE_NEAR.1.y).abs() < 0.02, "rests on the table: {low}");
        assert!(a.collision.len() > 10);
    }

    #[test]
    fn guns_get_metros_sound_sets_when_its_sounds_are_there() {
        let weapons = builtins();
        let guns: Vec<(u32, &Weapon)> = weapons.iter().enumerate().map(|(i, w)| (WEAPON_BASE + i as u32, w)).collect();
        let assets = std::env::temp_dir().join(format!("rustvr-range-sounds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&assets);
        assert!(sound_pack(&assets, &guns).path().is_empty(), "nothing extracted: no pack");
        std::fs::create_dir_all(assets.join("metro2033").join("sounds")).unwrap();
        std::fs::write(assets.join("metro2033").join("sounds").join("sounds.toml"), "format = 1\n").unwrap();
        let pack = sound_pack(&assets, &guns);
        assert!(pack.path().ends_with("sounds") && pack.flags == 0, "{} {:#x}", pack.path(), pack.flags);
        let set = |key: &str| {
            let (id, _) = guns.iter().find(|(_, w)| w.key == key).unwrap();
            pack.object(*id).unwrap().set_name().to_owned()
        };
        assert_eq!((set("builtin_rifle").as_str(), set("builtin_pistol").as_str()), ("wpn_ak", "wpn_revolver"));
        let _ = std::fs::remove_dir_all(&assets);
    }

    #[test]
    fn throwables_get_their_sound_sets_only_with_a_pack() {
        let weapons = builtins();
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let mut a = Armory::new(&refs);
        *a.sounds = SoundPack::zeroed();
        a.add_sound_sets(&[(3000, "grenade")]);
        assert_eq!(a.sounds.count, 0, "no pack, no sets");
        *a.sounds = SoundPack::new("C:\\metro2033\\sounds");
        a.add_sound_sets(&[(3000, "grenade"), (3002, "throw_knife")]);
        assert_eq!(a.sounds.object(3000).unwrap().set_name(), "grenade");
        assert_eq!(a.sounds.object(3002).unwrap().set_name(), "throw_knife");
    }

    #[test]
    fn only_rigged_guns_are_laid_out() {
        let mut weapons = everything();
        // A gun without authored hands (like Metro's others) stays out.
        let mut unrigged = content::builtin_rifle("Unrigged", false);
        unrigged.grip_rot = None;
        weapons.push(unrigged);
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let a = Armory::new(&refs);
        let laid: Vec<&Weapon> = weapon_descs(&a).iter().map(|d| refs[(d.id - WEAPON_BASE) as usize]).collect();
        assert!(laid.iter().all(|w| w.rigged()), "only rigged guns");
        assert_eq!(laid.len(), weapons.iter().filter(|w| w.rigged()).count(), "every rigged gun");
        let keys: Vec<&str> = laid.iter().map(|w| w.key.as_str()).collect();
        for k in ["builtin_rifle", "builtin_rifle_scoped", "builtin_pistol"] {
            assert!(keys.contains(&k), "{k} in {keys:?}");
        }
        if weapons.iter().any(|w| w.from_game) {
            assert!(keys.contains(&"wpn_ak") && keys.contains(&"wpn_ubludok"), "{keys:?}");
        }
        // Each kind of magazine the laid-out guns take has one template, and nothing else is
        // registered.
        let templates: Vec<u32> = a.table.descs[..a.table.count as usize].iter().filter(|d| d.flags & OBJF_TEMPLATE != 0).map(|d| d.ammo_type).collect();
        let mut kinds: Vec<u32> = weapon_descs(&a).iter().filter(|d| !refs[(d.id - WEAPON_BASE) as usize].round_parts().is_empty()).map(|d| d.ammo_type).collect();
        kinds.sort();
        kinds.dedup();
        assert_eq!(templates.len(), kinds.len(), "one template per magazine: {templates:x?} for {kinds:x?}");
        assert!(kinds.iter().all(|k| templates.contains(k)), "every gun's magazine has its template");
    }

    #[test]
    fn the_tables_hold_the_guns_apart() {
        let weapons = everything();
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let a = Armory::new(&refs);
        let mut boxes = Vec::new();
        for d in weapon_descs(&a).iter().filter(|d| d.start_slot == 0) {
            let w = refs[(d.id - WEAPON_BASE) as usize];
            let rot = Quat::from_array(d.spawn_pose.rot);
            let pos = Vec3::from_array(d.spawn_pose.pos);
            let world = |(lo, hi): (Vec3, Vec3)| {
                let c = (0..8).map(|k| pos + rot * Vec3::new([lo.x, hi.x][k & 1], [lo.y, hi.y][(k >> 1) & 1], [lo.z, hi.z][k >> 2]));
                bounds_of(c).unwrap()
            };
            let (lo, hi) = world(drawn_bounds(w));
            let (clo, _) = world(weapon_bounds(w));
            // On the near table or the armory bench, whichever it's over.
            let (tmin, tmax) = [content::TABLE_NEAR, content::TABLE_ARMORY].into_iter().find(|(tmin, tmax)| pos.z > tmin.z - 1.0 && pos.z < tmax.z + 1.0 && pos.x > tmin.x - 1.0 && pos.x < tmax.x + 1.0).unwrap_or_else(|| panic!("{}: on no table at {pos}", w.key));
            assert!((clo.y - tmax.y - 0.01).abs() < 1e-3, "{}: rests on the table ({})", w.key, clo.y);
            assert!(lo.z >= tmin.z && hi.z <= tmax.z, "{}: {lo}..{hi} off the table's depth", w.key);
            assert!(lo.x >= tmin.x - 0.03 && hi.x <= tmax.x + 0.03, "{}: {lo}..{hi} off the table's width", w.key);
            assert!(lo.y >= tmax.y - 0.001, "{}: drawn into the table ({lo})", w.key);
            boxes.push((w.key.clone(), lo, hi));
        }
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let ((a, alo, ahi), (b, blo, bhi)) = (&boxes[i], &boxes[j]);
                let apart = ahi.x <= blo.x || bhi.x <= alo.x || ahi.z <= blo.z || bhi.z <= alo.z;
                assert!(apart, "{a} {alo}..{ahi} overlaps {b} {blo}..{bhi}");
            }
        }
        // The scenarios reach for the plain rifle: it's the nearest gun on the near table.
        let (nmin, nmax) = content::TABLE_NEAR;
        let nearest = boxes.iter().filter(|b| b.1.x >= nmin.x - 0.03 && b.2.x <= nmax.x + 0.03 && b.1.z >= nmin.z && b.2.z <= nmax.z).min_by(|a, b| a.1.z.total_cmp(&b.1.z)).unwrap();
        assert_eq!(nearest.0, "builtin_rifle");
        // Metro's other guns are on the bench, its Kalash (the scenarios' game rifle) on the near table.
        if weapons.iter().any(|w| w.from_game) {
            let on_near = |key: &str| boxes.iter().any(|b| b.0 == key && b.1.z >= nmin.z && b.2.z <= nmax.z && b.1.x >= nmin.x - 0.03);
            assert!(on_near("wpn_ak"));
            let (bmin, bmax) = content::TABLE_ARMORY;
            let on_bench = boxes.iter().filter(|b| b.1.z >= bmin.z && b.2.z <= bmax.z).count();
            assert_eq!(on_bench + 5, weapons.iter().filter(|w| w.rigged()).count(), "every other rigged gun is on the bench");
        }
    }

    #[test]
    fn descriptors_carry_the_recoil_feel_and_the_game_hands() {
        let weapons = everything();
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let a = Armory::new(&refs);
        let with_game = weapons.iter().any(|w| w.from_game);
        for d in weapon_descs(&a) {
            let w = refs[(d.id - WEAPON_BASE) as usize];
            let want = match w.key.as_str() {
                "wpn_ak" => [0.65, 8.0, 14.0],
                "wpn_ubludok" => [0.7, 7.5, 18.0],
                "builtin_pistol" => [0.4, 14.0, 8.0],
                "builtin_rifle" | "builtin_rifle_scoped" => [0.6, 9.0, 12.0],
                // (The other rigs' own, from content/rigs/.)
                _ => w.recoil_feel,
            };
            assert_eq!([d.recoil_climb, d.recoil_return, d.recoil_angle], want, "{}", w.key);
            // Every rigged gun has Metro's hands when Metro's there (the built-in ones the Kalash's).
            assert_eq!(d.grip_pose != 0, with_game, "{}: firing pose", w.key);
            assert_eq!(d.support_pose != 0, with_game && !w.pistol, "{}: support pose", w.key);
            if let Some(p) = a.poses.find(d.grip_pose) {
                assert_eq!(p.side as usize, HAND_RIGHT);
                assert!(p.flags & HPOSE_TRIGGER != 0, "{}: the index curls with the trigger", w.key);
                assert!((0.07..0.13).contains(&p.size));
                // The wrist is behind the grip and to its right, the fingers forward.
                let (at, rot) = (Vec3::from_array(p.wrist.pos), Quat::from_array(p.wrist.rot));
                let grip = Vec3::from_array(d.grip);
                assert!(at.z < grip.z && at.x > grip.x - 0.01, "{}: wrist {at}", w.key);
                assert!((rot * Vec3::Y).z > 0.5, "{}: fingers {}", w.key, rot * Vec3::Y);
            }
            if let Some(p) = a.poses.find(d.support_pose) {
                assert_eq!(p.side as usize, HAND_LEFT);
                assert_eq!(p.flags & HPOSE_TRIGGER, 0);
                // Under the foregrip, near it.
                let at = Vec3::from_array(p.wrist.pos);
                assert!(at.distance(Vec3::from_array(d.foregrip)) < 0.12, "{}: support wrist {at}", w.key);
            }
        }
        let ids: Vec<u32> = a.poses.poses[..a.poses.count as usize].iter().map(|p| p.id).collect();
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "pose ids are unique");
        assert!(ids.iter().all(|&id| id != 0));
    }

    /// The pouch hands each gun a magazine of its kind (the template its ammo type names), and
    /// seated it sits where the gun's own magazine does: the magazine drawn in the gun covers the
    /// gun's own (a shared one, the Kalash's in the AKSU, within a few millimetres).
    #[test]
    fn every_gun_gets_a_magazine_that_fits_it() {
        let weapons = everything();
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let a = Armory::new(&refs);
        let mag_bounds = |w: &Weapon| rustvr_guns::desc::bounds_where(w, |i| w.mag_parts.contains(&i));
        for d in weapon_descs(&a) {
            let i = (d.id - WEAPON_BASE) as usize;
            let w = refs[i];
            if w.round_parts().is_empty() {
                continue;
            }
            let t = a.descs.values().find(|m| m.flags & OBJF_TEMPLATE != 0 && m.ammo_type == d.ammo_type).unwrap_or_else(|| panic!("{}: no magazine", w.key));
            if w.round_base().is_some() {
                // Loaded round by round: the pouch hands one round, its base at the first
                // chamber's mouth (or the magazine's port); the gun has a chamber (or a place in
                // its magazine) per round.
                assert_eq!((t.capacity, t.rounds), (1, 1), "{}: one round", w.key);
                assert_eq!(t.mag_well, d.mag_well, "{}", w.key);
                let (slots, flag) = if w.action.chambers.is_empty() { (w.action.round_parts.len(), WPN_LOADS_ROUNDS) } else { (w.action.chambers.len(), WPN_LOADS_CHAMBERS) };
                assert_eq!(d.capacity as usize, slots, "{}", w.key);
                assert!(d.weapon_flags & flag != 0, "{}", w.key);
                continue;
            }
            let Some(Kind::Mag(j)) = a.kinds.get(&t.id) else { panic!("{}: its magazine draws as nothing", w.key) };
            let from = refs[*j];
            assert_eq!(from.magazine, w.magazine, "{}: takes {}'s magazine", w.key, from.key);
            // Seated, the magazine object sits at the gun's pose moved by (gun well - its well), in
            // the gun's weapon space (the runtime's `Holder::InWeapon`), and draws `from`'s parts.
            let shift = Vec3::from_array(d.mag_well) - Vec3::from_array(t.mag_well);
            let (lo, hi) = mag_bounds(from).unwrap();
            let (own_lo, own_hi) = mag_bounds(w).unwrap();
            let off = ((lo + hi) * 0.5 + shift).distance((own_lo + own_hi) * 0.5);
            let size = ((hi - lo) - (own_hi - own_lo)).abs().max_element();
            assert!(off < 0.005 && size < 0.005, "{}: {}'s magazine sits {:.1} mm off its own, {:.1} mm different", w.key, from.key, off * 1000.0, size * 1000.0);
            assert_eq!(t.capacity, w.mag_size, "{}: its magazine holds its rounds", w.key);
        }
    }

    #[test]
    fn every_gun_keeps_its_own_recoil() {
        let weapons = everything();
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let a = Armory::new(&refs);
        let table = &a.kicks.kicks[..a.kicks.count as usize];
        assert!(table.len() <= MAX_KICKS);
        for d in weapon_descs(&a) {
            let w = refs[(d.id - WEAPON_BASE) as usize];
            let ours: Vec<&KickDesc> = table.iter().filter(|k| k.object == d.id).collect();
            let kinds = |ks: &[KickDesc]| {
                let mut k: Vec<(u32, u32)> = ks.iter().map(|k| (k.layer, k.when)).collect();
                k.sort();
                k.dedup();
                k
            };
            let theirs = kick_descs(d.id, w);
            // Every kind of kick the game gives the gun is there, at least once.
            let have: Vec<KickDesc> = ours.iter().map(|k| **k).collect();
            assert_eq!(kinds(&have), kinds(&theirs), "{}: its recoil for every way of firing", w.key);
            // The near table's guns (the scenarios fire them) keep every variant.
            if matches!(w.key.as_str(), "wpn_ak" | "wpn_ubludok") {
                assert_eq!(ours.len(), theirs.len(), "{}: all its kicks", w.key);
            }
        }
    }

    #[test]
    fn descriptors_carry_the_authored_rig() {
        let weapons = everything();
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let a = Armory::new(&refs);
        for d in weapon_descs(&a) {
            let w = refs[(d.id - WEAPON_BASE) as usize];
            let q = Quat::from_array(d.grip_rot);
            assert!((q.length() - 1.0).abs() < 1e-4, "{}: grip_rot {:?}", w.key, d.grip_rot);
            assert!((q * Vec3::Z).dot(w.grip_rot.unwrap() * Vec3::Z) > 0.9999);
            assert_eq!([d.recoil_pitch, d.recoil_yaw, d.recoil_back], w.recoil, "{}", w.key);
            if w.pistol {
                assert!((10.0..=14.0).contains(&d.recoil_pitch) && (0.025..=0.035).contains(&d.recoil_back), "{}", w.key);
                assert!(d.flags & OBJF_TWO_HANDED == 0 && d.weapon_flags & WPN_PISTOL != 0, "{}", w.key);
            } else {
                assert!(d.flags & OBJF_TWO_HANDED != 0);
                assert!((Quat::from_array(d.support_rot).length() - 1.0).abs() < 1e-4, "{}: support_rot", w.key);
                assert!((1.0..=14.0).contains(&d.recoil_pitch) && (0.3..=4.0).contains(&d.recoil_yaw), "{}", w.key);
                assert!((0.003..=0.06).contains(&d.recoil_back), "{}", w.key);
            }
            // A gun with something to rack needs racking after a fresh magazine; one without
            // (a revolver, a break action) or whose handle is an air pump chambers as the magazine
            // goes in.
            assert_eq!(d.weapon_flags & WPN_NEEDS_RACK != 0, w.handle.is_some() && w.action.air.is_none(), "{}", w.key);
            if w.handle.is_some() {
                assert!(d.handle_travel >= 0.015, "{}: handle travel {}", w.key, d.handle_travel);
            }
            // How it's worked: a hinged action's hinge is where `handle` points, with nothing to
            // pull; a bolt lifts; an air gun's pressures are its rig's.
            match d.action {
                ACTION_BREAK | ACTION_SWING => {
                    assert!(w.handle.is_none() && d.handle_travel == 0.0 && d.action_angle.abs() > 1.0, "{}: {:?}", w.key, w.action);
                    assert_eq!(Vec3::from_array(d.handle), w.action.hinge, "{}", w.key);
                }
                ACTION_BOLT => assert!(d.action_angle > 0.5 && d.weapon_flags & WPN_NEEDS_RACK != 0, "{}", w.key),
                _ => {}
            }
            if let Some([pump, shot, min]) = w.action.air {
                assert!(d.weapon_flags & WPN_PNEUMATIC != 0 && [d.pump_pressure, d.shot_pressure, d.min_pressure] == [pump, shot, min], "{}", w.key);
            }
            // The magazine template's box is the magazine (a box, a drum, a speed loader, a pair
            // of shells), a small part of the gun. (A gun without one holds its rounds.)
            if w.round_parts().is_empty() {
                assert!(w.from_game && a.descs.values().all(|m| m.flags & OBJF_TEMPLATE == 0 || m.ammo_type != d.ammo_type), "{}", w.key);
                continue;
            }
            let m = *a.descs.values().find(|m| m.flags & OBJF_TEMPLATE != 0 && m.ammo_type == d.ammo_type).unwrap_or_else(|| panic!("{}: no magazine", w.key));
            let (mh, gh) = (Vec3::from_array(m.half_extents), Vec3::from_array(d.half_extents));
            assert!(mh.min_element() > 0.004 && mh.max_element() < 0.25 && mh.z < gh.z * 0.75, "{}: magazine box {mh}", w.key);
            // (A round alone: a dart 30 g, a shell 45 g.)
            assert!(m.mass >= 0.02 && m.mass < 3.0, "{}: magazine {} kg", w.key, m.mass);
        }
    }
}
