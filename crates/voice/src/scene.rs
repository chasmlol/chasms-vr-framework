//! The game, for m8: what the adapter says in the voice regions (who's around, who's listening,
//! who they are), and m8's lines going back to it. Each sentence's voice is gathered whole, the
//! mouth's shapes are made for it (when each word is said, from speech to text on the voice
//! itself), and its files written where the game plays them from, before the adapter is told.

use crate::clips;
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

/// The mouth's frames a second (Oblivion's lip files are at 30).
const LIPS_FPS: u32 = 30;
/// A sentence's files are kept this many lines, then deleted (the game has long played them).
const LINES_KEPT: u32 = 32;

/// m8's id for a speaker: the game's, as hex.
pub fn speaker_id(id: u32) -> String {
    format!("{id:08X}")
}

fn game_id(speaker: &str) -> u32 {
    u32::from_str_radix(speaker, 16).unwrap_or(0)
}

/// A sentence being voiced: its words, how they're said, and the voice so far.
struct Voicing {
    speaker: String,
    text: String,
    how: String,
    samples: Vec<f32>,
    rate: u32,
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
}

impl Scene {
    pub fn new(link: Arc<Link>, first: VoiceScene, template: m8::site::PromptFile, transcriber: Transcriber) -> Arc<Scene> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let stops = Arc::new(AtomicU64::new(0));
        {
            let (link, stops) = (link.clone(), stops.clone());
            let lines_dir = PathBuf::from(first.lines_dir());
            std::thread::spawn(move || {
                // (numbered on from the clock, so no run's files share a name with an earlier run's:
                // a game may keep a sound it played by its file's name)
                let mut line = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 1_000_000) as u32 * 1000;
                let first = line;
                for job in queue {
                    match job {
                        Job::Sentence(voicing, epoch) => {
                            if epoch != stops.load(Ordering::SeqCst) {
                                continue;
                            }
                            line += 1;
                            match write_line(&lines_dir, line, &voicing, &transcriber) {
                                Ok(ms) if epoch == stops.load(Ordering::SeqCst) => {
                                    link.push_voice_line(&VoiceLine::new(VL_SAY, game_id(&voicing.speaker), line, ms, &voicing.how, &voicing.text));
                                }
                                Ok(_) => {}
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

/// Writes a sentence's voice and mouth: `<line>.wav` and `<line>.lips`. Its length, in ms.
fn write_line(dir: &std::path::Path, line: u32, voicing: &Voicing, transcriber: &Transcriber) -> anyhow::Result<u32> {
    let (heard, words) = transcriber.words(&voicing.samples, voicing.rate).unwrap_or_else(|e| {
        eprintln!("[voice] no word times for line {line}: {e:#}");
        (String::new(), Vec::new())
    });
    let frames = m8::lips::frames(&voicing.samples, voicing.rate, &words, &voicing.text, LIPS_FPS as f32);
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{line}.wav")), clips::wav_bytes(&voicing.samples, voicing.rate))?;
    std::fs::write(dir.join(format!("{line}.lips")), lips_bytes(LIPS_FPS, &frames))?;
    let ms = (voicing.samples.len() as u64 * 1000 / voicing.rate.max(1) as u64) as u32;
    println!("[voice] line {line}: {ms} ms, {} words timed (heard \"{heard}\")", words.len());
    Ok(ms)
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
        *self.voicing.lock().unwrap() =
            Some(Voicing { speaker: speaker.to_string(), text: text.to_string(), how: how.to_string(), samples: Vec::new(), rate: 0 });
    }

    fn audio(&self, _speaker: &str, samples: &[f32], rate: u32) {
        if let Some(voicing) = self.voicing.lock().unwrap().as_mut() {
            voicing.rate = rate;
            voicing.samples.extend_from_slice(samples);
        }
    }

    fn sentence_end(&self, _speaker: &str) {
        if let Some(voicing) = self.voicing.lock().unwrap().take().filter(|v| !v.samples.is_empty()) {
            let _ = self.jobs.send(Job::Sentence(voicing, self.stops.load(Ordering::SeqCst)));
        }
    }

    fn line_end(&self, speaker: &str) {
        let _ = self.jobs.send(Job::End(speaker.to_string(), self.stops.load(Ordering::SeqCst)));
    }

    fn stop_audio(&self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        *self.voicing.lock().unwrap() = None;
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
    /// Remembers who was listening when the player started talking, in case the adapter has let
    /// them go by the time the words are known.
    pub fn note_listener(&self) {
        let listener = self.scene.lock().unwrap().listener;
        if listener != 0 {
            self.addressed.store(listener, Ordering::Relaxed);
        }
    }
}
