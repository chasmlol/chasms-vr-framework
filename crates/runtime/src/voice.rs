//! The voice plugin (`rustvr-voice`, beside the runtime's own exe): talking to the game's
//! characters out loud. It runs while the player has Voice on and a game is connected, as a
//! process of its own: it opens the mapping by name and talks to the adapter through the voice
//! regions (`rustvr_protocol::voice`), so speech recognition and the network stay out of the
//! runtime's frames. Its output goes to `voice.log` beside the runtime's log.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A plugin that stopped is started again no sooner than this.
const RETRY: Duration = Duration::from_secs(10);
/// The game gone this long, the plugin stops: a loading screen stops the game's frames (and so its
/// heartbeat) for seconds, and the plugin keeps what everyone's been told since the last save.
const GONE: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct VoicePlugin {
    child: Option<Child>,
    started: Option<Instant>,
    /// Since when the game hasn't been there, while the plugin runs.
    gone: Option<Instant>,
    /// Said once that it isn't there.
    missing: bool,
}

impl VoicePlugin {
    /// Runs the plugin while the player has Voice `on` and a game is connected (`game`): started
    /// when both are, stopped at once when Voice is turned off, and when the game has been gone a
    /// while (not for a loading screen).
    pub fn update(&mut self, on: bool, game: bool, mapping: &str, logs: &Path) {
        if let Some(child) = &mut self.child {
            if let Ok(Some(status)) = child.try_wait() {
                tracing::warn!("voice: the plugin stopped ({status}); see {}", logs.join("voice.log").display());
                self.child = None;
            }
        }
        self.gone = if game { None } else { self.gone.or(Some(Instant::now())) };
        let running = self.child.is_some();
        if !on || self.gone.is_some_and(|since| since.elapsed() > GONE) {
            if running {
                self.stop();
            }
        } else if game && !running && self.started.is_none_or(|at| at.elapsed() > RETRY) {
            self.started = Some(Instant::now());
            self.start(mapping, logs);
        }
    }

    fn exe() -> Option<PathBuf> {
        Some(std::env::current_exe().ok()?.with_file_name("rustvr-voice.exe"))
    }

    fn start(&mut self, mapping: &str, logs: &Path) {
        let Some(exe) = Self::exe().filter(|exe| exe.exists()) else {
            if !std::mem::replace(&mut self.missing, true) {
                tracing::warn!("voice: no rustvr-voice.exe beside the runtime (build it: -p rustvr-voice)");
            }
            return;
        };
        let log = std::fs::create_dir_all(logs).and_then(|_| std::fs::File::create(logs.join("voice.log")));
        let (out, err) = match log.and_then(|f| Ok((f.try_clone()?, f))) {
            Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
            Err(_) => (Stdio::null(), Stdio::null()),
        };
        let mut command = Command::new(&exe);
        command.args(["--mapping", mapping]).stdin(Stdio::null()).stdout(out).stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // (no console window of its own: the player is in the headset)
            command.creation_flags(0x0800_0000);
        }
        match command.spawn() {
            Ok(child) => {
                tracing::info!("voice: started the plugin (pid {})", child.id());
                self.child = Some(child);
            }
            Err(e) => tracing::warn!("voice: couldn't start {}: {e}", exe.display()),
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("voice: stopped the plugin");
        }
    }
}

impl Drop for VoicePlugin {
    fn drop(&mut self) {
        self.stop();
    }
}
