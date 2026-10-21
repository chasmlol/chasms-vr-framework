//! The sound library: what each [`Sound`] is made of.
//!
//! Every sound is built from the same few ingredients (see `synth`): filtered noise bursts for
//! contact, decaying sines for the resonant modes of the object, noise sweeps for things sliding,
//! and short tones for the glove's own voice. Each variation gets its own pitch, timing, layer
//! levels and noise, and is normalised to a per-sound peak so the library sits at consistent
//! levels: UI quietest, hand interactions in the middle, impacts and shots loudest.

use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::Sound;
use crate::synth::{Buf, Env, Noise, Rng, Svf, normalise};

/// Variations per sound (the research asks for 3–5).
pub(crate) const VARIATIONS: usize = 4;

/// Frames per entry of [`Variation::level`].
pub(crate) const LEVEL_BLOCK: usize = 256;

/// One variation of a sound, ready to mix.
pub(crate) struct Variation {
    /// Spatialised at the source.
    pub main: Vec<f32>,
    /// Also spatialised, mixed in by the square of `intensity`: the crack and debris of a hard
    /// hit that a soft one doesn't have. Empty for most sounds.
    pub hard: Vec<f32>,
    /// Stereo, interleaved, not spatialised: a shot's sub-bass and room tail, which Alyx keeps
    /// out of the HRTF so they feel big rather than point-like. Empty for most sounds.
    pub bed: Vec<f32>,
    /// Made to repeat seamlessly (and so not silent at its start).
    pub looped: bool,
    /// Frames until every layer has ended.
    pub len: usize,
    /// Peak level per [`LEVEL_BLOCK`] frames, so the mixer can tell which voice is quietest.
    pub level: Vec<f32>,
}

impl Variation {
    fn new(main: Vec<f32>, hard: Vec<f32>, bed: Vec<f32>, looped: bool) -> Variation {
        let len = main.len().max(hard.len()).max(bed.len() / 2);
        let level = (0..len.div_ceil(LEVEL_BLOCK))
            .map(|block| {
                (block * LEVEL_BLOCK..((block + 1) * LEVEL_BLOCK).min(len))
                    .map(|i| {
                        let at = |v: &[f32], i: usize| v.get(i).map_or(0.0, |s| s.abs());
                        at(&main, i) + at(&hard, i) + at(&bed, 2 * i).max(at(&bed, 2 * i + 1))
                    })
                    .fold(0.0, f32::max)
            })
            .collect();
        Variation { main, hard, bed, looped, len, level }
    }

    fn single(main: Vec<f32>) -> Variation {
        Variation::new(main, Vec::new(), Vec::new(), false)
    }
}

/// Every variation of every sound at one sample rate.
pub(crate) struct Bank {
    pub rate: u32,
    /// Indexed by `Sound as usize`, then variation.
    pub sounds: Vec<Vec<Variation>>,
}

/// The bank for `rate`, shared with any other live mixer at that rate. Generation takes a while
/// in debug builds and the result is deterministic, so a device reopen (or a test suite making
/// many offline mixers) reuses it instead of rebuilding.
pub(crate) fn bank(rate: u32) -> Arc<Bank> {
    static BANKS: Mutex<Vec<Weak<Bank>>> = Mutex::new(Vec::new());
    let mut banks = BANKS.lock().unwrap_or_else(PoisonError::into_inner);
    banks.retain(|b| b.strong_count() > 0);
    if let Some(bank) = banks.iter().filter_map(Weak::upgrade).find(|b| b.rate == rate) {
        return bank;
    }
    let started = std::time::Instant::now();
    let bank = Arc::new(Bank {
        rate,
        sounds: Sound::ALL
            .iter()
            .map(|&sound| (0..VARIATIONS).map(|i| design(sound, i, rate as f32)).collect())
            .collect(),
    });
    tracing::debug!(rate, ms = started.elapsed().as_millis() as u64, "generated interaction sounds");
    banks.push(Arc::downgrade(&bank));
    bank
}

/// One variation: its pitch step and timing, and the random source for its layer levels, mode
/// detuning and noise.
struct Vary {
    rng: Rng,
    rate: f32,
    pitch: f32,
    time: f32,
}

impl Vary {
    fn hz(&self, freq: f32) -> f32 {
        freq * self.pitch
    }

    /// A resonant mode's frequency: the variation's pitch plus its own ±1%, so the ratios
    /// between modes (what the material sounds like) shift a little from one variation to the
    /// next. Kept under half the pitch step, so variations stay apart.
    fn partial(&mut self, freq: f32) -> f32 {
        self.hz(freq) * self.rng.range(0.99, 1.01)
    }

    fn s(&self, seconds: f32) -> f32 {
        seconds * self.time
    }

    /// A layer level within ±20%, so the balance inside the sound shifts a little each time.
    fn amp(&mut self, level: f32) -> f32 {
        level * self.rng.range(0.8, 1.2)
    }

    fn buf(&mut self) -> Buf {
        Buf::new(self.rate, self.rng.bits())
    }
}

fn design(sound: Sound, index: usize, rate: f32) -> Variation {
    let mut rng = Rng::new(0xA0D1_0000 ^ ((sound as u64) << 8 | index as u64));
    // Pitch steps evenly over ±6% (3% apart with four variations), so no two variations land on
    // the same pitch and blur into one; each mode then adds its own random ±1% (`partial`).
    // Timing is free within ±10%.
    let pitch = 0.94 + 0.12 * (index as f32 + 0.5) / VARIATIONS as f32;
    let time = rng.range(0.9, 1.1);
    let mut v = Vary { rng, rate, pitch, time };
    match sound {
        Sound::Grab => grab(&mut v),
        Sound::Release => release(&mut v),
        Sound::HolsterIn => holster_in(&mut v),
        Sound::HolsterOut => holster_out(&mut v),
        Sound::MagEject => mag_eject(&mut v),
        Sound::MagSeat => mag_seat(&mut v),
        Sound::SlideBack => slide_back(&mut v),
        Sound::SlideForward => slide_forward(&mut v),
        Sound::LastRound => last_round(&mut v),
        Sound::EmptyClick => empty_click(&mut v),
        Sound::Fail => fail(&mut v),
        Sound::GloveHover => glove_hover(&mut v),
        Sound::GloveLock => glove_lock(&mut v),
        Sound::GloveFlick => glove_flick(&mut v),
        Sound::GloveTravel => glove_travel(&mut v),
        Sound::GloveCatch => glove_catch(&mut v),
        Sound::Impact => impact(&mut v),
        Sound::UiHover => ui_hover(&mut v),
        Sound::UiClick => ui_click(&mut v),
        Sound::ZoneTick => zone_tick(&mut v),
        Sound::ShotPistol => shot(&mut v, &PISTOL),
        Sound::ShotRifle => shot(&mut v, &RIFLE),
        Sound::ShotShotgun => shot(&mut v, &SHOTGUN),
        Sound::Melee => melee(&mut v),
    }
}

/// A soft, short "tuck": the glove meeting the object (a mid noise burst) over the object's low
/// knock. Kept plain on purpose: it plays constantly, so it must stay familiar, not interesting.
fn grab(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.noise(0.0, v.amp(1.0), Noise::band(v.hz(1500.0), 0.9), Env::new(0.002, 0.0, v.s(0.018)));
    b.mode(0.0, v.partial(190.0), v.s(0.030), v.amp(0.6));
    b.mode(0.0, v.partial(470.0), v.s(0.015), v.amp(0.35));
    b.mode(0.0, v.partial(1150.0), v.s(0.008), v.amp(0.15));
    b.noise(0.0, v.amp(0.3), Noise::band(v.hz(5000.0), 2.0), Env::new(0.0003, 0.0, 0.002));
    Variation::single(b.finish(0.32))
}

/// Grab mirrored at lower pitch and level, with a softer front (Meta: release "mirrors Grab at
/// lower amplitude and frequency").
fn release(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.noise(0.0, v.amp(1.0), Noise::band(v.hz(1000.0), 0.8), Env::new(0.003, 0.0, v.s(0.022)));
    b.mode(0.0, v.partial(150.0), v.s(0.025), v.amp(0.45));
    b.mode(0.0, v.partial(360.0), v.s(0.012), v.amp(0.25));
    Variation::single(b.finish(0.22))
}

/// Leather rasp sliding down into the holster, then the seat: a low thud and the strap click.
fn holster_in(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    let rasp = Noise::band(v.hz(3200.0), 1.2).sweep(v.hz(900.0), v.s(0.12));
    b.noise(0.0, v.amp(0.6), rasp, Env::new(v.s(0.03), v.s(0.04), v.s(0.025)));
    let seat = v.s(0.1);
    b.mode(seat, v.partial(115.0), v.s(0.05), v.amp(0.8));
    b.mode(seat, v.partial(240.0), v.s(0.03), v.amp(0.4));
    b.noise(seat, v.amp(0.5), Noise::low(v.hz(700.0)), Env::new(0.001, 0.0, 0.02));
    b.noise(seat, v.amp(0.3), Noise::band(v.hz(2800.0), 1.5), Env::hit(0.004));
    Variation::single(b.finish(0.36))
}

/// The draw: the strap snapping free, then a rising leather "shhp".
fn holster_out(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.noise(0.0, v.amp(0.6), Noise::band(v.hz(2400.0), 1.5), Env::new(0.0005, 0.0, 0.005));
    b.mode(0.0, v.partial(900.0), 0.01, v.amp(0.3));
    let rasp = Noise::band(v.hz(900.0), 1.2).sweep(v.hz(3600.0), v.s(0.12));
    b.noise(0.01, v.amp(0.7), rasp, Env::new(v.s(0.02), v.s(0.05), v.s(0.03)));
    Variation::single(b.finish(0.34))
}

/// The catch letting go (a bright metal click with ringing modes), then the magazine rasping out
/// of the well with its hollow body resonance.
fn mag_eject(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.noise(0.0, v.amp(0.8), Noise::band(v.hz(4200.0), 1.0), Env::new(0.0002, 0.0, 0.003));
    b.mode(0.0, v.partial(2300.0), 0.02, v.amp(0.35));
    b.mode(0.0, v.partial(3700.0), 0.015, v.amp(0.25));
    b.mode(0.0, v.partial(5150.0), 0.01, v.amp(0.2));
    let rasp = Noise::band(v.hz(2600.0), 2.0).sweep(v.hz(1400.0), v.s(0.14));
    b.noise(0.012, v.amp(0.35), rasp, Env::new(0.01, v.s(0.06), v.s(0.04)));
    b.mode(0.012, v.partial(520.0), v.s(0.04), v.amp(0.25));
    Variation::single(b.finish(0.38))
}

/// The most satisfying sound in the set: a solid low-mid "clack" of the magazine bottoming out,
/// and 9 ms later the higher click of the catch springing over it.
fn mag_seat(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.noise(0.0, v.amp(0.9), Noise::band(v.hz(2200.0), 0.8), Env::new(0.0003, 0.0, 0.006));
    b.mode(0.0, v.partial(240.0), v.s(0.045), v.amp(0.9));
    b.mode(0.0, v.partial(610.0), v.s(0.028), v.amp(0.55));
    b.mode(0.0, v.partial(1450.0), v.s(0.016), v.amp(0.35));
    b.mode(0.0, v.partial(2900.0), 0.009, v.amp(0.2));
    let latch = v.s(0.009);
    b.noise(latch, v.amp(0.6), Noise::band(v.hz(4500.0), 1.5), Env::hit(0.0025));
    b.mode(latch, v.partial(3300.0), 0.012, v.amp(0.3));
    b.mode(latch, v.partial(4900.0), 0.008, v.amp(0.2));
    Variation::single(b.finish(0.48))
}

/// Metal sliding on metal (a narrow noise band sweeping up as the slide travels), ending in the
/// click at full travel.
fn slide_back(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    let rasp = Noise::band(v.hz(1800.0), 2.5).sweep(v.hz(4800.0), v.s(0.09));
    b.noise(0.0, v.amp(0.6), rasp, Env::new(v.s(0.035), v.s(0.03), v.s(0.012)));
    let stop = v.s(0.085);
    b.noise(stop, v.amp(0.8), Noise::band(v.hz(3500.0), 1.2), Env::hit(0.003));
    b.mode(stop, v.partial(2700.0), 0.012, v.amp(0.4));
    b.mode(stop, v.partial(4300.0), 0.01, v.amp(0.3));
    b.mode(stop, v.partial(1200.0), 0.02, v.amp(0.25));
    Variation::single(b.finish(0.40))
}

/// The spring snapping the slide home: a quick falling rasp, then a heavy "clack".
fn slide_forward(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    let rasp = Noise::band(v.hz(5000.0), 2.0).sweep(v.hz(2200.0), v.s(0.03));
    b.noise(0.0, v.amp(0.5), rasp, Env::new(0.004, v.s(0.01), 0.008));
    let home = v.s(0.028);
    b.noise(home, v.amp(1.0), Noise::band(v.hz(2600.0), 0.9), Env::new(0.0002, 0.0, 0.005));
    b.mode(home, v.partial(380.0), v.s(0.04), v.amp(0.8));
    b.mode(home, v.partial(1100.0), v.s(0.022), v.amp(0.5));
    b.mode(home, v.partial(2600.0), 0.012, v.amp(0.35));
    b.mode(home, v.partial(4100.0), 0.008, v.amp(0.2));
    Variation::single(b.finish(0.48))
}

/// A bright, ringing "ting" with inharmonic partials: high enough to cut through the shot it
/// plays with, and unlike any of the clicks.
fn last_round(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.mode(0.0, v.partial(3150.0), v.s(0.06), v.amp(0.8));
    b.mode(0.0, v.partial(5520.0), v.s(0.04), v.amp(0.45));
    b.mode(0.0, v.partial(8100.0), v.s(0.025), v.amp(0.25));
    b.mode(0.0, v.partial(1640.0), v.s(0.035), v.amp(0.2));
    b.noise(0.0, v.amp(0.4), Noise::band(6000.0, 1.0), Env::hit(0.002));
    Variation::single(b.finish(0.30))
}

/// A hammer falling on nothing: tiny, dry and high.
fn empty_click(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.noise(0.0, v.amp(1.0), Noise::band(v.hz(3000.0), 1.2), Env::new(0.0002, 0.0, 0.0025));
    b.mode(0.0, v.partial(1900.0), 0.008, v.amp(0.45));
    b.mode(0.0, v.partial(3300.0), 0.006, v.amp(0.3));
    b.mode(0.0, v.partial(750.0), 0.006, v.amp(0.15));
    Variation::single(b.finish(0.34))
}

/// The failed-action clunk: two dull, falling knocks, the second lower and quieter, like a
/// shrug. Nothing bright, so it never reads as success; but nothing below about 150 Hz carries
/// it either, because headset speakers barely reproduce that.
fn fail(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    for (at, level, pitch) in [(0.0, 1.0, 1.0), (v.s(0.075), 0.7, 0.88)] {
        let env = Env::new(0.001, 0.0, v.s(0.04));
        b.tone(at, v.amp(level), v.hz(160.0 * pitch), v.hz(115.0 * pitch), 0.06, env);
        b.mode(at, v.partial(330.0 * pitch), 0.03, v.amp(0.6 * level));
        b.noise(at, v.amp(0.5 * level), Noise::low(v.hz(1200.0 * pitch)), Env::new(0.001, 0.0, 0.02));
    }
    Variation::single(b.finish(0.40))
}

/// A short soft blip with a slight upward glide: short so it never gets in the way of the grab
/// that follows (Meta's guidance for hover).
fn glove_hover(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    let env = Env::new(0.003, 0.0, 0.022);
    b.tone(0.0, v.amp(1.0), v.hz(1250.0), v.hz(1400.0), 0.02, env);
    b.tone(0.0, v.amp(0.15), v.hz(2510.0), v.hz(2810.0), 0.02, env);
    Variation::single(b.finish(0.20))
}

/// Two rising tones (Meta's "success" shape) over a faint filtered lock-on hiss.
fn glove_lock(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    let first = Env::new(0.002, 0.0, 0.025);
    b.tone(0.0, v.amp(0.7), v.hz(880.0), v.hz(880.0), 0.0, first);
    b.tone(0.0, v.amp(0.1), v.hz(1760.0), v.hz(1760.0), 0.0, first);
    let at = v.s(0.055);
    let second = Env::new(0.002, 0.0, 0.04);
    b.tone(at, v.amp(1.0), v.hz(1320.0), v.hz(1320.0), 0.0, second);
    b.tone(at, v.amp(0.12), v.hz(2640.0), v.hz(2640.0), 0.0, second);
    b.noise(0.0, v.amp(0.12), Noise::band(1800.0, 2.0), Env::new(0.005, 0.0, 0.03));
    Variation::single(b.finish(0.26))
}

/// The flick ping (a fast upward chirp) over the air being pulled: a noise band sweeping up.
fn glove_flick(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.tone(0.0, v.amp(0.8), v.hz(1200.0), v.hz(2300.0), 0.035, Env::new(0.002, 0.0, 0.05));
    let air = Noise::band(v.hz(500.0), 1.5).sweep(v.hz(3000.0), v.s(0.16));
    b.noise(0.0, v.amp(0.5), air, Env::new(v.s(0.05), v.s(0.03), v.s(0.04)));
    Variation::single(b.finish(0.32))
}

/// The object in flight: band-passed noise whose centre and level wobble, over a soft hum.
/// Built to repeat seamlessly: the modulation and the hum complete whole cycles per loop, and
/// the noise's overrun is crossfaded back into its start.
fn glove_travel(v: &mut Vary) -> Variation {
    let rate = v.rate;
    let seconds = v.s(1.0);
    let n = (seconds * rate) as usize;
    let overlap = (0.08 * rate) as usize;
    let cycle = |i: usize| TAU * i as f32 / n as f32;
    let (wobble, swell) = (v.rng.range(0.0, TAU), v.rng.range(0.0, TAU));
    let centre = v.hz(1100.0);
    let mut noise = Rng::new(v.rng.bits());
    let mut filter = Svf::default();
    let mut whoosh: Vec<f32> = (0..n + overlap)
        .map(|i| {
            let freq = centre * (1.0 + 0.3 * (2.0 * cycle(i) + wobble).sin());
            let level = 1.0 + 0.25 * (3.0 * cycle(i) + swell).sin();
            level * filter.run(noise.range(-1.0, 1.0), freq, 1.8, rate).band
        })
        .collect();
    // Equal-power crossfade: the two stretches of noise are uncorrelated.
    for i in 0..overlap {
        let w = (i as f32 + 0.5) / overlap as f32 * PI / 2.0;
        whoosh[i] = whoosh[i] * w.sin() + whoosh[n + i] * w.cos();
    }
    whoosh.truncate(n);
    normalise(&mut whoosh, 1.0);
    // Whole cycles per loop, with a vibrato that also completes whole cycles.
    let low = (v.hz(220.0) * seconds).round();
    let fifth = (v.hz(330.0) * seconds).round();
    let mut main: Vec<f32> = whoosh
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let vibrato = 0.4 * (5.0 * cycle(i)).sin();
            let hum = 0.12 * (low * cycle(i) + vibrato).sin() + 0.05 * (fifth * cycle(i)).sin();
            0.8 * w + hum
        })
        .collect();
    normalise(&mut main, 0.20);
    Variation::new(main, Vec::new(), Vec::new(), true)
}

/// The catch: a ping as the hand closes (an octave-and-fifth pair) and a soft grab-like thud.
fn glove_catch(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.tone(0.0, v.amp(0.6), v.hz(2093.0), v.hz(2093.0), 0.0, Env::new(0.001, 0.0, v.s(0.07)));
    b.tone(0.0, v.amp(0.3), v.hz(3136.0), v.hz(3136.0), 0.0, Env::new(0.001, 0.0, v.s(0.05)));
    b.mode(0.0, v.partial(160.0), v.s(0.035), v.amp(0.8));
    b.mode(0.0, v.partial(400.0), v.s(0.015), v.amp(0.35));
    b.noise(0.0, v.amp(0.5), Noise::band(v.hz(1200.0), 1.0), Env::new(0.001, 0.0, 0.015));
    Variation::single(b.finish(0.36))
}

/// Layered by intensity: every hit has the low thud of the body; a hard one adds a broadband
/// crack, mid modes and a few small debris rattles after it.
fn impact(v: &mut Vary) -> Variation {
    let mut body = v.buf();
    body.mode(0.0, v.partial(100.0), v.s(0.06), v.amp(1.0));
    body.mode(0.0, v.partial(190.0), v.s(0.04), v.amp(0.7));
    body.mode(0.0, v.partial(370.0), v.s(0.025), v.amp(0.5));
    body.noise(0.0, v.amp(0.7), Noise::low(v.hz(900.0)), Env::new(0.001, 0.0, v.s(0.03)));

    let mut hard = v.buf();
    hard.noise(0.0, v.amp(1.0), Noise::band(v.hz(3000.0), 0.6), Env::new(0.0002, 0.0, 0.007));
    for (freq, decay, level) in [(700.0, 0.03, 0.5), (1250.0, 0.022, 0.4), (2300.0, 0.015, 0.3)] {
        hard.mode(0.0, v.partial(freq), v.s(decay), v.amp(level));
    }
    for _ in 0..3 + v.rng.below(4) {
        let at = v.rng.range(0.03, 0.22) * v.time;
        let noise = Noise::band(v.rng.range(2000.0, 6000.0), 1.5);
        let level = v.rng.range(0.1, 0.35) * (1.0 - at / 0.3);
        hard.noise(at, level, noise, Env::hit(v.rng.range(0.002, 0.004)));
    }
    Variation::new(body.finish(0.40), hard.finish(0.36), Vec::new(), false)
}

/// A light tick, the quietest sound in the set.
fn ui_hover(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.tone(0.0, 1.0, v.hz(2400.0), v.hz(2400.0), 0.0, Env::new(0.001, 0.0, 0.015));
    Variation::single(b.finish(0.12))
}

/// A crisp click: a short high noise tick on a falling blip.
fn ui_click(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.noise(0.0, v.amp(0.8), Noise::band(v.hz(4000.0), 1.5), Env::new(0.0002, 0.0, 0.003));
    b.tone(0.0, v.amp(1.0), v.hz(1600.0), v.hz(1500.0), 0.02, Env::new(0.0005, 0.0, 0.025));
    Variation::single(b.finish(0.25))
}

/// A small woody "tok" for a hand entering a holster or ammo zone: felt more than heard, and
/// lower than the UI sounds so the two never get confused.
fn zone_tick(v: &mut Vary) -> Variation {
    let mut b = v.buf();
    b.mode(0.0, v.partial(700.0), v.s(0.012), v.amp(1.0));
    b.mode(0.0, v.partial(1430.0), v.s(0.008), v.amp(0.5));
    b.noise(0.0, v.amp(0.4), Noise::band(v.hz(2500.0), 1.0), Env::hit(0.0015));
    Variation::single(b.finish(0.18))
}

/// A blunt hit: low thump and mid "thwack"; a hard one adds a gritty crunch.
fn melee(v: &mut Vary) -> Variation {
    let mut body = v.buf();
    body.mode(0.0, v.partial(75.0), v.s(0.06), v.amp(1.0));
    body.mode(0.0, v.partial(140.0), v.s(0.04), v.amp(0.6));
    body.noise(0.0, v.amp(0.8), Noise::band(v.hz(900.0), 0.7), Env::new(0.001, 0.0, v.s(0.03)));

    let mut hard = v.buf();
    hard.noise(0.0, v.amp(1.0), Noise::band(v.hz(2500.0), 0.8), Env::new(0.0005, 0.0, v.s(0.015)));
    for _ in 0..6 + v.rng.below(5) {
        let at = v.rng.range(0.0, 0.04);
        hard.noise(at, v.rng.range(0.1, 0.3), Noise::band(3500.0, 1.0), Env::hit(0.001));
    }
    hard.mode(0.0, v.partial(520.0), 0.025, v.amp(0.3));
    Variation::new(body.finish(0.42), hard.finish(0.30), Vec::new(), false)
}

/// The four layers of a gunshot, after Alyx's design.
struct Shot {
    /// Head: the crack, a very short broadband burst (centre Hz, decay s).
    crack: (f32, f32),
    /// Body: the blast, low-passed noise (cutoff Hz, decay s) over two low modes.
    blast: (f32, f32),
    modes: [(f32, f32, f32); 2],
    /// The action cycling (s after the shot); a pump gun has none.
    action: Option<f32>,
    /// LFE: a falling sub-bass sine (from Hz, to Hz, decay s).
    lfe: (f32, f32, f32),
    /// Tail: the room, decorrelated low-passed noise closing down (from Hz, to Hz, decay s).
    tail: (f32, f32, f32),
    /// Peaks of the spatial layers, the LFE and the tail.
    peaks: (f32, f32, f32),
}

const PISTOL: Shot = Shot {
    crack: (3500.0, 0.0025),
    blast: (2800.0, 0.035),
    modes: [(180.0, 0.03, 0.5), (420.0, 0.02, 0.3)],
    action: Some(0.03),
    lfe: (110.0, 45.0, 0.035),
    tail: (3500.0, 500.0, 0.10),
    peaks: (0.52, 0.36, 0.14),
};

const RIFLE: Shot = Shot {
    crack: (5000.0, 0.0018),
    blast: (2200.0, 0.06),
    modes: [(120.0, 0.05, 0.6), (300.0, 0.03, 0.35)],
    action: Some(0.045),
    lfe: (85.0, 35.0, 0.05),
    tail: (3000.0, 350.0, 0.15),
    peaks: (0.55, 0.42, 0.17),
};

const SHOTGUN: Shot = Shot {
    crack: (2500.0, 0.003),
    blast: (1600.0, 0.08),
    modes: [(95.0, 0.06, 0.7), (230.0, 0.04, 0.4)],
    action: None,
    lfe: (70.0, 30.0, 0.065),
    tail: (2500.0, 300.0, 0.18),
    peaks: (0.55, 0.45, 0.18),
};

/// Head and Body are spatialised at the gun; LFE and Tail go in the unspatialised bed. The tail
/// also carries a few early reflections of the crack, at different delays in each ear.
fn shot(v: &mut Vary, shot: &Shot) -> Variation {
    let mut head = v.buf();
    let (crack, decay) = shot.crack;
    head.noise(0.0, v.amp(1.0), Noise::band(v.hz(crack), 0.5), Env::new(0.0001, 0.0, decay));
    head.noise(0.0, v.amp(0.6), Noise::band(v.hz(crack * 1.8), 0.7), Env::new(0.0001, 0.0, decay * 0.6));
    let head = head.finish(1.0);

    let mut main = v.buf();
    main.add(0.0, 1.0, &head);
    let (cutoff, decay) = shot.blast;
    let blast = Noise::low(v.hz(cutoff)).sweep(v.hz(cutoff * 0.5), v.s(decay * 3.0));
    main.noise(0.0, v.amp(0.8), blast, Env::new(0.0005, 0.0, v.s(decay)));
    for (freq, decay, level) in shot.modes {
        main.mode(0.0, v.partial(freq), v.s(decay), v.amp(level));
    }
    if let Some(at) = shot.action {
        let at = v.s(at);
        main.noise(at, v.amp(0.25), Noise::band(v.hz(4000.0), 1.2), Env::hit(0.003));
        main.mode(at, v.partial(2600.0), 0.01, v.amp(0.15));
    }

    // The sub thump swells in over 4 ms, so its first peak lands after the crack, not on it.
    let mut lfe = v.buf();
    let (from, to, decay) = shot.lfe;
    lfe.tone(0.0, 1.0, v.hz(from), v.hz(to), v.s(decay * 2.0), Env::new(0.004, 0.0, v.s(decay)));
    let lfe = lfe.finish(shot.peaks.1);

    let (from, to, decay) = shot.tail;
    let reflections = [[0.011, 0.023, 0.037], [0.014, 0.019, 0.041]];
    let tails: Vec<Vec<f32>> = reflections
        .iter()
        .map(|delays| {
            let mut tail = v.buf();
            let room = Noise::low(from).sweep(to, v.s(decay * 4.0));
            tail.noise(0.004, 1.0, room, Env::new(0.01, 0.0, v.s(decay)));
            for (delay, gain) in delays.iter().zip([0.5, 0.35, 0.25]) {
                tail.add(v.s(*delay), gain, &head);
            }
            tail.finish(shot.peaks.2)
        })
        .collect();

    let frames = lfe.len().max(tails[0].len()).max(tails[1].len());
    let at = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0);
    let bed =
        (0..frames).flat_map(|i| [at(&lfe, i) + at(&tails[0], i), at(&lfe, i) + at(&tails[1], i)]).collect();
    Variation::new(main.finish(shot.peaks.0), Vec::new(), bed, false)
}
