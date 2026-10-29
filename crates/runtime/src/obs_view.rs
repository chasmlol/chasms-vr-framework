//! OBS in the headset: the window of whatever OBS is running on this PC (its preview, the audio
//! mixer with the mic's meters, the recording and streaming status), shown just as it is, at the
//! rate it draws, on a panel only the headset sees: never the desktop view, never the recording.
//!
//! It comes up over the back of the left wrist when the player looks at it (the left hand empty),
//! or pinned in front of them with F9 (F9 again puts it away). The window is captured with Windows
//! Graphics Capture: straight from the compositor on the GPU, covered by other windows or not.
//!
//! OBS minimised to the tray isn't drawing its window at all: the panel brings it out the way a
//! click on its tray icon does (behind everything else), and puts it back in the tray once the
//! panel's been away a moment. Minimised to the taskbar, likewise.

use anyhow::{Context, Result};
use glam::{Quat, Vec3};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F9};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Interface, PWSTR};

use crate::panels::Panel;
use rustvr_gpu::{Gpu, TexDesc, Texture};

/// The panel over the wrist: this wide (m), this far off the back of the hand towards the eyes.
const WRIST_WIDTH: f32 = 0.26;
const WRIST_OFF: f32 = 0.07;
/// Looking at the wrist: within this of straight ahead (cos), the back of the hand turned to the
/// eyes this much (cos), the hand this near (m). To put it away, less than the second pair.
const GLANCE_ON: (f32, f32) = (0.85, 0.5);
const GLANCE_OFF: (f32, f32) = (0.7, 0.25);
const GLANCE_NEAR: f32 = 0.75;
/// Pinned (F9): this far ahead of the eyes and this far below them (m), this wide.
const PINNED_AHEAD: f32 = 0.8;
const PINNED_BELOW: f32 = 0.1;
const PINNED_WIDTH: f32 = 1.0;
/// Away this long (s), it stops capturing and OBS goes back where it was.
const LINGER: f64 = 1.5;
/// How often (s) to look for OBS while it isn't found; how long (s) to wait for it to come out of
/// the tray before trying again.
const LOOK_EVERY: f64 = 1.0;
const REVEAL_WAIT: f64 = 2.0;

/// Qt's tray icon message (`WM_APP + 101`), and the tray's "selected" notification: what a click on
/// OBS's tray icon sends its window. OBS shows or hides itself on it.
const QT_TRAY_MESSAGE: u32 = WM_APP + 101;
const NIN_SELECT: u32 = WM_USER;

/// How OBS was before the panel brought it out, to put it back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Was {
    Showing,
    InTray,
    Minimised,
}

struct Capture {
    hwnd: HWND,
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    device: IDirect3DDevice,
    size: SizeInt32,
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

#[derive(Default)]
pub struct ObsView {
    capture: Option<Capture>,
    /// OBS's window, and how it was before the panel brought it out.
    window: Option<(HWND, Was)>,
    /// The latest frame of it (its size), sampled by the panel.
    picture: Option<Texture>,
    shown: (u32, u32),
    pinned: Option<Panel>,
    wrist: bool,
    key_was_down: bool,
    /// When the panel was last wanted, and when to look for OBS next.
    wanted_at: f64,
    look_at: f64,
    /// Bringing OBS out of the tray: since when, and the window in front before it (given the
    /// front back once OBS is out).
    revealing: Option<(f64, HWND)>,
    winrt: bool,
}

impl ObsView {
    /// This frame: the head (`head`, its rotation `turn`) and the left hand's grip pose (None when
    /// it holds something), tracking space. Returns where the panel is, if it's up.
    pub fn update(&mut self, gpu: &Gpu, head: Vec3, turn: Quat, left: Option<(Vec3, Quat)>, now: f64) -> Option<Panel> {
        // F9: pinned in front of the eyes, or put away.
        let down = unsafe { GetAsyncKeyState(VK_F9.0 as i32) } as u16 & 0x8000 != 0;
        if down && !self.key_was_down {
            self.pinned = match self.pinned {
                Some(_) => None,
                None => {
                    let fwd = (turn * Vec3::Z * Vec3::new(1.0, 0.0, 1.0)).normalize_or(Vec3::Z);
                    Some(facing(head + fwd * PINNED_AHEAD - Vec3::Y * PINNED_BELOW, head, PINNED_WIDTH, 16.0 / 9.0))
                }
            };
        }
        self.key_was_down = down;
        // Over the wrist while the player looks at the back of it.
        let wrist = left.and_then(|(at, rot)| {
            let back = rot * Vec3::NEG_X;
            let to = at - head;
            let near = to.length() < GLANCE_NEAR;
            let to = to.normalize_or_zero();
            let (looked, turned) = (to.dot(turn * Vec3::Z), back.dot(-to));
            let (look, face) = if self.wrist { GLANCE_OFF } else { GLANCE_ON };
            self.wrist = near && looked > look && turned > face;
            self.wrist.then(|| at + back * WRIST_OFF - to * 0.02)
        });
        if left.is_none() {
            self.wrist = false;
        }
        let place = match (self.pinned, wrist) {
            (Some(p), _) => Some(p),
            (None, Some(c)) => Some(facing(c, head, WRIST_WIDTH, 16.0 / 9.0)),
            (None, None) => None,
        };
        if place.is_some() {
            self.wanted_at = now;
        } else if now - self.wanted_at > LINGER {
            self.stop();
            return None;
        }
        if let Err(e) = self.capture_frame(gpu, now) {
            tracing::warn!("obs view: {e:#}");
            self.capture = None;
        }
        let place = place?;
        // As tall as OBS's window is for its width.
        let (w, h) = self.shown;
        (w > 0 && h > 0).then(|| Panel { height: place.width * h as f32 / w as f32, ..place })
    }

    /// The panel's picture: OBS's window as it last drew.
    pub fn srv(&self) -> Option<&windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView> {
        self.picture.as_ref().and_then(|t| t.srv.as_ref()).filter(|_| self.shown.0 > 0)
    }

    /// Finds OBS (bringing it out of the tray), starts capturing it, and copies its latest frame.
    fn capture_frame(&mut self, gpu: &Gpu, now: f64) -> Result<()> {
        if self.capture.as_ref().is_some_and(|c| !unsafe { IsWindow(Some(c.hwnd)).as_bool() }) {
            self.capture = None;
            self.window = None;
        }
        if self.capture.is_none() {
            if now < self.look_at {
                return Ok(());
            }
            self.look_at = now + LOOK_EVERY;
            let hwnd = match self.window {
                Some((h, _)) if unsafe { IsWindow(Some(h)).as_bool() } => h,
                _ => {
                    let Some(h) = find_obs() else { return Ok(()) };
                    self.window = Some((h, Was::Showing));
                    h
                }
            };
            // In the tray: a click on its icon brings it out (once; it takes a moment to show).
            if unsafe { !IsWindowVisible(hwnd).as_bool() } {
                match self.revealing {
                    None => {
                        let front = unsafe { GetForegroundWindow() };
                        if click_tray(hwnd) {
                            self.window = Some((hwnd, Was::InTray));
                            self.revealing = Some((now, front));
                        }
                    }
                    Some((since, _)) if now - since > REVEAL_WAIT => self.revealing = None,
                    Some(_) => {}
                }
                self.look_at = now + 0.05;
                return Ok(());
            }
            // Minimised: shown again without taking the front.
            if unsafe { IsIconic(hwnd).as_bool() } {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                self.window = Some((hwnd, Was::Minimised));
                self.look_at = now + 0.05;
                return Ok(());
            }
            // Out: behind everything, the player's window given the front back (it takes it as it
            // shows itself).
            if let Some((_, front)) = self.revealing.take() {
                unsafe {
                    let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    if !front.is_invalid() {
                        let _ = SetForegroundWindow(front);
                    }
                }
                tracing::info!("obs view: brought OBS out of the tray");
            }
            if !self.winrt {
                let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
                self.winrt = true;
            }
            self.capture = Some(start(gpu, hwnd)?);
            tracing::info!("obs view: showing OBS's window");
        }
        let Some(c) = self.capture.as_mut() else { return Ok(()) };
        // Only the latest frame matters; any older ones are dropped.
        let mut latest = None;
        while let Ok(frame) = c.pool.TryGetNextFrame() {
            latest = Some(frame);
        }
        let Some(frame) = latest else { return Ok(()) };
        let size = frame.ContentSize()?;
        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let src: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let (w, h) = (size.Width.max(1) as u32, size.Height.max(1) as u32);
        let mut d = Default::default();
        unsafe { src.GetDesc(&mut d) };
        let (w, h) = (w.min(d.Width), h.min(d.Height));
        if self.picture.as_ref().is_none_or(|t| (t.width, t.height) != (w, h)) {
            let desc = TexDesc { width: w, height: h, format: DXGI_FORMAT_B8G8R8A8_TYPELESS, rtv: None, srv: Some(DXGI_FORMAT_B8G8R8A8_UNORM_SRGB), dsv: None, mips: 1 };
            self.picture = Some(Texture::new(gpu, &desc).context("the panel's texture")?);
        }
        let dst = &self.picture.as_ref().unwrap().tex;
        let region = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
        unsafe { gpu.ctx.CopySubresourceRegion(dst, 0, 0, 0, 0, &src, 0, Some(&region)) };
        self.shown = (w, h);
        drop(frame);
        // The window changed size: frames that size from now on.
        if size.Width != c.size.Width || size.Height != c.size.Height {
            c.pool.Recreate(&c.device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
            c.size = size;
        }
        Ok(())
    }

    /// Stops capturing, and puts OBS back as it was (in the tray, minimised).
    fn stop(&mut self) {
        if self.capture.take().is_some() {
            tracing::info!("obs view: put away");
        }
        if let Some((hwnd, was)) = self.window.take() {
            put_back(hwnd, was);
        }
        self.revealing = None;
        self.look_at = 0.0;
    }
}

/// An upright panel at `center` facing `eye`, `width` wide.
fn facing(center: Vec3, eye: Vec3, width: f32, aspect: f32) -> Panel {
    let d = (center - eye).normalize_or(Vec3::Z);
    Panel::facing(center, d.x.atan2(d.z), (-d.y).clamp(-1.0, 1.0).asin(), width, aspect)
}

fn start(gpu: &Gpu, hwnd: HWND) -> Result<Capture> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
    let dxgi: IDXGIDevice = gpu.device.cast()?;
    let device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
    let size = item.Size()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
    let session = pool.CreateCaptureSession(&item)?;
    // (Neither the yellow border round OBS's window nor the mouse: where available.)
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);
    session.StartCapture()?;
    Ok(Capture { hwnd, session, pool, device, size })
}

/// The executable's file name of the process that owns `hwnd`, lower case.
fn process_name(hwnd: HWND) -> Option<(u32, String)> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    unsafe {
        let _ = CloseHandle(process);
    }
    ok.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
    Some((pid, path.rsplit(['\\', '/']).next()?.to_string()))
}

fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn windows_of(f: &mut dyn FnMut(HWND)) {
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        let f = unsafe { &mut *(data.0 as *mut &mut dyn FnMut(HWND)) };
        f(hwnd);
        BOOL(1)
    }
    let mut f = f;
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut f as *mut &mut dyn FnMut(HWND) as isize));
    }
}

/// OBS's main window (shown or not): the largest top-level window of an OBS process titled "OBS".
fn find_obs() -> Option<HWND> {
    let mut best: Option<(HWND, i32)> = None;
    windows_of(&mut |hwnd| {
        if !window_text(hwnd).starts_with("OBS") || !class_name(hwnd).contains("QWindow") {
            return;
        }
        let Some((_, exe)) = process_name(hwnd) else { return };
        if !matches!(exe.as_str(), "obs64.exe" | "obs32.exe" | "obs.exe") {
            return;
        }
        let mut r = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut r) };
        let area = (r.right - r.left) * (r.bottom - r.top);
        if best.is_none_or(|(_, a)| area > a) {
            best = Some((hwnd, area));
        }
    });
    best.map(|(h, _)| h)
}

/// The window OBS's tray icon talks to (Qt's), in OBS's process.
fn tray_window(obs: HWND) -> Option<HWND> {
    let (pid, _) = process_name(obs)?;
    let mut found = None;
    windows_of(&mut |hwnd| {
        if found.is_none() && class_name(hwnd).ends_with("QTrayIconMessageWindowClass") && process_name(hwnd).is_some_and(|(p, _)| p == pid) {
            found = Some(hwnd);
        }
    });
    found
}

/// A click on OBS's tray icon: it shows itself if it was in the tray, or goes back there.
fn click_tray(obs: HWND) -> bool {
    let Some(tray) = tray_window(obs) else { return false };
    unsafe { PostMessageW(Some(tray), QT_TRAY_MESSAGE, WPARAM(0), LPARAM(NIN_SELECT as isize)) }.is_ok()
}

/// Puts OBS back as it was before the panel brought it out.
fn put_back(hwnd: HWND, was: Was) {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return;
        }
        match was {
            Was::InTray if IsWindowVisible(hwnd).as_bool() => {
                click_tray(hwnd);
            }
            Was::Minimised if !IsIconic(hwnd).as_bool() => {
                let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
            }
            _ => {}
        }
    }
}
