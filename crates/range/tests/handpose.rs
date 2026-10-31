//! The game's own hands on the rigged guns, drawn the way the runtime draws them, written to
//! `captures/handpose/` to look at, with the contact measured on the drawn skin.
//!
//! The path is the real one: the range's armory registers its guns (`ObjectDesc`) and Metro's hand
//! poses (`HandPoseDesc`, from the arms that hold each gun in its idle); the rig holds the gun with
//! the firing hand's controller grip on `gun * (grip, grip_rot)` and the support hand's on the
//! foregrip line; `rustvr_avatar::held_hands` turns the tables, the object's state and the hands
//! into the held poses; the IK reaches each held wrist exactly (`HandOffsets::grip_for`, as the
//! runtime's body does); and the avatar (Metro's Miller with `hands_basic_suit`, as the runtime
//! loads it) draws the hands in those poses over the range's own render of the gun.
//!
//! Views per gun (`<gun>_<view>.png`; `_pulled` with the trigger fully pulled, the range's trigger
//! swung back with it; `_metro` the same view of Metro's own first-person arms model for that gun,
//! as the game draws it, for comparison; `<gun>__sheet.png` all of them at half size): `side`,
//! `under`, `body` (the full body's own hands, close, from the right), `eye` (the shooter's view
//! down the sights), `eye_down` (looking down at the gun in
//! the hands), `trigger` (the trigger hand from the right, a little below), `trigger_left` (from
//! the left, below), `trigger_below`, `trigger_front`, `grip` (from the right), `support` (the
//! support hand from below), `support_right`, and `slide_back` / `slide_front` (the support hand
//! slid to either end of its foregrip range, from below).
//!
//! Numbers (printed, and asserted): the gap between the index finger's pad (its last segment's
//! skin) and the trigger blade, released and pulled, how far the pad moves with a full pull, and
//! how deep the skin of each hand goes into the gun's surface: the fingertips (their outer two
//! segments) and any skin at all, with Metro's own arms measured the same way for comparison.

#![allow(dead_code, clippy::too_many_arguments)]

#[path = "../src/content.rs"]
mod content;
#[path = "../src/render.rs"]
mod render;
#[path = "../src/physical.rs"]
mod physical;

use std::path::{Path, PathBuf};

use bytemuck::Zeroable;
use content::Weapon;
use glam::{Mat3, Mat4, Quat, Vec3};
use render::{Draw, Mesh, Renderer};
use rustvr_adapter::math::{projection, view_matrix};
use rustvr_avatar::{Avatar, AvatarFrame, AvatarSource, DepthInput, HeldHand, held_hands};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_ik::{BodyParams, BodySolver, Calibration, FingerInput, FingerSolver, HoldShape, Pose as IkPose, Side, TrackedInput};
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::*;

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("captures").join("handpose")
}

fn metro_dir() -> PathBuf {
    rustvr_asset::default_root().join("metro2033")
}

fn has_metro() -> bool {
    metro_dir().join("models").join("hands_ak.rvm").exists() && metro_dir().join("models").join("body_miller.rvm").exists()
}

/// Every gun the range lays out, with the game's hands where it has them.
fn weapons() -> Vec<Weapon> {
    let mut w = content::metro_weapons(&rustvr_asset::default_root());
    w.extend([content::builtin_rifle("Range rifle", false), content::builtin_rifle("Range rifle (scoped)", true), content::builtin_pistol()]);
    content::share_game_hands(&mut w);
    w
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

fn quat(a: [f32; 4]) -> Quat {
    Quat::from_array(a).normalize()
}

fn ik(p: (Vec3, Quat)) -> IkPose {
    IkPose { pos: p.0, rot: p.1.normalize() }
}

fn proto(p: IkPose) -> Pose {
    Pose { pos: p.pos.to_array(), valid: 1, rot: p.rot.to_array() }
}

struct Held {
    joints: [Pose; JOINT_COUNT],
    hands: [HandState; 2],
    fingers: [Fingers; 2],
    held: [Option<HeldHand>; 2],
}

/// What the runtime draws for a player holding the gun `d` at `gun` (tracking space): the support
/// hand `slide` metres along its foregrip line, the trigger pulled `trigger`.
fn hold(armory: &physical::Armory, d: &ObjectDesc, gun: Mat4, slide: f32, trigger: f32) -> Held {
    let (_, r, _) = gun.to_scale_rotation_translation();
    let v = Vec3::from_array;
    // The rig's hands (controller grip poses): the firing hand on the grip frame, the support hand
    // on the foregrip line.
    let firing = (gun.transform_point3(v(d.grip)), (r * quat(d.grip_rot)).normalize());
    let two = d.flags & OBJF_TWO_HANDED != 0;
    let support = two.then(|| (gun.transform_point3(v(d.foregrip) + Vec3::Z * slide), (r * quat(d.support_rot)).normalize()));
    let grips = [support, Some(firing)];
    // What the rig reports: the gun held in the right hand (tracking space), both hands on it.
    let mut states: Box<ObjectStates> = bytemuck::zeroed_box();
    states.states[0] = ObjectState {
        id: d.id,
        flags: OS_SIMULATED | OS_TRACKING | OS_HELD_RIGHT,
        pose: Pose { pos: gun.w_axis.truncate().to_array(), valid: 1, rot: r.to_array() },
        desc: d.id,
        ..ObjectState::zeroed()
    };
    states.count = 1;
    let state = |on: bool| HandState { pose: Pose::default(), flags: HAND_TRACKED | if on { HAND_HOLDING } else { 0 }, holding: if on { d.id } else { 0 }, _pad: [0; 2] };
    let rig_hands = [state(two), state(true)];
    let held = held_hands(grips, &rig_hands, &states, &armory.table, &armory.poses, [0.0, trigger]);
    // The body (as the runtime's): a held hand's grip is moved so its hand joint is the held wrist.
    let offsets = BodyParams::default().hand_offsets;
    let sides = [Side::Left, Side::Right];
    let ik_grips = [0, 1].map(|i| match (&held[i], grips[i]) {
        (Some(h), _) => Some(offsets.grip_for(sides[i], ik((Vec3::from_array(h.wrist.pos), quat(h.wrist.rot))))),
        (None, g) => g.map(ik),
    });
    let mut solver = BodySolver::new(BodyParams::default(), Calibration::default());
    let head = IkPose { pos: EYE, rot: Quat::IDENTITY };
    let mut out = None;
    for _ in 0..120 {
        out = Some(solver.solve(&TrackedInput { dt: 1.0 / 90.0, head, grips: ik_grips, locomotion: Vec3::ZERO, seated: false, elbows: [None; 2], swim: None, kicks: [None; 2] }));
    }
    let out = out.unwrap();
    let mut fs = [FingerSolver::new(), FingerSolver::new()];
    let mut fingers = [Fingers::default(); 2];
    for _ in 0..60 {
        fingers[1] = fs[1].update(1.0 / 90.0, &FingerInput { trigger, squeeze: 1.0, touch_trigger: true, ..Default::default() }, HoldShape::PistolGrip);
        fingers[0] = fs[0].update(1.0 / 90.0, &FingerInput { squeeze: 1.0, ..Default::default() }, if two { HoldShape::Foregrip } else { HoldShape::None });
    }
    let hands = [0, 1].map(|i| HandState { pose: proto(out.hand_targets[i]), ..rig_hands[i] });
    Held { joints: out.to_protocol(), hands, fingers, held }
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

const SIZE: (u32, u32) = (900, 700);

fn frame<'a>(held: &'a Held, visibility: u32, first_person: bool) -> AvatarFrame<'a> {
    AvatarFrame {
        joints: &held.joints,
        fingers: held.fingers,
        hands: held.hands,
        ghosts: [None; 2],
        held: held.held,
        grasp: [None; 2],
        visibility,
        light: daylight(),
        first_person,
        plain: false,
    }
}

#[derive(Clone, Copy)]
struct View {
    name: &'static str,
    cam: (Vec3, Quat),
    fov: Fov,
    visibility: u32,
    first_person: bool,
    /// Use the runtime's depth bias (what the player sees) rather than exact occlusion.
    runtime_bias: bool,
}

/// Each of the gun's parts' model matrices at `gun` (tracking space), its trigger pulled `pull`, as
/// the range draws it.
fn part_models(w: &Weapon, gun: Mat4, pull: f32) -> Vec<Mat4> {
    (0..w.parts.len())
        .map(|i| match w.trigger_pivot {
            Some((at, angle)) if w.trigger_parts.contains(&i) => gun * Mat4::from_translation(at) * Mat4::from_rotation_x(angle * pull) * Mat4::from_translation(-at) * w.fix,
            _ => gun * w.fix,
        })
        .collect()
}

/// Renders the gun (its parts at `models`, and `extra` at the gun's own place, e.g. Metro's own
/// arms), then the avatar over it unless `held` is None; returns the RGBA8 pixels.
fn shot(gpu: &Gpu, renderer: &mut Renderer, avatar: &mut Avatar, gun: &[(Mesh, bool)], models: &[Mat4], extra: &[(Mesh, bool)], model: Mat4, held: Option<&Held>, view: &View) -> Vec<u8> {
    let (width, height) = SIZE;
    let color = Texture::new(gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
    let depth = Texture::new(gpu, &TexDesc::target(width, height, DXGI_FORMAT_R32_FLOAT)).unwrap();
    let (cam, rot) = view.cam;
    let vp = projection(&view.fov, 0.02, 50.0) * view_matrix(cam, rot);
    let mut draws: Vec<Draw> = gun.iter().zip(models).map(|((m, two), &model)| Draw { mesh: m, model, tint: [1.0; 4], emissive: 0.0, two_sided: *two }).collect();
    draws.extend(extra.iter().map(|(m, two)| Draw { mesh: m, model, tint: [1.0; 4], emissive: 0.0, two_sided: *two }));
    renderer.render(gpu, color.rtv.as_ref().unwrap(), depth.rtv.as_ref(), width, height, vp, cam, rot * Vec3::Z, &draws).unwrap();
    let px = color.read_rgba8(gpu).unwrap();
    let Some(held) = held else { return px };
    // The runtime reads the game's picture as sRGB and draws the body over it.
    let target = Texture::new(gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)).unwrap();
    unsafe { gpu.ctx.UpdateSubresource(&target.tex, 0, None, px.as_ptr().cast(), width * 4, 0) };
    let eye = Pose { pos: cam.to_array(), valid: 1, rot: rot.to_array() };
    // Exact depth unless the view asks for the runtime's look (see tests/rig.rs).
    let exact = if view.runtime_bias {
        None
    } else {
        let raw = depth.read_raw(gpu).unwrap();
        let texels: Vec<f32> = raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).map(|g| if g > 0.0 { ((g - 0.0015) / 1.002).max(1e-4) } else { 0.0 }).collect();
        Some(Texture::upload(gpu, width, height, DXGI_FORMAT_R32_FLOAT, &[bytemuck::cast_slice(&texels)]).unwrap())
    };
    let srv = exact.as_ref().unwrap_or(&depth).srv.as_ref().unwrap();
    let f = frame(held, view.visibility, view.first_person);
    avatar.draw(gpu, target.rtv.as_ref().unwrap(), width, height, &eye, &view.fov, &f, Some(DepthInput { srv, units_per_meter: 1.0, bloom: None, lens: None })).unwrap();
    target.read_rgba8(gpu).unwrap()
}

// ---- measuring ----------------------------------------------------------------------------------

use rustvr_avatar::fit::{Surface, closest_on_triangle};

/// A gun's surface in tracking space near `around` (within `reach` of its bounds).
fn surface(w: &Weapon, gun: Mat4, around: &[Vec3], reach: f32) -> Surface {
    let lo = around.iter().fold(Vec3::splat(f32::MAX), |a, &b| a.min(b)) - Vec3::splat(reach);
    let hi = around.iter().fold(Vec3::splat(f32::MIN), |a, &b| a.max(b)) + Vec3::splat(reach);
    let m = gun * w.fix;
    let tris = w.parts.iter().enumerate().filter(|(i, _)| !(w.pistol && w.mag_parts.contains(i))).flat_map(|(_, part)| {
        part.indices.as_chunks::<3>().0.iter().map(move |t| {
            let v = t.map(|i| &part.vertices[i as usize]);
            (v.map(|v| m.transform_point3(Vec3::from_array(v.pos))), v.map(|v| m.transform_vector3(Vec3::from_array(v.normal))))
        })
    });
    Surface::new(tris).near(lo, hi)
}

/// How deep each point is inside the surface (0 outside).
fn depths(points: &[Vec3], surface: &Surface) -> Vec<f32> {
    points.iter().map(|&p| surface.depth(p, 0.02)).collect()
}

fn distance_to(points: &[Vec3], tris: &[[Vec3; 3]]) -> f32 {
    points.iter().flat_map(|&p| tris.iter().map(move |t| closest_on_triangle(p, t).distance(p))).fold(f32::MAX, f32::min)
}

/// A finger's outer two segments (Biped's `finger01`, `finger02` ...; others' intermediate and
/// distal bones).
fn is_fingertip(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let b = n.as_bytes();
    let biped = n.contains("finger") && b.len() >= 2 && matches!(b[b.len() - 1], b'1' | b'2') && b[b.len() - 2].is_ascii_digit();
    biped || n.contains("intermediate") || n.contains("distal")
}

fn is_right_index_tip(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with("r_finger12") || (n.contains("right") && n.contains("index") && (n.contains("distal") || n.ends_with('3')))
}

/// A bone of hand `side`'s skin (the hand itself and its fingers).
fn on_hand(name: &str, side: usize) -> bool {
    let n = name.to_ascii_lowercase();
    let s = ["_l_", "_r_"][side];
    (n.contains(s) && (n.contains("hand") || n.contains("finger"))) || (n.starts_with(["left", "right"][side]) && ["hand", "thumb", "index", "middle", "ring", "little"].iter().any(|k| n.contains(k)))
}

struct Measured {
    /// Index pad to the trigger blade, released (mm).
    pad_gap: f32,
    /// How far the pad's centre moves with a full pull (mm), and the gap then.
    pull_travel: f32,
    pulled_gap: f32,
    /// Per hand [left, right]: deepest skin point inside the gun (mm), its bone, and how many
    /// vertices are more than 2 mm in, of how many.
    deepest: [(f32, String, usize, usize); 2],
    /// ...and the deepest point of the fingers' outer two segments (the fingertips: the palm and
    /// the knuckles sit where the game puts the hand, pressed into the grip, out of sight).
    tips: [(f32, String); 2],
}

/// Metro's own arms model for `w` (posed in the gun's model space) as skin points in tracking
/// space with their heaviest bone, like [`Avatar::skin`].
fn metro_skin(w: &Weapon, gun: Mat4) -> Vec<(Vec3, String)> {
    let Ok(m) = rustvr_asset::load_model(&metro_dir(), &format!("hands_{}", w.key.trim_start_matches("wpn_"))) else { return Vec::new() };
    let at = gun * w.fix;
    m.meshes
        .iter()
        .flat_map(|mesh| mesh.vertices.iter())
        .map(|v| {
            let k = (0..4).max_by_key(|&k| v.weights[k]).unwrap_or(0);
            (at.transform_point3(Vec3::from_array(v.pos)), m.bones[v.bones[k] as usize].name.clone())
        })
        .collect()
}

/// The contact of the hands `visibility` draws: the first-person arms (`BODYVIS_HANDS`) or the
/// full body's own hands (`BODYVIS_FULL`).
fn measure(avatar: &mut Avatar, w: &Weapon, gun: Mat4, released: &Held, pulled: &Held, visibility: u32) -> Measured {
    let skin = |held: &Held, avatar: &mut Avatar| -> Vec<(Vec3, String)> { avatar.skin(&frame(held, visibility, false)).into_iter().map(|(p, n)| (p, n.to_string())).collect() };
    let rest = skin(released, avatar);
    let pull = skin(pulled, avatar);
    measure_skin(w, gun, &rest, &pull, released.held[0].is_some())
}

fn measure_skin(w: &Weapon, gun: Mat4, rest: &[(Vec3, String)], pull: &[(Vec3, String)], support: bool) -> Measured {
    let blade: Vec<[Vec3; 3]> = w.trigger_blade.iter().map(|t| t.map(|p| gun.transform_point3(p))).collect();
    // The trigger swung back as the range draws it at a full pull.
    let swung = match w.trigger_pivot {
        Some((at, angle)) => gun * Mat4::from_translation(at) * Mat4::from_rotation_x(angle) * Mat4::from_translation(-at),
        None => gun,
    };
    let blade_pulled: Vec<[Vec3; 3]> = w.trigger_blade.iter().map(|t| t.map(|p| swung.transform_point3(p))).collect();
    let pad = |s: &[(Vec3, String)]| s.iter().filter(|(_, n)| is_right_index_tip(n)).map(|(p, _)| *p).collect::<Vec<_>>();
    let (pad_rest, pad_pull) = (pad(&rest), pad(&pull));
    assert!(!pad_rest.is_empty(), "{}: no index fingertip skin", w.key);
    let centre = |p: &[Vec3]| p.iter().copied().sum::<Vec3>() / p.len() as f32;
    let mut deepest = [(0.0, String::new(), 0, 0), (0.0, String::new(), 0, 0)];
    let mut tips = [(0.0, String::new()), (0.0, String::new())];
    for side in 0..2 {
        let pts: Vec<(Vec3, &str)> = rest.iter().filter(|(_, n)| on_hand(n, side)).map(|(p, n)| (*p, n.as_str())).collect();
        if side == 0 && !support {
            continue;
        }
        let points: Vec<Vec3> = pts.iter().map(|p| p.0).collect();
        let surf = surface(w, gun, &points, 0.02);
        let d = depths(&points, &surf);
        let (k, &max) = d.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        deepest[side] = (max * 1000.0, pts[k].1.to_string(), d.iter().filter(|&&x| x > 0.002).count(), d.len());
        if let Some((k, &max)) = d.iter().enumerate().filter(|(k, _)| is_fingertip(pts[*k].1)).max_by(|a, b| a.1.total_cmp(b.1)) {
            tips[side] = (max * 1000.0, pts[k].1.to_string());
        }
        if std::env::var_os("POSE_DEBUG").is_some() {
            let inv = (gun * w.fix).inverse();
            let mut by_bone: std::collections::BTreeMap<&str, (f32, Vec3, usize)> = Default::default();
            for (j, &x) in d.iter().enumerate().filter(|(_, x)| **x > 0.001) {
                let e = by_bone.entry(pts[j].1).or_insert((0.0, Vec3::ZERO, 0));
                e.2 += 1;
                if x > e.0 {
                    *e = (x, inv.transform_point3(pts[j].0), e.2);
                }
            }
            for (b, (x, at, n)) in by_bone {
                eprintln!("    {} {b}: {n} vertices over 1 mm, deepest {:.1} mm at model {at:.4}", w.key, x * 1000.0);
            }
        }
    }
    if std::env::var_os("POSE_DEBUG").is_some() {
        let inv = gun.inverse();
        let (mut best, mut at) = (f32::MAX, (Vec3::ZERO, Vec3::ZERO));
        for &p in &pad_rest {
            for t in &blade {
                let q = closest_on_triangle(p, t);
                if q.distance(p) < best {
                    best = q.distance(p);
                    at = (inv.transform_point3(p), inv.transform_point3(q));
                }
            }
        }
        eprintln!("    {}: nearest pad point {:.4} (weapon space), blade point {:.4}, offset {:.4}", w.key, at.0, at.1, at.0 - at.1);
    }
    Measured {
        pad_gap: distance_to(&pad_rest, &blade) * 1000.0,
        pull_travel: centre(&pad_rest).distance(centre(&pad_pull)) * 1000.0,
        pulled_gap: distance_to(&pad_pull, &blade_pulled) * 1000.0,
        deepest,
        tips,
    }
}

#[test]
fn the_game_hands_hold_the_rigged_guns() {
    if !has_metro() {
        eprintln!("no Metro 2033 assets: nothing to check (the built-in guns borrow the Kalash's hands)");
        return;
    }
    let gpu = Gpu::new().unwrap();
    std::fs::create_dir_all(out_dir()).unwrap();
    let source = AvatarSource::Model { dir: metro_dir(), body: "body_miller".into(), hands: Some("hands_basic_suit".into()) };
    let mut avatar = Avatar::new(&gpu, source).unwrap();
    let mut renderer = Renderer::new(&gpu, rustvr_asset::default_root()).unwrap();
    let weapons = weapons();
    let refs: Vec<&Weapon> = weapons.iter().collect();
    let armory = physical::Armory::new(&refs);
    let only = std::env::var("POSE_ONLY").ok();
    let renders = std::env::var("POSE_NO_RENDERS").is_err();
    let descs: Vec<ObjectDesc> = armory.table.descs[..armory.table.count as usize].iter().filter(|d| d.kind == OBJ_WEAPON).copied().collect();
    assert_eq!(descs.len(), weapons.iter().filter(|w| w.rigged()).count(), "every rigged gun");
    for d in &descs {
        let w = refs[(d.id - physical::WEAPON_BASE) as usize];
        assert!(d.grip_pose != 0, "{}: a firing-hand pose", w.key);
        assert_eq!(d.support_pose != 0, !w.pistol, "{}: a support-hand pose for the long guns", w.key);
        if only.as_ref().is_some_and(|k| !k.split(',').any(|k| w.key.starts_with(k))) {
            continue;
        }
        let gun = aimed(w);
        let released = hold(&armory, d, gun, 0.0, 0.0);
        let pulled = hold(&armory, d, gun, 0.0, 1.0);
        assert!(released.held[1].is_some() && released.held[0].is_some() == !w.pistol, "{}: the held poses", w.key);
        // The IK reached each held wrist exactly.
        for i in 0..2 {
            if let Some(h) = &released.held[i] {
                let got = Vec3::from_array(released.hands[i].pose.pos);
                assert!(got.distance(Vec3::from_array(h.wrist.pos)) < 1e-4, "{}: hand {i} at its held wrist", w.key);
            }
        }
        for (side, h) in w.game_hands.iter().enumerate() {
            if let Some(h) = h {
                let deg = h.fitted.map(|j| j.map(|a| a.round() as i32));
                eprintln!("{} hand {side}: the game's pose, fitted by (thumb .. little, knuckle first, degrees opened) {deg:?}", w.key);
            }
        }
        let m = measure(&mut avatar, w, gun, &released, &pulled, BODYVIS_HANDS);
        // The full body draws Miller's own hands in the same poses (a different rig and mesh,
        // scaled to the game's hand size): looser, since its mesh isn't the pose's own.
        let b = measure(&mut avatar, w, gun, &released, &pulled, BODYVIS_FULL);
        eprintln!(
            "{} (the full body's hands): index pad {:.1} mm from the trigger blade (pulled {:.1} mm); fingertips inside the gun at most: right {:.1} mm ({}), left {:.1} mm ({}); any skin: right {:.1} mm, left {:.1} mm",
            w.key, b.pad_gap, b.pulled_gap, b.tips[1].0, b.tips[1].1, b.tips[0].0, b.tips[0].1, b.deepest[1].0, b.deepest[0].0
        );
        // The game is the truth: our hands must sit on the gun exactly as Metro's own arms do (the
        // same trigger contact, the same fingertip and skin depths), not meet thresholds of ours.
        eprintln!(
            "{}: index pad {:.1} mm from the trigger blade (a full pull moves it {:.1} mm); fingertips inside the gun at most: right {:.1} mm ({}), left {:.1} mm ({})",
            w.key, m.pad_gap, m.pull_travel, m.tips[1].0, m.tips[1].1, m.tips[0].0, m.tips[0].1
        );
        if w.from_game {
            let metro = metro_skin(w, gun);
            let g = measure_skin(w, gun, &metro, &metro, true);
            eprintln!("{} (Metro's own arms): index pad {:.1} mm from the trigger blade; fingertips inside the gun at most: right {:.1} mm, left {:.1} mm", w.key, g.pad_gap, g.tips[1].0, g.tips[0].0);
            // (A trigger that isn't a part of its own, as on the Hellbreath, has no blade to measure.)
            if !w.trigger_blade.is_empty() {
                assert!((m.pad_gap - g.pad_gap).abs() < 1.0, "{}: our index pad is {:.1} mm from the trigger, Metro's {:.1} mm", w.key, m.pad_gap, g.pad_gap);
            }
            // (A pistol is held in one hand: Metro's other hand cups it, ours is free.)
            for side in (0..2).filter(|&s| s == 1 || !w.pistol) {
                assert!((m.tips[side].0 - g.tips[side].0).abs() < 1.0, "{}: hand {side}'s fingertips {:.1} mm in, Metro's {:.1} mm", w.key, m.tips[side].0, g.tips[side].0);
            }
        }
        // Our one animation: the trigger finger follows the trigger, a believable pull.
        assert!((5.0..16.0).contains(&m.pull_travel), "{}: a full pull moves the pad {:.1} mm", w.key, m.pull_travel);

        if !renders {
            continue;
        }
        let gun_meshes: Vec<(Mesh, bool)> = w.parts.iter().map(|p| (renderer.upload(&gpu, p).unwrap(), w.from_game)).collect();
        // Metro's own arms holding this gun, as the game draws them (its first-person arms model
        // is in the gun's model space).
        let arms: Vec<(Mesh, bool)> = if w.from_game { w.hands.iter().map(|p| (renderer.upload(&gpu, p).unwrap(), true)).collect() } else { Vec::new() };
        let at = |p: [f32; 3]| gun.transform_point3(Vec3::from_array(p));
        let grip = at(w.rig.grip);
        let trig = gun.transform_point3(w.trigger.unwrap());
        let fore = if w.pistol { at(w.rig.muzzle) } else { at(w.rig.foregrip) };
        let mid = (grip + fore) * 0.5 + Vec3::Y * 0.03;
        let span = (fore - grip).length().max(0.2);
        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        let (wide, close) = (fov(0.36, aspect), fov(0.17, aspect));
        let view = |name: &'static str, cam: (Vec3, Quat), fov: Fov, visibility: u32| View { name, cam, fov, visibility, first_person: false, runtime_bias: false };
        let mut views = vec![
            view("side", look_at(mid + Vec3::new(span * 1.9 + 0.3, 0.06, 0.0), mid, Vec3::Y), wide, BODYVIS_ARMS),
            view("under", look_at(mid + Vec3::new(0.0, -(span * 1.9 + 0.35), 0.0), mid, Vec3::Z), wide, BODYVIS_ARMS),
            // The full body (Miller's own hands, scaled to Metro's first-person ones), from the right.
            view("body", look_at(grip + Vec3::new(0.30, 0.0, 0.10), grip + Vec3::new(0.0, 0.0, 0.06), Vec3::Y), fov(0.24, aspect), BODYVIS_FULL),
            View { first_person: true, runtime_bias: true, ..view("eye", (EYE, Quat::IDENTITY), fov(0.75, aspect), BODYVIS_ARMS) },
            // Looking down at the gun in your hands.
            View { first_person: true, runtime_bias: true, ..view("eye_down", look_at(EYE, grip, Vec3::Y), fov(0.6, aspect), BODYVIS_ARMS) },
            view("trigger", look_at(trig + Vec3::new(0.34, -0.10, 0.12), trig + Vec3::new(0.0, -0.01, -0.01), Vec3::Y), close, BODYVIS_HANDS),
            view("trigger_left", look_at(trig + Vec3::new(-0.34, -0.14, 0.06), trig + Vec3::new(0.0, -0.01, -0.01), Vec3::Y), close, BODYVIS_HANDS),
            view("trigger_below", look_at(trig + Vec3::new(0.0, -0.40, 0.02), trig, Vec3::Z), close, BODYVIS_HANDS),
            view("trigger_front", look_at(trig + Vec3::new(0.08, -0.24, 0.16), trig, Vec3::Y), close, BODYVIS_HANDS),
            view("grip", look_at(grip + Vec3::new(0.42, 0.03, 0.03), grip + Vec3::new(0.0, 0.0, 0.03), Vec3::Y), close, BODYVIS_HANDS),
        ];
        if !w.pistol {
            views.push(view("support", look_at(fore + Vec3::new(-0.3, -0.28, -0.05), fore, Vec3::Y), close, BODYVIS_HANDS));
            views.push(view("support_right", look_at(fore + Vec3::new(0.32, -0.12, 0.08), fore, Vec3::Y), close, BODYVIS_HANDS));
        }
        let shots = std::cell::RefCell::new(Vec::new());
        let save = |name: String, px: &[u8]| {
            rustvr_gpu::save_png(&out_dir().join(format!("{}_{name}.png", w.key)), SIZE.0, SIZE.1, px).unwrap();
            shots.borrow_mut().push(px.to_vec());
        };
        let model = gun * w.fix;
        let (rest_models, pulled_models) = (part_models(w, gun, 0.0), part_models(w, gun, 1.0));
        for v in &views {
            save(v.name.into(), &shot(&gpu, &mut renderer, &mut avatar, &gun_meshes, &rest_models, &[], model, Some(&released), v));
            if matches!(v.name, "side" | "trigger" | "trigger_left" | "trigger_below" | "trigger_front" | "eye_down") {
                save(format!("{}_pulled", v.name), &shot(&gpu, &mut renderer, &mut avatar, &gun_meshes, &pulled_models, &[], model, Some(&pulled), v));
            }
            if !arms.is_empty() && !v.name.starts_with("eye") {
                save(format!("{}_metro", v.name), &shot(&gpu, &mut renderer, &mut avatar, &gun_meshes, &rest_models, &arms, model, None, v));
            }
        }
        // The full body's own hands (what the runtime draws by default).
        for v in views.iter().filter(|v| matches!(v.name, "side" | "trigger" | "trigger_left" | "trigger_below" | "grip" | "support" | "support_right")) {
            let full = View { visibility: BODYVIS_FULL, ..*v };
            save(format!("{}_body", v.name), &shot(&gpu, &mut renderer, &mut avatar, &gun_meshes, &rest_models, &[], model, Some(&released), &full));
        }
        // Every view of the gun on one sheet, at half size, to look over at once.
        let (sw, sh, cols) = (SIZE.0 / 2, SIZE.1 / 2, 5u32);
        let shots = shots.into_inner();
        let rows = (shots.len() as u32).div_ceil(cols);
        let mut sheet = vec![0u8; (sw * cols * sh * rows * 4) as usize];
        for (k, px) in shots.iter().enumerate() {
            let (ox, oy) = ((k as u32 % cols) * sw, (k as u32 / cols) * sh);
            for y in 0..sh {
                for x in 0..sw {
                    for c in 0..4 {
                        let at = |dx: u32, dy: u32| px[(((y * 2 + dy) * SIZE.0 + x * 2 + dx) * 4 + c) as usize] as u32;
                        let v = (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4;
                        sheet[(((oy + y) * sw * cols + ox + x) * 4 + c) as usize] = v as u8;
                    }
                }
            }
        }
        rustvr_gpu::save_png(&out_dir().join(format!("{}__sheet.png", w.key)), sw * cols, sh * rows, &sheet).unwrap();
        eprintln!("{}: renders in {}", w.key, out_dir().display());
    }
}

/// The poses the range registers are Metro's own hands: on the Kalash and the Bastard the drawn
/// firing wrist is exactly where Metro's arms model has it on the gun (the support hand may have
/// been moved a little out of the handguard by the fit).
#[test]
fn the_registered_poses_are_metros() {
    if !has_metro() {
        return;
    }
    let weapons = weapons();
    let refs: Vec<&Weapon> = weapons.iter().collect();
    let armory = physical::Armory::new(&refs);
    for w in weapons.iter().filter(|w| w.from_game && w.rigged()) {
        let arms = rustvr_asset::load_model(&metro_dir(), &format!("hands_{}", w.key.trim_start_matches("wpn_"))).unwrap();
        let i = refs.iter().position(|r| r.key == w.key).unwrap();
        for (id, side, bone) in [(physical::grip_pose_id(i), 1, "bip01_r_hand"), (physical::support_pose_id(i), 0, "bip01_l_hand")].into_iter().filter(|&(_, side, _)| side == 1 || !w.pistol) {
            let p = armory.poses.find(id).unwrap_or_else(|| panic!("{}: pose {id}", w.key));
            assert_eq!(p.side as usize, side);
            let b = arms.bones.iter().find(|b| b.name == bone).unwrap();
            let at = w.fix.transform_point3(Mat4::from_cols_array(&b.model_matrix).w_axis.truncate());
            let d = Vec3::from_array(p.wrist.pos).distance(at);
            eprintln!("{} {bone}: wrist {:.2} mm from Metro's; hand size {:.1} mm", w.key, d * 1000.0, p.size * 1000.0);
            let allowed = if side == 1 { 1e-4 } else { 0.0121 };
            assert!(d < allowed, "{}: {bone} {d} m off", w.key);
            assert!((0.07..0.12).contains(&p.size), "{}: a hand {} m long", w.key, p.size);
        }
    }
}

/// (Diagnostics) The trigger blades' faces: where they are and which way they face.
#[test]
fn print_trigger_blades() {
    if !has_metro() || std::env::var_os("POSE_DEBUG").is_none() {
        return;
    }
    for w in weapons().iter().filter(|w| w.rigged()) {
        eprintln!("{}: {} blade triangles", w.key, w.trigger_blade.len());
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for t in &w.trigger_blade {
            let c = (t[0] + t[1] + t[2]) / 3.0;
            lo = lo.min(c);
            hi = hi.max(c);
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            if n.z.abs() > 0.5 {
                eprintln!("    face at {c:.4} normal {n:.2}");
            }
        }
        eprintln!("  centres {lo:.4} .. {hi:.4}; trigger point {:.4}", w.trigger.unwrap());
    }
}

/// (Diagnostics) The first-person arms' hand and the full body's hand, side by side: size, finger
/// bases and segment lengths in the hand's frame (millimetres).
#[test]
fn print_hand_proportions() {
    if !has_metro() || std::env::var_os("POSE_DEBUG").is_none() {
        return;
    }
    for key in ["hands_basic_suit", "body_miller"] {
        let m = rustvr_asset::load_model(&metro_dir(), key).unwrap();
        let g = rustvr_avatar::game_hand(&m.bones, None, 1).unwrap();
        let at = |n: &str| m.bones.iter().find(|b| b.name == n).map(|b| Mat4::from_cols_array(&b.model_matrix).w_axis.truncate());
        let (w, q) = g.wrist;
        let local = |p: Vec3| (q.inverse() * (p - w)) * 1000.0;
        eprintln!("{key}: hand size {:.1} mm", g.size * 1000.0);
        for f in 0..5 {
            let names: Vec<String> = ["", "1", "2"].iter().map(|k| format!("bip01_r_finger{f}{k}")).collect();
            let pts: Vec<Option<Vec3>> = names.iter().map(|n| at(n)).collect();
            let lens: Vec<String> = pts.windows(2).map(|w| match (w[0], w[1]) { (Some(a), Some(b)) => format!("{:.1}", a.distance(b) * 1000.0), _ => "-".into() }).collect();
            eprintln!("  finger {f}: base {:.1?} segments {}", pts[0].map(local), lens.join(" / "));
        }
    }
}
