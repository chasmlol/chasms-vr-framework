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
- captures show the picture both eyes would have seen;
- the logs say what the app asked of the runtime and what it submitted.

Mod makers can let an AI agent playtest this way: the agent runs a script, reads the pictures and
logs, fixes, and runs it again.

## How it works

The OpenXR loader picks its runtime from `XR_RUNTIME_JSON` when that is set, before anything
installed (SteamVR, Virtual Desktop, Oculus). `rustvr-xrsim run` sets it for one process only, to a
manifest naming `rustvr_xrsim.dll`. Nothing changes system-wide, and a real headset left plugged in
is untouched.

The DLL implements the core OpenXR 1.0 API and `XR_KHR_D3D11_enable`:

- **Session.** It goes IDLE → READY when created, then SYNCHRONIZED → VISIBLE → FOCUSED on
  `xrBeginSession`. It goes STOPPING → IDLE → EXITING when the script ends.
- **Actions.** Bindings are checked against the controllers' real input paths. An unsupported path
  fails as it would on a real runtime, and `xrsim.log` lists every binding the app suggested. Action
  states read from the script and are inactive while the session isn't focused.
- **Spaces.**
  - STAGE is the floor at the play area's centre.
  - LOCAL is the same as STAGE. A real headset puts LOCAL at the head's starting pose.
  - VIEW is the head.
  - Action spaces are the controllers' grip and aim poses.
- **Swapchains.** Textures are made on the app's own D3D11 device, three per swapchain.
- **Frames.** `xrWaitFrame` paces to the headset's refresh rate (90 Hz by default).
  `xrEndFrame` records the projection layer for captures.

## Building and running

```bat
cargo build --release -p rustvr-xrsim
target\release\rustvr-xrsim run --dir captures\xr --script my_test.toml -- target\release\rustvr-play.exe --no-range
```

Anything after `--` is the program to run (any OpenXR app using D3D11). It starts on the emulated
headset, and `run` returns when the program exits. The program exits when the script's `duration`
is up, because the headset ends the session as if the player closed it.

`<dir>` (`--dir`, default `%TEMP%\rustvr-xrsim`) gets:

| File | What |
|---|---|
| `xrsim.log` | The session: every state change, binding, swapchain and capture, with times in seconds |
| `status.toml` | Rewritten every second. It has the session state, focus, frames, fps, layers submitted, haptic pulses, whether actions are attached, and the views' sizes |
| `captures/<name>.png` | Both eyes as the headset would show them (see below) |
| `captures/<name>_raw.png` | The images the app submitted, as they are |
| `captures/<name>.toml` | The numbers: the time, the head's pose, and for each eye the layer's field of view and pose against the headset's |

While a program runs, from another shell:

```bat
rustvr-xrsim capture menu_open            :: save a capture now; prints its path when written
rustvr-xrsim ctl "right = { trigger = 1 }" :: override the script live (one key, no t)
rustvr-xrsim ctl "focused = false"         :: the headset's dashboard over the app (unfocused)
rustvr-xrsim ctl "quit = true"             :: end the session
rustvr-xrsim status
```

## Scripts

A script is TOML. Keys set the head and controllers at a time. Values ease from one key to the next
(poses, triggers, sticks), while buttons flip exactly at their key. Each key names only what changes.

```toml
duration = 60            # end the session after this many seconds (0: never)
profile = "/interaction_profiles/oculus/touch_controller"
eye_size = [2064, 2208]  # the per-eye size the headset recommends
fov = [-54, 43, 46, -52] # left eye: left, right, up, down (degrees); the right eye mirrors it
ipd = 0.064
refresh = 90
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
```

The other fields are:

- each hand's `yaw`, `pitch`, `roll`, `squeeze`, `stick = [x, y]`, `a`, `b` (X and Y on the left
  controller), `menu` and `stick_click`;
- `connected = false`, which makes a controller lose tracking.

Keys and captures may be written in any order.

The script's times are its own clock: it starts with the emulator and stops while a capture is
saved (saving holds the app's frame up for about a second, and the app sees nothing of that time),
so a key that falls during a capture still reaches the app. `xrsim.log` gives each capture's
duration and the script time after it; its own times are real seconds.

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
bug from a submission bug. A black capture means nothing was submitted that frame, and the `.toml`
says so.

**What it doesn't emulate:**

- **Lens distortion.** The captures are rectilinear.
- **Translation in reprojection.** It uses rotation only.
- **Depth layers.**
- **Layers other than projection layers.** Quad and cylinder layers aren't drawn, and the runtime
  composites its own panels into the projection layer anyway.
- **Haptics.** Pulses are counted in `status.toml`, not felt.

Performance is measured only roughly. The frame rate in `status.toml` is real, but there is no
GPU-timed compositor.

## Games

A game adapter's repository wraps a whole run in one script: it builds the adapter, the runtime and
the emulator, installs the adapter in the game, runs `rustvr-play` on the emulated headset and
starts the game, waits for the script to end and copies the runtime's and the adapter's logs.
Metro 2033 Redux's is `tools\xrsim_metro.ps1` in its adapter's repository (chasms-vr-metro,
`docs/XRSIM_METRO.md`).

## For AI agents

1. Write a script that does what the player did, with a capture at each moment that matters.
2. Run it. For a game use its adapter's script (Metro: `tools\xrsim_metro.ps1`); for other apps
   use `rustvr-xrsim run`. Use a
   background shell, because the run blocks until the script ends.
3. Read the captures. Look at `<name>.png` before `_raw.png`, and check the `.toml` when the picture
   looks shifted.
4. Read `xrsim.log` for what the app asked of the runtime, and the app's own logs.
5. Fix the problem, and run the same script again to confirm the fix in the captures.
