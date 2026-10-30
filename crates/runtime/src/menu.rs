//! The runtime's in-headset settings menu (egui), the Rust version of the Metro mod's `vrmenu.cpp`.
//! It floats beside the game's pause menu, or in front of you when no game is connected. Point
//! at it and pull the trigger; changes apply at once and are saved.

use anyhow::Result;
use glam::Vec2;
use rustvr_gpu::{EguiPainter, Gpu, TexDesc, Texture};
use windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

use crate::settings::{BodyVisibility, GrenadeArming, Hand, MoveRelative, ReloadStyle, Settings, TurnMode};

const WIDTH_PX: u32 = 1024;
const HEIGHT_PX: u32 = 1280;
const PIXELS_PER_POINT: f32 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Comfort,
    Hands,
    Body,
    Controls,
    Weapon,
    Display,
    Help,
}

/// What the menu shows about the session.
#[derive(Default, Clone)]
pub struct Status {
    pub game: Option<String>,
    pub fps: f32,
    pub backend: String,
    /// The arm span the body uses now (metres, grip to grip).
    pub arm_span: f32,
    /// Your standing head height as measured (tracking space, 0 = not yet).
    pub head_height: f32,
    /// Seconds left before the arms are measured, and the pose they're measured in.
    pub countdown: Option<f32>,
    pub countdown_pose: &'static str,
    /// The headset's per-eye resolution (what it's shown), and the game's (what it draws).
    pub headset_size: (u32, u32),
    pub game_size: Option<(u32, u32)>,
    /// The overscan the game draws with now (less than the setting's while a game that can't
    /// change its size waits to be restarted).
    pub overscan_shown: f32,
}

pub struct RuntimeMenu {
    ctx: egui::Context,
    painter: EguiPainter,
    tex: Texture,
    tab: Tab,
    was_down: bool,
    message: String,
    /// Centres of named widgets as panel UVs (tests aim the laser at them).
    widgets: Vec<(String, Vec2)>,
    /// Settings were changed: save them.
    pub changed: bool,
    pub recenter: bool,
    /// "Measure height" was pressed (stand straight).
    pub measure_height: bool,
    /// "Measure arms" was pressed (T-pose in 3 seconds).
    pub measure_arms: bool,
}

impl RuntimeMenu {
    pub fn new(gpu: &Gpu) -> Result<RuntimeMenu> {
        let desc = TexDesc {
            width: WIDTH_PX,
            height: HEIGHT_PX,
            format: DXGI_FORMAT_R8G8B8A8_TYPELESS,
            rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM),
            srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
            dsv: None,
            mips: 1,
        };
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        Ok(RuntimeMenu {
            ctx,
            painter: EguiPainter::new(gpu)?,
            tex: Texture::new(gpu, &desc)?,
            tab: Tab::Help,
            was_down: false,
            message: "Changes apply immediately".into(),
            widgets: Vec::new(),
            changed: false,
            recenter: false,
            measure_height: false,
            measure_arms: false,
        })
    }

    pub fn aspect(&self) -> f32 {
        WIDTH_PX as f32 / HEIGHT_PX as f32
    }

    pub fn widgets(&self) -> &[(String, Vec2)] {
        &self.widgets
    }

    pub fn set_message(&mut self, m: impl Into<String>) {
        self.message = m.into();
    }

    pub fn srv(&self) -> &ID3D11ShaderResourceView {
        self.tex.srv.as_ref().unwrap()
    }

    /// Runs and paints the menu. `pointer`: where the laser hits it (UV) and whether the trigger
    /// is down.
    pub fn update(&mut self, gpu: &Gpu, s: &mut Settings, pointer: Option<(Vec2, bool)>, status: &Status) -> Result<()> {
        let size = egui::vec2(WIDTH_PX as f32 / PIXELS_PER_POINT, HEIGHT_PX as f32 / PIXELS_PER_POINT);
        let mut events = Vec::new();
        match pointer {
            Some((uv, down)) => {
                let pos = egui::pos2(uv.x * size.x, uv.y * size.y);
                events.push(egui::Event::PointerMoved(pos));
                if down != self.was_down {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: down,
                        modifiers: Default::default(),
                    });
                }
                self.was_down = down;
            }
            None => {
                if self.was_down {
                    events.push(egui::Event::PointerGone);
                }
                self.was_down = false;
            }
        }
        self.ctx.set_pixels_per_point(PIXELS_PER_POINT);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let before = s.clone();
        let mut tab = self.tab;
        let mut recenter = false;
        let mut measure = (false, false);
        let mut save = false;
        let message = self.message.clone();
        let mut rects: Vec<(String, egui::Rect)> = Vec::new();
        let mut out = self.ctx.run_ui(input, |ui| {
            let frame = egui::Frame::new().fill(egui::Color32::from_rgb(18, 20, 24)).inner_margin(14.0);
            frame.show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.heading("RustVR");
                ui.label(match &status.game {
                    Some(g) => format!("{g} · {} · {:.0} fps", status.backend, status.fps),
                    None => format!("No game connected · {}", status.backend),
                });
                ui.separator();
                ui.horizontal(|ui| {
                    for (t, name) in [
                        (Tab::Help, "Help"),
                        (Tab::Comfort, "Comfort"),
                        (Tab::Hands, "Hands"),
                        (Tab::Body, "Body"),
                        (Tab::Controls, "Controls"),
                        (Tab::Weapon, "Weapon"),
                        (Tab::Display, "Display"),
                    ] {
                        let r = ui.selectable_value(&mut tab, t, name);
                        rects.push((format!("tab:{name}"), r.rect));
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(size.y - 190.0).show(ui, |ui| match tab {
                    Tab::Comfort => comfort(ui, s, &mut rects),
                    Tab::Hands => hands(ui, s, &mut rects),
                    Tab::Body => measure = body(ui, s, status, &mut rects),
                    Tab::Help => help(ui),
                    Tab::Controls => controls(ui, s),
                    Tab::Weapon => weapon(ui, s),
                    Tab::Display => display(ui, s, status),
                });
                ui.separator();
                ui.horizontal(|ui| {
                    let r = ui.add_sized([150.0, 40.0], egui::Button::new("Recenter"));
                    rects.push(("recenter".into(), r.rect));
                    if r.clicked() {
                        recenter = true;
                    }
                    let r = ui.add_sized([150.0, 40.0], egui::Button::new("Save"));
                    rects.push(("save".into(), r.rect));
                    if r.clicked() {
                        save = true;
                    }
                });
                ui.label(egui::RichText::new(message.as_str()).weak());
            });
        });
        self.tab = tab;
        self.widgets = rects.into_iter().map(|(n, r)| (n, Vec2::new(r.center().x / size.x, r.center().y / size.y))).collect();
        if measure.0 {
            self.measure_height = true;
        }
        if measure.1 {
            self.measure_arms = true;
            self.message = "Stand straight and hold your arms straight out to the sides".into();
        }
        if recenter {
            self.recenter = true;
            self.message = "View will recenter when you resume".into();
        }
        if *s != before || save {
            self.changed = true;
            self.message = if save { "Saved".into() } else { "Changes apply immediately".into() };
        }
        let prims = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        self.painter
            .paint(gpu, self.tex.rtv.as_ref().unwrap(), WIDTH_PX, HEIGHT_PX, out.pixels_per_point, &prims, &mut out.textures_delta, true)
    }
}

fn slider(ui: &mut egui::Ui, label: &str, v: &mut f32, lo: f32, hi: f32, suffix: &str, decimals: usize) {
    ui.add(egui::Slider::new(v, lo..=hi).text(label).suffix(suffix).fixed_decimals(decimals));
}

fn comfort(ui: &mut egui::Ui, s: &mut Settings, rects: &mut Vec<(String, egui::Rect)>) {
    let c = &mut s.comfort;
    ui.horizontal(|ui| {
        ui.label("Turning");
        let r = ui.selectable_value(&mut c.turn_mode, TurnMode::Snap, "Snap");
        rects.push(("turn:snap".into(), r.rect));
        let r = ui.selectable_value(&mut c.turn_mode, TurnMode::Smooth, "Smooth");
        rects.push(("turn:smooth".into(), r.rect));
    });
    match c.turn_mode {
        TurnMode::Snap => slider(ui, "Snap angle", &mut c.snap_angle, 10.0, 90.0, " deg", 0),
        TurnMode::Smooth => slider(ui, "Smooth turn speed", &mut c.smooth_turn_speed, 30.0, 360.0, " deg/s", 0),
    }
    ui.horizontal(|ui| {
        ui.label("Walk direction");
        ui.selectable_value(&mut c.move_relative, MoveRelative::Head, "Head");
        ui.selectable_value(&mut c.move_relative, MoveRelative::OffHand, "Off-hand");
    });
    ui.checkbox(&mut c.roomscale, "Roomscale (body follows you)");
    if c.roomscale {
        slider(ui, "Max distance from body", &mut c.roomscale_limit, 0.2, 1.5, " m", 2);
    }
    ui.checkbox(&mut s.input.physical_crouch, "Duck to crouch");
    if s.input.physical_crouch {
        slider(ui, "Crouch depth", &mut s.input.crouch_threshold, 0.15, 0.7, " m", 2);
    }
    slider(ui, "Height offset", &mut s.comfort.height_offset, -0.5, 0.5, " m", 2);
    ui.checkbox(&mut s.comfort.camera_anims, "Follow scripted camera (ladders, cut-scenes)");
    ui.checkbox(&mut s.comfort.head_bob, "Camera bob / shake");
    slider(ui, "Vignette while moving", &mut s.comfort.vignette, 0.0, 1.0, "", 2);
    ui.checkbox(&mut s.comfort.wall_fade, "Fade out when your head goes into a wall");
    ui.checkbox(&mut s.comfort.seated, "Seated play (see the Body tab)");
}

/// Your body: height, arms, seated play. Returns (measure height, measure arms) presses.
fn body(ui: &mut egui::Ui, s: &mut Settings, status: &Status, rects: &mut Vec<(String, egui::Rect)>) -> (bool, bool) {
    let mut out = (false, false);
    ui.heading("Height");
    let r = ui.checkbox(&mut s.comfort.seated, "Seated mode (you see from standing height; ducking doesn't crouch)");
    rects.push(("body:seated".into(), r.rect));
    ui.checkbox(&mut s.comfort.real_height, "Play at my real height (otherwise the game character's)");
    slider(ui, "Your height (0 = measure as you stand)", &mut s.body.height, 0.0, 2.2, " m", 2);
    ui.horizontal(|ui| {
        let r = ui.add_enabled(!s.comfort.seated, egui::Button::new("Measure my height (stand up straight)"));
        rects.push(("body:measure_height".into(), r.rect));
        out.0 = r.clicked();
        if status.head_height > 0.5 {
            ui.label(format!("eyes at {:.2} m", status.head_height));
        }
    });
    slider(ui, "Height offset", &mut s.comfort.height_offset, -0.5, 0.5, " m", 2);
    ui.separator();
    ui.heading("Arms");
    slider(ui, "Arm span the body is built for (0 = automatic)", &mut s.body.arm_span, 0.0, 2.4, " m", 2);
    ui.label(format!("Using {:.2} m{}", status.arm_span, if s.body.arm_span <= 1.0 { " (grows when your real arms reach further)" } else { "" }));
    ui.horizontal(|ui| {
        let r = ui.add_enabled(status.countdown.is_none(), egui::Button::new("Measure my arms"));
        rects.push(("body:measure_arms".into(), r.rect));
        out.1 = r.clicked();
        if let Some(t) = status.countdown {
            ui.label(egui::RichText::new(format!("{}... {:.0}", status.countdown_pose, t.ceil())).strong());
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Show");
        ui.selectable_value(&mut s.body.visibility, BodyVisibility::Hands, "Hands");
        ui.selectable_value(&mut s.body.visibility, BodyVisibility::Arms, "Arms");
        ui.selectable_value(&mut s.body.visibility, BodyVisibility::Full, "Full body");
    });
    ui.checkbox(&mut s.body.game_lighting, "Light the body with the game's lighting");
    ui.checkbox(&mut s.body.tracked_elbows, "Elbows from the headset's body tracking (Quest 3 / Pro)");
    slider(ui, "Hand size (1 = a real hand for your height)", &mut s.body.hand_size, 0.8, 1.3, "x", 2);
    out
}

/// Every control and feature, for players who just put the headset on.
fn help(ui: &mut egui::Ui) {
    let section = |ui: &mut egui::Ui, title: &str, rows: &[(&str, &str)]| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(title).strong().size(15.0));
        egui::Grid::new(title).num_columns(2).spacing([12.0, 3.0]).striped(true).show(ui, |ui| {
            for (k, v) in rows {
                ui.label(egui::RichText::new(*k).color(egui::Color32::from_rgb(140, 200, 255)));
                ui.label(*v);
                ui.end_row();
            }
        });
    };
    ui.label("Right-handed layout; swap the weapon hand in Controls.");
    section(ui, "Moving", &[
        ("Left stick", "walk (click: sprint)"),
        ("Right stick left / right", "snap turn"),
        ("Right stick down / click", "crouch"),
        ("A or X, empty hand", "jump"),
        ("Duck for real", "crouch"),
        ("Menu button", "pause, and this menu"),
    ]);
    section(ui, "Hands", &[
        ("Grip", "grab what your hand is on (hold to keep holding)"),
        ("Trigger, empty hand", "use"),
        ("Point your hand or palm at something far", "it glows (gravity gloves, up to 10 m)"),
        ("Hold grip on it", "it locks on and hops towards you"),
        ("Flick your wrist back (or yank)", "it flies to you: keep the grip to catch it"),
        ("Let go while moving", "throw (the speed at your swing's peak counts, not the release)"),
        ("Walls and tables", "stop your hands (a ghost of the real one is in the Hands tab)"),
    ]);
    section(ui, "Guns", &[
        ("Trigger", "fire (one-handed or off the shoulder it kicks harder)"),
        ("Other hand on the barrel + grip", "two-handed: locks on, slides along the handguard"),
        ("A on the gun hand", "drop the magazine (squeeze the other hand under the gun to catch it)"),
        ("Grip at your chest or belt", "a fresh magazine (unlimited) while the other hand holds a gun"),
        ("Push it up into the gun", "it seats itself"),
        ("Grab the charging handle / slide, pull back", "chamber a round (needed after a fresh magazine)"),
        ("B on the gun hand", "drop a locked-back slide"),
        ("Hold B, empty hand", "weapon wheel (games that have one)"),
    ]);
    section(ui, "Guns worked by hand", &[
        ("B on a double barrel or a revolver", "open it: the barrels drop (the cylinder swings out); a flick down works too"),
        ("Flick it up (roll a revolver back)", "snap it shut; or push the barrels home with the other hand"),
        ("Grip at your chest or belt", "one shell (or dart) at a time: push it base first into a chamber or the port"),
        ("A with the cylinder out", "drop a revolver's cases"),
        ("Bolt: lift, pull back, push, turn down", "after every shot (the case flies out, the next goes in)"),
        ("Pump an air gun", "each whole stroke builds pressure (watch the gauge); empty, it only hisses"),
    ]);
    section(ui, "Grenades and knives", &[
        ("Grip at either side of your chest, or the belt", "a grenade or knife from its pouch"),
        ("Other hand on the pin + pull", "pull the pin (Hands tab: or the trigger arms it)"),
        ("Let go / B on the grenade hand", "the lever flies and the fuse starts / cook it in your hand"),
        ("Trigger or A on the grenade hand", "arm it one-handed while the other hand holds a gun"),
        ("Throw a knife", "it flies blade first and sticks; grab it to pull it out"),
    ]);
    section(ui, "Games that keep their own gun (Metro 2033)", &[
        ("Trigger", "fire; raise the sights to your eye to aim down them"),
        ("Other hand on the barrel + grip", "two-handed (one-handed kicks three times harder)"),
        ("A on the gun hand", "drop the magazine; grip at your hip for a fresh one, push it into the gun, pull the charging handle"),
        ("Hold A on the gun hand", "change the gas mask's filter"),
        ("B on the gun hand", "jump"),
        ("A on the other hand / hold it", "use / the journal and map"),
        ("B on the other hand / hold it", "flashlight / charge it"),
        ("Trigger on the other hand", "throw a grenade"),
        ("Grip at your face / the top of your head", "gas mask / night vision"),
        ("Grip over your shoulder", "next weapon"),
        ("Other wrist up in front of your eyes", "your watch (the light and the filter's time)"),
    ]);
    section(ui, "Holsters", &[
        ("Let go of a gun at a hip", "holster it there"),
        ("Let go of it behind your shoulder", "carry it on your back (both shoulders)"),
        ("Grip at the hip / over the shoulder", "draw it"),
    ]);
    section(ui, "Body", &[
        ("Body tab", "your height, arm span, seated mode"),
        ("Measure my arms", "arms straight out to the sides for 3 seconds, then straight out in front for 3"),
        ("Hands tab", "physics hands, gloves, grip toggle, weight, laser, stock, the game's recoil"),
    ]);
}

fn hands(ui: &mut egui::Ui, s: &mut Settings, rects: &mut Vec<(String, egui::Rect)>) {
    let h = &mut s.interaction;
    let r = ui.checkbox(&mut h.physics_hands, "Physics hands (walls stop them)");
    rects.push(("hands:physics".into(), r.rect));
    if h.physics_hands {
        ui.checkbox(&mut h.ghost_hands, "Ghost hands (a see-through hand where your real one is when it's held back)");
    }
    slider(ui, "Grab transition (fast .. slow)", &mut h.grab_transition, 0.0, 0.4, " s", 2);
    ui.checkbox(&mut h.grip_toggle, "Grip toggles (press once to hold, again to let go)");
    ui.checkbox(&mut h.auto_holster, "Guns you let go of return to their holster");
    let r = ui.checkbox(&mut h.gravity_gloves, "Gravity gloves (point, grip, flick to pull)");
    rects.push(("hands:gloves".into(), r.rect));
    if h.gravity_gloves {
        slider(ui, "Glove range", &mut h.glove_range, 1.0, 10.0, " m", 1);
        slider(ui, "Flick speed", &mut h.glove_flick_speed, 0.6, 3.0, " m/s", 1);
    }
    ui.checkbox(&mut h.easy_mag, "Easy magazine insertion");
    ui.checkbox(&mut h.virtual_stock, "Virtual stock (shouldered rifles aim from the shoulder)");
    ui.checkbox(&mut h.game_recoil, "The game's own recoil (off: RustVR's)");
    if h.game_recoil {
        slider(ui, "Game recoil: turn", &mut h.game_recoil_turn, 0.0, 2.0, "x", 2);
        slider(ui, "Game recoil: push back", &mut h.game_recoil_push, 0.0, 2.0, "x", 2);
        slider(ui, "Recoil one-handed", &mut h.recoil_one_hand, 1.0, 5.0, "x", 1);
        slider(ui, "Recoil off the shoulder", &mut h.recoil_unbraced, 1.0, 3.0, "x", 1);
    }
    ui.checkbox(&mut h.laser_sight, "Laser sight on guns");
    ui.checkbox(&mut h.mag_slam_release, "Seating a magazine drops a locked slide");
    slider(ui, "Grab reach", &mut h.grab_radius, 0.03, 0.15, " m", 2);
    slider(ui, "Weight (heavy things lag)", &mut h.weight, 0.0, 1.0, "", 2);
    slider(ui, "Mechanisms' stiffness (valves, switches)", &mut h.mechanism_stiffness, 0.25, 2.0, "x", 2);
    slider(ui, "Throw strength", &mut h.throw_scale, 0.6, 2.0, "x", 2);
    ui.horizontal(|ui| {
        ui.label("Grenades");
        ui.selectable_value(&mut h.grenade_arming, GrenadeArming::Pin, "Pull the pin with the other hand");
        ui.selectable_value(&mut h.grenade_arming, GrenadeArming::Button, "Trigger arms it");
    });
    ui.checkbox(&mut h.knife_assist, "Forgiving knife throws (they stick more)");
    ui.separator();
    ui.checkbox(&mut s.sound.enabled, "Interaction sounds");
    if s.sound.enabled {
        slider(ui, "Volume", &mut s.sound.volume, 0.0, 1.0, "", 2);
        ui.checkbox(&mut s.sound.shots, "Gunshot sounds");
        ui.checkbox(&mut s.sound.game_sounds, "The game's own sounds (off: RustVR's)");
    }
}

fn controls(ui: &mut egui::Ui, s: &mut Settings) {
    let i = &mut s.input;
    ui.horizontal(|ui| {
        ui.label("Weapon hand");
        ui.selectable_value(&mut i.dominant_hand, Hand::Left, "Left");
        ui.selectable_value(&mut i.dominant_hand, Hand::Right, "Right");
    });
    ui.checkbox(&mut i.two_handed_aim, "Two-handed aiming");
    if i.two_handed_aim {
        slider(ui, "Foregrip reach", &mut i.two_handed_grab_dist, 0.1, 0.5, " m", 2);
    }
    ui.checkbox(&mut i.gestures, "Body gestures (mask, medkit, knife, ...)");
    ui.checkbox(&mut i.manual_reload, "Reload with your hands");
    if i.manual_reload {
        ui.horizontal(|ui| {
            ui.label("Reload");
            ui.selectable_value(&mut i.reload_style, ReloadStyle::Full, "Eject, insert, rack");
            ui.selectable_value(&mut i.reload_style, ReloadStyle::Slap, "Slap the magazine in");
        });
    }
    ui.checkbox(&mut i.ability_flicks, "Ability flicks (hold grip, flick a direction)");
    ui.checkbox(&mut i.melee_swing, "Melee by swinging your hand");
    if i.melee_swing {
        slider(ui, "Strike speed", &mut i.melee_speed, 1.5, 5.0, " m/s", 1);
    }
    slider(ui, "Stick deadzone", &mut i.stick_deadzone, 0.0, 0.5, "", 2);
    ui.checkbox(&mut i.arm_swimming, "Swim with your arms (in the game's water)");
    if i.arm_swimming {
        slider(ui, "Stroke strength", &mut i.swim_ease, 0.5, 3.0, "x", 1);
    }
}

fn weapon(ui: &mut egui::Ui, s: &mut Settings) {
    let w = &mut s.weapon;
    slider(ui, "Offset left / right", &mut w.offset[0], -0.2, 0.2, " m", 3);
    slider(ui, "Offset up / down", &mut w.offset[1], -0.2, 0.2, " m", 3);
    slider(ui, "Offset forward / back", &mut w.offset[2], -0.2, 0.2, " m", 3);
    slider(ui, "Pitch", &mut w.pitch, -45.0, 45.0, " deg", 1);
    slider(ui, "Yaw", &mut w.yaw, -45.0, 45.0, " deg", 1);
    slider(ui, "Roll", &mut w.roll, -45.0, 45.0, " deg", 1);
    ui.checkbox(&mut s.scope.enabled, "Picture-in-picture scopes");
    if s.scope.enabled {
        let mut every = s.scope.every as i32;
        ui.add(egui::Slider::new(&mut every, 1..=4).text("Scope refresh divider"));
        s.scope.every = every.max(1) as u32;
    }
}

fn display(ui: &mut egui::Ui, s: &mut Settings, status: &Status) {
    slider(ui, "Supersampling (the game's resolution, each way)", &mut s.vr.render_scale, 0.5, 2.0, "x", 2);
    let (hw, hh) = status.headset_size;
    ui.label(match status.game_size {
        Some((w, h)) => format!("The headset is shown {hw} x {hh} an eye; the game draws {w} x {h}"),
        None => format!("The headset is shown {hw} x {hh} an eye"),
    });
    slider(ui, "World scale", &mut s.vr.world_scale, 0.7, 1.4, "", 2);
    ui.checkbox(&mut s.vr.reproject, "Smooth the game's picture to every headset frame (walking and turning too)");
    ui.checkbox(&mut s.vr.predict, "Draw the game for when its picture is shown (head and hands a frame ahead)");
    ui.checkbox(&mut s.vr.headset_depth, "Give the headset each frame's depth");
    ui.horizontal(|ui| {
        ui.label("Refresh rate");
        for (hz, name) in [(0.0, "highest"), (72.0, "72"), (90.0, "90"), (120.0, "120")] {
            ui.radio_value(&mut s.vr.refresh_hz, hz, name);
        }
    });
    // (More draws more: for a wider desktop view when recording, in games that can spare it.)
    slider(ui, "Overscan: drawn past the headset's view", &mut s.vr.overscan, 0.0, 30.0, " deg", 0);
    if s.vr.overscan - status.overscan_shown > 0.5 {
        ui.label(format!(
            "The game can't change its size while it runs: {:.0} of the {:.0} degrees show now, all of them when it's restarted",
            status.overscan_shown, s.vr.overscan
        ));
    }
    slider(ui, "HUD distance", &mut s.ui.hud_distance, 0.6, 3.0, " m", 2);
    slider(ui, "HUD size", &mut s.ui.hud_size, 0.6, 3.0, " m", 2);
    ui.checkbox(&mut s.ui.hud_follow_head, "HUD follows your head");
    slider(ui, "HUD follows after a turn of", &mut s.ui.hud_deadzone_deg, 0.0, 45.0, " deg", 0);
    ui.checkbox(&mut s.ui.split_hud, "HUD in pieces (subtitles low, ammo on the gun) when the game supports it");
    if s.ui.split_hud {
        slider(ui, "Subtitles: distance", &mut s.ui.caption_distance, 0.6, 3.0, " m", 2);
        slider(ui, "Subtitles: below gaze", &mut s.ui.caption_pitch_deg, 0.0, 30.0, " deg", 0);
        slider(ui, "Subtitles: text size", &mut s.ui.caption_width_deg, 40.0, 120.0, "", 0);
        slider(ui, "Ammo counter size", &mut s.ui.ammo_width, 0.05, 0.25, " m", 2);
    }
    slider(ui, "Menu distance", &mut s.ui.menu_distance, 1.0, 4.0, " m", 2);
    slider(ui, "Menu size", &mut s.ui.menu_size, 1.0, 5.0, " m", 2);
    ui.separator();
    ui.label("Desktop view (the window on your monitor, for recording)");
    ui.horizontal(|ui| {
        ui.label("Game picture from the");
        ui.radio_value(&mut s.spectator.eye, crate::settings::Eye::Left, "left eye");
        ui.radio_value(&mut s.spectator.eye, crate::settings::Eye::Right, "right eye (your aiming eye)");
    });
    slider(ui, "Desktop field of view", &mut s.spectator.fov, 60.0, 140.0, " deg", 0);
    slider(ui, "Desktop camera smoothing", &mut s.spectator.smoothing, 0.0, 0.4, " s", 2);
    slider(ui, "Desktop camera lag at most", &mut s.spectator.max_lag, 0.0, 20.0, " deg", 0);
    ui.checkbox(&mut s.spectator.roll_lock, "Keep the desktop view's horizon level");
    ui.checkbox(&mut s.spectator.fullscreen, "Desktop view opens fullscreen (F11 switches)");
    ui.checkbox(&mut s.ui.obs_view, "OBS in the headset: look at the back of your left wrist, or F9 to pin it");
}
