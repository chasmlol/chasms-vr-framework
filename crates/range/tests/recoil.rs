//! The Metro guns' own recoil, end to end: the kicks the extractor translated from the game
//! (`local_assets/metro2033/recoil/`), taken into weapon space by the range (`physical::kick_descs`)
//! and played by the runtime's player (`rig::kick::GameRecoil`), next to the runtime's own recoil.
//!
//! Fires each rigged Metro gun (a single shot from the hip and aimed, a three-round burst, two
//! seconds of full auto), checks the kick, and writes the muzzle's path to `captures/recoil/`
//! (`<gun>.csv` and `<gun>.svg`: pitch in degrees and push back in millimetres over time).
//! Skips when the assets haven't been extracted.

#![allow(dead_code, clippy::too_many_arguments)]

#[path = "../src/content.rs"]
mod content;
#[path = "../src/physical.rs"]
mod physical;
#[path = "../src/render.rs"]
mod render;

use glam::{Quat, Vec3};
use rustvr_protocol::*;
use rustvr_runtime::rig::kick::{GameRecoil, HoldScale, Stance};
use rustvr_runtime::rig::recoil::{Hold, Recoil};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const DT: f32 = 1.0 / 90.0;

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("captures").join("recoil")
}

/// The muzzle's pitch (deg, up +), yaw (deg) and push back along the barrel (mm, back +).
fn muzzle(r: Quat, t: Vec3, muzzle: Vec3) -> (f32, f32, f32) {
    let f = r * Vec3::Z;
    let moved = r * muzzle + t - muzzle;
    (f.y.atan2(f.z).to_degrees(), f.x.atan2(f.z).to_degrees(), -moved.z * 1000.0)
}

struct Run {
    name: &'static str,
    stance: Stance,
    /// How hard the hold makes it kick (the game's own: both hands, at the shoulder).
    hold: HoldScale,
    shots: u32,
    seconds: f32,
}

/// Fires `run` at `rate`, both recoils; per frame (time, game (pitch, yaw, back), own (pitch, yaw, back)).
fn fire(kicks: &[KickDesc], d: &ObjectDesc, run: &Run, rate: f32) -> Vec<(f32, (f32, f32, f32), (f32, f32, f32))> {
    let mut game = GameRecoil::new(11);
    let mut own = Recoil::new(11);
    let hold = if run.stance.aimed { Hold::SHOULDERED } else { Hold::TWO_HANDS };
    let m = Vec3::from_array(d.muzzle);
    let grip = Vec3::from_array(d.grip);
    let (mut t, mut next, mut fired, mut out) = (0.0f32, 0.0f32, 0, Vec::new());
    while t < run.seconds {
        if fired < run.shots && t >= next {
            game.kick(kicks, run.stance, run.hold);
            own.kick(d, hold);
            fired += 1;
            next += 1.0 / rate;
        }
        game.update(DT, kicks);
        own.update(DT);
        let (gr, gt) = game.transform(kicks, 1.0, 1.0);
        let (or, ot) = own.transform(grip);
        out.push((t, muzzle(gr, gt, m), muzzle(or, ot, m)));
        t += DT;
    }
    out
}

fn peak(v: &[(f32, (f32, f32, f32), (f32, f32, f32))], f: impl Fn(&(f32, (f32, f32, f32), (f32, f32, f32))) -> f32) -> f32 {
    v.iter().map(f).fold(0.0, |a: f32, b| if b.abs() > a.abs() { b } else { a })
}

/// A small chart: the game's pitch (solid) and push back (dashed) for each run, the runtime's own
/// pitch in grey.
fn svg(gun: &str, runs: &[(&Run, Vec<(f32, (f32, f32, f32), (f32, f32, f32))>)]) -> String {
    let (w, h, pad) = (900.0, 280.0, 40.0);
    let mut s = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{}\" font-family=\"sans-serif\" font-size=\"12\">\n", h * runs.len() as f32);
    s += "<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n";
    for (i, (run, data)) in runs.iter().enumerate() {
        let y0 = h * i as f32;
        let tmax = run.seconds;
        let pmax = data.iter().map(|d| d.1.0.abs().max(d.2.0.abs())).fold(1.0f32, f32::max).ceil();
        let bmax = data.iter().map(|d| d.1.2.abs()).fold(10.0f32, f32::max);
        let x = |t: f32| pad + (w - 2.0 * pad) * t / tmax;
        let yp = |p: f32| y0 + h - pad - (h - 2.0 * pad) * (p / pmax).max(-0.2);
        let yb = |b: f32| y0 + h - pad - (h - 2.0 * pad) * (b / bmax).max(-0.2);
        let _ = writeln!(s, "<text x=\"{pad}\" y=\"{}\" font-weight=\"bold\">{gun}: {} ({} shots)</text>", y0 + 18.0, run.name, run.shots);
        let _ = writeln!(s, "<line x1=\"{pad}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#999\"/>", yp(0.0), w - pad, yp(0.0));
        let _ = writeln!(s, "<text x=\"{pad}\" y=\"{}\" fill=\"#c33\">red: the game's muzzle climb (top = {pmax} deg)</text><text x=\"{}\" y=\"{}\" fill=\"#36c\">blue dashed: push back (top = {bmax:.0} mm)</text><text x=\"{}\" y=\"{}\" fill=\"#888\">grey: RustVR's own climb</text>", y0 + 36.0, pad + 330.0, y0 + 36.0, pad + 600.0, y0 + 36.0);
        let line = |pts: Vec<(f32, f32)>, color: &str, dash: &str| {
            let p: Vec<String> = pts.iter().map(|(a, b)| format!("{a:.1},{b:.1}")).collect();
            format!("<polyline fill=\"none\" stroke=\"{color}\" stroke-width=\"1.6\" {dash} points=\"{}\"/>\n", p.join(" "))
        };
        s += &line(data.iter().map(|d| (x(d.0), yp(d.2.0))).collect(), "#aaa", "");
        s += &line(data.iter().map(|d| (x(d.0), yb(d.1.2))).collect(), "#36c", "stroke-dasharray=\"4 3\"");
        s += &line(data.iter().map(|d| (x(d.0), yp(d.1.0))).collect(), "#c33", "");
        let _ = writeln!(s, "<text x=\"{}\" y=\"{}\" fill=\"#666\">{tmax} s</text>", w - pad - 20.0, y0 + h - pad + 16.0);
    }
    s + "</svg>\n"
}

#[test]
fn metro_guns_kick_the_way_metro_does() {
    let root = rustvr_asset::default_root();
    let weapons = content::metro_weapons(&root);
    let guns: Vec<&content::Weapon> = weapons.iter().filter(|w| w.rigged() && !w.kicks.is_empty()).collect();
    if guns.is_empty() {
        eprintln!("no extracted Metro guns with recoil in {}: skipped", root.display());
        return;
    }
    std::fs::create_dir_all(out_dir()).unwrap();
    for w in guns {
        let d = physical::weapon_desc(0, w, Pose::default(), 0);
        let kicks = physical::kick_descs(d.id, w);
        let views = kicks.iter().filter(|k| k.layer == KICK_LAYER_VIEW).count();
        eprintln!("== {} ({}): {} kicks ({} view, {} gun), {} rounds/s", w.name, w.key, kicks.len(), views, kicks.len() - views, w.rounds_per_second);
        // The Kalash family has both layers; some guns only the gun's own fire motions (Metro
        // gives the revolver, the Helsing and the single shots no camera tracks).
        assert!(!kicks.is_empty());
        if matches!(w.key.as_str(), "wpn_ak" | "wpn_ubludok") {
            assert!(views > 0 && views < kicks.len(), "both layers");
        }
        let mut runs = vec![
            Run { name: "one shot from the hip", stance: Stance::default(), hold: HoldScale::GAME, shots: 1, seconds: 1.2 },
            Run { name: "one shot aimed", stance: Stance { aimed: true, crouched: false }, hold: HoldScale::GAME, shots: 1, seconds: 1.2 },
            // (Long enough for a slow gun's third shot to settle too.)
            Run { name: "three-round burst from the hip", stance: Stance::default(), hold: HoldScale::GAME, shots: 3, seconds: 2.0 / w.rounds_per_second + 1.5 },
        ];
        if w.automatic {
            runs.push(Run { name: "two seconds of full auto, aimed", stance: Stance { aimed: true, crouched: false }, hold: HoldScale::GAME, shots: (2.0 * w.rounds_per_second) as u32, seconds: 3.5 });
            runs.push(Run { name: "full auto in one hand, off the shoulder", stance: Stance::default(), hold: HoldScale { turn: 3.0, push: 2.0 }, shots: (2.0 * w.rounds_per_second) as u32, seconds: 3.5 });
        }
        let mut csv = String::from("run,t,pitch_deg,yaw_deg,back_mm,own_pitch_deg,own_yaw_deg,own_back_mm\n");
        let mut charts = Vec::new();
        for run in &runs {
            let data = fire(&kicks, &d, run, w.rounds_per_second);
            for (t, g, o) in &data {
                let _ = writeln!(csv, "{},{t:.4},{:.3},{:.3},{:.2},{:.3},{:.3},{:.2}", run.name, g.0, g.1, g.2, o.0, o.1, o.2);
            }
            let (pitch, yaw, back) = (peak(&data, |d| d.1.0), peak(&data, |d| d.1.1), peak(&data, |d| d.1.2));
            let own = peak(&data, |d| d.2.0);
            let end = data.last().unwrap().1;
            eprintln!("  {:34} peak pitch {pitch:+5.2} deg, yaw {yaw:+5.2} deg, back {back:5.1} mm; at the end {:+.2} deg   (RustVR's own: {own:+5.2} deg)", run.name, end.0);
            // Every shot kicks the muzzle up and the gun back, and it all settles by itself.
            // (With only the gun's fire motions, the turn is whatever the motion does: the Padonacheg's
            // aimed one dips the muzzle.)
            assert!(pitch > 0.5 || (views == 0 && pitch.abs() > 0.5), "{}: {}: the muzzle kicks up: {pitch}", w.key, run.name);
            assert!(back > 5.0, "{}: {}: the gun pushes back: {back}", w.key, run.name);
            assert!(end.0.abs() < 0.05 && end.2.abs() < 0.5, "{}: {}: settled: {end:?}", w.key, run.name);
            charts.push((run, data));
        }
        // Full auto holds the muzzle up well above one shot's kick while it fires.
        // (A shouldered automatic's: the Gatling's own kicks don't build up like that.)
        // (A fast one: a shotgun's kicks, four a second, each settle before the next.)
        if w.automatic && w.stock && views > 0 && w.rounds_per_second >= 8.0 {
            let one = peak(&charts[1].1, |d| d.1.0);
            let auto = &charts[3].1;
            let held = auto.iter().filter(|d| (0.8..1.8).contains(&d.0)).map(|d| d.1.0).sum::<f32>() / auto.iter().filter(|d| (0.8..1.8).contains(&d.0)).count() as f32;
            eprintln!("  full auto holds the muzzle up {held:.2} deg on average (one aimed shot peaks at {one:.2})");
            assert!(held > one, "{}: a burst holds it up: {held} vs {one}", w.key);
        }
        std::fs::write(out_dir().join(format!("{}.csv", w.key)), csv).unwrap();
        std::fs::write(out_dir().join(format!("{}.svg", w.key)), svg(&w.name, &charts)).unwrap();
    }
}
