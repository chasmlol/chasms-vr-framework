# Melee

The framework's melee: weapons with real weight in the hands, hits judged from real impacts, stabs
that stick, limbs that come off, creatures that are ragdolls held up by their muscles, and the
extras (throwing, telekinesis, slowed time, imbuing, breakable things, climbing). Its numbers are
tuned as one set, and every choice that needs a reason is written down here, under "The calls".

## Who does what

The runtime owns the hands and judges physics: what a weapon in the hand does when it meets a
creature, how hard, at what angle, whether it goes in, cuts through, or breaks something. The game
owns its world and its creatures: their health, what a hit does to them, their brains, their
bodies' physics. The two meet through the protocol, as everything else in the framework does.

* The game describes its melee weapons (`MeleeWeapons`): each weapon's handles (where hands hold
  it and slide along it) and its damagers (each with its numbers: its damage type, its angles,
  its damage curve, which of the damage tables it's judged by, its penetration depth and dampers,
  its tier). The runtime holds and judges them; a weapon without one is judged by the older,
  simpler `MeleeDesc`.
* The game lists what its creatures hold (`ActorItems`): the runtime's hands and weapons meet
  those things as solid, block and parry them, and are struck by them.
* The game says its rules for the player (`MeleeRules`): its strength, its stamina (when the game
  keeps one: Oblivion's Fatigue), and whether the player may slow time, climb or use telekinesis.
* The runtime tells the game, every frame (`RtMelee`): how fast the game's time should run, how
  the arms are moving the player while climbing, what the swings spent of the stamina, which
  weapons are filled with a spell.
* Hits and the rest are events (`CombatEvent`): `CE_HIT` (a judged hit: its damage, damage type,
  push level, depth, the blade, whether it severs, whether it was thrown, imbued, two-handed),
  `CE_WOUND` (a blade gone in deep, or out the far side), `CE_PULLED_OUT`, `CE_BLOCK` (a weapon met
  a creature's, a parry if it was moving into it), `CE_STRUCK` (a creature's swing hit the player),
  `CE_BROKE` (a breakable thing broke). The game decides what each does to its creatures.

The test range is the game that plays it all: `crates/range` (its dummies, its fighters, their
brains, its weapons and its areas), and `crates/melee` (`rustvr-melee`) holds the tables, curves
and body rules, shared by the runtime and the range.

## Holding

* **Weight** (`rig/melee.rs`, `rig/physics.rs`'s `Joint`). A weapon hangs on its hand's joint: a
  spring of 5000 N/m and a damper of 100 from the controller to the point held, at most
  3000 N; turning, 1000 N m a radian times a light or heavy handle's 1.1 or 0.9, a damper of 50
  times 0.9, at most 250 N m. It keeps its mass and its weight, so a dagger keeps up with the hand
  and an 8 kg maul lags a swing, swings on past a stop and is slow to turn. The joint is solved
  implicitly each step (the velocity that lands the weapon where the spring and damper would put
  it at the end of the step), so it's stable for any weight. The hand is drawn on the weapon,
  held back with it; an empty hand follows its controller exactly.
* **Two hands** double each hand's spring and force, one of them turns it twice as hard, and the
  two pulls a distance apart turn it besides (k d²/2, F d/2): leverage, so two hands swing a heavy
  weapon far better than one.
* **Sliding.** A hand on a handle that slides (a spear's shaft, a staff, a mace's haft) slides along
  it while its trigger is held, against a damper (300 at a light pull down to 20 at a full one);
  at a handle's end it stops, or goes on to the handle that continues it (up the blade to
  half-sword). Two-handed staffs slide through both hands.
* **Handles.** A weapon's handles say where it can be held (a sword's grip, its pommel, its blade
  for half-swording) and how the fingers close round them. A grab never moves the hand or the
  weapon: the hand takes the weapon where it is (the user's rule; it isn't pulled into a
  posed grip instead). The handle the hand is on only shapes the fingers and says whether the hold
  is reversed.
* A weapon stuck an arm's length (0.7 m) behind its hand, against something, is let go of (the stuck
  rule).

## Hitting

* **Judging a hit** (`crates/runtime/src/combat.rs`, `rustvr_melee::judge`). Each damager is swept
  from where it was to where it is (for a held weapon, where the hand would have taken it this
  step, so a blade stopped by a body is still judged at the speed it met it), against each
  creature part's capsule; the first touch is found by a binary search back along the sweep, and
  the contact is judged by the impact velocity's angles against the damager's frame
  (slash 35°/90°/60°, axe 45°/80°/80°, pierce 35°/35°/60°), its damage curve, the damage table for
  the damage type against the part's material and the weapon's tier, a bad angle falling back to a
  light blunt hit. A weapon's points are tried before its edges before its blunt parts, and a
  contact is one hit until it's lost; a collider isn't hit again within 0.1 s.
* **Damage types against materials**: the damage tables (slash, heavy slash, weak slash,
  axe, pierce, fine pierce, blunt, light blunt, punch, kick...) against flesh, leather, chainmail
  and plate, by tier: the minimum speed, the damage multiplier, the speed it goes in at, how deep,
  its damper, the push levels, and the knockout speeds. A fire-filled blade is judged by the fire
  rows first.
* **Stabs that stick** (`rig/embed.rs`). A point that goes in stays in: as deep as the hand pushes
  it against the dampers (with a hand on it going in and coming out, and with none), up to the
  damager's depth. A point pressed on a body goes in by pressure. It goes in deeper as it's pushed,
  it cuts as it's dragged (a slash slides along its cut 3 mm deep), it can be twisted, and it's
  pulled out against its damper. In past the part's deep depth (a head's 4 cm, a torso's 10) it
  deals the deep damage times the part's multiplier; out the far side, its skewer damage. A blade
  can go through two bodies.
* **Dismemberment.** An edge that went in at more than its dismemberment speed times the weapon's
  tier factor (2.5, 1.5, 1, 0.9, 0.8, 0.8), within 4 cm of a severable joint and square enough to
  the limb, severs it (`CEF_SEVER`); the game cuts it off and the runtime's gore makes the stump.
* **Knockouts.** A blunt hit to the head past the knockout speed of its row knocks the creature out
  (6 s), as the tables say (see the calls).
* **Impact feedback.** A hit's sound is picked by the damage type and what it met (a cut, a stab, a
  blunt thump, metal clashing, a blade into wood), as loud as it was hard (its intensity, 0.25 to
  15 m/s); metal sparks above an intensity of 0.5; the controllers buzz.
* **Blood.** Blood runs on the blade where it went in and drips from it; the game draws its wounds
  and bruises.

## Defence

* **Blocking** is physical: the player's weapon meeting a creature's attacking weapon stops it, and
  it's told to the game as `CE_BLOCK`.
* **Parrying**: a block with the weapon moving into the blow (over 1 m/s) is a parry (`CEF_PARRY`);
  the game's brain rolls its parry reactions (the swing turned back, nothing, or a recoil).
* **Disarming**: a hit on a creature's hand at push level 2, or a stab through its arm, makes it
  drop its weapon; a hand can also take a creature's weapon and pull it away (see the
  calls).
* **Armour**: each part has its material (leather, chainmail, plate), and the tables judge against
  it: a sword's edge glances off plate; a mace still hurts through it.

## Enemies (the range)

* **Active ragdolls** (`crates/range/src/ragdoll.rs`): thirteen parts, pinned to an animated pose
  with pins (position 1000/50, at most 1000 N; rotation 800/50, at most 100 N m), the arms
  pinned harder while swinging (spring ×2, damper ×0, force ×10) and guarding (×3, ×1, ×30). Its
  balance: standing, knocked down (its pins loosened to a destabilised ×0.5 spring and ×0.1
  damper, ×0.1 on the ground), getting up once still under 1 m/s for 0.5 s (its pins ramping back
  over 3 s), or limp. It weighs 65 kg. Falls and slams hurt.
* **Hit reactions by force**: each hit's push level (by the hit's speed against its row) picks
  a reaction for where it landed and what the creature was doing: a flinch, a stagger, a fall.
* **Melee AI** (`brain.rs`): stances (one sword, sword and shield, two swords), real swings at full
  speeds that can be blocked and parried, combos and feints; it guards against the player's blades
  coming at it; Easy, Medium and Hard tiers have their own numbers (how soon it attacks again, its odds
  of a recoil, of rolling...).
* **Fear and pain**: badly hurt it may flee; its face shows pain, it grunts, shouts as it swings,
  and its last breath; disarmed, it goes and picks a weapon back up.

## Grappling and unarmed

* **Grabbing** any part of a creature (the framework's creature grabs): the throat to choke (a hand's
  choke: one weapon dropped after 2 s, both after 5), a limb to drag or lift, letting go with a
  throw. A held creature that's awake struggles (escape tries: a try every 2 to 3 s, 10% at
  first, 5% more each time, at most 50%).
* **Hands hit**: every empty hand carries a punch damager over its palm and fingers, open
  or closed, judged by the same judge (Punch's curve, its table: 2 m/s on flesh at ×0.5, within 60°
  of square, never going in). It's swept where the controller takes it, not where the drawn hand
  stops on what it hits. Shoves are the hands' own contact pushes.
* **Kicks** need tracked feet, which a headset and two controllers don't have: not done.

## Extras

* **Throwing** (`rig/melee.rs`): a weapon leaves as it was moving; let go of with the hand faster
  than 2 m/s (in the world's time) it's thrown at twice that speed. A weapon made to fly point
  first (`MW_FLIES`: a dagger within 145° of its
  flight, a spear within 60°) is turned along its flight by 3 × its speed × the step of what's
  left, until it first touches something; the rest tumble. In flight it's judged from its sweep
  before the physics stepped (at the speed it met what it hit) with the thrown multipliers, and a
  thrown point goes in all the way.
* **Telekinesis** (`rig/telekinesis.rs`): in place of the gravity gloves when it's on. An
  open empty hand raised to point picks out a loose thing (its ray turned towards the palm and
  then the head's line through the hand, out to 6 m, a metre about it, within 40° of the gaze); the
  grip catches it onto a soft spring (100 N/m, 10, at most 1000 N; turning 10, 1, 1000), weightless
  and dragged, held where the hand points as far off as it was; the trigger pulls it in and the
  bottom button pushes it away at up to 8 m/s, out to 12 m; at the hand it's taken where it
  touches; let go of with the hand moving it's thrown at 20 m/s, a weapon point first; a weapon
  stuck in something is pulled out (recall).
* **Slowed time** (`powers.rs`): the off hand's stick click slows the game's time to half on a
  focus meter (it needs more than 10, drains 6 a second, comes back at 3, half while walking),
  easing out over 0.7 s. The hands keep the player's real speed, so in it they swing twice as fast
  and hit twice as hard. The runtime's world steps at the scale and the game is told to.
* **Imbuing** (`powers.rs`): a hand charging a fire or shock spell held within 0.2 m of a held
  weapon's blade fills it (100 a second at full charge for a tier 1 to 3 blade), and it drains. Its
  hits carry the spell (`CEF_IMBUED`): in the range a fire-filled blade heats what it hits (6 ×
  the hit's intensity × the fill, 40 a second stuck in) until it catches fire at 100 and burns; a
  shock over 4 m/s does 2 and may stun (always when stuck in, or in metal).
* **Breakable things** (`rig/breaking.rs`): a thing marked `OBJF_BREAKABLE` breaks when
  something meets it hard enough (the hitter's mass weighed clamp(0.5 ln m, 1, 5) times the speed
  they meet at, squared, 20 or more); the runtime takes it out of its world
  (`OS_BROKEN`), gives the hitter its speed back so a blade goes on through, and tells the game
  (`CE_BROKE`), which makes the pieces and puts it back. Weapons don't break, unless a game makes
  one break (`MW_BREAKABLE`).
* **Climbing** (`rig/climbing.rs`): an empty hand gripping the static world where there's an edge
  to close on holds there, and the player's body hangs from it on the player's joint (5000 N/m,
  300, at most 2000 N a hand, a 70 kg body with its weight), so pulling the controllers down lifts
  it; past an arm's length the hand lets go. The runtime tells the game how the arms move the player
  (`RtMelee::climb`, `RTM_CLIMBING`) and the game moves them.
* **Stamina** (`powers.rs`; an option, off by default): swinging hard costs it (the
  setting's seconds of an arming sword swung hard at 8 m/s empty it, a heavier weapon or faster
  swing sooner), and it comes back once the player has rested a second; a tired player's hands are
  weaker on their weapons (down to half). A game that keeps its own (Oblivion's Fatigue) gives it
  in `MeleeRules` and is told what the swings spent.

## Settings and the game's say

`settings.toml`'s `[melee]`: `strength` (1: the standard player), `stamina` (off), `stamina_time` (12 s),
`stamina_refill` (6 s), `slow_time` (on), `telekinesis` (off), `climbing` (on). A game's
`MeleeRules` override slowing time, climbing and telekinesis when they say (`RULES_SAYS_*`), and
its stamina replaces the runtime's.

## The calls

The choices that need a reason, and why:

| What | Usual | Ours | Why |
|---|---|---|---|
| Taking a weapon | pulls it into a posed grip on the handle | taken where it is; the handle only shapes the fingers | the user's rule: grabs never snap |
| The hold's solve | a configurable joint | the same spring, damper and caps, solved implicitly per step | stable at any weight in Rapier |
| Sliding's damper | against the hand's slide | against the slide relative to the controller's own motion | a steady swing doesn't slide the hand off |
| Creature pins | joint drives | implicit drives on each part's mass and inertia | explicit drives on light hand parts blew up |
| Knockouts | the tables carry knockout speeds nothing uses | a blunt head hit past them knocks out for 6 s | the user asked for knockouts; the speeds are in the tables |
| Choking | a hand choke hurts nothing | 5 a second | a choke can kill |
| Wrenching a weapon away | NPCs never let go to a pull | pulled 0.95 m from its holder's chest for 0.25 s, it lets go | the user asked for grab-and-wrench disarms |
| Feints | fighters never feint | Medium one swing in ten, Hard one in five | the user asked for feints |
| Focus | a most of 60 | 30 | reads as a slip against its other numbers |
| Stamina | none | the option above | the user asked for one, game-drivable |
| Burning to death | at 85% of a fire-death animation's clip | 4 s alight | we have no animation |
| Bone | a material of effects only | cut and stabbed like wood | it needs a row in the tables |
| A thrown point | its going-in damper is 0 for 0.5–1 s | goes in to its full depth at once | the same end, without timing the damper |
| Breaking on the world | left to the hitter's rigid body | the thing's own mass is weighed | a world has no mass |
| Telekinesis | always there | off by default, on when the game says or the settings ask | the gloves stay what a game gets unless it asks |
| A hand by a held blade | the spell hand charges whatever it's near | the spell hand may charge beside the other hand's weapon, and while the gloves only mark something far off | so imbuing works, and casting isn't stolen by a highlight |
| Climbing a top | the thumb-to-fingers cast must find an edge | an edge, or a ledge's top pressed on | a palm pushing down on a top holds as on its edge |
| Climbing's body | the player's physical body | an integrated 70 kg body; the game's measured movement is taken only where it held the body back | the game's frames aren't the runtime's |
| Stepping up | none (a climber jumps) | the range steps a climber let go within a metre below the block's top onto it | the range has no climb jump |
| Spells in the range | chosen from a wheel | fire and shock alternate as they're cast | the range has no spell wheel |

## Not done

* Kicks (no tracked feet), an extra push on downed bodies, a karate chop
  and special moves (punch spells, a telekinetic spin, chain lightning).
* Telekinesis on creatures (lifting a body by a part); it takes loose things only.
* The climb jump and feet placement (cosmetic IK).
* A fighter's blade sometimes reads very fast right after a parry (its pose snapping back); the
  player's damage from creatures is clamped to 5 to 15 anyway.

## Testing

Melee is checked by doing it, with the emulated headset (`docs/XRSIM.md`): scripts that take a
weapon from a table and swing, stab, throw, block, climb, and captures and logs to see what
happened. `RUSTVR_DEBUG_MELEE=1` logs how far held weapons lag their hands, `RUSTVR_DEBUG_COMBAT=1`
each contact's judging, `RUSTVR_SILENT=1` keeps the runtime's sounds off the speakers.
`PLAYTEST.md` (at the adapter's root) says how to start each area and what to try.
