//! Armour marks: which parts of a game's body models are rigid plates, and what each plate rides.
//!
//! A body model (a cuirass, a pair of gauntlets) is skinned by its artist with soft weights
//! everywhere, so drawn on the player its plates bend like cloth. A marks file says, piece by piece,
//! which of its parts are plates: each plate then moves as one rigid piece, riding one bone, or
//! several with shares (a couter half on the upper arm and half on the forearm, turning about the
//! elbow). Everything not marked stays soft (and is skinned so its joints keep their volume, see
//! `rustvr_avatar`). `docs/ARMOUR.md` is the guide to marking a game's armour.
//!
//! Marks are ours, hand-written per game in `content/armour/<game>.toml` of the game's adapter
//! repository (beside its `local_assets/`), or this repository's for a game it carries. Set
//! `RUSTVR_ARMOUR` to a file to use it instead (to try an edit without rebuilding); a modder's own
//! marks for their own models go in `local_assets/<game>/armour/*.toml`, read after the game's.
//!
//! Plates are chosen from a model's **islands**: the pieces of a mesh whose triangles share
//! corners (corners at the same place count as shared, whatever their texture coordinates), which
//! is how a game's plates are usually modelled. Bone names may use `*` for any run of characters
//! (`Bip01 * Forearm` is either forearm); a plate riding a bone named that way rides the one on its
//! own side. Points are in the model's space (metres, X to the body's right, Y up, Z forward) and
//! given for the right side: a plate on the left side uses them mirrored.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A game's marks.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArmourFile {
    /// The game folder under `local_assets/` the models are in.
    pub game: String,
    /// What the preview tool puts a body together from.
    #[serde(default)]
    pub preview: Preview,
    /// Named sets of models, for the preview (`rustvr-armour <game> --set <name>`).
    #[serde(default, rename = "set")]
    pub sets: Vec<Set>,
    #[serde(default, rename = "piece")]
    pub pieces: Vec<Piece>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    /// The skeleton model every body part is skinned to (a model path as the game names it).
    #[serde(default)]
    pub skeleton: String,
    /// The bare body, worn under what a set leaves uncovered: each with the slot it fills
    /// (`[[preview.skin]]`), so a piece that covers the slot (`Piece::covers`) replaces it.
    #[serde(default)]
    pub skin: Vec<Skin>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Skin {
    pub slot: String,
    pub model: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub name: String,
    pub models: Vec<String>,
}

/// One piece of armour or clothing: the models it's made of (a male and a female version, say,
/// when the same marks fit both), and its plates.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Piece {
    pub name: String,
    /// Model paths as the game's forms name them (any case, either slash; `meshes\` may be left
    /// off).
    pub models: Vec<String>,
    /// The preview's slots it covers (its skin isn't worn under it).
    #[serde(default, deserialize_with = "one_or_many")]
    pub covers: Vec<String>,
    #[serde(default, rename = "plate")]
    pub plates: Vec<Plate>,
}

/// A rigid part of a piece. Chosen by its mesh, its islands and their bones (all optional, each
/// narrowing the choice); one plate a side (left, right, the middle), made of every island chosen
/// on that side.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Plate {
    pub name: String,
    /// The mesh: its index in the model, or its name (none: every mesh).
    #[serde(default)]
    pub mesh: Option<MeshRef>,
    /// Islands by number, as the preview lists them for the mesh (none: every island).
    #[serde(default)]
    pub island: Vec<usize>,
    /// Islands whose skin is mostly on one of these bones (none: whatever bone).
    #[serde(default, deserialize_with = "one_or_many")]
    pub bone: Vec<String>,
    /// Which sides: `any` (the default), `both` (left and right, not the middle), `left`, `right`
    /// or `middle`.
    #[serde(default)]
    pub side: Sides,
    /// Islands whose middle is inside this box (right side; mirrored on the left).
    #[serde(default)]
    pub within: Option<Bounds>,
    /// Of the islands chosen, only the corners mostly on one of these bones (none: all of them):
    /// a plate that is part of an island (a glove's back plate, a boot's toe cap).
    #[serde(default, deserialize_with = "one_or_many")]
    pub vertices: Vec<String>,
    /// What it rides: bones and their shares (none: its own skin's weights, averaged).
    #[serde(default)]
    pub ride: BTreeMap<String, f32>,
    /// What it turns about when its riders turn apart: a bone's joint, or a point (none: the joint
    /// between its two heaviest riders, else its own middle).
    #[serde(default)]
    pub pivot: Option<Pivot>,
    /// Drawn over the plates of lower layers where they meet (0, the default, the lowest), so
    /// overlapping plates slide over one another rather than clip.
    #[serde(default)]
    pub layer: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MeshRef {
    Index(usize),
    Name(String),
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Sides {
    #[default]
    Any,
    Both,
    Left,
    Right,
    Middle,
}

/// Which side of the body something is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Left,
    Middle,
    Right,
}

impl Side {
    /// The side a point is on: more than `MIDDLE` from the body's middle plane.
    pub fn of(x: f32) -> Side {
        if x < -MIDDLE {
            Side::Left
        } else if x > MIDDLE {
            Side::Right
        } else {
            Side::Middle
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Middle => "middle",
            Side::Right => "right",
        }
    }
}

/// Metres from the body's middle plane within which a thing is in the middle.
pub const MIDDLE: f32 = 0.02;

impl Sides {
    pub fn takes(self, side: Side) -> bool {
        match self {
            Sides::Any => true,
            Sides::Both => side != Side::Middle,
            Sides::Left => side == Side::Left,
            Sides::Right => side == Side::Right,
            Sides::Middle => side == Side::Middle,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    /// `p` (right side, or mirrored onto it) is inside.
    pub fn contains(&self, p: [f32; 3]) -> bool {
        let p = [p[0].abs(), p[1], p[2]];
        (0..3).all(|k| p[k] >= self.min[k] && p[k] <= self.max[k])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Pivot {
    /// A bone's joint (its origin in the bind pose).
    Bone(String),
    /// A point (right side; mirrored on the left).
    Point([f32; 3]),
}

/// A string or a list of them.
fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Either::deserialize(d)? {
        Either::One(s) => vec![s],
        Either::Many(v) => v,
    })
}

impl ArmourFile {
    /// The piece a model is (by its path as the game names it, or as the asset index lists it),
    /// the last one that names it.
    pub fn piece(&self, model: &str) -> Option<&Piece> {
        let want = normalise(model);
        self.pieces.iter().rev().find(|p| p.models.iter().any(|m| normalise(m) == want))
    }

    pub fn set(&self, name: &str) -> Option<&Set> {
        self.sets.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Adds another file's sets and pieces after these (its pieces win for the models they name).
    pub fn extend(&mut self, other: ArmourFile) {
        self.sets.extend(other.sets);
        self.pieces.extend(other.pieces);
        if self.preview.skeleton.is_empty() {
            self.preview = other.preview;
        }
    }
}

/// A model path as marks compare them: lowercase, backslashes, without a leading `meshes\` (or
/// `data\meshes\`).
pub fn normalise(path: &str) -> String {
    let mut p = path.trim().replace('/', "\\").to_ascii_lowercase();
    while p.starts_with('\\') {
        p.remove(0);
    }
    for prefix in ["data\\", "meshes\\"] {
        if let Some(rest) = p.strip_prefix(prefix) {
            p = rest.to_owned();
        }
    }
    p
}

/// Whether `name` matches `pattern`, ignoring case: `*` matches any run of characters.
pub fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some((b'*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some((c, rest)) => n.split_first().is_some_and(|(d, tail)| c.eq_ignore_ascii_case(d) && go(rest, tail)),
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

pub fn parse(text: &str) -> Result<ArmourFile> {
    let f: ArmourFile = toml::from_str(text)?;
    for piece in &f.pieces {
        if piece.models.is_empty() {
            bail!("piece '{}' names no models", piece.name);
        }
        for plate in &piece.plates {
            let what = || format!("piece '{}', plate '{}'", piece.name, plate.name);
            if plate.ride.values().any(|s| !(*s > 0.0 && s.is_finite())) {
                bail!("{}: a ride share isn't a positive number", what());
            }
            if plate.ride.len() > 4 {
                bail!("{}: rides {} bones (at most 4)", what(), plate.ride.len());
            }
            if plate.layer > 7 {
                bail!("{}: layer {} (at most 7)", what(), plate.layer);
            }
        }
    }
    Ok(f)
}

/// Where a game's marks are read from: `RUSTVR_ARMOUR` when that's set, else
/// `content/armour/<game>.toml` in the repository ([`crate::repo_root`]: a game adapter's, which
/// has this framework as its `framework/`), else this framework's own.
pub fn path(game: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RUSTVR_ARMOUR") {
        return Some(PathBuf::from(p));
    }
    let file = format!("{game}.toml");
    let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("content").join("armour");
    [crate::repo_root().join("content").join("armour"), own].into_iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

/// A game's marks ([`path`]), with a modder's own (`<assets>/armour/*.toml`, `assets` being the
/// game's folder under `local_assets/`) after them. None found: no marks (an error only for a file
/// that's there and wrong).
pub fn load(game: &str, assets: &Path) -> Result<ArmourFile> {
    let mut out = ArmourFile { game: game.to_owned(), ..Default::default() };
    if let Some(p) = path(game) {
        let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        out = parse(&text).with_context(|| format!("parsing {}", p.display()))?;
    }
    let mut extra: Vec<PathBuf> = std::fs::read_dir(assets.join("armour"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect())
        .unwrap_or_default();
    extra.sort();
    for p in extra {
        let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        out.extend(parse(&text).with_context(|| format!("parsing {}", p.display()))?);
    }
    Ok(out)
}
