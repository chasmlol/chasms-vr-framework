//! The range's effects rendered offscreen, close up and from the side (where a headset view would
//! see them end on), written to `captures/fx/` to look at, with checks that each one shows: a
//! shot's flash and light, its tracer, the casing on the floor, holes in a plate, an explosion's
//! flash, fire and smoke. Metro's sprites when they're extracted, the built-in ones otherwise.

#![allow(dead_code, clippy::too_many_arguments)]

#[path = "../src/content.rs"]
mod content;
#[path = "../src/effects.rs"]
mod effects;
#[path = "../src/hits.rs"]
mod hits;
#[path = "../src/render.rs"]
mod render;

use std::path::{Path, PathBuf};

use effects::{BitKind, Blast, Casing, Effects, GunFx, Sprites};
use glam::{Mat3, Mat4, Quat, Vec3};
use hits::{Surface, World};
use render::{Draw, Mesh, Renderer};
use rustvr_adapter::math::{projection, symmetric_fov, view_matrix};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use windows::Win32::Graphics::Dxgi::Common::*;

const SIZE: (u32, u32) = (640, 400);
const DT: f32 = 1.0 / 90.0;

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("captures").join("fx")
}

fn look_at(from: Vec3, to: Vec3) -> (Vec3, Quat) {
    let z = (to - from).normalize();
    let x = Vec3::Y.cross(z).normalize();
    (from, Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)))
}

struct Scene {
    gpu: Gpu,
    renderer: Renderer,
    room: Vec<Mesh>,
    casings: Vec<Mesh>,
    cube: Mesh,
    world: World,
}

impl Scene {
    fn new() -> Scene {
        let gpu = Gpu::new().unwrap();
        let mut renderer = Renderer::new(&gpu, rustvr_asset::default_root()).unwrap();
        let (grid, plain) = content::room();
        let mut room = vec![renderer.upload(&gpu, &grid).unwrap(), renderer.upload(&gpu, &plain).unwrap()];
        for t in content::targets() {
            room.push(renderer.upload(&gpu, &content::target_mesh(&t)).unwrap());
        }
        let casings = [Casing::Rifle, Casing::Pistol, Casing::Shell].iter().map(|c| renderer.upload(&gpu, &effects::casing_mesh(*c)).unwrap()).collect();
        let mut cube = content::MeshData::default();
        cube.add_box(Vec3::splat(-0.5), Vec3::splat(0.5), [255; 4]);
        let cube = renderer.upload(&gpu, &cube).unwrap();
        let mut boxes: Vec<(Vec3, Vec3, Surface)> = content::shell().into_iter().map(|(lo, hi)| (lo, hi, Surface::Stone)).collect();
        boxes.extend(content::furniture().into_iter().map(|(lo, hi, _)| (lo, hi, Surface::Wood)));
        let world = World::new(boxes, content::targets().iter().map(|t| (t.center, t.radius)).collect());
        Scene { gpu, renderer, room, casings, cube, world }
    }

    fn sprites(&mut self) -> Sprites {
        let (gpu, renderer) = (&self.gpu, &mut self.renderer);
        Sprites::load(|key, fallback| renderer.fx_texture(gpu, key, fallback))
    }

    /// The room and `fx` from a camera: the image (RGBA8, sRGB).
    fn shot(&mut self, name: &str, cam: (Vec3, Quat), fx: Option<&Effects>) -> Vec<u8> {
        let (w, h) = SIZE;
        let color = Texture::new(&self.gpu, &TexDesc::target(w, h, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
        let vp = projection(&symmetric_fov(70.0, w as f32 / h as f32), 0.03, 120.0) * view_matrix(cam.0, cam.1);
        let mut draws: Vec<Draw> = self.room.iter().map(|m| Draw { mesh: m, model: Mat4::IDENTITY, tint: [1.0; 4], emissive: 0.0, two_sided: false }).collect();
        let (sprites, lights) = fx.map(|f| (f.sprites(), f.lights())).unwrap_or_default();
        if let Some(f) = fx {
            for (kind, p, q, k) in f.bits() {
                let (mesh, model, tint) = match kind {
                    BitKind::Casing(c) => (&self.casings[c as usize], Mat4::from_scale_rotation_translation(Vec3::splat(k), q, p), [1.0; 4]),
                    BitKind::Debris => (&self.cube, Mat4::from_scale_rotation_translation(Vec3::splat(0.022 * k), q, p), [0.2, 0.18, 0.16, 1.0]),
                };
                draws.push(Draw { mesh, model, tint, emissive: 0.0, two_sided: false });
            }
        }
        self.renderer.render_scene(&self.gpu, color.rtv.as_ref().unwrap(), None, w, h, vp, cam.0, cam.1 * Vec3::Z, &draws, &sprites, &lights).unwrap();
        let px = color.read_rgba8(&self.gpu).unwrap();
        std::fs::create_dir_all(out_dir()).unwrap();
        rustvr_gpu::save_png(&out_dir().join(format!("{name}.png")), w, h, &px).unwrap();
        px
    }
}

/// Pixels that differ from `base` by more than `by` (0..255 summed over RGB).
fn changed(a: &[u8], base: &[u8], by: u32) -> usize {
    a.chunks(4).zip(base.chunks(4)).filter(|(p, q)| (0..3).map(|c| (p[c] as i32 - q[c] as i32).unsigned_abs()).sum::<u32>() > by).count()
}

/// Bright warm pixels (flashes, tracers, fire).
fn hot(a: &[u8]) -> usize {
    a.chunks(4).filter(|p| p[0] > 200 && p[1] > 120 && p[0] as i32 - p[2] as i32 > 40).count()
}

fn rifle() -> GunFx {
    GunFx { muzzle: Vec3::new(0.0, 0.03, 0.66), port: Some(Vec3::new(0.027, 0.05, 0.12)), casing: Casing::Rifle, flash: 1.0 }
}

/// A rifle at the hip of a player at the origin, aimed at the plate 10 m ahead.
fn gun() -> (Vec3, Quat) {
    let at = Vec3::new(0.22, 1.34, 0.3);
    look_at(at, Vec3::new(0.0, 1.6, 10.0))
}

#[test]
fn gun_effects_show_from_the_side() {
    let mut s = Scene::new();
    let sprites = s.sprites();
    eprintln!("sprites from the game: {}", sprites.flash.from_game);
    let side = look_at(Vec3::new(-1.6, 1.55, 1.2), Vec3::new(0.2, 1.4, 2.5));
    let base = s.shot("side_nothing", side, None);
    let mut fx = Effects::new(sprites);
    let (p, q) = gun();
    let fwd = q * Vec3::Z;
    let hit = s.world.raycast(p + q * rifle().muzzle, fwd, 150.0, &[]);
    assert!(matches!(hit.map(|h| h.what), Some(hits::HitWhat::Target(0))), "{hit:?}");
    fx.shot(&rifle(), (p, q), Vec3::ZERO, fwd, hit.as_ref());
    fx.update(DT, &s.world, &|_| None);
    let flash = s.shot("side_flash", side, Some(&fx));
    assert!(hot(&flash) > 40 + hot(&base), "a flash: {} hot pixels", hot(&flash));
    assert!(changed(&flash, &base, 30) > 400, "and its light: {} pixels changed", changed(&flash, &base, 30));
    // Two frames on: the flash is out, the tracer's on its way down range.
    fx.update(DT, &s.world, &|_| None);
    fx.update(DT, &s.world, &|_| None);
    let tracer = s.shot("side_tracer", side, Some(&fx));
    assert!(hot(&tracer) > 10, "a tracer: {} hot pixels", hot(&tracer));
    // The casing lands on the floor to the right.
    for _ in 0..200 {
        fx.update(DT, &s.world, &|_| None);
    }
    let brass = fx.bits().next().map(|b| b.1).expect("a casing");
    assert!(brass.y < 0.05 && brass.x > 0.5, "on the floor to the right: {brass:?}");
    let floor = look_at(brass + Vec3::new(-0.25, 0.35, -0.25), brass);
    let base = s.shot("casing_floor_nothing", floor, None);
    let img = s.shot("casing_floor", floor, Some(&fx));
    assert!(changed(&img, &base, 60) > 30, "the casing shows: {} pixels", changed(&img, &base, 60));
}

#[test]
fn bullet_holes_show_on_the_plate() {
    let mut s = Scene::new();
    let sprites = s.sprites();
    let mut fx = Effects::new(sprites);
    let (p, q) = gun();
    for k in 0..6 {
        let spread = Quat::from_euler(glam::EulerRot::YXZ, (k as f32 - 2.5) * 0.004, ((k % 3) as f32 - 1.0) * 0.006, 0.0);
        let fwd = (q * spread) * Vec3::Z;
        let hit = s.world.raycast(p + q * rifle().muzzle, fwd, 150.0, &[]);
        fx.shot(&rifle(), (p, q * spread), Vec3::ZERO, fwd, hit.as_ref());
    }
    for _ in 0..60 {
        fx.update(DT, &s.world, &|_| None);
    }
    let near = look_at(Vec3::new(0.25, 1.65, 8.9), Vec3::new(0.0, 1.6, 10.0));
    let base = s.shot("plate_nothing", near, None);
    let img = s.shot("plate_holes", near, Some(&fx));
    assert!(changed(&img, &base, 60) > 100, "holes in the plate: {} pixels", changed(&img, &base, 60));
}

#[test]
fn an_explosion_flashes_burns_and_smokes() {
    let mut s = Scene::new();
    let sprites = s.sprites();
    let mut fx = Effects::new(sprites);
    let eye = look_at(Vec3::new(0.0, 1.7, 0.0), Vec3::new(0.0, 0.8, 6.0));
    let base = s.shot("explosion_nothing", eye, None);
    fx.explosion(Vec3::new(0.3, 0.05, 6.0), Blast::Frag);
    fx.update(DT, &s.world, &|_| None);
    let flash = s.shot("explosion_flash", eye, Some(&fx));
    assert!(hot(&flash) > 500, "a big flash: {} hot pixels", hot(&flash));
    for _ in 0..30 {
        fx.update(DT, &s.world, &|_| None);
    }
    let fire = s.shot("explosion_fire", eye, Some(&fx));
    assert!(changed(&fire, &base, 40) > 2000, "fire and debris: {}", changed(&fire, &base, 40));
    for _ in 0..120 {
        fx.update(DT, &s.world, &|_| None);
    }
    let smoke = s.shot("explosion_smoke", eye, Some(&fx));
    assert!(changed(&smoke, &base, 25) > 2000, "smoke: {}", changed(&smoke, &base, 25));
    let mut fx = Effects::new(s.sprites());
    fx.explosion(Vec3::new(0.3, 0.05, 6.0), Blast::Fire);
    for _ in 0..60 {
        fx.update(DT, &s.world, &|_| None);
    }
    let burning = s.shot("incendiary_burning", eye, Some(&fx));
    assert!(hot(&burning) > 100, "it burns: {}", hot(&burning));
}
