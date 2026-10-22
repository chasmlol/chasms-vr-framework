//! What the range is made of: the room, the targets, and the guns (Metro 2033's own, from the
//! local asset cache, or simple built-in ones when that cache doesn't exist). The guns themselves,
//! and Metro's, are `rustvr_guns`' (shared with the game adapters and the runtime).

use glam::{Mat4, Quat, Vec3};
#[allow(unused_imports)] // (the tests use it)
use rustvr_asset as asset;
use rustvr_protocol::*;

pub use rustvr_guns::metro::{MetroRig, metro_rigs, metro_weapons};
pub use rustvr_guns::*;

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
        glass: Vec::new(),
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
        glass: Vec::new(),
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
            if let Some(port) = w.action.port {
                // A magazine that stays in it: each of its rounds its own small part, the port at
                // the first's base.
                assert!(!w.action.round_parts.is_empty() && w.mag_parts.is_empty(), "{}", w.key);
                for (k, parts) in w.action.round_parts.iter().enumerate() {
                    let (lo, hi) = parts_bounds(&w.parts, parts, w.fix).unwrap_or_else(|| panic!("{}: round {k} has no part", w.key));
                    assert!(tris(parts) > 0 && (hi - lo).max_element() < 0.5, "{}: round {k} {lo}..{hi}", w.key);
                    if k == 0 {
                        let d = port.clamp(lo, hi).distance(port);
                        assert!(d < 0.005, "{}: the port {port} is {d} m from the first round {lo}..{hi}", w.key);
                    }
                }
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
            // (Iron sights a long way apart; a collimator's axis is its mount's and its reticle's
            // centre, both the game's own points, 2.8 cm apart on the Kalash 2012.)
            assert!(front.z - rear.z > 0.1 || (w.key == "wpn_2012" && front.z - rear.z > 0.02), "{}", w.key);
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

