//! Close renders of the drawn hand holding things as the rig takes them: the controller brought
//! slowly to a thing (the hand stopped where its palm and fingers meet it), the grip squeezed, the
//! thing lifted clear; then the body solved and drawn (Metro's when its assets are extracted, else
//! the mannequin) with the thing beside it. `captures/grasp/<case>_<view>.png`. For looking at:
//! the fingers should close on the thing from where they are, on its surface, neither floating off
//! it nor sunk in.

use glam::{Mat3, Mat4, Quat, Vec3};
use rustvr_avatar::{HeldDraw, HeldModel, HeldPart};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::world::ColBox;
use rustvr_protocol::*;
use rustvr_runtime::backend::{EyeTarget, HmdFrame};
use rustvr_runtime::body::{BodyInput, PlayerBody};
use rustvr_runtime::rig::{Rig, RigInput, WorldMap};
use rustvr_runtime::settings::Settings;
use windows::Win32::Graphics::Dxgi::Common::*;

const DT: f32 = 1.0 / 90.0;
const SIZE: (u32, u32) = (800, 640);

fn out_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("captures").join("grasp")
}

fn pose(p: Vec3, q: Quat) -> Pose {
    Pose { pos: p.to_array(), valid: 1, rot: q.to_array() }
}

/// A fist round a tube along `tube` (little finger to thumb), the back of the hand towards `back`
/// (the controller's grip pose).
fn fist(tube: Vec3, back: Vec3) -> Quat {
    let z = tube.normalize();
    let x = (back - z * back.dot(z)).normalize();
    Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)).normalize()
}

fn look_at(from: Vec3, to: Vec3) -> (Vec3, Quat) {
    let z = (to - from).normalize();
    let x = Vec3::Y.cross(z).normalize_or(Vec3::X);
    (from, Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)))
}

/// A box's mesh (its half sizes), for drawing it.
fn box_model(half: Vec3) -> rustvr_asset::Model {
    let mut m = rustvr_asset::Mesh::default();
    for (n, u, v) in [(Vec3::X, Vec3::Y, Vec3::Z), (Vec3::NEG_X, Vec3::Z, Vec3::Y), (Vec3::Y, Vec3::Z, Vec3::X), (Vec3::NEG_Y, Vec3::X, Vec3::Z), (Vec3::Z, Vec3::X, Vec3::Y), (Vec3::NEG_Z, Vec3::Y, Vec3::X)] {
        let base = m.vertices.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = (n + u * a + v * b) * half;
            m.vertices.push(rustvr_asset::Vertex { pos: p.to_array(), normal: n.to_array(), ao: 255, ..Default::default() });
        }
        // Counter-clockwise seen from outside, in the left-handed space the assets use.
        m.indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
    }
    rustvr_asset::Model { meshes: vec![m], ..Default::default() }
}

struct Case {
    name: &'static str,
    /// The thing: where it starts, its half sizes.
    at: Vec3,
    half: Vec3,
    /// The right controller: turned so, from `from` slowly to `to` (into the thing: the hand
    /// stops where it meets it), then the grip, then lifted `lift`.
    rot: Quat,
    from: Vec3,
    to: Vec3,
}

fn world_box(rig: &mut Rig, id: u32, center: [f32; 3], half: [f32; 3]) {
    let b = ColBox { id, center, half_extents: half, rot: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
    rig.physics.apply_collision(COL_BOX, bytemuck::bytes_of(&b));
}

#[test]
fn hands_close_on_what_they_take() {
    let gpu = Gpu::new().unwrap();
    std::fs::create_dir_all(out_dir()).unwrap();
    let up = Vec3::Y;
    let cases = [
        // A bottle standing on the table, taken round its middle from its right.
        Case { name: "bottle", at: Vec3::new(0.0, 0.92, 0.6), half: Vec3::new(0.035, 0.12, 0.035), rot: fist(up, Vec3::X), from: Vec3::new(0.25, 0.94, 0.6), to: Vec3::new(0.0, 0.94, 0.6) },
        // A box, its near top edge taken from above, palm down, fingers over the edge.
        Case { name: "box_edge", at: Vec3::new(0.0, 0.9, 0.65), half: Vec3::splat(0.1), rot: fist(Vec3::X, up), from: Vec3::new(0.0, 1.25, 0.53), to: Vec3::new(0.0, 0.98, 0.53) },
        // A bar (a handle, a barrel) lying on the table, taken from above.
        Case { name: "bar", at: Vec3::new(0.0, 0.815, 0.6), half: Vec3::new(0.2, 0.015, 0.015), rot: fist(Vec3::X, up), from: Vec3::new(0.0, 1.15, 0.6), to: Vec3::new(0.0, 0.8, 0.6) },
        // A crate's top taken flat, palm down.
        Case { name: "crate_top", at: Vec3::new(0.0, 0.92, 0.6), half: Vec3::splat(0.12), rot: fist(Vec3::Z, up), from: Vec3::new(0.0, 1.3, 0.6), to: Vec3::new(0.0, 1.0, 0.6) },
    ];
    let settings = Settings::default();
    for case in &cases {
        let mut rig = Rig::default();
        world_box(&mut rig, 1, [0.0, -0.5, 0.0], [20.0, 0.5, 20.0]);
        world_box(&mut rig, 2, [0.0, 0.4, 0.6], [0.6, 0.4, 0.3]);
        let mut table: Box<ObjectTable> = bytemuck::zeroed_box();
        let d = &mut table.descs[0];
        d.id = 1;
        d.kind = OBJ_PROP;
        d.flags = OBJF_GRABBABLE;
        d.spawn_seq = 1;
        d.spawn_pose = pose(case.at, Quat::IDENTITY);
        d.shape = SHAPE_BOX;
        d.half_extents = case.half.to_array();
        d.mass = 1.0;
        table.count = 1;
        table.seq = 1;
        rig.sync(&table);
        let mut f = HmdFrame { index: 0, dt: DT, user_present: true, ..Default::default() };
        f.hmd = pose(Vec3::new(0.0, 1.7, 0.1), Quat::IDENTITY);
        for (i, x) in [(0, -0.25), (1, 0.25)] {
            f.hands[i].grip = pose(Vec3::new(x, 1.05, 0.35), Quat::IDENTITY);
            f.hands[i].aim = f.hands[i].grip;
            f.hands[i].connected = 1;
        }
        let mut now = 0.0;
        let mut step = |rig: &mut Rig, f: &mut HmdFrame, n: usize| {
            let mut out = Default::default();
            for _ in 0..n {
                f.index += 1;
                now += DT as f64;
                out = rig.update(&RigInput { frame: f, map: WorldMap::IDENTITY, dt: DT, now, settings: &settings, eye_height: 1.7, crouched: false, paused: false, infinite_ammo: false, arm_length: 0.0 });
            }
            out
        };
        step(&mut rig, &mut f, 30);
        let set = |f: &mut HmdFrame, p: Vec3, q: Quat| {
            f.hands[1].grip = pose(p, q);
            f.hands[1].aim = f.hands[1].grip;
        };
        // Over to the start, then slowly in (0.3 m/s).
        for k in 1..=40 {
            set(&mut f, Vec3::new(0.25, 1.05, 0.35).lerp(case.from, k as f32 / 40.0), Quat::IDENTITY.slerp(case.rot, k as f32 / 40.0));
            step(&mut rig, &mut f, 1);
        }
        let steps = ((case.from.distance(case.to) / 0.3) / DT).ceil() as usize;
        for k in 1..=steps {
            set(&mut f, case.from.lerp(case.to, k as f32 / steps as f32), case.rot);
            step(&mut rig, &mut f, 1);
        }
        let out = step(&mut rig, &mut f, 10);
        {
            let h = out.hands[1];
            let segs = rig.debug_fingers(1);
            eprintln!(
                "{}: controller {:.3?}, hand {:.3?} blocked {}, hand-to-thing {:.3}, finger tips {:?}",
                case.name,
                case.to,
                h.pose.pos,
                h.flags & HAND_BLOCKED != 0,
                rig.debug_hand_gap(1, 1),
                segs.iter().map(|(_, b)| format!("{:.3?}", b)).collect::<Vec<_>>()
            );
        }
        f.hands[1].squeeze = 1.0;
        step(&mut rig, &mut f, 3);
        assert_eq!(rig.held(1), Some(1), "{}: took it", case.name);
        // Lifted clear of the table.
        let lifted = case.to + Vec3::new(0.0, 0.25, -0.1);
        for k in 1..=30 {
            set(&mut f, case.to.lerp(lifted, k as f32 / 30.0), case.rot);
            step(&mut rig, &mut f, 1);
        }
        let out = step(&mut rig, &mut f, 30);
        let (op, oq) = rig.object_world_pose(1).unwrap();
        let grasp = rig.grasps()[1].clone();
        eprintln!("{}: grasp weight {:?}, {} solids", case.name, grasp.as_ref().map(|g| g.weight), grasp.as_ref().map_or(0, |g| g.solids.len()));
        // The body, as the session solves it.
        let mut body = PlayerBody::default();
        let hands = out.hands.map(|h| (Vec3::from_array(h.pose.pos), Quat::from_array(h.pose.rot)));
        for _ in 0..60 {
            body.update(&BodyInput {
                frame: &f,
                hands: Some(hands),
                hand_states: Some(out.hands),
                ghosts: [None; 2],
                holds: out.holds,
                held: [None; 2],
                grasps: [None, grasp.as_ref()],
                finger_give: [[0.0; 5]; 2],
                locomotion: Vec3::ZERO,
                eye_height: 1.7,
                floor: 0.0,
                saddle: None,
                settings: &settings,
            });
        }
        if let (Some(g), Ok(m)) = (grasp.as_ref(), rustvr_asset::load_model(&rustvr_asset::default_root().join("metro2033"), "body_miller")) {
            let curls = rustvr_avatar::grasp_curls(Some(&m.bones), &body.body.joints, 1, body.body.hands[1].pose, g).unwrap();
            let palm = Vec3::from_array(body.body.hands[1].pose.pos);
            eprintln!("{}: wrap curls {:?}; wrist {:.3?}, its distance from the thing {:.3}", case.name, curls.map(|c| c.map(|v| (v * 100.0).round() / 100.0)), palm, g.distance(palm));
            // Each segment's nearest to the surface: straight, and wrapped.
            let gap = |segs: &[Vec<(Vec3, Vec3)>; 5]| segs.clone().map(|s| s.iter().map(|&(a, b)| ((0..=10).map(|t| g.distance(a.lerp(b, t as f32 / 10.0))).fold(f32::INFINITY, f32::min) * 1000.0).round() / 10.0).collect::<Vec<_>>());
            let straight = rustvr_avatar::finger_segments(Some(&m.bones), &body.body.joints, 1, body.body.hands[1].pose, [[0.0; 3]; 5]).unwrap();
            let wrapped = rustvr_avatar::finger_segments(Some(&m.bones), &body.body.joints, 1, body.body.hands[1].pose, curls).unwrap();
            eprintln!("{}: segment gaps (cm) straight {:?}
   wrapped {:?}", case.name, gap(&straight), gap(&wrapped));
            if let Some(rustvr_avatar::Solid::Box { centre, rot, .. }) = g.solids.first() {
                let local = |p: Vec3| ((rot.inverse() * (p - *centre)) * 1000.0).round() / 10.0;
                for f in 0..5 {
                    eprintln!("   finger {f} (thing's frame, cm): {:?}", wrapped[f].iter().map(|&(a, b)| (local(a), local(b))).collect::<Vec<_>>());
                }
            }
        }
        let model = box_model(case.half);
        let all = HeldPart { part: 0, mesh: 0, vertices: (0..24).collect(), indices: model.meshes[0].indices.clone() };
        let held = HeldModel::load(&gpu, &std::env::temp_dir(), &model, &[all]).unwrap();
        let place = [Some(Mat4::from_rotation_translation(oq, op))];
        let hand_at = Vec3::from_array(out.hands[1].pose.pos);
        let centre = (hand_at + op) * 0.5;
        let views = [("front", centre + Vec3::new(0.05, 0.05, -0.32)), ("side", centre + Vec3::new(0.32, 0.06, 0.02)), ("under", centre + Vec3::new(-0.15, -0.22, -0.18)), ("back", centre + Vec3::new(-0.05, 0.1, 0.32)), ("top", centre + Vec3::new(0.01, 0.3, 0.02))];
        for (view, eye_at) in views {
            let (width, height) = SIZE;
            let tex = Texture::new(&gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)).unwrap();
            unsafe { gpu.ctx.ClearRenderTargetView(tex.rtv.as_ref().unwrap(), &[0.42, 0.45, 0.5, 1.0]) };
            let target = EyeTarget { tex: tex.tex.clone(), rtv: tex.rtv.clone().unwrap(), width, height };
            let (cam, rot) = look_at(eye_at, centre);
            let fov = Fov { left: -0.6, right: 0.6, up: 0.48, down: -0.48 };
            let light = LightHint { sun_dir: Vec3::new(-0.4, 0.8, -0.45).normalize().to_array(), sun_color: [1.0, 0.95, 0.9], ambient: [0.3, 0.32, 0.36], ..Default::default() };
            body.draw(&gpu, &target, &pose(cam, rot), &fov, light, None, &[HeldDraw { model: &held, parts: &place, key: "", overlay: None, bend: 0.0 }]);
            rustvr_gpu::save_png(&out_dir().join(format!("{}_{view}.png", case.name)), width, height, &tex.read_rgba8(&gpu).unwrap()).unwrap();
        }
    }
}
