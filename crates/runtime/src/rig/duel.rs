//! The duel's side of holding a weapon (`crate::duel`, the numbers in `rustvr_melee::duel`): each
//! hand's weapon held by plain physics (its own mass and inertia on a spring and damper from the
//! controllers, no harder than the hands are strong, its weight the arms': it moves by its own
//! momentum, trailing and carrying on; staggered the hands are weaker, in a riposte stronger),
//! resting on the player's body, sent off a clash as the duel judged it, and the creatures' weapons
//! solid to nothing the hands hold (the blades meet only by the duel's rules: no passive block stops
//! a swing).
//!
//! And for every melee: the weapon leaned forward in the hand ([`Rig::set_grip_tilt`]), as a hilt
//! held upright sits in a fist, rather than along the controller's handle.

use glam::{Quat, Vec3};
use rapier3d::prelude::{ColliderHandle, InteractionGroups, InteractionTestMode, RigidBodyHandle};
use rustvr_melee::duel as tune;

use super::Rig;
use super::physics::{G_HAND, G_HELD, G_OBJECT, G_PLAYER, Joint};

/// How the duel holds each hand's weapon.
#[derive(Clone, Debug)]
pub struct DuelHold {
    pub on: bool,
    /// Each hand's share of its strength (1: all of it; less staggered, more in a riposte).
    pub drive: [f32; 2],
    /// The player's body as the held weapons meet it (head, torso, shoulders): each capsule's body and
    /// collider, its half length and radius.
    pub body: Vec<(RigidBodyHandle, ColliderHandle, f32, f32)>,
}

impl Default for DuelHold {
    fn default() -> Self {
        DuelHold { on: false, drive: [1.0; 2], body: Vec::new() }
    }
}

/// A creature's weapon in a duel: the hands meet it (they push it, as any of the creature's parts),
/// what they hold goes through it.
pub fn groups_item() -> InteractionGroups {
    InteractionGroups::new(G_OBJECT, G_OBJECT | G_HAND[0] | G_HAND[1], InteractionTestMode::And)
}

impl Rig {
    /// The duel's rules on or off, and how strongly each hand holds its weapon this frame.
    pub fn set_duel(&mut self, on: bool, drive: [f32; 2]) {
        let d = &mut self.melee.duel;
        d.on = on;
        d.drive = drive.map(|d| if d.is_finite() && d > 0.0 { d } else { 1.0 });
    }

    pub fn duel_on(&self) -> bool {
        self.melee.duel.on
    }

    /// The player's body as capsules (world: ends, radius: their head, torso and shoulders), which
    /// what the hands hold rests on in a duel (none off it).
    pub fn set_player_body(&mut self, capsules: &[(Vec3, Vec3, f32)]) {
        let capsules = if self.melee.duel.on { capsules } else { &[] };
        let mut body = std::mem::take(&mut self.melee.duel.body);
        while body.len() > capsules.len() {
            let (b, ..) = body.pop().unwrap();
            self.physics.remove(b);
        }
        for (i, &(a, b, radius)) in capsules.iter().enumerate() {
            let half = 0.5 * a.distance(b);
            let (mid, rot) = ((a + b) * 0.5, Quat::from_rotation_arc(Vec3::Y, (b - a).normalize_or(Vec3::Y)));
            match body.get(i) {
                Some(&(rb, _, h, r)) if (h - half).abs() < 0.01 && (r - radius).abs() < 0.002 => self.physics.move_kinematic(rb, mid, rot),
                old => {
                    if let Some(&(rb, ..)) = old {
                        self.physics.remove(rb);
                    }
                    let (rb, c) = self.physics.add_actor_part(mid, rot, half, radius);
                    self.physics.set_groups(c, InteractionGroups::new(G_PLAYER, G_HELD[0] | G_HELD[1], InteractionTestMode::And));
                    if i < body.len() {
                        body[i] = (rb, c, half, radius);
                    } else {
                        body.push((rb, c, half, radius));
                    }
                }
            }
        }
        self.melee.duel.body = body;
    }

    /// How far a second hand on a weapon may be from its controller before it lets go, of the
    /// sandbox's (a duel's swing carries the hands on further).
    pub(super) fn support_slack(&self, sandbox: f32) -> f32 {
        if self.melee.duel.on { tune::SUPPORT_BREAK.max(sandbox) } else { sandbox }
    }

    /// The duel's hold of hand `hand` on weapon `id`, from the sandbox's (`joint`, metres): a spring
    /// and a damper settling it by its own mass and its inertia about the grip, no harder than the
    /// hands are strong (two hands' when both are on its handles), times the player's `strength` and
    /// the hand's share now (staggered, in a riposte), on the weapon as a whole body (its turn pulls
    /// its grip, and the hands, along: [`Joint::whole`]). Its weight is the arms' while it's held
    /// ([`Rig::duel_gravity`]).
    pub(super) fn duel_joint(&self, id: u32, hand: usize, joint: Joint, strength: f32) -> Joint {
        let Some(o) = self.objects.get(&id) else { return joint };
        let w = self.map.world_per_meter;
        let mass = self.physics.world.bodies[o.body].mass().max(0.05);
        let (pos, rot) = self.physics.pose(o.body);
        let pivot = pos + rot * joint.pivot;
        // (About the grip, swung round: the two big turns of a long thing, taken as one.)
        let inertia = self.physics.body_inertia_tensor_about(o.body, pivot);
        let swing = (0.5 * (inertia.x_axis.x + inertia.y_axis.y + inertia.z_axis.z) / (w * w)).max(1e-3);
        let two = self.two_handed_melee(id);
        let share = self.melee.duel.drive[hand] * strength;
        let (force, torque) = if two { (tune::HOLD_FORCE_TWO, tune::HOLD_TORQUE_TWO) } else { (tune::HOLD_FORCE_ONE, tune::HOLD_TORQUE_ONE) };
        let (wg, ws) = (tune::GRIP_SETTLE, tune::SWING_SETTLE);
        Joint {
            spring: mass * wg * wg,
            damper: 2.0 * tune::GRIP_DAMPING * mass * wg,
            max_force: force * share,
            ang_spring: swing * ws * ws,
            ang_damper: 2.0 * tune::SWING_DAMPING * swing * ws,
            max_torque: torque * share,
            whole: true,
            ..joint
        }
    }

    /// How much of its weight a held weapon carries itself: none in a duel (the arms bear it), all of
    /// it otherwise.
    pub(super) fn duel_gravity(&self) -> f32 {
        if self.melee.duel.on { 0.0 } else { 1.0 }
    }

    /// How far each hand's melee weapon leans forward of the controller's handle (`degrees`): turned
    /// about the palm's normal (the grip's +X) from the handle's line (its +Z) towards the knuckles
    /// (its -Y: a pistol's barrel, held, `rustvr_guns::fist`), the same for either hand. A weapon taken
    /// from now on sits so in the hand.
    pub fn set_grip_tilt(&mut self, degrees: f32) {
        let tilt = if degrees.is_finite() { Quat::from_axis_angle(Vec3::X, degrees.clamp(-90.0, 90.0).to_radians()) } else { Quat::IDENTITY };
        self.melee.tilt = [tilt; 2];
    }

    /// Held weapon `object` sent off a clash at `vel` (world units a second), its turning halved.
    pub fn kick_melee(&mut self, object: u32, vel: Vec3) {
        let Some(o) = self.objects.get(&object) else { return };
        let body = o.body;
        let (_, ang) = self.physics.velocity(body);
        self.physics.set_velocity(body, vel, ang * 0.5);
    }
}
