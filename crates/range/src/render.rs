//! A small forward renderer for the range: lit, textured meshes into an eye (or scope) target, then
//! the effects (sprites: flashes, sparks, smoke, tracers, bullet holes) over them, and up to four
//! point lights (a muzzle flash, an explosion) on the meshes.
//!
//! The adapter views the shared eye textures as UNORM, like a game's back buffer, so the pixel
//! shaders write gamma-encoded colour; the runtime reads them back as sRGB. Effects test the depth
//! but write neither it nor the linear depth (the runtime's body isn't hidden behind smoke).

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec2, Vec3};
use rustvr_asset as asset;
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer, static_buffer};
use rustvr_gpu::shader::{Program, element};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use std::collections::HashMap;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::content::{MeshData, Vertex};

pub const MAX_LIGHTS: usize = 4;

const SHADER: &str = r#"
cbuffer Frame : register(b0) { float4x4 view_proj; float4 eye_pos; float4 sun_dir; float4 fog; float4 view_fwd; float4 light_pos[4]; float4 light_color[4]; };
cbuffer Object : register(b1) { float4x4 model; float4 tint; float4 params; };
Texture2D tex : register(t0);
SamplerState samp : register(s0);
struct VSIn { float3 pos : POSITION; float3 normal : NORMAL; float2 uv : TEXCOORD0; float4 color : COLOR0; };
struct VSOut { float4 pos : SV_Position; float3 world : TEXCOORD1; float3 normal : NORMAL; float2 uv : TEXCOORD0; float4 color : COLOR0; };
VSOut VSMain(VSIn i) {
    VSOut o;
    float4 w = mul(model, float4(i.pos, 1));
    o.world = w.xyz;
    o.pos = mul(view_proj, w);
    o.normal = mul((float3x3)model, i.normal);
    o.uv = i.uv;
    o.color = i.color;
    return o;
}
struct PSOut { float4 color : SV_Target0; float depth : SV_Target1; };
PSOut PSMain(VSOut i, bool front : SV_IsFrontFace) {
    float3 base = pow(i.color.rgb, 2.2) * tint.rgb;   // vertex colours are sRGB
    if (params.x > 0.5) base *= tex.Sample(samp, i.uv).rgb;
    float3 n = normalize(i.normal) * (front ? 1 : -1);
    float diffuse = saturate(dot(n, -sun_dir.xyz));
    float hemi = 0.5 + 0.5 * n.y;
    float3 c = base * (0.18 + 0.22 * hemi + 0.75 * diffuse) + base * params.y;
    // Point lights (flashes): mostly facing, a little all round.
    for (int k = 0; k < 4; k++) {
        float3 d = light_pos[k].xyz - i.world;
        float att = saturate(1 - length(d) / max(light_pos[k].w, 1e-3));
        c += base * light_color[k].rgb * att * att * (0.25 + 0.75 * saturate(dot(n, normalize(d))));
    }
    float dist = distance(i.world, eye_pos.xyz);
    c = lerp(c, fog.rgb, saturate(dist * fog.w));
    PSOut o;
    o.color = float4(pow(saturate(c), 1.0 / 2.2), 1);
    // Linear depth along the view axis, for the runtime to hide the player's body behind the world.
    o.depth = dot(i.world - eye_pos.xyz, view_fwd.xyz);
    return o;
}
"#;

/// Sprites: a quad (`model` places it) showing `uv` of the texture, tinted; additive or
/// alpha-blended (`params.x` 1 for alpha), fading into the fog.
const FX_SHADER: &str = r#"
cbuffer Frame : register(b0) { float4x4 view_proj; float4 eye_pos; float4 sun_dir; float4 fog; float4 view_fwd; float4 light_pos[4]; float4 light_color[4]; };
cbuffer Fx : register(b1) { float4x4 model; float4 color; float4 uv_rect; float4 params; };
Texture2D tex : register(t0);
SamplerState samp : register(s0);
struct VSIn { float3 pos : POSITION; float3 normal : NORMAL; float2 uv : TEXCOORD0; float4 vcolor : COLOR0; };
struct VSOut { float4 pos : SV_Position; float3 world : TEXCOORD1; float2 uv : TEXCOORD0; };
VSOut VSMain(VSIn i) {
    VSOut o;
    float4 w = mul(model, float4(i.pos, 1));
    o.world = w.xyz;
    o.pos = mul(view_proj, w);
    o.uv = lerp(uv_rect.xy, uv_rect.zw, i.uv);
    return o;
}
float4 PSMain(VSOut i) : SV_Target0 {
    float4 t = tex.Sample(samp, i.uv);
    float clear = 1 - saturate(distance(i.world, eye_pos.xyz) * fog.w);
    float3 c = t.rgb * color.rgb;
    if (params.x > 0.5) return float4(pow(saturate(c), 1.0 / 2.2), t.a * color.a * clear);
    return float4(pow(saturate(c * color.a * clear), 1.0 / 2.2), 0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameCb {
    view_proj: [f32; 16],
    eye_pos: [f32; 4],
    sun_dir: [f32; 4],
    fog: [f32; 4],
    view_fwd: [f32; 4],
    light_pos: [[f32; 4]; MAX_LIGHTS],
    light_color: [[f32; 4]; MAX_LIGHTS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjectCb {
    model: [f32; 16],
    tint: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FxCb {
    model: [f32; 16],
    color: [f32; 4],
    uv: [f32; 4],
    params: [f32; 4],
}

pub struct Mesh {
    vb: ID3D11Buffer,
    ib: ID3D11Buffer,
    count: u32,
    texture: Option<String>,
}

pub struct Renderer {
    program: Program,
    fx_program: Program,
    frame_cb: ConstantBuffer<FrameCb>,
    object_cb: ConstantBuffer<ObjectCb>,
    fx_cb: ConstantBuffer<FxCb>,
    raster: ID3D11RasterizerState,
    raster_two_sided: ID3D11RasterizerState,
    depth_state: ID3D11DepthStencilState,
    fx_depth: ID3D11DepthStencilState,
    blend: ID3D11BlendState,
    fx_add: ID3D11BlendState,
    fx_alpha: ID3D11BlendState,
    sampler: ID3D11SamplerState,
    fx_sampler: ID3D11SamplerState,
    quad: Mesh,
    textures: HashMap<String, Texture>,
    /// Effect sprites by id ([`Renderer::fx_texture`]).
    fx_textures: Vec<String>,
    depth: HashMap<(u32, u32), Texture>,
    asset_dir: std::path::PathBuf,
}

/// One thing to draw this frame.
pub struct Draw<'a> {
    pub mesh: &'a Mesh,
    pub model: Mat4,
    pub tint: [f32; 4],
    pub emissive: f32,
    /// Draw both sides (models from games, whose winding we don't control).
    pub two_sided: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxBlend {
    /// Light: flashes, sparks, fire, tracers.
    Add,
    /// Smoke, dust, holes.
    Alpha,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FxShape {
    /// Faces the eye, turned `roll` radians.
    Billboard { roll: f32 },
    /// Its width along this world axis, turned about it to face the eye (tracers, a muzzle's
    /// side flash).
    Axial(Vec3),
    /// Fixed: +X and +Y span it (decals: +Z out of the surface).
    Fixed(Quat),
}

/// A sprite: texture `tex` ([`Renderer::fx_texture`]), `size` metres wide and high, the `uv`
/// rectangle (u0, v0, u1, v1) of the texture, a linear colour (alpha fades it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxDraw {
    pub tex: usize,
    pub pos: Vec3,
    pub size: Vec2,
    pub shape: FxShape,
    pub color: [f32; 4],
    pub uv: [f32; 4],
    pub blend: FxBlend,
}

/// A point light: linear colour times intensity, falling to nothing at `radius`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub pos: Vec3,
    pub color: Vec3,
    pub radius: f32,
}

fn fx_blend_state(gpu: &Gpu, additive: bool) -> Result<ID3D11BlendState> {
    let colour = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: if additive { D3D11_BLEND_ONE } else { D3D11_BLEND_SRC_ALPHA },
        DestBlend: if additive { D3D11_BLEND_ONE } else { D3D11_BLEND_INV_SRC_ALPHA },
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ZERO,
        DestBlendAlpha: D3D11_BLEND_ONE,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    // The linear depth target keeps the world's.
    let keep = D3D11_RENDER_TARGET_BLEND_DESC { RenderTargetWriteMask: 0, ..colour };
    let mut rts = [keep; 8];
    rts[0] = colour;
    let desc = D3D11_BLEND_DESC { AlphaToCoverageEnable: false.into(), IndependentBlendEnable: true.into(), RenderTarget: rts };
    let mut s = None;
    unsafe { gpu.device.CreateBlendState(&desc, Some(&mut s))? };
    Ok(s.unwrap())
}

impl Renderer {
    pub fn new(gpu: &Gpu, asset_dir: std::path::PathBuf) -> Result<Renderer> {
        let layout = [
            element(b"POSITION\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0),
            element(b"NORMAL\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 12),
            element(b"TEXCOORD\0", 0, DXGI_FORMAT_R32G32_FLOAT, 24),
            element(b"COLOR\0", 0, DXGI_FORMAT_R8G8B8A8_UNORM, 32),
        ];
        let mut quad = MeshData::default();
        quad.add_plate(Vec3::ZERO, 0.5, 0.5, [255; 4]);
        let mut r = Renderer {
            program: Program::new(gpu, SHADER, "range", &layout)?,
            fx_program: Program::new(gpu, FX_SHADER, "range_fx", &layout)?,
            frame_cb: ConstantBuffer::new(gpu)?,
            object_cb: ConstantBuffer::new(gpu)?,
            fx_cb: ConstantBuffer::new(gpu)?,
            raster: pipeline::raster_state(gpu, true, false)?,
            raster_two_sided: pipeline::raster_state(gpu, false, false)?,
            depth_state: pipeline::depth_state(gpu, true, true)?,
            fx_depth: pipeline::depth_state(gpu, true, false)?,
            blend: pipeline::blend_state(gpu, Blend::Opaque)?,
            fx_add: fx_blend_state(gpu, true)?,
            fx_alpha: fx_blend_state(gpu, false)?,
            sampler: pipeline::sampler(gpu, true, true)?,
            fx_sampler: pipeline::sampler(gpu, true, false)?,
            quad: Mesh {
                vb: static_buffer(gpu, bytemuck::cast_slice(&quad.vertices), D3D11_BIND_VERTEX_BUFFER)?,
                ib: static_buffer(gpu, bytemuck::cast_slice(&quad.indices), D3D11_BIND_INDEX_BUFFER)?,
                count: quad.indices.len() as u32,
                texture: None,
            },
            textures: HashMap::new(),
            fx_textures: Vec::new(),
            depth: HashMap::new(),
            asset_dir,
        };
        r.textures.insert("grid".into(), builtin_texture(gpu, grid_pixels)?);
        r.textures.insert("target".into(), builtin_texture(gpu, target_pixels)?);
        for (name, f) in BUILTIN_FX {
            r.textures.insert(name.to_string(), builtin_texture(gpu, f)?);
        }
        Ok(r)
    }

    pub fn upload(&mut self, gpu: &Gpu, m: &MeshData) -> Result<Mesh> {
        if let Some(t) = &m.texture {
            self.ensure_texture(gpu, t);
        }
        Ok(Mesh {
            vb: static_buffer(gpu, bytemuck::cast_slice(&m.vertices), D3D11_BIND_VERTEX_BUFFER)?,
            ib: static_buffer(gpu, bytemuck::cast_slice(&m.indices), D3D11_BIND_INDEX_BUFFER)?,
            count: m.indices.len() as u32,
            texture: m.texture.clone().filter(|t| self.textures.contains_key(t)),
        })
    }

    /// A mesh whose vertices change every frame (a figure skinned on the CPU, `crate::figure`):
    /// [`Renderer::update`] gives it new ones, as many as it was made with.
    pub fn upload_dynamic(&mut self, gpu: &Gpu, m: &MeshData) -> Result<Mesh> {
        if let Some(t) = &m.texture {
            self.ensure_texture(gpu, t);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&m.vertices);
        let desc = D3D11_BUFFER_DESC { ByteWidth: bytes.len() as u32, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32, ..Default::default() };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: bytes.as_ptr().cast(), SysMemPitch: 0, SysMemSlicePitch: 0 };
        let mut vb = None;
        unsafe { gpu.device.CreateBuffer(&desc, Some(&init), Some(&mut vb))? };
        Ok(Mesh {
            vb: vb.unwrap(),
            ib: static_buffer(gpu, bytemuck::cast_slice(&m.indices), D3D11_BIND_INDEX_BUFFER)?,
            count: m.indices.len() as u32,
            texture: m.texture.clone().filter(|t| self.textures.contains_key(t)),
        })
    }

    /// New vertices for a mesh made by [`Renderer::upload_dynamic`] (exactly as many).
    pub fn update(&self, gpu: &Gpu, mesh: &Mesh, vertices: &[Vertex]) {
        unsafe { gpu.ctx.UpdateSubresource(&mesh.vb, 0, None, vertices.as_ptr().cast(), 0, 0) };
    }

    /// Loads a game texture from the asset cache (once): Metro's by its key, another game's as
    /// `<game>/<key>`. A missing one leaves the mesh untextured.
    fn ensure_texture(&mut self, gpu: &Gpu, key: &str) {
        if self.textures.contains_key(key) {
            return;
        }
        let (game, stem) = key.split_once('/').unwrap_or(("metro2033", key));
        let tex = asset::load_texture(&self.asset_dir.join(game), stem).and_then(|t| {
            let format = match t.format {
                asset::TexFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                asset::TexFormat::Bc1 => DXGI_FORMAT_BC1_UNORM_SRGB,
                asset::TexFormat::Bc2 => DXGI_FORMAT_BC2_UNORM_SRGB,
                asset::TexFormat::Bc3 => DXGI_FORMAT_BC3_UNORM_SRGB,
                asset::TexFormat::Bc4 => DXGI_FORMAT_BC4_UNORM,
                asset::TexFormat::Bc5 => DXGI_FORMAT_BC5_UNORM,
                asset::TexFormat::Bc7 => DXGI_FORMAT_BC7_UNORM_SRGB,
            };
            let mips: Vec<&[u8]> = t.mips.iter().map(|m| m.as_slice()).collect();
            Texture::upload(gpu, t.width, t.height, format, &mips)
        });
        match tex {
            Ok(t) => {
                self.textures.insert(key.to_owned(), t);
            }
            Err(e) => tracing::debug!("range: texture {key}: {e:#}"),
        }
    }

    /// An id for an effect sprite: the game's texture `key` from the asset cache if it's there,
    /// else the built-in `fallback` (one of `fx_*`).
    pub fn fx_texture(&mut self, gpu: &Gpu, key: &str, fallback: &str) -> (usize, bool) {
        self.ensure_texture(gpu, key);
        let (name, from_game) = if self.textures.contains_key(key) { (key, true) } else { (fallback, false) };
        let id = match self.fx_textures.iter().position(|t| t == name) {
            Some(i) => i,
            None => {
                self.fx_textures.push(name.to_string());
                self.fx_textures.len() - 1
            }
        };
        (id, from_game)
    }

    /// Renders `draws` into `target` from a camera (`view_proj`, eye position and forward in the
    /// world). With `linear_depth`, also writes each pixel's distance along the view axis there
    /// (0 where nothing was drawn).
    #[allow(dead_code)] // (the tests' renders)
    pub fn render(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        linear_depth: Option<&ID3D11RenderTargetView>,
        w: u32,
        h: u32,
        view_proj: Mat4,
        eye: Vec3,
        fwd: Vec3,
        draws: &[Draw],
    ) -> Result<()> {
        self.render_scene(gpu, target, linear_depth, w, h, view_proj, eye, fwd, draws, &[], &[])
    }

    /// [`Renderer::render`], then the effects over it, with `lights` (the first [`MAX_LIGHTS`])
    /// on the meshes.
    pub fn render_scene(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        linear_depth: Option<&ID3D11RenderTargetView>,
        w: u32,
        h: u32,
        view_proj: Mat4,
        eye: Vec3,
        fwd: Vec3,
        draws: &[Draw],
        fx: &[FxDraw],
        lights: &[Light],
    ) -> Result<()> {
        if let std::collections::hash_map::Entry::Vacant(e) = self.depth.entry((w, h)) {
            e.insert(Texture::new(gpu, &TexDesc::depth(w, h))?);
        }
        let depth = self.depth.get(&(w, h)).unwrap().dsv.clone().unwrap();
        let ctx = &gpu.ctx;
        let fog = [0.20f32, 0.22, 0.25];
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone()), linear_depth.cloned()]), &depth);
            let clear = [fog[0].powf(1.0 / 2.2), fog[1].powf(1.0 / 2.2), fog[2].powf(1.0 / 2.2), 1.0];
            ctx.ClearRenderTargetView(target, &clear);
            if let Some(d) = linear_depth {
                ctx.ClearRenderTargetView(d, &[0.0; 4]);
            }
            ctx.ClearDepthStencilView(&depth, D3D11_CLEAR_DEPTH.0, 1.0, 0);
            ctx.OMSetDepthStencilState(&self.depth_state, 0);
            ctx.OMSetBlendState(&self.blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone()), Some(self.object_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone()), Some(self.object_cb.buf.clone())]));
        }
        pipeline::viewport(ctx, w, h);
        self.program.bind(ctx);
        let sun = Vec3::new(-0.4, -1.0, 0.6).normalize();
        let mut frame = FrameCb {
            view_proj: view_proj.to_cols_array(),
            eye_pos: eye.extend(1.0).to_array(),
            sun_dir: sun.extend(0.0).to_array(),
            fog: [fog[0], fog[1], fog[2], 0.012],
            view_fwd: fwd.normalize_or(Vec3::Z).extend(0.0).to_array(),
            light_pos: [[0.0; 4]; MAX_LIGHTS],
            light_color: [[0.0; 4]; MAX_LIGHTS],
        };
        for (k, l) in lights.iter().take(MAX_LIGHTS).enumerate() {
            frame.light_pos[k] = l.pos.extend(l.radius).to_array();
            frame.light_color[k] = l.color.extend(0.0).to_array();
        }
        self.frame_cb.update(ctx, &frame);
        for d in draws {
            let tex = d.mesh.texture.as_ref().and_then(|t| self.textures.get(t));
            self.object_cb.update(
                ctx,
                &ObjectCb { model: d.model.to_cols_array(), tint: d.tint, params: [tex.is_some() as u32 as f32, d.emissive, 0.0, 0.0] },
            );
            unsafe {
                ctx.RSSetState(if d.two_sided { &self.raster_two_sided } else { &self.raster });
                ctx.PSSetShaderResources(0, Some(&[tex.and_then(|t| t.srv.clone())]));
                let stride = size_of::<Vertex>() as u32;
                let offset = 0u32;
                ctx.IASetVertexBuffers(0, 1, Some(&Some(d.mesh.vb.clone())), Some(&stride), Some(&offset));
                ctx.IASetIndexBuffer(&d.mesh.ib, DXGI_FORMAT_R32_UINT, 0);
                ctx.DrawIndexed(d.mesh.count, 0, 0);
            }
        }
        if !fx.is_empty() {
            self.draw_fx(gpu, eye, fx);
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// The effects: smoke and holes back to front, then the light (additive) ones.
    fn draw_fx(&mut self, gpu: &Gpu, eye: Vec3, fx: &[FxDraw]) {
        let ctx = &gpu.ctx;
        let mut order: Vec<&FxDraw> = fx.iter().collect();
        order.sort_by(|a, b| {
            let key = |d: &FxDraw| (d.blend == FxBlend::Add, -d.pos.distance_squared(eye));
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
        });
        self.fx_program.bind(ctx);
        unsafe {
            ctx.OMSetDepthStencilState(&self.fx_depth, 0);
            ctx.RSSetState(&self.raster_two_sided);
            ctx.PSSetSamplers(0, Some(&[Some(self.fx_sampler.clone())]));
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone()), Some(self.fx_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone()), Some(self.fx_cb.buf.clone())]));
            let stride = size_of::<Vertex>() as u32;
            let offset = 0u32;
            ctx.IASetVertexBuffers(0, 1, Some(&Some(self.quad.vb.clone())), Some(&stride), Some(&offset));
            ctx.IASetIndexBuffer(&self.quad.ib, DXGI_FORMAT_R32_UINT, 0);
        }
        let mut blend = None;
        for d in order {
            let Some(tex) = self.fx_textures.get(d.tex).and_then(|k| self.textures.get(k)) else { continue };
            if blend != Some(d.blend) {
                blend = Some(d.blend);
                unsafe { ctx.OMSetBlendState(if d.blend == FxBlend::Add { &self.fx_add } else { &self.fx_alpha }, Some(&[0.0; 4]), 0xFFFF_FFFF) };
            }
            let model = fx_model(d, eye);
            self.fx_cb.update(ctx, &FxCb { model: model.to_cols_array(), color: d.color, uv: d.uv, params: [(d.blend == FxBlend::Alpha) as u32 as f32, 0.0, 0.0, 0.0] });
            unsafe {
                ctx.PSSetShaderResources(0, Some(&[tex.srv.clone()]));
                ctx.DrawIndexed(self.quad.count, 0, 0);
            }
        }
        unsafe { ctx.OMSetBlendState(&self.blend, Some(&[0.0; 4]), 0xFFFF_FFFF) };
    }
}

/// Where a sprite's unit quad (in XY, -0.5..0.5) goes for an eye at `eye`.
pub fn fx_model(d: &FxDraw, eye: Vec3) -> Mat4 {
    let to = (d.pos - eye).normalize_or(Vec3::Z);
    let (x, y, z) = match d.shape {
        FxShape::Billboard { roll } => {
            let r = Vec3::Y.cross(to).normalize_or(Vec3::X);
            let u = to.cross(r);
            let (s, c) = roll.sin_cos();
            (r * c + u * s, u * c - r * s, to)
        }
        FxShape::Axial(axis) => {
            let a = axis.normalize_or(Vec3::Z);
            let y = to.cross(a).normalize_or(Vec3::Y);
            (a, y, a.cross(y))
        }
        FxShape::Fixed(q) => (q * Vec3::X, q * Vec3::Y, q * Vec3::Z),
    };
    Mat4::from_cols((x * d.size.x).extend(0.0), (y * d.size.y).extend(0.0), z.extend(0.0), d.pos.extend(1.0))
}

fn builtin_texture(gpu: &Gpu, f: fn(u32, u32) -> [u8; 4]) -> Result<Texture> {
    let n = 256u32;
    let mut levels: Vec<Vec<u8>> = Vec::new();
    let mut size = n;
    while size >= 1 {
        let mut px = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                px.extend_from_slice(&f(x * n / size, y * n / size));
            }
        }
        levels.push(px);
        if size == 1 {
            break;
        }
        size /= 2;
    }
    let mips: Vec<&[u8]> = levels.iter().map(|l| l.as_slice()).collect();
    Texture::upload(gpu, n, n, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &mips)
}

/// One-metre tiles with a thin line (the UVs are in metres).
fn grid_pixels(x: u32, y: u32) -> [u8; 4] {
    let line = x < 4 || y < 4;
    let check = ((x / 128) + (y / 128)).is_multiple_of(2);
    let v = if line { 90 } else if check { 200 } else { 185 };
    [v, v, v, 255]
}

/// Concentric red and white rings.
fn target_pixels(x: u32, y: u32) -> [u8; 4] {
    let dx = x as f32 - 127.5;
    let dy = y as f32 - 127.5;
    let r = (dx * dx + dy * dy).sqrt() / 128.0;
    if r > 1.0 {
        return [40, 40, 40, 255];
    }
    if r < 0.12 {
        return [230, 190, 30, 255];
    }
    if ((r * 5.0) as u32).is_multiple_of(2) { [200, 30, 30, 255] } else { [235, 235, 235, 255] }
}

/// The built-in effect sprites (when the game's aren't extracted).
pub const BUILTIN_FX: [(&str, fn(u32, u32) -> [u8; 4]); 6] =
    [("fx_soft", soft_pixels), ("fx_flash", flash_pixels), ("fx_streak", streak_pixels), ("fx_hole", hole_pixels), ("fx_smoke", smoke_pixels), ("fx_scorch", scorch_pixels)];

fn polar(x: u32, y: u32) -> (f32, f32) {
    let (dx, dy) = (x as f32 - 127.5, y as f32 - 127.5);
    ((dx * dx + dy * dy).sqrt() / 128.0, dy.atan2(dx))
}

/// A cheap hash noise in 0..1.
fn noise(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    (h & 0xFFFF) as f32 / 65535.0
}

/// A soft white dot, alpha and colour both falling off (glows, sparks, dust).
fn soft_pixels(x: u32, y: u32) -> [u8; 4] {
    let (r, _) = polar(x, y);
    let v = (1.0 - r).clamp(0.0, 1.0).powf(2.0);
    let c = (v * 255.0) as u8;
    [c, c, c, c]
}

/// A hot star: a white core, orange rays.
fn flash_pixels(x: u32, y: u32) -> [u8; 4] {
    let (r, a) = polar(x, y);
    let rays = (0.5 + 0.5 * (a * 7.0).cos()).powf(6.0) * (1.0 - r).max(0.0);
    let core = (1.0 - r * 2.2).clamp(0.0, 1.0);
    let v = (core + rays * 0.8).min(1.0);
    [(255.0 * v) as u8, (210.0 * v.powf(1.3)) as u8, (120.0 * v.powf(2.0)) as u8, (255.0 * v) as u8]
}

/// A tracer: a bright line along u, its head at u = 1, fading back along its tail.
fn streak_pixels(x: u32, y: u32) -> [u8; 4] {
    let u = x as f32 / 255.0;
    let across = ((y as f32 - 127.5) / 128.0).abs();
    let v = u.powf(1.5) * (1.0 - across).clamp(0.0, 1.0).powf(3.0);
    [(255.0 * v) as u8, (220.0 * v) as u8, (140.0 * v) as u8, (255.0 * v) as u8]
}

/// A bullet hole: black in the middle, a torn grey rim.
fn hole_pixels(x: u32, y: u32) -> [u8; 4] {
    let (r, a) = polar(x, y);
    let rim = 0.55 + 0.12 * (a * 5.0).sin() + 0.1 * noise(x / 8, y / 8);
    if r < 0.22 {
        [8, 8, 8, 250]
    } else if r < rim {
        let k = (r - 0.22) / (rim - 0.22);
        let c = (30.0 + 70.0 * k) as u8;
        [c, c, c, (230.0 * (1.0 - k * 0.6)) as u8]
    } else {
        [0, 0, 0, 0]
    }
}

/// A puff of smoke: soft, a little lumpy.
fn smoke_pixels(x: u32, y: u32) -> [u8; 4] {
    let (r, _) = polar(x, y);
    let lump = 0.75 + 0.25 * noise(x / 16, y / 16);
    let a = ((1.0 - r) * lump).clamp(0.0, 1.0).powf(1.5);
    [200, 200, 200, (a * 255.0) as u8]
}

/// A scorch mark: dark, ragged edge.
fn scorch_pixels(x: u32, y: u32) -> [u8; 4] {
    let (r, a) = polar(x, y);
    let edge = 0.8 + 0.15 * (a * 9.0).sin() * noise(x / 6, y / 6);
    let k = (1.0 - r / edge).clamp(0.0, 1.0);
    [12, 10, 8, (k.powf(0.6) * 220.0) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprites_face_the_eye_or_lie_along_their_axis() {
        let d = FxDraw { tex: 0, pos: Vec3::new(0.0, 1.5, 5.0), size: Vec2::new(0.4, 0.2), shape: FxShape::Billboard { roll: 0.0 }, color: [1.0; 4], uv: [0.0, 0.0, 1.0, 1.0], blend: FxBlend::Add };
        let eye = Vec3::new(0.0, 1.5, 0.0);
        let m = fx_model(&d, eye);
        // The quad spans x and y across the view, its normal along it.
        assert!((m.x_axis.truncate() - Vec3::X * 0.4).length() < 1e-5, "{:?}", m.x_axis);
        assert!((m.y_axis.truncate() - Vec3::Y * 0.2).length() < 1e-5);
        assert!((m.z_axis.truncate() - Vec3::Z).length() < 1e-5);
        // A tracer along +X seen from the side: long along X, square to the view.
        let t = FxDraw { shape: FxShape::Axial(Vec3::X), ..d };
        let m = fx_model(&t, eye);
        assert!((m.x_axis.truncate().normalize() - Vec3::X).length() < 1e-5);
        assert!(m.y_axis.truncate().normalize().dot(Vec3::Z).abs() < 1e-5, "turned to face the eye");
    }

    #[test]
    fn built_in_sprites_have_clear_edges() {
        for (name, f) in BUILTIN_FX {
            assert!(f(0, 0)[3] < 10, "{name}: the corner is clear");
        }
        assert!(hole_pixels(128, 128)[3] > 200 && hole_pixels(128, 128)[0] < 20, "a dark hole");
        assert!(streak_pixels(250, 128)[0] > streak_pixels(30, 128)[0], "the head is brighter");
    }
}
