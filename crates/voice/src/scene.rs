//! The game, for m8: what the adapter says in the voice regions (who's around, who's listening,
//! who they are), and m8's lines going back to it. Streamed (RustVR's Voice setting, on by
//! default), a line's voice goes to the game as it's made, with its mouth ([`crate::stream`]).
//! Otherwise each sentence's voice is gathered whole, a clause at a time, the mouth's shapes are
//! made for it (when each word is said, from speech to text on the voice itself), and its files
//! written where the game plays them from, before the adapter is told.

use crate::clips;
use crate::stream::Piece;
use m8::game::Game;
use m8::listen::Transcriber;
use rustvr_protocol::link::Link;
use rustvr_protocol::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The mouth's frames a second (Oblivion's lip files are at 30).
pub(crate) const LIPS_FPS: u32 = VOICE_STREAM_FPS;
/// A sentence's files are kept this many lines, then deleted (the game has long played them).
const LINES_KEPT: u32 = 32;

/// m8's id for a speaker: the game's, as hex.
pub fn speaker_id(id: u32) -> String {
    format!("{id:08X}")
}

fn game_id(speaker: &str) -> u32 {
    u32::from_str_radix(speaker, 16).unwrap_or(0)
}

/// A sentence being voiced: its words, how they're said, and the voice so far. A clause cut from
/// it carries the whole sentence's words: its own are as many as speech to text hears in it, on
/// from the clauses before (`last`: the sentence's end, the rest of them).
struct Voicing {
    speaker: String,
    text: String,
    how: String,
    samples: Vec<f32>,
    rate: u32,
    last: bool,
    /// How loud the game's own voice for them is (the RMS of its clip's speech): every line of
    /// theirs is made as loud, as the game plays its own lines.
    level: Option<f32>,
    /// The player's volume for the characters' voices (RustVR's Voice settings).
    volume: f32,
}

/// A long sentence goes to the game a clause at a time, at the voice's own pauses: the game plays
/// whole files, and its first words shouldn't wait for the voice of all of it. A clause is at
/// least this long (seconds; a line's first, sooner, as everything waits on it; the rest are
/// ready while the one before plays), and a pause at least `PAUSE`.
const CLAUSE: f32 = 2.0;
const FIRST_CLAUSE: f32 = 0.8;
/// With the game about to run out of what it has to say (within `RUNNING_OUT` seconds), the next
/// clause goes at the first pause after this: a short clause now rather than a gap. (A voice made
/// faster than it's said never gets there, and its clauses stay long.) A voice made slower than
/// it's said leaves the game waiting all the same, at a pause, where a speaker would.
const HURRIED_CLAUSE: f32 = 0.8;
const RUNNING_OUT: f32 = 0.4;
const PAUSE: f32 = 0.12;
/// Only in a hurry with this much voice and no pause in it yet, a cut at a gap this short: a
/// quiet this short is mostly inside a word (the closure of a "p" or a "t"), not between two, and
/// the word comes out in two halves.
const NO_PAUSE: f32 = 3.0;
const WORD_GAP: f32 = 0.04;

/// A clip's loudness while it speaks: the RMS of its 10 ms windows louder than a tenth of its
/// loudest (its pauses left out).
pub(crate) fn speech_level(samples: &[f32], rate: u32) -> Option<f32> {
    let window = (rate / 100).max(1) as usize;
    let rms: Vec<f32> = samples.chunks_exact(window).map(|w| (w.iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt()).collect();
    let loud = rms.iter().copied().fold(0.0, f32::max);
    let voiced: Vec<f32> = rms.into_iter().filter(|&r| r > loud * 0.1).collect();
    (!voiced.is_empty() && loud > 0.0).then(|| (voiced.iter().map(|r| r * r).sum::<f32>() / voiced.len() as f32).sqrt())
}

/// The loudest a line's samples go (-0.5 dB).
const CEILING: f32 = 0.94;
/// How far ahead the limiter turns a line down for a peak, and how fast it comes back up after.
const LOOKAHEAD: f32 = 0.005;
const RELEASE: f32 = 0.08;

/// A line made as loud as the game's own voice (`level`) times the player's `volume`. Its peaks
/// are kept under the `CEILING` by a limiter: the line turned down around a peak, smoothly (from
/// `LOOKAHEAD` before it, back up over `RELEASE` after), not its waveform bent, which crackles. (The
/// game's own clips have room to spare; ours are as loud as the game plays them, and the voice is
/// 3D in the world where the dialogue menu's is 2D, a few dB louder, so the player turns them up.)
fn as_loud(samples: &[f32], rate: u32, level: Option<f32>, volume: f32) -> Vec<f32> {
    let gain = match (level, speech_level(samples, rate)) {
        (Some(want), Some(have)) => (want / have).clamp(0.25, 8.0) * volume,
        _ => volume,
    };
    let loud: Vec<f32> = samples.iter().map(|s| s * gain).collect();
    Limiter::default().run(&loud, rate)
}

/// The limiter (see [`as_loud`]), over a line's voice piece by piece: how far it's turned down
/// carries on from one piece to the next.
pub(crate) struct Limiter {
    turned: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter { turned: 1.0 }
    }
}

impl Limiter {
    pub(crate) fn run(&mut self, loud: &[f32], rate: u32) -> Vec<f32> {
        let n = loud.len();
        let ahead = ((LOOKAHEAD * rate as f32) as usize).max(1);
        // (each sample's most it may be turned up; the least of those within `ahead` of each;
        // their average across `ahead` either side, which is never more than any sample's own
        // most, but ramps down to a peak and up from it)
        let most: Vec<f32> = loud.iter().map(|x| if x.abs() > CEILING { CEILING / x.abs() } else { 1.0 }).collect();
        let mut least = vec![1.0f32; n];
        for (i, l) in least.iter_mut().enumerate() {
            *l = most[i.saturating_sub(ahead)..(i + ahead + 1).min(n)].iter().copied().fold(1.0, f32::min);
        }
        let mut sums = vec![0.0f64; n + 1];
        for i in 0..n {
            sums[i + 1] = sums[i] + least[i] as f64;
        }
        let back = 1.0 - (-1.0 / (RELEASE * rate as f32)).exp();
        loud.iter()
            .enumerate()
            .map(|(i, x)| {
                let (from, to) = (i.saturating_sub(ahead), (i + ahead + 1).min(n));
                let smooth = ((sums[to] - sums[from]) / (to - from) as f64) as f32;
                self.turned = smooth.min(self.turned + (1.0 - self.turned) * back);
                x * self.turned
            })
            .collect()
    }
}

/// Where to cut the voice so far: the middle of its first pause after `clause` seconds (a pause:
/// `pause` seconds of 10 ms windows all under a tenth of the voice's loudest), if there's one yet.
fn pause_in(samples: &[f32], rate: u32, clause: f32, pause: f32) -> Option<usize> {
    let window = (rate / 100).max(1) as usize;
    let rms: Vec<f32> = samples.chunks_exact(window).map(|w| (w.iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt()).collect();
    let loud = rms.iter().copied().fold(0.0, f32::max);
    if loud <= 0.0 {
        return None;
    }
    let (first, needed) = ((clause * 100.0) as usize, ((pause * 100.0) as usize).max(2));
    let mut run = 0;
    for (i, &r) in rms.iter().enumerate().skip(first) {
        if r < loud * 0.1 {
            run += 1;
            if run == needed {
                return Some((i + 1 - needed / 2) * window);
            }
        } else {
            run = 0;
        }
    }
    None
}

/// Work for the thread that makes each sentence's files: in order, so the game hears them in
/// order. `epoch` is the stops' count when it was queued: a stop drops what came before it.
enum Job {
    Sentence(Voicing, u64),
    End(String, u64),
}

pub struct Scene {
    link: Arc<Link>,
    /// The adapter's latest scene.
    scene: Mutex<VoiceScene>,
    /// What each speaker was, the last time they were the listener: their persona stays theirs
    /// while a line of theirs is written, whoever's listening by then.
    known: Mutex<HashMap<u32, Value>>,
    template: m8::site::PromptFile,
    voicing: Mutex<Option<Voicing>>,
    jobs: Sender<Job>,
    stops: Arc<AtomicU64>,
    /// The listener when the player last started talking (before the words were known).
    addressed: AtomicU32,
    /// The voices whose clip couldn't be made yet (said once each).
    unvoiced: Mutex<std::collections::HashSet<String>>,
    /// Nothing of the line being voiced has gone to the game yet.
    fresh: std::sync::atomic::AtomicBool,
    /// When the game will have said everything it's been given.
    playing_until: Arc<Mutex<Instant>>,
    /// Each voice's loudness, by its clip.
    levels: Mutex<HashMap<PathBuf, Option<f32>>>,
    /// The player's volume for the characters' voices (an `f32`'s bits), as RustVR's settings have it.
    volume: AtomicU32,
    /// Lines streamed as they're made, to [`crate::stream`]'s thread (None: a clause at a time).
    stream: Option<Mutex<Sender<(Piece, u64)>>>,
}

impl Scene {
    pub fn new(link: Arc<Link>, first: VoiceScene, template: m8::site::PromptFile, transcriber: Transcriber, streamed: bool) -> Arc<Scene> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let stops = Arc::new(AtomicU64::new(0));
        let stream = streamed.then(|| {
            let (pieces, taken) = mpsc::channel::<(Piece, u64)>();
            let (link, transcriber, stops) = (link.clone(), transcriber.clone(), stops.clone());
            std::thread::spawn(move || crate::stream::run(link, transcriber, stops, taken));
            Mutex::new(pieces)
        });
        let playing_until = Arc::new(Mutex::new(Instant::now()));
        {
            let (link, stops, playing_until) = (link.clone(), stops.clone(), playing_until.clone());
            let lines_dir = PathBuf::from(first.lines_dir());
            std::thread::spawn(move || {
                // (numbered on from the clock, so no run's files share a name with an earlier run's:
                // a game may keep a sound it played by its file's name)
                let mut line = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 1_000_000) as u32 * 1000;
                let first = line;
                // (the words of the sentence being said that its clauses so far have taken)
                let mut taken = 0;
                for job in queue {
                    match job {
                        Job::Sentence(voicing, epoch) => {
                            if epoch != stops.load(Ordering::SeqCst) {
                                continue;
                            }
                            line += 1;
                            match write_line(&lines_dir, line, &voicing, &transcriber, &mut taken) {
                                Ok((ms, said)) if epoch == stops.load(Ordering::SeqCst) => {
                                    // (the game says it once it's said what it has: until then, plus its length)
                                    let mut until = playing_until.lock().unwrap();
                                    *until = (*until).max(Instant::now()) + std::time::Duration::from_millis(ms as u64);
                                    drop(until);
                                    link.push_voice_line(&VoiceLine::new(VL_SAY, game_id(&voicing.speaker), line, ms, &voicing.how, &said));
                                }
                                Ok(_) => {}
                                Err(e) if e.to_string() == "only silence" => line -= 1,
                                Err(e) => eprintln!("[voice] line {line} not written: {e:#}"),
                            }
                            if line > first + LINES_KEPT {
                                for extension in ["wav", "lips", "lip"] {
                                    let _ = std::fs::remove_file(lines_dir.join(format!("{}.{extension}", line - LINES_KEPT)));
                                }
                            }
                        }
                        Job::End(speaker, epoch) => {
                            if epoch == stops.load(Ordering::SeqCst) {
                                link.push_voice_line(&VoiceLine::new(VL_END, game_id(&speaker), line, 0, "", ""));
                            }
                        }
                    }
                }
            });
        }
        Arc::new(Scene {
            link,
            scene: Mutex::new(first),
            known: Mutex::new(HashMap::new()),
            template,
            voicing: Mutex::new(None),
            jobs,
            stops,
            addressed: AtomicU32::new(0),
            unvoiced: Mutex::new(Default::default()),
            fresh: std::sync::atomic::AtomicBool::new(true),
            playing_until,
            levels: Mutex::new(HashMap::new()),
            volume: AtomicU32::new(1.0f32.to_bits()),
            stream,
        })
    }

    /// The adapter's scene changed.
    pub fn update(&self, scene: VoiceScene) {
        if let Some(listener) = scene.speaker(scene.listener) {
            let state = self.describe(&scene, listener);
            self.known.lock().unwrap().insert(listener.id, state);
        }
        *self.scene.lock().unwrap() = scene;
    }

    /// m8's state for the listener: who they are (the prompt around the game's words for them),
    /// where they are, their voice.
    fn describe(&self, scene: &VoiceScene, who: &VoiceSpeaker) -> Value {
        let persona = self
            .template
            .get()
            .replace("{name}", who.name())
            .replace("{persona}", scene.persona().trim())
            .replace("{game}", scene.game());
        let voice = match clips::reference(&PathBuf::from(scene.voices_dir()).join(who.voice())) {
            Ok(voice) => Some(voice),
            Err(e) => {
                if self.unvoiced.lock().unwrap().insert(who.voice().to_string()) {
                    eprintln!("[voice] {}'s voice ({}) isn't there yet: {e:#}", who.name(), who.voice());
                }
                None
            }
        };
        json!({
            "name": who.name(),
            "persona": persona,
            "voice": voice,
            "state": scene.situation(),
            "player_name": scene.player(),
            "game": scene.game(),
        })
    }
}

/// A word as speech to text and the LLM would both write it: its letters and digits, lower case.
fn plain(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Where a clause's words end in its sentence's (the LLM's) words, from `from`: as many as speech
/// to text heard in it, moved to just after the sentence's word that matches the last one heard,
/// if one does within a few words (it can hear two words as one, or miss one at a cut).
fn clause_end(all: &[&str], from: usize, heard: &[m8::listen::Word]) -> usize {
    // (only words count: a dash between them is said as a pause)
    let mut guess = from;
    let mut counted = 0;
    while guess < all.len() && counted < heard.len().max(1) {
        if !plain(all[guess]).is_empty() {
            counted += 1;
        }
        guess += 1;
    }
    let Some(last) = heard.last().map(|w| plain(&w.text)).filter(|w| !w.is_empty()) else { return guess };
    (0..=3usize)
        .flat_map(|d| [guess as isize + d as isize, guess as isize - d as isize])
        .filter(|&end| end > from as isize && end as usize <= all.len())
        .find(|&end| plain(all[end as usize - 1]) == last)
        .map_or(guess, |end| end as usize)
}

/// Writes a sentence's (or a clause's) voice and mouth: `<line>.wav` and `<line>.lips`. Its
/// length, in ms, and its words (`taken`: the sentence's words that clauses before it took).
fn write_line(dir: &std::path::Path, line: u32, voicing: &Voicing, transcriber: &Transcriber, taken: &mut usize) -> anyhow::Result<(u32, String)> {
    let (heard, words) = transcriber.words(&voicing.samples, voicing.rate).unwrap_or_else(|e| {
        eprintln!("[voice] no word times for line {line}: {e:#}");
        (String::new(), Vec::new())
    });
    let all: Vec<&str> = voicing.text.split_whitespace().collect();
    let from = (*taken).min(all.len());
    let to = if voicing.last { all.len() } else { clause_end(&all, from, &words) };
    *taken = if voicing.last { 0 } else { to };
    // (a clause doesn't start with the dash or comma it was cut after)
    let said = all[from..to].join(" ").trim_start_matches(['\u{2014}', '\u{2013}', '-', ',', ';', ' ']).to_string();
    // A sentence's tail with none of its words left: silence, or a sound the voice added that
    // the line hasn't got ("mm", "yeah"), unless what's heard is one of the sentence's last words
    // (its clauses ran ahead of the voice): nothing to say.
    let ending: Vec<String> = all.iter().rev().take(3).map(|w| plain(w)).collect();
    if voicing.last && said.is_empty() && !words.iter().any(|w| ending.contains(&plain(&w.text))) {
        anyhow::bail!("only silence");
    }
    let frames = m8::lips::frames(&voicing.samples, voicing.rate, &words, &said, LIPS_FPS as f32);
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{line}.wav")), clips::wav_bytes(&as_loud(&voicing.samples, voicing.rate, voicing.level, voicing.volume), voicing.rate))?;
    std::fs::write(dir.join(format!("{line}.lips")), lips_bytes(LIPS_FPS, &frames))?;
    let ms = (voicing.samples.len() as u64 * 1000 / voicing.rate.max(1) as u64) as u32;
    println!("[voice] line {line}: {ms} ms, {} words timed (heard \"{heard}\")", words.len());
    Ok((ms, said))
}

impl Game for Scene {
    fn state(&self, speaker: &str) -> Value {
        let id = game_id(speaker);
        let scene = self.scene.lock().unwrap();
        if let Some(known) = self.known.lock().unwrap().get(&id) {
            let mut state = known.clone();
            // (where they are and what's going on is as the game has it now, if they're still
            // the listener)
            if scene.listener == id {
                state["state"] = json!(scene.situation());
            }
            return state;
        }
        match scene.speaker(id) {
            Some(who) => json!({"name": who.name(), "player_name": scene.player(), "game": scene.game()}),
            None => Value::Null,
        }
    }

    fn act(&self, _speaker: &str, _action: &Value) {}

    fn heard(&self, speaker: &str, text: &str) {
        self.link.push_voice_line(&VoiceLine::new(VL_HEARD, game_id(speaker), 0, 0, "", text));
    }

    fn listening(&self) {
        self.note_listener();
        self.link.push_voice_line(&VoiceLine::new(VL_LISTEN, 0, 0, 0, "", ""));
    }

    fn sentence(&self, speaker: &str, text: &str, how: &str) {
        let level = self.state(speaker)["voice"].as_str().map(PathBuf::from).and_then(|clip| self.level_of(&clip));
        if self.streams(|| Piece::Sentence { speaker: game_id(speaker), text: text.to_string(), how: how.to_string(), level, volume: self.volume() }) {
            return;
        }
        *self.voicing.lock().unwrap() =
            Some(Voicing { speaker: speaker.to_string(), text: text.to_string(), how: how.to_string(), samples: Vec::new(), rate: 0, last: true, level, volume: self.volume() });
    }

    fn audio(&self, _speaker: &str, samples: &[f32], rate: u32) {
        if self.streams(|| Piece::Audio(samples.to_vec(), rate, Instant::now())) {
            return;
        }
        let mut voicing = self.voicing.lock().unwrap();
        let Some(v) = voicing.as_mut() else { return };
        v.rate = rate;
        v.samples.extend_from_slice(samples);
        let running_out = self.playing_until.lock().unwrap().saturating_duration_since(Instant::now()).as_secs_f32() < RUNNING_OUT;
        let clause = if self.fresh.load(Ordering::SeqCst) {
            FIRST_CLAUSE
        } else if running_out {
            HURRIED_CLAUSE
        } else {
            CLAUSE
        };
        let long = v.samples.len() as f32 > NO_PAUSE * rate as f32 && clause < CLAUSE;
        if let Some(at) = pause_in(&v.samples, rate, clause, PAUSE).or_else(|| long.then(|| pause_in(&v.samples, rate, clause, WORD_GAP)).flatten()) {
            self.fresh.store(false, Ordering::SeqCst);
            let rest = v.samples.split_off(at);
            let clause = Voicing { speaker: v.speaker.clone(), text: v.text.clone(), how: v.how.clone(), samples: std::mem::replace(&mut v.samples, rest), rate, last: false, level: v.level, volume: v.volume };
            let _ = self.jobs.send(Job::Sentence(clause, self.stops.load(Ordering::SeqCst)));
        }
    }

    fn sentence_end(&self, _speaker: &str) {
        if self.streams(|| Piece::SentenceEnd) {
            return;
        }
        if let Some(voicing) = self.voicing.lock().unwrap().take().filter(|v| !v.samples.is_empty()) {
            self.fresh.store(false, Ordering::SeqCst);
            let _ = self.jobs.send(Job::Sentence(voicing, self.stops.load(Ordering::SeqCst)));
        }
    }

    fn line_end(&self, speaker: &str) {
        if self.streams(|| Piece::LineEnd) {
            return;
        }
        self.fresh.store(true, Ordering::SeqCst);
        let _ = self.jobs.send(Job::End(speaker.to_string(), self.stops.load(Ordering::SeqCst)));
    }

    /// What came before goes: what's queued for the game, and the game's own playing. (Not the
    /// sentence being gathered: a new line's first sound stops the old line's, and that sentence
    /// is the new line's; one the player talked over is never ended, and the next replaces it.)
    fn stop_audio(&self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.fresh.store(true, Ordering::SeqCst);
        self.link.push_voice_line(&VoiceLine::new(VL_STOP, 0, 0, 0, "", ""));
    }

    /// Deaf while the game says so (a menu, the game paused or loading), or not ready yet.
    fn mic_muted(&self) -> bool {
        let flags = self.scene.lock().unwrap().flags;
        flags & VS_DEAF != 0 || flags & VS_READY == 0
    }

    fn addressed(&self) -> Option<String> {
        let listener = self.scene.lock().unwrap().listener;
        let id = if listener != 0 { listener } else { self.addressed.load(Ordering::Relaxed) };
        (id != 0).then(|| speaker_id(id))
    }

    fn shown_state(&self) -> Value {
        let listener = self.scene.lock().unwrap().listener;
        self.state(&speaker_id(listener))
    }
}

impl Scene {
    /// A piece for the streamed line, if lines are streamed (with the stops' count now).
    fn streams(&self, piece: impl FnOnce() -> Piece) -> bool {
        let Some(stream) = &self.stream else { return false };
        let _ = stream.lock().unwrap().send((piece(), self.stops.load(Ordering::SeqCst)));
        true
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume.store(volume.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    /// A voice's loudness while it speaks, from its clip (read once).
    fn level_of(&self, clip: &std::path::Path) -> Option<f32> {
        let mut levels = self.levels.lock().unwrap();
        *levels.entry(clip.to_path_buf()).or_insert_with(|| {
            let wav = std::fs::read(clip).ok()?;
            let rate = u32::from_le_bytes(wav.get(24..28)?.try_into().ok()?);
            let samples: Vec<f32> = wav.get(44..)?.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
            speech_level(&samples, rate)
        })
    }

    /// Remembers who was listening when the player started talking, in case the adapter has let
    /// them go by the time the words are known.
    pub fn note_listener(&self) {
        let listener = self.scene.lock().unwrap().listener;
        if listener != 0 {
            self.addressed.store(listener, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_sentence_is_cut_at_its_first_pause_after_a_clause() {
        let rate = 16000;
        let voice = |seconds: f32| (0..(seconds * rate as f32) as usize).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect::<Vec<f32>>();
        let quiet = |seconds: f32| vec![0.0f32; (seconds * rate as f32) as usize];
        // (a breath at 1 s is too soon; the pause at 2.5 s is the cut, in its middle)
        let mut samples = [voice(1.0), quiet(0.2), voice(1.3), quiet(0.3), voice(1.0)].concat();
        let at = pause_in(&samples, rate, CLAUSE, PAUSE).expect("a pause");
        assert!((at as f32 / rate as f32 - 2.56).abs() < 0.03, "{}", at as f32 / rate as f32);
        // (not yet a whole pause: no cut)
        samples.truncate((2.55 * rate as f32) as usize);
        assert_eq!(pause_in(&samples, rate, CLAUSE, PAUSE), None);
        // (a line's first clause goes at the first pause after 0.8 s: the breath at 1 s)
        assert!((pause_in(&samples, rate, FIRST_CLAUSE, PAUSE).unwrap() as f32 / rate as f32 - 1.06).abs() < 0.03);
        // (a quiet inside a word, a "p"'s closure, is no pause: only a gap between words, the
        // last resort)
        let word = [voice(1.0), quiet(0.05), voice(1.0)].concat();
        assert_eq!(pause_in(&word, rate, FIRST_CLAUSE, PAUSE), None);
        assert!(pause_in(&word, rate, FIRST_CLAUSE, WORD_GAP).is_some());
    }

    #[test]
    fn a_line_is_made_as_loud_as_the_games_own_voice() {
        let rate = 16000;
        let tone = |amp: f32| (0..rate as usize).map(|i| amp * (i as f32 * 0.05).sin()).collect::<Vec<f32>>();
        let game = speech_level(&tone(0.3), rate).unwrap();
        let ours = as_loud(&tone(0.1), rate, Some(game), 1.0);
        assert!((speech_level(&ours, rate).unwrap() / game - 1.0).abs() < 0.01);
        // (louder by the player's volume)
        let louder = as_loud(&tone(0.1), rate, Some(game), 1.5);
        assert!(speech_level(&louder, rate).unwrap() > game * 1.3);
        // (turned up far past full scale: under the ceiling, and turned down, not bent: away from
        // its start, every sample the tone's own times one gain)
        let far = as_loud(&tone(0.1), rate, Some(0.9), 3.0);
        assert!(far.iter().all(|s| s.abs() <= CEILING + 1e-5), "{}", far.iter().fold(0.0f32, |m, s| m.max(s.abs())));
        let gains: Vec<f32> = far.iter().zip(tone(0.1)).skip(rate as usize / 2).filter(|(_, x)| x.abs() > 0.02).map(|(y, x)| y / x).collect();
        let (low, high) = gains.iter().fold((f32::MAX, 0.0f32), |(l, h), &g| (l.min(g), h.max(g)));
        assert!(high / low < 1.01, "{low} {high}");
    }

    #[test]
    fn a_peak_is_turned_down_around_it_not_cut() {
        // (a quiet voice with one loud word: the word under the ceiling, the voice either side
        // as it was, a little way off)
        let rate = 16000;
        let voice: Vec<f32> = (0..rate as usize).map(|i| (if (7000..9000).contains(&i) { 2.0 } else { 0.2 }) * (i as f32 * 0.05).sin()).collect();
        let out = as_loud(&voice, rate, None, 1.0);
        assert!(out.iter().all(|s| s.abs() <= CEILING + 1e-5));
        assert!((out[4000] - voice[4000]).abs() < 1e-6 && (out[15000] - voice[15000]).abs() < 2e-3);
        let word = &out[7500..8500];
        assert!(word.iter().fold(0.0f32, |m, s| m.max(s.abs())) > 0.9 * CEILING);
    }

    #[test]
    fn a_clause_takes_its_own_words_of_the_sentence() {
        let all: Vec<&str> = "It's a far cry from the market, but a merchant can always appreciate a good show.".split_whitespace().collect();
        let heard = |words: &str| words.split_whitespace().map(|w| m8::listen::Word { text: w.into(), start: 0.0, end: 0.0 }).collect::<Vec<_>>();
        assert_eq!(clause_end(&all, 0, &heard("It's a far cry from the market,")), 7);
        // (heard one word short, ending on "show": the clause ends after "show.")
        assert_eq!(clause_end(&all, 7, &heard("but a merchant can always appreciate good show.")), all.len());
        // (nothing it matches: as many words as were heard)
        assert_eq!(clause_end(&all, 0, &heard("eats thrice")), 2);
        // (a dash isn't a word)
        let dashed: Vec<&str> = "Good day, friend — what will it be?".split_whitespace().collect();
        assert_eq!(clause_end(&dashed, 0, &heard("Good day friend what")), 5);
    }
}
