//! `rustvr-play`: what the "RustVR" Steam shortcut launches. Starts the runtime on the headset
//! (the current OpenXR runtime: Virtual Desktop, SteamVR...) with the test range connected, and no
//! console window. Logs go to `%LOCALAPPDATA%\RustVR\rustvr.log` and `range.log`; if it can't
//! start, a message box says why.
//!
//! Options: `--no-range` (just the runtime, waiting for a game adapter), `--exit-with-game` (quit
//! once the game that connected has exited: what game adapters start it with; `--game-pid <pid>`
//! names that game before it connects, and then a missing headset is waited for quietly, with no
//! message box, until the game exits), `--wait` (the same wait for a headset with no game: started
//! ahead of time, the range comes up whenever the headset does, and again if it drops out), and
//! anything after `--` is passed to the range (e.g. `-- --weapon vsv`).

#![windows_subsystem = "windows"]

use rustvr_runtime::app::{VrOptions, data_dir, run_vr};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::HSTRING;

fn main() {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let log_path = dir.join("rustvr.log");
    if let Ok(file) = std::fs::File::create(&log_path) {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("RUSTVR_LOG").unwrap_or_else(|_| "info".into()))
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let no_range = args[..split].iter().any(|a| a == "--no-range");
    let exit_with_game = args[..split].iter().any(|a| a == "--exit-with-game");
    let game_pid: Option<u32> = args[..split].iter().position(|a| a == "--game-pid").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok());
    let wait = args[..split].iter().any(|a| a == "--wait");
    let range_args = args.get(split + 1..).map(|a| a.to_vec()).unwrap_or_default();
    let range = std::env::current_exe().ok().map(|p| p.with_file_name("rustvr-range.exe"));
    let opts = VrOptions {
        mapping: rustvr_protocol::DEFAULT_MAPPING.to_owned(),
        settings_dir: dir.clone(),
        range: if no_range { None } else { range },
        range_args,
        range_log: Some(dir.join("range.log")),
        exit_with_game: exit_with_game.then(|| std::time::Duration::from_secs(5)),
    };
    tracing::info!("rustvr-play: starting");
    // Started by a game, or ahead of time: nobody may be at the desktop, so wait for the headset
    // instead of saying it's missing (for as long as the game runs).
    if game_pid.is_some() || wait {
        let mut said = false;
        loop {
            match run_vr(opts.clone()) {
                Ok(()) => break,
                Err(e) => {
                    if let Some(pid) = game_pid
                        && !rustvr_runtime::app::process_alive(pid)
                    {
                        tracing::info!("rustvr-play: the game (pid {pid}) has exited");
                        break;
                    }
                    if !said {
                        let until = if game_pid.is_some() { " while the game runs" } else { "" };
                        tracing::warn!("rustvr-play: {e:#}; trying again every few seconds{until}");
                        said = true;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
            }
        }
        tracing::info!("rustvr-play: finished");
        return;
    }
    if let Err(e) = run_vr(opts) {
        tracing::error!("rustvr-play: {e:#}");
        let text = format!("RustVR couldn't start:\n\n{e:#}\n\nLog: {}", log_path.display());
        unsafe {
            MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("RustVR"), MB_OK | MB_ICONERROR);
        }
        std::process::exit(1);
    }
    tracing::info!("rustvr-play: finished");
}
