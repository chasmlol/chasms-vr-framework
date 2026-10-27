//! Measures what drawing the avatar costs the GPU per eye (timestamp queries) and the CPU per
//! call. Its own test binary, so nothing else is rendering while it measures.

use std::time::Instant;

use glam::Vec3;
use rustvr_avatar::{Avatar, AvatarFrame, AvatarSource, DepthInput, pose};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

/// A Quest 3-sized eye.
const EYE: (u32, u32) = (2064, 2208);
const FOV: Fov = Fov { left: -1.0, right: 1.0, up: 0.95, down: -1.15 };
const FRAMES: usize = 200;

fn query(gpu: &Gpu, kind: D3D11_QUERY) -> ID3D11Query {
    let mut q = None;
    unsafe { gpu.device.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut q)).unwrap() };
    q.unwrap()
}

/// Waits for a query's result (`GetData` returns S_FALSE until it's ready, which the wrapper
/// would report as success, so it's called through the vtable).
fn data<T: Default>(gpu: &Gpu, q: &ID3D11Query) -> T {
    let mut out = T::default();
    loop {
        let hr = unsafe {
            (Interface::vtable(&gpu.ctx).GetData)(Interface::as_raw(&gpu.ctx), q.as_raw(), (&mut out as *mut T).cast(), size_of::<T>() as u32, 0)
        };
        if hr.0 == 0 {
            return out;
        }
        assert!(hr.0 > 0, "GetData failed: {hr:?}");
        std::thread::yield_now();
    }
}

/// Average GPU milliseconds per draw, and CPU microseconds per call.
fn measure(gpu: &Gpu, avatar: &mut Avatar, frame: &AvatarFrame, depth: Option<&Texture>) -> (f64, f64) {
    let target = Texture::new(gpu, &TexDesc::target(EYE.0, EYE.1, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)).unwrap();
    let rtv = target.rtv.as_ref().unwrap();
    let eye = pose::eye(&frame.joints[Joint::Head as usize], 1.78);
    let disjoint = query(gpu, D3D11_QUERY_TIMESTAMP_DISJOINT);
    let start = query(gpu, D3D11_QUERY_TIMESTAMP);
    let end = query(gpu, D3D11_QUERY_TIMESTAMP);
    let depth_input = || depth.map(|d| DepthInput { srv: d.srv.as_ref().unwrap(), units_per_meter: 1.0, bloom: None, lens: None });
    // Warm up (first use of shaders and buffers).
    for _ in 0..10 {
        avatar.draw(gpu, rtv, EYE.0, EYE.1, &eye, &FOV, frame, depth_input()).unwrap();
    }
    let mut gpu_ms = 0.0;
    let mut cpu = std::time::Duration::ZERO;
    for _ in 0..FRAMES {
        unsafe {
            gpu.ctx.Begin(&disjoint);
            gpu.ctx.End(&start);
        }
        let t = Instant::now();
        avatar.draw(gpu, rtv, EYE.0, EYE.1, &eye, &FOV, frame, depth_input()).unwrap();
        cpu += t.elapsed();
        unsafe {
            gpu.ctx.End(&end);
            gpu.ctx.End(&disjoint);
        }
        let freq: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = data(gpu, &disjoint);
        let (a, b): (u64, u64) = (data(gpu, &start), data(gpu, &end));
        assert!(!freq.Disjoint.as_bool(), "timestamps were disjoint");
        gpu_ms += (b - a) as f64 * 1000.0 / freq.Frequency as f64;
    }
    (gpu_ms / FRAMES as f64, cpu.as_secs_f64() * 1e6 / FRAMES as f64)
}

#[test]
fn gpu_cost_per_eye() {
    let gpu = Gpu::new().unwrap();
    let mut sources = vec![("procedural", AvatarSource::Procedural)];
    let dir = rustvr_asset::default_root().join("metro2033");
    if dir.join("models").join("body_miller.rvm").exists() {
        sources.push(("metro", AvatarSource::Model { dir, body: "body_miller".into(), hands: Some("hands_basic_suit".into()) }));
    }
    // A game depth far away, so the depth test runs but hides nothing.
    let far = vec![100f32; (EYE.0 * EYE.1) as usize];
    let depth = Texture::upload(&gpu, EYE.0, EYE.1, DXGI_FORMAT_R32_FLOAT, &[bytemuck::cast_slice(&far)]).unwrap();
    for (name, source) in sources {
        let mut avatar = Avatar::new(&gpu, source).unwrap();
        let gun = pose::holding_gun(1.78);
        // Looking down at yourself is the most pixels the body ever covers.
        let down = pose::looking_down(1.78, 65.0);
        // The ghost a little ahead of the right hand.
        let mut ghost = pose::hands_at_joints(&gun)[1].pose;
        ghost.pos = (Vec3::from_array(ghost.pos) + Vec3::new(0.0, 0.0, 0.1)).to_array();
        for (label, joints, visibility, ghosts) in [
            ("full body, aiming", &gun, BODYVIS_FULL, [None, None]),
            ("full body, looking down", &down, BODYVIS_FULL, [None, None]),
            ("hands + ghost", &gun, BODYVIS_HANDS, [None, Some(ghost)]),
        ] {
            let hands = pose::hands_at_joints(joints);
            let frame = AvatarFrame {
                joints,
                fingers: [pose::relaxed_hand(), pose::trigger_grip()],
                hands,
                ghosts,
                held: [None; 2],
                visibility,
                light: LightHint::default(),
                first_person: true,
                plain: false,
            };
            let (gpu_ms, cpu_us) = measure(&gpu, &mut avatar, &frame, Some(&depth));
            eprintln!("{name}, {label}, {}x{}: GPU {gpu_ms:.3} ms per eye, CPU {cpu_us:.1} us per draw call", EYE.0, EYE.1);
            assert!(gpu_ms < 1.0, "{name} {label}: {gpu_ms:.3} ms per eye");
        }
    }
}
