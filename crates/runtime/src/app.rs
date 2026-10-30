//! Running the runtime for real: the headset through OpenXR, optionally with the test range
//! started alongside (what the "RustVR" Steam shortcut launches).

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::backend::openxr::OpenXrBackend;
use crate::session::{Options, Session};

#[derive(Clone)]
pub struct VrOptions {
    pub mapping: String,
    pub settings_dir: PathBuf,
    /// Start this test-range executable once the headset is up (killed again on exit).
    pub range: Option<PathBuf>,
    pub range_args: Vec<String>,
    /// Where the range's own log goes.
    pub range_log: Option<PathBuf>,
    /// Quit once the game that connected has exited (its process, not just its heartbeat: games go
    /// quiet while loading). A game that launches the runtime gets it out of the way when it closes
    /// (Steam counts it as part of the game). The duration is unused (kept for the option's shape).
    pub exit_with_game: Option<std::time::Duration>,
}

/// `%LOCALAPPDATA%\RustVR`: settings, profiles and logs. `RUSTVR_DIR` puts them elsewhere, so a
/// test run (an agent's, say) keeps its own settings and logs and leaves the player's alone.
pub fn data_dir() -> PathBuf {
    match std::env::var_os("RUSTVR_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("RustVR"),
    }
}

/// Runs until the headset session ends (closed from the VR dashboard, or the VR runtime quits).
pub fn run_vr(o: VrOptions) -> Result<()> {
    let (backend, gpu) = OpenXrBackend::new("RustVR")?;
    let mut session = Session::new(
        gpu,
        Box::new(backend),
        Options { mapping: o.mapping.clone(), settings_dir: Some(o.settings_dir.clone()), lockstep: false, lockstep_timeout_ms: 0, audio: true, offline_audio: false },
    )?;
    let _range = match &o.range {
        Some(exe) => Some(spawn_range(exe, &o.range_args, &o.mapping, o.range_log.as_deref(), false)?),
        None => None,
    };
    // The game that connected, and whether it has exited (not just gone quiet: a game loading a
    // level stops its heartbeat for a while).
    let mut game_pid = 0u32;
    // The same from a thread of its own, for when the loop can't get round to it: a headset
    // runtime may hold the frame wait while the headset is off.
    let watched = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    if o.exit_with_game.is_some() {
        let watched = watched.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let pid = watched.load(std::sync::atomic::Ordering::Relaxed);
                if pid != 0 && !process_alive(pid) {
                    // (The loop gets its chance first.)
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    tracing::info!("runtime: the game (pid {pid}) has exited and the headset isn't drawing: exiting");
                    std::process::exit(0);
                }
            }
        });
    }
    while session.step()? {
        if o.exit_with_game.is_none() {
            continue;
        }
        let pid = session.link.adapter_pid();
        if session.adapter_connected() && pid != 0 {
            game_pid = pid;
            watched.store(pid, std::sync::atomic::Ordering::Relaxed);
        } else if game_pid != 0 && !process_alive(game_pid) {
            tracing::info!("runtime: the game (pid {game_pid}) has exited: exiting");
            break;
        }
    }
    Ok(())
}

/// Whether a process is still running.
pub fn process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == STILL_ACTIVE.0 as u32
    }
}

/// Starts the test range with no window, connected to `mapping`.
pub fn spawn_range(exe: &std::path::Path, args: &[String], mapping: &str, log: Option<&std::path::Path>, low_priority: bool) -> Result<KillOnDrop> {
    anyhow::ensure!(exe.exists(), "{} not found: build the workspace first", exe.display());
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).env("RUSTVR_LINK", mapping).stdout(std::process::Stdio::null());
    match log {
        Some(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            cmd.stderr(std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?);
        }
        None => {
            cmd.stderr(std::process::Stdio::null());
        }
    }
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(CREATE_NO_WINDOW | if low_priority { BELOW_NORMAL_PRIORITY_CLASS } else { 0 });
    }
    let child = cmd.spawn().with_context(|| format!("starting {}", exe.display()))?;
    tracing::info!("runtime: started the test range (pid {})", child.id());
    Ok(KillOnDrop(child))
}

/// A child process that's stopped when this is dropped.
pub struct KillOnDrop(pub std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
