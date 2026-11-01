# The emulated headset (`crates/xrsim`)

An OpenXR runtime with no hardware behind it, for playtesting VR without putting a headset on. A
person or an AI agent can run a VR app on it, steer the head and both controllers from a script,
and look at **exactly what each eye of a real headset would show**. The app doesn't know the
difference: it runs its real OpenXR code. That code covers:

- the instance and the session's states;
- the actions and their bindings;
- the swapchains;
- frame timing and the layers it submits.

The runtime's other test headset (`rustvr --sim`, [DESIGN.md §8](DESIGN.md)) is faster and scripted
in Rust, but it replaces the OpenXR backend. A bug in the path a real headset uses only shows up on
this one. Examples:

- a binding;
- a pose or field of view in the submitted layers;
- a session state;
- something the game sends only in menus.

## Why it exists

A real headset hides what went wrong. The player sees "the menu isn't there" or "the gun is in the
wrong place", and can't say why, or test the fix, without putting it on again. With the emulator:

- every run is repeatable;
- captures show the picture both eyes would have seen, and films show it frame by frame;
- the logs say what the app asked of the runtime and what it submitted.

Mod makers can let an AI agent playtest this way: the agent runs a script, reads the pictures and
logs, fixes, and runs it again.

## How it works

The OpenXR loader picks its runtime from `XR_RUNTIME_JSON` when that is set, before anything
installed (SteamVR, Virtual Desktop, Oculus). `rustvr-xrsim run` sets it for one process only, to a
manifest naming `rustvr_xrsim.dll`. Nothing changes system-wide, and a real headset left plugged in
is untouched.

The DLL implements the core OpenXR 1.0 API, `XR_KHR_D3D11_enable` and
`XR_KHR_win32_convert_performance_counter_time`:

- **Session.** It goes IDLE → READY when created, then SYNCHRONIZED → VISIBLE → FOCUSED on
  `xrBeginSession` (VISIBLE only, while the headset is unfocused). When the script's `duration` is
  up, or on `quit`, it goes STOPPING, and IDLE → EXITING when the app ends the session.
- **Actions.** Bindings for the Touch controllers' profile are checked against their real input
  paths: X, Y and the menu button on the left, A and B on the right, and on both the trigger, the
  squeeze, the thumbstick, the thumbrest, the grip and aim poses and the haptic output. One path
  that isn't there fails the whole suggestion (`XR_ERROR_PATH_UNSUPPORTED`), as on a real runtime,
  and `xrsim.log` lists every binding the app suggested. The trigger clicks past 0.85, the squeeze
  past 0.75. Action states read from the script and are inactive while the session isn't focused.
- **Spaces.**
  - STAGE is the floor at the play area's centre (a 2.5 m square).
  - LOCAL is the same as STAGE. A real headset puts LOCAL at the head's starting pose.
  - VIEW is the head.
  - Action spaces are the controllers' grip and aim poses, with their velocities as the script
    moves them. A controller that's lost tracking (`connected = false`) isn't located.
- **Swapchains.** Textures are made on the app's own D3D11 device, three per swapchain: RGBA8 or
  BGRA8 (sRGB or not) for colour, `D32_FLOAT` for depth.
- **Frames.** `xrWaitFrame` paces to the headset's refresh rate (90 Hz by default) and predicts the
  display a frame ahead (or `latency_ms` ahead, when that's more). `xrEndFrame` records the
  projection layer for captures and films.
- **What a Quest 3's runtime adds.**
  - Poses at any moment: `xrLocateViews` and `xrLocateSpace` play the script on to the time asked
    for, as a real runtime predicts (the runtime gives a game the poses for when its picture shows).
  - The Touch controllers' touch sensors: the trigger once pulled at all, A/B/X/Y and the stick
    while used, the thumbrest otherwise.
  - Each eye's depth with its picture (`XR_KHR_composition_layer_depth`): captures save it as
    `<name>_depth.png` and its centre's distance in the `.toml`.
  - Refresh rates of 72, 90 and 120 Hz (`XR_FB_display_refresh_rate`); asking for one changes the
    pacing and sends the change event.

## Building and running

```bat
cargo build --release -p rustvr-xrsim
target\release\rustvr-xrsim run --dir captures\xr --script my_test.toml -- target\release\rustvr-play.exe --no-range
```

Anything after `--` is the program to run (any OpenXR app using D3D11). It starts on the emulated
headset, and `run` returns the program's exit code when it exits. The program exits when the
script's `duration` is up, because the headset ends the session as if the player closed it.
Without `--script` the headset holds still at the script defaults (below) and never ends the
session by itself. A script that doesn't parse is logged in `xrsim.log`, and the defaults are used.

`run` writes the manifest (`rustvr_xrsim.json` next to the DLL; `rustvr-xrsim manifest` writes it
alone and prints its path), clears an old `control.toml`, and starts the program with
`XR_RUNTIME_JSON`, `RUSTVR_XRSIM_DIR` (the folder) and `RUSTVR_XRSIM_SCRIPT` (the script) set.

`<dir>` (`--dir` on every command; default `RUSTVR_XRSIM_DIR`, else `%TEMP%\rustvr-xrsim`) gets:

| File | What |
|---|---|
| `xrsim.log` | The session: every state change, binding, swapchain, control file, capture and film, with times in seconds from the headset's start |
| `status.toml` | Rewritten every second: the session state, focus, frames, fps, layers submitted, haptic pulses, whether actions are attached, and each submitted view's field of view, position and size |
| `captures/<name>.png` | Both eyes as the headset would show them (see below) |
| `captures/<name>_raw.png` | The images the app submitted, as they are |
| `captures/<name>_depth.png` | The depth the app gave with them, when it did (white near, black at 10 m and on) |
| `captures/<name>.toml` | The numbers: the script time, frames, focus, layers, refresh, the head's pose, and for each eye the layer's field of view against the headset's, its position and image size, and the depth at its centre |
| `films/<name>/NNN.png` | A film's frames, both eyes as submitted (see Scripts) |
| `films/<name>/frames.csv` | Each frame's number and script time, both submitted eyes' poses and fields of view, the hands' and the head's poses, the image's size and a crop's place |
| `control.toml` | The live control file (below) |

While a program runs, from another shell:

```bat
rustvr-xrsim capture menu_open             :: save a capture now; prints its path when written
rustvr-xrsim ctl "right = { trigger = 1 }" :: override the script live (a key without t)
rustvr-xrsim ctl "focused = false"         :: the headset's dashboard over the app (unfocused)
rustvr-xrsim ctl "quit = true"             :: end the session
rustvr-xrsim status
```

`ctl` writes its arguments, one TOML line each, to `control.toml`, which the headset rereads
whenever it changes. It may hold:

- `head`, `left` and `right` as in a key, without `t`: they override the script from then on, and
  build up over every control file;
- `capture = "<name>"` (a new name each time), `focused = true|false`, `quit = true`;
- `[[key]]`s: a path played from when the file is read, each `t` seconds from then, eased into from
  wherever the script is and held at its end (a test driver's reach for something it has found).

`capture` writes `capture = "<name>"` and waits up to 15 s for the picture.

## Scripts

A script is TOML. Keys set the head and controllers at a time. Positions, turns, triggers,
squeezes and sticks ease from one key to the next; buttons and `connected` flip exactly at their
key. Each key names only what changes.

```toml
duration = 60            # end the session after this many seconds (0, the default: never)
profile = "/interaction_profiles/oculus/touch_controller"
eye_size = [2064, 2208]  # the per-eye size the headset recommends
fov = [-54, 43, 46, -52] # left eye: left, right, up, down (degrees); the right eye mirrors it
ipd = 0.064
refresh = 90
latency_ms = 0           # how far ahead each frame's display is predicted (at least a frame)
capture_width = 960      # each eye's width in the saved captures
aim_pitch = 0            # how far the aim pose points above the grip (degrees)

[[key]]
t = 0
head = { pos = [0, 1.7, 0], yaw = 0, pitch = 0, roll = 0 }   # stage metres; degrees
right = { pos = [0.2, -0.55, 0.25] }  # [right, up, forward] metres from the head, turned with its yaw

[[key]]
t = 4
right = { pos = [0.04, -0.07, 0.38], trigger = 0 }  # the gun raised to the eye

[[key]]
t = 6
right = { trigger = 1 }  # pulled over 2 s

[[key]]
t = 10
head = { yaw = 50, pitch = 15 }  # turn right and look up over 4 s

[[capture]]
t = 5
name = "aim"

[[film]]                 # consecutive frames, without holding the app up
t = 11
frames = 90
name = "turn"
```

The values shown are the defaults, but for `duration` and the keys. Before the first key the head
is at `[0, 1.7, 0]` looking ahead and the hands at `[±0.2, -0.55, 0.25]`. The other fields are:

- each hand's `yaw`, `pitch`, `roll`, `squeeze`, `stick = [x, y]`, `a`, `b` (X and Y on the left
  controller), `menu` (the left controller's) and `stick_click`;
- `connected = false`, which makes a controller lose tracking;
- a film's `crop = [x, y, width, height]` (that part of each eye at full size, in the submitted
  image's pixels, instead of the whole), `every` (only every so many frames: 3 at 90 Hz gives 30 a
  second) and `mip` (the size kept: 0 the whole, 1 half, 2 a quarter, the default).

Keys, captures and films may be written in any order. Films are taken one at a time: one that's
due waits for the last to finish.

The script's times are its own clock: it starts when the app first calls the headset (making its
OpenXR instance) and stops while a capture is saved (saving holds the app's frame up for about a
second, and the app sees nothing of that time), so a key that falls during a capture still reaches
the app. A film's frames are copied
on the GPU and written as they come, so it doesn't hold the app up. `xrsim.log` gives each
capture's and film's saving time and the script time after it; its own times are real seconds.

A button press is two keys a moment apart:

```toml
[[key]]
t = 15
right = { a = false }

[[key]]
t = 15.01
right = { a = true }

[[key]]
t = 15.25
right = { a = false }
```

## Reading a capture

`<name>.png` is what the headset would show. Each eye's own view (its pose and field of view at that
frame) is filled from the layer the app submitted, reprojected as a compositor does it. Anything the
app got wrong shows up as it would in the headset:

- a stale or wrong pose shows a shifted picture;
- the wrong field of view shows it stretched or cut off;
- the wrong eye's image shows up in the other eye;
- dark grey means the layer has nothing there (outside its field of view).

`<name>_raw.png` is the submitted images before reprojection, which helps tell an app's rendering
bug from a submission bug. A black capture means no projection layer had been submitted yet, and
the `.toml` says so. A film's frames are the submitted images, like `_raw.png`; `frames.csv` gives
each one's poses, to judge motion frame by frame.

**What it doesn't emulate:**

- **Lens distortion.** The captures are rectilinear.
- **Translation in reprojection.** It uses rotation only.
- **Depth in reprojection.** The depth an app gives is saved, not used.
- **Layers other than projection layers.** Quad and cylinder layers aren't drawn, and the runtime
  composites its own panels into the projection layer anyway.
- **Haptics.** Pulses are counted in `status.toml`, not felt.
- **Other controllers.** Only the Touch controllers' bindings are checked.

Performance is measured only roughly. The frame rate in `status.toml` is real, but there is no
GPU-timed compositor.

## Games

A game adapter's repository wraps a whole run in one script: it builds the adapter, the runtime and
the emulator, installs the adapter in the game, runs `rustvr-play --no-range` on the emulated
headset and starts the game, waits for the script to end, closes the game and copies the runtime's
and the adapter's logs.

- **Oblivion** (`chasms-vr-oblivion`): `tools/oblivion.ps1 -Sim tools\xrsim\<script>.toml`, or
  `-Hold <seconds>` for the headset held still. It takes the game lock, boots the game into a test
  save (`-Save`, `-At`, `-Give`, `-Commands`, `-ConsoleFile` set the scene), sets the adapter's
  `background = true` (the game never takes focus) and any `-Settings`, and leaves the results in
  `-Out` (`captures\run` by default): the emulator's folder, `runtime.log` and this run's part of
  `adapter.log`. Its scripts are in `tools/xrsim/`, their times seconds from the runtime's start;
  its `docs/DEVELOPING.md` has the rest.

## For AI agents

1. Write a script that does what the player did, with a capture (or a film) at each moment that
   matters.
2. Run it. For a game use its adapter's script (Oblivion: `tools/oblivion.ps1 -Sim`); for other
   apps use `rustvr-xrsim run`. Use a background shell, because the run blocks until the script
   ends.
3. Read the captures. Look at `<name>.png` before `_raw.png`, and check the `.toml` when the picture
   looks shifted.
4. Read `xrsim.log` for what the app asked of the runtime, and the app's own logs.
5. Fix the problem, and run the same script again to confirm the fix in the captures.
