//! Gun rigs: how the player's hands hold a game's guns, measured from the game's own models.
//!
//! A rig file is ours, not the game's: points measured on a gun's first-person model (in the
//! model's own space, the space [`crate::Model`]s are cached in) plus a few facts about the gun.
//! They're committed under `content/rigs/<game>.toml` and compiled in ([`builtin`]), so the test
//! range and a game's adapter register exactly the same rigs with the runtime. `docs/RIGGING.md`
//! says how each field is measured (`cargo run -p rustvr-asset --example rig_probe`).
//!
//! Set `RUSTVR_RIGS` to a folder to load `<game>.toml` from there instead (to try a rig without
//! rebuilding).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The rig files compiled in, by game.
const BUILTIN: &[(&str, &str)] = &[("metro2033", include_str!("../../../content/rigs/metro2033.toml"))];

/// A game's rigs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RigFile {
    /// The game folder under `local_assets/` the keys refer to.
    pub game: String,
    #[serde(default, rename = "gun")]
    pub guns: Vec<GunRig>,
}

impl RigFile {
    pub fn gun(&self, key: &str) -> Option<&GunRig> {
        self.guns.iter().find(|g| g.key == key)
    }
}

/// How a gun is held.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// Two hands: the firing hand on the pistol grip, the support hand on the foregrip.
    #[default]
    Rifle,
    /// One hand (it holsters on the hip).
    Pistol,
}

/// A box: its centre and half sizes.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub centre: [f32; 3],
    pub half: [f32; 3],
}

/// One gun's rig. Points are in the gun model's own space (metres, X right, Y up, Z forward).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GunRig {
    /// The gun's key in the game's asset index (`wpn_ak`).
    pub key: String,
    /// Shown to the player; the index's name when left out.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub hold: Hold,
    /// What it fires, as the game calls it (`5.45x39`, `12 gauge`...): for naming magazines and
    /// for the record.
    #[serde(default)]
    pub caliber: String,
    /// The magazine it takes. Guns naming the same magazine take each other's (the runtime's
    /// `ammo_type`); the first of them in the file provides it. Left out: its own.
    #[serde(default)]
    pub magazine: Option<String>,
    /// Top of the rear sight notch's shoulders (or an optic's axis) and the front post's tip:
    /// weapon +Z is the line through them.
    pub rear_sight: [f32; 3],
    pub front_sight: [f32; 3],
    /// The firing hand's palm centre on the pistol grip's axis, the grip's rake back from
    /// vertical and an extra turn of the hand about it (degrees). The weapon's origin.
    pub grip: [f32; 3],
    pub grip_rake_deg: f32,
    #[serde(default = "default_twist")]
    pub grip_twist_deg: f32,
    /// The trigger finger's pad on the trigger's face.
    pub trigger: [f32; 3],
    /// The pistol grip as a block: centre, and half size across, front to back (square to its
    /// axis) and along it.
    pub grip_block: Block,
    /// Two-handed guns: the support hand's palm centre cradling the handguard, and its fist's
    /// tube and palm normal (used only without the game's own hands).
    #[serde(default)]
    pub foregrip: Option<[f32; 3]>,
    #[serde(default)]
    pub support_tube: Option<[f32; 3]>,
    #[serde(default)]
    pub support_palm: Option<[f32; 3]>,
    /// Where the magazine's top sits when it's in.
    pub mag_well: [f32; 3],
    pub muzzle: [f32; 3],
    /// The part pulled back to chamber a round (a charging handle, bolt handle, slide, pump
    /// forend or cocking lever): where the hand takes it, and how far it travels back (m).
    #[serde(default)]
    pub handle: Option<[f32; 3]>,
    #[serde(default)]
    pub handle_travel: f32,
    /// The trigger's bone (its blade's skin swings with the finger).
    pub trigger_bone: String,
    /// Bones (with their children) whose skin is the magazine (none: the rounds stay in the
    /// gun), and the part that moves back with the handle. A trailing `*` matches any ending
    /// (`patron_*`).
    #[serde(default)]
    pub mag_bones: Vec<String>,
    #[serde(default)]
    pub bolt_bones: Vec<String>,
    /// Meshes (their index in the model) that are the magazine too: a magazine the extractor
    /// mounted on a locator whose bone isn't the magazine's (rig_probe's `parts` lists them).
    #[serde(default)]
    pub mag_meshes: Vec<usize>,
    /// The runtime's own recoil, for when the game's isn't loaded: muzzle climb (deg), sideways
    /// scatter (deg), push back (m); and its character: climb kept on full auto, return (1/s),
    /// lean (deg).
    pub recoil: [f32; 3],
    pub recoil_feel: [f32; 3],
    pub rounds_per_second: f32,
    #[serde(default = "yes")]
    pub automatic: bool,
    /// The slide or bolt locks back on the last round.
    #[serde(default)]
    pub slide_locks: bool,
    /// Rounds per magazine; the index's when left out.
    #[serde(default)]
    pub mag_size: Option<u32>,
    /// Kilograms: the gun without its magazine, and a loaded magazine.
    pub mass: f32,
    pub mag_mass: f32,
    /// Lies on a table with its left side up (a magazine sticking out of its left side points up).
    #[serde(default)]
    pub left_side_up: bool,
    /// Zoom while aiming down the sights (above 1.4 opens a magnified scope); 1.1 when left out.
    #[serde(default)]
    pub ads_zoom: Option<f32>,
}

fn default_twist() -> f32 {
    -15.0
}

fn yes() -> bool {
    true
}

pub fn parse(text: &str) -> Result<RigFile> {
    let file: RigFile = toml::from_str(text)?;
    for g in &file.guns {
        anyhow::ensure!(file.guns.iter().filter(|o| o.key == g.key).count() == 1, "{} is rigged twice", g.key);
        anyhow::ensure!(g.hold == Hold::Pistol || g.foregrip.is_some(), "{}: a two-handed gun needs a foregrip", g.key);
        anyhow::ensure!(g.handle.is_none() || g.handle_travel > 0.0, "{}: a handle needs its travel", g.key);
    }
    Ok(file)
}

/// The committed rigs of `game`, as compiled in.
pub fn builtin(game: &str) -> Option<RigFile> {
    let (_, text) = BUILTIN.iter().find(|(g, _)| *g == game)?;
    Some(parse(text).unwrap_or_else(|e| panic!("content/rigs/{game}.toml: {e:#}")))
}

/// A folder of rig files to use instead of the compiled-in ones (`RUSTVR_RIGS`).
pub fn override_dir() -> Option<PathBuf> {
    std::env::var_os("RUSTVR_RIGS").map(PathBuf::from)
}

/// The rigs of `game`: `RUSTVR_RIGS/<game>.toml` when that's set, else the compiled-in ones.
pub fn load(game: &str) -> Result<RigFile> {
    if let Some(dir) = override_dir() {
        let path = dir.join(format!("{game}.toml"));
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        return parse(&text).with_context(|| format!("parsing {}", path.display()));
    }
    builtin(game).with_context(|| format!("no rigs for {game}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_rigs_parse() {
        let f = builtin("metro2033").unwrap();
        assert_eq!(f.game, "metro2033");
        let ak = f.gun("wpn_ak").expect("the Kalash is rigged");
        assert_eq!(ak.hold, Hold::Rifle);
        assert_eq!(ak.trigger_bone, "kurok");
        assert!(ak.handle.is_some() && ak.handle_travel > 0.0);
    }

    #[test]
    fn mistakes_are_refused() {
        let gun = r#"
            key = "g"
            rear_sight = [0, 0, 0]
            front_sight = [0, 0, 1]
            grip = [0, 0, 0]
            grip_rake_deg = 20
            trigger = [0, 0, 0]
            grip_block = { centre = [0, 0, 0], half = [0, 0, 0] }
            mag_well = [0, 0, 0]
            muzzle = [0, 0, 1]
            trigger_bone = "t"
            recoil = [0, 0, 0]
            recoil_feel = [0, 0, 0]
            rounds_per_second = 1
            mass = 1
            mag_mass = 0.1
        "#;
        assert!(parse(&format!("game = \"x\"\n[[gun]]\n{gun}")).is_err(), "a rifle without a foregrip");
        assert!(parse(&format!("game = \"x\"\n[[gun]]\nhold = \"pistol\"\n{gun}")).is_ok());
        assert!(parse(&format!("game = \"x\"\n[[gun]]\nhold = \"pistol\"\nbogus = 1\n{gun}")).is_err(), "unknown fields");
    }
}
