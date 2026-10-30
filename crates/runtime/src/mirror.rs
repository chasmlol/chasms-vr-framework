//! The mirror: a standing mirror in the game's world, in which the player sees their whole body
//! (the game's body, head and face, lit by the game's lights) with the game's world behind them.
//! It's for a game's character creator (the game asks for it: [`GS_MIRROR`]) and for showing off
//! (the player calls it up from the menu's Body tab, or by pulling their gripped hands apart in
//! front of the chest, as if opening curtains; the same puts it away).
//!
//! **Summoned by the game.** A game can give the player a spell for it ([`EV_MIRROR_CAST`]): cast
//! while looking at the mirror, it sends it away; cast anywhere else, it summons one in front. The
//! game plays its own effect at the glass, and the glass appears (or goes) at the effect's moment
//! ([`EV_MIRROR_HIT`]), as a summoned creature appears at its effect's.
//!
//! **Where it stands.** In the game's world, so it stays put as the player walks and turns: on the
//! floor, in front of where the head faced when it was called, its glass facing the player
//! ([`crate::settings::Mirror`]: 1.2 m off, 0.95 m wide and 2 m tall by default; nearer when a
//! wall is closer, never nearer than 0.6 m).
//!
//! **What each eye sees in it.** The reflection is drawn every headset frame from this frame's
//! poses: each eye's view is the camera at that eye reflected in the glass's plane, looking
//! straight through the glass (its frustum through the glass's corners, its near plane on the
//! glass: [`Glass::camera`]). Into it go, in this order:
//!
//! * **The game's world behind the player.** The adapter renders it once a frame from the head
//!   reflected ([`MirrorView`], into the aux texture with its depth); each eye's picture is that
//!   moved to the eye's own reflected camera by its depth (`crate::warp`), so one game render gives
//!   both eyes their parallax. Without it the glass is dark.
//! * **The player**, posed this frame, with the head (no first-person collapse), the whole body
//!   whatever the body setting, and what the hands hold, hidden behind what's nearer in the game's
//!   picture.
//!
//! Then the glass is drawn in each eye with its reflection, hidden behind the game's world where
//! that's nearer (a mirror half through a doorway shows half).

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Quat, Vec3};
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer};
use rustvr_gpu::shader::Program;
use rustvr_gpu::{Gpu, TexDesc, Texture};
#[cfg(doc)]
use rustvr_protocol::{EV_MIRROR_CAST, EV_MIRROR_HIT};
use rustvr_protocol::{Fov, GS_MENU, GS_MIRROR, GS_PAUSED, MIR_RENDER, MIR_SHOWN, MirrorView, Pose};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};

use crate::backend::EyeTarget;
use crate::compositor::{projection, view_matrix};
use crate::rig::WorldMap;

/// The glass in the dark, where the game's world isn't in it (linear).
const DARK: [f32; 4] = [0.012, 0.013, 0.016, 1.0];
/// Each eye's reflection's width, pixels (its height follows the glass's shape).
const REFLECTION_WIDTH: u32 = 1024;
/// The frame round the glass, a fraction of its width.
const BORDER: f32 = 0.018;

const SHADER: &str = r#"
cbuffer Glass : register(b0) {
    float4x4 view_proj;   // tracking -> this eye's clip space
    float4x4 reflect_vp;  // tracking -> the reflection's clip space (this eye reflected)
    float4 corners[4];    // the glass: bottom left, bottom right, top right, top left (tracking)
    float4 view_z;        // this eye's view depth (metres) = dot(xyz, p) + w
    float4 depth;         // x: the game's depth bound, y: metres per its unit, zw: the frame's width across, up (0..1)
    float4 target;        // xy: the eye's target size (pixels)
};
Texture2D reflection : register(t0);
Texture2D<float> game_depth : register(t1);
SamplerState samp : register(s0);

struct VOut {
    float4 pos : SV_Position;
    float3 world : TEXCOORD0;
    float2 uv : TEXCOORD1;
};

VOut VSMain(uint id : SV_VertexID) {
    uint idx[6] = { 0, 1, 2, 0, 2, 3 };
    float2 uvs[4] = { float2(0, 1), float2(1, 1), float2(1, 0), float2(0, 0) };
    uint k = idx[id];
    VOut o;
    o.world = corners[k].xyz;
    o.pos = mul(view_proj, float4(o.world, 1));
    o.uv = uvs[k];
    return o;
}

float4 PSMain(VOut i) : SV_Target {
    // Hidden where the game's world is nearer.
    float z = dot(view_z.xyz, i.world) + view_z.w;
    if (depth.x > 0) {
        uint w, h;
        game_depth.GetDimensions(w, h);
        float g = game_depth.Load(int3(i.pos.xy / target.xy * float2(w, h), 0)) * depth.y;
        if (g > 0 && g < z - 0.004 - 0.002 * z) discard;
    }
    // The reflection: where this point of the glass is in the reflected eye's picture.
    float4 c = mul(reflect_vp, float4(i.world, 1));
    float2 uv = float2(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
    float3 col = reflection.Sample(samp, uv).rgb;
    // A thin dark frame round the glass, so it reads as a mirror and not a hole.
    float2 e = min(i.uv, 1 - i.uv) / depth.zw;
    float frame = saturate(min(e.x, e.y) - 1);
    col = lerp(float3(0.018, 0.016, 0.014), col, frame);
    return float4(col, 1);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlassCb {
    view_proj: [f32; 16],
    reflect_vp: [f32; 16],
    corners: [[f32; 4]; 4],
    view_z: [f32; 4],
    depth: [f32; 4],
    target: [f32; 4],
}

/// The glass this frame, tracking space (left-handed: X right, Y up, Z forward, as the protocol's):
/// its centre and its half extents to the right (as seen from in front) and up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glass {
    pub center: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

impl Glass {
    /// The way its glass faces (towards the player).
    pub fn normal(&self) -> Vec3 {
        self.up.cross(self.right).normalize_or_zero()
    }

    /// Its corners: bottom left, bottom right, top right, top left.
    pub fn corners(&self) -> [Vec3; 4] {
        let (c, r, u) = (self.center, self.right, self.up);
        [c - r - u, c + r - u, c + r + u, c - r + u]
    }

    /// The camera that sees in the glass what an eye at `eye` sees in the mirror: at the eye
    /// reflected in the glass's plane, looking straight through the glass (its frustum through the
    /// glass's corners, its near plane, the returned distance, on the glass). None when the eye is
    /// behind the glass.
    ///
    /// Looking through the glass from behind it, its own right is the mirror's left: the picture is
    /// the mirror's own, flipped as a reflection is, with nothing flipped in the drawing.
    pub fn camera(&self, eye: Vec3) -> Option<(Pose, Fov, f32)> {
        let n = self.normal();
        let d = (eye - self.center).dot(n);
        if d < 0.02 {
            return None;
        }
        let at = eye - 2.0 * d * n;
        // Forward (+Z) through the glass towards the player, up the glass's up, right (+X, the
        // mirror's left) from those.
        let up = self.up.normalize_or_zero();
        let x = up.cross(n);
        let rot = Quat::from_mat3(&Mat3::from_cols(x, up, n)).normalize();
        let (mut l, mut r, mut b, mut t) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for k in self.corners() {
            let rel = k - at;
            let (tx, ty) = (rel.dot(x) / d, rel.dot(up) / d);
            (l, r, b, t) = (l.min(tx), r.max(tx), b.min(ty), t.max(ty));
        }
        let fov = Fov { left: l, right: r, up: t, down: b };
        Some((Pose { pos: at.to_array(), valid: 1, rot: rot.to_array() }, fov, d))
    }
}

/// Where the mirror stands: its glass's centre and the way it faces (yaw), in the game's world
/// when there is one (else tracking space).
#[derive(Clone, Copy, Debug)]
struct Place {
    center: Vec3,
    yaw: f32,
    in_world: bool,
}

/// One eye's reflection: its picture and the camera it was drawn from.
struct Reflection {
    tex: Texture,
    target: EyeTarget,
    view_proj: Mat4,
    drawn: bool,
}

/// The gesture that calls the mirror up or puts it away: both grips held with empty hands close
/// together in front of the chest, then pulled apart wide.
#[derive(Default)]
struct Curtains {
    /// When the hands were together, gripped (seconds).
    together: Option<f64>,
}

/// The glass is rendered while some of it is within this angle of where the head looks (radians:
/// past the headset's edge, so it's there before it comes into view).
const SEEN_WITHIN: f32 = 1.35;

/// A mirror farther than this from the head (metres) is put away.
const FORGET_FARTHER: f32 = 25.0;

/// How long the hands may take to go from together to apart (seconds), and how near and how far.
const CURTAINS_TIME: f64 = 0.8;
const CURTAINS_TOGETHER: f32 = 0.22;
const CURTAINS_APART: f32 = 0.75;

/// The game's mirror spell sends away a mirror the head faces, within this angle of where it looks
/// (radians) and this near (metres); cast anywhere else, it summons one in front.
const SEND_AWAY_WITHIN: f32 = 0.6;
const SEND_AWAY_NEARER: f32 = 6.0;

/// A summons the game doesn't answer ends anyway after this long (seconds of the game running, not
/// paused): a game with no effect for it, or one that lost it.
const SUMMONS_LONGEST: f64 = 4.0;

/// The mirror being summoned or sent away with the game's effect: the glass appears, or goes, at
/// the effect's moment.
#[derive(Clone, Copy, Debug)]
struct Summons {
    /// Its count ([`MirrorView::summons`]).
    count: u32,
    /// Appearing (else going).
    appearing: bool,
    /// How long the game has been playing its effect (seconds, while the game runs).
    waited: f64,
}

pub struct Mirror {
    place: Option<Place>,
    /// The player called it up (the menu, the gesture, the game's spell).
    pub called: bool,
    /// The game wants it (its character creator is open).
    game: bool,
    glass: Option<Glass>,
    /// The game's mirror spell was cast (handled in the next update, where the head is known), and
    /// the game's effect for a summons reached its moment.
    cast: bool,
    hit: Option<u32>,
    summons: Option<Summons>,
    /// Summons so far ([`MirrorView::summons`]).
    summons_count: u32,
    /// The last update's time (seconds).
    last_now: f64,
    /// The cameras asked of the game for its world, by frame: its picture comes back a frame or
    /// more later.
    asked: [(u64, Pose, Fov); 32],
    eyes: [Option<Reflection>; 2],
    curtains: Curtains,
    program: Option<Program>,
    cb: Option<ConstantBuffer<GlassCb>>,
    sampler: Option<ID3D11SamplerState>,
    blend: Option<ID3D11BlendState>,
    depth_off: Option<ID3D11DepthStencilState>,
    raster: Option<ID3D11RasterizerState>,
}

impl Default for Mirror {
    fn default() -> Self {
        Mirror {
            place: None,
            called: false,
            game: false,
            glass: None,
            cast: false,
            hit: None,
            summons: None,
            summons_count: 0,
            last_now: 0.0,
            asked: [(0, Pose::default(), Fov::default()); 32],
            eyes: [None, None],
            curtains: Curtains::default(),
            program: None,
            cb: None,
            sampler: None,
            blend: None,
            depth_off: None,
            raster: None,
        }
    }
}

/// What the mirror is placed from this frame.
pub struct MirrorInput<'a> {
    pub settings: &'a crate::settings::Mirror,
    pub game_flags: u32,
    pub head: (Vec3, Quat),
    /// The game's floor in tracking space.
    pub floor: f32,
    pub map: Option<&'a WorldMap>,
    /// The controllers, and whether each hand is empty.
    pub hands: &'a [rustvr_protocol::Controller; 2],
    pub empty: [bool; 2],
    pub now: f64,
    /// How far the world's collision is from a point along a direction (metres), within a reach.
    pub wall: &'a dyn Fn(Vec3, Vec3, f32) -> Option<f32>,
}

impl Mirror {
    /// The mirror is up (and not still being summoned).
    pub fn shown(&self) -> bool {
        self.glass.is_some() && !self.summons.is_some_and(|s| s.appearing)
    }

    /// The glass this frame (tracking space), when the mirror stands (shown, or being summoned).
    pub fn glass(&self) -> Option<Glass> {
        self.glass
    }

    /// The game's mirror spell was cast ([`EV_MIRROR_CAST`]).
    pub fn cast(&mut self) {
        self.cast = true;
    }

    /// The game's effect for the summons `count` reached its moment ([`EV_MIRROR_HIT`]).
    pub fn hit(&mut self, count: u32) {
        self.hit = Some(count);
    }

    /// A point's reflection in the glass (where you see it in the mirror), when the mirror is up
    /// and the point is in front of it.
    pub fn reflected(&self, p: Vec3) -> Option<Vec3> {
        let g = self.glass?;
        let n = g.normal();
        let d = (p - g.center).dot(n);
        (d > 0.0).then(|| p - 2.0 * d * n)
    }

    /// Calls the mirror up, or puts it away.
    pub fn toggle(&mut self) {
        self.called = !(self.called || self.game);
        if !self.called {
            self.game = false;
        }
        tracing::info!("mirror: {}", if self.called { "called up" } else { "put away" });
    }

    /// One frame: the gesture, whether it's up, where it stands, its glass in tracking space now.
    pub fn update(&mut self, inp: &MirrorInput) {
        let s = inp.settings;
        // The curtains: both hands gripped and empty, together in front of the chest, then apart.
        let gripped = (0..2).all(|i| inp.hands[i].connected != 0 && inp.hands[i].squeeze > 0.7 && inp.empty[i]);
        let (a, b) = (Vec3::from_array(inp.hands[0].grip.pos), Vec3::from_array(inp.hands[1].grip.pos));
        let (head, rot) = inp.head;
        let fwd = flat(rot * Vec3::Z);
        let in_front = |p: Vec3| (p - head).dot(fwd) > 0.1 && p.y < head.y - 0.05 && p.y > head.y - 0.75;
        if !s.gesture || !gripped {
            self.curtains.together = None;
        } else if a.distance(b) < CURTAINS_TOGETHER && in_front(a) && in_front(b) {
            self.curtains.together = Some(inp.now);
        } else if let Some(t) = self.curtains.together
            && a.distance(b) > CURTAINS_APART
        {
            self.curtains.together = None;
            if inp.now - t < CURTAINS_TIME {
                self.toggle();
            }
        }
        // The game's mirror spell: it sends away the mirror the head faces, else summons one in
        // front (stood anew, where the gesture would stand it). The glass appears or goes at the
        // game's effect's moment, or after a while if the game never says.
        let dt = (inp.now - self.last_now).clamp(0.0, 0.25);
        self.last_now = inp.now;
        if std::mem::take(&mut self.cast) {
            if self.summons.is_some() {
                tracing::info!("mirror: the spell again while it's being summoned: nothing more");
            } else {
                let look = rot * Vec3::Z;
                let faced = self.glass.is_some_and(|g| {
                    let to = g.center - head;
                    to.length() < SEND_AWAY_NEARER && to.normalize_or_zero().dot(look) > SEND_AWAY_WITHIN.cos()
                });
                self.summons_count = self.summons_count.wrapping_add(1).max(1);
                self.summons = Some(Summons { count: self.summons_count, appearing: !faced, waited: 0.0 });
                if !faced {
                    (self.place, self.called) = (None, true);
                }
                tracing::info!("mirror: the spell {} (summons {})", if faced { "sends it away" } else { "summons it in front" }, self.summons_count);
            }
        }
        let hit = self.hit.take();
        if let Some(s) = self.summons.as_mut() {
            if inp.game_flags & (GS_MENU | GS_PAUSED) == 0 {
                s.waited += dt;
            }
            let answered = hit == Some(s.count);
            if answered || s.waited > SUMMONS_LONGEST {
                let s = *s;
                self.summons = None;
                tracing::info!(
                    "mirror: {}{}",
                    if s.appearing { "summoned" } else { "sent away" },
                    if answered { " at the game's effect's moment" } else { ": the game never said when" }
                );
                if !s.appearing {
                    (self.place, self.glass, self.called) = (None, None, false);
                }
            }
        }
        // The game's character creator: up while it's open, gone after unless the player called it.
        let game = inp.game_flags & GS_MIRROR != 0;
        if game != self.game {
            self.game = game;
            if !game && !self.called {
                tracing::info!("mirror: the game is done with it");
            }
        }
        if !(self.called || self.game) {
            (self.place, self.glass, self.summons) = (None, None, None);
            return;
        }
        let place = *self.place.get_or_insert_with(|| {
            // In front of where the head faces, nearer when a wall is closer.
            let chest = Vec3::new(head.x, (head.y - 0.4).max(inp.floor + 0.5), head.z);
            let wall = (inp.wall)(chest, fwd, s.distance + 0.3).map(|d| d - 0.15);
            let d = wall.map_or(s.distance, |w| w.min(s.distance)).max(0.6);
            let center = Vec3::new(head.x, inp.floor + s.lift + s.height * 0.5, head.z) + fwd * d;
            // Facing the player: its normal back along where they look.
            let yaw = (-fwd).x.atan2((-fwd).z);
            tracing::info!("mirror: stands {d:.2} m in front{} ({:.2} x {:.2} m)", if wall.is_some_and(|w| w < s.distance) { ", before a wall" } else { "" }, s.width, s.height);
            match inp.map {
                Some(m) => Place { center: m.point(center), yaw: yaw + m.body_yaw, in_world: true },
                None => Place { center, yaw, in_world: false },
            }
        });
        // In tracking space now.
        let (center, yaw) = match (place.in_world, inp.map) {
            (true, Some(m)) => (m.inv_point(place.center), place.yaw - m.body_yaw),
            _ => (place.center, place.yaw),
        };
        // Left far behind (walked away from, or the player went through a door to another place,
        // where its place means nothing): put away.
        if center.distance(head) > FORGET_FARTHER {
            tracing::info!("mirror: left {:.0} m behind: put away", center.distance(head));
            (self.place, self.glass, self.called, self.summons) = (None, None, false, None);
            return;
        }
        let normal = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let up = Vec3::Y * (s.height * 0.5);
        // Its right as seen from in front: the viewer's right, looking along -normal.
        let right = normal.cross(Vec3::Y) * (s.width * 0.5);
        self.glass = Some(Glass { center, right, up });
    }

    /// The view of the mirror for the game this frame (`frame`): its glass, and the camera to
    /// render its world from, the head (`head`, between the eyes) reflected. (Not rendered while
    /// the head looks away from the glass, `look` its forward: the game's extra render is saved.)
    pub fn view(&mut self, frame: u64, head: Vec3, look: Vec3, render: bool) -> MirrorView {
        let Some(g) = self.glass else { return MirrorView { summons: self.summons_count, ..Default::default() } };
        let seen = g.corners().into_iter().chain([g.center]).any(|p| (p - head).normalize_or_zero().dot(look) > SEEN_WITHIN.cos());
        let render = render && seen;
        // (While it's being summoned the game renders its world in it already, so the glass has it
        // the moment it appears.)
        let mut v = MirrorView {
            flags: MIR_SHOWN,
            frame: frame as u32,
            summons: self.summons_count,
            center: g.center.extend(0.0).to_array(),
            right: g.right.extend(0.0).to_array(),
            up: g.up.extend(0.0).to_array(),
            ..Default::default()
        };
        if let (true, Some((camera, fov, near))) = (render, g.camera(head)) {
            v.flags |= MIR_RENDER;
            (v.camera, v.fov, v.near) = (camera, fov, near);
            self.asked[(frame % 32) as usize] = (frame, camera, fov);
        }
        v
    }

    /// The camera the game was asked to render its world from for the frame `aux_frame` (its low
    /// 32 bits), if it's still known.
    pub fn asked_for(&self, aux_frame: u32) -> Option<(Pose, Fov)> {
        self.asked.iter().find(|(f, ..)| *f as u32 == aux_frame && *f != 0).map(|&(_, p, f)| (p, f))
    }

    /// Draws eye `e`'s reflection (the eye at `eye`): the game's world in the mirror (`world`: its
    /// picture from the camera it was asked for, moved here by `warp`), then the player (`draw`:
    /// into a target from a camera, with the game's depth seen from there). False if the eye can't
    /// see the glass.
    pub fn render_eye(
        &mut self,
        gpu: &Gpu,
        e: usize,
        eye: &Pose,
        warp: &mut crate::warp::Warp,
        world: Option<crate::warp::WarpSource>,
        draw: &mut dyn FnMut(&Gpu, &EyeTarget, &Pose, &Fov, Option<&ID3D11ShaderResourceView>),
    ) -> Result<bool> {
        let Some(g) = self.glass.filter(|_| self.shown()) else { return Ok(false) };
        let Some((camera, fov, _)) = g.camera(Vec3::from_array(eye.pos)) else {
            if let Some(r) = self.eyes[e].as_mut() {
                r.drawn = false;
            }
            return Ok(false);
        };
        let (w, h) = (REFLECTION_WIDTH, ((REFLECTION_WIDTH as f32 * g.up.length() / g.right.length().max(1e-3)).round() as u32).clamp(64, 4096));
        if self.eyes[e].as_ref().is_none_or(|r| (r.target.width, r.target.height) != (w, h)) {
            let desc = TexDesc { width: w, height: h, format: DXGI_FORMAT_R8G8B8A8_TYPELESS, rtv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), srv: Some(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), dsv: None, mips: 1 };
            let tex = Texture::new(gpu, &desc)?;
            let target = EyeTarget { tex: tex.tex.clone(), rtv: tex.rtv.clone().unwrap(), width: w, height: h };
            self.eyes[e] = Some(Reflection { tex, target, view_proj: Mat4::IDENTITY, drawn: false });
        }
        let r = self.eyes[e].as_mut().unwrap();
        unsafe { gpu.ctx.ClearRenderTargetView(&r.target.rtv, &DARK) };
        let mut depth = None;
        if let Some(src) = world {
            warp.draw(gpu, &r.target, &camera, &fov, &[src])?;
            depth = Some(warp.post(gpu, w, h)?.depth);
        }
        draw(gpu, &r.target, &camera, &fov, depth.as_ref());
        r.view_proj = projection(&fov) * view_matrix(&camera);
        r.drawn = true;
        Ok(true)
    }

    fn pipeline(&mut self, gpu: &Gpu) -> Result<()> {
        if self.program.is_none() {
            self.program = Some(Program::new(gpu, SHADER, "mirror", &[])?);
            self.cb = Some(ConstantBuffer::new(gpu)?);
            self.sampler = Some(pipeline::sampler(gpu, true, false)?);
            self.blend = Some(pipeline::blend_state(gpu, Blend::Opaque)?);
            self.depth_off = Some(pipeline::depth_state(gpu, false, false)?);
            self.raster = Some(pipeline::raster_state(gpu, false, false)?);
        }
        Ok(())
    }

    /// Draws the glass into eye `e`'s target (the eye at `eye` with `fov`) with its reflection,
    /// hidden behind the game's world where that's nearer (`depth`: its view depth there, and the
    /// metres a unit of it is).
    pub fn draw_glass(&mut self, gpu: &Gpu, target: &EyeTarget, e: usize, eye: &Pose, fov: &Fov, depth: Option<(&ID3D11ShaderResourceView, f32)>) -> Result<()> {
        let Some(g) = self.glass.filter(|_| self.shown()) else { return Ok(()) };
        let Some(r) = self.eyes[e].as_ref().filter(|r| r.drawn) else { return Ok(()) };
        let reflect_vp = r.view_proj;
        let srv = r.tex.srv.clone();
        self.pipeline(gpu)?;
        let view = view_matrix(eye);
        // The view depth along the eye's forward (+Z in view space).
        let z = view.row(2);
        let (bw, bh) = (BORDER, BORDER * g.right.length() / g.up.length().max(1e-3));
        let cb = GlassCb {
            view_proj: (projection(fov) * view).to_cols_array(),
            reflect_vp: reflect_vp.to_cols_array(),
            corners: g.corners().map(|c| c.extend(1.0).to_array()),
            view_z: z.to_array(),
            depth: [depth.is_some() as u32 as f32, depth.map_or(1.0, |d| d.1), bw, bh],
            target: [target.width as f32, target.height as f32, 0.0, 0.0],
        };
        let ctx = &gpu.ctx;
        let cbuf = self.cb.as_ref().unwrap();
        cbuf.update(ctx, &cb);
        self.program.as_ref().unwrap().bind(ctx);
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
            pipeline::viewport(ctx, target.width, target.height);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(cbuf.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(cbuf.buf.clone())]));
            ctx.PSSetShaderResources(0, Some(&[srv, depth.map(|d| d.0.clone())]));
            ctx.PSSetSamplers(0, Some(&[self.sampler.clone()]));
            ctx.OMSetBlendState(self.blend.as_ref(), Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(self.depth_off.as_ref(), 0);
            ctx.RSSetState(self.raster.as_ref());
            ctx.Draw(6, 0);
            ctx.PSSetShaderResources(0, Some(&[None, None]));
        }
        Ok(())
    }
}

/// Where a menu shown beside the mirror stands (a character creator's): its edge at the glass's
/// left edge as the player faces it, a hand in front of the glass, 0.85 m tall (`aspect` wide for
/// each of that), at about the head's height and turned to face the head: a short turn of the head
/// from the mirror to the menu and back.
pub fn beside(g: &Glass, head: Vec3, aspect: f32) -> crate::panels::Panel {
    let height = 0.85;
    let width = height * aspect.max(0.1);
    let edge = g.center - g.right + g.normal() * 0.12;
    let to_edge = edge - head;
    let side = Quat::from_rotation_y(to_edge.x.atan2(to_edge.z)) * Vec3::X;
    let mut center = edge - side * (width * 0.5 + 0.04);
    center.y = head.y - 0.2;
    let to = center - head;
    crate::panels::Panel::facing(center, to.x.atan2(to.z), 0.0, width, aspect)
}

/// A direction flattened onto the floor (unit; forward when it points straight up or down).
fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z).try_normalize().unwrap_or(Vec3::Z)
}
