# The duel: a second melee mode

The framework has two melee modes. The **physics sandbox** ([MELEE.md](MELEE.md)) is the one it
has always had: weapons with real weight that hit by their momentum, blocks by meeting, limbs that
come off. The **duel** is slow, weighty and timing-based: swings that wind up and recover, a short
parry window opened by pushing your blade into the blow, a counter after a parry, staggers, and no
way to stop a blow but a parry. The hands, the weapons, the hits and the damage are the sandbox's
own; the duel changes what happens when blades meet, how heavy a weapon is in the hand, and how
fast you walk with one drawn.

Every number of the duel is in one place: `crates/melee/src/duel.rs` (`rustvr_melee::duel`), for
the runtime (the player's side) and the range's duelist alike.

## Choosing it

* **The player**: `[melee] mode` in `settings.toml`, and the **Melee** row in the menu's Controls
  tab: *The game's* (the default), *Physics sandbox*, *Duel*.
* **The game**: `MeleeRules::mode` (`MELEE_PHYSICS`, `MELEE_DUEL`; 0 says nothing: the sandbox).
  With the player's setting on *The game's*, the game's choice is played.
* The runtime tells the game what's played: `RtMelee::flags` has `RTM_DUEL`, and `RTM_STAGGERED`
  while a weapon of the player's is staggered.
* **The range** asks for the duel while the player stands in its duel ring (`--arena duel`), and
  for the sandbox everywhere else.

## Research

What the duel takes from slow, timing-based melee games, from public sources. These are design
comparisons only: nothing of theirs is in the repositories.

**Mordhau's cadence** (patch notes and community guides, below):

* **Parry**: right-click opens a window; one patch shortened it from 400 to 350 ms. Its recovery
  (when no new parry opens) went from 650 to 350 ms in the same patch. A parry that meets nothing
  leaves you in that recovery.
* **Riposte**: the attack straight after a successful parry; its input window was raised from 150
  to 400 ms. Guides describe it as having no wind-up animation; it can't be feinted or
  interrupted. It beats the parried attacker's next swing because they're still in recovery:
  they may parry it but can't attack through it.
* **Being parried / missing**: a missed swing costs stamina and starts a recovery in which you
  can neither attack nor parry; the parried attacker can't attack before the riposte lands.
* **Hits**: a weapon's "can flinch" decides whether a hit interrupts the enemy; in a simultaneous
  exchange the first to land flinches the other. A swing in its release is otherwise committed:
  trades happen.
* **Chambers** (parrying by attacking into the same direction), **feints** (cancelling a wind-up,
  costing stamina, about 10), **morphs**. **Stamina**: attacks, feints and being parried drain it;
  parrying at low stamina disarms you.
* **Drags and accels**: not a system, a free-flow skill. Moving the view against the swing drags
  it (the blade lands later, baiting an early parry); with it, accels it (the blade lands sooner
  than the parry can react). Over-dragged swings can't hit behind you and do less damage.

**The weight of Dark Souls and Elden Ring**: commitment (an attack, once started, plays out;
recovery after it is when you're punished), poise and stance (hits stagger by weapon class, and a
broken stance opens a critical counter), and slower, deliberate movement with a weapon out. The
duel takes the commitment and recovery, the stagger that opens a counter, and the slower walk.

**What VR players say about parrying**: purely physical parries feel janky, "a lottery" when a
slightly wrong angle clips through; parries that always work once your weapon is on a marker feel
cheap; melee that never staggers anything feels cheap too. The duel keeps the blades physical
(you really have to meet the blow) but judges a parry by intent (a fast push into the swing,
inside a window), not by a precise angle, and every parry and every parried swing staggers.

Sources: [Mordhau patch notes (parry, recovery, riposte windows)](https://steamdb.info/patchnotes/13618125),
[Mordhau patch notes (release timing, miss recovery, cancels)](https://steamdb.info/patchnotes/4423941),
[Mordhau wiki: Combat](https://mordhau.fandom.com/wiki/Combat?oldid=3196),
[ludo.guide: Mordhau weapon swing mechanics](https://www.ludo.guide/guide/mordhau/getting-started/core-mechanics-explained/weapon-swing-mechanics),
[Mordhau guide for beginners](https://steamah.com/mordhau-guide-for-beginner/),
[Twinfinite: how to riposte in Mordhau](https://twinfinite.net/guides/mordhau-riposte-how/),
[Elden Ring wiki: Poise](https://eldenring.wiki.fextralife.com/Poise),
[Elden Ring wiki: Stance](https://eldenring.wiki.gg/wiki/Stance),
[r/virtualreality: Behemoth's VR combat](https://lr.psf.lt/r/virtualreality/comments/1h971qi/behemoth_vr_combat_system_is_quite_bad/).

## The design (the user's words first)

* **A parry is pushing your blade fast into the incoming swing.** A blade pushed faster than
  1.8 m/s opens a parry window of 0.3 s. A creature's swing it meets inside the window, moving
  into it (towards the other blade, or against its motion), is parried. A window that meets
  nothing closes into 0.35 s when no new one opens, and a new one needs the blade to slow first:
  waving the sword about isn't a guard.
* **The parry's ricochet sets up the counter.** The blade is thrown back off the other at 2.6 m/s
  or more (the push turned back), and for 0.9 s the weapon is held nearly as strongly as in the
  sandbox: it has the way and the lightness for the counter, swung however you like (left, right,
  over, under: it's your arm).
* **Being parried staggers you.** The runtime can't move your arm, so the stagger lives in what it
  controls: the blade is knocked back off the parry at 3 m/s, the weapon goes heavy in the hand
  (held at 0.18 of the sandbox's strength, easing back over the stagger's second half) and does no
  harm for 0.85 s, and the hand feels a long heavy buzz. The creature's riposte (a 0.28 s wind-up)
  lands well inside that: your next swing loses to it. You can still parry it once 0.3 s have
  passed.
* **Nothing but a parry stops a blow.** No passive block: in a duel the creatures' weapons are
  solid to nothing the player holds. A blade only held in the swing's way, too slow, or pushed
  outside its window, is beaten aside (knocked along the blow at 2 m/s) and the blow carries on
  into you. A hit on a creature while it winds up holds its wind-up back 0.15 s (a flinch); while
  it releases, it does nothing to the swing. A blow lands once.
* **Momentum.** The hands swing a weapon easily but stop it hard. On top of holding it level
  against its weight, a hand bringing it after itself pushes with up to 150 N and turns it with up
  to 20 N m (two hands on its handles 250 N and 35 N m); but a weapon going faster than the hands
  (they've stopped or slowed, and it hasn't) is braked with only 20 N and 6 N m (35 N and 12 N m
  with two). So a committed swing carries on past where the hands stop, pulling them (and the arms
  and body drawn after them: a tracked elbow goes with its hand) away from the controllers until
  they've braked it, by the weapon's own mass and length. Staggered, the hands bring it after them
  with a quarter of their strength; in a riposte, a quarter more. The stick walks you at 0.55 of
  its speed with a melee weapon in hand. Started in the duel area, the range plays the duel's rules
  across the whole room.
* **The grip's angle** (every melee, not only the duel): a weapon leans forward in the hand from the
  controller's handle (30° by default, the Controls tab's *Sword angle in the hand*), turned about
  the knuckles away from the controller's aim ray, as a hilt held upright sits in a fist; along the
  handle it leaned back. It's set as the weapon is taken.
* **Drags and accels need no system.** The blade is yours: a hit lands when it really reaches the
  creature, so turning your body away really delays it and turning into it really speeds it up.
* **The creature is fooled by them.** It reads your blade as a person would: it sees it 0.22 s late
  (give or take 0.05 s), judges when it will reach it from how fast it was closing then and how
  that was changing (a swing gathers speed), and opens its parry so the blow lands 0.1 s into the
  window, its blade put across the blow's way. Once open it can't re-time it. A drag slows the
  blade after it judged it: the window closes before the blade comes. An accel brings the blade
  before it has seen it coming.
* **The creature's swings are slow and readable**: a 0.8 s wind-up (stepping in), a 0.65 s release
  (lunging; the blade's tip about 6 m/s at its fastest), a 0.45 s recovery (0.7 s after a swing
  that met nothing). It swings from its right, its left, over the top or from under, never the
  same side three times running, 0.6 to 1.6 s apart. It parries, ripostes and is staggered by the
  same rules as the player. Early in its own wind-up it may break off to parry.

## Where it lives

* `crates/melee/src/duel.rs`: every timing and weight.
* `crates/runtime/src/duel.rs`: the player's side (the parry window, the clashes, the stagger and
  riposte, the hold's strength, the walk); `rig/duel.rs`: the hold, the kicks off a clash, and the
  creatures' weapons passing through what the hands hold. `combat.rs` calls the duel's clashes in
  place of blocks, and skips a staggered weapon's hits; `session.rs` sets the mode and the walk.
* The protocol: `MeleeRules::mode` (from its padding: the layout is unchanged), `AI_PARRYING`
  (a creature's parry window), `CEF_PARRIED` (a creature's parry turned the player's blow away),
  `RTM_DUEL`, `RTM_STAGGERED`.
* The range: `arenas.rs` (the ring), `duel.rs` (the duelist's brain and keyframes), `figure.rs`
  (its Oblivion figure), `arena.rs` (it among the range's people).

## The range's duelist

One duelist stands across the duel ring (`--arena duel`) and fights whoever comes into it. It's a
range person (the ragdoll of [MELEE.md](MELEE.md), hurt by the sandbox's rules), with:

* **Our own animation.** Its swings and parries are keyframes of the sword hand (where the wrist
  is, how the blade and its edge lie, how the torso turns), posed through the arm's reach. Its
  sword is led along them (its hand following it), so a swing lands where and when it's keyed;
  on its hold's soft joint it lagged so far that its blade stayed upright through a swing.
* **Oblivion's figure.** Drawn as the game's male body in iron armour (cuirass, gauntlets,
  greaves, boots, the Imperial head), from the user's own `local_assets/oblivion/`, put together
  as the runtime puts the player's body together and posed every frame from the ragdoll's parts
  through the avatar's retargeting, skinned on the CPU and drawn lit and in the range's depth.
  Whatever its body does (its keyframes, its hits, its falls), the figure does. Without the files
  it's drawn as its parts.
* Its limbs stay on (a cut through one is a cut), a blow lands on it once a swing, it takes its
  sword up again if disarmed, and 4 s after it dies another stands in its place.

## Checking it

`--duel-test cue:<right|left|over|under>` drives the duelist for a test: it stands 1.45 m before
the player's start and swings from that side only when a hand is raised well over the head.
`--duel-test guard` has it never swing but to riposte. The emulated headset's scripts are in
`tools/xrsim/duel_*.toml`, run on a link of their own ([XRSIM.md](XRSIM.md)):

```sh
rustvr-xrsim run --dir runs/parry --script tools/xrsim/duel_parry.toml -- rustvr-play.exe -- --arena duel --duel-test cue:right
rustvr-xrsim run --dir runs/guard --script tools/xrsim/duel_guard.toml -- rustvr-play.exe -- --arena duel --duel-test guard
```

Checked headless (2026-11-01, each run twice, the same each time; the runtime's `duel:` lines and
the range's `duelist` lines say what happened):

1. **A parry within the window counters**: pushed into the swing 0.02 to 0.04 s into the window,
   the swing is parried (`CEF_PARRY`), the duelist staggered, and the counter 0.4 s later lands
   while it's still staggered.
2. **A mistimed parry is hit**: pushed 0.6 s early, the window and its recovery are over when the
   swing comes; the blade held in its way is beaten aside and the player struck, once.
3. **Parried, the attacker's swing loses to the counter**: the duelist parries the player's
   forehand (`CEF_PARRIED`), the player's next swing does nothing, and its riposte strikes the
   player 0.6 s after the parry.
4. **A dragged swing makes it parry early**: a forehand fast at first, then held while the body
   turns away, then on round: its parry opens on the fast start, meets nothing, and the blade lands
   in its recovery.

And a unit check (`cargo test -p rustvr-range duel`): a blow dragged after the duelist judged it
lands after its parry's window; a steady one lands inside it. Not checked headless: how the
weight, the stagger's heaviness, the ricochet and the buzzes feel, and the slower walk (the stick's
walk scaled with a weapon drawn): that's the playtest's.

## Not done

* Stamina, feints, chambers, morphs and kicks in the duel; a riposte that can't be feinted.
* The duelist's sword is the range's arming sword, not Oblivion's; one duelist only.
* Oblivion: its adapter's melee still plays the sandbox; the duel goes in once it's played here.
