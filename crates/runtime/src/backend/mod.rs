//! Headset backends: OpenXR for real headsets, and a simulated headset for headless tests.
//!
//! All poses are in tracking space as the protocol defines it (left-handed metres, X right,
//! Y up, Z forward, floor at Y = 0); each backend converts from its own conventions.

pub mod openxr;
pub mod sim;

use anyhow::Result;
use rustvr_protocol::{Controller, Fov, Pose};
use windows::Win32::Graphics::Direct3D11::{ID3D11RenderTargetView, ID3D11Texture2D};

use rustvr_gpu::Gpu;

/// One headset frame's predicted poses and controller state.
#[derive(Clone, Copy, Debug, Default)]
pub struct HmdFrame {
    pub index: u64,
    pub display_time_ns: i64,
    /// When the frame is shown, in QueryPerformanceCounter ticks (0: not known): the clock the
    /// game's adapter stamps its state with.
    pub display_qpc: i64,
    /// Seconds since the previous frame.
    pub dt: f32,
    pub hmd: Pose,
    pub eyes: [Pose; 2],
    pub fov: [Fov; 2],
    pub hands: [Controller; 2],
    pub user_present: bool,
    /// The system dashboard (or another app) has the headset's focus.
    pub dashboard: bool,
}

/// Where the compositor draws one eye this frame (an sRGB render target).
pub struct EyeTarget {
    pub tex: ID3D11Texture2D,
    pub rtv: ID3D11RenderTargetView,
    pub width: u32,
    pub height: u32,
}

pub trait Backend {
    fn name(&self) -> &'static str;
    /// Recommended per-eye render size.
    fn recommended_size(&self) -> (u32, u32);
    fn refresh_hz(&self) -> f32;
    /// Blocks until the next frame should start; None when nothing should be rendered now
    /// (session not visible yet), Err when the backend is finished (session ended).
    fn wait_frame(&mut self) -> Result<Option<HmdFrame>>;
    /// The two eye images to draw this frame.
    fn eye_targets(&mut self, gpu: &Gpu) -> Result<[EyeTarget; 2]>;
    /// Hands the drawn eyes to the headset, saying which poses they were drawn with.
    fn submit(&mut self, gpu: &Gpu, poses: &[Pose; 2], fov: &[Fov; 2]) -> Result<()>;
    /// Vibrates a controller (0 left, 1 right).
    fn haptic(&mut self, hand: usize, seconds: f32, frequency: f32, amplitude: f32);
    /// The simulated headset (tests), for capturing frames.
    fn as_sim(&mut self) -> Option<&mut sim::SimBackend> {
        None
    }
}
