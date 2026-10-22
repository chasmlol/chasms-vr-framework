//! The OpenXR runtime: the functions the loader hands to the app.
//!
//! Enough of OpenXR 1.0 with `XR_KHR_D3D11_enable` for an app to run a full session as it would on
//! a real headset: instance and system, a session that goes IDLE -> READY -> SYNCHRONIZED ->
//! VISIBLE -> FOCUSED, reference and action spaces, actions with suggested bindings (validated
//! against the emulated controllers' real input paths, as a real runtime rejects a bad suggestion),
//! D3D11 swapchains made on the app's own device, frame pacing at the headset's rate, and the
//! submitted projection layers, which captures read back.
//!
//! Every call takes the one global lock; `xrWaitFrame` sleeps outside it.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use glam::{Quat, Vec3};
use openxr_sys as xr;
use openxr_sys::Handle as _;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use crate::capture;
use crate::script::{Control, Pose, Script, sample};

pub const RUNTIME_NAME: &str = "RustVR emulated headset";
const SYSTEM: u64 = 1;

pub(crate) enum SpaceKind {
    Reference(xr::ReferenceSpaceType, xr::Posef),
    Action { action: u64, hand: Option<usize>, offset: xr::Posef },
}

pub(crate) struct ActionDef {
    #[allow(dead_code)]
    pub ty: xr::ActionType,
    pub subpaths: Vec<u64>,
    pub name: String,
}

pub(crate) struct Chain {
    pub images: Vec<ID3D11Texture2D>,
    pub width: u32,
    pub height: u32,
    pub format: i64,
    next: u32,
    pub last_released: Option<u32>,
}

/// One projection view as submitted.
#[derive(Clone, Copy)]
pub(crate) struct Submitted {
    pub pose: xr::Posef,
    pub fov: xr::Fovf,
    pub swapchain: u64,
    pub rect: xr::Rect2Di,
}

pub(crate) struct Rt {
    pub start: Instant,
    pub dir: PathBuf,
    pub script: Script,
    control: Option<Control>,
    control_stamp: Option<std::time::SystemTime>,
    control_checked: Instant,
    last_capture_cmd: Option<String>,
    pub pose: Pose,
    pub focused: bool,
    quit: bool,
    paths: Vec<String>,
    next_handle: u64,
    pub session: Option<u64>,
    session_state: xr::SessionState,
    pub device: Option<ID3D11Device>,
    spaces: HashMap<u64, SpaceKind>,
    action_sets: HashMap<u64, String>,
    pub actions: HashMap<u64, ActionDef>,
    /// Suggested bindings: profile path -> (action, input path).
    bindings: HashMap<u64, Vec<(u64, u64)>>,
    attached: bool,
    pub chains: HashMap<u64, Chain>,
    events: VecDeque<Event>,
    pub frames: u64,
    last_frame: Option<Instant>,
    pub haptics: u64,
    pub last_layers: Vec<Submitted>,
    pub last_layer_count: u32,
    captures_done: usize,
    pending: Vec<String>,
    status_at: Instant,
    fps_count: u64,
    fps_at: Instant,
    pub fps: f32,
    log: Option<std::fs::File>,
}

enum Event {
    State(xr::SessionState),
    ProfileChanged,
}

pub(crate) static RT: Mutex<Option<Rt>> = Mutex::new(None);

fn rt() -> std::sync::MutexGuard<'static, Option<Rt>> {
    let mut g = RT.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(Rt::new());
    }
    g
}

macro_rules! with_rt {
    ($r:ident => $body:expr) => {{
        let mut guard = rt();
        let $r = guard.as_mut().unwrap();
        $body
    }};
}

impl Rt {
    fn new() -> Rt {
        let dir = std::env::var_os("RUSTVR_XRSIM_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("rustvr-xrsim"));
        let _ = std::fs::create_dir_all(dir.join("captures"));
        let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("xrsim.log")).ok();
        let mut script_err = None;
        let script = match std::env::var_os("RUSTVR_XRSIM_SCRIPT") {
            Some(p) => match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| Script::parse(&t)) {
                Ok(s) => s,
                Err(e) => {
                    script_err = Some(format!("script {}: {e}", PathBuf::from(&p).display()));
                    Script::default()
                }
            },
            None => Script::default(),
        };
        let now = Instant::now();
        let mut r = Rt {
            start: now,
            dir,
            script,
            control: None,
            control_stamp: None,
            control_checked: now,
            last_capture_cmd: None,
            pose: Pose::default(),
            focused: true,
            quit: false,
            paths: Vec::new(),
            next_handle: 1,
            session: None,
            session_state: xr::SessionState::UNKNOWN,
            device: None,
            spaces: HashMap::new(),
            action_sets: HashMap::new(),
            actions: HashMap::new(),
            bindings: HashMap::new(),
            attached: false,
            chains: HashMap::new(),
            events: VecDeque::new(),
            frames: 0,
            last_frame: None,
            haptics: 0,
            last_layers: Vec::new(),
            last_layer_count: 0,
            captures_done: 0,
            pending: Vec::new(),
            status_at: now,
            fps_count: 0,
            fps_at: now,
            fps: 0.0,
            log,
        };
        r.log(&format!("--- {RUNTIME_NAME} loaded into pid {} (dir {})", std::process::id(), r.dir.display()));
        if let Some(e) = script_err {
            r.log(&e);
        }
        r
    }

    pub fn log(&mut self, s: &str) {
        use std::io::Write;
        if let Some(f) = &mut self.log {
            let _ = writeln!(f, "{:9.3} {s}", self.start.elapsed().as_secs_f64());
        }
    }

    pub fn now_ns(&self) -> i64 {
        self.start.elapsed().as_nanos() as i64 + 1_000_000_000
    }

    fn handle(&mut self) -> u64 {
        self.next_handle += 1;
        self.next_handle
    }

    fn path(&mut self, s: &str) -> u64 {
        if let Some(i) = self.paths.iter().position(|p| p == s) {
            return i as u64 + 1;
        }
        self.paths.push(s.to_string());
        self.paths.len() as u64
    }

    pub fn path_str(&self, p: u64) -> &str {
        p.checked_sub(1).and_then(|i| self.paths.get(i as usize)).map(String::as_str).unwrap_or("")
    }

    fn set_state(&mut self, s: xr::SessionState) {
        if self.session_state != s {
            self.session_state = s;
            self.events.push_back(Event::State(s));
            self.log(&format!("session -> {s:?}"));
        }
    }

    /// The script's time, the control file and the pose for now. Once a frame.
    fn tick(&mut self) {
        let t = self.start.elapsed().as_secs_f64();
        let mut pose = sample(&self.script, t);
        if self.control_checked.elapsed() > Duration::from_millis(100) {
            self.control_checked = Instant::now();
            let path = self.dir.join("control.toml");
            let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if stamp.is_some() && stamp != self.control_stamp {
                self.control_stamp = stamp;
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Control::parse(&t)) {
                    Ok(c) => {
                        self.log(&format!("control: {c:?}"));
                        if let Some(name) = &c.capture
                            && self.last_capture_cmd.as_ref() != Some(name)
                        {
                            self.pending.push(name.clone());
                            self.last_capture_cmd = Some(name.clone());
                        }
                        if let Some(f) = c.focused {
                            self.set_focus(f);
                        }
                        if c.quit == Some(true) {
                            self.quit = true;
                        }
                        // Live pose values accumulate over every control file seen.
                        let mut merged = self.control.take().unwrap_or_default();
                        merge(&mut merged, &c);
                        self.control = Some(merged);
                    }
                    Err(e) => self.log(&format!("control.toml: {e}")),
                }
            }
        }
        if let Some(c) = &self.control {
            pose.apply(&c.head, &c.left, &c.right);
        }
        self.pose = pose;
        while let Some(c) = self.script.captures.get(self.captures_done)
            && c.t <= t
        {
            self.pending.push(c.name.clone());
            self.captures_done += 1;
        }
        if self.script.duration > 0.0 && t > self.script.duration {
            self.quit = true;
        }
        if self.quit && matches!(self.session_state, xr::SessionState::FOCUSED | xr::SessionState::VISIBLE | xr::SessionState::SYNCHRONIZED) {
            self.set_state(xr::SessionState::STOPPING);
        }
        if self.status_at.elapsed() > Duration::from_secs(1) {
            self.status_at = Instant::now();
            self.write_status();
        }
    }

    fn set_focus(&mut self, f: bool) {
        self.focused = f;
        if matches!(self.session_state, xr::SessionState::FOCUSED | xr::SessionState::VISIBLE) {
            self.set_state(if f { xr::SessionState::FOCUSED } else { xr::SessionState::VISIBLE });
        }
    }

    fn write_status(&self) {
        let layers: Vec<String> = self
            .last_layers
            .iter()
            .map(|l| {
                format!(
                    "{{ fov = [{:.1}, {:.1}, {:.1}, {:.1}], pos = [{:.3}, {:.3}, {:.3}], rect = [{}, {}] }}",
                    l.fov.angle_left.to_degrees(),
                    l.fov.angle_right.to_degrees(),
                    l.fov.angle_up.to_degrees(),
                    l.fov.angle_down.to_degrees(),
                    l.pose.position.x,
                    l.pose.position.y,
                    l.pose.position.z,
                    l.rect.extent.width,
                    l.rect.extent.height
                )
            })
            .collect();
        let text = format!(
            "time = {:.2}\nstate = \"{:?}\"\nfocused = {}\nframes = {}\nfps = {:.1}\nlayers = {}\nhaptics = {}\nattached = {}\nviews = [{}]\n",
            self.start.elapsed().as_secs_f64(),
            self.session_state,
            self.focused,
            self.frames,
            self.fps,
            self.last_layer_count,
            self.haptics,
            self.attached,
            layers.join(", ")
        );
        let _ = std::fs::write(self.dir.join("status.toml"), text);
    }

    /// A space's pose in the stage, and whether it's tracked.
    fn space_pose(&self, s: u64) -> Option<(Vec3, Quat)> {
        match self.spaces.get(&s)? {
            SpaceKind::Reference(ty, off) => {
                let (p, q) = to_glam(off);
                match *ty {
                    xr::ReferenceSpaceType::VIEW => {
                        let r = self.pose.head.rot();
                        Some((self.pose.head.pos + r * p, r * q))
                    }
                    _ => Some((p, q)),
                }
            }
            SpaceKind::Action { action, hand, offset } => {
                let h = (*hand)?;
                if !self.pose.hands[h].connected {
                    return None;
                }
                let input = self.bound_input(*action, h)?;
                let (mut p, mut q) = self.pose.hand_pose(h);
                if input.ends_with("/input/aim/pose") {
                    q *= Quat::from_rotation_x(self.script.aim_pitch.to_radians());
                } else if !input.ends_with("/input/grip/pose") {
                    return None;
                }
                let (op, oq) = to_glam(offset);
                p += q * op;
                q *= oq;
                Some((p, q))
            }
        }
    }

    /// The input `action` is bound to for hand `h` in the emulated controllers' profile.
    pub fn bound_input(&self, action: u64, h: usize) -> Option<&str> {
        let profile = self.paths.iter().position(|p| *p == self.script.profile)? as u64 + 1;
        let prefix = if h == 0 { "/user/hand/left/" } else { "/user/hand/right/" };
        self.bindings.get(&profile)?.iter().find(|(a, b)| *a == action && self.path_str(*b).starts_with(prefix)).map(|(_, b)| self.path_str(*b))
    }

    fn hand_of(&self, subpath: u64) -> Option<usize> {
        match self.path_str(subpath) {
            "/user/hand/left" => Some(0),
            "/user/hand/right" => Some(1),
            _ => None,
        }
    }

    /// Which hands an action state query covers.
    fn hands_for(&self, action: u64, subpath: u64) -> Vec<usize> {
        if subpath != 0 {
            return self.hand_of(subpath).into_iter().collect();
        }
        match self.actions.get(&action) {
            Some(a) if !a.subpaths.is_empty() => a.subpaths.iter().filter_map(|p| self.hand_of(*p)).collect(),
            _ => vec![0, 1],
        }
    }

    /// An input's value for hand `h`: (active, analog value, vector).
    fn input_value(&self, input: &str, h: usize) -> (f32, glam::Vec2) {
        let hd = &self.pose.hands[h];
        let leaf = input.rsplit_once("/input/").map(|(_, l)| l).unwrap_or("");
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        match leaf {
            "trigger/value" => (hd.trigger, glam::Vec2::ZERO),
            "trigger/click" => (b(hd.trigger > 0.85), glam::Vec2::ZERO),
            "squeeze/value" => (hd.squeeze, glam::Vec2::ZERO),
            "squeeze/click" => (b(hd.squeeze > 0.75), glam::Vec2::ZERO),
            "thumbstick" | "trackpad" => (0.0, hd.stick),
            "thumbstick/x" => (hd.stick.x, glam::Vec2::ZERO),
            "thumbstick/y" => (hd.stick.y, glam::Vec2::ZERO),
            "thumbstick/click" | "trackpad/click" => (b(hd.stick_click), glam::Vec2::ZERO),
            "a/click" | "x/click" => (b(hd.a), glam::Vec2::ZERO),
            "b/click" | "y/click" => (b(hd.b), glam::Vec2::ZERO),
            "menu/click" => (b(hd.menu), glam::Vec2::ZERO),
            _ => (0.0, glam::Vec2::ZERO),
        }
    }
}

fn merge(into: &mut Control, c: &Control) {
    macro_rules! m {
        ($k:ident, $($f:ident),*) => { $(if c.$k.$f.is_some() { into.$k.$f = c.$k.$f; })* };
    }
    m!(head, pos, yaw, pitch, roll);
    m!(left, pos, yaw, pitch, roll, trigger, squeeze, stick, a, b, menu, stick_click, connected);
    m!(right, pos, yaw, pitch, roll, trigger, squeeze, stick, a, b, menu, stick_click, connected);
}

/// The input paths the emulated Touch controllers have (a suggestion naming any other is
/// rejected, as a real runtime does).
fn touch_input_valid(path: &str) -> bool {
    let (hand, rest) = if let Some(r) = path.strip_prefix("/user/hand/left/") {
        (0, r)
    } else if let Some(r) = path.strip_prefix("/user/hand/right/") {
        (1, r)
    } else {
        return false;
    };
    let common = [
        "input/squeeze/value",
        "input/trigger/value",
        "input/trigger/touch",
        "input/thumbstick",
        "input/thumbstick/x",
        "input/thumbstick/y",
        "input/thumbstick/click",
        "input/thumbstick/touch",
        "input/thumbrest/touch",
        "input/grip/pose",
        "input/aim/pose",
        "output/haptic",
    ];
    let own: &[&str] = if hand == 0 {
        &["input/x/click", "input/x/touch", "input/y/click", "input/y/touch", "input/menu/click"]
    } else {
        &["input/a/click", "input/a/touch", "input/b/click", "input/b/touch"]
    };
    common.contains(&rest) || own.contains(&rest)
}

pub fn to_glam(p: &xr::Posef) -> (Vec3, Quat) {
    let o = p.orientation;
    let q = Quat::from_xyzw(o.x, o.y, o.z, o.w);
    let q = if q.length_squared() > 1e-8 { q.normalize() } else { Quat::IDENTITY };
    (Vec3::new(p.position.x, p.position.y, p.position.z), q)
}

pub fn to_xr(p: Vec3, q: Quat) -> xr::Posef {
    xr::Posef { orientation: xr::Quaternionf { x: q.x, y: q.y, z: q.z, w: q.w }, position: xr::Vector3f { x: p.x, y: p.y, z: p.z } }
}

/// The headset's field of view for eye `e` (the right eye mirrors the left).
pub fn eye_fov(s: &Script, e: usize) -> xr::Fovf {
    let [l, r, u, d] = s.fov.map(f32::to_radians);
    if e == 0 { xr::Fovf { angle_left: l, angle_right: r, angle_up: u, angle_down: d } } else { xr::Fovf { angle_left: -r, angle_right: -l, angle_up: u, angle_down: d } }
}

/// Eye `e`'s pose in the stage.
pub(crate) fn eye_pose(rt: &Rt, e: usize) -> (Vec3, Quat) {
    let r = rt.pose.head.rot();
    let side = if e == 0 { -0.5 } else { 0.5 };
    (rt.pose.head.pos + r * Vec3::new(side * rt.script.ipd, 0.0, 0.0), r)
}

unsafe fn copy_str(dst: &mut [c_char], s: &str) {
    let n = s.len().min(dst.len() - 1);
    for (i, b) in s.bytes().take(n).enumerate() {
        dst[i] = b as c_char;
    }
    dst[n] = 0;
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

/// The two-call idiom: writes `items` if there's room.
unsafe fn two_call<T: Copy>(cap: u32, count: *mut u32, out: *mut T, items: &[T]) -> xr::Result {
    unsafe {
        if !count.is_null() {
            *count = items.len() as u32;
        }
        if cap == 0 {
            return xr::Result::SUCCESS;
        }
        if (cap as usize) < items.len() {
            return xr::Result::ERROR_SIZE_INSUFFICIENT;
        }
        for (i, it) in items.iter().enumerate() {
            *out.add(i) = *it;
        }
    }
    xr::Result::SUCCESS
}

// ---- the functions -----------------------------------------------------------------------------

pub unsafe extern "system" fn get_instance_proc_addr(_instance: xr::Instance, name: *const c_char, function: *mut Option<xr::pfn::VoidFunction>) -> xr::Result {
    let name = unsafe { cstr(name) };
    macro_rules! table {
        ($($n:literal => $f:expr),* $(,)?) => {
            match name.as_str() {
                $($n => Some(unsafe { std::mem::transmute::<*const (), xr::pfn::VoidFunction>($f as *const ()) }),)*
                _ => None,
            }
        };
    }
    let f = table! {
        "xrGetInstanceProcAddr" => get_instance_proc_addr,
        "xrEnumerateInstanceExtensionProperties" => enumerate_instance_extension_properties,
        "xrEnumerateApiLayerProperties" => enumerate_api_layer_properties,
        "xrCreateInstance" => create_instance,
        "xrDestroyInstance" => destroy_instance,
        "xrGetInstanceProperties" => get_instance_properties,
        "xrPollEvent" => poll_event,
        "xrResultToString" => result_to_string,
        "xrStructureTypeToString" => structure_type_to_string,
        "xrGetSystem" => get_system,
        "xrGetSystemProperties" => get_system_properties,
        "xrEnumerateEnvironmentBlendModes" => enumerate_environment_blend_modes,
        "xrCreateSession" => create_session,
        "xrDestroySession" => destroy_session,
        "xrEnumerateReferenceSpaces" => enumerate_reference_spaces,
        "xrCreateReferenceSpace" => create_reference_space,
        "xrGetReferenceSpaceBoundsRect" => get_reference_space_bounds_rect,
        "xrCreateActionSpace" => create_action_space,
        "xrLocateSpace" => locate_space,
        "xrDestroySpace" => destroy_space,
        "xrEnumerateViewConfigurations" => enumerate_view_configurations,
        "xrGetViewConfigurationProperties" => get_view_configuration_properties,
        "xrEnumerateViewConfigurationViews" => enumerate_view_configuration_views,
        "xrEnumerateSwapchainFormats" => enumerate_swapchain_formats,
        "xrCreateSwapchain" => create_swapchain,
        "xrDestroySwapchain" => destroy_swapchain,
        "xrEnumerateSwapchainImages" => enumerate_swapchain_images,
        "xrAcquireSwapchainImage" => acquire_swapchain_image,
        "xrWaitSwapchainImage" => wait_swapchain_image,
        "xrReleaseSwapchainImage" => release_swapchain_image,
        "xrBeginSession" => begin_session,
        "xrEndSession" => end_session,
        "xrRequestExitSession" => request_exit_session,
        "xrWaitFrame" => wait_frame,
        "xrBeginFrame" => begin_frame,
        "xrEndFrame" => end_frame,
        "xrLocateViews" => locate_views,
        "xrStringToPath" => string_to_path,
        "xrPathToString" => path_to_string,
        "xrCreateActionSet" => create_action_set,
        "xrDestroyActionSet" => destroy_action_set,
        "xrCreateAction" => create_action,
        "xrDestroyAction" => destroy_action,
        "xrSuggestInteractionProfileBindings" => suggest_interaction_profile_bindings,
        "xrAttachSessionActionSets" => attach_session_action_sets,
        "xrGetCurrentInteractionProfile" => get_current_interaction_profile,
        "xrGetActionStateBoolean" => get_action_state_boolean,
        "xrGetActionStateFloat" => get_action_state_float,
        "xrGetActionStateVector2f" => get_action_state_vector2f,
        "xrGetActionStatePose" => get_action_state_pose,
        "xrSyncActions" => sync_actions,
        "xrEnumerateBoundSourcesForAction" => enumerate_bound_sources_for_action,
        "xrGetInputSourceLocalizedName" => get_input_source_localized_name,
        "xrApplyHapticFeedback" => apply_haptic_feedback,
        "xrStopHapticFeedback" => stop_haptic_feedback,
        "xrGetD3D11GraphicsRequirementsKHR" => get_d3d11_graphics_requirements,
    };
    unsafe { *function = f };
    if f.is_some() { xr::Result::SUCCESS } else { xr::Result::ERROR_FUNCTION_UNSUPPORTED }
}

unsafe extern "system" fn enumerate_instance_extension_properties(_layer: *const c_char, cap: u32, count: *mut u32, out: *mut xr::ExtensionProperties) -> xr::Result {
    unsafe {
        *count = 1;
        if cap == 0 {
            return xr::Result::SUCCESS;
        }
        let p = &mut *out;
        copy_str(&mut p.extension_name, "XR_KHR_D3D11_enable");
        p.extension_version = 9;
    }
    xr::Result::SUCCESS
}

unsafe extern "system" fn enumerate_api_layer_properties(_cap: u32, count: *mut u32, _out: *mut xr::ApiLayerProperties) -> xr::Result {
    unsafe { *count = 0 };
    xr::Result::SUCCESS
}

unsafe extern "system" fn create_instance(info: *const xr::InstanceCreateInfo, instance: *mut xr::Instance) -> xr::Result {
    let app = unsafe { CStr::from_ptr((*info).application_info.application_name.as_ptr()) }.to_string_lossy().into_owned();
    with_rt!(r => {
        r.log(&format!("xrCreateInstance by '{app}'"));
        unsafe { *instance = xr::Instance::from_raw(1) };
    });
    xr::Result::SUCCESS
}

unsafe extern "system" fn destroy_instance(_instance: xr::Instance) -> xr::Result {
    with_rt!(r => r.log("xrDestroyInstance"));
    xr::Result::SUCCESS
}

unsafe extern "system" fn get_instance_properties(_instance: xr::Instance, p: *mut xr::InstanceProperties) -> xr::Result {
    unsafe {
        (*p).runtime_version = xr::Version::new(0, 1, 0);
        copy_str(&mut (*p).runtime_name, RUNTIME_NAME);
    }
    xr::Result::SUCCESS
}

unsafe extern "system" fn poll_event(_instance: xr::Instance, buf: *mut xr::EventDataBuffer) -> xr::Result {
    with_rt!(r => {
        let Some(e) = r.events.pop_front() else { return xr::Result::EVENT_UNAVAILABLE };
        let session = xr::Session::from_raw(r.session.unwrap_or(0));
        let time = xr::Time::from_nanos(r.now_ns());
        unsafe {
            match e {
                Event::State(state) => {
                    let ev = buf as *mut xr::EventDataSessionStateChanged;
                    *ev = xr::EventDataSessionStateChanged { ty: xr::StructureType::EVENT_DATA_SESSION_STATE_CHANGED, next: std::ptr::null(), session, state, time };
                }
                Event::ProfileChanged => {
                    let ev = buf as *mut xr::EventDataInteractionProfileChanged;
                    *ev = xr::EventDataInteractionProfileChanged { ty: xr::StructureType::EVENT_DATA_INTERACTION_PROFILE_CHANGED, next: std::ptr::null(), session };
                }
            }
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn result_to_string(_instance: xr::Instance, value: xr::Result, buf: *mut c_char) -> xr::Result {
    let s = format!("{value:?}");
    unsafe { copy_str(std::slice::from_raw_parts_mut(buf, xr::MAX_RESULT_STRING_SIZE), &s) };
    xr::Result::SUCCESS
}

unsafe extern "system" fn structure_type_to_string(_instance: xr::Instance, value: xr::StructureType, buf: *mut c_char) -> xr::Result {
    let s = format!("{value:?}");
    unsafe { copy_str(std::slice::from_raw_parts_mut(buf, xr::MAX_STRUCTURE_NAME_SIZE), &s) };
    xr::Result::SUCCESS
}

unsafe extern "system" fn get_system(_instance: xr::Instance, info: *const xr::SystemGetInfo, id: *mut xr::SystemId) -> xr::Result {
    if unsafe { (*info).form_factor } != xr::FormFactor::HEAD_MOUNTED_DISPLAY {
        return xr::Result::ERROR_FORM_FACTOR_UNSUPPORTED;
    }
    unsafe { *id = xr::SystemId::from_raw(SYSTEM) };
    xr::Result::SUCCESS
}

unsafe extern "system" fn get_system_properties(_instance: xr::Instance, _id: xr::SystemId, p: *mut xr::SystemProperties) -> xr::Result {
    unsafe {
        (*p).system_id = xr::SystemId::from_raw(SYSTEM);
        (*p).vendor_id = 0x5256;
        copy_str(&mut (*p).system_name, RUNTIME_NAME);
        (*p).graphics_properties = xr::SystemGraphicsProperties { max_swapchain_image_height: 8192, max_swapchain_image_width: 8192, max_layer_count: 16 };
        (*p).tracking_properties = xr::SystemTrackingProperties { orientation_tracking: xr::TRUE, position_tracking: xr::TRUE };
    }
    xr::Result::SUCCESS
}

unsafe extern "system" fn enumerate_environment_blend_modes(_i: xr::Instance, _s: xr::SystemId, _v: xr::ViewConfigurationType, cap: u32, count: *mut u32, out: *mut xr::EnvironmentBlendMode) -> xr::Result {
    unsafe { two_call(cap, count, out, &[xr::EnvironmentBlendMode::OPAQUE]) }
}

unsafe extern "system" fn create_session(_instance: xr::Instance, info: *const xr::SessionCreateInfo, session: *mut xr::Session) -> xr::Result {
    // The graphics binding is on the `next` chain.
    let mut device = None;
    let mut p = unsafe { (*info).next } as *const xr::BaseInStructure;
    while !p.is_null() {
        unsafe {
            if (*p).ty == xr::StructureType::GRAPHICS_BINDING_D3D11_KHR {
                let b = p as *const xr::GraphicsBindingD3D11KHR;
                let raw = (*b).device as *mut c_void;
                device = ID3D11Device::from_raw_borrowed(&raw).cloned();
            }
            p = (*p).next;
        }
    }
    with_rt!(r => {
        let Some(device) = device else {
            r.log("xrCreateSession without a D3D11 binding");
            return xr::Result::ERROR_GRAPHICS_DEVICE_INVALID;
        };
        let h = r.handle();
        r.session = Some(h);
        r.device = Some(device);
        r.log("xrCreateSession (D3D11)");
        r.set_state(xr::SessionState::IDLE);
        r.set_state(xr::SessionState::READY);
        unsafe { *session = xr::Session::from_raw(h) };
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn destroy_session(_session: xr::Session) -> xr::Result {
    with_rt!(r => {
        r.log("xrDestroySession");
        r.session = None;
        r.chains.clear();
        r.device = None;
    });
    xr::Result::SUCCESS
}

unsafe extern "system" fn enumerate_reference_spaces(_s: xr::Session, cap: u32, count: *mut u32, out: *mut xr::ReferenceSpaceType) -> xr::Result {
    unsafe { two_call(cap, count, out, &[xr::ReferenceSpaceType::VIEW, xr::ReferenceSpaceType::LOCAL, xr::ReferenceSpaceType::STAGE]) }
}

unsafe extern "system" fn create_reference_space(_s: xr::Session, info: *const xr::ReferenceSpaceCreateInfo, space: *mut xr::Space) -> xr::Result {
    with_rt!(r => {
        let h = r.handle();
        let (ty, pose) = unsafe { ((*info).reference_space_type, (*info).pose_in_reference_space) };
        r.log(&format!("xrCreateReferenceSpace {ty:?}"));
        r.spaces.insert(h, SpaceKind::Reference(ty, pose));
        unsafe { *space = xr::Space::from_raw(h) };
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn get_reference_space_bounds_rect(_s: xr::Session, _t: xr::ReferenceSpaceType, bounds: *mut xr::Extent2Df) -> xr::Result {
    unsafe { *bounds = xr::Extent2Df { width: 2.5, height: 2.5 } };
    xr::Result::SUCCESS
}

unsafe extern "system" fn create_action_space(_s: xr::Session, info: *const xr::ActionSpaceCreateInfo, space: *mut xr::Space) -> xr::Result {
    with_rt!(r => {
        let (action, sub, offset) = unsafe { ((*info).action.into_raw(), (*info).subaction_path.into_raw(), (*info).pose_in_action_space) };
        let hand = r.hand_of(sub);
        let h = r.handle();
        r.spaces.insert(h, SpaceKind::Action { action, hand, offset });
        unsafe { *space = xr::Space::from_raw(h) };
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn locate_space(space: xr::Space, base: xr::Space, _time: xr::Time, loc: *mut xr::SpaceLocation) -> xr::Result {
    with_rt!(r => {
        let (a, b) = (r.space_pose(space.into_raw()), r.space_pose(base.into_raw()));
        unsafe {
            match (a, b) {
                (Some((pa, qa)), Some((pb, qb))) => {
                    let inv = qb.inverse();
                    (*loc).pose = to_xr(inv * (pa - pb), (inv * qa).normalize());
                    (*loc).location_flags = xr::SpaceLocationFlags::POSITION_VALID
                        | xr::SpaceLocationFlags::ORIENTATION_VALID
                        | xr::SpaceLocationFlags::POSITION_TRACKED
                        | xr::SpaceLocationFlags::ORIENTATION_TRACKED;
                }
                _ => (*loc).location_flags = xr::SpaceLocationFlags::EMPTY,
            }
            // Velocities on the chain: still (the script moves in steps the app sees frame to frame).
            let mut n = (*loc).next as *mut xr::BaseOutStructure;
            while !n.is_null() {
                if (*n).ty == xr::StructureType::SPACE_VELOCITY {
                    let v = n as *mut xr::SpaceVelocity;
                    (*v).velocity_flags = xr::SpaceVelocityFlags::LINEAR_VALID | xr::SpaceVelocityFlags::ANGULAR_VALID;
                    (*v).linear_velocity = xr::Vector3f { x: 0.0, y: 0.0, z: 0.0 };
                    (*v).angular_velocity = xr::Vector3f { x: 0.0, y: 0.0, z: 0.0 };
                }
                n = (*n).next;
            }
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn destroy_space(space: xr::Space) -> xr::Result {
    with_rt!(r => r.spaces.remove(&space.into_raw()));
    xr::Result::SUCCESS
}

unsafe extern "system" fn enumerate_view_configurations(_i: xr::Instance, _s: xr::SystemId, cap: u32, count: *mut u32, out: *mut xr::ViewConfigurationType) -> xr::Result {
    unsafe { two_call(cap, count, out, &[xr::ViewConfigurationType::PRIMARY_STEREO]) }
}

unsafe extern "system" fn get_view_configuration_properties(_i: xr::Instance, _s: xr::SystemId, ty: xr::ViewConfigurationType, p: *mut xr::ViewConfigurationProperties) -> xr::Result {
    unsafe {
        (*p).view_configuration_type = ty;
        (*p).fov_mutable = xr::TRUE;
    }
    xr::Result::SUCCESS
}

unsafe extern "system" fn enumerate_view_configuration_views(_i: xr::Instance, _s: xr::SystemId, _t: xr::ViewConfigurationType, cap: u32, count: *mut u32, out: *mut xr::ViewConfigurationView) -> xr::Result {
    let [w, h] = with_rt!(r => r.script.eye_size);
    unsafe {
        *count = 2;
        if cap == 0 {
            return xr::Result::SUCCESS;
        }
        if cap < 2 {
            return xr::Result::ERROR_SIZE_INSUFFICIENT;
        }
        for e in 0..2 {
            let v = &mut *out.add(e);
            v.recommended_image_rect_width = w;
            v.recommended_image_rect_height = h;
            v.max_image_rect_width = 8192;
            v.max_image_rect_height = 8192;
            v.recommended_swapchain_sample_count = 1;
            v.max_swapchain_sample_count = 4;
        }
    }
    xr::Result::SUCCESS
}

unsafe extern "system" fn enumerate_swapchain_formats(_s: xr::Session, cap: u32, count: *mut u32, out: *mut i64) -> xr::Result {
    let formats = [DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_D32_FLOAT].map(|f| f.0 as i64);
    unsafe { two_call(cap, count, out, &formats) }
}

unsafe extern "system" fn create_swapchain(_s: xr::Session, info: *const xr::SwapchainCreateInfo, sc: *mut xr::Swapchain) -> xr::Result {
    let i = unsafe { &*info };
    with_rt!(r => {
        let Some(device) = r.device.clone() else { return xr::Result::ERROR_SESSION_LOST };
        let depth = i.usage_flags.contains(xr::SwapchainUsageFlags::DEPTH_STENCIL_ATTACHMENT);
        let mut bind = 0;
        if i.usage_flags.contains(xr::SwapchainUsageFlags::COLOR_ATTACHMENT) {
            bind |= D3D11_BIND_RENDER_TARGET.0;
        }
        if depth {
            bind |= D3D11_BIND_DEPTH_STENCIL.0;
        }
        if i.usage_flags.contains(xr::SwapchainUsageFlags::SAMPLED) && !depth {
            bind |= D3D11_BIND_SHADER_RESOURCE.0;
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: i.width,
            Height: i.height,
            MipLevels: i.mip_count.max(1),
            ArraySize: i.array_size.max(1),
            Format: DXGI_FORMAT(i.format as i32),
            SampleDesc: DXGI_SAMPLE_DESC { Count: i.sample_count.max(1), Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut images = Vec::new();
        for _ in 0..3 {
            let mut t = None;
            if let Err(e) = unsafe { device.CreateTexture2D(&desc, None, Some(&mut t)) } {
                r.log(&format!("xrCreateSwapchain {}x{} format {}: {e}", i.width, i.height, i.format));
                return xr::Result::ERROR_SWAPCHAIN_FORMAT_UNSUPPORTED;
            }
            images.push(t.unwrap());
        }
        let h = r.handle();
        r.log(&format!("xrCreateSwapchain {}x{} format {} usage {:?}", i.width, i.height, i.format, i.usage_flags));
        r.chains.insert(h, Chain { images, width: i.width, height: i.height, format: i.format, next: 0, last_released: None });
        unsafe { *sc = xr::Swapchain::from_raw(h) };
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn destroy_swapchain(sc: xr::Swapchain) -> xr::Result {
    with_rt!(r => r.chains.remove(&sc.into_raw()));
    xr::Result::SUCCESS
}

unsafe extern "system" fn enumerate_swapchain_images(sc: xr::Swapchain, cap: u32, count: *mut u32, out: *mut xr::SwapchainImageBaseHeader) -> xr::Result {
    with_rt!(r => {
        let Some(c) = r.chains.get(&sc.into_raw()) else { return xr::Result::ERROR_HANDLE_INVALID };
        unsafe {
            *count = c.images.len() as u32;
            if cap == 0 {
                return xr::Result::SUCCESS;
            }
            if (cap as usize) < c.images.len() {
                return xr::Result::ERROR_SIZE_INSUFFICIENT;
            }
            let out = out as *mut xr::SwapchainImageD3D11KHR;
            for (i, t) in c.images.iter().enumerate() {
                (*out.add(i)).texture = t.as_raw() as *mut _;
            }
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn acquire_swapchain_image(sc: xr::Swapchain, _info: *const xr::SwapchainImageAcquireInfo, index: *mut u32) -> xr::Result {
    with_rt!(r => {
        let Some(c) = r.chains.get_mut(&sc.into_raw()) else { return xr::Result::ERROR_HANDLE_INVALID };
        let i = c.next;
        c.next = (c.next + 1) % c.images.len() as u32;
        unsafe { *index = i };
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn wait_swapchain_image(_sc: xr::Swapchain, _info: *const xr::SwapchainImageWaitInfo) -> xr::Result {
    xr::Result::SUCCESS
}

unsafe extern "system" fn release_swapchain_image(sc: xr::Swapchain, _info: *const xr::SwapchainImageReleaseInfo) -> xr::Result {
    with_rt!(r => {
        let Some(c) = r.chains.get_mut(&sc.into_raw()) else { return xr::Result::ERROR_HANDLE_INVALID };
        let n = c.images.len() as u32;
        c.last_released = Some((c.next + n - 1) % n);
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn begin_session(_s: xr::Session, _info: *const xr::SessionBeginInfo) -> xr::Result {
    with_rt!(r => {
        r.log("xrBeginSession");
        r.set_state(xr::SessionState::SYNCHRONIZED);
        r.set_state(xr::SessionState::VISIBLE);
        if r.focused {
            r.set_state(xr::SessionState::FOCUSED);
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn end_session(_s: xr::Session) -> xr::Result {
    with_rt!(r => {
        r.log("xrEndSession");
        r.set_state(xr::SessionState::IDLE);
        r.set_state(xr::SessionState::EXITING);
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn request_exit_session(_s: xr::Session) -> xr::Result {
    with_rt!(r => {
        r.log("xrRequestExitSession");
        r.set_state(xr::SessionState::STOPPING);
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn wait_frame(_s: xr::Session, _info: *const xr::FrameWaitInfo, state: *mut xr::FrameState) -> xr::Result {
    // Pace to the headset's refresh outside the lock.
    let (period, last) = with_rt!(r => (Duration::from_secs_f64(1.0 / r.script.refresh.max(1.0) as f64), r.last_frame));
    if let Some(last) = last {
        let due = last + period;
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
    }
    with_rt!(r => {
        let now = Instant::now();
        r.last_frame = Some(match r.last_frame {
            Some(l) if now.duration_since(l) < period * 2 => l + period,
            _ => now,
        });
        r.tick();
        r.frames += 1;
        r.fps_count += 1;
        if r.fps_at.elapsed() > Duration::from_secs(1) {
            r.fps = r.fps_count as f32 / r.fps_at.elapsed().as_secs_f32();
            r.fps_count = 0;
            r.fps_at = Instant::now();
        }
        let render = matches!(r.session_state, xr::SessionState::VISIBLE | xr::SessionState::FOCUSED);
        unsafe {
            (*state).predicted_display_time = xr::Time::from_nanos(r.now_ns() + period.as_nanos() as i64);
            (*state).predicted_display_period = xr::Duration::from_nanos(period.as_nanos() as i64);
            (*state).should_render = if render { xr::TRUE } else { xr::FALSE };
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn begin_frame(_s: xr::Session, _info: *const xr::FrameBeginInfo) -> xr::Result {
    xr::Result::SUCCESS
}

unsafe extern "system" fn end_frame(_s: xr::Session, info: *const xr::FrameEndInfo) -> xr::Result {
    let i = unsafe { &*info };
    with_rt!(r => {
        r.last_layer_count = i.layer_count;
        let mut views = Vec::new();
        for l in 0..i.layer_count as usize {
            let base = unsafe { *i.layers.add(l) };
            if base.is_null() || unsafe { (*base).ty } != xr::StructureType::COMPOSITION_LAYER_PROJECTION {
                continue;
            }
            let p = base as *const xr::CompositionLayerProjection;
            for v in 0..unsafe { (*p).view_count } as usize {
                let pv = unsafe { &*(*p).views.add(v) };
                views.push(Submitted { pose: pv.pose, fov: pv.fov, swapchain: pv.sub_image.swapchain.into_raw(), rect: pv.sub_image.image_rect });
            }
        }
        if !views.is_empty() {
            r.last_layers = views;
        }
        if !r.pending.is_empty() {
            let names = std::mem::take(&mut r.pending);
            for name in names {
                match capture::save(r, &name) {
                    Ok(path) => {
                        let msg = format!("capture {name} -> {}", path.display());
                        r.log(&msg);
                    }
                    Err(e) => {
                        let msg = format!("capture {name}: {e:#}");
                        r.log(&msg);
                    }
                }
            }
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn locate_views(_s: xr::Session, info: *const xr::ViewLocateInfo, state: *mut xr::ViewState, cap: u32, count: *mut u32, out: *mut xr::View) -> xr::Result {
    with_rt!(r => {
        let base = unsafe { (*info).space.into_raw() };
        let (pb, qb) = r.space_pose(base).unwrap_or((Vec3::ZERO, Quat::IDENTITY));
        unsafe {
            (*state).view_state_flags = xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID | xr::ViewStateFlags::POSITION_TRACKED | xr::ViewStateFlags::ORIENTATION_TRACKED;
            *count = 2;
            if cap == 0 {
                return xr::Result::SUCCESS;
            }
            if cap < 2 {
                return xr::Result::ERROR_SIZE_INSUFFICIENT;
            }
            let inv = qb.inverse();
            for e in 0..2 {
                let (p, q) = eye_pose(r, e);
                let v = &mut *out.add(e);
                v.pose = to_xr(inv * (p - pb), (inv * q).normalize());
                v.fov = eye_fov(&r.script, e);
            }
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn string_to_path(_i: xr::Instance, s: *const c_char, path: *mut xr::Path) -> xr::Result {
    let s = unsafe { cstr(s) };
    if !s.starts_with('/') || s.ends_with('/') || s.contains("//") {
        return xr::Result::ERROR_PATH_FORMAT_INVALID;
    }
    with_rt!(r => unsafe { *path = xr::Path::from_raw(r.path(&s)) });
    xr::Result::SUCCESS
}

unsafe extern "system" fn path_to_string(_i: xr::Instance, path: xr::Path, cap: u32, count: *mut u32, buf: *mut c_char) -> xr::Result {
    let s = with_rt!(r => r.path_str(path.into_raw()).to_string());
    let bytes: Vec<c_char> = s.bytes().map(|b| b as c_char).chain(std::iter::once(0)).collect();
    unsafe { two_call(cap, count, buf, &bytes) }
}

unsafe extern "system" fn create_action_set(_i: xr::Instance, info: *const xr::ActionSetCreateInfo, set: *mut xr::ActionSet) -> xr::Result {
    let name = unsafe { CStr::from_ptr((*info).action_set_name.as_ptr()) }.to_string_lossy().into_owned();
    with_rt!(r => {
        let h = r.handle();
        r.action_sets.insert(h, name);
        unsafe { *set = xr::ActionSet::from_raw(h) };
    });
    xr::Result::SUCCESS
}

unsafe extern "system" fn destroy_action_set(set: xr::ActionSet) -> xr::Result {
    with_rt!(r => r.action_sets.remove(&set.into_raw()));
    xr::Result::SUCCESS
}

unsafe extern "system" fn create_action(_set: xr::ActionSet, info: *const xr::ActionCreateInfo, action: *mut xr::Action) -> xr::Result {
    let i = unsafe { &*info };
    let name = unsafe { CStr::from_ptr(i.action_name.as_ptr()) }.to_string_lossy().into_owned();
    let subpaths: Vec<u64> = (0..i.count_subaction_paths as usize).map(|k| unsafe { (*i.subaction_paths.add(k)).into_raw() }).collect();
    with_rt!(r => {
        let h = r.handle();
        r.actions.insert(h, ActionDef { ty: i.action_type, subpaths, name });
        unsafe { *action = xr::Action::from_raw(h) };
    });
    xr::Result::SUCCESS
}

unsafe extern "system" fn destroy_action(action: xr::Action) -> xr::Result {
    with_rt!(r => r.actions.remove(&action.into_raw()));
    xr::Result::SUCCESS
}

unsafe extern "system" fn suggest_interaction_profile_bindings(_i: xr::Instance, s: *const xr::InteractionProfileSuggestedBinding) -> xr::Result {
    let s = unsafe { &*s };
    let list: Vec<(u64, u64)> = (0..s.count_suggested_bindings as usize)
        .map(|k| unsafe {
            let b = &*s.suggested_bindings.add(k);
            (b.action.into_raw(), b.binding.into_raw())
        })
        .collect();
    with_rt!(r => {
        let profile = r.path_str(s.interaction_profile.into_raw()).to_string();
        if profile == r.script.profile && profile.ends_with("oculus/touch_controller") {
            for (_, b) in &list {
                let p = r.path_str(*b).to_string();
                if !touch_input_valid(&p) {
                    r.log(&format!("xrSuggestInteractionProfileBindings {profile}: {p} isn't an input of the controllers: the whole suggestion is rejected"));
                    return xr::Result::ERROR_PATH_UNSUPPORTED;
                }
            }
        }
        let names: Vec<String> = list.iter().map(|(a, b)| format!("{} <- {}", r.actions.get(a).map(|d| d.name.as_str()).unwrap_or("?"), r.path_str(*b))).collect();
        r.log(&format!("bindings for {profile}: {}", names.join(", ")));
        r.bindings.insert(s.interaction_profile.into_raw(), list);
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn attach_session_action_sets(_s: xr::Session, _info: *const xr::SessionActionSetsAttachInfo) -> xr::Result {
    with_rt!(r => {
        r.attached = true;
        r.events.push_back(Event::ProfileChanged);
        let has = r.paths.iter().position(|p| *p == r.script.profile).is_some_and(|i| r.bindings.contains_key(&(i as u64 + 1)));
        let msg = format!("xrAttachSessionActionSets: the controllers ({}) {}", r.script.profile, if has { "have bindings" } else { "have NO bindings: every action is inactive" });
        r.log(&msg);
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn get_current_interaction_profile(_s: xr::Session, _user: xr::Path, state: *mut xr::InteractionProfileState) -> xr::Result {
    with_rt!(r => {
        let p = r.paths.iter().position(|p| *p == r.script.profile).map(|i| i as u64 + 1).unwrap_or(0);
        unsafe { (*state).interaction_profile = xr::Path::from_raw(p) };
        xr::Result::SUCCESS
    })
}

/// (active, value, vector) for an action over the hands it covers.
fn action_value(r: &Rt, action: u64, sub: u64) -> (bool, f32, glam::Vec2) {
    let mut active = false;
    let (mut v, mut vec) = (0f32, glam::Vec2::ZERO);
    for h in r.hands_for(action, sub) {
        if !r.pose.hands[h].connected {
            continue;
        }
        let Some(input) = r.bound_input(action, h).map(str::to_string) else { continue };
        active = true;
        let (x, xy) = r.input_value(&input, h);
        if x.abs() > v.abs() {
            v = x;
        }
        if xy.length() > vec.length() {
            vec = xy;
        }
    }
    (active && r.focused, v, vec)
}

unsafe extern "system" fn get_action_state_boolean(_s: xr::Session, info: *const xr::ActionStateGetInfo, st: *mut xr::ActionStateBoolean) -> xr::Result {
    with_rt!(r => {
        let (a, v, _) = action_value(r, unsafe { (*info).action.into_raw() }, unsafe { (*info).subaction_path.into_raw() });
        unsafe {
            (*st).is_active = if a { xr::TRUE } else { xr::FALSE };
            (*st).current_state = if a && v > 0.5 { xr::TRUE } else { xr::FALSE };
            (*st).changed_since_last_sync = xr::FALSE;
            (*st).last_change_time = xr::Time::from_nanos(r.now_ns());
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn get_action_state_float(_s: xr::Session, info: *const xr::ActionStateGetInfo, st: *mut xr::ActionStateFloat) -> xr::Result {
    with_rt!(r => {
        let (a, v, _) = action_value(r, unsafe { (*info).action.into_raw() }, unsafe { (*info).subaction_path.into_raw() });
        unsafe {
            (*st).is_active = if a { xr::TRUE } else { xr::FALSE };
            (*st).current_state = if a { v } else { 0.0 };
            (*st).changed_since_last_sync = xr::FALSE;
            (*st).last_change_time = xr::Time::from_nanos(r.now_ns());
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn get_action_state_vector2f(_s: xr::Session, info: *const xr::ActionStateGetInfo, st: *mut xr::ActionStateVector2f) -> xr::Result {
    with_rt!(r => {
        let (a, _, v) = action_value(r, unsafe { (*info).action.into_raw() }, unsafe { (*info).subaction_path.into_raw() });
        unsafe {
            (*st).is_active = if a { xr::TRUE } else { xr::FALSE };
            (*st).current_state = if a { xr::Vector2f { x: v.x, y: v.y } } else { xr::Vector2f { x: 0.0, y: 0.0 } };
            (*st).changed_since_last_sync = xr::FALSE;
            (*st).last_change_time = xr::Time::from_nanos(r.now_ns());
        }
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn get_action_state_pose(_s: xr::Session, info: *const xr::ActionStateGetInfo, st: *mut xr::ActionStatePose) -> xr::Result {
    with_rt!(r => {
        let (a, _, _) = action_value(r, unsafe { (*info).action.into_raw() }, unsafe { (*info).subaction_path.into_raw() });
        unsafe { (*st).is_active = if a { xr::TRUE } else { xr::FALSE } };
        xr::Result::SUCCESS
    })
}

unsafe extern "system" fn sync_actions(_s: xr::Session, _info: *const xr::ActionsSyncInfo) -> xr::Result {
    with_rt!(r => {
        if !r.attached {
            return xr::Result::ERROR_ACTIONSET_NOT_ATTACHED;
        }
        if !r.focused { xr::Result::SESSION_NOT_FOCUSED } else { xr::Result::SUCCESS }
    })
}

unsafe extern "system" fn enumerate_bound_sources_for_action(_s: xr::Session, info: *const xr::BoundSourcesForActionEnumerateInfo, cap: u32, count: *mut u32, out: *mut xr::Path) -> xr::Result {
    let action = unsafe { (*info).action.into_raw() };
    let paths: Vec<xr::Path> = with_rt!(r => {
        let mut v = Vec::new();
        for h in 0..2 {
            if let Some(i) = r.bound_input(action, h).map(str::to_string) {
                v.push(xr::Path::from_raw(r.path(&i)));
            }
        }
        v
    });
    unsafe { two_call(cap, count, out, &paths) }
}

unsafe extern "system" fn get_input_source_localized_name(_s: xr::Session, _info: *const xr::InputSourceLocalizedNameGetInfo, cap: u32, count: *mut u32, buf: *mut c_char) -> xr::Result {
    let bytes: Vec<c_char> = b"Emulated controller\0".iter().map(|b| *b as c_char).collect();
    unsafe { two_call(cap, count, buf, &bytes) }
}

unsafe extern "system" fn apply_haptic_feedback(_s: xr::Session, _info: *const xr::HapticActionInfo, _f: *const xr::HapticBaseHeader) -> xr::Result {
    with_rt!(r => r.haptics += 1);
    xr::Result::SUCCESS
}

unsafe extern "system" fn stop_haptic_feedback(_s: xr::Session, _info: *const xr::HapticActionInfo) -> xr::Result {
    xr::Result::SUCCESS
}

unsafe extern "system" fn get_d3d11_graphics_requirements(_i: xr::Instance, _s: xr::SystemId, req: *mut xr::GraphicsRequirementsD3D11KHR) -> xr::Result {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    // The first hardware adapter, as a desktop headset's runtime picks the GPU the headset is on.
    let luid = unsafe {
        CreateDXGIFactory1::<IDXGIFactory1>().ok().and_then(|f| f.EnumAdapters1(0).ok()).and_then(|a| a.GetDesc1().ok()).map(|d| d.AdapterLuid)
    };
    let Some(luid) = luid else { return xr::Result::ERROR_SYSTEM_INVALID };
    unsafe {
        let r = &mut *req;
        std::ptr::write_unaligned(std::ptr::addr_of_mut!(r.adapter_luid) as *mut windows::Win32::Foundation::LUID, luid);
        std::ptr::write_unaligned(std::ptr::addr_of_mut!(r.min_feature_level) as *mut i32, 0xb000); // D3D_FEATURE_LEVEL_11_0
    }
    xr::Result::SUCCESS
}
