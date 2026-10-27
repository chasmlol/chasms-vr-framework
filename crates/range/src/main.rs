//! `rustvr-range`: the test range. A tiny "flat game" with an empty room, target plates and a table
//! of guns, connected to the runtime through the same adapter SDK a real game adapter uses. It
//! never opens a window: it renders straight into the runtime's shared eye textures.
//!
//! It reports what happened (shots, hits, reloads, crouching, gestures...) in
//! `GameState::stats` (layout: `rustvr_protocol::range_stats`) so headless scenarios can check
//! the whole loop end to end.
//!
//! Physical mode (the default): guns, magazines and props are objects for the runtime's hands
//! (see `physical`); the range draws them where the runtime puts them and fires the shots it
//! counts. Grenades and throwing knives too (`throwables`): the runtime arms and throws them, the
//! range makes their explosions. `--legacy` instead glues the current gun to the firing hand and
//! drives it from intents, the way an adapter for a game without object support works.
//!
//! Either way the range shows what a game would (`effects`): muzzle flashes, smoke, tracers,
//! bullet holes and sparks where the shots hit (`hits`), ejected casings, explosions.
//!
//! Options: `--weapon <name|scoped|first>` picks the starting gun (legacy); `--fps <n>` caps the
//! frame rate when the runtime isn't in lockstep (to test a game slower than the headset);
//! `--table <key>[,<key>...]` lays those guns (asset keys, `wpn_duplet`) nearest on the table at
//! your right hand (physical mode; the scenarios that handle a particular gun use it);
//! `--runtime-draws` has the runtime draw Metro's guns and their magazines (`OBJF_RUNTIME_DRAWS`,
//! from the same models, with the player's hands) instead of the range, as a game adapter does.

#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

mod content;
mod effects;
mod hits;
mod physical;
mod render;
mod throwables;

use anyhow::Result;
use glam::{Mat4, Quat, Vec3};
use rustvr_adapter::actions::{held, pressed};
use rustvr_adapter::math::{projection, symmetric_fov, view_matrix};
use rustvr_adapter::{Adapter, AdapterConfig, Anchor, View};
use rustvr_gpu::{EguiPainter, Gpu};
use rustvr_protocol::range_stats as rs;
use rustvr_protocol::*;
use std::collections::VecDeque;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

use content::{MeshData, Weapon};
use effects::{BitKind, Casing, Effects, GunFx};
use hits::{HitWhat, ObjBox, Surface};
use render::{Draw, Mesh, Renderer};
use std::collections::HashMap;

const STAND_EYE: f32 = 1.7;
const CROUCH_EYE: f32 = 1.1;
const WALK_SPEED: f32 = 3.0;
const SPRINT_SPEED: f32 = 5.0;
const NEAR: f32 = 0.03;
const FAR: f32 = 120.0;

struct GpuWeapon {
    w: Weapon,
    parts: Vec<Mesh>,
    hands: Vec<Mesh>,
}

/// Everything drawable, kept apart from the renderer so both can be borrowed at once.
struct Scene {
    scenery: Vec<Mesh>,
    target_meshes: Vec<Mesh>,
    cube: Mesh,
    hand_box: Mesh,
    weapons: Vec<GpuWeapon>,
    /// Each throwable's parts.
    throwables: Vec<Vec<Mesh>>,
    /// Effect meshes: the casings (`Casing` order: rifle, pistol, shell).
    fx: Vec<Mesh>,
    /// The valves' wheels (`content::valves` order).
    valves: Vec<Mesh>,
}

struct Game {
    gpu: Gpu,
    adapter: Adapter,
    renderer: Renderer,
    scene: Scene,
    targets: Vec<content::Target>,
    weapon: usize,
    hud: (egui::Context, EguiPainter),
    /// `--metro-hud`: the HUD stands in for Metro 2033 Redux's UI layer (its pieces where Redux
    /// draws them, additive over black) and is described with Redux's layout, as Metro's adapter
    /// does, so the runtime's HUD in pieces can be tested without the game. The layout's been sent.
    metro_hud: Option<bool>,

    time: f64,
    feet: Vec3,
    vel_y: f32,
    crouched: bool,
    eye_h: f32,
    yaw: f32,
    paused: bool,
    ammo: u32,
    reload_until: Option<f64>,
    next_shot: f64,
    fire_was_held: bool,
    jump_was_held: bool,
    cursor_was_down: bool,
    target_flash: Vec<f64>,
    impacts: VecDeque<(Vec3, f64)>,
    stats: [u32; 24],
    stats_f: [f32; 12],
    connected_once: bool,
    gone_since: Option<std::time::Instant>,
    /// Physical mode's objects (None: legacy mode).
    armory: Option<physical::Armory>,
    connections: u32,
    /// Physical mode: the gun in a hand, if any (index, rounds in its magazine, rounds in its chambers).
    held: Option<(usize, u32, u32)>,
    /// Physical mode's throwables, and what's happened to them.
    throwables: Vec<throwables::Throwable>,
    watch: throwables::Watch,
    /// The guns' shots as a game shows them, and what the bullets hit.
    effects: Effects,
    gun_fx: Vec<GunFx>,
    world: hits::World,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("RUSTVR_LOG").unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let mut start_weapon = String::from("first");
    let mut fps_cap = 0.0f64;
    let mut legacy = false;
    let mut front: Vec<String> = Vec::new();
    let mut runtime_draws = false;
    let mut metro_hud = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--weapon" => start_weapon = args.next().unwrap_or_default(),
            "--fps" => fps_cap = args.next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
            "--legacy" => legacy = true,
            "--runtime-draws" => runtime_draws = true,
            "--metro-hud" => metro_hud = true,
            "--table" => front = args.next().unwrap_or_default().split(',').filter(|k| !k.is_empty()).map(String::from).collect(),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let gpu = Gpu::new()?;
    let mut game = Game::new(gpu, &start_weapon, legacy, &front, runtime_draws)?;
    game.metro_hud = metro_hud.then_some(false);
    game.run(fps_cap)
}

impl Game {
    fn new(gpu: Gpu, start_weapon: &str, legacy: bool, front: &[String], runtime_draws: bool) -> Result<Game> {
        let assets = rustvr_asset::default_root();
        let mut renderer = Renderer::new(&gpu, assets.clone())?;
        let (grid, plain) = content::room();
        let scenery = vec![renderer.upload(&gpu, &grid)?, renderer.upload(&gpu, &plain)?];
        let targets = content::targets();
        let target_meshes = targets.iter().map(|t| renderer.upload(&gpu, &content::target_mesh(t))).collect::<Result<Vec<_>>>()?;
        let mut cube_data = MeshData::default();
        cube_data.add_box(Vec3::splat(-0.5), Vec3::splat(0.5), [255, 255, 255, 255]);
        let cube = renderer.upload(&gpu, &cube_data)?;
        let mut hand_data = MeshData::default();
        hand_data.add_box(Vec3::new(-0.035, -0.05, -0.06), Vec3::new(0.035, 0.03, 0.05), [205, 160, 130, 255]);
        let hand_box = renderer.upload(&gpu, &hand_data)?;

        // Guns: Metro 2033's own when they've been extracted, plus the built-in ones.
        let mut list = content::metro_weapons(&assets);
        let from_game = list.len();
        list.push(content::builtin_rifle("Range rifle", false));
        list.push(content::builtin_rifle("Range rifle (scoped)", true));
        list.push(content::builtin_pistol());
        // The built-in guns are held with the Kalash's own hands when Metro's are there.
        content::share_game_hands(&mut list);
        tracing::info!("range: {} guns ({} from Metro 2033)", list.len(), from_game);
        let mut weapons = Vec::new();
        for w in list {
            let parts = w.parts.iter().map(|p| renderer.upload(&gpu, p)).collect::<Result<Vec<_>>>()?;
            let hands = w.hands.iter().map(|p| renderer.upload(&gpu, p)).collect::<Result<Vec<_>>>()?;
            weapons.push(GpuWeapon { w, parts, hands });
        }
        let weapon = match start_weapon {
            "scoped" => weapons.iter().position(|g| g.w.ads_zoom > 1.4).unwrap_or(0),
            "first" | "" => 0,
            name => weapons.iter().position(|g| g.w.name.to_lowercase().contains(&name.to_lowercase())).unwrap_or(0),
        };

        let mut armory = (!legacy).then(|| physical::Armory::with_front(&weapons.iter().map(|g| &g.w).collect::<Vec<_>>(), front));
        if let (Some(a), true) = (&mut armory, runtime_draws) {
            a.let_runtime_draw(&weapons.iter().map(|g| &g.w).collect::<Vec<_>>());
        }
        // Grenades and knives (physical mode): on their own table at your left hand, and in the
        // pouches.
        let throw_list = if legacy { Vec::new() } else { throwables::all(&assets) };
        let mut scenery = scenery;
        if let Some(a) = &mut armory {
            let placed = throwables::layout(&throw_list);
            a.add_objects(&placed.iter().map(|(i, d)| (*d, physical::Kind::Throwable(*i))).collect::<Vec<_>>());
            a.add_sound_sets(&placed.iter().map(|(i, d)| (d.id, throwables::sound_set(&throw_list[*i]))).collect::<Vec<_>>());
            let table = throwables::furniture();
            a.add_collision(&table.iter().map(|(lo, hi, _)| (*lo, *hi)).collect::<Vec<_>>());
            let mut m = MeshData::default();
            for (lo, hi, c) in table {
                m.add_box(lo, hi, c);
            }
            scenery.push(renderer.upload(&gpu, &m)?);
            let names: Vec<&str> = throw_list.iter().map(|t| t.name.as_str()).collect();
            tracing::info!("range: {} throwables ({}), {} placed", throw_list.len(), names.join(", "), placed.len());
        }
        let throw_meshes = throw_list.iter().map(|t| t.parts.iter().map(|p| renderer.upload(&gpu, p)).collect::<Result<Vec<_>>>()).collect::<Result<Vec<_>>>()?;
        let fx_meshes = [Casing::Rifle, Casing::Pistol, Casing::Shell].iter().map(|c| renderer.upload(&gpu, &effects::casing_mesh(*c))).collect::<Result<Vec<_>>>()?;
        let valve_meshes = content::valves().iter().map(|v| renderer.upload(&gpu, &content::valve_mesh(v))).collect::<Result<Vec<_>>>()?;
        let sprites = effects::Sprites::load(|key, fallback| renderer.fx_texture(&gpu, key, fallback));
        tracing::info!("range: effect sprites {}", if sprites.flash.from_game { "from Metro 2033" } else { "built in" });
        let gun_fx = weapons.iter().map(|g| effects::gun_fx(&g.w, &assets)).collect();
        // What bullets hit: the room (stone), the furniture (wood, the steel rails metal), the plates.
        let mut boxes: Vec<(Vec3, Vec3, Surface)> = content::shell().into_iter().map(|(lo, hi)| (lo, hi, Surface::Stone)).collect();
        for (lo, hi, c) in content::furniture().into_iter().chain(if legacy { Vec::new() } else { throwables::furniture() }) {
            boxes.push((lo, hi, if c[2] > c[0] { Surface::Metal } else { Surface::Wood }));
        }
        let world = hits::World::new(boxes, targets.iter().map(|t| (t.center, t.radius)).collect());
        let mut cfg = AdapterConfig::new("range");
        cfg.request = TargetRequest {
            color_format: DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32,
            flags: TGT_HUD | TGT_AUX | TGT_DEPTH,
            eye_w: 0,
            eye_h: 0,
            hud_w: 1280,
            hud_h: 720,
            aux_size: 768,
            _pad: 0,
        };
        let adapter = Adapter::new(gpu.clone(), cfg);
        let painter = EguiPainter::new(&gpu)?;
        let ammo = weapons[weapon].w.mag_size;
        Ok(Game {
            gpu,
            adapter,
            renderer,
            scene: Scene { scenery, target_meshes, cube, hand_box, weapons, throwables: throw_meshes, fx: fx_meshes, valves: valve_meshes },
            target_flash: vec![-10.0; targets.len()],
            targets,
            weapon,
            hud: (egui::Context::default(), painter),
            metro_hud: None,
            time: 0.0,
            feet: Vec3::ZERO,
            vel_y: 0.0,
            crouched: false,
            eye_h: STAND_EYE,
            yaw: 0.0,
            paused: false,
            ammo,
            reload_until: None,
            next_shot: 0.0,
            fire_was_held: false,
            jump_was_held: false,
            cursor_was_down: false,
            impacts: VecDeque::new(),
            stats: [0; 24],
            stats_f: [0.0; 12],
            connected_once: false,
            gone_since: None,
            armory,
            connections: 0,
            held: None,
            throwables: throw_list,
            watch: throwables::Watch::default(),
            effects: Effects::new(sprites),
            gun_fx,
            world,
        })
    }

    fn run(&mut self, fps_cap: f64) -> Result<()> {
        let mut last = std::time::Instant::now();
        let mut next_frame = std::time::Instant::now();
        loop {
            let Some(frame) = self.adapter.wait_frame(100) else {
                // Quit once the runtime that started us has gone for good: its heartbeat stopped,
                // not just its frames (it sends none while the headset is off or the dashboard
                // has focus).
                if self.adapter.connected() {
                    self.gone_since = None;
                } else if self.connected_once {
                    let since = *self.gone_since.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() > std::time::Duration::from_secs(5) {
                        tracing::info!("range: the runtime has gone, exiting");
                        return Ok(());
                    }
                }
                continue;
            };
            self.connected_once = true;
            self.gone_since = None;
            let wall = last.elapsed().as_secs_f32();
            last = std::time::Instant::now();
            let dt = if frame.flags & RT_LOCKSTEP != 0 { frame.dt } else { wall.min(0.1) };
            let t0 = std::time::Instant::now();
            self.frame(&frame, dt)?;
            self.stats_f[rs::F_FRAME_MS] = t0.elapsed().as_secs_f32() * 1000.0;
            if fps_cap > 0.0 && frame.flags & RT_LOCKSTEP == 0 {
                next_frame += std::time::Duration::from_secs_f64(1.0 / fps_cap);
                let now = std::time::Instant::now();
                match next_frame.checked_duration_since(now) {
                    Some(wait) => std::thread::sleep(wait),
                    None => next_frame = now,
                }
            }
        }
    }

    fn frame(&mut self, f: &RtFrame, dt: f32) -> Result<()> {
        self.time += dt as f64;
        let now = self.time;
        let it = f.intents;
        let presses = self.adapter.presses.update(&it);
        if let Some(a) = &mut self.armory {
            if self.adapter.connections() != self.connections {
                self.connections = self.adapter.connections();
                a.reset();
                self.watch.reset();
            }
            a.register(&self.adapter);
            a.update(&self.adapter);
        }

        // ---- game logic
        if pressed(&presses, Action::Menu) {
            self.paused = !self.paused;
            self.stats[rs::MENU_TOGGLES] += 1;
        }
        let anchor = Anchor {
            eye: self.feet + Vec3::new(0.0, self.eye_h, 0.0),
            base_eye: self.feet + Vec3::new(0.0, self.eye_h, 0.0),
            look_yaw: self.yaw,
            scripted: false,
        };
        let view = self.adapter.tracking.update(f, &anchor, now, dt);
        if self.paused {
            let down = it.flags & INT_CURSOR != 0 && it.flags & INT_CURSOR_DOWN != 0;
            if down && !self.cursor_was_down {
                self.stats[rs::MENU_CLICKS] += 1;
            }
            self.cursor_was_down = down;
        } else if let Some(v) = &view {
            self.gameplay(f, v, &presses, dt);
            self.throwables(v, now);
            let objects = self.object_poses(v);
            self.effects.update(dt, &self.world, &|id| objects.get(&id).copied());
        }
        self.stats_f[rs::F_FX_LIVE] = self.effects.live() as f32;
        if let Some(until) = self.reload_until
            && now >= until {
                self.ammo = self.scene.weapons[self.weapon].w.mag_size;
                self.reload_until = None;
                self.stats[rs::RELOADS] += 1;
            }
        self.impacts.retain(|(_, t)| now - t < 3.0);

        // ---- report
        let held = self.armory.as_ref().and_then(|a| a.held_gun(1));
        self.held = held;
        if let Some((i, rounds, chambered)) = held {
            self.weapon = i;
            self.ammo = rounds + chambered;
        }
        let w = &self.scene.weapons[self.weapon].w;
        let ads = it.flags & INT_ADS != 0;
        let mut gs: GameState = bytemuck::Zeroable::zeroed();
        let has_weapon = self.armory.is_none() || held.is_some();
        gs.flags = GS_IN_GAME
            | if has_weapon { GS_HAS_WEAPON } else { 0 }
            | if self.crouched { GS_CROUCHED } else { 0 }
            | if self.paused { GS_MENU | GS_PAUSED } else { 0 };
        gs.eye_above_feet = self.eye_h;
        gs.game_zoom = if ads && !self.paused && has_weapon { w.ads_zoom } else { 1.0 };
        if self.armory.is_none() {
            gs.rig = w.rig;
        }
        self.stats[rs::WEAPON] = self.weapon as u32;
        self.stats[rs::AMMO] = self.ammo;
        self.stats[rs::HELD] = held.map(|(i, _, _)| i as u32 + 1).unwrap_or(0);
        self.stats[rs::OBJECTS] = self.armory.as_ref().map(|a| a.states.count).unwrap_or(0);
        self.stats[rs::CROUCHED] = self.crouched as u32;
        self.stats_f[rs::F_FEET_X] = self.feet.x;
        self.stats_f[rs::F_FEET_Z] = self.feet.z;
        self.stats_f[rs::F_MAG_SIZE] = w.mag_size as f32;
        if let Some(v) = &view {
            self.stats_f[rs::F_LOOK_YAW] = v.look_yaw;
        }
        gs.stats = self.stats;
        gs.stats_f = self.stats_f;
        self.adapter.write_state(&mut gs);

        // ---- render
        if let Some(v) = view {
            self.render(f, &v)?;
        }
        Ok(())
    }

    fn gameplay(&mut self, f: &RtFrame, v: &View, presses: &[u16; ACTION_COUNT], dt: f32) {
        let it = &f.intents;
        self.yaw = v.look_yaw;
        // Walk relative to where the player looks.
        let (s, c) = v.look_yaw.sin_cos();
        let fwd = Vec3::new(s, 0.0, c);
        let right = Vec3::new(c, 0.0, -s);
        let speed = if held(it, Action::Sprint) { SPRINT_SPEED } else { WALK_SPEED } * if self.crouched { 0.5 } else { 1.0 };
        let mv = (right * it.move_xy[0] + fwd * it.move_xy[1]) * speed * dt;
        self.feet += mv;
        self.feet.x = self.feet.x.clamp(content::ROOM_MIN.x, content::ROOM_MAX.x);
        self.feet.z = self.feet.z.clamp(content::ROOM_MIN.z, content::ROOM_MAX.z);
        // Jump and gravity.
        let jump = held(it, Action::Jump);
        if jump && !self.jump_was_held && self.feet.y <= 0.0 {
            self.vel_y = 4.0;
            self.stats[rs::JUMPS] += 1;
        }
        self.jump_was_held = jump;
        self.vel_y -= 9.81 * dt;
        self.feet.y = (self.feet.y + self.vel_y * dt).max(0.0);
        if self.feet.y == 0.0 {
            self.vel_y = 0.0;
        }
        if pressed(presses, Action::Crouch) {
            self.crouched = !self.crouched;
        }
        let target_eye = if self.crouched { CROUCH_EYE } else { STAND_EYE };
        self.eye_h += (target_eye - self.eye_h) * (dt * 12.0).min(1.0);

        // Physical mode: the runtime fires the guns; the range only flies the bullets.
        if let Some(a) = &mut self.armory {
            for shot in a.shots(v) {
                self.stats[rs::SHOTS] += 1;
                self.stats_f[rs::F_SHOT_AIM_ERROR] = 0.0;
                self.shoot(shot.origin, shot.dir.normalize());
                self.shot_fx(shot.weapon, shot.gun, shot.vel, shot.origin, shot.dir.normalize(), v);
            }
        }
        // The cases a break action throws out as it opens, and the ones a bolt or a pump does.
        if let Some(a) = &mut self.armory {
            let guns: Vec<&Weapon> = self.scene.weapons.iter().map(|g| &g.w).collect();
            for e in a.ejections(v, &guns) {
                let Some(fx) = self.gun_fx.get(e.weapon).copied() else { continue };
                match e.chamber {
                    Some((at, along)) => self.effects.eject_from_chamber(fx.casing, at, along, e.vel),
                    None => self.effects.eject_from_port(&fx, e.gun, e.vel),
                }
            }
        }
        // Weapons.
        let n = self.scene.weapons.len();
        let switch = |game: &mut Game, to: usize| {
            game.weapon = to % n;
            game.ammo = game.scene.weapons[game.weapon].w.mag_size;
            game.reload_until = None;
            game.stats[rs::SWITCHES] += 1;
        };
        let legacy = self.armory.is_none();
        if pressed(presses, Action::NextWeapon) && legacy {
            switch(self, self.weapon + 1);
        }
        if pressed(presses, Action::PrevWeapon) && legacy {
            switch(self, self.weapon + n - 1);
        }
        for (a, i) in [
            (Action::GestureFace, rs::GESTURE_FACE),
            (Action::GestureHeadTop, rs::GESTURE_HEAD_TOP),
            (Action::GestureChest, rs::GESTURE_CHEST),
            (Action::GestureShoulder, rs::GESTURE_SHOULDER),
            (Action::GestureHip, rs::GESTURE_HIP),
        ] {
            self.stats[i] += presses[a as usize] as u32;
        }
        self.stats[rs::THROWS] += presses[Action::Throw as usize] as u32;
        self.stats[rs::MELEES] += presses[Action::Melee as usize] as u32;
        for (a, i) in [(Action::AbilityUp, rs::ABILITY_UP), (Action::AbilityDown, rs::ABILITY_DOWN), (Action::AbilityLeft, rs::ABILITY_LEFT), (Action::AbilityRight, rs::ABILITY_RIGHT)] {
            self.stats[i] += presses[a as usize] as u32;
        }
        self.stats[rs::USES] += presses[Action::Use as usize] as u32;
        if !legacy {
            return;
        }
        if pressed(presses, Action::Reload) && self.reload_until.is_none() && self.ammo < self.scene.weapons[self.weapon].w.mag_size {
            self.reload_until = Some(self.time + 1.0);
        }

        // Fire along the gun's line of sight.
        let fire = it.fire > 0.5 && it.flags & INT_WEAPON != 0;
        let w = &self.scene.weapons[self.weapon].w;
        let trigger_pull = fire && !self.fire_was_held;
        self.fire_was_held = fire;
        if fire && (w.automatic || trigger_pull) && self.time >= self.next_shot && self.ammo > 0 && self.reload_until.is_none() {
            self.next_shot = self.time + 1.0 / w.rounds_per_second as f64;
            self.ammo -= 1;
            self.stats[rs::SHOTS] += 1;
            let origin = v.point(Vec3::from_array(it.sight_origin));
            let dir = v.dir(Vec3::from_array(it.aim_dir)).normalize();
            let barrel = v.weapon_to_world(it).z_axis.truncate().normalize();
            self.stats_f[rs::F_SHOT_AIM_ERROR] = dir.angle_between(barrel).to_degrees();
            self.shoot(origin, dir);
            let (_, rot, pos) = v.weapon_to_world(it).to_scale_rotation_translation();
            self.shot_fx(self.weapon, (pos, rot), Vec3::ZERO, origin, dir, v);
            self.adapter.rumble(0.35);
        }
    }

    /// The loose objects a bullet can hit, as boxes (not what the hands hold or wear).
    fn object_boxes(&self, v: &View) -> Vec<ObjBox> {
        let Some(a) = &self.armory else { return Vec::new() };
        a.states()
            .iter()
            .filter(|s| s.flags & (OS_TRACKING | OS_IN_WEAPON | OS_SPENT) == 0)
            .filter_map(|s| {
                let d = a.desc(s.desc)?;
                let (pos, rot) = physical::Armory::world_pose(s, v);
                let name = name_str(&d.name);
                let surface = match d.kind {
                    OBJ_PROP if name.contains("box") || name.contains("crate") => Surface::Wood,
                    OBJ_PROP if name.contains("bottle") => Surface::Stone,
                    _ => Surface::Metal,
                };
                Some(ObjBox { id: s.id, pos, rot, center: Vec3::from_array(d.center), half: Vec3::from_array(d.half_extents), surface })
            })
            .collect()
    }

    /// Where each object is (world), for what rides on them.
    fn object_poses(&self, v: &View) -> HashMap<u32, (Vec3, Quat)> {
        let Some(a) = &self.armory else { return HashMap::new() };
        a.states().iter().filter(|s| s.flags & OS_SPENT == 0).map(|s| (s.id, physical::Armory::world_pose(s, v))).collect()
    }

    /// A shot as the game shows it: the gun's flash, smoke, tracer and casing, and what it hit
    /// (a hole, sparks or dust, and a push on a loose object).
    fn shot_fx(&mut self, weapon: usize, gun: (Vec3, Quat), vel: Vec3, origin: Vec3, dir: Vec3, v: &View) {
        let boxes = self.object_boxes(v);
        let hit = self.world.raycast(origin, dir, 150.0, &boxes);
        let Some(fx) = self.gun_fx.get(weapon).copied() else { return };
        self.effects.shot(&fx, gun, vel, dir, hit.as_ref());
        if let Some(h) = hit
            && let HitWhat::Object(id, _) = h.what
        {
            // A rifle round knocks a can over; real momentum would throw it across the room.
            self.adapter.impulse(id, (dir * 1.5).to_array(), h.pos.to_array());
        }
    }

    /// The throwables: explosions (effects, the blast's push, damage to the plates), knives in the
    /// plates, lit fuses, and spent grenades put back.
    fn throwables(&mut self, v: &View, now: f64) {
        let Some(a) = &mut self.armory else { return };
        let states: Vec<(ObjectState, Vec3, Quat)> = a
            .states()
            .iter()
            .filter(|s| a.throwable(s.desc).is_some())
            .map(|s| {
                let (p, q) = physical::Armory::world_pose(s, v);
                (*s, p, q)
            })
            .collect();
        let kinds: HashMap<u32, usize> = states.iter().filter_map(|(s, _, _)| Some((s.desc, a.throwable(s.desc)?))).collect();
        let events = self.watch.update(&states, &|d| kinds.get(&d).copied(), &self.throwables, now);
        for id in self.watch.due(now) {
            a.respawn(id);
        }
        for e in events {
            match e {
                throwables::Event::Exploded { at, blast, .. } => {
                    self.effects.explosion(at, blast);
                    let (damage, (radius, impulse)) = match blast {
                        effects::Blast::Frag => (throwables::FRAG_DAMAGE, throwables::FRAG_PUSH),
                        effects::Blast::Fire => (throwables::FIRE_DAMAGE, throwables::FIRE_PUSH),
                    };
                    self.adapter.blast(at.to_array(), radius, impulse);
                    self.stats_f[rs::F_EXPLOSIONS] += 1.0;
                    for (i, t) in self.targets.iter().enumerate() {
                        if t.center.distance(at) <= damage + t.radius {
                            self.target_flash[i] = self.time;
                            self.stats_f[rs::F_BLAST_HITS] += 1.0;
                        }
                    }
                    tracing::info!("range: an explosion at {at:.2} ({blast:?})");
                }
                throwables::Event::Stuck { tip, .. } => {
                    let plate = self.targets.iter().position(|t| throwables::in_plate(tip, (t.center, t.radius)));
                    if let Some(i) = plate {
                        self.stats_f[rs::F_KNIFE_HITS] += 1.0;
                        self.target_flash[i] = self.time;
                    }
                    self.effects.knife_hit(tip, plate.is_some());
                    tracing::info!("range: a knife stuck at {tip:.2}{}", if plate.is_some() { " in a target" } else { "" });
                }
            }
        }
        // Lit fuses sputter; the frag's countdown light blinks faster at the end.
        for (s, p, q) in &states {
            if s.flags & OS_ARMED == 0 || s.flags & OS_SPENT != 0 {
                continue;
            }
            let t = &self.throwables[kinds[&s.desc]];
            if let Some(f) = t.fuse_at {
                self.effects.fuse(*p + *q * f);
            }
            if let Some(led) = t.led {
                let period = if s.fuse <= 1.5 { 0.15 } else { 0.5 };
                if (s.fuse / period).fract() < 0.5 {
                    self.effects.blink(*p + *q * led, [3.0, 0.2, 0.1]);
                }
            }
        }
    }

    fn shoot(&mut self, origin: Vec3, dir: Vec3) {
        // Nearest target plate the ray crosses, and how far from that plate's centre.
        let mut best: Option<(f32, usize, f32, Vec3)> = None;
        for (i, t) in self.targets.iter().enumerate() {
            if dir.z.abs() < 1e-5 {
                continue;
            }
            let k = (t.center.z - origin.z) / dir.z;
            if k <= 0.0 {
                continue;
            }
            let p = origin + dir * k;
            let err = Vec3::new(p.x - t.center.x, p.y - t.center.y, 0.0).length();
            if err < t.radius * 3.0 && best.is_none_or(|b| k < b.0) {
                best = Some((k, i, err, p));
            }
        }
        if let Some((_, i, err, p)) = best {
            self.stats_f[rs::F_LAST_HIT_ERROR] = err;
            if err <= self.targets[i].radius {
                self.stats[rs::HITS] += 1;
                self.target_flash[i] = self.time;
                self.impacts.push_back((p - dir * 0.01, self.time));
            }
        }
    }

    fn render(&mut self, f: &RtFrame, v: &View) -> Result<()> {
        let it = f.intents;
        let paused = self.paused;
        let scope = it.flags & INT_SCOPE != 0 && !paused;
        // Build the draw list once.
        let mut draws: Vec<(usize, Mat4, [f32; 4], f32, bool)> = Vec::new(); // (mesh id, model, tint, emissive, two-sided)
        const SCENERY: usize = 0;
        for i in 0..self.scene.scenery.len() {
            draws.push((SCENERY + i, Mat4::IDENTITY, [1.0; 4], 0.0, false));
        }
        let targets_at = 100;
        for i in 0..self.scene.target_meshes.len() {
            let flash = (self.time - self.target_flash[i]) < 0.15;
            draws.push((targets_at + i, Mat4::IDENTITY, [1.0; 4], if flash { 0.8 } else { 0.0 }, false));
        }
        let impacts_at = 200;
        for (p, _) in &self.impacts {
            draws.push((impacts_at, Mat4::from_scale_rotation_translation(Vec3::splat(0.04), Quat::IDENTITY, *p), [1.0, 0.85, 0.2, 1.0], 2.0, false));
        }
        let gun_draws_start = draws.len();
        let g = &self.scene.weapons[self.weapon];
        let gun = v.weapon_to_world(&it) * g.w.fix;
        // Casings and debris.
        for (kind, p, q, k) in self.effects.bits() {
            match kind {
                BitKind::Casing(c) => draws.push((30_000 + c as usize, Mat4::from_scale_rotation_translation(Vec3::splat(k), q, p), [1.0; 4], 0.0, false)),
                BitKind::Debris => draws.push((200, Mat4::from_scale_rotation_translation(Vec3::splat(0.022 * k), q, p), [0.2, 0.18, 0.16, 1.0], 0.0, false)),
            }
        }
        if let Some(a) = &self.armory {
            let ws: Vec<&Weapon> = self.scene.weapons.iter().map(|g| &g.w).collect();
            draws.extend(a.draws(v, &ws, f.hands.map(|h| h.trigger)));
            // The throwables: without the pin once it's out, the lever once it's flown.
            for s in a.states() {
                let Some(i) = a.throwable(s.desc).filter(|_| s.flags & OS_SPENT == 0) else { continue };
                let t = &self.throwables[i];
                let (p, q) = physical::Armory::world_pose(s, v);
                let model = Mat4::from_rotation_translation(q, p) * t.fix;
                let glow = if s.flags & OS_HOVERED != 0 { 0.25 } else { 0.0 };
                for part in 0..t.parts.len() {
                    let gone = (s.flags & OS_PIN_PULLED != 0 && t.pin_parts.contains(&part)) || (s.flags & OS_ARMED != 0 && t.lever_parts.contains(&part));
                    if !gone {
                        draws.push((throwables::part_id(i, part), model, [1.0; 4], glow, t.from_game));
                    }
                }
            }
            // The runtime's hands (where its physics put them), and a ghost where a wall holds one back,
            // unless the runtime draws the player's body itself.
            if let Some(body) = self.adapter.read_body().filter(|_| f.flags & RT_DRAWS_BODY == 0) {
                for (i, h) in body.hands.iter().enumerate() {
                    if h.flags & HAND_TRACKED == 0 {
                        continue;
                    }
                    let (p, r) = v.pose(&h.pose);
                    draws.push((500, Mat4::from_rotation_translation(r, p), [1.0; 4], 0.0, false));
                    if h.flags & HAND_BLOCKED != 0 {
                        let (gp, gr) = v.pose(&f.hands[i].grip);
                        draws.push((500, Mat4::from_rotation_translation(gr, gp), [0.5, 0.7, 1.0, 1.0], 0.6, false));
                    }
                }
            }
        } else if it.flags & INT_WEAPON != 0 {
            let mag_out = it.flags & INT_MAG_OUT != 0;
            for i in 0..g.parts.len() {
                if mag_out && g.w.mag_parts.contains(&i) {
                    continue;
                }
                draws.push((300 + i, gun, [1.0; 4], 0.0, g.w.from_game));
            }
            // A fresh magazine in the off hand (the hand further from the gun's grip).
            if it.flags & INT_HOLDING_MAG != 0 {
                let grip = gun.w_axis.truncate();
                let hands: Vec<_> = (0..2).map(|h| v.pose(&f.hands[h].grip)).collect();
                let (p, r) = if hands[0].0.distance(grip) > hands[1].0.distance(grip) { hands[0] } else { hands[1] };
                let hand = Mat4::from_rotation_translation(r, p);
                if g.w.mag_parts.is_empty() {
                    draws.push((200, hand * Mat4::from_scale(Vec3::new(0.035, 0.17, 0.07)), [0.35, 0.3, 0.22, 1.0], 0.0, false));
                }
                for &i in &g.w.mag_parts {
                    draws.push((300 + i, hand * Mat4::from_translation(-g.w.mag_center) * g.w.fix, [1.0; 4], 0.0, g.w.from_game));
                }
            }
            for i in 0..g.hands.len() {
                draws.push((400 + i, gun, [1.0; 4], 0.0, true));
            }
        }
        if g.hands.is_empty() && self.armory.is_none() {
            for h in 0..2 {
                if f.hands[h].connected != 0 {
                    let (p, r) = v.pose(&f.hands[h].grip);
                    draws.push((500, Mat4::from_rotation_translation(r, p), [1.0; 4], 0.0, false));
                }
            }
        }

        let Some(slot) = self.adapter.begin_slot() else { return Ok(()) };
        let slot_hud = slot.hud.as_ref().map(|h| (h.rtv.clone().unwrap(), h.width, h.height, h.tex.clone()));
        let slot_aux = slot.aux.as_ref().map(|a| (a.rtv.clone().unwrap(), a.width, a.height));
        let eyes: Vec<_> = slot.eyes.iter().map(|e| (e.rtv.clone().unwrap(), e.width, e.height)).collect();
        let depths: Option<Vec<_>> = slot.depth.as_ref().map(|d| d.iter().map(|t| t.rtv.clone().unwrap()).collect());
        let weapon = self.weapon;
        let fx = self.effects.sprites();
        let lights = self.effects.lights();

        let mut flags = SLOT_HUD;
        if !paused {
            flags |= SLOT_STEREO;
            for e in 0..2 {
                let (rtv, w, h) = &eyes[e];
                let vp = projection(&f.fov[e], NEAR, FAR) * v.eye_view(&f.eyes[e]);
                let (eye_world, eye_rot) = v.pose(&f.eyes[e]);
                let list = to_draws(&self.scene, weapon, &draws);
                let depth = depths.as_ref().map(|d| &d[e]);
                self.renderer.render_scene(&self.gpu, rtv, depth, *w, *h, vp, eye_world, eye_rot * Vec3::Z, &list, &fx, &lights)?;
            }
            if depths.is_some() {
                flags |= SLOT_DEPTH;
            }
            if let (true, Some((rtv, w, h))) = (scope, &slot_aux) {
                let (p, r) = v.pose(&it.scope_cam);
                let aspect = *w as f32 / *h as f32;
                let vp = projection(&symmetric_fov(it.scope_vfov_deg, aspect), 0.05, FAR) * view_matrix(p, r);
                let list = to_draws(&self.scene, weapon, &draws[..gun_draws_start]);
                self.renderer.render_scene(&self.gpu, rtv, None, *w, *h, vp, p, r * Vec3::Z, &list, &fx, &lights)?;
                flags |= SLOT_AUX;
                self.stats_f[rs::F_SCOPE_FRAMES] += 1.0;
            }
        } else {
            flags |= SLOT_MENU;
        }
        if let Some((rtv, w, h, _)) = &slot_hud {
            self.paint_hud_into(rtv, *w, *h)?;
        }
        let info = SlotInfo {
            frame_index: f.frame_index,
            generation: 0,
            flags,
            eye_pose: f.eyes,
            fov: f.fov,
            depth_near: NEAR,
            depth_far: FAR,
            depth_flags: DEPTH_LINEAR,
            aux_frame: f.frame_index as u32,
            aux_pose: it.scope_cam,
            hud_aspect: slot_hud.as_ref().map(|(_, w, h, _)| *w as f32 / *h as f32).unwrap_or(16.0 / 9.0),
            _pad: [0; 3],
            // (The range renders every headset frame: nothing to move.)
            eye_map: [SlotMap::default(); 2],
        };
        if self.metro_hud == Some(false) && slot_hud.is_some() {
            self.adapter.write_hud_layout(&metro_2033_redux_layout(info.hud_aspect));
            self.metro_hud = Some(true);
        }
        self.adapter.submit(info);
        Ok(())
    }

    /// The HUD (ammo, gun, hits) or, while paused, the pause menu, as a premultiplied overlay.
    fn paint_hud_into(&mut self, rtv: &windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView, w: u32, h: u32) -> Result<()> {
        let (ctx, painter) = &mut self.hud;
        let ppp = 2.0;
        ctx.set_pixels_per_point(ppp);
        let size = egui::vec2(w as f32 / ppp, h as f32 / ppp);
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
        let weapon = &self.scene.weapons[self.weapon].w;
        let (ammo, mag, reloading, hits, shots, paused) =
            (self.ammo, weapon.mag_size, self.reload_until.is_some(), self.stats[rs::HITS], self.stats[rs::SHOTS], self.paused);
        // Physical mode: no gun in hand, no ammo counter.
        let show_gun = self.armory.is_none() || self.held.is_some();
        let name = weapon.name.clone();
        let metro = self.metro_hud.is_some();
        let mut out = ctx.run_ui(input, |ui| {
            let painter = ui.painter();
            if paused {
                painter.rect_filled(ui.max_rect(), 0.0, egui::Color32::from_rgb(20, 24, 30));
                painter.text(egui::pos2(size.x / 2.0, size.y * 0.3), egui::Align2::CENTER_CENTER, "PAUSED", egui::FontId::proportional(48.0), egui::Color32::WHITE);
                let button = egui::Rect::from_center_size(egui::pos2(size.x / 2.0, size.y / 2.0), egui::vec2(220.0, 60.0));
                painter.rect_filled(button, 8.0, egui::Color32::from_rgb(160, 60, 30));
                painter.text(button.center(), egui::Align2::CENTER_CENTER, "Resume (menu button)", egui::FontId::proportional(18.0), egui::Color32::WHITE);
                painter.text(egui::pos2(size.x / 2.0, size.y * 0.7), egui::Align2::CENTER_CENTER, "RustVR test range", egui::FontId::proportional(16.0), egui::Color32::GRAY);
            } else if metro {
                // Metro's UI, where Redux draws it (fractions of the layer: see the layout).
                let at = |u: f32, v: f32| egui::pos2(size.x * u, size.y * v);
                let orange = egui::Color32::from_rgb(230, 120, 40);
                painter.text(at(0.43, 0.84), egui::Align2::CENTER_TOP, "Shhh... Something moving in the next room.", egui::FontId::proportional(18.0), orange);
                painter.text(at(0.43, 0.88), egui::Align2::CENTER_TOP, "Open the gate, cover.", egui::FontId::proportional(18.0), orange);
                painter.text(at(0.5, 0.72), egui::Align2::CENTER_CENTER, "Use L to move.", egui::FontId::proportional(16.0), egui::Color32::WHITE);
                if show_gun {
                    painter.text(at(0.9, 0.9), egui::Align2::CENTER_CENTER, format!("{ammo} | {mag}"), egui::FontId::proportional(24.0), egui::Color32::LIGHT_GRAY);
                }
            } else {
                if show_gun {
                    let panel = egui::Rect::from_min_size(egui::pos2(20.0, size.y - 90.0), egui::vec2(240.0, 70.0));
                    painter.rect_filled(panel, 6.0, egui::Color32::from_black_alpha(150));
                    painter.text(panel.min + egui::vec2(12.0, 10.0), egui::Align2::LEFT_TOP, &name, egui::FontId::proportional(14.0), egui::Color32::LIGHT_GRAY);
                    let ammo_text = if reloading { "RELOADING".to_owned() } else { format!("{ammo} / {mag}") };
                    painter.text(panel.min + egui::vec2(12.0, 30.0), egui::Align2::LEFT_TOP, ammo_text, egui::FontId::proportional(28.0), egui::Color32::WHITE);
                }
                painter.text(egui::pos2(size.x - 20.0, 20.0), egui::Align2::RIGHT_TOP, format!("hits {hits} / {shots}"), egui::FontId::proportional(16.0), egui::Color32::WHITE);
            }
        });
        let prims = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        painter.paint(&self.gpu, rtv, w, h, out.pixels_per_point, &prims, &mut out.textures_delta, true)
    }
}


/// Resolves the frame's draw list (mesh ids by range: 0 scenery, 100 targets, 200 cube, 300 gun parts,
/// 400 gun hands, 500 hand box, 600 valves' wheels, 10 000 physical guns' parts, 20 000 throwables'
/// parts, 30 000 effect meshes) against the scene.
fn to_draws<'a>(scene: &'a Scene, weapon: usize, list: &[(usize, Mat4, [f32; 4], f32, bool)]) -> Vec<Draw<'a>> {
    list.iter()
        .map(|(id, m, t, e, two)| {
            let mesh = match *id {
                30_000.. => &scene.fx[id - 30_000],
                20_000..30_000 => &scene.throwables[(id - 20_000) / 100][(id - 20_000) % 100],
                10_000..20_000 => &scene.weapons[(id - 10_000) / 100].parts[(id - 10_000) % 100],
                0..100 => &scene.scenery[*id],
                100..200 => &scene.target_meshes[id - 100],
                200..300 => &scene.cube,
                300..400 => &scene.weapons[weapon].parts[id - 300],
                400..500 => &scene.weapons[weapon].hands[id - 400],
                600..700 => &scene.valves[id - 600],
                _ => &scene.hand_box,
            };
            Draw { mesh, model: *m, tint: *t, emissive: *e, two_sided: *two }
        })
        .collect()
}
