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
}

impl Default for Options {
    fn default() -> Self {
        Options { mapping: DEFAULT_MAPPING.to_owned(), settings_dir: None, lockstep: false, lockstep_timeout_ms: 2000, audio: false }
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
    objects_seq: u32,
    /// The game's own hand poses for its objects (`ObjectDesc::grip_pose`, `support_pose`).
    hand_poses: Box<HandPoseTable>,
    hand_poses_seq: u32,
    kicks: Box<KickTable>,
    kicks_seq: u32,
    audio: Option<rustvr_audio::Audio>,
    comfort: Comfort,
    menu_trigger_was: bool,
    /// The player's body (IK) and its avatar.
    pub body: crate::body::PlayerBody,
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
        Ok(Session {
            comp: Compositor::new(&gpu)?,
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
            objects_seq: 0,
            hand_poses: bytemuck::zeroed_box(),
            hand_poses_seq: 0,
            kicks: bytemuck::zeroed_box(),
            kicks_seq: 0,
            audio: opts.audio.then(rustvr_audio::Audio::new),
            comfort: Comfort::default(),
            menu_trigger_was: false,
            body: crate::body::PlayerBody::default(),
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
            was_alive: false,
        })
    }

    pub fn adapter_connected(&self) -> bool {
        self.was_alive
    }

    /// One headset frame. Ok(false) when the backend has finished.
    pub fn step(&mut self) -> Result<bool> {
        let frame = match self.backend.wait_frame() {
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
        self.link.heartbeat_runtime();
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
                        tracing::info!("runtime: targets generation {} {}x{} flags {:#x}", self.generation, spec.eye_w, spec.eye_h, spec.flags);
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
        // A click on either panel: a soft UI click where the laser hits.
        if let (Some((_, t, _)), Some(a)) = (pick, &self.audio)
            && trigger_down && !self.menu_trigger_was && self.settings.current.sound.enabled {
                a.play(rustvr_audio::Sound::UiClick, laser_origin + laser_dir * t, 1.0);
            }
        self.menu_trigger_was = trigger_down;

        // ---- the rig: the game's collision and objects, then the hands
        if alive {
            let physics = &mut self.rig.physics;
            self.link.drain_collision(|kind, payload| physics.apply_collision(kind, payload));
            let seq = self.link.objects_seq();
            if seq != self.objects_seq && self.link.read_objects_into(&mut self.objects) {
                self.objects_seq = seq;
                self.rig.sync(&self.objects);
                tracing::info!("runtime: {} objects registered (table {})", self.objects.count, seq);
            }
            let seq = self.link.hand_poses_seq();
            if seq != self.hand_poses_seq && self.link.read_hand_poses_into(&mut self.hand_poses) {
                self.hand_poses_seq = seq;
                tracing::info!("runtime: {} hand poses registered (table {})", self.hand_poses.count, seq);
            }
            let seq = self.link.kicks_seq();
            if seq != self.kicks_seq && self.link.read_kicks_into(&mut self.kicks) {
                self.kicks_seq = seq;
                self.rig.set_kicks(&self.kicks);
                tracing::info!("runtime: {} recoil kicks registered (table {})", self.kicks.count, seq);
            }
        }
        let rig_active = alive && self.rig.has_objects() && self.game.is_some();
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
            let g = self.game.as_ref().unwrap();
            let stand = self.mech.debug.stand_h - floor;
            self.rig_out = self.rig.update(&RigInput {
                frame: &frame,
                map: WorldMap::from_state(&g.tracking),
                dt: frame.dt,
                now,
                settings: &self.settings.current,
                eye_height: if stand > 1.0 { stand } else { 1.7 },
                crouched: g.flags & GS_CROUCHED != 0,
                paused: menu_mode || menu_pointer.is_some() || g.flags & (GS_PAUSED | GS_LOADING | GS_DEAD) != 0,
                infinite_ammo: g.flags & GS_INFINITE_AMMO != 0,
            });
            // One event, one frame: the pulse and the sound together.
            let snd = &self.settings.current.sound;
            if let Some(a) = &self.audio {
                a.set_volume(if snd.enabled { snd.volume } else { 0.0 });
                a.set_listener(rustvr_audio::Listener { pos: Vec3::from_array(frame.hmd.pos), rot: quat(frame.hmd.rot) });
            }
            for fb in &self.rig.feedback {
                if let (Some(hand), Some((secs, freq, amp))) = (fb.hand, feedback::haptic(fb.cue, fb.intensity)) {
                    self.backend.haptic(hand, secs, freq, amp * fb.intensity.clamp(0.0, 1.0).max(0.3));
                }
                if let Some(sound) = sound_of(fb.cue).filter(|_| snd.enabled && (snd.shots || !is_shot(fb.cue))) {
                    self.sounds_played += 1;
                    if let Some(a) = &self.audio {
                        a.play(sound, fb.pos, fb.intensity);
                    }
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
            self.rig.write_states(&mut self.states);
            self.link.write_object_states(&self.states);
        }

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
            if self.targets.as_ref().is_some_and(|t| t.generation == info.generation) {
                self.shown = Some((front, info));
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
        self.compose(&frame, &out, &layout, pick.map(|(_, t, _)| laser_origin + laser_dir * t), menu_mode, alive)?;
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
            if let Some(g) = &self.game {
                let head = WorldMap::from_state(&g.tracking).point(Vec3::from_array(frame.hmd.pos));
                fade = (self.rig.physics.sphere_penetration(head, 0.12) / 0.08).clamp(0.0, 1.0);
            }
        }
        let kf = 1.0 - (-dt / 0.08).exp();
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
        self.hand_poses.count = 0;
        self.hand_poses_seq = 0;
        self.kicks.count = 0;
        self.kicks_seq = 0;
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
        let eyes = self.backend.eye_targets(&self.gpu)?;
        let shown = self.shown.and_then(|(slot, info)| self.targets.as_ref().map(|t| (&t.slots[slot], info)));
        let locked = shown.filter(|(slot, _)| slot.acquire(50));
        let s = &self.settings.current;
        let stereo = locked.filter(|(_, i)| i.flags & SLOT_STEREO != 0 && !menu_mode && alive);
        let (poses, fovs) = match &stereo {
            Some((_, info)) => (info.eye_pose, info.fov),
            None => (frame.eyes, frame.fov),
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
        if let Some((slot, info)) = &locked
            && let Some(hud) = &slot.hud {
                if menu_mode {
                    if let Some(p) = layout.game_menu {
                        layers.panels.push(PanelLayer { panel: p, srv: hud.srv.as_ref().unwrap(), opacity: 1.0 });
                    }
                } else if info.flags & SLOT_HUD != 0
                    && let Some(p) = layout.hud {
                        layers.panels.push(PanelLayer { panel: p, srv: hud.srv.as_ref().unwrap(), opacity: 1.0 });
                    }
            }
        if let Some(p) = layout.runtime_menu {
            layers.panels.push(PanelLayer { panel: p, srv: self.menu.srv(), opacity: 1.0 });
        }
        let dom = s.dominant();
        if let (true, Some(w), false) = (s.interaction.laser_sight, &out.weapon, menu_mode) {
            // Up to 30 m, or the first thing in the game's collision.
            let hit = self.game.as_ref().and_then(|g| {
                let map = WorldMap::from_state(&g.tracking);
                let o = map.point(w.muzzle);
                self.rig.physics.ray_world(o, map.rot(Quat::IDENTITY) * w.aim_dir, 30.0).map(|d| d / map.world_per_meter)
            });
            layers.gun_laser = Some(Laser { origin: w.muzzle, end: w.muzzle + w.aim_dir * hit.unwrap_or(30.0), hit: hit.is_some() });
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
        let draw_body = self.draws_body && !menu_mode;
        let (wpm, light) = match &self.game {
            Some(g) => {
                let map = WorldMap::from_state(&g.tracking);
                let mut l = g.light;
                l.sun_dir = (map.inv_rot(Quat::IDENTITY) * Vec3::from_array(l.sun_dir)).to_array();
                (map.world_per_meter, l)
            }
            None => (1.0, LightHint::default()),
        };
        let body = &mut self.body;
        for e in 0..2 {
            let game = stereo.as_ref().and_then(|(slot, _)| slot.eyes[e].srv.as_ref());
            let mut depth = stereo
                .as_ref()
                .filter(|(_, info)| info.flags & SLOT_DEPTH != 0)
                .and_then(|(slot, _)| slot.depth.as_ref())
                .and_then(|d| d[e].srv.as_ref())
                .map(|srv| rustvr_avatar::DepthInput { srv, units_per_meter: wpm });
            let target = &eyes[e];
            let (eye, fov) = (&poses[e], &fovs[e]);
            self.comp.draw_eye(&self.gpu, target, eye, fov, game, &layers, &mut |gpu| {
                if draw_body {
                    body.draw(gpu, target, eye, fov, light, depth.take());
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
        self.backend.submit(&self.gpu, &poses, &fovs)?;
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

fn sound_of(c: feedback::Cue) -> Option<rustvr_audio::Sound> {
    use feedback::Cue as C;
    use rustvr_audio::Sound as S;
    Some(match c {
        C::Grab => S::Grab,
        C::Release => S::Release,
        C::HolsterIn => S::HolsterIn,
        C::HolsterOut => S::HolsterOut,
        C::MagEject => S::MagEject,
        C::MagSeat => S::MagSeat,
        C::SlideBack => S::SlideBack,
        C::SlideForward => S::SlideForward,
        C::LastRound => S::LastRound,
        C::EmptyClick => S::EmptyClick,
        C::Fail => S::Fail,
        C::GloveHover => S::GloveHover,
        C::GloveLock => S::GloveLock,
        C::GloveFlick => S::GloveFlick,
        C::GloveCatch => S::GloveCatch,
        C::Impact => S::Impact,
        C::ZoneTick => S::ZoneTick,
        C::ShotPistol => S::ShotPistol,
        C::ShotRifle => S::ShotRifle,
        C::ShotShotgun => S::ShotShotgun,
        C::Melee => S::Melee,
    })
}

fn is_shot(c: feedback::Cue) -> bool {
    matches!(c, feedback::Cue::ShotPistol | feedback::Cue::ShotRifle | feedback::Cue::ShotShotgun)
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}
