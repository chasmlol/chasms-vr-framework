//! Gun rigs: how the player's hands hold a game's guns, measured from the game's own models.
//!
//! A rig file is ours, not the game's: points measured on a gun's first-person model (in the
//! model's own space, the space [`crate::Model`]s are cached in) plus a few facts about the gun.
//! `docs/RIGGING.md` says how each field is measured (`cargo run -p rustvr-asset --example
//! rig_probe`).
//!
//! A game's rigs are `<game>.toml` in the folder `RUSTVR_RIGS` names ([`load`]).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Two hands: the firing hand on the pistol grip, the support hand on the foregrip; shouldered.
    #[default]
    Rifle,
    /// One hand (it holsters on the hip).
    Pistol,
    /// Two hands on handles rather than a pistol grip and a handguard (a spade grip, a side
    /// handle, a lever trigger), fired from the hip: no stock to shoulder.
    Heavy,
}

/// How a gun is worked between shots (the protocol's `ACTION_*`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// It cycles itself (self-loading, semi or full auto): the handle only racks it after a reload.
    #[default]
    Auto,
    /// A bolt action: the bolt's handle (`handle`) is lifted (`bolt_turn`), pulled back and pushed
    /// home after every shot.
    Bolt,
    /// A pump action: the forend (`handle`) is pulled back and pushed forward after every shot.
    Pump,
    /// A break action: the barrels tip down about the `hinge` (model +X) by `open_deg`.
    Break,
    /// A swing-out action: the barrel or cylinder swings out sideways about the `hinge` (a pin along
    /// the bore, model +Z) by `open_deg` (positive: its top to the left).
    Swing,
}

/// An air gun's pressure: what the pump builds, a shot spends, and the gauge that shows it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pressure {
    /// What one full stroke of the pump (the gun's `handle`) adds and one shot spends, as fractions
    /// of full; below `min` it doesn't fire.
    pub pump: f32,
    pub shot: f32,
    pub min: f32,
    /// The gauge's moving part: its bone (and its children), and how it shows the pressure: turned
    /// about the bone's own X axis (`gauge_turn`, degrees) or slid along it (`gauge_slide`, metres),
    /// from empty to full, relative to the model's pose (the game's own poses for it).
    #[serde(default)]
    pub gauge_bone: Option<String>,
    #[serde(default)]
    pub gauge_turn: Option<[f32; 2]>,
    #[serde(default)]
    pub gauge_slide: Option<[f32; 2]>,
}

/// A handle that swings about a pivot (a lever) rather than sliding straight back.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HandleSwing {
    /// The lever's pivot (model space); it swings about model +X through it, `deg` at full travel
    /// (positive: its far end goes down and back).
    pub pivot: [f32; 3],
    pub deg: f32,
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
    /// The gun's key in the game's asset index.
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
    /// (`round_*`).
    #[serde(default)]
    pub mag_bones: Vec<String>,
    #[serde(default)]
    pub bolt_bones: Vec<String>,
    /// Meshes (their index in the model) that are the magazine too: a magazine the extractor
    /// mounted on a locator whose bone isn't the magazine's (rig_probe's `parts` lists them).
    #[serde(default)]
    pub mag_meshes: Vec<usize>,
    /// Bones (with their children) whose skin isn't drawn: parts the game only shows in an
    /// animation or with an upgrade (a round held in the hand while reloading, an ejected case,
    /// the shells of barrels the stock gun doesn't have).
    #[serde(default)]
    pub hidden_bones: Vec<String>,
    /// Meshes (their index in the model) not drawn: glass the test range can't draw see-through
    /// (a gauge's, over its needle).
    #[serde(default)]
    pub hidden_meshes: Vec<usize>,
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
    /// How it's worked between shots (`auto` when left out).
    #[serde(default)]
    pub action: Action,
    /// Break and swing actions: a point on the hinge's pin (the barrels' bone's origin, which the
    /// game's own motion turns them about), how far they open (degrees: the game's own motion's
    /// furthest), and the bones that turn with them (and their children).
    #[serde(default)]
    pub hinge: Option<[f32; 3]>,
    #[serde(default)]
    pub open_deg: f32,
    #[serde(default)]
    pub barrel_bones: Vec<String>,
    /// Guns loaded round by round into their chambers (a double barrel, a single shot): each
    /// chamber's round, a bone (with its children) whose origin is the round's base at the chamber's
    /// mouth, in the order they fire. Such a gun has no magazine (`mag_bones` stays empty): its
    /// "magazine" is one round, the first chamber's.
    #[serde(default)]
    pub chamber_bones: Vec<String>,
    /// Guns whose magazine stays in them and is loaded one round at a time (a tube, a revolving
    /// shotgun's cylinder, a dart cluster): each of its rounds, a bone (with its children) whose
    /// origin is the round's base, in the order they're loaded and fired. `mag_well` is the port
    /// they go in at; no `mag_bones` then.
    #[serde(default)]
    pub round_bones: Vec<String>,
    /// Bolt actions: the bolt's turn about the bore (degrees about model +Z, positive lifting a
    /// handle on the right), closed and open, from the model's pose (the game's own motion's
    /// poses for it).
    #[serde(default)]
    pub bolt_turn: Option<[f32; 2]>,
    /// The handle is a lever that swings rather than slides (`bolt_bones` are the lever).
    #[serde(default)]
    pub handle_swing: Option<HandleSwing>,
    /// Air guns: the pressure its pump (`handle`) builds and its shots spend.
    #[serde(default)]
    pub pressure: Option<Pressure>,
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
        let hinged = matches!(g.action, Action::Break | Action::Swing);
        anyhow::ensure!(!hinged || (g.hinge.is_some() && g.open_deg != 0.0 && !g.barrel_bones.is_empty()), "{}: a hinged action needs its hinge, open_deg and barrel_bones", g.key);
        anyhow::ensure!(hinged || (g.hinge.is_none() && g.barrel_bones.is_empty()), "{}: a hinge on a gun that isn't a break or swing action", g.key);
        anyhow::ensure!(!hinged || g.handle.is_none(), "{}: a hinged action has nothing to rack (its hinge is its moving part)", g.key);
        anyhow::ensure!(!matches!(g.action, Action::Bolt | Action::Pump) || g.handle.is_some(), "{}: a bolt or pump action needs its handle", g.key);
        anyhow::ensure!(g.bolt_turn.is_none() || g.action == Action::Bolt, "{}: bolt_turn is for bolt actions", g.key);
        anyhow::ensure!(g.chamber_bones.is_empty() || g.mag_bones.is_empty(), "{}: loaded into its chambers or from a magazine, not both", g.key);
        anyhow::ensure!(g.round_bones.is_empty() || (g.mag_bones.is_empty() && g.chamber_bones.is_empty()), "{}: a magazine loaded round by round has no magazine to take out", g.key);
        anyhow::ensure!(g.round_bones.is_empty() || g.mag_size.is_none_or(|n| n as usize == g.round_bones.len()), "{}: one round bone per round", g.key);
        anyhow::ensure!(g.chamber_bones.is_empty() || g.mag_size.is_none_or(|n| n as usize == g.chamber_bones.len()), "{}: one chamber bone per chamber", g.key);
        if let Some(p) = &g.pressure {
            anyhow::ensure!(g.handle.is_some(), "{}: an air gun needs its pump (the handle)", g.key);
            anyhow::ensure!(p.pump > 0.0 && p.shot > 0.0 && (0.0..1.0).contains(&p.min), "{}: pressure pump and shot above 0, min in 0..1", g.key);
            anyhow::ensure!(p.gauge_bone.is_none() || (p.gauge_turn.is_some() != p.gauge_slide.is_some()), "{}: a gauge turns or slides", g.key);
        }
    }
    Ok(file)
}

/// The folder of rig files (`RUSTVR_RIGS`).
pub fn override_dir() -> Option<PathBuf> {
    std::env::var_os("RUSTVR_RIGS").map(PathBuf::from)
}

/// The rigs of `game`: `RUSTVR_RIGS/<game>.toml`.
pub fn load(game: &str) -> Result<RigFile> {
    let dir = override_dir().with_context(|| format!("no rigs for {game} (RUSTVR_RIGS isn't set)"))?;
    let path = dir.join(format!("{game}.toml"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let pistol = |extra: &str| parse(&format!("game = \"x\"\n[[gun]]\nhold = \"pistol\"\n{extra}\n{gun}"));
        assert!(pistol("action = \"break\"").is_err(), "a break action without its hinge");
        assert!(pistol("action = \"break\"\nhinge = [0, 0, 0.5]\nopen_deg = 45\nbarrel_bones = [\"b\"]").is_ok());
        assert!(pistol("hinge = [0, 0, 0.5]").is_err(), "a hinge on a self-loading gun");
        assert!(pistol("action = \"bolt\"").is_err(), "a bolt action without a handle");
        assert!(pistol("action = \"bolt\"\nhandle = [0.1, 0, 0]\nhandle_travel = 0.08\nbolt_turn = [0, 90]").is_ok());
        assert!(pistol("chamber_bones = [\"a\"]\nmag_bones = [\"m\"]").is_err(), "chambers and a magazine");
        assert!(pistol("pressure = { pump = 0.25, shot = 0.1, min = 0.1 }").is_err(), "an air gun without its pump");
        let air = "handle = [0, -0.1, 0.5]\nhandle_travel = 0.08\npressure = { pump = 0.25, shot = 0.1, min = 0.1, gauge_bone = \"g\", gauge_turn = [0, 270] }";
        assert!(pistol(air).is_ok());
    }
}
