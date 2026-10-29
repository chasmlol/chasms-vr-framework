//! The range's combat: a training dummy (a creature the hands can hit, published as capsules every
//! frame), a sword on the throwables' table (its edges cut both ways, its point stabs, its pommel
//! bludgeons) and one spell, a fire bolt, so the runtime's melee and magic can be played and tested
//! without a game (`rustvr_protocol::combat`).

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use crate::content::MeshData;
use crate::throwables::Throwable;

/// The dummy's id, as the game knows it.
pub const DUMMY_ID: u32 = 7001;
/// Where it stands (its feet), in front of the player's start.
pub const DUMMY_AT: Vec3 = Vec3::new(0.15, 0.0, 1.15);
/// The fire bolt's id.
pub const FIRE_BOLT: u32 = 1;

/// The dummy's body: (from, to, radius, part kind, material), relative to its feet.
const BODY: &[(Vec3, Vec3, f32, u32, u32)] = &[
    (Vec3::new(0.0, 1.05, 0.0), Vec3::new(0.0, 1.45, 0.0), 0.17, PART_TORSO, MAT_LEATHER),
    (Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.0, 1.68, 0.0), 0.11, PART_HEAD, MAT_FLESH),
    (Vec3::new(-0.22, 1.42, 0.0), Vec3::new(-0.26, 1.05, 0.0), 0.05, PART_ARM, MAT_FLESH),
    (Vec3::new(0.22, 1.42, 0.0), Vec3::new(0.26, 1.05, 0.0), 0.05, PART_ARM, MAT_FLESH),
    (Vec3::new(-0.1, 0.95, 0.0), Vec3::new(-0.1, 0.1, 0.0), 0.07, PART_LEG, MAT_FLESH),
    (Vec3::new(0.1, 0.95, 0.0), Vec3::new(0.1, 0.1, 0.0), 0.07, PART_LEG, MAT_FLESH),
];

pub fn dummy() -> Actor {
    let mut a: Actor = bytemuck::Zeroable::zeroed();
    a.id = DUMMY_ID;
    a.flags = ACT_ALIVE | ACT_HOSTILE;
    a.health = 1.0;
    for (i, &(from, to, r, kind, material)) in BODY.iter().enumerate() {
        a.parts[i] = ActorPart { a: (DUMMY_AT + from).to_array(), radius: r, b: (DUMMY_AT + to).to_array(), kind, material, game_part: i as u32, _pad: [0; 2] };
    }
    a.count = BODY.len() as u32;
    a
}

pub fn actors() -> Actors {
    let mut t: Actors = bytemuck::Zeroable::zeroed();
    t.actors[0] = dummy();
    t.count = 1;
    t
}

/// The dummy as boxes round its capsules (straw yellow, a dark head).
pub fn dummy_mesh() -> MeshData {
    let mut m = MeshData::default();
    for &(from, to, r, kind, _) in BODY {
        let (a, b) = (DUMMY_AT + from, DUMMY_AT + to);
        let colour = if kind == PART_HEAD { [70, 55, 40, 255] } else { [196, 168, 96, 255] };
        let mid = (a + b) * 0.5;
        let along = (b - a).normalize_or(Vec3::Y);
        let rot = Quat::from_rotation_arc(Vec3::Y, along);
        m.add_obox(mid, Vec3::new(r, (b - a).length() * 0.5 + r, r), rot, colour);
    }
    // Its post.
    m.add_box(DUMMY_AT + Vec3::new(-0.03, 0.0, -0.03), DUMMY_AT + Vec3::new(0.03, 1.05, 0.03), [90, 70, 50, 255]);
    m
}

/// A one-handed sword: the blade along +Z from the guard at 0 to its point at +80 cm, its edges to
/// ±X, the handle behind (the grip at -6 cm), the pommel at -13 cm.
pub fn sword() -> Throwable {
    let steel = [190, 192, 198, 255];
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-0.022, -0.003, 0.0), Vec3::new(0.022, 0.003, 0.72), steel);
    m.add_box(Vec3::new(-0.012, -0.0025, 0.72), Vec3::new(0.012, 0.0025, 0.8), steel);
    m.add_box(Vec3::new(-0.08, -0.012, -0.012), Vec3::new(0.08, 0.012, 0.0), [120, 100, 60, 255]);
    m.add_box(Vec3::new(-0.014, -0.014, -0.11), Vec3::new(0.014, 0.014, -0.012), [60, 40, 30, 255]);
    m.add_box(Vec3::new(-0.022, -0.022, -0.135), Vec3::new(0.022, 0.022, -0.11), [120, 100, 60, 255]);
    let mut d: ObjectDesc = bytemuck::Zeroable::zeroed();
    d.kind = OBJ_MELEE;
    d.flags = OBJF_GRABBABLE | OBJF_PULLABLE | OBJF_HOLSTERABLE;
    d.shape = SHAPE_BOX;
    d.half_extents = [0.08, 0.022, 0.47];
    d.center = [0.0, 0.0, 0.33];
    d.mass = 1.2;
    d.grip = [0.0, 0.0, -0.06];
    d.muzzle = [0.0, 0.0, 0.8];
    // The fist round the handle (little finger to thumb along the blade), the back of the hand to
    // the blade's flat.
    d.grip_rot = rustvr_guns::fist(Vec3::Z, Vec3::Y).to_array();
    // The handle, from the pommel to the guard: a hand goes on anywhere along it.
    d.grip_line = [[0.0, 0.0, -0.11], [0.0, 0.0, -0.012]];
    d.grip_radius = 0.014;
    set_name(&mut d.name, "Arming sword");
    Throwable {
        key: "builtin_sword".into(),
        name: "Arming sword".into(),
        parts: vec![m],
        fix: glam::Mat4::IDENTITY,
        from_game: false,
        pin_parts: Vec::new(),
        lever_parts: Vec::new(),
        desc: d,
        blast: None,
        fuse_at: None,
        led: None,
        // Flat on the table.
        lying: (Quat::from_rotation_x(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 0.012),
    }
}

/// The sword's damagers: both edges along the blade, the point, the pommel.
pub fn sword_melee(id: u32) -> MeleeDesc {
    let mut w = MeleeDesc { id, count: 3, damage: 12.0, reach: 0.86, ..Default::default() };
    w.damagers[0] = Damager { kind: DMG_SLASH, flags: DMGF_BOTH_WAYS, radius: 0.025, a: [0.0, 0.0, 0.08], b: [0.0, 0.0, 0.72], edge: [1.0, 0.0, 0.0], ..Default::default() };
    w.damagers[1] = Damager { kind: DMG_PIERCE, radius: 0.015, depth: 0.2, a: [0.0, 0.0, 0.7], b: [0.0, 0.0, 0.8], ..Default::default() };
    w.damagers[2] = Damager { kind: DMG_BLUNT, radius: 0.025, a: [0.0, 0.0, -0.135], b: [0.0, 0.0, -0.11], ..Default::default() };
    w
}

pub fn melee_table(ids: &[u32]) -> MeleeTable {
    let mut t: MeleeTable = bytemuck::Zeroable::zeroed();
    for (i, &id) in ids.iter().take(MAX_MELEE).enumerate() {
        t.weapons[i] = sword_melee(id);
    }
    t.count = ids.len().min(MAX_MELEE) as u32;
    t
}

pub fn spells() -> Spells {
    let mut s: Spells = bytemuck::Zeroable::zeroed();
    let mut bolt: Spell = bytemuck::Zeroable::zeroed();
    bolt.id = FIRE_BOLT;
    bolt.delivery = DELIVER_TARGET;
    bolt.flags = SPELL_SELECTED | SPELL_CASTABLE;
    bolt.cost = 10.0;
    bolt.color = [1.0, 0.45, 0.1];
    bolt.set_name("Fire bolt");
    s.spells[0] = bolt;
    s.count = 1;
    s.magicka = 100.0;
    s.magicka_max = 100.0;
    s.cast_time = 0.5;
    s
}
