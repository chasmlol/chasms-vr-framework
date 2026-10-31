//! The range's combat: training dummies (people the hands can hit, push and stab: `crate::ragdoll`,
//! published as capsules every frame), the melee weapons (`crate::arsenal`), and what spells do to
//! them (a fire bolt's and a shock's hits, a blade filled with either), so the runtime's melee and
//! magic can be played and tested without a game (`rustvr_protocol::combat`); and the range's guns,
//! grenades and throwing knives on them too. The player's spells are `crate::magic`'s.

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
        (Some(b), true) => rustvr_melee::bad_angle(&b).0,
        (None, true) => rustvr_melee::MOD_BLUNT_LIGHT,
        (Some(b), _) => rustvr_melee::modifier_of(&b),
        // (A hand's blow, or a foot's: "hands" 2 and 3 are the feet.)
        (None, _) if ev.object == 0 && ev.hand >= 2 => rustvr_melee::MOD_KICK,
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
    let intensity = blow_intensity(ev, weapons);
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

/// A gun's round on a creature (`docs/MELEE.md`): its damage before
/// the armour's and the part's multipliers (a head x3, the limbs x0.5, the hands x0.25), the push
/// level it lands at, how fast it sends the part it hits along its way, and whether it carries on
/// through flesh (out the far side, a second wound).
#[derive(Clone, Copy, Debug)]
pub struct Round {
    pub damage: f32,
    pub level: u32,
    pub speed: f32,
    pub through: bool,
}

/// A pistol's: four to the body, two to the head. A rifle's: two to the body, one to the head, and
/// through. A shotgun's: one to the body staggers it hard.
pub const PISTOL: Round = Round { damage: 15.0, level: 1, speed: 3.0, through: false };
pub const RIFLE: Round = Round { damage: 30.0, level: 2, speed: 4.0, through: true };
pub const SHOTGUN: Round = Round { damage: 45.0, level: 3, speed: 6.0, through: false };

/// What armour leaves of a round (our call): leather most of it, chainmail more than half, plate
/// less than half (and it sparks).
fn armour_lets_through(material: u32) -> f32 {
    match material {
        MAT_LEATHER => 0.9,
        MAT_CHAINMAIL => 0.6,
        MAT_PLATE => 0.4,
        _ => 1.0,
    }
}

/// A round from a gun into person `k`'s part `part` at `point` (world), along `dir`: it's hurt by
/// the round's damage (the throat's cuts its voice, as a stab's does), reels or falls at its push
/// level, the part is sent along the shot, a round through the arm or the hand makes it let go of
/// what it holds (as a stab through the arm does), and it's left a wound (and one where a round
/// that carries on through comes out).
pub fn shot(people: &mut crate::arena::People, k: usize, part: usize, point: Vec3, dir: Vec3, round: Round) -> Vec<Bled> {
    let d = &mut people.dummies;
    let kind = d.part_kind(k, part);
    let material = d.part_material(k, part);
    let throat = kind == PART_HEAD && on_neck(d, k, part, point);
    let body = BodyPart::of(kind, throat);
    let damage = round.damage * armour_lets_through(material) * body.damage();
    let (id, alive, now) = (d.people[k].id, d.people[k].alive, d.time);
    d.people[k].last_hit = (DMG_PIERCE, body);
    d.people[k].silenced |= body == BodyPart::Neck;
    d.hurt(k, damage, point);
    if let Some(brain) = people.brain_of(k) {
        brain.hurt(damage / rustvr_melee::body::HEALTH, now);
    }
    let d = &mut people.dummies;
    d.react(k, part, round.level, dir, point, false);
    d.speed_up(k, part, point, dir, round.speed);
    if let Some(h) = (0..2).find(|&h| [crate::anim::HAND[h], crate::anim::FOREARM[h], crate::anim::UPPER_ARM[h]].contains(&part))
        && let Some(item) = d.drop_item(k, h)
    {
        tracing::info!("range: {id:#x}, shot through the arm, lets go of {item:#x}");
        d.happened.push(crate::ragdoll::Happened::Disarmed { id, item });
    }
    d.wound(k, part, point, DMG_PIERCE, 1.0, true, true, false);
    let mut out = vec![Bled { point, dir: -dir, strength: 0.8 }];
    if round.through
        && matches!(material, MAT_FLESH | MAT_LEATHER)
        && let Some(exit) = d.exit_point(k, part, point, dir)
    {
        d.wound(k, part, exit, DMG_PIERCE, 1.0, true, true, false);
        out.push(Bled { point: exit, dir, strength: 1.0 });
    }
    tracing::info!("range: {id:#x} part {part} ({body:?}) shot: {damage:.1} of its health ({:.0} left){}", d.people[k].health.max(0.0), if alive && !d.people[k].alive { ", dead" } else { "" });
    out
}

/// A grenade's blast on the creatures (`docs/MELEE.md`): how far
/// it reaches (m), the damage at its centre (falling off with the square of the distance, against a
/// person's 50: a frag kills within about 2 m), within what it floors a person and tears its limbs
/// off, how fast it throws a body out at its centre, and the heat it gives (100: on fire).
#[derive(Clone, Copy, Debug)]
pub struct Blast {
    pub radius: f32,
    pub damage: f32,
    pub floor: f32,
    pub tear: f32,
    pub speed: f32,
    pub heat: f32,
}

pub const FRAG: Blast = Blast { radius: 6.0, damage: 120.0, floor: 4.0, tear: 1.2, speed: 8.0, heat: 0.0 };
pub const FIRE: Blast = Blast { radius: 2.5, damage: 15.0, floor: 1.5, tear: 0.0, speed: 4.0, heat: 100.0 };

/// A blast at `at` (world) on every person it reaches that nothing of the room shields (`clear`:
/// whether the line between two points is clear): hurt by how near its nearest part is, heated,
/// floored (or set reeling further out), its limbs torn off close in, thrown out, and wounded where
/// it faced the blast.
pub fn blast(people: &mut crate::arena::People, at: Vec3, b: Blast, clear: impl Fn(Vec3, Vec3) -> bool) -> Vec<Bled> {
    let mut out = Vec::new();
    for k in 0..people.dummies.people.len() {
        let d = &people.dummies;
        let Some((near, part)) = (0..crate::anim::PARTS).map(|i| (d.distance_to(k, i, at), i)).min_by(|x, y| x.0.total_cmp(&y.0)) else { continue };
        if near > b.radius || !clear(at + Vec3::Y * 0.1, d.part_pose(k, crate::anim::TORSO).0) {
            continue;
        }
        let (a, bb, r) = d.capsule(k, part);
        let t = ((at - a).dot(bb - a) / (bb - a).length_squared().max(1e-9)).clamp(0.0, 1.0);
        let on = a.lerp(bb, t);
        let away = (on - at).normalize_or(Vec3::Y);
        let point = on - away * r;
        let near_k = 1.0 - near / b.radius;
        let damage = b.damage * near_k * near_k;
        let (id, alive, now) = (d.people[k].id, d.people[k].alive, d.time);
        let d = &mut people.dummies;
        d.people[k].last_hit = (DMG_BLUNT, BodyPart::Torso);
        d.hurt(k, damage, point);
        if let Some(brain) = people.brain_of(k) {
            brain.hurt(damage / rustvr_melee::body::HEALTH, now);
        }
        let d = &mut people.dummies;
        if b.heat > 0.0 {
            d.heat(k, b.heat * near_k, point);
        }
        if near <= b.floor {
            d.floor(k, point);
        } else {
            d.react(k, part, 1, away, point, false);
        }
        // (Its outer parts first: each torn off on its own, not whole from the shoulder.)
        let mut torn = 0;
        for i in (0..crate::anim::PARTS).rev() {
            if near <= b.tear && d.distance_to(k, i, at) <= b.tear {
                let from = d.part_pose(k, i).0;
                if d.sever(k, i, (from - at).normalize_or(Vec3::Y)) {
                    torn += 1;
                }
            }
        }
        d.blast(k, at, b.radius, b.speed);
        d.wound(k, part, point, if b.heat > 0.0 { DMG_BLUNT } else { DMG_PIERCE }, near_k, true, false, b.heat > 0.0);
        if b.heat <= 0.0 {
            out.push(Bled { point, dir: away, strength: near_k });
        }
        tracing::info!(
            "range: {id:#x} in a blast {near:.1} m off: {damage:.1} of its health ({:.0} left){}{}",
            d.people[k].health.max(0.0),
            if torn > 0 { format!(", {torn} parts torn off") } else { String::new() },
            if alive && !d.people[k].alive { ", dead" } else { "" }
        );
    }
    out
}

/// A thrown knife stuck in person `k`'s part `part`, its tip at `tip` (world), come along `dir` at
/// `speed` m/s: a thrown dagger's stab (the curve from 1 at a standstill to 5 at 20 m/s, x3 thrown,
/// by the armour's row and the part's multiplier; then, gone in, its penetration damage 7.5 times
/// the part's deep multiplier, a head or a throat x32).
pub fn knife(people: &mut crate::arena::People, k: usize, part: usize, tip: Vec3, dir: Vec3, speed: f32) -> Vec<Bled> {
    let d = &mut people.dummies;
    let kind = d.part_kind(k, part);
    let material = d.part_material(k, part);
    let throat = kind == PART_HEAD && on_neck(d, k, part, tip);
    let body = BodyPart::of(kind, throat);
    let row = rustvr_melee::row(rustvr_melee::MOD_PIERCE_FINE, material, 1, false);
    let went_in = row.is_some_and(|r| speed >= r.pen_speed);
    let hit = rustvr_melee::curve(rustvr_melee::DAGGER_PIERCE_CURVE, rustvr_melee::DAGGER_PIERCE_SLOPES, speed) * KNIFE_THROWN * row.map_or(0.0, |r| r.damage);
    let damage = hit * body.damage() + if went_in { KNIFE_PENETRATION * body.deep().1 } else { 0.0 };
    let (id, alive, now) = (d.people[k].id, d.people[k].alive, d.time);
    d.people[k].last_hit = (DMG_PIERCE, body);
    d.people[k].silenced |= went_in && body == BodyPart::Neck;
    d.hurt(k, damage, tip);
    if let Some(brain) = people.brain_of(k) {
        brain.hurt(damage / rustvr_melee::body::HEALTH, now);
    }
    let d = &mut people.dummies;
    let level = row.map_or(0, |r| r.push_level(speed, true));
    d.react(k, part, level, dir, tip, false);
    d.wound(k, part, tip, DMG_PIERCE, 1.0, damage > 0.0, went_in, false);
    tracing::info!("range: {id:#x} part {part} ({body:?}) knifed at {speed:.1} m/s{}: {damage:.1} of its health ({:.0} left){}", if went_in { ", in" } else { "" }, d.people[k].health.max(0.0), if alive && !d.people[k].alive { ", dead" } else { "" });
    if damage > 0.0 { vec![Bled { point: tip, dir: -dir, strength: 0.6 }] } else { Vec::new() }
}

/// A dagger's stab thrown (its thrown multiplier), and its penetration damage.
const KNIFE_THROWN: f32 = 3.0;
const KNIFE_PENETRATION: f32 = 7.5;

/// A spell cast at a creature (the runtime's `CE_CAST` with the creature its aim found): its bolt
/// lands on what it meets on its way (else its torso). The fire bolt is an energy damager (10 at
/// any tier, armour no matter, times the part's; its push at its 12 m/s, a body that's down shoved
/// with 260 for 0.2 s; its fire's heat, `IMBUE_HIT_HEAT` 6); the shock is a lightning hit (2 at
/// push level 1, electrocuted two times in five, certain on metal, for 2 s, 4 in metal armour).
pub fn bolt(people: &mut crate::arena::People, ev: &CombatEvent) -> Vec<Bled> {
    let d = &mut people.dummies;
    let Some(k) = d.find(ev.actor) else { return Vec::new() };
    let (from, dir) = (Vec3::from_array(ev.point), Vec3::from_array(ev.dir).normalize_or(Vec3::Z));
    let (part, point) = match d.raycast(from, dir, 40.0) {
        Some((_, at, _, crate::ragdoll::Struck::Part(who, part))) if who == k => (part, at),
        _ => (crate::anim::TORSO, d.part_pose(k, crate::anim::TORSO).0),
    };
    let dir = (point - from).normalize_or(dir);
    let kind = d.part_kind(k, part);
    let body = BodyPart::of(kind, kind == PART_HEAD && on_neck(d, k, part, point));
    let now = d.time;
    let damage = match ev.what {
        FIRE_BOLT => BOLT_DAMAGE * body.damage(),
        SHOCK => SHOCK_DAMAGE,
        _ => return Vec::new(),
    };
    d.people[k].last_hit = (DMG_BLUNT, body);
    d.hurt(k, damage, point);
    if let Some(brain) = people.brain_of(k) {
        brain.hurt(damage / rustvr_melee::body::HEALTH, now);
    }
    let d = &mut people.dummies;
    if ev.what == FIRE_BOLT {
        let level = rustvr_melee::row(rustvr_melee::MOD_ENERGY, MAT_FLESH, 1, false).map_or(0, |r| r.push_level(BOLT_SPEED, false));
        d.react(k, part, level, dir, point, false);
        d.shove(k, part, point, dir * BOLT_SPEED, BOLT_FORCE.0, BOLT_FORCE.1, 1.0, 1.0, false);
        d.heat(k, IMBUE_HIT_HEAT, point);
        d.wound(k, part, point, DMG_BLUNT, 1.0, true, false, true);
    } else {
        d.react(k, part, 1, dir, point, true);
        let metal = matches!(d.part_material(k, part), MAT_CHAINMAIL | MAT_PLATE);
        if metal || d.chance() < SHOCK_CHANCE {
            d.shock(k, if metal { 4.0 } else { 2.0 });
        }
    }
    tracing::info!("range: {:#x} part {part} ({body:?}) hit by a {} bolt: {damage:.1} of its health ({:.0} left)", ev.actor, if ev.what == FIRE_BOLT { "fire" } else { "shock" }, d.people[k].health.max(0.0));
    Vec::new()
}

/// The fire bolt (10, flying at 12 m/s, shoving with 260 for 0.2 s) and the shock's damage.
const BOLT_DAMAGE: f32 = 10.0;
const BOLT_SPEED: f32 = 12.0;
const BOLT_FORCE: (f32, f32) = (260.0, 0.2);
const SHOCK_DAMAGE: f32 = 2.0;

/// A blade going in deep, or out the far side (the runtime's `CE_WOUND`): its penetration damage,
/// times the part's: a stab's deep multiplier (a head or a neck x32, the torso x2), a cut's (the
/// neck x32), a skewer's.
pub fn wound(people: &mut crate::arena::People, ev: &CombatEvent) {
    let d = &mut people.dummies;
    let Some(k) = d.find(ev.actor) else { return };
    let part = ev.part as usize;
    let kind = d.part_kind(k, part);
    let point = Vec3::from_array(ev.point);
    let throat = kind == PART_HEAD && on_neck(d, k, part, point);
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

/// One of the player's damagers met a person without doing anything (the runtime's `CE_TOUCH`):
/// on armour it leaves a scratch (an edge or a point) or a scuff (a blunt one); on flesh nothing.
pub fn touch(people: &mut crate::arena::People, ev: &CombatEvent) {
    let d = &mut people.dummies;
    let Some(k) = d.find(ev.actor) else { return };
    d.wound(k, ev.part as usize, Vec3::from_array(ev.point), ev.what, ev.strength, false, false, false);
}

/// How loud and bright a hit's effects are, 0..1 (its intensity: the damager's own speeds after a
/// blow that did damage, else the contact's).
pub fn blow_intensity(ev: &CombatEvent, weapons: &MeleeWeapons) -> f32 {
    match weapons.get(ev.object).and_then(|w| w.blades().get(ev.blade as usize)) {
        Some(b) => rustvr_melee::blow_intensity(b, ev.speed, ev.damage > 0.0),
        None => rustvr_melee::intensity(ev.speed),
    }
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

/// A blade's shove on a body that's down (`MeleeBlade::push`: its force, how long, and its
/// multipliers on the part hit and on the rest), and whether it's an impulse rather than an
/// acceleration.
fn add_force(b: &MeleeBlade) -> (f32, f32, f32, f32, bool) {
    let [force, seconds, part_x, other_x] = b.push;
    (force, seconds, part_x, other_x, b.flags & DMGF_PUSH_IMPULSE != 0)
}

/// A punch's: 100 for 0.15 s, x2 on the part hit, x0.5 on the rest, an acceleration.
const PUNCH_FORCE: (f32, f32, f32, f32, bool) = (100.0, 0.15, 2.0, 0.5, false);

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
            level = level.max(level_of(rustvr_melee::bad_angle(&b).0));
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
