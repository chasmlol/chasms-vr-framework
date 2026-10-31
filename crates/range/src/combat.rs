//! The range's combat: training dummies (people the hands can hit, push and stab: `crate::ragdoll`,
//! published as capsules every frame), the melee weapons (`crate::arsenal`) and one spell, a fire
//! bolt, so the runtime's melee and magic can be played and tested without a game
//! (`rustvr_protocol::combat`).

use glam::Vec3;
use rustvr_melee::body::{Part as BodyPart, Reaction};
use rustvr_protocol::*;

use crate::arena::People;
use crate::ragdoll::Dummies;


/// The fire bolt's id: fire, so a hand charged with it fills a blade with fire.
pub const FIRE_BOLT: u32 = 1;
/// A shock spell: a hand charged with it fills a blade with lightning.
pub const SHOCK: u32 = 2;

/// The range is the game: a hit on one of its people does what the melee rules say
/// (`rustvr_melee`): its damage (the runtime's, by the damager's curve, tier and the material's
/// multiplier) times the body part's (a head x3, the limbs x0.5, the hands x0.25); its push level
/// (by the material's speeds) staggers or floors it by the push table; a body that isn't standing
/// it shoves (the blade's `push`); a clean blunt blow to the head fast enough knocks it out (at the
/// tables' knockout speeds, written in `docs/MELEE.md`); a cut that severs takes the limb
/// off; a hand hit at the disarming push level, a blow on an arm whose weapon the player has hold
/// of, or a stab through the arm makes it let go of what it holds; and it's left a wound where it
/// was hit. A hit on the hand of a creature attacking does nothing, unless it cuts the hand off.
pub fn hit(people: &mut crate::arena::People, ev: &CombatEvent, weapons: &MeleeWeapons) -> Vec<Bled> {
    let mut out = Vec::new();
    let d = &mut people.dummies;
    let Some(k) = d.find(ev.actor) else { return out };
    let part = ev.part as usize;
    let kind = d.part_kind(k, part);
    let point = Vec3::from_array(ev.point);
    let dir = Vec3::from_array(ev.dir).normalize_or_zero();
    let throat = kind == PART_HEAD && on_neck(d, k, part, point);
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
    let now = d.time;
    // A hit on the hand of a creature that's attacking does nothing, unless it cuts the hand off:
    // no damage, no push, no disarm. A hit that does no damage pushes nothing.
    let (attacking, disarm) = people.brain_of(k).map_or((false, 2), |b| (b.attacking(), b.tier.difficulty().disarm));
    let damaging = ev.damage > 0.0 && !(kind == PART_HAND && attacking && ev.flags & CEF_SEVER == 0);
    let d = &mut people.dummies;
    if damaging {
        // A stab through the throat cuts its voice.
        d.people[k].last_hit = (ev.what, body);
        d.people[k].silenced |= ev.what == DMG_PIERCE && body == BodyPart::Neck;
        d.hurt(k, damage, point);
        if let Some(brain) = people.brain_of(k) {
            brain.hurt(damage / rustvr_melee::body::HEALTH, now);
        }
    }
    let d = &mut people.dummies;
    let level = row.map_or(0, |r| r.push_level(ev.speed, thrown));
    if damaging {
        d.react(k, part, level, dir, point, false);
    }
    // A blow from the hand shoves a body that isn't standing, a moment long (a mace's, a shield's,
    // a fist's; a sword's not at all).
    if !thrown {
        let (force, seconds, part_x, other_x, impulse) = match blade {
            Some(b) => add_force(&b),
            None if ev.object == 0 => PUNCH_FORCE,
            None => (0.0, 0.0, 0.0, 0.0, false),
        };
        d.shove(k, part, point, dir * ev.speed, force, seconds, part_x, other_x, impulse);
    }
    // A clean blunt blow to the head fast enough knocks it out (for the standard 6 s, at the
    // tables' knockout speeds).
    if let Some(r) = row
        && damaging
        && ev.what == DMG_BLUNT
        && ev.flags & CEF_GLANCING == 0
        && body == BodyPart::Head
        && ev.speed >= if thrown { r.knockout_thrown } else { r.knockout }
        && d.people[k].alive
    {
        tracing::info!("range: {:#x} knocked out ({:.1} m/s to the head)", ev.actor, ev.speed);
        d.knock_out(k, point);
    }
    // A cut through a limb takes it off (the head by its neck).
    if ev.flags & CEF_SEVER != 0 && body.severable() && kind != PART_TORSO && d.sever(k, part, dir) {
        tracing::info!("range: {:#x}'s part {part} cut off at {:.1} m/s", ev.actor, ev.speed);
    }
    // Disarmed: a blow on an arm whose weapon the player has hold of too, at its tier's disarming
    // push level or more (`disarm`: 2, the armoured 3); a point through the arm; or a hand hit at
    // exactly that level, but not the hand with a shield.
    let hand = (0..2).find(|&h| [crate::anim::HAND[h], crate::anim::FOREARM[h], crate::anim::UPPER_ARM[h]].contains(&part));
    if let Some(h) = hand
        && damaging
    {
        let held = d.people[k].hands[h].map(|i| (d.items[i].players, d.items[i].shield));
        let wrenched = held.is_some_and(|(players, _)| players) && level >= disarm;
        let on_hand = part == crate::anim::HAND[h] && level == disarm && !held.is_some_and(|(_, shield)| shield);
        let stabbed = ev.what == DMG_PIERCE && ev.flags & CEF_PENETRATED != 0;
        if (wrenched || on_hand || stabbed)
            && let Some(item) = d.drop_item(k, h)
        {
            tracing::info!("range: {:#x} lets go of {item:#x}", ev.actor);
            d.happened.push(crate::ragdoll::Happened::Disarmed { id: ev.actor, item });
        }
    }
    let intensity = rustvr_melee::intensity(ev.speed);
    d.wound(k, part, point, ev.what, intensity, ev.damage > 0.0, ev.flags & CEF_PENETRATED != 0, fire);
    if ev.damage > 0.0 {
        out.push(Bled { point, dir, strength: intensity });
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
    // Run through: the point's way out is a stab's wound too (a skewer's exit, 0.15 at full
    // strength).
    if ev.flags & CEF_SKEWER != 0
        && let Some(out) = d.exit_point(k, part, point, Vec3::from_array(ev.dir))
    {
        d.wound(k, part, out, DMG_PIERCE, 1.0, true, true, false);
    }
    let now = people.dummies.time;
    if let Some(brain) = people.brain_of(k) {
        brain.hurt(damage / rustvr_melee::body::HEALTH, now);
    }
    tracing::info!("range: {:#x} part {part} ({body:?}) {}: {damage:.1}, {:.0} left", ev.actor, if ev.flags & CEF_SKEWER != 0 { "run through" } else { "wounded deep" }, people.dummies.people[k].health.max(0.0));
}

/// Where blood comes from a hit (for the range's own effects).
#[derive(Clone, Copy, Debug)]
pub struct Bled {
    pub point: Vec3,
    pub dir: Vec3,
    pub strength: f32,
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

/// A blade filled with a spell stuck in `actor`'s part `part`, a step of `dt`: fire heats them, a
/// shock keeps shocking them (four seconds a time in metal armour).
pub fn imbued_stuck(people: &mut crate::arena::People, actor: u32, part: u32, spell: u32, charge: f32, dt: f32) {
    let d = &mut people.dummies;
    let Some(k) = d.find(actor) else { return };
    let at = d.part_pose(k, crate::anim::TORSO).0;
    let metal = matches!(d.part_material(k, part as usize), MAT_CHAINMAIL | MAT_PLATE);
    match spell {
        FIRE_BOLT => d.heat(k, IMBUE_STUCK_HEAT * charge.clamp(0.0, 1.0) * dt, at),
        SHOCK if !d.is_shocked(k) => d.shock(k, if metal { 4.0 } else { 2.0 }),
        _ => {}
    }
}

/// A hit with a blade filled with a spell: fire adds heat until the creature catches fire at 100; a
/// shock hit over 4 m/s does 2 at push level 1 (the shock bolt's) and stuns it two seconds (four in
/// metal armour) two times in five (`SHOCK_CHANCE`: an imbued blade's hit is always at full
/// intensity), always when the blade went in or met metal.
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
            d.react(k, ev.part as usize, 1, Vec3::from_array(ev.dir).normalize_or_zero(), point, true);
            if certain || d.chance() < SHOCK_CHANCE {
                d.shock(k, if metal { 4.0 } else { 2.0 });
            }
        }
        _ => {}
    }
}

/// A shock hit stuns this often.
const SHOCK_CHANCE: f32 = 0.4;

/// Whether `point` (world) on person `k`'s head part `part` is on its neck (by a person's
/// proportions): below its jaw, which runs from its chin at the front (the lower 45% of the head's
/// capsule) back to the base of its skull (its lower 80%).
pub fn on_neck(d: &Dummies, k: usize, part: usize, point: Vec3) -> bool {
    let (a, b, _) = d.capsule(k, part);
    let (_, rot) = d.part_pose(k, part);
    let up = (b - a).normalize_or(Vec3::Y);
    let share = (point - a).dot(b - a) / (b - a).length_squared().max(1e-6);
    let out = point - a - up * (point - a).dot(up);
    let front = (rot * Vec3::Z).dot(out.normalize_or_zero());
    share < 0.625 - 0.175 * front
}

/// `DMGF_PUSH_IMPULSE` (the protocol's, once it has it): a blade's shove is an impulse.
const DMGF_PUSH_IMPULSE: u32 = 1 << 7;

/// A blade's shove (`MeleeBlade::push`: the shove, its duration and its multipliers
/// on the part hit and on the rest, its maces' 2 and 0.5 where it gives none), and whether it's an
/// impulse rather than an acceleration.
fn add_force(b: &MeleeBlade) -> (f32, f32, f32, f32, bool) {
    let [force, seconds, ..] = b.push;
    let part_x = b.push.get(2).copied().unwrap_or(2.0);
    let other_x = b.push.get(3).copied().unwrap_or(0.5);
    (force, seconds, part_x, other_x, b.flags & DMGF_PUSH_IMPULSE != 0)
}

/// A punch's: 100 for 0.15 s, x2 on the part hit, x0.5 on the rest, an acceleration.
const PUNCH_FORCE: (f32, f32, f32, f32, bool) = (100.0, 0.15, 2.0, 0.5, false);

/// The table a blade's bad-angle blow is judged by: its own, or a sword's light blunt (as
/// `rustvr_melee`'s `bad_angle` has it, once that's in).
fn bad_angle_table(b: &MeleeBlade) -> u32 {
    if b.bad_modifier != 0 { b.bad_modifier } else { rustvr_melee::MOD_BLUNT_LIGHT }
}

/// The push a weapon passes on to a guard (its speed modifier, its push level modifier, and the
/// least and most push level it passes on) for the range's weapons by their keys: the longsword,
/// the mace, the staff and the maul pass their blows on; the arming sword, the dagger, the axe, the
/// spear and the shield don't.
fn transfer(key: &str) -> Option<(f32, i32, u32, u32)> {
    match key {
        "kit_longsword" => Some((-6.0, 0, 2, 3)),
        "kit_mace" | "kit_staff" => Some((-2.0, 0, 2, 3)),
        "kit_maul" => Some((0.0, 0, 2, 4)),
        _ => None,
    }
}

/// A swing meeting the player's weapon counts from this (m/s); a shield push shoves what weighs no
/// more than this (kg, a shield counted half again).
const RECOIL_MIN_SPEED: f32 = 1.0;
const MAX_PUSH_ITEM_MASS: f32 = 2.99;

/// What a fighter's brain made of the player's weapon meeting its own.
enum Met {
    Nothing,
    Shoved(f32),
    Recoils,
    Guarded,
}

/// The player's weapon (`key`, its damager `blade` nearest the contact, weighing `mass`) met a
/// fighter's (the runtime's `CE_BLOCK`), by what the fighter was doing:
/// * its swing, in its damage window: a shield push shoves the player along its forward (unless what
///   it met is too heavy), else, met at over 1 m/s, it's parried (by its roll: a chain, nothing, or
///   a full recoil);
/// * its guard: a weapon that passes its blows on pushes it at its push level against plate (the
///   blade's own table or its bad angle's, whichever is more, with the weapon's speed and level
///   modifiers, at most 4): a light stagger at 2, a medium one at 3, and at 4 it's floored, flung
///   back and lets go of what it guarded with; else its block's roll (a riposte, nothing, a recoil).
///
/// Returns how the player is shoved (m/s), if they are.
pub fn block(people: &mut People, ev: &CombatEvent, weapons: &MeleeWeapons, key: &str, blade: Option<usize>, mass: f32) -> Option<Vec3> {
    let k = people.dummies.find(ev.actor)?;
    let now = people.dummies.time;
    let point = Vec3::from_array(ev.point);
    let dir = Vec3::from_array(ev.dir).normalize_or_zero();
    // Which of its hands holds what the player's weapon met.
    let i = people.dummies.items.iter().position(|it| it.id == ev.what)?;
    let (who, hand) = people.dummies.items[i].holder?;
    if who != k {
        return None;
    }
    let guard_shield = people.dummies.items[i].shield;
    let weapon = weapons.get(ev.object);
    let player_shield = weapon.is_some_and(|w| w.flags & MW_SHIELD != 0);
    let face = people.dummies.people[k].at.1;
    // A blow passed on to its guard: its push level against plate.
    let passed = transfer(key).zip(weapon).map(|((speed_mod, level_mod, least, most), w)| {
        let speed = (ev.speed + speed_mod).max(0.0);
        let level_of = |m: u32| rustvr_melee::row(m, MAT_PLATE, w.tier, false).map_or(0, |r| r.push_level(speed, false));
        let b = blade.and_then(|i| w.blades().get(i)).copied();
        let mut level = b.map_or(0, |b| level_of(rustvr_melee::modifier_of(&b)));
        if let Some(b) = b.filter(|b| b.flags & DMGF_BLUNT_BAD != 0) {
            level = level.max(level_of(bad_angle_table(&b)));
        }
        let level = ((level as i32 + level_mod).max(0) as u32).min(most).min(4);
        (level, least.max(1), speed)
    });
    let brain = people.brain_of(k)?;
    let met = if brain.hurting(now)[hand] {
        match brain.pushing(now) {
            Some(push) if mass * if player_shield { 1.5 } else { 1.0 } <= MAX_PUSH_ITEM_MASS => Met::Shoved(push),
            Some(_) => Met::Nothing,
            None if ev.speed > RECOIL_MIN_SPEED && brain.parried(now, player_shield) => Met::Recoils,
            None => Met::Nothing,
        }
    } else if brain.attacking() || !brain.defends() {
        Met::Nothing
    } else if passed.is_some_and(|(level, least, _)| level >= least) {
        Met::Guarded
    } else if brain.blocked(now, ev.speed, guard_shield) {
        Met::Recoils
    } else {
        Met::Nothing
    };
    let d = &mut people.dummies;
    match met {
        Met::Shoved(push) => {
            tracing::info!("range: {:#x}'s shield push shoves the player at {push:.0} m/s", ev.actor);
            return Some((face * Vec3::Z).with_y(0.0).normalize_or_zero() * push);
        }
        Met::Recoils => d.reel(k, 1.0, -dir),
        Met::Guarded => {
            let (level, _, speed) = passed.unwrap_or_default();
            tracing::info!("range: {:#x}'s guard takes a push level {level} blow ({:.1} m/s)", ev.actor, ev.speed);
            if d.react(k, crate::anim::TORSO, level, dir, point, false) == Reaction::Destabilize {
                // Floored through its guard: flung back, and it lets go of what it guarded with.
                d.fling(k, dir * speed);
                if let Some(item) = d.drop_item(k, hand) {
                    d.happened.push(crate::ragdoll::Happened::Disarmed { id: ev.actor, item });
                }
            }
        }
        Met::Nothing => {}
    }
    None
}
