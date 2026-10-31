//! Melee beyond [`crate::combat`]'s blades (`docs/MELEE.md`): how each weapon is held, its
//! damagers and their numbers, the creatures' weapons and shields as physical
//! things the player's weapons meet, and what the player's body does besides hitting (slowing
//! time, climbing, imbuing, tiring).
//!
//! The game stays the source of truth for what a hit does (its damage, armour, stagger, death). The
//! runtime judges what the player's body and what it holds physically did, with the game's numbers
//! where it gives them and its own where it doesn't.
//!
//! * [`MeleeWeapons`] (game -> runtime, when it changes): each melee weapon's handles (where and
//!   how hands hold it, which they slide along) and its damagers. A weapon here is judged by these
//!   rather than by its [`crate::MeleeDesc`].
//! * [`ActorItems`] (game -> runtime, every frame): what the creatures hold. The game registers
//!   its creatures' weapons and shields as its own objects ([`crate::OBJF_GAME_SIMULATED`], posed
//!   in [`crate::GamePoses`]); while a creature holds one it's listed here, and the player's hands
//!   and weapons meet it as something solid the creature moves: they block it, parry it and push
//!   it ([`crate::GamePush`] on the object), and a hand takes it as any of the game's things
//!   (its holder dragged after it: the game lets go when the pull wins). Its blades, while it
//!   swings, are judged against the player's body.
//! * [`MeleeRules`] (game -> runtime, when it changes): the game's own numbers for the player: how
//!   strong they are, their stamina (when the game keeps one), whether they may slow time, imbue,
//!   climb.
//! * [`RtMelee`] (runtime -> game, every frame): the player's side: how fast the game's time should
//!   run, how the arms are climbing, the stamina swings spent, each hand's imbuing.
//!
//! All positions are world space, metres, the protocol's axes, except where they're a thing's own
//! (object space).

use bytemuck::{Pod, Zeroable};

/// After the face the hands shape, the last of the chained regions.
pub const OFF_MELEE_WEAPONS: usize = (crate::OFF_FACE_SCULPT + size_of::<crate::FaceSculpt>()).next_multiple_of(64);
pub const MAX_MELEE_WEAPONS: usize = 32;
pub const MAX_HANDLES: usize = 4;
pub const MAX_BLADES: usize = 6;

// ---- handles ------------------------------------------------------------------------------------
/// The weapon's own grip: where the first hand goes (a sword's handle, an axe's haft near its end).
pub const HANDLE_MAIN: u32 = 1 << 0;
/// A loosened grip slides along it (a spear's shaft, a staff, a long haft).
pub const HANDLE_SLIDE: u32 = 1 << 1;
/// Held either way round (the fist's axis reversed along it: a reverse grip).
pub const HANDLE_REVERSIBLE: u32 = 1 << 2;
/// On the blade itself (half-swording): its edges never cut the hand that holds it there.
pub const HANDLE_BLADE: u32 = 1 << 3;

/// What a handle is: how strongly a hand holds and turns the weapon by it ([`MeleeHandle::kind`]).
/// A one-handed weapon's grip (its turn driven about the handle and across it apart, each to its
/// cap).
pub const GRIP_LIGHT: u32 = 0;
/// A long or two-handed weapon's grip (one cap on the whole turn).
pub const GRIP_HEAVY: u32 = 1;
/// A very heavy one's (x0.6 on everything).
pub const GRIP_SUPER_HEAVY: u32 = 2;
/// A second hand's place (a pommel, a ricasso): it never takes the turning when both hands hold it.
pub const GRIP_SUPPORTING: u32 = 3;
/// The blade, held to half-sword (turned less, damped more).
pub const GRIP_HALF_SWORD: u32 = 4;
/// A shield's grip.
pub const GRIP_SHIELD: u32 = 5;
/// A throwing knife's.
pub const GRIP_THROWING_KNIFE: u32 = 6;

/// Where a hand holds a weapon: the line `a`..`b` (object space) the fist closes round, `radius`
/// thick, and how it fits the hand.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct MeleeHandle {
    pub a: [f32; 3],
    pub radius: f32,
    pub b: [f32; 3],
    pub flags: u32, // HANDLE_*
    /// The fist's turn on it, the right hand's (the controller grip pose's rotation, object space):
    /// a hand that takes the weapon by this handle goes onto it turned so, or the other way round
    /// it ([`HANDLE_REVERSIBLE`]), and the weapon's pulled into the hand that way. The left hand
    /// holds it the same (a shield's turned half round the fist's axis). Zero: the hand's own turn
    /// about the line.
    pub rot: [f32; 4],
    /// How much of the hand's hold a grip here gives (its force's multiplier): a pommel held at its
    /// very end turns the blade less well. 0: 1.
    pub strength: f32,
    /// How hard a loosened hand must pull to slide along it (N; 0: the runtime's own).
    pub slide_force: f32,
    /// What kind of handle it is (`GRIP_*`).
    pub kind: u32,
    /// Where a hand sliding off its ends goes on to: the handle past its `b` (its index + 1) in the
    /// low byte, past its `a` in the next; 0: none, the hand stops at the end. A sword's grip goes
    /// on over the guard onto its blade.
    pub links: u32,
}

// ---- blades -------------------------------------------------------------------------------------
/// A pierce damager that also stabs by being pushed in slowly (a pressure stab).
pub const DMGF_PRESSURE: u32 = 1 << 3;
/// A slash that, gone in, slides along its edge: dragged, it cuts on.
pub const DMGF_SLIDES: u32 = 1 << 4;
/// Goes right through at its depth rather than stopping there: an arrow, a thin spear point.
pub const DMGF_THROUGH: u32 = 1 << 5;
/// At a bad angle it's still a hit, a blunt one: without it, a bad angle does nothing.
pub const DMGF_BLUNT_BAD: u32 = 1 << 6;
/// Its shove on a body that isn't standing ([`MeleeBlade::push`]) is an impulse each frame, not an
/// acceleration.
pub const DMGF_PUSH_IMPULSE: u32 = 1 << 7;

/// One damager and its numbers: the segment `a`..`b` (object space), `radius` thick, cutting along
/// `edge` (slash) or stabbing towards `b` (pierce).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct MeleeBlade {
    pub kind: u32,  // DMG_*
    pub flags: u32, // DMGF_*
    /// How deep it goes in (metres; 0: it doesn't), and along a slash how far its cut runs.
    pub depth: f32,
    pub radius: f32,
    pub a: [f32; 3],
    /// Its damage multiplier (the weapon's tier: 0.5 a training sword, 1.1 a good one).
    pub tier: f32,
    pub b: [f32; 3],
    /// The speed (m/s) a cut through a limb needs, before `tier` (0: it never cuts one off).
    pub dismember: f32,
    pub edge: [f32; 3],
    /// Thrown, its damage is multiplied by this (0: 1).
    pub thrown: f32,
    /// Its angle limits (degrees; 0: its kind's own): the impact velocity across the blade against
    /// its edge, along it against the blade's plane, and against the surface's normal.
    pub horizontal: f32,
    pub vertical: f32,
    pub normal: f32,
    /// What a penetration itself does.
    pub penetration_damage: f32,
    /// Its damage by impact speed: (m/s, multiplier) at two points, flat past them, with the
    /// curve's slopes there (multiplier per m/s, leaving the first and coming into the second; both
    /// 0: it eases in and out). All zero: its kind's own.
    pub curve: [f32; 4],
    pub slopes: [f32; 2],
    /// What its point coming out the far side does (0: nothing), and how far a slash's cut runs
    /// along its edge before it comes off the end (metres; 0: its length).
    pub skewer: f32,
    pub slide_length: f32,
    /// The damping a penetration slides against (N s/m): with a hand on it going in, coming out,
    /// and with none. 0: its kind's own.
    pub damper_in: f32,
    pub damper_out: f32,
    pub damper_free: f32,
    /// Which damage table it's judged by against what it meets (`rustvr_melee`'s `MOD_*`: a sword's
    /// slash, a dagger's weaker one, an axe's; a stab, a fine point's; a mace's
    /// blunt, a pommel's lighter one...). 0: its kind's own.
    pub modifier: u32,
    /// Cutting a limb off: how far off square the limb's way may be to its cutting way and to its
    /// length (degrees; zeros: a sword's 35, 90), and `dismember`'s multiplier at each of the
    /// weapon's tiers 0..5 (zeros: a one-handed blade's 2.5, 1.5, 1, 0.9, 0.8, 0.8).
    pub dismember_angles: [f32; 2],
    pub dismember_tiers: [f32; 6],
    /// Stuck in something, it rocks on where it went in: up to `rock[1]` degrees while it's in
    /// less than `rock[0]` metres, stiffening to rigid there (a sword's 0.1 m and 30, a dagger's
    /// point 0.03 and 30, an axe's 0.12 and 30). An angle of 0 with a depth: rigid at once. Zeros:
    /// its kind's own.
    pub rock: [f32; 2],
    /// At a bad angle (with [`DMGF_BLUNT_BAD`]): the table its blunt blow is judged by and its
    /// damage (a sword's light blunt 0.5, a dagger's very light 0.5, an axe's blunt 2). 0: a light
    /// blunt's, 0.5.
    pub bad_modifier: u32,
    pub bad_damage: f32,
    /// How it shoves a body that isn't standing (down, or dead): `push[0]` (per m/s of the blow: an
    /// acceleration, or with [`DMGF_PUSH_IMPULSE`] an impulse) for `push[1]` seconds, times
    /// `push[2]` on the part hit and `push[3]` on the rest of its region: a mace's 105 for 0.15 s,
    /// x2 and x0.5; a shield's 400. Zeros: none.
    pub push: [f32; 4],
    /// After a blow that did damage, its effects are as loud and bright as its speed between these
    /// (m/s: a mace's 6 to 16, a pommel's 4 to 16).
    /// Zeros: the contact's own, 0.25 to 15.
    pub intensity: [f32; 2],
    pub _pad: [u32; 2],
}

// ---- weapons ------------------------------------------------------------------------------------
/// A shield: it blocks with its face, and its rim bludgeons.
pub const MW_SHIELD: u32 = 1 << 0;
/// Thrown point first, it flies point first (a dagger, a spear): let go
/// with its point within [`MeleeWeapon::fly_angle`] of the way it goes, it's turned along its
/// flight each step until it first touches something. Without it, it tumbles as it was let go (a
/// sword, an axe, a mace).
pub const MW_FLIES: u32 = 1 << 1;
/// It breaks when something meets it hard enough, as a breakable thing does
/// ([`crate::OBJF_BREAKABLE`]), at its own [`MeleeWeapon::break_momentum`].
pub const MW_BREAKABLE: u32 = 1 << 2;
/// Held, it's a staff that slides through the hands (both hands may slide).
pub const MW_STAFF: u32 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct MeleeWeapon {
    /// The object's [`crate::ObjectDesc::id`] (or a template's: what's spawned from it shares it).
    pub id: u32,
    pub flags: u32, // MW_*
    pub handles: u32,
    pub blades: u32,
    /// How hard a knock must be to break it ([`MW_BREAKABLE`]): the squared weighed speed of
    /// [`crate::OBJF_BREAKABLE`] (0: 20).
    pub break_momentum: f32,
    /// Its balance (object space): the centre of mass, which a grip turns it about.
    pub balance: [f32; 3],
    /// Its tier (0..5: 2 a common one), which picks its rows of the damage tables and
    /// how fast it must cut to sever.
    pub tier: u32,
    /// Flying point first ([`MW_FLIES`]): the most its point may be off the way it's thrown
    /// (degrees: a dagger 145, a spear 60), and how fast it's turned along its flight (that times
    /// its speed times the step, of what's left; 3).
    pub fly_angle: f32,
    pub fly_rate: f32,
    /// A throw's speed times this (1 on every weapon; 0: 1).
    pub throw_mult: f32,
    /// Its air drag, linear and angular (a sword's 1 and 0.05, a
    /// dagger's, a spear's and a shield's 0.4; zeros: 1 and 0.05).
    pub drag: [f32; 2],
    pub _pad: [u32; 2],
    pub handle: [MeleeHandle; MAX_HANDLES],
    pub blade: [MeleeBlade; MAX_BLADES],
}

impl MeleeWeapon {
    pub fn handles(&self) -> &[MeleeHandle] {
        &self.handle[..(self.handles as usize).min(MAX_HANDLES)]
    }

    pub fn blades(&self) -> &[MeleeBlade] {
        &self.blade[..(self.blades as usize).min(MAX_BLADES)]
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct MeleeWeapons {
    /// Seqlock word.
    pub seq: u32,
    pub count: u32,
    pub _pad: [u32; 14],
    pub weapons: [MeleeWeapon; MAX_MELEE_WEAPONS],
}

impl MeleeWeapons {
    pub fn get(&self, id: u32) -> Option<&MeleeWeapon> {
        self.weapons[..(self.count as usize).min(MAX_MELEE_WEAPONS)].iter().find(|w| w.id == id)
    }
}

// ---- the creatures' weapons -----------------------------------------------------------------------
pub const OFF_ACTOR_ITEMS: usize = (OFF_MELEE_WEAPONS + size_of::<MeleeWeapons>()).next_multiple_of(64);
pub const MAX_ACTOR_ITEMS: usize = 32;

/// Swinging or thrusting at the player now: its blades hurt what they meet of the player's body.
pub const AI_ATTACKING: u32 = 1 << 0;
/// Its holder is guarding with it.
pub const AI_BLOCKING: u32 = 1 << 1;
/// A shield.
pub const AI_SHIELD: u32 = 1 << 2;
/// The swing was a feint: it will turn into another before it lands (for the game's own look; the
/// runtime judges what it does).
pub const AI_FEINT: u32 = 1 << 3;

/// A thing one of the game's creatures holds, beside its object ([`crate::ObjectDesc`] of the same
/// id, posed in [`crate::GamePoses`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct ActorItem {
    /// The object's [`crate::ObjectDesc::id`].
    pub id: u32,
    /// Who holds it ([`crate::Actor::id`]), and in which hand (0 left, 1 right).
    pub actor: u32,
    pub hand: u32,
    pub flags: u32, // AI_*
    /// How it moves (world, m/s and rad/s): a hit's speed is the blade's against what it meets.
    pub vel: [f32; 3],
    pub _p0: f32,
    pub ang_vel: [f32; 3],
    pub _p1: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ActorItems {
    /// Seqlock word.
    pub seq: u32,
    pub count: u32,
    pub frame: u64,
    pub _pad: [u32; 12],
    pub items: [ActorItem; MAX_ACTOR_ITEMS],
}

impl ActorItems {
    pub fn entries(&self) -> &[ActorItem] {
        &self.items[..(self.count as usize).min(MAX_ACTOR_ITEMS)]
    }

    pub fn get(&self, id: u32) -> Option<&ActorItem> {
        self.entries().iter().find(|i| i.id == id)
    }
}

// ---- the game's rules for the player --------------------------------------------------------------
pub const OFF_MELEE_RULES: usize = (OFF_ACTOR_ITEMS + size_of::<ActorItems>()).next_multiple_of(64);

/// The game keeps the player's stamina (`stamina`, `stamina_max`, its units): swings spend it, and a
/// tired player's arms are weaker. Without it the runtime keeps none (unless its own setting says).
pub const RULES_STAMINA: u32 = 1 << 0;
/// Slowing time is the game's to allow: without [`RULES_SLOW_TIME`] while this is set, it's off.
pub const RULES_SAYS_SLOW: u32 = 1 << 1;
pub const RULES_SLOW_TIME: u32 = 1 << 2;
/// Climbing by the hands is the game's to allow ([`RULES_CLIMB`]).
pub const RULES_SAYS_CLIMB: u32 = 1 << 3;
pub const RULES_CLIMB: u32 = 1 << 4;
/// Telekinesis in place of the gravity gloves is the game's to say ([`RULES_TELEKINESIS`]).
pub const RULES_SAYS_TELEKINESIS: u32 = 1 << 5;
pub const RULES_TELEKINESIS: u32 = 1 << 6;
/// The melee buttons: the movement stick's click kicks (an untracked kick, the runtime's `kick`),
/// and slowing time goes on the dominant hand's B. Without it, the movement stick's click is the
/// game's and slowing time the off hand's stick click.
pub const RULES_KICK: u32 = 1 << 7;
/// Telekinesis takes things only, not creatures (Oblivion's own Telekinesis: it moves what lies
/// about, and no one).
pub const RULES_TK_THINGS: u32 = 1 << 8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct MeleeRules {
    /// Seqlock word.
    pub seq: u32,
    pub flags: u32, // RULES_*
    /// The player's stamina now and at most (the game's units: Oblivion's Fatigue).
    pub stamina: f32,
    pub stamina_max: f32,
    /// What a swing costs: the game's units per metre a second of the blade's speed past a swing's
    /// threshold, at a weapon's weight of 1 kg (0: the runtime's own).
    pub swing_cost: f32,
    /// The hand's strength against the runtime's own (1: an average person; the game's skill or
    /// attribute).
    pub strength: f32,
    /// Slowing time: the game's time scale while it's slowed (0: the runtime's own), how long the
    /// full meter lasts and how long it takes to fill again (seconds; 0: the runtime's own).
    pub slow_scale: f32,
    pub focus_time: f32,
    pub focus_refill: f32,
    /// How far telekinesis reaches (metres; 0: the runtime's own): the game's own spell's range.
    pub tk_reach: f32,
    pub _pad: [u32; 6],
}

// ---- the player's side --------------------------------------------------------------------------
pub const OFF_RT_MELEE: usize = (OFF_MELEE_RULES + size_of::<MeleeRules>()).next_multiple_of(64);

/// Time is slowed: run the game's world at `time_scale`.
pub const RTM_SLOW: u32 = 1 << 0;
/// The hands hold the world and climb: move the player by `climb`, without the game's gravity.
pub const RTM_CLIMBING: u32 = 1 << 1;
/// The player is out of stamina (the runtime's own meter, or the game's).
pub const RTM_SPENT: u32 = 1 << 2;

/// A hand imbuing what the other hand holds (or itself holds) with its charged spell.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct Imbue {
    /// The weapon ([`crate::ObjectDesc::id`]; 0: none), and the spell going into it.
    pub object: u32,
    pub spell: u32,
    /// How full of it the weapon is (0..1).
    pub charge: f32,
    pub _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RtMelee {
    /// Seqlock word.
    pub seq: u32,
    pub flags: u32, // RTM_*
    /// How fast the game's time should run (1: normal).
    pub time_scale: f32,
    /// The slowing meter (0..1).
    pub focus: f32,
    /// While climbing: how the arms move the player (world, the world's units a second).
    pub climb: [f32; 3],
    /// The stamina swings have spent so far (the game's units, a running total: the game takes
    /// what it went up by), and the runtime's own meter (0..1) when it keeps one.
    pub stamina_spent: f32,
    pub stamina: f32,
    pub _p0: [u32; 3],
    /// The weapons being imbued, and how full: one per hand.
    pub imbue: [Imbue; 2],
    /// Filled weapons out of the hands (thrown, stuck in something, dropped), the fullest first.
    pub loose_imbues: [Imbue; 2],
    /// A jump off what the hands climb, this frame (with the melee buttons, [`RULES_KICK`]): the
    /// jump force (world, the world's units a second: a velocity change each 90th of a second at
    /// first, eased to none over 0.6 s by the game, as its `Locomotion.Jump`); zeros: none.
    pub jump: [f32; 3],
    pub _pad: u32,
}

impl RtMelee {
    /// How full weapon `id` is with its spell, in a hand or out of them.
    pub fn imbue_of(&self, id: u32) -> Option<Imbue> {
        self.imbue.iter().chain(self.loose_imbues.iter()).find(|i| i.object == id && i.charge > 0.0).copied()
    }
}

pub const OFF_MELEE_END: usize = OFF_RT_MELEE + size_of::<RtMelee>();

const _: () = {
    assert!(size_of::<MeleeHandle>() == 64);
    assert!(size_of::<MeleeBlade>() == 208);
    assert!(size_of::<MeleeWeapon>() == 64 + 64 * MAX_HANDLES + 208 * MAX_BLADES);
    assert!(size_of::<ActorItem>() == 48);
    assert!(size_of::<MeleeRules>() == 64);
    assert!(size_of::<RtMelee>() == 128);
    assert!(OFF_MELEE_WEAPONS >= crate::OFF_FACE_SCULPT + size_of::<crate::FaceSculpt>());
    assert!(OFF_MELEE_END <= crate::OFF_HUD_LAYOUT);
};
