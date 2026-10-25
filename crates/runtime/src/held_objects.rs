//! Objects the runtime draws itself: what an adapter registers with [`OBJF_RUNTIME_DRAWS`] (a gun
//! the player holds, its magazines). The game draws the world; the player, and what the player
//! holds, are the runtime's: drawn with the body by the avatar ([`rustvr_avatar::HeldModel`]), in
//! the same depth and with the game's own materials and lighting, wherever the rig has them.
//!
//! The model is the one the object's name gives (`<game>/<asset key>`, `rustvr_guns::desc::
//! model_name`) in the runtime's asset cache, rigged by `content/rigs/<game>.toml`: exactly the gun
//! the adapter registered (`rustvr_guns`), so weapon space is the same on both sides. Its moving
//! parts go where the object's state puts them, as the test range draws its own guns
//! (`rustvr_guns::desc::object_part_matrices`): the bolt or handle back with `slide`, the trigger
//! under the finger pulling it, the rounds a magazine holds, the magazine where its own object is.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use rustvr_avatar::{HeldDraw, HeldModel, HeldPart};
use rustvr_gpu::Gpu;
use rustvr_guns::{PartsState, Weapon};
use rustvr_protocol::*;

use crate::rig::WorldMap;

/// A model loaded for drawing: the gun it is (its parts and how they move) and its GPU model.
pub struct Loaded {
    pub weapon: Weapon,
    pub model: HeldModel,
    /// Each part as boxes (model space): the framework view's low-poly gun ([`crate::blockout`]).
    pub blocks: Vec<Vec<(Vec3, Vec3)>>,
}

/// One object to draw this frame: its model (by name) and each part's place in tracking space.
struct Placed {
    name: String,
    parts: Vec<Option<Mat4>>,
}

#[derive(Default)]
pub struct HeldObjects {
    /// By model name; None: it failed to load (said once).
    models: HashMap<String, Option<Loaded>>,
    placed: Vec<Placed>,
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

/// Loads `name` (`<game>/<key>`) from the asset cache under `root`.
pub fn load(gpu: &Gpu, root: &std::path::Path, name: &str) -> anyhow::Result<Loaded> {
    use anyhow::Context;
    let (game, key) = rustvr_guns::desc::parse_model_name(name).with_context(|| format!("{name:?} isn't <game>/<asset key>"))?;
    // (Rigs are Metro 2033's so far: `content/rigs/metro2033.toml`.)
    anyhow::ensure!(game == "metro2033", "no rigs for {game}'s guns yet");
    let dir = root.join(game);
    let weapon = rustvr_guns::metro::metro_weapon(&dir, key)?;
    let model = rustvr_asset::load_model(&dir, key)?;
    let model = HeldModel::load(gpu, &dir, &model, &held_parts(&weapon))?;
    let blocks = weapon.parts.iter().map(|m| crate::blockout::blockout(&m.vertices.iter().map(|v| Vec3::from_array(v.pos)).collect::<Vec<_>>(), &m.indices)).collect();
    Ok(Loaded { weapon, model, blocks })
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
            let loaded = match load(gpu, &root, &name) {
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

    /// Places this frame's objects from the rig's states: held and holstered ones in tracking space,
    /// the rest in the game's world through `map` (None: not drawn). `triggers`: each controller's
    /// trigger (a held gun's trigger follows its hand's).
    pub fn update(&mut self, objects: &ObjectTable, states: &ObjectStates, map: Option<&WorldMap>, triggers: [f32; 2]) {
        self.placed.clear();
        let descs = &objects.descs[..(objects.count as usize).min(MAX_OBJECTS)];
        for s in &states.states[..(states.count as usize).min(MAX_OBJECTS)] {
            if s.id == 0 || s.flags & OS_SPENT != 0 {
                continue;
            }
            let Some(d) = descs.iter().find(|d| d.id == s.desc && d.id != 0 && d.flags & OBJF_RUNTIME_DRAWS != 0) else { continue };
            let name = name_str(&d.name);
            let Some(Some(l)) = self.models.get(name) else { continue };
            let (pos, rot) = (Vec3::from_array(s.pose.pos), quat(s.pose.rot));
            let (pos, rot) = if s.flags & OS_TRACKING != 0 {
                (pos, rot)
            } else {
                let Some(map) = map else { continue };
                (map.inv_point(pos), map.inv_rot(rot))
            };
            let trigger = match s.flags & (OS_HELD_LEFT | OS_HELD_RIGHT) {
                OS_HELD_LEFT => triggers[0],
                OS_HELD_RIGHT => triggers[1],
                _ => 0.0,
            };
            let parts = part_places(&l.weapon, d.kind, s, trigger, Mat4::from_rotation_translation(rot, pos));
            self.placed.push(Placed { name: name.to_string(), parts });
        }
    }

    /// Nothing to draw this frame (no game, or the rig isn't running).
    pub fn clear(&mut self) {
        self.placed.clear();
    }

    /// This frame's objects as the protocol carries them to an adapter that draws the player
    /// ([`RtPlayerDraw`]): the model's name and each part's place.
    pub fn drawn(&self) -> Vec<DrawnObject> {
        self.placed
            .iter()
            .take(DRAWN_OBJECTS_MAX)
            .map(|p| {
                let mut o: DrawnObject = bytemuck::Zeroable::zeroed();
                let n = p.name.len().min(o.name.len() - 1);
                o.name[..n].copy_from_slice(&p.name.as_bytes()[..n]);
                o.part_count = p.parts.len().min(DRAWN_PARTS_MAX) as u32;
                for (k, m) in p.parts.iter().enumerate().take(DRAWN_PARTS_MAX) {
                    o.set_part(k, m.map(|m| [0, 1, 2].map(|r| m.row(r).to_array())));
                }
                o
            })
            .collect()
    }

    /// This frame's objects as the framework view draws them (tracking space): each part's boxes,
    /// coloured by what it is (the magazine, the bolt or charging handle, the trigger, the rest).
    pub fn framework_triangles(&self, out: &mut Vec<([Vec3; 3], [f32; 3])>) {
        for p in &self.placed {
            let Some(Some(l)) = self.models.get(&p.name) else { continue };
            let w = &l.weapon;
            for (i, blocks) in l.blocks.iter().enumerate() {
                let Some(Some(m)) = p.parts.get(i) else { continue };
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

    /// This frame's draws (see [`HeldObjects::update`]).
    pub fn draws(&self) -> Vec<HeldDraw<'_>> {
        self.placed
            .iter()
            .filter_map(|p| {
                let l = self.models.get(&p.name)?.as_ref()?;
                Some(HeldDraw { model: &l.model, parts: &p.parts, key: &p.name })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
