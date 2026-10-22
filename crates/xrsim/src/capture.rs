//! Captures: what the app submitted, read back, and what the headset would show from it.
//!
//! `<name>.png` is both eyes as the headset shows them: each eye's own view (its pose and field of
//! view this frame) filled from the submitted projection layer, reprojected by rotation as a
//! compositor does. A wrong pose or field of view in the submission shows up here exactly as it
//! would in the headset (a shifted, stretched or cut-off picture; dark grey where the layer has
//! nothing). `<name>_raw.png` is the submitted images as they are, and `<name>.toml` the numbers.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use glam::{Quat, Vec3};
use openxr_sys as xr;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::runtime::{Rt, eye_fov, eye_pose, to_glam};

/// An image read back: width, height, RGBA8.
struct Image {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

fn read_back(r: &Rt, swapchain: u64, rect: &xr::Rect2Di) -> Result<Image> {
    let c = r.chains.get(&swapchain).context("the layer's swapchain is gone")?;
    let i = c.last_released.context("the layer's swapchain has no released image")?;
    let tex = &c.images[i as usize];
    let device = r.device.as_ref().context("no device")?;
    let (x, y) = (rect.offset.x.max(0) as u32, rect.offset.y.max(0) as u32);
    let w = (rect.extent.width.max(1) as u32).min(c.width - x.min(c.width - 1));
    let h = (rect.extent.height.max(1) as u32).min(c.height - y.min(c.height - 1));
    let format = DXGI_FORMAT(c.format as i32);
    let bgra = matches!(format, DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS);
    if !matches!(
        format,
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS
    ) {
        bail!("can't read format {}", c.format);
    }
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.context("staging texture")?;
        let ctx = device.GetImmediateContext()?;
        let bx = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
        ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, tex, 0, Some(&bx));
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        for row in 0..h as usize {
            let src = std::slice::from_raw_parts((m.pData as *const u8).add(row * m.RowPitch as usize), w as usize * 4);
            px[row * w as usize * 4..(row + 1) * w as usize * 4].copy_from_slice(src);
        }
        ctx.Unmap(&staging, 0);
        if bgra {
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
        }
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Ok(Image { w, h, px })
    }
}

/// Eye `e` as the headset shows it, `width` pixels wide, from the submitted layer image.
fn reproject(r: &Rt, e: usize, layer: &crate::runtime::Submitted, img: &Image, width: u32) -> Image {
    let fov = eye_fov(&r.script, e);
    let (l, rr, u, d) = (fov.angle_left.tan(), fov.angle_right.tan(), fov.angle_up.tan(), fov.angle_down.tan());
    let height = ((width as f32) * (u - d) / (rr - l)).round().max(1.0) as u32;
    let (_, qd) = eye_pose(r, e);
    let (_, ql) = to_glam(&layer.pose);
    let lf = layer.fov;
    let (ll, lr, lu, ld) = (lf.angle_left.tan(), lf.angle_right.tan(), lf.angle_up.tan(), lf.angle_down.tan());
    let to_layer: Quat = ql.inverse() * qd;
    let mut px = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let tx = l + (x as f32 + 0.5) / width as f32 * (rr - l);
            let ty = u - (y as f32 + 0.5) / height as f32 * (u - d);
            let dl = to_layer * Vec3::new(tx, ty, -1.0);
            let o = ((y * width + x) * 4) as usize;
            let mut c = [40u8, 40, 40, 255];
            if dl.z < -1e-6 {
                let (lx, ly) = (dl.x / -dl.z, dl.y / -dl.z);
                let su = (lx - ll) / (lr - ll);
                let sv = (lu - ly) / (lu - ld);
                if (0.0..1.0).contains(&su) && (0.0..1.0).contains(&sv) {
                    let (ix, iy) = ((su * img.w as f32) as u32, (sv * img.h as f32) as u32);
                    let i = ((iy.min(img.h - 1) * img.w + ix.min(img.w - 1)) * 4) as usize;
                    c = [img.px[i], img.px[i + 1], img.px[i + 2], 255];
                }
            }
            px[o..o + 4].copy_from_slice(&c);
        }
    }
    Image { w: width, h: height, px }
}

fn side_by_side(a: &Image, b: &Image) -> Image {
    let (w, h) = (a.w + b.w, a.h.max(b.h));
    let mut px = vec![0u8; (w * h * 4) as usize];
    for (img, x0) in [(a, 0), (b, a.w)] {
        for y in 0..img.h {
            let src = &img.px[(y * img.w * 4) as usize..((y + 1) * img.w * 4) as usize];
            let o = ((y * w + x0) * 4) as usize;
            px[o..o + src.len()].copy_from_slice(src);
        }
    }
    Image { w, h, px }
}

fn shrink(img: &Image, width: u32) -> Image {
    if img.w <= width {
        return Image { w: img.w, h: img.h, px: img.px.clone() };
    }
    let k = img.w as f32 / width as f32;
    let h = (img.h as f32 / k).round().max(1.0) as u32;
    let mut px = vec![0u8; (width * h * 4) as usize];
    for y in 0..h {
        for x in 0..width {
            let (sx, sy) = (((x as f32 + 0.5) * k) as u32, ((y as f32 + 0.5) * k) as u32);
            let i = ((sy.min(img.h - 1) * img.w + sx.min(img.w - 1)) * 4) as usize;
            let o = ((y * width + x) * 4) as usize;
            px[o..o + 4].copy_from_slice(&img.px[i..i + 4]);
        }
    }
    Image { w: width, h, px }
}

/// Saves capture `name` from the frame just submitted. Returns the picture's path.
pub(crate) fn save(r: &mut Rt, name: &str) -> Result<PathBuf> {
    let dir = r.dir.join("captures");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.png"));
    let width = r.script.capture_width.max(64);
    let mut info = format!(
        "time = {:.3}\nframes = {}\nfocused = {}\nlayers = {}\nhead = {{ pos = {:?}, yaw = {}, pitch = {} }}\n",
        r.start.elapsed().as_secs_f64(),
        r.frames,
        r.focused,
        r.last_layer_count,
        r.pose.head.pos.to_array(),
        r.pose.head.yaw,
        r.pose.head.pitch
    );
    if r.last_layers.len() < 2 {
        // Nothing submitted: the headset shows black.
        let blank = Image { w: width * 2, h: width, px: vec![0; (width * 2 * width * 4) as usize] };
        rustvr_gpu::save_png(&path, blank.w, blank.h, &blank.px)?;
        info.push_str("note = \"no projection layer submitted\"\n");
        std::fs::write(dir.join(format!("{name}.toml")), info)?;
        return Ok(path);
    }
    let layers = r.last_layers.clone();
    let raw: Vec<Image> = layers.iter().take(2).map(|l| read_back(r, l.swapchain, &l.rect)).collect::<Result<_>>()?;
    let shown = [reproject(r, 0, &layers[0], &raw[0], width), reproject(r, 1, &layers[1], &raw[1], width)];
    let both = side_by_side(&shown[0], &shown[1]);
    rustvr_gpu::save_png(&path, both.w, both.h, &both.px)?;
    let raw_both = side_by_side(&shrink(&raw[0], width), &shrink(&raw[1], width));
    rustvr_gpu::save_png(&dir.join(format!("{name}_raw.png")), raw_both.w, raw_both.h, &raw_both.px)?;
    for (e, l) in layers.iter().take(2).enumerate() {
        let f = eye_fov(&r.script, e);
        info.push_str(&format!(
            "[eye{e}]\nlayer_fov = [{:.2}, {:.2}, {:.2}, {:.2}]\nheadset_fov = [{:.2}, {:.2}, {:.2}, {:.2}]\nlayer_pos = [{:.3}, {:.3}, {:.3}]\nimage = [{}, {}]\n",
            l.fov.angle_left.to_degrees(),
            l.fov.angle_right.to_degrees(),
            l.fov.angle_up.to_degrees(),
            l.fov.angle_down.to_degrees(),
            f.angle_left.to_degrees(),
            f.angle_right.to_degrees(),
            f.angle_up.to_degrees(),
            f.angle_down.to_degrees(),
            l.pose.position.x,
            l.pose.position.y,
            l.pose.position.z,
            raw[e].w,
            raw[e].h
        ));
    }
    std::fs::write(dir.join(format!("{name}.toml")), info)?;
    Ok(path)
}
