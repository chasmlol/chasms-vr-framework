//! A line streamed to the game as it's made ([`VoiceStream`]): the voice goes on as the voice
//! server makes it, with its mouth's shapes. The mouth needs when each word is said, from speech
//! to text on the voice itself, so the voice goes a word at a time: as far as the last word speech
//! to text is sure of, cut in the gap after it, with the mouth for it: one a little way before the
//! end of what's come, with another word heard after it (the last heard may be the start of a
//! longer word: "a" of "Azura"). A sentence's end sends the rest.
//!
//! The voice server can make the voice slower than it's said (in the headset, the game's rendering
//! has most of the GPU), and then the game waits for it. Said a word at a time as it comes, that
//! stutters. So while it's slower (its speed measured as the voice comes) and the game has little
//! left to say (`LOW`), the voice goes only as far as a pause (a comma, a full stop: the last sure
//! word a pause follows), and with the game waiting, only once there's a good stretch of it
//! (`RESUME`) or its sentence's end: a wait, when there is one, is where a speaker would pause. A
//! line's first voice goes at its first pause, or a second in at a word's end (`FIRST_HOLD`).

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
/// The voice server slower than real time, and the game with less than this left to say (seconds),
/// the voice goes only as far as a pause.
const LOW: f32 = 0.6;
/// The game waiting, it goes on with at least this much (seconds), or the sentence's end.
const RESUME: f32 = 0.8;
/// A line's first voice waits this long for a pause (seconds), then goes at a word's end.
const FIRST_HOLD: f32 = 1.0;
/// A pause between two words: a gap this long (seconds), or this long after a comma or a stop.
const PAUSE: f32 = 0.15;
const MARKED_PAUSE: f32 = 0.05;
/// Held this long for a pause that doesn't come (seconds), the voice goes at a word's end.
const HOLD: f32 = 2.5;
/// The game not taking the voice for this long, the line is given up (seconds).
const STUCK: Duration = Duration::from_secs(3);

/// What the scene hears from m8, for the streamed line, in order.
pub enum Piece {
    Sentence { speaker: u32, text: String, how: String, level: Option<f32>, volume: f32 },
    /// The voice, its rate, and when it came (how fast it's made).
    Audio(Vec<f32>, u32, Instant),
    SentenceEnd,
    LineEnd,
}

/// The line being streamed.
struct Line {
    id: u32,
    speaker: u32,
    epoch: u64,
    rate: u32,
    /// Its VL_STREAM has gone (with its first voice); the game stopped taking it (the rest dropped).
    started: bool,
    given_up: bool,
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
    /// Its voice so far (samples), and when the first and the latest of it came.
    made: usize,
    first: Option<Instant>,
    latest: Option<Instant>,
}

impl Sentence {
    /// How many seconds of voice a second the server makes it at, once there's enough to tell.
    fn speed(&self, rate: u32) -> Option<f32> {
        let seconds = self.latest?.duration_since(self.first?).as_secs_f32();
        (self.made as f32 >= rate as f32 && seconds > 0.2).then(|| self.made as f32 / rate as f32 / seconds)
    }
}

/// Runs the streamed lines (the scene's thread for them): each piece with the stops' count when it
/// came. What came before a stop is dropped, but for a sentence's words: m8 stops the line the
/// game is still saying as the next one's first voice comes, after that sentence was begun.
pub fn run(link: Arc<Link>, transcriber: Transcriber, stops: Arc<AtomicU64>, pieces: Receiver<(Piece, u64)>) {
    // (numbered on from the clock, as the files are: no two runs share a number)
    let mut next = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 1_000_000) as u32 * 1000 + 500;
    let mut line: Option<Line> = None;
    // (the voice server's speed, seconds of voice a second: the last sentences', till this one's known)
    let mut speed = 1.0f32;
    let mut fresh = |speaker: u32, epoch: u64, sentence: Option<Sentence>| {
        next += 1;
        Line { id: next, speaker, epoch, rate: 0, started: false, given_up: false, gain: None, limiter: Limiter::default(), sent: 0, context: Vec::new(), sentence }
    };
    while let Ok((piece, epoch)) = pieces.recv() {
        let stale = epoch != stops.load(Ordering::SeqCst);
        if let Piece::Sentence { speaker, text, how, level, volume } = piece {
            let sentence = Sentence { text, how, level, volume, pending: Vec::new(), tried: 0, subtitled: false, made: 0, first: None, latest: None };
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
            Piece::Audio(samples, rate, came) => {
                l.rate = rate;
                let Some(s) = l.sentence.as_mut() else { continue };
                s.first.get_or_insert(came);
                s.latest = Some(came);
                s.made += samples.len();
                s.pending.extend(&samples);
                if (s.pending.len() - s.tried.min(s.pending.len())) as f32 >= STEP * rate as f32 {
                    send(&link, &transcriber, &stops, l, false, speed);
                }
            }
            Piece::SentenceEnd => {
                if let Some(measured) = l.sentence.as_ref().and_then(|s| s.speed(l.rate)) {
                    speed = 0.5 * speed + 0.5 * measured;
                }
                send(&link, &transcriber, &stops, l, true, speed);
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
/// gap after the last sure word (`at_pause`: the last sure word a pause follows, or, with none in
/// `hold` seconds, the last sure word), if that leaves at least `at_least` samples to go. 0: nothing
/// to send yet.
fn cut_at(words: &[Word], context: usize, pending: usize, rate: u32, at_pause: bool, at_least: usize, hold: f32) -> usize {
    let hop = (rate / LIPS_FPS).max(1) as usize;
    let r = rate as f32;
    let (from, end) = (context as f32 / r, (context + pending) as f32 / r);
    let next = |w: &Word| words.iter().map(|n| n.start).filter(|&s| s >= w.end - 0.01 && s > w.start).fold(f32::MAX, f32::min);
    let paused = |w: &Word| {
        let gap = next(w) - w.end;
        gap >= PAUSE || (gap >= MARKED_PAUSE && w.text.ends_with([',', '.', '!', '?', ';', ':']))
    };
    let sure: Vec<&Word> = words.iter().filter(|w| w.end > from && w.end <= end - SURE && next(w) < f32::MAX).collect();
    let cut = |w: &Word| {
        let t = ((w.end + next(w).max(w.end)) * 0.5).min(end - SURE * 0.5);
        ((t * r) as usize).saturating_sub(context).min(pending) / hop * hop
    };
    let pick = if at_pause { sure.iter().rev().find(|w| paused(w)) } else { sure.last() };
    match pick {
        Some(w) if cut(w) >= at_least => cut(w),
        // (no pause in a long stretch: at a word's end after all)
        _ if at_pause && !sure.is_empty() && pending as f32 > hold * r => sure.last().map_or(0, |w| cut(w)),
        Some(_) => 0,
        None if sure.is_empty() && pending as f32 > UNHEARD * r => (pending - (SURE * r) as usize) / hop * hop,
        None => 0,
    }
}

/// Sends what's sure of the sentence's voice (`last`: all of it) and its mouth; `speed`, the voice
/// server's lately (this sentence's, once there's enough of it to tell).
fn send(link: &Link, transcriber: &Transcriber, stops: &AtomicU64, l: &mut Line, last: bool, speed: f32) {
    let rate = l.rate;
    let Some(s) = l.sentence.as_mut() else { return };
    if rate == 0 || s.pending.is_empty() || l.given_up {
        s.pending.clear();
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
        // (how much the game has left to say: it hasn't started on the line, it has none)
        let (_, _, played) = link.voice_stream_progress();
        let left = l.sent.saturating_sub(played) as f32 / rate as f32;
        let slow = s.speed(rate).unwrap_or(speed) < 1.0;
        let at_least = if slow && l.started && left < 0.05 { (RESUME * rate as f32) as usize } else { 0 };
        let hold = if l.started { HOLD } else { FIRST_HOLD };
        cut_at(&words, context, s.pending.len(), rate, !l.started || (slow && left < LOW), at_least, hold)
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
        l.given_up = true;
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
        let cut = cut_at(&words, 0, rate as usize, rate, false, 0, HOLD);
        assert_eq!(cut % hop, 0);
        assert!((cut as f32 / rate as f32 - 0.785).abs() < 0.04, "{}", cut as f32 / rate as f32);
        // (0.7 s come: "me" ends 0.62, not sure until 0.77; only "Kill" is)
        let cut = cut_at(&words, 0, (0.7 * rate as f32) as usize, rate, false, 0, HOLD);
        assert!((cut as f32 / rate as f32 - 0.36).abs() < 0.04, "{}", cut as f32 / rate as f32);
        // (with a second of context before, the cut is into what's new)
        let shifted: Vec<Word> = words.iter().map(|w| word(&w.text, w.start + 1.0, w.end + 1.0)).collect();
        assert_eq!(cut_at(&shifted, rate as usize, rate as usize, rate, false, 0, HOLD), cut_at(&words, 0, rate as usize, rate, false, 0, HOLD));
        // (the last word heard, with none after it yet, may be the start of a longer one: not cut after)
        assert_eq!(cut_at(&words[..2], 0, rate as usize, rate, false, 0, HOLD), cut_at(&words, 0, (0.7 * rate as f32) as usize, rate, false, 0, HOLD));
        // (nothing heard: nothing goes, until there's been voice enough)
        assert_eq!(cut_at(&[], 0, rate as usize, rate, false, 0, HOLD), 0);
        assert!(cut_at(&[], 0, 2 * rate as usize, rate, false, 0, HOLD) > rate as usize);
    }

    #[test]
    fn with_little_left_to_say_the_voice_goes_to_a_pause() {
        let rate = 24000;
        let r = rate as f32;
        // ("Well, there's a man" then on: a pause after "Well," and none after "there's")
        let words = [word("Well,", 0.05, 0.30), word("there's", 0.45, 0.70), word("a", 0.72, 0.78), word("man", 0.80, 1.10), word("who", 1.12, 1.30)];
        let pending = (1.6 * r) as usize;
        // (at a word's end: after "man"; at a pause: after "Well,")
        assert!((cut_at(&words, 0, pending, rate, false, 0, HOLD) as f32 / r - 1.11).abs() < 0.04);
        assert!((cut_at(&words, 0, pending, rate, true, 0, HOLD) as f32 / r - 0.375).abs() < 0.04);
        // (the game waiting: not with less than it needs to go on)
        assert_eq!(cut_at(&words, 0, pending, rate, true, (0.8 * r) as usize, HOLD), 0);
        // (no pause in a long stretch: a word's end after all)
        let long = (2.6 * r) as usize;
        let on: Vec<Word> = (0..12).map(|i| word("word", 0.05 + i as f32 * 0.2, 0.22 + i as f32 * 0.2)).collect();
        assert!(cut_at(&on, 0, long, rate, true, 0, HOLD) > (2.0 * r) as usize);
    }
}
