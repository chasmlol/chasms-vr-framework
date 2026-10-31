//! The range's throwables: the game side of `rustvr_runtime::rig::throwable`. The runtime arms
//! them, times the fuses and flies them; the range (the game) registers them, draws them where
//! it's told (the pin gone once it's pulled, the lever once it flies, a lit fuse sputtering), and
//! when one goes off makes the explosion: its effects, a blast that pushes the props about
//! (`EV_BLAST`), damage to the target plates near it, and the grenade put back a few seconds
//! later. A knife stuck in a target plate counts as a hit on it.
//!
//! Content: built-in frag grenades (an RGD-5-like egg: pin, lever, the UZRGM's 4 s fuse as the
//! range's own data) and throwing knives always; Metro 2033's own dynamite, sticky dynamite,
//! incendiary bomb and throwing knife when extracted (`metro-extract --extras`). Metro's bombs
//! have a fuse cord rather than a pin: the other hand pulls its end to light it (`THROW_PIN`
//! without a lever), and their fuse is left to the runtime's default (4 s) because Metro's
//! configs don't name it. Two frags start in the chest pouches and a knife on the belt; the rest
//! lie on a table at your left hand, in front of the bottles.

use bytemuck::Zeroable;
use glam::{Mat4, Quat, Vec3};
use rustvr_asset as asset;
use rustvr_protocol::*;
use std::collections::HashMap;

use crate::content::MeshData;
use crate::effects::Blast;

pub const THROW_BASE: u32 = 3000;
/// The throwables' table, at your left hand in front of the side table.
pub const TABLE_THROW: (Vec3, Vec3) = (Vec3::new(-0.95, 0.0, 0.80), Vec3::new(-0.45, 0.85, 1.30));
/// How far a blast hurts (the target plates within it are hit) and how hard it pushes the props
/// (radius, N s at the centre).
pub const FRAG_DAMAGE: f32 = 5.0;
pub const FRAG_PUSH: (f32, f32) = (6.0, 10.0);
pub const FIRE_DAMAGE: f32 = 2.5;
pub const FIRE_PUSH: (f32, f32) = (2.5, 2.0);
/// A grenade that went off is back on its table (or in its pouch) this much later.
pub const RESPAWN: f64 = 3.0;

/// A throwable: what it looks like (`parts` in its model space, `fix` into object space), and the
/// descriptor the runtime gets (without its id and place).
pub struct Throwable {
    pub key: String,
    pub name: String,
    pub parts: Vec<MeshData>,
    pub fix: Mat4,
    pub from_game: bool,
    /// Parts drawn only until the pin is out, and until the lever flies.
    pub pin_parts: Vec<usize>,
    pub lever_parts: Vec<usize>,
    pub desc: ObjectDesc,
    /// What it does when it goes off (None: a knife).
    pub blast: Option<Blast>,
    /// Where a lit fuse sputters, or a countdown light blinks (object space).
    pub fuse_at: Option<Vec3>,
    pub led: Option<Vec3>,
    /// How it lies on a table: its rotation and its height above the top.
    pub lying: (Quat, f32),
}

fn base_desc(kind_flags: u32, half: Vec3, center: Vec3, mass: f32) -> ObjectDesc {
    let mut d = ObjectDesc::zeroed();
    d.kind = OBJ_THROWABLE;
    d.flags = OBJF_GRABBABLE | OBJF_PULLABLE | OBJF_HOLSTERABLE;
    d.throw_flags = kind_flags;
    d.shape = SHAPE_BOX;
    d.half_extents = half.to_array();
    d.center = center.to_array();
    d.mass = mass;
    d
}

/// The controller's grip rotation that holds a grenade with its fuze out of the top of the fist
/// (the fist's axis, the grip pose's +Z, along the grenade's +Y), its pin towards the palm.
pub fn fist_up() -> Quat {
    Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}

/// A frag grenade, RGD-5 style: an olive egg along +Y, the fuze on top, the lever down its +Z
/// side and the pin's ring on its -X side.
pub fn builtin_frag() -> Throwable {
    let olive = [86, 94, 60, 255];
    let steel = [150, 150, 145, 255];
    let mut body = MeshData::default();
    body.add_rounded_prism(Vec3::ZERO, Vec3::new(0.029, 0.029, 0.034), fist_up(), olive);
    body.add_box(Vec3::new(-0.009, 0.033, -0.009), Vec3::new(0.009, 0.052, 0.009), [70, 72, 70, 255]);
    let mut lever = MeshData::default();
    lever.add_box(Vec3::new(-0.0045, 0.046, 0.004), Vec3::new(0.0045, 0.052, 0.024), steel);
    lever.add_box(Vec3::new(-0.0065, -0.024, 0.024), Vec3::new(0.0065, 0.05, 0.029), steel);
    let mut pin = MeshData::default();
    pin.add_box(Vec3::new(-0.017, 0.043, -0.0012), Vec3::new(-0.009, 0.046, 0.0012), steel);
    let ring = Vec3::new(-0.027, 0.045, 0.0);
    for k in 0..10 {
        let a = k as f32 / 10.0 * std::f32::consts::TAU;
        let c = ring + Vec3::new(a.cos(), a.sin(), 0.0) * 0.01;
        pin.add_obox(c, Vec3::new(0.0034, 0.0012, 0.0012), Quat::from_rotation_z(a + std::f32::consts::FRAC_PI_2), steel);
    }
    let mut d = base_desc(THROW_GRENADE | THROW_PIN | THROW_LEVER, Vec3::new(0.029, 0.045, 0.031), Vec3::new(0.0, 0.007, 0.0), 0.31);
    d.handle = ring.to_array();
    d.handle_travel = 0.05;
    d.grip_rot = fist_up().to_array();
    // The range's own data: an UZRGM fuze, 3.2-4.2 s.
    d.fuse = 4.0;
    set_name(&mut d.name, "Frag grenade");
    Throwable {
        key: "builtin_frag".into(),
        name: "Frag grenade".into(),
        parts: vec![body, lever, pin],
        fix: Mat4::IDENTITY,
        from_game: false,
        pin_parts: vec![2],
        lever_parts: vec![1],
        desc: d,
        blast: Some(Blast::Frag),
        fuse_at: None,
        led: Some(Vec3::new(0.0, 0.054, 0.0)),
        lying: (Quat::IDENTITY, 0.039),
    }
}

/// A throwing knife: the blade along +Z, its tip at +12.5 cm, the handle behind.
pub fn builtin_knife() -> Throwable {
    let steel = [175, 178, 182, 255];
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.0015, -0.012, 0.0), Vec3::new(0.0015, 0.011, 0.09), steel);
    m.add_box(Vec3::new(-0.0012, -0.007, 0.09), Vec3::new(0.0012, 0.006, 0.125), steel);
    m.add_box(Vec3::new(-0.005, -0.016, -0.005), Vec3::new(0.005, 0.016, 0.0), [60, 60, 62, 255]);
    m.add_box(Vec3::new(-0.0055, -0.011, -0.11), Vec3::new(0.0055, 0.011, -0.005), [35, 32, 30, 255]);
    let mut d = base_desc(THROW_KNIFE, Vec3::new(0.006, 0.016, 0.118), Vec3::new(0.0, 0.0, 0.008), 0.13);
    d.grip = [0.0, 0.0, -0.055];
    d.muzzle = [0.0, 0.0, 0.125];
    d.grip_rot = Quat::IDENTITY.to_array();
    set_name(&mut d.name, "Throwing knife");
    Throwable {
        key: "builtin_knife".into(),
        name: "Throwing knife".into(),
        parts: vec![m],
        fix: Mat4::IDENTITY,
        from_game: false,
        pin_parts: Vec::new(),
        lever_parts: Vec::new(),
        desc: d,
        blast: None,
        fuse_at: None,
        led: None,
        // Flat on the table.
        lying: (Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 0.009),
    }
}

/// Metro 2033's throwables from `local_assets/metro2033` (`metro-extract --extras`), if there.
pub fn metro_throwables(root: &std::path::Path) -> Vec<Throwable> {
    let dir = root.join("metro2033");
    let mut out = Vec::new();
    let bombs = [
        ("throw_dynamite", "Dynamite", 0, Blast::Frag),
        ("throw_sticky_dynamite", "Sticky dynamite", THROW_STICKY, Blast::Frag),
        ("throw_incendiary", "Incendiary bomb", THROW_IMPACT, Blast::Fire),
    ];
    for (key, name, extra, blast) in bombs {
        let Ok(model) = asset::load_model(&dir, key) else { continue };
        // The fuse cord's end: where the other hand lights it, and where it sputters.
        let fuse = bone(&model, "fetil_fire").unwrap_or(Vec3::new(0.0, 0.0, 0.24));
        let sticky = extra & THROW_STICKY != 0;
        let half = if sticky { Vec3::new(0.04, 0.04, 0.12) } else { Vec3::new(0.026, 0.026, 0.115) };
        let mut d = base_desc(THROW_GRENADE | THROW_PIN | extra, half, Vec3::ZERO, 0.45);
        d.handle = fuse.to_array();
        d.handle_travel = 0.05;
        d.grip = [0.0, 0.0, -0.02];
        // Along the fist, the fuse out past the thumb.
        d.grip_rot = Quat::IDENTITY.to_array();
        d.impact_delay = 0.15;
        set_name(&mut d.name, name);
        out.push(Throwable {
            key: key.into(),
            name: name.into(),
            parts: model_meshes(&model),
            fix: Mat4::IDENTITY,
            from_game: true,
            pin_parts: Vec::new(),
            lever_parts: Vec::new(),
            desc: d,
            blast: Some(blast),
            fuse_at: Some(fuse),
            led: None,
            lying: (Quat::IDENTITY, half.y + 0.003),
        });
    }
    if let Ok(model) = asset::load_model(&dir, "throw_knife") {
        let lo = Vec3::from_array(model.bounds_min);
        let hi = Vec3::from_array(model.bounds_max);
        let mut d = base_desc(THROW_KNIFE, (hi - lo) * 0.5, (hi + lo) * 0.5, 0.12);
        // The blade is the +Z half; the handle (wrapped) the -Z half, its ring at the end.
        d.grip = [0.0, 0.0, lo.z * 0.5];
        d.muzzle = [0.0, 0.0, hi.z];
        d.grip_rot = Quat::IDENTITY.to_array();
        set_name(&mut d.name, "Throwing knife");
        out.push(Throwable {
            key: "throw_knife".into(),
            name: "Throwing knife (Metro)".into(),
            parts: model_meshes(&model),
            fix: Mat4::IDENTITY,
            from_game: true,
            pin_parts: Vec::new(),
            lever_parts: Vec::new(),
            desc: d,
            blast: None,
            fuse_at: None,
            led: None,
            lying: (Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), (hi.x - lo.x) * 0.5 + 0.003),
        });
    }
    out
}

fn bone(model: &asset::Model, name: &str) -> Option<Vec3> {
    let b = model.bones.iter().find(|b| b.name == name)?;
    Some(Mat4::from_cols_array(&b.model_matrix).w_axis.truncate())
}

fn model_meshes(m: &asset::Model) -> Vec<MeshData> {
    m.meshes
        .iter()
        .filter(|mesh| !mesh.indices.is_empty() && !mesh.name.starts_with("collision"))
        .map(|mesh| MeshData {
            vertices: mesh.vertices.iter().map(|v| crate::content::Vertex { pos: v.pos, normal: v.normal, uv: v.uv, color: [255; 4] }).collect(),
            indices: mesh.indices.clone(),
            source: None,
            texture: if mesh.texture.is_empty() { None } else { Some(mesh.texture.clone()) },
        })
        .collect()
}

/// The sound set a throwable uses in Metro's sound pack (`metro-assets`' `sound_pack`): Metro's
/// its own (its key), the built-in knife Metro's throwing knife's, and the built-in frag the
/// pack's pin grenade (`grenade`: Metro has none, so its pin and lever stay the runtime's own).
pub fn sound_set(t: &Throwable) -> &str {
    match t.key.as_str() {
        "builtin_frag" => "grenade",
        "builtin_knife" => "throw_knife",
        key => key,
    }
}

/// Everything the range throws: the built-in ones, then Metro's.
pub fn all(root: &std::path::Path) -> Vec<Throwable> {
    let mut v = vec![builtin_frag(), builtin_knife()];
    // The melee weapons (the arming sword first, on this table; the rest on the melee rack).
    v.extend(crate::arsenal::all().into_iter().map(|a| a.thing));
    v.extend(metro_throwables(root));
    v
}

/// Their places: (which throwable, its descriptor with id, spawn pose and start slot). Two frags
/// in the chest pouches and a knife in the belt; two frags, two knives (Metro's if it's there),
/// the arming sword, a dagger and Metro's bombs on the table.
pub fn layout(list: &[Throwable]) -> Vec<(usize, ObjectDesc)> {
    let find = |key: &str| list.iter().position(|t| t.key == key);
    let frag = find("builtin_frag").unwrap();
    let knife = find("throw_knife").or(find("builtin_knife")).unwrap();
    let (lo, hi) = TABLE_THROW;
    let top = hi.y;
    let mut places: Vec<(usize, Vec3, u32)> = vec![
        (frag, Vec3::new(-0.53, top, 0.87), 0),
        (frag, Vec3::new(-0.62, top, 0.87), 0),
        (knife, Vec3::new(-0.84, top, 1.05), 0),
        (knife, Vec3::new(-0.91, top, 1.05), 0),
        (frag, Vec3::new(-0.53, top, 0.87), SLOT_CHEST_LEFT + 1),
        (frag, Vec3::new(-0.53, top, 0.87), SLOT_CHEST_RIGHT + 1),
        (find("builtin_knife").unwrap(), Vec3::new(-0.91, top, 1.05), SLOT_BELT + 1),
    ];
    // The sword, its handle towards the player, and a dagger to throw behind it.
    for (key, at) in [("builtin_sword", Vec3::new(-0.72, top, 0.82)), ("kit_dagger", Vec3::new(-0.66, top, 0.97))] {
        if let Some(i) = find(key) {
            places.push((i, at, 0));
        }
    }
    for (key, x) in [("throw_dynamite", -0.53), ("throw_incendiary", -0.61), ("throw_sticky_dynamite", -0.71)] {
        if let Some(i) = find(key) {
            places.push((i, Vec3::new(x, top, 1.08), 0));
        }
    }
    let n_table = places.len();
    // The melee rack's weapons, each lying its own way.
    let mut turned: Vec<(usize, Quat)> = Vec::new();
    for (key, at, rot) in crate::arsenal::rack_places() {
        if let Some(i) = find(key) {
            turned.push((places.len(), rot));
            places.push((i, at, 0));
        }
    }
    places
        .into_iter()
        .enumerate()
        .map(|(k, (i, at, slot))| {
            let t = &list[i];
            let mut d = t.desc;
            d.id = THROW_BASE + k as u32;
            d.spawn_seq = 1;
            d.start_slot = slot;
            // (The rack's weapons lie on the rack, at the height their lying pose puts them.)
            let rot = turned.iter().find(|(n, _)| *n == k).map_or(t.lying.0, |(_, r)| *r);
            let at = if k < n_table { at.clamp(lo, hi) } else { at };
            d.spawn_pose = Pose { pos: (at + Vec3::Y * t.lying.1).to_array(), valid: 1, rot: rot.to_array() };
            (i, d)
        })
        .collect()
}

pub fn furniture() -> Vec<(Vec3, Vec3, [u8; 4])> {
    vec![(TABLE_THROW.0, TABLE_THROW.1, [110, 85, 60, 255])]
}

/// Mesh ids for throwable parts in the draw list.
pub fn part_id(throwable: usize, part: usize) -> usize {
    20_000 + throwable * 100 + part
}

/// What happened to the throwables since the last frame (the game's side).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// Object `id` went off at `at` (world).
    Exploded { id: u32, at: Vec3, blast: Blast },
    /// A knife (or sticky bomb) went in, its tip at `tip` (world).
    Stuck { id: u32, tip: Vec3 },
}

/// Watches the runtime's states for detonations and sticks, and puts spent grenades back.
#[derive(Default)]
pub struct Watch {
    /// Per object: detonations seen, stuck last frame.
    seen: HashMap<u32, (u32, bool)>,
    respawn: Vec<(u32, f64)>,
}

impl Watch {
    /// The new events. `states` with their world poses; `which` maps a desc to its throwable.
    pub fn update(&mut self, states: &[(ObjectState, Vec3, Quat)], which: &dyn Fn(u32) -> Option<usize>, list: &[Throwable], now: f64) -> Vec<Event> {
        let mut out = Vec::new();
        for (s, p, q) in states {
            let Some(i) = which(s.desc) else { continue };
            let t = &list[i];
            let (shots, stuck) = self.seen.get(&s.id).copied().unwrap_or((s.shots, s.flags & OS_STUCK != 0));
            if s.shots.wrapping_sub(shots) > 0
                && let Some(blast) = t.blast
            {
                out.push(Event::Exploded { id: s.id, at: *p, blast });
                self.respawn.push((s.id, now + RESPAWN));
            }
            let now_stuck = s.flags & OS_STUCK != 0;
            if now_stuck && !stuck {
                out.push(Event::Stuck { id: s.id, tip: *p + *q * Vec3::from_array(t.desc.muzzle) });
            }
            self.seen.insert(s.id, (s.shots, now_stuck));
        }
        out
    }

    /// Grenades due back now.
    pub fn due(&mut self, now: f64) -> Vec<u32> {
        let due: Vec<u32> = self.respawn.iter().filter(|(_, t)| *t <= now).map(|(id, _)| *id).collect();
        self.respawn.retain(|(_, t)| *t > now);
        due
    }

    pub fn reset(&mut self) {
        self.seen.clear();
    }
}

/// A knife's tip in a target plate (centre, radius): within its face, just behind it.
pub fn in_plate(tip: Vec3, plate: (Vec3, f32)) -> bool {
    let (c, r) = plate;
    (tip - c).truncate().length() <= r + 0.03 && (tip.z - c.z) > -0.03 && (tip.z - c.z) < 0.08
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_fills_the_pouches_and_the_table() {
        let list = vec![builtin_frag(), builtin_knife()];
        let placed = layout(&list);
        let slots: Vec<u32> = placed.iter().map(|(_, d)| d.start_slot).filter(|s| *s != 0).collect();
        assert_eq!(slots, vec![SLOT_CHEST_LEFT + 1, SLOT_CHEST_RIGHT + 1, SLOT_BELT + 1]);
        let (lo, hi) = TABLE_THROW;
        for (i, d) in &placed {
            let p = Vec3::from_array(d.spawn_pose.pos);
            assert!(p.x >= lo.x && p.x <= hi.x && p.z >= lo.z && p.z <= hi.z, "{}: on the table {p}", list[*i].key);
            assert!(d.kind == OBJ_THROWABLE && d.id >= THROW_BASE);
            // Resting on the top: the collider's lowest point just above it.
            let rot = Quat::from_array(d.spawn_pose.rot);
            let half = Vec3::from_array(d.half_extents);
            let low = (0..8).map(|k| (rot * (Vec3::from_array(d.center) + half * Vec3::new([-1.0, 1.0][k & 1], [-1.0, 1.0][(k >> 1) & 1], [-1.0, 1.0][k >> 2]))).y).fold(f32::MAX, f32::min);
            assert!((p.y + low - hi.y).abs() < 0.006, "{}: rests on the table ({})", list[*i].key, p.y + low);
        }
        let ids: Vec<u32> = placed.iter().map(|(_, d)| d.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "unique ids");
    }

    #[test]
    fn throwables_sound_like_metros() {
        assert_eq!((sound_set(&builtin_frag()), sound_set(&builtin_knife())), ("grenade", "throw_knife"));
        let mut t = builtin_knife();
        t.key = "throw_dynamite".into();
        assert_eq!(sound_set(&t), "throw_dynamite", "Metro's are their own");
    }

    #[test]
    fn the_frags_pin_faces_the_palm_and_its_fuze_the_thumb() {
        let f = builtin_frag();
        assert!(f.desc.throw_flags & (THROW_GRENADE | THROW_PIN | THROW_LEVER) == THROW_GRENADE | THROW_PIN | THROW_LEVER);
        let g = Quat::from_array(f.desc.grip_rot);
        // In the hand (object = hand * g^-1): the fuze (+Y) along the fist (+Z), the ring (-X) on the
        // palm's side (-X of a right hand's grip pose).
        assert!((g.inverse() * Vec3::Y - Vec3::Z).length() < 1e-5);
        assert!(f.desc.handle[0] < -0.02);
        let k = builtin_knife();
        assert!(k.desc.muzzle[2] > 0.1 && k.desc.grip[2] < 0.0, "the blade from the handle to the tip along +Z");
    }

    #[test]
    fn detonations_and_sticks_are_seen_once() {
        let list = vec![builtin_frag(), builtin_knife()];
        let mut w = Watch::default();
        let mut s = ObjectState::zeroed();
        s.id = 3000;
        s.desc = 3000;
        let which = |d: u32| Some(if d == 3000 { 0 } else { 1 });
        let at = (s, Vec3::new(0.0, 0.1, 7.0), Quat::IDENTITY);
        assert!(w.update(&[at], &which, &list, 0.0).is_empty());
        let mut s2 = s;
        s2.shots = 1;
        s2.flags = OS_SPENT;
        let ev = w.update(&[(s2, at.1, at.2)], &which, &list, 1.0);
        assert_eq!(ev, vec![Event::Exploded { id: 3000, at: at.1, blast: Blast::Frag }]);
        assert!(w.update(&[(s2, at.1, at.2)], &which, &list, 1.1).is_empty(), "once");
        assert!(w.due(2.0).is_empty() && w.due(4.1) == vec![3000], "back after a while");
        // A knife sticking into the 5 m plate.
        let mut k = ObjectState::zeroed();
        k.id = 3001;
        k.desc = 3001;
        w.update(&[(k, Vec3::ZERO, Quat::IDENTITY)], &which, &list, 0.0);
        k.flags = OS_STUCK;
        let tip_at = Vec3::new(-1.5, 1.4, 5.0 - 0.125 + 0.02);
        let ev = w.update(&[(k, tip_at, Quat::IDENTITY)], &which, &list, 0.1);
        let Event::Stuck { tip, .. } = ev[0] else { panic!("{ev:?}") };
        assert!(in_plate(tip, (Vec3::new(-1.5, 1.4, 5.0), 0.3)));
        assert!(!in_plate(tip + Vec3::X, (Vec3::new(-1.5, 1.4, 5.0), 0.3)));
    }
}
