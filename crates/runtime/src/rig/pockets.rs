//! The body's slots that hand out the player's things: the game's inventory ([`Inventory`]) read
//! as its categories, each slot a place on the body drawing from some of them in an order. A hand
//! gripping in a slot takes the slot's first thing as the runtime's copy of it (the game's own
//! model, in the fist); held at the mouth it's eaten or drunk, held up to the eyes it's read,
//! touched to the weapon in the other hand it's put on it, each the game's own use of it; let go in
//! any slot it goes back; let go anywhere else it falls, for the game to drop where it comes to
//! rest (see `rustvr_protocol::inventory`).
//!
//! What the research behind it found players want (`docs/design/holsters.md` in the Oblivion
//! adapter's repository): slots placed from the head, not a guessed body (they hang off the same
//! body anchor as the holsters); a tick on finding one and a different one when it's empty; a
//! generous mouth (a small one is much complained of) with a moment's
//! hold and bites in the hand on the way, so nothing is eaten by accident; what's put back always
//! taken back; slots moved by hand, on the body, rather than with sliders.

use std::cmp::Ordering;
use std::collections::HashMap;

use glam::{Mat3, Quat, Vec3};
use rustvr_protocol::*;

use super::zones::Zone;
use crate::settings::{BodySlot, SlotEmpty, SlotOrder};

/// The mouth, from the eyes (head space: x right, y up, z forward): just under the nose, a little
/// in front of the face.
pub const MOUTH: Vec3 = Vec3::new(0.0, -0.075, 0.05);
/// How near the mouth a thing's surface comes for it to be at the mouth (metres): generous, as a
/// thing held to the face is (a small target has players pouring potions on their necks).
pub const MOUTH_REACH: f32 = 0.09;
/// Reading: the thing's middle this near and far in front of the eyes, and within this of where
/// they look.
pub const READ_NEAR: f32 = 0.12;
pub const READ_FAR: f32 = 0.55;
pub const READ_ANGLE: f32 = 0.4;
/// Putting it on a weapon: its surface this near what the other hand holds.
pub const COAT_REACH: f32 = 0.03;
/// A hand moving faster than this isn't holding a thing to its mouth or eyes, it's passing.
pub const USE_SPEED: f32 = 1.2;
/// A grip held in a slot opens its editor only while the hand stays within this of where it closed.
pub const EDIT_STILL: f32 = 0.08;
/// A use the game hasn't taken in this long was refused (the potion limit, a quest item).
pub const USE_WAIT: f64 = 1.5;
/// The bites (or gulps) felt on the way to a use, this far apart.
pub const BITE_EVERY: f32 = 0.16;

/// A thing drawn from a slot, in a hand or let go: which of the inventory's stacks it is, and how
/// its use is going.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drawn {
    /// The stack ([`InvItem::id`]) and what it is ([`InvItem::flags`], [`InvItem::key`]).
    pub item: u32,
    pub flags: u32,
    pub key: u32,
    /// The slot it came from.
    pub slot: usize,
    /// Times used, for the game ([`ObjectState::shots`]).
    pub uses: u32,
    /// How long it has been held where it's used, and when the next bite is felt.
    pub held: f32,
    pub next_bite: f32,
    /// Used, and waiting for the game to take it: since when.
    pub used_at: Option<f64>,
    /// After a refused use, it must leave the mouth (or the eyes, the weapon) before it can be used
    /// again.
    pub armed: bool,
}

impl Drawn {
    pub fn new(item: &InvItem, slot: usize) -> Drawn {
        Drawn { item: item.id, flags: item.flags, key: item.key, slot, uses: 0, held: 0.0, next_bite: 0.0, used_at: None, armed: true }
    }
}

/// A slot's zones: where a hand takes from it, and where a thing let go goes back in.
#[derive(Clone, Copy, Debug)]
pub struct SlotZones {
    pub take: Zone,
    pub put: Zone,
}

/// A slot's zones for a player whose eyes are `eye_height` up.
pub fn zones(slot: &BodySlot, eye_height: f32) -> SlotZones {
    let k = (eye_height / 1.70).clamp(0.6, 1.3);
    let offset = Vec3::from_array(slot.at) * k;
    let behind = slot.behind.then_some(super::zones::BEHIND);
    SlotZones { take: Zone { offset, radius: slot.radius + slot.reach, behind }, put: Zone { offset, radius: slot.radius, behind } }
}

pub fn rule_of(o: SlotOrder) -> u32 {
    match o {
        SlotOrder::Healing => RULE_HEALING,
        SlotOrder::Magicka => RULE_MAGICKA,
        SlotOrder::Fatigue => RULE_FATIGUE,
        SlotOrder::Value => RULE_VALUE,
        SlotOrder::Heaviest => RULE_HEAVIEST,
        SlotOrder::Lightest => RULE_LIGHTEST,
        SlotOrder::Newest => RULE_NEWEST,
        SlotOrder::Name => RULE_NAME,
        SlotOrder::Most => RULE_MOST,
        SlotOrder::Strongest => RULE_STRONGEST,
        SlotOrder::Own => RULE_OWN,
    }
}

pub fn order_of(rule: u32) -> SlotOrder {
    match rule {
        RULE_HEALING => SlotOrder::Healing,
        RULE_MAGICKA => SlotOrder::Magicka,
        RULE_FATIGUE => SlotOrder::Fatigue,
        RULE_HEAVIEST => SlotOrder::Heaviest,
        RULE_LIGHTEST => SlotOrder::Lightest,
        RULE_NEWEST => SlotOrder::Newest,
        RULE_NAME => SlotOrder::Name,
        RULE_MOST => SlotOrder::Most,
        RULE_STRONGEST => SlotOrder::Strongest,
        RULE_OWN => SlotOrder::Own,
        _ => SlotOrder::Value,
    }
}

fn empty_of(e: u32) -> SlotEmpty {
    match e {
        EMPTY_SILENT => SlotEmpty::Silent,
        EMPTY_ANY => SlotEmpty::Any,
        _ => SlotEmpty::Tick,
    }
}

/// A slot as the protocol has it, its categories numbered as `inv` has them (one the game hasn't
/// got is left out).
pub fn to_desc(slot: &BodySlot, inv: &Inventory) -> SlotDesc {
    let mut d: SlotDesc = bytemuck::Zeroable::zeroed();
    set_name(&mut d.name, &slot.name);
    d.offset = slot.at;
    d.radius = slot.radius;
    d.reach = slot.reach;
    d.rule = rule_of(slot.order);
    d.empty = match slot.empty {
        SlotEmpty::Tick => EMPTY_TICK,
        SlotEmpty::Silent => EMPTY_SILENT,
        SlotEmpty::Any => EMPTY_ANY,
    };
    d.flags = if slot.equipped_first { SLOTF_EQUIPPED_FIRST } else { 0 } | if slot.behind { SLOTF_BEHIND } else { 0 };
    d.categories = [SLOT_NO_CATEGORY; MAX_SLOT_CATEGORIES];
    for (k, c) in slot.categories.iter().filter_map(|c| inv.category(c)).take(MAX_SLOT_CATEGORIES).enumerate() {
        d.categories[k] = c;
    }
    d
}

/// A slot from the protocol's (the game's defaults, the editor's), its categories by their names;
/// the player's own order carried over from `was` (the slot it replaces).
pub fn from_desc(d: &SlotDesc, inv: &Inventory, was: Option<&BodySlot>) -> BodySlot {
    let cats = inv.categories();
    BodySlot {
        name: d.name().to_owned(),
        at: d.offset,
        radius: d.radius.clamp(0.03, 0.3),
        reach: d.reach.clamp(0.0, 0.2),
        behind: d.flags & SLOTF_BEHIND != 0,
        categories: d.categories().filter_map(|c| cats.get(c as usize)).map(|c| name_str(&c.name).to_owned()).collect(),
        order: order_of(d.rule),
        equipped_first: d.flags & SLOTF_EQUIPPED_FIRST != 0,
        empty: empty_of(d.empty),
        own: was.map(|w| w.own.clone()).unwrap_or_default(),
    }
}

/// The game's own slots, for a player of `dominant` hand (made for a right-handed one: mirrored for
/// a left-handed one).
pub fn defaults(inv: &Inventory, dominant: usize) -> Vec<BodySlot> {
    inv.defaults()
        .iter()
        .map(|d| {
            let mut s = from_desc(d, inv, None);
            if dominant == 0 {
                s.at[0] = -s.at[0];
            }
            s
        })
        .collect()
}

/// What a thing restores by a restoring rule: the rule's own (health for healing...), and the
/// other two together.
fn restores(rule: u32, i: &InvItem) -> Option<(f32, f32)> {
    let k = match rule {
        RULE_HEALING => 0,
        RULE_MAGICKA => 1,
        RULE_FATIGUE => 2,
        _ => return None,
    };
    let r = i.restores.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    Some((r[k], r.iter().sum::<f32>() - r[k]))
}

/// Where the player's own order puts a thing (what was put back last is first).
fn own_rank(own: &[u32], key: u32) -> usize {
    own.iter().position(|&k| k == key).unwrap_or(usize::MAX)
}

/// How two things compare by a slot's rule (the first is the one given out first).
fn by_rule(rule: u32, own: &[u32], a: &InvItem, b: &InvItem) -> Ordering {
    let desc = |x: f32, y: f32| y.total_cmp(&x);
    let newest = b.newest.cmp(&a.newest);
    let then = match rule {
        RULE_HEALING | RULE_MAGICKA | RULE_FATIGUE => {
            let (ra, rb) = (restores(rule, a).unwrap(), restores(rule, b).unwrap());
            desc(ra.0, rb.0).then(desc(ra.1, rb.1))
        }
        RULE_VALUE => desc(a.value, b.value),
        RULE_HEAVIEST => desc(a.weight, b.weight),
        RULE_LIGHTEST => a.weight.total_cmp(&b.weight),
        RULE_NEWEST => newest,
        RULE_NAME => a.name().to_lowercase().cmp(&b.name().to_lowercase()),
        RULE_MOST => b.count.cmp(&a.count),
        RULE_STRONGEST => desc(a.power, b.power),
        RULE_OWN => own_rank(own, a.key).cmp(&own_rank(own, b.key)).then(newest),
        _ => Ordering::Equal,
    };
    // (Ties by value, then by name: the same thing always comes out first.)
    then.then(desc(a.value, b.value)).then_with(|| a.name().cmp(b.name())).then(a.id.cmp(&b.id))
}

/// The things a slot has to give, first first: those of its categories that can be drawn (the
/// game has a model for them) and aren't all out of it already (`out`: how many of each stack are
/// out, in hands or let go). By the categories' order, then the slot's rule; the equipped first if
/// it says so. A restoring rule (most healing first) gives only what restores something, and what
/// restores the rule's own before the rest, whatever the category. With nothing in its categories,
/// a slot that gives anything when empty gives the rule's first of everything.
pub fn contents<'a>(slot: &BodySlot, inv: &'a Inventory, out: &HashMap<u32, u32>) -> Vec<&'a InvItem> {
    let rule = rule_of(slot.order);
    let cats: Vec<u8> = slot.categories.iter().filter_map(|c| inv.category(c)).collect();
    let left = |i: &InvItem| i.count > out.get(&i.id).copied().unwrap_or(0);
    let drawable = |i: &&InvItem| i.id != 0 && left(i) && !i.model().is_empty() && restores(rule, i).is_none_or(|(own, rest)| own + rest > 0.0);
    let pick = |any: bool| -> Vec<&'a InvItem> {
        let mut v: Vec<&InvItem> = inv.items().iter().filter(drawable).filter(|i| any || cats.contains(&(i.category as u8))).collect();
        let place = |i: &InvItem| cats.iter().position(|&c| c as u32 == i.category).unwrap_or(cats.len());
        v.sort_by(|a, b| {
            let equipped = |i: &InvItem| !(slot.equipped_first && i.flags & INV_EQUIPPED != 0);
            let tier = |i: &InvItem| restores(rule, i).is_some_and(|(own, _)| own <= 0.0);
            equipped(a).cmp(&equipped(b)).then(tier(a).cmp(&tier(b))).then(place(a).cmp(&place(b))).then_with(|| by_rule(rule, &slot.own, a, b))
        });
        v
    };
    let v = pick(false);
    if v.is_empty() && slot.empty == SlotEmpty::Any { pick(true) } else { v }
}

/// The object a thing drawn from a slot is: one of the runtime's own props, drawn with the thing's
/// model, its box its collision.
pub fn object_desc(item: &InvItem, id: u32) -> ObjectDesc {
    let mut d: ObjectDesc = bytemuck::Zeroable::zeroed();
    d.id = id;
    d.kind = OBJ_PROP;
    d.flags = OBJF_GRABBABLE | OBJF_PULLABLE | OBJF_RUNTIME_DRAWS;
    d.shape = SHAPE_BOX;
    d.center = item.center;
    d.half_extents = Vec3::from_array(item.half_extents).max(Vec3::splat(0.01)).to_array();
    d.mass = if item.mass.is_finite() { item.mass.clamp(0.02, 20.0) } else { 0.3 };
    set_name(&mut d.name, item.model());
    d
}

/// The fist's hole: from the controller's grip (the axis a fist closes round) to the palm. A thing
/// thicker than it lies against the palm, its own axis out from the grip by the difference.
const FIST: f32 = 0.02;
/// How much of a handle the fingers close round, either way of the hand's middle (half a hand's
/// width).
const HAND_HALF: f32 = 0.045;
/// How far in from a flat thing's edge the fist's axis runs.
const EDGE: f32 = 0.025;
/// Where a bottle is held, up from its base: round its body, below the neck.
const BOTTLE_AT: f32 = 0.35;
/// A thing this many times longer than it's wide is held along its length; one this many times
/// thinner than it's wide, by an edge.
const LONG: f32 = 1.8;
const FLAT: f32 = 0.45;
/// A handle's thickness the fingers close round, at least and at most (metres, its radius).
const RADIUS_MIN: f32 = 0.008;
const RADIUS_MAX: f32 = 0.06;

/// How the hand holds a thing drawn from a slot, made to be held that way as a weapon is by its
/// hilt: the controller's grip pose on it for the right hand (model space; +Z along the fist from
/// the little finger to the thumb, the palm on its +X side), and the part of it the fingers close
/// round, its axis end to end and its thickness (`ObjectDesc::grip_line`, `grip_radius`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold {
    pub grip: Vec3,
    pub rot: Quat,
    pub line: (Vec3, Vec3),
    pub radius: f32,
}

/// How a thing of model `points` (its vertices, model space: +Y up as it stands in the world) is
/// held, by its kind (`flags`, `INV_*`) and its shape:
/// - a bottle (a drink, a poison): upright round its body, a third of the way up, its mouth past the
///   thumb;
/// - a long thing (an arrow, a leek, a rolled scroll, a loaf): along the fist, by its narrowest
///   place near its middle, the palm on its flat side;
/// - a flat thing (a book, a plate, a cheese wheel): by an edge, along its long side, the rest of it
///   out past the fingers;
/// - anything else (an apple, a soul gem): in the fist, upright as it stands.
///
/// A thing thicker than a fist's hole lies against the palm, the fingers round it as far as they go.
pub fn hold(points: &[Vec3], flags: u32) -> Option<Hold> {
    let (lo, hi) = points.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(*p), h.max(*p)));
    if points.is_empty() || !(hi - lo).is_finite() {
        return None;
    }
    let size = (hi - lo).max(Vec3::splat(1e-3));
    let mut axes = [Vec3::X, Vec3::Y, Vec3::Z];
    axes.sort_by(|a, b| size.dot(*b).total_cmp(&size.dot(*a)));
    let [long, mid, short] = axes;
    let ext = |a: Vec3| size.dot(a);
    // (The palm on the thinner of the two sides across the fist: on a flask's face, a knife's.)
    let thinner = |z: Vec3| axes.into_iter().filter(|a| *a != z).min_by(|a, b| ext(*a).total_cmp(&ext(*b))).unwrap();
    if flags & (INV_DRINK | INV_COAT) != 0 {
        return Some(round_axis(points, Vec3::Y, thinner(Vec3::Y), lo.y + BOTTLE_AT * size.y));
    }
    if ext(long) > LONG * ext(mid) {
        // Its narrowest place within a sixth of its length of its middle.
        let (a, b) = (lo.dot(long), hi.dot(long));
        let middle = (a + b) * 0.5;
        let t = (0..=8)
            .map(|k| a + (b - a) * (1.0 / 3.0 + k as f32 / 24.0))
            .filter_map(|t| Some((t, section(points, long, t)?.1)))
            .min_by(|x, y| x.1.total_cmp(&y.1).then((x.0 - middle).abs().total_cmp(&(y.0 - middle).abs())))
            .map_or(middle, |(t, _)| t);
        return Some(round_axis(points, long, short, t));
    }
    if ext(short) < FLAT * ext(mid) {
        // The fist along its long side, its axis a little in from the edge, the palm on the edge.
        let at = hi.dot(mid) - EDGE.min(ext(mid) * 0.5);
        let strip: Vec<Vec3> = points.iter().copied().filter(|p| (p.dot(mid) - at).abs() < EDGE).collect();
        let (s_lo, s_hi) = strip.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p.dot(short)), h.max(p.dot(short))));
        let (s_mid, radius) = if s_lo <= s_hi { ((s_lo + s_hi) * 0.5, (s_hi - s_lo) * 0.5) } else { (((lo + hi) * 0.5).dot(short), ext(short) * 0.5) };
        let centre = long * ((lo + hi) * 0.5).dot(long) + mid * at + short * s_mid;
        return Some(handle(centre, mid, long, radius, (lo.dot(long), hi.dot(long))));
    }
    Some(round_axis(points, Vec3::Y, thinner(Vec3::Y), ((lo + hi) * 0.5).y))
}

/// Held round its section across `axis` at `t` along it (the fist along `axis`, the palm on its
/// `palm` side).
fn round_axis(points: &[Vec3], axis: Vec3, palm: Vec3, t: f32) -> Hold {
    let (lo, hi) = points.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p.dot(axis)), h.max(p.dot(axis))));
    let (centre, radius) = section(points, axis, t).unwrap_or_else(|| {
        let c = points.iter().fold(Vec3::ZERO, |a, p| a + *p) / points.len().max(1) as f32;
        (c - axis * c.dot(axis) + axis * t, 0.02)
    });
    handle(centre, palm, axis, radius, (lo, hi))
}

/// A hold round a handle through `centre` along `axis` (the thing's extent along it `span`), the
/// palm on its `palm` side.
fn handle(centre: Vec3, palm: Vec3, axis: Vec3, radius: f32, span: (f32, f32)) -> Hold {
    let radius = radius.clamp(RADIUS_MIN, RADIUS_MAX);
    let rot = Quat::from_mat3(&Mat3::from_cols(palm, axis.cross(palm), axis)).normalize();
    let t = centre.dot(axis);
    let (a, b) = ((span.0 - t).max(-HAND_HALF).min(0.0), (span.1 - t).min(HAND_HALF).max(0.0));
    Hold { grip: centre + palm * (radius - FIST).max(0.0), rot, line: (centre + axis * a, centre + axis * b), radius }
}

/// The section of `points` across model axis `axis` at `t` along it: its middle (on the plane)
/// and how far it reaches out from there (its larger half-width), from the points near the plane
/// (as near as there are enough of them: a round thing's vertices are in rings apart).
fn section(points: &[Vec3], axis: Vec3, t: f32) -> Option<(Vec3, f32)> {
    let (lo, hi) = points.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p.dot(axis)), h.max(p.dot(axis))));
    let mut slab = ((hi - lo) * 0.06).max(0.005);
    let near = loop {
        let near: Vec<Vec3> = points.iter().copied().filter(|p| (p.dot(axis) - t).abs() <= slab).collect();
        if near.len() >= 8 || slab > hi - lo {
            break near;
        }
        slab *= 1.5;
    };
    if near.len() < 3 {
        return None;
    }
    let mut centre = axis * t;
    let mut radius: f32 = 0.0;
    for a in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().filter(|a| *a != axis) {
        let (l, h) = near.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p.dot(a)), h.max(p.dot(a))));
        centre += a * (l + h) * 0.5;
        radius = radius.max((h - l) * 0.5);
    }
    Some((centre, radius))
}

/// A thing drawn from a slot rigged with its hold: the hand goes on it there, and the fingers close
/// round its handle.
pub fn rigged(mut d: ObjectDesc, h: &Hold) -> ObjectDesc {
    d.grip = h.grip.to_array();
    d.grip_rot = h.rot.to_array();
    d.grip_line = [h.line.0.to_array(), h.line.1.to_array()];
    d.grip_radius = h.radius;
    d
}

/// How a thing drawn from a slot sits in the fist (its rotation in the controller's grip space,
/// whose +Z runs along the fist from the little finger to the thumb): a long thing (an arrow, a
/// scroll) along the fist as a handle is; anything else upright, as it stands in the world (its
/// model's up, +Y, along the fist: a bottle held as it's drunk from).
pub fn hold_rot(item: &InvItem) -> Quat {
    use std::f32::consts::FRAC_PI_2;
    let h = Vec3::from_array(item.half_extents).abs();
    let long = |a: f32, b: f32, c: f32| a > 2.0 * b && a > 2.0 * c;
    if long(h.x, h.y, h.z) {
        Quat::from_rotation_y(-FRAC_PI_2)
    } else if long(h.z, h.x, h.y) {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_x(FRAC_PI_2)
    }
}

/// How a thing shown at its slot while the editor is open lies (in the body's frame): as it stands
/// in the world, but a long thing (an arrow, a scroll) laid across the body, not up into the face.
pub fn shown_rot(half_extents: [f32; 3]) -> Quat {
    use std::f32::consts::FRAC_PI_2;
    let h = Vec3::from_array(half_extents).abs();
    let long = |a: f32, b: f32, c: f32| a > 2.0 * b && a > 2.0 * c;
    if long(h.y, h.x, h.z) {
        Quat::from_rotation_z(FRAC_PI_2)
    } else if long(h.z, h.x, h.y) {
        Quat::from_rotation_y(FRAC_PI_2)
    } else {
        Quat::IDENTITY
    }
}

/// The mouth (tracking space) for the head at `head`.
pub fn mouth(head: (Vec3, Quat)) -> Vec3 {
    head.0 + head.1 * MOUTH
}

/// A point (tracking space) is where a thing is read: in front of the eyes, near where they look.
pub fn readable(head: (Vec3, Quat), p: Vec3) -> bool {
    let local = head.1.inverse() * (p - head.0);
    let d = local.length();
    (READ_NEAR..READ_FAR).contains(&d) && local.z > 0.0 && local.angle_between(Vec3::Z) < READ_ANGLE
}
