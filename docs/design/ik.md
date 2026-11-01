# The player's own body in first person: what makes it read as real

Research for the body solver (`crates/ik`), October 2026: what shipped VR bodies do and what
players say is wrong with them, the 3-point body-estimation research, the biomechanics of the
joints the headset doesn't track, and what real people's motion capture says when our solver is
measured against it. The longer notes this is drawn from (with every source) are outside the
repository, beside the evaluation tool (`landscape.md`, `papers.md`, `biomechanics.md`); the
earlier `docs/design/vr-player-controller/full_body_ik.md`
covers VRIK, FRIK, Parger and VRArmIK in detail and isn't repeated.

Marks: [est.] a value no source gives; [inference] a conclusion of ours.

## 1. What makes a VR body look like a mannequin

Players' complaints about first-person bodies (about 200 reports across Reddit, Steam, the VRChat
feedback board and the Unity, Meta and Godot forums, counted roughly), in order:

| | Complaint | ~Reports |
|---|---|---|
| 1 | Arms the wrong length: always bent ("T-rex arms") or rubbery, the hand coming off at the end of a reach | 45 |
| 2 | Elbows: out at the sides ("chicken dance"), hanging, not following the wrist's roll, flipping | 25 |
| 3 | The body in the wrong place crouching or bending: moving back, the neck stretching, kneeling when only looking down | 22 |
| 4 | Seeing inside yourself: the view sinking into the neck or chest (VRChat's most-voted item, 632 votes) | 20 |
| 5 | The torso turning when only the head turns | 17 |
| 6 | Knees always bent, legs shifting as the head turns, feet sliding | 15 |
| 7 | Shoulders: no clavicle motion, never dropping; arms seeming to come from the head | 12 |
| 8 | Hands off the controller or the held thing | 10 |
| 9 | Wrists twisting like candy wrappers | 8 |
| 10 | Lag, jitter, rubberiness | 7 |

Two causes cover most of the top five [inference, agreeing with the developers' own diagnoses]:
the body's proportions against the eyes (where the shoulders and the chest are from the eyes, the
arms' length, the torso's depth) and how the torso's facing is estimated. Perception studies add:

- Arm length matters most at full extension: users reject an avatar whose arm is still bent when
  their own elbow has locked (Porssut et al. 2022, N = 25); stretching the arm so the hand always
  meets the controller raised embodiment, up to 26 cm on average (Ponton et al., CHI 2024).
- Noticeability of a shoulder or elbow offset grows about quadratically, most with the arm raised
  forward (Li et al. 2022). A few degrees of noise in the proximal joints are invisible; lag is not
  (Toothman & Neff 2019; ownership falls from about 100 ms, Caserman 2019).
- Self-contact keeps embodiment through distortion (Bovet 2018); people are most sensitive to a
  hand offset towards their own body (7.8 cm, Benda 2020).
- Head-and-hands VRIK legs measured no better than no avatar, blamed on auto-steps with the wrong
  foot (Eubanks et al. 2020). Meta shows no first-person legs at all.

## 2. Shipped bodies

- **Final IK's VRIK** (Unity; Dead and Buried's): spine, locomotion, legs, arms in that order;
  torso yaw from the hands with the head as a clamp; procedural steps; the elbow's bend normal a
  weighted guess ("never conclusive"). Most Unity VR bodies since 2017 are tuned VRIKs, some with
  VRIK driving a sprung ragdoll.
- **Skyrim VR's VRIK** and **Fallout 4 VR's FRIK**: VRIK's arm and leg solvers ported, Parger's
  torso yaw with FRIK's weights, a clavicle that reaches 15–22% of the arm past half extension,
  10° minimum elbow bend. FRIK (open source) is the reference our solver started from.
- **Lone Echo** (GDC 2017 notes): only the elbow's swivel estimated; the clavicle always aimed at
  the hand with extension from a curve; chest facing a blend of the head's look and the
  head-to-hand directions, the hands' weights falling near the chest and to zero behind it; arm
  length adjusted live, hands always 1:1; a momentum-driven spine plus an animation layer.
- **Boneworks, Bonelab, Marrow** (Stress Level Zero): a physics rig (6, 11, then 18 bodies) with an
  anatomical visual rig on top: the only shipped body with sternoclavicular, acromioclavicular and
  glenohumeral landmarks and a modelled torso depth; clavicle reach curves.
- **Half-Life: Alyx**: hands only, by playtesting: correct arms go unnoticed, wrong ones stand out.
  Invisible arms are still simulated for physics checks.
- **VRChat IK 2.0**: lock modes trading head against hip accuracy; its elbow-clipping avoidance
  produced the "chicken dance" and cut the wrist roll's effect on the elbow that players expected.
- **Meta's Movement SDK**: on Quest 3, inside-out body tracking (IOBT) camera-tracks the upper body
  (torso, elbows, wrists); Generative Legs infers legs from the upper body on any Quest. Joint sets:
  `XR_FB_body_tracking`'s 70 joints, `XR_META_body_tracking_full_body`'s 84, with a fidelity and a
  calibration extension. Meta's own first-person tricks: hand IK weighted by closeness to the gaze
  ray, at most 5 cm of hand correction, proportions clamped to 0.8–1.2, a twist processor against
  candy-wrapping. **Virtual Desktop forwards all 84 joints to PC OpenXR apps**, with a fidelity flag
  and one confidence for the whole body; its tracker emulation maps the shoulder to the scapula.
  Never take the hands from it: they lag the controllers.

What none of them do [inference]: fit the person's own geometry from their motion (neck pivot,
shoulder centres, reach, torso depth; only research does, Ponton's Fitted Avatars); move the
shoulder girdle through the whole range of arm motion by its biomechanics; keep the chest still
while only the head turns, as motion capture shows people do; read the Quest's tracked body on PC;
feed a learned swivel into an analytic solve (MANIKIN's split, below) in a shipped game.

## 3. The research on bodies from three points

- **Learned 3-point methods** (AvatarPoser, AGRoL, BoDiffusion, AvatarJLM, HMD-NeMo, HMD-Poser,
  SAGE, MANIKIN, EgoPoser, DivaTrack, RPM, all trained on AMASS): upper-body position error
  1.3–2.0 cm on synthetic AMASS input, 4.4–5.8 cm on real headset data; the lower body is 3–5×
  worse (16–21 cm on real data). Feet slide in every kinematic method. Physics trackers (QuestSim,
  QuestEnvSim, Neural3Points) remove sliding at the price of latency, falls and drift.
- **MANIKIN** (ECCV 2024) is the strongest evidence for our solver's shape: a network predicting
  only the torso, the feet and one swivel angle per limb, with the limbs then solved in closed form
  (hands exact), beat optimisation IK (4.18 cm), a network predicting every rotation (4.41 cm) and
  a geometric swivel search without a motion prior (5.51 cm), at 3.02 cm.
- Derive the pelvis from the head down the spine; regressing it directly is worse (AvatarPoser).
- Inputs in a frame on the floor under the head, yawed with it, beat absolute ones by 23%
  (EgoAllo). About 0.3–0.7 s of history buys most of the accuracy (AvatarJLM); jitter comes from
  estimating each frame independently (AGRoL online: 12× the jitter of offline).
- Knowing the user's bone lengths is worth as much as the model (HMD-Poser 5.1 -> 3.2 cm; EgoPoser
  6.4 -> 5.3 cm with a T-pose calibration).
- **Size**: the smallest useful published networks are about 4 M parameters (0.33 GFLOP); the only
  CPU timing is 4.4 ms (HMD-NeMo, 5.3 M). A purpose-built head of 0.1–0.3 M parameters predicting
  only the chest and swivels should cost tens of microseconds [est.]: ours (30-128-128-10, 22 k
  parameters) takes about 5 us of a 12 us solve (one core, unvectorised).
- **No published weights can ship**: all are trained on AMASS, whose licence bans training for
  commercial use (and SMPL's is non-commercial).
- **Elbow swivel outside VR**: Kim & Rosen 2012's head-point rule (the elbow in the plane of
  shoulder, wrist and head, away from the head) measured 1.4–6° on slow table reaches; Soechting &
  Flanders 1989's linear arm-posture model; Gielen 1997: humeral torsion is fixed by the arm's
  direction to 3–4° (a deterministic elbow rule is justified). Against motion capture the head-point
  rule did worse than our Parger-plus-costs search at every weight (§6).

## 4. Biomechanics, with the numbers used

**Shoulder girdle (scapulohumeral rhythm).** The glenohumeral (GH) centre doesn't move below 30° of
arm elevation, then rises and moves in and back. Klopčar & Lenarčič 2006: girdle elevation
0.36φ − 10.8° past 30°, protraction −0.22φ + 15.4° past 70° (so retraction); reaching behind,
elevation 0.3° and retraction 0.35° per degree of extension. Zhang et al. 2020 measured the GH
centre 2.7 cm medial, 1.9 cm back, 4.8 cm up at 90° and 5, 3.3, 7.8 cm at 150° (skin markers).
Ludewig 2009 (bone pins): the clavicle rises less than skin markers suggest (6° by 120°), retracts
16° and rotates back 31°; 7.5° more retraction raising the arm sideways than in the scapular plane,
as much less raising it forward. Arm-to-scapula ratio 2.1–2.4 to 1.

**Elbow and forearm.** Everyday tasks fit within 60° wrist extension, 54° flexion, 40° ulnar and
17° radial deviation (Ryu 1991), and forearm rotation of 50° each way (Morrey 1981); relaxed:
wrist 15° extended, 5° ulnar, forearm near neutral. Pronation spins the radius about the ulna, so
the skin's twist is near zero at the elbow and the full angle at the wrist, close to linear between
(a twist bone at a fraction f of the forearm takes f of the roll). With the elbow bent, hand roll
about the forearm can only come from the forearm.

**Head, neck and trunk.** Gaze shifts under about 20° need almost no head motion. Turning in place
the latencies run eye 0.5 s, head 0.6 s, trunk 0.7 s, feet 1.1 s (Anastasopoulos 2009); the head
leads the trunk by 0.2–0.6 s walking, by up to 19° (Sreenivasa 2008). Land 2004: the head's path
in space is unchanged when the trunk joins: the neck is the head's turn less the trunk's, and the
trunk a slower process of its own. Cervical rotation about ±70–80°, flexion 50–65°; looking at a
phone, 33–45° of head flexion.

**Trunk in reaching.** Almost no trunk use until the target is beyond arm length (armpit to wrist
crease); then it starts with or before the hand and finishes about 200 ms after (Kaminski 1995,
Levin 2002, Ma & Feldman 1995). People switch to leaning a little before the absolute limit.

**Stance and stepping.** Heel centres 0.17 m apart, feet open 14° (McIlroy & Maki 1997). Stability
needs the extrapolated centre of mass, x + v·√(l/g), inside the base of support (Hof 2005); people
step well before the limit. Turning: the head, then the trunk, then the feet.

**Effort.** Shoulder gravity torque about 12 N·m with a straight horizontal arm, 3 N·m with the
elbow tucked (80 kg person); endurance 14.86·f^−1.83 s at a fraction f of maximum strength. Arms
hang and elbows tuck because it's cheaper.

**Dynamics.** Hand paths and the elbow–shoulder coupling don't change with speed: real arms don't
visibly flop in voluntary motion, so the tracked chain is solved kinematically. The trunk is a
second-order follower (natural frequency 1.5–2.5 Hz, damping 0.7–1) [est. from the research].

**Proportions (ANSUR II, 6,068 US Army personnel, computed from the public CSVs).** Eye height
0.935 of stature (SD 0.005). Biacromial breadth 0.237 H (men), 0.224 H (women); the GH centres
1–2 cm in from the acromion and 3.5–5 cm under its top, so about 0.21 H apart. Acromion height
0.820 H, cervicale (C7) 0.864 H, suprasternale ≈ acromion height. Eye to C7 125 mm vertically; the
ear canal 17 mm below the eyes, the skull's pivot on the neck some 3 cm under that. Hip joints at
0.53 H (Drillis & Contini). Arm span predicts shoulder width only loosely (r = 0.62–0.66), and span
is not shoulder width plus two arms: in a T-pose the shoulders move in.

## 5. Motion capture and licences

| Data | Licence | Used for |
|---|---|---|
| CMU Graphics Lab | "free for all uses"; may be in commercial products, not resold as data | Measuring (42 clips, 16 subjects) and training the learned prior (1,850 trials of the other subjects) |
| LaFAN1 (Ubisoft) | CC BY-NC-ND 4.0 | Measuring only (14 windows of 40 s, 5 subjects); never trained on |
| ACCAD, AndyData, Cologne, 100STYLE, U-Limb, Emokine | CC BY 3.0/4.0, CC0 | Permissive; not used yet (AndyData: industrial reaching and lifting, Xsens) |
| HDM05, SparsePoser | CC BY-SA | Usable, but a model trained on them takes the same licence |
| AMASS, BMLmovi, Bandai Namco (CC BY-NC), HumanEva, Human3.6M, TotalCapture, SFU, Nymeria, GORP, CIRCLE, EgoBody | Non-commercial or research only | Not used; every headset-plus-mocap set is in this group |
| Mixamo, MoCap Online | Bar machine learning | Not used |

CMU's clavicles are frozen in its solved skeletons, and LaFAN1's barely move (under 0.5 cm up to
100° of elevation): neither shows the shoulder girdle, which therefore comes from §4 alone and
counts as error against them. CMU's hand channels are 2–10× jerkier than its elbows (few markers on
the hand); the evaluation smooths them both ways, 30 ms, as a controller's are clean. CMU's shoulder
joints sit some 5 cm higher against the eyes than ANSUR's GH centres (markers on top of the
acromion), LaFAN1's further back: absolute chest placement is taken from anthropometry, not mocap.

## 6. Measuring our body against real people

**How.** The evaluation tool (`ikeval`, with its converters and the prior's trainer) and the motion
capture it plays are kept outside the repositories, with the notes above; the adapter repository's
IK playtest notes (`docs/playtest/ik.md`) say how it was run. Each clip
plays at 90 Hz as a Quest would track it: the eyes (3.5 cm above and 8.5 cm in front of the
mocap's skull pivot) and both hand joints (controller grips through `HandOffsets`), with white
noise (hands 0.4 mm and 0.08°, head 0.2 mm and 0.02° [est.]). The solver is calibrated as a player
would be: standing eye height, and the arm span from a T-pose (`straight_arm_span`). Measured per
frame: joint position errors (shoulders, elbows, hip joints, knees, ankles), the elbow's swivel and
the bones' directions, the chest's and pelvis's orientation (yaw and lean apart), the shoulders'
middle as the eyes see it, acceleration and jerk against the real body's, planted feet sliding,
steps a minute; and pictures (capsule bodies ray-traced from the eyes, looking down, and from
outside, the real body a blue ghost over ours).

**What the real bodies do** (findings that changed the solver):

- **The chest stays with the hips while the head looks round.** The head turns 10–25° from the
  hips half the time, 40–60° a tenth of it; the chest twists on the hips 3–6° and 10–25°: about
  0.15 of the head's turn, in both data sets.
- **Leaning forward, the hips stay over the feet** (the hips move −0.09 of the eyes' forward offset
  in CMU, 0.19 in LaFAN1) and the chest pitches about 4.4° per 10 cm of lean.
- **People sway, lean and look round over planted feet**: looking round (CMU 77_01) the hips swing
  36 cm and twist 55° on feet that don't move. Real people stepped 46 times a minute across the
  clips; the original solver 79, and 35 a minute just standing.
- Looking down 10° brings the shoulders 1.3–1.5 cm nearer eye level (both sets): the head nods on
  the neck about a point 3–10 cm below and 7–9 cm behind the eyes.

**What changed, and how much** (CMU's 16 evaluation subjects, never trained on; LaFAN1, another lab
and skeleton; all with tracking noise; the adapter's `docs/playtest/ik.md` has every number):

| | CMU before | CMU after | LaFAN1 before | LaFAN1 after |
|---|---|---|---|---|
| Elbow position (cm) | 10.9 | 8.9 | 9.6 | 8.5 |
| Elbow, worst 5% (cm) | 17.5 | 14.6 | 19.6 | 15.5 |
| Elbow swivel (°) | 23.7 | 18.4 | 19.2 | 13.3 |
| Shoulder position (cm) | 10.7 | 8.7 | 11.2 | 8.7 |
| Chest orientation (°) | 15.8 | 14.4 | 30.7 | 20.3 |
| Chest yaw (°) | 12.2 | 9.3 | 23.3 | 16.2 |
| Pelvis yaw (°) | 14.2 | 12.6 | 28.1 | 21.2 |
| Planted feet sliding (cm/s) | 4.7 | 0.0 | 20.2 | 0.0 |
| Steps a minute (real: 46, 88) | 79 | 66 | 181 | 114 |
| Knee acceleration (× the real one's) | 6.0 | 3.4 | 36 | 25 |
| Elbow acceleration (× the real one's) | 2.3 | 1.9 | 15.9 | 4.0 |

1. Feet planted while the centre of mass is over them (§4), landing without sliding; the trunk's
   centre of mass, 0.15 s ahead.
2. ANSUR proportions: shoulders 0.38 m apart (were 0.31), the neck's base and the hip joints where
   ANSUR has them, the skull's pivot 4.6 cm below and 8.7 cm behind the eyes.
3. The shoulder girdle by Klopčar & Lenarčič and Ludewig, scaled 0.7 on its rise.
4. A fix for bending right over: the spine's frames spun (the shoulders swapping sides at
   2,500 m/s²) because they were built from a forward hint lying along the bone.
5. Knees easing into straight; steps rising, travelling and landing without jolts.
6. An elbow cost for folding in across the chest.
7. The chest's yaw and the spine's pitch as a critically damped spring.
8. The chest from the Quest's tracked shoulders, when Virtual Desktop forwards them: simulated
   with 1.5 cm of noise, the chest's yaw 12.4 -> 7.8° (CMU) and 23.7 -> 11.4° (LaFAN1).
9. A learned prior (MANIKIN's split): the chest's yaw, the hips' heading and each elbow's direction
   from a 30-128-128-10 perceptron trained only on CMU, blended into the hand-written estimates
   (`crates/ik/src/prior.rs`; `prior_chest`, `prior_elbow` and `prior_hips` in `BodyParams`). Its
   weights ship as `crates/ik/src/prior.bin`, built into the solver: the evaluation tool's trainer
   made them from CMU's clips of the subjects not evaluated, mirrored left for right, with the
   evaluation's subjects and LaFAN1 held out. Nothing of LaFAN1's is in them.
10. Walking about the room uses the walking gait (a stride at a time) instead of balance steps
    under the moving body, which came twice as often as people's.
11. The forearm joint carries none of the hand's roll; the avatar spreads it along the forearm.

**Tried and measured worse** (kept out):

- A trunk heading estimated on its own (from the hands and from the head past a dead zone), with
  the chest a share of the head's turn from it: the chest's yaw 12.4 -> 15.1°. Head and hands say
  little more about the trunk than FRIK's blend gets; the learned prior does better.
- Hips derived from the chest by the 0.15 twist ratio: the hips followed the head turning with the
  hands still.
- Kim & Rosen's head-point swivel rule as the swivel's prior: worse at every weight.
- A balance step mirroring the other foot through the centre of mass: a 72 cm split stance after
  walking a metre.
- Holding the hips back over the feet as the head leans (the data's finding above): right in
  principle, but the feet's notion of where they belong (under the centre of mass) stepped it
  away; it needs the feet reworked first.
- A 2 s memory of the head's yaw in the learned prior: the chest led the head a second after a turn.

**Still short**: steps (66 a minute against the real 46), the legs generally (knees 15 cm off),
chest pitch (unchanged), lying and crawling (LaFAN1's ground clips). The prior's chest follows a
head turn about a third of the way when the hands hold a shouldered gun still, where the arms
should hold it more.

## 7. Next

- Rework the feet's place as the support the hips stand over (not a spot under the centre of mass),
  then hold the hips back as the head leans; step for turning as the research's sequence has it.
- Train the prior on more permissive motion (AndyData's reaching and lifting, ACCAD), and give it
  the chest's pitch and the feet.
- Use the Quest's fidelity flag (`XR_META_body_tracking_fidelity`): its tracked body only when
  IOBT is live, as the research advises.
- Fit the player from their own motion: the neck pivot from a sphere fit of the head's rotation,
  the shoulders from straight-arm circles (Ponton's Fitted Avatars).
- Self-contact: a hand touching the chest or thigh pulls the chest's surface to it.
