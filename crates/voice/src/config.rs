//! The plugin's config, `voice.toml` in the runtime's folder (`%LOCALAPPDATA%\RustVR`): the
//! player's own, outside every repository. It names the servers m8 calls (speech to text, the
//! cloned voice, the LLM, Kev) and, for a hosted LLM, the environment variable holding its key.
//! The first run writes one to fill in.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// What a first run writes: every setting, with what it does.
pub const DEFAULT: &str = r#"# The voice plugin: talk to the game's characters out loud, and they answer in their own voices
# (RustVR's Voice setting turns it on). Each model is a server it calls.

[llm]
# An OpenAI-style /v1/chat/completions: a hosted API (its model, and the environment variable
# holding its key: set it yourself, the key never goes in this file's repo or any other), or a
# local server (llama.cpp's).
url = "https://api.openai.com/v1/chat/completions"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"
temperature = 1.0
# The longest answer, in tokens.
max_tokens = 300
# Anything else the API takes in every request (DeepSeek: thinking = { type = "disabled" }).
[llm.extra]

[stt]
# Speech to text: an OpenAI-style /v1/audio/transcriptions with word times (m8's stt-server,
# Parakeet).
url = "http://127.0.0.1:8083/v1/audio/transcriptions"

[voice]
# The cloned voice: a server with Breeze TTS 2's API (m8's start-breeze.ps1), and its noise seed.
server = "http://127.0.0.1:7860"
seed = 42

[mic]
# (Which mic, talking over them, the pause that ends what you say, acting lines out and the
# voices' volume are RustVR's own settings: its menu's Voice tab.)
# Silero's sensitivity (0-1, higher needs clearer speech).
vad_threshold = 0.5

[models]
# Silero VAD (m8's models/silero_vad.onnx), which finds where speech starts and ends.
silero_vad = "silero_vad.onnx"

[kev]
# The decision model (llama.cpp's /v1/systemone): whether what happens in the game makes a
# character speak up. Off, every line you say to someone is answered, and nothing else starts one.
enabled = false
url = "http://127.0.0.1:8080/v1/systemone"
"#;

#[derive(Deserialize)]
pub struct VoiceConfig {
    pub llm: m8::config::LlmConfig,
    pub stt: m8::config::SttConfig,
    pub voice: Speech,
    #[serde(default = "mic")]
    pub mic: m8::config::MicConfig,
    pub models: m8::config::ModelsConfig,
    #[serde(default)]
    pub kev: m8::config::KevConfig,
}

#[derive(Deserialize)]
pub struct Speech {
    pub server: String,
    #[serde(default = "seed")]
    pub seed: u64,
    /// Say each line as its note says ("[sneering]"): Breeze's guidance, about twice the work,
    /// on a GPU the game's rendering already has busy. Off, lines are said plainly (the note still
    /// makes the character's face), and come twice as fast.
    #[serde(default = "yes")]
    pub instruct: bool,
}

/// RustVR's own Voice settings, as its menu sets them (its `settings.toml`'s `[voice]`): the mic,
/// how it listens, whether lines are acted out, the voices' volume. The same defaults as RustVR's.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Rustvr {
    pub mic: String,
    pub volume: f32,
    pub act_out: bool,
    pub barge_in: bool,
    pub pause: f32,
}

impl Default for Rustvr {
    fn default() -> Self {
        Rustvr { mic: String::new(), volume: 1.5, act_out: true, barge_in: false, pause: 0.3 }
    }
}

impl Rustvr {
    /// RustVR's settings file's (the defaults where it hasn't any).
    pub fn load(settings: &Path) -> Rustvr {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct File {
            voice: Rustvr,
        }
        std::fs::read_to_string(settings).ok().and_then(|text| toml::from_str::<File>(&text).ok()).map(|f| f.voice).unwrap_or_default()
    }
}

fn mic() -> m8::config::MicConfig {
    toml::from_str("").expect("every mic setting has a default")
}

fn seed() -> u64 {
    42
}

fn yes() -> bool {
    true
}

impl VoiceConfig {
    /// The config at `path`; a missing one is written with the defaults first.
    pub fn load(path: &Path) -> Result<VoiceConfig> {
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, DEFAULT).with_context(|| format!("writing {}", path.display()))?;
            println!("[voice] wrote {}: fill in your LLM and servers", path.display());
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: VoiceConfig = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        if cfg.models.silero_vad.is_relative() {
            cfg.models.silero_vad = base.join(&cfg.models.silero_vad);
        }
        Ok(cfg)
    }

    /// m8's config for one game: its memory and logs in `memory`, the mic as RustVR's settings
    /// have it.
    pub fn m8(&self, memory: PathBuf, rustvr: &Rustvr) -> m8::config::Config {
        let mut mic = self.mic.clone();
        mic.device = Some(if rustvr.mic.is_empty() { "Virtual Desktop".to_string() } else { rustvr.mic.clone() });
        mic.barge_in = rustvr.barge_in;
        mic.mute_while_speaking = !rustvr.barge_in;
        mic.end_of_speech_silence = rustvr.pause;
        m8::config::Config {
            mate: m8::config::MateConfig {
                name: "someone".to_string(),
                persona: Some(memory.clone()),
                voice_server: self.voice.server.clone(),
                voice_reference: None,
                voice_seed: self.voice.seed,
                voice_instruct: rustvr.act_out,
            },
            mic,
            stt: self.stt.clone(),
            kev: self.kev.clone(),
            llm: self.llm.clone(),
            models: self.models.clone(),
            game: Default::default(),
            base_dir: memory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_written_default_reads_back() {
        let cfg: VoiceConfig = toml::from_str(DEFAULT).unwrap();
        assert!(!cfg.kev.enabled, "Kev is off unless the player turns him on");
        assert_eq!(cfg.llm.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert!(cfg.llm.api_key.is_none() && cfg.llm.extra.is_empty());
    }

    #[test]
    fn rustvrs_own_voice_settings_are_read() {
        let dir = std::env::temp_dir().join(format!("rustvr-voice-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.toml");
        std::fs::write(&file, "[comfort]\nseated = true\n\n[voice]\nenabled = true\nmic = \"USB Mic\"\nvolume = 2.0\n").unwrap();
        let rv = Rustvr::load(&file);
        assert_eq!((rv.mic.as_str(), rv.volume, rv.act_out, rv.pause), ("USB Mic", 2.0, true, 0.3));
        let cfg: VoiceConfig = toml::from_str(DEFAULT).unwrap();
        let m8 = cfg.m8(dir.clone(), &rv);
        assert_eq!(m8.mic.device.as_deref(), Some("USB Mic"));
        assert!(m8.mate.voice_instruct && !m8.mic.barge_in && m8.mic.mute_while_speaking);
        assert_eq!(Rustvr::load(&dir.join("none.toml")), Rustvr::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deepseek_takes_its_extras() {
        let text = DEFAULT.replace("[llm.extra]\n", "[llm.extra]\nthinking = { type = \"disabled\" }\n");
        let cfg: VoiceConfig = toml::from_str(&text).unwrap();
        assert_eq!(cfg.llm.extra["thinking"]["type"], "disabled");
    }
}
