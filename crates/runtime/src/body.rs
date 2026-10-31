//! The player's body: solved by IK (`rustvr_ik`) from the head and the hands (the rig's physics
//! hands when it owns them, else the controllers), published as [`RtBody`], and drawn over the
//! game's picture by the avatar (`rustvr_avatar`), hidden behind the game's geometry using its depth.

use std::cell::RefCell;
use std::rc::Rc;

use glam::{Quat, Vec3};
use rustvr_avatar::collimator::Collimator;
use rustvr_avatar::{Avatar, AvatarFrame, AvatarSource, DepthInput, GameShading, Grasp, HeldHand};
use rustvr_gpu::Gpu;
use rustvr_ik::{BodyParams, BodySolver, Calibration, FingerInput, FingerSolver, HandOffsets, HoldShape, Side, StraightArms, TrackedInput};
use rustvr_protocol::*;

use crate::backend::{EyeTarget, HmdFrame};
use crate::settings::{BodyVisibility, Settings};

/// What the body is solved from this frame.
pub struct BodyInput<'a> {
    pub frame: &'a HmdFrame,
    /// Where the hands are drawn, as controller grip poses in tracking space (the rig's physics
    /// hands); None: the controllers themselves.
    pub hands: Option<[(Vec3, Quat); 2]>,
    /// The rig's hand states (blocked, holding), if it owns the hands.
    pub hand_states: Option<[HandState; 2]>,
    /// Controller grips to show as ghost hands (a physics hand is held back).
    pub ghosts: [Option<(Vec3, Quat)>; 2],
    pub holds: [HoldShape; 2],
    /// Hands holding something in the game's own pose for it (`rustvr_avatar::held_hands`): the
    /// hand goes exactly to that wrist and is drawn in that pose.
    pub held: [Option<HeldHand>; 2],
    /// Hands holding something otherwise: what their fingers close on (the rig's grasps).
    pub grasps: [Option<&'a Grasp>; 2],
    /// How far each finger (thumb .. little) is pushed open from its curl by what the empty hand is
    /// pressed on (the rig's, 0 or less).
    pub finger_give: [[f32; 5]; 2],
    /// Stick locomotion velocity in tracking space (m/s), for the legs' gait.
    pub locomotion: Vec3,
    /// The player is swimming in the game's water: the body floats.
    pub swim: Option<rustvr_ik::Swimming>,
    /// Standing head height as measured in tracking space (0: not known yet, or the game is
    /// crouching: the body keeps the size it had).
    pub eye_height: f32,
    /// Where the game's floor is in tracking space (the body stands on it, whatever the real
    /// floor's height: the game puts your eyes at its own eye height).
    pub floor: f32,
    /// Riding: the game's seat on its mount (tracking space), where the body sits instead.
    pub saddle: Option<rustvr_ik::Saddle>,
    pub settings: &'a Settings,
}

/// A special draw for held meshes (see [`PlayerBody::special`]).
pub type SpecialDrawFn = Box<dyn FnMut(&rustvr_avatar::SpecialDraw)>;

pub struct PlayerBody {
    solver: BodySolver,
    offsets: HandOffsets,
    fingers: [FingerSolver; 2],
    avatar: Option<Avatar>,
    avatar_tried: bool,
    /// The body the game makes up (`rustvr_protocol::AvatarDesc`), as last asked for, and being
    /// put together off the frame.
    game_body_seq: u32,
    game_body: Option<std::sync::mpsc::Receiver<anyhow::Result<AvatarSource>>>,
    /// The body's parts as last asked for ((game, skeleton), parts, swaps), and its face
    /// (`rustvr_protocol::FaceDesc`, by its seq): a new face puts the same parts together again.
    game_parts: Option<((String, String), Vec<String>, Vec<(String, String)>)>,
    game_face_seq: u32,
    game_face: Option<rustvr_avatar::facegen::Face>,
    /// A body is being put together while another change came in: put it together again after.
    game_body_stale: bool,
    pub body: RtBody,
    /// Ghost hands (canonical hand frames), drawn translucent.
    pub ghosts: [Option<Pose>; 2],
    /// Hands drawn in the game's own pose this frame.
    pub held: [Option<HeldHand>; 2],
    /// What the other holding hands' fingers close on this frame.
    pub grasp: [Option<Grasp>; 2],
    pub reached: [bool; 2],
    seq: u32,
    /// Draws the held meshes the avatar doesn't shade itself (see `rustvr_avatar::SpecialDraw`:
    /// a collimator's lens and reticle); None leaves them out.
    pub special: Option<SpecialDrawFn>,
    /// The collimator sights' draw (`rustvr_avatar::collimator`), installed as `special` when
    /// nothing else is.
    collimator: Option<Rc<RefCell<Collimator>>>,
    /// Automatic arm span: grows while a hand keeps reaching beyond the arms.
    auto_span: Option<f32>,
    /// The hands' size the player's are drawn at (wrist to the middle finger's knuckle, metres),
    /// and the size last said in the log.
    hand_length: f32,
    hand_said: f32,
    /// Seconds since the arms were last logged.
    arms_said: f32,
    unreached: [f32; 2],
    /// Each hand on its way into a held pose or out of one (see [`Grab`]).
    grab: [Grab; 2],
    /// The plain look (the framework view).
    plain: bool,
}

/// A hand going into the game's pose for what it takes, and back out letting go, over
/// `Interaction::grab_transition`: how far it's gone (0 its own, 1 the held pose), and the held
/// pose it's leaving (for the way out).
#[derive(Clone, Copy, Default)]
struct Grab {
    weight: f32,
    last: Option<HeldHand>,
}

impl Grab {
    /// One frame: towards the held pose while there is one, back to the hand's own after. The
    /// hand drawn: the held pose part way from the hand's own wrist (`own`), its fingers by the same
    /// weight (`HeldHand::weight`).
    ///
    /// `transition`: the grab transition setting; a wrist with further to go takes longer
    /// (`rig::travel_time`).
    fn step(&mut self, held: Option<HeldHand>, own: (Vec3, Quat), dt: f32, transition: f32) -> Option<HeldHand> {
        let to = held.or(self.last).map_or(0.0, |h| own.0.distance(Vec3::from_array(h.wrist.pos)));
        let time = crate::rig::travel_time(transition, to);
        let rate = if time > 1e-3 { dt / time } else { 1.0 };
        match held {
            Some(h) => {
                self.weight = (self.weight + rate).min(1.0);
                self.last = Some(h);
            }
            None => self.weight = (self.weight - rate).max(0.0),
        }
        let base = held.or(self.last).filter(|_| self.weight > 0.0)?;
        let w = self.weight * self.weight * (3.0 - 2.0 * self.weight);
        let (p, q) = (Vec3::from_array(base.wrist.pos), quat(base.wrist.rot));
        let wrist = Pose { pos: own.0.lerp(p, w).to_array(), valid: 1, rot: own.1.slerp(q, w).normalize().to_array() };
        Some(HeldHand { wrist, weight: w, ..base })
    }
}

impl Default for PlayerBody {
    fn default() -> Self {
        let params = BodyParams::default();
        PlayerBody {
            offsets: params.hand_offsets,
            solver: BodySolver::new(params, Calibration { eye_height: 1.70, arm_span: grip_span(1.70) }),
            fingers: [FingerSolver::new(), FingerSolver::new()],
            avatar: None,
            avatar_tried: false,
            game_body_seq: 0,
            game_body: None,
            game_parts: None,
            game_face_seq: 0,
            game_face: None,
            game_body_stale: false,
            body: bytemuck::Zeroable::zeroed(),
            ghosts: [None; 2],
            held: [None; 2],
            grasp: [None, None],
            reached: [true; 2],
            seq: 0,
            special: None,
            collimator: None,
            auto_span: None,
            hand_length: 0.0,
            hand_said: 0.0,
            arms_said: 0.0,
            unreached: [0.0; 2],
            grab: [Grab::default(); 2],
            plain: false,
        }
    }
}

fn ik(p: (Vec3, Quat)) -> rustvr_ik::Pose {
    rustvr_ik::Pose { pos: p.0, rot: p.1.normalize() }
}

fn proto(p: rustvr_ik::Pose) -> Pose {
    Pose { pos: p.pos.to_array(), valid: 1, rot: p.rot.normalize().to_array() }
}

fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

fn grip_of(c: &Controller) -> (Vec3, Quat) {
    let q = Quat::from_array(c.grip.rot);
    (Vec3::from_array(c.grip.pos), if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() })
}

const SIDES: [Side; 2] = [Side::Left, Side::Right];
/// Wrist to the middle finger's knuckle against a person's height (palm length: about 10.3 cm in
/// a 1.75 m adult).
const HAND_PER_HEIGHT: f32 = 0.059;
/// A grown-up's height (metres), for the hands when the player hasn't said theirs.
const TYPICAL_HEIGHT: f32 = 1.75;

/// Grip-to-grip distance with the arms out, estimated from the eye height: the arm span about
/// equals the height (eyes 11 cm below the top of the head), less most of the hands beyond the
/// grips. A little generous: the automatic span only ever grows.
pub fn grip_span(eye_height: f32) -> f32 {
    eye_height + 0.11 - 0.15
}

/// A T-pose measurement: (height, grip-to-grip span) from the head and both grips, if they look
/// like one (both hands out level with the shoulders).
pub fn measure_tpose(head: Vec3, left: Vec3, right: Vec3) -> Option<(f32, f32)> {
    let span = left.distance(right);
    let level = (left.y - (head.y - 0.25)).abs() < 0.25 && (right.y - (head.y - 0.25)).abs() < 0.25;
    (level && (0.9..2.6).contains(&span) && (1.0..2.4).contains(&head.y)).then_some((head.y + 0.11, span))
}

/// Both arms held straight out in front: both grips well ahead of the head (the way it faces),
/// about shoulder height, about shoulder width apart.
pub fn arms_in_front(head: (Vec3, Quat), left: Vec3, right: Vec3) -> bool {
    let fwd = head.1 * Vec3::Z;
    let Some(fwd) = Vec3::new(fwd.x, 0.0, fwd.z).try_normalize() else { return false };
    let out = |g: Vec3| (g - head.0).dot(fwd) > 0.3 && (g.y - (head.0.y - 0.25)).abs() < 0.3;
    out(left) && out(right) && left.distance(right) < 0.9
}

impl PlayerBody {
    /// The arm span at which the body's arms are just straight in every pose given (the arms out
    /// to the sides and in front): as the body solver lays the arm out, so a player's straight
    /// arm is drawn straight.
    pub fn straight_arm_span(&self, poses: &[StraightArms]) -> Option<f32> {
        self.solver.straight_arm_span(poses)
    }

    /// The body's calibration: the player's height from the settings, else the measured standing
    /// eye height; the arm span from the settings, else about the height.
    fn calibration(&self, inp: &BodyInput) -> Option<Calibration> {
        let b = &inp.settings.body;
        // The body's standing eye height is the view's height above the game's floor.
        let eye = inp.eye_height - inp.floor;
        if inp.eye_height <= 0.0 || !(1.0..2.4).contains(&eye) {
            return None;
        }
        let real_eye = if b.height > 1.0 { b.height - 0.11 } else { eye };
        let span = if b.arm_span > 1.0 { b.arm_span } else { self.auto_span.unwrap_or(grip_span(real_eye)) };
        Some(Calibration { eye_height: eye, arm_span: span })
    }

    /// The automatic arm span now (for the menu).
    pub fn arm_span(&self) -> f32 {
        self.solver.calibration().arm_span
    }

    pub fn update(&mut self, inp: &BodyInput) -> &RtBody {
        // The hands a real hand's size for the player's height (wrist to the middle finger's
        // knuckle is about 5.9% of a person's height), times the setting: the height they've set,
        // else a grown-up's. (Never the head's height as measured: a seated player's, it changes
        // as they move.)
        let height = if inp.settings.body.height > 1.0 { inp.settings.body.height } else { TYPICAL_HEIGHT };
        self.hand_length = HAND_PER_HEIGHT * height * inp.settings.body.hand_size.clamp(0.5, 2.0);
        let f = inp.frame;
        if let Some(c) = self.calibration(inp) {
            let cur = self.solver.calibration();
            if (cur.eye_height - c.eye_height).abs() > 0.02 || (cur.arm_span - c.arm_span).abs() > 0.03 {
                self.solver.set_calibration(c);
            }
        }
        let head = (Vec3::from_array(f.hmd.pos), {
            let q = Quat::from_array(f.hmd.rot);
            if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
        });
        // Solve with the game's floor at 0, then put the body back where it stands.
        let down = Vec3::new(0.0, inp.floor, 0.0);
        // Hands taking hold of something (or letting go) go into its pose over the transition.
        let held = [0, 1].map(|i| {
            let g = inp.hands.map(|h| h[i]).unwrap_or_else(|| grip_of(&f.hands[i]));
            let own = self.offsets.hand_frame(SIDES[i], ik(g));
            self.grab[i].step(inp.held[i], (own.pos, own.rot), f.dt.clamp(1e-4, 0.1), inp.settings.interaction.grab_transition)
        });
        let grips = [0, 1].map(|i| {
            (f.hands[i].connected != 0 && f.hands[i].grip.valid != 0).then(|| {
                // A hand in the game's own pose goes exactly to its wrist: the grip whose hand
                // joint that is.
                if let Some(h) = &held[i] {
                    let wrist = rustvr_ik::Pose { pos: Vec3::from_array(h.wrist.pos) - down, rot: quat(h.wrist.rot) };
                    return self.offsets.grip_for(SIDES[i], wrist);
                }
                let g = inp.hands.map(|h| h[i]).unwrap_or_else(|| grip_of(&f.hands[i]));
                ik((g.0 - down, g.1))
            })
        });
        // Seated players still play a standing character: the body is drawn standing.
        // The elbows and the shoulders where the headset tracks them (when the player lets it).
        let tracked = |joints: [Option<[f32; 3]>; 2]| joints.map(|e| e.filter(|_| inp.settings.body.tracked_elbows).map(|e| Vec3::from_array(e) - down));
        let (elbows, shoulders) = (tracked(f.elbows), tracked(f.shoulders));
        // In a saddle the body sits where the game seats its rider.
        self.solver.ride(inp.saddle.map(|s| rustvr_ik::Saddle {
            seat: rustvr_ik::Pose { pos: s.seat.pos - down, rot: s.seat.rot },
            legs: s.legs.map(|l| l.map(|leg| leg.map(|p| p - down))),
        }));
        let mut out = self.solver.solve(&TrackedInput { dt: f.dt.clamp(1e-4, 0.1), head: ik((head.0 - down, head.1)), grips, locomotion: inp.locomotion, seated: false, elbows, shoulders, swim: inp.swim });
        for j in out.joints.iter_mut().chain(out.hand_targets.iter_mut()) {
            j.pos += down;
        }
        self.reached = out.reached;
        // Each arm's elbow and hand as the chest has them (the left's mirrored onto the right's
        // side), now and then: two arms held alike read alike, so a side solved differently shows.
        self.arms_said += f.dt.max(0.0);
        if self.arms_said > 2.0 && grips.iter().all(|g| g.is_some()) {
            self.arms_said = 0.0;
            let chest = out.joints[Joint::UpperChest as usize];
            let local = |v: Vec3, side: usize| {
                let l = chest.rot.inverse() * v;
                if side == 0 { Vec3::new(-l.x, l.y, l.z) } else { l }
            };
            let arm = |side: usize| {
                let [upper, lower, hand] = if side == 0 {
                    [Joint::LeftUpperArm, Joint::LeftLowerArm, Joint::LeftHand]
                } else {
                    [Joint::RightUpperArm, Joint::RightLowerArm, Joint::RightHand]
                };
                let p = |j: Joint| out.joints[j as usize].pos;
                let h = out.joints[hand as usize];
                (
                    local(p(hand) - p(upper), side),
                    local((p(lower) - p(upper)).normalize_or_zero(), side),
                    local(h.rot * Vec3::Z, side),
                    local(h.rot * Vec3::Y, side),
                )
            };
            let (l, r) = (arm(0), arm(1));
            tracing::info!(
                "body: arms (chest space, the left mirrored) hand at {:.2?} / {:.2?}, upper arm {:.2?} / {:.2?}, thumb {:.2?} / {:.2?}, fingers {:.2?} / {:.2?}",
                l.0, r.0, l.1, r.1, l.2, r.2, l.3, r.3
            );
        }
        // Arms that keep falling short of the hands grow (automatic arm span only).
        if inp.settings.body.arm_span <= 1.0 {
            let dt = f.dt.clamp(0.0, 0.1);
            for i in 0..2 {
                self.unreached[i] = if out.reached[i] || grips[i].is_none() { 0.0 } else { self.unreached[i] + dt };
            }
            if self.unreached.iter().any(|t| *t > 0.25) {
                let span = self.auto_span.unwrap_or(self.solver.calibration().arm_span);
                self.auto_span = Some((span + 0.25 * dt).min(2.4));
            }
        }
        self.seq = self.seq.wrapping_add(1);
        let mut body: RtBody = bytemuck::Zeroable::zeroed();
        body.seq = self.seq;
        body.flags = BODY_VALID;
        body.frame_index = f.index;
        body.eye_height = self.solver.calibration().eye_height;
        body.scale = out.scale;
        body.arm_length = out.arm_length;
        body.visibility = match inp.settings.body.visibility {
            BodyVisibility::Hands => BODYVIS_HANDS,
            BodyVisibility::Arms => BODYVIS_ARMS,
            BodyVisibility::Full => BODYVIS_FULL,
        };
        body.joints = out.to_protocol();
        for i in 0..2 {
            let c = &f.hands[i];
            let rig = inp.hand_states.map(|h| h[i]);
            body.hands[i] = HandState {
                pose: proto(out.hand_targets[i]),
                flags: rig.map(|h| h.flags).unwrap_or(if grips[i].is_some() { HAND_TRACKED } else { 0 }),
                holding: rig.map(|h| h.holding).unwrap_or(0),
                _pad: [0; 2],
            };
            let fi = FingerInput::from_hand(c, f.fingers[i]);
            body.fingers[i] = self.fingers[i].update(f.dt.clamp(1e-4, 0.1), &fi, inp.holds[i]);
            // Pressed on something, the fingers are pushed open (and the hand spreads a little).
            let give = inp.finger_give[i];
            for (c, g) in body.fingers[i].curl.iter_mut().zip(give) {
                *c += g;
            }
            body.fingers[i].splay = (body.fingers[i].splay - 0.4 * give.iter().sum::<f32>() / 5.0).min(1.0);
            self.ghosts[i] = inp.ghosts[i].map(|g| proto(self.offsets.hand_frame(SIDES[i], ik(g))));
        }
        self.held = [0, 1].map(|i| held[i].filter(|_| grips[i].is_some()));
        self.grasp = [0, 1].map(|i| inp.grasps[i].filter(|_| grips[i].is_some()).cloned());
        self.body = body;
        &self.body
    }

    /// The body the game makes up, when it or its face has changed: put together from its parts off
    /// the frame (`rustvr_avatar::compose`, the face's morphs on the parts made to it), then drawn
    /// instead of the avatar so far. (While one is being put together a newer change waits for it,
    /// so a character creator's slider dragged along makes one body at a time, the latest.)
    pub fn read_game_body(&mut self, link: &rustvr_protocol::link::Link) {
        let mut d: rustvr_protocol::AvatarDesc = bytemuck::Zeroable::zeroed();
        if link.read_avatar_into(&mut d) && d.seq != self.game_body_seq {
            self.game_body_seq = d.seq;
            let split = |name: &str| name.split_once('/').map(|(g, k)| (g.to_owned(), k.to_owned()));
            match split(rustvr_protocol::name_str(&d.skeleton)) {
                Some(skeleton) => {
                    let parts: Vec<String> = (0..(d.count as usize).min(rustvr_protocol::MAX_AVATAR_PARTS)).filter_map(|i| split(d.part(i)).map(|(_, k)| k)).collect();
                    let swaps: Vec<(String, String)> =
                        (0..(d.swap_count as usize).min(rustvr_protocol::MAX_AVATAR_SWAPS)).map(|i| d.swap(i)).map(|(a, b)| (a.to_owned(), b.to_owned())).collect();
                    tracing::info!("runtime: the game's body: {}/{} with {} parts ({}), {} texture swaps", skeleton.0, skeleton.1, parts.len(), parts.join(", "), swaps.len());
                    self.game_parts = Some((skeleton, parts, swaps));
                    self.game_body_stale = true;
                }
                None => tracing::warn!("runtime: the game's body names no skeleton"),
            }
        }
        let mut f: rustvr_protocol::FaceDesc = bytemuck::Zeroable::zeroed();
        if link.read_face_into(&mut f) && f.seq != self.game_face_seq {
            self.game_face_seq = f.seq;
            let face = (f.flags & rustvr_protocol::FACE_GIVEN != 0).then(|| rustvr_avatar::facegen::Face {
                coords: rustvr_asset::FaceCoords {
                    geometry_symmetric: f.geometry_symmetric.to_vec(),
                    geometry_asymmetric: f.geometry_asymmetric.to_vec(),
                    texture_symmetric: f.texture_symmetric.to_vec(),
                },
                hair: [f.hair_color[0], f.hair_color[1], f.hair_color[2]],
                detail: Some(f.detail()).filter(|(on, _, _)| !on.is_empty()).map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned())),
                eyes: [0, 1].map(|s| f.eye(s).split_once('/').map(|(_, k)| k.to_owned())),
            });
            if face != self.game_face {
                self.game_face = face;
                self.game_body_stale = self.game_parts.is_some();
            }
        }
        if !self.game_body_stale || self.game_body.is_some() {
            return;
        }
        let Some(((game, skeleton), parts, swaps)) = self.game_parts.clone() else { return };
        self.game_body_stale = false;
        let face = self.game_face.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = rustvr_asset::default_root().join(&game);
        std::thread::spawn(move || {
            // The armour's plates, as the game's marks say (`rustvr_asset::armour`); without marks
            // everything is soft, its joints keeping their volume.
            let marks = rustvr_asset::armour::load(&game, &dir).map_err(|e| tracing::warn!("runtime: the game's armour marks: {e:#}")).ok();
            match (&marks, rustvr_asset::armour::path(&game)) {
                (Some(m), Some(p)) => tracing::info!("runtime: armour marks for {} pieces from {}", m.pieces.len(), p.display()),
                _ => tracing::info!("runtime: no armour marks for {game}: every piece soft"),
            }
            let mut skinning = marks.as_ref().map_or(rustvr_avatar::Skinning::Centres, rustvr_avatar::Skinning::Marked);
            // (`RUSTVR_SKINNING=linear` or `centres`: the game's own skinning, or no plates, to
            // compare against.)
            match std::env::var("RUSTVR_SKINNING").as_deref() {
                Ok("linear") => skinning = rustvr_avatar::Skinning::Linear,
                Ok("centres") => skinning = rustvr_avatar::Skinning::Centres,
                _ => {}
            }
            let t = std::time::Instant::now();
            let built = rustvr_avatar::compose(&dir, &skeleton, &parts, &swaps, face.as_ref(), skinning);
            tracing::debug!("runtime: the game's body put together in {:.0} ms", t.elapsed().as_secs_f32() * 1000.0);
            let _ = tx.send(built);
        });
        self.game_body = Some(rx);
    }

    /// Loads the avatar the first time it's needed: Metro's Miller when the extracted assets are
    /// there (local test content), else the built-in mannequin; later, the body the game makes up.
    fn avatar(&mut self, gpu: &Gpu) -> Option<&mut Avatar> {
        let received = self.game_body.as_ref().map(|rx| rx.try_recv());
        if let Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) = received {
            tracing::error!("runtime: putting the game's body together stopped");
            self.game_body = None;
        }
        if let Some(Ok(built)) = received {
            self.game_body = None;
            self.avatar_tried = true;
            let t = std::time::Instant::now();
            // (A body already drawn keeps its renderer: only the new body goes onto the GPU.)
            let made = match self.avatar.as_mut() {
                Some(a) => built.and_then(|source| a.set_source(gpu, source)),
                None => built.and_then(|source| Avatar::new(gpu, source)).map(|a| self.avatar = Some(a)),
            };
            match made {
                Ok(()) => {
                    tracing::debug!("runtime: drawing the game's body (on the GPU in {:.0} ms)", t.elapsed().as_secs_f32() * 1000.0);
                    self.take_proportions();
                }
                Err(e) => tracing::error!("runtime: the game's body: {e:#}"),
            }
        }
        if !self.avatar_tried {
            self.avatar_tried = true;
            let dir = rustvr_asset::default_root().join("metro2033");
            let source = if dir.join("models").join("body_miller.rvm").exists() {
                AvatarSource::Model { dir, body: "body_miller".into(), hands: Some("hands_sviter".into()) }
            } else {
                AvatarSource::Procedural
            };
            match Avatar::new(gpu, source) {
                Ok(a) => {
                    self.avatar = Some(a);
                    self.take_proportions();
                }
                Err(e) => tracing::error!("runtime: no avatar: {e:#}"),
            }
        }
        self.avatar.as_mut()
    }

    /// The body solver's arms as long as the player's, shared out as the drawn model's are.
    fn take_proportions(&mut self) {
        if let Some(share) = self.avatar.as_ref().and_then(|a| a.forearm_share()) {
            tracing::info!("runtime: the arms keep the model's proportions (forearm {:.0}% of the arm)", share * 100.0);
            self.solver.set_forearm_share(share);
        }
    }

    /// Shades the body with the game's lighting from now on (None: the runtime's own look). Once a
    /// frame, before the eyes.
    pub fn set_game_shading(&mut self, gpu: &Gpu, shading: Option<&GameShading>) {
        // (Without the game's lighting there's nothing to load the avatar for yet.)
        let avatar = if shading.is_some() { self.avatar(gpu) } else { self.avatar.as_mut() };
        if let Some(avatar) = avatar {
            avatar.set_game_shading(gpu, shading);
        }
        // A collimator's lens and reticle: drawn by `rustvr_avatar::collimator` through `special`
        // (unless something else took it), under the same lighting.
        if self.collimator.is_none() && self.special.is_none() {
            match Collimator::new(gpu, rustvr_asset::default_root()) {
                Ok(c) => {
                    let c = Rc::new(RefCell::new(c));
                    let draw = c.clone();
                    self.special = Some(Box::new(move |sd| draw.borrow_mut().draw(sd)));
                    self.collimator = Some(c);
                }
                Err(e) => tracing::error!("runtime: no collimator draw: {e:#}"),
            }
        }
        if let Some(c) = &self.collimator {
            c.borrow_mut().set_shading(gpu, shading);
        }
    }

    /// The plain look (one colour, no textures: the framework view) from now on, or not.
    pub fn set_plain(&mut self, plain: bool) {
        self.plain = plain;
    }

    /// Testing: the body in one flat colour and what the runtime draws with it in another (None:
    /// shaded as ever).
    pub fn set_flat(&mut self, colors: Option<([f32; 3], [f32; 3])>) {
        if let Some(a) = self.avatar.as_mut() {
            a.set_flat(colors);
        }
    }

    /// The held gun's laser from now on (None: none), drawn with the body.
    /// The game's water near the player (tracking space), from now on: the body under its
    /// surface is seen through it.
    pub fn set_water(&mut self, water: Option<rustvr_avatar::Water>) {
        if let Some(a) = self.avatar.as_mut() {
            a.set_water(water);
        }
    }

    pub fn set_laser(&mut self, gpu: &Gpu, laser: Option<rustvr_avatar::LaserBeam>) {
        if let Some(a) = self.avatar.as_mut() {
            a.set_laser(gpu, laser);
        }
    }

    /// The game's effect shaders over the body and hands from now on (a spell's shader on the
    /// player).
    pub fn set_body_overlays(&mut self, overlays: Vec<rustvr_avatar::OverlayDraw>) {
        if let Some(a) = self.avatar.as_mut() {
            a.set_body_overlays(overlays);
        }
    }

    /// The blood to draw into the next eye (`crate::gore`; `None`: none).
    pub fn set_gore(&mut self, gpu: &Gpu, frame: Option<&rustvr_avatar::gore::GoreFrame>) {
        if let Some(a) = self.avatar.as_mut() {
            a.set_gore(gpu, frame);
        }
    }

    /// The sheets on what the hands hold from now on (the game's menus held in them), drawn with
    /// the body.
    pub fn set_sheets(&mut self, sheets: Vec<rustvr_avatar::Sheet>) {
        if let Some(a) = self.avatar.as_mut() {
            a.set_sheets(sheets);
        }
    }

    /// Draws the body into one eye, with what the runtime draws of what the player holds (`holding`,
    /// see `crate::held_objects`). `light.sun_dir` must already be in tracking space.
    pub fn draw(&mut self, gpu: &Gpu, target: &EyeTarget, eye: &Pose, fov: &Fov, light: LightHint, depth: Option<DepthInput>, holding: &[rustvr_avatar::HeldDraw]) {
        let body = self.body;
        let ghosts = self.ghosts;
        let held = self.held;
        // (The avatar's loaded first: `special` is borrowed beside it.)
        self.avatar(gpu);
        let hand = (self.hand_length > 0.02).then_some(self.hand_length);
        let Some(avatar) = self.avatar.as_mut() else { return };
        avatar.set_hand_length(hand);
        if let Some(l) = hand
            && (l - self.hand_said).abs() > 0.002
        {
            self.hand_said = l;
            tracing::info!("runtime: hands drawn {:.1} cm wrist to knuckle (the model's {:.1} cm)", l * 100.0, avatar.model_hand_length() * 100.0);
        }
        let frame = AvatarFrame {
            joints: &body.joints,
            fingers: body.fingers,
            hands: body.hands,
            ghosts,
            held,
            grasp: [self.grasp[0].as_ref(), self.grasp[1].as_ref()],
            visibility: body.visibility,
            light,
            first_person: true,
            plain: self.plain,
        };
        let special = self.special.as_deref_mut().map(|f| f as &mut dyn FnMut(&rustvr_avatar::SpecialDraw));
        if let Err(e) = avatar.draw_with(gpu, &target.rtv, target.width, target.height, eye, fov, &frame, depth, true, holding, special) {
            tracing::warn!("runtime: drawing the avatar: {e:#}");
        }
    }

    /// Draws the body as a mirror shows it (`crate::mirror`): from `eye` (a camera reflected in the
    /// glass) into `target`, the whole body whatever the body setting, with the head (the wearer
    /// isn't inside it), what the hands hold, no ghost hands; hidden behind the game's world as it
    /// shows there (`depth`). The eyes look at `gaze` (the wearer's own eyes' reflection: they meet
    /// your eyes in the mirror).
    pub fn draw_reflection(&mut self, gpu: &Gpu, target: &EyeTarget, eye: &Pose, fov: &Fov, light: LightHint, depth: Option<DepthInput>, holding: &[rustvr_avatar::HeldDraw], gaze: Option<Vec3>) {
        let body = self.body;
        let held = self.held;
        self.avatar(gpu);
        let hand = (self.hand_length > 0.02).then_some(self.hand_length);
        let Some(avatar) = self.avatar.as_mut() else { return };
        avatar.set_hand_length(hand);
        let frame = AvatarFrame {
            joints: &body.joints,
            fingers: body.fingers,
            hands: body.hands,
            ghosts: [None; 2],
            held,
            grasp: [self.grasp[0].as_ref(), self.grasp[1].as_ref()],
            visibility: BODYVIS_FULL,
            light,
            first_person: false,
            plain: self.plain,
        };
        avatar.set_gaze(gaze);
        if let Err(e) = avatar.draw_with(gpu, &target.rtv, target.width, target.height, eye, fov, &frame, depth, true, holding, None) {
            tracing::warn!("runtime: drawing the body in the mirror: {e:#}");
        }
        avatar.set_gaze(None);
    }

    /// The body's depth (and what it holds), as [`Body::draw`] last drew it from `eye`, into the
    /// headset's depth for that eye where it's nearer (the compositor's projection).
    pub fn draw_depth(&mut self, gpu: &Gpu, target: &crate::backend::DepthTarget, eye: &Pose, fov: &Fov, holding: &[rustvr_avatar::HeldDraw]) {
        let body = self.body;
        let Some(avatar) = self.avatar.as_mut() else { return };
        let frame = AvatarFrame {
            joints: &body.joints,
            fingers: body.fingers,
            hands: body.hands,
            ghosts: self.ghosts,
            held: self.held,
            grasp: [self.grasp[0].as_ref(), self.grasp[1].as_ref()],
            visibility: body.visibility,
            light: LightHint::default(),
            first_person: true,
            plain: self.plain,
        };
        let proj = crate::compositor::projection(fov);
        if let Err(e) = avatar.draw_game_depth(gpu, &target.dsv, target.width, target.height, eye, fov, proj, &frame, holding) {
            tracing::warn!("runtime: the body's depth for the headset: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustvr_ik::Pose as IkPose;

    #[test]
    fn a_hand_takes_hold_over_the_transition_and_lets_go_the_same_way() {
        let held = HeldHand {
            wrist: Pose { pos: [0.3, 1.2, 0.5], valid: 1, rot: Quat::from_rotation_x(0.6).to_array() },
            fingers: [Quat::from_rotation_z(-0.8); 15],
            index_pulled: None,
            trigger: 0.0,
            size: 0.1,
            reach: [None; 5],
            index_reach_pulled: None,
            weight: 1.0,
        };
        let own = (Vec3::new(0.2, 1.1, 0.4), Quat::IDENTITY);
        let mut g = Grab::default();
        let dt = 1.0 / 90.0;
        // Taking hold: part way after the first frame, all the way after the transition.
        let first = g.step(Some(held), own, dt, 0.15).unwrap();
        assert!(first.weight > 0.0 && first.weight < 0.1, "{}", first.weight);
        assert!(Vec3::from_array(first.wrist.pos).distance(own.0) < 0.01, "the wrist starts where the hand is");
        let mut last = first;
        for _ in 0..14 {
            last = g.step(Some(held), own, dt, 0.15).unwrap();
        }
        assert_eq!(last.weight, 1.0);
        assert_eq!(last.wrist.pos, held.wrist.pos, "all the way into the hold");
        // Letting go: back out over the same time, then the hand's own.
        let out = g.step(None, own, dt, 0.15).unwrap();
        assert!(out.weight < 1.0 && out.weight > 0.9);
        for _ in 0..14 {
            g.step(None, own, dt, 0.15);
        }
        assert!(g.step(None, own, dt, 0.15).is_none(), "its own hand again");
        // No transition: straight in.
        let mut g = Grab::default();
        assert_eq!(g.step(Some(held), own, dt, 0.0).unwrap().weight, 1.0);
    }

    #[test]
    fn automatic_arms_grow_to_reach_the_hands() {
        let settings = Settings::default();
        let mut body = PlayerBody::default();
        let mut f = HmdFrame { dt: 1.0 / 90.0, ..Default::default() };
        f.hmd = Pose { pos: [0.0, 1.7, 0.0], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
        // Arms straight out in front, further than the default arms reach.
        for (i, x) in [(0, -0.18), (1, 0.18)] {
            f.hands[i].connected = 1;
            f.hands[i].grip = Pose { pos: [x, 1.45, 0.78], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
        }
        let before = body.arm_span();
        for _ in 0..360 {
            body.update(&BodyInput {
                frame: &f,
                hands: None,
                hand_states: None,
                ghosts: [None; 2],
                holds: [HoldShape::None; 2],
                held: [None; 2],
                grasps: [None; 2],
                finger_give: [[0.0; 5]; 2],
                locomotion: Vec3::ZERO,
                swim: None,
                eye_height: 1.7,
                floor: 0.0,
                saddle: None,
                settings: &settings,
            });
        }
        assert!(body.arm_span() > before + 0.05, "grew from {before:.2} to {:.2}", body.arm_span());
        assert_eq!(body.reached, [true, true], "and now reaches");
    }

    /// A hand in the game's own pose on a gun: the body puts that hand joint exactly on the
    /// game's wrist (whatever the floor), and the avatar gets the pose.
    #[test]
    fn a_held_hand_goes_exactly_to_the_games_wrist() {
        let settings = Settings::default();
        let mut f = HmdFrame { dt: 1.0 / 90.0, ..Default::default() };
        for floor in [0.0, -0.3] {
            let mut body = PlayerBody::default();
            f.hmd = Pose { pos: [0.0, 1.7 + floor, 0.0], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
            for (i, x) in [(0, -0.12), (1, 0.12)] {
                f.hands[i].connected = 1;
                f.hands[i].grip = Pose { pos: [x, 1.3 + floor, 0.4], valid: 1, rot: [0.0, 0.0, 0.0, 1.0] };
            }
            let wrist = Pose { pos: [0.1, 1.33 + floor, 0.36], valid: 1, rot: Quat::from_euler(glam::EulerRot::YXZ, 0.3, -1.1, 1.4).to_array() };
            let held = HeldHand { wrist, fingers: [Quat::from_rotation_z(-0.8); 15], index_pulled: None, trigger: 0.3, size: 0.1, reach: [None; 5], index_reach_pulled: None, weight: 1.0 };
            for _ in 0..30 {
                body.update(&BodyInput {
                    frame: &f,
                    hands: None,
                    hand_states: None,
                    ghosts: [None; 2],
                    holds: [HoldShape::None, HoldShape::PistolGrip],
                    held: [None, Some(held)],
                    grasps: [None; 2],
                    finger_give: [[0.0; 5]; 2],
                    locomotion: Vec3::ZERO,
                    swim: None,
                    eye_height: 1.7 + floor,
                    floor,
                    saddle: None,
                    settings: &settings,
                });
            }
            let hand = body.body.hands[1].pose;
            let joint = body.body.joints[Joint::RightHand as usize];
            for p in [hand, joint] {
                assert!(Vec3::from_array(p.pos).distance(Vec3::from_array(wrist.pos)) < 1e-4, "floor {floor}: {:?}", p.pos);
                assert!(Quat::from_array(p.rot).angle_between(Quat::from_array(wrist.rot)) < 1e-3);
            }
            assert_eq!(body.held, [None, Some(held)], "drawn in the game's pose");
        }
    }

    #[test]
    fn t_pose_measurement() {
        let m = measure_tpose(Vec3::new(0.0, 1.65, 0.0), Vec3::new(-0.8, 1.42, 0.0), Vec3::new(0.8, 1.42, 0.0)).unwrap();
        assert!((m.0 - 1.76).abs() < 1e-4 && (m.1 - 1.6).abs() < 1e-4);
        assert!(measure_tpose(Vec3::new(0.0, 1.65, 0.0), Vec3::new(-0.2, 1.0, 0.3), Vec3::new(0.2, 1.0, 0.3)).is_none(), "arms down isn't a T-pose");
        assert!((grip_span(1.70) - 1.66).abs() < 1e-4);
    }

    /// The IK and the avatar must agree on the joint convention: absolute canonical frames, so a
    /// player standing in the reference T-pose gets exactly the avatar's reference frames.
    #[test]
    fn a_t_pose_solves_to_the_reference_frames() {
        let mut solver = BodySolver::new(BodyParams::default(), Calibration { eye_height: 1.62, arm_span: 1.72 });
        let offsets = BodyParams::default().hand_offsets;
        let head = IkPose { pos: Vec3::new(0.0, 1.62, 0.0), rot: Quat::IDENTITY };
        // Grips whose hand frames are the T-pose hands, at arm's length.
        let grip_for = |side: Side, x: f32| {
            let want = rustvr_avatar::pose::reference_frame(if side == Side::Left { Joint::LeftHand } else { Joint::RightHand });
            let o = offsets.hand_frame(side, IkPose::IDENTITY);
            let rot = (want * o.rot.inverse()).normalize();
            IkPose { pos: Vec3::new(x, 1.42, 0.0) - rot * o.pos, rot }
        };
        let grips = [Some(grip_for(Side::Left, -0.78)), Some(grip_for(Side::Right, 0.78))];
        let mut out = None;
        for _ in 0..120 {
            out = Some(solver.solve(&TrackedInput { dt: 1.0 / 90.0, head, grips, locomotion: Vec3::ZERO, seated: false, elbows: [None; 2], shoulders: [None; 2], swim: None }));
        }
        let out = out.unwrap();
        for j in [Joint::LeftUpperArm, Joint::LeftLowerArm, Joint::RightUpperArm, Joint::RightLowerArm, Joint::LeftHand, Joint::RightHand, Joint::Hips, Joint::Chest] {
            let got = out.joints[j as usize].rot;
            let want = rustvr_avatar::pose::reference_frame(j);
            // A straight arm's roll is the IK's elbow choice; the bone direction is the convention.
            let axes: &[Vec3] = if matches!(j, Joint::LeftHand | Joint::RightHand | Joint::Hips | Joint::Chest) { &[Vec3::Y, Vec3::Z] } else { &[Vec3::Y] };
            for &axis in axes {
                let err = (got * axis).angle_between(want * axis).to_degrees();
                assert!(err < 25.0, "{j:?}: {axis:?} goes to {:?}, should be {:?} ({err:.0} degrees off)", got * axis, want * axis);
            }
        }
    }
}


