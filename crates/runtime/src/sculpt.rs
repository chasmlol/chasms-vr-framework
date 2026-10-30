//! Shaping the player's face by hand, in a game's character creator ([`FACE_SCULPT`]): the hands
//! take hold of the face with the grips and pull its parts into shape, through the face's own
//! FaceGen coordinates (`rustvr_avatar::sculpt`), and the face they make goes to the game
//! ([`FaceSculpt`]), which puts it into the character as its own sliders would. So the face shaped
//! is one the game makes, saves and shows its people, and the game's sliders show it.
//!
//! **What the hands take hold of.** Both of these, whichever a hand is nearer:
//!
//! * **A copy of the head** standing within reach, in front of the player and to their right (the
//!   game's menu stands at the mirror's left), [`BUST_SCALE`] times life size and turned to face
//!   them: a sculptor's bust of their own head, drawn as the body draws it. Every feature can be
//!   reached on it (a headset sits where a real face's nose and eyes are), it's big enough to take
//!   a nostril or a lip, and the hands on it don't hide the mirror.
//! * **Their own head**, where it is: reach up and pinch your own chin, pull your ears out, and
//!   watch it happen in the mirror. It moves with the head, so the pull is the hand's move against
//!   it.
//!
//! **Taking hold.** A hand's palm within [`REACH`] of the face shows what its grip would take: the
//! vertices round the nearest point glow in the game's cursor gold, most where they'd move most.
//! The grip takes it. Nothing snaps: the hand stays where it is, its fingers close on the point
//! taken, and the face doesn't move until the hand does.
//!
//! **Pulling.** The hand's move (against the head it holds) is the pull: the face is solved for it
//! each frame and drawn live, both on the player's body (so in the mirror) and on the copy. The part
//! taken goes as far with the hand as the face can take it (its morphs, the sliders' ranges); the
//! hand is drawn holding it where it got to, short of the controller when the face can't follow (a
//! ghost hand at the controller), and the pull is felt harder the further it is held back. Squeeze
//! the trigger while pulling and the pull goes asymmetric (one side against the other: a crooked
//! nose, one ear out); let it go and it's symmetric again, both sides alike. Both hands can pull at
//! once.
//!
//! **Each pull is a stroke** ([`FaceSculpt::stroke`]): from the first hand taking hold to the last
//! letting go. The game keeps the face from before each, for its undo.
//!
//! **Nothing passes through hands.** A free hand pushed into the copy is drawn on its surface (the
//! controller's ghost inside), as the physics hands are against the world.

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use rustvr_asset::FaceControls;
use rustvr_avatar::facegen::FaceNow;
use rustvr_avatar::sculpt::{Hold, Sculptor};
use rustvr_avatar::{FaceSource, Grasp, HeldDraw, HeldModel, Solid};
use rustvr_protocol::link::Link;
use rustvr_protocol::{FACE_CONTROLS, FACE_GIVEN, FACE_SCULPT, FaceDesc, FaceSculpt, SCULPT_HELD};

use crate::backend::HmdFrame;

/// How far from the face a palm takes hold (metres, the world's size: on the copy it's that much
/// of its larger face).
pub const REACH: f32 = 0.045;
/// The copy's size against the player's own head.
pub const BUST_SCALE: f32 = 1.7;
/// Where the copy stands from the head as the character creator opens (metres): ahead, to the
/// right, below the eyes (its face's middle).
const BUST_AHEAD: f32 = 0.42;
const BUST_RIGHT: f32 = 0.24;
const BUST_DOWN: f32 = 0.17;
/// The grip's squeeze that takes hold, and the one under which it lets go.
const TAKE: f32 = 0.6;
const LET_GO: f32 = 0.35;
/// How soon the trigger's asymmetry follows it (seconds).
const ASYM_EASE: f32 = 0.12;
/// The point taken's size, for the fingers to close round (metres, the face's own size).
const POINT_RADIUS: f32 = 0.012;
/// How long after the hands let go the game's face is waited for to come back with the one they
/// made, before what the game says goes (something else changed it).
const ECHO_WAIT: f64 = 0.6;

/// Which head a hand is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum On {
    Copy,
    Own,
}

/// What [`Sculpt::update`] needs each frame.
pub struct SculptInput<'a> {
    pub frame: &'a HmdFrame,
    /// Where the hands are drawn (tracking space: the hand joint's canonical frame).
    pub hands: [(Vec3, Quat); 2],
    pub head: (Vec3, Quat),
    /// The player's own head (its bone's skinning matrix: bind model space -> tracking space), as
    /// the mirror shows it.
    pub own_head: Option<Mat4>,
    pub now: f64,
    pub dt: f32,
}

/// What a hand holding the face, or pushed against it, is drawn as.
#[derive(Clone, Debug)]
pub struct HandOut {
    /// The drawn hand moved by this (tracking space): where the face held it.
    pub moved: Vec3,
    /// Its fingers on the point taken (none for a hand only pushed against the copy).
    pub grasp: Option<Grasp>,
}

#[derive(Default)]
struct HandState {
    holding: Option<Holding>,
    /// What it would take hold of (for its glow), and the tick when that starts.
    near: Option<(On, usize, usize, f32)>,
    asym: f32,
    squeezed: bool,
    /// Where the face held it this frame (see [`HandOut`]).
    out: Option<HandOut>,
}

struct Holding {
    on: On,
    hold: Hold,
    /// The palm against the head held, as it took hold (model space).
    from: Vec3,
    /// The pull now (model space) and the face's change for it (symmetric, then asymmetric).
    pull: Vec3,
    change: Vec<f32>,
    /// How far the point taken is held back from the hand (metres, tracking space).
    gap: f32,
}

/// A body's face, ready to be shaped by hand: its parts on their own, the solver for them, and
/// FaceGen's controls. Made off the frame with the body ([`FaceKit::new`]).
pub struct FaceKit {
    source: FaceSource,
    sculptor: Sculptor,
    controls: Option<Arc<FaceControls>>,
}

impl FaceKit {
    /// The face of a body just put together (`rustvr_avatar::compose`), if it has one; `dir` is the
    /// game's asset cache folder (FaceGen's controls, `facegen.toml`).
    pub fn new(body: &rustvr_avatar::AvatarSource, dir: &std::path::Path) -> Option<FaceKit> {
        let source = rustvr_avatar::face_source(body)?;
        let controls = controls(dir);
        let mut shapes: Vec<_> = source.meshes.iter().filter_map(|(m, x)| x.shape.clone().map(|s| (s, m.material.shader != rustvr_asset::Shader::GamebryoHair))).collect();
        shapes.sort_by_key(|(s, _)| s.part);
        let t = std::time::Instant::now();
        let sculptor = Sculptor::new(shapes, controls.clone());
        tracing::info!(
            "sculpt: a face of {} parts ({} coordinates and {} asymmetric) on {}, its solver ready in {:.0} ms",
            source.meshes.len(),
            sculptor.counts().0,
            sculptor.counts().1,
            source.head.0,
            t.elapsed().as_secs_f32() * 1000.0
        );
        Some(FaceKit { source, sculptor, controls })
    }
}

/// FaceGen's controls in a game's asset cache folder, read once.
fn controls(dir: &std::path::Path) -> Option<Arc<FaceControls>> {
    static READ: std::sync::Mutex<Option<(std::path::PathBuf, Option<Arc<FaceControls>>)>> = std::sync::Mutex::new(None);
    let mut read = READ.lock().unwrap();
    if let Some((d, c)) = read.as_ref()
        && d == dir
    {
        return c.clone();
    }
    let c = rustvr_asset::load_face_controls(dir).map(Arc::new).map_err(|e| tracing::warn!("sculpt: FaceGen's controls: {e:#}: a face shaped by hand keeps no age or ranges")).ok();
    *read = Some((dir.to_path_buf(), c.clone()));
    c
}

/// The face being shaped: its parts, the solver, the copy.
struct Kit {
    source: FaceSource,
    sculptor: Sculptor,
    /// The copy on the GPU (made at its first draw).
    model: Option<HeldModel>,
    /// The face's middle (model space), what the copy stands on.
    centre: Vec3,
    /// The face's vertices now (model space), and their normals, by part.
    positions: Vec<Vec<Vec3>>,
    normals: Vec<Vec<Vec3>>,
}

#[derive(Default)]
pub struct Sculpt {
    kit: Option<Kit>,
    controls: Option<Arc<FaceControls>>,
    /// The game's face as last read, and whether it may be shaped now, with its controls' limits.
    desc: Option<Box<FaceDesc>>,
    desc_seq: u32,
    sculptable: bool,
    limits: Vec<f32>,
    /// The face as the hands have it now (symmetric, asymmetric), and the face before the stroke
    /// under way (the changes of the hands holding it are added to it).
    face: (Vec<f32>, Vec<f32>),
    base: (Vec<f32>, Vec<f32>),
    hands: [HandState; 2],
    /// The copy: where it stands (tracking space: its face's middle and its turn).
    copy: Option<(Vec3, Quat)>,
    /// What was written to the game, and since when its face is waited for to come back.
    stroke: u32,
    sent: Option<(Vec<f32>, Vec<f32>)>,
    echo_since: Option<f64>,
    /// The hands have changed the face since it was last written.
    dirty: bool,
    /// The face drawn live (on the body and the copy).
    now: Option<Arc<FaceNow>>,
    /// Haptics this frame: (hand, seconds, frequency, amplitude); and sounds: (take hold or let
    /// go, where).
    pub haptics: Vec<(usize, f32, f32, f32)>,
    pub sounds: Vec<(bool, Vec3)>,
}

fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::from_array(a)
}

/// A hand's palm, where its grip takes hold: the middle of the palm, where the controller's grip
/// pose is (the rig's hands are drawn as grip poses).
fn palm(hand: (Vec3, Quat)) -> Vec3 {
    hand.0
}

impl Sculpt {
    /// A hand is on the face (holding it): its grip and trigger aren't the game's.
    pub fn busy(&self) -> [bool; 2] {
        [0, 1].map(|i| self.hands[i].holding.is_some() || (self.sculptable && self.hands[i].near.is_some()))
    }

    /// A hand holds the face now.
    pub fn holding(&self) -> bool {
        self.hands.iter().any(|h| h.holding.is_some())
    }

    /// The bone that carries the head being shaped (to find the player's own head on the body).
    pub fn head_bone(&self) -> Option<String> {
        self.kit.as_ref().filter(|_| self.sculptable).map(|k| k.source.head.0.clone())
    }

    /// The face may be shaped now (the game's character creator is open).
    pub fn active(&self) -> bool {
        self.sculptable && self.kit.is_some()
    }

    /// What each hand is drawn as this frame, where the face has it.
    pub fn hand_out(&self, i: usize) -> Option<&HandOut> {
        self.hands[i].out.as_ref()
    }

    /// The face drawn live now (None: the face the body was put together with).
    pub fn face_now(&self) -> Option<Arc<FaceNow>> {
        self.now.clone().filter(|_| self.active())
    }

    /// A new body's face ([`FaceKit`]), to shape from now on (None: it has none). The copy keeps
    /// its place; a hand holding the old face lets go of it.
    pub fn set_face(&mut self, kit: Option<FaceKit>) {
        for h in &mut self.hands {
            h.holding = None;
            h.near = None;
        }
        self.kit = kit.map(|FaceKit { source, sculptor, controls }| {
            self.controls = controls;
            let (lo, hi) = source.meshes.iter().flat_map(|(m, _)| &m.vertices).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| (lo.min(v3(v.pos)), hi.max(v3(v.pos))));
            Kit { source, sculptor, model: None, centre: (lo + hi) * 0.5, positions: Vec::new(), normals: Vec::new() }
        });
        self.now = None;
        self.reshaped();
    }

    /// The game's face, as it is now ([`FaceDesc`]).
    fn read(&mut self, link: &Link, now: f64) {
        let mut d: FaceDesc = bytemuck::Zeroable::zeroed();
        if !link.read_face_into(&mut d) || d.seq == self.desc_seq {
            return;
        }
        self.desc_seq = d.seq;
        let was = self.sculptable;
        self.sculptable = d.flags & (FACE_GIVEN | FACE_SCULPT) == FACE_GIVEN | FACE_SCULPT;
        self.limits = d.limits[..FACE_CONTROLS].to_vec();
        let game = (d.geometry_symmetric.to_vec(), d.geometry_asymmetric.to_vec());
        self.desc = Some(Box::new(d));
        if self.sculptable != was {
            tracing::info!("sculpt: the face {}", if self.sculptable { "may be shaped by hand (the character creator is open)" } else { "can't be shaped now" });
        }
        // While the hands hold it, the game's face is the one they made coming back. After, the
        // last they made is waited for; then the game's is the face (a slider moved, an undo).
        if self.holding() {
            return;
        }
        if let (Some(since), Some(sent)) = (self.echo_since, &self.sent) {
            let back = sent.0.iter().zip(&game.0).chain(sent.1.iter().zip(&game.1)).all(|(a, b)| (a - b).abs() < 1e-3);
            if back || now - since > ECHO_WAIT {
                self.echo_since = None;
            } else {
                return;
            }
        }
        if game != self.face {
            self.face = game;
            self.reshaped();
        }
    }

    /// One frame: the hands on the face, the face they make, what's written to the game.
    pub fn update(&mut self, link: &Link, inp: &SculptInput) {
        self.read(link, inp.now);
        for h in &mut self.hands {
            h.out = None;
        }
        if !self.active() || self.face.0.is_empty() {
            for h in &mut self.hands {
                h.holding = None;
                h.near = None;
            }
            self.copy = None;
            return;
        }
        // The copy stands where it first stood while the creator is open.
        if self.copy.is_none() {
            let (head, rot) = inp.head;
            let fwd = rot * Vec3::Z;
            let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::Z);
            let right = Vec3::Y.cross(flat);
            let at = head + flat * BUST_AHEAD + right * BUST_RIGHT - Vec3::Y * BUST_DOWN;
            // Turned to face the player: its forward towards their head.
            let to_head = Vec3::new(head.x - at.x, 0.0, head.z - at.z).normalize_or(-flat);
            self.copy = Some((at, Quat::from_rotation_arc(Vec3::Z, to_head)));
            tracing::info!("sculpt: the copy of the head stands at {at:.2?}, {BUST_SCALE}x life size");
        }
        let copy = self.copy_matrix();
        let own = inp.own_head;
        let to_tracking = |on: On| match on {
            On::Copy => copy,
            On::Own => own,
        };
        let mut changed = false;
        for i in 0..2 {
            let c = &inp.frame.hands[i];
            let hand = inp.hands[i];
            let p = palm(hand);
            let squeeze = c.squeeze;
            let pressed = squeeze > TAKE && !self.hands[i].squeezed;
            self.hands[i].squeezed = if self.hands[i].squeezed { squeeze > LET_GO } else { squeeze > TAKE };
            let ease = 1.0 - (-inp.dt / ASYM_EASE).exp();
            self.hands[i].asym += (c.trigger.clamp(0.0, 1.0) - self.hands[i].asym) * ease;
            if c.connected == 0 {
                self.let_go(i, p);
                continue;
            }
            // Holding: pulled, or let go.
            if self.hands[i].holding.is_some() {
                if !self.hands[i].squeezed {
                    self.let_go(i, p);
                    changed = true;
                    continue;
                }
                let h = self.hands[i].holding.as_mut().unwrap();
                let Some(m) = to_tracking(h.on) else {
                    continue;
                };
                let local = m.inverse().transform_point3(p);
                h.pull = local - h.from;
                changed = true;
                continue;
            }
            // Free: what it's near, which it takes at the grip.
            self.hands[i].near = self.nearest(p, copy, own);
            if pressed && let Some((on, part, vertex, _)) = self.hands[i].near {
                let kit = self.kit.as_ref().unwrap();
                let Some(m) = to_tracking(on) else { continue };
                if !self.hands.iter().any(|h| h.holding.is_some()) {
                    self.base = self.face.clone();
                    self.stroke = self.stroke.wrapping_add(1);
                }
                if let Some(hold) = kit.sculptor.hold(part, vertex, self.face.clone()) {
                    let from = m.inverse().transform_point3(p);
                    tracing::info!("sculpt: the {} hand takes hold of the {} at part {part}, vertex {vertex}", if i == 0 { "left" } else { "right" }, if on == On::Copy { "copy" } else { "own head" });
                    let n = self.face.0.len() + self.face.1.len();
                    self.hands[i].holding = Some(Holding { on, hold, from, pull: Vec3::ZERO, change: vec![0.0; n], gap: 0.0 });
                    self.hands[i].near = None;
                    self.haptics.push((i, 0.03, 160.0, 0.45));
                    self.sounds.push((true, p));
                }
            }
        }
        if changed {
            self.pulled();
        }
        // Where the face has each hand now: holding, the point taken where it got to; free, on the
        // copy's surface if pushed into it.
        for i in 0..2 {
            let p = palm(inp.hands[i]);
            let out = match &self.hands[i].holding {
                Some(h) => to_tracking(h.on).and_then(|m| {
                    let kit = self.kit.as_ref()?;
                    let got = *kit.positions.get(h.hold.part)?.get(h.hold.vertex)?;
                    let want = h.hold.at + h.pull;
                    let moved = m.transform_vector3(got - want);
                    let scale = if h.on == On::Copy { BUST_SCALE } else { 1.0 };
                    let point = m.transform_point3(got);
                    let grasp = Grasp { solids: vec![Solid::Sphere { centre: point, radius: POINT_RADIUS * scale }], fingers: rustvr_avatar::grasp::ALL_FINGERS, weight: 1.0, ..Default::default() };
                    Some(HandOut { moved, grasp: Some(grasp) })
                }),
                None => self.pushed_out(p, copy).map(|moved| HandOut { moved, grasp: None }),
            };
            // The pull felt: a fine texture while it moves, harder the further the face holds the
            // hand back.
            if let (Some(h), Some(o)) = (self.hands[i].holding.as_mut(), out.as_ref()) {
                let gap = o.moved.length();
                let moving = (gap - h.gap).abs() / inp.dt.max(1e-3);
                h.gap = gap;
                let strain = (gap / 0.04).min(1.0);
                if strain > 0.05 || moving > 0.02 {
                    self.haptics.push((i, inp.dt.max(0.011) * 1.5, 120.0 + 120.0 * strain, (0.08 + 0.6 * strain + 0.1 * moving.min(1.0)).min(0.8)));
                }
            }
            self.hands[i].out = out;
        }
        self.glow();
        self.write(link, inp.now);
    }

    /// A hand lets go: its change stays in the face.
    fn let_go(&mut self, i: usize, p: Vec3) {
        let Some(h) = self.hands[i].holding.take() else { return };
        let (ks, _) = (self.base.0.len(), self.base.1.len());
        for (k, x) in h.change.iter().enumerate() {
            if k < ks {
                self.base.0[k] += x;
            } else if let Some(v) = self.base.1.get_mut(k - ks) {
                *v += x;
            }
        }
        tracing::info!("sculpt: the {} hand lets go, pulled {:.1} cm", if i == 0 { "left" } else { "right" }, h.pull.length() * 100.0);
        self.haptics.push((i, 0.02, 120.0, 0.25));
        self.sounds.push((false, p));
        self.dirty = true;
        self.pulled();
    }

    /// The face for the hands' pulls: the stroke's base and each holding hand's change, kept in the
    /// sliders' ranges.
    fn pulled(&mut self) {
        if self.kit.is_none() {
            return;
        }
        let (mut s, mut a) = self.base.clone();
        let ks = s.len();
        for h in self.hands.iter_mut() {
            let asym = h.asym;
            let Some(hold) = h.holding.as_mut() else { continue };
            let (ds, da) = hold.hold.pulled(hold.pull, asym, None, &[]);
            hold.change = ds.iter().zip(&hold.hold.from.0).map(|(x, f)| x - f).chain(da.iter().zip(&hold.hold.from.1).map(|(x, f)| x - f)).collect();
            for (k, x) in hold.change.iter().enumerate() {
                if k < ks {
                    s[k] += x;
                } else if let Some(v) = a.get_mut(k - ks) {
                    *v += x;
                }
            }
        }
        if let Some(c) = &self.controls {
            c.limit(&mut s, &self.limits, &self.base.0);
        }
        if (s.clone(), a.clone()) != self.face {
            self.face = (s, a);
            self.dirty = true;
            self.reshaped();
        }
    }

    /// The face's vertices and normals for the face as it is now.
    fn reshaped(&mut self) {
        let Some(kit) = self.kit.as_mut() else { return };
        if self.face.0.is_empty() {
            return;
        }
        kit.positions = kit.sculptor.positions(&self.face.0, &self.face.1);
        kit.normals = kit
            .source
            .shapes()
            .iter()
            .zip(&kit.positions)
            .map(|((_, tris), ps)| {
                let mut n = vec![Vec3::ZERO; ps.len()];
                for t in tris.chunks_exact(3) {
                    let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                    if a >= ps.len() || b >= ps.len() || c >= ps.len() {
                        continue;
                    }
                    let f = (ps[b] - ps[a]).cross(ps[c] - ps[a]);
                    for v in [a, b, c] {
                        n[v] += f;
                    }
                }
                n.into_iter().map(|v| v.normalize_or_zero()).collect()
            })
            .collect();
    }

    /// The copy's model space -> tracking space.
    fn copy_matrix(&self) -> Option<Mat4> {
        let (at, rot) = self.copy?;
        let kit = self.kit.as_ref()?;
        Some(Mat4::from_scale_rotation_translation(Vec3::splat(BUST_SCALE), rot, at) * Mat4::from_translation(-kit.centre))
    }

    /// What a palm at `p` (tracking space) would take hold of: the nearer of the copy and the own
    /// head within [`REACH`] (on, part, vertex, distance in metres).
    fn nearest(&self, p: Vec3, copy: Option<Mat4>, own: Option<Mat4>) -> Option<(On, usize, usize, f32)> {
        let kit = self.kit.as_ref()?;
        [(On::Copy, copy, BUST_SCALE), (On::Own, own, 1.0)]
            .into_iter()
            .filter_map(|(on, m, scale)| {
                let m = m?;
                let local = m.inverse().transform_point3(p);
                let (part, vertex, d) = kit.sculptor.nearest(&kit.positions, local)?;
                let d = d * scale;
                (d < REACH).then_some((on, part, vertex, d))
            })
            .min_by(|a, b| a.3.total_cmp(&b.3))
    }

    /// How far a free palm at `p` pushed into the copy is moved back out onto its surface.
    fn pushed_out(&self, p: Vec3, copy: Option<Mat4>) -> Option<Vec3> {
        let (kit, m) = (self.kit.as_ref()?, copy?);
        let local = m.inverse().transform_point3(p);
        let (part, vertex, d) = kit.sculptor.nearest(&kit.positions, local)?;
        if d * BUST_SCALE > REACH {
            return None;
        }
        let q = kit.positions[part][vertex];
        let n = *kit.normals.get(part)?.get(vertex)?;
        let depth = (local - q).dot(n);
        (depth < 0.0).then(|| m.transform_vector3(-n * depth))
    }

    /// The glow on what each free hand would take, and on what each holding hand has: the hold's
    /// weights, brighter the nearer the hand.
    fn glow(&mut self) {
        let Some(kit) = self.kit.as_ref() else { return };
        let parts = kit.positions.len();
        let mut glow: Vec<Vec<u8>> = vec![Vec::new(); parts];
        let mut add = |weights: &[Vec<f32>], k: f32| {
            for (g, w) in glow.iter_mut().zip(weights) {
                if w.is_empty() {
                    continue;
                }
                if g.len() < w.len() {
                    g.resize(w.len(), 0);
                }
                for (g, w) in g.iter_mut().zip(w) {
                    *g = (*g).max((w * k * 255.0).round().clamp(0.0, 255.0) as u8);
                }
            }
        };
        for h in &self.hands {
            if let Some(hold) = &h.holding {
                add(&hold.hold.weights, 0.55);
            } else if let Some((_, part, vertex, d)) = h.near {
                let at = kit.positions[part][vertex];
                add(&kit.sculptor.weights(&kit.positions, at), 1.0 - (d / REACH).clamp(0.0, 1.0) * 0.6);
            }
        }
        let now = FaceNow { symmetric: self.face.0.clone(), asymmetric: self.face.1.clone(), glow };
        if self.now.as_deref() != Some(&now) {
            self.now = Some(Arc::new(now));
            if let (Some(kit), Some(n)) = (self.kit.as_ref(), &self.now) {
                if let Some(m) = &kit.model {
                    m.set_face(Some(n.clone()));
                }
            }
        }
    }

    /// The face to the game, when the hands have changed it.
    fn write(&mut self, link: &Link, now: f64) {
        // (Only what the hands made: a face from the game isn't sent back to it.)
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let held = self.holding();
        let face = (self.face.0.clone(), self.face.1.clone());
        let mut sculpt: FaceSculpt = bytemuck::Zeroable::zeroed();
        sculpt.flags = if held { SCULPT_HELD } else { 0 };
        sculpt.stroke = self.stroke;
        for (to, from) in sculpt.geometry_symmetric.iter_mut().zip(&face.0) {
            *to = *from;
        }
        for (to, from) in sculpt.geometry_asymmetric.iter_mut().zip(&face.1) {
            *to = *from;
        }
        link.write_face_sculpt(&sculpt);
        self.sent = Some(face);
        if !held {
            self.echo_since = Some(now);
        }
    }

    /// The copy of the head to draw (made on the GPU the first time).
    pub fn prepare(&mut self, gpu: &rustvr_gpu::Gpu) {
        if !self.active() || self.copy.is_none() {
            return;
        }
        let Some(kit) = self.kit.as_mut() else { return };
        if kit.model.is_none() {
            match HeldModel::face(gpu, &kit.source) {
                Ok(m) => {
                    m.set_face(self.now.clone());
                    kit.model = Some(m);
                }
                Err(e) => {
                    tracing::warn!("sculpt: the copy of the head: {e:#}");
                    self.kit = None;
                }
            }
        }
    }

    /// The copy's draw, with its one part's place (`parts`, filled here).
    pub fn draw<'a>(&'a self, parts: &'a mut [Option<Mat4>; 1]) -> Option<HeldDraw<'a>> {
        let m = self.copy_matrix()?;
        let model = self.kit.as_ref()?.model.as_ref()?;
        parts[0] = Some(m);
        let parts: &'a [Option<Mat4>] = parts;
        Some(HeldDraw { model, parts, key: "sculpt/copy", overlay: None, bend: 0.0 })
    }
}
