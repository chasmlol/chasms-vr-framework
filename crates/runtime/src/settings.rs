//! Every tunable of the VR experience. Defaults come from the Metro 2033 VR mod's `config.h`, which
//! were tuned by playing; games override them in profiles.
//!
//! `settings.toml` holds the player's settings; `profiles/<adapter>.toml` holds per-game
//! overrides (gun offsets differ between games) and is applied on top. Both are re-read live when
//! they change on disk.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Settings {
    pub vr: Vr,
    pub comfort: Comfort,
    pub input: Input,
    pub weapon: Weapon,
    pub scope: Scope,
    pub ui: Ui,
    pub interaction: Interaction,
    pub body: Body,
    pub sound: Sound,
    pub spectator: Spectator,
}

/// Which eye.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eye {
    Left,
    Right,
}

/// The desktop view (`crate::spectator`): a window of its own for recording and streaming, from
/// between the eyes with a steadied camera, the way the flat game would look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spectator {
    pub enabled: bool,
    /// The eye whose picture of the game is shown (your aiming eye: the sights line up); the other
    /// only fills what it can't see.
    pub eye: Eye,
    /// Degrees across the window.
    pub fov: f32,
    /// Seconds for the camera to turn half way to where the head looks (0 = no smoothing).
    pub smoothing: f32,
    /// Degrees the steadied camera may lag the head at most (it catches up on fast turns).
    pub max_lag: f32,
    /// Keep the horizon level (the head's roll isn't shown).
    pub roll_lock: bool,
    /// The window's first size, pixels (it's kept on the screen).
    pub width: u32,
    pub height: u32,
    /// It opens borderless over the whole monitor (F11 switches back to a window).
    pub fullscreen: bool,
}

impl Default for Spectator {
    fn default() -> Self {
        // (105 degrees across: wide, still inside what the headset's two eyes see.)
        Spectator { enabled: true, eye: Eye::Right, fov: 105.0, smoothing: 0.1, max_lag: 8.0, roll_lock: true, width: 1600, height: 900, fullscreen: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vr {
    /// Multiplies the headset's recommended per-eye resolution.
    pub render_scale: f32,
    /// Cap on the per-eye size's long side, pixels (0 = none).
    pub max_eye_size: u32,
    /// >1 makes the world feel bigger.
    pub world_scale: f32,
    pub ipd_scale: f32,
    /// Move the game's picture, by its depth, to where the head and the walking player are at each
    /// headset frame (`crate::warp`): a game slower than the headset stays smooth.
    pub reproject: bool,
    /// Give the game the head and hands where they'll be when its picture is shown (its picture
    /// comes a frame or more after it's asked for), not where they were when it was asked for.
    pub predict: bool,
    /// Give the headset each frame's depth with its picture, for its own reprojection (when its
    /// runtime takes it). Off by default: Virtual Desktop takes it, but nothing it does is known
    /// to use it, and it's more for the stream to carry.
    pub headset_depth: bool,
    /// The refresh rate to ask the headset for (Hz; 0: its highest). Some runtimes (Virtual
    /// Desktop) keep the rate their own settings have.
    pub refresh_hz: f32,
    /// How far past the headset's view the game draws every eye, all round (degrees,
    /// `targets::render_fov`). Some keeps a turning head from seeing an undrawn edge; more lets the
    /// desktop view go wider on the aiming eye's picture alone (for recording), at the cost of
    /// drawing more.
    pub overscan: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnMode {
    Snap,
    Smooth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveRelative {
    Head,
    OffHand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Comfort {
    pub turn_mode: TurnMode,
    /// Degrees per snap turn.
    pub snap_angle: f32,
    /// Degrees per second at full stick.
    pub smooth_turn_speed: f32,
    pub move_relative: MoveRelative,
    /// Let the head move away from the game body (walk around the play space).
    pub roomscale: bool,
    /// Metres from the body before the view is held back.
    pub roomscale_limit: f32,
    /// Keep the game's camera bob and shake (nauseating for most).
    pub head_bob: bool,
    /// Follow scripted camera animations (ladders, cutscenes).
    pub camera_anims: bool,
    /// Metres added to the eye height.
    pub height_offset: f32,
    /// Darken the edges of the view while moving or turning smoothly (0 = off).
    pub vignette: f32,
    /// Clear radius of the vignette at full strength (0..1 of the view).
    pub vignette_aperture: f32,
    /// Soft edge width.
    pub vignette_feather: f32,
    /// Seconds to fade the vignette in and out.
    pub vignette_ease: f32,
    /// Fade the view out when the head pushes into a wall.
    pub wall_fade: bool,
    /// Seated play: you see from standing height, your body is drawn standing, and ducking
    /// doesn't crouch.
    pub seated: bool,
    /// See from your own eye height (from `body.height`, or as measured standing) instead of the
    /// game character's.
    pub real_height: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hand {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadStyle {
    /// Bring the off hand up into the magazine well (one motion).
    Slap,
    /// Titanfall 2 VR style: eject (firing-hand A), grab a magazine from your hip or chest, seat it,
    /// then rack the charging handle. Guns without a magazine well fall back to `Slap`.
    Full,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    pub dominant_hand: Hand,
    pub stick_deadzone: f32,
    pub two_handed_aim: bool,
    /// The off hand must be this close (metres) to the foregrip to grab it.
    pub two_handed_grab_dist: f32,
    /// Body gestures: gas mask, night vision, medkit, knife, watch...
    pub gestures: bool,
    /// Reload with your hands (otherwise the reload button does it).
    pub manual_reload: bool,
    pub reload_style: ReloadStyle,
    /// Swing a hand fast (a punch or strike) to melee.
    pub melee_swing: bool,
    /// Forward hand speed (m/s) that counts as a strike.
    pub melee_speed: f32,
    /// Hold the off-hand grip and flick up / down / left / right for the game's abilities
    /// (Titanfall 2 VR's titan abilities).
    pub ability_flicks: bool,
    /// Flick speed (m/s) that counts.
    pub flick_speed: f32,
    /// Ducking in real life crouches in the game.
    pub physical_crouch: bool,
    /// Metres below the calibrated standing height that count as crouching.
    pub crouch_threshold: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Weapon {
    /// Metres in controller space.
    pub offset: [f32; 3],
    /// Degrees.
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
    pub scale: f32,
}

/// How the hands touch the world (the runtime's own player controller).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interaction {
    /// Hands collide with the world and objects.
    pub physics_hands: bool,
    /// A see-through hand shows where the controller really is while the hand's held back.
    pub ghost_hands: bool,
    /// Seconds a hand takes to go into the grip on what it takes (and back out letting go): the
    /// wrist onto it, the fingers into their hold, the thing into the hand, a hand let go of a
    /// valve back to the controller. Over more than 20 cm it takes longer in proportion
    /// (`rig::travel_time`); 0: at once.
    pub grab_transition: f32,
    /// Metres from the palm to an object's surface that still grab it.
    pub grab_radius: f32,
    /// Grip press / release thresholds (hysteresis).
    pub grip_press: f32,
    pub grip_release: f32,
    /// Press grip once to hold, again to let go.
    pub grip_toggle: bool,
    /// Guns let go of gently glide back to the holster they came from.
    pub auto_holster: bool,
    /// Gravity gloves: point at an object, grip, flick back to pull it to the hand.
    pub gravity_gloves: bool,
    /// Metres.
    pub glove_range: f32,
    /// Hand speed (m/s) towards you that counts as a flick.
    pub glove_flick_speed: f32,
    /// A pulled object is caught when it comes this close (metres) with the grip held.
    pub glove_catch_radius: f32,
    /// Generous magazine insertion (9 cm, 45 degrees) instead of strict (4 cm, 25 degrees).
    pub easy_mag: bool,
    /// Seating a magazine on a locked-back slide drops the slide.
    pub mag_slam_release: bool,
    /// 0 = everything moves with the hand, 1 = heavy things lag (the Boneworks feel).
    pub weight: f32,
    /// How hard a game's mechanisms (valves, switches, levers) are to turn: their stiffness as the
    /// game made it times this (0.25: easy, 2: twice as stiff).
    pub mechanism_stiffness: f32,
    /// Multiplies throw speed (on top of the speed-dependent gain every throw gets).
    pub throw_scale: f32,
    /// How grenades arm: the other hand pulls the pin, or a button on the hand holding it.
    pub grenade_arming: GrenadeArming,
    /// Throwing knives stick more forgivingly (the blade within 72 degrees of its flight, not 60).
    pub knife_assist: bool,
    /// Shoulder the gun: its stock rests at the shoulder when held near the face.
    pub virtual_stock: bool,
    /// Draw a laser from held guns.
    pub laser_sight: bool,
    /// Guns kick the way their game makes them (when it says how); off: the runtime's own recoil.
    pub game_recoil: bool,
    /// The game's recoil: how much of its turn and its push the gun takes (1: all of it).
    pub game_recoil_turn: f32,
    pub game_recoil_push: f32,
    /// The game's recoil is for a long gun in both hands at the shoulder; this many times as hard
    /// in one hand, and with both hands but off the shoulder.
    pub recoil_one_hand: f32,
    pub recoil_unbraced: f32,
}

/// How a grenade arms (see `rig::throwable`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrenadeArming {
    /// The other hand pulls the pin; a lever holds the fuse until you let go (or press B to cook).
    /// One-handed anyway while the other hand holds a gun, or for grenades without a pin.
    Pin,
    /// The trigger or A on the hand holding it arms it and starts the fuse.
    Button,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyVisibility {
    Hands,
    Arms,
    Full,
}

/// Interaction sounds (grabs, holsters, reloads, shots): the game's own where it has a sound pack,
/// the runtime's synthesised ones otherwise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sound {
    pub enabled: bool,
    /// 0..1
    pub volume: f32,
    /// Play gunshots too (off when the game makes its own).
    pub shots: bool,
    /// The game's own sounds where it has a sound pack (off: always the runtime's own).
    pub game_sounds: bool,
}

impl Default for Sound {
    fn default() -> Self {
        Sound { enabled: true, volume: 0.8, shots: true, game_sounds: true }
    }
}

/// The player's body (IK avatar).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Body {
    pub visibility: BodyVisibility,
    /// Metres, the player's real height (0 = measure it as you stand).
    pub height: f32,
    /// Metres, controller grip to controller grip with the arms straight out (0 = automatic: an
    /// estimate from the height that grows whenever your real arms reach further).
    pub arm_span: f32,
    /// Hide the head and neck (always) so they never block the view.
    pub hide_head: bool,
    /// Shade the body with the game's own lighting model when the game publishes its lighting
    /// (otherwise, and when off, the runtime's own look).
    pub game_lighting: bool,
    /// The elbows where the headset's body tracking has them (a Quest 3 or Pro's, when its
    /// runtime gives it), not only where the arm's model puts them.
    pub tracked_elbows: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub enabled: bool,
    /// Square resolution of the scope view, pixels.
    pub resolution: u32,
    /// Radius of the virtual lens, metres.
    pub lens_size: f32,
    /// Magnification when forced (normally the game's own zoom is used).
    pub magnification: f32,
    /// Dev: always show the lens.
    pub force: bool,
    /// Metres from the optic mount back to the eyepiece.
    pub lens_back: f32,
    /// Nominal eye-to-lens distance for true magnification.
    pub eye_relief: f32,
    /// Render the scope view after every N eye pairs.
    pub every: u32,
    /// 0 none, 1 cross, 2 cross + dot.
    pub reticle: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub menu_distance: f32,
    /// Metres wide.
    pub menu_size: f32,
    pub hud_distance: f32,
    pub hud_size: f32,
    /// The HUD lazily follows the head (otherwise it stays where it was put).
    pub hud_follow_head: bool,
    /// The head turns this far before the HUD follows (degrees), and it catches up in this time
    /// constant (seconds).
    pub hud_deadzone_deg: f32,
    pub hud_catchup: f32,
    /// Place the HUD's pieces where VR games do, when the game says what they are (see `hud.rs`).
    pub split_hud: bool,
    /// The caption strip (subtitles, hints, prompts): distance, how far under gaze (degrees), the
    /// whole HUD layer's width on it (degrees: text size), and the widest a piece gets.
    pub caption_distance: f32,
    pub caption_pitch_deg: f32,
    pub caption_width_deg: f32,
    pub caption_max_deg: f32,
    /// Width of the ammo counter on the gun, and of the status on the wrist (metres).
    pub ammo_width: f32,
    pub status_width: f32,
}


impl Default for Vr {
    fn default() -> Self {
        Vr {
            render_scale: 1.0,
            max_eye_size: 2208,
            world_scale: 1.0,
            ipd_scale: 1.0,
            reproject: true,
            predict: true,
            headset_depth: false,
            refresh_hz: 0.0,
            overscan: 0.0,
        }
    }
}

impl Default for Comfort {
    fn default() -> Self {
        Comfort {
            turn_mode: TurnMode::Snap,
            snap_angle: 45.0,
            smooth_turn_speed: 120.0,
            move_relative: MoveRelative::Head,
            roomscale: true,
            roomscale_limit: 0.6,
            head_bob: false,
            camera_anims: true,
            height_offset: 0.0,
            vignette: 0.5,
            vignette_aperture: 0.7,
            vignette_feather: 0.2,
            vignette_ease: 0.3,
            wall_fade: true,
            seated: false,
            real_height: false,
        }
    }
}

impl Default for Interaction {
    fn default() -> Self {
        Interaction {
            physics_hands: true,
            ghost_hands: false,
            grab_transition: 0.15,
            grab_radius: 0.07,
            grip_press: 0.6,
            grip_release: 0.4,
            grip_toggle: false,
            auto_holster: false,
            gravity_gloves: true,
            glove_range: 10.0,
            glove_flick_speed: 1.2,
            glove_catch_radius: 0.5,
            easy_mag: true,
            mag_slam_release: true,
            weight: 0.5,
            mechanism_stiffness: 1.0,
            throw_scale: 1.0,
            grenade_arming: GrenadeArming::Pin,
            knife_assist: true,
            virtual_stock: false,
            laser_sight: false,
            game_recoil: true,
            game_recoil_turn: 1.0,
            game_recoil_push: 1.0,
            recoil_one_hand: 3.0,
            recoil_unbraced: 1.7,
        }
    }
}

impl Default for Body {
    fn default() -> Self {
        Body { visibility: BodyVisibility::Full, height: 0.0, arm_span: 0.0, hide_head: true, game_lighting: true, tracked_elbows: true }
    }
}

impl Default for Input {
    fn default() -> Self {
        Input {
            dominant_hand: Hand::Right,
            stick_deadzone: 0.08,
            two_handed_aim: true,
            two_handed_grab_dist: 0.25,
            gestures: true,
            manual_reload: true,
            reload_style: ReloadStyle::Full,
            melee_swing: true,
            melee_speed: 2.8,
            ability_flicks: true,
            flick_speed: 2.0,
            physical_crouch: true,
            crouch_threshold: 0.35,
        }
    }
}

impl Default for Weapon {
    fn default() -> Self {
        Weapon { offset: [0.0; 3], pitch: 0.0, yaw: 0.0, roll: 0.0, scale: 1.0 }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Scope {
            enabled: true,
            resolution: 768,
            lens_size: 0.026,
            magnification: 4.0,
            force: false,
            lens_back: 0.04,
            eye_relief: 0.08,
            every: 1,
            reticle: 2,
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            menu_distance: 2.2,
            menu_size: 2.6,
            hud_distance: 1.4,
            hud_size: 1.5,
            hud_follow_head: true,
            hud_deadzone_deg: 20.0,
            hud_catchup: 0.15,
            split_hud: true,
            caption_distance: 1.5,
            caption_pitch_deg: 13.0,
            caption_width_deg: 70.0,
            caption_max_deg: 50.0,
            ammo_width: 0.10,
            status_width: 0.09,
        }
    }
}

impl Settings {
    pub fn dominant(&self) -> usize {
        match self.input.dominant_hand {
            Hand::Left => 0,
            Hand::Right => 1,
        }
    }
}

/// Loads `settings.toml` (plus the active profile) and notices when either changes on disk.
pub struct SettingsStore {
    dir: PathBuf,
    profile: Option<String>,
    stamp: (Option<SystemTime>, Option<SystemTime>),
    pub current: Settings,
}

impl SettingsStore {
    /// `dir` holds `settings.toml` and `profiles/`. A missing file means defaults.
    pub fn open(dir: &Path) -> SettingsStore {
        let mut s = SettingsStore { dir: dir.to_owned(), profile: None, stamp: (None, None), current: Settings::default() };
        s.reload();
        s
    }

    /// In-memory only (tests).
    pub fn memory(settings: Settings) -> SettingsStore {
        SettingsStore { dir: PathBuf::new(), profile: None, stamp: (None, None), current: settings }
    }

    pub fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.toml")
    }

    fn profile_path(&self) -> Option<PathBuf> {
        self.profile.as_ref().map(|p| self.dir.join("profiles").join(format!("{p}.toml")))
    }

    /// Switches the game profile (the adapter's name). Reloads if it changed.
    pub fn set_profile(&mut self, name: Option<&str>) {
        let name = name.filter(|n| !n.is_empty()).map(sanitize);
        if name != self.profile {
            self.profile = name;
            self.reload();
        }
    }

    /// Re-reads the files if their timestamps changed. True if the settings changed.
    pub fn poll(&mut self) -> bool {
        if self.dir.as_os_str().is_empty() {
            return false;
        }
        let stamp = (mtime(&self.settings_path()), self.profile_path().and_then(|p| mtime(&p)));
        if stamp != self.stamp {
            let before = self.current.clone();
            self.reload();
            return self.current != before;
        }
        false
    }

    fn reload(&mut self) {
        if self.dir.as_os_str().is_empty() {
            return;
        }
        let base = read_table(&self.settings_path());
        let merged = match self.profile_path().map(|p| read_table(&p)) {
            Some(profile) => merge(base, profile),
            None => base,
        };
        match toml::Value::Table(merged).try_into::<Settings>() {
            Ok(s) => self.current = s,
            Err(e) => tracing::warn!("settings: {e} (keeping the previous settings)"),
        }
        self.stamp = (mtime(&self.settings_path()), self.profile_path().and_then(|p| mtime(&p)));
    }

    /// Saves the current settings to `settings.toml` (the in-headset menu calls this). Values
    /// equal to the active profile's are still written, so the player's choice sticks.
    pub fn save(&mut self) -> Result<()> {
        if self.dir.as_os_str().is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let text = toml::to_string_pretty(&self.current)?;
        std::fs::write(self.settings_path(), text).with_context(|| format!("writing {}", self.settings_path().display()))?;
        self.stamp.0 = mtime(&self.settings_path());
        Ok(())
    }
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn read_table(p: &Path) -> toml::Table {
    let Ok(text) = std::fs::read_to_string(p) else { return toml::Table::new() };
    match text.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("settings: {}: {e}", p.display());
            toml::Table::new()
        }
    }
}

/// `over` wins; nested tables merge key by key.
fn merge(mut base: toml::Table, over: toml::Table) -> toml::Table {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => {
                let merged = merge(std::mem::take(b), o);
                *b = merged;
            }
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn profile_overrides_and_partial_files() {
        let dir = std::env::temp_dir().join(format!("rustvr-settings-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("profiles")).unwrap();
        std::fs::write(dir.join("settings.toml"), "[comfort]\nturn_mode = \"smooth\"\n").unwrap();
        std::fs::write(dir.join("profiles").join("metro2033.toml"), "[weapon]\npitch = 12.5\n").unwrap();
        let mut store = SettingsStore::open(&dir);
        assert_eq!(store.current.comfort.turn_mode, TurnMode::Smooth);
        assert_eq!(store.current.weapon.pitch, 0.0);
        store.set_profile(Some("metro2033"));
        assert_eq!(store.current.weapon.pitch, 12.5);
        assert_eq!(store.current.comfort.turn_mode, TurnMode::Smooth, "base settings still apply");
        assert_eq!(store.current.comfort.snap_angle, Comfort::default().snap_angle, "missing keys keep defaults");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
