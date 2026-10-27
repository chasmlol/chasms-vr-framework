//! Real headsets through OpenXR (SteamVR, Virtual Desktop, Quest Link, WMR, Pimax...) with a D3D11
//! session.
//!
//! OpenXR is right-handed with -Z forward; tracking space here is left-handed with +Z forward, so
//! every pose crossing this boundary has Z mirrored: positions (x, y, -z), quaternions
//! (-x, -y, z, w). FOV angles keep their meaning (left is still -X).

use anyhow::{Context, Result, anyhow, bail};
use openxr as xr;
use rustvr_gpu::Gpu;
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_RENDER_TARGET_VIEW_DESC, D3D11_RENDER_TARGET_VIEW_DESC_0, D3D11_RTV_DIMENSION_TEXTURE2D, D3D11_TEX2D_RTV,
    ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};
use windows::core::Interface;

use super::{Backend, EyeTarget, HmdFrame};

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

struct Hands {
    set: xr::ActionSet,
    grip: xr::Action<xr::Posef>,
    /// Kept alive for its spaces (`aim_space`).
    _aim: xr::Action<xr::Posef>,
    trigger: xr::Action<f32>,
    squeeze: xr::Action<f32>,
    stick: xr::Action<xr::Vector2f>,
    a: xr::Action<bool>,
    b: xr::Action<bool>,
    stick_click: xr::Action<bool>,
    menu: xr::Action<bool>,
    haptic: xr::Action<xr::Haptic>,
    paths: [xr::Path; 2],
    grip_space: [xr::Space; 2],
    aim_space: [xr::Space; 2],
}

struct EyeChain {
    swapchain: xr::Swapchain<xr::D3D11>,
    images: Vec<(ID3D11Texture2D, ID3D11RenderTargetView)>,
    acquired: Option<u32>,
}

pub struct OpenXrBackend {
    _instance: xr::Instance,
    session: xr::Session<xr::D3D11>,
    waiter: xr::FrameWaiter,
    stream: xr::FrameStream<xr::D3D11>,
    stage: xr::Space,
    view: xr::Space,
    hands: Hands,
    chains: Vec<EyeChain>,
    size: (u32, u32),
    refresh: f32,
    running: bool,
    focused: bool,
    frame_state: Option<xr::FrameState>,
    began: bool,
    index: u64,
    last_time: Option<i64>,
    event_buf: xr::EventDataBuffer,
    /// The runtime's time -> QueryPerformanceCounter, when it offers it.
    to_qpc: Option<xr::sys::pfn::ConvertTimeToWin32PerformanceCounterKHR>,
}

impl OpenXrBackend {
    /// Creates the instance, picks the headset's GPU, creates `gpu` on it, and starts a session.
    pub fn new(app_name: &str) -> Result<(OpenXrBackend, Gpu)> {
        let entry = xr::Entry::linked(&()).context("OpenXR loader")?;
        let available = entry.enumerate_extensions().context("OpenXR extensions (is a runtime installed and running?)")?;
        if !available.khr_d3d11_enable {
            bail!("the OpenXR runtime has no D3D11 support");
        }
        let mut exts = xr::ExtensionSet::default();
        exts.khr_d3d11_enable = true;
        exts.khr_win32_convert_performance_counter_time = available.khr_win32_convert_performance_counter_time;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo { application_name: app_name, application_version: 1, engine_name: "RustVR", engine_version: 1, api_version: xr::Version::new(1, 0, 0) },
                &exts,
                &[],
                &(),
            )
            .context("creating the OpenXR instance")?;
        let props = instance.properties()?;
        tracing::info!("openxr: runtime {} {}", props.runtime_name, props.runtime_version);
        let system = instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY).context("no headset found")?;
        let reqs = instance.graphics_requirements::<xr::D3D11>(system)?;
        let luid = ((reqs.adapter_luid.HighPart as i64) << 32) | reqs.adapter_luid.LowPart as i64;
        let gpu = Gpu::new_on_adapter(luid).context("creating the D3D11 device on the headset's GPU")?;

        let views = instance.enumerate_view_configuration_views(system, VIEW_TYPE)?;
        let size = (views[0].recommended_image_rect_width, views[0].recommended_image_rect_height);
        let (session, waiter, stream) = unsafe {
            instance.create_session::<xr::D3D11>(system, &xr::d3d::SessionCreateInfoD3D11 { device: gpu.device.as_raw() as *mut _ })?
        };
        let stage = match session.create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY) {
            Ok(s) => s,
            Err(_) => session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?,
        };
        let view = session.create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)?;
        let hands = create_actions(&instance, &session)?;

        let formats = session.enumerate_swapchain_formats()?;
        let format = [DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB]
            .into_iter()
            .find(|f| formats.contains(&(f.0 as u32)))
            .ok_or_else(|| anyhow!("no sRGB swapchain format among {formats:?}"))?;
        let mut chains = Vec::new();
        for v in &views {
            let swapchain = session.create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::SAMPLED,
                format: format.0 as u32,
                sample_count: 1,
                width: v.recommended_image_rect_width,
                height: v.recommended_image_rect_height,
                face_count: 1,
                array_size: 1,
                mip_count: 1,
            })?;
            let mut images = Vec::new();
            for raw in swapchain.enumerate_images()? {
                let tex = unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }
                    .cloned()
                    .ok_or_else(|| anyhow!("null swapchain image"))?;
                images.push((tex.clone(), rtv(&gpu, &tex, format)?));
            }
            chains.push(EyeChain { swapchain, images, acquired: None });
        }
        let refresh = 90.0;
        tracing::info!("openxr: {}x{} per eye, swapchain format {:?}", size.0, size.1, format);
        let to_qpc = instance.exts().khr_win32_convert_performance_counter_time.map(|e| e.convert_time_to_win32_performance_counter);
        if to_qpc.is_none() {
            tracing::warn!("openxr: the runtime can't say when a frame shows in the PC's clock; guessed a frame ahead");
        }
        Ok((
            OpenXrBackend {
                _instance: instance,
                session,
                waiter,
                stream,
                stage,
                view,
                hands,
                chains,
                size,
                refresh,
                running: false,
                focused: false,
                frame_state: None,
                began: false,
                index: 0,
                last_time: None,
                event_buf: xr::EventDataBuffer::new(),
                to_qpc,
            },
            gpu,
        ))
    }

    /// When a frame shows, in QueryPerformanceCounter ticks: the runtime's own conversion, or (not
    /// offered) a frame period from now.
    fn display_qpc(&self, time: xr::Time, period: xr::Duration) -> i64 {
        if let Some(f) = self.to_qpc {
            let mut q = 0i64;
            if unsafe { f(self._instance.as_raw(), time, &mut q) } == xr::sys::Result::SUCCESS && q > 0 {
                return q;
            }
        }
        use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
        let (mut now, mut freq) = (0i64, 0i64);
        unsafe {
            let _ = QueryPerformanceCounter(&mut now);
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        now + (period.as_nanos() as i128 * freq as i128 / 1_000_000_000) as i64
    }

    /// Handles session state changes. Err when the session is over.
    fn pump_events(&mut self) -> Result<()> {
        let instance = self.session.instance().clone();
        while let Some(event) = instance.poll_event(&mut self.event_buf)? {
            match event {
                xr::Event::SessionStateChanged(e) => {
                    tracing::info!("openxr: session {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            self.session.begin(VIEW_TYPE)?;
                            self.running = true;
                        }
                        xr::SessionState::STOPPING => {
                            self.session.end()?;
                            self.running = false;
                        }
                        xr::SessionState::FOCUSED => self.focused = true,
                        xr::SessionState::VISIBLE => self.focused = false,
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => bail!("session ended ({:?})", e.state()),
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => bail!("the OpenXR runtime is shutting down"),
                _ => {}
            }
        }
        Ok(())
    }

    fn controller(&self, h: usize, time: xr::Time) -> Result<Controller> {
        let hd = &self.hands;
        let p = hd.paths[h];
        let mut c = Controller::default();
        let (grip, grip_vel) = hd.grip_space[h].relate(&self.stage, time)?;
        let aim = hd.aim_space[h].locate(&self.stage, time)?;
        let valid = grip.location_flags.contains(xr::SpaceLocationFlags::ORIENTATION_VALID | xr::SpaceLocationFlags::POSITION_VALID);
        c.connected = (valid && hd.grip.is_active(&self.session, p)?) as u32;
        c.grip = from_xr(&grip.pose, valid);
        c.aim = from_xr(&aim.pose, aim.location_flags.contains(xr::SpaceLocationFlags::ORIENTATION_VALID));
        let v = grip_vel.linear_velocity;
        c.vel = [v.x, v.y, -v.z];
        let w = grip_vel.angular_velocity;
        c.ang_vel = [-w.x, -w.y, w.z];
        c.trigger = hd.trigger.state(&self.session, p)?.current_state;
        c.squeeze = hd.squeeze.state(&self.session, p)?.current_state;
        let s = hd.stick.state(&self.session, p)?.current_state;
        c.stick = [s.x, s.y];
        let mut b = 0;
        let mut bit = |a: &xr::Action<bool>, flag: u32| -> Result<()> {
            if a.state(&self.session, p)?.current_state {
                b |= flag;
            }
            Ok(())
        };
        bit(&hd.a, CTRL_A)?;
        bit(&hd.b, CTRL_B)?;
        bit(&hd.stick_click, CTRL_STICK_CLICK)?;
        bit(&hd.menu, CTRL_MENU)?;
        if c.trigger > 0.85 {
            b |= CTRL_TRIGGER_CLICK;
        }
        if c.squeeze > 0.75 {
            b |= CTRL_SQUEEZE_CLICK;
        }
        c.buttons = b;
        Ok(c)
    }
}

impl Backend for OpenXrBackend {
    fn name(&self) -> &'static str {
        "openxr"
    }

    fn recommended_size(&self) -> (u32, u32) {
        self.size
    }

    fn refresh_hz(&self) -> f32 {
        self.refresh
    }

    fn wait_frame(&mut self) -> Result<Option<HmdFrame>> {
        self.pump_events()?;
        if !self.running {
            std::thread::sleep(std::time::Duration::from_millis(50));
            return Ok(None);
        }
        let state = self.waiter.wait()?;
        self.stream.begin()?;
        let time = state.predicted_display_time;
        if !state.should_render {
            // Every begun frame must be ended, even when there's nothing to show.
            self.stream.end(time, xr::EnvironmentBlendMode::OPAQUE, &[])?;
            return Ok(None);
        }
        self.began = true;
        self.frame_state = Some(state);
        self.session.sync_actions(&[xr::ActiveActionSet::new(&self.hands.set)])?;
        let (flags, views) = self.session.locate_views(VIEW_TYPE, time, &self.stage)?;
        let valid = flags.contains(xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID);
        let head = self.view.locate(&self.stage, time)?;
        let t_ns = time.as_nanos();
        let dt = self.last_time.map(|l| ((t_ns - l) as f32 * 1e-9).clamp(0.0, 0.1)).unwrap_or(1.0 / self.refresh);
        self.last_time = Some(t_ns);
        self.index += 1;
        let mut f = HmdFrame {
            index: self.index,
            display_time_ns: t_ns,
            display_qpc: self.display_qpc(time, state.predicted_display_period),
            dt,
            hmd: from_xr(&head.pose, valid),
            user_present: true,
            dashboard: !self.focused,
            ..Default::default()
        };
        for e in 0..2 {
            f.eyes[e] = from_xr(&views[e].pose, valid);
            let fov = views[e].fov;
            f.fov[e] = Fov { left: fov.angle_left.tan(), right: fov.angle_right.tan(), up: fov.angle_up.tan(), down: fov.angle_down.tan() };
        }
        for h in 0..2 {
            f.hands[h] = self.controller(h, time).unwrap_or_default();
        }
        Ok(Some(f))
    }

    fn eye_targets(&mut self, _gpu: &Gpu) -> Result<[EyeTarget; 2]> {
        let mut out = Vec::new();
        for c in &mut self.chains {
            let i = match c.acquired {
                Some(i) => i,
                None => {
                    let i = c.swapchain.acquire_image()?;
                    c.swapchain.wait_image(xr::Duration::INFINITE)?;
                    c.acquired = Some(i);
                    i
                }
            };
            let (tex, rtv) = &c.images[i as usize];
            out.push(EyeTarget { tex: tex.clone(), rtv: rtv.clone(), width: self.size.0, height: self.size.1 });
        }
        let mut it = out.into_iter();
        Ok([it.next().unwrap(), it.next().unwrap()])
    }

    fn submit(&mut self, _gpu: &Gpu, poses: &[Pose; 2], fov: &[Fov; 2]) -> Result<()> {
        let Some(state) = self.frame_state.take() else { return Ok(()) };
        if !self.began {
            return Ok(());
        }
        self.began = false;
        let mut rendered = false;
        for c in &mut self.chains {
            if c.acquired.take().is_some() {
                c.swapchain.release_image()?;
                rendered = true;
            }
        }
        if !rendered || !state.should_render {
            self.stream.end(state.predicted_display_time, xr::EnvironmentBlendMode::OPAQUE, &[])?;
            return Ok(());
        }
        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di { width: self.size.0 as i32, height: self.size.1 as i32 },
        };
        let views: Vec<xr::CompositionLayerProjectionView<xr::D3D11>> = (0..2)
            .map(|e| {
                xr::CompositionLayerProjectionView::new()
                    .pose(to_xr(&poses[e]))
                    .fov(xr::Fovf {
                        angle_left: fov[e].left.atan(),
                        angle_right: fov[e].right.atan(),
                        angle_up: fov[e].up.atan(),
                        angle_down: fov[e].down.atan(),
                    })
                    .sub_image(xr::SwapchainSubImage::new().swapchain(&self.chains[e].swapchain).image_array_index(0).image_rect(rect))
            })
            .collect();
        let layer = xr::CompositionLayerProjection::new().space(&self.stage).views(&views);
        self.stream.end(state.predicted_display_time, xr::EnvironmentBlendMode::OPAQUE, &[&layer])?;
        Ok(())
    }

    fn haptic(&mut self, hand: usize, seconds: f32, frequency: f32, amplitude: f32) {
        let v = xr::HapticVibration::new()
            .amplitude(amplitude.clamp(0.0, 1.0))
            .frequency(frequency)
            .duration(xr::Duration::from_nanos((seconds.max(0.0) * 1e9) as i64));
        let _ = self.hands.haptic.apply_feedback(&self.session, self.hands.paths[hand & 1], &v);
    }
}

fn create_actions(instance: &xr::Instance, session: &xr::Session<xr::D3D11>) -> Result<Hands> {
    let set = instance.create_action_set("gameplay", "Gameplay", 0)?;
    let paths = [instance.string_to_path("/user/hand/left")?, instance.string_to_path("/user/hand/right")?];
    let grip = set.create_action::<xr::Posef>("grip_pose", "Grip pose", &paths)?;
    let aim = set.create_action::<xr::Posef>("aim_pose", "Aim pose", &paths)?;
    let trigger = set.create_action::<f32>("trigger", "Trigger", &paths)?;
    let squeeze = set.create_action::<f32>("squeeze", "Grip", &paths)?;
    let stick = set.create_action::<xr::Vector2f>("thumbstick", "Thumbstick", &paths)?;
    let a = set.create_action::<bool>("button_a", "A / X", &paths)?;
    let b = set.create_action::<bool>("button_b", "B / Y", &paths)?;
    let stick_click = set.create_action::<bool>("thumbstick_click", "Thumbstick click", &paths)?;
    let menu = set.create_action::<bool>("menu", "Menu", &paths)?;
    let haptic = set.create_action::<xr::Haptic>("haptic", "Vibration", &paths)?;

    // Suggested bindings for the common controllers, as (input, action) per hand. Players can
    // rebind everything in their VR runtime.
    enum Act {
        Grip,
        Aim,
        Trigger,
        Squeeze,
        Stick,
        A,
        B,
        StickClick,
        Menu,
        Haptic,
    }
    use Act::*;
    let common: &[(&str, Act)] = &[
        ("input/grip/pose", Grip),
        ("input/aim/pose", Aim),
        ("output/haptic", Haptic),
        ("input/trigger/value", Trigger),
    ];
    let profiles: [(&str, [&[(&str, Act)]; 2]); 4] = [
        (
            "/interaction_profiles/oculus/touch_controller",
            [
                &[("input/squeeze/value", Squeeze), ("input/thumbstick", Stick), ("input/thumbstick/click", StickClick), ("input/x/click", A), ("input/y/click", B), ("input/menu/click", Menu)],
                &[("input/squeeze/value", Squeeze), ("input/thumbstick", Stick), ("input/thumbstick/click", StickClick), ("input/a/click", A), ("input/b/click", B)],
            ],
        ),
        (
            "/interaction_profiles/valve/index_controller",
            [
                &[("input/squeeze/value", Squeeze), ("input/thumbstick", Stick), ("input/thumbstick/click", StickClick), ("input/a/click", A), ("input/b/click", B)],
                &[("input/squeeze/value", Squeeze), ("input/thumbstick", Stick), ("input/thumbstick/click", StickClick), ("input/a/click", A), ("input/b/click", B)],
            ],
        ),
        (
            "/interaction_profiles/microsoft/motion_controller",
            [
                &[("input/squeeze/click", Squeeze), ("input/thumbstick", Stick), ("input/thumbstick/click", StickClick), ("input/trackpad/click", A), ("input/menu/click", Menu)],
                &[("input/squeeze/click", Squeeze), ("input/thumbstick", Stick), ("input/thumbstick/click", StickClick), ("input/trackpad/click", A), ("input/menu/click", Menu)],
            ],
        ),
        (
            "/interaction_profiles/htc/vive_controller",
            [
                &[("input/squeeze/click", Squeeze), ("input/trackpad", Stick), ("input/trackpad/click", StickClick), ("input/menu/click", Menu)],
                &[("input/squeeze/click", Squeeze), ("input/trackpad", Stick), ("input/trackpad/click", StickClick), ("input/menu/click", Menu)],
            ],
        ),
    ];
    for (profile, per_hand) in profiles {
        let mut bindings = Vec::new();
        for (h, hand) in ["left", "right"].iter().enumerate() {
            for (input, act) in common.iter().chain(per_hand[h].iter()) {
                let path = instance.string_to_path(&format!("/user/hand/{hand}/{input}"))?;
                bindings.push(match act {
                    Grip => xr::Binding::new(&grip, path),
                    Aim => xr::Binding::new(&aim, path),
                    Trigger => xr::Binding::new(&trigger, path),
                    Squeeze => xr::Binding::new(&squeeze, path),
                    Stick => xr::Binding::new(&stick, path),
                    A => xr::Binding::new(&a, path),
                    B => xr::Binding::new(&b, path),
                    StickClick => xr::Binding::new(&stick_click, path),
                    Menu => xr::Binding::new(&menu, path),
                    Haptic => xr::Binding::new(&haptic, path),
                });
            }
        }
        if let Err(e) = instance.suggest_interaction_profile_bindings(instance.string_to_path(profile)?, &bindings) {
            tracing::warn!("openxr: bindings for {profile}: {e}");
        }
    }
    session.attach_action_sets(&[&set])?;
    let grip_space = [
        grip.create_space(session, paths[0], xr::Posef::IDENTITY)?,
        grip.create_space(session, paths[1], xr::Posef::IDENTITY)?,
    ];
    let aim_space = [aim.create_space(session, paths[0], xr::Posef::IDENTITY)?, aim.create_space(session, paths[1], xr::Posef::IDENTITY)?];
    Ok(Hands { set, grip, _aim: aim, trigger, squeeze, stick, a, b, stick_click, menu, haptic, paths, grip_space, aim_space })
}

fn rtv(gpu: &Gpu, tex: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11RenderTargetView> {
    let desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
    };
    let mut v = None;
    unsafe { gpu.device.CreateRenderTargetView(tex, Some(&desc), Some(&mut v))? };
    v.ok_or_else(|| anyhow!("no render target view"))
}

/// OpenXR (right-handed, -Z forward) -> tracking space (left-handed, +Z forward).
pub fn from_xr(p: &xr::Posef, valid: bool) -> Pose {
    let o = p.orientation;
    let t = p.position;
    Pose { pos: [t.x, t.y, -t.z], valid: valid as u32, rot: [-o.x, -o.y, o.z, o.w] }
}

pub fn to_xr(p: &Pose) -> xr::Posef {
    xr::Posef {
        orientation: xr::Quaternionf { x: -p.rot[0], y: -p.rot[1], z: p.rot[2], w: p.rot[3] },
        position: xr::Vector3f { x: p.pos[0], y: p.pos[1], z: -p.pos[2] },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};

    #[test]
    fn mirroring_keeps_meaning() {
        // In OpenXR, a head turned 90 degrees to the right looks along +X (yaw -90 about +Y).
        let q = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        let xr_pose = xr::Posef {
            orientation: xr::Quaternionf { x: q.x, y: q.y, z: q.z, w: q.w },
            position: xr::Vector3f { x: 0.0, y: 1.7, z: -1.0 },
        };
        let t = from_xr(&xr_pose, true);
        let fwd = Quat::from_array(t.rot) * Vec3::Z;
        assert!(fwd.abs_diff_eq(Vec3::X, 1e-5), "still looking right: {fwd:?}");
        assert_eq!(t.pos, [0.0, 1.7, 1.0], "a metre in front stays in front");
        let back = to_xr(&t);
        assert!((back.orientation.y - q.y).abs() < 1e-6 && (back.position.z + 1.0).abs() < 1e-6);
    }
}
