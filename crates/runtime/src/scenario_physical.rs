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

/// The nearest gun on that table that kicks the way its game does (Metro's Kalash).
fn near_game_rifle(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter()
        .filter(|o| o.kind == OBJ_WEAPON && o.kicks > 0 && o.flags & OS_HOLSTERED == 0 && (0.4..1.6).contains(&o.pos.x) && (0.2..1.6).contains(&o.pos.z) && o.pos.y > 0.7)
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
/// The sword on the throwables' table.
fn table_sword(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter().find(|o| o.kind == OBJ_MELEE && o.flags & OS_HOLSTERED == 0).copied()
}

fn live(see: &See, id: u32) -> Option<ObjSnap> {
    see.world.objects.iter().find(|o| o.id == id).copied()
}

fn throwable<'a>(c: &Ctx<'a>, flag: u32) -> Option<&'a ObjSnap> {
    c.objects.iter().find(|o| o.kind == OBJ_THROWABLE && o.throw_flags & flag != 0 && o.flags & (OS_THROWN | OS_ARMED | OS_STUCK | OS_SPENT | OS_HELD_LEFT | OS_HELD_RIGHT) != 0)
}

/// Metro's dynamite on the throwables' table: a lit-fuse bomb (a pin, its fuse cord, without a
/// lever), not sticky, not set off by a hit.
fn table_dynamite(objs: &[ObjSnap]) -> Option<ObjSnap> {
    let kind = THROW_GRENADE | THROW_PIN | THROW_LEVER | THROW_STICKY | THROW_IMPACT;
    objs.iter().find(|o| o.kind == OBJ_THROWABLE && o.throw_flags & kind == (THROW_GRENADE | THROW_PIN) && o.flags & OS_HOLSTERED == 0 && o.pos.x < -0.4 && o.pos.z > 0.8).copied()
}

/// The runtime's audio when the range handed over Metro's sound pack (it does whenever Metro's
/// sounds have been extracted), to see which of its events played; `None` when it handed over
/// none, and the runtime's own sounds played.
fn game_pack<'a>(c: &Ctx<'a>) -> Result<Option<&'a rustvr_audio::Audio>, String> {
    let a = c.audio.ok_or("no audio")?;
    match a.pack_status() {
        rustvr_audio::PackStatus::None => Ok(None),
        rustvr_audio::PackStatus::Ready(_) => Ok(Some(a)),
        other => Err(format!("the sound pack: {other:?}")),
    }
}

/// With Metro's pack: `test` on its plays. Without: nothing to check.
fn pack_check(c: &Ctx, test: impl FnOnce(&dyn Fn(rustvr_audio::Event) -> u32) -> Result<(), String>) -> Result<(), String> {
    match game_pack(c)? {
        Some(a) => test(&|e| a.pack_plays(e)),
        None => Ok(()),
    }
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

// ---- guns worked by hand (`rig::actions`) --------------------------------------------------------

/// The gun the range laid nearest on the table at your right hand (`--table <key>` puts it there).
fn table_gun(objs: &[ObjSnap]) -> Option<ObjSnap> {
    objs.iter()
        .filter(|o| o.kind == OBJ_WEAPON && o.flags & OS_HOLSTERED == 0 && (0.0..2.0).contains(&o.pos.x) && (0.2..1.1).contains(&o.pos.z) && o.pos.y > 0.7)
        .min_by(|a, b| a.pos.z.total_cmp(&b.pos.z))
        .copied()
}

/// A hinged gun's chamber `k` as it is now (open by its `slide`): its mouth and the way it points
/// (tracking space).
fn chamber_at(gun: &ObjSnap, k: usize) -> (Vec3, Vec3) {
    let hinge = gun.handle;
    let axis = if gun.action == ACTION_SWING { Vec3::Z } else { Vec3::X };
    let turn = Quat::from_axis_angle(axis, gun.action_angle * gun.slide);
    let mouth = gun.mag_well + Vec3::X * gun.chamber_step * k as f32;
    (gun.at(hinge + turn * (mouth - hinge)), (gun.rot * turn * Vec3::Z).normalize())
}

/// A swung-out cylinder's well (where its speed loader's top goes, tracking space) and the turn
/// it's swung out by (tracking space rotation of the gun's frame).
fn cylinder_well(gun: &ObjSnap) -> (Vec3, Quat) {
    let turn = Quat::from_rotation_z(gun.action_angle * gun.slide);
    (gun.at(gun.handle + turn * (gun.mag_well - gun.handle)), (gun.rot * turn).normalize())
}

/// The hand holding a round from the pouch with its base at `base`, pointing along `dir`.
fn round_in_hand(base: Vec3, dir: Vec3) -> (Vec3, Quat) {
    let rot = (look_rot(dir) * crate::rig::held_round_rot().inverse()).normalize();
    (base - rot * crate::rig::HELD_ROUND_BASE, rot)
}

/// A bolt's knob (tracking space) lifted by `lift` and back by `back` (0..1 each): turned about
/// the bore, then slid back (see `rig::actions::Cycle::knob`).
fn knob_at(gun: &ObjSnap, lift: f32, back: f32) -> Vec3 {
    let home = gun.handle;
    let axis = Vec3::new(gun.muzzle.x, gun.muzzle.y, home.z);
    gun.at(axis + Quat::from_rotation_z(gun.action_angle * lift) * (home - axis) - Vec3::Z * back * gun.handle_travel)
}

/// The held gun in the live snapshot.
fn held_gun(c: &Ctx) -> Option<ObjSnap> {
    c.objects.iter().find(|o| o.kind == OBJ_WEAPON && o.flags & OS_HELD_RIGHT != 0).copied()
}

/// A gun pose `rot` turned by `deg` about its own +X (positive: the muzzle down).
fn pitched(rot: Quat, deg: f32) -> Quat {
    (rot * Quat::from_rotation_x(deg.to_radians())).normalize()
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
            name: "runtime_gun",
            about: "physical: the range has the runtime draw Metro's guns (as an adapter does): pick up the Kalash 2012, aim it through its collimator and fire; the runtime draws it in the hands",
            duration: 3.4,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--table", "wpn_2012", "--runtime-draws"],
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
                check(0.5, "the runtime draws the guns on the table", |c| ensure(c.stats.held_drawn >= 1, || format!("{} drawn", c.stats.held_drawn))),
                check(0.9, "the rifle is in the right hand", |c| ensure(gun_held_right(c), || "not held".into())),
                check(1.8, "raised to the eye it aims down its collimator", |c| {
                    let w = c.out.weapon.as_ref().ok_or("no weapon pose")?;
                    ensure(w.ads, || format!("sight {:.3} m from the eye", w.sight_to_eye))
                }),
                check(3.3, "the shots it fired hit the target", |c| {
                    ensure(c.stat(rs::SHOTS) >= 3 && c.stat(rs::HITS) >= 3, || format!("shots {} hits {} (rig {})", c.stat(rs::SHOTS), c.stat(rs::HITS), c.rig.shots))
                }),
                check(3.3, "the runtime drew it every frame it was in view", |c| ensure(c.stats.held_drawn >= 1, || format!("{} drawn", c.stats.held_drawn))),
            ],
            captures: vec![(0.62, "reach"), (1.2, "lifted"), (1.95, "aim")],
        },
        Scenario {
            name: "metro_hud",
            about: "the HUD in pieces: the range stands in for Metro 2033 Redux's UI layer (subtitles, a hint, the ammo counter where Redux draws them) and sends Redux's layout, as Metro's adapter does; the runtime puts the subtitles low in view and the ammo on the held gun",
            duration: 2.0,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--table", "wpn_2012", "--runtime-draws", "--metro-hud"],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, _| {
                    let mut b = Body::rest();
                    let Some(rifle) = near_rifle(objs) else { return b };
                    let (p, r) = take_rifle(t, &rifle);
                    b.hands[1].pos = keys(t, &[(1.0, p), (1.5, Vec3::new(0.2, 1.25, 0.35))]);
                    b.hands[1].rot = r;
                    if t < 1.0 {
                        b.hands[1].pos = p;
                    }
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    b
                })
            },
            checks: vec![
                check(1.8, "the game's HUD is drawn in pieces", |c| {
                    let roles: Vec<u32> = c.stats.hud_pieces[..c.stats.hud_piece_count].iter().map(|p| p.0).collect();
                    ensure(roles.contains(&HUD_SUBTITLES) && roles.contains(&HUD_AMMO), || format!("pieces {roles:?}"))
                }),
                check(1.8, "the subtitles sit low in view, a comfortable way off", |c| {
                    let (_, at) = c.stats.hud_pieces[..c.stats.hud_piece_count].iter().find(|p| p.0 == HUD_SUBTITLES).ok_or("no subtitles")?;
                    let d = *at - Body::rest().head_pos;
                    let below = (-d.y).atan2(d.z).to_degrees();
                    ensure((5.0..25.0).contains(&below) && (0.8..3.0).contains(&d.length()), || format!("{below:.1} degrees below gaze, {:.2} m away", d.length()))
                }),
                check(1.8, "the ammo counter is on the held gun", |c| {
                    ensure(gun_held_right(c), || "not held".into())?;
                    let (_, at) = c.stats.hud_pieces[..c.stats.hud_piece_count].iter().find(|p| p.0 == HUD_AMMO).ok_or("no ammo counter")?;
                    // The gun is in the right hand, held where the script puts it (tracking space).
                    let hand = Vec3::new(0.2, 1.25, 0.35);
                    ensure(at.distance(hand) < 0.35, || format!("{:.2} m from the hand holding the gun ({at:.2?})", at.distance(hand)))
                }),
            ],
            captures: vec![(1.8, "hud")],
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
            name: "game_sounds",
            about: "physical: the range hands over Metro's sound pack; the Kalash's grab, shots (with their tail and casings) and magazine are Metro's own sounds",
            duration: 4.0,
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
                    let aim = aim_gun(&rifle, Body::rest().eye(1), TARGET_AHEAD, 0.12);
                    let (p, r) = take_rifle(t, &rifle);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.6, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.6, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    // Three single shots, then A drops the magazine.
                    b.hands[1].trigger = on(t, &[(1.9, 1.96), (2.3, 2.36), (2.7, 2.76)]);
                    b.hands[1].buttons = if between(t, 3.3, 3.4) { CTRL_A } else { 0 };
                    b
                })
            },
            checks: vec![
                check(0.1, "the range's sound pack (Metro's, extracted) has loaded", |c| {
                    let a = c.audio.ok_or("no audio")?;
                    match a.pack_status() {
                        rustvr_audio::PackStatus::Ready(info) => ensure(info.name == "Metro 2033 Redux" && info.sets > 20 && info.missing.is_empty(), || format!("{info:?}")),
                        other => Err(format!("{other:?} (run metro-extract --sounds?)")),
                    }
                }),
                check(0.1, "it gives every throwable Metro's sounds: the bombs' fuse, throw and explosion, the knife's throw and hit", |c| {
                    let a = c.audio.ok_or("no audio")?;
                    let rustvr_audio::PackStatus::Ready(info) = a.pack_status() else { return Err("no pack".into()) };
                    let spec = rustvr_audio::pack::PackSpec::read(&info.dir).map_err(|e| format!("{e:#}"))?;
                    let wants: [(&str, &[&str]); 5] = [
                        ("throw_dynamite", &["grenade_pin", "grenade_arm", "grenade_throw", "explosion"]),
                        ("throw_sticky_dynamite", &["grenade_pin", "grenade_arm", "grenade_throw", "explosion"]),
                        ("throw_incendiary", &["grenade_pin", "grenade_arm", "grenade_throw", "explosion"]),
                        ("throw_knife", &["knife_throw", "knife_stick"]),
                        ("grenade", &["grenade_tick", "grenade_throw", "explosion"]),
                    ];
                    for (set, events) in wants {
                        let have = spec.resolve(Some(set)).map_err(|e| format!("{e:#}"))?;
                        for e in events {
                            ensure(have.contains_key(*e), || format!("{set} has no {e}"))?;
                        }
                    }
                    Ok(())
                }),
                check(0.95, "picking the Kalash up was the game's sound", |c| {
                    let a = c.audio.ok_or("no audio")?;
                    ensure(gun_held_right(c) && a.pack_plays(rustvr_audio::Event::Grab) >= 1, || format!("held {}, pack grabs {}", gun_held_right(c), a.pack_plays(rustvr_audio::Event::Grab)))
                }),
                check(3.2, "every shot played the Kalash's own shot, its tail and a casing landing", |c| {
                    let a = c.audio.ok_or("no audio")?;
                    let n = |e| a.pack_plays(e);
                    use rustvr_audio::Event as E;
                    ensure(c.rig.shots == 3 && n(E::Fire) == 3 && n(E::FireTail) == 3 && n(E::Casing) == 3, || {
                        format!("{} shots: fire {}, tail {}, casing {}", c.rig.shots, n(E::Fire), n(E::FireTail), n(E::Casing))
                    })
                }),
                check(3.2, "the tails are still ringing", |c| ensure(c.audio.is_some_and(|a| a.voices() >= 3), || format!("{} voices", c.audio.map_or(0, |a| a.voices())))),
                check(3.9, "the magazine came out with the AK's reload sound", |c| {
                    let a = c.audio.ok_or("no audio")?;
                    ensure(c.rig.ejects == 1 && a.pack_plays(rustvr_audio::Event::MagOut) == 1, || format!("ejects {}, pack mag_out {}", c.rig.ejects, a.pack_plays(rustvr_audio::Event::MagOut)))
                }),
            ],
            captures: vec![],
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
                check(2.95, "with Metro's sounds (when extracted): the throw is the game's", |c| {
                    use rustvr_audio::Event as E;
                    pack_check(c, |n| ensure(n(E::GrenadeThrow) == 1, || format!("pack grenade_throw {}", n(E::GrenadeThrow))))
                }),
                check(6.6, "not before its fuse", |c| ensure(c.rig.detonations == 0, || format!("{} went off", c.rig.detonations))),
                check(6.6, "with Metro's sounds: its fuse counts down with the game's beep", |c| {
                    use rustvr_audio::Event as E;
                    // Every second, then four times a second from 1.5 s left.
                    pack_check(c, |n| ensure(n(E::GrenadeTick) >= 6, || format!("pack grenade_tick {}", n(E::GrenadeTick))))
                }),
                check(7.2, "it went off down range near the target, and the range made the explosion", |c| {
                    let at = c.rig.last_detonation.ok_or("no detonation")?;
                    let near = at.distance(TARGET_AHEAD) < 5.0;
                    ensure(c.rig.detonations == 1 && at.z > 4.0 && near && c.stat_f(rs::F_EXPLOSIONS) >= 1.0 && c.stat_f(rs::F_BLAST_HITS) >= 1.0, || {
                        format!("detonations {} at {at:?}; range: explosions {}, plates hit {}", c.rig.detonations, c.stat_f(rs::F_EXPLOSIONS), c.stat_f(rs::F_BLAST_HITS))
                    })
                }),
                check(7.2, "with Metro's sounds: the explosion is the game's", |c| {
                    use rustvr_audio::Event as E;
                    pack_check(c, |n| ensure(n(E::Explosion) == 1, || format!("pack explosion {}", n(E::Explosion))))
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
                check(1.4, "with Metro's sounds (when extracted): the throw is its knife's", |c| {
                    use rustvr_audio::Event as E;
                    pack_check(c, |n| ensure(n(E::KnifeThrow) == 1, || format!("pack knife_throw {}", n(E::KnifeThrow))))
                }),
                check(2.4, "it stuck in the target, blade first", |c| {
                    let k = throwable(c, THROW_KNIFE).ok_or("no knife in flight or stuck")?;
                    let tip = c.rig.last_stick.ok_or("it never stuck")?;
                    let off = (tip - Vec3::new(-1.5, 1.4, 5.0)).truncate().length();
                    let blade = k.rot * (k.muzzle - k.grip).normalize();
                    ensure(k.flags & OS_STUCK != 0 && c.rig.sticks == 1 && off < 0.3 && blade.z > 0.5 && c.stat_f(rs::F_KNIFE_HITS) >= 1.0, || {
                        format!("flags {:#x}, sticks {}, tip {tip:?} ({off:.2} m off), blade {blade:?}, range knife hits {}", k.flags, c.rig.sticks, c.stat_f(rs::F_KNIFE_HITS))
                    })
                }),
                check(2.4, "with Metro's sounds: it went in with the game's knife hit", |c| {
                    use rustvr_audio::Event as E;
                    pack_check(c, |n| ensure(n(E::KnifeStick) == 1, || format!("pack knife_stick {}", n(E::KnifeStick))))
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
                    // The blade forward and level, its edges up and down (the back of the hand to
                    // the flat): raised, then brought down through the dummy's chest, then (after a
                    // pause) laid on it slowly.
                    let up = Vec3::new(0.2, 1.75, 0.42);
                    let down = Vec3::new(0.2, 0.95, 0.42);
                    let rest = Body::rest().hands[1].pos;
                    b.hands[1].pos = keys(t, &[(0.0, rest), (0.4, above), (0.6, above), (1.0, up), (1.2, up), (1.36, down), (1.8, down), (2.2, Vec3::new(0.2, 1.38, 0.42)), (2.8, Vec3::new(0.2, 1.22, 0.42))]);
                    b.hands[1].rot = qkeys(t, &[(0.6, Quat::IDENTITY), (1.0, look_rot(Vec3::Z))]);
                    b.hands[1].squeeze = on(t, &[(0.5, 3.2)]);
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
            name: "game_dynamite",
            about: "physical: Metro's dynamite from the throwables' table, the other hand pulls its fuse to light it, a throw down range: Metro's own sounds for the fuse lit, the throw and the explosion",
            duration: 6.6,
            needs_game: true,
            lockstep: true,
            range_args: vec![],
            physical: true,
            start: Body::rest,
            settings: |_| {},
            input: || {
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(d) = table_dynamite(objs) else { return b };
                    let rest = Body::rest().hands;
                    let above = d.at(d.grip) + Vec3::Y * 0.04;
                    let hold = Vec3::new(-0.12, 1.3, 0.38);
                    // The throw: the grenade's, from the left shoulder, letting go at 2.8 s.
                    let dir = Vec3::new(0.0, 0.5, 1.0).normalize();
                    let (release, speed, accel, steady) = (Vec3::new(-0.3, 1.62, 0.2), 6.5, 0.12, 0.05);
                    let t0 = 2.8 - (accel + steady) as f64;
                    let start = swing(t0, t0, release, dir, speed, accel, steady);
                    let follow = swing(2.9, t0, release, dir, speed, accel, steady);
                    b.hands[0].pos = if t < t0 {
                        keys(t, &[(0.0, rest[0].pos), (0.4, above), (0.55, above), (1.0, hold), (2.2, hold), (2.55, start)])
                    } else if t < 2.9 {
                        swing(t, t0, release, dir, speed, accel, steady)
                    } else {
                        keys(t, &[(2.9, follow), (3.5, rest[0].pos)])
                    };
                    b.hands[0].squeeze = on(t, &[(0.48, 2.8)]);
                    // The right hand takes the fuse's end where the held bomb has it and pulls it.
                    let fuse = live(see, d.id).filter(|o| o.flags & OS_HELD_LEFT != 0).map(|o| o.at(o.handle)).unwrap_or(hold);
                    b.hands[1].pos = keys(t, &[(1.15, rest[1].pos), (1.5, fuse), (1.6, fuse), (1.8, fuse + Vec3::X * 0.1), (2.1, rest[1].pos)]);
                    b.hands[1].squeeze = on(t, &[(1.55, 1.95)]);
                    b.head_rot = qkeys(t, &[(0.0, look_rot(above - Body::rest().head_pos)), (0.9, look_rot(hold + Vec3::Z * 0.1 - Body::rest().head_pos)), (2.1, look_rot(hold + Vec3::Z * 0.1 - Body::rest().head_pos)), (2.5, Quat::IDENTITY)]);
                    b
                })
            },
            checks: vec![
                check(1.0, "the dynamite is in the left hand", |c| {
                    let held = c.objects.iter().any(|o| o.kind == OBJ_THROWABLE && o.throw_flags & THROW_PIN != 0 && o.throw_flags & THROW_LEVER == 0 && o.flags & OS_HELD_LEFT != 0);
                    ensure(held, || "not held (run metro-extract --extras?)".into())
                }),
                check(2.0, "the other hand pulled its fuse: it's lit", |c| {
                    let d = throwable(c, THROW_PIN).ok_or("no dynamite")?;
                    ensure(c.rig.pins == 1 && c.rig.arms == 1 && d.flags & OS_ARMED != 0, || format!("pins {}, arms {}, flags {:#x}", c.rig.pins, c.rig.arms, d.flags))
                }),
                check(2.0, "Metro's sounds: the fuse pulled and lit", |c| {
                    use rustvr_audio::Event as E;
                    let a = game_pack(c)?.ok_or("no sound pack (run metro-extract --sounds?)")?;
                    let n = |e| a.pack_plays(e);
                    // One sound each (the pull's pulse is in both hands).
                    ensure(n(E::GrenadePin) == 1 && n(E::GrenadeArm) == 1, || format!("pack grenade_pin {}, grenade_arm {}", n(E::GrenadePin), n(E::GrenadeArm)))
                }),
                check(2.95, "thrown down range, the fuse burning, with Metro's throw", |c| {
                    use rustvr_audio::Event as E;
                    let d = throwable(c, THROW_PIN).ok_or("no dynamite")?;
                    let a = game_pack(c)?.ok_or("no sound pack")?;
                    let lit = d.flags & (OS_ARMED | OS_THROWN) == OS_ARMED | OS_THROWN;
                    ensure(c.rig.throws >= 1 && lit && a.pack_plays(E::GrenadeThrow) == 1, || format!("throws {}, flags {:#x}, pack grenade_throw {}", c.rig.throws, d.flags, a.pack_plays(E::GrenadeThrow)))
                }),
                check(6.5, "it went off down range with Metro's explosion, and the range made it", |c| {
                    use rustvr_audio::Event as E;
                    let at = c.rig.last_detonation.ok_or("no detonation")?;
                    let a = game_pack(c)?.ok_or("no sound pack")?;
                    ensure(c.rig.detonations == 1 && at.z > 3.0 && c.stat_f(rs::F_EXPLOSIONS) >= 1.0 && a.pack_plays(E::Explosion) == 1, || {
                        format!("detonations {} at {at:?}, range explosions {}, pack explosion {}", c.rig.detonations, c.stat_f(rs::F_EXPLOSIONS), a.pack_plays(E::Explosion))
                    })
                }),
            ],
            captures: vec![(1.7, "dynamite_lit"), (5.9, "dynamite_explosion")],
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
            name: "break_reload",
            about: "physical: Metro's Duplet (a break action): both barrels fired, the lever lets them fall open and the cases fly out, two shells from the pouch one into each chamber, a flick up snaps it shut, both barrels again",
            duration: 11.0,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--table", "wpn_duplet"],
            physical: true,
            start: Body::rest,
            // Hands only: the arms would hide the breech in the captures.
            settings: |s| s.body.visibility = crate::settings::BodyVisibility::Hands,
            input: || {
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(gun) = table_gun(objs) else { return b };
                    let eye = Body::rest().eye(1);
                    let aim = aim_gun(&gun, eye, TARGET_AHEAD, 0.15);
                    let aim_rot = gun_rot_in(&gun, aim.1);
                    // Loading: the gun low in front, the muzzle 20 degrees down and turned a little
                    // left so the open breech faces you.
                    let load_rot = (Quat::from_rotation_y(-0.35) * pitched(aim_rot, 20.0)).normalize();
                    let load = (Vec3::new(0.1, 1.28, 0.42), hand_rot_for(&gun, load_rot));
                    // The flick that shuts it: the muzzle snapped up 35 degrees and back.
                    let flick = hand_rot_for(&gun, pitched(aim_rot, -35.0));
                    let (p, r) = take_rifle(t, &gun);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.6, aim.0), (3.6, aim.0), (4.2, load.0), (8.6, load.0), (9.0, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.6, aim.1), (3.6, aim.1), (4.2, load.1), (8.6, load.1), (9.0, aim.1), (9.2, aim.1), (9.245, flick), (9.35, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    // Two barrels, then a click; later both again.
                    b.hands[1].trigger = on(t, &[(1.9, 1.96), (2.4, 2.46), (2.9, 2.96), (9.8, 9.86), (10.3, 10.36)]);
                    b.hands[1].buttons = if between(t, 3.3, 3.4) { CTRL_B } else { 0 };
                    // The left hand: a shell from the pouch over the shoulder into each chamber,
                    // base first along the bore.
                    let now = live(see, gun.id).unwrap_or(gun);
                    let rest = Body::rest().hands[0];
                    let pouch = Vec3::new(-0.27, 1.56, -0.05);
                    let into = |k: usize, depth: f32| {
                        let (mouth, bore) = chamber_at(&now, k);
                        round_in_hand(mouth + bore * depth, bore)
                    };
                    let (above0, in0, above1, in1) = (into(0, -0.12), into(0, 0.02), into(1, -0.12), into(1, 0.02));
                    b.hands[0].pos = keys(
                        t,
                        &[(4.2, rest.pos), (4.7, pouch), (4.9, pouch), (5.6, above0.0), (5.75, above0.0), (6.1, in0.0), (6.25, in0.0), (6.7, pouch), (6.9, pouch), (7.6, above1.0), (7.75, above1.0), (8.1, in1.0), (8.25, in1.0), (8.7, rest.pos)],
                    );
                    b.hands[0].rot = match () {
                        _ if t < 4.9 => Quat::IDENTITY,
                        _ if t < 5.6 => Quat::IDENTITY.slerp(above0.1, ramp(t, 4.9, 5.6)),
                        _ if t < 6.25 => above0.1,
                        _ if t < 6.9 => above0.1.slerp(Quat::IDENTITY, ramp(t, 6.25, 6.7)),
                        _ if t < 7.6 => Quat::IDENTITY.slerp(above1.1, ramp(t, 6.9, 7.6)),
                        _ if t < 8.25 => above1.1,
                        _ => above1.1.slerp(Quat::IDENTITY, ramp(t, 8.25, 8.7)),
                    };
                    b.hands[0].squeeze = on(t, &[(4.8, 6.3), (6.8, 8.3)]);
                    // Watch the breech while loading.
                    let breech = chamber_at(&now, 0).0;
                    let look = look_rot(breech - Body::rest().head_pos);
                    b.head_rot = qkeys(t, &[(0.0, Quat::from_rotation_x(0.5)), (1.2, Quat::IDENTITY), (3.6, Quat::IDENTITY), (4.2, look), (8.6, look), (9.0, Quat::IDENTITY)]);
                    b
                })
            },
            checks: vec![
                check(1.0, "the Duplet is in the right hand", |c| {
                    let g = held_gun(c).ok_or("no gun in hand (run metro-extract?)")?;
                    ensure(g.action == ACTION_BREAK && g.capacity == 2, || format!("action {} capacity {}", g.action, g.capacity))
                }),
                check(3.2, "one barrel a pull, two shots, then a click", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.shots == 2 && c.rig.dry_fires >= 1 && g.chambered == 0b11 << CHAMBER_SPENT_SHIFT, || format!("shots {} dry {} chambers {:#x}", c.rig.shots, c.rig.dry_fires, g.chambered))
                }),
                check(4.0, "the lever let the barrels fall open and threw both cases out", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(g.flags & OS_ACTION_OPEN != 0 && g.slide > 0.7 && c.rig.opens == 1 && c.rig.case_ejects == 2 && g.chambered == 0, || {
                        format!("open {} slide {:.2} opens {} ejected {} chambers {:#x}", g.flags & OS_ACTION_OPEN != 0, g.slide, c.rig.opens, c.rig.case_ejects, g.chambered)
                    })
                }),
                check(6.3, "the first shell went into the right barrel", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.loads == 1 && g.chambered == 0b01, || format!("loads {} chambers {:#x}", c.rig.loads, g.chambered))
                }),
                check(8.4, "and the second into the left", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.loads == 2 && g.chambered == 0b11 && c.rig.mags_spawned == 2, || format!("loads {} chambers {:#x} from the pouch {}", c.rig.loads, g.chambered, c.rig.mags_spawned))
                }),
                check(9.7, "a flick up snapped it shut", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.closes == 1 && g.flags & OS_ACTION_OPEN == 0 && g.slide == 0.0, || format!("closes {} open {} slide {:.2}", c.rig.closes, g.flags & OS_ACTION_OPEN != 0, g.slide))
                }),
                // (The second barrel of a pair goes off with the first's kick still in the gun.)
                check(10.1, "the first barrel after the reload hits", |c| {
                    ensure(c.rig.shots == 3 && c.stat(rs::HITS) >= 2, || format!("shots {} hits {}", c.rig.shots, c.stat(rs::HITS)))
                }),
                check(10.9, "both barrels again", |c| ensure(c.rig.shots == 4 && c.stat(rs::SHOTS) == 4, || format!("shots {} (range {})", c.rig.shots, c.stat(rs::SHOTS)))),
            ],
            captures: vec![(1.92, "break_fire"), (3.9, "break_open"), (5.7, "break_shell"), (6.2, "break_shell_in"), (8.4, "break_loaded"), (9.6, "break_shut"), (9.83, "break_fire_again")],
        },
        Scenario {
            name: "bolt_cycle",
            about: "physical: Metro's Ventil (a bolt action): a shot, the trigger again does nothing, the left hand lifts the bolt, pulls it back (the case flies out), pushes it home and turns it down, and it fires again",
            duration: 6.6,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--table", "wpn_ventil"],
            physical: true,
            start: Body::rest,
            settings: |s| s.body.visibility = crate::settings::BodyVisibility::Hands,
            input: || {
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(gun) = table_gun(objs) else { return b };
                    let aim = aim_gun(&gun, Body::rest().eye(1), TARGET_AHEAD, 0.15);
                    let (p, r) = take_rifle(t, &gun);
                    // Lowered from the eye a little to work the bolt, then back up to aim.
                    let low = aim.0 + Vec3::new(0.0, -0.14, 0.04);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.6, aim.0), (2.5, aim.0), (2.75, low), (4.7, low), (5.0, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.6, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    b.hands[1].trigger = on(t, &[(1.9, 1.96), (2.4, 2.46), (5.2, 5.26), (5.8, 5.86)]);
                    // The left hand on the knob (2 cm to its right): up, back, home, down; the knob
                    // follows the hand round the bore and along it.
                    let now = live(see, gun.id).unwrap_or(gun);
                    let off = now.rot * Vec3::new(0.02, 0.0, 0.0);
                    let (lift, back) = match () {
                        _ if t < 3.1 => (0.0, 0.0),
                        _ if t < 3.5 => (ramp(t, 3.1, 3.4), 0.0),
                        _ if t < 3.9 => (1.0, ramp(t, 3.5, 3.8)),
                        _ if t < 4.3 => (1.0, 1.0 - ramp(t, 3.9, 4.2)),
                        _ => (1.0 - ramp(t, 4.3, 4.5), 0.0),
                    };
                    let rest = Body::rest().hands[0].pos;
                    let knob = knob_at(&now, lift, back) + off;
                    b.hands[0].pos = match () {
                        _ if t < 2.6 => rest,
                        _ if t < 3.0 => rest.lerp(knob, ramp(t, 2.6, 3.0)),
                        _ if t < 4.6 => knob,
                        _ => knob.lerp(rest, ramp(t, 4.6, 5.0)),
                    };
                    b.hands[0].squeeze = on(t, &[(3.0, 4.6)]);
                    // Watch the bolt.
                    let look = look_rot(knob_at(&now, 0.0, 0.0) - Body::rest().head_pos);
                    b.head_rot = qkeys(t, &[(0.0, Quat::from_rotation_x(0.5)), (1.2, Quat::IDENTITY), (2.5, Quat::IDENTITY), (2.8, look), (4.7, look), (5.0, Quat::IDENTITY)]);
                    b
                })
            },
            checks: vec![
                check(1.0, "the Ventil is in the right hand", |c| {
                    let g = held_gun(c).ok_or("no gun in hand (run metro-extract?)")?;
                    ensure(g.action == ACTION_BOLT && g.action_angle > 1.0, || format!("action {} lift {:.2}", g.action, g.action_angle))
                }),
                check(2.2, "the first shot", |c| ensure(c.rig.shots == 1 && c.stat(rs::SHOTS) == 1, || format!("shots {}", c.rig.shots))),
                check(2.8, "no second shot without the bolt: the case is still in the chamber", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.shots == 1 && g.chambered == 1 << CHAMBER_SPENT_SHIFT, || format!("shots {} chambers {:#x}", c.rig.shots, g.chambered))
                }),
                check(3.45, "the bolt turned up", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(g.fuse > 0.85 && g.flags & OS_ACTION_OPEN != 0 && g.slide < 0.05, || format!("lift {:.2} back {:.2}", g.fuse, g.slide))
                }),
                check(3.85, "pulled back, the case flew out", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(g.slide > 0.9 && c.rig.case_ejects == 1 && g.chambered == 0, || format!("back {:.2} ejected {} chambers {:#x}", g.slide, c.rig.case_ejects, g.chambered))
                }),
                check(4.7, "home and turned down: the next round is in", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    // (It arrived with ten in the magazine and one chambered: that one fired, the next in.)
                    ensure(c.rig.cycles == 1 && g.flags & OS_ACTION_OPEN == 0 && g.chambered == 1 && g.rounds == 9, || {
                        format!("cycles {} open {} chambers {:#x} rounds {}", c.rig.cycles, g.flags & OS_ACTION_OPEN != 0, g.chambered, g.rounds)
                    })
                }),
                check(5.5, "it fires again", |c| ensure(c.rig.shots == 2 && c.stat(rs::HITS) >= 1, || format!("shots {} hits {}", c.rig.shots, c.stat(rs::HITS)))),
                check(6.5, "and only once", |c| ensure(c.rig.shots == 2, || format!("shots {}", c.rig.shots))),
            ],
            captures: vec![(1.92, "bolt_fire"), (2.8, "bolt_closed"), (3.45, "bolt_up"), (3.85, "bolt_back"), (4.55, "bolt_home")],
        },
        Scenario {
            name: "pump",
            about: "physical: Metro's Tihar (an air rifle): without pressure the trigger only hisses; four strokes of its pump lever fill the gauge, and it fires, each shot spending some",
            duration: 6.6,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--table", "wpn_tihar"],
            physical: true,
            start: Body::rest,
            settings: |s| s.body.visibility = crate::settings::BodyVisibility::Hands,
            input: || {
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(gun) = table_gun(objs) else { return b };
                    let aim = aim_gun(&gun, Body::rest().eye(1), TARGET_AHEAD, 0.15);
                    let (p, r) = take_rifle(t, &gun);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.6, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.6, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    b.hands[1].trigger = on(t, &[(1.9, 1.96), (5.5, 5.56), (5.9, 5.96)]);
                    // The left hand on the pump's handle, four strokes back along the barrel.
                    let now = live(see, gun.id).unwrap_or(gun);
                    let handle = now.at(gun.handle) - Vec3::Y * 0.02;
                    let stroke = now.rot * Vec3::Z * -(gun.handle_travel + 0.01);
                    let rest = Body::rest().hands[0].pos;
                    let pumped = (0..4).map(|k| 2.8 + 0.5 * k as f64).map(|t0| ramp(t, t0, t0 + 0.22) - ramp(t, t0 + 0.25, t0 + 0.47)).sum::<f32>();
                    b.hands[0].pos = match () {
                        _ if t < 2.2 => rest,
                        _ if t < 2.7 => rest.lerp(handle, ramp(t, 2.2, 2.7)),
                        _ if t < 4.9 => handle + stroke * pumped,
                        _ => handle.lerp(rest, ramp(t, 4.9, 5.3)),
                    };
                    b.hands[0].squeeze = on(t, &[(2.7, 4.85)]);
                    // Watch the lever, then the gauge (by the rear sight).
                    let lever = look_rot(handle - Body::rest().head_pos);
                    b.head_rot = qkeys(t, &[(0.0, Quat::from_rotation_x(0.5)), (1.2, Quat::IDENTITY), (2.2, Quat::IDENTITY), (2.6, lever), (4.9, lever), (5.2, Quat::IDENTITY)]);
                    b
                })
            },
            checks: vec![
                check(1.0, "the Tihar is in the right hand", |c| {
                    let g = held_gun(c).ok_or("no gun in hand (run metro-extract?)")?;
                    ensure(g.weapon_flags & WPN_PNEUMATIC != 0 && g.fuse == 0.0, || format!("flags {:#x} pressure {:.2}", g.weapon_flags, g.fuse))
                }),
                check(2.2, "no pressure: the trigger only hissed", |c| ensure(c.rig.shots == 0 && c.rig.hisses >= 1 && c.stat(rs::SHOTS) == 0, || format!("shots {} hisses {}", c.rig.shots, c.rig.hisses))),
                check(3.05, "the lever down: half a stroke", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(g.slide > 0.9 && c.rig.pumps == 0, || format!("pump {:.2} strokes {}", g.slide, c.rig.pumps))
                }),
                check(4.95, "four whole strokes: full", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.pumps == 4 && (g.fuse - 1.0).abs() < 1e-3, || format!("strokes {} pressure {:.3}", c.rig.pumps, g.fuse))
                }),
                check(6.4, "it fires, each shot spending some", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.shots == 2 && c.stat(rs::HITS) >= 1 && (g.fuse - 0.84).abs() < 1e-3 && (c.rig.last_strength - 0.92).abs() < 1e-3, || {
                        format!("shots {} hits {} pressure {:.3} last strength {:.3}", c.rig.shots, c.stat(rs::HITS), g.fuse, c.rig.last_strength)
                    })
                }),
            ],
            captures: vec![(2.75, "pump_home"), (3.05, "pump_back"), (4.95, "pumped"), (5.52, "air_shot")],
        },
        Scenario {
            name: "cylinder_reload",
            about: "physical: Metro's revolver: six shots and a click, B swings the cylinder out, A drops the cases, a speed loader from the pouch goes into the swung-out cylinder, a roll of the wrist swings it home, and it fires",
            duration: 8.2,
            needs_game: true,
            lockstep: true,
            range_args: vec!["--table", "wpn_revolver"],
            physical: true,
            start: Body::rest,
            settings: |s| s.body.visibility = crate::settings::BodyVisibility::Hands,
            input: || {
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(gun) = table_gun(objs) else { return b };
                    let aim = aim_gun(&gun, Body::rest().eye(1), TARGET_AHEAD, 0.4);
                    let aim_rot = gun_rot_in(&gun, aim.1);
                    // Loading: low in front, the muzzle 35 degrees down, the swung-out cylinder's
                    // back towards you.
                    let load = (Vec3::new(0.15, 1.25, 0.4), hand_rot_for(&gun, pitched(aim_rot, 35.0)));
                    // The roll that swings it home: the top to the right, 35 degrees, and back.
                    let roll = hand_rot_for(&gun, (aim_rot * Quat::from_rotation_z(-35f32.to_radians())).normalize());
                    let (p, r) = take_rifle(t, &gun);
                    b.hands[1].pos = if t < 1.0 { p } else { keys(t, &[(1.0, p), (1.6, aim.0), (4.6, aim.0), (5.0, load.0), (6.8, load.0), (7.1, aim.0)]) };
                    b.hands[1].rot = if t < 1.0 { r } else { qkeys(t, &[(1.0, r), (1.6, aim.1), (4.6, aim.1), (5.0, load.1), (6.8, load.1), (7.1, aim.1), (7.2, aim.1), (7.245, roll), (7.35, aim.1)]) };
                    b.hands[1].squeeze = if t > 0.65 { 1.0 } else { 0.0 };
                    b.hands[1].trigger = on(t, &[(1.8, 1.86), (2.1, 2.16), (2.4, 2.46), (2.7, 2.76), (3.0, 3.06), (3.3, 3.36), (3.6, 3.66), (7.7, 7.76)]);
                    b.hands[1].buttons = if between(t, 3.9, 4.0) { CTRL_B } else if between(t, 4.4, 4.5) { CTRL_A } else { 0 };
                    // The left hand: a speed loader from the pouch into the swung-out cylinder, from
                    // behind along its axis, upright in the cylinder's frame.
                    let now = live(see, gun.id).unwrap_or(gun);
                    let (well, frame) = cylinder_well(&now);
                    let rot = (frame * crate::rig::held_mag_rot().inverse()).normalize();
                    let at = |back: f32| well - rot * crate::rig::HELD_MAG_TOP - frame * Vec3::Z * back;
                    let rest = Body::rest().hands[0];
                    let pouch = Vec3::new(-0.27, 1.56, -0.05);
                    b.hands[0].pos = keys(t, &[(4.6, rest.pos), (5.1, pouch), (5.3, pouch), (6.0, at(0.15)), (6.1, at(0.15)), (6.45, at(-0.01)), (6.6, at(-0.01)), (7.0, rest.pos)]);
                    b.hands[0].rot = match () {
                        _ if t < 5.3 => Quat::IDENTITY,
                        _ if t < 6.0 => Quat::IDENTITY.slerp(rot, ramp(t, 5.3, 6.0)),
                        _ if t < 6.6 => rot,
                        _ => rot.slerp(Quat::IDENTITY, ramp(t, 6.6, 7.0)),
                    };
                    b.hands[0].squeeze = on(t, &[(5.2, 6.6)]);
                    let look = look_rot(well - Body::rest().head_pos);
                    b.head_rot = qkeys(t, &[(0.0, Quat::from_rotation_x(0.5)), (1.2, Quat::IDENTITY), (3.8, Quat::IDENTITY), (4.1, look), (6.8, look), (7.1, Quat::IDENTITY)]);
                    b
                })
            },
            checks: vec![
                check(1.0, "the revolver is in the right hand", |c| {
                    let g = held_gun(c).ok_or("no gun in hand (run metro-extract?)")?;
                    ensure(g.action == ACTION_SWING && g.rounds == 5 && g.chambered == 1, || format!("action {} rounds {} chambered {:#x}", g.action, g.rounds, g.chambered))
                }),
                check(3.8, "six shots, then a click", |c| ensure(c.rig.shots == 6 && c.rig.dry_fires >= 1 && c.stat(rs::HITS) >= 4, || format!("shots {} dry {} hits {}", c.rig.shots, c.rig.dry_fires, c.stat(rs::HITS)))),
                check(4.35, "B swung the cylinder out, the loader with it", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(g.flags & OS_ACTION_OPEN != 0 && g.slide > 0.9 && c.rig.opens == 1, || format!("open {} slide {:.2}", g.flags & OS_ACTION_OPEN != 0, g.slide))
                }),
                check(4.7, "A dropped the cases out", |c| ensure(c.rig.ejects == 1, || format!("ejects {}", c.rig.ejects))),
                check(6.7, "a fresh speed loader went into the swung-out cylinder", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.seats == 1 && g.rounds == 5 && g.chambered == 1, || format!("seats {} rounds {} chambered {:#x}", c.rig.seats, g.rounds, g.chambered))
                }),
                check(7.6, "a roll of the wrist swung it home", |c| {
                    let g = held_gun(c).ok_or("no gun")?;
                    ensure(c.rig.closes == 1 && g.flags & OS_ACTION_OPEN == 0, || format!("closes {} open {}", c.rig.closes, g.flags & OS_ACTION_OPEN != 0))
                }),
                check(8.1, "it fires from the fresh loader", |c| ensure(c.rig.shots == 7, || format!("shots {}", c.rig.shots))),
            ],
            captures: vec![(1.82, "rev_fire"), (4.35, "rev_open"), (4.75, "rev_cases_out"), (6.3, "rev_loading"), (6.75, "rev_loaded"), (7.5, "rev_shut")],
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
                latched(|t, objs, see| {
                    let mut b = Body::rest();
                    let Some(bottle) = prop_near(objs, Vec3::new(-0.55, 0.97, 0.36)) else { return b };
                    b.head_rot = Quat::from_rotation_x(0.35);
                    // The fist's axis up the bottle, the palm against its side.
                    let upright = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                    let side = bottle.pos - Vec3::X * (bottle.half_extents.x + 0.03);
                    let lifted = Vec3::new(-0.08, 1.4, 0.36);
                    let rest = Body::rest().hands[0].pos;
                    b.hands[0].pos = keys(t, &[(0.0, rest), (0.4, side + Vec3::Y * 0.25 - Vec3::X * 0.1), (0.8, side), (0.95, side), (1.4, lifted), (3.0, lifted), (3.4, rest)]);
                    b.hands[0].rot = upright;
                    b.hands[0].squeeze = on(t, &[(0.85, 3.0)]);
                    // The right hand to its other side once it's up.
                    let rest = Body::rest().hands[1].pos;
                    let other = live(see, bottle.id).filter(|_| t > 1.5).map(|now| now.pos + Vec3::X * (bottle.half_extents.x + 0.03));
                    b.hands[1].pos = match other {
                        Some(p) => rest.lerp(p, ramp(t, 1.5, 2.1)),
                        None => rest,
                    };
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
