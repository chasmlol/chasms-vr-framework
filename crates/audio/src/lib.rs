//! Spatial interaction sounds for the runtime.
//!
//! A flat game has no grab, holster or magazine-seat sounds of its own, so the runtime makes
//! them: a small, consistent library played at the hand on the same frame as the haptic. See
//! "Haptics and sound must land as a single event" in `reports/VR player controller design.md`.
//!
//! * Every [`Sound`] is synthesised at startup, four variations each, no sample files.
//! * [`Audio::play`] only enqueues a command. The mixer (on the device's audio thread, or in
//!   [`Audio::render`] offline) picks a variation and starts the voice at its next block.
//! * Spatialisation is cheap: interaural time and level differences, head shadow, a rear
//!   low-pass, inverse distance per ear and a near-field bass lift (see `spatial`).
//! * At most [`MAX_VOICES`] play at once (the quietest is stolen), and the master bus goes
//!   through a look-ahead limiter, so it never clips.

mod device;
mod mixer;
mod sounds;
mod spatial;
mod synth;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};

use glam::{Quat, Vec3};

use crate::mixer::Mixer;

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
    StartLoop { id: u64, sound: Sound, pos: Vec3 },
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
        }
    }

    /// Sets the head pose that sounds are heard from. Call it every frame.
    pub fn set_listener(&self, l: Listener) {
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
        let intensity = if intensity.is_nan() { 1.0 } else { intensity.clamp(0.0, 1.0) };
        self.send(Command::Play { sound: s, pos, intensity });
    }

    /// A loop that follows a position until stopped (glove travel). Any sound can loop;
    /// [`Sound::GloveTravel`] is the one made to.
    pub fn start_loop(&self, s: Sound, pos: Vec3) -> LoopId {
        let id = self.next_loop.fetch_add(1, Ordering::Relaxed);
        self.send(Command::StartLoop { id, sound: s, pos });
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
        out
    }

    fn send(&self, command: Command) {
        let pos = match command {
            Command::Play { pos, .. } | Command::StartLoop { pos, .. } | Command::MoveLoop { pos, .. } => pos,
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

/// The default output device, as [`Audio::new`].
impl Default for Audio {
    fn default() -> Audio {
        Audio::new()
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        // Not joined: a driver stuck in a call must not hang the runtime's shutdown.
        if let Output::Device(events) = &self.output {
            let _ = events.send(device::Event::Stop);
        }
    }
}
