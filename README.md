# RustVR

One VR player experience that connects into any flat game.

RustVR is a VR runtime written in Rust. It runs as its own process and owns everything about your body
in VR: head, physics hands, a full body (IK), picking things up and throwing them, holsters, the ammo
pouch over your shoulder, guns with real magazines, chambers and slides, two-handed aiming, aiming
down sights, scopes, gravity gloves, interaction sounds and haptics, body gestures, melee, turning,
roomscale and comfort options, menus. Games connect to it through a small **adapter**
that only translates: it maps the headset's space into the game's world, renders the two eyes it's
asked for, and turns what you're doing into the game's own input.

So instead of every VR mod rebuilding hands, aiming and comfort inside its game, every game gets the
same, tested VR experience, and a new game only needs an adapter.

> **Status: early.** The runtime, its player controller (physics hands, full body, physical guns),
> the protocol, the adapter SDK and a test range work end to end and are tested headlessly; the
> runtime has run on a Quest 3 through Virtual Desktop. The Metro 2033 Redux adapter is written but
> not yet run in the game, and doesn't hand its guns to the player controller yet.

See [docs/DESIGN.md](docs/DESIGN.md) for how it fits together.

## Layout

| Crate | What |
|---|---|
| `crates/protocol` | The shared-memory protocol (also generated as `include/rustvr_protocol.h` for C/C++/C#) |
| `crates/runtime` | `rustvr.exe`: OpenXR and simulated headsets, mechanics, menus, compositor, scenarios |
| `crates/adapter` | The adapter SDK every game adapter uses |
| `crates/gpu` | Direct3D 11 helpers: devices, shared textures, shaders, PNGs, egui painting |
| `crates/range` | `rustvr-range.exe`: a fake game for testing (an empty range with targets and guns) |
| `crates/metro-adapter` | The Metro 2033 Redux adapter (`dinput8.dll`) |
| `crates/metro-assets` | `metro-extract`: copies Metro's guns, hands and props from your install into `local_assets/` |
| `crates/asset` | The format of `local_assets/` |
| `crates/ik` | Full-body IK from the head and hands |
| `crates/avatar` | Draws the player's body and hands over the game's picture |
| `crates/audio` | Procedural, spatial interaction sounds |
| `content/rigs` | How the hands hold each game's guns, measured on its models ([docs/RIGGING.md](docs/RIGGING.md)) |

## Building

You need Visual Studio 2026 (C++ tools, for the OpenXR loader), CMake and rustup. The repository pins
Rust 1.99 in `rust-toolchain.toml`.

```bat
cargo build --workspace
```

If `cargo` on your PATH isn't rustup's (a standalone Rust install can shadow it), put
`%USERPROFILE%\.cargo\bin` first.

## Testing without a headset or a game

```bat
cargo test --workspace
target\debug\rustvr --sim all --launch-range
```

The second command runs every scenario: a scripted player in a simulated headset plays the test range
over the real protocol. It picks guns up off the table, aims, fires, ejects and reloads from the
shoulder pouch, racks, draws from and returns to the holster, throws, pulls things with the gravity
gloves, pushes its hand into a table, crouches, makes gestures, turns, walks around the room, uses
the menus and looks through a scope, and the run checks that the game did the right thing.
Each scenario saves what the headset would have shown into `captures/<scenario>/*.png`.
`rustvr --list` lists them. Nothing opens a window.

To use Metro 2033's own guns in the range (optional; you need the game installed):

```bat
target\debug\metro-extract
```

This reads your install and writes into `local_assets/`, which is git-ignored. Never commit or share
those files: they're the game's.

## Running with a headset

```bat
target\debug\rustvr
```

This starts an OpenXR session on your current OpenXR runtime (SteamVR, Virtual Desktop, Quest Link and
so on) and waits for a game adapter to connect. Add `--launch-range` to start the test range with it.

To launch it from inside the headset, add it to Steam as a VR non-Steam game (Steam must be closed, or let
the script restart it):

```bat
cargo build --release -p rustvr-runtime -p rustvr-range
python tools\add_steam_shortcut.py --restart-steam
```

"RustVR" then appears in Steam and in SteamVR's library. It runs `rustvr-play.exe`: the runtime plus the test
range, with no console window. Logs go to `%LOCALAPPDATA%\RustVR\rustvr.log` and `range.log`.

In the range (controls for a right-handed player; swap sides in the menu):

| | |
|---|---|
| Left stick / right stick | Move / snap turn (flick down to crouch, click to crouch) |
| Grip | Grab whatever your hand is on; hold to keep holding |
| Trigger | Fire the gun in that hand; with an empty hand, use |
| A on the gun hand / B on the gun hand | Drop the magazine / drop a locked slide |
| A or X with an empty hand | Jump |
| Grip at your chest or belt with a gun in the other hand | A fresh magazine for that gun (unlimited) |
| Other hand on the barrel, grip | Two-handed: it locks on and slides along the handguard |
| Point at something far away, grip, flick back | Gravity gloves: it flies to your hand |
| Let go of a gun gently at a hip or behind a shoulder | Holster it; grip there to draw it |
| Menu button | Pause; the runtime's menu opens on a Help page listing everything |

The pistol starts on your right hip and the properly rigged guns lie on the table at your right
hand. To reload: drop the magazine with A (squeeze your other hand under the gun to catch it),
grip at your chest for a fresh one, push it up into the gun, then pull the charging handle or slide
back. Set your height and arm span (or measure them) in the menu's Body tab; seated mode is there
too.

Settings live in `%LOCALAPPDATA%\RustVR\settings.toml` (per-game overrides go in
`profiles/<adapter>.toml`) and can be changed from the in-headset menu.

## License

[MIT](LICENSE)
