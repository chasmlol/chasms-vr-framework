//! The cache format the test range loads its guns, hands and bodies from.
//!
//! Extractors (such as `metro-assets`) read a game's own archives and write these files into
//! `local_assets/` at the repository root. That folder is git-ignored: the files come from games
//! the user owns and are only for local testing. They are never committed or redistributed.
//!
//! Layout of `local_assets/<game>/`:
//!
//! | Path | Contents |
//! |---|---|
//! | `index.toml` | [`Index`]: every extracted item and what it is |
//! | `models/<key>.rvm` | [`Model`], postcard-encoded |
//! | `textures/<key>.rvt` | [`Texture`], postcard-encoded |
//! | `recoil/<key>.rvk` | [`Recoil`] of the gun `<key>`, postcard-encoded |
//!
//! Space: model positions are in the game's own model space, converted to left-handed metres with
//! X right, Y up and Z forward (the same convention as the RustVR protocol). Viewmodels (first-person
//! guns and arms) keep the placement the game uses relative to its first-person camera, so a gun and
//! the arms holding it line up when both are drawn with the same transform.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod rigs;

pub const MODEL_EXT: &str = "rvm";
pub const TEXTURE_EXT: &str = "rvt";
pub const RECOIL_EXT: &str = "rvk";
/// Bump when [`Model`] or [`Texture`] change shape; loaders reject other versions.
pub const FORMAT_VERSION: u32 = 2;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Model {
    pub version: u32,
    /// Where it came from inside the game, e.g. `meshes/dynamic/hud/weapon/ak_74/ak_74.model`.
    pub source: String,
    pub meshes: Vec<Mesh>,
    /// Skeleton in bind pose. Empty for static models.
    pub bones: Vec<Bone>,
    /// Axis-aligned bounds of all vertices (model space, metres).
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mesh {
    /// Texture key (file stem under `textures/`), empty for untextured.
    pub texture: String,
    pub vertices: Vec<Vertex>,
    /// Triangle list, counter-clockwise front faces when viewed in the left-handed space above.
    pub indices: Vec<u32>,
    /// The game's own name for the part, if it has one (useful to hide e.g. a scope lens).
    pub name: String,
    /// How the game shades it.
    pub material: Material,
}

/// How the game shades a surface: its shading model, the textures it binds beside the base
/// colour, and its constants, all as the game's own shaders take them (Metro: the `surf_*`
/// constants, see docs/research/metro-viewmodel-lighting.md section 2.4).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Material {
    pub shader: Shader,
    /// Texture keys (stems under `textures/`), empty for none: the bump map (height in alpha,
    /// gloss in green, specular in green, detail selector in red), the detail map and the
    /// environment cube.
    pub bump: String,
    pub detail: String,
    pub env: String,
    /// Metro's `surf_params`: (detail tiling u, v, normal weight, parallax/detail strength).
    pub params: [f32; 4],
    /// Metro's `surf_color`: albedo tint (rgb, applied ×2), w spare.
    pub color: [f32; 4],
}

/// A game's shading model for a surface (Metro's G-buffer pixel shaders).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Shader {
    /// Not known: drawn with the base texture alone.
    #[default]
    Unknown,
    /// Opaque, no environment term (or alpha-tested).
    Plain,
    /// Metal with the environment reflection baked into the albedo (5622).
    MetalEnv,
    /// Cloth and skin: Lambert, the fake subsurface tint, grey specular (5608).
    ClothSkin,
    /// Glossy glass (5634).
    GlossyGlass,
    /// Emissive, unlit (5696/5699/5751).
    Emissive,
    /// Forward glass, tint only (5732).
    GlassTint,
    /// Forward glass with reflection (5734).
    GlassReflect,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Indices into [`Model::bones`]; weights sum to 255. All zero weights: rigid, unskinned.
    pub bones: [u8; 4],
    pub weights: [u8; 4],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    /// Index into [`Model::bones`], or -1 for a root.
    pub parent: i32,
    /// Bind pose relative to the parent: translation (metres) and rotation quaternion (x, y, z, w).
    pub local_pos: [f32; 3],
    pub local_rot: [f32; 4],
    /// Bind pose in model space (column-major 4x4), precomputed for convenience.
    pub model_matrix: [f32; 16],
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TexFormat {
    /// 8-bit RGBA, sRGB-encoded colour.
    Rgba8,
    /// Block-compressed data exactly as D3D11 consumes it (`DXGI_FORMAT_BC*_UNORM`).
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc7,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Texture {
    pub version: u32,
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub format: TexFormat,
    /// Largest mip first. Each level is tightly packed (block rows for BC formats). A cube map
    /// stores its six faces one after another (+X, -X, +Y, -Y, +Z, -Z), each with all its mips.
    pub mips: Vec<Vec<u8>>,
    /// 1, or 6 for a cube map.
    pub faces: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A first-person gun (viewmodel).
    Weapon,
    /// First-person arms/hands.
    Hands,
    /// A full player or NPC body.
    Body,
    /// Anything else useful as scenery (crates, barrels, targets).
    Prop,
}

/// Points on a gun in its model space, used to put it in the player's hand.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct WeaponRig {
    /// Where the firing hand's palm holds it.
    pub grip: [f32; 3],
    /// Where the other hand holds it (foregrip / handguard).
    pub foregrip: [f32; 3],
    /// Rear sight or optic: the line of sight passes through here, parallel to `forward`.
    pub sight: [f32; 3],
    pub muzzle: [f32; 3],
    /// Where a magazine goes in.
    pub mag_well: [f32; 3],
    /// Barrel direction in model space (unit vector).
    pub forward: [f32; 3],
    /// True when the points came from named bones; false when guessed from the bounds.
    pub from_bones: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    /// File stem of the model under `models/`.
    pub key: String,
    pub kind: ItemKind,
    /// Human-readable name, e.g. "AK-74".
    pub name: String,
    #[serde(default)]
    pub rig: Option<WeaponRig>,
    /// For weapons: the hands model meant to be drawn with it, if known.
    #[serde(default)]
    pub hands: Option<String>,
    /// Rounds per magazine, if known.
    #[serde(default)]
    pub mag_size: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub game: String,
    pub version: u32,
    pub items: Vec<Item>,
}

/// The `local_assets/` folder at the repository root, or `RUSTVR_ASSETS` if set.
pub fn default_root() -> PathBuf {
    if let Some(p) = std::env::var_os("RUSTVR_ASSETS") {
        return PathBuf::from(p);
    }
    // crates/asset -> repository root
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("local_assets")
}

pub fn save_model(dir: &Path, key: &str, model: &Model) -> Result<()> {
    let path = dir.join("models").join(format!("{key}.{MODEL_EXT}"));
    write(&path, &postcard::to_stdvec(model)?)
}

pub fn load_model(dir: &Path, key: &str) -> Result<Model> {
    let path = dir.join("models").join(format!("{key}.{MODEL_EXT}"));
    let m: Model = postcard::from_bytes(&read(&path)?).with_context(|| format!("decoding {}", path.display()))?;
    anyhow::ensure!(m.version == FORMAT_VERSION, "{}: format {} (want {FORMAT_VERSION})", path.display(), m.version);
    Ok(m)
}

pub fn save_texture(dir: &Path, key: &str, tex: &Texture) -> Result<()> {
    let path = dir.join("textures").join(format!("{key}.{TEXTURE_EXT}"));
    write(&path, &postcard::to_stdvec(tex)?)
}

pub fn load_texture(dir: &Path, key: &str) -> Result<Texture> {
    let path = dir.join("textures").join(format!("{key}.{TEXTURE_EXT}"));
    let t: Texture = postcard::from_bytes(&read(&path)?).with_context(|| format!("decoding {}", path.display()))?;
    anyhow::ensure!(t.version == FORMAT_VERSION, "{}: format {} (want {FORMAT_VERSION})", path.display(), t.version);
    Ok(t)
}

// ---- recoil -------------------------------------------------------------------------------------

/// A gun's recoil as the game authored it: the kicks it plays on each shot.
///
/// Each shot plays one kick of each [`Kick::layer`], picked at random among those whose
/// [`Kick::when`] matches how the shot was fired; the kicks of a burst overlap and add up.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Recoil {
    pub version: u32,
    /// Where it came from inside the game.
    pub source: String,
    pub kicks: Vec<Kick>,
}

/// The game's view kick: on a flat screen, the camera's (and with it the gun's) jolt.
pub const KICK_LAYER_VIEW: u8 = 0;
/// The gun model's own kick relative to the camera.
pub const KICK_LAYER_GUN: u8 = 1;

/// [`Kick::when`]: fired from the hip, or aimed (shouldered, down the sights).
pub const KICK_HIP: u32 = 1 << 0;
pub const KICK_AIMED: u32 = 1 << 1;
/// Standing or crouched.
pub const KICK_STANDING: u32 = 1 << 2;
pub const KICK_CROUCHED: u32 = 1 << 3;
/// The first shot of a burst, the next few, or a long burst's later shots.
pub const KICK_FIRST: u32 = 1 << 4;
pub const KICK_FOLLOW: u32 = 1 << 5;
pub const KICK_LONG: u32 = 1 << 6;
/// Any shot.
pub const KICK_ALWAYS: u32 = 0x7F;

/// One authored kick: how the gun moves over the time after a shot, as an offset from where it's
/// held, in its model space (the space [`Model`]s are in).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Kick {
    /// The game's name for it.
    pub name: String,
    pub layer: u8,
    /// `KICK_*` bits: which shots it's for (one bit of each pair or group must match).
    pub when: u32,
    /// Playback rate (1: as authored).
    pub speed: f32,
    /// How long it lasts, in authored time (s).
    pub length: f32,
    /// The point it turns the gun about (model space).
    pub pivot: [f32; 3],
    /// [`KICK_PITCH`] .. [`KICK_Z`]: the offset over time.
    pub channels: [Vec<KickKey>; KICK_CHANNELS],
}

/// Muzzle up (rad). The rotation is yaw, then pitch, then roll (`Ry(yaw) * Rx(-pitch) * Rz(roll)`).
pub const KICK_PITCH: usize = 0;
/// About +Y (rad): positive turns the muzzle (+Z) towards +X.
pub const KICK_YAW: usize = 1;
/// About +Z, the barrel (rad).
pub const KICK_ROLL: usize = 2;
/// Translation (m), model space.
pub const KICK_X: usize = 3;
pub const KICK_Y: usize = 4;
pub const KICK_Z: usize = 5;
pub const KICK_CHANNELS: usize = 6;

/// A key of a kick channel: its time (s), value, and slopes (per second) into and out of it.
/// Between keys a channel is the cubic Hermite through them; before the first key it holds the
/// first value and after the last the last one.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KickKey {
    pub t: f32,
    pub v: f32,
    pub slope_in: f32,
    pub slope_out: f32,
}

/// A channel's value at `t`.
pub fn kick_value(keys: &[KickKey], t: f32) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return 0.0 };
    if t <= first.t {
        return first.v;
    }
    if t >= last.t {
        return last.v;
    }
    let i = keys.partition_point(|k| k.t <= t).clamp(1, keys.len() - 1);
    let (a, b) = (keys[i - 1], keys[i]);
    let h = (b.t - a.t).max(1e-6);
    let s = (t - a.t) / h;
    let (s2, s3) = (s * s, s * s * s);
    (2.0 * s3 - 3.0 * s2 + 1.0) * a.v + (s3 - 2.0 * s2 + s) * h * a.slope_out + (-2.0 * s3 + 3.0 * s2) * b.v + (s3 - s2) * h * b.slope_in
}

/// A channel's slope at `t`, from the left (`right` false) or from the right.
pub fn kick_slope(keys: &[KickKey], t: f32, right: bool) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return 0.0 };
    if t < first.t || t > last.t || (t == first.t && !right) || (t == last.t && right) {
        return 0.0;
    }
    if let Some(k) = keys.iter().find(|k| k.t == t) {
        return if right { k.slope_out } else { k.slope_in };
    }
    let i = keys.partition_point(|k| k.t <= t).clamp(1, keys.len() - 1);
    let (a, b) = (keys[i - 1], keys[i]);
    let h = (b.t - a.t).max(1e-6);
    let s = (t - a.t) / h;
    let s2 = s * s;
    ((6.0 * s2 - 6.0 * s) * a.v + (3.0 * s2 - 4.0 * s + 1.0) * h * a.slope_out + (-6.0 * s2 + 6.0 * s) * b.v + (3.0 * s2 - 2.0 * s) * h * b.slope_in) / h
}

/// The sum of two channels, exactly (with keys at both one's times).
pub fn kick_sum(a: &[KickKey], b: &[KickKey]) -> Vec<KickKey> {
    let mut times: Vec<f32> = a.iter().chain(b).map(|k| k.t).collect();
    times.sort_by(f32::total_cmp);
    times.dedup();
    times
        .into_iter()
        .map(|t| KickKey {
            t,
            v: kick_value(a, t) + kick_value(b, t),
            slope_in: kick_slope(a, t, false) + kick_slope(b, t, false),
            slope_out: kick_slope(a, t, true) + kick_slope(b, t, true),
        })
        .collect()
}

pub fn save_recoil(dir: &Path, key: &str, recoil: &Recoil) -> Result<()> {
    write(&dir.join("recoil").join(format!("{key}.{RECOIL_EXT}")), &postcard::to_stdvec(recoil)?)
}

/// The gun's recoil, if the extractor found one.
pub fn load_recoil(dir: &Path, key: &str) -> Result<Recoil> {
    let path = dir.join("recoil").join(format!("{key}.{RECOIL_EXT}"));
    let r: Recoil = postcard::from_bytes(&read(&path)?).with_context(|| format!("decoding {}", path.display()))?;
    anyhow::ensure!(r.version == FORMAT_VERSION, "{}: format {} (want {FORMAT_VERSION})", path.display(), r.version);
    Ok(r)
}

pub fn save_index(dir: &Path, index: &Index) -> Result<()> {
    write(&dir.join("index.toml"), toml::to_string_pretty(index)?.as_bytes())
}

/// A skeleton's identity from its bone names: the same for an extracted model and the game's own
/// copy in memory, so an adapter can tell which cached model the game is showing (FNV-1a over the
/// names lowercased, sorted and joined by newlines).
pub fn bone_signature<'a>(names: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut v: Vec<String> = names.into_iter().map(|n| n.to_ascii_lowercase()).collect();
    v.sort();
    v.dedup();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, n) in v.iter().enumerate() {
        if i > 0 {
            h = (h ^ 0x0a).wrapping_mul(0x0000_0100_0000_01b3); // '\n' between names
        }
        for b in n.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

impl Model {
    /// [`bone_signature`] of this model's bones.
    pub fn bone_signature(&self) -> u64 {
        bone_signature(self.bones.iter().map(|b| b.name.as_str()))
    }
}

pub fn load_index(dir: &Path) -> Result<Index> {
    let path = dir.join("index.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_round_trip() {
        let dir = std::env::temp_dir().join(format!("rustvr-asset-test-{}", std::process::id()));
        let model = Model {
            version: FORMAT_VERSION,
            source: "test".into(),
            meshes: vec![Mesh {
                texture: "t".into(),
                vertices: vec![Vertex { pos: [1.0, 2.0, 3.0], ..Default::default() }],
                indices: vec![0, 0, 0],
                name: "m".into(),
                material: Default::default(),
            }],
            ..Default::default()
        };
        save_model(&dir, "x", &model).unwrap();
        let back = load_model(&dir, "x").unwrap();
        assert_eq!(back.meshes[0].vertices[0], model.meshes[0].vertices[0]);
        let index = Index {
            game: "g".into(),
            version: 1,
            items: vec![Item {
                key: "x".into(),
                kind: ItemKind::Weapon,
                name: "X".into(),
                rig: Some(WeaponRig::default()),
                hands: None,
                mag_size: Some(30),
            }],
        };
        save_index(&dir, &index).unwrap();
        assert_eq!(load_index(&dir).unwrap().items[0].mag_size, Some(30));
        let recoil = Recoil { version: FORMAT_VERSION, source: "t".into(), kicks: vec![Kick { name: "k".into(), speed: 1.0, ..Default::default() }] };
        save_recoil(&dir, "x", &recoil).unwrap();
        assert_eq!(load_recoil(&dir, "x").unwrap().kicks, recoil.kicks);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn key(t: f32, v: f32, slope_in: f32, slope_out: f32) -> KickKey {
        KickKey { t, v, slope_in, slope_out }
    }

    #[test]
    fn kick_channels_hold_outside_their_keys_and_follow_the_hermite_between() {
        // Linear: slopes equal to the chord.
        let line = [key(0.1, 1.0, -10.0, -10.0), key(0.2, 0.0, -10.0, -10.0)];
        assert_eq!(kick_value(&line, 0.0), 1.0, "held before the first key");
        assert_eq!(kick_value(&line, 0.5), 0.0, "and after the last");
        assert!((kick_value(&line, 0.15) - 0.5).abs() < 1e-5);
        assert!((kick_slope(&line, 0.15, false) + 10.0).abs() < 1e-3);
        // Flat ends: an ease.
        let ease = [key(0.0, 0.0, 0.0, 0.0), key(1.0, 1.0, 0.0, 0.0)];
        assert!((kick_value(&ease, 0.25) - 0.15625).abs() < 1e-5);
    }

    #[test]
    fn bone_signatures_ignore_order_and_case() {
        assert_eq!(bone_signature(["Root", "kurok", "zatvor"]), bone_signature(["zatvor", "root", "KUROK"]));
        assert_ne!(bone_signature(["root", "kurok"]), bone_signature(["root", "kurok", "zatvor"]));
        assert_ne!(bone_signature(["ab", "c"]), bone_signature(["a", "bc"]));
    }

    #[test]
    fn summing_kick_channels_is_exact() {
        let a = [key(0.0, 0.0, 0.0, 2.0), key(0.5, 1.0, 0.0, 0.0)];
        let b = [key(0.2, 0.5, 0.0, -1.0), key(0.8, 0.0, -0.5, 0.0)];
        let s = kick_sum(&a, &b);
        assert_eq!(s.len(), 4);
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            assert!((kick_value(&s, t) - kick_value(&a, t) - kick_value(&b, t)).abs() < 1e-4, "at {t}");
        }
    }
}
