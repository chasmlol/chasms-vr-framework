//! Building blocks for the procedural sounds: noise, a filter, envelopes and resonant modes.

use std::f32::consts::{PI, TAU};

/// SplitMix64: tiny, fast and plenty for noise and variation. Seeded, so every run (and every
/// test) builds exactly the same library.
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn bits(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in 0..1.
    pub fn unit(&mut self) -> f32 {
        (self.bits() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.bits() % n as u64) as usize
    }
}

/// Linear attack, a hold, then an exponential decay (`decay` is the time constant in seconds).
#[derive(Clone, Copy)]
pub(crate) struct Env {
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
}

impl Env {
    pub const fn new(attack: f32, hold: f32, decay: f32) -> Env {
        Env { attack, hold, decay }
    }

    /// A strike: full level at once, then decay.
    pub const fn hit(decay: f32) -> Env {
        Env::new(0.0, 0.0, decay)
    }

    /// Long enough to fall 60 dB.
    fn length(&self) -> f32 {
        self.attack + self.hold + 6.9 * self.decay
    }

    fn at(&self, t: f32) -> f32 {
        if t < self.attack {
            t / self.attack
        } else if t < self.attack + self.hold {
            1.0
        } else {
            (-(t - self.attack - self.hold) / self.decay).exp()
        }
    }
}

/// The outputs of one filter step.
pub(crate) struct Taps {
    pub low: f32,
    /// Unity gain at the centre frequency.
    pub band: f32,
}

/// Zavalishin's topology-preserving state-variable filter: stable while its cutoff moves every
/// sample, which the sweeps need.
#[derive(Default)]
pub(crate) struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    pub fn run(&mut self, x: f32, freq: f32, q: f32, rate: f32) -> Taps {
        let g = (PI * freq.min(0.45 * rate) / rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        Taps { low: v2, band: k * v1 }
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Low,
    Band,
}

/// How a noise burst is filtered: a low- or band-pass whose cutoff can glide.
#[derive(Clone, Copy)]
pub(crate) struct Noise {
    shape: Shape,
    from: f32,
    to: f32,
    sweep: f32,
    q: f32,
}

impl Noise {
    pub fn band(freq: f32, q: f32) -> Noise {
        Noise { shape: Shape::Band, from: freq, to: freq, sweep: 0.0, q }
    }

    pub fn low(freq: f32) -> Noise {
        Noise { shape: Shape::Low, from: freq, to: freq, sweep: 0.0, q: 0.707 }
    }

    /// Glides the cutoff to `to` over `seconds`. Exponential, as a pitch moves, because that is
    /// what a metal slide or a whoosh sounds like.
    pub fn sweep(self, to: f32, seconds: f32) -> Noise {
        Noise { to, sweep: seconds, ..self }
    }

    fn freq_at(&self, t: f32) -> f32 {
        glide(self.from, self.to, self.sweep, t)
    }
}

fn glide(from: f32, to: f32, seconds: f32, t: f32) -> f32 {
    if seconds > 0.0 { from * (to / from).powf((t / seconds).min(1.0)) } else { from }
}

/// A mono sound under construction. Layers are added at a time offset; the buffer grows to fit.
pub(crate) struct Buf {
    rate: f32,
    rng: Rng,
    samples: Vec<f32>,
}

impl Buf {
    pub fn new(rate: f32, seed: u64) -> Buf {
        Buf { rate, rng: Rng::new(seed), samples: Vec::new() }
    }

    /// The index range for `length` seconds starting at `at`, growing the buffer to hold it.
    fn span(&mut self, at: f32, length: f32) -> (usize, usize) {
        let start = (at * self.rate).round() as usize;
        let count = (length * self.rate).ceil() as usize;
        if self.samples.len() < start + count {
            self.samples.resize(start + count, 0.0);
        }
        (start, count)
    }

    /// A sine whose pitch glides from `from` to `to` over `glide` seconds, under `env`.
    pub fn tone(&mut self, at: f32, amp: f32, from: f32, to: f32, glide_time: f32, env: Env) {
        let (start, count) = self.span(at, env.length());
        let nyquist = 0.45 * self.rate;
        let mut phase = 0.0f32;
        for i in 0..count {
            let t = i as f32 / self.rate;
            self.samples[start + i] += amp * env.at(t) * phase.sin();
            phase += TAU * glide(from, to, glide_time, t).min(nyquist) / self.rate;
            if phase > TAU {
                phase -= TAU;
            }
        }
    }

    /// A struck resonant mode: a sine that starts at full level and decays.
    pub fn mode(&mut self, at: f32, freq: f32, decay: f32, amp: f32) {
        self.tone(at, amp, freq, freq, 0.0, Env::hit(decay));
    }

    /// Filtered white noise under `env`. The envelope goes on after the filter so a burst keeps
    /// its sharp front edge.
    pub fn noise(&mut self, at: f32, amp: f32, noise: Noise, env: Env) {
        let (start, count) = self.span(at, env.length());
        let mut filter = Svf::default();
        for i in 0..count {
            let t = i as f32 / self.rate;
            let x = self.rng.range(-1.0, 1.0);
            let taps = filter.run(x, noise.freq_at(t), noise.q, self.rate);
            let y = match noise.shape {
                Shape::Low => taps.low,
                Shape::Band => taps.band,
            };
            self.samples[start + i] += amp * env.at(t) * y;
        }
    }

    /// Mixes finished samples in at `at` seconds.
    pub fn add(&mut self, at: f32, gain: f32, other: &[f32]) {
        let (start, _) = self.span(at, other.len() as f32 / self.rate);
        for (s, o) in self.samples[start..].iter_mut().zip(other) {
            *s += gain * o;
        }
    }

    /// Cleans the sound up and scales it to `peak`.
    pub fn finish(mut self, peak: f32) -> Vec<f32> {
        // Low-passed noise and pitch glides leave a little DC; a 20 Hz blocker keeps every voice
        // centred so the mix never drifts.
        let r = 1.0 - TAU * 20.0 / self.rate;
        let (mut x1, mut y1) = (0.0, 0.0);
        for s in &mut self.samples {
            let y = *s - x1 + r * y1;
            x1 = *s;
            y1 = y;
            *s = y;
        }
        // Cut the tail once it is 50 dB down (inaudible under anything else, and it would hold
        // a voice) and fade the last 3 ms, so no sound ends on a step.
        let floor = 3e-3 * peak_of(&self.samples);
        let end = self.samples.iter().rposition(|s| s.abs() > floor).map_or(0, |i| i + 1);
        self.samples.truncate(end);
        let fade = ((0.003 * self.rate) as usize).min(end);
        for (i, s) in self.samples[end - fade..].iter_mut().enumerate() {
            *s *= 1.0 - (i + 1) as f32 / fade as f32;
        }
        normalise(&mut self.samples, peak);
        self.samples
    }
}

pub(crate) fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

pub(crate) fn normalise(samples: &mut [f32], peak: f32) {
    let max = peak_of(samples);
    if max > 0.0 {
        let gain = peak / max;
        samples.iter_mut().for_each(|s| *s *= gain);
    }
}
