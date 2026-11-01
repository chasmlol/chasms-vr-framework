# VR throwables, grenades and throwing knives: research notes

> A research snapshot (2026-10-21), not a description of the framework's code: what was built is in
> `docs/`. The Metro adapter it mentions lives in its own repository now.

Researched 2026-10-21 for the RustVR runtime (game-agnostic, our own physics rig, OpenXR).
Every fact has a source URL. **[src]** means I read the source code. **[snippet]** means the figure comes only from a
search-engine summary because the page itself was blocked (Cloudflare or 402), so treat it as weaker.
**[inference]** marks my own reasoning. Section 10 is the implementation parameter list.

Prior art in a sibling project: `vittorioromeo/quakevr`, `docs/vr-port/THROWING.md`, already covers much of this ground,
with an implementation, cvars and a test plan
(https://github.com/vittorioromeo/quakevr/blob/master/docs/vr-port/THROWING.md). I re-checked its primary sources rather than copying it.

---

## 1. Release-velocity estimation: what shipping code actually does

### 1.1 Valve: SteamVR Interaction System, Half-Life: Alyx, and the Index patent
- **`Throwable.cs` defaults** [src] (https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/Throwable.cs):
  - `releaseVelocityStyle = GetFromHand`: the runtime's own controller velocity, read at `releaseVelocityTimeOffset = -0.011` s, one 90 Hz frame in the past.
  - `scaleReleaseVelocity = 1.1`, applied to linear velocity only.
  - An optional speed-dependent gain, disabled by default (`scaleReleaseVelocityThreshold = -1`): `factor = clamp01(curve(|v| / threshold))`, curve `EaseInOut(0, 0.1, 1, 1)`. The tooltip says it "allows greater differentiation between a drop, toss, and throw". A slow release gets 0.1× and a full-speed one gets 1.0×.
  - `rigidbody.maxAngularVelocity = 50` rad/s.
  - `catchingSpeedThreshold = -1`. When enabled, an object moving faster than this attaches on trigger *hold* rather than *press*, which is how you catch.
  - Other styles: `NoChange`, `ShortEstimation` and `AdvancedEstimation`.
- **`VelocityEstimator.cs` (ShortEstimation)** [src] (https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/VelocityEstimator.cs):
  differences the transform every frame (`Δp / Δt`) and takes a plain box mean, `velocityAverageFrames = 5` and `angularVelocityAverageFrames = 11`.
- **AdvancedEstimation** [src]:
  - `SteamVR_Behaviour_Pose` keeps `SteamVR_HistoryBuffer(30)`, the last 30 updates of pose plus *runtime* velocity (https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/Input/SteamVR_Behaviour_Pose.cs).
  - `GetEstimatedPeakVelocities` calls `GetTopVelocity(10, 1)`, which finds the max-|v| sample in the last 10 frames.
  - It then calls `GetAverageVelocities(…, 2, top)`, which averages the **peak frame and the frame before it** (https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/Scripts/SteamVR_RingBuffer.cs).
- **Half-Life: Alyx developer commentary, "Throwing" (Tony Cox)** (https://combineoverwiki.net/wiki/Developer_commentary/Half-Life:_Alyx):
  - Controllers are sampled about every 11 ms, and a hand can move 15 cm or more between samples.
  - *The Lab* differenced positions and averaged over several frames around the release. It was inconsistent.
  - *Moondust* biased the estimate toward higher velocities and used the controller's **hardware (IMU) velocity, computed every 2 ms**. That made throwing frame-rate independent.
  - **Alyx takes the 10 frames before the release and averages the 3 frames bracketing the peak controller velocity.** At 90 Hz that is a ~111 ms look-back and a ~33 ms averaging span.
  - It was made reliable enough to *require* throws in puzzles, such as an armed grenade through a high duct.
- **Valve patent US11185763B2, "Holding and releasing virtual objects"** (Index controllers) (https://patents.google.com/patent/US11185763B2/en):
  - Once a release is detected, it looks back a window ("e.g. 5, 15, or 50" frames; the worked example uses **15**) and finds the frame with **peak controller speed**, which is taken as the *intended* release.
  - The velocity is the average of frames N-1, N and N+1, in both speed and direction.
  - The object may be released **from its position at the peak frame**. Alternatively the system computes the trajectory from there and animates the object from its current position onto that path, accelerating or decelerating toward it rather than snapping.
  - Fallback: if the release is never confirmed, release a set time after it started (example "five frames").
  - The grab thresholds are given as a percentage of sensor maximum:
    - force-grip > 10% or 25%
    - force-pinch > 1% or 25% plus trigger
    - cap-grip (mean of middle, ring and pinky proximity) > 50% or 70%
    - cap-pinch > 30% or 60%
  - A release starts when the grab criteria fail, when force drops by more than a threshold between readings, or when a velocity threshold is exceeded (example 1 m/s). The exact role of the velocity test is not clear from the text I could read.

### 1.2 Unity XR Interaction Toolkit (`XRGrabInteractable`) [src]
(https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Interaction/Interactables/XRGrabInteractable.cs,
docs: https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/xr-grab-interactable.html)
- A 20-frame ring (`k_ThrowSmoothingFrameCount = 20`) holds the **differenced target pose of the object**, so the tangential (ω×r) velocity at the object is implicit.
- Release uses a weighted mean over `throwSmoothingDuration = 0.25` s, weighted by `throwSmoothingCurve`, which is flat by default.
- `throwVelocityScale = 1.5` and `throwAngularVelocityScale = 1.0`.
- Frames with dt < 1 ms are skipped (`k_DeltaTimeThreshold`, "frame-timing issue on Quest").
- On teleport, the buffered velocities are rotated by the teleport delta (`OnTeleported`), so locomotion jumps don't pollute the throw.
- A source comment admits that two-handed release is imperfect: frames after the first hand lets go should be ignored.
- Note: a 0.25 s box mean blends in the wind-up. Deck (§1.6) found this kind of low-pass filter made throws feel "underwater".

### 1.3 Meta Interaction SDK [src]
(package source mirrored at https://github.com/USEPA/VRDecon/tree/main/Library/PackageCache/com.meta.xr.sdk.interaction@72.0.0/Runtime/Scripts/Throw)
- **`RANSACVelocityCalculator`** (current) uses `RANSACOffsettedVelocity(samplesCount = 8, samplesDeadZone = 2)`.
  - It buffers 8 timed **root (wrist or controller) poses**.
  - Each candidate velocity is the difference of a *pair* of samples, evaluated at the **grab-point offset** (`PoseUtils.Multiply(pose, offset)`), so rotation-induced tangential velocity at the object is included.
  - It picks the pair model that minimises the squared distance to all other pair models, with N random trials.
  - Low-confidence samples (hand tracking) are dropped. After a gap, the last pose is re-timestamped using `MaxSyntheticSpeed = 5` m/s.
- **`StandardVelocityCalculator`** (obsolete):
  - Buffer `BufferLengthSeconds = 0.4` at 90 Hz.
  - `StepBackTime = 0.08` s: it reads the buffered velocity 80 ms *before* the release ("hand tracking might experience greater latency").
  - It sums instant + trend + tangential + external terms with influences 1, 1, 1 and 0.
  - The trend is the mean of the largest window of samples moving the same way (dot threshold 0.6).
  - If more than 50% of samples are zero, it falls back to the most recent moving sample.
- **`ThrowTuner` / `ThrowPhysicsProfile`** (v83+) (https://developers.meta.com/horizon/documentation/unity/unity-isdk-throwing-objects/):
  - Velocity controls (in local space): `VelocityScale`, `VelocityAdd`, `MaxSpeed`.
  - Spin controls: `SpinScale`, `SpinAdd`, `MaxSpin`.
  - Alignment: `AlignForwardOnce` snaps the object's forward to its velocity at release. `KeepForwardToVelocity` keeps lerping forward to the velocity in flight, with `ForwardLerpSpeed`.
  - Flight: `GravityScale` (0–4), `LocalConstantForce` and `LocalVelocityDamping`.
  - The body is switched to dynamic one physics tick after release, "for more reliable throws".
  - See §6.2 for the sample's dart and axe numbers.

### 1.4 Hurricane VR (physics hands, PD/joint driven) [src]
(`HVRHandGrabber.cs`, `HVRGrabbable.cs`, mirrored at https://github.com/USEPA/VR/tree/main/EPAVR-2/Assets/HurricaneVR; API docs:
https://cloudwalker2020.github.io/HurricaneVR-Docs/api/HurricaneVR.Framework.Core.Grabbers.HVRHandGrabber.html; release notes:
https://github.com/USEPA/VR/blob/main/EPAVR-2/Assets/HurricaneVR/Documentation/ReleaseNotes.txt)
- **Formula:** `throw = handFactor·avg(handVel) + grabbable.ReleasedVelocityFactor·avg(objVel)`.
- **Angular-to-linear term:** if `|ω_hand| > ReleasedAngularThreshold` (default **1 rad/s**), it adds `ω_hand × (grabPoint − throwingCoM) · conversionFactors`.
  - `HVRThrowingCenterOfMass` is a per-controller-model point. The release notes ask that it "match the controller type".
  - The grabbable's angular-to-linear factor default was changed from 1 to 0 in later releases, so the tangential term is off by default.
- **Averaging:** over `ThrowLookback` frames after skipping `ThrowLookbackStart`. The code default is 5 and the shipped hand prefabs use **10**.
  - `TakePeakVelocities = true` with `CountPeakVelocities = 3` on the shipped prefabs: it sorts and averages the 3 fastest samples.
  - Before averaging it **discards samples whose direction differs from the window mean** (dot < 0.2), which kills wind-up and back-swing samples.
- **Gains:** default throw strength was "lowered to 1.0 from 1.1". The frame storage cap is 10, and the notes say 5 "seems to work fine".
- **Post-release collision:** hand collision stays off for `timeout ≥ 0.2 s` when release speed > 2 m/s, which prevents the hand clipping the thrown object.
- **Holstering safety:** `HVRShoulderSocket.CanHover` refuses to socket an item whose current throw velocity points along the socket's forward. A wind-up over the shoulder therefore doesn't holster the grenade.

### 1.5 Auto Hand, VRIF and Godot XR Tools [src]
- **Auto Hand** (`HandVelocityTracker.cs`) (https://github.com/Colo629/Biplane/blob/HEAD/Assets/AutoHand/Scripts/Internal/HandVelocityTracker.cs):
  - Box mean of the *held rigidbody's* velocity over the last `throwVelocityExpireTime = 0.125` s, and angular over 0.25 s.
  - Multiplied by `throwPower`: 1.0 on the hand (1.1 in older versions) times the grabbable's `throwPower` of 1.
  - **Angular is scaled by `sqrt(throwPower)/2`**, so spin is roughly halved.
- **VR Interaction Framework (BNG)** (`Grabbable.cs`, `VelocityTracker.cs`) (https://github.com/Zhes-20/EnJoyTheVR/tree/HEAD/Assets/third-party-Assets/BNG%20Framework/Scripts):
  - Averages over `AverageVelocityCount = 3` frames.
  - `ThrowForceMultiplier = 2.0` (field `ThrowForceMultiplierAngular = 1.5`).
  - **It adds the player rig's rigidbody velocity** (locomotion) to the throw.
- **Godot XR Tools** (`function_pickup.gd`) (https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd):
  `velocity_samples = 5` and `impulse_factor = 1.0`.

### 1.6 Developer write-ups
- **Charlie Deck (Rescuties), "Why throwing in VR sucks"** (https://www.gamedeveloper.com/design/why-throwing-in-vr-sucks--and-how-to-make-it-better):
  - **Measure velocity at the controller** (the mass you feel), not at a virtual object offset up to a foot away. Three versus twelve inches of offset turns "your hand into a catapult" through wrist rotation.
  - Moving average felt "underwater". Pure peak velocity was fast but went in noisy directions.
  - A **linear-regression trendline over the last few frames (the debug view shows 4)** worked best.
  - **Release when pressure eases ~20% below the peak since pickup**, not at 0%. The grab threshold must leave headroom; set too low, it caused grab-drop-grab chatter.
  - Vive trigger: about 80% is easy to reach, 100% needs a hard squeeze, and the signal has sensor and heartbeat jitter.
- **Wilco Schoneveld (web VR)** (https://dev.to/wilcoschoneveld/challenges-of-a-web-vr-throwing-game-1c66):
  a least-squares **quadratic** fit over the last 6 object positions, with t centred on release. Velocity is the linear coefficient. An explicit ω×r term was tried, then dropped.
- **NormalVR, "Throwing Throw Down"** (https://www.normalvr.com/blog/throwing-throw-down/):
  - Use the runtime's controller v and ω and evaluate the point velocity anywhere on the controller (`GetPointVelocity`, i.e. v + ω×r). Self-differenced estimates were less consistent.
  - Read poses on the runtime's new-poses event; `Update()` is "usually a frame behind".

### 1.7 OpenXR facts that matter for our runtime
- **History:** "The runtime must retain and return at least **50 milliseconds** of historical data … preceding the most recently received pose." Past times can be located exactly within that window (https://raw.githubusercontent.com/KhronosGroup/OpenXR-Docs/main/specification/sources/chapters/fundamentals.adoc). A 100–170 ms look-back needs our own ring buffer.
- **`lastChangeTime`** on `XrActionState{Boolean,Float}` is "the runtime's best estimate of when the physical state … last changed". It is an XrTime, so the release edge can be timestamped in the same time base as poses (https://raw.githubusercontent.com/KhronosGroup/OpenXR-Docs/main/specification/sources/chapters/input.adoc).
- **Bool from analog:** when a bool action binds to a scalar, "a threshold must be applied … should use hysteresis … may vary from device to device" (same file). The runtime's bool squeeze threshold is out of our control and usually late, so use the float.
- **Interaction profiles:** Index has `/input/squeeze/value` **and** `/input/squeeze/force`. Touch has `/input/squeeze/value` (https://raw.githubusercontent.com/KhronosGroup/OpenXR-Docs/main/specification/sources/chapters/semantic_paths.adoc).
- **Prediction:** poses located at `predictedDisplayTime` are 1–3 frames *in the future* (extrapolated). Timestamp each sample with the XrTime it was located for, not the host clock, or the window is skewed (quakevr THROWING.md P2, link above). [inference] A cleaner option is to also locate the grip at "now", which needs `XR_KHR_win32_convert_performance_counter_time`.
- `XrSpaceVelocity` supplies runtime (IMU-fused) linear and angular velocity. Use it when the `*_VALID_BIT` flags are set, otherwise difference the poses. This is the Valve or Moondust preference.

### 1.8 Physics-driven hands: a caveat for our rig
- Held items that are joint and spring driven lag the controller: thrown daggers land about two feet in front, and players stiffen the spring to compensate.
- Boneworks simulates mass, so heavy items are hard to throw (https://roadtovr.com/boneworks-reveal-stress-level-zero-physics-based-interactions-embodiment/).
- HVR blends the physics-hand velocity with the grabbable velocity (§1.4).
- **[inference]** With our PD-driven rig, estimate from the **tracked controller** (Deck, Valve) and apply mass only as a deliberate gameplay scale (§3.4). Do not read the lagging simulated body's velocity, or every heavy item and every frame hitch will weaken throws.

### 1.9 Summary table

| System | Source signal | Window | Pick | Gain (lin / ang) |
|---|---|---|---|---|
| SteamVR GetFromHand | runtime vel @ −11 ms | 1 sample | — | 1.1 / 1.0 |
| SteamVR Advanced | runtime vel | last 10 frames | peak + previous frame | 1.1 / 1.0 |
| HL:A (commentary) | IMU vel | 10 frames before release | mean of 3 around peak | n/a |
| Valve patent | controller vel | e.g. 15 frames back | mean N−1..N+1 at peak; release from peak position | n/a |
| XRI | differenced object pose | 0.25 s, max 20 frames | weighted mean | 1.5 / 1.0 |
| Meta RANSAC | root pose at grab offset | 8 samples | RANSAC pair | profile |
| Meta Standard (old) | runtime vel | 0.4 s | instant@−80 ms + trend + tangential | 1 / 1 |
| HVR (prefabs) | hand + object vel | 10 frames | top 3 after direction filter (dot ≥ 0.2) | 1.0; ω×r above 1 rad/s |
| Auto Hand | held body vel | 0.125 s (ang 0.25 s) | mean | 1.0 / ≈0.5 |
| VRIF | grabber vel + player vel | 3 frames | mean | 2.0 / (1.5) |
| Godot XR Tools | object deltas | 5 samples | mean | 1.0 |
| Deck | controller point | ~4 frames | linear regression | — |
| Schoneveld | object pos | 6 samples | quadratic regression | 1.0 |

---

## 2. Release detection and controller differences
- **Late or unreliable release is the main complaint on Index.**
  - Duck Season players saw objects hitting "an invisible wall 1 foot in front of me" until they set grip release "on zero or very low" in bindings (https://steamcommunity.com/app/503580/discussions/0/1628538005521247419).
  - "If you have it set too high when releasing you will find that you often don't release objects" (https://itch.io/post/1427738).
  - In Pavlov on Index, players drop pin-pulled grenades unless they keep the same grip force, or press the trigger slowly (https://steamcommunity.com/app/555160/discussions/0/1745643424481471676/).
- **Fixes in the wild:**
  - Analog drop-from-peak release, ~20% below peak (Deck, §1.6).
  - Valve's force, capacitance and velocity release logic (§1.1 patent).
  - Look back to the speed peak, so a late release edge doesn't matter: Alyx, the Valve patent, and Meta `StepBackTime` 80 ms.
- **SteamVR binding defaults** are anecdotal: the trigger-click default is reportedly 25% activate and 20% deactivate (https://feedback.vrchat.com/bug-reports/p/trigger-thresholds-in-steamvr-bindings-are-too-high-since-input-system-update). Users tune Index "Force Hold / Force Release" on Grip Grab bindings (https://docs.vrchat.com/docs/valve-index).
- **Hardware and ergonomics:**
  - The HL:A forum blames WMR and Quest 1 tracking for bad throws (https://steamcommunity.com/app/546560/discussions/0/3068614788771586125).
  - SuperHot switched from hold-to-grab and release-to-throw to button press because of hand and tendon strain after about 30 minutes (https://itch.io/post/1427738). [inference] Offer a toggle-grip option, where the throw is a button press, for accessibility.
- **Research on point of release:**
  - GI'24 (Ghasemaghaei et al.) compared manual and automatic release methods. Velocity was a backward finite difference over **5 frames for controllers** and 10 for trackers or Kinect.
  - Most participants *preferred* manual release on a handheld controller ("I was able to have a better knowledge of when I should release"). The body tracker with automatic release was most accurate for overarm throws, and controller hold was best for underarm (https://www.cs.ucf.edu/~jjl/pubs/gi24-16final.pdf).
- **Perception of release timing** (FauxThrow, Yamac & O'Sullivan) (https://arxiv.org/abs/2208.02166):
  - A ±10 ms shift of the release point was not reliably noticed. Shifts of ±20 to 50 ms were.
  - For overarm throws, early release was flagged more often than late. Underarm was the reverse.
  - Detection tracked angular deviation of the trajectory and landing-distance change, mostly shortened throws.
  - So aim to land within ~10 ms of the true release, which backdating from the peak gives.

---

## 3. Throw assist

### 3.1 Linear gain: everyone boosts, by 1.0 to 2.5×
| Source | Gain |
|---|---|
| Godot | 1.0 |
| HVR | 1.0 (was 1.1) |
| SteamVR | 1.1 |
| XRI | 1.5 |
| VRIF | 2.0 |
| Meta sample profiles | basketball 2.5, bowling 1.5, axe x 1.5, dart forward-z 2.0 + 1 m/s (§6.2) |

- **Why:**
  - VR throws are less accurate than real ones, "particularly in the distance and height dimensions". VR throwing also takes more effort (Zindulka, Bachynskyi, Müller, CHI 2020; https://dblp.uni-trier.de/pid/128/9625.html, abstract via https://arxiv.org/abs/2208.02166).
  - Jeong, Stuerzlinger & Jeong found a systematic **undershoot that grows with distance**, interpreted as players avoiding over-throwing out of view. They used the Meta SDK v83 with gain 1 (https://vvise.iat.sfu.ca/user/data/papers/throwmodel.pdf).
  - But Butkus & Čeponis (n small, Vive tracker, automatic release on deceleration) found VR users used **3–5% *more*** power and were accurate at longer range (https://ceur-ws.org/Vol-2470/p33.pdf). Large multipliers are therefore a *gameplay* choice, not a perceptual correction.
- **Player perception:**
  - HL:A players feel distance depends on "only the angle", and armed grenades are "artificially cut short" (https://steamcommunity.com/app/546560/discussions/0/3068614788771586125).
  - Arizona Sunshine players say grenades go "a few feet" and ask for "more acceleration" (https://steamcommunity.com/app/342180/discussions/0/152391285459172087/).
  - Short throws are the dominant complaint.
- **Speed-dependent gain** (Valve curve, §1.1) keeps hand-offs and drops at 1× and boosts only real throws. quakevr's adopted version: 1.0 below 1.5 m/s, smoothstep to 1.5× by 6 m/s.
- **Clamp max speed** per item. The Meta sample uses `MaxSpeed` 18 (basketball), 20 (axe), 25 (bowling), 50 (dart, rugby) and 5 (paper plane) m/s.

### 3.2 Spin (angular) handling
- Spin scale 1.0 in XRI. About 0.5 in Auto Hand. SteamVR clamps at 50 rad/s.
- Meta `MaxSpin` values are 45 (bowling), 80 (basketball), 200 (axe, rugby) and **5 (dart)**. The Jeong study used `MaxSpin` 150.
- quakevr caps at ~20 rad/s so that wrist-flick noise doesn't give "helicopter" weapons.

### 3.3 Aim assist
- **Rec Room patent US10990169B2** (https://patents.google.com/patent/US10990169B2/en):
  - The target comes from gaze: head forward or eye, or simply "anywhere within a viewing frustum".
  - It is weighted by the angle between gaze and the hand-motion vector. Gaze gets full weight near an acute angle ("such as 20 degrees or 30 degrees") and is ignored beyond ~90° (a no-look throw).
  - The object follows its natural release path for a first stage "on the order of 10 milliseconds or less". It is then nudged in direction and/or speed onto a path that lands at or near the target, and normal physics resumes.
- **HL:A gravity gloves** (catching, not throwing): the launch is physics-driven, with "small corrective impulses" steering the object to the moving hand, plus a "generous catch range" (HL:A dev commentary, §1.1 link).
- quakevr's adopted defaults (§1 link): cone 15°, full strength inside 4°, strength 0.35, max speed change 15%, line-of-sight check, lead on moving targets, low-arc ballistic solution.

### 3.4 Weight
- Boneworks: true mass (§1.8).
- HVR and Auto Hand: per-grabbable `ReleasedVelocityFactor` / `throwPower` (0–1 to reduce).
- quakevr found per-item multipliers of 0.4–0.6 "roughly halve the range (range ∝ v²)". It changed to `mix(1, mult, 0.25)`.
- [inference] Keep weight effects mild (0.85–1.0×) for one-hand throwables such as grenades and knives.

### 3.5 Throw versus drop
- Valve's curve: 0.1× at zero speed, ramping to 1× at the threshold.
- HVR's direction filter, dot ≥ 0.2.
- Auto Hand `minThrowVelocity` (0 by default).
- HVR stabbing `RequiredVelocity = 1 m/s`.
- [inference] Treat |v| < ~1 m/s at release as a *drop*: no gain, no assist, no knife alignment, and spin damped.

---

## 4. Grenade mechanics by game

| Game | Arm / pin | Fuse starts | Fuse | Feedback | Storage / carry |
|---|---|---|---|---|---|
| **HL:A Combine frag** | "Press center priming button" while held; spoon and handle release and the body expands | at priming (in hand) | ~4–5 s; enemy grenades much longer, so catch-and-return is possible | 5 blue lights each side light one by one; loud rhythmic beep; red light | 2 in wrist pockets + 2 in hands + 1 in shotgun launcher (max 5) |
| **HL:A Xen grenade** | "Squeeze to prime"; [snippet] on Index a hard squeeze (grip force) | at priming | not stated | inflates to a sphere, glows green, clicks | wrist pockets; regrows on plant pedestals |
| **HL2 (reference)** | throw / toss / roll inputs | on throw only | "a few seconds" | red blinking light + chirp; the ticking was *added* because unlit grenades were "harder to locate and escape" | — |
| **Pavlov VR** | two-handed: hold grenade, other hand pulls pin; one-hand arming was refused by design ("players are meant to be vulnerable") | — | — | — | 3 on vest + 1 per hand |
| **H3VR** | pull pin; optional spoon release via touchpad (Classic) or B (Streamlined) starts fuse immediately (cooking) | spoon release | per real model: RG42 3.2–4.2 s, M26 4–5 s; URG86 has the largest variance; RGO/RGN are impact (RGO arms ~1 s after throw; a dropped RGN doesn't instantly detonate) | — | quickbelt slots (layouts; mods add hand quickbelts) |
| **Into the Radius** | grab grenade, pull pin with other hand, press B/Y on holding hand to release lever, throw | lever release | "a few seconds" | fragmentation is modelled, so self-kills are possible | vest / backpack |
| **Arizona Sunshine** (early) | pin pulled by pressing touchpad on the holding hand | pin pull | ~3 s | — | — |
| **Metro Awakening** | "stick of explosive with a fuse lit by a small battery once the pin is pulled"; cooking recommended because humans flee lit grenades | pin pull | ~4–5 s | — | 3, on the backpack: right hand over left shoulder pulls a grenade; left hand over left shoulder pulls the backpack. Grenades bounce, and mutants can kick them back |
| **Bonelab/Boneworks** | not found (true-mass physics) | — | — | — | — |

Sources for the table:
- HL:A frag: https://combineoverwiki.net/wiki/Grenade_(Half-Life:_Alyx)
- HL:A Xen: https://combineoverwiki.net/wiki/Xen_Grenade
- HL2: https://www.combineoverwiki.net/wiki/Grenade_(HL2)
- Pavlov: https://steamcommunity.com/app/555160/discussions/0/1745643424481471676/ and https://steamcommunity.com/app/555160/discussions/0/1745605598716836055/
- H3VR [snippet]: https://h3vr.fandom.com/wiki/RG42, https://h3vr.fandom.com/wiki/M26, https://h3vr.fandom.com/wiki/RGO, https://h3vr.fandom.com/wiki/RGN, https://h3vr.fandom.com/wiki/URG86
- H3VR quickbelt mods: https://thunderstore.io/c/h3vr/p/quaternion/Hand_Quickbelts/
- ITR [snippet]: https://into-the-radius.fandom.com/wiki/Notes and https://www.imfdb.org/wiki/Into_the_Radius
- Arizona Sunshine [snippet]: https://arizona-sunshine.fandom.com/ja/wiki/M67_Fragmentation_Grenade
- Metro Awakening: https://www.realityremake.com/articles/all-metro-awakening-weapons-all-metro-vr-weapons and https://steamcommunity.com/app/2669410/discussions/0/4634860720019828942/

**Real-world fuse references**
- Soviet UZRG/UZRGM fuze (RGD-5, F1): 3.2–4.2 s (https://en.wikipedia.org/wiki/RGD-5).
- M67: 4–5 s after spoon release (https://en.wikipedia.org/wiki/M67_grenade).
- In real grenades the pin only unsafes; the **spoon release** starts the fuse.

**HL:A health injector:** a button arms it first, to prevent accidental use and to add haptic and audio "weight". This is the same pattern as grenade priming (dev commentary, §1.1 link).

**Design patterns**
1. **Realistic, two-handed:** pin → spoon held by grip → fuse on release or explicit spoon pop (cook). Used by Pavlov, H3VR and ITR.
2. **Arcade, one-handed:** a button on the holding hand arms, and the fuse runs in hand. Used by HL:A, Arizona Sunshine and Metro Awakening.

A clearly *visible and audible* countdown is the norm in both.

---

## 5. Throwing knives: making them land point-first

### 5.1 Splitting the drawn spin from the collision
- **Mesh and physics rotation are split.**
  - The *visual* mesh turns continuously about the knife's lateral axis.
  - The colliders and damagers turn only to **discrete angles where a forward-pointing damager (blade tip) faces the fly direction**.
- Each forward damager owns an angular window, computed from its angle to the fly direction: up to 180° of range per damager, plus 15° lead.
- The physics pose snaps to the active damager's window, so **whatever the visual spin phase, a hit uses a tip-forward collider**.
- A rotation speed of 5, the fly direction re-oriented by the grip used and moved to the centre of mass.
- **This is the key "stick every time" trick:** the spin is cosmetic and the collision geometry is cheated.

### 5.2 Meta Interaction SDK sample profiles [src]
(https://github.com/oculus-samples/Unity-InteractionSDK-Samples/tree/main/Assets/ShowcaseSamples/Throwing/Prefabs/ThrowProfiles)
- **Dart (no spin):**
  - velocityScale (1,1,**2**), velocityAdd (0,0,**+1 m/s**) along local forward, maxSpeed 50.
  - spinScale (0,0,0.1), **maxSpin 5**.
  - **alignForwardOnce = 1, keepForwardToVelocity = 1, forwardLerpSpeed 16**.
  - gravityScale **0.8**, constant local lift force (0, 0.4, 0), drag 0.002.
- **Axe (end-over-end):**
  - velocityScale (1.5,1,1), velocityAdd (1,0,0), maxSpeed 20.
  - **spinScale (0,1,0)**: only one spin axis is kept. maxSpin 200.
  - alignForwardOnce = 1, keepForward = 0.
  - **constant local torque (0,10,0)** keeps it tumbling. gravityScale 0.8, drag 0.002, angular drag 0.005.
- **Rugby:** velocityScale (1.6,1,2.4), spinAdd z 10, keepForwardToVelocity with lerp 15, maxSpin 200.
- **Sticking** (`CollisionHandler.cs`): on a collision with a "Target", ignore it if the contact collider is tagged "Handle". Otherwise set kinematic and parent to the target.

### 5.3 Hurricane VR stab thresholds [src]
(`HVRStabberSettings` / `HVRStabbableSettings`, same mirror as §1.4)
- Stab only if the dot(blade axis, collision direction) is greater than **`AngleThreshold = 0.5`**, i.e. within 60°.
- `RequiredVelocity = 1 m/s`.
- `OuterShellThickness = 0.025 m` with `OuterShellDamper = 5000`, then `Damper = 1000` scaled by a depth curve.
- `LockVelocity = 0.1 m/s` locks the stabber after `LockTime = 0.25 s`, with `UnlockForce = 300`.
- `FullStabDepth = 0.15 m`.

### 5.4 Complaints
- Into the Radius knives fly in an odd arc and drop quickly. Players are told to release earlier (https://steamcommunity.com/app/1012790/discussions/0/2260186248403802896/).
- That is the case for assist.

### 5.5 Recipe [inference, combining 5.1–5.3]
1. At release, decompose ω into a component about the knife's lateral axis and a remainder.
2. Choose the mode:
   - If |ω_lateral| exceeds a flick threshold (say 8–10 rad/s), use **spin mode**. Keep only lateral spin (Meta axe style), clamp it, and optionally quantise it so the tip faces forward at the predicted impact distance.
   - Otherwise use **dart mode**: align forward once, keep forward to velocity with lerp ~15–16/s, and damp the remaining spin to below 5 rad/s.
3. On impact: stick if the tip collider hit (not the handle), the speed is at least 1 m/s, and the blade dot velocity is at least 0.5 (≤ 60°). Widen to ~0.3 when assist is on. Otherwise bounce.
4. For spin mode, add the split-spin cheat (§5.1): evaluate the stick test against the tip-forward physics pose, not the visual phase.

---

## 6. Body storage and grabbing from a slot
- **HL:A:** two **wrist pockets**, one per forearm, holding one item each; the grenade wikis count "two in her wrist pockets". Reloads come "from their backpack" over the shoulder (Xen and frag wiki links; dev commentary; the spectator-HUD node shows "the contents of their wrist pockets").
- **Metro Awakening:** everything except ammo hangs on the backpack, which is pulled over the left shoulder. Grenades are a separate shoulder reach (right hand over left shoulder, per players). A review notes "minor instances of input misfires—such as grabbing the wrong item" (https://www.theaureview.com/games/metro-awakening-review/; Steam thread above).
- **Pavlov:** 3 grenades on the vest. **H3VR:** configurable quickbelt (§4).
- **HVR:** `HVRShoulderSocket` blocks storing during a forward throw (§1.4). `HVRSocket.CheckHandOverlap` requires the hand's "socket bag collider" to overlap the slot, so long objects aren't misplaced into body slots (release notes).
- **Torso anchoring:** a Resident Evil VR mod uses 3 zones (right shoulder, right hip, left chest) with a **vibration pulse when the hand enters a zone** (https://www.nexusmods.com/residentevilrequiem/mods/1081). A Skyrim VR holster mod uses 14 body points with calibration (https://nexusmods.com/skyrimspecialedition/mods/23416). A USPTO torso-orientation patent notes that head-anchored hip slots break when the user looks sideways (https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/11216045).
- **[inference] Slot grab flow:**
  1. Hover: the hand enters the slot volume, a haptic tick plays and the item highlights.
  2. Grip edge: transfer to the hand with the item at its grip pose and the pin facing the off hand.
  3. Make the item *unarmed* by construction: a slot never stores an armed grenade, or if it does, the timer keeps running.

---

## 7. Common complaints and the fix for each

| Complaint | Cause | Fix (source) |
|---|---|---|
| Throws go short, or "hit an invisible wall" | late bool release; averaging includes deceleration; lagging physics hand | release-anchored look-back to the speed peak (Alyx, Valve patent); analog drop-from-peak release (Deck); controller-based velocity (Deck, NormalVR); gain 1.1–1.5 (Valve, XRI) |
| Direction jitter | single-sample peak; noisy ω×r lever | average 2–3 samples around the peak, picking the peak on *controller* speed without the lever term (Alyx); regression for direction (Deck, Schoneveld); HVR direction filter (dot ≥ 0.2); gate ω×r above an ω threshold (HVR 1 rad/s) |
| Frame-rate dependence | differencing render-frame poses | runtime / IMU velocity (Moondust, Alyx); time-weighted windows in seconds, not frames (XRI) |
| Object clips the hand or body on release | hand colliders | disable hand↔object collision ≥ 0.2 s when v > 2 m/s (HVR) |
| Accidental holstering during wind-up | slot accepts any overlap | reject when throw velocity points away from the slot (HVR shoulder socket) |
| Grip fatigue | hold-to-hold for minutes | toggle-grip option (SuperHot change, itch post) |
| Grenade "artificially cut short" when armed | (HL:A) likely per-item tuning | keep grenade gain ≥ generic; visible arc preview optional (players praised the HL:A grenade-launcher laser arc, same thread) |
| Knives never stick | real spin phase is random | physics/visual spin split (§5.1); Meta dart alignment; relaxed stick angle |

---

## 8. Haptics and audio
- **Meta haptics design system** (https://developers.meta.com/vr/design/haptics-system/):
  - Grab conveys weight and resistance at contact. **Release mirrors the grab at lower amplitude and frequency.**
  - Button press is a spring "bounce-back" transient. Long press marks the moment the hold threshold is reached.
  - Hover uses a short sharp transient. Select uses two transients.
  - Success is 2 events over **175 ms** ramping up. Warning is 2 events over **150 ms** ramping down. Error is 4 abrupt events.
- **Meta grab best practices:** haptics are limited, so pair them with distinct audio and visual highlight (https://developers.meta.com/horizon/design/grab_bp/).
- **HL:A:** arming an injector adds haptic and audio weight. The grenade gives a **progressive light ring (5 lights) plus rhythmic beep plus red light**, and the Xen grenade clicks and glows. HL2's ticking and red light were added because unlit grenades were too hard to locate and escape (§4 links).
- **Metro Awakening (gameplay):** human enemies flee a *lit* grenade, so audio and visual arming is also an AI signal (realityremake link, §4).
- HVR plays a release haptic (`GrabHaptics.HandRelease`) on every release [src, §1.4].
- **[inference] Cue set:**
  - pin pull: two crisp transients + metallic "tink"
  - spoon pop or arm: one strong transient + "ping" or click; start a 1 Hz beep or tick that speeds up over the last ~1.5 s
  - in-hand armed: a faint pulse at each tick on the holding hand
  - release: the low-amplitude mirror of the grab
  - knife stick: a short heavy transient on the throwing hand only if the knife stuck (feedback at a distance is optional)
  - drop (no throw): no throw whoosh

---

## 9. Metro-specific note (game is the source of truth)
- The user's Metro mod has throwing knives and grenades.
- Per our framework principle, **fuse time, damage, bounce and projectile gravity come from the game's own grenade and knife data**. The runtime only supplies the release pose, velocity and spin, plus optional assist.
- The fallback defaults in §10 apply only when the adapter can't read a value.
- [inference] If the game's thrown projectile is a scripted ballistic entity with its own launch speed, the adapter should map our release velocity onto it, keeping our direction and a speed ratio clamped to the game's min/max. It should not override the game's physics constants.

---

## 10. Implementation parameter list (proposal; starting values to be validated in the test range)

**Sampling**
- One ring buffer per hand, of about 0.3 s, with **≥ 64 entries**.
- Each entry: `{t: XrTime the pose was located for, grip pose, v, ω (runtime if VALID else differenced, in play-space), squeeze value/force, trigger}`.
- Convert to world space only at release, and add rig locomotion velocity then (VRIF, quakevr). Rotate the buffered history by snap-turn or teleport deltas (XRI).

**Release detection (analog)**
- Grab when `squeeze ≥ 0.70`, then track `peak = max(squeeze)`.
- **Release when `squeeze < peak·(1 − 0.25)` or `squeeze < 0.35`** (Deck's ~20%; quakevr uses 0.25–0.30 and 0.35).
- On Index, also OR in `squeeze/force` falling below 10% (patent-style force-grip) or capacitive finger release, when exposed.
- **t_rel** is the `lastChangeTime` of the float action at the crossing (OpenXR). Use the newest sample time if it's unavailable.

**Estimator (Alyx / Valve-patent style)**
- **Window:** `[t_rel − 0.12 s, t_rel]`. That is ≈ 10–11 frames at 90 Hz, Alyx's 111 ms. The patent's 15 frames is 167 ms, so allow up to 0.17 s.
- **Peak:** `argmax |v_controller|`, without the lever term.
- **Linear v:** mean over samples with `|t − t_pk| ≤ 0.017 s` (about 3 frames at 90 Hz).
- **ω:** mean over ±0.034 s, because ω is noisier.
- **Tangential term:** `v += k·(ω × r)` with `r = item CoM − grip origin`. Use `k = 0.7–1.0` only if `|ω| > 1–6 rad/s` (HVR threshold 1, quakevr 6).
- **Optional direction refinement:** least-squares line over `[t_pk − 0.04, t_pk + 0.01]` (Deck, Schoneveld).
- **Freeze** the estimate at the release edge. Don't recompute it on later frames.

**Spawn**
- Release from the item pose at `t_pk`, backdated: `p_now = p_pk + v·age + ½g·age²`, with `age = now − t_pk` clamped to ≤ 0.15 s. This follows the Valve patent and quakevr.
- Sweep or trace from the hand to `p_now` so the item never spawns in a wall.
- Turn off hand and body collision with the item for **0.2 s** if `|v| > 2 m/s` (HVR).

**Gain and assist**
- **Drop:** if `|v| < 1.0 m/s`, it's a drop. Gain 1.0, no assist, no alignment, ω × 0.25.
- **Gain:** `1 + (G − 1)·smoothstep(1.5, 6.0, |v|)` with **G = 1.35 for grenades** and 1.25 for generic items. That is between Valve's 1.1 and XRI's 1.5; it is tunable, and users undershoot. Per-item weight `mix(1, item_mult, 0.25)`.
- **Clamp:** `|v| ≤ 25 m/s` (Meta axe 20, bowling 25); knives ≤ 30 m/s.
- **Spin:** ω scale 0.75 (between Auto Hand's ~0.5 and XRI's 1.0). Clamp **20 rad/s** generic and 40 rad/s for knives in spin mode. The runtime ceiling is 50 (SteamVR).
- **Aim assist** (opt-in, off by default for grenades, light by default for knives):
  - Gaze gate: the target is within **25°** of head forward (Rec Room's 20–30°); ignore gaze beyond 90°.
  - Throw cone **12–15°**, full strength inside **4°**, strength **0.35**.
  - Allowed speed change **≤ 15%**. Use a low-arc ballistic solve with target lead, and require line of sight.
  - Apply after a **≤ 10 ms natural-flight** stage, or blend at spawn (Rec Room patent).

**Grenades**
- **Fuse:** game data first. Fallback **4.0 s** (range 3.2–5 s: real UZRGM/M67; HL:A and Metro Awakening ~4–5 s; Arizona Sunshine ~3 s).
- **Impact type:** arm **1.0 s** after release (H3VR RGO); never detonate on a drop before arming.
- **Pin and arm scheme**, as a per-game adapter choice:
  - **Realistic (default for realistic adapters):**
    - Hold the grenade in grip. The off hand grabs the pin ring, which is a grab point, and pulls past a travel threshold, about 4–6 cm [inference], and the pin detaches.
    - While grip is held the spoon stays held. **The fuse starts on spoon release.**
    - The spoon releases either on the throw (grip release) or on an explicit cook: B/Y (ITR, H3VR Streamlined) or a thumb-stick click.
    - Re-gripping after release doesn't stop the fuse.
  - **One-handed / accessibility:** the trigger on the holding hand arms (HL:A "priming button", Arizona Sunshine touchpad). The fuse starts at arming.
  - Never require both hands when the off hand is holding a weapon, unless the game design wants that vulnerability (Pavlov's stated intent).
- **Armed feedback:** a progressive light ring or LED plus a beep whose rate rises in the last ~1.5 s (HL:A 5-light ring, HL2 chirp), plus the haptics in §8. Notify the game AI if the adapter supports it, since Metro humans flee lit grenades.
- **Catching:** with `catchingSpeedThreshold`-style logic, a live grenade caught mid-air attaches on grip *hold* (SteamVR). Keep its timer running (HL:A design allows return-throws thanks to long enemy fuses).

**Throwing knives**
- Author a `fly_dir` (tip axis) plus a `lateral` axis per item.
- **Mode select at release:** spin if `|ω·lateral| > 8 rad/s` [inference], else dart.
  - **Dart:** align forward once, keep forward to velocity with lerp **16/s**, ω → ≤ 5 rad/s, gravity scale **0.8–1.0** (Meta dart: game data first).
  - **Spin:** keep lateral spin only (Meta axe spinScale (0,1,0)). Visual mesh spins and the physics pose snaps tip-forward within a **±15° lead / 180° window** (§5.1).
- **Stick test:** tip collider (not handle), **`|v| ≥ 1 m/s`**, **`dot(tip, v̂) ≥ 0.5`** (HVR), relaxed to 0.3 with assist on. On stick: kinematic, parent to the hit body, and make it grabbable for retrieval (Meta sample).

**Storage slots (torso-anchored)**
- Anchor slots to a **torso yaw** estimate (smoothed head yaw with hysteresis, or the shoulder line), not head yaw.
- Defaults: 2 chest pouches for grenades (left and right of the sternum), 1–2 belt slots, an optional over-shoulder backpack reach (Metro Awakening style), and an optional wrist pocket per forearm (HL:A).
- Hover radius ~0.10–0.12 m [inference], with a haptic tick on entry (RE VR mod).
- Reject stowing while `throw_velocity · slot_forward > 0` (HVR).
- Grenades come out of a slot unarmed, with the pin facing the off hand.

**Logging, for the test range**
- Per throw: `t_rel − t_pk` (expect 20–120 ms), peak versus release speed, |ω| and the tangential share, gain, assist delta (in degrees and speed %), and the landing error versus the target at 5, 10 and 15 m (quakevr test plan).
