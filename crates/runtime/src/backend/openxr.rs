//! Real headsets through OpenXR (SteamVR, Virtual Desktop, Quest Link, WMR, Pimax...) with a D3D11
//! session.
//!
//! Everything the headset offers that the player feels is used when its runtime has it: the
//! refresh rate it runs at (`XR_FB_display_refresh_rate`), poses predicted for a later moment (what
//! a game drawing a frame behind is given), each eye's depth with its picture
//! (`XR_KHR_composition_layer_depth`, for the headset's own reprojection), the controllers' touch
//! sensors (a finger off the trigger points, a thumb off the thumbrest lifts), and hands tracked by
//! the headset's cameras (`XR_EXT_hand_tracking`: the fingers as they are, a fist grabs).
//!
//! OpenXR is right-handed with -Z forward; tracking space here is left-handed with +Z forward, so
//! every pose crossing this boundary has Z mirrored: positions (x, y, -z), quaternions
//! (-x, -y, z, w). FOV angles keep their meaning (left is still -X).

use anyhow::{Context, Result, anyhow, bail};
use openxr as xr;
use rustvr_gpu::Gpu;
use rustvr_protocol::*;
use glam::Vec3;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_DEPTH_STENCIL_VIEW_DESC, D3D11_DEPTH_STENCIL_VIEW_DESC_0, D3D11_DSV_DIMENSION_TEXTURE2D, D3D11_RENDER_TARGET_VIEW_DESC,
    D3D11_RENDER_TARGET_VIEW_DESC_0, D3D11_RTV_DIMENSION_TEXTURE2D, D3D11_TEX2D_DSV, D3D11_TEX2D_RTV, ID3D11DepthStencilView,
    ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_D16_UNORM, DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_FORMAT_D32_FLOAT,
    DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
};
use windows::core::Interface;

use super::{Backend, DepthTarget, EyeTarget, HmdFrame, Located};

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
    /// The touch sensors: the index finger on the trigger, the thumb on A / X, B / Y, the stick or
    /// the thumbrest.
    touch_trigger: xr::Action<bool>,
    touch_a: xr::Action<bool>,
    touch_b: xr::Action<bool>,
    touch_stick: xr::Action<bool>,
    touch_rest: xr::Action<bool>,
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

/// One eye's depth images, for the headset (`XR_KHR_composition_layer_depth`).
struct DepthChain {
    swapchain: xr::Swapchain<xr::D3D11>,
    images: Vec<ID3D11DepthStencilView>,
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
    /// Each eye's depth for the headset, when its runtime takes it.
    depth_chains: Vec<DepthChain>,
    /// The hands the headset's cameras track, when its runtime offers that.
    trackers: Option<[xr::HandTracker; 2]>,
    size: (u32, u32),
    /// The headset's refresh rate (Hz), as its runtime says (`XR_FB_display_refresh_rate`) or as
    /// its frame period says.
    refresh: f32,
    /// Its runtime says the refresh rate (and may change it when asked).
    refresh_ext: bool,
    /// What the player last asked for (Hz, 0: the highest), so it's asked once.
    refresh_wanted: Option<f32>,
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
        exts.khr_composition_layer_depth = available.khr_composition_layer_depth;
        exts.fb_display_refresh_rate = available.fb_display_refresh_rate;
        exts.ext_hand_tracking = available.ext_hand_tracking;
        exts.ext_hand_tracking_data_source = available.ext_hand_tracking && available.ext_hand_tracking_data_source;
        tracing::info!(
            "openxr: headset depth {}, refresh rate control {}, hand tracking {}{}",
            yes(exts.khr_composition_layer_depth),
            yes(exts.fb_display_refresh_rate),
            yes(exts.ext_hand_tracking),
            if exts.ext_hand_tracking_data_source { " (the hands alone)" } else { "" }
        );
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
        // The headset's depth (its runtime reprojects with it): a depth format it takes, one chain
        // an eye the size of the colour.
        let mut depth_chains = Vec::new();
        if instance.exts().khr_composition_layer_depth.is_some() {
            let depth_format = [DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_FORMAT_D16_UNORM].into_iter().find(|f| formats.contains(&(f.0 as u32)));
            match depth_format {
                Some(df) => {
                    for v in &views {
                        let made = session.create_swapchain(&xr::SwapchainCreateInfo {
                            create_flags: xr::SwapchainCreateFlags::EMPTY,
                            usage_flags: xr::SwapchainUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                            format: df.0 as u32,
                            sample_count: 1,
                            width: v.recommended_image_rect_width,
                            height: v.recommended_image_rect_height,
                            face_count: 1,
                            array_size: 1,
                            mip_count: 1,
                        });
                        let chain = made.map_err(anyhow::Error::from).and_then(|swapchain| {
                            let mut images = Vec::new();
                            for raw in swapchain.enumerate_images()? {
                                let tex = unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }.cloned().ok_or_else(|| anyhow!("null depth image"))?;
                                images.push(dsv(&gpu, &tex, df)?);
                            }
                            Ok(DepthChain { swapchain, images, acquired: None })
                        });
                        match chain {
                            Ok(c) => depth_chains.push(c),
                            Err(e) => {
                                tracing::warn!("openxr: no depth for the headset: {e:#}");
                                depth_chains.clear();
                                break;
                            }
                        }
                    }
                    if !depth_chains.is_empty() {
                        tracing::info!("openxr: each eye's depth goes to the headset ({df:?})");
                    }
                }
                None => tracing::info!("openxr: the runtime takes depth but no depth format we write"),
            }
        }
        // The rate it runs at: its runtime's word, else (until frames say) 90.
        let refresh_ext = instance.exts().fb_display_refresh_rate.is_some();
        let refresh = if refresh_ext {
            match session.enumerate_display_refresh_rates() {
                Ok(rates) => tracing::info!("openxr: refresh rates {rates:?}"),
                Err(e) => tracing::info!("openxr: refresh rates: {e}"),
            }
            session.get_display_refresh_rate().ok().filter(|r| *r > 20.0).unwrap_or(90.0)
        } else {
            90.0
        };
        tracing::info!("openxr: {refresh:.0} Hz");
        let trackers = if instance.exts().ext_hand_tracking.is_some() {
            match hand_trackers(&instance, &session) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::info!("openxr: no hand tracking: {e:#}");
                    None
                }
            }
        } else {
            None
        };
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
                depth_chains,
                trackers,
                size,
                refresh,
                refresh_ext,
                refresh_wanted: None,
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
                xr::Event::DisplayRefreshRateChangedFB(e) => {
                    let (from, to) = (e.from_display_refresh_rate(), e.to_display_refresh_rate());
                    tracing::info!("openxr: refresh rate {from:.0} -> {to:.0} Hz");
                    if to > 20.0 {
                        self.refresh = to;
                    }
                }
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
        bit(&hd.touch_trigger, CTRL_TOUCH_TRIGGER)?;
        bit(&hd.touch_a, CTRL_TOUCH_A)?;
        bit(&hd.touch_b, CTRL_TOUCH_B)?;
        bit(&hd.touch_stick, CTRL_TOUCH_STICK)?;
        bit(&hd.touch_rest, CTRL_TOUCH_REST)?;
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
        // Without its runtime's word on the rate, the frame period says it.
        let period = state.predicted_display_period.as_nanos();
        if !self.refresh_ext && (4_000_000..20_000_000).contains(&period) {
            let hz = 1e9 / period as f32;
            if (hz - self.refresh).abs() > 0.5 {
                tracing::info!("openxr: {hz:.0} Hz (by the frame period)");
                self.refresh = hz;
            }
        }
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
        // Hands the cameras track: the fingers as they are, and a fist grabs (the runtime makes
        // a pinch the trigger itself).
        if let Some(trackers) = &self.trackers {
            for h in 0..2 {
                let Ok(Some(joints)) = self.stage.locate_hand_joints(&trackers[h], time) else { continue };
                let tracked = |j: &xr::HandJointLocation| j.location_flags.contains(xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID);
                if !joints.iter().all(tracked) {
                    continue;
                }
                let curls = finger_curls(&joints.map(|j| Vec3::new(j.pose.position.x, j.pose.position.y, j.pose.position.z)));
                f.fingers[h] = Some(curls);
                let c = &mut f.hands[h];
                c.squeeze = c.squeeze.max(fist(&curls));
                if c.squeeze > 0.75 {
                    c.buttons |= CTRL_SQUEEZE_CLICK;
                }
            }
        }
        Ok(Some(f))
    }

    fn locate_at(&mut self, time_ns: i64) -> Option<Located> {
        if !self.running {
            return None;
        }
        let time = xr::Time::from_nanos(time_ns);
        let (flags, views) = self.session.locate_views(VIEW_TYPE, time, &self.stage).ok()?;
        let valid = flags.contains(xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID);
        if !valid || views.len() < 2 {
            return None;
        }
        let head = self.view.locate(&self.stage, time).ok()?;
        let mut l = Located { hmd: from_xr(&head.pose, true), ..Default::default() };
        for e in 0..2 {
            l.eyes[e] = from_xr(&views[e].pose, true);
        }
        let ok = |s: &xr::SpaceLocation| s.location_flags.contains(xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID);
        for h in 0..2 {
            if let Ok(g) = self.hands.grip_space[h].locate(&self.stage, time) {
                l.grips[h] = from_xr(&g.pose, ok(&g));
            }
            if let Ok(a) = self.hands.aim_space[h].locate(&self.stage, time) {
                l.aims[h] = from_xr(&a.pose, ok(&a));
            }
        }
        Some(l)
    }

    fn depth_targets(&mut self, _gpu: &Gpu) -> Option<[DepthTarget; 2]> {
        if self.depth_chains.len() != 2 || !self.began {
            return None;
        }
        let mut out = Vec::new();
        for c in &mut self.depth_chains {
            let i = match c.acquired {
                Some(i) => i,
                None => {
                    let i = c.swapchain.acquire_image().ok()?;
                    c.swapchain.wait_image(xr::Duration::INFINITE).ok()?;
                    c.acquired = Some(i);
                    i
                }
            };
            out.push(DepthTarget { dsv: c.images[i as usize].clone(), width: self.size.0, height: self.size.1 });
        }
        let mut it = out.into_iter();
        Some([it.next()?, it.next()?])
    }

    fn want_refresh(&mut self, hz: f32) {
        if !self.refresh_ext || !self.running || self.refresh_wanted == Some(hz) {
            return;
        }
        self.refresh_wanted = Some(hz);
        let rates = self.session.enumerate_display_refresh_rates().unwrap_or_default();
        let pick = if hz > 0.0 {
            rates.iter().copied().min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs()))
        } else {
            rates.iter().copied().max_by(f32::total_cmp)
        };
        let Some(rate) = pick else { return };
        match self.session.request_display_refresh_rate(rate) {
            Ok(()) => tracing::info!("openxr: asked for {rate:.0} Hz (of {rates:?})"),
            // (Virtual Desktop keeps the rate its own settings have: 72 to 120 there.)
            Err(e) => tracing::info!("openxr: {rate:.0} Hz refused ({e}); the headset's own settings choose its rate"),
        }
        if let Ok(r) = self.session.get_display_refresh_rate()
            && r > 20.0
        {
            self.refresh = r;
        }
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
        let mut depth = 0;
        for c in &mut self.depth_chains {
            if c.acquired.take().is_some() {
                c.swapchain.release_image()?;
                depth += 1;
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
        // Each eye's depth with it (as the compositor's projection keeps it: near at 0, far at 1).
        let depth_infos: Vec<xr::sys::CompositionLayerDepthInfoKHR> = if depth == 2 {
            (0..2)
                .map(|e| xr::sys::CompositionLayerDepthInfoKHR {
                    ty: xr::sys::CompositionLayerDepthInfoKHR::TYPE,
                    next: std::ptr::null(),
                    sub_image: xr::sys::SwapchainSubImage { swapchain: self.depth_chains[e].swapchain.as_raw(), image_rect: rect, image_array_index: 0 },
                    min_depth: 0.0,
                    max_depth: 1.0,
                    near_z: crate::compositor::NEAR,
                    far_z: crate::compositor::FAR,
                })
                .collect()
        } else {
            Vec::new()
        };
        let views: Vec<xr::CompositionLayerProjectionView<xr::D3D11>> = (0..2)
            .map(|e| {
                let v = xr::CompositionLayerProjectionView::new()
                    .pose(to_xr(&poses[e]))
                    .fov(xr::Fovf {
                        angle_left: fov[e].left.atan(),
                        angle_right: fov[e].right.atan(),
                        angle_up: fov[e].up.atan(),
                        angle_down: fov[e].down.atan(),
                    })
                    .sub_image(xr::SwapchainSubImage::new().swapchain(&self.chains[e].swapchain).image_array_index(0).image_rect(rect));
                match depth_infos.get(e) {
                    Some(d) => {
                        let mut raw = v.into_raw();
                        raw.next = d as *const xr::sys::CompositionLayerDepthInfoKHR as *const _;
                        // (`depth_infos` outlives the frame's end below.)
                        unsafe { xr::CompositionLayerProjectionView::from_raw(raw) }
                    }
                    None => v,
                }
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
    let touch_trigger = set.create_action::<bool>("touch_trigger", "Finger on the trigger", &paths)?;
    let touch_a = set.create_action::<bool>("touch_a", "Thumb on A / X", &paths)?;
    let touch_b = set.create_action::<bool>("touch_b", "Thumb on B / Y", &paths)?;
    let touch_stick = set.create_action::<bool>("touch_thumbstick", "Thumb on the thumbstick", &paths)?;
    let touch_rest = set.create_action::<bool>("touch_thumbrest", "Thumb on the thumbrest", &paths)?;
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
        TouchTrigger,
        TouchA,
        TouchB,
        TouchStick,
        TouchRest,
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
                &[
                    ("input/squeeze/value", Squeeze),
                    ("input/thumbstick", Stick),
                    ("input/thumbstick/click", StickClick),
                    ("input/x/click", A),
                    ("input/y/click", B),
                    ("input/menu/click", Menu),
                    ("input/trigger/touch", TouchTrigger),
                    ("input/x/touch", TouchA),
                    ("input/y/touch", TouchB),
                    ("input/thumbstick/touch", TouchStick),
                    ("input/thumbrest/touch", TouchRest),
                ],
                &[
                    ("input/squeeze/value", Squeeze),
                    ("input/thumbstick", Stick),
                    ("input/thumbstick/click", StickClick),
                    ("input/a/click", A),
                    ("input/b/click", B),
                    ("input/trigger/touch", TouchTrigger),
                    ("input/a/touch", TouchA),
                    ("input/b/touch", TouchB),
                    ("input/thumbstick/touch", TouchStick),
                    ("input/thumbrest/touch", TouchRest),
                ],
            ],
        ),
        (
            "/interaction_profiles/valve/index_controller",
            [
                &[
                    ("input/squeeze/value", Squeeze),
                    ("input/thumbstick", Stick),
                    ("input/thumbstick/click", StickClick),
                    ("input/a/click", A),
                    ("input/b/click", B),
                    ("input/trigger/touch", TouchTrigger),
                    ("input/a/touch", TouchA),
                    ("input/b/touch", TouchB),
                    ("input/thumbstick/touch", TouchStick),
                ],
                &[
                    ("input/squeeze/value", Squeeze),
                    ("input/thumbstick", Stick),
                    ("input/thumbstick/click", StickClick),
                    ("input/a/click", A),
                    ("input/b/click", B),
                    ("input/trigger/touch", TouchTrigger),
                    ("input/a/touch", TouchA),
                    ("input/b/touch", TouchB),
                    ("input/thumbstick/touch", TouchStick),
                ],
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
                    TouchTrigger => xr::Binding::new(&touch_trigger, path),
                    TouchA => xr::Binding::new(&touch_a, path),
                    TouchB => xr::Binding::new(&touch_b, path),
                    TouchStick => xr::Binding::new(&touch_stick, path),
                    TouchRest => xr::Binding::new(&touch_rest, path),
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
    Ok(Hands {
        set,
        grip,
        _aim: aim,
        trigger,
        squeeze,
        stick,
        a,
        b,
        stick_click,
        menu,
        touch_trigger,
        touch_a,
        touch_b,
        touch_stick,
        touch_rest,
        haptic,
        paths,
        grip_space,
        aim_space,
    })
}

/// One tracker for each hand. Where the runtime can tell, only the hands themselves (not hands it
/// makes up from controllers): a controller in the hand drives the fingers by its own sensors.
fn hand_trackers(instance: &xr::Instance, session: &xr::Session<xr::D3D11>) -> Result<[xr::HandTracker; 2]> {
    let fp = instance.exts().ext_hand_tracking.as_ref().ok_or_else(|| anyhow!("no XR_EXT_hand_tracking"))?;
    let alone = instance.exts().ext_hand_tracking_data_source.is_some();
    let make = |hand: xr::Hand| -> Result<xr::HandTracker> {
        let mut sources = [xr::sys::HandTrackingDataSourceEXT::UNOBSTRUCTED];
        let source = xr::sys::HandTrackingDataSourceInfoEXT {
            ty: xr::sys::HandTrackingDataSourceInfoEXT::TYPE,
            next: std::ptr::null(),
            requested_data_source_count: 1,
            requested_data_sources: sources.as_mut_ptr(),
        };
        let info = xr::sys::HandTrackerCreateInfoEXT {
            ty: xr::sys::HandTrackerCreateInfoEXT::TYPE,
            next: if alone { &source as *const _ as *const _ } else { std::ptr::null() },
            hand,
            hand_joint_set: xr::sys::HandJointSetEXT::DEFAULT,
        };
        let mut out = <xr::sys::HandTrackerEXT as xr::sys::Handle>::NULL;
        let r = unsafe { (fp.create_hand_tracker)(session.as_raw(), &info, &mut out) };
        if r != xr::sys::Result::SUCCESS {
            bail!("creating a hand tracker: {r:?}");
        }
        Ok(unsafe { xr::HandTracker::from_raw(session, out) })
    };
    Ok([make(xr::Hand::LEFT)?, make(xr::Hand::RIGHT)?])
}

/// How far each finger is closed (thumb .. little, 0 straight to 1 closed) from a tracked hand's
/// joint positions (OpenXR's 26: palm, wrist, then each finger base to tip): the bend along the
/// finger, joint by joint, against a closed finger's.
fn finger_curls(j: &[Vec3; 26]) -> [f32; 5] {
    // (Each finger's joints, base to tip, and how much a closed one bends in all, est.: the thumb
    // across the palm, the others into a fist.)
    const FINGERS: [(&[usize], f32); 5] = [(&[2, 3, 4, 5], 1.0), (&[6, 7, 8, 9, 10], 4.0), (&[11, 12, 13, 14, 15], 4.0), (&[16, 17, 18, 19, 20], 4.0), (&[21, 22, 23, 24, 25], 4.0)];
    FINGERS.map(|(chain, closed)| {
        let mut bend = 0.0;
        for k in 0..chain.len() - 2 {
            let a = (j[chain[k + 1]] - j[chain[k]]).normalize_or_zero();
            let b = (j[chain[k + 2]] - j[chain[k + 1]]).normalize_or_zero();
            bend += a.dot(b).clamp(-1.0, 1.0).acos();
        }
        (bend / closed).clamp(0.0, 1.0)
    })
}

/// A tracked hand's grab: the middle, ring and little fingers closed (a fist), 0 to 1.
fn fist(curls: &[f32; 5]) -> f32 {
    let closed = (curls[2] + curls[3] + curls[4]) / 3.0;
    ((closed - 0.45) / 0.35).clamp(0.0, 1.0)
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn dsv(gpu: &Gpu, tex: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11DepthStencilView> {
    let desc = D3D11_DEPTH_STENCIL_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_DSV_DIMENSION_TEXTURE2D,
        Flags: 0,
        Anonymous: D3D11_DEPTH_STENCIL_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_DSV { MipSlice: 0 } },
    };
    let mut v = None;
    unsafe { gpu.device.CreateDepthStencilView(tex, Some(&desc), Some(&mut v))? };
    v.ok_or_else(|| anyhow!("no depth stencil view"))
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
    use glam::Quat;

    /// A hand laid out flat (fingers straight along -Z), then each finger bent `angle` at each of
    /// its joints.
    fn hand(angle: [f32; 5]) -> [Vec3; 26] {
        let mut j = [Vec3::ZERO; 26];
        let chains: [&[usize]; 5] = [&[2, 3, 4, 5], &[6, 7, 8, 9, 10], &[11, 12, 13, 14, 15], &[16, 17, 18, 19, 20], &[21, 22, 23, 24, 25]];
        for (f, chain) in chains.iter().enumerate() {
            let mut p = Vec3::new(f as f32 * 0.02, 0.0, 0.0);
            let mut dir = Vec3::NEG_Z;
            for (k, &i) in chain.iter().enumerate() {
                j[i] = p;
                if k > 0 {
                    dir = Quat::from_rotation_x(-angle[f]) * dir;
                }
                p += dir * 0.03;
            }
        }
        j
    }

    #[test]
    fn tracked_fingers_read_open_and_closed() {
        let open = finger_curls(&hand([0.0; 5]));
        assert!(open.iter().all(|c| *c < 0.02), "a flat hand is open: {open:?}");
        let fist_hand = finger_curls(&hand([0.55, 1.4, 1.4, 1.4, 1.4]));
        assert!(fist_hand[1..].iter().all(|c| *c > 0.9), "a fist is closed: {fist_hand:?}");
        assert!(fist(&fist_hand) > 0.99 && fist(&open) == 0.0, "a fist grabs, an open hand doesn't");
        let point = finger_curls(&hand([0.4, 0.0, 1.4, 1.4, 1.4]));
        assert!(point[1] < 0.05 && point[2] > 0.9, "pointing: {point:?}");
    }

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
