# Pin the Hands, Cheat Everything Else

> A research snapshot (2026-10-21) of what other VR games and frameworks do, with its sources. It
> doesn't describe the framework's code: what was built is in `docs/`.

Great VR player controllers follow one rule. The rendered hand stays exactly where the player's real hand is, and everything else bends in the player's favour: arms, body, held-object physics, magazine insertion, throws and distant pickups. Half-Life: Alyx is the template. Its hands collide with the world but latch onto levers. Throws ignore the inertia sway the player feels. Doors open both ways, gravity gloves replace bending down, and no arms are rendered, because wrong arms are worse than none. Player and critic complaints rank failures the same way across Boneworks, Arizona Sunshine 2, Metro Awakening, Into the Radius and the October 2026 Titanfall 2 VR mod. The worst are sickness from camera motion the player did not cause, grabs and ammo or holster pulls that misfire under time pressure, and hand or weapon poses that don't match the physical controller. All three are game-agnostic, so a Rust runtime that drives flat games can solve them once, below the per-game adapter, and the expensive game-specific work (authored grab poses, physics bodies) is what players miss least. Commercial studios publish almost no tuning constants. The numbers below come from inspectable sources instead: the HIGGS Skyrim mod, FRIK, Final IK's VRIK, the SteamVR Interaction System, Godot XR Tools, Unity XRI, VRExpansionPlugin, H3VR mod assets and the Rust shock2quest port. Treat them as tested starting points, not as Valve's or Stress Level Zero's values. The report ends with a six-tier build list. Comfort and hand truth come first, then interaction reliability, weapon feel and feedback. Physics hands and full-body IK come last, as options the player can turn on rather than defaults.

## Players forgive a missing body but never a hand that lies

The acclaimed titles earn praise for the same things:

- hands that follow 1:1 with believable finger poses
- manual reloads that feel tactile
- a free hand that is always available
- interactions that never send the player looking for a button

Valve built Alyx for simultaneous two-handed play throughout, so the player always needs an easily free hand ([VentureBeat](https://venturebeat.com/business/half-life-alyx-ama-everything-we-learned)). Lone Echo's arm IK was called "as good as I've ever seen in VR" ([Road to VR](https://roadtovr.com/lone-echo-review-oculus-rift/)). Its procedural grip gives every grab a unique finger arrangement that keeps shifting with arm position after the grab ([Road to VR](https://roadtovr.com/lone-echos-virtual-hands-unassuming-vr-innovation/)). State that players can read off the object is the other constant. Alyx pistol magazines show remaining rounds as lights, and "completely empty magazines emit no light" ([Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/)). Fallout 4 VR's Idle Hands mod moves the ammo counter and health bar to the back of the hand ([Nexus](https://www.nexusmods.com/fallout4/mods/42922)). The HL2VR mod rebuilt weapons to be viewable from any angle and added on-weapon HUD elements ([Road to VR](https://roadtovr.com/news-bits-hl2vr-1-4-mod-released-best-half-life-2-experience-just-got-better/)).

Complaints have shifted toward exactly this layer. A University of Alberta study of **750 popular VR games and 17,635 Steam reviews** found that game-specific issues are now the most frequent complaint type, while cybersickness complaints are fading ([UAlberta ASGAARD](https://asgaard.ece.ualberta.ca/an-empirical-study-of-trends-of-popular-virtual-reality-games-and-their-complaints-was-accepted-for-publication-in-the-tg-journal/)). Sickness is still the issue that shuts people out entirely. Road to VR says the Titanfall 2 VR mod is "not going to be for everyone—or probably even *most* people" ([Road to VR](https://roadtovr.com/titanfall-2-vr-mod-circuitlord/)), and Bonelab "will almost certainly make you sick within minutes" ([Android Central](https://www.androidcentral.com/gaming/virtual-reality/bonelab-review-the-new-standard-for-vr-physics-based-games)).

Failed grabs under pressure come next. Into the Radius grab points are "hit and miss" among many grabbables ([Road to VR](https://www.roadtovr.com/?p=109307)). Arizona Sunshine 2's chest pouch pulls magazines back into inventory mid-reload, and players drop guns while reloading ([TheSixthAxis](https://www.thesixthaxis.com/2023/12/06/arizona-sunshine-2-review/)). In Into the Radius 2, a magazine that starts near the well won't seat until the player pulls it away and re-inserts it ([Steam](https://steamcommunity.com/app/2307350/discussions/0/6394634754792839046)).

Pose misalignment is third, and it is runtime-level:

- Early Pavlov rifles sat **about 30° low and right** of the player's hands ([Steam](https://steamcommunity.com/app/555160/discussions/0/154644787622699656)).
- Quest 3 controllers point **about 15° outward** in some games until an OpenXR rotation correction is applied ([Meta forum](https://communityforums.atmeta.com/games-apps-13/quest-3-controllers-are-not-aligned-in-game-131073)).

Physics snagging, jitter at range, and weight or throw tuning trail behind:

- Boneworks arms get "stuck between two pipes" ([Road to VR](https://roadtovr.com/boneworks-physics-driven-mechanics-gameplay-overview/)). Bonelab hands twitch against obstacles ([Mixed](https://mixed-news.com/en/bonelab-review-vr-shooter-hit-or-failed-experiment/)).
- Weapons that lag the hand on purpose, simulated inertia to sell impacts, get called floaty and laggy.

Flat-to-VR conversions show why the hand layer decides whether a conversion feels native. Luke Ross's R.E.A.L. mods are "not designed for motion controls" ([Mixed](https://mixed-news.com/en/kingdom-come-deliverance-ii-vr-mod/)). UEVR works through input translation and does not guarantee grabbing, physical reloads or gestures ([uevr.org](https://uevr.org/?p=193)). Visible hands, recoil, haptics, holsters and manual reloads arrive only through advanced per-game profiles ([PCVR Central](https://pcvrcentral.com/guides/best-uevr-games-2026)), and some profiles ship explicit fixes "to better align weapons with your hands in real life" ([GitHub](https://github.com/Ashok0/SystemShock-UEVR)).

CircuitLord's Titanfall 2 VR mod is praised as feeling "like a native VR game". It remaps in-game prompts to VR controllers instead of LB/RB/L3/R3 and lets cutscenes play on a 2D panel ([UploadVR](https://www.uploadvr.com/titanfall-2s-vr-mod-is-stellar-and-its-installer-is-the-best-thing-about-it/)). It also has fully articulated hands, per-gun manual reloads and physical punching ([Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/)). Its criticisms are just as instructive. Weapon swapping is "cumbersome and clunky" ([UploadVR](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/)), some abilities stay head-aimed, and players asked for a laser sight because there is no crosshair ([ResetEra](https://www.resetera.com/threads/titanfall-2-vr-mod-is-now-available.1644223/)). For a runtime with per-game adapters, the lesson is architectural. The runtime must own hands, weapon poses, prompts and cutscene routing. The adapter only tells it what the game's objects are.

## Three tracked points can carry a body only if the body absorbs every error

A rendered body is optional, and a misbehaving one is actively harmful. Valve's Robin Walker and Brad Kinley explained that correct arms go unnoticed while incorrect arms stand out, and arm lengths vary too much to place arms accurately for everyone ([Game Informer](https://gameinformer.com/interview/2020/03/23/valve-talks-half-life-alyx-and-why-arms-dont-work-in-vr)). Alyx still simulates invisible arms back to the HMD to detect impossible situations, such as a drawer closing over the wrist ([VentureBeat](https://venturebeat.com/business/half-life-alyx-ama-everything-we-learned)). Critics disagreed, and PC Gamer argued "Alyx needs arms" ([PC Gamer](https://www.pcgamer.com/uk/alyx-needs-arms/)).

The research splits the difference. In Parger et al.'s study (N = 55), **IK arms were preferred over hands-only in 67–71% of cases**, but **23% still preferred hands-only** in archery. The authors conclude that "ill-behaved arms are considered a strong disturbance" ([Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)). Lugrin et al. found that showing more body parts in an action game gave no significant gain in body ownership, immersion or perceived control ([Lugrin 2018](https://downloads.hci.informatik.uni-wuerzburg.de/2018-ieeevr-lugrin-igod-preprint.pdf)). Players echo both sides:

- A Boneworks player "hands down" prefers just hands ([Steam](https://steamcommunity.com/app/823500/discussions/0/1740009778259159938/)).
- Players say a full-body IK that moves the hands breaks the 1:1 feel.
- Skyrim VR players say VRIK, HIGGS and PLANCK take the game "from a 7 to a 10" ([Steam](https://steamcommunity.com/app/611670/discussions/0/3763353492968498409)).

The defensible design is a player-selectable body (hands-only, hands plus arms, full body). The hand is pinned 1:1 to the controller, and the body stretches, clamps or fades rather than pulling the hand away.

Shipping solvers fall into three families:

- **Analytic and heuristic IK.** Final IK's VRIK came from *Dead and Buried*. It solves spine, then locomotion, then legs, then arms, and the write-up claims it runs 2–3× faster than FullBodyBipedIK ([Meta dev blog](https://developers.meta.com/horizon/blog/developer-perspectives-character-animation-in-dead-and-buried/)). The GPL-3.0 FRIK mod for Fallout 4 VR (v0.79.1, September 2026) credits its arm and leg solvers to prog's Skyrim VRIK ([FRIK](https://github.com/rollingrock/Fallout-4-VR-Body)).
- **Physics bodies.** Stress Level Zero's physics player grew from **6 rigid bodies in Boneworks to 11 in Bonelab and 18 in Marrow 2** ([Laatsch on X](https://x.com/BrandonJLa/status/2009741997897044046)).
- **Hands only**, as in Alyx.

Engine toolkits (Unreal FBIK, Unity Animation Rigging) supply only generic solvers that take targets. The VR-specific torso, pelvis and elbow heuristics are the real deliverable for a game-agnostic runtime.

### Torso yaw comes from the hands, clamped by the head

If torso yaw followed HMD yaw, the shoulders would swing every time the player looked around. Parger instead derives neck yaw from the **sum of the normalized HMD-to-controller directions**. HMD yaw is used only to break ties when both hands are behind the torso and to cap the head-neck difference at 90° ([Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)). The VRArmIK implementation of Parger detects the hands-behind flip as a frame-to-frame yaw jump of 150–210° and clamps the result to **80° of head yaw** ([VRArmIK](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)).

FRIK adds practical weighting:

- Hand yaw is zeroed when either hand is within about 14 cm of the HMD (my estimate from Bethesda units).
- A hand's weight falls as it rises above the head and as the hands cross.
- The result is clamped to **±50°**, and the body forward is rotated by **0.7×** that angle.

Its own comments admit it "still struggles with hands close to the face and with one hand low and one hand high" ([FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)). The feet then lag with a dead zone. VRIK's root rotates only once head yaw exceeds **25°**. Its Animated mode uses **10° while moving and 90° standing** ([IKSolverVRSpine.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs); [Animated.cs](https://github.com/whx-prog/The-Seed-Link-Future/blob/HEAD/The%20Seed(2022)/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion_Animated.cs)).

Bending over and crouching must be told apart. Parger's chest pitch is θ = ((h0 − h)/h0)·**135.3°** + **0.333**·HMD pitch, where h0 is calibrated standing height and h is current HMD height. FRIK uses a 105.3° base plus 0.1·pitch ([Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf); [FRIK](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)). VRIK pushes the pelvis backward as the head drops (`moveBodyBackWhenCrouching` 0.5) and keeps the head at least **0.8 m** above the root ([IKSolverVRSpine.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs)). A head that drops while looking down therefore bends the spine with the hips moving back. A head that drops with level gaze bends the knees. Parger's geometry fixes the neck-to-head distance at **0.13 m** and shoulder width at **0.31 m** ([Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)).

### Elbows are a swivel guess, and clamps beat cleverness

Every solver uses the analytic law-of-cosines two-bone solve. Quality lives in the swivel: the angle of the elbow around the shoulder-to-hand axis. As Parger puts it, "the difficult part … is finding the direction in which the elbow should be oriented" ([Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)). Parger's fitted model sets the swivel from the hand position in shoulder space. It adds two corrections: blending toward a fixed vector within 0.5 m of the shoulder's vertical axis (the singularity correction), and quadratic corrections once wrist yaw passes 45°. That model reached **4.6–21.7 cm elbow RMSE versus FABRIK's 15.7–34.3 cm** ([Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)). Final IK and FRIK both use hand orientation as the main extra cue: Final IK points the elbow away from the palm and thumb axes, and FRIK maps wrist twist into a pose-dependent window ([IKSolverVRArm.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRArm.cs); [FRIK](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)). Because both depend on hand orientation, per-controller grip-pose calibration matters for the elbow as much as for the gun.

The most valuable constants are clamps:

- **Elbow flexion limits.** FRIK keeps at least **10°** of elbow bend at full reach and stretches the bones slightly, which "stops the elbow spinning when the arm is straight", the exact pose of an aimed weapon. It caps bend at **145°** near the shoulder so the arm shortens instead of flipping.
- **Clavicle reach.** The shoulder moves toward the hand only beyond half arm length, by **0.15 L** (FRIK default) or **0.225 L** (Skyrim VRIK), where L is arm length. Downward motion is damped to 0.4 ([FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)).
- **Smoothing.** FRIK smooths the swivel at 0.75 retention per 90 Hz frame (τ ≈ 39 ms) and hands at 0.6 (τ ≈ 22 ms). Its 2026 release made both frame-rate independent because "the original factors were per frame, so reprojection doubled the lag" ([FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)).

Smooth the swivel, never the hand, and write every filter as α = 1 − e^(−dt/τ).

### Legs need two gaits

VRIK's procedural stepping triggers a step when the predicted centre of mass moves **0.4 m** from the support point, or the foot-to-root angle exceeds **60°**. Feet sit **0.3 m** apart, steps take **0.22–0.33 s**, and a **0.25 m** sphere prevents leg crossing ([IKSolverVRLocomotion.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion.cs)). RootMotion's own tooltip says this mode is "for stationary VR games. Not designed for roomscale and thumbstick locomotion" ([IKSolverVR.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVR.cs)). Artificial locomotion therefore needs a velocity-driven gait. VRIK Animated uses a 0.3 m move threshold, 0.1 s animation smoothing and a 0.5 m maximum root offset ([Animated.cs](https://github.com/whx-prog/The-Seed-Link-Future/blob/HEAD/The%20Seed(2022)/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion_Animated.cs)). FRIK's phase generator starts walking at roughly 0.5 m/s, with step time clamped to 0.28–0.50 s ([FRIK](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)). When the head is too high for the feet to reach the ground, *Dead and Buried* keeps the feet planted and adjusts the spine and hips rather than floating the feet ([Meta dev blog](https://developers.meta.com/horizon/blog/developer-perspectives-character-animation-in-dead-and-buried/)).

### Calibrate twice, render the head twice

Two measurements are enough:

- **Eye height** from a "stand straight" step. VRIK scales the root by (headTarget.y − root.y)/(head.y − root.y) ([VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html)).
- **Arm span** from a T-pose. Parger derives segment length as (span − 0.31 m)/4 ([Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)).

VRChat found height-based measurement worked better for full-body tracking ([VRChat IK 2.0](https://docs.vrchat.com/docs/ik-20-features-and-options)). FRIK is the cautionary case: changing its PlayerHeight rescales the skeleton and breaks hand-weapon alignment ([FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)). Keep the world at 1:1 and scale the avatar. The wearer's own head must disappear without vanishing from shadows and mirrors. VRChat's HeadChop scales the head bone toward zero for the wearer only ([VRChat](https://creators.vrchat.com/avatars/avatar-components/vrc-headchop)). FRIK pushes the head back instead, because hiding it makes the player's shadow headless ([FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)).

## Physics hands earn their cost only where the game cheats for the player

Valve's Jason Mitchell gave the canonical reason to hold back on physics. When the player holds something large and rigid, "there's a lot that's gonna happen in a physical simulation… that's not gonna feed back into your hand" ([UploadVR](https://www.uploadvr.com/half-life-alyx-boneworks-physics/)). Alyx's hands collide with walls, doors and tables but latch onto levers, knobs and cranks until release, a technique credited to Job Simulator. The stated lesson: "we have to know when to show where the players real hands are, and when to show where their hands should be" ([Transform Interactive](https://transforminteractive.com/what-half-life-alyx-taught-us/)). Held Alyx weapons don't clip and can push doors and swat objects ([Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2/)). Doors open both ways because players can't feel whether to push or pull ([Engadget](https://www.engadget.com/2020-03-31-half-life-alyx-valve-interview-vr.html)).

Boneworks took the opposite bet: "If something looks heavy, it is" ([UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)). Its designer admits that "eventually playing just gets way too tiring for long sessions" ([GamesBeat](https://gamesbeat.com/how-boneworks-turns-your-body-into-its-key-mechanic/)). A 2025 study of attachment, penetration and torque grab methods found attachment best suited to commercial apps ([arXiv](https://arxiv.org/abs/2504.00337v1)). An industry patent describes the common hybrid: a kinematic ghost hand plus a simulated visible hand, with the ghost shown when the visible hand is obstructed ([US 10657696](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/10657696)). A game-agnostic injector rarely has authored grab poses. For it, a physical proxy that collides with the world, combined with generous cheats, beats Boneworks-grade simulation.

### A dynamic hand on a leash

There are three ways to make the hand follow the controller:

- **Kinematic copy.** The hand is set to the controller pose every frame. It has no collision response and can't be stopped by anything.
- **Velocity tracking.** Each physics step sets the hand's linear and angular velocity to close the gap to the controller.
- **PD drive.** A spring-damper joint or force with a force cap.

LÖVR's example explains why the hand should be dynamic. A kinematic hand can't be pushed back by contacts, so squeezing an object between two hands can't resolve. LÖVR drives a 0.04 × 0.08 × 0.08 m, 0.5 kg box with force gain 30,000 and torque gain 20 ([LÖVR](https://lovr.org/docs/v0.19.0/Physics/Hand_Physics)). HurricaneVR's PD tuning recipe is spring first, then damper (with "Spring value by 10" as the starting damper), then max force last ([HVR](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/pdstrength.md)). PD gains must be retuned per hand mass ([Amebous Labs](https://amebouslabs.com/developing-physics-based-vr-hands-in-unity/)), which makes velocity tracking the simpler choice for a Rapier runtime.

Every robust framework adds a leash. The hand snaps back once separation passes **0.7 (HIGGS) or 1.0 m (Godot XR Tools)** ([HIGGS config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/hands/collision_hand.gd)). HurricaneVR's variant drops the held object and disables hand collision while the hand "smoothly returns to the controller" ([HVR](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/physicshands.md)).

Held objects need their own rules:

- **Constraint strength.** HIGGS holds objects with a constraint (tau 0.03, damping 0.8, max force 2,000, or 9,000 for weapons), capped at **500 × object mass**. Stiffness drops to **0.01 tau while the held object is in contact**, so objects rest against walls instead of fighting them ([HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)).
- **Hand collision.** HIGGS disables hand collision with the held object, and after a throw it ignores hand contact for **0.1 s** ([hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)).
- **Collision copying.** Godot freezes the held body and copies its collision shapes onto the colliding hand, so hand plus gun is what the wall stops ([Godot pickable](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/objects/pickable.gd)).
- **Weight.** Godot conveys weight by limiting hand displacement per step to force·dt/mass, with force rising from **15 N to 300 N** as the controller leads by up to 1 m ([Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/hands/collision_hand.gd)).
- **Don't drop on collision.** Schell Games' early design dropped held objects past a collision threshold, and it "made object interaction frustrating" as levels grew ([Schell Games](https://legacymicrosites.schellgames.com/blog/engineering-a-super-spy-in-i-expect-you-to-die)).

### Selection needs a radius, a cone, hysteresis and leeway

Grab selection works in two layers:

- **A near query.** Godot XR Tools checks a **0.3 m** sphere, then a **5 m, 5°** ranged cone ([Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd)). HIGGS uses a tighter **0.05 m-radius cast over 0.1 m**, plus a 0.3 m-radius far cast out to 5 m, gated by a 50° cone around HMD forward ([HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)).
- **Hysteresis.** Meta recommends a narrow cone to hover and a wider one to unhover ([Meta](https://developers.meta.com/horizon/design/grab_usage/)). Godot's grip registers a press at threshold + 0.1 and a release at threshold − 0.1 ([Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd)).

HIGGS's **input leeway** matters most for a flat-game injector. When the trigger is pressed on a selected object, the vanilla game input is withheld for **0.3 s**. If no grab happens, the press is re-injected for 0.03 s, so a single button serves both VR grab and game action ([HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)).

There are two grab styles:

- **Grab anywhere.** HIGGS grabs at the actual contact point and curls each finger by raycasting against nearby triangles, with a curl floor of 0.2. It lerps objects into the hand over 0.1–0.2 s depending on distance ([hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)). Boneworks lets lifting the index finger loosen the grip, so the hand can slide along a handle without letting go ([UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)).
- **Authored grab points.** XRI uses attach transforms, including a secondary attach for two-handed holds, and eases the attach over **0.15 s** by default ([XRGrabInteractable.cs](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactables/XRGrabInteractable.cs)).

For two-handed items, HIGGS aligns the weapon to the palm-to-palm axis (factor 1.0), shifts it halfway along that axis, and blends half the off-hand twist ([hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)).

Finger posing has to work with coarse input:

- **Index** controllers have capacitive sensing for the middle, ring and pinky fingers and a trigger sensor for the index finger ([VRChat](https://docs.vrchat.com/docs/valve-index)). Their grip force sensor runs from a light touch to a firm squeeze ([Valve](https://www.valvesoftware.com/en/index/controllers)).
- **Touch** controllers have capacitive contact plus "near touch" on the thumbrest ([iFixit](https://www.ifixit.com/News/oculus-touch-teardown)). NearTouch read as zero under some OpenXR-backed SDK versions ([Meta forum](https://communityforums.atmeta.com/unity-development-123/ovrinput-neartouch-not-working-with-ovrplugin-with-openxr-backend-111266)).
- **Core OpenXR** did not supply Index per-finger curl through hand tracking ([Khronos forum](https://community.khronos.org/t/how-is-a-limited-finger-tracking-controller-like-index-controller-supposed-to-work/108265)).

On Index, Alyx combines the trigger with tracked finger positions for grab, hold and throw. Players can relax their hands without dropping things, which let testers play longer ([UploadVR](https://uploadvr.com/valve-index-alyx-interactions/)).

### Throws fail on velocity estimation, not on physics

The published throw estimators differ in detail but agree on the principle:

| Source | Method | Scaling |
|---|---|---|
| SteamVR Interaction System | Tracking runtime's own controller velocity, sampled **11 ms in the past** ([Throwable.cs](https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/Throwable.cs)). Its fallback estimator averages 5 frames linear and 11 frames angular ([VelocityEstimator.cs](https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/VelocityEstimator.cs)) | ×1.1 linear |
| HIGGS | Peak-speed frame in a recent history, averaged with its two neighbours. Adds player locomotion velocity and ω × r for off-centre objects, capped at 5 m/s | Angular ×0.6 ([hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)) |
| Unity XRI | **0.25 s** smoothing window | ×1.5 linear ([XRGrabInteractable.cs](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactables/XRGrabInteractable.cs)) |
| shock2quest | **50 ms** smoothing window ([dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs)) | — |

Alyx ignores inertia sway "only in its throwing calculation", so heavy objects still throw accurately ([Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/)). The failure modes follow from these mechanisms:

- reading the velocity of a lagging physics body instead of the controller
- sampling only the release frame, when the wrist is already decelerating
- averaging so long that the wind-up is included
- ignoring the lever arm or the player's own movement
- passing raw angular velocity, which gives wild spin

Two more hazards are subtler. Velocity can point the wrong way for a frame when a controller re-enters the tracking field of view ([Meta forum](https://communityforums.atmeta.com/discussions/dev-general/velocity-estimation-is-working-worse-than-it-was-when-entering-the-tracking-fov/1241171)). Meta's Interaction SDK fits recent timed poses with RANSAC for robustness ([Meta](https://developers.meta.com/horizon/reference/interaction/v72/class_oculus_interaction_throw_r_a_n_s_a_c_velocity)). The Titanfall 2 VR reviewer kept "accidentally lobbing a grenade six feet shorter than you intended", the signature of an under-scaled throw ([Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)).

### Gravity gloves are a four-stage gesture, not a button

Valve's Kerry Davis framed movement as "one of the highest friction areas of the experience". He called buttons the "least VR thing you can do". Objects arrive "in a smooth arc so they're easy to catch", and the game "anticipates" when the player is reaching for ammo ([Engadget](https://www.engadget.com/2020-03-31-half-life-alyx-valve-interview-vr.html)). An early separate toggle button made playtesters use the gloves less, so Valve dropped it. Only objects "light enough to be carried in one hand" can be pulled ([Combine OverWiki](https://combineoverwiki.net/wiki/Gravity_Gloves)). The stages run:

1. Selection: the targeted object glows.
2. Lock-on: pressing the trigger fixes the selection, "even if you move your hand now".
3. Flick: a wrist yank toward yourself launches the object.
4. Catch: the trajectory adapts in real time so it reaches the hand without feeling like an auto-snap.

Each stage has its own sound ([Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/)). The common player failure is pulling too gently: "You really have to flick that thing" ([Steam](https://steamcommunity.com/app/546560/discussions/0/1861616237339229551)).

HIGGS's open reimplementation supplies numbers. These are a community mod's values, not Valve's:

- The selection locks if the trigger is pressed within **0.3 s**.
- A pull fires when hand speed toward the player exceeds **1.2 m/s**.
- Flight time is **0.715619 − 0.415619·e^(−0.656256·d)** seconds for distance d in metres. That works out to 0.30 s at the hand, about 0.50 s at 1 m and about 0.70 s at 5 m, so catch timing stays predictable at range.
- The target re-aims at the palm for 0.1 s, and the velocity is re-solved ballistically for 0.2 s.
- Angular damping is set to 8 during flight.
- A widened **0.9 m** catch radius catches the arriving object ([hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)).

Climbing follows a similar kinematic pattern. Godot XR Tools moves the body by the negative of the gripping hand's controller-to-grab-point offset, with gravity off. It drops the hold at 1.0 m drift and flings with a 5-sample average velocity plus a forward push ([Godot climb](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/movement_climb.gd)).

## Reloads delight when they cut repetition rather than physicality

The best-regarded reloads keep the physical sequence (eject, fetch, insert, rack) but remove fumbling and repetition. Alyx models chamber state independently of the magazine. Removing a magazine leaves a round chambered. Firing the last round locks the slide open, "so it's ready to receive a new bullet". Distinct sounds mark "mag empty, last round chambered" and "fired last round, chamber open" ([Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2)). Its shotgun pulls **two shells per reach**, so "a full reload happens in four gestures instead of seven", and a unique sound marks the last shell so players don't waste extras ([Road to VR](https://www.roadtovr.com/inside-xr-design-best-shotguns-in-vr-what-makes-them-great-feedback/2/)).

The complaints target complexity, not manual reloading itself:

- A Bonelab player: "we don't need 15 jitsu moves to reload a gun" ([Steam](https://steamcommunity.com/app/1592190/discussions/0/3395175706735256154/?ctp=2)).
- Arizona Sunshine 2's reloads felt "like playing Twister with just your fingers" ([TheSixthAxis](https://www.thesixthaxis.com/2023/12/06/arizona-sunshine-2-review/)).
- Metro Awakening sometimes read an ammo grab as an attempt to cock the gun ([Mixed](https://mixed-news.com/en/metro-awakening-review/)). It was faulted for having no auto-reload against "a seemingly endless mob" ([GamingTrend](https://gamingtrend.com/reviews/metro-awakening-review-the-metro-series-arrives-in-vr-2/)).

Every well-regarded game or mod ships an escape hatch:

- H3VR's "Use Easy Mag Loading" seats magazines that are "close enough" to the well, and its alternate magazine poses reduce controller bumping ([H3VR wiki](https://h3vr.fandom.com/wiki/Options_Panel)).
- HL2VR's Quick Reload is "drop the weapon to your waist, aim at the ground, and wait", and it recovers unspent ammo ([HL2VR manual](https://halflife2vr.com/manual/)).
- Arizona Sunshine 2 reloads when the gun is slammed into the waist holster ([PSNProfiles](https://psnprofiles.com/guide/19860-arizona-sunshine-2-trophy-guide)).
- Hard Bullet has an arcade mode with automatic reloads ([UploadVR](https://www.uploadvr.com/vr-action-shooter-hard-bullet-finds-a-new-home-on-quest-3/)).

Open frameworks bound the snap geometry. XRI's socket snaps at **0.10 m** and waits 1 s before re-accepting an object, unless hover snapping is enabled ([XRSocketInteractor.cs](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactors/XRSocketInteractor.cs)). Auto Hand's slider ignores the first 10% of travel and fires its end events at ±0.9 ([Auto Hand](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/extras/physics-gadgets/physics-gadget-slider)). No commercial game publishes its magwell radius or alignment tolerance. The strict tier (3–5 cm, ±25°) and default easy tier (8–10 cm, ±45°) in the build list are my synthesis.

### One ammo zone that already knows your gun

The dominant pattern is a single grab zone that returns a magazine for the gun in the other hand, drawn from an abstract counted pool:

- **Alyx:** an empty hand reaching over the shoulder gets a magazine for the main-hand gun, or pistol ammo when nothing is held ([gameplay.tips](https://gameplay.tips/guides/7161-half-life-alyx.html)).
- **Boneworks/Bonelab:** the ammo belt "will also detect which weapon is in your hands" ([TV Tropes](https://tvtropes.org/pmwiki/pmwiki.php/VideoGame/Boneworks)).
- **Arizona Sunshine 2** uses the chest, **Pavlov** a hip pouch, and **HL2VR** the shoulder. In HL2VR, a caught magazine returns to the pool ([HL2VR manual](https://halflife2vr.com/manual/)).
- **H3VR** makes infinite ammo a per-slot "spawn-lock" toggle ([H3VR wiki](https://h3vr.fandom.com/wiki/Spawn-Locking)).

Simulation games use physical magazines in physical slots, and players then complain about slot counts. Into the Radius's four vest pouches were called too few ([Steam](https://steamcommunity.com/app/1012790/discussions/5/3823032780423413079)).

The only concrete published geometry for an over-the-shoulder backpack comes from the open-source Rust port shock2quest:

- two zones centred at head − 0.06 m forward − 0.10 m up, ± 0.24 m right
- radius **0.25 m**
- a gate requiring the hand to be less than 0.08 m forward of the head and more than 0.10 m to the side, which keeps face and forward gestures out ([shoulder_backpack.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/mission/shoulder_backpack.rs))

### Holsters fail when the estimated hips drift

Without hip tracking, slots on estimated hips move back a few inches while the player moves, and turning the head twists the estimated hips enough to block sheathing. shock2quest fixes this:

- body yaw follows head yaw with a **0.5 s exponential time constant**, ignoring head pitch and roll
- body yaw **freezes while a hand is inside a zone**, so the zone can't slide away mid-reach
- holster reach radius **0.15 m**
- belt **0.55 m below the eyes**
- a 0.15 s exit hysteresis before zone feedback resets ([body_inventory.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/mission/body_inventory.rs); [dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs))

Hard Bullet's developer told struggling players to adjust holster offsets in settings ([Steam](https://steamcommunity.com/app/1294760/discussions/0/6741412150958521826)), so expose per-slot offsets.

Bigger, fewer, visible slots win:

- Bonelab's underarm pockets "seem useless now that most things can't fit in them" ([Steam](https://steamcommunity.com/app/1592190/discussions/0/3594464430443109431/)).
- Metro's invisible holstered guns drew complaints ([Steam](https://steamcommunity.com/app/2669410/discussions/0/4631482917606969203)).
- Boneworks was praised for showing open slots on the body ([Road to VR](https://roadtovr.com/boneworks-physics-driven-mechanics-gameplay-overview/)).

H3VR adds two reliability features. "Harnessing" returns a gun to its slot when released ([H3VR wiki](https://h3vr.fandom.com/wiki/Quickbelt)). A grip-drop guard lets the player choose instant drop, a 1 s delay, or double-click ([H3VR wiki](https://h3vr.fandom.com/wiki/Options_Panel)).

Weapon switching is where Titanfall 2 VR stumbled. Its reviewer wanted "hip switching or same-hand swapping like Half-Life: Alyx", a stick click plus a direction ([Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)). HL2VR's hex-grid selector "is controlled by your hand's position; it is not a pointer" ([HL2VR manual](https://halflife2vr.com/manual/)). The HL2VRU fork's Grip-Holster Mode holsters the weapon on grip release and re-equips it on grip hold, which simplifies dual-wield reloading "a lot" ([HL2VRU](https://github.com/vittorioromeo/HL2VRU)).

### Two hands steer, the stock anchors, recoil never moves the hand

VRExpansionPlugin's open gun code is the clearest reference for two-handed aiming. The gun rotates about the primary grip by the change in direction from the primary hand to the support hand. The support hand passes through a One-Euro filter: global defaults MinCutoff 0.1, CutoffSlope 10, DeltaCutoff 10, with a stock-hand filter of 5/20. The virtual stock pivots at the HMD plus a shoulder offset. It engages at **35 cm** and reaches full influence **20 cm** inside that edge ([GS_GunTools.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_GunTools.h); [VRGlobalSettings.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/VRGlobalSettings.cpp)). Players split on the virtual stock, from "amazing for sniper weapons" to "gets in the way of aiming" ([Steam](https://steamcommunity.com/app/555160/discussions/0/3047235309986395024)). Pavlov players asked for "VStock for sniper rifles ONLY" ([Pavlov upvote](https://pavlovvr.featureupvote.com/suggestions/232993/virtual-stock-vstock-for-snipers-only)), so the toggle belongs per weapon class. Jitter at range is also fixed with hold-breath buttons and filters that back off during fast motion ([Sniper Elite VR Steam](https://steamcommunity.com/app/752480/discussions/0/3044979493857958046); [UE forum](https://forums.unrealengine.com/t/metaxis-games-smooth-vr-hand-motion-controller-jitter-filter-multiplayer-sync/2732702)).

Recoil works best as a clamped, additive offset on the weapon pose. VRE lerps the applied offset toward its target at **30/s** and decays the target at **20/s** ([GS_GunTools.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/GripScripts/GS_GunTools.cpp)). H3VR recoil profiles from mod assets show the hold-state structure. A .45 pistol kicks 35 vertical per shot (max 60, recovery 20), while the bipodded profile kicks only 3 (max 10), about a **10× reduction**. Separate stabilisation factors apply for no support, two hands and a foregrip ([LSIIC](https://github.com/BlockBuilder57/LSIIC/tree/HEAD/Unity/Assets/LSIIC/ScriptableObjects/RecoilProfile)). In HL2VR, two-handing "significantly" reduces SMG and pulse-rifle recoil, which is the intended way to fire them ([HL2VR manual](https://halflife2vr.com/manual/)). Apply the offset to the rendered and aimed weapon, never to the tracked hand, so the hand stays where the player put it.

Scopes render a zoomed camera to a texture on the lens, with eyebox shadow and fake parallax ([Godot Shaders](https://godotshaders.com/shader/3d-picture-in-picture-scope-with-eyebox-reticle-scope-shadow-and-fake-parallax/)). Titanfall 2 VR's scope shadow "makes aiming harder but rewards a realistic stance" ([Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)). HL2VR offers a laser pointer and says it "doesn't count as a cheat" ([HL2VR manual](https://halflife2vr.com/manual/)).

Melee is velocity-gated contact damage. HL2VR's crowbar requires a minimum swing speed and distance plus a short cooldown ([HL2VR manual](https://halflife2vr.com/manual/)). shock2quest damages above a 2.0 world-unit/s closing speed and halves one-handed damage ([dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs)). VRE requires stabs to align within 0.1 of a perfect forward-to-normal dot product ([GS_Melee.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_Melee.h)). PLANCK registers Skyrim hits only on real ragdoll contact at true weapon size. Vanilla Skyrim scales the weapon to 1.75× for hit detection ([TheGamer](https://www.thegamer.com/skyrim-vr-slap-npcs-planck-mod/)).

## Comfort is a stack of independent switches, and the camera belongs to the player

Alyx defined the locomotion menu that every later shooter copies ([SegmentNext](https://segmentnext.com/2020/03/25/half-life-alyx-accessibility-settings-guide)):

- **Blink:** teleport with a fade.
- **Shift:** a rapid animated zoom to the target.
- **Continuous:** stick movement, with head-oriented forward.
- **Continuous Hand:** stick movement, with the aiming hand setting forward.

Valve validates teleports against "a standard minimum body size", because tall testers kept hitting low pipes ([PC Gamer](https://www.pcgamer.com/uk/half-life-alyxs-movement-system-was-changed-because-tall-testers-kept-getting-stuck/)). Robin Walker concluded there isn't "that significant a difference at all" between teleport and free movement as experiences ([UploadVR](https://uploadvr.com/half-life-alyx-teleporting/)). Metro Awakening packages the menu as presets. Comfort is teleport-focused. Immersion is stick movement with snap turn, and it is the only preset with forced camera moves such as knock-downs ([Road to VR](https://www.roadtovr.com/metro-awakening-screenshots-details-locomotion/)).

Head versus hand orientation has no universal answer. A small thesis (n = 15) rated controller-oriented movement more natural with no sickness difference ([nweon summary](https://paper.nweon.com/?p=6638)), while Meta says the user should choose ([Meta](https://developers.meta.com/horizon/design/locomotion-user-preferences/)). Pavlov exposes Head, Dominant Hand or Offhand as the input vector ([Pavlov wiki](https://pavlovwiki.com/index.php/Game_Settings)).

### Snap at 45 degrees around the head, vignette in degrees of visual angle

Snap-turn defaults and options cluster tightly:

- **Defaults:** XRI snaps **45°** with a **0.5 s** debounce and a stick-back 180° turn-around ([XRI SnapTurnProvider](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/SnapTurnProvider.cs)).
- **Options:** Meta recommends offering 30, 45 or 90° ([Meta](https://developers.meta.com/horizon/design/locomotion-user-preferences/)). Alyx offers 15° increments up to 90° ([SegmentNext](https://segmentnext.com/2020/03/25/half-life-alyx-accessibility-settings-guide)).

Smooth turn should be opt-in ([Meta](https://developers.meta.com/horizon/design/locomotion-best-practices/)). Its speed ranges from XRI's 60°/s ([XRI](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Turning/ContinuousTurnProvider.cs)) through Meta's 120°/s example ([IWSDK](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/turn/)), with Pavlov allowing 45–360 ([Pavlov wiki](https://pavlovwiki.com/index.php/Game_Settings)). Rotate the rig about the HMD's floor projection, not the playspace origin. XRI evaluates the body position as the camera projected onto the origin's floor plane ([XRBodyTransformer.cs](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/XRBodyTransformer.cs)). Never add a second yaw transform to the camera ([IWSDK](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/turn/)).

Vignettes have research and defaults behind them. Fernandes and Feiner's dynamic field-of-view restriction study (n = 30) significantly reduced sickness without lowering presence, and most participants didn't notice the restriction ([IEEE Spectrum](https://spectrum.ieee.org/dynamic-field-of-view-restriction-makes-virtual-reality-less-barfy)). XRI's vignette defaults to **aperture 0.7** (1 = no vignette), **feather 0.2** and **0.3 s** ease in and out ([XRI TunnelingVignetteController](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Comfort/TunnelingVignetteController.cs)). Meta's Interaction SDK triggers on rotation speed, linear speed and acceleration, and defines the vignette directly in degrees, so it behaves the same across headsets ([Meta ISDK](https://developers.meta.com/horizon/documentation/unity/unity-isdk-locomotion-comfort/)). Meta's IWSDK suggests a 0.4–0.6 default strength ([IWSDK slide](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/)).

Speeds should match the real world: **walk 1.4 m/s, run 2.8 m/s**. The vestibular system responds to changes in velocity, not to constant velocity, so Meta recommends quantized speeds switched instantly ([Meta](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)). Meta's IWSDK puts the ceiling at 4–6 m/s ([IWSDK slide](https://developers.meta.com/horizon/documentation/iwsdk/concepts/locomotion/slide/)). XRI defaults to 1 m/s with 0.5× in-air control ([XRI ContinuousMoveProvider](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/Movement/ContinuousMoveProvider.cs)). OVRPlayerController halves backward and strafe input ([teratail](https://teratail.com/questions/285432?rss=all)).

Roomscale needs the collision capsule to follow the head. XRI sets capsule height to camera height minus ground height and moves the origin through a character controller, so walls stop the rig rather than the camera ([XRI](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/Locomotion/CharacterControllerBodyManipulator.cs)). For head-in-wall cases, Meta recommends "soft collisions" that slow before stopping ([Meta](https://developers.meta.com/horizon/design/locomotion-comfort-usability/)). Developers mostly fade to black, since pushing the body back felt "harsh and disruptive even when smoothed" ([Meta forum](https://communityforums.atmeta.com/discussions/dev-unity/looking-throug-walls-with-position-tracking/177837)). With non-intrusive feedback, **87%** of participants refrained from walking through virtual walls ([You Shall Not Pass](https://unpaywall.org/10.1109%2FVR.2018.8446177)).

Accessibility options are part of the comfort stack. Alyx's reference set includes Dominant Hand, Single Controller mode, Height Adjust (crouch only, stand only or both) and Seated Mode ([Road to VR](https://roadtovr.com/half-life-alyx-accessibility-options-subtitles-seated-single-controller/)). Meta's seated height offset pushes the virtual floor down so standing-height interactables stay reachable ([Meta ISDK](https://developers.meta.com/horizon/documentation/unity/unity-isdk-locomotion-comfort/)).

### A control map both Touch and Index users already know

No studio publishes a full button map. The table below is my synthesis of the mappings documented in Alyx, Bonelab, Pavlov, HL2VR, Titanfall 2 VR and Into the Radius for a right-handed player; mirror it for left-handed players. The strongest negative signal is sprint on a held stick click. Into the Radius 2 players call it "AWFUL on the Oculus controllers", cite Pavlov's auto-sprint as better, and want dead-zone options after accidental jumps and crouches ([Steam](https://steamcommunity.com/app/2307350/discussions/0/4518883513868880108)).

| Action | Quest Touch | Valve Index | Evidence |
|---|---|---|---|
| Move | Left stick (head-oriented; hand option) | Left thumbstick | Alyx, Pavlov ([Pavlov wiki](https://pavlovwiki.com/index.php/Game_Settings)) |
| Turn | Right stick X, snap 45° | Right thumbstick X | XRI, Meta |
| Sprint | Left stick click, latching toggle; auto-sprint option | Same | ITR2 complaint; Pavlov Auto Sprint ([Pavlov wiki](https://pavlovwiki.com/index.php/Game_Settings)) |
| Crouch / slide | Right stick flick down (toggle); real crouch always works | Same | Titanfall 2 VR ([Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/)) |
| Analog height | Right stick hold up/down | Same | Bonelab ([vrpupu](https://vrpupu.com/en/2026/07/bonelab-meta-quest-review/)) |
| Grab | Grip, hold default, toggle option | Grip force plus finger curl with release hysteresis | Pavlov `GripModeOculus=Hold`; Alyx Index ([UploadVR](https://uploadvr.com/valve-index-alyx-interactions/)) |
| Fire / use | Trigger | Trigger | Universal |
| Magazine release | Bottom face button on the gun hand (A) | A or trackpad down on the gun hand | Alyx ([gameplay.tips](https://gameplay.tips/guides/7161-half-life-alyx.html)); Pavlov Index ([Steam guide](https://steamcommunity.com/sharedfiles/filedetails/?id=2211883306)) |
| Jump | Offhand bottom face button (X), or right stick up in stick-jump profiles | Same | Bonelab uses A when unarmed ([Thunderstore](https://thunderstore.io/c/bonelab/p/LabEnhanced/WallGrabJump/v/1.1.1/)); my conflict resolution |
| Weapon select | Right stick click hold, hand-position wheel | Same | Alyx, HL2VR ([HL2VR manual](https://halflife2vr.com/manual/)) |
| Inventory / radial | Hold B | Hold B | Bonelab ([vrpupu](https://vrpupu.com/en/2026/07/bonelab-meta-quest-review/)) |
| Pause | Left menu button; non-essential menus on long press | Same | HL2VR advice ([Mixed](https://mixed-news.com/en/half-life-2-vr-mod-unleashed/)) |
| Abilities | Grip plus a 4-direction gesture | Same | Titanfall 2 VR; "takes a bit of getting used to" ([Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/)) |

## Haptics and sound must land as a single event

Meta's guidance is the most complete published haptics doctrine:

- Haptics should reinforce "a clear causal relationship" with the action.
- They should be adjustable and mutable, and softer sounds should get softer haptics.
- Avoid "long, overlapping effects, especially in fast-paced action games".
- Haptics, audio and visuals should be perceived as "one event" ([Meta](https://developers.meta.com/horizon/documentation/unity/unity-haptics-design-guidelines/)).

Hardware differs. **Quest 2 has a narrowband actuator (amplitude only), while Quest Pro and Quest 3 have wideband actuators with frequency control** ([Meta](https://developers.meta.com/horizon/documentation/unity/unity-haptics-design-guidelines/)). Meta's system pack gives the only explicit timings. Success is 2 events over 175 ms ramping up, Warning is 2 events over 150 ms ramping down, and Error is 4 abrupt events. Release "mirrors Grab at lower amplitude and frequency", and object hover is short rather than continuous "so it doesn't interfere with the grab" ([Meta haptics system](https://developers.meta.com/horizon/design/haptics-system/)).

The APIs set the constraints:

- OpenXR takes amplitude 0–1, duration in nanoseconds, and `XR_MIN_HAPTIC_DURATION` for the shortest pulse the device supports. Runtimes may clamp duration and frequency ([Khronos](https://registry.khronos.org/OpenXR/specs/1.1/man/html/XrHapticVibration.html)).
- On Quest the simple API ignores frequency ([Meta](https://developers.meta.com/vr/documentation/native/android/mobile-openxr-haptic/)).
- Legacy OpenVR forbids re-triggering a pulse within **5 ms** on the same controller and axis ([OpenVR](https://github.com/ValveSoftware/openvr/wiki/IVRSystem::TriggerHapticPulse)).

So encode effects as amplitude envelopes first and treat frequency as an optional enhancement. HIGGS's collision haptics give a physics-driven formula: no pulse below **0.2 m/s**, strength **0.1 + 0.05·speed + 0.03·mass^0.6**, duration 10 ms, ×0.4 for damped collisions ([HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)). Reviewers almost never mention haptics in the case-study games, and no studio has published per-event tables. The evidence here is platform doctrine, not player testing.

Valve treated audio as core to presence:

- **Grab sounds.** Sound designer Roland Shaw: "There are only a few hand grab sounds — a deliberate decision to provide clear familiar feedback."
- **Impacts.** Velocity-driven layering produces "many thousands of possible variations".
- **Gravity gloves.** Sound designer Dave Feise layered a hand loop, a rollover sound on the target, travelling loops, and separate pings on hand close and on the flick.
- **Gunshots.** Each shot is four layers (Head, Body, LFE, Tail) with 3–5 random variations per layer. Head and Body are spatialised through HRTF; LFE and Tail skip it for spread.
- **Close-up sounds.** These were reworked because VR players inspect objects at ear distance ([A Sound Effect](https://asoundeffect.com/half-life-alyx-vr-sound/)).

Lone Echo's "ever-present whir of tiny servos" is cited as part of its best-in-class hand presence ([Shacknews](https://shacknews.com/article/97189/lone-echo-impressions-from-oculus-connect-3)). A flat game has no grab, holster or magazine-seat sounds of its own. The runtime should ship a small, consistent confirmation library and fire each sound on the same frame as its haptic.

## A prioritised build list for a Rust and Rapier runtime

The tiers are ordered by how severe each problem is in player sentiment and by how game-agnostic it is. Tiers 0–1 fix the three worst complaint classes once for every game. Tiers 2–3 make adapted games feel authored. Tiers 4–5 are optional fidelity layers that players should be able to switch on. In the Basis column, a named source means that framework, mod or game ships or documents the value. "Est." marks my synthesis where no source publishes a number. Tune every value in playtests.

Two Rapier mappings apply throughout:

- **Filters.** Run physics at a fixed step and write every smoothing constant as α = 1 − e^(−dt/τ), with dt clamped to about three nominal frames (FRIK's fix).
- **Bodies and queries:**
  - Hands and held objects are dynamic rigid bodies with CCD.
  - Hand-object pairs are excluded through interaction groups or a contact-filter physics hook.
  - Held objects use a generic 6-DOF joint with motors and max-force limits.
  - The player capsule uses Rapier's kinematic character controller (move-and-slide, autostep, snap-to-ground).
  - Grab and holster tests are shape-intersection queries.

Verify the API names against the Rapier version you pin. (When this was written the repository didn't depend on Rapier yet; the runtime's rig and the test range now run on it.)

**Tier 0: camera ownership, comfort and hand truth**

| Mechanic | Values | Basis |
|---|---|---|
| Camera never moves without player input; game cutscenes and forced cameras go to a head-locked 2D panel; full-VR cutscenes are opt-in behind a warning | 2D panel default | Titanfall 2 VR, Metro |
| Snap turn about the HMD floor projection | 45° default; 15–90° in 15° steps; stick-back 180° | XRI, Meta, Alyx |
| Snap stick thresholds | fire at 0.75, re-arm below 0.25, repeat every 0.5 s while held | XRI 0.5 s debounce; thresholds est. |
| Smooth turn (opt-in) | 90°/s default, 30–360°/s slider | XRI 60, IWSDK 120, Pavlov 45–360; default est. |
| Move speeds | walk 1.4 m/s, sprint 2.8 m/s (up to 4 m/s advanced, 6 m/s hard cap); back and strafe 0.5–0.75×; in-air control 0.5× | Meta, OVR, XRI, IWSDK |
| Acceleration | instant, or ≤ 0.1 s ramp with the vignette active; instant stop | Meta quantized velocity; 0.1 s est. |
| Vignette | strength 0.5 (0 = off), aperture 0.7 at full motion, feather 0.2, ease 0.3 s in and out, defined in degrees of view; trigger on speed > 0.1 m/s, yaw rate > 15°/s, or any acceleration | XRI, Meta ISDK, IWSDK; triggers est. |
| Movement direction | head-oriented default; hand option using the aim pose flattened to horizontal, roll ignored | Alyx, Pavlov, IWSDK |
| Teleport (Blink) and Dash | fade or rapid move; destination validated for a standard minimum body | Alyx |
| Player capsule | feet at HMD XZ; height = HMD − floor + headroom, clamped 0.5 m to standing; move the tracking origin with collide-and-slide | XRI; clamp est. |
| Head in geometry | sphere r 0.10–0.15 m; fade begins at contact, fully black at 0.05–0.10 m penetration; never freeze HMD pose | Meta soft collision, forums; numbers est. |
| Grip-pose calibration | per controller model (Touch, Quest 3, Index) plus user rotation and position offset; guns aim from the grip or aim pose, never the head | Pavlov 30°, Quest 3 15° fix, UEVR profiles |
| Hand filtering | no position smoothing beyond τ ≈ 20 ms; all filters τ-based | FRIK |
| Accessibility | dominant hand, movement-hand swap, single-controller mode, seated mode with height offset, artificial crouch; all persisted | Alyx, Meta |

**Tier 1: interaction reliability under pressure**

| Mechanic | Values | Basis |
|---|---|---|
| Near grab | 7 cm from the palm point (5–10 cm range); score by distance plus palm-direction alignment | HIGGS 5 cm cast, Godot 30 cm; 7 cm est. |
| Grip analog hysteresis | press at 0.6, release at 0.4 | Godot ±0.1 |
| Grip mode and drop guard | hold by default, toggle option; firearm drop modes instant, 1 s delay or double-click | Pavlov, H3VR |
| Shared-button leeway | withhold the game's input 0.3 s after a press on a selected object; re-inject for 0.03 s if no grab | HIGGS |
| Attach | object lerps to the hand over 0.1–0.2 s, scaled with distance; two-hand ease-in 0.15 s | HIGGS, XRI |
| Context priority | when the other hand holds a gun, the ammo zone beats nearby grabbables; the rack handle is grabbable only on approach from behind the slide | Metro misreads; rule est. |
| Body anchor for slots | body yaw follows head yaw with τ 0.5 s, ignores pitch and roll, freezes while a hand is in or approaching a zone | shock2quest |
| Hip and chest holsters | radius 0.15 m (0.12–0.18); belt 0.55 m below the eyes, scaled to calibrated height; per-slot user offsets | shock2quest, Hard Bullet; range est. |
| Shoulder ammo or backpack zones | centres at head − 0.06 m forward − 0.10 m up, ± 0.24 m right; radius 0.25 m; gate: hand < 0.08 m forward of the head and > 0.10 m to the side | shock2quest |
| Zone feedback | highlight plus haptic tick on entry; 0.15 s exit hysteresis; holstered items visible | shock2quest, HurricaneVR, Metro |
| Ammo zone | spawns a magazine for the other hand's gun from a counted per-type pool; empty hand returns the last weapon's magazine; partial magazines return rounds to the pool by default; infinite ammo as a zone flag | Alyx, Bonelab, HL2VR, H3VR |
| Reload state | `mag: Option<MagId>`, `chambered: bool`, `slide_locked: bool`, all independent | Alyx |
| Magwell snap | easy (default) 8–10 cm and ±45°; strict 3–5 cm and ±25°; seat with a 50–100 ms lerp plus click and haptic; 0.10 m is the upper bound | XRI 0.10 m, H3VR Easy Mag Loading; tiers est. |
| Slides, bolts, pumps | 10% dead zone, complete at ≥ 90% of travel, spring return; slide release by button or mag-slam option | Auto Hand; est. |
| Gesture budget | ≤ 3–4 gestures per default reload; multi-round grabs (2 shells per reach) | Alyx 7→4, Bonelab complaint |
| Quick-reload mode | eject → lower the gun to the waist or point it at the ground → short dwell → reloaded with unspent ammo recovered; alternative: slam the gun into the holster | HL2VR, Arizona Sunshine 2; dwell length est. |
| Weapon switching | same-hand wheel on stick-click hold, selected by hand position; opt-in auto-holster on release with a 0.2–0.5 s return lerp; menu fallback | Alyx, HL2VR, HL2VRU, H3VR; lerp est. |
| Gravity gloves and far grab | 5° hover cone, about 8° unhover, 5 m range; lock within 0.3 s; flick ≥ 1.2 m/s toward the hand; flight 0.715619 − 0.415619·e^(−0.656256·d) s; re-aim at the palm 0.1 s; ballistic re-solve 0.2 s; angular damping 8; catch radius 0.5 m (HIGGS 0.9); one-hand-mass gate; assist option with a lower threshold | HIGGS, Godot, Meta, Valve |

**Tier 2: weapon feel**

| Mechanic | Values | Basis |
|---|---|---|
| Two-hand aim | forward = filtered support-hand position − primary grip; roll from the primary hand; One-Euro on the support hand (VRE defaults 0.1 / 10 / 10; at metre scale start at minCutoff 0.5–1.0 Hz, β 5–10); break when the support hand is more than 25 cm from the foregrip | VRE, XRI; metre-scale values est. |
| Virtual stock | per weapon class, default on for scoped rifles only; pivot = HMD + shoulder offset; engages at 35 cm, full influence over the last 20 cm | VRE, Pavlov, H3VR |
| Recoil | clamped additive pitch, yaw, rearward and jitter offset; lerp 30/s, decay 20/s; pistols 5–10° pitch and about 1 cm rearward per shot; two-hand ×0.5, stock or bipod ×0.1–0.3; applied to the weapon pose only | VRE, H3VR ratios; magnitudes est. |
| Throw release | runtime controller velocity about 11 ms in the past, or the peak of a 5-frame (about 55 ms) window averaged with its neighbours; linear ×1.1 (≤ 1.5); angular ×0.6; add ω × r (cap 5 m/s) and locomotion velocity; computed from the controller, never the lagging body; drop samples for a frame after tracking re-acquires; ignore hand collision 0.1 s | SteamVR, HIGGS, XRI, Meta forum |
| Melee | speed at the impact point over 3–5 frames; minimum 1.5–2.5 m/s; per-target cooldown 0.2–0.5 s; one-hand ×0.5; stabs need forward-normal dot ≥ 0.9 | HL2VR, shock2quest, VRE; thresholds est. |
| Scopes and aim aids | render-to-texture lens with eyebox shadow, mono, reduced rate; optional laser sight | Godot shader, Titanfall 2 VR, HL2VR |
| Grenades | arm by pin pull or trigger; fuse on release; optional arc preview while a modifier is held | HL2VR |

**Tier 3: the feedback bus**

| Mechanic | Values | Basis |
|---|---|---|
| One event, one frame | hover, grab, release, holster in and out, mag seat, eject, rack, last round, empty click, fire, damage and collision each emit a haptic and a sound on the same frame | Meta, Valve |
| Haptic encoding | amplitude envelopes first, frequency optional; ticks at `XR_MIN_HAPTIC_DURATION`; ≥ 5 ms between pulses per hand; release = grab at lower amplitude; success 2 pulses over 175 ms rising, warning 2 over 150 ms falling, error 4 abrupt; global slider and mute | Meta, OpenXR, OpenVR |
| Collision haptics | none below 0.2 m/s; amplitude 0.1 + 0.05·v + 0.03·m^0.6; 10 ms; ×0.4 when damped | HIGGS |
| Runtime sound library | a few consistent grab sounds; distinct mag seat, eject, rack, last-round and empty-chamber cues and a failed-action clunk; spatialised at the hand; impacts scaled by velocity | Alyx |

**Tier 4: physics hands and world interaction (optional, per game)**

| Mechanic | Values | Basis |
|---|---|---|
| Hand body | dynamic box about 0.10 × 0.03 × 0.18 m, about 0.5 kg, CCD on; velocity tracking v = clamp(k·Δp/dt), k 0.5–1.0, with an angular equivalent | HIGGS half-extents, LÖVR; k est. |
| Leash and ghost | snap to the controller at 0.7 m separation (0.5–1.0), collision off during return; show a ghost hand when blocked by more than 3–5 cm | HIGGS, Godot, HurricaneVR; ghost threshold est. |
| Held objects | 6-DOF motor joint to the hand; hand-object contacts filtered; motor force ≤ 500 × mass; stiffness drops to 1–5% while the object touches the world; 0.1 s force fade-in; two-hand strength ×2; guns may instead merge their colliders into the hand | HIGGS, Godot, HurricaneVR |
| Weight | hand displacement per step ≤ F·dt/m, with F rising 15 → 300 N over 1 m of controller lead | Godot |
| Mechanisms | hinge and slider joints with limits; the hand latches to the handle and drives it from the controller target; unlatch at 0.3–0.5 m; doors open both ways | Alyx; distances est. |
| Climbing | body moves by −(controller − grab point), gravity off; release at 1.0 m drift; fling = 5-sample average × 1.0 + forward push 1.0 | Godot |

**Tier 5: the body (player-selectable)**

| Mechanic | Values | Basis |
|---|---|---|
| Visibility | hands-only, hands + arms, or full body; hands-only default for fast games | Alyx, Parger, Lugrin |
| Torso | neck-to-head 0.13 m, shoulders 0.31 m; yaw from Σ normalized HMD→hand directions, clamped to ±50–80° of HMD yaw, blended at 0.7; flip detector at a 150–210° jump; down-weight hands near the face or above the head | Parger, VRArmIK, FRIK |
| Root yaw dead zone | 25° standing, 10° moving | VRIK |
| Spine | pitch = (h0 − h)/h0 × 105–135° + 0.1–0.33 × head pitch; pelvis back 0.5; minimum head height 0.8 m | Parger, FRIK, VRIK |
| Arms | analytic two-bone; elbow flexion 10°–145°; clavicle reach 0.15 L beyond 0.5 L, downward ×0.4; swivel smoothing τ 40–80 ms; skip the solve beyond 2.25 L | FRIK, VRIK |
| Legs | roomscale: step at 0.4 m or 60°, feet 0.3 m apart, 0.22–0.33 s steps, 0.25 m leg-cross guard; stick locomotion: velocity gait with 0.3 m move threshold and 0.5 m maximum root offset | VRIK |
| Calibration | stand → eye height h0; T-pose → (span − 0.31 m)/4 per arm segment; scale the avatar, keep the world 1:1; seated mode sets a virtual standing h0 | Parger, VRIK, FRIK |
| Head | chopped for the wearer only; full head in shadows and mirrors | VRChat, FRIK |

The adapter contract follows from this split. This is my inference from what the successful conversions had to rebuild.

The runtime owns:

- tracking, hands, grab and holster zones
- the reload state machine and comfort settings
- the feedback bus and prompt glyphs

Each per-game adapter declares:

- **Weapon data:** grip, foregrip, magwell, stock and muzzle transforms, ammo type and capacity, chamber model, recoil profile and hold class.
- **Game state hooks:** reading and writing ammo and chamber state, a "reload complete" injection, fire and weapon-switch commands.
- **Camera and HUD:** a cutscene and forced-camera flag, and HUD elements to re-project onto the hand or weapon.
- **World data:** near-field collision geometry for hands, held objects and head fade, and the game's interactables with approximate masses.

Where a game cannot export collision, the runtime falls back to non-colliding hands with ghost rendering. This is the configuration players already accept from hands-only shooters.

## Conclusion

The decisive design variable is not realism but where the system allows itself to lie. The hand may never lie, about position or about pose. The body, the held object's lag, the magwell's tolerance and the trajectory of a pulled object may all lie as long as the lie favours the player. That split suits an injector runtime unusually well. The fixes for the three worst complaint classes (unrequested camera motion, misfiring grabs and pulls, misaligned poses) sit entirely below the adapter. The most expensive game-specific features (authored grab poses, physics bodies, convincing legs) are the ones the research shows players value least or split on. Arm and leg fidelity is close to zero-sum: done right it goes unnoticed, and done wrong it is worse than leaving it out.

The independent open-source systems land on similar constants: snap-back at 0.7–1.0 m, throw windows near 50 ms, 0.3 s input leeway, 0.10–0.25 m slot radii and 45° snap turns. They describe a shared tuning space, but no user study has validated any of it, and Valve's and Stress Level Zero's real values remain unknown. The runtime's advantage is that it sees every game. Instrumenting grab-failure rate, drops mid-reload, reload duration and comfort-setting churn across adapters would turn these inherited defaults into measured ones. No single studio has been in a position to do that.
