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
#[derive(Clone, Debug)]
pub struct DuelHold {
    pub on: bool,
    /// Each hand's share of its strength (1: all of it; less staggered, more in a riposte).
    pub drive: [f32; 2],
    /// Weapons a swing carried the hands on with, being brought back: where the hands have them so
    /// far (their grip, world, and their turn), by id.
    pub paced: std::collections::HashMap<u32, (Vec3, Quat)>,
}

impl Default for DuelHold {
    fn default() -> Self {
        DuelHold { on: false, drive: [1.0; 2], paced: Default::default() }
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
        if !on {
            d.paced.clear();
        }
    }

    pub fn duel_on(&self) -> bool {
        self.melee.duel.on
    }

    /// How far a second hand on a weapon may be from its controller before it lets go, of the
    /// sandbox's (a duel's swing carries the hands on further).
    pub(super) fn support_slack(&self, sandbox: f32) -> f32 {
        if self.melee.duel.on { tune::SUPPORT_BREAK.max(sandbox) } else { sandbox }
    }

    /// The duel's hold of hand `hand` on weapon `id`, from the sandbox's (`joint`, metres), the hands
    /// moving at `hand_vel` (m/s, the world's way) and turning at `hand_spin` (rad/s): as hard as
    /// holds it level against its weight, and on top of that the hands' strength bringing it after
    /// them (two hands' when both are on its handles, times the player's `strength` and the hand's
    /// share now: staggered, in a riposte), or only their braking once it's going faster than they
    /// are, so a swing carries on; its springs settling it as the duel says.
    pub(super) fn duel_joint(&self, id: u32, hand: usize, joint: Joint, strength: f32, hand_vel: Vec3, hand_spin: Vec3, turned: Vec3) -> (Joint, bool) {
        let Some(o) = self.objects.get(&id) else { return (joint, false) };
        let w = self.map.world_per_meter;
        let rb = &self.physics.world.bodies[o.body];
        let mass = rb.mass().max(0.05);
        let (pos, rot) = self.physics.pose(o.body);
        let pivot = pos + rot * joint.pivot;
        let com = physics::from_v(rb.center_of_mass());
        let reach = (com - pivot).length() / w;
        let spin = physics::from_v(rb.angvel());
        let moving = (physics::from_v(rb.linvel()) + spin.cross(pivot - com)) / w;
        // (About the grip, swung round: the two big turns of a long thing, taken as one.)
        let inertia = self.physics.body_inertia_tensor_about(o.body, pivot);
        let swing = (0.5 * (inertia.x_axis.x + inertia.y_axis.y + inertia.z_axis.z) / (w * w)).max(1e-3);
        let two = self.two_handed_melee(id);
        let share = self.melee.duel.drive[hand] * strength;
        let (force, torque) = if two { (tune::HAND_FORCE_TWO, tune::HAND_TORQUE_TWO) } else { (tune::HAND_FORCE_ONE, tune::HAND_TORQUE_ONE) };
        let (force, torque) = (force * share, torque * share);
        // How far it has carried the hands from the controllers: free so far, then the arms take it
        // back (0 to 1 over the ramp).
        let carried = ((o.lag.length() / w - tune::CARRY) / tune::CARRY_RAMP).clamp(0.0, 1.0);
        let carried = carried * carried * (3.0 - 2.0 * carried);
        // Going faster than the hands and not on its way back to them: it carries on, only braked
        // (going down, its weight carrying it on); else the hands bring it after them, gently while
        // they're still.
        let lag = o.lag / w;
        let back = lag.length() > 0.03 && (moving - hand_vel).dot(lag) > 0.0;
        let flying = !back && moving.length() > hand_vel.length() + tune::BRAKING_FROM.0;
        let turning_back = turned.length() > 0.05 && (spin - hand_spin).dot(turned) > 0.0;
        let spinning = !turning_back && spin.length() > hand_spin.length() + tune::BRAKING_FROM.1;
        let support = if flying && moving.y < -0.3 { tune::FALL_SUPPORT + (1.0 - tune::FALL_SUPPORT) * carried } else { 1.0 };
        let force = if flying { tune::BRAKE_FORCE + (force - tune::BRAKE_FORCE) * carried } else { force };
        let torque = if spinning { tune::BRAKE_TORQUE + (torque - tune::BRAKE_TORQUE) * carried } else { torque };
        if tracing::enabled!(tracing::Level::DEBUG) && self.frame_index.is_multiple_of(3) {
            tracing::debug!(
                "duel: {id:#x}{} {:.2} m from the hands ({:.2} carried), moving {:.2} m/s ({:.2} the hands'), turning {:.1} rad/s ({:.1}){}{}",
                if two { " (two hands)" } else { "" },
                o.lag.length() / w,
                carried,
                moving.length(),
                hand_vel.length(),
                spin.length(),
                hand_spin.length(),
                if flying { ", carrying on" } else { "" },
                if spinning { ", swinging on" } else { "" }
            );
        }
        let g = 9.81;
        let (ws, wg) = (tune::SWING_SETTLE, tune::GRIP_SETTLE);
        (Joint {
            spring: mass * wg * wg,
            damper: 2.0 * tune::GRIP_DAMPING * mass * wg,
            max_force: mass * g * support + force,
            ang_spring: swing * ws * ws,
            ang_damper: 2.0 * tune::SWING_DAMPING * swing * ws,
            max_torque: mass * g * reach * support + torque,
            ..joint
        }, flying || spinning)
    }

    /// Where the hands bring weapon `id` this step in a duel (it at `now`, held at `pivot`, the hands
    /// wanting it at `target` and moving at `hand_speed` m/s, turning at `hand_spin` rad/s): where
    /// they want it, but after a swing carried them on (while it's `carrying` on, from where it's got
    /// to) no faster back to it than the arms would bring it, so it comes back up smoothly rather than
    /// snapping.
    pub(super) fn duel_target(&mut self, id: u32, target: (Vec3, Quat), now: (Vec3, Quat), pivot: Vec3, hand_speed: f32, hand_spin: f32, carrying: bool, dt: f32) -> (Vec3, Quat) {
        let w = self.map.world_per_meter;
        let (now_at, to) = (now.0 + now.1 * pivot, target.0 + target.1 * pivot);
        if carrying {
            if now_at.distance(to) > 0.02 * w {
                self.melee.duel.paced.insert(id, (now_at, now.1));
            }
            return target;
        }
        let Some(&(from, from_rot)) = self.melee.duel.paced.get(&id) else { return target };
        let reach = (tune::RETURN_SPEED + tune::RETURN_WITH_HANDS * hand_speed) * dt * w;
        let gap = to - from;
        let at = if gap.length() > reach { from + gap * (reach / gap.length()) } else { to };
        let turn = (tune::RETURN_TURN + tune::RETURN_WITH_HANDS * hand_spin) * dt;
        let off = from_rot.angle_between(target.1);
        let rot = if off > turn { from_rot.slerp(target.1, turn / off).normalize() } else { target.1 };
        if at == to && rot == target.1 {
            self.melee.duel.paced.remove(&id);
        } else {
            self.melee.duel.paced.insert(id, (at, rot));
        }
        (at - rot * pivot, rot)
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
