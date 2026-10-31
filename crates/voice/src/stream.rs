//! A line streamed to the game as it's made ([`VoiceStream`]): the voice goes on as the voice
//! server makes it, with its mouth's shapes. The mouth needs when each word is said, from speech
//! to text on the voice itself, so the voice goes a word at a time: as far as the last word speech
//! to text is sure of, cut in the gap after it, with the mouth for it: one a little way before the
//! end of what's come, with another word heard after it (the last heard may be the start of a
//! longer word: "a" of "Azura"). A sentence's end sends the rest.

use crate::scene::{speech_level, Limiter, LIPS_FPS};
use m8::listen::{Transcriber, Word};
use rustvr_protocol::link::Link;
use rustvr_protocol::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Speech to text is asked again once this much more voice has come (seconds).
const STEP: f32 = 0.25;
/// A word is sure once the voice has run this long past its end (seconds).
const SURE: f32 = 0.15;
/// What was sent before is heard again with what's new: speech to text hears the new words in
/// their sentence, and the mouth opens as loud as the voice has been (seconds).
const CONTEXT: f32 = 1.0;
/// Speech to text hearing no word in this much voice (seconds), it goes anyway, to a little way
/// before the end of what's come, the mouth from its loudness.
const UNHEARD: f32 = 1.5;
/// The game not taking the voice for this long, the line is given up (seconds).
const STUCK: Duration = Duration::from_secs(3);

/// What the scene hears from m8, for the streamed line, in order.
pub enum Piece {
    Sentence { speaker: u32, text: String, how: String, level: Option<f32>, volume: f32 },
    Audio(Vec<f32>, u32),
    SentenceEnd,
    LineEnd,
}

/// The line being streamed.
struct Line {
    id: u32,
    speaker: u32,
    epoch: u64,
    rate: u32,
    /// Its VL_STREAM has gone (with its first voice).
    started: bool,
    /// The gain making it as loud as the game's own voice for them (fixed at its first voice).
    gain: Option<f32>,
    limiter: Limiter,
    /// The samples gone to the game.
    sent: u64,
    /// The last of what went, for speech to text and the mouth (a whole number of the mouth's
    /// frames long).
    context: Vec<f32>,
    sentence: Option<Sentence>,
}

struct Sentence {
    text: String,
    how: String,
    level: Option<f32>,
    volume: f32,
    /// Its voice not gone yet.
    pending: Vec<f32>,
    /// How much was pending when speech to text was last asked.
    tried: usize,
    subtitled: bool,
}

/// Runs the streamed lines (the scene's thread for them): each piece with the stops' count when it
/// came. What came before a stop is dropped, but for a sentence's words: m8 stops the line the
/// game is still saying as the next one's first voice comes, after that sentence was begun.
pub fn run(link: Arc<Link>, transcriber: Transcriber, stops: Arc<AtomicU64>, pieces: Receiver<(Piece, u64)>) {
    // (numbered on from the clock, as the files are: no two runs share a number)
    let mut next = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 1_000_000) as u32 * 1000 + 500;
    let mut line: Option<Line> = None;
    let mut fresh = |speaker: u32, epoch: u64, sentence: Option<Sentence>| {
        next += 1;
        Line { id: next, speaker, epoch, rate: 0, started: false, gain: None, limiter: Limiter::default(), sent: 0, context: Vec::new(), sentence }
    };
    while let Ok((piece, epoch)) = pieces.recv() {
        let stale = epoch != stops.load(Ordering::SeqCst);
        if let Piece::Sentence { speaker, text, how, level, volume } = piece {
            let sentence = Sentence { text, how, level, volume, pending: Vec::new(), tried: 0, subtitled: false };
            match line.as_mut() {
                Some(l) if !(l.started && l.epoch != epoch) => l.sentence = Some(sentence),
                _ => line = Some(fresh(speaker, epoch, Some(sentence))),
            }
            continue;
        }
        if stale {
            continue;
        }
        let Some(l) = line.as_mut() else { continue };
        // (a stop since it began: the line that was is over, its sentence is the next's)
        if l.epoch != epoch {
            if l.started {
                let (speaker, sentence) = (l.speaker, l.sentence.take());
                *l = fresh(speaker, epoch, sentence);
            } else {
                l.epoch = epoch;
            }
        }
        match piece {
            Piece::Audio(samples, rate) => {
                l.rate = rate;
                let Some(s) = l.sentence.as_mut() else { continue };
                s.pending.extend(&samples);
                if (s.pending.len() - s.tried.min(s.pending.len())) as f32 >= STEP * rate as f32 {
                    send(&link, &transcriber, &stops, l, false);
                }
            }
            Piece::SentenceEnd => {
                send(&link, &transcriber, &stops, l, true);
                l.sentence = None;
            }
            Piece::LineEnd => {
                if let Some(l) = line.take().filter(|l| l.started) {
                    let mut end = VoiceLine::new(VL_END, l.speaker, l.id, 0, "", "");
                    end.at = l.sent as u32;
                    link.push_voice_line(&end);
                }
            }
            Piece::Sentence { .. } => {}
        }
    }
}

/// Where to cut what's pending (samples into it, a whole number of the mouth's frames), given the
/// words speech to text heard in `context` and it (their times from the context's start): in the
/// gap after the last sure word. 0: nothing sure yet.
fn cut_at(words: &[Word], context: usize, pending: usize, rate: u32) -> usize {
    let hop = (rate / LIPS_FPS).max(1) as usize;
    let r = rate as f32;
    let (from, end) = (context as f32 / r, (context + pending) as f32 / r);
    let next = |w: &Word| words.iter().map(|n| n.start).filter(|&s| s >= w.end - 0.01 && s > w.start).fold(f32::MAX, f32::min);
    let sure: Vec<&Word> = words.iter().filter(|w| w.end > from && w.end <= end - SURE && next(w) < f32::MAX).collect();
    match sure.last() {
        Some(last) => {
            let t = ((last.end + next(last).max(last.end)) * 0.5).min(end - SURE * 0.5);
            ((t * r) as usize).saturating_sub(context).min(pending) / hop * hop
        }
        None if pending as f32 > UNHEARD * r => (pending - (SURE * r) as usize) / hop * hop,
        None => 0,
    }
}

/// Sends what's sure of the sentence's voice (`last`: all of it) and its mouth.
fn send(link: &Link, transcriber: &Transcriber, stops: &AtomicU64, l: &mut Line, last: bool) {
    let rate = l.rate;
    let Some(s) = l.sentence.as_mut() else { return };
    if rate == 0 || s.pending.is_empty() {
        return;
    }
    let hop = (rate / LIPS_FPS).max(1) as usize;
    s.tried = s.pending.len();
    let context = l.context.len();
    let mut window = l.context.clone();
    window.extend(&s.pending);
    let words = transcriber.words(&window, rate).map(|(_, w)| w).unwrap_or_else(|e| {
        eprintln!("[voice] no word times for the stream: {e:#}");
        Vec::new()
    });
    let cut = if last {
        // (the sentence's end: all of it, to a whole frame, with a moment's silence)
        let whole = s.pending.len().div_ceil(hop) * hop;
        s.pending.resize(whole, 0.0);
        window.resize(context + whole, 0.0);
        whole
    } else {
        cut_at(&words, context, s.pending.len(), rate)
    };
    if cut == 0 {
        return;
    }
    let upto = context + cut;
    let heard: Vec<Word> = words.into_iter().filter(|w| w.end * rate as f32 <= upto as f32).collect();
    // (exactly a frame for each `hop` of voice sent: the mouth keeps time with the voice)
    let mut frames = m8::lips::frames(&window[..upto], rate, &heard, &s.text, LIPS_FPS as f32);
    frames.resize(upto / hop, [0.0; 16]);
    let frames = &frames[context / hop..];
    let voice = &s.pending[..cut];
    let gain = *l.gain.get_or_insert_with(|| match (s.level, speech_level(voice, rate)) {
        (Some(want), Some(have)) => (want / have).clamp(0.25, 8.0),
        _ => 1.0,
    }) * s.volume;
    let loud: Vec<f32> = voice.iter().map(|x| x * gain).collect();
    let pcm: Vec<i16> = l.limiter.run(&loud, rate).iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    if !l.started {
        l.started = true;
        link.begin_voice_stream(l.id, rate);
        let mut start = VoiceLine::new(VL_STREAM, l.speaker, l.id, 0, &s.how, "");
        start.at = rate;
        link.push_voice_line(&start);
    }
    if !s.subtitled {
        s.subtitled = true;
        let mut said = VoiceLine::new(VL_SUBTITLE, l.speaker, l.id, 0, &s.how, &s.text);
        said.at = l.sent as u32;
        link.push_voice_line(&said);
    }
    // (the mouth first: the game plays no voice its mouth hasn't come for)
    if !put(stops, l.epoch, frames, |f| link.write_voice_frames(f)) || !put(stops, l.epoch, &pcm, |p| link.write_voice_samples(p)) {
        eprintln!("[voice] the game took no more of line {}: given up", l.id);
        s.pending.clear();
        return;
    }
    l.sent += cut as u64;
    l.context.extend(&s.pending[..cut]);
    let keep = ((CONTEXT * rate as f32) as usize / hop * hop).min(l.context.len() / hop * hop);
    l.context.drain(..l.context.len() - keep);
    s.pending.drain(..cut);
    s.tried = s.pending.len();
}

/// Writes all of `items` with `write` (which takes what its ring has room for), waiting while the
/// game takes them; false if it stops taking them, or the line's stopped.
fn put<T>(stops: &AtomicU64, epoch: u64, mut items: &[T], write: impl Fn(&[T]) -> usize) -> bool {
    let started = Instant::now();
    while !items.is_empty() {
        let n = write(items);
        items = &items[n..];
        if items.is_empty() {
            break;
        }
        if stops.load(Ordering::SeqCst) != epoch || started.elapsed() > STUCK {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f32, end: f32) -> Word {
        Word { text: text.into(), start, end }
    }

    #[test]
    fn the_voice_goes_to_the_gap_after_the_last_sure_word() {
        let rate = 24000;
        let hop = (rate / LIPS_FPS) as usize;
        let words = [word("Kill", 0.05, 0.32), word("me", 0.40, 0.62), word("By", 0.95, 1.05)];
        // (1.0 s come: "me" is sure, "By" isn't yet; the cut is midway between "me" and "By")
        let cut = cut_at(&words, 0, rate as usize, rate);
        assert_eq!(cut % hop, 0);
        assert!((cut as f32 / rate as f32 - 0.785).abs() < 0.04, "{}", cut as f32 / rate as f32);
        // (0.7 s come: "me" ends 0.62, not sure until 0.77; only "Kill" is)
        let cut = cut_at(&words, 0, (0.7 * rate as f32) as usize, rate);
        assert!((cut as f32 / rate as f32 - 0.36).abs() < 0.04, "{}", cut as f32 / rate as f32);
        // (with a second of context before, the cut is into what's new)
        let shifted: Vec<Word> = words.iter().map(|w| word(&w.text, w.start + 1.0, w.end + 1.0)).collect();
        assert_eq!(cut_at(&shifted, rate as usize, rate as usize, rate), cut_at(&words, 0, rate as usize, rate));
        // (the last word heard, with none after it yet, may be the start of a longer one: not cut after)
        assert_eq!(cut_at(&words[..2], 0, rate as usize, rate), cut_at(&words, 0, (0.7 * rate as f32) as usize, rate));
        // (nothing heard: nothing goes, until there's been voice enough)
        assert_eq!(cut_at(&[], 0, rate as usize, rate), 0);
        assert!(cut_at(&[], 0, 2 * rate as usize, rate) > rate as usize);
    }
}
