//! `rustvr`: the VR runtime.
//!
//! ```text
//! rustvr [--launch-range]                run with the headset (OpenXR), waiting for a game
//!                                         (or with the test range started alongside)
//! rustvr --sim <scenario|all> --launch-range
//!                                         headless test: simulated headset + the test range
//! rustvr --list                           list the scenarios
//! ```
//!
//! Options: `--mapping <name>` (shared-memory name), `--settings <dir>` (where settings.toml
//! lives; default `%LOCALAPPDATA%\RustVR`), `--captures <dir>` (scenario PNGs; default
//! `captures/` in the repository), `--size <w>x<h>` (simulated eye size), `--range <exe>`.

use anyhow::{Context, Result, bail};
use rustvr_runtime::scenario::{self, RunOptions};
use std::path::PathBuf;

struct Args {
    sim: Option<String>,
    list: bool,
    launch_range: bool,
    /// Scenarios: connect to a running game's adapter instead of starting the test range.
    attach: bool,
    mapping: Option<String>,
    settings: Option<PathBuf>,
    captures: Option<PathBuf>,
    size: (u32, u32),
    range: Option<PathBuf>,
}

fn parse() -> Result<Args> {
    let mut a = Args { sim: None, list: false, launch_range: false, attach: false, mapping: None, settings: None, captures: None, size: (640, 704), range: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--sim" => a.sim = Some(value()?),
            "--list" => a.list = true,
            "--launch-range" => a.launch_range = true,
            "--attach" => a.attach = true,
            "--mapping" => a.mapping = Some(value()?),
            "--settings" => a.settings = Some(value()?.into()),
            "--captures" => a.captures = Some(value()?.into()),
            "--range" => a.range = Some(value()?.into()),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').context("--size WxH")?;
                a.size = (w.parse()?, h.parse()?);
            }
            "-h" | "--help" => {
                println!("{}", include_str!("main.rs").lines().skip(2).take(12).map(|l| l.trim_start_matches("//!").trim_start_matches(' ')).collect::<Vec<_>>().join("\n"));
                std::process::exit(0);
            }
            other => bail!("unknown argument {other} (see --help)"),
        }
    }
    Ok(a)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("RUSTVR_LOG").unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let args = parse()?;
    if args.list {
        for s in scenario::all() {
            println!("{:<12} {}", s.name, s.about);
        }
        return Ok(());
    }
    if let Some(which) = &args.sim {
        let gpu = rustvr_gpu::Gpu::new()?;
        let root = repo_root().canonicalize().unwrap_or_else(|_| repo_root());
        let opts = RunOptions {
            captures: args.captures.clone().unwrap_or_else(|| root.join("captures")),
            launch_range: args.launch_range,
            attach: args.attach.then(|| args.mapping.clone().unwrap_or_else(|| rustvr_protocol::DEFAULT_MAPPING.to_owned())),
            range_exe: args.range.clone(),
            eye_size: args.size,
            warmup_timeout: std::time::Duration::from_secs(90),
        };
        let all = scenario::all();
        // `all` is the range's: the scenarios that play a real game (`metro_*`, `live`) run only by name.
        let real_game = |name: &str| name.starts_with("metro_") || name == "live";
        let chosen: Vec<_> = all.iter().filter(|s| s.name == which || (which == "all" && !real_game(s.name))).collect();
        if chosen.is_empty() {
            bail!("no scenario '{which}' (see --list)");
        }
        let mut failed = Vec::new();
        for s in chosen {
            match scenario::run(&gpu, s, &opts) {
                Ok(r) => {
                    if !scenario::print(&r, &root) {
                        failed.push(s.name);
                    }
                }
                Err(e) => {
                    println!("FAIL {}: {e:#}", s.name);
                    failed.push(s.name);
                }
            }
        }
        if !failed.is_empty() {
            println!("\n{} failed: {}", failed.len(), failed.join(", "));
            std::process::exit(1);
        }
        println!("\nall passed");
        return Ok(());
    }

    // The real thing: the headset through OpenXR, waiting for a game to connect.
    let range = if args.launch_range {
        Some(args.range.clone().unwrap_or(std::env::current_exe()?.with_file_name("rustvr-range.exe")))
    } else {
        None
    };
    rustvr_runtime::app::run_vr(rustvr_runtime::app::VrOptions {
        mapping: args.mapping.clone().unwrap_or_else(|| rustvr_protocol::DEFAULT_MAPPING.to_owned()),
        settings_dir: args.settings.clone().unwrap_or_else(rustvr_runtime::app::data_dir),
        range,
        range_args: Vec::new(),
        range_log: None,
        exit_with_game: None,
    })
}
