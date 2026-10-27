//! The player the game draws inside its own frame (an adapter that sets `GS_DRAWS_BODY`, as the
//! Metro adapter does, SkyCraft's way), moved to where it is at every headset frame.
//!
//! Drawn in the game's frame, the player gets everything the game does to its picture: its light,
//! fog, bloom, tone curve and grading. But the game draws its pictures slower than the headset
//! shows them (Metro, one eye per game frame: 50-60 pairs a second at a headset's resolution) and
//! each one 20-30 ms after the runtime posed the player for it, so hands and gun drawn there would
//! trail the controllers and step at the game's pace. So:
//!
//! * the runtime poses the player it sends for when the game's picture of it will show
//!   ([`predict`]), and keeps every pose it sent;
//! * the adapter says which pose each pair of eyes was drawn with (`SlotInfo::player_frame`);
//! * at every headset frame the player is drawn where it is now with the runtime's copy of the same
//!   meshes, each point in the colour the game gave it, taken from where that point was in the
//!   game's picture (`rustvr_avatar::Avatar::draw_reprojected`): the game's own pixels, moved;
//! * and the world is moved by its depth to the head and the walking player (`crate::warp`)
//!   without the player's old pixels: where the other eye saw the world behind them that shows,
//!   and where neither did it's filled from around ([`Fill`]). (The game's depth has the player in
//!   it, as the game's own passes need: the depth behind the player is filled from around too.)

use std::collections::VecDeque;

use anyhow::Result;
use glam::{Mat4, Quat, Vec3, Vec4};
use rustvr_avatar::{AvatarFrame, HeldDraw, HeldHand};
use rustvr_gpu::shader::Program;
use rustvr_gpu::{Gpu, TexDesc, Texture, pipeline};
use rustvr_protocol::*;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32_FLOAT};

/// Poses kept (headset frames): the game's picture of one is shown well within this.
const KEPT: usize = 64;

/// The furthest ahead (seconds) the player is posed for the game's picture.
pub const MAX_LEAD: f32 = 0.06;

/// The player as an avatar frame (what the adapter draws from the same [`RtPlayerDraw`]).
pub fn avatar_frame(pd: &RtPlayerDraw) -> AvatarFrame<'_> {
    AvatarFrame {
        joints: &pd.body.joints,
        fingers: pd.body.fingers,
        hands: pd.body.hands,
        ghosts: pd.ghosts.map(|g| (g.valid != 0).then_some(g)),
        held: [HeldHand::from_drawn(&pd.hands[0]), HeldHand::from_drawn(&pd.hands[1])],
        visibility: pd.body.visibility,
        light: LightHint::default(),
        first_person: true,
        plain: false,
    }
}

/// A drawn object's parts' places (model space -> tracking space; None: not drawn).
pub fn part_places(o: &DrawnObject) -> Vec<Option<Mat4>> {
    (0..(o.part_count as usize).min(DRAWN_PARTS_MAX))
        .map(|k| o.part(k).map(|r| Mat4::from_cols(Vec4::from(r[0]), Vec4::from(r[1]), Vec4::from(r[2]), Vec4::W).transpose()))
        .collect()
}

/// The objects of a posed player, by model name.
pub fn objects(pd: &RtPlayerDraw) -> &[DrawnObject] {
    &pd.objects[..(pd.object_count as usize).min(DRAWN_OBJECTS_MAX)]
}

/// The objects of `now` with where each was in `then`: paired by model and, among the same model's,
/// by order. One with no pair isn't in the game's picture.
pub fn pair_objects(now: &RtPlayerDraw, then: &RtPlayerDraw) -> Vec<(usize, usize)> {
    let (a, b) = (objects(now), objects(then));
    let mut out = Vec::new();
    for (i, o) in a.iter().enumerate() {
        let nth = a[..i].iter().filter(|x| x.name == o.name).count();
        if let Some(j) = b.iter().enumerate().filter(|(_, x)| x.name == o.name).map(|(j, _)| j).nth(nth) {
            out.push((i, j));
        }
    }
    out
}

/// `pd` posed `lead` seconds on: each hand that's moving freely, and what it holds, carried on by
/// its controller's motion now (turning about the controller); the rest where it is. The game shows
/// its picture of the player about that much later. `hands`: the controllers; `free`: the hands the
/// world isn't holding back; `object_hands`: the hand holding each of `pd`'s objects.
pub fn predict(pd: &mut RtPlayerDraw, hands: &[Controller; 2], free: [bool; 2], object_hands: &[Option<usize>], lead: f32) {
    let lead = lead.clamp(0.0, MAX_LEAD);
    if lead <= 0.0 {
        return;
    }
    for h in 0..2 {
        let c = &hands[h];
        if c.connected == 0 || !free[h] {
            continue;
        }
        let (v, w) = (Vec3::from_array(c.vel), Vec3::from_array(c.ang_vel));
        if !v.is_finite() || !w.is_finite() {
            continue;
        }
        let r = Quat::from_scaled_axis(w * lead);
        let pivot = Vec3::from_array(c.grip.pos);
        let m = Mat4::from_translation(pivot + v * lead) * Mat4::from_quat(r) * Mat4::from_translation(-pivot);
        let carry = |p: &mut Pose| {
            if p.valid != 0 {
                p.pos = m.transform_point3(Vec3::from_array(p.pos)).to_array();
                p.rot = (r * Quat::from_array(p.rot)).normalize().to_array();
            }
        };
        carry(&mut pd.body.joints[[Joint::LeftHand, Joint::RightHand][h] as usize]);
        carry(&mut pd.body.hands[h].pose);
        if pd.hands[h].flags & DH_HELD != 0 {
            carry(&mut pd.hands[h].wrist);
        }
        let n = (pd.object_count as usize).min(DRAWN_OBJECTS_MAX);
        for (o, _) in pd.objects[..n].iter_mut().zip(object_hands).filter(|(_, held)| **held == Some(h)) {
            for k in 0..(o.part_count as usize).min(DRAWN_PARTS_MAX) {
                if let Some(rows) = o.part(k) {
                    let part = m * Mat4::from_cols(Vec4::from(rows[0]), Vec4::from(rows[1]), Vec4::from(rows[2]), Vec4::W).transpose();
                    o.set_part(k, Some([0, 1, 2].map(|i| part.row(i).to_array())));
                }
            }
        }
    }
}

/// For the shown pair of eyes: which of each eye's pixels are the player's (its view depth there,
/// 0 elsewhere) and the picture with them filled from the world around.
pub struct Prepared {
    key: (usize, u32, u64, u64),
    pub player: [Texture; 2],
    pub world: [Texture; 2],
    /// The world's depth without the player (the game's units, 0: nothing drawn).
    pub depth: [Texture; 2],
}

pub struct InFrame {
    /// The poses sent for the game to draw, oldest first.
    sent: VecDeque<Box<RtPlayerDraw>>,
    /// The player as it is this frame (unpredicted), when the game draws it.
    now: Option<Box<RtPlayerDraw>>,
    prepared: Option<Prepared>,
    fill: Fill,
    /// Pairs whose pose was no longer kept (said now and then).
    missed: u64,
    /// The first pair readied has been said.
    said: bool,
}

impl InFrame {
    pub fn new(gpu: &Gpu) -> Result<InFrame> {
        Ok(InFrame { sent: VecDeque::new(), now: None, prepared: None, fill: Fill::new(gpu)?, missed: 0, said: false })
    }

    /// The pose sent for the game to draw (as the adapter got it).
    pub fn record(&mut self, pd: &RtPlayerDraw) {
        let mut b = if self.sent.len() >= KEPT { self.sent.pop_front().unwrap() } else { Box::default() };
        *b = *pd;
        self.sent.push_back(b);
    }

    /// The player as it is this frame (None: the game doesn't draw it now).
    pub fn set_now(&mut self, pd: Option<&RtPlayerDraw>) {
        match pd {
            Some(pd) => **self.now.get_or_insert_with(Box::default) = *pd,
            None => self.now = None,
        }
    }

    pub fn now(&self) -> Option<&RtPlayerDraw> {
        self.now.as_deref()
    }

    /// The pair readied last ([`InFrame::prepare`]).
    pub fn prepared(&self) -> Option<&Prepared> {
        self.prepared.as_ref()
    }

    /// The pose sent for runtime frame `frame`.
    pub fn sent(&self, frame: u64) -> Option<&RtPlayerDraw> {
        self.sent.iter().rev().find(|p| p.frame_index == frame).map(|b| &**b)
    }

    /// Readies the shown pair (`slot`, number `index`): which pixels are the player's (posed as
    /// `info.player_frame` was sent) and the world without them. None when the game drew no
    /// player into it, or its pose isn't kept.
    pub fn prepare(
        &mut self,
        gpu: &Gpu,
        index: usize,
        slot: &crate::targets::Slot,
        info: &SlotInfo,
        body: &mut crate::body::PlayerBody,
        held: &crate::held_objects::HeldObjects,
    ) -> Option<&Prepared> {
        if info.player_frame == 0 || self.now.is_none() {
            return None;
        }
        let depth = slot.depth.as_ref()?;
        let key = (index, info.generation, info.frame_index, info.player_frame);
        if self.prepared.as_ref().is_some_and(|p| p.key == key) {
            return self.prepared.as_ref();
        }
        let Some(then) = self.sent.iter().rev().find(|p| p.frame_index == info.player_frame) else {
            self.missed += 1;
            if self.missed.is_power_of_two() {
                tracing::info!("in frame: {} pairs drawn with a pose no longer kept (runtime frame {})", self.missed, info.player_frame);
            }
            return None;
        };
        let (w, h) = (slot.eyes[0].width, slot.eyes[0].height);
        let made = (|| -> Result<Prepared> {
            let reuse = self.prepared.take().filter(|p| p.player[0].width == w && p.player[0].height == h);
            let mut p = match reuse {
                Some(p) => p,
                None => {
                    let t = |format| Texture::new(gpu, &TexDesc::target(w, h, format));
                    Prepared {
                        key,
                        player: [t(DXGI_FORMAT_R32_FLOAT)?, t(DXGI_FORMAT_R32_FLOAT)?],
                        world: [t(DXGI_FORMAT_R16G16B16A16_FLOAT)?, t(DXGI_FORMAT_R16G16B16A16_FLOAT)?],
                        depth: [t(DXGI_FORMAT_R32_FLOAT)?, t(DXGI_FORMAT_R32_FLOAT)?],
                    }
                }
            };
            p.key = key;
            let places: Vec<(String, Vec<Option<Mat4>>)> = objects(then).iter().map(|o| (name_str(&o.name).to_string(), part_places(o))).collect();
            let draws: Vec<HeldDraw> = places.iter().filter_map(|(name, parts)| Some(HeldDraw { model: held.model(name)?, parts, key: name })).collect();
            for e in 0..2 {
                let (Some(color), Some(z)) = (slot.eyes[e].srv.as_ref(), depth[e].srv.as_ref()) else { anyhow::bail!("eye {e} has no picture") };
                let world = rustvr_avatar::DepthInput { srv: z, units_per_meter: info.eye_map[e].world_per_meter.max(1e-6) };
                body.draw_view_depth(gpu, p.player[e].rtv.as_ref().unwrap(), w, h, &info.eye_pose[e], &info.fov[e], then, world, &draws)?;
                self.fill.run(gpu, color, p.player[e].srv.as_ref().unwrap(), &p.world[e])?;
                self.fill.run_depth(gpu, z, p.player[e].srv.as_ref().unwrap(), &p.depth[e])?;
            }
            Ok(p)
        })();
        match made {
            Ok(p) => {
                if !std::mem::replace(&mut self.said, true) {
                    tracing::info!("in frame: the player the game drew is moved to where it is at each headset frame ({w}x{h} pictures)");
                }
                self.prepared = Some(p);
            }
            Err(e) => {
                tracing::warn!("in frame: {e:#}");
                return None;
            }
        }
        self.prepared.as_ref()
    }
}

const FILL: &str = r#"
Texture2D a : register(t0);
Texture2D b : register(t1);
Texture2D c : register(t2);
SamplerState lin : register(s0);

float4 VSMain(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}

// The picture (a) premultiplied by how much of each pixel is the world: none where the player
// (b: its view depth, > 0) is, or right beside it (the game's anti-aliasing blends the player's
// edge into its neighbours).
float4 PSStart(float4 p : SV_Position) : SV_Target {
    uint w, h;
    b.GetDimensions(w, h);
    int2 c = int2(p.xy);
    float world = 1;
    [unroll] for (int k = 0; k < 9; k++) {
        if (b.Load(int3(clamp(c + int2(k % 3 - 1, k / 3 - 1), 0, int2(w - 1, h - 1)), 0)).r > 0) world = 0;
    }
    return float4(a.Load(int3(c, 0)).rgb * world, world);
}

// The depth (a, 0: nothing drawn) as the picture is in PSStart: inverse depth (the far and the sky
// mix sensibly), premultiplied by how much of each pixel is the world.
float4 PSStartDepth(float4 p : SV_Position) : SV_Target {
    uint w, h;
    b.GetDimensions(w, h);
    int2 c = int2(p.xy);
    float world = 1;
    [unroll] for (int k = 0; k < 9; k++) {
        if (b.Load(int3(clamp(c + int2(k % 3 - 1, k / 3 - 1), 0, int2(w - 1, h - 1)), 0)).r > 0) world = 0;
    }
    float d = a.Load(int3(c, 0)).r;
    return float4((d > 0 ? 1 / d : 0) * world, 0, 0, world);
}

// The depth: the world's own (c) where the pixel is all world (b: the start's weight), else the
// filled inverse depth (a) back as depth (0: nothing there).
float PSEndDepth(float4 p : SV_Position) : SV_Target {
    int3 px = int3(p.xy, 0);
    if (b.Load(px).a >= 1) return c.Load(px).r;
    float inv = a.Load(px).r;
    return inv > 1e-5 ? 1 / inv : 0;
}

// Half the size: the average of four.
float4 PSPull(float4 p : SV_Position) : SV_Target {
    uint w, h;
    a.GetDimensions(w, h);
    int2 at = int2(p.xy) * 2;
    float4 s = 0;
    [unroll] for (int k = 0; k < 4; k++) s += a.Load(int3(min(at + int2(k & 1, k >> 1), int2(w - 1, h - 1)), 0));
    return s * 0.25;
}

// The smallest: what it knows, as a colour.
float4 PSTop(float4 p : SV_Position) : SV_Target {
    float4 c = a.Load(int3(p.xy, 0));
    return float4(c.a > 0 ? c.rgb / c.a : 0, 1);
}

// Back down a level: its own colour as far as it knows it (a), the level above (b, filled) for the
// rest.
float4 PSPush(float4 p : SV_Position) : SV_Target {
    uint w, h;
    a.GetDimensions(w, h);
    float4 c = a.Load(int3(p.xy, 0));
    float3 up = b.SampleLevel(lin, p.xy / float2(w, h), 0).rgb;
    float k = saturate(c.a);
    return float4(k > 0 ? lerp(up, c.rgb / c.a, k) : up, 1);
}
"#;

/// Fills the player's pixels of a picture from the world around them (pull-push: the known
/// colours averaged down to one pixel, then filled back up through the levels).
struct Fill {
    start: Program,
    start_depth: Program,
    end_depth: Program,
    pull: Program,
    top: Program,
    push: Program,
    sampler: ID3D11SamplerState,
    /// Per level (full size first): the pulled averages and the filled colours.
    levels: Vec<(Texture, Texture)>,
}

impl Fill {
    fn new(gpu: &Gpu) -> Result<Fill> {
        Ok(Fill {
            start: Program::with_entries(gpu, FILL, "fill", "VSMain", "PSStart", &[])?,
            start_depth: Program::with_entries(gpu, FILL, "fill", "VSMain", "PSStartDepth", &[])?,
            end_depth: Program::with_entries(gpu, FILL, "fill", "VSMain", "PSEndDepth", &[])?,
            pull: Program::with_entries(gpu, FILL, "fill", "VSMain", "PSPull", &[])?,
            top: Program::with_entries(gpu, FILL, "fill", "VSMain", "PSTop", &[])?,
            push: Program::with_entries(gpu, FILL, "fill", "VSMain", "PSPush", &[])?,
            sampler: pipeline::sampler(gpu, true, false)?,
            levels: Vec::new(),
        })
    }

    /// `color` with the pixels where `player` (view depth, > 0) is filled from around, into `out`
    /// (RGBA16F, the same size).
    fn run(&mut self, gpu: &Gpu, color: &ID3D11ShaderResourceView, player: &ID3D11ShaderResourceView, out: &Texture) -> Result<()> {
        self.levels_for(gpu, out.width, out.height)?;
        self.pull_push(gpu, &self.start, color, player, out, None);
        Ok(())
    }

    /// The depth `depth` (R32F) with the player's pixels filled from the world's around them, into
    /// `out` (R32F, the same size).
    fn run_depth(&mut self, gpu: &Gpu, depth: &ID3D11ShaderResourceView, player: &ID3D11ShaderResourceView, out: &Texture) -> Result<()> {
        self.levels_for(gpu, out.width, out.height)?;
        self.pull_push(gpu, &self.start_depth, depth, player, out, Some(&self.end_depth));
        Ok(())
    }

    /// The levels for a picture `w` x `h` (made when the size changes).
    fn levels_for(&mut self, gpu: &Gpu, w: u32, h: u32) -> Result<()> {
        if self.levels.first().is_none_or(|l| (l.0.width, l.0.height) != (w, h)) {
            self.levels.clear();
            let (mut lw, mut lh) = (w, h);
            loop {
                let t = || Texture::new(gpu, &TexDesc::target(lw, lh, DXGI_FORMAT_R16G16B16A16_FLOAT));
                self.levels.push((t()?, t()?));
                if lw == 1 && lh == 1 {
                    break;
                }
                (lw, lh) = (lw.div_ceil(2), lh.div_ceil(2));
            }
        }
        Ok(())
    }

    /// Pull-push from `start` (its inputs `input` and `player`) into `out`, through the last pass
    /// `end` when given (else the filled values go straight into `out`).
    fn pull_push(&self, gpu: &Gpu, start: &Program, input: &ID3D11ShaderResourceView, player: &ID3D11ShaderResourceView, out: &Texture, end: Option<&Program>) {
        let ctx = &gpu.ctx;
        let pass = |program: &Program, target: &Texture, inputs: [Option<ID3D11ShaderResourceView>; 3]| unsafe {
            ctx.OMSetRenderTargets(Some(&[target.rtv.clone()]), None);
            pipeline::viewport(ctx, target.width, target.height);
            program.bind(ctx);
            ctx.PSSetShaderResources(0, Some(&inputs));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None, None, None]));
        };
        unsafe {
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.OMSetBlendState(None, None, 0xFFFF_FFFF);
            ctx.OMSetDepthStencilState(None, 0);
            ctx.RSSetState(None);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        }
        let n = self.levels.len();
        pass(start, &self.levels[0].0, [Some(input.clone()), Some(player.clone()), None]);
        for k in 1..n {
            pass(&self.pull, &self.levels[k].0, [self.levels[k - 1].0.srv.clone(), None, None]);
        }
        pass(&self.top, &self.levels[n - 1].1, [self.levels[n - 1].0.srv.clone(), None, None]);
        for k in (0..n - 1).rev() {
            let target = if k == 0 && end.is_none() { out } else { &self.levels[k].1 };
            pass(&self.push, target, [self.levels[k].0.srv.clone(), self.levels[k + 1].1.srv.clone(), None]);
        }
        if let Some(end) = end {
            pass(end, out, [self.levels[0].1.srv.clone(), self.levels[0].0.srv.clone(), Some(input.clone())]);
        }
        unsafe { ctx.OMSetRenderTargets(None, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(name: &str, part: Mat4) -> DrawnObject {
        let mut o: DrawnObject = bytemuck::Zeroable::zeroed();
        o.name[..name.len()].copy_from_slice(name.as_bytes());
        o.part_count = 1;
        o.set_part(0, Some([0, 1, 2].map(|r| part.row(r).to_array())));
        o
    }

    #[test]
    fn a_moving_hand_and_its_gun_are_posed_where_they_will_be() {
        let mut pd: Box<RtPlayerDraw> = Box::default();
        let hand = Pose { pos: [0.2, 1.2, 0.4], valid: 1, rot: Quat::IDENTITY.to_array() };
        pd.body.joints[Joint::RightHand as usize] = hand;
        pd.body.hands[1].pose = hand;
        pd.object_count = 1;
        pd.objects[0] = object("metro2033/wpn_2012", Mat4::from_translation(Vec3::new(0.2, 1.2, 0.6)));
        let mut hands = [Controller::default(); 2];
        hands[1].connected = 1;
        hands[1].grip = hand;
        hands[1].vel = [1.0, 0.0, 0.0];
        // A quarter turn a second about +Y.
        hands[1].ang_vel = [0.0, std::f32::consts::FRAC_PI_2, 0.0];
        predict(&mut pd, &hands, [true, true], &[Some(1)], 0.04);
        let p = Vec3::from_array(pd.body.joints[Joint::RightHand as usize].pos);
        assert!(p.distance(Vec3::new(0.24, 1.2, 0.4)) < 1e-5, "the hand went on 4 cm: {p}");
        // The gun, 20 cm ahead of the grip, turned with it about the controller and carried along.
        let r = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 * 0.04);
        let want = Vec3::new(0.24, 1.2, 0.4) + r * Vec3::new(0.0, 0.0, 0.2);
        let got = part_places(&pd.objects[0])[0].unwrap().transform_point3(Vec3::ZERO);
        assert!(got.distance(want) < 1e-5, "{got} {want}");
        // The other hand (not moving) and a hand the world holds back stay.
        let mut held_back: Box<RtPlayerDraw> = Box::default();
        held_back.body.joints[Joint::RightHand as usize] = hand;
        predict(&mut held_back, &hands, [true, false], &[], 0.04);
        assert_eq!(held_back.body.joints[Joint::RightHand as usize].pos, hand.pos);
    }

    #[test]
    fn objects_pair_by_model_and_order() {
        let mut now: Box<RtPlayerDraw> = Box::default();
        let mut then: Box<RtPlayerDraw> = Box::default();
        let m = Mat4::IDENTITY;
        now.object_count = 3;
        now.objects[0] = object("a/gun", m);
        now.objects[1] = object("a/mag", m);
        now.objects[2] = object("a/mag", m);
        then.object_count = 2;
        then.objects[0] = object("a/mag", m);
        then.objects[1] = object("a/gun", m);
        // The second magazine wasn't drawn then.
        assert_eq!(pair_objects(&now, &then), vec![(0, 1), (1, 0)]);
    }

    /// A square hole in a picture of two colours is filled from around it: no black, each side's
    /// colour near that side.
    #[test]
    fn the_players_pixels_are_filled_from_the_world_around() {
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_FLOAT as R32, DXGI_FORMAT_R8G8B8A8_UNORM};
        let gpu = Gpu::new().unwrap();
        let n = 64u32;
        let mut color = vec![0u8; (n * n * 4) as usize];
        let mut player = vec![0f32; (n * n) as usize];
        for y in 0..n {
            for x in 0..n {
                let i = (y * n + x) as usize;
                color[i * 4..i * 4 + 4].copy_from_slice(if x < n / 2 { &[200, 0, 0, 255] } else { &[0, 0, 200, 255] });
                if (16..48).contains(&x) && (16..48).contains(&y) {
                    player[i] = 0.5;
                    color[i * 4..i * 4 + 4].copy_from_slice(&[0, 255, 0, 255]);
                }
            }
        }
        let src = Texture::upload(&gpu, n, n, DXGI_FORMAT_R8G8B8A8_UNORM, &[&color]).unwrap();
        let mask = Texture::upload(&gpu, n, n, R32, &[bytemuck::cast_slice(&player)]).unwrap();
        let out = Texture::new(&gpu, &TexDesc::target(n, n, DXGI_FORMAT_R16G16B16A16_FLOAT)).unwrap();
        let mut fill = Fill::new(&gpu).unwrap();
        fill.run(&gpu, src.srv.as_ref().unwrap(), mask.srv.as_ref().unwrap(), &out).unwrap();
        let raw = out.read_raw(&gpu).unwrap();
        let half = |h: u16| {
            let (e, m) = (((h >> 10) & 0x1f) as i32, (h & 0x3ff) as f32);
            if e == 0 { m * 2f32.powi(-24) } else { (1.0 + m / 1024.0) * 2f32.powi(e - 15) }
        };
        let at = |x: u32, y: u32| {
            let i = ((y * n + x) * 8) as usize;
            [0, 1, 2].map(|c| half(u16::from_le_bytes([raw[i + 2 * c], raw[i + 2 * c + 1]])))
        };
        let (left, right) = (at(18, 32), at(45, 32));
        assert!(left[1] < 0.05 && right[1] < 0.05, "none of the player's green is left: {left:?} {right:?}");
        assert!(left[0] > left[2] && right[2] > right[0], "each side takes its own neighbour's colour: {left:?} {right:?}");
        assert!(left[0] + left[2] > 0.3, "not black: {left:?}");
        // The world's own pixels stay as they were.
        assert!((at(4, 4)[0] - 200.0 / 255.0).abs() < 0.01);
    }

    /// The depth under the player (in the game's depth, as its own passes need) is the world's
    /// around it, not the player's.
    #[test]
    fn the_depth_behind_the_player_is_the_worlds() {
        let gpu = Gpu::new().unwrap();
        let n = 64u32;
        let mut depth = vec![5.0f32; (n * n) as usize];
        let mut player = vec![0f32; (n * n) as usize];
        for y in 20..44 {
            for x in 20..44 {
                depth[(y * n + x) as usize] = 0.4;
                player[(y * n + x) as usize] = 0.4;
            }
        }
        let up = |v: &[f32]| Texture::upload(&gpu, n, n, DXGI_FORMAT_R32_FLOAT, &[bytemuck::cast_slice(v)]).unwrap();
        let (d, p) = (up(&depth), up(&player));
        let out = Texture::new(&gpu, &TexDesc::target(n, n, DXGI_FORMAT_R32_FLOAT)).unwrap();
        let mut fill = Fill::new(&gpu).unwrap();
        fill.run_depth(&gpu, d.srv.as_ref().unwrap(), p.srv.as_ref().unwrap(), &out).unwrap();
        let got: Vec<f32> = bytemuck::cast_slice(&out.read_raw(&gpu).unwrap()).to_vec();
        let mid = got[(32 * n + 32) as usize];
        assert!((mid - 5.0).abs() < 0.05, "the middle of the player is the wall behind: {mid}");
        assert!((got[(2 * n + 2) as usize] - 5.0).abs() < 1e-3, "the world stays");
    }
}
