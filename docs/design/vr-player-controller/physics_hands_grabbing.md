# Physics-driven hands and object interaction in VR games (grab, hold, throw, force pull)

> A research snapshot (2026-10-21), not a description of the framework's code: what was built is in
> `docs/`.

Scope note: these notes favor primary, inspectable sources. The best ones turned out to be open-source or documented interaction frameworks with real default values: HIGGS (the Skyrim VR mod, full C++ source on GitHub), the SteamVR Interaction System (Valve's Unity plugin source), Godot XR Tools (source), the LÖVR hand-physics example, and the docs for HurricaneVR, Auto Hand, Unity XRI and Meta. Valve and Stress Level Zero have published almost no numeric tuning values, so the commercial-game material is qualitative and comes from interviews, reviews and forums. Units are metres, kilograms and seconds unless stated otherwise. HIGGS runs in Havok at Skyrim's world scale, so its values are roughly metres where commented. Anything I computed or estimated is marked as an inference.

---

## 1. Physics hands vs kinematic hands: what acclaimed games do, and what players prefer

### Takeaway
There is a spectrum. At one end, kinematic hands copy the controller pose and pass through the world (common in early VR, H3VR-style, and I Expect You To Die). In the middle, hands collide with the world but are tuned to "just work": Half-Life: Alyx is the case study, with world collision, latching onto levers and cranks, assisted throws and gravity gloves. At the far end is a fully simulated, physically driven body (Boneworks/Bonelab). Players praise full physics for emergent freedom and weight, and criticize it for jank, objects snagging, and fatigue. Valve deliberately avoided Boneworks-style heavy physics because controllers give no force feedback.

### Cited Findings
**Half-Life: Alyx (Valve)**
- Valve's Jason Mitchell explained why Alyx avoided Boneworks-style physics and melee: "In Alyx if we have the player hold something large and rigid there's a lot that's gonna happen in a physical simulation… that's not gonna feed back into your hand." He added, "it's not a strength of VR systems now, so we decided not to lean into it." He also said melee ("bludgeoning and impaling… even… tapping a tool on a desk") lacks force feedback. — [UploadVR](https://www.uploadvr.com/half-life-alyx-boneworks-physics/)
- Alyx's hands collide with the environment, so players can lean on walls, push doors open, and brush tables. On levers, knobs and cranks the virtual hand latches onto the object until release, which means it is briefly out of sync with the real hand (technique credited to Job Simulator). The lesson stated: "we have to know when to show where the players real hands are, and when to show where their hands should be." — [Transform Interactive (third-party dev blog)](https://transforminteractive.com/what-half-life-alyx-taught-us/)
- Held weapons in Alyx do not clip through walls or objects. "Almost anything you can touch with your hand you can also touch with your weapons," including pushing doors and swatting objects. A held tool also works as an "immersion insulator" that explains why you can't feel what you touch. — [Road to VR, Inside XR Design p.2](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2/)
- Alyx objects have weight through inertia, but the throw calculation ignores the inertia sway. Ignoring it "only in its throwing calculation" makes throws easier without losing the sense of weight. — [Road to VR, Inside XR Design p.1](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/)
- Players still report spots where a hand passes through geometry and where items pass through the head. — [Steam forum "Physics" thread](https://steamcommunity.com/app/546560/discussions/0/2137462524945044561/)
- Doors in Alyx open both ways, because players can't feel whether to push or pull. Kerry Davis: getting through matters more than faithful door mechanics. — [Engadget](https://www.engadget.com/2020-03-31-half-life-alyx-valve-interview-vr.html)

**Boneworks / Bonelab (Stress Level Zero)**
- Boneworks uses a fully simulated body driven by IK from head and hand positions. The world can affect and obstruct that body. Objects respond according to size, weight and material: cups and hammers work one-handed, while crates and barrels "take more effort." — [Wikipedia](https://en.wikipedia.org/wiki/Boneworks)
- Brandon Laatsch's (SLZ) design rule: "If something looks heavy, it is," and "realistic mass for the objects is necessary to give the end result the expected look." Releasing the index finger loosens the virtual grip so the player can slide or rotate the hand along a handle without letting go. Thrown objects can be "force" grabbed back "by making a fist with your index and middle fingers." Steadying a gun with the second hand "just worked." — [UploadVR hands-on](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)
- The same article: "With floating hands alone, if they stick to a heavy object in space, it feels jarring." This is the argument for a connected full body/arms. — [UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)
- Heavy items such as a metal barrel or a two-handed sledgehammer must be handled as heavy. A tutorial sign tells players to mime the lift. Laatsch admits it gets tiring: "Eventually playing just gets way too tiring for long sessions." — [GamesBeat](https://gamesbeat.com/how-boneworks-turns-your-body-into-its-key-mechanic/)
- Player sentiment, mostly from 2019–2020 Steam forums: Boneworks is praised for small physics details (pushing enemies, melee). Some players complain about "objects sticking to hands and catching on everything," and one calls it "janky as hell" but more fun. Others prefer Alyx's floating hands, arguing that a full body moving in ways the player didn't move breaks immersion. — [Steam: "This vs. bone works"](https://steamcommunity.com/app/546560/discussions/0/1861616237339476882/); [Steam: Boneworks "Comparing… isn't Apples to Apples"](https://steamcommunity.com/app/823500/discussions/0/2975149180225046820/); [Steam: "floating hands is better than full body"](https://steamcommunity.com/app/546560/discussions/0/1657817111852541560/) (only search-result summaries were seen, so per-quote attribution between threads is uncertain)

**Lone Echo (Ready At Dawn)**
- Lone Echo uses a "smart procedural posing system which adapts the grip animation to each object and surface based on a physical model." Every grip has a unique finger arrangement, and "even after you grab, the grip pose is dynamic and continues to shift based on your arm position." Arm IK is called among the best to date. — [Road to VR](https://roadtovr.com/lone-echos-virtual-hands-unassuming-vr-innovation/)
- GDC 2017 talk by Jacob Copenhaver, "It's All in the Hands: VR Animation and Locomotion Systems in 'Lone Echo'": procedurally generated hand animations for grabbing arbitrary surfaces, plus a constraint system that drives all player locomotion. — [GDC Vault abstract](https://gdcvault.com/play/1024240/It-s-All-in-the)

**Skyrim VR mods: HIGGS and PLANCK (adamhynek / FlyingParticle)**
- HIGGS "Adds native VR mechanics and enhanced physics to Skyrim VR," with a C++ API for other mods. — [GitHub adamhynek/higgs](https://github.com/adamhynek/higgs)
- What HIGGS adds, from its config and source: a physical hand collision box (half-extents 0.05 × 0.015 × 0.09 m), grabbing any physics object at the point where the hand actually touches it (geometry-based), gravity-glove pull and looting, weapon two-handing, shoulder-stash and mouth-consume gestures, movement slowed by held mass, collision haptics, and procedural finger curl. — [HIGGS config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [HIGGS config.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/config.cpp)
- PLANCK ("Physical Animation and Character Kinetics") makes NPC ragdolls physically present, so you can grab, slap and throw things at them. Melee hits only register on collision with the ragdoll, using the weapon at its real size (vanilla scales the weapon to 1.75× for hit detection). — [TheGamer](https://www.thegamer.com/skyrim-vr-slap-npcs-planck-mod/); [80.lv](https://80.lv/articles/skyrim-vr-ragdoll-physics-mod-released)
- Division of labor: "HIGGS handles hands colliding with moveable items and grabbing them," but in HIGGS alone hands and weapons do not collide with NPCs. PLANCK adds that collision. It requires HIGGS. — [Steam discussion](https://steamcommunity.com/app/611670/discussions/0/4038104984936444224); [Nexus PLANCK](https://nexusmods.com/skyrimspecialedition/mods/66025)

**Non-colliding hands (counter-example)**
- I Expect You To Die (Schell Games): hands have no colliders, only trigger volumes for grabbing, so they clip through geometry. Held objects first collided with the environment and dropped when collisions exceeded a threshold, but as levels grew more complex "it made object interaction frustrating." — [Schell Games blog (search snippet; page returned HTTP 500 when fetched)](https://legacymicrosites.schellgames.com/blog/engineering-a-super-spy-in-i-expect-you-to-die)
- Meta's guidance frames the same trade-off. Kinematic grabs: "The interactable follows the hand exactly and can pass through other virtual objects." Physics grabs give mass and velocity and may feel "heavy" if the hand moves faster than the physics can follow. — [Meta Horizon grab design](https://developers.meta.com/horizon/design/grab_usage/)

### Inferences
- Valve's position (no force feedback, so don't simulate heavy rigid coupling into the hand) and the Boneworks complaints (snagging, fatigue) point to the same design: keep the hand physical against the world, but cheat in the player's favor wherever there is no haptic channel. Alyx's own cheats are latching onto mechanisms, throws that ignore inertia sway, two-way doors and gravity gloves.
- A game-agnostic system should expose "physicality" as per-object and per-mode tuning (spring, damper, max force; one-hand vs two-hand strength) instead of baking in one philosophy. Weight is subjective, so the player should be able to tune the spring and damper; HurricaneVR's per-grabbable one-hand/two-hand overrides (Section 2) show per-object tuning works.
- Simulated mass reads as weight: a fast swing outruns the weapon, which lags the real hand on purpose, and heavy weapons accelerate slowly and are hard to stop or redirect. Weapons meant for two hands (claymores, spears, polearms) should be weak and imprecise in one.
- The full-body-plus-physics approach (Boneworks) delivers the most emergent play but carries the highest polish cost. The common failure modes are hands snagging on geometry, objects sticking, and the body getting pushed around. Collision filtering, softening and teleport fallbacks (Sections 2 and 4) mitigate them.

### Gaps
- No primary source found for H3VR's or Into the Radius's hand implementation (whether hands collide, how held objects behave). Searches returned nothing specific.
- Stress Level Zero has not published its physics-rig internals (joint types, gains, the Marrow engine). Only qualitative interview material is available.
- No systematic survey of player preferences was found. The sentiment above is forum- and review-based. Reddit threads were not reachable through search.

---

## 2. How hands follow controllers (PD/joint/velocity tracking, gains, heavy objects, ghost hands, snap-back)

### Takeaway
Three follow methods are common: (a) direct kinematic copy (no collision response); (b) velocity tracking, where each physics step sets the body's linear and angular velocity to close the error, with clamping; and (c) a PD/spring-damper joint or force drive from a kinematic "target" body to a dynamic hand body, with a max-force cap. All the robust frameworks add a separation threshold that teleports the hand back (or drops the object) when it gets too far from the controller: 0.7 m in HIGGS, 1.0 m in Godot XR Tools, configurable in HurricaneVR and Auto Hand. For heavy objects, frameworks scale hand strength by mass, use stronger two-hand settings, or slow the hand by held weight.

### Cited Findings
**PD / joint drive**
- HurricaneVR: "The hands are driven using a joint based PD controller," built on Unity's ConfigurableJoint. Spring is P, Damper is D, and Max Force/Torque caps the output. Tuning advice: tune Spring first for responsiveness and ignore overshoot, then raise Damper until overshoot stops, then set Max Force last for strength. "Dividing the Spring value by 10 is a good starting point for tuning the Damper." Position (force) and rotation (torque) PD values can be overridden separately. — [HVR docs: PD Strength](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/pdstrength.md)
- HurricaneVR: HVRJointSettings sets the default hand strength. Grabbables can override the hand-to-object joint and "The strength of the hand (one or two hand overrides)." "One Hand Strength will take affect when only one hand is grabbing this object. Two Hand Strength… when both hands are grabbing," and HVRTwoHandStrength links grabbables so they count as two-hand. — [HVR docs: Joint Settings](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/jointsettings.md); [HVR docs: PD Strength](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/pdstrength.md)
- HurricaneVR: HVRRigidBodyOverrides zeroes the inertia tensor rotation for smooth rotation on all axes, and sets the inertia tensor to a value matched to the default joint settings. Adjusting it changes the torque needed to rotate the hand. — [HVR docs: Physics Hands](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/physicshands.md)
- Auto Hand: "The hand will follow the given transform and match its position/rotation using rigidbody forces." It recommends an empty follow target parented under the controller so the hand can be offset. — [Auto Hand docs](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/hand)
- Amebous Labs (Pierce McBride): a PD-style physics hand where a position strength of 15 works well with a hand mass of 1 and must be retuned for other masses. — [Amebous Labs (search snippet; page returned HTTP 403)](https://amebouslabs.com/developing-physics-based-vr-hands-in-unity/)

**Force drive, minimal example (LÖVR)**
- LÖVR's hand-physics example uses a dynamic box collider per hand, sized 0.04 × 0.08 × 0.08 m with mass 0.5, pushed by `applyForce(delta * dt * hand_force)` with `hand_force = 30000`. Torque comes from the angle-axis of `handOrientation * colliderOrientation:conjugate()`, wrapped to (−π, π], applied as `axis * angle * dt * hand_torque` with `hand_torque = 20`. Linear damping is 0.7 and angular damping 0.9. — [LÖVR docs: Hand Physics](https://lovr.org/docs/v0.19.0/Physics/Hand_Physics)
- Why the hands are dynamic rather than kinematic (same source): a kinematic hand has its velocity set directly and can't be pushed back by contacts, so squeezing an object between two hands can't resolve and the physics glitches. The example also notes hands can get stuck or buried under objects when pushed hard. Its workaround is a "ghost" mode: the trigger toggles the hand collider between solid and sensor (`setSensor(not solid)`). — [LÖVR docs](https://lovr.org/docs/v0.19.0/Physics/Hand_Physics)

**Velocity tracking (Unity XRI held objects)**
- XRI's "Velocity Tracking" movement type moves the object by setting its Rigidbody velocity and angular velocity. It can lag behind the interactor and has Velocity Damping/Scale and Angular Velocity Damping/Scale settings. "Kinematic" moves a kinematic Rigidbody in FixedUpdate. "Instantaneous" sets the Transform every frame for minimum latency, but the object can pass through colliders. — [Unity XRI 3.0 docs](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/xr-grab-interactable.html)

**Character-body collide-and-slide hands (Godot XR Tools)**
- XRToolsCollisionHand has modes DISABLED, TELEPORT and COLLIDE. In COLLIDE it rotates and collides toward the controller basis, then `move_and_slide(target_movement)`. If `distance_to(target) > TELEPORT_DISTANCE` (1.0 m), it emits `max_distance_reached` and snaps to the controller. Finger collision shapes are generated from skeleton bones (Proximal/Intermediate/Distal) plus a palm shape. — [godot-xr-tools collision_hand.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/hands/collision_hand.gd)
- Heavy objects in the same file: when holding weight, `force = min_pickup_force + target_movement.length() * (max_pickup_force − min_pickup_force) / TELEPORT_DISTANCE`, with `min_pickup_force = 15 N` and `max_pickup_force = 300 N`. Allowed movement per frame is `delta * force / held_weight`, so heavy objects slow the hand and the force rises the further the controller pulls ahead. — [collision_hand.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/hands/collision_hand.gd)
- Other Godot projects use a CharacterBody3D "hand anchor" with a small sphere and a PinJoint3D linking the visual hand to it. A 2014 Unity forum thread recommends two targets per hand, so the visible hand stops at static colliders while the tracked target keeps moving. — [godot4-vr-physics-template (search snippet)](https://github.com/jtnicholl/godot4-vr-physics-template)

**Skyrim HIGGS: hand and held-object constraint (Havok)**
- Held "physics grab" uses a Havok constraint between the hand body and the object, with pivots at the palm and the grab point. It is used for actors (ragdolls) and constrained objects, or always with `ForcePhysicsGrab`. Other objects are switched to keyframed motion. — [HIGGS hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)
- Constraint motor defaults: linear/angular tau 0.03, damping 0.8, proportional recovery velocity 2.0, constant recovery velocity 1.0, linear max force 2000 (weapons 9000, actors 2500; actor angular max force 40). The "Body" taus (constrained bodies) are linear 0.8 and angular 0.65, starting from 0.01 and lerped up (`physicsGrabLerpTauTimeBody = 0.1 s`, `grabConstraintTauLerpSpeed = 0.5`). "Colliding" taus are 0.01 (linear and angular). `grabConstraintMaxForceToMassRatio = 500` (max force capped at mass × 500). `grabConstraintAngularToLinearForceRatio = 12.5`. Fade-in: `grabConstraintFadeInTime = 0.1 s`, starting angular max-force ratio 100. — [HIGGS config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)
- HIGGS scales actor max force by physics frame rate (`fpsToActorMaxForceMultiplierMapLinear/Angular`) and compensates for player movement during physics grabs (`doPhysicsGrabPlayerMovementCompensation`). — [HIGGS config.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/config.cpp)
- When you grab, the object is lerped into the hand. Lerp time maps the hand-to-grab-point distance [0.1, 0.2] m to [0.1, 0.2] s, clamped. — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)
- Hand max separation: `maxHandDistance = 0.7`. — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Held mass slows locomotion: `slowMovementMassProportion = 0.675`, exponent 1.0, max reduction 75, fade-out 5 s. Jump height is reduced by mass too. — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [config.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/config.cpp)

**Separation and "too far" handling**
- HurricaneVR has a max-distance breach when the hand is more than MaxTargetDistance from the controller, or more than ArmLength from the shoulder Anchor. MaxDistanceBehaviour options are GrabbablePrevents (holding stops the reset), GrabbableDrops (object released, hand collision disabled, hand "smoothly returns to the controller"), and HandSweep (moves hand and object to a safe position between player and controller). It can be overridden per grabbable. — [HVR docs: Physics Hands](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/physicshands.md)
- Auto Hand: "The hand will teleport to the follow position if its distance from the follow target is greater than the max follow distance." A held grabbable is released or brought along depending on its "parent on grab" setting. — [Auto Hand docs](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/hand)

### Inferences
- Recommended baseline for a Rapier runtime (engineering inference, not from a cited game; check API names against current Rapier docs):
  - Make each hand a dynamic body, not kinematic, so contacts can push it back. LÖVR's rationale about squeezing objects applies directly.
  - Drive it by velocity tracking each physics step: `v = clamp((p_target − p)/dt · k_p, v_max)` and `ω = clamp(axis·angle/dt · k_r, ω_max)`, with `k ≈ 0.5–1.0`. Alternatively use a joint/force PD with max force. Velocity tracking is the simplest to keep stable in Rapier and needs no gain-to-mass retuning. A force PD needs gains retuned per hand mass, as Amebous Labs notes.
  - If using PD force: critically damped D ≈ 2·sqrt(K·m). HVR's "Damper ≈ Spring/10" is an empirical starting point for Unity joint units.
  - Teleport the hand to the controller when separation exceeds about 0.5–1.0 m (HIGGS 0.7, Godot 1.0). While teleporting, briefly disable hand collision so it doesn't explode out of geometry. HVR's GrabbableDrops disables hand collision as it returns.
- Ghost hand (inference): show a translucent controller-pose hand only when the physical hand is blocked by more than a few cm. Alyx's "show where the real hand is vs where it should be" principle supports this. My estimate for the visibility threshold is 3–5 cm, which is unsourced.
- Heavy objects (inference): scale effective hand strength by the held mass ratio, or follow Godot and limit hand displacement per step to `force·dt/mass`. For two-hand grips, give the combined body roughly double strength (HVR's two-hand override) so heavy two-handed items feel controllable. HIGGS's force-to-mass cap of 500 (about a 500 m/s² acceleration ceiling, my reading) keeps light objects from being yanked violently while the hand stays strong for heavy ones.
- HIGGS's "colliding tau = 0.01" (stiffness drops sharply when the held object is in contact) is, I infer, how it lets held objects rest against walls without jitter or tunnelling, then re-stiffen on separation. Rapier equivalent: lower the joint motor stiffness or max force while the held collider has active contacts.
- Havok terms (inference about the Havok API): "tau" is roughly a 0–1 per-step stiffness fraction, "damping" 0–1, and "recovery velocities" cap how fast position error is corrected. Map them to Rapier motor stiffness and damping by tuning, not by direct conversion.

### Gaps
- No published PD gains for Boneworks/Bonelab or Alyx.
- HurricaneVR's and Auto Hand's numeric defaults (spring, damper, max force, max follow distance) are not in the public docs. They are only visible in the asset inspector. The Unity ConfigurableJoint-based "VR physics hands the right way" Medium article returned HTTP 403.
- No source states Alyx's exact hand-vs-world collision behavior (for example, whether static geometry stops the hand and how far before it snaps). The Transform Interactive claim is third-party.

---

## 3. Grab system design (grab points, radius, hover, hold vs toggle, pose snapping, two-handed, foregrips)

### Takeaway
Robust systems combine three layers. A near sphere or overlap query picks the closest grabbable within about 5–30 cm. A far cone or spherecast handles distance grab, with the selection then locked. Hysteresis is applied to both the selection and the grip analog. Grabbing is either "anywhere" (Lone Echo, Boneworks, HIGGS: geometric contact point plus procedural finger wrap) or authored grab points with poses (HurricaneVR, Auto Hand, XRI attach transforms). Two-hand grabs use primary and secondary hands: the primary sets position, and the secondary contributes rotation along the hand-to-hand axis.

### Cited Findings
**Selection and radii**
- Godot XR Tools defaults: `grab_distance = 0.3 m` (near sphere), `ranged_enable = true`, `ranged_distance = 5.0 m`, `ranged_angle = 5.0°` (range 0–45°). The near sphere is checked first, nearest by squared distance. The ranged cone is checked only if nothing is near, choosing the best dot product with hand-forward. The cone cylinder has radius `tan(ranged_angle)·ranged_distance` and candidates must pass `dot ≥ cos(ranged_angle)`. The highlight moves to the new closest object each frame. — [function_pickup.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd)
- HIGGS defaults: near spherecast radius 0.05 over 0.1 distance, and far spherecast radius 0.3 out to 5.0. Far candidates are scored by perpendicular distance to the ray and must lie within a 50° half-angle cone of the HMD forward (`requiredCastDotProduct = cos(50°)`), not the hand. `nearbyGrabBodyRadius = 0.1` damps nearby bodies (below 0.2 m/s) at grab time so neighbors don't jostle. Projectiles in flight are skipped. — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)
- HIGGS grab-point scoring weights: `GrabLateralWeight`, `GrabDirectionalWeight`, `grabMaxTriangleDistance`. It can ignore hair geometry, vertex-alpha geometry, blood, decals and soft effects, and it blacklists node names. — [config.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/config.cpp)
- Meta: distance grab uses "a cone-shape projection" with a narrow cone for hovering and a wider cone for unhovering (selection hysteresis). The direct-grab collider should sit precisely (for example, between pinching fingers). Assist options include airgrab, velocity-based and friction-based grabbing. Feedback should persist while in zone and "increase as the selection is committed." — [Meta Horizon grab design](https://developers.meta.com/horizon/design/grab_usage/)
- Auto Hand: the Palm Transform's forward sets where automatic grabs are positioned and where the grab-search cone is projected. Passive highlight copies the mesh, applies the highlight material, and scales it up slightly. — [Auto Hand docs](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/hand)

**Input: hold vs toggle, leeway**
- Godot XR Tools: the grip analog uses hysteresis. Press registers above `threshold + 0.1` and release below `threshold − 0.1`. `press_to_hold = true` by default (hold to keep). A non-press-to-hold object toggles: press to grab, press again to drop. — [function_pickup.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd); [pickable.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/objects/pickable.gd)
- HIGGS uses an input "leeway" state machine (Idle → Leeway → Block/Force). If a trigger press happens on a selected object, the vanilla game input is withheld for `inputLeewayTime = 0.3 s`. If no grab occurred, the press is re-injected (`forceInputTime = 0.03 s`), so grab and game actions can share one button. Release happens on the falling edge of trigger or grip when neither is held. `GripInputMethod` selects the mapping. — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)
- Meta: on controllers, the index trigger suits smaller objects and the grip button bigger ones. With hand tracking, pinch is the thumb–index tip distance and palm grab uses finger curl. A user should be able to "start with a pinch and transition to a palm grab without dropping the interactable." — [Meta Horizon grab design](https://developers.meta.com/horizon/design/grab_usage/)

**Attach and pose**
- Meta: either the object snaps to the hand, or the hand moves to the object to keep the object's world placement. — [Meta Horizon grab design](https://developers.meta.com/horizon/design/grab_usage/)
- XRI: Attach Transform (falls back to the object origin), Secondary Attach Transform for two-handed, "Use Dynamic Attach" (attach where the interactor actually was at grab time, i.e. grab anywhere), and Attach Ease In Time (seconds, 0 = none). — [XRI 3.0 docs](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/xr-grab-interactable.html)
- Auto Hand Grab Type can be "grabs objects," "moves objects to the hand," or "grabs instantly." It has Min/Max Grab Time for the grab animation, a Grab Curve mapping time to pose (0–1), a Pose Index, and Gentle Grab Speed. Fingers have per-knuckle components with a tip radius acting as a "grab bumper." Open/closed poses are saved, and Grip Offset adds bend (0.14 ≈ slight bend, 1 = full). — [Auto Hand docs](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/hand)
- HIGGS procedural finger wrap: each finger's curl comes from intersecting finger rays with nearby triangles, floored at 0.2. Finger animation speeds: grab 4.0 linear / 630 angular, start 2.0 / 135, end 2.0 / 45. — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Lone Echo: procedural, physically based grip posing on arbitrary surfaces, continuously updated with arm position after grabbing. — [Road to VR](https://roadtovr.com/lone-echos-virtual-hands-unassuming-vr-innovation/)
- Boneworks: lifting the index finger loosens the grip so the hand can slide or rotate along a handle. — [UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)

**Two-handed / off-hand**
- Godot XR Tools `second_hand_grab`: IGNORE (default, a second hand can't grab a held object), SWAP (the first hand lets go with zero velocity and the new hand becomes primary), or SECOND (adds a second grab to the driver). When the primary releases and the remaining grab uses a SECONDARY grab point, it switches to a better grab point. — [pickable.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/objects/pickable.gd)
- HIGGS two-handed weapons: the desired transform starts where the main hand wants the weapon. It is then rotated to align with the palm-to-palm axis (`twoHandedHandToHandAlignmentFactor = 1.0`), shifted along that axis (`ShiftFactor = 0.5`), and optionally twisted by the off-hand palm direction (`RotationFactor = 0.5`, scaled by palm orthogonality). Position is effectively snapped (`posSpeed = 9999`) and rotation blended at `twoHandedRotationSnapSpeed = 1200`. Each hand is then placed at its grip offset on the weapon. Grabbing an object held by the other hand hands it off (the other hand's drop events are suppressed). — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- XRI's default grab transformer supports two-handed rotation, and two-handed scaling (off by default). Select Mode "Multiple" allows simultaneous selection by several interactors. — [XRI 3.0 docs](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/xr-grab-interactable.html)

### Inferences
- Default parameters for a game-agnostic system (inference, synthesized from the cited defaults):
  - Near grab radius 5–10 cm from the palm point (HIGGS 5 cm cast; Meta "precise"). An up-to-30 cm sphere (Godot) is forgiving but makes cluttered tables ambiguous. Score by distance plus palm-direction alignment.
  - Far selection: a cone of about 5–10° from the hand-pointing ray, with a wider unhover cone (Meta). HIGGS's 50° HMD cone is a gate, not the selector.
  - Grip analog threshold about 0.5 with ±0.1 hysteresis.
  - Hold-to-grip by default. Offer a toggle option for accessibility and long holds (Godot supports both per object).
- Grab points: support both authored grab points (handles, foregrips, magazines) and fallback grab-anywhere using a closest-point query from the palm against the collider. The pose then comes from a procedural finger-curl solve (raycasts or sphere sweeps per finger, as HIGGS and Auto Hand do).
- Two-handed guns (inference): the primary (grip) hand owns position and roll. The secondary (foregrip) hand defines the aim axis as primary→secondary. Apply a smoothed look-rotation, and let the off-hand grab only designated secondary points. Godot's "switch_active_grab_point" behavior (promote the secondary to primary when the primary releases) avoids the object snapping awkwardly.

### Gaps
- No source found for Alyx's exact near-grab radius, hover highlight logic, or "hand pose snapping" authoring pipeline.
- No numeric grab radii were found for Boneworks/Bonelab or Into the Radius.

---

## 4. Held objects (kinematic vs joint attach, collisions while held, wall penetration, dropping)

### Takeaway
There are two families. Kinematic or frozen attach, where the object is reparented or driven to the hand pose, is low latency but passes through walls unless its collision is copied onto a colliding hand. Joint or velocity attach (constraint, fixed joint with motors, or velocity tracking) collides naturally and conveys weight, but lags and can jitter. Mature systems disable collision between the hand and its held object, lower stiffness when the held object is in contact, copy held colliders onto the hand's collision body, and briefly ignore hand collision on release.

### Cited Findings
- SteamVR Interaction System Throwable defaults: `attachmentFlags = ParentToHand | DetachFromOtherHand | TurnOnKinematic`. In other words, it is a kinematic, parented attach. — [Throwable.cs](https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/Throwable.cs)
- Godot XR Tools pickable: on pick-up the body is frozen (`freeze = true`), moved to a "held" collision layer (layer 17), and given `collision_mask = 0`. A GrabDriver then positions it. The pickup copies each enabled CollisionShape3D of the held RigidBody onto the hand's collision body and keeps the copies aligned every frame, so the colliding hand carries the object's shape against the world. On release, freeze state, layer and mask are restored and the velocities applied. — [pickable.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/objects/pickable.gd); [function_pickup.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd)
- HIGGS: free physics objects become keyframed, while ragdolls and constrained objects use a constraint. The hand collision is disabled (Havok collision-off bit) while in HeldBody or two-handing, and re-enabled after `handWeaponCollisionEnableDelay`. On release below the throw threshold the object is allowed to collide with the hand again. Above it, hand collision is ignored for `throwIgnoreHandCollisionTime = 0.1 s`. — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- HIGGS clamps connected-body inertia when grabbing (`grabbedObjectMaxInertiaRatio`, `grabbedObjectMinInertia`) and sets a minimum colliding clutter mass (`minCollideClutterMass`). — [config.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/config.cpp); [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)
- XRI: Instantaneous movement "can pass through Colliders that lack a Rigidbody." Velocity Tracking collides but lags. Kinematic keeps visuals matched to physics. Retain Transform Parent restores the original parent on drop. Force Gravity On Detach controls gravity after release. — [XRI 3.0 docs](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/xr-grab-interactable.html)
- LÖVR grabs with joints: a ball joint plus a slider joint between the hand collider and the object, destroyed on release. — [LÖVR docs](https://lovr.org/docs/v0.19.0/Physics/Hand_Physics)
- I Expect You To Die tried colliding held objects that drop past a collision threshold, then judged it frustrating in complex levels. — [Schell Games (snippet)](https://legacymicrosites.schellgames.com/blog/engineering-a-super-spy-in-i-expect-you-to-die)
- Alyx: held weapons don't clip through walls, and weapons can push doors and swat objects. — [Road to VR p.2](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2/)
- HurricaneVR's max-distance behaviors (drop the held object, or sweep the hand and object to a safe spot) double as a drop rule when a held object is jammed. — [HVR docs](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/physicshands.md)
- HIGGS extras on drop: dropping near the mouth consumes the item and dropping over the shoulder stashes it (radius and velocity thresholds configurable). Drop haptics and sound scale with mass. — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/config.cpp)

### Inferences
- Recommended approach for Rapier (inference): keep the held object a dynamic body attached to the dynamic hand with a 6-DOF joint driven by motors (or velocity-track the object directly toward the hand-grip target). Exclude hand-object contact pairs while held, using collision groups or a contact-pair filter. Enable CCD on held and thrown objects to prevent tunnelling. When the held object reports active contacts, reduce motor stiffness or max force (HIGGS "colliding tau" pattern) so it rests on walls instead of fighting. On release, keep hand-object collision filtered for about 0.1 s (HIGGS).
- If kinematic attach is preferred for low latency (guns), do what Godot does: merge the held object's colliders into the hand's collide-and-slide body, so the combined hand plus gun is what's stopped by walls.
- Light objects held by a strong hand joint can inject huge impulses into the world (pushing through stacks). Cap motor force by held mass (HIGGS mass × 500 cap) and clamp held-object velocity.

### Gaps
- No primary source on how Alyx attaches held props (kinematic vs constraint) or how Bonelab handles held-object versus world collision.

---

## 5. Throwing (velocity estimation, assist, angular velocity, why throws feel weak)

### Takeaway
Don't use the last frame's position delta of a kinematic object. It is noisy, and at the moment of release the hand is decelerating. The cited systems use one of three things: the tracking runtime's own controller velocity sampled slightly in the past (SteamVR −11 ms, ×1.1), a short moving average (5 frames linear, 11 angular; Godot 5 samples; XRI about 0.25 s window, up to 20 frames), or the peak speed in a recent history averaged with its neighbors (HIGGS). Add locomotion/player velocity and ω × r tangential velocity for objects held off-center, and scale angular velocity down (HIGGS × 0.6). Alyx additionally ignores inertia sway when computing the throw.

### Cited Findings
- SteamVR Throwable defaults: `releaseVelocityStyle = GetFromHand`, `releaseVelocityTimeOffset = −0.011 s`, `scaleReleaseVelocity = 1.1`, `scaleReleaseVelocityThreshold = −1` (disabled), and `scaleReleaseVelocityCurve = EaseInOut(0, 0.1, 1, 1)`. GetFromHand calls `hand.GetTrackedObjectVelocity(−0.011)` and the angular equivalent, so it uses the tracking system's pose velocity about 11 ms in the past. Other styles: ShortEstimation (VelocityEstimator moving average), AdvancedEstimation (`GetEstimatedPeakVelocities`), and NoChange. Scaling applies only to linear velocity: `velocity *= clamp01(curve(|v|/threshold)) * 1.1`, with the curve part used only when the threshold is above 0. — [Throwable.cs](https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/Throwable.cs)
- SteamVR VelocityEstimator: `velocityAverageFrames = 5` and `angularVelocityAverageFrames = 11`. Each sample is the single-frame `(pos − prevPos)/dt` in a ring buffer, and the estimate is the mean of the filled slots. Angular velocity per frame: `dq = q * inverse(q_prev)`, `θ = 2·acos(clamp(dq.w))` wrapped to ±π, axis = normalize(dq.xyz), then `ω = θ/dt · axis`, averaged over 11 frames. — [VelocityEstimator.cs](https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/VelocityEstimator.cs)
- HIGGS throw release:
  - Hand velocity = `GetMaxVelocity(history)`, which takes the frame of peak speed and averages it with its two neighbors (`(v[i−1]+v[i]+v[i+1])/3`).
  - It adds a rolling-average player (locomotion) velocity.
  - If the hand component exceeds `throwVelocityThreshold = 1.0 m/s`, it is multiplied by `throwVelocityBoostFactor` (default 1.0, i.e. no boost).
  - Angular = hand angular × `angularVelocityMultiplier = 0.6`.
  - With `inheritTangentialVelocity`, it adds `ω × r_com` from the palm to the object's center of mass, clamped at `tangentialVelocityLimit = 5.0 m/s` (rescaling ω to match when clamped).
  - Total = player + hand + tangential, applied as body linear velocity, plus the angular velocity.
  - Throws above threshold ignore hand collision for 0.1 s.

  — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- HurricaneVR's HVRThrowingCenterOfMass supplies the center of mass used when converting angular velocity to linear velocity on a throw (otherwise the hand rigidbody's COM). — [HVR docs](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/physicshands.md)
- Godot XR Tools: a velocity averager with `velocity_samples = 5` samples the held object's transform each frame (or the hand's when empty). On drop it applies `linear_velocity × impulse_factor` (default 1.0) and the averaged angular velocity. — [function_pickup.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd)
- Unity XRI: Throw On Detach is "not supported for kinematic Rigidbodies." Throw Smoothing Duration is the time window of samples, up to 20 previous frames, with 0.25 s as the documented example. The Throw Smoothing Curve weights samples (flat 1.0 by default). There are separate Throw Velocity Scale and Throw Angular Velocity Scale values. — [XRI 3.0 docs](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/xr-grab-interactable.html)
- Auto Hand: Throw Power multiplies the grabbable's velocity on release. ForceRelease() releases without throw force. — [Auto Hand docs](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/hand)
- Alyx ignores inertia sway only in the throw calculation, to keep throws accurate while held objects still feel weighty. — [Road to VR p.1](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/)
- Meta: "The object inherits the hand's velocity and direction at the point of release." — [Meta Horizon grab design](https://developers.meta.com/horizon/design/grab_usage/)

### Inferences
- Common mistakes that make throws feel weak or wrong (synthesized from the mechanisms above):
  1. Reading the velocity of a kinematic or held body. It is zero or meaningless; XRI explicitly doesn't support it.
  2. Sampling only the release frame, when the wrist is already decelerating. HIGGS's peak-of-history and SteamVR's −11 ms offset both fix this.
  3. Over-smoothing with long windows, which averages in the wind-up. Keep the window around 50–100 ms (5 frames at 90 Hz is about 55 ms), versus XRI's 0.25 s example.
  4. Ignoring the lever arm. A wrist flick with an object held 20–40 cm from the palm (hammer head, bottle neck) needs ω × r added.
  5. Ignoring locomotion or platform velocity.
  6. Passing raw angular velocity, which gives crazy spin. HIGGS uses 0.6×.
  7. Releasing into the hand collider. Ignore hand contact for about 0.1 s.
  8. The object lagging behind a PD hand. Compute the throw from the controller, not from the lagging physics object (the Alyx "ignore inertia sway" principle).
- Prefer the OpenXR runtime's reported controller linear and angular velocity, sampled with a small negative time offset, when available. Fall back to a ring buffer of tracked poses with peak-of-window selection.
- Multipliers: SteamVR 1.1 and HIGGS 1.0 boost are the published defaults. A speed-dependent curve that boosts only fast throws (SteamVR's optional threshold curve) avoids making gentle drops fly.

### Gaps
- Unity XRI's actual default values for throwVelocityScale and throwAngularVelocityScale were not confirmed (the doc page gives no defaults).
- No source gives Boneworks/Bonelab or Alyx throw multipliers or windows.
- The SteamVR `GetEstimatedPeakVelocities` (AdvancedEstimation) internals in Hand.cs were not fetched.

---

## 6. Half-Life: Alyx gravity gloves (and similar force-pull features)

### Takeaway
Alyx's gloves are a four-stage, buttonless-feeling interaction:
1. Selection: point, and the object glows.
2. Lock-on: pull the trigger or grip, and the selection no longer changes.
3. Pull: a quick wrist "yank" toward yourself.
4. Catch: the object flies on a smooth, assisted arc that adjusts toward your hand, and you close your hand to catch.

Each stage has its own sound. Valve rejected a dedicated button because it reduced use. Players love that it removes bending and walking. The main failure is pulling too gently or too early. HIGGS reimplements the gloves with published numbers: a 1.2 m/s pull threshold, a guided ballistic arc of about 0.3–0.7 s, and re-aiming at the hand for the first 0.1 s.

### Cited Findings
**Valve's design and rationale**
- Kerry Davis (Valve): reaching objects from a distance is valuable because movement is "sort of one of the highest friction areas of the experience." The gloves are always on, not a separate weapon. Valve avoided buttons, calling them the "least VR thing you can do." You point, flick your wrist, and catch. It is awkward at first and second nature after a few hours. Objects come toward you "in a smooth arc so they're easy to catch," and "if you're reaching toward an important item like ammo, the game anticipates that you want to pick it up." In playtests, players went from consciously working the gloves to using them automatically. — [Engadget](https://www.engadget.com/2020-03-31-half-life-alyx-valve-interview-vr.html)
- Road to VR's breakdown: (1) selection, where the object glows; (2) lock-on, where pulling the trigger confirms "even if you move your hand now the selection won't change"; (3) a pulling gesture, not a button, starts the pull. Each stage has distinct sound. The trajectory adjusts in real time to reach your hand, tuned so it doesn't feel like auto-snapping. Lock-on helps because "both the player and the game are on the same page before the object is pulled." — [Road to VR p.1](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/)
- Development history: early "Grabbity Gloves" naming. Developers first thought a toggle was needed to avoid conflicts with normal pickup, but a separate button made playtesters use the gloves less, so it was dropped. Similar controls for grab and pull proved most intuitive. Pullable objects are those "light enough to be carried in one hand." — [Combine OverWiki (citing Valve's commentary)](https://combineoverwiki.net/wiki/Gravity_Gloves)
- The gloves counter the VR problem of players inching or bending into things outside their play area. — [TechCrunch (via search summary)](https://techcrunch.com/2020/03/23/half-lifealyx-delivers-the-watershed-moment-vr-gaming-needs/); they cut fatigue from bending down. — [Transform Interactive](https://transforminteractive.com/what-half-life-alyx-taught-us/)

**Player technique and failure modes**
- Community advice: palm facing down, not at the object, so the glove claws raise and glow. Aim until it lights up and hold the trigger the whole time. Hold the highlight "for a longer period than a mere split second," then "flick your controller like you are trying to pull it as if it is on a piece of invisible string." Common failures are pulling too gently ("You really have to flick that thing") and pulling too early with nothing highlighted. Some Vive-wand users couldn't trigger it at all. — [Steam forum thread](https://steamcommunity.com/app/546560/discussions/0/1861616237339229551)

**HIGGS: an open-source Alyx-style implementation with numbers**
- States: Idle → SelectedClose/SelectedFar → (trigger within `triggerPressedLeewayTime = 0.3 s`) SelectionLocked → Pulled → grab. — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Selection: far spherecast radius 0.3 to 5.0 distance along the pointing vector, with a 50° HMD cone gate (see Section 3). — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Flick detection in SelectionLocked, when the hand isn't near the object: `controllerSpeedTowardsHand = dot(controllerVelocity, normalize(handPos − selectedPoint))`, and it pulls if this is greater than `pullSpeedThreshold = 1.2 m/s` and the object is pullable. A locked object that comes within grab range transitions straight to held. — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp)
- Pull duration from palm–object distance d: `pullDuration = A + B·exp(−C·d)` with A = 0.715619, B = −0.415619, C = 0.656256. The pull expires after `pullDuration + 1 s`. — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Guided ballistic arc. For the first `pullTrackHandTime = 0.1 s` the target is re-aimed at `palmPos + (0, 0, 0.01 m)`. For `pullApplyVelocityTime = 0.2 s` the velocity is re-solved every frame with `t = pullDuration − elapsed`:
  - `v_xy = Δxy / t`
  - `v_z = 0.5·9.81·t + Δz / t`

  After that the object flies freely. Pulled-object angular damping is set to 8.0, and the original value is restored on end. — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Catch assist: `widePullGrabRadius = 0.9` is used to detect the pulled object for a grab when the normal near cast misses. Grabbing a pulled object ends the pull and nudges it toward the palm (`pulledGrabHandAdjustDistance`). — [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Looting variant: flicking at a container or corpse with `lootSpeedThreshold = 1.2` loots it. — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)

**Other force pulls**
- Boneworks: force-grab thrown objects back "by making a fist with your index and middle fingers." — [UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)
- Godot XR Tools: ranged grab (5 m, 5° cone) with either SNAP (instant) or LERP to the hand at `ranged_grab_speed = 20.0`. — [pickable.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/objects/pickable.gd); [function_pickup.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd)
- HurricaneVR: a Hand Grabber in "Pull Mode" creates a joint (HVRJointSettings) to pull the object to the hand with physics. — [HVR docs: Joint Settings](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/jointsettings.md)
- Auto Hand Grab Type "moves objects to the hand." — [Auto Hand docs](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/hand)

### Inferences
- Computed HIGGS pull durations (my arithmetic from the formula): d = 0 → 0.30 s; 1 m → about 0.50 s; 2 m → about 0.60 s; 3 m → about 0.66 s; 5 m → about 0.70 s. Flight time stays nearly constant past 2 m, so far objects arrive fast and catch timing stays predictable. The high apex at long range comes from `0.5·g·t`, about 3.4 m/s initial up-velocity at t = 0.7 s.
- Alyx-style implementation recipe (inference):
  1. Selection ray from the hand with a cone of a few degrees, scored by angle and distance, with hysteresis (Meta's narrow hover / wide unhover).
  2. Glow plus a soft haptic tick on selection change.
  3. Lock on grip or trigger press within a ~0.3 s leeway.
  4. Detect the flick as hand velocity projected toward the player above about 1.2 m/s. Optionally also accept wrist angular velocity (pitch-back) above an unsourced estimate of about 3–6 rad/s.
  5. Launch on a re-solved ballistic arc for ~0.2 s, re-aiming at the palm, then go ballistic.
  6. Widen the catch radius to ~0.5–0.9 m around the hand for the flying object, and auto-grab if grip is held when it enters (Valve's "anticipates" ammo behavior).
  7. Mass and size gate: one-handable objects only.
- What players love, from the interview material: it removes the highest-friction action in VR (moving or bending to reach), it is always available, and it is gestural. What frustrates them (forum material): an unclear hand orientation requirement and flick thresholds that are too strict for some users or controllers. Expose the threshold and offer an assist option (for example, a lower threshold or an option to auto-pull on lock).

### Gaps
- Valve's actual flick velocity or angular threshold, selection cone angle, max pull range, and arc formula are not public. HIGGS's values are a community reimplementation, not Valve's numbers.
- No primary source for Bonelab's force-grab parameters or Into the Radius's pull mechanics (if any).
- The Valve "Inside Valve"/developer commentary transcript was not directly accessed. The toggle-button history is via the Combine OverWiki.

---

## 7. Hand posing from controller input (finger curl, capacitive sensing, analog blending)

### Takeaway
Controllers provide coarse finger state. Touch gives capacitive touch and "near touch" on the trigger, the face buttons/thumbstick and the thumbrest, plus analog trigger and grip. Index gives per-finger capacitive curl for middle, ring and pinky, trigger touch and pull for the index finger, a grip force sensor, and OpenVR skeletal curl and splay values. Games blend authored poses (open, point, fist, thumbs-up) from these signals. When holding, they switch to an object-specific or procedurally solved grip pose (Lone Echo, HIGGS, Auto Hand).

### Cited Findings
- Index Controllers have capacitive sensors for middle, ring and pinky plus a trigger sensor for the index finger. VRChat uses them for approximate finger posing. — [VRChat docs](https://docs.vrchat.com/docs/valve-index)
- The Index grip force sensor is tuned from a light touch to a firm squeeze, enabling actions like crushing. — [Valve Index controllers page](https://www.valvesoftware.com/en/index/controllers)
- SteamVR Skeletal Input provides skeleton-based hand animation for Vive, Touch and Knuckles. Hardware vendors, not apps, add support for new controllers. — [Road to VR](https://www.roadtovr.com/valve-announces-skeletal-input-give-steamvr-apps-lifelike-hands-moving-forward/)
- OpenVR's skeletal summary returns per-finger curl and splay (`flFingerCurl`, `flFingerSplay`). A Khronos forum thread reports that OpenXR's hand-tracking extension didn't supply this for Index controllers. — [Khronos forum](https://community.khronos.org/t/how-is-a-limited-finger-tracking-controller-like-index-controller-supposed-to-work/108265)
- Unreal's SteamVR plugin exposes per-frame finger curl and splay getters. — [Unreal docs](https://docs.unrealengine.com/5.0/en-US/PythonAPI/class/SteamVRInputDeviceFunctionLibrary.html)
- Oculus Touch: buttons and thumbsticks are capacitive, so the controller knows which surfaces fingers are resting on, and that data becomes a hand pose. Tracking is coarse, not per-finger. — [VRroom review archive](https://archive.vrroom.studio/?p=29905); the controller detects thumb, index and middle finger. — [Tom's Hardware](https://www.tomshardware.com/uk/reviews/oculus-touch-motion-controller-review,4841.html)
- The Touch thumbrest is a capacitive sensor registering contact and "near touch," which produces thumbs-up poses. — [iFixit teardown](https://www.ifixit.com/News/oculus-touch-teardown)
- With some OpenXR-backed SDK versions, NearTouch read as always zero, which broke point and thumbs-up detection. The workaround was plain Touch (contact only). — [Meta community forum](https://communityforums.atmeta.com/unity-development-123/ovrinput-neartouch-not-working-with-ovrplugin-with-openxr-backend-111266)
- Boneworks reads index-finger release to loosen the grip while holding. — [UploadVR](https://uploadvr.com/boneworks-feels-like-the-first-next-generation-vr-game/)
- Auto Hand has finger sway with hand motion (Sway Strength) and Grip Offset bend. HIGGS animates fingers at configurable linear and angular speeds and closes them partially when an object is selected nearby (`SelectedCloseFingerAnimValue`). — [Auto Hand docs](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/hand); [HIGGS config.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/config.cpp)

### Inferences
- Portable mapping (inference, standard practice; not from a cited game spec), per frame and per hand:
  - `index = trigger_touched ? lerp(0.3, 1.0, trigger_value) : 0.0`. Not touching the trigger gives "point."
  - `middle/ring/pinky = grip_value`, or the per-finger capacitive curl on Index.
  - `thumb = any_thumb_surface_touched ? 1.0 : 0.0`. Not touching gives "thumbs up," using near-touch when available.
  - Blend authored poses: open → fist (per-finger curl) with additive point and thumb-up layers. Smooth each finger with a critically damped filter (~50–80 ms) to hide capacitive flicker.
- OpenXR (2026): expose the abstraction through action bindings (`trigger/value`, `trigger/touch`, `squeeze/value`, `thumbrest/touch`, `thumbstick/touch`). Treat Index per-finger curl as an optional extension, since the cited Khronos thread says it's not in core OpenXR.
- While holding, override the input pose with the grab pose. Optionally allow partial release (Boneworks index-lift) and grip-force-scaled "squeeze" (Index force sensor) for crush or grip-strength mechanics.

### Gaps
- No primary Valve or Meta spec was retrieved for the current OpenXR finger-curl extension status for Index (for example, whether an `XR_EXT_hand_tracking` data-source or controller-curl extension now carries curl). Verify against current runtime docs.
- No source found for how Alyx specifically maps capacitive input to hand poses.

---

## 8. Physical interactions with the world (buttons, levers, doors, drawers, pushing, climbing)

### Takeaway
Mechanisms are best modeled as constrained bodies (hinge, slider) that the hand grabs. The hand then latches to the handle, so the virtual hand stays on the mechanism even though the real hand diverges. This is the Alyx/Job Simulator approach. Pushing is free when hands are dynamic colliders. Climbing is done by moving the player body opposite to the gripping hand's controller offset, with gravity off while gripping and a fling velocity on release.

### Cited Findings
- Alyx latches the hand to levers, knobs and cranks until release (credited to Job Simulator). Hands collide with walls, doors and tables. — [Transform Interactive](https://transforminteractive.com/what-half-life-alyx-taught-us/)
- Alyx doors open both ways, because push vs pull can't be felt. — [Engadget](https://www.engadget.com/2020-03-31-half-life-alyx-valve-interview-vr.html)
- Meta: "Constraints: Limit movement to specific axes, such as a lever or drawer." "Use while holding: Hold a tool with a palm grab and activate its primary function with a pinch." — [Meta Horizon grab design](https://developers.meta.com/horizon/design/grab_usage/)
- HurricaneVR ships door and stabbing components and a climbable type (HVRClimbable), indicating these are treated as grabbable mechanisms. — [HVR docs repo tree](https://api.github.com/repos/cloudwalker2020/HurricaneVR-Docs/git/trees/main?recursive=1)
- Godot XR Tools climbing:
  - Climbing is active while a "dominant" handle is held. The most recent grab is dominant, and dominance transfers to the other hand on release.
  - Each physics frame: `offset = controller_pos − grab_pos`, then `player_body.move_and_collide(−offset)`, `velocity = 0`, and gravity bypassed.
  - A hand drops its handle if the controller drifts more than `SNAP_DISTANCE = 1.0 m` from the grab point.
  - On release: `velocity = averaged_velocity × fling_multiplier (1.0) + camera_forward_flat × forward_push (1.0)`, with `velocity_averages = 5` (minimum 2).

  — [movement_climb.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/movement_climb.gd)
- Lone Echo zero-G: grab any surface and "push and pull yourself around." Locomotion is driven by a constraint system, and the head is positioned 1:1 relative to the gripping hand. — [Road to VR](https://roadtovr.com/lone-echos-virtual-hands-unassuming-vr-innovation/); [GDC Vault abstract](https://gdcvault.com/play/1024240/It-s-All-in-the); [Road to VR (search summary)](https://www.roadtovr.com/?p=60767)
- Boneworks' simulated body can be obstructed by the world, and heavy-object handling is physical and tiring (Section 1). — [Wikipedia](https://en.wikipedia.org/wiki/Boneworks); [GamesBeat](https://gamesbeat.com/how-boneworks-turns-your-body-into-its-key-mechanic/)
- Collision haptics (HIGGS): minimum speed 0.2 m/s; strength = base 0.1 + 0.05 × speed + 0.03 × mass^0.6; duration 0.01 s; ×0.4 for damped collisions. — [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h)
- Alyx: plants react to touch, alien flora reacts to proximity, and liquids slosh. The argument is that players will try to touch everything within reach. — [Road to VR p.2](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2/)

### Inferences
- Buttons (inference): use a kinematic or prismatic-jointed cap with a spring back to rest. Fire at about 70–80% travel with a hysteresis release (my estimate), so both physics hands and fast pokes work. Fire on finger-tip contact, not palm, to avoid accidental presses.
- Levers, doors, drawers, valves: dynamic body with a revolute or prismatic joint plus limits and friction or damping motors. On grab, attach the hand to the handle with a joint (or make the visual hand latch). Drive the mechanism by the controller target, not the blocked physics hand. Release the latch if separation exceeds about 0.3–0.5 m (estimate; compare Godot's 1.0 m climb snap, HIGGS 0.7 m).
- Climbing (Boneworks, Lone Echo, The Climb, Stride style): the Godot formula (move the body by −(controller − grab point) of the dominant hand, gravity off, fling = averaged hand velocity) is the standard kinematic approach. A physics-body variant (Boneworks-like) instead joints the hand to the wall and lets the body's PD-driven arms pull the torso, which costs stability and nausea risk.

### Gaps
- No primary sources were retrieved on Stride's or The Climb's climbing implementations, or on Boneworks' physical climbing specifics.
- No Valve source on how Alyx's latching hands are implemented (joint vs pose override).

---

## 9. Concrete numbers (consolidated reference)

### Takeaway
The best-documented public numbers come from HIGGS (Skyrim VR), the SteamVR Interaction System, Godot XR Tools and LÖVR. Commercial-game values (Alyx, Boneworks) are not public. The table below collects every value found, and each row is cited.

### Cited Findings
| Parameter | Value | Source |
|---|---|---|
| Near grab sphere | 0.3 m | [Godot function_pickup.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd) |
| Near grab cast | radius 0.05 over 0.1 distance | [HIGGS config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Far/ranged grab range | 5.0 m (Godot, HIGGS) | [Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd); [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Ranged cone | 5° (Godot, range 0–45°) | [Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd) |
| Far spherecast radius | 0.3 | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| HMD-forward gate cone | 50° half-angle | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Grip hysteresis | threshold ± 0.1 | [Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd) |
| Input leeway (trigger after select) | 0.3 s; forced re-input 0.03 s | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Gravity-glove flick threshold | 1.2 m/s hand speed toward player | [HIGGS hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp); [config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Pull duration | 0.715619 − 0.415619·e^(−0.656256·d) s | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Pull: apply-velocity window / re-aim window | 0.2 s / 0.1 s | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Pull: destination offset above palm | 0.01 m | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Pull: angular damping during flight | 8.0 | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Pull: wide catch radius | 0.9 | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Ranged lerp speed | 20.0 (Godot, units unspecified) | [Godot pickable.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/objects/pickable.gd) |
| Throw: SteamVR time offset / scale | −0.011 s / ×1.1 | [SteamVR Throwable.cs](https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/Throwable.cs) |
| Throw: averaging frames | linear 5, angular 11 (SteamVR); 5 (Godot) | [VelocityEstimator.cs](https://raw.githubusercontent.com/ValveSoftware/steamvr_unity_plugin/master/Assets/SteamVR/InteractionSystem/Core/Scripts/VelocityEstimator.cs); [Godot](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/function_pickup.gd) |
| Throw: smoothing window | example 0.25 s, ≤ 20 frames (XRI) | [XRI docs](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.0/manual/xr-grab-interactable.html) |
| Throw: threshold / boost | 1.0 m/s / ×1.0 | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Throw: angular multiplier | 0.6 | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Throw: tangential velocity cap | 5.0 m/s | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Throw: ignore hand collision after release | 0.1 s | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Hand collision box half-extents | 0.05 × 0.015 × 0.09 m | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Hand collider (LÖVR) | 0.04 × 0.08 × 0.08 m, mass 0.5 | [LÖVR](https://lovr.org/docs/v0.19.0/Physics/Hand_Physics) |
| Hand force/torque gains (LÖVR) | F = Δp·dt·30000; τ = axis·angle·dt·20; damping lin 0.7 / ang 0.9 | [LÖVR](https://lovr.org/docs/v0.19.0/Physics/Hand_Physics) |
| PD position strength (Unity example) | 15 with hand mass 1 | [Amebous Labs (snippet)](https://amebouslabs.com/developing-physics-based-vr-hands-in-unity/) |
| PD damper rule of thumb | Damper ≈ Spring / 10 | [HVR PD docs](https://raw.githubusercontent.com/cloudwalker2020/HurricaneVR-Docs/main/docfx_project/manual/rig/pdstrength.md) |
| Held constraint (Havok) | tau 0.03, damping 0.8, maxForce 2000 (weapon 9000), maxForce ≤ 500 × mass, colliding tau 0.01, fade-in 0.1 s | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Grab lerp into hand | 0.1–0.2 s for 0.1–0.2 m distance | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Hand teleport/snap-back distance | 0.7 (HIGGS); 1.0 m (Godot collision hand) | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [Godot collision_hand.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/hands/collision_hand.gd) |
| Held-weight hand force | 15 N → 300 N, scaled by controller lead / 1.0 m | [Godot collision_hand.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/hands/collision_hand.gd) |
| Two-handed blend | alignment 1.0, shift 0.5, rotation 0.5, rot snap speed 1200 | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Finger anim speeds | grab 4.0 / 630; start 2.0 / 135; end 2.0 / 45; min curl 0.2 | [HIGGS config.h](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h); [hand.cpp](https://raw.githubusercontent.com/adamhynek/higgs/master/src/hand.cpp) |
| Climb snap distance / fling | 1.0 m / fling ×1.0 + forward push 1.0, 5-sample average | [Godot movement_climb.gd](https://raw.githubusercontent.com/GodotVR/godot-xr-tools/master/addons/godot-xr-tools/functions/movement_climb.gd) |
| Movement slow with held mass | proportion 0.675, exponent 1, max 75 (%) | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Collision haptics | min speed 0.2 m/s; 0.1 + 0.05·v + 0.03·m^0.6; 10 ms | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Grab start speed (exact role not verified in source read) | 200 Skyrim units/s, 360°/s, 0.5 s max | [HIGGS](https://raw.githubusercontent.com/adamhynek/higgs/master/include/config.h) |
| Vanilla Skyrim melee hitbox scale (removed by PLANCK) | 1.75× weapon | [TheGamer](https://www.thegamer.com/skyrim-vr-slap-npcs-planck-mod/) |

### Inferences
- Unit caveats: HIGGS comments state m/s for pull and throw thresholds and metres for the hand box, palm offset and pull Z offset. The cast radii and distances are uncommented but used alongside Havok-scaled palm distances, so they are probably metres (inference). 200 Skyrim units/s is about 2.9 m/s, taking about 70 Skyrim units per metre (my approximation).
- Suggested starting values for a new Rapier-based system, synthesized and to be tuned in playtests:
  - Near grab radius 0.07 m from the palm point.
  - Ranged selection 5–8 m, 5–8° cone with ~1.5× unhover.
  - Grip threshold 0.5 ± 0.1.
  - Flick 1.2 m/s toward the player.
  - Pull flight 0.3–0.7 s via the HIGGS formula.
  - Catch radius 0.5 m (HIGGS uses 0.9).
  - Throw: peak of a 5–8 frame window (or the runtime velocity at −11 ms), ×1.0–1.2 linear, ×0.6 angular, plus ω×r capped at 5 m/s, plus locomotion velocity.
  - 0.1 s hand-collision ignore after release.
  - Snap-back at 0.7 m.
  - Held-object motor force capped at 500 × mass.
  - Stiffness drop to about 1–5% while the held object is in contact.

### Gaps
- No public numeric values for Valve (Alyx), Stress Level Zero (Boneworks/Bonelab), CM Games (Into the Radius) or RUST LTD (H3VR) hand or grab systems.
- HurricaneVR and Auto Hand default numbers live in their Unity assets and are not in the public docs.
- Unity XRI default throw scales are unconfirmed. Only the 0.25 s smoothing example and the flat default curve are documented.
