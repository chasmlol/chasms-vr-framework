# RustVR — Design Doc

> One VR player experience (hands, guns, aiming, gestures, comfort, menus) that connects into any
> flat game through a small per-game adapter, instead of every VR mod rebuilding VR inside its game.

Status: draft v0.3 · 2026-10-31

---

## 1. Core principle

**The runtime owns the player's body; the game owns the world.**

- **The runtime** (`rustvr.exe`, its own 64-bit process) owns the headset, both hands, what they
  hold, the aiming maths, the gestures, turning and comfort, the menus and compositing.
- **The game** keeps its world, collision, damage, ammo, AI and saves.
- **An adapter** (a DLL inside the game, 32- or 64-bit, or the test range) only *translates*:
  - it maps tracking space into its world;
  - it renders the eyes it's asked for;
  - it reports what the game is doing;
  - it turns the runtime's intents into the game's own input.

If an adapter re-implements a VR mechanic, or the runtime re-implements a game rule, the design has
gone wrong. It's the same rule as SkyCraft's (its `docs/DESIGN.md`, §1), with Minecraft's player
replaced by a VR body.

## 2. Components

```
headset ◀─ OpenXR ─┐
                   │  rustvr.exe, rustvr-play.exe (crates/runtime)
                   │    backend/openxr.rs   headset, controllers, swapchains, haptics, depth
                   │    backend/sim.rs      simulated headset for headless tests
                   │    mechanics.rs        the body -> intents
                   │    rig/                the player rig: physics hands, objects, guns, melee (Rapier)
                   │    body.rs             full-body IK and the avatar that draws it
                   │    combat.rs, gore.rs  hits and spells judged from the hands; blood
                   │    hud.rs, panels.rs   the game's HUD and menus in VR, the laser
                   │    menu.rs             the egui settings menu
                   │    warp.rs             the game's picture moved to each headset frame
                   │    compositor.rs       game eyes + runtime layers -> headset
                   │    spectator*.rs       the desktop view (a window on the monitor)
                   │    session.rs          the frame loop
                   │    scenario*.rs        scripted end-to-end tests
                   │
                   │  shared memory "Local\RustVR_v17" (crates/protocol)
                   │  + D3D11 shared textures (NT handles, keyed mutex) + named events
                   │
                   │  game process
                   │    crates/adapter      the adapter SDK: connect, tracking -> world, slots
                   │    crates/range        the test range (a fake game)
                   │    a game's adapter    in its own repository: Oblivion's (chasms-vr-oblivion)
                   │
                   │  rustvr-voice.exe (crates/voice, optional): the voice plugin, on the same mapping
```

The workspace is every crate under `crates/`. Supporting crates:

- `crates/gpu`: D3D11 device, shared textures, shader compile, PNG readback, egui painter.
- `crates/asset`: the cache format for what extractors read out of installed games
  (`local_assets/<game>/`: models, textures, recoil, bitmap fonts, face morphs, FaceGen's
  controls), and the readers of our own data: gun rigs (`rigs.rs`) and armour marks (`armour.rs`).
- `crates/audio`: the runtime's interaction sounds and a small spatial mixer (cpal, WASAPI). A
  game's own sounds come first: its **sound pack** (§6c), decoded in the background, replaces the
  procedural library (35 sounds, four variations each, no sample files), which plays only while
  no pack is loaded.
- `crates/ik`: full-body IK from the head and hands (torso, spine, arms with elbow prediction,
  stepping legs, a small learned prior for the chest's yaw and the elbows), calibration,
  grip-to-hand offsets, finger curls, and the body floating and kicking in water (`swim.rs`).
- `crates/avatar`: draws the player's body and hands (and what they hold) over the game's picture,
  hidden behind the game's geometry using its depth. Every joint keeps its volume (skinned about
  optimised centres of rotation), and a game's armour is made of rigid plates where its marks say
  ([ARMOUR.md](ARMOUR.md); the `rustvr-armour` binary previews them). It also has the fingers
  closing on what they hold (`grasp.rs`), FaceGen faces and shaping them by hand (`facegen.rs`,
  `sculpt.rs`), and the blood it draws (`gore.rs`). When the adapter publishes its lighting
  (`GameLighting`), meshes with the game's material are shaded as the game shades its own
  first-person model (`shading.rs`): a deferred renderer's shading classes, tone curve and grading
  (`LM_DEFERRED`), or Gamebryo's per-pixel lighting as Oblivion's shaders do it (`gamebryo.rs`,
  `LM_GAMEBRYO`).
- `crates/guns`: guns as RustVR holds them, shared by the range, the adapters and the runtime:
  weapon space, the parts cut from the game's model and how they move with the object's state,
  the game's hands on them, the protocol's descriptions of a gun (`desc.rs`), and a model's convex
  parts as its collision (`hulls.rs`).
- `crates/melee`: the melee rules and numbers: how a contact is judged, its damage, how deep a
  blade goes in, when an edge cuts a limb off (§6f, [MELEE.md](MELEE.md)).
- `crates/xrsim`: an emulated headset, an OpenXR runtime with no hardware behind it (§8,
  [XRSIM.md](XRSIM.md)).
- `crates/voice`: the voice plugin, `rustvr-voice.exe` (§6j). Optional, and left out of the usual
  build.
- A game's extractor, in its adapter's repository, reads the user's installed game into
  `local_assets/<game>/` (Oblivion's `oblivion-extract`, in chasms-vr-oblivion). Local test content
  only, never committed.
- `<game>.toml` gun rigs (in the game adapter's repository): how the hands hold each of a game's
  guns, measured on its models (ours; `rustvr_asset::rigs` reads them from the folder
  `RUSTVR_RIGS` names). No game's guns are rigged yet. [RIGGING.md](RIGGING.md) says how to rig a
  gun.
- `content/armour/<game>.toml` (in the game adapter's repository): which parts of the game's armour
  are rigid plates and what each rides (ours, hand-written per piece; `rustvr_asset::armour` reads
  them). [ARMOUR.md](ARMOUR.md) says how to mark a game's armour.

**The binaries:**

- `rustvr` (`crates/runtime/src/main.rs`): `rustvr [--launch-range]` runs with the headset
  (OpenXR), waiting for a game, or with the test range started beside it; `rustvr --sim
  <scenario|all> --launch-range` runs the headless scenarios (§8); `rustvr --list` lists them.
  Options: `--mapping <name>`, `--settings <dir>` (where `settings.toml` lives), `--captures <dir>`
  (the scenarios' PNGs, default `captures/`), `--size <w>x<h>` (the simulated eye size),
  `--range <exe>`, and `--attach` (a scenario connects to a running game's adapter instead of
  starting the range). It logs to the console (`RUSTVR_LOG` sets the filter).
- `rustvr-play` (`src/bin/play.rs`): the same runtime with no console window. It's what the
  "RustVR" Steam shortcut launches (`tools/add_steam_shortcut.py` adds it) and what a game's
  adapter starts. It starts the runtime on the headset with the test range connected; `--no-range`
  waits for a game instead, `--exit-with-game` quits once the game that connected has exited
  (`--game-pid <pid>` names it before it connects, and a missing headset is then waited for
  quietly while the game runs), `--wait` waits for a headset with no game, and anything after `--`
  goes to the range. Logs go to `rustvr.log` and `range.log` in the runtime's folder; if it can't
  start, a message box says why.
- `rustvr-range` (`crates/range`): the test range (§8, §9). `rustvr-xrsim` (`crates/xrsim`): the
  emulated headset's tool. `rustvr-armour` (`crates/avatar`): the armour preview. `rustvr-voice`
  (`crates/voice`): the voice plugin.

The runtime's folder is `%LOCALAPPDATA%\RustVR` (settings, profiles, logs, `voice.toml`);
`RUSTVR_DIR` puts it elsewhere. `RUSTVR_LINK` names the shared memory (the runtime and the SDK both
read it, and the runtime passes it to the range it starts), so with both set several test runs
share a machine without crossing. `RUSTVR_DESKTOP_VIEW=0` turns the desktop view off
(`capture:<dir>` draws it off screen and saves a frame there every 5 s, or every
`RUSTVR_DESKTOP_EVERY` seconds); `RUSTVR_SILENT=1` mixes the sounds offline instead of playing
them; `RUSTVR_WARP` uses the WARP software device; `RUSTVR_ASSETS` names the `local_assets/`
folder. `RUSTVR_SHOW_COLLISION=1` draws the static world's collision within reach of the head as
lines over the game's picture (what the hands meet, held up to what's drawn).

## 3. Spaces

Everything in the protocol is **left-handed metres, X right, Y up, Z forward** (DirectX, Unity).
Quaternions are `[x, y, z, w]`; matrices are column-major (glam).

| Space | What | Who uses it |
|---|---|---|
| Tracking | the headset's standing space, floor at Y = 0 | the runtime, for everything |
| World | the game's world, in metres with canonical axes (`Basis` converts game units and axes) | the adapter |
| Weapon | a gun's model space, barrel +Z | the adapter describes the gun as a `WeaponRig`; the runtime answers with `weapon_to_tracking` |

OpenXR is right-handed with -Z forward. Its backend mirrors Z at the boundary: positions become
(x, y, -z) and quaternions (-x, -y, z, w). This is tested in `backend/openxr.rs`.

**Tracking -> world lives in the adapter SDK** (`crates/adapter/src/tracking.rs`). It covers
recentring, turning, vertical anchor smoothing, scripted-camera following and roomscale absorption,
and it must use the game's anchor *for the frame being rendered*. If the runtime did it a frame
late, the view would jitter while walking. The runtime only
sends the controls (`TrackingControl`: recentre counter, total turn, world scale, height offset and
compensation, roomscale limit, a mount's turn step, and flags for roomscale, following the game's
camera, its head bob and its camera animations), and the adapter reports back what it applied
(`TrackingState`).

## 4. Protocol

One file mapping, created by the runtime and opened by adapters: protocol version 17, named
`Local\RustVR_v17` by default (`DEFAULT_MAPPING`), 0x550000 bytes (`MAPPING_BYTES`, 5.3 MiB).
Its layout is `crates/protocol/src/lib.rs` and a module per subject beside it (`world.rs`,
`combat.rs`, `magic.rs`, `melee.rs`, `gore.rs`, `grab.rs`, `archery.rs`, `ride.rs`,
`inventory.rs`, `pockets.rs`, `stations.rs`, `cards.rs`, `items.rs`, `avatar.rs`, `player.rs`,
`lighting.rs`, `sound.rs`, `hud.rs`, `voice.rs`; `link.rs` is the mapping's reader and writer). Every
region is listed in address order in `LAYOUT`, which the build checks: none overlaps the next, and
all lie inside the mapping. A change bumps `VERSION`, and with it the mapping's name.
`include/rustvr_protocol.h` is generated from the crate by cbindgen (names prefixed `RVR_`) for
C/C++/C# adapters; `tests/header.rs` fails when it's out of step (`RUSTVR_BLESS=1` rewrites it).
The table goes by subject, not by address.

| Region | Direction | Access |
|---|---|---|
| `Header`: magic, version, PIDs, heartbeats, connect epochs | both | atomics |
| `RtFrame`: predicted poses, controllers, `TrackingControl`, `Intents`, the headset's eye size, the mirror's camera (`MirrorView`) | runtime -> adapter | seqlock |
| `GameState`: flags, when the game's clock stood there (`time_qpc`), tracking state, eye height, zoom, `WeaponRig`, texture request, the sun and ambient (`LightHint`), its water (`WaterState`), its view bob (`ViewBob`), the menu's rectangle in its picture, the adapter's name (which picks the settings profile), stats | adapter -> runtime | seqlock |
| `Targets`: the shared textures' size, format and generation | runtime -> adapter | seqlock |
| `SubmitCtl` + 3 x `SlotInfo`: the texture triple buffer | adapter -> runtime | xchg |
| `Event` ring: haptics, game rumble, recentre requests, the game's blasts and bullet pushes, the shots it fired, its mirror spell cast and its moment, a creature's cry (`EV_VOICE`) | adapter -> runtime | SPSC ring |
| `RtBody`: the player's 22 joints, finger curls, hand states | runtime -> adapter | seqlock |
| `AvatarDesc`: the models the player's body is put together from (a race's skin, what it wears); `FaceDesc`: the player's FaceGen face; `FaceSculpt`: the face as the hands shape it | adapter -> runtime; `FaceSculpt` runtime -> adapter | seqlock |
| `RtPlayerDraw`: the player as the runtime posed it, for an adapter that draws it in its own frame (the runtime doesn't write it now) | runtime -> adapter | seqlock |
| `ObjectTable`: the objects the player can handle (guns, magazines, grenades, knives, melee weapons, bows and arrows, hinges and slides, props, templates), and how a gun is worked (`action`: self-loading, bolt, pump, break, swing; loaded into its chambers; air) | adapter -> runtime | seqlock |
| `ObjectStates`: where each object is, who holds it, rounds, shots fired, what's in a gun's chambers and where its action is (a slide, a bolt's lift, open barrels, an air gun's pressure), a grenade's fuse and when it went off | runtime -> adapter | seqlock |
| Collision ring: static collision (boxes, triangle meshes with their materials, as many meshes as the adapter likes, each replaced or taken out by its id) for the runtime's physics | adapter -> runtime | SPSC byte ring |
| `GamePoses`: where the game's own simulated objects are; `GameHulls`: their convex collision; `GamePushes`: where the hands push them, or a creature's part | `GamePushes` runtime -> adapter, the rest adapter -> runtime | seqlock |
| `Actors`: the creatures near the player as capsules on their skeletons; `MeleeTable`: the blades of the player's melee weapons; `Spells`: the player's spells and magicka; `CombatEvents`: a hit, a block or parry, a spell cast, an arrow loosed, the player struck, a blade pulled out or cutting on, a thing broken, a touch that did nothing | `CombatEvents` runtime -> adapter (ring), the rest adapter -> runtime | seqlock, ring |
| `MeleeWeapons`, `ActorItems`, `MeleeRules`: the melee (each weapon's handles and damagers, what the creatures hold, the game's numbers for the player); `RtMelee`: the player's side (time's pace, climbing, stamina spent, imbuing) | `RtMelee` runtime -> adapter, the rest adapter -> runtime | seqlock |
| `HandPoseTable`: the game's own hands on its guns (every finger joint) | adapter -> runtime | seqlock |
| `KickTable`: the guns' recoil as the game authored it (per-shot kicks, keyframed) | adapter -> runtime | seqlock |
| `GameLighting`: the lights, ambient and grading the game lit its first-person model with, and their light atlas, so the body and hands are lit as the game would light them | adapter -> runtime | seqlock |
| `SoundPack`: where the game's sound pack is, each object's sound set, which sounds the game plays itself | adapter -> runtime | seqlock |
| `HudLayout`: which rectangle of the HUD texture is what (subtitles, hints, ammo, status...) | adapter -> runtime | seqlock |
| `Casters`: each hand's casting (readied, charging, ready), what, how charged, where its art goes (the palm, or a held staff's tip) and which way it would go, for the game's own casting art | runtime -> adapter | seqlock |
| `Overlays`: the game's effect shaders over what the runtime draws (an enchanted weapon's shimmer, a spell's shader on the player's body) | adapter -> runtime | seqlock |
| `ActorBodies`: what grabbing the creatures needs beside their capsules (each part's rotation, speed and mass, the body's mass, whether it's all physics, the player's strength) | adapter -> runtime | seqlock |
| `CreatureGrabs`: each hand's grab of a creature (the point held in its part's frame, where the hand would have it, the pull, the squeeze on a throat, the throw when let go) | runtime -> adapter | seqlock |
| `Ride` / `RtRide`: the mounts near the player and the one ridden (its saddle, gait, where the rider sits, the reins) / what the rider does with it (gait, pace, steering, getting on and off) | both | seqlock |
| `BloodTable`: the game's kinds of blood and their textures; `GoreBodies`: which creature bleeds which, and its stumps; `GoreEvents`: a limb cut off, and the prop it became | adapter -> runtime | seqlock, SPSC ring |
| `Bows`: a bow's model bent at full draw, how its limbs spring back, how the game's arrow flies, its crosshair | adapter -> runtime | seqlock |
| `Inventory`: the player's things as the game keeps them (its categories, each thing's values, how it's used, its model), the game's own slots, what the game took back; `BodySlots`: the body's slots as set up and what each gives now; `SlotEdits`: the game's editor's changes | both | seqlock |
| `Pockets`: the game's creatures' pockets (on their bones), what's in them, how aware each creature is of the player, and the game's answers to what the hands did | adapter -> runtime | seqlock |
| `PocketEvents`: a creature's pickpocketing opening and closing, a thing pulled out (the attempt), planted, kept or dropped | runtime -> adapter | ring |
| `Receptacles`: which objects are bowls the hands make things in (their shape, how much they hold, what may go in now, what grinds in them) | adapter -> runtime | seqlock |
| `Cards`: cards of the game's own interface (its menu art cut out, text in its bitmap fonts) to draw over its objects, or as the face of a page (`OBJF_SHEET`) | adapter -> runtime | seqlock |
| `ItemInfos`: what the game says about a thing in the hands (its name and count, its numbers as the game words them, whether taking it is a crime), for the runtime's card beside it | adapter -> runtime | seqlock |
| `VoiceScene`, `VoiceEvents`: who's around, who the player talks to and who they are, saves and loads, what the listener saw; `VoiceLines`, `VoiceStream`: what was heard, the lines to say, a streamed line's voice and mouth (§6j) | adapter -> voice plugin; plugin -> adapter | seqlock, rings |

- **Shared memory** is only touched through 32-bit atomics, so concurrent access is defined
  behaviour.
- **Connect handshake:** the adapter bumps `adapter_epoch`; the runtime resets the triple buffer
  and acks the epoch. A runtime restart resets the ack, so adapters reconnect by themselves.
- **Actions** are press *counters*, not pulses, so a game that skips frames never misses a press.
- **Textures** are created by the runtime (typeless 8-bit colour; adapters view them as UNORM like a
  back buffer, the runtime as sRGB), named `<mapping>_g<generation>_<kind>_<slot>`, and opened by
  name. Kinds (`TexKind`) are left/right eye, left/right depth, HUD, aux (a scope or the mirror's
  view) and its depth, the light atlas, and the game's bloom and lens light for each eye; an
  adapter asks for the ones beyond the eyes (`TGT_*`).
- **Events:** the runtime signals `<mapping>_frame` after each `RtFrame`, the adapter
  `<mapping>_submit` after each published slot. A side is alive while its heartbeat is at most 2 s
  old.

## 5. A frame

1. The backend predicts the next display time and poses (`xrWaitFrame`, or the sim clock).
2. Mechanics and the rig turn the body into `Intents` and `TrackingControl`.
3. The runtime writes the `RtFrame` and signals `<mapping>_frame`. With `vr.predict` on (and not
   in lockstep) the game is sent the head and hands predicted for when its picture will show: the
   measured lag between a frame and its picture, at most 50 ms ahead.
4. The adapter maps tracking space into its world with its current anchor, drives the game, and
   renders the eyes (plus the HUD, and the aux view if asked) into its back slot. It records the
   poses it actually used in that slot's `SlotInfo`, publishes the slot and signals
   `<mapping>_submit`.
5. The runtime takes the newest slot and draws its layers over it with **the same poses** (lens,
   body, held things, panels, laser). It submits those poses to OpenXR, so the headset's
   reprojection corrects for any lateness.

   When the picture is late (more than 2 ms) or the eyes have moved more than 4 mm since it was
   drawn, the runtime instead moves the picture itself (`crates/runtime/src/warp.rs`, setting
   `vr.reproject`, on by default): each eye is laid over a grid, pushed out to its depth, carried
   from where that eye was in the world (`SlotInfo::eye_map`) to the game's mapping now and drawn
   from this headset frame's eyes, the other eye's picture filling what the move uncovers. A
   picture that arrives on time is submitted as it was drawn. The headset only turns a picture for
   the head, so without this the world steps whenever the game walks or turns the player between
   its pictures, while the hands (drawn every headset frame) don't.

   "The game's mapping now" is the game's last step carried on to now
   (`crates/runtime/src/locomotion.rs`): the adapter stamps each state with when the game's clock
   stood where it has the player (`GameState::time_qpc`, QueryPerformanceCounter ticks), and the
   walk and turn between the game's last two steps go on from there, at most 0.15 s past the last
   step. It goes to now, not to when the frame shows: carried that far, the world would run ahead
   of the game's own picture of it. The picture itself is carried on by the walk for as late as it
   is. The game steps at its own uneven pace, so without this the world stands still for a frame
   and then jumps, however smoothly the picture is moved. A step that
   disagrees with what was being shown (a stop, a hitch) is glided out (a 60 ms time constant)
   rather than jumped; a step of more than 1 m or 0.3 rad is a jump (a snap turn, a teleport). The
   rig uses the same mapping, so the hands reach into the world where the player sees it. What a
   hand holds goes with the walk and turn as a velocity (`rig::physics::Drive::carry`): the world
   can stop it, and its weight only resists the hand's own swing. Held back by the world, it stays
   held back (nothing held goes through anything); more than 0.7 m from the hand, it's put back in
   the hand if the head can reach that spot.

   While the game loads another place (`SLOT_HELD`, `crates/runtime/src/held.rs`), its adapter
   gives the last picture it drew of the old one again; the runtime keeps it where it was in
   tracking space and draws it through the warp every headset frame, so the place stays round the
   player. When the game draws again, the held picture fades over the new one as the game's own fade
   says (`SlotInfo::held_over`), or hands over in 0.5 s.

   A game that draws the player inside its own frame says so with `GS_DRAWS_BODY` (so its fog,
   bloom, tone curve and grading reach it). The runtime then draws no body or hands of its own (it
   clears `RT_DRAWS_BODY`, and the menus held in the hands are drawn in the world as the rest); it
   still solves the body and publishes `RtBody`. The protocol keeps the posed player's region
   (`RtPlayerDraw`) and `SlotInfo::player_frame` for it, but the runtime writes and reads neither
   now: the runtime's redrawing of the player's pixels at the headset's rate (`in_frame.rs`) has
   been taken out. With `vr.headset_depth` on, the runtime gives the headset each eye's depth
   (`XR_KHR_composition_layer_depth`), the body's included (`Avatar::draw_game_depth`).

In tests the runtime runs in **lockstep** (`RT_LOCKSTEP`): it waits for the adapter's frame every
time, and the adapter steps exactly once per headset frame using its `dt`, so runs are
deterministic. With a real headset the game runs at its own pace and the newest slot wins.

Games that can only draw one eye per engine frame (alternate-eye rendering) fill eye L and eye R
of the same slot on consecutive engine frames and publish after the second. `SlotInfo` carries
per-eye poses, so both eyes are still submitted with the poses they were rendered with.

## 6. Mechanics

`crates/runtime/src/mechanics.rs` turns the player's body into what they do in the game, with
numbers tuned by playing; Titanfall 2 VR is the feel reference. All of it works in tracking space:

| Feature | Behaviour |
|---|---|
| Gun in hand | The rig's grip sits on the palm and the barrel follows the aim ray; per-game offsets come from the settings |
| Two-handed | The off hand grips near the foregrip (25 cm reach), and the gun pivots around the firing hand so its foregrip follows the off hand |
| Recoil | The game's own (`rig/kick.rs`, as on the rig): the adapter registers its held gun's kicks for `KICK_OBJECT_HELD` in the `WeaponRig`'s weapon space and says when the game fires (`EV_GAME_SHOTS`, from the game's own round count); each shot plays a kick on the gun, so bursts climb and you pull it back down. One-handed its turn is `recoil_one_hand` times the game's (3 by default) and its push grows half as much (twice the game's) |
| Line of sight | Shots start at the sight and travel parallel to the barrel. Adapters can report the exact sight line (`RIG_SIGHT_LINE`) |
| Auto aim-down-sights | Fires when the sight line passes within 6 cm of an eye while you look along it, or when the grip is squeezed; it drops out past 12 cm (35 cm while the trigger is held, so a burst never drops it) |
| Scopes | A camera just past the muzzle renders into a lens behind the optic, at true magnification from the game's own zoom |
| Movement | Head- or off-hand-relative stick, sprint toggle, snap (45° by default) or smooth (120°/s) turning, weapon switch on the stick |
| Roomscale | While the stick is idle, the game body walks after the head (starts at 20 cm, stops at 5 cm). The view stays put because the adapter absorbs the motion |
| Up against things | A game's body is often wider than the player's (Oblivion's is 30 cm round), so it stops the view well short of a table. The game's adapter, which knows its body and its world, tells the mapper how far the view stands ahead of the body each frame (`set_lead`). Oblivion's gives it whatever of the step asked the body didn't go: up to a table until the player's front is at it, otherwise a head's breadth from a wall, and inside its body for what isn't the world's (a door, someone). Walking, the view moves on; following, the head's way from the body shortens instead |
| Physical crouch | Standing height is calibrated as you play (and held while you crouch); crouching a little (8 cm by default, the Comfort tab's "Stealth crouch amount") crouches the game, its sneak, and the view stays at your real head height (crouching for real in a crouch from the stick takes the view lower still: no jump back up). The Comfort tab's "Duck to crouch" turns it off |
| Height | Your upright head height (standing, or sat up in a chair) is the game's eye height, whenever the view was recentred: the runtime keeps that height (it follows the head until the headset is worn and still; held higher a second it rises there; a slouch (less than half the stealth crouch) lowers it slowly, a deeper drop not at all; a recentre takes it as it is) and takes the recentre's own height back off (`height_offset`). The body is drawn from the same height, so it stands on the game's floor |
| Manual reload (default) | For a game that registers no objects (with objects the rig reloads, §6b). Titanfall 2 VR style: firing-hand A ejects the magazine, the off hand grabs a fresh one at the hip or chest (squeeze), seats it in the well (8 cm), then racks the charging handle (pull it back 5 cm). Letting go drops the magazine. The game gets one Reload press, at the rack. Intent flags let the game hide the empty magazine and show the one in your hand |
| Slap reload | The simpler option, and the fallback for guns without a magazine well: the off hand goes into the well (12 cm), then a squeeze or an upward slap faster than 0.9 m/s reloads |
| Melee | A hand striking forward faster than 2.8 m/s, with a 0.5 s cooldown; not the firing hand while it's aiming |
| Ability flicks | Titanfall 2 VR's titan abilities, for any game: hold the off-hand grip away from the body and flick up, down, left or right (2 m/s) |
| Gestures | Squeezing the grip at the face, the mouth, top of the head, chest, over the shoulder or at the hip; an open hand swept across the eyes faster than 0.7 m/s wipes them |
| Watch | The off-hand wrist turned up in front of the eyes |
| Menus | The game's menu becomes a world-locked panel with a laser cursor; the runtime's settings panel floats beside it |

Each mechanic has unit tests (`cargo test -p rustvr-runtime`), and most have a headless scenario
that drives them through the real protocol against the test range (§8).

## 6b. The player rig

When a game registers objects (`ObjectTable`) and streams its static collision, the runtime's
**rig** (`crates/runtime/src/rig/`) owns the hands and everything they do. It's built from the
research in `docs/design/vr-player-controller.md`, whose rule is **pin the hands, cheat
everything else**: hands are exactly where the controllers are in free space, and everything else
(grabs, magazines, throws, the body) bends in the player's favour.

Physics is Rapier, in game-world metres, using the `TrackingState` mapping the adapter reports.
Held and holstered objects are reported back in tracking space (`OS_TRACKING`) so they never trail
the hands while walking.

| Mechanic | Behaviour |
|---|---|
| Physics hands | Light hands driven by velocity, each one's collider its palm (a box from the wrist to the knuckles, as thick as a hand, where the drawn palm is) and its five fingers curled as the controller has them (`rig/fingers.rs`): 1:1 in free space, stopped by walls, tables and things (nothing goes through them, nor through what they hold), pushing loose objects. An empty hand is sent only as far towards its controller as it can get (`Physics::reach`): turned as the controller is, put out of anything solid it's in, then swept there, stopping on the world and on what the other hand holds and sliding on along them. It's never driven into them, so a controller pushed on through a wall leaves the hand resting on its face, following the controller along it as calmly as the controller moves. Held back, it never jumps to catch up (slid off an edge, round a corner, off something loose it was pressed on): it keeps up with the controller's own movement and makes up the rest eased (settled in about a tenth of a second), swept again so it goes round what's in the way. Its fingers aren't stiff: where the palm alone could go, each gives way to what it'd be pressed on, pushed open (a relaxed finger back past straight, a clenched one hardly) or curled in, whichever is nearer. Fingers ease there and back (over about 0.06 s, at most from straight to a fist in a fifth of a second), never snapping, and the hand goes only as far as its fingers as they are let it: they hold it back until they've given way, so pressing harder never pushes them into anything. A palm pressed on a wall comes flat onto it with the fingers and thumb laid along it. The hand's body never touches the world itself (only loose things and creatures), so the solver never fights the sweep; pressed on something, it turns with the controller a touch behind (0.06 s), so the controller's tremor doesn't jolt it. The drawn fingers show the same (`RigOutput::finger_give`). Stuck on the wrong side of something (held back 5 cm or more, getting no nearer, its controller clear through on the far side), it finds the way round, as the robots' "bug" algorithms do. It tries 12 ways along the surface with its palm, a few a frame, each a step at a time out to 40 cm, until a straight way to the controller opens, then goes round the shortest way. It slides along the edge, rolling the wrist over it if it's caught, and heads straight on once clear. With no way round within reach (a wall, the floor), it rests where it is and looks again half a second later. Loose things it's held back by that won't move (a gun on a table) are solid to it while it goes round them, and it presses on them gently, never hard enough to be pushed through. A hand that lets go of what the other hand still holds comes out of it rather than being knocked off it. With the Hands tab's "Ghost hands" on (off by default), a ghost hand shows the controller when one is held back; past 0.7 m they snap back, when the head could reach the controller |
| Grabbing | The palm or a finger on it (within 3 cm), grip hysteresis 0.6 press / 0.4 release (optional toggle). Anything is held just where the hand took it: neither the hand nor the thing moves, and the fingers close on it from where they are (`rig/grips.rs`). Near one of its handles it goes into the hand the way it's made to be held: a gun's pistol grip (10 cm) locks it on, grip-to-palm, barrel where the hand points (eased in over `grab_transition`, 0.15 s, longer from further than 20 cm), the only hold the trigger fires from; a handle (`ObjectDesc::grip_line`: a sword's grip, a haft, a knife's handle; 7 cm) takes the hand anywhere along it, held as `grip_rot` holds it (or reversed, the fist the other way along it). Drawn from a holster or caught, a gun or rigged weapon comes by its grip. A hand closing in slowly (under 0.8 m/s) on what it's about to take rests on it rather than knocking it away |
| Both hands | The other hand takes anything a hand holds: on its grip when the holding hand isn't (it becomes the holding hand, the other holds on where it is: a gun carried by the barrel, then taken by its grip), on the handle a second hand takes (a gun's handguard, `ObjectDesc::foregrip_line`, anywhere along it; a long handle), or anywhere on it. Two hands aim it, pivoting on the holding hand's hold. Let go of either and the other has it, where it has it (hand to hand). A hand on what the other holds beats the pouches and holsters behind it |
| Rigged guns | A gun's descriptor says how the hand holds it (`grip_rot`: the controller grip pose in weapon space) and where the support hand goes (a foregrip line with a slide range, and `support_rot`). Rigged guns point where the hand does; unrigged ones follow the aim ray. Left hands use the mirror image. The rigs are data, measured on each game's models (a game's `<game>.toml`, [RIGGING.md](RIGGING.md)): the line of sight through the gun's own sights, the game's own hands, the parts that move, the game's rate of fire, fire mode, magazine and caliber |
| Weight | Held objects can only speed up and turn so hard, less the heavier they are (the weight setting, squared): a 3.5 kg rifle about 30 m/s^2 and 60 rad/s^2 in one hand (a quarter turn takes a third of a second), two to three times that in two hands; pistols nearly and bottles fully keep up. Both brake into the hand rather than overshoot; pressed into the world a held thing stays as stiff, and nothing held goes through it. Heavy things also throw slower (a rifle about 80% of the hand's speed). Melee weapons are held on the melee hand joint instead (§6f) |
| Two-handed | The support hand takes the handguard within 7 cm of it, anywhere along it (the gun's support hold, slid along, and the game's support pose with it), or the gun anywhere else; it stays where it took hold, drawn on the gun, and steers the barrel about the firing hand (steadied by a One-Euro filter); pulling it 30 cm away lets go. Letting go of the gun hand leaves the gun in the support hand, where it has it. Optional virtual stock: shouldered rifles aim along the shoulder-to-support-hand line |
| Holsters | Both hips (13 cm) and both shoulders (28 cm, from above the shoulder down behind it, the hand behind the eyes), on a body anchor that lags the head yaw by 0.5 s and freezes while a hand is within 10 cm of a zone. The anchor hangs from the eyes as they'd be looking level over the neck (the IK body's neck pivot), so looking down at a hip doesn't swing the zones forward and down with the eyes. A zone counts a hand at its middle or deeper, up to 12 cm in toward the body's middle line, so a hand pressed against the body at a slot is in it; a hand at a zone's middle is in it first, else a hand only deeper is in the zone it's furthest into. Let go of a gun gently in a slot to holster it (a swing faster than 1.2 m/s throws instead); a tick tells you you're in one. Auto-return of let-go guns is optional (off). Pouches for small throwables, torso-anchored as the research recommends: either side of the sternum (8.5 cm, clear of the magazine pouch in its middle) and on the belt front on the weapon hand's side; only throwables and small things (8 cm or less from the middle to their furthest side) fit them, and never an armed grenade |
| Slots of the player's things | `rig/pockets.rs`, `holsters.rs`: slots on the body (on the holsters' anchor, scaled with the player), each a place, a size (let go within it, a thing goes back in) and a reach (taken from that much further out), drawing from the game's inventory (`Inventory`) by its categories in order and a rule (most healing first, magicka, fatigue, most valuable, heaviest, lightest, newest, by name, most of, strongest, or the player's own: what they last put back comes out first), the worn first or not, and what an empty slot does (a fainter tick, nothing, or the rule's first of everything). A grip in one takes its first thing as the runtime's copy (`OS_INVENTORY`, the game's model in the fist, held as its shape is held: a bottle round its body, a long thing at its narrowest near the middle, a flat thing by an edge, anything else upright). Held at the mouth (within 9 cm of a point under the nose) for half a second, with bites or gulps felt on the way, it's eaten or drunk; held up before the eyes, read; touched to the weapon in the other hand, put on it: each once, for the game to do (`shots`); never while the hand moves faster than 1.2 m/s or is held back. Let go in any slot it goes back; anywhere else it falls for the game to drop where it rests. The game says what it took back (used, dropped), and the copy goes. A weapon the game equips from a copy arrives in that hand (`ObjectDesc::replaces`). A grip held still in a slot (1.5 s, the Hands tab's "Holsters' editor"; the menu's Holsters button too) asks for the game's editor; a slot is placed by the next squeeze when the editor asks. While the editor is open every slot shows as a sphere of light the size of its reach (`compositor::Bubble`, over the drawn body), the picked one bright, with its first thing still at its place. Kept per game in the settings (`[[holsters.games.<game>]]`), the game's own until then |
| Magazines | Unlimited (for a gun whose ammo is the game's, `OBJF_GAME_AMMO`, only as many as the game has): grip at the chest or belt front (or any empty slot) while the other hand holds a magazine-fed gun. Firing-hand A drops the magazine, straight into the other hand if that hand is empty, squeezing and within 20 cm of the well |
| Reloading | Guns keep their own magazine, chamber and slide state. A magazine within 9 cm and 45° of the well seats itself (strict mode 4 cm / 25°), but not the one just taken out until it has left the well; pull the slide or charging handle back 90% of its travel to chamber (a gun without `WPN_NEEDS_RACK` chambers as soon as a magazine is seated); B or seating a magazine drops a locked slide |
| Guns worked by hand | `rig/actions.rs`, the way Half-Life: Alyx, H3VR and Pavlov do them; the descriptor says which (`ObjectDesc::action`, `WPN_LOADS_CHAMBERS`, `WPN_PNEUMATIC`). **Break actions** (a double barrel): B (or A) is the lever, or a flick of the wrist muzzle-down (6 rad/s); unlatched, the support hand on the barrels' foregrip can take them and carry them, and the barrels fall open under their own weight, a pendulum on the gun hanging where gravity puts it (the stock lifted to them swings them shut); past 75° the spent cases fly out (live rounds stay); a flick up (5 rad/s, read from the controller since a heavy gun lags the wrist) throws them shut, or the hand pushes them home, and they latch. **Swing-outs** (a barrel that swings out sideways, a revolver's cylinder on its crane) are the same about a pin along the bore; a revolver takes and drops its speed loader only with the cylinder out, and it swings out with it. Guns **loaded round by round** fire their chambers in order (one barrel a pull), the cases staying in them; the pouch hands one round, which goes into an open chamber when its base comes within 5 cm of the mouth pointing within 50 degrees of the bore (strict mode 2.5 cm, 25 degrees); a live one can be pulled back out. A **magazine that stays in the gun** (a revolving shotgun's cylinder, a cluster of darts: `WPN_LOADS_ROUNDS`) takes its rounds the same way, one at a time through its port, at any time. **Bolt actions** and pumps fire once and wait: the knob follows the other hand along its L-shaped path in weapon space (turned up about the bore before it slides, turned down only once it's home), the case flies out at the back of the stroke, the next round goes in on the way home, and it stays where it's let go of. **Air guns** start empty: each whole stroke of the pump adds pressure, each shot spends some, below the minimum the trigger hisses, and a shot's strength is the pressure it fired at. The game draws it all from the reported state (`rustvr_guns::desc::object_part_matrices`: barrels and chambers turned on the hinge, the bolt turned then slid, a pump lever swung to match the pull, the gauge's needle between the game's own empty and full poses); every step is a cue |
| Shooting | Semi or full auto as the descriptor says (`WPN_AUTOMATIC`, at its `fire_rate`); each shot increments `ObjectState::shots` for the game to fire. **The game's own recoil** when the adapter gives it (`KickTable`, `rig/kick.rs`): each shot plays one of the gun's view kicks and one of its gun kicks, picked for how it was fired (hip or aimed, standing or crouched, first shot / follow-up / long burst). View kicks overlap and add up, so a burst holds the muzzle up and shakes it and it settles when you stop; a gun kick replaces the one playing. The head never moves: each kick turns the gun about the pivot the adapter gives it (`KickDesc::pivot`; a view kick about the eye, where the flat game's camera turn takes the gun with it). The game's numbers are for its player's hold, a long gun in both hands at the shoulder: off the shoulder it kicks 1.7 times as hard, in one hand 3 times (twice with the stock at the shoulder); a pistol in two hands 0.75 times (the push grows half as much as the turn). The shoulder is generous: the back of the gun within 20 cm of it is fully braced, fading out by 38 cm. Hands tab: on/off, turn and push scales (1 = the game's), the one-handed and off-the-shoulder scales. **Otherwise** recoil follows Escape from Tarkov's (`rig/recoil.rs`): a fast snap and push back, then EFT's hand-rotation process. Each kick leans a random angle off vertical, the first shot of a burst hits hardest, and the return spring is weak while firing, so full auto climbs; from the 4th shot it plateaus and wanders (stable mode). When you stop it returns on a curved, sideways path and settles a little up and off (the after-recoil offset), so you pull it back on target; the offset fades over a few seconds. Per-gun character: `recoil_climb`, `recoil_return`, `recoil_angle`. Two hands give 55% of the rotation and a faster return; a shouldered stock 40%. Fixed 240 Hz springs with seeded randomness |
| Throwing | Half-Life: Alyx's estimator (`rig/throwing.rs`, from `docs/design/vr-throwables-and-ui/throwables.md`): 0.3 s of controller history per hand in tracking space (the runtime's own velocity where it gives one, so snap turns don't pollute it), the fastest three frames in the 0.12 s before the release averaged, their direction from a line through the positions around them, locomotion added at the release. So a hand that slows before letting go, or a late grip release, still throws at its peak, and the object leaves from where it was then (the Valve patent's backdating, at most 0.15 s and 0.35 m, never through the world). A speed-dependent gain on top (Valve's curve: 1 below 1.5 m/s, 1.25 by 6 m/s, 1.35 for grenades since players undershoot), capped at 25 m/s (knives 30); under 1 m/s it's a drop (no gain, a quarter of the spin). Spin is 0.75 of the hand's, at most 20 rad/s (knives 40); the wrist's spin adds `0.8 w x r`, coming in from 2 rad/s to all of it at 4, at most 5 m/s (HVR). Mid-swing (over 1.5 m/s) the grip lets go of a throwable as it eases a quarter off its hardest squeeze (Deck's release), not only once it's nearly open; guns and magazines stay held. Hands leave a hard throw alone for 0.2 s |
| Grenades | `rig/throwable.rs`. The fuse is the game's (`ObjectDesc::fuse`), else 4 s. Two arming schemes (Hands tab): *pin* (default, Pavlov / H3VR / Into the Radius): the other hand takes the ring (8 cm) and pulls it 5 cm out; a lever holds the fuse while the grip does, so it starts when you let go, or press B to cook it in your hand; a grenade without a lever (a lit fuse) starts as the pin comes out. *Button* (HL:A's priming button): the trigger or A on the hand holding it. One-handed anyway while the other hand holds a gun, or for a grenade without a pin. It ticks (sound and a faint pulse in the holding hand) every second, four times a second in the last 1.5 s; it can be re-gripped or caught, the fuse keeps burning; the game's pause stops it. When it runs out (impact grenades: on a hard hit once thrown) the game is told (`ObjectState::shots`, `OS_SPENT`) and makes the explosion; its blast pushes the rig's loose objects through `EV_BLAST` |
| Throwing knives | Blade first (Meta's dart, HVR's stab test): at the release, more than 8 rad/s about the blade's flat throws it end over end (that spin only, nudged so the tip comes round where it's going to hit); otherwise it's a dart, turned tip-first once and kept on its flight (16/s). It sticks if the tip meets something at 1 m/s or more within 60 degrees of its flight (72 with the assist, on by default); a spinning knife is judged in its tip-forward pose whenever its tip is round the front. Stuck (`OS_STUCK`), it rides with what it's in and pulls back out with a grip. Sticky bombs stick to whatever they hit |
| Bows | `rig/bow.rs`, from the best-liked VR bows (the research is in Oblivion's adapter, `docs/design/archery.md`). A bow (`OBJ_BOW`) is held by its grip, in either hand: that hand is the bow hand and the arrow rests on its side. While the other hand holds one, gripping over that hand's shoulder (behind the head) or at its hip (Hands tab: either or both) takes an arrow (`OBJ_ARROW` templates of the bow's ammo type; with the game's count, none when it has none), held along the fist. Brought within 9 cm of the string's nock point it goes on the string, eased on over 0.12 s, a click in both hands; the hand holding it holds the string. The string comes back with the hand along the bow's arrow line, the bow turning about its grip so the arrow points from the string hand through its rest (eased in over 0.15 s, then exact; the roll is the bow wrist's), to full draw: the bow's own, or with the Hands tab's "Fit a bow's full draw to my arm" (on by default) 0.92 of the arm from the shoulder to the grip, less the grip-to-string distance, kept between half and all of the bow's travel. Past it the string gives only stiffly (up to 15% more), and the shot is no stronger. The bow bends with it, between its model and the game's bent one, as far as the string is drawn of the bow's travel. A tick in both hands each centimetre (the Longbow's: faint at rest, three times as long and four times as strong at full draw), a low strain now and then at full draw. Let go past a fifth of full draw, the arrow is loosed exactly along the drawn arrow: `CE_LOOSE` gives the game its tip, its way and the draw, and the game fires its own arrow; the bow kicks in the bow hand, the string snaps in the other, and the limbs spring back as the game animates them (`Bows`), felt at each swing. Short of it the arrow stays on the string, to draw again or take off. Arrows come from the game's inventory slots when it has them (the quiver zones are for a game without). The crosshair (Hands tab, off by default; `crosshair.rs`): the game's own image straight out along the drawn arrow, 20 m out: where the bow points, not where the arrow lands (no drop, never put on what's in the way) |
| Gravity gloves | Half-Life: Alyx's, with its own numbers (its console variables `vr_hand_pull_*` / `vr_gg_*`, see `rig::alyx`): point at something within 10 m along the line from the shoulder through the hand (a 15° cone from 25 cm behind the hand, held to 20°; nothing nearer than 30 cm), in clear sight and inside Alyx's window round a point a little below where the head looks (60° wide, 70° high); 0.1 s pointed at it takes it (it glows). Grip locks it on: it hops and strains towards you; 0.25 s with the grip let go drops the lock (wherever the hand turns meanwhile, the grip holds it). A flick (the hand yanked back at 1.2 m/s, or the wrist flicked back at 5 rad/s) throws it on Alyx's arc: 0.6 s in flight out to 2.5 m, 0.8 s at 10 m, launched 20° above the line near and 10° far, gravity eased to as little as half; it steers after your moving hand from 0.6 s out to 0.05 s before arriving (up to 12.7 m/s^2). Caught within 0.5 m with the grip held (a 0.125 s pulse) |
| Feedback | Every interaction is one cue: a haptic pulse and a spatial sound on the same frame. Impacts scale with speed. Cues have event names (`grenade_pin`, `grenade_arm`, `grenade_tick`, `grenade_throw`, `explosion`, `knife_throw`, `knife_stick`; for the guns worked by hand `break_open`, `break_close`, `swing_open`, `swing_close`, `shell_in`, `shell_out`, `case_eject`, `bolt_up`, `bolt_back`, `bolt_forward`, `bolt_down`, `pump_back`, `pump_forward`, `pump`, `air_hiss` ...) for games' sound packs (the actions' play the pack's nearest handling event, `bolt_back` and the like, until the audio crate has events by their names); until one gives them their own they use the nearest synthesized sound. A cue also names its object (the gun, for its magazine, bolt and shots), so the sound is that object's own from the game's sound pack (§6c) |
| Hinges and slides | `rig/valve.rs` (`OBJ_VALVE`): anything worked on a hinge or a slide fixed in the world, simulated exactly as one degree of freedom (it can't come off its hinge or be flung off). A valve's wheel, a switch's knob, a lever; a door's leaf, a lid, a drawer, a button, a pull chain (`JOINT_SLIDE`). The hand takes it where it is: on its rim, on the runtime-drawn part that turns, or, a part the game draws, anywhere on its collider (the game's own shape of it), nothing moving. It's cranked by the arm's force-capped spring at that point (180 N over 5 cm of stretch, 3600 N/m, and past that the body pulls, so the point stays within 5 cm of the controller; the leverage is the arm's, near a door's hinge it hardly moves it, at its edge it swings it) or, within 6 cm of its axle, twisted by the wrist (at most 6 N m), against its inertia (a ring's, or its collider's own about the axle: a door is a slab turned about its edge), its friction (static and sliding), its range and the clicks at its ends, its weight (`JOINT_GRAVITY`: a lid falls shut), a spring back to where it rests and damping (`joint_spring`, `joint_damping`). A part that swings through the room (`JOINT_PANEL`, a door's leaf) is a wall to the empty hands and what they hold, and a hand pressed on it pushes it as far as its controller is past the hand (pushing, never pulling); the player's body (a capsule from the head down) and the game's creatures stop it. A hand on one lets go once its controller is 30 cm from its hold, and glides back. Coming hard against a stop is felt (`Cue::Stop`). The game may change its range and stiffness as it goes (a door's latch): the runtime takes them from the table, where it's turned to kept |
| Bowls and grinding | `rig/receptacle.rs`: an object the adapter says is a bowl (`Receptacle`: its floor, mouth, depth and wall in object space, how many things it holds, which objects may go in now) gets a bowl's collision (a floor and a ring of wall), so what the hands hold goes into it and meets it (a pestle grinds against its floor and walls). A thing that may go in, let go over the mouth (its middle within 4 cm of it, up to 30 cm over the rim; or falling in), lies on its floor beside the rest (alone in the middle, else round it), shrunk to fit if it's bigger than the bowl, and rides with the bowl however it's carried; turned over, the bowl spills it. A hand reaching into the bowl takes what's in it (only then: above the rim it takes the bowl or its grinder). The bowl's grinder (a pestle) rests in it where the game's model has it until a hand takes it, and goes back when let go with its tip over the mouth. Its tip pressed low in the bowl (within 2.5 cm of the floor beyond its own size) and worked about grinds what's in it: every metre the hand takes it along the floor (the hand's motion, not the tip's: pressed in, the tip shivers on the floor; a vertical stroke counts half; at most 0.8 m/s counts) grinds each thing `1 / grind_work` of the way, shared among them; what's ground is drawn smaller (to 45%). The hand feels a low buzz while it grinds, stronger the faster, and a pulse each 9 cm stroke; the game sounds the strokes (the bowl's state counts them). Things in a bowl are `OS_IN_RECEPTACLE` (`fuse` how ground, `slide` the size to draw), the bowl's own state the share ground (`slide`) and its strokes (`shots`) |
| Creatures' bodies | Every part of the game's creatures (`Actors`) is a kinematic capsule in the rig's physics, following the game: hands, what they hold and loose things are stopped by them, and push them (`GamePush` naming the part: the point must move at least as fast as it's pressed into, plus what the hand would have it go further). A blow is judged by the swing (where the hand drives the weapon, before the body stops it): the velocity it meets the body with. A stab or chop that goes in stays in: it only slides along its way in, against the melee dampers (`rig/embed.rs`); what the hand pulls it across, or out harder than the hold gives, drags the creature by where it went in; it comes out back past where it went in. A hand further than an arm's reach from the shoulders lets go of it, and it stays in. The same into the world, whatever it's made of (wood when the game doesn't say: a table, a wall, the floor), where it doesn't move at all, and into loose things, which it drags |
| Magic | `combat.rs`: the off hand, empty, is the spell hand while the game has a spell ready; a held thing that casts (`OBJF_CASTS`, a staff) is a spell hand whatever hand holds it, cast from its tip. A finger on the trigger readies the spell, pulling it charges it over the game's casting time (pulses every 0.1 s down to every 0.02 s, strength 0.06 + 0.6 x charge^4, after Skyrim VR's ISPVR; a spike at full; a soft tick every 0.3 s held ready), letting go casts (a touch spell: touching a creature). Target spells go along the line from the shoulder through the palm (a staff's own length), bent towards a throw, onto a creature within 15 degrees. The game draws its own casting art where `Casters` says (`SPELLS_GAME_ART`); a game without one gets the runtime's glow in the palm. Upright things (`OBJF_UPRIGHT`, a torch) hang flame up at the hip and are only held that way up |
| Holding creatures | `rig/creatures.rs`, HIGGS's way: a hand whose grip closes on any part of a creature the game lets the hands take (`ActorBodies`) takes hold of it where it touched it, neither moving, the fingers closing round its capsule (and the ones beside it at the throat). The game moves the body with its own physics: each frame the runtime sends where the controller would have the point held, the hand's pull (the Hands tab's strength times the game's own strength for the player), how hard a throat is squeezed, and on letting go, the throw (the hand's speed at its peak, no gain). The hand is drawn on the part where the game has it, so the body's weight holds it back (a light arm follows closely, a body hangs and lags) and it never goes into the body; it feels the strain (a low rumble, harder the further it's held back); past an arm's reach (60 cm from the controller) the grip slips. Let go, it's a free hand again where it is, on the body's surface (held back by the body if its controller is in it, catching up eased). A fist closed on a creature doesn't punch it. A push on a creature says how fast the pusher itself came in (a shove, not a lean). The range's dummies take grabs with a stiff spring at the point (5000 N/m), go down when lifted by the head or torso and stay down while held |
| Pockets | `rig/pickpocket.rs`, for picking pockets by hand (a game's adapter's `docs/design/pickpocket.md` has the research). The game says where its creatures' pockets are (on their own bones, every frame) and what's in them (each thing a template, drawn from the game's model, and how it feels). An empty hand in one feels a soft brush and a tick for the thing it's on, each kind its own (coins, a key, glass, paper, cloth, something hard); every 4 cm it moves in the pocket it's on the next thing. Gripping takes a copy of that thing into the hand, still the creature's while it's in the pocket (let go there: back, nothing tried); pulled out past the pocket's edge (3 cm) the attempt goes to the game (`PE_TAKE`), and its answer keeps the thing (the player's: kept in a body slot, the game's own drop let go elsewhere, planted let go in a pocket) or takes it from the hand with a hard jolt. Anything to take (`OBJF_LOOT`) let go gently in a pocket is planting, for the game to say. A creature's pickpocketing opens once a hand has been in its pockets for a second (at once on a take or a plant; a shorter look is only feeling) and closes once no hand has been near them for 0.75 s, so the game can run its own rules for both. A slow heartbeat in the hand while the pocket's creature sees the player (the game's own detection); a hand in a pocket pushes nothing of its creature, and its trigger doesn't use |
| Fingers | Curls from the trigger, grip and thumb-rest touch. Holding something (not in a game's own hand pose for it), each finger closes on it (`avatar/src/grasp.rs`): bent in from straight, every joint stops as its segment meets the surface while the joints past it curl on, so it wraps a handle and folds over an edge (a segment may come closer near its knuckle, which folds round the edge it's on); a finger that's in the thing straight (a thumb under a palm laid flat on a crate) first bends back at its base until it's out; the drawn hand's own finger geometry, the handle it's on (a capsule) or the thing's collision shape. It closes over 0.12 s as the hand takes hold and opens as it lets go; a gun held by its grip keeps the index on the trigger |
| Full body | `crates/ik` solves 22 joints every frame from the head and the hands the rig drew (VRIK/FRIK-style: torso yaw from the hands, spine pitch from the head drop, stepping legs, elbow prediction, hands pinned exactly). The avatar (the game's own body once its adapter sends it, `AvatarDesc`; else a built-in mannequin) is drawn between the game's picture and the overlays, hidden behind the game's geometry using its linear depth; the head is cut away in first person. Hands only, arms or full body. The body stands on the game's floor (the game puts your eyes at its own eye height, so the real floor would leave it floating or crouched), its proportions follow your height (set, measured, or the game character's), and its arms follow your arm span: set, measured in a T-pose, or automatic (it grows whenever your real arms reach further). Seated mode draws it standing |
| Face | A game whose heads are FaceGen's (Oblivion, Fallout 3, New Vegas) describes the player's face (`FaceDesc`: its coordinates, detail map and hair colour); the parts with face morphs in the cache (the head and the parts made to it) are moved to it as the body is put together, and the head's texture morphs make the face's texture, laid over its skin as the game's shader does (`avatar/src/facegen.rs`). A slider moved in the game's character creator is redone off the frame (the morphed vertices and the texture, not the whole body). While the creator is open (`FACE_SCULPT`) the hands can shape the face (`sculpt.rs`): a grip takes hold of a point of a bust of the head standing within reach (1.7 times life size) or of the player's own head, and pulls; the face becomes the nearest the face's own FaceGen coordinates can make, within the sliders' ranges, and goes to the game (`FaceSculpt`) |
| Mirror | A standing mirror in the game's world (`runtime/src/mirror.rs`), called up from the Body tab, by pulling the gripped hands apart in front of the chest, by the game's mirror spell (`EV_MIRROR_CAST`, the glass appearing at its effect's moment), or by the game (`GS_MIRROR`: its character creator, its menu beside the mirror with the world round it). Each eye's reflection is drawn every headset frame from that eye reflected in the glass: the game's world rendered once a frame from the head reflected (`MirrorView`, the aux texture and its depth), moved to each eye by its depth, then the whole body with its head, lit by the game's lights |
| Comfort | A vignette eases in while the stick moves or smooth-turns you (strength 0.5, 0.3 s); the view fades out when the head pushes into the game's collision; snap turn 45° (0.75 press, 0.25 re-arm past the stick's 0.25 dead zone, 0.5 s repeat); seated mode; play at your real height or the character's |
| Menu | Opens on a Help page with every control and feature; tabs Help, Comfort (with riding), Hands, Voice, Body (height, arms, what's shown, the mirror), Controls, Weapon, Display and Studio (§6k) |
| Riding | `riding.rs` (`rustvr_protocol::ride`): a free hand closing on a mount's saddle gets on (the game's own way); the reins are where the game has them, taken by the grip, in the rider's frame: one pulled back further than the other or both moved aside steers, measured against the rider and the mount (past a few centimetres' dead zone, as the square of the way past it, all of it at `rein_travel`, 30 cm); both back slows it (nothing within `rein_dead`, 5 cm; hardest at `rein_pull`, 15 cm), and where it's slowed to it stays; from a stand, reins eased and pulled right back again and held 1.2 s, it backs. Its speed is a share of its full gallop: a flick of a rein hand (1.1 m/s down, stopped) or its trigger spurs it, a surge that settles to `cruise` (0.7 of it) over `spur_fade` (8 s); let go, its pace holds 2.5 s, then slows; the sticks ride too. A free hand's trigger held, or the pommel held while leaning 35 cm out, gets off. The body sits in the saddle (`BodySolver::ride`), legs where the game puts them. The mount's turn carries the view (`TrackingMapper::carry_turn`, smoothly or in steps); a vignette while it turns or gallops; a fade while the game gets the rider on or off; no walking the body after the head, the turning stick steers |

In rig mode the mechanics leave grabbing and gun buttons to the rig and map the rest the way most
VR shooters do: the off hand's stick moves, the weapon hand's turns (flicked down, it crouches),
A or X jumps on a hand not holding the gun, the off hand's B or Y is the flashlight, an empty
hand's trigger uses (held, the game's long use), holding the weapon hand's B opens the weapon wheel,
and menu pauses.

## 6c. Sound packs

The runtime plays a sound for every interaction, and **the game's own sounds come first**. A game
that has them hands the runtime a sound pack, which replaces the runtime's synthesised library
(`crates/audio`): an event the pack has no sound for (in its set or its defaults) plays nothing.
The synthesised sounds play only without a pack: before it has loaded, with none, after a load
that failed, or with "the game's own sounds" turned off.

- **The pack** is a folder with a `sounds.toml` and the files it names (Ogg Vorbis or WAV). It maps
  *events* (`grab`, `release`, `holster_in`/`out`, `mag_out`, `mag_in`, `bolt_back`,
  `bolt_forward`, `dry_fire`, `fire`, `fire_tail`, `casing`, `impact`, `pouch`, `zone_tick`,
  `ui_click`, `melee`, `grenade_pin`, `grenade_arm`, `grenade_tick`, `grenade_throw`, `explosion`,
  `knife_throw`, `knife_stick`, `bite`, `gulp`, `hit_cut`, `hit_stab`, `clash`, `chop`, a
  creature's `grunt`, `shout`, `death`, `scream` ...; 47 in all, `rustvr_audio::Event`) to sounds,
  in `[defaults]` and per **sound set** (`[sets.rifle]`, one per gun, say; a set can `extends`
  another). An event is layers that play together (a shot's close crack and its body), each a
  choice of files picked at random, never the same twice running. A layer can be `spatial = false`
  (the unspatialised stereo bed: a shot's echo fills the head) or wait (`delay`: the casing lands
  half a second later). The format is documented in `crates/audio/src/pack.rs`.
- **The adapter publishes it** (`SoundPack` region, `Adapter::set_sound_pack`; `local_sound_pack`
  finds an extractor's pack in `local_assets/<game>/sounds`): the folder, each object's set, and
  what the game plays itself. `SND_GAME_SHOTS`: the game plays the gunshots of the shots it fires,
  so the runtime plays no shot, tail, casing or explosion; `SND_GAME_HANDLING`: it plays its own
  reload and handling sounds (a dry fire too); `SND_GAME_INVENTORY`: it plays its own sounds of
  things going into and out of the body's slots. Per object or for the whole pack. The region is
  additive: an adapter that never writes it gets the runtime's own sounds. The test range plays
  nothing itself and hands over no pack, so the runtime plays its own sounds for it, shots and
  explosions included.
- **The runtime** loads the pack on a background thread when the path (or its `revision`) changes,
  or the game-sounds setting or the output's sample rate does, decoding and resampling everything
  to the mixer's rate, and unloads it when the game goes (each new connection starts with none, so
  one game's sounds never carry over to the next). The audio thread still never allocates, blocks
  or frees: a new pack arrives through a `try_lock`ed slot, the old one stays while its voices
  finish, and packs are freed on the game thread. Each cue then plays the object's set's event (its
  own, inherited, or the pack's defaults). Settings: sound on/off, volume, gunshots, and "the
  game's own sounds".

## 6d. The HUD in VR

A flat game's HUD is a full-screen layer. Hung in front of the face as one sheet it covers the view
and chases the head, which no well-liked VR game does (`docs/design/vr-throwables-and-ui/ui.md`:
Half-Life: Alyx, the Titanfall 2 VR mod, UEVR, Doom 3 VR). So the adapter
describes its HUD (`HudLayout`: rectangles of the HUD texture and their roles) and the runtime
(`crates/runtime/src/hud.rs`) puts each piece where native VR games do:

| Role | Where |
|---|---|
| Subtitles | a caption strip 13 degrees under gaze at 1.5 m, at most 50 degrees wide |
| Hints | a row just above the subtitles |
| Prompts | the caption strip's right end |
| Ammo | on the held gun's left side, facing the eye; hidden with no gun |
| Status | over the inside of the off-hand wrist, shown while its palm is turned up (fully within about 37 degrees of straight up, gone by 57) and you look at it; several stack upwards |
| Overlay | fixed to the face over the whole view (damage, fades) |
| Gaze | under where the head looks, eased after it (0.18 s), just in front of the thing it's about (`HudLayout::gaze`), as big to the eye at any distance (what looking there does: "Open Door", a name) |
| Compass | low in front, 1.2 m out and 26 degrees under the horizon, lazily following the head's turn |
| Target | over the foe it's about (its health) |
| Centre and the rest | the flat screen, 1.4 m ahead |
| Hidden | not drawn (a crosshair: the gun's sights aim) |
| Held | the game's card of a thing in a hand, beside it (`held_cards.rs`, one picture of it kept for each thing) |
| Menu | a piece of a menu, placed by its group (below) |

Everything that follows the head waits for a 20 degree turn (15 up or down), glides back in front
(easing at 0.15 s) and stops; it never rolls. The game stays the source of truth for *when* a piece
shows: its rectangle is drawn every frame, so it appears and fades as the game draws it. Without a
layout the HUD is one flat screen, as before; a layout that hides what it doesn't cover
(`HUDL_REST_HIDDEN`) with nothing in it shows nothing. Each picture says which layout it was drawn
with (`SlotInfo::hud_layout`), and the runtime cuts it by that one: a layout written as the picture
is finished waits for its picture, so no piece is ever cut from the frame before or after (a menu's
pieces from the HUD, or the HUD's from a menu).

**Menus in pieces.** A game's menu is never hung in front of the face whole: the adapter describes
it as groups of pieces (`HudGroup`: the parts that matter, cut out of its UI) and the runtime floats
each group where it belongs, world-locked once placed (placed again when its thing moves, or after
the head has walked 0.8 m from where it was; beside a thing, never more than 40 degrees round):

| Anchor | Where |
|---|---|
| a thing in the world (a chest, a merchant, a speaker) | at its distance (0.9 to 1.4 m), 10 degrees under the eyes, beside it only where it would cover it; or over or under it (a speaker's name, their line) |
| the player (their own menus) | 1.1 m ahead, 8 degrees under the eyes; it comes round after a turn of 65 degrees |
| a pop-up (a question, a count) | 0.8 m ahead where the eyes look |
| over a view (`ANCHOR_VISTA`: a title screen in the world) | 5 m out towards the view's subject, beside it, its middle 6 degrees under the horizon, fixed in the game's world |

A group's pieces keep the game's own arrangement, lie on a cylinder round the eye, and are sized
from the game's own text (`HudLayout::text_height`) so a line reads at `ui.menu_text_deg` (1.5
degrees) wherever it floats. With real alpha (`HUDL_ALPHA`) and the game's picture under the menu,
the world, the body and the hands stay, and the pieces are drawn in the world (before the body, so
the hands are in front of them), the edges of those it says were cut out (`HUDE_SOFT`) soft. A piece
the adapter says is a page (`HUDE_SOLID`) is opaque wherever the game drew half or more of it,
whatever coverage its blending left in the HUD texture's alpha. The laser picks among the pieces
that take it (`HUDE_POINT`); where it hits is the game's cursor (`Intents::cursor_uv`), so the game
keeps its own hover, selection and clicks; the dominant stick scrolls (`Intents::scroll`, the game's
mouse wheel). The runtime's own settings float beside a game menu only when the layout asks
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
turned to the eyes on the thing (`HELD_FACING`), or a sheet held at its middle a hand's depth in
front of the hand and spread between both hands when the other takes a side (`HELD_SHEET`: a map).
It follows the hands every frame (steadied by a one-euro filter while the hand is nearly still),
sized so the game's text reads at `menu_text_deg` at `ui.held_distance` (0.45 m: nearer is bigger,
as with a real book), and is drawn with the body in its depth (`rustvr_avatar::Sheet`): a hand in
front of a page covers it, the fingers behind a sheet are hidden by it. The free hand's grip near
the pages holds them, and moving it across turns them, a page for every few centimetres, leftwards
onwards (`HELD_PAGES`: `Action::PageNext`, `PagePrev`, a tick in the hand); its grip on a sheet's
face drags its content (`HELD_DRAG`: the game's cursor held down where the hand is); and its index
fingertip presses the game's buttons on a held piece (there's no laser on what's held: it moves to
the free hand only for the floating pieces). Things that open (`OBJF_OPENS`, a book) open and shut
with the other hand's grip on them (their state's `shots`); a kept thing (`OBJF_KEPT`, a map) goes
back to its slot when let go. The hands keep going under such a menu (the rig isn't paused, the
game is), unless the game says it's paused (`GS_PAUSED`).

## 6e. Cards of a game's own interface

A game can describe a small card of its own interface to show over one of its objects (`Cards`,
`crates/runtime/src/game_cards.rs`): the pieces of its own menu art (a texture from the asset cache,
the part of it, where on the card) and lines of text in its own bitmap font (`rustvr_asset::Font`:
each character's glyph in an atlas, laid along the line as the game lays text: the space before it,
its width, the space after, its top over the baseline; words wrapped at the piece's width), in its
colours. The runtime paints each card (egui, into a texture of its own) whenever its revision
changes and floats it over its object (0.26 m wide, 8 cm above it), turned to the eyes, fading in
over 0.2 s and out over 0.3 s. Oblivion's alchemy bench shows what the bowl would make this way, in
its alchemy menu's own parchment, icons and fonts. Two more cards follow what a hand holds: the
runtime's own (`item_card.rs`), from what the game says of the thing (`ItemInfos`: its name and
count, its numbers, whether taking it is stealing), beside a thing in the hand, flying to it or under
the fingers in a pocket; and the game's own card of it (`HUD_HELD`, `held_cards.rs`), cut from its
HUD while it was of that hand's thing, so both hands have theirs.

A card can also be a page (`OBJF_SHEET`): its object is a thin box the hands take up and put down,
and the card is drawn on its face (`rustvr_avatar::HeldModel::sheet`, the card's texture on a quad,
its paper on the back) with the held things, in their depth and the game's lighting, so a finger
shows over it. The hands and fingers go through a lying page (pushed, it would only be squeezed
between a hand and its table); an empty hand's index fingertip tapping its face is a touch
(`rig/sheet.rs`: down within 1.2 cm over it or 8 mm through it, felt as a tap; lifted 2.5 cm off
again, counted in the page's state with where on the card it came down; the hand taking the page up
meanwhile, nothing). Oblivion's alchemy bench leaves a note of the game's scroll this way, in its
handwriting, listing what the player can make and the steps of the recipe touched.

## 6f. Melee

Melee is physical (`docs/MELEE.md`). The runtime holds a game's melee weapons on a hand joint (their weight their own, two hands and leverage, hands sliding along
handles; a hand near a handle goes onto it at the grip it's made for, as a gun's grip locks on),
judges every contact by the impact's speed and angles against each damager and the damage tables
by damage type, material and tier, keeps blades stuck in what they went into (rocking, sliding
and running through), cuts limbs off at the body's joints, kicks with untracked feet, and does the
extras: throwing, telekinesis (on things and on bodies), slowed time, imbuing,
breakable things, climbing by the hands and the climb jump, and an optional stamina
(`rig/melee.rs`, `rig/embed.rs`, `rig/telekinesis.rs`, `rig/climbing.rs`, `rig/breaking.rs`,
`kick.rs`, `powers.rs`, `combat.rs`, with the rules in `crates/melee`). A game may ask for the melee
buttons (`RULES_KICK`): the kicks (the movement stick clicked) and the climb jump work only then.
Telekinesis and stamina are off unless the settings or the game turn them on (`[melee]`). The game
describes its weapons (`MeleeWeapons`), lists what its creatures hold (`ActorItems`), says its
rules (`MeleeRules`), and is told the hits (`CombatEvent`) and the player's side (`RtMelee`); its
creatures, their health and their brains are its own. A game that writes only the combat tables
(`Actors`, `MeleeTable`: Oblivion's adapter on main) has its blades judged by those. The test range
plays it all: ragdoll fighters with their pins, balance, reactions and brains, a
bench of dummies in armour, a melee rack, a block to climb.

## 6g. Gore

`crates/runtime/src/gore.rs` makes the blood of the player's blows and the creatures' stumps;
`rustvr-avatar`'s `gore.rs` draws it. The game says what its blood looks like (`BloodTable`: its
own decal, splatter, drip and droplet textures per kind), which creature bleeds which and where its
stumps are (`GoreBodies`), and when one loses a limb (`GoreEvents`). Whether an edge cuts a limb
off is the same rule for every game (`rustvr_protocol::severs`): only a clean slash or chop, only
through a creature that dies of it or is already dead, and only when the cut's power reaches the
limb's toughness; the adapter asks it with its game's numbers once the game has landed the hit.

- **Wounds:** a clean hit sprays from where the blade met the body, as hard as it hit (a slash fans
  along the blade's way, a stab jets back out, a blunt blow spatters), then bleeds a few seconds
  where it is on the body part.
- **Stumps** pump at a heart's beat, each pulse weakening over half a minute into a flow and then
  drips. The severed piece is the adapter's prop, simulated by the rig: it leaves the body at half
  the part's speed, pushed off the blade at 3 m/s, and bleeds from its own cut.
- **Blades** carry what they cut through and drip from their lowest wet point until they dry.
- **Drops** fly with gravity and air drag, at most 3072 (the oldest go first), collided with the
  world's static collision by 160 ray casts a frame shared between them, and with the player's
  hands, body and held blades. A drop that lands becomes a decal (streaked when it hits at a
  slant), joins a pool on a floor, or runs down anything steep, leaving a streak. At most 640
  decals (pools last to go); they dry over minutes and don't fade. Fine mist goes with every spray.
- **Drawn** over the game's picture with its depth: decals projected onto whatever the game drew
  inside them, caps of the game's flesh on stumps, drops as sprites stretched along their flight,
  and splats on the player's body and what the hands hold, given to the body's own shader.

## 6h. Swimming

`crates/runtime/src/swim.rs`, for a game whose adapter says where its water is (`WaterState`: the
surface's height, the game's swim speed, the breath left, the water's colour). The game decides
when the player swims; the runtime turns the hands in its water into the velocity the game moves
them at (`Intents::swim`), as Freediver: Triton Down and Lone Echo do it. The grip squeezed with a
hand under the surface holds the water there, and pulling that hand carries the swimmer the other
way, the hands holding water making one stroke together; let go, the swimmer glides on (the drag
takes it out over a few seconds) while the hand reaches out again. Any stroke in any direction
works; a stroke pulls once it's 10 cm from where it started; light strokes swim well (the pace
grows with the hands' speed to the 0.4th power); a push back against the glide stops it. The
game's swim speed scales the pull, and no stroke drives the swimmer past 4 times it. The hands'
speed is measured against the head (walking about the room swims nothing), and a hand holding
something doesn't swim. The strokes never turn the player; at the surface the game holds them up,
and under water a still swimmer hangs where they are. A hand catching the water pulses and buzzes
as it pulls; with little breath left the heart beats in both hands. The body floats from the head
and kicks (`crates/ik`'s `swim.rs`). Settings: `input.arm_swimming` and `input.swim_ease` (the
Controls tab's "Swim with your arms" and its stroke strength).

## 6i. The desktop view

A window on the monitor for recording and streaming, the same for every game
(`crates/runtime/src/spectator.rs`); nothing in it ever reaches the headset.

- **The camera** sits between the eyes, steadied as a flat game's would be (Half-Life: Alyx's
  spectator camera, LIV's smoothed first person): the head's turns eased (`smoothing`, a 0.1 s
  half-life), never more than `max_lag` (8 degrees) behind, the horizon kept level. Its field of
  view is a flat game's, 105 degrees across a 16:9 window (1600 x 900, fullscreen by default; F11
  switches), and it narrows to 65 while the player aims. The game's picture is the aiming eye's
  (`eye`, right by default), the other's where it doesn't reach; everything the runtime draws is
  drawn again from the camera.
- **The bob** (`view_bob.rs`, `[desktop_bob]`): while the stick or the game moves the player, the
  camera bobs, the game's own (`ViewBob`, from its adapter) or the runtime's (steps at the walk's
  pace, a drop at each footfall), or not at all. The headset's view never bobs.
- **The studio** (`spectator_fx.rs`, the Studio tab, `[studio]`): looks (clean, cinematic,
  hyper-real, action, vintage) and the effects they set: snap turns, snap crouches and recentres
  turned through instead of cut (on by default, 160 degrees a second), the picture's edges stretched
  over the gaps a fast turn leaves, the game's HUD where a flat player has it, motion blur, a view
  wider at speed, shake with hits and shots, handheld sway, a chest mount, depth of field, a lens
  (fisheye, colour fringes, vignette, glow, grain, sharpen) and colour (exposure, contrast,
  saturation, temperature, tint, a grade, a LUT). The research is in `docs/design/desktop-fx.md`.
- **OBS in the headset** (`obs_view.rs`, `ui.obs_view`, off by default): the window of the OBS
  running on the machine, captured with Windows Graphics Capture, on a panel only the headset sees,
  over the back of the left wrist when the player looks at it (the left hand empty), or pinned in
  front with F9.
- **The framework view** (`physics_view.rs`): both sticks clicked together swap the game's picture
  for the world as the runtime's physics has it (the game's collision, its props' convex hulls, the
  runtime's objects, the palms and fingers, each its own colour; a gun as boxes, `blockout.rs`), to
  see what the hands are really touching. The desktop window shows it too.

`RUSTVR_DESKTOP_VIEW=0` keeps the window shut (tests on the emulated headset); the built-in
simulated headset never opens it.

## 6j. Voice

Talking to the game's characters out loud: speech to text, an LLM, and each character answering in
a voice cloned from the game's own lines, through the game's own lip sync and sound. It's an
optional plugin for power users, a process of its own (`crates/voice`, `rustvr-voice.exe`), not in
the usual build (`-p rustvr-runtime -p rustvr-range -p rustvr-xrsim`): it's built with
`-p rustvr-voice` (a `cargo build --workspace` builds it too, as a workspace member). It depends
on the user's public repository `chasmlol/m8` (branch `rustvr`), the brain: hearing, the LLM, the
cloned voice (Breeze), every character's memory.

- **The runtime** (`crates/runtime/src/voice.rs`) starts `rustvr-voice.exe` from beside its own
  exe while the Voice setting is on and a game is connected, with `--mapping`; stops it when Voice
  is turned off or the game has been gone a minute (a loading screen isn't long enough); starts it
  again when the settings it takes at start change (the mic, acting out, barging in, the pause,
  streaming), and no sooner than 10 s after it stopped. Its output goes to `voice.log` beside the
  runtime's log.
- **The plugin** opens the mapping by name and touches only the voice regions (§4): the adapter
  says who's around, who the player is talking to and who they are, in words from the game's data
  (`VoiceScene`), and what happened (`VoiceEvents`: saves and loads, since each character's memory
  goes with the saves). It answers with what it heard and the lines to say (`VoiceLines`); a line
  either streams as it's made, its voice and mouth shapes in `VoiceStream`'s rings, or comes
  sentence by sentence as files (`<n>.wav`, and `<n>.lips`: FaceGen's 16 phonemes over time) that
  the adapter turns into what its game plays (Oblivion: its own `.lip` beside the WAV). A voice is
  cloned from the game's own lines that the adapter writes into a folder per voice. Its brain page,
  http://127.0.0.1:8090, shows every prompt and line.
- **Its config** is the player's own `voice.toml` in the runtime's folder (written with defaults on
  the first run): the servers m8 calls (speech to text, the cloned voice, the LLM) and, for a hosted
  LLM, its key, as `api_key` or the environment variable `api_key_env` names (`OPENAI_API_KEY` by
  default). The key is never in a repository. The servers are the player's to run (Oblivion's
  repository has a script that starts them, `clips/voice-servers.ps1`).
- **Settings** (`[voice]`, the Voice tab): on (off by default), the microphone (the headset's own,
  else the default), the characters' volume (2.5 times the game's voices), acting out how a line is
  said, streaming, talking over them to cut them off, the pause that ends what the player says
  (0.3 s), and simple voice commands (`commands`, on: the game's adapter listens with the system's
  own speech recognition, no servers, for a menu's words; `RT_VOICE_COMMANDS`).

A game with no voices of its own can also have its creatures cry out (`EV_VOICE`: a grunt as one's
hit, a shout as it swings, its last breath, a scream as it falls): the runtime plays the pack's or
its own synthesised `grunt`, `shout`, `death`, `scream` ... at the creature, one voice each.

## 6k. Settings and the menu

**Settings** (`crates/runtime/src/settings.rs`) are `settings.toml` in the runtime's folder
(`%LOCALAPPDATA%\RustVR`, or `--settings`, or `RUSTVR_DIR`), with `profiles/<adapter>.toml` on
top: the adapter's name selects its game's profile, which overrides key by key. Both are read again
whenever they change on disk; a missing file or key keeps the defaults. The in-headset menu saves
each change as it's made.

| Section | What it holds (defaults) |
|---|---|
| `[vr]` | `render_scale` 1 (supersampling), `world_scale` 1, `ipd_scale` 1, `reproject` on, `predict` on, `headset_depth` off, `refresh_hz` 0 (the headset's highest), `overscan` 0 degrees (at most 20) |
| `[comfort]` | `turn_mode` snap, `snap_angle` 45, `smooth_turn_speed` 120, `move_relative` head, `roomscale` on (`roomscale_limit` 0.6 m), `head_bob` off, `camera_anims` on, `height_offset` 0, `vignette` 0.5 (`vignette_aperture` 0.7, `vignette_feather` 0.2, `vignette_ease` 0.3 s), `wall_fade` on, `seated` off, `real_height` off |
| `[input]` | `dominant_hand` right, `stick_deadzone` 0.08, `two_handed_aim` on (`two_handed_grab_dist` 0.25), `gestures` on, `manual_reload` on (`reload_style` full), `melee_swing` on (`melee_speed` 2.8), `ability_flicks` on (`flick_speed` 2), `physical_crouch` on (`stealth_crouch_amount` 0.08), `arm_swimming` on (`swim_ease` 1) |
| `[weapon]` | the glued gun's `offset`, `pitch`, `yaw`, `roll`, `scale` (per game, in its profile) |
| `[scope]` | `enabled`, `resolution` 768, `lens_size` 0.026, `magnification` 4, `force`, `lens_back`, `eye_relief`, `every` 1, `reticle` 2 |
| `[ui]` | menu and HUD distance and size, `hud_follow_head`, `hud_deadzone_deg` 20, `hud_catchup` 0.15, `split_hud` on, the captions (1.5 m, 13 degrees under gaze), the ammo and status sizes, `obs_view` off, `menu_text_deg` 1.5, `menu_opacity` 1, `menu_scroll` 10, `held_distance` 0.45 |
| `[interaction]` | the rig: `physics_hands` on, `ghost_hands` off, `grab_transition` 0.15, `grip_press` 0.6 / `grip_release` 0.4, `grip_toggle`, `auto_holster` off, `gravity_gloves` on (`glove_range` 10, `glove_flick_speed` 1.2, `glove_catch_radius` 0.5), `easy_mag` on, `weight` 0.5, `mechanism_stiffness` 1, `throw_scale` 1, `creature_strength` 1500 N, `grenade_arming` pin, `knife_assist` on, `virtual_stock` off, `laser_sight` off, `game_recoil` on with its scales (`recoil_one_hand` 3, `recoil_unbraced` 1.7) |
| `[body]` | `visibility` full, `height` 0 (measured), `arm_span` 0 (automatic), `hide_head` on, `game_lighting` on, `tracked_elbows` on, `hand_size` 1 |
| `[sound]` | `enabled`, `volume` 0.8, `shots`, `game_sounds` |
| `[archery]` | `quiver` both, `crosshair` off, `fit_draw` on |
| `[riding]` | `turn` smooth (or steps of `turn_step` 30), `vignette` 0.5, `fade_on_off`, the reins' `rein_dead` 0.05, `rein_travel` 0.30 and `rein_pull` 0.15 m, `cruise` 0.7, `spur_fade` 8 s, `flick`, `coast` 2.5 s, `stride_haptics` |
| `[mirror]` | `distance` 1.2, `width` 0.95, `height` 2, `lift` 0.02, `gesture` on, `game_world` on |
| `[holsters]` | `mouth_time` 0.5, `read_time` 0.8, `coat_time` 0.3, `edit_hold` 1.5, and each game's slots as the player set them up (`[[holsters.games.<game>]]`) |
| `[melee]` | `strength` 1, `stamina` off (`stamina_time` 12, `stamina_refill` 6), `slow_time` on, `telekinesis` off, `climbing` on, `easy_dismemberment` on |
| `[voice]` | `enabled` off, `mic`, `volume` 2.5, `act_out`, `barge_in`, `pause` 0.3, `stream`, `commands` on (§6j) |
| `[spectator]` | the desktop view: `enabled` on, `eye` right, `fov` 105, `aim_zoom` to `aim_fov` 65, `smoothing` 0.1, `max_lag` 8, `roll_lock`, 1600 x 900, `fullscreen` |
| `[desktop_bob]` | `mode` game (or generic, off), `strength` 1 |
| `[studio]` | the desktop view's look (clean) and effects (§6i) |

**The menu** (`crates/runtime/src/menu.rs`, egui) floats beside the game's pause menu (beside a
menu in pieces only when its layout asks, `HUDL_SETTINGS`), or in front of the player when no game
is connected; the laser
points at it, the trigger clicks, the dominant stick scrolls. Its header says the game, the backend
and the frame rate. Tabs, in order:

| Tab | What's on it |
|---|---|
| Help | every control and feature (it opens on this) |
| Comfort | turning, walk direction, roomscale, duck to crouch and the stealth crouch amount, height offset, following the game's scripted camera and its bob, the vignette, the wall fade, and riding |
| Hands | physics and ghost hands, the grab, grip toggling, holstering, the gravity gloves, magazines, the virtual stock, recoil, the laser sight, weight, mechanisms, throwing, strength on creatures, grenades, knives, eating and reading, the holsters' editor, the quiver and the bow, and the sounds |
| Voice | talking to the characters, voice commands, the microphone, their volume, acting out, streaming, barging in, the pause |
| Body | height (seated, real height, measure it), arms (span, measure them), what's shown (hands, arms, full body), the game's lighting on it, tracked elbows, hand size, and the mirror |
| Controls | the weapon hand, two-handed aiming, gestures, reloading by hand or by slap, ability flicks, melee swings, the stick's dead zone, swimming |
| Weapon | the glued gun's offsets and angles, picture-in-picture scopes and their refresh |
| Display | supersampling, world scale, moving the game's picture, prediction, the headset's depth, refresh rate, overscan, the HUD and subtitles, the menu, the desktop view, OBS in the headset |
| Studio | the desktop view's looks, camera, bob and effects (§6i) |

Below the tabs: Recenter, Save, and with a game connected Holsters (the game's editor of the body's
slots).

## 7. Adapters

An adapter is small because the SDK (`crates/adapter`, `rustvr-adapter`) does the shared work. Per
game it:

1. **Connects** with `Adapter::new(gpu, AdapterConfig::new(name))` (the name selects the runtime's
   settings profile; the mapping is `RUSTVR_LINK`'s, else the default) and calls `poll` every game
   frame: with no runtime running it returns nothing and the game stays flat. `wait_frame` is the
   lockstep wait; `connections` changes when the runtime restarts, the cue to register objects and
   collision again.
2. **Anchors**: gives the game's eye position with and without bob (`Anchor::eye`, `base_eye`), its
   look yaw, and whether a scripted camera is running. `adapter.tracking.update(&frame, &anchor,
   now, dt)` (`TrackingMapper`) returns the `View`; `set_lead` says how far the view stands ahead of
   the game's body, `carry_turn` turns the view with a mount.
3. **Renders stereo** with `View::eye_view` + `math::projection` per eye, widens culling
   (`math::combined_fov`), and keeps the HUD out of the 3D image (into the HUD texture), into the
   slot `begin_slot` gives it, then `submit`s the slot with the poses it used. This is the per-game
   (or per-engine) part the runtime can't remove.
4. **Drives the game:** look angles from `View::look_yaw`/`look_pitch`; actions from `Intents`
   (press counters via `actions::Presses`); the gun's viewmodel at `View::weapon_to_world`; shots
   from `View::point(sight_origin)` along `View::dir(aim_dir)`. `Basis` converts the game's units
   and axes.
5. **Reports** `GameState` (`write_state`): menu, loading, crouched, eye height, zoom, the gun's
   `WeaponRig`, its water; and its own first-person bob (`ViewBob`, the eye with bob against the eye
   without, in its camera's frame), which the runtime's desktop view shows when the player picks
   the game's own bob there. The headset never takes it.
6. **Optionally hands its objects to the rig:** registers guns, magazine templates and props
   (`Adapter::write_objects`; `rustvr_guns::desc` builds a gun's), streams its static collision
   (`push_collision`), gives its own simulated objects' poses and convex hulls
   (`write_game_poses`, `write_game_hulls`) and takes the hands' pushes on them
   (`read_game_pushes`), draws objects where `read_object_states` says they are, fires a shot for
   each increment of a gun's `shots`, and writes linear depth (`TGT_DEPTH`) so the player's body is
   hidden behind its world. Its hands on its guns and their recoil go in with `write_hand_poses` and
   `write_kicks` (`kicks` turns an extractor's kicks into weapon space). Games that don't fall back
   to the gun glued to the firing hand (`Intents`).
7. **Optionally hands over its own sounds** (§6c): writes a sound pack from the game's files (its
   extractor) and publishes it with `Adapter::set_sound_pack`
   (`local_sound_pack` finds it): the folder, each object's sound set, and `SND_GAME_SHOTS` /
   `SND_GAME_HANDLING` / `SND_GAME_INVENTORY` for what the game already plays itself.
8. **Optionally writes the rest of what the runtime can use**, each region on its own: its HUD's
   layout (`write_hud_layout`, §6d), its lighting (`write_game_lighting`), its creatures and combat
   (`write_actors`, `write_actor_bodies`, `write_melee`, `write_spells`, `write_overlays`, and back
   `combat_events`, `read_casters`, `read_creature_grabs`), the melee
   (`write_melee_weapons`, `write_actor_items`, `write_melee_rules`, `read_rt_melee`), the player's
   body and face (`write_avatar`, `write_face`, `read_face_sculpt`, `read_body`), its inventory and
   the body's slots (`write_inventory`, `read_body_slots`, `write_slot_edits`), pockets
   (`write_pockets`, `pocket_events`), bowls, cards and item cards (`write_receptacles`,
   `write_cards`, `write_item_infos`), bows (`write_bows`), mounts (`write_ride`, `read_rt_ride`),
   gore (`write_bloods`, `write_gore_bodies`, `push_gore_event`), the voice plugin's scene
   (`write_voice_scene`, `push_voice_event`, `read_voice_lines`, `take_voice_stream`,
   `set_voice_stream_played`), and events (`haptic`, `rumble`, `blast`, `impulse`, `game_shots`,
   `mirror_cast`, `mirror_hit`).

Adapters can be written in any language: Rust (hook-and-offset games, using `retour`: Oblivion's,
a 32-bit `dinput8.dll` built for `i686-pc-windows-msvc`), C++ (games with C++ SDKs,
using the generated header), or C# (Unity).

## 8. Testing without a headset or a game

- **Unit tests in the crates** (every one but `crates/melee`):
  - protocol: seqlock, triple buffer, event ring, wake-ups, the C header in step;
  - shared textures across devices;
  - tracking maths;
  - every mechanic and the rig (`rig/tests.rs`);
  - panels and lasers;
  - settings profiles.
- **Scenarios:** `rustvr --sim all --launch-range` (also run by `cargo test -p rustvr-runtime`'s
  `tests/scenarios.rs`). A scripted player in the simulated headset plays against the test range
  over the real protocol and real shared textures. Checks look at the runtime's state and the
  range's reported stats (shots, hits, reloads, crouching, gestures), and each run saves PNGs of
  exactly what the headset would show into `captures/<scenario>/`. The mixer runs offline in
  scenarios, so the checks see which sounds played. `rustvr --list` lists them all; `all` leaves
  out `live`, which plays a real game and runs only by name with `--attach`.
  - Legacy mode (`scenario.rs`, the range started with `--legacy`: the gun glued to the hand):
    - `smoke`: connect, look around, the game picture reaches both eyes every frame.
    - `aim`: raise the gun to the eye, aim down the sights and hit the target 10 m ahead.
    - `two_handed`: grab the foregrip and steer the barrel with the off hand.
    - `reload`: eject, grab a magazine at the hip, seat it, rack the charging handle.
    - `reload_slap`: fire some rounds, then slap a magazine in with the off hand.
    - `melee`: a fast punch is one strike; a slow reach isn't.
    - `abilities`: flick up, down, left and right: four abilities, no melee.
    - `crouch`: duck in real life; the game crouches without lowering the view twice.
    - `gestures`: grip at the chest, face, top of the head, hip and shoulder.
    - `turn`: one snap turn (the configured angle), applied by the game.
    - `roomscale`: walk half a metre; the game body follows the head.
    - `menu`: pause, switch to smooth turning in the runtime menu, click the game menu, resume.
    - `scope`: aim through an optic; the scope view renders into the lens.
    - `async`: the headset at 90 Hz, the game at 45 frames a second: the newest picture is shown.
    - `void`: no game; the runtime shows its own room and menu.
    - `live`: an attached game: look around for 12 s.
  - Physical mode (`scenario_physical.rs`, the range's objects in the runtime's hands):
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
    - `sword`: a chop through the dummy hits; a slow touch doesn't;
    - `stab`, `stab_floor`: a thrust into the dummy (or the floor) stays in, drags it, and comes
      back out;
    - `spell`: charge the fire bolt and cast it at the dummy;
    - `gloves`: point at a can on the far table, flick it to you and catch it;
    - `valve`: turn the free-running valve a quarter turn;
    - `two_hand`: the support hand on the foregrip steers the rifle;
    - `body`: the runtime solves and draws the body; the wrists follow the hands, fists close;
    - `short_player`: a player shorter than the character stands on the game's floor, legs straight;
    - `seated`: a seated player is drawn standing and never crouches;
    - `vignette`: walking closes the vignette in, stopping opens it;
    - `wall_fade`: the head inside the table fades the view out;
    - `wall_hand`: a hand pushed into the table stops at its surface (a ghost shows the real one);
    - `grip_handle`, `grip_handguard`, `grip_gun_barrel`, `grip_hand_to_hand`: a sword by the
      pommel's end with the other hand on the blade; the handguard taken again further forward; the
      rifle held by its barrel (the trigger does nothing); a bottle passed from hand to hand.
- **The range's own renders:** `cargo test -p rustvr-range` also draws the guns as the rig holds
  them (`tests/rig.rs`, `captures/rig/`, [RIGGING.md](RIGGING.md) §6) and the guns' convex hulls
  (`tests/hulls.rs`), and its effects close up and from the side (`tests/fx.rs`, `captures/fx/`: a
  shot's flash and tracer, a casing on the floor, holes in a plate, an explosion's flash, fire and
  smoke), with checks that each one shows. The runtime's own render tests draw hands closing on
  things (`tests/grasp_render.rs`, `captures/grasp/`) and pressing (`tests/press_render.rs`,
  `captures/press/`).
- **The emulated headset:** `crates/xrsim` is an OpenXR runtime with no hardware behind it
  ([XRSIM.md](XRSIM.md)). The runtime, or a game through it, runs its real OpenXR path on it. A
  script drives the head and controllers, and captures save what each eye would see
  (`tools/xrsim/bench_framework.toml` runs `rustvr-play` and its range flat out). A game adapter's
  repository runs its game on it with one script (Oblivion's `tools/xrsim_oblivion.ps1`).
- **Nothing opens a window.** The scenarios launch no game, Steam, SteamVR or Virtual Desktop, and
  the built-in simulated headset opens no desktop view. The range renders offscreen, and the
  range the scenarios start runs at below-normal priority; builds take at most 8 jobs
  (`.cargo/config.toml`).

## 9. Test content

The test range runs in **physical mode** by default. Its guns are its own, made in code
(`content.rs`): a rifle and a scoped rifle lie on a table at your right hand and a pistol starts
on your right hip; an armory bench behind you takes any more rigged guns, side by side, pistols
last and the rest grouped by caliber. There are bottles, cans, crates and a barrel to knock about
on the long table ahead and around, some of them breakable (with their debris, `debris.rs`). Every
gun's own magazines come from the ammo pouch while it's in your other hand (guns that take the same
magazine share one). Throwables (`crates/range/src/throwables.rs`): two frag grenades in the chest
pouches and a knife on the belt; two more frags, two knives, an arming sword and a dagger on a table
at your left hand. The range is the game for them: it makes the explosions (effects, a blast that
hurts the target plates within 5 m and pushes the props within 6 m; the grenade back on its table
or in its pouch 3 s later) and counts knives stuck in the plates. Six target plates down the lane,
a board of seven valves on your right (`content.rs`), a bow on your back with its arrows from the
runtime's quiver (`archery.rs`), and two spells, a fire bolt and a shock (`combat.rs`).

The range is also the game for the melee (`--arena range|rack|arena|bench|climb`
starts the player in one of its areas, `arenas.rs`): two training dummies near the start, a melee
rack (dagger, mace, axe, longsword, maul, shield, spear; `arsenal.rs`), an arena of fighters in
waves from easy to hard with their brains and ragdolls (`arena.rs`, `brain.rs`, `ragdoll.rs`), a
bench of dummies to skewer and dummies in leather, chainmail and plate, a wooden post and a table of
weapons, and a 2 m block to climb. It asks for the melee buttons (`RULES_KICK`) everywhere but the
`range` area.

The range also shows what a game shows when its guns fire (`crates/range/src/effects.rs`): a muzzle
flash (a sprite facing you and one along the barrel, two frames)
and a point light on the gun, hands and tables; gas and smoke; a tracer streaking to the hit (its
head glows, since from behind your own tracer is a receding dot); a hole where the bullet hit
(`hits.rs` casts it against the room, the plates and the loose objects; holes ride with the object
they're in, at most 64, the oldest fading first), sparks off metal, dust and chips off wood and
stone, a push on a loose object (`EV_IMPULSE`); and a casing from the ejection port (the right of
a rifle's receiver, the top of a pistol's slide), out right, up and a little back with a spin,
bouncing on the floor and tables and fading after 6 s; a bolt or a pump throws its case as it's
worked. All of it is the range's own cheap sprites (at most a few hundred), drawn after the world
with depth testing and no depth writes.

Its other options: `--legacy` (the current gun glued to the firing hand, driven by intents, which
the legacy scenarios test; `--weapon <name|scoped|first>` picks it), `--fps <n>` (caps its frame
rate when the runtime isn't in lockstep), `--table <key>[,<key>...]` (those guns nearest on the
table) and `--arena`.

The range needs nothing from a game: no sound pack either, so the runtime plays its own
synthesised sounds for it. What a game's extractor reads out of the user's own install (its models,
textures, recoil and sound pack, in `crates/asset`'s format) goes into `local_assets/<game>/`, at
the repository root, or in a folder above it (a game adapter's, with this repository as its
`framework/` submodule), or where `RUSTVR_ASSETS` says. **`local_assets/` is git-ignored and never
committed or redistributed.**

## 10. Phased plan

| # | Phase | Done when |
|---|---|---|
| 0 | **Loop** | Protocol, SDK, runtime, sim headset and test range; all scenarios pass headless |
| 1 | **A game** | Oblivion (1.2.0.416: Gamebryo, 32-bit, Direct3D 9): its world in the headset with the runtime's player controller in it (as SkyCraft does it: the game supplies the world, the runtime the player); then its collision, creatures and weapons. A Rust `dinput8.dll` on the SDK, in its own repository (chasms-vr-oblivion) |
| 2 | **Headset** | The OpenXR backend runs on the user's headset (a Quest 3 through Virtual Desktop), and the game's picture is moved to every headset frame (§5); the HUD is still drawn into the projection layer: quad layers are to come |
| 3 | **A C++ adapter** | Skyrim SE through CommonLibSSE (C++ on the generated header), reusing SkyCraft's camera and actor code |
| 4 | **Feel** | Done in the runtime: physics hands, objects, holsters, pouches and the body's slots, physical reloads and guns worked by hand, throwing (grenades, knives), gravity gloves, physical melee and telekinesis, creatures held by hand, spells, bows, pockets, bowls, hinges and slides, riding, swimming, gore, full-body IK and an avatar with the game's own body, face and lighting, a mirror, spatial sound and the game's own sounds, comfort, the desktop view, and voice as a plugin (§6 to §6k). Next: tuning on the headset |

## 11. Decisions (2026-10-21)

1. **Rust** for the runtime, the SDK and the game adapters (Oblivion's). C++ only where a
   game's modding SDK is C++.
2. **The runtime is a separate process**, so a game crash doesn't take down the headset and a
   loading screen doesn't freeze the view. Optional parts that would slow its frames (the voice
   plugin) are processes of their own on the same mapping.
3. **OpenXR, not OpenVR.**
4. **The runtime creates the shared textures** (D3D11, typeless 8-bit, keyed mutex). Games on other
   APIs bring their pictures over through D3D11 shared handles: Oblivion's adapter runs the game's
   Direct3D 9 on D3D9On12 and copies each eye through a texture shared with its own D3D11 device.
5. **Runtime-drawn layers are composited into the projection layer** for now, so the simulated and
   real backends draw identically. Quad layers for sharper panels are still to come (phase 2).
6. **Toolchain:** Rust 1.99 is pinned (`rust-toolchain.toml`) because egui 0.37 needs 1.95+.
7. **The runtime simulates the player's hands and objects (Rapier)**, not the game: hands and
   held things feel the same in every game. The game only supplies collision and draws objects
   where they are; its own physics takes over nothing it can't see.

## 12. Risks

| Risk | Mitigation |
|---|---|
| Stereo rendering stays per-game work | The SDK keeps it to "render these two views"; it's per engine where possible |
| Cross-process GPU sync stalls | Triple buffer plus keyed mutexes never contend in steady state; the runtime's GPU work goes before the game's (`SetGPUThreadPriority`), and its pacing log (`pacing.rs`, every 10 s) counts the frames shown late |
| A game slower than the headset | Its picture is moved to each headset frame by its depth, and its walk carried on between its steps (§5); the hands are drawn every headset frame |
| A game can only draw one eye per frame | Supported (alternate eyes into one slot), with synchronized eyes via its frame timer |
| OpenXR runtimes differ | The backend has run on the user's headset through Virtual Desktop; the emulated headset (`crates/xrsim`) runs the real OpenXR path without one |
| Anti-cheat | Single-player games only |
