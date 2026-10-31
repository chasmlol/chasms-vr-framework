//! Telekinesis (`docs/MELEE.md`), in place of the gravity gloves when it's on (the settings, or
//! the game's say). An open, empty hand points at a loose
//! thing: the hand's ray (where it points turned 35% towards the palm, then 65% towards the line
//! from the head through the hand) picks the thing out to 6 m, a metre either side of it, within 40
//! degrees of where the head looks, nearest the ray turned 30% towards the gaze (the one already
//! marked kept unless another's 3 degrees better), in the eyes' sight. The grip catches it.
//!
//! Caught, it floats where the hand points, as far off as it was: on a soft spring (100 N/m, 10 N s
//! a metre, at most 1000 N; turning with the hand at 10, 1, at most 1000), without its weight and
//! dragged (5), the point it's sent to following the hand at up to 10 m/s (that from nothing over
//! the first second). The trigger pulls it in, the hand's bottom button pushes it away, at up to
//! 8 m/s, out to 12 m (pulled or pushed hard, it goes without drag). It comes to the hand and the
//! hand takes it where it touches it. Let go of with the hand moving faster than 1 m/s, it's thrown
//! at 20 m/s the way the hand went, a weapon point first; let go of gently, it falls. A thrown
//! weapon is caught and pulled back the same way: a recall.
//!
//! A creature's part is picked out the same way (by its ragdoll handles): caught, the game holds
//! it towards where the hand sends it on the ragdoll drive ([`GRAB_TK`]); the point follows the
//! hand at up to 3 m/s (from nothing over 3 s; at once for the dead), pulled and pushed at 3.2 m/s
//! (0.4 of a thing's), out to 6 m. Thrown, it's flung at 30 m/s. Two hands holding one body
//! that way may tear its limbs off.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use super::physics::{Joint, groups_free};
use super::{Glove, Holder, Hover, Rig};

/// Picking: out to this far (m), a sphere this wide about the ray (m), within this of where the
/// head looks (degrees); the ray turned this much towards the palm, then towards the head's line
/// through the hand, and ranked this much towards the gaze; the thing marked kept unless another
/// is this much better (degrees).
pub const TK_REACH: f32 = 6.0;
pub const TK_SPHERE: f32 = 1.0;
pub const TK_ANGLE: f32 = 40.0;
pub const TK_PALM: f32 = 0.35;
pub const TK_ARM: f32 = 0.65;
pub const TK_LOOK: f32 = 0.3;
pub const TK_STICKY: f32 = 3.0;
/// For this long after a hand held a thing (s), every other thing ranks this much worse (degrees):
/// the one just thrown is the one to catch again.
pub const TK_RECENT: f64 = 5.0;
pub const TK_RECENT_PENALTY: f32 = 10.0;
/// The hold: its spring, damper and most force (N/m, N s/m, N), turning (N m/rad, N m s/rad, N m),
/// and the drag on the thing held, moving and turning.
pub const TK_SPRING: f32 = 100.0;
pub const TK_DAMPER: f32 = 10.0;
pub const TK_MAX_FORCE: f32 = 1000.0;
pub const TK_TURN_SPRING: f32 = 10.0;
pub const TK_TURN_DAMPER: f32 = 1.0;
pub const TK_MAX_TORQUE: f32 = 1000.0;
pub const TK_DRAG: f32 = 5.0;
pub const TK_TURN_DRAG: f32 = 0.5;
/// The point it's sent to follows the hand at up to this (m/s), reached over this long (s).
pub const TK_FOLLOW: f32 = 10.0;
pub const TK_FOLLOW_RAMP: f32 = 1.0;
/// Pulled in or pushed off at up to this (m/s), out to this far (m); past this much pull, no drag.
pub const TK_PULL: f32 = 8.0;
pub const TK_FAR: f32 = 12.0;
pub const TK_NO_DRAG: f32 = 0.8;
/// Let go of with the hand faster than this (m/s), thrown at this (m/s).
pub const TK_THROW_HAND: f32 = 1.0;
pub const TK_THROW: f32 = 20.0;
/// It's at the hand (taken) this near (m).
pub const TK_AT_HAND: f32 = 0.15;
/// A creature held by telekinesis (its ragdoll handles' x2.5 force cap of 1000: at most 2500 N):
/// its point follows the hand at up to this (m/s), reached over this long (s); pulled and pushed at
/// this share of a thing's speed and out to this share of its distance; thrown, the part held at
/// this (m/s).
pub const TK_BODY_FORCE: f32 = 2500.0;
pub const TK_BODY_FOLLOW: f32 = 3.0;
pub const TK_BODY_FOLLOW_RAMP: f32 = 3.0;
pub const TK_BODY_PULL: f32 = 0.4;
pub const TK_BODY_FAR: f32 = 0.5;
pub const TK_BODY_THROW: f32 = 30.0;

/// A thing a hand holds by telekinesis (or a creature's part: then `id` is 0).
#[derive(Clone, Copy, Debug)]
pub struct Tk {
    pub id: u32,
    /// The creature and its part held, and the point held (the part's frame).
    pub body: Option<(u32, usize, Vec3)>,
    /// How far from the hand it's held (m), and where it's sent now (world).
    pub distance: f32,
    pub point: Vec3,
    /// How it's turned against the hand (from the catch), and when it was caught.
    pub turn: Quat,
    pub since: f64,
}

impl Rig {
    /// Whether telekinesis takes the gravity gloves' place now (the settings, and the game's say).
    pub fn set_telekinesis(&mut self, on: bool) {
        if !on {
            for i in 0..2 {
                if self.melee.tk[i].is_some() {
                    self.tk_drop(i, Vec3::ZERO);
                }
            }
        }
        self.melee.tk_on = on;
    }

    pub fn telekinesis_on(&self) -> bool {
        self.melee.tk_on
    }

    /// The shoulder hand `i` reaches from (world), below the eyes and out to its side, as the gloves
    /// have it.
    fn tk_shoulder(&self, i: usize, head: (Vec3, Quat)) -> Vec3 {
        let w = self.map.world_per_meter;
        let fwd = (head.1 * Vec3::Z).with_y(0.0).normalize_or(Vec3::Z);
        let right = Vec3::Y.cross(fwd).normalize_or(Vec3::X);
        head.0 - Vec3::Y * super::SHOULDER_DROP * w + right * if i == 0 { -super::SHOULDER_OUT } else { super::SHOULDER_OUT } * w
    }

    /// Hand `i`'s ray (world): where it points, turned towards its palm and then towards the line
    /// from the head through it.
    fn tk_ray(&self, i: usize, head: (Vec3, Quat)) -> (Vec3, Vec3) {
        let (p, q) = self.hands.as_ref().unwrap()[i].target;
        // (The controller points along its +Z, its palm out along +X on the left, -X on the right.)
        let point = q * Vec3::Z;
        let palm = q * Vec3::X * if i == 0 { 1.0 } else { -1.0 };
        let arm = (p - head.0).normalize_or(point);
        let d = point.lerp(palm, TK_PALM).normalize_or(point);
        (p, d.lerp(arm, TK_ARM).normalize_or(arm))
    }

    /// The creature's part hand `i` would catch (its nearest to the ray, of those in reach), and how
    /// far off the ray it is (degrees, as [`Rig::tk_target`] ranks things).
    fn tk_body_target(&self, i: usize, head: (Vec3, Quat), current: Option<(u32, usize)>) -> Option<((u32, usize), f32)> {
        let w = self.map.world_per_meter;
        let (from, ray) = self.tk_ray(i, head);
        let look = head.1 * Vec3::Z;
        let ranked = ray.lerp(look, TK_LOOK).normalize_or(ray);
        let within = |d: Vec3| d.angle_between(look).to_degrees() <= TK_ANGLE;
        if !within(ray) || !within(from - head.0) {
            return None;
        }
        let mut best: Option<((u32, usize), f32)> = None;
        for (&(actor, part), &(a, b, radius)) in &self.actor_parts {
            if !self.bodies.get(&actor).is_some_and(|bd| bd.grabbable()) {
                continue;
            }
            let p = super::creatures::surface(a, b, radius, from + ray * (from.distance((a + b) * 0.5))).0;
            let to = p - from;
            let along = to.dot(ray);
            if along <= 0.0 || along > TK_REACH * w || (to - ray * along).length() > TK_SPHERE * w || !within(p - head.0) {
                continue;
            }
            let angle = (p - from).angle_between(ranked).to_degrees() + if Some((actor, part)) == current { 0.0 } else { TK_STICKY };
            if best.is_none_or(|b| angle < b.1) {
                best = Some(((actor, part), angle));
            }
        }
        best
    }

    /// The thing hand `i` would catch, `current` the one already marked, and how far off the ray it
    /// is (degrees).
    fn tk_target(&self, i: usize, head: (Vec3, Quat), current: Option<u32>) -> Option<(u32, f32)> {
        let w = self.map.world_per_meter;
        let (from, ray) = self.tk_ray(i, head);
        let look = head.1 * Vec3::Z;
        let ranked = ray.lerp(look, TK_LOOK).normalize_or(ray);
        let within = |d: Vec3| d.angle_between(look).to_degrees() <= TK_ANGLE;
        if !within(ray) || !within(from - head.0) {
            return None;
        }
        let mut best: Option<(u32, f32)> = None;
        // (Telekinesis takes any thing a hand can: held, it weighs nothing.)
        let recent = self.melee.tk_last_held[i].filter(|&(_, t)| self.now - t < TK_RECENT).map(|(id, _)| id);
        for (id, o) in &self.objects {
            let pullable = o.desc.flags & (OBJF_PULLABLE | OBJF_GRABBABLE) != 0;
            if !pullable || o.holder != Holder::Free || o.desc.flags & OBJF_CONSTRAINED != 0 {
                continue;
            }
            let (p, _) = self.physics.pose(o.body);
            let to = p - from;
            let along = to.dot(ray);
            if along <= 0.0 || along > TK_REACH * w || (to - ray * along).length() > TK_SPHERE * w || !within(p - head.0) {
                continue;
            }
            if !self.physics.reachable(head.0, p) {
                continue;
            }
            let angle = (p - from).angle_between(ranked).to_degrees()
                + if Some(*id) == current { 0.0 } else { TK_STICKY }
                + if recent.is_some_and(|r| r != *id) { TK_RECENT_PENALTY } else { 0.0 };
            if best.is_none_or(|b| angle < b.1) {
                best = Some((*id, angle));
            }
        }
        best
    }

    /// Hand `i`, once a frame, in telekinesis's place of the gloves: an open empty hand marks what
    /// it would catch; one holding something by it pulls or pushes it.
    pub(super) fn telekinesis(&mut self, i: usize, inp: &super::RigInput) {
        let head = (inp.map.point(super::v3(inp.frame.hmd.pos)), inp.map.rot(super::q4(inp.frame.hmd.rot)));
        let w = self.map.world_per_meter;
        if let Some(tk) = self.melee.tk[i]
            && let Some((actor, part, _)) = tk.body
        {
            // A creature's part: pulled in and pushed off slower and nearer (its ragdoll handles).
            if !self.actor_parts.contains_key(&(actor, part)) {
                self.tk_let_go(i);
                return;
            }
            let c = &inp.frame.hands[i];
            let pull = c.trigger.clamp(0.0, 1.0);
            let push = if c.buttons & CTRL_A != 0 { 1.0 } else { 0.0 };
            let dt = inp.dt.max(1e-4);
            let distance = (tk.distance + (push - pull) * TK_PULL * TK_BODY_PULL * dt).clamp(0.0, TK_FAR * TK_BODY_FAR);
            self.melee.tk[i].as_mut().unwrap().distance = distance;
            return;
        }
        if let Some(tk) = self.melee.tk[i] {
            let Some(o) = self.objects.get(&tk.id).filter(|o| o.holder == Holder::Free) else {
                self.melee.tk[i] = None;
                self.set_glove(i, Glove::Idle);
                return;
            };
            let body = o.body;
            // Pulled in by the trigger, pushed off by the bottom button.
            let c = &inp.frame.hands[i];
            let pull = c.trigger.clamp(0.0, 1.0);
            let push = if c.buttons & CTRL_A != 0 { 1.0 } else { 0.0 };
            let dt = inp.dt.max(1e-4);
            let distance = (tk.distance + (push - pull) * TK_PULL * dt).clamp(0.0, TK_FAR);
            self.melee.tk[i].as_mut().unwrap().distance = distance;
            self.physics.set_linear_damping(body, if pull.max(push) > TK_NO_DRAG { 0.0 } else { TK_DRAG });
            // At the hand: taken, where it touches it.
            let (p, _) = self.physics.pose(body);
            let hand = self.hands.as_ref().unwrap()[i].target.0;
            if self.physics.distance_to(self.objects[&tk.id].collider, hand) <= TK_AT_HAND * w || p.distance(hand) <= TK_AT_HAND * w {
                tracing::info!("rig: hand {i} takes {:#x} come to it by telekinesis", tk.id);
                self.tk_let_go(i);
                self.take(tk.id, i, false);
            }
            return;
        }
        let h = &self.hands.as_ref().unwrap()[i];
        if h.holding.is_some() || h.support.is_some() || h.handle.is_some() || h.creature.is_some() || h.valve.is_some() || h.hover != Hover::None || self.melee.climbs[i].is_some() {
            if matches!(h.glove, Glove::Hover(_)) {
                self.set_glove(i, Glove::Idle);
            }
            return;
        }
        // (Only an open hand picks a thing out; closing on it, it keeps it for the grip to catch.)
        if inp.frame.hands[i].squeeze >= 0.3 {
            return;
        }
        let current = if let Glove::Hover(id) = h.glove { Some(id) } else { None };
        // A thing or a creature's part, whichever's nearer the ray (a ragdoll's handles take
        // telekinesis as any thing's do).
        let thing = self.tk_target(i, head, current);
        let body = self.tk_body_target(i, head, self.melee.tk_marked[i]);
        match (thing, body) {
            (_, Some((b, a))) if thing.is_none_or(|t| a < t.1) => {
                if self.melee.tk_marked[i] != Some(b) {
                    let palm = self.hands.as_ref().unwrap()[i].target.0;
                    self.emit(super::feedback::Cue::GloveHover, Some(i), None, palm, 0.4);
                }
                self.melee.tk_marked[i] = Some(b);
                self.set_glove(i, Glove::Idle);
            }
            (Some((id, _)), _) if Some(id) != current => {
                self.melee.tk_marked[i] = None;
                let palm = self.hands.as_ref().unwrap()[i].target.0;
                self.emit(super::feedback::Cue::GloveHover, Some(i), Some(id), palm, 0.4);
                self.set_glove(i, Glove::Hover(id));
            }
            (Some(_), _) => self.melee.tk_marked[i] = None,
            (None, _) => {
                self.melee.tk_marked[i] = None;
                self.set_glove(i, Glove::Idle);
            }
        }
    }

    /// Hand `i`'s grip on the creature's part it marks: caught where it is, the point held where
    /// the ray meets it.
    pub(super) fn tk_catch_body(&mut self, i: usize, actor: u32, part: usize) {
        let w = self.map.world_per_meter;
        let (Some(frame), Some(&(a, b, radius))) = (self.part_frame(actor, part), self.actor_parts.get(&(actor, part))) else { return };
        let (hp, hq) = self.hands.as_ref().unwrap()[i].target;
        let at = super::creatures::surface(a, b, radius, hp).0;
        let point = frame.1.inverse() * (at - frame.0);
        self.melee.tk[i] = Some(Tk { id: 0, body: Some((actor, part, point)), distance: at.distance(hp) / w, point: at, turn: hq.inverse() * frame.1, since: self.now });
        self.melee.tk_marked[i] = None;
        self.grab_serial += 1;
        tracing::info!("rig: hand {i} holds creature {actor:#x} part {part} by telekinesis, {:.1} m off", at.distance(hp) / w);
        self.emit(super::feedback::Cue::GloveLock, Some(i), None, hp, 0.8);
    }

    /// Hand `i`'s grip on what it marks: caught, held off where it is.
    pub(super) fn tk_catch(&mut self, i: usize, id: u32) {
        let Some(o) = self.objects.get(&id).filter(|o| o.holder == Holder::Free) else { return };
        // Stuck in something (a thrown dagger in a body): pulled out.
        if o.embed.is_some() {
            self.unembed(id);
        }
        let Some(o) = self.objects.get(&id) else { return };
        let (body, w) = (o.body, self.map.world_per_meter);
        let (p, q) = self.physics.pose(body);
        let (hp, hq) = self.hands.as_ref().unwrap()[i].target;
        self.physics.set_kinematic(body, false);
        self.physics.set_gravity(body, 0.0);
        self.physics.set_linear_damping(body, TK_DRAG);
        self.physics.set_angular_damping(body, TK_TURN_DRAG);
        self.melee.tk[i] = Some(Tk { id, body: None, distance: p.distance(hp) / w, point: p, turn: hq.inverse() * q, since: self.now });
        self.melee.thrown.remove(&id);
        self.set_glove(i, Glove::Locked(id));
        tracing::info!("rig: hand {i} holds {id:#x} by telekinesis, {:.1} m off", p.distance(hp) / w);
        self.emit(super::feedback::Cue::GloveLock, Some(i), Some(id), hp, 0.8);
    }

    /// Hand `i` lets go of what it holds by telekinesis: thrown if the hand was moving, else
    /// dropped.
    pub(super) fn tk_release(&mut self, i: usize) {
        let w = self.map.world_per_meter;
        let hand = self.hands.as_ref().unwrap()[i].history.recent();
        let throw = if hand.length() > TK_THROW_HAND { hand.normalize() * TK_THROW * w } else { Vec3::ZERO };
        self.tk_drop(i, throw);
    }

    /// Out of telekinesis's hold, going at `throw` (world; zero: as it is).
    fn tk_drop(&mut self, i: usize, throw: Vec3) {
        // A creature: let go, flung at 30 m/s if thrown (the game gives its body the rest).
        if let Some(Tk { body: Some((actor, _, _)), .. }) = self.melee.tk[i] {
            self.melee.tk[i] = None;
            let w = self.map.world_per_meter;
            let fling = throw.normalize_or_zero() * TK_BODY_THROW * w;
            let g = &mut self.grabs[i];
            g.flags &= !GRAB_HELD;
            g.vel = fling.to_array();
            tracing::info!("rig: hand {i} lets go of creature {actor:#x} by telekinesis{}", if throw != Vec3::ZERO { ", thrown" } else { "" });
            return;
        }
        let Some(tk) = self.tk_let_go(i) else { return };
        let Some(o) = self.objects.get(&tk.id) else { return };
        let body = o.body;
        if throw == Vec3::ZERO {
            return;
        }
        let (_, ang) = self.physics.velocity(body);
        self.physics.set_velocity(body, throw, ang);
        // A weapon goes point first (thrown by telekinesis, it's made to fly).
        if let Some(w) = self.melee.weapons.get(&tk.id).copied() {
            let point = super::melee::point_of(&w);
            let rate = if w.fly_rate > 0.0 { w.fly_rate } else { 3.0 };
            self.melee.thrown.insert(tk.id, super::melee::Thrown { hand: i, since: self.now, flying: Some((point, rate)), sweep: None, vel: (Vec3::ZERO, Vec3::ZERO) });
        }
        tracing::info!("rig: hand {i} throws {:#x} by telekinesis", tk.id);
        let palm = self.hands.as_ref().unwrap()[i].target.0;
        self.emit(super::feedback::Cue::GloveFlick, Some(i), Some(tk.id), palm, 1.0);
    }

    /// Hand `i`'s telekinesis lets go: the thing has its weight and drag again.
    fn tk_let_go(&mut self, i: usize) -> Option<Tk> {
        let tk = self.melee.tk[i].take()?;
        self.set_glove(i, Glove::Idle);
        // (A creature's part: the game's told it's let go.)
        if tk.body.is_some() {
            self.grabs[i].flags &= !GRAB_HELD;
        }
        if let Some(o) = self.objects.get(&tk.id).filter(|o| o.holder == Holder::Free) {
            let body = o.body;
            let collider = o.collider;
            self.physics.set_gravity(body, 1.0);
            self.air_drag(tk.id);
            self.physics.set_groups(collider, groups_free());
        }
        Some(tk)
    }

    /// Once a step before the physics: each thing held by telekinesis pulled on its soft spring
    /// towards where the hand sends it.
    pub(super) fn tk_hold(&mut self, inp: &super::RigInput, dt: f32) {
        let w = self.map.world_per_meter;
        let head = (inp.map.point(super::v3(inp.frame.hmd.pos)), inp.map.rot(super::q4(inp.frame.hmd.rot)));
        for i in 0..2 {
            let Some(tk) = self.melee.tk[i] else { continue };
            // A creature's part: where the hand sends it, for the game to hold it there.
            if let Some((actor, part, held)) = tk.body {
                let (hp, hq) = self.hands.as_ref().unwrap()[i].target;
                let shoulder = self.tk_shoulder(i, head);
                let goal = hp + (hp - shoulder).normalize_or(hq * Vec3::Y) * tk.distance * w;
                let dead = self.bodies.get(&actor).is_some_and(|b| b.flags & BODY_CONSCIOUS == 0);
                let ramp = if dead { 1.0 } else { ((self.now - tk.since) as f32 / TK_BODY_FOLLOW_RAMP).clamp(0.0, 1.0) };
                let point = tk.point + (goal - tk.point).clamp_length_max(TK_BODY_FOLLOW * ramp * w * dt);
                // (The grip's own speed: the game's damper works against it, a kinematic grip.)
                let moving = (point - tk.point) / dt.max(1e-4);
                self.melee.tk[i].as_mut().unwrap().point = point;
                let both = self.melee.tk[i ^ 1].is_some_and(|o| o.body.is_some_and(|b| b.0 == actor));
                let held_at = self.part_frame(actor, part).map_or(point, |f| f.0 + f.1 * held);
                self.grabs[i] = CreatureGrab {
                    serial: self.grab_serial,
                    flags: GRAB_HELD | GRAB_TK | if both { GRAB_TWO_HANDS } else { 0 },
                    actor,
                    part: part as u32,
                    point: held.to_array(),
                    force: TK_BODY_FORCE,
                    target: point.to_array(),
                    squeeze: 0.0,
                    target_rot: (hq * tk.turn).normalize().to_array(),
                    vel: moving.to_array(),
                    stretch: held_at.distance(point),
                    ang_vel: Vec3::ZERO.to_array(),
                    _p0: 0.0,
                };
                continue;
            }
            let Some(o) = self.objects.get(&tk.id) else { continue };
            let body = o.body;
            let (hp, hq) = self.hands.as_ref().unwrap()[i].target;
            let shoulder = self.tk_shoulder(i, head);
            let goal = hp + (hp - shoulder).normalize_or(hq * Vec3::Y) * tk.distance * w;
            // The point it's sent to follows the hand, at up to 10 m/s (from nothing at the catch).
            let follow = TK_FOLLOW * ((self.now - tk.since) as f32 / TK_FOLLOW_RAMP).clamp(0.0, 1.0) * w;
            let to = goal - tk.point;
            let point = tk.point + to.clamp_length_max(follow * dt);
            self.melee.tk[i].as_mut().unwrap().point = point;
            let joint = Joint {
                pivot: Vec3::ZERO,
                carry: None,
                carried: (Vec3::ZERO, Vec3::ZERO),
                target_vel: Vec3::ZERO,
                target_ang: Vec3::ZERO,
                spring: TK_SPRING,
                damper: TK_DAMPER,
                max_force: TK_MAX_FORCE,
                ang_spring: TK_TURN_SPRING,
                ang_damper: TK_TURN_DAMPER,
                max_torque: TK_MAX_TORQUE,
                twist_axis: None,
            }
            .in_units(w);
            self.physics.joint(body, point, (hq * tk.turn).normalize(), dt, &joint);
        }
    }
}
