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
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP};
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
pub(crate) const DEPTH_BIAS: f32 = 0.0015;
pub(crate) const DEPTH_BIAS_PER_METER: f32 = 0.002;

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
    float4 plain;        // rgb, w: 1 for the plain look (one colour, no textures, a headlamp), 2 for one flat colour (testing)
    float4 post;         // x: the game's bloom and lens bound
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
    float4 atlas;              // x: light atlas bound, yz: 1 / its size, w: in the game's frame (HDR out)
    float4 fog_color;          // the game's fog towards the sun
    float4 fog_away;           // ... and away from it
    float4 fog_fade;           // amount = saturate(distance·y + x); 0: no fog
    float4 sun_view;           // towards the sun, in the game camera's view space
    MLight lights[32];
};
Texture2D albedo : register(t0);
Texture2D<float> game_depth : register(t1);
Texture2D bump_tex : register(t2);
Texture2D detail_tex : register(t3);
TextureCube env_tex : register(t4);
Texture3D grade_tex : register(t5);
Texture2D atlas_tex : register(t6);
Texture2D bloom_tex : register(t11);
Texture2D lens_tex : register(t12);
// A FaceGen head's texture morphs' texture and its detail map (Gamebryo's FaceGen blend).
Texture2D face_tex : register(t13);
Texture2D face_detail_tex : register(t14);
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
    if (plain.w > 1.5) return float4(plain.rgb, atlas.w > 0 ? i.view_z : 1);
    float3 base = tint.rgb;
    if (plain.w > 0) {
        // A mannequin: one colour, lit from the eye, edges darker.
        float3 pn = normalize(i.normal);
        float3 pv = normalize(eye_pos.xyz - i.world);
        float facing = abs(dot(pn, pv));
        return float4(plain.rgb * (0.35 + 0.65 * facing) * (front ? 1.0 : 0.5), 1);
    }
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

// Not a number or infinite (bit test: the compiler may fold isnan away).
bool m_bad(float3 c) { return any((asuint(c) & 0x7f800000) == 0x7f800000); }

// The game's post chain on an HDR colour at screen position svpos, as the value to write to the
// sRGB eye target so it stores the code value the game would have displayed. In the game's own
// frame, the HDR colour.
float3 finish(float3 hdr, float4 svpos) {
    // Drawn into the game's HDR picture before its post chain: the game tone maps and grades it.
    // Never anything but a finite colour: the game feeds its picture back into later frames (its
    // reflection and bloom read the last one), so one bad pixel would spread for good.
    if (atlas.w > 0) return m_bad(hdr) ? 0 : clamp(hdr, 0, 60000);
    // The game's bloom here: added before the tone curve, and an eighth of it after; and its lens's
    // light (a visor's sheen) after it (Metro's final pass; docs/research/metro-viewmodel-lighting.md).
    float3 b = 0, after = 0;
    if (post.x > 0) {
        float2 uv = svpos.xy / target_size.xy;
        b = max(bloom_tex.SampleLevel(clamp_samp, uv, 0).rgb, 0);
        after = max(lens_tex.SampleLevel(clamp_samp, uv, 0).rgb, 0);
    }
    float3 tm = m_tonemap(max(hdr + b, 0)) + 0.125 * b + after;
    float4 l = grade_tex.SampleLevel(clamp_samp, tm.bgr, 0);
    return m_display_to_linear(m_grade(tm, l, lit.w != 0));
}

// The game's fog at view-space point pv, as its combine fogs the world (docs/research/
// metro-viewmodel-lighting.md §5.1). (In the game's own frame, it fogs the player itself.)
float3 fogged(float3 c, float3 pv) {
    if (atlas.w > 0 || fog_fade.y == 0) return c;
    float d = length(pv);
    float f = saturate(d * fog_fade.y + fog_fade.x);
    float k = min(20 * f, 1);
    float g = sqrt(f) * k * k * (3 - 2 * k);
    float3 fc = lerp(fog_away.rgb, fog_color.rgb, saturate(dot(pv / max(d, 1e-4), sun_view.xyz) - 0.5));
    return lerp(c, fc, g);
}

// A gun's laser, as Metro 2033 Redux draws its own (`weapons\attaches\weapon_laser`): the beam
// is `static34\fx\fx_laser2_red`, two crossed planes 10 cm wide along it, texture `fx\fx_laser1`
// (its first quarter across them) under `effects\fx_laser` (added light, faded where a plane is
// seen edge on, the texture's dust sparkling in screen space, red from the mesh's colours); the
// dot is the spotlight `spotlights\spot_laser2_npc_red`, its falloff `light\light_radial_falloff1`,
// lying on what it hits. It ends at the first thing the game drew along it (the game's depth: a
// door, a monster, whatever is there), else where its collision or its range stops it.
cbuffer Laser : register(b4) {
    float4 laser_a;      // xyz: its start (the laser's mount)
    float4 laser_b;      // xyz: as far as it can go (its collision's hit, or its range)
    float4 laser_color;  // rgb: its light (HDR); w: seconds (the dust drifts)
    float4 laser_mode;   // x: 0 the beam, 1 the dot; y: the dot's radius; z: the game's textures
                         // bound; w: a pixel's width a metre away
    float4x4 laser_inv_vp;
};
Texture2D laser_tex : register(t13);
Texture2D laser_dot_tex : register(t14);
struct LaserOut {
    float4 pos : SV_Position;
    float3 world : TEXCOORD0;
    float2 uv : TEXCOORD1;
    float view_z : TEXCOORD2;
    float3 normal : TEXCOORD3;
};

// The game's depth (metres) where world point p shows, or 0 (off screen, nothing drawn).
float laser_depth_at(float3 p, out float z) {
    z = dot(view_z.xyz, p) + view_z.w;
    float4 c = mul(view_proj, float4(p, 1));
    if (depth.x == 0 || c.w <= 0) return 0;
    float2 ndc = c.xy / c.w;
    if (any(abs(ndc) > 1)) return 0;
    uint w, h;
    game_depth.GetDimensions(w, h);
    int2 px = int2(float2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * float2(w, h));
    return game_depth.Load(int3(min(px, int2(w, h) - 1), 0)) * depth.y;
}

// Whether p is in what the game drew (behind its surface there, but not by more than a surface's
// thickness: a beam passing behind a pillar hasn't hit it).
bool laser_inside(float3 p) {
    float z;
    float g = laser_depth_at(p, z);
    return g > 0 && z > g + 0.01 && z < g + 0.25 + 0.05 * g;
}

// Where the beam stops: the first thing the game drew along it.
float3 laser_end() {
    float3 a = laser_a.xyz, b = laser_b.xyz;
    float prev = 0;
    [loop] for (int k = 1; k <= 128; k++) {
        // (Closer together near the gun.)
        float t = k / 128.0;
        t *= t;
        if (laser_inside(lerp(a, b, t))) {
            float lo = prev, hi = t;
            [unroll] for (int r = 0; r < 6; r++) {
                float m = 0.5 * (lo + hi);
                if (laser_inside(lerp(a, b, m))) hi = m; else lo = m;
            }
            return lerp(a, b, hi);
        }
        prev = t;
    }
    return b;
}

// The game's surface at screen position ndc: the world point its depth puts there.
float3 laser_surface(float2 ndc, float g) {
    float4 w = mul(laser_inv_vp, float4(ndc, 0.5, 1));
    float3 d = normalize(w.xyz / w.w - eye_pos.xyz);
    return eye_pos.xyz + d * (g / max(dot(view_z.xyz, d), 1e-4));
}

LaserOut VSLaser(uint id : SV_VertexID, uint inst : SV_InstanceID) {
    float2 c = float2((id & 1) ? 1 : -1, (id & 2) ? 1 : 0);
    float3 a = laser_a.xyz;
    float3 end = laser_end();
    float3 dir = normalize(end - a);
    float3 side = normalize(cross(dir, abs(dir.y) < 0.95 ? float3(0, 1, 0) : float3(1, 0, 0)));
    float3 up = cross(side, dir);
    LaserOut o;
    float3 p;
    if (laser_mode.x < 0.5) {
        // The beam: the mesh's two crossed planes, 5 cm either side, along to where it stops;
        // across them u runs 0.25 to 0 (the texture's beam), along them v 0 to 1.
        float3 across = inst == 0 ? side : up;
        p = lerp(a, end, c.y) + across * 0.05 * c.x;
        o.uv = float2((0.5 - 0.5 * c.x) * 0.25, c.y);
        o.normal = inst == 0 ? up : side;
    } else {
        // The dot: on the surface where it stops (its normal from the game's depth around it),
        // a little off it towards the eye.
        float4 cc = mul(view_proj, float4(end, 1));
        float2 ndc = cc.xy / max(cc.w, 1e-4);
        float z;
        float g = laser_depth_at(end, z);
        float3 n = -dir;
        if (g > 0) {
            float2 px = 2.0 / target_size.xy * 3;
            float gx, gy;
            float3 e = laser_surface(ndc, g);
            float gx0 = laser_depth_at(laser_surface(ndc + float2(px.x, 0), g), z);
            float gy0 = laser_depth_at(laser_surface(ndc + float2(0, px.y), g), z);
            float3 ex = laser_surface(ndc + float2(px.x, 0), gx0 > 0 ? gx0 : g);
            float3 ey = laser_surface(ndc + float2(0, px.y), gy0 > 0 ? gy0 : g);
            float3 m = cross(ex - e, ey - e);
            if (dot(m, m) > 1e-12) n = normalize(m);
            if (dot(n, eye_pos.xyz - end) < 0) n = -n;
        }
        float3 t1 = normalize(cross(n, abs(n.y) < 0.95 ? float3(0, 1, 0) : float3(1, 0, 0)));
        float3 t2 = cross(n, t1);
        float r = max(laser_mode.y * (1 + 0.01 * length(end - a)), length(end - eye_pos.xyz) * laser_mode.w * 2);
        p = end + n * 0.01 + (t1 * c.x + t2 * (c.y * 2 - 1)) * r;
        o.uv = float2(c.x, c.y * 2 - 1);
        o.normal = n;
    }
    o.pos = mul(view_proj, float4(p, 1));
    o.world = p;
    o.view_z = dot(view_z.xyz, p) + view_z.w;
    return o;
}

float4 PSLaser(LaserOut i) : SV_Target {
    if (behind_game(i.pos, i.view_z)) discard;
    float3 view = normalize(eye_pos.xyz - i.world);
    float3 light;
    if (laser_mode.x < 0.5) {
        // Faded where the plane is seen edge on (the other plane shows there).
        float facing = saturate(abs(dot(normalize(i.normal), view)) * 1.5);
        float beam, dust = 1;
        if (laser_mode.z > 0) {
            beam = laser_tex.Sample(samp, i.uv).g;
            // Its dust, drifting, sparkling in screen space.
            float2 s = i.pos.xy / 512 + float2(0, laser_color.w * 0.03);
            dust = 0.55 + 0.9 * laser_tex.Sample(samp, s).r;
        } else {
            float x = (i.uv.x / 0.125) - 1;
            beam = saturate(1 - x * x);
        }
        light = laser_color.rgb * beam * facing * dust;
    } else {
        float spot = laser_mode.z > 0 ? laser_dot_tex.Sample(clamp_samp, i.uv * 0.5 + 0.5).r : saturate(1 - dot(i.uv, i.uv));
        light = laser_color.rgb * spot * 4;
    }
    // Its light fades into the game's fog as anything there would.
    float d = length(i.world - eye_pos.xyz);
    if (atlas.w == 0 && fog_fade.y != 0) {
        float f = saturate(d * fog_fade.y + fog_fade.x);
        float k = min(20 * f, 1);
        light *= 1 - sqrt(f) * k * k * (3 - 2 * k);
    }
    // The game's tone curve and grading on the light alone (it's added to what's there).
    if (atlas.w > 0) return float4(light, 0);
    float3 tm = m_tonemap(max(light, 0));
    float4 l = grade_tex.SampleLevel(clamp_samp, saturate(tm).bgr, 0);
    return float4(max(m_display_to_linear(m_grade(tm, l, lit.w != 0)), 0), 0);
}

// What an opaque fragment writes in alpha: in the game's HDR picture its view depth (metres), as
// the game's combine pass writes it for its later passes; else opaque.
float out_alpha(float view_z) { return atlas.w > 0 ? view_z : 1; }

// The game's own shading of its first-person model (metro.rs), evaluated for this fragment.
float4 PSMetro(VSOut i, bool front : SV_IsFrontFace) : SV_Target {
    float3 pv = mul(game_view, float4(i.world, 1)).xyz;
    float3 nv = mul((float3x3)game_view, i.normal);
    float3 dpx = ddx_coarse(pv), dpy = ddy_coarse(pv);
    float2 dux = ddx_coarse(i.uv), duy = ddy_coarse(i.uv);
    if (i.vis < 0) discard;
    if (behind_game(i.pos, i.view_z)) discard;
    if (plain.w > 1.5) return float4(plain.rgb, out_alpha(i.view_z));
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
    if (!front) return float4(finish(fogged(x.base.rgb * amb.rgb * 0.3, pv), i.pos), out_alpha(i.view_z));
    MG g = m_gbuffer(cls, f, n, x, surf_color.rgb, material.w, options.xy, lit.y, envc.rgb, up.xyz);
    float3 d = 0, s = 0;
    if (!g.emissive) {
        uint count = (uint)envc.w;
        for (uint k = 0; k < count; k++) {
            if (!m_reaches(pv, lights[k])) continue;
            float3 ld, ls;
            m_light(g, lights[k], atlas_term(lights[k], g.p), ld, ls);
            // (A light the game's own shader would give nothing finite here adds nothing.)
            if (m_bad(ld) || m_bad(ls)) continue;
            d += ld;
            s += ls;
        }
    }
    return float4(finish(fogged(m_combine(g, d, s, sh_r, sh_g, sh_b, amb, lit.z, m_local_ao_r(d), 0), pv), i.pos), out_alpha(i.view_z));
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
    return float4(finish(fogged(c.rgb, pv), i.pos), m_bad(c.aaa) ? 0 : saturate(c.a));
}

// Gamebryo's per-pixel lighting (gamebryo.rs: Oblivion's ShadowLightShader and SkinShader), in the
// display's range as the game's back buffer holds it, written through the sRGB target. The game's
// textures raw; the lights and the eye in tracking space (the lighting's `eye` is the identity).
float4 PSGamebryo(VSOut i, bool front : SV_IsFrontFace) : SV_Target {
    float3 dpx = ddx(i.world), dpy = ddy(i.world);
    float2 dux = ddx(i.uv), duy = ddy(i.uv);
    if (i.vis < 0) discard;
    if (behind_game(i.pos, i.view_z)) discard;
    if (plain.w > 1.5) return float4(plain.rgb, 1);
    float4 base = albedo.Sample(samp, i.uv);
    if (material.y > 0 && base.a < 0.5) discard;
    // A FaceGen head (the skin shader's FaceGen blend, gamebryo.rs `face`): its texture morphs move
    // the base colour, under the detail map (a 4x modulate; without one, neutral).
    if (options.z > 0) {
        float3 morphs = face_tex.Sample(samp, i.uv).rgb;
        float3 detail = options.z > 1.5 ? face_detail_tex.Sample(samp, i.uv).rgb : 0.25;
        base.rgb = 4 * detail * (base.rgb + 2 * (morphs - 0.5));
    }
    bool hair = (uint)material.z == GB_HAIR;
    // The hair's layer map: its strands over the base, by their alpha.
    if (hair && options.y > 0) {
        float4 layer = detail_tex.Sample(samp, i.uv);
        base.rgb = lerp(base.rgb, layer.rgb, layer.a);
    }
    // The base times the vertex colour (the game's are grey: the vertex's AO byte holds it); the
    // hair's vertex colour is its tint's mask instead (gamebryo.rs `hair_tint`), its tint the
    // wearer's hair colour.
    float3 c = hair ? base.rgb * 2 * (0.5 + i.ao * (surf_color.rgb - 0.5)) : base.rgb * i.ao;
    // The inside of the shell, seen through a cut (the game culls it): the ambient, dark.
    if (!front) return float4(m_display_to_linear(c * amb.rgb * 0.3), 1);
    float3 nv = normalize(i.normal);
    float4 nm = options.x > 0 ? bump_tex.Sample(samp, i.uv) : float4(0.5, 0.5, 1, 0);
    float3 t, b;
    gb_frame(dpx, dpy, dux, duy, nv, t, b);
    float3 nt = normalize(nm.rgb * 2 - 1);
    float3 n = normalize(nt.x * t + nt.y * b + nt.z * nv);
    float3 v = normalize(eye_pos.xyz - i.world);
    float3 glow = options.y > 0 && !hair ? detail_tex.Sample(samp, i.uv).rgb * surf_color.rgb : 0;
    bool skin = (uint)material.z == GB_SKIN;
    float gloss = surf_params.y > 0 ? surf_params.x : 0;
    float3 d = 0, s = 0;
    uint count = (uint)envc.w;
    for (uint k = 0; k < count; k++) gb_light(lights[k], i.world, n, v, skin, hair, gloss, nm.a, d, s);
    // (The hair's light isn't capped: its shader adds the ambient to its lights as they are.)
    float3 col = saturate(c * (hair ? amb.rgb + d : saturate(amb.rgb + glow + d)) + s);
    if (fog_fade.y != 0) col = lerp(col, fog_color.rgb, saturate(length(eye_pos.xyz - i.world) * fog_fade.y + fog_fade.x));
    return float4(m_display_to_linear(col), 1);
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

// ---- The player the game drew inside its own frame, moved to where it is now
// (`Renderer::draw_view_depth`, `Renderer::draw_reprojected`).
cbuffer BonesThen : register(b4) {
    float4 was_rows[MAX_BONES * 3];   // where the bones were in the game's pictures, 3x4 rows
    float4 was_vis[MAX_BONES];
};
cbuffer Sources : register(b5) {
    float4x4 src_view_proj[2];   // tracking space -> each picture's clip space
    float4 src_view_z[2];        // its view z = dot(xyz, p) + w
    float4 src_info;             // x: pictures (1 or 2), y: depth tolerance (metres), z: per metre, w: 1 to tint by where each colour came from
};
Texture2D src_color0 : register(t7);
Texture2D src_color1 : register(t8);
Texture2D<float> src_player0 : register(t9);    // the player's view z where it shows (0: not there)
Texture2D<float> src_player1 : register(t10);

// The view depth (metres) of the player where it shows.
float PSViewZ(VSOut i) : SV_Target {
    if (i.vis < 0) discard;
    if (behind_game(i.pos, i.view_z)) discard;
    if (material.x > 0 && material.y > 0 && albedo.Sample(samp, i.uv).a < 0.5) discard;
    return i.view_z;
}

struct VSRepOut {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float view_z : TEXCOORD1;
    float vis : TEXCOORD2;
    float4 clip0 : TEXCOORD3;
    float4 clip1 : TEXCOORD4;
    float2 was_z : TEXCOORD5;
};

// Skinned twice: where the vertex is now (drawn there) and where it was in the game's pictures
// (where its colour is in them).
VSRepOut VSReproject(VSIn i) {
    float4 w = i.weights / max(dot(i.weights, 1), 1e-5);
    float4 r0 = 0, r1 = 0, r2 = 0, t0 = 0, t1 = 0, t2 = 0;
    float vis = 0;
    [unroll] for (int k = 0; k < 4; k++) {
        uint b = i.bones[k];
        r0 += bone_rows[b * 3 + 0] * w[k];
        r1 += bone_rows[b * 3 + 1] * w[k];
        r2 += bone_rows[b * 3 + 2] * w[k];
        t0 += was_rows[b * 3 + 0] * w[k];
        t1 += was_rows[b * 3 + 1] * w[k];
        t2 += was_rows[b * 3 + 2] * w[k];
        vis += w[k] * (dot(vis_planes[b].xyz, i.pos) + vis_planes[b].w);
    }
    float4 p = float4(i.pos, 1);
    float3 now = float3(dot(r0, p), dot(r1, p), dot(r2, p));
    float4 was = float4(dot(t0, p), dot(t1, p), dot(t2, p), 1);
    VSRepOut o;
    o.pos = mul(view_proj, float4(now, 1));
    o.uv = i.uv;
    o.view_z = dot(view_z.xyz, now) + view_z.w;
    o.vis = vis;
    o.clip0 = mul(src_view_proj[0], was);
    o.clip1 = mul(src_view_proj[1], was);
    o.was_z = float2(dot(src_view_z[0], was), dot(src_view_z[1], was));
    return o;
}

float2 clip_uv(float4 c) {
    float2 n = c.xy / c.w;
    return float2(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5);
}

// This point's colour in a picture the game drew it in, if the player shows this point there: its
// depth within what the player's pixels around its place span (a surface seen edge-on changes
// depth a lot from one pixel to the next). Blended from the four pixels around it that are the
// player's only: at the player's outline the world beside it doesn't bleed in (it moves on
// differently).
bool from_source(Texture2D col, Texture2D<float> pl, float4 clip, float z, out float3 c) {
    c = 0;
    if (clip.w <= 1e-4) return false;
    float2 uv = clip_uv(clip);
    if (any(uv < 0) || any(uv > 1)) return false;
    uint w, h;
    pl.GetDimensions(w, h);
    int2 at = int2(uv * float2(w, h));
    float lo = 1e30, hi = 0;
    [unroll] for (int k = 0; k < 9; k++) {
        float d = pl.Load(int3(clamp(at + int2(k % 3 - 1, k / 3 - 1), 0, int2(w - 1, h - 1)), 0));
        if (d > 0) {
            lo = min(lo, d);
            hi = max(hi, d);
        }
    }
    float tol = src_info.y + src_info.z * z;
    if (!(hi > 0) || z < lo - tol || z > hi + tol) return false;
    float2 t = uv * float2(w, h) - 0.5;
    int2 base = int2(floor(t));
    float2 f = t - base;
    float3 sum = 0;
    float weight = 0;
    [unroll] for (int k = 0; k < 4; k++) {
        int2 o = int2(k & 1, k >> 1);
        int2 p = clamp(base + o, 0, int2(w - 1, h - 1));
        float d = pl.Load(int3(p, 0));
        float wk = (o.x ? f.x : 1 - f.x) * (o.y ? f.y : 1 - f.y);
        if (d > 0 && abs(d - z) <= 3 * tol) {
            sum += col.Load(int3(p, 0)).rgb * wk;
            weight += wk;
        }
    }
    c = weight > 1e-4 ? sum / weight : col.SampleLevel(clamp_samp, uv, 0).rgb;
    return true;
}

// For a point neither picture shows (a side turned into view since, or behind the other hand
// then): the colour of the nearest of the player's pixels at about its depth in the picture.
bool near_source(Texture2D col, Texture2D<float> pl, float4 clip, float z, out float3 c) {
    c = 0;
    if (clip.w <= 1e-4) return false;
    uint w, h;
    pl.GetDimensions(w, h);
    int2 at = int2(saturate(clip_uv(clip)) * float2(w, h));
    float tol = 4 * (src_info.y + src_info.z * z);
    float best = 1e9;
    [loop] for (int y = -5; y <= 5; y++) {
        [loop] for (int x = -5; x <= 5; x++) {
            int2 p = clamp(at + int2(x, y), 0, int2(w - 1, h - 1));
            float d = pl.Load(int3(p, 0));
            float r = x * x + y * y;
            if (d > 0 && abs(d - z) < tol && r < best) {
                best = r;
                c = col.Load(int3(p, 0)).rgb;
            }
        }
    }
    return best < 1e9;
}

// The player as the game drew it (its light, its fog, its tone curve and grading: the game's own
// pixels), where the player is now.
float4 PSReproject(VSRepOut i) : SV_Target {
    if (i.vis < 0) discard;
    if (behind_game(i.pos, i.view_z)) discard;
    if (material.x > 0 && material.y > 0 && albedo.Sample(samp, i.uv).a < 0.5) discard;
    float3 c;
    // (Debugging, `src_info.w`: the first picture's colours reddened, the second's greened, and
    // blue where neither shows the point.)
    bool tint = src_info.w > 0;
    if (from_source(src_color0, src_player0, i.clip0, i.was_z.x, c)) return float4(tint ? c * float3(1, 0.35, 0.35) + float3(0.15, 0, 0) : c, 1);
    if (src_info.x > 1 && from_source(src_color1, src_player1, i.clip1, i.was_z.y, c)) return float4(tint ? c * float3(0.35, 1, 0.35) + float3(0, 0.15, 0) : c, 1);
    if (tint) return float4(0, 0, 0.6, 1);
    // Hidden in the pictures: the nearest of the player's pixels around where it would be.
    if (near_source(src_color0, src_player0, i.clip0, i.was_z.x, c)) return float4(c, 1);
    if (src_info.x > 1 && near_source(src_color1, src_player1, i.clip1, i.was_z.y, c)) return float4(c, 1);
    return float4(src_color0.SampleLevel(clamp_samp, saturate(clip_uv(i.clip0)), 0).rgb, 1);
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
struct LaserCb {
    a: [f32; 4],
    b: [f32; 4],
    color: [f32; 4],
    mode: [f32; 4],
    inv_vp: [f32; 16],
}

/// The laser's light (HDR): Metro's red (the beam mesh's vertex colours are pure red).
const LASER_RED: [f32; 3] = [1.6, 0.0, 0.0];
/// The dot's radius (m) close up (the spotlight's spot), growing 1% a metre out.
const LASER_DOT_RADIUS: f32 = 0.012;
/// The game's textures for it (`metro-extract --extras`), in the asset cache's `metro2033`.
const LASER_TEXTURES: [&str; 2] = ["fx__fx_laser1", "light__light_radial_falloff1"];

/// The player's layer over the game's picture, through the game's lens: at each pixel, the layer
/// where the lens takes the picture from (its warp: the bloom and lens textures' alphas).
const THROUGH_LENS: &str = r#"
Texture2D layer : register(t0);
Texture2D bloom_tex : register(t1);
Texture2D lens_tex : register(t2);
SamplerState lin : register(s0);
cbuffer C : register(b0) { float4 size; };
float4 VSMain(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}
float4 PSMain(float4 p : SV_Position) : SV_Target {
    float2 uv = p.xy / size.xy;
    float2 warp = float2(bloom_tex.SampleLevel(lin, uv, 0).a, lens_tex.SampleLevel(lin, uv, 0).a);
    return layer.SampleLevel(lin, uv + warp, 0);
}
"#;

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
    plain: [f32; 4],
    post: [f32; 4],
}

/// The plain look's colour (linear): a light grey-blue mannequin.
const PLAIN: [f32; 3] = [0.62, 0.66, 0.74];

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

/// The pictures the game drew the player in, as the shader's `Sources` buffer holds them.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SourcesCb {
    view_proj: [[f32; 16]; 2],
    view_z: [[f32; 4]; 2],
    info: [f32; 4],
}

/// How far (metres, and per metre of distance) a point's depth may be from the depth of the player
/// a picture shows at its place and still be that point: rasterization differences, well under a
/// finger's thickness.
const SOURCE_BIAS: f32 = 0.006;
const SOURCE_BIAS_PER_METER: f32 = 0.005;

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
    fog_color: [f32; 4],
    fog_away: [f32; 4],
    fog_fade: [f32; 4],
    sun_view: [f32; 4],
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
/// textures are UNORM, so their numbers are used as linear values), and a FaceGen head's two
/// (t13, t14).
struct MetroMesh {
    class: Class,
    base: ID3D11ShaderResourceView,
    bump: Option<ID3D11ShaderResourceView>,
    detail: Option<ID3D11ShaderResourceView>,
    env: Option<ID3D11ShaderResourceView>,
    face: Option<ID3D11ShaderResourceView>,
    face_detail: Option<ID3D11ShaderResourceView>,
}

/// A texture's two views: sRGB-decoded (the runtime's own look) and raw (the game's).
#[derive(Clone)]
pub(crate) struct GpuTex {
    pub(crate) srgb: ID3D11ShaderResourceView,
    pub(crate) linear: ID3D11ShaderResourceView,
}

/// One drawable model: its rig, GPU meshes and palettes.
pub struct Character {
    pub(crate) rig: Rig,
    posed: Posed,
    meshes: Vec<GpuMesh>,
    /// Visibility planes for each [`Show`]: full, arms, hands, left hand, right hand.
    vis: [Vec<[f32; 4]>; 5],
    palette: Box<BonesCb>,
    body_cb: ConstantBuffer<BonesCb>,
    /// Where the bones were in the game's pictures ([`Renderer::draw_reprojected`]).
    then_cb: ConstantBuffer<BonesCb>,
    ghost_cbs: [ConstantBuffer<BonesCb>; 2],
    /// The hand poses the last [`Character::pose`] drew, for re-rooting the ghosts.
    drawn_hands: [Option<Pose>; 2],
    /// The skin on the CPU (bind positions and weights) and the bones' names, for measuring.
    cpu: Vec<Vec<asset::Vertex>>,
    bone_names: Vec<String>,
    /// Where the eyes look, when they look at something (tracking space; [`crate::Avatar::set_gaze`]).
    pub(crate) gaze: Option<Vec3>,
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
            meshes.push(gpu_mesh(gpu, m, &indices, ranges, &mut upload)?);
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
            then_cb: ConstantBuffer::new(gpu)?,
            ghost_cbs: [ConstantBuffer::new(gpu)?, ConstantBuffer::new(gpu)?],
            drawn_hands: [None; 2],
            gaze: None,
        })
    }

    /// Poses the rig for this frame.
    fn pose(&mut self, input: &PoseInput) {
        self.rig.pose(input, &mut self.posed);
        if let Some(g) = self.gaze.filter(|_| !input.chop_head) {
            self.rig.aim_eyes(&mut self.posed, g);
        }
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

    /// Poses the rig for `input` and uploads that palette as where the bones were
    /// ([`Renderer::draw_reprojected`]). Pose the rig for now after it.
    fn upload_then(&mut self, ctx: &ID3D11DeviceContext, input: &PoseInput, show: Show) {
        self.rig.pose(input, &mut self.posed);
        self.fill_palette(Mat4::IDENTITY, show);
        self.then_cb.update(ctx, &self.palette);
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

/// One mesh on the GPU: its vertices, `indices` (the views' `ranges` in them), its base texture
/// and material, and the game's material when it has one with all its textures.
fn gpu_mesh(gpu: &Gpu, m: &crate::SourceMesh, indices: &[u32], ranges: [(u32, u32); 3], upload: &mut impl FnMut(&Arc<asset::Texture>, &str) -> Option<GpuTex>) -> Result<GpuMesh> {
    let vertices: Vec<Vertex> =
        m.vertices.iter().map(|v| Vertex { pos: v.pos, normal: v.normal, uv: v.uv, bones: v.bones, weights: v.weights, ao: [v.ao, 0, 0, 0] }).collect();
    let base = m.texture.as_ref().and_then(|t| upload(t, &m.name));
    // The game's own flag where it has one, else a guess from the texture.
    let cutout = base.is_some() && m.material.as_ref().map_or_else(|| m.texture.as_deref().is_some_and(is_cutout), |mat| mat.alpha_test);
    let metro = match (&m.material, &base) {
        (Some(mat), Some(base)) => {
            let mut get = |t: &Option<Arc<asset::Texture>>| t.as_ref().and_then(|t| upload(t, &m.name)).map(|t| t.linear);
            let (bump, detail, env) = (get(&mat.bump), get(&mat.detail), get(&mat.env));
            let (face, face_detail) = (get(&mat.face), get(&mat.face_detail));
            let (nb, nd, ne) = mat.class.needs();
            ((!nb || bump.is_some()) && (!nd || detail.is_some()) && (!ne || env.is_some())).then(|| MetroMesh { class: mat.class, base: base.linear.clone(), bump, detail, env, face, face_detail })
        }
        _ => None,
    };
    let (surf_params, mut surf_color, emissive) = m.material.as_ref().map_or(([0.0; 4], [0.0; 4], 0.0), |mat| (mat.params, mat.color, mat.emissive));
    // (The hair's colour is its wearer's.)
    if let Some(t) = m.material.as_ref().and_then(|mat| mat.hair_tint) {
        surf_color = [t[0], t[1], t[2], 1.0];
    }
    let options = m.material.as_ref().map_or([0.0; 4], |mat| {
        if mat.class.gamebryo() {
            // Gamebryo's: whether it has a normal map and a glow map, and a FaceGen head's
            // textures (1: its texture morphs, 2: and its detail map).
            let face = if mat.face.is_none() { 0 } else if mat.face_detail.is_none() { 1 } else { 2 };
            [mat.bump.is_some() as u32 as f32, mat.detail.is_some() as u32 as f32, face as f32, 0.0]
        } else {
            [mat.env.is_some() as u32 as f32, mat.fresnel as u32 as f32, 0.0, 0.0]
        }
    });
    let class = metro.as_ref().map_or(0.0, |mm| mm.class as u32 as f32);
    let material = MaterialCb {
        tint: [m.tint[0], m.tint[1], m.tint[2], 1.0],
        params: [base.is_some() as u32 as f32, cutout as u32 as f32, class, emissive],
        surf_params,
        surf_color,
        options,
    };
    let srv = base.map(|b| b.srgb);
    Ok(GpuMesh {
        vb: static_buffer(gpu, bytemuck::cast_slice(&vertices), D3D11_BIND_VERTEX_BUFFER)?,
        ib: static_buffer(gpu, bytemuck::cast_slice(indices), D3D11_BIND_INDEX_BUFFER)?,
        ranges,
        srv,
        material: static_buffer(gpu, bytemuck::bytes_of(&material), D3D11_BIND_CONSTANT_BUFFER)?,
        metro,
    })
}

/// A rigid model in parts that move on their own (a gun: its body, magazine, bolt, trigger,
/// rounds), drawn with the body: each part is one "bone" of the palette, placed where the frame
/// says ([`crate::HeldDraw`]) or hidden.
pub struct HeldModel {
    meshes: Vec<GpuMesh>,
    info: Vec<crate::HeldMeshInfo>,
    parts: usize,
    palette: std::cell::RefCell<Box<BonesCb>>,
    cb: ConstantBuffer<BonesCb>,
    /// The parts where they were in the game's pictures ([`Renderer::draw_reprojected`]).
    then_cb: ConstantBuffer<BonesCb>,
}

impl HeldModel {
    /// `meshes`: each with the part it belongs to (its vertices' bones are ignored) and whether a
    /// special draw takes it instead (see [`crate::SpecialDraw`]).
    pub(crate) fn new(gpu: &Gpu, meshes: Vec<(crate::SourceMesh, crate::HeldMeshInfo)>) -> Result<HeldModel> {
        let parts = meshes.iter().map(|(_, i)| i.part + 1).max().unwrap_or(0);
        anyhow::ensure!(parts <= MAX_BONES, "{parts} parts (at most {MAX_BONES})");
        let mut uploaded: HashMap<*const asset::Texture, Option<GpuTex>> = HashMap::new();
        let mut upload = |t: &Arc<asset::Texture>, what: &str| {
            uploaded
                .entry(Arc::as_ptr(t))
                .or_insert_with(|| upload_texture(gpu, t).map_err(|e| tracing::warn!("avatar: texture of {what}: {e:#}")).ok())
                .clone()
        };
        let mut out = Vec::new();
        let mut info = Vec::new();
        for (mut m, i) in meshes.into_iter().filter(|(m, _)| !m.indices.is_empty()) {
            for v in &mut m.vertices {
                v.bones = [i.part as u8, 0, 0, 0];
                v.weights = [255, 0, 0, 0];
            }
            fix_winding(&mut m);
            let n = m.indices.len() as u32;
            out.push(gpu_mesh(gpu, &m, &m.indices, [(0, n); 3], &mut upload)?);
            info.push(i);
        }
        Ok(HeldModel { meshes: out, info, parts, palette: std::cell::RefCell::new(Box::new(BonesCb::zeroed())), cb: ConstantBuffer::new(gpu)?, then_cb: ConstantBuffer::new(gpu)? })
    }

    /// Its meshes: which part each is, where it came from, and whether a special draw takes it.
    pub fn meshes(&self) -> &[crate::HeldMeshInfo] {
        &self.info
    }

    /// How many parts it has (a [`crate::HeldDraw`] gives each a place).
    pub fn parts(&self) -> usize {
        self.parts
    }

    /// Uploads the parts' places (tracking space; None or missing: hidden).
    fn upload(&self, ctx: &ID3D11DeviceContext, parts: &[Option<Mat4>]) {
        self.upload_to(ctx, parts, &self.cb);
    }

    /// Uploads where the parts were in the game's pictures ([`Renderer::draw_reprojected`]).
    fn upload_then(&self, ctx: &ID3D11DeviceContext, parts: &[Option<Mat4>]) {
        self.upload_to(ctx, parts, &self.then_cb);
    }

    fn upload_to(&self, ctx: &ID3D11DeviceContext, parts: &[Option<Mat4>], cb: &ConstantBuffer<BonesCb>) {
        let mut palette = self.palette.borrow_mut();
        for p in 0..self.parts {
            let (m, vis) = match parts.get(p).copied().flatten() {
                Some(m) => (m, [0.0, 0.0, 0.0, 1.0]),
                None => (Mat4::IDENTITY, [0.0, 0.0, 0.0, -1.0]),
            };
            palette.rows[p] = [m.row(0).to_array(), m.row(1).to_array(), m.row(2).to_array()];
            palette.vis[p] = vis;
        }
        cb.update(ctx, &palette);
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
pub(crate) fn upload_texture(gpu: &Gpu, t: &asset::Texture) -> Result<GpuTex> {
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
    gamebryo: Program,
    /// The player's view depth ([`Renderer::draw_view_depth`]) and the game's own pixels of the
    /// player moved to where it is now ([`Renderer::draw_reprojected`]).
    view_z: Program,
    reproject: Program,
    sources_cb: ConstantBuffer<SourcesCb>,
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
    depth: Vec<Texture>,
    /// Drawing into the game's HDR picture, before its post chain ([`Renderer::set_in_frame`]).
    in_frame: bool,
    /// Testing: the body, and what it holds, each in one flat colour ([`Renderer::set_flat`]).
    flat: Option<([f32; 3], [f32; 3])>,
    /// The held gun's laser ([`Renderer::set_laser`]), its pass and light added over what's there,
    /// the game's textures for it (loaded with the first laser; None inside: not there), and the
    /// clock its dust drifts by.
    laser: Option<crate::LaserBeam>,
    laser_textures: Option<Option<[ID3D11ShaderResourceView; 2]>>,
    laser_clock: std::time::Instant,
    laser_program: Program,
    laser_cb: ConstantBuffer<LaserCb>,
    additive: ID3D11BlendState,
    /// The player drawn on its own, laid over the picture through the game's lens
    /// ([`Renderer::draw`]): the layer (kept until the size or format changes), and the pass.
    layer: Option<Texture>,
    through_lens: Program,
    lens_cb: ConstantBuffer<[f32; 4]>,
    premultiplied: ID3D11BlendState,
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
        let source = format!("{}{}{SHADER}", metro::HLSL, crate::gamebryo::HLSL);
        let (lut, lut_srv) = lut_texture(gpu)?;
        Ok(Renderer {
            opaque: Program::new(gpu, &source, "avatar", &layout)?,
            ghost: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSGhost", &layout)?,
            metro: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSMetro", &layout)?,
            metro_glass: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSMetroGlass", &layout)?,
            gamebryo: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSGamebryo", &layout)?,
            view_z: Program::with_entries(gpu, &source, "avatar", "VSMain", "PSViewZ", &layout)?,
            reproject: Program::with_entries(gpu, &source, "avatar", "VSReproject", "PSReproject", &layout)?,
            sources_cb: ConstantBuffer::new(gpu)?,
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
            depth: Vec::new(),
            in_frame: false,
            flat: None,
            laser: None,
            laser_textures: None,
            laser_clock: std::time::Instant::now(),
            laser_program: Program::with_entries(gpu, &source, "avatar", "VSLaser", "PSLaser", &[])?,
            laser_cb: ConstantBuffer::new(gpu)?,
            additive: additive_blend(gpu)?,
            layer: None,
            through_lens: Program::new(gpu, THROUGH_LENS, "avatar_lens", &[])?,
            lens_cb: ConstantBuffer::new(gpu)?,
            premultiplied: pipeline::blend_state(gpu, Blend::Premultiplied)?,
        })
    }

    /// The held gun's laser from now on (None: none): drawn with the player.
    pub fn set_laser(&mut self, gpu: &Gpu, laser: Option<crate::LaserBeam>) {
        if laser.is_some() && self.laser_textures.is_none() {
            let dir = asset::default_root().join("metro2033");
            let load = |k: &str| asset::load_texture(&dir, k).and_then(|t| Ok(upload_texture(gpu, &t)?.linear));
            self.laser_textures = Some(match (load(LASER_TEXTURES[0]), load(LASER_TEXTURES[1])) {
                (Ok(beam), Ok(dot)) => Some([beam, dot]),
                (a, b) => {
                    tracing::warn!("avatar: the laser's textures ({:?}, {:?}): drawn without them", a.err(), b.err());
                    None
                }
            });
        }
        self.laser = laser;
    }

    /// Draw into the game's own HDR picture, before its post chain (an adapter drawing the player in
    /// the game's frame): the game's shading writes HDR colour and view depth (alpha), and the game
    /// tone maps and grades it. Takes effect at the next [`Renderer::set_shading`].
    pub fn set_in_frame(&mut self, in_frame: bool) {
        self.in_frame = in_frame;
    }

    /// Testing: draw the body in one flat colour and what it holds in another (linear, HDR in the
    /// game's frame), so a picture of them can be found exactly in each frame; None: as they are.
    pub fn set_flat(&mut self, colors: Option<([f32; 3], [f32; 3])>) {
        self.flat = colors;
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
            atlas: [self.atlas.is_some() as u32 as f32, 1.0 / LIGHT_ATLAS_W as f32, 1.0 / LIGHT_ATLAS_H as f32, self.in_frame as u32 as f32],
            fog_color: l.fog_color,
            fog_away: l.fog_color_away,
            fog_fade: l.fog_fade,
            sun_view: l.sun_dir,
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
        draw_body: bool,
        held: &[crate::HeldDraw],
        special: Option<&mut dyn FnMut(&crate::SpecialDraw)>,
    ) -> Result<()> {
        // The game's lens bends its picture (a gas mask's visor, heat haze): the player, drawn on
        // its own, is laid over it bent the same way.
        let lens = depth.as_ref().and_then(|d| Some((d.bloom?.clone(), d.lens?.clone())));
        let Some((bloom, lens)) = lens.filter(|_| !self.in_frame) else {
            return self.draw_into(gpu, target, width, height, eye, fov, frame, depth, body, arms, draw_body, held, special);
        };
        let (rtv, srv) = self.layer_for(gpu, target, width, height)?;
        unsafe { gpu.ctx.ClearRenderTargetView(&rtv, &[0.0; 4]) };
        // (The special draws, a collimator's lens, look through to what's behind them: the eye's
        // picture, once the player is laid over it.)
        let game_depth = depth.as_ref().map(|d| (d.srv.clone(), d.units_per_meter));
        self.draw_into(gpu, &rtv, width, height, eye, fov, frame, depth, body, arms, draw_body, held, None)?;
        let ctx = &gpu.ctx;
        self.lens_cb.update(ctx, &[width as f32, height as f32, 0.0, 0.0]);
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            pipeline::viewport(ctx, width, height);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.through_lens.bind(ctx);
            ctx.PSSetShaderResources(0, Some(&[Some(srv), Some(bloom), Some(lens)]));
            ctx.PSSetSamplers(0, Some(&[Some(self.clamp_sampler.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.lens_cb.buf.clone())]));
            ctx.OMSetBlendState(&self.premultiplied, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(None, 0);
            ctx.RSSetState(None);
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None, None, None]));
            ctx.OMSetRenderTargets(None, None);
        }
        // What the special draws take, over the eye's picture with the player on it, in the player's
        // depth (the gun's body hides a collimator's lens where it's in front).
        if let Some(special) = special {
            let dsv = self.depth_for(gpu, width, height)?;
            unsafe { ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), &dsv) };
            self.bind_common(ctx, game_depth.as_ref().map(|d| &d.0), width, height);
            self.specials(gpu, held, special, eye, fov, width, height, game_depth.as_ref().map(|d| (&d.0, d.1)));
            unsafe {
                ctx.PSSetShaderResources(0, Some(&[None, None, None, None, None, None, None]));
                ctx.OMSetRenderTargets(None, None);
            }
        }
        Ok(())
    }

    /// The special draws of what the hands hold (a collimator's lens, `special`), into what's bound.
    #[allow(clippy::too_many_arguments)]
    fn specials(&self, gpu: &Gpu, held: &[crate::HeldDraw], special: &mut dyn FnMut(&crate::SpecialDraw), eye: &Pose, fov: &Fov, width: u32, height: u32, game_depth: Option<(&ID3D11ShaderResourceView, f32)>) {
        let view_proj = projection(fov) * view_matrix(eye);
        for h in held {
            for info in h.model.info.iter().filter(|i| i.special) {
                let Some(model) = h.parts.get(info.part).copied().flatten() else { continue };
                special(&crate::SpecialDraw { gpu, key: h.key, mesh: info, model, view_proj, near_far: (NEAR, FAR), eye, fov, width, height, game_depth, shading: self.shading });
            }
        }
    }

    /// The layer the player is drawn into for [`Renderer::draw`] through the game's lens: the
    /// target's size and format, transparent where the player isn't (premultiplied). Its views.
    fn layer_for(&mut self, gpu: &Gpu, target: &ID3D11RenderTargetView, width: u32, height: u32) -> Result<(ID3D11RenderTargetView, ID3D11ShaderResourceView)> {
        let mut d = D3D11_RENDER_TARGET_VIEW_DESC::default();
        unsafe { target.GetDesc(&mut d) };
        let fits = |t: &Texture| t.width == width && t.height == height && t.format == d.Format;
        if !self.layer.as_ref().is_some_and(fits) {
            self.layer = Some(Texture::new(gpu, &TexDesc::target(width, height, d.Format))?);
        }
        let layer = self.layer.as_ref().unwrap();
        Ok((layer.rtv.clone().context("layer view")?, layer.srv.clone().context("layer view")?))
    }

    /// [`Renderer::draw`] straight into `target`.
    fn draw_into(
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
        draw_body: bool,
        held: &[crate::HeldDraw],
        mut special: Option<&mut dyn FnMut(&crate::SpecialDraw)>,
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

        let mut cb = frame_constants(width, height, eye, fov, frame, depth.as_ref());
        if let Some((c, _)) = self.flat {
            cb.plain = [c[0], c[1], c[2], 2.0];
        }
        self.frame_cb.update(ctx, &cb);
        let dsv = self.depth_for(gpu, width, height)?;
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), &dsv);
            ctx.ClearDepthStencilView(&dsv, D3D11_CLEAR_DEPTH.0, 1.0, 0);
        }
        let game_depth = depth.as_ref().map(|d| d.srv.clone());
        let post = depth.as_ref().map_or([None, None], |d| [d.bloom.cloned(), d.lens.cloned()]);
        self.bind_common(ctx, game_depth.as_ref(), width, height);
        unsafe {
            ctx.PSSetShaderResources(11, Some(&post));
            ctx.VSSetConstantBuffers(1, Some(&[Some(main.body_cb.buf.clone())]));
        }
        let range = match show {
            Show::Full => ALL,
            Show::Arms => ARMS,
            _ => HANDS,
        };
        if draw_body {
            self.draw_body(ctx, main, range);
        }

        // What the hands hold, in the same depth (fingers round a grip hide behind it and it behind
        // them), shaded the same way; then the meshes a special draw takes (a collimator's lens).
        if let (Some((_, c)), false) = (self.flat, held.is_empty()) {
            cb.plain = [c[0], c[1], c[2], 2.0];
            self.frame_cb.update(ctx, &cb);
        }
        if !held.is_empty() {
            for h in held {
                h.model.upload(ctx, h.parts);
                unsafe { ctx.VSSetConstantBuffers(1, Some(&[Some(h.model.cb.buf.clone())])) };
                let meshes: Vec<&GpuMesh> = h.model.meshes.iter().zip(&h.model.info).filter(|(_, i)| !i.special).map(|(m, _)| m).collect();
                self.draw_shaded(ctx, &meshes, ALL);
            }
            if let Some(special) = special.as_mut() {
                self.specials(gpu, held, &mut **special, eye, fov, width, height, depth.as_ref().map(|d| (d.srv, d.units_per_meter)));
                // (It may have changed any state.)
                unsafe {
                    ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), &dsv);
                    ctx.PSSetShaderResources(11, Some(&post));
                }
                self.bind_common(ctx, game_depth.as_ref(), width, height);
            }
        }

        let mut ghost_ready = [false; 2];
        if draw_body && ghosts.iter().any(Option::is_some) {
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
        if let Some(l) = self.laser.filter(|_| draw_body) {
            self.draw_laser(ctx, eye, fov, width, &l);
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&[None, None, None, None, None, None, None]));
            ctx.PSSetShaderResources(11, Some(&[None, None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// The avatar's depth buffer for a target this size (the eyes, a desktop view, the game's
    /// pictures): the last few sizes kept.
    fn depth_for(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<ID3D11DepthStencilView> {
        let i = match self.depth.iter().position(|d| d.width == width && d.height == height) {
            Some(i) => i,
            None => {
                if self.depth.len() >= 4 {
                    self.depth.remove(0);
                }
                self.depth.push(Texture::new(gpu, &TexDesc::depth(width, height))?);
                self.depth.len() - 1
            }
        };
        Ok(self.depth[i].dsv.clone().unwrap())
    }

    /// The view depth (metres) of the player `frame` poses, with `held`, seen from `eye`, where it
    /// shows: into `target` (R32F, 0 where it doesn't), hidden behind the game's world (`depth`) as
    /// the game's frame hid it. The game's forward glass and the special draws (a collimator's
    /// lens) aren't in it: what shows there is what's behind them.
    pub fn draw_view_depth(
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
        held: &[crate::HeldDraw],
    ) -> Result<()> {
        let ctx = &gpu.ctx;
        let show = show_of(frame);
        let main = main_model(body, arms, show);
        main.pose(&pose_input(frame));
        main.upload_body(ctx, show);
        self.frame_cb.update(ctx, &frame_constants(width, height, eye, fov, frame, depth.as_ref()));
        let dsv = self.depth_for(gpu, width, height)?;
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), &dsv);
            ctx.ClearRenderTargetView(target, &[0.0; 4]);
            ctx.ClearDepthStencilView(&dsv, D3D11_CLEAR_DEPTH.0, 1.0, 0);
        }
        let game_depth = depth.as_ref().map(|d| d.srv.clone());
        self.bind_common(ctx, game_depth.as_ref(), width, height);
        self.view_z.bind(ctx);
        draw_opaque(ctx, main, range_of(show), held);
        unsafe {
            ctx.PSSetShaderResources(0, Some(&vec![None; 11]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// Writes the depth of the player `frame` poses (with `held`), seen from `eye`, into the game's
    /// own depth buffer (`dsv`, `width` x `height`) as the game's projection (`proj`, with its own
    /// near and far planes) stores it, where it's nearer than what's there: the game's later passes
    /// (its volumetric light, particles and glass) then stop at the player as at its own things.
    pub fn draw_game_depth(
        &mut self,
        gpu: &Gpu,
        dsv: &ID3D11DepthStencilView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        proj: Mat4,
        frame: &AvatarFrame,
        body: &mut Character,
        arms: Option<&mut Character>,
        held: &[crate::HeldDraw],
    ) -> Result<()> {
        let ctx = &gpu.ctx;
        let show = show_of(frame);
        let main = main_model(body, arms, show);
        main.pose(&pose_input(frame));
        main.upload_body(ctx, show);
        let mut cb = frame_constants(width, height, eye, fov, frame, None);
        cb.view_proj = (proj * view_matrix(eye)).to_cols_array();
        self.frame_cb.update(ctx, &cb);
        unsafe { ctx.OMSetRenderTargets(None, dsv) };
        self.bind_common(ctx, None, width, height);
        self.view_z.bind(ctx);
        draw_opaque(ctx, main, range_of(show), held);
        unsafe {
            ctx.PSSetShaderResources(0, Some(&vec![None; 11]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// Draws the player `now` poses into `target` as seen from `eye`, every point in the colour
    /// the game drew it: `then` is the pose the game drew the player in, `held` each held object
    /// with its parts' places now and then, and `sources` the game's pictures (with the player's
    /// depth in them, [`Renderer::draw_view_depth`]); the first is looked in first. Hidden behind
    /// the world as it shows now (`depth`).
    pub fn draw_reprojected(
        &mut self,
        gpu: &Gpu,
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
        eye: &Pose,
        fov: &Fov,
        now: &AvatarFrame,
        then: &AvatarFrame,
        depth: Option<DepthInput>,
        body: &mut Character,
        arms: Option<&mut Character>,
        held: &[(crate::HeldDraw, &[Option<Mat4>])],
        sources: &[crate::PlayerSource],
    ) -> Result<()> {
        let show = show_of(now);
        // (A body drawn as another model then has nothing to take its colours from.)
        if sources.is_empty() || show_of(then) != show {
            return Ok(());
        }
        let ctx = &gpu.ctx;
        let main = main_model(body, arms, show);
        main.upload_then(ctx, &pose_input(then), show);
        main.pose(&pose_input(now));
        main.upload_body(ctx, show);
        self.frame_cb.update(ctx, &frame_constants(width, height, eye, fov, now, depth.as_ref()));
        let mut cb = SourcesCb::zeroed();
        for (k, s) in sources.iter().take(2).enumerate() {
            let view = view_matrix(&s.eye);
            cb.view_proj[k] = (projection(&s.fov) * view).to_cols_array();
            cb.view_z[k] = view.row(2).to_array();
        }
        cb.info = [sources.len().min(2) as f32, SOURCE_BIAS, SOURCE_BIAS_PER_METER, debug_tint() as u32 as f32];
        self.sources_cb.update(ctx, &cb);
        let dsv = self.depth_for(gpu, width, height)?;
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), &dsv);
            ctx.ClearDepthStencilView(&dsv, D3D11_CLEAR_DEPTH.0, 1.0, 0);
        }
        let game_depth = depth.as_ref().map(|d| d.srv.clone());
        self.bind_common(ctx, game_depth.as_ref(), width, height);
        self.reproject.bind(ctx);
        let src = |k: usize| sources.get(k).unwrap_or(&sources[0]);
        unsafe {
            ctx.VSSetConstantBuffers(1, Some(&[Some(main.body_cb.buf.clone())]));
            ctx.VSSetConstantBuffers(4, Some(&[Some(main.then_cb.buf.clone()), Some(self.sources_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(5, Some(&[Some(self.sources_cb.buf.clone())]));
            ctx.PSSetShaderResources(7, Some(&[Some(src(0).color.clone()), Some(src(1).color.clone()), Some(src(0).player_depth.clone()), Some(src(1).player_depth.clone())]));
        }
        let range = range_of(show);
        for m in main.meshes.iter().filter(|m| !forward(m)) {
            draw_mesh(ctx, m, range);
        }
        for (h, then_parts) in held {
            h.model.upload(ctx, h.parts);
            h.model.upload_then(ctx, then_parts);
            unsafe {
                ctx.VSSetConstantBuffers(1, Some(&[Some(h.model.cb.buf.clone())]));
                ctx.VSSetConstantBuffers(4, Some(&[Some(h.model.then_cb.buf.clone())]));
            }
            for (m, _) in h.model.meshes.iter().zip(&h.model.info).filter(|(m, i)| !i.special && !forward(m)) {
                draw_mesh(ctx, m, ALL);
            }
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&vec![None; 11]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// The state every draw starts from: the frame, lighting, samplers, the game's depth, the
    /// grading table and light atlas, opaque blending with depth writes.
    fn bind_common(&self, ctx: &ID3D11DeviceContext, game_depth: Option<&ID3D11ShaderResourceView>, width: u32, height: u32) {
        unsafe {
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.frame_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(3, Some(&[Some(self.lighting_cb.buf.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone()), Some(self.clamp_sampler.clone())]));
            ctx.PSSetShaderResources(1, Some(&[game_depth.cloned()]));
            ctx.PSSetShaderResources(5, Some(&[Some(self.lut_srv.clone()), self.atlas.clone()]));
            ctx.OMSetBlendState(&self.opaque_blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_write, 0);
            ctx.RSSetState(&self.two_sided);
        }
        pipeline::viewport(ctx, width, height);
    }

    /// The body's meshes (see [`Renderer::draw_shaded`]).
    fn draw_body(&self, ctx: &ID3D11DeviceContext, c: &Character, range: usize) {
        let meshes: Vec<&GpuMesh> = c.meshes.iter().collect();
        self.draw_shaded(ctx, &meshes, range);
    }

    /// Meshes with the game's shading where it's set and the mesh has the game's material (its
    /// forward glass blended after everything opaque), else the runtime's own look.
    fn draw_shaded(&self, ctx: &ID3D11DeviceContext, meshes: &[&GpuMesh], range: usize) {
        let shading = self.shading;
        fn metro_of(m: &GpuMesh, shading: bool) -> Option<&MetroMesh> {
            m.metro.as_ref().filter(|_| shading)
        }
        // (0: the runtime's own look, 1: Metro's, 2: Gamebryo's.)
        let mut bound: Option<u32> = None;
        for &m in meshes {
            let metro = metro_of(m, shading);
            if metro.is_some_and(|mm| mm.class.forward()) {
                continue;
            }
            let program = match metro {
                None => 0,
                Some(mm) if mm.class.gamebryo() => 2,
                Some(_) => 1,
            };
            if bound != Some(program) {
                match program {
                    0 => self.opaque.bind(ctx),
                    1 => self.metro.bind(ctx),
                    _ => self.gamebryo.bind(ctx),
                }
                bound = Some(program);
            }
            draw_mesh(ctx, m, range);
        }
        if meshes.iter().any(|m| metro_of(m, shading).is_some_and(|mm| mm.class.forward())) {
            self.metro_glass.bind(ctx);
            unsafe {
                ctx.OMSetBlendState(&self.alpha_blend, Some(&[0.0; 4]), 0xFFFF_FFFF);
                ctx.OMSetDepthStencilState(&self.depth_test, 0);
                ctx.RSSetState(&self.cull_back);
            }
            for &m in meshes.iter().filter(|m| metro_of(m, shading).is_some_and(|mm| mm.class.forward())) {
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
    /// The laser's beam and (when it hits) its dot, in the player's depth (no writes), its light
    /// added.
    fn draw_laser(&self, ctx: &ID3D11DeviceContext, eye: &Pose, fov: &Fov, width: u32, l: &crate::LaserBeam) {
        let pixel = (fov.right - fov.left) / width.max(1) as f32;
        let inv_vp = (projection(fov) * view_matrix(eye)).inverse().to_cols_array();
        let textures = self.laser_textures.as_ref().and_then(|t| t.as_ref());
        self.laser_program.bind(ctx);
        unsafe {
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            ctx.OMSetBlendState(&self.additive, Some(&[0.0; 4]), 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(&self.depth_test, 0);
            ctx.RSSetState(&self.two_sided);
            // (Vertex shaders read the game's depth too: where the beam stops.)
            ctx.VSSetShaderResources(1, Some(&[self.bound_game_depth(ctx)]));
            ctx.VSSetConstantBuffers(4, Some(&[Some(self.laser_cb.buf.clone())]));
            ctx.PSSetConstantBuffers(4, Some(&[Some(self.laser_cb.buf.clone())]));
            ctx.PSSetShaderResources(13, Some(&textures.map_or([None, None], |t| [Some(t[0].clone()), Some(t[1].clone())])));
        }
        let t = self.laser_clock.elapsed().as_secs_f32();
        let color = [LASER_RED[0], LASER_RED[1], LASER_RED[2], t];
        let (from, to) = (l.from.extend(0.0).to_array(), l.to.extend(0.0).to_array());
        let has = textures.is_some() as u32 as f32;
        self.laser_cb.update(ctx, &LaserCb { a: from, b: to, color, mode: [0.0, 0.0, has, pixel], inv_vp });
        // The beam's two crossed planes.
        unsafe { ctx.DrawInstanced(4, 2, 0, 0) };
        // Its dot: where its collision stops it, or wherever the game's depth shows it stopping.
        self.laser_cb.update(ctx, &LaserCb { a: from, b: to, color, mode: [1.0, LASER_DOT_RADIUS, has, pixel], inv_vp });
        unsafe {
            ctx.Draw(4, 0);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShaderResources(1, Some(&[None]));
            ctx.PSSetShaderResources(13, Some(&[None, None]));
        }
    }

    /// The game's depth as bound for the pixel shaders (t1).
    fn bound_game_depth(&self, ctx: &ID3D11DeviceContext) -> Option<ID3D11ShaderResourceView> {
        let mut v = [None];
        unsafe { ctx.PSGetShaderResources(1, Some(&mut v)) };
        v[0].take()
    }

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

/// The model that draws `show`: the first-person arms model (if any) in the arms and hands
/// views, the body in the full view.
fn main_model<'a>(body: &'a mut Character, arms: Option<&'a mut Character>, show: Show) -> &'a mut Character {
    match arms {
        Some(arms) if show != Show::Full => arms,
        _ => body,
    }
}

/// The index range of `show`'s triangles.
fn range_of(show: Show) -> usize {
    match show {
        Show::Full => ALL,
        Show::Arms => ARMS,
        _ => HANDS,
    }
}

/// Debugging (`RUSTVR_DEBUG_IN_FRAME=1`): the moved player tinted by which of the game's pictures
/// each of its colours came from ([`Renderer::draw_reprojected`]).
fn debug_tint() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTVR_DEBUG_IN_FRAME").is_ok_and(|v| v == "1"))
}

/// The opaque meshes of `main` (`range`) and of `held` (not the forward glass, not the special
/// draws), with whatever program is bound.
fn draw_opaque(ctx: &ID3D11DeviceContext, main: &Character, range: usize, held: &[crate::HeldDraw]) {
    unsafe { ctx.VSSetConstantBuffers(1, Some(&[Some(main.body_cb.buf.clone())])) };
    for m in main.meshes.iter().filter(|m| !forward(m)) {
        draw_mesh(ctx, m, range);
    }
    for h in held {
        h.model.upload(ctx, h.parts);
        unsafe { ctx.VSSetConstantBuffers(1, Some(&[Some(h.model.cb.buf.clone())])) };
        for (m, _) in h.model.meshes.iter().zip(&h.model.info).filter(|(m, i)| !i.special && !forward(m)) {
            draw_mesh(ctx, m, ALL);
        }
    }
}

/// Drawn as the game's forward glass (blended over the rest).
fn forward(m: &GpuMesh) -> bool {
    m.metro.as_ref().is_some_and(|mm| mm.class.forward())
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
    let held = frame.held.map(|h| h.map(|h| PosedFingers { rel: h.finger_rotations(), size: h.size, reach: h.reach_points(), weight: h.weight }));
    PoseInput { joints: frame.joints, hands, fingers: &frame.fingers, held, grasp: frame.grasp, chop_head: frame.first_person && show_of(frame) == Show::Full }
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
                if mm.face.is_some() {
                    ctx.PSSetShaderResources(13, Some(&[mm.face.clone(), mm.face_detail.clone()]));
                }
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

/// Light added to what's there (a laser's).
fn additive_blend(gpu: &Gpu) -> Result<ID3D11BlendState> {
    let rt = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D11_BLEND_ONE,
        DestBlend: D3D11_BLEND_ONE,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ZERO,
        DestBlendAlpha: D3D11_BLEND_ONE,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let desc = D3D11_BLEND_DESC { AlphaToCoverageEnable: false.into(), IndependentBlendEnable: false.into(), RenderTarget: [rt; 8] };
    let mut s = None;
    unsafe { gpu.device.CreateBlendState(&desc, Some(&mut s))? };
    s.context("no blend state")
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
        plain: [PLAIN[0], PLAIN[1], PLAIN[2], frame.plain as u32 as f32],
        post: [depth.is_some_and(|d| d.bloom.is_some() && d.lens.is_some()) as u32 as f32, 0.0, 0.0, 0.0],
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

#[cfg(test)]
mod tests {
    #[test]
    fn the_shader_compiles() {
        let source = format!("{}{}{}", super::metro::HLSL, crate::gamebryo::HLSL, super::SHADER);
        for (vs, ps) in [("VSMain", "PSMain"), ("VSMain", "PSMetro"), ("VSMain", "PSMetroGlass"), ("VSMain", "PSGamebryo"), ("VSMain", "PSGhost")] {
            rustvr_gpu::shader::compile(&source, "avatar", vs, "vs_5_0").unwrap_or_else(|e| panic!("{vs}: {e:#}"));
            rustvr_gpu::shader::compile(&source, "avatar", ps, "ps_5_0").unwrap_or_else(|e| panic!("{ps}: {e:#}"));
        }
    }
}
