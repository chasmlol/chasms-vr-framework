//! A FaceGen face pulled into shape by hand, within its own geometry coordinates: a hand takes hold
//! of a point of the face and pulls, and the face becomes the one nearest that pull that the face's
//! morphs can make (the game's own space of faces: whatever the hand does, the face stays one the
//! game makes, saves and shows).
//!
//! **What a hold takes.** The vertices round the point taken, falling off with distance
//! ([`REACH`]): the hold's weights. They're also what glows before it's taken
//! ([`crate::facegen::FaceNow::glow`]).
//!
//! **The pull.** Pulled by `d`, each vertex `v` wants to go `w_v · d` (all the way at the point
//! taken, less round it, not at all far from it). The change in the coordinates `Δ` is the least
//! squares one over every vertex of the face, `Σ_v ρ_v |M_v Δ − w_v d|² + λ |Δ|²` (`M_v` the
//! morphs' moves of `v`), with what's held counting [`HOLD`] times more than the rest staying where
//! it is (`ρ_v = 1 + HOLD · w_v`): so the part taken goes with the hand as far as the face can take
//! it, and the rest of the face changes as little as it can for that. It's linear in `d`, so it's
//! solved once as the hold is taken (`Δ = H d`), and each frame is a 3-column multiply.
//!
//! **Symmetric, or not.** FaceGen's symmetric morphs move both sides alike: a pull on one cheek
//! widens both, and a nose pulled sideways goes nowhere. Its asymmetric morphs move one side against
//! the other. A pull is symmetric, or as asymmetric as the caller says (`asym` 0..1, a trigger
//! squeezed: blended, so the face never jumps).
//!
//! **What's kept.** The face's age and gender (the game's own sliders for them, and the age picks
//! its age map) are kept as they were; and its named controls stay within their sliders' ranges
//! ([`rustvr_asset::FaceControls::limit`]).

use std::sync::Arc;

use glam::Vec3;
use rustvr_asset::FaceControls;

use crate::facegen::FaceShape;

/// How far round the point taken a hold reaches (metres, the face's own size): the Gaussian falloff
/// of its weights.
pub const REACH: f32 = 0.022;
/// How much more the part held counts than the rest of the face staying put.
pub const HOLD: f64 = 250.0;
/// A touch of the change's own size in the least squares, so it never goes wild where the morphs
/// can't tell their coordinates apart (relative to the morphs' mean square).
const LAMBDA: f64 = 1e-3;
/// Weights under this aren't part of a hold.
const FLOOR: f32 = 0.01;

/// A face's parts and their morphs, ready to be pulled.
pub struct Sculptor {
    parts: Vec<Part>,
    /// The symmetric and asymmetric coordinates' counts (the most any part has).
    ks: usize,
    ka: usize,
    /// `Σ_v M_vᵀ M_v` over every vertex of the parts a hand takes, (ks + ka)², row-major.
    gram: Vec<f64>,
    controls: Option<Arc<FaceControls>>,
}

struct Part {
    shape: Arc<FaceShape>,
    /// Whether a hand takes hold of it (not the hair: it's drawn over the head's shape, but a
    /// hand on it holds the hair, not the face).
    held: bool,
}

/// A hold on the face: where, what it takes, and how its coordinates go with the pull.
#[derive(Clone, Debug)]
pub struct Hold {
    /// The part and its vertex taken hold of, and where that was (model space).
    pub part: usize,
    pub vertex: usize,
    pub at: Vec3,
    /// What the hold takes: each part's vertices' weights (0..1; empty: none of it).
    pub weights: Vec<Vec<f32>>,
    /// The face it was taken from (symmetric, asymmetric coordinates).
    pub from: (Vec<f32>, Vec<f32>),
    /// The coordinates' change per metre of pull along x, y and z (model space): symmetric only,
    /// and with the asymmetric ones.
    symmetric: Vec<[f32; 3]>,
    both: Vec<[f32; 3]>,
}

impl Sculptor {
    /// `shapes` by part ([`FaceShape::part`]), and whether a hand takes hold of each.
    pub fn new(shapes: Vec<(Arc<FaceShape>, bool)>, controls: Option<Arc<FaceControls>>) -> Sculptor {
        let ks = shapes.iter().map(|(s, _)| s.symmetric.len()).max().unwrap_or(0);
        let ka = shapes.iter().map(|(s, _)| s.asymmetric.len()).max().unwrap_or(0);
        let k = ks + ka;
        let parts: Vec<Part> = shapes.into_iter().map(|(shape, held)| Part { shape, held }).collect();
        let mut gram = vec![0f64; k * k];
        for p in parts.iter().filter(|p| p.held) {
            for v in 0..p.shape.rest.len() {
                let m = moves(&p.shape, v, ks, ka);
                add_outer(&mut gram, &m, k, 1.0);
            }
        }
        Sculptor { parts, ks, ka, gram, controls }
    }

    /// How many symmetric and asymmetric coordinates the face has.
    pub fn counts(&self) -> (usize, usize) {
        (self.ks, self.ka)
    }

    /// The parts' vertices where the face `(symmetric, asymmetric)` puts them (model space), by part.
    pub fn positions(&self, symmetric: &[f32], asymmetric: &[f32]) -> Vec<Vec<Vec3>> {
        self.parts.iter().map(|p| p.shape.positions(symmetric, asymmetric).into_iter().map(Vec3::from_array).collect()).collect()
    }

    /// The vertex of a part a hand takes nearest `p` (model space) in `positions`, and how far it
    /// is: (part, vertex, distance).
    pub fn nearest(&self, positions: &[Vec<Vec3>], p: Vec3) -> Option<(usize, usize, f32)> {
        let mut best: Option<(usize, usize, f32)> = None;
        for (i, (_, at)) in self.parts.iter().zip(positions).enumerate().filter(|(_, (part, _))| part.held) {
            for (v, q) in at.iter().enumerate() {
                let d = q.distance_squared(p);
                if best.is_none_or(|b| d < b.2) {
                    best = Some((i, v, d));
                }
            }
        }
        best.map(|(i, v, d)| (i, v, d.sqrt()))
    }

    /// What a hold at `at` (model space, on the face `positions`) would take: each part's
    /// vertices' weights, falling off over [`REACH`].
    pub fn weights(&self, positions: &[Vec<Vec3>], at: Vec3) -> Vec<Vec<f32>> {
        let s2 = 2.0 * REACH * REACH;
        self.parts
            .iter()
            .zip(positions)
            .map(|(part, ps)| {
                if !part.held {
                    return Vec::new();
                }
                ps.iter()
                    .map(|q| {
                        let w = (-q.distance_squared(at) / s2).exp();
                        if w < FLOOR { 0.0 } else { w }
                    })
                    .collect()
            })
            .collect()
    }

    /// Takes hold of vertex `vertex` of part `part` on the face `from` (symmetric, asymmetric).
    pub fn hold(&self, part: usize, vertex: usize, from: (Vec<f32>, Vec<f32>)) -> Option<Hold> {
        let positions = self.positions(&from.0, &from.1);
        let at = *positions.get(part)?.get(vertex)?;
        let weights = self.weights(&positions, at);
        let symmetric = self.gain(&weights, false);
        let both = self.gain(&weights, true);
        Some(Hold { part, vertex, at, weights, from, symmetric, both })
    }

    /// The coordinates' change per metre of pull (x, y, z): `H = A⁻¹ G`, the age and gender taken
    /// back off it.
    fn gain(&self, weights: &[Vec<f32>], asymmetric: bool) -> Vec<[f32; 3]> {
        let (ks, ka) = (self.ks, self.ka);
        let full = ks + ka;
        let k = if asymmetric { full } else { ks };
        // A = Σ_v (1 + HOLD w_v) M_vᵀ M_v + λ I; G = Σ_v (1 + HOLD w_v) w_v M_vᵀ.
        let mut a = vec![0f64; k * k];
        for r in 0..k {
            for c in 0..k {
                a[r * k + c] = self.gram[r * full + c];
            }
        }
        let mut g = vec![0f64; k * 3];
        for (p, ws) in self.parts.iter().zip(weights) {
            for (v, &w) in ws.iter().enumerate().filter(|(_, w)| **w > 0.0) {
                let m = moves(&p.shape, v, ks, if asymmetric { ka } else { 0 });
                let w = w as f64;
                add_outer(&mut a, &m, k, HOLD * w);
                let rho = (1.0 + HOLD * w) * w;
                for c in 0..k {
                    for axis in 0..3 {
                        g[c * 3 + axis] += rho * m[c][axis];
                    }
                }
            }
        }
        let mean = (0..k).map(|i| a[i * k + i]).sum::<f64>() / k.max(1) as f64;
        for i in 0..k {
            a[i * k + i] += LAMBDA * mean.max(1e-12);
        }
        // Three right-hand sides, one per axis.
        let mut out = vec![[0f32; 3]; full];
        for axis in 0..3 {
            let mut aa = a.clone();
            let mut b: Vec<f64> = (0..k).map(|c| g[c * 3 + axis]).collect();
            if !rustvr_asset::facegen::solve(&mut aa, &mut b, k) {
                return vec![[0.0; 3]; full];
            }
            // The age and gender as they were: the change taken off along them.
            if let Some(c) = &self.controls {
                let kept = c.kept();
                if !kept.is_empty() {
                    let sym: Vec<f32> = b[..ks.min(k)].iter().map(|x| *x as f32).collect();
                    let want: Vec<f64> = kept.iter().map(|r| -(r.iter().zip(&sym).map(|(a, b)| a * b).sum::<f32>()) as f64).collect();
                    let d = rustvr_asset::facegen::least_change(&kept, &want, ks);
                    for (x, d) in b.iter_mut().zip(d) {
                        *x += d as f64;
                    }
                }
            }
            for (o, x) in out.iter_mut().zip(&b) {
                o[axis] = *x as f32;
            }
        }
        out
    }
}

impl Hold {
    /// The face for a pull of `d` (model space, metres), as asymmetric as `asym` (0..1), kept
    /// within the named controls' `limits` ([`rustvr_asset::FaceControls::limit`]).
    pub fn pulled(&self, d: Vec3, asym: f32, controls: Option<&FaceControls>, limits: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut s, mut a) = self.from.clone();
        let t = asym.clamp(0.0, 1.0);
        let ks = s.len();
        for (i, (gs, gb)) in self.symmetric.iter().zip(&self.both).enumerate() {
            let along = |g: &[f32; 3]| g[0] * d.x + g[1] * d.y + g[2] * d.z;
            let x = along(gs) * (1.0 - t) + along(gb) * t;
            if i < ks {
                s[i] += x;
            } else if let Some(v) = a.get_mut(i - ks) {
                *v += x;
            }
        }
        if let Some(c) = controls {
            c.limit(&mut s, limits, &self.from.0);
        }
        (s, a)
    }
}

/// Vertex `v`'s moves by each coordinate (`ks` symmetric, then `ka` asymmetric; a part with fewer
/// has none for the rest).
fn moves(s: &FaceShape, v: usize, ks: usize, ka: usize) -> Vec<[f64; 3]> {
    let get = |set: &Vec<Vec<[f32; 3]>>, k: usize| set.get(k).and_then(|m| m.get(v)).map_or([0.0; 3], |d| [d[0] as f64, d[1] as f64, d[2] as f64]);
    (0..ks).map(|k| get(&s.symmetric, k)).chain((0..ka).map(|k| get(&s.asymmetric, k))).collect()
}

/// `a += scale · Mᵀ M` for one vertex's moves `m` (k of them), `a` k×k row-major.
fn add_outer(a: &mut [f64], m: &[[f64; 3]], k: usize, scale: f64) {
    for r in 0..k.min(m.len()) {
        let mr = m[r];
        if mr == [0.0; 3] {
            continue;
        }
        for c in 0..k.min(m.len()) {
            let mc = m[c];
            a[r * k + c] += scale * (mr[0] * mc[0] + mr[1] * mc[1] + mr[2] * mc[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A face of six vertices along x with three symmetric morphs: one moves the first vertex up,
    /// one the last, one all of them; and an asymmetric one that moves the first sideways.
    fn sculptor() -> Sculptor {
        let n = 6;
        let rest = (0..n).map(|i| [i as f32 * 0.03, 0.0, 0.0]).collect();
        let only = |v: usize, d: [f32; 3]| (0..n).map(|i| if i == v { d } else { [0.0; 3] }).collect::<Vec<_>>();
        let shape = FaceShape {
            part: 0,
            rest,
            symmetric: vec![only(0, [0.0, 0.01, 0.0]), only(n - 1, [0.0, 0.01, 0.0]), vec![[0.0, 0.01, 0.0]; n]],
            asymmetric: vec![only(0, [0.01, 0.0, 0.0])],
        };
        Sculptor::new(vec![(Arc::new(shape), true)], None)
    }

    #[test]
    fn a_pull_moves_what_was_taken_and_little_else() {
        let s = sculptor();
        let h = s.hold(0, 0, (vec![0.0; 3], vec![0.0])).unwrap();
        let (sym, asym) = h.pulled(Vec3::new(0.0, 0.01, 0.0), 0.0, None, &[]);
        let at = s.positions(&sym, &asym);
        // The vertex taken goes most of the way up with the hand; the far end stays nearly put.
        assert!(at[0][0].y > 0.007, "the vertex taken went {}", at[0][0].y);
        assert!(at[0][5].y.abs() < 0.002, "the far end went {}", at[0][5].y);
    }

    #[test]
    fn a_sideways_pull_needs_the_asymmetric_morphs() {
        let s = sculptor();
        let h = s.hold(0, 0, (vec![0.0; 3], vec![0.0])).unwrap();
        let (sym, asym) = h.pulled(Vec3::new(0.01, 0.0, 0.0), 0.0, None, &[]);
        assert!(s.positions(&sym, &asym)[0][0].x.abs() < 1e-4);
        let (sym, asym) = h.pulled(Vec3::new(0.01, 0.0, 0.0), 1.0, None, &[]);
        assert!(s.positions(&sym, &asym)[0][0].x > 0.007);
    }

    #[test]
    fn a_hold_reaches_round_the_point_taken() {
        let s = sculptor();
        let at = s.positions(&[0.0; 3], &[0.0]);
        let w = s.weights(&at, at[0][0]);
        assert!((w[0][0] - 1.0).abs() < 1e-6 && w[0][1] > 0.1 && w[0][1] < 1.0 && w[0][5] == 0.0);
        assert_eq!(s.nearest(&at, Vec3::new(0.031, 0.01, 0.0)).map(|n| (n.0, n.1)), Some((0, 1)));
    }
}

