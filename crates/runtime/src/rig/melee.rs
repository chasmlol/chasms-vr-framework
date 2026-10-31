//! Melee weapons in the hands (`docs/MELEE.md`):
//!
//! * **Weight.** A weapon hangs on its hand's joint ([`physics::Joint`]): a spring and damper from
//!   the controller to the point held, as strong as a hand whatever the weapon weighs, with the
//!   most force and torque it gives capped. It keeps its mass and its gravity. A dagger keeps up
//!   with the hand; a maul lags behind a fast swing, swings on past a stop and is slow to turn. The
//!   hand is drawn on the weapon, so it's held back with it; an empty hand still follows its
//!   controller exactly.
//! * **Two hands** double each hand's pull, and one of them turns it twice as hard; their pulls a
//!   distance apart along the handle turn it besides (leverage), so two hands swing a heavy weapon
//!   far better than one.
//! * **Sliding.** A hand on a handle that slides (a spear's shaft, a staff, a haft) slides along it
//!   while its trigger is held, against a friction the trigger's depth sets (a damper of 300 at a
//!   light squeeze down to 20 at a full pull): let go of the trigger, it holds where it is. At
//!   a handle's end it stops, or goes on onto the next handle (the blade, for half-swording).
//! * **Where a hand takes it** is where it is: a grab moves neither the hand nor the weapon (the
//!   user's rule, rather than pulling the weapon into a pose). The handle the hand is on says how
//!   the fingers close (round its line) and whether the hold is reversed.
//! * **Throwing.** Let go of, it leaves as it was moving; let go of with the hand faster than
//!   2 m/s (in the world's time) it's thrown, its speed doubled. One made to fly point first
//!   ([`MW_FLIES`]: a dagger, a spear), thrown with its point near the way it goes,
//!   is turned along its flight each step until it first touches something; the rest tumble. A
//!   thrown weapon is judged as it flies (`combat`), until it sticks, is taken, or comes to rest.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use super::physics::{self, Joint};
use super::{Attach, Holder, On, Rig, compose, inverse, v3};

/// How far the player reaches from the middle of their shoulders, of their height (half the
/// height, times 1.2): a held weapon stopped by something or stuck in it with its grip further off
/// than that is let go (the stuck rule).
pub const REACH_OF_HEIGHT: f32 = 0.5 * 1.2;
/// After a weapon's let go of, the hands go through it until this long (s) and then until its grip
/// is this far from both palms (metres).
pub const UNGRAB_DELAY: f64 = 0.25;
pub const UNGRAB_RADIUS: f32 = 0.13;
/// A melee weapon's air drag where its game gives none (linear 1, angular 0.05).
pub const DRAG: (f32, f32) = (1.0, 0.05);
/// A grip this far from the shoulders lets go whatever holds it.
pub const STRETCH_SNAP: f32 = 6.0;
/// The trigger slides a hand along its handle past this, with the friction going from
/// [`SLIDE_DAMPER_MAX`] here to [`SLIDE_DAMPER_MIN`] at a full pull (over the trigger's 0.2..1).
pub const SLIDE_FROM: f32 = 0.2;
pub const SLIDE_DAMPER_MAX: f32 = 300.0;
pub const SLIDE_DAMPER_MIN: f32 = 20.0;
/// A hand sliding within this of a handle's end goes on to the handle that continues it, or stops
/// there.
pub const HANDLE_HOP: f32 = 0.01;
/// The fingers close round a handle this near the point held (metres from its line).
pub const ON_HANDLE: f32 = 0.07;
/// A weapon let go of with the hand faster than this (m/s, the world's time) is thrown, its speed
/// times [`THROW_MULT`].
pub const THROW_SPEED: f32 = 2.0;
pub const THROW_MULT: f32 = 2.0;
/// A thrown weapon slower than this (m/s) has come to rest: no longer thrown.
pub const THROWN_REST: f32 = 0.1;

/// Testing (`RUSTVR_DEBUG_MELEE=1`): how far each held weapon lags its hand, a few times a second.
pub fn debug_melee() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTVR_DEBUG_MELEE").is_ok_and(|v| v == "1"))
}

/// What the runtime keeps about the game's melee weapons and the player's hold on them.
#[derive(Default)]
pub struct MeleeRig {
    /// The game's weapons' handles and damagers ([`MeleeWeapons`]), by id.
    pub weapons: std::collections::HashMap<u32, MeleeWeapon>,
    /// How strong the player's hands are against the standard player's (1; the game's own number,
    /// times the stamina left), and with the player's own setting too (this frame's).
    pub strength: f32,
    pub hold_strength: f32,
    /// Each held weapon: where its target was last frame (tracking space: the controller's own
    /// motion, without the walk), and where it goes this step if nothing stops it.
    pub held: std::collections::HashMap<u32, Held>,
    /// Each hand sliding along a handle: the weapon, which handle, where along it (metres from its
    /// `a`), and its offset from the line (weapon space).
    pub slides: [Option<Slide>; 2],
    /// The game's creatures' weapons and shields this frame (their object ids).
    pub creature_items: std::collections::HashSet<u32>,
    /// Each hand holding the world to climb: where it holds it (the hand's pose, world); how the
    /// arms move the player this frame (world units a second); where the player was last frame
    /// (the tracking origin, world); and whether climbing's allowed (`climbing`).
    pub climbs: [Option<(Vec3, Quat)>; 2],
    pub climb: Vec3,
    pub climb_from: Option<Vec3>,
    pub climb_allowed: bool,
    /// The climbing body's velocity (m/s), and the player's as the game moved them, averaged.
    pub climb_v: Vec3,
    pub climb_seen: Vec3,
    /// What each hand holds by telekinesis, and whether telekinesis is on (`telekinesis`); the
    /// creature's part each open hand marks.
    pub tk: [Option<super::telekinesis::Tk>; 2],
    pub tk_on: bool,
    pub tk_marked: [Option<(u32, usize)>; 2],
    /// The thing each hand last held, and when it let go of it (for telekinesis's choosing).
    pub tk_last_held: [Option<(u32, f64)>; 2],
    /// Weapons thrown and still flying or tumbling, by id.
    pub thrown: std::collections::HashMap<u32, Thrown>,
    /// Weapons that cut a limb off, carrying on through the creature (its id) until they're clear
    /// of it (see [`Rig::cut_through`]).
    pub cutting: std::collections::HashMap<u32, u32>,
    /// The melee buttons (the game's say): the dominant stick's click jumps off what the hands climb;
    /// the jump made this frame, for the game; the stick's click last frame.
    pub melee_buttons: bool,
    pub jump: Option<Vec3>,
    pub jump_click_was: bool,
    /// Where each held weapon last buzzed against what it rubs on, by (weapon, the collider it
    /// touches): the contact then, in the weapon's frame and in the other's.
    pub rubs: std::collections::HashMap<(u32, rapier3d::prelude::ColliderHandle), (Vec3, Vec3)>,
}

/// A thrown weapon: the hand that threw it, when, (flying point first) its point's way (weapon
/// space) and how fast it's turned along its flight, and this step's sweep: where it was before the
/// physics stepped, and where it would have gone had nothing stopped it (what it hit is judged at
/// the speed it met it, before the physics resolves the contact), at the speed it went then
/// (linear, angular).
#[derive(Clone, Copy, Debug)]
pub struct Thrown {
    pub hand: usize,
    pub since: f64,
    pub flying: Option<(Vec3, f32)>,
    pub sweep: Option<((Vec3, Quat), (Vec3, Quat))>,
    pub vel: (Vec3, Vec3),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    pub target_prev: Option<(Vec3, Quat)>,
    pub swing: Option<(Vec3, Quat)>,
    /// Sliding through the holding hand: along which way (world), at what speed. The joint's
    /// damper along it works against that, not the controller's.
    pub slide: Option<(Vec3, f32)>,
}

/// A hand sliding along weapon `id`'s handle `handle`, along the line `line` (weapon space): the
/// handle's own, or, crossing from the end of one onto the handle it goes on to (over a sword's
/// guard onto its blade), from that end to the far end of the next.
#[derive(Clone, Copy, Debug)]
pub struct Slide {
    pub id: u32,
    pub handle: usize,
    pub line: (Vec3, Vec3),
}

impl MeleeRig {
    pub fn set_weapons(&mut self, table: &MeleeWeapons) {
        self.weapons.clear();
        for w in &table.weapons[..(table.count as usize).min(MAX_MELEE_WEAPONS)] {
            if w.id != 0 {
                self.weapons.insert(w.id, *w);
            }
        }
    }

    /// The handle of weapon `id` nearest `at` (weapon space) within `reach`: its index, and the
    /// point on its line.
    pub fn handle_near(&self, id: u32, at: Vec3, reach: f32) -> Option<(usize, Vec3)> {
        let w = self.weapons.get(&id)?;
        w.handles()
            .iter()
            .enumerate()
            .map(|(k, h)| (k, on_line(v3(h.a), v3(h.b), at)))
            .filter(|(k, p)| p.distance(at) <= reach + w.handles()[*k].radius)
            .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
    }
}

/// A handle's hold by kind (`GRIP_*`): its position spring and damper multipliers, its rotation
/// spring and damper multipliers, its two-handed rotation ones, whether its turning drive is `WYZ`
/// (about the handle and across it apart: a light handle's), and whether it takes the turning when
/// both hands hold it (not a supporting hand's).
#[derive(Clone, Copy, Debug)]
pub struct GripData {
    pub pos: [f32; 2],
    pub rot: [f32; 2],
    pub rot2: [f32; 2],
    pub wyz: bool,
    pub turns: bool,
}

pub fn grip_data(kind: u32) -> GripData {
    let light = GripData { pos: [1.0, 1.0], rot: [1.1, 0.9], rot2: [1.0, 1.0], wyz: true, turns: true };
    match kind {
        GRIP_HEAVY => GripData { wyz: false, ..light },
        GRIP_SUPER_HEAVY => GripData { pos: [0.6, 0.6], rot: [0.6, 0.6], rot2: [0.6, 0.6], wyz: false, turns: true },
        GRIP_SUPPORTING => GripData { rot: [1.0, 1.0], wyz: false, turns: false, ..light },
        GRIP_HALF_SWORD => GripData { rot: [0.9, 1.1], rot2: [1.1, 0.9], wyz: false, ..light },
        GRIP_SHIELD => GripData { rot: [1.0, 1.0], wyz: false, ..light },
        GRIP_THROWING_KNIFE => GripData { rot: [0.8, 1.0], ..light },
        _ => light,
    }
}

/// A hand gripping a weapon off its handles (the grip multipliers): its linear spring and damper
/// x0.5 and its force at most 2000 N (of 3000: x2/3); its turning spring and damper x0.1 and at most
/// 100 N m (of 250: x0.4).
pub const GRIP_OFF: (f32, f32, f32, f32) = (0.5, 2.0 / 3.0, 0.1, 0.4);

/// The point of the segment `a`..`b` nearest `p`.
pub fn on_line(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-9 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

impl Rig {
    /// The game's melee weapons ([`MeleeWeapons`]): when the table changes.
    pub fn set_melee_weapons(&mut self, table: &MeleeWeapons) {
        self.melee.set_weapons(table);
    }

    /// How strong the player's hands are (1: the standard player's).
    pub fn set_melee_strength(&mut self, strength: f32) {
        self.melee.strength = if strength.is_finite() && strength > 0.0 { strength } else { 1.0 };
    }

    /// The handle a hand on weapon `id` at `at` (its grip point, weapon space) is on, if any.
    pub(super) fn melee_handle(&self, id: u32, at: Vec3) -> Option<(MeleeHandle, Vec3)> {
        let (k, p) = self.melee.handle_near(id, at, ON_HANDLE * self.map.world_per_meter)?;
        Some((self.melee.weapons[&id].handle[k], p))
    }

    /// How the hands hold weapon `id` (the player's hand joints), held by hand `hand` at `pivot`
    /// (weapon space) and maybe the other hand too, as one joint at the pivot: each hand's linear
    /// drive (a hand away from the pivot also turns it, as stiffly as its spring times its distance
    /// squared and as hard as its force times the distance) and the turning drive of the hand that
    /// takes the turning. A hand on a handle holds by that handle's kind (`GRIP_*`); off its
    /// handles, it only grips it (the grip multipliers, x0.5 and x0.1). Both hands on handles is
    /// two-handed: each hand's linear drive doubled (and its
    /// damping x1.5 on one handle), the turning on one hand only (the dominant side's, unless it's
    /// on a supporting handle), twice its one-hand value; on one handle with the grips facing
    /// opposite ways (a staff held hand over hand), a fifth of it.
    /// Also the share of its turning stiffness that's the other hand's pull (its leverage).
    pub(super) fn melee_joint(&self, id: u32, hand: usize, pivot: Vec3, strength: f32, dominant: usize) -> (Joint, f32) {
        let w = self.map.world_per_meter;
        let main = self.melee.handle_near(id, pivot, ON_HANDLE * w);
        let other = self.hands.as_ref().unwrap()[hand ^ 1].support.filter(|s| s.id == id);
        let second = other.and_then(|s| self.melee.handle_near(id, s.at, ON_HANDLE * w).map(|(k, _)| (k, s)));
        let weapon = self.melee.weapons.get(&id);
        let kind_of = |k: usize| weapon.map_or(GRIP_LIGHT, |wp| wp.handle[k].kind);
        // Strength scales the springs and the caps, the dampers by half of it past 1.
        let damp = (strength * 0.5).max(1.0);
        let lin = |k: f32, c: f32, f: f32| (physics::HOLD_SPRING * k * strength, physics::HOLD_DAMPER * c * damp, physics::HOLD_MAX_FORCE * f * strength);
        let turn = |k: f32, c: f32, t: f32| (physics::HOLD_TURN_SPRING * k * strength, physics::HOLD_TURN_DAMPER * c * damp, physics::HOLD_MAX_TORQUE * t * strength);
        // (linear drive, its place (weapon space), turning drive, its twist axis (weapon space)).
        let mut hands: Vec<((f32, f32, f32), Vec3, Option<((f32, f32, f32), Option<Vec3>)>)> = Vec::new();
        let axis_of = |k: usize| weapon.map(|wp| (v3(wp.handle[k].b) - v3(wp.handle[k].a)).normalize_or_zero());
        match (main, second) {
            (Some((km, _)), Some((ks, s))) => {
                let same = km == ks;
                let (dm, ds) = (grip_data(kind_of(km)), grip_data(kind_of(ks)));
                let c2 = if same { 1.5 } else { 1.0 };
                hands.push((lin(2.0 * dm.pos[0], c2 * dm.pos[1], 2.0), pivot, None));
                hands.push((lin(2.0 * ds.pos[0], c2 * ds.pos[1], 2.0), s.at, None));
                // Which hand turns it: the dominant side's, unless only the other is on a handle
                // that turns it (a pommel or a ricasso is a supporting hand's).
                let mine = match (dm.turns, ds.turns) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => hand == dominant,
                };
                let (k, d) = if mine { (km, dm) } else { (ks, ds) };
                let drive = if same {
                    // Hand over hand on one handle: a fifth of one hand's; else twice it, as WYZ.
                    let held_at = self.objects.get(&id).and_then(|o| match o.attach {
                        super::Attach::Fixed(p, q) => Some(inverse((p, q)).1),
                        _ => None,
                    });
                    let ax = axis_of(k).unwrap_or(Vec3::Z);
                    let opposite = held_at.is_some_and(|q| (q * Vec3::Z).dot(ax) * (s.rot * Vec3::Z).dot(ax) < 0.0);
                    if opposite { (turn(0.2 * d.rot[0], 0.2 * d.rot[1], 0.2), None) } else { (turn(2.0 * d.rot[0], 2.0 * d.rot[1], 2.0), axis_of(k)) }
                } else {
                    (turn(2.0 * d.rot[0] * d.rot2[0], 2.0 * d.rot[1] * d.rot2[1], 2.0 * d.rot2[0]), if d.wyz { axis_of(k) } else { None })
                };
                hands[if mine { 0 } else { 1 }].2 = Some(drive);
            }
            _ => {
                // Each hand on its own: by its handle's kind, or gripping it off its handles.
                let one = |on: Option<usize>| match on {
                    Some(k) => {
                        let d = grip_data(kind_of(k));
                        (lin(d.pos[0], d.pos[1], 1.0), Some((turn(d.rot[0], d.rot[1], 1.0), if d.wyz { axis_of(k) } else { None })))
                    }
                    None => (lin(GRIP_OFF.0, GRIP_OFF.0, GRIP_OFF.1), Some((turn(GRIP_OFF.2, GRIP_OFF.2, GRIP_OFF.3), None))),
                };
                let (l, t) = one(main.map(|(k, _)| k));
                hands.push((l, pivot, t));
                if let Some(s) = other {
                    let (l, t) = one(self.melee.handle_near(id, s.at, ON_HANDLE * w).map(|(k, _)| k));
                    hands.push((l, s.at, t));
                }
            }
        }
        let (pos, rot) = self.objects.get(&id).map_or((Vec3::ZERO, Quat::IDENTITY), |o| self.physics.pose(o.body));
        let mut joint = Joint { spring: 0.0, damper: 0.0, max_force: 0.0, ang_spring: 0.0, ang_damper: 0.0, max_torque: 0.0, twist_axis: None, ..Joint::one_hand(pivot, strength) };
        let mut lever = 0.0;
        for ((k, c, f), at, drive) in hands {
            let d = at.distance(pivot) / w;
            joint.spring += k;
            joint.damper += c;
            joint.max_force += f;
            joint.ang_spring += k * d * d;
            joint.ang_damper += c * d * d;
            joint.max_torque += f * d;
            lever += k * d * d;
            if let Some(((ka, ca, t), axis)) = drive {
                joint.ang_spring += ka;
                joint.ang_damper += ca;
                joint.max_torque += t;
                joint.twist_axis = axis.map(|a| (rot * a).normalize_or_zero()).filter(|a| *a != Vec3::ZERO);
            }
        }
        let _ = pos;
        (joint, if joint.ang_spring > 0.0 { lever / joint.ang_spring } else { 0.0 })
    }

    /// Weapon `id` in hand `hand`, which would have it at `target`: on the hand's joint (both
    /// hands' when the other holds it too), its weight its own. False: it's stuck an arm's length
    /// behind the hand, which lets it go.
    pub(super) fn hold_melee(&mut self, id: u32, hand: usize, target: (Vec3, Quat), inp: &super::RigInput, dt: f32) -> bool {
        let map = self.map;
        let w = map.world_per_meter;
        let o = &self.objects[&id];
        let pivot = match o.attach {
            Attach::Gun => v3(o.desc.grip),
            Attach::Fixed(p, q) => inverse((p, q)).0,
        };
        let (now_pos, now_rot) = self.physics.pose(o.body);
        let (body, collider, carry, carried) = (o.body, o.collider, o.carry, o.carried);
        let lag = (target.0 + target.1 * pivot) - (now_pos + now_rot * pivot);
        {
            let o = self.objects.get_mut(&id).unwrap();
            o.aim = Some(target);
            o.lag = lag;
        }
        // Stopped by something with its grip further than an arm's length from the shoulder: it's
        // let go (the stuck rule), rather than dragged through or snapped back.
        let grip = now_pos + now_rot * pivot;
        if self.stuck_off(grip, self.physics.in_contact(collider)) {
            tracing::info!("rig: {id:#x} stuck {:.2} m from the shoulders: hand {hand} lets go", grip.distance(self.shoulders()) / w);
            return false;
        }
        // How the controller moves and turns, from where it had the weapon last frame (in tracking
        // space: the player's walk and turn are the carry's).
        let t_now = (map.inv_point(target.0), map.inv_rot(target.1));
        let held = self.melee.held.entry(id).or_default();
        let (tv, ta) = match held.target_prev {
            Some((p, q)) => ((t_now.0 + t_now.1 * pivot / w - (p + q * pivot / w)) / dt.max(1e-4), physics::scaled_axis((t_now.1 * q.inverse()).normalize()) / dt.max(1e-4)),
            None => (Vec3::ZERO, Vec3::ZERO),
        };
        held.target_prev = Some(t_now);
        let strength = self.melee.strength.max(0.05) * inp.settings.melee.strength.max(0.05);
        let (joint, lever) = self.melee_joint(id, hand, pivot, strength, inp.settings.dominant());
        // The other hand on it pulls its own hold towards its controller: the turn it would have
        // (its hold lying towards that hand, about this one's) shares the target with this hand's
        // own, by how much of the turning is its pull (no filtering: the hands pull as they are).
        let target = match self.hands.as_ref().unwrap()[hand ^ 1].support.filter(|s| s.id == id) {
            Some(sup) if lever > 0.0 => {
                let (cp, cq) = self.hands.as_ref().unwrap()[hand ^ 1].target;
                let pivot_w = target.0 + target.1 * pivot;
                let cur = (target.1 * (sup.at - pivot)).normalize_or_zero();
                let want = (cp + cq * sup.reach - pivot_w).normalize_or_zero();
                if cur == Vec3::ZERO || want == Vec3::ZERO {
                    target
                } else {
                    let rot = target.1.slerp((Quat::from_rotation_arc(cur, want) * target.1).normalize(), lever).normalize();
                    (pivot_w - rot * pivot, rot)
                }
            }
            _ => target,
        };
        // Sliding through the hand, the weapon's way along the handle is its own.
        let mut target_vel = map.vel(tv);
        if let Some((axis, v)) = self.melee.held.get(&id).and_then(|h| h.slide) {
            target_vel += axis * (v - target_vel.dot(axis));
        }
        let joint = Joint { carry, carried, target_vel, target_ang: map.rot(Quat::IDENTITY) * ta, ..joint.in_units(w) };
        let (carried, swing) = self.physics.joint(body, target.0, target.1, dt, &joint);
        self.objects.get_mut(&id).unwrap().carried = carried;
        if debug_melee() && self.frame_index.is_multiple_of(9) {
            let turned = (target.1 * now_rot.inverse()).normalize();
            tracing::info!(
                "melee: {id:#x} in hand {hand}{}: {:.3} m behind its hold, turned {:.1} deg off, hand at {:.2} m/s",
                if joint.spring > physics::HOLD_SPRING * strength * 1.5 { " (two hands)" } else { "" },
                lag.length() / w,
                physics::scaled_axis(turned).length().to_degrees(),
                tv.length()
            );
        }
        self.melee.held.entry(id).or_default().swing = Some(swing);
        self.physics.set_gravity(body, 1.0);
        true
    }

    /// Where a held melee weapon goes this step if nothing stops it: what its blow is judged by.
    pub(super) fn melee_swing(&self, id: u32) -> Option<(Vec3, Quat)> {
        self.melee.held.get(&id).and_then(|h| h.swing)
    }

    /// Hand `i` on weapon `id` slides along the handle it's on while its trigger is held. Along the
    /// handle the hand's joint has no spring, only a damper the trigger sets (300 N s/m at a light
    /// pull, 20 at a full one): the weapon keeps its own way along the handle, dragged after
    /// the hand by the damper and pulled by its weight, and the hold moves through the fist by
    /// whatever's between them. Let go of the trigger, the hold stays where it got to.
    pub(super) fn slide_melee(&mut self, i: usize, inp: &super::RigInput, dt: f32) {
        let trigger = inp.frame.hands[i].trigger;
        let h = &self.hands.as_ref().unwrap()[i];
        // The holding hand's hold (its controller grip point on the weapon), or the other hand's (a
        // support hand on its handle).
        let (id, at, holding) = match (h.holding, h.support) {
            (Some(id), _) => match self.objects.get(&id).map(|o| o.attach) {
                Some(Attach::Fixed(p, q)) => (id, inverse((p, q)), true),
                _ => return,
            },
            (None, Some(s)) => (s.id, (s.at, s.rot), false),
            _ => {
                self.melee.slides[i] = None;
                return;
            }
        };
        let w = self.map.world_per_meter;
        // Sliding on: the line it's on (a handle's, or the way across onto the next); starting: the
        // handle it holds. Only one hand slides along a handle at a time.
        let going = self.melee.slides[i].filter(|s| s.id == id && trigger >= SLIDE_FROM);
        let start = || {
            let (k, _) = self.melee.handle_near(id, at.0, ON_HANDLE * w)?;
            let h = self.melee.weapons[&id].handle[k];
            Some(Slide { id, handle: k, line: (v3(h.a), v3(h.b)) })
        };
        let slide = going.or_else(|| start().filter(|s| trigger >= SLIDE_FROM && self.melee.weapons[&id].handle[s.handle].flags & HANDLE_SLIDE != 0));
        let other_on = self.melee.slides[i ^ 1].is_some_and(|o| o.id == id && slide.is_some_and(|s| s.handle == o.handle));
        let Some(slide) = slide.filter(|_| !other_on) else {
            self.melee.slides[i] = None;
            if let Some(held) = self.melee.held.get_mut(&id) {
                held.slide = None;
            }
            return;
        };
        if self.melee.slides[i].is_none() {
            tracing::info!("rig: hand {i} slides along handle {} of {id:#x}", slide.handle);
        }
        self.melee.slides[i] = Some(slide);
        let weapon = self.melee.weapons[&id];
        let k = slide.handle;
        let (a, b) = slide.line;
        let len = a.distance(b).max(1e-4);
        let dir = (b - a) / len;
        let Some((op, oq)) = self.obj_pose(id) else { return };
        let Some(o) = self.objects.get(&id) else { return };
        let (body, mass) = (o.body, o.desc.mass.max(0.1));
        let axis = oq * dir;
        // The weapon's own way along the handle, and the controller's.
        let (v, wv) = self.physics.velocity(body);
        let point = op + oq * at.0;
        let (com, _) = self.physics.pose(body);
        let v_item = (v + wv.cross(point - com)).dot(axis);
        let v_ctrl = self.hands.as_ref().unwrap()[i].target_vel.dot(axis);
        // The damper drags it after the hand, its weight pulls it (implicitly, so it's steady at
        // any friction): m (v' - v) = dt (c (v_ctrl - v') + m g_along). With the other hand on it
        // too, that hand keeps it where it is (its own joint): this one only moves along it.
        let squeeze = ((trigger - SLIDE_FROM) / (1.0 - SLIDE_FROM)).clamp(0.0, 1.0);
        let damper = SLIDE_DAMPER_MAX + (SLIDE_DAMPER_MIN - SLIDE_DAMPER_MAX) * squeeze;
        let both = self.hands.as_ref().unwrap().iter().filter(|h| h.holding == Some(id) || h.support.is_some_and(|s| s.id == id)).count() == 2;
        let g_along = if both { 0.0 } else { -9.81 * w * axis.y };
        let v_next = if both { v_item } else { (mass * v_item + dt * (damper * v_ctrl + mass * g_along)) / (mass + damper * dt) };
        // The hold moves through the fist by what the hand moved past the weapon.
        let now = (at.0 - a).dot(dir);
        let mut along = now + (v_ctrl - v_next) * dt;
        // At the ends: on to the handle that continues it (the handle's links: a sword's grip on over
        // its guard onto the blade, for half-swording), across the way between them, or held there
        // (the joint's spring back along the handle). Without links, a handle that starts where
        // this one ends (within [`HANDLE_HOP`]) continues it.
        let mut stopped = false;
        if along <= 0.0 || along >= len {
            let up = along >= len;
            let end = if up { b } else { a };
            let h = weapon.handle[k];
            let link = if up { h.links & 0xFF } else { (h.links >> 8) & 0xFF };
            let next = (link > 0).then(|| link as usize - 1).filter(|&n| n < weapon.handles().len() && n != k).or_else(|| {
                weapon.handles().iter().enumerate().find(|(n, hh)| *n != k && (v3(hh.a).distance(end) < HANDLE_HOP * w || v3(hh.b).distance(end) < HANDLE_HOP * w)).map(|(n, _)| n)
            });
            match next {
                Some(n) => {
                    tracing::info!("rig: hand {i} slides from handle {k} on to handle {n} of {id:#x}");
                    let nh = weapon.handle[n];
                    let far = if v3(nh.a).distance(end) > v3(nh.b).distance(end) { v3(nh.a) } else { v3(nh.b) };
                    self.melee.slides[i] = Some(Slide { id, handle: n, line: (end, far) });
                    // (On along the new line from where it's got to past the end.)
                    along = (along - if up { len } else { 0.0 }).abs();
                    let moved = end + (far - end).normalize_or_zero() * along - at.0;
                    self.slide_hold(i, id, holding, (at.0 + moved, at.1));
                    return;
                }
                None => {
                    along = along.clamp(0.0, len);
                    stopped = true;
                }
            }
        }
        if let Some(held) = self.melee.held.get_mut(&id) {
            held.slide = (!stopped && holding).then_some((axis, v_next));
        }
        let moved = (along - now) * dir;
        if moved.length() < 1e-6 {
            return;
        }
        // A buzz in the hand as it runs, the stronger the faster.
        let speed = moved.length() / w / dt.max(1e-4);
        self.slide_hold(i, id, holding, (at.0 + moved, at.1));
        if speed > 0.05 {
            self.melee_slide_felt(i, id, speed);
        }
    }

    /// Hand `i`'s hold on weapon `id` moved along it to `new_at` (weapon space): the weapon moves
    /// through the holding hand (its hold shifts, and the drawn hand with it), or a second hand
    /// moves along it.
    fn slide_hold(&mut self, i: usize, id: u32, holding: bool, new_at: (Vec3, Quat)) {
        if holding {
            let Some(Attach::Fixed(p, q)) = self.objects.get(&id).map(|o| o.attach) else { return };
            let moved = new_at.0 - inverse((p, q)).0;
            let rel = inverse(new_at);
            self.objects.get_mut(&id).unwrap().attach = Attach::Fixed(rel.0, rel.1);
            if let Some(hold) = self.hands.as_mut().unwrap()[i].hold.as_mut() {
                hold.0 += moved;
            }
        } else if let Some(s) = self.hands.as_mut().unwrap()[i].support.as_mut() {
            s.at = new_at.0;
        }
    }

    fn melee_slide_felt(&mut self, i: usize, id: u32, speed: f32) {
        let at = self.hands.as_ref().unwrap()[i].target.0;
        self.emit(super::Cue::Slide, Some(i), Some(id), at, (speed / 2.0).clamp(0.05, 1.0));
    }

    /// Hand `i` takes melee weapon `id` where it is on it: neither moves. Near one of its handles
    /// (`docs/MELEE.md`: the user's rule, never a pull into a pose) the fingers close round that
    /// handle; elsewhere round the weapon itself.
    pub(super) fn take_melee(&self, id: u32, i: usize) -> (Attach, On, Option<(Vec3, Quat)>) {
        let (palm, grip_rot) = self.hands.as_ref().unwrap()[i].target;
        let o = &self.objects[&id];
        let (pos, rot) = self.physics.pose(o.body);
        let rel = compose(inverse((palm, grip_rot)), (pos, rot));
        let hand = self.physics.pose(self.hands.as_ref().unwrap()[i].body);
        let drawn = compose(inverse((pos, rot)), hand);
        let on = if self.melee.handle_near(id, drawn.0, ON_HANDLE * self.map.world_per_meter).is_some() { On::Grip } else { On::Free };
        (Attach::Fixed(rel.0, rel.1), on, Some(drawn))
    }
}

impl Rig {
    /// Both hands on weapon `id`'s handles (a hand gripping it off its handles doesn't make it
    /// two-handed).
    pub fn two_handed_melee(&self, id: u32) -> bool {
        let Some(hands) = self.hands.as_ref() else { return false };
        let w = self.map.world_per_meter;
        let on_handle = |at: Vec3| self.melee.handle_near(id, at, ON_HANDLE * w).is_some();
        let held = hands.iter().any(|h| h.holding == Some(id)) && self.objects.get(&id).is_some_and(|o| matches!(o.attach, Attach::Fixed(p, q) if on_handle(inverse((p, q)).0)));
        held && hands.iter().any(|h| h.support.is_some_and(|s| s.id == id && on_handle(s.at)))
    }

    /// How far weapon `id`'s hold is behind where its hand would have it (world), and the most its
    /// hand's joint pulls with (N): the push a pressed blade has.
    pub fn melee_pull(&self, id: u32) -> Option<(Vec3, f32, f32)> {
        let o = self.objects.get(&id)?;
        let strength = self.melee.hold_strength.max(0.05) * if self.two_handed_melee(id) { 2.0 } else { 1.0 };
        Some((o.lag / self.map.world_per_meter, physics::HOLD_SPRING * strength, physics::HOLD_MAX_FORCE * strength))
    }

    /// Thing `id`'s mass (kg).
    pub fn mass_of(&self, id: u32) -> f32 {
        self.objects.get(&id).map_or(1.0, |o| o.desc.mass.max(0.05))
    }

    /// Thing `id` is stuck in something.
    pub fn embedded(&self, id: u32) -> bool {
        self.objects.get(&id).is_some_and(|o| o.embed.is_some())
    }

    /// The blades stuck in creatures: (weapon, creature, the deepest it's gone in (metres), where on
    /// it it went in and its way in (weapon space)).
    pub fn stuck_blades(&self) -> Vec<(u32, u32, f32, Vec3, Vec3)> {
        let mut out = Vec::new();
        for (&id, o) in &self.objects {
            for e in o.embed.iter().chain(o.through.iter()) {
                if let super::embed::Into::Actor { id: actor, .. } = e.into {
                    let back = inverse(e.rel);
                    out.push((id, actor, e.deepest, back.0 + back.1 * e.at, (e.rel.1.inverse() * e.axis).normalize_or_zero()));
                }
            }
        }
        out
    }

    /// Whether object `id` is stuck in creature `actor` (in it first, or run on into it).
    pub fn embedded_in(&self, id: u32, actor: u32) -> bool {
        self.objects.get(&id).is_some_and(|o| o.embed.iter().chain(o.through.iter()).any(|e| matches!(e.into, super::embed::Into::Actor { id, .. } if id == actor)))
    }
}

impl Rig {
    /// What the game's creatures hold this frame ([`ActorItems`]): solid to the hands and what they
    /// hold (a creature's weapon blocks the player's, and is pushed by it), not as a loose prop the
    /// hands go through.
    pub fn set_actor_items(&mut self, items: &ActorItems) {
        self.melee.creature_items = items.entries().iter().map(|i| i.id).collect();
    }

    /// A creature's weapon or shield (the game moves it in its holder's hand).
    pub(super) fn creature_item(&self, id: u32) -> bool {
        self.melee.creature_items.contains(&id)
    }

    /// What the player's held things push the creatures' weapons and shields by: each pressed on
    /// one, at the contact, the way it presses (its own speed into it, and what the hand would have
    /// it go further: the hand's joint pushing it).
    pub(super) fn item_pushes(&self) -> Vec<GamePush> {
        let mut out: Vec<GamePush> = Vec::new();
        for id in &self.melee.creature_items {
            let Some(item) = self.objects.get(id) else { continue };
            if !matches!(item.holder, Holder::Free) {
                continue;
            }
            for c in self.physics.touching(item.collider) {
                let Some(o) = self.objects.values().find(|o| o.collider == c && matches!(o.holder, Holder::Held(_))) else { continue };
                let Some((point, normal, depth)) = self.physics.overlap(c, item.collider).or_else(|| {
                    let p = self.physics.closest_point(item.collider, self.physics.pose(o.body).0);
                    let n = (p - self.physics.pose(o.body).0).normalize_or_zero();
                    (n != Vec3::ZERO).then_some((p, n, 0.0))
                }) else {
                    continue;
                };
                let vel = self.physics.velocity(o.body).0;
                let speed = (vel.dot(normal).max(0.0) + o.lag.dot(normal).max(0.0) * super::PUSH_LAG + depth * super::PUSH_OUT).min(super::PUSH_MAX * 2.0);
                if speed > 0.05 && out.len() < MAX_GAME_PUSHES / 2 {
                    out.push(GamePush { id: *id, part: 0, moving: vel.dot(normal).max(0.0), point: point.extend(1.0).to_array(), normal: normal.extend(speed).to_array(), ..Default::default() });
                }
            }
        }
        out
    }
}

/// A held weapon rubbing along what it touches buzzes its hands each time the contact has slid this
/// far (m) along the two of them ([`RUB_PERIOD`]), this hard ([`RUB_INTENSITY`]).
pub const RUB_PERIOD: f32 = 0.02;
pub const RUB_INTENSITY: f32 = 0.8;

impl Rig {
    /// Rubbing (a contact that stays): a held weapon sliding along what it touches (the
    /// world, a loose thing, a creature) buzzes the hands on it each 2 cm the contact slides, as
    /// measured on both: the point where it last buzzed, carried by each, that far apart.
    pub(super) fn rub_melee(&mut self) {
        let w = self.map.world_per_meter;
        let hands: Vec<rapier3d::prelude::ColliderHandle> = self.hands.as_ref().map_or(Vec::new(), |h| vec![h[0].collider, h[1].collider]);
        let held: Vec<(u32, usize, rapier3d::prelude::ColliderHandle, rapier3d::prelude::RigidBodyHandle)> = self
            .objects
            .iter()
            .filter_map(|(&id, o)| match o.holder {
                Holder::Held(h) if self.melee.weapons.contains_key(&id) && o.embed.is_none() => Some((id, h, o.collider, o.body)),
                _ => None,
            })
            .collect();
        let mut touching = std::collections::HashSet::new();
        for (id, hand, collider, body) in held {
            let me = self.physics.pose(body);
            for other in self.physics.touching(collider) {
                if hands.contains(&other) {
                    continue;
                }
                // (Touching within a centimetre: the solver's contacts keep a hair apart.)
                let (Some((point, ..)), Some(them)) = (self.physics.touch(collider, other, 0.01 * w), self.physics.collider_world_pose(other)) else { continue };
                touching.insert((id, other));
                let here = (inverse(me).1 * (point - me.0), inverse(them).1 * (point - them.0));
                let Some(&(on_me, on_them)) = self.melee.rubs.get(&(id, other)) else {
                    self.melee.rubs.insert((id, other), here);
                    continue;
                };
                if (me.0 + me.1 * on_me).distance(them.0 + them.1 * on_them) > RUB_PERIOD * w {
                    self.melee.rubs.insert((id, other), here);
                    for h in [hand, hand ^ 1] {
                        if h == hand || self.two_handed_melee(id) {
                            self.emit(super::feedback::Cue::Rub, Some(h), Some(id), point, RUB_INTENSITY);
                        }
                    }
                }
            }
        }
        self.melee.rubs.retain(|k, _| touching.contains(k));
    }
}

impl Rig {
    /// The hands holding the world climb ([`super::melee`]'s climbing): how the arms move the player
    /// (world, m/s), and whether any hand climbs.
    pub fn climb_velocity(&self) -> Vec3 {
        self.melee.climb
    }

    pub fn climbing(&self) -> bool {
        self.melee.climbs.iter().any(|c| c.is_some())
    }

    /// With the melee buttons or not (the game's say).
    pub fn set_melee_buttons(&mut self, on: bool) {
        self.melee.melee_buttons = on;
    }

    /// A jump off what the hands climbed, made this frame (see [`RtMelee::jump`]).
    pub fn take_climb_jump(&mut self) -> Option<Vec3> {
        self.melee.jump.take()
    }
}

impl Rig {
    /// Weapon `id` let go of by hand `i`: it leaves as it was moving, and
    /// thrown (the hand faster than [`THROW_SPEED`] in the world's time) twice as fast. False: not
    /// a melee weapon of the game's.
    pub(super) fn release_melee(&mut self, i: usize, id: u32, inp: &super::RigInput) -> bool {
        self.melee.tk_last_held[i] = Some((id, self.now));
        let Some(w) = self.melee.weapons.get(&id).copied() else { return false };
        let body = self.objects[&id].body;
        // (The hand's speed in the world's time: in slowed time a throw is that much faster.)
        let time_scale = (inp.dt / inp.frame.dt.max(1e-4)).clamp(0.05, 1.0);
        let hand_speed = self.hands.as_ref().unwrap()[i].history.recent().length() / time_scale;
        let thrown = hand_speed > THROW_SPEED;
        let (v, ang) = self.physics.velocity(body);
        let v = if thrown { v * THROW_MULT * if w.throw_mult > 0.0 { w.throw_mult } else { 1.0 } } else { v };
        // No hand contact until it's clear of the hand that let it go (checked after a quarter
        // second, a 0.13 m sphere at the grip).
        self.free_body(id, super::groups_held(i));
        self.objects.get_mut(&id).unwrap().hands_off_until = self.now + UNGRAB_DELAY;
        self.physics.set_velocity(body, v, ang);
        let (palm, _) = self.hands.as_ref().unwrap()[i].target;
        if !thrown {
            self.emit(super::feedback::Cue::Release, Some(i), Some(id), palm, 0.7);
            return true;
        }
        // Flying point first: its point within its angle of the way it goes.
        let (_, q) = self.physics.pose(body);
        let point = point_of(&w);
        let off = (q * point).angle_between(v.normalize_or(q * point)).to_degrees();
        let flying = (w.flags & MW_FLIES != 0 && off <= w.fly_angle).then_some((point, if w.fly_rate > 0.0 { w.fly_rate } else { 3.0 }));
        self.melee.thrown.insert(id, Thrown { hand: i, since: self.now, flying, sweep: None, vel: (Vec3::ZERO, Vec3::ZERO) });
        tracing::info!(
            "rig: {id:#x} thrown at {:.1} m/s (the hand {hand_speed:.1}){}",
            v.length() / self.map.world_per_meter,
            if flying.is_some() { format!(", point first ({off:.0} degrees off)") } else { String::new() }
        );
        self.emit(super::feedback::Cue::KnifeThrow, Some(i), Some(id), palm, 1.0);
        true
    }

    /// Thrown weapons, once a step before the physics: one flying point first is turned along its
    /// flight by its rate times its speed times the step, of what's left (its pitch and yaw turned,
    /// a rotation, not a torque), until it first touches something. A thrown weapon stops
    /// being thrown once it's stuck in something, taken (by a hand, or by a creature: its swings
    /// are then its own), or at rest.
    pub(super) fn fly_melee(&mut self, dt: f32) {
        let w = self.map.world_per_meter;
        let now = self.now;
        let ids: Vec<u32> = self.melee.thrown.keys().copied().collect();
        for id in ids {
            let t = self.melee.thrown[&id];
            let Some(o) = self.objects.get(&id).filter(|o| o.holder == Holder::Free && o.embed.is_none() && !self.creature_item(id)) else {
                self.melee.thrown.remove(&id);
                continue;
            };
            let (body, collider) = (o.body, o.collider);
            let (v, ang) = self.physics.velocity(body);
            if (now - t.since > 0.2 && v.length() < THROWN_REST * w) || now - t.since > 10.0 {
                self.melee.thrown.remove(&id);
                continue;
            }
            let (p, mut q) = self.physics.pose(body);
            if let Some((point, rate)) = t.flying {
                if self.physics.in_contact(collider) {
                    self.melee.thrown.get_mut(&id).unwrap().flying = None;
                } else {
                    let speed = v.length() / w;
                    let turn = Quat::from_rotation_arc((q * point).normalize_or(Vec3::Z), v.normalize_or(q * point));
                    let k = (rate * speed * dt).min(1.0);
                    q = (Quat::IDENTITY.slerp(turn, k) * q).normalize();
                    self.physics.set_pose(body, p, q);
                }
            }
            let to = (p + v * dt, (Quat::from_scaled_axis(ang * dt) * q).normalize());
            let t = self.melee.thrown.get_mut(&id).unwrap();
            t.sweep = Some(((p, q), to));
            t.vel = (v, ang);
        }
    }

    /// Whether a held thing with its grip at `grip` (world) is let go of (the stuck rule): stopped
    /// by something or stuck in it (`stopped`) with the grip further than the player's reach from
    /// the middle of their shoulders, or whatever it is, stretched past `STRETCH_SNAP`.
    pub(super) fn stuck_off(&self, grip: Vec3, stopped: bool) -> bool {
        let w = self.map.world_per_meter;
        let off = grip.distance(self.shoulders()) / w;
        // (The player is as tall as their eyes are high and a sixteenth more.)
        let reach = self.eye_height / (1.0 - 1.0 / 16.0) * REACH_OF_HEIGHT;
        (stopped && off > reach) || off > STRETCH_SNAP
    }

    /// The middle of the player's shoulders (world): below the eyes by their height over 7.5 and
    /// back by half that.
    pub(super) fn shoulders(&self) -> Vec3 {
        let down = self.eye_height / 7.5;
        let local = Vec3::new(0.0, -down, -down * 0.5);
        self.map.point(self.anchor.head + self.anchor.rot() * local)
    }

    /// Thrown weapon `id`'s sweep this step (see [`Thrown`]).
    pub fn thrown_sweep(&self, id: u32) -> Option<((Vec3, Quat), (Vec3, Quat))> {
        self.melee.thrown.get(&id)?.sweep
    }

    /// The weapons thrown and still flying or tumbling, and the hand that threw each.
    pub fn thrown_melee(&self) -> Vec<(u32, usize)> {
        self.melee.thrown.iter().map(|(&id, t)| (id, t.hand)).collect()
    }

    /// Weapon `object` just cut a limb off creature `actor` (the combat's): it carries on through
    /// (the speed it met the limb at put back, and nothing of the body holds it), passing the
    /// creature's body until it's clear of it. Held, its hand's hold carries it on; thrown, it flies
    /// on as it was.
    pub fn cut_through(&mut self, object: u32, actor: u32) {
        let Some(o) = self.objects.get(&object) else { return };
        if o.embed.is_some() {
            return;
        }
        let body = o.body;
        if let Some(&Thrown { vel: (v, ang), .. }) = self.melee.thrown.get(&object) {
            self.physics.set_velocity(body, v, ang);
        }
        self.melee.cutting.insert(object, actor);
    }

    /// The groups creature `actor`'s parts collide with: a creature's own, but not a weapon carrying
    /// on through a cut it made (the hand's held things, or loose ones, while one's thrown).
    pub(super) fn actor_groups(&self, actor: u32) -> rapier3d::prelude::InteractionGroups {
        let mut g = physics::groups_actor();
        for (&id, _) in self.melee.cutting.iter().filter(|&(_, &a)| a == actor) {
            g.filter &= !match self.objects.get(&id).map(|o| o.holder) {
                Some(Holder::Held(hand)) => physics::G_HELD[hand],
                _ => physics::G_OBJECT,
            };
        }
        g
    }

    /// Weapons carrying on through a cut, once clear of the creature (none of its parts meets them)
    /// or stuck in something, or gone: they meet it again.
    pub(super) fn end_cuts(&mut self) {
        let done: Vec<u32> = self
            .melee
            .cutting
            .iter()
            .filter(|&(&id, &actor)| {
                let Some(o) = self.objects.get(&id) else { return true };
                o.embed.is_some() || !self.proxies.iter().any(|(&(a, _), &(_, proxy, ..))| a == actor && self.physics.overlap(o.collider, proxy).is_some())
            })
            .map(|(&id, _)| id)
            .collect();
        for id in done {
            self.melee.cutting.remove(&id);
        }
    }
}

/// The way weapon `w`'s point goes (weapon space): along its first point's damager, else its +Z.
pub(super) fn point_of(w: &MeleeWeapon) -> Vec3 {
    w.blades().iter().find(|b| b.kind == DMG_PIERCE).map_or(Vec3::Z, |b| (v3(b.b) - v3(b.a)).normalize_or(Vec3::Z))
}
