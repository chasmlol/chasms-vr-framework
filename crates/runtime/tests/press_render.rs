//! Close renders of the drawn hand pressed flat on a wall, the controller pushed on into it: the
//! fingers give way (pushed open, flat on it) rather than staying curled into the wall, and the
//! same hand with them stiff for comparison. `captures/press/<case>_<view>.png`. For looking at.

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

fn pose(p: Vec3, q: Quat) -> Pose {
    Pose { pos: p.to_array(), valid: 1, rot: q.to_array() }
}

fn look_at(from: Vec3, to: Vec3) -> (Vec3, Quat) {
    let z = (to - from).normalize();
    let x = Vec3::Y.cross(z).normalize_or(Vec3::X);
    (from, Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)))
}

/// A box's mesh (its half sizes), for drawing the wall.
fn box_model(half: Vec3) -> rustvr_asset::Model {
    let mut m = rustvr_asset::Mesh::default();
    for (n, u, v) in [(Vec3::X, Vec3::Y, Vec3::Z), (Vec3::NEG_X, Vec3::Z, Vec3::Y), (Vec3::Y, Vec3::Z, Vec3::X), (Vec3::NEG_Y, Vec3::X, Vec3::Z), (Vec3::Z, Vec3::X, Vec3::Y), (Vec3::NEG_Z, Vec3::Y, Vec3::X)] {
        let base = m.vertices.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = (n + u * a + v * b) * half;
            m.vertices.push(rustvr_asset::Vertex { pos: p.to_array(), normal: n.to_array(), ao: 255, ..Default::default() });
        }
        m.indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
    }
    rustvr_asset::Model { meshes: vec![m], ..Default::default() }
}

#[test]
fn a_hand_pressed_flat_on_a_wall() {
    let gpu = Gpu::new().unwrap();
    let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("captures").join("press");
    std::fs::create_dir_all(&out_dir).unwrap();
    let settings = Settings::default();
    let mut rig = Rig::default();
    for (id, center, half) in [(1, [0.0, -0.5, 0.0], [20.0, 0.5, 20.0]), (2, [0.2, 1.3, 0.75], [0.4, 0.4, 0.05])] {
        let b = ColBox { id, center, half_extents: half, rot: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
        rig.physics.apply_collision(COL_BOX, bytemuck::bytes_of(&b));
    }
    let mut f = HmdFrame { index: 0, dt: DT, user_present: true, ..Default::default() };
    f.hmd = pose(Vec3::new(0.0, 1.7, 0.1), Quat::IDENTITY);
    for (i, x) in [(0, -0.25), (1, 0.25)] {
        f.hands[i].grip = pose(Vec3::new(x, 1.05, 0.35), Quat::IDENTITY);
        f.hands[i].aim = f.hands[i].grip;
        f.hands[i].connected = 1;
    }
    let mut now = 0.0;
    let mut out = Default::default();
    // The right palm to the wall (its face at z = 0.70), then pushed 12 cm on into it.
    let palm_on = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    for k in 1..=90 {
        let p = if k <= 40 { Vec3::new(0.25, 1.05, 0.35).lerp(Vec3::new(0.15, 1.3, 0.5), k as f32 / 40.0) } else { Vec3::new(0.15, 1.3, 0.5).lerp(Vec3::new(0.15, 1.3, 0.82), ((k - 40) as f32 / 30.0).min(1.0)) };
        f.hands[1].grip = pose(p, Quat::IDENTITY.slerp(palm_on, (k as f32 / 40.0).min(1.0)));
        f.hands[1].aim = f.hands[1].grip;
        f.index += 1;
        now += DT as f64;
        out = rig.update(&RigInput { frame: &f, map: WorldMap::IDENTITY, dt: DT, now, settings: &settings, eye_height: 1.7, crouched: false, paused: false, infinite_ammo: false, arm_length: 0.0 });
    }
    eprintln!("pressed: hand {:.3?}, finger give {:.2?}", out.hands[1].pose.pos, out.finger_give[1]);
    let model = box_model(Vec3::new(0.4, 0.4, 0.05));
    let all = HeldPart { part: 0, mesh: 0, vertices: (0..24).collect(), indices: model.meshes[0].indices.clone() };
    let wall = HeldModel::load(&gpu, &std::env::temp_dir(), &model, &[all]).unwrap();
    let wall_at = [Some(Mat4::from_translation(Vec3::new(0.2, 1.3, 0.75)))];
    for (case, give) in [("give", out.finger_give), ("stiff", [[0.0; 5]; 2])] {
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
                grasps: [None; 2],
                finger_give: give,
                locomotion: Vec3::ZERO,
                eye_height: 1.7,
                floor: 0.0,
                saddle: None,
                hands_only: false,
                settings: &settings,
            });
        }
        let at = Vec3::from_array(out.hands[1].pose.pos);
        for (view, eye) in [("side", at + Vec3::new(0.3, 0.05, -0.02)), ("above", at + Vec3::new(0.02, 0.32, -0.04))] {
            let (width, height) = SIZE;
            let tex = Texture::new(&gpu, &TexDesc::target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)).unwrap();
            unsafe { gpu.ctx.ClearRenderTargetView(tex.rtv.as_ref().unwrap(), &[0.42, 0.45, 0.5, 1.0]) };
            let target = EyeTarget { tex: tex.tex.clone(), rtv: tex.rtv.clone().unwrap(), width, height };
            let (cam, rot) = look_at(eye, at + Vec3::new(0.0, 0.0, 0.06));
            let fov = Fov { left: -0.6, right: 0.6, up: 0.48, down: -0.48 };
            let light = LightHint { sun_dir: Vec3::new(-0.4, 0.8, -0.45).normalize().to_array(), sun_color: [1.0, 0.95, 0.9], ambient: [0.3, 0.32, 0.36], ..Default::default() };
            body.draw(&gpu, &target, &pose(cam, rot), &fov, light, None, &[HeldDraw { model: &wall, parts: &wall_at, key: "", overlay: None, bend: 0.0 }]);
            rustvr_gpu::save_png(&out_dir.join(format!("{case}_{view}.png")), width, height, &tex.read_rgba8(&gpu).unwrap()).unwrap();
        }
    }
}
