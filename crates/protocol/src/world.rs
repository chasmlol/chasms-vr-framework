//! The world around the player: the player's tracked body, the objects they can pick up (guns,
//! magazines, props) and the static collision near them.
//!
//! # Objects
//!
//! The game registers what the player can handle as [`ObjectDesc`]s (shape, mass, grab points,
//! weapon data). The runtime simulates them (Rapier physics: grabbing, throwing, holstering,
//! reloading, gravity-glove pulls) and publishes where each one is in [`ObjectStates`]; the game
//! draws them there and applies the outcomes (a weapon's `shots` counter going up means one round
//! was fired from its muzzle, along its sight line; a grenade's means it went off where it is).
//!
//! Throwables ([`OBJ_THROWABLE`], `THROW_*`): the runtime owns the hand side (arming, the fuse,
//! throwing, a knife's flight and where it sticks); the game owns the explosion and the damage.
//! Its explosions push what the runtime simulates through [`crate::EV_BLAST`].
//!
//! # Actions
//!
//! A gun that doesn't cycle itself says how it's worked ([`ObjectDesc::action`], `ACTION_*`): a
//! bolt or a pump worked by hand after every shot, a break action whose barrels tip open about a
//! hinge, a barrel or cylinder that swings out; and how it's loaded and powered (`WPN_*`): round by
//! round into its chambers ([`WPN_LOADS_CHAMBERS`]) or its fixed magazine ([`WPN_LOADS_ROUNDS`]),
//! or with air its pump builds ([`WPN_PNEUMATIC`]). The runtime works them with the player's hands and reports where the
//! moving parts are (`slide`, `fuse`, [`OS_ACTION_OPEN`]) and what's in the chambers
//! (`chambered`), so the game draws them; the game still only fires each shot. All of it lives in
//! what used to be padding: a table written without it (zeros) gets self-loading guns, as before.
//!
//! An object the player holds (or has holstered) is reported in *tracking space* ([`OS_TRACKING`]),
//! so the adapter maps it with the same `View` as the hands and it never lags behind them while
//! the player moves. Everything else is reported in the world.
//!
//! # Hand poses
//!
//! A game that animates its own first-person hands knows exactly how they hold each of its guns.
//! The adapter hands those poses to the runtime as [`HandPoseDesc`]s in the [`HandPoseTable`]
//! (written like the object table, read again whenever its `seq` changes), and each
//! [`ObjectDesc`] names its firing-hand and support-hand pose (`grip_pose`, `support_pose`). A
//! hand holding a weapon that has a pose is drawn exactly in it: the wrist where the game's wrist
//! is on the gun, every finger joint as the game has it, the hand scaled to the game's hand size,
//! and the index finger curling from its resting place towards the pulled pose with the trigger.
//!
//! # Collision
//!
//! The adapter streams static world geometry near the player (metres, world space) through the
//! collision ring: hands, held objects and thrown objects collide with it, and the head fades out
//! when it goes into a wall.
//!
//! # Body
//!
//! [`RtBody`] is the player's full body, solved by the runtime from the head and hands: 22
//! humanoid joints plus finger curls. The runtime draws it itself unless the game says it draws
//! the body ([`crate::GS_DRAWS_BODY`]).
//!
//! Joint frames follow one convention so any skeleton can be retargeted. Each joint has a
//! *canonical frame*, and a joint's rotation is that frame's absolute orientation in tracking
//! space (its columns are the frame's X, Y and Z axes), so `rot * Y` always points along the bone.
//! In the reference pose (a T-pose facing +Z: up +Y, arms straight out along +-X, palms down, legs
//! straight down) the spine chain is the identity, the left arm is (X, Y, Z) = (+Y, -X, +Z), the
//! right arm (-Y, +X, +Z), the legs (-X, -Y, +Z) and the feet (-X, +Z, +Y). The frames:
//!
//! * spine chain (hips .. head): +Y towards the child joint (up), +Z forward, +X to the body's right;
//! * arms (shoulder, upper arm, lower arm) and hands: +Y along the bone towards the fingers, +Z
//!   forward in the reference pose (the thumb side for hands), X = Y x Z;
//! * legs (upper leg, lower leg): +Y along the bone towards the foot (down), +Z forward (knee);
//! * feet and toes: +Y towards the toes, +Z up.

use bytemuck::{Pod, Zeroable};

use crate::Pose;

pub const OFF_BODY: usize = 0x0001_0000;
pub const OFF_OBJECTS: usize = 0x0001_1000;
pub const MAX_OBJECTS: usize = 256;
pub const OFF_OBJECT_STATES: usize = 0x0003_0000;
pub const OFF_HAND_POSES: usize = 0x0003_8000;
pub const MAX_HAND_POSES: usize = 64;
pub const OFF_COLLISION: usize = 0x0004_0000;
/// Collision ring: `u64` head (producer) at +0x00, `u64` tail (consumer) at +0x40, bytes at +0x80.
pub const COLLISION_RING_BYTES: usize = 0x0040_0000;
pub const COL_RING_DATA_OFF: usize = 0x80;
pub const COL_RING_DATA_BYTES: usize = COLLISION_RING_BYTES - COL_RING_DATA_OFF;
pub const OFF_KICKS: usize = OFF_COLLISION + COLLISION_RING_BYTES;
pub const MAX_KICKS: usize = 256;
/// `KickDesc::object` for the gun the game holds in legacy mode (no objects registered): its
/// kicks are in the `WeaponRig`'s weapon space.
pub const KICK_OBJECT_HELD: u32 = u32::MAX;

// ---- body -------------------------------------------------------------------------------------
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Joint {
    Hips = 0,
    Spine = 1,
    Chest = 2,
    UpperChest = 3,
    Neck = 4,
    Head = 5,
    LeftShoulder = 6,
    LeftUpperArm = 7,
    LeftLowerArm = 8,
    LeftHand = 9,
    RightShoulder = 10,
    RightUpperArm = 11,
    RightLowerArm = 12,
    RightHand = 13,
    LeftUpperLeg = 14,
    LeftLowerLeg = 15,
    LeftFoot = 16,
    LeftToes = 17,
    RightUpperLeg = 18,
    RightLowerLeg = 19,
    RightFoot = 20,
    RightToes = 21,
}
pub const JOINT_COUNT: usize = 22;

impl Joint {
    pub const ALL: [Joint; JOINT_COUNT] = [
        Joint::Hips,
        Joint::Spine,
        Joint::Chest,
        Joint::UpperChest,
        Joint::Neck,
        Joint::Head,
        Joint::LeftShoulder,
        Joint::LeftUpperArm,
        Joint::LeftLowerArm,
        Joint::LeftHand,
        Joint::RightShoulder,
        Joint::RightUpperArm,
        Joint::RightLowerArm,
        Joint::RightHand,
        Joint::LeftUpperLeg,
        Joint::LeftLowerLeg,
        Joint::LeftFoot,
        Joint::LeftToes,
        Joint::RightUpperLeg,
        Joint::RightLowerLeg,
        Joint::RightFoot,
        Joint::RightToes,
    ];

    /// Parent joint (None for the hips).
    pub fn parent(self) -> Option<Joint> {
        use Joint::*;
        Some(match self {
            Hips => return None,
            Spine => Hips,
            Chest => Spine,
            UpperChest => Chest,
            Neck => UpperChest,
            Head => Neck,
            LeftShoulder | RightShoulder => UpperChest,
            LeftUpperArm => LeftShoulder,
            LeftLowerArm => LeftUpperArm,
            LeftHand => LeftLowerArm,
            RightUpperArm => RightShoulder,
            RightLowerArm => RightUpperArm,
            RightHand => RightLowerArm,
            LeftUpperLeg | RightUpperLeg => Hips,
            LeftLowerLeg => LeftUpperLeg,
            LeftFoot => LeftLowerLeg,
            LeftToes => LeftFoot,
            RightLowerLeg => RightUpperLeg,
            RightFoot => RightLowerLeg,
            RightToes => RightFoot,
        })
    }
}

/// Finger curls for one hand: 0 straight .. 1 fully curled.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Fingers {
    /// Thumb, index, middle, ring, pinky.
    pub curl: [f32; 5],
    /// 0 together .. 1 spread.
    pub splay: f32,
    pub _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct HandState {
    /// Where the hand is drawn (tracking space, the hand joint's canonical frame). With physics
    /// hands this is the simulated hand, which a wall can hold back from the controller.
    pub pose: Pose,
    pub flags: u32, // HAND_*
    /// Object held by this hand (0: none).
    pub holding: u32,
    pub _pad: [u32; 2],
}
pub const HAND_TRACKED: u32 = 1 << 0;
/// The physics hand is held back by the world: also show a ghost hand at the controller.
pub const HAND_BLOCKED: u32 = 1 << 1;
pub const HAND_HOLDING: u32 = 1 << 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RtBody {
    pub seq: u32,
    pub flags: u32, // BODY_*
    pub frame_index: u64,
    /// Calibrated standing eye height, metres.
    pub eye_height: f32,
    /// Avatar scale applied to fit the player.
    pub scale: f32,
    pub arm_length: f32,
    pub visibility: u32, // BODYVIS_*
    /// Tracking-space joint poses (see the module docs for the frames).
    pub joints: [Pose; JOINT_COUNT],
    pub fingers: [Fingers; 2],
    pub hands: [HandState; 2],
}
pub const BODY_VALID: u32 = 1 << 0;
pub const BODYVIS_HANDS: u32 = 0;
pub const BODYVIS_ARMS: u32 = 1;
pub const BODYVIS_FULL: u32 = 2;

// ---- objects ----------------------------------------------------------------------------------
/// Something the player can handle. Object space is left-handed metres; guns have the barrel
/// along +Z.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ObjectDesc {
    /// Non-zero and stable while the object exists. 0 marks an empty table entry.
    pub id: u32,
    pub kind: u32,  // OBJ_*
    pub flags: u32, // OBJF_*
    /// Bump to (re)place the object at `spawn_pose` (world space), e.g. when it first appears.
    pub spawn_seq: u32,
    pub spawn_pose: Pose,
    pub shape: u32, // SHAPE_*
    pub mass: f32,
    /// Starts in this body slot (`SLOT_*` + 1) instead of at `spawn_pose`; 0 = no.
    pub start_slot: u32,
    /// Throwables ([`OBJ_THROWABLE`]): what kind, and how it arms (`THROW_*`).
    pub throw_flags: u32,
    /// Box: half extents. Capsule: `radius` and `half_extents[1]` = half height of the cylinder (Y axis).
    pub half_extents: [f32; 3],
    pub radius: f32,
    /// Collider centre in object space.
    pub center: [f32; 3],
    /// Weapons with an [`ObjectDesc::action`]: how far its moving part turns, radians. A break
    /// action's barrels open this far about weapon +X through the hinge (`handle`; positive tips
    /// the muzzle down), a swing action's barrel or cylinder swings out this far about weapon +Z
    /// through it (positive swings its top to the left); a bolt's handle lifts this far about the
    /// bore (the line through `muzzle` along +Z; positive lifts a handle on the right side) before
    /// it slides back (0: a straight pull).
    pub action_angle: f32,
    /// Where the palm holds it (guns: the pistol grip; knives: the handle). The held object keeps
    /// its +Z along the controller's aim.
    pub grip: [f32; 3],
    /// Air guns ([`WPN_PNEUMATIC`]): the pressure one full stroke of the pump (the handle) adds, as
    /// a fraction of full (1).
    pub pump_pressure: f32,
    /// Where the other hand holds a two-handed gun. A break action's foregrip ahead of its hinge is
    /// on the barrels: while they're open it turns with them, and the hand on it opens and shuts
    /// them instead of steering the gun.
    pub foregrip: [f32; 3],
    /// Air guns: the pressure a shot spends (fraction of full).
    pub shot_pressure: f32,
    /// Weapon: where a magazine's top goes in; guns loaded round by round into their chambers
    /// ([`WPN_LOADS_CHAMBERS`]): the first chamber's mouth (where a round's base sits when it's
    /// in); into their magazine ([`WPN_LOADS_ROUNDS`]): its loading port. Magazine: its top (the
    /// insertion point); a round: its base.
    pub mag_well: [f32; 3],
    /// [`WPN_LOADS_CHAMBERS`]: how far each chamber's mouth is from the one before, along weapon +X
    /// (side-by-side barrels; 0 for one chamber): chamber `k` is at `mag_well + k * chamber_step`
    /// (on the barrels, so it turns with them).
    pub chamber_step: f32,
    /// Guns: the muzzle. Throwing knives: the blade's tip (the blade runs from `grip` to it).
    pub muzzle: [f32; 3],
    /// Air guns: below this pressure (fraction of full) it doesn't fire (the trigger only hisses).
    pub min_pressure: f32,
    /// A point on the line of sight (the line runs along +Z through it).
    pub sight: [f32; 3],
    pub _p6: f32,
    /// Charging handle / slide grab point; pulling it back `handle_travel` metres racks the gun.
    /// Bolt and pump actions: the bolt's handle or the pump's forend, worked back and forward after
    /// every shot. Air guns: the pump. Break and swing actions: a point on the hinge's pin (with
    /// `handle_travel` 0: there's nothing to pull). Grenades with [`THROW_PIN`]: the pin's ring (or
    /// a fuse's end); pulling it `handle_travel` away (0: 5 cm) pulls it.
    pub handle: [f32; 3],
    pub handle_travel: f32,
    /// Weapons and magazines with the same ammo type fit together.
    pub ammo_type: u32,
    /// Magazine capacity (weapons: of their magazine).
    pub capacity: u32,
    /// Magazines: rounds when spawned. Weapons with [`OBJF_GAME_AMMO`]: every round in the gun as
    /// it arrives (or is respawned): a self-loading gun chambers one of them and its seated
    /// magazine holds the rest (0: the chamber and the magazine empty); a gun worked by hand has
    /// them all in its magazine.
    pub rounds: u32,
    pub weapon_flags: u32, // WPN_*
    /// Rounds per second on full auto.
    pub fire_rate: f32,
    /// Recoil per shot: muzzle climb (degrees), sideways scatter (degrees), push back (metres).
    pub recoil_pitch: f32,
    pub recoil_yaw: f32,
    pub recoil_back: f32,
    /// UTF-8, NUL-terminated.
    pub name: [u8; 32],
    /// How the hand holds it: the controller's grip-pose rotation in object space, with the grip
    /// pose's origin at `grip` (guns: on the pistol grip, so the barrel points where the hand
    /// does). All zero: the barrel follows the controller's aim ray. Throwables with one snap into
    /// the hand that way (the left hand takes the mirror image).
    pub grip_rot: [f32; 4],
    /// Two-handed guns: the support hand's controller grip rotation in weapon space while it holds
    /// the foregrip. All zero: palm up under the barrel.
    pub support_rot: [f32; 4],
    /// Grenades: seconds from arming to going off (0: the runtime's default, 4 s; an impact
    /// grenade with 0 waits for its impact). The game's own number when it has one.
    pub fuse: f32,
    /// Impact grenades ([`THROW_IMPACT`]): go off on a hard hit at least this long after the throw.
    pub impact_delay: f32,
    /// Recoil character (EFT-style; 0 = the runtime's default for the gun type):
    /// - `recoil_climb`: how much of each kick stays while firing on full auto (0..1);
    /// - `recoil_return`: how fast the gun comes back down after a shot (1/s);
    /// - `recoil_angle`: how far from straight up a shot's kick may lean, either way (degrees).
    pub recoil_climb: f32,
    pub recoil_return: f32,
    pub recoil_angle: f32,
    /// Ids of the game's own hand poses for this object in the [`HandPoseTable`]: the firing hand
    /// (on `grip`) and the support hand (on `foregrip`, sliding along the foregrip line with the
    /// hand); 0 = none (the runtime curls the fingers itself). Weapons with an authored `grip_rot`.
    pub grip_pose: u32,
    pub support_pose: u32,
    /// Weapons: how it's worked between shots and loaded (`ACTION_*`; 0: it cycles itself, as a
    /// self-loading gun does).
    pub action: u32,
}
pub const OBJ_PROP: u32 = 1;
pub const OBJ_WEAPON: u32 = 2;
pub const OBJ_MAGAZINE: u32 = 3;
pub const OBJ_THROWABLE: u32 = 4;
pub const OBJ_MELEE: u32 = 5;

pub const OBJF_GRABBABLE: u32 = 1 << 0;
/// Gravity gloves can pull it.
pub const OBJF_PULLABLE: u32 = 1 << 1;
pub const OBJF_HOLSTERABLE: u32 = 1 << 2;
pub const OBJF_TWO_HANDED: u32 = 1 << 3;
pub const OBJF_SCOPED: u32 = 1 << 4;
/// Not placed in the world: the runtime spawns copies of it (magazines from the ammo pouch).
pub const OBJF_TEMPLATE: u32 = 1 << 5;
/// The runtime draws it (with the player's hands, lit by the game's [`crate::GameLighting`]) from
/// the model its `name` gives as `<game>/<asset key>` in the runtime's asset cache
/// (`metro2033/wpn_2012`); the game doesn't. A weapon is drawn with its moving parts where its
/// [`ObjectState`] puts them; a magazine (or a round) naming the same model as its gun's magazine
/// (or round) part, in place (magazine space is the gun's).
pub const OBJF_RUNTIME_DRAWS: u32 = 1 << 6;
/// Weapons: the game keeps this gun's ammunition (Metro's magazine and reserve), so the runtime's
/// rounds follow the game's instead of being unlimited. The gun arrives (and comes back with each
/// `spawn_seq`) holding `rounds` in all, not a full magazine: a self-loading gun chambers one of
/// them and its seated magazine holds the rest (0: chamber and magazine empty). A magazine the hand
/// takes from the pouch holds its template's `rounds`: what the game's own reload would load now,
/// less a live round already in the gun's chamber (0: the pouch is empty). The game rewrites both
/// as its ammunition changes.
pub const OBJF_GAME_AMMO: u32 = 1 << 7;

pub const SHAPE_BOX: u32 = 0;
pub const SHAPE_CAPSULE: u32 = 1;
pub const SHAPE_SPHERE: u32 = 2;

pub const WPN_AUTOMATIC: u32 = 1 << 0;
/// The slide or bolt locks back after the last round.
pub const WPN_SLIDE_LOCKS: u32 = 1 << 1;
/// A fresh magazine needs the gun racked when the chamber is empty.
pub const WPN_NEEDS_RACK: u32 = 1 << 2;
pub const WPN_PISTOL: u32 = 1 << 3;
/// Shoulder-fired: the virtual stock may anchor it.
pub const WPN_STOCK: u32 = 1 << 4;
/// An air gun: its shots spend pressure (`shot_pressure`) that strokes of its pump (the handle)
/// build (`pump_pressure`); below `min_pressure` it doesn't fire. It starts empty. The runtime
/// reports the pressure in [`ObjectState::fuse`]; a shot's strength is the pressure it was fired
/// at (the value reported before `shots` went up).
pub const WPN_PNEUMATIC: u32 = 1 << 5;
/// Loaded round by round straight into its `capacity` chambers (a double barrel's two, a single
/// shot's one): no magazine. Its ammo template is one round (an [`OBJ_MAGAZINE`] of capacity 1)
/// that goes into an empty chamber when its base is brought to the chamber's mouth (see
/// [`ObjectDesc::chamber_step`]); it fires the chambers in order, and their spent cases stay in
/// them until the action is opened.
pub const WPN_LOADS_CHAMBERS: u32 = 1 << 6;
/// Its magazine stays in it (a tube, a cylinder, a dart cluster: `capacity` rounds) and is loaded
/// one round at a time through its port at `mag_well`: its ammo template is one round, which goes in
/// when its base is brought to the port pointing along weapon +Z. It arrives full.
/// [`ObjectState::rounds`] is how many the magazine holds.
pub const WPN_LOADS_ROUNDS: u32 = 1 << 7;

/// [`ObjectDesc::action`]: it cycles itself (self-loading, semi or full auto): the default.
pub const ACTION_SELF_LOADING: u32 = 0;
/// A bolt action: after every shot the bolt's handle (`handle`) is lifted (`action_angle`), pulled
/// back (`handle_travel`: the case flies out) and pushed forward and down again (chambering the
/// next round) before it fires again.
pub const ACTION_BOLT: u32 = 1;
/// A pump action: the forend (`handle`) is pulled back and pushed forward after every shot.
pub const ACTION_PUMP: u32 = 2;
/// A break action: the barrels tip down about a hinge (`handle`, weapon +X) by `action_angle` when
/// unlatched (B or A on the gun hand, a flick down, or the other hand on the barrels' foregrip
/// pulling them), falling open under their weight; opened, spent cases fly out and the chambers can
/// be loaded; they shut with a flick up, under their weight with the muzzle down, or pushed by the
/// other hand, and latch. It fires only shut.
pub const ACTION_BREAK: u32 = 3;
/// A swing-out action: the barrel or cylinder swings out sideways about a pin along the bore
/// (`handle`, weapon +Z) by `action_angle`, otherwise as a break action. A magazine-fed one (a
/// revolver's cylinder and its speed loader) takes and drops its magazine only while it's out.
pub const ACTION_SWING: u32 = 4;

/// [`ObjectState::chambered`]: bits 0..15 are the chambers holding a live round, bits 16..31 the
/// ones holding a spent case (chamber `k` is bit `k`; a gun with one chamber uses bit 0).
pub const CHAMBER_SPENT_SHIFT: u32 = 16;
pub const CHAMBER_LIVE_MASK: u32 = 0xFFFF;

/// [`ObjectDesc::throw_flags`]: a grenade. The runtime arms it, times its fuse (or waits for its
/// impact) and tells the game when it goes off ([`ObjectState::shots`], [`OS_SPENT`]); the game
/// does the explosion.
pub const THROW_GRENADE: u32 = 1 << 0;
/// It has a pin (at `handle`) the other hand pulls. Without one, a button on the holding hand arms it.
pub const THROW_PIN: u32 = 1 << 1;
/// A lever (spoon) holds the fuse until the grenade is let go of (or cooked): the fuse starts then,
/// not when the pin comes out.
pub const THROW_LEVER: u32 = 1 << 2;
/// Goes off on a hard impact once armed and thrown (after `impact_delay`), rather than on a timer
/// (or as well as one, if `fuse` is set).
pub const THROW_IMPACT: u32 = 1 << 3;
/// The game times the fuse: the runtime only reports it armed, and the game removes or respawns it.
pub const THROW_GAME_FUSE: u32 = 1 << 4;
/// A throwing knife: flies blade first (the blade from `grip` to the tip at `muzzle`) and sticks
/// in what it hits tip first ([`OS_STUCK`]).
pub const THROW_KNIFE: u32 = 1 << 5;
/// Sticks to whatever it hits once thrown (sticky bombs).
pub const THROW_STICKY: u32 = 1 << 6;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ObjectTable {
    pub seq: u32,
    /// Entries in use are `descs[..count]` (with gaps where `id` is 0).
    pub count: u32,
    pub _pad: [u32; 14],
    pub descs: [ObjectDesc; MAX_OBJECTS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ObjectState {
    pub id: u32,
    pub flags: u32, // OS_*
    /// Holster slot (OS_HOLSTERED): see `SLOT_*`.
    pub slot: u32,
    /// The weapon a seated magazine is in (OS_IN_WEAPON), or what a stuck throwable is in
    /// ([`OS_STUCK`]; 0: the world).
    pub parent: u32,
    pub pose: Pose,
    pub lin_vel: [f32; 3],
    /// Magazines: rounds left. Weapons: rounds in the seated magazine.
    pub rounds: u32,
    pub ang_vel: [f32; 3],
    /// Weapons: rounds fired so far (each increase is one shot from the muzzle). Grenades: times it
    /// went off (each increase is one explosion at `pose`, for the game to make).
    pub shots: u32,
    /// Weapons: which chambers hold a live round and which a spent case (see
    /// [`CHAMBER_SPENT_SHIFT`]); `chambered & CHAMBER_LIVE_MASK != 0`: a round is chambered. (A gun
    /// that cycles itself only ever sets bit 0.)
    pub chambered: u32,
    /// Weapons: slide / charging handle / bolt / pump position, 0 forward .. 1 fully back. Break and
    /// swing actions: how far the barrels (or cylinder) are open, 0 shut .. 1 fully open
    /// (`action_angle`).
    pub slide: f32,
    /// Which [`ObjectDesc`] to draw it with (itself, or the template it was spawned from).
    pub desc: u32,
    /// Grenades: seconds left on a burning fuse the runtime times ([`OS_ARMED`]), else 0. Air guns
    /// ([`WPN_PNEUMATIC`]): the pressure, 0 empty .. 1 full. Bolt actions: how far the bolt's handle
    /// is lifted, 0 down .. 1 up (`action_angle`).
    pub fuse: f32,
}
/// The runtime is moving it: draw it at `pose`.
pub const OS_SIMULATED: u32 = 1 << 0;
/// `pose` is in tracking space (held or holstered): map it with the adapter's `View`.
pub const OS_TRACKING: u32 = 1 << 1;
pub const OS_HELD_LEFT: u32 = 1 << 2;
pub const OS_HELD_RIGHT: u32 = 1 << 3;
pub const OS_HOLSTERED: u32 = 1 << 4;
/// A magazine seated in `parent`.
pub const OS_IN_WEAPON: u32 = 1 << 5;
/// Flying to a hand (gravity gloves).
pub const OS_PULLED: u32 = 1 << 6;
/// Highlighted: a hand is about to grab it (or pull it).
pub const OS_HOVERED: u32 = 1 << 7;
/// Spawned by the runtime from a template (e.g. a magazine from the ammo pouch).
pub const OS_SPAWNED: u32 = 1 << 8;
/// The slide / bolt is locked back.
pub const OS_SLIDE_LOCKED: u32 = 1 << 9;
/// Grenades: the fuse is burning (the game may show it lit, and its AI flee it).
pub const OS_ARMED: u32 = 1 << 10;
/// Grenades: the pin is out (draw it without one).
pub const OS_PIN_PULLED: u32 = 1 << 11;
/// Thrown, and still in the air or rolling.
pub const OS_THROWN: u32 = 1 << 12;
/// Stuck in something (a knife's blade, a sticky bomb): `parent` is what (0: the world).
pub const OS_STUCK: u32 = 1 << 13;
/// Went off: inert, the game hides it until it respawns it (`spawn_seq`) or removes it.
pub const OS_SPENT: u32 = 1 << 14;
/// Weapons: the action is open and won't fire: a break or swing action unlatched, a bolt lifted or
/// back, a pump back.
pub const OS_ACTION_OPEN: u32 = 1 << 15;

pub const SLOT_HIP_RIGHT: u32 = 0;
pub const SLOT_HIP_LEFT: u32 = 1;
/// Over the right / left shoulder, behind the head (long guns).
pub const SLOT_BACK_RIGHT: u32 = 2;
pub const SLOT_BACK_LEFT: u32 = 3;
/// Grenade pouches either side of the sternum, and a belt pouch on the weapon hand's side (small
/// throwables: grenades, knives).
pub const SLOT_CHEST_LEFT: u32 = 4;
pub const SLOT_CHEST_RIGHT: u32 = 5;
pub const SLOT_BELT: u32 = 6;
pub const SLOT_COUNT_BODY: usize = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ObjectStates {
    pub seq: u32,
    pub count: u32,
    pub frame_index: u64,
    pub _pad: [u32; 12],
    pub states: [ObjectState; MAX_OBJECTS],
}

// ---- hand poses ---------------------------------------------------------------------------------
/// How the game's own hand holds an object: one hand, every finger joint. Rotations use the joint
/// convention above (absolute canonical frames; for the hand +Y towards the fingers, +Z the thumb
/// side, X = Y x Z).
///
/// Finger segments have canonical frames too: +Y along the segment towards the next joint (the
/// last one along its bone), and X = Y x Z with, for the four fingers, +Z as near the hand's +Z
/// (the thumb side, about which they bend) as the segment allows; for the thumb, +X as near the
/// hand's +X. A flat hand has every segment at the identity relative to the hand.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HandPoseDesc {
    /// Non-zero and unique in the table (`ObjectDesc::grip_pose` / `support_pose` name it). 0
    /// marks an empty entry.
    pub id: u32,
    /// The hand the pose is for ([`crate::HAND_LEFT`] or [`crate::HAND_RIGHT`]). The other hand
    /// takes its mirror image through the object's YZ plane at the grip (firing poses) or the
    /// foregrip (support poses), the way the runtime mirrors the grip itself.
    pub side: u32,
    pub flags: u32, // HPOSE_*
    /// The size of the hand that took the pose: wrist to the base of the middle finger, metres.
    /// The drawn hand is scaled about its wrist to it, so the fingertips land where the game's do.
    pub size: f32,
    /// The hand joint's canonical frame in object space.
    pub wrist: Pose,
    /// Each finger segment's canonical rotation relative to the hand's frame: thumb, index,
    /// middle, ring, little, three segments each from the knuckle out (`[x, y, z, w]`; all zero
    /// reads as the identity).
    pub fingers: [[f32; 4]; 15],
    /// [`HPOSE_TRIGGER`]: the index finger's three segments at a full trigger pull (relative to the
    /// hand, like `fingers`). The index blends from `fingers[3..6]` to these with the trigger.
    pub index_pulled: [[f32; 4]; 3],
    /// Where each finger's last segment is (thumb .. little): `xyz` a point on its axis in the
    /// hand's frame (metres, for a hand of `size`), `w` how far out along the segment it is (0 its
    /// pivot .. 1 the fingertip; 0 or less: none). Usually the fingertip; for a trigger finger, where
    /// it touches the trigger. A drawn hand of other proportions bends its fingers a little to
    /// reach these, so its fingertips land where the game's do.
    pub reach: [[f32; 4]; 5],
    /// [`HPOSE_TRIGGER`]: the index's `reach` point at a full pull (blended with the trigger too).
    pub index_reach_pulled: [f32; 4],
}
/// The index finger rests on a trigger and curls to `index_pulled` as it's pulled.
pub const HPOSE_TRIGGER: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HandPoseTable {
    pub seq: u32,
    /// Entries in use are `poses[..count]` (with gaps where `id` is 0).
    pub count: u32,
    pub _pad: [u32; 14],
    pub poses: [HandPoseDesc; MAX_HAND_POSES],
}

impl HandPoseTable {
    /// The pose with this id (0 is never one).
    pub fn find(&self, id: u32) -> Option<&HandPoseDesc> {
        if id == 0 {
            return None;
        }
        self.poses[..(self.count as usize).min(MAX_HAND_POSES)].iter().find(|p| p.id == id)
    }
}

// ---- kicks --------------------------------------------------------------------------------------
/// A gun's recoil as the game authored it, one kick at a time: how the gun moves over the time
/// after a shot, as an offset from where it's held (in the gun's object space). The runtime plays
/// one kick of each [`KickDesc::layer`] per shot, picked at random among the gun's kicks whose
/// [`KickDesc::when`] matches how it was fired, and the kicks of a burst add up. A gun with no
/// kicks in the [`KickTable`] gets the runtime's own recoil.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct KickDesc {
    /// The gun ([`ObjectDesc::id`]); 0 marks an empty entry.
    pub object: u32,
    pub layer: u32, // KICK_LAYER_*
    pub when: u32,  // KICK_* (one bit of each group must match the shot)
    /// Playback rate (1: as authored).
    pub speed: f32,
    /// The point it turns the gun about (object space).
    pub pivot: [f32; 3],
    /// How long it lasts in authored time (s).
    pub length: f32,
    /// [`KICK_PITCH`] .. [`KICK_Z`].
    pub channels: [KickCurve; KICK_CHANNELS],
}

/// One channel: `keys[..count]` in time order, each `[t (s), value, slope in, slope out]` (slopes
/// per second). Between keys the channel is the cubic Hermite through them; before the first it
/// holds the first value and after the last the last one.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct KickCurve {
    pub count: u32,
    pub _pad: [u32; 3],
    pub keys: [[f32; 4]; KICK_KEYS],
}
pub const KICK_KEYS: usize = 16;

/// Muzzle up (rad). The offset turns yaw, then pitch, then roll: `Ry(yaw) * Rx(-pitch) * Rz(roll)`
/// about the pivot, then moves by the translation.
pub const KICK_PITCH: usize = 0;
/// About +Y (rad): positive turns the muzzle (+Z) towards +X.
pub const KICK_YAW: usize = 1;
/// About +Z, the barrel (rad).
pub const KICK_ROLL: usize = 2;
/// Translation (m), object space.
pub const KICK_X: usize = 3;
pub const KICK_Y: usize = 4;
pub const KICK_Z: usize = 5;
pub const KICK_CHANNELS: usize = 6;

/// The game's view kick (on a flat screen, the camera's jolt, which takes the gun with it). A
/// shot's view kick adds to the ones still playing.
pub const KICK_LAYER_VIEW: u32 = 0;
/// The gun model's own kick (an animation): a shot's replaces the one playing.
pub const KICK_LAYER_GUN: u32 = 1;
pub const KICK_LAYERS: usize = 2;

/// [`KickDesc::when`]: fired from the hip, or aimed (shouldered or down the sights) ...
pub const KICK_HIP: u32 = 1 << 0;
pub const KICK_AIMED: u32 = 1 << 1;
/// ... standing or crouched ...
pub const KICK_STANDING: u32 = 1 << 2;
pub const KICK_CROUCHED: u32 = 1 << 3;
/// ... and the first shot of a burst, the next few, or a long burst's later ones.
pub const KICK_FIRST: u32 = 1 << 4;
pub const KICK_FOLLOW: u32 = 1 << 5;
pub const KICK_LONG: u32 = 1 << 6;

impl KickCurve {
    pub fn keys(&self) -> &[[f32; 4]] {
        &self.keys[..(self.count as usize).min(KICK_KEYS)]
    }

    /// The channel's value at `t` (authored seconds).
    pub fn value(&self, t: f32) -> f32 {
        let k = self.keys();
        let (Some(first), Some(last)) = (k.first(), k.last()) else { return 0.0 };
        if t <= first[0] {
            return first[1];
        }
        if t >= last[0] {
            return last[1];
        }
        let i = k.partition_point(|x| x[0] <= t).clamp(1, k.len() - 1);
        let (a, b) = (k[i - 1], k[i]);
        let h = (b[0] - a[0]).max(1e-6);
        let s = (t - a[0]) / h;
        let (s2, s3) = (s * s, s * s * s);
        (2.0 * s3 - 3.0 * s2 + 1.0) * a[1] + (s3 - 2.0 * s2 + s) * h * a[3] + (-2.0 * s3 + 3.0 * s2) * b[1] + (s3 - s2) * h * b[2]
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct KickTable {
    pub seq: u32,
    /// Entries in use are `kicks[..count]` (with gaps where `object` is 0).
    pub count: u32,
    pub _pad: [u32; 14],
    pub kicks: [KickDesc; MAX_KICKS],
}

impl KickTable {
    pub fn entries(&self) -> &[KickDesc] {
        &self.kicks[..(self.count as usize).min(MAX_KICKS)]
    }
}

// ---- collision ring -----------------------------------------------------------------------------
/// Every message starts 8-byte aligned with this header; `COL_PAD` means "skip to the ring start".
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ColHeader {
    pub kind: u32, // COL_*
    pub bytes: u32,
}
pub const COL_PAD: u32 = 0;
/// Drop all static geometry (level change).
pub const COL_CLEAR: u32 = 1;
/// [`ColMesh`] + `count` x [`ColTriangle`]: replaces mesh `id` (0 triangles removes it).
pub const COL_MESH: u32 = 2;
/// [`ColBox`]: replaces (or adds) box `id`.
pub const COL_BOX: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ColMesh {
    pub id: u32,
    pub count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ColTriangle {
    /// World metres.
    pub v: [f32; 9],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ColBox {
    pub id: u32,
    pub _pad: u32,
    pub center: [f32; 3],
    pub _p0: f32,
    pub half_extents: [f32; 3],
    pub _p1: f32,
    pub rot: [f32; 4],
}

const _: () = {
    assert!(size_of::<Fingers>() == 32);
    assert!(size_of::<HandState>() == 48);
    assert!(size_of::<RtBody>() == 32 + 32 * JOINT_COUNT + 64 + 96);
    assert!(OFF_BODY + size_of::<RtBody>() <= OFF_OBJECTS);
    assert!(size_of::<ObjectDesc>() == 320);
    assert!(size_of::<ObjectState>() == 96);
    assert!(OFF_OBJECTS + size_of::<ObjectTable>() <= OFF_OBJECT_STATES);
    assert!(OFF_OBJECT_STATES + size_of::<ObjectStates>() <= OFF_HAND_POSES);
    assert!(size_of::<HandPoseDesc>() == 432);
    assert!(size_of::<HandPoseTable>() == 64 + 432 * MAX_HAND_POSES);
    assert!(OFF_HAND_POSES + size_of::<HandPoseTable>() <= OFF_COLLISION);
    assert!(OFF_HAND_POSES.is_multiple_of(64));
    assert!(OFF_COLLISION + COLLISION_RING_BYTES <= OFF_KICKS);
    assert!(size_of::<KickCurve>() == 16 + 16 * KICK_KEYS);
    assert!(size_of::<KickDesc>() == 32 + size_of::<KickCurve>() * KICK_CHANNELS);
    assert!(OFF_KICKS.is_multiple_of(64));
    assert!(OFF_KICKS + size_of::<KickTable>() <= crate::MAPPING_BYTES);
    assert!(size_of::<ColBox>() == 56);
};
