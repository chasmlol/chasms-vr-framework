# Rigging weapons for RustVR

A **rig** says how the player's hands hold a game's gun: where the line of sight runs, where the
firing hand and the support hand go, which parts of the model are the magazine, the bolt and the
trigger, where the magazine seats and the muzzle points, and a few facts (rate of fire, weight,
magazine size, caliber). With a rig, the runtime's player controller (DESIGN.md §6b) can pick the
gun up, aim it down its own sights, reload it with its own magazine and rack its own handle.
Sections 0 to 7 are guns; melee weapons, knives, bows and torches are rigged by their grips (§8).

Every weapon snaps to its rigged grip when it's grabbed near it: a gun's pistol grip locks the
hand on, a handle takes the hand anywhere along it, turned as it's made to be held (or reversed).
Grabbed anywhere else, a thing is held where the hand took it. Drawn from a holster or caught from
the gravity gloves, a rigged weapon goes into the hand by its grip wherever the hand is.

Rigs are **data**, measured on the game's own models: a game's rigs are one file of ours,
`<game>.toml`, kept with its adapter (the models they're measured on stay in the user's own
`local_assets/`, never committed). `rustvr_asset::rigs` parses and checks them (`rigs::parse`
refuses a two-handed gun without a foregrip, a hinge on a self-loading gun, a bolt action without
its handle, unknown fields and the like), and `rigs::load(game)` reads `<game>.toml` from the folder
`RUSTVR_RIGS` names, so an edit is tried without rebuilding.

No game's guns are rigged yet. The format, its checks and the measuring tool are here; making a
gun (`rustvr_guns::Weapon`) from its rig and its model is its adapter's, and until then the runtime
doesn't draw a game's gun itself (§7). The test range's own guns (a rifle, a scoped rifle and a
pistol, `crates/range/src/content.rs`) are made in code, in the same weapon space, and checked the
way a rigged gun is (§6).

The rule is the project's: **the game is the source of truth**. The hands are the game's own
(its first-person arms posed for the gun), the recoil is the game's (its kicks), the rate of fire,
fire mode, magazine size and caliber come from its data where it has them. What we measure is
geometry the game doesn't state (the sights, the magazine well), and every measurement is checked
by looking at it, held and down the sights.

## 0. Spaces

* **Model space** is the gun model's own, as the extractor caches it (`crates/asset`): left-handed
  metres, X right, Y up, Z forward, in the game's first-person placement (relative to its
  first-person camera, so a gun and the arms holding it line up). Every point in a rig is in model
  space, and the tools print model space.
* **Weapon space** is what the runtime sees (`ObjectDesc`): the line of sight along +Z, the origin
  at the rig's `grip`. A gun's model goes into it by `Weapon::fix = R * T(-grip)`, with `R` turning
  the line from `rear_sight` to `front_sight` onto +Z. So the line of sight *is* the gun's forward:
  shots leave along it, and aiming down the sights puts the eye on it.

## 1. Get the game's guns

A game adapter's extractor writes the game's first-person guns, the arms posed for each and their
recoil into `local_assets/<game>/` (`crates/asset`'s format). The index (`index.toml`) names each
gun's arms (`hands`), the game's own aim line where it has one (`rig`) and, where the game says, its
magazine size (`mag_size`). The tools find the folder through `RUSTVR_ASSETS` (or `local_assets/`
at the repository root).

## 2. Decide what kind of gun it is

* `hold = "rifle"` (the default): a pistol grip and a handguard, shouldered;
* `hold = "pistol"`: one hand; it holsters on the hip (the runtime has no cupping support hand, so
  a game that cups a pistol with its other hand only gives us the firing hand);
* `hold = "heavy"`: two hands on handles that aren't a pistol grip and a handguard (a spade grip, a
  side handle, a handlebar with a lever trigger), fired from the hip: no stock to shoulder.

## 3. Measure it

```sh
cargo run --release -p rustvr-asset --example rig_probe -- <game> <key> <mode> ...
```

measures a cached model. Its modes:

* `parts`: every bone carrying skin (each vertex counted for its heaviest bone) with its bounds,
  then every mesh with its index, texture and name: which parts are the trigger, the magazine and
  its rounds, the bolt or handle, the barrels;
* `bones`: every bone's bind pose (its origin, the pivot the game's motions turn its skin about,
  and its axes) under its parent; bones without skin are the game's locators (a muzzle point, where
  the magazine sits, the ejection port);
* `slab <axis> <lo> <hi> ...`: the bounds of the vertices inside a box, per mesh and per bone;
* `rows <axis> <lo> <hi> <step> ...`: the vertex bounds of each slice;
* `cuts <axis> <lo> <hi> <step> ...`: cross-sections, the bounds of where the triangles cross each
  plane, so it doesn't matter where the model happens to have vertices. The precise one: `cuts z
  1.60 1.66 0.005 x 0.139 0.143 y -0.07 0` finds a post inside a hood;
* `hand`: the arms' canonical hand frames (Y to the middle finger's base, Z the thumb side) in their
  bind pose: where the game's palms are on the gun.

The measurements every gun needs:

* **the sights**: `cuts` along the line of sight over the top of the gun, the top of the gun on the
  line and of its shoulders either side: a notch is shoulders above the centre, a post a peak on
  the line (§4.1);
* **the pistol grip**: its section every centimetre down from the receiver under the game's right
  palm (`cuts y`), the axis fitted through the middle of the hand-hold and its rake, and the grip as
  a block;
* **the trigger**: its bone's bounds, and the pad point 40% up the blade, 2 mm behind its face;
* **the handguard** at the game's left palm: its section and where the palm is under it.

## 4. Fill in the rig

Every field of a `[[gun]]` (`rustvr_asset::rigs::GunRig`); points are model space, metres:

| Field | What | How it's measured |
|---|---|---|
| `key` | The gun's key in the asset index | |
| `name` | Shown to the player (optional; the index's) | |
| `hold` | `rifle`, `pistol` or `heavy` (§2) | The gun's shape and how the game holds it |
| `caliber` | What it fires, as the game calls it (`5.45x39`, `12 gauge`...) | The game's ammo for it (§5) |
| `magazine` | The magazine it takes; guns naming the same one take each other's (optional: its own) | The game's magazines, and the meshes being the same at the same place |
| `rear_sight`, `front_sight` | Two points on the line of sight: the top of the rear notch's shoulders and the front post's tip | §4.1 |
| `grip` | The firing hand's palm centre on the pistol grip's axis: the weapon's origin, and the hold without the game's arms | The grip's axis level with the game's palm (§3) |
| `grip_rake_deg`, `grip_twist_deg` | The grip axis's rake back from vertical; an extra turn of the hand about it (default -15) | The axis fitted through the grip's sections |
| `grip_block` | The grip as a block: `centre`, `half` (across, front to back square to the axis, along it) | The grip's sections; drawn in the `grip_section` renders |
| `trigger` | Where the trigger finger's pad rests on the trigger | The pad point (§3), or the blade's face by `cuts` |
| `trigger_bone` | The trigger's bone: its skin swings with the finger, about the bone (`""`: no trigger part) | `rig_probe parts` |
| `foregrip` | Two-handed guns: the support hand's palm centre (the game's left palm) | `rig_probe hand`, and the handguard's section under it |
| `support_tube`, `support_palm` | The support fist's axis and palm normal, used only without the game's arms (optional) | `rig_probe hand` |
| `mag_well` | Where the magazine's top sits when it's in (on the magazine's bounds) | The game's magazine locator, or the magazine's top or the rims of its rounds |
| `muzzle` | The bore's end | The game's muzzle locator when it's at the barrel's end (some sit past the model: then the barrel's end on the bore) |
| `handle`, `handle_travel` | The part pulled back to chamber (charging handle, bolt knob, slide, pump, cocking lever) and how far it travels (m); none: nothing to rack | The handle's knob from the parts; travel from the real gun or the game's motion (§4.4) |
| `mag_bones` | Bones (and everything under them) whose skin is the magazine; a trailing `*` matches any ending | `rig_probe parts` |
| `mag_meshes` | Whole meshes (by index) that are the magazine too: one the extractor mounted on a bone that isn't the magazine's | `rig_probe parts` lists meshes |
| `bolt_bones` | Bones whose skin moves back with the handle | `rig_probe parts` |
| `hidden_bones` | Bones whose skin isn't drawn: what the game shows only in an animation or with an upgrade (a shell in the hand, an ejected case, an upgrade's barrels' shells) | The renders (things floating, or doubled) |
| `hidden_meshes` | Whole meshes (by index) that are glass: a gauge's (over its needle), a reflex sight's lens and reticle. The test range can't draw them see-through and leaves them out; the runtime draws them (`Weapon::glass`) with the game's forward shading, or leaves them to a special draw where it doesn't reproduce the game's shader | `rig_probe parts` lists meshes and their textures |
| `action` | How it's worked between shots: `auto` (default: it cycles itself), `bolt`, `pump`, `break`, `swing` (§4.4) | What the gun really is, and the game's motions |
| `hinge`, `open_deg`, `barrel_bones` | Break and swing actions: a point on the hinge's pin, how far it opens (degrees; a break action about model +X tipping the muzzle down, a swing about +Z, positive to the left) and the bones that turn (with their children) | The barrels' bone's origin (`rig_probe bones`) and the game's reload's furthest turn of it (§4.4) |
| `chamber_bones` | Guns loaded round by round into their chambers: each chamber's round (a bone whose origin is its base at the chamber's mouth), in the order they fire. No `mag_bones` then; `mag_well` is the first chamber's mouth, `mag_size` the chambers | `rig_probe parts` and `bones` |
| `round_bones` | Guns whose magazine stays in them and is loaded one round at a time (a revolving shotgun's cylinder, a dart cluster, a tube): each of its rounds (a bone whose origin is its base), in the order they're loaded and fired. No `mag_bones` then; `mag_well` is the port (the first's base), `mag_size` the rounds | `rig_probe parts` and `bones`; the game's reload (does it load one at a time?) |
| `bolt_turn` | Bolt actions: the bolt's turn about the bore (degrees about model +Z, positive lifting a handle on the right) closed and open, from the model's pose. `handle` is the knob in the model's pose | The game's cycling motion (§4.4), else the real gun's lift |
| `handle_swing` | A handle that's a lever: its pivot (model space) and swing (degrees about model +X, positive its far end down and back) at full travel; `bolt_bones` are the lever, `handle_travel` how far the handle comes back over the swing | The lever's bone origin and the game's pump motion |
| `pressure` | Air guns: `pump` (what a whole stroke of the `handle` adds) and `shot` (what a shot spends), fractions of full, and `min` (below it the trigger hisses); `gauge_bone` with `gauge_turn` (degrees about the bone's own X) or `gauge_slide` (metres along it), empty and full, from the model's pose | The game's gauge poses (its needle's motions, §4.4); the numbers from the game's data if it names them, else documented guesses |
| `recoil`, `recoil_feel` | The runtime's own recoil, used only when the game's kicks aren't loaded: climb (deg), scatter (deg), push (m); climb kept on auto, return (1/s), lean (deg) | By class: rifles about `[7, 1.8, 0.025]`, shotguns `[10, 2.5, 0.04]`, pistols `[12, 3, 0.03]` |
| `rounds_per_second` | Rate of fire | The game's data (§5) |
| `automatic` | Full auto (default true) | The game's data (§5) |
| `slide_locks` | The slide locks back on the last round | The real gun |
| `mag_size` | Rounds per magazine (optional: the index's) | The index; else the model's rounds |
| `mass`, `mag_mass` | Kilograms: the gun without its magazine, and a loaded magazine | Real-world references for what the gun copies (documented per gun) |
| `left_side_up` | Lies on a table with its left side up (a magazine sticking out of the left) | The gun's shape |
| `ads_zoom` | Zoom while aiming (above 1.4 opens a magnified scope; default 1.1) | |

Write down, in a comment above the entry, what you measured and how: the next person re-checks
it from there.

### 4.1 The line of sight

The line of sight must run through the gun's own sights as a player lines them up, because in VR
the eye really is on it: shots go where the front post's tip is.

1. **A notch and a post**: the rear point is the notch's *shoulders' top*, the front point the
   post's tip, both by `cuts`. The true pair lines up with the game's aimed camera (the game shows
   its sight picture from there), within a few millimetres. Several notches on a long leaf or a
   slider all line up within a millimetre or two; the sights render decides (§6).
2. **A ring, a peep or a reticle**: the ring's centre and the post's tip (or the reticle's centre),
   by `cuts`.
3. **No pair lines up with the camera** (a camera held over the sights, a leaf higher than the
   camera's line): measure the notch and the post by `cuts`, and check the render.
4. **No sights at all** (a bare barrel): the game's aim line, which is how the game aims it.
5. **A hip-fired gun whose aim line runs through its own body**: the bore (through the muzzle),
   which is where its shots come from.

### 4.2 The hands

Where the game has first-person arms for a gun (the index's `hands`), they are the hold: the arms
in their bind pose (the idle the extractor baked) put the hands on the gun, and the controllers go
where the drawn hands land exactly on them (`GunHand::from_arms`, `controller_on` in
`crates/guns`). So `grip`, `grip_rake_deg`, `foregrip` and the rest are the fallback for a gun
without arms, and `grip` is the weapon's origin. Given the gun's surface and its trigger, the pose
is fitted for close viewing: fingertips the game sank into the gun come back out to its surface,
and the index finger's pad goes onto the trigger. Nothing else is adjusted.

### 4.3 Moving parts, magazines and handles

The magazine is the part that comes out: a box, a drum, a tube of balls or darts, or the rounds
of a cylinder taken as one (a revolver's six rounds with their cases, a revolving shotgun's six
shells: a speed loader). Guns loaded straight into their chambers (a double barrel) have
`chamber_bones` instead (§4.4). Seated, it sits exactly where the model has it (magazine space is
the gun's own), and the runtime seats a held magazine when its well point comes within 9 cm and
45° of the gun's (`[interaction]`'s `easy_mag`, the menu's "Easy magazine insertion", on by
default; 4 cm and 25° with it off). Guns naming the same `magazine` share one magazine template
(and ammo type, the range's `physical::ammo_types`: `caliber << 8 | magazine`, both numbered in
order of first appearance), so give it only to guns whose magazines are the same mesh at the same
place. A gun without `mag_bones` keeps its rounds (a flamethrower's fuel) and has no reload.

`handle` is whatever is pulled back to chamber: the runtime moves `bolt_bones` straight back by the
pull (a lever's swing about its pivot with `handle_swing`). Map a **pump** to its forend (its back
end, well behind the support hand, so reaching for the handguard doesn't grab it instead) and a
**bolt** to its knob. Leave `handle` out for guns that chamber by themselves (a double-action
revolver, a revolving shotgun) and for hinged actions: a fresh magazine then chambers as it goes
in.

### 4.4 Actions: guns worked by hand

`action` says how a gun that doesn't cycle itself is worked; the runtime does the rest
(`crates/runtime/src/rig/actions.rs`, DESIGN.md §6b) and reports where the parts are, so the game
(and the range) draws them. Measure the moving part's **pivot** on the model (`rig_probe <game>
<gun> bones`: every bone's origin, the point the game's motions turn its skin about) and its
**travel** from the game's own motion (its reload or cycling): how far the motion turns the bone
from its bind pose about its own axis, or slides it, at its furthest. Without a motion, the real
gun's.

* **`break`** (a double barrel): the barrels' bone and its children turn about `hinge`, model +X,
  by `open_deg` (the game's reload's furthest). A foregrip ahead of the hinge is on the barrels: the
  support hand on it opens and shuts them.
* **`swing`** (a revolver's cylinder on its crane, a barrel that swings out sideways): the same
  about a pin along the bore, model +Z, positive to the left. Check it's really a swing (which axis
  the game's reload turns it about), not a tip-down break.
* **`chamber_bones`**: a gun loaded round by round (a double barrel's two shells, a single shot's
  one), in the order it fires them. The pouch hands one round, its mesh the first chamber's.
* **`round_bones`**: a magazine that stays in the gun and takes its rounds one at a time, at any
  time, through its port (a revolving shotgun's cylinder, a dart cluster, a tube), as the game loads
  them; the pouch hands one round, and the gun's rounds are drawn as many as it holds.
* **`bolt`**: `bolt_turn` is where the game's cycling motion has the bolt closed and lifted,
  relative to the model's own pose (which may have it neither), `handle_travel` how far it comes
  back. With no game motion, the real gun's (a Mosin-Nagant's 90° lift).
* **`pump`**: a pump action's forend.
* **`pressure`**: an air gun; its `handle` pumps. The gauge's poses are the game's (its needle's
  poses over its dial), the pressure's numbers ours when the game's data names none: say so in the
  comment.

Check them in the renders (§6): `<gun>_open.png`, `_half_open.png`, `_open_parts.png` (barrels
orange, chambers' rounds green), `_open_empty.png`, `_breech.png`; `_bolt_closed.png`,
`_bolt_up.png`, `_bolt_back.png`; `_pump_home.png`, `_half_pumped.png`, `_pumped.png`;
`_gauge_empty.png`, `_gauge_half.png`, `_gauge_full.png` (gauge blue in the parts view);
`_two_rounds.png` (a magazine loaded round by round with two left).

What the runtime can't do yet, so rigs approximate it:

* **cranks and swinging levers that aren't pumps**: parts that swing rather than slide stay still
  (no `bolt_bones`), though the handle racks;
* **over-under barrels**: `chamber_step` runs across only;
* **a cylinder turning** as it fires: the rounds it holds are drawn in their places in order, the
  cylinder itself stays still;
* **a cupping support hand** on a pistol, and **support hands in the game's aiming pose** (one
  held in its hip pose can cover the front of the sight picture).

## 5. The game's facts

From the game's own data, where it has them:

* **Rate of fire**: the gun's rounds per minute;
* **Caliber**: what the game says the gun fires (its ammo), named as the game names it;
* **Fire mode**: full auto where the game fires it so (its recoil's long-burst kicks say so too);
* **Magazine size**: the index's `mag_size`; the model's rounds otherwise;
* **Recoil**: the extractor's `recoil/<key>.rvk`, loaded for every gun that has one. The protocol's
  kick table holds 256 kicks (`MAX_KICKS`), fewer than a game's guns may have together: every gun
  keeps one kick of each kind (its recoil for every way of firing), the near table's guns all their
  variants (the range's `physical::fit_kicks`).

Where the game has no number (masses, a missing value), use a real-world reference for what the
gun copies, and say so in its comment.

## 6. Check it

```sh
cargo test -p rustvr-range --test rig
```

renders the range's guns held the way the runtime holds them into `captures/rig/<key>_<view>.png`
(`RIG_ONLY=<key>,...` picks guns) and checks them. Look at the pictures:

* `<gun>_sights.png`: down the sights, magnified. The red dot is where the shot goes; it should
  sit on the front post's tip, the post standing in the notch level with (or just under) its
  shoulders. A post hidden behind the rear sight means the line runs under the notch; a dot
  floating free means it runs over the post;
* `<gun>_side.png`, `_left.png`, `_top.png`, `_under.png`: the hands on the grip and the handguard;
* `<gun>_eye.png`, `_eye_runtime.png`: the shooter's view, exactly and with the runtime's depth
  bias (what the player sees);
* `<gun>_grip.png`, `_grip_left.png`, `_grip_below.png`, `_grip_section*.png`: the firing hand and
  the trigger finger;
* `<gun>_support.png`, `_support_right.png`: the support hand;
* `<gun>_parts.png`: the moving parts tinted (magazine red, bolt or handle cyan, trigger yellow,
  barrels or cylinder orange, chambers' rounds green, gauge blue): nothing missing, nothing extra;
* the action worked (§4.4): `<gun>_open.png` and the rest, the parts where the runtime puts them;
* `<gun>_markers.png`: the rig's points (red grip, yellow trigger, green line of sight, blue
  foregrip, magenta well, white muzzle, cyan handle);
* `bench_top.png`, `bench_player.png`, `table_*.png`: where the range lays the guns out.

The numbers checked: the line of sight passes through both sights, with the muzzle a few
centimetres under it (a rifle's or a pistol's); the hands drawn are exactly where their grip poses
put them, the palm facing the grip and the trigger in reach of the index finger; the guns lie apart
on the tables. Then the physical scenarios, which pick guns up, aim, reload and fire through the
real runtime (`pickup`, `mag_reload`, `holster`,
`two_hand`, `grip_handguard`; the range's `--table <key>` lays a gun nearest on the table at your
right hand):

```sh
cargo build --release -p rustvr-runtime -p rustvr-range
target/release/rustvr --sim all --launch-range
```

## 7. In an adapter

An adapter registers a gun exactly as the range does, through `rustvr_guns` (`crates/guns`,
shared by the range, the adapters and the runtime). The gun is a `rustvr_guns::Weapon`: its parts
cut from the game's model by the rig's bones, weapon space from the sights and the grip, the game's
hands and kicks. `desc::registration(w, ids, ammo_type, game)` makes its registration
(`desc::GunRegistration`: the gun's `ObjectDesc`, its magazine template, the game's hand poses and
its kicks). What it holds: weapon space from the sights and the grip, `grip_rot` and `support_rot`
from the game's hands (or the measured hold), `handle` and `handle_travel`, `mag_well`, the
magazine template's `ammo_type`, the fire rate and mode, mass, and the game's hand poses and kicks
in their tables.

With `game` (its folder in the asset cache) the gun and its magazines are flagged
`OBJF_RUNTIME_DRAWS`, for the runtime to draw them itself from the same model and rig, named
`<game>/<key>` (`desc::model_name`), with the game's lighting. The runtime doesn't draw a game's
gun yet (`held_objects::load` refuses one: no game's guns are rigged), so for now the game draws
its own (`game` left out).

## 8. Melee weapons and other things held by a handle

A sword, an axe, a shield, a knife, a bow or a torch has no rig file: its adapter describes its
grip in its `ObjectDesc`, measured on the model where the game's own hand holds it, and the runtime
snaps the hand onto it (`crates/runtime/src/rig/grips.rs`):

| Field | What |
|---|---|
| `grip` | The hold: the hand's controller grip pose's origin (object space) |
| `grip_rot` | Its turn there, the right hand's. The left hand takes the mirror image (a prop's is held the same way round, turned half round the fist's axis). A rigged weapon drawn or caught goes into the hand this way |
| `grip_line`, `grip_radius` | The handle's axis, end to end (a sword's grip from the pommel to the guard, an axe's haft, a staff), and its radius. A hand within 7 cm of it goes on anywhere along it, held as `grip_rot` holds it at `grip`, or the other way round it (a reversed grip); a second hand may hold it there too. All zero: a melee weapon's or a knife's handle 6 cm long through `grip`, along `grip_rot`'s fist |
| `OBJF_UPRIGHT` | Held only its one way up (a torch's flame up); a bow (`OBJ_BOW`) is never held reversed either |

A gun's pistol grip (`grip`, `grip_rot`) locks the hand on within 10 cm; its handguard
(`foregrip_line`, `foregrip_radius`; all zero: 10 cm of the bore through `foregrip`) takes the
support hand anywhere along it. A grenade with a `grip_rot` is always held that way.

A melee weapon the game also lists in `MeleeWeapons` (the melee hold, `docs/MELEE.md`) is
taken by its handles there instead, up to four (`MeleeHandle`):

| Field | What |
|---|---|
| `a`, `b`, `radius` | The line the fist closes round (object space), and how thick it is |
| `rot` | The fist's turn on it, the right hand's (the left holds it the same; a shield's turned half round the fist's axis). Zero: the hand's own turn about the line |
| `flags` | `HANDLE_MAIN` (where a first hand goes, and a weapon drawn or caught), `HANDLE_SLIDE` (a loosened hand slides along it), `HANDLE_REVERSIBLE` (held either way round), `HANDLE_BLADE` (on the blade, to half-sword) |
| `kind` | `GRIP_LIGHT`, `GRIP_HEAVY`, `GRIP_SUPER_HEAVY`, `GRIP_SUPPORTING`, `GRIP_HALF_SWORD`, `GRIP_SHIELD`, `GRIP_THROWING_KNIFE`: how strongly a hand holds and turns it there |
| `links` | The handle a hand sliding off this one's `b` (low byte) or `a` (next byte) goes on to, as its index + 1 |
| `strength`, `slide_force` | Its share of the hand's hold (0: 1), and the pull a loosened hand needs to slide (0: the runtime's) |

A hand within 7 cm of a handle's line (past its radius) goes onto it, a first hand onto the main
one whenever that's within reach. Worked examples: the range's weapons (`crates/range/src/arsenal.rs`,
handles on boxes of our own) and, in the Oblivion adapter, its weapons, shields, bows and
torches (`crates/oblivion/src/weapons.rs`, `archery.rs`, `torch.rs`: measured on each model, a
weapon's grip at its origin where the game's hand holds it, a shield's at its strap).
