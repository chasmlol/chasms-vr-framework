//! `rustvr-xrsim`: drives the emulated headset.
//!
//! ```text
//! rustvr-xrsim manifest                      write rustvr_xrsim.json next to the DLL, print its path
//! rustvr-xrsim run [--dir D] [--script S] -- <program> [args...]
//!                                            run a program on the emulated headset
//! rustvr-xrsim ctl [--dir D] <toml line>...  live control: e.g. 'right = { menu = true }'
//! rustvr-xrsim capture [--dir D] <name>      save what the eyes see now; waits for the picture
//! rustvr-xrsim status [--dir D]              the headset's state (session, fps, layers)
//! ```

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

fn default_dir() -> PathBuf {
    std::env::var_os("RUSTVR_XRSIM_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("rustvr-xrsim"))
}

fn write_manifest() -> Result<PathBuf> {
    let dll = exe_dir().join("rustvr_xrsim.dll");
    if !dll.exists() {
        bail!("{} isn't built (cargo build -p rustvr-xrsim)", dll.display());
    }
    let path = exe_dir().join("rustvr_xrsim.json");
    std::fs::write(&path, rustvr_xrsim::manifest(&dll))?;
    Ok(path)
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        bail!("usage: rustvr-xrsim manifest | run [--dir D] [--script S] -- <program> [args] | ctl [--dir D] <toml line>... | capture [--dir D] <name> | status [--dir D]");
    }
    let cmd = args.remove(0);
    let mut dir = default_dir();
    let mut script = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                dir = PathBuf::from(args.get(i + 1).context("--dir needs a folder")?);
                i += 2;
            }
            "--script" => {
                script = Some(PathBuf::from(args.get(i + 1).context("--script needs a file")?));
                i += 2;
            }
            "--" => {
                rest.extend(args[i + 1..].iter().cloned());
                break;
            }
            _ => {
                rest.push(args[i].clone());
                i += 1;
            }
        }
    }
    std::fs::create_dir_all(&dir)?;
    match cmd.as_str() {
        "manifest" => println!("{}", write_manifest()?.display()),
        "run" => {
            let manifest = write_manifest()?;
            let (prog, prog_args) = rest.split_first().context("run needs a program")?;
            // A fresh session folder: no old control file or captures to confuse anyone.
            let _ = std::fs::remove_file(dir.join("control.toml"));
            let mut c = std::process::Command::new(prog);
            c.args(prog_args).env("XR_RUNTIME_JSON", &manifest).env("RUSTVR_XRSIM_DIR", &dir);
            if let Some(s) = &script {
                c.env("RUSTVR_XRSIM_SCRIPT", std::fs::canonicalize(s)?);
            }
            let status = c.status().with_context(|| format!("running {prog}"))?;
            std::process::exit(status.code().unwrap_or(1));
        }
        "ctl" => {
            std::fs::write(dir.join("control.toml"), rest.join("\n") + "\n")?;
        }
        "capture" => {
            let name = rest.first().context("capture needs a name")?;
            let png = dir.join("captures").join(format!("{name}.png"));
            let _ = std::fs::remove_file(&png);
            std::fs::write(dir.join("control.toml"), format!("capture = \"{name}\"\n"))?;
            let t0 = std::time::Instant::now();
            while !png.exists() {
                if t0.elapsed().as_secs() > 15 {
                    bail!("no capture after 15 s: is an app running on the emulated headset and submitting frames? (see {})", dir.join("xrsim.log").display());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            println!("{}", png.display());
        }
        "status" => print!("{}", std::fs::read_to_string(dir.join("status.toml")).unwrap_or_else(|_| "no status yet\n".into())),
        other => bail!("unknown command {other}"),
    }
    Ok(())
}
