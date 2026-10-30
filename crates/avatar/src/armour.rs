//! Plates: the parts of a body model a marks file ([`rustvr_asset::armour`]) says are rigid.
//!
//! A plate's vertices all get the same weights (its riders') and the same centre of rotation (its
//! pivot), so the skinning that keeps joints' volume (`cor.rs`) turns the whole plate by one
//! rotation about one point and takes that point where its riders take it: the plate moves as one
//! piece and never bends. Riding two bones, it turns by each one's share about the joint between
//! them, which is what a game's helper joint does for a couter or a pauldron.
//!
//! A plate keeps the shape the game's own skinning gives it on a body standing at rest, the arms at
//! its sides ([`crate::rig::rest_pose`]), not the shape it's modelled in, with the arms out level:
//! a pauldron the game drapes over the shoulder by bending it stays draped, where rigid in its
//! modelled shape it would stand out like a wing with the arms down. Its vertices are put in that
//! shape and skinned to copies of its riders bound at rest (`compose`); for a plate whose riders
//! all move alike between the two poses (all on one arm, or none on an arm) that's the same as its
//! modelled shape on the riders themselves.
//!
//! Islands (the pieces of a mesh whose triangles share corners, corners at the same place being
//! the same corner) are what plates are chosen from; [`islands`] lists them as the preview shows
//! them.
//!
//! A plate is usually joined to what's next to it (a soft band, or the next plate across a
//! joint), and the corners they share bend smoothly between them. One marked to come apart at
//! its seam gets its own copy of those corners ([`cut_seams`]), so it slides over or under what it
//! meets as a real plate does.

use std::collections::{HashMap, HashSet};

use glam::{Mat4, Vec3};
use rustvr_asset as asset;
use rustvr_asset::armour::{self as marks, MeshRef, Pivot, Side};

/// Corners closer than this (metres) are the same corner.
const WELD: f32 = 1e-4;
/// A rider with less than this share of a plate's own weights is left out of its riders.
const LEAST_SHARE: f32 = 0.03;

/// An island of a mesh.
#[derive(Clone, Debug)]
pub struct Island {
    /// Its vertices (indices into the mesh), lowest first.
    pub vertices: Vec<u32>,
    /// The mean of its vertices, and its bounds (model space).
    pub centre: Vec3,
    pub min: Vec3,
    pub max: Vec3,
    /// Its bones and their shares of its skin, heaviest first.
    pub bones: Vec<(usize, f32)>,
    pub side: Side,
    pub triangles: usize,
}

impl Island {
    pub fn heaviest(&self) -> Option<usize> {
        self.bones.first().map(|b| b.0)
    }
}

/// Each vertex's weld group: vertices at the same place share one (the lowest index among them).
fn welds(mesh: &asset::Mesh) -> Vec<u32> {
    let key = |p: [f32; 3]| p.map(|c| (c / WELD).round() as i64);
    let mut first: HashMap<[i64; 3], u32> = HashMap::new();
    mesh.vertices.iter().enumerate().map(|(i, v)| *first.entry(key(v.pos)).or_insert(i as u32)).collect()
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

/// A mesh's islands, in the order of their lowest vertex.
pub fn islands(mesh: &asset::Mesh) -> Vec<Island> {
    let n = mesh.vertices.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let join = |parent: &mut Vec<u32>, a: u32, b: u32| {
        let (a, b) = (find(parent, a), find(parent, b));
        if a != b {
            parent[a.max(b) as usize] = a.min(b);
        }
    };
    for (i, w) in welds(mesh).into_iter().enumerate() {
        join(&mut parent, i as u32, w);
    }
    for t in mesh.indices.chunks_exact(3) {
        if t.iter().all(|&i| (i as usize) < n) {
            join(&mut parent, t[0], t[1]);
            join(&mut parent, t[0], t[2]);
        }
    }
    let mut by_root: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut slot: HashMap<u32, usize> = HashMap::new();
    for i in 0..n as u32 {
        let r = find(&mut parent, i);
        let s = *slot.entry(r).or_insert_with(|| {
            by_root.push((r, Vec::new()));
            by_root.len() - 1
        });
        by_root[s].1.push(i);
    }
    let mut tris = vec![0usize; by_root.len()];
    for t in mesh.indices.chunks_exact(3) {
        if let Some(&s) = t.first().filter(|&&i| (i as usize) < n).and_then(|&i| slot.get(&find(&mut parent, i))) {
            tris[s] += 1;
        }
    }
    by_root
        .into_iter()
        .zip(tris)
        .map(|((_, vertices), triangles)| {
            let (mut lo, mut hi, mut sum) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN), Vec3::ZERO);
            let mut bones: Vec<(usize, f32)> = Vec::new();
            for &i in &vertices {
                let v = &mesh.vertices[i as usize];
                let p = Vec3::from_array(v.pos);
                (lo, hi, sum) = (lo.min(p), hi.max(p), sum + p);
                add_weights(&mut bones, v, 1.0);
            }
            let total: f32 = bones.iter().map(|b| b.1).sum::<f32>().max(1e-6);
            bones.iter_mut().for_each(|b| b.1 /= total);
            bones.sort_by(|a, b| b.1.total_cmp(&a.1));
            let centre = sum / vertices.len().max(1) as f32;
            Island { vertices, centre, min: lo, max: hi, bones, side: Side::of(centre.x), triangles }
        })
        .collect()
}

/// Adds a vertex's weights (as shares of 1, times `k`) into `bones`.
fn add_weights(bones: &mut Vec<(usize, f32)>, v: &asset::Vertex, k: f32) {
    let total: u32 = v.weights.iter().map(|&w| w as u32).sum();
    if total == 0 {
        return;
    }
    for j in 0..4 {
        if v.weights[j] > 0 {
            let w = v.weights[j] as f32 / total as f32 * k;
            match bones.iter_mut().find(|b| b.0 == v.bones[j] as usize) {
                Some(b) => b.1 += w,
                None => bones.push((v.bones[j] as usize, w)),
            }
        }
    }
}

/// The bone a vertex is mostly on, and its share.
fn heaviest(v: &asset::Vertex) -> Option<(usize, f32)> {
    let total: u32 = v.weights.iter().map(|&w| w as u32).sum();
    let k = (0..4).filter(|&k| v.weights[k] > 0).max_by_key(|&k| v.weights[k])?;
    Some((v.bones[k] as usize, v.weights[k] as f32 / total.max(1) as f32))
}

/// A plate as it's applied: its vertices (of one mesh) and what they all get.
#[derive(Clone, Debug)]
pub struct Plate {
    pub name: String,
    pub side: Side,
    pub mesh: usize,
    pub vertices: Vec<u32>,
    /// The riders, as vertex weights (bone indices, 255ths summing to 255).
    pub bones: [u8; 4],
    pub weights: [u8; 4],
    /// Where it turns about, at rest.
    pub pivot: Vec3,
    pub layer: u8,
    /// Its vertices at rest (positions and normals, in `vertices`' order).
    pub shape: Vec<(Vec3, Vec3)>,
    /// For a plate that comes apart at its seam, its triangles (their numbers in the mesh), which
    /// [`cut_seams`] gives corners of their own; else none.
    pub triangles: Vec<u32>,
}

/// The bone named `pattern` (a [`marks::glob`]) for something on `side` near `near`: the one on that
/// side, else the nearest.
fn bone_named(bones: &[asset::Bone], pattern: &str, side: Side, near: Vec3) -> Option<usize> {
    let at = |b: &asset::Bone| Vec3::new(b.model_matrix[12], b.model_matrix[13], b.model_matrix[14]);
    let found: Vec<usize> = (0..bones.len()).filter(|&i| marks::glob(pattern, &bones[i].name)).collect();
    let same = found.iter().copied().filter(|&i| Side::of(at(&bones[i]).x) == side || Side::of(at(&bones[i]).x) == Side::Middle);
    let nearest = |it: &mut dyn Iterator<Item = usize>| it.min_by(|&a, &b| at(&bones[a]).distance(near).total_cmp(&at(&bones[b]).distance(near)));
    nearest(&mut same.into_iter()).or_else(|| nearest(&mut found.iter().copied()))
}

fn origin(b: &asset::Bone) -> Vec3 {
    Vec3::new(b.model_matrix[12], b.model_matrix[13], b.model_matrix[14])
}

/// Where a plate at `near` riding `a` and `b` turns: the joint between them nearest it. The joints
/// between two bones are where each bone on the way from one to the other, through their nearest
/// common ancestor, meets its parent; a bone at its parent's own joint (a twist bone at the elbow,
/// under the forearm) only rolls there, and isn't one (a pauldron on the upper chest and the upper
/// arm turns at the shoulder, not at the neck the clavicles hang from; a couter at the elbow).
/// None for bones with no common ancestor.
fn joint_between(bones: &[asset::Bone], a: usize, b: usize, near: Vec3) -> Option<Vec3> {
    let parent = |i: usize| usize::try_from(bones[i].parent).ok().filter(|&p| p < bones.len() && p != i);
    let up = |mut i: usize| {
        let mut chain = vec![i];
        while let Some(p) = parent(i) {
            if chain.len() > bones.len() {
                break;
            }
            chain.push(p);
            i = p;
        }
        chain
    };
    let (ca, cb) = (up(a), up(b));
    let common = *ca.iter().find(|x| cb.contains(x))?;
    let below = |chain: &[usize]| chain.iter().take_while(|&&x| x != common).copied().collect::<Vec<_>>();
    below(&ca)
        .into_iter()
        .chain(below(&cb))
        .filter(|&j| parent(j).is_none_or(|p| origin(&bones[j]).distance(origin(&bones[p])) > 1e-3))
        .map(|j| origin(&bones[j]))
        .min_by(|x, y| x.distance(near).total_cmp(&y.distance(near)))
}

/// Shares as vertex weights: the four largest, in 255ths summing to 255.
fn quantise(shares: &[(usize, f32)]) -> Option<([u8; 4], [u8; 4])> {
    let mut s: Vec<(usize, f32)> = shares.iter().copied().filter(|x| x.1 > 0.0 && x.0 < 256).collect();
    s.sort_by(|a, b| b.1.total_cmp(&a.1));
    s.truncate(4);
    let total: f32 = s.iter().map(|x| x.1).sum();
    if s.is_empty() || total <= 0.0 {
        return None;
    }
    let (mut bones, mut weights) = ([0u8; 4], [0u8; 4]);
    let mut left = 255u32;
    for (k, &(b, w)) in s.iter().enumerate() {
        let q = if k + 1 == s.len() { left } else { ((w / total) * 255.0).round().min(left as f32) as u32 };
        bones[k] = b as u8;
        weights[k] = q as u8;
        left -= q;
    }
    Some((bones, weights))
}

/// What a piece's marks make of a model.
#[derive(Clone, Debug, Default)]
pub struct Marked {
    pub plates: Vec<Plate>,
    /// What went wrong (a bone a plate rides that the skeleton hasn't).
    pub problems: Vec<String>,
    /// The plates that chose nothing in this model (a rule for several models that this one has
    /// nothing for).
    pub unchosen: Vec<String>,
}

/// Where the game's own skinning (linear blending) puts a vertex with each bone moved by `moves`.
fn blended(v: &asset::Vertex, moves: &[Mat4]) -> (Vec3, Vec3) {
    let total: u32 = v.weights.iter().map(|&w| w as u32).sum();
    if total == 0 || moves.is_empty() {
        return (Vec3::from_array(v.pos), Vec3::from_array(v.normal));
    }
    let mut m = Mat4::ZERO;
    for k in 0..4 {
        if v.weights[k] > 0 {
            m += moves.get(v.bones[k] as usize).copied().unwrap_or(Mat4::IDENTITY) * (v.weights[k] as f32 / total as f32);
        }
    }
    (m.transform_point3(Vec3::from_array(v.pos)), m.transform_vector3(Vec3::from_array(v.normal)).normalize_or_zero())
}

/// The plates of `piece` in `model` (a part skinned to its body's skeleton), shaped at rest:
/// `rest` is each bone's model matrix with the body at rest ([`crate::rig::rest_pose`]; empty: as
/// bound).
pub fn plates(piece: &marks::Piece, model: &asset::Model, rest: &[Mat4]) -> Marked {
    // Each bone's move from its bind pose to rest.
    let moves: Vec<Mat4> = model.bones.iter().zip(rest).map(|(b, r)| *r * Mat4::from_cols_array(&b.model_matrix).inverse()).collect();
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let mut unchosen = Vec::new();
    // The side each bone is on (where its joint is).
    let bone_side: Vec<Side> = model.bones.iter().map(|b| Side::of(origin(b).x)).collect();
    let islands: Vec<Vec<Island>> = model.meshes.iter().map(islands).collect();
    for mark in &piece.plates {
        let meshes: Vec<usize> = match &mark.mesh {
            None => (0..model.meshes.len()).collect(),
            Some(MeshRef::Index(i)) => vec![*i].into_iter().filter(|&i| i < model.meshes.len()).collect(),
            Some(MeshRef::Name(n)) => (0..model.meshes.len()).filter(|&i| model.meshes[i].name.eq_ignore_ascii_case(n)).collect(),
        };
        let bone_ok = |b: Option<usize>| mark.bone.is_empty() || b.is_some_and(|b| mark.bone.iter().any(|p| marks::glob(p, &model.bones[b].name)));
        // (side, mesh) -> the vertices chosen.
        let mut chosen: Vec<((Side, usize), Vec<u32>)> = Vec::new();
        for &m in &meshes {
            let mesh = &model.meshes[m];
            let welded = welds(mesh);
            // Each weld group's bone: the heaviest of its first corner (they share weights).
            for (k, isl) in islands[m].iter().enumerate() {
                let size = (isl.max - isl.min).max_element();
                let take = (mark.island.is_empty() || mark.island.contains(&k))
                    && bone_ok(isl.heaviest())
                    && mark.side.takes(isl.side)
                    && mark.within.is_none_or(|b| b.contains(isl.centre.to_array()))
                    && mark.largest.is_none_or(|l| size <= l);
                if !take {
                    continue;
                }
                // The places drawn from the texture's box: a corner there with other texture
                // coordinates (the soft part's, where it meets the plate) goes too, so the two stay
                // joined. (A plate coming apart at its seam leaves them apart.)
                let drawn: Option<HashSet<u32>> = mark.uv_within.map(|b| {
                    isl.vertices.iter().filter(|&&i| b.contains(mesh.vertices[i as usize].uv)).map(|&i| if mark.seam { i } else { welded[i as usize] }).collect()
                });
                // A plate made of some of an island's corners is on the side of their bones (an
                // island across the body, both legs' leather, gives each leg its own plate);
                // one made of whole islands, on theirs.
                for &i in &isl.vertices {
                    if mark.corners_within.is_some_and(|b| !b.contains(mesh.vertices[i as usize].pos)) {
                        continue;
                    }
                    if drawn.as_ref().is_some_and(|d| !d.contains(&if mark.seam { i } else { welded[i as usize] })) {
                        continue;
                    }
                    let side = if mark.vertices.is_empty() {
                        isl.side
                    } else {
                        match heaviest(&mesh.vertices[welded[i as usize] as usize]) {
                            Some((b, share)) if share >= mark.firm && mark.vertices.iter().any(|p| marks::glob(p, &model.bones[b].name)) => bone_side[b],
                            _ => continue,
                        }
                    };
                    match chosen.iter_mut().find(|(key, _)| *key == (side, m)) {
                        Some((_, v)) => v.push(i),
                        None => chosen.push(((side, m), vec![i])),
                    }
                }
            }
        }
        if chosen.is_empty() {
            unchosen.push(mark.name.clone());
            continue;
        }
        for ((side, m), mut vertices) in chosen {
            let mesh = &model.meshes[m];
            // One that comes apart at its seam is the triangles with a corner chosen, whole.
            let mut triangles = Vec::new();
            if mark.seam {
                let corners: HashSet<u32> = vertices.iter().copied().collect();
                triangles = mesh.indices.chunks_exact(3).enumerate().filter(|(_, t)| t.iter().any(|i| corners.contains(i))).map(|(t, _)| t as u32).collect();
                let mut all: Vec<u32> = triangles.iter().flat_map(|&t| mesh.indices[t as usize * 3..t as usize * 3 + 3].iter().copied()).collect();
                all.sort_unstable();
                all.dedup();
                vertices = all;
            }
            let centre = vertices.iter().map(|&i| Vec3::from_array(mesh.vertices[i as usize].pos)).sum::<Vec3>() / vertices.len() as f32;
            // Its riders: as marked, else its own weights averaged.
            let mut shares: Vec<(usize, f32)> = Vec::new();
            if mark.ride.is_empty() {
                for &i in &vertices {
                    add_weights(&mut shares, &mesh.vertices[i as usize], 1.0 / vertices.len() as f32);
                }
                let most = shares.iter().map(|s| s.1).fold(0.0, f32::max);
                shares.retain(|s| s.1 >= LEAST_SHARE.min(most));
            } else {
                for (name, &share) in &mark.ride {
                    // A plate in the middle riding a bone of either side (`Bip01 * Clavicle`) rides
                    // both, half each.
                    let both = (side == Side::Middle).then(|| (bone_named(&model.bones, name, Side::Left, centre), bone_named(&model.bones, name, Side::Right, centre)));
                    match (both, bone_named(&model.bones, name, side, centre)) {
                        (Some((Some(l), Some(r))), _) if l != r => shares.extend([(l, share * 0.5), (r, share * 0.5)]),
                        (_, Some(b)) => shares.push((b, share)),
                        _ => problems.push(format!("{}: plate '{}' rides '{name}', which the skeleton hasn't", piece.name, mark.name)),
                    }
                }
            }
            shares.sort_by(|a, b| b.1.total_cmp(&a.1));
            let Some((bones, weights)) = quantise(&shares) else {
                problems.push(format!("{}: plate '{}' ({}) rides nothing", piece.name, mark.name, side.name()));
                continue;
            };
            let mirror = |p: [f32; 3]| Vec3::new(if side == Side::Left { -p[0] } else { p[0] }, p[1], p[2]);
            let pivot = match &mark.pivot {
                Some(Pivot::Point(p)) => mirror(*p),
                Some(Pivot::Bone(name)) => match bone_named(&model.bones, name, side, centre) {
                    Some(b) => origin(&model.bones[b]),
                    None => {
                        problems.push(format!("{}: plate '{}' turns about '{name}', which the skeleton hasn't", piece.name, mark.name));
                        centre
                    }
                },
                None if shares.len() >= 2 => joint_between(&model.bones, shares[0].0, shares[1].0, centre).unwrap_or(centre),
                None => centre,
            };
            // At rest: its vertices as the game skins them there, its pivot where its riders take it.
            let shape: Vec<(Vec3, Vec3)> = vertices.iter().map(|&i| blended(&mesh.vertices[i as usize], &moves)).collect();
            let total: f32 = shares.iter().map(|s| s.1).sum::<f32>().max(1e-6);
            let pivot = if moves.is_empty() {
                pivot
            } else {
                shares.iter().map(|&(b, w)| moves.get(b).map_or(pivot, |m| m.transform_point3(pivot)) * (w / total)).sum()
            };
            out.push(Plate { name: mark.name.clone(), side, mesh: m, vertices, bones, weights, pivot, layer: mark.layer, shape, triangles });
        }
    }
    attach_studs(model, &islands, &moves, &mut out);
    Marked { plates: out, problems, unchosen }
}

/// Gives each plate that comes apart at its seam its own copy of the corners its triangles share
/// with the rest of its mesh (added to the mesh, its triangles turned to them), so the rest keeps
/// its own and the plate moves apart from it instead of dragging it along. Call it on the model
/// the plates were made of, before they're applied.
pub fn cut_seams(model: &mut asset::Model, plates: &mut [Plate]) {
    for p in plates.iter_mut().filter(|p| !p.triangles.is_empty()) {
        let mesh = &mut model.meshes[p.mesh];
        let mine: HashSet<u32> = p.triangles.iter().copied().collect();
        let shared: HashSet<u32> = mesh.indices.chunks_exact(3).enumerate().filter(|(t, _)| !mine.contains(&(*t as u32))).flat_map(|(_, t)| t.iter().copied()).collect();
        let mut copy_of: HashMap<u32, u32> = HashMap::new();
        for &t in &p.triangles {
            for k in t as usize * 3..t as usize * 3 + 3 {
                let i = mesh.indices[k];
                if !shared.contains(&i) {
                    continue;
                }
                let copy = match copy_of.get(&i) {
                    Some(&c) => c,
                    None => {
                        mesh.vertices.push(mesh.vertices[i as usize]);
                        let c = mesh.vertices.len() as u32 - 1;
                        copy_of.insert(i, c);
                        c
                    }
                };
                mesh.indices[k] = copy;
            }
        }
        for v in &mut p.vertices {
            if let Some(&c) = copy_of.get(v) {
                *v = c;
            }
        }
    }
}

/// Islands no bigger than this (metres across) are studs, rivets and buckles...
const STUD: f32 = 0.03;
/// ... and one within this of a plate is on it.
const ON: f32 = 0.01;

/// Studs, rivets and buckles (islands too small to be plates) sitting on a plate move with it, so
/// they stay where they're fixed rather than sliding over it on their own weights.
fn attach_studs(model: &asset::Model, islands: &[Vec<Island>], moves: &[Mat4], out: &mut Vec<Plate>) {
    let taken: std::collections::HashSet<(usize, u32)> = out.iter().flat_map(|p| p.vertices.iter().map(move |&v| (p.mesh, v))).collect();
    let mut joins: Vec<(usize, usize, Vec<u32>)> = Vec::new();
    for (m, mesh) in model.meshes.iter().enumerate() {
        for isl in &islands[m] {
            if (isl.max - isl.min).max_element() > STUD || isl.vertices.iter().any(|&v| taken.contains(&(m, v))) {
                continue;
            }
            let at = |v: u32| Vec3::from_array(mesh.vertices[v as usize].pos);
            // The plate nearest any of its corners.
            let nearest = out
                .iter()
                .enumerate()
                .map(|(k, p)| {
                    let pm = &model.meshes[p.mesh];
                    let d = p.vertices.iter().map(|&i| Vec3::from_array(pm.vertices[i as usize].pos)).map(|q| isl.vertices.iter().map(|&v| at(v).distance(q)).fold(f32::MAX, f32::min)).fold(f32::MAX, f32::min);
                    (k, d)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((k, d)) = nearest
                && d <= ON
            {
                joins.push((k, m, isl.vertices.clone()));
            }
        }
    }
    for (k, m, vertices) in joins {
        let shape: Vec<(Vec3, Vec3)> = vertices.iter().map(|&i| blended(&model.meshes[m].vertices[i as usize], moves)).collect();
        if out[k].mesh == m {
            out[k].vertices.extend(vertices);
            out[k].shape.extend(shape);
        } else {
            let plate = Plate { mesh: m, vertices, shape, triangles: Vec::new(), ..out[k].clone() };
            out.push(plate);
        }
    }
}

/// What a plate rides, for the log and the preview: "Bip01 L Forearm 70%, Bip01 L Hand 30%".
pub fn riders(plate: &Plate, bones: &[asset::Bone]) -> String {
    (0..4)
        .filter(|&k| plate.weights[k] > 0)
        .map(|k| format!("{} {:.0}%", bones.get(plate.bones[k] as usize).map_or("?", |b| b.name.as_str()), plate.weights[k] as f32 / 2.55))
        .collect::<Vec<_>>()
        .join(", ")
}
