//! The range's combat: training dummies (people the hands can hit, push and stab: `crate::ragdoll`,
//! published as capsules every frame), the melee weapons (`crate::arsenal`) and one spell, a fire
//! bolt, so the runtime's melee and magic can be played and tested without a game
//! (`rustvr_protocol::combat`).

use glam::Vec3;
use rustvr_melee::body::Part as BodyPart;
use rustvr_protocol::*;


/// The fire bolt's id: fire, so a hand charged with it fills a blade with fire.
pub const FIRE_BOLT: u32 = 1;
/// A shock spell: a hand charged with it fills a blade with lightning.
pub const SHOCK: u32 = 2;

/// The range is the game: a hit on one of its people does what the melee rules say
/// (`rustvr_melee`): its damage (the runtime's, by the damager's curve, tier and the material's
/// multiplier) times the body part's (a head x3, the limbs x0.5, the hands x0.25); its push level
/// (by the material's speeds) staggers or floors it by the push table; a blunt blow to the head fast
/// enough knocks it out (the tables' knockout speeds: our call, written in
/// `docs/MELEE.md`); a cut that severs takes the limb off; a hand hit at push level 2, or a stab
/// through the arm, makes it let go of what it holds; and it's left a wound where it was hit.
pub fn hit(people: &mut crate::arena::People, ev: &CombatEvent, weapons: &MeleeWeapons) -> Vec<Bled> {
    let mut out = Vec::new();
    let d = &mut people.dummies;
    let Some(k) = d.find(ev.actor) else { return out };
    let part = ev.part as usize;
    let kind = d.part_kind(k, part);
    let (a, b, _) = d.capsule(k, part);
    let point = Vec3::from_array(ev.point);
    let dir = Vec3::from_array(ev.dir).normalize_or_zero();
    // The throat: the lower part of the head's capsule (the neck).
    let throat = kind == PART_HEAD && (point - a).dot(b - a) / (b - a).length_squared().max(1e-6) < 0.45;
    let body = BodyPart::of(kind, throat);
    let weapon = weapons.get(ev.object);
    let blade = weapon.and_then(|w| w.blades().get(ev.blade as usize)).copied();
    let tier = weapon.map_or(2, |w| w.tier);
    let modifier = match (blade, ev.flags & CEF_GLANCING != 0) {
        (_, true) => rustvr_melee::MOD_BLUNT_LIGHT,
        (Some(b), _) => rustvr_melee::modifier_of(&b),
        (None, _) if ev.object == 0 => rustvr_melee::MOD_PUNCH,
        _ => rustvr_melee::MOD_BLUNT,
    };
    let fire = ev.flags & CEF_IMBUED != 0 && ev.spell == FIRE_BOLT;
    let thrown = ev.flags & CEF_THROWN != 0;
    let row = rustvr_melee::row(modifier, ev.material, tier, fire);
    let damage = ev.damage * body.damage();
    let alive = d.people[k].alive;
    d.hurt(k, damage, point);
    let share = damage / rustvr_melee::body::HEALTH;
    let now = people.dummies.time;
    if let Some(brain) = people.brain_of(k) {
        brain.hurt(share, now);
    }
    let d = &mut people.dummies;
    let level = row.map_or(0, |r| r.push_level(ev.speed, thrown));
    // The mace and shield damagers shove what they hit (an acceleration a moment
    // long: about 0.17 of the speed): felt by a body already down; a standing one's pins take it.
    let shove = if ev.what == DMG_BLUNT { 0.17 * ev.speed * 2.0 } else { 0.0 };
    d.react(k, part, level, dir, point, shove);
    // A blunt blow to the head fast enough knocks it out.
    if let Some(r) = row
        && ev.what == DMG_BLUNT
        && body == BodyPart::Head
        && ev.speed >= if thrown { r.knockout_thrown } else { r.knockout }
        && d.people[k].alive
    {
        let seconds = (rustvr_melee::body::KNOCKOUT_TIME * r.knockout_time) as f64;
        tracing::info!("range: {:#x} knocked out for {seconds:.1} s ({:.1} m/s to the head)", ev.actor, ev.speed);
        d.knock_out(k, seconds, point);
    }
    // A cut through a limb takes it off (the head by its neck).
    if ev.flags & CEF_SEVER != 0 && body.severable() && kind != PART_TORSO && d.sever(k, part, dir) {
        tracing::info!("range: {:#x}'s part {part} cut off at {:.1} m/s", ev.actor, ev.speed);
    }
    // Disarmed: a hand hit at exactly push level 2, or a point
    // through the arm.
    let hand = (0..2).find(|&h| [crate::anim::HAND[h], crate::anim::FOREARM[h], crate::anim::UPPER_ARM[h]].contains(&part));
    if let Some(h) = hand {
        let on_hand = part == crate::anim::HAND[h] && level == 2 && ev.what != DMG_PIERCE;
        let stabbed = ev.what == DMG_PIERCE && ev.flags & CEF_PENETRATED != 0;
        if (on_hand || stabbed)
            && let Some(item) = d.drop_item(k, h)
        {
            tracing::info!("range: {:#x} lets go of {item:#x}", ev.actor);
            d.happened.push(crate::ragdoll::Happened::Disarmed { id: ev.actor, item });
        }
    }
    let intensity = rustvr_melee::intensity(ev.speed);
    if ev.damage > 0.0 {
        d.wound(k, part, point, ev.what, intensity);
        out.push(Bled { actor: ev.actor, point, dir, strength: intensity, kind: ev.what });
    }
    tracing::info!(
        "range: {:#x} part {part} ({body:?}) hit by {:#x} at {:.1} m/s: {damage:.1} of its health ({:.0} left), push level {level}{}",
        ev.actor,
        ev.object,
        ev.speed,
        d.people[k].health.max(0.0),
        if alive && !d.people[k].alive { ", dead" } else { "" }
    );
    out
}

/// A blade going in deep, or out the far side (the runtime's `CE_WOUND`): its penetration damage,
/// times the part's: a stab's deep multiplier (a head or a neck x32, the torso x2), a cut's (the
/// neck x32), a skewer's.
pub fn wound(people: &mut crate::arena::People, ev: &CombatEvent) {
    let d = &mut people.dummies;
    let Some(k) = d.find(ev.actor) else { return };
    let part = ev.part as usize;
    let kind = d.part_kind(k, part);
    let (a, b, _) = d.capsule(k, part);
    let point = Vec3::from_array(ev.point);
    let throat = kind == PART_HEAD && (point - a).dot(b - a) / (b - a).length_squared().max(1e-6) < 0.45;
    let body = BodyPart::of(kind, throat);
    let mult = if ev.flags & CEF_SKEWER != 0 {
        body.skewer()
    } else if ev.what == DMG_PIERCE {
        body.deep().1
    } else {
        body.cut()
    };
    let damage = ev.damage * mult;
    d.hurt(k, damage, point);
    let now = people.dummies.time;
    if let Some(brain) = people.brain_of(k) {
        brain.hurt(damage / rustvr_melee::body::HEALTH, now);
    }
    tracing::info!("range: {:#x} part {part} ({body:?}) {}: {damage:.1}, {:.0} left", ev.actor, if ev.flags & CEF_SKEWER != 0 { "run through" } else { "wounded deep" }, people.dummies.people[k].health.max(0.0));
}

/// Where blood comes from a hit (for the range's own effects).
#[derive(Clone, Copy, Debug)]
pub struct Bled {
    pub actor: u32,
    pub point: Vec3,
    pub dir: Vec3,
    pub strength: f32,
    pub kind: u32,
}

/// The range's blood: the runtime's own look (its built-in splat), a person's worth.
pub fn bloods() -> BloodTable {
    let mut t: BloodTable = bytemuck::Zeroable::zeroed();
    let b = &mut t.bloods[0];
    b.flags = BLOOD_LIQUID;
    b.amount = 1.0;
    b.viscosity = 1.0;
    b.atlas = 1;
    Blood::set_texture(&mut b.splat, "builtin/splat");
    Blood::set_texture(&mut b.splatter, "builtin/splatter");
    Blood::set_texture(&mut b.stain, "builtin/stain");
    t.count = 1;
    t
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

/// The player's spells: a fire bolt and a shock, `selected` the one the spell hand charges.
pub fn spells_with(selected: u32) -> Spells {
    let mut s: Spells = bytemuck::Zeroable::zeroed();
    for (i, (id, name, color, element)) in [(FIRE_BOLT, "Fire bolt", [1.0, 0.45, 0.1], SPELL_FIRE), (SHOCK, "Shock", [0.5, 0.7, 1.0], SPELL_SHOCK)].into_iter().enumerate() {
        let mut spell: Spell = bytemuck::Zeroable::zeroed();
        spell.id = id;
        spell.delivery = DELIVER_TARGET;
        spell.flags = SPELL_CASTABLE | element | if id == selected { SPELL_SELECTED } else { 0 };
        spell.cost = 10.0;
        spell.color = color;
        spell.set_name(name);
        s.spells[i] = spell;
    }
    s.count = 2;
    s.magicka = 100.0;
    s.magicka_max = 100.0;
    s.cast_time = 0.5;
    s
}

/// A fire-filled blade's hit heats what it hits 6 times the hit's intensity (0 to 1) times how full
/// the blade is (some 17 hard hits in quick succession to catch fire at 100); stuck in someone, 40
/// a second times its fill (some 2.5 s).
const IMBUE_HIT_HEAT: f32 = 6.0;
const IMBUE_STUCK_HEAT: f32 = 40.0;

/// A blade filled with a spell stuck in `actor`, a step of `dt`: fire heats them, a shock keeps
/// shocking them.
pub fn imbued_stuck(people: &mut crate::arena::People, actor: u32, spell: u32, charge: f32, dt: f32) {
    let d = &mut people.dummies;
    let Some(k) = d.find(actor) else { return };
    let at = d.part_pose(k, crate::anim::TORSO).0;
    match spell {
        FIRE_BOLT => d.heat(k, IMBUE_STUCK_HEAT * charge.clamp(0.0, 1.0) * dt, at),
        SHOCK if !d.is_shocked(k) => d.shock(k, 2.0),
        _ => {}
    }
}

/// A hit with a blade filled with a spell (an imbued hit): fire adds heat until the creature
/// catches fire at 100; a shock hit over 4 m/s does 2 and stuns it two seconds (four in metal
/// armour) two times in five, always when the blade went in.
pub fn imbued_hit(people: &mut crate::arena::People, ev: &CombatEvent, charge: f32) {
    let d = &mut people.dummies;
    let Some(k) = d.find(ev.actor) else { return };
    let point = Vec3::from_array(ev.point);
    match ev.spell {
        FIRE_BOLT => {
            let heat = IMBUE_HIT_HEAT * rustvr_melee::intensity(ev.speed) * charge.clamp(0.0, 1.0);
            d.heat(k, heat, point);
        }
        SHOCK if ev.speed >= 4.0 || ev.flags & CEF_PENETRATED != 0 => {
            let metal = matches!(ev.material, MAT_CHAINMAIL | MAT_PLATE);
            let certain = ev.flags & CEF_PENETRATED != 0 || metal;
            d.hurt(k, 2.0, point);
            if certain || (ev.speed * 13.7).fract() < 0.4 {
                d.shock(k, if metal { 4.0 } else { 2.0 });
            }
        }
        _ => {}
    }
}
