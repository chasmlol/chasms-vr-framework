//! The range's combat: training dummies (people the hands can hit, push and stab: `crate::ragdoll`,
//! published as capsules every frame), the melee weapons (`crate::arsenal`) and one spell, a fire
//! bolt, so the runtime's melee and magic can be played and tested without a game
//! (`rustvr_protocol::combat`).

use glam::{Quat, Vec3};
use rustvr_protocol::*;


/// The dummy's id, as the game knows it.
pub const DUMMY_ID: u32 = 7001;
/// Where it stands (its feet), in front of the player's start.
pub const DUMMY_AT: Vec3 = Vec3::new(0.15, 0.0, 1.15);
/// The fire bolt's id.
pub const FIRE_BOLT: u32 = 1;

/// The second dummy: off to the left, turned a little towards the player, behind the throwables'
/// table (clear of the sword's blade lying across it).
pub const DUMMY_LEFT_AT: Vec3 = Vec3::new(-0.85, 0.0, 1.88);

/// The training dummies (people, as ragdolls: `crate::ragdoll`), facing the player, among the
/// range's walls and furniture: `DUMMY_ID` at `DUMMY_AT`, the next at `DUMMY_LEFT_AT`.
pub fn dummies() -> crate::ragdoll::Dummies {
    let facing = |turn: f32| Quat::from_rotation_y(std::f32::consts::PI + turn);
    let world: Vec<(Vec3, Vec3)> = crate::content::shell().into_iter().chain(crate::content::furniture().into_iter().map(|(lo, hi, _)| (lo, hi))).collect();
    crate::ragdoll::Dummies::new(&[(DUMMY_AT, facing(0.0)), (DUMMY_LEFT_AT, facing(0.35))], DUMMY_ID, &world)
}

/// The melee weapons' blades as the runtime's simpler table has them ([`MeleeTable`]: four damagers
/// a weapon), for a runtime that reads only that; the full ones are [`MeleeWeapons`].
pub fn melee_table(weapons: &MeleeWeapons) -> MeleeTable {
    let mut t: MeleeTable = bytemuck::Zeroable::zeroed();
    for (i, w) in weapons.weapons[..(weapons.count as usize).min(MAX_MELEE_WEAPONS)].iter().take(MAX_MELEE).enumerate() {
        let mut d = MeleeDesc { id: w.id, damage: 12.0, ..Default::default() };
        for b in w.blades().iter().take(MAX_DAMAGERS) {
            d.damagers[d.count as usize] = Damager { kind: b.kind, flags: b.flags & (DMGF_BOTH_WAYS | DMGF_AXE), depth: b.depth, radius: b.radius, a: b.a, b: b.b, edge: b.edge, ..Default::default() };
            d.count += 1;
            d.reach = d.reach.max(glam::Vec3::from_array(b.b).length() + b.radius);
        }
        t.weapons[i] = d;
        t.count += 1;
    }
    t
}

pub fn spells() -> Spells {
    let mut s: Spells = bytemuck::Zeroable::zeroed();
    let mut bolt: Spell = bytemuck::Zeroable::zeroed();
    bolt.id = FIRE_BOLT;
    bolt.delivery = DELIVER_TARGET;
    bolt.flags = SPELL_SELECTED | SPELL_CASTABLE;
    bolt.cost = 10.0;
    bolt.color = [1.0, 0.45, 0.1];
    bolt.set_name("Fire bolt");
    s.spells[0] = bolt;
    s.count = 1;
    s.magicka = 100.0;
    s.magicka_max = 100.0;
    s.cast_time = 0.5;
    s
}
