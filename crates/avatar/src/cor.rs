//! Joints that keep their volume: skinning about optimised centres of rotation (Le and Hodgins,
//! "Real-time Skeletal Skinning with Optimized Centers of Rotation", SIGGRAPH 2016).
//!
//! Linear blend skinning (LBS) blends the bones' matrices, so a vertex between two bones that turn
//! apart is pulled in towards the line between where each would take it: a bent elbow or knee
//! pinches, and a turned wrist collapses into a twisted ribbon (the "candy wrapper"). Instead,
//! each vertex on two or more bones gets a **centre**, found once from the rest pose and the
//! weights: where the surface weighted like it sits on average, which is the middle of the limb at
//! that joint. Skinned, the vertex turns by the blend of its bones' rotations about its centre, and
//! the centre goes where LBS takes it:
//!
//! ```text
//! v' = R · S · (v − p*) + LBS(p*)
//! ```
//!
//! R is the normalised blend of the bones' rotations (quaternions, each turned into the first one's
//! hemisphere) and S the blend of the rest of their matrices (the body's scale, an arm segment's
//! stretch), see `render.rs`. A rotation can't collapse anything, and LBS is right at the middle of
//! a limb, so nothing pinches or bulges. A vertex on one bone is skinned exactly as LBS does.
//!
//! The centre of vertex i is the paper's (its eq. 3, over triangles): the mean of the triangles'
//! middles c_t, weighted by their areas a_t and by how alike their weights w_t (the mean of their
//! corners') are to the vertex's own w_i:
//!
//! ```text
//! p*_i = Σ_t s(w_i, w_t) a_t c_t / Σ_t s(w_i, w_t) a_t
//! s(wp, wv) = Σ_{j≠k} wp_j wp_k wv_j wv_k exp(−(wp_j wv_k − wp_k wv_j)² / σ²),  σ = 0.1
//! ```
//!
//! over every mesh of the body: the skin and the armour over it are the same limb. A game's meshes
//! are coarse, so a triangle whose corners' weights differ by more than [`SPLIT`] is sampled as its
//! four halves (and theirs, to [`DEPTH`] levels), as the paper subdivides the mesh. Only the pairs of
//! bones a vertex has can make `s` other than 0, so the triangles are filed by the pairs they carry
//! and each vertex visits only its pairs' files.

use std::collections::HashMap;

use glam::Vec3;
use rustvr_asset as asset;

/// The similarity's width (the paper's σ).
const SIGMA: f32 = 0.1;
/// Weights under this (a share of the vertex) don't count as a bone the vertex is on.
const EPS: f32 = 1e-3;
/// A triangle whose corners' weights differ by more than this (Euclidean) is sampled finer...
const SPLIT: f32 = 0.1;
/// ... down to this many halvings.
const DEPTH: u32 = 3;

/// A vertex's or a sample's weights: (bone, share) pairs, shares summing to 1.
type Weights = Vec<(u8, f32)>;

fn weights_of(v: &asset::Vertex) -> Weights {
    let total: u32 = v.weights.iter().map(|&w| w as u32).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut out: Weights = Vec::with_capacity(4);
    for k in 0..4 {
        if v.weights[k] > 0 {
            let w = v.weights[k] as f32 / total as f32;
            match out.iter_mut().find(|(b, _)| *b == v.bones[k]) {
                Some(e) => e.1 += w,
                None => out.push((v.bones[k], w)),
            }
        }
    }
    out
}

/// `a·x + b·y + c·z` of three weight sets.
fn mix(a: &Weights, b: &Weights, c: &Weights, x: f32, y: f32, z: f32) -> Weights {
    let mut out: Weights = Vec::with_capacity(a.len() + b.len() + c.len());
    for (set, k) in [(a, x), (b, y), (c, z)] {
        for &(bone, w) in set {
            match out.iter_mut().find(|(o, _)| *o == bone) {
                Some(e) => e.1 += w * k,
                None => out.push((bone, w * k)),
            }
        }
    }
    out
}

fn distance(a: &Weights, b: &Weights) -> f32 {
    let mut d = 0.0;
    for &(bone, w) in a {
        let o = b.iter().find(|(x, _)| *x == bone).map_or(0.0, |e| e.1);
        d += (w - o) * (w - o);
    }
    for &(bone, w) in b {
        if !a.iter().any(|(x, _)| *x == bone) {
            d += w * w;
        }
    }
    d.sqrt()
}

/// A piece of surface: its middle, area and weights.
struct Sample {
    centre: Vec3,
    area: f32,
    weights: Weights,
}

/// Samples a triangle (corners `p`, weights `w`), finer where its weights change a lot.
fn sample(p: [Vec3; 3], w: [&Weights; 3], depth: u32, out: &mut Vec<Sample>) {
    let area = 0.5 * (p[1] - p[0]).cross(p[2] - p[0]).length();
    if area <= 1e-12 {
        return;
    }
    let spread = distance(w[0], w[1]).max(distance(w[1], w[2])).max(distance(w[0], w[2]));
    if spread <= SPLIT || depth >= DEPTH {
        let third = 1.0 / 3.0;
        out.push(Sample { centre: (p[0] + p[1] + p[2]) * third, area, weights: mix(w[0], w[1], w[2], third, third, third) });
        return;
    }
    // The four halves: the corners' triangles and the middle one.
    let m = [(p[0] + p[1]) * 0.5, (p[1] + p[2]) * 0.5, (p[2] + p[0]) * 0.5];
    let mw = [mix(w[0], w[1], w[2], 0.5, 0.5, 0.0), mix(w[0], w[1], w[2], 0.0, 0.5, 0.5), mix(w[0], w[1], w[2], 0.5, 0.0, 0.5)];
    sample([p[0], m[0], m[2]], [w[0], &mw[0], &mw[2]], depth + 1, out);
    sample([m[0], p[1], m[1]], [&mw[0], w[1], &mw[1]], depth + 1, out);
    sample([m[2], m[1], p[2]], [&mw[2], &mw[1], w[2]], depth + 1, out);
    sample([m[0], m[1], m[2]], [&mw[0], &mw[1], &mw[2]], depth + 1, out);
}

/// The paper's similarity of two weight sets.
fn similarity(p: &Weights, v: &Weights) -> f32 {
    let mut s = 0.0;
    for (a, &(j, pj)) in p.iter().enumerate() {
        let Some(vj) = v.iter().find(|(b, _)| *b == j).map(|e| e.1) else { continue };
        for &(k, pk) in &p[a + 1..] {
            let Some(vk) = v.iter().find(|(b, _)| *b == k).map(|e| e.1) else { continue };
            let d = pj * vk - pk * vj;
            s += pj * pk * vj * vk * (-(d * d) / (SIGMA * SIGMA)).exp();
        }
    }
    s
}

/// Each vertex's centre of rotation, mesh by mesh: `Some` for every vertex on two or more bones
/// (where any surface is weighted like it), `None` for the rest (they're skinned as LBS skins them).
pub fn centres(meshes: &[(&[asset::Vertex], &[u32])]) -> Vec<Vec<Option<Vec3>>> {
    let started = std::time::Instant::now();
    let weights: Vec<Vec<Weights>> = meshes.iter().map(|(v, _)| v.iter().map(weights_of).collect()).collect();
    let mut samples = Vec::new();
    for (&(vertices, indices), w) in meshes.iter().zip(&weights) {
        for t in indices.chunks_exact(3) {
            let i = [t[0] as usize, t[1] as usize, t[2] as usize];
            if i.iter().any(|&k| k >= vertices.len()) {
                continue;
            }
            let p = i.map(|k| Vec3::from_array(vertices[k].pos));
            sample(p, [&w[i[0]], &w[i[1]], &w[i[2]]], 0, &mut samples);
        }
    }
    // Each sample in the file of every pair of bones it carries.
    let mut files: HashMap<(u8, u8), Vec<u32>> = HashMap::new();
    for (n, s) in samples.iter().enumerate() {
        for (a, &(j, wj)) in s.weights.iter().enumerate() {
            for &(k, wk) in &s.weights[a + 1..] {
                if wj > EPS && wk > EPS {
                    files.entry((j.min(k), j.max(k))).or_default().push(n as u32);
                }
            }
        }
    }
    // The centre of a vertex with weights `w`. `stamp` marks the samples it has counted (one with
    // three of its bones is in several of its files), `mark` being this vertex's mark.
    let centre = |w: &Weights, stamp: &mut [u32], mark: u32| -> Option<Vec3> {
        let on: Weights = w.iter().copied().filter(|&(_, x)| x > EPS).collect();
        if on.len() < 2 {
            return None;
        }
        let (mut sum, mut den) = (glam::DVec3::ZERO, 0.0f64);
        for (a, &(j, _)) in on.iter().enumerate() {
            for &(k, _) in &on[a + 1..] {
                let Some(file) = files.get(&(j.min(k), j.max(k))) else { continue };
                for &n in file {
                    if std::mem::replace(&mut stamp[n as usize], mark) == mark {
                        continue;
                    }
                    let s = &samples[n as usize];
                    let k = similarity(&on, &s.weights) as f64 * s.area as f64;
                    sum += s.centre.as_dvec3() * k;
                    den += k;
                }
            }
        }
        (den > 1e-14).then(|| (sum / den).as_vec3())
    };
    // Spread over a few threads (bodies are tens of thousands of vertices).
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4);
    let all: Vec<&Weights> = weights.iter().flatten().collect();
    let chunk = all.len().div_ceil(threads).max(1);
    let flat: Vec<Option<Vec3>> = std::thread::scope(|scope| {
        let (centre, count) = (&centre, samples.len());
        let jobs: Vec<_> = all
            .chunks(chunk)
            .enumerate()
            .map(|(c, part)| {
                scope.spawn(move || {
                    let mut stamp = vec![0u32; count];
                    part.iter().enumerate().map(|(i, w)| centre(w, &mut stamp, (c * chunk + i + 1) as u32)).collect::<Vec<_>>()
                })
            })
            .collect();
        jobs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let mut flat = flat.into_iter();
    let out: Vec<Vec<Option<Vec3>>> = weights.iter().map(|w| flat.by_ref().take(w.len()).collect()).collect();
    let soft: usize = out.iter().map(|m| m.iter().filter(|c| c.is_some()).count()).sum();
    tracing::debug!("avatar: centres of rotation for {soft} soft vertices from {} samples in {:.0} ms", samples.len(), started.elapsed().as_secs_f32() * 1000.0);
    out
}
