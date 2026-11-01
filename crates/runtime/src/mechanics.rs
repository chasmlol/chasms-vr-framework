//! The player's body -> what they're doing in the game.
//!
//! The numbers here were tuned by playing. Everything here works in tracking space from controller
//! poses, so it's the same for every game: the adapter only maps the results into its world and
//! onto its own input.
//!
//! * The gun sits in the firing hand (rig grip on the palm, barrel along the controller's aim) and
//!   pivots around that hand when the other hand holds the foregrip.
//! * Shots follow the line of sight; aiming down the sights is detected when that line passes
//!   close to an eye (or the grip button is held).
//! * Scopes: a magnified camera just past the muzzle renders into a lens behind the optic.
//! * Movement, snap/smooth turning, sprint toggle, weapon switching on the stick.
//! * Roomscale: the game body is walked after the head when the stick is idle.
//! * Physical crouch, magazine reload (off hand into the magazine well), body gestures (grip in a
//!   zone: face, top of head, chest, shoulder, hip) and the watch (wrist up in front of the eyes).
//!
//! Rig mode: when the game registers objects, the rig (`crate::rig`) owns the hands. Guns are
//! picked up, held, reloaded and fired there, so this takes the held gun's pose from the rig,
//! leaves the hands' grip and gun buttons alone, and maps the rest the way most VR shooters do:
//! X jump, Y flashlight, empty-hand trigger use, right stick down crouch, hold B weapon wheel.

use glam::{EulerRot, Mat3, Mat4, Quat, Vec2, Vec3};
use rustvr_protocol::*;

use crate::backend::HmdFrame;
use crate::rig::RigOutput;
use crate::settings::{MoveRelative, ReloadStyle, Settings, TurnMode};

/// A request to vibrate a controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Haptic {
    pub hand: usize,
    pub seconds: f32,
    pub frequency: f32,
    pub amplitude: f32,
}

/// The gun this frame, in tracking space (the compositor draws the scope lens from it).
#[derive(Clone, Copy, Debug, Default)]
pub struct WeaponPose {
    pub weapon_to_tracking: Mat4,
    pub sight_origin: Vec3,
    pub aim_dir: Vec3,
    pub muzzle: Vec3,
    pub two_handed: bool,
    pub ads: bool,
    pub sight_to_eye: f32,
    pub zoom: f32,
    pub scope: Option<ScopeView>,
    /// The gun's own laser (fitted and on, the game says): where it shines from, tracking space.
    pub laser: Option<Vec3>,
}

#[derive(Clone, Copy, Debug)]
pub struct ScopeView {
    /// Camera just past the muzzle (tracking space).
    pub cam_pos: Vec3,
    pub cam_rot: Quat,
    pub vfov_deg: f32,
    pub aspect: f32,
    /// Lens disc behind the optic, facing the eye.
    pub lens_pos: Vec3,
    pub lens_right: Vec3,
    pub lens_up: Vec3,
    pub lens_radius: f32,
}

/// Where the laser hits the game's menu panel (from the panel layout).
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub uv: Vec2,
    pub down: bool,
}

pub struct MechInput<'a> {
    pub frame: &'a HmdFrame,
    pub game: Option<&'a GameState>,
    pub settings: &'a Settings,
    pub now: f64,
    pub dt: f32,
    /// The game's menu is shown as a panel (gameplay input off).
    pub menu_mode: bool,
    /// The runtime's own menu has the controllers.
    pub runtime_menu: bool,
    pub cursor: Option<Cursor>,
    /// Aspect of the aux texture the scope view renders into.
    pub aux_aspect: f32,
    /// Game rumble strength reported this frame (0..1), if any.
    pub rumble: Option<f32>,
    /// The rig's frame when it owns the hands (the game registered objects).
    pub rig: Option<&'a RigOutput>,
    /// Hands whose trigger charges a spell (`crate::combat`): it's not the game's "use".
    pub spell_hands: [bool; 2],
    /// Legacy mode's recoil: the shots the game says it fired since the last frame
    /// (`EV_GAME_SHOTS`), and its held gun's kicks (`KICK_OBJECT_HELD`).
    pub game_shots: u32,
    pub held_kicks: &'a [KickDesc],
    /// The player rides a mount (`crate::riding`): the body stays in the saddle (no walking it
    /// after the head), the mount turns the view (the turning stick steers it instead), and the
    /// trigger of a hand off the reins is riding's (getting off), not the game's "use".
    pub riding: bool,
    /// Hands on the face shaped in a character creator (`crate::sculpt`): their trigger is the
    /// pull's, not the game's menu's.
    pub sculpting: [bool; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct MechOutput {
    pub intents: Intents,
    pub tracking: TrackingControl,
    pub weapon: Option<WeaponPose>,
}

impl Default for MechOutput {
    fn default() -> Self {
        MechOutput { intents: bytemuck::Zeroable::zeroed(), tracking: TrackingControl::default(), weapon: None }
    }
}

/// Values tests and the debug overlay look at.
#[derive(Clone, Copy, Debug, Default)]
pub struct MechDebug {
    /// Legacy mode: the game's kicks playing on the gun.
    pub game_kicks: usize,
    pub two_handed: bool,
    pub ads: bool,
    pub reloads: u32,
    pub gestures: [u32; 7],
    pub following: bool,
    pub phys_crouch: bool,
    pub game_crouched: bool,
    pub height_comp: f32,
    pub dist_mag: f32,
    pub up_speed: f32,
    pub sight_to_eye: f32,
    pub stand_h: f32,
    pub room_len: f32,
    pub watch: bool,
    pub mag: MagState,
    pub ejects: u32,
    pub melees: u32,
    /// Ability flicks: up, down, left, right.
    pub abilities: [u32; 4],
}

/// Where the magazine is during a full manual reload (Titanfall 2 VR style).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MagState {
    /// In the gun.
    #[default]
    In,
    /// Ejected: fetch a new one from the hip or chest.
    Out,
    /// The off hand holds a fresh magazine.
    Held,
    /// Seated in the gun; rack the charging handle to finish.
    Seated,
}

/// A button's press / tap / long-press tracking.
#[derive(Clone, Copy, Default)]
struct Btn {
    down: bool,
    prev: bool,
    t0: f64,
    long_fired: bool,
    /// Released quickly this frame.
    tapped: bool,
    /// The long-press threshold was crossed this frame.
    long_start: bool,
}

const LONG_PRESS: f64 = 0.45;
/// A free hand points (the game picks what to use along it) once it's reached out this far in
/// front of the head (metres): put up towards something, not hanging at the side; it stops once
/// it's back this much nearer. The other free hand takes over once it's reached out this much
/// further.
const POINT_OUT: f32 = 0.3;
const POINT_BACK: f32 = 0.05;
const POINT_SWITCH: f32 = 0.1;
/// How long a lone stick click waits for the other stick (both together: the framework view).
const STICK_CHORD: f64 = 0.12;
/// Heights a worn headset's at (metres over the real floor), sat or stood (not on a desk): seated
/// play takes the head's height as upright only within them.
const UPRIGHT_LOW: f32 = 0.9;
const UPRIGHT_HIGH: f32 = 2.3;
/// Where a nod turns the head (its frame: x right, y up, z forward), metres: the top of the neck,
/// under and behind the eyes. The head's height is measured there, as if the head were held level:
/// looking down lowers the eyes 10 cm or more, but it isn't a crouch (nor, measured there, does it
/// lift the head's height and its upright with it).
const NECK: Vec3 = Vec3::new(0.0, -0.11, -0.08);

impl Btn {
    fn update(&mut self, d: bool, now: f64) {
        self.prev = self.down;
        self.down = d;
        self.tapped = false;
        self.long_start = false;
        if self.down && !self.prev {
            self.t0 = now;
            self.long_fired = false;
        }
        if self.down && !self.long_fired && now - self.t0 >= LONG_PRESS {
            self.long_fired = true;
            self.long_start = true;
        }
        if !self.down && self.prev && !self.long_fired {
            self.tapped = true;
        }
    }

    fn pressed(&self) -> bool {
        self.down && !self.prev
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Zone {
    None,
    /// The eyes (the face's upper part)...
    Face,
    /// ...and the mouth (its lower part).
    Mouth,
    HeadTop,
    Chest,
    Shoulder,
    Hip,
}

/// Zone of a hand: `local` in a yaw-only head frame (x right, y up, z forward), `full` in the full
/// head frame (pitch included), so face and top of head follow where you look.
fn zone_of(local: Vec3, full: Vec3) -> Zone {
    if full.z > -0.02 && full.z < 0.20 && full.x.abs() < 0.12 && full.y > -0.14 && full.y < 0.08 {
        return if full.y > MOUTH_BELOW { Zone::Face } else { Zone::Mouth };
    }
    if full.y > 0.06 && full.y < 0.30 && full.x.abs() < 0.14 && full.z > -0.15 && full.z < 0.12 {
        return Zone::HeadTop;
    }
    if local.y < -0.05 && local.y > -0.35 && local.z < -0.02 && local.x.abs() < 0.40 {
        return Zone::Shoulder;
    }
    if local.y < -0.25 && local.y > -0.60 && local.z > -0.05 && local.z < 0.30 && local.x.abs() < 0.25 {
        return Zone::Chest;
    }
    if local.y < -0.70 && local.y > -1.10 && local.x.abs() < 0.45 && local.z < 0.25 {
        return Zone::Hip;
    }
    Zone::None
}

/// The face's lower part, the mouth, below this (metres under the eyes).
const MOUTH_BELOW: f32 = -0.06;
/// An open hand sweeping across the eyes this fast (m/s, sideways in the head's frame) wipes them,
/// at most once in this long (s).
const WIPE_SPEED: f32 = 0.7;
const WIPE_COOLDOWN: f64 = 0.6;

fn deadzone(v: f32, dz: f32) -> f32 {
    let a = v.abs();
    if a < dz { 0.0 } else { v.signum() * (a - dz) / (1.0 - dz) }
}

/// A stick past `inner` of its throw, rescaled to start from nothing there and reach 1 at `outer`
/// (a stick's rim rarely reads a full 1), its direction kept whatever it is.
fn radial_deadzone(x: f32, y: f32, inner: f32, outer: f32) -> (f32, f32) {
    let m = x.hypot(y);
    if m <= inner || m <= 1e-6 {
        return (0.0, 0.0);
    }
    let k = ((m - inner) / (outer - inner).max(1e-3)).min(1.0) / m;
    (x * k, y * k)
}

/// The stick's reach that counts as all the way.
const STICK_RIM: f32 = 0.95;

fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::from_array(a)
}

fn q(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

fn yaw_of(d: Vec3) -> f32 {
    d.x.atan2(d.z)
}

/// The magazine well in weapon space: from the rig, or a little in front of the firing hand under
/// the receiver.
pub fn mag_well(rig: &WeaponRig) -> Vec3 {
    if rig.flags & RIG_HAS_MAG_WELL != 0 {
        return v3(rig.mag_well);
    }
    let (rg, lg) = (v3(rig.grip), v3(rig.foregrip));
    rg + (lg - rg) * 0.35 + Vec3::new(0.0, -0.07, 0.0)
}

/// The line of sight in weapon space: where shots start and their direction.
pub fn sight_line(rig: &WeaponRig) -> (Vec3, Vec3) {
    let rg = v3(rig.grip);
    let mut sight = if rig.flags & RIG_HAS_SIGHT != 0 {
        v3(rig.sight)
    } else if rig.flags & RIG_HAS_MUZZLE != 0 {
        v3(rig.muzzle)
    } else {
        rg + Vec3::new(0.0, 0.08, 0.0)
    };
    sight.z = rg.z - 0.05;
    if rig.flags & RIG_SIGHT_LINE != 0 {
        // The adapter learned the exact line of sight: keep the origin at the optic's depth.
        let o = v3(rig.sight_line_origin);
        let d = v3(rig.sight_line_dir).normalize_or(Vec3::Z);
        let optic = if rig.flags & RIG_HAS_SIGHT != 0 { v3(rig.sight) } else { sight };
        return (o + d * (optic - o).dot(d), d);
    }
    (sight, Vec3::Z)
}

/// Default rig for when the game reports none.
pub fn default_rig() -> WeaponRig {
    WeaponRig {
        grip: [0.10, -0.15, 0.39],
        foregrip: [0.06, -0.14, 0.67],
        sight: [0.10, -0.03, 0.58],
        muzzle: [0.10, -0.10, 0.95],
        flags: RIG_HAS_SIGHT | RIG_HAS_MUZZLE | RIG_TWO_HANDED,
        ..Default::default()
    }
}

const MAG_RADIUS: f32 = 0.12;
/// A held magazine seats when it's this close to the magazine well.
const SEAT_RADIUS: f32 = 0.08;
/// The off hand grabs the charging handle within this distance.
const HANDLE_RADIUS: f32 = 0.10;
/// Pulling the charging handle back this far racks the gun.
const RACK_PULL: f32 = 0.05;

pub struct Mechanics {
    /// When a non-finite gun pose was last logged.
    insane_logged: f64,
    /// The game's recoil on the gun glued to the hand (legacy mode), from the shots it reports.
    game_recoil: crate::rig::kick::GameRecoil,
    last_ads: bool,
    btn_a: [Btn; 2],
    btn_b: [Btn; 2],
    btn_stick: [Btn; 2],
    /// A lone stick click waiting out [`STICK_CHORD`] (since when), the two clicked together
    /// (until both are let go), and the lone clicks that go through this frame.
    stick_pending: [Option<f64>; 2],
    stick_chord: bool,
    stick_fire: [bool; 2],
    /// Both sticks clicked together swap the game's picture for the runtime's physics world
    /// (`crate::physics_view`).
    pub framework_view: bool,
    btn_menu: [Btn; 2],
    btn_grip: [Btn; 2],
    btn_trig: [Btn; 2],
    presses: [u16; 64],
    snap_cooldown: f64,
    sprint_toggle: bool,
    last_stick_y_dir: i32,
    two_held: bool,
    // physical crouch, and the view's height
    stand_h: f32,
    /// Seated play has taken the head's height as upright (a recentre takes it again).
    seated_taken: bool,
    recalibrate: bool,
    phys_crouch: bool,
    crouch_by_phys: bool,
    crouch_pending: i32,
    crouch_cooldown: f64,
    /// The hand the game picks what to use along ([`INT_POINT_LEFT`]), if one's free.
    point_hand: Option<usize>,
    stand_eye: f32,
    // roomscale follow
    following: bool,
    // magazine reload
    prev_mag_rel: Option<Vec3>,
    reload_cooldown: f64,
    in_mag: bool,
    mag: MagState,
    rack_start: Option<Vec3>,
    rig_id: u64,
    melee_cooldown: f64,
    wipe_cooldown: f64,
    flick_cooldown: f64,
    last_zone: [Zone; 2],
    last_rumble: f32,
    // tracking control
    pub turn_total: f32,
    pub recenter_seq: u32,
    pub haptics: Vec<Haptic>,
    pub debug: MechDebug,
}

impl Default for Mechanics {
    fn default() -> Self {
        Mechanics {
            game_recoil: crate::rig::kick::GameRecoil::new(0x9E37_79B9),
            last_ads: false,
            insane_logged: f64::MIN,
            btn_a: Default::default(),
            btn_b: Default::default(),
            btn_stick: Default::default(),
            stick_pending: [None; 2],
            stick_chord: false,
            stick_fire: [false; 2],
            framework_view: false,
            btn_menu: Default::default(),
            btn_grip: Default::default(),
            btn_trig: Default::default(),
            presses: [0; 64],
            snap_cooldown: 0.0,
            sprint_toggle: false,
            last_stick_y_dir: 0,
            two_held: false,
            stand_h: 0.0,
            seated_taken: false,
            recalibrate: false,
            phys_crouch: false,
            crouch_by_phys: false,
            crouch_pending: -1,
            crouch_cooldown: 0.0,
            point_hand: None,
            stand_eye: 1.8,
            following: false,
            prev_mag_rel: None,
            reload_cooldown: 0.0,
            in_mag: false,
            mag: MagState::In,
            rack_start: None,
            rig_id: 0,
            melee_cooldown: 0.0,
            wipe_cooldown: 0.0,
            flick_cooldown: 0.0,
            last_zone: [Zone::None; 2],
            last_rumble: 0.0,
            turn_total: 0.0,
            recenter_seq: 0,
            haptics: Vec::new(),
            debug: MechDebug::default(),
        }
    }
}

impl Mechanics {
    /// The player's eyes' height standing (metres over the floor): 11 cm under the top of the
    /// height they've set (as a height measured standing is set), else a typical grown-up's.
    fn standing_eyes(c: &Settings) -> f32 {
        (if c.body.height > 1.0 { c.body.height } else { crate::body::TYPICAL_HEIGHT }) - 0.11
    }

    /// Playing at your real height: the view goes up or down by the difference between your eye
    /// height and the game character's.
    fn real_height_offset(&self, c: &Settings) -> f32 {
        if !c.comfort.real_height || (c.comfort.seated && c.body.height <= 1.0) {
            return 0.0;
        }
        (Self::standing_eyes(c) - self.stand_eye).clamp(-0.8, 0.6)
    }

    /// The head's upright height (over the real floor), the game's eyes there. Playing standing,
    /// the player's eyes standing (`standing_eyes`), whatever the head does: sat or ducked under
    /// it is a crouch. Playing seated, the head's height `h` when seated play was turned on (or
    /// the game started with it on), taken again at each recentre.
    fn update_upright(&mut self, c: &Settings, h: f32) {
        if !c.comfort.seated {
            self.stand_h = Self::standing_eyes(c);
            self.seated_taken = false;
            self.recalibrate = false;
        } else if (!self.seated_taken || self.recalibrate) && (UPRIGHT_LOW..UPRIGHT_HIGH).contains(&h) {
            self.stand_h = h;
            self.seated_taken = true;
            self.recalibrate = false;
        }
        self.debug.stand_h = self.stand_h;
    }

    /// The view's height is the head's over its upright height (standing, or sat up in a chair),
    /// the game's eyes there. The adapter puts the head at the game's eyes as the head was when the
    /// view was last recentred (its tracking origin); this takes that back off. Recentred with the
    /// headset on the desk, or put on sitting after it started standing, the view would otherwise
    /// float (or sink) for good, and the body hung from it with it.
    fn upright_offset(&self, game: Option<&GameState>) -> f32 {
        match game {
            Some(g) if self.stand_h > 0.0 && g.tracking.world_per_meter > 0.0 => (g.tracking.origin[1] - self.stand_h) * g.tracking.world_per_meter,
            _ => 0.0,
        }
    }

    pub fn recenter(&mut self) {
        self.recenter_seq = self.recenter_seq.wrapping_add(1);
        self.recalibrate = true;
    }

    /// One press of `a` for the game (the runtime's own gestures press them too: a page turned by
    /// hand).
    pub fn press(&mut self, a: Action) {
        self.presses[a as usize] = self.presses[a as usize].wrapping_add(1);
    }

    fn haptic(&mut self, hand: usize, seconds: f32, frequency: f32, amplitude: f32) {
        self.haptics.push(Haptic { hand, seconds, frequency, amplitude });
    }

    /// One headset frame. Haptics requested this frame are appended to `self.haptics`.
    /// Both sticks clicked together toggle the framework view; a lone click goes through as itself
    /// once it's clear it isn't half of that (`STICK_CHORD` later, or let go sooner).
    fn sticks(&mut self, now: f64) {
        self.stick_fire = [false; 2];
        if self.stick_chord {
            if !self.btn_stick[0].down && !self.btn_stick[1].down {
                self.stick_chord = false;
            }
            return;
        }
        for h in 0..2 {
            if self.btn_stick[h].pressed() {
                self.stick_pending[h] = Some(now);
            }
        }
        if self.btn_stick[0].down && self.btn_stick[1].down && self.stick_pending.iter().any(Option::is_some) {
            self.framework_view = !self.framework_view;
            self.stick_chord = true;
            self.stick_pending = [None; 2];
            tracing::info!("framework view {}", if self.framework_view { "on" } else { "off" });
            return;
        }
        for h in 0..2 {
            if let Some(t) = self.stick_pending[h]
                && (now - t >= STICK_CHORD || !self.btn_stick[h].down)
            {
                self.stick_fire[h] = true;
                self.stick_pending[h] = None;
            }
        }
    }

    pub fn update(&mut self, inp: &MechInput) -> MechOutput {
        let f = inp.frame;
        let c = inp.settings;
        let now = inp.now;
        let dom = c.dominant();
        let off = dom ^ 1;
        for h in 0..2 {
            let hand = &f.hands[h];
            self.btn_a[h].update(hand.buttons & CTRL_A != 0, now);
            self.btn_b[h].update(hand.buttons & CTRL_B != 0, now);
            self.btn_stick[h].update(hand.buttons & CTRL_STICK_CLICK != 0, now);
            self.btn_menu[h].update(hand.buttons & CTRL_MENU != 0, now);
            self.btn_grip[h].update(hand.squeeze > 0.7, now);
            self.btn_trig[h].update(hand.trigger > 0.6, now);
        }
        self.sticks(now);

        let mut out = MechOutput {
            tracking: TrackingControl {
            recenter_seq: self.recenter_seq,
            flags: if c.comfort.roomscale { TRK_ROOMSCALE } else { 0 }
                | if c.comfort.head_bob { TRK_HEAD_BOB } else { 0 }
                | if c.comfort.camera_anims { TRK_CAMERA_ANIMS } else { 0 },
            turn_total: self.turn_total,
            world_scale: c.vr.world_scale,
            height_offset: c.comfort.height_offset + self.real_height_offset(c) + self.upright_offset(inp.game),
            height_comp: 0.0,
            room_limit: c.comfort.roomscale_limit,
            carry_step: if c.riding.turn == TurnMode::Snap { c.riding.turn_step.clamp(20.0, 90.0).to_radians() } else { 0.0 },
            },
            ..Default::default()
        };
        out.intents.weapon_to_tracking = Mat4::IDENTITY.to_cols_array();
        if let Some(r) = inp.rumble {
            if r > 0.03 && (r - self.last_rumble).abs() > 1e-4 {
                self.haptic(dom, 0.05, 200.0, (r * 65535.0 / 40000.0).min(1.0));
            }
            self.last_rumble = r;
        }

        if f.hmd.valid == 0 {
            out.intents.presses = self.presses;
            return out;
        }
        // The head's height over the real floor (where its neck turns it), and its upright height
        // (in menus too: a new game starts in the character creator, the body in its mirror hung
        // from the head by it).
        let head_h = (v3(f.hmd.pos) + q(f.hmd.rot) * NECK).y - NECK.y;
        self.update_upright(c, head_h);
        // A loading screen takes the menu controls too: its "press to continue" wants the menu's
        // accept (and nothing should fire or move while it's up).
        let loading = inp.game.is_some_and(|g| g.flags & GS_LOADING != 0);
        if inp.menu_mode || inp.runtime_menu || loading {
            if std::env::var_os("RUSTVR_TRACE_GUN").is_some() && now - self.insane_logged > 0.5 {
                self.insane_logged = now;
                tracing::info!("gun trace: menu controls (game menu {}, runtime menu {}, loading {loading})", inp.menu_mode, inp.runtime_menu);
            }
            self.menu_controls(inp, &mut out);
            self.following = false;
            self.debug.following = false;
            out.intents.presses = self.presses;
            return out;
        }

        let hmd_pos = v3(f.hmd.pos);
        let hmd_rot = q(f.hmd.rot);
        let head_fwd = hmd_rot * Vec3::Z;
        let head_yaw = yaw_of(head_fwd);
        let game = inp.game;
        let gflags = game.map(|g| g.flags).unwrap_or(0);

        // ---------------- the gun in the firing hand
        let dc = &f.hands[dom];
        let oc = &f.hands[off];
        let rig = match game {
            Some(g) if g.flags & GS_HAS_WEAPON != 0 => g.rig,
            _ => default_rig(),
        };
        let rig_mode = inp.rig.is_some();
        let rigw = inp.rig.and_then(|r| r.primary_weapon);
        let busy = inp.rig.map(|r| r.hand_busy).unwrap_or([false; 2]);
        let targeting = inp.rig.map(|r| r.hand_targeting).unwrap_or([false; 2]);
        let has_weapon = if rig_mode { rigw.is_some() } else { game.is_none_or(|g| g.flags & GS_HAS_WEAPON != 0) };
        if rig.id != self.rig_id {
            self.rig_id = rig.id;
            self.mag = MagState::In;
            self.rack_start = None;
        }
        let full_reload = !rig_mode
            && c.input.manual_reload
            && c.input.reload_style == ReloadStyle::Full
            && rig.flags & RIG_HAS_MAG_WELL != 0
            && has_weapon;
        let mut handle_t = None;
        let mut weapon = None;
        let mut two = false;
        let mut have_mag = false;
        let mut mag_t = Vec3::ZERO;
        let mut mag_up = Vec3::Y;
        let mut dist_mag = 10.0;
        // No gun in hand (the game says so): nothing to place, aim or reload.
        if let Some(rw) = rigw {
            // The rig's gun: already placed, steadied by the support hand and kicked by recoil.
            let w2t = rw.weapon_to_tracking;
            two = rw.two_handed;
            let aim = w2t.transform_vector3(Vec3::Z).normalize_or(head_fwd);
            let so = w2t.transform_point3(rw.sight);
            let mz = w2t.transform_point3(rw.muzzle);
            let mut best = 10.0f32;
            for e in 0..2 {
                let v = v3(f.eyes[e].pos) - so;
                let along = v.dot(aim);
                let perp = v - aim * along;
                best = best.min(perp.length() + (along - 0.05).max(0.0) * 0.2);
            }
            let ads = best < 0.06 && aim.dot(head_fwd) > 0.9;
            let zoom = game.map(|g| g.game_zoom).filter(|z| *z > 0.0).unwrap_or(1.0);
            let scope = (c.scope.enabled && (c.scope.force || (ads && (zoom > 1.4 || rw.scoped))))
                .then(|| scope_view(c, w2t, aim, so, mz, if c.scope.force || zoom <= 1.4 { c.scope.magnification } else { zoom }, inp.aux_aspect));
            if w2t.is_finite() && so.is_finite() && aim.is_finite() && mz.is_finite() {
                let laser = rw.laser.map(|l| w2t.transform_point3(l));
                weapon = Some(WeaponPose { weapon_to_tracking: w2t, sight_origin: so, aim_dir: aim, muzzle: mz, two_handed: two, ads, sight_to_eye: best, zoom, scope, laser });
            }
        } else if dc.connected != 0 && has_weapon && !rig_mode {
            let w = &c.weapon;
            let grip_t = v3(dc.grip.pos);
            let ctrl = Mat4::from_rotation_translation(q(dc.aim.rot), grip_t);
            let rg = v3(rig.grip);
            let lg = v3(rig.foregrip);
            // weapon space -> controller: the rig's grip on the palm, barrel along the aim ray.
            let w2c = Mat4::from_translation(Vec3::from_array(w.offset))
                * Mat4::from_quat(Quat::from_euler(EulerRot::YXZ, w.yaw.to_radians(), w.pitch.to_radians(), w.roll.to_radians()))
                * Mat4::from_scale(Vec3::splat(w.scale))
                * Mat4::from_translation(-rg);
            let mut w2t = ctrl * w2c;

            mag_t = w2t.transform_point3(mag_well(&rig));
            mag_up = w2t.y_axis.truncate().normalize_or(Vec3::Y);
            have_mag = has_weapon && oc.connected != 0;
            if have_mag {
                dist_mag = v3(oc.grip.pos).distance(mag_t);
            }

            // Two-handed: the off hand grips near the foregrip -> the gun pivots around the firing
            // hand so its foregrip follows the off hand.
            if c.input.two_handed_aim && oc.connected != 0 && rig.flags & RIG_TWO_HANDED != 0 {
                let off_t = v3(oc.grip.pos);
                let fore_t = w2t.transform_point3(lg);
                let dist_fore = off_t.distance(fore_t);
                let gripping = oc.squeeze > 0.6;
                let at_mag = c.input.manual_reload && have_mag && dist_mag < MAG_RADIUS && dist_mag < dist_fore;
                if !gripping {
                    self.two_held = false;
                } else if !self.two_held && dist_fore < c.input.two_handed_grab_dist && !at_mag && self.mag == MagState::In {
                    self.two_held = true;
                    self.haptic(off, 0.04, 150.0, 0.6);
                }
                let cur = fore_t - grip_t;
                let want = off_t - grip_t;
                let (lc, lw) = (cur.length(), want.length());
                if self.two_held && lc > 0.05 && lw > 0.08 && lw < 0.9 {
                    let rot = Quat::from_rotation_arc(cur / lc, want / lw);
                    let pivot = Mat4::from_translation(grip_t) * Mat4::from_quat(rot) * Mat4::from_translation(-grip_t);
                    w2t = pivot * w2t;
                    two = true;
                }
            } else {
                self.two_held = false;
            }

            if rig.flags & RIG_HAS_CHARGING_HANDLE != 0 {
                handle_t = Some(w2t.transform_point3(v3(rig.charging_handle)));
            }

            // The game's own recoil (`rig::kick`), from the shots the game says it fired: its kicks
            // turn the gun in weapon space, which here is the game's (its viewmodel camera space,
            // say, so a view kick turns the gun about the eye as the flat game turns its camera).
            // Held in one hand it kicks harder, as on the rig.
            let ia = &c.interaction;
            if ia.game_recoil && !inp.held_kicks.is_empty() {
                let stance = crate::rig::kick::Stance { aimed: self.last_ads, crouched: gflags & GS_CROUCHED != 0 };
                let hold = if two {
                    crate::rig::kick::HoldScale::GAME
                } else {
                    crate::rig::kick::HoldScale { turn: ia.recoil_one_hand, push: 1.0 + (ia.recoil_one_hand - 1.0) * 0.5 }
                };
                for _ in 0..inp.game_shots.min(8) {
                    self.game_recoil.kick(inp.held_kicks, stance, hold);
                }
                self.game_recoil.update(inp.dt, inp.held_kicks);
                let (r, t) = self.game_recoil.transform(inp.held_kicks, ia.game_recoil_turn, ia.game_recoil_push);
                if std::env::var_os("RUSTVR_TRACE_GUN").is_some() && self.game_recoil.playing() > 0 {
                    let (axis, angle) = r.to_axis_angle();
                    tracing::info!("recoil trace: {} playing, turn {:.2} deg about {axis:.2?}, move {t:.3?}", self.game_recoil.playing(), angle.to_degrees());
                }
                // Never a jump: a kick that would move the grip more than 25 cm at once (a broken
                // pivot) is dropped.
                let kick = Mat4::from_rotation_translation(r, t);
                if kick.transform_point3(rg).distance(rg) < 0.25 {
                    w2t *= kick;
                } else if now - self.insane_logged > 1.0 {
                    self.insane_logged = now;
                    tracing::warn!("mechanics: dropped a recoil kick that would move the gun {:.2} m", kick.transform_point3(rg).distance(rg));
                }
                self.debug.game_kicks = self.game_recoil.playing();
            }

            // Shots follow the line of sight (through the optic or rear sight), parallel to the barrel.
            let (sight_w, sight_dir) = sight_line(&rig);
            let aim = w2t.transform_vector3(sight_dir).normalize_or(head_fwd);
            let so = w2t.transform_point3(sight_w);
            let mz = if rig.flags & RIG_HAS_MUZZLE != 0 { w2t.transform_point3(v3(rig.muzzle)) } else { so + aim * 0.6 };

            // Auto-ADS: the sight line passes close to an eye while pointing roughly where you look.
            let mut best = 10.0f32;
            for e in 0..2 {
                let v = v3(f.eyes[e].pos) - so;
                let along = v.dot(aim);
                let perp = v - aim * along;
                best = best.min(perp.length() + (along - 0.05).max(0.0) * 0.2);
            }
            // Hysteresis: in at 6 cm, out past 12 cm, and never mid-burst (a game's fire animation
            // nudges the gun, and with it the sights; dropping out of the aim pose then would pull
            // the gun off the eye, further out of it).
            let firing = dc.trigger > 0.5;
            let keep = if firing { 0.35 } else { 0.12 };
            let ads = (best < 0.06 && aim.dot(head_fwd) > 0.9) || (self.last_ads && best < keep && aim.dot(head_fwd) > 0.8) || dc.squeeze > 0.7;
            self.last_ads = ads;

            // Scope picture-in-picture: magnification from the game's own aim zoom.
            let zoom = game.map(|g| g.game_zoom).filter(|z| *z > 0.0).unwrap_or(1.0);
            let mag = if c.scope.force { c.scope.magnification } else { zoom };
            let scope = (c.scope.enabled && (c.scope.force || (ads && zoom > 1.4))).then(|| scope_view(c, w2t, aim, so, mz, mag, inp.aux_aspect));

            let sane = w2t.is_finite() && so.is_finite() && aim.is_finite() && mz.is_finite();
            if !sane && now - self.insane_logged > 1.0 {
                self.insane_logged = now;
                tracing::warn!(
                    "mechanics: the gun's pose isn't finite (w2t {}, sight {so:?}, aim {aim:?}); rig grip {:?} foregrip {:?} line {:?}/{:?} flags {:#x}; kicks {}",
                    w2t.is_finite(),
                    rig.grip,
                    rig.foregrip,
                    rig.sight_line_origin,
                    rig.sight_line_dir,
                    rig.flags,
                    self.game_recoil.playing()
                );
            }
            if sane {
                weapon = Some(WeaponPose {
                    weapon_to_tracking: w2t,
                    sight_origin: so,
                    aim_dir: aim,
                    muzzle: mz,
                    two_handed: two,
                    ads,
                    sight_to_eye: best,
                    zoom,
                    scope,
                    laser: None,
                });
            }
        } else {
            self.two_held = false;
        }

        // ---------------- controls
        let mut held = 0u64;
        let dz = c.input.stick_deadzone;
        let (mut mx, mut my) = radial_deadzone(oc.stick[0], oc.stick[1], dz, STICK_RIM);
        if c.comfort.move_relative == MoveRelative::OffHand && oc.connected != 0 {
            let d = yaw_of(q(oc.aim.rot) * Vec3::Z) - head_yaw;
            let (s, co) = d.sin_cos();
            (mx, my) = (mx * co + my * s, -mx * s + my * co);
        }
        let mut move_xy = Vec2::new(mx, my);

        // Roomscale: walk the game body after the tracked head while the stick is idle (the view
        // itself never moves because of this: the adapter absorbs the body motion).
        let room = game.map(|g| Vec2::from_array(g.tracking.room_offset)).unwrap_or(Vec2::ZERO);
        let look_yaw = game.map(|g| g.tracking.look_yaw).unwrap_or(head_yaw);
        let len = room.length();
        let stick_idle = mx.abs() < 0.05 && my.abs() < 0.05;
        if !c.comfort.roomscale || !stick_idle || gflags & GS_SCRIPTED != 0 || game.is_none() || inp.riding {
            self.following = false;
        } else if !self.following && len > 0.20 {
            self.following = true;
        } else if self.following && len < 0.05 {
            self.following = false;
        }
        if self.following && len > 1e-4 {
            let (ox, oz) = (room.x, room.y);
            let fwd = ox * look_yaw.sin() + oz * look_yaw.cos();
            let right = ox * look_yaw.cos() - oz * look_yaw.sin();
            let m = (len * 2.5).clamp(0.32, 1.0);
            move_xy = Vec2::new(right / len * m, fwd / len * m);
            out.tracking.flags |= TRK_FOLLOW;
        }
        self.debug.following = self.following;
        self.debug.room_len = len;
        out.intents.move_xy = move_xy.to_array();

        // Turning: firing-hand stick X (on a mount it steers the mount, which turns the view).
        self.turn(c, if inp.riding { 0.0 } else { dc.stick[0] }, now, inp.dt);
        out.tracking.turn_total = self.turn_total;

        // Firing-hand stick Y: next / previous weapon.
        let ty = deadzone(dc.stick[1], 0.25);
        let ydir = if ty > 0.7 { 1 } else if ty < -0.7 { -1 } else { 0 };
        if ydir != 0 && ydir != self.last_stick_y_dir {
            if !rig_mode {
                self.press(if ydir > 0 { Action::NextWeapon } else { Action::PrevWeapon });
            } else if ydir < 0 && !inp.riding {
                // Guns are in your hands and holsters; the stick's down flick toggles crouch.
                self.press(Action::Crouch);
                self.crouch_by_phys = false;
            }
        }
        if ty.abs() < 0.3 {
            self.last_stick_y_dir = 0;
        } else if ydir != 0 {
            self.last_stick_y_dir = ydir;
        }

        // In rig mode guns fire in the rig (the game counts their shots), so the trigger of an
        // empty hand is free to use things.
        let fire = if dc.trigger > 0.25 && !rig_mode { (dc.trigger * 1.2).min(1.0) } else { 0.0 };
        out.intents.fire = fire;
        if fire > 0.0 {
            held |= Action::Fire.bit();
        }
        if rig_mode {
            self.rig_controls(inp, dom, busy, targeting, &mut held);
        }
        // The hand the game picks what to use along (a door, a person: what its trigger uses, its
        // card over the hand): a free one (holding, pulling, casting nothing) put up in front, the
        // one reached out furthest, the other taking over only once it's a good way further. With
        // none, the eyes'.
        let ahead = Vec3::new(head_fwd.x, 0.0, head_fwd.z).normalize_or(Vec3::Z);
        let out_by = |h: usize| (v3(f.hands[h].aim.pos) - hmd_pos).dot(ahead);
        let was = self.point_hand;
        let put_up = |h: usize| out_by(h) > if was == Some(h) { POINT_OUT - POINT_BACK } else { POINT_OUT };
        let free = |h: usize| rig_mode && f.hands[h].connected != 0 && f.hands[h].aim.valid != 0 && !busy[h] && !targeting[h] && !inp.spell_hands[h] && !inp.riding && put_up(h);
        self.point_hand = match (free(0), free(1)) {
            (false, false) => None,
            (true, false) => Some(0),
            (false, true) => Some(1),
            (true, true) => match was {
                Some(h) if out_by(h ^ 1) < out_by(h) + POINT_SWITCH => Some(h),
                _ => Some(if out_by(0) >= out_by(1) { 0 } else { 1 }),
            },
        };
        if let Some(h) = self.point_hand {
            out.intents.flags |= if h == 0 { INT_POINT_LEFT } else { INT_POINT_RIGHT };
        }
        // Firing hand. A: reload, or with full manual reloads, eject the magazine (and rack the gun if a
        // new one is seated but the charging handle is out of reach).
        if self.btn_a[dom].tapped && !rig_mode {
            match (full_reload, self.mag) {
                (false, _) => self.press(Action::Reload),
                (true, MagState::In) => {
                    self.mag = MagState::Out;
                    self.debug.ejects += 1;
                    self.haptic(dom, 0.05, 120.0, 0.6);
                }
                (true, MagState::Seated) => self.finish_reload(off, dom),
                (true, _) => {}
            }
        }
        if self.btn_a[dom].long_start && !rig_mode {
            self.press(Action::ReloadHold);
        }
        if self.btn_b[dom].down && !rig_mode {
            held |= Action::Jump.bit();
        }
        if self.stick_fire[dom] && !inp.riding {
            self.press(Action::Crouch);
            self.crouch_by_phys = false;
        }
        // Off hand.
        if self.btn_a[off].tapped && !rig_mode {
            self.press(Action::Use);
        }
        if self.btn_a[off].long_fired && self.btn_a[off].down && !rig_mode {
            held |= Action::UseHold.bit();
        }
        if self.btn_b[off].tapped && !(rig_mode && busy[off]) {
            self.press(Action::Flashlight);
        }
        if self.btn_b[off].long_fired && self.btn_b[off].down && !(rig_mode && busy[off]) {
            held |= Action::FlashlightHold.bit();
        }
        if self.stick_fire[off] {
            self.sprint_toggle = !self.sprint_toggle;
        }
        if my.abs() < 0.2 && mx.abs() < 0.2 {
            self.sprint_toggle = false;
        }
        if self.sprint_toggle {
            held |= Action::Sprint.bit();
        }
        if self.btn_menu[off].pressed() || self.btn_menu[dom].pressed() {
            self.press(Action::Menu);
        }
        if self.btn_trig[off].pressed() && !(c.input.two_handed_aim && two) && !rig_mode {
            self.press(Action::Throw);
        }

        // Physical crouch: ducking in real life crouches the game body (fits under obstacles,
        // quieter footsteps) while the view height stays the real head height.
        {
            let game_crouched = gflags & GS_CROUCHED != 0;
            let eye_above_feet = game.map(|g| g.eye_above_feet).unwrap_or(self.stand_eye);
            if !game_crouched && eye_above_feet > 1.0 && eye_above_feet < 2.5 {
                self.stand_eye += (eye_above_feet - self.stand_eye) * 0.05;
            }
            // The head's upright height (over the real floor, `update_upright`): the view's height
            // is the head's over it (`upright_offset`), and ducking a good way under it crouches.
            let h = head_h;
            let thr = c.input.stealth_crouch_amount.clamp(0.05, 0.4);
            let was = self.phys_crouch;
            if !self.phys_crouch && h < self.stand_h - thr {
                self.phys_crouch = true;
            } else if self.phys_crouch && h > self.stand_h - thr + (thr * 0.5).min(0.06) {
                self.phys_crouch = false;
            }
            if self.phys_crouch != was {
                tracing::info!("physical crouch: {} (the head {:.2} m under upright, {:.2} m)", if self.phys_crouch { "crouched" } else { "stood up" }, self.stand_h - h, self.stand_h);
            }
            // (Leaning down on a mount's neck isn't sneaking. Swimming, a head ducked is a face put
            // into the water, not a crouch. Nor could a crouch be kept in step there: a swimmer's
            // game body isn't crouched, so one pressed in the water would still be on at the shore.)
            let swimming = game.is_some_and(|g| g.water.flags & WATER_SWIMMING != 0);
            if c.input.physical_crouch && !c.comfort.seated && game.is_some() && !inp.riding && !swimming {
                if self.phys_crouch != was {
                    self.crouch_pending = if self.phys_crouch { 1 } else if self.crouch_by_phys { 0 } else { -1 };
                }
                if self.crouch_pending >= 0 && now > self.crouch_cooldown {
                    if game_crouched != (self.crouch_pending == 1) {
                        self.press(Action::Crouch);
                        self.crouch_cooldown = now + 0.6;
                        self.crouch_by_phys = self.crouch_pending == 1;
                    }
                    self.crouch_pending = -1;
                }
            } else if self.crouch_by_phys && game.is_some() && (c.comfort.seated || !c.input.physical_crouch) && now > self.crouch_cooldown {
                // Seated play turned on (or the physical crouch off) in a crouch the head made: the
                // game stands back up (sat, the head is upright now).
                if game_crouched {
                    self.press(Action::Crouch);
                    self.crouch_cooldown = now + 0.6;
                }
                self.crouch_by_phys = false;
                self.crouch_pending = -1;
            }
            // The real head already went down: don't lower the view a second time by the game's crouch
            // (by as much as it says its eye went down: a game that doesn't say does that itself).
            // Only for a crouch the real one made: crouching for real in the stick's, the view goes
            // lower still, never jumping back up.
            let comp = if c.input.physical_crouch && !c.comfort.seated && game_crouched && self.phys_crouch && self.crouch_by_phys && eye_above_feet > 0.3 {
                (self.stand_eye - eye_above_feet).max(0.0)
            } else {
                0.0
            };
            out.tracking.height_comp = comp;
            if c.input.physical_crouch && !c.comfort.seated && self.phys_crouch {
                out.intents.crouch = (self.stand_h - h).max(0.0);
            }
            self.debug.phys_crouch = self.phys_crouch;
            self.debug.game_crouched = game_crouched;
            self.debug.height_comp = comp;
        }

        if full_reload && have_mag {
            self.full_reload(inp, off, dom, hmd_pos, hmd_rot, dist_mag, handle_t, weapon.map(|w| w.aim_dir).unwrap_or(head_fwd));
        } else if c.input.manual_reload && have_mag {
            // Magazine reload: bring the off hand up into the magazine well (squeeze there, or slap up).
            let rel = v3(oc.grip.pos) - mag_t;
            let up_speed = match self.prev_mag_rel {
                Some(prev) if inp.dt > 0.0 => ((rel - prev) / inp.dt).dot(mag_up),
                _ => 0.0,
            };
            self.prev_mag_rel = Some(rel);
            let inside = dist_mag < MAG_RADIUS;
            if inside && !self.in_mag {
                self.haptic(off, 0.015, 180.0, 0.3);
            }
            self.in_mag = inside;
            if inside && now > self.reload_cooldown && !two && (self.btn_grip[off].pressed() || up_speed > 0.9) {
                self.press(Action::Reload);
                self.reload_cooldown = now + 1.0;
                self.debug.reloads += 1;
                self.haptic(off, 0.08, 90.0, 0.9);
                self.haptic(dom, 0.05, 90.0, 0.5);
            }
            self.debug.up_speed = up_speed;
        } else {
            self.prev_mag_rel = None;
        }
        self.debug.dist_mag = dist_mag;

        // Gestures: grip inside a body zone.
        self.debug.watch = false;
        if c.input.gestures {
            for h in 0..2 {
                if f.hands[h].connected == 0 {
                    continue;
                }
                let (local, full) = head_local(hmd_pos, hmd_rot, v3(f.hands[h].grip.pos));
                let z = zone_of(local, full);
                if z != self.last_zone[h] && z != Zone::None {
                    self.haptic(h, 0.02, 160.0, 0.4);
                }
                self.last_zone[h] = z;
                if !self.btn_grip[h].pressed() || z == Zone::None || busy[h] || targeting[h] {
                    continue;
                }
                // Aiming down the sights puts the firing hand near the face with the grip squeezed:
                // that's not the face gesture.
                if h == dom && weapon.is_some_and(|w| w.ads) {
                    continue;
                }
                // During a manual reload the hip and chest are magazine pouches, not gestures.
                if h == off && self.mag != MagState::In && matches!(z, Zone::Hip | Zone::Chest) {
                    continue;
                }
                let (action, i) = match z {
                    Zone::Face => (Action::GestureFace, 0),
                    Zone::Mouth => (Action::GestureMouth, 5),
                    Zone::HeadTop => (Action::GestureHeadTop, 1),
                    Zone::Chest => (Action::GestureChest, 2),
                    Zone::Shoulder => (Action::GestureShoulder, 3),
                    Zone::Hip => (Action::GestureHip, 4),
                    Zone::None => unreachable!(),
                };
                self.press(action);
                self.debug.gestures[i] += 1;
                self.haptic(h, 0.06, 120.0, 0.8);
            }
            // Wiping the eyes: an open hand swept sideways across them.
            for h in 0..2 {
                let hand = &f.hands[h];
                if hand.connected == 0 || hand.squeeze > 0.5 || busy[h] || now < self.wipe_cooldown {
                    continue;
                }
                let (local, full) = head_local(hmd_pos, hmd_rot, v3(hand.grip.pos));
                let across = (hmd_rot.inverse() * v3(hand.vel)).x.abs();
                if zone_of(local, full) == Zone::Face && across > WIPE_SPEED {
                    self.press(Action::GestureWipe);
                    self.debug.gestures[6] += 1;
                    self.wipe_cooldown = now + WIPE_COOLDOWN;
                    self.haptic(h, 0.05, 140.0, 0.5);
                }
            }
            // Watch: off-hand wrist held in front of the face with the inside of the wrist up.
            if oc.connected != 0 {
                let (_, full) = head_local(hmd_pos, hmd_rot, v3(oc.grip.pos));
                let up = q(oc.grip.rot) * Vec3::Y;
                let to_head = (hmd_pos - v3(oc.grip.pos)).normalize_or_zero();
                let facing = up.dot(to_head);
                let watch = full.z > 0.15 && full.z < 0.55 && full.y < 0.0 && full.y > -0.4 && full.x.abs() < 0.3 && facing > 0.6 && oc.squeeze < 0.5;
                if watch {
                    held |= Action::Watch.bit();
                }
                self.debug.watch = watch;
            }
        }

        // Melee: a fast strike forward with either hand (not the firing hand while aiming).
        if c.input.melee_swing && now > self.melee_cooldown {
            let aiming = weapon.is_some_and(|w| w.ads);
            for h in 0..2 {
                let hand = &f.hands[h];
                if hand.connected == 0 || (h == dom && aiming) {
                    continue;
                }
                let v = v3(hand.vel);
                let speed = v.length();
                if speed > c.input.melee_speed && v.dot(head_fwd) > 0.6 * speed {
                    self.press(Action::Melee);
                    self.debug.melees += 1;
                    self.melee_cooldown = now + 0.5;
                    self.haptic(h, 0.05, 100.0, 0.9);
                    break;
                }
            }
        }
        // Ability flicks: off-hand grip held away from the body, then a fast flick up, down, left or
        // right (relative to where you face). Not while that hand is busy with the gun.
        if c.input.ability_flicks && now > self.flick_cooldown && oc.connected != 0 && oc.squeeze > 0.7 && !two && !busy[off] && !targeting[off] && self.mag == MagState::In && dist_mag > MAG_RADIUS * 1.5 {
            let (local, full) = head_local(hmd_pos, hmd_rot, v3(oc.grip.pos));
            if zone_of(local, full) == Zone::None {
                let v = Quat::from_rotation_y(-head_yaw) * v3(oc.vel);
                let speed = v.length();
                let pick = if v.y.abs() > 0.7 * speed { Some(if v.y > 0.0 { 0 } else { 1 }) } else if v.x.abs() > 0.7 * speed { Some(if v.x < 0.0 { 2 } else { 3 }) } else { None };
                if let (true, Some(i)) = (speed > c.input.flick_speed, pick) {
                    self.press([Action::AbilityUp, Action::AbilityDown, Action::AbilityLeft, Action::AbilityRight][i]);
                    self.debug.abilities[i] += 1;
                    self.flick_cooldown = now + 0.4;
                    self.haptic(off, 0.05, 140.0, 0.8);
                }
            }
        }
        match self.mag {
            MagState::Out => out.intents.flags |= INT_MAG_OUT,
            MagState::Held => out.intents.flags |= INT_MAG_OUT | INT_HOLDING_MAG,
            _ => {}
        }
        self.debug.mag = self.mag;

        if let Some(w) = &weapon {
            out.intents.flags |= INT_WEAPON;
            if w.two_handed {
                out.intents.flags |= INT_TWO_HANDED;
            }
            if w.ads {
                out.intents.flags |= INT_ADS;
                held |= Action::AimDownSights.bit();
            }
            out.intents.weapon_to_tracking = w.weapon_to_tracking.to_cols_array();
            out.intents.sight_origin = w.sight_origin.to_array();
            out.intents.aim_dir = w.aim_dir.to_array();
            out.intents.muzzle = w.muzzle.to_array();
            out.intents.zoom = w.zoom;
            out.intents.sight_to_eye = w.sight_to_eye;
            if let Some(s) = &w.scope {
                out.intents.flags |= INT_SCOPE;
                out.intents.scope_cam = Pose { pos: s.cam_pos.to_array(), valid: 1, rot: s.cam_rot.to_array() };
                out.intents.scope_vfov_deg = s.vfov_deg;
                out.intents.scope_aspect = s.aspect;
                out.intents.scope_every = c.scope.every.max(1);
            }
            self.debug.two_handed = w.two_handed;
            self.debug.ads = w.ads;
            self.debug.sight_to_eye = w.sight_to_eye;
        } else {
            self.debug.two_handed = false;
            self.debug.ads = false;
        }
        if std::env::var_os("RUSTVR_TRACE_GUN").is_some() && now - self.insane_logged > 0.5 {
            self.insane_logged = now;
            tracing::info!(
                "gun trace: has_weapon {has_weapon} rig_mode {rig_mode} connected {} weapon {} sight_to_eye {:.3} ads {} flags {:#x} trigger {:.2}",
                dc.connected,
                weapon.is_some(),
                weapon.as_ref().map_or(-1.0, |w| w.sight_to_eye),
                weapon.as_ref().is_some_and(|w| w.ads),
                gflags,
                dc.trigger
            );
        }
        out.weapon = weapon;
        out.intents.held = held;
        out.intents.presses = self.presses;
        // A fingertip on what's held in the hands as the game plays on (a book's pages): where on the
        // HUD texture, and whether it's pressing. The game's own mouse isn't it: its button would
        // swing or cast.
        if let Some(cur) = inp.cursor {
            out.intents.flags |= INT_CURSOR;
            out.intents.cursor_uv = cur.uv.to_array();
            if cur.down {
                out.intents.flags |= INT_CURSOR_DOWN;
            }
        }
        out
    }

    /// The buttons the rig leaves to the game. A hand holding a gun keeps A (eject) and B (slide
    /// release) for the rig; an empty hand's A/X jumps and its trigger uses what you look at (when
    /// it isn't reaching for an object). Holding the dominant B opens the weapon wheel.
    fn rig_controls(&mut self, inp: &MechInput, dom: usize, busy: [bool; 2], targeting: [bool; 2], held: &mut u64) {
        let gun_hand = |h: usize| inp.rig.and_then(|r| r.primary_weapon).is_some() && busy[h];
        for h in 0..2 {
            if self.btn_a[h].down && !gun_hand(h) {
                *held |= Action::Jump.bit();
            }
            // (On a mount the game's "use" gets off, which riding does by a held trigger.)
            if self.btn_trig[h].pressed() && !busy[h] && !targeting[h] && !inp.spell_hands[h] && !inp.riding {
                self.press(Action::Use);
            }
            if self.btn_trig[h].down && !busy[h] && !targeting[h] && !inp.spell_hands[h] && !inp.riding && inp.now - self.btn_trig[h].t0 > LONG_PRESS {
                *held |= Action::UseHold.bit();
            }
        }
        if self.btn_b[dom].long_fired && self.btn_b[dom].down && !gun_hand(dom) {
            *held |= Action::WeaponWheel.bit();
        }
    }

    /// Full manual reload: after the eject, grab a magazine at the hip or chest, seat it in the well,
    /// then rack the charging handle (pull it back towards you).
    #[allow(clippy::too_many_arguments)]
    fn full_reload(&mut self, inp: &MechInput, off: usize, dom: usize, hmd_pos: Vec3, hmd_rot: Quat, dist_mag: f32, handle: Option<Vec3>, aim: Vec3) {
        let oc = &inp.frame.hands[off];
        let hand = v3(oc.grip.pos);
        match self.mag {
            MagState::In => {}
            MagState::Out => {
                let (local, full) = head_local(hmd_pos, hmd_rot, hand);
                if self.btn_grip[off].pressed() && matches!(zone_of(local, full), Zone::Hip | Zone::Chest) {
                    self.mag = MagState::Held;
                    self.haptic(off, 0.04, 160.0, 0.6);
                }
            }
            MagState::Held => {
                if oc.squeeze < 0.4 {
                    self.mag = MagState::Out; // dropped it
                    self.haptic(off, 0.02, 80.0, 0.3);
                } else if dist_mag < SEAT_RADIUS {
                    self.haptic(off, 0.08, 90.0, 0.9);
                    self.haptic(dom, 0.05, 90.0, 0.5);
                    if handle.is_some() {
                        self.mag = MagState::Seated;
                    } else {
                        self.finish_reload(off, dom);
                    }
                }
            }
            MagState::Seated => {
                let Some(h) = handle else {
                    self.finish_reload(off, dom);
                    return;
                };
                match self.rack_start {
                    None => {
                        if self.btn_grip[off].pressed() && hand.distance(h) < HANDLE_RADIUS {
                            self.rack_start = Some(hand);
                            self.haptic(off, 0.02, 200.0, 0.5);
                        }
                    }
                    Some(start) => {
                        if oc.squeeze < 0.4 {
                            self.rack_start = None;
                        } else if (start - hand).dot(aim) > RACK_PULL {
                            self.haptic(off, 0.06, 200.0, 1.0);
                            self.finish_reload(off, dom);
                        }
                    }
                }
            }
        }
    }

    fn finish_reload(&mut self, _off: usize, dom: usize) {
        self.press(Action::Reload);
        self.mag = MagState::In;
        self.rack_start = None;
        self.debug.reloads += 1;
        self.haptic(dom, 0.04, 150.0, 0.7);
    }

    /// The view turned by the turning stick's X (`x`, before its dead zone): a step for each flick,
    /// or smoothly, as the comfort settings say.
    fn turn(&mut self, c: &Settings, x: f32, now: f64, dt: f32) {
        let tx = deadzone(x, 0.25);
        match c.comfort.turn_mode {
            TurnMode::Snap => {
                if tx.abs() > 0.75 && now > self.snap_cooldown {
                    self.turn_total = wrap(self.turn_total + c.comfort.snap_angle.to_radians() * tx.signum());
                    self.snap_cooldown = now + 0.5;
                }
                if tx.abs() < 0.25 {
                    self.snap_cooldown = self.snap_cooldown.min(now);
                }
            }
            TurnMode::Smooth => {
                if tx != 0.0 {
                    self.turn_total = wrap(self.turn_total + c.comfort.smooth_turn_speed.to_radians() * tx * dt);
                }
            }
        }
    }

    /// Menus on the panel: the stick and buttons navigate, the laser drives the cursor. In one the
    /// player can turn in (a conversation, [`GS_MENU_TURNS`]) the stick's left and right turn them.
    fn menu_controls(&mut self, inp: &MechInput, out: &mut MechOutput) {
        let f = inp.frame;
        let dom = inp.settings.dominant();
        let off = dom ^ 1;
        out.intents.flags |= INT_MENU_MODE;
        out.intents.move_xy = [deadzone(f.hands[off].stick[0], 0.3), deadzone(f.hands[off].stick[1], 0.3)];
        let (sx, sy) = (f.hands[dom].stick[0], f.hands[dom].stick[1]);
        let turns = !inp.runtime_menu && inp.game.is_some_and(|g| g.flags & GS_MENU_TURNS != 0);
        let mut held = 0u64;
        if sy > 0.6 {
            held |= Action::MenuUp.bit();
        }
        if sy < -0.6 {
            held |= Action::MenuDown.bit();
        }
        if turns {
            self.turn(inp.settings, sx, inp.now, inp.dt);
            out.tracking.turn_total = self.turn_total;
        } else {
            if sx < -0.6 {
                held |= Action::MenuLeft.bit();
            }
            if sx > 0.6 {
                held |= Action::MenuRight.bit();
            }
        }
        if !inp.runtime_menu {
            // A on either hand accepts; so does a trigger pull (the natural "continue").
            let trigger = (0..2).any(|h| self.btn_trig[h].pressed() && !inp.sculpting[h]) && inp.cursor.is_none();
            if self.btn_a[dom].pressed() || self.btn_a[off].pressed() || trigger {
                self.press(Action::MenuAccept);
            }
            if self.btn_b[dom].pressed() || self.btn_b[off].pressed() {
                self.press(Action::MenuBack);
            }
            // The dominant stick scrolls the game's menu, as its mouse wheel would.
            out.intents.scroll = deadzone(sy, 0.2) * inp.settings.ui.menu_scroll;
            if let Some(cur) = inp.cursor {
                out.intents.flags |= INT_CURSOR;
                out.intents.cursor_uv = cur.uv.to_array();
                if cur.down {
                    out.intents.flags |= INT_CURSOR_DOWN;
                }
            }
        }
        if self.btn_menu[dom].pressed() || self.btn_menu[off].pressed() {
            self.press(Action::Menu);
        }
        out.intents.held = held;
    }
}

/// The scope's camera (just past the muzzle, along the sight line) and its lens behind the optic.
fn scope_view(c: &Settings, w2t: Mat4, aim: Vec3, so: Vec3, mz: Vec3, mag: f32, aux_aspect: f32) -> ScopeView {
    let x = w2t.x_axis.truncate().normalize_or(Vec3::X);
    let y = aim.cross(x).normalize_or(Vec3::Y);
    let x = y.cross(aim);
    let cam_rot = Quat::from_mat3(&Mat3::from_cols(x, y, aim));
    let lens_angle = 2.0 * (c.scope.lens_size / c.scope.eye_relief.max(0.03)).atan();
    let square_fov = lens_angle.to_degrees() / mag.max(1.0);
    let aspect = if aux_aspect > 0.0 { aux_aspect } else { 1.0 };
    let sq = aspect.min(1.0);
    let vfov = (2.0 * ((square_fov.to_radians() * 0.5).tan() / sq).atan()).to_degrees();
    ScopeView {
        cam_pos: mz + aim * 0.06,
        cam_rot,
        vfov_deg: vfov.clamp(0.5, 60.0),
        aspect,
        lens_pos: so - aim * c.scope.lens_back,
        lens_right: x,
        lens_up: y,
        lens_radius: c.scope.lens_size,
    }
}

/// A point relative to the head: in a yaw-only head frame, and in the full head frame.
fn head_local(head_pos: Vec3, head_rot: Quat, p: Vec3) -> (Vec3, Vec3) {
    let d = p - head_pos;
    let full = head_rot.inverse() * d;
    let yaw = yaw_of(head_rot * Vec3::Z);
    let yaw_only = Quat::from_rotation_y(-yaw) * d;
    (yaw_only, full)
}

fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let r = a.rem_euclid(t);
    if r > std::f32::consts::PI { r - t } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::sim::pose;
    use bytemuck::Zeroable;

    #[test]
    fn the_stick_counts_from_its_deadzone_to_its_rim_in_any_direction() {
        // Inside the deadzone: nothing.
        assert_eq!(radial_deadzone(0.05, 0.03, 0.08, STICK_RIM), (0.0, 0.0));
        // From its edge, a gradual rise; all the way at the rim and past it.
        let (_, y) = radial_deadzone(0.0, 0.08 + (STICK_RIM - 0.08) * 0.5, 0.08, STICK_RIM);
        assert!((y - 0.5).abs() < 1e-5);
        assert!((radial_deadzone(0.0, -STICK_RIM, 0.08, STICK_RIM).1 + 1.0).abs() < 1e-5);
        assert!((radial_deadzone(0.0, 1.0, 0.08, STICK_RIM).1 - 1.0).abs() < 1e-5);
        // A diagonal keeps its direction (no axis snapped off).
        let (x, y) = radial_deadzone(0.3, 0.6, 0.08, STICK_RIM);
        assert!((x / y - 0.5).abs() < 1e-5);
    }

    fn standing() -> HmdFrame {
        let mut f = HmdFrame { index: 1, dt: 1.0 / 90.0, user_present: true, ..Default::default() };
        f.hmd = pose(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY);
        f.eyes = [pose(Vec3::new(-0.032, 1.7, 0.0), Quat::IDENTITY), pose(Vec3::new(0.032, 1.7, 0.0), Quat::IDENTITY)];
        for h in 0..2 {
            f.hands[h].connected = 1;
            let x = if h == 0 { -0.25 } else { 0.25 };
            f.hands[h].grip = pose(Vec3::new(x, 1.1, 0.25), Quat::IDENTITY);
            f.hands[h].aim = f.hands[h].grip;
        }
        f
    }

    fn game() -> GameState {
        let mut g = GameState::zeroed();
        g.flags = GS_IN_GAME | GS_HAS_WEAPON;
        g.rig = default_rig();
        g.game_zoom = 1.0;
        g.eye_above_feet = 1.7;
        g
    }

    fn run(m: &mut Mechanics, f: &HmdFrame, g: &GameState, s: &Settings, t: f64) -> MechOutput {
        run_firing(m, f, g, s, t, 0, &[])
    }

    fn run_firing(m: &mut Mechanics, f: &HmdFrame, g: &GameState, s: &Settings, t: f64, game_shots: u32, held_kicks: &[KickDesc]) -> MechOutput {
        m.update(&MechInput {
            frame: f,
            game: Some(g),
            settings: s,
            now: t,
            dt: 1.0 / 90.0,
            menu_mode: false,
            runtime_menu: false,
            cursor: None,
            aux_aspect: 1.0,
            rumble: None,
            rig: None,
            spell_hands: [false; 2],
            game_shots,
            held_kicks,
            riding: false,
            sculpting: [false; 2],
        })
    }

    #[test]
    fn the_games_shots_kick_the_gun_with_its_own_recoil_and_it_settles() {
        // One view kick: 2 degrees of muzzle climb at once, gone over half a second.
        let mut k = KickDesc::zeroed();
        k.object = KICK_OBJECT_HELD;
        k.layer = KICK_LAYER_VIEW;
        k.when = KICK_HIP | KICK_AIMED | KICK_STANDING | KICK_CROUCHED | KICK_FIRST | KICK_FOLLOW | KICK_LONG;
        k.speed = 1.0;
        k.length = 0.5;
        let v = 2f32.to_radians();
        k.channels[KICK_PITCH].count = 2;
        k.channels[KICK_PITCH].keys[0] = [0.0, v, -v / 0.5, -v / 0.5];
        k.channels[KICK_PITCH].keys[1] = [0.5, 0.0, -v / 0.5, -v / 0.5];
        let kicks = [k];
        let (mut m, s, f, g) = (Mechanics::default(), Settings::default(), standing(), game());
        let still = run(&mut m, &f, &g, &s, 0.0).weapon.unwrap().aim_dir;
        let kicked = run_firing(&mut m, &f, &g, &s, 0.011, 1, &kicks).weapon.unwrap().aim_dir;
        let climb = (kicked.y - still.y).asin().to_degrees();
        // Held in one hand (the off hand isn't on the foregrip): three times the game's kick.
        let want = 2.0 * s.interaction.recoil_one_hand;
        assert!((climb - want).abs() < 0.5, "the muzzle climbs with the kick: {climb:.2} degrees, want {want}");
        let mut t = 0.022;
        let mut last = kicked;
        for _ in 0..60 {
            last = run_firing(&mut m, &f, &g, &s, t, 0, &kicks).weapon.unwrap().aim_dir;
            t += 1.0 / 90.0;
        }
        assert!(last.angle_between(still) < 0.2f32.to_radians(), "and settles once it's over");
        // No kicks for the gun, or the game's recoil turned off: no climb.
        let mut off = s.clone();
        off.interaction.game_recoil = false;
        let mut m2 = Mechanics::default();
        let none = run_firing(&mut m2, &f, &g, &off, 0.0, 1, &kicks).weapon.unwrap().aim_dir;
        assert!(none.angle_between(still) < 1e-4);
    }

    #[test]
    fn both_sticks_together_swap_the_view_and_a_lone_click_still_crouches() {
        let (mut m, s, g) = (Mechanics::default(), Settings::default(), game());
        let dom = s.dominant();
        let mut f = standing();
        let mut t = 0.0;
        let mut step = |m: &mut Mechanics, f: &HmdFrame, n: usize| {
            let mut out = None;
            for _ in 0..n {
                out = Some(run(m, f, &g, &s, t));
                t += 1.0 / 90.0;
            }
            out.unwrap()
        };
        let crouches = |o: &MechOutput| o.intents.presses[Action::Crouch as usize];
        let base = crouches(&step(&mut m, &f, 2));
        // Both clicked, a frame apart: the view swaps, nothing crouches or sprints.
        f.hands[dom].buttons |= CTRL_STICK_CLICK;
        step(&mut m, &f, 1);
        f.hands[dom ^ 1].buttons |= CTRL_STICK_CLICK;
        let o = step(&mut m, &f, 30);
        assert!(m.framework_view, "both sticks: the framework view");
        assert_eq!(crouches(&o), base, "and no crouch");
        assert!(!m.sprint_toggle, "and no sprint");
        f.hands[0].buttons = 0;
        f.hands[1].buttons = 0;
        step(&mut m, &f, 5);
        f.hands[0].buttons = CTRL_STICK_CLICK;
        f.hands[1].buttons = CTRL_STICK_CLICK;
        step(&mut m, &f, 5);
        assert!(!m.framework_view, "again: back to the game");
        f.hands[0].buttons = 0;
        f.hands[1].buttons = 0;
        step(&mut m, &f, 5);
        // A lone click on the dominant stick: a crouch, a moment later.
        let before = crouches(&step(&mut m, &f, 1));
        f.hands[dom].buttons = CTRL_STICK_CLICK;
        let o = step(&mut m, &f, 1);
        assert_eq!(crouches(&o), before, "not yet: it might be half of the pair");
        let o = step(&mut m, &f, 15);
        assert_eq!(crouches(&o), before.wrapping_add(1), "a lone click crouches");
        assert!(!m.framework_view);
    }

    #[test]
    fn gun_sits_in_the_firing_hand() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let f = standing();
        let out = run(&mut m, &f, &game(), &s, 0.0);
        let w = out.weapon.unwrap();
        let grip_world = w.weapon_to_tracking.transform_point3(Vec3::from_array(default_rig().grip));
        assert!(grip_world.abs_diff_eq(Vec3::new(0.25, 1.1, 0.25), 1e-4), "rig grip on the palm: {grip_world:?}");
        assert!(w.aim_dir.abs_diff_eq(Vec3::Z, 1e-4), "barrel along the aim ray");
        assert!(!w.ads, "held at the hip");
        assert_eq!(out.intents.flags & INT_WEAPON, INT_WEAPON);
    }

    #[test]
    fn sights_at_the_eye_aim_down_sights() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        // Put the right hand so the sight line passes through the right eye.
        let rig = default_rig();
        let sight_local = Vec3::new(rig.sight[0], rig.sight[1], rig.grip[2] - 0.05) - Vec3::from_array(rig.grip);
        let eye = Vec3::from_array(f.eyes[1].pos);
        let hand = eye - Vec3::new(sight_local.x, sight_local.y, 0.0) + Vec3::new(0.0, 0.0, 0.2);
        f.hands[1].grip = pose(hand, Quat::IDENTITY);
        f.hands[1].aim = f.hands[1].grip;
        let out = run(&mut m, &f, &game(), &s, 0.0);
        let w = out.weapon.unwrap();
        assert!(w.sight_to_eye < 0.06, "{}", w.sight_to_eye);
        assert!(w.ads);
        assert!(out.intents.held & Action::AimDownSights.bit() != 0);
    }

    #[test]
    fn foregrip_grab_pivots_the_gun() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        let g = game();
        let w0 = run(&mut m, &f, &g, &s, 0.0).weapon.unwrap();
        let fore = w0.weapon_to_tracking.transform_point3(Vec3::from_array(g.rig.foregrip));
        // Off hand grabs the foregrip, then moves 10 cm up: the barrel tilts up toward it.
        f.hands[0].grip = pose(fore, Quat::IDENTITY);
        f.hands[0].squeeze = 1.0;
        let out = run(&mut m, &f, &g, &s, 0.1);
        assert!(out.weapon.unwrap().two_handed);
        f.hands[0].grip = pose(fore + Vec3::new(0.0, 0.1, 0.0), Quat::IDENTITY);
        let w = run(&mut m, &f, &g, &s, 0.2).weapon.unwrap();
        let fore2 = w.weapon_to_tracking.transform_point3(Vec3::from_array(g.rig.foregrip));
        let grip = Vec3::from_array(f.hands[1].grip.pos);
        let want = (fore + Vec3::new(0.0, 0.1, 0.0) - grip).normalize();
        assert!(((fore2 - grip).normalize()).abs_diff_eq(want, 1e-3), "foregrip follows the off hand");
        assert!(w.aim_dir.y > 0.1, "barrel tilted up: {:?}", w.aim_dir);
        assert!(m.haptics.iter().any(|h| h.hand == 0 && h.frequency == 150.0), "grab buzz");
    }

    #[test]
    fn slapping_the_magazine_well_reloads_once() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        let g = game();
        let out = run(&mut m, &f, &g, &s, 0.0);
        let w2t = out.weapon.unwrap().weapon_to_tracking;
        let rg = Vec3::from_array(g.rig.grip);
        let lg = Vec3::from_array(g.rig.foregrip);
        let mag = w2t.transform_point3(rg + (lg - rg) * 0.35 + Vec3::new(0.0, -0.07, 0.0));
        let mut t = 0.0;
        let mut y = -0.2;
        let mut reloads = 0;
        // Off hand comes up from below at 1.5 m/s through the well.
        while y < 0.05 {
            t += 1.0 / 90.0;
            y += 1.5 / 90.0;
            f.hands[0].grip = pose(mag + Vec3::new(0.0, y, 0.0), Quat::IDENTITY);
            let out = run(&mut m, &f, &g, &s, t);
            reloads = out.intents.presses[Action::Reload as usize];
        }
        assert_eq!(reloads, 1, "exactly one reload");
        assert_eq!(m.debug.reloads, 1);
    }

    #[test]
    fn chest_grip_is_the_medkit_gesture() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        let g = game();
        run(&mut m, &f, &g, &s, 0.0);
        f.hands[0].grip = pose(Vec3::new(-0.05, 1.7 - 0.4, 0.1), Quat::IDENTITY);
        f.hands[0].squeeze = 1.0;
        let out = run(&mut m, &f, &g, &s, 0.1);
        assert_eq!(out.intents.presses[Action::GestureChest as usize], 1);
        assert_eq!(m.debug.gestures[2], 1);
    }

    #[test]
    fn ducking_crouches_and_compensates_height() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        let mut g = game();
        let mut t = 0.0;
        // Stood still a moment: upright is learned.
        for _ in 0..180 {
            t += 1.0 / 90.0;
            run(&mut m, &f, &g, &s, t);
        }
        f.hmd.pos[1] = 1.2; // duck 50 cm
        t += 1.0 / 90.0;
        let out = run(&mut m, &f, &g, &s, t);
        assert_eq!(out.intents.presses[Action::Crouch as usize], 1, "one crouch toggle");
        // The game crouches: its eye drops to 1.1 m; the view must not drop again.
        g.flags |= GS_CROUCHED;
        g.eye_above_feet = 1.1;
        t += 1.0 / 90.0;
        let out = run(&mut m, &f, &g, &s, t);
        // Standing eye height was learned as the game's 1.7 m.
        assert!((out.tracking.height_comp - 0.6).abs() < 0.02, "{}", out.tracking.height_comp);
        // Standing up again toggles back.
        f.hmd.pos[1] = 1.7;
        t += 1.0;
        let out = run(&mut m, &f, &g, &s, t);
        assert_eq!(out.intents.presses[Action::Crouch as usize], 2);
    }

    #[test]
    fn crouching_for_real_in_a_stick_crouch_keeps_it() {
        let (mut m, s) = (Mechanics::default(), Settings::default());
        let dom = s.dominant();
        let (mut f, mut g) = (standing(), game());
        let mut t = 0.0;
        let mut step = |m: &mut Mechanics, f: &HmdFrame, g: &GameState, secs: f64| {
            let mut out = MechOutput::default();
            for _ in 0..(secs * 90.0) as usize {
                t += 1.0 / 90.0;
                out = run(m, f, g, &s, t);
            }
            out
        };
        let crouches = |o: &MechOutput| o.intents.presses[Action::Crouch as usize];
        step(&mut m, &f, &g, 2.0);
        // The stick: one crouch, and the game crouches.
        f.hands[dom].buttons |= CTRL_STICK_CLICK;
        step(&mut m, &f, &g, 0.05);
        f.hands[dom].buttons = 0;
        let o = step(&mut m, &f, &g, 0.3);
        assert_eq!(crouches(&o), 1, "the stick crouches");
        g.flags |= GS_CROUCHED;
        g.eye_above_feet = 1.1;
        // Crouching for real in it, and standing up again: nothing pressed, the game stays crouched.
        f.hmd.pos[1] = 1.5;
        let o = step(&mut m, &f, &g, 1.0);
        assert!(m.debug.phys_crouch && crouches(&o) == 1, "crouched for real: no press");
        // (Nor the view lifted back up by the game's crouch: it goes lower still.)
        assert_eq!(o.tracking.height_comp, 0.0);
        f.hmd.pos[1] = 1.7;
        let o = step(&mut m, &f, &g, 1.0);
        assert!(!m.debug.phys_crouch && crouches(&o) == 1, "stood up: still the stick's crouch");
    }

    #[test]
    fn snap_turn_once_per_flick() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        f.hands[1].stick = [1.0, 0.0];
        let mut t = 0.0;
        for _ in 0..20 {
            t += 1.0 / 90.0;
            run(&mut m, &f, &game(), &s, t);
        }
        assert!((m.turn_total - s.comfort.snap_angle.to_radians()).abs() < 1e-4, "held stick: one snap");
        f.hands[1].stick = [0.0, 0.0];
        run(&mut m, &f, &game(), &s, t + 0.01);
        f.hands[1].stick = [1.0, 0.0];
        run(&mut m, &f, &game(), &s, t + 0.02);
        assert!((m.turn_total - 2.0 * s.comfort.snap_angle.to_radians()).abs() < 1e-4);
    }

    #[test]
    fn a_conversation_turns_with_the_stick_other_menus_take_it() {
        let menu = |m: &mut Mechanics, f: &HmdFrame, g: &GameState, s: &Settings, t: f64| {
            m.update(&MechInput {
                frame: f,
                game: Some(g),
                settings: s,
                now: t,
                dt: 1.0 / 90.0,
                menu_mode: true,
                runtime_menu: false,
                cursor: None,
                aux_aspect: 1.0,
                rumble: None,
                rig: None,
                spell_hands: [false; 2],
                game_shots: 0,
                held_kicks: &[],
                riding: false,
                sculpting: [false; 2],
            })
        };
        let s = Settings::default();
        let mut f = standing();
        f.hands[s.dominant()].stick = [1.0, 0.0];
        let mut g = game();
        g.flags |= GS_MENU;
        // (any other menu: the stick's right is the menu's)
        let mut m = Mechanics::default();
        let out = menu(&mut m, &f, &g, &s, 0.0);
        assert_eq!((m.turn_total, out.intents.held & Action::MenuRight.bit() != 0), (0.0, true));
        // (a conversation: a step, once for the flick, and the menu doesn't get it)
        g.flags |= GS_MENU_TURNS;
        let mut m = Mechanics::default();
        let mut out = menu(&mut m, &f, &g, &s, 0.0);
        for i in 1..20 {
            out = menu(&mut m, &f, &g, &s, i as f64 / 90.0);
        }
        assert!((m.turn_total - s.comfort.snap_angle.to_radians()).abs() < 1e-4, "{}", m.turn_total);
        assert_eq!(out.tracking.turn_total, m.turn_total);
        assert_eq!(out.intents.held & Action::MenuRight.bit(), 0);
    }

    #[test]
    fn room_offset_walks_the_body() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let f = standing();
        let mut g = game();
        g.tracking.room_offset = [0.0, 0.3];
        g.tracking.look_yaw = 0.0;
        let out = run(&mut m, &f, &g, &s, 0.0);
        assert!(out.tracking.flags & TRK_FOLLOW != 0);
        assert!(out.intents.move_xy[1] > 0.3, "walks forward: {:?}", out.intents.move_xy);
        g.tracking.room_offset = [0.0, 0.03];
        let out = run(&mut m, &f, &g, &s, 0.1);
        assert!(out.tracking.flags & TRK_FOLLOW == 0, "stops when caught up");
    }

    fn rifle() -> GameState {
        let mut g = game();
        g.rig.id = 7;
        g.rig.flags |= RIG_HAS_MAG_WELL | RIG_HAS_CHARGING_HANDLE;
        g.rig.mag_well = [0.0, -0.02, 0.1];
        g.rig.charging_handle = [0.035, 0.05, 0.16];
        g
    }

    /// Steps `frames` frames with the off hand moved to `pos` (tracking space).
    fn hold_off(m: &mut Mechanics, f: &mut HmdFrame, g: &GameState, s: &Settings, t: &mut f64, pos: Vec3, squeeze: f32, frames: u32) -> MechOutput {
        f.hands[0].grip = pose(pos, Quat::IDENTITY);
        f.hands[0].squeeze = squeeze;
        let mut out = MechOutput::default();
        for _ in 0..frames {
            *t += 1.0 / 90.0;
            out = run(m, f, g, s, *t);
        }
        out
    }

    #[test]
    fn full_reload_eject_fetch_seat_rack() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        let g = rifle();
        let mut t = 0.0;
        let out = run(&mut m, &f, &g, &s, t);
        let w2t = out.weapon.unwrap().weapon_to_tracking;
        let well = w2t.transform_point3(Vec3::from_array(g.rig.mag_well));
        let handle = w2t.transform_point3(Vec3::from_array(g.rig.charging_handle));
        // Eject: tap A on the firing hand.
        f.hands[1].buttons = CTRL_A;
        run(&mut m, &f, &g, &s, 0.05);
        f.hands[1].buttons = 0;
        let out = run(&mut m, &f, &g, &s, 0.1);
        t = 0.1;
        assert_eq!(m.debug.mag, MagState::Out);
        assert!(out.intents.flags & INT_MAG_OUT != 0);
        assert_eq!(out.intents.presses[Action::Reload as usize], 0, "ejecting isn't reloading yet");
        // Grab a magazine at the hip, bring it to the well.
        let hip = Vec3::new(-0.25, 0.85, 0.05);
        hold_off(&mut m, &mut f, &g, &s, &mut t, hip, 0.0, 3);
        let out = hold_off(&mut m, &mut f, &g, &s, &mut t, hip, 1.0, 2);
        assert_eq!(m.debug.mag, MagState::Held);
        assert!(out.intents.flags & INT_HOLDING_MAG != 0);
        assert_eq!(out.intents.presses[Action::GestureHip as usize], 0, "the hip is a pouch during a reload, not the knife");
        hold_off(&mut m, &mut f, &g, &s, &mut t, well, 1.0, 2);
        assert_eq!(m.debug.mag, MagState::Seated);
        // Rack: grab the charging handle and pull it back 8 cm.
        hold_off(&mut m, &mut f, &g, &s, &mut t, handle, 0.0, 3);
        hold_off(&mut m, &mut f, &g, &s, &mut t, handle, 1.0, 2);
        let out = hold_off(&mut m, &mut f, &g, &s, &mut t, handle - Vec3::Z * 0.08, 1.0, 2);
        assert_eq!(m.debug.mag, MagState::In);
        assert_eq!(out.intents.presses[Action::Reload as usize], 1, "the game reloads once, at the rack");
    }

    #[test]
    fn dropping_the_magazine_needs_another() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        let g = rifle();
        let mut t = 0.0;
        run(&mut m, &f, &g, &s, t);
        f.hands[1].buttons = CTRL_A;
        run(&mut m, &f, &g, &s, 0.05);
        f.hands[1].buttons = 0;
        run(&mut m, &f, &g, &s, 0.1);
        t = 0.1;
        let chest = Vec3::new(-0.05, 1.3, 0.1);
        hold_off(&mut m, &mut f, &g, &s, &mut t, chest, 0.0, 2);
        hold_off(&mut m, &mut f, &g, &s, &mut t, chest, 1.0, 2);
        assert_eq!(m.debug.mag, MagState::Held);
        hold_off(&mut m, &mut f, &g, &s, &mut t, chest, 0.0, 2);
        assert_eq!(m.debug.mag, MagState::Out, "letting go drops it");
    }

    #[test]
    fn a_fast_punch_is_a_melee_but_a_slow_reach_isnt() {
        let mut m = Mechanics::default();
        let s = Settings::default();
        let mut f = standing();
        let g = game();
        let mut t = 0.0;
        // Slow reach forward: 1 m/s.
        f.hands[0].vel = [0.0, 0.0, 1.0];
        t += 0.1;
        let out = run(&mut m, &f, &g, &s, t);
        assert_eq!(out.intents.presses[Action::Melee as usize], 0);
        // Punch: 4 m/s forward.
        f.hands[0].vel = [0.0, 0.3, 4.0];
        t += 0.1;
        let out = run(&mut m, &f, &g, &s, t);
        assert_eq!(out.intents.presses[Action::Melee as usize], 1);
        // Still moving fast right after: the cooldown stops a double hit.
        t += 0.1;
        let out = run(&mut m, &f, &g, &s, t);
        assert_eq!(out.intents.presses[Action::Melee as usize], 1);
        // Racking motion (pulling back fast) isn't a strike.
        f.hands[0].vel = [0.0, 0.0, -4.0];
        t += 1.0;
        let out = run(&mut m, &f, &g, &s, t);
        assert_eq!(out.intents.presses[Action::Melee as usize], 1);
    }

    #[test]
    fn zones_lie_round_the_head_and_body() {
        assert_eq!(zone_of(Vec3::ZERO, Vec3::new(0.0, -0.05, 0.08)), Zone::Face);
        assert_eq!(zone_of(Vec3::ZERO, Vec3::new(0.0, 0.15, 0.0)), Zone::HeadTop);
        assert_eq!(zone_of(Vec3::new(0.2, -0.2, -0.1), Vec3::new(0.2, -0.2, -0.1)), Zone::Shoulder);
        assert_eq!(zone_of(Vec3::new(0.0, -0.4, 0.1), Vec3::new(0.0, -0.4, 0.1)), Zone::Chest);
        assert_eq!(zone_of(Vec3::new(0.3, -0.9, 0.0), Vec3::new(0.3, -0.9, 0.0)), Zone::Hip);
        assert_eq!(zone_of(Vec3::new(0.0, 0.0, 0.6), Vec3::new(0.0, 0.0, 0.6)), Zone::None);
    }
}
