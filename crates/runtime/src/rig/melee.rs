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

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use super::physics::{self, Joint};
use super::{Attach, On, Rig, compose, inverse, v3};

/// A held weapon this far (metres) behind where its hand would have it, and stopped by something:
/// the hand lets it go (the stuck rule: the grip point an arm's length off).
pub const STUCK_DROP: f32 = 0.7;
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
    /// How strong the player's hands are against the standard player (the game's own number, times the
    /// stamina left).
    pub strength: f32,
    /// Each held weapon: where its target was last frame (tracking space: the controller's own
    /// motion, without the walk), and where it goes this step if nothing stops it.
    pub held: std::collections::HashMap<u32, Held>,
    /// Each hand sliding along a handle: the weapon, which handle, where along it (metres from its
    /// `a`), and its offset from the line (weapon space).
    pub slides: [Option<Slide>; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    pub target_prev: Option<(Vec3, Quat)>,
    pub swing: Option<(Vec3, Quat)>,
    /// Sliding through the holding hand: along which way (world), at what speed. The joint's
    /// damper along it works against that, not the controller's.
    pub slide: Option<(Vec3, f32)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Slide {
    pub id: u32,
    pub handle: usize,
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
        let (k, p) = self.melee.handle_near(id, at, ON_HANDLE)?;
        Some((self.melee.weapons[&id].handle[k], p))
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
        // Stopped by something an arm's length behind the hand: it's let go (the stuck rule),
        // rather than dragged through or snapped back.
        if lag.length() > STUCK_DROP * w && self.physics.in_contact(collider) {
            tracing::info!("rig: {id:#x} stuck {:.2} m behind hand {hand}: let go", lag.length() / w);
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
        let mut joint = Joint::one_hand(pivot, strength);
        // The other hand on it too: both hands' joint, with the leverage of their hold apart.
        if let Some(sup) = self.hands.as_ref().unwrap()[hand ^ 1].support.filter(|s| s.id == id) {
            joint = joint.two_hands(sup.at.distance(pivot) / w);
        }
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
        let handle = self.melee.handle_near(id, at.0, ON_HANDLE * w).map(|(k, _)| (k, self.melee.weapons[&id]));
        let Some((k, weapon)) = handle.filter(|(k, wpn)| trigger >= SLIDE_FROM && wpn.handle[*k].flags & HANDLE_SLIDE != 0) else {
            self.melee.slides[i] = None;
            if let Some(held) = self.melee.held.get_mut(&id) {
                held.slide = None;
            }
            return;
        };
        if self.melee.slides[i].is_none() {
            tracing::info!("rig: hand {i} slides along handle {k} of {id:#x}");
        }
        self.melee.slides[i] = Some(Slide { id, handle: k });
        let handle = weapon.handle[k];
        let (a, b) = (v3(handle.a), v3(handle.b));
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
        // any friction): m (v' - v) = dt (c (v_ctrl - v') + m g_along).
        let squeeze = ((trigger - SLIDE_FROM) / (1.0 - SLIDE_FROM)).clamp(0.0, 1.0);
        let damper = SLIDE_DAMPER_MAX + (SLIDE_DAMPER_MIN - SLIDE_DAMPER_MAX) * squeeze;
        let g_along = -9.81 * w * axis.y;
        let v_next = (mass * v_item + dt * (damper * v_ctrl + mass * g_along)) / (mass + damper * dt);
        // The hold moves through the fist by what the hand moved past the weapon.
        let now = (at.0 - a).dot(dir);
        let mut along = now + (v_ctrl - v_next) * dt;
        // At the ends: on to the handle that continues it (the blade, for half-swording), or held
        // there (the joint's spring back along the handle).
        let mut stopped = false;
        if along <= 0.0 || along >= len {
            let end = if along <= 0.0 { a } else { b };
            let near = HANDLE_HOP * w + 0.02 * w;
            let next = weapon.handles().iter().enumerate().find(|(n, hh)| *n != k && (v3(hh.a).distance(end) < near || v3(hh.b).distance(end) < near));
            match next {
                Some((n, _)) => {
                    tracing::info!("rig: hand {i} slides from handle {k} onto handle {n} of {id:#x}");
                    self.melee.slides[i] = Some(Slide { id, handle: n });
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
        let new_at = (at.0 + moved, at.1);
        if holding {
            // The weapon moves through the hand: its hold (the weapon in the controller's frame)
            // shifts, and the drawn hand with it.
            let rel = inverse(new_at);
            self.objects.get_mut(&id).unwrap().attach = Attach::Fixed(rel.0, rel.1);
            if let Some(hold) = self.hands.as_mut().unwrap()[i].hold.as_mut() {
                hold.0 += moved;
            }
        } else if let Some(s) = self.hands.as_mut().unwrap()[i].support.as_mut() {
            s.at = new_at.0;
        }
        // A buzz in the hand as it runs, the stronger the faster.
        let speed = moved.length() / w / dt.max(1e-4);
        if speed > 0.05 {
            self.melee_slide_felt(i, id, speed);
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
        let on = if self.melee_handle(id, drawn.0).is_some() { On::Grip } else { On::Free };
        (Attach::Fixed(rel.0, rel.1), on, Some(drawn))
    }
}
