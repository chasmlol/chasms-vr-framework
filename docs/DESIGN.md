# RustVR — Design Doc

> One VR player experience (hands, guns, aiming, gestures, comfort, menus) that connects into any
> flat game through a small per-game adapter, instead of every VR mod rebuilding VR inside its game.

Status: draft v0.2 · 2026-10-21

---

## 1. Core principle

**The runtime owns the player's body; the game owns the world.**

- **The runtime** (`rustvr.exe`, its own process) owns the headset, both hands, what they hold,
  the aiming maths, the gestures, turning and comfort, the menus and compositing.
- **The game** keeps its world, collision, damage, ammo, AI and saves.
- **An adapter** (a DLL inside the game, or the test range) only *translates*:
  - it maps tracking space into its world;
  - it renders the eyes it's asked for;
  - it reports what the game is doing;
  - it turns the runtime's intents into the game's own input.

If an adapter re-implements a VR mechanic, or the runtime re-implements a game rule, the design has
gone wrong. It's the same rule as SkyCraft (`skycraft\docs\DESIGN.md` §1), with Minecraft's player
replaced by a VR body.

## 2. Components

```
headset ◀─ OpenXR ─┐
                   │  rustvr.exe (crates/runtime)
                   │    backend/openxr.rs   headset, controllers, swapchains, haptics
                   │    backend/sim.rs      simulated headset for headless tests
                   │    mechanics.rs        the body -> intents (ported from the Metro mod)
                   │    rig/                the player rig: physics hands, objects, guns (Rapier)
                   │    panels.rs, menu.rs  HUD / menu panels, laser, egui settings menu
                   │    compositor.rs       game eyes + runtime layers -> headset
                   │    session.rs          the frame loop
                   │    scenario.rs         scripted end-to-end tests
                   │
                   │  shared memory "Local\RustVR_v14" (crates/protocol)
                   │  + D3D11 shared textures (NT handles, keyed mutex) + named events
                   │
                   │  game process
                   │    crates/adapter      the adapter SDK: connect, tracking -> world, slots
                   │    crates/range        test range (a fake game)
                   │    crates/metro        Metro 2033 Redux (dinput8.dll proxy)
```

Supporting crates:

- `crates/gpu`: D3D11 device, shared textures, shader compile, PNG readback, egui painter.
- `crates/asset`: the cache format for models and textures extracted from installed games.
- `crates/audio`: the runtime's interaction sounds and a small spatial mixer on WASAPI. A game's
  own sounds come first: its **sound pack** (§6c), decoded in the background; a procedural library
  (24 sounds, four variations each, no sample files) is the fallback.
- `crates/ik`: full-body IK from the head and hands (torso, spine, arms with elbow prediction,
  stepping legs), calibration and grip-to-hand offsets.
- `crates/avatar`: draws the player's body and hands (and what they hold), hidden behind the
  game's geometry using its depth: over the game's picture in the runtime, or, SkyCraft's way,
  inside the game's own frame by an adapter that sets `GS_DRAWS_BODY` (the runtime posts the posed
  player as `RtPlayerDraw`; Metro's adapter draws it into Metro's HDR scene before its post chain).
  Every joint keeps its volume (skinned about optimised centres of rotation), and a game's armour
  is made of rigid plates where its marks say ([ARMOUR.md](ARMOUR.md)).
- Metro's `metro-assets` (in the chasms-vr-metro repository) reads Metro 2033 Redux's archives
  (`metro-extract`): models, textures, recoil, and its sound pack (`sounds.bin`, §6c).
  Local test content only, never committed.
- `content/rigs/<game>.toml`: how the hands hold each of a game's guns, measured on its models
  (ours, so committed; `rustvr_asset::rigs` reads them). [RIGGING.md](RIGGING.md) says how to rig
  a gun.
- `content/armour/<game>.toml` (in the game adapter's repository): which parts of the game's armour
  are rigid plates and what each rides (ours, hand-written per piece; `rustvr_asset::armour` reads
  them). [ARMOUR.md](ARMOUR.md) says how to mark a game's armour.

## 3. Spaces

Everything in the protocol is **left-handed metres, X right, Y up, Z forward** (DirectX, Unity, 4A).
Quaternions are `[x, y, z, w]`; matrices are column-major (glam).

| Space | What | Who uses it |
|---|---|---|
| Tracking | the headset's standing space, floor at Y = 0 | the runtime, for everything |
| World | the game's world, in metres with canonical axes (`Basis` converts game units and axes) | the adapter |
| Weapon | a gun's model space, barrel +Z | the adapter describes the gun as a `WeaponRig`; the runtime answers with `weapon_to_tracking` |

OpenXR is right-handed with -Z forward. Its backend mirrors Z at the boundary: positions become
(x, y, -z) and quaternions (-x, -y, z, w). This is tested in `backend/openxr.rs`.

**Tracking -> world lives in the adapter SDK** (`crates/adapter/src/tracking.rs`, ported from the Metro
mod's camera code). It covers recentring, turning, vertical anchor smoothing, scripted-camera
following and roomscale absorption, and it must use the game's anchor *for the frame being
rendered*. If the runtime did it a frame late, the view would jitter while walking. The runtime only
sends the controls (`TrackingControl`: recentre counter, total turn, world scale, height
compensation, follow flag), and the adapter reports back what it applied (`TrackingState`).

## 4. Protocol

One file mapping (4.7 MB), created by the runtime and opened by adapters. Its layout is in
`crates/protocol/src/lib.rs`; `include/rustvr_protocol.h` is generated from it for C/C++/C#
adapters (`tests/header.rs` keeps the two in step).

| Region | Direction | Access |
|---|---|---|
| `Header`: magic, version, PIDs, heartbeats, connect epochs | both | atomics |
| `RtFrame`: predicted poses, controllers, `TrackingControl`, `Intents` | runtime -> adapter | seqlock |
| `GameState`: flags, tracking state, eye height, zoom, `WeaponRig`, texture request, stats | adapter -> runtime | seqlock |
| `Targets`: the shared textures' size, format and generation | runtime -> adapter | seqlock |
| `SubmitCtl` + 3 x `SlotInfo`: the texture triple buffer | adapter -> runtime | xchg |
| `Event` ring: haptics, game rumble, recentre requests, the game's blasts and bullet pushes, the shots it fired | adapter -> runtime | SPSC ring |
| `RtBody`: the player's 22 joints, finger curls, hand states | runtime -> adapter | seqlock |
| `ObjectTable`: the objects the player can handle (guns, magazines, grenades, knives, bows and arrows, props, templates), and how a gun is worked (`action`: self-loading, bolt, pump, break, swing; loaded into its chambers; air) | adapter -> runtime | seqlock |
| `ObjectStates`: where each object is, who holds it, rounds, shots fired, what's in a gun's chambers and where its action is (a slide, a bolt's lift, open barrels, an air gun's pressure), a grenade's fuse and when it went off | runtime -> adapter | seqlock |
| Collision ring: static collision (boxes, triangle meshes) for the runtime's physics | adapter -> runtime | SPSC byte ring |
| `HandPoseTable`: the game's own hands on its guns (every finger joint) | adapter -> runtime | seqlock |
| `KickTable`: the guns' recoil as the game authored it (per-shot kicks, keyframed) | adapter -> runtime | seqlock |
| `SoundPack`: where the game's sound pack is, each object's sound set, which sounds the game plays itself | adapter -> runtime | seqlock |
| `HudLayout`: which rectangle of the HUD texture is what (subtitles, hints, ammo, status...) | adapter -> runtime | seqlock |
| `Casters`: each hand's casting (readied, charging, ready), what, how charged, where its art goes (the palm, or a held staff's tip) and which way it would go, for the game's own casting art | runtime -> adapter | seqlock |
| `Overlays`: the game's effect shaders over what the runtime draws (an enchanted weapon's shimmer, a spell's shader on the player's body) | adapter -> runtime | seqlock |
| `ActorBodies`: what grabbing the creatures needs beside their capsules (each part's rotation, speed and mass, the body's mass, whether it's all physics, the player's strength) | adapter -> runtime | seqlock |
| `CreatureGrabs`: each hand's grab of a creature (the point held in its part's frame, where the hand would have it, the pull, the squeeze on a throat, the throw when let go) | runtime -> adapter | seqlock |
| `Ride` / `RtRide`: the mounts near the player and the one ridden (its saddle, gait, where the rider sits, the reins) / what the rider does with it (gait, pace, steering, getting on and off) | both | seqlock |
| `Inventory`: the player's things as the game keeps them (its categories, each thing's values, how it's used, its model), the game's own slots, what the game took back; `BodySlots`: the body's slots as set up and what each gives now; `SlotEdits`: the game's editor's changes | both | seqlock |
| `Pockets`: the game's creatures' pockets (on their bones), what's in them, how aware each creature is of the player, and the game's answers to what the hands did | adapter -> runtime | seqlock |
| `PocketEvents`: a creature's pickpocketing opening and closing, a thing pulled out (the attempt), planted, kept or dropped | runtime -> adapter | ring |
| `Receptacles`: which objects are bowls the hands make things in (their shape, how much they hold, what may go in now, what grinds in them) | adapter -> runtime | seqlock |
| `Cards`: cards of the game's own interface (its menu art cut out, text in its bitmap fonts) to draw over its objects, or as the face of a page (`OBJF_SHEET`) | adapter -> runtime | seqlock |

- **Shared memory** is only touched through 32-bit atomics, so concurrent access is defined
  behaviour.
- **Connect handshake:** the adapter bumps `adapter_epoch`; the runtime resets the triple buffer
  and acks the epoch. A runtime restart resets the ack, so adapters reconnect by themselves.
- **Actions** are press *counters*, not pulses, so a game that skips frames never misses a press.
- **Textures** are created by the runtime (typeless 8-bit colour; adapters view them as UNORM like a
  back buffer, the runtime as sRGB), named `<mapping>_g<generation>_<kind>_<slot>`, and opened by
  name. Kinds are left/right eye, left/right depth, HUD and aux (scope).

## 5. A frame

1. The backend predicts the next display time and poses (`xrWaitFrame`, or the sim clock).
2. Mechanics turn the body into `Intents` and `TrackingControl`.
3. The runtime writes the `RtFrame` and signals `<mapping>_frame`.
4. The adapter maps tracking space into its world with its current anchor, drives the game, and
   renders the eyes (plus the HUD, and the scope view if asked) into its back slot. It records the
   poses it actually used in that slot's `SlotInfo`, publishes the slot and signals
   `<mapping>_submit`.
5. The runtime takes the newest slot and draws its layers over it with **the same poses** (lens,
   panels, laser). It submits those poses to OpenXR, so the headset's reprojection corrects for
   any lateness.

   When the slot brings linear depth and where each eye was in the world (`SlotInfo::eye_map`),
   the runtime instead moves the picture itself (`crates/runtime/src/warp.rs`, setting
   `vr.reproject`): each eye is laid over a grid, pushed out to its depth, carried from that eye's
   world mapping to the game's mapping now and drawn from this headset frame's eyes. The headset
   only turns a picture for the head, so without this the world steps whenever the game walks or
   turns the player between its pictures, while the hands (drawn every headset frame) don't.

   "The game's mapping now" is itself carried on to when the frame shows
   (`crates/runtime/src/locomotion.rs`): the adapter stamps each state with when the game's clock
   stood where it has the player (`GameState::time_qpc`, QueryPerformanceCounter ticks; the
   OpenXR backend converts each frame's display time to the same clock), and the walk and turn
   between the game's last two steps go on from there. The game steps at its own uneven pace (Metro
   once an eye pair), so without this the world stands still for a frame and then jumps, however
   smoothly the picture is moved. A step that disagrees with what was being shown (a stop, a
   hitch) is glided out over 60 ms rather than jumped. The rig uses the same mapping, so the hands
   reach into the world where the player sees it. What a hand holds goes with the walk and turn as
   a velocity (`rig::physics::Drive::carry`): the world can stop it, its weight only resists the
   hand's own swing, and if the world holds it more than 10 cm behind the hand it goes through
   until it's back in the hand and clear (`rig::SLIP`), so it's never dragged along and snapped back.

   A game that draws the player inside its own frame (`GS_DRAWS_BODY`, so its fog, bloom, tone
   curve and grading reach it) says which posed player each pair was drawn with
   (`SlotInfo::player_frame`). The runtime poses the player it sends for when the picture will
   show (`in_frame::predict`), keeps every pose it sent, and at each headset frame
   (`crates/runtime/src/in_frame.rs`): finds the player's pixels in each eye (the sent pose drawn
   as depth from that eye), moves the world without them (what the other eye saw behind them, or
   filled from around), and draws the player where it is now with its own copy of the same meshes,
   every point in the colour the game's picture has it in (`Avatar::draw_reprojected`). The
   player's pixels stay the game's; they just follow the controllers at the headset's rate. The
   adapter also writes the player's depth into the game's depth buffer (`Avatar::draw_game_depth`,
   with the game's own projection), so the game's later passes (Metro's volumetric light,
   particles, glass) stop at the player instead of shining through it from the wall behind; the
   runtime fills the world's depth behind the player from around for the warp.

In tests the runtime runs in **lockstep** (`RT_LOCKSTEP`): it waits for the adapter's frame every
time, and the adapter steps exactly once per headset frame using its `dt`, so runs are
deterministic. With a real headset the game runs at its own pace and the newest slot wins.

Games that can only draw one eye per engine frame (the Metro mod's alternate-eye rendering)
fill eye L and eye R of the same slot on consecutive engine frames and publish after the second.
`SlotInfo` carries per-eye poses, so both eyes are still submitted with the poses they were
rendered with.

## 6. Mechanics

`crates/runtime/src/mechanics.rs` is a port of the Metro 2033 VR mod's `player.cpp` (`Update` and
`MenuControls`). That mod collected its features from Titanfall 2 VR, the feel reference. All of it
works in tracking space:

| Feature | Behaviour |
|---|---|
| Gun in hand | The rig's grip sits on the palm and the barrel follows the aim ray; per-game offsets come from the settings |
| Two-handed | The off hand grips near the foregrip (25 cm reach), and the gun pivots around the firing hand so its foregrip follows the off hand |
| Recoil | The game's own (`rig/kick.rs`, as on the rig): the adapter registers its held gun's kicks for `KICK_OBJECT_HELD` in the `WeaponRig`'s weapon space and says when the game fires (`EV_GAME_SHOTS`, from the game's own round count); each shot plays a kick on the gun, so bursts climb and you pull it back down. One-handed it kicks `recoil_one_hand` times harder |
| Line of sight | Shots start at the sight and travel parallel to the barrel. Adapters can report the exact sight line (`RIG_SIGHT_LINE`) |
| Auto aim-down-sights | Fires when the sight line passes within 6 cm of an eye while you look along it, or when the grip is held |
| Scopes | A camera just past the muzzle renders into a lens behind the optic, at true magnification from the game's own zoom |
| Movement | Head- or off-hand-relative stick, sprint toggle, snap (30°) or smooth (120°/s) turning, weapon switch on the stick |
| Roomscale | While the stick is idle, the game body walks after the head (starts at 20 cm, stops at 5 cm). The view stays put because the adapter absorbs the motion |
| Up against things | A game's body is often wider than the player's (Oblivion's is 30 cm round), so it stops the view well short of a table. The game's adapter, which knows its body and its world, tells the mapper how far the view stands ahead of the body each frame (`set_lead`). Oblivion's gives it whatever of the step asked the body didn't go: up to a table until the player's front is at it, otherwise a head's breadth from a wall, and inside its body for what isn't the world's (a door, someone). Walking, the view moves on; following, the head's way from the body shortens instead |
| Physical crouch | Standing height is calibrated as you play (and held while you crouch); crouching a little (12 cm by default, the Comfort tab's "Stealth crouch amount") crouches the game, its sneak, and the view stays at your real head height |
| Height | Your upright head height (standing, or sat up in a chair) is the game's eye height, whenever the view was recentred: the runtime keeps that height (it follows the head until the headset is worn and still; held higher a second it rises there, lower it settles slowly; a recentre takes it as it is) and takes the recentre's own height back off (`height_offset`). The body is drawn from the same height, so it stands on the game's floor |
| Manual reload (default) | Titanfall 2 VR style: firing-hand A ejects the magazine, the off hand grabs a fresh one at the hip or chest (squeeze), seats it in the well (8 cm), then racks the charging handle (pull it back 5 cm). Letting go drops the magazine. The game gets one Reload press, at the rack. Intent flags let the game hide the empty magazine and show the one in your hand |
| Slap reload | The simpler option, and the fallback for guns without a magazine well: the off hand goes into the well (12 cm), then a squeeze or an upward slap faster than 0.9 m/s reloads |
| Melee | A hand striking forward faster than 2.8 m/s, with a 0.5 s cooldown; not the firing hand while it's aiming |
| Ability flicks | Titanfall 2 VR's titan abilities, for any game: hold the off-hand grip away from the body and flick up, down, left or right (2 m/s) |
| Gestures | Squeezing the grip at the face, top of the head, chest, over the shoulder or at the hip |
| Watch | The off-hand wrist turned up in front of the eyes |
| Menus | The game's menu becomes a world-locked panel with a laser cursor; the runtime's settings panel floats beside it |

Each mechanic has unit tests (`cargo test -p rustvr-runtime`) plus a headless scenario that drives
it through the real protocol against the test range (§8).

**The Metro mod is a reference for features, not code to copy.** It was built by asking for
Titanfall 2 VR's features and is known to be buggy, so behaviour is re-derived and tested rather
than transliterated. Where the port differs on purpose, the code says why.

## 6b. The player rig

When a game registers objects (`ObjectTable`) and streams its static collision, the runtime's
**rig** (`crates/runtime/src/rig/`) owns the hands and everything they do. It's built from the
research in `reports/VR player controller design.md`, whose rule is **pin the hands, cheat
everything else**: hands are exactly where the controllers are in free space, and everything else
(grabs, magazines, throws, the body) bends in the player's favour.

Physics is Rapier, in game-world metres, using the `TrackingState` mapping the adapter reports.
Held and holstered objects are reported back in tracking space (`OS_TRACKING`) so they never trail
the hands while walking.

| Mechanic | Behaviour |
|---|---|
| Physics hands | Light hands driven by velocity, each one's collider its palm (a box from the wrist to the knuckles, as thick as a hand, where the drawn palm is) and its five fingers curled as the controller has them (`rig/fingers.rs`): 1:1 in free space, stopped by walls, tables and things (nothing goes through them, nor through what they hold), pushing loose objects. An empty hand is sent only as far towards its controller as it can get (`Physics::reach`): turned as the controller is, put out of anything solid it's in, then swept there, stopping on the world and on what the other hand holds and sliding on along them. It's never driven into them, so a controller pushed on through a wall leaves the hand resting on its face, following the controller along it as calmly as the controller moves. Held back, it never jumps to catch up (slid off an edge, round a corner, off something loose it was pressed on): it keeps up with the controller's own movement and makes up the rest eased (settled in about a tenth of a second), swept again so it goes round what's in the way. Its fingers aren't stiff: where the palm alone could go, each gives way to what it'd be pressed on, pushed open (a relaxed finger back past straight, a clenched one hardly) or curled in, whichever is nearer. Fingers ease there and back (over about 0.06 s, at most from straight to a fist in a fifth of a second), never snapping, and the hand goes only as far as its fingers as they are let it: they hold it back until they've given way, so pressing harder never pushes them into anything. A palm pressed on a wall comes flat onto it with the fingers and thumb laid along it. The hand's body never touches the world itself (only loose things and creatures), so the solver never fights the sweep; pressed on something, it turns with the controller a touch behind (0.06 s), so the controller's tremor doesn't jolt it. The drawn fingers show the same (`RigOutput::finger_give`). Stuck on the wrong side of something (held back 5 cm or more, getting no nearer, its controller clear through on the far side), it finds the way round, as the robots' "bug" algorithms do. It tries 12 ways along the surface with its palm, a few a frame, each a step at a time out to 40 cm, until a straight way to the controller opens, then goes round the shortest way. It slides along the edge, rolling the wrist over it if it's caught, and heads straight on once clear. With no way round within reach (a wall, the floor), it rests where it is and looks again half a second later. Loose things it's held back by that won't move (a gun on a table) are solid to it while it goes round them, and it presses on them gently, never hard enough to be pushed through. A hand that lets go of what the other hand still holds comes out of it rather than being knocked off it. A ghost hand shows the controller when one is held back; past 0.7 m they snap back |
| Grabbing | The palm or a finger on it (within 3 cm), grip hysteresis 0.6 press / 0.4 release (optional toggle). Anything is held just where the hand took it: neither the hand nor the thing moves, and the fingers close on it from where they are (`rig/grips.rs`). Near one of its handles it goes into the hand the way it's made to be held: a gun's pistol grip (10 cm) locks it on, grip-to-palm, barrel where the hand points (0.12 s ease), the only hold the trigger fires from; a handle (`ObjectDesc::grip_line`: a sword's grip, a haft, a knife's handle; 7 cm) takes the hand anywhere along it, held as `grip_rot` holds it (or reversed, the fist the other way along it). Drawn from a holster or caught, a gun or rigged weapon comes by its grip. A hand closing in slowly (under 0.8 m/s) on what it's about to take rests on it rather than knocking it away |
| Both hands | The other hand takes anything a hand holds: on its grip when the holding hand isn't (it becomes the holding hand, the other holds on where it is: a gun carried by the barrel, then taken by its grip), on the handle a second hand takes (a gun's handguard, `ObjectDesc::foregrip_line`, anywhere along it; a long handle), or anywhere on it. Two hands aim it, pivoting on the holding hand's hold. Let go of either and the other has it, where it has it (hand to hand). A hand on what the other holds beats the pouches and holsters behind it |
| Rigged guns | A gun's descriptor says how the hand holds it (`grip_rot`: the controller grip pose in weapon space) and where the support hand goes (a foregrip line with a slide range, and `support_rot`). Rigged guns point where the hand does; unrigged ones follow the aim ray. Left hands use the mirror image. The rigs are data, measured on each game's models (`content/rigs/<game>.toml`, [RIGGING.md](RIGGING.md)): the line of sight through the gun's own sights, the game's own hands, the parts that move, the game's rate of fire, fire mode, magazine and caliber |
| Weight | Held objects can only speed up and turn so hard, less the heavier they are (the weight setting, squared): a 3.5 kg rifle about 30 m/s^2 and 60 rad/s^2 in one hand (a quarter turn takes a third of a second), two to three times that in two hands; pistols nearly and bottles fully keep up. Both brake into the hand rather than overshoot, and soften to 5% against the world, so they never jitter through walls. Heavy things also throw slower (a rifle about 80% of the hand's speed) |
| Two-handed | The support hand takes the handguard within 7 cm of it, anywhere along it (the gun's support hold, slid along, and the game's support pose with it), or the gun anywhere else; it stays where it took hold, drawn on the gun, and steers the barrel about the firing hand (steadied by a One-Euro filter); pulling it 30 cm away lets go. Letting go of the gun hand leaves the gun in the support hand, where it has it. Optional virtual stock: shouldered rifles aim along the shoulder-to-support-hand line |
| Holsters | Both hips (13 cm) and both shoulders (28 cm, from above the shoulder down behind it, the hand behind the eyes), on a body anchor that lags the head yaw by 0.5 s and freezes while a hand reaches into a zone. The anchor hangs from the eyes as they'd be looking level over the neck (the IK body's neck pivot), so looking down at a hip doesn't swing the zones forward and down with the eyes. A zone counts a hand at its middle or deeper, up to 12 cm in toward the body's middle line, so a hand pressed against the body at a slot is in it; a hand at a zone's middle is in it first, else a hand only deeper is in the zone it's furthest into. Let go of a gun gently in a slot to holster it (a swing faster than 1.2 m/s throws instead); a tick tells you you're in one. Auto-return of let-go guns is optional (off). Pouches for small throwables, torso-anchored as the research recommends: either side of the sternum (8.5 cm, clear of the magazine pouch in its middle) and on the belt front on the weapon hand's side; only grenades and knives fit them, and never an armed grenade |
| Slots of the player's things | `rig/pockets.rs`, `holsters.rs`: slots on the body (on the holsters' anchor, scaled with the player), each a place, a size (let go within it, a thing goes back in) and a reach (taken from that much further out), drawing from the game's inventory (`Inventory`) by its categories in order and a rule (most healing first, magicka, fatigue, most valuable, heaviest, lightest, newest, by name, most of, strongest, or the player's own: what they last put back comes out first), the worn first or not, and what an empty slot does (a fainter tick, nothing, or the rule's first of everything). A grip in one takes its first thing as the runtime's copy (`OS_INVENTORY`, the game's model in the fist: upright, or a long thing along the fist). Held at the mouth (within 9 cm of a point under the nose) for half a second, with bites or gulps felt on the way, it's eaten or drunk; held up before the eyes, read; touched to the weapon in the other hand, put on it: each once, for the game to do (`shots`); never while the hand moves fast or is held back. Let go in any slot it goes back; anywhere else it falls for the game to drop where it rests. The game says what it took back (used, dropped), and the copy goes. A weapon the game equips from a copy arrives in that hand (`ObjectDesc::replaces`). A grip held still in a slot asks for the game's editor; a slot is placed by the next squeeze when the editor asks. While the editor is open every slot shows as a sphere of light the size of its reach (`compositor::Bubble`, over the drawn body), the picked one bright, with its first thing still at its place. Kept per game in the settings (`[[holsters.games.<game>]]`), the game's own until then |
| Magazines | Unlimited: grip at the chest or belt front (or any empty slot) while the other hand holds a magazine-fed gun. Firing-hand A drops the magazine, straight into the other hand if it's squeezing under the gun |
| Reloading | Guns keep their own magazine, chamber and slide state. A magazine within 9 cm and 45° of the well seats itself (strict mode 4 cm / 25°), but not the one just taken out until it has left the well; pull the slide or charging handle back 90% of its travel to chamber; B or slamming a magazine drops a locked slide |
| Guns worked by hand | `rig/actions.rs`, the way Half-Life: Alyx, H3VR and Pavlov do them; the descriptor says which (`ObjectDesc::action`, `WPN_LOADS_CHAMBERS`, `WPN_PNEUMATIC`). **Break actions** (the Duplet): B (or A) is the lever, a flick of the wrist muzzle-down (6 rad/s) or the support hand on the barrels' foregrip pulling them works too; unlatched, the barrels fall open under their own weight, a pendulum on the gun hanging where gravity puts it (the stock lifted to them swings them shut); past 75° the spent cases fly out (live rounds stay); a flick up (5 rad/s, read from the controller since a heavy gun lags the wrist) throws them shut, or the hand pushes them home, and they latch. **Swing-outs** (the Ashot's barrel, a revolver's cylinder on its crane) are the same about a pin along the bore; a revolver takes and drops its speed loader only with the cylinder out, and it swings out with it. Guns **loaded round by round** fire their chambers in order (one barrel a pull), the cases staying in them; the pouch hands one round, which goes into an open chamber when its base comes within 5 cm of the mouth pointing along the bore (strict mode 2.5 cm); a live one can be pulled back out. A **magazine that stays in the gun** (the Uboynicheg's cylinder, the Helsing's darts: `WPN_LOADS_ROUNDS`) takes its rounds the same way, one at a time through its port, at any time. **Bolt actions** (the Preved, the Ventil) and pumps fire once and wait: the knob follows the other hand along its L-shaped path in weapon space (turned up about the bore before it slides, turned down only once it's home), the case flies out at the back of the stroke, the next round goes in on the way home, and it stays where it's let go of. **Air guns** (the Tihar, the Helsing) start empty: each whole stroke of the pump adds pressure, each shot spends some, below the minimum the trigger hisses, and a shot's strength is the pressure it fired at. The range draws it all from the reported state (barrels and chambers turned on the hinge, the bolt turned then slid, a pump lever swung to match the pull, the gauge's needle between the game's own empty and full poses); every step is a cue |
| Shooting | Semi or full auto from the gun's state; each shot increments `ObjectState::shots` for the game to fire. **The game's own recoil** when the adapter gives it (`KickTable`, `rig/kick.rs`): each shot plays one of the gun's view kicks and one of its gun kicks, picked for how it was fired (hip or aimed, standing or crouched, first shot / follow-up / long burst). View kicks overlap and add up, so a burst holds the muzzle up and shakes it and it settles when you stop; a gun kick replaces the one playing. The head never moves: on a flat screen the camera's turn takes the gun with it, here it turns the gun about the eye. Metro's come from its camera tracks and fire motions (see `metro-assets/src/recoil.rs`; Kalash: 2.5 degrees and 16 mm a hip shot, about 3 degrees held through aimed full auto). The game's numbers are for its player's hold, a long gun in both hands at the shoulder: off the shoulder it kicks 1.7 times as hard, in one hand 3 times (twice with the stock at the shoulder); a pistol in two hands 0.75 times (the push grows half as much as the turn). The shoulder is generous: the back of the gun within 20 cm of it is fully braced, fading out by 38 cm. Hands tab: on/off, turn and push scales (1 = the game's), the one-handed and off-the-shoulder scales. **Otherwise** recoil follows Escape from Tarkov's (`rig/recoil.rs`): a fast snap and push back, then EFT's hand-rotation process. Each kick leans a random angle off vertical, the first shot of a burst hits hardest, and the return spring is weak while firing, so full auto climbs; from the 4th shot it plateaus and wanders (stable mode). When you stop it returns on a curved, sideways path and settles a little up and off (the after-recoil offset), so you pull it back on target; the offset fades over a few seconds. Per-gun character: `recoil_climb`, `recoil_return`, `recoil_angle`. Two hands give 55% of the rotation and a faster return; a shouldered stock 40%. Fixed 240 Hz springs with seeded randomness |
| Throwing | Half-Life: Alyx's estimator (`rig/throwing.rs`, from `research_notes/VR throwables and UI/throwables.md`): 0.3 s of controller history per hand in tracking space (the runtime's own velocity where it gives one, so snap turns don't pollute it), the fastest three frames in the 0.12 s before the release averaged, their direction from a line through the positions around them, locomotion added at the release. So a hand that slows before letting go, or a late grip release, still throws at its peak, and the object leaves from where it was then (the Valve patent's backdating, at most 0.15 s, never through the world). A speed-dependent gain on top (Valve's curve: 1 below 1.5 m/s, 1.25 by 6 m/s, 1.35 for grenades since players undershoot), capped at 25 m/s (knives 30); under 1 m/s it's a drop (no gain, a quarter of the spin). Spin is 0.75 of the hand's, at most 20 rad/s; the wrist's spin adds `0.8 w x r` only past 3 rad/s (HVR). Mid-swing (over 1.5 m/s) the grip lets go of a throwable as it eases a quarter off its hardest squeeze (Deck's release), not only once it's nearly open; guns and magazines stay held. Hands leave a hard throw alone for 0.2 s |
| Grenades | `rig/throwable.rs`. The fuse is the game's (`ObjectDesc::fuse`), else 4 s. Two arming schemes (Hands tab): *pin* (default, Pavlov / H3VR / Into the Radius): the other hand takes the ring (8 cm) and pulls it 5 cm out; a lever holds the fuse while the grip does, so it starts when you let go, or press B to cook it in your hand; a grenade without a lever (Metro's lit fuse) starts as the pin comes out. *Button* (HL:A's priming button): the trigger or A on the hand holding it. One-handed anyway while the other hand holds a gun, or for a grenade without a pin. It ticks (sound and a faint pulse in the holding hand) every second, four times a second in the last 1.5 s; it can be re-gripped or caught, the fuse keeps burning; the game's pause stops it. When it runs out (impact grenades: on a hard hit once thrown) the game is told (`ObjectState::shots`, `OS_SPENT`) and makes the explosion; its blast pushes the rig's loose objects through `EV_BLAST` |
| Throwing knives | Blade first (Meta's dart, HVR's stab test): at the release, more than 8 rad/s about the blade's flat throws it end over end (that spin only, nudged so the tip comes round where it's going to hit); otherwise it's a dart, turned tip-first once and kept on its flight (16/s). It sticks if the tip meets something at 1 m/s or more within 60 degrees of its flight (72 with the assist, on by default); a spinning knife is judged in its tip-forward pose whenever its tip is round the front. Stuck (`OS_STUCK`), it rides with what it's in and pulls back out with a grip. Sticky bombs stick to whatever they hit |
| Bows | `rig/bow.rs`, from the best-liked VR bows (the research is in Oblivion's adapter, `docs/research/archery.md`). A bow (`OBJ_BOW`) is held by its grip, in either hand: that hand is the bow hand and the arrow rests on its side. While the other hand holds one, gripping over that hand's shoulder (behind the head) or at its hip (Hands tab: either or both) takes an arrow (`OBJ_ARROW` templates of the bow's ammo type; with the game's count, none when it has none), held along the fist. Brought within 9 cm of the string's nock point it goes on the string, eased on over 0.12 s, a click in both hands; the hand holding it holds the string. The string comes back with the hand along the bow's arrow line, the bow turning about its grip so the arrow points from the string hand through its rest (eased in over 0.15 s, then exact; the roll is the bow wrist's), to full draw: the bow's own, or less for a shorter arm (90% of the arm from the grip, the chin's anchor). Past it the hand stays on the string and its ghost shows the controller. The bow bends with it, between its model and the game's bent one, as far as the string is drawn of the bow's travel. A tick in both hands each centimetre (the Longbow's: faint at rest, five times as long at full draw), a low strain now and then at full draw. Let go past a fifth of full draw, the arrow is loosed exactly along the drawn arrow: `CE_LOOSE` gives the game its tip, its way and the draw, and the game fires its own arrow; the bow kicks in the bow hand, the string snaps in the other, and the limbs spring back as the game animates them (`Bows`), felt at each swing. Short of it the arrow stays on the string, to draw again or take off. The crosshair (Hands tab, off by default): the game's own image where the arrow would land, from the game's speed and gravity for the draw, traced against the world and the creatures |
| Gravity gloves | Half-Life: Alyx's, with its own numbers (`vr_hand_pull_*` / `vr_gg_*` from its `server.dll`, see `rig::alyx`): point the hand (its aim or its palm) at something within 10 m (a 15° cone from 25 cm behind the hand, held to 20°; nothing nearer than 30 cm); 0.1 s pointed at it takes it (it glows). Grip locks it on: it hops and strains towards you; pointing 60° away, or 0.25 s with the grip let go, drops the lock. A flick (the hand yanked back at 1.2 m/s, or the wrist flicked back at 5 rad/s) throws it on Alyx's arc: 0.6 s in flight out to 2.5 m, 0.8 s at 10 m, launched 20° above the line near and 10° far, gravity eased to as little as half; it steers after your moving hand from 0.6 s out to 0.05 s before arriving (up to 12.7 m/s^2). Caught within 0.5 m with the grip held (a 0.125 s pulse) |
| Feedback | Every interaction is one cue: a haptic pulse and a spatial sound on the same frame. Impacts scale with speed. Cues have event names (`grenade_pin`, `grenade_arm`, `grenade_tick`, `grenade_throw`, `explosion`, `knife_throw`, `knife_stick`; for the guns worked by hand `break_open`, `break_close`, `swing_open`, `swing_close`, `shell_in`, `shell_out`, `case_eject`, `bolt_up`, `bolt_back`, `bolt_forward`, `bolt_down`, `pump_back`, `pump_forward`, `pump`, `air_hiss` ...) for games' sound packs (the actions' play the pack's nearest handling event, `bolt_back` and the like, until the audio crate has events by their names); until one gives them their own they use the nearest synthesized sound | A cue also names its object (the gun, for its magazine, bolt and shots), so the sound is that object's own from the game's sound pack (§6c)
| Hinges and slides | `rig/valve.rs` (`OBJ_VALVE`): anything worked on a hinge or a slide fixed in the world, simulated exactly as one degree of freedom (it can't come off its hinge or be flung off). A valve's wheel, a switch's knob, a lever; a door's leaf, a lid, a drawer, a button, a pull chain (`JOINT_SLIDE`). The hand takes it where it is: on its rim, on the runtime-drawn part that turns, or, a part the game draws, anywhere on its collider (the game's own shape of it), nothing moving. It's cranked by the arm's force-capped spring at that point (600 N/m, at most 180 N: the leverage is the arm's, near a door's hinge it hardly moves it, at its edge it swings it) or, within 6 cm of its axle, twisted by the wrist (at most 6 N m), against its inertia (a ring's, or its collider's own about the axle: a door is a slab turned about its edge), its friction (static and sliding), its range and the clicks at its ends, its weight (`JOINT_GRAVITY`: a lid falls shut), a spring back to where it rests and damping (`joint_spring`, `joint_damping`). A part that swings through the room (`JOINT_PANEL`, a door's leaf) is a wall to the empty hands and what they hold, and a hand pressed on it pushes it as far as its controller is past the hand (pushing, never pulling); the player's body (a capsule from the head down) and the game's creatures stop it. A hand on one lets go once its controller is 50 cm from its hold, and glides back. Coming hard against a stop is felt (`Cue::Stop`). The game may change its range and stiffness as it goes (a door's latch): the runtime takes them from the table, where it's turned to kept |
| Bowls and grinding | `rig/receptacle.rs`: an object the adapter says is a bowl (`Receptacle`: its floor, mouth, depth and wall in object space, how many things it holds, which objects may go in now) gets a bowl's collision (a floor and a ring of wall), so what the hands hold goes into it and meets it (a pestle grinds against its floor and walls). A thing that may go in, let go over the mouth (its middle within 4 cm of it, up to 30 cm over the rim; or falling in), lies on its floor beside the rest (alone in the middle, else round it), shrunk to fit if it's bigger than the bowl, and rides with the bowl however it's carried; turned over, the bowl spills it. A hand reaching into the bowl takes what's in it (only then: above the rim it takes the bowl or its grinder). The bowl's grinder (a pestle) rests in it where the game's model has it until a hand takes it, and goes back when let go with its tip over the mouth. Its tip pressed low in the bowl (within 2.5 cm of the floor beyond its own size) and worked about grinds what's in it: every metre the hand takes it along the floor (the hand's motion, not the tip's: pressed in, the tip shivers on the floor; a vertical stroke counts half; at most 0.8 m/s counts) grinds each thing `1 / grind_work` of the way, shared among them; what's ground is drawn smaller (to 45%). The hand feels a low buzz while it grinds, stronger the faster, and a pulse each 9 cm stroke; the game sounds the strokes (the bowl's state counts them). Things in a bowl are `OS_IN_RECEPTACLE` (`fuse` how ground, `slide` the size to draw), the bowl's own state the share ground (`slide`) and its strokes (`shots`) |
| Creatures' bodies | Every part of the game's creatures (`Actors`) is a kinematic capsule in the rig's physics, following the game: hands, what they hold and loose things are stopped by them, and push them (`GamePush` naming the part: the point must move at least as fast as it's pressed into, plus what the hand would have it go further). A blow is judged by the swing (where the hand drives the weapon, before the body stops it): the velocity it meets the body with. A stab or chop that goes in stays in: it only slides along its way in (out at most 0.9 m/s); what the hand pulls it across, or out faster, drags the creature by where it went in; it comes out back past where it went in (or yanked 50 cm across). The same into the world, whatever it's made of (wood when the game doesn't say: a table, a wall, the floor), where it doesn't move at all, and into loose things, which it drags |
| Magic | `combat.rs`: the off hand, empty, is the spell hand while the game has a spell ready; a held thing that casts (`OBJF_CASTS`, a staff) is a spell hand whatever hand holds it, cast from its tip. A finger on the trigger readies the spell, pulling it charges it over the game's casting time (pulses every 0.1 s down to every 0.02 s, strength 0.06 + 0.6 x charge^4, after Skyrim VR's ISPVR; a spike at full; a soft tick every 0.3 s held ready), letting go casts (a touch spell: touching a creature). Target spells go along the line from the shoulder through the palm (a staff's own length), bent towards a throw, onto a creature within 15 degrees. The game draws its own casting art where `Casters` says (`SPELLS_GAME_ART`); a game without one gets the runtime's glow in the palm. Upright things (`OBJF_UPRIGHT`, a torch) hang flame up at the hip and are only held that way up |
| Holding creatures | `rig/creatures.rs`, HIGGS's way: a hand whose grip closes on any part of a creature the game lets the hands take (`ActorBodies`) takes hold of it where it touched it, neither moving, the fingers closing round its capsule (and the ones beside it at the throat). The game moves the body with its own physics: each frame the runtime sends where the controller would have the point held, the hand's pull (the Hands tab's strength times the game's own strength for the player), how hard a throat is squeezed, and on letting go, the throw (the hand's speed at its peak, no gain). The hand is drawn on the part where the game has it, so the body's weight holds it back (a light arm follows closely, a body hangs and lags) and it never goes into the body; it feels the strain (a low rumble, harder the further it's held back); past an arm's reach (60 cm from the controller) the grip slips. Let go, it's a free hand again where it is, on the body's surface (held back by the body if its controller is in it, catching up eased). A fist closed on a creature doesn't punch it. A push on a creature says how fast the pusher itself came in (a shove, not a lean). The range's dummies take grabs with a stiff spring at the point (5000 N/m), go down when lifted by the head or torso and stay down while held |
| Pockets | `rig/pickpocket.rs`, for picking pockets by hand (a game's adapter's `docs/research/pickpocket.md` has the research). The game says where its creatures' pockets are (on their own bones, every frame) and what's in them (each thing a template, drawn from the game's model, and how it feels). An empty hand in one feels a soft brush and a tick for the thing it's on, each kind its own (coins, a key, glass, paper, cloth, something hard); every 4 cm it moves in the pocket it's on the next thing. Gripping takes a copy of that thing into the hand, still the creature's while it's in the pocket (let go there: back, nothing tried); pulled out past the pocket's edge (3 cm) the attempt goes to the game (`PE_TAKE`), and its answer keeps the thing (the player's: kept in a body slot, the game's own drop let go elsewhere, planted let go in a pocket) or takes it from the hand with a hard jolt. Anything to take (`OBJF_LOOT`) let go gently in a pocket is planting, for the game to say. A creature's pickpocketing opens as a hand goes into one of its pockets and closes once no hand has been near them for 0.75 s, so the game can run its own rules for both. A slow heartbeat in the hand while the pocket's creature sees the player (the game's own detection); a hand in a pocket pushes nothing of its creature, and its trigger doesn't use |
| Fingers | Curls from the trigger, grip and thumb-rest touch. Holding something (not in a game's own hand pose for it), each finger closes on it (`avatar/src/grasp.rs`): bent in from straight, every joint stops as its segment meets the surface while the joints past it curl on, so it wraps a handle and folds over an edge (a segment may come closer near its knuckle, which folds round the edge it's on); a finger that's in the thing straight (a thumb under a palm laid flat on a crate) first bends back at its base until it's out; the drawn hand's own finger geometry, the handle it's on (a capsule) or the thing's collision shape. It closes over 0.12 s as the hand takes hold and opens as it lets go; a gun held by its grip keeps the index on the trigger |
| Full body | `crates/ik` solves 22 joints every frame from the head and the hands the rig drew (VRIK/FRIK-style: torso yaw from the hands, spine pitch from the head drop, stepping legs, elbow prediction, hands pinned exactly). The avatar (Metro's Miller when extracted, else a mannequin) is drawn between the game's picture and the overlays, hidden behind the game's geometry using its linear depth; the head is cut away in first person. Hands only, arms or full body. The body stands on the game's floor (the game puts your eyes at its own eye height, so the real floor would leave it floating or crouched), its proportions follow your height (set, measured, or the game character's), and its arms follow your arm span: set, measured in a T-pose, or automatic (it grows whenever your real arms reach further). Seated mode draws it standing |
| Face | A game whose heads are FaceGen's (Oblivion, Fallout 3, New Vegas) describes the player's face (`FaceDesc`: its coordinates, detail map and hair colour); the parts with face morphs in the cache (the head and the parts made to it) are moved to it as the body is put together, and the head's texture morphs make the face's texture, laid over its skin as the game's shader does (`avatar/src/facegen.rs`). A slider moved in the game's character creator shows a few milliseconds later |
| Mirror | A standing mirror in the game's world (`runtime/src/mirror.rs`), called up from the Body tab or by pulling the gripped hands apart in front of the chest, or by the game (`GS_MIRROR`: its character creator, its menu beside the mirror with the world round it). Each eye's reflection is drawn every headset frame from that eye reflected in the glass: the game's world rendered once a frame from the head reflected (`MirrorView`, the aux texture and its depth), moved to each eye by its depth, then the whole body with its head, lit by the game's lights |
| Comfort | A vignette eases in while the stick moves or smooth-turns you (strength 0.5, 0.3 s); the view fades out when the head pushes into the game's collision; snap turn 45° (0.75 press, 0.25 re-arm, 0.5 s repeat); seated mode; play at your real height or the character's |
| Menu | Opens on a Help page with every control and feature; tabs for comfort, hands, body (height, arms, seated), controls, weapon and display |
| Riding | `riding.rs` (`rustvr_protocol::ride`): a free hand closing on a mount's saddle gets on (the game's own way); the reins are where the game has them, taken by the grip, measured from where each hand took its rein in the rider's frame: one pulled back or both moved aside steers (full at 12 cm), both back slows a gait (7 cm; held, it stops and backs), a flick of a rein hand (1.1 m/s down, stopped) or its trigger spurs on a gait; let go, the gait holds 2.5 s, then slows; the sticks ride too. A free hand's trigger held, or the pommel held while leaning 35 cm out, gets off. The body sits in the saddle (`BodySolver::ride`), legs where the game puts them. The mount's turn carries the view (`TrackingMapper::carry_turn`, smoothly or in steps); a vignette while it turns or gallops; a fade while the game gets the rider on or off; no walking the body after the head, the turning stick steers |

In rig mode the mechanics leave grabbing and gun buttons to the rig and map the rest the way most
VR shooters do: left stick move, right stick turn (flick down to crouch), A/X jump with an empty
hand, Y flashlight, empty-hand trigger use, hold B for the weapon wheel, menu to pause.

## 6c. Sound packs

The runtime plays a sound for every interaction, and **the game's own sounds come first**. A game
that has them hands the runtime a sound pack; the runtime's synthesised library (`crates/audio`) is
only the fallback, for events the pack doesn't have and until it has loaded.

- **The pack** is a folder with a `sounds.toml` and the files it names (Ogg Vorbis or WAV). It maps
  *events* (`grab`, `release`, `holster_in`/`out`, `mag_out`, `mag_in`, `bolt_back`,
  `bolt_forward`, `dry_fire`, `fire`, `fire_tail`, `casing`, `impact`, `pouch`, `zone_tick`,
  `ui_click`, `melee`, `grenade_pin`, `grenade_arm`, `grenade_tick`, `grenade_throw`, `explosion`,
  `knife_throw`, `knife_stick`, ...; `rustvr_audio::Event`) to sounds, in
  `[defaults]` and per **sound set** (`[sets.wpn_ak]`, one per gun, say; a set can `extends`
  another). An event is layers that play together (a shot's close crack and its body), each a
  choice of files picked at random, never the same twice running. A layer can be `spatial = false`
  (the unspatialised stereo bed: a shot's echo fills the head) or wait (`delay`: the casing lands
  half a second later). The format is documented in `crates/audio/src/pack.rs`.
- **The adapter publishes it** (`SoundPack` region, `Adapter::set_sound_pack`): the folder, each
  object's set, and what the game plays itself. `SND_GAME_SHOTS`: the game plays the gunshots of
  the shots it fires (Metro does), so the runtime plays no shot, tail or casing; `SND_GAME_HANDLING`:
  it plays its own reload sounds. Per object or for the whole pack. The test range plays nothing
  itself, so it sets neither and the runtime plays Metro's shots for it. The region is additive: an
  adapter that never writes it gets the runtime's own sounds.
- **The runtime** loads the pack on a background thread when the path (or its `revision`) changes,
  decoding and resampling everything to the mixer's rate, and unloads it when the game goes (each new
  connection starts with none, so one game's sounds never carry over to the next). The
  audio thread still never allocates, blocks or frees: a new pack arrives through a `try_lock`ed
  slot, the old one stays while its voices finish, and packs are freed on the game thread. Each
  cue then plays the object's set's event (its own, inherited, or the pack's defaults) or the
  synthesised sound. Settings: sound on/off, volume, gunshots, and "the game's own sounds".
- **Metro's pack** comes from `metro-extract` (`metro-assets/src/sound_pack.rs`), from the game's
  own data: `sounds.bin` (its sound items: files, random choices, numbered groups, two items
  layered) and each gun's config, which names its shot, echo, casings and dry fire. The Kalash's
  shot is `ak_close_shot_*` layered with `ak_boom_shot_*`, its tail `generic_ar_echo`, its casings
  `cc_545_concrete_*`. Reloads are Redux's split reloads (`weapons\split_reloads`: the AK's magazine
  out, magazine in and bolt), which suit a reload made by hand; a table there says which part is
  which. Grabs and holsters are the player's cloth and gear, a dropped gun Metro's weapon-fall
  sounds, the pouch its ammo pickup. **Throwables** have sets of their own, from their configs
  too (the `weapons\fast drop\...` configs that hold their first-person models): Metro's
  dynamite and sticky dynamite are lit with `bomb_prepare`, thrown with `bomb_atack_1` or `_2`
  and go off with `grenade_explosion`; the incendiary bomb goes off with `molotoff_explosion`;
  every explosion has the echo every Metro explosive's config names after it (`preved_echo`, in
  the bed like a shot's tail). The throwing knife flies with `weapons\dagger` and sinks in with
  `knife_hit_default` (what Metro's material table plays for knife hits); the range's own knife
  sounds like it. Pulling a bomb's fuse is a cord tugged (Metro's tripwire, `trap_check_pull`:
  Metro's hands strike a lighter instead). The range's frag (`[sets.grenade]`) counts down with
  the claymore's beep and throws and goes off with the defaults; its pin and lever, which Metro
  hasn't got, stay the runtime's own, and a lit fuse doesn't tick. Not used yet: the bombs' fuse
  burning (`bomb_idle`, `molotoff_fly2`), a loop that would need a looped event that follows the
  bomb. The table of every throwable's events is in `sound_pack.rs`. Events are levelled to where
  the runtime's own sounds sit, except shots, which share one gain so the guns keep their
  loudness relative to each other. `metro-extract --sound-items <substring>` lists `sounds.bin`'s
  items and what they play.

## 6d. The HUD in VR

A flat game's HUD is a full-screen layer. Hung in front of the face as one sheet it covers the view
and chases the head, which no well-liked VR game does (`research_notes/VR throwables and UI/ui.md`:
Half-Life: Alyx, the Titanfall 2 VR mod, UEVR, Doom 3 VR, Metro Awakening). So the adapter
describes its HUD (`HudLayout`: rectangles of the HUD texture and their roles) and the runtime
(`crates/runtime/src/hud.rs`) puts each piece where native VR games do:

| Role | Where |
|---|---|
| Subtitles | a caption strip 13 degrees under gaze at 1.5 m, at most 50 degrees wide |
| Hints | a row just above the subtitles |
| Prompts | the caption strip's right end |
| Ammo | on the held gun's left side, facing the eye; hidden with no gun |
| Status | over the inside of the off-hand wrist, shown while its palm is turned up (within about 40 degrees of straight up, fading out by 57) and you look at it; several stack upwards |
| Overlay | fixed to the face over the whole view (damage, fades) |
| Gaze | under where the head looks, eased after it (0.18 s), just in front of the thing it's about (`HudLayout::gaze`), as big to the eye at any distance (what looking there does: "Open Door", a name) |
| Compass | low in front of the body (26 degrees under the horizon), lazily following its turn |
| Target | over the foe it's about (its health) |
| Centre and the rest | the flat screen, 1.4 m ahead |
| Hidden | not drawn (a crosshair: the gun's sights aim) |

Everything that follows the head waits for a 20 degree turn, glides back in front (0.15 s) and
stops; it never rolls. The game stays the source of truth for *when* a piece shows: its rectangle
is drawn every frame, so it appears and fades as the game draws it. Without a layout the HUD is one
flat screen, as before; a layout that hides what it doesn't cover (`HUDL_REST_HIDDEN`) with nothing
in it shows nothing. Each picture says which layout it was drawn with (`SlotInfo::hud_layout`), and
the runtime cuts it by that one: a layout written as the picture is finished waits for its picture,
so no piece is ever cut from the frame before or after (a menu's pieces from the HUD, or the HUD's
from a menu). Metro's layout is in its adapter's `docs/METRO.md` (the chasms-vr-metro
repository).

**Menus in pieces.** A game's menu is never hung in front of the face whole: the adapter describes
it as groups of pieces (`HudGroup`: the parts that matter, cut out of its UI) and the runtime floats
each group where it belongs, world-locked once placed:

| Anchor | Where |
|---|---|
| a thing in the world (a chest, a merchant, a speaker) | at its distance (0.9 to 1.4 m), 10 degrees under the eyes, beside it only where it would cover it; or over or under it (a speaker's name, their line) |
| the player (their own menus) | 1.1 m ahead, 8 degrees under the eyes; it comes round after a turn of 65 degrees |
| a pop-up (a question, a count) | 0.8 m ahead where the eyes look |

A group's pieces keep the game's own arrangement, lie on a cylinder round the eye, and are sized
from the game's own text (`HudLayout::text_height`) so a line reads at `ui.menu_text_deg` (1.5
degrees) wherever it floats. With real alpha (`HUDL_ALPHA`) and the game's picture under the menu,
the world, the body and the hands stay, and the pieces are drawn in the world (before the body, so
the hands are in front of them), their cut edges soft. A piece the adapter says is a page
(`HUDE_SOLID`) is opaque wherever the game drew half or more of it, whatever coverage its blending
left in the HUD texture's alpha. The laser picks among the pieces that take it
(`HUDE_POINT`); where it hits is the game's cursor (`Intents::cursor_uv`), so the game keeps its
own hover, selection and clicks; the dominant stick scrolls (`Intents::scroll`, the game's mouse
wheel). The runtime's own settings float beside a game menu only when the layout asks
(`HUDL_SETTINGS`: a pause menu). A menu with no layout is one world-locked panel with the laser
(§6). Over the game's world, a menu whose pieces haven't come yet (the game says it's in a menu as
the frame that opens it starts; the pieces come with that frame's picture) shows nothing for up to
half a second rather than the flat screen.

**The laser** is a thread of light in a glow, fainter and fading out when it points at nothing, with
motes of light running out along it; where it hits, the light it casts and a dot with a dark edge
and a bright heart, pressed in while the trigger is held. Its colours are the game's own when the
layout gives them (`HudLayout::pointer`: its cursor's, its UI's), else the runtime's blue.

**Menus in the hands** (`ANCHOR_HELD`, `held_menus.rs`): a group can lie on a thing a hand holds
(`HudHeld`): a book's pages on the book's face turned to the eyes when it opened (`HELD_FACE`), or
a sheet held by its edge like paper and spread between both hands when the other takes its far side
(`HELD_SHEET`: a map). It follows the hands every frame, sized so the game's text reads at
`menu_text_deg` at `ui.held_distance` (0.45 m: nearer is bigger, as with a real book), and is drawn
with the body in its depth (`rustvr_avatar::Sheet`): a hand in front of a page covers it, the
fingers behind a sheet are hidden by it. The free hand turns pages by sweeping across them past the
middle, or pinching an edge and pulling it across (`HELD_PAGES`: `Action::PageNext`, `PagePrev`, a
tick in the hand), drags a sheet's content with its grip (`HELD_DRAG`: the game's cursor held down
where the hand is), and points with the laser (it moves to the free hand). Things that open
(`OBJF_OPENS`, a book) open and shut with the other hand's grip on them (their state's `shots`); a
kept thing (`OBJF_KEPT`, a map) goes back to its slot when let go. The hands keep going under such a
menu (the rig isn't paused, the game is).

## 6e. Cards of a game's own interface

A game can describe a small card of its own interface to show over one of its objects (`Cards`,
`crates/runtime/src/game_cards.rs`): the pieces of its own menu art (a texture from the asset cache,
the part of it, where on the card) and lines of text in its own bitmap font (`rustvr_asset::Font`:
each character's glyph in an atlas, laid along the line as the game lays text: the space before it,
its width, the space after, its top over the baseline; words wrapped at the piece's width), in its
colours. The runtime paints each card (egui, into a texture of its own) whenever its revision
changes and floats it over its object, turned to the eyes, fading in and out. Oblivion's alchemy
bench shows what the bowl would make this way, in its alchemy menu's own parchment, icons and fonts.

A card can also be a page (`OBJF_SHEET`): its object is a thin box the hands take up and put down,
and the card is drawn on its face (`rustvr_avatar::HeldModel::sheet`, the card's texture on a quad,
its paper on the back) with the held things, in their depth and the game's lighting, so a finger
shows over it. The hands and fingers go through a lying page (pushed, it would only be squeezed
between a hand and its table); an empty hand's index fingertip tapping its face is a touch
(`rig/sheet.rs`: down within 1.2 cm over it or 8 mm through it, felt as a tap; lifted 2.5 cm off
again, counted in the page's state with where on the card it came down; the hand taking the page up
meanwhile, nothing). Oblivion's alchemy bench leaves
a note of the game's scroll this way, in its handwriting, listing what the player can make and the
steps of the recipe touched.

## 7. Adapters

An adapter is small because the SDK does the shared work. Per game it:

1. **Connects** with `Adapter::new` / `poll`. With no runtime running, the game stays flat.
2. **Anchors**: gives the game's eye position (with and without bob), its look yaw, and whether a
   scripted camera is running. `TrackingMapper::update` returns the `View`.
3. **Renders stereo** with `View::eye_view` + `math::projection` per eye, widens culling
   (`math::combined_fov`), and keeps the HUD out of the 3D image (into the HUD texture). This is
   the per-game (or per-engine) part the runtime can't remove.
4. **Drives the game:** look angles from `View::look_yaw/pitch`; actions from `Intents` (press
   counters via `actions::Presses`); the gun's viewmodel at `View::weapon_to_world`; shots from
   `View::point(sight_origin)` along `View::dir(aim_dir)`.
5. **Reports** `GameState`: menu, loading, crouched, eye height, zoom, and the gun's `WeaponRig`.
6. **Optionally hands its objects to the rig:** registers guns, magazine templates and props
   (`Adapter::write_objects`), streams its static collision (`push_collision`), draws objects
   where `read_object_states` says they are, fires a shot for each increment of a gun's `shots`,
   and writes linear depth (`TGT_DEPTH`) so the player's body is hidden behind its world. Games
   that don't fall back to the gun glued to the firing hand (`Intents`).
7. **Optionally hands over its own sounds** (§6c): writes a sound pack from the game's files (an
   extractor, like `metro-extract`) and publishes it with `Adapter::set_sound_pack`: the folder,
   each object's sound set, and `SND_GAME_SHOTS` / `SND_GAME_HANDLING` for what the game already
   plays itself.

Adapters can be written in any language: Rust (hook-and-offset games like Metro, using
`retour`), C++ (games with C++ SDKs, using the generated header), or C# (Unity).

## 8. Testing without a headset or a game

- **Unit tests in every crate:**
  - protocol: seqlock, triple buffer, event ring, wake-ups;
  - shared textures across devices;
  - tracking maths;
  - every mechanic;
  - panels and lasers;
  - settings profiles.
- **Scenarios:** `rustvr --sim all --launch-range`. A scripted player in the simulated headset plays
  against the test range over the real protocol and real shared textures. Checks look at the
  runtime's state and the range's reported stats (shots, hits, reloads, crouching, gestures), and
  each run saves PNGs of exactly what the headset would show into `captures/<scenario>/`.
  `rustvr --list` lists them:
  - `smoke`: connect, look around, the game picture reaches both eyes every frame.
  - `aim`: raise the gun to the eye, aim down the sights and hit the target 10 m ahead.
  - `two_handed`: grab the foregrip and steer the barrel with the off hand.
  - `game_recoil` (physical): pick up Metro's Kalash, fire a burst aimed: it kicks with Metro's own
    recoil (the range's kick table), the muzzle held up mid-burst and back on target after.
  - `reload`: eject, grab a magazine at the hip, seat it, rack the charging handle.
  - `reload_slap`: fire some rounds, then slap a magazine in with the off hand.
  - `melee`: a fast punch is one strike; a slow reach isn't.
  - `abilities`: flick up, down, left and right: four abilities, no melee.
  - `crouch`: duck in real life; the game crouches without lowering the view twice.
  - `gestures`: grip at the chest, face, top of the head, hip and shoulder.
  - `turn`: one 30° snap turn, applied by the game.
  - `roomscale`: walk half a metre; the game body follows the head.
  - `menu`: pause, switch to smooth turning in the runtime menu, click the game menu, resume.
  - `scope`: aim through an optic; the scope view renders into the lens.
  - `void`: no game; the runtime shows its own room and menu.
  - Physical mode (the range's objects in the runtime's hands):
    - `pickup`: pick up the rifle from the table, aim down the sights, hit the target;
    - `gun_fx`: three shots from the hip: the range shows them as a game would (the flash lights
      the view, smoke, tracers, holes in the target, casings out of the port onto the floor);
    - `mag_reload`: eject, a magazine from the shoulder pouch, seat it, rack the handle, fire;
    - `holster`: draw the pistol from the hip, fire, let go and it returns to the holster;
    - `throw`: throw a bottle down the range;
    - `grenade`: a frag from the chest pouch, the other hand pulls the pin (the lever holds the
      fuse), a throw down range; it goes off 4 s after the release near the target, and the range
      makes the explosion (flash, debris, smoke, the plates within its reach hit);
    - `knife`: a throwing knife from the throwables' table, thrown at the plate 5 m away: it flies
      blade first and sticks in it;
    - `gloves`: point at a can on the far table, flick it to you and catch it;
    - `two_hand`: the support hand on the foregrip steers the rifle;
    - `body`: the runtime solves and draws the body; the wrists follow the hands, fists close;
    - `short_player`: a player shorter than the character stands on the game's floor, legs straight;
    - `seated`: a seated player is drawn standing and never crouches;
    - `vignette`: walking closes the vignette in, stopping opens it;
    - `wall_fade`: the head inside the table fades the view out;
    - `wall_hand`: a hand pushed into the table stops at its surface (ghost hand);
    - `game_sounds`: the range hands over Metro's sound pack; picking up the Kalash, its three
      shots (each with its tail and a casing) and dropping its magazine play Metro's own sounds
      (the mixer runs offline in scenarios, so the checks see which sounds played), and every
      throwable has its set;
    - `game_dynamite`: Metro's dynamite from the throwables' table, the other hand pulls its fuse,
      a throw down range: the fuse lit, the throw and the explosion are Metro's own sounds. With
      Metro's sounds extracted, `grenade` and `knife` check theirs too (the frag's countdown
      beeps, its throw and explosion; the knife's throw and hit);
    - `break_reload`: Metro's Duplet fires one barrel a pull, then clicks; the lever lets the
      barrels fall open and both cases fly out; a shell from the pouch goes into each chamber; a
      flick up snaps it shut and it fires both barrels again;
    - `bolt_cycle`: Metro's Ventil fires once and won't again until the left hand lifts the bolt,
      pulls it back (the case flies out), pushes it home (the next round goes in) and turns it down;
    - `pump`: Metro's Tihar, empty of air, only hisses; four strokes of its pump lever fill it, and
      each shot spends some;
    - `cylinder_reload`: Metro's revolver fires six and clicks; B swings the cylinder out, A drops
      the cases, a speed loader from the pouch goes into the swung-out cylinder, a roll of the wrist
      swings it home and it fires. (These four lay their gun nearest with the range's `--table`,
      and need Metro's guns extracted.)
- **The range's own renders:** `cargo test -p rustvr-range` also draws the guns as the rig holds
  them (`tests/rig.rs`, `captures/rig/`) and its effects close up and from the side
  (`tests/fx.rs`, `captures/fx/`: a shot's flash and tracer, a casing on the floor, holes in a
  plate, an explosion's flash, fire and smoke), with checks that each one shows.
- **The emulated headset:** `crates/xrsim` is an OpenXR runtime with no hardware behind it
  ([XRSIM.md](XRSIM.md)). The runtime, or a game through it, runs its real OpenXR path on it. A
  script drives the head and controllers, and captures save what each eye would see.
  A game adapter's repository runs its game on it with one script (Metro's:
  `tools\xrsim_metro.ps1`, in chasms-vr-metro).
- **Nothing opens a window.** Nothing launches a game, Steam, SteamVR or Virtual Desktop. The
  range renders offscreen, and builds and child processes run at below-normal priority.

## 9. Test content

The test range runs in **physical mode** by default: the built-in rifles, Metro's Kalash and its
Bastard lie on a table at your right hand, the built-in pistol starts on your right hip, every other
rigged Metro gun (20 of them, when extracted) lies on an armory bench behind you, side by side and
grouped by caliber, and there are bottles, cans, crates and a barrel to knock about on the long table
ahead and around. Every gun's own magazines come from the ammo pouch while it's in your other hand
(guns that take the same magazine share one: the Kalash's and the AKSU's). Throwables
(`crates/range/src/throwables.rs`): two frag grenades in the chest pouches and a knife on the belt;
two more frags, two knives and Metro's dynamite, sticky dynamite and incendiary bomb (when
extracted) on a table at your left hand. The range is the game for them: it makes the explosions
(effects, a blast that pushes the props and hits the target plates within 5 m, the grenade back
on its table 3 s later) and counts knives stuck in the plates.

The range also shows what a game shows when its guns fire (`crates/range/src/effects.rs`; in Metro
the game draws its own): a muzzle flash (a sprite facing you and one along the barrel, two frames)
and a point light on the gun, hands and tables; gas and smoke; a tracer streaking to the hit (its
head glows, since from behind your own tracer is a receding dot); a hole where the bullet hit
(`hits.rs` casts it against the room, the plates and the loose objects; holes ride with the object
they're in, at most 64, the oldest fading first), sparks off metal, dust and chips off wood and
stone, a push on a loose object (`EV_IMPULSE`); and a casing from the ejection port (Metro's
`loc_shell_point` locator, the built-in guns' receivers), out right, up and a little back with a
spin, bouncing on the floor and tables and fading after 6 s. Magazine-fed rifles and pistols
eject; revolvers, break-actions and air guns keep their brass. With Metro extracted the sprites
are Metro's own (its muzzle flash `pfx_muzzle1`, gas, sparks `part_iskry`, the fireball
`part_flame_1`, explosion star and smoke, dust, the wallmark `wm_bullet_metall_thin_1` and the
`fx_tracer1` sheet the tracer meshes use); otherwise built-in ones. All of it is cheap sprites
(at most a few hundred), drawn after the world with depth testing and no depth writes. `--legacy` gives
the old behaviour (the current gun glued to the firing hand, driven by intents), which the
original scenarios still test.

`metro-extract` reads the user's own Metro 2033 Redux install (read-only) and writes first-person
guns, hands, a body and props into `local_assets/metro2033/` (`crates/asset` format), and its sound
pack into `local_assets/metro2033/sounds/` (`--sounds` for just that). The test range loads them
when present, and uses built-in guns and the runtime's own sounds otherwise; its built-in guns
borrow Metro's sets (the rifles the Kalash's, the pistol the revolver's). `metro-extract --extras` adds only the
throwables (the first-person bombs and knife, `meshes/dynamic/hud/weapon/bomb|knives`) and the
effect sprites (muzzle flash, sparks, smoke, fireball, bullet hole, tracer sheet), merged into the
existing index. Metro's grenade configs are value-only, so their fuse isn't readable by name
(`examples/config_search` narrows such searches); the range leaves Metro's bombs on the runtime's
4 s default. **`local_assets/` is git-ignored and never
committed or redistributed.**

## 10. Phased plan

| # | Phase | Done when |
|---|---|---|
| 0 | **Loop** | Protocol, SDK, runtime, sim headset and test range; all scenarios pass headless |
| 1 | **Metro adapter** | Metro's world in the headset with the runtime's player controller in it (as SkyCraft does it: the game supplies the world, the runtime the player); then its collision, actors and guns |
| 2 | **Headset** | The OpenXR backend is tested on the user's headset (Virtual Desktop); the HUD moves to quad layers; reprojection is verified |
| 3 | **Second game** | Metro Exodus Enhanced Edition, same engine family: the new adapter takes days, not months |
| 4 | **Second engine** | Skyrim SE through CommonLibSSE (C++ adapter on the generated header), reusing SkyCraft's camera and actor code |
| 5 | **Feel** | Done in the runtime: physics hands, objects, holsters and pouches, physical reloads, throwing (grenades, knives), gravity gloves, full-body IK and an avatar, spatial sound, comfort (§6b). Next: the Metro adapter registers its guns, grenades, knives and collision (and maps the runtime's throws onto its own projectiles), melee and throw aim assist aimed at actor capsules (needs an actor table, as in SkyCraft), tuning on the headset |

## 11. Decisions (2026-10-21)

1. **Rust** for the runtime, the SDK and the Metro adapter. C++ only where a game's modding SDK is C++.
2. **The runtime is a separate process**, so a game crash doesn't take down the headset and a
   loading screen doesn't freeze the view.
3. **OpenXR, not OpenVR.**
4. **The runtime creates the shared textures** (D3D11, typeless 8-bit, keyed mutex). D3D12 and
   Vulkan games import D3D11 shared handles.
5. **Runtime-drawn layers are composited into the projection layer** for now, so the simulated and
   real backends draw identically. Quad layers for sharper panels come in phase 2.
6. **Toolchain:** Rust 1.99 is pinned (`rust-toolchain.toml`) because egui 0.37 needs 1.95+.
7. **The runtime simulates the player's hands and objects (Rapier)**, not the game: hands and
   held things feel the same in every game. The game only supplies collision and draws objects
   where they are; its own physics takes over nothing it can't see.

## 12. Risks

| Risk | Mitigation |
|---|---|
| Stereo rendering stays per-game work | The SDK keeps it to "render these two views"; it's per engine where possible |
| Cross-process GPU sync stalls | Triple buffer plus keyed mutexes never contend in steady state; measure in phase 2 |
| A game can only draw one eye per frame | Supported (alternate eyes into one slot), with synchronized eyes via its frame timer, as the Metro mod does |
| OpenXR is untested until the user tries it | The backend is written against the openxr crate and compiles; phase 2 |
| Bugs inherited from the Metro mod | Features are re-derived and tested, not copied |
| Anti-cheat | Single-player games only |
