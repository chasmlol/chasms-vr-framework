//! Plates: the parts of a body model a marks file ([`rustvr_asset::armour`]) says are rigid.
//!
//! A plate's vertices all get the same weights (its riders') and the same centre of rotation (its
//! pivot), so the skinning that keeps joints' volume (`cor.rs`) turns the whole plate by one
//! rotation about one point and takes that point where its riders take it: the plate moves as one
//! piece and never bends. Riding two bones, it turns by each one's share about the joint between
//! them, which is what a game's helper joint does for a couter or a pauldron.
//!
//! Islands (the pieces of a mesh whose triangles share corners, corners at the same place being
//! the same corner) are what plates are chosen from; [`islands`] lists them as the preview shows
//! them.

use std::collections::HashMap;

use glam::Vec3;
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

/// The bone a vertex is mostly on.
fn heaviest(v: &asset::Vertex) -> Option<usize> {
    (0..4).filter(|&k| v.weights[k] > 0).max_by_key(|&k| v.weights[k]).map(|k| v.bones[k] as usize)
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
    pub pivot: Vec3,
    pub layer: u8,
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

/// Where a plate riding `a` and `b` turns: the joint between them. Below their nearest common
/// ancestor each is reached through one joint (the first bone on the way down to it); the joint is
/// whichever of those is further from the ancestor (a twist bone at the elbow, under the forearm,
/// turns about the forearm's own axis; the hand, beside it, at the wrist). None when neither is
/// below the other's ancestor (unrelated bones).
fn joint_between(bones: &[asset::Bone], a: usize, b: usize) -> Option<Vec3> {
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
    let below = |chain: &[usize]| chain.iter().position(|&x| x == common).and_then(|k| k.checked_sub(1)).map(|k| chain[k]);
    let at = origin(&bones[common]);
    [below(&ca), below(&cb)].into_iter().flatten().map(|j| origin(&bones[j])).max_by(|x, y| x.distance(at).total_cmp(&y.distance(at)))
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

/// The plates of `piece` in `model` (a part skinned to its body's skeleton), with what went wrong
/// in its marks (a bone it rides that the skeleton hasn't, a plate that chose nothing).
pub fn plates(piece: &marks::Piece, model: &asset::Model) -> (Vec<Plate>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
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
                let take = (mark.island.is_empty() || mark.island.contains(&k))
                    && bone_ok(isl.heaviest())
                    && mark.side.takes(isl.side)
                    && mark.within.is_none_or(|b| b.contains(isl.centre.to_array()));
                if !take {
                    continue;
                }
                let vertices: Vec<u32> = isl
                    .vertices
                    .iter()
                    .copied()
                    .filter(|&i| {
                        mark.vertices.is_empty()
                            || heaviest(&mesh.vertices[welded[i as usize] as usize]).is_some_and(|b| mark.vertices.iter().any(|p| marks::glob(p, &model.bones[b].name)))
                    })
                    .collect();
                if vertices.is_empty() {
                    continue;
                }
                match chosen.iter_mut().find(|(key, _)| *key == (isl.side, m)) {
                    Some((_, v)) => v.extend(vertices),
                    None => chosen.push(((isl.side, m), vertices)),
                }
            }
        }
        if chosen.is_empty() {
            problems.push(format!("{}: plate '{}' chose nothing", piece.name, mark.name));
            continue;
        }
        for ((side, m), vertices) in chosen {
            let mesh = &model.meshes[m];
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
                    match bone_named(&model.bones, name, side, centre) {
                        Some(b) => shares.push((b, share)),
                        None => problems.push(format!("{}: plate '{}' rides '{name}', which the skeleton hasn't", piece.name, mark.name)),
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
                None if shares.len() >= 2 => joint_between(&model.bones, shares[0].0, shares[1].0).unwrap_or(centre),
                None => centre,
            };
            out.push(Plate { name: mark.name.clone(), side, mesh: m, vertices, bones, weights, pivot, layer: mark.layer });
        }
    }
    (out, problems)
}

/// What a plate rides, for the log and the preview: "Bip01 L Forearm 70%, Bip01 L Hand 30%".
pub fn riders(plate: &Plate, bones: &[asset::Bone]) -> String {
    (0..4)
        .filter(|&k| plate.weights[k] > 0)
        .map(|k| format!("{} {:.0}%", bones.get(plate.bones[k] as usize).map_or("?", |b| b.name.as_str()), plate.weights[k] as f32 / 2.55))
        .collect::<Vec<_>>()
        .join(", ")
}
