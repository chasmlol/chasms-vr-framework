//! The duel's side of holding a weapon (`crate::duel`, the numbers in `rustvr_melee::duel`): each
//! hand's weapon held as strongly as the duel says (weaker than the physics sandbox: it lags, swings
//! on and sags by its mass; weaker still staggered, nearly as strong in a riposte), sent off a clash
//! as the duel judged it, and the creatures' weapons solid to nothing the hands hold (the blades
//! meet only by the duel's rules: no passive block stops a swing).

use glam::Vec3;
use rapier3d::prelude::{InteractionGroups, InteractionTestMode};

use super::Rig;
use super::physics::{G_HAND, G_OBJECT};

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

    /// The duel's hold on hand `hand`'s weapon (1 off the duel).
    pub(super) fn duel_drive(&self, hand: usize) -> f32 {
        if self.melee.duel.on { self.melee.duel.drive[hand] } else { 1.0 }
    }

    /// Held weapon `object` sent off a clash at `vel` (world units a second), its turning halved.
    pub fn kick_melee(&mut self, object: u32, vel: Vec3) {
        let Some(o) = self.objects.get(&object) else { return };
        let body = o.body;
        let (_, ang) = self.physics.velocity(body);
        self.physics.set_velocity(body, vel, ang * 0.5);
    }
}
