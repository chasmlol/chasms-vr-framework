# VR UI/HUD placement: how great VR shooters and flat-to-VR conversions do it, and a placement spec for a flat game's UI

> A research snapshot (2026-10-21), not a description of the framework's code: the HUD placement as
> built is `crates/runtime/src/hud.rs`.

Research notes, 2026-10-21. About 110 search/fetch calls plus direct reads of open-source code (UEVR, REFramework, Doom 3 BFG VR, MRTK, Unity XRI).

Legend:
- **(excerpt)**: the claim comes from a search-engine summary, not a full-page fetch.
- **[UNVERIFIED]**: my prior knowledge, not confirmed by a source in this session.
- **[computed]**: my arithmetic. Angular size is θ = 2·atan(s / 2d).
- **[inference]**: my design conclusion.
- Angles are degrees of visual angle. "Below" means pitch below the head's forward direction unless I say "horizon".

Context, as it was when this was written: an adapter drew all of its game's UI into one transparent UI texture, after the game's own post-processing. The runtime floated that whole texture as one panel, 1.5 m wide at 1.4 m (about 56°×34° [computed]). The panel lazily followed head yaw and pitch with a 0.25 s exponential time constant and no dead zone. Menus were world-locked, 2.6 m wide at 2.2 m (about 61°×37° [computed]), and re-anchored when you turned more than 70° away. Everything below is measured against that baseline. (Since then the runtime places the HUD piece by piece from the adapter's `HudLayout`, with a caption strip and a 20° dead zone among much of what §7 and §8 propose: `crates/runtime/src/hud.rs`.)

---

## 0. Executive summary

**Takeaway**
1. **None of the acclaimed games keep a single head-following HUD sheet.** They split the HUD:
   - Status goes on the body. Health and stat displays sit on the back of the hand or the wrist in Alyx, HL2VR, Doom 3 VR, Roboquest VR, RE4 VR and Metro Awakening.
   - Ammo goes on the weapon, in Alyx, HL2VR, Doom 3 VR and Roboquest VR.
   - Inventory goes on body slots or a backpack.
   - Text (subtitles, prompts) goes on a lazily-following or snapping strip low in view.
   - Menus go on a world-locked panel, or on a hand-held device such as Doom 3's PDA.
2. **A game whose own fiction already puts its readouts in the world suits this best:** a wristwatch that shows a stealth indicator and a timer, a journal with a compass, the rounds visible in a magazine. Metro Awakening has such a watch and "no on-screen UI" (§4).
3. **Concrete defaults the sources support:**
   - Long-read text: at least 0.5 m away, about 1 m comfortable (Meta). Microsoft's comfort zone is 1.25–5 m.
   - Character height: about 1.2–1.4° for comfortable reading on current headsets, 0.75–0.9° minimum.
   - Keep content within about ±15° of eye rotation and from eye level to 15° below.
   - Lazy-follow dead zones of 20–30° with roughly 0.1–0.25 s catch-up (MRTK and XRI defaults).
   - Never roll a panel.
   - Draw reticles at the target's depth.
   - Avoid world-occlusion conflicts by pulling text in front of near geometry, or by keeping it on the body.
4. **TF2 VR (CircuitLord, Sept 2026)** is praised for VR-native button prompts and a cutscene 2D-panel toggle. It is criticised for subtitles wider than the Index FOV and for having no crosshair or laser. Its HUD is head-attached by default (`tf2vr_head_hud 1`), with HUD, Titan HUD and hide options.

---

## 1. Half-Life: Alyx: where every piece of information lives

### Takeaway
Alyx has **no conventional HUD**.
- **Health:** three hearts on monitors on the back of the left (off-hand) glove. They flash at low health, and an orange "check engine" light comes on. The glove's lights blink and beep only when you raise the hand to your face.
- **Glove LED:** the same display shows ammo. With an empty hand selected, it shows resin.
- **Weapons:** each has its own diegetic magazine and ammo readout, plus distinct sounds for last-round and empty.
- **Inventory:** ammo comes from a contextual "over the shoulder" grab, there is a quick weapon select on the weapon hand, and two wrist "hand pockets" hold items.
- **Menu:** designed mainly for laser pointing, but it also works as a touchscreen.
- **Subtitles:** sit in the world. They hold still until pushed out of view, then snap to a new readable spot. A Deaf/HoH reviewer called them the best VR subtitles they had seen.
- **Controller tutorials:** come from the Source "game instructor" hint system and recur, for example on every reload. They can be disabled with `+sv_gameinstructor_disable 1`.

### Cited findings
- **Glove display.** Health, ammo and resin show on built-in monitors on the back of the left glove, replacing the HUD of earlier games. When the player is badly hurt, the three hearts flash and an orange check-engine symbol lights. The hearts nod to Zelda. Early in development the indicators lived in a wristwatch. It was dropped because wrists aren't rendered, and unused watch pieces remain on the glove model. — [Combine OverWiki: Gravity Gloves](https://combineoverwiki.net/wiki/Gravity_Gloves)
- **Health location.** It sits on the back of the non-dominant hand, so the player must physically look at it. — [Wikiwand: Health (game terminology)](https://www.wikiwand.com/en/Health_(game_terminology)) (excerpt). A review says the back of the left hand shows health and ammo and doesn't take you out of the world. — [Gaming Nexus review](https://www.gamingnexus.com/Article/6095/Half-Life-Alyx/)
- **Gaze-gated blinking.** The two lights above the health bar flash and beep only when the gloves are lifted into view, and stop when you lower your arms. With an empty hand selected, the main LED display shows resin. — [Steam: "Two beeping lights on Alyx's gloves"](https://steamcommunity.com/app/546560/discussions/0/2143091644385720908/). Holding weapon-select also shows ammo counts. — [Steam: Health/Ammunition Info](https://steamcommunity.com/app/546560/discussions/0/2137462524941270892/)
- **Per-weapon ammo readouts.** The pistol magazine has a light per round and a radial indicator on its base that depletes; an empty magazine emits no light. The pistol has a segmented indicator, the shotgun a small numeric one, the Combine SMG a radial one. Sounds mark chambering, last round chambered, and empty with the chamber open. Key items get subtle lights, not bright outlines. The optional spectator UI shows weapons, health, resin and "hand-pockets" on the monitor only. The menu is "primarily designed for laser pointer interaction" but also works like a touchscreen. — [Road to VR: Inside XR Design, Alyx p.1](https://roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/); [p.2](https://roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2/)
- **Valve AMA.** Ammo is an over-the-shoulder contextual inventory on the off hand. There is a quick weapon select on the weapon hand and a couple of wrist bags. The aim was to keep the focus on the environment and always leave a free hand. — [UploadVR: Alyx AMA](https://www.uploadvr.com/half-life-alyx-ama/)
- **Subtitles.** A "middle ground": they don't bounce around and aren't rigidly fixed. If pushed too far out of view they quickly snap to a new readable location and stay there. Speakers are colour-coded with no names. Options are size, speed and on/off. — [Can I Play That? Deaf/HoH review](https://caniplaythat.com/2020/03/25/deaf-hoh-review-half-life-alyx/). Equal Entry classes Alyx's captions as head-locked, positioned relative to gaze. — [Equal Entry](https://equalentry.com/accessibility-virtual-reality-captions-open-source/)
- **Tutorial hints.** The controller tutorial appears throughout, including on every reload, and is disabled with `+sv_gameinstructor_disable 1`. — [Steam: How to disable hints?](https://steamcommunity.com/app/546560/discussions/0/2272575584119900269/)
- **Developer commentary.** The Commentary mode has 147 nodes and over three hours of audio. I found no transcript covering the UI. — [UploadVR](https://uploadvr.com/half-life-alyx-developer-commentary/)

### Gaps
- I found no exact distances or sizes for Alyx's subtitle panel or menu.
- **[UNVERIFIED]** The pause menu is a floating world panel with the scene darkened.
- **[UNVERIFIED]** Hints render as a small controller hologram near the hand with the relevant button highlighted.

---

## 2. Titanfall 2 VR (the 2026 mod)

### Takeaway
- **What it is:** **"Titanfall 2 VR" by CircuitLord**, a closed-source campaign-only OpenXR conversion running on Northstar. It installs through the MIT-licensed *CircuitLord VR Mod Installer*. Titanfall 2 support arrived in installer v1.2.0 on 29 Sept 2026, and mod 1.0.17 was on the stable channel by 17 Oct 2026.
- **HUD:** head-attached by default (`tf2vr_head_hud` = 1). An in-game VR settings menu adjusts the HUD, the Titan HUD and the gun angle (`tf2vr_gun_angle_offset` default −15). The HUD can be hidden.
- **Praised:**
  - Controller-correct prompts ("feels like a native VR game").
  - Reload highlights on the gun.
  - Holographic sights that work at depth.
  - Cutscenes on a floating 2D screen by default, with a "force 3D" opt-in that carries a warning.
  - The Titan cockpit's grid and window, which help comfort.
- **Criticised:**
  - Subtitles run off the edge of the Index FOV. The user asked for them centred low, width-capped and wrapped, with convars for distance, scale and height.
  - No crosshair or laser option.
  - A reticle offset bug.
  - The Titan HUD seat feels too far back and too high.
  - Some users want the weapon highlights to be optional.
- **No data found** on the smart pistol, the pilot HUD layout, or ammo counters.

### Cited findings
- **Mod identity.** Name, author, closed source, MIT installer, Big Walk first (6 Aug 2026), TF2 in installer v1.2.0 (29 Sept 2026), beta label, campaign only. — [vr.org install article](https://vr.org/articles/titanfall-2-vr-mod-how-to-install-circuitlord-northstar-2026); [CircuitLord installer GitHub](https://github.com/CircuitLord/CircuitLordVRModInstaller)
- **Versions.** Mod 1.0.12 on 4 Oct, 1.0.17 stable by 17 Oct; the installer shipped six builds in seven days. — search excerpts of [vr.org guide](https://vr.org/titanfall-2-vr) and [Mixed](https://mixed-news.com/en/titanfall-2-vr-mod-installer-six-builds-dxgi-launch-block/)
- **VR settings menu.** It adjusts the HUD, Titan HUD and gun angle. A highlight shows which gun needs reloading ("a nice touch"). Titan abilities map to grip plus four directions. — [ARVRTips](https://arvrtips.com/titanfall-2-vr-mod/)
- **Prompts and cutscenes.** Prompts are adapted to VR controllers, unlike mods that "suggest you press LB, RB, L3". There is a toggle between full-VR cinematics (camera jerkiness) and a 2D panel. — [UploadVR](https://www.uploadvr.com/titanfall-2s-vr-mod-is-stellar-and-its-installer-is-the-best-thing-about-it/)
- **Aftermath.**
  - Cutscenes default to a floating 2D screen; an advanced option forces head-tracked 3D with a "too intense" warning.
  - Holographic sights render with real depth, and scopes have scope shadow.
  - Abilities use left grip plus a hand direction. Ion's laser and core are head-aimed.
  - Grenades are on the chest, cloak is on the off-hand trigger, and the secondary weapon is on the left hip.
  — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord/)
- **Subtitle issue (open).** Issue #34 (Valve Index): subtitles extend outside the visible FOV and cut lines off. The reporter asks to centre them low, cap the width so long lines wrap, and add `tf2vr_*` convars for distance, scale and vertical position. Their environment lists `tf2vr_head_hud` at default 1. Confirmed for Traditional Chinese too. — [GitHub issue #34](https://github.com/CircuitLord/CircuitLordVRModInstaller/issues/34)
- **Reticle and scopes.** Issue #20: bullets land offset from the reticle and scoped weapons show black. `tf2vr_gun_angle_offset` defaults to −15. — [GitHub issue #20](https://github.com/CircuitLord/CircuitLordVRModInstaller/issues/20)
- **Highlights and HUD toggle.** Issue #88 asks for an option to disable weapon-handling highlights, the "same way as you did give an option to hide the HUD". — [GitHub issue #88](https://github.com/CircuitLord/CircuitLordVRModInstaller/issues/88)
- **Recentre and Titan seat.** Issue #98: recentre drifts right, and the player feels too far back or too high in the Titan HUD, with no offset slider. — [GitHub issue #98](https://github.com/CircuitLord/CircuitLordVRModInstaller/issues/98)
- **ResetEra.** "it needs a laser sight… in VR, you cant do [crosshairs]". The Titan view's grid and centre "window" help motion sickness. — [ResetEra thread](https://www.resetera.com/threads/titanfall-2-vr-mod-is-now-available.1644223/)
- **Reviews.** UploadVR's review (14 Oct 2026) calls it among the best VR mods, with moments that remind you it wasn't made for VR. Weapon swapping is "cumbersome", and small bolts are hard to grip. — [UploadVR review](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/). Road to VR (2 Oct 2026) covers controls and comfort, nothing on UI. — [Road to VR](https://roadtovr.com/titanfall-2-vr-mod-circuitlord/)

### Lessons for a conversion [inference]
- Remap prompts to VR glyphs.
- Give cutscenes and videos a 2D panel by default.
- Cap the subtitle width to the comfortable FOV and expose distance, scale and height.
- Offer an optional reticle or laser.
- Make every highlight optional.

---

## 3. Other flat-to-VR conversions and VR ports

### 3.1 UEVR (praydog): generic projection of the 2D UI

**Takeaway.** UEVR draws the game's whole Slate UI on one quad, or optionally a cylinder:
- Default distance **2.0 m**. Default **UI_Size 2.0**, which is the panel **height** in metres.
- That is roughly 53° tall × 83° wide for 16:9 [computed]: a very large, cinema-like sheet.
- "UI follows view" is **off** by default, so the panel is anchored to the standing origin plus a recentre offset, not to the head.
- Its own framework UI can move to the left wrist. A controller laser emulates the mouse (A and B click, stick scrolls) and blocks aiming while it points at the UI.

**Cited (source code)** — [UEVR OverlayComponent.hpp](https://raw.githubusercontent.com/praydog/UEVR/master/src/mods/vr/OverlayComponent.hpp), [OverlayComponent.cpp](https://raw.githubusercontent.com/praydog/UEVR/master/src/mods/vr/OverlayComponent.cpp), [VR.hpp](https://raw.githubusercontent.com/praydog/UEVR/master/src/mods/VR.hpp)
- `UI_OverlayType` is Quad or Cylinder, default Quad.
- `UI_Distance` ranges 0.5–10, default 2.0. `UI_Size` ranges 0.5–10, default 2.0.
- `UI_X/Y_Offset` range ±10. `UI_Cylinder_Angle` ranges 0–360, default 90 (central angle, clamped to at least 1 rad).
- `UI_FollowView` defaults false. `UI_InvertAlpha` defaults false.
- `UI_Framework_Distance` defaults 1.75 and `UI_Framework_Size` 2.0. `UI_Framework_WristUI` and `UI_Framework_FollowView` default false.
- `DecoupledPitchUIAdjust` defaults true.
- **Code behaviour.**
  - Size is the height: OpenVR sets width = size × aspect; OpenXR uses meters_h = size.
  - Follow off uses the stage space plus standing origin. Follow on uses the view space or HMD pose.
  - The cylinder radius is set so the surface stays at the chosen distance.
  - Mouse emulation lerps the cursor at 10·dt and calls `set_aim_allowed(false)` while pointing at the UI.
  - Wrist mode attaches to the left controller and shows only when the head gaze also hits it.
- Community feedback: UEVR "worked remarkably well at separating out UI". — [Tildes](https://tildes.net/~games/1di9/uevr_is_here_hype_or_legit_its_legit) (from earlier notes)

### 3.2 REFramework VR (praydog; RE2/RE3/RE7/RE8 and other RE Engine games)
- Defaults:
  - `2DUIDistance`: 1.0 (range 0.01–100).
  - `2DUIScale`: 12 (1–100).
  - `WorldSpaceUIScale`: 15, for UI the game itself places in the world.
  - Overlay position and rotation are hard-coded. — [REFramework VR.hpp](https://raw.githubusercontent.com/praydog/REFramework/master/src/mods/VR.hpp)
- The VR code fires bullets from the muzzle instead of the screen centre. In RE2/RE3 a weapon-config variable handles this; RE4 needs a hook. — [GitHub discussion #869](https://github.com/praydog/REFramework/discussions/869) (excerpt)

### 3.3 Doom 3 BFG VR "Fully Possessed" (KozGit, now the CarlKenner repo): the most configurable open-source reference
**Takeaway.**
- **Stat watch.** A wrist "stat watch" shows ammo for the active weapon plus vertical health and armour bars.
- **HUD.** The HUD is a translucent world-projected object, **body-locked** (body yaw, never head roll). By default it sits **30° below horizontal at 32 in (0.81 m)** and **fades in only when the HMD pitches down ≥48°**.
- **Other HUD options:** force the HUD at low health, hide individual elements, and choose whether objects occlude the HUD.
- **PDA.** The PDA is held in the off hand, and the pause/system menu appears on the PDA.
- **GUIs.** All in-game GUIs are touchscreens by default.
- **Sight.** The default weapon sight is a laser.

**Cited (source)** — [Vr.cpp cvars](https://github.com/CarlKenner/DOOM-3-BFG-VR/blob/master/neo/vr/Vr.cpp), [default cfg](https://github.com/CarlKenner/DOOM-3-BFG-VR/blob/master/vr_assets/Fully%20Possessed/vr_openvr_default.cfg), [Player.cpp UpdateVrHud](https://github.com/CarlKenner/DOOM-3-BFG-VR/blob/master/neo/d3xp/Player.cpp), [MenuScreen_HUD.cpp](https://github.com/CarlKenner/DOOM-3-BFG-VR/blob/master/neo/d3xp/menus/MenuScreen_HUD.cpp), [README](https://github.com/CarlKenner/DOOM-3-BFG-VR/blob/master/README.txt)

**HUD cvars**

| cvar | default | meaning |
|---|---|---|
| `vr_hudType` | 2 | Look Activate. 0 disables the HUD, 1 shows it fully. |
| `vr_hudRevealAngle` | 48 | HMD pitch, in degrees, that reveals the HUD. Alpha ramps in and out each frame. |
| `vr_hudPosLock` | 1 | Locks the HUD to the body. 0 locks it to the face. |
| `vr_hudPosAngle` | 30 | Pitch of the HUD placement axis in look-down mode. 10° in the other modes. |
| `vr_hudPosDis` | 32 | Distance in inches (0.81 m). |
| `vr_hudPosVer` / `vr_hudPosHor` | 0 | Vertical and horizontal offsets in inches. The cfg sets vertical to 0; the code default is 7. |
| `vr_hudScale` | 1.0 | Scale, range 0.1–2. |
| `vr_hudTransparency` | 1 | HUD opacity. |
| `vr_hudOcclusion` | 1 | 1 means world objects do *not* occlude the HUD (the depth hack). |
| `vr_hudLowHealth` | 0 | Forces the HUD on at or below this health. The code default is 20. |

**Other cvars**

| cvar | default | meaning |
|---|---|---|
| per-element `vr_hud*` toggles | on | Health, ammo, pickups, tips, location, objectives, stamina, comms, weapon icons, flashlight. |
| `vr_wristStatMon` | 1 | Wrist stat watch on the right wrist. 2 puts it on the left. |
| `vr_PDAfixLocation` | 0 | 0 keeps the PDA in the hand; 1 fixes it in space. |
| `vr_pdaPitch` | 30 | PDA pitch. |
| `vr_guiMode` | 2 | Touchscreen. 0 uses the weapon as the cursor, 1 uses gaze. |
| `vr_weaponSight` | 0 | Laser. 1 red dot, 2 circle dot, 3 crosshair, 4 beam plus dot. |

- **Placement code.** The HUD axis is `idAngles(hudPitch, bodyYaw, 0)`. During cinematics it uses the last HMD yaw instead. The origin is the view position plus the axis × distance.

### 3.4 Half-Life 2 VR mods
- **HL2VR 1.2 (Nathan Andrews and WormSlayer, 2013).** Health and shield sit on the back of the hand; you flip your wrist to read them like a watch. Ammo appears on the side of each weapon as a hologram. — [Road to VR](https://roadtovr.com/hl2vr-mod-half-life-2-virtual-reality-download/)
- **Half-Life 2: VR Mod (Source VR Mod Team, Steam).**
  - Radial weapon menu, over-the-shoulder ammo storage, captions, laser sight, quick-reload. — [Steam store](https://store.steampowered.com/app/658920/HalfLife_2_VR_Mod/)
  - Later versions added context-specific HUD elements on weapons. — [Road to VR 1.4](https://roadtovr.com/news-bits-hl2vr-1-4-mod-released-best-half-life-2-experience-just-got-better/) (from earlier notes)
- **Garry's Mod VRMod HUD addons.**
  - Health and armour on the back of the hand. — [Steam Workshop](https://steamcommunity.com/sharedfiles/filedetails/?id=2339148390)
  - A "VR Hand Hud" draws the screen HUD on a curved panel attached to either hand. — [Steam Workshop](https://steamcommunity.com/sharedfiles/filedetails/?id=3502382025) (excerpt)

### 3.5 Skyrim VR and Fallout 4 VR (Bethesda ports, plus mods)
- **Skyrim VR.**
  - The compass sits below the point of view and is hard to find. On horseback it is in direct view, which reads better. The pause menu is panned left, and the recentre button opens the menu instead of recentring. — [Hardcore Gamer review](https://hardcoregamer.com/reviews/review-the-elder-scrolls-v-skyrim-vr/280166/)
  - Users move HUD elements with `[VRUI] fHUDCompassZ` / `fHUDMenuZ` in SkyrimVR.ini. — [Steam thread](https://steamcommunity.com/app/611670/discussions/0/1696043806556909510) (excerpt)
  - Spell Wheel VR adds wrist bars for Health, Magicka and Stamina. — [fkmods forum](https://fkmods.freeflarum.com/d/80) (excerpt)
- **Fallout 4 VR.**
  - The Pip-Boy is a wrist extension you raise and turn to see; there is an optional "projected" mode for comfort. — [Bethesda launch details](https://bethesda.net/en-US/news/fallout-4-vr-launch-details)
  - A reviewer found holding the arm up tiring and switched to the overlay mode, which was "very easy and intuitive". The Pip-Boy size is adjustable. — [UploadVR FO4VR review](https://uploadvr.com/fallout-4-vr-review/)
  - "The Pip-Boy can be a pain". — [PC Gamer](https://pcgamer.com/fallout-4-is-impressively-playable-in-vr)
  - With no rendered forearm, the raised Pip-Boy looks bizarre. An .ini controls how much the Pip-Boy inflates when looked at. — [Bit-Tech](https://bit-tech.net/reviews/vr-spotlight-fallout-4/1/), [Steam thread](https://steamcommunity.com/app/611660/discussions/0/1621724904371360432) (excerpts)
  - The vanilla status HUD lives in `world_HMD_status.nif` files. A power-armour HUD mod moves the HUD so it is "only visible when you look down, more akin to the standard compass". — Nexus excerpts (Nexus returned 403)
  - Idle Hands moves the HUD to the back of the hand. — [Nexus Idle Hands](https://www.nexusmods.com/fallout4/mods/42922) (from earlier notes)
  - The GingasVR essentials pack says "2D menus suck in VR" and ships no compass and no HUD by default, with a customisable HUD. — [Road to VR](https://roadtovr.com/fallout-4-vr-essential-mod-list/)

### 3.6 Flat2VR Studios and official ports
- **Roboquest VR (Flat2VR).**
  - The UI moved to the wrist, ammo counters went onto the guns, and the minimap was made less "in your face". That is the difference between "a port, and a conversion". — [Road to VR BTS](https://roadtovr.com/flat2vr-roboquest-bts-developer-video/)
  - A floating health bar shows by default. The left wrist gives detailed HP and level progress, the right wrist a minimap that expands on grip. Perk choices are physical cards. The reviewer called the wrist changes "very welcome". — [UploadVR review](https://uploadvr.com/roboquest-vr-review)
- **Resident Evil 4 VR (Armature, official).**
  - The health meter is on a wristwatch. — [SimpleGamer](https://www.simplegamer.co.uk/resident-evil-4-vr-gets-october-2021-release-date/)
  - Inventory and menus are a tabbed separate menu that feels "built for the flat game". — [TheVRGrid](https://www.thevrgrid.com/resident-evil-4-vr/)
- **BioShock.** BioShock Remastered VR (BioVRDev) and another bioshock-vr repo exist. I found no HUD details. — [GitHub BioVRDev](https://github.com/BioVRDev/Bioshock-Remastered-VR/releases) (gap)

### 3.7 Analogous native VR survival games
- **The Walking Dead: Saints & Sinners.**
  - A left-wrist watch warns before the bells and beeps as time runs out.
  - The backpack is over the left shoulder, the flashlight on the chest, and the journal (quests, crafting, notes, live map) on the right chest. — search summary of [Road to VR (Cas & Chary)](https://www.roadtovr.com/cas-chary-present-walking-dead-saints-sinners-gameplay-overview/), [GameChronicles](https://gamechronicles.com/the-walking-dead-saints-sinners-review-oculus-quest/) (excerpt)
- **LMU INTERACT 2021 study (37 participants).** It compared a head-up health bar, a health value on a virtual wristwatch, and physical effects (trembling, slowing).
  - The bar suited multiplayer.
  - The wristwatch suited single-player, story-driven games.
  - The physical effects were liked for gameplay impact.
  — [Köhle et al., IFIP](https://dl.ifip.org/IFIP-LNCS/hal-04292375v1)

---

## 4. Metro Awakening: a native VR shooter's diegetic UI

### Takeaway
**Metro Awakening (Vertigo, 2024)** has a watch that shows light/dark, counts down the filter and beeps. There is "no on-screen UI":
- Magazines have cutouts so you count rounds.
- Two backpacks (over the left and right shoulders) hold gear and guns, with ammo counters and a small compass on the pack.
- A chest slot gives context items.
- Mask removal is a grab of the highlighted mask, with a tutorial text prompt.
- Reviewers criticise the lack of indicators for where body slots are.

### Cited findings
- **Metro Awakening, watch and mask.** The watch shows light/dark and counts down the filter. The timer beeps a countdown. Fogged lenses are wiped by hand. There is "no on-screen UI". — [PlayStation Blog](https://blog.playstation.com/?p=397002); [Road to VR gameplay](https://www.roadtovr.com/?p=120197) (excerpt)
- **Metro Awakening, Duuro review.** The watch shows the real time and a stealth light ("If it's on, enemies can see you"). Magazines have cutouts.
  - Left hand over the left shoulder opens the main backpack (ammo counters, a small compass).
  - Right hand over the right shoulder draws the main gun.
  - Left hand over the right shoulder opens the weapon backpack. The pistol is at the right waist.
  — [Duuro](https://duuro.net/blog/metro-awakening-review)
- **Metro Awakening, Road to VR review.** Reach-over-the-shoulder detection was consistent. Items show at true count. The context chest slot reduces cognitive load. With no indicators for where inventory lives, it is "a miss for a VR game of this scope". — [Road to VR review](https://roadtovr.com/metro-awakening-vr-review-quest-pc-vr-psvr-2/)
- **Metro Awakening, mask removal.** A "Remove The Mask" tutorial prompt appears, but removal needs the mask highlighted first, and the prompt can appear too early. — [Steam](https://steamcommunity.com/app/2669410/discussions/0/4634860913217800201/)

---

## 5. General VR UI best practice, with numbers

### 5.1 Distance and depth
- **Meta.**
  - Discourage traditional HUDs. Embed information in the world or avatar: a held map or compass, a cockpit, a watch.
  - Long-focus objects at least 0.5 m away. About 1 m "is a comfortable distance for menus and GUIs".
  - Blur the background behind menus.
  - Depth conflict: a HUD that occludes a scene object which is *closer* than the HUD plane gives contradictory occlusion and disparity cues. This is extremely common with reticles and subtitles.
  - The reticle must be drawn on the targeted object; a fixed-depth reticle double-images.
  — [Meta: Display/Visual design](https://developers.meta.com/vr/design/display/)
- **Microsoft (HoloLens).** Comfort zone 1.25–5 m. Nothing closer than 40 cm; fade at 40 cm and clip at 30 cm. Content moving in depth is less comfortable than static content. — [Microsoft comfort](https://developer.microsoft.com/windows/mixed-reality/comfort) (excerpt)
- **LaValle.** Floating menus and virtual displays should be at least 2 m away. — [LaValle, Virtual Reality, §](https://lavalle.pl/vr/node400.html) (excerpt)
- **Apple visionOS.**
  - "Avoid anchoring content to the wearer's head"; anchor it in the person's space instead.
  - Centre important content.
  - Avoid depth on text.
  - Limit depth changes, because refocusing often is tiring.
  - Points are defined as angles. Dynamic scale keeps the angular size constant with distance.
  - Direct touch at or above the line of sight is tiring.
  — [Apple HIG: Spatial layout](https://developer.apple.com/design/human-interface-guidelines/spatial-layout)
- **Halo blur.** Dynamic depth adjustment of the UI, or a blur halo around it, reduces UI-vs-scene depth conflict in stereo video. — [Adobe Research, CHI 2018](https://research.adobe.com/publication/depth-conflict-reduction-for-stereo-vr-video-interfaces)
- **Occlusion on HoloLens.** Unity's default font material doesn't occlude, so text shows through objects; MRTK ships an occluding text shader. — [Microsoft: Text in Unity](https://learn.microsoft.com/en-us/windows/mixed-reality/develop/unity/text-in-unity)

### 5.2 Text legibility (angular size)
- **Microsoft, user research.**

  | Distance | Minimum legible | Comfortable |
  |---|---|---|
  | 2 m | 0.35–0.4° (12.2–14.0 mm) | 0.6–0.75° (20.9–26.2 mm) |
  | 0.45 m | 0.4–0.5° (3.1–3.9 mm) | 0.65–0.8° (5.1–6.3 mm) |

  Avoid light or thin weights at small sizes. — [Microsoft: Text in Unity](https://learn.microsoft.com/en-us/windows/mixed-reality/develop/unity/text-in-unity). These figures are HoloLens-derived and too small for lower-PPD VR headsets [inference].
- **Frontiers in VR 2023 (Kilpeläinen and Häkkinen).** Reported figures:
  - Google I/O'17: 1.375° comfortable.
  - Solum 2019: 21–23 dmm (1.2–1.3°) comfortable, 14–16 dmm (0.8–0.92°) minimum.
  - Dingler 2018: 1.8° comfortable.
  - Hoffmann 2019, on a 13.8 ppd display: legibility threshold 45′ (0.75°), comfort 70.2′ (1.17°).
  - Comfortable size is about 2× threshold for young normal vision and 3× for older viewers.
  - Higher resolution "improves legibility quite significantly".
  - Quest 2 = 20.6 ppd; Vive Pro Eye = 13.1 ppd.
  — [Frontiers 10.3389/frvir.2023.1243387](https://frontiersin.org/journals/virtual-reality/articles/10.3389/frvir.2023.1243387/full)
- **Headset PPD.** Quest 3 is about 25 PPD and Quest 2 about 20 PPD. — [Meta compare](https://www.meta.com/gb/quest/compare/) (excerpt). Index is about 14–15 PPD (forum excerpt, sources disagree).
- **Google dmm.** 1 dmm = 1 mm at 1 m. — [Google VR sticker sheet](https://developers.google.com/vr/design/sticker-sheet) (excerpt). Daydream sized fonts at 0.027 × distance, about 1.55° [computed]. — [fxguide](https://www.fxguide.com/?p=188260) (excerpt)
- **Meta.** Minimum 14 px, comfortable ≥18 px for its panel system. Sans-serif with a high x-height. Avoid pure white; keep light backgrounds at or below #DADADA. — [Meta typography](https://developers.meta.com/horizon/design/styles_typography/)
- **Texel-density rule [computed/inference].** A captured 1080-line UI texture is sampled about 1:1 when it spans 1080/PPD degrees vertically: about 43° on Quest 3, 54° on Quest 2, 77° on Index. Shrinking a crop below that minifies it, so use mipmaps or supersampling. Enlarging it wastes nothing but FOV. For a text crop, the binding constraint is *glyph angular height*, since you can't re-wrap a capture: `θ_glyph = glyph_px / capture_height_px × θ_panel_vertical`.

### 5.3 Field of view, head and eye rotation, vertical placement
- **Alex Chu (Samsung) and Mike Alger.**
  - Comfortable horizontal head turn is about 30°, maximum about 55°.
  - Up: 20° comfortable, 60° maximum. Down: 12° comfortable, 40° maximum.
  - The main content zone is about ±56°.
  — [Dummies: VR comfort zones](https://www.dummies.com/article/technology/programming-web-design/general-programming-web-design/virtual-reality-design-principles-starting-up-user-attention-and-comfort-zones-256440/) (excerpt)
- **Neck and shoulders.** Load is least for targets between eye level and 15° below. Optimal eye rotations are about ±15°, so put the most important content in a 30×30° area around the line of sight. — [NIU ergonomics](https://ceet.news.niu.edu/?p=1173), [DesignNews](https://www.designnews.com/design-software/researchers-are-looking-for-ways-to-make-vr-less-painful) (excerpts)
- **Meta panels.** Panels too large force head turns, so default to a comfortable size and let users scale. — [Meta panel sizing](https://developers.meta.com/horizon/essentials/horizon-os-panel-sizing/)

### 5.4 Head-locked vs world-locked vs body-locked vs lazy-follow
- **Microsoft tag-along.**
  - Content stays "a glance away", slides to the periphery and comes into view when looked at.
  - A rubber-band or damped spring gives weight.
  - Never keep moving after the head stops; that confuses the senses.
  - Wild springing is nauseating.
  — [Microsoft: Billboarding and tag-along](https://learn.microsoft.com/en-us/windows/mixed-reality/design/billboarding-and-tag-along)
- **MRTK 2 defaults (source).** — [Follow.cs](https://raw.githubusercontent.com/microsoft/MixedRealityToolkit-Unity/main/Assets/MRTK/SDK/Features/Utilities/Solvers/Follow.cs), [RadialView.cs](https://raw.githubusercontent.com/microsoft/MixedRealityToolkit-Unity/main/Assets/MRTK/SDK/Features/Utilities/Solvers/RadialView.cs), [Solver.cs](https://raw.githubusercontent.com/microsoft/MixedRealityToolkit-Unity/main/Assets/MRTK/SDK/Features/Utilities/Solvers/Solver.cs), [solver docs](https://learn.microsoft.com/en-us/windows/mixed-reality/mrtk-unity/mrtk2/features/ux-building-blocks/solvers/solver)
  - **Follow:** minDistance 0.3, maxDistance 0.9, defaultDistance 0.7 m. maxViewHorizontalDegrees **30**, maxViewVerticalDegrees **20**. Faces the tracked object while clamped. orientToControllerDeadzone 60°.
  - **RadialView:** min 1 m, max 2 m, maxViewDegrees **30**. Optional fixed vertical position −0.4 m. Reference direction is world-up, so it never rolls.
  - **Solver smoothing:** moveLerpTime and rotateLerpTime **0.1 s**, with t = dt/lerpTime per frame (≈0.094 s time constant at 90 Hz [computed]).
- **Unity XRI LazyFollow defaults (source).**
  - Movement speed 6, with ±25% speed variance.
  - Position dead-band grows from 0.01 m to **0.3 m** over 3 s.
  - Angle dead-band grows from 0.1° to **5°** over 3 s.
  - Rotation mode LookAt.
  — [XRI LazyFollow.cs (needle-mirror)](https://raw.githubusercontent.com/needle-mirror/com.unity.xr.interaction.toolkit/master/Runtime/UI/LazyFollow.cs); [docs](https://docs.unity3d.com/Packages/com.unity.xr.interaction.toolkit@3.2/manual/lazy-follow.html)
  - The "growing threshold" idea: tight while you are moving, loose once you settle.
- **Meta Spatial SDK Followable.** Defaults: minAngle −90, maxAngle 90, tolerance 0.2 (the distance change before it moves), speed 1.0. — [Meta Spatial SDK Followable](https://developers.meta.com/horizon/reference/spatial-sdk/v0.6.1/root/com.meta.spatial.toolkit/followable/) (excerpt)
- **Alyx subtitles.** Stay put, then snap when pushed out of view (§1).
- **Doom 3 VR.** The HUD is body-locked and pitch-revealed (§3.3).
- **UEVR.** Stage-anchored unless "follow view" is on (§3.1).
- **Ranking [inference]:**
  1. Body-attached (hand, wrist, weapon) for status.
  2. World-locked or recentred for menus.
  3. Yaw-lazy with a large dead zone for text.
  4. Head-locked only for things that really are head-mounted (mask visor, damage vignette).

### 5.5 Hand and wrist UI
- **MRTK hand menu.**
  - Use "Require flat hand" or "Use gaze activation" to avoid false activation.
  - Put content on the ulnar side of the hand.
  - World-lock complex menus by grab-and-pull, with re-attach by raising and looking at the palm.
  — [MRTK Hand menu](https://learn.microsoft.com/en-us/windows/mixed-reality/mrtk-unity/mrtk2/features/ux-building-blocks/hand-menu)
- **HandConstraintPalmUp defaults.**
  - The palm-up must be within an **80°** cone of the camera forward.
  - Flat-hand threshold 45°.
  - followHandCameraFacingThresholdAngle 60°.
  - Gaze activation proximity: eye 0.01 m² (about 0.1 m), head 0.04 m² (about 0.2 m) [computed sqrt].
  — [HandConstraintPalmUp.cs](https://raw.githubusercontent.com/microsoft/MixedRealityToolkit-Unity/main/Assets/MRTK/SDK/Features/Utilities/Solvers/HandConstraintPalmUp.cs)
- **Fatigue.** Holding a wrist UI up is tiring (the FO4VR Pip-Boy, §3.5). Keep wrist reads to glances; long reading belongs on a world-lockable or held surface [inference].

### 5.6 Subtitles
- **BBC R&D 2017/18 (24 participants).** Tested four behaviours: evenly-spaced fixed, follow-head immediately ("always visible"), follow with lag, and appear-in-front-then-fixed. The head-up "always visible" style performed best and was preferred. Discriminating themes were effort, missing out, obstruction, distraction and immersion. — [Brown et al. abstract](https://www.aminer.org/pub/5f0e60c09fced0a24bc466a7/exploring-subtitle-behaviour-for-video); [BBC R&D](https://bbc.in/2uJhCps) (excerpt)
- **ImAc and UAB.** Always-visible subtitles at bottom centre of the FOV were preferred and gave more presence than fixed ones. For off-screen speakers, an **arrow** next to the subtitle (shown only when the speaker is out of view) beat an always-on radar. — [UAB Media4All study](https://ddd.uab.cat/record/206285), [guidelines study](https://observatorio-cientifico.ua.es/documentos/6780207657a4a056e8ae0ae2) (excerpts)
- **Brescia-Zapata 2023.** Head-locked coloured subtitles were preferred. — [Salford repository](https://salford-repository.worktribe.com/output/2152391/subtitles-in-vr-360-video-results-from-an-eye-tracking-experiment) (excerpt)
- **Chirp (XR Access).** Exposes a follow delay and a caption distance. — [Equal Entry](https://equalentry.com/accessibility-virtual-reality-captions-open-source/)
- **Meta requirement.** VRC.Quest.Accessibility.1 asks for subtitle options. — search excerpt
- **NTNU.** Curved text layouts fit 155–167% more characters per line than flat ones. — [NTNU thesis](https://ntnuopen.ntnu.no/ntnu-xmlui/handle/11250/2617909)

### 5.7 Curved panels and compositor layers
- **Meta compositor layers.**
  - They are sampled once instead of twice, so they are noticeably sharper for text.
  - Shapes are quad, cylinder and cubemap. Use cylinder slices for large curved UIs.
  - There is no custom pixel shader. Quest allows up to 16 layers.
  — [Meta compositor layers](https://developers.meta.com/horizon/essentials/compositor-layers/), [Unity sample](https://developers.meta.com/vr/documentation/unity/unity-sample-compositor-layers), [Unreal sample](https://developers.meta.com/horizon/documentation/unreal/unreal-sample-layer)
- **Caveat [inference].** A layer composited over the projection layer is never occluded, so the depth-conflict rules in §5.1 apply.

### 5.8 Laser pointer vs direct touch
- **IEEE TVCG 2024.** Direct input was fastest for single-level menus and unaffected by item count. Raycast menus were among the slowest but rated most consistently usable. — search excerpt, [arXiv 2603.18435](https://arxiv.org/html/2603.18435)
- **Pfeuffer et al. 2020.** Direct controller selection was fast but more physically demanding. Raypointing adds wrist effort. Dwell trades eye fatigue for no physical effort. — [Pfeuffer VRST 2020](https://www.rz.unibw-muenchen.de/usable-security-and-privacy/publikationen/pdf/pfeuffer2020vrst.pdf) (excerpt)
- **Within-reach touch panels.** These beat raycasting on time and errors. — [ViewfinderVR arXiv 2110.02514](https://arxiv.org/pdf/2110.02514) (excerpt)
- **Shipped choices.** Alyx: laser primary, touch too. Doom 3 VR: touch default. UEVR: laser.
- **Hit targets.** Meta: ≥48 dp. Apple: centres ≥60 pt apart.

---

## 6. Pause and main menu presentation ("theatre mode")
- **UEVR.** The default is a 2 m tall panel at 2 m, about 53°×83° [computed]. An optional cylinder has a 90° central angle. (§3.1)
- **Titanfall 2 VR.** Cutscenes default to a floating 2D screen, with full-VR opt-in. (§2)
- **SteamVR.**
  - The 2016 Desktop Theater looked like a 10–12 ft screen seen from about 6 ft (≈80–85° [computed]). — [PCWorld](https://www.pcworld.com/article/3045272/virtual-reality/hands-on-steamvrs-desktop-theater-mode-plays-your-2d-games-on-a-giant-vr-screen.html) (excerpt)
  - The 2.1.1 beta "Theater Screen" replaces it; no size defaults are documented. — [Road to VR](https://roadtovr.com/steamvr-theater-screen-flatscreen-vr-games/)
- **vorpX.** Virtual Cinema Mode is a big screen in a virtual living room. — [Road to VR](https://roadtovr.com/vorpxs-new-virtual-cinema-lets-you-play-non-vr-games-on-a-virtual-big-screen/)
- **Delight VR.** Its default cinema screen is 2.5×1.5 m at 2 m (≈64° wide [computed]). — search excerpt
- **Real-world cinema standards.** SMPTE EG-18 calls for at least 30° horizontal (16:9). THX: 26° minimum acceptable, 36° recommended, 40° ideal. — [Audioholics](https://www.audioholics.com/hdtv-formats/how-to-find-the-right-size-tv), [KEF](https://ap.kef.com/blogs/news/calculate-the-ideal-tv-height) (excerpts)
- **Menus.**
  - About 1 m is comfortable for extended focus (Meta); LaValle says at least 2 m. A world-locked, recentre-on-open panel avoids head-locked discomfort (Apple, Microsoft). Blur or dim the scene behind it (Meta).
  - Skyrim VR's left-panned pause menu with no recentre was criticised (§3.5).
  - RE4 VR's flat-style tabbed menus were called clunky (§3.6).
  - Doom 3 VR puts the system menu on the hand-held PDA (§3.3).
- **Reconciling the menu numbers [inference].** Video wants about 40–50° wide, but flat-game menus need text at about 1°+. Flat UI text is usually 2–3% of screen height, so a menu panel needs roughly 35–45° vertical (60–80° wide at 16:9). That is why UEVR's defaults are huge. Use a curved panel so the edges stay at equal distance, and let the user scale it.

---

## 7. Implementation notes for a capture-based HUD (RustVR) [inference]
1. **Split the captured UI layer into elements.** Where the adapter sees each UI draw, record its screen-space bounding box (from VB positions or scissor) and texture/shader. Cluster the draws into the game's known elements by region and texture: subtitles (bottom-centre band), hints/prompts (centre-low), ammo block (bottom-right), item icons, crosshair (exact centre, tiny), fades, and full-screen overlays. Render each cluster into its own atlas rect. Fall back to fixed-UV crops measured once from captures at 16:9, for a game that lays out at the screen aspect.
2. **Let the game's own visibility drive ours** ("game is source of truth"). Show an element only while the game draws it, judged by its region's coverage or alpha. A minimal HUD that fades its ammo in and out on use, or a no-HUD mode's empty layer, then comes through as the game has it. Gaze gating is an extra filter on top, not a replacement.
3. **Prefer game data over pixels where we can.** Hooking the subtitle string, the use-target entity and the ammo counts lets us re-layout and wrap text at a legible size, and anchor prompts on the real object. Pixels are the fallback.
4. **Prefer the game's own diegetic meshes.** Use its first-person props (a watch, a journal, a lighter), attached to the tracked hands, over textured quads: they already show their readouts in-engine.
5. **Rendering.**
   - Text strips and menus go on OpenXR quad or cylinder composition layers for sharpness.
   - Hand, wrist and weapon quads render in-scene with depth testing against the hands. Keep a small depth bias so world geometry doesn't eat them.
   - Mipmap the captured texture, or supersample it, when a crop is minified.
6. **Depth conflict for world-space text.**
   - Sample the game's depth in the strip's screen rect, nearest 5th-percentile value d_min.
   - If d_min < d_panel + 0.1 m, move the strip to max(0.5 m, d_min − 0.1 m), scaling it so its angular size stays constant.
   - Smooth this with τ = 0.3 s and 0.15 m hysteresis, so the depth doesn't pump (Apple and Microsoft both warn against frequent depth changes).
   - Add a 60–70% dark backing plate with a soft halo (per Adobe's halo-blur idea).
7. **Snap turn and recentre.** Rotate every lazy or world-locked UI anchor by the same yaw delta instantly. Never roll UI; keep it world-up.

---

## 8. Recommendation table: a flat game's HUD element → VR placement

Sizes are given in both metres and degrees [computed]. Defaults should be user-adjustable: scale 0.5–2×, distance ±50%, per-element hide, left-handed mirror.

| # | Element (flat source) | VR placement / attach point | Distance from eye | Size | Follow behaviour | Visibility rules | Precedent |
|---|---|---|---|---|---|---|---|
| 1 | **Ammo counter** (mag + reserve + ammo type; bottom-right HUD crop or hooked counts) | Quad on the **weapon's left receiver side** (hand-side for left-handers), 2 cm proud of the surface, tilted 20° toward the eye when held at the chest. Optional: also on the back of the support hand. | 0.30–0.55 m (held at chest ≈0.45 m) | 6×3 cm → ≈7.6°×3.8° at 0.45 m. Digits ≈1.8 cm → ≈2.3°. | Rigid to the weapon | Shown while the game shows its ammo element, **or** when the panel is within **30°** of gaze centre **and** faces the eye within 70°. Always shown during reload and for **2.5 s** after the count changes. Fade in 0.15 s, out 0.4 s. Hidden while the game hides it (a no-HUD mode). | Alyx, HL2VR, Doom 3 VR, Roboquest VR |
| 2 | **Timers** (a filter's or an air supply's time, the clock) | **Left-wrist watch face**, the game's own watch mesh where it has one. Otherwise a 4 cm decal on the inner or outer wrist. | 0.30–0.40 m when raised | 4 cm face → ≈6.5° at 0.35 m. Digits ≥8 mm → ≥1.3°. | Rigid to the left hand or wrist | Always physically present. The readout and backlight come alive when the watch is **within 25° of gaze, the face normal is within 50° of the eye vector, held ≥0.15 s** (cf. MRTK palm cone 80° and head-gaze 0.2 m). Optionally trigger the game's own watch check so its logic runs. Keep the game's beeps. | Metro Awakening, Saints & Sinners, RE4 VR, LMU study |
| 3 | **Light/stealth indicator** | Same watch: the game's lights or LED | as #2 | as #2 | Rigid | Always on (it's a light); visible in peripheral vision. **No head-locked copy.** Optional, off by default: a faint 2 mm LED-glow scale-up, or a haptic tick on the off-hand when the state changes. | Metro Awakening |
| 4 | **Health** shown as damage effects | **No HUD.** Keep the game's red pulse/vignette and breathing. If the vignette arrives in the 2D layer, draw it as a full-FOV, head-locked, soft-edged overlay at ≤70% of the game's alpha (the one legit head-locked layer). | n/a (head-locked; vignette at ∞ disparity) | full FOV, edges only | Head-locked | Exactly as the game. | Alyx keeps health off-screen |
| 5 | **Subtitles** | **Caption strip**: curved (cylinder) or quad, centred **13° below head-forward**, never rolled | **1.5 m** default, adaptive 0.5–1.5 m per §7.6 | Glyph height target **1.3°** (≈3.4 cm at 1.5 m); min 0.9°. **Max width 40°** (≈1.1 m at 1.5 m). If a crop would exceed 40° at 1.3° glyphs, shrink toward a 0.9° floor; if hooked text, wrap to ≤2 lines. Dark 65% backing. | **Yaw-lazy**: dead zone **±20°**, then catch up with τ = **0.15 s**, or Alyx-style fade-snap (out 0.08 s, move, in 0.12 s). **Pitch-lazy**: dead zone ±15°. Position locked 1:1 to head translation. Snap-turn applies the yaw delta instantly. | Shown while the game draws subtitles. Speaker-direction arrow beside the line when the speaker is >40° off-gaze (needs the hooked source position; optional). | BBC, ImAc (always visible, bottom, arrows), Alyx (snap), TF2 VR #34 (cap width) |
| 6 | **Hint / interaction prompts** ("Hold E to …", tutorial hints) | **On the use-target object** if the game exposes it: world-locked at the object, billboarded about yaw, constant angular size. Fallback: caption strip upper row at **6° below head-forward**. Remap key glyphs to VR controller glyphs (or attach a small controller-button icon to the relevant hand). | object depth clamped ≥0.5 m; fallback 1.5 m | Glyph **1.2°** constant (≈2.5 cm at 1.2 m) | World-locked to the object; fallback same lazy rules as #5 | While the game draws the prompt. Tutorial-type hints get a global toggle (cf. Alyx `sv_gameinstructor_disable`, TF2 VR #88 highlight toggle). | TF2 VR (VR glyphs praised), Metro Awakening (text + highlight), Alyx (game instructor) |
| 7 | **Crosshair** | **Remove** the 2D crosshair from the capture. Offer: none (default with iron sights) / dot / laser, drawn **at the muzzle-ray hit depth**. | hit depth (≤30 m cap) | Dot **0.2°** constant angular size; laser 2 mm beam, dot at the end | Rigid to the weapon ray | Only while a weapon is held. Hide when ADS (sights aligned) if the user wants. | Meta reticle rule; Doom 3 VR laser default; TF2 VR laser complaint; existing `reticle` setting |
| 8 | **Item counters** (medkits, consumables, throwables) | **Backpack/belt:** count chips on the body slots where those items are grabbed. Interim: show them on the watch strap or the inner left forearm as small icons. | 0.35–0.6 m | Icons 1.5 cm (≈2–2.5°) | Rigid to body slot or wrist | While the game shows them (on use or pickup), or when gazing at the slot within 30°. | Metro Awakening (counters on backpack), Alyx (true-count items) |
| 9 | **Weapon/inventory wheel** (a held inventory key, D-pad directions) | On open, **freeze a world-locked panel** at the dominant hand's position (or 0.45 m in front along body yaw, 20° below horizon), facing the eye. Select with the thumbstick direction (passed to the game) or by pointing the hand. | ≈0.45 m | Ø 0.30 m → ≈37° | World-locked while open (no follow) | While the hold-key is held; close on release. Paused only if the game pauses. | Alyx quick-select on weapon hand, HL2VR radial menu, MRTK world-lock on grab |
| 10 | **Journal / clipboard + compass + objectives** | **Held in the off hand**: the game's journal model, or the captured full-screen journal texture on a 0.28×0.20 m board in the left hand, tilted toward the face. Option "fix in space" (Doom 3 `vr_PDAfixLocation`). | 0.35–0.45 m | 0.28×0.20 m → ≈39°×28° at 0.40 m | Rigid to the hand (or world-locked on option) | While the game's journal is open (its hold-to-open key). The compass is live because it is the game's. | Doom 3 VR (PDA in hand, touch GUI), Saints & Sinners (chest journal) |
| 11 | **Pickup / notification icons** ("+ammo", save icon, objective updated) | Caption strip, right end (+15° yaw, −10°), or on the item's slot/weapon if ammo | 1.5 m | ≤1.2° glyphs, ≤8° wide | As #5 | While the game shows them (it times them). | Roboquest VR wrist, Doom 3 per-element toggles |
| 12 | **Pause / main / options menu** | **World-locked curved panel** placed at the recentred yaw on open; centre at eye height, 3° below horizon. The 3D scene is dimmed 50% or blurred (keep the last frame), or a dark void in the main menu. | Cylinder radius **2.0 m** (1.2–3.0) | **65° wide** default (arc ≈2.27 m) × ≈35° tall at 16:9; user 45–90°. Rule: smallest menu glyph ≥1.0°. | World-locked; re-anchor only on reopen or if the user turns >70° (the runtime's rule when this was written) | When the game shows a menu. Laser from the dominant hand (trigger = click, stick = scroll, A = accept, B = back), aim blocked while pointing (UEVR). Runtime settings panel to the right, angled 20° in (existing). | UEVR (stage-anchored, laser, aim blocked), Alyx (laser + touch), Meta (blur behind, 1 m+), Skyrim VR (recentre complaint) |
| 13 | **Loading screens, pre-rendered videos, credits** | Same world-locked panel in a dark void, but smaller: **50° wide** (≈2.33 m at 2.5 m) for video | 2.5 m | 50°×≈29° | World-locked | During loads and videos. | THX 36–40°, SMPTE ≥30°, TF2 VR 2D panel |
| 14 | **Scripted first-person sequences** (camera-driven) | Default: keep head tracking and play the game camera's translation and yaw only. Option: a "comfort" 2D panel per TF2 VR. | n/a / 2.5 m | as #13 | n/a | per setting (existing `follow scripted camera animations`) | TF2 VR cutscene toggle |
| 15 | **Visor effects** (a mask's fog, cracks, wipe), **night-vision tint** | If 3D viewmodel geometry: leave it, it renders in stereo. If 2D layer: head-locked full-FOV overlay at zero disparity, so it reads as "on the glass", with edges kept peripheral. | n/a | full FOV | Head-locked (it *is* on your head) | As the game | Metro Awakening (physical wipe), Meta (head-locked only if justified) [inference] |

### Parameter block (proposed defaults for `settings.rs` / an adapter profile)
```toml
[ui.caption_strip]            # subtitles, hints fallback, notifications
distance_m = 1.5; min_distance_m = 0.5; depth_adapt = true; depth_hysteresis_m = 0.15; depth_tau_s = 0.3
pitch_offset_deg = -13.0      # subtitles row; hints row at -6
glyph_target_deg = 1.3; glyph_min_deg = 0.9; max_width_deg = 40.0; backing_alpha = 0.65; curved = true
yaw_deadzone_deg = 20.0; pitch_deadzone_deg = 15.0; catchup_tau_s = 0.15; snap_mode = false  # true = Alyx-style fade-snap
fade_out_s = 0.08; fade_in_s = 0.12; never_roll = true; speaker_arrow_off_gaze_deg = 40.0

[ui.weapon_ammo]
size_m = [0.06, 0.03]; offset = "left_receiver"; surface_offset_m = 0.02; tilt_to_eye_deg = 20
gaze_cone_deg = 30.0; face_cone_deg = 70.0; show_after_change_s = 2.5; fade_in_s = 0.15; fade_out_s = 0.4

[ui.watch]                    # prefer the game's watch mesh on the left hand
face_diameter_m = 0.04; gaze_cone_deg = 25.0; face_cone_deg = 50.0; dwell_s = 0.15; trigger_game_watch_check = true

[ui.prompt]
anchor = "use_target"; min_depth_m = 0.5; glyph_deg = 1.2; fallback_row_pitch_deg = -6.0; remap_glyphs = true; tutorial_hints = true

[ui.reticle]
mode = "none"   # none | dot | laser
dot_deg = 0.2; at_hit_depth = true; max_range_m = 30.0

[ui.wheel]
anchor = "dominant_hand"; distance_m = 0.45; diameter_m = 0.30; world_lock_while_open = true

[ui.journal]
hand = "off"; size_m = [0.28, 0.20]; tilt_to_face = true; fix_in_space = false

[ui.menu]
shape = "cylinder"; radius_m = 2.0; width_deg = 65.0; min_glyph_deg = 1.0; pitch_deg = -3.0
reanchor_turn_deg = 70.0; dim_scene = 0.5; pointer = "dominant_hand_laser"; block_aim_while_pointing = true

[ui.theatre]                  # loads/videos
distance_m = 2.5; width_deg = 50.0; void = true
```

### Why these numbers [inference, from the sources above]
- **Eye-relative distances.** The 0.30–0.45 m distances for wrist and weapon follow Microsoft's 0.45 m near-interaction band. Sizes are scaled up from its 0.65–0.8° comfortable band to ≥1.2°, per the Frontiers figures for 13–25 PPD headsets.
- **1.5 m caption distance.** It sits between Meta's 1 m and Microsoft's 1.25 m+ comfort zone. It is short enough to stay in front of most corridor walls, with depth adaptation catching the rest.
- **−13° / −6° rows.** Comfortable downward gaze is 0 to −15° and Chu's comfortable down rotation is 12°. Keeping the rows off-centre avoids covering the aim point (BBC's "obstruction" theme).
- **20° yaw dead zone, 0.15 s catch-up.** This sits between MRTK's 30° / 0.1 s and XRI's tight 5°. Microsoft warns against content that keeps moving after the head stops, which argues for short catch-up and no overshoot (critically damped). The runtime's 0.25 s with no dead zone, at the time, meant the panel was always moving.
- **40° max subtitle width.** About ±20° keeps a line inside comfortable eye rotation (±15°, plus a little head) and fixes the TF2 VR #34 failure.
- **65° curved menu at 2 m.** Flat UIs need about 35–40° vertical for ≈1° text; about 43° vertical would sample 1080p at 1:1 on Quest 3. Curvature keeps the edges at equal depth, and the user can scale it.
- **Body-attached status with gaze gating.** This combines Alyx (lights blink only when raised), Doom 3 VR (pitch-revealed HUD), and Roboquest/RE4/Metro Awakening (wrist and watch). The LMU study found the wristwatch fits single-player story games.

---

## 9. Gaps / not found
- Titanfall 2 VR: no detail on the smart pistol, pilot HUD layout, ammo counter placement, or Titan HUD geometry. The mod is closed source and reviews don't describe it.
- No exact measurements for Alyx's subtitle panel or menu distances and sizes.
- No numeric defaults for SteamVR Theater Screen or vorpX Virtual Cinema.
- No Flat2VR developer write-up on HUD conversion beyond Roboquest quotes.
- No HUD details for BioShock VR mods.
- Nexus Mods pages (Idle Hands, FO4VR HUD mods) returned 403, so those claims come from earlier notes or excerpts.
- Two Meta developer pages (panel sizing, panels) contained embedded instructions aimed at AI agents. I ignored them as data.
