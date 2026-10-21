//! A simulated headset for automated tests: scripted head and hand poses on a fixed clock, eyes
//! rendered offscreen and saved as PNGs on request. No window, no VR runtime.

use anyhow::Result;
use glam::{Quat, Vec2, Vec3};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

use super::{Backend, EyeTarget, HmdFrame};

#[derive(Clone, Copy, Debug)]
pub struct SimHand {
    pub pos: Vec3,
    pub rot: Quat,
    pub trigger: f32,
    pub squeeze: f32,
    pub stick: Vec2,
    /// CTRL_* bits.
    pub buttons: u32,
    pub connected: bool,
}

impl Default for SimHand {
    fn default() -> Self {
        SimHand { pos: Vec3::ZERO, rot: Quat::IDENTITY, trigger: 0.0, squeeze: 0.0, stick: Vec2::ZERO, buttons: 0, connected: true }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SimInput {
    pub head_pos: Vec3,
    pub head_rot: Quat,
    pub hands: [SimHand; 2],
}

/// Supplies the simulated player's body each frame.
pub trait SimDriver {
    fn input(&mut self, frame: u64, t: f64) -> SimInput;
}

pub struct SimBackend {
    driver: Box<dyn SimDriver>,
    size: (u32, u32),
    hz: f32,
    /// Sleep to keep real time (manual runs); tests run as fast as possible.
    realtime: bool,
    pub ipd: f32,
    index: u64,
    start: std::time::Instant,
    prev_hand: [Option<Vec3>; 2],
    eyes: Option<[Texture; 2]>,
    /// Every vibration requested, for checks: (frame, hand, seconds, frequency, amplitude).
    pub haptics: Vec<(u64, usize, f32, f32, f32)>,
    pub submitted: u64,
    pub last_poses: [Pose; 2],
}

impl SimBackend {
    pub fn new(driver: Box<dyn SimDriver>, size: (u32, u32), hz: f32, realtime: bool) -> SimBackend {
        SimBackend {
            driver,
            size,
            hz,
            realtime,
            ipd: 0.064,
            index: 0,
            start: std::time::Instant::now(),
            prev_hand: [None; 2],
            eyes: None,
            haptics: Vec::new(),
            submitted: 0,
            last_poses: [Pose::default(); 2],
        }
    }

    pub fn frame_index(&self) -> u64 {
        self.index
    }

    /// Saves the last submitted frame as one PNG, left eye | right eye.
    pub fn capture(&self, gpu: &Gpu, path: &std::path::Path) -> Result<()> {
        let Some(eyes) = &self.eyes else { anyhow::bail!("nothing rendered yet") };
        let (w, h) = (eyes[0].width, eyes[0].height);
        let l = eyes[0].read_rgba8(gpu)?;
        let r = eyes[1].read_rgba8(gpu)?;
        let mut out = vec![0u8; (w * 2 * h * 4) as usize];
        let row = (w * 4) as usize;
        for y in 0..h as usize {
            out[y * row * 2..y * row * 2 + row].copy_from_slice(&l[y * row..(y + 1) * row]);
            out[y * row * 2 + row..(y + 1) * row * 2].copy_from_slice(&r[y * row..(y + 1) * row]);
        }
        rustvr_gpu::save_png(path, w * 2, h, &out)
    }

    /// The last submitted eye image as RGBA8 (for pixel checks).
    pub fn eye_pixels(&self, gpu: &Gpu, eye: usize) -> Result<(u32, u32, Vec<u8>)> {
        let Some(eyes) = &self.eyes else { anyhow::bail!("nothing rendered yet") };
        Ok((eyes[eye].width, eyes[eye].height, eyes[eye].read_rgba8(gpu)?))
    }
}

/// A Quest-3-like asymmetric frustum (from the Metro mod's simulated headset).
pub fn sim_fov(eye: usize) -> Fov {
    if eye == 0 {
        Fov { left: -1.30, right: 1.05, up: 1.10, down: -1.20 }
    } else {
        Fov { left: -1.05, right: 1.30, up: 1.10, down: -1.20 }
    }
}

impl Backend for SimBackend {
    fn name(&self) -> &'static str {
        "sim"
    }

    fn recommended_size(&self) -> (u32, u32) {
        self.size
    }

    fn refresh_hz(&self) -> f32 {
        self.hz
    }

    fn wait_frame(&mut self) -> Result<Option<HmdFrame>> {
        let dt = 1.0 / self.hz;
        self.index += 1;
        let t = (self.index - 1) as f64 * dt as f64;
        if self.realtime {
            let due = std::time::Duration::from_secs_f64(t);
            if let Some(wait) = due.checked_sub(self.start.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let input = self.driver.input(self.index, t);
        let mut f = HmdFrame {
            index: self.index,
            display_time_ns: (t * 1e9) as i64,
            dt,
            hmd: pose(input.head_pos, input.head_rot),
            user_present: true,
            ..Default::default()
        };
        for e in 0..2 {
            let side = if e == 0 { -0.5 } else { 0.5 };
            let p = input.head_pos + input.head_rot * Vec3::new(side * self.ipd, 0.0, 0.0);
            f.eyes[e] = pose(p, input.head_rot);
            f.fov[e] = sim_fov(e);
        }
        for h in 0..2 {
            let s = &input.hands[h];
            let vel = match self.prev_hand[h] {
                Some(prev) => (s.pos - prev) / dt,
                None => Vec3::ZERO,
            };
            self.prev_hand[h] = Some(s.pos);
            let mut buttons = s.buttons;
            if s.trigger > 0.85 {
                buttons |= CTRL_TRIGGER_CLICK;
            }
            if s.squeeze > 0.75 {
                buttons |= CTRL_SQUEEZE_CLICK;
            }
            f.hands[h] = Controller {
                grip: pose(s.pos, s.rot),
                aim: pose(s.pos, s.rot),
                vel: vel.to_array(),
                connected: s.connected as u32,
                ang_vel: [0.0; 3],
                buttons,
                trigger: s.trigger,
                squeeze: s.squeeze,
                stick: s.stick.to_array(),
            };
        }
        Ok(Some(f))
    }

    fn eye_targets(&mut self, gpu: &Gpu) -> Result<[EyeTarget; 2]> {
        if self.eyes.is_none() {
            let desc = TexDesc {
                width: self.size.0,
                height: self.size.1,
                format: DXGI_FORMAT_R8G8B8A8_TYPELESS,
                rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
                srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM),
                dsv: None,
                mips: 1,
            };
            self.eyes = Some([Texture::new(gpu, &desc)?, Texture::new(gpu, &desc)?]);
        }
        let eyes = self.eyes.as_ref().unwrap();
        Ok(std::array::from_fn(|i| EyeTarget {
            tex: eyes[i].tex.clone(),
            rtv: eyes[i].rtv.clone().unwrap(),
            width: eyes[i].width,
            height: eyes[i].height,
        }))
    }

    fn submit(&mut self, _gpu: &Gpu, poses: &[Pose; 2], _fov: &[Fov; 2]) -> Result<()> {
        self.submitted += 1;
        self.last_poses = *poses;
        Ok(())
    }

    fn haptic(&mut self, hand: usize, seconds: f32, frequency: f32, amplitude: f32) {
        self.haptics.push((self.index, hand, seconds, frequency, amplitude));
    }

    fn as_sim(&mut self) -> Option<&mut SimBackend> {
        Some(self)
    }
}

pub fn pose(pos: Vec3, rot: Quat) -> Pose {
    Pose { pos: pos.to_array(), valid: 1, rot: rot.normalize().to_array() }
}
