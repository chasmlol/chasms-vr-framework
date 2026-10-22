//! GPU side: skinned meshes, the bone palette, per-bone visibility, lighting (the game's own
//! model, `metro.rs`, or the runtime's), the game-depth test and the ghost hands.
//!
//! Each frame the rig is posed once per model and its skinning matrices go into a constant buffer
//! (a palette of up to 256 bones as 3x4 rows, plus a visibility half-space per bone). The vertex
//! shader blends four bones per vertex. Each mesh is one draw: its index buffer holds the whole
//! mesh and, after it, the triangles the arms view and the hands view can show, so those views
//! don't even run the rest of the body through the vertex shader.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Quat, Vec3, Vec4};
use rustvr_asset as asset;
use rustvr_gpu::pipeline::{self, Blend, ConstantBuffer, static_buffer};
use rustvr_gpu::shader::{Program, element};
use rustvr_gpu::{Gpu, TexDesc, Texture};
use rustvr_protocol::{BODYVIS_ARMS, BODYVIS_HANDS, Fov, GAME_LIGHTS_MAX, GameLight, Joint, LIGHT_ATLAS_H, LIGHT_ATLAS_W, LUT_SIZE, Pose};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::rig::{MAX_BONES, PoseInput, Posed, PosedFingers, Rig, Show};
use crate::metro::{self, Class};
use crate::{AvatarFrame, DepthInput, GameShading, Source};

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
cbuffer Material : register(b2) {
    float4 tint;
    float4 material;     // x: textured, y: alpha test, z: the game's shading class (M_*), w: emissive ×
    float4 surf_params;  // the game's surf_params
    float4 surf_color;
    float4 options;      // x: use_env, y: use_fresnel (the class's shader permutation)
};
// The game's lighting (metro.rs), when it publishes it.
cbuffer Lighting : register(b3) {
    float4x4 game_view;        // tracking space -> the game camera's view space
    float4 view_to_world[3];   // the game camera's view space -> the game's world (rows)
    float4 sh_r;
    float4 sh_g;
    float4 sh_b;
    float4 amb;                // flat ambient, w: reflection scale
    float4 lit;                // x: hemi, y: depth squash, z: SSAO, w: grading table valid
    float4 envc;               // the environment map's tint, w: light count
    float4 up;                 // the world's up direction in the game camera's view space
    float4 atlas;              // x: light atlas bound, yz: 1 / its size
    MLight lights[32];
};
Texture2D albedo : register(t0);
Texture2D<float> game_depth : register(t1);
Texture2D bump_tex : register(t2);
Texture2D detail_tex : register(t3);
TextureCube env_tex : register(t4);
Texture3D grade_tex : register(t5);
Texture2D atlas_tex : register(t6);
SamplerState samp : register(s0);
SamplerState clamp_samp : register(s1);

struct VSIn {
    float3 pos : POSITION;
    float3 normal : NORMAL;
    float2 uv : TEXCOORD0;
    uint4 bones : BLENDINDICES;
    float4 weights : BLENDWEIGHT;
    float4 ao : COLOR0;        // x: the game's baked vertex AO
};
struct VSOut {
    float4 pos : SV_Position;
    float3 world : TEXCOORD0;
    float3 normal : TEXCOORD1;
    float2 uv : TEXCOORD2;
    float view_z : TEXCOORD3;
    float vis : TEXCOORD4;
    float ao : TEXCOORD5;
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
    // Unit length, as the game's vertex normals are (the rig's matrices scale the body).
    float3 n = float3(dot(r0.xyz, i.normal), dot(r1.xyz, i.normal), dot(r2.xyz, i.normal));
    o.normal = n * rsqrt(max(dot(n, n), 1e-12));
    o.uv = i.uv;
    o.view_z = dot(view_z.xyz, world) + view_z.w;
    o.vis = vis;
    o.ao = i.ao.x;
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

float3 to_world(float3 v) { return float3(dot(view_to_world[0].xyz, v), dot(view_to_world[1].xyz, v), dot(view_to_world[2].xyz, v)); }

// A light's texture term from the light atlas (1 without one).
float4 atlas_term(MLight l, float3 p) {
    if ((l.info.y & M_GLF_TILE) == 0 || atlas.x == 0) return 1;
    float2 a, b;
    float k;
    m_atlas_taps(l.info.z, p, lit.y, a, b, k);
    return lerp(atlas_tex.SampleLevel(clamp_samp, a * atlas.yz, 0), atlas_tex.SampleLevel(clamp_samp, b * atlas.yz, 0), k);
}

// The game's post chain on an HDR colour, as the value to write to the sRGB eye target so it
// stores the code value the game would have displayed.
float3 finish(float3 hdr) {
    float3 tm = m_tonemap(hdr);
    float4 l = grade_tex.SampleLevel(clamp_samp, tm.bgr, 0);
    return m_display_to_linear(m_grade(tm, l, lit.w != 0));
}

// The game's own shading of its first-person model (metro.rs), evaluated for this fragment.
float4 PSMetro(VSOut i, bool front : SV_IsFrontFace) : SV_Target {
    float3 pv = mul(game_view, float4(i.world, 1)).xyz;
    float3 nv = mul((float3x3)game_view, i.normal);
    float3 dpx = ddx_coarse(pv), dpy = ddy_coarse(pv);
    float2 dux = ddx_coarse(i.uv), duy = ddy_coarse(i.uv);
    if (i.vis < 0) discard;
    if (behind_game(i.pos, i.view_z)) discard;
    uint cls = (uint)material.z;
    // The game's v2.w: vertex AO × the per-object hemi.
    MFrag f = m_frag(pv, nv, surf_params, i.ao * lit.x);
    MTex x = (MTex)0;
    float3 n = 0;
    if (cls == M_EMISSIVE) {
        x.base = albedo.Sample(samp, i.uv);
        if (material.y > 0 && x.base.a < 0.5) discard;
        x.detail = detail_tex.Sample(samp, i.uv * surf_params.xy).rgb;
    } else {
        float3 t, b;
        m_frame(dpx, dpy, dux, duy, nv, t, b);
        float2 uv = m_parallax(i.uv, t, b, f, bump_tex.Sample(samp, i.uv).a);
        x.base = albedo.Sample(samp, uv);
        if (material.y > 0 && x.base.a < 0.5) discard;
        x.bump_l = bump_tex.Sample(samp, uv - float2(M_BUMP_STEP, 0));
        x.height_r = bump_tex.Sample(samp, uv + float2(M_BUMP_STEP, 0)).a;
        x.height_u = bump_tex.Sample(samp, uv - float2(0, M_BUMP_STEP)).a;
        x.height_d = bump_tex.Sample(samp, uv + float2(0, M_BUMP_STEP)).a;
        x.gloss = bump_tex.SampleBias(samp, uv, 3).g;
        x.detail = detail_tex.Sample(samp, uv * surf_params.xy).rgb;
        n = m_bumped_normal(t, b, f, x, cls);
        if (cls != M_PLAIN && (cls != M_CLOTH_SKIN || options.x != 0)) x.env = env_tex.Sample(clamp_samp, to_world(m_reflection(pv, n))).rgb;
    }
    // The inside of the shell, seen through a cut (the game culls it): the flat ambient, dark.
    if (!front) return float4(finish(x.base.rgb * amb.rgb * 0.3), 1);
    MG g = m_gbuffer(cls, f, n, x, surf_color.rgb, material.w, options.xy, lit.y, envc.rgb, up.xyz);
    float3 d = 0, s = 0;
    if (!g.emissive) {
        uint count = (uint)envc.w;
        for (uint k = 0; k < count; k++) {
            if (!m_reaches(pv, lights[k])) continue;
            float3 ld, ls;
            m_light(g, lights[k], atlas_term(lights[k], g.p), ld, ls);
            d += ld;
            s += ls;
        }
    }
    return float4(finish(m_combine(g, d, s, sh_r, sh_g, sh_b, amb, lit.z, m_local_ao_r(d), 0)), 1);
}

// The game's forward glass, blended over what's drawn.
float4 PSMetroGlass(VSOut i) : SV_Target {
    if (i.vis < 0) discard;
    if (behind_game(i.pos, i.view_z)) discard;
    uint cls = (uint)material.z;
    float3 pv = mul(game_view, float4(i.world, 1)).xyz;
    float3 nv = normalize(mul((float3x3)game_view, i.normal));
    MTex x = (MTex)0;
    x.base = albedo.Sample(samp, i.uv);
    if (cls == M_GLASS_REFLECT) x.env = env_tex.Sample(clamp_samp, to_world(m_reflection(pv, nv))).rgb;
    float4 c = m_glass(cls, x, envc.rgb);
    return float4(finish(c.rgb), c.a);
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
    /// x: the game's baked vertex AO (255: none).
    ao: [u8; 4],
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
    surf_params: [f32; 4],
    surf_color: [f32; 4],
    options: [f32; 4],
}

/// The game's lighting, as the shader's `Lighting` buffer holds it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightingCb {
    game_view: [f32; 16],
    view_to_world: [[f32; 4]; 3],
    sh: [[f32; 4]; 3],
    ambient: [f32; 4],
    lit: [f32; 4],
    envc: [f32; 4],
    up: [f32; 4],
    atlas: [f32; 4],
    lights: [GameLight; GAME_LIGHTS_MAX],
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
    /// How the game shades it, when that's known.
    metro: Option<MetroMesh>,
}

/// A mesh's game material on the GPU: its class and the views of t0 and t2..t4 (raw: the game's
/// textures are UNORM, so their numbers are used as linear values).
struct MetroMesh {
    class: Class,
    base: ID3D11ShaderResourceView,
    bump: Option<ID3D11ShaderResourceView>,
    detail: Option<ID3D11ShaderResourceView>,
    env: Option<ID3D11ShaderResourceView>,
}

/// A texture's two views: sRGB-decoded (the runtime's own look) and raw (the game's).
#[derive(Clone)]
struct GpuTex {
    srgb: ID3D11ShaderResourceView,
    linear: ID3D11ShaderResourceView,
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
        // Uploaded once each, however many meshes share them.
        let mut uploaded: HashMap<*const asset::Texture, Option<GpuTex>> = HashMap::new();
        let mut upload = |t: &Arc<asset::Texture>, what: &str| {
            uploaded
                .entry(Arc::as_ptr(t))
                .or_insert_with(|| upload_texture(gpu, t).map_err(|e| tracing::warn!("avatar: texture of {what}: {e:#}")).ok())
                .clone()
        };
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
                m.vertices.iter().map(|v| Vertex { pos: v.pos, normal: v.normal, uv: v.uv, bones: v.bones, weights: v.weights, ao: [v.ao, 0, 0, 0] }).collect();
            let base = m.texture.as_ref().and_then(|t| upload(t, &m.name));
            // The game's own flag where it has one, else a guess from the texture.
            let cutout = base.is_some() && m.material.as_ref().map_or_else(|| m.texture.as_deref().is_some_and(is_cutout), |mat| mat.alpha_test);
            let metro = match (&m.material, &base) {
                (Some(mat), Some(base)) => {
                    let mut get = |t: &Option<Arc<asset::Texture>>| t.as_ref().and_then(|t| upload(t, &m.name)).map(|t| t.linear);
                    let (bump, detail, env) = (get(&mat.bump), get(&mat.detail), get(&mat.env));
                    let (nb, nd, ne) = mat.class.needs();
                    ((!nb || bump.is_some()) && (!nd || detail.is_some()) && (!ne || env.is_some()))
                        .then(|| MetroMesh { class: mat.class, base: base.linear.clone(), bump, detail, env })
                }
                _ => None,
            };
            let (surf_params, surf_color, emissive) = m.material.as_ref().map_or(([0.0; 4], [0.0; 4], 0.0), |mat| (mat.params, mat.color, mat.emissive));
            let options = m.material.as_ref().map_or([0.0; 4], |mat| [mat.env.is_some() as u32 as f32, mat.fresnel as u32 as f32, 0.0, 0.0]);
            let class = metro.as_ref().map_or(0.0, |mm| mm.class as u32 as f32);
            let material = MaterialCb {
                tint: [m.tint[0], m.tint[1], m.tint[2], 1.0],
                params: [base.is_some() as u32 as f32, cutout as u32 as f32, class, emissive],
                surf_params,
                surf_color,
                options,
            };
            let srv = base.map(|b| b.srgb);
            meshes.push(GpuMesh {
                vb: static_buffer(gpu, bytemuck::cast_slice(&vertices), D3D11_BIND_VERTEX_BUFFER)?,
                ib: static_buffer(gpu, bytemuck::cast_slice(&indices), D3D11_BIND_INDEX_BUFFER)?,
                ranges,
                srv,
                material: static_buffer(gpu, bytemuck::bytes_of(&material), D3D11_BIND_CONSTANT_BUFFER)?,
                metro,
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

/// Uploads a texture (a cube map when it has six faces) with an sRGB-decoding and a raw view.
fn upload_texture(gpu: &Gpu, t: &asset::Texture) -> Result<GpuTex> {
    let (storage, srgb, linear) = match t.format {
        asset::TexFormat::Rgba8 => (DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_UNORM),
        asset::TexFormat::Bc1 => (DXGI_FORMAT_BC1_TYPELESS, DXGI_FORMAT_BC1_UNORM_SRGB, DXGI_FORMAT_BC1_UNORM),
        asset::TexFormat::Bc2 => (DXGI_FORMAT_BC2_TYPELESS, DXGI_FORMAT_BC2_UNORM_SRGB, DXGI_FORMAT_BC2_UNORM),
        asset::TexFormat::Bc3 => (DXGI_FORMAT_BC3_TYPELESS, DXGI_FORMAT_BC3_UNORM_SRGB, DXGI_FORMAT_BC3_UNORM),
        asset::TexFormat::Bc4 => (DXGI_FORMAT_BC4_UNORM, DXGI_FORMAT_BC4_UNORM, DXGI_FORMAT_BC4_UNORM),
        asset::TexFormat::Bc5 => (DXGI_FORMAT_BC5_UNORM, DXGI_FORMAT_BC5_UNORM, DXGI_FORMAT_BC5_UNORM),
        asset::TexFormat::Bc7 => (DXGI_FORMAT_BC7_TYPELESS, DXGI_FORMAT_BC7_UNORM_SRGB, DXGI_FORMAT_BC7_UNORM),
    };
    let faces = t.faces.max(1) as usize;
    anyhow::ensure!(faces == 1 || faces == 6, "{} faces", t.faces);
    anyhow::ensure!(!t.mips.is_empty() && t.mips.len().is_multiple_of(faces), "{} mips over {faces} faces", t.mips.len());
    let levels = t.mips.len() / faces;
    let block = rustvr_gpu::texture::block_bytes(linear);
    let init: Vec<D3D11_SUBRESOURCE_DATA> = t
        .mips
        .iter()
        .enumerate()
        .map(|(i, data)| {
            let w = (t.width >> (i % levels)).max(1);
            let pitch = match block {
                Some(b) => w.div_ceil(4) * b,
                None => w * 4,
            };
            D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr().cast(), SysMemPitch: pitch, SysMemSlicePitch: 0 }
        })
        .collect();
    let desc = D3D11_TEXTURE2D_DESC {
        Width: t.width,
        Height: t.height,
        MipLevels: levels as u32,
        ArraySize: faces as u32,
        Format: storage,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: if faces == 6 { D3D11_RESOURCE_MISC_TEXTURECUBE.0 as u32 } else { 0 },
    };
    unsafe {
        let mut tex = None;
        gpu.device.CreateTexture2D(&desc, Some(init.as_ptr()), Some(&mut tex))?;
        let tex = tex.context("no texture")?;
        let view = |format: DXGI_FORMAT| -> Result<ID3D11ShaderResourceView> {
            let mut d = D3D11_SHADER_RESOURCE_VIEW_DESC { Format: format, ..Default::default() };
            if faces == 6 {
                d.ViewDimension = windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURECUBE;
                d.Anonymous.TextureCube = D3D11_TEXCUBE_SRV { MostDetailedMip: 0, MipLevels: levels as u32 };
            } else {
                d.ViewDimension = windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2D;
                d.Anonymous.Texture2D = D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: levels as u32 };
            }
            let mut srv = None;
            gpu.device.CreateShaderResourceView(&tex, Some(&d), Some(&mut srv))?;
            srv.context("no view")
        };
        Ok(GpuTex { srgb: view(srgb)?, linear: view(linear)? })
    }
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
    /// The game's shading (metro.rs): opaque classes and forward glass.
    metro: Program,
    metro_glass: Program,
    frame_cb: ConstantBuffer<FrameCb>,
    lighting_cb: ConstantBuffer<LightingCb>,
    /// The game's colour grading table (16³ RGBA8) and its view.
    lut: ID3D11Texture3D,
    lut_srv: ID3D11ShaderResourceView,
    clamp_sampler: ID3D11SamplerState,
    /// The game's lighting is set for this frame ([`Renderer::set_shading`]).
    shading: bool,
    atlas: Option<ID3D11ShaderResourceView>,
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
            element(b"COLOR\0", 0, DXGI_FORMAT_R8G8B8A8_UNORM, 40),
        ];
        let source = format!("{}{SHADER}", metro::HLSL);
        let (lut, lut_srv) = lut_texture(gpu)?;
        Ok(Renderer {
            opaque: Program::new(gpu, &source, "avatar", &layout)?,
            ghost: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSGhost", &layout)?,
            metro: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSMetro", &layout)?,
            metro_glass: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSMetroGlass", &layout)?,
            frame_cb: ConstantBuffer::new(gpu)?,
            lighting_cb: ConstantBuffer::new(gpu)?,
            lut,
            lut_srv,
            clamp_sampler: clamp_sampler(gpu)?,
            shading: false,
            atlas: None,
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

    /// The game's lighting for this frame (None: the runtime's own look).
    pub fn set_shading(&mut self, gpu: &Gpu, shading: Option<&GameShading>) {
        self.atlas = shading.and_then(|s| s.atlas.cloned());
        let Some(sh) = shading else {
            self.shading = false;
            return;
        };
        let l = sh.lighting;
        let view = view_matrix(&l.eye);
        // The game camera's view space -> tracking -> the game's world.
        let to_world = Mat3::from_quat(sh.world_rot * quat(l.eye.rot));
        let count = (l.count as usize).min(GAME_LIGHTS_MAX);
        let cb = LightingCb {
            game_view: view.to_cols_array(),
            view_to_world: [0, 1, 2].map(|r| to_world.row(r).extend(0.0).to_array()),
            sh: l.sh,
            ambient: l.ambient,
            lit: [l.hemi, l.depth_squash, l.ssao, (l.lut_valid != 0) as u32 as f32],
            envc: metro::envmap_color(l).extend(count as f32).to_array(),
            // m_iV[1]: the world's y in view space.
            up: to_world.row(1).extend(0.0).to_array(),
            atlas: [self.atlas.is_some() as u32 as f32, 1.0 / LIGHT_ATLAS_W as f32, 1.0 / LIGHT_ATLAS_H as f32, 0.0],
            lights: l.lights,
        };
        self.lighting_cb.update(&gpu.ctx, &cb);
        if l.lut_valid != 0 {
            let row = (LUT_SIZE * 4) as u32;
            unsafe { gpu.ctx.UpdateSubresource(&self.lut, 0, None, l.lut.as_ptr().cast(), row, row * LUT_SIZE as u32) };
        }
        self.shading = true;
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
            ctx.PSSetConstantBuffers(3, Some(&[Some(self.lighting_cb.buf.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone()), Some(self.clamp_sampler.clone())]));
            ctx.PSSetShaderResources(1, Some(&[depth.as_ref().map(|d| d.srv.clone())]));
            ctx.PSSetShaderResources(5, Some(&[Some(self.lut_srv.clone()), self.atlas.clone()]));
            ctx.OMSetBlendState(&self.opaque_blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_write, 0);
            ctx.RSSetState(&self.two_sided);
        }
        pipeline::viewport(ctx, width, height);
        let range = match show {
            Show::Full => ALL,
            Show::Arms => ARMS,
            _ => HANDS,
        };
        self.draw_body(ctx, main, range);

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
            ctx.PSSetShaderResources(0, Some(&[None, None, None, None, None, None, None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// The body's meshes: with the game's shading where it's set and the mesh has the game's
    /// material (its forward glass blended after everything opaque), else the runtime's own look.
    fn draw_body(&self, ctx: &ID3D11DeviceContext, c: &Character, range: usize) {
        let shading = self.shading;
        fn metro_of(m: &GpuMesh, shading: bool) -> Option<&MetroMesh> {
            m.metro.as_ref().filter(|_| shading)
        }
        let mut bound: Option<bool> = None;
        for m in &c.meshes {
            let metro = metro_of(m, shading);
            if metro.is_some_and(|mm| mm.class.forward()) {
                continue;
            }
            if bound != Some(metro.is_some()) {
                if metro.is_some() { self.metro.bind(ctx) } else { self.opaque.bind(ctx) }
                bound = Some(metro.is_some());
            }
            draw_mesh(ctx, m, range);
        }
        if c.meshes.iter().any(|m| metro_of(m, shading).is_some_and(|mm| mm.class.forward())) {
            self.metro_glass.bind(ctx);
            unsafe {
                ctx.OMSetBlendState(&self.alpha_blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
                ctx.OMSetDepthStencilState(&self.depth_test, 0);
                ctx.RSSetState(&self.cull_back);
            }
            for m in c.meshes.iter().filter(|m| metro_of(m, shading).is_some_and(|mm| mm.class.forward())) {
                draw_mesh(ctx, m, range);
            }
            unsafe {
                ctx.OMSetBlendState(&self.opaque_blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
                ctx.OMSetDepthStencilState(&self.depth_write, 0);
                ctx.RSSetState(&self.two_sided);
            }
        }
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
    for m in &c.meshes {
        draw_mesh(ctx, m, range);
    }
}

/// One mesh with its material's textures (the game's, raw, when the bound program shades with
/// the game's model; they're ignored otherwise).
fn draw_mesh(ctx: &ID3D11DeviceContext, m: &GpuMesh, range: usize) {
    let stride = size_of::<Vertex>() as u32;
    let offset = 0u32;
    let (start, count) = m.ranges[range];
    if count == 0 {
        return;
    }
    unsafe {
        ctx.IASetVertexBuffers(0, 1, Some(&Some(m.vb.clone())), Some(&stride), Some(&offset));
        ctx.IASetIndexBuffer(&m.ib, DXGI_FORMAT_R32_UINT, 0);
        ctx.PSSetConstantBuffers(2, Some(&[Some(m.material.clone())]));
        match &m.metro {
            Some(mm) => {
                ctx.PSSetShaderResources(0, Some(&[Some(mm.base.clone())]));
                ctx.PSSetShaderResources(2, Some(&[mm.bump.clone(), mm.detail.clone(), mm.env.clone()]));
            }
            None => ctx.PSSetShaderResources(0, Some(std::slice::from_ref(&m.srv))),
        }
        ctx.DrawIndexed(count, start, 0);
    }
}

/// The grading table's texture: 16³ RGBA8, red (x) fastest, updated each frame the game sends one.
fn lut_texture(gpu: &Gpu) -> Result<(ID3D11Texture3D, ID3D11ShaderResourceView)> {
    let n = LUT_SIZE as u32;
    let desc = D3D11_TEXTURE3D_DESC {
        Width: n,
        Height: n,
        Depth: n,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    // Neutral until the game sends one (and unused while it hasn't).
    let neutral = vec![0x8080_8080u32; LUT_SIZE * LUT_SIZE * LUT_SIZE];
    let init = D3D11_SUBRESOURCE_DATA { pSysMem: neutral.as_ptr().cast(), SysMemPitch: n * 4, SysMemSlicePitch: n * n * 4 };
    unsafe {
        let mut tex = None;
        gpu.device.CreateTexture3D(&desc, Some(&init), Some(&mut tex))?;
        let tex = tex.context("no grading table")?;
        let mut srv = None;
        gpu.device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
        Ok((tex, srv.context("no grading table view")?))
    }
}

/// Clamped trilinear filtering: the game's `s_clamp_tri` / `s_clamp_bi` (environment cube, grading
/// table, light atlas).
fn clamp_sampler(gpu: &Gpu) -> Result<ID3D11SamplerState> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let mut s = None;
    unsafe { gpu.device.CreateSamplerState(&desc, Some(&mut s))? };
    s.context("no sampler")
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
