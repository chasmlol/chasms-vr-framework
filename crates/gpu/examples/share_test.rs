//! Checks that a texture shared by one device opens on another the way a game's device would.
//!
//! `cargo run -p rustvr-gpu --example share_test` shares and opens in one process;
//! `share_test serve <name>` shares one and waits, `share_test open <name>` opens it from another
//! process with each kind of device (the runtime's: feature level 11_1 with BGRA support; a game's:
//! 11_0, no flags).
use anyhow::{Context, Result};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

fn device(levels: &[D3D_FEATURE_LEVEL], flags: D3D11_CREATE_DEVICE_FLAG) -> Result<Gpu> {
    let mut device = None;
    unsafe {
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), flags, Some(levels), D3D11_SDK_VERSION, Some(&mut device), None, None)?;
    }
    Gpu::from_device(&device.context("no device")?)
}

fn share(name: &str) -> Result<(Gpu, Texture)> {
    let runtime = Gpu::new()?;
    let desc = TexDesc { width: 64, height: 64, format: DXGI_FORMAT_R8G8B8A8_TYPELESS, rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM), srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), dsv: None, mips: 1 };
    let t = Texture::new_shared(&runtime, &desc, name, rustvr_protocol::link::Security::for_this_user().as_ref().map(|s| s.attributes()).as_ref())?;
    Ok((runtime, t))
}

fn open_all(name: &str) -> Result<()> {
    for (what, levels, flags) in [
        ("11_1 + BGRA", &[D3D_FEATURE_LEVEL_11_1][..], D3D11_CREATE_DEVICE_BGRA_SUPPORT),
        ("11_0, no flags (a game's)", &[D3D_FEATURE_LEVEL_11_0][..], D3D11_CREATE_DEVICE_FLAG(0)),
    ] {
        let gpu = device(levels, flags)?;
        let unorm = Some(DXGI_FORMAT_R8G8B8A8_UNORM);
        match Texture::open_shared(&gpu, name, unorm, unorm) {
            Ok(_) => println!("{what}: opens"),
            Err(e) => println!("{what}: {e:#}"),
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => {
            let _t = share(&args[1])?;
            println!("sharing {}", args[1]);
            std::thread::sleep(std::time::Duration::from_secs(30));
            Ok(())
        }
        Some("open") => open_all(&args[1]),
        _ => {
            let name = format!("Local\\RustVR_share_test_{}", std::process::id());
            let _t = share(&name)?;
            open_all(&name)?;
            match Texture::open_shared(&Gpu::new()?, "Local\\RustVR_no_such_texture", None, None) {
                Ok(_) => println!("a missing name opens?!"),
                Err(e) => println!("a missing name: {e:#}"),
            }
            Ok(())
        }
    }
}
