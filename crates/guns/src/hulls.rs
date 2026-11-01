//! A model's convex parts, as its collision (`rustvr_protocol::GameHulls`, several for one object):
//! what it rests on and knocks with, and what the hands' fingers close on when they take it where
//! it has no handle. One box round a whole gun has its fingers closing on the air beside it.
//!
//! The parts come from an approximate convex decomposition of the model's triangles (V-HACD, as
//! parry has it): a gun's receiver, barrel, stock, grip each their own, a sword's blade, guard and
//! handle.

use glam::{Mat4, Vec3};
use parry3d::math::Vector;
use parry3d::query::PointQuery;
use parry3d::shape::ConvexPolyhedron;
use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
use rustvr_protocol::{GameHull, MAX_HULL_POINTS};

use crate::{MeshData, Weapon};

/// The voxel grid the decomposition works on (across the model's longest side), and how far a part
/// may be from convex (a share of the model's size): fine enough for a barrel or a blade.
const RESOLUTION: u32 = 96;
const CONCAVITY: f32 = 0.004;

/// The convex parts of the triangles (each part its corner points, at most `max_points` of them),
/// at most `max_parts` of them, the biggest first.
pub fn convex_parts(triangles: &[[Vec3; 3]], max_parts: usize, max_points: usize) -> Vec<Vec<Vec3>> {
    convex_parts_with(triangles, max_parts, max_points, RESOLUTION, CONCAVITY)
}

/// [`convex_parts`] on a voxel grid of `resolution` across the triangles' longest side, each part at
/// most `concavity` (a share of their size) from convex: coarser is much quicker (a thing worked
/// out as it's offered, not a gun before the game starts).
pub fn convex_parts_with(triangles: &[[Vec3; 3]], max_parts: usize, max_points: usize, resolution: u32, concavity: f32) -> Vec<Vec<Vec3>> {
    if triangles.is_empty() {
        return Vec::new();
    }
    let points: Vec<Vector> = triangles.iter().flatten().map(|p| Vector::new(p.x, p.y, p.z)).collect();
    let indices: Vec<[u32; 3]> = (0..triangles.len() as u32).map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect();
    let params = VHACDParameters { resolution, concavity, max_convex_hulls: max_parts.max(1) as u32, ..Default::default() };
    let parts = VHACD::decompose(&params, &points, &indices, false).compute_convex_hulls(4);
    let mut out: Vec<Vec<Vec3>> = parts
        .into_iter()
        .map(|(pts, _)| pts.into_iter().map(|p| Vec3::new(p.x, p.y, p.z)).collect::<Vec<_>>())
        .filter(|p| p.len() >= 4)
        .map(|p| fewest(p, max_points))
        .collect();
    out.sort_by(|a, b| extent(b).total_cmp(&extent(a)));
    out.truncate(max_parts);
    out
}

/// [`convex_parts_with`] for a thing of any shape. Each separate piece of it that is a solid its
/// own hull fits (a gate's bars and crossbars, a chair's legs, a sword's guard) is a part as it is,
/// over and above `max_parts`; the rest is decomposed on a grid the finer the longer and thinner it is (up to [`RESOLUTION`]: a
/// sword's grip is a voxel or two across on a bowl's grid), with a part more for each connected
/// piece of its triangles that no part covers (within a voxel), so nothing drawn is lost. A flat
/// piece is given a millimetre's thickness.
pub fn convex_parts_covering(triangles: &[[Vec3; 3]], max_parts: usize, max_points: usize, resolution: u32, concavity: f32) -> Vec<Vec<Vec3>> {
    let Some((lo, size, grid)) = grid(triangles, resolution) else { return Vec::new() };
    let (mut whole, rest) = whole_pieces(triangles, lo, size, size / grid.max(1) as f32, max_parts, max_points);
    if whole.is_empty() {
        return decomposed(triangles, max_parts, max_points, resolution, concavity);
    }
    let rest: Vec<[Vec3; 3]> = rest.iter().map(|&t| triangles[t]).collect();
    if !rest.is_empty() {
        whole.extend(decomposed(&rest, max_parts, max_points, resolution, concavity));
    }
    whole
}

/// The triangles' bounds' low corner and longest side, and the grid for them: `resolution` across
/// it, finer for a long thin thing (its longest side against its middle one).
fn grid(triangles: &[[Vec3; 3]], resolution: u32) -> Option<(Vec3, f32, u32)> {
    let (lo, hi) = triangles.iter().flatten().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let sides = hi - lo;
    let size = sides.max_element();
    if triangles.is_empty() || !size.is_finite() || size <= 0.0 {
        return None;
    }
    let middle = sides.x + sides.y + sides.z - size - sides.min_element();
    let long = (size / middle.max(size * 0.01)).clamp(1.0, RESOLUTION as f32 / resolution.max(1) as f32);
    Some((lo, size, (resolution as f32 * long).round() as u32))
}

/// The triangles decomposed ([`convex_parts_covering`] without its whole pieces).
fn decomposed(triangles: &[[Vec3; 3]], max_parts: usize, max_points: usize, resolution: u32, concavity: f32) -> Vec<Vec<Vec3>> {
    let Some((lo, size, resolution)) = grid(triangles, resolution) else { return Vec::new() };
    let mut parts = convex_parts_with(triangles, max_parts, max_points, resolution, concavity);
    let near = size / resolution.max(1) as f32;
    let solids: Vec<ConvexPolyhedron> = parts.iter().filter_map(|p| ConvexPolyhedron::from_convex_hull(&p.iter().map(|q| Vector::new(q.x, q.y, q.z)).collect::<Vec<_>>())).collect();
    let covered = |p: Vec3| solids.iter().any(|s| s.distance_to_local_point(Vector::new(p.x, p.y, p.z), true) <= near);
    let bare: Vec<usize> = (0..triangles.len()).filter(|&i| !covered((triangles[i][0] + triangles[i][1] + triangles[i][2]) / 3.0)).collect();
    if bare.is_empty() {
        return parts;
    }
    let mut extra: Vec<Vec<Vec3>> = pieces(triangles, &bare, lo, size)
        .into_iter()
        .map(|tris| {
            let mut points: Vec<Vec3> = tris.iter().flat_map(|&t| triangles[t]).collect();
            // (Flat: a millimetre either side of it.)
            let n = tris.iter().map(|&t| (triangles[t][1] - triangles[t][0]).cross(triangles[t][2] - triangles[t][0])).sum::<Vec3>().normalize_or_zero();
            if n != Vec3::ZERO && points.iter().all(|p| (*p - points[0]).dot(n).abs() < 1e-4) {
                let flat = points.clone();
                points = flat.iter().flat_map(|p| [*p + n * 0.0005 * size.min(1.0), *p - n * 0.0005 * size.min(1.0)]).collect();
            }
            fewest(points, max_points)
        })
        .filter(|p| p.len() >= 4)
        .collect();
    extra.sort_by(|a, b| extent(b).total_cmp(&extent(a)));
    parts.extend(extra.into_iter().take(max_parts / 2));
    parts
}

/// The triangles `of` (indices into `triangles`) in connected pieces: sharing a corner, to a hair
/// (a ten-thousandth of `size`, from `lo`).
fn pieces(triangles: &[[Vec3; 3]], of: &[usize], lo: Vec3, size: f32) -> Vec<Vec<usize>> {
    let key = |p: Vec3| ((p - lo) / (size * 1e-4)).round().as_ivec3();
    let mut root: Vec<usize> = (0..of.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    let mut by_corner: std::collections::HashMap<glam::IVec3, usize> = std::collections::HashMap::new();
    for (k, &t) in of.iter().enumerate() {
        for p in triangles[t] {
            if let Some(&other) = by_corner.get(&key(p)) {
                let (a, b) = (find(&mut root, k), find(&mut root, other));
                root[a] = b;
            } else {
                by_corner.insert(key(p), k);
            }
        }
    }
    let mut out: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for k in 0..of.len() {
        let r = find(&mut root, k);
        out.entry(r).or_default().push(of[k]);
    }
    let mut out: Vec<Vec<usize>> = out.into_values().collect();
    out.sort_by_key(|p| p[0]);
    out
}

/// Of a hull's surface, the share that must lie within a voxel of the piece's own triangles for the
/// piece to be its hull; how many points of the surface are held up to them; the most triangles a
/// piece may have to be looked at.
const FIT: f32 = 0.95;
const FIT_SAMPLES: usize = 256;
const FIT_MOST_TRIANGLES: usize = 4000;

/// The pieces of the triangles that are each a solid their hull fits (within `near`): their hulls'
/// points (at most `max_points`), and the triangles of the rest. None when the thing is one piece,
/// or more of its pieces fit than twice `max_parts` (a fence's hundred pickets: decomposed as a
/// whole).
fn whole_pieces(triangles: &[[Vec3; 3]], lo: Vec3, size: f32, near: f32, max_parts: usize, max_points: usize) -> (Vec<Vec<Vec3>>, Vec<usize>) {
    let all: Vec<usize> = (0..triangles.len()).collect();
    let pieces = pieces(triangles, &all, lo, size);
    if pieces.len() < 2 || pieces.len() > 8 * max_parts {
        return (Vec::new(), all);
    }
    let (mut whole, mut rest) = (Vec::new(), Vec::new());
    for p in pieces {
        let tris: Vec<[Vec3; 3]> = p.iter().map(|&t| triangles[t]).collect();
        match (tris.len() <= FIT_MOST_TRIANGLES).then(|| hull_if_fits(&tris, near)).flatten() {
            Some(hull) => whole.push(fewest(hull, max_points)),
            None => rest.extend(p),
        }
    }
    if whole.is_empty() || whole.len() > 2 * max_parts {
        return (Vec::new(), all);
    }
    whole.sort_by(|a, b| extent(b).total_cmp(&extent(a)));
    rest.sort_unstable();
    (whole, rest)
}

/// A piece's hull's corners, if nearly all its hull's surface (by area) lies within `near` of the
/// piece's triangles: a bar, a leg, a block. A cup's (the hull's lid over its mouth), a ring's or a
/// bent bar's (the hull across its bend) don't. Flat pieces have no hull here.
fn hull_if_fits(tris: &[[Vec3; 3]], near: f32) -> Option<Vec<Vec3>> {
    let points: Vec<Vector> = tris.iter().flatten().map(|p| Vector::new(p.x, p.y, p.z)).collect();
    let (corners, faces) = std::panic::catch_unwind(|| parry3d::transformation::convex_hull(&points)).ok()?;
    if corners.len() < 4 || faces.len() < 4 {
        return None;
    }
    let corners: Vec<Vec3> = corners.iter().map(|p| Vec3::new(p.x, p.y, p.z)).collect();
    let face = |f: &[u32; 3]| [corners[f[0] as usize], corners[f[1] as usize], corners[f[2] as usize]];
    let area = |t: &[Vec3; 3]| (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5;
    let total: f32 = faces.iter().map(|f| area(&face(f))).sum();
    if !(total > 0.0) {
        return None;
    }
    let shapes: Vec<parry3d::shape::Triangle> = tris.iter().map(|t| parry3d::shape::Triangle::new(Vector::new(t[0].x, t[0].y, t[0].z), Vector::new(t[1].x, t[1].y, t[1].z), Vector::new(t[2].x, t[2].y, t[2].z))).collect();
    let near_piece = |p: Vec3| shapes.iter().any(|s| s.distance_to_local_point(Vector::new(p.x, p.y, p.z), true) <= near);
    // Points spread over each face by its area (a fixed scatter, the same every time).
    let mut held = 0.0;
    for f in &faces {
        let t = face(f);
        let a = area(&t);
        let n = ((a / total * FIT_SAMPLES as f32).ceil() as usize).clamp(1, FIT_SAMPLES);
        let on = (0..n)
            .filter(|&k| {
                let (mut u, mut v) = (((k as f32 + 0.5) * 0.618_034).fract(), ((k as f32 + 0.5) * 0.754_877_7).fract());
                if u + v > 1.0 {
                    (u, v) = (1.0 - u, 1.0 - v);
                }
                near_piece(t[0] + (t[1] - t[0]) * u + (t[2] - t[0]) * v)
            })
            .count();
        held += a * on as f32 / n as f32;
    }
    (held >= FIT * total).then_some(corners)
}

/// A part's size (its bounds' volume), for keeping the biggest.
fn extent(points: &[Vec3]) -> f32 {
    let (lo, hi) = points.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    (hi - lo).max(Vec3::ZERO).element_product()
}

/// At most `max` of the points, spread over the part (each next the one furthest from those kept):
/// the hull of what's kept is nearly the whole one's.
fn fewest(points: Vec<Vec3>, max: usize) -> Vec<Vec3> {
    if points.len() <= max {
        return points;
    }
    let centre = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let first = points.iter().copied().max_by(|a, b| a.distance(centre).total_cmp(&b.distance(centre))).unwrap();
    let mut kept = vec![first];
    let mut nearest: Vec<f32> = points.iter().map(|p| p.distance(first)).collect();
    while kept.len() < max {
        let (k, d) = nearest.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        if d <= 1e-5 {
            break;
        }
        kept.push(points[k]);
        for (n, p) in nearest.iter_mut().zip(&points) {
            *n = n.min(p.distance(points[k]));
        }
    }
    kept
}

/// The triangles of `parts` (indices into `meshes`), each moved by `fix`.
pub fn triangles(meshes: &[MeshData], parts: impl IntoIterator<Item = usize>, fix: Mat4) -> Vec<[Vec3; 3]> {
    let mut out = Vec::new();
    for m in parts.into_iter().filter_map(|k| meshes.get(k)) {
        let at = |i: u32| fix.transform_point3(Vec3::from_array(m.vertices[i as usize].pos));
        out.extend(m.indices.chunks_exact(3).map(|t| [at(t[0]), at(t[1]), at(t[2])]));
    }
    out
}

/// A gun's convex parts in weapon space: everything but its magazine (its own object) and trigger.
pub fn weapon_parts(w: &Weapon, max_parts: usize) -> Vec<Vec<Vec3>> {
    let kept = (0..w.parts.len()).filter(|k| !w.mag_parts.contains(k) && !w.trigger_parts.contains(k));
    convex_parts(&triangles(&w.parts, kept, w.fix), max_parts, MAX_HULL_POINTS)
}

/// Convex parts as hull table entries for object `id` (each part's points thinned to what an entry
/// holds, the ones furthest apart).
pub fn hull_entries(id: u32, parts: &[Vec<Vec3>]) -> Vec<GameHull> {
    parts
        .iter()
        .map(|p| {
            let p = fewest(p.clone(), MAX_HULL_POINTS);
            let mut h: GameHull = bytemuck::Zeroable::zeroed();
            h.id = id;
            h.count = p.len() as u32;
            for (k, q) in p.iter().enumerate() {
                h.points[k] = [q.x, q.y, q.z, 0.0];
            }
            h
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxes(list: &[(Vec3, Vec3)]) -> Vec<[Vec3; 3]> {
        let mut out = Vec::new();
        for &(lo, hi) in list {
            let c = |i: usize| Vec3::new([lo.x, hi.x][i & 1], [lo.y, hi.y][(i >> 1) & 1], [lo.z, hi.z][i >> 2]);
            for f in [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
                out.push([c(f[0]), c(f[1]), c(f[2])]);
                out.push([c(f[0]), c(f[2]), c(f[3])]);
            }
        }
        out
    }

    #[test]
    fn an_l_shape_comes_apart_into_its_two_arms() {
        // A long bar and a grip hanging under one end (a pistol, side on).
        let t = boxes(&[(Vec3::new(-0.015, 0.0, 0.0), Vec3::new(0.015, 0.03, 0.2)), (Vec3::new(-0.015, -0.1, 0.0), Vec3::new(0.015, 0.0, 0.03))]);
        let parts = convex_parts(&t, 8, 64);
        assert!(parts.len() >= 2, "{} parts", parts.len());
        // No part fills the empty corner under the bar's far end.
        let corner = Vec3::new(0.0, -0.06, 0.15);
        for p in &parts {
            let (lo, hi) = p.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), q| (lo.min(*q), hi.max(*q)));
            assert!(!(corner.cmpge(lo).all() && corner.cmple(hi).all()), "a part spans the empty corner: {lo:?}..{hi:?}");
        }
        assert!(parts.iter().all(|p| p.len() <= 64));
    }

    #[test]
    fn fewest_keeps_the_extremes() {
        let pts: Vec<Vec3> = (0..200).map(|k| Vec3::new((k as f32 * 0.37).sin(), (k as f32 * 0.11).cos(), k as f32 / 200.0)).collect();
        let kept = fewest(pts.clone(), 16);
        assert_eq!(kept.len(), 16);
        let reach = |s: &[Vec3]| s.iter().map(|p| p.z).fold(f32::MIN, f32::max);
        assert!(reach(&kept) > 0.9 * reach(&pts));
    }
}
