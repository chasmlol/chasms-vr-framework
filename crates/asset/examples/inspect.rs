//! Prints what's in a cached model: meshes, bones and bounds.
//!
//! `cargo run -p rustvr-asset --example inspect -- <game> <model key> [--bones]`

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(game), Some(key)) = (args.first(), args.get(1)) else {
        anyhow::bail!("usage: inspect <game> <model key> [--bones]");
    };
    let dir = rustvr_asset::default_root().join(game);
    let m = rustvr_asset::load_model(&dir, key)?;
    println!("{key}: {} meshes, {} bones, bounds {:?} .. {:?}", m.meshes.len(), m.bones.len(), m.bounds_min, m.bounds_max);
    for (i, mesh) in m.meshes.iter().enumerate() {
        let skinned = mesh.vertices.iter().filter(|v| v.weights.iter().any(|&w| w > 0)).count();
        println!("  mesh {i} '{}': {} verts ({} skinned), {} tris, texture '{}'", mesh.name, mesh.vertices.len(), skinned, mesh.indices.len() / 3, mesh.texture);
    }
    if args.iter().any(|a| a == "--bones") {
        for (i, b) in m.bones.iter().enumerate() {
            let p = &b.model_matrix[12..15];
            println!("  bone {i:3} '{}' parent {:3}  at ({:.3}, {:.3}, {:.3})", b.name, b.parent, p[0], p[1], p[2]);
        }
    }
    Ok(())
}
