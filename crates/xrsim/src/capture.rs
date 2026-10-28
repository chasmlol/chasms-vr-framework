//! Captures: what the app submitted, read back, and what the headset would show from it.
//!
//! `<name>.png` is both eyes as the headset shows them: each eye's own view (its pose and field of
//! view this frame) filled from the submitted projection layer, reprojected by rotation as a
//! compositor does. A wrong pose or field of view in the submission shows up here exactly as it
//! would in the headset (a shifted, stretched or cut-off picture; dark grey where the layer has
//! nothing). `<name>_raw.png` is the submitted images as they are, and `<name>.toml` the numbers.

use std::collections::VecDeque;
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

/// The mip level films keep unless told (a quarter of the submitted size).
pub(crate) const FILM_MIP: u32 = 2;

/// A film being taken (`crate::script::Film`).
pub(crate) struct Film {
    name: String,
    left: u32,
    /// A part of each eye at full size instead of the whole a quarter size (x, y, width, height).
    crop: Option<[u32; 4]>,
    /// Only every this many frames, and how many until the next.
    every: u32,
    skip: u32,
    /// The mip level kept (each halves the size).
    mip: u32,
    /// Per eye: a copy of the submitted image with its mips (made the first frame).
    mips: Vec<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    /// Frames on their way to the CPU (read a frame or two later, never waited for): their row of
    /// numbers, both eyes, and each eye's size.
    waiting: VecDeque<(String, [ID3D11Texture2D; 2], [(u32, u32); 2])>,
    /// Frames read: their rows, their pictures on their way to the disk as they come (a thread
    /// of their own: a long film at a good size would fill the memory), and the first error.
    done: Vec<String>,
    writer: Option<(std::sync::mpsc::Sender<(usize, Image, Image)>, std::thread::JoinHandle<Result<()>>)>,
    format: DXGI_FORMAT,
}

impl Film {
    pub(crate) fn new(name: &str, frames: u32, crop: Option<[u32; 4]>, every: u32, mip: u32) -> Film {
        Film {
            name: name.to_string(),
            left: frames,
            crop,
            every: every.max(1),
            skip: 0,
            mip: mip.min(4),
            mips: Vec::new(),
            waiting: VecDeque::new(),
            done: Vec::new(),
            writer: None,
            format: DXGI_FORMAT_UNKNOWN,
        }
    }

    /// Frame `k` (both eyes) to the disk, on the writer's thread.
    fn write(&mut self, dir: PathBuf, k: usize, a: Image, b: Image) {
        let (tx, _) = self.writer.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<(usize, Image, Image)>();
            let thread = std::thread::spawn(move || -> Result<()> {
                std::fs::create_dir_all(&dir)?;
                for (k, a, b) in rx {
                    let both = side_by_side(&a, &b);
                    rustvr_gpu::save_png_fast(&dir.join(format!("{k:03}.png")), both.w, both.h, &both.px)?;
                }
                Ok(())
            });
            (tx, thread)
        });
        let _ = tx.send((k, a, b));
    }
}

/// A staging texture's pixels, if the GPU has them ready (`wait`: wait until it has).
fn read_staging(ctx: &ID3D11DeviceContext, t: &ID3D11Texture2D, (w, h): (u32, u32), bgra: bool, wait: bool) -> Option<Image> {
    unsafe {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        let flags = if wait { 0 } else { D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32 };
        ctx.Map(t, 0, D3D11_MAP_READ, flags, Some(&mut m)).ok()?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        for row in 0..h as usize {
            let src = std::slice::from_raw_parts((m.pData as *const u8).add(row * m.RowPitch as usize), w as usize * 4);
            px[row * w as usize * 4..(row + 1) * w as usize * 4].copy_from_slice(src);
        }
        ctx.Unmap(t, 0);
        for p in px.chunks_exact_mut(4) {
            if bgra {
                p.swap(0, 2);
            }
            p[3] = 255;
        }
        Some(Image { w, h, px })
    }
}

/// This frame into the film (from `xrEndFrame`); the film is saved when it's whole.
pub(crate) fn film_frame(r: &mut Rt) {
    let Some(mut f) = r.film.take() else { return };
    let took = (|| -> Result<bool> {
        let device = r.device.clone().context("no device")?;
        let ctx = unsafe { device.GetImmediateContext()? };
        let bgra = matches!(f.format, DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS);
        // The ones the GPU has done (all of them once the film is shot).
        let last = f.left == 0;
        while let Some((_, st, size)) = f.waiting.front() {
            let (Some(a), Some(b)) = (read_staging(&ctx, &st[0], size[0], bgra, last), read_staging(&ctx, &st[1], size[1], bgra, last)) else { break };
            let (row, _, _) = f.waiting.pop_front().unwrap();
            let k = f.done.len();
            f.done.push(row);
            f.write(r.dir.join("films").join(&f.name), k, a, b);
        }
        let take = f.skip == 0;
        f.skip = if take { f.every - 1 } else { f.skip - 1 };
        if f.left > 0 && take && r.last_layers.len() >= 2 {
            let layers = r.last_layers.clone();
            let mut st: Vec<ID3D11Texture2D> = Vec::new();
            let mut sizes = [(0, 0); 2];
            // Where the frame's pictures sit in the eyes' images, and the images' size, in the film's
            // pixels.
            let mut place = (0, 0, 0, 0);
            for (e, l) in layers.iter().take(2).enumerate() {
                let c = r.chains.get(&l.swapchain).context("the layer's swapchain is gone")?;
                let i = c.last_released.context("the layer's swapchain has no released image")?;
                let tex = &c.images[i as usize];
                f.format = DXGI_FORMAT(c.format as i32);
                if let Some([x, y, w, h]) = f.crop {
                    // A part at full size: straight from the submitted image.
                    let (x, y) = (x.min(c.width.saturating_sub(1)), y.min(c.height.saturating_sub(1)));
                    let (w, h) = (w.min(c.width - x).max(1), h.min(c.height - y).max(1));
                    let desc = D3D11_TEXTURE2D_DESC {
                        Width: w,
                        Height: h,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: f.format,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    };
                    let mut s = None;
                    let b = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
                    unsafe {
                        device.CreateTexture2D(&desc, None, Some(&mut s))?;
                        ctx.CopySubresourceRegion(s.as_ref().context("the film's staging texture")?, 0, 0, 0, 0, tex, 0, Some(&b));
                    }
                    st.push(s.context("the film's staging texture")?);
                    sizes[e] = (w, h);
                    place = (x, y, c.width, c.height);
                    continue;
                }
                place = (0, 0, (c.width >> f.mip).max(1), (c.height >> f.mip).max(1));
                if f.mips.len() <= e {
                    let desc = D3D11_TEXTURE2D_DESC {
                        Width: c.width,
                        Height: c.height,
                        MipLevels: f.mip + 1,
                        ArraySize: 1,
                        Format: f.format,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                        CPUAccessFlags: 0,
                        MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
                    };
                    let mut t = None;
                    unsafe { device.CreateTexture2D(&desc, None, Some(&mut t))? };
                    let t = t.context("the film's texture")?;
                    let mut v = None;
                    unsafe { device.CreateShaderResourceView(&t, None, Some(&mut v))? };
                    f.mips.push((t, v.context("the film's view")?));
                }
                let (mt, mv) = &f.mips[e];
                let (w, h) = ((c.width >> f.mip).max(1), (c.height >> f.mip).max(1));
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: f.format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut s = None;
                unsafe {
                    device.CreateTexture2D(&desc, None, Some(&mut s))?;
                    ctx.CopySubresourceRegion(mt, 0, 0, 0, 0, tex, 0, None);
                    ctx.GenerateMips(mv);
                }
                let s = s.context("the film's staging texture")?;
                unsafe { ctx.CopySubresourceRegion(&s, 0, 0, 0, 0, mt, f.mip, None) };
                st.push(s);
                sizes[e] = (w, h);
            }
            let q = |p: &xr::Posef| format!("{},{},{},{},{},{},{}", p.position.x, p.position.y, p.position.z, p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w);
            let fv = |v: &xr::Fovf| format!("{},{},{},{}", v.angle_left, v.angle_right, v.angle_up, v.angle_down);
            let hand = |h: usize| {
                let (p, o) = r.pose.hand_pose(h);
                format!("{},{},{},{},{},{},{}", p.x, p.y, p.z, o.x, o.y, o.z, o.w)
            };
            let (hp, hq) = (r.pose.head.pos, r.pose.head.rot());
            let row = format!(
                "{},{:.5},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                r.frames,
                r.script_time(),
                q(&layers[0].pose),
                fv(&layers[0].fov),
                q(&layers[1].pose),
                fv(&layers[1].fov),
                hand(0),
                hand(1),
                hp.x,
                hp.y,
                hp.z,
                hq.x,
                hq.y,
                hq.z,
                hq.w,
                place.2,
                place.3,
                place.0,
                place.1
            );
            let [a, b]: [ID3D11Texture2D; 2] = st.try_into().map_err(|_| anyhow::anyhow!("two eyes"))?;
            f.waiting.push_back((row, [a, b], sizes));
            f.left -= 1;
        }
        Ok(f.left == 0 && f.waiting.is_empty())
    })();
    match took {
        Ok(false) => r.film = Some(f),
        Ok(true) => {
            let began = std::time::Instant::now();
            let saved = save_film(r, &mut f);
            let took = began.elapsed();
            r.stalled += took;
            let msg = match saved {
                Ok(dir) => format!("film {} -> {} ({} frames, {:.2} s to save, script time {:.3})", f.name, dir.display(), f.done.len(), took.as_secs_f64(), r.script_time()),
                Err(e) => format!("film {}: {e:#}", f.name),
            };
            r.log(&msg);
        }
        Err(e) => {
            let msg = format!("film {}: {e:#}", f.name);
            r.log(&msg);
        }
    }
}

fn save_film(r: &Rt, f: &mut Film) -> Result<PathBuf> {
    let dir = r.dir.join("films").join(&f.name);
    std::fs::create_dir_all(&dir)?;
    // The pictures written (the last of them now).
    if let Some((tx, thread)) = f.writer.take() {
        drop(tx);
        thread.join().map_err(|_| anyhow::anyhow!("the film's writer panicked"))??;
    }
    let mut csv = String::from(
        "frame,t,l_px,l_py,l_pz,l_qx,l_qy,l_qz,l_qw,l_fl,l_fr,l_fu,l_fd,r_px,r_py,r_pz,r_qx,r_qy,r_qz,r_qw,r_fl,r_fr,r_fu,r_fd,lh_px,lh_py,lh_pz,lh_qx,lh_qy,lh_qz,lh_qw,rh_px,rh_py,rh_pz,rh_qx,rh_qy,rh_qz,rh_qw,head_px,head_py,head_pz,head_qx,head_qy,head_qz,head_qw,img_w,img_h,crop_x,crop_y\n",
    );
    for row in &f.done {
        csv.push_str(row);
        csv.push('\n');
    }
    std::fs::write(dir.join("frames.csv"), csv)?;
    Ok(dir)
}

/// Saves capture `name` from the frame just submitted. Returns the picture's path.
pub(crate) fn save(r: &mut Rt, name: &str) -> Result<PathBuf> {
    let dir = r.dir.join("captures");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.png"));
    let width = r.script.capture_width.max(64);
    let mut info = format!(
        "time = {:.3}\nframes = {}\nfocused = {}\nlayers = {}\nhead = {{ pos = {:?}, yaw = {}, pitch = {} }}\n",
        r.script_time(),
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
