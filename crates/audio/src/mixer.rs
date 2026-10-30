//! The mixer: voices, spatialisation and the master bus.
//!
//! It runs on the audio thread, so once built it never allocates, never blocks and never
//! panics: commands arrive through a bounded lock-free queue, voices live in a fixed pool, and
//! the scratch buffers are sized up front.
//!
//! A voice plays a variation of a synthesised [`Sound`] or a clip of a sound pack. A new pack
//! arrives through `Shared::pending` (only ever `try_lock`ed); the one it replaces is kept until
//! its last voice ends, and freed on the game thread (see `Shared::packs`), never here.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;

use glam::Vec3;

use crate::pack::Pack;
use crate::sounds::{Bank, LEVEL_BLOCK, VARIATIONS, Variation};
use crate::spatial::{self, Placement, Shelf};
use crate::synth::Rng;
use crate::{Command, Event, Listener, MAX_VOICES, SetKey, Shared, Sound};

/// Extra slots for voices fading out after being stolen or stopped, so a steal never cuts a
/// sound dead and never has to wait for the fade before starting the new one.
const FADING: usize = 8;
/// Frames per inner block. Spatial parameters are re-aimed once per block and smoothed per sample.
const BLOCK: usize = 256;
/// Delay line for the interaural time difference: longer than the largest one (0.66 ms) at 192 kHz.
const RING: usize = 128;
/// Time constant of the parameter smoothing: quick enough to follow a hand, slow enough not to zipper.
const SMOOTHING: f32 = 0.015;
/// Fade-in for loop sounds, which start mid-waveform.
const FADE_IN: f32 = 0.015;
/// Fade-out for a stopped loop, and over the last moments of any one-shot.
const RELEASE: f32 = 0.03;
/// Fade-out for a stolen voice: short, it is the quietest thing playing.
const STEAL: f32 = 0.003;
/// Corner of the low-pass that a soft (low intensity) sound is blended towards, Hz.
const DULL_CORNER: f32 = 1200.0;
/// Seed for the variation picker, so offline renders are repeatable.
const SEED: u64 = 0x5EED_A0D1;

/// Per-sample constants for one sample rate.
struct Tuning {
    smooth: f32,
    dull: f32,
    bass: f32,
    rear: f32,
    shelf: Shelf,
    fade_in: f32,
    release: f32,
    release_frames: usize,
    steal: f32,
}

impl Tuning {
    fn new(rate: f32) -> Tuning {
        let pole = |hz: f32| 1.0 - (-TAU * hz / rate).exp();
        Tuning {
            smooth: 1.0 - (-1.0 / (SMOOTHING * rate)).exp(),
            dull: pole(DULL_CORNER),
            bass: pole(spatial::BASS_CORNER),
            rear: pole(spatial::REAR_CORNER),
            shelf: Shelf::new(rate),
            fade_in: 1.0 / (FADE_IN * rate),
            release: 1.0 / (RELEASE * rate),
            release_frames: (RELEASE * rate) as usize,
            steal: 1.0 / (STEAL * rate),
        }
    }
}

/// A parameter gliding towards its target.
#[derive(Clone, Copy)]
struct Smoothed {
    now: f32,
    target: f32,
}

impl Smoothed {
    const ZERO: Smoothed = Smoothed { now: 0.0, target: 0.0 };

    fn step(&mut self, k: f32) -> f32 {
        self.now += (self.target - self.now) * k;
        self.now
    }

    fn snap(&mut self) {
        self.now = self.target;
    }
}

#[derive(Clone, Copy)]
struct Ear {
    delay: Smoothed,
    gain: Smoothed,
    shadow: Smoothed,
    /// Head-shadow filter state.
    x1: f32,
    y1: f32,
}

impl Ear {
    const IDLE: Ear =
        Ear { delay: Smoothed::ZERO, gain: Smoothed::ZERO, shadow: Smoothed::ZERO, x1: 0.0, y1: 0.0 };
}

#[derive(Clone, Copy)]
struct Voice {
    playing: bool,
    /// The pack the clip is from (its generation), or 0: the synthesised bank.
    pack: u64,
    /// The bank's sound, or the pack's clip.
    sound: usize,
    variation: usize,
    /// Frames of silence before it starts (a delayed pack layer).
    wait: usize,
    /// Next frame of the variation.
    at: usize,
    looping: Option<u64>,
    source: Vec3,
    /// Gain from intensity.
    loudness: f32,
    /// Weight of the hard layer.
    hard: f32,
    /// 1 plays the sound as made; lower blends towards a dull low-pass.
    bright: f32,
    /// Start order: the oldest goes first when levels tie.
    order: u64,
    fade: f32,
    /// Per-sample fade change; negative while releasing.
    fade_step: f32,
    ears: [Ear; 2],
    behind: Smoothed,
    bass: Smoothed,
    bed: Smoothed,
    /// One-pole filter states: intensity low-pass, near-field shelf, rear low-pass.
    dull: f32,
    low: f32,
    rear: f32,
    ring: [f32; RING],
    write: usize,
}

impl Voice {
    const IDLE: Voice = Voice {
        playing: false,
        pack: 0,
        sound: 0,
        variation: 0,
        wait: 0,
        at: 0,
        looping: None,
        source: Vec3::ZERO,
        loudness: 0.0,
        hard: 0.0,
        bright: 1.0,
        order: 0,
        fade: 0.0,
        fade_step: 0.0,
        ears: [Ear::IDLE; 2],
        behind: Smoothed::ZERO,
        bass: Smoothed::ZERO,
        bed: Smoothed::ZERO,
        dull: 0.0,
        low: 0.0,
        rear: 0.0,
        ring: [0.0; RING],
        write: 0,
    };

    fn releasing(&self) -> bool {
        self.fade_step < 0.0
    }

    fn release(&mut self, step: f32) {
        self.fade_step = self.fade_step.min(-step);
    }

    /// Roughly how loud this voice is right now, for deciding which one to steal.
    fn level(&self, v: &Variation) -> f32 {
        let gain = self.ears[0].gain.target.max(self.ears[1].gain.target).max(self.bed.target);
        v.level.get(self.at / LEVEL_BLOCK).copied().unwrap_or(0.0) * gain
    }

    fn aim(&mut self, p: &Placement) {
        for (i, ear) in self.ears.iter_mut().enumerate() {
            ear.delay.target = p.delay[i];
            ear.gain.target = p.gain[i] * self.loudness;
            ear.shadow.target = p.shadow[i];
        }
        self.behind.target = p.behind;
        self.bass.target = p.bass;
        self.bed.target = p.bed * self.loudness;
    }

    /// Jumps every parameter to its target, so a new voice starts in place rather than gliding there.
    fn snap(&mut self) {
        for ear in &mut self.ears {
            ear.delay.snap();
            ear.gain.snap();
            ear.shadow.snap();
        }
        self.behind.snap();
        self.bass.snap();
        self.bed.snap();
    }

    /// Adds this voice into `left` and `right`. Returns false once it has finished.
    fn render(&mut self, v: &Variation, t: &Tuning, left: &mut [f32], right: &mut [f32]) -> bool {
        let k = t.smooth;
        let sample = |layer: &[f32], i: usize| layer.get(i).copied().unwrap_or(0.0);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.wait > 0 {
                self.wait -= 1;
                continue;
            }
            // A one-shot fades over its last moments, so it never ends on a cut even when it
            // is a loop sound played once.
            if self.looping.is_none() && self.at + t.release_frames >= v.len {
                self.release(t.release);
            }

            let x = sample(&v.main, self.at) + self.hard * sample(&v.hard, self.at);
            self.dull += t.dull * (x - self.dull);
            let x = self.dull + (x - self.dull) * self.bright;
            self.low += t.bass * (x - self.low);
            let x = x + self.bass.step(k) * self.low;
            self.rear += t.rear * (x - self.rear);
            let x = x + (self.rear - x) * self.behind.step(k);

            self.ring[self.write] = x;
            let mut out = [0.0; 2];
            for (o, ear) in out.iter_mut().zip(&mut self.ears) {
                let y = delayed(&self.ring, self.write, ear.delay.step(k));
                let (b0, b1) = t.shelf.b(ear.shadow.step(k));
                let z = b0 * y + b1 * ear.x1 - t.shelf.a1 * ear.y1;
                ear.x1 = y;
                ear.y1 = z;
                *o = z * ear.gain.step(k) * self.fade;
            }
            let bed = self.bed.step(k) * self.fade;
            *l += out[0] + bed * sample(&v.bed, 2 * self.at);
            *r += out[1] + bed * sample(&v.bed, 2 * self.at + 1);

            self.write = (self.write + 1) % RING;
            self.at += 1;
            if self.looping.is_some() && self.at >= v.main.len() {
                self.at = 0;
            }
            self.fade = (self.fade + self.fade_step).min(1.0);
            if self.fade <= 0.0 && self.releasing() {
                return false;
            }
        }
        true
    }
}

/// Reads the delay line `delay` samples behind the newest sample, interpolating linearly.
fn delayed(ring: &[f32; RING], newest: usize, delay: f32) -> f32 {
    let delay = delay.clamp(0.0, (RING - 2) as f32);
    let whole = delay as usize;
    let a = ring[(newest + RING - whole) % RING];
    let b = ring[(newest + RING - whole - 1) % RING];
    a + (b - a) * (delay - whole as f32)
}

/// Samples of look-ahead in the limiter (1.3 ms at 48 kHz), which is also its latency.
const WINDOW: usize = 64;
/// The master bus never goes above this.
const CEILING: f32 = 0.9;
/// Time constant of the limiter's recovery.
const LIMITER_RELEASE: f32 = 0.08;

/// A look-ahead peak limiter. The gain each sample needs is known `WINDOW` samples before the
/// sample goes out, so the gain can ramp down over the window (a box filter) and still meet
/// every peak: smooth like a soft limiter, but nothing ever gets past the ceiling.
struct Limiter {
    /// Gain each of the last `WINDOW` input samples needs.
    needs: [f32; WINDOW],
    /// The released gain over the same samples; their mean is applied.
    held: [f32; WINDOW],
    /// The input, delayed by `WINDOW - 1` samples.
    delayed: [[f32; 2]; WINDOW - 1],
    at: usize,
    delayed_at: usize,
    gain: f32,
    release: f32,
}

impl Limiter {
    fn new(rate: f32) -> Limiter {
        Limiter {
            needs: [1.0; WINDOW],
            held: [1.0; WINDOW],
            delayed: [[0.0; 2]; WINDOW - 1],
            at: 0,
            delayed_at: 0,
            gain: 1.0,
            release: 1.0 - (-1.0 / (LIMITER_RELEASE * rate)).exp(),
        }
    }

    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let peak = l.abs().max(r.abs());
        self.needs[self.at] = if peak > CEILING { CEILING / peak } else { 1.0 };
        let need = self.needs.iter().fold(1.0f32, |m, &n| m.min(n));
        // Down at once (the window already gives the ramp time), back up slowly.
        self.gain = if need < self.gain { need } else { self.gain + (need - self.gain) * self.release };
        self.held[self.at] = self.gain;
        let gain = self.held.iter().sum::<f32>() / WINDOW as f32;
        self.at = (self.at + 1) % WINDOW;

        let [dl, dr] = self.delayed[self.delayed_at];
        self.delayed[self.delayed_at] = [l, r];
        self.delayed_at = (self.delayed_at + 1) % (WINDOW - 1);
        (dl * gain, dr * gain)
    }
}

/// The variation a voice plays: from the bank, the current pack or the one before it.
fn clip<'a>(bank: &'a Bank, packs: [Option<&'a Pack>; 2], v: &Voice) -> Option<&'a Variation> {
    if v.pack == 0 {
        return bank.sounds.get(v.sound)?.get(v.variation);
    }
    packs.into_iter().flatten().find(|p| p.generation == v.pack)?.clips.get(v.sound)
}

/// What a new voice plays.
#[derive(Clone, Copy)]
enum Source {
    Synth(Sound),
    /// A pack's clip: the pack's generation, the clip, its gain and the frames it waits.
    Clip { pack: u64, clip: usize, gain: f32, wait: usize },
}

pub(crate) struct Mixer {
    bank: Arc<Bank>,
    /// The sound pack, and the one it replaced while voices still play from that.
    pack: Option<Arc<Pack>>,
    old: Option<Arc<Pack>>,
    tuning: Tuning,
    commands: Receiver<Command>,
    shared: Arc<Shared>,
    voices: Box<[Voice]>,
    listener: Listener,
    /// Smoothed master volume; `None` until the first block, so the stream starts at the
    /// requested volume instead of gliding there from a default.
    volume: Option<f32>,
    limiter: Limiter,
    left: Box<[f32]>,
    right: Box<[f32]>,
    rng: Rng,
    /// Last variation played of each sound.
    last: [usize; Sound::ALL.len()],
    started: u64,
}

impl Mixer {
    pub fn new(bank: Arc<Bank>, commands: Receiver<Command>, shared: Arc<Shared>) -> Mixer {
        let rate = bank.rate as f32;
        shared.rate.store(bank.rate, Ordering::Release);
        Mixer {
            bank,
            pack: None,
            old: None,
            tuning: Tuning::new(rate),
            commands,
            shared,
            voices: vec![Voice::IDLE; MAX_VOICES + FADING].into_boxed_slice(),
            listener: Listener::default(),
            volume: None,
            limiter: Limiter::new(rate),
            left: vec![0.0; BLOCK].into_boxed_slice(),
            right: vec![0.0; BLOCK].into_boxed_slice(),
            rng: Rng::new(SEED),
            last: [usize::MAX; Sound::ALL.len()],
            started: 0,
        }
    }

    pub fn rate(&self) -> u32 {
        self.bank.rate
    }

    /// Switches to a bank at another sample rate (a new device). Whatever was playing stops, and
    /// the sound pack goes until it has been decoded again for the new rate (`Audio` sees to
    /// that). Runs on the device thread while no stream does, so letting go of packs is fine.
    pub fn retune(&mut self, bank: Arc<Bank>) {
        let rate = bank.rate as f32;
        self.shared.rate.store(bank.rate, Ordering::Release);
        self.bank = bank;
        self.tuning = Tuning::new(rate);
        self.limiter = Limiter::new(rate);
        self.voices.fill(Voice::IDLE);
        self.pack = None;
        self.old = None;
    }

    /// Takes the pack the game thread left for it, if any. The one it replaces stays (as `old`)
    /// while its voices finish; one older than that has its voices stopped. Every pack is also
    /// held in `Shared::packs`, so letting go of one here never frees it.
    fn switch_pack(&mut self) {
        let Ok(mut pending) = self.shared.pending.try_lock() else { return };
        let Some(next) = pending.take() else { return };
        drop(pending);
        // A pack decoded for another rate (the device changed meanwhile) waits for its reload.
        let next = next.filter(|p| p.rate == self.bank.rate);
        if let Some(old) = self.old.take() {
            for v in self.voices.iter_mut().filter(|v| v.pack == old.generation) {
                v.playing = false;
            }
        }
        self.old = std::mem::replace(&mut self.pack, next);
    }

    /// Voices playing, not counting ones fading out after a steal or a stop.
    fn active(&self) -> usize {
        self.voices.iter().filter(|v| v.playing && !v.releasing()).count()
    }

    /// Mixes the next `frames` stereo frames, handing each to `write(index, left, right)`.
    pub fn mix(&mut self, frames: usize, mut write: impl FnMut(usize, f32, f32)) {
        // The newest head pose; if the game thread holds the lock this instant, the last one
        // is a block old at most.
        if let Ok(listener) = self.shared.listener.try_lock() {
            self.listener = *listener;
        }
        self.switch_pack();
        while let Ok(command) = self.commands.try_recv() {
            self.apply(command);
        }
        // Read after the commands, so a volume set before a play applies to it.
        let target = f32::from_bits(self.shared.volume.load(Ordering::Relaxed));
        let mut volume = self.volume.unwrap_or(target);
        let rate = self.bank.rate as f32;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            for voice in self.voices.iter_mut().filter(|v| v.playing) {
                voice.aim(&spatial::place(&self.listener, voice.source, rate));
            }
            let (left, right) = (&mut self.left[..n], &mut self.right[..n]);
            left.fill(0.0);
            right.fill(0.0);
            let packs = [self.pack.as_deref(), self.old.as_deref()];
            for voice in self.voices.iter_mut().filter(|v| v.playing) {
                voice.playing = match clip(&self.bank, packs, voice) {
                    Some(variation) => voice.render(variation, &self.tuning, left, right),
                    None => false,
                };
            }
            for i in 0..n {
                volume += (target - volume) * self.tuning.smooth;
                let (l, r) = self.limiter.process(finite(left[i] * volume), finite(right[i] * volume));
                write(done + i, l, r);
            }
            done += n;
        }
        self.volume = Some(volume);
        self.shared.voices.store(self.active(), Ordering::Relaxed);
        // The pack before this one goes once its last voice has.
        if let Some(old) = &self.old
            && !self.voices.iter().any(|v| v.playing && v.pack == old.generation)
        {
            self.old = None;
        }
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::Play { sound, pos, intensity } => {
                self.shared.synth_plays.fetch_add(1, Ordering::Relaxed);
                self.start(Source::Synth(sound), pos, intensity, None);
            }
            Command::Event { event, set, fallback, pos, intensity } => self.start_event(event, set, fallback, pos, intensity),
            // (A game's pack replaces the runtime's sounds: no synthesised loop with one loaded.)
            Command::StartLoop { id, sound, event, pos } => {
                // A pack's own loop for the event, else (no pack) the synthesised one.
                let pack = self.pack.clone();
                match (pack.as_deref(), event) {
                    (Some(p), Some(event)) => {
                        if let Some(layer) = p.find(SetKey::NONE, event).and_then(|e| e.layers.first()) {
                            let clip = layer.clips[layer.pick(&mut self.rng)];
                            self.start(Source::Clip { pack: p.generation, clip, gain: layer.gain * p.gain, wait: 0 }, pos, 1.0, Some(id));
                        }
                    }
                    (Some(_), None) => {}
                    (None, _) => self.start(Source::Synth(sound), pos, 1.0, Some(id)),
                }
            }
            Command::MoveLoop { id, pos } => {
                if let Some(voice) = self.voices.iter_mut().find(|v| v.playing && v.looping == Some(id)) {
                    voice.source = pos;
                }
            }
            Command::StopLoop { id } => {
                if let Some(voice) = self.voices.iter_mut().find(|v| v.playing && v.looping == Some(id)) {
                    voice.release(self.tuning.release);
                }
            }
        }
    }

    /// The pack's sound for `event`, every layer at once (each picking one of its clips). With no
    /// pack loaded, the synthesised fallback; with one, nothing where it has none: a game's pack
    /// replaces the runtime's sounds, it doesn't patch them.
    fn start_event(&mut self, event: Event, set: SetKey, fallback: Option<Sound>, pos: Vec3, intensity: f32) {
        // A clone of the `Arc`, so the voices can be started while it's borrowed; dropping it
        // again never frees the pack (`Shared::packs` holds every one).
        let pack = self.pack.clone();
        match pack.as_deref().and_then(|p| Some((p, p.find(set, event)?))) {
            Some((p, e)) => {
                self.shared.pack_plays[event as usize].fetch_add(1, Ordering::Relaxed);
                for layer in &e.layers {
                    let clip = layer.clips[layer.pick(&mut self.rng)];
                    let source = Source::Clip { pack: p.generation, clip, gain: layer.gain * p.gain, wait: layer.delay };
                    self.start(source, pos, intensity, None);
                }
            }
            None if pack.is_some() => {}
            None => {
                if let Some(sound) = fallback {
                    self.shared.synth_plays.fetch_add(1, Ordering::Relaxed);
                    self.start(Source::Synth(sound), pos, intensity, None);
                }
            }
        }
    }

    fn start(&mut self, what: Source, source: Vec3, intensity: f32, looping: Option<u64>) {
        let (pack, s, variation, gain, wait) = match what {
            Source::Synth(sound) => (0, sound as usize, self.pick(sound as usize), 1.0, 0),
            Source::Clip { pack, clip, gain, wait } => (pack, clip, 0, gain, wait),
        };
        let bank = &self.bank;
        let packs = [self.pack.as_deref(), self.old.as_deref()];
        let Some(v) = clip(bank, packs, &Voice { pack, sound: s, variation, ..Voice::IDLE }) else { return };
        let mut voice = Voice {
            playing: true,
            pack,
            sound: s,
            variation,
            wait,
            looping,
            source,
            // A soft sound is quieter (down to −16 dB), duller, and without its hard layer.
            loudness: (0.15 + 0.85 * intensity) * gain,
            hard: intensity * intensity,
            bright: intensity,
            order: self.started,
            fade: if v.looped { 0.0 } else { 1.0 },
            fade_step: if v.looped { self.tuning.fade_in } else { 0.0 },
            ..Voice::IDLE
        };
        voice.aim(&spatial::place(&self.listener, source, bank.rate as f32));
        voice.snap();
        self.started += 1;

        if self.active() >= MAX_VOICES {
            // Steal the quietest playing voice, one-shots before loops, the oldest on a tie.
            let level = |v: &Voice| clip(bank, packs, v).map_or(0.0, |c| v.level(c));
            let playing = self.voices.iter_mut().filter(|v| v.playing && !v.releasing());
            let victim = playing.map(|v| (level(v), v)).min_by(|(la, a), (lb, b)| {
                (a.looping.is_some(), *la, a.order)
                    .partial_cmp(&(b.looping.is_some(), *lb, b.order))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let Some((victim_level, victim)) = victim else { return };
            // A new one-shot quieter than everything playing would be inaudible: drop it
            // instead. A loop always gets a voice, since nothing would restart it later.
            if looping.is_none() && voice.level(v) < victim_level {
                return;
            }
            victim.release(self.tuning.steal);
        }
        // A free slot, or else the fading voice nearest to silence.
        let slot = self.voices.iter().position(|v| !v.playing).unwrap_or_else(|| {
            let fading = self.voices.iter().enumerate().filter(|(_, v)| v.releasing());
            fading.min_by(|a, b| a.1.fade.total_cmp(&b.1.fade)).map_or(0, |(i, _)| i)
        });
        self.voices[slot] = voice;
    }

    /// A random variation, never the one this sound played last: repeats are what make a
    /// library sound canned.
    fn pick(&mut self, sound: usize) -> usize {
        let n = self.rng.below(VARIATIONS - 1);
        let pick = if n >= self.last[sound] { n + 1 } else { n };
        self.last[sound] = pick;
        pick
    }
}


/// A NaN or infinity reaching the master bus would poison the limiter; make it silence.
fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}
