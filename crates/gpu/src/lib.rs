//! Direct3D 11 helpers shared by the runtime, the adapter SDK and the test range: device creation,
//! shader compilation, shared textures (NT handles + keyed mutex), CPU readback to PNG, small
//! pipeline-state builders and an egui painter.
//!
//! Nothing here ever creates a window: the runtime renders into OpenXR swapchains or offscreen
//! textures, and the test range renders offscreen.

#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

pub mod egui_paint;
pub mod pipeline;
pub mod shader;
pub mod texture;

pub use egui_paint::EguiPainter;
pub use texture::{TexDesc, Texture};

use anyhow::{Context, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::*;
use windows::core::Interface;

/// A D3D11 device and its immediate context.
#[derive(Clone)]
pub struct Gpu {
    pub device: ID3D11Device,
    pub device1: ID3D11Device1,
    pub ctx: ID3D11DeviceContext,
}

impl Gpu {
    /// Creates a hardware device (WARP if `RUSTVR_WARP` is set, for machines without a GPU).
    pub fn new() -> Result<Gpu> {
        let warp = std::env::var_os("RUSTVR_WARP").is_some();
        let mut flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        if std::env::var_os("RUSTVR_D3D_DEBUG").is_some() {
            flags |= D3D11_CREATE_DEVICE_DEBUG;
        }
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut device = None;
        let mut ctx = None;
        let mut level = D3D_FEATURE_LEVEL::default();
        unsafe {
            D3D11CreateDevice(
                None,
                if warp { D3D_DRIVER_TYPE_WARP } else { D3D_DRIVER_TYPE_HARDWARE },
                HMODULE::default(),
                flags,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut level),
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice")?;
        }
        let device = device.context("no device")?;
        Gpu::from_parts(device, ctx.context("no context")?)
    }

    /// Creates a device on the GPU with this LUID (what OpenXR asks for: the headset's GPU).
    pub fn new_on_adapter(luid: i64) -> Result<Gpu> {
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
        use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut i = 0;
            loop {
                let adapter = factory.EnumAdapters1(i).context("the headset's GPU wasn't found")?;
                let desc = adapter.GetDesc1()?;
                let id = ((desc.AdapterLuid.HighPart as i64) << 32) | desc.AdapterLuid.LowPart as i64;
                if id == luid {
                    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
                    let mut device = None;
                    let mut ctx = None;
                    D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        Some(&levels),
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut ctx),
                    )
                    .context("D3D11CreateDevice")?;
                    return Gpu::from_parts(device.context("no device")?, ctx.context("no context")?);
                }
                i += 1;
            }
        }
    }

    /// Wraps an existing device, e.g. the game's own (adapters).
    pub fn from_device(device: &ID3D11Device) -> Result<Gpu> {
        let ctx = unsafe { device.GetImmediateContext()? };
        Gpu::from_parts(device.clone(), ctx)
    }

    fn from_parts(device: ID3D11Device, ctx: ID3D11DeviceContext) -> Result<Gpu> {
        let device1: ID3D11Device1 = device.cast().context("ID3D11Device1 (D3D 11.1) is required for shared textures")?;
        Ok(Gpu { device, device1, ctx })
    }

    /// Name of the GPU, for logs.
    pub fn adapter_name(&self) -> String {
        use windows::Win32::Graphics::Dxgi::IDXGIDevice;
        unsafe {
            let Ok(dxgi) = self.device.cast::<IDXGIDevice>() else { return String::new() };
            let Ok(adapter) = dxgi.GetAdapter() else { return String::new() };
            let Ok(desc) = adapter.GetDesc() else { return String::new() };
            let n = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            String::from_utf16_lossy(&desc.Description[..n])
        }
    }

    pub fn flush(&self) {
        unsafe { self.ctx.Flush() }
    }
}

/// Writes tightly packed RGBA8 pixels as a PNG.
pub fn save_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}
