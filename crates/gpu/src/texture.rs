//! Textures: plain, shared between processes by name (NT handle + keyed mutex), and read back to
//! the CPU.

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIKeyedMutex, IDXGIResource, IDXGIResource1};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::core::{HSTRING, Interface};

use crate::Gpu;

/// A device of our own for opening shared textures by name on a game's device's behalf (see
/// [`Texture::open_shared`]); made the first time it's needed.
static HELPER: std::sync::Mutex<Option<Gpu>> = std::sync::Mutex::new(None);

/// Opens `name` on the helper device, makes an unnamed NT handle to it and opens that on `gpu`.
unsafe fn open_through_helper(gpu: &Gpu, name: &str, access: u32) -> Result<ID3D11Texture2D> {
    let mut helper = HELPER.lock().unwrap();
    if helper.is_none() {
        *helper = Some(Gpu::new().context("helper device")?);
    }
    let h = helper.as_ref().unwrap();
    unsafe {
        let theirs: ID3D11Texture2D = h.device1.OpenSharedResourceByName(&HSTRING::from(name), access).context("by name on a helper device")?;
        let res: IDXGIResource1 = theirs.cast()?;
        let handle = res.CreateSharedHandle(None, access, None).context("an unnamed handle")?;
        let opened = gpu.device1.OpenSharedResource1::<ID3D11Texture2D>(handle);
        let _ = CloseHandle(handle);
        opened.context("the unnamed handle on the game's device")
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TexDesc {
    pub width: u32,
    pub height: u32,
    /// Storage format. Use a TYPELESS format to pick different view formats.
    pub format: DXGI_FORMAT,
    /// View formats; `None` = no view of that kind.
    pub rtv: Option<DXGI_FORMAT>,
    pub srv: Option<DXGI_FORMAT>,
    pub dsv: Option<DXGI_FORMAT>,
    pub mips: u32,
}

impl TexDesc {
    /// A render target you can also sample, single format.
    pub fn target(width: u32, height: u32, format: DXGI_FORMAT) -> TexDesc {
        TexDesc { width, height, format, rtv: Some(format), srv: Some(format), dsv: None, mips: 1 }
    }

    pub fn depth(width: u32, height: u32) -> TexDesc {
        TexDesc { width, height, format: DXGI_FORMAT_D32_FLOAT, rtv: None, srv: None, dsv: Some(DXGI_FORMAT_D32_FLOAT), mips: 1 }
    }
}

pub struct Texture {
    pub tex: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
    pub format: DXGI_FORMAT,
    pub rtv: Option<ID3D11RenderTargetView>,
    pub srv: Option<ID3D11ShaderResourceView>,
    pub dsv: Option<ID3D11DepthStencilView>,
    pub mutex: Option<IDXGIKeyedMutex>,
    shared_handle: Option<HANDLE>,
}

// D3D11 devices are free-threaded; textures are used from one thread at a time.
unsafe impl Send for Texture {}

impl Texture {
    pub fn new(gpu: &Gpu, desc: &TexDesc) -> Result<Texture> {
        Texture::create(gpu, desc, D3D11_RESOURCE_MISC_FLAG(0), None)
    }

    /// Creates a texture other processes can open with [`Texture::open_shared`] under `name`.
    pub fn new_shared(gpu: &Gpu, desc: &TexDesc, name: &str, sa: Option<&SECURITY_ATTRIBUTES>) -> Result<Texture> {
        let misc = D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX;
        let mut t = Texture::create(gpu, desc, misc, None)?;
        unsafe {
            let res: IDXGIResource1 = t.tex.cast()?;
            let handle = res
                .CreateSharedHandle(
                    sa.map(|s| s as *const SECURITY_ATTRIBUTES),
                    (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0,
                    &HSTRING::from(name),
                )
                .with_context(|| format!("sharing texture {name}"))?;
            t.shared_handle = Some(handle);
            t.mutex = Some(t.tex.cast()?);
        }
        Ok(t)
    }

    /// Creates a texture another device in this process can open with [`Texture::open_local`]: a
    /// legacy shared handle with a keyed mutex, which devices that can't open NT-handle textures
    /// (some games') still can. Returns the texture and the handle (owned by the texture).
    pub fn new_shared_local(gpu: &Gpu, desc: &TexDesc) -> Result<(Texture, HANDLE)> {
        let mut t = Texture::create(gpu, desc, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, None)?;
        unsafe {
            let res: IDXGIResource = t.tex.cast()?;
            let handle = res.GetSharedHandle().context("legacy shared handle")?;
            t.mutex = Some(t.tex.cast()?);
            Ok((t, handle))
        }
    }

    /// Opens a texture from [`Texture::new_shared_local`] on another device (no views).
    pub fn open_local(gpu: &Gpu, handle: HANDLE) -> Result<Texture> {
        unsafe {
            let mut out: Option<ID3D11Texture2D> = None;
            gpu.device.OpenSharedResource(handle, &mut out).context("opening a legacy shared texture")?;
            let tex = out.context("no texture")?;
            let mut d = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut d);
            let mutex = tex.cast().ok();
            Ok(Texture { tex, width: d.Width, height: d.Height, format: d.Format, rtv: None, srv: None, dsv: None, mutex, shared_handle: None })
        }
    }

    /// Opens a texture shared by another process (or device) under `name`, with the given views.
    ///
    /// Some games' devices can't look a shared texture up by name (they get "the parameter is
    /// incorrect" for names any other device in the process finds). Then a helper device of our
    /// own opens it by name and hands the game's device an unnamed handle to it.
    pub fn open_shared(gpu: &Gpu, name: &str, rtv: Option<DXGI_FORMAT>, srv: Option<DXGI_FORMAT>) -> Result<Texture> {
        unsafe {
            let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
            let tex: ID3D11Texture2D = match gpu.device1.OpenSharedResourceByName(&HSTRING::from(name), access) {
                Ok(t) => t,
                Err(by_name) => open_through_helper(gpu, name, access).with_context(|| format!("opening shared texture {name} ({by_name})"))?,
            };
            let mut d = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut d);
            let mut t = Texture {
                tex,
                width: d.Width,
                height: d.Height,
                format: d.Format,
                rtv: None,
                srv: None,
                dsv: None,
                mutex: None,
                shared_handle: None,
            };
            t.mutex = t.tex.cast().ok();
            t.make_views(gpu, rtv, srv, None)?;
            Ok(t)
        }
    }

    /// Uploads an immutable texture with all its mips (e.g. BC-compressed model textures).
    pub fn upload(gpu: &Gpu, width: u32, height: u32, format: DXGI_FORMAT, mips: &[&[u8]]) -> Result<Texture> {
        let block = block_bytes(format);
        let init: Vec<D3D11_SUBRESOURCE_DATA> = mips
            .iter()
            .enumerate()
            .map(|(i, data)| {
                let w = (width >> i).max(1);
                let pitch = match block {
                    Some(b) => w.div_ceil(4) * b,
                    None => w * bytes_per_pixel(format),
                };
                D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr().cast(), SysMemPitch: pitch, SysMemSlicePitch: 0 }
            })
            .collect();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: mips.len() as u32,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        unsafe {
            let mut tex = None;
            gpu.device.CreateTexture2D(&desc, Some(init.as_ptr()), Some(&mut tex))?;
            let mut t = Texture {
                tex: tex.unwrap(),
                width,
                height,
                format,
                rtv: None,
                srv: None,
                dsv: None,
                mutex: None,
                shared_handle: None,
            };
            t.make_views(gpu, None, Some(format), None)?;
            Ok(t)
        }
    }

    fn create(gpu: &Gpu, desc: &TexDesc, misc: D3D11_RESOURCE_MISC_FLAG, init: Option<&[u8]>) -> Result<Texture> {
        let mut bind = 0u32;
        if desc.rtv.is_some() {
            bind |= D3D11_BIND_RENDER_TARGET.0 as u32;
        }
        if desc.srv.is_some() {
            bind |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        }
        if desc.dsv.is_some() {
            bind |= D3D11_BIND_DEPTH_STENCIL.0 as u32;
        }
        let d = D3D11_TEXTURE2D_DESC {
            Width: desc.width,
            Height: desc.height,
            MipLevels: desc.mips.max(1),
            ArraySize: 1,
            Format: desc.format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind,
            CPUAccessFlags: 0,
            MiscFlags: misc.0 as u32,
        };
        let data = init.map(|bytes| D3D11_SUBRESOURCE_DATA {
            pSysMem: bytes.as_ptr().cast(),
            SysMemPitch: desc.width * bytes_per_pixel(desc.format),
            SysMemSlicePitch: 0,
        });
        unsafe {
            let mut tex = None;
            gpu.device
                .CreateTexture2D(&d, data.as_ref().map(|d| d as *const _), Some(&mut tex))
                .with_context(|| format!("CreateTexture2D {}x{} {:?}", desc.width, desc.height, desc.format))?;
            let mut t = Texture {
                tex: tex.unwrap(),
                width: desc.width,
                height: desc.height,
                format: desc.format,
                rtv: None,
                srv: None,
                dsv: None,
                mutex: None,
                shared_handle: None,
            };
            t.make_views(gpu, desc.rtv, desc.srv, desc.dsv)?;
            Ok(t)
        }
    }

    /// Another view of it to sample, as `format` (one its TYPELESS storage takes: the raw UNORM of
    /// a texture its `srv` reads as sRGB, say).
    pub fn view(&self, gpu: &Gpu, format: DXGI_FORMAT) -> Result<ID3D11ShaderResourceView> {
        let mut td = D3D11_TEXTURE2D_DESC::default();
        unsafe { self.tex.GetDesc(&mut td) };
        let d = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: td.MipLevels } },
        };
        let mut v = None;
        unsafe { gpu.device.CreateShaderResourceView(&self.tex, Some(&d), Some(&mut v)) }.context("shader resource view")?;
        v.context("no view")
    }

    fn make_views(&mut self, gpu: &Gpu, rtv: Option<DXGI_FORMAT>, srv: Option<DXGI_FORMAT>, dsv: Option<DXGI_FORMAT>) -> Result<()> {
        unsafe {
            if let Some(f) = rtv {
                let d = D3D11_RENDER_TARGET_VIEW_DESC {
                    Format: f,
                    ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
                };
                let mut v = None;
                gpu.device.CreateRenderTargetView(&self.tex, Some(&d), Some(&mut v)).context("render target view")?;
                self.rtv = v;
            }
            if let Some(f) = srv {
                let mut td = D3D11_TEXTURE2D_DESC::default();
                self.tex.GetDesc(&mut td);
                let d = D3D11_SHADER_RESOURCE_VIEW_DESC {
                    Format: f,
                    ViewDimension: windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: td.MipLevels },
                    },
                };
                let mut v = None;
                gpu.device.CreateShaderResourceView(&self.tex, Some(&d), Some(&mut v)).context("shader resource view")?;
                self.srv = v;
            }
            if let Some(f) = dsv {
                let d = D3D11_DEPTH_STENCIL_VIEW_DESC {
                    Format: f,
                    ViewDimension: D3D11_DSV_DIMENSION_TEXTURE2D,
                    Flags: 0,
                    Anonymous: D3D11_DEPTH_STENCIL_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_DSV { MipSlice: 0 } },
                };
                let mut v = None;
                gpu.device.CreateDepthStencilView(&self.tex, Some(&d), Some(&mut v)).context("depth stencil view")?;
                self.dsv = v;
            }
        }
        Ok(())
    }

    /// Takes the keyed mutex (key 0). True if acquired (or the texture isn't shared).
    pub fn acquire(&self, timeout_ms: u32) -> bool {
        let Some(m) = &self.mutex else { return true };
        // Called through the vtable to see WAIT_TIMEOUT, which is a success HRESULT.
        let hr = unsafe { (Interface::vtable(m).AcquireSync)(Interface::as_raw(m), 0, timeout_ms) };
        hr.0 == 0
    }

    pub fn release(&self) {
        if let Some(m) = &self.mutex {
            unsafe {
                let _ = m.ReleaseSync(0);
            }
        }
    }

    /// Copies the texture to the CPU as tightly packed RGBA8 (BGRA is swizzled). Only for 8-bit
    /// colour formats.
    pub fn read_rgba8(&self, gpu: &Gpu) -> Result<Vec<u8>> {
        let bgra = matches!(
            self.format,
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS
        );
        if bytes_per_pixel(self.format) != 4 || block_bytes(self.format).is_some() {
            bail!("read_rgba8: unsupported format {:?}", self.format);
        }
        let raw = self.read_raw(gpu)?;
        let mut out = raw;
        if bgra {
            for px in out.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
        }
        Ok(out)
    }

    /// Copies mip 0 to the CPU, tightly packed (4 bytes per pixel formats only).
    pub fn read_raw(&self, gpu: &Gpu) -> Result<Vec<u8>> {
        let bpp = bytes_per_pixel(self.format);
        unsafe {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            self.tex.GetDesc(&mut d);
            d.MipLevels = 1;
            d.Usage = D3D11_USAGE_STAGING;
            d.BindFlags = 0;
            d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            d.MiscFlags = 0;
            let mut staging = None;
            gpu.device.CreateTexture2D(&d, None, Some(&mut staging))?;
            let staging = staging.unwrap();
            gpu.ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &self.tex, 0, None);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = (self.width * bpp) as usize;
            let mut out = vec![0u8; row * self.height as usize];
            for y in 0..self.height as usize {
                let src = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                std::ptr::copy_nonoverlapping(src, out.as_mut_ptr().add(y * row), row);
            }
            gpu.ctx.Unmap(&staging, 0);
            Ok(out)
        }
    }

    pub fn save_png(&self, gpu: &Gpu, path: &std::path::Path) -> Result<()> {
        let px = self.read_rgba8(gpu)?;
        crate::save_png(path, self.width, self.height, &px)
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        if let Some(h) = self.shared_handle.take() {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

pub fn bytes_per_pixel(f: DXGI_FORMAT) -> u32 {
    match f {
        DXGI_FORMAT_R8_UNORM => 1,
        DXGI_FORMAT_R16G16B16A16_FLOAT => 8,
        DXGI_FORMAT_R32G32B32A32_FLOAT => 16,
        _ => 4,
    }
}

/// Bytes per 4x4 block for block-compressed formats.
pub fn block_bytes(f: DXGI_FORMAT) -> Option<u32> {
    match f {
        DXGI_FORMAT_BC1_UNORM | DXGI_FORMAT_BC1_UNORM_SRGB | DXGI_FORMAT_BC4_UNORM => Some(8),
        DXGI_FORMAT_BC2_UNORM | DXGI_FORMAT_BC2_UNORM_SRGB | DXGI_FORMAT_BC3_UNORM | DXGI_FORMAT_BC3_UNORM_SRGB
        | DXGI_FORMAT_BC5_UNORM | DXGI_FORMAT_BC7_UNORM | DXGI_FORMAT_BC7_UNORM_SRGB => Some(16),
        _ => None,
    }
}

/// The TYPELESS storage format and the UNORM / UNORM_SRGB views of an 8-bit colour format.
pub fn color_family(f: DXGI_FORMAT) -> (DXGI_FORMAT, DXGI_FORMAT, DXGI_FORMAT) {
    match f {
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS => {
            (DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB)
        }
        _ => (DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_texture_crosses_devices() {
        let a = Gpu::new().unwrap();
        let b = Gpu::new().unwrap();
        let name = format!("Local\\RustVR_test_tex_{}", std::process::id());
        let (typeless, unorm, srgb) = color_family(DXGI_FORMAT_R8G8B8A8_UNORM);
        let desc = TexDesc { width: 16, height: 8, format: typeless, rtv: Some(unorm), srv: Some(srgb), dsv: None, mips: 1 };
        let owner = Texture::new_shared(&a, &desc, &name, None).unwrap();
        let other = Texture::open_shared(&b, &name, Some(unorm), Some(srgb)).unwrap();
        assert_eq!((other.width, other.height), (16, 8));
        assert!(other.acquire(100));
        unsafe { b.ctx.ClearRenderTargetView(other.rtv.as_ref().unwrap(), &[1.0, 0.5, 0.0, 1.0]) };
        other.release();
        b.flush();
        assert!(owner.acquire(1000));
        let px = owner.read_rgba8(&a).unwrap();
        owner.release();
        assert!(px[0] == 255 && (127..=128).contains(&px[1]) && px[2] == 0 && px[3] == 255, "{:?}", &px[0..4]);
    }
}
