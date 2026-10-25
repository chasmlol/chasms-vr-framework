//! The desktop view: a window of its own on the monitor, for recording and streaming, the same for
//! every game. Its camera sits between the eyes and is steadied the way a flat game's would be (the
//! approach of Half-Life: Alyx's spectator camera and LIV's smoothed first person): the head's turns
//! eased by a half-life (`smoothing`), never more than `max_lag` behind, and the horizon kept level
//! (`roll_lock`). Its field of view is a flat game's (`fov` across a 16:9 window), well inside the
//! headset's, so the lenses' edges never show.
//!
//! The game's picture is one eye's (the aiming eye, so the sights line up), looked at through the
//! steadied camera; where that eye doesn't reach, the other's (`compositor::Background::
//! Reprojected`). Everything the runtime draws (the body, hands, guns, the laser, panels) is drawn
//! again from the camera itself. In the framework view the window shows that view too.
//!
//! The window never takes focus when it opens. F11 switches it to borderless fullscreen on the
//! monitor it's on and back; it opens sized to fit its monitor.

use glam::{Quat, Vec3};
use rustvr_protocol::{Fov, Pose};

use crate::settings::Spectator as Settings;

/// The steadied camera (tracking space).
#[derive(Clone, Copy, Debug, Default)]
pub struct SteadyCamera {
    rot: Option<Quat>,
}

impl SteadyCamera {
    /// The camera for this frame: at `center` (between the eyes), turning after `head`.
    pub fn update(&mut self, head: Quat, center: Vec3, dt: f32, s: &Settings) -> Pose {
        let head = head.normalize();
        let target = if s.roll_lock { level(head) } else { head };
        let mut rot = match self.rot {
            Some(prev) if s.smoothing > 0.0 && dt > 0.0 => prev.slerp(target, 1.0 - 0.5f32.powf(dt / s.smoothing)),
            _ => target,
        };
        // Never too far behind: on a fast turn it keeps up.
        let max = s.max_lag.max(0.0).to_radians();
        let lag = rot.angle_between(target);
        if lag > max && lag > 1e-6 {
            rot = target.slerp(rot, max / lag);
        }
        if s.roll_lock {
            rot = level(rot);
        }
        self.rot = Some(rot);
        Pose { pos: center.to_array(), valid: 1, rot: rot.to_array() }
    }

    pub fn reset(&mut self) {
        self.rot = None;
    }
}

/// `q` without its roll: the same forward, the horizon level (straight up or down keeps it as is).
pub fn level(q: Quat) -> Quat {
    let f = q * Vec3::Z;
    let flat = (f.x * f.x + f.z * f.z).sqrt();
    if flat < 1e-4 {
        return q;
    }
    let yaw = f.x.atan2(f.z);
    let pitch = (-f.y).atan2(flat);
    (Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)).normalize()
}

/// The window's field of view: `degrees` across, `aspect` wide to high.
pub fn fov(degrees: f32, aspect: f32) -> Fov {
    let x = (degrees.clamp(30.0, 150.0).to_radians() * 0.5).tan();
    let y = x / aspect.max(0.1);
    Fov { left: -x, right: x, up: y, down: -y }
}

/// The middle of the two eyes.
pub fn between(eyes: &[Pose; 2]) -> Vec3 {
    (Vec3::from_array(eyes[0].pos) + Vec3::from_array(eyes[1].pos)) * 0.5
}

#[cfg(windows)]
pub use win::Window;

#[cfg(windows)]
mod win {
    use anyhow::{Context, Result};
    use rustvr_gpu::Gpu;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F11;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{Interface, w};

    use crate::backend::EyeTarget;

    // What the window procedure saw since the last frame (one window, on the runtime's thread).
    static RESIZED: AtomicBool = AtomicBool::new(false);
    static FULLSCREEN: AtomicBool = AtomicBool::new(false);
    static CLOSED: AtomicBool = AtomicBool::new(false);
    static CLIENT_W: AtomicU32 = AtomicU32::new(0);
    static CLIENT_H: AtomicU32 = AtomicU32::new(0);

    extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_SIZE => {
                CLIENT_W.store((lp.0 & 0xFFFF) as u32, Ordering::Relaxed);
                CLIENT_H.store(((lp.0 >> 16) & 0xFFFF) as u32, Ordering::Relaxed);
                RESIZED.store(true, Ordering::Relaxed);
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if wp.0 as u16 == VK_F11.0 => {
                FULLSCREEN.store(true, Ordering::Relaxed);
                LRESULT(0)
            }
            WM_CLOSE => {
                CLOSED.store(true, Ordering::Relaxed);
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    /// The desktop view's window and its swap chain.
    pub struct Window {
        hwnd: HWND,
        swap: IDXGISwapChain1,
        target: Option<EyeTarget>,
        /// The window's place before fullscreen.
        windowed: Option<RECT>,
    }

    impl Window {
        /// Opens the window, `width` x `height` (made to fit its monitor), without taking focus;
        /// `hidden`: off every screen (for tests: its frames are only saved).
        pub fn open(gpu: &Gpu, width: u32, height: u32, hidden: bool) -> Result<Window> {
            unsafe {
                let inst = GetModuleHandleW(None)?;
                let class = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wndproc),
                    hInstance: inst.into(),
                    hCursor: LoadCursorW(None, IDC_ARROW)?,
                    lpszClassName: w!("RustVR.DesktopView"),
                    hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                    ..Default::default()
                };
                RegisterClassW(&class);
                // Fits the primary monitor's work area: at most 90% of it, centred.
                let work = monitor_work(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY));
                let (ww, wh) = ((work.right - work.left) as f32 * 0.9, (work.bottom - work.top) as f32 * 0.9);
                let k = (ww / width.max(1) as f32).min(wh / height.max(1) as f32).min(1.0);
                let (cw, ch) = (((width as f32 * k) as i32).max(320), ((height as f32 * k) as i32).max(180));
                let style = WS_OVERLAPPEDWINDOW;
                let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
                AdjustWindowRect(&mut r, style, false)?;
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                let (x, y) = match hidden {
                    true => (-30000, -30000),
                    false => (work.left + ((work.right - work.left) - w) / 2, (work.top + ((work.bottom - work.top) - h) / 2).max(work.top)),
                };
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("RustVR.DesktopView"),
                    w!("RustVR - desktop view (F11: fullscreen)"),
                    style,
                    x,
                    y,
                    w,
                    h,
                    None,
                    None,
                    Some(inst.into()),
                    None,
                )?;
                let factory: IDXGIFactory2 = gpu.device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
                let desc = DXGI_SWAP_CHAIN_DESC1 {
                    Width: 0,
                    Height: 0,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    Scaling: DXGI_SCALING_STRETCH,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    ..Default::default()
                };
                let swap = factory.CreateSwapChainForHwnd(&gpu.device, hwnd, &desc, None, None).context("the desktop view's swap chain")?;
                factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
                // Shown without taking focus from the game or the headset.
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                CLOSED.store(false, Ordering::Relaxed);
                let mut win = Window { hwnd, swap, target: None, windowed: None };
                win.make_target(gpu)?;
                Ok(win)
            }
        }

        fn make_target(&mut self, gpu: &Gpu) -> Result<()> {
            unsafe {
                let tex: ID3D11Texture2D = self.swap.GetBuffer(0)?;
                let mut d = D3D11_TEXTURE2D_DESC::default();
                tex.GetDesc(&mut d);
                let rd = D3D11_RENDER_TARGET_VIEW_DESC {
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                    ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                };
                let mut rtv = None;
                gpu.device.CreateRenderTargetView(&tex, Some(&rd), Some(&mut rtv))?;
                self.target = Some(EyeTarget { tex, rtv: rtv.context("render target view")?, width: d.Width, height: d.Height });
            }
            Ok(())
        }

        /// The window's messages; false once it's been closed (it's gone then).
        pub fn pump(&mut self, gpu: &Gpu) -> bool {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, Some(self.hwnd), 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if CLOSED.swap(false, Ordering::Relaxed) {
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
                tracing::info!("desktop view: closed");
                return false;
            }
            if FULLSCREEN.swap(false, Ordering::Relaxed) {
                self.toggle_fullscreen();
            }
            if RESIZED.swap(false, Ordering::Relaxed) {
                let (w, h) = (CLIENT_W.load(Ordering::Relaxed), CLIENT_H.load(Ordering::Relaxed));
                if w > 0 && h > 0 && self.target.as_ref().is_none_or(|t| (t.width, t.height) != (w, h)) {
                    self.target = None;
                    unsafe { gpu.ctx.OMSetRenderTargets(None, None) };
                    let ok = unsafe { self.swap.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)) };
                    if let Err(e) = ok.map_err(anyhow::Error::from).and_then(|_| self.make_target(gpu)) {
                        tracing::warn!("desktop view: resizing to {w}x{h}: {e:#}");
                    }
                }
            }
            true
        }

        /// Borderless over the whole monitor it's on, or back to the window it was.
        fn toggle_fullscreen(&mut self) {
            unsafe {
                match self.windowed.take() {
                    Some(r) => {
                        SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_OVERLAPPEDWINDOW | WS_VISIBLE).0 as isize);
                        let _ = SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE);
                    }
                    None => {
                        let mut r = RECT::default();
                        let _ = GetWindowRect(self.hwnd, &mut r);
                        self.windowed = Some(r);
                        let m = monitor_rect(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST));
                        SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
                        let _ = SetWindowPos(self.hwnd, None, m.left, m.top, m.right - m.left, m.bottom - m.top, SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE);
                    }
                }
            }
        }

        /// This frame's target (none while minimised).
        pub fn target(&self) -> Option<&EyeTarget> {
            let minimised = unsafe { IsIconic(self.hwnd).as_bool() };
            self.target.as_ref().filter(|_| !minimised)
        }

        /// Saves this frame (drawn, not yet shown) as a PNG.
        pub fn save(&self, gpu: &Gpu, path: &std::path::Path) -> Result<()> {
            let t = self.target.as_ref().context("no frame")?;
            unsafe {
                let mut d = D3D11_TEXTURE2D_DESC::default();
                t.tex.GetDesc(&mut d);
                d.Usage = D3D11_USAGE_STAGING;
                d.BindFlags = 0;
                d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                d.MiscFlags = 0;
                let mut staging = None;
                gpu.device.CreateTexture2D(&d, None, Some(&mut staging))?;
                let staging = staging.context("staging texture")?;
                gpu.ctx.CopyResource(&staging, &t.tex);
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                gpu.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
                let row = (d.Width * 4) as usize;
                let mut px = vec![0u8; row * d.Height as usize];
                for y in 0..d.Height as usize {
                    std::ptr::copy_nonoverlapping((m.pData as *const u8).add(y * m.RowPitch as usize), px.as_mut_ptr().add(y * row), row);
                }
                gpu.ctx.Unmap(&staging, 0);
                rustvr_gpu::save_png(path, d.Width, d.Height, &px)
            }
        }

        /// Shows the frame; never waits for the monitor (the headset sets the pace).
        pub fn present(&self) {
            unsafe {
                let _ = self.swap.Present(0, DXGI_PRESENT_DO_NOT_WAIT);
            }
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    unsafe fn monitor_work(m: HMONITOR) -> RECT {
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = unsafe { GetMonitorInfoW(m, &mut mi) };
        mi.rcWork
    }

    unsafe fn monitor_rect(m: HMONITOR) -> RECT {
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = unsafe { GetMonitorInfoW(m, &mut mi) };
        mi.rcMonitor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::default()
    }

    #[test]
    fn level_keeps_where_it_looks_and_drops_the_roll() {
        let q = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.3) * Quat::from_rotation_z(0.4);
        let l = level(q);
        assert!((l * Vec3::Z).distance(q * Vec3::Z) < 1e-4, "same forward");
        assert!((l * Vec3::X).y.abs() < 1e-4, "its right stays level");
    }

    #[test]
    fn the_camera_eases_after_the_head_and_keeps_up_on_fast_turns() {
        let s = settings();
        let mut c = SteadyCamera::default();
        c.update(Quat::IDENTITY, Vec3::ZERO, 1.0 / 90.0, &s);
        // A small nod: eased, part of the way after one frame.
        let p = c.update(Quat::from_rotation_y(0.05), Vec3::ZERO, 1.0 / 90.0, &s);
        let a = Quat::from_array(p.rot).angle_between(Quat::IDENTITY);
        assert!(a > 0.001 && a < 0.05, "eased: {a}");
        // A fast turn: never more than `max_lag` behind.
        let p = c.update(Quat::from_rotation_y(1.5), Vec3::ZERO, 1.0 / 90.0, &s);
        let lag = Quat::from_array(p.rot).angle_between(Quat::from_rotation_y(1.5)).to_degrees();
        assert!(lag <= s.max_lag + 0.01, "{lag} degrees behind");
        // Held still, it settles on the head.
        let mut p = p;
        for _ in 0..200 {
            p = c.update(Quat::from_rotation_y(1.5), Vec3::ZERO, 1.0 / 90.0, &s);
        }
        assert!(Quat::from_array(p.rot).angle_between(Quat::from_rotation_y(1.5)) < 1e-3);
    }

    #[test]
    fn a_16_by_9_window_sees_a_flat_games_field_of_view() {
        let f = fov(90.0, 16.0 / 9.0);
        assert!((f.right - 1.0).abs() < 1e-5 && (f.left + 1.0).abs() < 1e-5);
        let vertical = (f.up.atan() * 2.0).to_degrees();
        assert!((vertical - 58.7).abs() < 0.5, "{vertical}");
    }
}
