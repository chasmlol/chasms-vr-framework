//! The duel's side of holding a weapon (`crate::duel`, the numbers in `rustvr_melee::duel`): each
//! hand's weapon held as heavy as it is (the hand turns and moves it no faster than it could the real
//! thing: a fast jerk leaves the blade behind, a committed swing carries on past where the hands
//! stop, a long blade sags; staggered it's barely swung, in a riposte lighter), sent off a clash as
//! the duel judged it, and the creatures' weapons solid to nothing the hands hold (the blades meet
//! only by the duel's rules: no passive block stops a swing).
//!
//! And for every melee: the weapon leaned forward in the hand ([`Rig::set_grip_tilt`]), as a hilt
//! held upright sits in a fist, rather than along the controller's handle.

use glam::{Quat, Vec3};
use rapier3d::prelude::{InteractionGroups, InteractionTestMode};
use rustvr_melee::duel as tune;

use super::Rig;
use super::physics::{self, G_HAND, G_OBJECT, Joint};

/// How the duel holds each hand's weapon.
#[derive(Clone, Copy, Debug)]
pub struct DuelHold {
    pub on: bool,
    /// Each hand's hold, of the physics sandbox's (1: as strong).
    pub drive: [f32; 2],
}

impl Default for DuelHold {
    fn default() -> Self {
        DuelHold { on: false, drive: [1.0; 2] }
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
        self.melee.duel = DuelHold { on, drive: drive.map(|d| if d.is_finite() && d > 0.0 { d } else { 1.0 }) };
    }

    pub fn duel_on(&self) -> bool {
        self.melee.duel.on
    }

    /// The duel's hold of hand `hand` on weapon `id`, from the sandbox's (`joint`, metres): as hard
    /// as holds it level against its weight and swings it at most at the duel's accelerations (two
    /// hands' when both are on its handles), times the player's `strength` and the hand's share now
    /// (staggered, in a riposte); its springs settling it as the duel says by its own mass and its
    /// inertia about the grip.
    pub(super) fn duel_joint(&self, id: u32, hand: usize, joint: Joint, strength: f32) -> Joint {
        let Some(o) = self.objects.get(&id) else { return joint };
        let w = self.map.world_per_meter;
        let rb = &self.physics.world.bodies[o.body];
        let mass = rb.mass().max(0.05);
        let (pos, rot) = self.physics.pose(o.body);
        let pivot = pos + rot * joint.pivot;
        let reach = (physics::from_v(rb.center_of_mass()) - pivot).length() / w;
        // (About the grip, swung round: the two big turns of a long thing, taken as one.)
        let inertia = self.physics.body_inertia_tensor_about(o.body, pivot);
        let swing = (0.5 * (inertia.x_axis.x + inertia.y_axis.y + inertia.z_axis.z) / (w * w)).max(1e-3);
        let two = self.two_handed_melee(id);
        let share = self.melee.duel.drive[hand] * strength;
        let (turn, move_) = if two { (tune::SWING_TWO, tune::GRIP_TWO) } else { (tune::SWING_ONE, tune::GRIP_ONE) };
        let g = 9.81;
        let (ws, wg) = (tune::SWING_SETTLE, tune::GRIP_SETTLE);
        Joint {
            spring: mass * wg * wg,
            damper: 2.0 * tune::GRIP_DAMPING * mass * wg,
            max_force: mass * (g + move_ * share),
            ang_spring: swing * ws * ws,
            ang_damper: 2.0 * tune::SWING_DAMPING * swing * ws,
            max_torque: mass * g * reach + swing * turn * share,
            ..joint
        }
    }

    /// How far each hand's melee weapon leans forward of the controller's handle (`degrees`, towards
    /// where the controller points: its aim), from the controllers this frame. A weapon taken from
    /// now on sits so in the hand.
    pub fn set_grip_tilt(&mut self, hands: &[rustvr_protocol::Controller; 2], degrees: f32) {
        self.melee.tilt = std::array::from_fn(|h| {
            let grip = Quat::from_array(hands[h].grip.rot);
            let aim = Quat::from_array(hands[h].aim.rot);
            if !(grip.is_finite() && aim.is_finite()) || grip.length_squared() < 0.5 || aim.length_squared() < 0.5 {
                return Quat::IDENTITY;
            }
            // (The aim in the hand's own frame: the handle runs along its +Z.)
            let aim_here = grip.normalize().inverse() * (aim.normalize() * Vec3::Z);
            match Vec3::Z.cross(aim_here).try_normalize() {
                Some(axis) if degrees.is_finite() => Quat::from_axis_angle(axis, degrees.clamp(-90.0, 90.0).to_radians()),
                _ => Quat::IDENTITY,
            }
        });
    }

    /// Held weapon `object` sent off a clash at `vel` (world units a second), its turning halved.
    pub fn kick_melee(&mut self, object: u32, vel: Vec3) {
        let Some(o) = self.objects.get(&object) else { return };
        let body = o.body;
        let (_, ang) = self.physics.velocity(body);
        self.physics.set_velocity(body, vel, ang * 0.5);
    }
}
