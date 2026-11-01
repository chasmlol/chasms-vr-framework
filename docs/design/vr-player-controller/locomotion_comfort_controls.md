# VR Locomotion, Comfort Options and Controller Mappings for First-Person VR Shooters/Action Games

> A research snapshot (2026-10-21), not a description of the framework's code: what was built is in
> `docs/`.

Scope: implementation-oriented notes for a game-agnostic locomotion/comfort system and default control scheme (Meta Quest Touch + Valve Index). Researched 2026-10-21. Legend: **[UNVERIFIED]** = from the researcher's prior knowledge, not confirmed by a source in this session; treat it as an estimate. "XRI" = Unity XR Interaction Toolkit (3.x source via the needle-mirror GitHub copy of the package).

---

## 1. Locomotion options and how the best games implement them

### Takeaway
Shipped games offer the same menu of modes: teleport ("blink", with a fade), dash/"shift" (a fast visible slide to the target), and smooth/continuous stick movement, which is either head-oriented or hand-oriented. Valve's Alyx exposes all of these with four named modes. Physics-body games (Boneworks/Bonelab), arm-swingers (H3VR, Stride) and grab-and-push zero-g (Lone Echo) are well-loved alternatives. Titanfall 2 VR shows that parkour-speed motion is playable for some people but is "not for everyone".

### Cited Findings
**Half-Life: Alyx (Valve)**
- Alyx ships three movement families, Continuous, Shift and Blink teleport. Blink fades to black, Shift teleports with no fade, and Continuous follows head orientation; a continuous hand-oriented mode was added later — [UploadVR via search](https://uploadvr.com/valve-deep-dive-locomotion/); [TechRadar](https://www.techradar.com/news/new-half-life-alyx-gameplay-videos-showcase-three-types-of-vr-movement)
- The in-game mode names are **Blink** (teleport with fade in/out, recommended for beginners, lets you rotate during the teleport), **Shift** (zooms you to the destination with a rapid animation that "may cause discomfort"), **Continuous** (stick movement, forward = head) and **Continuous Hand** (forward = aimed hand, head only looks around) — [SegmentNext](https://segmentnext.com/2020/03/25/half-life-alyx-accessibility-settings-guide)
- Greg Coomer (Valve) says the main goal of the teleport systems is "to ensure the end player position is a valid place for the player's body" — [UploadVR](https://uploadvr.com/valve-deep-dive-locomotion/)
- Valve gave players "a standard minimum body size" while teleporting, after taller testers kept hitting their virtual heads on low pipes. The movement system was changed because of this — [PC Gamer headline](https://www.pcgamer.com/uk/half-life-alyxs-movement-system-was-changed-because-tall-testers-kept-getting-stuck/); [UploadVR via search summary](https://uploadvr.com/half-life-alyx-teleporting/)
- Robin Walker (Valve): "when we started we had this idea that teleport was a concession of some kind to free movement", and later "We don't think there's actually that significant a difference at all between the experience". In the fiction Alyx still walks to each point. Enemy AI tracks the teleport path (it notices if you teleport across a corridor), and the clothing simulation plays drop and landing sounds when the traversal includes a drop — [UploadVR](https://uploadvr.com/half-life-alyx-teleporting/)
- Footstep volume and timing scale with the distance moved. Playtesters expected a louder landing when teleporting from high to low, and Valve implemented that — [UploadVR](https://uploadvr.com/valve-deep-dive-locomotion/)
- In Alyx, movement uses the non-dominant controller's stick/pad and teleport sits on the dominant hand — [search summary of Alyx guides, e.g. gameplay.tips](https://gameplay.tips/guides/7161-half-life-alyx.html)
- Players asked for a dedicated jump and sprint at launch (Index), which suggests Alyx had no conventional jump button at release — [Steam discussion](https://steamcommunity.com/app/546560/discussions/0/1861616237338853226)

**Metro Awakening (Vertigo, 2024)**
- It has two presets, **Comfort** (teleport-focused) and **Immersion**. Immersion defaults to continuous stick movement with snap rotation, and smooth rotation is available in Settings. Immersion also includes forced camera movement: being moved around a bed, knocked over and waking on the floor, being thrown off a rail car — [Road to VR](https://www.roadtovr.com/metro-awakening-screenshots-details-locomotion/); [GamingBolt](https://gamingbolt.com/metro-awakening-features-two-locomotion-settings-ps-vr2-support-detailed)

**Arizona Sunshine 2 (Vertigo)**
- Pre-launch info lists Teleport and Sliding/Smooth locomotion, plus vignetting and snap turning, with no free button reassignment planned — [Mixed News](https://mixed-news.com/en/arizona-sunshine-2-info-guide/)
- The original Arizona Sunshine launched teleport-only, with an endurance meter, and got full locomotion in a later update — [UploadVR review](https://uploadvr.com/arizona-sunshine-review/); [UploadVR locomotion update](https://uploadvr.com/arizona-sunshine-locomotion-update/)

**Boneworks / Bonelab (Stress Level Zero): physics body**
- Boneworks replaces floating hands with a fully simulated body driven by IK from the head and hands. The world can push that body around and objects often block it. Stick movement works like a non-VR game, while sprinting, jumping and crouching happen through controller inputs or real movement — [Wikipedia: Boneworks](https://en.wikipedia.org/wiki/Boneworks)
- The developers say everything that moves in Boneworks, the player included, is moved by physical forces and simulated — [UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)
- Brandon Laatsch (SLZ) on fatigue: "Eventually playing just gets way too tiring for long sessions", and "We had to figure out what a longer form VR gameplay session looks like". He contrasts Boneworks with Sprint Vector's short intense bursts — [UploadVR](https://uploadvr.com/boneworks-body-vr-game-mechanic)
- Per a mod author, Bonelab "already has a full crouch rig — a knee-to-pelvis joint, a crouch input". The DropProne mod adds prone, toggled with left-stick click by default — [Thunderstore DropProne](https://thunderstore.io/c/bonelab/p/Lemmys/DropProne/)

**Arm-swing locomotion (H3VR, Stride, ArmSwinger)**
- H3VR's built-in modes, inferred from mod pages, include Armswinger, Twinstick and Dash, selected from the wrist menu. Popular mods combine twin-stick direction with arm-swing speed (stick sets direction, swinging adds speed) or use head direction plus an ArmSwinger button — [Thunderstore TwinStickArmSprint](https://thunderstore.io/c/h3vr/p/ODekaK/TwinStickArmSprint/v/1.1.5/); [Thunderstore HeadArmSwing](https://thunderstore.io/c/h3vr/p/NGA/HeadArmSwing/)
- A Steam user suggested that every VR game should offer H3VR-style "armswinger" because it avoids stick drift and dead-zone problems — [Steam ITR2 discussion](https://steamcommunity.com/app/2307350/discussions/0/4518883513868880108)
- ArmSwinger is an MIT-licensed Unity library (Electric Night Owl) with "extensive options for tweaking" and an in-game settings panel — [GitHub ArmSwinger](https://github.com/Keron320/ArmSwinger); [UploadVR](https://uploadvr.com/armswinger-locomotion-system-vr/)
- **Stride**: running comes from physically pumping your arms, which is faster than the "snail's pace" of a full-throttle left stick. Jumping means holding a button and throwing your arms forward, and an Arcade option lets you jump by thrusting your arms instead of pressing A. UploadVR calls it "an intense game from a comfort perspective" and says wall-runs and slides can feel "particularly jarring". Road to VR found the Quest version "exceedingly comfortable" because you're "nearly always in direct control" — [UploadVR Stride review](https://uploadvr.com/stride-review/); [Road to VR Stride Quest review](https://roadtovr.com/stride-quest-review/)
- Stride: Fates lists snap, quick and smooth turning; teleport, dash and smooth movement; head- or controller-based blinders (vignette); a swappable movement hand; standing/seated modes; artificial and real crouch — [Road to VR Stride: Fates review](https://roadtovr.com/stride-fates-review-quest-3-steam-vr/)

**Zero-g / climbing (Lone Echo)**
- In Lone Echo you move by grabbing and pushing off the environment. In Ready At Dawn's GDC talk, players can "climb over anything they can see, then push off and gently sail in any direction". Hand poses are generated procedurally to grab arbitrary surfaces, and one constraint system drives all player locomotion. Thrusters are secondary, unlike thruster-first ADR1FT — [GDC Vault: It's All in the Hands](https://www.gdcvault.com/play/1024240/contactUs); [Road to VR hands-on](https://roadtovr.com/hands-on-lone-echo-nails-zero-gravity-in-vr-with-oculus-touch/); [Wikipedia](https://en.wikipedia.org/wiki/Lone_Echo)
- XRI's climbing component has per-axis freedom flags (`m_AllowFreeXMovement/Y/Z = true` by default), so climbing can be limited to a ladder axis — [XRI ClimbSettings.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Climbing/ClimbSettings.cs)

**Titanfall 2 VR (CircuitLord mod, released ~Sept/Oct 2026; campaign only, built on Northstar)**
- Features: stereo rendering, full-body IK, manual reloads, Titan controls. You launch it through the installer's "Launch in VR". Saves live in `%LOCALAPPDATA%\Respawn\Titanfall2_VR` — [GitHub CircuitLordVRModInstaller](https://github.com/CircuitLord/CircuitLordVRModInstaller)
- "Crouching and sliding is done by flicking the stick down or by literally crouching". Secondary abilities use the left grip plus moving the controller in one of four directions. Cloak is on the offhand trigger, grenades are ripped off the chest, and the secondary weapon sits on the left hip — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/)
- Comfort: there is a snap-turn option, and cutscenes play on a floating 2D screen by default. Head-tracked 3D cutscenes are in an advanced submenu "with a big, explicit warning that forced VR cutscenes are too intense". The Aftermath writer found wall-running "surprisingly organic" and "never once got motion sick" — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/)
- Road to VR: the motion "goes above and beyond even a regular flatscreen shooter"; the mod "is not going to be for everyone—or probably even *most* people", and there is "only so much that can be done" about sickness from wall-running and grappling — [Road to VR](https://www.roadtovr.com/titanfall-2-vr-mod-circuitlord/)
- UploadVR: double-jump, wall-run and sprint-slide work independently of where you aim. Wall-running is "super useful" but loses fluidity versus flatscreen. Weapon swapping is "cumbersome and clunky" — [UploadVR review](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/)
- A Chinese report mentions an "assist-movement" option for motion sickness — [search summary of ifeng](https://tech.ifeng.com/c/8wvWjz8b1QS) (not confirmed in English sources)

**HL2VR (Source VR mods)**
- HL2VR Unleashed has a physical-jumping toggle (a real-world jump triggers an in-game jump) with the threshold in CVar `hlvr_control_physical_jump_threshold`. Dual wielding needs Fire, Alt Fire, Eject Magazine and Open Weapon Selection bound on both hands. The author suggests unbinding left-hand Sprint so the stick click can open weapon selection, and rebinding Toggle Menu to a long press or chord. The last-equipped weapon auto-holsters when you release grab — [Mixed News](https://mixed-news.com/en/half-life-2-vr-mod-unleashed/); [Road to VR](https://roadtovr.com/half-life-2-vr-mod-dual-wield-steam/)

**Meta guidance on modes**
- Meta lists blinks, snap turns, quick turns, smooth turns, position warps and teleports as reorientation methods that reduce vection. It suggests "stepped translations", meaning movement as "a series of small, rapid teleports", and "quantized velocity" (fixed speeds such as stopped, walking and running, switched instantly). It also suggests teleport nodes for slopes and stairs — [Meta Locomotion Best Practices](https://developers.meta.com/horizon/design/locomotion-best-practices/); [Meta Locomotion Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)
- Meta's IWSDK says to default to teleport + snap turn for new users and to offer both teleport and slide — [Meta IWSDK locomotion](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/)

**Head- vs hand-oriented movement**
- A 2020 thesis (Blekinge Institute of Technology, n=15) found controller-oriented movement rated more natural and likeable and less restrictive or difficult, with no significant difference in simulator sickness or task performance — [nweon paper summary](https://paper.nweon.com/?p=6638) (search summary; page not retrievable)
- Meta says there is no definitive answer and the user should pick the mapping — [Meta Locomotion User Preferences](https://developers.meta.com/horizon/design/locomotion-user-preferences/)
- Meta's IWSDK slide guidance: "Use viewer-relative movement and avoid rotating input by controller grip to reduce unintended strafing" — [Meta IWSDK slide (search summary)](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/)
- Reddit and Steam opinion is split. Some prefer head-oriented movement because it feels natural when looking around; others say controller-oriented wins forum polls. A Reddit claim that "90% prefer controller" is unsourced — [r/Vive mirror](https://nyc1.lr.ggtyler.dev/r/Vive/comments/9fd5v8/unpopular_vr_opinions_2018_thread/e5xc1fd/?context=3)
- Pavlov exposes an "Input Vector" setting (Head / Dominant Hand / Offhand) — [Pavlov Wiki Game Settings](https://pavlovwiki.com/index.php/Game_Settings)
- XRI's `ContinuousMoveProvider.m_ForwardSource` defaults to null, which falls back to the camera (head-oriented) — [XRI ContinuousMoveProvider.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Movement/ContinuousMoveProvider.cs)

### Inferences
- The minimum viable mode set, matching Alyx, Stride: Fates and the Road to VR checklist: {Teleport (blink with fade), Dash/Shift, Smooth}, each with {Head-oriented, Hand-oriented (L or R)} direction, plus a swappable movement hand. Implement teleport as one "validated destination for a standard-size body" query that smooth movement's jump/mantle can reuse, which is what Alyx did with its standard minimum body size.
- For hand-oriented movement, flatten the controller's forward vector to the horizontal plane and ignore controller roll; this follows Meta's advice against rotating input by grip. **[Estimate]** Use the controller's pointer/aim pose, not the grip pose, as the direction source.
- Default for shooters: Smooth + Snap turn, as in Metro's Immersion preset, with a first-launch "Comfort" preset (Teleport + Snap + Vignette) and an "Advanced" preset (Smooth + Smooth turn, no vignette), following Meta's Recommended/Comfortable/Advanced tiers.
- Treat scripted forced-camera moments as opt-in (Titanfall 2 VR's 2D cutscene default, Metro's Comfort preset dropping forced motion).
- **[UNVERIFIED]** In Alyx's continuous modes, the dominant-hand stick pushed forward brings up a teleport-style arc for jumping gaps and climbing ledges, so a "jump" is a short validated teleport, not a physics jump. This needs confirmation.
- **[UNVERIFIED]** Bonelab's body is a multi-rigidbody rig (pelvis, knee, a sphere "locomotion ball" for feet) that rolls under the player. Only the knee-pelvis crouch joint is confirmed (Thunderstore).

### Gaps
- No primary Valve GDC talk text on Alyx locomotion was found. The ~11-minute Valve deep-dive video was summarised only second-hand, with no numbers for teleport range, arc velocity or body capsule size.
- No official full control/option documentation was found for Titanfall 2 VR (the README has no bindings). How wall-run camera roll is handled is not described anywhere found.
- No sources were found for The Climb's grip/stamina parameters or Skyrim VR's locomotion internals.
- Bonelab's physics-rig internals (sphere, joint stiffness) are not documented in any source found.

---

## 2. Turning: snap angles, smooth speeds, cooldowns, repeat, pivot point

### Takeaway
Snap turn is the comfort default. Engine defaults are 45° (Unity XRI, OVRPlayerController) and 30° (a Meta IWSDK example). Players expect options of 15° to 90°: Alyx uses 15° steps up to 90°, and Meta recommends offering 30/45/90. Smooth turn defaults range from 60°/s (XRI) to 120°/s (an IWSDK example), and Pavlov lets players choose 45–360. Turning should pivot around the head's floor projection, not the playspace origin.

### Cited Findings
- XRI `SnapTurnProvider`: `m_TurnAmount = 45f`, `m_DebounceTime = 0.5f` (wait before another snap), `m_EnableTurnLeftRight = true`, `m_EnableTurnAround = true` (stick back = 180°), `m_DelayTime` = 0 — [XRI SnapTurnProvider.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/SnapTurnProvider.cs)
- XRI `ContinuousTurnProvider`: `m_TurnSpeed = 60f` (°/s), turn-around disabled by default — [XRI ContinuousTurnProvider.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/ContinuousTurnProvider.cs)
- Meta OVRPlayerController (older script excerpt): `RotationRatchet = 45.0f` (snap degrees), `SnapRotation = true`, `RotationAmount = 1.5f` (smooth multiplier) — [teratail excerpt](https://teratail.com/questions/285432?rss=all); field meanings at [Meta API ref](https://developers.meta.com/horizon/reference/unity/v78/class_o_v_r_player_controller)
- Meta IWSDK code example: `turningAngle = 30` ("degrees per snap") and `turningSpeed = 120` ("degrees per second"). Snap triggers "when the turn action crosses its activation state". The docs recommend snap as the initial mode, user-selectable snap angle and smooth speed, and no second yaw transform on the camera — [Meta IWSDK turn](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/turn/)
- Meta design: snap turn should "provide the following options: 30, 45, or 90 degrees", and smooth-turn speed should be adjustable — [Meta Locomotion User Preferences](https://developers.meta.com/horizon/design/locomotion-user-preferences/). Separately, restricting movement or turning to specific angles "like 15, 30, 45, 90, or 180 degrees" reduces disorientation — [Meta Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)
- Meta: "Offer smooth turning as an opt-in feature, tuning speed and acceleration carefully" — [Meta Best Practices](https://developers.meta.com/horizon/design/locomotion-best-practices/)
- Alyx: Quick Turn angles come in 15° increments up to 90° — [SegmentNext](https://segmentnext.com/2020/03/25/half-life-alyx-accessibility-settings-guide). Patch 1.1 added continuous controller turning, renamed "quick turn" to snap turn, and added an option to disable controller turning — [PC Gamer (search summary)](https://pcgamer.com/half-life-alyx-smooth-turning). Road to VR's checklist lists Alyx snap turn as "adjustable increments" and smooth turn as "adjustable speed" — [Road to VR checklist](https://roadtovr.com/vr-comfort-settings-checklist-glossary-developers-players/)
- Road to VR's turning glossary: snap-turn (instant steps, "comfortable for most"), quick-turn (fast animated steps, also called dash-turn, "suits some"), smooth-turn ("least comfortable for most") — [Road to VR glossary](https://roadtovr.com/vr-comfort-settings-checklist-glossary-developers-players/)
- Pavlov: "Enable Artificial Turn" plus "Smooth Artificial Turn" with a range of 45–360 (presumably °/s) — [Pavlov Wiki Game Settings](https://pavlovwiki.com/index.php/Game_Settings)
- Pivot point: XRI's body transformer estimates body position as "the camera position projected onto the XZ plane of the XR Origin" (`UnderCameraBodyPositionEvaluator`), and yaw is applied through that `XRMovableBody` (default transformation `XRBodyYawRotation`) — [XRI XRBodyTransformer.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/XRBodyTransformer.cs); [XRI ContinuousTurnProvider.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/ContinuousTurnProvider.cs)
- The NomaiVR mod has a user issue asking for faster snap and larger angle options, which shows users want snap speed and angle configurable — [NomaiVR issue #457](https://zff.dev/Raicuparta/NomaiVR/issues/457)

### Inferences
- Rotate the tracking origin around the vertical axis through the HMD's floor-projected position, not around the origin. Otherwise the user's head swings in an arc, which adds translation and causes sickness. XRI's design shows this. The pivot math is `origin = R(θ) · (origin − headXZ) + headXZ`. **[UNVERIFIED]** XRI's `XRBodyYawRotation` pivots about the body ground position; the BodyTransformations source file returned 404.
- Recommended defaults (estimate): snap 45° (options 15/30/45/60/90), stick back = 180° turn-around (XRI default on), stick threshold ~0.5–0.75 to fire and re-arm below ~0.25 (hysteresis), 0.25–0.5 s repeat interval while held (XRI uses 0.5 s debounce). Smooth turn 90°/s default with a 30–360°/s slider (between XRI's 60 and IWSDK's 120; Pavlov range 45–360). Optionally ramp smooth-turn speed with stick deflection.
- **[UNVERIFIED]** Many games add a 1–3 frame fade or "blink" on snap turn; offer an optional short fade (~50–100 ms). No source was found for the duration.
- Snap turn should also work during teleport aiming (Alyx Blink allows rotation during the teleport).

### Gaps
- No source was found for Alyx's default snap angle or smooth-turn speed, or for Bonelab, H3VR and ITR snap defaults.
- No study-backed value was found for the optimal snap cooldown or repeat-while-held behaviour. XRI's 0.5 s debounce is the only documented number.

---

## 3. Comfort systems (vignette, acceleration, camera rules, head collision, seated, height, handedness, accessibility)

### Takeaway
Comfort features fall into a well-agreed checklist: vignette/blinders, snap/teleport, head- vs hand-direction, seated mode, artificial crouch, adjustable height, handedness and single-hand play. Implement them as orthogonal settings. Vignettes should react to linear speed, angular speed and acceleration, ease in and out over ~0.3 s, and be user-scalable from 0 (off) upward. Acceleration should be brief or eliminated, since the vestibular system responds to changes in velocity, not constant velocity.

### Cited Findings
**Vignette / tunnelling**
- XRI `TunnelingVignetteController` defaults: `apertureSize = 0.7` (0..1, where 1 = no vignette), `featheringEffect = 0.2`, `easeInTime = 0.3 s`, `easeOutTime = 0.3 s`, `easeInTimeLock = false`, `easeOutDelayTime = 0 s`, color black, `apertureVerticalPosition = 0` — [XRI TunnelingVignetteController.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Comfort/TunnelingVignetteController.cs)
- When several locomotion providers trigger the vignette, XRI uses the smallest aperture. Providers drive ease-in and ease-out through their locomotion state — [XRI manual](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/tunneling-vignette-controller.html)
- Meta Interaction SDK `LocomotionTunneling` triggers occlusion on rotation above a speed, movement above a linear speed, and acceleration ("this includes accelerations and decelerations"). The vignette is "directly defined in degrees", so it is independent of headset FOV, and it can be eye-directed — [Meta ISDK locomotion comfort](https://developers.meta.com/horizon/documentation/unity/unity-isdk-locomotion-comfort/)
- Meta IWSDK: the vignette scales with input (near-invisible at small stick input, more occlusion at full tilt), with 0.4–0.6 a common default and 0 to disable. One version suggests fading quickly (lerp ~10 Hz) to avoid a laggy feel, or enabling it only while moving with a short fade-out on stop — [Meta IWSDK slide (search summary)](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/)
- Pavlov: "Movement Vignette" strength slider, 0–100% — [Pavlov Wiki](https://pavlovwiki.com/index.php/Game_Settings)
- Research: Fernandes & Feiner's "Combating VR Sickness through Subtle Dynamic Field-Of-View Modification" (IEEE 3DUI 2016 best paper, n=30, multi-day crossover) found that soft-edged dynamic FOV restrictors, which narrow during perceived motion, significantly reduced sickness. Presence did not drop and most participants did not notice — [IEEE Spectrum](https://spectrum.ieee.org/dynamic-field-of-view-restriction-makes-virtual-reality-less-barfy); [ScienceDaily](https://www.sciencedaily.com/releases/2016/06/160614121922.htm)
- Road to VR defines "blinders" as narrowing the field of view at the edges to reduce peripheral motion (also called vignette). Stride: Fates offers head-based or controller-based blinders — [Road to VR glossary](https://roadtovr.com/vr-comfort-settings-checklist-glossary-developers-players/); [Road to VR Stride: Fates](https://roadtovr.com/stride-fates-review-quest-3-steam-vr/)

**Acceleration and velocity**
- The vestibular organs "respond to changes in motion but not to constant velocity", so accelerations should be short and infrequent. Meta suggests quantized velocity with instant switching between stopped, walk and run — [Meta Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)
- "Keep acceleration events brief and infrequent", limit acceleration duration and frequency, control camera elevation, restrict movement axes, apply "temporal occlusion during rapid movement", and use "independent backgrounds" that move only with the head — [Meta Best Practices](https://developers.meta.com/horizon/design/locomotion-best-practices/)
- OVRPlayerController uses smoothed movement (`Acceleration = 0.1f`, `Damping = 0.3f`, `BackAndSideDampen = 0.5f` scaling back and strafe input), meaning slower backward and sideways movement — [teratail excerpt](https://teratail.com/questions/285432?rss=all)

**Camera control rules**
- Avoid deriving height only from device elevation; stabilise elevation by repositioning the camera only when necessary. Standing and crouching transitions should be "brief but not instant" — [Meta Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)
- Do not add a second yaw transform on the camera; rotate the rig — [Meta IWSDK turn](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/turn/)
- Metro Awakening puts forced camera moves (knock-downs, thrown off a rail car) only in the Immersion preset, and Titanfall 2 VR defaults cutscenes to a 2D screen with a warning on the 3D option — [Road to VR Metro](https://www.roadtovr.com/metro-awakening-screenshots-details-locomotion/); [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/)
- Keep frame rate consistent. Stutters are amplified at 72–120 Hz, so run locomotion off the main thread where possible — [Meta Best Practices](https://developers.meta.com/horizon/design/locomotion-best-practices/); [Meta IWSDK slide (search summary)](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/)

**Head collision with walls**
- Meta recommends "soft collisions", where "the camera slows down before stopping, rather than an abrupt halt" — [Meta Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)
- Developer forums (Meta/Oculus, Unity, Unreal) mostly recommend fade to black when the head enters geometry, saying Oculus recommended it and that it discourages looking through walls. Pushing the body back felt "harsh and disruptive even when smoothed", and fast head movement still clipped. Other suggestions were a hybrid (soft push plus fade on deliberate clipping), padded collision, and level design that keeps heads out of walls. A Minecraft VR mod fades only for opaque blocks and lets the head pass through glass — [Meta forum](https://communityforums.atmeta.com/discussions/dev-unity/looking-throug-walls-with-position-tracking/177837); [Unity forum](https://forum.unity.com/threads/how-do-you-handle-not-going-through-walls-in-steamvr-using-fading-to-black.811362/); [Unity discussion](https://discussions.unity.com/t/going-through-walls-when-moving-irl/1536611) (community opinion, not official guidance)
- Research ("You Shall Not Pass", IEEE VR 2018): without feedback most participants walked through virtual walls; with non-intrusive feedback 87% refrained — [unpaywall DOI 10.1109/VR.2018.8446177](https://unpaywall.org/10.1109%2FVR.2018.8446177) (numbers via search summary)

**Seated mode, height and crouch**
- Seated: default avatar and camera height "to the player height reported by the system". "Artificial turning is necessary, since not all chairs spin" — [Meta Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)
- Meta ISDK `CapsuleLocomotionHandler` has a `Height Offset` for seated players, "pushing the virtual-floor down, so they reach all interactables placed at standing height" — [Meta ISDK comfort](https://developers.meta.com/horizon/documentation/unity/unity-isdk-locomotion-comfort/)
- Alyx Height Adjust options: **Crouch only** (stand disabled), **Stand only** (a toggle raises your position for reach; recommended for seated play), **Crouch and stand** (two separate inputs, or a hybrid single input). The modes are controller toggles. Seated play is calibrated in SteamVR Room Setup, then height is set in the in-game Accessibility menu, which is available mid-game — [Road to VR](https://roadtovr.com/half-life-alyx-accessibility-options-subtitles-seated-single-controller/); [GameSpot seated guide](https://gamespot.com/articles/how-to-play-half-life-alyx-seated/1100-6475084/) (one guide says four height options but lists three; the fourth is probably "off", unconfirmed)
- Road to VR distinguishes artificial crouch (button) from real crouch, and "adjustable player height" (persistent, independent of real height) from crouch — [Road to VR glossary](https://roadtovr.com/vr-comfort-settings-checklist-glossary-developers-players/)

**Handedness and accessibility (Alyx as the reference)**
- Alyx options: Dominant Hand (right or left), Single Controller (all actions on one controller), Height Adjust, Seated Mode ("fixes settings so you never need to physically stand"), Weapon Selection (Hand / Head / Hybrid pointing), Light Sensitivity, subtitles and closed captions (size and width adjustable), and Difficulty (Story, Easy, Normal, Hard) — [SegmentNext](https://segmentnext.com/2020/03/25/half-life-alyx-accessibility-settings-guide); [Road to VR](https://roadtovr.com/half-life-alyx-accessibility-options-subtitles-seated-single-controller/); [Windows Central](https://windowscentral.com/accessibility-mind-half-life-alyx-can-be-played-single-hand)
- Road to VR's checklist marks Alyx as: two hands not required, real crouch not required, hearing not required, no adjustable player height — [Road to VR checklist](https://roadtovr.com/vr-comfort-settings-checklist-glossary-developers-players/)
- Pavlov: Dominant Hand, Swap Locomotion Hand, and "Inventory Lock" (locks inventory rotation if you look down) — [Pavlov Wiki](https://pavlovwiki.com/index.php/Game_Settings)
- Meta: let users change preferences in real time and persist them across sessions — [Meta Locomotion User Preferences](https://developers.meta.com/horizon/design/locomotion-user-preferences/)

### Inferences
- Vignette design (estimate, based on XRI and Meta ISDK): compute a target aperture each frame as `min(aperture_linear(speed), aperture_angular(yawRate), aperture_accel(|a|))`, then ease toward it (~0.3 s in, ~0.3 s out, optional 0–0.2 s out-delay). Scale it with a user "strength" of 0–1, default ~0.5 per IWSDK's 0.4–0.6. Define the aperture in degrees of visual angle, not screen fraction, so it behaves the same across headsets (Meta ISDK). Render it head-locked in each eye's view, centred on the view direction (or eye-gaze if available), with a soft feather of ~0.2. Black, or an optional world-anchored grid/"cage" as an "independent background" per Meta.
- Default to instant velocity changes (quantized: 0 → walk speed in ≤0.1 s) rather than long ramps. If ramps are used, keep them ≤~0.1–0.2 s and trigger the vignette during them. **[Estimate]** The OVRPlayerController acceleration/damping values are per-frame factors, not seconds.
- Hard rule: the camera never moves or rotates without user input except in opt-in scripted moments; scripted moves fade or blink, or use a 2D screen.
- Head-in-wall: run a sphere test (r ≈ 0.1–0.15 m, estimate) at the HMD each frame. Begin a soft push or fade as penetration starts, fade to black at a depth of ~0.05–0.1 m, and fully black-out inside geometry. Never block HMD pose updates.
- Expose: dominant hand, swappable movement hand, single-controller mode, seated mode + height offset, artificial crouch (toggle/hold/analog), snap/smooth turn, teleport/dash/smooth, direction source, vignette strength, subtitle options. Persist all of these.

### Gaps
- No published numbers were found for "best" vignette aperture in degrees, speed thresholds that trigger it, or ideal fade times beyond the XRI defaults.
- No source was found for Valve's own comfort research (Steam Dev Days etc.). No source documents Alyx's head-in-wall behaviour.
- The Fernandes & Feiner FOV values (degrees) were not retrieved; see DOI 10.1109/3DUI.2016.7460053.

---

## 4. Roomscale: player capsule following the head, walking into walls, leaning, reconciliation

### Takeaway
The standard solution: the collision capsule's XZ follows the HMD's floor projection and its height follows HMD height. Physical walking is applied as a collision-checked move of the rig, so walls stop the rig, not the head. Leaning over obstacles is allowed because the HMD can move beyond the capsule. Head penetration is handled with fade or soft push. Physics-body games (Boneworks/Bonelab) go further, with a simulated body that the world can block and push.

### Cited Findings
- XRI `CharacterControllerBodyManipulator`: each move, `height = CameraInOriginSpaceHeight − bodyGroundPosition.y` and `center = (groundX, groundY + height/2 + skinWidth, groundZ)`, then `characterController.Move(motion)` resolves collisions. There is no min/max clamp in that code. If the controller is disabled, the origin moves without collision — [XRI CharacterControllerBodyManipulator.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/CharacterControllerBodyManipulator.cs)
- XRI body ground position defaults to the camera projected onto the origin's XZ plane (`UnderCameraBodyPositionEvaluator`), and a CharacterController is used automatically if present (`m_UseCharacterControllerIfExists = true`) — [XRI XRBodyTransformer.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/XRBodyTransformer.cs)
- A common bug: a capsule on the rig that does not follow the head, or a collider parented to the camera that cannot stop the camera. The suggested fix is to move the VR origin in world space (sweep the offset) rather than the camera. Stopping the camera from following the HMD may cause sickness — [Unreal answers](https://answers.unrealengine.com/questions/413286/view.html); [Unreal forum](https://forums.unrealengine.com/t/how-do-i-stop-moving-through-walls/374521)
- In roomscale, virtual walls cannot physically stop the player; non-intrusive feedback stopped 87% of participants walking through — [unpaywall: You Shall Not Pass](https://unpaywall.org/10.1109%2FVR.2018.8446177)
- Alyx teleports validate the destination for "a standard minimum body size", because taller testers hit low pipes — [UploadVR](https://uploadvr.com/valve-deep-dive-locomotion/); [PC Gamer](https://www.pcgamer.com/uk/half-life-alyxs-movement-system-was-changed-because-tall-testers-kept-getting-stuck/)
- Boneworks: the IK body, driven by head and hands, can be pushed around and is often blocked by objects; everything, the player included, is moved by physical forces — [Wikipedia](https://en.wikipedia.org/wiki/Boneworks); [UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)
- Meta ISDK `CapsuleLocomotionHandler` is the capsule-based locomotion handler, with a seated height offset — [Meta ISDK comfort](https://developers.meta.com/horizon/documentation/unity/unity-isdk-locomotion-comfort/)

### Inferences
- Recommended algorithm (estimate): each physics tick, (1) read the HMD pose in tracking space; (2) compute the desired capsule foot position = HMD XZ (optionally offset ~0.1 m behind the eyes toward the neck); (3) sweep the capsule from its previous position to the desired one. Whatever displacement is blocked is the "unreconciled" offset; shift the tracking origin by its negation so the head stays at the wall (XRI-style move of the origin with collision). Alternatively, keep the capsule at the wall and let the head lean past it up to a lean limit (~0.3–0.5 m), then fade. (4) Capsule height = HMD height − floor + small head-room, clamped to [crouch-min ~0.5 m, standing max]. XRI lacks a clamp, so add one. (5) Step-up and slope handling come from the character controller.
- Leaning over tables or ledges: only the capsule (feet/torso) collides; the head is a separate small sphere used for fade detection. This permits leaning over objects without pushing the body.
- For physics-hand games, make hands physics-driven with a max separation (~0.3–0.5 m, estimate) before snapping back. The capsule-plus-head-sphere model above is the low-cost baseline; a Bonelab-style physics body is a later option.
- **[UNVERIFIED]** Skyrim VR and Fallout 4 VR let the camera pass through walls during roomscale movement, and mods add collision. Not confirmed this session.

### Gaps
- No source was found for Alyx's or Bonelab's exact roomscale capsule or head-collision implementation, or for Skyrim VR's approach.
- No published numbers were found for lean limits or head-sphere radius.

---

## 5. Control schemes: de facto standard and per-game mappings (Touch & Index)

### Takeaway
The de facto shooter layout is: left stick = move (offhand, swappable); right stick X = turn, with right stick up or down used for jump/teleport or crouch/stand; A = jump; B (hold) = menu or inventory; bottom face button on the gun hand = mag eject/reload; grip = grab (hold on Touch, configurable toggle); trigger = fire/use; left stick click = sprint (widely disliked, so offer auto-sprint); menu button = pause. Index adds finger-tracked grab/release and lets players relax their hands without dropping things.

### Cited Findings
**Bonelab (Quest/Touch)**
- A = jump; holding A crouches and releasing gives a higher jump. The right stick up/down sets height continuously ("you aren't limited to full stand or full crouch"), and right stick left/right turns. Holding B opens the backpack, menu and settings (point and press the trigger to select); Y appears to be the left-hand equivalent — [search excerpts from vrpupu review](https://vrpupu.com/en/2026/07/bonelab-meta-quest-review/) and [ZyberVR beginner's guide](https://zybervr.com/blogs/news/bonelab-beginners-guide-complete-starter-walkthrough) (exact attribution between these two pages not confirmed)
- A mod page confirms the A button does the default jump action in BONELAB — [Thunderstore WallGrabJump](https://thunderstore.io/c/bonelab/p/LabEnhanced/WallGrabJump/v/1.1.1/)
- A Vive/trackpad user reports pressing the pad centre to avoid hitting crouch, which breaks jumps — [Steam discussion](https://steamcommunity.com/app/1592190/discussions/0/3370405364920187011)

**Half-Life: Alyx**
- Reload: tap the bottom face button (A/X) on the dominant controller. With a pistol it drops the clip; with the shotgun or SMG it opens the chamber — [search summary of Alyx guides](https://gameplay.tips/guides/7161-half-life-alyx.html)
- On Touch, X/A act as "squeeze" inputs, for example to prime grenades (user report) — [Steam discussion via search](https://steamcommunity.com/app/546560/discussions/0/2143091389822241718)
- Valve tailors input methods per controller type so simpler controllers aren't disadvantaged — [Windows Central](https://windowscentral.com/do-you-need-valve-knuckles-controllers-half-life-alyx)
- Index: core grab, hold, drop and throw combine the trigger with tracked finger positions, which Valve found the most successful method in playtests. Players can relax their hands without dropping the controller, which let testers play longer. Crushing a can is an optional Index-only extra ("fun but not required"). On Vive, Touch and WMR, actions work with a trigger or button press — [UploadVR](https://uploadvr.com/valve-index-alyx-interactions/)
- Touch has coarser finger sensing (index on trigger, thumb raised; the other three fingers share one sensor) — [UploadVR via search](https://uploadvr.com/half-life-alyx-best-headset/)
- Weapon selection by Hand, Head or Hybrid pointing — [SegmentNext](https://segmentnext.com/2020/03/25/half-life-alyx-accessibility-settings-guide)

**Pavlov**
- Index: magazine eject = trackpad down or B on the gun-holding controller; sprint = left thumbstick depressed and forward, with the weapon lowered. The old wiki's Oculus section lists eject as B (right hand on trigger) / Y (left hand) — [Steam guide: Valve Index controls](https://steamcommunity.com/sharedfiles/filedetails/?id=2211883306); [Pavlov wiki Game Controls](http://wiki.pavlov-vr.com/index.php?title=Game_Controls) (via search summary; the page was ~1,400 days old)
- Sprint works when the barrel points at the ground; "Auto Sprint" (delay-triggered sprint) is a setting — [Steam discussion](https://steamcommunity.com/app/555160/discussions/0/1696045708650114585/); [Pavlov Wiki Settings](https://pavlovwiki.com/index.php/Game_Settings)
- Grip settings: Toggle Grip Mode; ini keys `GripMode=Auto`, `GripModeVive=Click`, `GripModeOculus=Hold`, `bAutoUngrip`, `bLegacyGrip`, `SmoothLocomotionMode=Offhand`, `bHeadLocomotion=False`, `PlayerCrouchHeight` (example user ini, not confirmed defaults) — [Pavlov Wiki Settings](https://pavlovwiki.com/index.php/Game_Settings)
- A 2019 thread complains that grip was toggle-only on WMR controllers — [Steam discussion](https://steamcommunity.com/app/555160/discussions/0/1744483505469699330)

**Into the Radius / ITR2**
- Sprint = click the movement stick in, which players call "AWFUL on the Oculus controllers" and ask to rebind; they cite Pavlov's auto-sprint (when the weapon isn't aimed) as better. Users report accidental jump and crouch from the right stick and want dead-zone options — [Steam ITR2 discussion](https://steamcommunity.com/app/2307350/discussions/0/4518883513868880108)

**Titanfall 2 VR**
- Stick flick down (or real crouch) = crouch/slide; left grip plus a 4-direction controller gesture = secondary abilities; offhand trigger = cloak; grenades torn from the chest; secondary weapon on the left hip; manual reloads (eject, insert, rack; gesture reload for some guns like the Wingman); you physically jump into the Titan and trigger its abilities with hand gestures; some abilities are gaze-aimed — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/); [UploadVR](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/); [Road to VR](https://www.roadtovr.com/titanfall-2-vr-mod-circuitlord/)

**HL2VR**
- Left-hand stick click (Sprint) is the "most natural" bind for weapon selection when dual wielding; Toggle Menu should be a long press or chord so it doesn't clash with left-hand Alt Fire; auto-holster on grab release; physical jump toggle — [Mixed News](https://mixed-news.com/en/half-life-2-vr-mod-unleashed/); [Road to VR](https://roadtovr.com/half-life-2-vr-mod-dual-wield-steam/)
- Older Source VR bindings were made for the original Vive wands and were "basically unusable" on Index; later patches added alternate controls — [TechSpot](https://www.techspot.com/news/98222-half-life-2-episode-2-vr-mod-arrives.html)

**Arizona Sunshine 2 / Metro Awakening**
- AS2: no free button reassignment at least at launch — [Mixed News](https://mixed-news.com/en/arizona-sunshine-2-info-guide/). Metro: diegetic UI (check ammo by opening the magazine, wipe the gas mask by hand) — [Road to VR](https://www.roadtovr.com/metro-awakening-screenshots-details-locomotion/)

**Engine default action names**
- XRI binds move and turn to separate "Left Hand Move / Right Hand Move" and "Left/Right Hand Snap Turn" / "Turn" actions, so either hand can host either function — [XRI ContinuousMoveProvider.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Movement/ContinuousMoveProvider.cs); [XRI SnapTurnProvider.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/SnapTurnProvider.cs)
- OVRPlayerController has `RotationEitherThumbstick = false` and `EnableLinearMovement/EnableRotation = true` — [teratail excerpt](https://teratail.com/questions/285432?rss=all)

### Inferences
Proposed default mapping (synthesised from the sources above; right-handed; mirror for left-handed). Quest Touch → Index:
- **Left stick** (Index: left thumbstick): smooth move, or teleport aim in teleport mode. Direction source: head by default, with a hand-oriented option.
- **Left stick click**: sprint, toggled by default (latching until the stick returns to centre or speed drops). Also offer "auto-sprint when weapon lowered or stick held >0.9 for 0.5 s", following Pavlov. The ITR complaint shows click-and-hold sprint is disliked.
- **Right stick X**: snap or smooth turn. **Right stick down** (flick): crouch toggle, or slide while sprinting (Titanfall 2 VR); **right stick hold up/down**: analog height adjust (Bonelab). **Right stick up**: jump in "stick-jump" mode, or teleport arc (Alyx-like) in teleport/hybrid mode. Pull back as a 180° turn conflicts with crouch, so pick one per profile.
- **A** (Index: A): jump. **B**: hold to open the inventory or radial menu (Bonelab); tap for a context action. **X/Y** (left): mag eject or quick-holster on the offhand; Y = menu on the left in single-controller mode.
- **Bottom face button on the gun hand** (A/X when holding a gun): magazine release (Alyx, Pavlov). This overrides jump only while a weapon is held, unless the user picks a separate jump button. **[Estimate]** Many games move jump to the offhand A/X while armed; offer both.
- **Grip**: grab. Default **hold** on Touch and Index (Pavlov `GripModeOculus=Hold`); offer a **toggle** option (Pavlov "Toggle Grip Mode"; WMR and Vive users prefer it). On Index, use grip force/finger curl for release hysteresis so relaxed hands don't drop items (Alyx).
- **Trigger**: fire/use/interact; also a "pinch" on Index.
- **Menu button** (left): pause/system menu. Make non-essential menu toggles a long press (HL2VR advice).
- **Index trackpad**: map to the same actions as the stick click (direction-split: down = mag eject on the gun hand, as in Pavlov Index). Pad touch counts as "thumb rest" for hand poses.
- **Finger tracking (Index)**: grab = trigger + finger curl (Alyx), open hand to release, hand poses and gestures; Touch approximates with grip/trigger/thumb capacitive sensors.
- **Single-controller mode** (Alyx): one hand hosts move (stick), turn (stick-click + X), and a radial menu for the rest.
- Avoid putting critical actions on stick clicks without dead zones and rebinding (ITR complaints). Expose dead-zone settings.

### Gaps
- No official full button maps were found for Alyx, H3VR, Into the Radius, Metro Awakening, Arizona Sunshine 2 or Titanfall 2 VR (Bonelab's comes from third-party guides; Pavlov's Oculus map comes from an old wiki). PCGamingWiki pages were not checked successfully. Fandom wikis were blocked (HTTP 402).
- Not found: what the most popular community SteamVR binding files are, or what Index grip-force thresholds games use.

---

## 6. Movement speeds, acceleration, jump heights

### Takeaway
Meta's guidance is to match real-world rates: walk ~1.4 m/s (3 mph) and run ~2.8 m/s (6 mph). Unity XRI defaults to 1 m/s with 0.5× air control. Meta IWSDK caps slide "max speed" at 4–6 m/s, which is far above the run guidance and should be treated as a ceiling, not a default. Little published data exists for jump heights or acceleration times.

### Cited Findings
- "Set avatar walking and running speeds to match real-world rates (walking ~3 mph/1.4 m/s, running ~6 mph/2.8 m/s)" — [Meta Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/); the same numbers in mph at [Meta User Preferences](https://developers.meta.com/horizon/design/locomotion-user-preferences/)
- Meta IWSDK: "Keep maxSpeed reasonable (4–6 m/s) to minimize discomfort" — [Meta IWSDK slide (search summary)](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/) (conflicts in spirit with the 1.4/2.8 m/s guidance above; likely an upper bound for web demos)
- XRI `ContinuousMoveProvider`: `m_MoveSpeed = 1f` (m/s), `m_InAirControlModifier = 0.5f`, `m_EnableStrafe = true`, `m_EnableFly = false`; gravity is handled by a separate GravityProvider — [XRI ContinuousMoveProvider.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Movement/ContinuousMoveProvider.cs)
- OVRPlayerController: `JumpForce = 0.3f`, `GravityModifier = 0.379f`, `Acceleration = 0.1f`, `Damping = 0.3f`, `BackAndSideDampen = 0.5f` (back and strafe at 50%) — [teratail excerpt](https://teratail.com/questions/285432?rss=all)
- Meta: quantized velocity (stopped / walking / running, switched instantly) is a recommended technique — [Meta Comfort & Usability](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)
- Stride: a full-throttle left stick is "snail's pace"; arm pumping gives faster running — [UploadVR](https://uploadvr.com/stride-review/)

### Inferences
- Recommended defaults (estimate): walk 1.4 m/s (stick magnitude scales 0 to 1.4), sprint 2.8 m/s (user-adjustable up to ~4 m/s for "Advanced"), backward and strafe 0.5–0.75× (per OVR `BackAndSideDampen = 0.5`), crouch-walk ~0.7 m/s. Acceleration to target ≤0.1 s or instant, deceleration instant. In-air control 0.5× (XRI).
- Jump (estimate, not sourced): apex ~0.4–0.6 m for realistic shooters, with gravity at 9.81 m/s² (no "floaty" low gravity, which lengthens airtime and vection). Fire the vignette during airborne time. Titanfall-style double jump and wall-run should be behind an "Intense movement" warning.
- **[UNVERIFIED]** XRI 3.x `JumpProvider` defaults (believed ~1.25 m jump height); the source URL returned 404 this session.

### Gaps
- No sources were found for Alyx continuous speed, Bonelab, Pavlov or ITR walk/sprint speeds, or for comfortable acceleration times in seconds.
- No sourced jump-height comfort data was found.

---

## 7. Consolidated parameter table (concrete numbers found)

### Takeaway
The documented numeric defaults come mostly from engine toolkits (Unity XRI, Meta OVR/IWSDK) and Meta's design guidance. Game-specific numbers are rare, apart from Alyx's 15° snap increments and Pavlov's smooth-turn range.

### Cited Findings
| Parameter | Value | Source |
|---|---|---|
| Snap turn angle default | 45° | [XRI SnapTurnProvider](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/SnapTurnProvider.cs); OVR `RotationRatchet=45` [teratail](https://teratail.com/questions/285432?rss=all) |
| Snap angle example | 30° | [Meta IWSDK turn](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/turn/) |
| Snap angle options | 30/45/90 (Meta); 15° steps to 90° (Alyx) | [Meta Prefs](https://developers.meta.com/horizon/design/locomotion-user-preferences/); [SegmentNext](https://segmentnext.com/2020/03/25/half-life-alyx-accessibility-settings-guide) |
| Snap debounce/cooldown | 0.5 s | [XRI SnapTurnProvider](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/SnapTurnProvider.cs) |
| Turn-around (stick back) | enabled for snap, disabled for smooth (XRI) | XRI Snap/ContinuousTurnProvider |
| Smooth turn speed | 60°/s (XRI), 120°/s (IWSDK example), 45–360 range (Pavlov) | [XRI ContinuousTurn](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/ContinuousTurnProvider.cs); [IWSDK](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/turn/); [Pavlov Wiki](https://pavlovwiki.com/index.php/Game_Settings) |
| Walk / run speed | 1.4 / 2.8 m/s | [Meta Comfort](https://developers.meta.com/horizon/design/locomotion-comfort-usability/) |
| Move speed default | 1.0 m/s; in-air control 0.5 | [XRI ContinuousMove](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Movement/ContinuousMoveProvider.cs) |
| Slide max speed | 4–6 m/s ceiling | [Meta IWSDK slide](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/) |
| Back/side dampen | 0.5 | OVRPlayerController [teratail](https://teratail.com/questions/285432?rss=all) |
| Vignette aperture | 0.7 (0..1, 1 = none) | [XRI TunnelingVignette](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Comfort/TunnelingVignetteController.cs) |
| Vignette feather | 0.2 | same |
| Vignette ease in / out / out-delay | 0.3 s / 0.3 s / 0 s | same |
| Vignette strength default | 0.4–0.6 (0 = off); fade lerp ~10 Hz | [Meta IWSDK slide](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/) |
| Movement/turn angle quantization | 15/30/45/90/180° | [Meta Comfort](https://developers.meta.com/horizon/design/locomotion-comfort-usability/) |
| Pavlov vignette | 0–100% slider | [Pavlov Wiki](https://pavlovwiki.com/index.php/Game_Settings) |
| FOV-restriction study | n=30, significant sickness reduction, no presence loss | [IEEE Spectrum](https://spectrum.ieee.org/dynamic-field-of-view-restriction-makes-virtual-reality-less-barfy) |
| Wall feedback study | 87% refrained with feedback | [unpaywall](https://unpaywall.org/10.1109%2FVR.2018.8446177) |
| OVR jump/gravity | JumpForce 0.3, GravityModifier 0.379 (engine-relative units) | [teratail](https://teratail.com/questions/285432?rss=all) |
| Capsule height | HMD height − ground (no clamp) | [XRI CharacterControllerBodyManipulator](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/CharacterControllerBodyManipulator.cs) |

### Inferences
- Suggested starting config for the runtime (estimates where not in the table): snap 45° (steps 15–90°), snap stick threshold 0.75 / re-arm 0.25, repeat 0.4 s; smooth turn 90°/s (30–360°/s); walk 1.4, sprint 2.8 m/s; accel instant; vignette strength 0.5 → aperture ~0.7 at full speed, feather 0.2, ease 0.3 s, triggers on |v| > 0.1 m/s, |yaw rate| > 15°/s, or any acceleration event; crouch-button height target ~55–65% of calibrated standing height, with a transition of ~0.15–0.25 s ("brief but not instant"); seated offset = (standing calibrated height − seated HMD height), defaulting to the system-reported player height; head-fade sphere 0.1–0.15 m.
- **[Estimate]** Physical crouch detection: treat HMD height < ~0.75 × standing height as "crouched" for gameplay (stealth or hitbox) and < ~0.45 × as "prone"; there is no source for these thresholds.

### Gaps
- No sourced crouch-height thresholds, prone mechanics (except the Bonelab mod's stick-click toggle), or snap-fade durations were found.
- XRI JumpProvider and teleport defaults were not retrieved (404).
