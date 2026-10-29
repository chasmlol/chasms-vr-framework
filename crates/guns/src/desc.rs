//! A gun as the protocol registers it: its [`ObjectDesc`], its magazine template, the game's hand
//! poses on it and its kicks. The test range and the game adapters build them here, so a gun is
//! registered the same way wherever it's played.

use bytemuck::Zeroable;
use glam::{Mat4, Vec3};
use rustvr_protocol::*;

use crate::weapon::{GunHand, Weapon};

/// Bounds of points, if there are any.
pub fn bounds_of(points: impl Iterator<Item = Vec3>) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for p in points {
        min = min.min(p);
        max = max.max(p);
    }
    (min.x <= max.x).then_some((min, max))
}

/// Weapon-space bounds of the parts of a gun `keep` picks (by index in `parts`).
pub fn bounds_where(w: &Weapon, keep: impl Fn(usize) -> bool) -> Option<(Vec3, Vec3)> {
    let pts = w.parts.iter().enumerate().filter(|(i, _)| keep(*i)).flat_map(|(_, p)| p.vertices.iter().map(|v| w.fix.transform_point3(Vec3::from_array(v.pos))));
    bounds_of(pts)
}

/// Weapon-space bounds of a gun without its magazine (its collider).
pub fn weapon_bounds(w: &Weapon) -> (Vec3, Vec3) {
    bounds_where(w, |i| !w.mag_parts.contains(&i)).unwrap_or((Vec3::new(-0.03, -0.1, -0.3), Vec3::new(0.03, 0.08, 0.6)))
}

/// Weapon-space bounds of a gun with its magazine in (what it covers lying on a table).
pub fn drawn_bounds(w: &Weapon) -> (Vec3, Vec3) {
    bounds_where(w, |_| true).unwrap_or_else(|| weapon_bounds(w))
}

/// A gun's description (object space is weapon space): its collider, grab points, how it's worked
/// and loaded, how it fires and how the hands hold it. `poses` are the ids its game hand poses
/// get in the hand pose table ([support, firing]; see [`pose_desc`]): set where it has that hand.
/// Placing it (`spawn_*`, `start_slot`) and its `ammo_type` are the caller's.
pub fn weapon_desc(id: u32, w: &Weapon, poses: [u32; 2]) -> ObjectDesc {
    let (min, max) = weapon_bounds(w);
    let mut d = ObjectDesc::zeroed();
    d.id = id;
    d.kind = OBJ_WEAPON;
    d.flags = OBJF_GRABBABLE
        | OBJF_PULLABLE
        | OBJF_HOLSTERABLE
        | if w.pistol { 0 } else { OBJF_TWO_HANDED }
        | if w.ads_zoom > 1.4 { OBJF_SCOPED } else { 0 };
    d.shape = SHAPE_BOX;
    d.mass = w.mass;
    d.half_extents = ((max - min) * 0.5).to_array();
    d.center = ((max + min) * 0.5).to_array();
    d.grip = w.rig.grip;
    d.foregrip = w.rig.foregrip;
    // The pistol grip's thickness, and the handguard (the fingers close round them).
    if let Some((_, half)) = w.grip_block {
        d.grip_radius = 0.5 * (half.x + half.y);
    }
    if let Some((a, b, r)) = w.handguard {
        d.foregrip_line = [a.to_array(), b.to_array()];
        d.foregrip_radius = r;
    }
    d.mag_well = w.rig.mag_well;
    d.muzzle = w.rig.muzzle;
    if let Some(l) = w.laser {
        d.laser = l.to_array();
    }
    d.sight = w.rig.sight;
    if let Some((h, travel)) = w.handle {
        d.handle = h.to_array();
        d.handle_travel = travel;
    }
    d.capacity = w.mag_size;
    // How it's worked: a self-loading gun with a handle (not an air gun's pump) or a bolt or pump
    // action needs a fresh magazine racked in.
    let a = &w.action;
    let racks = w.handle.is_some() && a.air.is_none() && matches!(a.kind, ACTION_SELF_LOADING | ACTION_BOLT | ACTION_PUMP);
    d.weapon_flags = if w.automatic { WPN_AUTOMATIC } else { 0 }
        | if w.slide_locks { WPN_SLIDE_LOCKS } else { 0 }
        | if racks { WPN_NEEDS_RACK } else { 0 }
        | if a.air.is_some() { WPN_PNEUMATIC } else { 0 }
        | if a.chambers.is_empty() { 0 } else { WPN_LOADS_CHAMBERS }
        | if a.round_parts.is_empty() { 0 } else { WPN_LOADS_ROUNDS }
        | if w.pistol { WPN_PISTOL } else if w.stock { WPN_STOCK } else { 0 };
    d.action = a.kind;
    match a.kind {
        ACTION_BREAK | ACTION_SWING => {
            d.handle = a.hinge.to_array();
            d.handle_travel = 0.0;
            d.action_angle = a.open;
        }
        ACTION_BOLT => d.action_angle = a.bolt_turn.map_or(0.0, |[closed, open]| open - closed),
        _ => {}
    }
    if !a.chambers.is_empty() {
        d.capacity = a.chambers.len() as u32;
        d.mag_well = a.chambers[0].to_array();
        d.chamber_step = a.chambers.get(1).map_or(0.0, |c1| c1.x - a.chambers[0].x);
    }
    if let (Some(port), false) = (a.port, a.round_parts.is_empty()) {
        d.capacity = a.round_parts.len() as u32;
        d.mag_well = port.to_array();
    }
    if let Some([pump, shot, min]) = a.air {
        [d.pump_pressure, d.shot_pressure, d.min_pressure] = [pump, shot, min];
    }
    d.fire_rate = w.rounds_per_second;
    [d.recoil_pitch, d.recoil_yaw, d.recoil_back] = w.recoil;
    [d.recoil_climb, d.recoil_return, d.recoil_angle] = w.recoil_feel;
    // How the hands hold it (all zero: the runtime's fallbacks), and the game's own hand poses.
    d.grip_rot = w.grip_rot.map(|q| q.normalize().to_array()).unwrap_or([0.0; 4]);
    if w.grip_rot.is_some() && w.game_hands[1].is_some() {
        d.grip_pose = poses[1];
    }
    if !w.pistol {
        d.support_rot = w.support_rot.map(|q| q.normalize().to_array()).unwrap_or([0.0; 4]);
        if w.support_rot.is_some() && w.game_hands[0].is_some() {
            d.support_pose = poses[0];
        }
    }
    set_name(&mut d.name, &w.name);
    d
}

/// A gun's magazine template (`OBJF_TEMPLATE`: the runtime's ammo pouch spawns copies), or for a
/// gun loaded round by round one round. Magazine space is the gun's own space: seated, a magazine
/// sits exactly where the gun's model has it, so its part draws in place. Its collider is the
/// magazine mesh's box; a round's base is at the first chamber's mouth (or the port). None for a gun
/// without one (it holds its rounds itself: the flamethrower's fuel). `ammo_type` is the caller's.
pub fn mag_desc(id: u32, w: &Weapon) -> Option<ObjectDesc> {
    if w.round_parts().is_empty() {
        return None;
    }
    let round = w.round_base().is_some();
    let (min, max) = bounds_where(w, |p| w.round_parts().contains(&p)).unwrap_or((w.mag_center - Vec3::new(0.016, 0.07, 0.03), w.mag_center + Vec3::new(0.016, 0.07, 0.03)));
    let mut d = ObjectDesc::zeroed();
    d.id = id;
    d.kind = OBJ_MAGAZINE;
    d.flags = OBJF_GRABBABLE | OBJF_PULLABLE | OBJF_TEMPLATE;
    d.shape = SHAPE_BOX;
    d.mass = w.mag_mass;
    d.half_extents = ((max - min) * 0.5).to_array();
    d.center = ((max + min) * 0.5).to_array();
    d.mag_well = w.round_base().unwrap_or(Vec3::from_array(w.rig.mag_well)).to_array();
    d.capacity = if round { 1 } else { w.mag_size };
    d.rounds = d.capacity;
    set_name(&mut d.name, &format!("{} {}", w.name, if round { "round" } else { "magazine" }));
    Some(d)
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

/// A gun's own recoil as kick table entries: its kicks taken from the model's space into the
/// weapon's (`Weapon::fix`), rotations and translations alike (the channels at each one's key times,
/// so the turn is exact).
pub fn kick_descs(object: u32, w: &Weapon) -> Vec<rustvr_protocol::KickDesc> {
    rustvr_adapter::kicks::kick_descs(object, &w.kicks, w.fix)
}

/// What the runtime draws a gun with when it draws it itself ([`OBJF_RUNTIME_DRAWS`]): the model
/// named `<game folder>/<asset key>` in its asset cache (`metro2033/wpn_2012`).
pub fn model_name(game: &str, key: &str) -> String {
    format!("{game}/{key}")
}

/// The other half of [`model_name`]: (game folder, asset key), if `name` is one.
pub fn parse_model_name(name: &str) -> Option<(&str, &str)> {
    let (game, key) = name.split_once('/')?;
    (!game.is_empty() && !key.is_empty() && !key.contains('/')).then_some((game, key))
}

/// Ids a gun's registration takes (see [`GunRegistration`]): its object, its magazine template and
/// its two hand poses. All must be non-zero and unique in their tables.
#[derive(Clone, Copy, Debug)]
pub struct GunIds {
    pub gun: u32,
    pub magazine: u32,
    pub grip_pose: u32,
    pub support_pose: u32,
}

/// Everything to register for one gun: its description, its magazine template (when it takes
/// magazines), the game's hand poses on it (the hand pose table) and its recoil (the kick table).
#[derive(Clone, Debug)]
pub struct GunRegistration {
    pub desc: ObjectDesc,
    pub magazine: Option<ObjectDesc>,
    pub poses: Vec<HandPoseDesc>,
    pub kicks: Vec<KickDesc>,
}

/// A gun's registration (see [`GunRegistration`]), the gun and its magazine sharing `ammo_type`.
/// With `game` (its folder in the asset cache) both are drawn by the runtime from that model
/// ([`OBJF_RUNTIME_DRAWS`], named by [`model_name`]); without, the game draws them.
pub fn registration(w: &Weapon, ids: GunIds, ammo_type: u32, game: Option<&str>) -> GunRegistration {
    let mut desc = weapon_desc(ids.gun, w, [ids.support_pose, ids.grip_pose]);
    desc.ammo_type = ammo_type;
    let mut magazine = mag_desc(ids.magazine, w);
    let mut poses = Vec::new();
    for (id, side) in [(desc.grip_pose, HAND_RIGHT), (desc.support_pose, HAND_LEFT)] {
        if let (true, Some(h)) = (id != 0, &w.game_hands[side]) {
            poses.push(pose_desc(id, side, h));
        }
    }
    if let Some(m) = &mut magazine {
        m.ammo_type = ammo_type;
    }
    if let Some(game) = game {
        let name = model_name(game, &w.key);
        set_name(&mut desc.name, &name);
        desc.flags |= OBJF_RUNTIME_DRAWS;
        if let Some(m) = &mut magazine {
            set_name(&mut m.name, &name);
            m.flags |= OBJF_RUNTIME_DRAWS;
        }
    }
    GunRegistration { kicks: kick_descs(ids.gun, w), desc, magazine, poses }
}

/// The transform a part of `w` is drawn with for an object of it at `object` (its pose's matrix):
/// a gun's parts where [`Weapon::part_matrices`] puts them, a magazine object's (a template's copy)
/// its magazine parts in place (magazine space is the gun's). The same for the range's own drawing
/// and the runtime's. None: not drawn.
pub fn object_part_matrices(w: &Weapon, kind: u32, state: &crate::PartsState, object: Mat4) -> Vec<Option<Mat4>> {
    match kind {
        OBJ_MAGAZINE => (0..w.parts.len()).map(|i| w.round_parts().contains(&i).then_some(object * w.fix)).collect(),
        _ => w.part_matrices(state).into_iter().map(|m| m.map(|m| object * m)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_round_trip() {
        assert_eq!(parse_model_name(&model_name("metro2033", "wpn_2012")), Some(("metro2033", "wpn_2012")));
        for bad in ["wpn_2012", "/wpn_2012", "metro2033/", "a/b/c", "Kalash 2012"] {
            assert_eq!(parse_model_name(bad), None, "{bad}");
        }
        // (It fits the protocol's name.)
        assert!(model_name("metro2033", "wpn_railgun_hand").len() < 32);
    }
}
