//! Measures a cached model for rigging a gun: which bones own which vertices, and the shape of
//! slices through the mesh (pistol grip axis, sight posts, handguard).
//!
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> parts`
//!     per-bone vertex counts and bounds (each vertex counted for its heaviest bone);
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> slab <axis> <lo> <hi> [<axis> <lo> <hi>...]`
//!     bounds of the vertices inside the box (axis x, y or z), split per mesh and per bone;
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> rows <axis> <lo> <hi> <step> [<axis> <lo> <hi>...]`
//!     slices along `axis` from lo to hi: the bounds of each slice (after the extra filters);
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> hand`
//!     canonical hand frames (Y to the middle finger base, Z the thumb side) from the bind pose.

use rustvr_asset as asset;

fn axis(s: &str) -> usize {
    match s {
        "x" => 0,
        "y" => 1,
        "z" => 2,
        _ => panic!("axis {s}"),
    }
}

fn heaviest(v: &asset::Vertex) -> usize {
    let mut best = 0;
    for k in 1..4 {
        if v.weights[k] > v.weights[best] {
            best = k;
        }
    }
    v.bones[best] as usize
}

#[derive(Clone, Copy)]
struct B {
    min: [f32; 3],
    max: [f32; 3],
    n: usize,
}

impl B {
    fn new() -> B {
        B { min: [f32::MAX; 3], max: [f32::MIN; 3], n: 0 }
    }
    fn add(&mut self, p: [f32; 3]) {
        for (k, v) in p.into_iter().enumerate() {
            self.min[k] = self.min[k].min(v);
            self.max[k] = self.max[k].max(v);
        }
        self.n += 1;
    }
    fn show(&self) -> String {
        if self.n == 0 {
            return "empty".into();
        }
        let c = |k: usize| 0.5 * (self.min[k] + self.max[k]);
        format!(
            "n {:5}  x {:.4}..{:.4}  y {:.4}..{:.4}  z {:.4}..{:.4}  centre ({:.4}, {:.4}, {:.4})",
            self.n, self.min[0], self.max[0], self.min[1], self.max[1], self.min[2], self.max[2], c(0), c(1), c(2)
        )
    }
}

fn filters(args: &[String]) -> Vec<(usize, f32, f32)> {
    args.chunks(3).map(|c| (axis(&c[0]), c[1].parse().unwrap(), c[2].parse().unwrap())).collect()
}

fn inside(p: [f32; 3], f: &[(usize, f32, f32)]) -> bool {
    f.iter().all(|&(a, lo, hi)| p[a] >= lo && p[a] <= hi)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = asset::default_root().join(&args[0]);
    let m = asset::load_model(&dir, &args[1])?;
    let name = |b: usize| m.bones.get(b).map(|b| b.name.as_str()).unwrap_or("?");
    match args[2].as_str() {
        "parts" => {
            let mut per = vec![B::new(); m.bones.len().max(1)];
            for mesh in &m.meshes {
                for v in &mesh.vertices {
                    per[heaviest(v)].add(v.pos);
                }
            }
            for (b, bb) in per.iter().enumerate() {
                if bb.n > 0 {
                    println!("bone {b:3} {:24} {}", name(b), bb.show());
                }
            }
            for (i, mesh) in m.meshes.iter().enumerate() {
                let mut bb = B::new();
                mesh.vertices.iter().for_each(|v| bb.add(v.pos));
                println!("mesh {i:2} {:24} {}", mesh.texture, bb.show());
            }
        }
        "slab" => {
            let f = filters(&args[3..]);
            let mut all = B::new();
            let mut per_mesh = vec![B::new(); m.meshes.len()];
            let mut per_bone = std::collections::BTreeMap::new();
            for (i, mesh) in m.meshes.iter().enumerate() {
                for v in mesh.vertices.iter().filter(|v| inside(v.pos, &f)) {
                    all.add(v.pos);
                    per_mesh[i].add(v.pos);
                    per_bone.entry(heaviest(v)).or_insert_with(B::new).add(v.pos);
                }
            }
            println!("all            {}", all.show());
            for (i, b) in per_mesh.iter().enumerate().filter(|(_, b)| b.n > 0) {
                println!("mesh {i:2}        {}", b.show());
            }
            for (k, b) in per_bone {
                println!("bone {k:3} {:10} {}", name(k), b.show());
            }
        }
        "rows" => {
            let a = axis(&args[3]);
            let (lo, hi, step): (f32, f32, f32) = (args[4].parse()?, args[5].parse()?, args[6].parse()?);
            let f = filters(&args[7..]);
            let mut t = lo;
            while t < hi - 1e-6 {
                let mut b = B::new();
                for mesh in &m.meshes {
                    for v in mesh.vertices.iter().filter(|v| inside(v.pos, &f) && v.pos[a] >= t && v.pos[a] < t + step) {
                        b.add(v.pos);
                    }
                }
                println!("{:.4}..{:.4}  {}", t, t + step, b.show());
                t += step;
            }
        }
        "cuts" => {
            // Cross-sections: every triangle cut by the plane axis = t, bounds of the cut segments.
            let a = axis(&args[3]);
            let (lo, hi, step): (f32, f32, f32) = (args[4].parse()?, args[5].parse()?, args[6].parse()?);
            let f = filters(&args[7..]);
            let mut t = lo;
            while t <= hi + 1e-6 {
                let mut b = B::new();
                for mesh in &m.meshes {
                    for tri in mesh.indices.chunks(3) {
                        let p = [0, 1, 2].map(|k| mesh.vertices[tri[k] as usize].pos);
                        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                            let (da, db) = (p[i][a] - t, p[j][a] - t);
                            if (da <= 0.0) != (db <= 0.0) {
                                let s = da / (da - db);
                                let q = [0, 1, 2].map(|k| p[i][k] + (p[j][k] - p[i][k]) * s);
                                if inside(q, &f) {
                                    b.add(q);
                                }
                            }
                        }
                    }
                }
                println!("{:.4}  {}", t, b.show());
                t += step;
            }
        }
        "hand" => {
            let find = |n: &str| m.bones.iter().position(|b| b.name == n);
            let at = |i: usize| {
                let p = &m.bones[i].model_matrix[12..15];
                [p[0], p[1], p[2]]
            };
            for side in ["l", "r"] {
                let (Some(h), Some(f1), Some(f2), Some(f4)) = (
                    find(&format!("bip01_{side}_hand")),
                    find(&format!("bip01_{side}_finger1")),
                    find(&format!("bip01_{side}_finger2")),
                    find(&format!("bip01_{side}_finger4")),
                ) else {
                    continue;
                };
                let (h, f1, f2, f4) = (at(h), at(f1), at(f2), at(f4));
                let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let norm = |a: [f32; 3]| {
                    let l = dot(a, a).sqrt();
                    [a[0] / l, a[1] / l, a[2] / l]
                };
                let y = norm(sub(f2, h));
                let t = sub(f1, f4);
                let d = dot(t, y);
                let z = norm([t[0] - y[0] * d, t[1] - y[1] * d, t[2] - y[2] * d]);
                let x = [y[1] * z[2] - y[2] * z[1], y[2] * z[0] - y[0] * z[2], y[0] * z[1] - y[1] * z[0]];
                println!("{side}: wrist {h:?}");
                println!("   X {x:?}\n   Y {y:?}\n   Z {z:?}");
            }
        }
        other => anyhow::bail!("unknown mode {other}"),
    }
    Ok(())
}
