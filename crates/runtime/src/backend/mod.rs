//! Headset backends: OpenXR for real headsets, and a simulated headset for headless tests.
//!
//! All poses are in tracking space as the protocol defines it (left-handed metres, X right,
//! Y up, Z forward, floor at Y = 0); each backend converts from its own conventions.

pub mod openxr;
pub mod sim;

use anyhow::Result;
use rustvr_protocol::{Controller, Fov, Pose};
use windows::Win32::Graphics::Direct3D11::{ID3D11DepthStencilView, ID3D11RenderTargetView, ID3D11Texture2D};

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
    /// Each hand's fingers as the headset's cameras see them, when it tracks the hand itself (the
    /// controller put down): curls, thumb .. little, 0 straight to 1 closed.
    pub fingers: [Option<[f32; 5]>; 2],
    /// Each elbow where the headset tracks it (a Quest's body tracking: its cameras and its own
    /// body model), tracking space.
    pub elbows: [Option<[f32; 3]>; 2],
    /// Each shoulder joint where the headset tracks it, likewise.
    pub shoulders: [Option<[f32; 3]>; 2],
}

/// Where the head, the eyes and the controllers are at some moment, as the headset predicts it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Located {
    pub hmd: Pose,
    pub eyes: [Pose; 2],
    pub grips: [Pose; 2],
    pub aims: [Pose; 2],
}

/// Where the compositor writes one eye's depth for the headset (its projection's, near
/// [`crate::compositor::NEAR`] at 0 to far [`crate::compositor::FAR`] at 1).
pub struct DepthTarget {
    pub dsv: ID3D11DepthStencilView,
    pub width: u32,
    pub height: u32,
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
    /// Where the head, eyes and controllers will be at `time_ns` (the clock of
    /// [`HmdFrame::display_time_ns`]), as the headset predicts it. None: it can't say.
    fn locate_at(&mut self, _time_ns: i64) -> Option<Located> {
        None
    }
    /// The two eyes' depth images this frame, when the headset takes depth with the picture (its
    /// reprojection then knows how far each pixel is). Asked for after [`Backend::eye_targets`],
    /// written before [`Backend::submit`].
    fn depth_targets(&mut self, _gpu: &Gpu) -> Option<[DepthTarget; 2]> {
        None
    }
    /// The refresh rate the player wants (Hz; 0: the headset's highest). Called every frame; a
    /// backend asks the headset only when it changes.
    fn want_refresh(&mut self, _hz: f32) {}
    /// Hands the drawn eyes to the headset, saying which poses they were drawn with.
    fn submit(&mut self, gpu: &Gpu, poses: &[Pose; 2], fov: &[Fov; 2]) -> Result<()>;
    /// Vibrates a controller (0 left, 1 right).
    fn haptic(&mut self, hand: usize, seconds: f32, frequency: f32, amplitude: f32);
    /// The simulated headset (tests), for capturing frames.
    fn as_sim(&mut self) -> Option<&mut sim::SimBackend> {
        None
    }
}
