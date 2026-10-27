//! The player a game drew inside its own frame, moved to where it is now
//! ([`Avatar::draw_reprojected`]): the game's pixels of the player land where a picture drawn at
//! the new pose has it, in the game's colours.

use glam::{Quat, Vec3};
use rustvr_avatar::{Avatar, AvatarFrame, AvatarSource, PlayerSource, pose};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::*;

const HEIGHT: f32 = 1.78;
const N: u32 = 256;
const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn frame(joints: &[Pose; JOINT_COUNT]) -> AvatarFrame<'_> {
    AvatarFrame {
        joints,
        fingers: [pose::relaxed_hand(); 2],
        hands: pose::hands_at_joints(joints),
        ghosts: [None; 2],
        held: [None; 2],
        visibility: BODYVIS_FULL,
        light: LightHint::default(),
        first_person: false,
        plain: false,
    }
}

fn target(gpu: &Gpu, format: DXGI_FORMAT) -> Texture {
    Texture::new(gpu, &TexDesc { rtv: Some(format), srv: Some(format), ..TexDesc::target(N, N, format) }).unwrap()
}

fn clear(gpu: &Gpu, t: &Texture, c: [f32; 4]) {
    unsafe { gpu.ctx.ClearRenderTargetView(t.rtv.as_ref().unwrap(), &c) };
}

/// Pixels that aren't the background, and their colours.
fn drawn(gpu: &Gpu, t: &Texture) -> Vec<Option<[u8; 3]>> {
    t.read_rgba8(gpu).unwrap().chunks(4).map(|p| (p[0] > 2 || p[1] > 2 || p[2] > 2).then(|| [p[0], p[1], p[2]])).collect()
}

#[test]
fn the_games_pixels_of_the_player_follow_it() {
    let gpu = Gpu::new().unwrap();
    let mut avatar = Avatar::new(&gpu, AvatarSource::Procedural).unwrap();
    // Seen from 2.6 m in front, looking back at it.
    let eye = Pose { pos: [0.0, 1.2, 2.6], valid: 1, rot: Quat::from_rotation_y(std::f32::consts::PI).to_array() };
    let fov = Fov { left: -0.6, right: 0.6, up: 0.6, down: -0.6 };
    let then = pose::reaching_forward(HEIGHT, [0.4, 0.3]);
    // Now 12 cm to the side and turned a little.
    let turn = Quat::from_rotation_y(0.15);
    let mut now = then;
    for j in now.iter_mut() {
        j.pos = (turn * Vec3::from_array(j.pos) + Vec3::new(0.12, 0.0, 0.0)).to_array();
        j.rot = (turn * Quat::from_array(j.rot)).to_array();
    }

    // The game's picture: the player posed as then, its view depth there.
    let srgb = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
    let picture = target(&gpu, srgb);
    clear(&gpu, &picture, BACKGROUND);
    avatar.draw(&gpu, picture.rtv.as_ref().unwrap(), N, N, &eye, &fov, &frame(&then), None).unwrap();
    let depth = target(&gpu, DXGI_FORMAT_R32_FLOAT);
    avatar.draw_view_depth(&gpu, depth.rtv.as_ref().unwrap(), N, N, &eye, &fov, &frame(&then), None, &[]).unwrap();

    // Its pixels moved to the pose now, against a picture drawn there.
    let moved = target(&gpu, srgb);
    clear(&gpu, &moved, BACKGROUND);
    let sources = [PlayerSource { color: picture.srv.as_ref().unwrap(), player_depth: depth.srv.as_ref().unwrap(), eye, fov }];
    avatar.draw_reprojected(&gpu, moved.rtv.as_ref().unwrap(), N, N, &eye, &fov, &frame(&now), &frame(&then), None, &[], &sources).unwrap();
    let direct = target(&gpu, srgb);
    clear(&gpu, &direct, BACKGROUND);
    avatar.draw(&gpu, direct.rtv.as_ref().unwrap(), N, N, &eye, &fov, &frame(&now), None).unwrap();

    let (a, b, src) = (drawn(&gpu, &moved), drawn(&gpu, &direct), drawn(&gpu, &picture));
    let both = a.iter().zip(&b).filter(|(x, y)| x.is_some() && y.is_some()).count();
    let either = a.iter().zip(&b).filter(|(x, y)| x.is_some() || y.is_some()).count();
    let iou = both as f32 / either.max(1) as f32;
    assert!(either > 2000, "the player is in view: {either} pixels");
    assert!(iou > 0.97, "the moved pixels cover what the new pose covers: {iou:.3}");
    // Somewhere the moved player is now where it wasn't in the picture (it did move)...
    let new_place = a.iter().zip(&src).filter(|(x, y)| x.is_some() && y.is_none()).count();
    assert!(new_place > 300, "it moved: {new_place} pixels where it wasn't");
    // ... in the picture's colours: as the new pose's own shading has them, give or take the view.
    let diff: f32 = a.iter().zip(&b).filter_map(|(x, y)| Some((x.as_ref()?, y.as_ref()?))).map(|(x, y)| (0..3).map(|k| (x[k] as f32 - y[k] as f32).abs()).sum::<f32>() / 3.0).sum::<f32>() / both.max(1) as f32;
    assert!(diff < 12.0, "the colours are the picture's: mean difference {diff:.1} of 255");
}
