//! The adapter SDK: the shared code that runs inside a game (or the test range) and connects it to
//! the RustVR runtime.
//!
//! A game adapter's per-frame loop:
//!
//! ```ignore
//! let frame = adapter.poll()?;                         // latest headset frame + intents
//! let view = adapter.tracking.update(&frame, &anchor, now, dt)?;   // tracking -> this frame's world
//! // drive the game: look angles from `view`, actions from `frame.intents`, the gun from
//! // `view.weapon_to_world(&frame.intents)`, shots along `view.point(sight_origin)` / `view.dir(aim_dir)`
//! adapter.write_state(&mut state);                     // what the game is doing
//! if let Some(slot) = adapter.begin_slot() { /* render eyes (and HUD, scope) into slot */ }
//! adapter.submit(info);                                // hand the frame to the runtime
//! ```
//!
//! Everything VR-specific (hands, aiming, gestures, comfort) is decided by the runtime; the
//! adapter only translates.

pub mod actions;
pub mod basis;
pub mod math;
pub mod tracking;

pub use basis::Basis;
pub use tracking::{Anchor, TrackingMapper, View};

use rustvr_gpu::{Gpu, Texture, texture::color_family};
use rustvr_protocol::link::Link;
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT};

pub struct AdapterConfig {
    /// Adapter name: shown in logs and selects the runtime's settings profile.
    pub name: String,
    /// Mapping name; [`DEFAULT_MAPPING`] unless testing several runtimes side by side.
    pub mapping: String,
    pub request: TargetRequest,
}

impl AdapterConfig {
    pub fn new(name: &str) -> AdapterConfig {
        let mapping = std::env::var("RUSTVR_LINK").unwrap_or_else(|_| DEFAULT_MAPPING.to_owned());
        AdapterConfig { name: name.to_owned(), mapping, request: TargetRequest::default() }
    }
}

/// The textures of one slot, viewed for rendering: colour as UNORM render targets.
pub struct SlotTextures {
    pub eyes: [Texture; 2],
    pub depth: Option<[Texture; 2]>,
    pub hud: Option<Texture>,
    pub aux: Option<Texture>,
}

impl SlotTextures {
    fn all(&self) -> impl Iterator<Item = &Texture> {
        self.eyes
            .iter()
            .chain(self.depth.iter().flatten())
            .chain(self.hud.iter())
            .chain(self.aux.iter())
    }
}

struct OpenTargets {
    targets: Targets,
    slots: Vec<SlotTextures>,
}

pub struct Adapter {
    pub cfg: AdapterConfig,
    gpu: Gpu,
    link: Option<Link>,
    epoch: u32,
    connected: bool,
    open: Option<OpenTargets>,
    back: usize,
    back_acquired: bool,
    last_frame_index: u64,
    last_frame: Option<RtFrame>,
    retry_at: std::time::Instant,
    /// A target generation that failed to open, and when to try it again.
    targets_failed: Option<(u32, std::time::Instant)>,
    state_frame: u64,
    connections: u32,
    pub tracking: TrackingMapper,
    pub presses: actions::Presses,
}

impl Adapter {
    pub fn new(gpu: Gpu, cfg: AdapterConfig) -> Adapter {
        Adapter {
            cfg,
            gpu,
            link: None,
            epoch: 0,
            connected: false,
            open: None,
            back: 0,
            back_acquired: false,
            last_frame_index: 0,
            last_frame: None,
            retry_at: std::time::Instant::now(),
            targets_failed: None,
            state_frame: 0,
            connections: 0,
            tracking: TrackingMapper::default(),
            presses: actions::Presses::default(),
        }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// Connected to a live runtime that has acknowledged us.
    pub fn connected(&self) -> bool {
        self.connected
    }

    /// Counts connections: a change means a (re)started runtime that knows nothing of this game
    /// yet (register objects and collision again).
    pub fn connections(&self) -> u32 {
        self.connections
    }

    /// Keeps the connection alive and returns the latest headset frame, or None when no runtime is
    /// running (play flat then). Cheap; call every game frame.
    pub fn poll(&mut self) -> Option<RtFrame> {
        self.maintain();
        if !self.connected {
            return None;
        }
        let link = self.link.as_ref()?;
        let f = link.read_rt_frame()?;
        self.last_frame_index = f.frame_index;
        self.last_frame = Some(f);
        Some(f)
    }

    /// Lockstep: waits up to `timeout_ms` for a frame newer than the last one returned.
    pub fn wait_frame(&mut self, timeout_ms: u32) -> Option<RtFrame> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        loop {
            self.maintain();
            if self.connected
                && let Some(f) = self.link.as_ref().and_then(|l| l.read_rt_frame())
                    && f.frame_index != self.last_frame_index {
                        self.last_frame_index = f.frame_index;
                        self.last_frame = Some(f);
                        return Some(f);
                    }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            match &self.link {
                Some(l) if self.connected => {
                    l.wait_frame(left.as_millis().min(20) as u32);
                }
                _ => std::thread::sleep(left.min(std::time::Duration::from_millis(20))),
            }
        }
    }

    pub fn last_frame(&self) -> Option<&RtFrame> {
        self.last_frame.as_ref()
    }

    fn maintain(&mut self) {
        let now = std::time::Instant::now();
        if self.link.is_none() {
            if now < self.retry_at {
                return;
            }
            self.retry_at = now + std::time::Duration::from_millis(500);
            match Link::open(&self.cfg.mapping) {
                Ok(l) => {
                    tracing::info!("rustvr: found runtime (pid {}) on {}", l.runtime_pid(), self.cfg.mapping);
                    self.epoch = l.begin_connect();
                    self.link = Some(l);
                }
                Err(_) => return,
            }
        }
        let link = self.link.as_ref().unwrap();
        link.heartbeat_adapter();
        if !link.runtime_alive() {
            if self.connected {
                tracing::warn!("rustvr: runtime stopped responding");
            }
            self.disconnect_soft();
            return;
        }
        if !link.connect_acked(self.epoch) {
            if self.connected {
                // The runtime restarted (it reset its ack): announce ourselves again.
                tracing::info!("rustvr: runtime restarted, reconnecting");
                self.disconnect_soft();
                self.epoch = self.link.as_ref().unwrap().begin_connect();
            }
            return;
        }
        if !self.connected {
            tracing::info!("rustvr: connected as '{}'", self.cfg.name);
            self.connected = true;
            self.connections += 1;
            self.back = 0;
            self.back_acquired = false;
            self.presses.reset();
        }
        self.refresh_targets();
    }

    fn disconnect_soft(&mut self) {
        self.release_back();
        self.connected = false;
        self.open = None;
    }

    fn refresh_targets(&mut self) {
        let Some(t) = self.link.as_ref().and_then(|l| l.read_targets()) else { return };
        if t.generation == 0 || self.open.as_ref().is_some_and(|o| o.targets.generation == t.generation) {
            return;
        }
        // Opening shared textures is slow: a generation that won't open is retried once a second,
        // not every frame (which would stall the game), and its error is reported once.
        let now = std::time::Instant::now();
        if self.targets_failed.is_some_and(|(g, at)| g == t.generation && now < at) {
            return;
        }
        self.release_back();
        self.open = None;
        let mapping = self.cfg.mapping.clone();
        match open_targets(&self.gpu, &mapping, &t) {
            Ok(slots) => {
                tracing::info!("rustvr: targets generation {} ({}x{} per eye)", t.generation, t.eye_w, t.eye_h);
                self.open = Some(OpenTargets { targets: t, slots });
                self.targets_failed = None;
            }
            Err(e) => {
                if self.targets_failed.is_none_or(|(g, _)| g != t.generation) {
                    tracing::warn!("rustvr: can't open targets generation {} yet: {e:#}", t.generation);
                    // Diagnose: does a device of our own, in this process, see them?
                    let fresh = Gpu::new().and_then(|g| open_targets(&g, &mapping, &t).map(|_| g.adapter_name()));
                    tracing::warn!("rustvr: a fresh device here: {}", match fresh { Ok(n) => format!("opens them ({n})"), Err(e) => format!("{e:#}") });
                }
                self.targets_failed = Some((t.generation, now + std::time::Duration::from_secs(1)));
            }
        }
    }

    pub fn targets(&self) -> Option<&Targets> {
        self.open.as_ref().map(|o| &o.targets)
    }

    /// Reports the game's state. Fills in the name, the texture request, the tracking state and
    /// the frame counter.
    pub fn write_state(&mut self, state: &mut GameState) {
        let Some(link) = &self.link else { return };
        set_name(&mut state.name, &self.cfg.name);
        state.request = self.cfg.request;
        state.tracking = self.tracking.state();
        self.state_frame += 1;
        state.frame = self.state_frame;
        link.write_game_state(state);
    }

    /// The back slot's textures, locked for rendering. None while not connected or the targets
    /// aren't open.
    pub fn begin_slot(&mut self) -> Option<&SlotTextures> {
        if !self.connected {
            return None;
        }
        let open = self.open.as_ref()?;
        let slot = &open.slots[self.back];
        if !self.back_acquired {
            for t in slot.all() {
                if !t.acquire(100) {
                    tracing::warn!("rustvr: slot {} is busy", self.back);
                    for t2 in slot.all() {
                        if std::ptr::eq(t2, t) {
                            break;
                        }
                        t2.release();
                    }
                    return None;
                }
            }
            self.back_acquired = true;
        }
        Some(&self.open.as_ref().unwrap().slots[self.back])
    }

    /// Publishes the back slot. `info.generation` is filled in.
    pub fn submit(&mut self, mut info: SlotInfo) -> bool {
        if !self.back_acquired {
            return false;
        }
        let (Some(link), Some(open)) = (&self.link, &self.open) else { return false };
        info.generation = open.targets.generation;
        for t in open.slots[self.back].all() {
            t.release();
        }
        self.back_acquired = false;
        // Make sure the GPU has the work before the runtime's device waits on the keyed mutex.
        self.gpu.flush();
        link.write_slot_info(self.back, &info);
        self.back = link.publish(self.back);
        link.signal_submit();
        true
    }

    fn release_back(&mut self) {
        if self.back_acquired {
            if let Some(open) = &self.open {
                for t in open.slots[self.back].all() {
                    t.release();
                }
            }
            self.back_acquired = false;
        }
    }

    // ---- the runtime's player rig ---------------------------------------------------------------
    /// Registers the game's objects (guns, magazines, props) for the runtime's hands. Bump
    /// `table.seq` on every change; the runtime re-reads the table when it changes.
    pub fn write_objects(&self, table: &ObjectTable) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_objects(table);
                true
            }
            _ => false,
        }
    }

    /// Registers the guns' recoil as the game authored it (see [`KickTable`]). Bump `table.seq`
    /// on every change, as for the objects; call again after a reconnect.
    pub fn write_kicks(&self, table: &KickTable) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_kicks(table);
                true
            }
            _ => false,
        }
    }

    /// Says which part of the HUD texture is what (subtitles, prompts, ammo...), so the runtime
    /// places each piece where VR games do (see [`HudLayout`]). Call again after a reconnect.
    pub fn write_hud_layout(&self, layout: &HudLayout) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_hud_layout(layout);
                true
            }
            _ => false,
        }
    }

    /// Registers the game's own hand poses (`ObjectDesc::grip_pose` / `support_pose` name them).
    /// Bump `table.seq` on every change, as for the objects; call again after a reconnect.
    pub fn write_hand_poses(&self, table: &HandPoseTable) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_hand_poses(table);
                true
            }
            _ => false,
        }
    }

    /// Streams static collision (`COL_*` messages) for the runtime's physics. False when the
    /// ring is full (try again next frame).
    pub fn push_collision(&self, kind: u32, payload: &[u8]) -> bool {
        match &self.link {
            Some(l) if self.connected => l.push_collision(kind, payload),
            _ => false,
        }
    }

    /// Where the runtime has the objects this frame (held ones in tracking space, `OS_TRACKING`).
    pub fn read_object_states(&self, out: &mut ObjectStates) -> bool {
        match &self.link {
            Some(l) if self.connected => l.read_object_states_into(out),
            _ => false,
        }
    }

    /// The player's body and hands (tracking space).
    pub fn read_body(&self) -> Option<RtBody> {
        match &self.link {
            Some(l) if self.connected => l.read_body().filter(|b| b.flags & BODY_VALID != 0),
            _ => None,
        }
    }

    pub fn push_event(&self, ev: Event) -> bool {
        match &self.link {
            Some(l) if self.connected => l.push_event(&ev),
            _ => false,
        }
    }

    pub fn haptic(&self, hand: u32, seconds: f32, freq: f32, amplitude: f32) -> bool {
        self.push_event(Event { kind: EV_HAPTIC, a: hand, f: [seconds, freq, amplitude, 0.0, 0.0, 0.0] })
    }

    pub fn rumble(&self, strength: f32) -> bool {
        self.push_event(Event { kind: EV_RUMBLE, a: 0, f: [strength, 0.0, 0.0, 0.0, 0.0, 0.0] })
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.release_back();
    }
}

fn open_targets(gpu: &Gpu, mapping: &str, t: &Targets) -> anyhow::Result<Vec<SlotTextures>> {
    let (_, unorm, _) = color_family(DXGI_FORMAT(t.color_format as i32));
    let mut slots = Vec::new();
    for s in 0..t.slots.min(SLOT_COUNT as u32) {
        let open = |kind: TexKind, fmt: DXGI_FORMAT| {
            Texture::open_shared(gpu, &texture_name(mapping, t.generation, kind, s), Some(fmt), Some(fmt))
        };
        let eyes = [open(TexKind::EyeLeft, unorm)?, open(TexKind::EyeRight, unorm)?];
        let depth = if t.flags & TGT_DEPTH != 0 {
            Some([open(TexKind::DepthLeft, DXGI_FORMAT_R32_FLOAT)?, open(TexKind::DepthRight, DXGI_FORMAT_R32_FLOAT)?])
        } else {
            None
        };
        let hud = if t.flags & TGT_HUD != 0 { Some(open(TexKind::Hud, unorm)?) } else { None };
        let aux = if t.flags & TGT_AUX != 0 { Some(open(TexKind::Aux, unorm)?) } else { None };
        slots.push(SlotTextures { eyes, depth, hud, aux });
    }
    Ok(slots)
}
