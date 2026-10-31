//! Things that break (`docs/MELEE.md`). A thing the game marks [`OBJF_BREAKABLE`] (a pot, a crate,
//! a stool) breaks the moment something meets it hard enough: a knock is weighed by the hitter's
//! mass, clamp(0.5 ln m, 1, 5) (anything up to some 7.4 kg counts as 1), times the speed the two
//! meet at, and a knock whose square is 20 or more breaks it (with no health of its own: one
//! counting knock, some 4.5 m/s for a light weapon). The runtime takes it out of its world
//! ([`OS_BROKEN`]), lets the hitter carry on as it was going (its speed given back: a blade goes on
//! through a pot), and tells the game ([`CE_BROKE`]), which makes its pieces and puts it back when
//! it likes (a new `spawn_seq`). Weapons don't break by default; a game may still make one break
//! ([`MW_BREAKABLE`]).
//!
//! What it meets: one of the runtime's things weighs its own mass and goes at its speed before the
//! step; a hand weighs as a light thing; the world (or a creature) stands still and the thing's
//! own mass is weighed.

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use rapier3d::prelude::ColliderHandle;
use rustvr_protocol::*;

use super::physics::{groups_hand, groups_none};
use super::{Holder, Rig};

/// A knock this hard (squared weighed speed) or more breaks a thing.
pub const BREAK_MOMENTUM: f32 = 20.0;

#[derive(Default)]
pub struct Breaking {
    /// What each breakable thing touched after the last step (a knock is a new touch).
    touching: HashMap<u32, HashSet<ColliderHandle>>,
    /// Things broken, out of the world until the game puts them back.
    pub broken: HashSet<u32>,
    /// What broke this frame, for the game.
    pub events: Vec<CombatEvent>,
}

/// How much a hitter of `mass` kg weighs in a knock (clamp(0.5 ln m, 1, 5)).
pub fn mass_factor(mass: f32) -> f32 {
    (0.5 * mass.max(1e-3).ln()).clamp(1.0, 5.0)
}

impl Rig {
    /// How hard a knock must be to break thing `id` (squared weighed speed), if it breaks.
    fn break_momentum(&self, id: u32) -> Option<f32> {
        let o = self.objects.get(&id)?;
        if o.desc.flags & OBJF_BREAKABLE != 0 {
            return Some(BREAK_MOMENTUM);
        }
        let w = self.melee.weapons.get(&id)?;
        (w.flags & MW_BREAKABLE != 0).then_some(if w.break_momentum > 0.0 { w.break_momentum } else { BREAK_MOMENTUM })
    }

    /// Right after each step: each breakable thing that's started touching something is knocked,
    /// and breaks if the knock is hard enough.
    pub(super) fn breaks(&mut self) {
        let w = self.map.world_per_meter;
        let ids: Vec<u32> = self.objects.keys().copied().collect();
        for id in ids {
            let Some(needed) = self.break_momentum(id) else { continue };
            let o = &self.objects[&id];
            if o.embed.is_some() || !matches!(o.holder, Holder::Free | Holder::Held(_) | Holder::Pulled { .. }) {
                self.breaking.touching.remove(&id);
                continue;
            }
            let (collider, mass, own) = (o.collider, o.desc.mass, o.prev_vel);
            let now: HashSet<ColliderHandle> = self.physics.touching(collider).into_iter().collect();
            let before = self.breaking.touching.insert(id, now.clone()).unwrap_or_default();
            let Some((by, body, other_mass, other_vel)) = now.difference(&before).map(|&c| self.knocker(c, mass)).max_by(|a, b| (own - a.3).length_squared().total_cmp(&(own - b.3).length_squared())) else {
                continue;
            };
            let speed = (own - other_vel).length() / w;
            let momentum = mass_factor(other_mass) * speed;
            if momentum * momentum < needed {
                continue;
            }
            // The hitter carries on as it was going.
            if let Some(b) = body {
                let (_, ang) = self.physics.velocity(b);
                self.physics.set_velocity(b, other_vel, ang);
            }
            let dir = (other_vel - own).normalize_or_zero();
            self.break_thing(id, by, speed, momentum, dir);
        }
    }

    /// What collider `c` is, as a hitter of a thing of `mass`: (its id if it's one of the
    /// runtime's things, its body to give its speed back, its mass, its speed before the step).
    fn knocker(&self, c: ColliderHandle, mass: f32) -> (u32, Option<rapier3d::prelude::RigidBodyHandle>, f32, Vec3) {
        if let Some((&id, o)) = self.objects.iter().find(|(_, o)| o.collider == c) {
            return (id, Some(o.body), o.desc.mass, o.prev_vel);
        }
        if let Some(h) = self.hands.as_ref().and_then(|hs| hs.iter().find(|h| h.collider == c)) {
            return (0, None, 1.0, self.physics.velocity(h.body).0);
        }
        (0, None, mass, Vec3::ZERO)
    }

    /// Thing `id` broken: out of any hand, inert, out of the world, and the game told.
    fn break_thing(&mut self, id: u32, by: u32, speed: f32, momentum: f32, dir: Vec3) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        let was = o.holder;
        o.holder = Holder::Gone;
        let (body, collider) = (o.body, o.collider);
        let (p, _) = self.physics.pose(body);
        self.physics.set_kinematic(body, true);
        self.physics.set_groups(collider, groups_none());
        self.physics.set_velocity(body, Vec3::ZERO, Vec3::ZERO);
        let hands = self.hands.as_mut().unwrap();
        for (i, h) in hands.iter_mut().enumerate() {
            if h.holding == Some(id) {
                h.holding = None;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
            if h.support.is_some_and(|s| s.id == id) {
                h.support = None;
                self.physics.set_groups(h.collider, groups_hand(i));
            }
        }
        self.melee.thrown.remove(&id);
        self.melee.held.remove(&id);
        self.breaking.touching.remove(&id);
        self.breaking.broken.insert(id);
        let hand = if let Holder::Held(h) = was { h as u32 } else { 2 };
        tracing::info!("rig: {id:#x} broken by {by:#x} at {speed:.1} m/s (a knock of {:.0})", momentum * momentum);
        self.breaking.events.push(CombatEvent {
            kind: CE_BROKE,
            object: id,
            what: by,
            hand,
            speed,
            strength: momentum * momentum,
            point: p.to_array(),
            dir: dir.to_array(),
            ..Default::default()
        });
        self.emit(super::feedback::Cue::Chop, (hand < 2).then_some(hand as usize), Some(id), p, (momentum / 10.0).clamp(0.3, 1.0));
    }

    /// The things that broke since this was last asked.
    pub fn take_breaks(&mut self) -> Vec<CombatEvent> {
        std::mem::take(&mut self.breaking.events)
    }
}
