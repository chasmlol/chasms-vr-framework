//! Verification renders of the rigged guns, held exactly the way the runtime holds them, written to
//! `captures/rig/` to look at, plus numeric checks of the rig.
//!
//! For each properly rigged gun (see `content::Weapon::rigged`) the gun is put where a shooter
//! aims it: the line of sight through the eye, long guns shouldered, the pistol at arm's length.
//! Then, as the runtime does:
//!
//! * the firing hand's controller grip pose is `gun * (grip, grip_rot)`, the support hand's is
//!   `gun * (foregrip, support_rot)`;
//! * a `rustvr_ik::BodySolver` turns the head and the two grip poses into joints (each hand joint
//!   is `HandOffsets::hand_frame` of its grip);
//! * `rustvr_ik::FingerSolver` wraps the fingers (`PistolGrip` with the index on the trigger,
//!   `Foregrip` for the support hand);
//! * `rustvr_avatar::Avatar` (Metro's Miller with `hands_basic_suit`, as the runtime loads it)
//!   draws the body over the range's own render of the gun, hidden behind it by its depth.
//!
//! Views per gun: `side` (from the right), `left`, `top`, `eye` (the shooter's view down the
//! sights), `grip` (the firing hand close up from the right), `trigger` (from the left, below:
//! the fingers round the grip and the index on the trigger), `support` (the support hand from
//! below) and `markers` (the side view with the rig points drawn: red grip, yellow trigger,
//! green line of sight, blue foregrip line, magenta magazine well, cyan charging handle).

#![allow(dead_code, clippy::too_many_arguments)]

#[path = "../src/content.rs"]
mod content;
#[path = "../src/render.rs"]
mod render;
#[path = "../src/physical.rs"]
mod physical;

use std::path::{Path, PathBuf};

use content::{MeshData, Weapon};
use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use render::{Draw, Mesh, Renderer};
use rustvr_adapter::math::{projection, view_matrix};
use rustvr_avatar::{Avatar, AvatarFrame, AvatarSource, DepthInput};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_ik::{BodyParams, BodySolver, Calibration, FingerInput, FingerSolver, HoldShape, Pose as IkPose, Side, TrackedInput};
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::*;

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("captures").join("rig")
}

fn metro_dir() -> PathBuf {
    rustvr_asset::default_root().join("metro2033")
}

fn has_metro() -> bool {
    metro_dir().join("models").join("body_miller.rvm").exists()
}

/// Every rigged gun the range has.
fn rigged() -> Vec<Weapon> {
    let mut w = content::metro_weapons(&rustvr_asset::default_root());
    w.extend([content::builtin_rifle("Range rifle", false), content::builtin_rifle("Range rifle (scoped)", true), content::builtin_pistol()]);
    w.into_iter().filter(|w| w.rigged()).collect()
}

/// The eye of a 1.70 m player standing at the origin, aiming.
const EYE: Vec3 = Vec3::new(0.0, 1.64, 0.0);

/// The gun's pose in tracking space: barrel level along +Z, the line of sight through the eye;
/// long guns shouldered (the butt 12 cm behind the eye), the pistol at arm's length.
fn aimed(w: &Weapon) -> Mat4 {
    let sight = Vec3::from_array(w.rig.sight);
    let relief = if w.pistol {
        0.50
    } else {
        let butt = w.parts.iter().filter_map(|p| p.bounds(w.fix)).map(|b| b.0.z).fold(f32::MAX, f32::min);
        (sight.z - butt - 0.12).max(0.08)
    };
    Mat4::from_translation(EYE + Vec3::Z * relief - sight)
}

fn pose(m: Mat4, p: Vec3, q: Quat) -> IkPose {
    let (_, r, _) = m.to_scale_rotation_translation();
    IkPose { pos: m.transform_point3(p), rot: (r * q).normalize() }
}

fn proto(p: IkPose) -> Pose {
    Pose { pos: p.pos.to_array(), valid: 1, rot: p.rot.to_array() }
}

/// The controller grip poses the runtime gives the hands holding `w` at `gun` ([left, right]).
fn grips(w: &Weapon, gun: Mat4) -> [Option<IkPose>; 2] {
    let right = pose(gun, Vec3::from_array(w.rig.grip), w.grip_rot.unwrap());
    let left = w.support_rot.filter(|_| !w.pistol).map(|q| pose(gun, Vec3::from_array(w.rig.foregrip), q));
    [left, Some(right)]
}

struct Held {
    joints: [Pose; JOINT_COUNT],
    hands: [HandState; 2],
    fingers: [Fingers; 2],
    targets: [IkPose; 2],
}

/// What the runtime draws for a player holding `w` at `gun`.
fn hold(w: &Weapon, gun: Mat4) -> Held {
    hold_with(w, gun, true)
}

/// [`hold`], with the support hand on the foregrip or (`support` false) the gun in one hand.
fn hold_with(w: &Weapon, gun: Mat4, support: bool) -> Held {
    let mut grips = grips(w, gun);
    if !support {
        grips[0] = None;
    }
    let mut solver = BodySolver::new(BodyParams::default(), Calibration::default());
    let head = IkPose { pos: EYE, rot: Quat::IDENTITY };
    let mut out = None;
    for _ in 0..120 {
        out = Some(solver.solve(&TrackedInput { dt: 1.0 / 90.0, head, grips, locomotion: Vec3::ZERO, seated: false, elbows: [None; 2], shoulders: [None; 2], swim: None }));
    }
    let out = out.unwrap();
    let mut fs = [FingerSolver::new(), FingerSolver::new()];
    let firing = FingerInput { trigger: 0.0, squeeze: 1.0, touch_trigger: true, touch_thumb: false, thumb_press: false, tracked: None };
    let support = FingerInput { trigger: 0.0, squeeze: 1.0, ..Default::default() };
    let mut fingers = [Fingers::default(); 2];
    for _ in 0..60 {
        fingers[1] = fs[1].update(1.0 / 90.0, &firing, HoldShape::PistolGrip);
        fingers[0] = fs[0].update(1.0 / 90.0, &support, if grips[0].is_some() { HoldShape::Foregrip } else { HoldShape::None });
    }
    let hand = |i: usize| HandState {
        pose: proto(out.hand_targets[i]),
        flags: HAND_TRACKED | if grips[i].is_some() { HAND_HOLDING } else { 0 },
        holding: if grips[i].is_some() { 1 } else { 0 },
        _pad: [0; 2],
    };
    Held { joints: out.to_protocol(), hands: [hand(0), hand(1)], fingers, targets: out.hand_targets }
}

fn daylight() -> LightHint {
    let d = Vec3::new(-0.45, 0.8, 0.5).normalize();
    LightHint { sun_dir: d.to_array(), sun_color: [1.0, 0.94, 0.86], ambient: [0.2, 0.22, 0.26], ..Default::default() }
}

/// A camera at `from` looking at `to`, upright (`up` breaks the tie looking straight down).
fn look_at(from: Vec3, to: Vec3, up: Vec3) -> (Vec3, Quat) {
    let z = (to - from).normalize();
    let x = up.cross(z).normalize();
    let y = z.cross(x);
    (from, Quat::from_mat3(&Mat3::from_cols(x, y, z)))
}

fn fov(tan: f32, aspect: f32) -> Fov {
    Fov { left: -tan * aspect, right: tan * aspect, up: tan, down: -tan }
}

struct Scene<'a> {
    gpu: &'a Gpu,
    renderer: Renderer,
    avatar: Avatar,
    cube: Mesh,
}

struct View {
    name: &'static str,
    cam: (Vec3, Quat),
    fov: Fov,
    visibility: u32,
    first_person: bool,
    markers: bool,
    /// Use the runtime's depth bias (what the player sees) rather than exact occlusion.
    runtime_bias: bool,
    /// Draw the gun (off: only the markers and a block where the pistol grip is, to see the fist
    /// round it).
    gun: bool,
    /// Tint the gun's moving parts: the magazine red, the bolt or handle cyan, the trigger yellow,
    /// the barrels (or cylinder) on their hinge orange, the chambers' rounds green, a gauge blue.
    parts: bool,
    /// Where the moving parts are (the runtime's `slide`, `chambered`, `fuse`): at rest, or a
    /// break action open, a bolt worked back, a pump lever down, a gauge full.
    state: Option<content::PartsState>,
}

const SIZE: (u32, u32) = (900, 700);

impl Scene<'_> {
    /// Renders the gun (and markers), then the body over it, and returns the RGBA8 pixels.
    fn shot(&mut self, gun: &[(Mesh, bool)], gun_at: Mat4, w: &Weapon, held: &Held, view: &View) -> Vec<u8> {
        let gpu = self.gpu;
        let (width, height) = SIZE;
        let color = Texture::new(gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
        let depth = Texture::new(gpu, &TexDesc::target(width, height, DXGI_FORMAT_R32_FLOAT)).unwrap();
        let (cam, rot) = view.cam;
        let vp = projection(&view.fov, 0.02, 50.0) * view_matrix(cam, rot);
        // Each part where the runtime's state puts it (the seated magazine where the model has it).
        let state = view.state.unwrap_or_else(|| content::PartsState::at_rest(w));
        let mats = w.part_matrices(&state);
        let a = &w.action;
        let chamber = |i: usize| a.chamber_parts.iter().chain(&a.round_parts).any(|c| c.contains(&i));
        let gauge = |i: usize| a.gauge.as_ref().is_some_and(|g| g.parts.contains(&i));
        let tint = |i: usize| match () {
            _ if !view.parts => [1.0; 4],
            _ if w.mag_parts.contains(&i) => [1.0, 0.25, 0.25, 1.0],
            _ if w.bolt_parts.contains(&i) => [0.25, 1.0, 1.0, 1.0],
            _ if w.trigger_parts.contains(&i) => [1.0, 1.0, 0.2, 1.0],
            _ if a.barrel_parts.contains(&i) => [1.0, 0.6, 0.15, 1.0],
            _ if chamber(i) => [0.3, 1.0, 0.3, 1.0],
            _ if gauge(i) => [0.3, 0.45, 1.0, 1.0],
            _ => [0.6, 0.6, 0.6, 1.0],
        };
        let moving = |i: usize| w.mag_parts.contains(&i) || w.bolt_parts.contains(&i) || w.trigger_parts.contains(&i) || a.barrel_parts.contains(&i) || chamber(i) || gauge(i);
        let mut draws: Vec<Draw> = if view.gun {
            gun.iter()
                .enumerate()
                .filter_map(|(i, (m, two))| {
                    // (The seated magazine, as the runtime puts it: in a swung-out cylinder it swings
                    // out with it.)
                    let seated = if a.hinged() { a.hinge_matrix(state.slide) } else { Mat4::IDENTITY } * w.fix;
                    let at = mats[i].or_else(|| w.mag_parts.contains(&i).then_some(seated))?;
                    Some(Draw { mesh: m, model: gun_at * at, tint: tint(i), emissive: if view.parts && moving(i) { 0.6 } else { 0.0 }, two_sided: *two })
                })
                .collect()
        } else {
            Vec::new()
        };
        if !view.gun {
            // The pistol grip's cross-section (measured), as a block along its axis.
            let (centre, half) = w.grip_block.unwrap();
            // The block square to the grip's own axis: the hand's tube without the hand's twist.
            let axis = w.grip_rot.unwrap() * Vec3::Z;
            let q = content::fist(axis, Vec3::X);
            let block = Mat4::from_scale_rotation_translation(half * 2.0, q, centre);
            draws.push(Draw { mesh: &self.cube, model: gun_at * block, tint: [0.25, 0.25, 0.3, 1.0], emissive: 0.0, two_sided: false });
            if let Some(t) = w.trigger {
                draws.push(Draw { mesh: &self.cube, model: gun_at * Mat4::from_scale_rotation_translation(Vec3::new(0.004, 0.025, 0.004), Quat::IDENTITY, t + Vec3::Y * 0.008), tint: [0.8, 0.7, 0.1, 1.0], emissive: 0.5, two_sided: false });
            }
        }
        if view.markers {
            for (m, tint) in markers(w) {
                draws.push(Draw { mesh: &self.cube, model: gun_at * m, tint, emissive: 1.5, two_sided: false });
            }
        }
        if view.name == "sights" {
            // Where the shot goes: a red dot 6 m out on the line of sight.
            let dot = Mat4::from_scale_rotation_translation(Vec3::splat(0.006), Quat::IDENTITY, EYE + Vec3::Z * 6.0);
            draws.push(Draw { mesh: &self.cube, model: dot, tint: [1.0, 0.1, 0.1, 1.0], emissive: 1.5, two_sided: false });
        }
        self.renderer.render(gpu, color.rtv.as_ref().unwrap(), depth.rtv.as_ref(), width, height, vp, cam, rot * Vec3::Z, &draws).unwrap();
        let px = color.read_rgba8(gpu).unwrap();
        // The runtime reads the game's picture as sRGB and draws the body over it.
        let target = Texture::new(gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)).unwrap();
        unsafe { gpu.ctx.UpdateSubresource(&target.tex, 0, None, px.as_ptr().cast(), width * 4, 0) };
        let frame = AvatarFrame {
            joints: &held.joints,
            fingers: held.fingers,
            hands: held.hands,
            ghosts: [None; 2],
            held: [None; 2],
            grasp: [None; 2],
            visibility: view.visibility,
            light: daylight(),
            first_person: view.first_person,
            plain: false,
        };
        let eye = Pose { pos: cam.to_array(), valid: 1, rot: rot.to_array() };
        // The avatar lets the body show through up to 1 cm + 1% of the distance behind the game's
        // surface (depth noise in a real game). Here the depth is exact, so unless the view asks for
        // the runtime's look, take that bias back out of the depth: the renders then show exactly
        // what's in front of what (fingers behind the grip hidden).
        let exact = if view.runtime_bias {
            None
        } else {
            let raw = depth.read_raw(gpu).unwrap();
            let texels: Vec<f32> = raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).map(|g| if g > 0.0 { ((g - 0.01) / 1.01).max(1e-4) } else { 0.0 }).collect();
            Some(Texture::upload(gpu, width, height, DXGI_FORMAT_R32_FLOAT, &[bytemuck::cast_slice(&texels)]).unwrap())
        };
        let srv = exact.as_ref().unwrap_or(&depth).srv.as_ref().unwrap();
        let d = DepthInput { srv, units_per_meter: 1.0, bloom: None, lens: None };
        self.avatar.draw(gpu, target.rtv.as_ref().unwrap(), width, height, &eye, &view.fov, &frame, Some(d)).unwrap();
        target.read_rgba8(gpu).unwrap()
    }
}

/// Unit cubes scaled and placed (weapon space) on the rig's points.
fn markers(w: &Weapon) -> Vec<(Mat4, [f32; 4])> {
    let at = |p: Vec3, s: f32| Mat4::from_scale_rotation_translation(Vec3::splat(s), Quat::IDENTITY, p);
    let span = |a: Vec3, b: Vec3, t: f32| Mat4::from_scale_rotation_translation(Vec3::new(t, t, (b - a).length()), Quat::from_rotation_arc(Vec3::Z, (b - a).normalize()), (a + b) * 0.5);
    let v = Vec3::from_array;
    let mut out = vec![(at(v(w.rig.grip), 0.008), [1.0, 0.1, 0.1, 1.0]), (at(v(w.rig.mag_well), 0.007), [1.0, 0.2, 1.0, 1.0]), (at(v(w.rig.muzzle), 0.007), [1.0, 1.0, 1.0, 1.0])];
    if let Some(t) = w.trigger {
        out.push((at(t, 0.006), [1.0, 0.9, 0.1, 1.0]));
    }
    if let Some((rear, front)) = w.sights {
        out.push((span(rear - Vec3::Z * 0.25, front + Vec3::Z * 0.05, 0.0012), [0.1, 1.0, 0.2, 1.0]));
    }
    if !w.pistol {
        let f = v(w.rig.foregrip);
        out.push((at(f, 0.006), [0.2, 0.4, 1.0, 1.0]));
    }
    if let Some((h, _)) = w.handle {
        out.push((at(h, 0.007), [0.1, 1.0, 1.0, 1.0]));
    }
    out
}

/// The line of sight (along +Z through `rig.sight`) passes through both sights.
#[test]
fn the_line_of_sight_passes_through_the_sights() {
    let guns = rigged();
    assert!(guns.len() >= 3);
    for w in &guns {
        let (rear, front) = w.sights.expect("rigged guns have their sights measured");
        let sight = Vec3::from_array(w.rig.sight);
        assert_eq!(w.rig.sight_line_dir, [0.0, 0.0, 1.0], "{}", w.key);
        for (what, p) in [("rear", rear), ("front", front)] {
            let off = Vec3::new(p.x - sight.x, p.y - sight.y, 0.0).length();
            eprintln!("{}: {what} sight {:.2} mm off the line of sight", w.key, off * 1000.0);
            assert!(off < 0.0005, "{}: the line of sight misses the {what} sight by {:.2} mm", w.key, off * 1000.0);
        }
        // Shots fly along it: from the eye's place behind the rear sight, the front sight is dead
        // ahead, and so is the point of aim 10 m out.
        let gun = aimed(w);
        let eye_to_front = gun.transform_point3(front) - EYE;
        let err = eye_to_front.normalize().angle_between(Vec3::Z).to_degrees();
        assert!(err < 0.01, "{}: the front sight is {err}° off the shot line", w.key);
        // The muzzle is near the line (a sight sits a few centimetres over the bore).
        let m = Vec3::from_array(w.rig.muzzle);
        let over = sight.y - m.y;
        // (A heavy gun's line of sight is the game's aim, which can run well off the bore.)
        if w.stock || w.pistol {
            assert!((0.0..0.12).contains(&over) && (m.x - sight.x).abs() < 0.01, "{}: muzzle {m} vs sight {sight}", w.key);
        }
    }
}

/// The Kalash 2012 (the first gun in Metro 2033) aims through the collimator the game mounts on
/// it: the line of sight is the collimator's own axis (its mount `loc_kolimator`'s +Z through its
/// origin), through the reticle's centre and the middle of the lens's window; the lens and the
/// reticle are glass the runtime draws (`Weapon::glass`), not range parts.
#[test]
fn the_kalash_2012_sights_through_its_collimator() {
    let Some(w) = rigged().into_iter().find(|w| w.key == "wpn_2012") else {
        eprintln!("no Metro 2033 assets: skipped");
        return;
    };
    let model = rustvr_asset::load_model(&metro_dir(), "wpn_2012").unwrap();
    let mount = model.bones.iter().find(|b| b.name == "loc_kolimator").expect("the collimator's mount");
    let mount = Mat4::from_cols_array(&mount.model_matrix);
    let sight = Vec3::from_array(w.rig.sight);
    // The mount's axis in weapon space is the line of sight.
    let axis = w.fix.transform_vector3(mount.z_axis.truncate()).normalize();
    eprintln!("wpn_2012: the collimator's axis is {:.4}° off the line of sight", axis.angle_between(Vec3::Z).to_degrees());
    assert!(axis.angle_between(Vec3::Z).to_degrees() < 0.01, "the line of sight is the collimator's axis ({axis})");
    let on_line = |p: Vec3| Vec2::new(p.x - sight.x, p.y - sight.y).length();
    let origin = w.fix.transform_point3(mount.w_axis.truncate());
    assert!(on_line(origin) < 1e-4, "through the mount: {:.2} mm off", on_line(origin) * 1000.0);
    // The glass: the panes and the reticle, each centred on the line.
    assert_eq!(w.glass.len(), 4, "the collimator's four lens meshes are glass");
    for g in &w.glass {
        let (lo, hi) = g.bounds(w.fix).unwrap();
        let c = (lo + hi) * 0.5;
        eprintln!("wpn_2012: glass {:?} centred {:.2} mm off the line of sight", g.texture, on_line(c) * 1000.0);
        assert!(on_line(c) < 0.001, "a lens mesh {lo}..{hi} is off the line of sight");
        assert!(c.z > sight.z - 0.01, "the lens is ahead of the sight point");
    }
    assert!(w.parts.iter().all(|p| p.source.as_ref().is_some_and(|s| s.mesh < 6)), "no glass among the parts");
}

/// The hands the runtime would draw are where the rig says: each hand joint is exactly
/// `hand_frame` of its grip pose, the palm faces the grip, and the trigger is in reach of the
/// index finger.
#[test]
fn the_drawn_hands_sit_on_the_gun() {
    let offsets = BodyParams::default().hand_offsets;
    for w in &rigged() {
        let gun = aimed(w);
        let held = hold(w, gun);
        let g = grips(w, gun);
        let right = offsets.hand_frame(Side::Right, g[1].unwrap());
        assert!(right.pos.distance(held.targets[1].pos) < 1e-4, "{}: the right hand is on its grip pose", w.key);
        if let Some(left) = g[0] {
            let l = offsets.hand_frame(Side::Left, left);
            assert!(l.pos.distance(held.targets[0].pos) < 1e-4, "{}: the left hand is on its grip pose", w.key);
        }
        // A heavy gun's handles are the game's own shapes (a spade grip, a handlebar, a side
        // handle): its hands are Metro's (`metro_arms_hold_their_guns`), not a pistol grip's.
        if !w.stock && !w.pistol {
            continue;
        }
        // The right palm (canonical +X out of it) faces the gun's left, the fingers point forward.
        let to_weapon = gun.inverse();
        let r = Quat::from_mat4(&to_weapon) * right.rot;
        let wrist = to_weapon.transform_point3(right.pos);
        assert!((r * Vec3::X).x < -0.8, "{}: right palm faces {:?}", w.key, r * Vec3::X);
        assert!((r * Vec3::Y).z > 0.6, "{}: right fingers point {:?}", w.key, r * Vec3::Y);
        let grip = Vec3::from_array(w.rig.grip);
        assert!(wrist.z < grip.z && wrist.x > grip.x - 0.01, "{}: the wrist {wrist} is behind and right of the grip", w.key);
        // Wrist to trigger pad: the hand to the knuckle plus the index finger's first two joints
        // (about 14 cm for the default body), and the trigger straight ahead of the hand.
        let reach = wrist.distance(w.trigger.unwrap());
        eprintln!("{}: wrist to trigger {:.1} cm", w.key, reach * 100.0);
        assert!((0.12..0.18).contains(&reach), "{}: trigger {:.1} cm from the wrist", w.key, reach * 100.0);
        let ahead = (w.trigger.unwrap() - wrist).normalize();
        assert!(ahead.z > 0.8, "{}: the trigger is off to the side of the hand ({ahead})", w.key);
        if let Some(left) = g[0] {
            let l = offsets.hand_frame(Side::Left, left);
            assert!(l.pos.distance(held.targets[0].pos) < 1e-4, "{}: the left hand is on its grip pose", w.key);
            let lr = Quat::from_mat4(&to_weapon) * l.rot;
            // The left hand's canonical -X is out of its palm: up, under the handguard.
            assert!((lr * -Vec3::X).y > 0.7, "{}: left palm faces {:?}", w.key, lr * -Vec3::X);
        }
    }
}

/// Metro's first-person arms for a gun hold it in their bind pose (in the gun's own space): the
/// grip poses that would put the avatar's hand joints exactly on those hands, in weapon space
/// ([left, right]), using the same canonical hand frame the avatar computes (Y from the wrist to
/// the middle finger's base, Z from the little finger's base to the index's, X = Y x Z).
fn metro_hand_grips(w: &Weapon) -> Option<[IkPose; 2]> {
    let dir = metro_dir();
    let index = rustvr_asset::load_index(&dir).ok()?;
    let hands = index.items.iter().find(|i| i.key == w.key)?.hands.clone()?;
    let m = rustvr_asset::load_model(&dir, &hands).ok()?;
    let at = |name: String| m.bones.iter().find(|b| b.name == name).map(|b| Vec3::new(b.model_matrix[12], b.model_matrix[13], b.model_matrix[14]));
    let offsets = BodyParams::default().hand_offsets;
    let (_, fix_rot, _) = w.fix.to_scale_rotation_translation();
    let grip = |side: Side, s: &str| -> Option<IkPose> {
        let wrist = at(format!("bip01_{s}_hand"))?;
        let y = (at(format!("bip01_{s}_finger2"))? - wrist).normalize();
        let across = at(format!("bip01_{s}_finger1"))? - at(format!("bip01_{s}_finger4"))?;
        let z = (across - y * across.dot(y)).normalize();
        let hand = Quat::from_mat3(&Mat3::from_cols(y.cross(z), y, z));
        let o = offsets.grip_to_hand[side as usize];
        let rot = (hand * o.rot.inverse()).normalize();
        let pos = wrist - rot * o.pos;
        Some(IkPose { pos: w.fix.transform_point3(pos), rot: (fix_rot * rot).normalize() })
    };
    Some([grip(Side::Left, "l")?, grip(Side::Right, "r")?])
}

fn describe(q: Quat) -> String {
    let (x, y, z) = (q * Vec3::X, q * Vec3::Y, q * Vec3::Z);
    format!("X {x:.3} Y {y:.3} Z {z:.3} quat {:.4?}", q.to_array())
}

#[test]
fn metro_arms_hold_their_guns() {
    for w in rigged().iter().filter(|w| w.from_game) {
        let Some([left, right]) = metro_hand_grips(w) else { continue };
        eprintln!("{} from Metro's arms:", w.key);
        eprintln!("  right grip pose at {:.4} {}", right.pos, describe(right.rot));
        eprintln!("  left  grip pose at {:.4} {}", left.pos, describe(left.rot));
        let g = w.grip_rot.unwrap();
        let grip = Vec3::from_array(w.rig.grip);
        let dr = g.angle_between(right.rot).to_degrees();
        eprintln!("  authored right {}  ({dr:.1}° from Metro's, palm {:.1} cm away)", describe(g), grip.distance(right.pos) * 100.0);
        // The hands are Metro's own: the controllers go where they put the drawn hands exactly on
        // Metro's arms for the gun.
        assert!(dr < 25.0 && grip.distance(right.pos) < 0.03, "{}: the firing hand strays from Metro's", w.key);
        // A pistol is held in the firing hand alone (Metro's other hand cups it; the runtime has no
        // cupping hold).
        if let Some(s) = w.support_rot.filter(|_| !w.pistol) {
            let fore = Vec3::from_array(w.rig.foregrip);
            let dl = s.angle_between(left.rot).to_degrees();
            eprintln!("  authored left  {}  ({dl:.1}° from Metro's, palm {:.1} cm away)", describe(s), fore.distance(left.pos) * 100.0);
            assert!(dl < 5.0 && fore.distance(left.pos) < 0.01, "{}: the support hand strays from Metro's", w.key);
        }
    }
}

#[test]
fn renders() {
    let gpu = Gpu::new().unwrap();
    eprintln!("GPU: {}", gpu.adapter_name());
    if !has_metro() {
        eprintln!("no Metro 2033 assets: the built-in guns only, with the mannequin");
    }
    std::fs::create_dir_all(out_dir()).unwrap();
    let source = if has_metro() {
        AvatarSource::Model { dir: metro_dir(), body: "body_miller".into(), hands: Some("hands_basic_suit".into()) }
    } else {
        AvatarSource::Procedural
    };
    let mut renderer = Renderer::new(&gpu, rustvr_asset::default_root()).unwrap();
    let mut cube = MeshData::default();
    cube.add_box(Vec3::splat(-0.5), Vec3::splat(0.5), [255, 255, 255, 255]);
    let cube = renderer.upload(&gpu, &cube).unwrap();
    let mut scene = Scene { gpu: &gpu, avatar: Avatar::new(&gpu, source).unwrap(), renderer, cube };
    let only = std::env::var("RIG_ONLY").ok();
    // RIG_METRO=1: hold Metro's guns the way Metro's own arms do instead (for comparison).
    let metro_hold = std::env::var("RIG_METRO").is_ok();
    let mut guns = rigged();
    if metro_hold {
        for w in guns.iter_mut() {
            if let Some([l, r]) = metro_hand_grips(w) {
                w.grip_rot = Some(r.rot);
                w.rig.grip = r.pos.to_array();
                w.support_rot = Some(l.rot);
                w.rig.foregrip = l.pos.to_array();
                w.key += "_metro";
            }
        }
    }
    for w in guns.iter().filter(|w| only.as_ref().is_none_or(|k| k.split(',').any(|k| w.key.starts_with(k)))) {
        let gun_meshes: Vec<(Mesh, bool)> = w.parts.iter().map(|p| (scene.renderer.upload(&gpu, p).unwrap(), w.from_game)).collect();
        let gun = aimed(w);
        let held = hold(w, gun);
        // The action views: the firing hand alone (the support hand would hold the shut barrels).
        let held_one = hold_with(w, gun, false);
        let at = |p: [f32; 3]| gun.transform_point3(Vec3::from_array(p));
        let grip = at(w.rig.grip);
        let tube = w.grip_rot.unwrap() * Vec3::Z;
        let fore = if w.pistol { at(w.rig.muzzle) } else { at(w.rig.foregrip) };
        let mid = (grip + fore) * 0.5 + Vec3::Y * 0.03;
        let span = (fore - grip).length().max(0.2);
        let wide = fov(0.36, SIZE.0 as f32 / SIZE.1 as f32);
        let close = fov(0.17, SIZE.0 as f32 / SIZE.1 as f32);
        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        let all_markers = std::env::var("RIG_MARKERS").is_ok();
        let view = |name: &'static str, cam: (Vec3, Quat), fov: Fov, visibility: u32| View { name, cam, fov, visibility, first_person: false, markers: all_markers, runtime_bias: false, gun: true, parts: false, state: None };
        let side = mid + Vec3::new(span * 1.9 + 0.3, 0.06, 0.0);
        let mut shots = vec![
            view("side", look_at(side, mid, Vec3::Y), wide, BODYVIS_ARMS),
            View { markers: true, ..view("markers", look_at(side, mid, Vec3::Y), wide, BODYVIS_HANDS) },
            // The parts the rig moves, tinted: magazine red, bolt or handle cyan, trigger yellow.
            View { parts: true, ..view("parts", look_at(side, mid, Vec3::Y), wide, BODYVIS_HANDS) },
            view("left", look_at(mid + Vec3::new(-(span * 1.9 + 0.3), 0.06, 0.0), mid, Vec3::Y), wide, BODYVIS_ARMS),
            view("top", look_at(mid + Vec3::new(0.0, span * 1.9 + 0.35, 0.0), mid, Vec3::Z), wide, BODYVIS_ARMS),
            view("under", look_at(mid + Vec3::new(0.0, -(span * 1.9 + 0.35), 0.0), mid, Vec3::Z), wide, BODYVIS_ARMS),
            View { first_person: true, ..view("eye", (EYE, Quat::IDENTITY), fov(0.75, aspect), BODYVIS_FULL) },
            View { first_person: true, runtime_bias: true, ..view("eye_runtime", (EYE, Quat::IDENTITY), fov(0.75, aspect), BODYVIS_FULL) },
            // Down the sights, magnified: the front post's tip should sit in the rear notch, level
            // with its shoulders, on the point the shot goes (the image's centre).
            View { first_person: true, ..view("sights", (EYE, Quat::IDENTITY), fov(0.03, aspect), BODYVIS_HANDS) },
            view("grip", look_at(grip + Vec3::new(0.42, 0.03, 0.03), grip + Vec3::new(0.0, 0.03, 0.03), Vec3::Y), close, BODYVIS_HANDS),
            view("grip_left", look_at(grip + Vec3::new(-0.42, 0.03, 0.03), grip + Vec3::new(0.0, 0.03, 0.03), Vec3::Y), close, BODYVIS_HANDS),
            view("grip_below", look_at(grip + Vec3::new(0.0, -0.42, 0.03), grip + Vec3::new(0.0, 0.0, 0.03), Vec3::Z), close, BODYVIS_HANDS),
            // Down the grip's axis, the gun replaced by a block the size of its pistol grip.
            View { gun: false, ..view("grip_section", look_at(grip + tube * 0.42, grip, Vec3::Z), close, BODYVIS_HANDS) },
            View { gun: false, ..view("grip_section_side", look_at(grip + Vec3::new(-0.42, 0.0, 0.0), grip, Vec3::Y), close, BODYVIS_HANDS) },
        ];
        if !w.pistol {
            shots.push(view("support", look_at(fore + Vec3::new(-0.3, -0.28, -0.05), fore, Vec3::Y), close, BODYVIS_HANDS));
            shots.push(view("support_right", look_at(fore + Vec3::new(0.32, -0.12, 0.08), fore, Vec3::Y), close, BODYVIS_HANDS));
        }
        // The action worked (`rustvr::rig::actions`): the parts the runtime moves, where it puts them.
        let a = &w.action;
        let rest = content::PartsState::at_rest(w);
        let parts_view = |name, cam, state: content::PartsState| View { parts: true, state: Some(state), ..view(name, cam, wide, BODYVIS_HANDS) };
        if a.hinged() {
            // Open half way and all the way, from the right (a break action) or from above and
            // behind (a swing-out barrel or cylinder goes left), loaded and with its cases thrown out.
            let from = if a.kind == ACTION_SWING { mid + Vec3::new(-0.35, span * 0.9 + 0.35, -0.6) } else { side };
            let cam = look_at(from, mid, Vec3::Y);
            shots.push(View { state: Some(content::PartsState { slide: 0.5, ..rest }), ..view("half_open", cam, wide, BODYVIS_HANDS) });
            shots.push(View { state: Some(content::PartsState { slide: 1.0, ..rest }), ..view("open", cam, wide, BODYVIS_HANDS) });
            shots.push(parts_view("open_parts", cam, content::PartsState { slide: 1.0, ..rest }));
            shots.push(View { state: Some(content::PartsState { slide: 1.0, chambered: 0, ..rest }), ..view("open_empty", cam, wide, BODYVIS_HANDS) });
            // Close up on the open breech: the chambers' mouths.
            let breech = gun.transform_point3(a.hinge_matrix(1.0).transform_point3(a.chambers.first().copied().unwrap_or(a.hinge)));
            let close_from = if a.kind == ACTION_SWING { breech + Vec3::new(-0.05, 0.3, -0.25) } else { breech + Vec3::new(0.32, 0.18, -0.12) };
            shots.push(View { state: Some(content::PartsState { slide: 1.0, ..rest }), ..view("breech", look_at(close_from, breech, Vec3::Y), close, BODYVIS_HANDS) });
        }
        if a.kind == ACTION_BOLT || a.lever.is_some() || w.handle.is_some() && matches!(a.kind, ACTION_PUMP) {
            // A bolt turned up, then back; a lever (a pump) down: from the right, behind and above,
            // the knob's whole path in view.
            let knob = w.handle.map_or(mid, |(h, t)| gun.transform_point3(h - Vec3::Z * t * 0.5));
            let cam = look_at(knob + Vec3::new(0.5, 0.3, -0.35), knob, Vec3::Y);
            if a.kind == ACTION_BOLT {
                shots.push(View { state: Some(content::PartsState { fuse: 1.0, ..rest }), ..view("bolt_up", cam, wide, BODYVIS_HANDS) });
                shots.push(View { state: Some(content::PartsState { fuse: 1.0, slide: 1.0, chambered: 0, ..rest }), ..view("bolt_back", cam, wide, BODYVIS_HANDS) });
                shots.push(View { state: Some(rest), ..view("bolt_closed", cam, wide, BODYVIS_HANDS) });
            } else if let Some((pivot, _)) = a.lever {
                let at = gun.transform_point3(pivot);
                let cam = look_at(at + Vec3::new(0.55, 0.05, 0.0), at, Vec3::Y);
                for (name, slide) in [("pump_home", 0.0), ("half_pumped", 0.5), ("pumped", 1.0)] {
                    shots.push(View { state: Some(content::PartsState { slide, ..rest }), ..view(name, cam, wide, BODYVIS_HANDS) });
                }
            }
        }
        if !a.round_parts.is_empty() {
            // A magazine that stays in it, with two of its rounds left (parts view: rounds green).
            let two = content::PartsState { rounds: 1, chambered: 1, ..rest };
            let port = gun.transform_point3(a.port.unwrap_or(mid));
            shots.push(View { state: Some(two), ..view("two_rounds", look_at(port + Vec3::new(0.25, 0.2, -0.3), port, Vec3::Y), close, BODYVIS_HANDS) });
        }
        if let Some(g) = &a.gauge {
            // The gauge empty, half and full, looking at its face (the side its parts are thinnest
            // across, the shooter's).
            let at = gun.transform_point3(g.at);
            let (lo, hi) = content::parts_bounds(&w.parts, &g.parts, w.fix).unwrap_or((g.at, g.at));
            let size = hi - lo;
            let face = if size.z <= size.x && size.z <= size.y { -Vec3::Z } else if size.x <= size.y { Vec3::X } else { Vec3::Y };
            let from = at + face * 0.1 + Vec3::Y * 0.005;
            let dial = fov(0.12, aspect);
            for (name, p) in [("gauge_empty", 0.0), ("gauge_half", 0.5), ("gauge_full", 1.0)] {
                shots.push(View { state: Some(content::PartsState { fuse: p, ..rest }), ..view(name, look_at(from, at, Vec3::Y), dial, BODYVIS_HANDS) });
            }
        }
        for v in &shots {
            let px = scene.shot(&gun_meshes, gun, w, if v.state.is_some() { &held_one } else { &held }, v);
            let path = out_dir().join(format!("{}_{}.png", w.key, v.name));
            rustvr_gpu::save_png(&path, SIZE.0, SIZE.1, &px).unwrap();
        }
        eprintln!("{}: wrote {} views", w.key, shots.len());
    }
}

/// The physical armory's near table as laid out: from above and from where the player starts.
#[test]
fn table_layout_renders() {
    let gpu = Gpu::new().unwrap();
    std::fs::create_dir_all(out_dir()).unwrap();
    let mut weapons = content::metro_weapons(&rustvr_asset::default_root());
    weapons.extend([content::builtin_rifle("Range rifle", false), content::builtin_rifle("Range rifle (scoped)", true), content::builtin_pistol()]);
    let refs: Vec<&Weapon> = weapons.iter().collect();
    let armory = physical::Armory::new(&refs);
    let mut renderer = Renderer::new(&gpu, rustvr_asset::default_root()).unwrap();
    let (_, plain) = content::room();
    let room = renderer.upload(&gpu, &plain).unwrap();
    let mut meshes = Vec::new();
    let mut placed = Vec::new();
    for d in &armory.table.descs[..armory.table.count as usize] {
        if d.kind != OBJ_WEAPON || d.start_slot != 0 {
            continue;
        }
        let w = refs[(d.id - physical::WEAPON_BASE) as usize];
        let at = Mat4::from_rotation_translation(Quat::from_array(d.spawn_pose.rot), Vec3::from_array(d.spawn_pose.pos)) * w.fix;
        for p in &w.parts {
            meshes.push((renderer.upload(&gpu, p).unwrap(), at, w.from_game));
        }
        placed.push(w.key.clone());
    }
    eprintln!("on the near table: {placed:?}");
    let (width, height) = SIZE;
    let (tmin, tmax) = content::TABLE_NEAR;
    let centre = Vec3::new((tmin.x + tmax.x) * 0.5, tmax.y, (tmin.z + tmax.z) * 0.5);
    // The armory bench behind the player: from above, and as you turn round to it.
    let (bmin, bmax) = content::TABLE_ARMORY;
    let bench = Vec3::new((bmin.x + bmax.x) * 0.5, bmax.y, (bmin.z + bmax.z) * 0.5);
    for (name, (cam, rot), tan) in [
        ("table_top", look_at(centre + Vec3::new(0.0, 1.5, 0.0), centre, Vec3::Z), 0.45),
        ("table_player", look_at(Vec3::new(0.0, 1.70, 0.0), centre + Vec3::new(0.0, 0.0, 0.05), Vec3::Y), 0.45),
        ("bench_top", look_at(bench + Vec3::new(0.0, 4.2, 0.0), bench, -Vec3::Z), 0.45),
        ("bench_player", look_at(Vec3::new(0.0, 1.70, 0.2), bench + Vec3::new(0.0, 0.0, 0.4), Vec3::Y), 0.75),
    ] {
        let color = Texture::new(&gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
        let vp = projection(&fov(tan, width as f32 / height as f32), 0.05, 50.0) * view_matrix(cam, rot);
        let mut draws = vec![Draw { mesh: &room, model: Mat4::IDENTITY, tint: [1.0; 4], emissive: 0.0, two_sided: false }];
        draws.extend(meshes.iter().map(|(m, at, two)| Draw { mesh: m, model: *at, tint: [1.0; 4], emissive: 0.0, two_sided: *two }));
        renderer.render(&gpu, color.rtv.as_ref().unwrap(), None, width, height, vp, cam, rot * Vec3::Z, &draws).unwrap();
        rustvr_gpu::save_png(&out_dir().join(format!("{name}.png")), width, height, &color.read_rgba8(&gpu).unwrap()).unwrap();
    }
}

/// (For rigging, see docs/RIGGING.md) Every Metro gun as the game holds it: its model with
/// Metro's own first-person arms for it, in the model's space, from the right, the left, above,
/// below and behind, with the rig points marked (the rig's when it has one, else the extractor's
/// guesses): red grip, yellow trigger, green line of sight, blue foregrip, magenta magazine well,
/// white muzzle, cyan charging handle. `captures/rig/survey/<gun>_<view>.png`; `RIG_ONLY` picks guns.
#[test]
fn metro_guns_as_the_game_holds_them() {
    if !has_metro() {
        return;
    }
    let gpu = Gpu::new().unwrap();
    let dir = out_dir().join("survey");
    std::fs::create_dir_all(&dir).unwrap();
    let mut renderer = Renderer::new(&gpu, rustvr_asset::default_root()).unwrap();
    let mut cube = MeshData::default();
    cube.add_box(Vec3::splat(-0.5), Vec3::splat(0.5), [255, 255, 255, 255]);
    let cube = renderer.upload(&gpu, &cube).unwrap();
    let only = std::env::var("RIG_ONLY").ok();
    let (width, height) = SIZE;
    let aspect = width as f32 / height as f32;
    for w in content::metro_weapons(&rustvr_asset::default_root()).iter().filter(|w| only.as_ref().is_none_or(|k| k.split(',').any(|k| w.key.starts_with(k)))) {
        let meshes: Vec<Mesh> = w.parts.iter().chain(&w.hands).map(|p| renderer.upload(&gpu, p).unwrap()).collect();
        let Some((lo, hi)) = content::parts_bounds(&w.parts, &(0..w.parts.len()).collect::<Vec<_>>(), Mat4::IDENTITY) else { continue };
        let centre = (lo + hi) * 0.5;
        let size = (hi - lo).length().max(0.3);
        let to_model = w.fix.inverse();
        let marks: Vec<(Mat4, [f32; 4])> = markers(w).into_iter().map(|(m, tint)| (to_model * m, tint)).collect();
        let d = size * 1.25;
        let mut views = vec![
            ("right".to_string(), look_at(centre + Vec3::X * d, centre, Vec3::Y), 0.45),
            ("left".to_string(), look_at(centre - Vec3::X * d, centre, Vec3::Y), 0.45),
            ("top".to_string(), look_at(centre + Vec3::Y * d, centre, Vec3::Z), 0.45),
            ("under".to_string(), look_at(centre - Vec3::Y * d, centre, Vec3::Z), 0.45),
            ("back".to_string(), look_at(centre - Vec3::Z * d, centre, Vec3::Y), 0.45),
        ];
        // The top of the gun along the line of sight, close up from the right in four tiles (rear
        // to front), to see its sights side-on.
        let to_model = |p: [f32; 3]| to_model.transform_point3(Vec3::from_array(p));
        let (sight, muzzle) = (to_model(w.rig.sight), to_model(w.rig.muzzle));
        let (z0, z1) = (to_model(w.rig.grip).z - 0.05, muzzle.z + 0.02);
        for k in 0..4 {
            let at = Vec3::new(sight.x, sight.y, z0 + (z1 - z0) * (k as f32 + 0.5) / 4.0);
            views.push((format!("sights_{k}"), look_at(at + Vec3::X * 0.4, at, Vec3::Y), (z1 - z0) / 8.0 / 0.4 / aspect * 1.1));
        }
        for (name, (cam, rot), tan) in views {
            let color = Texture::new(&gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
            let vp = projection(&fov(tan, aspect), 0.02, 20.0) * view_matrix(cam, rot);
            let mut draws: Vec<Draw> = meshes.iter().map(|m| Draw { mesh: m, model: Mat4::IDENTITY, tint: [1.0; 4], emissive: 0.0, two_sided: true }).collect();
            draws.extend(marks.iter().map(|(m, tint)| Draw { mesh: &cube, model: *m, tint: *tint, emissive: 1.5, two_sided: false }));
            renderer.render(&gpu, color.rtv.as_ref().unwrap(), None, width, height, vp, cam, rot * Vec3::Z, &draws).unwrap();
            rustvr_gpu::save_png(&dir.join(format!("{}_{name}.png", w.key)), width, height, &color.read_rgba8(&gpu).unwrap()).unwrap();
        }
    }
}

/// Prints every laid-out gun's descriptor rig (weapon space), for the record.
#[test]
fn print_descriptors() {
    let mut weapons = content::metro_weapons(&rustvr_asset::default_root());
    weapons.extend([content::builtin_rifle("Range rifle", false), content::builtin_rifle("Range rifle (scoped)", true), content::builtin_pistol()]);
    let refs: Vec<&Weapon> = weapons.iter().collect();
    let a = physical::Armory::new(&refs);
    let descs = &a.table.descs[..a.table.count as usize];
    for d in descs.iter().filter(|d| d.kind == OBJ_WEAPON) {
        let w = refs[(d.id - physical::WEAPON_BASE) as usize];
        let m = descs.iter().find(|m| m.flags & OBJF_TEMPLATE != 0 && m.ammo_type == d.ammo_type);
        let v = |a: [f32; 3]| format!("({:.4}, {:.4}, {:.4})", a[0], a[1], a[2]);
        let q = |a: [f32; 4]| format!("[{:.4}, {:.4}, {:.4}, {:.4}]", a[0], a[1], a[2], a[3]);
        eprintln!("{} ({}):", w.key, w.name);
        eprintln!("  grip {} grip_rot {}  (tube {:.3})", v(d.grip), q(d.grip_rot), Quat::from_array(d.grip_rot) * Vec3::Z);
        eprintln!("  barrel in the controller's grip axes {:.3}", Quat::from_array(d.grip_rot).inverse() * Vec3::Z);
        eprintln!("  foregrip {} support_rot {}", v(d.foregrip), q(d.support_rot));
        eprintln!("  sight {} muzzle {} mag_well {} handle {} travel {}", v(d.sight), v(d.muzzle), v(d.mag_well), v(d.handle), d.handle_travel);
        eprintln!("  trigger {}", v(w.trigger.unwrap().to_array()));
        eprintln!("  recoil {} deg, {} deg, {} m; mass {} kg; fire rate {}/s", d.recoil_pitch, d.recoil_yaw, d.recoil_back, d.mass, d.fire_rate);
        match m {
            Some(m) => eprintln!("  collider centre {} half {}; magazine centre {} half {} mass {} kg", v(d.center), v(d.half_extents), v(m.center), v(m.half_extents), m.mass),
            None => eprintln!("  collider centre {} half {}; no magazine ({} rounds held)", v(d.center), v(d.half_extents), d.capacity),
        }
        eprintln!("  ammo type {:#x} ({}), {} rounds", d.ammo_type, w.caliber, d.capacity);
        eprintln!("  spawn {} slot {}", v(d.spawn_pose.pos), d.start_slot);
    }
}
