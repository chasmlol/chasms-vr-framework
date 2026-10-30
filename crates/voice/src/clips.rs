//! A voice's clip, made from the game's own lines in that voice: the clips the adapter put in the
//! voice's folder (`<k>.mp3` or `<k>.wav`, each with `<k>.txt`), joined into the one clip m8's
//! voice clones (`reference.wav`, `reference.txt`). Breeze clones best from 5 to 20 seconds of
//! clean speech, so lines are taken in order until there are enough.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Enough of a voice to clone it, and the most that's worth sending with every line.
const ENOUGH_SECONDS: f32 = 10.0;
const MOST_SECONDS: f32 = 18.0;
/// A breath between the lines joined.
const GAP_SECONDS: f32 = 0.3;

/// The clip of the voice in `dir`: `reference.wav`, made from its lines the first time.
pub fn reference(dir: &Path) -> Result<PathBuf> {
    let wav = dir.join("reference.wav");
    if wav.exists() && dir.join("reference.txt").exists() {
        return Ok(wav);
    }
    let mut lines = Vec::new();
    for k in 1.. {
        let text = dir.join(format!("{k}.txt"));
        let Some(audio) = ["mp3", "wav"].iter().map(|e| dir.join(format!("{k}.{e}"))).find(|p| p.exists()) else { break };
        if !text.exists() {
            break;
        }
        lines.push((audio, std::fs::read_to_string(&text)?.trim().to_string()));
    }
    if lines.is_empty() {
        bail!("no lines in {} to clone the voice from", dir.display());
    }
    let mut rate = 0;
    let (mut samples, mut said) = (Vec::new(), Vec::new());
    for (path, text) in &lines {
        let (clip, clip_rate) = match decode(&std::fs::read(path)?, path) {
            Ok(decoded) => decoded,
            Err(e) => {
                eprintln!("[voice] skipping {}: {e:#}", path.display());
                continue;
            }
        };
        if rate == 0 {
            rate = clip_rate;
        }
        let clip = resample(&clip, clip_rate, rate);
        let seconds = |n: usize| n as f32 / rate as f32;
        if !samples.is_empty() && seconds(samples.len() + clip.len()) > MOST_SECONDS {
            continue;
        }
        if !samples.is_empty() {
            samples.extend(std::iter::repeat_n(0.0, (GAP_SECONDS * rate as f32) as usize));
        }
        samples.extend(trim(&clip, rate));
        said.push(text.clone());
        if seconds(samples.len()) >= ENOUGH_SECONDS {
            break;
        }
    }
    if samples.is_empty() {
        bail!("none of the lines in {} could be read", dir.display());
    }
    std::fs::write(&wav, wav_bytes(&samples, rate)).with_context(|| format!("writing {}", wav.display()))?;
    std::fs::write(dir.join("reference.txt"), said.join(" "))?;
    println!("[voice] {}: {:.1} s from {} of its lines", dir.display(), samples.len() as f32 / rate as f32, said.len());
    Ok(wav)
}

/// An MP3 or WAV's audio, as mono samples at its rate.
fn decode(bytes: &[u8], path: &Path) -> Result<(Vec<f32>, u32)> {
    let source = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe().format(&hint, source, &FormatOptions::default(), &MetadataOptions::default())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or_else(|| anyhow!("no audio in it"))?;
    let (id, rate) = (track.id, track.codec_params.sample_rate.ok_or_else(|| anyhow!("no sample rate"))?);
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut mono = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // (a damaged frame is skipped, as players do)
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let channels = decoded.spec().channels.count().max(1);
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        buffer.copy_interleaved_ref(decoded);
        mono.extend(buffer.samples().chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
    }
    Ok((mono, rate))
}

/// The clip without the near-silence at its ends (MP3s pad both), keeping a tenth of a second.
fn trim(samples: &[f32], rate: u32) -> Vec<f32> {
    let loud = |s: &&f32| s.abs() > 0.01;
    let keep = rate as usize / 10;
    let start = samples.iter().position(|s| loud(&s)).unwrap_or(0).saturating_sub(keep);
    let end = (samples.iter().rposition(|s| loud(&s)).map_or(samples.len(), |e| e + 1) + keep).min(samples.len());
    samples[start..end.max(start)].to_vec()
}

/// Straight-line resampling: the clips are speech, and nearly always at one rate already.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let n = (samples.len() as u64 * to as u64 / from as u64) as usize;
    (0..n)
        .map(|i| {
            let at = i as f64 * from as f64 / to as f64;
            let (j, f) = (at as usize, (at.fract()) as f32);
            let a = samples[j.min(samples.len() - 1)];
            let b = samples[(j + 1).min(samples.len() - 1)];
            a + (b - a) * f
        })
        .collect()
}

/// A mono 16-bit WAV.
pub fn wav_bytes(samples: &[f32], rate: u32) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for s in samples {
        out.extend(((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_join_into_one_clip_and_its_transcript() {
        let dir = std::env::temp_dir().join(format!("rustvr-voice-clips-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tone = |seconds: f32| (0..(seconds * 16000.0) as usize).map(|i| 0.5 * (i as f32 * 0.07).sin()).collect::<Vec<f32>>();
        for (k, (seconds, text)) in [(4.0, "Stop right there."), (5.0, "You've committed crimes."), (9.0, "Too long to fit.")].iter().enumerate() {
            std::fs::write(dir.join(format!("{}.wav", k + 1)), wav_bytes(&tone(*seconds), 16000)).unwrap();
            std::fs::write(dir.join(format!("{}.txt", k + 1)), text).unwrap();
        }
        let wav = reference(&dir).unwrap();
        let said = std::fs::read_to_string(dir.join("reference.txt")).unwrap();
        // (the first two make 9.3 s, short of enough; the third would pass the most there's room for)
        assert_eq!(said, "Stop right there. You've committed crimes.");
        let (joined, rate) = decode(&std::fs::read(&wav).unwrap(), &wav).unwrap();
        assert_eq!(rate, 16000);
        assert!((joined.len() as f32 / 16000.0 - 9.3).abs() < 0.05, "{}", joined.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
