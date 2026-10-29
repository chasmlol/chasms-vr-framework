//! Objects the runtime draws itself: what an adapter registers with [`OBJF_RUNTIME_DRAWS`] (a gun
//! the player holds, its magazines; one of the game's props while a hand has it). The game draws
//! the world; the player, and what the player holds, are the runtime's: drawn with the body by the
//! avatar ([`rustvr_avatar::HeldModel`]), in the same depth and with the game's own materials and
//! lighting, wherever the rig has them.
//!
//! The model is the one the object's name gives (`<game>/<asset key>`, `rustvr_guns::desc::
//! model_name`) in the runtime's asset cache, rigged by `content/rigs/<game>.toml`: exactly the gun
//! the adapter registered (`rustvr_guns`), so weapon space is the same on both sides. Its moving
//! parts go where the object's state puts them, as the test range draws its own guns
//! (`rustvr_guns::desc::object_part_matrices`): the bolt or handle back with `slide`, the trigger
//! under the finger pulling it, the rounds a magazine holds, the magazine where its own object is.
//! The game's effect shader on one ([`Overlays`]: an enchanted weapon's shimmer) is drawn over it.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use rustvr_avatar::{HeldDraw, HeldModel, HeldPart, OverlayDraw};
use rustvr_gpu::Gpu;
use rustvr_guns::{PartsState, Weapon};
use rustvr_protocol::*;

use crate::rig::WorldMap;

/// A model loaded for drawing: the gun it is (its parts and how they move; None: a prop, one
/// piece, or a valve, two: what stays put and what turns) and its GPU model.
pub struct Loaded {
    pub weapon: Option<Weapon>,
    pub model: HeldModel,
    /// Each part as boxes (model space): the framework view's low-poly gun ([`crate::blockout`]).
    pub blocks: Vec<Vec<(Vec3, Vec3)>>,
    /// A valve: the part that turns, as points (model space, a centimetre apart at most): where a
    /// hand can take it.
    pub turning: Vec<Vec3>,
}

/// One object to draw this frame: its model (by name) and each part's place: in tracking space
/// (held or holstered: it goes with the player), or in the game's world (`world`: lying about),
/// placed into tracking space for each picture by that picture's own mapping
/// ([`HeldObjects::place`]): where the game was when it drew the picture the object is drawn over,
/// so it stays put on the game's floor however the player moves.
struct Placed {
    id: u32,
    name: String,
    parts: Vec<Option<Mat4>>,
    world: bool,
    /// The hand holding it.
    hand: Option<usize>,
}

#[derive(Default)]
pub struct HeldObjects {
    /// By model name; None: it failed to load (said once).
    models: HashMap<String, Option<Loaded>>,
    placed: Vec<Placed>,
    /// Each placed object's parts in tracking space, for the picture being drawn ([`HeldObjects::place`]).
    parts_now: Vec<Vec<Option<Mat4>>>,
    /// Props a hand had lately: drawn until then ([`RUNTIME_DRAW_TAIL`] after the hand lets go,
    /// while the game shows its own again).
    props_until: HashMap<u32, f64>,
    /// The game's effect shaders over what it draws, their textures by name (None: failed, said
    /// once), and the time they run on (seconds).
    overlays: Vec<Overlay>,
    overlay_textures: HashMap<String, Option<windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView>>,
    overlays_said: Vec<(u32, u32)>,
    now: f64,
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if !q.is_finite() || q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

/// The parts a held model is drawn in: a gun's `parts` (cut from the game's meshes), then its
/// glass (see [`Weapon::glass`]).
pub fn held_parts(w: &Weapon) -> Vec<HeldPart> {
    w.parts
        .iter()
        .chain(&w.glass)
        .enumerate()
        .filter_map(|(part, m)| {
            let s = m.source.as_ref()?;
            Some(HeldPart { part, mesh: s.mesh, vertices: s.vertices.clone(), indices: m.indices.clone() })
        })
        .collect()
}

/// Which of `bones` are `bone` or hang from it (the part of a mechanism that turns with it).
pub fn on_bone(bones: &[rustvr_asset::Bone], bone: usize) -> Vec<bool> {
    let mut on = vec![false; bones.len()];
    // Parents may come after their children: settle it in as many passes as it takes.
    for _ in 0..bones.len() {
        let mut changed = false;
        for (i, b) in bones.iter().enumerate() {
            let now = i == bone || (b.parent >= 0 && on.get(b.parent as usize).copied().unwrap_or(false));
            if now && !on[i] {
                on[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    on
}

/// A valve's model in two parts: 0, what stays put; 1, the triangles on its turning bone (`turns`:
/// each bone, whether it turns), by the bone each corner leans on most (two corners of three
/// decide).
pub fn valve_parts(model: &rustvr_asset::Model, turns: &[bool]) -> Vec<HeldPart> {
    let mut out = Vec::new();
    for (mesh, m) in model.meshes.iter().enumerate() {
        let turning = |v: u32| {
            let v = &m.vertices[v as usize];
            let k = (0..4).max_by_key(|&k| v.weights[k]).unwrap_or(0);
            v.weights[k] > 0 && turns.get(v.bones[k] as usize).copied().unwrap_or(false)
        };
        let (mut still, mut moving) = (Vec::new(), Vec::new());
        for t in m.indices.chunks_exact(3) {
            let n = t.iter().filter(|&&v| turning(v)).count();
            if n >= 2 { moving.extend_from_slice(t) } else { still.extend_from_slice(t) }
        }
        let all: Vec<u32> = (0..m.vertices.len() as u32).collect();
        for (part, indices) in [(0, still), (1, moving)] {
            if !indices.is_empty() {
                out.push(HeldPart { part, mesh, vertices: all.clone(), indices });
            }
        }
    }
    out
}

/// How far apart (metres) the points a hand can take a valve's turning part by are, at most.
const GRIP_SPACING: f32 = 0.01;

/// The corners of `part`'s triangles (model space), one per `spacing` cube.
pub fn part_points(model: &rustvr_asset::Model, parts: &[HeldPart], part: usize, spacing: f32) -> Vec<Vec3> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in parts.iter().filter(|p| p.part == part) {
        let mesh = &model.meshes[p.mesh];
        for &i in &p.indices {
            let v = Vec3::from_array(mesh.vertices[p.vertices[i as usize] as usize].pos);
            if seen.insert((v / spacing).floor().as_ivec3().to_array()) {
                out.push(v);
            }
        }
    }
    out
}

/// Loads `name` (`<game>/<key>`) from the asset cache under `root`, for an object of `kind`
/// (a valve's: `turn_bone`, the bone that turns, [`ObjectDesc::turn_bone`]).
pub fn load(gpu: &Gpu, root: &std::path::Path, name: &str, kind: u32, turn_bone: u32) -> anyhow::Result<Loaded> {
    use anyhow::Context;
    let (game, key) = rustvr_guns::desc::parse_model_name(name).with_context(|| format!("{name:?} isn't <game>/<asset key>"))?;
    if kind == OBJ_VALVE || kind == OBJ_DRIVEN {
        // A mechanism: what stays put, and what the hand turns (or the valve moves).
        let dir = root.join(game);
        let model = rustvr_asset::load_model(&dir, key)?;
        anyhow::ensure!((turn_bone as usize) < model.bones.len(), "{name} has no bone {turn_bone}");
        let parts = valve_parts(&model, &on_bone(&model.bones, turn_bone as usize));
        anyhow::ensure!(parts.iter().any(|p| p.part == 1), "nothing of {name} is on its bone {}", model.bones[turn_bone as usize].name);
        let turning = part_points(&model, &parts, 1, GRIP_SPACING);
        let model = HeldModel::load(gpu, &dir, &model, &parts)?;
        return Ok(Loaded { weapon: None, model, blocks: Vec::new(), turning });
    }
    if kind == OBJ_PROP || kind == OBJ_MELEE {
        // A prop or a melee weapon: the whole model, one piece.
        let dir = root.join(game);
        let model = rustvr_asset::load_model(&dir, key)?;
        let parts: Vec<HeldPart> = model
            .meshes
            .iter()
            .enumerate()
            .map(|(mesh, m)| HeldPart { part: 0, mesh, vertices: (0..m.vertices.len() as u32).collect(), indices: m.indices.clone() })
            .collect();
        let model = HeldModel::load(gpu, &dir, &model, &parts)?;
        return Ok(Loaded { weapon: None, model, blocks: Vec::new(), turning: Vec::new() });
    }
    // (Rigs are Metro 2033's so far: `content/rigs/metro2033.toml`.)
    anyhow::ensure!(game == "metro2033", "no rigs for {game}'s guns yet");
    let dir = root.join(game);
    let weapon = rustvr_guns::metro::metro_weapon(&dir, key)?;
    let model = rustvr_asset::load_model(&dir, key)?;
    let model = HeldModel::load(gpu, &dir, &model, &held_parts(&weapon))?;
    let blocks = weapon.parts.iter().map(|m| crate::blockout::blockout(&m.vertices.iter().map(|v| Vec3::from_array(v.pos)).collect::<Vec<_>>(), &m.indices)).collect();
    Ok(Loaded { weapon: Some(weapon), model, blocks, turning: Vec::new() })
}

/// Where a prop's model sits on it ([`ObjectDesc::model_pos`], `model_rot`).
pub fn model_place(d: &ObjectDesc) -> Mat4 {
    let r = Quat::from_array(d.model_rot);
    let r = if r.is_finite() && r.length_squared() > 0.5 { r.normalize() } else { Quat::IDENTITY };
    Mat4::from_rotation_translation(r, Vec3::from_array(d.model_pos))
}

/// Where a valve's two parts go (the game's world): what stays put where the game has the model,
/// what turns turned `angle` about the valve's axle (its spawn pose's +Z).
pub fn valve_places(d: &ObjectDesc, angle: f32) -> Vec<Option<Mat4>> {
    let at = Mat4::from_rotation_translation(quat(d.spawn_pose.rot), Vec3::from_array(d.spawn_pose.pos));
    let angle = if angle.is_finite() { angle } else { 0.0 };
    vec![Some(at * model_place(d)), Some(at * Mat4::from_rotation_z(angle) * model_place(d))]
}

/// Where a driven thing's two parts go (the game's world): what stays put where the game has the
/// model, what moves (a gate) `drive_travel` along as far as its valve has turned towards
/// `drive_turn` (`angle`).
pub fn driven_places(d: &ObjectDesc, angle: f32) -> Vec<Option<Mat4>> {
    let share = if d.drive_turn > 0.0 && angle.is_finite() { (angle / d.drive_turn).clamp(0.0, 1.0) } else { 0.0 };
    driven_places_at(d, share)
}

/// The same, its moving part `share` of the way along `drive_travel`.
fn driven_places_at(d: &ObjectDesc, share: f32) -> Vec<Option<Mat4>> {
    let at = Mat4::from_rotation_translation(quat(d.spawn_pose.rot), Vec3::from_array(d.spawn_pose.pos));
    let moved = Mat4::from_translation(Vec3::from_array(d.drive_travel) * share);
    vec![Some(at * model_place(d)), Some(at * moved * model_place(d))]
}

/// How far along `drive_travel` (share of the way) the game's pose `p` of a driven thing has its
/// moving part.
fn driven_share_at(d: &ObjectDesc, p: Vec3) -> f32 {
    let travel = Vec3::from_array(d.drive_travel);
    let local = quat(d.spawn_pose.rot).inverse() * (p - Vec3::from_array(d.spawn_pose.pos));
    if travel.length_squared() < 1e-8 { 0.0 } else { (local.dot(travel) / travel.length_squared()).clamp(0.0, 1.0) }
}

/// Where each part of an object of `w` goes (tracking space), for its description's kind, its
/// state and the holding hand's trigger; `object` is its pose's matrix.
pub fn part_places(w: &Weapon, kind: u32, s: &ObjectState, trigger: f32, object: Mat4) -> Vec<Option<Mat4>> {
    let state = PartsState { slide: s.slide, chambered: s.chambered, fuse: s.fuse, trigger, rounds: s.rounds };
    let mut parts = rustvr_guns::desc::object_part_matrices(w, kind, &state, object);
    // The glass rides on the gun's body.
    parts.extend(w.glass.iter().map(|_| (kind == OBJ_WEAPON).then_some(object * w.fix)));
    parts
}

impl HeldObjects {
    /// Loads the models the adapter's table names for the runtime to draw (each once; a model that
    /// fails is said once and skipped). Call when the table changes.
    pub fn sync(&mut self, gpu: &Gpu, objects: &ObjectTable) {
        let root = rustvr_asset::default_root();
        let descs = &objects.descs[..(objects.count as usize).min(MAX_OBJECTS)];
        for d in descs.iter().filter(|d| d.id != 0 && d.flags & OBJF_RUNTIME_DRAWS != 0) {
            let name = name_str(&d.name).to_string();
            if self.models.contains_key(&name) {
                continue;
            }
            let loaded = match load(gpu, &root, &name, d.kind, d.turn_bone) {
                Ok(l) => {
                    tracing::info!("runtime: drawing {name} ({} parts)", l.model.parts());
                    Some(l)
                }
                Err(e) => {
                    tracing::warn!("runtime: can't draw {name}: {e:#}");
                    None
                }
            };
            self.models.insert(name, loaded);
        }
    }

    /// The part of each valve the runtime draws that a hand takes it by: its description's id and
    /// the points of its turning part in the valve's frame.
    pub fn valve_shapes(&self, objects: &ObjectTable) -> Vec<(u32, Vec<Vec3>)> {
        let descs = &objects.descs[..(objects.count as usize).min(MAX_OBJECTS)];
        descs
            .iter()
            .filter(|d| d.id != 0 && d.kind == OBJ_VALVE && d.flags & OBJF_RUNTIME_DRAWS != 0)
            .filter_map(|d| {
                let l = self.models.get(name_str(&d.name))?.as_ref()?;
                let m = model_place(d);
                Some((d.id, l.turning.iter().map(|&p| m.transform_point3(p)).collect()))
            })
            .collect()
    }

    /// Places this frame's objects from the rig's states: held and holstered ones in tracking space,
    /// the rest in the game's world (placed for each picture, [`HeldObjects::place`]). `map`: the
    /// runtime's mapping now, which places them until then. `triggers`: each controller's
    /// trigger (a held gun's trigger follows its hand's). The game's props only while a hand has
    /// them ([`OS_RUNTIME_DRAWN`], and a moment after); `now` in seconds.
    pub fn update(&mut self, objects: &ObjectTable, states: &ObjectStates, game: &GamePoses, map: Option<&WorldMap>, triggers: [f32; 2], now: f64) {
        self.placed.clear();
        self.now = now;
        self.props_until.retain(|_, until| *until > now);
        let descs = &objects.descs[..(objects.count as usize).min(MAX_OBJECTS)];
        for s in &states.states[..(states.count as usize).min(MAX_OBJECTS)] {
            if s.id == 0 || s.flags & OS_SPENT != 0 {
                continue;
            }
            let Some(d) = descs.iter().find(|d| d.id == s.desc && d.id != 0 && d.flags & OBJF_RUNTIME_DRAWS != 0) else { continue };
            let name = name_str(&d.name);
            let Some(Some(l)) = self.models.get(name) else { continue };
            if d.kind == OBJ_PROP {
                if s.flags & OS_RUNTIME_DRAWN != 0 {
                    self.props_until.insert(s.id, now + RUNTIME_DRAW_TAIL as f64);
                }
                if !self.props_until.contains_key(&s.id) {
                    continue;
                }
            }
            let (pos, rot) = (Vec3::from_array(s.pose.pos), quat(s.pose.rot));
            let world = s.flags & OS_TRACKING == 0;
            let hand = match s.flags & (OS_HELD_LEFT | OS_HELD_RIGHT) {
                OS_HELD_LEFT => Some(0),
                OS_HELD_RIGHT => Some(1),
                _ => None,
            };
            let trigger = hand.map_or(0.0, |h| triggers[h]);
            let object = Mat4::from_rotation_translation(rot, pos);
            let parts = match &l.weapon {
                Some(w) => part_places(w, d.kind, s, trigger, object),
                // A valve: its model where the game has it, the turning part turned as far as it is.
                None if d.kind == OBJ_VALVE => valve_places(d, s.slide),
                // A prop: its model where the game draws it on the object.
                None => vec![Some(object * model_place(d))],
            };
            self.placed.push(Placed { id: s.id, name: name.to_string(), parts, world, hand });
        }
        // What the valves work (a gate), with them.
        for d in descs.iter().filter(|d| d.id != 0 && d.kind == OBJ_DRIVEN && d.flags & OBJF_RUNTIME_DRAWS != 0) {
            let name = name_str(&d.name);
            if !matches!(self.models.get(name), Some(Some(_))) {
                continue;
            }
            // Where the game has it (it moves it some itself), or as far as its valve is turned.
            let given = (d.flags & OBJF_GAME_MOVED != 0)
                .then(|| game.poses[..(game.count as usize).min(MAX_OBJECTS)].iter().find(|p| p.id == d.id && p.pose.valid != 0))
                .flatten();
            let parts = match given {
                Some(p) => driven_places_at(d, driven_share_at(d, Vec3::from_array(p.pose.pos))),
                None => {
                    let angle = states.states[..(states.count as usize).min(MAX_OBJECTS)].iter().find(|s| s.desc == d.driven_by && s.id != 0).map_or(0.0, |s| s.slide);
                    driven_places(d, angle)
                }
            };
            self.placed.push(Placed { id: d.id, name: name.to_string(), parts, world: true, hand: None });
        }
        self.place(map);
    }

    /// The game's effect shaders over what it draws, as the adapter last said (their textures loaded
    /// once from the asset cache).
    pub fn set_overlays(&mut self, gpu: &Gpu, overlays: &Overlays) {
        let root = rustvr_asset::default_root();
        self.overlays = overlays.entries().to_vec();
        for o in &self.overlays {
            let name = o.texture().to_string();
            if name.is_empty() || self.overlay_textures.contains_key(&name) {
                continue;
            }
            let loaded = rustvr_guns::desc::parse_model_name(&name)
                .ok_or_else(|| anyhow::anyhow!("{name:?} isn't <game>/<asset key>"))
                .and_then(|(game, key)| rustvr_avatar::overlay_texture(gpu, &root.join(game), key))
                .map_err(|e| tracing::warn!("runtime: an overlay's texture {name}: {e:#}"))
                .ok();
            self.overlay_textures.insert(name, loaded);
        }
        // (Said as what's overlaid changes: a body's overlay fades every frame.)
        let what: Vec<(u32, u32)> = self.overlays.iter().map(|o| (o.object, o.flags)).collect();
        if what != self.overlays_said {
            tracing::info!("runtime: {} overlays: {:?}", self.overlays.len(), self.overlays.iter().map(|o| (o.object, o.flags, o.fill, o.edge, o.edge_falloff)).collect::<Vec<_>>());
            self.overlays_said = what;
        }
    }

    /// Places the objects lying in the game's world into tracking space by `map`: the mapping of the
    /// picture they're drawn over (each eye's own, `SlotInfo::eye_map`), so they sit exactly where
    /// that picture shows the world. None: they aren't drawn.
    pub fn place(&mut self, map: Option<&WorldMap>) {
        let to_tracking = map.map(|m| m.matrix().inverse());
        self.parts_now.clear();
        for p in &self.placed {
            let parts = match (p.world, to_tracking) {
                (false, _) => p.parts.clone(),
                (true, Some(m)) => p.parts.iter().map(|q| q.map(|q| m * q)).collect(),
                (true, None) => vec![None; p.parts.len()],
            };
            self.parts_now.push(parts);
        }
    }

    /// Nothing to draw this frame (no game, or the rig isn't running).
    pub fn clear(&mut self) {
        self.placed.clear();
        self.parts_now.clear();
    }

    /// This frame's objects as the protocol carries them to an adapter that draws the player
    /// ([`RtPlayerDraw`]): the model's name and each part's place.
    pub fn drawn(&self) -> Vec<DrawnObject> {
        self.placed
            .iter()
            .zip(&self.parts_now)
            .take(DRAWN_OBJECTS_MAX)
            .map(|(p, parts)| {
                let mut o: DrawnObject = bytemuck::Zeroable::zeroed();
                let n = p.name.len().min(o.name.len() - 1);
                o.name[..n].copy_from_slice(&p.name.as_bytes()[..n]);
                o.part_count = parts.len().min(DRAWN_PARTS_MAX) as u32;
                for (k, m) in parts.iter().enumerate().take(DRAWN_PARTS_MAX) {
                    o.set_part(k, m.map(|m| [0, 1, 2].map(|r| m.row(r).to_array())));
                }
                o
            })
            .collect()
    }

    /// This frame's objects as the framework view draws them (tracking space): each part's boxes,
    /// coloured by what it is (the magazine, the bolt or charging handle, the trigger, the rest).
    pub fn framework_triangles(&self, out: &mut Vec<([Vec3; 3], [f32; 3])>) {
        for (p, parts) in self.placed.iter().zip(&self.parts_now) {
            let Some(Some(l)) = self.models.get(&p.name) else { continue };
            let Some(w) = &l.weapon else { continue };
            for (i, blocks) in l.blocks.iter().enumerate() {
                let Some(Some(m)) = parts.get(i) else { continue };
                let color = if w.mag_parts.contains(&i) {
                    [0.95, 0.78, 0.2]
                } else if w.bolt_parts.contains(&i) {
                    [1.0, 0.5, 0.12]
                } else if w.trigger_parts.contains(&i) {
                    [0.9, 0.25, 0.2]
                } else {
                    [0.38, 0.42, 0.5]
                };
                for &(lo, hi) in blocks {
                    for t in crate::blockout::box_triangles(lo, hi, |v| m.transform_point3(v)) {
                        out.push((t, color));
                    }
                }
            }
        }
    }

    /// The model loaded for `name` (`<game>/<key>`).
    pub fn model(&self, name: &str) -> Option<&HeldModel> {
        self.models.get(name)?.as_ref().map(|l| &l.model)
    }

    /// The hand holding each of [`HeldObjects::drawn`]'s objects (None: none, it's holstered or
    /// loose).
    pub fn drawn_hands(&self) -> Vec<Option<usize>> {
        self.placed.iter().take(DRAWN_OBJECTS_MAX).map(|p| p.hand).collect()
    }

    /// The game's effect shaders over the player's own body ([`OVERLAY_BODY`]) now.
    pub fn body_overlays(&self) -> Vec<OverlayDraw> {
        self.overlays
            .iter()
            .filter(|o| o.object == OVERLAY_BODY)
            .map(|o| OverlayDraw { overlay: *o, texture: self.overlay_textures.get(o.texture()).cloned().flatten(), time: (self.now % 3600.0) as f32 })
            .collect()
    }

    /// This frame's draws, as last placed ([`HeldObjects::update`], [`HeldObjects::place`]).
    pub fn draws(&self) -> Vec<HeldDraw<'_>> {
        self.placed
            .iter()
            .zip(&self.parts_now)
            .filter_map(|(p, parts)| {
                let l = self.models.get(&p.name)?.as_ref()?;
                let overlay = self.overlays.iter().find(|o| o.object == p.id).map(|o| OverlayDraw {
                    overlay: *o,
                    texture: self.overlay_textures.get(o.texture()).cloned().flatten(),
                    time: (self.now % 3600.0) as f32,
                });
                Some(HeldDraw { model: &l.model, parts, key: &p.name, overlay })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gate a valve winds up: its moving part goes up as far as the valve has turned of the way.
    #[test]
    fn a_driven_gate_goes_up_with_its_valve() {
        let mut d: ObjectDesc = bytemuck::Zeroable::zeroed();
        d.kind = OBJ_DRIVEN;
        d.spawn_pose = Pose { pos: [10.0, 0.0, 0.0], valid: 1, rot: Quat::IDENTITY.to_array() };
        d.model_rot = Quat::IDENTITY.to_array();
        (d.drive_turn, d.drive_travel) = (4.0, [0.0, 1.7, 0.0]);
        let y = |angle: f32| driven_places(&d, angle)[1].unwrap().transform_point3(Vec3::ZERO).y;
        assert!(y(0.0).abs() < 1e-6 && (y(2.0) - 0.85).abs() < 1e-5 && (y(9.0) - 1.7).abs() < 1e-5 && y(-1.0).abs() < 1e-6);
        assert_eq!(driven_places(&d, 2.0)[0].unwrap().transform_point3(Vec3::ZERO), Vec3::new(10.0, 0.0, 0.0), "its frame stays put");
    }

    /// A switch: a box (bone 0) and a knob on bone 1, with a pointer on bone 2 hung from the knob.
    /// The knob and what hangs from it turn; the box stays.
    #[test]
    fn a_valve_is_drawn_in_what_stays_and_what_turns() {
        let bone = |name: &str, parent: i32| rustvr_asset::Bone { name: name.into(), parent, ..Default::default() };
        let bones = vec![bone("box", -1), bone("pointer", 2), bone("knob", 0)];
        assert_eq!(on_bone(&bones, 2), vec![false, true, true], "the knob and the pointer on it");
        let v = |b: u8| rustvr_asset::Vertex { bones: [b, 0, 0, 0], weights: [255, 0, 0, 0], ..Default::default() };
        let mesh = rustvr_asset::Mesh { vertices: vec![v(0), v(0), v(0), v(2), v(1), v(2)], indices: vec![0, 1, 2, 3, 4, 5, 0, 3, 4], ..Default::default() };
        let model = rustvr_asset::Model { bones, meshes: vec![mesh], ..Default::default() };
        let parts = valve_parts(&model, &on_bone(&model.bones, 2));
        let of = |p: usize| parts.iter().find(|h| h.part == p).map(|h| h.indices.clone()).unwrap_or_default();
        assert_eq!(of(0), vec![0, 1, 2], "the box");
        assert_eq!(of(1), vec![3, 4, 5, 0, 3, 4], "the knob, and a triangle mostly on it");
        // Turned: the knob's part turns about the axle, the box's doesn't.
        let mut d: ObjectDesc = bytemuck::Zeroable::zeroed();
        d.spawn_pose = Pose { pos: [1.0, 2.0, 3.0], valid: 1, rot: Quat::IDENTITY.to_array() };
        d.model_rot = Quat::IDENTITY.to_array();
        let places = valve_places(&d, std::f32::consts::FRAC_PI_2);
        let p = |k: usize| places[k].unwrap().transform_point3(Vec3::X);
        assert!(p(0).distance(Vec3::new(2.0, 2.0, 3.0)) < 1e-5);
        assert!(p(1).distance(Vec3::new(1.0, 3.0, 3.0)) < 1e-5, "a quarter turn about +Z");
    }

    /// The Kalash 2012 as an adapter registers it: the runtime reads the model from its name, and
    /// places the parts as the range does (the magazine with its own object, the glass on the body,
    /// the trigger under the finger).
    #[test]
    fn a_registered_gun_names_its_model_and_moves_its_parts() {
        let dir = rustvr_asset::default_root().join("metro2033");
        let ids = rustvr_guns::desc::GunIds { gun: 7, magazine: 8, grip_pose: 3, support_pose: 4 };
        let Ok((w, reg)) = rustvr_guns::metro::metro_gun(&dir, "wpn_2012", ids) else {
            eprintln!("no Metro 2033 assets: skipped");
            return;
        };
        for d in [Some(&reg.desc), reg.magazine.as_ref()].into_iter().flatten() {
            assert!(d.flags & OBJF_RUNTIME_DRAWS != 0);
            assert_eq!(rustvr_guns::desc::parse_model_name(name_str(&d.name)), Some(("metro2033", "wpn_2012")));
        }
        let m = reg.magazine.expect("a magazine template");
        assert_eq!((m.ammo_type, m.capacity, m.flags & OBJF_TEMPLATE != 0), (reg.desc.ammo_type, 40, true));
        assert_eq!((reg.desc.grip_pose, reg.desc.support_pose, reg.poses.len()), (3, 4, 2), "Metro's two hands on it");
        assert!(!reg.kicks.is_empty(), "its own recoil");
        let at = Mat4::from_translation(Vec3::new(0.2, 1.3, 0.4));
        let s: ObjectState = bytemuck::Zeroable::zeroed();
        let gun = part_places(&w, OBJ_WEAPON, &s, 0.0, at);
        let mag = part_places(&w, OBJ_MAGAZINE, &s, 0.0, at);
        assert_eq!(gun.len(), w.parts.len() + w.glass.len());
        for i in 0..w.parts.len() {
            let is_mag = w.mag_parts.contains(&i);
            assert_eq!((gun[i].is_some(), mag[i].is_some()), (!is_mag, is_mag), "part {i}: the gun or its magazine draws it");
        }
        for k in 0..w.glass.len() {
            assert_eq!(gun[w.parts.len() + k], Some(at * w.fix), "the glass rides on the body");
            assert_eq!(mag[w.parts.len() + k], None);
        }
        let pulled = part_places(&w, OBJ_WEAPON, &s, 1.0, at);
        let t = w.trigger_parts[0];
        assert!(pulled[t] != gun[t] && pulled[0] == gun[0], "the trigger swings with the finger, the body stays");
    }
}
