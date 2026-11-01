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

/// The head leaning in to look at `at` from `eye` between `t0` and `t1`, back from `t2` to `t3`
/// (for a close look at the hands).
fn lean_in(b: &mut Body, t: f64, [t0, t1, t2, t3]: [f64; 4], eye: Vec3, at: Vec3) {
    let k = ramp(t, t0, t1) * (1.0 - ramp(t, t2, t3));
    if k > 0.0 {
        b.head_pos = b.head_pos.lerp(eye, k);
        b.head_rot = b.head_rot.slerp(look_rot(at - eye), k);
    }
}

/// The body stepping towards `at` (a thing on the table) between `t0` and `t1`, as far as a person
/// would to reach it, and back from `t2` to `t3`: a weapon taken from where it rests is let go of
/// past the player's reach from their shoulders (the stuck rule), as a hand couldn't be there.
fn step_to(b: &mut Body, t: f64, [t0, t1, t2, t3]: [f64; 4], at: Vec3) {
    let k = ramp(t, t0, t1) * (1.0 - ramp(t, t2, t3));
    let to = (at - b.head_pos).with_y(0.0);
    b.head_pos += to.normalize_or_zero() * (to.length() - STEP_REACH).max(0.0) * k;
}

/// How far (horizontally, m) the eyes stay from what the hand takes off the table.
const STEP_REACH: f32 = 0.55;

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

/// The plain rifle on the table at your right hand (the nearer of the two). (In front of you: the
/// range's armory bench behind you has guns too.)
fn near_rifle(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter()
        .filter(|o| o.kind == OBJ_WEAPON && o.flags & OS_HOLSTERED == 0 && (0.4..1.6).contains(&o.pos.x) && (0.2..1.1).contains(&o.pos.z) && o.pos.y > 0.7)
        .min_by(|a, b| a.pos.z.total_cmp(&b.pos.z))
        .copied()
}

fn holstered_gun(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter().find(|o| o.kind == OBJ_WEAPON && o.flags & OS_HOLSTERED != 0).copied()
}

/// Standing at the range's valve board, facing it (+X), half a metre from it, hands low.
fn at_valve_board() -> Body {
    let mut b = Body::rest();
    b.head_pos = Vec3::new(1.55, 1.6, -0.4);
    b.head_rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    b.hands[0].pos = Vec3::new(1.85, 1.0, -0.15);
    b.hands[1].pos = Vec3::new(1.85, 1.0, -0.65);
    b
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
    // Then slowly down onto it (the hand stops on the gun, round its grip).
    let on = grip + Vec3::Y * 0.015;
    let level = hand_rot_for(rifle, Quat::IDENTITY);
    let pos = keys(t, &[(0.0, rest), (0.35, Vec3::new(grip.x, 1.2, grip.z)), (0.5, above), (0.62, on), (0.75, on), (1.0, Vec3::new(0.5, 1.3, 0.35))]);
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
/// The sword on the throwables' table.
/// Where the stab into the floor goes in (x; clear of the tables, in front of the player).
const STAB_FLOOR_X: f32 = 0.15;

fn table_sword(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter().find(|o| o.kind == OBJ_MELEE && o.flags & OS_HOLSTERED == 0).copied()
}

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
                    // Lifted clear of the table first (dragged across it, a held gun lags behind).
                    let lifted = if t < 1.0 { p } else { take_rifle(1.0, &rifle).0 + Vec3::Y * 0.3 };
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, take_rifle(1.0, &rifle).0), (1.25, lifted), (1.6, aim.0)]) };
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
                    // Round its neck from the left: the fist upright, the palm towards it, brought
                    // in slowly until the palm is on it (the controller goes on into it).
                    let neck = bottle.pos + Vec3::Y * (bottle.half_extents.y * 0.6);
                    let beside = neck - Vec3::X * 0.12;
                    let rest = Body::rest().hands[0].pos;
                    b.head_rot = Quat::from_rotation_x(0.45 * (1.0 - ramp(t, 0.9, 1.2)));
                    b.hands[0].pos = keys(
                        t,
                        &[(0.0, rest), (0.2, beside + Vec3::Y * 0.3), (0.35, beside), (0.5, neck - Vec3::X * 0.04), (0.7, neck), (1.0, Vec3::new(-0.35, 1.4, 0.2)), (1.3, Vec3::new(-0.35, 1.5, -0.15)), (1.45, Vec3::new(-0.3, 1.65, 0.65))],
                    );
                    b.hands[0].rot = qkeys(t, &[(0.0, Quat::IDENTITY), (0.2, Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))]);
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
                    step_to(&mut b, t, [0.1, 0.35, 0.65, 0.95], above);
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
            name: "sword",
            about: "physical: the sword from the throwables' table in the right hand, a chop down through the training dummy: a clean, strong hit; then a slow touch: none",
            duration: 3.2,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(s) = table_sword(objs) else { return b };
                    let above = s.at(s.grip) + Vec3::Y * 0.05;
                    // Raised point up, then the blade forward and level over the dummy's head, its
                    // edges up and down (the back of the hand to the flat); brought down through the
                    // dummy, then (after a pause) laid on it slowly.
                    let raised = Vec3::new(0.2, 1.65, 0.2);
                    let up = Vec3::new(0.2, 1.9, 0.3);
                    let down = Vec3::new(0.2, 0.95, 0.42);
                    let rest = Body::rest().hands[1].pos;
                    b.hands[1].pos = keys(t, &[(0.0, rest), (0.25, above + Vec3::Y * 0.25), (0.45, above), (0.6, above), (0.85, raised), (1.0, up), (1.2, up), (1.36, down), (1.8, down), (2.2, Vec3::new(0.2, 1.38, 0.42)), (2.8, Vec3::new(0.2, 1.22, 0.42))]);
                    // Turned to hold it as it lies (thumb towards the blade), then point up, then
                    // forward.
                    let lies = hand_rot_for(&s, s.rot);
                    b.hands[1].rot = qkeys(t, &[(0.0, Quat::IDENTITY), (0.25, lies), (0.6, lies), (0.85, look_rot(Vec3::Y)), (1.0, look_rot(Vec3::Z))]);
                    b.hands[1].squeeze = on(t, &[(0.5, 3.2)]);
                    step_to(&mut b, t, [0.1, 0.4, 0.7, 0.95], above);
                    b
                })
            },
            checks: vec![
                check(1.0, "the sword is in the right hand", |c| ensure(c.objects.iter().any(|o| o.kind == OBJ_MELEE && o.flags & OS_HELD_RIGHT != 0), || "not held".into())),
                check(1.6, "the chop hit the dummy, cleanly and hard", |c| {
                    ensure(c.stat_f(rs::F_MELEE_HITS) == 1.0 && c.stat_f(rs::F_MELEE_POWER) <= 1.0, || format!("hits {}, power {}", c.stat_f(rs::F_MELEE_HITS), c.stat_f(rs::F_MELEE_POWER)))
                }),
                check(3.1, "laying it on slowly isn't another hit", |c| ensure(c.stat_f(rs::F_MELEE_HITS) == 1.0, || format!("hits {}", c.stat_f(rs::F_MELEE_HITS)))),
            ],
            captures: vec![(1.2, "sword_raised"), (1.4, "sword_chop")],
        },
        Scenario {
            name: "stab",
            about: "physical: the sword taken by its handle, thrust point first into the dummy's chest: it goes in and stays; pulled sideways it drags the dummy (the blade stays in); pulled back the way it went in, it comes out",
            duration: 4.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(s) = table_sword(objs) else { return b };
                    let above = s.at(s.grip) + Vec3::Y * 0.05;
                    let rest = Body::rest().hands[1].pos;
                    // Level, the point ahead, at the dummy's chest height: back, then thrust in
                    // (0.35 m in 0.12 s: about 3 m/s), held, dragged 15 cm right, held, pulled out.
                    let back = Vec3::new(0.15, 1.25, -0.12);
                    let into = back + Vec3::Z * 0.36;
                    let dragged = into + Vec3::X * 0.15;
                    b.hands[1].pos = keys(
                        t,
                        &[(0.0, rest), (0.25, above + Vec3::Y * 0.25), (0.6, above), (0.75, above), (1.3, back), (1.6, back), (1.72, into), (2.2, into), (2.7, dragged), (3.1, dragged), (3.3, into), (4.0, back - Vec3::Z * 0.1)],
                    );
                    let lies = hand_rot_for(&s, s.rot);
                    b.hands[1].rot = qkeys(t, &[(0.0, Quat::IDENTITY), (0.25, lies), (0.75, lies), (1.3, look_rot(Vec3::Z))]);
                    b.hands[1].squeeze = on(t, &[(0.65, 4.6)]);
                    step_to(&mut b, t, [0.1, 0.5, 0.85, 1.2], above);
                    b.head_rot = Quat::from_rotation_x(0.2);
                    b
                })
            },
            checks: vec![
                check(1.5, "the sword is in the right hand", |c| ensure(c.objects.iter().any(|o| o.kind == OBJ_MELEE && o.flags & OS_HELD_RIGHT != 0), || "not held".into())),
                check(2.1, "the thrust went in and stayed", |c| ensure(c.rig.embeds >= 1, || format!("{} embeds", c.rig.embeds))),
                check(2.1, "it was a hit", |c| ensure(c.stat_f(rs::F_MELEE_HITS) >= 1.0, || format!("hits {}", c.stat_f(rs::F_MELEE_HITS)))),
                check(4.5, "still in the hand once out", |c| ensure(c.objects.iter().any(|o| o.kind == OBJ_MELEE && o.flags & OS_HELD_RIGHT != 0), || "dropped".into())),
            ],
            captures: vec![(2.15, "stab_in"), (3.0, "stab_dragged"), (4.4, "stab_out")],
        },
        Scenario {
            name: "stab_floor",
            about: "physical: the sword taken by its handle, thrust point down into the floor: it goes in and stays; the hand pulled sideways doesn't move it (the blade stays where it went in); pulled back up the way it went in, it comes out",
            duration: 4.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(s) = table_sword(objs) else { return b };
                    let above = s.at(s.grip) + Vec3::Y * 0.05;
                    let rest = Body::rest().hands[1].pos;
                    // Point down, the point 44 cm over the floor: then thrust (0.5 m in 0.12 s, the
                    // point 6 cm into the floor), held, the hand pulled 15 cm right, held, back,
                    // pulled up out.
                    let back = Vec3::new(STAB_FLOOR_X, 1.3, 0.4);
                    let into = back - Vec3::Y * 0.5;
                    let dragged = into + Vec3::X * 0.15;
                    b.hands[1].pos = keys(
                        t,
                        &[(0.0, rest), (0.25, above + Vec3::Y * 0.25), (0.6, above), (0.75, above), (1.3, back), (1.6, back), (1.72, into), (2.2, into), (2.7, dragged), (3.1, dragged), (3.3, into), (4.0, back + Vec3::Y * 0.05)],
                    );
                    let lies = hand_rot_for(&s, s.rot);
                    b.hands[1].rot = qkeys(t, &[(0.0, Quat::IDENTITY), (0.25, lies), (0.75, lies), (1.3, look_rot(Vec3::NEG_Y))]);
                    b.hands[1].squeeze = on(t, &[(0.65, 4.6)]);
                    step_to(&mut b, t, [0.1, 0.5, 0.85, 1.2], above);
                    b.head_rot = Quat::from_rotation_x(0.4);
                    b
                })
            },
            checks: vec![
                check(1.5, "the sword is in the right hand", |c| ensure(c.objects.iter().any(|o| o.kind == OBJ_MELEE && o.flags & OS_HELD_RIGHT != 0), || "not held".into())),
                check(2.1, "the thrust went into the floor and stayed", |c| ensure(c.rig.embeds >= 1, || format!("{} embeds", c.rig.embeds))),
                check(3.0, "pulled sideways, the blade stays where it went in", |c| {
                    let Some(s) = c.objects.iter().find(|o| o.kind == OBJ_MELEE && o.flags & OS_HELD_RIGHT != 0) else { return Err("not held".into()) };
                    // (Not dragged along with the hand, 15 cm to the right of the stab: the sword hangs
                    // in the hand as it was taken, so its point went in a few cm off the hand's line.)
                    ensure(s.pos.x < STAB_FLOOR_X + 0.05, || format!("the sword was dragged to x {:.3} (the hand stabbed at {STAB_FLOOR_X})", s.pos.x))
                }),
                check(4.5, "pulled up, it came out and is still in the hand", |c| {
                    let Some(s) = c.objects.iter().find(|o| o.kind == OBJ_MELEE && o.flags & OS_HELD_RIGHT != 0) else { return Err("dropped".into()) };
                    ensure(s.pos.y > 0.9, || format!("still in the floor: y {:.3}", s.pos.y))
                }),
            ],
            captures: vec![(2.15, "stab_floor_in"), (3.0, "stab_floor_dragged"), (4.4, "stab_floor_out")],
        },
        Scenario {
            name: "spell",
            about: "physical: the off hand charges the fire bolt with its trigger (half a second) and lets go pointing at the training dummy: one cast, aimed at the dummy; let go too soon: nothing",
            duration: 3.0,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, _, _| {
                    let mut b = Body::rest();
                    // The palm on the line from the left shoulder (at the eyes) to the dummy's chest.
                    let shoulder = Vec3::new(-0.18, 1.7, 0.0);
                    let chest = Vec3::new(0.15, 1.25, 1.15);
                    let dir = (chest - shoulder).normalize();
                    let palm = shoulder + dir * 0.5 - Vec3::new(0.0, 0.0, 0.06);
                    let rest = Body::rest().hands[0].pos;
                    b.hands[0].pos = keys(t, &[(0.0, rest), (0.4, palm)]);
                    b.hands[0].rot = qkeys(t, &[(0.0, Quat::IDENTITY), (0.4, look_rot(dir))]);
                    // Charged 0.8 s and let go; then a short tap (0.2 s, not charged).
                    b.hands[0].trigger = on(t, &[(0.5, 1.3), (2.0, 2.2)]);
                    b
                })
            },
            checks: vec![
                check(1.5, "one fire bolt, aimed at the dummy", |c| {
                    ensure(c.stat_f(rs::F_CASTS) == 1.0 && c.stat_f(rs::F_CASTS_AT_DUMMY) == 1.0, || format!("casts {}, at the dummy {}", c.stat_f(rs::F_CASTS), c.stat_f(rs::F_CASTS_AT_DUMMY)))
                }),
                check(2.9, "a tap isn't a cast", |c| ensure(c.stat_f(rs::F_CASTS) == 1.0, || format!("casts {}", c.stat_f(rs::F_CASTS)))),
            ],
            captures: vec![(1.0, "spell_charging")],
        },
        Scenario {
            name: "gloves",
            about: "physical: the arm reaching at a can on the far table, grip, flick it to you and catch it",
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
                    b.head_rot = look_rot(can.pos - Body::rest().head_pos);
                    // The right arm reaching at it from the shoulder, the controller held as it is.
                    let fwd = (can.pos - Body::rest().head_pos).with_y(0.0).normalize();
                    let shoulder = Body::rest().head_pos - Vec3::Y * 0.25 + Vec3::Y.cross(fwd) * 0.18;
                    let hand = shoulder + (can.pos - shoulder).normalize() * 0.55;
                    let flick = hand + Vec3::new(0.0, 0.08, -0.15);
                    b.hands[1].pos = keys(t, &[(0.0, Body::rest().hands[1].pos), (0.4, hand), (0.7, hand), (0.76, flick)]);
                    b.hands[1].rot = qkeys(t, &[(0.0, Quat::IDENTITY), (0.4, Quat::from_rotation_y(fwd.x.atan2(fwd.z)))]);
                    b.hands[1].squeeze = on(t, &[(0.6, 9.0)]);
                    b
                })
            },
            checks: vec![
                check(0.55, "reaching at the can highlights it", |c| {
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
            name: "valve",
            about: "physical: at the valve board, the right hand takes the free-running valve's rim and turns it a quarter round",
            duration: 2.2,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: at_valve_board,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = at_valve_board();
                    let Some(v) = objs.iter().filter(|o| o.kind == OBJ_VALVE).min_by(|a, b| a.pos.distance(Vec3::new(2.14, 0.95, -0.4)).total_cmp(&b.pos.distance(Vec3::new(2.14, 0.95, -0.4)))).copied() else { return b };
                    let r = 0.18;
                    let at = |a: f32| v.pos + Vec3::new(-0.02, r * a.cos(), r * a.sin());
                    let circle = ((t - 0.8) / 1.0).clamp(0.0, 1.0) as f32 * std::f32::consts::FRAC_PI_2;
                    b.hands[1].pos = if t < 0.8 { keys(t, &[(0.0, at_valve_board().hands[1].pos), (0.4, at(0.0) + Vec3::new(-0.15, 0.08, 0.0)), (0.6, at(0.0))]) } else { at(circle) };
                    b.hands[1].rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
                    b.hands[1].squeeze = on(t, &[(0.7, 9.0)]);
                    b
                })
            },
            checks: vec![
                check(0.65, "the hand at the rim highlights the valve", |c| {
                    ensure(c.objects.iter().any(|o| o.kind == OBJ_VALVE && o.flags & OS_HOVERED != 0), || "no valve hovered".into())
                }),
                check(2.1, "turned a quarter round with the hand", |c| {
                    let spawn = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
                    let turned = c.objects.iter().filter(|o| o.kind == OBJ_VALVE).map(|o| o.rot.angle_between(spawn)).fold(0.0f32, f32::max);
                    ensure((turned - std::f32::consts::FRAC_PI_2).abs() < 0.3, || format!("turned {turned:.2} rad"))
                }),
            ],
            captures: vec![(1.3, "valve_turning")],
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
        Scenario {
            name: "grip_handle",
            about: "physical: the sword taken by the pommel end of its handle (the hand goes on there, as the sword is held) and raised; then the left hand takes its blade: both hands on it, the fingers closed round handle and blade",
            duration: 3.4,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(s) = table_sword(objs) else { return b };
                    b.head_rot = Quat::from_rotation_x(0.35);
                    // Down onto the handle by the pommel, then up in front of the eyes, the blade up
                    // and forward.
                    let above = s.at(Vec3::new(0.0, 0.0, -0.095)) + Vec3::Y * 0.05;
                    let raised = Vec3::new(0.1, 1.42, 0.36);
                    let rest = Body::rest().hands[1].pos;
                    b.hands[1].pos = keys(t, &[(0.0, rest), (0.4, above + Vec3::Y * 0.2), (0.7, above), (0.85, above), (1.3, raised)]);
                    b.hands[1].rot = qkeys(t, &[(0.85, Quat::IDENTITY), (1.3, Quat::from_rotation_x(-45f32.to_radians()))]);
                    b.hands[1].squeeze = on(t, &[(0.75, 3.4)]);
                    step_to(&mut b, t, [0.15, 0.6, 0.95, 1.2], above);
                    // The left hand to the blade's flat, 25 cm up it, from its side.
                    let rest = Body::rest().hands[0].pos;
                    let blade = live(see, s.id).filter(|_| t > 1.6).map(|now| now.at(Vec3::new(0.0, -0.04, 0.14)));
                    if let Some(p) = blade {
                        b.hands[0].pos = rest.lerp(p, ramp(t, 1.6, 2.2));
                        b.hands[0].rot = Quat::from_rotation_x(-45f32.to_radians());
                    } else {
                        b.hands[0].pos = rest;
                    }
                    b.hands[0].squeeze = on(t, &[(2.3, 3.4)]);
                    // A close look at the hand on the handle, then at both hands.
                    lean_in(&mut b, t, [1.2, 1.45, 1.6, 1.75], raised + Vec3::new(-0.24, 0.06, 0.14), raised);
                    if let Some(p) = blade {
                        let mid = p.lerp(raised, 0.5);
                        lean_in(&mut b, t, [2.6, 2.9, 3.3, 3.4], mid + Vec3::new(-0.3, 0.02, 0.12), mid);
                    }
                    b
                })
            },
            checks: vec![
                check(1.4, "the sword is in the right hand", |c| ensure(c.objects.iter().any(|o| o.kind == OBJ_MELEE && o.flags & OS_HELD_RIGHT != 0), || "not held".into())),
                check(3.0, "both hands on it", |c| {
                    let sword = c.objects.iter().find(|o| o.kind == OBJ_MELEE).map(|o| o.id).ok_or("no sword")?;
                    let on = |i: usize| c.body.hands[i].flags & HAND_HOLDING != 0 && c.body.hands[i].holding == sword;
                    ensure(on(0) && on(1), || format!("hands {:?}", c.body.hands.map(|h| (h.flags, h.holding))))
                }),
            ],
            captures: vec![(1.5, "grip_handle_pommel"), (3.0, "grip_handle_and_blade")],
        },
        Scenario {
            name: "grip_handguard",
            about: "physical: the rifle taken by its pistol grip and held in front; the left hand takes the handguard at its back end, lets go and takes it again further forward: on the handguard where it took it each time",
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
                    b.head_rot = Quat::from_rotation_x(0.3);
                    let level = hand_rot_for(&rifle, Quat::IDENTITY);
                    let held = (Vec3::new(0.12, 1.36, 0.22), level);
                    let (p, q) = take_rifle(t.min(1.0), &rifle);
                    b.hands[1].pos = if t < 1.0 { p } else { p.lerp(held.0, ramp(t, 1.0, 1.4)) };
                    b.hands[1].rot = if t < 1.0 { q } else { q.slerp(held.1, ramp(t, 1.0, 1.4)) };
                    b.hands[1].squeeze = on(t, &[(0.65, 4.4)]);
                    // The handguard from under it: 4 cm back of the game's support hold, then 6 cm
                    // ahead of it.
                    let gun = gun_rot_in(&rifle, held.1);
                    let under = |along: f32| on_held(&rifle, held, rifle.foregrip + Vec3::Z * along) - gun * Vec3::Y * 0.02;
                    let rest = Body::rest().hands[0].pos;
                    let (back, front) = (under(-0.04), under(0.06));
                    b.hands[0].pos = keys(t, &[(1.4, rest), (1.9, back), (2.6, back), (2.9, back - Vec3::Y * 0.06), (3.2, front), (4.4, front)]);
                    b.hands[0].rot = rifle.support_rot.map(|s| (gun * s).normalize()).unwrap_or(Quat::IDENTITY);
                    b.hands[0].squeeze = on(t, &[(2.0, 2.65), (3.3, 4.4)]);
                    // Close looks at the support hand from the left, a little below.
                    lean_in(&mut b, t, [2.1, 2.4, 2.55, 2.7], back + Vec3::new(-0.26, -0.04, 0.06), back);
                    lean_in(&mut b, t, [3.7, 4.0, 4.3, 4.4], front + Vec3::new(-0.26, -0.04, 0.06), front);
                    b
                })
            },
            checks: vec![
                check(1.2, "the rifle is in the right hand", |c| ensure(gun_held_right(c), || "not held".into())),
                check(2.5, "the left hand on the handguard", |c| {
                    let gun = c.objects.iter().find(|o| o.kind == OBJ_WEAPON && o.flags & OS_HELD_RIGHT != 0).map(|o| o.id).ok_or("no rifle")?;
                    ensure(c.body.hands[0].flags & HAND_HOLDING != 0 && c.body.hands[0].holding == gun, || format!("left {:?}", (c.body.hands[0].flags, c.body.hands[0].holding)))
                }),
                check(4.2, "and again further forward", |c| {
                    let gun = c.objects.iter().find(|o| o.kind == OBJ_WEAPON && o.flags & OS_HELD_RIGHT != 0).ok_or("no rifle")?;
                    let h = &c.body.hands[0];
                    ensure(h.flags & HAND_HOLDING != 0 && h.holding == gun.id, || "not holding".into())
                }),
            ],
            captures: vec![(2.45, "grip_handguard_back"), (4.1, "grip_handguard_front"), (1.35, "grip_handguard_firing")],
        },
        Scenario {
            name: "grip_gun_barrel",
            about: "physical: the rifle taken by its barrel, well ahead of its grip, and lifted: held there, the fingers closed round the barrel itself (its convex parts, not a box round the gun); the trigger does nothing",
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
                    let Some(rifle) = near_rifle(objs) else { return b };
                    b.head_rot = Quat::from_rotation_x(0.35);
                    // Over the barrel 15 cm behind the muzzle, then down onto it slowly.
                    let barrel = rifle.at(rifle.muzzle - Vec3::Z * 0.15);
                    let rest = Body::rest().hands[1].pos;
                    let lifted = Vec3::new(0.12, 1.4, 0.38);
                    b.hands[1].pos = keys(t, &[(0.0, rest), (0.4, barrel + Vec3::Y * 0.25), (0.9, barrel + Vec3::Y * 0.02), (1.0, barrel + Vec3::Y * 0.02), (1.5, lifted)]);
                    b.hands[1].squeeze = on(t, &[(0.95, 2.6)]);
                    b.hands[1].trigger = on(t, &[(1.9, 2.1)]);
                    lean_in(&mut b, t, [1.5, 1.75, 2.4, 2.6], lifted + Vec3::new(-0.22, 0.05, 0.16), lifted);
                    b
                })
            },
            checks: vec![
                check(1.6, "the rifle is in the right hand", |c| ensure(gun_held_right(c), || "not held".into())),
                check(2.4, "the trigger fired nothing", |c| ensure(c.rig.shots == 0, || format!("{} shots", c.rig.shots))),
            ],
            captures: vec![(1.85, "grip_gun_barrel")],
        },
        Scenario {
            name: "grip_hand_to_hand",
            about: "physical: the bottle taken from its side and lifted in front, the other hand takes it too, then the first lets go: it's in the other hand, where that hand took it",
            duration: 4.2,
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
                    b.head_rot = Quat::from_rotation_x(0.35);
                    // The fist's axis up the bottle, the palm against its side (the controller a
                    // little into it: the hand stops on it).
                    let upright = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                    let side = bottle.pos + Vec3::Y * (bottle.half_extents.y * 0.6) - Vec3::X * (bottle.half_extents.x - 0.01);
                    let lifted = Vec3::new(-0.08, 1.4, 0.36);
                    let rest = Body::rest().hands[0].pos;
                    // Round its neck: over it, down beside it, then in from the side (it stands).
                    let beside = side - Vec3::X * 0.12;
                    b.hands[0].pos = keys(t, &[(0.0, rest), (0.2, beside + Vec3::Y * 0.3), (0.35, beside), (0.5, side - Vec3::X * 0.04), (0.75, side), (0.95, side), (1.4, lifted), (3.0, lifted), (3.4, rest)]);
                    b.hands[0].rot = upright;
                    b.hands[0].squeeze = on(t, &[(0.85, 3.0)]);
                    // The right hand to its other side once it's up: over beside it, then slowly
                    // in until the hand is on it (it rests on it rather than shoving it out of the
                    // left hand).
                    let rest = Body::rest().hands[1].pos;
                    let held_at = lifted + Vec3::X * (bottle.half_extents.x - 0.01) - Vec3::Y * (bottle.half_extents.y * 0.6);
                    let other = held_at + Vec3::X * (bottle.half_extents.x - 0.01);
                    b.hands[1].pos = keys(t, &[(1.5, rest), (1.9, other + Vec3::X * 0.08), (2.15, other)]);
                    b.hands[1].rot = upright;
                    b.hands[1].squeeze = on(t, &[(2.2, 4.2)]);
                    lean_in(&mut b, t, [1.2, 1.5, 1.65, 1.8], lifted + Vec3::new(0.05, 0.08, 0.3), lifted);
                    lean_in(&mut b, t, [2.4, 2.65, 2.8, 2.95], lifted + Vec3::new(0.05, 0.08, 0.3), lifted);
                    let right = b.hands[1].pos;
                    lean_in(&mut b, t, [3.7, 3.95, 4.1, 4.2], right + Vec3::new(-0.05, 0.08, 0.3), right);
                    b
                })
            },
            checks: vec![
                check(1.4, "the bottle is in the left hand", |c| ensure(c.body.hands[0].flags & HAND_HOLDING != 0 && c.body.hands[0].holding != 0, || "not held".into())),
                check(2.6, "both hands have it", |c| {
                    let [l, r] = c.body.hands;
                    ensure(l.flags & r.flags & HAND_HOLDING != 0 && l.holding == r.holding && l.holding != 0, || format!("left {:?} right {:?}", (l.flags, l.holding), (r.flags, r.holding)))
                }),
                check(4.0, "handed over: the right hand has it", |c| {
                    let [l, r] = c.body.hands;
                    ensure(r.flags & HAND_HOLDING != 0 && r.holding != 0 && l.flags & HAND_HOLDING == 0, || format!("left {:?} right {:?}", (l.flags, l.holding), (r.flags, r.holding)))
                }),
            ],
            captures: vec![(1.55, "grip_bottle_left"), (2.7, "grip_bottle_both"), (4.0, "grip_bottle_right")],
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
