//! Draws each eye: the game's image (or the runtime's own empty room when there's no game picture),
//! then the runtime's layers on top: the scope lens, panels (HUD, menus), the laser pointer and
//! controller markers. Everything the runtime draws lives in tracking space and is drawn with the
//! eye pose the picture underneath is seen from (the game's own, or this headset frame's when the
//! game's picture has been moved to it, `crate::warp`), so the headset's reprojection treats it
//! all as one picture.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer};
use rustvr_gpu::shader::Program;
use rustvr_gpu::Gpu;
use rustvr_protocol::{Fov, Pose};
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP};
use windows::Win32::Graphics::Direct3D11::*;

use crate::backend::EyeTarget;
use crate::panels::Panel;

const SHADER: &str = r#"
cbuffer Draw : register(b0) {
    float4x4 view_proj;      // tracking space -> clip
    float4x4 inv_view_proj;
    float4 center;           // quad centre
    float4 axis_x;           // half width (towards u = 1)
    float4 axis_y;           // half height (towards v = 0)
    float4 color;            // premultiplied, linear
    float4 uv_rect;          // u0 v0 u1 v1
    float4 params;           // x mode, y reticle, z opacity
    float4x4 view_proj_src;  // the re-projected picture's eye
};
Texture2D tex : register(t0);
SamplerState samp : register(s0);

struct VOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float2 local : TEXCOORD1; };

VOut VSFull(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VOut o;
    o.pos = float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
    o.uv = uv;
    o.local = uv;
    return o;
}

float4 PSBlit(VOut i) : SV_Target { return float4(tex.Sample(samp, i.uv).rgb, 1); }

// Another camera's view of the game's picture: each pixel's direction looked up in one eye's image
// (`view_proj` here is that eye's), else in the other's (`eye_b`), else dark. A turn of the camera
// is exact; the few centimetres between it and the eye are left out (far things don't move).
cbuffer Reproject : register(b1) {
    float4x4 eye_b;
};
Texture2D tex_b : register(t1);
float3 eye_sample(Texture2D t, float4x4 m, float3 d, out bool ok) {
    float4 c = mul(m, float4(d, 0));
    float2 n = c.xy / max(c.w, 1e-6);
    ok = c.w > 0 && abs(n.x) <= 1 && abs(n.y) <= 1;
    return t.SampleLevel(samp, float2(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5), 0).rgb;
}
float4 PSReproject(VOut i) : SV_Target {
    float2 ndc = float2(i.uv.x * 2 - 1, 1 - i.uv.y * 2);
    float4 a = mul(inv_view_proj, float4(ndc, 0, 1)); a /= a.w;
    float4 b = mul(inv_view_proj, float4(ndc, 1, 1)); b /= b.w;
    float3 d = normalize(b.xyz - a.xyz);
    bool ok;
    float3 c = eye_sample(tex, view_proj_src, d, ok);
    if (ok) return float4(c, 1);
    c = eye_sample(tex_b, eye_b, d, ok);
    if (ok) return float4(c, 1);
    return float4(0.004, 0.004, 0.005, 1);
}

// The runtime's own room: a floor grid at the real floor with a ring around the play-space centre.
float4 PSVoid(VOut i) : SV_Target {
    float2 ndc = float2(i.uv.x * 2 - 1, 1 - i.uv.y * 2);
    float4 a = mul(inv_view_proj, float4(ndc, 0, 1)); a /= a.w;
    float4 b = mul(inv_view_proj, float4(ndc, 1, 1)); b /= b.w;
    float3 d = normalize(b.xyz - a.xyz);
    float3 sky = lerp(float3(0.010, 0.012, 0.018), float3(0.05, 0.06, 0.09), saturate(d.y * 2 + 0.3));
    if (d.y > -1e-3) return float4(sky, 1);
    float t = -a.y / d.y;
    float3 p = a.xyz + d * t;
    float2 g = abs(frac(p.xz + 0.5) - 0.5) / max(fwidth(p.xz), 1e-4);
    float grid = 1 - saturate(min(g.x, g.y));
    float fade = saturate(1 - t / 25);
    float r = length(p.xz);
    float ring = 1 - saturate(abs(r - 1.0) / max(fwidth(r), 1e-4) * 0.5);
    float3 floorc = float3(0.012, 0.014, 0.018) + grid * fade * float3(0.05, 0.16, 0.22) + ring * fade * float3(0.30, 0.20, 0.05);
    return float4(lerp(sky, floorc, fade * 0.9 + 0.1), 1);
}

// Comfort: a vignette that closes in around the centre of view (the head's forward, `center`), and a
// fade to black. color = (strength, aperture, feather, fade). Angles, not screen positions, so
// both eyes darken the same way.
float4 PSComfort(VOut i) : SV_Target {
    float2 ndc = float2(i.uv.x * 2 - 1, 1 - i.uv.y * 2);
    float4 a = mul(inv_view_proj, float4(ndc, 0, 1)); a /= a.w;
    float4 b = mul(inv_view_proj, float4(ndc, 1, 1)); b /= b.w;
    float3 d = normalize(b.xyz - a.xyz);
    float angle = acos(clamp(dot(d, normalize(center.xyz)), -1, 1));
    float r = angle / radians(55.0);
    float v = smoothstep(color.y - color.z, color.y + color.z, r) * color.x;
    float dark = 1 - (1 - v) * (1 - color.w);
    return float4(0, 0, 0, saturate(dark));
}

VOut VSQuad(uint id : SV_VertexID) {
    float2 c = float2(id & 1, id >> 1);
    float2 s = c * 2 - 1;
    float3 p = center.xyz + axis_x.xyz * s.x - axis_y.xyz * s.y;
    VOut o;
    o.pos = mul(view_proj, float4(p, 1));
    o.uv = lerp(uv_rect.xy, uv_rect.zw, c);
    o.local = s;
    return o;
}

float4 PSQuad(VOut i) : SV_Target {
    int mode = (int)params.x;
    if (mode == 1) return color;
    if (mode == 2) {
        float r = length(i.local);
        if (r > 1) discard;
        return color * saturate((1 - r) * 3);
    }
    if (mode == 3) {
        float r = length(i.local);
        if (r > 1) discard;
        float3 c = tex.Sample(samp, i.uv).rgb;
        if (params.y > 0) {
            float w = 0.008;
            bool crossline = (abs(i.local.x) < w || abs(i.local.y) < w) && r > 0.06;
            bool centre_dot = params.y > 1 && r < 0.015;
            if (crossline || centre_dot) c = float3(0.01, 0.01, 0.01);
        }
        float vignette = saturate((1 - r) * 12);
        return float4(c * vignette, 1);
    }
    return tex.Sample(samp, i.uv) * params.z;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawCb {
    view_proj: [f32; 16],
    inv_view_proj: [f32; 16],
    center: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    color: [f32; 4],
    uv_rect: [f32; 4],
    params: [f32; 4],
    view_proj_src: [f32; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ReprojectCb {
    eye_b: [f32; 16],
}

/// What's behind everything in a view.
pub enum Background<'a> {
    /// The game's picture for this eye, as it was rendered.
    Game(&'a ID3D11ShaderResourceView),
    /// The runtime's own room.
    Void,
    /// Another camera's view of the game's picture: from `first` (its eye's pose and field of
    /// view), else from `second`, where `first` doesn't reach.
    Reprojected { first: (&'a ID3D11ShaderResourceView, Pose, Fov), second: Option<(&'a ID3D11ShaderResourceView, Pose, Fov)> },
    /// What's in the target already (the game's picture moved there, `crate::warp`).
    Keep,
}

const MODE_TEXTURE: f32 = 0.0;
const MODE_SOLID: f32 = 1.0;
const MODE_DOT: f32 = 2.0;
const MODE_LENS: f32 = 3.0;

/// One textured panel to draw.
pub struct PanelLayer<'a> {
    pub panel: Panel,
    pub srv: &'a ID3D11ShaderResourceView,
    pub opacity: f32,
    /// The part of the texture shown: u0, v0, u1, v1.
    pub uv: [f32; 4],
}

impl<'a> PanelLayer<'a> {
    /// The whole texture on `panel`.
    pub fn whole(panel: Panel, srv: &'a ID3D11ShaderResourceView) -> Self {
        PanelLayer { panel, srv, opacity: 1.0, uv: [0.0, 0.0, 1.0, 1.0] }
    }
}

pub struct LensLayer<'a> {
    pub pos: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub radius: f32,
    pub srv: &'a ID3D11ShaderResourceView,
    /// Width / height of the aux image (the lens shows its centre square).
    pub aspect: f32,
    pub reticle: u32,
}

pub struct Laser {
    pub origin: Vec3,
    pub end: Vec3,
    /// It hits a panel (draw the dot).
    pub hit: bool,
}

#[derive(Default)]
pub struct Layers<'a> {
    pub lens: Option<LensLayer<'a>>,
    pub panels: Vec<PanelLayer<'a>>,
    pub laser: Option<Laser>,
    /// Controller grip poses to mark (when the game isn't drawing hands).
    pub markers: Vec<(Vec3, Quat)>,
    pub comfort: Comfort,
}

/// The comfort overlay for this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Comfort {
    /// 0 = no vignette.
    pub vignette: f32,
    /// Clear radius at full strength, as a fraction of 55 degrees from the centre of view.
    pub aperture: f32,
    pub feather: f32,
    /// Head forward (tracking space): the vignette's centre.
    pub forward: Vec3,
    /// 0..1 black.
    pub fade: f32,
}

pub struct Compositor {
    full_blit: Program,
    full_void: Program,
    full_comfort: Program,
    full_reproject: Program,
    reproject_cb: ConstantBuffer<ReprojectCb>,
    quad: Program,
    opaque: ID3D11BlendState,
    premul: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    no_depth: ID3D11DepthStencilState,
    sampler: ID3D11SamplerState,
    cb: ConstantBuffer<DrawCb>,
}

pub const NEAR: f32 = 0.03;
pub const FAR: f32 = 200.0;

impl Compositor {
    pub fn new(gpu: &Gpu) -> Result<Compositor> {
        Ok(Compositor {
            full_blit: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSBlit", &[])?,
            full_void: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSVoid", &[])?,
            full_comfort: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSComfort", &[])?,
            full_reproject: Program::with_entries(gpu, SHADER, "compositor", "VSFull", "PSReproject", &[])?,
            reproject_cb: ConstantBuffer::new(gpu)?,
            quad: Program::with_entries(gpu, SHADER, "compositor", "VSQuad", "PSQuad", &[])?,
            opaque: pipeline::blend_state(gpu, Blend::Opaque)?,
            premul: pipeline::blend_state(gpu, Blend::Premultiplied)?,
            raster: pipeline::raster_state(gpu, false, false)?,
            no_depth: pipeline::depth_state(gpu, false, false)?,
            sampler: pipeline::sampler(gpu, true, false)?,
            cb: ConstantBuffer::new(gpu)?,
        })
    }

    fn bind(&self, gpu: &Gpu, target: &EyeTarget) {
        let ctx = &gpu.ctx;
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
            ctx.RSSetState(&self.raster);
            ctx.OMSetDepthStencilState(&self.no_depth, 0);
            ctx.OMSetBlendState(&self.opaque, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.buf.clone())]));
            ctx.IASetInputLayout(None);
        }
        pipeline::viewport(ctx, target.width, target.height);
    }

    /// Draws one eye. `game` is the game's eye image (sRGB view), or None for the runtime's room.
    /// `between` runs after the picture and before the overlays (the player's body goes there),
    /// with the eye's target bound; it may change any pipeline state.
    pub fn draw_eye(
        &self,
        gpu: &Gpu,
        target: &EyeTarget,
        eye: &Pose,
        fov: &Fov,
        game: Option<&ID3D11ShaderResourceView>,
        layers: &Layers,
        between: &mut dyn FnMut(&Gpu),
    ) {
        let background = game.map_or(Background::Void, Background::Game);
        self.draw_view(gpu, target, eye, fov, background, layers, true, between);
    }

    /// [`Compositor::draw_eye`] for any camera, with any [`Background`], and with or without the
    /// comfort vignette and fade (`comfort`: a desktop view has none of the headset's).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_view(
        &self,
        gpu: &Gpu,
        target: &EyeTarget,
        eye: &Pose,
        fov: &Fov,
        background: Background,
        layers: &Layers,
        comfort: bool,
        between: &mut dyn FnMut(&Gpu),
    ) {
        let ctx = &gpu.ctx;
        let view_proj = projection(fov) * view_matrix(eye);
        let mut cb = DrawCb::zeroed();
        cb.view_proj = view_proj.to_cols_array();
        cb.inv_view_proj = view_proj.inverse().to_cols_array();
        cb.uv_rect = [0.0, 0.0, 1.0, 1.0];
        self.bind(gpu, target);
        let keep = matches!(background, Background::Keep);
        unsafe {
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            match background {
                Background::Keep => {}
                Background::Game(srv) => {
                    self.cb.update(ctx, &cb);
                    self.full_blit.bind(ctx);
                    ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
                }
                Background::Void => {
                    self.cb.update(ctx, &cb);
                    self.full_void.bind(ctx);
                    ctx.PSSetShaderResources(0, Some(&[None]));
                }
                Background::Reprojected { first, second } => {
                    let mut c = cb;
                    c.view_proj_src = (projection(&first.2) * view_matrix(&first.1)).to_cols_array();
                    self.cb.update(ctx, &c);
                    let (b_srv, b_vp) = match &second {
                        Some((srv, pose, fov)) => (Some((*srv).clone()), projection(fov) * view_matrix(pose)),
                        // Nothing to fall back on: a matrix that puts everything behind it.
                        None => (None, Mat4::ZERO),
                    };
                    self.reproject_cb.update(ctx, &ReprojectCb { eye_b: b_vp.to_cols_array() });
                    ctx.PSSetConstantBuffers(1, Some(&[Some(self.reproject_cb.buf.clone())]));
                    self.full_reproject.bind(ctx);
                    ctx.PSSetShaderResources(0, Some(&[Some(first.0.clone()), b_srv]));
                }
            }
            if !keep {
                ctx.Draw(3, 0);
            }
            ctx.PSSetShaderResources(0, Some(&[None, None]));
        }
        self.cb.update(ctx, &cb);
        between(gpu);
        self.bind(gpu, target);
        unsafe {
            ctx.OMSetBlendState(&self.premul, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        }
        self.quad.bind(ctx);
        let eye_pos = Vec3::from_array(eye.pos);
        let eye_rot = quat(eye.rot);

        if let Some(l) = &layers.lens {
            let (u0, u1) = if l.aspect > 1.0 { (0.5 - 0.5 / l.aspect, 0.5 + 0.5 / l.aspect) } else { (0.0, 1.0) };
            let (v0, v1) = if l.aspect < 1.0 { (0.5 - 0.5 * l.aspect, 0.5 + 0.5 * l.aspect) } else { (0.0, 1.0) };
            let mut c = cb;
            c.center = l.pos.extend(1.0).to_array();
            c.axis_x = (l.right * l.radius).extend(0.0).to_array();
            c.axis_y = (l.up * l.radius).extend(0.0).to_array();
            c.uv_rect = [u0, v0, u1, v1];
            c.params = [MODE_LENS, l.reticle as f32, 1.0, 0.0];
            self.draw_quad(gpu, &c, Some(l.srv));
        }
        for p in &layers.panels {
            let mut c = cb;
            c.center = p.panel.center.extend(1.0).to_array();
            c.axis_x = (p.panel.right * p.panel.width * 0.5).extend(0.0).to_array();
            c.axis_y = (p.panel.up * p.panel.height * 0.5).extend(0.0).to_array();
            c.params = [MODE_TEXTURE, 0.0, p.opacity, 0.0];
            c.uv_rect = p.uv;
            self.draw_quad(gpu, &c, Some(p.srv));
        }
        for (pos, rot) in &layers.markers {
            self.draw_ribbon(gpu, &cb, eye_pos, *pos, *pos + *rot * Vec3::Z * 0.08, 0.012, Vec4::new(0.35, 0.55, 0.65, 0.9));
            self.draw_dot(gpu, &cb, eye_rot, *pos, 0.022, Vec4::new(0.5, 0.7, 0.8, 0.95));
        }
        if let Some(l) = &layers.laser {
            let color = if l.hit { Vec4::new(0.25, 0.75, 1.0, 0.7) } else { Vec4::new(0.25, 0.5, 0.7, 0.35) };
            self.draw_ribbon(gpu, &cb, eye_pos, l.origin, l.end, 0.003, color);
            if l.hit {
                self.draw_dot(gpu, &cb, eye_rot, l.end, 0.012, Vec4::new(0.6, 0.9, 1.0, 1.0));
            }
        }
        let k = &layers.comfort;
        if comfort && (k.vignette > 0.003 || k.fade > 0.003) {
            let mut c = cb;
            c.center = k.forward.normalize_or(Vec3::Z).extend(0.0).to_array();
            c.color = [k.vignette.clamp(0.0, 1.0), k.aperture, k.feather.max(0.01), k.fade.clamp(0.0, 1.0)];
            self.cb.update(ctx, &c);
            unsafe {
                ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                self.full_comfort.bind(ctx);
                ctx.PSSetShaderResources(0, Some(&[None]));
                ctx.Draw(3, 0);
            }
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
    }

    fn draw_quad(&self, gpu: &Gpu, cb: &DrawCb, srv: Option<&ID3D11ShaderResourceView>) {
        self.cb.update(&gpu.ctx, cb);
        unsafe {
            gpu.ctx.PSSetShaderResources(0, Some(&[srv.cloned()]));
            gpu.ctx.Draw(4, 0);
        }
    }

    /// A camera-facing strip from `a` to `b`.
    fn draw_ribbon(&self, gpu: &Gpu, base: &DrawCb, eye: Vec3, a: Vec3, b: Vec3, width: f32, color: Vec4) {
        let mid = (a + b) * 0.5;
        let along = (b - a) * 0.5;
        if along.length_squared() < 1e-10 {
            return;
        }
        let side = along.cross(eye - mid).normalize_or_zero() * width * 0.5;
        let mut c = *base;
        c.center = mid.extend(1.0).to_array();
        c.axis_x = side.extend(0.0).to_array();
        c.axis_y = along.extend(0.0).to_array();
        c.color = color.to_array();
        c.params = [MODE_SOLID, 0.0, 1.0, 0.0];
        self.draw_quad(gpu, &c, None);
    }

    fn draw_dot(&self, gpu: &Gpu, base: &DrawCb, eye_rot: Quat, at: Vec3, radius: f32, color: Vec4) {
        let mut c = *base;
        c.center = at.extend(1.0).to_array();
        c.axis_x = (eye_rot * Vec3::X * radius).extend(0.0).to_array();
        c.axis_y = (eye_rot * Vec3::Y * radius).extend(0.0).to_array();
        c.color = color.to_array();
        c.params = [MODE_DOT, 0.0, 1.0, 0.0];
        self.draw_quad(gpu, &c, None);
    }
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

pub fn view_matrix(eye: &Pose) -> Mat4 {
    Mat4::from_rotation_translation(quat(eye.rot), Vec3::from_array(eye.pos)).inverse()
}

/// Left-handed off-centre projection with D3D depth, from FOV tangents.
pub fn projection(fov: &Fov) -> Mat4 {
    let w = fov.right - fov.left;
    let h = fov.up - fov.down;
    let range = FAR / (FAR - NEAR);
    Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new(-(fov.left + fov.right) / w, -(fov.up + fov.down) / h, range, 1.0),
        Vec4::new(0.0, 0.0, -range * NEAR, 0.0),
    )
}

/// Screen UV (0..1, top-left origin) of a tracking-space point seen from an eye, if in front.
pub fn project(eye: &Pose, fov: &Fov, p: Vec3) -> Option<Vec2> {
    let c = projection(fov) * view_matrix(eye) * p.extend(1.0);
    if c.w <= 0.0 {
        return None;
    }
    Some(Vec2::new(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5))
}
