//! Sample files for sound packs: Ogg Vorbis (lewton) and PCM WAV, decoded to `f32` and resampled
//! to the mixer's rate.
//!
//! Everything here runs on the pack loader's thread, never on the audio thread.

use std::f64::consts::PI;
use std::io::Cursor;

use anyhow::{Context, Result, bail, ensure};

/// Decoded audio: `channels` interleaved channels at `rate`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.rate.max(1) as f32
    }

    /// The largest absolute sample.
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0, |m, s| m.max(s.abs()))
    }

    /// One channel: the mean of all of them.
    pub fn mono(&self) -> Vec<f32> {
        let c = self.channels.max(1);
        if c == 1 {
            return self.samples.clone();
        }
        self.samples.chunks_exact(c).map(|f| f.iter().sum::<f32>() / c as f32).collect()
    }

    /// Two channels, interleaved: mono is copied to both, more than two keep the first two.
    pub fn stereo(&self) -> Vec<f32> {
        match self.channels {
            0 => Vec::new(),
            1 => self.samples.iter().flat_map(|&s| [s, s]).collect(),
            c => self.samples.chunks_exact(c).flat_map(|f| [f[0], f[1]]).collect(),
        }
    }
}

/// Decodes an Ogg Vorbis or WAV file, recognised by its contents (not its name).
pub fn decode(bytes: &[u8]) -> Result<Pcm> {
    if bytes.starts_with(b"OggS") {
        ogg(bytes)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        wav(bytes)
    } else {
        bail!("neither Ogg Vorbis nor WAV ({} bytes)", bytes.len())
    }
}

fn ogg(bytes: &[u8]) -> Result<Pcm> {
    let mut reader = lewton::inside_ogg::OggStreamReader::new(Cursor::new(bytes)).context("reading the Ogg Vorbis headers")?;
    let rate = reader.ident_hdr.audio_sample_rate;
    let channels = reader.ident_hdr.audio_channels as usize;
    ensure!(channels > 0 && rate > 0, "{channels} channels at {rate} Hz");
    let mut samples = Vec::new();
    while let Some(packet) = reader.read_dec_packet_generic::<Vec<Vec<f32>>>().context("decoding Vorbis")? {
        let n = packet.first().map_or(0, Vec::len);
        for i in 0..n {
            for ch in &packet {
                samples.push(ch.get(i).copied().unwrap_or(0.0));
            }
        }
    }
    Ok(Pcm { rate, channels, samples })
}

/// PCM WAV: 8, 16, 24 and 32-bit integers or 32-bit floats, plain or `WAVE_FORMAT_EXTENSIBLE`.
fn wav(bytes: &[u8]) -> Result<Pcm> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let mut at = 12;
    let mut format = None;
    while let (Some(id), Some(size)) = (bytes.get(at..at + 4), u32_at(at + 4)) {
        let body = at + 8;
        let size = size as usize;
        match id {
            b"fmt " => {
                let tag = u16_at(body).context("short fmt chunk")?;
                let channels = u16_at(body + 2).context("short fmt chunk")? as usize;
                let rate = u32_at(body + 4).context("short fmt chunk")?;
                let bits = u16_at(body + 14).context("short fmt chunk")?;
                // Extensible: the real format tag leads the sub-format GUID.
                let tag = if tag == 0xFFFE { u16_at(body + 24).context("short extensible fmt chunk")? } else { tag };
                format = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format.context("WAV data before its fmt chunk")?;
                ensure!(channels > 0 && rate > 0, "{channels} channels at {rate} Hz");
                let data = bytes.get(body..(body + size).min(bytes.len())).context("WAV data chunk out of range")?;
                let samples: Vec<f32> = match (tag, bits) {
                    (1, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
                    (1, 16) => data.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect(),
                    (1, 24) => data.as_chunks::<3>().0.iter().map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0).collect(),
                    (1, 32) => data.as_chunks::<4>().0.iter().map(|b| i32::from_le_bytes(*b) as f32 / 2_147_483_648.0).collect(),
                    (3, 32) => data.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect(),
                    _ => bail!("unsupported WAV format {tag} with {bits}-bit samples"),
                };
                return Ok(Pcm { rate, channels, samples });
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        at = body + size + (size & 1);
    }
    bail!("no WAV data chunk")
}

/// Zero crossings of the resampling kernel on each side.
const ZEROS: usize = 16;
/// Kernel table entries per zero crossing (linearly interpolated between).
const STEPS: usize = 512;

/// A windowed-sinc kernel sampled at `STEPS` points per zero crossing, out to `ZEROS`.
fn kernel() -> &'static [f32] {
    static TABLE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=ZEROS * STEPS + 1)
            .map(|i| {
                let x = i as f64 / STEPS as f64;
                let sinc = if x == 0.0 { 1.0 } else { (PI * x).sin() / (PI * x) };
                // Blackman-Harris over the kernel's span: about -90 dB side lobes.
                let w = 0.5 + 0.5 * (x / ZEROS as f64).min(1.0);
                let a = 2.0 * PI * w;
                let window = 0.35875 - 0.48829 * a.cos() + 0.14128 * (2.0 * a).cos() - 0.01168 * (3.0 * a).cos();
                if x >= ZEROS as f64 { 0.0 } else { (sinc * window) as f32 }
            })
            .collect()
    })
}

/// One channel from `from` Hz to `to` Hz: band-limited (windowed sinc) interpolation, with the
/// cutoff a little under the lower of the two Nyquist frequencies.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let table = kernel();
    let ratio = to as f64 / from as f64;
    let cutoff = ratio.min(1.0) * 0.96;
    // Input samples on each side of the output point that the kernel reaches.
    let reach = (ZEROS as f64 / cutoff).ceil() as isize;
    let frames = (x.len() as f64 * ratio).round() as usize;
    let tap = |d: f64| -> f32 {
        let u = (d.abs() * cutoff) * STEPS as f64;
        let i = u as usize;
        if i >= ZEROS * STEPS {
            return 0.0;
        }
        let f = (u - i as f64) as f32;
        table[i] + (table[i + 1] - table[i]) * f
    };
    (0..frames)
        .map(|n| {
            let t = n as f64 / ratio;
            let centre = t.floor() as isize;
            let lo = (centre - reach + 1).max(0);
            let hi = (centre + reach).min(x.len() as isize - 1);
            let mut acc = 0.0f32;
            for k in lo..=hi {
                acc += x[k as usize] * tap(t - k as f64);
            }
            acc * cutoff as f32
        })
        .collect()
}

/// A 16-bit PCM WAV of interleaved samples (tests, and listening to what a pack decoded to).
pub fn wav_bytes(pcm: &Pcm) -> Vec<u8> {
    let channels = pcm.channels.max(1) as u16;
    let data = (pcm.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&pcm.rate.to_le_bytes());
    out.extend_from_slice(&(pcm.rate * 2 * channels as u32).to_le_bytes());
    out.extend_from_slice(&(2 * channels).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in &pcm.samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}
