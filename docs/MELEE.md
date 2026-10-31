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
  its damage curve, which of the damage tables it's judged by, its penetration depth and
  dampers, its tier). The runtime holds and judges them; a weapon the game gives none for doesn't
  hit (the older, simpler `MeleeTable` is no longer read). Ready-made damagers and handles, as
  items carry them, are `rustvr_melee::weapons` (the range's weapons, and a game's own weapons by
  their kinds: Oblivion's).
* The game lists what its creatures hold (`ActorItems`): the runtime's hands and weapons meet
  those things as solid, block and parry them, and are struck by them.
* The game says its rules for the player (`MeleeRules`): its strength, its stamina (when the game
  keeps one: Oblivion's Fatigue), whether the player may slow time, climb or use telekinesis, and
  whether the melee buttons are used.
* The runtime tells the game, every frame (`RtMelee`): how fast the game's time should run, how
  the arms are moving the player while climbing, a jump off a climb, what the swings spent of the
  stamina, which weapons are filled with a spell (held, and thrown, stuck or dropped).
* Hits and the rest are events (`CombatEvent`): `CE_HIT` (a judged hit: its damage, damage type,
  push level, depth, the blade, whether it severs, whether it was thrown, imbued, two-handed),
  `CE_WOUND` (a blade gone in deep, or out the far side), `CE_PULLED_OUT`, `CE_TOUCH` (a contact
  that did nothing: a scratch, a scuff), `CE_BLOCK` (a weapon met a creature's, a parry if it was
  moving into it), `CE_STRUCK` (a creature's swing hit the player), `CE_BROKE` (a breakable thing
  broke). A creature's part held by telekinesis is a creature grab (`GRAB_TK`), and a hold the
  creature broke free of is the game's word back (`ActorBody::let_go`). The game decides what each
  does to its creatures.

The test range is the game that plays it all: `crates/range` (its dummies, its fighters, their
brains, its weapons and its areas), and `crates/melee` (`rustvr-melee`) holds the tables, curves
and body rules, shared by the runtime and the range.

## Holding

* **Weight** (`rig/melee.rs`, `rig/physics.rs`'s `Joint`). A weapon hangs on its hand's joint: a
  spring of 5000 N/m and a damper of 100 from the controller to the point held, at most
  3000 N; turning, 1000 N m a radian and a damper of 50 times the handle's multipliers, at most
  250 N m. It keeps its mass and its weight, and the joint pulls at the grip: the weight at the
  centre of mass, and the grip's own acceleration, turn it about the grip, so a heavy head sags (the
  8 kg maul 2° at rest), trails a fast swing (60°) and swings on past a stop, while a dagger keeps
  up. The joint is solved implicitly each step (the velocity that lands the weapon where the spring
  and damper would put it at the end of the step), so it's stable for any weight, its turning
  against the weapon's inertia about the grip each way round (a long weapon twists about its own
  handle far more easily than it swings round: taken as one number, the weight swinging an 8 kg
  maul spun it about its haft at 50 rad/s, off the hand). The hand is drawn
  on the weapon, held back with it; an empty hand follows its controller exactly. Strength scales
  the springs and caps, the dampers by half of it past 1.
* **Handles** (`MeleeHandle::kind`). A light grip (one-handed weapons) turns it about the handle
  and across it apart, each to its cap; a heavy one (long and two-handed weapons) under one cap; a
  super heavy one x0.6 on everything; a supporting hand's (a pommel, a ricasso) never takes the
  turning; a half-sword grip on the blade turns less and damps more; a shield's and a throwing
  knife's their own. Off its handles a hand only grips the weapon (the grip multipliers: x0.5
  linear, x0.1 turning, at most 2000 N and 100 N m).
* **Grabbing** (`take_melee`). A hand that
  takes the weapon within 7 cm of one of its handles' lines (where the hand is, against the weapon)
  goes onto that handle (a first hand onto the main one whenever it's within reach, so a sword
  taken by the end of its grip is held by its grip, not its pommel): the hold on its line nearest the hand, turned as the handle's made to be
  held (`MeleeHandle::rot`, the right hand's; the left holds it the same, the fingers round the same
  side, and a shield's turned half round the fist's axis so its face is out from the back of either
  hand), or the other way round it (a reversed grip) when the handle's held either way and that's
  the nearer turn to the hand's. The hand goes onto the handle there and the weapon, left where it
  lay, eases into the hand on its joint. A second hand goes onto the handle it takes the same way.
  Drawn from a holster or caught from the gloves, it goes in by its main handle wherever the hand
  is, its own way round. Anywhere off its handles it's taken
  where it is, neither moving, the fingers round the weapon itself.
* **Two hands** on its handles are the two-handed hold: each hand's linear drive doubled (10000
  N/m, at most 6000 N; damped x1.5 when both are on one handle), the turning on one hand only (the
  dominant side's, unless that hand is on a supporting handle) at twice its one-hand value (a fifth,
  hand over hand on one handle with the grips facing opposite ways). The other hand's pull at its
  distance d also turns the weapon (its spring k d², its force F d): leverage, so two hands swing a
  heavy weapon far better than one, and the weapon leans towards that hand by its share of the
  turning.
* **Sliding.** A hand on a handle that slides (a spear's shaft, a staff, a mace's haft, a sword's
  grip) slides along it while its trigger is held, against a damper (300 at a light pull down to
  20 at a full one). At a handle's end it goes on to the handle that one names
  (`MeleeHandle::links`: a sword's grip on over its guard onto the blade,
  to half-sword; the blade back down), crossing between them as the weapon slides through the fist,
  or it stops there. One hand slides along a handle at a time; with both hands on the weapon, the
  other holds it, so nothing falls through them.
* **Stuck.** A weapon stopped by something, or stuck in it, with its grip further from the middle of
  the player's shoulders than they reach (half their height x1.2), is let go of (the stuck rule); 6 m
  off, anything held is.
* A released weapon goes through the hand that let it go until a quarter second has passed and its
  grip is 0.13 m clear of the palms. Weapons have air drag (1; a dagger's, a spear's and a
  shield's 0.4; turning 0.05).

## Hitting

* **Judging a hit** (`crates/runtime/src/combat.rs`, `rustvr_melee::judge`). Each damager is swept
  from where it was to where it is (for a held weapon, where the hand would have taken it this
  step, so a blade stopped by a body is still judged at the speed it met it), its points 6 to 8 cm
  apart, against each creature part's capsule; the first touch is found by a binary search back
  along the sweep, and the contact is judged by the impact velocity's angles against the
  damager's frame (an edge's 35°/90°/60°, an axe's 45°/80°/80°, a point's 35°/35°/60°, the normal
  either way round; a blunt damager and a fist hit from any way), its damage curve, the damage
  table for the damage type against the part's material and the weapon's tier, a bad angle falling
  back to the blade's own blunt blow (a sword's light 0.5, a dagger's very light 0.5, an axe's
  blunt 2). A weapon's points are tried before its edges before its blunt parts. A
  contact belongs to one body part and one of the weapon's collider groups (its edges and point;
  each blunt part), and is one hit until it's been lost 5 frames; a damager doesn't hit the part it
  last hit again within 0.1 s; each fist is its own.
* A contact that does nothing (too slow for what it met, or at a bad angle with no blunt to fall back
  on) is heard, softly, and on armour leaves a scratch or a scuff (`CE_TOUCH`).
* A blow on an arm or a hand that went through the creature's shield on its way there is the
  shield's.
* **Damage types against materials**: the damage tables (slash, heavy slash, weak slash,
  axe, pierce, fine pierce, blunt, light blunt, very light blunt, punch, kick, energy, a body
  slamming, a head slamming) against flesh, leather, chainmail and plate, by tier: the minimum
  speed, the damage multiplier, the speed it goes in at, how deep, its damper, its draw cuts' and
  pressure stabs' multiplier, the push levels, and the knockout speeds; a fire-filled blade's rows
  first. The world's stone and metal are plate to an edge or a point and nothing to a blunt blow.
* **Stabs that stick** (`rig/embed.rs`). A blade that went in stays in, jointed to the part it's in
  (turning with it): as deep as its momentum and then the hand push it against the dampers (a
  hand's going in, coming out; with none, its own weight against its free damper), up to the
  damager's depth (an arming sword's point 0.64 m, its edge 1 cm). A point pressed on
  a body goes in by pressure, and an edge pressed on and drawn along its length goes in too (a draw
  cut: the pressure curves, the faster drawn the less push). A cut slides along its edge 3 mm deep
  and comes out off the end of its edge; a back-edge cut goes in the way it cut. While it's shallow
  the blade rocks on where it went in, up to 30°, stiffening to rigid at its short depth (a sword's
  0.1 m, a dagger's 3 cm); it can't turn about its way in, so twisting it turns the part, and what
  it can't follow pulls the part with the hand's own force. It's never torn out sideways: pulled
  back past where it went in it's out. In past the part's deep depth (a head's 4 cm, a throat's 2,
  a torso's 10) it deals the deep damage times the part's multiplier (a cut's as soon as it's
  in); a point out the far side, its skewer damage. One blade can be in two bodies at once, run
  through one into the next, each resisting and hurt by it. Held, it buzzes 0.9 each centimetre it
  moves in or out and 0.8 each two it slides.
* **Dismemberment.** An edge (or a point that cuts limbs off) that went in faster than its
  dismemberment speed times its factor at the weapon's tier, square enough to the limb (each
  damager's own angles), takes off the part whose cut is within 4 cm of the blow:
  the part hit, else its parent, else one of its children (`rustvr_melee::sever_part`, by the
  human body's cuts: an upper arm cut at the shoulder leaning 0.35 to the torso's way, a forearm, a
  hand and a shin at their joints, a foot 3 cm up the shin, the head by its neck 2.8 cm up it; a
  thigh, the torso never). The event names the part taken off (`CEF_SEVER`); the game cuts it off,
  and it kills. The blade carries on through: it passes the creature until it's clear of it, and
  doesn't go in again for a second. With easy dismemberment (the `easy_dismemberment`
  setting, on unless it's turned off) neither the speed nor the place matters: a cutting edge that
  goes into a part that comes off takes it off (its parent and children still only at their
  joints), a stab never (see the calls).
* **Knockouts.** A blunt hit to the head past the knockout speed of its row knocks the creature out,
  as the tables say (see the calls).
* **Impact feedback.** A hit's sound is picked by what hit what (a blade, a mace or a fist against
  flesh, mail and plate, wood), as loud as it was hard (its intensity, 0.25 to 15 m/s, or the
  damager's own after a damaging blow: a mace's 6 to 16); metal sparks above an intensity of 0.5;
  the holding hands buzz a 0.3 s hit at the sound's volume. A held weapon rubbing along what it
  touches (a wall, a table, a body) buzzes them 0.8 each 2 cm the contact slides.
* **Blood.** A blade is bloodied off flesh, or where it went in: a stuck blade a mark each 6 cm it
  goes in. A neck a blade came out of bleeds; a cut-off piece leaves at half its speed plus 3 m/s
  along the limb (a head 3.5). The game draws its wounds (the range: reveal masks per armour,
  cuts, stabs, bruises fainter, burns, scratches and scuffs on armour, a skewer's exit wound).
* **The player struck**: a creature's blade is judged against each of the player's parts (0.1 s a
  part), by its weapon's tier, at its speed against the player's own; a blow the player's
  weapon or shield stopped doesn't go on into them; the game takes the head's x1.5 and the hands'
  x0.5, clamped to 5..15.

## Defence

* **Blocking** is physical: the player's weapon meeting a creature's attacking weapon stops it, and
  it's told to the game as `CE_BLOCK`. A blow the player's weapon or shield stopped doesn't go on
  into them.
* **Parrying**: a block with the weapon moving into the blow over 1 m/s inside the swing's damage
  window is a parry (`CEF_PARRY`); the game's brain rolls its reactions by the hand met: the swing
  turned back (played back 0.2 s), nothing, or a recoil; a shield never chains off one.
* **The creatures' guards**: a blocked blow from a heavy weapon pushes the guard by its level
  against plate (each weapon's block push: L2 and L3 staggers, L4 floors, flings and disarms); a
  half-raised guard recoils; a shield counters three times in ten.
* **Disarming**: a hit on a creature's hand at push level 2 (3 for the armoured), or a stab through
  its arm, makes it drop its weapon; an arm hit while the player holds its weapon too; a hand (or
  telekinesis) can take a creature's weapon and pull it away (see the calls).
* **Armour**: each part has its material (leather, chainmail, plate), and the tables judge against
  it: a sword's edge glances off plate; a mace still hurts through it.

## Enemies (the range)

* **Active ragdolls** (`crates/range/src/ragdoll.rs`): thirteen parts, pinned to an animated pose
  with pins (position 1000/50, at most 1000 N; rotation 800/50, at most 100 N m), the arms
  pinned harder while swinging (spring x2, damper x0, force x10) and guarding (x3, x1, x30). Its
  balance: standing, knocked down (its pins loosened to a destabilised x0.5 spring and x0.1
  damper, x0.1 on the ground), getting up once still under 1 m/s for 0.5 s (its pins ramping back
  over 3 s; any push puts it down again), or limp. It weighs 65 kg. Falls and slams hurt by the
  slamming tables (a head's by its own), from the contact's speed and the slamming part's armour.
  Each pin turns its part against the part's own inertia each way (an arm twists about its length
  far more easily than it swings), and each pull on a part (its pin, a hand's hold, a loose pin to
  its parent) is worked out from how the part will move after the pulls before it: one that took
  every part for as hard to twist as to swing, or two that each stopped a twist in one step, shook
  the arms at 94 rad/s standing still. Its physics steps once a frame, so it moves smoothly in
  slowed time too.
* **The range's guns, grenades, knives and spells** on them (see the calls, below): a round meets
  the first part or held thing on its line (a pistol's 15, a rifle's
  30 and through, a shotgun's 45, by the armour and the part's multiplier; a push level of 1, 2,
  3; through the arm, the weapon dropped); a grenade's blast hurts by the distance from its
  nearest part (a frag's 120 falling off over 6 m with the square, so about 2 m kills; floored
  within 4 m, the limbs within 1.2 m torn off, thrown out at 8 m/s at its centre; the fire bomb's
  2.5 m sets it alight), unless the room shields it; a throwing knife that sticks in one is judged
  as a thrown dagger, and rides with the part; a spell cast at one lands as a fire bolt's or a
  lightning bolt's hit.
* **Hit reactions by force**: each hit's push level (by the hit's speed against its row) picks
  a reaction from its brain's push table (Easy, Medium and Hard; the armoured MediumHeavy and
  HardHeavy a step tougher) for where it landed and what the creature was doing: a flinch, a
  stagger, a fall. Any stagger stops a swing. A body that's down is pushed by the blow (a punch's
  100 for 0.15 s, a kick's 400). Shocked, a blow pushes a level harder, any stagger floors it, and
  it can't get up.
* **Melee AI** (`brain.rs`): three stances (one sword, sword and shield, two swords), each
  with its openings, chains and ripostes (ranges, marks, clip lengths, root motion,
  difficulties, weights, skip chances, sides, sweeps; the dual-wield moves, the cross slash, the
  shield pushes, lunges and the combo), swung through each strike at speed. Its brain's numbers
  go by tier: attack delays, at most 1, 2 or 3 of them at the player at once
  with the rest at its safe distance, back dodges by a chance curve when its guard won't cover
  a blow (always from a thrust at a sword), danger rolled each brain cycle; feints (the user's call).
* **Fear and pain**: there's no fleeing by health: a fighter flees only unarmed with no weapon to
  take up (none within 4 m of the player), to the arena's far corners, and is gone; disarmed, it
  stoops to take a weapon back up. It grunts when hurt, shouts as it swings (seven times in ten),
  and says its last line by what killed it (silence after a stab through the throat, a grunt for a
  stab to the head, breathless for a cut throat or a choke, the long one for a stab elsewhere); it
  screams falling faster than 5 m/s until it lands, and cries out held (its grappled line, not
  for a wave's fighters); it blinks, and shuts its eyes out cold or dead. Each line is the
  creature's own voice (`EV_VOICE`'s speaker): a new one cuts off the last.
* **Knockouts**: out cold standing (its brain off, its weapons kept), limp only if already down or
  out, 6 s from the last blow.
* **Waves** (`arena.rs`): each comes while the player's in the arena; from the second, its
  shield-bearer is an armoured unit, in plate.

## Grappling and unarmed

* **Grabbing** any part of a creature (the framework's creature grabs) by its handles (neck,
  mouth, skull, torso, shoulder, wrist, hand, legs): held on the grip (5000 N/m, 50, at
  most 3000 N; turning 1000, 50, 250 N m), the held arm reaching after the grip, the head held
  tight on a neck or torso hold. Held standing, it's floored at once in the states its handle says
  (by the neck or torso idle, fully reeling or shocked; by a leg idle, attacking, reeling or
  shocked; by a wrist shocked); lifted 5 cm (a leg 10) it falls by a neck or a leg, else the hand's
  forced off; its head pressed below its hips, it falls. The throat chokes (5 a second; its weapons
  dropped); a hand over the mouth muffles it. Awake, it struggles: escape tries (10% at
  first, 5% more each, 10% less in two hands, at most 50%), and one that works forces the hand off
  (`ActorBody::let_go`). Let go of while moving fast, it's thrown down on its own momentum.
* **Tearing** (the range's tearing joints: the shoulders, the knees, the neck): two hands
  holding one body tear a part they hold, or its parent but the torso, past 3450 N; the part comes
  off, and it kills.
* **Hands hit**: every empty hand carries a punch damager over its palm and fingers, open
  or closed, judged by the same judge (the punch's curve, its table: 2 m/s on flesh at x0.5, any
  way, never going in). It's swept where the controller takes it, not where the drawn hand stops on
  what it hits. Shoves are the hands' own contact pushes.
* **Kicks** (`kick.rs`), untracked: with the melee buttons (`RULES_KICK`) the movement
  stick's click, walking under 3 m/s, kicks at what the eyes see within 2 m (else straight ahead),
  with the foot on its side: folded up over 0.2 s, driven out 1.2 legs' lengths from the hip over
  0.2 s, out 0.1 s unless it met something, back over 0.4 s. The body's legs show it, and the foot
  hits as a kick (its table; a body that's down shoved with 400).

## Extras

* **Throwing** (`rig/melee.rs`): a weapon leaves as it was moving; let go of with the hand faster
  than 2 m/s (in the world's time) it's thrown at twice that speed. A weapon made to fly point
  first (`MW_FLIES`: a dagger within 145° of its
  flight, a spear within 60°, a staff end first within 30°) is turned along its flight by its rate
  (3; a staff's 5) × its speed × the step, until it first touches something; the rest tumble. In
  flight it's judged from its sweep before the physics stepped (at the speed it met what it hit)
  with the thrown multipliers, and a thrown point goes in all the way.
* **Telekinesis** (`rig/telekinesis.rs`): in place of the gravity gloves when it's on. An
  open empty hand raised to point picks out a loose thing or a creature's part (its ray turned
  towards the palm and then the head's line through the hand, out to 6 m, a metre about it, within
  40° of the gaze; for 5 s after a hand held a thing every other thing ranks 10° worse); the grip
  catches it. A thing goes onto a soft spring (100 N/m, 10, at most 1000 N; turning 10, 1, 1000),
  weightless and dragged, held where the hand points as far off as it was; the trigger pulls it in
  and the bottom button pushes it away at up to 8 m/s, out to 12 m; at the hand it's taken where it
  touches; let go of with the hand moving it's thrown at 20 m/s, a weapon point first; a weapon
  stuck in something is pulled out (recall). A creature's part is held by the game on a ragdoll
  drive (`GRAB_TK`: 2000 N/m, 100, at most 2500 N; turning 10, 1, 1000 N m, the neck's x5), its
  point following the hand at up to 3 m/s (from nothing over 3 s; a dead one's at once), pulled and
  pushed at 3.2 m/s out to 6 m; let go of with the hand moving, the part flies at 30 m/s and the rest
  of the body at 8. It isn't struggled out of. Two telekinesis hands on one body tear its tearing
  joints past 3000 N; a body dead and 10 s alight is charred through: stiff as
  it lies, its tearing joints breaking at 3000 N.
* **Slowed time** (`powers.rs`): the off hand's stick click (with the melee buttons, the dominant
  hand's B) slows the game's time to half on a focus meter: it starts at 30 and holds 60, needs
  more than 10, drains 6 a second of real time while slowed and comes back at 3 a second of the
  game's always (half while walking): about 6.7 s from 30. It eases out over 0.7 s. The game may
  give its own scale, meter length and refill (`MeleeRules`). The hands keep the player's real
  speed, so in it they swing twice as fast and hit twice as hard. The runtime's world steps at the
  scale and the game is told to.
* **Imbuing** (`powers.rs`): a hand charging a fire or shock spell held within 0.2 m of a held
  weapon's blade fills it, by tier: 50 a second at full charge x2 for a tier 1 to 3 blade,
  x3 for tier 4 and 5, none for tier 0; it loses 2, 1, 0.45, 0.1 or 0.05 a second by tier (a
  shock x0.4), 4, 3, 2, 0.5 or 0.1 a hit, 5 a second lying dropped. Its hits carry the spell
  (`CEF_IMBUED`): in the range a fire-filled blade heats what it hits (6 × the hit's intensity × the
  fill, 40 a second stuck in from the moment it goes in) until it catches fire at 100 and burns; a
  shock over 4 m/s does 2 and may stun. A thrown or stuck weapon keeps its fill
  (`RtMelee::loose_imbues`).
* **Breakable things** (`rig/breaking.rs`): a thing marked `OBJF_BREAKABLE` breaks when
  something meets it hard enough (the hitter's mass weighed clamp(0.5 ln m, 1, 5) times the speed
  they meet at, squared, 20 or more); the runtime takes it out of its world
  (`OS_BROKEN`), gives the hitter its speed back so a blade goes on through, and tells the game
  (`CE_BROKE`), which makes the pieces and puts it back. Weapons don't break, unless a game makes
  one break (`MW_BREAKABLE`).
* **Climbing** (`rig/climbing.rs`): an empty hand gripping the static world where there's an edge
  to close on holds there, and so does a hand holding a weapon stuck fast in the world (an axe in a
  wall); the player's body hangs from it on the player's joint (5000 N/m, 300, at most 2000 N a
  hand, a 70 kg body with its weight), so pulling the controllers down lifts it. A hand lets go held
  beyond the player's reach from their shoulders (the stuck rule). With the melee buttons the
  dominant stick's click jumps off (the climb jump: a jump force of 0.3 a 90th of a second eased
  out over 0.6 s, up, and along the gaze x0.3; `RtMelee::jump`). The runtime tells the game how
  the arms move the player (`RtMelee::climb`, `RTM_CLIMBING`) and the game moves them.
* **Stamina** (`powers.rs`; an option, off by default): swinging hard costs it (the
  setting's seconds of an arming sword swung hard at 8 m/s empty it, a heavier weapon or faster
  swing sooner), and it comes back once the player has rested a second; a tired player's hands are
  weaker on their weapons (down to half). A game that keeps its own (Oblivion's Fatigue) gives it
  in `MeleeRules` and is told what the swings spent.

## Settings and the game's say

`settings.toml`'s `[melee]`: `strength` (1: the standard player), `stamina` (off), `stamina_time` (12 s),
`stamina_refill` (6 s), `slow_time` (on), `telekinesis` (off), `climbing` (on), `easy_dismemberment`
(on). A game's
`MeleeRules` override slowing time, climbing and telekinesis when they say (`RULES_SAYS_*`), turn on
the melee buttons (`RULES_KICK`: the movement stick kicks, slowing time on the dominant B, the climb
jump on the dominant stick), give their own slowing (`slow_scale`, `focus_time`, `focus_refill`),
and their stamina replaces the runtime's.

## The calls

The choices that need a reason, and why:

| What | Usual | Ours | Why |
|---|---|---|---|
| Taking a weapon | pulls it into a posed grip on the handle | the same: near a handle onto it, at its grip turn or reversed, eased into the hand; off its handles taken where it is | the user's rule: weapons snap to their rigged grips (it replaced "grabs never snap") |
| Drawn from a holster, or caught | into the hand by its grip | the same (into the grip) | a weapon in a holster or in the air has no "where the hand is on it" |
| Sliding over the guard | the hand jumps to the linked handle | it crosses the gap, the weapon sliding through the fist | a slide goes on as the hand moves, rather than jumping |
| The hold's solve | a configurable joint | the same spring, damper and caps, solved implicitly per step, with the weight's turn at the grip | stable at any weight in Rapier |
| Two hands | a joint at each hand | one joint at the holding hand: the other's drive at its distance as leverage, its pull leaning the target | one body, one joint |
| Sliding's damper | against the hand's slide | against the slide relative to the controller's own motion | a steady swing doesn't slide the hand off |
| Creature pins | joint drives | implicit drives on each part's mass and its inertia each way, each from how the part moves after the drives before it | explicit drives on light hand parts blew up; one inertia for every way, and drives each solved alone, shook the arms |
| The creatures' physics step | its fixed step, the headset's frame | one a frame (a frame longer than 1/72 s split evenly) | a fixed 120 Hz stepped twice some frames and none others at 90 Hz, and once in three frames in slowed time |
| Guns on creatures | none | a round: pistol 15, rifle 30 (and on through flesh and leather), shotgun 45; x0.9 through leather, x0.6 chainmail, x0.4 plate; push levels 1, 2, 3; the part sent on at 3 to 6 m/s; through an arm or a hand, its weapon dropped | the range's guns had no say on them; two rifle rounds or four pistol rounds kill, a head shot sooner |
| Grenades on creatures | none | a frag: 120 at the centre falling off with the square over 6 m, floored within 4 m, limbs within 1.2 m torn off, thrown at 8 m/s; the fire bomb: 15 over 2.5 m, alight close in; none through the room's walls and furniture | as above |
| Throwing knives in creatures | none (its daggers are its throwing knives) | a thrown dagger's point: its curve x3, then 7.5 times the part's deep multiplier gone in; the knife rides with the part | the range's knives are its own throwables, not melee weapons |
| Spells cast at creatures | a projectile at 12 m/s, guided | lands at once on the creature the runtime's aim found: the fire bolt's 10, its push at 12 m/s and its heat; the lightning's 2 at push level 1 and its electrocution | the range has no projectile of its own |
| A stuck blade's pull on the body | the joint's force on the part's rigidbody | the hand's force as the speed it gives the part's mass in a step, for the game to add (at most 4 m/s) | the protocol pushes by speed |
| A blade's way between its samples | its colliders, the whole blade | points 6 to 8 cm apart along it | a sweep of points, as close as a hand's thickness |
| Dismemberment | strict: 12 m/s for a sword, within 4 cm of a joint, square to it (easy dismemberment an option, off by default) | easy dismemberment on by default, for cutting edges only: a stab still sticks | the strict cut proved too hard to land in play, and the user wants limbs coming off; easy dismemberment for stabs too would let a stab into an arm or a face cut it off |
| Knockouts | the tables carry knockout speeds nothing uses | a blunt head hit past them knocks out | the user asked for knockouts; the speeds are in the tables |
| Choking | a hand choke hurts nothing | 5 a second | a choke can kill |
| Wrenching a weapon away | NPCs never let go to a pull | pulled 0.95 m from its holder's chest for 0.25 s, it lets go | the user asked for grab-and-wrench disarms |
| Feints | fighters never feint | Medium one swing in ten, Hard one in five | the user asked for feints |
| Stamina | none | the option above | the user asked for one, game-drivable |
| Burning to death | at 85% of a fire-death animation's clip | 4 s alight | we have no animation |
| Bone | a material of effects only | cut and stabbed like wood, struck like plate | it needs a row in the tables |
| A thrown point | its going-in damper is 0 (50 for sword stabs) for 0.5–1 s, then locked going out | goes in to its full depth at once | the same end, without timing the damper |
| Breaking on the world | left to the hitter's rigid body | the thing's own mass is weighed | a world has no mass |
| Telekinesis | always there | off by default, on when the game says or the settings ask; the range's melee areas keep the gravity gloves | the user wants the gloves' pull, not telekinesis |
| A telekinesis throw | a skill | always | the range has no skill tree; it was asked for |
| Telekinesis on bodies | a skill (or a gravity spell's creature control) | always, with telekinesis | the same |
| Tearing limbs off | skills (two telekinesis hands on a body, both hands on one); a body burned to a full char | all three, always | the same; it was asked for |
| A hand by a held blade | the spell hand charges whatever it's near | the spell hand may charge beside the other hand's weapon, and while the gloves only mark something far off | so imbuing works, and casting isn't stolen by a highlight |
| Climbing a top | the thumb-to-fingers cast must find an edge | an edge, or a ledge's top pressed on | a palm pushing down on a top holds as on its edge |
| Climbing's body | the player's physical body | an integrated 70 kg body; the game's measured movement is taken only where it held the body back | the game's frames aren't the runtime's |
| Stepping up | none (a climber jumps) | the range steps a climber let go within a metre below the block's top onto it, besides the climb jump | its block has no foothold to jump from |
| Buttons | its own (kick on the movement stick, slow time on B, jump on the other stick) | the framework's, unless the game asks for the melee buttons (`RULES_KICK`); the range's melee areas do | a game's own buttons come first |
| Spells in the range | chosen from a wheel | fire and shock alternate as they're cast | the range has no spell wheel |
| Blood | its reveal masks, sparse drops and decals that fade in 30 s; only stumps and a neck bleed | the runtime's gore (its own design: wounds bleed a few seconds, blood runs and pools) with blade blood by depth and a bleeding neck | the gore is the framework's, made for every game |
| The spear's points | two side points | its leaf point and edge only | the range's spear has no side points to put them on |
| Telekinesis on a fighter's weapon | its handles stay telekinesis's while an NPC holds them | the same: pulled away past 0.95 m for 0.25 s, the fighter lets go | the range's wrench rule, as for a hand |
| The tearing force | the joint's force, as the engine reports it | each joint's last substep impulse over that substep | Rapier keeps no force; a steady pull reads the same |
| The grip on a creature | the player's 3000 N | the runtime's setting (1500 N) times the game's strength: 2 in the range's melee areas | the setting is every game's |
| Waves | waves start when the player picks one | each comes while the player's in the arena | one room holds every area |
| Moves' shapes | animation clips | keyframed shapes (swings from each side, above, low, a thrust, the combo, a shield push) with each move's marks laid on, through each key at speed | the range has no animation clips |
| Chains' and ripostes' clips | animation clips | 1.33 s, leaning in 0.22 m as its plain swings do | one length for all of them |
| Root motion | each clip's root curve | each opening's reduced to its furthest, when, and where it ends, eased between | close enough to its curves |
| Idles' guards, dodges, picking up, blinking | its clips | ours (guards up and forward; a metre back over 0.6 s; a stoop over 0.75 s, the weapon taken at 1.05 s within 0.45 m; eyes shut for the middle half of 0.5 s) | the range has no animation clips |
| Spacing | moving to its idles' ranges | hovering just past the player's reach, stepping in when its delay is up; not engaged, at its safe distance + 0.25 to + 2.25 | the idles' ranges, without a navmesh |
| Spreading out | a spot clear of its allies | it strafes away from the nearest ally in its reach | no navmesh spots |
| A covered blow | a capsule cast along the blow | the blow's tip's way against its blade, within 6 cm, before the body | the same test without a cast |
| Where it flees | its level's exits | the arena's two far corners | the range has no exits |
| Its voice | a clip each line, by its own pitch | the runtime's own sound for each line, one at a time a creature | the range has no voice clips |
| A blow's shove | a frame's acceleration landing in one physics step | taken at 90 Hz, as a speed change spread over its duration (mass-free; an impulse in its impulse mode) | frame-rate free |
| A shove from thrown weapons, and in slowed time | per damager's state; x a slow-motion multiplier | never from a thrown weapon; not scaled | the protocol doesn't carry them |
| Slams | the engine's collision speed; the part's tier and the creature's multiplier | the two bodies' speeds at a new contact, from before the step; 1 and 1 | the solver has changed them after |
| Grappled turning, stepping, height | its turn speed, feet at 3x its forward speed, its animator's height | 6 a second; our walk's 1.4 m/s x 3; the pelvis squats by the hand's height, down to half | no animator |
| A held limb's IK | the hand and wrist 1, the shoulder 0.25, the trunk 0.25 | the arm's wrist lerped to the grip by those; a leg pointed at it; no trunk | our two-bone arm IK |
| The neck held tight | swing 20, twist 20 / 45 on the head and neck | the head's nod ±20, turn -20 / 45 | one joint for head and neck |
| Escaping a hold | an escape clip, then the hand forced off after the handle's delay; per handle | a 0.6 s twist, the next try 1 s + 2 to 3 s later, the hand let go 0.5 s after one that works; every handle | the range has no escape clip |
| The armoured fighters | armoured enemies in plate, Heavy brains | from wave 2, each wave's shield-bearer, in plate | so the Heavy tables are played |
| Block push per weapon | each weapon's own | a table by the range's weapons (longsword, mace, staff, maul) | the protocol doesn't carry it |

## Not done

* Spells as spells: a fire bolt's flight (a cast lands at once on the creature it's aimed at),
  flame and lightning sprays, and casts from a gripping hand.
* Special moves: a karate chop, punch spells, a telekinetic spin, a stronger kick, a flying kick,
  chain lightning, a deeper slowed time.
* Feet placed on walls while climbing (looks only).
* Rubbing's sound and sparks: a hit's effect playing on while a blade is drawn along something (the
  runtime's loops have no scrape of their own yet; the buzz is done).
* A hit shaking a climbing grip loose (dagger points, rocks, spells, arrows; the range's fighters
  carry none of those).
* A breakable's pieces keeping its own speed (the range's debris flies from the blow).
* Telekinesis choking a body.

## Testing

Melee is checked by doing it, with the emulated headset (`docs/XRSIM.md`): scripts that take a
weapon from a table and swing, stab, throw, block, climb, and captures and logs to see what
happened. `RUSTVR_DEBUG_MELEE=1` logs how far held weapons lag their hands, `RUSTVR_DEBUG_COMBAT=1`
each contact's judging, `RUSTVR_DEBUG_BRAIN=1` (the range) each fighter's state once a second,
`RUSTVR_SILENT=1` keeps the runtime's sounds off the speakers.
`PLAYTEST.md` (at the adapter's root) says how to start each area and what to try.
