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
                   │    crates/metro-adapter  Metro 2033 Redux (dinput8.dll proxy)
```

Supporting crates:

- `crates/gpu`: D3D11 device, shared textures, shader compile, PNG readback, egui painter.
- `crates/asset`: the cache format for models and textures extracted from installed games.
- `crates/audio`: the runtime's interaction sounds and a small spatial mixer on WASAPI. A game's
  own sounds come first: its **sound pack** (§6c), decoded in the background; a procedural library
  (24 sounds, four variations each, no sample files) is the fallback.
- `crates/ik`: full-body IK from the head and hands (torso, spine, arms with elbow prediction,
  stepping legs), calibration and grip-to-hand offsets.
- `crates/avatar`: draws the player's body and hands over the game's picture, hidden behind the
  game's geometry using its depth.
- `crates/metro-assets`: reads Metro 2033 Redux's archives (`metro-extract`): models, textures,
  recoil, and its sound pack (`sounds.bin`, §6c). Local test content only, never committed.

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
| `Event` ring: haptics, game rumble, recentre requests | adapter -> runtime | SPSC ring |
| `RtBody`: the player's 22 joints, finger curls, hand states | runtime -> adapter | seqlock |
| `ObjectTable`: the objects the player can handle (guns, magazines, props, templates) | adapter -> runtime | seqlock |
| `ObjectStates`: where each object is, who holds it, rounds, shots fired | runtime -> adapter | seqlock |
| Collision ring: static collision (boxes, triangle meshes) for the runtime's physics | adapter -> runtime | SPSC byte ring |
| `HandPoseTable`: the game's own hands on its guns (every finger joint) | adapter -> runtime | seqlock |
| `KickTable`: the guns' recoil as the game authored it (per-shot kicks, keyframed) | adapter -> runtime | seqlock |
| `SoundPack`: where the game's sound pack is, each object's sound set, which sounds the game plays itself | adapter -> runtime | seqlock |

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
| Physics hands | Light palm boxes driven by velocity: 1:1 in free space, stopped by walls and tables, pushing loose objects. A ghost hand shows the controller when one is held back; past 0.7 m they snap back |
| Grabbing | Within 7 cm of an object's surface, grip hysteresis 0.6 press / 0.4 release (optional toggle). Props are held where you touched them; guns snap grip-to-palm, barrel along the aim (0.12 s ease) |
| Rigged guns | A gun's descriptor says how the hand holds it (`grip_rot`: the controller grip pose in weapon space) and where the support hand goes (a foregrip line with a slide range, and `support_rot`). Rigged guns point where the hand does; unrigged ones follow the aim ray. Left hands use the mirror image |
| Weight | Held objects can only speed up and turn so hard, less the heavier they are (the weight setting, squared): a 3.5 kg rifle about 30 m/s^2 and 60 rad/s^2 in one hand (a quarter turn takes a third of a second), two to three times that in two hands; pistols nearly and bottles fully keep up. Both brake into the hand rather than overshoot, and soften to 5% against the world, so they never jitter through walls. Heavy things also throw slower (a rifle about 80% of the hand's speed) |
| Two-handed | The support hand grips the barrel within 14 cm of its foregrip line and locks onto it: it's drawn on the gun, slides along the handguard within the rigged range, and steers the barrel about the firing hand (steadied by a One-Euro filter); pulling it 30 cm away lets go. Letting go of the gun hand hands the gun over. Optional virtual stock: shouldered rifles aim along the shoulder-to-support-hand line |
| Holsters | Both hips (13 cm) and both shoulders (behind the head only), on a body anchor that lags the head yaw by 0.5 s and freezes while a hand reaches into a zone. Let go of a gun gently in a slot to holster it (a swing faster than 1.2 m/s throws instead); a tick tells you you're in one. Auto-return of let-go guns is optional (off) |
| Magazines | Unlimited: grip at the chest or belt front (or any empty slot) while the other hand holds a magazine-fed gun. Firing-hand A drops the magazine, straight into the other hand if it's squeezing under the gun |
| Reloading | Guns keep their own magazine, chamber and slide state. A magazine within 9 cm and 45° of the well seats itself (strict mode 4 cm / 25°), but not the one just taken out until it has left the well; pull the slide or charging handle back 90% of its travel to chamber; B or slamming a magazine drops a locked slide |
| Shooting | Semi or full auto from the gun's state; each shot increments `ObjectState::shots` for the game to fire. **The game's own recoil** when the adapter gives it (`KickTable`, `rig/kick.rs`): each shot plays one of the gun's view kicks and one of its gun kicks, picked for how it was fired (hip or aimed, standing or crouched, first shot / follow-up / long burst). View kicks overlap and add up, so a burst holds the muzzle up and shakes it and it settles when you stop; a gun kick replaces the one playing. The head never moves: on a flat screen the camera's turn takes the gun with it, here it turns the gun about the eye. Metro's come from its camera tracks and fire motions (see `metro-assets/src/recoil.rs`; Kalash: 2.5 degrees and 16 mm a hip shot, about 3 degrees held through aimed full auto). The game's numbers are for its player's hold, a long gun in both hands at the shoulder: off the shoulder it kicks 1.7 times as hard, in one hand 3 times (twice with the stock at the shoulder); a pistol in two hands 0.75 times (the push grows half as much as the turn). The shoulder is generous: the back of the gun within 20 cm of it is fully braced, fading out by 38 cm. Hands tab: on/off, turn and push scales (1 = the game's), the one-handed and off-the-shoulder scales. **Otherwise** recoil follows Escape from Tarkov's (`rig/recoil.rs`): a fast snap and push back, then EFT's hand-rotation process. Each kick leans a random angle off vertical, the first shot of a burst hits hardest, and the return spring is weak while firing, so full auto climbs; from the 4th shot it plateaus and wanders (stable mode). When you stop it returns on a curved, sideways path and settles a little up and off (the after-recoil offset), so you pull it back on target; the offset fades over a few seconds. Per-gun character: `recoil_climb`, `recoil_return`, `recoil_angle`. Two hands give 55% of the rotation and a faster return; a shouldered stock 40%. Fixed 240 Hz springs with seeded randomness |
| Throwing | Release velocity is the peak of the last five frames (x1.1) plus the wrist's spin, so throws keep their speed even if the hand slows before letting go |
| Gravity gloves | Half-Life: Alyx's, with its own numbers (`vr_hand_pull_*` / `vr_gg_*` from its `server.dll`, see `rig::alyx`): point the hand (its aim or its palm) at something within 10 m (a 15° cone from 25 cm behind the hand, held to 20°; nothing nearer than 30 cm); 0.1 s pointed at it takes it (it glows). Grip locks it on: it hops and strains towards you; pointing 60° away, or 0.25 s with the grip let go, drops the lock. A flick (the hand yanked back at 1.2 m/s, or the wrist flicked back at 5 rad/s) throws it on Alyx's arc: 0.6 s in flight out to 2.5 m, 0.8 s at 10 m, launched 20° above the line near and 10° far, gravity eased to as little as half; it steers after your moving hand from 0.6 s out to 0.05 s before arriving (up to 12.7 m/s^2). Caught within 0.5 m with the grip held (a 0.125 s pulse) |
| Feedback | Every interaction is one cue: a haptic pulse and a spatial sound on the same frame. Impacts scale with speed. A cue names its object (the gun, for its magazine, bolt and shots), so the sound is that object's own from the game's sound pack (§6c); the same cue twice in a frame (both hands' pulses for one shot) is one sound |
| Fingers | Curls from the trigger, grip and thumb-rest touch, wrapped round whatever the hand holds (pistol grip, foregrip, magazine, small or large object) |
| Full body | `crates/ik` solves 22 joints every frame from the head and the hands the rig drew (VRIK/FRIK-style: torso yaw from the hands, spine pitch from the head drop, stepping legs, elbow prediction, hands pinned exactly). The avatar (Metro's Miller when extracted, else a mannequin) is drawn between the game's picture and the overlays, hidden behind the game's geometry using its linear depth; the head is cut away in first person. Hands only, arms or full body. The body stands on the game's floor (the game puts your eyes at its own eye height, so the real floor would leave it floating or crouched), its proportions follow your height (set, measured, or the game character's), and its arms follow your arm span: set, measured in a T-pose, or automatic (it grows whenever your real arms reach further). Seated mode draws it standing |
| Comfort | A vignette eases in while the stick moves or smooth-turns you (strength 0.5, 0.3 s); the view fades out when the head pushes into the game's collision; snap turn 45° (0.75 press, 0.25 re-arm, 0.5 s repeat); seated mode; play at your real height or the character's |
| Menu | Opens on a Help page with every control and feature; tabs for comfort, hands, body (height, arms, seated), controls, weapon and display |

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
  `ui_click`, `melee`, `grenade_throw`, `explosion`, ...; `rustvr_audio::Event`) to sounds, in
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
  decoding and resampling everything to the mixer's rate, and unloads it when the game goes. The
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
  sounds, the pouch its ammo pickup. Events are levelled to where the runtime's own sounds sit,
  except shots, which share one gain so the guns keep their loudness relative to each other.

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
    - `mag_reload`: eject, a magazine from the shoulder pouch, seat it, rack the handle, fire;
    - `holster`: draw the pistol from the hip, fire, let go and it returns to the holster;
    - `throw`: throw a bottle down the range;
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
      (the mixer runs offline in scenarios, so the checks see which sounds played).
- **Nothing opens a window.** Nothing launches a game, Steam, SteamVR or Virtual Desktop. The
  range renders offscreen, and builds and child processes run at below-normal priority.

## 9. Test content

The test range runs in **physical mode** by default: the built-in rifles lie on a table at your
right hand, the built-in pistol starts on your right hip, Metro's guns (when extracted) lie along a
long table ahead, and there are bottles, cans, crates and a barrel to knock about. `--legacy` gives
the old behaviour (the current gun glued to the firing hand, driven by intents), which the
original scenarios still test.

`metro-extract` reads the user's own Metro 2033 Redux install (read-only) and writes first-person
guns, hands, a body and props into `local_assets/metro2033/` (`crates/asset` format), and its sound
pack into `local_assets/metro2033/sounds/` (`--sounds` for just that). The test range loads them
when present, and uses built-in guns and the runtime's own sounds otherwise; its built-in guns
borrow Metro's sets (the rifles the Kalash's, the pistol the revolver's). **`local_assets/` is git-ignored and never
committed or redistributed.**

## 10. Phased plan

| # | Phase | Done when |
|---|---|---|
| 0 | **Loop** | Protocol, SDK, runtime, sim headset and test range; all scenarios pass headless |
| 1 | **Metro adapter** | The Metro 2033 Redux adapter runs in the game, and Metro plays as well as the C++ mod or better |
| 2 | **Headset** | The OpenXR backend is tested on the user's headset (Virtual Desktop); the HUD moves to quad layers; reprojection is verified |
| 3 | **Second game** | Metro Exodus Enhanced Edition, same engine family: the new adapter takes days, not months |
| 4 | **Second engine** | Skyrim SE through CommonLibSSE (C++ adapter on the generated header), reusing SkyCraft's camera and actor code |
| 5 | **Feel** | Done in the runtime: physics hands, objects, holsters, physical reloads, gravity gloves, full-body IK and an avatar, spatial sound, comfort (§6b). Next: the Metro adapter registers its guns and collision, melee aimed at actor capsules (needs an actor table, as in SkyCraft), tuning on the headset |

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
