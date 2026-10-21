//! Headless end-to-end scenarios: a scripted player in the simulated headset, the test range (or
//! no game at all) on the other side of the real protocol, checks on what both sides did, and
//! PNG captures of what the headset would have shown.
//!
//! `rustvr --sim <name> --launch-range` runs one; `rustvr --sim all --launch-range` runs them all.

use anyhow::Result;
use glam::{Mat3, Quat, Vec2, Vec3};
use rustvr_gpu::Gpu;
use rustvr_protocol::range_stats as rs;
use rustvr_protocol::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::backend::sim::{SimBackend, SimDriver, SimHand, SimInput};
use crate::mechanics::{MagState, MechDebug, MechOutput, default_rig, mag_well, sight_line};
use crate::rig::{ObjSnap, RigDebug};
use crate::session::{Options, Session, Stats, World};
use crate::settings::{ReloadStyle, Settings, SettingsStore, TurnMode};

pub const HZ: f32 = 90.0;
const IPD: f32 = 0.064;

/// The player's body at one moment.
#[derive(Clone, Copy)]
pub struct Body {
    pub head_pos: Vec3,
    pub head_rot: Quat,
    pub hands: [SimHand; 2],
}

impl Body {
    /// Standing upright, facing +Z, hands low in front, clear of every gesture zone.
    pub fn rest() -> Body {
        let hand = |x: f32| SimHand { pos: Vec3::new(x, 1.05, 0.40), ..Default::default() };
        Body { head_pos: Vec3::new(0.0, 1.7, 0.0), head_rot: Quat::IDENTITY, hands: [hand(-0.25), hand(0.25)] }
    }

    pub fn eye(&self, e: usize) -> Vec3 {
        let side = if e == 0 { -0.5 } else { 0.5 };
        self.head_pos + self.head_rot * Vec3::new(side * IPD, 0.0, 0.0)
    }

    fn input(&self) -> SimInput {
        SimInput { head_pos: self.head_pos, head_rot: self.head_rot, hands: self.hands }
    }
}

/// A rotation whose +Z is `fwd`, keeping +Y as close to world up as possible.
pub fn look_rot(fwd: Vec3) -> Quat {
    let z = fwd.normalize();
    let x = Vec3::Y.cross(z).normalize_or(Vec3::X);
    let y = z.cross(x);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

/// Where the firing hand goes so the gun's line of sight passes through `eye` towards `target`
/// (default weapon settings). Returns the hand pose.
pub fn aim_hand(rig: &WeaponRig, eye: Vec3, target: Vec3) -> (Vec3, Quat) {
    let (sight, _) = sight_line(rig);
    let d = (target - eye).normalize();
    let r = look_rot(d);
    let grip = Vec3::from_array(rig.grip);
    // The sight origin sits 15 cm in front of the eye, on the line.
    let pos = eye + d * 0.15 - r * (sight - grip);
    (pos, r)
}

/// A weapon-space point on the gun held by a hand at `pose`.
pub fn on_gun(rig: &WeaponRig, hand: (Vec3, Quat), p: Vec3) -> Vec3 {
    hand.0 + hand.1 * (p - Vec3::from_array(rig.grip))
}

pub fn lerp_body(a: &Body, b: &Body, k: f32) -> Body {
    let k = k.clamp(0.0, 1.0);
    let mut out = *a;
    out.head_pos = a.head_pos.lerp(b.head_pos, k);
    out.head_rot = a.head_rot.slerp(b.head_rot, k);
    for h in 0..2 {
        out.hands[h].pos = a.hands[h].pos.lerp(b.hands[h].pos, k);
        out.hands[h].rot = a.hands[h].rot.slerp(b.hands[h].rot, k);
    }
    out
}

/// 0 before `t0`, 1 after `t1`, smooth in between.
pub fn ramp(t: f64, t0: f64, t1: f64) -> f32 {
    let x = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) as f32;
    x * x * (3.0 - 2.0 * x)
}

pub fn between(t: f64, t0: f64, t1: f64) -> bool {
    t >= t0 && t < t1
}

/// What the scripted body can see: the runtime's panels and the game's last state.
pub struct See<'a> {
    pub world: &'a World,
    pub game: Option<&'a GameState>,
}

impl See<'_> {
    pub fn rig(&self) -> WeaponRig {
        match self.game {
            Some(g) if g.flags & GS_HAS_WEAPON != 0 => g.rig,
            _ => default_rig(),
        }
    }

    pub fn widget(&self, name: &str) -> Option<Vec3> {
        let p = self.world.runtime_menu?;
        let uv = self.world.widgets.iter().find(|(n, _)| n == name)?.1;
        Some(p.point(uv))
    }
}

pub type InputFn = Box<dyn FnMut(f64, &See) -> Body + Send>;

/// What a check can look at, right after the frame at time `t`.
pub struct Ctx<'a> {
    pub t: f64,
    pub rig: &'a RigDebug,
    pub objects: &'a [ObjSnap],
    /// The body the runtime solved (and drew, if `draws_body`).
    pub body: &'a RtBody,
    pub draws_body: bool,
    pub reached: [bool; 2],
    /// The game's floor in tracking space.
    pub floor: f32,
    pub game: Option<&'a GameState>,
    pub mech: &'a MechDebug,
    pub out: &'a MechOutput,
    pub stats: &'a Stats,
    pub settings: &'a Settings,
    pub turn_total: f32,
    pub haptics: usize,
    pub frames_run: u64,
    pub captures: &'a [Capture],
}

impl Ctx<'_> {
    pub fn stat(&self, i: usize) -> u32 {
        self.game.map(|g| g.stats[i]).unwrap_or(0)
    }

    pub fn stat_f(&self, i: usize) -> f32 {
        self.game.map(|g| g.stats_f[i]).unwrap_or(0.0)
    }

    pub fn capture(&self, name: &str) -> Option<&Capture> {
        self.captures.iter().find(|c| c.name == name)
    }
}

type CheckFn = Box<dyn Fn(&Ctx) -> std::result::Result<(), String>>;

pub struct Check {
    pub at: f64,
    pub what: &'static str,
    pub test: CheckFn,
}

#[derive(Clone, Debug)]
pub struct Capture {
    pub name: String,
    pub path: PathBuf,
    /// Mean brightness 0..1 over both eyes.
    pub mean: f32,
    /// Fraction of pixels brighter than near-black.
    pub lit: f32,
}

pub struct Scenario {
    pub name: &'static str,
    pub about: &'static str,
    pub duration: f64,
    /// Runs against the test range (false: the runtime on its own).
    pub needs_game: bool,
    /// The runtime waits for the game every frame (deterministic). False: real time, both sides at
    /// their own pace, as with a real headset.
    pub lockstep: bool,
    pub range_args: Vec<&'static str>,
    /// The range's objects go to the runtime's hands (false: `--legacy`, the gun glued to the
    /// firing hand and driven by intents).
    pub physical: bool,
    /// The player's pose while the range starts up (and recentres on them).
    pub start: fn() -> Body,
    pub settings: fn(&mut Settings),
    pub input: fn() -> InputFn,
    pub checks: Vec<Check>,
    pub captures: Vec<(f64, &'static str)>,
}

pub fn check(at: f64, what: &'static str, test: impl Fn(&Ctx) -> std::result::Result<(), String> + 'static) -> Check {
    Check { at, what, test: Box::new(test) }
}

pub fn ensure(ok: bool, msg: impl FnOnce() -> String) -> std::result::Result<(), String> {
    if ok { Ok(()) } else { Err(msg()) }
}

/// A target the range places straight ahead at eye height (see the range's world layout).
pub const TARGET_AHEAD: Vec3 = Vec3::new(0.0, 1.6, 10.0);

pub fn all() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "smoke",
            about: "connect, look around, the game picture reaches both eyes every frame",
            duration: 2.5,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    b.head_rot = Quat::from_rotation_y(0.6 * (t as f32 * 2.0).sin()) * Quat::from_rotation_x(0.1);
                    b
                })
            },
            checks: vec![
                check(2.4, "every frame showed the game", |c| {
                    ensure(c.stats.game_frames as f64 >= c.frames_run as f64 * 0.98, || format!("{} of {}", c.stats.game_frames, c.frames_run))
                }),
                check(2.4, "the game rendered each exact headset frame (lockstep)", |c| {
                    ensure(c.stats.exact_frames as f64 >= c.frames_run as f64 * 0.98, || format!("{} of {}", c.stats.exact_frames, c.frames_run))
                }),
                check(2.4, "no lockstep timeouts", |c| ensure(c.stats.lockstep_timeouts == 0, || c.stats.lockstep_timeouts.to_string())),
                check(2.4, "the picture is lit", |c| {
                    let cap = c.capture("front").ok_or("no capture")?;
                    ensure(cap.lit > 0.5 && cap.mean > 0.05, || format!("mean {:.3} lit {:.2}", cap.mean, cap.lit))
                }),
            ],
            captures: vec![(0.3, "front"), (1.0, "turned")],
        },
        Scenario {
            name: "aim",
            about: "raise the gun to the eye (auto aim-down-sights) and hit the target ahead",
            duration: 3.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, see| {
                    let rest = Body::rest();
                    let mut aimed = rest;
                    let rig = see.rig();
                    let (p, r) = aim_hand(&rig, rest.eye(1), TARGET_AHEAD);
                    aimed.hands[1].pos = p;
                    aimed.hands[1].rot = r;
                    let mut b = lerp_body(&rest, &aimed, ramp(t, 0.1, 0.8));
                    b.hands[1].trigger = if between(t, 1.2, 1.4) { 1.0 } else { 0.0 };
                    b
                })
            },
            checks: vec![
                check(1.0, "sights at the eye: aiming down sights", |c| {
                    ensure(c.mech.ads && c.mech.sight_to_eye < 0.06, || format!("ads {} sight_to_eye {:.3}", c.mech.ads, c.mech.sight_to_eye))
                }),
                check(2.0, "the game fired", |c| ensure(c.stat(rs::SHOTS) >= 1, || format!("shots {}", c.stat(rs::SHOTS)))),
                check(2.0, "and hit the target", |c| {
                    ensure(c.stat(rs::HITS) >= 1, || format!("hits {} (miss by {:.2} m)", c.stat(rs::HITS), c.stat_f(rs::F_LAST_HIT_ERROR)))
                }),
                check(2.0, "shots follow the gun, not the head", |c| {
                    ensure(c.stat_f(rs::F_SHOT_AIM_ERROR) < 0.5, || format!("{:.2} degrees off", c.stat_f(rs::F_SHOT_AIM_ERROR)))
                }),
            ],
            captures: vec![(1.0, "ads")],
        },
        Scenario {
            name: "two_handed",
            about: "grab the foregrip with the off hand and steer the barrel with it",
            duration: 2.5,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, see| {
                    let mut b = Body::rest();
                    let rig = see.rig();
                    let hand = aim_hand(&rig, b.eye(1), TARGET_AHEAD);
                    let hand = (hand.0 + Vec3::new(0.0, -0.12, 0.0), hand.1); // hip-ish, below the eye line
                    b.hands[1].pos = hand.0;
                    b.hands[1].rot = hand.1;
                    let fore = on_gun(&rig, hand, Vec3::from_array(rig.foregrip));
                    let lift = Vec3::new(0.0, 0.10 * ramp(t, 1.2, 1.8), 0.0);
                    let off = lerp_body(&Body::rest(), &Body { hands: [SimHand { pos: fore + lift, ..b.hands[0] }, b.hands[1]], ..b }, ramp(t, 0.2, 0.7));
                    b.hands[0] = off.hands[0];
                    b.hands[0].squeeze = if t > 0.75 { 1.0 } else { 0.0 };
                    b
                })
            },
            checks: vec![
                check(1.0, "the off hand holds the foregrip", |c| ensure(c.mech.two_handed, || "not two-handed".into())),
                check(2.2, "lifting the off hand tilts the barrel up", |c| {
                    let w = c.out.weapon.ok_or("no weapon")?;
                    ensure(w.aim_dir.y > 0.05, || format!("aim {:?}", w.aim_dir))
                }),
                check(2.2, "the grab buzzed the off hand", |c| ensure(c.haptics > 0, || "no haptics".into())),
            ],
            captures: vec![(1.0, "two_handed")],
        },
        Scenario {
            name: "reload",
            about: "Titanfall-style reload: eject, grab a magazine at the hip, seat it, rack the charging handle",
            duration: 6.0,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--weapon", "Range rifle"],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, see| {
                    let mut b = Body::rest();
                    let rig = see.rig();
                    let hand = aim_hand(&rig, b.eye(1), TARGET_AHEAD);
                    let hand = (hand.0 + Vec3::new(0.0, -0.15, 0.05), hand.1);
                    b.hands[1].pos = hand.0;
                    b.hands[1].rot = hand.1;
                    b.hands[1].trigger = if between(t, 0.3, 1.0) { 1.0 } else { 0.0 };
                    b.hands[1].buttons = if between(t, 1.2, 1.3) { CTRL_A } else { 0 };
                    let rest = Body::rest().hands[0].pos;
                    let hip = Vec3::new(-0.25, 0.85, 0.05);
                    let well = on_gun(&rig, hand, mag_well(&rig)) + Vec3::new(0.0, -0.02, 0.0);
                    let handle = on_gun(&rig, hand, Vec3::from_array(rig.charging_handle));
                    let pulled = handle - hand.1 * Vec3::Z * 0.08;
                    b.hands[0].pos = if t < 1.9 {
                        rest.lerp(hip, ramp(t, 1.4, 1.8))
                    } else if t < 2.9 {
                        hip.lerp(well, ramp(t, 2.0, 2.5))
                    } else if t < 3.45 {
                        well.lerp(handle, ramp(t, 3.0, 3.3))
                    } else {
                        handle.lerp(pulled, ramp(t, 3.5, 3.8))
                    };
                    b.hands[0].squeeze = if between(t, 1.9, 2.9) || between(t, 3.4, 4.1) { 1.0 } else { 0.0 };
                    b
                })
            },
            checks: vec![
                check(1.1, "rounds were fired", |c| ensure(c.stat(rs::SHOTS) >= 3, || format!("shots {}", c.stat(rs::SHOTS)))),
                check(1.6, "A ejected the magazine", |c| {
                    ensure(c.mech.ejects == 1 && c.mech.mag == MagState::Out, || format!("ejects {} mag {:?}", c.mech.ejects, c.mech.mag))
                }),
                check(2.0, "a fresh magazine was grabbed at the hip", |c| {
                    ensure(c.mech.mag == MagState::Held && c.stat(rs::GESTURE_HIP) == 0, || format!("mag {:?}, knife {}", c.mech.mag, c.stat(rs::GESTURE_HIP)))
                }),
                check(2.8, "and seated in the magazine well", |c| ensure(c.mech.mag == MagState::Seated, || format!("mag {:?}", c.mech.mag))),
                check(4.0, "racking the charging handle finished the reload", |c| {
                    ensure(c.mech.reloads == 1 && c.mech.mag == MagState::In, || format!("reloads {} mag {:?}", c.mech.reloads, c.mech.mag))
                }),
                check(5.8, "the game reloaded to a full magazine", |c| {
                    let full = c.stat_f(rs::F_MAG_SIZE) as u32;
                    ensure(c.stat(rs::RELOADS) == 1 && full > 0 && c.stat(rs::AMMO) == full, || {
                        format!("reloads {} ammo {} of {full}", c.stat(rs::RELOADS), c.stat(rs::AMMO))
                    })
                }),
            ],
            captures: vec![(2.2, "holding_mag"), (3.6, "racking")],
        },
        Scenario {
            name: "reload_slap",
            about: "the simpler reload: slap a magazine up into the well with the off hand",
            duration: 5.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |s| s.input.reload_style = ReloadStyle::Slap,
            input: || {
                Box::new(|t, see| {
                    let mut b = Body::rest();
                    let rig = see.rig();
                    let hand = aim_hand(&rig, b.eye(1), TARGET_AHEAD);
                    let hand = (hand.0 + Vec3::new(0.0, -0.15, 0.05), hand.1);
                    b.hands[1].pos = hand.0;
                    b.hands[1].rot = hand.1;
                    b.hands[1].trigger = if between(t, 0.3, 1.3) { 1.0 } else { 0.0 };
                    // Off hand: below the magazine well, then up through it fast (a slap).
                    let well = on_gun(&rig, hand, mag_well(&rig));
                    let below = well + Vec3::new(0.0, -0.25, 0.0);
                    b.hands[0].pos = if t < 1.8 {
                        Body::rest().hands[0].pos.lerp(below, ramp(t, 1.3, 1.8))
                    } else {
                        below.lerp(well + Vec3::new(0.0, 0.03, 0.0), ramp(t, 1.8, 2.0))
                    };
                    b
                })
            },
            checks: vec![
                check(1.5, "rounds were fired", |c| ensure(c.stat(rs::SHOTS) >= 3, || format!("shots {}", c.stat(rs::SHOTS)))),
                check(3.0, "one reload gesture", |c| ensure(c.mech.reloads == 1, || format!("reloads {}", c.mech.reloads))),
                check(4.8, "the game reloaded to a full magazine", |c| {
                    let full = c.stat_f(rs::F_MAG_SIZE) as u32;
                    ensure(c.stat(rs::RELOADS) >= 1 && full > 0 && c.stat(rs::AMMO) == full, || {
                        format!("reloads {} ammo {} of {full}", c.stat(rs::RELOADS), c.stat(rs::AMMO))
                    })
                }),
            ],
            captures: vec![(2.2, "slap")],
        },
        Scenario {
            name: "melee",
            about: "punch forward fast with the off hand: one melee strike; a slow reach isn't one",
            duration: 3.5,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    let rest = b.hands[0].pos;
                    let cocked = Vec3::new(-0.2, 1.40, 0.15);
                    let struck = Vec3::new(-0.2, 1.45, 0.70);
                    b.hands[0].pos = if t < 0.5 {
                        rest.lerp(cocked, ramp(t, 0.1, 0.4))
                    } else if t < 1.0 {
                        cocked.lerp(struck, ramp(t, 0.5, 0.62))
                    } else if t < 2.2 {
                        struck.lerp(cocked, ramp(t, 1.0, 2.0))
                    } else {
                        cocked.lerp(struck, ramp(t, 2.2, 3.2))
                    };
                    b
                })
            },
            checks: vec![
                check(0.9, "the punch landed as a melee", |c| ensure(c.stat(rs::MELEES) == 1, || format!("melees {}", c.stat(rs::MELEES)))),
                check(3.4, "the slow reach didn't", |c| ensure(c.stat(rs::MELEES) == 1, || format!("melees {}", c.stat(rs::MELEES)))),
            ],
            captures: vec![],
        },
        Scenario {
            name: "crouch",
            about: "duck in real life: the game crouches and the view stays at your real head height",
            duration: 4.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    let duck = ramp(t, 0.5, 1.0) - ramp(t, 2.5, 3.0);
                    b.head_pos.y -= 0.55 * duck;
                    for h in &mut b.hands {
                        h.pos.y -= 0.4 * duck;
                    }
                    b
                })
            },
            checks: vec![
                check(2.2, "the game crouched", |c| ensure(c.stat(rs::CROUCHED) == 1, || "standing".into())),
                check(2.2, "the view isn't lowered twice", |c| {
                    ensure(c.mech.height_comp > 0.2, || format!("height_comp {:.2}", c.mech.height_comp))
                }),
                check(3.9, "standing up stands the game up", |c| ensure(c.stat(rs::CROUCHED) == 0, || "still crouched".into())),
            ],
            captures: vec![(2.0, "crouched")],
        },
        Scenario {
            name: "gestures",
            about: "grip at the chest, face, top of the head, hip and over the shoulder",
            duration: 6.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    let rest = b.hands[0].pos;
                    let spots = [
                        (0.3, Vec3::new(-0.05, 1.30, 0.10)), // chest
                        (1.4, Vec3::new(0.0, 1.66, 0.10)),   // face
                        (2.5, Vec3::new(0.0, 1.88, 0.0)),    // top of the head
                        (3.6, Vec3::new(-0.25, 0.80, 0.05)), // hip
                        (4.7, Vec3::new(-0.20, 1.55, -0.12)), // over the shoulder
                    ];
                    for (t0, spot) in spots {
                        if between(t, t0, t0 + 1.0) {
                            let k = ramp(t, t0, t0 + 0.3) - ramp(t, t0 + 0.7, t0 + 1.0);
                            b.hands[0].pos = rest.lerp(spot, k);
                            b.hands[0].squeeze = if between(t, t0 + 0.4, t0 + 0.6) { 1.0 } else { 0.0 };
                        }
                    }
                    b
                })
            },
            checks: vec![check(5.9, "every gesture reached the game", |c| {
                let got = [
                    c.stat(rs::GESTURE_CHEST),
                    c.stat(rs::GESTURE_FACE),
                    c.stat(rs::GESTURE_HEAD_TOP),
                    c.stat(rs::GESTURE_HIP),
                    c.stat(rs::GESTURE_SHOULDER),
                ];
                ensure(got.iter().all(|&n| n == 1), || format!("chest/face/head/hip/shoulder = {got:?} (runtime saw {:?})", c.mech.gestures))
            })],
            captures: vec![],
        },
        Scenario {
            name: "turn",
            about: "flick the stick: one 30 degree snap turn, applied by the game",
            duration: 2.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    b.hands[1].stick = if between(t, 0.3, 0.6) { Vec2::new(1.0, 0.0) } else { Vec2::ZERO };
                    b
                })
            },
            checks: vec![check(1.5, "one snap (the configured angle), and the game turned with it", |c| {
                let yaw = c.stat_f(rs::F_LOOK_YAW).to_degrees();
                let snap = c.settings.comfort.snap_angle;
                ensure((c.turn_total.to_degrees() - snap).abs() < 0.1 && (yaw - snap).abs() < 1.0, || {
                    format!("runtime {:.1} game {:.1}", c.turn_total.to_degrees(), yaw)
                })
            })],
            captures: vec![(1.5, "turned")],
        },
        Scenario {
            name: "roomscale",
            about: "walk half a metre in the room: the game body follows the head",
            duration: 4.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    let step = Vec3::new(0.0, 0.0, 0.5 * ramp(t, 0.5, 1.5));
                    b.head_pos += step;
                    for h in &mut b.hands {
                        h.pos += step;
                    }
                    b
                })
            },
            checks: vec![
                check(1.3, "the body walks after the head", |c| ensure(c.stat_f(rs::F_FEET_Z) > 0.05, || format!("feet z {:.2}", c.stat_f(rs::F_FEET_Z)))),
                check(3.8, "the game body caught up with the head", |c| {
                    ensure(c.stat_f(rs::F_FEET_Z) > 0.35 && c.mech.room_len < 0.1, || {
                        format!("feet z {:.2}, offset {:.2}", c.stat_f(rs::F_FEET_Z), c.mech.room_len)
                    })
                }),
            ],
            captures: vec![],
        },
        Scenario {
            name: "abilities",
            about: "hold the off-hand grip and flick up, down, left and right: four abilities, no melee",
            duration: 5.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    let home = Vec3::new(-0.30, 1.30, 0.35);
                    let flicks = [(0.6, Vec3::Y), (1.7, -Vec3::Y), (2.8, -Vec3::X), (3.9, Vec3::X)];
                    let mut pos = Body::rest().hands[0].pos.lerp(home, ramp(t, 0.0, 0.3));
                    for (t0, dir) in flicks {
                        // Fast out (0.12 s), slow back (0.7 s).
                        let k = ramp(t, t0, t0 + 0.12) - ramp(t, t0 + 0.2, t0 + 0.9);
                        pos += dir * 0.25 * k;
                    }
                    b.hands[0].pos = pos;
                    b.hands[0].squeeze = if between(t, 0.35, 4.8) { 1.0 } else { 0.0 };
                    b
                })
            },
            checks: vec![check(4.9, "each flick fired its ability once, and none was a melee", |c| {
                let got = [c.stat(rs::ABILITY_UP), c.stat(rs::ABILITY_DOWN), c.stat(rs::ABILITY_LEFT), c.stat(rs::ABILITY_RIGHT)];
                ensure(got == [1, 1, 1, 1] && c.stat(rs::MELEES) == 0, || format!("up/down/left/right = {got:?}, melees {}", c.stat(rs::MELEES)))
            })],
            captures: vec![],
        },
        Scenario {
            name: "menu",
            about: "pause, switch to smooth turning in the runtime menu, click the game menu, resume",
            duration: 5.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, see| {
                    let mut b = Body::rest();
                    let menu_press = between(t, 0.2, 0.3) || between(t, 3.4, 3.5);
                    b.hands[0].buttons = if menu_press { CTRL_MENU } else { 0 };
                    let hand = b.hands[1].pos;
                    let aim_at = |p: Option<Vec3>| p.map(|p| look_rot(p - hand));
                    // The menu opens on Help: open the Comfort tab, then pick smooth turning.
                    if between(t, 0.45, 0.85) {
                        if let Some(r) = aim_at(see.widget("tab:Comfort")) {
                            b.hands[1].rot = r;
                        }
                        b.hands[1].trigger = if between(t, 0.6, 0.7) { 1.0 } else { 0.0 };
                    } else if between(t, 0.9, 1.8) {
                        if let Some(r) = aim_at(see.widget("turn:smooth")) {
                            b.hands[1].rot = r;
                        }
                        b.hands[1].trigger = if between(t, 1.4, 1.5) { 1.0 } else { 0.0 };
                    } else if between(t, 2.0, 3.0) {
                        if let Some(r) = aim_at(see.world.game_menu.map(|p| p.center)) {
                            b.hands[1].rot = r;
                        }
                        b.hands[1].trigger = if between(t, 2.4, 2.5) { 1.0 } else { 0.0 };
                    }
                    b
                })
            },
            checks: vec![
                check(0.8, "the game paused into its menu", |c| {
                    ensure(c.game.is_some_and(|g| g.flags & GS_MENU != 0), || "no menu".into())
                }),
                check(1.9, "the runtime menu switched to smooth turning", |c| {
                    ensure(c.settings.comfort.turn_mode == TurnMode::Smooth, || format!("{:?}", c.settings.comfort.turn_mode))
                }),
                check(2.9, "the laser clicked the game's menu", |c| ensure(c.stat(rs::MENU_CLICKS) >= 1, || "no click".into())),
                check(4.8, "the game resumed", |c| {
                    ensure(c.game.is_some_and(|g| g.flags & GS_MENU == 0) && c.stat(rs::MENU_TOGGLES) >= 2, || {
                        format!("toggles {}", c.stat(rs::MENU_TOGGLES))
                    })
                }),
            ],
            captures: vec![(1.45, "menu")],
        },
        Scenario {
            name: "scope",
            about: "aim through a magnified optic: the scope view renders into the lens",
            duration: 2.5,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--weapon", "scoped"],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, see| {
                    let rest = Body::rest();
                    let mut aimed = rest;
                    let (p, r) = aim_hand(&see.rig(), rest.eye(1), TARGET_AHEAD);
                    aimed.hands[1].pos = p;
                    aimed.hands[1].rot = r;
                    lerp_body(&rest, &aimed, ramp(t, 0.1, 0.8))
                })
            },
            checks: vec![
                check(1.5, "aiming through the optic opens the scope", |c| {
                    ensure(c.out.intents.flags & INT_SCOPE != 0, || format!("zoom {:.2} ads {}", c.out.intents.zoom, c.mech.ads))
                }),
                check(2.4, "the game rendered the scope view and the lens showed it", |c| {
                    ensure(c.stat_f(rs::F_SCOPE_FRAMES) > 10.0 && c.stats.scope_frames > 10, || {
                        format!("game {} runtime {}", c.stat_f(rs::F_SCOPE_FRAMES), c.stats.scope_frames)
                    })
                }),
            ],
            captures: vec![(1.5, "scope")],
        },
        Scenario {
            name: "async",
            about: "real time: the headset at 90 Hz, the game at its own 45 fps; the newest picture is always shown",
            duration: 3.0,
            needs_game: true,
            lockstep: false,
            range_args: vec!["--fps", "45"],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    b.head_rot = Quat::from_rotation_y(0.5 * (t as f32 * 1.5).sin());
                    b
                })
            },
            checks: vec![
                check(2.9, "every headset frame showed a game picture", |c| {
                    ensure(c.stats.game_frames as f64 >= c.frames_run as f64 * 0.95, || format!("{} of {}", c.stats.game_frames, c.frames_run))
                }),
                check(2.9, "the game ran at its own pace, about half the headset's rate", |c| {
                    let k = c.stats.slots as f64 / c.frames_run as f64;
                    ensure((0.3..0.75).contains(&k), || format!("{} game frames for {} headset frames", c.stats.slots, c.frames_run))
                }),
            ],
            captures: vec![(1.5, "async")],
        },
        Scenario {
            name: "live",
            about: "a running game (--attach): look around for 12 s and capture what the headset shows",
            duration: 12.0,
            needs_game: true,
            lockstep: false,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    // Ahead, then a slow look left and right, up and down.
                    let k = ((t - 2.0).max(0.0) * 0.6) as f32;
                    b.head_rot = Quat::from_rotation_y(0.6 * k.sin()) * Quat::from_rotation_x(0.25 * (k * 0.7).sin());
                    b
                })
            },
            checks: vec![check(11.9, "the game's picture reached the headset", |c| {
                ensure(c.stats.game_frames > 30, || format!("{} game frames of {}", c.stats.game_frames, c.frames_run))
            })],
            captures: vec![(1.0, "live_01"), (3.0, "live_03"), (5.0, "live_05"), (7.0, "live_07"), (9.0, "live_09"), (11.5, "live_11")],
        },
        Scenario {
            name: "metro_boot",
            about: "Metro, attached (--attach) as it starts: tap A through its intro and menus into the game, capturing the headset every 5 s",
            duration: 170.0,
            needs_game: true,
            lockstep: false,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    // A tap every 6 s for the first two minutes (the splash, "press any key", the
                    // main menu's first item, the loading screen's "press to continue").
                    if t < 120.0 && (t % 6.0) < 0.2 {
                        b.hands[1].buttons |= rustvr_protocol::CTRL_A;
                    }
                    let k = ((t - 130.0).max(0.0) * 0.6) as f32;
                    b.head_rot = Quat::from_rotation_y(0.6 * k.sin());
                    b
                })
            },
            checks: vec![check(169.0, "the game's picture reached the headset", |c| {
                ensure(c.stats.game_frames > 100, || format!("{} game frames", c.stats.game_frames))
            })],
            captures: (0..34).map(|i| (i as f64 * 5.0, &*Box::leak(format!("boot_{:03}", i * 5).into_boxed_str()))).collect(),
        },
        Scenario {
            name: "void",
            about: "no game: the runtime shows its own room and settings menu",
            duration: 1.0,
            needs_game: false,
            lockstep: true,
            range_args: vec![],
            physical: false,
            start: Body::rest,
            settings: |_| {},
            input: || Box::new(|_, _| Body::rest()),
            checks: vec![check(0.9, "the room and menu are drawn", |c| {
                let cap = c.capture("void").ok_or("no capture")?;
                ensure(cap.lit > 0.2, || format!("lit {:.2}", cap.lit))
            })],
            captures: vec![(0.5, "void")],
        },
    ]
    .into_iter()
    .chain(crate::scenario_physical::all())
    .collect()
}

// ---- running ----------------------------------------------------------------------------------

struct Driver {
    rest: fn() -> Body,
    start: Arc<AtomicU64>,
    world: Arc<Mutex<World>>,
    game: Arc<Mutex<Option<GameState>>>,
    input: InputFn,
}

impl SimDriver for Driver {
    fn input(&mut self, frame: u64, _t: f64) -> SimInput {
        let start = self.start.load(Ordering::Acquire);
        if start == 0 || frame < start {
            return (self.rest)().input();
        }
        let t = (frame - start) as f64 / HZ as f64;
        let world = self.world.lock().unwrap().clone();
        let game = *self.game.lock().unwrap();
        (self.input)(t, &See { world: &world, game: game.as_ref() }).input()
    }
}

pub struct RunOptions {
    pub captures: PathBuf,
    /// Start the test range for scenarios that need a game.
    pub launch_range: bool,
    /// Or play against a game that's already running: its adapter connects to this mapping (the
    /// one the real runtime uses), and the captures show what the headset would.
    pub attach: Option<String>,
    pub range_exe: Option<PathBuf>,
    pub eye_size: (u32, u32),
    pub warmup_timeout: std::time::Duration,
}

pub struct Report {
    pub name: &'static str,
    pub results: Vec<(&'static str, std::result::Result<(), String>)>,
    pub captures: Vec<Capture>,
    pub frames: u64,
    pub stats: Stats,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.results.iter().all(|(_, r)| r.is_ok())
    }
}

pub fn run(gpu: &Gpu, sc: &Scenario, opts: &RunOptions) -> Result<Report> {
    let mapping = opts.attach.clone().unwrap_or_else(|| format!("Local\\RustVR_scenario_{}_{}", sc.name, std::process::id()));
    let start = Arc::new(AtomicU64::new(0));
    let world = Arc::new(Mutex::new(World::default()));
    let game = Arc::new(Mutex::new(None));
    let driver = Driver { rest: sc.start, start: start.clone(), world: world.clone(), game: game.clone(), input: (sc.input)() };
    let backend = SimBackend::new(Box::new(driver), opts.eye_size, HZ, !sc.lockstep);
    let mut session = Session::new(
        gpu.clone(),
        Box::new(backend),
        Options { mapping: mapping.clone(), settings_dir: None, lockstep: sc.lockstep, lockstep_timeout_ms: 3000, audio: false },
    )?;
    session.world = world.clone();
    let mut settings = Settings::default();
    (sc.settings)(&mut settings);
    session.settings = SettingsStore::memory(settings);

    let mut child = None;
    if sc.needs_game && opts.attach.is_none() {
        anyhow::ensure!(opts.launch_range, "scenario '{}' needs the test range (--launch-range)", sc.name);
        let exe = match &opts.range_exe {
            Some(p) => p.clone(),
            None => std::env::current_exe()?.with_file_name("rustvr-range.exe"),
        };
        anyhow::ensure!(exe.exists(), "{} not found: build the workspace first", exe.display());
        let log = opts.captures.join(sc.name).join("range.log");
        let mut args: Vec<String> = sc.range_args.iter().map(|a| a.to_string()).collect();
        if !sc.physical {
            args.push("--legacy".into());
        }
        child = Some(crate::app::spawn_range(&exe, &args, &mapping, Some(&log), true)?);
    }

    // Warm up: the range connects, gets its textures and renders a few frames.
    // A game being started by hand takes a while to reach its first frame.
    let deadline = std::time::Instant::now() + if opts.attach.is_some() { opts.warmup_timeout * 3 } else { opts.warmup_timeout };
    loop {
        session.step()?;
        *game.lock().unwrap() = session.game;
        // An attached game may sit in its menus (flat frames) for a while: any frame from it counts.
        let ready = if !sc.needs_game {
            session.stats.frames > 5
        } else if opts.attach.is_some() {
            session.adapter_connected() && session.stats.slots > 5
        } else {
            session.adapter_connected() && session.stats.game_frames > 5
        };
        if ready {
            break;
        }
        if let Some(c) = &mut child
            && let Some(status) = c.0.try_wait()? {
                anyhow::bail!("the test range exited during warm-up ({status}); see {}", opts.captures.join(sc.name).join("range.log").display());
            }
        anyhow::ensure!(std::time::Instant::now() < deadline, "warm-up timed out (adapter connected: {})", session.adapter_connected());
    }
    let first = session.backend.as_sim().unwrap().frame_index() + 1;
    start.store(first, Ordering::Release);
    let base = session.stats;
    let base_haptics = session.backend.as_sim().unwrap().haptics.len();

    let mut checks: Vec<(&Check, bool)> = sc.checks.iter().map(|c| (c, false)).collect();
    let mut captures_due: Vec<(f64, &str, bool)> = sc.captures.iter().map(|(t, n)| (*t, *n, false)).collect();
    let mut captures = Vec::new();
    let mut results = Vec::new();
    let frames = (sc.duration * HZ as f64).ceil() as u64;
    let mut run_stats = Stats::default();
    for i in 0..frames {
        session.step()?;
        *game.lock().unwrap() = session.game;
        let t = i as f64 / HZ as f64;
        run_stats = diff(&session.stats, &base);
        for (at, name, done) in &mut captures_due {
            if !*done && t >= *at {
                *done = true;
                let path = opts.captures.join(sc.name).join(format!("{name}.png"));
                let sim = session.backend.as_sim().unwrap();
                sim.capture(gpu, &path)?;
                let (mean, lit) = brightness(gpu, sim)?;
                tracing::info!("capture {name}: game flags {:#x}, {} game frames", session.game.map(|g| g.flags).unwrap_or(0), session.stats.game_frames);
                captures.push(Capture { name: name.to_string(), path, mean, lit });
            }
        }
        let haptics = session.backend.as_sim().unwrap().haptics.len() - base_haptics;
        let objects = session.world.lock().unwrap().objects.clone();
        let ctx = Ctx {
            t,
            rig: &session.rig.debug,
            objects: &objects,
            body: &session.body.body,
            draws_body: session.draws_body,
            reached: session.body.reached,
            floor: session.floor,
            game: session.game.as_ref(),
            mech: &session.mech.debug,
            out: &session.last_out,
            stats: &run_stats,
            settings: &session.settings.current,
            turn_total: session.mech.turn_total,
            haptics,
            frames_run: i + 1,
            captures: &captures,
        };
        for (c, done) in &mut checks {
            if !*done && t >= c.at {
                *done = true;
                results.push((c.what, (c.test)(&ctx)));
            }
        }
    }
    for (c, done) in checks {
        if !done {
            results.push((c.what, Err(format!("never reached t = {}", c.at))));
        }
    }
    drop(child);
    Ok(Report { name: sc.name, results, captures, frames, stats: run_stats })
}

fn diff(a: &Stats, b: &Stats) -> Stats {
    Stats {
        frames: a.frames - b.frames,
        game_frames: a.game_frames - b.game_frames,
        slots: a.slots - b.slots,
        lockstep_timeouts: a.lockstep_timeouts - b.lockstep_timeouts,
        last_slot_frame: a.last_slot_frame,
        exact_frames: a.exact_frames - b.exact_frames,
        scope_frames: a.scope_frames - b.scope_frames,
        menu_frames: a.menu_frames - b.menu_frames,
        vignette: a.vignette,
        fade: a.fade,
        vignette_frames: a.vignette_frames - b.vignette_frames,
        fade_frames: a.fade_frames - b.fade_frames,
    }
}

fn brightness(gpu: &Gpu, sim: &SimBackend) -> Result<(f32, f32)> {
    let mut sum = 0.0f64;
    let mut lit = 0u64;
    let mut n = 0u64;
    for e in 0..2 {
        let (_, _, px) = sim.eye_pixels(gpu, e)?;
        for p in px.as_chunks::<4>().0 {
            let l = (p[0] as f64 * 0.2126 + p[1] as f64 * 0.7152 + p[2] as f64 * 0.0722) / 255.0;
            sum += l;
            if l > 0.04 {
                lit += 1;
            }
            n += 1;
        }
    }
    Ok(((sum / n as f64) as f32, lit as f32 / n as f32))
}

/// Prints a report and returns whether it passed.
pub fn print(r: &Report, root: &Path) -> bool {
    let ok = r.passed();
    println!("{} {}  ({} frames, {} with the game, {} lockstep timeouts)", if ok { "PASS" } else { "FAIL" }, r.name, r.frames, r.stats.game_frames, r.stats.lockstep_timeouts);
    for (what, res) in &r.results {
        match res {
            Ok(()) => println!("   ok   {what}"),
            Err(e) => println!("   FAIL {what}: {e}"),
        }
    }
    for c in &r.captures {
        let shown = c.path.strip_prefix(root).unwrap_or(&c.path);
        println!("   png  {} (mean {:.2}, lit {:.0}%)", shown.display(), c.mean, c.lit * 100.0);
    }
    ok
}
