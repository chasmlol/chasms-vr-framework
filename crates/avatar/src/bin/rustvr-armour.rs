//! `rustvr-armour`: the armour preview. Puts a body together from a game's extracted models, sweeps
//! it through the poses armour has to survive (arms raised and crossed, elbows bent as far as they
//! go, wrists turned and bent both ways, reaching, a squat) and draws each with the runtime's own
//! renderer from the front, the side, the back, the wearer's own eyes and close on the right arm,
//! the right elbow side on and the right wrist from the thumb's side, skinned three ways (the
//! game's linear blending, with joints that keep their volume, and with the game's armour marks),
//! for marking pieces and checking them (`docs/ARMOUR.md`).
//!
//! ```text
//! rustvr-armour <game> (--set <name> | <model>...) [--out <folder>] [--poses <a,b,..>]
//!               [--views <a,b,..>] [--skin linear,centres,marked] [--size <pixels>] [--plain]
//!               [--visibility hands|arms|full] [--fingers relaxed|open|fist|point|trigger] [--list]
//! ```
//!
//! Models are named as the game names them (`armor\iron\m\gauntlets.nif`) or by their keys in the
//! asset index; a set is one of the marks file's `[[set]]`s, worn over the preview's bare body
//! where it doesn't cover it. Writes, into `--out` (default `captures/armour/<set>`):
//!
//! * `<pose>.png`: the pose, a row per way of skinning (as `--skin` lists them, top first), a
//!   column per view (front, right side, back, the wearer's eyes, the right forearm, the right
//!   shoulder, the right elbow side on, the right wrist from the thumb's side, the back of the right
//!   hand; `--views` picks);
//! * `islands.png`: the body at rest, every island its own colour, numbered `mesh.island` as the
//!   listing numbers them (each part's meshes counted from 0);
//! * `plates.png`: the same, each plate its own colour, soft parts grey, at rest and arms raised
//!   (or in the poses `--poses` names, from the views `--views` names);
//! * `uv-<model>.png`: each marked model's textures, its triangles drawn where they take their
//!   texture from, a plate's filled in its colour (for `uv_within`);
//! * `list.txt` (with `--list`, printed instead of drawing anything): each model's meshes and
//!   islands (their vertices, bones and bounds), and the plates its marks give it (what each
//!   rides, its pivot at rest), with what in the marks didn't work.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::{Mat3, Quat, Vec3};
use rustvr_asset as asset;
use rustvr_asset::armour::ArmourFile;
use rustvr_avatar::{Avatar, AvatarFrame, AvatarSource, Skinning, armour, pose};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

const HEIGHT: f32 = 1.78;

/// The poses: name and joints.
fn poses() -> Vec<(&'static str, [Pose; JOINT_COUNT])> {
    vec![
        ("rest", pose::arms_down(HEIGHT)),
        ("raised", pose::arms_raised(HEIGHT)),
        ("crossed", pose::arms_crossed(HEIGHT)),
        ("elbows", pose::elbows_bent(HEIGHT)),
        ("palms_up", pose::forearms_out(HEIGHT, 90.0, 0.0)),
        ("palms_down", pose::forearms_out(HEIGHT, -90.0, 0.0)),
        ("wrists_in", pose::forearms_out(HEIGHT, 0.0, 70.0)),
        ("wrists_back", pose::forearms_out(HEIGHT, 0.0, -65.0)),
        ("reach", pose::reaching_forward(HEIGHT, [0.53, 0.53])),
        ("gun", pose::holding_gun(HEIGHT)),
        ("squat", pose::squat(HEIGHT)),
    ]
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Front,
    Side,
    Back,
    Eye,
    Arm,
    Shoulder,
    Elbow,
    Wrist,
    Hand,
}

const VIEWS: [(View, &str); 9] = [
    (View::Front, "front"),
    (View::Side, "side"),
    (View::Back, "back"),
    (View::Eye, "eye"),
    (View::Arm, "arm"),
    (View::Shoulder, "shoulder"),
    (View::Elbow, "elbow"),
    (View::Wrist, "wrist"),
    (View::Hand, "hand"),
];

struct Args {
    game: String,
    set: Option<String>,
    models: Vec<String>,
    out: Option<PathBuf>,
    poses: Option<Vec<String>>,
    views: Option<Vec<String>>,
    skins: Vec<String>,
    size: u32,
    plain: bool,
    /// `BODYVIS_*`: what of the body the pose pictures show.
    visibility: u32,
    /// How the hands are held (relaxed, unless asked).
    fingers: Fingers,
    list: bool,
}

fn args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let mut a = Args { game: String::new(), set: None, models: Vec::new(), out: None, poses: None, views: None, skins: vec!["linear".into(), "centres".into(), "marked".into()], size: 400, plain: false, visibility: BODYVIS_FULL, fingers: pose::relaxed_hand(), list: false };
    while let Some(x) = it.next() {
        let mut value = || it.next().with_context(|| format!("{x} needs a value"));
        match x.as_str() {
            "--set" => a.set = Some(value()?),
            "--out" => a.out = Some(PathBuf::from(value()?)),
            "--poses" => a.poses = Some(value()?.split(',').map(str::to_owned).collect()),
            "--views" => a.views = Some(value()?.split(',').map(str::to_owned).collect()),
            "--skin" => a.skins = value()?.split(',').map(str::to_owned).collect(),
            "--size" => a.size = value()?.parse()?,
            "--plain" => a.plain = true,
            "--visibility" => {
                a.visibility = match value()?.as_str() {
                    "hands" => BODYVIS_HANDS,
                    "arms" => BODYVIS_ARMS,
                    "full" => BODYVIS_FULL,
                    other => bail!("--visibility {other}: hands, arms or full"),
                }
            }
            "--fingers" => {
                a.fingers = match value()?.as_str() {
                    "relaxed" => pose::relaxed_hand(),
                    "open" => pose::open_hand(),
                    "fist" => pose::fist(),
                    "point" => pose::pointing(),
                    "trigger" => pose::trigger_grip(),
                    other => bail!("--fingers {other}: relaxed, open, fist, point or trigger"),
                }
            }
            "--list" => a.list = true,
            _ if x.starts_with("--") => bail!("unknown option {x}"),
            _ if a.game.is_empty() => a.game = x,
            _ => a.models.push(x),
        }
    }
    if a.game.is_empty() || (a.set.is_none() && a.models.is_empty()) {
        bail!("usage: rustvr-armour <game> (--set <name> | <model>...) [--out <folder>] [--poses a,b] [--skin linear,centres,marked] [--size n] [--plain] [--visibility hands|arms|full] [--fingers relaxed|open|fist|point|trigger] [--list]");
    }
    Ok(a)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_env("RUSTVR_LOG").unwrap_or_else(|_| "warn".into())).with_writer(std::io::stderr).init();
    let a = args()?;
    let dir = asset::default_root().join(&a.game);
    let marks = asset::armour::load(&a.game, &dir)?;
    let index = asset::load_index(&dir).with_context(|| format!("the asset index in {}", dir.display()))?;
    let key_of: HashMap<String, String> = index.items.iter().map(|i| (asset::armour::normalise(&i.name), i.key.clone())).collect();
    let name_of: HashMap<String, String> = index.items.iter().map(|i| (i.key.clone(), i.name.clone())).collect();
    let key = |m: &str| -> Result<String> {
        if name_of.contains_key(m) {
            return Ok(m.to_owned());
        }
        key_of.get(&asset::armour::normalise(m)).cloned().with_context(|| format!("{m} isn't in {}'s asset index (extract it first)", a.game))
    };
    // What's worn: the set's models, over the bare body's slots they leave uncovered (and under
    // the ones they cover, as the game's body is: see `rustvr_protocol::UNDER`).
    let mut models: Vec<String> = a.models.clone();
    if let Some(name) = &a.set {
        let set = marks.set(name).with_context(|| format!("no set '{name}' in the marks"))?;
        models.extend(set.models.iter().cloned());
    }
    let covered: Vec<String> = models.iter().filter_map(|m| marks.piece(m)).flat_map(|p| p.covers.iter().cloned()).collect();
    let mut under = Vec::new();
    for skin in &marks.preview.skin {
        if !covered.iter().any(|c| c.eq_ignore_ascii_case(&skin.slot)) {
            models.push(skin.model.clone());
        } else {
            under.push(skin.model.clone());
        }
    }
    anyhow::ensure!(!marks.preview.skeleton.is_empty(), "the marks name no preview skeleton ([preview] skeleton = ...)");
    let skeleton = key(&marks.preview.skeleton)?;
    // (The listing and the pictures of islands, plates and textures are of what's worn.)
    let worn: Vec<String> = models.iter().map(|m| key(m)).collect::<Result<_>>()?;
    let mut parts = worn.clone();
    for m in &under {
        parts.push(format!("{}{}", key(m)?, rustvr_protocol::UNDER));
    }
    let label = a.set.clone().unwrap_or_else(|| Path::new(&asset::armour::normalise(&models[0]).replace('\\', "/")).file_stem().map_or("body".into(), |s| s.to_string_lossy().into_owned()));
    let out = a.out.clone().unwrap_or_else(|| asset::repo_root().join("captures").join("armour").join(&label));
    std::fs::create_dir_all(&out)?;

    let listing = list(&dir, &skeleton, &worn, &name_of, &marks)?;
    std::fs::write(out.join("list.txt"), &listing)?;
    if a.list {
        print!("{listing}");
        return Ok(());
    }

    let gpu = Gpu::new()?;
    let size = (a.size, a.size);
    let wanted = |name: &str| a.poses.as_ref().is_none_or(|p| p.iter().any(|x| x == name));
    let views: Vec<View> = VIEWS.iter().filter(|(_, n)| a.views.as_ref().is_none_or(|v| v.iter().any(|x| x == n))).map(|(v, _)| *v).collect();
    let poses: Vec<_> = poses().into_iter().filter(|(n, _)| wanted(n)).collect();
    let mut sheets: Vec<Vec<Vec<u8>>> = vec![Vec::new(); poses.len()];
    for skin in &a.skins {
        let skinning = match skin.as_str() {
            "linear" => Skinning::Linear,
            "centres" => Skinning::Centres,
            "marked" => Skinning::Marked(&marks),
            other => bail!("--skin {other}: linear, centres or marked"),
        };
        let mut avatar = Avatar::new(&gpu, rustvr_avatar::compose(&dir, &skeleton, &parts, &[], None, skinning)?)?;
        for (p, (_, joints)) in poses.iter().enumerate() {
            let row: Vec<Vec<u8>> = views.iter().map(|&v| render(&gpu, &mut avatar, size, joints, a.fingers, v, a.plain, a.visibility)).collect::<Result<_>>()?;
            sheets[p].push(tile_row(&row, size));
        }
    }
    for ((name, _), rows) in poses.iter().zip(&sheets) {
        let (w, h) = (size.0 * views.len() as u32, size.1 * rows.len() as u32);
        rustvr_gpu::save_png(&out.join(format!("{name}.png")), w, h, &rows.concat())?;
    }

    // The islands, and the plates, coloured (the plates in the poses asked for, to see what bends
    // where).
    let plates_posed = if a.poses.is_some() { poses.iter().map(|p| p.1).collect() } else { vec![pose::arms_down(HEIGHT), pose::arms_raised(HEIGHT)] };
    let coloured_views: Vec<View> = if a.views.is_some() { views.clone() } else { vec![View::Front, View::Side, View::Back, View::Arm] };
    for (file, skinning, colours, posed) in [
        ("islands.png", Skinning::Linear, Colours::Islands, vec![pose::arms_down(HEIGHT)]),
        ("plates.png", Skinning::Marked(&marks), Colours::Plates, plates_posed),
    ] {
        let (source, labels) = coloured(rustvr_avatar::compose(&dir, &skeleton, &parts, &[], None, skinning)?, &dir, &worn, &marks, &name_of, colours)?;
        let (AvatarSource::Built { model, .. } | AvatarSource::Composed { model, .. }) = &source else { unreachable!() };
        // Where each mesh's vertices start in the posed skin.
        let starts: Vec<usize> = model.meshes.iter().scan(0, |n, m| Some(std::mem::replace(n, *n + m.vertices.len()))).collect();
        let mut avatar = Avatar::new(&gpu, source)?;
        let mut rows = Vec::new();
        for joints in &posed {
            // Each label at the middle of its island as posed.
            let skin = avatar.skin(&frame(joints, a.fingers, false, false, BODYVIS_FULL));
            let placed: Vec<(&str, Vec3)> = labels
                .iter()
                .map(|(text, mesh, vertices)| (text.as_str(), vertices.iter().map(|&v| skin[starts[*mesh] + v as usize].0).sum::<Vec3>() / vertices.len().max(1) as f32))
                .collect();
            let row: Vec<Vec<u8>> = coloured_views
                .iter()
                .map(|&v| {
                    let mut px = render(&gpu, &mut avatar, size, joints, a.fingers, v, false, BODYVIS_FULL)?;
                    let (eye, fov) = camera(joints, v, size);
                    for (text, at) in &placed {
                        if let Some(p) = project(&eye, &fov, size, *at) {
                            draw_text(&mut px, size, p, text);
                        }
                    }
                    Ok(px)
                })
                .collect::<Result<_>>()?;
            rows.push(tile_row(&row, size));
        }
        rustvr_gpu::save_png(&out.join(file), size.0 * coloured_views.len() as u32, size.1 * rows.len() as u32, &rows.concat())?;
    }
    uv_sheets(&dir, &skeleton, &worn, &name_of, &marks, &out)?;
    println!("{} poses x {} ways of skinning, islands, plates and their textures: {}", poses.len(), a.skins.len(), out.display());
    Ok(())
}

/// `uv-<model>.png` for each marked model: its textures, with the right side's (and the
/// middle's) triangles drawn where they take their texture from, a plate's filled in its colour
/// as the listing names it, the soft ones outlined. A piece whose metal and leather are one island
/// is split by the texture (`uv_within`): its boxes are read off these.
fn uv_sheets(dir: &Path, skeleton: &str, parts: &[String], name_of: &HashMap<String, String>, marks: &ArmourFile, out: &Path) -> Result<()> {
    let skel = asset::load_model(dir, skeleton)?;
    // (Plates numbered across the parts, as the listing numbers them for their colours.)
    let mut plate_count = 0;
    for key in parts {
        let name = name_of.get(key).map_or(key.as_str(), String::as_str);
        let mut model = asset::load_model(dir, key)?;
        if model.bones.len() != skel.bones.len() || model.bones.iter().zip(&skel.bones).any(|(a, b)| a.name != b.name) {
            rustvr_avatar::rebind(&mut model, &skel.bones)?;
        }
        let Some(piece) = marks.piece(name) else { continue };
        let plates = armour::plates(piece, &model, &[]).plates;
        // Each triangle's plate: all its corners on it (a plate's own triangles, if it has them).
        let mut plate_of: HashMap<(usize, usize), usize> = HashMap::new();
        for (k, p) in plates.iter().enumerate() {
            let mesh = &model.meshes[p.mesh];
            let mine: std::collections::HashSet<u32> = p.vertices.iter().copied().collect();
            for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
                if (p.triangles.is_empty() && tri.iter().all(|i| mine.contains(i))) || p.triangles.contains(&(t as u32)) {
                    plate_of.insert((p.mesh, t), plate_count + k);
                }
            }
        }
        plate_count += plates.len();
        let mut textures: Vec<&str> = Vec::new();
        for m in &model.meshes {
            if !m.texture.is_empty() && !textures.contains(&m.texture.as_str()) {
                textures.push(&m.texture);
            }
        }
        let mut sheets: Vec<(u32, u32, Vec<u8>)> = Vec::new();
        for tex in textures {
            let (w, h, mut px) = match asset::load_texture(dir, tex).ok().and_then(|t| decoded(&t).map(|px| (t.width, t.height, px))) {
                Some(t) => t,
                None => (512, 512, [60, 60, 60, 255].repeat(512 * 512)),
            };
            for (m, mesh) in model.meshes.iter().enumerate().filter(|(_, m)| m.texture == tex) {
                for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
                    let corners = [0, 1, 2].map(|k| &mesh.vertices[tri[k] as usize]);
                    if corners.iter().all(|v| v.pos[0] < -0.02) {
                        continue;
                    }
                    let at = corners.map(|v| (v.uv[0].rem_euclid(1.0) * w as f32, v.uv[1].rem_euclid(1.0) * h as f32));
                    match plate_of.get(&(m, t)) {
                        Some(&k) => {
                            let c = PALETTE[k % PALETTE.len()].0.map(|x| (x * 255.0) as u8);
                            fill_triangle(&mut px, (w, h), at, c);
                            outline_triangle(&mut px, (w, h), at, c);
                        }
                        None => outline_triangle(&mut px, (w, h), at, [235, 235, 235]),
                    }
                }
            }
            sheets.push((w, h, px));
        }
        // Side by side, the shorter ones padded below.
        let (w, h) = (sheets.iter().map(|s| s.0).sum::<u32>(), sheets.iter().map(|s| s.1).max().unwrap_or(1));
        let mut px = [0u8, 0, 0, 255].repeat((w * h) as usize);
        let mut x0 = 0;
        for (sw, sh, s) in &sheets {
            for y in 0..*sh {
                let row = (*sw * 4) as usize;
                let to = ((y * w + x0) * 4) as usize;
                px[to..to + row].copy_from_slice(&s[y as usize * row..(y as usize + 1) * row]);
            }
            x0 += sw;
        }
        let stem = Path::new(&asset::armour::normalise(name).replace('\\', "/")).file_stem().map_or("model".into(), |s| s.to_string_lossy().into_owned());
        rustvr_gpu::save_png(&out.join(format!("uv-{stem}.png")), w, h, &px)?;
    }
    Ok(())
}

/// A texture's largest level as 8-bit RGBA, for the formats the games' diffuse maps use (none for
/// the others).
fn decoded(t: &asset::Texture) -> Option<Vec<u8>> {
    let (w, h) = (t.width as usize, t.height as usize);
    let data = t.mips.first()?;
    let block = match t.format {
        asset::TexFormat::Rgba8 => return Some(data.clone()),
        asset::TexFormat::Bc1 => 8,
        asset::TexFormat::Bc2 | asset::TexFormat::Bc3 => 16,
        _ => return None,
    };
    let rgb = |c: u16| [((c >> 11) & 31) as u32 * 255 / 31, ((c >> 5) & 63) as u32 * 255 / 63, (c & 31) as u32 * 255 / 31];
    let mut out = vec![255u8; w * h * 4];
    let across = w.div_ceil(4);
    for by in 0..h.div_ceil(4) {
        for bx in 0..across {
            let b = data.get((by * across + bx) * block..)?.get(..block)?;
            // (The colour half of a block: BC2's and BC3's after their alpha, always four colours.)
            let c = &b[block - 8..];
            let (c0, c1) = (u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]));
            let (p0, p1) = (rgb(c0), rgb(c1));
            let four = block == 16 || c0 > c1;
            let mix = |a: u32, b: u32, k: u32| if four { (a * (3 - k) + b * k) / 3 } else { (a + b) / 2 };
            let palette = [p0, p1, [0, 1, 2].map(|i| mix(p0[i], p1[i], 1)), if four { [0, 1, 2].map(|i| mix(p0[i], p1[i], 2)) } else { [0, 0, 0] }];
            let bits = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
            for i in 0..16 {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    let p = palette[((bits >> (2 * i)) & 3) as usize];
                    out[(y * w + x) * 4..][..3].copy_from_slice(&p.map(|v| v as u8));
                }
            }
        }
    }
    Some(out)
}

/// A triangle filled half over what's there.
fn fill_triangle(px: &mut [u8], (w, h): (u32, u32), p: [(f32, f32); 3], colour: [u8; 3]) {
    let edge = |a: (f32, f32), b: (f32, f32), x: f32, y: f32| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
    let area = edge(p[0], p[1], p[2].0, p[2].1);
    if area.abs() < 1e-6 {
        return;
    }
    let (x0, x1) = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min).max(0.0) as u32, p.iter().map(|q| q.0).fold(f32::MIN, f32::max).min(w as f32 - 1.0) as u32);
    let (y0, y1) = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min).max(0.0) as u32, p.iter().map(|q| q.1).fold(f32::MIN, f32::max).min(h as f32 - 1.0) as u32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = [edge(p[0], p[1], fx, fy), edge(p[1], p[2], fx, fy), edge(p[2], p[0], fx, fy)].iter().all(|&e| e * area >= 0.0);
            if inside {
                let o = ((y * w + x) * 4) as usize;
                for k in 0..3 {
                    px[o + k] = ((px[o + k] as u32 + colour[k] as u32) / 2) as u8;
                }
            }
        }
    }
}

/// A triangle's edges, a pixel wide.
fn outline_triangle(px: &mut [u8], (w, h): (u32, u32), p: [(f32, f32); 3], colour: [u8; 3]) {
    for k in 0..3 {
        let (a, b) = (p[k], p[(k + 1) % 3]);
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().max(1.0) as u32;
        for s in 0..=steps {
            let f = s as f32 / steps as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            if x >= 0.0 && y >= 0.0 && (x as u32) < w && (y as u32) < h {
                let o = ((y as u32 * w + x as u32) * 4) as usize;
                px[o..o + 3].copy_from_slice(&colour);
            }
        }
    }
}

/// The listing: every part's meshes, islands and plates.
fn list(dir: &Path, skeleton: &str, parts: &[String], name_of: &HashMap<String, String>, marks: &ArmourFile) -> Result<String> {
    let skel = asset::load_model(dir, skeleton)?;
    let mut s = String::new();
    // Colours as the pictures give them: every island (plate) of the body a colour in turn.
    let (mut island_count, mut plate_count) = (0, 0);
    for key in parts {
        let name = name_of.get(key).map_or(key.as_str(), String::as_str);
        let mut model = asset::load_model(dir, key)?;
        if model.bones.len() != skel.bones.len() || model.bones.iter().zip(&skel.bones).any(|(a, b)| a.name != b.name) {
            rustvr_avatar::rebind(&mut model, &skel.bones)?;
        }
        let piece = marks.piece(name);
        writeln!(s, "== {name} ({key}){}", piece.map_or(" (not marked)".to_owned(), |p| format!(": {}", p.name)))?;
        for (m, mesh) in model.meshes.iter().enumerate() {
            let islands = armour::islands(mesh);
            writeln!(s, "  mesh {m} '{}': {} vertices, {} triangles, {} islands", mesh.name, mesh.vertices.len(), mesh.indices.len() / 3, islands.len())?;
            let first = island_count;
            island_count += islands.len();
            let island_count = first;
            for (k, isl) in islands.iter().enumerate() {
                let bones: Vec<String> = isl.bones.iter().take(4).map(|&(b, w)| format!("{} {:.0}%", model.bones[b].name, w * 100.0)).collect();
                writeln!(
                    s,
                    "    island {m}.{k} ({}, {}): {} vertices, {} triangles, middle {:.3?}, {:.3?} .. {:.3?}: {}",
                    colour_name(island_count + k),
                    isl.side.name(),
                    isl.vertices.len(),
                    isl.triangles,
                    isl.centre.to_array(),
                    isl.min.to_array(),
                    isl.max.to_array(),
                    bones.join(", ")
                )?;
            }
        }
        if let Some(piece) = piece {
            let marked = armour::plates(piece, &model, &rustvr_avatar::rest_pose(&model.bones));
            for p in &marked.plates {
                writeln!(s, "  plate '{}' ({}, {}): mesh {}, {} vertices, rides {}, turns about {:.3?}, layer {}", p.name, p.side.name(), colour_name(plate_count), p.mesh, p.vertices.len(), armour::riders(p, &model.bones), p.pivot.to_array(), p.layer)?;
                plate_count += 1;
            }
            for p in &marked.problems {
                writeln!(s, "  PROBLEM: {p}")?;
            }
            if !marked.unchosen.is_empty() {
                writeln!(s, "  (nothing here for: {})", marked.unchosen.join(", "))?;
            }
        }
    }
    Ok(s)
}

#[derive(Clone, Copy, PartialEq)]
enum Colours {
    Islands,
    Plates,
}

/// Distinct colours (linear) and their names, for the listing.
const PALETTE: [([f32; 3], &str); 12] = [
    ([0.80, 0.10, 0.08], "red"),
    ([0.05, 0.45, 0.85], "blue"),
    ([0.10, 0.65, 0.12], "green"),
    ([0.95, 0.60, 0.02], "orange"),
    ([0.55, 0.12, 0.70], "purple"),
    ([0.05, 0.70, 0.70], "cyan"),
    ([0.90, 0.85, 0.10], "yellow"),
    ([0.85, 0.25, 0.55], "pink"),
    ([0.40, 0.25, 0.08], "brown"),
    ([0.45, 0.80, 0.30], "lime"),
    ([0.15, 0.15, 0.45], "navy"),
    ([0.95, 0.95, 0.95], "white"),
];

fn colour_name(k: usize) -> &'static str {
    PALETTE[k % PALETTE.len()].1
}

/// An island's label: its text, and the mesh and vertices it goes in the middle of.
type Label = (String, usize, Vec<u32>);

/// A body with every island (or plate) its own colour, and (islands) their labels.
fn coloured(source: AvatarSource, dir: &Path, parts: &[String], marks: &ArmourFile, name_of: &HashMap<String, String>, colours: Colours) -> Result<(AvatarSource, Vec<Label>)> {
    let (AvatarSource::Built { model, skinning, .. } | AvatarSource::Composed { model, skinning, .. }) = source else { bail!("not a built body") };
    let skinning = skinning.unwrap_or_default();
    let centres = &skinning.centres;
    // Each composed mesh's part and its index in the part, as `compose` lays them out.
    let mut owner = Vec::new();
    for key in parts {
        let n = asset::load_model(dir, key).map_or(0, |m| m.meshes.len());
        owner.extend((0..n).map(|m| (key.clone(), m)));
    }
    let mut textures = HashMap::new();
    for (k, (c, _)) in PALETTE.iter().enumerate() {
        textures.insert(format!("colour{k}"), solid(*c));
    }
    textures.insert("grey".to_owned(), solid([0.35, 0.35, 0.36]));
    let mut meshes = Vec::new();
    let mut out_centres = Vec::new();
    let mut labels = Vec::new();
    // The plates, mesh by mesh: (composed mesh, its vertices) -> plate number.
    let mut plate_of: HashMap<(usize, u32), usize> = HashMap::new();
    if colours == Colours::Plates {
        let skel = model.bones.clone();
        let mut count = 0;
        let mut first = 0;
        for key in parts {
            let Ok(mut part) = asset::load_model(dir, key) else { continue };
            let n = part.meshes.len();
            if part.bones.len() != skel.len() || part.bones.iter().zip(&skel).any(|(a, b)| a.name != b.name) {
                let _ = rustvr_avatar::rebind(&mut part, &skel);
            }
            if let Some(piece) = name_of.get(key).and_then(|n| marks.piece(n)) {
                // (Cut as `compose` cuts them, so the copies it adds are coloured too.)
                let mut plates = armour::plates(piece, &part, &[]).plates;
                armour::cut_seams(&mut part, &mut plates);
                for p in plates {
                    for &v in &p.vertices {
                        plate_of.insert((first + p.mesh, v), count);
                    }
                    count += 1;
                }
            }
            first += n;
        }
    }
    let mut island_count = 0;
    for (m, mesh) in model.meshes.iter().enumerate() {
        // Each triangle's group: its island, or its plate (by its first corner), or none.
        let islands = armour::islands(mesh);
        let mut island_of = vec![0usize; mesh.vertices.len()];
        let first = meshes.len();
        for (k, isl) in islands.iter().enumerate() {
            for &v in &isl.vertices {
                island_of[v as usize] = island_count + k;
            }
            if colours == Colours::Islands {
                let part_mesh = owner.get(m).map_or(m, |o| o.1);
                // (The mesh it goes in is put right below.)
                labels.push((format!("{part_mesh}.{k}"), first, isl.vertices.clone()));
            }
        }
        island_count += islands.len();
        let mut groups: HashMap<Option<usize>, Vec<u32>> = HashMap::new();
        for t in mesh.indices.chunks_exact(3) {
            let g = match colours {
                Colours::Islands => Some(island_of[t[0] as usize]),
                Colours::Plates => plate_of.get(&(m, t[0])).copied(),
            };
            groups.entry(g).or_default().extend_from_slice(t);
        }
        for (g, indices) in groups {
            let texture = g.map_or("grey".to_owned(), |g| format!("colour{}", g % PALETTE.len()));
            meshes.push(asset::Mesh { texture, vertices: mesh.vertices.clone(), indices, name: mesh.name.clone(), material: asset::Material::default() });
            out_centres.push(centres.get(m).cloned().unwrap_or_default());
        }
    }
    let model = asset::Model { meshes, ..model };
    Ok((AvatarSource::Built { model, textures, skinning: Some(rustvr_avatar::Skin { centres: out_centres, copies: skinning.copies }) }, labels))
}

fn solid(c: [f32; 3]) -> asset::Texture {
    let srgb = |x: f32| (if x <= 0.0031308 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 } * 255.0).round() as u8;
    asset::Texture { version: asset::FORMAT_VERSION, source: "colour".into(), width: 1, height: 1, format: asset::TexFormat::Rgba8, mips: vec![vec![srgb(c[0]), srgb(c[1]), srgb(c[2]), 255]], faces: 1 }
}

/// Where a view looks from, and its field of view.
fn camera(joints: &[Pose; JOINT_COUNT], view: View, size: (u32, u32)) -> (Pose, Fov) {
    let at = |j: Joint| Vec3::from_array(joints[j as usize].pos);
    let aspect = size.0 as f32 / size.1 as f32;
    let fov = |tan: f32| Fov { left: -tan * aspect, right: tan * aspect, up: tan, down: -tan };
    // The whole body: the joints' bounds, with room for the skin round them and the head.
    let (lo, hi) = Joint::ALL.into_iter().map(at).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(p), hi.max(p)));
    let (lo, hi) = (lo - Vec3::new(0.1, 0.08, 0.1), hi + Vec3::new(0.1, 0.25, 0.1));
    let middle = (lo + hi) * 0.5;
    let reach = ((hi.y - lo.y) * 0.5).max((hi.x - lo.x).max(hi.z - lo.z) * 0.5 / aspect);
    let whole = |from: Vec3| {
        let d = from.normalize() * 2.4;
        (look_at(middle + d, middle), fov(reach / 2.4 * 1.05))
    };
    match view {
        View::Front => whole(Vec3::new(0.25, 0.12, 1.0)),
        View::Side => whole(Vec3::new(1.0, 0.12, 0.12)),
        View::Back => whole(Vec3::new(-0.25, 0.15, -1.0)),
        View::Eye => {
            let eye = pose::eye(&joints[Joint::Head as usize], HEIGHT);
            let from = Vec3::from_array(eye.pos);
            // Towards the hands, or down at the body when they're out of sight above.
            let hands = (at(Joint::LeftHand) + at(Joint::RightHand)) * 0.5;
            let ahead = Quat::from_array(eye.rot) * Vec3::Z;
            let in_front = (hands - from).dot(Vec3::new(ahead.x, 0.0, ahead.z).normalize_or(Vec3::Z)) > 0.2;
            let to = if in_front { hands } else { from + Vec3::new(0.0, -0.75, 0.65) };
            (look_at(from, to), Fov { left: -0.95, right: 0.95, up: 0.9, down: -1.0 })
        }
        View::Arm => {
            // The right arm from the elbow to the fingers, close, from outside it and in front,
            // square to the forearm.
            let (elbow, wrist) = (at(Joint::RightLowerArm), at(Joint::RightHand));
            let along = (wrist - elbow).normalize_or(Vec3::Z);
            let mid = elbow.lerp(wrist, 0.6);
            let from = Vec3::new(0.8, 0.35, 0.6);
            let side = (from - along * from.dot(along)).normalize_or(Vec3::X);
            (look_at(mid + side * 0.7, mid), fov(0.36))
        }
        View::Shoulder => {
            // The right shoulder from in front, outside and above.
            let at_ = at(Joint::RightUpperArm);
            (look_at(at_ + Vec3::new(0.45, 0.3, 0.45), at_ + Vec3::new(0.0, -0.05, 0.0)), fov(0.42))
        }
        View::Elbow => {
            // Along the elbow's hinge from outside the body, so a bend shows in profile (bent
            // straight, from the outside).
            let (shoulder, elbow, wrist) = (at(Joint::RightUpperArm), at(Joint::RightLowerArm), at(Joint::RightHand));
            let hinge = (elbow - shoulder).cross(wrist - elbow);
            let hinge = if hinge.length() > 1e-3 { hinge.normalize() } else { Vec3::X };
            let side = if hinge.x < 0.0 { -hinge } else { hinge };
            let from = (side + Vec3::new(0.0, 0.15, 0.15)).normalize();
            (look_at(elbow + from * 0.5, elbow), fov(0.3))
        }
        View::Wrist => {
            // From the thumb's side, so a bend at the wrist shows in profile.
            let wrist = at(Joint::RightHand);
            let hand = Quat::from_array(joints[Joint::RightHand as usize].rot);
            let fingers = hand * Vec3::Y;
            let thumb = (hand * Vec3::Z + Vec3::new(0.0, 0.2, 0.0)).normalize();
            let to = wrist + fingers * 0.02;
            (look_at(to + thumb * 0.35, to), fov(0.3))
        }
        View::Hand => {
            // Down on the back of the right hand and its fingers, a little from the little finger's
            // side, close: where a gauntlet's plates meet its glove.
            let wrist = at(Joint::RightHand);
            let hand = Quat::from_array(joints[Joint::RightHand as usize].rot);
            let (fingers, thumb) = (hand * Vec3::Y, hand * Vec3::Z);
            let back = thumb.cross(fingers).normalize_or(Vec3::Y);
            let to = wrist + fingers * 0.07;
            (look_at(to + (back - thumb * 0.3).normalize() * 0.3, to), fov(0.3))
        }
    }
}

fn look_at(from: Vec3, to: Vec3) -> Pose {
    let z = (to - from).normalize();
    let x = Vec3::Y.cross(z).normalize_or(Vec3::X);
    let y = z.cross(x);
    Pose { pos: from.to_array(), valid: 1, rot: Quat::from_mat3(&Mat3::from_cols(x, y, z)).to_array() }
}

/// Pixel of a tracking-space point, if it's in front.
fn project(eye: &Pose, fov: &Fov, (w, h): (u32, u32), p: Vec3) -> Option<(i32, i32)> {
    let local = Quat::from_array(eye.rot).inverse() * (p - Vec3::from_array(eye.pos));
    if local.z < 0.05 {
        return None;
    }
    let (tx, ty) = (local.x / local.z, local.y / local.z);
    let u = (tx - fov.left) / (fov.right - fov.left);
    let v = (fov.up - ty) / (fov.up - fov.down);
    Some(((u * w as f32) as i32, (v * h as f32) as i32))
}

/// One view of the avatar posed as `joints`, over a plain backdrop: sRGB RGBA8.
fn render(gpu: &Gpu, avatar: &mut Avatar, size: (u32, u32), joints: &[Pose; JOINT_COUNT], fingers: Fingers, view: View, plain: bool, visibility: u32) -> Result<Vec<u8>> {
    let (w, h) = size;
    let target = Texture::new(gpu, &TexDesc::target(w, h, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB))?;
    let mut bg = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let t = y as f32 / h as f32;
        for _ in 0..w {
            bg.extend_from_slice(&[(58.0 - 20.0 * t) as u8, (62.0 - 20.0 * t) as u8, (70.0 - 18.0 * t) as u8, 255]);
        }
    }
    unsafe { gpu.ctx.UpdateSubresource(&target.tex, 0, None, bg.as_ptr().cast(), w * 4, 0) };
    let (eye, fov) = camera(joints, view, size);
    avatar.draw(gpu, target.rtv.as_ref().context("the target's view")?, w, h, &eye, &fov, &frame(joints, fingers, view == View::Eye, plain, visibility), None)?;
    target.read_rgba8(gpu)
}

/// The body posed as `joints`, its hands held as `fingers`, in daylight.
fn frame(joints: &[Pose; JOINT_COUNT], fingers: Fingers, first_person: bool, plain: bool, visibility: u32) -> AvatarFrame<'_> {
    let sun = Vec3::new(-0.45, 0.8, 0.5).normalize();
    AvatarFrame {
        joints,
        fingers: [fingers; 2],
        hands: pose::hands_at_joints(joints),
        ghosts: [None; 2],
        held: [None; 2],
        grasp: [None; 2],
        visibility,
        light: LightHint { sun_dir: sun.to_array(), sun_color: [1.0, 0.94, 0.86], ambient: [0.2, 0.22, 0.26], ..Default::default() },
        first_person,
        plain,
    }
}

/// Tiles side by side, as rows of one picture.
fn tile_row(tiles: &[Vec<u8>], (w, h): (u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity(tiles.len() * (w * h * 4) as usize);
    for y in 0..h as usize {
        for t in tiles {
            let row = (w * 4) as usize;
            out.extend_from_slice(&t[y * row..(y + 1) * row]);
        }
    }
    out
}

/// Digits and a dot, 3 x 5.
const GLYPHS: [(char, [u8; 5]); 11] = [
    ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
    ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
    ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
    ('3', [0b111, 0b001, 0b111, 0b001, 0b111]),
    ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
    ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
    ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
    ('7', [0b111, 0b001, 0b010, 0b010, 0b010]),
    ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
    ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
    ('.', [0b000, 0b000, 0b000, 0b000, 0b010]),
];

/// White text, black-edged, centred on `at`, each glyph pixel 2 x 2.
fn draw_text(px: &mut [u8], (w, h): (u32, u32), at: (i32, i32), text: &str) {
    const K: i32 = 2;
    let width = text.len() as i32 * 4 * K;
    let (x0, y0) = (at.0 - width / 2, at.1 - 5 * K / 2);
    for pass in 0..2 {
        for (n, c) in text.chars().enumerate() {
            let Some((_, rows)) = GLYPHS.iter().find(|(g, _)| *g == c) else { continue };
            for (r, bits) in rows.iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) == 0 {
                        continue;
                    }
                    let (gx, gy) = (x0 + (n as i32 * 4 + col) * K, y0 + r as i32 * K);
                    let (spread, colour) = if pass == 0 { (1, [0, 0, 0, 255]) } else { (0, [255, 255, 255, 255]) };
                    for y in gy - spread..gy + K + spread {
                        for x in gx - spread..gx + K + spread {
                            if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                                let o = ((y as u32 * w + x as u32) * 4) as usize;
                                px[o..o + 4].copy_from_slice(&colour);
                            }
                        }
                    }
                }
            }
        }
    }
}
