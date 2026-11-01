# Weapon handling, reloading and inventory/holster systems in VR shooters

> A research snapshot (2026-10-21), not a description of the framework's code: what was built is in
> `docs/`.

Scope: how the most praised VR shooters handle guns, reloading, ammo, inventory, aiming, melee and grenades, what players praise and complain about, and the parameter values that open frameworks and open-source ports actually ship, for building a game-agnostic weapon and inventory system in a VR runtime. Researched October 2026.

Source quality legend: **[primary-code]** = shipped source or asset values read directly from a repository; **[primary-doc]** = official docs or manual; **[review]** = press review or analysis; **[community]** = forum, wiki or mod page (anecdotal, may be dated). Where a value comes from a hobby or open-source port rather than a commercial game, that is stated.

---

## 1. Reloading: how the praised games do it, insertion assist, chambering, shotguns and revolvers, quick-reload options, and what feels great vs tedious

### Takeaway
The best-regarded reloads (Half-Life: Alyx, H3VR, Pavlov) keep the physical sequence (eject by button, fetch, insert, rack) but cut repetition and fumbling: a generous magwell snap, ammo auto-matched to the held gun, multi-shell grabs, and clear audio cues for chamber state. Every well-regarded mod or port also ships a lighter mode (H3VR's "Easy Mag Loading", HL2VR's "Quick Reload", Arizona Sunshine 2's auto/easy reloads, Hard Bullet's auto-reload mode). The complaints are about fumbling (dropped mags, tiny bolts), too many steps, and input misreads, not about manual reloading as such.

### Cited Findings

**Half-Life: Alyx (Valve) [review]**
- Pistol chambering is modelled: the first round has to be chambered, removing the mag afterwards leaves a round ready, and on empty "the chamber stays open so it's ready to receive a new bullet." Distinct sounds mark "mag empty, last round chambered" and "fired last round, chamber open". The ideal reload is after the second-to-last shot, so a new mag can go in without re-racking. — [Road to VR, Inside XR Design: Alyx details](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2)
- Each weapon has its own diegetic ammo counter (segmented on the pistol, numeric on the shotgun, radial on the SMG). The reviewer says this encourages players to "become intimately familiar with their weapons." — [Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2)
- The shotgun holds 7 shells but Alyx pulls **2 shells per reach**, so "a full reload happens in four gestures instead of seven." One shell is pulled when only one slot is left. A unique sound plays when the last shell goes in, so players don't throw extra shells on the floor. The breech closes by pushing it from several spots or by flicking. Custom hand poses are used while pushing shells in. — [Road to VR shotguns](https://www.roadtovr.com/inside-xr-design-best-shotguns-in-vr-what-makes-them-great-feedback/2/); [Road to VR Alyx](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2)
- The SMG has no chambering step ("sci-fi space magic"). The reviewer criticised a mag part simply falling to the ground and would rather "rip off that other part with a nice, satisfying sound." — [Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2)
- Weapons collide with and push world objects (doors, props, piano keys) instead of clipping. The reviewer frames this as keeping the weapon in the player's body schema. — [Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2)
- Criticism: a Forbes column called the cycle tedious ("release the clip, reach over your back, grab a new clip and insert it into your gun"). — [Forbes](https://www.forbes.com/sites/erikkain/2020/04/14/half-life-alyx-makes-me-sick/)

**Hot Dogs, Horseshoes & Hand Grenades (H3VR) [community wiki]**
- The Options Panel has **"Use Easy Mag Loading"**, which "automatically insert[s] mags when they're 'close enough' to the magwell" (for controller bumping or poor tracking), and **"Use Alternate Magazine Poses"**, which changes the mag hold angle so controllers bump less. — [H3VR wiki: Options Panel](https://h3vr.fandom.com/wiki/Options_Panel)
- **"Grip Button Drop Mode"** lets the player drop held firearms instantly on grip release, after a one-second delay, or only on a double-click. This guards against accidental drops. — [H3VR wiki: Options Panel](https://h3vr.fandom.com/wiki/Options_Panel)
- "Haptic Buzz for Interactions" buzzes on every firearm interaction. — [H3VR wiki: Options Panel](https://h3vr.fandom.com/wiki/Options_Panel)
- Speedloaders work only with swing-out and top-break revolvers. Capacities are per caliber, for example .357 Magnum 6 or 8, .38 Special 5 or 6, .44 Magnum 6, 12 gauge 5. — [H3VR wiki: Speedloader](https://h3vr.fandom.com/wiki/Speedloader)
- H3VR is the usual reference point in community discussion. A Pavlov player said they wanted "the same awesome reloads in Pavlov" as in H3VR. — [Steam (Pavlov forum), via search](https://steamcommunity.com/app/555160/discussions/0/1744511953045095220)

**Pavlov VR [community]**
- Older official wiki controls: mag drop is on the touchpad. On Vive, the player holds the trigger with the non-dominant hand **left of the hip** to grab a new magazine. On Oculus, "the magazine snaps into place when brought to the gun's port." — [Pavlov VR Wiki: Game Controls (site was returning a database error when checked; content from search index)](http://wiki.pavlov-vr.com/index.php?title=Game_Controls)
- Mag removal differs by weapon: the AK-47 needs a manual grab, the M4 drops on a button press. Players drop the mag and grab a new one from the hip pouch at the same time. Racking with a round still chambered wastes a round. On pistols, pressing up on the touchpad hits the slide release instead of racking. — [Steam: "How the F#@& do you reload?"](https://steamcommunity.com/app/555160/discussions/0/135510194252870489/) and related Pavlov threads
- Off-hand reloading needs the mag-drop and bolt-release actions bound to the off hand. — [Steam: Offhand reload?](https://steamcommunity.com/app/555160/discussions/0/1697175413680135847/)

**Half-Life 2: VR Mod (Source VR Mod Team) [primary-doc]**
- Manual reload (default): press A to eject (any remaining rounds go with the mag), then hold the off-hand grip **near the shoulder** to fetch a fresh mag. "Once ejected, a magazine can't be held, only added back to inventory", and a caught or picked-up mag is added back to inventory immediately. Per-weapon actions: pistol and SMG mags go into the well; revolver clips load after tilting the gun to empty it; the pulse-rifle mag snaps on underneath; shotgun shells go through a bottom port; crossbow bolts are nocked; the RPG rocket slides in. After an empty mag, the pistol slide, SMG charging handle or shotgun pump (back and forward) chambers a round. — [HL2VR manual](https://halflife2vr.com/manual/)
- **Quick Reload** (Difficulty options): eject, then "drop the weapon to your waist, aim at the ground, and wait." "Any unspent ammo in the previous magazine will be automatically recovered." — [HL2VR manual](https://halflife2vr.com/manual/)
- The shotgun needs two hands: pull the off hand toward the primary hand, then push it back. The gun won't fire unless both strokes are done. — [HL2VR manual](https://halflife2vr.com/manual/)

**Titanfall 2 VR (CircuitLord mod) [review]**
- Reloads are fully manual: eject, grab a fresh mag from the backpack, insert, and cock with the other hand. Steps differ per weapon and are often learned by trial and error. The Mastiff loads shell by shell. The Kraber's mag is on the left but the bolt must be worked after every shot ("The Kraber is a huge pain in the ass to use. It should be."). The Wingman has a gesture-based reload. — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord); [Road to VR](https://www.roadtovr.com/titanfall-2-vr-mod-circuitlord/)
- UploadVR's review: even with practice, "every now and then you're going to fumble with weapons or screw up a reload." Dropped mags are a problem, and some parts (bolts) are "a little too small to grip easily." — [UploadVR review](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/)
- As of Oct 17 2026 the mod is at 1.0.17, with a 1.0.20 beta. — [vr.org install guide (Oct 2026), via search](https://vr.org/articles/titanfall-2-vr-mod-how-to-install-circuitlord-northstar-2026)

**Arizona Sunshine 2 [review/community]**
- Reloads are manual: a button ejects the mag, the new mag goes in by hand, then a handle is pulled or the gun loaded depending on type. The sawn-off is broken open, shells are inserted one at a time, and the barrels snapped shut. — [Analog Stick Gaming review](https://www.analogstickgaming.com/game-reviews/2023/12/11/arizona-sunshine-2); [MKAU Gaming review](https://www.mkaugaming.com/all-review-list/arizona-sunshine-2-ps-vr2-review/) (statement-level attribution between these two not individually verified)
- There is an auto-reload option: dump the mag and **slam the gun into the waist holster** to reload. Another guide recommends enabling "Easy Reloads" on harder modes. — [PSNProfiles guide](https://psnprofiles.com/guide/19860-arizona-sunshine-2-trophy-guide); [ZyberVR guide (promotional blog, treat with caution)](https://zybervr.com/blogs/news/arizona-sunshine-2-essential-guide-survival-tips-gameplay-basics)
- Players complain that clips "disappear out of your hands" while being inserted, which one attributes to the chest-level ammo storage. — [Steam: "Janky reload design"](https://steamcommunity.com/app/1540210/discussions/0/4035851529331474006/)

**Metro Awakening [review/community]**
- Reloading is manual: remove the empty mag, reach to the chest for a new one and insert it. One review says you can just "drag a new clip from your chest and it will tend to the rest." Misreads: the game "sometimes thought they were trying to cock the gun rather than grab ammo", and players grabbed the backpack or a syringe while reaching for ammo mid-fight. — [Mixed review](https://mixed-news.com/en/metro-awakening-review/); [UploadVR review](https://www.uploadvr.com/metro-awakening-review-a-mesmerizing-tale-of-survival-in-moscows-dark-underworld/); [GamingTrend review](https://gamingtrend.com/reviews/metro-awakening-review-the-metro-series-arrives-in-vr-2/) (statement-level attribution among these not individually verified)

**Contractors [community]**
- Some guns let you pull the mag straight out; others need the mag-release button (the menu button on WMR and Vive, which players called inconvenient). You can reload a gun while it is holstered by sliding the mag into it, and you can reload a gun held in the off hand. — [UploadVR Contractors beginner's guide](https://www.uploadvr.com/contractors-vr-beginners-guide-tips/); [Steam: how to reload?](https://steamcommunity.com/app/963930/discussions/0/1743355067110392489/)
- Revolver: press the button and flick it sideways to swing the cylinder out, then reload it. — [Steam: Cant reload revolver](https://steamcommunity.com/app/963930/discussions/3/1778262124923033324/)

**Hard Bullet [community/press]**
- There are two gunplay modes: a realistic one with manual reloads and two-handed recoil handling, and one with automatic reloads that allows dual-wielding. — [UploadVR (Quest 3 launch)](https://www.uploadvr.com/vr-action-shooter-hard-bullet-finds-a-new-home-on-quest-3/) and store descriptions via search

**Bonelab / Boneworks [community]**
- A Bonelab player complaint: "just because VR games are 'immersive' doesn't mean we need 15 jitsu moves to reload a gun." — [Steam Bonelab discussion](https://steamcommunity.com/app/1592190/discussions/0/3395175706735256154/?ctp=2)

**Other references**
- Gun Club VR: a firearms instructor disliked a reload that needs the left controller held, since the left hand can't then be a support hand, and asked for a one-button option. The devs declined a flat auto-reload but floated throwing the gun away and drawing a freshly loaded one from the holster. — [Steam Gun Club VR thread](https://steamcommunity.com/app/691320/discussions/0/1733210552675297783/)
- Stormland skips mag swaps: when a gun runs dry you rip it in half for ammo, which suits a faster pace. — [Road to VR, Inside XR Design: Stormland](https://www.roadtovr.com/inside-vr-design-stormland-weapons-locomotion-open-world-analysis/2/)
- Fallout 4 VR players are split between button reloads and interactive reloads ("interactive reloading feels way better then button press reloading"). — [Steam FO4VR thread](https://steamcommunity.com/app/611660/discussions/0/1696044439572686389)

**Framework mechanics that encode insertion and slides [primary-doc/primary-code]**
- Auto Hand "Place Point" (used for magwells and holsters) has these settings: `Placed Radius` (distance from the placed offset where placement is allowed); `Force Place` (snaps in as soon as an item enters the area, without a release); `Force Hand Release`; `Parent on Place`; `Make Placed Kinematic`; `Placed Joint Link` with `Joint Break Force` (a breakable fixed joint instead of kinematic parenting); name, tag, layer and list filters; `Held Place Only`; and OnPlace, OnRemove, OnHighlight and OnStopHighlight events. — [Auto Hand docs: Place Point](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/place-point)
- The Auto Hand "Auto Ammo" component holds `Current Ammo` per clip, and a mag only counts as ammo once it sits in the gun's `Magazine Point` Place Point. — [Auto Hand docs: Auto Ammo](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/extras/auto-gun/auto-ammo)
- The Auto Hand Physics Gadget Slider (for slides and charging handles) runs on a ConfigurableJoint. `Play Range` is a dead zone: with 0.1, "the gadget must be moved 10% of its range to yield a result". `Threshold` (minimum 0.01) fires OnMax at 0.9 and OnMin at -0.9 for a 0.1 threshold. `Step Count` gives detents (2 = on/off). — [Auto Hand docs: Physics Gadget Slider](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/extras/physics-gadgets/physics-gadget-slider)
- Unity XR Interaction Toolkit 3.7 `XRSocketInteractor` defaults: `m_SocketSnappingRadius = 0.1f` (metres), `m_RecycleDelayTime = 1f` s (delay before a socket accepts an object again after removal), hover scale 1, `m_PauseOnTriggerStay = true`. When hover socket snapping is enabled, the recycle delay is disabled "to prevent snap flickering." — [XRSocketInteractor.cs (needle-mirror of com.unity.xr.interaction.toolkit 3.7.0-pre.1)](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactors/XRSocketInteractor.cs)
- The Unreal VR Weaponsmith plugin simulates charging handles, slides, bolts, triggers and safeties from hand movement and lists "dual spring-damper recoil solvers". — [Unreal forums: VR Weaponsmith Plugin](https://forums.unrealengine.com/t/bo-tje-vr-weaponsmith-plugin/2746247)

### Inferences
- **Use an insertion state machine, not one trigger.** Every praised system has the same shape: (a) an approach volume around the magwell; (b) an alignment gate (mag axis roughly parallel to the well axis); (c) "seat" on contact or release, with a click sound and a haptic tick; (d) a separate chamber state (empty / chambered / slide locked back). Model `chambered: bool`, `slide_locked: bool` and `mag: Option<MagId>` independently, as Alyx does. That gives tactical reloads (no rack needed if `chambered`) and readable audio cues for free.
- **Assist tiers (estimate, not sourced values):** a strict mode with an approach radius of about 3–5 cm and alignment within about 20–30°; an "Easy Mag Loading" mode (H3VR wording) with about 8–12 cm, about 45°, and snapping without exact alignment. XRI's default socket snap radius of 0.10 m is a reasonable upper bound for an easy mode. Snap with a short positional lerp (about 50–100 ms, an estimate) rather than teleporting, so it reads as "guided" rather than "magic".
- **Cut repetition, not physicality.** Multi-round grabs (Alyx: 2 shells), speedloaders and stripper clips, and Contractors-style "reload into a holstered gun" all keep the gesture while reducing the count. Cap any reload at about 3–4 distinct gestures in the default mode (inferred from Alyx's 7→4 reasoning and the "15 jitsu moves" complaint).
- **Ship a quick-reload accessibility mode.** Two tested patterns: HL2VR's "eject, lower the gun to the waist or point it at the ground, wait" with ammo recovery, and AS2's "eject, slam the gun into the waist holster". Both use a pose plus a dwell, not a button, so they still feel embodied.
- **Disambiguate grabs by context.** Metro's misreads (ammo grab read as a rack; backpack grabbed instead of ammo) argue for priority rules: if the other hand holds a gun and the hand is in the ammo zone, ammo wins over a nearby grabbable; rack handles should only be grabbable when the hand approaches from behind the slide.

### Gaps
- No primary source (Valve talk, commentary or code) gives Alyx's magwell snap radius, alignment tolerance or slide-pull distance. Alyx's internal parameters are undocumented.
- Pavlov's, H3VR's and Bonelab's actual snap radii and angles are not published. H3VR's "close enough" threshold is unquantified.
- No source gives real slide-pull distances in metres for any game. A sensible default is to scale to the real firearm's slide travel (an estimate: about 2–3 cm for pistols, up to 10+ cm for rifle charging handles), with a "complete" threshold at about 90% of travel (matching the Auto Hand Threshold 0.1 example).
- Into the Radius and Metro Awakening chambering and mag-check details were not found in reliable sources.

---

## 2. Ammo sources: backpack over the shoulder, belt and chest pouches, infinite vs counted ammo, auto-matching, wrist pockets

### Takeaway
The dominant pattern is one ammo grab zone that returns a magazine matched to the gun in the other hand (Alyx backpack, Boneworks/Bonelab belt pouch, Arizona Sunshine 2 chest, Pavlov hip pouch, HL2VR shoulder), with ammo counted as an abstract pool rather than as physical magazines. Simulation-leaning games (H3VR, Into the Radius) use physical mags in physical slots. H3VR makes infinite ammo an explicit per-slot toggle ("spawn-locking").

### Cited Findings
- **Alyx backpack:** reaching an empty hand over the shoulder makes Alyx grab a mag for the gun in the main hand, and pistol ammo when nothing is held. The backpack stores only ammo and resin. Grenades and syringes go in the wrist pockets, one item per wrist (sources disagree on wording, but agree on two pocket slots total). — [gameplay.tips Alyx guide](https://gameplay.tips/guides/7161-half-life-alyx.html); [VR One Alyx tips](https://www.vrone.co.uk/articles/half-life-alyx-tips); [WayTooManyGames review](https://waytoomany.games/2020/05/27/review-half-life-alyx/)
- One review notes that ammo and resin can only be stored over the left shoulder (the off-hand side). — [WayTooManyGames review](https://waytoomany.games/2020/05/27/review-half-life-alyx/) (via search summary)
- Alyx players found the backpack faster than picking hip mags would be. Early on, players reported failures stowing clips over the shoulder on some headsets, and the backpack not accepting items until a story gate was passed. — [Steam Alyx thread](https://steamcommunity.com/app/546560/discussions/0/1861616237339477679); [Steam: can't drop ammo in backpack](https://steamcommunity.com/app/546560/discussions/0/1861616360699221193/)
- Alyx won't accept partly used magazines back into the backpack. Players see this as a risk/reward trade-off. One forum comment claims a threshold of about 7 rounds before partial mags are accepted (unverified). — [Steam: half-full mags thread](https://steamcommunity.com/app/546560/discussions/0/2143091389818821897); [Steam: Body Holsters workshop discussion](https://steamcommunity.com/sharedfiles/filedetails/?id=3144612716)
- **Boneworks/Bonelab:** the inventory is 5 body slots plus ammo pouches, and the pouch shows the count for each ammo type. The inventory can also be opened from a radial menu, which shows circles matching each body slot. — [Boneworks/Bonelab wiki: Inventory](https://boneworks.fandom.com/wiki/Inventory)
- Boneworks: dropping mags into the belt sorts them, dropped mags don't reduce carried ammo and eventually despawn, and "the ammo belt will also detect which weapon is in your hands" so you get the right mag type. — [TV Tropes: Boneworks (community)](https://tvtropes.org/pmwiki/pmwiki.php/VideoGame/Boneworks)
- Bonelab tracks Light, Medium and Heavy ammo pools separately (shown by a mod that toggles each). The "AntiMagCodeMod" disables the ammo pouch so players must holster physical mags instead, which shows the pouch is an abstract pool. — [Thunderstore: Infinite Ammo](https://old.thunderstore.io/c/bonelab/p/Dusstt22sTeam/Infinite_Ammo/); [Thunderstore: AntiMagCodeMod](https://thunderstore.io/c/bonelab/p/Dommygutman/AntiMagCodeMod/)
- **Arizona Sunshine 2:** all ammo is stored on the chest, and grabbing there gives the ammo for the gun in hand. — [Analog Stick Gaming review](https://www.analogstickgaming.com/game-reviews/2023/12/11/arizona-sunshine-2) (via search summary)
- **Metro Awakening:** ammo comes from the chest. Everything else carried (syringes and so on) hangs off the backpack and is pulled off physically. — [Mixed review](https://mixed-news.com/en/metro-awakening-review/); [UploadVR review](https://www.uploadvr.com/metro-awakening-review-a-mesmerizing-tale-of-survival-in-moscows-dark-underworld/)
- **Pavlov:** belt pouches (lower left, plus a right-side pouch) act as an effectively limitless ammo source. — [Steam Pavlov threads](https://steamcommunity.com/app/555160/discussions/0/135510194252870489/)
- **H3VR spawn-locking:** put a cartridge or mag in a quickbelt slot, hover and press the touchpad or B, and the slot turns blue. Every pull then spawns a duplicate. Harder scenes disable it. Rounds can be stacked in one slot: "6 shotgun shells, 5-10 rifle cartridges, 10 pistol cartridges, or 4 40mm grenades." — [H3VR wiki: Spawn-Locking](https://h3vr.fandom.com/wiki/Spawn-Locking)
- **Into the Radius (1):** up to four mags in vest pouches. Players called four too few and kept mags in the general pocket pouch. — [IMFDB: Into the Radius](https://www.imfdb.org/wiki/Into_the_Radius); [Steam ITR thread](https://steamcommunity.com/app/1012790/discussions/5/3823032780423413079)
- **Into the Radius 2:** gear has capacities rather than fixed slots, for example Recon Backpack 650, Explorer's Backpack 1200, Light Vest 450, Heavy Vest 550, and an Ammo Crate Pouch of 260 that other pouches attach to. A mid-tier bag has "two item slots on each side for mounting magazines". Players asked for webbing-style external backpack pouches because digging in a cluttered main compartment "was a pain". — [ITR2 wiki: Equipment](https://into-the-radius-2.fandom.com/wiki/Equipment); [NamuWiki: ITR2](https://en.namu.wiki/w/Into%20the%20Radius%202); [Steam ITR2 suggestion](https://steamcommunity.com/app/2307350/discussions/1/4632611275488127478)
- **Contractors:** vest mags are refilled from ammo crates. Taking a mag off the vest leaves an empty slot, which a crate mag can refill. — [Steam Contractors threads](https://steamcommunity.com/app/963930/discussions/0/3816285100589883244/)
- **HL2VR:** fetch mags at the shoulder. Caught or picked-up mags return to the inventory pool. Quick Reload recovers unspent rounds. — [HL2VR manual](https://halflife2vr.com/manual/)
- **Titanfall 2 VR:** mags come from the backpack and grenades from the chest. — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)
- **Open-source reference (shock2quest, a Rust System Shock 2 VR port):** "Base chest and ammo target volumes" are body-inventory zones. The belt sits `vr_belt_drop` = 0.55 m below the eyes (range 0.30–0.90), with an adjustable forward offset. — [shock2quest dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs)

### Inferences
- **Recommended default:** a single "ammo zone" (pick one or let the user choose: over the shoulder, Alyx-style, or a belt or chest pouch) that spawns a magazine of the type the other hand's gun accepts, drawn from a counted pool per ammo type (`HashMap<AmmoType, u32>`). Each spawned magazine is filled with `min(capacity, pool)`. This one design covers Alyx, Bonelab, AS2 and Pavlov.
- **Partial mags:** pick a policy explicitly. Options are (a) Alyx's "no stowing partials" for tension; (b) HL2VR/Bonelab's "return remaining rounds to the pool" for convenience; (c) H3VR/ITR's "physical mags with exact round counts". (b) is the most forgiving default.
- **Infinite ammo** is a policy flag on the zone (H3VR's spawn-lock), not a separate system.
- **Empty hand over the zone:** Alyx's "empty hand gives pistol ammo" fallback avoids a dead grab. Return the mag for the last-held or primary weapon.
- **Physical-slot inventories** (ITR, Contractors) need a dump-pouch or "drop to discard" affordance. Players repeatedly complain about having to find room for empties (the Incursion: Red River complaint, and the ITR pouch-count complaints).

### Gaps
- No exact geometry for Alyx's backpack zone (offset behind the head, radius, whether it is gated by head yaw) was found in any official or community source. The open-source shock2quest numbers in section 7 are the only concrete reference, and they belong to a different game.
- Bonelab's exact mag-matching rules (for example, which ammo type returns with an empty other hand) and pouch positions are not documented beyond community wikis.
- Into the Radius 1's backpack and vest slot layouts are only partly described. Community reports are contradictory and version-dependent.

---

## 3. Inventory and holsters: body-relative slots, holster detection, auto-return, weapon wheels, wrist pockets, ITR vest/backpack, slots on estimated hips, and why Titanfall 2 VR swapping felt clunky

### Takeaway
There are two families. **Physical body slots** (H3VR quickbelt, Bonelab, Pavlov, AS2, ITR) are immersive but suffer when the body or hip estimate drifts from the real body. **Abstract selection** (Alyx's weapon wheel, HL2VR's hex grid, Bonelab's radial inventory) is reliable but less tactile. The most praised systems mix them: a small number of large, well-placed slots, an auto-return ("harness") option, and a menu fallback. Titanfall 2 VR's swapping was called "cumbersome and clunky" because it has one hip slot and no same-hand quick swap. The fix reviewers asked for is an Alyx-style stick-click plus direction selection.

### Cited Findings

**H3VR quickbelt [community wiki]**
- Slots are size-classed and a slot holds anything smaller than itself. Magazine slots (square) sit on the chest and waist; Small (pistols, knives) on the chest and waist; Medium (SMGs, shotguns, most rifles) on the hips; Large (MGs, snipers, launchers) on the shoulders; Back (backpack, shield) on the back, "reached behind your neck... or by bending over and reaching through your stomach." — [H3VR wiki: Quickbelt](https://h3vr.fandom.com/wiki/Quickbelt)
- There are many preset layouts, for example Tactical (6 small, 3 medium, 2 large), Packrat (12 small, 3 medium, 2 large), Desperado (4 small, 6 medium, 2 large), Concealed Carry (4 medium), Small Pockets (2 small), and TacticalCentered (6 magazine, 3 small, 3 medium, 2 large). — [H3VR wiki: Quickbelt](https://h3vr.fandom.com/wiki/Quickbelt)
- **Harnessing:** press the touchpad or B on a gun sitting in a slot and the slot turns green. "The weapon will return to that slot when you let go of it in your hand." — [H3VR wiki: Quickbelt](https://h3vr.fandom.com/wiki/Quickbelt)

**Boneworks/Bonelab [community]**
- There are 5 body slots, "one behind each shoulder, one under each arm, and one on the lower back." Only certain item types fit each slot. Inserting means dropping an item into a slot. A radial menu offers the same slots as a HUD. — [Boneworks/Bonelab wiki: Inventory](https://boneworks.fandom.com/wiki/Inventory)
- Complaints: the underarm pockets "seem useless now that most things can't fit in them." Players want more flexible slot rules to reduce competition for the other slots. Mods hide or resize holsters (Holster Hider: DEFAULT/INVISIBLE/DISABLED per holster; HolsterResizer). — [Steam Bonelab: Inventory](https://steamcommunity.com/app/1592190/discussions/0/3594464430443109431/); [Thunderstore: Holster Hider](https://thunderstore.io/c/bonelab/p/Lakatrazz/Holster_Hider/); [Thunderstore: HolsterResizer](https://thunderstore.io/c/bonelab/p/Toedtmanns/HolsterResizer/)

**Pavlov [community]**
- Holster by moving the hand to a circle on the chest and pressing grip (primary, pistol, knife). — [Pavlov VR Wiki: Game Controls (via search index)](http://wiki.pavlov-vr.com/index.php?title=Game_Controls)
- A suggested way to reload two guns: drop one or both mags, holster one gun at the hip, load the gun still in hand, and rack. — [Steam Pavlov threads (via search)](https://steamcommunity.com/app/555160/discussions/0/2592234299527476848/)

**Arizona Sunshine 2 [review/community]**
- Three weapons are carried: one holster on each hip and one over the shoulder. Only one heavy weapon (rifle, SMG, shotgun) plus two pistols or smaller. You can grab a gun off the dog Buddy to swap. One reviewer wanted a holsterable melee weapon to fight with while reloading. — [Analog Stick Gaming review](https://www.analogstickgaming.com/game-reviews/2023/12/11/arizona-sunshine-2); [PSNProfiles guide](https://psnprofiles.com/guide/19860-arizona-sunshine-2-trophy-guide)

**Metro Awakening [review/community]**
- Weapons sit at the waist and shoulders. Pressing grab next to the right waist "makes the pistol appear in your hand". Players disliked holstered guns being invisible and want to see their gear. — [Reviews via search](https://mixed-news.com/en/metro-awakening-review/); [Steam Metro Awakening thread](https://steamcommunity.com/app/2669410/discussions/0/4631482917606969203)

**Half-Life: Alyx [review/community]**
- There are no holsters. A gesture-based weapon wheel keeps a gun in the primary hand at all times. One reviewer called the wheel "a learning curve that bordered on unnecessary" but liked always having a gun in hand. — [CGMagazine review](https://www.cgmagonline.com/review/game/half-life-alyx-review)
- Selection uses a stick click plus a direction. — [Aftermath (comparing with TF2 VR)](https://aftermath.site/titanfall-2-vr-mod-circuitlord)
- Players asked for hip or underarm holsters and for a way to drop or switch hands. A Workshop "Body Holsters" mod adds them. — [Steam Alyx thread](https://steamcommunity.com/app/546560/discussions/0/1861616237339477679); [Steam Workshop: Body Holsters](https://steamcommunity.com/sharedfiles/filedetails/?id=3144612716)
- Accessibility: there is a Single Controller Mode ("adapt the game to be played by anyone who can only operate one controller"). Update 1.2 decoupled movement and rotation hand settings from weapon handedness. — [Road to VR: Alyx accessibility](https://roadtovr.com/half-life-alyx-accessibility-options-subtitles-seated-single-controller/); [Road to VR: Alyx update 1.2](https://roadtovr.com/half-life-alyx-update-1-2-left-hand-options/)

**HL2VR / HL2VRU [primary-doc]**
- Weapon selection: hold the primary stick to open a hexagonal grid; "The weapon selection menu is controlled by your hand's position; it is not a pointer." Release to equip. — [HL2VR manual](https://halflife2vr.com/manual/); the features page says it was adapted from Alyx — [HL2VR features](https://halflife2vr.com/features/)
- The HL2VRU fork's **Grip-Holster Mode** holsters the weapon when the grip is released and re-equips the last holstered weapon when the grip is held. Other hand interactions (props, flashlight) take priority. It is recommended for dual-wielding because it "simplifies the reloading process for two weapons a lot." — [HL2VRU README](https://github.com/vittorioromeo/HL2VRU)

**Slots on estimated hips [community]**
- Shoulder holsters behind the body are hard to see or hit. Hip holsters are easier but only take small weapons. Without full-body tracking the hips are estimated from head and hands, so the slots move back a few inches while the player moves, and turning the head can twist the hips enough to block sheathing. Belt slots with a configurable snap range are a community fix.

**Hard Bullet [community]**
- Players struggled to re-holster. The developer suggested adjusting the holster offsets in Player Settings while they reviewed holsters. — [Steam Hard Bullet thread](https://steamcommunity.com/app/1294760/discussions/0/6741412150958521826)

**Into the Radius / Contractors / Road to Vostok VR mod [community]**
- ITR uses physical vest pouches and a backpack (see section 2). Contractors guns can be reloaded while holstered. — [UploadVR Contractors guide](https://www.uploadvr.com/contractors-vr-beginners-guide-tips/)
- The Road to Vostok VR mod auto-holsters sidearm, knife and grenade on grip release, and the primary hangs on a sling. — [ModWorkShop: Road to Vostok VR mod](https://modworkshop.net/mod/56142) (via search summary)

**Titanfall 2 VR swapping [review]**
- "Weapon swapping itself is cumbersome and clunky." — [UploadVR review](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/)
- Two guns are carried, with the secondary on the left hip by default. The reviewer wished for "hip switching or same-hand swapping like Half-Life: Alyx" (stick click plus direction), found juggling weapons "fiddly", and kept a loaded EV-8 on the hip as a backup instead of reloading. — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)
- Neither the installer repo's release notes nor its manifests record a changelog for swap changes. — [CircuitLordVRModInstaller releases](https://github.com/CircuitLord/CircuitLordVRModInstaller/releases)

**Frameworks [primary-doc/primary-code]**
- HurricaneVR `HVRSocket` is "useful for... building Saints and Sinners style inventories". Filters use bitmask tags with 32 categories (AND/OR filter modes). "Scale Grabbable" auto-scales a socketed item to `Size / longest bounding-box axis`. Hover actions scale with hand proximity and turn green or red by validity (mimicking Saints & Sinners). The demo rig's **shoulder socket holds the backpack**. Holster orientation overrides align a per-item child transform to the socket axes. — [HurricaneVR docs: Sockets](https://github.com/cloudwalker2020/HurricaneVR-Docs/blob/main/docfx_project/manual/sockets.md)
- Auto Hand Place Point: radius, force-place and filters (see section 1). Auto Hand also ships a "Wrist Look Event" component (for wrist menus and pockets). — [Auto Hand docs index](https://earnest-robot.gitbook.io/auto-hand-docs/llms.txt)
- **shock2quest (open-source Rust port):** a "release-to-stow behind either shoulder" backpack with two zones. Body direction comes from "the head's slowly followed yaw, never its pitch/roll." Holster reach radius defaults to **0.15 m** (min 0.14, max 0.5). Body yaw follows the head with an exponential time constant of 0.5 s (`yaw += Δ·(1−e^(−dt/0.5))`) and freezes while a hand is inside a zone, so the zone doesn't slide away mid-reach. — [shoulder_backpack.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/mission/shoulder_backpack.rs); [body_inventory.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/mission/body_inventory.rs); [dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs)

### Inferences
- **Slot anchoring:** derive a "body yaw" from the head with lag (time constant about 0.5 s), ignore head pitch and roll, and **freeze body yaw while a hand is inside or approaching any slot**. That fixes the head turn twisting the estimated hips. Hips: place them at a fixed drop below the head (shock2quest's belt is 0.55 m below the eyes), scaled by calibrated user height, with a user offset setting (Hard Bullet's fix).
- **Few big slots beat many small ones.** Bonelab's small underarm slots and behind-the-back slots draw complaints. Hips and over-shoulder slots with radii of about 0.15–0.25 m are the reliable zones.
- **Show and confirm:** highlight on hover (HVR's green/red scale), a haptic tick on entry (with about 150 ms exit hysteresis so jitter doesn't re-buzz; see section 7), and visible holstered items (Metro's invisible-holster complaint).
- **Auto-return ("harness")** should be a per-slot or per-weapon opt-in, as in H3VR. HL2VRU's "grip release equals holster" is the simplest accessible variant. Return should animate or lerp back (estimate: about 0.2–0.5 s) rather than teleport, and should be disabled for weapons that are thrown.
- **Fixes for Titanfall-style clunkiness:** (1) add a same-hand quick swap (stick click plus direction, or Alyx/HL2VR's hand-position wheel); (2) let the secondary be drawn from either hip, or from the shoulder for long guns; (3) put the drop-versus-holster decision on a timer or double-click (H3VR Grip Button Drop Mode) so swaps don't drop guns; (4) offer auto-holster on release.
- **Always provide a menu fallback** (Bonelab's radial inventory, HL2VR's hex grid) for seated or one-handed play (Alyx's Single Controller Mode).

### Gaps
- I found no confirmed built-in "dropped gun returns to holster" option in Pavlov or Bonelab. H3VR harnessing, HL2VRU Grip-Holster and the Road to Vostok VR mod are the confirmed examples. Claims about Pavlov/Bonelab auto-return remain unverified.
- No official holster detection radii were found for any commercial game (Pavlov, Bonelab, AS2). Only open frameworks and ports publish numbers.
- Details of Alyx's wrist-pocket interaction (trigger radius, wrist-up gating) were not found.
- I found no changelog evidence that CircuitLord has changed swap mechanics since the reviews.

---

## 4. Aiming: sights and scopes, two-hand stabilization, virtual stock, weight and inertia, recoil models, lasers

### Takeaway
Two-hand aim is almost universally implemented as: the primary hand is the pivot, and the weapon's forward axis points from the primary grip toward the secondary hand, optionally blended with a "virtual stock" pivot near the neck or shoulder, with a One-Euro or similar low-pass filter on the support hand. Recoil is best modelled as an additive transform offset (pitch, yaw, rearward) that is clamped, lerped in and decayed out, scaled down for two-handed, foregrip and stocked holds. Physical scopes are rendered as a camera-to-texture on the lens, with fake parallax and scope shadow (eyebox).

### Cited Findings

**Two-hand and virtual stock math [primary-code: VRExpansionPlugin, MIT, Unreal]**
- Two-hand rotation: `rotVal = FindBetweenVectors(frontLocOrig, frontLoc)` is applied around the pivot (the primary hand), so the gun rotates by the change in direction from the primary grip to the secondary hand since the second grab. — [GS_GunTools.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/GripScripts/GS_GunTools.cpp)
- Virtual stock: when "mounted", an extra rotation `FindBetweenVectors(frontLocOrig, controllerPivot − MountWorldTransform.translation)` aligns the weapon to the line from the mount point (the HMD location plus `StockSnapOffset`, "the neck / mount pivot") to the hand. It is blended with the normal hold by `StockLerpValue`. — [GS_GunTools.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/GripScripts/GS_GunTools.cpp); [GS_GunTools.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_GunTools.h)
- Virtual stock defaults (Unreal units, cm): `bUseDistanceBasedStockSnapping = true`, **`StockSnapDistance = 35`** (the distance at which it snaps to or from the stock), **`StockSnapLerpThreshold = 20`** (the distance from the snap edge at which influence reaches 100%, lerping before that), `bAdjustZOfStockToPrimaryHand = true`, `bSmoothStockHand = false`, `SmoothingValueForStock = 0` (0 = none, 1 = full), and stock-hand One-Euro filter `MinCutoff = 5`, `DeltaCutoff = 20` ("speed up the lerp on fast movements"). — [GS_GunTools.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_GunTools.h)
- Secondary-grip smoothing: an `FBPEuroLowPassFilter` on the front hand. Global One-Euro defaults are **`MinCutoff 0.1`, `CutoffSlope 10`, `DeltaCutoff 10`**. Optional distance influence: `GripInfluenceDeadZone = 50` (100% influence within 50 cm of the grip point) and `GripInfluenceDistanceToZero = 100`. — [GS_GunTools.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_GunTools.h); [VRGlobalSettings.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/VRGlobalSettings.cpp)
- Unity XRI `XRGrabInteractable` defaults: attach ease-in 0.15 s, position and rotation smoothing amount 8 with tighten 0.1, velocity scale and damping 1, max linear velocity delta 10, max angular velocity delta 20. — [XRGrabInteractable.cs (XRI 3.7.0-pre.1 mirror)](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactables/XRGrabInteractable.cs)

**Virtual stock in games [community/primary-doc]**
- H3VR: "Use Virtual Stock: if the firearm has a stock (or a Pistol Stock attached), pressing the stock against your 'body' will help stabilize the gun." It is not recommended with a physical gunrig, and there is a separate "Gunrig Mode". — [H3VR wiki: Options Panel](https://h3vr.fandom.com/wiki/Options_Panel)
- Pavlov: virtual stock is a settings toggle. Players asked for per-weapon toggles, for example "VStock for sniper rifles ONLY". Physical stock accessories make reloading awkward (you pop the controller out of the stock), and players say a grip-drop button is faster. — [Pavlov feature-upvote: VStock for snipers only](https://pavlovvr.featureupvote.com/suggestions/232993/virtual-stock-vstock-for-snipers-only); [Pavlov feature-upvote: Virtual Stock Toggle](https://pavlovvr.featureupvote.com/suggestions/269621/virtual-stock-toggle); [Steam Pavlov thread](https://steamcommunity.com/app/555160/discussions/0/1744480967032786266)
- HL2VRU: the virtual stock "interpolates two-handed aiming with the player's approximate shoulder position", with a tunable shoulder position. — [HL2VRU README](https://github.com/vittorioromeo/HL2VRU)

**Weight and inertia**
- HL2VRU "Weapon Weight Simulation": heavier weapons resist movement and rotation, holding with both hands reduces the effect, and the pistol and revolver are nearly unchanged. — [HL2VRU README](https://github.com/vittorioromeo/HL2VRU)
- Auto Hand: held "feel" comes from the Rigidbody mass and drag of the hand and the item, joined by a physics joint. `Ignore weight` simulates weightlessness, and `Joint Break Force` sets how hard a pull breaks the grip. — [Auto Hand docs: Grabbable](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/grabbable)

**Recoil models**
- VRE recoil defaults: `MaxRecoilTranslation`/`MaxRecoilRotation` = 0 (per-gun), `DecayRate = 20`, `LerpRate = 30`. Each tick: `Storage = Blend(Storage, Target, clamp(LerpRate·dt))`, `Target = Blend(Target, Identity, clamp(DecayRate·dt))`. `AddRecoilInstance` adds to Target and clamps it per axis to the max values. It can alternatively apply recoil as a physical force. — [GS_GunTools.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/GripScripts/GS_GunTools.cpp)
- H3VR's `FVRFireArmRecoilProfile` fields as stored in mod assets. ".45 standard pistol" profile: `VerticalRotPerShot 35`, `HorizontalRotPerShot 3`, `MaxVerticalRot 60`, `MaxHorizontalRot 30`, `VerticalRotRecovery 20`, `HorizontalRotRecovery 15`, `ZLinearPerShot 0.005`, `ZLinearMax 0.035`, `ZLinearRecovery 8`, `XYLinearPerShot 0.1`, `XYLinearMax 0.005`, `XYLinearRecovery 8`. Bipodded: `VerticalRotPerShot 3`, `MaxVerticalRot 10`. `RecoveryStabilizationFactors_None {0.25,0.5,0.25,0.25}`, `_Twohand {0.5,0.8,0.3,0.75}`, `_Foregrip {1,1,0.8,0.8}`. Mass drift: intensity 0.1, max magnitude 5, max rotation 20, recovery 5. Muzzle-device factors: suppressor 0.95, muzzle brake 0.8, brake multipliers vertical 0.8, horizontal 0.9, rearward 0.9. A closed-bolt rifle profile uses `VerticalRotPerShot 15`, `MaxVerticalRot 40`. Guns carry separate unstocked and stocked profiles. — [LSIIC (H3VR mod) RecoilProfile assets](https://github.com/BlockBuilder57/LSIIC/tree/HEAD/Unity/Assets/LSIIC/ScriptableObjects/RecoilProfile); [cityrobo/H3VR_Mods (RecoilProfile and RecoilProfileStocked fields)](https://github.com/cityrobo/H3VR_Mods); [H3VR modding wiki: the basics](https://github.com/H3VR-Modding/wiki/blob/main/src/creating/implementing/the_basics.md)
- HL2VR: two-handing slightly reduces pistol recoil and "significantly" reduces SMG and pulse-rifle recoil, which is the intended way to fire them. For the revolver, gravity gun and RPG, the two-hand grip is purely cosmetic. — [HL2VR manual](https://halflife2vr.com/manual/)
- HL2VRU exposes a recoil multiplier and recommends lowering it when dual-wielding automatics. — [HL2VRU README](https://github.com/vittorioromeo/HL2VRU)
- Alyx's shotgun recoil animation is praised for balancing "the action of firing and the weight of this big metal weapon", with a slide kick. — [Road to VR shotguns](https://www.roadtovr.com/inside-xr-design-best-shotguns-in-vr-what-makes-them-great-feedback/2/)
- shock2quest (open-source port) recoil gains: `gun_kickback_scale 0.3`, `gun_pitch_scale 1.0`, `gun_yaw_scale 1.0`, `gun_one_hand_scale 1.0` ("additional one-handed recoil only; support already removes this spring"). — [shock2quest dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs)

**Sights, scopes, lasers**
- Render-texture scope (NeoFPS, flatscreen): a scope camera renders zoomed into a render texture. A behaviour manages focal point, crosshair and parallax, and "angle compensation" rotates the scope camera slightly as the scope turns away from the eye. — [NeoFPS docs: render texture scope](https://docs.neofps.com/tutorials/firearm-scopes-rtscope.html)
- A VR-tagged Godot PiP scope shader uses two viewport textures (the 3D scope camera and a 2D reticle) with an eyebox, scope shadow and fake parallax. — [Godot Shaders: 3D PiP scope](https://godotshaders.com/shader/3d-picture-in-picture-scope-with-eyebox-reticle-scope-shadow-and-fake-parallax/)
- In Titanfall 2 VR, holographic sights and scopes have convincing depth. Real optics cast a scope shadow, which "makes aiming harder but rewards a realistic stance." — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)
- Aiming down sights in TF2 VR "takes a lot of getting used to". Two-handing is hard because there is no solid physical link between the hands. — [UploadVR review](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/); [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)
- HL2VR offers a laser pointer in Difficulty options and says it "doesn't count as a cheat". — [HL2VR manual](https://halflife2vr.com/manual/)

### Inferences
- **Two-hand aim algorithm (recommended):** `forward = normalize(secondary_pos_filtered − primary_grip_pos)`. Weapon roll comes from the primary hand's roll (or the average of both hands' up vectors). Filter the secondary-hand position with a One-Euro filter (VRE's global defaults are minCutoff 0.1, β (cutoff slope) 10, dCutoff 10; tune minCutoff up for less lag). Ease the two-hand pose in over about 0.15 s on the second grab (XRI's attach ease-in) so the gun doesn't snap.
- **Distance gating:** keep two-hand influence at 100% near the foregrip and fade it out as the hand leaves. VRE's 50/100 cm defaults are generous; an estimate of 10–15 cm deadzone and 25–30 cm to zero suits handguard-length grips.
- **Virtual stock:** pivot = HMD position + a neck or shoulder offset (lateral toward the dominant side, a few cm down). It activates when the gun's stock end is within the snap distance (VRE 35 cm, blending over the last 20 cm). Then aim along `pivot → front hand` (or `pivot → primary hand` one-handed). Make it a per-weapon-class toggle, since players want it for snipers only.
- **Recoil (recommended structure):** per shot, add impulses to a target offset `{pitch, yaw, rearward z, xy jitter}`, clamped to maxima. Each frame, lerp the applied offset toward the target (VRE: 30/s) and decay the target toward zero (VRE: 20/s; H3VR's recovery rates of about 15–20 for rotation and 8 for linear sit in the same range). Multiply by hold-state factors: none (one-handed), two-hand, foregrip, stocked or bipod. H3VR's tables give a usable starting ratio: bipod cuts vertical rotation per shot by about 10×. Apply the offset to the rendered and aimed weapon pose, not to the tracked hand, so the hand stays where the user put it.
- **Scopes:** use render-to-texture on the lens (one extra camera at reduced resolution and frame rate, e.g. half rate, an estimate), plus an eyebox or scope-shadow vignette computed from eye-to-lens offset for parallax. Mono rendering of the scope view is a common optimization (an estimate: one camera, not stereo). Provide a fallback "zoomed reticle without RT" for low-end standalone.

### Gaps
- No primary data on Pavlov's or Alyx's virtual-stock offsets or smoothing values.
- The exact semantics of H3VR's `RecoveryStabilizationFactors` vector components are not documented (they look like per-axis multipliers applied by hold state). The values come from mod assets and may not match every shipped weapon.
- No VR-specific scope performance measurements (cost of a second camera per eye on Quest-class hardware) were found.
- No published "weapon inertia" constants (mass-to-lag curves) were found beyond qualitative descriptions (HL2VRU, Auto Hand).

---

## 5. Melee: velocity-based damage, physical blocking, penetration and lodging, Boneworks

### Takeaway
Shipped VR melee is velocity-gated contact damage: damage only registers above a minimum closing speed, scaled by the speed of a defined "impact point" and by the swing direction for stabs, with per-surface multipliers and cooldowns to prevent jitter spam. Physics-melee games add penetration and lodging with a dampening model and keep damage curves in data files, some scaling damage by momentum and by type (pierce, slash, blunt).

### Cited Findings
- HL2VR crowbar: you must physically swing, damage depends partly on swing distance and speed, there is a minimum speed and distance plus a short cooldown between swings, and "Swinging props or weapons other than the crowbar does not do any damage." — [HL2VR manual](https://halflife2vr.com/manual/)
- HL2VRU "Universal Melee": physical melee with any weapon or empty hands, with tunable damage. — [HL2VRU README](https://github.com/vittorioromeo/HL2VRU)
- VRE `GS_Melee` lodge and penetration defaults: `PenetrationVelocity = 8000` (impulse along the component's forward axis needed to fire OnPenetrated), `MinimumHitVelocity = 1000` (impulse needed for OnHit when no stab happened), `AcceptableForwardProductRange = 0.1` (the dot product of forward and impact normal must be within 0.1 of 1.0, i.e. roughly a 25° cone), `PenetrationDepth = 100`, per-surface `BluntDamageScaler`, `SharpDamageScaler`, `StabVelocityScaler` (default 1) and `bSurfaceAllowsPenetration`, and an option `bOnlyPenetrateWithTwoHands`. Values are Unreal impulse-based units, not m/s. — [GS_Melee.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_Melee.h)
- Auto Hand "Smasher": the trigger fires when `velocity × Force Multi` reaches the target's `Smash Force`. Velocity is measured at a configurable `Center of Mass Point` (e.g. the hammer head). — [Auto Hand docs: Smashing](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/extras/smashing)
- Auto Hand `Pull Apart Break Only` lets an OnBreak event fire only when two hands hold the item, to simulate ripping things apart. — [Auto Hand docs: Grabbable](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/grabbable)
- shock2quest (open-source port): a held melee weapon damages when its closing speed exceeds `melee_free_swing = 2.0` world units/s ("a swing damages because it was moving, not because a button was down"). One-handed melee damage is scaled 0.5. Simulated melee speed and turn are capped (`melee_max_speed 60`, `melee_max_turn 60` rad/s). — [shock2quest dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs)
- In Titanfall 2 VR, melee is a physical punch to the head. — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord)
- Alyx lets guns physically push and swat objects, though without a melee damage system. — [Road to VR](https://www.roadtovr.com/these-details-make-half-life-alyx-unlike-any-other-vr-game-inside-xr-design/2)

### Inferences
- **Recommended:** compute velocity at a per-weapon "impact point" (the tip or head) from a short history window (about 3–5 frames, an estimate). Damage = `curve(relative_speed)` × surface multiplier × (two-hand bonus or one-hand penalty), applied only above a minimum speed (an estimate for real-metre scales: about 1.5–2.5 m/s) and with a per-target cooldown (about 0.2–0.5 s, an estimate) to stop resting contact from jittering damage. For stabbing, require the forward-to-normal alignment (VRE: within 0.1 of a perfect dot product) plus a higher speed threshold.
- **Blocking:** treat a held weapon as a kinematic or physically driven collider. When an enemy weapon's contact speed is below the player weapon's "parry" threshold and their contact normals oppose, cancel or deflect the attack. This follows from how VRE uses contact impulses; no exact numbers were found.

### Gaps
- No primary Boneworks/Stress Level Zero technical description of melee damage was found. The UploadVR interview with Brandon Laatsch was cut off in search excerpts.

---

## 6. Grenades: pin pull and throw

### Takeaway
The documented patterns are "press to arm, throw, release" (HL2VR) and physical pin pulling (various sims, not documented in sources found), with an optional aim arc for accessibility. Throw velocity should be smoothed over a short window and scaled up modestly, because raw controller release velocity under-throws. That matches the TF2 VR complaint about lobbing grenades "six feet shorter than you intended."

### Cited Findings
- HL2VR: hold the trigger to arm, make a throwing motion, release to throw. Holding alt-fire shows a visual arc, and releasing throws along it, which helps with small spaces and windows. — [HL2VR manual](https://halflife2vr.com/manual/)
- Titanfall 2 VR: grenades sit on the chest and are pulled off and thrown. The reviewer reported "accidentally lobbing a grenade six feet shorter than you intended." — [Aftermath](https://aftermath.site/titanfall-2-vr-mod-circuitlord); [UploadVR review](https://www.uploadvr.com/titanfall-2-vr-mod-review-a-flatscreen-classic-trades-power-for-presence/)
- Alyx grenades can't go in the backpack, only in the wrist pockets (two at once), plus one in each hand. — [VR One Alyx tips](https://www.vrone.co.uk/articles/half-life-alyx-tips); [gameplay.tips](https://gameplay.tips/guides/7161-half-life-alyx.html)
- Unity XRI throw defaults: `ThrowOnDetach = true`, **throw smoothing duration 0.25 s**, **throw linear velocity scale 1.5**, angular scale 1.0. — [XRGrabInteractable.cs](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactables/XRGrabInteractable.cs)
- shock2quest throw defaults: speed scale 1.0, spin scale 1.0, max speed 12 world units/s, max spin 25 rad/s, **motion smoothing 50 ms**, minimum throw speed 1.5 units/s, weight slowdown exponent 0.25. — [shock2quest dev_params.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs)
- Auto Hand: `Ignore Release Time` sets how long the hand ignores the item's colliders after release, "for smoother releases with thrown objects". — [Auto Hand docs: Grabbable](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/grabbable)

### Inferences
- **Grenade state machine:** `Stowed → Held → Armed (pin pulled by the other hand grabbing a pin child with a small pull threshold, or a trigger press) → Released (fuse starts on lever release or throw) → Detonate`. Offer a one-hand "trigger to arm" mode for accessibility. Add an optional predicted arc while a modifier is held (HL2VR).
- **Throw velocity:** average hand velocity over the last 50–250 ms (shock2quest 50 ms, XRI 250 ms; the window should end at release). Scale by about 1.2–1.5 (XRI default 1.5), and ignore colliders on the hand for a brief release window.

### Gaps
- I found no reliable source documenting pin-pull distances or fuse timings for Pavlov, Into the Radius, Contractors or Alyx grenades.

---

## 7. Concrete numbers (consolidated)

### Takeaway
Commercial games almost never publish their interaction constants. The usable concrete values come from open frameworks (Unity XRI, VRExpansionPlugin, Auto Hand, HurricaneVR) and open-source ports (shock2quest in Rust, plus H3VR mod assets). Treat them as tested starting points, not as the values Alyx or Pavlov ship.

### Cited Findings

| Parameter | Value | Source (type) |
|---|---|---|
| Socket/holster snap radius | 0.10 m | [Unity XRI XRSocketInteractor](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactors/XRSocketInteractor.cs) (primary-code) |
| Socket re-accept delay after removal | 1.0 s | same |
| Holster reach radius | 0.15 m (range 0.14–0.5) | [shock2quest dev_params](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs) (open-source port) |
| Over-shoulder backpack zone radius | 0.25 m (range 0.18–0.5) + hand radius | same |
| Backpack zone centres (relative to head, using lagged body yaw) | centre = head − forward·0.06 m − up·0.10 m, then ± right·0.24 m (two zones) | [shock2quest shoulder_backpack.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/mission/shoulder_backpack.rs) |
| Backpack gating (excludes face and forward gestures) | hand offset·forward < 0.08 m AND abs(offset·right) > 0.10 m | same |
| Zone exit hysteresis before feedback resets | 0.15 s outside | same |
| Grip squeeze threshold | 0.5 (analog) | same |
| Body yaw follow time constant | 0.5 s exponential, frozen while a hand is in a zone | [shock2quest body_inventory.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/mission/body_inventory.rs) |
| Shoulder model | 0.25 m below head, 0.19 m half-width; arm reach 0.7 m | [shock2quest hand_target.rs](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/input/hand_target.rs) |
| Belt height | 0.55 m below eyes (range 0.30–0.90) | [shock2quest dev_params](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs) |
| Virtual stock snap distance / full-influence lerp band | 35 cm / 20 cm | [VRE GS_GunTools.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_GunTools.h) (primary-code) |
| Stock-hand One-Euro filter | MinCutoff 5, DeltaCutoff 20 | same |
| Global One-Euro (secondary grip) | MinCutoff 0.1, CutoffSlope 10, DeltaCutoff 10 | [VRE VRGlobalSettings.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/VRGlobalSettings.cpp) |
| Secondary grip influence | 100% within 50 cm, 0% at 100 cm (optional) | [VRE GS_GunTools.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_GunTools.h) |
| Recoil lerp-in / decay rates | 30 /s / 20 /s (blend factor = rate·dt) | [VRE GS_GunTools.cpp](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Private/GripScripts/GS_GunTools.cpp) |
| H3VR .45 pistol recoil | vertical 35/shot (max 60), horizontal 3/shot (max 30), rotation recovery 20 (V) and 15 (H), z-linear 0.005/shot (max 0.035, recovery 8) | [LSIIC H3VR mod assets](https://github.com/BlockBuilder57/LSIIC/tree/HEAD/Unity/Assets/LSIIC/ScriptableObjects/RecoilProfile) (mod data) |
| H3VR hold-state stabilization factors | none {0.25,0.5,0.25,0.25}, two-hand {0.5,0.8,0.3,0.75}, foregrip {1,1,0.8,0.8} | same |
| H3VR bipod recoil | vertical 3/shot, max 10 | same |
| Grab attach ease-in | 0.15 s | [Unity XRI XRGrabInteractable](https://github.com/needle-mirror/com.unity.xr.interaction.toolkit/blob/master/Runtime/Interaction/Interactables/XRGrabInteractable.cs) |
| Throw smoothing window / velocity scale | 0.25 s / ×1.5 | same |
| Throw smoothing (alternative) | 50 ms; min throw 1.5 u/s; max 12 u/s | [shock2quest dev_params](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs) |
| Slider dead zone / event threshold examples | Play Range 0.1 (10% travel), Threshold 0.1 (events at ±0.9) | [Auto Hand Slider](https://earnest-robot.gitbook.io/auto-hand-docs/auto-hand/extras/physics-gadgets/physics-gadget-slider) |
| Melee stab facing tolerance | dot product ≥ 0.9 (range 0.1) | [VRE GS_Melee.h](https://github.com/mordentral/VRExpansionPlugin/blob/master/VRExpansionPlugin/Source/VRExpansionPlugin/Public/GripScripts/GS_Melee.h) |
| Melee impulse thresholds | penetrate 8000, hit 1000 (Unreal impulse units) | same |
| Melee min swing speed | 2.0 world u/s; one-hand damage ×0.5 | [shock2quest dev_params](https://github.com/tommy-xr/shock2quest/blob/HEAD/shock2vr/src/dev_params.rs) |
| Alyx shotgun reload | 2 shells per grab (7 rounds in 4 gestures) | [Road to VR](https://www.roadtovr.com/inside-xr-design-best-shotguns-in-vr-what-makes-them-great-feedback/2/) (review) |
| H3VR round stacking per slot | 6 shotgun shells, 5–10 rifle, 10 pistol, 4 × 40mm | [H3VR wiki: Spawn-Locking](https://h3vr.fandom.com/wiki/Spawn-Locking) |
| H3VR grip-drop guard | instant / 1 s delay / double-click | [H3VR wiki: Options Panel](https://h3vr.fandom.com/wiki/Options_Panel) |
| HurricaneVR socket auto-scale | localScale = Size / longest bounding-box axis | [HVR docs: Sockets](https://github.com/cloudwalker2020/HurricaneVR-Docs/blob/main/docfx_project/manual/sockets.md) |

### Inferences
These are estimates synthesized from the table and the complaints above. They are not sourced values.
- **Holster detection:** hip and chest slot radius of 0.12–0.18 m, shoulder and back zone radius of 0.20–0.25 m (shoulder zones can be bigger because they are out of view). Use the hand-to-slot sphere test plus a facing gate so forward reaches don't trigger the back zones.
- **Mag snap:** strict 3–5 cm and ±25°, easy 8–10 cm and ±45°. Seat over 50–100 ms with a click sound and haptic.
- **Slide and rack:** register a rack at ≥ 90% of travel, return with a spring, and allow "slide release" by button (Pavlov) or by slamming a mag into a locked-open gun as an option.
- **Two-hand:** filter the front hand with One-Euro (minCutoff 0.5–1.0 Hz, β 5–10 at metre scale), ease in over 0.15 s, and drop out when the support hand leaves about 25 cm from the foregrip.
- **Recoil:** pistols about 5–10° pitch per shot visual and aim kick with about 1 cm rearward; rifles about 2–4° when two-handed. Recovery rates 15–20 /s. Two-hand multiplier about 0.5, stock or bipod about 0.1–0.3.
- **Body anchoring:** body yaw lagged by about 0.5 s and frozen during reach. Belt at about 0.55 m below the eyes, or user-height × 0.33 below the head. Expose per-slot offsets in settings.

### Gaps
- Not found anywhere: Alyx's backpack zone offset and radius, Alyx's or Pavlov's magwell snap radius and angle, Bonelab's holster radii, Pavlov's virtual-stock offsets, real slide-pull distances in any commercial game. All numbers in the table come from open frameworks, open-source ports or mod assets.
- The shock2quest values are world-unit based in places ("u/s"). Conversion to metres depends on its `METERS_PER_WORLD_UNIT` constant, which was not checked for the throw and melee speeds. The radii and zone offsets are explicitly in metres.
