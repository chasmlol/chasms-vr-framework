//! `rustvr-play`: what the "RustVR" Steam shortcut launches. Starts the runtime on the headset
//! (the current OpenXR runtime: Virtual Desktop, SteamVR...) with the test range connected, and no
//! console window. Logs go to `%LOCALAPPDATA%\RustVR\rustvr.log` and `range.log`; if it can't
//! start, a message box says why.
//!
//! Options: `--no-range` (just the runtime, waiting for a game adapter), `--exit-with-game` (quit
//! a few seconds after the game that connected has gone: what game adapters start it with), and
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
