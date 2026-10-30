//! Gore: the game's blood and the limbs its creatures lose.
//!
//! The game stays the source of truth for what blood looks like (its own decal, splatter, stain,
//! drip and droplet textures, per creature) and for whether a blow kills. The runtime makes the
//! blood the player's blows and the stumps spill: a budgeted pool of drops flying with gravity,
//! landing as decals and pools, running down slopes and walls, dripping from blades, and splashing
//! the player's hands, weapon and body (`rustvr-runtime`'s `gore`, drawn by `rustvr-avatar`'s).
//!
//! * [`BloodTable`] (game -> runtime, when it changes): the kinds of blood the game has, each
//!   with its textures (named as the runtime's asset cache names them).
//! * [`GoreBodies`] (game -> runtime, every frame): each creature near the player (by its
//!   [`crate::Actor::id`]) with its blood, and the stumps where it lost limbs.
//! * [`GoreEvents`] (game -> runtime, a ring): a limb cut off, with the piece it became (one of the
//!   adapter's props, [`crate::ObjectDesc`]: the runtime simulates it, and the game draws it there).
//!
//! When an edge cuts a limb off is the same for every game ([`severs`]): an adapter asks it with
//! its game's own numbers (the weapon's damage and kind, the hit's strength, whether the blow
//! killed) once the game has landed the hit. All positions are world space, metres, the protocol's
//! axes (the adapter's `Basis`).

use bytemuck::{Pod, Zeroable};

pub const OFF_GORE: usize = 0x0050_0000;
pub const MAX_BLOODS: usize = 16;
pub const MAX_GORE_BODIES: usize = crate::MAX_ACTORS;
pub const MAX_STUMPS: usize = 8;
pub const GORE_EVENT_SLOTS: usize = 32;

// ---- blood ----------------------------------------------------------------------------------------
/// A liquid: it sprays, runs, drips and pools (red blood, a lich's, a daedra's).
pub const BLOOD_LIQUID: u32 = 1 << 0;
/// A dust or ash (a skeleton's bone dust): it puffs and settles, and never runs or drips.
pub const BLOOD_DUST: u32 = 1 << 1;
/// Thin and glowing (a will-o'-the-wisp's, a ghost's): drawn by its own light, unlit.
pub const BLOOD_GLOW: u32 = 1 << 2;

/// One kind of blood, from the game's art. A texture is named `<game>/<asset key>` (the asset
/// cache's `textures` under the game's folder); empty: the kind has none of that (the splat's
/// stands in for a missing decal, a plain drop of its colour for a missing particle).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Blood {
    pub flags: u32, // BLOOD_*
    /// How much a body holds, against a person's (1): a rat's less, a troll's more.
    pub amount: f32,
    /// How thick it is (1: blood): thicker runs slower.
    pub viscosity: f32,
    /// The splat texture is an atlas of `atlas` x `atlas` splats (a drop takes one).
    pub atlas: u32,
    /// A drop landing: the game's blood decal (its alpha the shape).
    pub splat: [u8; 48],
    /// A fast drop landing at a slant, a heavy spray: a splatter with its streaks along the way.
    pub splatter: [u8; 48],
    /// A pool.
    pub stain: [u8; 48],
    /// What runs down a wall leaves (its red the streak).
    pub drip: [u8; 48],
    /// A drop in the air: the game's blood particle (a drop drawn out along its fall).
    pub drop: [u8; 48],
    /// Fine mist in the air.
    pub mist: [u8; 48],
    /// A stump's cut face: the game's flesh.
    pub flesh: [u8; 48],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BloodTable {
    pub seq: u32,
    pub count: u32,
    pub _pad: [u32; 14],
    pub bloods: [Blood; MAX_BLOODS],
}

// ---- limbs ----------------------------------------------------------------------------------------
pub const LIMB_HEAD: u32 = 0;
pub const LIMB_UPPER_ARM: u32 = 1;
pub const LIMB_FOREARM: u32 = 2;
pub const LIMB_HAND: u32 = 3;
pub const LIMB_THIGH: u32 = 4;
pub const LIMB_CALF: u32 = 5;
pub const LIMB_FOOT: u32 = 6;
/// A tail, a wing, a creature's own limb: cut like a forearm.
pub const LIMB_OTHER: u32 = 7;
/// On the body's left side (with any of the above but the head).
pub const LIMB_LEFT: u32 = 1 << 8;

/// How hard a limb is to cut through (the cut's power it takes, [`cut_power`]): a neck less than a
/// thigh, a wrist less than a shoulder.
pub fn toughness(limb: u32) -> f32 {
    match limb & 0xFF {
        LIMB_HEAD => 0.55,
        LIMB_UPPER_ARM => 0.9,
        LIMB_FOREARM => 0.65,
        LIMB_HAND => 0.45,
        LIMB_THIGH => 1.3,
        LIMB_CALF => 0.85,
        LIMB_FOOT => 0.55,
        _ => 0.65,
    }
}

/// What a weapon of `damage` (the game's own number for a full blow) cuts with, against a
/// reference blade of [`REFERENCE_DAMAGE`]: `kind` 1 for a one-handed blade, more for a heavy one
/// (a greatsword, an axe's edge), less for a short one (a dagger). An adapter gives its game's
/// numbers.
pub fn weapon_cut(damage: f32, kind: f32) -> f32 {
    (damage / REFERENCE_DAMAGE).max(0.0) * kind.max(0.0)
}
/// The damage the reference blade does (an iron longsword's, in Oblivion's numbers).
pub const REFERENCE_DAMAGE: f32 = 10.0;

/// The cut's power: the hit's strength (`CombatEvent::strength`, 1 a full-strength hit) times what
/// the weapon cuts with ([`weapon_cut`]).
pub fn cut_power(strength: f32, cut: f32) -> f32 {
    strength.max(0.0) * cut
}

/// Whether a hit cuts the limb off. Only an edge cuts (a slash or an axe's chop, clean: never a
/// glancing blow, a stab or a blunt one), and only through a creature that dies of it or is
/// already dead (as Fallout's dismemberment: on the killing blow or a corpse). Then the cut's
/// power must reach the limb's toughness; a corpse gives way a little easier.
pub fn severs(limb: u32, damage_kind: u32, hit_flags: u32, killed: bool, dead_before: bool, power: f32) -> bool {
    if damage_kind != crate::DMG_SLASH || hit_flags & crate::CEF_GLANCING != 0 {
        return false;
    }
    if !(killed || dead_before) {
        return false;
    }
    let need = toughness(limb) * if dead_before { 0.8 } else { 1.0 };
    power >= need
}

// ---- bodies ---------------------------------------------------------------------------------------
/// Where a creature lost a limb: on its body, at the joint.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct Stump {
    /// The joint, and the limb's radius there.
    pub at: [f32; 3],
    pub radius: f32,
    /// Out of the stump (unit): the way the limb went.
    pub out: [f32; 3],
    pub limb: u32, // LIMB_*
}

/// It lives (its stumps pump; a dead body's only drain).
pub const GB_ALIVE: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GoreBody {
    /// The creature ([`crate::Actor::id`]).
    pub id: u32,
    /// Its blood: an index into the [`BloodTable`] (`u32::MAX`: none, a creature that doesn't
    /// bleed).
    pub blood: u32,
    pub flags: u32, // GB_*
    pub count: u32,
    pub stumps: [Stump; MAX_STUMPS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GoreBodies {
    pub seq: u32,
    pub count: u32,
    pub frame: u64,
    pub _pad: [u32; 12],
    pub bodies: [GoreBody; MAX_GORE_BODIES],
}

// ---- events ---------------------------------------------------------------------------------------
/// A limb cut off ([`GoreEvent`]): `actor` lost `limb` at `at` (a stump now, in its
/// [`GoreBodies`]), and it became the prop `object` (0: nothing to show).
pub const GE_SEVER: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct GoreEvent {
    pub kind: u32, // GE_*
    pub actor: u32,
    pub limb: u32,
    pub object: u32,
    /// The joint cut through (world) and the limb's radius there.
    pub at: [f32; 3],
    pub radius: f32,
    /// Out of the body's stump, towards the piece (unit, world).
    pub out: [f32; 3],
    /// The blood (an index into the [`BloodTable`]).
    pub blood: u32,
    /// The piece's cut face in the piece's own (object) space: its centre, and out of it (unit).
    pub cap: [f32; 3],
    /// How hard the cut was (the hit's strength).
    pub strength: f32,
    pub cap_out: [f32; 3],
    pub _p0: f32,
    /// How the piece was moving when it was cut (the body part's velocity, world), and the
    /// blade's way through it (unit; its speed in `push_speed`).
    pub vel: [f32; 3],
    pub push_speed: f32,
    pub push: [f32; 3],
    pub _p1: f32,
}

/// Game -> runtime: `written` events so far; event `n` is in slot `n % GORE_EVENT_SLOTS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GoreEvents {
    pub written: u64,
    pub _pad: [u32; 14],
    pub events: [GoreEvent; GORE_EVENT_SLOTS],
}

pub const OFF_BLOODS: usize = OFF_GORE;
pub const OFF_GORE_BODIES: usize = (OFF_BLOODS + size_of::<BloodTable>()).next_multiple_of(64);
pub const OFF_GORE_EVENTS: usize = (OFF_GORE_BODIES + size_of::<GoreBodies>()).next_multiple_of(64);
pub const OFF_GORE_END: usize = OFF_GORE_EVENTS + size_of::<GoreEvents>();

const _: () = {
    assert!(size_of::<Blood>() == 16 + 48 * 7);
    assert!(size_of::<Stump>() == 32);
    assert!(size_of::<GoreBody>() == 16 + 32 * MAX_STUMPS);
    assert!(size_of::<GoreEvent>() == 112);
    assert!(OFF_GORE >= crate::OFF_ITEM_INFO + size_of::<crate::ItemInfos>());
    assert!(OFF_GORE_END <= crate::OFF_HUD_LAYOUT);
};

impl Blood {
    /// A texture's name as UTF-8 (empty: none).
    pub fn texture(name: &[u8; 48]) -> &str {
        let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        std::str::from_utf8(&name[..n]).unwrap_or("")
    }

    pub fn set_texture(name: &mut [u8; 48], s: &str) {
        *name = [0; 48];
        let mut n = s.len().min(47);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        name[..n].copy_from_slice(&s.as_bytes()[..n]);
    }
}
