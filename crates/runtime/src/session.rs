//! The runtime's frame loop: headset -> mechanics -> adapter -> compositor -> headset.

use anyhow::Result;
use glam::{Quat, Vec2, Vec3};
use rustvr_gpu::Gpu;
use rustvr_protocol::link::{Link, Security};
use rustvr_protocol::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::backend::{Backend, HmdFrame};
use crate::compositor::{Comfort, Compositor, Laser, Layers, LensLayer, PanelLayer};
use crate::mechanics::{Cursor, MechInput, MechOutput, Mechanics};
use crate::menu::{RuntimeMenu, Status};
use crate::panels::{self, PanelId};
use crate::rig::{ObjSnap, Rig, RigInput, RigOutput, WorldMap, feedback};
use crate::settings::SettingsStore;
use crate::targets::{SharedTargets, Spec};

pub struct Options {
    pub mapping: String,
    /// Folder with `settings.toml` and `profiles/`; None = defaults, nothing saved.
    pub settings_dir: Option<PathBuf>,
    /// Wait for the adapter's frame every headset frame (deterministic tests).
    pub lockstep: bool,
    pub lockstep_timeout_ms: u32,
    /// Play sounds on the default output device (off for headless tests).
    pub audio: bool,
    /// Without `audio`: mix the sounds offline anyway, each frame, heard by no one, so headless
    /// tests can see which ones played (the game's or the runtime's own).
    pub offline_audio: bool,
}

/// The offline mixer's rate.
const OFFLINE_RATE: u32 = 48_000;

impl Default for Options {
    fn default() -> Self {
        Options { mapping: DEFAULT_MAPPING.to_owned(), settings_dir: None, lockstep: false, lockstep_timeout_ms: 2000, audio: false, offline_audio: false }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub frames: u64,
    /// Frames shown with a game picture.
    pub game_frames: u64,
    /// Slots taken from the adapter.
    pub slots: u64,
    pub lockstep_timeouts: u64,
    pub last_slot_frame: u64,
    /// Frames where the shown game picture was rendered for this exact headset frame.
    pub exact_frames: u64,
    pub scope_frames: u64,
    pub menu_frames: u64,
    /// Comfort overlay strength last frame.
    pub vignette: f32,
    pub fade: f32,
    pub vignette_frames: u64,
    pub fade_frames: u64,
    /// Objects the runtime drew itself into the eyes last frame (`OBJF_RUNTIME_DRAWS`), and in
    /// all.
    pub held_drawn: u32,
    pub held_drawn_frames: u64,
    /// The game's HUD in pieces last frame (see `hud.rs`): each drawn piece's role and its
    /// panel's centre (tracking space), the first `hud_piece_count`.
    pub hud_pieces: [(u32, Vec3); 8],
    pub hud_piece_count: usize,
}

/// What a scripted test can see of the runtime's UI, to aim at it.
#[derive(Default, Clone)]
pub struct World {
    pub game_menu: Option<panels::Panel>,
    pub runtime_menu: Option<panels::Panel>,
    /// Centres of named runtime-menu widgets, as panel UVs.
    pub widgets: Vec<(String, Vec2)>,
    /// The game's objects as the rig has them (tracking space).
    pub objects: Vec<ObjSnap>,
}

pub struct Session {
    pub gpu: Gpu,
    pub link: Link,
    sec: Option<Security>,
    pub backend: Box<dyn Backend>,
    pub settings: SettingsStore,
    pub mech: Mechanics,
    comp: Compositor,
    /// The runtime's physics world, shown instead of the game's picture (`Mechanics::framework_view`).
    physics_view: crate::physics_view::PhysicsView,
    /// Frame pacing, said every ten seconds (`crate::pacing`).
    pacing: crate::pacing::Pacing,
    /// The desktop view's window (`crate::spectator`), while it's open, and its camera.
    spectator: Option<crate::spectator::Window>,
    spectator_cam: crate::spectator::SteadyCamera,
    /// Tests: where the desktop view's frames are saved, and when one was last.
    spectator_capture: Option<(PathBuf, f64)>,
    physics_tris: Vec<([Vec3; 3], [f32; 3])>,
    /// The rig ran this step (there's a physics world to show).
    rig_active_last: bool,
    menu: RuntimeMenu,
    layout: panels::Layout,
    targets: Option<SharedTargets>,
    generation: u32,
    front: usize,
    shown: Option<(usize, SlotInfo)>,
    pub game: Option<GameState>,
    pub stats: Stats,
    pub last_out: MechOutput,
    pub last_frame: HmdFrame,
    pub world: Arc<Mutex<World>>,
    /// The player's hands and the game's objects (active once the game registers objects).
    pub rig: Rig,
    pub rig_out: RigOutput,
    objects: Box<ObjectTable>,
    states: Box<ObjectStates>,
    /// How long after the runtime's frame the game's picture of it is shown, seconds (smoothed).
    game_lag: f32,
    /// The player's walk and turn in the game, from its stamped steps (`crate::locomotion`), and
    /// the mapping (tracking -> world) for when this frame is shown: where the world is drawn and
    /// where the hands reach into it.
    locomotion: crate::locomotion::Locomotion,
    map_now: Option<WorldMap>,
    /// Testing: `RUSTVR_TEST_TAKE` has put a prop in the hand.
    test_taken: bool,
    objects_seq: u32,
    /// The game's own hand poses for its objects (`ObjectDesc::grip_pose`, `support_pose`).
    hand_poses: Box<HandPoseTable>,
    hand_poses_seq: u32,
    kicks: Box<KickTable>,
    kicks_seq: u32,
    /// Legacy mode's recoil: the held gun's kicks (`KICK_OBJECT_HELD`) and the shots the game
    /// reported since the last frame.
    held_kicks: Vec<KickDesc>,
    game_shots: u32,
    /// What the game said its HUD's pieces are (see `hud.rs`), and where they go.
    hud_layout: Box<HudLayout>,
    hud_layout_seq: u32,
    /// Where the game's own simulated objects are (`OBJF_GAME_SIMULATED`).
    game_poses: Box<GamePoses>,
    /// Where the hands push them (see `Rig::game_pushes`).
    game_pushes: Box<GamePushes>,
    /// Their convex collision (see `Rig::set_game_hulls`).
    game_hulls: Box<GameHulls>,
    hud: crate::hud::HudPlacer,
    /// The game's lighting of its first-person model (the last consistent copy), and whether
    /// there is one: the body is shaded with it.
    game_lighting: Box<GameLighting>,
    game_lighting_read: Box<GameLighting>,
    has_game_lighting: bool,
    /// The interaction sounds (see `play_cue`).
    pub audio: Option<rustvr_audio::Audio>,
    /// Where the game's sound pack is and the objects' sound sets (the `SoundPack` region).
    sound_pack: Box<SoundPack>,
    sound_pack_seq: u32,
    /// Per object: its sound set and `SND_*` flags.
    object_sounds: std::collections::HashMap<u32, (rustvr_audio::SetKey, u32)>,
    /// The pack asked of the audio (folder and revision), if any.
    pack_wanted: Option<(PathBuf, u32)>,
    comfort: Comfort,
    menu_trigger_was: bool,
    /// The player's body (IK) and its avatar.
    pub body: crate::body::PlayerBody,
    /// What the runtime draws of what the player holds (`OBJF_RUNTIME_DRAWS`).
    pub held_objects: crate::held_objects::HeldObjects,
    /// The runtime draws the body this frame.
    pub draws_body: bool,
    /// When to take the T-pose calibration (seconds, the frame clock).
    calibrate_at: Option<f64>,
    countdown_tick: i64,
    /// The game's floor height in tracking space this frame.
    pub floor: f32,
    _heartbeat: Heartbeat,
    glove_loops: [Option<rustvr_audio::LoopId>; 2],
    /// Sounds played (tests look at this).
    pub sounds_played: u64,
    opts: Options,
    fps: f32,
    last_tick: std::time::Instant,
    /// When a slow frame was last logged.
    slow_logged: f64,
    was_alive: bool,
}

impl Session {
    pub fn new(gpu: Gpu, backend: Box<dyn Backend>, opts: Options) -> Result<Session> {
        let link = Link::create(&opts.mapping)?;
        let settings = match &opts.settings_dir {
            Some(d) => SettingsStore::open(d),
            None => SettingsStore::memory(Default::default()),
        };
        tracing::info!("runtime: {} on {} ({}), link {}", backend.name(), gpu.adapter_name(), backend.recommended_size().0, opts.mapping);
        // The runtime's GPU work goes before the game's, as a headset compositor's does: the game
        // rendering flat out on the same GPU mustn't make a headset frame late.
        {
            use windows::core::Interface;
            let raised = gpu.device.cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice>().and_then(|d| unsafe { d.SetGPUThreadPriority(7) });
            match raised {
                Ok(()) => tracing::info!("runtime: GPU priority raised over the game's"),
                Err(e) => tracing::info!("runtime: GPU priority left as it is ({e})"),
            }
        }
        // The desktop view: off in tests on the emulated headset (RUSTVR_DESKTOP_VIEW=0, or
        // `capture:<dir>`: off screen, a frame saved there every few seconds) and with the
        // built-in simulator.
        let sp = &settings.current.spectator;
        let env = std::env::var("RUSTVR_DESKTOP_VIEW").unwrap_or_default();
        let capture = env.strip_prefix("capture:").map(PathBuf::from);
        let wanted = (sp.enabled || capture.is_some()) && backend.name() != "sim" && env != "0";
        let spectator = wanted
            .then(|| crate::spectator::Window::open(&gpu, sp.width, sp.height, capture.is_some()))
            .and_then(|w| w.inspect_err(|e| tracing::warn!("desktop view: {e:#}")).ok());
        if spectator.is_some() {
            tracing::info!("desktop view: open");
        }
        Ok(Session {
            spectator,
            spectator_capture: capture.map(|d| (d, 0.0)),
            spectator_cam: crate::spectator::SteadyCamera::default(),
            comp: Compositor::new(&gpu)?,
            physics_view: crate::physics_view::PhysicsView::new(&gpu)?,
            pacing: crate::pacing::Pacing::new(&gpu)?,
            physics_tris: Vec::new(),
            rig_active_last: false,
            menu: RuntimeMenu::new(&gpu)?,
            gpu,
            link,
            sec: Security::for_this_user(),
            backend,
            settings,
            mech: Mechanics::default(),
            layout: panels::Layout::default(),
            targets: None,
            generation: 0,
            front: 2,
            shown: None,
            game: None,
            stats: Stats::default(),
            last_out: MechOutput::default(),
            last_frame: HmdFrame::default(),
            world: Arc::new(Mutex::new(World::default())),
            rig: Rig::default(),
            rig_out: RigOutput::default(),
            objects: bytemuck::zeroed_box(),
            states: bytemuck::zeroed_box(),
            game_lag: 0.0,
            locomotion: crate::locomotion::Locomotion::new(),
            map_now: None,
            test_taken: false,
            objects_seq: 0,
            hand_poses: bytemuck::zeroed_box(),
            hand_poses_seq: 0,
            kicks: bytemuck::zeroed_box(),
            kicks_seq: 0,
            held_kicks: Vec::new(),
            game_shots: 0,
            hud_layout: bytemuck::zeroed_box(),
            hud_layout_seq: 0,
            game_poses: bytemuck::zeroed_box(),
            game_pushes: bytemuck::zeroed_box(),
            game_hulls: bytemuck::zeroed_box(),
            game_lighting: bytemuck::zeroed_box(),
            game_lighting_read: bytemuck::zeroed_box(),
            has_game_lighting: false,
            hud: Default::default(),
            audio: if opts.audio {
                Some(rustvr_audio::Audio::new())
            } else {
                opts.offline_audio.then(|| rustvr_audio::Audio::offline(OFFLINE_RATE))
            },
            sound_pack: bytemuck::zeroed_box(),
            sound_pack_seq: 0,
            object_sounds: Default::default(),
            pack_wanted: None,
            comfort: Comfort::default(),
            menu_trigger_was: false,
            body: crate::body::PlayerBody::default(),
            held_objects: Default::default(),
            draws_body: false,
            calibrate_at: None,
            countdown_tick: -1,
            floor: 0.0,
            _heartbeat: Heartbeat::start(&opts.mapping),
            glove_loops: [None; 2],
            sounds_played: 0,
            opts,
            fps: 0.0,
            last_tick: std::time::Instant::now(),
            slow_logged: f64::MIN,
            was_alive: false,
        })
    }

    pub fn adapter_connected(&self) -> bool {
        self.was_alive
    }

    /// One headset frame. Ok(false) when the backend has finished.
    pub fn step(&mut self) -> Result<bool> {
        let t_wait = std::time::Instant::now();
        let waited = self.backend.wait_frame();
        let headset_wait = t_wait.elapsed().as_secs_f32() * 1000.0;
        let frame = match waited {
            Ok(Some(f)) => f,
            Ok(None) => {
                self.link.heartbeat_runtime();
                return Ok(true);
            }
            Err(e) => {
                tracing::info!("runtime: backend finished: {e:#}");
                return Ok(false);
            }
        };
        let now = frame.display_time_ns as f64 * 1e-9;
        self.pacing.frame(frame.display_time_ns, 1.0 / self.backend.refresh_hz().max(30.0) as f64);
        self.pacing.wait(crate::pacing::Wait::Headset, headset_wait);
        self.link.heartbeat_runtime();
        if let Some(w) = &mut self.spectator
            && !w.pump(&self.gpu)
        {
            self.spectator = None;
        }
        let elapsed = self.last_tick.elapsed().as_secs_f32();
        self.last_tick = std::time::Instant::now();
        if elapsed > 0.0 {
            self.fps += (1.0 / elapsed - self.fps) * 0.05;
        }

        // ---- adapter
        if self.link.poll_connect() {
            tracing::info!("runtime: adapter connected (pid {})", self.link.adapter_pid());
            self.front = 2;
            self.shown = None;
            self.reset_rig();
        }
        let alive = self.link.adapter_alive();
        if alive != self.was_alive {
            tracing::info!("runtime: adapter {}", if alive { "running" } else { "gone" });
            if !alive {
                self.shown = None;
                self.reset_rig();
            }
            self.was_alive = alive;
        }
        self.game = if alive { self.link.read_game_state() } else { None };
        // Where the game has the player when this frame is shown: its last step, walked and turned
        // on from when its clock stood there (the game steps at its own, uneven pace).
        match self.game.as_ref() {
            Some(g) => {
                let m = WorldMap::from_state(&g.tracking);
                self.locomotion.see(g.time_qpc, m);
                self.map_now = Some(self.locomotion.at(frame.display_qpc, m));
            }
            None => {
                self.locomotion.reset();
                self.map_now = None;
            }
        }
        // A torn read (the adapter writing as we read) keeps the last good copy.
        if !alive {
            self.has_game_lighting = false;
        } else if self.link.read_game_lighting_into(&mut self.game_lighting_read) {
            std::mem::swap(&mut self.game_lighting, &mut self.game_lighting_read);
            self.has_game_lighting = self.game_lighting.eye.valid != 0 && self.game_lighting.depth_squash > 0.0;
        }
        let name = self.game.as_ref().map(|g| name_str(&g.name).to_owned());
        self.settings.set_profile(name.as_deref());
        if self.settings.poll() {
            tracing::info!("runtime: settings reloaded");
        }
        if let Some(g) = &self.game {
            let spec = Spec::new(&g.request, self.backend.recommended_size(), &self.settings.current);
            if self.targets.as_ref().is_none_or(|t| t.spec != spec) {
                self.generation += 1;
                match SharedTargets::create(&self.gpu, &self.opts.mapping, self.generation, spec, self.sec.as_ref()) {
                    Ok(t) => {
                        tracing::info!("runtime: targets generation {} {}x{} flags {:#x} format {} on {}", self.generation, spec.eye_w, spec.eye_h, spec.flags, spec.color_format, self.gpu.adapter_name());
                        self.link.write_targets(&t.info());
                        self.targets = Some(t);
                        self.shown = None;
                    }
                    Err(e) => tracing::error!("runtime: creating targets: {e:#}"),
                }
            }
        }

        let mut rumble = None;
        let dom = self.settings.current.dominant();
        while let Some(ev) = self.link.pop_event() {
            match ev.kind {
                EV_HAPTIC => {
                    let hand = match ev.a {
                        HAPTIC_LEFT => 0,
                        HAPTIC_RIGHT => 1,
                        HAPTIC_FIRING_HAND => dom,
                        _ => dom ^ 1,
                    };
                    self.backend.haptic(hand, ev.f[0], ev.f[1], ev.f[2]);
                }
                EV_RUMBLE => rumble = Some(ev.f[0]),
                EV_RECENTER => self.mech.recenter(),
                EV_GAME_SHOTS => self.game_shots = self.game_shots.saturating_add(ev.a),
                // The game's explosions and hits push what the rig simulates (on its next step).
                EV_BLAST => self.rig.blast(Vec3::new(ev.f[0], ev.f[1], ev.f[2]), ev.f[3], ev.f[4]),
                EV_IMPULSE => self.rig.impulse(ev.a, Vec3::new(ev.f[0], ev.f[1], ev.f[2]), Vec3::new(ev.f[3], ev.f[4], ev.f[5])),
                _ => {}
            }
        }

        // ---- panels and pointing
        let gflags = self.game.as_ref().map(|g| g.flags).unwrap_or(0);
        let menu_mode = gflags & GS_MENU != 0;
        let runtime_menu_open = menu_mode || !alive;
        let shown_info = self.shown.map(|(_, i)| i);
        let hud_aspect = shown_info
            .filter(|i| i.flags & SLOT_HUD != 0 && i.hud_aspect > 0.0)
            .map(|i| i.hud_aspect)
            .or_else(|| self.targets.as_ref().map(|t| t.spec.hud_w as f32 / t.spec.hud_h.max(1) as f32));
        let menu_panel = if menu_mode { hud_aspect.or(Some(16.0 / 9.0)) } else { None };
        let hud_panel = if !menu_mode && shown_info.is_some_and(|i| i.flags & SLOT_HUD != 0) { hud_aspect } else { None };
        let head_pos = Vec3::from_array(frame.hmd.pos);
        let head_rot = quat(frame.hmd.rot);
        let layout = self.layout.update(
            &self.settings.current,
            head_pos,
            head_rot,
            frame.dt,
            menu_panel,
            hud_panel,
            if runtime_menu_open { Some(self.menu.aspect()) } else { None },
        );
        let hand = &frame.hands[dom];
        let laser_origin = Vec3::from_array(hand.aim.pos);
        let laser_dir = quat(hand.aim.rot) * Vec3::Z;
        let pick = if hand.connected != 0 && (layout.game_menu.is_some() || layout.runtime_menu.is_some()) {
            layout.pick(laser_origin, laser_dir)
        } else {
            None
        };
        let trigger_down = hand.trigger > 0.6;
        let cursor = match pick {
            Some((PanelId::GameMenu, _, uv)) => Some(Cursor { uv, down: trigger_down }),
            _ => None,
        };
        let menu_pointer = match pick {
            Some((PanelId::RuntimeMenu, _, uv)) => Some((uv, trigger_down)),
            _ => None,
        };
        let snd = &self.settings.current.sound;
        if let Some(a) = &self.audio {
            a.set_volume(if snd.enabled { snd.volume } else { 0.0 });
            a.set_listener(rustvr_audio::Listener { pos: head_pos, rot: head_rot });
        }
        // A click on either panel: a soft UI click where the laser hits.
        if let (Some((_, t, _)), Some(a)) = (pick, &self.audio)
            && trigger_down && !self.menu_trigger_was && snd.enabled {
                let at = laser_origin + laser_dir * t;
                a.play_event(rustvr_audio::Event::UiClick, rustvr_audio::SetKey::NONE, Some(rustvr_audio::Sound::UiClick), at, 1.0);
            }
        self.menu_trigger_was = trigger_down;

        // ---- the rig: the game's collision and objects, then the hands
        let t_rig = std::time::Instant::now();
        let mut t_collision = 0.0f32;
        if alive {
            let physics = &mut self.rig.physics;
            self.link.drain_collision(|kind, payload| physics.apply_collision(kind, payload));
            t_collision = t_rig.elapsed().as_secs_f32() * 1000.0;
            let seq = self.link.objects_seq();
            if seq != self.objects_seq && self.link.read_objects_into(&mut self.objects) {
                self.objects_seq = seq;
                self.rig.sync(&self.objects);
                self.held_objects.sync(&self.gpu, &self.objects);
                tracing::info!("runtime: {} objects registered (table {})", self.objects.count, seq);
            }
            let seq = self.link.hand_poses_seq();
            if seq != self.hand_poses_seq && self.link.read_hand_poses_into(&mut self.hand_poses) {
                self.hand_poses_seq = seq;
                tracing::info!("runtime: {} hand poses registered (table {})", self.hand_poses.count, seq);
            }
            let seq = self.link.hud_layout_seq();
            if seq != self.hud_layout_seq && self.link.read_hud_layout_into(&mut self.hud_layout) {
                self.hud_layout_seq = seq;
                tracing::info!("runtime: the game's HUD has {} pieces (layout {})", self.hud_layout.count, seq);
            }
            let seq = self.link.kicks_seq();
            if seq != self.kicks_seq && self.link.read_kicks_into(&mut self.kicks) {
                self.kicks_seq = seq;
                self.rig.set_kicks(&self.kicks);
                self.held_kicks = self.kicks.entries().iter().filter(|k| k.object == KICK_OBJECT_HELD).copied().collect();
                tracing::info!("runtime: {} recoil kicks registered (table {})", self.kicks.count, seq);
            }
            let seq = self.link.sound_pack_seq();
            if seq != self.sound_pack_seq && self.link.read_sound_pack_into(&mut self.sound_pack) {
                self.sound_pack_seq = seq;
                let p = &self.sound_pack;
                let sets = p.entries().iter().filter(|o| o.object != 0);
                self.object_sounds = sets.map(|o| (o.object, (rustvr_audio::SetKey::of(o.set_name()), o.flags))).collect();
                tracing::info!(
                    "runtime: the game's sound pack: {:?} (revision {}, flags {:#x}, {} objects with sound sets)",
                    p.path(),
                    p.revision,
                    p.flags,
                    self.object_sounds.len()
                );
            }
        }
        self.update_sound_pack(alive);
        let rig_active = alive && self.game.as_ref().is_some_and(|g| self.rig.has_objects() || g.flags & GS_PLAYER_RIG != 0);
        self.rig_active_last = rig_active;
        // The game's floor in tracking space: the adapter maps the tracking origin onto the game's
        // eye (plus the height compensation and offset the runtime asked for).
        let floor = match &self.game {
            Some(g) if alive && g.eye_above_feet > 0.3 => {
                let t = &self.last_out.tracking;
                let wpm = if g.tracking.world_per_meter > 0.0 { g.tracking.world_per_meter } else { 1.0 };
                g.tracking.origin[1] - (g.eye_above_feet + t.height_comp + t.height_offset) / wpm
            }
            _ => 0.0,
        };
        self.floor = floor;
        if rig_active {
            // Where the game has its own simulated props now.
            if self.link.read_game_hulls_into(&mut self.game_hulls) {
                self.rig.set_game_hulls(&self.game_hulls);
            }
            if self.link.read_game_poses_into(&mut self.game_poses) {
                self.rig.set_game_poses(&self.game_poses);
            }
            let g = self.game.as_ref().unwrap();
            let stand = self.mech.debug.stand_h - floor;
            // Testing (`RUSTVR_TEST_TAKE=left|right[:ours]`): the first time that hand's grip
            // closes, the nearest of the game's loose props goes into it (no aiming the gloves;
            // `ours`: one the runtime moves itself).
            let test_take = std::env::var("RUSTVR_TEST_TAKE").ok();
            let test_take = test_take.as_deref().map(|t| t.split_once(':').unwrap_or((t, "")));
            if let Some((hand, ours)) = test_take.and_then(|(h, rest)| match h {
                "left" => Some((0usize, rest == "ours")),
                "right" => Some((1, rest == "ours")),
                _ => None,
            }) && !self.test_taken
                && frame.hands[hand].squeeze > 0.9
            {
                self.test_taken = true;
                let got = self.rig.test_take_nearest(hand, ours);
                tracing::info!("runtime: test: the nearest prop into the {} hand: {got:?}", if hand == 0 { "left" } else { "right" });
            }
            self.rig_out = self.rig.update(&RigInput {
                frame: &frame,
                map: self.map_now.unwrap_or_else(|| WorldMap::from_state(&g.tracking)),
                dt: frame.dt,
                now,
                settings: &self.settings.current,
                eye_height: if stand > 1.0 { stand } else { 1.7 },
                crouched: g.flags & GS_CROUCHED != 0,
                paused: menu_mode || menu_pointer.is_some() || g.flags & (GS_PAUSED | GS_LOADING | GS_DEAD) != 0,
                infinite_ammo: g.flags & GS_INFINITE_AMMO != 0,
            });
            // One event, one frame: the pulse and the sound together. The same cue for the same
            // object twice in a frame (both hands' pulses for one shot, or for one seat) is one
            // sound, at the louder of the two.
            let mut heard: Vec<feedback::Feedback> = Vec::new();
            for fb in &self.rig.feedback {
                if let (Some(hand), Some((secs, freq, amp))) = (fb.hand, feedback::haptic(fb.cue, fb.intensity)) {
                    self.backend.haptic(hand, secs, freq, amp * fb.intensity.clamp(0.0, 1.0).max(0.3));
                }
                match heard.iter_mut().find(|h| h.cue == fb.cue && h.object == fb.object) {
                    Some(h) => h.intensity = h.intensity.max(fb.intensity),
                    None => heard.push(*fb),
                }
            }
            if self.settings.current.sound.enabled {
                for fb in &heard {
                    self.play_cue(fb);
                }
            }
            // The gravity gloves' whoosh follows the object to the hand.
            for h in 0..2 {
                match (self.rig_out.pulled[h], self.glove_loops[h], &self.audio) {
                    (Some(p), None, Some(a)) => self.glove_loops[h] = Some(a.start_loop(rustvr_audio::Sound::GloveTravel, p)),
                    (Some(p), Some(id), Some(a)) => a.move_loop(id, p),
                    (None, Some(id), Some(a)) => {
                        a.stop_loop(id);
                        self.glove_loops[h] = None;
                    }
                    _ => {}
                }
            }
            // Every two seconds: where the hands are against their controllers, in the game's world.
            if let Some(map) = self.map_now
                && (now / 2.0).floor() != ((now - frame.dt as f64) / 2.0).floor()
            {
                let at = |p: &Pose| map.point(Vec3::from_array(p.pos));
                let line = |i: usize| {
                    let h = &self.rig_out.hands[i];
                    format!("{:.3?} for the controller at {:.3?}{}", at(&h.pose), at(&frame.hands[i].grip), if h.flags & HAND_BLOCKED != 0 { " (blocked)" } else { "" })
                };
                tracing::info!("rig: left hand {}; right hand {}", line(0), line(1));
            }
            self.rig.write_states(&mut self.states);
            // Where the hands push the game's own props.
            self.game_pushes.count = self.rig.game_pushes.len().min(MAX_GAME_PUSHES) as u32;
            for (i, p) in self.rig.game_pushes.iter().take(MAX_GAME_PUSHES).enumerate() {
                self.game_pushes.pushes[i] = *p;
            }
            self.link.write_game_pushes(&self.game_pushes);
            self.link.write_object_states(&self.states);
            self.held_objects.update(&self.objects, &self.states, self.map_now.as_ref(), frame.hands.map(|h| h.trigger));
        } else {
            self.held_objects.clear();
        }
        if let (true, Some(a)) = (self.opts.offline_audio && !self.opts.audio, &self.audio) {
            a.render((frame.dt.clamp(0.0, 0.1) * OFFLINE_RATE as f32).round() as usize);
        }

        let t_mech = std::time::Instant::now();
        // ---- mechanics
        let aux_aspect = self.targets.as_ref().map(|t| t.spec.aux_w as f32 / t.spec.aux_h.max(1) as f32).unwrap_or(1.0);
        let out = self.mech.update(&MechInput {
            frame: &frame,
            game: self.game.as_ref(),
            settings: &self.settings.current,
            now,
            dt: frame.dt,
            menu_mode,
            runtime_menu: menu_pointer.is_some(),
            cursor,
            aux_aspect,
            rumble,
            rig: rig_active.then_some(&self.rig_out),
            game_shots: std::mem::take(&mut self.game_shots),
            held_kicks: &self.held_kicks,
        });
        for h in self.mech.haptics.drain(..) {
            self.backend.haptic(h.hand, h.seconds, h.frequency, h.amplitude);
        }

        // Arm measurement: a tick each second of the countdown, then the T-pose is measured.
        if let Some(t) = self.calibrate_at {
            let left = t - now;
            let tick = (left.ceil() as i64).max(0);
            if tick != self.countdown_tick {
                self.countdown_tick = tick;
                for h in 0..2 {
                    self.backend.haptic(h, 0.03, 180.0, 0.5);
                }
            }
            if left <= 0.0 {
                self.calibrate_at = None;
                let p = |pose: &Pose| Vec3::from_array(pose.pos);
                match crate::body::measure_tpose(p(&frame.hmd), p(&frame.hands[0].grip), p(&frame.hands[1].grip)) {
                    Some((_, span)) => {
                        tracing::info!("runtime: arm span measured: {span:.2} m grip to grip");
                        self.settings.current.body.arm_span = span;
                        self.save_settings();
                        self.menu.set_message(format!("Arm span set to {span:.2} m"));
                        for h in 0..2 {
                            self.backend.haptic(h, 0.12, 160.0, 0.9);
                        }
                    }
                    None => {
                        self.menu.set_message("That didn't look like a T-pose: arms straight out at shoulder height, then try again");
                        for h in 0..2 {
                            self.backend.haptic(h, 0.1, 60.0, 0.6);
                        }
                    }
                }
            }
        }

        // ---- the body: IK from the head and the hands the rig drew, for the avatar and adapters
        let game_draws_body = gflags & GS_DRAWS_BODY != 0;
        self.draws_body = (rig_active || !alive) && !game_draws_body;
        if rig_active || self.draws_body {
            let hands = rig_active.then(|| {
                self.rig_out.hands.map(|h| (Vec3::from_array(h.pose.pos), quat(h.pose.rot)))
            });
            let ghosts = if rig_active { self.rig_out.ghosts.map(|g| g.map(|p| (Vec3::from_array(p.pos), quat(p.rot)))) } else { [None; 2] };
            let head_yaw = {
                let f = quat(frame.hmd.rot) * Vec3::Z;
                f.x.atan2(f.z)
            };
            let mv = Vec2::from_array(out.intents.move_xy);
            let locomotion = Quat::from_rotation_y(head_yaw) * Vec3::new(mv.x, 0.0, mv.y) * 2.5;
            let stand = self.mech.debug.stand_h;
            let floor = self.floor;
            // Hands holding a gun the game has its own hand pose for are drawn in it, where the rig
            // holds the gun (tracking space).
            let held = if rig_active {
                let grips = self.rig_out.hands.map(|h| (h.flags & HAND_TRACKED != 0).then(|| (Vec3::from_array(h.pose.pos), quat(h.pose.rot))));
                rustvr_avatar::held_hands(grips, &self.rig_out.hands, &self.states, &self.objects, &self.hand_poses, frame.hands.map(|h| h.trigger))
            } else {
                [None; 2]
            };
            let body = self.body.update(&crate::body::BodyInput {
                frame: &frame,
                hands,
                hand_states: rig_active.then_some(self.rig_out.hands),
                ghosts,
                holds: if rig_active { self.rig_out.holds } else { [rustvr_ik::HoldShape::None; 2] },
                held,
                locomotion,
                eye_height: if stand > 0.5 { stand } else { 0.0 },
                floor,
                settings: &self.settings.current,
            });
            self.link.write_body(body);
        }

        // ---- tell the adapter
        let mut rt = RtFrame {
            seq: 0,
            flags: RT_ACTIVE
                | if frame.user_present { RT_USER_PRESENT } else { 0 }
                | if self.backend.name() == "sim" { RT_SIMULATED } else { 0 }
                | if self.opts.lockstep { RT_LOCKSTEP } else { 0 }
                | if frame.dashboard { RT_DASHBOARD } else { 0 }
                | if menu_pointer.is_some() { RT_RUNTIME_MENU } else { 0 }
                | if self.draws_body { RT_DRAWS_BODY } else { 0 },
            frame_index: frame.index,
            display_time_ns: frame.display_time_ns,
            dt: frame.dt,
            refresh_hz: self.backend.refresh_hz(),
            hmd: frame.hmd,
            eyes: frame.eyes,
            fov: frame.fov,
            hands: frame.hands,
            tracking: out.tracking,
            intents: out.intents,
            eye_size: {
                let (w, h) = crate::targets::eye_size(self.backend.recommended_size(), &self.settings.current);
                [w, h]
            },
        };
        if frame.hmd.valid == 0 {
            rt.flags &= !RT_ACTIVE;
        }
        self.link.write_rt_frame(&rt);
        self.link.signal_frame();

        if self.opts.lockstep && alive && self.targets.is_some() {
            self.wait_for_adapter(frame.index);
        }
        if let Some(front) = self.link.take(self.front) {
            self.front = front;
            let info = self.link.read_slot_info(front);
            self.stats.slots += 1;
            self.stats.last_slot_frame = info.frame_index;
            self.pacing.picture();
            if self.targets.as_ref().is_some_and(|t| t.generation == info.generation) {
                self.shown = Some((front, info));
            }
            // The game's picture arrives this many of the runtime's frames after the frame it was
            // made for (and is shown in this one).
            if info.frame_index > 0 && info.frame_index <= frame.index {
                let lag = ((frame.index - info.frame_index) as f32 / self.backend.refresh_hz().max(30.0)).min(0.1);
                self.game_lag += (lag - self.game_lag) * 0.05;
                if frame.index % 900 == 0 {
                    tracing::info!("runtime: the game's picture is shown {:.0} ms after its frame", self.game_lag * 1000.0);
                }
            }
        }

        // ---- the runtime's menu
        if runtime_menu_open {
            let status = Status {
                game: name.filter(|n| !n.is_empty()),
                fps: self.fps,
                backend: self.backend.name().to_owned(),
                arm_span: self.body.arm_span(),
                head_height: self.mech.debug.stand_h,
                countdown: self.calibrate_at.map(|t| (t - now).max(0.0) as f32),
            };
            self.menu.update(&self.gpu, &mut self.settings.current, menu_pointer, &status)?;
            if std::mem::take(&mut self.menu.changed)
                && let Err(e) = self.settings.save() {
                    tracing::warn!("runtime: saving settings: {e:#}");
                }
            if std::mem::take(&mut self.menu.measure_arms) {
                self.calibrate_at = Some(now + 3.0);
                self.countdown_tick = -1;
            }
            if std::mem::take(&mut self.menu.measure_height) {
                let eye = frame.hmd.pos[1];
                if (1.0..2.3).contains(&eye) {
                    let height = eye + 0.11;
                    self.settings.current.body.height = height;
                    self.save_settings();
                    self.menu.set_message(format!("Height set to {height:.2} m"));
                } else {
                    self.menu.set_message("Stand up straight to measure your height");
                }
            }
            if std::mem::take(&mut self.menu.recenter) {
                self.mech.recenter();
            }
        }
        {
            let mut w = self.world.lock().unwrap();
            w.game_menu = layout.game_menu;
            w.runtime_menu = layout.runtime_menu;
            w.widgets = self.menu.widgets().to_vec();
            w.objects = if rig_active { self.rig.snapshot(&self.states) } else { Vec::new() };
        }

        self.update_comfort(&frame, &out, menu_mode || menu_pointer.is_some());

        // ---- compose
        let t_compose = std::time::Instant::now();
        self.compose(&frame, &out, &layout, pick.map(|(_, t, _)| laser_origin + laser_dir * t), menu_mode, alive)?;
        // A frame that took long (over a refresh and a half): where its time went, once a second.
        let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f32() * 1000.0;
        let t_end = std::time::Instant::now();
        let framework_shown = self.mech.framework_view && self.rig_active_last;
        self.pacing.cpu(ms(t_rig, t_end), framework_shown);
        if let Some(line) = self.pacing.report(now) {
            tracing::info!("runtime: {line}");
        }
        let budget = 1500.0 / self.backend.refresh_hz().max(30.0);
        if ms(t_rig, t_end) > budget && now - self.slow_logged > 1.0 {
            self.slow_logged = now;
            tracing::info!(
                "runtime: slow frame {:.1} ms (budget {budget:.1}): collision {t_collision:.1}, rig {:.1}, mechanics and the wait for the game {:.1}, compose {:.1}",
                ms(t_rig, t_end),
                ms(t_rig, t_mech) - t_collision,
                ms(t_mech, t_compose),
                ms(t_compose, t_end)
            );
        }
        self.stats.frames += 1;
        if menu_mode {
            self.stats.menu_frames += 1;
        }
        self.last_out = out;
        self.last_frame = frame;
        Ok(true)
    }

    /// The vignette (eased in while the stick moves you or turns you smoothly) and the fade when the
    /// head pushes into the game's walls.
    fn update_comfort(&mut self, frame: &HmdFrame, out: &crate::mechanics::MechOutput, menu: bool) {
        let c = &self.settings.current.comfort;
        let dt = frame.dt.max(1e-4);
        let moving = Vec2::from_array(out.intents.move_xy).length().min(1.0);
        let turning = if c.turn_mode == crate::settings::TurnMode::Smooth {
            let dom = self.settings.current.dominant();
            (frame.hands[dom].stick[0].abs() - self.settings.current.input.stick_deadzone).max(0.0) / (1.0 - self.settings.current.input.stick_deadzone).max(0.1)
        } else {
            0.0
        };
        // Walking the body after the head (roomscale) doesn't move the view: no vignette for it.
        let following = out.tracking.flags & TRK_FOLLOW != 0;
        let target = if menu || following { 0.0 } else { c.vignette * moving.max(turning).min(1.0) };
        let k = 1.0 - (-dt / c.vignette_ease.max(0.02)).exp();
        self.comfort.vignette += (target - self.comfort.vignette) * k;
        self.comfort.aperture = c.vignette_aperture;
        self.comfort.feather = c.vignette_feather;
        self.comfort.forward = quat(frame.hmd.rot) * Vec3::Z;
        let mut fade = 0.0;
        if c.wall_fade && self.rig.physics.has_world() && !menu {
            if let Some(map) = self.map_now {
                let head = map.point(Vec3::from_array(frame.hmd.pos));
                fade = (self.rig.physics.sphere_penetration(head, 0.12) / 0.08).clamp(0.0, 1.0);
            }
        }
        let kf = 1.0 - (-dt / 0.08).exp();
        if (fade > 0.5) != (self.comfort.fade > 0.5) {
            tracing::info!("runtime: the head is {} the world's geometry: fading {}", if fade > 0.5 { "inside" } else { "out of" }, if fade > 0.5 { "out" } else { "back in" });
        }
        self.comfort.fade += (fade - self.comfort.fade) * kf;
        self.stats.vignette = self.comfort.vignette;
        self.stats.fade = self.comfort.fade;
        if self.comfort.vignette > 0.1 {
            self.stats.vignette_frames += 1;
        }
        if self.comfort.fade > 0.3 {
            self.stats.fade_frames += 1;
        }
    }

    fn save_settings(&mut self) {
        if self.opts.settings_dir.is_some()
            && let Err(e) = self.settings.save() {
                tracing::warn!("runtime: saving settings: {e:#}");
            }
    }

    /// A new (or no) adapter: forget its objects and collision.
    fn reset_rig(&mut self) {
        self.rig = Rig::default();
        self.rig_out = RigOutput::default();
        self.objects_seq = 0;
        self.held_objects.clear();
        self.hand_poses.count = 0;
        self.hand_poses_seq = 0;
        self.kicks.count = 0;
        self.held_kicks.clear();
        self.hud_layout.count = 0;
        self.hud_layout_seq = 0;
        self.kicks_seq = 0;
        *self.sound_pack = bytemuck::Zeroable::zeroed();
        self.sound_pack_seq = 0;
        self.object_sounds.clear();
    }

    /// Loads the game's sound pack when it names one (and the setting allows), and goes back to
    /// the runtime's own sounds when it doesn't, or the game has gone.
    fn update_sound_pack(&mut self, alive: bool) {
        let Some(a) = &self.audio else { return };
        let path = self.sound_pack.path();
        let on = alive && self.settings.current.sound.game_sounds && !path.is_empty();
        let want = on.then(|| (PathBuf::from(path), self.sound_pack.revision));
        if want == self.pack_wanted {
            return;
        }
        match &want {
            Some((dir, rev)) => tracing::info!("runtime: loading the game's sounds from {} (revision {rev})", dir.display()),
            None => tracing::info!("runtime: the runtime's own sounds"),
        }
        // A new revision of the same folder: let go of the old copy, so it's read again.
        if let (Some((old, _)), Some((new, _))) = (&self.pack_wanted, &want)
            && old == new
        {
            a.load_pack(None);
        }
        a.load_pack(want.as_ref().map(|(d, _)| d.as_path()));
        self.pack_wanted = want;
    }

    /// The sound for one feedback cue: the object's own from the game's sound pack (its set, or
    /// the pack's defaults), else the runtime's synthesised one. A shot also plays its tail and
    /// its casing landing (the pack's; the runtime has none of its own), and none of it when the
    /// game plays its own gunshots; reloading sounds likewise when the game plays its own.
    fn play_cue(&mut self, fb: &feedback::Feedback) {
        use rustvr_audio::Event as E;
        let Some((event, fallback)) = event_of(fb.cue) else { return };
        let (set, flags) = fb.object.and_then(|id| self.object_sounds.get(&id)).copied().unwrap_or_default();
        if !runtime_plays(event, flags | self.sound_pack.flags, self.settings.current.sound.shots) {
            return;
        }
        let shot = event == E::Fire;
        self.sounds_played += 1;
        let Some(a) = &self.audio else { return };
        a.play_event(event, set, Some(fallback), fb.pos, fb.intensity);
        if shot {
            a.play_event(E::FireTail, set, None, fb.pos, fb.intensity);
            // The case lands on the floor under the gun.
            a.play_event(E::Casing, set, None, Vec3::new(fb.pos.x, self.floor + 0.02, fb.pos.z), fb.intensity);
        }
    }

    fn wait_for_adapter(&mut self, index: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(self.opts.lockstep_timeout_ms as u64);
        loop {
            // Peek without taking: the newest published slot is in the middle position.
            let published = self.link.published();
            if published > 0 {
                // Take it now if it's this frame's; otherwise keep waiting.
                if let Some(front) = self.link.take(self.front) {
                    self.front = front;
                    let info = self.link.read_slot_info(front);
                    self.stats.slots += 1;
                    self.stats.last_slot_frame = info.frame_index;
                    if self.targets.as_ref().is_some_and(|t| t.generation == info.generation) {
                        self.shown = Some((front, info));
                    }
                    if info.frame_index >= index {
                        return;
                    }
                }
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || !self.link.adapter_alive() {
                self.stats.lockstep_timeouts += 1;
                return;
            }
            self.link.wait_submit(left.as_millis().clamp(1, 50) as u32);
        }
    }

    fn compose(
        &mut self,
        frame: &HmdFrame,
        out: &MechOutput,
        layout: &panels::Frame,
        laser_end: Option<Vec3>,
        menu_mode: bool,
        alive: bool,
    ) -> Result<()> {
        let t_wait = std::time::Instant::now();
        let eyes = self.backend.eye_targets(&self.gpu)?;
        self.pacing.wait(crate::pacing::Wait::Image, t_wait.elapsed().as_secs_f32() * 1000.0);
        self.pacing.gpu_begin(&self.gpu);
        let shown = self.shown.and_then(|(slot, info)| self.targets.as_ref().map(|t| (&t.slots[slot], info)));
        let t_wait = std::time::Instant::now();
        let locked = shown.filter(|(slot, _)| slot.acquire(50));
        self.pacing.wait(crate::pacing::Wait::Picture, t_wait.elapsed().as_secs_f32() * 1000.0);
        let s = &self.settings.current;
        let stereo = locked.filter(|(_, i)| i.flags & SLOT_STEREO != 0 && !menu_mode && alive);
        let framework = self.mech.framework_view && self.rig_active_last;
        // Over the game's picture, everything is drawn from the eyes the game drew it from, and
        // submitted with them: the headset turns the whole frame for where the head is when it
        // shows. (The runtime's own view: this headset frame's eyes.)
        let (poses, fovs) = match &stereo {
            Some((_, info)) if !framework => (info.eye_pose, info.fov),
            _ => (frame.eyes, frame.fov),
        };

        let mut layers = Layers { comfort: self.comfort, ..Default::default() };
        if let (Some((slot, info)), Some(w)) = (&stereo, &out.weapon)
            && let (Some(scope), Some(aux)) = (&w.scope, &slot.aux)
                && info.flags & SLOT_AUX != 0 {
                    layers.lens = Some(LensLayer {
                        pos: scope.lens_pos,
                        right: scope.lens_right,
                        up: scope.lens_up,
                        radius: scope.lens_radius,
                        srv: aux.srv.as_ref().unwrap(),
                        aspect: aux.width as f32 / aux.height.max(1) as f32,
                        reticle: s.scope.reticle,
                    });
                    self.stats.scope_frames += 1;
                }
        let mut hud_shown = false;
        if let Some((slot, info)) = &locked
            && let Some(hud) = &slot.hud {
                if menu_mode {
                    if let Some(p) = layout.game_menu {
                        layers.panels.push(PanelLayer::whole(p, hud.srv.as_ref().unwrap()));
                    }
                } else if info.flags & SLOT_HUD != 0
                    && let Some(p) = layout.hud {
                        // The HUD in pieces: each where VR games put it (see `hud.rs`).
                        let dom = s.dominant();
                        let gun = out
                            .weapon
                            .as_ref()
                            .map(|w| (w.weapon_to_tracking, w.sight_origin, w.aim_dir))
                            .or_else(|| self.rig_out.primary_weapon.as_ref().map(|w| {
                                let m = w.weapon_to_tracking;
                                (m, m.transform_point3(w.sight), m.transform_vector3(Vec3::Z).normalize_or(Vec3::Z))
                            }));
                        let off = &frame.hands[dom ^ 1];
                        let at = crate::hud::Attach {
                            gun,
                            off_hand: (off.connected != 0).then(|| (Vec3::from_array(off.grip.pos), quat(off.grip.rot))),
                        };
                        let aspect = p.width / p.height.max(1e-4);
                        let pieces = self.hud.place(
                            s,
                            Vec3::from_array(frame.hmd.pos),
                            quat(frame.hmd.rot),
                            frame.dt,
                            aspect,
                            // In pieces only over the game's stereo picture: a loading screen or an
                            // intro (flat frames) is one screen.
                            (self.hud_layout.count > 0 && info.flags & SLOT_STEREO != 0).then_some(&*self.hud_layout),
                            &at,
                        );
                        self.stats.hud_piece_count = 0;
                        for piece in pieces {
                            if piece.opacity > 0.0 && self.stats.hud_piece_count < self.stats.hud_pieces.len() {
                                self.stats.hud_pieces[self.stats.hud_piece_count] = (piece.role, piece.panel.center);
                                self.stats.hud_piece_count += 1;
                            }
                            layers.panels.push(PanelLayer { panel: piece.panel, srv: hud.srv.as_ref().unwrap(), opacity: piece.opacity, uv: piece.uv });
                        }
                        hud_shown = true;
                    }
            }
        if !hud_shown {
            self.hud.reset();
        }
        if let Some(p) = layout.runtime_menu {
            layers.panels.push(PanelLayer::whole(p, self.menu.srv()));
        }
        let dom = s.dominant();
        // The gun's own laser (the game's attachment, fitted and on), or the runtime's laser sight
        // from the muzzle when that's turned on.
        let gun_laser = out.weapon.as_ref().filter(|_| !menu_mode).and_then(|w| w.laser.or(s.interaction.laser_sight.then_some(w.muzzle)).map(|o| (w, o)));
        if let Some((w, origin)) = gun_laser {
            // Up to 30 m, or the first thing in the game's collision.
            let hit = self.map_now.and_then(|map| {
                let o = map.point(origin);
                self.rig.physics.ray_world(o, map.rot(Quat::IDENTITY) * w.aim_dir, 30.0).map(|d| d / map.world_per_meter)
            });
            layers.gun_laser = Some(Laser { origin, end: origin + w.aim_dir * hit.unwrap_or(30.0), hit: hit.is_some() });
        }
        if layout.game_menu.is_some() || layout.runtime_menu.is_some() {
            let hand = &frame.hands[dom];
            if hand.connected != 0 {
                let o = Vec3::from_array(hand.aim.pos);
                let d = quat(hand.aim.rot) * Vec3::Z;
                layers.laser = Some(Laser { origin: o, end: laser_end.unwrap_or(o + d * 3.0), hit: laser_end.is_some() });
            }
        }
        if stereo.is_none() {
            for h in &frame.hands {
                if h.connected != 0 {
                    layers.markers.push((Vec3::from_array(h.grip.pos), quat(h.grip.rot)));
                }
            }
        }

        // The player's body goes between the game's picture and the overlays, behind the game's
        // geometry where the game gave its depth.
        // (A game that draws the body in its own picture has none in the framework view: the runtime
        // draws it there.)
        let draw_body = (self.draws_body || framework) && !menu_mode;
        let (wpm, light) = match (&self.game, self.map_now) {
            (Some(g), Some(map)) => {
                let mut l = g.light;
                l.sun_dir = (map.inv_rot(Quat::IDENTITY) * Vec3::from_array(l.sun_dir)).to_array();
                (map.world_per_meter, l)
            }
            _ => (1.0, LightHint::default()),
        };
        // The game's lighting for the body (the same for both eyes).
        let shading = (draw_body && self.has_game_lighting && s.body.game_lighting).then(|| rustvr_avatar::GameShading {
            lighting: &self.game_lighting,
            world_rot: self.map_now.map_or(Quat::IDENTITY, |m| m.rot(Quat::IDENTITY)),
            atlas: stereo.as_ref().and_then(|(slot, _)| slot.lights.as_ref()).and_then(|t| t.srv.as_ref()),
        });
        // The framework view: the physics world in tracking space instead of the game's picture,
        // the body and hands plain (the runtime's own look) and the guns as their collision.
        self.body.set_game_shading(&self.gpu, shading.as_ref().filter(|_| !framework));
        self.body.set_plain(framework);
        let held = if draw_body && !framework { self.held_objects.draws() } else { Vec::new() };
        self.stats.held_drawn = held.len() as u32;
        self.stats.held_drawn_frames += (!held.is_empty()) as u64;
        if framework {
            let map = self.map_now;
            self.physics_tris.clear();
            self.rig.physics_triangles(&mut self.physics_tris);
            let mut marks = Vec::new();
            self.rig.mechanism_markers(&mut marks);
            let to_tracking = |(t, c): &([Vec3; 3], [f32; 3])| (map.as_ref().map_or(*t, |m| t.map(|p| m.inv_point(p))), *c);
            // The guns the runtime draws, as their low-poly selves (tracking space already).
            let mut guns = Vec::new();
            self.held_objects.framework_triangles(&mut guns);
            let tris = self.physics_tris.iter().map(to_tracking).chain(guns);
            if let Err(e) = self.physics_view.set_triangles(&self.gpu, tris, marks.iter().map(to_tracking)) {
                tracing::warn!("framework view: {e:#}");
            }
        }
        let body = &mut self.body;
        let physics_view = &mut self.physics_view;
        for e in 0..2 {
            let target = &eyes[e];
            let (eye, fov) = (&poses[e], &fovs[e]);
            // Behind it: the game's picture as it drew it (and its depth, which the body hides
            // behind), or the runtime's own view.
            let mut background = crate::compositor::Background::Void;
            let mut depth_srv = None;
            if let (Some((slot, info)), false) = (&stereo, framework) {
                if let Some(srv) = slot.eyes[e].srv.as_ref() {
                    background = crate::compositor::Background::Game(srv);
                }
                depth_srv = (info.flags & SLOT_DEPTH != 0).then(|| slot.depth.as_ref().and_then(|d| d[e].srv.clone())).flatten();
            }
            let mut depth = depth_srv.as_ref().map(|srv| rustvr_avatar::DepthInput { srv, units_per_meter: wpm });
            self.comp.draw_view(&self.gpu, target, eye, fov, background, &layers, true, &mut |gpu| {
                if framework {
                    if let Err(e) = physics_view.draw(gpu, target, eye, fov) {
                        tracing::warn!("framework view: {e:#}");
                    }
                }
                if draw_body {
                    body.draw(gpu, target, eye, fov, light, if framework { None } else { depth.take() }, &held);
                }
            });
        }
        // The desktop view: the same frame from its steadied camera between the eyes.
        if let Some(target) = self.spectator.as_ref().and_then(|w| w.target()) {
            let sp = &s.spectator;
            let head = quat(frame.hmd.rot);
            let cam = self.spectator_cam.update(head, crate::spectator::between(&frame.eyes), frame.dt, sp);
            let fov = crate::spectator::fov(sp.fov, target.width as f32 / target.height.max(1) as f32);
            let first = match sp.eye {
                crate::settings::Eye::Left => 0,
                crate::settings::Eye::Right => 1,
            };
            let background = match (&stereo, framework) {
                (Some((slot, info)), false) => {
                    let eye = |e: usize| slot.eyes[e].srv.as_ref().map(|srv| (srv, info.eye_pose[e], info.fov[e]));
                    match (eye(first), eye(first ^ 1)) {
                        (Some(a), b) => crate::compositor::Background::Reprojected { first: a, second: b },
                        (None, _) => crate::compositor::Background::Void,
                    }
                }
                _ => crate::compositor::Background::Void,
            };
            self.comp.draw_view(&self.gpu, target, &cam, &fov, background, &layers, false, &mut |gpu| {
                if framework && let Err(e) = physics_view.draw(gpu, target, &cam, &fov) {
                    tracing::warn!("framework view: {e:#}");
                }
                if draw_body {
                    body.draw(gpu, target, &cam, &fov, light, None, &held);
                }
            });
        }
        if let Some((slot, info)) = &locked {
            slot.release();
            if stereo.is_some() {
                self.stats.game_frames += 1;
                if info.frame_index == frame.index {
                    self.stats.exact_frames += 1;
                }
            }
        }
        self.pacing.gpu_end(&self.gpu);
        let t_wait = std::time::Instant::now();
        self.backend.submit(&self.gpu, &poses, &fovs)?;
        self.pacing.wait(crate::pacing::Wait::Submit, t_wait.elapsed().as_secs_f32() * 1000.0);
        if let Some(w) = &self.spectator {
            let now = frame.display_time_ns as f64 * 1e-9;
            if let Some((dir, last)) = &mut self.spectator_capture
                && now - *last > 5.0
            {
                *last = now;
                let path = dir.join(format!("desktop_{:06}.png", frame.index));
                if let Err(e) = std::fs::create_dir_all(dir).map_err(anyhow::Error::from).and_then(|_| w.save(&self.gpu, &path)) {
                    tracing::warn!("desktop view: saving {}: {e:#}", path.display());
                }
            }
            w.present();
        }
        Ok(())
    }
}

/// Keeps the runtime's heartbeat going while the frame loop is blocked (OpenXR can hold
/// `xrWaitFrame` for seconds while the headset sleeps), so adapters don't take it for gone.
struct Heartbeat {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Heartbeat {
    fn start(mapping: &str) -> Heartbeat {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let name = mapping.to_owned();
        let thread = std::thread::Builder::new()
            .name("rustvr-heartbeat".into())
            .spawn(move || {
                let Ok(link) = Link::open(&name) else { return };
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    link.heartbeat_runtime();
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            })
            .ok();
        Heartbeat { stop, thread }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Whether the runtime plays `event` at all: not a shot when gunshots are off or the game plays
/// its own (`SND_GAME_SHOTS`), not a reloading or handling sound when the game plays those
/// (`SND_GAME_HANDLING`). `flags` are the pack's and the object's together.
fn runtime_plays(event: rustvr_audio::Event, flags: u32, shots: bool) -> bool {
    use rustvr_audio::Event as E;
    match event {
        // An explosion is the game's too (its grenade went off), and it plays with the shots.
        E::Fire | E::FireTail | E::Casing | E::Explosion => shots && flags & SND_GAME_SHOTS == 0,
        E::MagOut | E::MagIn | E::BoltBack | E::BoltForward | E::DryFire => flags & SND_GAME_HANDLING == 0,
        _ => true,
    }
}

/// The sound pack event for a cue, and the runtime's own sound for it (None: the cue is felt, not
/// heard).
fn event_of(c: feedback::Cue) -> Option<(rustvr_audio::Event, rustvr_audio::Sound)> {
    use feedback::Cue as C;
    use rustvr_audio::{Event as E, Sound as S};
    Some(match c {
        C::Grab => (E::Grab, S::Grab),
        C::Release => (E::Release, S::Release),
        C::HolsterIn => (E::HolsterIn, S::HolsterIn),
        C::HolsterOut => (E::HolsterOut, S::HolsterOut),
        C::MagEject => (E::MagOut, S::MagEject),
        C::MagSeat => (E::MagIn, S::MagSeat),
        C::SlideBack => (E::BoltBack, S::SlideBack),
        C::SlideForward => (E::BoltForward, S::SlideForward),
        C::LastRound => (E::LastRound, S::LastRound),
        C::EmptyClick => (E::DryFire, S::EmptyClick),
        C::Fail => (E::Fail, S::Fail),
        C::GloveHover => (E::GloveHover, S::GloveHover),
        C::GloveLock => (E::GloveLock, S::GloveLock),
        C::GloveFlick => (E::GloveFlick, S::GloveFlick),
        C::GloveCatch => (E::GloveCatch, S::GloveCatch),
        C::Impact => (E::Impact, S::Impact),
        C::ZoneTick => (E::ZoneTick, S::ZoneTick),
        C::Pouch => (E::Pouch, S::ZoneTick),
        C::ShotPistol => (E::Fire, S::ShotPistol),
        C::ShotRifle => (E::Fire, S::ShotRifle),
        C::ShotShotgun => (E::Fire, S::ShotShotgun),
        C::Melee => (E::Melee, S::Melee),
        C::GrenadePin => (E::GrenadePin, S::SlideBack),
        C::GrenadeArm => (E::GrenadeArm, S::LastRound),
        C::GrenadeTick => (E::GrenadeTick, S::ZoneTick),
        C::GrenadeThrow => (E::GrenadeThrow, S::GloveFlick),
        C::KnifeThrow => (E::KnifeThrow, S::GloveFlick),
        C::Explosion => (E::Explosion, S::ShotShotgun),
        C::KnifeStick => (E::KnifeStick, S::Impact),
        C::ExplosionRumble => return None,
        // The actions' cues play the nearest handling event of the pack (the gun's own bolt and
        // reload sounds) until the audio crate gives them events of their own by the same names.
        C::BreakOpen | C::SwingOpen | C::BoltUp | C::BoltBack | C::PumpBack | C::Pump => (E::BoltBack, S::SlideBack),
        C::BreakClose | C::SwingClose | C::BoltForward | C::BoltDown | C::PumpForward => (E::BoltForward, S::SlideForward),
        C::ShellIn => (E::MagIn, S::MagSeat),
        C::ShellOut | C::CaseEject => (E::MagOut, S::MagEject),
        C::AirHiss => (E::DryFire, S::EmptyClick),
    })
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustvr_audio::Event as E;

    #[test]
    fn the_game_playing_its_own_sounds_silences_the_runtimes() {
        // The range plays nothing of its own: the runtime plays everything.
        assert!(runtime_plays(E::Fire, 0, true) && runtime_plays(E::MagIn, 0, true) && runtime_plays(E::Grab, 0, true));
        // A game that fires the runtime's shots through its own weapon code plays their sound.
        assert!(!runtime_plays(E::Fire, SND_GAME_SHOTS, true) && !runtime_plays(E::Casing, SND_GAME_SHOTS, true));
        assert!(runtime_plays(E::MagOut, SND_GAME_SHOTS, true) && runtime_plays(E::Grab, SND_GAME_SHOTS, true));
        // One that animates its own reloads plays those.
        for e in [E::MagOut, E::MagIn, E::BoltBack, E::BoltForward, E::DryFire] {
            assert!(!runtime_plays(e, SND_GAME_HANDLING, true), "{e:?}");
        }
        assert!(runtime_plays(E::Fire, SND_GAME_HANDLING, true));
        // And the player can turn gunshots off.
        assert!(!runtime_plays(E::Fire, 0, false) && runtime_plays(E::MagIn, 0, false));
    }

    #[test]
    fn every_cue_has_a_sound_the_pack_can_replace() {
        use feedback::Cue as C;
        let cues = [
            C::Grab, C::Release, C::HolsterIn, C::HolsterOut, C::MagEject, C::MagSeat, C::SlideBack, C::SlideForward, C::LastRound, C::EmptyClick,
            C::Fail, C::GloveHover, C::GloveLock, C::GloveFlick, C::GloveCatch, C::Impact, C::ZoneTick, C::Pouch, C::ShotPistol, C::ShotRifle,
            C::ShotShotgun, C::Melee, C::BreakOpen, C::BreakClose, C::SwingOpen, C::SwingClose, C::ShellIn, C::ShellOut, C::CaseEject, C::BoltUp,
            C::BoltBack, C::BoltForward, C::BoltDown, C::PumpBack, C::PumpForward, C::Pump, C::AirHiss,
        ];
        for c in cues {
            let Some((event, fallback)) = event_of(c) else { continue };
            // The fallback is the runtime's own sound for that event (a shot keeps its kind).
            if event != E::Fire && c != C::Pouch {
                assert_eq!(event.fallback(), Some(fallback), "{c:?}");
            }
        }
        assert_eq!(event_of(C::ShotPistol), Some((E::Fire, rustvr_audio::Sound::ShotPistol)));
    }
}

