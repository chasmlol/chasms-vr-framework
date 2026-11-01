# Chasms VR Framework

One VR player that connects into any flat game.

The Chasms VR Framework (RustVR in the code: its crates are `rustvr-*`) is a VR runtime written in
Rust. It runs as its own 64-bit process and owns everything about your body in VR: the head,
physics hands, a full body solved from the head and hands, picking things up and throwing them,
holsters, guns with real magazines, chambers and slides, two-handed aiming, scopes, bows, physical
melee, the gravity gloves, swimming, riding, interaction sounds and haptics, gestures, turning,
roomscale and comfort, menus, and a desktop view for recording.

A game connects to it through an **adapter** that only translates: it maps the headset's space into
the game's world, renders the two eyes it's asked for, reports what the game is doing, and turns
what the player does into the game's own input and actions. The two share memory: a fixed layout,
`rustvr-protocol` (version 17, mapping `Local\RustVR_v17`), with a C header for adapters in other
languages. So every game gets the same, tested VR player, and a new game only needs an adapter.

See [docs/DESIGN.md](docs/DESIGN.md) for how it fits together.

## Games

Each game's adapter is a repository of its own, with this one as its `framework/` git submodule:

| Game | Adapter repository |
|---|---|
| The Elder Scrolls IV: Oblivion | chasms-vr-oblivion: a 32-bit `dinput8.dll`, Oblivion's own hits, spells, blood, menus and models |

## Layout

| Crate | What |
|---|---|
| `crates/protocol` | The shared-memory protocol (also generated as `include/rustvr_protocol.h` for C/C++/C#) |
| `crates/runtime` | `rustvr.exe` and `rustvr-play.exe`: OpenXR and simulated headsets, the player's rig and physics, menus, HUD, the compositor, the desktop view, scenarios |
| `crates/adapter` | The adapter SDK every game adapter uses |
| `crates/gpu` | Direct3D 11 helpers: devices, shared textures, shaders, PNGs, egui painting |
| `crates/range` | `rustvr-range.exe`: a fake game for testing: a room with targets, guns, throwables, a bow, and melee areas with fighters |
| `crates/xrsim` | `rustvr-xrsim`: an emulated headset (an OpenXR runtime) for playtesting any VR app without one ([docs/XRSIM.md](docs/XRSIM.md)) |
| `crates/asset` | The format of `local_assets/`, and the gun rig ([docs/RIGGING.md](docs/RIGGING.md)) and armour marks files |
| `crates/ik` | Full-body IK from the head and hands, with a small learned prior for the chest, hips and elbows |
| `crates/avatar` | Draws the player's body and hands: over the game's picture, or inside the game's frame by its adapter. A game's armour moves as plates where its marks say ([docs/ARMOUR.md](docs/ARMOUR.md); `rustvr-armour` previews them) |
| `crates/guns` | Guns: their parts, magazines and mechanisms, and their collision hulls |
| `crates/melee` | The melee rules: damage by part and armour, cuts, stabs, dismemberment ([docs/MELEE.md](docs/MELEE.md)) |
| `crates/audio` | Spatial interaction sounds: a game's own (its sound pack), or procedural ones |
| `crates/voice` | `rustvr-voice.exe`: an optional plugin for talking to a game's characters out loud (below) |

## Building

You need Visual Studio's C++ build tools and CMake (the OpenXR loader is built from source), and
rustup. The repository pins Rust 1.99 in `rust-toolchain.toml`.

```bat
cargo build --release -p rustvr-runtime -p rustvr-range -p rustvr-xrsim
```

If `cargo` on your PATH isn't rustup's (a standalone Rust install can shadow it), put
`%USERPROFILE%\.cargo\bin` first.

## Testing without a headset or a game

```bat
cargo test --workspace
target\release\rustvr --sim all --launch-range
```

The second command runs every scenario: a scripted player in a simulated headset plays the test
range over the real protocol. It picks guns up off the table, aims, fires, ejects and reloads from
the shoulder pouch, racks, draws from and returns to the holster, throws, pulls things with the
gravity gloves, pushes its hand into a table, crouches, makes gestures, turns, walks around the
room, swings swords, uses the menus and looks through a scope, and each run checks that the game
did the right thing. Each scenario saves what the headset would have shown into
`captures/<scenario>/*.png`. `rustvr --list` lists them. Nothing opens a window.

To playtest a real VR app without a headset, run it on the emulated headset: a script moves the
head and controllers, and the emulator saves what each eye would see. See
[docs/XRSIM.md](docs/XRSIM.md); a game adapter's repository has a script that does a whole run in
its game.

`local_assets/` holds what an adapter's extractor reads out of an installed game (its models,
textures and sounds), for the runtime to draw and play. It's git-ignored: those files are the
game's, never to be committed or shared. The runtime finds it in the nearest folder above its own
exe that has one (a game adapter's repository, which runs its runtime from its own `target/`), or
at `RUSTVR_ASSETS`. The range needs none of it: its rifle, scoped rifle and pistol are its own, and
the runtime plays its own sounds for it.

## Running with a headset

```bat
target\release\rustvr-play
```

This starts the runtime on your current OpenXR runtime (SteamVR, Virtual Desktop, Quest Link and so
on) with the test range connected, and no console window. A game adapter starts it for you with
`--no-range`; `--wait` waits for the headset first. `rustvr.exe` is the same runtime with a console:
`target\release\rustvr` waits for a game adapter to connect, and `--launch-range` starts the range
with it.

To launch it from inside the headset, add it to Steam as a VR non-Steam game (Steam must be closed,
or let the script restart it):

```bat
python tools\add_steam_shortcut.py --restart-steam
```

"RustVR" then appears in Steam and in SteamVR's library. It runs `rustvr-play.exe`. Logs go to
`%LOCALAPPDATA%\RustVR\rustvr.log` and `range.log`.

`RUSTVR_LINK` names the shared memory a game or the range connects on, and `RUSTVR_DIR` the folder
for settings and logs: with both set, several test runs, each with its own range, share a machine
without crossing. `rustvr-play.exe --wait -- --arena <range|rack|arena|bench|climb>` starts the
range in one of its melee areas ([docs/MELEE.md](docs/MELEE.md)).

In the range, the pistol starts on your right hip and the two rifles lie on the table at your
right hand; the throwables' table, the melee rack, the training dummies, the arena's fighters and
a bow on your back are round the room. The menu button opens the runtime's menu on a Help page
listing every control (right-handed; swap the weapon hand in Controls). The essentials:

| | |
|---|---|
| Left stick / right stick | Walk (click: sprint) / snap turn (down or click: crouch) |
| Grip | Grab what your hand is on; hold to keep holding |
| Trigger | Fire the gun in that hand; with an empty hand, use |
| A on the gun hand / B on the gun hand | Drop the magazine / drop a locked slide |
| A or X with an empty hand | Jump |
| Grip at your chest or belt with a gun in the other hand | A fresh magazine (unlimited) |
| Point at something far away, grip, flick back | Gravity gloves: it flies to your hand |
| Let go of a gun at a hip or behind a shoulder | Holster it; grip there to draw it |
| Swing a sword, a mace, an axe | Melee: each hit judged by its speed and angle |

Set your height and arm span (or measure them) in the menu's Body tab; seated mode is there too.

## Settings

Settings live in `%LOCALAPPDATA%\RustVR\settings.toml` (per-game overrides in
`profiles\<adapter>.toml` beside it) and are changed from the in-headset menu: its tabs are Help,
Comfort, Hands, Voice, Body, Controls, Weapon, Display and Studio (the desktop view's looks and
effects). `crates/runtime/src/settings.rs` documents every key.

## The voice plugin

`crates/voice` builds `rustvr-voice.exe`, an optional plugin for power users: talk to a game's
characters out loud and they answer in their own cloned voices, their lips moving with it. It isn't
built by default. Built into the same folder as the runtime
(`cargo build --release -p rustvr-voice`), it's started by the runtime while the Voice setting is
on and a game is connected. It needs speech to text, a cloned-voice server and an LLM: m8
([chasmlol/m8](https://github.com/chasmlol/m8), branch `rustvr`, a git dependency) and its servers.
Its config, `%LOCALAPPDATA%\RustVR\voice.toml`, is written on its first start; the LLM's key goes
there or in the environment variable it names, never in a repository. The Oblivion adapter's
`docs/playtest/voice.md` walks through the whole setup.

## License

[MIT](LICENSE)
