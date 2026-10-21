//! The range's visual effects: what a game shows when its guns fire and its grenades go off (the
//! range is the game here; in Metro the game draws its own). Metro's own sprites when they're
//! extracted (`metro-extract --extras`: its muzzle flash `pfx_muzzle1`, muzzle gas, sparks
//! `part_iskry`, the explosion's fireball `part_flame_1`, star and smoke, dust, glow, a wallmark
//! bullet hole and the `fx_tracer1` streak), built-in ones otherwise.
//!
//! * **A shot**: a flash at the muzzle (a sprite facing you and one along the barrel, two frames)
//!   and a light that brightens the gun, hands and table for 60 ms; a puff of gas drifting off;
//!   a tracer streaking to whatever the bullet hit; and a casing out of the ejection port (right,
//!   up and a little back, spinning) that bounces on the floor and tables and fades after a few
//!   seconds. Magazine-fed rifles and pistols eject; revolvers, break-actions and air guns don't.
//! * **A hit**: a hole on the surface (riding with an object it's in; at most 64, the oldest
//!   fading out first), sparks off metal and target plates, a puff of dust and chips off wood
//!   and stone.
//! * **An explosion**: a flash and a light (10 m), a fireball, sparks and debris thrown out (the
//!   debris bounces), smoke rolling up for a few seconds and a scorch mark; an incendiary burns
//!   on for a few seconds instead.
//!
//! It's all cheap sprites and a few small meshes (the range renders offscreen in tests): a few
//! hundred quads at most.

use glam::{Quat, Vec2, Vec3};
use std::collections::VecDeque;

use crate::hits::{Hit, HitWhat, Surface, World};
use crate::render::{FxBlend, FxDraw, FxShape, Light};

pub const MAX_DECALS: usize = 64;
pub const MAX_CASINGS: usize = 48;
pub const MAX_PARTICLES: usize = 600;
/// Bullet holes last this long (the oldest fade early when there are too many).
pub const DECAL_LIFE: f32 = 25.0;
pub const CASING_LIFE: f32 = 6.0;
/// How fast a tracer is drawn travelling (slowed from a bullet's 700 m/s so it's seen).
pub const TRACER_SPEED: f32 = 180.0;
/// A tracer's streak (m).
pub const TRACER_LENGTH: f32 = 2.0;

/// An effect sprite: its texture, how it's cut into frames (columns, rows), and whether it's the
/// game's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sprite {
    pub tex: usize,
    pub grid: (u32, u32),
    pub from_game: bool,
}

impl Sprite {
    /// The uv rectangle of frame `k` (row by row).
    pub fn frame(&self, k: u32) -> [f32; 4] {
        let (c, r) = (self.grid.0.max(1), self.grid.1.max(1));
        let k = k % (c * r);
        let (x, y) = ((k % c) as f32, (k / c) as f32);
        let (w, h) = (1.0 / c as f32, 1.0 / r as f32);
        [x * w, y * h, (x + 1.0) * w, (y + 1.0) * h]
    }

    pub fn frames(&self) -> u32 {
        self.grid.0.max(1) * self.grid.1.max(1)
    }
}

/// Every sprite the effects use.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sprites {
    pub flash: Sprite,
    pub gas: Sprite,
    pub sparks: Sprite,
    pub fireball: Sprite,
    pub smoke: Sprite,
    pub dust: Sprite,
    pub star: Sprite,
    pub glow: Sprite,
    pub hole: Sprite,
    pub tracer: Sprite,
    /// The tracer's cell in its atlas.
    pub tracer_uv: [f32; 4],
    pub scorch: Sprite,
    pub dot: Sprite,
}

impl Sprites {
    /// Resolves each sprite with `load(game texture key, built-in fallback) -> (id, from the game)`.
    pub fn load(mut load: impl FnMut(&str, &str) -> (usize, bool)) -> Sprites {
        let mut sprite = |key: &str, fallback: &str, grid: (u32, u32)| {
            let (tex, from_game) = load(key, fallback);
            Sprite { tex, grid: if from_game { grid } else { (1, 1) }, from_game }
        };
        let tracer = sprite("fx__fx_tracer1", "fx_streak", (8, 8));
        Sprites {
            flash: sprite("pfx__pfx_muzzle1", "fx_flash", (1, 1)),
            gas: sprite("pfx__pfx_muzzle_gas1", "fx_smoke", (2, 2)),
            sparks: sprite("part__part_iskry", "fx_soft", (4, 4)),
            fireball: sprite("part__part_flame_1", "fx_flash", (8, 8)),
            smoke: sprite("part__part_expl_smoke2", "fx_smoke", (8, 8)),
            dust: sprite("part__part_hit_dust", "fx_smoke", (1, 1)),
            star: sprite("part__part_expl_star", "fx_flash", (1, 1)),
            glow: sprite("part__part_glow", "fx_soft", (1, 1)),
            hole: sprite("wm__wm_bullet_metall_thin_1", "fx_hole", (1, 1)),
            // Metro's orange streak is the third cell down its first column, its head to the right.
            tracer_uv: if tracer.from_game { [0.0, 0.25, 0.125, 0.375] } else { [0.0, 0.0, 1.0, 1.0] },
            tracer: Sprite { grid: (1, 1), ..tracer },
            scorch: sprite("fx_scorch", "fx_scorch", (1, 1)),
            dot: sprite("fx_soft", "fx_soft", (1, 1)),
        }
    }
}

/// How a gun shows its shots, in weapon space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunFx {
    pub muzzle: Vec3,
    /// The ejection port and which way casings leave it (None: it keeps them).
    pub port: Option<Vec3>,
    pub casing: Casing,
    /// The flash's size (1: a rifle's).
    pub flash: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Casing {
    Rifle,
    Pistol,
    Shell,
}

impl Casing {
    /// Length and radius (m).
    pub fn size(self) -> (f32, f32) {
        match self {
            Casing::Rifle => (0.039, 0.0052),
            Casing::Pistol => (0.019, 0.0049),
            Casing::Shell => (0.065, 0.0105),
        }
    }
}

/// Guns that keep their brass: revolvers, break-actions, air and energy guns, flamethrowers.
pub fn keeps_casings(key: &str) -> bool {
    ["revolver", "duplet", "ashot", "uboynicheg", "helsing", "tihar", "railgun", "ognemet"].iter().any(|k| key.contains(k))
}

/// How a gun shows its shots: its muzzle, and its ejection port from the game's own locator
/// (`loc_shell_point`, which every Metro gun model has) or, for the built-in guns, the right side
/// of the receiver (rifle) or the top of the slide (pistol).
pub fn gun_fx(w: &crate::content::Weapon, assets: &std::path::Path) -> GunFx {
    let port = if keeps_casings(&w.key) {
        None
    } else if w.from_game {
        rustvr_asset::load_model(&assets.join("metro2033"), &w.key)
            .ok()
            .and_then(|m| m.bones.iter().find(|b| b.name == "loc_shell_point").map(|b| glam::Mat4::from_cols_array(&b.model_matrix).w_axis.truncate()))
            .map(|p| w.fix.transform_point3(p))
    } else if w.pistol {
        Some(Vec3::new(0.012, 0.042, 0.04))
    } else {
        Some(Vec3::new(0.027, 0.05, 0.12))
    };
    let casing = if w.key.contains("saiga") || w.key.contains("shot") {
        Casing::Shell
    } else if w.pistol {
        Casing::Pistol
    } else {
        Casing::Rifle
    };
    GunFx { muzzle: Vec3::from_array(w.rig.muzzle), port, casing, flash: if w.pistol { 0.7 } else { 1.0 } }
}

/// A casing's mesh: a brass tube along +Z (a shotgun shell's red, with a brass head).
pub fn casing_mesh(c: Casing) -> crate::content::MeshData {
    let (len, r) = c.size();
    let mut m = crate::content::MeshData::default();
    let brass = [196, 160, 70, 255];
    match c {
        Casing::Shell => {
            m.add_rounded_prism(Vec3::new(0.0, 0.0, -len * 0.4), Vec3::new(r, r, len * 0.1), Quat::IDENTITY, brass);
            m.add_rounded_prism(Vec3::new(0.0, 0.0, len * 0.1), Vec3::new(r * 0.96, r * 0.96, len * 0.4), Quat::IDENTITY, [150, 30, 25, 255]);
        }
        _ => {
            m.add_rounded_prism(Vec3::new(0.0, 0.0, -len * 0.1), Vec3::new(r, r, len * 0.4), Quat::IDENTITY, brass);
            // The neck.
            m.add_rounded_prism(Vec3::new(0.0, 0.0, len * 0.4), Vec3::new(r * 0.7, r * 0.7, len * 0.1), Quat::IDENTITY, brass);
        }
    }
    m
}

#[derive(Clone, Copy, Debug)]
struct Particle {
    sprite: Sprite,
    blend: FxBlend,
    pos: Vec3,
    vel: Vec3,
    /// Fraction of velocity kept per second, and gravity (m/s^2, down).
    drag: f32,
    gravity: f32,
    size: (f32, f32),
    roll: f32,
    spin: f32,
    color: [f32; 4],
    t: f32,
    life: f32,
    /// Plays its sprite's frames over its life (else stays on `frame`).
    animate: bool,
    frame: u32,
    /// Drawn along its velocity (a spark's streak) this many seconds long.
    streak: f32,
    /// Stays in a world point's frame: a flash is drawn at the muzzle it left.
    fade_in: f32,
}

#[derive(Clone, Copy, Debug)]
struct Decal {
    sprite: Sprite,
    pos: Vec3,
    rot: Quat,
    size: f32,
    color: [f32; 3],
    t: f32,
    life: f32,
    /// Stuck to an object: its id and the pose relative to it (once it's been seen).
    on: Option<(u32, Option<(Vec3, Quat)>)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Bit {
    pub pos: Vec3,
    pub rot: Quat,
    vel: Vec3,
    spin: Vec3,
    pub t: f32,
    life: f32,
    radius: f32,
    resting: bool,
    pub kind: BitKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitKind {
    Casing(Casing),
    /// A chip of debris (a dark cube).
    Debris,
}

#[derive(Clone, Copy, Debug)]
struct Tracer {
    from: Vec3,
    dir: Vec3,
    dist: f32,
    t: f32,
}

#[derive(Clone, Copy, Debug)]
struct Flash {
    light: Light,
    t: f32,
    life: f32,
    flicker: bool,
}

/// What blew up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blast {
    /// High explosive: a frag grenade, dynamite.
    Frag,
    /// Incendiary: burns where it lands.
    Fire,
}

pub struct Effects {
    pub sprites: Sprites,
    particles: Vec<Particle>,
    decals: VecDeque<Decal>,
    bits: Vec<Bit>,
    tracers: Vec<Tracer>,
    flashes: Vec<Flash>,
    /// Incendiary fires burning: where, since when.
    fires: Vec<(Vec3, f32)>,
    rng: u32,
    pub stats: FxStats,
}

/// Counts for the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FxStats {
    pub flashes: u32,
    pub tracers: u32,
    pub casings: u32,
    pub holes: u32,
    pub explosions: u32,
}

impl Effects {
    pub fn new(sprites: Sprites) -> Effects {
        Effects { sprites, particles: Vec::new(), decals: VecDeque::new(), bits: Vec::new(), tracers: Vec::new(), flashes: Vec::new(), fires: Vec::new(), rng: 0x9E37_79B9, stats: FxStats::default() }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.rand()
    }

    fn unit(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0));
            if v.length_squared() > 0.01 && v.length_squared() <= 1.0 {
                return v.normalize();
            }
        }
    }

    fn push(&mut self, p: Particle) {
        if self.particles.len() < MAX_PARTICLES {
            self.particles.push(p);
        }
    }

    fn particle(sprite: Sprite, blend: FxBlend, pos: Vec3, size: f32, life: f32, color: [f32; 4]) -> Particle {
        Particle { sprite, blend, pos, vel: Vec3::ZERO, drag: 1.0, gravity: 0.0, size: (size, size), roll: 0.0, spin: 0.0, color, t: 0.0, life, animate: false, frame: 0, streak: 0.0, fade_in: 0.0 }
    }

    /// A chip of debris thrown off at `vel`, tumbling.
    fn chip(&mut self, pos: Vec3, vel: Vec3, life: f32, radius: f32, spin: f32) {
        let rot = Quat::from_axis_angle(self.unit(), self.range(0.0, 3.0));
        let spin = self.unit() * spin;
        self.bits.push(Bit { pos, rot, vel, spin, t: 0.0, life, radius, resting: false, kind: BitKind::Debris });
    }

    fn light(&mut self, pos: Vec3, color: Vec3, radius: f32, life: f32, flicker: bool) {
        self.flashes.push(Flash { light: Light { pos, color, radius }, t: 0.0, life, flicker });
    }

    /// A gun at `gun` (world pose) fired a round that hit `hit` (or nothing). `vel`: the gun's
    /// velocity (the casing leaves with it).
    pub fn shot(&mut self, fx: &GunFx, gun: (Vec3, Quat), vel: Vec3, dir: Vec3, hit: Option<&Hit>) {
        let (p, q) = gun;
        let muzzle = p + q * fx.muzzle;
        let fwd = dir.normalize_or(q * Vec3::Z);
        self.stats.flashes += 1;
        let s = fx.flash;
        // The flash: a star facing you and a plume along the barrel, for two frames.
        let roll = self.range(0.0, std::f32::consts::TAU);
        let mut front = Self::particle(self.sprites.flash, FxBlend::Add, muzzle + fwd * 0.03, 0.26 * s, 0.035, [1.6, 1.4, 1.2, 1.0]);
        front.roll = roll;
        self.push(front);
        let side = Particle { size: (0.42 * s, 0.16 * s), streak: -1.0, vel: fwd, ..Self::particle(self.sprites.flash, FxBlend::Add, muzzle + fwd * 0.15 * s, 0.0, 0.035, [1.4, 1.2, 1.0, 0.9]) };
        self.push(side);
        self.light(muzzle + fwd * 0.05, Vec3::new(5.0, 3.2, 1.4) * s, 4.0, 0.06, false);
        // The hot gas (Metro's gas sprites are lit, so they add) and then smoke drifting off.
        let frame = (self.rand() * self.sprites.gas.frames() as f32) as u32;
        let gas = Particle { size: (0.3 * s, 0.4 * s), streak: -1.0, vel: fwd * 2.0, frame, ..Self::particle(self.sprites.gas, FxBlend::Add, muzzle + fwd * 0.18 * s, 0.0, 0.12, [0.5, 0.45, 0.4, 0.8]) };
        self.push(gas);
        for k in 0..3 {
            let mut g = Self::particle(self.sprites.dust, FxBlend::Alpha, muzzle + fwd * (0.04 + 0.06 * k as f32), 0.06, self.range(0.9, 1.4), [0.72, 0.72, 0.72, 0.32]);
            g.vel = fwd * self.range(0.3, 1.0) + Vec3::Y * 0.18 + self.unit() * 0.08;
            g.drag = 0.15;
            g.size = (0.06, 0.45);
            g.roll = self.range(0.0, 6.28);
            g.spin = self.range(-0.6, 0.6);
            g.fade_in = 0.05;
            self.push(g);
        }
        // The tracer, from the muzzle to the hit (the bullet left along the sights, a few
        // centimetres above).
        let (tdir, dist) = match hit {
            Some(h) => ((h.pos - muzzle).normalize_or(fwd), h.pos.distance(muzzle)),
            None => (fwd, 120.0),
        };
        self.tracers.push(Tracer { from: muzzle + tdir * 0.1, dir: tdir, dist: (dist - 0.1).max(0.0), t: 0.0 });
        self.stats.tracers += 1;
        // The casing.
        if let Some(port) = fx.port {
            let (len, r) = fx.casing.size();
            let out = q * Vec3::new(self.range(1.8, 2.6), self.range(1.1, 1.7), self.range(-0.7, -0.3));
            let spin = q * Vec3::new(self.range(-8.0, 8.0), self.range(18.0, 34.0), self.range(-6.0, 6.0));
            // Lying across the port, its mouth forward.
            let rot = (q * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).normalize();
            if self.bits.iter().filter(|b| matches!(b.kind, BitKind::Casing(_))).count() >= MAX_CASINGS
                && let Some(i) = self.bits.iter().position(|b| matches!(b.kind, BitKind::Casing(_)))
            {
                self.bits.remove(i);
            }
            self.bits.push(Bit { pos: p + q * port, rot, vel: vel + out, spin, t: 0.0, life: CASING_LIFE, radius: r.max(len * 0.25), resting: false, kind: BitKind::Casing(fx.casing) });
            self.stats.casings += 1;
        }
        if let Some(h) = hit {
            self.impact(h, fwd);
        }
    }

    /// A bullet hit something: a hole, and sparks or dust.
    pub fn impact(&mut self, hit: &Hit, dir: Vec3) {
        let n = hit.normal.normalize_or(-dir);
        let at = hit.pos + n * 0.002;
        let s = hit.surface();
        // The hole, turned randomly about the normal.
        let roll = Quat::from_rotation_z(self.range(0.0, 6.28));
        let rot = (Quat::from_rotation_arc(Vec3::Z, n) * roll).normalize();
        let size = match s {
            Surface::Target => 0.03,
            Surface::Metal => 0.03,
            _ => self.range(0.04, 0.055),
        };
        let color = match s {
            Surface::Wood => [0.55, 0.45, 0.35],
            _ => [1.0, 1.0, 1.0],
        };
        let on = match hit.what {
            HitWhat::Object(id, _) => Some((id, None)),
            _ => None,
        };
        self.decals.push_back(Decal { sprite: self.sprites.hole, pos: at, rot, size, color, t: 0.0, life: DECAL_LIFE, on });
        self.stats.holes += 1;
        while self.decals.len() > MAX_DECALS {
            self.decals.pop_front();
        }
        // The oldest few fade early as the cap nears.
        let n_decals = self.decals.len();
        if n_decals > MAX_DECALS - 8 {
            for d in self.decals.iter_mut().take(n_decals - (MAX_DECALS - 8)) {
                d.life = d.life.min(d.t + 1.0);
            }
        }
        let bounce = (dir - 2.0 * dir.dot(n) * n).normalize_or(n);
        match s {
            Surface::Metal | Surface::Target => {
                let mut flare = Self::particle(self.sprites.sparks, FxBlend::Add, at + n * 0.02, 0.18, 0.12, [1.5, 1.2, 0.8, 1.0]);
                flare.animate = true;
                flare.roll = self.range(0.0, 6.28);
                self.push(flare);
                for _ in 0..7 {
                    let v = (bounce + n * 0.4 + self.unit() * 0.6).normalize() * self.range(2.0, 6.0);
                    let spark = Particle { vel: v, gravity: 9.81, drag: 0.6, streak: 0.012, ..Self::particle(self.sprites.dot, FxBlend::Add, at, 0.012, self.range(0.15, 0.35), [3.0, 2.0, 0.8, 1.0]) };
                    self.push(spark);
                }
                self.light(at + n * 0.05, Vec3::new(1.6, 1.0, 0.5), 1.2, 0.05, false);
            }
            Surface::Wood | Surface::Stone => {
                let tint = if s == Surface::Wood { [0.55, 0.45, 0.32, 0.55] } else { [0.62, 0.6, 0.57, 0.5] };
                for k in 0..3 {
                    let mut d = Self::particle(self.sprites.dust, FxBlend::Alpha, at + n * (0.02 + 0.03 * k as f32), 0.06, self.range(0.5, 0.9), tint);
                    d.vel = n * self.range(0.3, 0.9) + self.unit() * 0.2;
                    d.drag = 0.2;
                    d.size = (0.06, 0.35);
                    d.roll = self.range(0.0, 6.28);
                    self.push(d);
                }
                for _ in 0..3 {
                    let v = (bounce + n * 0.8 + self.unit() * 0.5).normalize() * self.range(1.0, 3.0);
                    self.chip(at + n * 0.01, v, 1.5, 0.004, 15.0);
                }
            }
        }
    }

    /// A knife went in at `at` (a little puff, sparks off a target plate).
    pub fn knife_hit(&mut self, at: Vec3, metal: bool) {
        if metal {
            let mut flare = Self::particle(self.sprites.sparks, FxBlend::Add, at, 0.14, 0.1, [1.5, 1.2, 0.8, 1.0]);
            flare.animate = true;
            self.push(flare);
        } else {
            let mut d = Self::particle(self.sprites.dust, FxBlend::Alpha, at, 0.05, 0.5, [0.55, 0.45, 0.32, 0.4]);
            d.size = (0.05, 0.2);
            self.push(d);
        }
    }

    /// A lit fuse at `at` sputters (call every frame it burns).
    pub fn fuse(&mut self, at: Vec3) {
        let mut s = Self::particle(self.sprites.sparks, FxBlend::Add, at, 0.05, 0.08, [1.6, 1.1, 0.6, 1.0]);
        s.animate = true;
        s.roll = self.range(0.0, 6.28);
        self.push(s);
        if self.rand() < 0.4 {
            let v = (Vec3::Y + self.unit()).normalize() * self.range(0.5, 1.5);
            self.push(Particle { vel: v, gravity: 9.81, streak: 0.01, ..Self::particle(self.sprites.dot, FxBlend::Add, at, 0.006, 0.25, [3.0, 2.0, 0.8, 1.0]) });
        }
        if self.flashes.iter().all(|f| f.light.pos.distance(at) > 0.2 || f.t > 0.0) {
            self.light(at, Vec3::new(0.8, 0.5, 0.2), 1.2, 0.02, true);
        }
    }

    /// A blinking light (a grenade's countdown LED).
    pub fn blink(&mut self, at: Vec3, color: [f32; 3]) {
        self.push(Self::particle(self.sprites.glow, FxBlend::Add, at, 0.05, 0.02, [color[0], color[1], color[2], 1.0]));
    }

    /// Something blew up at `at` (on or near the ground).
    pub fn explosion(&mut self, at: Vec3, kind: Blast) {
        self.stats.explosions += 1;
        let up = Vec3::Y;
        let fire = kind == Blast::Fire;
        self.light(at + up * 0.4, if fire { Vec3::new(9.0, 4.5, 1.5) } else { Vec3::new(14.0, 9.0, 4.0) }, 10.0, if fire { 0.6 } else { 0.3 }, false);
        if !fire {
            self.push(Self::particle(self.sprites.star, FxBlend::Add, at + up * 0.3, 3.2, 0.18, [2.0, 1.8, 1.5, 1.0]));
        }
        // The fireball: the game's animated one (fire into smoke), or flashes.
        for k in 0..if fire { 4 } else { 6 } {
            let mut f = Self::particle(self.sprites.fireball, if self.sprites.fireball.from_game { FxBlend::Alpha } else { FxBlend::Add }, at + up * (0.25 + 0.15 * k as f32) + self.unit() * 0.3, 1.0, self.range(0.8, 1.2), [1.3, 1.2, 1.1, 1.0]);
            f.animate = true;
            f.size = (0.9, 2.2);
            f.vel = up * self.range(0.5, 1.5) + self.unit() * 0.6;
            f.drag = 0.5;
            f.roll = self.range(0.0, 6.28);
            f.spin = self.range(-0.5, 0.5);
            self.push(f);
        }
        // Sparks and debris out of it.
        let n_sparks = if fire { 12 } else { 36 };
        for _ in 0..n_sparks {
            let v = (self.unit() + up * 0.8).normalize() * self.range(4.0, 14.0);
            let life = self.range(0.4, 1.0);
            self.push(Particle { vel: v, gravity: 9.81, drag: 0.4, streak: 0.02, ..Self::particle(self.sprites.dot, FxBlend::Add, at + up * 0.1, 0.02, life, [3.0, 1.9, 0.6, 1.0]) });
        }
        if !fire {
            for _ in 0..14 {
                let v = (self.unit() + up * 1.2).normalize() * self.range(3.0, 9.0);
                self.chip(at + up * 0.1, v, 4.0, 0.012, 20.0);
            }
        }
        // Smoke rolling up and out.
        for k in 0..12 {
            let life = self.range(3.0, 5.0);
            let mut s = Self::particle(self.sprites.smoke, FxBlend::Alpha, at + up * (0.2 + 0.15 * k as f32) + self.unit() * 0.6, 1.4, life, [0.36, 0.35, 0.33, 0.8]);
            s.animate = true;
            s.size = (1.4, 4.0);
            s.vel = up * self.range(0.5, 1.4) + self.unit() * 0.8;
            s.drag = 0.35;
            s.roll = self.range(0.0, 6.28);
            s.spin = self.range(-0.3, 0.3);
            s.fade_in = 0.2;
            self.push(s);
        }
        // A scorch mark on the ground below.
        let rot = Quat::from_rotation_arc(Vec3::Z, Vec3::Y) * Quat::from_rotation_z(self.range(0.0, 6.28));
        self.decals.push_back(Decal { sprite: self.sprites.scorch, pos: Vec3::new(at.x, 0.003, at.z), rot, size: if fire { 2.2 } else { 1.8 }, color: [1.0; 3], t: 0.0, life: 40.0, on: None });
        while self.decals.len() > MAX_DECALS {
            self.decals.pop_front();
        }
        if fire {
            self.fires.push((Vec3::new(at.x, 0.0, at.z), 0.0));
        }
    }

    /// Moves everything on: `objects` says where the objects holes are in are now (id -> pose).
    pub fn update(&mut self, dt: f32, world: &World, objects: &dyn Fn(u32) -> Option<(Vec3, Quat)>) {
        let dt = dt.clamp(0.0, 0.1);
        for p in &mut self.particles {
            p.t += dt;
            p.vel *= p.drag.powf(dt);
            p.vel.y -= p.gravity * dt;
            p.pos += p.vel * dt;
            p.roll += p.spin * dt;
        }
        self.particles.retain(|p| p.t < p.life);
        for t in &mut self.tracers {
            t.t += dt;
        }
        self.tracers.retain(|t| t.t * TRACER_SPEED < t.dist + TRACER_LENGTH);
        for f in &mut self.flashes {
            f.t += dt;
        }
        self.flashes.retain(|f| f.t < f.life);
        for d in &mut self.decals {
            d.t += dt;
            if let Some((id, rel)) = &mut d.on {
                match (objects(*id), *rel) {
                    // Where it is on the object, the first time it's seen.
                    (Some((op, oq)), None) => *rel = Some((oq.inverse() * (d.pos - op), (oq.inverse() * d.rot).normalize())),
                    (Some((op, oq)), Some((rp, rq))) => {
                        d.pos = op + oq * rp;
                        d.rot = (oq * rq).normalize();
                    }
                    // The object's gone (or put away): so's the hole.
                    (None, _) => d.life = d.life.min(d.t),
                }
            }
        }
        self.decals.retain(|d| d.t < d.life);
        for b in &mut self.bits {
            b.t += dt;
            if b.resting {
                continue;
            }
            b.vel.y -= 9.81 * dt;
            b.pos += b.vel * dt;
            let w = b.spin.length();
            if w > 1e-3 {
                b.rot = (Quat::from_axis_angle(b.spin / w, w * dt) * b.rot).normalize();
            }
            if let Some((push, n, s)) = world.collide(b.pos, b.radius) {
                b.pos += push;
                let vn = b.vel.dot(n);
                if vn < 0.0 {
                    // Brass rings off stone and metal, thuds on wood.
                    let e = if s == Surface::Wood { 0.2 } else { 0.3 };
                    let tangent = b.vel - n * vn;
                    b.vel = tangent * 0.6 - n * vn * e;
                    b.spin *= 0.5;
                }
                if b.vel.length() < 0.15 {
                    b.resting = true;
                    // Lying on its side.
                    let axis = b.rot * Vec3::Z;
                    let flat = (axis - n * axis.dot(n)).normalize_or(Vec3::X);
                    b.rot = Quat::from_rotation_arc(b.rot * Vec3::Z, flat) * b.rot;
                }
            }
        }
        self.bits.retain(|b| b.t < b.life);
        for f in &mut self.fires {
            f.1 += dt;
        }
        self.fires.retain(|f| f.1 < 6.0);
        // Burning fires keep sending up flames and smoke.
        let fires = self.fires.clone();
        for (at, t) in fires {
            if self.rand() < 0.7 {
                let mut f = Self::particle(self.sprites.fireball, if self.sprites.fireball.from_game { FxBlend::Alpha } else { FxBlend::Add }, at + self.unit() * 0.7 * Vec3::new(1.0, 0.0, 1.0) + Vec3::Y * 0.25, 0.5, 0.7, [1.3, 1.15, 1.0, (1.0 - t / 6.0).max(0.25)]);
                f.animate = true;
                f.vel = Vec3::Y * self.range(0.8, 1.6);
                f.size = (0.55, 1.4);
                self.push(f);
            }
            if self.flashes.iter().all(|f| !f.flicker || f.light.pos.distance(at) > 0.5) {
                self.light(at + Vec3::Y * 0.5, Vec3::new(5.0, 2.5, 0.8) * (1.0 - t / 6.0), 6.0, 0.08, true);
            }
        }
    }

    /// Everything alive.
    pub fn live(&self) -> usize {
        self.particles.len() + self.decals.len() + self.bits.len() + self.tracers.len() + self.flashes.len()
    }

    #[allow(dead_code)] // (the tests)
    pub fn casings(&self) -> impl Iterator<Item = &Bit> {
        self.bits.iter().filter(|b| matches!(b.kind, BitKind::Casing(_)))
    }

    #[allow(dead_code)]
    pub fn decal_count(&self) -> usize {
        self.decals.len()
    }

    /// The sprites to draw this frame.
    pub fn sprites(&self) -> Vec<FxDraw> {
        let mut out = Vec::with_capacity(self.particles.len() + self.decals.len() + self.tracers.len());
        for p in &self.particles {
            let k = (p.t / p.life).clamp(0.0, 1.0);
            let size = p.size.0 + (p.size.1 - p.size.0) * k.sqrt();
            let fade = if p.fade_in > 0.0 { (p.t / p.fade_in).min(1.0) } else { 1.0 } * (1.0 - k).powf(if p.blend == FxBlend::Add { 1.0 } else { 1.5 });
            let frame = if p.animate { (k * p.sprite.frames() as f32) as u32 } else { p.frame };
            let (shape, dims, pos) = if p.streak > 0.0 && p.vel.length() > 0.1 {
                let len = (p.vel.length() * p.streak).max(size);
                (FxShape::Axial(p.vel), Vec2::new(len, size), p.pos - p.vel * p.streak * 0.5)
            } else if p.streak < 0.0 {
                (FxShape::Axial(p.vel), Vec2::new(p.size.0, p.size.1), p.pos)
            } else {
                (FxShape::Billboard { roll: p.roll }, Vec2::splat(size), p.pos)
            };
            let c = p.color;
            out.push(FxDraw { tex: p.sprite.tex, pos, size: dims, shape, color: [c[0], c[1], c[2], c[3] * fade], uv: p.sprite.frame(frame), blend: p.blend });
        }
        for t in &self.tracers {
            // The first frame it's already on its way (it left between frames).
            let travelled = (t.t + 1.0 / 90.0) * TRACER_SPEED;
            let head = travelled.min(t.dist);
            let tail = (travelled - TRACER_LENGTH).clamp(0.0, t.dist);
            if head - tail < 0.05 {
                continue;
            }
            let mid = t.from + t.dir * (0.5 * (head + tail));
            out.push(FxDraw { tex: self.sprites.tracer.tex, pos: mid, size: Vec2::new(head - tail, 0.06), shape: FxShape::Axial(t.dir), color: [2.6, 2.0, 1.2, 1.0], uv: self.sprites.tracer_uv, blend: FxBlend::Add });
            // Its glowing head: from behind (your own shots) the streak is end on, a receding dot.
            out.push(FxDraw { tex: self.sprites.glow.tex, pos: t.from + t.dir * head, size: Vec2::splat(0.09), shape: FxShape::Billboard { roll: 0.0 }, color: [2.4, 1.6, 0.7, 1.0], uv: [0.0, 0.0, 1.0, 1.0], blend: FxBlend::Add });
        }
        for d in &self.decals {
            let fade = ((d.life - d.t) / 2.0).clamp(0.0, 1.0);
            out.push(FxDraw { tex: d.sprite.tex, pos: d.pos, size: Vec2::splat(d.size), shape: FxShape::Fixed(d.rot), color: [d.color[0], d.color[1], d.color[2], fade], uv: [0.0, 0.0, 1.0, 1.0], blend: FxBlend::Alpha });
        }
        out
    }

    /// The flashes' lights this frame, brightest first.
    pub fn lights(&self) -> Vec<Light> {
        let mut out: Vec<Light> = self
            .flashes
            .iter()
            .map(|f| {
                let k = 1.0 - f.t / f.life;
                let flicker = if f.flicker { 0.75 + 0.25 * ((f.t * 53.0).sin() * (f.t * 31.0).cos()).abs() } else { 1.0 };
                Light { color: f.light.color * k * flicker, ..f.light }
            })
            .collect();
        out.sort_by(|a, b| (b.color.length() * b.radius).total_cmp(&(a.color.length() * a.radius)));
        out
    }

    /// The casings and debris (meshes): kind, position, rotation, scale (they shrink away at the end).
    pub fn bits(&self) -> impl Iterator<Item = (BitKind, Vec3, Quat, f32)> + '_ {
        self.bits.iter().map(|b| (b.kind, b.pos, b.rot, ((b.life - b.t) / 0.6).clamp(0.0, 1.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprites() -> Sprites {
        let mut n = 0;
        Sprites::load(|_, _| {
            n += 1;
            (n, false)
        })
    }

    fn room() -> World {
        World::new(vec![(Vec3::new(-10.0, -0.2, -5.0), Vec3::new(10.0, 0.0, 25.0), Surface::Stone), (Vec3::new(0.4, 0.0, 0.3), Vec3::new(1.5, 0.85, 1.4), Surface::Wood)], vec![(Vec3::new(0.0, 1.6, 10.0), 0.35)])
    }

    fn rifle() -> GunFx {
        GunFx { muzzle: Vec3::new(0.0, 0.03, 0.66), port: Some(Vec3::new(0.026, 0.05, 0.12)), casing: Casing::Rifle, flash: 1.0 }
    }

    fn none(_: u32) -> Option<(Vec3, Quat)> {
        None
    }

    #[test]
    fn a_shot_flashes_smokes_traces_and_ejects_a_casing_that_lands_and_fades() {
        let w = room();
        let mut fx = Effects::new(sprites());
        let gun = (Vec3::new(0.2, 1.4, 0.3), Quat::IDENTITY);
        let hit = w.raycast(gun.0 + Vec3::new(0.0, 0.03, 0.66), Vec3::Z, 100.0, &[]);
        fx.shot(&rifle(), gun, Vec3::ZERO, Vec3::Z, hit.as_ref());
        assert_eq!(fx.stats, FxStats { flashes: 1, tracers: 1, casings: 1, holes: 1, explosions: 0 });
        let lights = fx.lights();
        assert!(!lights.is_empty() && lights[0].pos.z > 0.9, "a light at the muzzle");
        let sprites = fx.sprites();
        assert!(sprites.iter().any(|s| s.blend == FxBlend::Add && s.pos.distance(gun.0 + Vec3::new(0.0, 0.03, 0.69)) < 0.05), "the flash");
        assert!(sprites.iter().any(|s| s.blend == FxBlend::Alpha && matches!(s.shape, FxShape::Fixed(_)) && (s.pos.z - 10.0).abs() < 0.01), "a hole in the target");
        // The tracer streaks off towards the hit, its head glowing.
        fx.update(1.0 / 90.0, &w, &none);
        let tracer = |fx: &Effects| fx.sprites().into_iter().filter(|s| s.tex == fx.sprites.tracer.tex || s.tex == fx.sprites.glow.tex).map(|s| s.pos.z).fold(0.0f32, f32::max);
        let z1 = tracer(&fx);
        fx.update(1.0 / 90.0, &w, &none);
        let z2 = tracer(&fx);
        assert!(z1 > 1.5 && z2 > z1 + 1.0, "on its way: {z1} then {z2}");
        // The flash is gone in a few frames, the light too; the tracer gets there.
        for _ in 0..4 {
            fx.update(1.0 / 90.0, &w, &none);
        }
        assert!(fx.lights().is_empty());
        assert!(!fx.sprites().iter().any(|s| s.pos.distance(gun.0 + Vec3::new(0.0, 0.03, 0.69)) < 0.05 && s.blend == FxBlend::Add && matches!(s.shape, FxShape::Billboard { .. }) && s.size.x < 0.3 && s.color[3] > 0.5), "the flash went out");
        // The casing flies out to the right and up, then lands on the floor.
        let c0 = fx.casings().next().unwrap().pos;
        assert!(c0.x > 0.22 && c0.y > 1.44, "out of the port to the right and up: {c0:?}");
        for _ in 0..180 {
            fx.update(1.0 / 90.0, &w, &none);
        }
        let c = fx.casings().next().expect("still there");
        assert!(c.pos.y < 0.03 && c.pos.y > -0.001 && c.pos.x > 0.3, "on the floor to the right: {:?}", c.pos);
        for _ in 0..(5.0 * 90.0) as usize {
            fx.update(1.0 / 90.0, &w, &none);
        }
        assert_eq!(fx.casings().count(), 0, "faded away");
        assert_eq!(fx.decal_count(), 1, "the hole stays a while");
    }

    #[test]
    fn revolvers_keep_their_brass() {
        assert!(keeps_casings("wpn_revolver") && keeps_casings("wpn_duplet"));
        assert!(!keeps_casings("wpn_ak") && !keeps_casings("builtin_pistol"));
    }

    #[test]
    fn holes_are_capped_and_the_oldest_go_first() {
        let w = room();
        let mut fx = Effects::new(sprites());
        for k in 0..(MAX_DECALS + 10) {
            let h = w.raycast(Vec3::new(k as f32 * 0.05 - 2.0, 1.0, 0.0), -Vec3::Y, 10.0, &[]).unwrap();
            fx.impact(&h, -Vec3::Y);
        }
        assert_eq!(fx.decal_count(), MAX_DECALS);
        for _ in 0..15 {
            fx.update(0.1, &w, &none);
        }
        assert!(fx.decal_count() < MAX_DECALS, "the oldest faded");
    }

    #[test]
    fn holes_ride_with_the_object_they_are_in() {
        let w = room();
        let mut fx = Effects::new(sprites());
        let hit = Hit { pos: Vec3::new(0.0, 1.0, 2.0), normal: -Vec3::Z, dist: 2.0, what: HitWhat::Object(9, Surface::Metal) };
        fx.impact(&hit, Vec3::Z);
        let at0 = |_: u32| Some((Vec3::new(0.0, 1.0, 2.05), Quat::IDENTITY));
        fx.update(1.0 / 90.0, &w, &at0);
        let moved = |_: u32| Some((Vec3::new(1.0, 1.0, 2.05), Quat::IDENTITY));
        fx.update(1.0 / 90.0, &w, &moved);
        let d = fx.sprites().into_iter().find(|s| matches!(s.shape, FxShape::Fixed(_))).unwrap();
        assert!((d.pos.x - 1.0).abs() < 1e-3, "{:?}", d.pos);
    }

    #[test]
    fn an_explosion_flashes_throws_debris_and_smokes_for_seconds() {
        let w = room();
        let mut fx = Effects::new(sprites());
        fx.explosion(Vec3::new(0.0, 0.05, 6.0), Blast::Frag);
        assert!(fx.lights()[0].radius >= 10.0);
        let n = fx.live();
        assert!(n > 50, "{n}");
        for _ in 0..90 {
            fx.update(1.0 / 90.0, &w, &none);
        }
        assert!(fx.lights().is_empty(), "the flash is over");
        assert!(fx.sprites().iter().any(|s| s.blend == FxBlend::Alpha && s.pos.y > 0.5), "smoke still rolling up");
        assert!(fx.bits().all(|(_, p, _, _)| p.y > -0.01), "debris stays above the floor");
    }
}
