# Rigging a gun for RustVR

A **rig** says how the player's hands hold a game's gun: where the line of sight runs, where the
firing hand and the support hand go, which parts of the model are the magazine, the bolt and the
trigger, where the magazine seats and the muzzle points, and a few facts (rate of fire, weight,
magazine size, caliber). With a rig, the runtime's player controller (DESIGN.md §6b) can pick the
gun up, aim it down its own sights, reload it with its own magazine and rack its own handle.

Rigs are **data**, measured on the game's own models and committed in `content/rigs/<game>.toml`
(the measurements are ours; the models stay in the user's own `local_assets/`, never committed).
`rustvr_asset::rigs` parses and checks them and compiles them in, so the test range and the game's
adapter register exactly the same rigs. Set `RUSTVR_RIGS` to a folder to load `<game>.toml` from
there instead, to try an edit without rebuilding.

The rule is the project's: **the game is the source of truth**. The hands are the game's own
(its first-person arms posed for the gun), the recoil is the game's (its kick table), the rate of
fire, fire mode, magazine size and caliber come from its data where it has them. What we measure
is geometry the game doesn't state (the sights, the magazine well), and every measurement is
checked by looking at it, held and down the sights.

Metro 2033 Redux is the worked example throughout: `content/rigs/metro2033.toml` rigs its 22
player-held guns, each with a comment saying what was measured and how.

## 0. Spaces

* **Model space** is the gun model's own, as the extractor caches it (`crates/asset`): left-handed
  metres, X right, Y up, Z forward, in the game's first-person placement (Metro's viewmodel space:
  the camera at the origin looking down +Z). Every point in a rig is in model space, and the tools
  print model space.
* **Weapon space** is what the runtime sees (`ObjectDesc`): the line of sight along +Z, the origin
  at the rig's `grip`. The range builds it as `fix = R * T(-grip)`, with `R` turning the line from
  `rear_sight` to `front_sight` onto +Z. So the line of sight *is* the gun's forward: shots leave
  along it, and aiming down the sights puts the eye on it.

## 1. Get the game's guns

Extract the game's first-person guns, the arms posed for each and their recoil into
`local_assets/<game>/` (for Metro, `metro-extract`). The index (`index.toml`) names each gun's
arms (`hands = "hands_ak"`) and, where the game says, its magazine size. The tools find the folder
through `RUSTVR_ASSETS` (or `local_assets/` at the repository root).

## 2. Look at it as the game holds it

```sh
RUSTVR_ASSETS=X:/rustvr/local_assets cargo test -p rustvr-range --test rig metro_guns_as_the_game_holds_them
```

writes `captures/rig/survey/<gun>_<view>.png` for every Metro gun, rigged or not: the model with the
game's arms for it, from the `right`, `left`, `top`, `under` and `back`, and the top of the gun along
its line of sight close up in four tiles (`sights_0` .. `sights_3`, rear to front), to see its sights
side-on. `RIG_ONLY=wpn_ak,wpn_rpk` picks guns. Decide here what kind of gun it is:

* `hold = "rifle"` (the default): a pistol grip and a handguard, shouldered;
* `hold = "pistol"`: one hand; it holsters on the hip (the runtime has no cupping support hand, so
  a game that cups a pistol with its other hand only gives us the firing hand);
* `hold = "heavy"`: two hands on handles that aren't a pistol grip and a handguard (a spade grip, a
  side handle, a handlebar with a lever trigger), fired from the hip: no stock to shoulder.

## 3. Survey it

```sh
cargo run --release -p rustvr-asset --example rig_probe -- metro2033 wpn_rpk survey
```

does the common measurements at once, on cross-sections of the mesh (where its triangles cross a
plane, so it doesn't matter where the model happens to have vertices):

* **parts**: every bone carrying skin, with numbered siblings folded (`patron_* x45`), its parent
  and bounds, tagged when its name looks like a trigger (`kurok`), a magazine or rounds (`magazin`,
  `patron`, `baraban`...) or a bolt or handle (`zatvor`, `nasos`, `ruchka`...);
* **locators**: bones without skin (`loc_fire_point`, `loc_magazin_m`...), with positions;
* **the game's aim line** (the line its aimed camera looks along, from the index) and the game's
  palms (`bone_hand_right` / `bone_hand_left` of the gun's arms);
* **the sight profile** along the aim line: the top of the gun on the line and of its shoulders
  either side, where either changes; the notches (shoulders above the centre) and posts (a peak on
  the line) it finds, and the pairs ranked by how near their line passes the game's aimed camera
  (the viewmodel camera at z = 0): the game shows its sight picture from there, so the true pair
  lines up with it (the Kalash's notch and post: 0.7 mm);
* **the pistol grip**: its section every centimetre down from the receiver under the game's right
  palm (the outline nearest the palm), the axis fitted through the middle of the hand-hold and its
  rake, and the grip as a block;
* **the trigger**: its bone's bounds, and the pad point 40% up the blade, 2 mm behind its face;
* **the handguard** at the game's left palm: its section and where the palm is under it;
* **a rig entry to start from**.

The other modes measure by hand: `parts` (per bone and per mesh, with each mesh's index and name),
`slab <axis> <lo> <hi> ...` (bounds inside a box, per mesh and bone), `rows <axis> <lo> <hi> <step>
...` (vertex bounds per slice), `cuts <axis> <lo> <hi> <step> ...` (cross-section bounds per plane:
the precise one, e.g. `cuts z 1.60 1.66 0.005 x 0.139 0.143 y -0.07 0` finds a post inside a hood)
and `hand` (the arms' canonical hand frames).

## 4. Fill in the rig

Every field of a `[[gun]]` (`rustvr_asset::rigs::GunRig`); points are model space, metres:

| Field | What | How it's measured |
|---|---|---|
| `key` | The gun's key in the asset index | |
| `name` | Shown to the player (optional; the index's) | |
| `hold` | `rifle`, `pistol` or `heavy` (§2) | The survey renders |
| `caliber` | What it fires, as the game calls it (`5.45x39`, `12 gauge`...) | Metro: the bullet section and cartridge case its config names (`config_strings`, §5) |
| `magazine` | The magazine it takes; guns naming the same one take each other's (optional: its own) | The game's magazine overlays (`ak_magazin_30` serves the Kalash and the AKSU), and the meshes being the same |
| `rear_sight`, `front_sight` | Two points on the line of sight: the top of the rear notch's shoulders and the front post's tip | §4.1 |
| `grip` | The firing hand's palm centre on the pistol grip's axis: the weapon's origin, and the hold without the game's arms | The survey's `grip` (the axis level with the game's palm) |
| `grip_rake_deg`, `grip_twist_deg` | The grip axis's rake back from vertical; an extra turn of the hand about it (default -15) | The survey's fit |
| `grip_block` | The grip as a block: `centre`, `half` (across, front to back square to the axis, along it) | The survey; drawn in the `grip_section` renders |
| `trigger` | Where the trigger finger's pad rests on the trigger | The survey's pad point (or the blade's face by `cuts`) |
| `trigger_bone` | The trigger's bone: its skin swings with the finger, about the bone (`""`: no trigger part) | The parts list (`kurok`) |
| `foregrip` | Two-handed guns: the support hand's palm centre (the game's left palm) | The survey's `foregrip` |
| `support_tube`, `support_palm` | The support fist's axis and palm normal, used only without the game's arms (optional: the Kalash's cradle) | `rig_probe hand` |
| `mag_well` | Where the magazine's top sits when it's in (on the magazine's bounds) | The magazine locator (`loc_magazin_m`), or the magazine's top or the rims of its rounds |
| `muzzle` | The bore's end | `loc_fire_point` when it's at the barrel's end (some sit past the model: then the barrel's end on the bore) |
| `handle`, `handle_travel` | The part pulled back to chamber (charging handle, bolt knob, slide, pump, cocking lever) and how far it travels (m); none: nothing to rack | The handle's knob from the parts; travel from the real gun or the game's motion (`export_viewmodel --peaks`) |
| `mag_bones` | Bones (and everything under them) whose skin is the magazine; a trailing `*` matches any ending | The parts list |
| `mag_meshes` | Whole meshes (by index) that are the magazine too: one the extractor mounted on a bone that isn't the magazine's | `rig_probe parts` lists meshes |
| `bolt_bones` | Bones whose skin moves back with the handle | The parts list |
| `hidden_bones` | Bones whose skin isn't drawn: what the game shows only in an animation or with an upgrade (a shell in the hand, an ejected case, an upgrade's barrels' shells) | The survey renders (things floating, or doubled) |
| `recoil`, `recoil_feel` | The runtime's own recoil, used only when the game's kicks aren't loaded: climb (deg), scatter (deg), push (m); climb kept on auto, return (1/s), lean (deg) | By class: rifles about `[7, 1.8, 0.025]`, shotguns `[10, 2.5, 0.04]`, pistols `[12, 3, 0.03]` |
| `rounds_per_second` | Rate of fire | The game's config (§5) |
| `automatic` | Full auto (default true) | The game's recoil: guns with long-burst tracks are automatic (§5) |
| `slide_locks` | The slide locks back on the last round | The real gun |
| `mag_size` | Rounds per magazine (optional: the index's) | The index, from the game's overlays; else the model's rounds |
| `mass`, `mag_mass` | Kilograms: the gun without its magazine, and a loaded magazine | Real-world references for what the gun copies (documented per gun) |
| `left_side_up` | Lies on a table with its left side up (a magazine sticking out of the left) | The survey renders |
| `ads_zoom` | Zoom while aiming (above 1.4 opens a magnified scope; default 1.1) | |

Write down, in a comment above the entry, what you measured and how: the next person re-checks
it from there.

### 4.1 The line of sight

The line of sight must run through the gun's own sights as a player lines them up, because in VR
the eye really is on it: shots go where the front post's tip is.

1. **A notch and a post**: take the survey's pair that passes nearest the game's camera (within
   6 mm it says so). The rear point is the notch's *shoulders' top*, the front point the post's
   tip. Several notches on a long leaf or a slider all line up within a millimetre or two; the
   sights render decides (step 6).
2. **A ring, a peep or a reticle** (the Preved's and the Hellbreath's ring with its reticle, the
   Vyhlop's aperture): the ring's centre and the post's tip (or the reticle's centre), by `cuts`.
3. **No pair lines up with the camera** (the Bastard's camera is 3 cm over its sights, the RPK's
   leaf sits higher than its camera line): measure the notch and the post by `cuts`, and check
   the render.
4. **No sights at all** (the Duplet's barrel sleeve): the game's aim line, which is how the game
   aims it.
5. **A hip-fired gun whose aim line runs through its own body** (the Abzats, the Gatling, the
   flamethrower): the bore (`loc_fire_point`'s axis), which is where its shots come from.

### 4.2 The hands

The runtime draws the game's own hands: each gun's arms in their bind pose (`hands_<gun>`, the
idle the extractor baked) are the hold, and the controllers go where the drawn hands land exactly on
them (`GunHand`, `controller_on` in `crates/range/src/content.rs`). So `grip`, `grip_rake_deg`,
`foregrip` and the rest are the fallback for a gun without arms, and `grip` is the weapon's origin.
Nothing is fitted or adjusted: if the game's index finger rests along the frame instead of on the
trigger, so does ours.

### 4.3 Moving parts, magazines and handles

The magazine is the part that comes out: a box, a drum, a tube of balls or darts, or, for guns
loaded round by round, the rounds themselves taken as one (a revolver's six rounds with their
cases, a revolving shotgun's six shells, a double barrel's two shells: a speed loader). Seated, it
sits exactly where the model has it (magazine space is the gun's own), and the runtime seats a
held magazine when its well point comes within 9 cm and 45° of the gun's. Guns naming the same
`magazine` share one magazine template (and ammo type: `caliber << 8 | magazine`, both numbered in
order of first appearance), so give it only to guns whose magazines are the same mesh at the same
place. A gun without `mag_bones` keeps its rounds (the flamethrower's fuel) and has no reload.

`handle` is whatever is pulled back to chamber: the runtime moves `bolt_bones` straight back by the
pull. Map a **pump** to its forend (the Helsing's pump `nasos`: its back end, 13 cm behind the
support hand, so reaching for the handguard doesn't grab it instead) and a **bolt** to its knob.
Leave `handle` out for guns that chamber by themselves (a double-action revolver, a revolving
shotgun) and for break actions: a fresh magazine then chambers as it goes in.

What the runtime can't do yet, so rigs approximate it:

* **break actions** (the Duplet, the Ashot): no hinge to open (the barrels tipping down about their
  pin, the extractor lifting the shells, closing it to fire); the shells go in at the closed
  breech;
* **bolt actions** (the Ventil, the Preved): no cycling between shots (lift, back, forward, down);
  the bolt racks after a reload only;
* **levers and cranks** (the Kalash 2012's `pumpachka`, the Tihar's pump lever, the Abzats's crank):
  parts that swing rather than slide stay still (no `bolt_bones`), though the handle still racks;
* **pressure** (the Tihar, the Helsing): the game pumps air that drops with each shot; here the pump
  only racks;
* **loading round by round** and **swinging a cylinder out**: magazines go in whole;
* **a cupping support hand** on a pistol, and **support hands in the game's aiming pose** (the
  VSV's support hand, in its hip pose, covers the front of its sight picture).

## 5. The game's facts

For Metro 2033 Redux (each reads the game read-only):

* **Rate of fire**: offset 392 of each gun's own config in `content/config.bin` is rounds per minute
  (AKSU 700 > Kalash 620 > RPK 500, as the real guns rank):
  `cargo run --release -p metro-assets --example config_values -- --at 392 ak aksu rpk`;
* **Caliber**: the bullet section and cartridge case the gun's config names (`bullet` and
  `cc_545`: 5.45; `bullet_buckshot`: 12 gauge; `bullet_medium`: the revolver's; `bullet_ball`: ball
  bearings; `bullet_bolt`: darts; `bullet_big`: 12.7 mm):
  `cargo run --release -p metro-assets --example config_strings -- 'weapon\rpk\rpk'`;
* **Fire mode**: guns whose recoil has long-burst tracks fire full auto:
  `cargo run --release -p metro-assets --example recoil_table -- saiga`;
* **Magazine size**: the index (the game's magazine overlays, `ak_magazin_45`); the model's rounds
  otherwise;
* **Recoil**: the extractor's `recoil/<key>.rvk`, loaded for every gun that has one. The protocol's
  kick table holds 256 kicks, fewer than all the guns' together: every gun keeps one kick of each
  kind (its recoil for every way of firing), the near table's guns all their variants
  (`physical::fit_kicks`).

Where the game has no number (masses, a missing config), use a real-world reference for what the
gun copies, and say so in its comment.

## 6. Check it

```sh
RUSTVR_ASSETS=X:/rustvr/local_assets cargo test -p rustvr-range
```

checks every rig and renders it held the way the runtime holds it into `captures/rig/`
(`RIG_ONLY=` picks guns). Look at the pictures:

* `<gun>_sights.png`: down the sights, magnified. The red dot is where the shot goes; it should
  sit on the front post's tip, the post standing in the notch level with (or just under) its
  shoulders. A post hidden behind the rear sight means the line runs under the notch; a dot
  floating free means it runs over the post;
* `<gun>_side.png`, `_left.png`, `_top.png`, `_under.png`: the hands on the grip and the handguard;
* `<gun>_grip.png`, `_grip_left.png`, `_grip_below.png`, `_grip_section*.png`: the firing hand and
  the trigger finger (the game's pose);
* `<gun>_support.png`, `_support_right.png`: the support hand;
* `<gun>_parts.png`: the moving parts tinted (magazine red, bolt or handle cyan, trigger yellow):
  nothing missing, nothing extra;
* `<gun>_markers.png`: the rig's points (red grip, yellow trigger, green line of sight, blue
  foregrip, magenta well, white muzzle, cyan handle);
* `bench_top.png`, `bench_player.png`, `table_*.png`: where the range lays the guns out.

The numbers checked: the line of sight passes through both sights; the hands drawn are exactly
the game's (`metro_arms_hold_their_guns`, `tests/handpose.rs`: the same trigger contact and
fingertip depths as the game's own arms); the magazine is its own small part with the well on it;
the guns lie apart on the tables; every gun keeps its recoil (`tests/recoil.rs` plays it). Then the
physical scenarios, which pick guns up, aim, reload and fire through the real runtime:

```sh
cargo build --release -p rustvr-runtime -p rustvr-range
target/release/rustvr --sim all --launch-range
```

## 7. In an adapter

An adapter loads the same rigs (`rustvr_asset::rigs::load("metro2033")`) and registers each gun as
an `ObjectDesc` the way the range does (`content::rigged_metro_weapon`, `physical::weapon_desc`):
weapon space from the sights and the grip, `grip_rot` and `support_rot` from the game's hands (or
the measured hold), `handle` and `handle_travel`, `mag_well`, the magazine template's `ammo_type`,
the fire rate and mode, mass, and the game's hand poses and kicks in their tables.
