//! A low-poly stand-in for a model, made of boxes: what the framework view draws for a gun (each of
//! its parts, so the magazine comes out and the bolt racks as the real ones do).
//!
//! A part's mesh is split into the pieces it's made of (triangles sharing corners: a barrel, a
//! sight, a rail, a grip are usually pieces of their own), and a piece that's long and thin is cut
//! across its length into slabs, so a stock tapers and a barrel stays thin. Each slab keeps the
//! heights it has with gaps between (a rail over a barrel stays two boxes). Each is boxed in the
//! model's own axes. Neighbouring slabs of the same section become one box.

use std::collections::HashMap;

use glam::Vec3;

/// The boxes (min, max) for a mesh in its own space.
pub fn blockout(positions: &[Vec3], indices: &[u32]) -> Vec<(Vec3, Vec3)> {
    let tris: Vec<[Vec3; 3]> = indices
        .chunks_exact(3)
        .filter_map(|t| Some([*positions.get(t[0] as usize)?, *positions.get(t[1] as usize)?, *positions.get(t[2] as usize)?]))
        .collect();
    let mut out = Vec::new();
    for piece in pieces(&tris) {
        boxes_of(&piece, &mut out);
    }
    out
}

/// Triangles grouped by the corners they share (corners within half a millimetre are one).
fn pieces(tris: &[[Vec3; 3]]) -> Vec<Vec<[Vec3; 3]>> {
    let key = |p: Vec3| ((p.x * 2000.0).round() as i32, (p.y * 2000.0).round() as i32, (p.z * 2000.0).round() as i32);
    let mut ids: HashMap<(i32, i32, i32), usize> = HashMap::new();
    let mut parent: Vec<usize> = Vec::new();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let corner_ids: Vec<[usize; 3]> = tris
        .iter()
        .map(|t| {
            t.map(|p| {
                *ids.entry(key(p)).or_insert_with(|| {
                    parent.push(parent.len());
                    parent.len() - 1
                })
            })
        })
        .collect();
    for c in &corner_ids {
        for k in 1..3 {
            let (a, b) = (find(&mut parent, c[0]), find(&mut parent, c[k]));
            if a != b {
                parent[b] = a;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<[Vec3; 3]>> = HashMap::new();
    for (t, c) in tris.iter().zip(&corner_ids) {
        let root = find(&mut parent, c[0]);
        groups.entry(root).or_default().push(*t);
    }
    let mut v: Vec<Vec<[Vec3; 3]>> = groups.into_values().collect();
    v.sort_by_key(|g| std::cmp::Reverse(g.len()));
    v
}

fn bounds(points: impl IntoIterator<Item = Vec3>) -> Option<(Vec3, Vec3)> {
    points.into_iter().fold(None, |b, p| Some(b.map_or((p, p), |(lo, hi): (Vec3, Vec3)| (lo.min(p), hi.max(p)))))
}

/// Points over a triangle at most `step` apart (its corners and between), so a slab a long
/// triangle crosses still has points in it.
fn sample(t: &[Vec3; 3], step: f32, out: &mut Vec<Vec3>) {
    let longest = (t[1] - t[0]).length().max((t[2] - t[1]).length()).max((t[0] - t[2]).length());
    let n = ((longest / step).ceil() as usize).clamp(1, 64);
    for i in 0..=n {
        for j in 0..=(n - i) {
            let (a, b) = (i as f32 / n as f32, j as f32 / n as f32);
            out.push(t[0] + (t[1] - t[0]) * a + (t[2] - t[0]) * b);
        }
    }
}

/// Smallest box side kept (a box thinner than this is grown to it).
const MIN_SIDE: f32 = 0.003;
/// Slab length a long piece is cut into, at most this many slabs.
const SLAB: f32 = 0.03;
const MAX_SLABS: usize = 24;
/// A gap across a slab at least this big splits it into boxes of their own.
const GAP: f32 = 0.012;
/// Neighbouring slabs whose boxes differ by less than this are merged.
const SAME: f32 = 0.004;

fn boxes_of(piece: &[[Vec3; 3]], out: &mut Vec<(Vec3, Vec3)>) {
    let Some((lo, hi)) = bounds(piece.iter().flatten().copied()) else { return };
    let size = hi - lo;
    if size.max_element() < 0.004 {
        return; // a speck (a screw head)
    }
    let axis = if size.x >= size.y && size.x >= size.z {
        0
    } else if size.y >= size.z {
        1
    } else {
        2
    };
    let len = size[axis];
    let mut sides = [size.x, size.y, size.z];
    sides.sort_by(|a, b| b.total_cmp(a));
    // Short or chunky: one box.
    if len < 0.08 || len < sides[1] * 1.8 {
        out.push(grow(lo, hi));
        return;
    }
    let n = ((len / SLAB).ceil() as usize).clamp(2, MAX_SLABS);
    let slab = len / n as f32;
    let mut points = Vec::new();
    for t in piece {
        sample(t, slab * 0.5, &mut points);
    }
    // Each slab's boxes: its points split where they leave a gap across the slab (up the model's
    // y, or along the other cross axis when y is the long one).
    let across = if axis == 1 { 2 } else { 1 };
    // Boxes still open (the last slab had them): each slab extends the ones it continues with the
    // same section; the rest are finished.
    let mut open: Vec<(Vec3, Vec3)> = Vec::new();
    for k in 0..n {
        let (a, b) = (lo[axis] + slab * k as f32, lo[axis] + slab * (k + 1) as f32);
        let mut in_slab: Vec<Vec3> = points.iter().copied().filter(|p| p[axis] >= a - 1e-5 && p[axis] <= b + 1e-5).collect();
        in_slab.sort_by(|p, q| p[across].total_cmp(&q[across]));
        let mut here: Vec<(Vec3, Vec3)> = Vec::new();
        let mut start = 0;
        for i in 1..=in_slab.len() {
            if i == in_slab.len() || in_slab[i][across] - in_slab[i - 1][across] > GAP {
                if let Some((mut l, mut h)) = bounds(in_slab[start..i].iter().copied()) {
                    l[axis] = l[axis].max(a);
                    h[axis] = h[axis].min(b);
                    here.push((l, h));
                }
                start = i;
            }
        }
        let mut next: Vec<(Vec3, Vec3)> = Vec::new();
        for (l, h) in here {
            let same = open.iter().position(|(ol, oh)| (0..3).filter(|&c| c != axis).all(|c| (ol[c] - l[c]).abs() < SAME && (oh[c] - h[c]).abs() < SAME));
            match same {
                Some(i) => {
                    let (ol, oh) = open.remove(i);
                    next.push((ol.min(l), oh.max(h)));
                }
                None => next.push((l, h)),
            }
        }
        out.extend(open.drain(..).map(|(l, h)| grow(l, h)));
        open = next;
    }
    out.extend(open.into_iter().map(|(l, h)| grow(l, h)));
}

fn grow(lo: Vec3, hi: Vec3) -> (Vec3, Vec3) {
    let c = (lo + hi) * 0.5;
    let half = ((hi - lo) * 0.5).max(Vec3::splat(MIN_SIDE * 0.5));
    (c - half, c + half)
}

/// A box's twelve triangles (corners through `to`), outward.
pub fn box_triangles(lo: Vec3, hi: Vec3, to: impl Fn(Vec3) -> Vec3) -> [[Vec3; 3]; 12] {
    let c = |i: usize| to(Vec3::new(if i & 1 != 0 { hi.x } else { lo.x }, if i & 2 != 0 { hi.y } else { lo.y }, if i & 4 != 0 { hi.z } else { lo.z }));
    let v: [Vec3; 8] = std::array::from_fn(c);
    let q = |a: usize, b: usize, cc: usize, d: usize| [[v[a], v[b], v[cc]], [v[a], v[cc], v[d]]];
    let faces = [q(0, 2, 3, 1), q(4, 5, 7, 6), q(0, 1, 5, 4), q(2, 6, 7, 3), q(0, 4, 6, 2), q(1, 3, 7, 5)];
    std::array::from_fn(|i| faces[i / 2][i % 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box's mesh: 8 corners, 12 triangles.
    fn cuboid(lo: Vec3, hi: Vec3, base: u32, pos: &mut Vec<Vec3>, idx: &mut Vec<u32>) {
        for i in 0..8 {
            pos.push(Vec3::new(if i & 1 != 0 { hi.x } else { lo.x }, if i & 2 != 0 { hi.y } else { lo.y }, if i & 4 != 0 { hi.z } else { lo.z }));
        }
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            idx.extend([f[0], f[1], f[2], f[0], f[2], f[3]].map(|k| base + k));
        }
    }

    #[test]
    fn a_gun_of_pieces_is_boxed_piece_by_piece() {
        // A receiver, a long thin barrel in front of it and a grip under it, not touching.
        let (mut pos, mut idx) = (Vec::new(), Vec::new());
        cuboid(Vec3::new(-0.02, 0.0, 0.0), Vec3::new(0.02, 0.06, 0.25), 0, &mut pos, &mut idx);
        cuboid(Vec3::new(-0.01, 0.02, 0.26), Vec3::new(0.01, 0.04, 0.66), 8, &mut pos, &mut idx);
        cuboid(Vec3::new(-0.015, -0.1, 0.05), Vec3::new(0.015, -0.005, 0.09), 16, &mut pos, &mut idx);
        let b = blockout(&pos, &idx);
        // The barrel is one box (a uniform section is merged back), the others theirs.
        assert_eq!(b.len(), 3, "{b:?}");
        let barrel = b.iter().find(|(l, _)| l.z > 0.255).expect("the barrel");
        assert!((barrel.1.z - 0.66).abs() < 0.002 && (barrel.0.z - 0.26).abs() < 0.002, "{barrel:?}");
        assert!((barrel.1.y - barrel.0.y - 0.02).abs() < 0.002, "as thin as it is");
    }

    #[test]
    fn a_tapering_stock_is_stepped_and_a_rail_over_a_barrel_stays_apart() {
        // A wedge 40 cm long, 2 cm high at one end and 10 cm at the other: one piece.
        let pos = vec![
            Vec3::new(-0.02, 0.0, 0.0),
            Vec3::new(0.02, 0.0, 0.0),
            Vec3::new(-0.02, 0.02, 0.0),
            Vec3::new(0.02, 0.02, 0.0),
            Vec3::new(-0.02, 0.0, 0.4),
            Vec3::new(0.02, 0.0, 0.4),
            Vec3::new(-0.02, 0.1, 0.4),
            Vec3::new(0.02, 0.1, 0.4),
        ];
        let mut idx = Vec::new();
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            idx.extend([f[0], f[1], f[2], f[0], f[2], f[3]]);
        }
        let b = blockout(&pos, &idx);
        assert!(b.len() >= 5, "stepped: {}", b.len());
        let high = |z: f32| b.iter().filter(|(l, h)| l.z <= z && h.z >= z).map(|(_, h)| h.y).fold(0.0, f32::max);
        assert!(high(0.02) < 0.035 && high(0.38) > 0.085, "low at the thin end, high at the thick: {} {}", high(0.02), high(0.38));

        // A rail 3 cm over a barrel, joined at the ends only: two boxes along most of it.
        let (mut pos, mut idx) = (Vec::new(), Vec::new());
        cuboid(Vec3::new(-0.01, 0.0, 0.0), Vec3::new(0.01, 0.02, 0.4), 0, &mut pos, &mut idx);
        cuboid(Vec3::new(-0.01, 0.05, 0.0), Vec3::new(0.01, 0.06, 0.4), 8, &mut pos, &mut idx);
        cuboid(Vec3::new(-0.01, 0.0, 0.38), Vec3::new(0.01, 0.06, 0.4), 16, &mut pos, &mut idx);
        let b = blockout(&pos, &idx);
        let mid = b.iter().filter(|(l, h)| l.z <= 0.2 && h.z >= 0.2).count();
        assert_eq!(mid, 2, "the barrel and the rail, apart: {b:?}");
    }
}
