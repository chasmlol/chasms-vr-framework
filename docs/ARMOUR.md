# Marking a game's armour for RustVR

A game's armour is skinned by its artist with soft weights everywhere: drawn on the player's body,
a pauldron, a gauntlet's cuff or a greave bends like cloth. A **marks file** says, piece by piece,
which parts of the game's models are rigid **plates** and what each **rides**; the runtime then
moves each plate as one piece. Everything not marked stays soft, and is skinned so its joints
keep their volume. The research behind it is the Oblivion adapter's `docs/research/armour.md`.

Marks are ours, written by hand per game, and meant to be easy to read and add to: a modder making
an armour for a game marks their own models the same way. Oblivion's
(`chasms-vr-oblivion/content/armour/oblivion.toml`) marks the game's sets and is the worked example.

## 0. How the body is skinned

* **Every soft vertex keeps its volume.** A vertex on two or more bones is skinned about its
  **centre of rotation** (Le and Hodgins, SIGGRAPH 2016; `crates/avatar/src/cor.rs`): it turns by
  the blend of its bones' rotations about a point found from the rest pose and the weights (the
  middle of the limb at that joint), so elbows and knees don't pinch and wrists don't twist into a
  ribbon. Nothing to mark: every model gets it.
* **A plate moves as one piece** (`crates/avatar/src/armour.rs`). All its vertices take the same
  weights (its riders') and the same centre (its pivot), so it turns by one rotation about one
  point. Riding two bones it turns by each one's share about the joint between them: a couter
  half on the upper arm and half on the forearm turns half the elbow's bend about the elbow, which
  is what a game's helper joint does for it.
* **Plates keep the shape the game gives them standing at rest**, the arms at the sides, rather
  than their modelled shape with the arms out level: a pauldron the game drapes over the shoulder
  by bending it stays draped.
* **Studs, rivets and buckles** (islands under 3 cm) within a centimetre of a plate move with it.
* **Layers**: a plate on a higher layer is drawn over a lower one's where they meet (its depth
  taken 2.5 mm a layer towards the eye), so overlapping plates slide over one another rather than
  clip.
* **The body under the armour.** A game's body made of parts sends its skin under what's worn too
  (a part named `<game>/<key>:under`, `rustvr_protocol::UNDER`). It's drawn 1 cm away from the eye
  and 5 mm inside its own skin (a glove's fingers are often the bare hand's own, or a little
  slimmer), so it never shows through or round armour it comes up against, only where nothing
  worn is in front of it: where a rigid cuff's open end is left as the hand bends out of it, or two
  plates part over a joint, the arm inside shows, not the armour's hollow. Nothing to mark: every
  piece gets it.

## 1. Where marks live

`content/armour/<game>.toml` in the game adapter's repository (beside its `local_assets/`), or in
this repository for a game it carries. The runtime reads the game's file when it puts the player's
body together. `RUSTVR_ARMOUR=<file>` reads that file instead (to try an edit without rebuilding);
a modder's own marks can also go in `local_assets/<game>/armour/*.toml`, read after the game's
(the last piece naming a model wins). `RUSTVR_SKINNING=linear` (the game's own skinning) or
`centres` (no plates) draws the body without the marks, to compare.

## 2. Look at a piece

```sh
cargo build --release -p rustvr-avatar --bin rustvr-armour
target/release/rustvr-armour <game> --set <name>             # a [[set]] from the marks
target/release/rustvr-armour <game> <model> <model>...       # models by path (or key)
target/release/rustvr-armour <game> --set <name> --list      # the listing only
```

It puts a body together from the game's skeleton, the models and the bare body for the slots they
leave uncovered (`[preview]`), poses it through the sweep (rest, arms raised and crossed, elbows
bent as far as they go, the forearms out with the palms up and down and the wrists bent in and
back, reaching, holding a gun, a squat) and draws each pose with the runtime's own renderer into
`captures/armour/<set>/` (`--out` elsewhere):

* `<pose>.png`: a row per way of skinning (`--skin linear,centres,marked`: the game's, with joints
  that keep their volume, and with the marks), a column per view (`--views`: `front`, `side`,
  `back`, `eye` (the wearer's own, the head cut away), `arm`, `shoulder`, `elbow` (side on to the
  bend), `wrist` (from the thumb's side), `hand` (down on the back of the hand and its fingers));
* `islands.png`: every island its own colour, numbered `mesh.island`;
* `plates.png`: every plate its own colour, soft parts grey, at rest and with the arms raised (or
  in the poses `--poses` names, from the views `--views` names: what bends where);
* `uv-<model>.png`: each marked model's textures with the right side's triangles drawn where they
  take their texture from, a plate's filled in its colour (as the listing names it), the soft ones
  outlined: where its metal, leather and cloth are drawn, for `uv_within`;
* `list.txt`: every model's meshes and islands (vertices, triangles, middle and bounds, and the
  bones their skin is on, heaviest first), then the plates its marks make (what each rides, its
  pivot at rest, its layer), what went wrong, and the rules that chose nothing in it.

`--poses`, `--size`, `--plain` (one colour, no textures), `--fingers relaxed|open|fist|point|trigger`
(how the hands are held) and `--visibility hands|arms|full` (what of the body the pose pictures
show, as the menu's setting does) narrow it down.

## 3. The file

```toml
game = "oblivion"

[preview]
skeleton = 'characters\_male\skeleton.nif'

[[preview.skin]]          # the bare body, by slot
slot = "hands"
model = 'characters\_male\hand.nif'

[[set]]                   # for the preview
name = "iron"
models = ['armor\iron\m\cuirass.nif', 'armor\iron\m\gauntlets.nif']

[[piece]]
name = "Iron gauntlets"
models = ['armor\iron\m\gauntlets.nif']   # as the game's forms name them; several if alike
covers = "hands"                          # the preview's slots it covers

[[piece.plate]]
name = "vambrace"
mesh = 0
vertices = ["Bip01 * ForearmTwist", "Bip01 * Forearm"]
firm = 0.7
ride = { "Bip01 * ForearmTwist" = 1.0 }
layer = 1
```

Bone names may use `*` for any run of characters: `Bip01 * Forearm` is either forearm, and a
plate riding a bone named that way rides the one on its own side (a plate in the middle rides
both, half each). Points are in the model's space (metres; X to the body's right, Y up, Z forward)
and given for the right side; a plate on the left uses them mirrored.

### 3.1 Choosing a plate

A plate is made of **islands**: the pieces of a mesh whose triangles share corners (corners at the
same place count as shared), which is how a game's plates are usually modelled. Every field
narrows the choice; none chooses everything:

| Field | Chooses |
|---|---|
| `mesh` | One mesh: its index, or its name |
| `island` | Islands by number, as the listing numbers them in their mesh |
| `bone` | Islands whose skin is mostly on one of these bones |
| `side` | `any` (the default), `both` (left and right, not the middle), `left`, `right`, `middle`: islands whose middle is there (more than 2 cm off the body's middle plane) |
| `within` | Islands whose middle is in a box, `{ min = [x, y, z], max = [x, y, z] }` |
| `largest` | Islands no bigger than this (metres, their bounds' longest side): a plate, not the sleeve it's on |
| `vertices` | Of the islands chosen, only the corners mostly on one of these bones: a plate that's part of an island (the back of a glove, a boot's toe cap) |
| `firm` | With `vertices`: only corners with at least this share on that bone, so the corners blended across a joint stay soft and bend smoothly between the plates either side |
| `corners_within` | Of the islands chosen, only the corners in a box, `{ min = [x, y, z], max = [x, y, z] }`: a plate that's part of an island whose weights don't say where it ends (a cuff whose flare the artist blended between the forearm and the hand) |
| `uv_within` | Of the islands chosen, only the corners whose texture coordinates are in a box, `{ min = [u, v], max = [u, v] }`: the metal of a piece whose leather or cloth is part of the same island (a bracer strapped on, a couter and its strap), told apart by where the texture draws each (`uv-<model>.png`). The soft part's corners where it meets the plate go with the plate, so it stays joined to the plate's edge |
| `seam` | `true`: the plate comes apart from the rest of its island where they meet. It takes the triangles with a corner chosen, whole, and its own copy of the corners it shares, so as the joint bends it slides over or under what it meets instead of stretching or dragging it along (a gauntlet's hand plate joined to its cuff at the rim) |

One plate is made each side (left, right, the middle) of everything a rule chooses there; a plate
made of `vertices` is on the side of their bones (so an island across the body, both legs'
leather, gives each leg its own). Write two rules to make two plates of one side's islands.

### 3.2 What it rides, and where it turns

| Field | What |
|---|---|
| `ride` | Bones and their shares, `{ "Bip01 * UpperArm" = 0.5, "Bip01 * Forearm" = 0.5 }` (at most four). None: the plate's own skin's weights, averaged (the artist's say of what it follows, made rigid; shares under 3% left out) |
| `pivot` | What it turns about when its riders turn apart: a bone's joint (`"Bip01 * Thigh"`) or a point. None: the joint between its two heaviest riders nearest the plate (the shoulder for a pauldron, the elbow for a couter, the hip for a tasset), else its own middle |
| `layer` | 0 (the default) .. 7: drawn over lower layers where they meet |

### 3.3 What to mark

* **Mark what's hard**: plate, boiled leather, a boot's sole and shaft. Leave mail, cloth, fur and
  soft leather soft: they bend, and the volume-preserving skinning keeps their joints round.
* **A plate spanning a joint rides both bones**: a couter half and half about the elbow, a poleyn
  about the knee, a tasset on the pelvis and the thigh about the hip. The game's own weights are
  usually right for a pauldron (rest shaping drapes it).
* **A plate on one side of a joint rides that side's bone**: a vambrace the forearm's twist bone
  (so it rolls with the forearm), a hand plate the hand, a rerebrace the upper arm. When the
  plates either side are one island, split it with `vertices` and keep the joint soft with `firm`.
  Where the game's weights blend a hard part across the joint (a cuff's flare over the wrist), a
  soft band there bends the metal: split by place instead (`corners_within`), and let the plate
  past the joint come apart at its `seam`, as a real gauntlet's hand plate slides over its cuff.
* **Only the hard part is rigid.** A plate's straps, its leather lining and the cloth under it are
  often the same island as the plate: leave them soft (`uv_within` the metal's part of the
  texture), above all a strap across the inside of a joint, which the arm or the leg goes through
  at the least bend if it's rigid.
* **Torso plates** ride the spine bones explicitly: averaged weights often carry a little of the
  head or the neck, and a breastplate turning with the head looks wrong (`ride = { "Bip01 Spine2"
  = 0.6, "Bip01 Spine1" = 0.4 }`). Make the chest rigid and leave the belly soft where the
  breastplate runs into the skirt.
* **Overlaps**: the plate on top gets the higher `layer` (a pauldron over the rerebrace, the outer
  lames of a pauldron over the inner ones).
* **Skin showing between pieces** is often the game's own design: compare with `--skin linear`
  before calling it a fault.

## 4. Check it

Look through the poses in `<pose>.png` with `--skin linear,marked`: plates stay flat where the
game's bend, nothing pokes through what's over it, nothing tears at a joint. `list.txt` lists every
plate with its riders; a `PROBLEM` line is a bone the skeleton hasn't. Then in the game: the
runtime's log says, for each piece the player wears, the plates its marks made
(`avatar: <model> (<piece>): N plates: ...`).
