//! Combat: the game's creatures, the player's melee weapons and spells, and what the player's hands
//! do with them.
//!
//! The game stays the source of truth for what a hit or a spell *does* (its damage formula,
//! skills, enchantments, sounds, blood, stagger, magicka, effects, projectiles). The runtime only
//! decides that the player's hand did it, and how well, from what the hand physically did.
//!
//! * [`Actors`] (game -> runtime, every frame): where each creature near the player is, as
//!   capsules on its skeleton (from the game's own ragdoll or hit shapes where it has them), and
//!   where its weapon is while it attacks.
//! * [`MeleeTable`] (game -> runtime, when it changes): the blade of each melee weapon the player
//!   can hold ([`crate::OBJ_MELEE`] objects): which parts of it cut, stab or bludgeon.
//! * [`Spells`] (game -> runtime, when it changes): the player's spells, the selected one, and the
//!   magicka to cast them.
//! * [`CombatEvents`] (runtime -> game, a ring): a melee hit (which creature and body part, with
//!   what, how hard, which way), a parry or block of a creature's attack, a spell cast (from which
//!   hand, which way, on whom).
//!
//! A hit is judged by the impact velocity at the contact against the damager's own frame and
//! angle limits, the body part's material, a cooldown per part. All positions here are world
//! space, metres, the protocol's axes (the adapter's `Basis`).

use bytemuck::{Pod, Zeroable};

pub const OFF_COMBAT: usize = 0x004F_0000;
pub const OFF_ACTORS: usize = OFF_COMBAT;
pub const MAX_ACTORS: usize = 24;
pub const MAX_ACTOR_PARTS: usize = 16;

// ---- body parts ---------------------------------------------------------------------------------
pub const PART_TORSO: u32 = 0;
pub const PART_HEAD: u32 = 1;
pub const PART_ARM: u32 = 2;
pub const PART_HAND: u32 = 3;
pub const PART_LEG: u32 = 4;
pub const PART_FOOT: u32 = 5;
/// A shield or a held weapon: hitting it is a block, not a wound.
pub const PART_SHIELD: u32 = 6;
pub const PART_TAIL: u32 = 7;

// ---- materials (what a part is covered with: it decides the speed a hit needs) -------------------
pub const MAT_FLESH: u32 = 0;
pub const MAT_LEATHER: u32 = 1;
pub const MAT_CHAINMAIL: u32 = 2;
pub const MAT_PLATE: u32 = 3;
pub const MAT_BONE: u32 = 4;
pub const MAT_WOOD: u32 = 5;
pub const MAT_STONE: u32 = 6;
pub const MAT_ETHEREAL: u32 = 7;
/// The ground: dirt, grass, snow.
pub const MAT_EARTH: u32 = 8;

/// A blade bites into it and sticks (wood, earth, flesh and what's worn over it but metal), not
/// glancing off (stone, metal, bone).
pub fn biteable(material: u32) -> bool {
    matches!(material, MAT_FLESH | MAT_LEATHER | MAT_WOOD | MAT_EARTH)
}

/// One capsule of a creature's body: from `a` (the end it hangs from: a limb's joint) to `b`,
/// `radius` thick.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct ActorPart {
    pub a: [f32; 3],
    pub radius: f32,
    pub b: [f32; 3],
    pub kind: u32, // PART_*
    pub material: u32, // MAT_*
    /// The game's own index for the part (its bone, its body part), passed back in hits.
    pub game_part: u32,
    pub _pad: [u32; 2],
}

/// Alive (hits on a dead body still count, as a corpse).
pub const ACT_ALIVE: u32 = 1 << 0;
pub const ACT_HOSTILE: u32 = 1 << 1;
/// Swinging or thrusting at someone: `weapon_a`..`weapon_b` is its blade, and the player's weapon
/// or shield meeting it is a block.
pub const ACT_ATTACKING: u32 = 1 << 2;
/// Holding its guard up.
pub const ACT_BLOCKING: u32 = 1 << 3;
/// Lying down, knocked over or ragdolled.
pub const ACT_DOWN: u32 = 1 << 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Actor {
    /// The game's id for it (passed back in hits).
    pub id: u32,
    pub flags: u32, // ACT_*
    pub count: u32,
    /// Its health as a share of its most (0..1; for the runtime's feedback).
    pub health: f32,
    /// Its weapon's blade while it attacks (`ACT_ATTACKING`), and how thick.
    pub weapon_a: [f32; 3],
    pub weapon_radius: f32,
    pub weapon_b: [f32; 3],
    pub _p0: f32,
    /// How fast it moves (metres per second): a hit's speed is the blade's relative to the body.
    pub velocity: [f32; 3],
    pub _p1: f32,
    pub parts: [ActorPart; MAX_ACTOR_PARTS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Actors {
    /// Seqlock word.
    pub seq: u32,
    pub count: u32,
    /// The game frame they're from.
    pub frame: u64,
    pub _pad: [u32; 12],
    pub actors: [Actor; MAX_ACTORS],
}

// ---- melee weapons ------------------------------------------------------------------------------
/// The older, simpler melee table ([`MeleeTable`]). The runtime no longer judges by it: a melee
/// weapon is described by its own numbers in [`crate::MeleeWeapons`] (the damage kinds and
/// flags below are still theirs). Its region stays where it is so the layout doesn't move.
pub const OFF_MELEE: usize = OFF_ACTORS + size_of::<Actors>();
pub const MAX_MELEE: usize = 32;
pub const MAX_DAMAGERS: usize = 4;

/// Cuts along `edge` (its frame: `edge` is the direction it cuts in).
pub const DMG_SLASH: u32 = 1;
/// Stabs along the blade, at the tip.
pub const DMG_PIERCE: u32 = 2;
/// Bludgeons (any direction).
pub const DMG_BLUNT: u32 = 3;
/// A fist (the hand itself).
pub const DMG_PUNCH: u32 = 4;

/// Cuts both ways along `edge` (a double-edged blade).
pub const DMGF_BOTH_WAYS: u32 = 1 << 0;
/// Only guards: meets blows (blocks, parries) and never hits (an axe's haft, a sword's grip).
pub const DMGF_GUARD: u32 = 1 << 1;
/// An axe's edge: an axe's limits (the cut within 45 degrees of the edge, into the surface within
/// 80, at 3 m/s whatever the armour) instead of a sword's (35, 60, by armour).
pub const DMGF_AXE: u32 = 1 << 2;

/// One part of a weapon that hurts one way (a sword: its edges cut, its point stabs, its flat and
/// hilt bludgeon): the segment `a`..`b` in the weapon's model space, `radius` thick.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct Damager {
    pub kind: u32, // DMG_*
    pub flags: u32, // DMGF_*
    /// How deep a stab goes in (metres; pierce only).
    pub depth: f32,
    pub radius: f32,
    pub a: [f32; 3],
    pub _p0: f32,
    pub b: [f32; 3],
    pub _p1: f32,
    /// The direction it cuts in (slash), model space.
    pub edge: [f32; 3],
    pub _p2: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct MeleeDesc {
    /// The object's [`crate::ObjectDesc::id`].
    pub id: u32,
    pub count: u32,
    /// The game's damage for a full-strength hit (for the runtime's feedback only).
    pub damage: f32,
    /// How far it reaches from the grip, metres.
    pub reach: f32,
    pub damagers: [Damager; MAX_DAMAGERS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct MeleeTable {
    pub seq: u32,
    pub count: u32,
    pub _pad: [u32; 14],
    pub weapons: [MeleeDesc; MAX_MELEE],
}

// ---- spells -------------------------------------------------------------------------------------
pub const OFF_SPELLS: usize = OFF_MELEE + size_of::<MeleeTable>();
pub const MAX_SPELLS: usize = 96;

/// On the caster.
pub const DELIVER_SELF: u32 = 0;
/// On what the casting hand touches.
pub const DELIVER_TOUCH: u32 = 1;
/// Thrown at a target (a bolt or projectile).
pub const DELIVER_TARGET: u32 = 2;

/// The spell the game has ready (its "selected" or "equipped" one).
pub const SPELL_SELECTED: u32 = 1 << 0;
/// Enough magicka (and nothing else stops it) to cast it now.
pub const SPELL_CASTABLE: u32 = 1 << 1;
/// A power (once a day) or a lesser power: no magicka.
pub const SPELL_POWER: u32 = 1 << 2;
/// The player has it on a quick slot (a favourite).
pub const SPELL_FAVOURITE: u32 = 1 << 3;
/// Fire, or shock: a hand charged with it held to a blade fills the blade with it (imbuing,
/// [`crate::RtMelee::imbue`]), and the blade's hits carry it (the fire rows of the damage tables;
/// burning, a shock's stun, for the game).
pub const SPELL_FIRE: u32 = 1 << 4;
pub const SPELL_SHOCK: u32 = 1 << 5;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Spell {
    /// The game's id for it.
    pub id: u32,
    pub delivery: u32, // DELIVER_*
    pub flags: u32,    // SPELL_*
    /// The game's school or kind of magic (the adapter's own numbering; for its look).
    pub school: u32,
    pub cost: f32,
    /// The colour its magic glows in the hand (linear RGB), and how strongly.
    pub color: [f32; 3],
    /// UTF-8, NUL-terminated.
    pub name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Spells {
    pub seq: u32,
    pub count: u32,
    pub magicka: f32,
    pub magicka_max: f32,
    /// How long the game takes to cast (seconds): the hand charges this long before the spell is
    /// ready (the game's cast animation's time to its release).
    pub cast_time: f32,
    pub flags: u32, // SPELLS_*
    pub _pad: [u32; 10],
    pub spells: [Spell; MAX_SPELLS],
}

/// The game draws its own casting art where the runtime's casting is ([`crate::Casters`]): the
/// runtime draws none of its own (no glow in the palm).
pub const SPELLS_GAME_ART: u32 = 1 << 0;

// ---- combat events ------------------------------------------------------------------------------
pub const OFF_COMBAT_EVENTS: usize = OFF_SPELLS + size_of::<Spells>();
pub const COMBAT_EVENT_SLOTS: usize = 128;

/// A melee hit: `actor` and its `part` (the game's index), struck by `object` (0: a fist) with a
/// damager of `kind`; `strength` 0..1+ (1: a full-strength hit), `speed` the impact speed (m/s);
/// `point` where, `dir` which way the blade was moving.
pub const CE_HIT: u32 = 1;
/// The player's weapon or shield (`object`; 0 a hand) met `actor`'s attacking weapon: a block, or
/// with `CEF_PARRY` a parry (the player's weapon moving into it).
pub const CE_BLOCK: u32 = 2;
/// A spell cast: `spell` from `hand` (0 left, 1 right), from `point` along `dir`; `actor` the one a
/// touch spell touched (or the target aimed at, 0: none); `strength` how charged (0..1). `object`:
/// the held thing it was cast from ([`crate::OBJF_CASTS`], a staff: `what` is then what it casts,
/// [`crate::ObjectDesc::casts`]), else 0.
pub const CE_CAST: u32 = 3;
/// An arrow loosed from the bow `object` (`hand`: the string hand): `what` the arrow (its template's
/// id), from `point` (its tip) along `dir`; `strength` the draw (0..1 of the bow's full draw);
/// `speed` how fast the bow's table says the game's arrow flies at that draw. The game fires its own
/// arrow.
pub const CE_LOOSE: u32 = 4;
/// The player was struck by a creature's weapon ([`crate::ActorItems`]): `actor` swung `object`,
/// whose damager of kind `what` met the player's body part `part` (`PART_*`) at `point`, moving
/// `dir` at `speed`; `strength` and `damage` as a hit's. The game hurts the player.
pub const CE_STRUCK: u32 = 5;
/// A blade the player holds that went in comes back out of `actor`'s part `part` (`object` the
/// weapon; `depth` how deep it had gone).
pub const CE_PULLED_OUT: u32 = 6;
/// A blade in `actor` cuts on as it's dragged (`DMGF_SLIDES`), or is pushed deeper: more damage
/// (`damage`), `depth` how deep it is now.
pub const CE_WOUND: u32 = 7;
/// Thing `object` broke ([`crate::OBJF_BREAKABLE`]), struck by `what` (one of the runtime's things,
/// or 0: a hand, a creature or the world) at `speed` along `dir`, `strength` the knock's weighed
/// speed, at `point`: the runtime has taken it out of its world until the game puts it back (a new
/// `spawn_seq`), and the game makes its pieces.
pub const CE_BROKE: u32 = 8;
/// One of the player's damagers (`object`, `blade`; 0 a hand) met `actor`'s part `part` (the
/// game's) of `material` without doing anything: too slow for what it met, or at a bad angle with
/// no blunt blow to fall back on. It's heard, and on armour
/// it scratches or scuffs (`what` the damager's kind, `speed`, `point`, `dir` as a hit's).
pub const CE_TOUCH: u32 = 9;

/// A hit with an edge or point at a bad angle: it's a glancing blow (the flat), weak and blunt.
pub const CEF_GLANCING: u32 = 1 << 0;
/// A stab that went in.
pub const CEF_PENETRATED: u32 = 1 << 1;
/// A thrown weapon's hit.
pub const CEF_THROWN: u32 = 1 << 2;
/// Hard enough to stagger (the game's own "power attack").
pub const CEF_POWER: u32 = 1 << 3;
pub const CEF_PARRY: u32 = 1 << 4;
/// From behind the creature.
pub const CEF_BACK: u32 = 1 << 5;
/// A blow that went in fast enough, and square enough at a joint, to cut through it (a limb's
/// dismemberment): the event's `part` is the one it takes off (the part hit, or the one
/// beside it whose joint it crossed: a forearm cut at the wrist takes the hand), and the game cuts
/// it off, and what hangs from it, at its joint. The cut kills. The blade doesn't stay in: it
/// carries on through.
pub const CEF_SEVER: u32 = 1 << 6;
/// A stab made by pushing slowly (a pressure stab).
pub const CEF_PRESSURE: u32 = 1 << 7;
/// It went on through into another creature behind (a skewer).
pub const CEF_SKEWER: u32 = 1 << 8;
/// From a weapon filled with a spell ([`crate::RtMelee::imbue`]): `spell` is which.
pub const CEF_IMBUED: u32 = 1 << 9;
/// Two hands on the weapon.
pub const CEF_TWO_HANDED: u32 = 1 << 10;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct CombatEvent {
    pub kind: u32, // CE_*
    pub flags: u32, // CEF_*
    pub actor: u32,
    pub part: u32,
    pub object: u32,
    /// Hit: `DMG_*`. Cast: the spell's id.
    pub what: u32,
    pub hand: u32,
    pub material: u32,
    pub strength: f32,
    pub speed: f32,
    /// How deep the blade went in (metres): a penetration's.
    pub depth: f32,
    /// The hit's damage by the melee rules (the damager's curve and tier, the material's
    /// multiplier), before the creature's own: a game may take it as it is, or use its own.
    pub damage: f32,
    pub point: [f32; 3],
    /// Which of the weapon's damagers it was (its index).
    pub blade: u32,
    pub dir: [f32; 3],
    /// The spell an imbued weapon carries ([`CEF_IMBUED`]), else 0.
    pub spell: u32,
}

/// Runtime -> game: `written` events so far; event `n` is in slot `n % COMBAT_EVENT_SLOTS`. The
/// game keeps how many it has read (and skips ahead if it fell more than a ring behind).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CombatEvents {
    pub written: u64,
    pub _pad: [u32; 14],
    pub events: [CombatEvent; COMBAT_EVENT_SLOTS],
}

pub const OFF_COMBAT_END: usize = OFF_COMBAT_EVENTS + size_of::<CombatEvents>();

const _: () = {
    assert!(size_of::<ActorPart>() == 48);
    assert!(size_of::<Actor>() == 64 + 48 * MAX_ACTOR_PARTS);
    assert!(size_of::<Damager>() == 64);
    assert!(size_of::<MeleeDesc>() == 16 + 64 * MAX_DAMAGERS);
    assert!(size_of::<Spell>() == 64);
    assert!(size_of::<CombatEvent>() == 80);
    assert!(OFF_COMBAT >= crate::OFF_GAME_HULLS + size_of::<crate::GameHulls>());
    assert!(OFF_COMBAT_END <= crate::OFF_HUD_LAYOUT);
};

impl Spell {
    pub fn name(&self) -> &str {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(self.name.len());
        std::str::from_utf8(&self.name[..n]).unwrap_or("")
    }

    pub fn set_name(&mut self, s: &str) {
        self.name = [0; 32];
        let mut n = s.len().min(31);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.name[..n].copy_from_slice(&s.as_bytes()[..n]);
    }
}
