//! GPU side: skinned meshes, the bone palette, per-bone visibility, lighting, the game-depth test
//! and the ghost hands.
//!
//! Each frame the rig is posed once per model and its skinning matrices go into a constant buffer
//! (a palette of up to 256 bones as 3x4 rows, plus a visibility half-space per bone). The vertex
//! shader blends four bones per vertex. Each mesh is one draw: its index buffer holds the whole
//! mesh and, after it, the triangles the arms view and the hands view can show, so those views
//! don't even run the rest of the body through the vertex shader.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3, Vec4};
use rustvr_asset as asset;
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer, static_buffer};
use rustvr_gpu::shader::{Program, element};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::{BODYVIS_ARMS, BODYVIS_HANDS, Fov, Joint, Pose};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::rig::{MAX_BONES, PoseInput, Posed, PosedFingers, Rig, Show};
use crate::{AvatarFrame, DepthInput, Source};

/// Near and far planes of the avatar's own depth buffer; the near plane matches the compositor's,
/// so the body clips where everything else the runtime draws does.
const NEAR: f32 = 0.03;
const FAR: f32 = 200.0;

/// Metres the body may poke out behind the game's surface before it's hidden: a fixed part for
/// depth noise, and a part per metre of distance for depth precision. Both depths are linear
/// 32-bit metres, so this only covers rasterization differences; it must stay well under the
/// thickness of a gun's grip, or fingers wrapped behind it show through.
const DEPTH_BIAS: f32 = 0.0015;
const DEPTH_BIAS_PER_METER: f32 = 0.002;

/// The ghost hands' colour (linear) and opacity.
const GHOST: [f32; 4] = [0.30, 0.62, 1.0, 0.35];

const SHADER: &str = r#"
#define MAX_BONES 256
cbuffer Frame : register(b0) {
    float4x4 view_proj;
    float4 view_z;       // view-space z (metres along the view axis) = dot(view_z.xyz, p) + view_z.w
    float4 eye_pos;
    float4 sun_dir;      // towards the sun
    float4 sun_color;
    float4 ambient;
    float4 depth;        // x: game depth bound, y: metres per game unit, z: bias, w: bias per metre
    float4 target_size;
    float4 ghost;        // rgb, opacity
};
cbuffer Bones : register(b1) {
    float4 bone_rows[MAX_BONES * 3];   // bind model space -> tracking space, 3x4 rows
    float4 vis_planes[MAX_BONES];      // visible where dot(xyz, bind position) + w >= 0
};
cbuffer Material : register(b2) { float4 tint; float4 material; };  // x: textured, y: alpha test
Texture2D albedo : register(t0);
Texture2D<float> game_depth : register(t1);
SamplerState samp : register(s0);

struct VSIn {
    float3 pos : POSITION;
    float3 normal : NORMAL;
    float2 uv : TEXCOORD0;
    uint4 bones : BLENDINDICES;
    float4 weights : BLENDWEIGHT;
};
struct VSOut {
    float4 pos : SV_Position;
    float3 world : TEXCOORD0;
    float3 normal : TEXCOORD1;
    float2 uv : TEXCOORD2;
    float view_z : TEXCOORD3;
    float vis : TEXCOORD4;
};

VSOut VSMain(VSIn i) {
    float4 w = i.weights / max(dot(i.weights, 1), 1e-5);
    float4 r0 = 0, r1 = 0, r2 = 0;
    float vis = 0;
    [unroll] for (int k = 0; k < 4; k++) {
        uint b = i.bones[k];
        r0 += bone_rows[b * 3 + 0] * w[k];
        r1 += bone_rows[b * 3 + 1] * w[k];
        r2 += bone_rows[b * 3 + 2] * w[k];
        vis += w[k] * (dot(vis_planes[b].xyz, i.pos) + vis_planes[b].w);
    }
    float4 p = float4(i.pos, 1);
    float3 world = float3(dot(r0, p), dot(r1, p), dot(r2, p));
    VSOut o;
    o.pos = mul(view_proj, float4(world, 1));
    o.world = world;
    o.normal = float3(dot(r0.xyz, i.normal), dot(r1.xyz, i.normal), dot(r2.xyz, i.normal));
    o.uv = i.uv;
    o.view_z = dot(view_z.xyz, world) + view_z.w;
    o.vis = vis;
    return o;
}

// The game drew something nearer than this fragment (game depth 0 means it drew nothing).
bool behind_game(float4 svpos, float z) {
    if (depth.x == 0) return false;
    uint w, h;
    game_depth.GetDimensions(w, h);
    int2 px = int2(svpos.xy * float2(w, h) / target_size.xy);
    float g = game_depth.Load(int3(px, 0)) * depth.y;
    return g > 0 && z > g + depth.z + depth.w * g;
}

float4 PSMain(VSOut i, bool front : SV_IsFrontFace) : SV_Target {
    if (i.vis < 0) discard;
    if (behind_game(i.pos, i.view_z)) discard;
    float3 base = tint.rgb;
    if (material.x > 0) {
        float4 t = albedo.Sample(samp, i.uv);
        if (material.y > 0 && t.a < 0.5) discard;
        base *= t.rgb;
    }
    // The inside of the shell, seen through a cut (a sleeve or the wrist): kept dark.
    if (!front) return float4(base * ambient.rgb * 0.3, 1);
    float3 n = normalize(i.normal);
    float3 v = normalize(eye_pos.xyz - i.world);
    float diffuse = saturate(dot(n, sun_dir.xyz));
    float sky = 0.7 + 0.3 * n.y;
    float3 c = base * (sun_color.rgb * diffuse + ambient.rgb * sky);
    // A faint rim that doesn't depend on the game's light, so hands still read in the dark.
    float rim = pow(1 - saturate(abs(dot(n, v))), 3);
    c += rim * (0.035 + 0.4 * ambient.rgb) * (0.4 + 0.6 * base);
    return float4(c, 1);
}

float4 PSGhost(VSOut i) : SV_Target {
    if (i.vis < 0) discard;
    float3 n = normalize(i.normal);
    float3 v = normalize(eye_pos.xyz - i.world);
    float f = pow(1 - saturate(abs(dot(n, v))), 2);
    float a = ghost.a * (0.45 + 1.2 * f);
    // Usually it's inside the wall that holds the real hand back: still shown, but fainter.
    if (behind_game(i.pos, i.view_z)) a *= 0.5;
    return float4(ghost.rgb * (0.45 + 1.1 * f), saturate(a));
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    bones: [u8; 4],
    weights: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameCb {
    view_proj: [f32; 16],
    view_z: [f32; 4],
    eye_pos: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    depth: [f32; 4],
    target_size: [f32; 4],
    ghost: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BonesCb {
    rows: [[[f32; 4]; 3]; MAX_BONES],
    vis: [[f32; 4]; MAX_BONES],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialCb {
    tint: [f32; 4],
    params: [f32; 4],
}

/// Index ranges per view: the whole mesh, then what the arms and the hands views can show.
const ALL: usize = 0;
const ARMS: usize = 1;
const HANDS: usize = 2;

struct GpuMesh {
    vb: ID3D11Buffer,
    ib: ID3D11Buffer,
    /// (first index, count) per view.
    ranges: [(u32, u32); 3],
    srv: Option<ID3D11ShaderResourceView>,
    material: ID3D11Buffer,
}

/// One drawable model: its rig, GPU meshes and palettes.
pub struct Character {
    rig: Rig,
    posed: Posed,
    meshes: Vec<GpuMesh>,
    /// Visibility planes for each [`Show`]: full, arms, hands, left hand, right hand.
    vis: [Vec<[f32; 4]>; 5],
    palette: Box<BonesCb>,
    body_cb: ConstantBuffer<BonesCb>,
    ghost_cbs: [ConstantBuffer<BonesCb>; 2],
    /// The hand poses the last [`Character::pose`] drew, for re-rooting the ghosts.
    drawn_hands: [Option<Pose>; 2],
    /// The skin on the CPU (bind positions and weights) and the bones' names, for measuring.
    cpu: Vec<Vec<asset::Vertex>>,
    bone_names: Vec<String>,
}

const SHOWS: [Show; 5] = [Show::Full, Show::Arms, Show::Hands, Show::Hand(0), Show::Hand(1)];

fn show_index(show: Show) -> usize {
    SHOWS.iter().position(|&s| s == show).unwrap()
}

impl Character {
    pub fn new(gpu: &Gpu, mut source: Source) -> Result<Character> {
        let root = source.bones.iter().position(|b| b.parent < 0).unwrap_or(0) as u8;
        for m in &mut source.meshes {
            for v in &mut m.vertices {
                // Unskinned vertices ride on the skeleton's root.
                if v.weights == [0; 4] {
                    v.bones = [root, 0, 0, 0];
                    v.weights = [255, 0, 0, 0];
                }
                if v.bones.iter().zip(v.weights).any(|(&b, w)| w > 0 && b as usize >= source.bones.len()) {
                    anyhow::bail!("mesh {} uses bone {:?} of {}", m.name, v.bones, source.bones.len());
                }
            }
            fix_winding(m);
        }
        let vertex_lists: Vec<&[asset::Vertex]> = source.meshes.iter().map(|m| m.vertices.as_slice()).collect();
        let rig = Rig::new(&source.bones, &vertex_lists)?;
        let vis = SHOWS.map(|s| rig.visibility(s));

        let mut meshes = Vec::new();
        for m in &source.meshes {
            let mut indices = m.indices.clone();
            let mut ranges = [(0, m.indices.len() as u32); 3];
            for (range, show) in [(ARMS, Show::Arms), (HANDS, Show::Hands)] {
                let planes = &vis[show_index(show)];
                let shown = |i: u32| visibility(&m.vertices[i as usize], planes) >= 0.0;
                let start = indices.len() as u32;
                for t in m.indices.chunks(3) {
                    if t.iter().any(|&i| shown(i)) {
                        indices.extend_from_slice(t);
                    }
                }
                ranges[range] = (start, indices.len() as u32 - start);
            }
            let vertices: Vec<Vertex> =
                m.vertices.iter().map(|v| Vertex { pos: v.pos, normal: v.normal, uv: v.uv, bones: v.bones, weights: v.weights }).collect();
            let (srv, cutout) = match &m.texture {
                Some(t) => match upload_texture(gpu, t) {
                    Ok(srv) => (Some(srv), is_cutout(t)),
                    Err(e) => {
                        tracing::warn!("avatar: texture of {}: {e:#}", m.name);
                        (None, false)
                    }
                },
                None => (None, false),
            };
            let material = MaterialCb { tint: [m.tint[0], m.tint[1], m.tint[2], 1.0], params: [srv.is_some() as u32 as f32, cutout as u32 as f32, 0.0, 0.0] };
            meshes.push(GpuMesh {
                vb: static_buffer(gpu, bytemuck::cast_slice(&vertices), D3D11_BIND_VERTEX_BUFFER)?,
                ib: static_buffer(gpu, bytemuck::cast_slice(&indices), D3D11_BIND_INDEX_BUFFER)?,
                ranges,
                srv,
                material: static_buffer(gpu, bytemuck::bytes_of(&material), D3D11_BIND_CONSTANT_BUFFER)?,
            });
        }
        let cpu = source.meshes.iter().map(|m| m.vertices.clone()).collect();
        let bone_names = source.bones.iter().map(|b| b.name.clone()).collect();
        Ok(Character {
            cpu,
            bone_names,
            posed: rig.posed(),
            rig,
            meshes,
            vis,
            palette: Box::new(BonesCb::zeroed()),
            body_cb: ConstantBuffer::new(gpu)?,
            ghost_cbs: [ConstantBuffer::new(gpu)?, ConstantBuffer::new(gpu)?],
            drawn_hands: [None; 2],
        })
    }

    /// Poses the rig for this frame.
    fn pose(&mut self, input: &PoseInput) {
        self.rig.pose(input, &mut self.posed);
        for side in 0..2 {
            let joint = [Joint::LeftHand, Joint::RightHand][side];
            self.drawn_hands[side] = input.hands[side].or(Some(input.joints[joint as usize])).filter(|p| p.valid != 0);
        }
    }

    /// Every vertex of the posed skin (tracking space), with the name of its heaviest bone.
    fn skin(&self) -> Vec<(Vec3, &str)> {
        let mut out = Vec::new();
        for mesh in &self.cpu {
            for v in mesh {
                let total: f32 = v.weights.iter().map(|&w| w as f32).sum::<f32>().max(1.0);
                let mut m = Mat4::ZERO;
                for k in 0..4 {
                    if v.weights[k] > 0 {
                        m += self.posed.skin[v.bones[k] as usize] * (v.weights[k] as f32 / total);
                    }
                }
                let heaviest = (0..4).max_by_key(|&k| v.weights[k]).unwrap_or(0);
                out.push((m.transform_point3(Vec3::from_array(v.pos)), self.bone_names[v.bones[heaviest] as usize].as_str()));
            }
        }
        out
    }

    /// Uploads the posed palette with the visibility of `show`.
    fn upload_body(&mut self, ctx: &ID3D11DeviceContext, show: Show) {
        self.fill_palette(Mat4::IDENTITY, show);
        self.body_cb.update(ctx, &self.palette);
    }

    /// Uploads the palette for a ghost of hand `side` at `at`: the posed body moved rigidly so its
    /// hand lands on the ghost, showing just that hand. False if there's no drawn hand to move.
    fn upload_ghost(&mut self, ctx: &ID3D11DeviceContext, side: usize, at: &Pose) -> bool {
        let Some(hand) = self.drawn_hands[side] else { return false };
        let rot = quat(at.rot) * quat(hand.rot).inverse();
        let moved = Mat4::from_rotation_translation(rot, Vec3::from_array(at.pos)) * Mat4::from_translation(-Vec3::from_array(hand.pos));
        self.fill_palette(moved, Show::Hand(side));
        self.ghost_cbs[side].update(ctx, &self.palette);
        true
    }

    fn fill_palette(&mut self, moved: Mat4, show: Show) {
        let vis = &self.vis[show_index(show)];
        for (b, skin) in self.posed.skin.iter().enumerate() {
            let m = moved * *skin;
            self.palette.rows[b] = [m.row(0).to_array(), m.row(1).to_array(), m.row(2).to_array()];
            self.palette.vis[b] = vis[b];
        }
    }
}

/// The CPU version of the shader's visibility, for picking each view's triangles.
fn visibility(v: &asset::Vertex, planes: &[[f32; 4]]) -> f32 {
    let p = Vec3::from_array(v.pos);
    let total: f32 = v.weights.iter().map(|&w| w as f32).sum::<f32>().max(1.0);
    (0..4)
        .filter(|&k| v.weights[k] > 0)
        .map(|k| {
            let pl = planes[v.bones[k] as usize];
            v.weights[k] as f32 / total * (Vec3::new(pl[0], pl[1], pl[2]).dot(p) + pl[3])
        })
        .sum()
}

/// Makes a mesh's front faces D3D's (clockwise on screen): when most triangles wind against
/// their vertex normals, the whole mesh is flipped. Game exporters disagree about winding.
fn fix_winding(m: &mut crate::SourceMesh) {
    let p = |i: u32| Vec3::from_array(m.vertices[i as usize].pos);
    let n = |i: u32| Vec3::from_array(m.vertices[i as usize].normal);
    let (mut with, mut against) = (0usize, 0usize);
    for t in m.indices.chunks(3) {
        let face = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        let d = face.dot(n(t[0]) + n(t[1]) + n(t[2]));
        if d > 0.0 {
            with += 1;
        } else if d < 0.0 {
            against += 1;
        }
    }
    if against > with {
        for t in m.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
    tracing::debug!("avatar: mesh {}: {with} triangles wound with their normals, {against} against", m.name);
}

fn upload_texture(gpu: &Gpu, t: &asset::Texture) -> Result<ID3D11ShaderResourceView> {
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
    let tex = Texture::upload(gpu, t.width, t.height, format, &mips)?;
    tex.srv.clone().ok_or_else(|| anyhow::anyhow!("no view"))
}

/// Whether a texture's alpha is a cut-out mask (hair cards, straps, fringes) rather than, say, a
/// specular mask: a good share of clearly transparent texels and not much in between.
fn is_cutout(t: &asset::Texture) -> bool {
    let Some(alpha) = small_mip_alpha(t) else { return false };
    let n = alpha.len().max(1) as f32;
    let clear = alpha.iter().filter(|&&a| a < 64).count() as f32 / n;
    let between = alpha.iter().filter(|&&a| (64..192).contains(&a)).count() as f32 / n;
    clear > 0.05 && between < 0.5 * clear.max(0.1)
}

/// Alpha of a mip around 64x64 (decoded for RGBA8, BC1, BC2 and BC3; None for formats without
/// alpha).
fn small_mip_alpha(t: &asset::Texture) -> Option<Vec<u8>> {
    let level = (0..t.mips.len()).find(|&i| (t.width >> i).max(1) <= 64 || i + 1 == t.mips.len())?;
    let data = &t.mips[level];
    let mut out = Vec::new();
    match t.format {
        asset::TexFormat::Rgba8 => out.extend(data.as_chunks::<4>().0.iter().map(|p| p[3])),
        asset::TexFormat::Bc1 => {
            for b in data.as_chunks::<8>().0 {
                let c0 = u16::from_le_bytes([b[0], b[1]]);
                let c1 = u16::from_le_bytes([b[2], b[3]]);
                let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                // Index 3 is transparent in the three-colour mode (c0 <= c1).
                out.extend((0..16).map(|k| if c0 <= c1 && (bits >> (2 * k)) & 3 == 3 { 0 } else { 255 }));
            }
        }
        asset::TexFormat::Bc2 => {
            for b in data.as_chunks::<16>().0 {
                out.extend((0..16).map(|k| ((b[k / 2] >> (4 * (k % 2))) & 15) * 17));
            }
        }
        asset::TexFormat::Bc3 => {
            for b in data.as_chunks::<16>().0 {
                let (a0, a1) = (b[0] as u32, b[1] as u32);
                let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
                out.extend((0..16).map(|k| {
                    let i = ((bits >> (3 * k)) & 7) as u32;
                    let a = match (i, a0 > a1) {
                        (0, _) => a0,
                        (1, _) => a1,
                        (i, true) => ((8 - i) * a0 + (i - 1) * a1) / 7,
                        (6, false) => 0,
                        (7, false) => 255,
                        (i, false) => ((6 - i) * a0 + (i - 1) * a1) / 5,
                    };
                    a as u8
                }));
            }
        }
        _ => return None,
    }
    Some(out)
}

pub struct Renderer {
    opaque: Program,
    ghost: Program,
    frame_cb: ConstantBuffer<FrameCb>,
    two_sided: ID3D11RasterizerState,
    cull_back: ID3D11RasterizerState,
    depth_write: ID3D11DepthStencilState,
    depth_test: ID3D11DepthStencilState,
    opaque_blend: ID3D11BlendState,
    alpha_blend: ID3D11BlendState,
    sampler: ID3D11SamplerState,
    /// The avatar's own depth buffer, kept until the eye size changes.
    depth: Option<Texture>,
}

impl Renderer {
    pub fn new(gpu: &Gpu) -> Result<Renderer> {
        let layout = [
            element(b"POSITION\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0),
            element(b"NORMAL\0", 0, DXGI_FORMAT_R32G32B32_FLOAT, 12),
            element(b"TEXCOORD\0", 0, DXGI_FORMAT_R32G32_FLOAT, 24),
            element(b"BLENDINDICES\0", 0, DXGI_FORMAT_R8G8B8A8_UINT, 32),
            element(b"BLENDWEIGHT\0", 0, DXGI_FORMAT_R8G8B8A8_UNORM, 36),
        ];
        Ok(Renderer {
            opaque: Program::new(gpu, SHADER, "avatar", &layout)?,
            ghost: Program::with_entries(gpu, SHADER, "avatar", "VSMain", "PSGhost", &layout)?,
            frame_cb: ConstantBuffer::new(gpu)?,
            two_sided: pipeline::raster_state(gpu, false, false)?,
            cull_back: pipeline::raster_state(gpu, true, false)?,
            depth_write: pipeline::depth_state(gpu, true, true)?,
            depth_test: pipeline::depth_state(gpu, true, false)?,
            opaque_blend: pipeline::blend_state(gpu, Blend::Opaque)?,
            alpha_blend: pipeline::blend_state(gpu, Blend::Alpha)?,
            sampler: pipeline::sampler(gpu, true, true)?,
            depth: None,
        })
    }

    pub fn draw(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        frame: &AvatarFrame,
        depth: Option<DepthInput>,
        body: &mut Character,
        arms: Option<&mut Character>,
    ) -> Result<()> {
        let ctx = &gpu.ctx;
        let show = show_of(frame);
        let input = pose_input(frame);
        let ghosts = frame.ghosts.map(|g| g.filter(|p| p.valid != 0));

        // The first-person arms model (if any) draws the arms and hands views and the ghosts; the
        // body draws the full view.
        let (main, ghost_model) = match arms {
            Some(arms) if show != Show::Full => (arms, None),
            Some(arms) => (body, Some(arms)),
            None => (body, None),
        };
        main.pose(&input);
        main.upload_body(ctx, show);

        self.frame_cb.update(ctx, &frame_constants(width, height, eye, fov, frame, depth.as_ref()));
        if self.depth.as_ref().is_none_or(|d| d.width != width || d.height != height) {
            self.depth = Some(Texture::new(gpu, &TexDesc::depth(width, height))?);
        }
        let dsv = self.depth.as_ref().and_then(|d| d.dsv.clone()).unwrap();
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), &dsv);
            ctx.ClearDepthStencilView(&dsv, D3D11_CLEAR_DEPTH.0, 1.0, 0);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone()), Some(main.body_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.PSSetShaderResources(1, Some(&[depth.as_ref().map(|d| d.srv.clone())]));
            ctx.OMSetBlendState(&self.opaque_blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_write, 0);
            ctx.RSSetState(&self.two_sided);
        }
        pipeline::viewport(ctx, width, height);
        self.opaque.bind(ctx);
        let range = match show {
            Show::Full => ALL,
            Show::Arms => ARMS,
            _ => HANDS,
        };
        draw_meshes(ctx, main, range);

        let mut ghost_ready = [false; 2];
        if ghosts.iter().any(Option::is_some) {
            let ghost_model = match ghost_model {
                Some(g) => {
                    g.pose(&input);
                    g
                }
                None => main,
            };
            for side in 0..2 {
                if let Some(at) = &ghosts[side] {
                    ghost_ready[side] = ghost_model.upload_ghost(ctx, side, at);
                }
            }
            self.draw_ghosts(ctx, ghost_model, ghost_ready);
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&[None, None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// The skin of the model `frame` draws (the arms model in the arms and hands views, when there
    /// is one), posed for it: tracking-space vertices with their heaviest bone's name.
    pub fn skin<'a>(&mut self, frame: &AvatarFrame, body: &'a mut Character, arms: Option<&'a mut Character>) -> Vec<(Vec3, &'a str)> {
        let main = match arms {
            Some(arms) if show_of(frame) != Show::Full => arms,
            _ => body,
        };
        main.pose(&pose_input(frame));
        main.skin()
    }

    /// Translucent hands over everything drawn so far, tested against (but not writing) depth.
    fn draw_ghosts(&self, ctx: &ID3D11DeviceContext, ghost_model: &Character, ghost_ready: [bool; 2]) {
        if !ghost_ready.iter().any(|&r| r) {
            return;
        }
        self.ghost.bind(ctx);
        unsafe {
            ctx.OMSetBlendState(&self.alpha_blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_test, 0);
            ctx.RSSetState(&self.cull_back);
        }
        for side in 0..2 {
            if ghost_ready[side] {
                unsafe { ctx.VSSetConstantBuffers(1, Some(&[Some(ghost_model.ghost_cbs[side].buf.clone())])) };
                draw_meshes(ctx, ghost_model, HANDS);
            }
        }
    }
}

fn show_of(frame: &AvatarFrame) -> Show {
    match frame.visibility {
        BODYVIS_HANDS => Show::Hands,
        BODYVIS_ARMS => Show::Arms,
        _ => Show::Full,
    }
}

/// The rig's inputs for a frame. A hand in the game's own pose goes exactly there, its fingers as
/// the game has them (the index following the trigger).
fn pose_input<'a>(frame: &'a AvatarFrame) -> PoseInput<'a> {
    let hands = [0, 1].map(|i| match &frame.held[i] {
        Some(h) => Some(h.wrist).filter(|p| p.valid != 0),
        None => Some(frame.hands[i].pose).filter(|p| p.valid != 0),
    });
    let held = frame.held.map(|h| h.map(|h| PosedFingers { rel: h.finger_rotations(), size: h.size, reach: h.reach_points() }));
    PoseInput { joints: frame.joints, hands, fingers: &frame.fingers, held, chop_head: frame.first_person && show_of(frame) == Show::Full }
}

fn draw_meshes(ctx: &ID3D11DeviceContext, c: &Character, range: usize) {
    let stride = size_of::<Vertex>() as u32;
    let offset = 0u32;
    for m in &c.meshes {
        let (start, count) = m.ranges[range];
        if count == 0 {
            continue;
        }
        unsafe {
            ctx.IASetVertexBuffers(0, 1, Some(&Some(m.vb.clone())), Some(&stride), Some(&offset));
            ctx.IASetIndexBuffer(&m.ib, DXGI_FORMAT_R32_UINT, 0);
            ctx.PSSetConstantBuffers(2, Some(&[Some(m.material.clone())]));
            ctx.PSSetShaderResources(0, Some(std::slice::from_ref(&m.srv)));
            ctx.DrawIndexed(count, start, 0);
        }
    }
}

fn frame_constants(width: u32, height: u32, eye: &Pose, fov: &Fov, frame: &AvatarFrame, depth: Option<&DepthInput>) -> FrameCb {
    let view = view_matrix(eye);
    let l = &frame.light;
    let unset = l.sun_color == [0.0; 3] && l.ambient == [0.0; 3];
    let (sun_dir, sun_color, ambient) = if unset {
        (Vec3::new(0.35, 0.85, 0.4).normalize(), [0.85, 0.82, 0.78], [0.22, 0.24, 0.28])
    } else {
        (Vec3::from_array(l.sun_dir).normalize_or(Vec3::Y), l.sun_color, l.ambient)
    };
    let eye_pos = Vec3::from_array(eye.pos);
    FrameCb {
        view_proj: (projection(fov) * view).to_cols_array(),
        view_z: view.row(2).to_array(),
        eye_pos: eye_pos.extend(1.0).to_array(),
        sun_dir: sun_dir.extend(0.0).to_array(),
        sun_color: [sun_color[0], sun_color[1], sun_color[2], 0.0],
        ambient: [ambient[0], ambient[1], ambient[2], 0.0],
        depth: match depth {
            Some(d) => [1.0, 1.0 / d.units_per_meter.max(1e-6), DEPTH_BIAS, DEPTH_BIAS_PER_METER],
            None => [0.0; 4],
        },
        target_size: [width as f32, height as f32, 0.0, 0.0],
        ghost: GHOST,
    }
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

/// Tracking space -> eye space (the same as the compositor's).
fn view_matrix(eye: &Pose) -> Mat4 {
    Mat4::from_rotation_translation(quat(eye.rot), Vec3::from_array(eye.pos)).inverse()
}

/// Left-handed off-centre projection with D3D depth from FOV tangents (the same as the
/// compositor's).
fn projection(fov: &Fov) -> Mat4 {
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
