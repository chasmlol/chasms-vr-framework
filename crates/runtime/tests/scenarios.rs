//! Runs every headless scenario (simulated headset + the test range) through the real binaries.
//! Needs the range built next to `rustvr.exe`: `cargo test --workspace` does that.

use std::path::Path;
use std::process::Command;

#[test]
fn every_scenario_passes() {
    let exe = env!("CARGO_BIN_EXE_rustvr");
    let range = Path::new(exe).with_file_name("rustvr-range.exe");
    assert!(range.exists(), "{} isn't built: run `cargo test --workspace` (or build rustvr-range first)", range.display());
    let mut cmd = Command::new(exe);
    cmd.args(["--sim", "all", "--launch-range"]).env("RUSTVR_LOG", "warn");
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().expect("running rustvr");
    let stdout = String::from_utf8_lossy(&out.stdout);
    println!("{stdout}");
    assert!(out.status.success(), "scenarios failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
}
