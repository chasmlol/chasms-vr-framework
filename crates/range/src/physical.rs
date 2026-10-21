//! Physical mode: the range's guns, magazines and props are objects the runtime's hands pick up,
//! holster, reload and fire. The range registers them (with its static collision, and the game's
//! own hand poses on the guns: Metro's first-person arms, see `content::GunHand`), then each frame
//! draws them where the runtime says they are, the trigger of a held gun following its hand's, and
//! turns their shot counters into bullets.
//!
//! The range makes no sound itself: it hands the runtime Metro's own sounds (the sound pack
//! `metro-extract` writes to `local_assets/metro2033/sounds`, see [`sound_pack`]), each gun the
//! set it sounds like, and the runtime plays them, its shots included, as it would for any game
//! that doesn't play its own.

use bytemuck::Zeroable;
use glam::{Mat3, Mat4, Quat, Vec3};
use rustvr_adapter::View;
use rustvr_protocol::*;
use std::collections::HashMap;

use crate::content::{self, GunHand, Weapon};

/// Desc ids: weapon `i` is `WEAPON_BASE + i`, its magazine template `MAG_BASE + i`.
pub const WEAPON_BASE: u32 = 1;
pub const MAG_BASE: u32 = 1000;
pub const PROP_BASE: u32 = 2000;

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
}

/// A bullet to fire: world origin and direction.
pub struct Shot {
    pub origin: Vec3,
    pub dir: Vec3,
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
    collision: Vec<ColBox>,
    /// How many of `collision` the runtime has (the ring can fill up mid-way).
    sent: usize,
    pub registered: bool,
}

/// What a draw list needs: (mesh id, model, tint, emissive, two-sided).
pub type DrawItem = (usize, Mat4, [f32; 4], f32, bool);

/// Mesh ids for weapon parts in the physical draw list (see `to_draws`).
pub fn part_id(weapon: usize, part: usize) -> usize {
    10_000 + weapon * 100 + part
}

fn bounds_of(points: impl Iterator<Item = Vec3>) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for p in points {
        min = min.min(p);
        max = max.max(p);
    }
    (min.x <= max.x).then_some((min, max))
}

/// Weapon-space bounds of some of a gun's parts.
fn parts_bounds(w: &Weapon, keep: impl Fn(usize) -> bool) -> Option<(Vec3, Vec3)> {
    let pts = w.parts.iter().enumerate().filter(|(i, _)| keep(*i)).flat_map(|(_, p)| p.vertices.iter().map(|v| w.fix.transform_point3(Vec3::from_array(v.pos))));
    bounds_of(pts)
}

/// Weapon-space bounds of a gun without its magazine (its collider).
fn weapon_bounds(w: &Weapon) -> (Vec3, Vec3) {
    parts_bounds(w, |i| !w.mag_parts.contains(&i)).unwrap_or((Vec3::new(-0.03, -0.1, -0.3), Vec3::new(0.03, 0.08, 0.6)))
}

/// Weapon-space bounds of a gun with its magazine in (what it covers lying on a table).
fn drawn_bounds(w: &Weapon) -> (Vec3, Vec3) {
    parts_bounds(w, |_| true).unwrap_or_else(|| weapon_bounds(w))
}

/// Weapon-space bounds of a gun's magazine.
fn mag_bounds(w: &Weapon) -> Option<(Vec3, Vec3)> {
    parts_bounds(w, |i| w.mag_parts.contains(&i))
}

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

pub fn weapon_desc(i: usize, w: &Weapon, spawn: Pose, start_slot: u32) -> ObjectDesc {
    let (min, max) = weapon_bounds(w);
    let mut d = ObjectDesc::zeroed();
    d.id = WEAPON_BASE + i as u32;
    d.kind = OBJ_WEAPON;
    d.flags = OBJF_GRABBABLE
        | OBJF_PULLABLE
        | OBJF_HOLSTERABLE
        | if w.pistol { 0 } else { OBJF_TWO_HANDED }
        | if w.ads_zoom > 1.4 { OBJF_SCOPED } else { 0 };
    d.spawn_seq = 1;
    d.spawn_pose = spawn;
    d.start_slot = start_slot;
    d.shape = SHAPE_BOX;
    d.mass = w.mass;
    d.half_extents = ((max - min) * 0.5).to_array();
    d.center = ((max + min) * 0.5).to_array();
    d.grip = w.rig.grip;
    d.foregrip = w.rig.foregrip;
    d.mag_well = w.rig.mag_well;
    d.muzzle = w.rig.muzzle;
    d.sight = w.rig.sight;
    if let Some((h, travel)) = w.handle {
        d.handle = h.to_array();
        d.handle_travel = travel;
    }
    d.ammo_type = i as u32 + 1;
    d.capacity = w.mag_size;
    d.weapon_flags = if w.automatic { WPN_AUTOMATIC } else { 0 }
        | if w.slide_locks { WPN_SLIDE_LOCKS } else { 0 }
        | if w.handle.is_some() { WPN_NEEDS_RACK } else { 0 }
        | if w.pistol { WPN_PISTOL } else { WPN_STOCK };
    d.fire_rate = w.rounds_per_second;
    [d.recoil_pitch, d.recoil_yaw, d.recoil_back] = w.recoil;
    [d.recoil_climb, d.recoil_return, d.recoil_angle] = w.recoil_feel;
    // How the hands hold it (all zero: the runtime's fallbacks), and the game's own hand poses.
    d.grip_rot = w.grip_rot.map(|q| q.normalize().to_array()).unwrap_or([0.0; 4]);
    if w.grip_rot.is_some() && w.game_hands[1].is_some() {
        d.grip_pose = grip_pose_id(i);
    }
    if !w.pistol {
        d.support_rot = w.support_rot.map(|q| q.normalize().to_array()).unwrap_or([0.0; 4]);
        if w.support_rot.is_some() && w.game_hands[0].is_some() {
            d.support_pose = support_pose_id(i);
        }
    }
    set_name(&mut d.name, &w.name);
    d
}

/// A gun's own recoil as kick table entries: its kicks taken from the model's space into the
/// weapon's (`Weapon::fix`), rotations and translations alike (the channels at each one's key times,
/// so the turn is exact).
pub fn kick_descs(object: u32, w: &Weapon) -> Vec<KickDesc> {
    use rustvr_asset::{KICK_CHANNELS as N, KickKey, kick_slope, kick_value};
    let (_, rot, _) = w.fix.to_scale_rotation_translation();
    // A rotation as a rotation vector, so it turns like a translation: (-pitch, yaw, roll).
    let turn = |keys: [&[KickKey]; 3], sign: [f32; 3]| -> [Vec<KickKey>; 3] {
        let mut times: Vec<f32> = keys.iter().flat_map(|k| k.iter().map(|k| k.t)).collect();
        times.sort_by(f32::total_cmp);
        times.dedup();
        let at = |f: &dyn Fn(&[KickKey]) -> f32| Vec3::from_array([0, 1, 2].map(|c| f(keys[c]) * sign[c]));
        let mut out: [Vec<KickKey>; 3] = Default::default();
        for t in times {
            let (v, si, so) = (rot * at(&|k| kick_value(k, t)), rot * at(&|k| kick_slope(k, t, false)), rot * at(&|k| kick_slope(k, t, true)));
            for c in 0..3 {
                out[c].push(KickKey { t, v: v[c] * sign[c], slope_in: si[c] * sign[c], slope_out: so[c] * sign[c] });
            }
        }
        out
    };
    let mut out = Vec::new();
    for k in &w.kicks {
        let ch = &k.channels;
        let [p, y, r] = turn([&ch[0], &ch[1], &ch[2]], [-1.0, 1.0, 1.0]);
        let [x, yy, z] = turn([&ch[3], &ch[4], &ch[5]], [1.0, 1.0, 1.0]);
        let channels: [Vec<KickKey>; N] = [p, y, r, x, yy, z];
        let mut d = KickDesc::zeroed();
        d.object = object;
        d.layer = k.layer as u32;
        d.when = k.when;
        d.speed = k.speed;
        d.length = k.length;
        d.pivot = w.fix.transform_point3(Vec3::from_array(k.pivot)).to_array();
        for (c, keys) in channels.iter().enumerate() {
            if keys.len() > KICK_KEYS {
                tracing::warn!("range: {} kick {}: {} keys, keeping {KICK_KEYS}", w.key, k.name, keys.len());
            }
            let curve = &mut d.channels[c];
            curve.count = keys.len().min(KICK_KEYS) as u32;
            for (i, key) in keys.iter().take(KICK_KEYS).enumerate() {
                curve.keys[i] = [key.t, key.v, key.slope_in, key.slope_out];
            }
        }
        out.push(d);
    }
    out
}

/// A hand pose table entry for one of the game's hands on a gun.
pub fn pose_desc(id: u32, side: usize, h: &GunHand) -> HandPoseDesc {
    let mut p = HandPoseDesc::zeroed();
    p.id = id;
    p.side = side as u32;
    p.size = h.size;
    p.wrist = Pose { pos: h.wrist.0.to_array(), valid: 1, rot: h.wrist.1.normalize().to_array() };
    p.fingers = h.fingers.map(|q| q.normalize().to_array());
    if let Some(pulled) = h.index_pulled {
        p.flags |= HPOSE_TRIGGER;
        p.index_pulled = pulled.map(|q| q.normalize().to_array());
    }
    let point = |r: Option<(Vec3, f32)>| r.map_or([0.0; 4], |(at, along)| [at.x, at.y, at.z, along]);
    p.reach = h.reach.map(point);
    p.index_reach_pulled = point(h.index_reach_pulled);
    p
}

/// Magazine space is the gun's own space: seated, a magazine sits exactly where the gun's model
/// has it, so its part draws in place. Its collider is the magazine mesh's box.
fn mag_desc(i: usize, w: &Weapon) -> ObjectDesc {
    let (min, max) = mag_bounds(w).unwrap_or((w.mag_center - Vec3::new(0.016, 0.07, 0.03), w.mag_center + Vec3::new(0.016, 0.07, 0.03)));
    let mut d = ObjectDesc::zeroed();
    d.id = MAG_BASE + i as u32;
    d.kind = OBJ_MAGAZINE;
    d.flags = OBJF_GRABBABLE | OBJF_PULLABLE | OBJF_TEMPLATE;
    d.shape = SHAPE_BOX;
    d.mass = w.mag_mass;
    d.half_extents = ((max - min) * 0.5).to_array();
    d.center = ((max + min) * 0.5).to_array();
    d.mag_well = w.rig.mag_well;
    d.ammo_type = i as u32 + 1;
    d.capacity = w.mag_size;
    d.rounds = w.mag_size;
    set_name(&mut d.name, &format!("{} magazine", w.name));
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
/// then Metro's rigged guns, then the scoped rifle.
fn table_rank(w: &Weapon) -> u32 {
    match w.key.as_str() {
        "builtin_rifle" => 0,
        "wpn_ak" => 1,
        "wpn_ubludok" => 2,
        "builtin_rifle_scoped" => 4,
        _ => 3,
    }
}

impl Armory {
    /// Lays out the range: the properly rigged guns (see [`Weapon::rigged`]) on the table at your
    /// right hand (the plain rifle nearest) and the pistol on your hip; things to throw around on
    /// the long table ahead and the floor. Guns that aren't rigged stay out (legacy mode only).
    pub fn new(weapons: &[&Weapon]) -> Armory {
        let mut table: Box<ObjectTable> = bytemuck::zeroed_box();
        let mut poses: Box<HandPoseTable> = bytemuck::zeroed_box();
        let mut kicks: Box<KickTable> = bytemuck::zeroed_box();
        let mut kick_list = Vec::new();
        let mut guns = Vec::new();
        let mut kinds = HashMap::new();
        let mut descs = Vec::new();
        let mut pose_descs = Vec::new();
        let long_top = content::TABLE_LONG.1.y;
        let mut on_table: Vec<usize> = (0..weapons.len()).filter(|&i| weapons[i].rigged() && !weapons[i].pistol).collect();
        on_table.sort_by_key(|&i| table_rank(weapons[i]));
        let mut places: HashMap<usize, (Pose, u32)> = table_layout(weapons, &on_table).into_iter().map(|(i, p)| (i, (p, 0))).collect();
        let mut hip_taken = false;
        let mut spare_x = 1.3f32;
        for (i, w) in weapons.iter().enumerate().filter(|(_, w)| w.rigged() && w.pistol) {
            // The first pistol starts in the right hip holster; its spawn pose (and any other
            // pistol's place) is on the right end of the long table.
            let (pose, _) = lying(w, spare_x, content::TABLE_LONG.0.z + 0.2, long_top);
            spare_x += 0.3;
            places.insert(i, (pose, if hip_taken { 0 } else { SLOT_HIP_RIGHT + 1 }));
            hip_taken = true;
        }
        for (i, w) in weapons.iter().enumerate() {
            let Some(&(spawn, slot)) = places.get(&i) else { continue };
            let d = weapon_desc(i, w, spawn, slot);
            for (id, side) in [(d.grip_pose, HAND_RIGHT), (d.support_pose, HAND_LEFT)] {
                if let (true, Some(h)) = (id != 0, &w.game_hands[side]) {
                    pose_descs.push(pose_desc(id, side, h));
                }
            }
            kick_list.extend(kick_descs(d.id, w));
            guns.push((d.id, *w));
            descs.push(d);
            descs.push(mag_desc(i, w));
            kinds.insert(WEAPON_BASE + i as u32, Kind::Weapon(i));
            kinds.insert(MAG_BASE + i as u32, Kind::Mag(i));
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
            descs.push(prop(id, name, at, half, mass));
            kinds.insert(id, Kind::Prop { color });
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
        if kick_list.len() > MAX_KICKS {
            tracing::warn!("range: {} kicks, the table holds {MAX_KICKS}", kick_list.len());
        }
        for (k, d) in kick_list.iter().take(MAX_KICKS).enumerate() {
            kicks.kicks[k] = *d;
        }
        kicks.count = kick_list.len().min(MAX_KICKS) as u32;
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
        Armory {
            table,
            kicks,
            poses,
            sounds,
            kinds,
            descs: descs.iter().map(|d| (d.id, *d)).collect(),
            states: bytemuck::zeroed_box(),
            shots_seen: HashMap::new(),
            collision,
            sent: 0,
            registered: false,
        }
    }

    /// (Re-)registers everything with a newly connected runtime.
    pub fn register(&mut self, adapter: &rustvr_adapter::Adapter) {
        if !self.registered {
            self.table.seq = self.table.seq.wrapping_add(1).max(1);
            // The poses first: when the runtime sees the objects, their poses are there.
            self.poses.seq = self.poses.seq.wrapping_add(1).max(1);
            self.kicks.seq = self.kicks.seq.wrapping_add(1).max(1);
            let written = adapter.write_hand_poses(&self.poses) && adapter.write_kicks(&self.kicks) && adapter.write_objects(&self.table);
            if written && adapter.set_sound_pack(&self.sounds) {
                self.registered = true;
                self.shots_seen.clear();
                tracing::info!("range: registered {} objects, {} hand poses and {} recoil kicks", self.table.count, self.poses.count, self.kicks.count);
                match self.sounds.path() {
                    "" => tracing::info!("range: no sound pack (run metro-extract): the runtime's own sounds"),
                    dir => tracing::info!("range: Metro's sounds from {dir}, {} guns with their own", self.sounds.count),
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
        self.states.count = 0;
    }

    /// The object states, read fresh.
    pub fn update(&mut self, adapter: &rustvr_adapter::Adapter) -> bool {
        adapter.read_object_states(&mut self.states)
    }

    fn states(&self) -> &[ObjectState] {
        &self.states.states[..(self.states.count as usize).min(MAX_OBJECTS)]
    }

    fn world_pose(s: &ObjectState, v: &View) -> (Vec3, Quat) {
        let p = Vec3::from_array(s.pose.pos);
        let q = Quat::from_array(s.pose.rot).normalize();
        if s.flags & OS_TRACKING != 0 { (v.point(p), v.rot(q)) } else { (p, q) }
    }

    /// New shots fired by the runtime since the last call, as world rays along the line of sight.
    pub fn shots(&mut self, v: &View) -> Vec<Shot> {
        let mut out = Vec::new();
        for s in &self.states.states[..(self.states.count as usize).min(MAX_OBJECTS)] {
            let Some(Kind::Weapon(_)) = self.kinds.get(&s.desc) else { continue };
            let seen = self.shots_seen.entry(s.id).or_insert(s.shots);
            let new = s.shots.wrapping_sub(*seen).min(4);
            *seen = s.shots;
            if new == 0 {
                continue;
            }
            let (p, q) = Self::world_pose(s, v);
            let d = &self.descs[&s.desc];
            for _ in 0..new {
                out.push(Shot { origin: p + q * Vec3::from_array(d.sight), dir: q * Vec3::Z });
            }
        }
        out
    }

    /// The gun in a hand (dominant first): weapon index, rounds in the magazine, chambered.
    pub fn held_gun(&self, dominant: usize) -> Option<(usize, u32, bool)> {
        let flag = |h: usize| if h == 0 { OS_HELD_LEFT } else { OS_HELD_RIGHT };
        [dominant, dominant ^ 1].into_iter().find_map(|h| {
            self.states().iter().find(|s| s.flags & flag(h) != 0).and_then(|s| match self.kinds.get(&s.desc) {
                Some(Kind::Weapon(i)) => Some((*i, s.rounds, s.chambered != 0)),
                _ => None,
            })
        })
    }

    /// Draw items for every object. `triggers`: each controller's trigger (a held gun's trigger
    /// follows its hand's).
    pub fn draws(&self, v: &View, weapons: &[&Weapon], triggers: [f32; 2]) -> Vec<DrawItem> {
        let mut out = Vec::new();
        for s in self.states() {
            let Some(kind) = self.kinds.get(&s.desc) else { continue };
            let (p, q) = Self::world_pose(s, v);
            let model = Mat4::from_rotation_translation(q, p);
            let glow = if s.flags & OS_HOVERED != 0 { 0.25 } else { 0.0 };
            match *kind {
                Kind::Weapon(i) => {
                    let w = &weapons[i];
                    // The bolt, slide or charging handle rides back as the runtime racks it.
                    let slide = if s.slide.is_finite() { s.slide.clamp(0.0, 1.0) } else { 0.0 };
                    let back = w.handle.map(|(_, travel)| travel).unwrap_or(0.0) * slide;
                    let bolt = model * Mat4::from_translation(Vec3::new(0.0, 0.0, -back)) * w.fix;
                    // The trigger swings back under the finger pulling it.
                    let pull = match s.flags & (OS_HELD_LEFT | OS_HELD_RIGHT) {
                        OS_HELD_LEFT => triggers[0],
                        OS_HELD_RIGHT => triggers[1],
                        _ => 0.0,
                    };
                    let pull = if pull.is_finite() { pull.clamp(0.0, 1.0) } else { 0.0 };
                    let trigger = match w.trigger_pivot {
                        Some((at, angle)) => model * Mat4::from_translation(at) * Mat4::from_rotation_x(angle * pull) * Mat4::from_translation(-at) * w.fix,
                        None => model * w.fix,
                    };
                    for part in (0..w.parts.len()).filter(|p| !w.mag_parts.contains(p)) {
                        let m = if w.bolt_parts.contains(&part) {
                            bolt
                        } else if w.trigger_parts.contains(&part) {
                            trigger
                        } else {
                            model * w.fix
                        };
                        out.push((part_id(i, part), m, [1.0; 4], glow, w.from_game));
                    }
                }
                Kind::Mag(i) => {
                    let w = &weapons[i];
                    if !w.mag_parts.is_empty() {
                        for &part in &w.mag_parts {
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
            assert_eq!(laid.len(), 5, "{keys:?}");
        }
        // Each laid-out gun has its own magazine template, and nothing else is registered.
        let templates = a.table.descs[..a.table.count as usize].iter().filter(|d| d.flags & OBJF_TEMPLATE != 0).count();
        assert_eq!(templates, laid.len());
    }

    #[test]
    fn the_near_table_holds_the_long_guns_apart() {
        let weapons = everything();
        let refs: Vec<&Weapon> = weapons.iter().collect();
        let a = Armory::new(&refs);
        let (tmin, tmax) = content::TABLE_NEAR;
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
        // The scenarios reach for the plain rifle: it's the nearest gun on the table.
        let nearest = boxes.iter().min_by(|a, b| a.1.z.total_cmp(&b.1.z)).unwrap();
        assert_eq!(nearest.0, "builtin_rifle");
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
                _ => [0.6, 9.0, 12.0],
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
                assert!(at.z < grip.z && at.x > grip.x, "{}: wrist {at}", w.key);
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
            if w.pistol {
                assert!((10.0..=14.0).contains(&d.recoil_pitch) && (0.025..=0.035).contains(&d.recoil_back), "{}", w.key);
            } else {
                assert!(d.flags & OBJF_TWO_HANDED != 0);
                assert!((Quat::from_array(d.support_rot).length() - 1.0).abs() < 1e-4, "{}: support_rot", w.key);
                assert!((6.0..=8.0).contains(&d.recoil_pitch) && (1.5..=2.0).contains(&d.recoil_yaw), "{}", w.key);
                assert!((0.02..=0.03).contains(&d.recoil_back), "{}", w.key);
            }
            assert!(d.handle_travel > 0.03 && d.weapon_flags & WPN_NEEDS_RACK != 0);
            // The magazine template's box is the magazine, a small part of the gun.
            let m = a.descs[&(MAG_BASE + d.id - WEAPON_BASE)];
            let (mh, gh) = (Vec3::from_array(m.half_extents), Vec3::from_array(d.half_extents));
            assert!(mh.min_element() > 0.004 && mh.max_element() < 0.12 && mh.z < gh.z * 0.4, "{}: magazine box {mh}", w.key);
            assert!(m.mass > 0.15 && m.mass < 0.8);
        }
    }
}
