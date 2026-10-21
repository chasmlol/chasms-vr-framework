//! Headless tests: everything renders offline except the two that check device handling.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use glam::{Quat, Vec3};

use super::*;
use crate::sounds::VARIATIONS;
use crate::synth::peak_of;

const RATE: u32 = 48_000;
/// Where the right hand usually is, relative to the head.
const HAND: Vec3 = Vec3::new(0.2, -0.25, 0.4);

fn frames(seconds: f32) -> usize {
    (seconds * RATE as f32) as usize
}

fn render_one(sound: Sound, pos: Vec3, intensity: f32, seconds: f32) -> Vec<f32> {
    let audio = Audio::offline(RATE);
    audio.play(sound, pos, intensity);
    audio.render(frames(seconds))
}

fn split(stereo: &[f32]) -> (Vec<f32>, Vec<f32>) {
    (stereo.iter().step_by(2).copied().collect(), stereo.iter().skip(1).step_by(2).copied().collect())
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn energy(x: &[f32]) -> f32 {
    x.iter().map(|s| s * s).sum()
}

/// Energy of the first difference over the energy: weights the spectrum by frequency, so a
/// duller sound scores lower.
fn brightness(x: &[f32]) -> f32 {
    x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>() / energy(x)
}

/// One-pole low-pass at `hz`.
fn low_pass(x: &[f32], hz: f32) -> Vec<f32> {
    let a = 1.0 - (-TAU * hz / RATE as f32).exp();
    let mut low = 0.0;
    x.iter()
        .map(|s| {
            low += a * (s - low);
            low
        })
        .collect()
}

/// Energy below about 150 Hz over the energy above about 1 kHz.
fn bass_ratio(x: &[f32]) -> f32 {
    let lows = low_pass(&low_pass(x, 150.0), 150.0);
    let highs: Vec<f32> = x.iter().zip(low_pass(x, 1000.0)).map(|(s, l)| s - l).collect();
    energy(&lows) / energy(&highs)
}

/// 0 for identical signals, about 1 for unrelated ones. Measured on the first difference
/// (+6 dB per octave), so it weighs the detail the ear follows rather than the phase of a slow
/// low mode.
fn difference(a: &[f32], b: &[f32]) -> f32 {
    let detail = |x: &[f32]| -> Vec<f32> { x.windows(2).map(|w| w[1] - w[0]).collect() };
    let (a, b) = (detail(a), detail(b));
    let at = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0);
    let n = a.len().max(b.len());
    let apart: f32 = (0..n).map(|i| (at(&a, i) - at(&b, i)).powi(2)).sum();
    apart / (energy(&a) + energy(&b))
}

/// How many samples `late` lags `early`, by cross-correlation.
fn lag(early: &[f32], late: &[f32], max: usize) -> isize {
    let score = |d: isize| -> f32 {
        (0..early.len())
            .filter_map(|i| {
                let j = i as isize + d;
                (j >= 0 && (j as usize) < late.len()).then(|| early[i] * late[j as usize])
            })
            .sum()
    };
    (-(max as isize)..=max as isize).max_by(|&a, &b| score(a).total_cmp(&score(b))).unwrap()
}

#[test]
fn audio_can_be_shared_between_threads() {
    fn shareable<T: Send + Sync>() {}
    shareable::<Audio>();
}

#[test]
fn every_sound_renders_clean() {
    for sound in Sound::ALL {
        let out = render_one(sound, HAND, 1.0, 2.0);
        assert!(out.iter().all(|s| s.is_finite()), "{sound:?} produced a non-finite sample");
        let peak = peak_of(&out);
        assert!(peak > 0.02, "{sound:?} is silent (peak {peak})");
        assert!(peak < 1.0, "{sound:?} peaks at {peak}");
    }
}

#[test]
fn library_is_short_and_bounded() {
    let bank = sounds::bank(RATE);
    for (sound, variations) in Sound::ALL.iter().zip(&bank.sounds) {
        assert_eq!(variations.len(), VARIATIONS);
        for v in variations {
            for layer in [&v.main, &v.hard, &v.bed] {
                assert!(layer.iter().all(|s| s.is_finite()), "{sound:?}");
                assert!(peak_of(layer) < 1.0, "{sound:?}");
            }
            let seconds = v.len as f32 / RATE as f32;
            let longest = match sound {
                // The shot tails are the room and ring on; the travel loop is one cycle.
                Sound::ShotPistol | Sound::ShotRifle | Sound::ShotShotgun | Sound::GloveTravel => 1.5,
                _ => 0.5,
            };
            assert!(seconds < longest, "{sound:?} lasts {seconds} s");
        }
    }
}

#[test]
fn variations_differ() {
    let bank = sounds::bank(RATE);
    for (sound, variations) in Sound::ALL.iter().zip(&bank.sounds) {
        for i in 0..VARIATIONS {
            for j in i + 1..VARIATIONS {
                let d = difference(&variations[i].main, &variations[j].main);
                assert!(d > 0.2, "{sound:?} variations {i} and {j} are nearly the same ({d})");
            }
        }
    }
    // And in play, the same sound twice in a row never repeats a variation.
    let audio = Audio::offline(RATE);
    audio.play(Sound::Grab, HAND, 1.0);
    let first = audio.render(frames(0.3));
    audio.play(Sound::Grab, HAND, 1.0);
    let second = audio.render(frames(0.3));
    assert!(difference(&first, &second) > 0.2);
}

#[test]
fn a_source_to_one_side_is_louder_and_earlier_in_that_ear() {
    for sound in [Sound::Grab, Sound::MagSeat, Sound::GloveHover, Sound::Impact, Sound::Melee] {
        let (l, r) = split(&render_one(sound, Vec3::new(0.5, 0.0, 0.0), 1.0, 0.5));
        assert!(rms(&r) > 1.3 * rms(&l), "{sound:?} at +X: left {} right {}", rms(&l), rms(&r));
        let (l, r) = split(&render_one(sound, Vec3::new(-0.5, 0.0, 0.0), 1.0, 0.5));
        assert!(rms(&l) > 1.3 * rms(&r), "{sound:?} at -X: left {} right {}", rms(&l), rms(&r));
    }
    // Straight to the right the far ear hears it about 0.66 ms (32 samples) later.
    let (l, r) = split(&render_one(Sound::MagSeat, Vec3::new(1.0, 0.0, 0.0), 1.0, 0.2));
    let itd = lag(&r, &l, 60);
    assert!((26..=38).contains(&itd), "left lags right by {itd} samples");
}

#[test]
fn a_shot_is_at_the_gun_but_its_tail_fills_the_head() {
    for sound in [Sound::ShotPistol, Sound::ShotRifle, Sound::ShotShotgun] {
        let (l, r) = split(&render_one(sound, Vec3::new(0.5, 0.0, 0.0), 1.0, 0.8));
        // The crack (high-passed by differencing, which leaves out the sub-bass bed).
        let crack = |x: &[f32]| x[..frames(0.01)].windows(2).map(|w| w[1] - w[0]).collect::<Vec<f32>>();
        assert!(rms(&crack(&r)) > 2.0 * rms(&crack(&l)), "{sound:?} crack isn't on the right");
        let tail = frames(0.3)..frames(0.6);
        let balance = rms(&r[tail.clone()]) / rms(&l[tail]);
        assert!((0.8..1.25).contains(&balance), "{sound:?} tail balance {balance}");
    }
}

#[test]
fn the_head_turn_is_applied() {
    // Facing +X: a source at +X is straight ahead, one at +Z (tracking forward) is on the left.
    let facing_right = Listener { pos: Vec3::new(0.0, 1.7, 0.0), rot: Quat::from_rotation_y(FRAC_PI_2) };
    let heard = |pos: Vec3| {
        let audio = Audio::offline(RATE);
        audio.set_listener(facing_right);
        audio.play(Sound::MagSeat, facing_right.pos + pos, 1.0);
        split(&audio.render(frames(0.3)))
    };
    let (l, r) = heard(Vec3::new(0.5, 0.0, 0.0));
    assert!((rms(&l) - rms(&r)).abs() < 0.01 * rms(&l));
    let (l, r) = heard(Vec3::new(0.0, 0.0, 0.5));
    assert!(rms(&l) > 1.3 * rms(&r));
}

#[test]
fn behind_is_duller_than_in_front() {
    // Broadband sounds: a low-pass can't change the balance of a pure tone.
    for sound in [Sound::Grab, Sound::MagSeat, Sound::EmptyClick, Sound::SlideBack] {
        let (front, _) = split(&render_one(sound, Vec3::new(0.0, 0.0, 0.5), 1.0, 0.5));
        let (back, _) = split(&render_one(sound, Vec3::new(0.0, 0.0, -0.5), 1.0, 0.5));
        let (f, b) = (brightness(&front), brightness(&back));
        assert!(b < 0.8 * f, "{sound:?}: front {f}, behind {b}");
    }
}

#[test]
fn distance_attenuation_is_monotonic() {
    let distances = [0.3, 0.45, 0.7, 1.0, 2.0, 4.0, 8.0, 16.0];
    let levels: Vec<f32> =
        distances.iter().map(|&d| rms(&render_one(Sound::Grab, Vec3::new(0.0, 0.0, d), 1.0, 0.4))).collect();
    for (pair, d) in levels.windows(2).zip(distances.windows(2)) {
        assert!(pair[1] < pair[0], "louder at {} m than at {} m: {levels:?}", d[1], d[0]);
    }
    // Inverse distance far out: twice as far is half the level.
    let halving = levels[7] / levels[6];
    assert!((halving - 0.5).abs() < 0.02, "8 m -> 16 m scales by {halving}");
    // The gain law itself never rises with distance and stays finite at the ear.
    let gains: Vec<f32> = (0..2000).map(|i| spatial::attenuation(i as f32 * 0.01)).collect();
    assert!(gains.windows(2).all(|g| g[1] <= g[0]) && gains[0].is_finite());
}

#[test]
fn near_sources_get_more_bass() {
    for sound in [Sound::Grab, Sound::Impact, Sound::MagSeat, Sound::Melee] {
        let (near, _) = split(&render_one(sound, Vec3::new(0.0, 0.0, 0.25), 1.0, 0.5));
        let (far, _) = split(&render_one(sound, Vec3::new(0.0, 0.0, 2.0), 1.0, 0.5));
        let (n, f) = (bass_ratio(&near), bass_ratio(&far));
        // At least +1 dB of bass relative to the highs: slight, as asked.
        assert!(n > 1.25 * f, "{sound:?} bass ratio near {n}, far {f}");
    }
}

#[test]
fn intensity_scales_level_and_brightness() {
    let soft = render_one(Sound::Impact, HAND, 0.2, 0.6);
    let hard = render_one(Sound::Impact, HAND, 1.0, 0.6);
    assert!(rms(&soft) < 0.5 * rms(&hard), "soft {} hard {}", rms(&soft), rms(&hard));
    assert!(brightness(&soft) < 0.7 * brightness(&hard));
}

#[test]
fn voice_limit_holds_under_200_plays() {
    let audio = Audio::offline(RATE);
    for i in 0..200 {
        let sound = Sound::ALL[i % Sound::ALL.len()];
        audio.play(sound, HAND + Vec3::new(0.0, 0.0, i as f32 * 0.01), 1.0);
    }
    let mut out = audio.render(1);
    assert_eq!(audio.voices(), MAX_VOICES);
    out.extend(audio.render(frames(0.5)));
    assert!(out.iter().all(|s| s.is_finite()) && peak_of(&out) < 1.0, "peak {}", peak_of(&out));

    // Spread over many blocks as well.
    for _ in 0..20 {
        for i in 0..10 {
            audio.play(Sound::ALL[i * 7 % Sound::ALL.len()], HAND, 1.0);
        }
        let out = audio.render(480);
        assert!(audio.voices() <= MAX_VOICES);
        assert!(peak_of(&out) < 1.0);
    }
    audio.render(frames(2.0));
    assert_eq!(audio.voices(), 0);
}

#[test]
fn loops_follow_and_stop() {
    let audio = Audio::offline(RATE);
    let settled = frames(0.25);
    let id = audio.start_loop(Sound::GloveTravel, Vec3::new(1.0, 0.0, 0.0));
    let (l, r) = split(&audio.render(frames(0.5)));
    assert!(rms(&r[settled..]) > 1.5 * rms(&l[settled..]));

    audio.move_loop(id, Vec3::new(-1.0, 0.0, 0.0));
    let (l, r) = split(&audio.render(frames(0.5)));
    assert!(rms(&l[settled..]) > 1.5 * rms(&r[settled..]));

    // It keeps going well past the end of its one-second buffer.
    let (l, _) = split(&audio.render(frames(2.0)));
    assert!(rms(&l[frames(1.8)..]) > 0.01);
    assert_eq!(audio.voices(), 1);

    audio.stop_loop(id);
    audio.render(frames(0.1));
    assert_eq!(audio.voices(), 0);
    assert_eq!(peak_of(&audio.render(frames(0.1))), 0.0);

    // A stale id is harmless.
    audio.move_loop(id, Vec3::ZERO);
    audio.stop_loop(id);
    assert_eq!(peak_of(&audio.render(256)), 0.0);
}

#[test]
fn a_play_between_renders_is_sample_accurate() {
    let front = Vec3::new(0.0, 0.0, 0.5);
    let reference = Audio::offline(RATE);
    reference.play(Sound::UiClick, front, 1.0);
    let expected = reference.render(4000);

    let audio = Audio::offline(RATE);
    assert!(audio.render(1237).iter().all(|&s| s == 0.0));
    audio.play(Sound::UiClick, front, 1.0);
    // Starts on the first frame of the next render: identical to playing at time zero.
    assert_eq!(audio.render(4000), expected);
    // ...after the limiter's fixed look-ahead (64 samples) and nothing else.
    let onset = expected.iter().position(|&s| s != 0.0).unwrap() / 2;
    assert!(onset <= 64, "onset at frame {onset}");

    // And how a span is cut into renders changes nothing.
    let whole = render_one(Sound::ShotRifle, HAND, 1.0, 0.2);
    let audio = Audio::offline(RATE);
    audio.play(Sound::ShotRifle, HAND, 1.0);
    let mut parts = audio.render(1);
    for n in [255, 2048, 7296] {
        parts.extend(audio.render(n));
    }
    assert_eq!(parts, whole);
}

#[test]
fn audio_new_neither_panics_nor_blocks() {
    let started = Instant::now();
    let audio = Audio::new();
    assert!(started.elapsed() < Duration::from_millis(250), "new() took {:?}", started.elapsed());
    // The user is at this PC: whatever device opens stays at zero volume.
    audio.set_volume(0.0);
    std::thread::sleep(Duration::from_millis(500));
    audio.set_listener(Listener::default());
    audio.play(Sound::UiHover, HAND, 0.0);
    let id = audio.start_loop(Sound::GloveTravel, HAND);
    audio.move_loop(id, Vec3::ZERO);
    audio.stop_loop(id);
    assert!(audio.render(64).iter().all(|&s| s == 0.0));
    assert!(audio.voices() <= MAX_VOICES);
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn without_a_device_it_runs_silently() {
    let audio = Audio::with_output(Box::new(|| anyhow::bail!("no audio device (test)")));
    audio.set_listener(Listener::default());
    audio.set_volume(0.5);
    audio.play(Sound::Grab, HAND, 1.0);
    let id = audio.start_loop(Sound::GloveTravel, HAND);
    audio.move_loop(id, Vec3::ZERO);
    audio.stop_loop(id);
    assert_eq!(audio.render(480), vec![0.0; 960]);
    assert_eq!(audio.voices(), 0);
    let dropped = Instant::now();
    drop(audio);
    assert!(dropped.elapsed() < Duration::from_millis(100));
}

#[test]
fn broken_input_is_ignored() {
    let audio = Audio::offline(RATE);
    audio.set_listener(Listener { pos: Vec3::NAN, rot: Quat::IDENTITY });
    audio.set_volume(f32::NAN);
    audio.play(Sound::Grab, Vec3::new(f32::INFINITY, 0.0, 0.0), 1.0);
    audio.play(Sound::Grab, HAND, f32::NAN);
    let out = audio.render(frames(0.3));
    assert!(out.iter().all(|s| s.is_finite()) && peak_of(&out) > 0.02);
}

/// Counts heap traffic on threads that ask for it, to prove the mixer leaves the heap alone.
struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

fn note() {
    if WATCHING.try_with(Cell::get).unwrap_or(false) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn mixing_never_allocates() {
    let audio = Audio::offline(RATE);
    let Output::Offline(mixer) = &audio.output else { unreachable!() };
    let mut mixer = mixer.lock().unwrap();
    // Everything the audio thread does: more plays than voices (stealing), loops moving and
    // stopping, and a head that moves.
    for i in 0..200 {
        audio.play(Sound::ALL[i % Sound::ALL.len()], HAND, 1.0);
    }
    let id = audio.start_loop(Sound::GloveTravel, HAND);
    audio.move_loop(id, -HAND);
    audio.set_listener(Listener { pos: Vec3::Y, rot: Quat::from_rotation_y(1.0) });
    WATCHING.set(true);
    mixer.mix(frames(0.5), |_, _, _| {});
    WATCHING.set(false);
    audio.stop_loop(id);
    WATCHING.set(true);
    mixer.mix(frames(0.5), |_, _, _| {});
    WATCHING.set(false);
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
}

/// Writes a few short scenes to `captures/audio` for listening on headphones:
/// `cargo test -p rustvr-audio -- --ignored write_listening_wavs`.
#[test]
#[ignore = "writes WAV files to captures/audio"]
fn write_listening_wavs() {
    enum Cue {
        Play(Sound, Vec3, f32),
        Loop(Sound, Vec3),
        Move(Vec3),
        Stop,
    }
    use Cue::*;

    fn scene(cues: &[(f32, Cue)], seconds: f32) -> Vec<f32> {
        let audio = Audio::offline(RATE);
        let mut out = Vec::new();
        let mut looping = None;
        for (at, cue) in cues {
            out.extend(audio.render(frames(*at) - out.len() / 2));
            match *cue {
                Play(sound, pos, intensity) => audio.play(sound, pos, intensity),
                Loop(sound, pos) => looping = Some(audio.start_loop(sound, pos)),
                Move(pos) => looping.iter().for_each(|&id| audio.move_loop(id, pos)),
                Stop => looping.iter().for_each(|&id| audio.stop_loop(id)),
            }
        }
        out.extend(audio.render(frames(seconds) - out.len() / 2));
        out
    }

    fn write(name: &str, samples: &[f32]) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../captures/audio");
        std::fs::create_dir_all(&dir).unwrap();
        let data = (samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(44 + data as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            wav.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(dir.join(name), wav).unwrap();
    }

    let hip = Vec3::new(0.22, -0.6, 0.05);
    let hands: Vec<(f32, Cue)> = [
        (Sound::Grab, HAND),
        (Sound::Release, HAND),
        (Sound::HolsterIn, hip),
        (Sound::HolsterOut, hip),
        (Sound::MagEject, HAND),
        (Sound::MagSeat, HAND),
        (Sound::SlideBack, HAND),
        (Sound::SlideForward, HAND),
        (Sound::LastRound, HAND),
        (Sound::EmptyClick, HAND),
        (Sound::Fail, HAND),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (sound, pos))| (0.1 + 0.5 * i as f32, Play(sound, pos, 1.0)))
    .collect();
    write("hands.wav", &scene(&hands, 5.8));

    // Hover, lock and flick at the hand; the object then flies in from 4 m and is caught.
    let target = Vec3::new(0.6, -0.4, 4.0);
    let mut glove = vec![
        (0.1, Play(Sound::GloveHover, HAND, 1.0)),
        (0.5, Play(Sound::GloveLock, HAND, 1.0)),
        (0.9, Play(Sound::GloveFlick, HAND, 1.0)),
        (0.95, Loop(Sound::GloveTravel, target)),
    ];
    glove.extend((1..=20).map(|i| (0.95 + 0.035 * i as f32, Move(target.lerp(HAND, i as f32 / 20.0)))));
    glove.extend([(1.65, Stop), (1.65, Play(Sound::GloveCatch, HAND, 1.0))]);
    write("glove.wav", &scene(&glove, 2.3));

    let floor = Vec3::new(0.3, -0.9, 0.7);
    let mut impacts: Vec<(f32, Cue)> = [0.1, 0.3, 0.55, 0.8, 1.0]
        .into_iter()
        .enumerate()
        .map(|(i, intensity)| (0.1 + 0.45 * i as f32, Play(Sound::Impact, floor, intensity)))
        .collect();
    impacts.extend([
        (2.5, Play(Sound::Melee, Vec3::new(0.1, -0.1, 0.6), 0.5)),
        (2.95, Play(Sound::Melee, Vec3::new(0.1, -0.1, 0.6), 1.0)),
    ]);
    write("impacts.wav", &scene(&impacts, 3.5));

    let shots = [
        (0.1, Play(Sound::ShotPistol, HAND, 1.0)),
        (1.4, Play(Sound::ShotRifle, HAND, 1.0)),
        (2.7, Play(Sound::ShotShotgun, HAND, 1.0)),
    ];
    write("shots.wav", &scene(&shots, 4.2));

    let shoulder = Vec3::new(0.24, 0.1, -0.06);
    let ui = [
        (0.1, Play(Sound::UiHover, HAND, 1.0)),
        (0.4, Play(Sound::UiHover, HAND, 1.0)),
        (0.7, Play(Sound::UiClick, HAND, 1.0)),
        (1.0, Play(Sound::ZoneTick, hip, 1.0)),
        (1.3, Play(Sound::ZoneTick, shoulder, 1.0)),
    ];
    write("ui.wav", &scene(&ui, 1.7));

    // A magazine seat circling the head at 0.5 m: front, right, behind, left.
    let orbit: Vec<(f32, Cue)> = (0..12)
        .map(|i| {
            let a = TAU * i as f32 / 12.0;
            (0.1 + 0.3 * i as f32, Play(Sound::MagSeat, 0.5 * Vec3::new(a.sin(), 0.0, a.cos()), 1.0))
        })
        .collect();
    write("orbit.wav", &scene(&orbit, 3.8));
}
