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
                   │  shared memory "Local\RustVR_v4" (crates/protocol)
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
- Metro's `metro-assets` (in the chasms-vr-metro repository) reads Metro 2033 Redux's archives
  (`metro-extract`): models, textures, recoil, and its sound pack (`sounds.bin`, §6c).
  Local test content only, never committed.
- `content/rigs/<game>.toml`: how the hands hold each of a game's guns, measured on its models
  (ours, so committed; `rustvr_asset::rigs` reads them). [RIGGING.md](RIGGING.md) says how to rig
  a gun.

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
| `ObjectTable`: the objects the player can handle (guns, magazines, grenades, knives, props, templates), and how a gun is worked (`action`: self-loading, bolt, pump, break, swing; loaded into its chambers; air) | adapter -> runtime | seqlock |
| `ObjectStates`: where each object is, who holds it, rounds, shots fired, what's in a gun's chambers and where its action is (a slide, a bolt's lift, open barrels, an air gun's pressure), a grenade's fuse and when it went off | runtime -> adapter | seqlock |
| Collision ring: static collision (boxes, triangle meshes) for the runtime's physics | adapter -> runtime | SPSC byte ring |
| `HandPoseTable`: the game's own hands on its guns (every finger joint) | adapter -> runtime | seqlock |
| `KickTable`: the guns' recoil as the game authored it (per-shot kicks, keyframed) | adapter -> runtime | seqlock |
| `SoundPack`: where the game's sound pack is, each object's sound set, which sounds the game plays itself | adapter -> runtime | seqlock |
| `HudLayout`: which rectangle of the HUD texture is what (subtitles, hints, ammo, status...) | adapter -> runtime | seqlock |
| `Ride` / `RtRide`: the mounts near the player and the one ridden (its saddle, gait, where the rider sits, the reins) / what the rider does with it (gait, pace, steering, getting on and off) | both | seqlock |

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
| Physical crouch | Standing height is calibrated as you play; ducking 35 cm crouches the game, and the view stays at your real head height |
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
| Holsters | Both hips (13 cm) and both shoulders (behind the head only), on a body anchor that lags the head yaw by 0.5 s and freezes while a hand reaches into a zone. Let go of a gun gently in a slot to holster it (a swing faster than 1.2 m/s throws instead); a tick tells you you're in one. Auto-return of let-go guns is optional (off). Pouches for small throwables, torso-anchored as the research recommends: either side of the sternum (8.5 cm, clear of the magazine pouch in its middle) and on the belt front on the weapon hand's side; only grenades and knives fit them, and never an armed grenade |
| Magazines | Unlimited: grip at the chest or belt front (or any empty slot) while the other hand holds a magazine-fed gun. Firing-hand A drops the magazine, straight into the other hand if it's squeezing under the gun |
| Reloading | Guns keep their own magazine, chamber and slide state. A magazine within 9 cm and 45° of the well seats itself (strict mode 4 cm / 25°), but not the one just taken out until it has left the well; pull the slide or charging handle back 90% of its travel to chamber; B or slamming a magazine drops a locked slide |
| Guns worked by hand | `rig/actions.rs`, the way Half-Life: Alyx, H3VR and Pavlov do them; the descriptor says which (`ObjectDesc::action`, `WPN_LOADS_CHAMBERS`, `WPN_PNEUMATIC`). **Break actions** (the Duplet): B (or A) is the lever, a flick of the wrist muzzle-down (6 rad/s) or the support hand on the barrels' foregrip pulling them works too; unlatched, the barrels fall open under their own weight, a pendulum on the gun hanging where gravity puts it (the stock lifted to them swings them shut); past 75° the spent cases fly out (live rounds stay); a flick up (5 rad/s, read from the controller since a heavy gun lags the wrist) throws them shut, or the hand pushes them home, and they latch. **Swing-outs** (the Ashot's barrel, a revolver's cylinder on its crane) are the same about a pin along the bore; a revolver takes and drops its speed loader only with the cylinder out, and it swings out with it. Guns **loaded round by round** fire their chambers in order (one barrel a pull), the cases staying in them; the pouch hands one round, which goes into an open chamber when its base comes within 5 cm of the mouth pointing along the bore (strict mode 2.5 cm); a live one can be pulled back out. A **magazine that stays in the gun** (the Uboynicheg's cylinder, the Helsing's darts: `WPN_LOADS_ROUNDS`) takes its rounds the same way, one at a time through its port, at any time. **Bolt actions** (the Preved, the Ventil) and pumps fire once and wait: the knob follows the other hand along its L-shaped path in weapon space (turned up about the bore before it slides, turned down only once it's home), the case flies out at the back of the stroke, the next round goes in on the way home, and it stays where it's let go of. **Air guns** (the Tihar, the Helsing) start empty: each whole stroke of the pump adds pressure, each shot spends some, below the minimum the trigger hisses, and a shot's strength is the pressure it fired at. The range draws it all from the reported state (barrels and chambers turned on the hinge, the bolt turned then slid, a pump lever swung to match the pull, the gauge's needle between the game's own empty and full poses); every step is a cue |
| Shooting | Semi or full auto from the gun's state; each shot increments `ObjectState::shots` for the game to fire. **The game's own recoil** when the adapter gives it (`KickTable`, `rig/kick.rs`): each shot plays one of the gun's view kicks and one of its gun kicks, picked for how it was fired (hip or aimed, standing or crouched, first shot / follow-up / long burst). View kicks overlap and add up, so a burst holds the muzzle up and shakes it and it settles when you stop; a gun kick replaces the one playing. The head never moves: on a flat screen the camera's turn takes the gun with it, here it turns the gun about the eye. Metro's come from its camera tracks and fire motions (see `metro-assets/src/recoil.rs`; Kalash: 2.5 degrees and 16 mm a hip shot, about 3 degrees held through aimed full auto). The game's numbers are for its player's hold, a long gun in both hands at the shoulder: off the shoulder it kicks 1.7 times as hard, in one hand 3 times (twice with the stock at the shoulder); a pistol in two hands 0.75 times (the push grows half as much as the turn). The shoulder is generous: the back of the gun within 20 cm of it is fully braced, fading out by 38 cm. Hands tab: on/off, turn and push scales (1 = the game's), the one-handed and off-the-shoulder scales. **Otherwise** recoil follows Escape from Tarkov's (`rig/recoil.rs`): a fast snap and push back, then EFT's hand-rotation process. Each kick leans a random angle off vertical, the first shot of a burst hits hardest, and the return spring is weak while firing, so full auto climbs; from the 4th shot it plateaus and wanders (stable mode). When you stop it returns on a curved, sideways path and settles a little up and off (the after-recoil offset), so you pull it back on target; the offset fades over a few seconds. Per-gun character: `recoil_climb`, `recoil_return`, `recoil_angle`. Two hands give 55% of the rotation and a faster return; a shouldered stock 40%. Fixed 240 Hz springs with seeded randomness |
| Throwing | Half-Life: Alyx's estimator (`rig/throwing.rs`, from `research_notes/VR throwables and UI/throwables.md`): 0.3 s of controller history per hand in tracking space (the runtime's own velocity where it gives one, so snap turns don't pollute it), the fastest three frames in the 0.12 s before the release averaged, their direction from a line through the positions around them, locomotion added at the release. So a hand that slows before letting go, or a late grip release, still throws at its peak, and the object leaves from where it was then (the Valve patent's backdating, at most 0.15 s, never through the world). A speed-dependent gain on top (Valve's curve: 1 below 1.5 m/s, 1.25 by 6 m/s, 1.35 for grenades since players undershoot), capped at 25 m/s (knives 30); under 1 m/s it's a drop (no gain, a quarter of the spin). Spin is 0.75 of the hand's, at most 20 rad/s; the wrist's spin adds `0.8 w x r` only past 3 rad/s (HVR). Mid-swing (over 1.5 m/s) the grip lets go of a throwable as it eases a quarter off its hardest squeeze (Deck's release), not only once it's nearly open; guns and magazines stay held. Hands leave a hard throw alone for 0.2 s |
| Grenades | `rig/throwable.rs`. The fuse is the game's (`ObjectDesc::fuse`), else 4 s. Two arming schemes (Hands tab): *pin* (default, Pavlov / H3VR / Into the Radius): the other hand takes the ring (8 cm) and pulls it 5 cm out; a lever holds the fuse while the grip does, so it starts when you let go, or press B to cook it in your hand; a grenade without a lever (Metro's lit fuse) starts as the pin comes out. *Button* (HL:A's priming button): the trigger or A on the hand holding it. One-handed anyway while the other hand holds a gun, or for a grenade without a pin. It ticks (sound and a faint pulse in the holding hand) every second, four times a second in the last 1.5 s; it can be re-gripped or caught, the fuse keeps burning; the game's pause stops it. When it runs out (impact grenades: on a hard hit once thrown) the game is told (`ObjectState::shots`, `OS_SPENT`) and makes the explosion; its blast pushes the rig's loose objects through `EV_BLAST` |
| Throwing knives | Blade first (Meta's dart, HVR's stab test): at the release, more than 8 rad/s about the blade's flat throws it end over end (that spin only, nudged so the tip comes round where it's going to hit); otherwise it's a dart, turned tip-first once and kept on its flight (16/s). It sticks if the tip meets something at 1 m/s or more within 60 degrees of its flight (72 with the assist, on by default); a spinning knife is judged in its tip-forward pose whenever its tip is round the front. Stuck (`OS_STUCK`), it rides with what it's in and pulls back out with a grip. Sticky bombs stick to whatever they hit |
| Gravity gloves | Half-Life: Alyx's, with its own numbers (`vr_hand_pull_*` / `vr_gg_*` from its `server.dll`, see `rig::alyx`): point the hand (its aim or its palm) at something within 10 m (a 15° cone from 25 cm behind the hand, held to 20°; nothing nearer than 30 cm); 0.1 s pointed at it takes it (it glows). Grip locks it on: it hops and strains towards you; pointing 60° away, or 0.25 s with the grip let go, drops the lock. A flick (the hand yanked back at 1.2 m/s, or the wrist flicked back at 5 rad/s) throws it on Alyx's arc: 0.6 s in flight out to 2.5 m, 0.8 s at 10 m, launched 20° above the line near and 10° far, gravity eased to as little as half; it steers after your moving hand from 0.6 s out to 0.05 s before arriving (up to 12.7 m/s^2). Caught within 0.5 m with the grip held (a 0.125 s pulse) |
| Feedback | Every interaction is one cue: a haptic pulse and a spatial sound on the same frame. Impacts scale with speed. Cues have event names (`grenade_pin`, `grenade_arm`, `grenade_tick`, `grenade_throw`, `explosion`, `knife_throw`, `knife_stick`; for the guns worked by hand `break_open`, `break_close`, `swing_open`, `swing_close`, `shell_in`, `shell_out`, `case_eject`, `bolt_up`, `bolt_back`, `bolt_forward`, `bolt_down`, `pump_back`, `pump_forward`, `pump`, `air_hiss` ...) for games' sound packs (the actions' play the pack's nearest handling event, `bolt_back` and the like, until the audio crate has events by their names); until one gives them their own they use the nearest synthesized sound | A cue also names its object (the gun, for its magazine, bolt and shots), so the sound is that object's own from the game's sound pack (§6c)
| Creatures' bodies | Every part of the game's creatures (`Actors`) is a kinematic capsule in the rig's physics, following the game: hands, what they hold and loose things are stopped by them, and push them (`GamePush` naming the part: the point must move at least as fast as it's pressed into, plus what the hand would have it go further). A blow is judged by the swing (where the hand drives the weapon, before the body stops it): the velocity it meets the body with. A stab or chop that goes in stays in: it only slides along its way in (out at most 0.9 m/s); what the hand pulls it across, or out faster, drags the creature by where it went in; it comes out back past where it went in (or yanked 50 cm across). The same into the world, whatever it's made of (wood when the game doesn't say: a table, a wall, the floor), where it doesn't move at all, and into loose things, which it drags |
| Fingers | Curls from the trigger, grip and thumb-rest touch. Holding something (not in a game's own hand pose for it), each finger closes on it (`avatar/src/grasp.rs`): bent in from straight, every joint stops as its segment meets the surface while the joints past it curl on, so it wraps a handle and folds over an edge (a segment may come closer near its knuckle, which folds round the edge it's on); a finger that's in the thing straight (a thumb under a palm laid flat on a crate) first bends back at its base until it's out; the drawn hand's own finger geometry, the handle it's on (a capsule) or the thing's collision shape. It closes over 0.12 s as the hand takes hold and opens as it lets go; a gun held by its grip keeps the index on the trigger |
| Full body | `crates/ik` solves 22 joints every frame from the head and the hands the rig drew (VRIK/FRIK-style: torso yaw from the hands, spine pitch from the head drop, stepping legs, elbow prediction, hands pinned exactly). The avatar (Metro's Miller when extracted, else a mannequin) is drawn between the game's picture and the overlays, hidden behind the game's geometry using its linear depth; the head is cut away in first person. Hands only, arms or full body. The body stands on the game's floor (the game puts your eyes at its own eye height, so the real floor would leave it floating or crouched), its proportions follow your height (set, measured, or the game character's), and its arms follow your arm span: set, measured in a T-pose, or automatic (it grows whenever your real arms reach further). Seated mode draws it standing |
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
| Status | on the back of the off-hand wrist, shown while you look at it |
| Overlay | fixed to the face over the whole view (damage, fades) |
| Centre and the rest | the flat screen, 1.4 m ahead |
| Hidden | not drawn (a crosshair: the gun's sights aim) |

Everything that follows the head waits for a 20 degree turn, glides back in front (0.15 s) and
stops; it never rolls. The game stays the source of truth for *when* a piece shows: its rectangle
is drawn every frame, so it appears and fades as the game draws it. Without a layout the HUD is one
flat screen, as before. Menus are a world-locked panel with a laser (§6). Metro's layout is in
its adapter's `docs/METRO.md` (the chasms-vr-metro repository).

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
