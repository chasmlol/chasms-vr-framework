//! Sound packs, headless: decoding, loading, variation choice, fallback and placement. The packs
//! are written to a temporary folder as WAV files at 44.1 kHz, so the resampler runs too; a
//! real game's Ogg Vorbis files are decoded too when `local_assets/` has them.

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::Vec3;

use super::*;
use crate::decode::{self, Pcm};
use crate::pack::PackSpec;

const RATE: u32 = 48_000;
const FILE_RATE: u32 = 44_100;
const HAND: Vec3 = Vec3::new(0.2, -0.25, 0.4);

fn frames(seconds: f32) -> usize {
    (seconds * RATE as f32) as usize
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn split(stereo: &[f32]) -> (Vec<f32>, Vec<f32>) {
    (stereo.iter().step_by(2).copied().collect(), stereo.iter().skip(1).step_by(2).copied().collect())
}

/// A tone with a 5 ms fade at each end, mono at 44.1 kHz.
fn tone(hz: f32, amp: f32, seconds: f32) -> Pcm {
    let n = (seconds * FILE_RATE as f32) as usize;
    let fade = (0.005 * FILE_RATE as f32) as usize;
    let samples = (0..n)
        .map(|i| {
            let env = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
            amp * env * (TAU * hz * i as f32 / FILE_RATE as f32).sin()
        })
        .collect();
    Pcm { rate: FILE_RATE, channels: 1, samples }
}

/// A pack folder (removed on drop) with `files` and a `sounds.toml`.
pub(crate) struct TempPack(pub PathBuf);

impl TempPack {
    fn new(name: &str, files: &[(&str, Pcm)], toml: &str) -> TempPack {
        let dir = std::env::temp_dir().join(format!("rustvr-audio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (path, pcm) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, decode::wav_bytes(pcm)).unwrap();
        }
        std::fs::write(dir.join(pack::PACK_FILE), toml).unwrap();
        TempPack(dir)
    }
}

impl Drop for TempPack {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn wait_ready(audio: &Audio) -> PackInfo {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match audio.pack_status() {
            PackStatus::Ready(info) => return info,
            PackStatus::Failed { error, .. } => panic!("the pack failed: {error}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "the pack never loaded");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A gun set with a two-layer shot and a tail, a dry fire of three variations (amplitudes
/// 0.1, 0.2 and 0.4, to tell them apart), and a default grab.
pub(crate) fn gun_pack(name: &str) -> TempPack {
    let files = [
        ("gun/close.wav", tone(900.0, 0.8, 0.15)),
        ("gun/body.wav", tone(180.0, 0.6, 0.3)),
        ("gun/tail.wav", tone(400.0, 0.5, 0.6)),
        ("gun/dry_1.wav", tone(1500.0, 0.1, 0.1)),
        ("gun/dry_2.wav", tone(1500.0, 0.2, 0.1)),
        ("gun/dry_3.wav", tone(1500.0, 0.4, 0.1)),
        ("hands/grab.wav", tone(300.0, 0.3, 0.1)),
        ("gun/casing.wav", tone(2500.0, 0.3, 0.1)),
    ];
    let toml = r#"
format = 1
name = "test"

[defaults]
grab = "hands/grab.wav"

[sets.rifle]
dry_fire = ["gun/dry_1.wav", "gun/dry_2.wav", "gun/dry_3.wav"]

[sets.gun]
extends = "rifle"
fire = [{ files = ["gun/close.wav"] }, { files = [{ file = "gun/body.wav", gain = 0.5 }] }]
fire_tail = { files = ["gun/tail.wav"], spatial = false }
casing = { files = ["gun/casing.wav"], delay = 0.2 }
"#;
    TempPack::new(name, &files, toml)
}

#[test]
fn wav_round_trips_and_resamples_cleanly() {
    let src = tone(1000.0, 0.5, 0.5);
    let back = decode::decode(&decode::wav_bytes(&src)).unwrap();
    assert_eq!((back.rate, back.channels, back.frames()), (FILE_RATE, 1, src.frames()));
    assert!(back.samples.iter().zip(&src.samples).all(|(a, b)| (a - b).abs() < 1e-4));

    // 44.1 -> 48 kHz: the length scales, a tone keeps its pitch and its level.
    let up = decode::resample(&src.samples, FILE_RATE, RATE);
    assert!((up.len() as f32 - src.frames() as f32 * RATE as f32 / FILE_RATE as f32).abs() <= 1.0);
    let mid = &up[frames(0.05)..frames(0.45)];
    let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    assert!((398..=402).contains(&crossings), "{crossings} cycles in 0.4 s");
    let level = rms(mid) * 2f32.sqrt();
    assert!((level - 0.5).abs() < 0.005, "amplitude {level}");

    // Down to 22.05 kHz: a tone above the new Nyquist is filtered out, not aliased.
    let high = tone(15_000.0, 0.5, 0.3);
    let down = decode::resample(&high.samples, FILE_RATE, 22_050);
    let left = rms(&down[1000..down.len() - 1000]);
    assert!(left < 0.5 * 0.01, "15 kHz leaks through at {left}");
}

#[test]
fn wav_formats_decode() {
    // 24-bit stereo and 32-bit float, hand-made.
    let header = |format: u16, channels: u16, bits: u16, data: &[u8]| {
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&format.to_le_bytes());
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&22_050u32.to_le_bytes());
        w.extend_from_slice(&(22_050 * channels as u32 * bits as u32 / 8).to_le_bytes());
        w.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        w.extend_from_slice(&bits.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(data);
        w
    };
    let s24 = header(1, 2, 24, &[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0]);
    let p = decode::decode(&s24).unwrap();
    assert_eq!((p.rate, p.channels), (22_050, 2));
    assert!((p.samples[0] - 0.5).abs() < 1e-6 && (p.samples[1] + 0.5).abs() < 1e-6, "{:?}", p.samples);
    let f32s: Vec<u8> = [0.25f32, -1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let p = decode::decode(&header(3, 1, 32, &f32s)).unwrap();
    assert_eq!(p.samples, vec![0.25, -1.0]);
    assert!(decode::decode(b"not audio at all").is_err());
}

#[test]
fn a_games_ogg_vorbis_decodes() {
    // Metro's `.vba` files are plain Ogg Vorbis; a pack extracted from it holds them as `.ogg`.
    let root = std::env::var_os("RUSTVR_ASSETS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local_assets"));
    let dir = root.join("metro2033").join("sounds");
    let Some(file) = std::fs::read_dir(dir.join("weapons").join("ak_74")).ok().and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "ogg")))
    else {
        eprintln!("no extracted Metro sounds under {}: skipped", dir.display());
        return;
    };
    let pcm = decode::decode(&std::fs::read(&file).unwrap()).unwrap();
    assert!(pcm.rate == 44_100 && (1..=2).contains(&pcm.channels), "{}: {} Hz x {}", file.display(), pcm.rate, pcm.channels);
    assert!(pcm.seconds() > 0.05 && pcm.peak() > 0.05 && pcm.peak() <= 1.5, "{}: {:.2} s, peak {}", file.display(), pcm.seconds(), pcm.peak());
}

#[test]
fn sets_inherit_and_override() {
    let spec = PackSpec::parse(
        r#"
format = 1
[defaults]
grab = "a.ogg"
mag_in = "default_in.ogg"
[sets.base]
mag_in = ["b1.ogg", "b2.ogg"]
[sets.gun]
extends = "base"
grab = { files = ["g.ogg"], gain = 0.5 }
fire = [{ files = ["c.ogg"] }, { files = ["d.ogg"], spatial = false, delay = 0.1 }]
"#,
    )
    .unwrap();
    let gun = spec.resolve(Some("gun")).unwrap();
    let (from, grab) = &gun["grab"];
    assert_eq!((from.as_deref(), grab.layers()[0].gain), (Some("gun"), 0.5));
    let (from, mag) = &gun["mag_in"];
    assert_eq!((from.as_deref(), mag.layers()[0].files.len()), (Some("base"), 2));
    let fire = gun["fire"].1.layers();
    assert_eq!(fire.len(), 2);
    assert!(fire[0].spatial && !fire[1].spatial && fire[1].delay == 0.1);
    assert_eq!(spec.resolve(None).unwrap()["mag_in"].0, None);
    assert!(spec.resolve(Some("nope")).is_err());
    let looped = PackSpec::parse("format = 1\n[sets.a]\nextends = \"b\"\n[sets.b]\nextends = \"a\"\n").unwrap();
    assert!(looped.resolve(Some("a")).is_err(), "a cycle is an error, not a hang");
    assert!(PackSpec::parse("format = 2").is_err());
}

#[test]
fn a_pack_event_plays_its_layers_and_falls_back_otherwise() {
    let p = gun_pack("layers");
    let audio = Audio::offline(RATE);
    // Before the pack is there, the synthesised sound plays.
    audio.play_event(Event::Fire, SetKey::of("gun"), Some(Sound::ShotRifle), HAND, 1.0);
    audio.render(frames(0.05));
    assert_eq!((audio.pack_plays(Event::Fire), audio.synth_plays()), (0, 1));

    audio.load_pack(Some(&p.0));
    let info = wait_ready(&audio);
    assert_eq!((info.sets, info.rate, info.missing.len()), (2, RATE, 0));
    audio.render(frames(1.0));
    assert_eq!(audio.voices(), 0);

    // The gun's shot: both spatial layers and the tail, at once.
    audio.play_event(Event::Fire, SetKey::of("GUN"), Some(Sound::ShotRifle), HAND, 1.0);
    audio.play_event(Event::FireTail, SetKey::of("gun"), None, HAND, 1.0);
    let out = audio.render(frames(0.05));
    assert_eq!(audio.voices(), 3);
    assert_eq!((audio.pack_plays(Event::Fire), audio.pack_plays(Event::FireTail), audio.synth_plays()), (1, 1, 1));
    assert!(out.iter().all(|s| s.is_finite()) && peak(&out) < 1.0);
    audio.render(frames(1.0));

    // Inherited from `rifle`, from `[defaults]`, and a set the pack doesn't have: its defaults.
    audio.play_event(Event::DryFire, SetKey::of("gun"), Some(Sound::EmptyClick), HAND, 1.0);
    audio.play_event(Event::Grab, SetKey::of("gun"), Some(Sound::Grab), HAND, 1.0);
    audio.play_event(Event::Grab, SetKey::of("bottle"), Some(Sound::Grab), HAND, 1.0);
    audio.render(frames(0.02));
    assert_eq!((audio.pack_plays(Event::DryFire), audio.pack_plays(Event::Grab), audio.synth_plays()), (1, 2, 1));

    // What the pack doesn't have falls back; with no fallback, nothing plays.
    audio.play_event(Event::MagOut, SetKey::of("gun"), Some(Sound::MagEject), HAND, 1.0);
    audio.play_event(Event::Explosion, SetKey::of("gun"), None, HAND, 1.0);
    audio.render(frames(0.02));
    assert_eq!((audio.pack_plays(Event::MagOut), audio.pack_plays(Event::Explosion), audio.synth_plays()), (0, 0, 2));

    // Unloaded, it's the runtime's own sounds again.
    audio.render(frames(1.0));
    audio.load_pack(None);
    audio.play_event(Event::Fire, SetKey::of("gun"), Some(Sound::ShotRifle), HAND, 1.0);
    audio.render(frames(0.02));
    assert_eq!((audio.pack_plays(Event::Fire), audio.synth_plays()), (1, 3));
}

#[test]
fn pack_variations_never_repeat_back_to_back() {
    let p = gun_pack("variations");
    let audio = Audio::offline(RATE);
    audio.load_pack(Some(&p.0));
    wait_ready(&audio);
    let front = Vec3::new(0.0, 0.0, 0.45);
    // Each variation's level tells it apart: 0.1, 0.2 and 0.4.
    let mut seen = Vec::new();
    for _ in 0..30 {
        audio.play_event(Event::DryFire, SetKey::of("gun"), None, front, 1.0);
        let level = rms(&audio.render(frames(0.15)));
        seen.push(level);
    }
    let lo = seen.iter().copied().fold(f32::MAX, f32::min);
    let which: Vec<usize> = seen.iter().map(|l| ((l / lo).log2().round()) as usize).collect();
    assert!(which.iter().all(|&w| w <= 2), "levels {seen:?}");
    assert!(which.windows(2).all(|w| w[0] != w[1]), "a variation repeated: {which:?}");
    for v in 0..3 {
        assert!(which.contains(&v), "variation {v} never played: {which:?}");
    }
}

#[test]
fn bed_layers_fill_the_head_and_delays_hold_back() {
    let p = gun_pack("placement");
    let audio = Audio::offline(RATE);
    audio.load_pack(Some(&p.0));
    wait_ready(&audio);
    let right = Vec3::new(0.5, 0.0, 0.0);
    // The shot's spatial layers are at the gun, on the right...
    audio.play_event(Event::Fire, SetKey::of("gun"), None, right, 1.0);
    let (l, r) = split(&audio.render(frames(0.12)));
    assert!(rms(&r) > 1.5 * rms(&l), "fire: left {} right {}", rms(&l), rms(&r));
    audio.render(frames(1.0));
    // ...its tail (spatial = false) fills both ears alike.
    audio.play_event(Event::FireTail, SetKey::of("gun"), None, right, 1.0);
    let (l, r) = split(&audio.render(frames(0.4)));
    let balance = rms(&r) / rms(&l);
    assert!((0.95..1.05).contains(&balance), "tail balance {balance}");
    audio.render(frames(1.0));
    // The casing waits 0.2 s (plus the limiter's 64-sample look-ahead), then lands.
    audio.play_event(Event::Casing, SetKey::of("gun"), None, Vec3::new(0.0, -1.0, 0.3), 1.0);
    let out = audio.render(frames(0.4));
    let onset = out.iter().position(|s| s.abs() > 1e-5).unwrap() / 2;
    assert!((frames(0.2)..frames(0.2) + 200).contains(&onset), "casing starts at frame {onset}");
}

#[test]
fn missing_files_are_skipped_and_a_broken_pack_falls_back() {
    let files = [("a.wav", tone(500.0, 0.3, 0.1))];
    let p = TempPack::new("missing", &files, "format = 1\n[defaults]\ngrab = [\"a.wav\", \"gone.wav\"]\nmag_in = \"gone_too.ogg\"\nwhatever = \"a.wav\"\n");
    let audio = Audio::offline(RATE);
    audio.load_pack(Some(&p.0));
    let info = wait_ready(&audio);
    assert_eq!(info.missing.len(), 2, "{:?}", info.missing);
    assert_eq!(info.unknown, vec!["whatever".to_string()]);
    audio.play_event(Event::Grab, SetKey::NONE, Some(Sound::Grab), HAND, 1.0);
    audio.play_event(Event::MagIn, SetKey::NONE, Some(Sound::MagSeat), HAND, 1.0);
    audio.render(frames(0.02));
    assert_eq!((audio.pack_plays(Event::Grab), audio.pack_plays(Event::MagIn), audio.synth_plays()), (1, 0, 1));

    let bad = TempPack::new("broken", &[], "format = 1\n[defaults\n");
    audio.load_pack(Some(&bad.0));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !matches!(audio.pack_status(), PackStatus::Failed { .. }) {
        assert!(Instant::now() < deadline, "{:?}", audio.pack_status());
        std::thread::sleep(Duration::from_millis(5));
    }
    // The old pack stays until a new one is ready: a failed load doesn't take it away.
    audio.play_event(Event::Grab, SetKey::NONE, Some(Sound::Grab), HAND, 1.0);
    audio.render(frames(0.02));
    assert_eq!(audio.pack_plays(Event::Grab), 2);
}

#[test]
fn switching_packs_mid_sound_is_clean() {
    let a = gun_pack("switch_a");
    let b = gun_pack("switch_b");
    let audio = Audio::offline(RATE);
    audio.load_pack(Some(&a.0));
    wait_ready(&audio);
    audio.play_event(Event::FireTail, SetKey::of("gun"), None, HAND, 1.0);
    audio.render(frames(0.1));
    // A new pack while the old one's tail still plays: the tail finishes, from the old pack.
    audio.load_pack(Some(&b.0));
    wait_ready(&audio);
    audio.play_event(Event::Fire, SetKey::of("gun"), None, HAND, 1.0);
    let out = audio.render(frames(0.1));
    assert_eq!(audio.voices(), 3, "the old tail and the new shot's two layers");
    assert!(out.iter().all(|s| s.is_finite()));
    audio.render(frames(1.0));
    assert_eq!(audio.voices(), 0);
    // And the old pack is freed once nothing plays from it.
    audio.set_listener(Listener::default());
    assert_eq!(audio.shared.packs.lock().unwrap().len(), 1);
}

/// Writes Metro's sounds as the runtime plays them (`local_assets/metro2033/sounds`, from
/// `metro-extract`) to `captures/audio/pack_*.wav`, for checking the mapping by ear:
/// `cargo test -p rustvr-audio -- --ignored write_pack_listening_wavs`.
#[test]
#[ignore = "writes WAV files to captures/audio; needs Metro's extracted sounds"]
fn write_pack_listening_wavs() {
    let root = std::env::var_os("RUSTVR_ASSETS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local_assets"));
    let dir = root.join("metro2033").join("sounds");
    let audio = Audio::offline(RATE);
    audio.load_pack(Some(&dir));
    wait_ready(&audio);
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../captures/audio");
    std::fs::create_dir_all(&out).unwrap();
    let floor = Vec3::new(0.2, -1.6, 0.4);
    let write = |name: &str, cues: &[(f32, Event, &str, Vec3)], seconds: f32| {
        let mut samples = Vec::new();
        for (at, event, set, pos) in cues {
            samples.extend(audio.render(frames(*at) - samples.len() / 2));
            audio.play_event(*event, SetKey::of(set), event.fallback(), *pos, 1.0);
        }
        samples.extend(audio.render(frames(seconds) - samples.len() / 2));
        std::fs::write(out.join(name), decode::wav_bytes(&Pcm { rate: RATE, channels: 2, samples })).unwrap();
    };
    for gun in ["wpn_ak", "wpn_ubludok", "wpn_revolver", "wpn_duplet"] {
        let mut cues = vec![(0.1, Event::Grab, gun, HAND)];
        for i in 0..3 {
            let t = 0.8 + 0.12 * i as f32;
            cues.extend([(t, Event::Fire, gun, HAND), (t, Event::FireTail, gun, HAND), (t, Event::Casing, gun, floor)]);
        }
        cues.extend([
            (3.5, Event::DryFire, gun, HAND),
            (4.2, Event::MagOut, gun, HAND),
            (5.4, Event::Pouch, gun, Vec3::new(-0.25, -0.1, -0.05)),
            (6.4, Event::MagIn, gun, HAND),
            (7.4, Event::BoltBack, gun, HAND),
            (7.9, Event::BoltForward, gun, HAND),
            (8.8, Event::HolsterIn, gun, Vec3::new(0.22, -0.6, 0.05)),
            (10.0, Event::HolsterOut, gun, Vec3::new(0.22, -0.6, 0.05)),
            (11.5, Event::Impact, gun, floor),
        ]);
        write(&format!("pack_{gun}.wav"), &cues, 13.0);
    }
    let ui = [(0.1, Event::UiClick, "", HAND), (0.6, Event::ZoneTick, "", HAND), (1.1, Event::Release, "", HAND), (3.0, Event::Melee, "wpn_ak", HAND)];
    write("pack_hands.wav", &ui, 4.5);
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}
