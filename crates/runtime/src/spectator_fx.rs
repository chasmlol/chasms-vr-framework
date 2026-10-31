//! The desktop view's effects studio: what makes a recording or a stream of VR play look filmed,
//! cinematic or hyper-real, with effects nobody would want inside a headset (a blur that smears a
//! turn, a view that shakes or widens by itself, a lens that bends). It's a stage of its own after
//! `crate::spectator`'s steadied camera, and only ever the desktop window's: the headset's picture
//! never changes. The research it rests on and how it fits with the other desktop view features is
//! in `docs/research/desktop-fx.md`.
//!
//! **The camera** ([`Stage::shot`]), before anything's drawn:
//! - *Smoothed jumps*: a snap turn, a snap crouch or a recentre jumps the game's world under the
//!   player in one frame. The desktop view's camera on the world turns through the jump over
//!   `snap_seconds` instead (the body and what the runtime draws stay with the player, as they
//!   turn with them); where the new picture doesn't reach yet, the one from before the jump fills
//!   in. A move the picture can't be turned through (a crouch, a recentre's step) dissolves from
//!   the picture before it.
//! - *Speed*: the field of view widens with the player's speed through the world, never past what
//!   the game's pictures cover.
//! - *Shake*: a trauma that the runtime's events add to (a gun's recoil, a blow landing, an
//!   explosion, a hard landing, the game's rumble), shaking the camera's turn by its square, with
//!   kicks that spring back (a recoil's climb, a landing's dip), and a handheld sway.
//! - *A chest mount*: the camera turns after the body lazily, a little down, leaning into turns.
//!
//! **The picture** ([`Stage::finish`], after the headset's frame is handed over, so the effects
//! never hold it up): everything's drawn into a picture of the studio's own, then:
//! - *motion blur* by McGuire et al.'s reconstruction (2012): each pixel's motion from the
//!   camera's (the game's depth and the world's walk and turn) and, for what the runtime draws,
//!   from the hand that carries it, blurred over a shutter's share of a frame; the camera's part
//!   stronger towards the edges;
//! - *depth of field* at half size, focused where the player looks, aims, or at what they hold
//!   up close;
//! - *the lens and the film*, in one pass: fisheye, colour fringes, sharpening, glow, exposure,
//!   contrast, white balance, a grade and a LUT, vignette, grain. (No letterbox: the player doesn't
//!   want black bars over the picture.)
//!
//! **The flat HUD**: the game's own HUD, from its own picture of it, where a flat player has it at
//! the screen's edges (its pieces anchored to the side they're nearest, sized by the game's text),
//! instead of floating where the headset has it.

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{EulerRot, Mat4, Quat, Vec2, Vec3};
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer};
use rustvr_gpu::shader::Program;
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use crate::backend::{DepthTarget, EyeTarget, HmdFrame};
use crate::compositor::{Background, Compositor, Layers, projection, view_matrix};
use crate::mechanics::WeaponPose;
use crate::rig::feedback::{Cue, Feedback};
use crate::rig::{RigOutput, WorldMap};
use crate::settings::{Grade, Look, Settings, Studio};

/// A jump of the world in one picture beyond its steady turn and walk: a turn (radians), a move
/// across (metres) or up and down. Below these it's the walk and turn carried on.
const JUMP_TURN: f32 = 0.14;
const JUMP_MOVE: f32 = 0.35;
const JUMP_RISE: f32 = 0.12;
/// Further than this in one picture (metres), or this long between pictures (seconds), is another
/// place (a door to a new cell, a load): a cut, nothing to ease through.
const CUT_MOVE: f32 = 20.0;
const CUT_GAP: f32 = 0.5;
/// A game held up longer than this (seconds) and then taking its steps all at once is a jump,
/// not a hitch.
const HITCH: f32 = 0.3;
/// The picture from before a jump fills in for this long at most (seconds).
const HISTORY_KEEP: f32 = 1.5;

/// The studio's looks: what each sets, and why. The viewers' own options (smoothed jumps, the
/// stretched edges, the flat HUD) are kept as they are.
pub fn look(l: Look, keep: &Studio) -> Studio {
    let clean = Studio {
        look: l,
        smooth_snaps: keep.smooth_snaps,
        snap_seconds: keep.snap_seconds,
        fill_edges: keep.fill_edges,
        flat_hud: keep.flat_hud,
        flat_hud_size: keep.flat_hud_size,
        ..Studio::default()
    };
    match l {
        Look::Clean | Look::Custom => Studio { look: l, ..clean },
        // A film's: 24 frames at a 180 degree shutter (the blur of motion a cinema audience
        // expects), a 90 degree lens (a flat game's, narrower than the headset's, which leaves
        // room to widen at a run), a shallow focus that's pulled to what's looked at, highlights
        // that bloom into a warm halo as film's do, fine grain, and the blockbuster's teal shadows
        // and warm skin. The camera sways a little, as an operator's.
        Look::Cinematic => Studio {
            motion_blur: true,
            shutter: 180.0,
            recorded_fps: 24.0,
            camera_blur: 0.8,
            centre_sharp: 0.6,
            object_blur: 1.0,
            speed_fov: true,
            speed_fov_deg: 8.0,
            shake: true,
            shake_strength: 0.6,
            handheld: 0.25,
            dof: true,
            dof_blur: 1.2,
            dof_near: 0.3,
            focus_seconds: 0.4,
            lens_fov: 90.0,
            chromatic: 0.15,
            vignette: 0.35,
            glow: 0.35,
            grain: 0.12,
            sharpen: 0.15,
            contrast: 1.1,
            saturation: 0.95,
            temperature: 0.1,
            grade: Grade::TealOrange,
            grade_mix: 0.6,
            ..clean
        },
        // A body camera's footage (Unrecord's and Bodycam's look): a wide lens bent well towards a
        // fisheye, worn on the chest (it turns after the body, a little down, swaying with it), a
        // slow shutter's smear at 30 frames (a small sensor exposes long), everything in focus (a
        // tiny sensor behind a wide lens), the cheap optics' fringes and dark corners, hard in-camera
        // sharpening, sensor noise, and a cold, flat, desaturated colour.
        Look::HyperReal => Studio {
            motion_blur: true,
            shutter: 300.0,
            recorded_fps: 30.0,
            camera_blur: 1.0,
            centre_sharp: 0.15,
            object_blur: 1.0,
            shake: true,
            shake_strength: 1.0,
            handheld: 0.6,
            lens_fov: 106.0,
            fisheye: 0.45,
            chromatic: 0.35,
            vignette: 0.5,
            glow: 0.15,
            grain: 0.25,
            sharpen: 0.6,
            exposure: 0.15,
            contrast: 1.15,
            saturation: 0.8,
            temperature: -0.15,
            grade: Grade::Bodycam,
            grade_mix: 0.8,
            chest_mount: 0.7,
            ..clean
        },
        // An action film's: every shot and blow felt in the camera, the view widening hard at a
        // run, a 1/60 shutter's crisp blur at 30 frames, punchy contrast and colour.
        Look::Action => Studio {
            motion_blur: true,
            shutter: 180.0,
            recorded_fps: 30.0,
            camera_blur: 1.0,
            centre_sharp: 0.5,
            object_blur: 1.2,
            speed_fov: true,
            speed_fov_deg: 12.0,
            speed_from: 2.5,
            speed_full: 8.0,
            shake: true,
            shake_strength: 1.6,
            handheld: 0.15,
            lens_fov: 95.0,
            fisheye: 0.1,
            chromatic: 0.2,
            vignette: 0.3,
            glow: 0.25,
            grain: 0.05,
            sharpen: 0.3,
            contrast: 1.2,
            saturation: 1.15,
            temperature: 0.05,
            grade: Grade::TealOrange,
            grade_mix: 0.4,
            ..clean
        },
        // Old film stock: a soft lens with fringes and heavy dark corners, a strong glow,
        // coarse grain, a gentle sway, and a faded warm print.
        Look::Vintage => Studio {
            motion_blur: true,
            shutter: 180.0,
            recorded_fps: 24.0,
            camera_blur: 0.8,
            centre_sharp: 0.3,
            object_blur: 1.0,
            shake: true,
            shake_strength: 0.4,
            handheld: 0.4,
            dof: true,
            dof_blur: 0.8,
            dof_near: 0.5,
            focus_seconds: 0.5,
            lens_fov: 85.0,
            fisheye: 0.05,
            chromatic: 0.25,
            vignette: 0.6,
            glow: 0.5,
            grain: 0.5,
            exposure: 0.1,
            contrast: 0.9,
            saturation: 0.6,
            temperature: 0.3,
            grade: Grade::WarmFilm,
            grade_mix: 1.0,
            ..clean
        },
    }
}

/// Whether the look draws anything over the picture (the camera's effects alone don't need the
/// studio's own picture).
fn draws_effects(s: &Studio, lut: bool) -> bool {
    s.motion_blur
        || s.dof
        || s.fisheye > 0.001
        || s.chromatic > 0.001
        || s.vignette > 0.001
        || s.glow > 0.001
        || s.grain > 0.001
        || s.sharpen > 0.001
        || s.exposure.abs() > 0.001
        || (s.contrast - 1.0).abs() > 0.001
        || (s.saturation - 1.0).abs() > 0.001
        || s.temperature.abs() > 0.001
        || s.tint.abs() > 0.001
        || (s.grade != Grade::None && s.grade_mix > 0.001)
        || (lut && s.lut_mix > 0.001)
}

// ---- the camera ---------------------------------------------------------------------------------

/// A jump of the world being turned out of the desktop view as a head turns through it.
///
/// - `yaw`: how far the world's camera is turned back (radians, about the vertical, round `pivot`).
/// - `speed`: how fast it's turning.
/// - `intent`: the turn the head still means to make. It lets go of the jump over the turn's first
///   moment, and a damped spring pulls the camera after it. So the turn gathers speed rather than
///   starting at full tilt, carries its momentum (a second snap mid-turn adds to it), and lands with
///   a degree or so of overshoot, as a head does.
/// - `dissolve`: seconds into the dissolve from the picture before a move a turn can't show.
#[derive(Clone, Copy, Debug)]
struct Ease {
    yaw: f32,
    speed: f32,
    intent: f32,
    /// The spring's natural frequency (radians a second), from the turn's size.
    omega: f32,
    pivot: Vec3,
    dissolve: Option<f32>,
}

/// The head's damping: under critical, so a 45 degree turn overshoots by about a degree and settles
/// back, and never wobbles.
const HEAD_DAMPING: f32 = 0.7;
/// How far the camera leans into the turn (radians of roll per radian a second): about a degree and
/// a half at a snap's fastest, as a head whipped round dips its ear a little.
const HEAD_LEAN: f32 = 0.0045;

/// How far off the world a bob's move is seen as a turn of it (metres): what a flat game's view
/// shows of a step, a room's walls and the people in it.
const BOB_SEEN_AT: f32 = 2.5;

/// The bob (`bobbed` against `cam`) as a turn of the camera on the world: its own turn, and its move
/// as the turn that moves what's `BOB_SEEN_AT` away as much.
///
/// The game's picture is looked up by direction, so moving the camera never moves the world in it;
/// it only moves the body the runtime draws, the wrong way and into the camera (a landing's 20 cm
/// dip puts the camera in the chest). As a turn of the world's camera, the world bobs and the body
/// stays with the camera, as a flat game's arms do.
fn bob_turn(cam: &Pose, bobbed: &Pose) -> Quat {
    let rot = Quat::from_array(cam.rot).normalize();
    let moved = rot.inverse() * (Vec3::from_array(bobbed.pos) - Vec3::from_array(cam.pos));
    let turned = rot.inverse() * Quat::from_array(bobbed.rot).normalize();
    // (Up looks like the view tipped up, which is a turn about x the negative way; right like it
    // turned right.)
    let seen = Quat::from_rotation_y((moved.x / BOB_SEEN_AT).atan()) * Quat::from_rotation_x(-(moved.y / BOB_SEEN_AT).atan());
    (turned * seen).normalize()
}

impl Ease {
    /// A jump of `yaw` (radians) on top of `was` (none: from rest), settled in about `seconds` for a
    /// 45 degree turn. A head turns further faster and for longer, so a bigger turn takes a little
    /// longer and a smaller one a little less.
    fn jump(was: Option<Ease>, yaw: f32, pivot: Vec3, seconds: f32, dissolve: bool) -> Ease {
        let mut e = was.unwrap_or(Ease { yaw: 0.0, speed: 0.0, intent: 0.0, omega: 1.0, pivot, dissolve: None });
        e.yaw += yaw;
        e.intent += yaw;
        let size = e.yaw.abs().max(e.intent.abs()) / 45f32.to_radians();
        let settle = seconds.max(0.05) * (0.65 + 0.35 * size).clamp(0.5, 1.6);
        e.omega = 4.0 / (HEAD_DAMPING * settle);
        e.pivot = pivot;
        if dissolve {
            e.dissolve = Some(0.0);
        }
        e
    }

    /// On by `dt` (in small steps: the spring is stiff); false once it's done.
    fn step(&mut self, dt: f32, seconds: f32) -> bool {
        let n = (dt * 480.0).ceil().clamp(1.0, 64.0) as usize;
        let h = dt / n as f32;
        // (The intent lets go over an eighth of the settling time.)
        let let_go = (-h * 2.0 * HEAD_DAMPING * self.omega).exp();
        for _ in 0..n {
            self.intent *= let_go;
            let acc = self.omega * self.omega * (self.intent - self.yaw) - 2.0 * HEAD_DAMPING * self.omega * self.speed;
            self.speed += acc * h;
            self.yaw += self.speed * h;
        }
        if let Some(t) = &mut self.dissolve {
            *t += dt;
            if *t >= seconds {
                self.dissolve = None;
            }
        }
        // (Done within a tenth of a degree and a degree a second: the last of it can't be seen.)
        self.yaw.abs() > 2e-3 || self.speed.abs() > 2e-2 || self.intent.abs() > 2e-3 || self.dissolve.is_some()
    }

    /// How much of the dissolve is left (1 at the move, 0 when it's done), eased at both ends.
    fn dissolving(&self, seconds: f32) -> f32 {
        self.dissolve.map_or(0.0, |t| {
            let x = (t / seconds.max(0.01)).clamp(0.0, 1.0);
            1.0 - x * x * (3.0 - 2.0 * x)
        })
    }
}

/// The game's world under the player, picture by picture: its steady turn and walk, and its jumps.
#[derive(Clone, Copy, Debug, Default)]
struct Ground {
    /// The last picture's frame and mapping, and when it came (seconds, the wall's clock: a game
    /// walks on in real time while the runtime's frame is held up).
    last: Option<(u64, WorldMap)>,
    at: f64,
    /// The world's steady turn (radians a second, about the vertical) and walk (metres a second,
    /// tracking space: how a point of the world moves past the player).
    turn: f32,
    walk: Vec3,
    /// The player's speed up and down before this picture (m/s, up positive), for a landing.
    fall: f32,
    ease: Option<Ease>,
}

/// What a new picture says happened to the world.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Nothing new (the same picture), or the world carried on as it was.
    Steady,
    /// A jump the view eases through.
    Jump,
    /// Another place: nothing to ease or blur across.
    Cut,
}

impl Ground {
    /// A picture drawn with `map` (frame `index`), seen at `now` (seconds), with the camera at
    /// `at`. Returns what happened, and the landing's speed (m/s) if the player just stopped
    /// falling.
    fn see(&mut self, index: u64, map: WorldMap, at: Vec3, now: f64) -> (Step, Option<f32>) {
        let Some((last_index, was)) = self.last else {
            (self.last, self.at) = (Some((index, map)), now);
            return (Step::Cut, None);
        };
        if index == last_index {
            return (Step::Steady, None);
        }
        let gap = (now - self.at) as f32;
        (self.last, self.at) = (Some((index, map)), now);
        if gap > CUT_GAP || gap <= 0.0 || (map.world_per_meter - was.world_per_meter).abs() > 1e-4 {
            self.turn = 0.0;
            self.walk = Vec3::ZERO;
            self.ease = None;
            return (Step::Cut, None);
        }
        // Where a point of the world by the camera was against where it is now (tracking space).
        let d = map.matrix().inverse() * was.matrix();
        let moved = d.transform_point3(at) - at;
        let turned = crate::wrap_angle(was.body_yaw - map.body_yaw);
        if moved.length() > CUT_MOVE {
            self.turn = 0.0;
            self.walk = Vec3::ZERO;
            self.ease = None;
            return (Step::Cut, None);
        }
        // A jump goes further than the steady turn and walk would have, by more than they could
        // change in a picture (stopping short, a landing, isn't one).
        let (turn, walk) = (self.turn * gap, self.walk * gap);
        let flat = |v: Vec3| Vec2::new(v.x, v.z);
        // A hitch (the game's steps held up, then taken all at once, a picture or two later) goes
        // the way the world was already going: it's carried on, not eased.
        let held = gap + HITCH;
        let hitch_turn = self.turn.abs() > 0.5 && turned * self.turn > 0.0 && turned.abs() < self.turn.abs() * held;
        let hitch_walk = flat(self.walk).length() > 1.0 && flat(moved).normalize_or_zero().dot(flat(self.walk).normalize_or_zero()) > 0.95 && flat(moved).length() < flat(self.walk).length() * held;
        let jumped = ((turned - turn).abs() > JUMP_TURN && turned.abs() > turn.abs() + JUMP_TURN && !hitch_turn)
            || (flat(moved - walk).length() > JUMP_MOVE && flat(moved).length() > flat(walk).length() + JUMP_MOVE && !hitch_walk)
            || ((moved.y - walk.y).abs() > JUMP_RISE && moved.y.abs() > walk.y.abs() + JUMP_RISE);
        if jumped {
            return (Step::Jump, None);
        }
        if hitch_turn && (turned - turn).abs() > JUMP_TURN || hitch_walk && flat(moved - walk).length() > JUMP_MOVE {
            // (Its time isn't the picture's: the speeds are left as they were.)
            return (Step::Steady, None);
        }
        let k = 0.5;
        self.turn += (turned / gap - self.turn) * k;
        let walk = moved / gap;
        // (A point of the world rising past the player is the player falling.)
        let fall = -walk.y;
        let landed = (self.fall < -5.0 && fall > -1.0).then_some(-self.fall);
        self.fall = fall;
        self.walk += (walk - self.walk) * k;
        (Step::Steady, landed)
    }
}

/// The camera's shake: a trauma events add to and that wears off, shaking by its square; kicks
/// that spring back; a handheld sway.
#[derive(Clone, Copy, Debug, Default)]
struct Shake {
    trauma: f32,
    /// The kick (degrees: pitch, yaw, roll) and how fast it's moving.
    kick: Vec3,
    kick_vel: Vec3,
    t: f32,
}

/// The most a full trauma turns the camera (degrees: pitch, yaw, roll) and how fast it wears off
/// (a second from full to none).
const SHAKE_MAX: Vec3 = Vec3::new(2.2, 1.8, 2.8);
const TRAUMA_WEAR: f32 = 1.0;

impl Shake {
    fn add(&mut self, trauma: f32, kick: Vec3) {
        self.trauma = (self.trauma + trauma.max(0.0)).min(1.0);
        self.kick_vel += kick * 30.0;
    }

    /// This frame's turn of the camera (degrees: pitch, yaw, roll).
    fn update(&mut self, dt: f32, strength: f32, handheld: f32) -> Vec3 {
        let dt = dt.clamp(0.0, 0.1);
        self.t = (self.t + dt) % 1000.0;
        self.trauma = (self.trauma - TRAUMA_WEAR * dt).max(0.0);
        // A spring, damped just enough not to wobble: a kick and back in about a fifth of a
        // second.
        let w = 18.0;
        let acc = -self.kick * w * w - self.kick_vel * 2.0 * w;
        self.kick_vel += acc * dt;
        self.kick += self.kick_vel * dt;
        let t = self.t;
        // Smooth noise from sines at frequencies that never line up: fast for a shake, slow for a
        // hand's sway.
        let n = |a: f32, b: f32, c: f32, s: f32| (t * a + s).sin() * 0.5 + (t * b + 2.0 * s).sin() * 0.3 + (t * c + 3.0 * s).sin() * 0.2;
        let fast = Vec3::new(n(71.0, 113.0, 173.0, 0.0), n(67.0, 109.0, 167.0, 1.7), n(61.0, 101.0, 157.0, 3.1));
        let slow = Vec3::new(n(1.9, 3.3, 5.5, 0.4), n(1.3, 2.9, 4.7, 2.2), n(1.1, 2.3, 3.9, 4.0));
        let k = self.trauma * self.trauma;
        (fast * SHAKE_MAX * k + self.kick) * strength + slow * Vec3::new(0.5, 0.7, 0.45) * handheld
    }
}

/// A camera worn on the chest: turned after the head lazily, a little down, leaning into turns.
#[derive(Clone, Copy, Debug, Default)]
struct Chest {
    yaw: Option<f32>,
    rate: f32,
}

/// How far behind the head a chest-worn camera may turn (radians): the game's pictures are drawn
/// round the head, and further would show past their edges.
const CHEST_LAG: f32 = 0.24;

impl Chest {
    fn update(&mut self, head: Quat, dt: f32) -> Quat {
        let f = head * Vec3::Z;
        let head_yaw = f.x.atan2(f.z);
        let head_pitch = (-f.y).atan2((f.x * f.x + f.z * f.z).sqrt());
        let yaw = self.yaw.get_or_insert(head_yaw);
        let was = *yaw;
        // (A third of a second to turn half way after the body.)
        let k = 1.0 - 0.5f32.powf(dt.max(0.0) / 0.3);
        let d = crate::wrap_angle(head_yaw - *yaw);
        *yaw += d * k;
        let behind = crate::wrap_angle(head_yaw - *yaw);
        if behind.abs() > CHEST_LAG {
            *yaw = head_yaw - behind.signum() * CHEST_LAG;
        }
        if dt > 0.0 {
            self.rate += (crate::wrap_angle(*yaw - was) / dt - self.rate) * 0.2;
        }
        // Down a little (a chest's camera looks over the hands), nodding half as much as the head.
        let pitch = (head_pitch * 0.55 + 4f32.to_radians()).clamp(head_pitch - CHEST_LAG, head_pitch + CHEST_LAG);
        let roll = (-self.rate * 0.04).clamp(-0.05, 0.05);
        Quat::from_rotation_y(*yaw) * Quat::from_rotation_x(pitch) * Quat::from_rotation_z(roll)
    }
}

/// What the camera shows this frame ([`Stage::shot`]).
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    /// The camera for everything the runtime draws (the body, what it holds, panels).
    pub cam: Pose,
    /// The camera for the game's world: `cam` turned through a jump that's being eased out.
    pub world: Pose,
    pub fov: Fov,
}

/// What [`Stage::shot`] takes.
pub struct ShotInput<'a> {
    pub settings: &'a Settings,
    /// The steadied camera and its field of view, as the desktop view has them.
    pub cam: Pose,
    /// The same camera bobbed as the player walks (`crate::view_bob`).
    pub bobbed: Pose,
    pub fov: Fov,
    pub frame: &'a HmdFrame,
    /// The picture the view shows: its slot info and the eye it's seen from (none: no game
    /// picture).
    pub picture: Option<(&'a SlotInfo, usize)>,
    /// The hands as the rig has them (none: the controllers).
    pub rig: Option<&'a RigOutput>,
    /// This frame's feedback (`Rig::feedback`), when the rig ran this frame.
    pub feedback: &'a [Feedback],
    pub weapon: Option<&'a WeaponPose>,
}

/// The game's HUD for [`Stage::background`] and [`Stage::drawn`]: its picture, what's in it, and
/// its shape (width / height).
pub struct FlatHud<'a> {
    pub tex: &'a Texture,
    pub layout: &'a HudLayout,
    pub aspect: f32,
}

/// One piece of the flat HUD: where on the window (pixels: x0, y0, x1, y1) and its rectangle of
/// the HUD's picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlatPiece {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
}

/// What a flat game's text is tall on its screen (a share of the screen's height: Oblivion's
/// ordinary text, 36 of its 960 menu units), for sizing the flat HUD from the game's own.
const FLAT_TEXT: f32 = 0.0375;

/// The game's HUD laid out as a flat player has it, in `area` of the window (pixels: x0, y0, x1,
/// y1): each of its pieces (but those it hides, the crosshair, as the hands
/// aim, and a held thing's card, which floats over the thing) anchored to the side of the screen
/// it's nearest, or its middle, as far from that side as the game has it, at the size the game's
/// text has on a flat screen (`size` times). Without pieces, the whole HUD over the middle,
/// fitted by height. `aspect`: the HUD picture's width / height.
pub fn flat_layout(layout: &HudLayout, aspect: f32, area: [f32; 4], size: f32) -> Vec<FlatPiece> {
    let (x0, y0, w, h) = (area[0], area[1], area[2] - area[0], area[3] - area[1]);
    let whole = [0.0, 0.0, 1.0, 1.0];
    if layout.count == 0 {
        if layout.flags & HUDL_REST_HIDDEN != 0 {
            return Vec::new();
        }
        let width = aspect * h;
        return vec![FlatPiece { rect: [x0 + (w - width) * 0.5, y0, x0 + (w + width) * 0.5, y0 + h], uv: whole }];
    }
    // The HUD's height on the window: its text as tall as a flat game's.
    let k = size * if layout.text_height > 0.0 { FLAT_TEXT / layout.text_height } else { 1.0 };
    let (hud_w, hud_h) = (aspect * h * k, h * k);
    let along = |c: f32, length: f32, start: f32, extent: f32| {
        if c < 1.0 / 3.0 {
            start + c * length
        } else if c > 2.0 / 3.0 {
            start + extent - (1.0 - c) * length
        } else {
            start + extent * 0.5 + (c - 0.5) * length
        }
    };
    let mut out = Vec::new();
    if layout.flags & HUDL_REST_HIDDEN == 0 {
        out.push(FlatPiece { rect: [x0 + (w - hud_w) * 0.5, y0 + (h - hud_h) * 0.5, x0 + (w + hud_w) * 0.5, y0 + (h + hud_h) * 0.5], uv: whole });
    }
    for e in layout.entries().iter().filter(|e| e.group == 0 && !matches!(e.role, HUD_HIDDEN | HUD_HELD | HUD_MENU)) {
        let (du, dv) = (e.uv[2] - e.uv[0], e.uv[3] - e.uv[1]);
        if du <= 0.0 || dv <= 0.0 {
            continue;
        }
        // (Something over the whole screen, a damage flash, stays over the whole picture.)
        if du > 0.9 && dv > 0.9 {
            out.push(FlatPiece { rect: [x0, y0, x0 + w, y0 + h], uv: e.uv });
            continue;
        }
        let cx = along((e.uv[0] + e.uv[2]) * 0.5, hud_w, x0, w);
        let cy = along((e.uv[1] + e.uv[3]) * 0.5, hud_h, y0, h);
        let (pw, ph) = (du * hud_w * 0.5, dv * hud_h * 0.5);
        out.push(FlatPiece { rect: [cx - pw, cy - ph, cx + pw, cy + ph], uv: e.uv });
    }
    out
}

/// Whether a panel of the desktop view's layers is one of the HUD's pieces the flat HUD draws
/// instead: its picture the HUD's, its rectangle one of the pieces'.
fn is_flat_piece(srv: &ID3D11ShaderResourceView, uv: &[f32; 4], hud: &ID3D11ShaderResourceView, pieces: &[FlatPiece]) -> bool {
    srv.as_raw() == hud.as_raw() && pieces.iter().any(|p| p.uv == *uv)
}

// ---- the GPU ------------------------------------------------------------------------------------

const SHADER: &str = r#"
static const float NEAR = 0.03;
static const float FAR = 200.0;

cbuffer Fx : register(b0) {
    float4x4 vp;            // the camera: tracking -> clip
    float4x4 inv_vp;
    float4x4 prev_vp;       // last frame's camera
    float4x4 world_inv_vp;  // the world's camera (turned through a jump being eased): clip -> tracking
    float4x4 world_prev;    // a point of the world now -> where last frame's world camera saw it (clip)
    float4x4 hand_prev[2];  // a point a hand carries now -> where it was last frame
    float4x4 eye_a;         // the eyes' pictures (a direction -> clip), and the one from before a jump
    float4x4 eye_b;
    float4x4 hist_vp;
    float4 world_cam;       // xyz the world's camera; w metres per unit of the game's depth
    float4 world_fwd;       // xyz its forward; w 1: the game's depth is there
    float4 body_axis;       // xyz the top of the body's axis (the neck); w 1: the body's depth is there
    float4 hands[2];        // xyz each hand; w 1: it was there last frame too
    float4 size;            // xy pixels, zw their inverses
    float4 blur;            // x the shutter's share of this frame's motion, y the camera's blur, z its middle kept sharp, w the hands' blur
    float4 tiles;           // x a tile (pixels: the longest blur), y the tangent of half the view across, zw tiles across and down
    float4 dof;             // x the farthest blur (radius, pixels), y in front (share), z the focus's pull this frame, w a focus given (m; 0: from the depth)
    float4 focus_uv;        // xy where to focus; z 1: depth of field
    float4 lens;            // x fisheye, y colour fringes, z vignette, w sharpen
    float4 film;            // x grain, y seconds, z glow, w 1: glow
    float4 colour;          // x exposure, y contrast, z saturation, w the LUT's share
    float4 white;           // xyz white balance, w the grade's share
    float4 grade;           // x which grade, y the LUT's size
    float4 fill;            // x 1: the picture from before a jump, y the dissolve's share, z 1: stretch the edges, w 1: a second eye
};
cbuffer Piece : register(b1) {
    float4 piece_rect;      // clip x0 y0 x1 y1
    float4 piece_uv;
    float4 piece_mode;      // x 1: added (no alpha)
};
Texture2D t0 : register(t0);
Texture2D t1 : register(t1);
Texture2D t2 : register(t2);
Texture2D t3 : register(t3);
Texture2D t4 : register(t4);
Texture2D t5 : register(t5);
SamplerState lin : register(s0);
SamplerState pt : register(s1);

struct VOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };

VOut VSFull(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VOut o;
    o.pos = float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
    o.uv = uv;
    return o;
}

float2 ndc_of(float2 uv) { return float2(uv.x * 2 - 1, 1 - uv.y * 2); }
float2 uv_of(float2 n) { return float2(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5); }
float2 px_of(float4 c) { return uv_of(c.xy / c.w) * size.xy; }
float3 ray(float4x4 inv, float2 uv) {
    float4 a = mul(inv, float4(ndc_of(uv), 0, 1)); a /= a.w;
    float4 b = mul(inv, float4(ndc_of(uv), 1, 1)); b /= b.w;
    return normalize(b.xyz - a.xyz);
}
float luma(float3 c) { return dot(c, float3(0.2126, 0.7152, 0.0722)); }
float hash(float2 p) { return frac(sin(dot(p, float2(12.9898, 78.233))) * 43758.5453); }
// How far out from the middle of the view a point is: 0 the middle, 1 a corner.
float out_from_middle(float2 uv) {
    float aspect = size.x * size.w;
    return length((uv - 0.5) * float2(aspect, 1)) / (0.5 * sqrt(aspect * aspect + 1));
}

// ---- motion

// How much of a point of the body moves with a hand: none on the body's axis (the torso, the legs
// under it, the shoulders), all of it half a metre out (a hand, a forearm, a sword held out).
float carried(float3 p) {
    float3 q = float3(body_axis.x, clamp(p.y, 0, body_axis.y), body_axis.z);
    return smoothstep(0.22, 0.5, length(p - q));
}

// Each pixel's motion over the shutter (pixels), its distance (m; 0: as far as can be) and
// whether the runtime drew it. The camera's own part is stronger towards the edges.
float4 PSVelocity(VOut i) : SV_Target {
    int2 px = int2(i.pos.xy);
    float zb = body_axis.w > 0 ? t0.Load(int3(px, 0)).r : 1;
    float zg = world_fwd.w > 0 ? t1.Load(int3(px, 0)).r : 0;
    float body_m = zb < 1 ? NEAR / (1 - zb * (FAR - NEAR) / FAR) : 1e9;
    float game_m = zg > 0 ? zg * world_cam.w : 1e9;
    float edge = lerp(1 - blur.z, 1, smoothstep(0.1, 0.85, out_from_middle(i.uv)));
    float2 v;
    float m;
    float mask = 0;
    if (body_m < game_m) {
        float4 p4 = mul(inv_vp, float4(ndc_of(i.uv), zb, 1));
        float3 p = p4.xyz / p4.w;
        float wa = hands[0].w / (dot(p - hands[0].xyz, p - hands[0].xyz) + 0.01);
        float wb = hands[1].w / (dot(p - hands[1].xyz, p - hands[1].xyz) + 0.01);
        float3 moved = wa * mul(hand_prev[0], float4(p, 1)).xyz + wb * mul(hand_prev[1], float4(p, 1)).xyz;
        float3 then = wa + wb > 0 ? lerp(p, moved / (wa + wb), carried(p)) : p;
        float2 cam = px_of(mul(prev_vp, float4(p, 1)));
        float2 all = px_of(mul(prev_vp, float4(then, 1)));
        v = (i.pos.xy - cam) * blur.y * edge + (cam - all) * blur.w;
        m = body_m;
        mask = 1;
    } else {
        float3 d = ray(world_inv_vp, i.uv);
        float4 c = game_m < 1e8 ? mul(world_prev, float4(world_cam.xyz + d * game_m / max(dot(d, world_fwd.xyz), 1e-3), 1)) : mul(world_prev, float4(d, 0));
        v = c.w > 1e-5 ? (i.pos.xy - px_of(c)) * blur.y * edge : 0;
        m = game_m < 1e8 ? game_m : 0;
    }
    v *= blur.x;
    float l = length(v);
    if (l > tiles.x) v *= tiles.x / l;
    return float4(v, m, mask);
}

// The longest motion in each tile (every other pixel: a blade's a few pixels wide at least).
float2 PSTileMax(VOut i) : SV_Target {
    int n = (int)tiles.x;
    int2 at = int2(i.pos.xy) * n;
    int2 last = int2(size.xy) - 1;
    float2 best = 0;
    float most = 0;
    [loop] for (int y = 0; y < n; y += 2) {
        [loop] for (int x = 0; x < n; x += 2) {
            float2 v = t1.Load(int3(min(at + int2(x, y), last), 0)).xy;
            float l = dot(v, v);
            if (l > most) { most = l; best = v; }
        }
    }
    return best;
}

// ... and in each tile's neighbourhood: a blur reaches across a tile's edge.
float2 PSNeighbourMax(VOut i) : SV_Target {
    int2 at = int2(i.pos.xy);
    int2 last = int2(tiles.zw) - 1;
    float2 best = 0;
    float most = 0;
    [unroll] for (int y = -1; y <= 1; y++) {
        [unroll] for (int x = -1; x <= 1; x++) {
            float2 v = t2.Load(int3(clamp(at + int2(x, y), 0, last), 0)).xy;
            float l = dot(v, v);
            if (l > most) { most = l; best = v; }
        }
    }
    return best;
}

// McGuire et al.'s reconstruction (2012): each pixel gathers along its neighbourhood's longest
// motion, each sample weighed by whether it's in front and its blur reaches here, or it's behind
// and this pixel's blur uncovers it.
float far_of(float m) { return m > 0 ? m : 1e4; }
float in_front(float za, float zb) { return saturate(1 - (za - zb) / (0.03 + 0.05 * zb)); }
float cone(float dist, float reach) { return saturate(1 - dist / reach); }
float cylinder(float dist, float reach) { return 1 - smoothstep(0.95 * reach, 1.05 * reach, dist); }
float4 PSMotionBlur(VOut i) : SV_Target {
    int2 px = int2(i.pos.xy);
    float2 vn = t2.Load(int3(px / int(tiles.x), 0)).xy;
    float3 c0 = t0.Load(int3(px, 0)).rgb;
    if (dot(vn, vn) < 0.25) return float4(c0, 1);
    float4 x = t1.Load(int3(px, 0));
    float zx = far_of(x.z);
    float rx = max(length(x.xy) * 0.5, 0.5);
    float total = 1 / rx;
    float3 sum = c0 * total;
    float j = hash(i.pos.xy + frac(film.y)) - 0.5;
    int2 last = int2(size.xy) - 1;
    const int N = 14;
    [loop] for (int k = 0; k < N; k++) {
        float t = ((k + 0.5 + j) / N) - 0.5;
        float2 off = vn * t;
        int2 sp = clamp(int2(i.pos.xy + off), 0, last);
        float4 y = t1.Load(int3(sp, 0));
        float zy = far_of(y.z);
        float ry = max(length(y.xy) * 0.5, 0.5);
        float dist = length(off);
        float w = in_front(zy, zx) * cone(dist, ry) + in_front(zx, zy) * cone(dist, rx) + cylinder(dist, ry) * cylinder(dist, rx) * 2;
        sum += w * t0.Load(int3(sp, 0)).rgb;
        total += w;
    }
    return float4(sum / total, 1);
}

// ---- focus

float depth_at(float2 uv) {
    float m = t1.Load(int3(clamp(uv, 0, 1) * (size.xy - 1), 0)).z;
    return m > 0 ? m : 1000;
}
// The distance to focus at, eased from last frame's: a distance given (what a hand holds up), or
// the middle of five depths round the point looked or aimed at (a gap between two things doesn't
// throw it to the background).
float PSFocus(VOut i) : SV_Target {
    float want = dof.w;
    if (want <= 0) {
        float2 o = float2(0.02 * size.y * size.z, 0.02);
        float a = depth_at(focus_uv.xy), b = depth_at(focus_uv.xy + float2(o.x, 0)), c = depth_at(focus_uv.xy - float2(o.x, 0));
        float d = depth_at(focus_uv.xy + float2(0, o.y)), e = depth_at(focus_uv.xy - float2(0, o.y));
        float lo = min(min(a, b), min(c, min(d, e)));
        float hi = max(max(a, b), max(c, max(d, e)));
        want = (a + b + c + d + e - lo - hi) / 3;
    }
    float was = t3.Load(int3(0, 0, 0)).r;
    if (!(was > 0.01) || !(was < 1e6)) return want;
    return exp(lerp(log(was), log(want), dof.z));
}

// A distance's blur (pixels, a radius at full size): behind the focus as a lens has it (focused at
// 2 m, infinity blurs the most), in front its share.
float blur_of(float m, float focus) {
    m = m > 0 ? m : 1000;
    float c = (1 / max(focus, 0.05) - 1 / m) * 2 * dof.x;
    return c >= 0 ? min(c, dof.x) : max(c * dof.y, -dof.x);
}

// Half size: the picture (four pixels' worth) and its blur (half-size pixels; in front negative),
// from the nearest of the four depths, so an edge in front keeps its blur.
float4 PSDofDown(VOut i) : SV_Target {
    int2 p = int2(i.pos.xy) * 2;
    int2 last = int2(size.xy) - 1;
    float focus = t3.Load(int3(0, 0, 0)).r;
    float3 c = t0.SampleLevel(lin, i.uv, 0).rgb;
    float z = min(min(far_of(t1.Load(int3(min(p, last), 0)).z), far_of(t1.Load(int3(min(p + int2(1, 0), last), 0)).z)),
                  min(far_of(t1.Load(int3(min(p + int2(0, 1), last), 0)).z), far_of(t1.Load(int3(min(p + 1, last), 0)).z)));
    return float4(c, blur_of(z, focus) * 0.5);
}

// The blur: a disc of samples as wide as this pixel's blur, or as far as a blur in front reaches
// over it. A sample counts where its own blur reaches here (a sharp thing doesn't smear onto the
// blurred background behind it). Its alpha: how blurred the result is, for the final pass.
float4 PSDofBlur(VOut i) : SV_Target {
    float2 texel = 2 * size.zw;
    float4 c = t0.SampleLevel(pt, i.uv, 0);
    float most = dof.x * 0.5;
    float near = max(-c.a, 0);
    [unroll] for (int r = 0; r < 8; r++) {
        float ang = r * 0.785398;
        [unroll] for (int ring = 1; ring <= 2; ring++) {
            float dist = most * ring * 0.5;
            float s = t0.SampleLevel(pt, i.uv + float2(cos(ang), sin(ang)) * dist * texel, 0).a;
            if (-s >= dist) near = max(near, -s);
        }
    }
    float reach = max(abs(c.a), near);
    if (reach < 0.5) return float4(c.rgb, 0);
    float3 sum = c.rgb;
    float total = 1;
    float covered = 0;
    float turn = hash(i.pos.xy) * 6.2832;
    const int N = 24;
    [loop] for (int k = 0; k < N; k++) {
        float dist = sqrt((k + 0.5) / N) * reach;
        float ang = k * 2.39996 + turn;
        float4 s = t0.SampleLevel(lin, i.uv + float2(cos(ang), sin(ang)) * dist * texel, 0);
        float w = saturate(abs(s.a) - dist + 1);
        if (s.a < 0) covered += w;
        sum += s.rgb * w;
        total += w;
    }
    return float4(sum / total, saturate(max(abs(c.a) * 2 - 0.5, covered * 3 / N)));
}

// ---- glow

// A quarter size, only what's bright (a soft knee): highlights bloom as film's halate.
float4 PSGlowDown(VOut i) : SV_Target {
    float2 o = size.zw;
    float3 c = (t0.SampleLevel(lin, i.uv + o * float2(-1, -1), 0).rgb + t0.SampleLevel(lin, i.uv + o * float2(1, -1), 0).rgb
              + t0.SampleLevel(lin, i.uv + o * float2(-1, 1), 0).rgb + t0.SampleLevel(lin, i.uv + o * float2(1, 1), 0).rgb) * 0.25;
    float l = luma(c);
    return float4(c * smoothstep(0.45, 0.95, l), 1);
}
float3 gauss(float2 uv, float2 step) {
    float3 c = t0.SampleLevel(lin, uv, 0).rgb * 0.2270;
    c += (t0.SampleLevel(lin, uv + step * 1.3846, 0).rgb + t0.SampleLevel(lin, uv - step * 1.3846, 0).rgb) * 0.3162;
    c += (t0.SampleLevel(lin, uv + step * 3.2308, 0).rgb + t0.SampleLevel(lin, uv - step * 3.2308, 0).rgb) * 0.0703;
    return c;
}
float4 PSGlowAcross(VOut i) : SV_Target { return float4(gauss(i.uv, float2(4 * size.z * 1.5, 0)), 1); }
float4 PSGlowDownwards(VOut i) : SV_Target { return float4(gauss(i.uv, float2(0, 4 * size.w * 1.5)), 1); }

// ---- the lens and the film

float3 picture(float2 uv) {
    float3 c = t0.SampleLevel(lin, uv, 0).rgb;
    if (focus_uv.z > 0) {
        float4 d = t1.SampleLevel(lin, uv, 0);
        c = lerp(c, d.rgb, d.a);
    }
    return c;
}
float3 to_srgb(float3 c) { return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1 / 2.4) - 0.055; }
float3 from_srgb(float3 e) { return e <= 0.04045 ? e / 12.92 : pow((e + 0.055) / 1.055, 2.4); }
// The grades, on the picture as it's seen (sRGB's encoding), where graders work.
float3 graded(float3 e, float id) {
    float y = luma(e);
    if (id < 1.5) {
        // Teal and orange: the shadows towards teal, the highlights (and skin) towards orange.
        float s = saturate(1 - y * 2), h = saturate(y * 2 - 1);
        float3 o = e + float3(-0.5, 0.05, 0.1) * 0.18 * s + float3(0.5, 0.1, -0.2) * 0.14 * h;
        float warm = saturate((e.r - e.b) * 3) * saturate(1 - abs(y - 0.55) * 2.5);
        return lerp(o, o * float3(1.04, 1.0, 0.94), warm);
    }
    if (id < 2.5) {
        // Bleach bypass: the picture overlaid with its own black and white, half its colour left.
        float3 o = y < 0.5 ? 2 * e * y : 1 - 2 * (1 - e) * (1 - y);
        return lerp(y.xxx, o, 0.55);
    }
    if (id < 3.5) {
        // Warm print film: lifted blacks, golden highlights, a soft S.
        float3 o = (0.035 + e * 0.95) * float3(1.04, 1.0, 0.88);
        return lerp(o, smoothstep(0, 1, o), 0.3);
    }
    if (id < 4.5) {
        // A body camera's cheap sensor: less colour, cyan in the shadows, a little crushed.
        float3 o = lerp(y.xxx, e, 0.72) + float3(-0.01, 0.012, 0.03) * (1 - y);
        return lerp(o, smoothstep(0.02, 1, o), 0.3);
    }
    // Noir: black and white through a red filter (dark skies, light skin), hard.
    float bw = dot(e, float3(0.55, 0.35, 0.10));
    return smoothstep(0.03, 0.97, bw).xxx;
}
// A 3D LUT laid out as its blue slices side by side.
float3 lut(float3 e) {
    float n = grade.y;
    float b = saturate(e.b) * (n - 1);
    float b0 = floor(b);
    float2 uv = float2((saturate(e.r) * (n - 1) + 0.5) / (n * n), (saturate(e.g) * (n - 1) + 0.5) / n);
    float3 a = t5.SampleLevel(lin, uv + float2(b0 / n, 0), 0).rgb;
    float3 c = t5.SampleLevel(lin, uv + float2(min(b0 + 1, n - 1) / n, 0), 0).rgb;
    return lerp(a, c, b - b0);
}
float4 PSFinal(VOut i) : SV_Target {
    float2 uv = i.uv;
    // The lens: from the middle (the right edge at 1) out, bent towards a fisheye's (equal angles
    // at equal distances) keeping the corners where they were.
    float aspect = size.x * size.w;
    float2 q = (uv - 0.5) * 2 * float2(1, 1 / aspect);
    float corner = length(float2(1, 1 / aspect));
    float rho = length(q);
    float2 src = q;
    if (lens.x > 0 && rho > 1e-5) {
        float far_angle = atan(corner * tiles.y);
        float r = lerp(rho, tan(rho / corner * far_angle) / tiles.y, lens.x);
        src = q * (r / rho);
    }
    float2 s = src * float2(1, aspect) * 0.5 + 0.5;
    float out_ = rho / corner;
    // Colour fringes towards the corners: red a little further out, blue a little further in.
    float k = lens.y * 0.012 * out_ * out_;
    float3 c = k > 0 ? float3(picture(0.5 + (s - 0.5) * (1 + k)).r, picture(s).g, picture(0.5 + (s - 0.5) * (1 - k)).b) : picture(s);
    // Sharpening, contrast-adaptive (where there's little detail, more; never on what's blurred).
    if (lens.w > 0) {
        float3 n = t0.SampleLevel(lin, s + float2(size.z, 0), 0).rgb + t0.SampleLevel(lin, s - float2(size.z, 0), 0).rgb
                 + t0.SampleLevel(lin, s + float2(0, size.w), 0).rgb + t0.SampleLevel(lin, s - float2(0, size.w), 0).rgb;
        float3 mid = t0.SampleLevel(lin, s, 0).rgb;
        float detail = abs(luma(mid) - luma(n) * 0.25);
        float amount = lens.w * 1.5 * saturate(1 - detail * 4);
        if (focus_uv.z > 0) amount *= 1 - t1.SampleLevel(lin, s, 0).a;
        c += (mid - n * 0.25) * amount;
    }
    c = max(c, 0) * colour.x * white.rgb;
    if (film.w > 0) c += t4.SampleLevel(lin, s, 0).rgb * film.z * float3(1.0, 0.82, 0.68);
    // Contrast about middle grey, in stops; saturation.
    c = 0.18 * pow(max(c / 0.18, 1e-6), colour.y);
    c = max(lerp(luma(c).xxx, c, colour.z), 0);
    // Past 0.8 the highlights roll off to white rather than clip.
    c = c < 0.8 ? c : 0.8 + 0.2 * (1 - exp(-(c - 0.8) / 0.2));
    float3 e = to_srgb(saturate(c));
    if (grade.x > 0) e = lerp(e, saturate(graded(e, grade.x)), white.w);
    if (colour.w > 0) e = lerp(e, lut(e), colour.w);
    // The vignette: a lens's natural fall-off towards the corners.
    float v = cos(min(out_ * 0.95, 1.5));
    e *= lerp(1, v * v * v * v, lens.z);
    // Grain, strongest in the middle tones, as film's.
    if (film.x > 0) {
        float2 g = i.pos.xy + frac(film.y * 7.13) * 512;
        float n = hash(g) + hash(g * 1.31 + 17.7) - 1;
        float y = luma(e);
        e += n * film.x * 0.12 * (0.25 + 3 * y * (1 - y));
    }
    return float4(from_srgb(saturate(e)), 1);
}

// ---- the jump, and the edges

// Where the eyes' pictures don't reach (each pixel's direction from the world's camera): mid-jump,
// the picture from before it (the world's turn since undone), and over the rest fading out for a
// move a turn can't show; past that, the edge of the nearer eye's picture drawn out over the gap,
// softened along the edge the further out it goes: a smear, never black.
float4 PSFill(VOut i) : SV_Target {
    float3 d = ray(world_inv_vp, i.uv);
    float4 a = mul(eye_a, float4(d, 0));
    float4 b = mul(eye_b, float4(d, 0));
    float2 na = a.xy / max(a.w, 1e-6), nb = b.xy / max(b.w, 1e-6);
    // How far outside each picture (0: inside it); a picture behind doesn't count.
    float oa = a.w > 0 ? max(max(abs(na.x), abs(na.y)) - 1, 0) : 1e6;
    float ob = b.w > 0 && fill.w > 0 ? max(max(abs(nb.x), abs(nb.y)) - 1, 0) : 1e6;
    bool seen = oa == 0 || ob == 0;
    if (fill.x > 0) {
        float alpha = seen ? fill.y : 1;
        float4 h = mul(hist_vp, float4(d, 0));
        if (alpha > 0 && h.w > 0 && all(abs(h.xy / h.w) <= 1)) return float4(t0.SampleLevel(lin, uv_of(h.xy / h.w), 0).rgb, alpha);
    }
    if (seen || fill.z <= 0 || min(oa, ob) > 1e5) discard;
    bool use_a = oa <= ob;
    float2 n = use_a ? na : nb;
    float past = use_a ? oa : ob;
    float2 edge = clamp(n, -0.998, 0.998);
    float2 along = abs(n.x) > abs(n.y) ? float2(0, 1) : float2(1, 0);
    float spread = min(past * 0.6, 0.2);
    float3 c = 0;
    [unroll] for (int k = -3; k <= 3; k++) {
        float2 m = uv_of(clamp(edge + along * spread * k / 3.0, -0.998, 0.998));
        c += use_a ? t1.SampleLevel(lin, m, 0).rgb : t2.SampleLevel(lin, m, 0).rgb;
    }
    return float4(c / 7, 1);
}

// ---- the flat HUD

VOut VSPiece(uint id : SV_VertexID) {
    float2 c = float2(id & 1, id >> 1);
    VOut o;
    o.pos = float4(lerp(piece_rect.xy, piece_rect.zw, c), 0, 1);
    o.uv = lerp(piece_uv.xy, piece_uv.zw, c);
    return o;
}
// The HUD's picture over its footprint (it's seen smaller than it was drawn: its text mustn't
// shimmer), premultiplied, or added.
float4 PSPiece(VOut i) : SV_Target {
    float2 dx = ddx(i.uv), dy = ddy(i.uv);
    uint w, h;
    t0.GetDimensions(w, h);
    float n = clamp(ceil(max(length(dx * float2(w, h)), length(dy * float2(w, h)))), 1, 3);
    float4 acc = 0;
    [loop] for (float a = 0; a < n; a++) {
        [loop] for (float b = 0; b < n; b++) {
            acc += t0.SampleLevel(lin, i.uv + dx * ((a + 0.5) / n - 0.5) + dy * ((b + 0.5) / n - 0.5), 0);
        }
    }
    acc /= n * n;
    if (piece_mode.x > 0.5) acc.a = 0;
    return acc;
}
"#;

/// The pixel shaders, by entry point, in [`Kit`]'s order.
const PASSES: [&str; 12] = [
    "PSVelocity",
    "PSTileMax",
    "PSNeighbourMax",
    "PSMotionBlur",
    "PSFocus",
    "PSDofDown",
    "PSDofBlur",
    "PSGlowDown",
    "PSGlowAcross",
    "PSGlowDownwards",
    "PSFinal",
    "PSFill",
];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FxCb {
    vp: [f32; 16],
    inv_vp: [f32; 16],
    prev_vp: [f32; 16],
    world_inv_vp: [f32; 16],
    world_prev: [f32; 16],
    hand_prev: [[f32; 16]; 2],
    eye_a: [f32; 16],
    eye_b: [f32; 16],
    hist_vp: [f32; 16],
    world_cam: [f32; 4],
    world_fwd: [f32; 4],
    body_axis: [f32; 4],
    hands: [[f32; 4]; 2],
    size: [f32; 4],
    blur: [f32; 4],
    tiles: [f32; 4],
    dof: [f32; 4],
    focus_uv: [f32; 4],
    lens: [f32; 4],
    film: [f32; 4],
    colour: [f32; 4],
    white: [f32; 4],
    grade: [f32; 4],
    fill: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PieceCb {
    rect: [f32; 4],
    uv: [f32; 4],
    mode: [f32; 4],
}

/// The compiled shaders and the states they're drawn with.
struct Kit {
    passes: Vec<Program>,
    piece: Program,
    cb: ConstantBuffer<FxCb>,
    piece_cb: ConstantBuffer<PieceCb>,
    opaque: ID3D11BlendState,
    alpha: ID3D11BlendState,
    premul: ID3D11BlendState,
    additive: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    no_depth: ID3D11DepthStencilState,
    linear: ID3D11SamplerState,
    point: ID3D11SamplerState,
}

impl Kit {
    fn new(gpu: &Gpu, code: &Compiled) -> Result<Kit> {
        let shader = |vs: &[u8], ps: &[u8]| -> Result<Program> {
            unsafe {
                let (mut v, mut p) = (None, None);
                gpu.device.CreateVertexShader(vs, None, Some(&mut v))?;
                gpu.device.CreatePixelShader(ps, None, Some(&mut p))?;
                Ok(Program { vs: v.context("vertex shader")?, ps: p.context("pixel shader")?, layout: None })
            }
        };
        let additive = unsafe {
            let mut d = D3D11_BLEND_DESC::default();
            d.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_ONE,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ZERO,
                DestBlendAlpha: D3D11_BLEND_ONE,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut s = None;
            gpu.device.CreateBlendState(&d, Some(&mut s))?;
            s.context("blend state")?
        };
        let point = unsafe {
            let d = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut s = None;
            gpu.device.CreateSamplerState(&d, Some(&mut s))?;
            s.context("sampler")?
        };
        let linear = unsafe {
            let d = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut s = None;
            gpu.device.CreateSamplerState(&d, Some(&mut s))?;
            s.context("sampler")?
        };
        Ok(Kit {
            passes: code.passes.iter().map(|ps| shader(&code.full, ps)).collect::<Result<_>>()?,
            piece: shader(&code.piece_vs, &code.piece_ps)?,
            cb: ConstantBuffer::new(gpu)?,
            piece_cb: ConstantBuffer::new(gpu)?,
            opaque: pipeline::blend_state(gpu, Blend::Opaque)?,
            alpha: pipeline::blend_state(gpu, Blend::Alpha)?,
            premul: pipeline::blend_state(gpu, Blend::Premultiplied)?,
            additive,
            raster: pipeline::raster_state(gpu, false, false)?,
            no_depth: pipeline::depth_state(gpu, false, false)?,
            linear,
            point,
        })
    }
}

/// The shaders' code, compiled off the runtime's thread (it takes a while, and the headset mustn't
/// wait for it).
struct Compiled {
    full: Vec<u8>,
    passes: Vec<Vec<u8>>,
    piece_vs: Vec<u8>,
    piece_ps: Vec<u8>,
}

fn compile_all() -> Result<Compiled> {
    use rustvr_gpu::shader::compile;
    Ok(Compiled {
        full: compile(SHADER, "spectator_fx", "VSFull", "vs_5_0")?,
        passes: PASSES.iter().map(|e| compile(SHADER, "spectator_fx", e, "ps_5_0")).collect::<Result<_>>()?,
        piece_vs: compile(SHADER, "spectator_fx", "VSPiece", "vs_5_0")?,
        piece_ps: compile(SHADER, "spectator_fx", "PSPiece", "ps_5_0")?,
    })
}

/// The pictures the effects work in, at the window's size.
struct Targets {
    width: u32,
    height: u32,
    /// What the desktop view draws into, and the effects' second picture.
    scene: Texture,
    work: Texture,
    /// The body's depth from the desktop view's camera (what the runtime draws: which pixels
    /// are its, and how far).
    body_depth: Texture,
    /// Each pixel's motion (pixels), its distance (metres; 0: as far as can be) and whether the
    /// runtime drew it.
    velocity: Texture,
    /// The longest motion in each tile, and in each tile's neighbourhood.
    tile_max: Texture,
    neighbour_max: Texture,
    tile: u32,
    /// Depth of field at half size: the picture and its blur's size, then blurred.
    dof_a: Texture,
    dof_b: Texture,
    /// The glow at a quarter size.
    glow_a: Texture,
    glow_b: Texture,
}

impl Targets {
    fn new(gpu: &Gpu, width: u32, height: u32) -> Result<Targets> {
        // (Half floats: R11G11B10's render targets came out empty on an RTX 5090.)
        let tex = |w: u32, h: u32, f| Texture::new(gpu, &TexDesc::target(w.max(1), h.max(1), f));
        // (Tiles as long as the longest blur: a 32nd of the width, 50 pixels across 1600, as far as
        // a sword's swing goes in a 24th of a second.)
        let tile = (width / 32).clamp(8, 96);
        let (tw, th) = (width.div_ceil(tile), height.div_ceil(tile));
        Ok(Targets {
            width,
            height,
            scene: tex(width, height, DXGI_FORMAT_R16G16B16A16_FLOAT)?,
            work: tex(width, height, DXGI_FORMAT_R16G16B16A16_FLOAT)?,
            body_depth: Texture::new(gpu, &TexDesc { width, height, format: DXGI_FORMAT_R32_TYPELESS, rtv: None, srv: Some(DXGI_FORMAT_R32_FLOAT), dsv: Some(DXGI_FORMAT_D32_FLOAT), mips: 1 })?,
            velocity: tex(width, height, DXGI_FORMAT_R16G16B16A16_FLOAT)?,
            tile_max: tex(tw, th, DXGI_FORMAT_R16G16_FLOAT)?,
            neighbour_max: tex(tw, th, DXGI_FORMAT_R16G16_FLOAT)?,
            tile,
            dof_a: tex(width.div_ceil(2), height.div_ceil(2), DXGI_FORMAT_R16G16B16A16_FLOAT)?,
            dof_b: tex(width.div_ceil(2), height.div_ceil(2), DXGI_FORMAT_R16G16B16A16_FLOAT)?,
            glow_a: tex(width.div_ceil(4), height.div_ceil(4), DXGI_FORMAT_R16G16B16A16_FLOAT)?,
            glow_b: tex(width.div_ceil(4), height.div_ceil(4), DXGI_FORMAT_R16G16B16A16_FLOAT)?,
        })
    }
}

/// The world's picture from before a jump: a copy of the desktop view's background as it was,
/// the camera it was seen from and the world's turn then.
struct History {
    tex: Texture,
    format: DXGI_FORMAT,
    vp: Mat4,
    yaw: f32,
    /// Seconds since it was taken; none taken yet.
    age: Option<f32>,
}

/// Timing the desktop view on the GPU: its picture (the background, the body, the layers) and its
/// effects, said every ten seconds.
struct Timer {
    sets: Vec<[ID3D11Query; 5]>,
    pending: Vec<bool>,
    next: usize,
    open: Option<usize>,
    picture: Vec<f32>,
    effects: Vec<f32>,
    since: Option<std::time::Instant>,
}

impl Timer {
    fn new(gpu: &Gpu) -> Result<Timer> {
        let query = |kind| -> Result<ID3D11Query> {
            let mut q = None;
            unsafe { gpu.device.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut q))? };
            q.context("no query")
        };
        let sets = (0..6)
            .map(|_| Ok([query(D3D11_QUERY_TIMESTAMP_DISJOINT)?, query(D3D11_QUERY_TIMESTAMP)?, query(D3D11_QUERY_TIMESTAMP)?, query(D3D11_QUERY_TIMESTAMP)?, query(D3D11_QUERY_TIMESTAMP)?]))
            .collect::<Result<Vec<_>>>()?;
        Ok(Timer { pending: vec![false; sets.len()], sets, next: 0, open: None, picture: Vec::new(), effects: Vec::new(), since: None })
    }

    /// Mark `n` (1: the picture starts, 2: it's done, 3: the effects start, 4: they're done) of
    /// this frame's set.
    fn mark(&mut self, gpu: &Gpu, n: usize) {
        unsafe {
            if n == 1 {
                self.collect(gpu);
                if self.pending[self.next] {
                    return;
                }
                self.open = Some(self.next);
                gpu.ctx.Begin(&self.sets[self.next][0]);
            }
            let Some(i) = self.open else { return };
            gpu.ctx.End(&self.sets[i][n]);
            if n == 4 {
                gpu.ctx.End(&self.sets[i][0]);
                self.pending[i] = true;
                self.open = None;
                self.next = (i + 1) % self.sets.len();
            }
        }
    }

    fn collect(&mut self, gpu: &Gpu) {
        for (i, set) in self.sets.iter().enumerate() {
            if !self.pending[i] {
                continue;
            }
            let Some(freq) = query_data::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>(gpu, &set[0]) else { continue };
            let t: Vec<Option<u64>> = set[1..].iter().map(|q| query_data::<u64>(gpu, q)).collect();
            let (Some(a), Some(b), Some(c), Some(d)) = (t[0], t[1], t[2], t[3]) else { continue };
            self.pending[i] = false;
            if !freq.Disjoint.as_bool() && freq.Frequency > 0 && b >= a && d >= c {
                let ms = |x: u64, y: u64| ((y - x) as f64 * 1000.0 / freq.Frequency as f64) as f32;
                self.picture.push(ms(a, b));
                self.effects.push(ms(c, d));
            }
        }
    }

    /// Every ten seconds: the medians since the last time, if there are any.
    fn report(&mut self) -> Option<(f32, f32, usize)> {
        let now = std::time::Instant::now();
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since).as_secs_f32() < 10.0 || self.picture.is_empty() {
            return None;
        }
        self.since = Some(now);
        let median = |v: &mut Vec<f32>| {
            v.sort_by(f32::total_cmp);
            let m = v[v.len() / 2];
            v.clear();
            m
        };
        let n = self.picture.len();
        Some((median(&mut self.picture), median(&mut self.effects), n))
    }
}

/// A query's data if it's there yet (never waits).
fn query_data<T: Default>(gpu: &Gpu, q: &ID3D11Query) -> Option<T> {
    let mut out = T::default();
    let hr = unsafe {
        (Interface::vtable(&gpu.ctx).GetData)(
            Interface::as_raw(&gpu.ctx),
            q.as_raw(),
            (&mut out as *mut T).cast(),
            size_of::<T>() as u32,
            D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
        )
    };
    (hr.0 == 0).then_some(out)
}

/// Last drawn frame's cameras and hands, for each pixel's motion.
#[derive(Clone, Copy, Debug)]
struct Prev {
    vp: Mat4,
    world_vp: Mat4,
    hands: [Option<Mat4>; 2],
}

/// This frame, between the hooks.
struct Frame {
    window: EyeTarget,
    /// Drawn into the studio's own picture, for the effects.
    effects: bool,
    studio: Studio,
    cam: Pose,
    world: Pose,
    fov: Fov,
    dt: f32,
    /// How a point of the world moved past the player this frame (tracking: last frame's place to
    /// this frame's).
    world_step: Mat4,
    hands: [Option<Mat4>; 2],
    body_axis: Vec3,
    /// Where to focus: a point of the picture (its depth there), or a distance (metres).
    focus: (Vec2, f32),
    /// No motion to blur across this frame (another place, the first frame).
    cut: bool,
    game_depth: Option<ID3D11ShaderResourceView>,
    metres_per_unit: f32,
    body_drawn: bool,
    /// The flat HUD this frame: its pieces, its picture (a copy, the game's own being given
    /// back before the effects run) and whether it's added rather than laid over.
    hud: Vec<FlatPiece>,
    hud_additive: bool,
    /// The jump being eased: how much of the dissolve is left.
    dissolve: f32,
}

/// The desktop view's effects stage, the session's (`Session::spectator_fx`).
pub struct Stage {
    compiling: Option<std::thread::JoinHandle<Result<Compiled>>>,
    kit: Option<Kit>,
    targets: Option<Targets>,
    history: Option<History>,
    hud_copy: Option<(Texture, ID3D11ShaderResourceView)>,
    focus: [Option<Texture>; 2],
    focus_flip: usize,
    lut: Option<(String, Option<(Texture, u32)>)>,
    timer: Option<Timer>,
    ground: Ground,
    shake: Shake,
    chest: Chest,
    widen: f32,
    /// Events from the game since the last frame: rumble, blasts (world position, radius), shots.
    rumble: f32,
    blasts: Vec<(Vec3, f32)>,
    game_shots: u32,
    frame: Option<Frame>,
    prev: Option<Prev>,
    seconds: f32,
    clock: std::time::Instant,
    said_look: Option<Look>,
}

impl Stage {
    /// Starts compiling the shaders (they're ready a moment later; until then the desktop view
    /// shows without effects).
    pub fn new(gpu: &Gpu) -> Stage {
        let compiling = std::thread::Builder::new().name("rustvr-studio-shaders".into()).spawn(compile_all).ok();
        Stage {
            compiling,
            kit: None,
            targets: None,
            history: None,
            hud_copy: None,
            focus: [None, None],
            focus_flip: 0,
            lut: None,
            timer: Timer::new(gpu).inspect_err(|e| tracing::warn!("desktop view: its timer: {e:#}")).ok(),
            ground: Ground::default(),
            shake: Shake::default(),
            chest: Chest::default(),
            widen: 0.0,
            rumble: 0.0,
            blasts: Vec::new(),
            game_shots: 0,
            frame: None,
            prev: None,
            seconds: 0.0,
            clock: std::time::Instant::now(),
            said_look: None,
        }
    }

    /// The game's events (`EV_*`) that shake the camera: its rumble, its explosions, its gun firing.
    pub fn event(&mut self, ev: &Event) {
        match ev.kind {
            EV_RUMBLE => self.rumble = self.rumble.max(ev.f[0]),
            EV_BLAST if self.blasts.len() < 8 => self.blasts.push((Vec3::new(ev.f[0], ev.f[1], ev.f[2]), ev.f[3])),
            EV_GAME_SHOTS => self.game_shots = self.game_shots.saturating_add(ev.a),
            _ => {}
        }
    }

    fn ready(&mut self, gpu: &Gpu) -> Option<&Kit> {
        if self.kit.is_none() && self.compiling.as_ref().is_some_and(|c| c.is_finished()) {
            let made = self.compiling.take().unwrap().join().map_err(|_| anyhow::anyhow!("the compiler's thread panicked")).and_then(|r| r).and_then(|c| Kit::new(gpu, &c));
            match made {
                Ok(k) => {
                    tracing::info!("desktop view: the effects studio's shaders are ready");
                    self.kit = Some(k);
                }
                Err(e) => tracing::error!("desktop view: the effects studio's shaders: {e:#}"),
            }
        }
        self.kit.as_ref()
    }

    /// The picture the desktop view draws into this frame: the studio's own while it has effects
    /// to draw over it, else the window's.
    pub fn target(&mut self, gpu: &Gpu, window: &EyeTarget, s: &Studio) -> EyeTarget {
        if let Some(t) = &mut self.timer {
            t.mark(gpu, 1);
        }
        if self.said_look != Some(s.look) {
            self.said_look = Some(s.look);
            tracing::info!("desktop view: the {:?} look", s.look);
        }
        let lut = self.lut_ready(gpu, s);
        // (The shaders are taken up as soon as they're compiled: smoothed snaps fill in with one.)
        let ready = self.ready(gpu).is_some();
        let effects = draws_effects(s, lut) && ready;
        let own = |t: &Texture| EyeTarget { tex: t.tex.clone(), rtv: t.rtv.clone().unwrap(), width: t.width, height: t.height };
        let window = EyeTarget { tex: window.tex.clone(), rtv: window.rtv.clone(), width: window.width, height: window.height };
        let mut target = None;
        if effects {
            if self.targets.as_ref().is_none_or(|t| (t.width, t.height) != (window.width, window.height)) {
                self.targets = Targets::new(gpu, window.width, window.height).inspect_err(|e| tracing::warn!("desktop view: the effects' pictures: {e:#}")).ok();
                self.prev = None;
            }
            target = self.targets.as_ref().map(|t| own(&t.scene));
        }
        let drawn = target.unwrap_or(EyeTarget { tex: window.tex.clone(), rtv: window.rtv.clone(), width: window.width, height: window.height });
        let still = Pose { pos: [0.0; 3], valid: 1, rot: Quat::IDENTITY.to_array() };
        self.frame = Some(Frame {
            window,
            effects: effects && self.targets.is_some(),
            studio: s.clone(),
            cam: still,
            world: still,
            fov: Fov::default(),
            dt: 0.0,
            world_step: Mat4::IDENTITY,
            hands: [None; 2],
            body_axis: Vec3::ZERO,
            focus: (Vec2::splat(0.5), 0.0),
            cut: true,
            game_depth: None,
            metres_per_unit: 1.0,
            body_drawn: false,
            hud: Vec::new(),
            hud_additive: false,
            dissolve: 0.0,
        });
        drawn
    }

    /// The `.cube` LUT the settings name, loaded (again when the name changes); whether there's
    /// one.
    fn lut_ready(&mut self, gpu: &Gpu, s: &Studio) -> bool {
        if s.lut.is_empty() {
            self.lut = None;
            return false;
        }
        if self.lut.as_ref().is_none_or(|(name, _)| *name != s.lut) {
            let path = luts_dir().join(&s.lut);
            let loaded = std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|text| parse_cube(&text)).and_then(|(n, rgb)| {
                // A strip of its blue slices side by side: n * n across, n down.
                let mut px = Vec::with_capacity(rgb.len() * 4);
                for c in &rgb {
                    px.extend_from_slice(&[c[0], c[1], c[2], 1.0]);
                }
                let bytes: &[u8] = bytemuck::cast_slice(&px);
                Ok((Texture::upload(gpu, n * n, n, DXGI_FORMAT_R32G32B32A32_FLOAT, &[bytes])?, n))
            });
            match &loaded {
                Ok((_, n)) => tracing::info!("desktop view: LUT {} ({n} a side)", path.display()),
                Err(e) => tracing::warn!("desktop view: LUT {}: {e:#}", path.display()),
            }
            self.lut = Some((s.lut.clone(), loaded.ok()));
        }
        self.lut.as_ref().is_some_and(|(_, l)| l.is_some())
    }

    /// The camera for this frame: the steadied camera turned after the chest, shaken, its field of
    /// view widened with speed, and its view of the world turned through a jump being eased out.
    pub fn shot(&mut self, i: &ShotInput) -> Shot {
        let none = Shot { cam: i.cam, world: Pose { rot: (Quat::from_array(i.cam.rot) * bob_turn(&i.cam, &i.bobbed)).normalize().to_array(), ..i.cam }, fov: i.fov };
        let Some(f) = self.frame.as_mut() else { return none };
        let s = &i.settings.studio;
        let dt = i.frame.dt.clamp(0.0, 0.1);
        self.seconds = (self.seconds + dt) % 3600.0;
        f.dt = dt;
        let pos = Vec3::from_array(i.cam.pos);
        let mut rot = Quat::from_array(i.cam.rot).normalize();
        let bob = bob_turn(&i.cam, &i.bobbed);
        // The world under the player, from the pictures.
        let mut cut = self.prev.is_none();
        let mut landed = None;
        // (A jump being eased moved the world past the player by itself this frame, and the
        // world's camera by as much: no motion between them.)
        let mut jump_step = None;
        match i.picture {
            Some((info, eye)) if info.eye_map[eye].world_per_meter > 0.0 => {
                let map = WorldMap::from_slot(&info.eye_map[eye]);
                let was = self.ground.last.map(|(_, m)| m);
                let (step, l) = self.ground.see(info.frame_index, map, pos, self.clock.elapsed().as_secs_f64());
                landed = l;
                match step {
                    Step::Cut => cut = true,
                    Step::Jump if s.smooth_snaps => {
                        let was = was.unwrap_or(map);
                        let d = map.matrix().inverse() * was.matrix();
                        let yaw = crate::wrap_angle(was.body_yaw - map.body_yaw);
                        let moved = d.transform_point3(pos) - pos;
                        // On top of what's still to come of one before it, and its momentum.
                        let dissolve = moved.length() > JUMP_RISE;
                        self.ground.ease = Some(Ease::jump(self.ground.ease, yaw, pos, s.snap_seconds, dissolve));
                        tracing::info!("desktop view: the world jumped {:.0} degrees and {:.2} m; turning through it as a head would", yaw.to_degrees(), moved.length());
                        // (No blur across the jump itself: the eased turn blurs instead.)
                        cut = cut || dissolve;
                        jump_step = Some(d);
                    }
                    Step::Jump => cut = true,
                    Step::Steady => {}
                }
            }
            _ => {
                self.ground = Ground::default();
                cut = true;
            }
        }
        // The chest's camera.
        if s.chest_mount > 0.001 {
            let chest = self.chest.update(rot, dt);
            rot = rot.slerp(chest, s.chest_mount.clamp(0.0, 1.0));
        } else {
            self.chest = Chest::default();
        }
        // The shake: this frame's events.
        if s.shake {
            let head = Vec3::from_array(i.frame.hmd.pos);
            for fb in i.feedback {
                let k = fb.intensity.clamp(0.0, 1.0);
                // (A rifle's round adds about what a second wears off, so a burst shakes steadily
                // rather than ever harder; each round's kick is felt.)
                match fb.cue {
                    Cue::ShotPistol => self.shake.add(0.12, Vec3::new(-0.8, 0.0, 0.3)),
                    Cue::ShotRifle => self.shake.add(0.1, Vec3::new(-1.2, 0.2, 0.4)),
                    Cue::ShotShotgun => self.shake.add(0.4, Vec3::new(-2.5, 0.3, 0.8)),
                    Cue::BowRelease => self.shake.add(0.06, Vec3::new(-0.3, 0.0, 0.0)),
                    // (A blow that lands hard: a light touch doesn't shake the camera.)
                    Cue::Impact | Cue::Melee if k > 0.25 => self.shake.add(0.45 * k * k, Vec3::new(0.6 * k, 0.0, if fb.pos.x > head.x { -1.0 } else { 1.0 } * 0.8 * k)),
                    Cue::Explosion => {
                        let near = (1.0 - fb.pos.distance(head) / 20.0).clamp(0.0, 1.0);
                        self.shake.add(0.9 * near, Vec3::new(1.5 * near, 0.0, 0.0));
                    }
                    _ => {}
                }
            }
            if self.rumble > 0.0 {
                self.shake.add(0.5 * self.rumble.clamp(0.0, 1.0), Vec3::ZERO);
            }
            for _ in 0..self.game_shots.min(4) {
                self.shake.add(0.15, Vec3::new(-1.0, 0.0, 0.3));
            }
            if let Some((info, eye)) = i.picture
                && info.eye_map[eye].world_per_meter > 0.0
            {
                let map = WorldMap::from_slot(&info.eye_map[eye]);
                for (at, radius) in &self.blasts {
                    let near = (1.0 - map.inv_point(*at).distance(head) / (radius.max(1.0) * 4.0)).clamp(0.0, 1.0);
                    self.shake.add(0.9 * near, Vec3::new(1.5 * near, 0.0, 0.0));
                }
            }
            // A landing: the view dips and shakes, harder the faster the fall.
            if let Some(speed) = landed {
                let k = ((speed - 4.0) / 8.0).clamp(0.0, 1.0);
                self.shake.add(0.6 * k, Vec3::new(6.0 * k, 0.0, 0.0));
            }
        }
        (self.rumble, self.game_shots) = (0.0, 0);
        self.blasts.clear();
        // The jump being turned out: on a step, and the camera leaning into its turn.
        if let Some(e) = &mut self.ground.ease
            && (!s.smooth_snaps || !e.step(dt, s.snap_seconds))
        {
            self.ground.ease = None;
        }
        if let Some(e) = &self.ground.ease {
            rot *= Quat::from_rotation_z((-e.speed * HEAD_LEAN).clamp(-0.04, 0.04));
        }
        if s.shake || s.handheld > 0.0 {
            let turn = self.shake.update(dt, if s.shake { s.shake_strength } else { 0.0 }, s.handheld);
            rot *= Quat::from_euler(EulerRot::YXZ, turn.y.to_radians(), turn.x.to_radians(), turn.z.to_radians());
        } else {
            self.shake = Shake::default();
        }
        let cam = Pose { pos: i.cam.pos, valid: 1, rot: rot.normalize().to_array() };
        // The world's camera: bobbed, and turned back by as much of a jump as the head hasn't turned
        // yet.
        let mut world = Pose { rot: (rot * bob).normalize().to_array(), ..cam };
        f.dissolve = 0.0;
        if let Some(e) = &self.ground.ease {
            let r = Quat::from_rotation_y(e.yaw);
            world.pos = (e.pivot + r * (pos - e.pivot)).to_array();
            world.rot = (r * rot * bob).normalize().to_array();
            f.dissolve = e.dissolving(s.snap_seconds);
        }
        // The field of view: the lens's (the zoom the desktop view has now kept), wider with speed.
        let aspect = i.fov.right / i.fov.up.max(1e-4);
        let now_deg = 2.0 * i.fov.right.atan().to_degrees();
        let base = i.settings.spectator.fov.max(1.0);
        let zoom = now_deg / base;
        let lens = if s.lens_fov > 1.0 { s.lens_fov } else { base };
        let speed = self.ground.walk.length();
        let want = if s.speed_fov && !cut { s.speed_fov_deg * smoothstep(s.speed_from, s.speed_full.max(s.speed_from + 0.1), speed) } else { 0.0 };
        // (Wider quickly, back slowly.)
        let tau = if want > self.widen { 0.35 } else { 0.6 };
        self.widen += (want - self.widen) * (1.0 - (-dt / tau).exp());
        // Never wider than the game's pictures reach across (but never narrower than asked).
        let reach = i.picture.map_or(180.0, |(info, _)| {
            let [a, b] = info.fov;
            2.0 * a.right.atan().max(b.right.atan()).min((-a.left).atan().max((-b.left).atan())).to_degrees()
        });
        let deg = (lens + self.widen.min((reach - lens).max(0.0))) * zoom;
        let fov = crate::spectator::fov(deg, aspect);
        // The hands, for what they carry.
        let hands = std::array::from_fn(|h| {
            let tracked = i.rig.map(|r| &r.hands[h]).filter(|r| r.flags & HAND_TRACKED != 0);
            match tracked {
                Some(r) => Some(Mat4::from_rotation_translation(Quat::from_array(r.pose.rot).normalize(), Vec3::from_array(r.pose.pos))),
                None => {
                    let c = &i.frame.hands[h];
                    (c.connected != 0).then(|| Mat4::from_rotation_translation(Quat::from_array(c.grip.rot).normalize(), Vec3::from_array(c.grip.pos)))
                }
            }
        });
        // The body's axis: under the neck, a little behind the eyes.
        let head = Vec3::from_array(i.frame.hmd.pos);
        let fwd = Quat::from_array(i.frame.hmd.rot) * Vec3::Z;
        let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::Z);
        f.body_axis = head - flat * 0.1 - Vec3::Y * 0.12;
        // How the world moved past this frame: its steady turn and walk (a jump is eased instead).
        let turn = Mat4::from_translation(pos) * Mat4::from_rotation_y(self.ground.turn * dt) * Mat4::from_translation(-pos);
        f.world_step = jump_step.unwrap_or(Mat4::from_translation(self.ground.walk * dt) * turn);
        // Where to focus: what a hand holds up close in front of the camera, else where a gun aims,
        // else the middle of the view (where the head looks).
        let cam_fwd = rot * Vec3::Z;
        let held = (0..2).filter_map(|h| {
            let r = i.rig?.hands[h];
            if r.flags & HAND_HOLDING == 0 {
                return None;
            }
            let p = Vec3::from_array(r.pose.pos) - pos;
            (p.length() < 0.7 && p.normalize_or_zero().dot(cam_fwd) > 25f32.to_radians().cos()).then(|| p.dot(cam_fwd))
        });
        let close = held.fold(None, |a: Option<f32>, d| Some(a.map_or(d, |a| a.min(d))));
        let aimed = i.weapon.filter(|w| w.aim_dir.normalize_or_zero().dot(cam_fwd) > 20f32.to_radians().cos()).and_then(|w| crate::compositor::project(&cam, &fov, w.sight_origin + w.aim_dir * 100.0));
        f.focus = match (close, aimed) {
            (Some(d), _) => (Vec2::splat(0.5), d.max(0.15)),
            (None, Some(uv)) => (uv.clamp(Vec2::ZERO, Vec2::ONE), 0.0),
            _ => (Vec2::splat(0.5), 0.0),
        };
        f.cam = cam;
        f.world = world;
        f.fov = fov;
        f.hands = hands;
        f.cut = cut;
        if let Some(h) = &mut self.history {
            h.age = h.age.map(|a| a + dt);
        }
        Shot { cam, world, fov }
    }

    /// The game's world drawn now, from the world's camera, when jumps are smoothed or the edges
    /// filled, returning what `draw_view` draws then (nothing more of the background). Where the
    /// eyes' pictures don't reach: mid-jump, the picture from before the jump; past that, the
    /// nearest picture's edge stretched out over the gap. A steady frame's picture is kept for the
    /// next jump. And with the flat HUD on, the HUD's pieces taken out of the desktop view's
    /// layers: it draws them flat instead.
    #[allow(clippy::too_many_arguments)]
    pub fn background<'a>(&mut self, gpu: &Gpu, comp: &Compositor, target: &EyeTarget, world: &Pose, fov: &Fov, background: Background<'a>, layers: &mut Layers, hud: Option<&FlatHud>) -> Background<'a> {
        let Some(f) = self.frame.as_mut() else { return background };
        let s = f.studio.clone();
        // The flat HUD's pieces, and the same pieces out of the world.
        f.hud.clear();
        if s.flat_hud
            && let Some(h) = hud
            && let Some(srv) = h.tex.srv.as_ref()
        {
            let (w, hh) = (f.window.width as f32, f.window.height as f32);
            f.hud = flat_layout(h.layout, h.aspect, [0.0, 0.0, w, hh], s.flat_hud_size);
            f.hud_additive = h.layout.flags & HUDL_ADDITIVE != 0;
            let pieces = &f.hud;
            layers.world_panels.retain(|p| !is_flat_piece(p.srv, &p.uv, srv, pieces));
            layers.panels.retain(|p| !is_flat_piece(p.srv, &p.uv, srv, pieces));
        }
        // The eyes' pictures, each with where it's seen from (a direction to its clip space).
        let picture = match &background {
            Background::Reprojected { first, second, .. } => Some((
                (projection(&first.2) * view_matrix(&first.1), first.0.clone()),
                second.as_ref().map(|b| (projection(&b.2) * view_matrix(&b.1), b.0.clone())),
            )),
            _ => None,
        };
        if !s.smooth_snaps {
            self.history = None;
        }
        let Some(((eye_a, srv_a), eye_b)) = picture.filter(|_| s.smooth_snaps || s.fill_edges) else {
            self.history = None;
            return background;
        };
        comp.draw_view(gpu, target, world, fov, background, &Layers::default(), false, &mut |_| {});
        let ease = self.ground.ease.filter(|_| f.world.rot != f.cam.rot || f.dissolve > 0.0);
        let yaw_now = self.ground.last.map_or(0.0, |(_, m)| m.body_yaw);
        let world_vp = projection(fov) * view_matrix(world);
        let history = self.history.as_ref().filter(|h| ease.is_some() && h.age.is_some_and(|a| a < HISTORY_KEEP));
        if let Some(kit) = self.kit.as_ref()
            && (history.is_some() || s.fill_edges)
        {
            let mut cb = FxCb::zeroed();
            cb.world_inv_vp = world_vp.inverse().to_cols_array();
            cb.eye_a = eye_a.to_cols_array();
            cb.eye_b = eye_b.as_ref().map_or(Mat4::ZERO, |b| b.0).to_cols_array();
            cb.hist_vp = history.map_or(Mat4::ZERO, |h| h.vp * Mat4::from_rotation_y(yaw_now - h.yaw)).to_cols_array();
            let on = |b: bool| if b { 1.0 } else { 0.0 };
            cb.fill = [on(history.is_some()), f.dissolve, on(s.fill_edges), on(eye_b.is_some())];
            kit.cb.update(&gpu.ctx, &cb);
            unsafe {
                gpu.ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
                gpu.ctx.OMSetBlendState(&kit.alpha, Some(&[0.0; 4]), 0xFFFF_FFFF);
            }
            let history_srv = history.and_then(|h| h.tex.srv.clone());
            draw_pass(gpu, kit, PASS_FILL, target.width, target.height, &[history_srv, Some(srv_a), eye_b.map(|b| b.1)]);
        }
        // Steady: this frame's picture is the one to fill in with if a jump comes next.
        if s.smooth_snaps && ease.is_none() {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            unsafe { target.tex.GetDesc(&mut d) };
            let fits = self.history.as_ref().is_some_and(|h| (h.tex.width, h.tex.height, h.format) == (d.Width, d.Height, d.Format));
            if !fits {
                // (The window's own picture is stored plain and seen as sRGB; the studio's is
                // linear.)
                let (store, view) = match d.Format {
                    DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_R8G8B8A8_TYPELESS => (DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
                    DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS => (DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB),
                    other => (other, other),
                };
                let desc = TexDesc { width: d.Width, height: d.Height, format: store, rtv: None, srv: Some(view), dsv: None, mips: 1 };
                self.history = Texture::new(gpu, &desc).inspect_err(|e| tracing::warn!("desktop view: the picture kept for jumps: {e:#}")).ok().map(|tex| History { tex, format: d.Format, vp: Mat4::IDENTITY, yaw: 0.0, age: None });
            }
            if let Some(h) = &mut self.history {
                unsafe { gpu.ctx.CopyResource(&h.tex.tex, &target.tex) };
                h.vp = world_vp;
                h.yaw = yaw_now;
                h.age = Some(0.0);
            }
        }
        unsafe { gpu.ctx.OMSetRenderTargets(None, None) };
        Background::Keep
    }

    /// After the desktop view's picture is drawn (before the game's picture is given back): the
    /// body's depth from the camera (for what moves with the hands, and the focus), the game's
    /// depth as the camera sees it, and a copy of the HUD's picture for the flat HUD.
    #[allow(clippy::too_many_arguments)]
    pub fn drawn(&mut self, gpu: &Gpu, body: &mut crate::body::PlayerBody, held: &[rustvr_avatar::HeldDraw], game_depth: Option<&ID3D11ShaderResourceView>, metres_per_unit: f32, hud: Option<&FlatHud>) {
        let Some(f) = self.frame.as_mut() else { return };
        let s = &f.studio;
        f.game_depth = game_depth.cloned();
        f.metres_per_unit = metres_per_unit;
        if f.effects
            && (s.motion_blur || s.dof)
            && let Some(t) = &self.targets
            && let Some(dsv) = &t.body_depth.dsv
        {
            unsafe { gpu.ctx.ClearDepthStencilView(dsv, D3D11_CLEAR_DEPTH.0, 1.0, 0) };
            body.draw_depth(gpu, &DepthTarget { dsv: dsv.clone(), width: t.width, height: t.height }, &f.cam, &f.fov, held);
            f.body_drawn = true;
        }
        // The HUD's picture, copied: it's drawn after the headset's frame, and the game's own is
        // the game's again by then.
        if let Some(h) = hud.filter(|_| !f.hud.is_empty())
            && let Some(src_view) = &h.tex.srv
        {
            let fits = self.hud_copy.as_ref().is_some_and(|(t, _)| (t.width, t.height, t.format) == (h.tex.width, h.tex.height, h.tex.format));
            if !fits {
                let mut vd = D3D11_SHADER_RESOURCE_VIEW_DESC::default();
                unsafe { src_view.GetDesc(&mut vd) };
                let desc = TexDesc { width: h.tex.width, height: h.tex.height, format: h.tex.format, rtv: None, srv: Some(vd.Format), dsv: None, mips: 1 };
                self.hud_copy = Texture::new(gpu, &desc).inspect_err(|e| tracing::warn!("desktop view: the HUD's copy: {e:#}")).ok().and_then(|t| Some((t.srv.clone()?, t))).map(|(v, t)| (t, v));
            }
            match &self.hud_copy {
                Some((t, _)) => unsafe { gpu.ctx.CopyResource(&t.tex, &h.tex.tex) },
                None => f.hud.clear(),
            }
        }
        // (What's drawn before the headset's frame is handed over ends here: it's the picture's.)
        if let Some(t) = &mut self.timer {
            t.mark(gpu, 2);
        }
    }

    /// The effects over the desktop view's picture, into the window, and the flat HUD over them:
    /// after the headset's frame has been handed over, so they never hold it up.
    pub fn finish(&mut self, gpu: &Gpu) {
        let Some(f) = self.frame.take() else { return };
        if let Some(t) = &mut self.timer {
            t.mark(gpu, 3);
        }
        if f.effects
            && let (Some(kit), Some(t)) = (self.kit.as_ref(), self.targets.as_ref())
        {
            let focus = self.focus_flip;
            if self.focus[0].is_none() {
                // (Cleared: no focus yet, the first frame takes the one it finds.)
                self.focus = std::array::from_fn(|_| Texture::new(gpu, &TexDesc::target(1, 1, DXGI_FORMAT_R32_FLOAT)).ok());
                for t in self.focus.iter().flatten() {
                    unsafe { gpu.ctx.ClearRenderTargetView(t.rtv.as_ref().unwrap(), &[0.0; 4]) };
                }
            }
            let lut = self.lut.as_ref().and_then(|(_, l)| l.as_ref());
            let prev = if f.cut { None } else { self.prev };
            effects(gpu, kit, t, &f, prev, lut, [self.focus[focus].as_ref(), self.focus[focus ^ 1].as_ref()], self.seconds);
            self.focus_flip ^= 1;
        }
        if !f.hud.is_empty()
            && let (Some(kit), Some((_, srv))) = (self.kit.as_ref(), self.hud_copy.as_ref())
        {
            draw_pieces(gpu, kit, &f.window, srv, &f.hud, f.hud_additive);
        }
        self.prev = Some(Prev { vp: projection(&f.fov) * view_matrix(&f.cam), world_vp: projection(&f.fov) * view_matrix(&f.world), hands: f.hands });
        if let Some(t) = &mut self.timer {
            t.mark(gpu, 4);
            if let Some((picture, fx, n)) = t.report() {
                tracing::info!(
                    "desktop view: {picture:.2} ms on the GPU for its picture, {fx:.2} ms for its effects and HUD ({:?} look, {}x{}, medians of {n} frames)",
                    f.studio.look,
                    f.window.width,
                    f.window.height
                );
            }
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where the studio's `.cube` LUTs are: the settings' folder's `luts`.
pub fn luts_dir() -> std::path::PathBuf {
    crate::app::data_dir().join("luts")
}

/// The `.cube` files there, by name (looked for again every few seconds: the menu asks every
/// frame it's open).
pub fn luts() -> Vec<String> {
    static FOUND: std::sync::Mutex<Option<(std::time::Instant, Vec<String>)>> = std::sync::Mutex::new(None);
    let mut found = FOUND.lock().unwrap();
    if found.as_ref().is_none_or(|(at, _)| at.elapsed().as_secs_f32() > 3.0) {
        let mut names: Vec<String> = std::fs::read_dir(luts_dir())
            .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.to_ascii_lowercase().ends_with(".cube")).collect())
            .unwrap_or_default();
        names.sort();
        *found = Some((std::time::Instant::now(), names));
    }
    found.as_ref().map(|(_, n)| n.clone()).unwrap_or_default()
}

/// A 3D LUT in the `.cube` format (Resolve's, Adobe's): its size and its colours, red changing
/// fastest, then green, then blue.
pub fn parse_cube(text: &str) -> Result<(u32, Vec<[f32; 3]>)> {
    let mut n = 0u32;
    let (mut lo, mut hi) = ([0.0f32; 3], [1.0f32; 3]);
    let mut rgb = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or_default();
        let nums = |w: std::str::SplitWhitespace| w.map(|x| x.parse::<f32>()).collect::<Result<Vec<f32>, _>>();
        match first {
            "LUT_3D_SIZE" => n = words.next().context("LUT_3D_SIZE without a size")?.parse()?,
            "DOMAIN_MIN" => lo.copy_from_slice(nums(words)?.get(..3).context("DOMAIN_MIN")?),
            "DOMAIN_MAX" => hi.copy_from_slice(nums(words)?.get(..3).context("DOMAIN_MAX")?),
            "TITLE" | "LUT_1D_SIZE" | "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                if first == "LUT_1D_SIZE" {
                    anyhow::bail!("a 1D LUT (only 3D ones are taken)");
                }
            }
            _ => {
                let v: Vec<f32> = line.split_whitespace().map(|x| x.parse::<f32>()).collect::<Result<_, _>>().with_context(|| format!("a line that isn't a colour: {line}"))?;
                anyhow::ensure!(v.len() == 3, "a line that isn't a colour: {line}");
                rgb.push([(v[0] - lo[0]) / (hi[0] - lo[0]), (v[1] - lo[1]) / (hi[1] - lo[1]), (v[2] - lo[2]) / (hi[2] - lo[2])]);
            }
        }
    }
    anyhow::ensure!((2..=128).contains(&n), "LUT_3D_SIZE {n}");
    anyhow::ensure!(rgb.len() == (n * n * n) as usize, "{} colours for a size of {n}", rgb.len());
    Ok((n, rgb))
}

/// One full-screen pass `n` into whatever's bound, `w` x `h`, reading `srvs` from slot 0.
fn draw_pass(gpu: &Gpu, kit: &Kit, n: usize, w: u32, h: u32, srvs: &[Option<ID3D11ShaderResourceView>]) {
    let ctx = &gpu.ctx;
    unsafe {
        ctx.RSSetState(&kit.raster);
        ctx.OMSetDepthStencilState(&kit.no_depth, 0);
        ctx.PSSetSamplers(0, Some(&[Some(kit.linear.clone()), Some(kit.point.clone())]));
        ctx.VSSetConstantBuffers(0, Some(&[Some(kit.cb.buf.clone())]));
        ctx.PSSetConstantBuffers(0, Some(&[Some(kit.cb.buf.clone())]));
        ctx.IASetInputLayout(None);
        ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        pipeline::viewport(ctx, w, h);
        kit.passes[n].bind(ctx);
        ctx.PSSetShaderResources(0, Some(srvs));
        ctx.Draw(3, 0);
        let none: Vec<Option<ID3D11ShaderResourceView>> = vec![None; srvs.len()];
        ctx.PSSetShaderResources(0, Some(&none));
    }
}

/// Pass `n` from `srvs` into `out` (opaque).
fn pass_into(gpu: &Gpu, kit: &Kit, n: usize, out: &Texture, srvs: &[Option<ID3D11ShaderResourceView>]) {
    unsafe {
        gpu.ctx.OMSetRenderTargets(Some(std::slice::from_ref(&out.rtv)), None);
        gpu.ctx.OMSetBlendState(&kit.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
    }
    draw_pass(gpu, kit, n, out.width, out.height, srvs);
    unsafe { gpu.ctx.OMSetRenderTargets(None, None) };
}

const PASS_VELOCITY: usize = 0;
const PASS_TILE_MAX: usize = 1;
const PASS_NEIGHBOUR_MAX: usize = 2;
const PASS_MOTION_BLUR: usize = 3;
const PASS_FOCUS: usize = 4;
const PASS_DOF_DOWN: usize = 5;
const PASS_DOF_BLUR: usize = 6;
const PASS_GLOW_DOWN: usize = 7;
const PASS_GLOW_ACROSS: usize = 8;
const PASS_GLOW_DOWNWARDS: usize = 9;
const PASS_FINAL: usize = 10;
const PASS_FILL: usize = 11;

/// The effects over the frame's picture (`t.scene`), into its window.
#[allow(clippy::too_many_arguments)]
fn effects(gpu: &Gpu, kit: &Kit, t: &Targets, f: &Frame, prev: Option<Prev>, lut: Option<&(Texture, u32)>, focus: [Option<&Texture>; 2], seconds: f32) {
    let s = &f.studio;
    let vp = projection(&f.fov) * view_matrix(&f.cam);
    let world_vp = projection(&f.fov) * view_matrix(&f.world);
    let world_rot = Quat::from_array(f.world.rot).normalize();
    let prev = prev.unwrap_or(Prev { vp, world_vp: world_vp * f.world_step, hands: f.hands });
    let mut cb = FxCb::zeroed();
    cb.vp = vp.to_cols_array();
    cb.inv_vp = vp.inverse().to_cols_array();
    cb.prev_vp = prev.vp.to_cols_array();
    cb.world_inv_vp = world_vp.inverse().to_cols_array();
    // A point of the world now, where it was last frame (the world's walk and turn undone) as the
    // world's camera saw it then.
    cb.world_prev = (prev.world_vp * f.world_step.inverse()).to_cols_array();
    for h in 0..2 {
        // A point a hand carries, where it was last frame: in the hand's frame now, placed by the
        // hand then.
        let (m, valid) = match (f.hands[h], prev.hands[h]) {
            (Some(now), Some(then)) => (then * now.inverse(), 1.0),
            _ => (Mat4::IDENTITY, 0.0),
        };
        cb.hand_prev[h] = m.to_cols_array();
        cb.hands[h] = f.hands[h].map_or(Vec3::ZERO, |m| m.w_axis.truncate()).extend(valid).to_array();
    }
    cb.world_cam = Vec3::from_array(f.world.pos).extend(f.metres_per_unit).to_array();
    cb.world_fwd = (world_rot * Vec3::Z).extend(if f.game_depth.is_some() { 1.0 } else { 0.0 }).to_array();
    cb.body_axis = f.body_axis.extend(if f.body_drawn { 1.0 } else { 0.0 }).to_array();
    let (w, h) = (t.width as f32, t.height as f32);
    cb.size = [w, h, 1.0 / w, 1.0 / h];
    // The shutter: open `shutter` degrees of a frame of the recording, against this frame's time.
    let exposure = s.shutter.clamp(0.0, 360.0) / 360.0 / s.recorded_fps.max(1.0);
    cb.blur = [exposure / f.dt.max(1e-3), s.camera_blur.max(0.0), s.centre_sharp.clamp(0.0, 1.0), s.object_blur.max(0.0)];
    cb.tiles = [t.tile as f32, f.fov.right, (t.width.div_ceil(t.tile)) as f32, (t.height.div_ceil(t.tile)) as f32];
    // Depth of field: the farthest blur (a radius, pixels), in front its share, and the focus's
    // pull this frame (most of the way over `focus_seconds`).
    let pull = 1.0 - (-f.dt / (s.focus_seconds.max(0.02) / 3.0)).exp();
    cb.dof = [s.dof_blur.max(0.0) * 0.01 * w * 0.5, s.dof_near.clamp(0.0, 1.0), pull, f.focus.1];
    cb.focus_uv = [f.focus.0.x, f.focus.0.y, if s.dof { 1.0 } else { 0.0 }, 0.0];
    cb.lens = [s.fisheye.clamp(0.0, 1.0), s.chromatic.clamp(0.0, 1.0), s.vignette.clamp(0.0, 1.0), s.sharpen.clamp(0.0, 1.0)];
    cb.film = [s.grain.clamp(0.0, 1.0), seconds, s.glow.max(0.0), if s.glow > 0.001 { 1.0 } else { 0.0 }];
    // White balance: warmer is redder and less blue, greener is a green tint; its brightness kept.
    let wb = Vec3::new(1.0 + 0.12 * s.temperature, 1.0 + 0.08 * s.tint, 1.0 - 0.12 * s.temperature);
    let wb = wb / wb.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(0.1);
    cb.colour = [2f32.powf(s.exposure), s.contrast.max(0.1), s.saturation.max(0.0), if lut.is_some() { s.lut_mix.clamp(0.0, 1.0) } else { 0.0 }];
    cb.white = [wb.x, wb.y, wb.z, s.grade_mix.clamp(0.0, 1.0)];
    let grade = match s.grade {
        Grade::None => 0.0,
        Grade::TealOrange => 1.0,
        Grade::BleachBypass => 2.0,
        Grade::WarmFilm => 3.0,
        Grade::Bodycam => 4.0,
        Grade::Noir => 5.0,
    };
    cb.grade = [grade, lut.map_or(0.0, |l| l.1 as f32), 0.0, 0.0];
    kit.cb.update(&gpu.ctx, &cb);
    let srv = |t: &Texture| t.srv.clone();
    let mut colour = &t.scene;
    if s.motion_blur || s.dof {
        pass_into(gpu, kit, PASS_VELOCITY, &t.velocity, &[srv(&t.body_depth), f.game_depth.clone()]);
    }
    if s.motion_blur {
        pass_into(gpu, kit, PASS_TILE_MAX, &t.tile_max, &[None, srv(&t.velocity)]);
        pass_into(gpu, kit, PASS_NEIGHBOUR_MAX, &t.neighbour_max, &[None, None, srv(&t.tile_max)]);
        pass_into(gpu, kit, PASS_MOTION_BLUR, &t.work, &[srv(colour), srv(&t.velocity), srv(&t.neighbour_max)]);
        colour = &t.work;
    }
    if s.dof
        && let [Some(now), Some(was)] = focus
    {
        pass_into(gpu, kit, PASS_FOCUS, now, &[None, srv(&t.velocity), None, srv(was)]);
        pass_into(gpu, kit, PASS_DOF_DOWN, &t.dof_a, &[srv(colour), srv(&t.velocity), None, srv(now)]);
        pass_into(gpu, kit, PASS_DOF_BLUR, &t.dof_b, &[srv(&t.dof_a)]);
    }
    if s.glow > 0.001 {
        pass_into(gpu, kit, PASS_GLOW_DOWN, &t.glow_a, &[srv(colour)]);
        pass_into(gpu, kit, PASS_GLOW_ACROSS, &t.glow_b, &[srv(&t.glow_a)]);
        pass_into(gpu, kit, PASS_GLOW_DOWNWARDS, &t.glow_a, &[srv(&t.glow_b)]);
    }
    unsafe {
        gpu.ctx.OMSetRenderTargets(Some(&[Some(f.window.rtv.clone())]), None);
        gpu.ctx.OMSetBlendState(&kit.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
    }
    let dof = if s.dof { srv(&t.dof_b) } else { None };
    let glow = if s.glow > 0.001 { srv(&t.glow_a) } else { None };
    draw_pass(gpu, kit, PASS_FINAL, f.window.width, f.window.height, &[srv(colour), dof, None, None, glow, lut.and_then(|l| srv(&l.0))]);
    unsafe { gpu.ctx.OMSetRenderTargets(None, None) };
}

/// The flat HUD's pieces over the window, from the HUD's picture (premultiplied by its alpha, or
/// added where it has none).
fn draw_pieces(gpu: &Gpu, kit: &Kit, window: &EyeTarget, srv: &ID3D11ShaderResourceView, pieces: &[FlatPiece], additive: bool) {
    let ctx = &gpu.ctx;
    let (w, h) = (window.width as f32, window.height as f32);
    unsafe {
        ctx.OMSetRenderTargets(Some(&[Some(window.rtv.clone())]), None);
        ctx.OMSetBlendState(if additive { &kit.additive } else { &kit.premul }, Some(&[0.0; 4]), 0xFFFF_FFFF);
        ctx.RSSetState(&kit.raster);
        ctx.OMSetDepthStencilState(&kit.no_depth, 0);
        ctx.PSSetSamplers(0, Some(&[Some(kit.linear.clone()), Some(kit.point.clone())]));
        ctx.VSSetConstantBuffers(1, Some(&[Some(kit.piece_cb.buf.clone())]));
        ctx.PSSetConstantBuffers(1, Some(&[Some(kit.piece_cb.buf.clone())]));
        ctx.IASetInputLayout(None);
        ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        pipeline::viewport(ctx, window.width, window.height);
        kit.piece.bind(ctx);
        ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
        for p in pieces {
            let ndc = |x: f32, y: f32| [x / w * 2.0 - 1.0, 1.0 - y / h * 2.0];
            let (a, b) = (ndc(p.rect[0], p.rect[1]), ndc(p.rect[2], p.rect[3]));
            kit.piece_cb.update(ctx, &PieceCb { rect: [a[0], a[1], b[0], b[1]], uv: p.uv, mode: [if additive { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0] });
            ctx.Draw(4, 0);
        }
        ctx.PSSetShaderResources(0, Some(&[None]));
        ctx.OMSetRenderTargets(None, None);
    }
}

// ---- the panel ----------------------------------------------------------------------------------

fn slider(ui: &mut egui::Ui, label: &str, v: &mut f32, lo: f32, hi: f32, suffix: &str, decimals: usize) {
    ui.add(egui::Slider::new(v, lo..=hi).text(label).suffix(suffix).fixed_decimals(decimals));
}

/// The menu's Studio tab: the looks; what's for viewers; the desktop camera (its field of view,
/// the overscan that gives it room, its bob as the player walks); then every effect with its own
/// strengths. (The desktop view's eye and smoothing stay on the Display tab; the aim zoom's goes in
/// the Camera section here once it's merged.)
pub fn panel(ui: &mut egui::Ui, s: &mut Settings, status: &crate::menu::Status) {
    ui.label("Effects only the desktop window shows, for recording and streaming: never the headset.");
    ui.horizontal_wrapped(|ui| {
        ui.label("Look");
        for (l, name) in [(Look::Clean, "Clean"), (Look::Cinematic, "Cinematic"), (Look::HyperReal, "Hyper-real"), (Look::Action, "Action"), (Look::Vintage, "Vintage")] {
            if ui.selectable_label(s.studio.look == l, name).clicked() {
                s.studio = look(l, &s.studio);
            }
        }
        if s.studio.look == Look::Custom {
            ui.label(egui::RichText::new("(custom)").weak());
        }
    });
    let before = s.studio.clone();
    let st = &mut s.studio;
    ui.separator();
    ui.label("For viewers");
    ui.checkbox(&mut st.smooth_snaps, "Snap turns, snap crouches and recentres turn as a head would, instead of cutting");
    if st.smooth_snaps {
        slider(ui, "A 45 degree snap settles in", &mut st.snap_seconds, 0.15, 0.8, " s", 2);
    }
    ui.checkbox(&mut st.fill_edges, "Stretch the picture's edges over the gaps a fast turn leaves (instead of black)");
    ui.checkbox(&mut st.flat_hud, "The game's HUD on the screen where a flat player has it");
    if st.flat_hud {
        slider(ui, "HUD size", &mut st.flat_hud_size, 0.5, 2.0, "", 2);
    }
    ui.separator();
    ui.label("Camera");
    // One field of view: the look's lens where it has one (changing it makes the look custom),
    // else the desktop view's own.
    let mut deg = if st.lens_fov > 1.0 { st.lens_fov } else { s.spectator.fov };
    slider(ui, "Desktop field of view", &mut deg, 60.0, 140.0, " deg", 0);
    if st.lens_fov > 1.0 {
        st.lens_fov = deg;
        ui.label(egui::RichText::new("(this look's lens; the Clean look uses your own)").weak());
    } else {
        s.spectator.fov = deg;
    }
    // (More draws more, in the headset too: for a wider desktop view, in games that can spare it.)
    slider(ui, "Overscan: the game drawn past the headset's view", &mut s.vr.overscan, 0.0, 30.0, " deg", 0);
    if s.vr.overscan - status.overscan_shown > 0.5 {
        ui.label(egui::RichText::new(format!("The game can't change its size while it runs: {:.0} of the {:.0} degrees show now", status.overscan_shown, s.vr.overscan)).weak());
    }
    ui.horizontal_wrapped(|ui| {
        use crate::settings::BobMode;
        ui.label("Bobs as you walk (never the headset)");
        ui.radio_value(&mut s.desktop_bob.mode, BobMode::Off, "off");
        ui.radio_value(&mut s.desktop_bob.mode, BobMode::Generic, "the runtime's");
        ui.radio_value(&mut s.desktop_bob.mode, BobMode::Game, "the game's own");
    });
    if s.desktop_bob.mode != crate::settings::BobMode::Off {
        slider(ui, "Bob strength", &mut s.desktop_bob.strength, 0.0, 3.0, "x", 2);
    }
    ui.checkbox(&mut st.motion_blur, "Motion blur");
    if st.motion_blur {
        slider(ui, "Shutter", &mut st.shutter, 0.0, 360.0, " deg", 0);
        slider(ui, "Recorded at", &mut st.recorded_fps, 12.0, 120.0, " fps", 0);
        slider(ui, "Turning and walking", &mut st.camera_blur, 0.0, 2.0, "", 2);
        slider(ui, "Middle kept sharp", &mut st.centre_sharp, 0.0, 1.0, "", 2);
        slider(ui, "Hands and what they hold", &mut st.object_blur, 0.0, 2.0, "", 2);
    }
    ui.checkbox(&mut st.speed_fov, "Wider at speed");
    if st.speed_fov {
        slider(ui, "Wider by", &mut st.speed_fov_deg, 0.0, 30.0, " deg", 0);
        slider(ui, "From", &mut st.speed_from, 0.0, 10.0, " m/s", 1);
        slider(ui, "All of it at", &mut st.speed_full, 1.0, 30.0, " m/s", 1);
    }
    ui.checkbox(&mut st.shake, "Shake with hits, shots, explosions and landings");
    if st.shake {
        slider(ui, "Shake", &mut st.shake_strength, 0.0, 3.0, "", 2);
    }
    slider(ui, "Handheld sway", &mut st.handheld, 0.0, 1.0, "", 2);
    slider(ui, "Worn on the chest", &mut st.chest_mount, 0.0, 1.0, "", 2);
    ui.checkbox(&mut st.dof, "Depth of field (focus on what you look at, aim at or hold up)");
    if st.dof {
        slider(ui, "Blur behind", &mut st.dof_blur, 0.1, 4.0, " %", 1);
        slider(ui, "Blur in front", &mut st.dof_near, 0.0, 1.0, "", 2);
        slider(ui, "Focus pull", &mut st.focus_seconds, 0.05, 1.5, " s", 2);
    }
    ui.separator();
    ui.label("Lens and film");
    slider(ui, "Fisheye", &mut st.fisheye, 0.0, 1.0, "", 2);
    slider(ui, "Colour fringes", &mut st.chromatic, 0.0, 1.0, "", 2);
    slider(ui, "Vignette", &mut st.vignette, 0.0, 1.0, "", 2);
    slider(ui, "Glow", &mut st.glow, 0.0, 1.5, "", 2);
    slider(ui, "Grain", &mut st.grain, 0.0, 1.0, "", 2);
    slider(ui, "Sharpen", &mut st.sharpen, 0.0, 1.0, "", 2);
    ui.separator();
    ui.label("Colour");
    slider(ui, "Exposure", &mut st.exposure, -2.0, 2.0, " stops", 2);
    slider(ui, "Contrast", &mut st.contrast, 0.5, 1.5, "", 2);
    slider(ui, "Saturation", &mut st.saturation, 0.0, 2.0, "", 2);
    slider(ui, "Temperature", &mut st.temperature, -1.0, 1.0, "", 2);
    slider(ui, "Tint", &mut st.tint, -1.0, 1.0, "", 2);
    ui.horizontal_wrapped(|ui| {
        ui.label("Grade");
        for (g, name) in [(Grade::None, "none"), (Grade::TealOrange, "teal & orange"), (Grade::BleachBypass, "bleach bypass"), (Grade::WarmFilm, "warm film"), (Grade::Bodycam, "body cam"), (Grade::Noir, "noir")] {
            ui.radio_value(&mut st.grade, g, name);
        }
    });
    if st.grade != Grade::None {
        slider(ui, "Grade strength", &mut st.grade_mix, 0.0, 1.0, "", 2);
    }
    let files = luts();
    ui.horizontal_wrapped(|ui| {
        ui.label("LUT");
        ui.radio_value(&mut st.lut, String::new(), "none");
        for name in &files {
            ui.radio_value(&mut st.lut, name.clone(), name.trim_end_matches(".cube"));
        }
    });
    if files.is_empty() {
        ui.label(egui::RichText::new(format!("(.cube files in {} show here)", luts_dir().display())).weak());
    }
    if !st.lut.is_empty() {
        slider(ui, "LUT strength", &mut st.lut_mix, 0.0, 1.0, "", 2);
    }
    // Anything changed by hand that the look doesn't have: a look of its own.
    if *st != before && st.look != Look::Custom && *st != look(st.look, st) {
        st.look = Look::Custom;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(yaw: f32, anchor: Vec3) -> WorldMap {
        WorldMap { origin: Vec3::ZERO, body_yaw: yaw, world_per_meter: 1.0, anchor }
    }

    #[test]
    fn a_snap_turn_is_a_jump_and_a_steady_turn_isnt() {
        let mut g = Ground::default();
        let at = Vec3::new(0.0, 1.7, 0.0);
        assert_eq!(g.see(1, map(0.0, Vec3::ZERO), at, 0.0).0, Step::Cut, "the first picture");
        // Turning at 90 degrees a second, a picture every 22 ms: steady.
        let mut yaw = 0.0;
        for i in 2..40 {
            yaw += 90f32.to_radians() * 0.022;
            let (step, _) = g.see(i, map(yaw, Vec3::ZERO), at, i as f64 * 0.022);
            assert_eq!(step, Step::Steady, "picture {i}");
        }
        assert!((g.turn.abs() - 90f32.to_radians()).abs() < 0.2, "{}", g.turn);
        // A snap of 45 degrees on top of it.
        yaw += 45f32.to_radians();
        assert_eq!(g.see(40, map(yaw, Vec3::ZERO), at, 40.0 * 0.022).0, Step::Jump);
        // The same picture again is nothing new.
        assert_eq!(g.see(40, map(yaw, Vec3::ZERO), at, 40.5 * 0.022).0, Step::Steady);
    }

    #[test]
    fn walking_is_steady_and_a_crouch_or_a_load_isnt() {
        let mut g = Ground::default();
        let at = Vec3::new(0.0, 1.7, 0.0);
        g.see(1, map(0.0, Vec3::ZERO), at, 0.011);
        let mut z = 0.0;
        for i in 2..30 {
            z += 6.0 * 0.011;
            assert_eq!(g.see(i, map(0.0, Vec3::new(0.0, 0.0, z)), at, i as f64 * 0.011).0, Step::Steady);
        }
        assert!((g.walk.length() - 6.0).abs() < 0.5, "{:?}", g.walk);
        // Held up a tenth of a second (the runtime's frame was), the game walked on: still steady.
        z += 6.0 * 0.1;
        assert_eq!(g.see(30, map(0.0, Vec3::new(0.0, 0.0, z)), at, 29.0 * 0.011 + 0.1).0, Step::Steady);
        // The game held up, its picture late, and its whole step in the next one a moment after: a
        // hitch, carried on.
        z += 6.0 * 0.1;
        assert_eq!(g.see(31, map(0.0, Vec3::new(0.0, 0.0, z)), at, 30.0 * 0.011 + 0.1).0, Step::Steady);
        // Down half a metre at once: a crouch.
        assert_eq!(g.see(32, map(0.0, Vec3::new(0.0, -0.5, z + 0.066)), at, 31.0 * 0.011 + 0.1).0, Step::Jump);
        // A step to the side (a recentre) is a jump, walking or not.
        assert_eq!(g.see(33, map(0.0, Vec3::new(0.8, -0.5, z + 0.132)), at, 32.0 * 0.011 + 0.1).0, Step::Jump);
        // Another place.
        assert_eq!(g.see(34, map(0.0, Vec3::new(500.0, 0.0, 0.0)), at, 33.0 * 0.011 + 0.1).0, Step::Cut);
    }

    #[test]
    fn a_hard_landing_is_seen() {
        let mut g = Ground::default();
        let at = Vec3::new(0.0, 1.7, 0.0);
        g.see(1, map(0.0, Vec3::ZERO), at, 0.011);
        // Falling ever faster to 9 m/s, a picture every 22 ms, then stopped.
        let (mut y, mut v) = (0.0, 0.0);
        for i in 2..60 {
            v = (v + 9.8 * 0.022f32).min(9.0);
            y -= v * 0.022;
            g.see(i, map(0.0, Vec3::new(0.0, y, 0.0)), at, i as f64 * 0.022);
        }
        let (step, landed) = g.see(60, map(0.0, Vec3::new(0.0, y, 0.0)), at, 60.0 * 0.022);
        assert_eq!(step, Step::Steady);
        assert!(landed.is_some_and(|s| s > 7.0), "{landed:?}");
    }

    #[test]
    fn a_snap_turns_as_a_head_does() {
        let deg = |r: f32| r.to_degrees();
        let snap = 45f32.to_radians();
        let mut e = Ease::jump(None, snap, Vec3::ZERO, 0.3, false);
        // It gathers speed: a frame in it has hardly moved.
        e.step(1.0 / 90.0, 0.3);
        assert!(deg(e.yaw) > 44.0, "{}", deg(e.yaw));
        let (mut t, mut half, mut most_back, mut fastest) = (1.0 / 90.0, None, 0.0f32, 0.0f32);
        while e.step(1.0 / 90.0, 0.3) {
            t += 1.0 / 90.0;
            if half.is_none() && e.yaw < snap * 0.5 {
                half = Some(t);
            }
            most_back = most_back.max(-e.yaw);
            fastest = fastest.max(e.speed.abs());
            assert!(t < 0.6, "settled");
        }
        let half = half.unwrap();
        assert!((0.08..0.15).contains(&half), "half way at {half} s");
        assert!(deg(most_back) > 0.3 && deg(most_back) < 2.0, "lands with a little overshoot: {}", deg(most_back));
        assert!(deg(fastest) > 200.0 && deg(fastest) < 450.0, "as fast as a head: {} deg/s", deg(fastest));
        // A second snap mid-turn carries the first's momentum.
        let mut e = Ease::jump(None, snap, Vec3::ZERO, 0.3, false);
        for _ in 0..9 {
            e.step(1.0 / 90.0, 0.3);
        }
        let speed = e.speed;
        let e = Ease::jump(Some(e), snap, Vec3::ZERO, 0.3, false);
        assert!(e.speed == speed && speed.abs() > 1.0, "{speed}");
    }

    #[test]
    fn a_bob_turns_the_world_and_never_moves_the_camera() {
        let look = Quat::from_rotation_y(0.7);
        let cam = Pose { pos: [0.0, 1.7, 0.0], valid: 1, rot: look.to_array() };
        // The camera dropped 2 cm (a step): the world's camera tips down, by what 2 cm is at 2.5 m.
        let down = Pose { pos: [0.0, 1.68, 0.0], ..cam };
        let t = bob_turn(&cam, &down);
        let fwd = t * Vec3::Z;
        assert!((fwd.y - (-0.02f32 / BOB_SEEN_AT).atan().sin()).abs() < 1e-4, "{fwd:?}");
        assert!(fwd.x.abs() < 1e-5);
        // Sideways to the right: turned right.
        let right = Pose { pos: (Vec3::from_array(cam.pos) + look * Vec3::X * 0.03).to_array(), ..cam };
        assert!((bob_turn(&cam, &right) * Vec3::Z).x > 0.0);
        // No bob: no turn.
        assert!(bob_turn(&cam, &cam).angle_between(Quat::IDENTITY) < 1e-5);
    }

    #[test]
    fn the_shake_wears_off_and_kicks_spring_back() {
        let mut s = Shake::default();
        s.add(1.0, Vec3::new(-2.0, 0.0, 0.0));
        let mut most: f32 = 0.0;
        for _ in 0..30 {
            most = most.max(s.update(0.011, 1.0, 0.0).abs().max_element());
        }
        assert!(most > 1.0, "it shook: {most}");
        let mut last = Vec3::ZERO;
        for _ in 0..200 {
            last = s.update(0.011, 1.0, 0.0);
        }
        assert!(last.abs().max_element() < 0.01, "and stopped: {last:?}");
    }

    #[test]
    fn the_flat_hud_keeps_its_pieces_by_their_sides_at_a_flat_games_size() {
        // A tall eye's HUD (as Oblivion draws it for a headset), its text a 36th of its height.
        let mut l = HudLayout::new(
            HUDL_ALPHA | HUDL_REST_HIDDEN,
            &[
                HudElement::new(HUD_STATUS, [0.02, 0.85, 0.30, 0.97]),
                HudElement::new(HUD_STATUS, [0.40, 0.90, 0.60, 0.97]),
                HudElement::new(HUD_SUBTITLES, [0.20, 0.70, 0.80, 0.78]),
                HudElement::new(HUD_HIDDEN, [0.48, 0.48, 0.52, 0.52]),
            ],
        );
        l.text_height = 36.0 / 1376.0;
        let area = [0.0, 0.0, 1920.0, 1080.0];
        let p = flat_layout(&l, 0.93, area, 1.0);
        assert_eq!(p.len(), 3, "the crosshair stays hidden");
        let k = FLAT_TEXT / l.text_height;
        // The bars at the bottom left: as far from the left and the bottom as the game has them.
        assert!((p[0].rect[0] - 0.02 * 0.93 * 1080.0 * k).abs() < 0.5, "{:?}", p[0].rect);
        assert!((1080.0 - p[0].rect[3] - 0.03 * 1080.0 * k).abs() < 0.5, "{:?}", p[0].rect);
        // The compass in the middle at the bottom.
        assert!(((p[1].rect[0] + p[1].rect[2]) * 0.5 - 960.0).abs() < 0.5);
        // A line of its text as tall as a flat game's.
        let line = l.text_height * (p[2].rect[3] - p[2].rect[1]) / (0.78 - 0.70);
        assert!((line / 1080.0 - FLAT_TEXT).abs() < 1e-4, "{line}");
        // Without pieces, nothing (the rest is hidden).
        let empty = HudLayout::new(HUDL_REST_HIDDEN, &[]);
        assert!(flat_layout(&empty, 0.93, area, 1.0).is_empty());
    }

    #[test]
    fn the_looks_keep_the_viewers_options_and_a_hand_change_makes_it_custom() {
        let mut keep = Studio { flat_hud: true, smooth_snaps: false, ..Studio::default() };
        let c = look(Look::Cinematic, &keep);
        assert!(c.flat_hud && !c.smooth_snaps && c.motion_blur && c.dof);
        assert!(draws_effects(&c, false));
        keep.look = Look::Clean;
        assert!(!draws_effects(&look(Look::Clean, &keep), false), "the clean look draws nothing over the picture");
        for l in [Look::Clean, Look::Cinematic, Look::HyperReal, Look::Action, Look::Vintage] {
            assert_eq!(look(l, &keep).look, l);
        }
    }

    #[test]
    fn a_cube_lut_is_read() {
        let text = "TITLE \"x\"\n# comment\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let (n, rgb) = parse_cube(text).unwrap();
        assert_eq!(n, 2);
        assert_eq!(rgb[1], [1.0, 0.0, 0.0]);
        assert_eq!(rgb[7], [1.0, 1.0, 1.0]);
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err());
    }
}
