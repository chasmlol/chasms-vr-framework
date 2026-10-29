//! Metro 2033's guns: its first-person models from the local asset cache (`metro-extract`), rigged
//! by `content/rigs/metro2033.toml` (see docs/RIGGING.md).

use glam::{Mat4, Quat, Vec3};
use rustvr_asset as asset;
use rustvr_protocol::*;

use crate::mesh::{MeshData, MeshSource, Vertex};
use crate::weapon::*;

/// The first-person arms the extractor poses for every gun (`metro-extract` bakes each gun's arms
/// idle into a copy of these): the rest pose the finger frames are carried from.
pub const ARMS_REST: &str = "hands_basic_suit";

/// A Metro 2033 gun rigged by hand: its entry in `content/rigs/metro2033.toml` (see
/// `rustvr_asset::rigs` and docs/RIGGING.md), in vectors. Every point is in the model's own space
/// (Metro's viewmodel space, as `cargo run -p rustvr-asset --example rig_probe` prints it); the
/// loader turns them into weapon space with the gun's `fix`.
pub struct MetroRig {
    pub key: String,
    pub name: Option<String>,
    /// One-handed (see `rigs::Hold`).
    pub pistol: bool,
    /// Shouldered (a rifle's hold, not a heavy gun's handles).
    pub stock: bool,
    pub caliber: String,
    /// The magazine it takes: guns naming the same one take each other's.
    pub magazine: String,
    /// Top of the rear sight notch's shoulders, and the front post's tip: weapon +Z is the line
    /// through them.
    pub rear_sight: Vec3,
    pub front_sight: Vec3,
    /// The firing hand's grip pose origin (palm centre) on the pistol grip's axis, the grip's rake
    /// and an extra turn of the hand about it.
    pub grip: Vec3,
    pub grip_rake_deg: f32,
    pub grip_twist_deg: f32,
    /// The trigger finger's pad on the trigger's face.
    pub trigger: Vec3,
    /// The pistol grip's centre, and its half size across, front to back (square to its axis) and
    /// along it.
    pub grip_block: (Vec3, Vec3),
    /// The support hand's grip pose origin (its palm centre, cradling the handguard) and its fist's
    /// tube and palm normal (see [`fist`]); used only without Metro's arms.
    pub foregrip: Option<Vec3>,
    pub support_tube: Vec3,
    pub support_palm: Vec3,
    /// Where the magazine's top sits when it's in.
    pub mag_well: Vec3,
    pub muzzle: Vec3,
    /// Charging handle (bolt, slide, pump, lever) knob, and how far it travels back.
    pub handle: Option<(Vec3, f32)>,
    /// The trigger's bone (its blade's skin).
    pub trigger_bone: String,
    /// Bones (with their children) whose skin is the magazine, and the bolt or charging handle.
    pub mag_bones: Vec<String>,
    pub bolt_bones: Vec<String>,
    /// Meshes (by index in the model) that are the magazine too.
    pub mag_meshes: Vec<usize>,
    /// Bones whose skin isn't drawn.
    pub hidden_bones: Vec<String>,
    pub hidden_meshes: Vec<usize>,
    pub recoil: [f32; 3],
    /// Recoil character: climb kept on full auto, return (1/s), lean (degrees).
    pub recoil_feel: [f32; 3],
    pub rounds_per_second: f32,
    pub automatic: bool,
    pub slide_locks: bool,
    pub mag_size: Option<u32>,
    pub mass: f32,
    pub mag_mass: f32,
    pub left_side_up: bool,
    pub ads_zoom: f32,
    /// How it's worked (see `rigs::Action`), and the parts that move for it (model space).
    pub action: asset::rigs::Action,
    pub hinge: Option<Vec3>,
    pub open_deg: f32,
    pub barrel_bones: Vec<String>,
    pub chamber_bones: Vec<String>,
    pub round_bones: Vec<String>,
    pub bolt_turn: Option<[f32; 2]>,
    pub handle_swing: Option<asset::rigs::HandleSwing>,
    pub pressure: Option<asset::rigs::Pressure>,
}

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
            round_bones: g.round_bones.clone(),
            bolt_turn: g.bolt_turn,
            handle_swing: g.handle_swing,
            pressure: g.pressure.clone(),
        }
    }
}

/// Metro 2033's rigs (`content/rigs/metro2033.toml`, or `RUSTVR_RIGS`'s), read once.
pub fn metro_rigs() -> &'static [MetroRig] {
    static RIGS: std::sync::OnceLock<Vec<MetroRig>> = std::sync::OnceLock::new();
    RIGS.get_or_init(|| {
        let file = asset::rigs::load("metro2033").unwrap_or_else(|e| {
            tracing::warn!("guns: {e:#}: using the compiled-in rigs");
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
                tracing::warn!("guns: {}: {e:#}", item.key);
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
                tracing::info!("guns: {} has no recoil of its own ({e:#}): the runtime's", item.key);
                Vec::new()
            }
        };
        if let Some(r) = metro_rigs().iter().find(|r| r.key == item.key) {
            let mut w = rigged_metro_weapon(r, item, &model, hands, arms.as_ref(), rest.as_ref(), id);
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
            glass: Vec::new(),
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
            handguard: None,
            trigger_blade: Vec::new(),
            game_arms: None,
            caliber: String::new(),
            magazine: item.key.clone(),
            action: WeaponAction::default(),
            laser: None,
        });
    }
    out
}

/// One properly rigged Metro gun (see [`metro_rigs`]) from the game's folder in the asset cache
/// (`local_assets/metro2033`, as `metro-extract` writes it): exactly what [`metro_weapons`] makes of
/// it, its parts cut from the model ([`MeshData::source`]) and the game's recoil with it.
pub fn metro_weapon(dir: &std::path::Path, key: &str) -> anyhow::Result<Weapon> {
    use anyhow::Context;
    let index = asset::load_index(dir).with_context(|| format!("the asset index in {}", dir.display()))?;
    let item = index.items.iter().find(|i| i.key == key && i.kind == asset::ItemKind::Weapon).with_context(|| format!("{key} isn't a gun in {}", dir.display()))?;
    let r = metro_rigs().iter().find(|r| r.key == key).with_context(|| format!("{key} isn't rigged in content/rigs/metro2033.toml"))?;
    let model = asset::load_model(dir, key)?;
    let arms = item.hands.as_ref().and_then(|k| asset::load_model(dir, k).ok());
    let hands = arms.as_ref().map(model_meshes).unwrap_or_default();
    let rest = asset::load_model(dir, ARMS_REST).ok();
    let mut w = rigged_metro_weapon(r, item, &model, hands, arms.as_ref(), rest.as_ref(), 100);
    w.kicks = asset::load_recoil(dir, key).map(|r| r.kicks).unwrap_or_default();
    Ok(w)
}

/// `arms`: the arms the index names for the gun (the plain suit for a gun the game poses none for).
fn rigged_metro_weapon(r: &MetroRig, item: &asset::Item, model: &asset::Model, hands: Vec<MeshData>, arms: Option<&asset::Model>, rest: Option<&asset::Model>, id: u64) -> Weapon {
    // The arms posed for this gun (not the plain suit every unposed gun falls back to).
    let arms = arms.filter(|_| item.hands.as_deref() != Some(ARMS_REST));
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
    let round_bones: Vec<[&str; 1]> = r.round_bones.iter().map(|b| [b.as_str()]).collect();
    // The groups, in order: magazine, bolt, trigger, hidden, glass, barrels, gauge, then each
    // chamber's round. (A bone goes to the group that names it or its nearest named ancestor:
    // rounds under the barrels are rounds.)
    let mut list = vec![
        Group { bones: &mag_bones, meshes: &r.mag_meshes },
        Group { bones: &bolt_bones, meshes: &[] },
        Group { bones: &trigger_bone, meshes: &[] },
        Group { bones: &hidden, meshes: &[] },
        Group { bones: &[], meshes: &r.hidden_meshes },
        Group { bones: &barrel_bones, meshes: &[] },
        Group { bones: &gauge_bone, meshes: &[] },
    ];
    list.extend(chamber_bones.iter().chain(&round_bones).map(|b| Group { bones: b, meshes: &[] }));
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
    let glass = groups.next().unwrap_or_default();
    let barrel_parts = take(&mut parts, groups.next().unwrap_or_default());
    let gauge_parts = take(&mut parts, groups.next().unwrap_or_default());
    let mut loose: Vec<Vec<usize>> = groups.map(|g| take(&mut parts, g)).collect();
    let round_parts = loose.split_off(r.chamber_bones.len().min(loose.len()));
    let chamber_parts = loose;
    // How it's worked, in weapon space.
    let muzzle_w = p(r.muzzle);
    let mut action = WeaponAction { barrel_parts, chamber_parts, round_parts, ..Default::default() };
    action.port = r.round_bones.first().map(|b| bone_position(model, b).map_or(p(r.mag_well), p));
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
        glass,
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
        handguard: None,
        trigger_blade: bone_triangles(model, &r.trigger_bone, fix),
        game_arms: arms.map(|a| std::sync::Arc::new(GameArms { posed: a.clone(), rest: rest.cloned() })),
        caliber: r.caliber.clone(),
        magazine: r.magazine.clone(),
        action,
        laser: bone_position(model, "loc_lazer").map(p),
    }
}

/// A bone's model-space position, by name.
pub fn bone_position(model: &asset::Model, name: &str) -> Option<Vec3> {
    let b = model.bones.iter().find(|b| b.name == name)?;
    Some(Mat4::from_cols_array(&b.model_matrix).w_axis.truncate())
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
        let source = Some(MeshSource { mesh: index, vertices: Vec::new() });
        let mut split: Vec<MeshData> = vec![MeshData { texture: texture.clone(), source, ..Default::default() }; groups.len() + 1];
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
                    if let Some(s) = &mut split[g].source {
                        s.vertices.push(i);
                    }
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

pub fn model_meshes(m: &asset::Model) -> Vec<MeshData> {
    m.meshes
        .iter()
        .enumerate()
        .filter(|(_, mesh)| !mesh.indices.is_empty())
        .map(|(index, mesh)| MeshData {
            vertices: mesh
                .vertices
                .iter()
                .map(|v| Vertex { pos: v.pos, normal: v.normal, uv: v.uv, color: [255, 255, 255, 255] })
                .collect(),
            indices: mesh.indices.clone(),
            texture: if mesh.texture.is_empty() { None } else { Some(mesh.texture.clone()) },
            source: Some(MeshSource { mesh: index, vertices: (0..mesh.vertices.len() as u32).collect() }),
        })
        .collect()
}

/// A properly rigged Metro gun ready to register with the runtime ([`crate::desc::registration`]):
/// its description in weapon space (the line of sight along +Z, the grip at the origin, as the
/// rig and the gun's model make it), its magazine template, Metro's own hands on it (its
/// `hands_<gun>` arms) and its recoil. The runtime draws it and its magazines itself
/// ([`OBJF_RUNTIME_DRAWS`]) from `<dir's name>/<key>` in its asset cache, with the game's lighting.
/// `dir` is the game's folder in the asset cache (`local_assets/metro2033`). The gun and its
/// magazine get `ammo_type` 0x101 (one caliber, one magazine).
pub fn metro_gun(dir: &std::path::Path, key: &str, ids: crate::desc::GunIds) -> anyhow::Result<(Weapon, crate::desc::GunRegistration)> {
    let w = metro_weapon(dir, key)?;
    let game = dir.file_name().and_then(|n| n.to_str()).unwrap_or("metro2033").to_string();
    let reg = crate::desc::registration(&w, ids, 1 << 8 | 1, Some(&game));
    Ok((w, reg))
}

/// The [`ObjectDesc`] of [`metro_gun`] alone, with ids 1 (the gun), 2 (its magazine) and hand
/// poses 1 (firing) and 2 (support).
pub fn metro_gun_desc(dir: &std::path::Path, key: &str) -> anyhow::Result<ObjectDesc> {
    Ok(metro_gun(dir, key, crate::desc::GunIds { gun: 1, magazine: 2, grip_pose: 1, support_pose: 2 })?.1.desc)
}
