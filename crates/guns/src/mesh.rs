//! Gun geometry: meshes in a gun's model space, as the range draws them and as parts of a game
//! model (see [`MeshSource`]).

use glam::{Mat4, Quat, Vec3};

/// One vertex of range geometry (left-handed metres).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

#[derive(Clone, Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Texture key in the asset cache (Metro models), or a built-in texture name.
    pub texture: Option<String>,
    /// Cut from a game model's mesh: which, and each vertex's index there (so a renderer can draw
    /// the part with that mesh's full vertices and material).
    pub source: Option<MeshSource>,
}

/// Where a part of a game model came from (see [`MeshData::source`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshSource {
    /// The mesh's index in the model.
    pub mesh: usize,
    /// For each of the part's vertices, its index in that mesh.
    pub vertices: Vec<u32>,
}

impl MeshData {
    /// An axis-aligned box, faces wound clockwise seen from outside (D3D front faces).
    pub fn add_box(&mut self, min: Vec3, max: Vec3, color: [u8; 4]) {
        let c = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, max.y, max.z),
            Vec3::new(min.x, max.y, max.z),
        ];
        let size = max - min;
        // (corners, normal, uv scale) per face; corners go bottom-left, top-left, top-right, bottom-right
        // as seen from outside, which is clockwise in a left-handed space.
        let faces: [([usize; 4], Vec3, (f32, f32)); 6] = [
            ([0, 3, 2, 1], -Vec3::Z, (size.x, size.y)),
            ([5, 6, 7, 4], Vec3::Z, (size.x, size.y)),
            ([4, 7, 3, 0], -Vec3::X, (size.z, size.y)),
            ([1, 2, 6, 5], Vec3::X, (size.z, size.y)),
            ([3, 7, 6, 2], Vec3::Y, (size.x, size.z)),
            ([4, 0, 1, 5], -Vec3::Y, (size.x, size.z)),
        ];
        for (idx, n, (su, sv)) in faces {
            let base = self.vertices.len() as u32;
            let uvs = [[0.0, sv], [0.0, 0.0], [su, 0.0], [su, sv]];
            for k in 0..4 {
                self.vertices.push(Vertex { pos: c[idx[k]].to_array(), normal: n.to_array(), uv: uvs[k], color });
            }
            self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    /// A box of half size `half` centred at `c`, turned by `rot`.
    pub fn add_obox(&mut self, c: Vec3, half: Vec3, rot: Quat, color: [u8; 4]) {
        let start = self.vertices.len();
        self.add_box(-half, half, color);
        for v in &mut self.vertices[start..] {
            v.pos = (c + rot * Vec3::from_array(v.pos)).to_array();
            v.normal = (rot * Vec3::from_array(v.normal)).to_array();
        }
    }

    /// Bounds of the vertices after `m`, if there are any.
    pub fn bounds(&self, m: Mat4) -> Option<(Vec3, Vec3)> {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for v in &self.vertices {
            let p = m.transform_point3(Vec3::from_array(v.pos));
            min = min.min(p);
            max = max.max(p);
        }
        (min.x <= max.x).then_some((min, max))
    }

    /// A rounded box (a squircle in cross-section, flat ends) centred at `c`, turned by `rot`: its
    /// section is `half.x` by `half.y`, its length runs along local +Z (`half.z` each way). Grips and
    /// handguards shaped like a real gun's.
    pub fn add_rounded_prism(&mut self, c: Vec3, half: Vec3, rot: Quat, color: [u8; 4]) {
        const N: usize = 24;
        let ring: Vec<(Vec3, Vec3)> = (0..N)
            .map(|i| {
                let t = i as f32 / N as f32 * std::f32::consts::TAU;
                let (s, co) = t.sin_cos();
                // Superellipse of power 4: |x|^4 + |y|^4 = 1.
                let f = |v: f32| v.signum() * v.abs().sqrt();
                let p = Vec3::new(half.x * f(co), half.y * f(s), 0.0);
                let n = Vec3::new(co.signum() * co.abs().powf(1.5) / half.x, s.signum() * s.abs().powf(1.5) / half.y, 0.0).normalize_or(Vec3::X);
                (p, n)
            })
            .collect();
        let at = |p: Vec3| c + rot * p;
        let tri = |m: &mut MeshData, v: [(Vec3, Vec3); 3]| {
            let base = m.vertices.len() as u32;
            let (a, b, cc) = (at(v[0].0), at(v[1].0), at(v[2].0));
            let n = rot * (v[0].1 + v[1].1 + v[2].1);
            // Front faces as `add_box` winds them: (b - a) x (c - a) points out.
            let order = if (b - a).cross(cc - a).dot(n) >= 0.0 { [0, 1, 2] } else { [0, 2, 1] };
            for k in order {
                m.vertices.push(Vertex { pos: at(v[k].0).to_array(), normal: (rot * v[k].1).normalize().to_array(), uv: [0.0, 0.0], color });
            }
            m.indices.extend_from_slice(&[base, base + 1, base + 2]);
        };
        let z = |p: Vec3, z: f32| p + Vec3::Z * z;
        for i in 0..N {
            let (p0, n0) = ring[i];
            let (p1, n1) = ring[(i + 1) % N];
            let (lo0, hi0, lo1, hi1) = (z(p0, -half.z), z(p0, half.z), z(p1, -half.z), z(p1, half.z));
            tri(self, [(lo0, n0), (hi0, n0), (hi1, n1)]);
            tri(self, [(lo0, n0), (hi1, n1), (lo1, n1)]);
            tri(self, [(z(Vec3::ZERO, half.z), Vec3::Z), (hi0, Vec3::Z), (hi1, Vec3::Z)]);
            tri(self, [(z(Vec3::ZERO, -half.z), -Vec3::Z), (lo1, -Vec3::Z), (lo0, -Vec3::Z)]);
        }
    }

    /// A rectangle facing -Z (towards a player looking +Z), centred at `c`.
    pub fn add_plate(&mut self, c: Vec3, half_w: f32, half_h: f32, color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        let n = -Vec3::Z;
        let corners = [(-half_w, -half_h, 0.0, 1.0), (-half_w, half_h, 0.0, 0.0), (half_w, half_h, 1.0, 0.0), (half_w, -half_h, 1.0, 1.0)];
        for (x, y, u, v) in corners {
            self.vertices.push(Vertex { pos: [c.x + x, c.y + y, c.z], normal: n.to_array(), uv: [u, v], color });
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}
