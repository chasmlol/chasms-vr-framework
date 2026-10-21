//! Measures a cached model for rigging a gun: which bones own which vertices, and the shape of
//! slices through the mesh (pistol grip axis, sight posts, handguard).
//!
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> parts`
//!     per-bone vertex counts and bounds (each vertex counted for its heaviest bone);
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> slab <axis> <lo> <hi> [<axis> <lo> <hi>...]`
//!     bounds of the vertices inside the box (axis x, y or z), split per mesh and per bone;
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> rows <axis> <lo> <hi> <step> [<axis> <lo> <hi>...]`
//!     slices along `axis` from lo to hi: the bounds of each slice (after the extra filters);
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> cuts <axis> <lo> <hi> <step> [<axis> <lo> <hi>...]`
//!     cross-sections: the bounds of where the triangles cross each plane (after the filters);
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> hand`
//!     canonical hand frames (Y to the middle finger base, Z the thumb side) from the bind pose;
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> bones`
//!     every bone's bind pose (model space): its origin, the pivot the game's animations turn its
//!     skin about (a break action's hinge, a lever's pivot), and its axes;
//! `cargo run -p rustvr-asset --example rig_probe -- <game> <key> survey`
//!     the common measurements of a gun in one go (see docs/RIGGING.md): its parts (bones, with
//!     numbered siblings folded together) and locators, the sight profile along the game's aim
//!     line with the rear notch and front post it finds, the pistol grip's sections under the
//!     game's firing palm with its rake and block, the trigger blade and where its pad goes, the
//!     handguard's section at the game's support palm, and a rig entry to start from.
//!
//! Without `RUSTVR_ASSETS` the game folder is `local_assets/<game>` at the repository root.

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
                println!("mesh {i:2} {:24} {:24} {}", mesh.texture, mesh.name, bb.show());
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
        "bones" => {
            // Every bone's bind pose in model space: where it is (the pivot the game's animations
            // turn its skin about) and its axes, with its parent.
            for (i, b) in m.bones.iter().enumerate() {
                let c = &b.model_matrix;
                let axis = |k: usize| [c[4 * k], c[4 * k + 1], c[4 * k + 2]];
                let f = |a: [f32; 3]| format!("({:7.4}, {:7.4}, {:7.4})", a[0], a[1], a[2]);
                println!("bone {i:3} {:24} under {:24} at {}  X {}  Y {}  Z {}", b.name, name(b.parent.max(0) as usize).to_string() + if b.parent < 0 { " (root)" } else { "" }, f(axis(3)), f(axis(0)), f(axis(1)), f(axis(2)));
            }
        }
        "survey" => survey::run(&dir, &args[1], &m)?,
        other => anyhow::bail!("unknown mode {other}"),
    }
    Ok(())
}

/// The `survey` mode: the measurements every rig needs, found automatically where the geometry
/// makes it clear, printed with the evidence so they can be checked (and then verified by the
/// range's rig renders). Everything is measured on cross-sections of the mesh (where its triangles
/// cross a plane), so it doesn't depend on where the model happens to have vertices.
mod survey {
    use glam::Vec3;
    use rustvr_asset as asset;
    use std::collections::{BTreeMap, HashMap};
    use std::path::Path;

    fn bone_pos(m: &asset::Model, name: &str) -> Option<Vec3> {
        let b = m.bones.iter().find(|b| b.name == name)?;
        Some(Vec3::new(b.model_matrix[12], b.model_matrix[13], b.model_matrix[14]))
    }

    fn heaviest(v: &asset::Vertex) -> Option<usize> {
        let k = (0..4).max_by_key(|&k| v.weights[k])?;
        (v.weights[k] > 0).then_some(v.bones[k] as usize)
    }

    fn bounds(points: impl IntoIterator<Item = Vec3>) -> Option<(Vec3, Vec3)> {
        points.into_iter().fold(None, |b, p| Some(b.map_or((p, p), |(lo, hi): (Vec3, Vec3)| (lo.min(p), hi.max(p)))))
    }

    fn v3(p: Vec3) -> String {
        format!("[{:.4}, {:.4}, {:.4}]", p.x, p.y, p.z)
    }

    fn range(lo: Vec3, hi: Vec3) -> String {
        format!("x {:.4}..{:.4}  y {:.4}..{:.4}  z {:.4}..{:.4}", lo.x, hi.x, lo.y, hi.y, lo.z, hi.z)
    }

    /// A bone's name without its trailing number (`patron_07` -> `patron_`), for folding siblings.
    fn stem(name: &str) -> &str {
        name.trim_end_matches(|c: char| c.is_ascii_digit())
    }

    /// Names that usually mean a magazine (or the rounds a gun is loaded with), a bolt or handle,
    /// and a trigger, in Metro's Russian.
    const MAG_WORDS: &[&str] = &["magazin", "pul", "patron", "baraban", "obojm", "oboym", "korob", "gilz", "drotik", "strel", "shar", "bolt_"];
    const BOLT_WORDS: &[&str] = &["zatvor", "rukoyat", "pomp", "cev", "nasos", "pump", "rychag", "ruchk", "vzvod", "zatvor"];
    const TRIGGER_WORDS: &[&str] = &["kurok", "spusk", "trigger"];

    fn has_word(name: &str, words: &[&str]) -> bool {
        let n = name.to_ascii_lowercase();
        words.iter().any(|w| n.contains(w))
    }

    /// A triangle and the bone carrying most of it.
    struct Tri {
        p: [Vec3; 3],
        bone: Option<usize>,
    }

    /// Where the triangles cross the plane `axis = t`: segments.
    fn section<'a>(tris: impl Iterator<Item = &'a Tri>, axis: usize, t: f32) -> Vec<(Vec3, Vec3)> {
        let mut out = Vec::new();
        for tri in tris {
            let mut cut = Vec::with_capacity(2);
            for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                let (a, b) = (tri.p[i], tri.p[j]);
                let (da, db) = (a[axis] - t, b[axis] - t);
                if (da <= 0.0) != (db <= 0.0) {
                    cut.push(a + (b - a) * (da / (da - db)));
                }
            }
            if cut.len() == 2 {
                out.push((cut[0], cut[1]));
            }
        }
        out
    }

    /// The connected outlines of a section (segments sharing ends, to 0.05 mm).
    fn outlines(segs: &[(Vec3, Vec3)]) -> Vec<Vec<(Vec3, Vec3)>> {
        let key = |p: Vec3| ((p.x * 20000.0).round() as i64, (p.y * 20000.0).round() as i64, (p.z * 20000.0).round() as i64);
        let mut parent: Vec<usize> = (0..segs.len()).collect();
        fn find(parent: &mut [usize], i: usize) -> usize {
            let mut r = i;
            while parent[r] != r {
                r = parent[r];
            }
            let mut i = i;
            while parent[i] != r {
                let n = parent[i];
                parent[i] = r;
                i = n;
            }
            r
        }
        let mut seen: HashMap<(i64, i64, i64), usize> = HashMap::new();
        for (i, (a, b)) in segs.iter().enumerate() {
            for p in [a, b] {
                match seen.get(&key(*p)) {
                    Some(&j) => {
                        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                        parent[ri] = rj;
                    }
                    None => {
                        seen.insert(key(*p), i);
                    }
                }
            }
        }
        let mut groups: BTreeMap<usize, Vec<(Vec3, Vec3)>> = BTreeMap::new();
        for i in 0..segs.len() {
            let r = find(&mut parent, i);
            groups.entry(r).or_default().push(segs[i]);
        }
        groups.into_values().collect()
    }

    /// The highest point of a z-section's outline straight above `x` (below `below`).
    fn top_at(segs: &[(Vec3, Vec3)], x: f32, below: f32) -> Option<f32> {
        segs.iter()
            .filter_map(|(a, b)| {
                let (lo, hi) = if a.x <= b.x { (a, b) } else { (b, a) };
                if x < lo.x || x > hi.x {
                    return None;
                }
                let s = if hi.x - lo.x > 1e-7 { (x - lo.x) / (hi.x - lo.x) } else { 0.5 };
                Some(lo.y + (hi.y - lo.y) * s).filter(|y| *y < below)
            })
            .fold(None, |m: Option<f32>, y| Some(m.map_or(y, |m| m.max(y))))
    }

    /// Like [`top_at`], the lowest point.
    fn bottom_at(segs: &[(Vec3, Vec3)], x: f32, above: f32) -> Option<f32> {
        segs.iter()
            .filter_map(|(a, b)| {
                let (lo, hi) = if a.x <= b.x { (a, b) } else { (b, a) };
                if x < lo.x || x > hi.x {
                    return None;
                }
                let s = if hi.x - lo.x > 1e-7 { (x - lo.x) / (hi.x - lo.x) } else { 0.5 };
                Some(lo.y + (hi.y - lo.y) * s).filter(|y| *y > above)
            })
            .fold(None, |m: Option<f32>, y| Some(m.map_or(y, |m| m.min(y))))
    }

    /// A first-person gun's locator by the names Metro uses, most specific first.
    fn locator(m: &asset::Model, names: &[String]) -> Option<(String, Vec3)> {
        names.iter().find_map(|n| bone_pos(m, n).map(|p| (n.clone(), p)))
    }

    pub fn run(dir: &Path, key: &str, m: &asset::Model) -> anyhow::Result<()> {
        let index = asset::load_index(dir)?;
        let item = index.items.iter().find(|i| i.key == key);
        let rig = item.and_then(|i| i.rig);
        let suffix = key.trim_start_matches("wpn_");
        println!("== {key}: {}", item.map_or("(not in the index)", |i| i.name.as_str()));
        if let Some(i) = item {
            println!("   hands {:?}, magazine {:?} rounds", i.hands, i.mag_size);
        }
        let mut tris = Vec::new();
        let mut per_bone: Vec<Vec<Vec3>> = vec![Vec::new(); m.bones.len()];
        for mesh in &m.meshes {
            for v in &mesh.vertices {
                if let Some(b) = heaviest(v).filter(|&b| b < per_bone.len()) {
                    per_bone[b].push(Vec3::from_array(v.pos));
                }
            }
            for t in mesh.indices.chunks_exact(3) {
                let v = [0, 1, 2].map(|k| &mesh.vertices[t[k] as usize]);
                let bones = v.map(heaviest);
                let bone = bones.iter().find(|b| bones.iter().filter(|c| c == b).count() >= 2).copied().flatten();
                tris.push(Tri { p: v.map(|v| Vec3::from_array(v.pos)), bone });
            }
        }
        if let Some((lo, hi)) = bounds(per_bone.iter().flatten().copied()) {
            println!("   model {}", range(lo, hi));
        }

        // ---- parts and locators ----
        let parent = |b: usize| usize::try_from(m.bones[b].parent).ok().and_then(|p| m.bones.get(p)).map_or("-", |p| p.name.as_str());
        let mut stems: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for b in 0..m.bones.len() {
            if !per_bone[b].is_empty() {
                stems.entry(stem(&m.bones[b].name)).or_default().push(b);
            }
        }
        println!("-- parts (bones and the skin they carry; numbered siblings folded):");
        for (s, bones) in &stems {
            let (lo, hi) = bounds(bones.iter().flat_map(|&b| per_bone[b].iter().copied())).unwrap();
            let n: usize = bones.iter().map(|&b| per_bone[b].len()).sum();
            let tag = match &m.bones[bones[0]].name {
                n if has_word(n, TRIGGER_WORDS) => "  <- trigger?",
                n if has_word(n, BOLT_WORDS) => "  <- bolt/handle?",
                n if has_word(n, MAG_WORDS) => "  <- magazine/rounds?",
                _ => "",
            };
            let name = if bones.len() > 2 { format!("{s}* x{}", bones.len()) } else { bones.iter().map(|&b| m.bones[b].name.clone()).collect::<Vec<_>>().join(", ") };
            println!("   {name:28} (under {:16}) n {n:5}  {}{tag}", parent(bones[0]), range(lo, hi));
        }
        println!("-- locators (bones without skin):");
        for (b, bone) in m.bones.iter().enumerate() {
            if per_bone[b].is_empty() {
                println!("   {:28} (under {:16}) at {}", bone.name, parent(b), v3(bone_pos(m, &bone.name).unwrap()));
            }
        }

        // ---- the game's aim line, muzzle, well and hands ----
        let (sight, fwd) = rig.map_or((Vec3::ZERO, Vec3::Z), |r| (Vec3::from_array(r.sight), Vec3::from_array(r.forward).normalize_or(Vec3::Z)));
        let names = |l: &[&str]| l.iter().map(|s| s.replace("{}", suffix)).collect::<Vec<String>>();
        let muzzle = locator(m, &names(&["loc_fire_point_{}", "loc_fire_point", "loc_firepoint"]));
        let well = locator(m, &names(&["loc_magazin_m", "loc_magazin_{}", "loc_magazin", "loc_magazine", "loc_magazin_1", "loc_magazin_b"]));
        let muzzle_at = muzzle.as_ref().map(|(_, p)| *p).or(rig.map(|r| Vec3::from_array(r.muzzle))).unwrap_or(sight + Vec3::Z * 0.5);
        let line = |z: f32| sight + fwd * ((z - sight.z) / fwd.z.max(1e-3));
        println!("-- the game's aim line (its aimed camera's axis): through {} along {}", v3(sight), v3(fwd));
        println!("   muzzle {}; magazine well {}", muzzle.as_ref().map_or("-".into(), |(n, p)| format!("{n} {}", v3(*p))), well.as_ref().map_or("-".into(), |(n, p)| format!("{n} {}", v3(*p))));
        let arms = item.and_then(|i| i.hands.as_ref()).and_then(|h| asset::load_model(dir, h).ok());
        let palm = |name: &str| arms.as_ref().and_then(|a| bone_pos(a, name));
        let right = palm("bone_hand_right").or(rig.map(|r| Vec3::from_array(r.grip)));
        let left = palm("bone_hand_left").or(rig.map(|r| Vec3::from_array(r.foregrip)));
        println!("-- the game's palms (its arms for this gun): right {}, left {}", right.map_or("-".into(), v3), left.map_or("-".into(), v3));

        // ---- sights ----
        let grip_z = right.map_or(sight.z - 0.2, |r| r.z);
        let (z0, z1) = ((grip_z - 0.15).min(sight.z - 0.1), muzzle_at.z + 0.01);
        let step = 0.0025;
        let mut rows = Vec::new();
        let mut z = z0;
        while z < z1 {
            let c = line(z);
            let segs: Vec<(Vec3, Vec3)> = section(tris.iter(), 2, z).into_iter().filter(|(a, b)| a.y.max(b.y) > c.y - 0.05 && a.y.min(b.y) < c.y + 0.04).collect();
            let below = c.y + 0.04;
            // On the line: the highest within a millimetre of it (the aim line may run a little
            // off the gun's middle, and a post is thin).
            let centre = (-2..=2).filter_map(|k| top_at(&segs, c.x + k as f32 * 0.0005, below)).fold(None, |m: Option<f32>, y| Some(m.map_or(y, |m| m.max(y))));
            // The shoulders: the highest either side, 1.5 to 8 mm out.
            let side = |s: f32| (0..14).filter_map(|k| top_at(&segs, c.x + s * (0.0015 + k as f32 * 0.0005), below)).fold(None, |m: Option<f32>, y| Some(m.map_or(y, |m| m.max(y))));
            let (l, r) = (side(-1.0), side(1.0));
            let shoulders = match (l, r) {
                (Some(l), Some(r)) => Some(l.min(r)),
                _ => None,
            };
            rows.push((z, centre, shoulders, c.y));
            z += step;
        }
        let near = |y: Option<f32>, line_y: f32| y.is_some_and(|y| y > line_y - 0.045);
        println!("-- sight profile: along the aim line, the top of the gun on it (centre) and of its shoulders 1.5..8 mm either side (the lower of the two), where either changes:");
        let fy = |y: &Option<f32>| y.map_or("   -   ".into(), |y| format!("{:+.4}", y));
        let mut last: Option<(Option<f32>, Option<f32>)> = None;
        let moved = |a: Option<f32>, b: Option<f32>| match (a, b) {
            (Some(a), Some(b)) => (a - b).abs() > 0.0003,
            (None, None) => false,
            _ => true,
        };
        for (z, c, s, ly) in &rows {
            if (near(*c, *ly) || near(*s, *ly)) && last.is_none_or(|(lc, ls)| moved(lc, *c) || moved(ls, *s)) {
                println!("   z {z:.4}  centre {}  shoulders {}  (line {:+.4})", fy(c), fy(s), ly);
                last = Some((*c, *s));
            }
        }
        // Front posts: places in the front half where the top of the gun on the line peaks (the
        // highest within a centimetre, standing 1.5 mm above what's within 2 cm), each with its
        // tip at the highest vertex there.
        let mid = 0.5 * (grip_z + muzzle_at.z);
        let centre_near = |z: f32, d: f32| rows.iter().filter(move |r| (r.0 - z).abs() <= d).filter_map(|r| r.1);
        let posts: Vec<Vec3> = rows
            .iter()
            .filter(|(z, c, _, ly)| *z > mid && c.is_some_and(|c| c > ly - 0.045 && c < ly + 0.035))
            .filter(|(z, c, _, _)| {
                let c = c.unwrap();
                centre_near(*z, 0.01).all(|o| o <= c) && centre_near(*z, 0.02).fold(f32::MAX, f32::min) < c - 0.0015
            })
            .map(|(z, c, _, ly)| {
                let x = line(*z).x;
                let tip = per_bone.iter().flatten().filter(|p| (p.z - z).abs() <= 0.003 && (p.x - x).abs() <= 0.002 && p.y < ly + 0.04).map(|p| p.y).fold(c.unwrap(), f32::max);
                Vec3::new(x, tip, *z)
            })
            .collect();
        // Rear notches: shoulders above the centre.
        let notches: Vec<(Vec3, Option<f32>)> = rows
            .iter()
            .filter(|(_, c, s, ly)| s.is_some_and(|s| s > ly - 0.045 && s - c.unwrap_or(f32::MIN) > 0.0008))
            .map(|(z, c, s, _)| (Vec3::new(line(*z).x, s.unwrap(), *z), *c))
            .collect();
        // The sight picture the game shows: its aimed camera sits on the line through the rear
        // shoulders and the front post's tip, about where the viewmodel's camera is (z = 0). The
        // pair whose line passes nearest it wins.
        let mut pairs: Vec<(f32, Vec3, Option<f32>, Vec3)> = Vec::new();
        for f in &posts {
            for (r, c) in notches.iter().filter(|(r, _)| r.z < f.z - 0.08) {
                let at_camera = r.y - (f.y - r.y) / (f.z - r.z) * r.z;
                pairs.push(((at_camera - line(0.0).y).abs(), *r, *c, *f));
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (miss, r, c, f) in pairs.iter().take(4) {
            let tilt = ((f.y - r.y) / (f.z - r.z)).atan().to_degrees() - fwd.y.atan2(fwd.z).to_degrees();
            println!(
                "   notch at z {:.4} (shoulders {:+.4}, bottom {}) and post at z {:.4} (tip {:+.4}): {:.2}° to the aim line, {:.1} mm from the game's camera",
                r.z,
                r.y,
                fy(c),
                f.z,
                f.y,
                tilt,
                miss * 1000.0
            );
        }
        // (Some guns' aimed camera sits well off their iron sights: then none is near it, and the
        // sights are measured by hand.)
        let best = pairs.first().filter(|p| p.0 < 0.006);
        let (rear, front_tip) = (best.map(|p| (p.1, p.2)), best.map(|p| p.3));
        match (rear, front_tip) {
            (Some((r, notch)), Some(f)) => {
                println!("   rear sight at z {:.4}: shoulders' top y {:.4}, notch bottom {}", r.z, r.y, fy(&notch));
                println!("   front post's tip {}", v3(f));
            }
            _ => println!("   no notch-and-post pair lines up with the game's camera (an optic, a peep or odd sights): use the aim line through the optic, or measure by hand"),
        }

        // ---- the pistol grip ----
        let mut grip_rows = Vec::new();
        if let Some(r) = right {
            println!("-- pistol grip: per 1 cm down from the receiver, its section under the game's right palm (the outline nearest it):");
            let mut y = r.y + 0.035;
            while y > r.y - 0.09 {
                let segs: Vec<(Vec3, Vec3)> = section(tris.iter(), 1, y).into_iter().filter(|(a, _)| (a.x - r.x).abs() < 0.05 && (a.z - r.z).abs() < 0.10).collect();
                let gap = |o: &Vec<(Vec3, Vec3)>| {
                    let (lo, hi) = bounds(o.iter().flat_map(|(a, b)| [*a, *b])).unwrap();
                    let d = Vec3::new(r.x, y, r.z).clamp(lo, hi).distance(Vec3::new(r.x, y, r.z));
                    // Prefer an outline the size of a hand-hold over a sliver.
                    d + if (hi.z - lo.z) < 0.012 { 0.02 } else { 0.0 }
                };
                if let Some(o) = outlines(&segs).iter().min_by(|a, b| gap(a).total_cmp(&gap(b))) {
                    let (lo, hi) = bounds(o.iter().flat_map(|(a, b)| [*a, *b])).unwrap();
                    let (w, d) = (hi.x - lo.x, hi.z - lo.z);
                    println!("   y {y:+.3}  x {:.4}..{:.4} ({:.1} cm)  z {:.4}..{:.4} ({:.1} cm)", lo.x, hi.x, w * 100.0, lo.z, hi.z, d * 100.0);
                    // The axis from the middle of the hand-hold: below the receiver's corner, above
                    // the grip's flared bottom.
                    if (0.012..0.06).contains(&d) && w < 0.045 && y < r.y - 0.008 && y > r.y - 0.066 {
                        grip_rows.push((y, (lo + hi) * 0.5, w, d));
                    }
                }
                y -= 0.01;
            }
        }
        // Its axis through the sections' centres, fitted from the rows the size of a grip.
        let fit = (grip_rows.len() >= 3).then(|| {
            let n = grip_rows.len() as f32;
            let (my, mz) = (grip_rows.iter().map(|r| r.0).sum::<f32>() / n, grip_rows.iter().map(|r| r.1.z).sum::<f32>() / n);
            let sxy: f32 = grip_rows.iter().map(|r| (r.0 - my) * (r.1.z - mz)).sum();
            let sxx: f32 = grip_rows.iter().map(|r| (r.0 - my).powi(2)).sum();
            let b = sxy / sxx.max(1e-9);
            let rake = b.atan().to_degrees();
            let x = grip_rows.iter().map(|r| r.1.x).sum::<f32>() / n;
            let (ylo, yhi) = (grip_rows.iter().map(|r| r.0).fold(f32::MAX, f32::min) - 0.005, grip_rows.iter().map(|r| r.0).fold(f32::MIN, f32::max) + 0.005);
            let yc = 0.5 * (ylo + yhi);
            let c = rake.to_radians().cos();
            let half = Vec3::new(grip_rows.iter().map(|r| r.2).sum::<f32>() / n * 0.5, grip_rows.iter().map(|r| r.3).sum::<f32>() / n * 0.5 * c, 0.5 * (yhi - ylo) / c);
            let axis_at = move |y: f32| Vec3::new(x, y, mz + b * (y - my));
            (rake, axis_at(yc), half, axis_at)
        });
        match &fit {
            Some((rake, centre, half, _)) => println!("   its axis is raked {rake:.1}° (the bottom further back); as a block: centre {} half {}", v3(*centre), v3(*half)),
            None => println!("   no pistol grip found under the palm (a stock grip, a revolver's frame?): measure by hand"),
        }

        // ---- the trigger ----
        let trigger_bone = m.bones.iter().enumerate().filter(|(b, bone)| !per_bone[*b].is_empty() && has_word(&bone.name, TRIGGER_WORDS)).min_by_key(|(_, bone)| bone.name.len());
        let pad = trigger_bone.and_then(|(b, bone)| {
            let (lo, hi) = bounds(per_bone[b].iter().copied())?;
            let y = lo.y + 0.4 * (hi.y - lo.y);
            let segs = section(tris.iter().filter(|t| t.bone == Some(b)), 1, y);
            let face = segs.iter().flat_map(|(a, c)| [a.z, c.z]).fold(f32::MIN, f32::max);
            let face = if face > f32::MIN { face } else { hi.z };
            let pad = Vec3::new(0.5 * (lo.x + hi.x), y, face - 0.002);
            println!("-- trigger: bone `{}` {}; its pad 40% up the blade, 2 mm behind its face: {}", bone.name, range(lo, hi), v3(pad));
            Some((bone.name.clone(), pad))
        });

        // ---- the handguard ----
        if let Some(l) = left {
            let c = line(l.z);
            let segs: Vec<(Vec3, Vec3)> = section(tris.iter(), 2, l.z).into_iter().filter(|(a, b)| (a.x - c.x).abs() < 0.07 && a.y.min(b.y) < c.y + 0.02 && a.y.max(b.y) > l.y - 0.04).collect();
            if let Some((lo, hi)) = bounds(segs.iter().flat_map(|(a, b)| [*a, *b])) {
                let bottom = bottom_at(&segs, c.x, l.y - 0.04);
                println!(
                    "-- handguard at the game's left palm (z {:.4}): {}; its bottom under the line of sight y {}; the palm {} under it and {:.1} cm to its {}",
                    l.z,
                    range(lo, hi),
                    bottom.map_or("-".into(), |b| format!("{b:.4}")),
                    bottom.map_or("-".into(), |b| format!("{:.1} cm", (b - l.y) * 100.0)),
                    (l.x - c.x).abs() * 100.0,
                    if l.x < c.x { "left" } else { "right" }
                );
            }
        }

        // ---- a rig entry to start from ----
        println!("-- a start for content/rigs/<game>.toml (check every point against the renders):");
        println!("[[gun]]\nkey = \"{key}\"");
        if let (Some((r, _)), Some(f)) = (rear, front_tip) {
            println!("rear_sight = {}\nfront_sight = {}", v3(r), v3(f));
        } else {
            println!("rear_sight = {}  # on the aim line\nfront_sight = {}", v3(line(sight.z)), v3(line(sight.z + 0.25)));
        }
        if let (Some((rake, centre, half, axis_at)), Some(r)) = (&fit, right) {
            println!("grip = {}  # the grip's axis level with the game's palm\ngrip_rake_deg = {:.1}", v3(axis_at(r.y)), rake);
            println!("grip_block = {{ centre = {}, half = {} }}", v3(*centre), v3(*half));
        } else if let Some(r) = right {
            println!("grip = {}  # the game's palm\ngrip_rake_deg = 0.0", v3(r));
        }
        if let Some((name, pad)) = &pad {
            println!("trigger = {}\ntrigger_bone = \"{name}\"", v3(*pad));
        }
        if let Some(l) = left {
            println!("foregrip = {}  # the game's left palm", v3(l));
        }
        if let Some((_, w)) = &well {
            println!("mag_well = {}", v3(*w));
        }
        println!("muzzle = {}", v3(muzzle_at));
        Ok(())
    }
}
