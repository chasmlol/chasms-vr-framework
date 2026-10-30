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
pub mod kicks;
pub mod basis;
pub mod math;
pub mod tracking;

pub use basis::Basis;
pub use tracking::{Anchor, TrackingMapper, View};

use rustvr_gpu::{Gpu, Texture, texture::color_family};
use rustvr_protocol::link::Link;
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32_FLOAT};

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
    /// The aux view's depth (`R32_FLOAT`, linear metres: [`rustvr_protocol::TGT_AUX_DEPTH`]).
    pub aux_depth: Option<Texture>,
    pub lights: Option<Texture>,
    /// The game's bloom and lens per eye (`R16G16B16A16_FLOAT`, [`rustvr_protocol::TGT_BLOOM`]).
    pub bloom: Option<[Texture; 2]>,
    pub lens: Option<[Texture; 2]>,
}

impl SlotTextures {
    fn all(&self) -> impl Iterator<Item = &Texture> {
        self.eyes
            .iter()
            .chain(self.depth.iter().flatten())
            .chain(self.hud.iter())
            .chain(self.aux.iter())
            .chain(self.aux_depth.iter())
            .chain(self.lights.iter())
            .chain(self.bloom.iter().flatten())
            .chain(self.lens.iter().flatten())
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
    /// How many combat events have been read ([`Adapter::combat_events`]).
    combat_read: u64,
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
            combat_read: 0,
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
    /// the frame counter, and (when the game hasn't said when its clock stood there) stamps it now:
    /// the runtime carries the player's walk and turn on from the stamp to every headset frame.
    pub fn write_state(&mut self, state: &mut GameState) {
        let Some(link) = &self.link else { return };
        set_name(&mut state.name, &self.cfg.name);
        state.request = self.cfg.request;
        state.tracking = self.tracking.state();
        if state.time_qpc == 0 {
            let mut now = 0i64;
            unsafe {
                let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut now);
            }
            state.time_qpc = now;
        }
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
    /// Publishes this frame's lighting (see [`GameLighting`]).
    pub fn write_game_lighting(&self, lighting: &GameLighting) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_game_lighting(lighting);
                true
            }
            _ => false,
        }
    }

    pub fn write_hud_layout(&self, layout: &HudLayout) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_hud_layout(layout);
                true
            }
            _ => false,
        }
    }

    /// The HUD layout written last (its sequence number; 0: none), for the picture it describes
    /// ([`SlotInfo::hud_layout`]).
    pub fn hud_layout_seq(&self) -> u32 {
        match &self.link {
            Some(l) if self.connected => l.hud_layout_seq(),
            _ => 0,
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

    /// Tells the runtime where the game's own sounds are (a sound pack: a folder with a
    /// `sounds.toml`, see `rustvr_audio::pack`) and which of its sets each object uses, and which
    /// sounds the game plays itself. The runtime then plays the pack's sounds instead of its own
    /// synthesised ones wherever the pack has them, and none where the game makes its own:
    ///
    /// ```ignore
    /// let dir = rustvr_adapter::local_sound_pack("metro2033");  // <local_assets>/metro2033/sounds
    /// let mut pack = SoundPack::new(&dir.map(|d| d.to_string_lossy().into_owned()).unwrap_or_default());
    /// pack.flags = SND_GAME_SHOTS;                               // the game plays its own gunshots
    /// pack.add_object(gun.id, "wpn_ak", 0);                      // the gun sounds like [sets.wpn_ak]
    /// adapter.set_sound_pack(&pack);
    /// ```
    ///
    /// Every call is a new write the runtime notices (`seq` is the seqlock's: leave it alone).
    /// Call it after connecting, again after every reconnect (when [`Adapter::connections`]
    /// changes), and whenever the objects' sets change; bump `pack.revision` to make the runtime
    /// reload the files from the same folder. False while not connected.
    pub fn set_sound_pack(&self, pack: &SoundPack) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_sound_pack(pack);
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

    /// Where the hands push the game's own simulated objects (see [`GamePushes`]). False when
    /// there's no runtime.
    pub fn read_game_pushes(&self, out: &mut GamePushes) -> bool {
        match &self.link {
            Some(l) if self.connected => l.read_game_pushes_into(out),
            _ => false,
        }
    }

    /// The game objects' convex collision (see [`GameHulls`]): before publishing them.
    pub fn write_game_hulls(&self, hulls: &GameHulls) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_game_hulls(hulls);
                true
            }
            _ => false,
        }
    }

    /// Where the game's own simulated objects are now (see [`GamePoses`]): every frame.
    pub fn write_game_poses(&self, poses: &GamePoses) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_game_poses(poses);
                true
            }
            _ => false,
        }
    }

    /// The creatures near the player (see [`Actors`]): every frame.
    pub fn write_actors(&self, actors: &Actors) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_actors(actors);
                true
            }
            _ => false,
        }
    }

    /// What grabbing the creatures needs (see [`ActorBodies`]): every frame, with them.
    pub fn write_actor_bodies(&self, bodies: &ActorBodies) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_actor_bodies(bodies);
                true
            }
            _ => false,
        }
    }

    /// Each hand's grab of a creature (see [`CreatureGrabs`]). False when there's no runtime.
    pub fn read_creature_grabs(&self, out: &mut CreatureGrabs) -> bool {
        match &self.link {
            Some(l) if self.connected => l.read_creature_grabs_into(out),
            _ => false,
        }
    }

    /// The blades of the melee weapons the game offers (see [`MeleeTable`]): when they change.
    pub fn write_melee(&self, table: &MeleeTable) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_melee(table);
                true
            }
            _ => false,
        }
    }

    /// What the things offered are, for the cards beside them (see [`ItemInfos`]): when they
    /// change.
    pub fn write_item_infos(&self, infos: &ItemInfos) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_item_infos(infos);
                true
            }
            _ => false,
        }
    }

    /// How the game's bows bend and their arrows fly (see [`Bows`]): with the objects, when they
    /// change.
    pub fn write_bows(&self, bows: &Bows) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_bows(bows);
                true
            }
            _ => false,
        }
    }

    /// The player's things as the game keeps them, and the game's own slots (see [`Inventory`]):
    /// when they change, and after every reconnect.
    pub fn write_inventory(&self, inv: &Inventory) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_inventory(inv);
                true
            }
            _ => false,
        }
    }

    /// The body's slots as the runtime has them (see [`BodySlots`]). False until it has written
    /// them.
    pub fn read_body_slots(&self, out: &mut BodySlots) -> bool {
        match &self.link {
            Some(l) if self.connected => l.read_body_slots_into(out),
            _ => false,
        }
    }

    /// The slots' editor's changes (see [`SlotEdits`]).
    pub fn write_slot_edits(&self, edits: &SlotEdits) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_slot_edits(edits);
                true
            }
            _ => false,
        }
    }

    /// The player's body as the game makes it up (see [`AvatarDesc`]): when it changes.
    pub fn write_avatar(&self, desc: &AvatarDesc) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_avatar(desc);
                true
            }
            _ => false,
        }
    }

    /// The player's face (see [`FaceDesc`]): when it changes.
    pub fn write_face(&self, face: &FaceDesc) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_face(face);
                true
            }
            _ => false,
        }
    }

    /// The mount the player rides and the mounts near them (see [`Ride`]): every game frame.
    pub fn write_ride(&self, ride: &Ride) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_ride(ride);
                true
            }
            _ => false,
        }
    }

    /// What the rider does with the mount (see [`RtRide`]): the gait, the steering, getting on
    /// and off. None while there's no runtime, or it has never ridden.
    pub fn read_rt_ride(&self) -> Option<RtRide> {
        match &self.link {
            Some(l) if self.connected => l.read_rt_ride(),
            _ => None,
        }
    }

    /// The player's spells and magicka (see [`Spells`]): when they change.
    pub fn write_spells(&self, spells: &Spells) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_spells(spells);
                true
            }
            _ => false,
        }
    }

    /// Each hand's casting now (see [`Casters`]), for the game to draw its casting art where it is.
    /// False when there's none.
    pub fn read_casters(&self, out: &mut Casters) -> bool {
        match &self.link {
            Some(l) if self.connected => l.read_casters_into(out),
            _ => false,
        }
    }

    /// The game's effect shaders on what the runtime draws (see [`Overlays`]): when they change.
    pub fn write_overlays(&self, overlays: &Overlays) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_overlays(overlays);
                true
            }
            _ => false,
        }
    }

    /// The game's kinds of blood (see [`BloodTable`]): when they change.
    pub fn write_bloods(&self, bloods: &BloodTable) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_bloods(bloods);
                true
            }
            _ => false,
        }
    }

    /// The creatures' blood and stumps (see [`GoreBodies`]): every frame.
    pub fn write_gore_bodies(&self, bodies: &GoreBodies) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.write_gore_bodies(bodies);
                true
            }
            _ => false,
        }
    }

    /// A limb cut off (see [`GoreEvent`]).
    pub fn push_gore_event(&self, ev: &GoreEvent) -> bool {
        match &self.link {
            Some(l) if self.connected => {
                l.push_gore_event(ev);
                true
            }
            _ => false,
        }
    }

    /// The melee hits, blocks and casts since the last call (see [`CombatEvent`]).
    pub fn combat_events(&mut self) -> Vec<CombatEvent> {
        match &self.link {
            Some(l) if self.connected => l.read_combat_events(&mut self.combat_read),
            _ => Vec::new(),
        }
    }

    /// The player as the runtime posed it (see [`RtPlayerDraw`]), for an adapter that draws it in
    /// the game's frame. False when there's none.
    pub fn read_player_draw(&self, out: &mut RtPlayerDraw) -> bool {
        match &self.link {
            Some(l) if self.connected => l.read_player_draw_into(out) && out.flags & PD_VALID != 0,
            _ => false,
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

    /// The game's mirror spell was cast ([`EV_MIRROR_CAST`]).
    pub fn mirror_cast(&self) -> bool {
        self.push_event(Event { kind: EV_MIRROR_CAST, a: 0, f: [0.0; 6] })
    }

    /// The game's effect for the mirror's summons `count` reached its moment ([`EV_MIRROR_HIT`]).
    pub fn mirror_hit(&self, count: u32) -> bool {
        self.push_event(Event { kind: EV_MIRROR_HIT, a: count, f: [0.0; 6] })
    }

    /// The game's explosion at `pos` (world metres) pushes the objects the runtime simulates:
    /// `impulse` (N s) at the centre, nothing past `radius` ([`EV_BLAST`]).
    pub fn blast(&self, pos: [f32; 3], radius: f32, impulse: f32) -> bool {
        self.push_event(Event { kind: EV_BLAST, a: 0, f: [pos[0], pos[1], pos[2], radius, impulse, 0.0] })
    }

    /// The game fired its held gun `shots` times since the last call ([`EV_GAME_SHOTS`]): legacy
    /// mode's recoil.
    pub fn game_shots(&self, shots: u32) -> bool {
        shots == 0 || self.push_event(Event { kind: EV_GAME_SHOTS, a: shots, f: [0.0; 6] })
    }

    /// Pushes one simulated object (`impulse` N s at world point `at`, [`EV_IMPULSE`]).
    pub fn impulse(&self, object: u32, impulse: [f32; 3], at: [f32; 3]) -> bool {
        self.push_event(Event { kind: EV_IMPULSE, a: object, f: [impulse[0], impulse[1], impulse[2], at[0], at[1], at[2]] })
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.release_back();
    }
}

/// The sound pack an extractor wrote for `game` (`<local_assets>/<game>/sounds`, e.g. what
/// `metro-extract` writes for `metro2033`), if it's there: for [`Adapter::set_sound_pack`].
/// `local_assets` is `RUSTVR_ASSETS`, or the one in the repository this was built from
/// ([`rustvr_asset::default_root`]).
pub fn local_sound_pack(game: &str) -> Option<std::path::PathBuf> {
    let dir = rustvr_asset::default_root().join(game).join("sounds");
    dir.join("sounds.toml").is_file().then_some(dir)
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
        let aux_depth = if t.flags & TGT_AUX_DEPTH != 0 { Some(open(TexKind::AuxDepth, DXGI_FORMAT_R32_FLOAT)?) } else { None };
        let lights = if t.flags & TGT_LIGHTS != 0 { Some(open(TexKind::Lights, DXGI_FORMAT_R8G8B8A8_UNORM)?) } else { None };
        let (bloom, lens) = if t.flags & TGT_BLOOM != 0 {
            let f = DXGI_FORMAT_R16G16B16A16_FLOAT;
            (Some([open(TexKind::BloomLeft, f)?, open(TexKind::BloomRight, f)?]), Some([open(TexKind::LensLeft, f)?, open(TexKind::LensRight, f)?]))
        } else {
            (None, None)
        };
        slots.push(SlotTextures { eyes, depth, hud, aux, aux_depth, lights, bloom, lens });
    }
    Ok(slots)
}
