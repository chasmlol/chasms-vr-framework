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
//! taken, less round it, not at all far from it). FaceGen's coordinates each move the whole face (a
//! statistical model's modes), so a change of one part alone is a mix of many of them whose moves
//! cancel elsewhere, and only as nearly as they can. The change in the coordinates `Δ` is the least
//! squares one between the part held following the hand and the rest of the face staying where it
//! is: `Σ_v h_v |M_v Δ − w_v d|² + Σ_v s_v |M_v Δ|² + λ |Δ|²` (`M_v` the morphs' moves of `v`; `h_v`
//! the hold's weights over their total, so its term is the hold's mean miss; `s_v` [`STAY`] times
//! each vertex's share of the face held still, so its term is the rest's mean move). What's held
//! still grows with the distance from the point taken: free to change with it up to [`FREE`], held
//! fully past [`PINNED`] (a nose pulled long takes its bridge and nostrils with it, not the ears).
//! It's linear in `d`, so it's solved once as the hold is taken (`Δ = H d`), and each frame is a
//! 3-column multiply.
//!
//! **Symmetric, or not.** FaceGen's symmetric morphs move both sides alike: a pull on one cheek
//! widens both, and a nose pulled sideways goes nowhere; so for a symmetric pull the face round the
//! point's mirror image is as free as round the point. Its asymmetric morphs move one side against
//! the other, and for those the other side is held still like the rest. A pull is symmetric, or as
//! asymmetric as the caller says (`asym` 0..1, a trigger squeezed: blended, so the face never
//! jumps).
//!
//! **What's kept** ([`Sculptor::limit`]). The face's age and gender (the game's own sliders for them,
//! and the age picks its age map) as they were; its named controls within their sliders' ranges;
//! and every coordinate within [`COORDINATE_LIMIT`] of the mean face. The sliders' ranges alone
//! hardly hold some mixes of the coordinates (the controls' smallest singular value is 0.013 of a
//! largest 2.5), so stroke after stroke could take a face far past any the game was made for, where
//! every part of it warps. Whatever goes past is taken back the way the holds' own least squares
//! say costs least, so the rest of the face stays put and the part held stays as near the hand as it
//! can.
//!
//! The figures were measured on the game's Nord head (docs/research/facesculpt.md): a 1 cm pull
//! moves the part taken 0.64 cm on average (the nose tip all of it, the chin a tenth: FaceGen has no
//! chin of its own), and the face more than 6 cm from it (and from its mirror image) 0.07 cm (rms),
//! a third of what weighing the part held against all of the face moved.

use std::sync::Arc;

use glam::Vec3;
use rustvr_asset::FaceControls;

use crate::facegen::FaceShape;

/// How far round the point taken a hold reaches (metres, the face's own size): the Gaussian falloff
/// of its weights.
pub const REACH: f32 = 0.022;
/// How far from the point taken (or its mirror image, pulled symmetric) the face changes freely
/// with it, and how far from it the face is held still fully (metres, the face's own size).
const FREE: f32 = 0.04;
const PINNED: f32 = 0.08;
/// How much the rest of the face staying put counts against the part held following the hand.
const STAY: f64 = 10.0;
/// A touch of the change's own size in the least squares, so it never goes wild where the morphs
/// can't tell their coordinates apart (relative to the morphs' mean square).
const LAMBDA: f64 = 1e-2;
/// Weights under this aren't part of a hold.
const FLOOR: f32 = 0.01;
/// How far a face's coordinates may go either side of the mean face's (FaceGen's coordinates are
/// in standard deviations of real faces). One already further keeps what it has.
pub const COORDINATE_LIMIT: f32 = 8.0;

/// A face's parts and their morphs, ready to be pulled.
pub struct Sculptor {
    parts: Vec<Part>,
    /// The symmetric and asymmetric coordinates' counts (the most any part has).
    ks: usize,
    ka: usize,
    /// `Σ_v M_vᵀ M_v` over every vertex of the parts a hand takes, (ks + ka)², row-major, and how
    /// many vertices that is: the face held still everywhere, which a hold changes only near it.
    gram: Vec<f64>,
    count: usize,
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
    /// What a change of the symmetric coordinates costs this hold (its least squares' matrix,
    /// ks², row-major): the way a face past its limits is taken back.
    metric: Vec<f64>,
}

impl Sculptor {
    /// `shapes` by part ([`FaceShape::part`]), and whether a hand takes hold of each.
    pub fn new(shapes: Vec<(Arc<FaceShape>, bool)>, controls: Option<Arc<FaceControls>>) -> Sculptor {
        let ks = shapes.iter().map(|(s, _)| s.symmetric.len()).max().unwrap_or(0);
        let ka = shapes.iter().map(|(s, _)| s.asymmetric.len()).max().unwrap_or(0);
        let k = ks + ka;
        let parts: Vec<Part> = shapes.into_iter().map(|(shape, held)| Part { shape, held }).collect();
        let mut gram = vec![0f64; k * k];
        let mut count = 0;
        for p in parts.iter().filter(|p| p.held) {
            for v in 0..p.shape.rest.len() {
                add_outer(&mut gram, &moves(&p.shape, v, ks, ka), k, 1.0);
                count += 1;
            }
        }
        mirror_upper(&mut gram, k);
        Sculptor { parts, ks, ka, gram, count, controls }
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
        let (symmetric, metric) = self.gain(&positions, at, &weights, false);
        let (both, _) = self.gain(&positions, at, &weights, true);
        Some(Hold { part, vertex, at, weights, from, symmetric, both, metric })
    }

    /// The coordinates' change per metre of pull (x, y, z): `H = A⁻¹ G`, its age and gender taken
    /// back off it the way that costs least (see the module's docs); and `A`'s symmetric part.
    fn gain(&self, positions: &[Vec<Vec3>], at: Vec3, weights: &[Vec<f32>], asymmetric: bool) -> (Vec<[f32; 3]>, Vec<f64>) {
        let (ks, ka) = (self.ks, self.ka);
        let full = ks + ka;
        let k = if asymmetric { full } else { ks };
        // The face held still round the point: none of it up to FREE, all of it past PINNED (the
        // point's mirror image too, pulled symmetric).
        let mirrored = Vec3::new(-at.x, at.y, at.z);
        let still = |q: Vec3| {
            let r = if asymmetric { q.distance(at) } else { q.distance(at).min(q.distance(mirrored)) };
            let t = ((r - FREE) / (PINNED - FREE)).clamp(0.0, 1.0);
            (t * t * (3.0 - 2.0 * t)) as f64
        };
        let mut near: Vec<(usize, usize, f64, f64)> = Vec::new();
        let (mut held, mut moved_off) = (0f64, 0f64);
        for (p, _) in self.parts.iter().enumerate().filter(|(_, part)| part.held) {
            for (v, q) in positions[p].iter().enumerate() {
                let w = weights.get(p).and_then(|ws| ws.get(v)).copied().unwrap_or(0.0) as f64;
                let s = still(*q);
                held += w * w;
                if w > 0.0 || s < 1.0 {
                    moved_off += 1.0 - s;
                    near.push((p, v, w, s));
                }
            }
        }
        // A = s·Σ M_vᵀ M_v + Σ_near (h_v + s (s_v − 1)) M_vᵀ M_v + λ I; G = Σ_near h_v w_v M_vᵀ.
        let stay = STAY / (self.count as f64 - moved_off).max(1.0);
        let hold = 1.0 / held.max(1e-9);
        let mut a = vec![0f64; k * k];
        for r in 0..k {
            for c in r..k {
                a[r * k + c] = stay * self.gram[r * full + c];
            }
        }
        let mut g = vec![0f64; k * 3];
        for &(p, v, w, s) in &near {
            let m = moves(&self.parts[p].shape, v, ks, if asymmetric { ka } else { 0 });
            add_outer(&mut a, &m, k, hold * w + stay * (s - 1.0));
            for c in 0..k {
                for axis in 0..3 {
                    g[c * 3 + axis] += hold * w * w * m[c][axis];
                }
            }
        }
        mirror_upper(&mut a, k);
        let mean = (0..k).map(|i| a[i * k + i]).sum::<f64>() / k.max(1) as f64;
        for i in 0..k {
            a[i * k + i] += LAMBDA * mean.max(1e-12);
        }
        let metric: Vec<f64> = (0..ks * ks).map(|i| a[(i / ks) * k + i % ks]).collect();
        let Some(l) = cholesky(&a, k) else {
            return (vec![[0.0; 3]; full], metric);
        };
        // The age and gender as they were.
        let kept = self.controls.as_ref().map(|c| Kept::new(&l, k, c, ks));
        let mut out = vec![[0f32; 3]; full];
        for axis in 0..3 {
            let mut x: Vec<f64> = (0..k).map(|c| g[c * 3 + axis]).collect();
            cho_solve(&l, k, &mut x);
            if let Some(kept) = &kept {
                kept.project(&mut x, &vec![0.0; kept.rows.len()]);
            }
            for (o, x) in out.iter_mut().zip(&x) {
                o[axis] = *x as f32;
            }
        }
        (out, metric)
    }

    /// Keeps the face `(s, a)` within what it may be (see the module's docs), from the face `from`
    /// it was pulled from by `holds`: its age and gender as `from` had them, each named control
    /// within its range (`limits[i]` either side of 0; 0 or missing: none) and each coordinate
    /// within [`COORDINATE_LIMIT`] (either, or as far as `from` had it if that's further). What's
    /// past them is taken back the way that costs the holds least, a little at a time until it all
    /// holds (or near enough, after a few rounds).
    pub fn limit(&self, s: &mut [f32], a: &mut [f32], from: &(Vec<f32>, Vec<f32>), holds: &[&Hold], limits: &[f32]) {
        for (x, f) in a.iter_mut().zip(&from.1) {
            let l = COORDINATE_LIMIT.max(f.abs());
            *x = x.clamp(-l, l);
        }
        let k = s.len().min(self.ks);
        // The holds' costs together (none: every coordinate alike).
        let mut metric = vec![0f64; k * k];
        for h in holds {
            for r in 0..k {
                for c in 0..k {
                    metric[r * k + c] += h.metric.get(r * self.ks + c).copied().unwrap_or(0.0);
                }
            }
        }
        if holds.is_empty() {
            for i in 0..k {
                metric[i * k + i] = 1.0;
            }
        }
        let Some(l) = cholesky(&metric, k) else { return };
        let gs = |v: &[f32]| -> Vec<f64> { v.iter().take(k).map(|x| *x as f64).collect() };
        let (mut y, f) = (gs(s), gs(&from.0));
        let dot = |r: &[f64], x: &[f64]| r.iter().zip(x).map(|(a, b)| a * b).sum::<f64>();
        let kept = self.controls.as_ref().map(|c| Kept::new(&l, k, c, k));
        let targets: Vec<f64> = kept.as_ref().map_or(Vec::new(), |kept| kept.rows.iter().map(|r| dot(r, &f)).collect());
        // Each control's row and range, then each coordinate's; their costs' ways back, as needed.
        let mut sides: Vec<(Vec<f64>, f64, Option<(Vec<f64>, f64)>)> = Vec::new();
        if let Some(c) = &self.controls {
            for (row, &lim) in c.controls.iter().zip(limits).filter(|(_, l)| **l > 0.0) {
                let r: Vec<f64> = (0..k).map(|i| row.coefficients.get(i).copied().unwrap_or(0.0) as f64).collect();
                let lim = (lim as f64).max(dot(&r, &f).abs());
                sides.push((r, lim, None));
            }
        }
        for i in 0..k {
            let mut r = vec![0.0; k];
            r[i] = 1.0;
            sides.push((r, (COORDINATE_LIMIT as f64).max(f[i].abs()), None));
        }
        for _ in 0..32 {
            let mut moved = false;
            for (r, lim, back) in &mut sides {
                let v = dot(r, &y);
                let over = v - v.clamp(-*lim, *lim);
                if over.abs() < 1e-6 {
                    continue;
                }
                let (ar, rar) = back.get_or_insert_with(|| {
                    let mut ar = r.clone();
                    cho_solve(&l, k, &mut ar);
                    let rar = dot(r, &ar).max(1e-12);
                    (ar, rar)
                });
                for (y, ar) in y.iter_mut().zip(ar.iter()) {
                    *y -= *ar * over / *rar;
                }
                moved = true;
            }
            if let Some(kept) = &kept {
                kept.project(&mut y, &targets);
            }
            if !moved {
                break;
            }
        }
        for (s, y) in s.iter_mut().zip(&y) {
            *s = *y as f32;
        }
    }
}

impl Hold {
    /// The face for a pull of `d` (model space, metres), as asymmetric as `asym` (0..1), before
    /// it's kept within its limits ([`Sculptor::limit`]).
    pub fn pulled(&self, d: Vec3, asym: f32) -> (Vec<f32>, Vec<f32>) {
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
        (s, a)
    }

    /// Goes on from the face `from` (another hand's pull put into it), its point now at `at`.
    pub fn rebase(&mut self, from: (Vec<f32>, Vec<f32>), at: Vec3) {
        self.from = from;
        self.at = at;
    }
}

/// The face's age and gender (rows over its first `k` symmetric coordinates), kept in a cost's way
/// (`l`, its Cholesky factor): a change projected onto the faces with them as they should be.
struct Kept {
    rows: Vec<Vec<f64>>,
    /// `A⁻¹ rᵀ` for each row, and `(R A⁻¹ Rᵀ)⁻¹` (row-major).
    back: Vec<Vec<f64>>,
    inverse: Vec<f64>,
}

impl Kept {
    /// `n` the cost's size (k, or more: asymmetric coordinates after the symmetric ones).
    fn new(l: &[f64], n: usize, c: &FaceControls, k: usize) -> Kept {
        let rows: Vec<Vec<f64>> = c.kept().iter().map(|r| (0..n).map(|i| if i < k { r.get(i).copied().unwrap_or(0.0) as f64 } else { 0.0 }).collect()).collect();
        let back: Vec<Vec<f64>> = rows
            .iter()
            .map(|r| {
                let mut b = r.clone();
                cho_solve(l, n, &mut b);
                b
            })
            .collect();
        let m = rows.len();
        let mut inverse = vec![0f64; m * m];
        let mut g = vec![0f64; m * m];
        for i in 0..m {
            for j in 0..m {
                g[i * m + j] = rows[i].iter().zip(&back[j]).map(|(a, b)| a * b).sum();
            }
            g[i * m + i] += 1e-12;
        }
        for j in 0..m {
            let mut e = vec![0f64; m];
            e[j] = 1.0;
            let mut a = g.clone();
            if rustvr_asset::facegen::solve(&mut a, &mut e, m) {
                for i in 0..m {
                    inverse[i * m + j] = e[i];
                }
            }
        }
        Kept { rows, back, inverse }
    }

    /// `x` brought to `rows · x = targets`, the least change in the cost's way.
    fn project(&self, x: &mut [f64], targets: &[f64]) {
        let m = self.rows.len();
        let miss: Vec<f64> = self.rows.iter().zip(targets).map(|(r, t)| r.iter().zip(x.iter()).map(|(a, b)| a * b).sum::<f64>() - t).collect();
        for j in 0..m {
            let lj: f64 = (0..m).map(|i| self.inverse[j * m + i] * miss[i]).sum();
            for (x, b) in x.iter_mut().zip(&self.back[j]) {
                *x -= b * lj;
            }
        }
    }
}

/// Vertex `v`'s moves by each coordinate (`ks` symmetric, then `ka` asymmetric; a part with fewer
/// has none for the rest).
fn moves(s: &FaceShape, v: usize, ks: usize, ka: usize) -> Vec<[f64; 3]> {
    let get = |set: &Vec<Vec<[f32; 3]>>, k: usize| set.get(k).and_then(|m| m.get(v)).map_or([0.0; 3], |d| [d[0] as f64, d[1] as f64, d[2] as f64]);
    (0..ks).map(|k| get(&s.symmetric, k)).chain((0..ka).map(|k| get(&s.asymmetric, k))).collect()
}

/// `a += scale · Mᵀ M` for one vertex's moves `m` (k of them), `a` k×k row-major: its upper
/// triangle only ([`mirror_upper`] fills the rest).
fn add_outer(a: &mut [f64], m: &[[f64; 3]], k: usize, scale: f64) {
    for r in 0..k.min(m.len()) {
        let mr = m[r];
        if mr == [0.0; 3] {
            continue;
        }
        for c in r..k.min(m.len()) {
            let mc = m[c];
            a[r * k + c] += scale * (mr[0] * mc[0] + mr[1] * mc[1] + mr[2] * mc[2]);
        }
    }
}

/// A symmetric k×k matrix's lower triangle from its upper.
fn mirror_upper(a: &mut [f64], k: usize) {
    for r in 0..k {
        for c in 0..r {
            a[r * k + c] = a[c * k + r];
        }
    }
}

/// The Cholesky factor `L` (lower, row-major) of a symmetric positive definite n×n `a`.
fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0f64; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if s <= 0.0 {
                    return None;
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    Some(l)
}

/// Solves `L Lᵀ x = b` in place.
fn cho_solve(l: &[f64], n: usize, b: &mut [f64]) {
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i * n + k] * b[k];
        }
        b[i] = s / l[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= l[k * n + i] * b[k];
        }
        b[i] = s / l[i * n + i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustvr_asset::FaceControl;

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
        let (sym, asym) = h.pulled(Vec3::new(0.0, 0.01, 0.0), 0.0);
        let at = s.positions(&sym, &asym);
        // The vertex taken goes most of the way up with the hand; the far end stays nearly put.
        assert!(at[0][0].y > 0.007, "the vertex taken went {}", at[0][0].y);
        assert!(at[0][5].y.abs() < 0.002, "the far end went {}", at[0][5].y);
    }

    #[test]
    fn a_sideways_pull_needs_the_asymmetric_morphs() {
        let s = sculptor();
        let h = s.hold(0, 0, (vec![0.0; 3], vec![0.0])).unwrap();
        let (sym, asym) = h.pulled(Vec3::new(0.01, 0.0, 0.0), 0.0);
        assert!(s.positions(&sym, &asym)[0][0].x.abs() < 1e-4);
        let (sym, asym) = h.pulled(Vec3::new(0.01, 0.0, 0.0), 1.0);
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

    #[test]
    fn a_face_is_kept_within_its_limits() {
        let mut s = sculptor();
        // The first coordinate a named control (a slider), the third the face's age.
        let controls = FaceControls { age: vec![0.0, 0.0, 1.0], age_offset: 30.0, controls: vec![FaceControl { name: "first".into(), coefficients: vec![1.0, 0.0, 0.0] }], ..Default::default() };
        s.controls = Some(Arc::new(controls));
        let from = (vec![0.0; 3], vec![0.0]);
        let h = s.hold(0, 0, from.clone()).unwrap();
        let (mut sym, mut asym) = h.pulled(Vec3::new(0.0, 0.2, 0.0), 0.0);
        s.limit(&mut sym, &mut asym, &from, &[&h], &[3.0]);
        assert!(sym[0].abs() <= 3.0 + 1e-3, "the slider is at {}", sym[0]);
        assert!(sym.iter().all(|x| x.abs() <= COORDINATE_LIMIT + 1e-3), "{sym:?}");
        assert!(sym[2].abs() < 1e-3, "the age went {}", sym[2]);
        // As far up as the slider lets it go, and the far end still where it was.
        let at = s.positions(&sym, &asym);
        assert!(at[0][0].y > 0.025 && at[0][5].y.abs() < 0.005, "{:?}", at[0]);
    }
}
