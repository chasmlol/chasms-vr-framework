//! The game's menus held in the hands (`HudLayout::held`, [`ANCHOR_HELD`]): a book's pages lying on
//! the book a hand holds, turned by the other hand; a map held like paper, spread between the hands
//! and dragged by one. See `docs/research/books.md` in the Oblivion adapter for what players asked
//! for, and the protocol's `hud.rs` for what a game says.
//!
//! - **On a thing** (`HELD_FACE`): the group lies on the thing's broad face that was turned to the
//!   eyes when it appeared (a book's cover), its middle on the face's middle, upright along the face
//!   side nearest the eyes' up then, a few millimetres out. It follows the thing exactly, as the rig
//!   has it, every frame: the pages never swim on the book.
//! - **A sheet** (`HELD_SHEET`): held by the middle of its outer side (the left hand its left side),
//!   facing the eyes as it's taken, then turning with the hand (a little smoothed: a sheet amplifies
//!   a hand's tremor, the shake Riven VR's players asked to be rid of). The other hand's grip near its
//!   far side spreads it between the two: as wide as the hands are apart (a sheet pulled bigger
//!   stays that big), facing the eyes about the line between them.
//! - **Its size**: a line of the game's text is `menu_text_deg` at `held_distance` (0.45 m), kept
//!   as it moves: nearer is bigger, as with a real book.
//! - **Pages** (`HELD_PAGES`): the free hand, near the pages, sweeping across the middle from well
//!   out on one page to the other (or pinching the trigger at a page's edge and pulling it across)
//!   turns a page: from the right page leftwards onwards, back the other way. The way back is free a
//!   moment after a turn (an overshoot is undone at once), but the hand's own return after a turn
//!   doesn't count against it until then.
//! - **Dragging** (`HELD_DRAG`): the free hand's grip on the sheet's face holds the game's cursor
//!   there, its button down, and moves it with the hand: the game drags what's on it (its map).
//!
//! The pieces are drawn with the body, in its depth (`rustvr_avatar::Sheet`), and the laser points
//! at them from the free hand.

use glam::{Quat, Vec2, Vec3};
use rustvr_protocol::*;

use crate::hud::HudPiece;
use crate::panels::Panel;
use crate::settings::Settings;

/// A thing a hand holds this frame, as the rig has it (tracking space).
#[derive(Clone, Copy, Debug)]
pub struct Thing {
    pub id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    /// Its box in its own space: centre and half extents (metres).
    pub centre: Vec3,
    pub half: Vec3,
    pub hand: usize,
}

/// A hand this frame: its controller's grip pose, its grip and its trigger.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hand {
    pub grip: Option<(Vec3, Quat)>,
    pub squeeze: f32,
    pub trigger: f32,
}

/// What the held menus did this frame.
#[derive(Default)]
pub struct HeldOut {
    /// Their pieces, where they lie now.
    pub pieces: Vec<HudPiece>,
    /// The hands holding one (the laser comes from the other).
    pub holding: [bool; 2],
    /// Pages the free hand turned: the actions to press.
    pub presses: Vec<Action>,
    /// A hand dragging what's on a sheet: the game's cursor there (the HUD texture's part), its
    /// button down; and the frame the drag ends, up.
    pub cursor: Option<(Vec2, bool)>,
    /// Ticks in the hands: which, seconds, frequency, amplitude.
    pub haptics: Vec<(usize, f32, f32, f32)>,
}

/// How far out from a thing's face its pages lie (metres): clear of the model, close enough that
/// they look printed on it.
const FACE_LIFT: f32 = 0.004;
/// The free hand is on the pages within this far in front of them (and a little behind).
const NEAR_PAGE: f32 = 0.12;
/// A sweep starts this far out on a page (a share of the half width) and ends this far across.
const SWEEP_FROM: f32 = 0.35;
const SWEEP_TO: f32 = 0.08;
/// A sweep takes at most this long (seconds); the way back is shut this long after a turn, the same
/// way this long.
const SWEEP_TIME: f64 = 1.2;
const BACK_AFTER: f64 = 0.7;
const AGAIN_AFTER: f64 = 0.2;
/// A grip within this share of the half width of a sheet's far side spreads it between the hands.
const FAR_SIDE: f32 = 0.3;
/// A sheet spread between the hands is this many times its natural size at least and at most.
const SPREAD: (f32, f32) = (0.6, 2.5);
/// How quickly a sheet held in one hand turns after it (seconds).
const SHEET_EASE: f32 = 0.06;

/// A sheet's hold.
#[derive(Clone, Copy, Debug)]
enum Hold {
    /// In one hand: its right and up in the hand's space.
    One { right: Vec3, up: Vec3 },
    /// Spread between both hands.
    Both,
}

/// How a held group lies on what holds it, kept while it's there.
struct Lie {
    id: u32,
    /// (`HELD_FACE`) the face's middle, right and up in the thing's space.
    face: Option<(Vec3, Vec3, Vec3)>,
    /// (`HELD_SHEET`) how it's held.
    hold: Option<Hold>,
    /// Metres per group unit (the HUD texture's height).
    unit: f32,
    /// The frame it's at now: its origin (the group's (0, 0)), right and up.
    frame: Option<(Vec3, Vec3, Vec3)>,
    /// A sweep under way: where on the pages it started (group units across) and when.
    sweep: Option<(f32, f64)>,
    /// The last turn: which way and when.
    turned: Option<(Action, f64)>,
    /// The free hand's grip and trigger last frame.
    gripped: bool,
    pinched: bool,
    /// Dragging: where on the HUD texture the cursor is.
    drag: Option<Vec2>,
}

/// The held menus' places, kept between frames.
#[derive(Default)]
pub struct HeldMenus {
    lies: Vec<Lie>,
}

/// The extent of group `group`'s pieces (x0, y0, x1, y1 in its units, y up).
fn footprint(layout: &HudLayout, group: u32, aspect: f32) -> Option<[f32; 4]> {
    let mut b: Option<[f32; 4]> = None;
    for e in layout.entries().iter().filter(|e| e.group == group) {
        let (w, h) = piece_size(e, aspect);
        let r = [e.offset[0] - w * 0.5, e.offset[1] - h * 0.5, e.offset[0] + w * 0.5, e.offset[1] + h * 0.5];
        b = Some(match b {
            None => r,
            Some(b) => [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])],
        });
    }
    b
}

fn piece_size(e: &HudElement, aspect: f32) -> (f32, f32) {
    let scale = if e.scale > 0.0 { e.scale } else { 1.0 };
    ((e.uv[2] - e.uv[0]).max(1e-4) * aspect * scale, (e.uv[3] - e.uv[1]).max(1e-4) * scale)
}

/// The face of a box (centre `c`, half extents `h`, in its own space) turned to `eye` (in the same
/// space), upright as `up` is: its middle, right and up.
fn face_toward(c: Vec3, h: Vec3, eye: Vec3, up: Vec3) -> (Vec3, Vec3, Vec3) {
    let axes = [Vec3::X, Vec3::Y, Vec3::Z];
    let thin = (0..3).min_by(|&a, &b| h[a].total_cmp(&h[b])).unwrap();
    let out = axes[thin] * if eye[thin] >= c[thin] { 1.0 } else { -1.0 };
    // The face's up: whichever of its two sides points most the way the eyes' up does.
    let side = (0..3).filter(|&k| k != thin).map(|k| axes[k]).flat_map(|a| [a, -a]).max_by(|a, b| a.dot(up).total_cmp(&b.dot(up))).unwrap();
    // (Right so the face's normal, up x right, is out of it.)
    let right = out.cross(side);
    (c + out * (h[thin] + FACE_LIFT), right, side)
}

/// Right and up for a sheet at `at` turned to the eye at `eye`, upright as `up` is.
fn facing(at: Vec3, eye: Vec3, up: Vec3) -> (Vec3, Vec3) {
    let n = (eye - at).normalize_or(Vec3::NEG_Z);
    let u = (up - n * up.dot(n)).normalize_or(Vec3::Y);
    (n.cross(u), u)
}

/// `v` eased towards `to` (unit vectors) by `k`.
fn ease(v: Vec3, to: Vec3, k: f32) -> Vec3 {
    v.lerp(to, k).normalize_or(to)
}

impl HeldMenus {
    /// This frame's held menus. `things`: what the hands hold; `hands`: both hands; `aspect`: the
    /// HUD texture's width / height.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, s: &Settings, head_pos: Vec3, head_rot: Quat, dt: f32, now: f64, aspect: f32, layout: &HudLayout, things: &[Thing], hands: &[Hand; 2]) -> HeldOut {
        let mut out = HeldOut::default();
        let groups = layout.group_entries();
        self.lies.retain(|l| groups.iter().any(|g| g.anchor == ANCHOR_HELD && g.id == l.id));
        let text = if layout.text_height > 0.0 { layout.text_height } else { 0.025 };
        let natural = s.ui.held_distance * s.ui.menu_text_deg.to_radians().tan() / text;
        let head_up = head_rot * Vec3::Y;
        for (i, g) in groups.iter().enumerate() {
            let n = i as u32 + 1;
            let Some(held) = layout.held_of(n).copied() else { continue };
            let Some(fp) = footprint(layout, n, aspect) else { continue };
            let thing = (held.object != 0).then(|| things.iter().find(|t| t.id == held.object)).flatten();
            if held.object != 0 && thing.is_none() {
                // (Not in a hand this frame: nowhere to put it.)
                continue;
            }
            // The hand that holds it, and the other.
            let holder = thing.map_or(held.hand as usize & 1, |t| t.hand);
            let free = holder ^ 1;
            let k = match self.lies.iter().position(|l| l.id == g.id) {
                Some(k) => k,
                None => {
                    let face = (held.flags & HELD_FACE != 0).then(|| thing.map(|t| {
                        let inv = t.rot.inverse();
                        face_toward(t.centre, t.half, inv * (head_pos - t.pos), inv * head_up)
                    })).flatten();
                    self.lies.push(Lie { id: g.id, face, hold: None, unit: natural, frame: None, sweep: None, turned: None, gripped: false, pinched: false, drag: None });
                    self.lies.len() - 1
                }
            };
            let lie = &mut self.lies[k];
            // Where it is now: its origin, right and up.
            let frame = match (lie.face, thing) {
                (Some((c, r, u)), Some(t)) => Some((t.pos + t.rot * c, t.rot * r, t.rot * u)),
                _ => sheet_frame(lie, &held, fp, holder, thing, hands, head_pos, head_up, natural, dt),
            };
            let Some((origin, right, up)) = frame else { continue };
            lie.frame = Some((origin, right, up));
            out.holding[holder] = true;
            if matches!(lie.hold, Some(Hold::Both)) {
                out.holding[free] = true;
            }
            let normal = up.cross(right);
            let unit = lie.unit;
            let half_w = (fp[2] - fp[0]) * 0.5;
            let mid_x = (fp[0] + fp[2]) * 0.5;
            let mut pieces = Vec::new();
            for e in layout.entries().iter().filter(|e| e.group == n) {
                let (w, h) = piece_size(e, aspect);
                // (An open book's two pages tip towards the eye about its middle.)
                let side = if e.offset[0] + w * 0.5 <= 1e-4 { -1.0 } else if e.offset[0] - w * 0.5 >= -1e-4 { 1.0 } else { 0.0 };
                // (The left page's outer edge comes towards the eye, and the right page's.)
                let a = held.fold * side;
                let r = (right * a.cos() + normal * a.sin()).normalize_or(right);
                let c = origin + r * e.offset[0] * unit + up * e.offset[1] * unit + normal * e.offset[2] * unit;
                let panel = Panel { center: c, right: r, up, width: w * unit, height: h * unit };
                let opacity = if e.opacity > 0.0 { e.opacity } else { 1.0 };
                pieces.push(HudPiece { panel, uv: e.uv, opacity, role: e.role, flags: e.flags });
            }
            // The free hand on it: where on it (group units) and how far in front.
            let on = hands[free].grip.map(|(p, _)| {
                let d = p - origin;
                (Vec2::new(d.dot(right), d.dot(up)) / unit, d.dot(normal))
            });
            let near = on.filter(|&(xy, z)| {
                z > -0.03 && z < NEAR_PAGE && xy.x > fp[0] - half_w * 0.15 && xy.x < fp[2] + half_w * 0.15 && xy.y > fp[1] - (fp[3] - fp[1]) * 0.2 && xy.y < fp[3] + (fp[3] - fp[1]) * 0.2
            });
            let grip = hands[free].squeeze > 0.6;
            let pinch = hands[free].trigger > 0.6;
            // Spreading a sheet: the free hand's grip near its far side.
            if held.flags & HELD_SHEET != 0 && grip && !lie.gripped && lie.drag.is_none() {
                let far = if holder == 0 { fp[2] } else { fp[0] };
                if near.is_some_and(|(xy, _)| (xy.x - far).abs() < half_w * FAR_SIDE) {
                    lie.hold = Some(Hold::Both);
                    out.haptics.push((free, 0.02, 160.0, 0.5));
                }
            }
            if !grip && matches!(lie.hold, Some(Hold::Both)) {
                // Let go with the other hand: held in the first again, as it is now.
                if let Some((_, q)) = thing.map(|t| (t.pos, t.rot)).or(hands[holder].grip) {
                    let inv = q.inverse();
                    lie.hold = Some(Hold::One { right: inv * right, up: inv * up });
                }
            }
            // Dragging what's on it: the free hand's grip on its face.
            if held.flags & HELD_DRAG != 0 && !matches!(lie.hold, Some(Hold::Both)) {
                if grip && !lie.gripped && near.is_some() {
                    lie.drag = Some(Vec2::ZERO);
                }
                if lie.drag.is_some() {
                    let at = hands[free].grip.map(|(p, _)| p).unwrap_or(origin);
                    // (The point of the texture under the hand, on whichever piece it's over.)
                    let uv = pieces.iter().find_map(|pc| pc.hit(at + normal * 0.3, -normal).map(|(_, uv)| uv));
                    if let Some(uv) = uv {
                        lie.drag = Some(uv);
                    }
                    let uv = lie.drag.unwrap();
                    if grip {
                        if uv != Vec2::ZERO {
                            out.cursor = Some((uv, true));
                        }
                    } else {
                        out.cursor = (uv != Vec2::ZERO).then_some((uv, false));
                        lie.drag = None;
                    }
                }
            }
            // Turning pages: a sweep across the middle, or a pinch at an edge pulled across.
            if held.flags & HELD_PAGES != 0 && lie.drag.is_none() {
                match near {
                    Some((xy, _)) => {
                        let x = (xy.x - mid_x) / half_w.max(1e-4);
                        let starts = lie.sweep.is_none() || (pinch && !lie.pinched);
                        if starts {
                            lie.sweep = (x.abs() > SWEEP_FROM).then_some((x, now));
                        }
                        if let Some((x0, t0)) = lie.sweep {
                            let way = if x0 > 0.0 && x < -SWEEP_TO { Some(Action::PageNext) } else if x0 < 0.0 && x > SWEEP_TO { Some(Action::PagePrev) } else { None };
                            if let Some(a) = way.filter(|_| now - t0 < SWEEP_TIME) {
                                let shut = lie.turned.is_some_and(|(b, t)| now - t < if a == b { AGAIN_AFTER } else { BACK_AFTER });
                                if !shut {
                                    out.presses.push(a);
                                    out.haptics.push((free, 0.02, 120.0, 0.45));
                                    lie.turned = Some((a, now));
                                }
                                // (The next sweep starts from here: the hand is out on the other page.)
                                lie.sweep = Some((x, now));
                            } else if now - t0 >= SWEEP_TIME {
                                lie.sweep = (x.abs() > SWEEP_FROM).then_some((x, now));
                            }
                        }
                    }
                    None => lie.sweep = None,
                }
            }
            lie.gripped = grip;
            lie.pinched = pinch;
            out.pieces.extend(pieces);
        }
        out
    }
}

/// Where a sheet is now (`HELD_SHEET`, or a group with nothing to lie on): its origin, right and
/// up, from the hands; its size kept in `lie.unit`.
#[allow(clippy::too_many_arguments)]
fn sheet_frame(lie: &mut Lie, held: &HudHeld, fp: [f32; 4], holder: usize, thing: Option<&Thing>, hands: &[Hand; 2], eye: Vec3, head_up: Vec3, natural: f32, dt: f32) -> Option<(Vec3, Vec3, Vec3)> {
    // The holding hand: where the thing is in it (it's where the hand has it), or the hand.
    let at = thing.map(|t| (t.pos, t.rot)).or(hands[holder].grip)?;
    let mid_y = (fp[1] + fp[3]) * 0.5;
    if let (Some(Hold::Both), Some((other, _))) = (lie.hold, hands[holder ^ 1].grip) {
        // Spread between the hands: its sides' middles in them, facing the eyes about the line
        // between them, as wide as they're apart.
        let (a, b) = (at.0, other);
        let eye_right = (b - a).dot(eye_right_of(eye, (a + b) * 0.5, head_up));
        let (l, r) = if eye_right >= 0.0 { (a, b) } else { (b, a) };
        let mid = (l + r) * 0.5;
        let right = (r - l).normalize_or(Vec3::X);
        let n = {
            let to_eye = eye - mid;
            (to_eye - right * to_eye.dot(right)).normalize_or(Vec3::NEG_Z)
        };
        let up = right.cross(n);
        let width = (fp[2] - fp[0]).max(1e-4);
        lie.unit = (l.distance(r) / width).clamp(natural * SPREAD.0, natural * SPREAD.1);
        let origin = mid - right * (fp[0] + fp[2]) * 0.5 * lie.unit - up * mid_y * lie.unit;
        return Some((origin, right, up));
    }
    if lie.hold.is_none() || matches!(lie.hold, Some(Hold::Both)) {
        // Taken just now (or the other hand gone): facing the eyes from where it's held.
        let (r, u) = facing(at.0, eye, head_up);
        let inv = at.1.inverse();
        lie.hold = Some(Hold::One { right: inv * r, up: inv * u });
        if held.flags & HELD_SHEET == 0 {
            lie.unit = natural;
        }
    }
    let Some(Hold::One { right, up, .. }) = lie.hold else { return None };
    let (mut r, mut u) = (at.1 * right, at.1 * up);
    if let Some((_, pr, pu)) = lie.frame {
        let k = 1.0 - (-dt / SHEET_EASE).exp();
        r = ease(pr, r, k);
        u = ease(pu, u, k);
        u = (u - r * u.dot(r)).normalize_or(u);
    }
    // Held by the middle of its outer side: the left hand its left side.
    let side_x = if holder == 0 { fp[0] } else { fp[2] };
    let origin = at.0 - r * side_x * lie.unit - u * mid_y * lie.unit;
    Some((origin, r, u))
}

/// The eyes' right at a point (across the line from the eye to it).
fn eye_right_of(eye: Vec3, at: Vec3, up: Vec3) -> Vec3 {
    let f = (at - eye).normalize_or(Vec3::Z);
    up.cross(f).normalize_or(Vec3::X)
}
