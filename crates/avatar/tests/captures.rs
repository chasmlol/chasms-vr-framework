//! Headless renders of the avatar, written to `captures/avatar/` to look at, plus checks that the
//! head chop and the game-depth test do their job.
//!
//! The Metro 2033 renders need the user's extracted assets (`local_assets/metro2033`); without
//! them those tests only render the mannequin.

#![allow(clippy::too_many_arguments)]

use std::path::{Path, PathBuf};

use glam::{Mat3, Quat, Vec3};
use rustvr_avatar::{Avatar, AvatarFrame, AvatarSource, DepthInput, pose};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::*;

const HEIGHT: f32 = 1.78;

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("captures").join("avatar")
}

fn metro() -> Option<AvatarSource> {
    let dir = rustvr_asset::default_root().join("metro2033");
    if !dir.join("models").join("body_miller.rvm").exists() {
        eprintln!("no Metro 2033 assets in {}: skipping the Metro renders", dir.display());
        return None;
    }
    Some(AvatarSource::Model { dir, body: "body_miller".into(), hands: Some("hands_basic_suit".into()) })
}

/// The avatars to render: the mannequin, and Metro's Miller when the assets are there.
fn avatars(gpu: &Gpu) -> Vec<(&'static str, Avatar)> {
    let mut out = vec![("procedural", Avatar::new(gpu, AvatarSource::Procedural).unwrap())];
    if let Some(src) = metro() {
        out.push(("metro", Avatar::new(gpu, src).unwrap()));
    }
    out
}

#[derive(Clone)]
struct Scene {
    joints: [Pose; JOINT_COUNT],
    fingers: [Fingers; 2],
    hands: [HandState; 2],
    ghosts: [Option<Pose>; 2],
    visibility: u32,
    light: LightHint,
    first_person: bool,
}

impl Scene {
    fn new(joints: [Pose; JOINT_COUNT], fingers: [Fingers; 2]) -> Scene {
        Scene { hands: pose::hands_at_joints(&joints), joints, fingers, ghosts: [None; 2], visibility: BODYVIS_FULL, light: daylight(), first_person: false }
    }

    fn frame(&self) -> AvatarFrame<'_> {
        AvatarFrame {
            joints: &self.joints,
            fingers: self.fingers,
            hands: self.hands,
            ghosts: self.ghosts,
            held: [None; 2],
            visibility: self.visibility,
            light: self.light,
            first_person: self.first_person,
            plain: false,
        }
    }

    fn eye(&self) -> Pose {
        pose::eye(&self.joints[Joint::Head as usize], HEIGHT)
    }
}

fn daylight() -> LightHint {
    let d = Vec3::new(-0.45, 0.8, 0.5).normalize();
    LightHint { sun_dir: d.to_array(), sun_color: [1.0, 0.94, 0.86], ambient: [0.17, 0.19, 0.23], ..Default::default() }
}

fn dark() -> LightHint {
    let d = Vec3::new(0.3, 0.6, -0.7).normalize();
    LightHint { sun_dir: d.to_array(), sun_color: [0.03, 0.03, 0.035], ambient: [0.012, 0.013, 0.016], ..Default::default() }
}

/// A camera at `from` looking at `to`, upright.
fn look_at(from: Vec3, to: Vec3) -> Pose {
    let z = (to - from).normalize();
    let x = Vec3::Y.cross(z).normalize();
    let y = z.cross(x);
    Pose { pos: from.to_array(), valid: 1, rot: Quat::from_mat3(&Mat3::from_cols(x, y, z)).to_array() }
}

fn symmetric(tan: f32, aspect: f32) -> Fov {
    Fov { left: -tan * aspect, right: tan * aspect, up: tan, down: -tan }
}

/// Roughly a headset's per-eye field of view.
const HEADSET: Fov = Fov { left: -1.0, right: 1.0, up: 0.95, down: -1.15 };

/// What the background behind the body looks like.
#[derive(Clone, Copy, PartialEq)]
enum Backdrop {
    /// Sky and a chequered floor at y = 0 (half-metre tiles).
    Floor,
    /// The floor, with a wall covering the right half of the view.
    WallRight,
    /// A flat colour, for counting covered pixels.
    Plain,
}

const PLAIN: [u8; 4] = [20, 22, 26, 255];

fn backdrop_pixels(kind: Backdrop, w: u32, h: u32, eye: &Pose, fov: &Fov, light: &LightHint) -> Vec<u8> {
    let rot = Quat::from_array(eye.rot);
    let origin = Vec3::from_array(eye.pos);
    let bright = if light.ambient[0] < 0.05 { 0.08 } else { 1.0 };
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = match kind {
                Backdrop::Plain => {
                    px.extend_from_slice(&PLAIN);
                    continue;
                }
                Backdrop::WallRight if x >= w / 2 => [0.16, 0.14, 0.12],
                _ => {
                    let u = (x as f32 + 0.5) / w as f32;
                    let v = (y as f32 + 0.5) / h as f32;
                    let d = rot * Vec3::new(fov.left + u * (fov.right - fov.left), fov.up + v * (fov.down - fov.up), 1.0);
                    if d.y < 0.0 {
                        let p = origin + d * (-origin.y / d.y);
                        let check = ((p.x * 2.0).floor() + (p.z * 2.0).floor()) as i32 & 1 == 0;
                        let fade = (1.0 - (p - origin).length() / 30.0).clamp(0.0, 1.0);
                        let f = if check { 0.16 } else { 0.12 } * fade + 0.05 * (1.0 - fade);
                        [f, f * 0.98, f * 0.95]
                    } else {
                        let t = d.y / d.length();
                        [0.05 + 0.10 * t, 0.06 + 0.13 * t, 0.08 + 0.2 * t]
                    }
                }
            };
            for ch in c {
                px.push((linear_to_srgb(ch * bright) * 255.0 + 0.5) as u8);
            }
            px.push(255);
        }
    }
    px
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Renders a scene from `eye` and returns the RGBA8 (sRGB) pixels.
fn render(gpu: &Gpu, avatar: &mut Avatar, size: (u32, u32), eye: &Pose, fov: &Fov, scene: &Scene, backdrop: Backdrop, depth: Option<DepthInput>) -> Vec<u8> {
    let (w, h) = size;
    let target = Texture::new(gpu, &TexDesc::target(w, h, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)).unwrap();
    let bg = backdrop_pixels(backdrop, w, h, eye, fov, &scene.light);
    unsafe { gpu.ctx.UpdateSubresource(&target.tex, 0, None, bg.as_ptr().cast(), w * 4, 0) };
    avatar.draw(gpu, target.rtv.as_ref().unwrap(), w, h, eye, fov, &scene.frame(), depth).unwrap();
    target.read_rgba8(gpu).unwrap()
}

fn save(name: &str, size: (u32, u32), px: &[u8]) {
    let path = out_dir().join(format!("{name}.png"));
    rustvr_gpu::save_png(&path, size.0, size.1, px).unwrap();
}

/// A fake game depth (R32_FLOAT, game units): `wall` in the right half, nothing elsewhere.
fn wall_depth(gpu: &Gpu, (w, h): (u32, u32), wall: f32) -> Texture {
    let mut texels = vec![0f32; (w * h) as usize];
    for y in 0..h {
        for x in w / 2..w {
            texels[(y * w + x) as usize] = wall;
        }
    }
    Texture::upload(gpu, w, h, DXGI_FORMAT_R32_FLOAT, &[bytemuck::cast_slice(&texels)]).unwrap()
}

/// Screen pixel of a tracking-space point.
fn project(eye: &Pose, fov: &Fov, (w, h): (u32, u32), p: Vec3) -> (u32, u32) {
    let local = Quat::from_array(eye.rot).inverse() * (p - Vec3::from_array(eye.pos));
    let (tx, ty) = (local.x / local.z, local.y / local.z);
    let u = (tx - fov.left) / (fov.right - fov.left);
    let v = (fov.up - ty) / (fov.up - fov.down);
    ((u * w as f32) as u32, (v * h as f32) as u32)
}

/// Share of pixels inside `rect` (x0, y0, x1, y1) that aren't the plain backdrop.
fn coverage(px: &[u8], w: u32, rect: (u32, u32, u32, u32)) -> f32 {
    let (x0, y0, x1, y1) = rect;
    let mut covered = 0;
    let mut total = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            total += 1;
            if px[i..i + 4] != PLAIN {
                covered += 1;
            }
        }
    }
    covered as f32 / total.max(1) as f32
}

/// A point `along` metres from the wrist towards the fingers.
fn on_hand(hand: &Pose, along: f32) -> Vec3 {
    Vec3::from_array(hand.pos) + Quat::from_array(hand.rot) * Vec3::new(0.0, along, 0.0) * (HEIGHT / 1.75)
}

fn palm_centre(hand: &Pose) -> Vec3 {
    on_hand(hand, 0.05)
}

#[test]
fn renders() {
    let gpu = Gpu::new().unwrap();
    eprintln!("GPU: {}", gpu.adapter_name());
    let third = (640, 800);
    let fp = (800, 800);
    let tall = symmetric(0.42, 0.8);
    // Left hand round the handguard, right hand on the grip with a finger on the trigger.
    let foregrip = Fingers { curl: [0.6, 0.75, 0.8, 0.85, 0.85], splay: 0.1, _pad: [0.0; 2] };
    let gun_fingers = [foregrip, pose::trigger_grip()];
    for (name, mut avatar) in avatars(&gpu) {
        let mut shot = |label: &str, size: (u32, u32), eye: Pose, fov: Fov, scene: &Scene, backdrop: Backdrop, depth: Option<DepthInput>| {
            let px = render(&gpu, &mut avatar, size, &eye, &fov, scene, backdrop, depth);
            save(&format!("{name}_{label}"), size, &px);
        };
        let target = Vec3::new(0.0, 0.98, 0.0);
        let front = look_at(Vec3::new(0.0, 1.15, 3.4), target);
        let side = look_at(Vec3::new(3.4, 1.15, 0.0), target);
        let three_quarter = look_at(Vec3::new(2.3, 1.6, 2.5), target);

        let tpose = Scene::new(pose::t_pose(HEIGHT), [pose::open_hand(); 2]);
        shot("tpose_front", third, front, symmetric(0.42, 0.8 * 1.25), &tpose, Backdrop::Floor, None);
        let standing = Scene::new(pose::arms_down(HEIGHT), [pose::relaxed_hand(); 2]);
        shot("front", third, front, tall, &standing, Backdrop::Floor, None);
        shot("side", third, side, tall, &standing, Backdrop::Floor, None);
        shot("three_quarter", third, three_quarter, tall, &standing, Backdrop::Floor, None);
        let gun = Scene::new(pose::holding_gun(HEIGHT), gun_fingers);
        shot("gun_three_quarter", third, three_quarter, tall, &gun, Backdrop::Floor, None);
        let crouch = Scene::new(pose::crouch(HEIGHT), gun_fingers);
        shot("crouch_side", third, look_at(Vec3::new(3.0, 0.9, 0.6), Vec3::new(0.0, 0.65, 0.0)), tall, &crouch, Backdrop::Floor, None);
        let mut night = standing.clone();
        night.light = dark();
        shot("dark", third, three_quarter, tall, &night, Backdrop::Floor, None);

        // First person.
        let mut fp_gun = gun.clone();
        fp_gun.first_person = true;
        shot("fp_forward", fp, fp_gun.eye(), HEADSET, &fp_gun, Backdrop::Floor, None);
        let mut down = Scene::new(pose::looking_down(HEIGHT, 65.0), [pose::relaxed_hand(); 2]);
        down.first_person = true;
        shot("fp_down", fp, down.eye(), HEADSET, &down, Backdrop::Floor, None);
        for (label, vis) in [("fp_hands", BODYVIS_HANDS), ("fp_arms", BODYVIS_ARMS)] {
            let mut s = fp_gun.clone();
            s.visibility = vis;
            shot(label, fp, s.eye(), HEADSET, &s, Backdrop::Floor, None);
        }

        // Fingers, close up on the right hand reaching forward.
        let reach = pose::reaching_forward(HEIGHT, [0.53, 0.53]);
        let hand = reach[Joint::RightHand as usize];
        // From above (the back of the hand), and from below (the palm and the thumb).
        let hand_cam = look_at(palm_centre(&hand) + Vec3::new(0.22, 0.20, 0.18), palm_centre(&hand));
        let palm_cam = look_at(palm_centre(&hand) + Vec3::new(-0.12, -0.25, 0.22), palm_centre(&hand));
        for (label, fingers) in [("hand_open", pose::open_hand()), ("hand_fist", pose::fist()), ("hand_trigger", pose::trigger_grip())] {
            let s = Scene::new(reach, [fingers; 2]);
            shot(label, (600, 600), hand_cam, symmetric(0.3, 1.0), &s, Backdrop::Floor, None);
            shot(&format!("{label}_palm"), (600, 600), palm_cam, symmetric(0.3, 1.0), &s, Backdrop::Floor, None);
        }

        // Ghost: the right hand is held back 12 cm (say by a wall); the controller is further on.
        let controller = reach[Joint::RightHand as usize];
        let mut blocked = Scene::new(pose::reaching_forward(HEIGHT, [0.53, 0.41]), [pose::relaxed_hand(), pose::open_hand()]);
        blocked.hands[1].flags |= HAND_BLOCKED;
        blocked.ghosts[1] = Some(controller);
        blocked.first_person = true;
        shot("fp_ghost", fp, blocked.eye(), HEADSET, &blocked, Backdrop::Floor, None);
        blocked.visibility = BODYVIS_HANDS;
        shot("fp_ghost_hands", fp, blocked.eye(), HEADSET, &blocked, Backdrop::Floor, None);
        let mut outside = blocked.clone();
        outside.visibility = BODYVIS_FULL;
        outside.first_person = false;
        shot("ghost_three_quarter", third, look_at(Vec3::new(1.6, 1.7, 1.6), Vec3::new(0.1, 1.35, 0.35)), symmetric(0.3, 0.8), &outside, Backdrop::Floor, None);

        // Game depth: a wall 0.4 m in front over the right half of the view (in centimetres).
        let mut reaching = Scene::new(reach, [pose::relaxed_hand(); 2]);
        reaching.first_person = true;
        let wall = wall_depth(&gpu, fp, 40.0);
        let depth = DepthInput { srv: wall.srv.as_ref().unwrap(), units_per_meter: 100.0, bloom: None, lens: None };
        shot("fp_wall", fp, reaching.eye(), HEADSET, &reaching, Backdrop::WallRight, Some(depth));
        shot("fp_reach", fp, reaching.eye(), HEADSET, &reaching, Backdrop::Floor, None);

        // Head chop: looking straight ahead from inside the head.
        let mut ahead = Scene::new(pose::arms_down(HEIGHT), [pose::relaxed_hand(); 2]);
        ahead.first_person = true;
        shot("fp_chop", fp, ahead.eye(), HEADSET, &ahead, Backdrop::Floor, None);
        ahead.first_person = false;
        shot("fp_no_chop", fp, ahead.eye(), HEADSET, &ahead, Backdrop::Floor, None);
        // What the chop leaves, seen from outside: the neck closes over where the head was.
        ahead.first_person = true;
        shot("chop_outside", (600, 600), look_at(Vec3::new(0.45, 1.9, 0.6), Vec3::new(0.0, 1.45, 0.0)), symmetric(0.35, 1.0), &ahead, Backdrop::Floor, None);
    }
}

#[test]
fn head_chop_hides_the_head() {
    let gpu = Gpu::new().unwrap();
    let size = (400, 400);
    for (name, mut avatar) in avatars(&gpu) {
        let mut scene = Scene::new(pose::arms_down(HEIGHT), [pose::relaxed_hand(); 2]);
        let eye = scene.eye();
        // The upper 60% of the view, looking ahead with the arms down: nothing of the body is there
        // unless the head is.
        let upper = (0, 0, size.0, size.1 * 6 / 10);
        scene.first_person = true;
        let chopped = coverage(&render(&gpu, &mut avatar, size, &eye, &HEADSET, &scene, Backdrop::Plain, None), size.0, upper);
        scene.first_person = false;
        let whole = coverage(&render(&gpu, &mut avatar, size, &eye, &HEADSET, &scene, Backdrop::Plain, None), size.0, upper);
        eprintln!("{name}: head chopped {:.1}% covered, whole {:.1}%", chopped * 100.0, whole * 100.0);
        assert!(chopped < 0.005, "{name}: head geometry in view ({:.2}%)", chopped * 100.0);
        // (Much of the face is nearer than the near plane, so the whole head covers less than
        // the view, but it's there.)
        assert!(whole > 0.03, "{name}: without the chop the head is in view");
        // Looking down: the neck and body are there, nothing above the chin.
        let mut down = Scene::new(pose::looking_down(HEIGHT, 65.0), [pose::relaxed_hand(); 2]);
        down.first_person = true;
        let px = render(&gpu, &mut avatar, size, &down.eye(), &HEADSET, &down, Backdrop::Plain, None);
        assert!(coverage(&px, size.0, (0, 0, size.0, size.1)) > 0.05, "{name}: the body is visible looking down");
    }
}

#[test]
fn game_depth_hides_the_hand_behind_a_wall() {
    let gpu = Gpu::new().unwrap();
    let size = (400, 400);
    let wall = wall_depth(&gpu, size, 40.0);
    for (name, mut avatar) in avatars(&gpu) {
        let mut scene = Scene::new(pose::reaching_forward(HEIGHT, [0.53, 0.53]), [pose::relaxed_hand(); 2]);
        scene.first_person = true;
        let eye = scene.eye();
        let window = |p: Vec3| {
            let (x, y) = project(&eye, &HEADSET, size, p);
            (x.saturating_sub(6), y.saturating_sub(6), (x + 6).min(size.0), (y + 6).min(size.1))
        };
        // Over the knuckles, clear of the sleeve cuff (which is in front of the wall).
        let left = window(on_hand(&scene.joints[Joint::LeftHand as usize], 0.085));
        let right = window(on_hand(&scene.joints[Joint::RightHand as usize], 0.085));
        assert!(right.0 > size.0 / 2 + 6 && left.2 < size.0 / 2, "the hands are either side of the wall's edge");
        let open = render(&gpu, &mut avatar, size, &eye, &HEADSET, &scene, Backdrop::Plain, None);
        let depth = DepthInput { srv: wall.srv.as_ref().unwrap(), units_per_meter: 100.0, bloom: None, lens: None };
        let walled = render(&gpu, &mut avatar, size, &eye, &HEADSET, &scene, Backdrop::Plain, Some(depth));
        let (r_open, r_wall) = (coverage(&open, size.0, right), coverage(&walled, size.0, right));
        let (l_open, l_wall) = (coverage(&open, size.0, left), coverage(&walled, size.0, left));
        eprintln!("{name}: right hand {r_open} -> {r_wall}, left hand {l_open} -> {l_wall}");
        assert!(r_open > 0.9 && l_open > 0.9, "{name}: both hands drawn without the wall");
        assert!(r_wall < 0.02, "{name}: the hand behind the wall is hidden");
        assert!(l_wall > 0.9, "{name}: the other hand is untouched");
    }
}

#[test]
fn the_game_lens_bends_the_player_as_it_bends_the_picture() {
    let gpu = Gpu::new().unwrap();
    let size = (400, 400);
    let nothing = wall_depth(&gpu, size, 0.0);
    // The game's bloom and lens (half size): no light, and a warp of `warp` in x everywhere.
    let post = |warp: f32| {
        let texels: Vec<[f32; 4]> = vec![[0.0, 0.0, 0.0, warp]; (size.0 * size.1 / 4) as usize];
        let lens: Vec<[f32; 4]> = vec![[0.0; 4]; texels.len()];
        let up = |t: &[[f32; 4]]| Texture::upload(&gpu, size.0 / 2, size.1 / 2, DXGI_FORMAT_R32G32B32A32_FLOAT, &[bytemuck::cast_slice(t)]).unwrap();
        (up(&texels), up(&lens))
    };
    // Mean x of the pixels that aren't the backdrop.
    let centre_x = |px: &[u8]| {
        let (mut sum, mut n) = (0.0, 0.0);
        for (i, p) in px.chunks(4).enumerate() {
            if p != PLAIN {
                sum += (i as u32 % size.0) as f32;
                n += 1.0;
            }
        }
        sum / n
    };
    for (name, mut avatar) in avatars(&gpu) {
        let mut scene = Scene::new(pose::reaching_forward(HEIGHT, [0.53, 0.53]), [pose::relaxed_hand(); 2]);
        scene.first_person = true;
        let eye = scene.eye();
        let mut draw = |post: Option<&(Texture, Texture)>| {
            let depth = DepthInput {
                srv: nothing.srv.as_ref().unwrap(),
                units_per_meter: 100.0,
                bloom: post.map(|p| p.0.srv.as_ref().unwrap()),
                lens: post.map(|p| p.1.srv.as_ref().unwrap()),
            };
            render(&gpu, &mut avatar, size, &eye, &HEADSET, &scene, Backdrop::Plain, Some(depth))
        };
        let straight = draw(None);
        let still = draw(Some(&post(0.0)));
        let bent = draw(Some(&post(0.05)));
        // Without a warp the layer lays the player down where it was drawn (bar rounding at edges).
        let differ = straight.chunks(4).zip(still.chunks(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 2)).count();
        let (c0, c1) = (centre_x(&straight), centre_x(&bent));
        eprintln!("{name}: {differ} pixels differ unbent; bent moves the player {:.1} px", c1 - c0);
        assert!(differ < 20, "{name}: the layer without a warp changes the player ({differ} pixels)");
        // Showing at uv what was drawn at uv + 0.05: the player moves 5% of the width left.
        assert!((c1 - c0 + 0.05 * size.0 as f32).abs() < 3.0, "{name}: bent by {:.1} px", c1 - c0);
    }
}

#[test]
fn a_laser_shows_and_whats_nearer_hides_it() {
    let gpu = Gpu::new().unwrap();
    let size = (400, 400);
    // Everything 33 cm away (game depth in centimetres): a wall right at the muzzle.
    let wall = {
        let texels = vec![33.0f32; (size.0 * size.1) as usize];
        Texture::upload(&gpu, size.0, size.1, DXGI_FORMAT_R32_FLOAT, &[bytemuck::cast_slice(&texels)]).unwrap()
    };
    let nothing = wall_depth(&gpu, size, 0.0);
    // Red light: the laser's.
    let red = |px: &[u8]| px.chunks(4).filter(|p| p[0] as i32 > p[1] as i32 + 40 && p[0] > 60).count();
    for (name, mut avatar) in avatars(&gpu) {
        let mut scene = Scene::new(pose::arms_down(HEIGHT), [pose::relaxed_hand(); 2]);
        scene.first_person = true;
        let eye = scene.eye();
        let at = |v: Vec3| Vec3::from_array(eye.pos) + Quat::from_array(eye.rot) * v;
        // From low right, out along the view to a dot 3 m ahead.
        avatar.set_laser(Some(rustvr_avatar::LaserBeam { from: at(Vec3::new(0.12, -0.15, 0.3)), to: at(Vec3::new(0.0, 0.0, 3.0)), hit: true }));
        let mut draw = |depth: &Texture| {
            let d = DepthInput { srv: depth.srv.as_ref().unwrap(), units_per_meter: 100.0, bloom: None, lens: None };
            render(&gpu, &mut avatar, size, &eye, &HEADSET, &scene, Backdrop::Plain, Some(d))
        };
        let shot = draw(&nothing);
        save(&format!("{name}_laser"), size, &shot);
        let open = red(&shot);
        let walled = red(&draw(&wall));
        avatar.set_laser(None);
        eprintln!("{name}: laser {open} red pixels in the open, {walled} with a wall 33 cm away");
        assert!(open > 200, "{name}: the laser shows ({open} pixels)");
        assert!(walled * 4 < open, "{name}: the wall hides most of it ({walled} of {open})");
    }
}
