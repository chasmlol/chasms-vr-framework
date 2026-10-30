/* RustVR shared-memory protocol. Generated from crates/protocol (cbindgen): do not edit.
 * See crates/protocol/src/lib.rs for what every field means. */

#ifndef RUSTVR_PROTOCOL_H
#define RUSTVR_PROTOCOL_H

#include <stdint.h>

#define RVR_MAGIC 1381389650

#define RVR_VERSION 12

#define RVR_MAPPING_BYTES 5373952

#define RVR_OFF_HEADER 0

#define RVR_OFF_SUBMIT 128

#define RVR_OFF_TARGETS 256

#define RVR_OFF_RT_FRAME 512

#define RVR_OFF_GAME_STATE 3072

#define RVR_OFF_SLOTS 4096

#define RVR_OFF_EVENTS 8192

/**
 * Number of texture slots in the triple buffer.
 */
#define RVR_SLOT_COUNT 3

#define RVR_SLOT_STRIDE 256

/**
 * Events ring: `u64` head (producer) at +0x00, `u64` tail (consumer) at +0x40, entries at +0x80.
 */
#define RVR_EVENT_RING_ENTRIES 256

#define RVR_RING_HEAD_OFF 0

#define RVR_RING_TAIL_OFF 64

#define RVR_RING_DATA_OFF 128

/**
 * A connected side is alive while its heartbeat is at most this old.
 */
#define RVR_HEARTBEAT_TIMEOUT_MS 2000

#define RVR_SUBMIT_DIRTY (1 << 2)

#define RVR_TGT_DEPTH (1 << 0)

#define RVR_TGT_HUD (1 << 1)

#define RVR_TGT_AUX (1 << 2)

/**
 * The light atlas (`LIGHT_ATLAS_W` x `LIGHT_ATLAS_H` RGBA8, see [`lighting`]).
 */
#define RVR_TGT_LIGHTS (1 << 3)

/**
 * The game's post chain's light and lens per eye, for the player the runtime draws over the game's
 * finished picture to get them too. Two `R16G16B16A16_FLOAT` textures per eye ([`bloom_size`]):
 *
 * * **bloom** ([`TexKind::BloomLeft`]): rgb, light the game adds to each pixel before its tone
 *   curve (its bloom, through the lens dirt), and an eighth of after; a, the lens's warp in x;
 * * **lens** ([`TexKind::LensLeft`]): rgb, light added after the tone curve (a gas mask visor's
 *   sheen); a, the warp in y.
 *
 * The warp (in texture coordinates, at the screen position) is where the picture there comes from
 * (a visor's glass, heat haze): the game shows at `uv` what it drew at `uv + warp(uv)`.
 */
#define RVR_TGT_BLOOM (1 << 4)

#define RVR_CTRL_A (1 << 0)

#define RVR_CTRL_B (1 << 1)

#define RVR_CTRL_STICK_CLICK (1 << 2)

#define RVR_CTRL_MENU (1 << 3)

#define RVR_CTRL_TRIGGER_CLICK (1 << 4)

#define RVR_CTRL_SQUEEZE_CLICK (1 << 5)

#define RVR_CTRL_TOUCH_TRIGGER (1 << 6)

#define RVR_CTRL_TOUCH_A (1 << 7)

#define RVR_CTRL_TOUCH_B (1 << 8)

#define RVR_CTRL_TOUCH_STICK (1 << 9)

/**
 * The thumb rests on the thumbrest (Touch controllers).
 */
#define RVR_CTRL_TOUCH_REST (1 << 10)

#define RVR_HAND_LEFT 0

#define RVR_HAND_RIGHT 1

/**
 * Tracking is valid and the runtime wants the game in VR.
 */
#define RVR_RT_ACTIVE (1 << 0)

#define RVR_RT_USER_PRESENT (1 << 1)

/**
 * Poses come from the simulated headset (tests).
 */
#define RVR_RT_SIMULATED (1 << 2)

/**
 * The runtime waits for every frame (tests): the adapter should render exactly once per
 * `frame_index`, using `dt` as its time step.
 */
#define RVR_RT_LOCKSTEP (1 << 3)

/**
 * The runtime's own menu has the controllers: the game should ignore gameplay intents.
 */
#define RVR_RT_RUNTIME_MENU (1 << 4)

/**
 * The headset's system dashboard is open.
 */
#define RVR_RT_DASHBOARD (1 << 5)

/**
 * The runtime draws the player's body and hands over the game's picture (the game shouldn't draw
 * its own first-person hands or arms).
 */
#define RVR_RT_DRAWS_BODY (1 << 6)

/**
 * The head may move away from the game body (roomscale).
 */
#define RVR_TRK_ROOMSCALE (1 << 0)

/**
 * The runtime is walking the game body after the head: absorb that movement into the origin.
 */
#define RVR_TRK_FOLLOW (1 << 1)

/**
 * Keep the game's camera bob and shake.
 */
#define RVR_TRK_HEAD_BOB (1 << 2)

/**
 * Follow scripted camera animations (ladders, cutscenes).
 */
#define RVR_TRK_CAMERA_ANIMS (1 << 3)

#define RVR_INT_WEAPON (1 << 0)

#define RVR_INT_TWO_HANDED (1 << 1)

/**
 * Aiming down the sights (sights at the eye, or the grip held).
 */
#define RVR_INT_ADS (1 << 2)

#define RVR_INT_SCOPE (1 << 3)

/**
 * The game's menu is shown as a panel: gameplay input is off, the cursor drives the menu.
 */
#define RVR_INT_MENU_MODE (1 << 4)

#define RVR_INT_CURSOR (1 << 5)

#define RVR_INT_CURSOR_DOWN (1 << 6)

/**
 * Manual reload: the magazine is out of the gun (hide it on the model).
 */
#define RVR_INT_MAG_OUT (1 << 7)

/**
 * Manual reload: the off hand holds a fresh magazine (draw one there).
 */
#define RVR_INT_HOLDING_MAG (1 << 8)

/**
 * One past the highest [Action] value (some values in between are unused).
 */
#define RVR_ACTION_COUNT 48

#define RVR_GS_IN_GAME (1 << 0)

/**
 * A game menu is open: the runtime shows the HUD texture as a big panel and points a laser at it.
 */
#define RVR_GS_MENU (1 << 1)

#define RVR_GS_LOADING (1 << 2)

/**
 * The game's camera is being driven by an animation or cutscene.
 */
#define RVR_GS_SCRIPTED (1 << 3)

#define RVR_GS_DEAD (1 << 4)

#define RVR_GS_CROUCHED (1 << 5)

#define RVR_GS_HAS_WEAPON (1 << 6)

#define RVR_GS_PAUSED (1 << 7)

/**
 * The game draws the player itself (from [RtPlayerDraw]: body, hands and what the runtime draws
 * of what they hold, inside the game's own frame); the runtime doesn't.
 */
#define RVR_GS_DRAWS_BODY (1 << 8)

/**
 * Magazines from the ammo pouch never run out.
 */
#define RVR_GS_INFINITE_AMMO (1 << 9)

/**
 * The game wants the runtime's player controller (hands, body) even with no objects registered:
 * what the player sees of themself is the runtime's, drawn over the game's picture.
 */
#define RVR_GS_PLAYER_RIG (1 << 10)

#define RVR_RIG_HAS_SIGHT (1 << 0)

#define RVR_RIG_HAS_MUZZLE (1 << 1)

#define RVR_RIG_SIGHT_LINE (1 << 2)

#define RVR_RIG_HAS_MAG_WELL (1 << 3)

/**
 * The gun can be held with two hands.
 */
#define RVR_RIG_TWO_HANDED (1 << 4)

#define RVR_RIG_HAS_CHARGING_HANDLE (1 << 5)

#define RVR_SLOT_STEREO (1 << 0)

#define RVR_SLOT_HUD (1 << 1)

/**
 * The HUD texture holds the game's whole picture (a menu or loading screen) instead of an overlay.
 */
#define RVR_SLOT_MENU (1 << 2)

#define RVR_SLOT_AUX (1 << 3)

#define RVR_SLOT_DEPTH (1 << 4)

/**
 * Both eyes' bloom and lens textures hold the game's for their pictures.
 */
#define RVR_SLOT_BLOOM (1 << 5)

/**
 * Depth holds linear view distance in metres.
 */
#define RVR_DEPTH_LINEAR (1 << 0)

/**
 * Depth holds a reversed-Z projection's depth (near = 1).
 */
#define RVR_DEPTH_REVERSED (1 << 1)

/**
 * Vibrate: `a` = [`HAPTIC_LEFT`]..[`HAPTIC_OFF_HAND`], f0 seconds, f1 frequency (Hz), f2 amplitude 0..1.
 */
#define RVR_EV_HAPTIC 1

/**
 * The game's own rumble (recoil, damage): f0 strength 0..1.
 */
#define RVR_EV_RUMBLE 2

/**
 * Ask the runtime to recentre (e.g. after a level load).
 */
#define RVR_EV_RECENTER 3

/**
 * The game's explosion pushes what the runtime simulates (its loose objects): f0..f2 the blast's
 * world position (metres), f3 its radius, f4 the impulse (N s) an object at the centre gets,
 * falling off to nothing at the radius.
 */
#define RVR_EV_BLAST 4

/**
 * A push on one object the runtime simulates (a bullet hitting it): `a` its id, f0..f2 the world
 * impulse (N s), f3..f5 the world point it acts at.
 */
#define RVR_EV_IMPULSE 5

/**
 * The game fired its held gun, `a` times since the last one (it counts its own rounds): in
 * legacy mode (the gun glued to the hand) the runtime kicks it with the game's recoil, the kicks
 * registered for [`KICK_OBJECT_HELD`].
 */
#define RVR_EV_GAME_SHOTS 6

#define RVR_HAPTIC_LEFT 0

#define RVR_HAPTIC_RIGHT 1

#define RVR_HAPTIC_FIRING_HAND 2

#define RVR_HAPTIC_OFF_HAND 3

#define RVR_MAX_AVATAR_PARTS 24

#define RVR_MAX_AVATAR_SWAPS 16

#define RVR_OFF_COMBAT 5177344

#define RVR_OFF_ACTORS RVR_OFF_COMBAT

#define RVR_MAX_ACTORS 24

#define RVR_MAX_ACTOR_PARTS 16

#define RVR_PART_TORSO 0

#define RVR_PART_HEAD 1

#define RVR_PART_ARM 2

#define RVR_PART_HAND 3

#define RVR_PART_LEG 4

#define RVR_PART_FOOT 5

/**
 * A shield or a held weapon: hitting it is a block, not a wound.
 */
#define RVR_PART_SHIELD 6

#define RVR_PART_TAIL 7

#define RVR_MAT_FLESH 0

#define RVR_MAT_LEATHER 1

#define RVR_MAT_CHAINMAIL 2

#define RVR_MAT_PLATE 3

#define RVR_MAT_BONE 4

#define RVR_MAT_WOOD 5

#define RVR_MAT_STONE 6

#define RVR_MAT_ETHEREAL 7

/**
 * The ground: dirt, grass, snow.
 */
#define RVR_MAT_EARTH 8

/**
 * Alive (hits on a dead body still count, as a corpse).
 */
#define RVR_ACT_ALIVE (1 << 0)

#define RVR_ACT_HOSTILE (1 << 1)

/**
 * Swinging or thrusting at someone: `weapon_a`..`weapon_b` is its blade, and the player's weapon
 * or shield meeting it is a block.
 */
#define RVR_ACT_ATTACKING (1 << 2)

/**
 * Holding its guard up.
 */
#define RVR_ACT_BLOCKING (1 << 3)

/**
 * Lying down, knocked over or ragdolled.
 */
#define RVR_ACT_DOWN (1 << 4)

/**
 * Its attack is a power attack (the game's heavy blow): it lands harder, and a guard it meets is
 * shoved harder.
 */
#define RVR_ACT_POWER_ATTACK (1 << 5)

#define RVR_MAX_MELEE 32

#define RVR_MAX_DAMAGERS 4

/**
 * Cuts along `edge` (its frame: `edge` is the direction it cuts in).
 */
#define RVR_DMG_SLASH 1

/**
 * Stabs along the blade, at the tip.
 */
#define RVR_DMG_PIERCE 2

/**
 * Bludgeons (any direction).
 */
#define RVR_DMG_BLUNT 3

/**
 * A fist (the hand itself).
 */
#define RVR_DMG_PUNCH 4

/**
 * The runtime's own speed of a full-strength cut (m/s): a one-handed sword's edge
 * at the middle of its blade. A game that measures its own swings scales them onto this, so its
 * weapons keep their proportions (a heavier, slower one needs less speed) and a VR arm can swing
 * them.
 */
#define RVR_FULL_SLASH_SPEED 7.0

/**
 * Cuts both ways along `edge` (a double-edged blade).
 */
#define RVR_DMGF_BOTH_WAYS (1 << 0)

/**
 * Only guards: meets blows (blocks, parries) and never hits (an axe's haft, a sword's grip).
 */
#define RVR_DMGF_GUARD (1 << 1)

/**
 * An axe's edge: an axe's limits (the cut within 45 degrees of the edge, into the
 * surface within 80, at 3 m/s whatever the armour) instead of a sword's (35, 60, by armour).
 */
#define RVR_DMGF_AXE (1 << 2)

#define RVR_MAX_SPELLS 96

/**
 * On the caster.
 */
#define RVR_DELIVER_SELF 0

/**
 * On what the casting hand touches.
 */
#define RVR_DELIVER_TOUCH 1

/**
 * Thrown at a target (a bolt or projectile).
 */
#define RVR_DELIVER_TARGET 2

/**
 * The spell the game has ready (its "selected" or "equipped" one).
 */
#define RVR_SPELL_SELECTED (1 << 0)

/**
 * Enough magicka (and nothing else stops it) to cast it now.
 */
#define RVR_SPELL_CASTABLE (1 << 1)

/**
 * A power (once a day) or a lesser power: no magicka.
 */
#define RVR_SPELL_POWER (1 << 2)

/**
 * The player has it on a quick slot (a favourite).
 */
#define RVR_SPELL_FAVOURITE (1 << 3)

#define RVR_COMBAT_EVENT_SLOTS 128

/**
 * A melee hit: `actor` and its `part` (the game's index), struck by `object` (0: a fist) with a
 * damager of `kind`; `strength` 0..1+ (1: a full-strength hit), `speed` the impact speed (m/s);
 * `point` where, `dir` which way the blade was moving.
 */
#define RVR_CE_HIT 1

/**
 * The player's weapon or shield (`object`; 0 a hand) met `actor`'s attacking weapon: a block, or
 * with `CEF_PARRY` a parry (the player's weapon moving into it).
 */
#define RVR_CE_BLOCK 2

/**
 * A spell cast: `spell` from `hand` (0 left, 1 right), from `point` along `dir`; `actor` the one a
 * touch spell touched (or the target aimed at, 0: none); `strength` how charged (0..1).
 */
#define RVR_CE_CAST 3

/**
 * A swing: `object` in `hand` swung at least as fast as a full hit (`strength` its peak, 1 a full
 * hit; with `CEF_POWER` as fast as a power hit), `swing` which way, over once it slowed; `actor` the
 * first creature it hit (0: it hit nothing). The game's own attack: its fatigue, and its swing when
 * it missed.
 */
#define RVR_CE_SWING 4

/**
 * A hit with an edge or point at a bad angle: it's a glancing blow (the flat), weak and blunt.
 */
#define RVR_CEF_GLANCING (1 << 0)

/**
 * A stab that went in.
 */
#define RVR_CEF_PENETRATED (1 << 1)

/**
 * A thrown weapon's hit.
 */
#define RVR_CEF_THROWN (1 << 2)

/**
 * Hard enough to stagger (the game's own "power attack").
 */
#define RVR_CEF_POWER (1 << 3)

#define RVR_CEF_PARRY (1 << 4)

/**
 * From behind the creature.
 */
#define RVR_CEF_BACK (1 << 5)

/**
 * A hit on its guard (its shield or weapon, `part` that part) while it holds its guard up: the
 * game's own block decides what it does.
 */
#define RVR_CEF_GUARD (1 << 6)

/**
 * Which way a swing went ([`CombatEvent::swing`]), in the player's own frame (the way the body
 * faces): the game picks its kind of attack by it.
 */
#define RVR_SWING_NONE 0

/**
 * Down: a chop from above.
 */
#define RVR_SWING_DOWN 1

/**
 * Up: a rising cut.
 */
#define RVR_SWING_UP 2

/**
 * Across to the player's left, and to the right.
 */
#define RVR_SWING_LEFT 3

#define RVR_SWING_RIGHT 4

/**
 * Forward along the blade: a thrust.
 */
#define RVR_SWING_THRUST 5



#define RVR_MAX_HUD_ELEMENTS 32

/**
 * Spoken lines: a caption strip low in view (13 degrees under gaze), lazily following the head.
 */
#define RVR_HUD_SUBTITLES 1

/**
 * Hints and tutorial prompts: a row just under gaze (6 degrees), lazily following the head.
 */
#define RVR_HUD_HINTS 2

/**
 * The ammo counter: on the held gun's left side, facing the eye.
 */
#define RVR_HUD_AMMO 3

/**
 * Status (health, filters, item counts): on the back of the off-hand wrist.
 */
#define RVR_HUD_STATUS 4

/**
 * Full-screen effects (damage, fades, a gas mask's glass): fixed to the face, covering the view.
 */
#define RVR_HUD_OVERLAY 5

/**
 * Button prompts and notifications ("A Continue", "+ammo"): the caption strip's right end.
 */
#define RVR_HUD_PROMPTS 6

/**
 * Things the game puts at the screen centre (use prompts, markers): at gaze, a little below.
 */
#define RVR_HUD_CENTER 7

/**
 * Not drawn at all (e.g. the crosshair).
 */
#define RVR_HUD_HIDDEN 8

/**
 * What the game says about the thing in a hand (its own panel of a thing's name and numbers): over
 * the thing a hand holds or pulls, turned to the eyes, fading with it; with nothing in a hand,
 * where it is on the flat screen. A game with this piece has its own card for what's held, so the
 * runtime draws none of its own ([`crate::ItemInfos`]).
 */
#define RVR_HUD_HELD 9

/**
 * Don't draw what no element covers.
 */
#define RVR_HUDL_REST_HIDDEN (1 << 0)

/**
 * The HUD texture's colour is added to the view (it has no usable alpha): every element is
 * blended additively.
 */
#define RVR_HUDL_ADDITIVE (1 << 1)

#define RVR_MAX_ITEM_INFOS 48

#define RVR_MAX_ITEM_STATS 6

/**
 * Taking it is stealing: someone else owns it (the game's red hand).
 */
#define RVR_ITEM_STOLEN (1 << 0)

#define RVR_ITEM_ENCHANTED (1 << 1)

/**
 * A quest needs it.
 */
#define RVR_ITEM_QUEST (1 << 2)

/**
 * The [`SoundPack`] region.
 */
#define RVR_OFF_SOUNDS 4886528

/**
 * Longest pack folder path, in UTF-8 bytes (with its NUL).
 */
#define RVR_SOUND_PATH_BYTES 512

/**
 * Longest sound set name, in UTF-8 bytes (with its NUL).
 */
#define RVR_SOUND_SET_BYTES 32

#define RVR_MAX_SOUND_OBJECTS RVR_MAX_OBJECTS

/**
 * The game plays the gunshots of the shots it fires (the shot, its tail and the casings): the
 * runtime plays none, from the pack or its own.
 */
#define RVR_SND_GAME_SHOTS (1 << 0)

/**
 * The game plays its own reloading and handling sounds (magazine out and in, the bolt, a dry
 * fire): the runtime plays none of those.
 */
#define RVR_SND_GAME_HANDLING (1 << 1)

#define RVR_OFF_BODY 65536

#define RVR_OFF_OBJECTS 69632

#define RVR_MAX_OBJECTS 256

#define RVR_OFF_OBJECT_STATES 196608

#define RVR_OFF_HAND_POSES 229376

#define RVR_MAX_HAND_POSES 64

#define RVR_OFF_COLLISION 262144

/**
 * Collision ring: `u64` head (producer) at +0x00, `u64` tail (consumer) at +0x40, bytes at +0x80.
 */
#define RVR_COLLISION_RING_BYTES 4194304

#define RVR_COL_RING_DATA_OFF 128

#define RVR_COL_RING_DATA_BYTES (RVR_COLLISION_RING_BYTES - RVR_COL_RING_DATA_OFF)

#define RVR_OFF_KICKS (RVR_OFF_COLLISION + RVR_COLLISION_RING_BYTES)

#define RVR_MAX_KICKS 256

/**
 * `KickDesc::object` for the gun the game holds in legacy mode (no objects registered): its
 * kicks are in the `WeaponRig`'s weapon space.
 */
#define RVR_KICK_OBJECT_HELD UINT32_MAX

#define RVR_JOINT_COUNT 22

#define RVR_HAND_TRACKED (1 << 0)

/**
 * The physics hand is held back by the world: also show a ghost hand at the controller.
 */
#define RVR_HAND_BLOCKED (1 << 1)

#define RVR_HAND_HOLDING (1 << 2)

#define RVR_BODY_VALID (1 << 0)

#define RVR_BODYVIS_HANDS 0

#define RVR_BODYVIS_ARMS 1

#define RVR_BODYVIS_FULL 2

#define RVR_OBJ_PROP 1

#define RVR_OBJ_WEAPON 2

#define RVR_OBJ_MAGAZINE 3

#define RVR_OBJ_THROWABLE 4

#define RVR_OBJ_MELEE 5

/**
 * Something a valve works, drawn by the runtime ([`OBJF_RUNTIME_DRAWS`]): a gate a wheel winds up.
 * It stands at its spawn pose (its model at `model_pos` / `model_rot`); its part on `turn_bone`
 * moves by `drive_travel` as the valve `driven_by` turns from 0 to `drive_turn`; with
 * [`OBJF_GAME_MOVED`], where the game's pose of it has that part whenever the game gives one (the
 * game moves it some itself). Not a body the hands meet: only drawn.
 */
#define RVR_OBJ_DRIVEN 7

/**
 * Anything turned by hand on an axle fixed in the world: a valve's wheel, a switch's knob, a lever.
 * Centre and axis (+Z) its spawn pose, the rim `radius` out, `mass` its weight, `friction` what
 * it takes to turn, the range it turns in and the clicks it holds in (`turn_min`, `turn_max`,
 * `detent`) and how a hand turns it (`joint_flags`) (`rig::valve`). Its state's `slide` is how
 * far it's turned: the game reads that to work its own mechanism.
 */
#define RVR_OBJ_VALVE 6

/**
 * Valves: a knob, turned by the hand's twist about its axis (a wheel is turned by the hand's place
 * on its rim). The runtime takes any valve smaller than a hand's grip as one.
 */
#define RVR_JOINT_TWIST (1 << 0)

#define RVR_OBJF_GRABBABLE (1 << 0)

/**
 * Gravity gloves can pull it.
 */
#define RVR_OBJF_PULLABLE (1 << 1)

#define RVR_OBJF_HOLSTERABLE (1 << 2)

#define RVR_OBJF_TWO_HANDED (1 << 3)

#define RVR_OBJF_SCOPED (1 << 4)

/**
 * Not placed in the world: the runtime spawns copies of it (magazines from the ammo pouch).
 */
#define RVR_OBJF_TEMPLATE (1 << 5)

/**
 * The runtime draws it (with the player's hands, lit by the game's [`crate::GameLighting`]) from
 * the model its `name` gives as `<game>/<asset key>` in the runtime's asset cache
 * (`metro2033/wpn_2012`); the game doesn't. A weapon is drawn with its moving parts where its
 * [`ObjectState`] puts them; a magazine (or a round) naming the same model as its gun's magazine
 * (or round) part, in place (magazine space is the gun's). A prop ([`OBJ_PROP`]) the game draws
 * is the runtime's to draw once a hand has had it ([`OS_RUNTIME_DRAWN`]), its model at
 * [`ObjectDesc::model_pos`] / `model_rot`: whole, one piece. A valve ([`OBJ_VALVE`]) is the
 * runtime's to draw always (the game hides its own): its model in two pieces, the part on
 * [`ObjectDesc::turn_bone`] turned by how far it's turned.
 */
#define RVR_OBJF_RUNTIME_DRAWS (1 << 6)

/**
 * Weapons: the game keeps this gun's ammunition (Metro's magazine and reserve), so the runtime's
 * rounds follow the game's instead of being unlimited. The gun arrives (and comes back with each
 * `spawn_seq`) holding `rounds` in all, not a full magazine: a self-loading gun chambers one of
 * them and its seated magazine holds the rest (0: chamber and magazine empty). A magazine the hand
 * takes from the pouch holds its template's `rounds`: what the game's own reload would load now,
 * less a live round already in the gun's chamber (0: the pouch is empty). The game rewrites both
 * as its ammunition changes.
 */
#define RVR_OBJF_GAME_AMMO (1 << 7)

/**
 * The game simulates this object (one of its own physics props) and draws it: while it's free the
 * runtime keeps it where the game says ([`GamePoses`]); hands and fingers going through it push
 * it instead ([`GamePushes`], for the game to apply to its body); and it only moves it while a
 * hand holds or pulls it (the state says so: `OS_HELD_*`, `OS_PULLED`; the game puts its
 * own body there). Let go, it flies on in the runtime for [`GAME_HANDBACK`] seconds, its state
 * carrying the throw's velocity for the game to take it back with, then follows the game again.
 */
#define RVR_OBJF_GAME_SIMULATED (1 << 8)

/**
 * It hangs on a ball joint from the point [`ObjectDesc::handle`] (object space), fixed in the
 * world where that point is when the object arrives: a cable, a lamp on a chain.
 */
#define RVR_OBJF_HANGING (1 << 9)

/**
 * With [`OBJF_GAME_SIMULATED`]: the game moves it itself (a kinematic body: a character, a door,
 * a cable it animates), so nobody takes or pushes it; it's followed and shown.
 */
#define RVR_OBJF_GAME_MOVED (1 << 10)

/**
 * Something to take (one of the game's things lying about): let go of in any of the body's slots
 * (whatever's there, however big it is), it's stowed: [`OS_STOWED`], out of the hands and not
 * drawn, for the game to take into the player's inventory (and drop from the table).
 */
#define RVR_OBJF_LOOT (1 << 11)

/**
 * Seconds a let-go [`OBJF_GAME_SIMULATED`] object stays the runtime's before following the game.
 */
#define RVR_GAME_HANDBACK 0.15

/**
 * Where the game's own simulated objects ([`OBJF_GAME_SIMULATED`]) are, every frame (world space).
 */
#define RVR_OFF_GAME_POSES 5021696

/**
 * Where the hands push the game's own simulated objects this frame (runtime -> game).
 */
#define RVR_OFF_GAME_PUSHES 5034112

#define RVR_MAX_GAME_PUSHES 32

#define RVR_SHAPE_BOX 0

#define RVR_SHAPE_CAPSULE 1

#define RVR_SHAPE_SPHERE 2

/**
 * The convex hulls of the object's points in [`GameHulls`] (object space): the game's own convex
 * collision, exactly, one hull or several (its convex parts: a gun's receiver, barrel and stock).
 * Until its points arrive, the box `half_extents` about `center`.
 */
#define RVR_SHAPE_HULL 3

/**
 * The points of game objects' convex collision ([`SHAPE_HULL`]), game -> runtime, written
 * whenever the objects offered change (before they're published). An object with several convex
 * parts has an entry for each.
 */
#define RVR_OFF_GAME_HULLS 5046272

#define RVR_MAX_GAME_HULLS 240

#define RVR_MAX_HULL_POINTS 32

#define RVR_WPN_AUTOMATIC (1 << 0)

/**
 * The slide or bolt locks back after the last round.
 */
#define RVR_WPN_SLIDE_LOCKS (1 << 1)

/**
 * A fresh magazine needs the gun racked when the chamber is empty.
 */
#define RVR_WPN_NEEDS_RACK (1 << 2)

#define RVR_WPN_PISTOL (1 << 3)

/**
 * Shoulder-fired: the virtual stock may anchor it.
 */
#define RVR_WPN_STOCK (1 << 4)

/**
 * An air gun: its shots spend pressure (`shot_pressure`) that strokes of its pump (the handle)
 * build (`pump_pressure`); below `min_pressure` it doesn't fire. It starts empty. The runtime
 * reports the pressure in [`ObjectState::fuse`]; a shot's strength is the pressure it was fired
 * at (the value reported before `shots` went up).
 */
#define RVR_WPN_PNEUMATIC (1 << 5)

/**
 * Loaded round by round straight into its `capacity` chambers (a double barrel's two, a single
 * shot's one): no magazine. Its ammo template is one round (an [`OBJ_MAGAZINE`] of capacity 1)
 * that goes into an empty chamber when its base is brought to the chamber's mouth (see
 * [`ObjectDesc::chamber_step`]); it fires the chambers in order, and their spent cases stay in
 * them until the action is opened.
 */
#define RVR_WPN_LOADS_CHAMBERS (1 << 6)

/**
 * Its magazine stays in it (a tube, a cylinder, a dart cluster: `capacity` rounds) and is loaded
 * one round at a time through its port at `mag_well`: its ammo template is one round, which goes in
 * when its base is brought to the port pointing along weapon +Z. It arrives full.
 * [`ObjectState::rounds`] is how many the magazine holds.
 */
#define RVR_WPN_LOADS_ROUNDS (1 << 7)

/**
 * A laser is fitted and on: the runtime draws its beam from [`ObjectDesc::laser`] (the game's
 * own one is off).
 */
#define RVR_WPN_LASER (1 << 8)

/**
 * [`ObjectDesc::action`]: it cycles itself (self-loading, semi or full auto): the default.
 */
#define RVR_ACTION_SELF_LOADING 0

/**
 * A bolt action: after every shot the bolt's handle (`handle`) is lifted (`action_angle`), pulled
 * back (`handle_travel`: the case flies out) and pushed forward and down again (chambering the
 * next round) before it fires again.
 */
#define RVR_ACTION_BOLT 1

/**
 * A pump action: the forend (`handle`) is pulled back and pushed forward after every shot.
 */
#define RVR_ACTION_PUMP 2

/**
 * A break action: the barrels tip down about a hinge (`handle`, weapon +X) by `action_angle` when
 * unlatched (B or A on the gun hand, a flick down, or the other hand on the barrels' foregrip
 * pulling them), falling open under their weight; opened, spent cases fly out and the chambers can
 * be loaded; they shut with a flick up, under their weight with the muzzle down, or pushed by the
 * other hand, and latch. It fires only shut.
 */
#define RVR_ACTION_BREAK 3

/**
 * A swing-out action: the barrel or cylinder swings out sideways about a pin along the bore
 * (`handle`, weapon +Z) by `action_angle`, otherwise as a break action. A magazine-fed one (a
 * revolver's cylinder and its speed loader) takes and drops its magazine only while it's out.
 */
#define RVR_ACTION_SWING 4

/**
 * [`ObjectState::chambered`]: bits 0..15 are the chambers holding a live round, bits 16..31 the
 * ones holding a spent case (chamber `k` is bit `k`; a gun with one chamber uses bit 0).
 */
#define RVR_CHAMBER_SPENT_SHIFT 16

#define RVR_CHAMBER_LIVE_MASK 65535

/**
 * [`ObjectDesc::throw_flags`]: a grenade. The runtime arms it, times its fuse (or waits for its
 * impact) and tells the game when it goes off ([`ObjectState::shots`], [`OS_SPENT`]); the game
 * does the explosion.
 */
#define RVR_THROW_GRENADE (1 << 0)

/**
 * It has a pin (at `handle`) the other hand pulls. Without one, a button on the holding hand arms it.
 */
#define RVR_THROW_PIN (1 << 1)

/**
 * A lever (spoon) holds the fuse until the grenade is let go of (or cooked): the fuse starts then,
 * not when the pin comes out.
 */
#define RVR_THROW_LEVER (1 << 2)

/**
 * Goes off on a hard impact once armed and thrown (after `impact_delay`), rather than on a timer
 * (or as well as one, if `fuse` is set).
 */
#define RVR_THROW_IMPACT (1 << 3)

/**
 * The game times the fuse: the runtime only reports it armed, and the game removes or respawns it.
 */
#define RVR_THROW_GAME_FUSE (1 << 4)

/**
 * A throwing knife: flies blade first (the blade from `grip` to the tip at `muzzle`) and sticks
 * in what it hits tip first ([`OS_STUCK`]).
 */
#define RVR_THROW_KNIFE (1 << 5)

/**
 * Sticks to whatever it hits once thrown (sticky bombs).
 */
#define RVR_THROW_STICKY (1 << 6)

/**
 * The runtime is moving it: draw it at `pose`.
 */
#define RVR_OS_SIMULATED (1 << 0)

/**
 * `pose` is in tracking space (held or holstered): map it with the adapter's `View`.
 */
#define RVR_OS_TRACKING (1 << 1)

#define RVR_OS_HELD_LEFT (1 << 2)

#define RVR_OS_HELD_RIGHT (1 << 3)

#define RVR_OS_HOLSTERED (1 << 4)

/**
 * A magazine seated in `parent`.
 */
#define RVR_OS_IN_WEAPON (1 << 5)

/**
 * Flying to a hand (gravity gloves).
 */
#define RVR_OS_PULLED (1 << 6)

/**
 * Highlighted: a hand is about to grab it (or pull it).
 */
#define RVR_OS_HOVERED (1 << 7)

/**
 * Spawned by the runtime from a template (e.g. a magazine from the ammo pouch).
 */
#define RVR_OS_SPAWNED (1 << 8)

/**
 * The slide / bolt is locked back.
 */
#define RVR_OS_SLIDE_LOCKED (1 << 9)

/**
 * Grenades: the fuse is burning (the game may show it lit, and its AI flee it).
 */
#define RVR_OS_ARMED (1 << 10)

/**
 * Grenades: the pin is out (draw it without one).
 */
#define RVR_OS_PIN_PULLED (1 << 11)

/**
 * Thrown, and still in the air or rolling.
 */
#define RVR_OS_THROWN (1 << 12)

/**
 * Stuck in something (a knife's blade, a sticky bomb): `parent` is what (0: the world).
 */
#define RVR_OS_STUCK (1 << 13)

/**
 * Went off: inert, the game hides it until it respawns it (`spawn_seq`) or removes it.
 */
#define RVR_OS_SPENT (1 << 14)

/**
 * Weapons: the action is open and won't fire: a break or swing action unlatched, a bolt lifted or
 * back, a pump back.
 */
#define RVR_OS_ACTION_OPEN (1 << 15)

/**
 * A prop with [`OBJF_RUNTIME_DRAWS`] a hand has had (holds, pulls or once did): the runtime draws
 * it, so the game hides its own. (A game may not draw a thing moved from where its level put it:
 * Metro files its clutter under the part of the level it starts in.) If the flag goes, the
 * runtime goes on drawing it for [`RUNTIME_DRAW_TAIL`] seconds while the game shows its own again.
 */
#define RVR_OS_RUNTIME_DRAWN (1 << 16)

/**
 * See [`OS_RUNTIME_DRAWN`].
 */
#define RVR_RUNTIME_DRAW_TAIL 0.15

/**
 * A thing to take ([`OBJF_LOOT`]) put in a body slot (`slot`): the game takes it.
 */
#define RVR_OS_STOWED (1 << 17)

#define RVR_SLOT_HIP_RIGHT 0

#define RVR_SLOT_HIP_LEFT 1

/**
 * Over the right / left shoulder, behind the head (long guns).
 */
#define RVR_SLOT_BACK_RIGHT 2

#define RVR_SLOT_BACK_LEFT 3

/**
 * Grenade pouches either side of the sternum, and a belt pouch on the weapon hand's side (small
 * throwables: grenades, knives).
 */
#define RVR_SLOT_CHEST_LEFT 4

#define RVR_SLOT_CHEST_RIGHT 5

#define RVR_SLOT_BELT 6

#define RVR_SLOT_COUNT_BODY 7

/**
 * The index finger rests on a trigger and curls to `index_pulled` as it's pulled.
 */
#define RVR_HPOSE_TRIGGER (1 << 0)

#define RVR_KICK_KEYS 16

/**
 * Muzzle up (rad). The offset turns yaw, then pitch, then roll: `Ry(yaw) * Rx(-pitch) * Rz(roll)`
 * about the pivot, then moves by the translation.
 */
#define RVR_KICK_PITCH 0

/**
 * About +Y (rad): positive turns the muzzle (+Z) towards +X.
 */
#define RVR_KICK_YAW 1

/**
 * About +Z, the barrel (rad).
 */
#define RVR_KICK_ROLL 2

/**
 * Translation (m), object space.
 */
#define RVR_KICK_X 3

#define RVR_KICK_Y 4

#define RVR_KICK_Z 5

#define RVR_KICK_CHANNELS 6

/**
 * The game's view kick (on a flat screen, the camera's jolt, which takes the gun with it). A
 * shot's view kick adds to the ones still playing.
 */
#define RVR_KICK_LAYER_VIEW 0

/**
 * The gun model's own kick (an animation): a shot's replaces the one playing.
 */
#define RVR_KICK_LAYER_GUN 1

#define RVR_KICK_LAYERS 2

/**
 * [`KickDesc::when`]: fired from the hip, or aimed (shouldered or down the sights) ...
 */
#define RVR_KICK_HIP (1 << 0)

#define RVR_KICK_AIMED (1 << 1)

/**
 * ... standing or crouched ...
 */
#define RVR_KICK_STANDING (1 << 2)

#define RVR_KICK_CROUCHED (1 << 3)

/**
 * ... and the first shot of a burst, the next few, or a long burst's later ones.
 */
#define RVR_KICK_FIRST (1 << 4)

#define RVR_KICK_FOLLOW (1 << 5)

#define RVR_KICK_LONG (1 << 6)

#define RVR_COL_PAD 0

/**
 * Drop all static geometry (level change).
 */
#define RVR_COL_CLEAR 1

/**
 * [`ColMesh`] + `count` x [`ColTriangle`]: replaces mesh `id` (0 triangles removes it).
 */
#define RVR_COL_MESH 2

/**
 * [`ColBox`]: replaces (or adds) box `id`.
 */
#define RVR_COL_BOX 3

/**
 * [`ColMesh`] + `count` bytes: each triangle's material (`MAT_*`, [`crate::combat`]), in the order
 * of mesh `id`'s triangles as last sent (after its [`COL_MESH`]). What a blade can bite into.
 */
#define RVR_COL_MESH_MATERIALS 4

/**
 * The region (after the HUD layout's place before it grew the mapping).
 */
#define RVR_OFF_GAME_LIGHTING 4915200

#define RVR_GAME_LIGHTS_MAX 32

/**
 * The colour-grading table's edge (`LUT_SIZE`³ RGBA8 texels, red fastest).
 */
#define RVR_LUT_SIZE 16

/**
 * The light atlas: one tile per light with a texture term, [`LIGHT_TILE`] texels square in view
 * space x and y over [`LIGHT_TILE_EXTENT`] metres around the view axis, [`LIGHT_TILE_SLICES`]
 * slices in view-space depth over 0..[`GameLighting::depth_squash`]·[`LIGHT_TILE_DEPTH`],
 * side by side. Texel (rgb) = the light map's colour, (a) = the shadow term (1 = lit).
 *
 * Exactly (T = `LIGHT_TILE`, S = `LIGHT_TILE_SLICES`, E = `LIGHT_TILE_EXTENT`): texel
 * (s·T + i, tile·T + j) holds the game's terms at the view-space point
 * x = −E + (i + ½)·2E/T, y = E − (j + ½)·2E/T (row 0 is the top, +y), z = s/(S − 1)·`depth_squash`
 * ·`LIGHT_TILE_DEPTH`: the squashed depth of a point at real view depth s/(S − 1)·`LIGHT_TILE_DEPTH`
 * (slice 0 at the camera, the last at `LIGHT_TILE_DEPTH`). Sample a slice bilinearly with u clamped
 * to its own texel centres, and blend the two slices around the point's depth linearly.
 */
#define RVR_LIGHT_TILE 64

#define RVR_LIGHT_TILE_SLICES 4

#define RVR_LIGHT_TILE_EXTENT 1.2

/**
 * The depth the slices cover before squashing (metres of real view depth).
 */
#define RVR_LIGHT_TILE_DEPTH 1.0

#define RVR_LIGHT_ATLAS_TILES 16

#define RVR_LIGHT_ATLAS_W (RVR_LIGHT_TILE * RVR_LIGHT_TILE_SLICES)

#define RVR_LIGHT_ATLAS_H (RVR_LIGHT_TILE * RVR_LIGHT_ATLAS_TILES)

/**
 * Omni light.
 */
#define RVR_GL_OMNI 0

/**
 * Light with a projected light map.
 */
#define RVR_GL_PROJECTED 1

/**
 * Light with a projected light map, attenuated by the surface's occlusion (`att *= |w|`).
 */
#define RVR_GL_PROJECTED_OCCLUDED 2

/**
 * Shadowed spot with a light map.
 */
#define RVR_GL_SHADOWED 3

/**
 * Shadowed spot without a light map.
 */
#define RVR_GL_SHADOWED_PLAIN 4

/**
 * "Ambient" omni: no N·L, no specular.
 */
#define RVR_GL_AMBIENT 5

/**
 * Metro 2033 Redux's deferred shading (everything above).
 */
#define RVR_LM_METRO 0

/**
 * Gamebryo's forward per-pixel lighting, as Oblivion's `ShadowLightShader` and `SkinShader` do it
 * (the Oblivion adapter's docs/research/oblivion-lighting.md): in the display's 0..1 range, each
 * light's diffuse capped at 1 and attenuated, summed with the ambient and capped at 1, lighting the
 * base texture (times the vertex colour); each light's specular (the normal map's alpha) added;
 * then the fog. The terms used:
 *
 * * [`GameLighting::ambient`]: the ambient light (rgb);
 * * [`GameLighting::lights`]: [`GL_GB_DIRECTIONAL`] (`pos.xyz` the direction towards it) and
 *   [`GL_GB_POINT`] (`pos.xyz` where it is, `pos.w` = 1/R², R its radius in metres), `color` the
 *   light's colour as the game's shaders get it (its diffuse times its dimmer and fades);
 * * [`GameLighting::fog_color`] and [`GameLighting::fog_fade`] (amount = saturate(distance·y + x)).
 *
 * Its `eye` is the identity: positions and directions are in tracking space. The rest is unused.
 */
#define RVR_LM_GAMEBRYO 1

/**
 * Gamebryo: a directional light (the sun, or an interior's), `pos.xyz` towards it.
 */
#define RVR_GL_GB_DIRECTIONAL 16

/**
 * Gamebryo: a point light, attenuated as `saturate(1 − d²/R²)`.
 */
#define RVR_GL_GB_POINT 17

/**
 * The light reaches the first-person model (its volume was shaded with depth test Always: the
 * game's "mode A"). Others never light it.
 */
#define RVR_GLF_REACHES_VIEWMODEL (1 << 0)

/**
 * [`GameLight::tile`] holds this light's texture term in the light atlas.
 */
#define RVR_GLF_TILE (1 << 1)

#define RVR_OFF_PLAYER_DRAW 4980736

/**
 * Objects drawn with the player at once (a gun, its magazines).
 */
#define RVR_DRAWN_OBJECTS_MAX 6

/**
 * Moving parts per drawn object (a gun's body, trigger, bolt, rounds, glass...).
 */
#define RVR_DRAWN_PARTS_MAX 128

/**
 * [`RtPlayerDraw::flags`]: a posed player is there.
 */
#define RVR_PD_VALID (1 << 0)

/**
 * [`DrawnHand::flags`]: the hand is drawn in a game's own pose for what it holds.
 */
#define RVR_DH_HELD (1 << 0)

/**
 * [`DrawnHand::index_pulled`] holds the index finger at a full trigger pull.
 */
#define RVR_DH_INDEX_PULLED (1 << 1)

/**
 * [`DrawnHand::index_reach_pulled`] is given.
 */
#define RVR_DH_INDEX_REACH_PULLED (1 << 2)

#define RVR_SHOTS 0

#define RVR_HITS 1

#define RVR_RELOADS 2

#define RVR_WEAPON 3

#define RVR_AMMO 4

#define RVR_CROUCHED 5

#define RVR_JUMPS 6

#define RVR_GESTURE_FACE 7

#define RVR_GESTURE_HEAD_TOP 8

#define RVR_GESTURE_CHEST 9

#define RVR_GESTURE_SHOULDER 10

#define RVR_GESTURE_HIP 11

#define RVR_MENU_TOGGLES 12

#define RVR_MENU_CLICKS 13

#define RVR_THROWS 14

#define RVR_SWITCHES 15

#define RVR_MELEES 16

#define RVR_ABILITY_UP 17

#define RVR_ABILITY_DOWN 18

#define RVR_ABILITY_LEFT 19

#define RVR_ABILITY_RIGHT 20

#define RVR_USES 21

/**
 * Physical mode: objects the runtime reports.
 */
#define RVR_OBJECTS 22

/**
 * Physical mode: 1 + the index of the gun in a hand (0 = none).
 */
#define RVR_HELD 23

#define RVR_F_FEET_X 0

#define RVR_F_FEET_Z 1

/**
 * Distance (m) from the last hit to the target's centre.
 */
#define RVR_F_LAST_HIT_ERROR 2

#define RVR_F_LOOK_YAW 3

/**
 * Angle (degrees) between the last shot and the runtime's aim direction.
 */
#define RVR_F_SHOT_AIM_ERROR 4

#define RVR_F_FRAME_MS 5

#define RVR_F_SCOPE_FRAMES 6

/**
 * Rounds in a full magazine of the held weapon.
 */
#define RVR_F_MAG_SIZE 7

/**
 * Physical mode: grenades that went off, the target plates their blasts reached, and knives
 * stuck in a target plate.
 */
#define RVR_F_EXPLOSIONS 8

#define RVR_F_BLAST_HITS 9

#define RVR_F_KNIFE_HITS 10

/**
 * Visual effects alive (muzzle flashes, smoke, tracers, bullet holes, casings, debris).
 */
#define RVR_F_FX_LIVE 11

/**
 * The runtime's melee hits on the dummy, those that were power hits, its casts, and those cast
 * at the dummy (`range/src/combat.rs`).
 */
#define RVR_F_MELEE_HITS 12

#define RVR_F_MELEE_POWER 13

#define RVR_F_CASTS 14

#define RVR_F_CASTS_AT_DUMMY 15

enum RVR_TexKind
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  RVR_TexKind_EyeLeft = 0,
  RVR_TexKind_EyeRight = 1,
  /**
   * `R32_FLOAT` linear view depth in metres (or the encoding in [`SlotInfo::depth_flags`]).
   */
  RVR_TexKind_DepthLeft = 2,
  RVR_TexKind_DepthRight = 3,
  /**
   * Premultiplied-alpha 2D layer: the game's HUD, or its whole picture while a menu is open.
   */
  RVR_TexKind_Hud = 4,
  /**
   * Extra camera view, e.g. a magnified scope picture.
   */
  RVR_TexKind_Aux = 5,
  /**
   * The light atlas: the game's light-map and shadow terms per light (see [`lighting`]).
   */
  RVR_TexKind_Lights = 6,
  /**
   * The game's bloom (and the lens's warp in x) for each eye (see [`TGT_BLOOM`]).
   */
  RVR_TexKind_BloomLeft = 7,
  RVR_TexKind_BloomRight = 8,
  /**
   * The lens's light (and its warp in y) for each eye (see [`TGT_BLOOM`]).
   */
  RVR_TexKind_LensLeft = 9,
  RVR_TexKind_LensRight = 10,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum RVR_TexKind RVR_TexKind;
#else
typedef uint32_t RVR_TexKind;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

enum RVR_Joint
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  RVR_Joint_Hips = 0,
  RVR_Joint_Spine = 1,
  RVR_Joint_Chest = 2,
  RVR_Joint_UpperChest = 3,
  RVR_Joint_Neck = 4,
  RVR_Joint_Head = 5,
  RVR_Joint_LeftShoulder = 6,
  RVR_Joint_LeftUpperArm = 7,
  RVR_Joint_LeftLowerArm = 8,
  RVR_Joint_LeftHand = 9,
  RVR_Joint_RightShoulder = 10,
  RVR_Joint_RightUpperArm = 11,
  RVR_Joint_RightLowerArm = 12,
  RVR_Joint_RightHand = 13,
  RVR_Joint_LeftUpperLeg = 14,
  RVR_Joint_LeftLowerLeg = 15,
  RVR_Joint_LeftFoot = 16,
  RVR_Joint_LeftToes = 17,
  RVR_Joint_RightUpperLeg = 18,
  RVR_Joint_RightLowerLeg = 19,
  RVR_Joint_RightFoot = 20,
  RVR_Joint_RightToes = 21,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum RVR_Joint RVR_Joint;
#else
typedef uint32_t RVR_Joint;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

#define RVR_TexKind_ALL { RVR_TexKind_EyeLeft, RVR_TexKind_EyeRight, RVR_TexKind_DepthLeft, RVR_TexKind_DepthRight, RVR_TexKind_Hud, RVR_TexKind_Aux, RVR_TexKind_Lights, RVR_TexKind_BloomLeft, RVR_TexKind_BloomRight, RVR_TexKind_LensLeft, RVR_TexKind_LensRight, }









#define RVR_Joint_ALL { RVR_Joint_Hips, RVR_Joint_Spine, RVR_Joint_Chest, RVR_Joint_UpperChest, RVR_Joint_Neck, RVR_Joint_Head, RVR_Joint_LeftShoulder, RVR_Joint_LeftUpperArm, RVR_Joint_LeftLowerArm, RVR_Joint_LeftHand, RVR_Joint_RightShoulder, RVR_Joint_RightUpperArm, RVR_Joint_RightLowerArm, RVR_Joint_RightHand, RVR_Joint_LeftUpperLeg, RVR_Joint_LeftLowerLeg, RVR_Joint_LeftFoot, RVR_Joint_LeftToes, RVR_Joint_RightUpperLeg, RVR_Joint_RightLowerLeg, RVR_Joint_RightFoot, RVR_Joint_RightToes, }

#endif  /* RUSTVR_PROTOCOL_H */
