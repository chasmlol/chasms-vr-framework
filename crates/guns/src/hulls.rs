//! A model's convex parts, as its collision (`rustvr_protocol::GameHulls`, several for one object):
//! what it rests on and knocks with, and what the hands' fingers close on when they take it where
//! it has no handle. One box round a whole gun has its fingers closing on the air beside it.
//!
//! The parts come from an approximate convex decomposition of the model's triangles (V-HACD, as
//! parry has it): a gun's receiver, barrel, stock, grip each their own, a sword's blade, guard and
//! handle.

use glam::{Mat4, Vec3};
use parry3d::math::Vector;
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
    if triangles.is_empty() {
        return Vec::new();
    }
    let points: Vec<Vector> = triangles.iter().flatten().map(|p| Vector::new(p.x, p.y, p.z)).collect();
    let indices: Vec<[u32; 3]> = (0..triangles.len() as u32).map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect();
    let params = VHACDParameters { resolution: RESOLUTION, concavity: CONCAVITY, max_convex_hulls: max_parts.max(1) as u32, ..Default::default() };
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
