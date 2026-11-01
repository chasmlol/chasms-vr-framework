# Full-Body IK for First-Person VR Avatars (HMD + 2 Controllers, Optional Trackers)

> A research snapshot (2026-10-21), not a description of the framework's code: the body solver as
> built is `crates/ik`, and the later research behind it is `docs/design/ik.md`.

Research notes, current as of 2026-10-21. Scope: real-time, game-agnostic reconstruction of a believable body from 3 tracked points (head, two hands), with optional extra trackers. ML approaches are covered briefly.

Conventions used below:
- "Bethesda units" (FRIK, Skyrim VRIK): the code works in game units. My conversion of about 1.43 cm per unit (70 units ≈ 1 m) is the commonly cited Bethesda scale. I did not verify it in this research, so every metric conversion of FRIK numbers is marked **[est.]**.
- "Retention per frame at 90 Hz" (FRIK smoothing): r means the value keeps r of its old value each 1/90 s. I converted these to exponential time constants with τ = −(1/90)/ln(r). These conversions are my own and are marked **[computed]**.
- The Final IK (RootMotion) values come from its C# source as vendored in open-source repos (vhvr-mod, and a newer copy for Animated locomotion). Final IK is a commercial asset, so treat the algorithm descriptions as paraphrases. Check the defaults against your own Final IK version.

---

## 1. How do the major games, mods and engines handle a body from 3 points, and why?

### Takeaway
Shipping approaches fall into four families:
1. **Analytic/heuristic IK with procedural feet.** Final IK VRIK, which came from *Dead and Buried*. Skyrim VRIK and its Fallout 4 port FRIK. VRChat and Resonite, which build on VRIK.
2. **Physics-simulated bodies driven by the tracked points.** Boneworks, Bonelab and Marrow 2, which grow from 6 to 11 to 18 rigid bodies.
3. **Deliberately no body.** Half-Life: Alyx renders only hands but simulates invisible arms for physics checks.
4. **Engine toolkits that supply only generic solvers.** Unreal's FBIK/PBIK, the VR Expansion Plugin's yaw-following component, and Unity Animation Rigging's Two-Bone IK. With these, the developer must supply the VR heuristics themselves.

The best-regarded mods (VRIK and FRIK) and the research solver (Parger 2018) agree on the core moves:
- Derive torso yaw mostly from the hand positions, with the HMD yaw only as a clamp.
- Bend the spine from head height and pitch.
- Choose the elbow plane with hand-position and wrist-twist heuristics.
- Plant feet procedurally.

### Cited Findings

**Final IK VRIK (RootMotion, Unity). The de-facto reference solver.**
- VRIK solves the body parts in order: spine, then locomotion, then legs, then arms. It combines analytic and heuristic algorithms and was built for *Dead and Buried*. The article says it runs 2–3× faster than Final IK's FullBodyBipedIK. — [Meta dev blog: Character Animation in Dead and Buried](https://developers.meta.com/horizon/blog/developer-perspectives-character-animation-in-dead-and-buried/)
- The *Dead and Buried* write-up describes the design choices:
  - The HMD is the primary target for head, neck, and spine bend and twist.
  - The horizontal (XZ) hand positions rotate the chest around the vertical axis. Hand height rotates it around the forward axis.
  - After the legs are solved, the spine moves so the hips let both toes reach their targets.
  - Locomotion is fully procedural and footstep-based. An earlier 8-direction strafing blend tree was dropped as "too slow and inaccurate" and because it conflicted with turning blend trees.
  - The center of mass is the centroid of the HMD-plus-hands triangle. The center of pressure is midway between the feet.
  - A step triggers when the angle of the CoP→CoM vector from vertical passes a threshold. Turning on the spot has its own angular threshold.
  - Before stepping, the solver checks that the foot won't cross the other leg, and ray-casts for obstacles. Head and hand velocity lowers the balance threshold.
  - Legs use a dual-pass trigonometric solver that anchors the toes, not the ankles, so the avatar can rise onto its toes. If the head is too high for the feet to reach the ground, the feet stay planted and the spine/hips adjust.
  - Shoulders are solved in chest space with rule-based angle offsets and clamps, which prevents flips during cross-body grabs.
  - The elbow plane normal is a weighted blend of three inputs: controller rotation, controller position relative to the chest, and empirically tuned weights. The article admits this guess is "never conclusive".
  - Motion was tuned by watching people play, favouring relaxed poses and avoiding singularities. — [same source](https://developers.meta.com/horizon/blog/developer-perspectives-character-animation-in-dead-and-buried/)
- The VRIK manual documents the solver's inputs and conventions:
  - Head and hand targets should be child objects of the camera and controllers, not the devices themselves.
  - An optional pelvis target is "useful for seated rigs or a pelvis tracker".
  - VRIK samples the start pose to decide limb bend directions, so elbows and knees must be slightly bent in the bind pose.
  - LOD 1 (simplified shoulders, no stretching of planted feet, reduced spine quality) gives about 30% speed-up. LOD 2 culls the solver but still moves the root.
  - Size calibration uses sizeF = (headTarget.y − root.y)/(head.y − root.y), and multiplies root.localScale by sizeF. — [Final IK VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html)
- VRIK has two locomotion modes: "Procedural (legacy)" footstep shuffling, and "Animated", which drives an Animator blend tree. The Animated tree uses VRIK_Horizontal, VRIK_Vertical, VRIK_Turn, VRIK_IsMoving and VRIK_Speed, with a transition named VRIK_Stop. — [Final IK VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html)
- The procedural mode's own tooltip says it is "for stationary VR games. Not designed for roomscale and thumbstick locomotion"; for those it recommends a strafing locomotion blend tree. — [IKSolverVR.cs (Final IK, vendored in vhvr-mod)](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVR.cs)
- The Resonite VRIK component is based on Final IK's VRIK solver. — [Resonite wiki: VRIK](https://wiki.resonite.com/Component:VRIK)

**Skyrim VR: VRIK Player Avatar (by "prog"). Closed source; the Nexus page blocked automated fetches.**
- The mod displays and animates a player body that follows your movements. It adds up to 14 weapon holsters, a gesture system, an MCM configuration menu, and a "VRIK Calibration Power" that calibrates avatar height. — [Nexus: VRIK (search summary)](https://nexusmods.com/skyrimspecialedition/mods/23416)
- The addon "VRIK Actions" changes VRIK settings automatically during control-loss events (sneaking, mounting, sitting, death) and "fixes most VRIK head spins". It warns it can cause motion sickness. — [Nexus: VRIK Actions (search summary)](https://www.nexusmods.com/skyrimspecialedition/mods/69282)
- FRIK's arm solver carries the comment "Algo credit to prog from SkyrimVR VRIK mod". Its leg solver is "adapted solver from VRIK. Thanks prog!". FRIK.ini quotes Skyrim VRIK's values for several parameters: shoulder reach fraction 0.225, downward damp 0.4, upper-arm twist split 0.7. So FRIK's code is the best available public proxy for VRIK's arm algorithm (details in §3). — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp); [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)

**Fallout 4 VR: FRIK (rollingrock / ArthurHub). Open source GPL-3.0, actively maintained.**
- FRIK is an F4SE plugin. It provides full-body IK (posture, arms, leg walking, head/neck, hand poses), weapon repositioning, an in-VR Pip-Boy, and a public API. Its latest release is v0.79.1 (2026-09-21), with commits through 2026-10-14. — [FRIK GitHub](https://github.com/rollingrock/Fallout-4-VR-Body)
- The per-frame order is:
  1. Restore the skeleton's local transforms to defaults.
  2. Compute neck yaw and pitch.
  3. Set up the head, unless it is hidden.
  4. Place the body under the HMD.
  5. Set body posture (spine pitch).
  6. Set knees.
  7. Walk (procedural steps).
  8. Two-bone leg IK.
  9. Update the hand targets, including recoil.
  10. Solve the right arm, then the left.
  11. Apply upper-arm twist.
  12. Cull geometry.
  13. Run hand pose.

  — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)

**Half-Life: Alyx (Valve). Deliberately hands only.**
- Valve doesn't render arms "due to our experiences with playtesting". Players don't notice them missing (spectators do), and players dislike arms blocking their view. — [VentureBeat: Alyx AMA](https://venturebeat.com/business/half-life-alyx-ama-everything-we-learned)
- Alyx simulates invisible arms from each hand back to the HMD. These detect physically impossible situations, such as a drawer closing over the wrist. — [VentureBeat: Alyx AMA](https://venturebeat.com/business/half-life-alyx-ama-everything-we-learned)
- Valve's reasoning, per Robin Walker and Brad Kinley:
  - The system knows the hand and head positions but not the arms, and arm lengths and motion vary too much to place arms accurately for everyone.
  - Correct arms go unnoticed but incorrect arms stand out, so they chose not to show arms.
  - Players filter out missing arms, much as the brain does in real life.
  - Busy scenes reduce attention to arms; sparse scenes increase it. — [Game Informer interview](https://gameinformer.com/interview/2020/03/23/valve-talks-half-life-alyx-and-why-arms-dont-work-in-vr)
- Critics disagreed: PC Gamer argued "Alyx needs arms" for immersion, noting other VR games already show arms and bodies. — [PC Gamer](https://www.pcgamer.com/uk/alyx-needs-arms/)

**Boneworks / Bonelab / Marrow 2 (Stress Level Zero). Physics body.**
- Boneworks is built to be entirely physics-based. The player controls a full virtual body that responds both to real-world input and to obstructions in the game world. — [Wikipedia: Boneworks](https://en.wikipedia.org/wiki/Boneworks); [Road to VR reveal](https://roadtovr.com/boneworks-reveal-stress-level-zero-physics-based-interactions-embodiment/)
- Brandon J. Laatsch (SLZ) gave the rigid-body counts. In Boneworks the physics rig had 6 rigidbodies. Bonelab added arms and a chest, for 11. In Marrow 2 "the spine has been completed, legs have been added and the ball has been removed", for an 18-body physics player. — [Brandon J. Laatsch on X](https://x.com/BrandonJLa/status/2009741997897044046)
- Fan-wiki description (secondhand, from search snippet; the page itself was paywalled to the fetcher):
  - "Hexabody" has separate controller, realtime, physics and simulated (visual) rigs.
  - Locomotion runs through a highly frictional rigidbody sphere (the "foot-ball"), which gives realistic acceleration and cheap ledge detection.
  - Waist, torso and head colliders form a primitive collision skeleton. The successor 1Marrow uses a "Heptabody" 7-point configuration. — [Boneworks fandom wiki: 1Marrow Interaction Engine](https://boneworks.fandom.com/wiki/1Marrow_Interaction_Engine)

**Lone Echo / Echo VR (Ready At Dawn).**
- Meta's acquisition announcement credits Lone Echo with reworking VR movement through continuous zero-g locomotion and full-body IK. — [Meta blog: Welcoming Ready At Dawn](https://www.meta.com/blog/welcoming-ready-at-dawn-to-facebook/)
- The robotic body is fully rendered and visible. — [Road to VR hands-on](https://roadtovr.com/hands-on-lone-echo-nails-zero-gravity-in-vr-with-oculus-touch/)
- TechRadar credits the experience feeling real partly to "the way it intelligently guesses at the position of your arms". — [TechRadar](https://www.techradar.com/news/lone-echo-should-do-for-vr-what-mario-64-did-for-nintendo-send-it-stratospheric)
- The hands use a procedural posing system that adapts the grip to each surface from a physical model. — [Road to VR: Lone Echo's virtual hands](https://roadtovr.com/?p=166501)

**Asgard's Wrath / Asgard's Wrath 2 (Sanzaru).**
- Sanzaru lists full-body IK among the VR features it has pushed. — [Unreal Engine developer interview (Sanzaru)](https://www.unrealengine.com/es-ES/developer-interviews/developer-sanzaru-games-aims-to-deliver-a-next-level-30-hour-vr-epic-with-asgard-s-wrath)
- A search snippet attributes to Sanzaru's creative director the claim that the studio was one of the first to do full-body IK, in Ripcoil. Verified only via the snippet. — [GamesBeat](https://gamesbeat.com/?p=173944)
- A Tom's Guide review of AW2 notes that "your hands constantly clip through each other". — [Tom's Guide AW2 review](https://www.tomsguide.com/reviews/asgards-wrath-2)

**Unreal Engine.**
- Full Body IK (Control Rig / IK Rig) is a Position Based IK solver. It supports per-bone rotation and position stiffness (0 free to 1 locked), per-axis limits, preferred angles and stretch. Effectors have Position/Rotation Alpha, Strength Alpha, Pull Chain Alpha and Pin Rotation.
- FBIK has three Root Behaviour options: Pre Pull, Pin to Input ("useful for partial-body setups") and Free.
- The docs use 0.8 pelvis stiffness and a 45° knee preferred angle as examples. The docs don't cover VR. — [UE docs: Control Rig Full Body IK](https://dev.epicgames.com/documentation/en-us/unreal-engine/control-rig-full-body-ik-in-unreal-engine)
- The VR Expansion Plugin (mordentral) does not ship a VR body solver. Its `ParentRelativeAttachmentComponent` follows HMD yaw:
  - `YawTolerance` defaults to 0, so it follows the HMD yaw every frame.
  - `bLerpTransition` defaults to true, with `LerpSpeed` = 100°/s via a constant-rate FixedTurn.
  - When the tolerance is exceeded, the component turns toward the HMD yaw and broadcasts `OnYawToleranceExceeded`.
  - Commented-out code ports the Parger/VRArmIK controller-based torso estimate: neck-to-shoulder offset (−0.02, 0, −0.15) m, head-neck distance 0.03 m, and an 80° head-yaw clamp. — [VRE ParentRelativeAttachmentComponent.h](https://github.com/mordentral/VRExpansionPlugin/blob/Master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/ParentRelativeAttachmentComponent.h); [.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/Master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/ParentRelativeAttachmentComponent.cpp)
- UBIKSolver is an Unreal C++ port of the Parger upper-body IK, driven by two controllers plus the HMD. — [UBIKSolver GitHub](https://github.com/kjduling/UBIKSolver)
- On an older UE forum thread, one developer suggested capturing several target torso poses and blending between them using HMD pitch and roll. — [UE forums: Body IK](https://forums.unrealengine.com/t/body-ik/75187)

**Unity.**
- In Animation Rigging, the Two Bone IK Constraint takes a target plus a "hint" (pole) object that controls the elbow direction. Unity's tutorial covers the setup: Rig Builder, Rig, constraint, target and hint.
- For grabbing, Unity staff suggested feeding a MultiParent constraint's result into the TwoBoneIK target. — [Unity Learn: Working with Animation Rigging](https://learn.unity.com/tutorial/working-with-animation-rigging); [Unity discussions: IK pinning](https://forum.unity.com/threads/ik-pinning.810444/)
- One devlog names the main difficulty: keeping the hint points moving in step with the targets. — [itch.io devlog: Breaking free from Mecanim with IK rigs](https://itch.io/devlog/530175/breaking-free-from-mecanim-with-ik-rigs.amp)
- VRArmIK (dabeschte) is a free Unity implementation of Parger's thesis. A forum user testing it on Quest 2 robot arms called its elbow correction the best they had seen. — [VRArmIK GitHub](https://github.com/dabeschte/VRArmIK); [UE forum post referencing it](https://forums.unrealengine.com/t/2639472)

**Social VR / platform solvers.**
- VRChat IK 2.0 offers measure-by-arm-span or measure-by-height. In VRChat's testing, height worked better for full-body tracking.
- Its lock modes trade head accuracy against hip accuracy:
  - Lock Hip strictly enforces the hip; the head may drift.
  - Lock Head strictly enforces the head; the hip may drift.
  - Lock Both enforces both and can produce odd spine or neck angles. — [VRChat docs: IK 2.0](https://docs.vrchat.com/docs/ik-20-features-and-options)
- A VRChat feedback thread reports that "Lock Both" spine scrunching pathologically overbends the neck. — [VRChat feedback](https://feedback.vrchat.com/vrchat-ik-20/p/in-lock-both-mode-spine-scrunching-pathologically-overbends-the-neck)
- Meta Movement SDK:
  - "Generative Legs" infers legs from the upper body on all Quests, with no extra sensors. It can't capture some motions, such as crossed legs.
  - Inside-Out Body Tracking (IOBT, Quest 3 only) camera-tracks elbows, wrists and torso. — [Meta dev blog: IOBT & Generative Legs](https://developers.meta.com/horizon/blog/inside-out-body-tracking-and-generative-legs); [Road to VR](https://www.roadtovr.com/quest-3-dev-tools-body-tracking-avatars/)
  - Generative Legs can run on only the three-point tracking of Quest 2 and Pro. — [Mixed News](https://mixed-news.com/en/?p=18891)

**ML approaches (brief, for heuristics worth borrowing; not required at runtime).**
- AvatarPoser (ECCV 2022) predicts world-space full-body pose from head and hand motion with a Transformer encoder. It then refines the arms with IK. — [AvatarPoser arXiv](https://arxiv.org/abs/2207.13784v1)
- QuestSim (Meta) uses reinforcement learning to drive a physics-simulated avatar from the HMD and two controllers. The simulation gives access to contact forces. — [QuestSim arXiv](https://arxiv.org/pdf/2209.09391.pdf)
- AGRoL ("Avatars Grow Legs", Meta) is a compact MLP-based diffusion model that runs in real time and is especially aimed at the lower body. — [AGRoL](https://ar5iv.labs.arxiv.org/html/2304.08577)
- Head and hand tracking give weak cues for the pelvis, legs and feet; that is why the lower body is the hard part. — [AllPCB summary (secondary)](https://www.allpcb.com/allelectrohub/mlp-based-full-body-avatar-motion-capture-for-vr-headsets)

### Inferences
- Final IK's VRIK and the *Dead and Buried* "VRIK" are almost certainly the same lineage (same name, same solve order, same 2–3× claim). In practice, most Unity VR bodies of 2017–2026 (Dead and Buried, VRChat, Resonite, many indies) are tuned versions of this solver. This is my inference; the blog doesn't name Final IK.
- Skyrim VRIK's arm algorithm is effectively public through FRIK's credited port. FRIK.ini quotes VRIK's constants where FRIK differs (0.225 shoulder reach, 0.7 twist split), so an engineer can reproduce "VRIK-feel" arms from FRIK's code.
- Physics bodies (Boneworks lineage) solve a different problem: hands can be blocked by the world, and the body interacts physically. That brings their own complaints (arms snag, the body lifts when you open doors; see §6). For a game-agnostic runtime, the analytic VRIK/FRIK family is the realistic baseline, with physics as an optional layer on top.
- None of the engine toolkits (UE FBIK, Unity Animation Rigging) solves the VR-specific estimation problem. They take targets, so the torso-yaw, pelvis and elbow-hint heuristics in §2–3 are the real deliverable of a runtime solver.

### Gaps
- Skyrim VRIK's own settings list (MCM names, defaults) could not be retrieved: Nexus returned HTTP 403, and the Reddit beta thread mirror returned 404.
- I found no technical talk or devlog for Lone Echo's body IK, Asgard's Wrath 2's first-person body, or Into the Radius's body. Searches returned only reviews or nothing. Whether AW2 shows a full first-person body in normal gameplay is unconfirmed.
- I found no primary SLZ documentation of how the Boneworks or Bonelab body handles knees and crouching. Only fan-wiki and social posts.

---

## 2. Pelvis / spine / chest estimation from head + hands

### Takeaway
The robust recipe is:
1. Place a neck pivot at a fixed offset below and behind the HMD.
2. Get torso yaw from the sum of the head-to-hand direction vectors. Clamp it to within about 50–90° of head yaw, and handle the hands-behind-back/head ambiguity.
3. Bend the spine by an amount proportional to how far the head is below standing height, plus a fraction of head pitch.
4. Push the pelvis backward as the head drops, so the body bends over rather than squats in place.
5. Let the root yaw lag the head with a dead zone: 25° in VRIK procedural; 10° while moving and 90° standing in VRIK Animated.

### Cited Findings

**Neck pivot offsets.**
- Parger 2018 uses a fixed offset in HMD space to the neck (the head's centre of rotation). The neck-to-head distance is fixed at ln = 0.13 m and the shoulder-to-shoulder width at 2ls = 0.31 m. Neck roll is assumed to be zero. — [Parger et al. 2018 (VRST)](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- The VRArmIK (Parger implementation) defaults are:
  - `headNeckDistance` = 0.03 m along `headNeckDirectionVector` = (0, −1, −0.05) in HMD space.
  - `neckShoulderDistance` = (0, −0.10, −0.02) m. — [VRArmIK ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)
- VR Expansion Plugin (commented-out port): `neckShoulderDistance` = (−0.02, 0, −0.15)·WorldToMeters and `headNeckDistance` = 0.03 m. — [VRE ParentRelativeAttachmentComponent.h](https://github.com/mordentral/VRExpansionPlugin/blob/Master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/ParentRelativeAttachmentComponent.h)
- Legacy Oculus SDK neck model: a forum poster reports config-utility neck-to-eye values of 0.075 vertical and 0.0805 horizontal. Oculus staff said the camera rig sits at centre-eye, and the neck pivot can be placed from these components. These are old (2014-era) forum values, not current docs. — [Meta community forum thread](https://communityforums.atmeta.com/discussions/dev-unity/0-4-3-head-rotation-pivot/321248)

**Torso (chest/neck) yaw.**
- Parger derives neck yaw from the sum of the normalized HMD→controller directions, not from HMD yaw. HMD yaw would make the shoulders swing whenever the user looks left or right. HMD yaw is used only to disambiguate when both hands are behind the torso, and to keep the head-neck yaw difference from exceeding 90°. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- The VRArmIK implementation does the following:
  - Takes yaw as atan2 of the summed horizontal (Y-ignored) shoulder→hand unit vectors.
  - Toggles a "hands behind head" flag when the target yaw jumps by 150–210° between frames, which adds 180°.
  - Clamps the result to within `maxDeltaHeadRotation` = 80° of head yaw. — [VRArmIK ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)
- FRIK `getNeckYaw` also uses the sum of the HMD→left and HMD→right hand vectors, but with weights:
  - Returns 0 if either hand is within 10 units of the HMD **[est. ~14 cm]**.
  - Weight starts at 1. It drops 0.05 per unit a hand is above the HMD, reaching 0 at about 20 units ≈ 29 cm **[est.]**.
  - If the hands cross the chest (left hand right of right hand in HMD space), the weight drops by 0.02 per unit of crossing.
  - Switches to a secondary angle when the pitch difference exceeds 80°.
  - Clamps the final yaw to ±50°.
  - The body forward is the pitch-levelled HMD forward rotated by **0.7 × neckYaw**.
  - The comments say this "still struggles with hands close to the face and with one hand low and one hand high". — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- Final IK VRIK. The chest target is the head rotation × the bind-pose chest offset, then "AdjustChestByHands":
  - Hand positions relative to the head, in the anchor frame, are normalized by sizeMlp = spineLength/0.7 m.
  - Yaw term: c.x = 5·rotateChestByHands·(pL.x|pL.x| + pL.z|pL.z| + pR.x|pR.x| − pR.z|pR.z|), with yaw = atan2(c.x, 1).
  - Roll term: t.x = 0.5·rotateChestByHands·(pL.y − pR.y), with roll = atan2(t.x, 1) about −forward.
  - The spine is then bent toward that target with weight `neckStiffness` (0.2) and clamped by `chestClampWeight` (0.5, which allows 90° relative to the head). — [IKSolverVRSpine.cs (vendored)](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs)
- In VRIK, the root (feet) yaw rotates only by the excess once the head's face direction passes `maxRootAngle` (default 25°). Root yaw therefore lags with a 25° dead zone. `rootHeadingOffset` allows an angled stance, for example in shooters. — [IKSolverVRSpine.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs); [VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html)
- VRIK Animated mode:
  - `maxRootAngleMoving` = 10° and `maxRootAngleStanding` = 90°, blended with SmoothDamp over 0.2 s.
  - Turning is detected when head-vs-root yaw exceeds 0.2·90° = 18°. The turn value is lerped at 3/s and fed to VRIK_Turn ×2. — [IKSolverVRLocomotion_Animated.cs (vendored copy)](https://github.com/whx-prog/The-Seed-Link-Future/blob/HEAD/The%20Seed(2022)/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion_Animated.cs)
- VRE's body-follow defaults to pure HMD yaw (`YawTolerance` = 0), turning at 100°/s when a tolerance is set. — [VRE .cpp](https://github.com/mordentral/VRExpansionPlugin/blob/Master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/ParentRelativeAttachmentComponent.cpp)
- Jiang et al. 2016, as summarized and reimplemented by Parger:
  - Combines head and hand horizontal positions for neck forward.
  - Keeps the waist directly under the head while standing.
  - Switches to a "crouch" state when looking and moving downward. In that state the waist is fixed horizontally and the upper body bends.
  - Updates the shoulder from head forward only when recent head velocity is low.
  - Parger could not get reasonable neck or shoulder placement from it. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)

**Spine bend / lean / crouch vs bend-over.**
- Parger's neck pitch (the text extraction was partly garbled): θn = ((h0 − h)/h0)·θn,0 + b·θH, with θn,0 = 135.3° and b = 0.333. Here h0 is the calibrated standing HMD height, h the current height and θH the HMD pitch. The lower and more downward-looking the head, the more the chest bends forward. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- FRIK `getBodyPitch` uses angle = |(H − hmdZ_adj)/H| · (105.3° + 0.1·neckPitch°), divided by 1.2 when not in power armour.
  - The hip (COM) is then placed by tan(bodyPitch) behind the neck point and re-normalized to the torso length.
  - The neck point is moved back by 5·|neckPitch|·scale (units), and down by 6·neckPitch·scale. This is "to not change player height when looking up/down and move the body back, especially when looking down".
  - For artificial "comfort sneak", FRIK uses a static 10° body pitch so the body doesn't bob. — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp); [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)
- Final IK VRIK spine:
  - The pelvis follows the head-target position delta with `bodyPosStiffness` = 0.55 and the head rotation with `bodyRotStiffness` = 0.1.
  - When the head moves down, the pelvis is pushed back along −root.forward by (vertical head delta / scale) · `moveBodyBackWhenCrouching` (0.5) · headHeight.
  - The pelvis is then limited over 2 iterations so both legs can still reach their footsteps.
  - A FABRIK pass runs from the pelvis (lerped toward its animated position by `maintainPelvisPosition` = 0.2) to the head. The neck→head is bent by `headClampWeight` = 0.6.
  - `minHeadHeight` = 0.8 m keeps the head above the root, which allows kneeling and crawling down to that height. — [IKSolverVRSpine.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs)
- VRIK with a pelvis tracker: the bones are translated to the pelvis target, and the spine is re-solved trigonometrically (spine→chest→head at 0.9×weight, then chest→neck→head). — [IKSolverVRSpine.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs)
- VRArmIK "shoulder right rotation" (forward lean of the shoulder girdle):
  - heightFactor = clamp(−(hmdY − vrSystemOffsetHeight)/playerHeightHmd − rightRotationStartHeight(0), 0, 1). This is effectively the fraction of standing HMD height lost, assuming vrSystemOffsetHeight is the standing reference.
  - Angle = heightFactor·142° + clamp((headPitchAngle − 20°)·0.3·heightFactor, 0, 50°).
  - The total is clamped to 0–50°. The shoulders are then rotated about the head position. — [VRArmIK ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)
- VRIK's root/pelvis placement for Animated locomotion:
  - The root lerps toward the HMD's horizontal position at 30 (moving), 10 (stopping) or 10 (turning), each × max(|headVel|, 0.2), smoothed at 20/s.
  - The offset is capped at `maxRootOffset` = 0.5 m.
  - `standOffset` shifts the standing XZ position relative to the HMD. — [IKSolverVRLocomotion_Animated.cs](https://github.com/whx-prog/The-Seed-Link-Future/blob/HEAD/The%20Seed(2022)/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion_Animated.cs); [VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html)
- FRIK always places the body root directly under the camera's X/Y, with configurable offsets in units:
  - Standing: HMD up 8.6, body up −4.2, body forward −3, leg slack 0.
  - Sitting: HMD up 35, body up −5, forward −3, leg slack 12.
  - Separate values for power armour.
  - `fSkeletonLegSlackTarget` = 2 "to preserve slight knee bend". — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)
- With trackers, VRChat exposes the head-vs-hip priority trade-off as the lock modes described in §1. — [VRChat IK 2.0](https://docs.vrchat.com/docs/ik-20-features-and-options)

### Inferences
- A good default torso-yaw estimator, synthesising Parger, FRIK and VRIK:
  - yaw_hands = atan2 of Σ normalized(HMD→hand), horizontal only.
  - Down-weight it as hands rise above the head or come within about 15 cm of the face, and when the hands cross. FRIK's weights are a ready-made starting point.
  - Clamp yaw_hands to within ±50–80° of HMD yaw.
  - Blend about 70% toward it (FRIK's 0.7).
  - Low-pass filter the result.
  - Then let the feet/root follow with a 25° dead zone when stationary. Use a tight dead zone (about 10°) when moving, so the legs face the travel direction.
- Use "Bend over vs crouch" as an explicit continuum:
  - Head-height loss with forward head pitch → spine pitch plus pelvis moving back (VRIK `moveBodyBackWhenCrouching`, FRIK's hip-back placement).
  - Head-height loss with level gaze → knee bend, with the pelvis staying under the head.
  - Parger's and FRIK's pitch formulas both multiply the relative head-height drop by a base angle of about 105–135°, and add 0.1–0.33 × head pitch.

### Gaps
- No source gave a measured "neck pivot from eye centre" value for current headsets (Quest 3, Index); only the 2014-era Oculus forum numbers and the solver defaults above. Typical values (≈8 cm behind and ≈7.5–10 cm below the eyes) are **[estimates]** consistent with those numbers.
- I found no published user-study comparison of yaw-from-hands vs yaw-from-HMD vs yaw-from-movement for full bodies, beyond Parger's arm-only study.

---

## 3. Arm IK: two-bone solve, elbow/pole prediction, shoulder/clavicle, length mismatch, wrist twist

### Takeaway
Everyone uses an analytic law-of-cosines two-bone solve. The quality lives in the swivel (elbow-plane) choice. The best open heuristics are:
- **Parger/VRArmIK.** A swivel angle from the hand position in shoulder space, plus singularity correction near the shoulder's vertical axis, plus correction from wrist rotation.
- **VRIK/FRIK.** A swivel from wrist twist mapped into a min/max window, and that window is modulated by "hand behind body", "arms crossing the chest" and "hand lifted".

The clavicle (shoulder bone) rotates toward the hand only past about 50% of arm length, up to about 33° (Parger), or moves by 15–22.5% of arm length (FRIK/VRIK). For length mismatch, use one of:
- Keep a minimum elbow bend (about 10°) and stretch the bones slightly (FRIK).
- Translate the arm root ("shoulder dislocation", VRArmIK).
- Use a stretch curve (Final IK).
- Hide the arms altogether (Alyx).

### Cited Findings

**Two-bone solve.**
- The cosine rule gives the elbow inner angle. The remaining DOF is a circle of possible elbow positions around the shoulder→hand axis; "the difficult part … is finding the direction in which the elbow should be oriented". — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- FRIK, following VRIK:
  - Computes the wrist angle A = acos((b² + c² − a²)/(2bc)).
  - Places the elbow at hand + xDir·cos(A)·forearm + yDir·sin(A)·forearm, where yDir is the chosen elbow direction orthogonalized against the hand→shoulder axis.
  - Rotates the upper arm to the elbow, twists it to align the forearm plane, aims the forearm, then sets the hand rotation exactly. — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- In Final IK VRIK the arm is a trigonometric solve with a bend normal from `GetBendNormal`:
  - Start from a "back/down" direction expressed relative to the chest and the target direction.
  - Add the arm's rest direction.
  - Subtract the hand's `wristToPalmAxis` and 0.5 × `palmToThumbAxis`, so the elbow points away from the palm and the thumb.
  - Optionally slerp toward a bend goal, then apply `swivelOffset`. — [IKSolverVRArm.cs (vendored)](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRArm.cs)

**Elbow (swivel) heuristics: Parger / VRArmIK.**
- Paper, step 1. α = α0 + Σi max(0, Xhs,i·wi + bi), with biases b = [30, 120, 65], weights w = [−50, −60, 260] and α0 = 15°, clamped to [13°, 175°]. Xhs is the hand position in shoulder space normalized by arm length. α = 0 means the elbow points along the neck's up vector; increasing α points it outward, then down. The parameters were fitted by observation and error minimization. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- Paper, step 2 (singularity correction). Within 0.5 m of the shoulder's vertical axis, the elbow direction is linearly blended toward a fixed vector vf = [0.133, −0.443, −0.886]. The same is done as the hand moves behind the shoulder (blend range 0–0.1 m on the forward axis). — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- Paper, step 3 (wrist-limit correction). If the hand's yaw in the elbow frame exceeds 45°, the elbow is rotated by c·(θ − 45°)², with c = 1/135. For roll there is a lower threshold of 0° (c = −1/600) and an upper threshold of 90° (c = 1/300). — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- The VRArmIK code defaults differ from the paper and are a tuned implementation:
  - `offsetAngle` 135°, `yWeight` −60.
  - `zWeightTop` 260 / `zWeightBottom` −100 (multiplied by max(0.6 − z, 0)·|y|), `xWeight` −50 with `xDistanceStart` 0.1.
  - Clamp 13–175° with a 10° log soft clamp.
  - Fixed-elbow correction near the shoulder: `startBelowDistance` 0.5, `startBelowY` 0.1, weight 2, local elbow dir (0.3, −1, −2).
  - "Elbow outside" pre-correction: weight −0.5, `startBelowZ` 0.4, `startAboveY` 0.1.
  - Elbow-from-wrist: `handDeltaPow` 1.5, factor −0.3, offset 45°. The forward variant has pow 2, factor 1 and deadzone 0.3; a TODO says values of 0.4–0.6 cause "horrible jumps".
  - Smoothing: lerp with `rotateElbowWithHandDelay` = 0.08 s. — [VRArmIK.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/VRArmIK.cs)

**Elbow (swivel) heuristics: VRIK lineage, from FRIK's port.**
- *Twist source.*
  - twistAngle = asin(handBack.z), where handBack is the hand's −X axis in world space.
  - twistAngle2 = −asin(clamp(handSide.z, −0.599, 0.999)).
  - Blend factor clamp((handBack.z + 0.866)·1.155, 0.45, 0.8), which uses the primary angle more when the wrist points down.
  - Smoothed with 0.75 retention per 90 Hz frame (τ ≈ 39 ms **[computed]**). — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- *Pose-dependent window* (units are Bethesda units, scaled by `size` = 1). These terms widen the window when the hand is behind the body and close it when the arms cross or the hands lift:
  - behindAmount = clamp(distance the hand is behind a plane 10 u behind the shoulder ÷ 40 u, 0, 1).
  - armCrossAmount = clamp((distance past a plane rotated 135° from forward, offset 16 u) ÷ 20 u, 0, 1).
  - armLiftLimit = clamp((chestZ + 60 u − handZ)/60 u, 0, 1), and upLimit = clamp((1 − armLiftLimit)·1.4, 0, 1).
  - adjustMin = smoothMax(behind, smoothMin(cross, liftLimit)), using a 0.1 band.
  - twistMin = −85° + 50°·adjustMin.
  - twistMax = 55° − smoothMax(90°·cross, 70°·upLimit), with a 9° band, and always at least twistMin + 15° (`fArmTwistWindowMinDeg`).
  - The twist (−90..90°) is mapped linearly into [twistMin, twistMax]. That angle rotates body-forward about the sideways axis to give bendDownDir. — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- *Final elbow direction.*
  - Rotate bendDownDir in the horizontal plane by ∓(150° − 25°·armTwist − elbowsTwistForward).
  - elbowsTwistForward = smoothMax(acrossAmount·90°, handBehindHead·120°).
  - acrossAmount is how far the hand crosses a sagittal plane 8 u from the shoulder, ÷ 16 u (scaled by 0.2 when negative).
  - handBehindHead = clamp(behindDist/15 u)·clamp(1.2·upLimit).
  - armTwist = clamp(dot(handSide, norm(side + 0.5·forward)) − max(0, across + 0.25), 0, 1). — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- FRIK v0.79.1 added smooth blends (polynomial smoothMax/smoothMin) at the heuristic switch points "instead of the elbow stepping when a pose crosses a threshold". It also made smoothing frame-rate independent: "the original factors were per frame, so reprojection doubled the lag". dt is clamped to 3 nominal frames. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)

**Shoulder / clavicle.**
- Parger. The shoulder rotates toward the hand by θs = c·(((Xh − Xs,n)·f̂n)/(lu + ll) − d), with c = 30 and d = 0.5, clamped to 0–33°. The same is done for roll using the up vector. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- VRArmIK clavicle limits: forward 33°, backward 0°, upward 33°, multiplier 30. The backward branch uses (ratio + 0.08)·300. — [VRArmIK ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)
- FRIK/VRIK use a "very simple" clavicle model:
  - adjust = clamp(|shoulder→hand| − 0.5L, 0, 0.85L)/(0.85L).
  - The shoulder joint is moved toward the hand by adjust·L·fraction, where fraction is 0.08 in the original FRIK, 0.225 in Skyrim VRIK and 0.15 in the FRIK default.
  - Downward movement is scaled by 0.4 (Skyrim VRIK and FRIK default; original 1.0).
  - The clavicle is then aimed at that point. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- Final IK VRIK "YawPitch" shoulder mode:
  - Yaw is measured in a chest-space frame offset by ±(90° − 45°).
  - Yaw is scaled by (1 − |dot(dir, up)|) and damped into the range −20..50° (left) or −50..20° (right), with a 0.7 damper.
  - Pitch uses an offset of −30° and is damped to ±45°.
  - `shoulderTwistWeight` twists the shoulders backward as the arms rise. — [IKSolverVRArm.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRArm.cs)

**Arm length mismatch: stretch vs not reaching.**
- FRIK `fArmElbowMinFlexionDeg` = 10° (try 6–14°). The elbow keeps at least this bend at full reach, and the bones stretch so the hand is still reached. This stops the elbow spinning when the arm is straight, which is the pose an aimed weapon sits in.
- FRIK `fArmElbowMaxFlexionDeg` = 145°. Near the shoulder the elbow bends at most this much, and the upper arm shortens smoothly instead of flipping.
- The older FRIK behaviour split the excess stretch between the bones in proportion to their lengths, plus a 0.1 slack.
- If the target is more than 2.25× arm length away, or more than 200 u, FRIK treats it as tracking loss and doesn't solve the arm. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- Final IK `stretchCurve` maps the target distance (as a multiple of arm length, from 1 to 2) to a stretch amount. RootMotion recommends starting the stretch slightly before full extension to reduce elbow snapping, and copying the Pilot character's curve. `armLengthMlp` ranges 0.01–2. — [VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html); [IKSolverVRArm.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRArm.cs)
- VRArmIK "shoulder dislocation": once the hand is beyond (1 − 0.005)·armLength from the shoulder anchor, the whole arm root is translated toward the hand by the excess. — [VRArmIK ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)
- Early-beta Skyrim VRIK testers reported the forearm stretching instead of the elbow and upper arm moving, and the head seeming too low. The developer acknowledged that the body position relative to the HMD "had never worked well". — [r/skyrimvr VRIK beta thread (mirror, via search summary)](https://reddit.birdcat.cafe/r/skyrimvr/comments/aqdex5/vrik_v01_beta_release)
- Boneworks players complained that arms "stretch far out", get stuck on objects, and that the body is too flexible. — [Steam: Boneworks thread](https://steamcommunity.com/app/823500/discussions/0/1740009778259159938/)

**Wrist / forearm twist distribution.**
- FRIK spreads the forearm roll over ForeArm1/2/3 with weights 0 / 0.5 / 0.5. A taper of 0.1 / 0.35 / 0.55 "twists the skin more evenly". The upper-arm roll can be counter-rotated on UpperTwist1 (Skyrim VRIK uses 0.7; FRIK leaves it off because it looked worse by eye). The hand is then set exactly to the controller rotation. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)
- VRArmIK clamps the hand-up-vs-forearm angle to ±90°, then applies 0.3× of it on wrist1 and 0.8× on wrist2. — [VRArmIK.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/VRArmIK.cs)

**Hand smoothing.**
- FRIK `DampenHands` uses rotation and translation retention 0.6 per 90 Hz frame (allowed range 0–0.95). That is τ ≈ 22 ms **[computed]**. Player locomotion is subtracted before smoothing so the hands don't lag behind movement. Inside a scope the default is 0.2. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)

**Accuracy evidence.**
- Elbow RMSE against mocap, with Parger estimating the entire chain:
  - Parger: 4.6 cm (easy dataset) to 21.7 cm (hard).
  - FABRIK: 15.7–34.3 cm.
  - Final IK, given ground-truth shoulders: 6.2–17.9 cm.
- Parger's shoulder error was 3.9–13.3 cm and its neck error 3.4–9.4 cm. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)

### Inferences
- For a game-agnostic solver, a pragmatic arm design is:
  - Analytic two-bone solve.
  - FRIK-style min/max elbow flexion (10°/145°) to kill straight-arm spin and near-shoulder flips.
  - Swivel from Parger's hand-position model, blended with VRIK/FRIK wrist-twist windowing.
  - Smoothing on the swivel only, with τ ≈ 40–80 ms. Never smooth the hand position itself beyond about 20 ms, because the hand must remain 1:1.
  - Clavicle reach of 15–22% of arm length past 50% extension, with the downward component damped to 0.4.
- Hand-orientation cues are the most informative extra signal for the elbow. Both Final IK (palm and thumb axes) and FRIK (hand back and side vectors) rely on them, so per-controller grip-pose calibration of the hand frame matters a lot.

### Gaps
- No source gave a human-validated "best" swivel weighting for modern controllers. The *Dead and Buried* weights are not published, and FRIK's are in Bethesda units tuned by eye.
- Parts of Parger's formulas were garbled in PDF extraction (signs and subscripts). The values above follow the extracted text and the VRArmIK code, but verify them against the paper's typeset equations.

---

## 4. Legs: procedural stepping vs animation blending, crouch/kneel, ground alignment

### Takeaway
Two workable designs exist:
- **Procedural footsteps.** VRIK procedural, from *Dead and Buried*, and FRIK. Best for room-scale shuffling and turning in place.
- **Locomotion blend tree driven by HMD velocity plus offset.** VRIK "Animated" mode. Best for thumbstick locomotion and roomscale walking.

RootMotion itself says procedural stepping is not designed for thumbstick locomotion. Crouch and kneel come from the pelvis following the head down, with leg IK keeping the feet planted. Head-too-high is handled by keeping the feet planted and lifting onto the toes, not by floating the feet.

### Cited Findings

**VRIK procedural (legacy).**
- Defaults:
  - `footDistance` 0.3: each foot's step target is ±0.3 m laterally from the predicted CoM.
  - `stepThreshold` 0.4 m, `angleThreshold` 60°, `comAngleMlp` 1.
  - `maxVelocity` 0.4 and `velocityFactor` 0.4.
  - `maxLegStretch` 1 (range 0.9–1).
  - `rootSpeed` 20, `stepSpeed` 3.
  - `maxBodyYOffset` 0.05 m ("reduce this value if locomotion makes the head bob too much").
  - `relaxLegTwistMinAngle` 20° and `relaxLegTwistSpeed` 400°/s.
  - `stepInterpolation` InOutSine.
  - Hidden blocking raycasts with radius 0.2 and height 0.2. — [IKSolverVRLocomotion.cs (vendored)](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion.cs)
- The algorithm, per the source:
  - CoM = (pelvis·1 + head·1 + leftHand·0.2 + rightHand·0.2)/2.4.
  - CoM velocity is clamped to 0.4 m/s, multiplied by 0.4 and added for prediction.
  - The CoP is the midpoint of the footsteps, and comAngle = angle(CoM_pred − CoP, up)·comAngleMlp.
  - Candidate step target = the CoM projected to the ground, plus the lateral foot offset.
  - A forced "length step" toward under-pelvis happens when the thigh-to-footstep distance reaches legLength·maxLegStretch.
  - The steps are checked for leg crossing with a 0.25 m sphere.
  - Effective threshold sT = lerp(stepThreshold, 0.1·stepThreshold, comAngle·0.015), halved for length steps and ×0.9 for the left foot (a tie-break bias).
  - A step is taken if the distance exceeds sT or the foot-root angle exceeds 60°.
  - A new step cannot start until the other foot's progress is ≥ 0.8.
  - Step speed is random in [3, 4.5]/s of normalized progress, so a step lasts ≈ 0.22–0.33 s.
  - While stepping, the target re-tracks at lerp 10/s.
  - Foot lift and heel lift come from curves over progress. — [IKSolverVRLocomotion.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion.cs); [IKSolverVRFootstep.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRFootstep.cs)
- `plantFeet` (default true) keeps the toes planted even when the head is out of reach. This can make the camera exit the head, and it costs extra leg solves. — [IKSolverVR.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVR.cs)
- Leg bend plane: `bendToTargetWeight` 0.5 slerps between the pelvis-locked plane (0) and the foot's forward (1). — [IKSolverVRLeg.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLeg.cs)

**VRIK Animated mode.**
- Defaults:
  - `moveThreshold` 0.3 m. The measure is horizontal HMD offset from the root plus HMD velocity; hysteresis ×0.9 while moving, and a stop timer of 0.05 s.
  - `animationSmoothTime` 0.1 s (0.2 s while turning in place).
  - Animation speed 0.2–3×, SmoothDamp 0.05 s.
  - Root lerp speeds 30 (moving), 10 (stopping), 10 (turning).
  - `maxRootOffset` 0.5 m.
  - `maxRootAngleMoving` 10° and `maxRootAngleStanding` 90°.
  - Turn dead zone 18°.
- The blend trees need "Compute Positions = Velocity XZ". — [IKSolverVRLocomotion_Animated.cs](https://github.com/whx-prog/The-Seed-Link-Future/blob/HEAD/The%20Seed(2022)/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion_Animated.cs); [VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html)

**FRIK procedural walk** (driven by camera world velocity, so it covers both thumbstick and roomscale). The state machine:
- **Start.** Start walking when speed ≥ 35 u/s **[est. ≈0.5 m/s]**. The first foot is chosen at random.
- **Stop.** Begin stopping below 20 u/s **[est. ≈0.29 m/s]**.
- **Speed.** Speed is clamped to 350 u/s and averaged with the previous frame above 20.
- **Deceleration.** A deceleration of more than 20 u/s per frame triggers a short re-target.
- **Step time.** stepTime = clamp(cos(speed/140), 0.28, 0.50) s.
- **Step length.** speed·stepTime·1.5, capped at 140 u **[est. ≈2 m]**.
- **Re-plan.** A re-plan happens when dot(dir, stepDir) < 0.9 (≈26°), with a 2-frame delay.
- **Lift.** sin(π·t)·max(9·clamp(len/150), 1) u **[est. max ≈13 cm]**.
- **Spine sway.** Spine sway is ±3°·sin(π·t).
- **Feet spacing.** The feet are pulled together by 30% each, so the separation drops to 40%. In power armour they are spread apart instead (−0.15).
- **Airborne.** Jumping or being in the air resets to standing.
- **Leg IK.** A VRIK-derived two-bone leg IK uses a hip-frame pole. When the target is too far, it stretches the bones in proportion plus 0.1 slack. When impossible, it falls back to equal bone lengths. — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- FRIK also smooths vertical camera stutter (SmoothMovementVR): `SmoothAmount` 5, `Damping` 1, horizontal 5 / 1, `StoppingMultiplier` 0.5 / 0.6. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini)

**Engine step settings for comparison.** UE's Locomotor plugin `StepSettings` has StepHeight 6 cm at the peak of the largest stride, and `percent_of_stride_in_air` 0.35 (search summary). — [UE Python API: StepSettings](https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/StepSettings)

**Crouch, kneel, seated.**
- VRIK `minHeadHeight` 0.8 m. With legs fixed, the pelvis drops and moves back (`moveBodyBackWhenCrouching` 0.5). — [IKSolverVRSpine.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs)
- Meta Movement SDK: the calibrated height lets the system distinguish standing, squatting and sitting. A synthesized standing pose can make a seated user appear standing. — [Meta docs: Movement SDK body tracking (Unity)](https://developers.meta.com/horizon/documentation/unity/move-body-tracking/)
- Generative Legs can't represent individual leg moves (for example karate or breakdance) or crossed legs. — [Road to VR](https://www.roadtovr.com/quest-3-dev-tools-body-tracking-avatars/); [Meta blog](https://developers.meta.com/horizon/blog/inside-out-body-tracking-and-generative-legs)
- Boneworks used a physics "foot-ball" sphere instead of legs. Marrow 2 replaces the ball with physical legs. — [Laatsch on X](https://x.com/BrandonJLa/status/2009741997897044046); [Boneworks fandom wiki](https://boneworks.fandom.com/wiki/1Marrow_Interaction_Engine)

### Inferences
- A runtime solver should support both modes:
  - **Stationary/roomscale shuffles** → VRIK-style balance-triggered steps (CoM angle and distance thresholds of 0.3–0.4 m, a 60° rotation trigger, 0.22–0.33 s steps, and a 0.25 m leg-crossing guard).
  - **Artificial locomotion** → a velocity-driven gait (a blend tree, or a FRIK-like phase generator). Root yaw should be tightly locked to travel direction (about 10°) while moving.
- Ground alignment isn't covered by these 3-point solvers beyond a footstep raycast (VRIK `blockingEnabled`). Add per-foot raycasts and toe/heel alignment, and clamp the pelvis using the lower foot (VRIK's LimitPelvisPosition idea).

### Gaps
- I found no published numbers for Boneworks or Bonelab leg behaviour, or for how commercial games decide crouch vs kneel.
- I found no direct source for VRIK's default stepHeight and heelHeight curve keys; they are set in the inspector or demo prefabs.

---

## 5. Calibration: height, arm span, T/A-pose, scale avatar vs world, seated play

### Takeaway
Calibration should measure two things: standing eye height (by "stand straight", or an automatic window) and arm span (by a T-pose). Then pick one of two strategies:
- Scale the avatar to the user. This is VRIK's default, and VRChat's measure-by-height.
- Keep the avatar fixed and scale the world or offsets. This is FRIK's world-scale plus offsets approach.

Arm-span scaling fits upper-body accuracy; height scaling fits full-body tracking (VRChat finding). Seated play needs a separate offset set, or a pelvis target.

### Cited Findings
- Final IK: Basic Head & Hands calibration matches only position and rotation offsets. For size, the root is scaled by (headTarget.y − root.y)/(head.y − root.y). Full-body calibration (VRIKCalibrator) has these settings:
  - `scaleMlp` 1.
  - Tracker forward/up axes.
  - `headOffset` and `handOffset`.
  - Foot forward, inward and heading offsets.
  - Pelvis position and rotation weights of 1. — [VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html); [VRIKCalibrator.cs (vendored)](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/Tools/VRIKCalibrator.cs)
- Parger calibrates with a single standing T-pose. Arm segments are assumed equal. The extracted formula is garbled; the geometric reading is lu = ll = (‖Xh,l − Xh,r‖ − 2ls)/4, with 2ls = 0.31 m. h0 is the standing HMD height. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- VRArmIK calibration sets each arm's length to the distance from the hand to the shoulder anchor at calibration time. — [VRArmIK ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)
- VRChat IK 2.0 calibration options:
  - Avatar measurement by arm span or by height. Height worked better for full-body tracking.
  - `--custom-arm-ratio` default 0.4537 (about 0.415 "may fit better").
  - `--calibration-range` 0.6 m: the tracker search sphere around predicted binding points.
  - `--freeze-tracking-on-disconnect`.
  - Calibration saving. — [VRChat IK 2.0](https://docs.vrchat.com/docs/ik-20-features-and-options)
- Meta Movement SDK calibration:
  - Auto-calibration runs in the first 10 s; the user should ideally be standing.
  - Seated users can extend an arm and draw a circle about 0.3 m across, and height is estimated from wingspan.
  - `SuggestBodyTrackingCalibrationOverride(height_m)` and `ResetBodyTrackingCalibration` override or reset it.
  - Only use the pose when the calibration status is Valid.
  - Skin weights should extend at least about 15 cm above each joint so the mesh can stretch for height adaptation. — [Meta docs: move-body-tracking](https://developers.meta.com/horizon/documentation/unity/move-body-tracking/)
- FRIK keeps world scale and avatar size separate:
  - `fVrScale` 71 is world scale (the same as Fallout4Custom.ini).
  - `PlayerHeight` 120.48 u. Changing it rescales the skeleton and breaks hand-weapon alignment.
  - `armLength` 36 u.
  - Root scale = PlayerHeight / DEFAULT_CAMERA_HEIGHT.
  - Body forward and vertical offsets are set in-game, with separate standing and seated values and a "Play Seated" toggle. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp); [FRIK in-game configuration guide](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/docs/in-game-configuration-guide.md)
- Skyrim VRIK provides a "VRIK Calibration Power" that fully calibrates avatar height. — [Nexus VRIK (search summary)](https://nexusmods.com/skyrimspecialedition/mods/23416)
- The Final IK pelvis target is recommended for seated rigs. — [VRIK docs](http://www.root-motion.com/finalikdox/html/page16.html)

### Inferences
- A two-step calibration is enough for 3-point bodies:
  1. Stand straight and look ahead → h0 (eye height). Derive stature with anthropometric ratios; eye height ≈ 0.93–0.94 of stature is a typical anthropometric value **[unverified estimate]**.
  2. T-pose → arm span → arm-segment lengths: (span − shoulder width)/4 per segment, Parger-style.
- Scale the avatar uniformly to match eye height. Then apply `armLengthMlp`-style per-arm length correction, rather than non-uniform skeleton scaling.
- Keep the world at 1:1. FRIK's coupling of world scale to hand-weapon alignment is a cautionary example.
- Seated mode should set a virtual standing h0 (or a pelvis target) so the bend heuristics don't read "seated" as "crouched".

### Gaps
- I found no source for a standard arm-span to height ratio beyond VRChat's 0.4537 arm-ratio constant, and that constant's exact definition is not documented on the page. A Penn State page confirms that Drillis & Contini (1966) ratios are the standard segment-to-stature table, but I didn't retrieve the values.
- How Skyrim VRIK's calibration power computes height (eye height vs head bone) is undocumented in the sources reached.

---

## 6. First-person rendering, failure modes, and what players prefer

### Takeaway
- Hide the head from the player's own camera. Push the head back or scale it to about zero for the wearer only, but keep it in shadows and mirrors.
- Avoid "wrong" limbs above all. Valve, Parger's study and community threads agree that inaccurate arms or legs are worse than none.
- Players split. Many prefer hands-only (Alyx-style) because IK breaks the 1:1 feel, especially in fast action. When well-behaved arms are offered, most users choose them (67–71% in Parger's study).
- Popular mods (VRIK, FRIK) are considered essential by their communities despite quirks such as head spins, stretching and body bounce.

### Cited Findings

**Head and neck hiding, shadows, mirrors.**
- FRIK hides head equipment by default and pushes the head back (`fHeadBackPositionOffset` = 5 u, plus 2·neckPitch) instead of hiding it. Hiding the head "will affect the shadow of the player, making it headless". Hiding geometry doesn't change hit-boxes. Selfie mode can ignore the hide flags and places the body 120 u in front. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [FRIK FAQ](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/docs/faq.md)
- FRIK hides the body while looking through a vanilla scope (`HideBodyInVanillaScope`). Power-armour line-of-sight obstruction is "somewhat intentional"; the advice is to move the body down or the camera up. — [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [FRIK FAQ](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/docs/faq.md)
- VRChat's VRCHeadChop scales the head bone to about zero for the wearer only. It can exempt bones such as hair or a nose, and it doesn't affect mirrors or other players. The docs warn that "Lock Hip" can make the head block the view. — [VRChat Creators: VRC HeadChop](https://creators.vrchat.com/avatars/avatar-components/vrc-headchop)
- In the Meta forum, a developer suggests scaling the head bone to zero and restoring it for third-person views. Others note that this breaks mirrors and reflections, and UE users note it removes the head from render-target cameras. — [Meta forum: avoiding seeing head model](https://communityforums.atmeta.com/discussions/dev-unity/avoiding-see-head-model/282681); [UE forum: first person with third-person shadow](https://forums.unrealengine.com/t/how-do-you-setup-a-first-person-game-but-still-use-the-third-person-mesh-for-shadowing/285114)
- In an Unreal full-body first-person setup, the camera is parented to the head bone and offset forward. Too close, and looking up shows the top of the head and looking down clips into the chest (search summary). — [UE answers (search summary)](https://answers.unrealengine.com/questions/52705/view.html)

**"Body follows wrong way" / spins / floating torso.**
- VRIK Actions exists partly to "fix most VRIK head spins". — [Nexus: VRIK Actions](https://www.nexusmods.com/skyrimspecialedition/mods/69282)
- FRIK's own comments list its yaw-estimation failure cases: hands moving across the chest rotate the body too much, and hands close to the face, or one hand high and one low, are still problems. — [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp)
- Both hands behind the torso make hand-sum yaw flip 180°. Parger disambiguates with HMD yaw; VRArmIK uses a 150–210° jump detector. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf); [ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs)
- Floating torso: early Meta and Microsoft avatars were ridiculed as "legless, waist-up bodies floating". Zuckerberg said the brain must accept a rendering based on how accurately it's positioned, and legs are harder because they're often out of the headset's view. — [AP via TechXplore](https://techxplore.com/news/2022-10-meta-virtual-reality-avatars-legs.amp)
- Feet floating when the head is out of reach is avoided by planting the feet and adjusting the spine and hips (*Dead and Buried* VRIK). — [Meta dev blog](https://developers.meta.com/horizon/blog/developer-perspectives-character-animation-in-dead-and-buried/)

**Player and research preferences.**
- Parger's user study (N = 55, Rift CV1 with OptiTrack):
  - In the "goalie" task, IK arms were preferred over motion-capture arms for embodiment (67%), fast games (69%) and overall (69%). The mocap arms had about 22–33 ms of extra latency. Controllability was 4.93/6 and accuracy 5.11/6.
  - In archery, the overall preference was IK arms 71%, hands-only 23%, mocap 6%.
  - The authors conclude that "ill-behaved arms are considered a strong disturbance, and users prefer not to display arms at all if they cannot reliably be estimated".
  - Earlier work they cite (Steed et al. 2016) found that incorrect poses can decrease embodiment. — [Parger et al. 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf)
- Lugrin et al., IEEE VR 2018 ("Any Body There?"): in an action game, showing more body parts gave no significant improvement in body ownership, immersion or perceived control. — [Lugrin et al. 2018 preprint](https://downloads.hci.informatik.uni-wuerzburg.de/2018-ieeevr-lugrin-igod-preprint.pdf)
- A survey links full-body tracking to stronger presence, but notes that the evidence mostly comes from mocap hardware. — [Caserman et al. 2019 survey](https://www.kom.tu-darmstadt.de/papers/CGG19.pdf)
- Valve: players don't notice missing arms, spectators do, and arms obscure the view. — [VentureBeat](https://venturebeat.com/business/half-life-alyx-ama-everything-we-learned); [Game Informer](https://gameinformer.com/interview/2020/03/23/valve-talks-half-life-alyx-and-why-arms-dont-work-in-vr)
- Community examples:
  - In a Boneworks thread, the original poster "hands down prefer[s] just hands". They found bending over odd, arms often in the wrong position, the body bouncing with arm movement, and the character nearly lifting off the ground when opening doors. Others defended the body as necessary for the physics puzzles. — [Steam: Boneworks](https://steamcommunity.com/app/823500/discussions/0/1740009778259159938/)
  - A Metro Awakening commenter said that without full-body tracking, floating hands are best. — [Steam: Metro Awakening](https://steamcommunity.com/app/2669410/discussions/0/4844274293903327355)
  - A Sairento VR commenter said a game feels "empty" without a body. — [Steam: Sairento VR](https://steamcommunity.com/app/555880/discussions/0/1291816880495317801)
  - Skyrim VR players rate VRIK (with HIGGS and PLANCK) as practically essential, moving the game "from a 7 to a 10". — [Steam: Skyrim VR](https://steamcommunity.com/app/611670/discussions/0/3763353492968498409)
  - A Lone Echo commenter said visible arms make a "HUGE difference" for immersion. — [Steam forum (via search)](https://steamcommunity.com/app/546560/discussions/0/1741140901902265966)
- A Unity forum reply argues that players rarely look down at their own body in games, and that VR players adapt to floating arms. — [Unity discussions](https://discussions.unity.com/t/proper-cinemachine-camera-placement-on-full-body-first-person-model/1590365)

### Inferences
- Ship body visibility as a player option: hands-only, hands+arms, or full body. Players ask for exactly this, and Parger shows a minority (23–27%) still prefer hands-only even with good arms.
- Default to hands+arms, or a full body with conservative legs, in slower and immersive games. Default to hands-only, or let the player opt in, in fast twitch games.
- The single most important rule: the hands must stay exactly at the controller (1:1). The body absorbs all error, and when it can't, it should stretch slightly or fade rather than pull the hand away. Alyx, Parger and the players' complaints all point the same way.
- Use separate render paths for the player camera (head hidden or chopped) and for shadows, mirrors and spectators (full head). FRIK's "push head back" is a cheap compromise when separate render paths aren't available.

### Gaps
- Direct Reddit threads (r/vrgaming, r/skyrimvr) couldn't be fetched (reddit.com is blocked for the fetcher). Community sentiment here comes from Steam forums, search snippets and the studies above.
- I found no survey quantifying how many players disable VRIK or FRIK bodies, or which option players choose when given one in commercial games.

---

## 7. Concrete parameter values (consolidated quick reference)

### Takeaway
The values below are the defaults that shipped solvers actually use. They are a sane starting tuning set for a new runtime solver. Units: m, deg, s, unless noted.

### Cited Findings

**Neck / head / torso**

| Parameter | Value | Source |
|---|---|---|
| Neck-to-head (pivot) distance | 0.13 m | [Parger 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf) |
| Shoulder width (2·ls) | 0.31 m | [Parger 2018](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf) |
| HMD→neck offset (VRArmIK) | 0.03 m along (0, −1, −0.05) HMD-local | [ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs) |
| Neck→shoulder centre (VRArmIK) | (0, −0.10, −0.02) m | [ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs) |
| Neck→shoulder (VRE commented port) | (−0.02, 0, −0.15) m | [VRE PRAC.h](https://github.com/mordentral/VRExpansionPlugin/blob/Master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/ParentRelativeAttachmentComponent.h) |
| Legacy Oculus neck→eye | 0.075 m vertical, 0.0805 m horizontal (forum) | [Meta forum](https://communityforums.atmeta.com/discussions/dev-unity/0-4-3-head-rotation-pivot/321248) |
| Torso yaw clamp vs head | 80° (VRArmIK, VRE port); 90° (Parger); ±50° neck yaw then ×0.7 (FRIK) | [ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs); [Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf); [FRIK](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp) |
| Hands-behind flip detector | yaw jump 150–210° | [ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs) |
| Spine pitch base angle × relative head drop | 135.3° + 0.333·HMD pitch (Parger); (105.3° + 0.1·pitch)/1.2 (FRIK) | [Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf); [FRIK](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp) |
| VRIK bodyPosStiffness / bodyRotStiffness / neckStiffness | 0.55 / 0.1 / 0.2 | [IKSolverVRSpine.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRSpine.cs) |
| VRIK chestClampWeight / headClampWeight | 0.5 (90°) / 0.6 | same |
| VRIK rotateChestByHands | 1 (yaw gain ×5, roll gain ×0.5; sizeMlp = spine/0.7 m) | same |
| VRIK moveBodyBackWhenCrouching / maintainPelvisPosition | 0.5 / 0.2 | same |
| VRIK minHeadHeight | 0.8 m | same |
| VRIK maxRootAngle (procedural) | 25° | same |
| VRIK Animated maxRootAngle moving / standing | 10° / 90° (SmoothDamp 0.2 s) | [Animated.cs](https://github.com/whx-prog/The-Seed-Link-Future/blob/HEAD/The%20Seed(2022)/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion_Animated.cs) |
| VRE body yaw follow | tolerance 0 (always), lerp 100°/s | [VRE PRAC.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/Master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/ParentRelativeAttachmentComponent.cpp) |

**Arms**

| Parameter | Value | Source |
|---|---|---|
| Clavicle rotation (Parger) | starts at 0.5·arm length, gain 30, clamp 0–33° | [Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf) |
| Clavicle reach (FRIK/VRIK) | starts at 0.5 L, full at 1.35 L; shoulder moves 0.15 L (FRIK) / 0.225 L (VRIK) / 0.08 L (orig.); downward ×0.4 | [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini) |
| VRIK shoulder yaw limits | L −20..50°, R −50..20° (offset 45°); pitch ±45° (offset −30°) | [IKSolverVRArm.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRArm.cs) |
| Elbow swivel model (Parger) | α0 15°, b [30, 120, 65], w [−50, −60, 260], clamp 13–175° | [Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf) |
| Elbow singularity blend | within 0.5 m of shoulder vertical axis → vf [0.133, −0.443, −0.886] | [Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf) |
| Wrist-limit elbow correction | yaw > 45°: (θ−45)²/135; roll < 0°: −θ²/600; roll > 90°: (θ−90)²/300 | [Parger](https://arbook.icg.tugraz.at/schmalstieg/Schmalstieg_364.pdf) |
| Elbow min / max flexion (FRIK) | 10° (try 6–14°) / 145° | [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini) |
| Elbow twist window (FRIK/VRIK) | min −85° + 50°·adj; max 55° − max(90°·cross, 70°·lift); width ≥ 15° | [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp) |
| Elbow base outward rotation (FRIK/VRIK) | 150° − 25°·armTwist − max(90°·across, 120°·behindHead) | same |
| Swivel smoothing | 0.75 retention per 90 Hz frame (τ ≈ 39 ms **[computed]**), FRIK; 0.08 s lerp delay, VRArmIK | [FRIK](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp); [VRArmIK.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/VRArmIK.cs) |
| Hand damping (FRIK) | 0.6 retention per 90 Hz frame (τ ≈ 22 ms **[computed]**), 0.2 in scope | [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini) |
| Forearm twist split | 0/0.5/0.5 (FRIK default) or taper 0.1/0.35/0.55; wrist1 0.3, wrist2 0.8 (VRArmIK); upper-twist counter-roll 0.7 (Skyrim VRIK) | [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini); [VRArmIK.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/VRArmIK.cs) |
| Arm stretch / reach cut-off | shoulder dislocation beyond 0.995·L (VRArmIK); tracking-loss bail > 2.25·L (FRIK) | [ShoulderPoser.cs](https://github.com/dabeschte/VRArmIK/blob/master/Assets/Plugins/VRArmIK/Scripts/ShoulderPoser.cs); [FRIK](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp) |

**Legs / locomotion**

| Parameter | Value | Source |
|---|---|---|
| VRIK footDistance / stepThreshold / angleThreshold | 0.3 m / 0.4 m / 60° | [IKSolverVRLocomotion.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion.cs) |
| VRIK CoM weights | pelvis 1, head 1, each hand 0.2 | same |
| VRIK velocity prediction | clamp 0.4 m/s × 0.4 | same |
| VRIK step speed / duration | 3–4.5 /s → 0.22–0.33 s; next step allowed at other foot's progress ≥ 0.8 | same; [IKSolverVRFootstep.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRFootstep.cs) |
| VRIK leg-cross guard | 0.25 m sphere | [IKSolverVRLocomotion.cs](https://github.com/brandonmousseau/vhvr-mod/blob/HEAD/Unity/ValheimVR/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion.cs) |
| VRIK rootSpeed / maxBodyYOffset | 20 / 0.05 m | same |
| VRIK leg twist relax | > 20°, 400°/s | same |
| VRIK Animated move threshold | 0.3 m (offset + velocity), ×0.9 hysteresis, 0.05 s stop timer | [Animated.cs](https://github.com/whx-prog/The-Seed-Link-Future/blob/HEAD/The%20Seed(2022)/Assets/Plugins/RootMotion/FinalIK/IK%20Solvers/IKSolverVRLocomotion_Animated.cs) |
| VRIK Animated smoothing / root lerp / max offset | 0.1 s (0.2 s turning) / 30-10-10 / 0.5 m | same |
| VRIK Animated turn dead zone | 18° | same |
| FRIK walk start / stop speed | 35 / 20 u/s **[est. 0.5 / 0.29 m/s]** | [FRIK Skeleton.cpp](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/src/skeleton/Skeleton.cpp) |
| FRIK step time | clamp(cos(v/140), 0.28, 0.50) s | same |
| FRIK step length / lift | v·t·1.5 (≤ 140 u **[est. 2 m]**) / ≤ 9 u **[est. 13 cm]** | same |
| FRIK re-plan angle | dot < 0.9 (≈ 26°) | same |
| UE Locomotor StepHeight / air fraction | 6 cm / 0.35 | [UE StepSettings](https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/StepSettings) |

**Calibration**

| Parameter | Value | Source |
|---|---|---|
| VRChat custom arm ratio | 0.4537 default (≈ 0.415 suggested) | [VRChat IK 2.0](https://docs.vrchat.com/docs/ik-20-features-and-options) |
| VRChat tracker calibration range | 0.6 m | same |
| Meta auto-calibration window | first 10 s (standing); seated: 0.3 m arm circle | [Meta docs](https://developers.meta.com/horizon/documentation/unity/move-body-tracking/) |
| Meta body skeleton | 84 bones (70 upper + 14 lower) | same |
| FRIK PlayerHeight / armLength / world scale | 120.48 u / 36 u / fVrScale 71 | [FRIK.ini](https://github.com/rollingrock/Fallout-4-VR-Body/blob/main/data/config/FRIK.ini) |
| FRIK standing vs seated HMD offset | 8.6 u vs 35 u | same |

### Inferences
- Bethesda-unit thresholds in FRIK should be re-derived in metres before reuse. My conversions assume about 70 u/m, and FRIK also scales the skeleton by PlayerHeight/DEFAULT_CAMERA_HEIGHT.
- The "retention per 90 Hz frame" smoothing constants should be implemented as τ-based exponential smoothing (α = 1 − exp(−dt/τ)), with dt clamped. FRIK's own fix shows that per-frame factors double the lag under reprojection.

### Gaps
- There are no published, human-validated defaults for neck offset or torso-yaw blend weights on current headsets.
- VRIK's default stepHeight and heelHeight curves, and its default arm stretchCurve keys, aren't in source defaults; they ship in demo prefabs I could not access.
- Bethesda unit-to-metre conversion was not verified in this research.
