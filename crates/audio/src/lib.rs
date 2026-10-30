//! Spatial interaction sounds for the runtime.
//!
//! A flat game has no grab, holster or magazine-seat sounds of its own, so the runtime makes
//! them: a small, consistent library played at the hand on the same frame as the haptic. See
//! "Haptics and sound must land as a single event" in `reports/VR player controller design.md`.
//!
//! * Every [`Sound`] is synthesised at startup, four variations each, no sample files.
//! * A game that has sounds of its own hands the runtime a **sound pack** (see [`pack`]): its
//!   files for each [`Event`], per sound set (one per gun, say). [`Audio::load_pack`] decodes it
//!   on a background thread; from then on [`Audio::play_event`] plays the pack's sound, and
//!   nothing where the pack has none: a game's pack replaces the runtime's sounds (the synthesised
//!   ones play only until a pack is loaded, and without one: the runtime's own range). The game
//!   is the source of truth; the library is the fallback for having no game.
//! * [`Audio::play`] and [`Audio::play_event`] only enqueue a command. The mixer (on the device's
//!   audio thread, or in [`Audio::render`] offline) picks a variation and starts the voice at its
//!   next block.
//! * Spatialisation is cheap: interaural time and level differences, head shadow, a rear
//!   low-pass, inverse distance per ear and a near-field bass lift (see `spatial`).
//! * At most [`MAX_VOICES`] play at once (the quietest is stolen), and the master bus goes
//!   through a look-ahead limiter, so it never clips.

pub mod decode;
mod device;
mod mixer;
pub mod pack;
mod sounds;
mod spatial;
mod synth;
#[cfg(test)]
mod pack_tests;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};

use glam::{Quat, Vec3};

use crate::mixer::Mixer;
pub use crate::pack::PackInfo;

/// Voices that can play at once.
pub const MAX_VOICES: usize = 32;

/// Commands that can wait for the mixer. A frame rarely sends more than a handful; this covers
/// a long stall of the audio thread.
const QUEUE: usize = 1024;

/// Every interaction sound the runtime makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sound {
    /// A hand closes on an object.
    Grab,
    /// A hand lets go: Grab mirrored, lower and quieter.
    Release,
    HolsterIn,
    HolsterOut,
    MagEject,
    MagSeat,
    SlideBack,
    SlideForward,
    /// The last round left the magazine: a bright ping that cuts through the shot.
    LastRound,
    /// The trigger on an empty chamber.
    EmptyClick,
    /// An action that didn't work (nothing to grab, wrong slot): a dull double clunk.
    Fail,
    /// The gravity glove's stages, in order.
    GloveHover,
    GloveLock,
    GloveFlick,
    /// The object in flight. Made for [`Audio::start_loop`].
    GloveTravel,
    GloveCatch,
    /// A collision; `intensity` is the impact speed mapped to 0..1 and adds crack and debris.
    Impact,
    UiHover,
    UiClick,
    /// A hand entering a holster or ammo zone.
    ZoneTick,
    ShotPistol,
    ShotRifle,
    ShotShotgun,
    /// A melee hit; `intensity` is the swing speed mapped to 0..1.
    Melee,
}

impl Sound {
    pub const ALL: [Sound; 24] = [
        Sound::Grab,
        Sound::Release,
        Sound::HolsterIn,
        Sound::HolsterOut,
        Sound::MagEject,
        Sound::MagSeat,
        Sound::SlideBack,
        Sound::SlideForward,
        Sound::LastRound,
        Sound::EmptyClick,
        Sound::Fail,
        Sound::GloveHover,
        Sound::GloveLock,
        Sound::GloveFlick,
        Sound::GloveTravel,
        Sound::GloveCatch,
        Sound::Impact,
        Sound::UiHover,
        Sound::UiClick,
        Sound::ZoneTick,
        Sound::ShotPistol,
        Sound::ShotRifle,
        Sound::ShotShotgun,
        Sound::Melee,
    ];
}

/// What happened, as a sound pack names it (`sounds.toml` keys). Each has a synthesised
/// [`Sound`] to fall back on, except the ones only a game's own sounds can make (a shot's tail,
/// casings landing, grenades).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Event {
    Grab,
    Release,
    /// Into a holster, and out of one (the draw).
    HolsterIn,
    HolsterOut,
    /// The magazine out of the gun, and a fresh one seated.
    MagOut,
    MagIn,
    /// The bolt, slide or charging handle pulled back, and going home.
    BoltBack,
    BoltForward,
    LastRound,
    /// The trigger on an empty chamber.
    DryFire,
    Fail,
    GloveHover,
    GloveLock,
    GloveFlick,
    GloveTravel,
    GloveCatch,
    Impact,
    UiHover,
    UiClick,
    /// A hand entering a holster or ammo zone.
    ZoneTick,
    /// A magazine taken from the ammo pouch.
    Pouch,
    /// The gunshot, at the muzzle (the runtime plays it only when the game doesn't).
    Fire,
    /// The shot's tail: its echo off the surroundings (a pack layer usually puts it in the bed).
    FireTail,
    /// The spent case landing (a pack usually delays it).
    Casing,
    Melee,
    GrenadePin,
    /// The grenade lit (the spoon flying off, a fuse catching).
    GrenadeArm,
    /// Its fuse ticking in the hand.
    GrenadeTick,
    GrenadeThrow,
    Explosion,
    KnifeThrow,
    /// A thrown knife sinking in.
    KnifeStick,
}

impl Event {
    pub const COUNT: usize = 32;
    pub const ALL: [Event; Event::COUNT] = [
        Event::Grab,
        Event::Release,
        Event::HolsterIn,
        Event::HolsterOut,
        Event::MagOut,
        Event::MagIn,
        Event::BoltBack,
        Event::BoltForward,
        Event::LastRound,
        Event::DryFire,
        Event::Fail,
        Event::GloveHover,
        Event::GloveLock,
        Event::GloveFlick,
        Event::GloveTravel,
        Event::GloveCatch,
        Event::Impact,
        Event::UiHover,
        Event::UiClick,
        Event::ZoneTick,
        Event::Pouch,
        Event::Fire,
        Event::FireTail,
        Event::Casing,
        Event::Melee,
        Event::GrenadePin,
        Event::GrenadeArm,
        Event::GrenadeTick,
        Event::GrenadeThrow,
        Event::Explosion,
        Event::KnifeThrow,
        Event::KnifeStick,
    ];

    /// Its key in `sounds.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Event::Grab => "grab",
            Event::Release => "release",
            Event::HolsterIn => "holster_in",
            Event::HolsterOut => "holster_out",
            Event::MagOut => "mag_out",
            Event::MagIn => "mag_in",
            Event::BoltBack => "bolt_back",
            Event::BoltForward => "bolt_forward",
            Event::LastRound => "last_round",
            Event::DryFire => "dry_fire",
            Event::Fail => "fail",
            Event::GloveHover => "glove_hover",
            Event::GloveLock => "glove_lock",
            Event::GloveFlick => "glove_flick",
            Event::GloveTravel => "glove_travel",
            Event::GloveCatch => "glove_catch",
            Event::Impact => "impact",
            Event::UiHover => "ui_hover",
            Event::UiClick => "ui_click",
            Event::ZoneTick => "zone_tick",
            Event::Pouch => "pouch",
            Event::Fire => "fire",
            Event::FireTail => "fire_tail",
            Event::Casing => "casing",
            Event::Melee => "melee",
            Event::GrenadePin => "grenade_pin",
            Event::GrenadeArm => "grenade_arm",
            Event::GrenadeTick => "grenade_tick",
            Event::GrenadeThrow => "grenade_throw",
            Event::Explosion => "explosion",
            Event::KnifeThrow => "knife_throw",
            Event::KnifeStick => "knife_stick",
        }
    }

    pub fn from_name(name: &str) -> Option<Event> {
        Event::ALL.into_iter().find(|e| e.name() == name)
    }

    /// The runtime's own sound for it (a rifle's shot for `fire`; callers that know better pass
    /// their own to [`Audio::play_event`]).
    pub fn fallback(self) -> Option<Sound> {
        Some(match self {
            Event::Grab => Sound::Grab,
            Event::Release => Sound::Release,
            Event::HolsterIn => Sound::HolsterIn,
            Event::HolsterOut => Sound::HolsterOut,
            Event::MagOut => Sound::MagEject,
            Event::MagIn => Sound::MagSeat,
            Event::BoltBack => Sound::SlideBack,
            Event::BoltForward => Sound::SlideForward,
            Event::LastRound => Sound::LastRound,
            Event::DryFire => Sound::EmptyClick,
            Event::Fail => Sound::Fail,
            Event::GloveHover => Sound::GloveHover,
            Event::GloveLock => Sound::GloveLock,
            Event::GloveFlick => Sound::GloveFlick,
            Event::GloveTravel => Sound::GloveTravel,
            Event::GloveCatch => Sound::GloveCatch,
            Event::Impact => Sound::Impact,
            Event::UiHover => Sound::UiHover,
            Event::UiClick => Sound::UiClick,
            Event::ZoneTick | Event::Pouch => Sound::ZoneTick,
            Event::Fire => Sound::ShotRifle,
            Event::Melee => Sound::Melee,
            // The throwables' fall back on the nearest synthesised sounds.
            Event::GrenadePin => Sound::SlideBack,
            Event::GrenadeArm => Sound::LastRound,
            Event::GrenadeTick => Sound::ZoneTick,
            Event::GrenadeThrow | Event::KnifeThrow => Sound::GloveFlick,
            Event::Explosion => Sound::ShotShotgun,
            Event::KnifeStick => Sound::Impact,
            Event::FireTail | Event::Casing => return None,
        })
    }

    /// The peak the runtime's own sound for this event is made at, at the reference distance
    /// (the hand): a pack builder can level a game's files to sit in the same mix (UI quietest,
    /// hands in the middle, shots loudest).
    pub fn level(self) -> f32 {
        match self {
            Event::UiHover => 0.12,
            Event::ZoneTick => 0.18,
            Event::GloveHover => 0.2,
            Event::Release => 0.22,
            Event::UiClick => 0.25,
            Event::GloveLock => 0.26,
            Event::GrenadeTick => 0.15,
            Event::Pouch | Event::LastRound | Event::GrenadePin | Event::GrenadeArm => 0.3,
            Event::KnifeThrow => 0.32,
            Event::KnifeStick => 0.45,
            Event::Grab | Event::GloveFlick => 0.32,
            Event::HolsterOut | Event::DryFire => 0.34,
            Event::HolsterIn | Event::GloveCatch => 0.36,
            Event::MagOut => 0.38,
            Event::BoltBack | Event::Fail | Event::Impact | Event::GrenadeThrow => 0.4,
            Event::Melee => 0.42,
            Event::MagIn | Event::BoltForward => 0.48,
            Event::GloveTravel => 0.2,
            Event::Fire | Event::Explosion => 0.6,
            // A shot's tail and its casings keep the game's level relative to the shot.
            Event::FireTail => 0.25,
            Event::Casing => 0.2,
        }
    }
}

/// A sound set's name, hashed (FNV-1a, case-insensitive), so a play command carries no string.
/// [`SetKey::NONE`] (and any set a pack doesn't have) plays the pack's `[defaults]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SetKey(u64);

impl SetKey {
    pub const NONE: SetKey = SetKey(0);

    pub fn of(name: &str) -> SetKey {
        if name.is_empty() {
            return SetKey::NONE;
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.bytes() {
            h ^= b.to_ascii_lowercase() as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        SetKey(h.max(1))
    }
}

/// Where a sound pack is at.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PackStatus {
    /// None asked for: the runtime's own sounds.
    #[default]
    None,
    /// Being decoded (or waiting for an output device to know the sample rate).
    Loading { dir: PathBuf },
    Ready(PackInfo),
    Failed { dir: PathBuf, error: String },
}

/// The head pose in tracking space (left-handed metres: X right, Y up, Z forward).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Listener {
    pub pos: Vec3,
    pub rot: Quat,
}

/// A running loop, from [`Audio::start_loop`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoopId(u64);

/// What the game thread asks of the mixer.
pub(crate) enum Command {
    Play { sound: Sound, pos: Vec3, intensity: f32 },
    /// A pack event, or `fallback` if the pack (for this set, or at all) has none.
    Event { event: Event, set: SetKey, fallback: Option<Sound>, pos: Vec3, intensity: f32 },
    StartLoop { id: u64, sound: Sound, event: Option<Event>, pos: Vec3 },
    MoveLoop { id: u64, pos: Vec3 },
    StopLoop { id: u64 },
}

/// State the game thread writes and the mixer reads as it starts each block.
pub(crate) struct Shared {
    /// The mixer only ever `try_lock`s this, so the audio thread never waits on the game.
    pub listener: Mutex<Listener>,
    /// Master volume, as `f32` bits.
    pub volume: AtomicU32,
    /// A mixer is taking commands: always offline; while a stream runs on a device.
    pub live: AtomicBool,
    /// Voices playing as of the last block.
    pub voices: AtomicUsize,
    /// The mixer's sample rate (0 until there is one): packs are decoded for it.
    pub rate: AtomicU32,
    /// The `Audio` has gone: loader threads give up.
    pub closed: AtomicBool,
    /// Every loaded pack, so the mixer's copy is never the last one: a pack is freed here, on the
    /// game thread, once nothing else holds it (never on the audio thread).
    pub packs: Mutex<Vec<Arc<pack::Pack>>>,
    pub pack_status: Mutex<PackStatus>,
    /// A pack (or `Some(None)`: none) for the mixer to switch to at its next block. It only
    /// `try_lock`s this, and takes the value out, so nothing is freed on the audio thread.
    pub pending: Mutex<Option<Option<Arc<pack::Pack>>>>,
    /// The latest load request; a loader whose request was superseded drops its result.
    pub pack_request: AtomicU64,
    /// Pack events started, per [`Event`], and synthesised sounds started.
    pub pack_plays: [AtomicU32; Event::COUNT],
    pub synth_plays: AtomicU64,
}

/// The runtime's interaction sounds: a sound library, a spatial mixer, and an output.
pub struct Audio {
    commands: SyncSender<Command>,
    shared: Arc<Shared>,
    next_loop: AtomicU64,
    output: Output,
}

enum Output {
    /// No device: [`Audio::render`] runs the mixer.
    Offline(Box<Mutex<Mixer>>),
    /// The device thread runs the mixer; this tells it to stop.
    Device(Sender<device::Event>),
}

impl Audio {
    /// Opens the default output device; if there is none (or it fails), runs silently (never
    /// panics, never blocks). The device is opened and the sounds generated on a background
    /// thread, which also reopens the output if the device goes away.
    pub fn new() -> Audio {
        Audio::with_output(Box::new(device::default_output))
    }

    pub(crate) fn with_output(open: device::Open) -> Audio {
        let (commands, receiver) = sync_channel(QUEUE);
        let shared = Arc::new(Audio::shared());
        let events = device::spawn(shared.clone(), receiver, open);
        Audio { commands, shared, next_loop: AtomicU64::new(1), output: Output::Device(events) }
    }

    /// No device at all (tests): mix only on demand with [`Audio::render`].
    pub fn offline(sample_rate: u32) -> Audio {
        assert!((8_000..=192_000).contains(&sample_rate), "unsupported sample rate {sample_rate}");
        let (commands, receiver) = sync_channel(QUEUE);
        let shared = Arc::new(Audio::shared());
        shared.live.store(true, Ordering::Release);
        let mixer = Mixer::new(sounds::bank(sample_rate), receiver, shared.clone());
        Audio {
            commands,
            shared,
            next_loop: AtomicU64::new(1),
            output: Output::Offline(Box::new(Mutex::new(mixer))),
        }
    }

    fn shared() -> Shared {
        Shared {
            listener: Mutex::new(Listener::default()),
            volume: AtomicU32::new(1.0f32.to_bits()),
            live: AtomicBool::new(false),
            voices: AtomicUsize::new(0),
            rate: AtomicU32::new(0),
            closed: AtomicBool::new(false),
            packs: Mutex::new(Vec::new()),
            pack_status: Mutex::new(PackStatus::None),
            pending: Mutex::new(None),
            pack_request: AtomicU64::new(0),
            pack_plays: std::array::from_fn(|_| AtomicU32::new(0)),
            synth_plays: AtomicU64::new(0),
        }
    }

    /// Sets the head pose that sounds are heard from. Call it every frame.
    pub fn set_listener(&self, l: Listener) {
        self.maintain();
        let rot = l.rot.normalize();
        if !(l.pos.is_finite() && rot.is_finite()) {
            tracing::debug!(?l, "ignored a broken listener pose");
            return;
        }
        *self.shared.listener.lock().unwrap_or_else(PoisonError::into_inner) = Listener { pos: l.pos, rot };
    }

    /// Master volume, 0..1.
    pub fn set_volume(&self, v: f32) {
        if !v.is_nan() {
            self.shared.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Plays a one-shot at a tracking-space position. `intensity` 0..1 scales loudness and
    /// timbre (e.g. impact speed): a soft sound is quieter and duller, and impacts and melee
    /// hits only get their crack and debris layer as it rises.
    pub fn play(&self, s: Sound, pos: Vec3, intensity: f32) {
        self.send(Command::Play { sound: s, pos, intensity: clean(intensity) });
    }

    /// Plays `event` from the loaded pack, as sound set `set` has it (its own, inherited, or the
    /// pack's defaults), at a tracking-space position; or `fallback`, synthesised, if the pack
    /// has none (or no pack is loaded). `intensity` works as for [`Audio::play`].
    pub fn play_event(&self, event: Event, set: SetKey, fallback: Option<Sound>, pos: Vec3, intensity: f32) {
        self.send(Command::Event { event, set, fallback, pos, intensity: clean(intensity) });
    }

    /// Loads the sound pack in `dir` (its `sounds.toml`) on a background thread, replacing the
    /// current one when it's ready; `None` goes back to the runtime's own sounds. Never blocks:
    /// until the pack is ready, events play their fallbacks. Loading the pack already loaded (or
    /// loading) does nothing.
    pub fn load_pack(&self, dir: Option<&Path>) {
        self.maintain();
        let current = self.pack_status();
        let same = match (&current, dir) {
            (PackStatus::Loading { dir: d }, Some(want)) => d == want,
            (PackStatus::Ready(info), Some(want)) => info.dir == want && info.rate == self.shared.rate.load(Ordering::Acquire),
            (PackStatus::None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let request = self.shared.pack_request.fetch_add(1, Ordering::AcqRel) + 1;
        let Some(dir) = dir else {
            *self.shared.pack_status.lock().unwrap_or_else(PoisonError::into_inner) = PackStatus::None;
            *self.shared.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(None);
            return;
        };
        let dir = dir.to_path_buf();
        *self.shared.pack_status.lock().unwrap_or_else(PoisonError::into_inner) = PackStatus::Loading { dir: dir.clone() };
        let shared = self.shared.clone();
        let started = std::thread::Builder::new().name("rustvr-sound-pack".into()).spawn(move || load_in_background(&shared, dir, request));
        if let Err(e) = started {
            tracing::error!("couldn't start the sound pack loader: {e}");
        }
    }

    /// The sound pack: none, loading, loaded (what it holds) or failed.
    pub fn pack_status(&self) -> PackStatus {
        self.shared.pack_status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Times the pack's sound for `event` has started (fallbacks not counted).
    pub fn pack_plays(&self, event: Event) -> u32 {
        self.shared.pack_plays[event as usize].load(Ordering::Relaxed)
    }

    /// Synthesised sounds started (plays, and events the pack didn't have).
    pub fn synth_plays(&self) -> u64 {
        self.shared.synth_plays.load(Ordering::Relaxed)
    }

    /// A loop that follows a position until stopped (glove travel). Any sound can loop;
    /// [`Sound::GloveTravel`] is the one made to. Synthesised: with a pack loaded it plays nothing
    /// (the pack's own loop is [`Audio::start_event_loop`]'s).
    pub fn start_loop(&self, s: Sound, pos: Vec3) -> LoopId {
        let id = self.next_loop.fetch_add(1, Ordering::Relaxed);
        self.send(Command::StartLoop { id, sound: s, event: None, pos });
        LoopId(id)
    }

    /// Starts a loop of the pack's sound for `event` (its first layer's clip, looped), or with no
    /// pack the synthesised `fallback`; a pack without the event plays nothing, as its one-shots.
    pub fn start_event_loop(&self, event: Event, fallback: Sound, pos: Vec3) -> LoopId {
        let id = self.next_loop.fetch_add(1, Ordering::Relaxed);
        self.send(Command::StartLoop { id, sound: fallback, event: Some(event), pos });
        LoopId(id)
    }

    pub fn move_loop(&self, id: LoopId, pos: Vec3) {
        self.send(Command::MoveLoop { id: id.0, pos });
    }

    /// Fades the loop out. Stopping a loop that has already gone is harmless.
    pub fn stop_loop(&self, id: LoopId) {
        self.send(Command::StopLoop { id: id.0 });
    }

    /// Voices playing as of the last mixed block (never more than [`MAX_VOICES`]), not counting
    /// ones fading out after being stolen or stopped.
    pub fn voices(&self) -> usize {
        self.shared.voices.load(Ordering::Relaxed)
    }

    /// Offline mixing for tests: the next `frames` stereo frames, interleaved. A device-backed
    /// `Audio` is mixed by its device, and this returns silence.
    pub fn render(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        if let Output::Offline(mixer) = &self.output {
            let mut mixer = mixer.lock().unwrap_or_else(PoisonError::into_inner);
            mixer.mix(frames, |i, l, r| {
                out[2 * i] = l;
                out[2 * i + 1] = r;
            });
        }
        self.maintain();
        out
    }

    /// Game-thread housekeeping: frees packs nothing uses any more, and reloads the pack if the
    /// output came back at another sample rate.
    fn maintain(&self) {
        self.shared.packs.lock().unwrap_or_else(PoisonError::into_inner).retain(|p| Arc::strong_count(p) > 1);
        let rate = self.shared.rate.load(Ordering::Acquire);
        let stale = match &*self.shared.pack_status.lock().unwrap_or_else(PoisonError::into_inner) {
            PackStatus::Ready(info) if rate != 0 && info.rate != rate => Some(info.dir.clone()),
            _ => None,
        };
        if let Some(dir) = stale {
            tracing::info!(rate, "audio output changed rate: reloading the sound pack");
            *self.shared.pack_status.lock().unwrap_or_else(PoisonError::into_inner) = PackStatus::None;
            self.load_pack(Some(&dir));
        }
    }

    fn send(&self, command: Command) {
        let pos = match command {
            Command::Play { pos, .. } | Command::Event { pos, .. } | Command::StartLoop { pos, .. } | Command::MoveLoop { pos, .. } => pos,
            Command::StopLoop { .. } => Vec3::ZERO,
        };
        if !pos.is_finite() {
            tracing::debug!(?pos, "ignored a sound at a broken position");
            return;
        }
        // While nothing is mixing (no device yet, or it was lost) a sound would only play late,
        // so drop it. Stops still go through, so a loop can't outlive a device reconnect.
        let stop = matches!(command, Command::StopLoop { .. });
        if !stop && !self.shared.live.load(Ordering::Acquire) {
            return;
        }
        if let Err(TrySendError::Full(_)) = self.commands.try_send(command) {
            tracing::debug!("audio command queue full; a sound was dropped");
        }
    }
}

fn clean(intensity: f32) -> f32 {
    if intensity.is_nan() { 1.0 } else { intensity.clamp(0.0, 1.0) }
}

/// The loader thread: waits for the mixer's sample rate, decodes the pack for it, and hands it
/// over (unless a newer request came in meanwhile).
fn load_in_background(shared: &Arc<Shared>, dir: PathBuf, request: u64) {
    let current = || shared.pack_request.load(Ordering::Acquire) == request && !shared.closed.load(Ordering::Acquire);
    let rate = loop {
        let rate = shared.rate.load(Ordering::Acquire);
        if rate != 0 {
            break rate;
        }
        if !current() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let result = pack::load(&dir, rate, request);
    if !current() {
        return;
    }
    match result {
        Ok((p, info)) => {
            tracing::info!(
                "sound pack {:?} from {}: {} sets, {} events, {} clips ({:.0} s of audio) in {} ms{}",
                info.name,
                dir.display(),
                info.sets,
                info.events,
                info.clips,
                info.seconds,
                info.millis,
                if info.missing.is_empty() { String::new() } else { format!(", {} files missing", info.missing.len()) }
            );
            if !info.unknown.is_empty() {
                tracing::warn!("sound pack {}: unknown events ignored: {}", dir.display(), info.unknown.join(", "));
            }
            let p = Arc::new(p);
            shared.packs.lock().unwrap_or_else(PoisonError::into_inner).push(p.clone());
            // Handed over before it's reported ready, so a play that follows "ready" finds it.
            *shared.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(Some(p));
            *shared.pack_status.lock().unwrap_or_else(PoisonError::into_inner) = PackStatus::Ready(info);
        }
        Err(e) => {
            tracing::warn!("sound pack {}: {e:#}; the runtime's own sounds play instead", dir.display());
            *shared.pack_status.lock().unwrap_or_else(PoisonError::into_inner) = PackStatus::Failed { dir, error: format!("{e:#}") };
        }
    }
}

/// The default output device, as [`Audio::new`].
impl Default for Audio {
    fn default() -> Audio {
        Audio::new()
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        // Not joined: a driver stuck in a call must not hang the runtime's shutdown.
        if let Output::Device(events) = &self.output {
            let _ = events.send(device::Event::Stop);
        }
    }
}
