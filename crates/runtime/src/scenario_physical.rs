//! Headless scenarios for the player rig: the test range in physical mode, its guns, magazines and
//! props handled by the runtime's own hands (see `crate::rig`).
//!
//! The scripts latch where things are the first time they see the range's objects (everything is
//! at rest on its table then) and move the hands relative to that. Tracking space and the range's
//! world line up at the start (the player stands at the origin facing +Z), so the positions in the
//! comments are both.

use glam::{Quat, Vec3};
use rustvr_protocol::range_stats as rs;
use rustvr_protocol::*;

use crate::rig::ObjSnap;
use crate::scenario::{Body, Ctx, InputFn, Scenario, See, TARGET_AHEAD, between, check, ensure, look_rot, ramp};

/// Smoothly from keyframe to keyframe (time, value); holds the first and last.
fn keys(t: f64, k: &[(f64, Vec3)]) -> Vec3 {
    if t <= k[0].0 {
        return k[0].1;
    }
    for w in k.windows(2) {
        if t < w[1].0 {
            return w[0].1.lerp(w[1].1, ramp(t, w[0].0, w[1].0));
        }
    }
    k[k.len() - 1].1
}

fn qkeys(t: f64, k: &[(f64, Quat)]) -> Quat {
    if t <= k[0].0 {
        return k[0].1;
    }
    for w in k.windows(2) {
        if t < w[1].0 {
            return w[0].1.slerp(w[1].1, ramp(t, w[0].0, w[1].0));
        }
    }
    k[k.len() - 1].1
}

fn on(t: f64, spans: &[(f64, f64)]) -> f32 {
    if spans.iter().any(|(a, b)| between(t, *a, *b)) { 1.0 } else { 0.0 }
}

/// A player whose eyes are 18 cm lower than the range character's.
fn short_body() -> Body {
    let mut b = Body::rest();
    b.head_pos.y = 1.52;
    for h in &mut b.hands {
        h.pos.y -= 0.18;
    }
    b
}

/// Sitting on a chair: eyes at 1.2 m.
fn seated_body() -> Body {
    let mut b = Body::rest();
    b.head_pos.y = 1.2;
    for h in &mut b.hands {
        h.pos.y -= 0.5;
    }
    b
}

/// The plain rifle on the table at your right hand (the nearer of the two).
fn near_rifle(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter()
        .filter(|o| o.kind == OBJ_WEAPON && o.flags & OS_HOLSTERED == 0 && (0.4..1.6).contains(&o.pos.x) && o.pos.z < 1.1 && o.pos.y > 0.7)
        .min_by(|a, b| a.pos.z.total_cmp(&b.pos.z))
        .copied()
}

/// The nearest gun on that table that kicks the way its game does (Metro's Kalash).
fn near_game_rifle(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter()
        .filter(|o| o.kind == OBJ_WEAPON && o.kicks > 0 && o.flags & OS_HOLSTERED == 0 && (0.4..1.6).contains(&o.pos.x) && o.pos.y > 0.7)
        .min_by(|a, b| a.pos.z.total_cmp(&b.pos.z))
        .copied()
}

/// The aim straight from the eye to the target ahead.
fn eye_to_target() -> Vec3 {
    (TARGET_AHEAD - Body::rest().eye(1)).normalize()
}

fn holstered_gun(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter().find(|o| o.kind == OBJ_WEAPON && o.flags & OS_HOLSTERED != 0).copied()
}

fn prop_near(objs: &[ObjSnap], p: Vec3) -> Option<ObjSnap> {
    objs.iter().filter(|o| o.kind == OBJ_PROP).min_by(|a, b| a.pos.distance(p).total_cmp(&b.pos.distance(p))).copied().filter(|o| o.pos.distance(p) < 0.3)
}

/// The controller rotation that holds `gun` at rotation `gun_rot` (rigged guns sit in the hand
/// by their grip rotation; unrigged ones along the aim ray, which the sim makes the grip's).
fn hand_rot_for(gun: &ObjSnap, gun_rot: Quat) -> Quat {
    (gun_rot * gun.grip_rot.unwrap_or(Quat::IDENTITY)).normalize()
}

/// The gun's rotation in a hand at `hand_rot`.
fn gun_rot_in(gun: &ObjSnap, hand_rot: Quat) -> Quat {
    (hand_rot * gun.grip_rot.unwrap_or(Quat::IDENTITY).inverse()).normalize()
}

/// Where the hand goes so a held gun's line of sight runs from `eye` (starting `ahead` metres in
/// front of it) to `target`.
fn aim_gun(gun: &ObjSnap, eye: Vec3, target: Vec3, ahead: f32) -> (Vec3, Quat) {
    let d = (target - eye).normalize();
    let r = look_rot(d);
    (eye + d * ahead - r * (gun.sight - gun.grip), hand_rot_for(gun, r))
}

/// A point on a gun held by a hand at `hand` (palm on the grip).
fn on_held(gun: &ObjSnap, hand: (Vec3, Quat), p: Vec3) -> Vec3 {
    hand.0 + gun_rot_in(gun, hand.1) * (p - gun.grip)
}

/// A script that waits for the range's objects, latches them, then runs `f`.
fn latched(mut f: impl FnMut(f64, &[ObjSnap], &See) -> Body + Send + 'static) -> InputFn {
    let mut objs: Option<Vec<ObjSnap>> = None;
    Box::new(move |t, see| {
        if objs.is_none() && !see.world.objects.is_empty() {
            objs = Some(see.world.objects.clone());
        }
        match &objs {
            Some(o) => f(t, o, see),
            None => Body::rest(),
        }
    })
}

/// The rifle-reach part of several scripts: reach over, grab it the way it lies, lift it clear.
/// Returns the right hand's pose up to `lift_end`.
fn take_rifle(t: f64, rifle: &ObjSnap) -> (Vec3, Quat) {
    let rest = Body::rest().hands[1].pos;
    let grip = rifle.at(rifle.grip);
    // Reach with the hand turned the way it would hold the gun as it lies, so it doesn't swing.
    let along = hand_rot_for(rifle, rifle.rot);
    let above = grip + Vec3::Y * 0.08;
    let level = hand_rot_for(rifle, Quat::IDENTITY);
    let pos = keys(t, &[(0.0, rest), (0.35, Vec3::new(grip.x, 1.2, grip.z)), (0.6, above), (0.75, above), (1.0, Vec3::new(0.5, 1.3, 0.35))]);
    let rot = qkeys(t, &[(0.0, Quat::IDENTITY), (0.35, along), (0.75, along), (1.0, level)]);
    (pos, rot)
}

fn gun_held_right(c: &Ctx) -> bool {
    c.objects.iter().any(|o| o.kind == OBJ_WEAPON && o.flags & OS_HELD_RIGHT != 0)
}

/// The frag grenade in the right chest pouch.
fn pouch_grenade(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter().find(|o| o.kind == OBJ_THROWABLE && o.throw_flags & THROW_PIN != 0 && o.flags & OS_HOLSTERED != 0 && o.pos.x > 0.0).copied()
}

/// The throwing knife nearest you on the throwables' table (at your left hand).
fn table_knife(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter()
        .filter(|o| o.kind == OBJ_THROWABLE && o.throw_flags & THROW_KNIFE != 0 && o.flags & OS_HOLSTERED == 0 && o.pos.x < -0.4 && o.pos.z > 0.8)
        .max_by(|a, b| a.pos.x.total_cmp(&b.pos.x))
        .copied()
}

/// The thrown object, in the live snapshot.
fn live(see: &See, id: u32) -> Option<ObjSnap> {
    see.world.objects.iter().find(|o| o.id == id).copied()
}

fn throwable<'a>(c: &Ctx<'a>, flag: u32) -> Option<&'a ObjSnap> {
    c.objects.iter().find(|o| o.kind == OBJ_THROWABLE && o.throw_flags & flag != 0 && o.flags & (OS_THROWN | OS_ARMED | OS_STUCK | OS_SPENT | OS_HELD_LEFT | OS_HELD_RIGHT) != 0)
}

/// A throwing swing: from rest at `t0` the hand speeds up along `dir` over `accel` seconds to
/// `speed`, passing `at` (where it lets go) `hold` seconds into the steady part. Its position at `t`.
fn swing(t: f64, t0: f64, at: Vec3, dir: Vec3, speed: f32, accel: f32, hold: f32) -> Vec3 {
    let dir = dir.normalize();
    let along = |tau: f32| if tau <= 0.0 { 0.0 } else if tau < accel { 0.5 * speed / accel * tau * tau } else { 0.5 * speed * accel + speed * (tau - accel) };
    let start = at - dir * along(accel + hold);
    start + dir * along((t - t0) as f32)
}

/// The rig's speed gain for a throw this fast (for a script to aim with): see `rig::throwing`.
fn throw_gain(class: crate::rig::throwing::ThrowClass, speed: f32) -> f32 {
    crate::rig::throwing::gain(class, speed)
}

pub fn all() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "pickup",
            about: "physical: reach for the rifle on the table, pick it up, aim down the sights and hit the target",
            duration: 3.4,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(rifle) = near_rifle(objs) else { return b };
                    b.head_rot = Quat::from_rotation_x(0.5 * (1.0 - ramp(t, 1.0, 1.4)));
                    let aim = aim_gun(&rifle, Body::rest().eye(1), TARGET_AHEAD, 0.15);
                    let (p, r) = take_rifle(t, &rifle);
                    b.hands[1].pos = keys(t, &[(1.0, p), (1.6, aim.0)]);
                    b.hands[1].rot = qkeys(t, &[(1.0, r), (1.6, aim.1)]);
                    if t < 1.0 {
                        b.hands[1].pos = p;
                        b.hands[1].rot = r;
                    }
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    b.hands[1].trigger = on(t, &[(1.9, 1.98), (2.4, 2.48), (2.9, 2.98)]);
                    b
                })
            },
            checks: vec![
                check(0.9, "the rifle is in the right hand", |c| {
                    ensure(gun_held_right(c) && c.rig.grabs >= 1, || format!("grabs {}, held: {:?}", c.rig.grabs, c.objects.iter().filter(|o| o.flags & (OS_HELD_LEFT | OS_HELD_RIGHT) != 0).map(|o| o.id).collect::<Vec<_>>()))
                }),
                check(1.8, "raised to the eye it aims down the sights", |c| {
                    let w = c.out.weapon.as_ref().ok_or("no weapon pose")?;
                    ensure(w.ads, || format!("sight {:.3} m from the eye", w.sight_to_eye))
                }),
                check(3.3, "the shots it fired hit the target", |c| {
                    ensure(c.stat(rs::SHOTS) >= 3 && c.stat(rs::HITS) >= 3, || format!("shots {} hits {} (rig {})", c.stat(rs::SHOTS), c.stat(rs::HITS), c.rig.shots))
                }),
                check(3.3, "the range sees a gun in hand", |c| ensure(c.stat(rs::HELD) > 0, || "no gun held".into())),
            ],
            captures: vec![(0.62, "reach"), (1.2, "lifted"), (1.95, "aim")],
        },
        Scenario {
            name: "gun_fx",
            about: "physical: the range shows the rifle's shots as a game does: muzzle flash and light, smoke, tracers, holes and sparks on the target, casings out of the port onto the floor",
            duration: 4.4,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(rifle) = near_rifle(objs) else { return b };
                    b.head_rot = Quat::from_rotation_x(0.5 * (1.0 - ramp(t, 1.0, 1.4)));
                    // Fired from the hip, so the eye sees the flash, the tracers and the port from the side.
                    let aim = aim_gun(&rifle, Vec3::new(0.22, 1.34, 0.3), TARGET_AHEAD, 0.0);
                    let (p, r) = take_rifle(t, &rifle);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.6, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.6, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    b.hands[1].trigger = on(t, &[(1.9, 1.98), (2.4, 2.48), (2.9, 2.98)]);
                    // Look down to the right at the brass on the floor at the end.
                    let brass = look_rot(Vec3::new(1.1, 0.0, 0.25) - Body::rest().head_pos);
                    b.head_rot = b.head_rot.slerp(brass, ramp(t, 3.4, 3.8));
                    b
                })
            },
            checks: vec![
                check(1.88, "nothing yet", |c| ensure(c.stat(rs::SHOTS) == 0, || format!("{} shots", c.stat(rs::SHOTS)))),
                check(1.95, "the first shot: flash, light, smoke, tracer, casing, a hole", |c| {
                    ensure(c.stat(rs::SHOTS) == 1 && c.stat_f(rs::F_FX_LIVE) >= 9.0, || format!("{} shots, {} effects", c.stat(rs::SHOTS), c.stat_f(rs::F_FX_LIVE)))
                }),
                check(3.3, "three shots from the hip, holes where they hit and three casings down", |c| {
                    ensure(c.stat(rs::SHOTS) >= 3 && c.stat(rs::HITS) >= 2 && c.stat_f(rs::F_FX_LIVE) >= 6.0, || format!("shots {} hits {}, {} effects", c.stat(rs::SHOTS), c.stat(rs::HITS), c.stat_f(rs::F_FX_LIVE)))
                }),
                check(3.3, "the flash lit the view up", |c| {
                    let (before, flash) = (c.capture("fx_before").ok_or("no capture")?, c.capture("fx_flash").ok_or("no capture")?);
                    ensure(flash.mean > before.mean + 0.002, || format!("mean {:.4} -> {:.4}", before.mean, flash.mean))
                }),
            ],
            captures: vec![(1.85, "fx_before"), (1.92, "fx_flash"), (1.94, "fx_flash_2"), (1.96, "fx_flash_3"), (2.2, "fx_after"), (2.95, "fx_third"), (4.3, "fx_casings")],
        },
        Scenario {
            name: "game_recoil",
            about: "physical: Metro's Kalash fires a burst with Metro's own recoil (its camera tracks and fire motions), the muzzle held up while it fires and back after",
            duration: 3.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(rifle) = near_game_rifle(objs) else { return b };
                    b.head_rot = Quat::from_rotation_x(0.5 * (1.0 - ramp(t, 1.0, 1.4)));
                    // Down the sights (the sight 12 cm from the eye: aimed).
                    let aim = aim_gun(&rifle, Body::rest().eye(1), TARGET_AHEAD, 0.12);
                    let (p, r) = take_rifle(t, &rifle);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.6, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.6, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    // Hold the trigger for 0.7 s: a burst of seven or eight.
                    b.hands[1].trigger = on(t, &[(1.9, 2.6)]);
                    b
                })
            },
            checks: vec![
                check(0.2, "the Kalash came with its game's recoil", |c| {
                    let k = c.objects.iter().filter(|o| o.kind == OBJ_WEAPON).map(|o| o.kicks).max().unwrap_or(0);
                    ensure(k > 20, || format!("kicks per gun: {:?}", c.objects.iter().filter(|o| o.kind == OBJ_WEAPON).map(|o| o.kicks).collect::<Vec<_>>()))
                }),
                check(0.9, "it's in the right hand", |c| ensure(gun_held_right(c), || "not held".into())),
                check(1.85, "aimed, before firing, it points at the target", |c| {
                    let w = c.out.weapon.as_ref().ok_or("no weapon pose")?;
                    let off = (w.aim_dir.y - eye_to_target().y).abs();
                    ensure(off < 0.02, || format!("aim {:?}, {:.3} off", w.aim_dir, off))
                }),
                check(2.45, "mid-burst the muzzle is kicked up", |c| {
                    let w = c.out.weapon.as_ref().ok_or("no weapon pose")?;
                    let up = (w.aim_dir.y - eye_to_target().y).asin().to_degrees();
                    ensure(c.rig.game_kicks >= 5 && up > 1.0 && up < 8.0, || format!("{} game kicks, muzzle {up:.2} deg up", c.rig.game_kicks))
                }),
                check(3.5, "and back on target once the burst is over", |c| {
                    let w = c.out.weapon.as_ref().ok_or("no weapon pose")?;
                    let off = (w.aim_dir.y - eye_to_target().y).abs();
                    ensure(c.rig.shots == c.rig.game_kicks && off < 0.02, || format!("{} shots, {} game kicks, {:.3} off", c.rig.shots, c.rig.game_kicks, off))
                }),
            ],
            captures: vec![(1.85, "game_recoil_before"), (2.45, "game_recoil_burst")],
        },
        Scenario {
            name: "mag_reload",
            about: "physical: eject (A), take a magazine from the pouch over the shoulder, seat it, rack the handle, fire",
            duration: 6.2,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(rifle) = near_rifle(objs) else { return b };
                    let eye = Body::rest().eye(1);
                    // Hold the gun out a little lower than the sights, as you would while reloading.
                    let aim = aim_gun(&rifle, eye, TARGET_AHEAD, 0.15);
                    let hold = (aim.0 + Vec3::new(0.0, -0.12, 0.12), aim.1);
                    let (p, r) = take_rifle(t, &rifle);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.5, hold.0), (5.0, hold.0), (5.4, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.5, hold.1), (5.0, hold.1), (5.4, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    b.hands[1].buttons = if between(t, 1.6, 1.7) { CTRL_A } else { 0 };
                    b.hands[1].trigger = on(t, &[(5.6, 5.68), (5.95, 6.03)]);
                    // Off hand: shoulder pouch -> under the well -> up into it -> the charging handle -> pull.
                    let gun = gun_rot_in(&rifle, hold.1);
                    let r0 = (gun * crate::rig::held_mag_rot().inverse()).normalize();
                    let well = on_held(&rifle, hold, rifle.mag_well);
                    let seat = well - r0 * crate::rig::HELD_MAG_TOP;
                    let below = seat - gun * Vec3::Y * 0.22;
                    let handle = on_held(&rifle, hold, rifle.handle) + gun * Vec3::new(0.03, 0.0, 0.0);
                    let pulled = handle - gun * Vec3::Z * 0.1;
                    let rest = Body::rest().hands[0].pos;
                    let pouch = Vec3::new(-0.27, 1.56, -0.05);
                    b.hands[0].pos = keys(
                        t,
                        &[(1.8, rest), (2.1, Vec3::new(-0.3, 1.45, 0.15)), (2.35, pouch), (2.6, pouch), (3.1, below), (3.5, seat + gun * Vec3::Y * 0.02), (3.7, seat), (4.1, handle), (4.35, handle), (4.6, pulled), (4.8, pulled), (5.1, rest)],
                    );
                    b.hands[0].rot = qkeys(t, &[(2.6, Quat::IDENTITY), (3.1, r0)]);
                    b.hands[0].squeeze = on(t, &[(2.45, 3.65), (4.2, 4.7)]);
                    b
                })
            },
            checks: vec![
                check(1.0, "the rifle is in hand", |c| ensure(gun_held_right(c), || "not held".into())),
                check(1.8, "A dropped the magazine", |c| ensure(c.rig.ejects == 1, || format!("ejects {}", c.rig.ejects))),
                check(2.7, "the shoulder pouch handed over a magazine for this gun", |c| {
                    let held = c.objects.iter().any(|o| o.kind == OBJ_MAGAZINE && o.flags & OS_HELD_LEFT != 0);
                    ensure(c.rig.mags_spawned == 1 && held, || format!("spawned {} held {held}", c.rig.mags_spawned))
                }),
                check(3.7, "pushed up into the well, it seated", |c| ensure(c.rig.seats == 1, || format!("seats {}", c.rig.seats))),
                check(4.8, "pulling the charging handle back racked it", |c| ensure(c.rig.racks == 1, || format!("racks {}", c.rig.racks))),
                check(6.1, "and it fires from the new magazine", |c| {
                    let rifle = c.objects.iter().find(|o| o.kind == OBJ_WEAPON && o.flags & OS_HELD_RIGHT != 0).ok_or("no rifle")?;
                    ensure(c.stat(rs::SHOTS) >= 2 && rifle.rounds < 30 && rifle.rounds >= 25, || format!("shots {} rounds {}", c.stat(rs::SHOTS), rifle.rounds))
                }),
            ],
            captures: vec![(2.55, "pouch"), (3.4, "seating"), (4.5, "racking")],
        },
        Scenario {
            name: "holster",
            about: "physical: draw the pistol from the hip, fire, let go and it returns to the holster",
            duration: 4.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |s| s.interaction.auto_holster = true,
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(pistol) = holstered_gun(objs) else { return b };
                    let hip = pistol.at(pistol.grip);
                    let aim = aim_gun(&pistol, Body::rest().eye(1), TARGET_AHEAD, 0.4);
                    let rest = Body::rest().hands[1].pos;
                    let low = Vec3::new(0.25, 1.15, 0.45);
                    b.head_rot = Quat::from_rotation_x(0.6 * (1.0 - ramp(t, 0.7, 1.0)) * ramp(t, 0.0, 0.3) + 0.5 * ramp(t, 2.3, 2.6));
                    b.hands[1].pos = keys(t, &[(0.0, rest), (0.5, hip), (0.65, hip), (1.2, aim.0), (2.1, aim.0), (2.5, low)]);
                    b.hands[1].rot = qkeys(t, &[(0.65, Quat::IDENTITY), (1.2, aim.1), (2.1, aim.1), (2.5, Quat::IDENTITY)]);
                    b.hands[1].squeeze = on(t, &[(0.6, 2.6)]);
                    b.hands[1].trigger = on(t, &[(1.4, 1.5), (1.8, 1.9)]);
                    b
                })
            },
            checks: vec![
                check(0.1, "the pistol starts on the right hip", |c| {
                    ensure(c.objects.iter().any(|o| o.kind == OBJ_WEAPON && o.flags & OS_HOLSTERED != 0), || "nothing holstered".into())
                }),
                check(0.8, "drawn from the holster", |c| ensure(c.rig.drawn == 1 && gun_held_right(c), || format!("drawn {}", c.rig.drawn))),
                check(2.1, "two shots, hits", |c| ensure(c.stat(rs::SHOTS) == 2 && c.stat(rs::HITS) >= 1, || format!("shots {} hits {}", c.stat(rs::SHOTS), c.stat(rs::HITS)))),
                check(3.9, "let go, it went back to the hip", |c| {
                    let back = c.objects.iter().any(|o| o.kind == OBJ_WEAPON && o.flags & OS_HOLSTERED != 0);
                    ensure(back && c.rig.returns == 1, || format!("returns {} holstered {back}", c.rig.returns))
                }),
            ],
            captures: vec![(0.55, "holster"), (1.45, "pistol"), (3.0, "returned")],
        },
        Scenario {
            name: "throw",
            about: "physical: pick up a bottle from the side table and throw it down the range",
            duration: 3.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(bottle) = prop_near(objs, Vec3::new(-0.55, 0.97, 0.36)) else { return b };
                    let top = bottle.pos + Vec3::Y * bottle.half_extents.y;
                    let above = top + Vec3::Y * 0.055;
                    let rest = Body::rest().hands[0].pos;
                    b.head_rot = Quat::from_rotation_x(0.45 * (1.0 - ramp(t, 0.9, 1.2)));
                    b.hands[0].pos = keys(
                        t,
                        &[(0.0, rest), (0.3, Vec3::new(-0.45, 1.3, 0.36)), (0.5, above), (0.7, above), (1.0, Vec3::new(-0.35, 1.4, 0.2)), (1.3, Vec3::new(-0.35, 1.5, -0.15)), (1.45, Vec3::new(-0.3, 1.65, 0.65))],
                    );
                    // Let go part-way through the swing, at full speed.
                    b.hands[0].squeeze = on(t, &[(0.6, 1.4)]);
                    b
                })
            },
            checks: vec![
                check(0.9, "the bottle is in hand", |c| {
                    ensure(c.objects.iter().any(|o| o.kind == OBJ_PROP && o.flags & OS_HELD_LEFT != 0), || "not held".into())
                }),
                check(1.5, "thrown hard", |c| ensure(c.rig.throws == 1 && c.rig.last_throw_speed > 4.0, || format!("throws {} speed {:.1}", c.rig.throws, c.rig.last_throw_speed))),
                check(2.9, "it flew down the range", |c| {
                    let far = c.objects.iter().filter(|o| o.kind == OBJ_PROP && o.pos.x < -0.1 && o.pos.x > -0.6).map(|o| o.pos.z).fold(f32::MIN, f32::max);
                    ensure(far > 2.0, || format!("the bottle got to z {far:.2}"))
                }),
            ],
            captures: vec![(0.65, "grab"), (1.42, "swing")],
        },
        Scenario {
            name: "grenade",
            about: "physical: a frag grenade from the chest pouch, the other hand pulls its pin, a throw down range: it goes off after its 4 s fuse near the target",
            duration: 8.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(g) = pouch_grenade(objs) else { return b };
                    let rest = Body::rest().hands;
                    let pouch = g.at(g.grip);
                    let hold = Vec3::new(0.18, 1.32, 0.35);
                    // The throw: back by the shoulder, then forward and up at 6.5 m/s, letting go at
                    // 2.8 s.
                    let dir = Vec3::new(0.0, 0.5, 1.0).normalize();
                    let (release, speed, accel, steady) = (Vec3::new(0.3, 1.62, 0.2), 6.5, 0.12, 0.05);
                    let t0 = 2.8 - (accel + steady) as f64;
                    let start = swing(t0, t0, release, dir, speed, accel, steady);
                    // After the throw the arm comes back down, out of the way of the view.
                    let follow = swing(2.9, t0, release, dir, speed, accel, steady);
                    b.hands[1].pos = if t < t0 {
                        keys(t, &[(0.0, rest[1].pos), (0.45, pouch), (0.62, pouch), (1.0, hold), (2.2, hold), (2.55, start)])
                    } else if t < 2.9 {
                        swing(t, t0, release, dir, speed, accel, steady)
                    } else {
                        keys(t, &[(2.9, follow), (3.5, rest[1].pos)])
                    };
                    b.hands[1].squeeze = on(t, &[(0.55, 2.8)]);
                    // The left hand takes the ring where the held grenade has it and pulls it out.
                    let ring = live(see, g.id).filter(|o| o.flags & OS_HELD_RIGHT != 0).map(|o| o.at(o.handle)).unwrap_or(hold);
                    b.hands[0].pos = keys(t, &[(1.15, rest[0].pos), (1.5, ring), (1.6, ring), (1.8, ring - Vec3::X * 0.1), (2.1, rest[0].pos)]);
                    b.hands[0].squeeze = on(t, &[(1.55, 1.95)]);
                    b.head_rot = Quat::from_rotation_x(0.45 * ramp(t, 0.8, 1.1) * (1.0 - ramp(t, 2.1, 2.4)));
                    b
                })
            },
            checks: vec![
                check(0.75, "drawn from the chest pouch", |c| {
                    let held = c.objects.iter().any(|o| o.kind == OBJ_THROWABLE && o.throw_flags & THROW_PIN != 0 && o.flags & OS_HELD_RIGHT != 0);
                    ensure(held && c.rig.drawn >= 1, || format!("held {held}, drawn {}", c.rig.drawn))
                }),
                check(2.0, "the other hand pulled the pin; the lever holds the fuse", |c| {
                    let g = throwable(c, THROW_PIN).ok_or("no grenade")?;
                    ensure(c.rig.pins == 1 && g.flags & OS_PIN_PULLED != 0 && g.flags & OS_ARMED == 0, || format!("pins {}, flags {:#x}", c.rig.pins, g.flags))
                }),
                check(2.95, "thrown hard down range, the fuse burning", |c| {
                    let g = throwable(c, THROW_PIN).ok_or("no grenade")?;
                    let lit = g.flags & (OS_ARMED | OS_THROWN) == OS_ARMED | OS_THROWN;
                    ensure(c.rig.throws >= 1 && c.rig.last_throw_speed > 6.0 && lit && (3.7..4.0).contains(&g.fuse), || format!("throws {} at {:.1} m/s (gain {:.2}), flags {:#x}, fuse {:.2}", c.rig.throws, c.rig.last_throw_speed, c.rig.last_throw_gain, g.flags, g.fuse))
                }),
                check(6.6, "not before its fuse", |c| ensure(c.rig.detonations == 0, || format!("{} went off", c.rig.detonations))),
                check(7.2, "it went off down range near the target, and the range made the explosion", |c| {
                    let at = c.rig.last_detonation.ok_or("no detonation")?;
                    let near = at.distance(TARGET_AHEAD) < 5.0;
                    ensure(c.rig.detonations == 1 && at.z > 4.0 && near && c.stat_f(rs::F_EXPLOSIONS) >= 1.0 && c.stat_f(rs::F_BLAST_HITS) >= 1.0, || {
                        format!("detonations {} at {at:?}; range: explosions {}, plates hit {}", c.rig.detonations, c.stat_f(rs::F_EXPLOSIONS), c.stat_f(rs::F_BLAST_HITS))
                    })
                }),
                check(7.9, "its smoke and debris are still about", |c| ensure(c.stat_f(rs::F_FX_LIVE) > 20.0, || format!("{} effects", c.stat_f(rs::F_FX_LIVE)))),
            ],
            captures: vec![(1.75, "pin"), (2.82, "throw"), (3.3, "flight"), (6.84, "explosion"), (7.4, "smoke")],
        },
        Scenario {
            name: "knife",
            about: "physical: a throwing knife from the table at your left hand, thrown at the target 5 m away: it flies blade first and sticks in it",
            duration: 3.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(k) = table_knife(objs) else { return b };
                    let target = Vec3::new(-1.5, 1.4, 5.0);
                    // Where the hand lets go, and the throw that carries the knife's tip into the
                    // plate (held by its handle, the blade along the hand, dart-aligned in flight).
                    let hand = Vec3::new(-0.35, 1.55, 0.45);
                    let aim = target - Vec3::Z * 0.12;
                    let mut dir = (aim - hand).normalize();
                    let mut v = Vec3::ZERO;
                    for _ in 0..4 {
                        let centre = hand - look_rot(dir) * k.grip;
                        let tf = centre.distance(aim) / 10.0;
                        v = (aim - centre) / tf + Vec3::Y * (0.5 * 9.81 * tf);
                        dir = v.normalize();
                    }
                    // The hand's speed for that, after the rig's gain (a knife's weight barely counts).
                    let speed = v.length() / (throw_gain(crate::rig::throwing::ThrowClass::Knife, 8.0) * 0.99);
                    let (accel, steady, t0) = (0.08, 0.04, 1.2);
                    let start = swing(t0, t0, hand, dir, speed, accel, steady);
                    let above = k.at(k.grip) + Vec3::Y * 0.04;
                    let rest = Body::rest().hands[0].pos;
                    let follow = swing(1.4, t0, hand, dir, speed, accel, steady);
                    b.hands[0].pos = if t < t0 {
                        keys(t, &[(0.0, rest), (0.4, above), (0.55, above), (1.0, start)])
                    } else if t < 1.4 {
                        swing(t, t0, hand, dir, speed, accel, steady)
                    } else {
                        keys(t, &[(1.4, follow), (1.9, rest)])
                    };
                    b.hands[0].rot = qkeys(t, &[(0.55, Quat::IDENTITY), (1.0, look_rot(dir))]);
                    b.hands[0].squeeze = on(t, &[(0.48, t0 + (accel + steady) as f64)]);
                    // Then a step to the right to see it in the plate.
                    b.head_pos = keys(t, &[(1.9, Body::rest().head_pos), (2.3, Body::rest().head_pos + Vec3::new(0.7, 0.0, 0.3))]);
                    b.head_rot = qkeys(t, &[(0.0, look_rot(above - Body::rest().head_pos)), (1.0, look_rot(target - Body::rest().head_pos)), (1.9, look_rot(target - Body::rest().head_pos)), (2.3, look_rot(target - b.head_pos))]);
                    b
                })
            },
            checks: vec![
                check(0.65, "the knife is in the left hand", |c| {
                    ensure(c.objects.iter().any(|o| o.kind == OBJ_THROWABLE && o.throw_flags & THROW_KNIFE != 0 && o.flags & OS_HELD_LEFT != 0), || "not held".into())
                }),
                check(1.4, "thrown", |c| ensure(c.rig.knife_throws == 1 && c.rig.last_throw_speed > 8.0, || format!("knife throws {} at {:.1} m/s", c.rig.knife_throws, c.rig.last_throw_speed))),
                check(2.4, "it stuck in the target, blade first", |c| {
                    let k = throwable(c, THROW_KNIFE).ok_or("no knife in flight or stuck")?;
                    let tip = c.rig.last_stick.ok_or("it never stuck")?;
                    let off = (tip - Vec3::new(-1.5, 1.4, 5.0)).truncate().length();
                    let blade = k.rot * (k.muzzle - k.grip).normalize();
                    ensure(k.flags & OS_STUCK != 0 && c.rig.sticks == 1 && off < 0.3 && blade.z > 0.5 && c.stat_f(rs::F_KNIFE_HITS) >= 1.0, || {
                        format!("flags {:#x}, sticks {}, tip {tip:?} ({off:.2} m off), blade {blade:?}, range knife hits {}", k.flags, c.rig.sticks, c.stat_f(rs::F_KNIFE_HITS))
                    })
                }),
                check(2.9, "and stays there", |c| ensure(throwable(c, THROW_KNIFE).is_some_and(|k| k.flags & OS_STUCK != 0), || "fell out".into())),
            ],
            captures: vec![(1.0, "knife_aim"), (1.5, "knife_flight"), (2.5, "knife_stuck")],
        },
        Scenario {
            name: "gloves",
            about: "physical: point at a can on the far table, grip, flick it to you and catch it",
            duration: 2.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(can) = prop_near(objs, Vec3::new(2.0, 0.91, 2.3)) else { return b };
                    let hand = Vec3::new(0.3, 1.2, 0.35);
                    let flick = hand + Vec3::new(0.0, 0.08, -0.15);
                    b.head_rot = look_rot(can.pos - Body::rest().head_pos);
                    b.hands[1].pos = keys(t, &[(0.0, Body::rest().hands[1].pos), (0.4, hand), (0.7, hand), (0.76, flick)]);
                    b.hands[1].rot = look_rot(can.pos - hand);
                    b.hands[1].squeeze = on(t, &[(0.6, 9.0)]);
                    b
                })
            },
            checks: vec![
                check(0.55, "pointing at the can highlights it", |c| {
                    ensure(c.objects.iter().any(|o| o.kind == OBJ_PROP && o.flags & OS_HOVERED != 0), || "nothing hovered".into())
                }),
                check(0.9, "the flick pulled it", |c| ensure(c.rig.pulls == 1, || format!("pulls {}", c.rig.pulls))),
                check(2.5, "caught in the hand", |c| {
                    let held = c.objects.iter().any(|o| o.kind == OBJ_PROP && o.flags & OS_HELD_RIGHT != 0);
                    ensure(c.rig.catches == 1 && held, || format!("catches {} held {held}", c.rig.catches))
                }),
            ],
            captures: vec![(0.55, "target"), (1.1, "flying")],
        },
        Scenario {
            name: "two_hand",
            about: "physical: support hand on the foregrip steers the rifle",
            duration: 3.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(rifle) = near_rifle(objs) else { return b };
                    let aim = aim_gun(&rifle, Body::rest().eye(1), TARGET_AHEAD, 0.15);
                    let hold = (aim.0 + Vec3::new(0.0, -0.1, 0.1), aim.1);
                    let (p, r) = take_rifle(t, &rifle);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.5, hold.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.5, hold.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    let gun = gun_rot_in(&rifle, hold.1);
                    let fore = on_held(&rifle, hold, rifle.foregrip) - if rifle.support_rot.is_some() { Vec3::ZERO } else { gun * Vec3::Y * 0.03 };
                    b.hands[0].pos = keys(t, &[(1.4, Body::rest().hands[0].pos), (1.9, fore), (2.2, fore), (2.6, fore + Vec3::new(0.08, 0.12, 0.0))]);
                    b.hands[0].rot = (gun * rifle.support_rot.unwrap_or(Quat::IDENTITY)).normalize();
                    b.hands[0].squeeze = on(t, &[(2.0, 9.0)]);
                    b
                })
            },
            checks: vec![
                check(2.15, "the support hand holds the foregrip", |c| {
                    let w = c.out.weapon.as_ref().ok_or("no weapon")?;
                    ensure(w.two_handed, || "one-handed".into())
                }),
                check(2.9, "and the barrel follows it", |c| {
                    let w = c.out.weapon.as_ref().ok_or("no weapon")?;
                    ensure(w.aim_dir.y > 0.12 && w.aim_dir.x > 0.04, || format!("aim {:?}", w.aim_dir))
                }),
            ],
            captures: vec![(2.15, "two_hand")],
        },
        Scenario {
            name: "body",
            about: "full body: the runtime solves and draws your body; its wrists follow the hands",
            duration: 2.4,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    // Look down at yourself (the head turns about the neck, so the eyes move down
                    // and forward), then hold both hands out in front.
                    b.head_rot = Quat::from_rotation_x(0.9 * ramp(t, 0.2, 0.8));
                    let eye_from_neck = Vec3::new(0.0, 0.075, 0.08);
                    b.head_pos = Body::rest().head_pos - eye_from_neck + b.head_rot * eye_from_neck;
                    let out = [Vec3::new(-0.25, 1.25, 0.45), Vec3::new(0.25, 1.25, 0.45)];
                    for h in 0..2 {
                        b.hands[h].pos = keys(t, &[(0.8, Body::rest().hands[h].pos), (1.4, out[h])]);
                        b.hands[h].rot = Quat::from_rotation_x(-0.5 * ramp(t, 0.8, 1.4));
                    }
                    b.hands[1].squeeze = if t > 1.7 { 1.0 } else { 0.0 };
                    b
                })
            },
            checks: vec![
                check(1.6, "the runtime draws the body and tells the game", |c| ensure(c.draws_body && c.body.flags & BODY_VALID != 0, || format!("draws {} flags {:#x}", c.draws_body, c.body.flags))),
                check(1.6, "every joint is finite and the body stands on the floor", |c| {
                    let ok = c.body.joints.iter().all(|j| j.pos.iter().chain(j.rot.iter()).all(|v| v.is_finite()));
                    let feet = [Joint::LeftFoot, Joint::RightFoot].map(|j| c.body.joints[j as usize].pos[1]);
                    ensure(ok && feet.iter().all(|y| (-0.05..0.25).contains(y)), || format!("finite {ok}, feet at {feet:?}"))
                }),
                check(1.6, "the wrists are where the hands are drawn, and both arms reach", |c| {
                    let d = [0, 1].map(|h| {
                        let wrist = Vec3::from_array(c.body.joints[if h == 0 { Joint::LeftHand } else { Joint::RightHand } as usize].pos);
                        wrist.distance(Vec3::from_array(c.body.hands[h].pose.pos))
                    });
                    ensure(d.iter().all(|d| *d < 0.03) && c.reached == [true, true], || format!("wrist offsets {d:?}, reached {:?}", c.reached))
                }),
                check(2.3, "a fist when the grip is squeezed", |c| {
                    let curl = c.body.fingers[1].curl;
                    ensure(curl[2] > 0.7 && c.body.fingers[0].curl[2] < 0.4, || format!("right {curl:?} left {:?}", c.body.fingers[0].curl))
                }),
            ],
            captures: vec![(0.9, "look_down"), (2.3, "hands")],
        },
        Scenario {
            name: "short_player",
            about: "a player shorter than the game's character: the body stands on the game's floor, legs straight",
            duration: 1.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: short_body,
            settings: |_| {},
            input: || {
                // Eyes at 1.52 m for real; the range puts them at its character's 1.7 m.
                Box::new(|_, _| short_body())
            },
            checks: vec![
                check(0.2, "the game's floor is below the real one by the height difference", |c| ensure((c.floor + 0.18).abs() < 0.03, || format!("floor {:.3}", c.floor))),
                check(1.5, "feet on the game's floor, not floating above it", |c| {
                    let feet = [Joint::LeftFoot, Joint::RightFoot].map(|j| c.body.joints[j as usize].pos[1] - c.floor);
                    ensure(feet.iter().all(|y| (-0.03..0.15).contains(y)), || format!("feet {feet:?} above the floor"))
                }),
                check(1.5, "standing straight, not crouched", |c| {
                    let hips = c.body.joints[Joint::Hips as usize].pos[1] - c.floor;
                    ensure(hips > 0.85, || format!("hips {hips:.2} above the floor"))
                }),
            ],
            captures: vec![],
        },
        Scenario {
            name: "seated",
            about: "seated mode: a seated player sees from standing height and their body is drawn standing",
            duration: 1.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: seated_body,
            settings: |s| s.comfort.seated = true,
            input: || {
                Box::new(|_, _| seated_body())
            },
            checks: vec![
                check(1.5, "the game didn't crouch", |c| ensure(c.stat(rs::CROUCHED) == 0, || "crouched".into())),
                check(1.5, "the body stands on the game's floor", |c| {
                    let hips = c.body.joints[Joint::Hips as usize].pos[1] - c.floor;
                    let feet = c.body.joints[Joint::LeftFoot as usize].pos[1] - c.floor;
                    ensure(hips > 0.85 && feet.abs() < 0.15, || format!("hips {hips:.2} feet {feet:.2} above the floor"))
                }),
            ],
            captures: vec![],
        },
        Scenario {
            name: "vignette",
            about: "comfort: walking with the stick closes a vignette in; standing still opens it again",
            duration: 2.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    b.hands[0].stick = glam::Vec2::new(0.0, if between(t, 0.2, 1.4) { 1.0 } else { 0.0 });
                    b
                })
            },
            checks: vec![
                check(1.3, "walking: the vignette is in", |c| ensure(c.stats.vignette > 0.4, || format!("vignette {:.2}", c.stats.vignette))),
                check(2.5, "stopped: it's gone again", |c| ensure(c.stats.vignette < 0.05, || format!("vignette {:.2}", c.stats.vignette))),
            ],
            captures: vec![(1.2, "walking")],
        },
        Scenario {
            name: "wall_fade",
            about: "comfort: putting your head into the table fades the view out instead of showing its inside",
            duration: 2.5,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |s| s.input.physical_crouch = false,
            input: || {
                Box::new(|t, _| {
                    let mut b = Body::rest();
                    let into = Vec3::new(0.95, 0.75, 0.55);
                    b.head_pos = keys(t, &[(0.0, b.head_pos), (1.0, Vec3::new(0.7, 1.2, 0.3)), (1.4, into)]);
                    b.head_rot = Quat::from_rotation_x(0.6 * ramp(t, 0.2, 1.0));
                    b
                })
            },
            checks: vec![
                check(0.9, "above the table: clear", |c| ensure(c.stats.fade < 0.1, || format!("fade {:.2}", c.stats.fade))),
                check(2.3, "head inside it: faded out", |c| ensure(c.stats.fade > 0.8, || format!("fade {:.2}", c.stats.fade))),
            ],
            captures: vec![(2.3, "inside")],
        },
        Scenario {
            name: "wall_hand",
            about: "physical: a hand pushed into the table stops at its surface; a ghost shows the real hand",
            duration: 2.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, _, _| {
                    let mut b = Body::rest();
                    b.head_rot = look_rot(Vec3::new(1.3, 0.6, 0.35) - Body::rest().head_pos);
                    let front = Vec3::new(1.3, 0.62, 0.05);
                    b.hands[1].pos = keys(t, &[(0.0, Body::rest().hands[1].pos), (0.5, front), (1.0, front + Vec3::Z * 0.4)]);
                    b
                })
            },
            checks: vec![check(1.5, "the hand was held back by the table", |c| ensure(c.rig.blocked_frames > 20, || format!("blocked {} frames", c.rig.blocked_frames)))],
            captures: vec![(1.4, "ghost")],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframes_hold_the_ends_and_ease_between() {
        let k = [(1.0, Vec3::ZERO), (2.0, Vec3::X)];
        assert_eq!(keys(0.0, &k), Vec3::ZERO);
        assert_eq!(keys(3.0, &k), Vec3::X);
        assert!((keys(1.5, &k).x - 0.5).abs() < 1e-5);
    }
}
