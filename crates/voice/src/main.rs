//! `rustvr-voice`: the voice plugin. The runtime starts it while the player has Voice on and a
//! game is connected; it opens the runtime's mapping by name and runs the user's m8 against the
//! voice regions (`rustvr_protocol::voice`): the headset's mic, speech to text, the LLM, each
//! character's cloned voice and memory. The game says who's around and who they are; m8 answers
//! with their lines, and the game says them.
//!
//! Usage: `rustvr-voice [--mapping <name>] [--config <voice.toml>]`. Its config (the servers, the
//! LLM, the key's environment variable) is the player's: `%LOCALAPPDATA%\RustVR\voice.toml`,
//! written with the defaults on the first run. The brain page, http://127.0.0.1:8090, shows every
//! prompt and line, and edits the characters' prompt.

mod clips;
mod config;
mod scene;

use anyhow::Result;
use bytemuck::Zeroable;
use m8::brain::{Brain, Input};
use m8::game::Game;
use rustvr_protocol::link::Link;
use rustvr_protocol::*;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often it beats, and looks at the scene and the game's events.
const POLL: Duration = Duration::from_millis(50);
/// The runtime gone this long, the plugin goes too (the runtime starts it again).
const ORPHANED: Duration = Duration::from_secs(5);

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("RustVR")
}

/// A game's name as a folder's.
fn folder(game: &str) -> String {
    let name: String = game.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let name = name.split('-').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-");
    if name.is_empty() { "game".to_string() } else { name }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mapping = arg("--mapping").unwrap_or_else(|| DEFAULT_MAPPING.to_string());
    let config_path = arg("--config").map(PathBuf::from).unwrap_or_else(|| data_dir().join("voice.toml"));
    let cfg = config::VoiceConfig::load(&config_path)?;

    // The runtime made the mapping; the game's adapter writes its scene once it's in the world.
    let link = Arc::new(loop {
        match Link::open(&mapping) {
            Ok(link) => break link,
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    });
    println!("[voice] on {mapping}, waiting for the game");
    let mut first = VoiceScene::zeroed();
    while !link.read_voice_scene_into(&mut first) {
        link.heartbeat_voice();
        if !link.runtime_alive() {
            println!("[voice] the runtime is gone");
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
    let memory = data_dir().join("voice").join(folder(first.game()));
    std::fs::create_dir_all(&memory)?;
    println!("[voice] {} (memory in {})", first.game(), memory.display());
    // RustVR's own Voice settings (its menu): the mic and how it listens as this starts (RustVR
    // starts it again when they change), the voices' volume as it changes.
    let settings = data_dir().join("settings.toml");
    let rustvr = config::Rustvr::load(&settings);
    println!("[voice] RustVR's settings: {rustvr:?}");
    let m8cfg = cfg.m8(memory.clone(), &rustvr);

    // The characters' prompt: the framework's, kept in the memory folder so the brain page can
    // edit it (and the edit lasts).
    let prompt_path = memory.join("npc_prompt.txt");
    if !prompt_path.exists() {
        std::fs::write(&prompt_path, include_str!("npc_prompt.txt"))?;
    }
    let template = m8::site::PromptFile::load(prompt_path)?;
    let none = |name: &str| m8::site::PromptFile::load(memory.join(name));
    let (combat, radio) = (none("combat_prompt.txt")?, none("radio_prompt.txt")?);

    let transcriber = m8::listen::Transcriber::new(&m8cfg);
    let scene = scene::Scene::new(link.clone(), first, template.clone(), transcriber.clone());
    scene.update(first);
    scene.set_volume(rustvr.volume);
    let game: Arc<dyn Game> = scene.clone();

    // (Breeze takes minutes on its first start: the plugin waits for it, beating meanwhile)
    let voice = loop {
        match m8::speak::Voice::connect(&cfg.voice.server, cfg.voice.seed) {
            Ok(voice) => break voice.instructed(rustvr.act_out),
            Err(e) => {
                println!("[voice] {e:#}; trying again");
                let until = Instant::now() + Duration::from_secs(3);
                while Instant::now() < until {
                    link.heartbeat_voice();
                    std::thread::sleep(POLL);
                }
            }
        }
    };

    let (inputs_tx, inputs_rx) = mpsc::channel::<Input>();
    let trace = m8::site::Trace::new(&memory.join("logs"));
    {
        let (trace, template, game, inputs) = (trace.clone(), template.clone(), game.clone(), inputs_tx.clone());
        let (combat, radio) = (combat.clone(), radio.clone());
        std::thread::spawn(move || {
            if let Err(e) = m8::site::run(trace, template, combat, radio, game, inputs) {
                eprintln!("[site] stopped: {e:#}");
            }
        });
    }
    println!("[voice] brain page: http://127.0.0.1:{}", m8::site::PORT);

    let speaking = m8::Speaking::new();
    let (speech_tx, speech_rx) = mpsc::channel();
    {
        let (game, speaking, trace) = (game.clone(), speaking.clone(), trace.clone());
        std::thread::spawn(move || {
            if let Err(e) = m8::speak::run(voice, game, speaking, trace, speech_rx) {
                eprintln!("[tts] stopped: {e:#}");
            }
        });
    }
    {
        let (cfg, heard, game, speaking) = (m8cfg.clone(), inputs_tx.clone(), game.clone(), speaking.clone());
        std::thread::spawn(move || {
            let (tx, rx) = mpsc::channel::<String>();
            let addressed = game.clone();
            std::thread::spawn(move || {
                // (said to whoever was addressed as the player started; the adapter picked them)
                for text in rx {
                    let _ = heard.send(Input::Heard { text, to: addressed.addressed() });
                }
            });
            if let Err(e) = m8::listen::run(cfg, transcriber, speaking, game, tx) {
                eprintln!("[mic] stopped: {e:#}");
            }
        });
    }

    // The scene and the game's events, the heartbeat; and the plugin's end with the runtime's.
    {
        let (link, scene, inputs) = (link.clone(), scene.clone(), inputs_tx.clone());
        std::thread::spawn(move || {
            let (mut seq, mut read) = (link.voice_scene_seq(), 0u64);
            let mut latest = VoiceScene::zeroed();
            let mut runtime_seen = Instant::now();
            let mut volume_read = Instant::now();
            loop {
                link.heartbeat_voice();
                let now = link.voice_scene_seq();
                if now != seq && link.read_voice_scene_into(&mut latest) {
                    seq = now;
                    scene.update(latest);
                }
                for event in link.read_voice_events(&mut read) {
                    let about = (event.speaker != 0).then(|| scene::speaker_id(event.speaker));
                    let input = match event.kind {
                        VE_SAVED => Input::Saved(event.text().to_string()),
                        VE_LOADED => Input::Loaded(event.text().to_string()),
                        _ => Input::Event { text: event.text().to_string(), react: event.flags & VEF_REACT != 0, about },
                    };
                    let _ = inputs.send(input);
                }
                if volume_read.elapsed() > Duration::from_secs(1) {
                    volume_read = Instant::now();
                    scene.set_volume(config::Rustvr::load(&settings).volume);
                }
                if link.runtime_alive() {
                    runtime_seen = Instant::now();
                } else if runtime_seen.elapsed() > ORPHANED {
                    println!("[voice] the runtime is gone: stopping");
                    std::process::exit(0);
                }
                std::thread::sleep(POLL);
            }
        });
    }

    println!("[voice] ready: talk to whoever you look at");
    Brain::new(m8cfg, game, speech_tx, speaking, trace, template, combat, radio, inputs_tx).run(inputs_rx);
    Ok(())
}
