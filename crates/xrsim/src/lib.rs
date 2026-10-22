//! An emulated headset: an OpenXR runtime for playtesting VR apps without a headset.
//!
//! The OpenXR loader loads it like any runtime (SteamVR, Virtual Desktop...) when a process
//! starts with `XR_RUNTIME_JSON` pointing at its manifest (`rustvr-xrsim manifest` writes one next
//! to `rustvr_xrsim.dll`), so nothing changes system-wide and the app under test runs its real
//! OpenXR code: instance, session states, actions and bindings, swapchains, frame timing and
//! submission. The head and controllers follow a script (`RUSTVR_XRSIM_SCRIPT`, see [`script`])
//! and live commands (`<dir>/control.toml`), and captures save what each eye would show
//! (`<dir>/captures/`, see [`capture`]). `<dir>` is `RUSTVR_XRSIM_DIR` (default
//! `%TEMP%\rustvr-xrsim`); `xrsim.log` there records the session and every binding the app made,
//! and `status.toml` its state each second. `docs/XRSIM.md` explains how to playtest with it.

pub mod capture;
pub mod runtime;
pub mod script;

use openxr_sys as xr;

/// The entry point the OpenXR loader looks for.
///
/// # Safety
/// Called by the OpenXR loader with valid negotiation structures.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn xrNegotiateLoaderRuntimeInterface(info: *const xr::NegotiateLoaderInfo, req: *mut xr::NegotiateRuntimeRequest) -> xr::Result {
    if info.is_null() || req.is_null() {
        return xr::Result::ERROR_INITIALIZATION_FAILED;
    }
    unsafe {
        let i = &*info;
        if i.min_interface_version > 1 || i.max_interface_version < 1 {
            return xr::Result::ERROR_INITIALIZATION_FAILED;
        }
        let r = &mut *req;
        r.runtime_interface_version = 1;
        r.runtime_api_version = xr::Version::new(1, 0, 34);
        r.get_instance_proc_addr = Some(runtime::get_instance_proc_addr);
    }
    xr::Result::SUCCESS
}

/// The runtime manifest's contents for the DLL at `dll`.
pub fn manifest(dll: &std::path::Path) -> String {
    let p = dll.display().to_string().replace('\\', "\\\\");
    format!("{{\n  \"file_format_version\": \"1.0.0\",\n  \"runtime\": {{\n    \"name\": \"{}\",\n    \"library_path\": \"{p}\"\n  }}\n}}\n", runtime::RUNTIME_NAME)
}
