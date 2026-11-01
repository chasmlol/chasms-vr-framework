# The desktop view's effects studio: research and design

The desktop view (`crates/runtime/src/spectator.rs`) is a window on the monitor for recording and
streaming. Its steadied camera sits at the aiming eye and shows that eye's picture warped onto the
camera, with everything the runtime draws (the body, the hands, held things) drawn again from the
camera. The effects studio (`crates/runtime/src/spectator_fx.rs`) is a stage after that camera.
It applies effects that would make a headset wearer sick but make footage look filmed:

- a blur that smears a turn;
- a view that shakes, widens or bends by itself;
- a shallow focus;
- a film's grade and grain.

None of it reaches the headset. The headset's picture is drawn before the studio runs, and the
studio only ever draws into the desktop window. (The other way round, OBS's own window shown on a
panel only the headset sees, is `crates/runtime/src/obs_view.rs`: never the desktop view, never the
recording.)

## 1. Research

Sources are listed at the end of each point. The research brief behind this, with every source,
is summarised here; where nothing could be found, that's said.

### Body-camera games (Unrecord, Bodycam)

- Neither studio has published its post-processing stack. Press analysis of Unrecord names:
  - a strong fisheye or barrel distortion;
  - heavy bloom, and skies blown out as a small sensor's limited range would;
  - chromatic aberration, grain, shallow depth of field;
  - a chest-vest viewpoint that moves with the body, not the head;
  - motion blur that follows movement;
  - exposure that pumps after bright light.
  (Digital Trends; LevelUp; Guided.)
- Bodycam's developer (80.lv interview) names fisheye from a post-process volume, a black border
  that reacts to shake and light, chromatic aberration and noise. They modelled the image on
  police body cameras and VHS.
- A third-party UE5 body-camera kit uses these settings:
  - a 118 degree chest camera with lag and sway;
  - fisheye that strengthens with speed;
  - chromatic aberration, hard sharpening, vignette, exposure pumping, highlight clipping;
  - noise and compression artefacts;
  - no motion blur.

What the studio takes from this:

- The Hyper-real look is a wide lens bent towards a fisheye.
- The camera is chest-worn: it turns after the body, a little down, and leans into turns.
- The look uses hard in-camera sharpening, fringes, a strong vignette, sensor noise and a cold,
  flat, desaturated grade.
- Everything is in focus: a tiny sensor behind a wide lens.
- It keeps the motion blur of a long exposure. Unrecord's footage smears; the UE5 kit leaves blur
  out.
- Compression artefacts are left out. A stream adds its own, and baking more in would compound
  them.

### Spectator cameras

- LIV's first-person cameras have:
  - smoothing (its samples use 40 %);
  - a field of view of 50 to 90 degrees;
  - a choice of eye;
  - a roll lock, so the horizon stays level.
- Half-Life: Alyx's spectator view has:
  - smoothing off, low, medium or high, which affects the spectator only;
  - a choice of eye (Valve says the dominant one);
  - a "cinematic" in-camera smoothing (yaw and pitch half-lives), which also delays the headset
    and can make the player sick.
- Beat Saber's CameraPlus smooths position, rotation and lane changes on its 360 camera.
- Unity's spectating guide warns that a tossed camera disorients spectators.

The desktop view already had this smoothing (`smoothing`, `max_lag`, `roll_lock`, the eye). What
none of these tools does, as far as could be found, is ease a snap turn for the spectator. That's
the studio's own: section 2.1.

### Photo modes

The Last of Us Part II exposes:

- the field of view and the depth of field;
- motion blur ("0 is a fast shutter");
- film grain, vignette and chromatic aberration;
- brightness, exposure, saturation and sharpness;
- filters (Noir, Blue Orange, Vintage) at a strength;
- bars at 1.33 or 2.75.

Ghost of Tsushima has focal length, focus distance, grading with strength, exposure and bars.
Cyberpunk 2077 has aperture and autofocus, exposure, contrast, highlights, vignette, chromatic
aberration, grain and filters. Every one has a grade or filter with a strength, and bars of a
film's shape.

The studio's Colour section follows them: exposure, contrast, saturation, white balance, a grade
with a strength, a LUT with a strength. It has no bars: the player doesn't want them (a letterbox
was built, then taken out at the first playtest's word).

### ReShade

There's no authoritative ranking. The bundled packages show what's thought essential:

- a LUT;
- CAS sharpening;
- chromatic aberration;
- film grain;
- lift, gamma and gain;
- a vignette and a border;
- bloom (MagicBloom);
- depth of field (CinematicDOF, qUINT ADOF).

The advice on its forum is to model the lens first, then the camera or film, then grade, and to
sharpen after grading. The final pass does them in that order:

1. The lens: fisheye, then fringes.
2. Sharpening, on the lens's picture.
3. The film: glow and exposure.
4. The grade, then the LUT.
5. Vignette and grain, last, as a print's.

### Film conventions

- **The 180-degree shutter**: at 24 frames, 1/48 s of each. This is the blur of motion audiences
  read as film.
- **Halation**: light bounced back through the film base, a red-orange glow round highlights. The
  studio's glow is tinted warm (1.0, 0.82, 0.68) for it.
- **Grain**: coarser at higher ISO, and strongest in the middle tones.
- **Teal and orange**: skin is orange, so the shadows go teal. It's widely called overused, so
  the Cinematic look has it at 60 %.
- **Rack focus**: no standard speed. The studio's focus pull is a setting, 0.4 s in Cinematic.

### Motion blur

McGuire et al. (I3D 2012), as NVIDIA's sample implements it:

- a velocity buffer;
- TileMax: the longest motion per tile of K pixels;
- NeighborMax: the same over the 3x3 tiles round each;
- a gather along the neighbourhood's longest motion, with about 15 samples weighed by depth order
  and each sample's own blur;
- a cost that scales with the resolution.

Jimenez (Call of Duty: Advanced Warfare, SIGGRAPH 2014) adds that they took out camera-rotation
blur in their shooter. For a player it smears what they track. For a viewer of a recording, it's
the camera's own motion, and the studio keeps it, with the middle of the view kept sharper.

### Camera shake

Eiserloh (GDC 2016):

- Keep a trauma value from 0 to 1; events add to it, and it wears off linearly.
- Shake by the trauma's square.
- Use smooth noise, not random numbers.
- In 3D, shake by rotation only; "tread carefully in VR".

The studio does all four:

- the trauma squared;
- sums of sines at frequencies that never line up, as its smooth noise;
- rotation only, since a translation would also break the picture's parallax;
- never in the headset.

### Field of view with speed

- MotoGP kept its angle below 100 mph and widened it to nearly double at top speed, warning that
  too wide is nauseating (Hargreaves).
- Unity's old FOVKick widened 3 degrees over a second.
- Minecraft's sprint widens it "slightly".

The studio widens 8 to 12 degrees between a run and a gallop, quickly in and slowly out. It never
widens past what the game's pictures cover.

## 2. Design

### 2.1 Smoothed jumps (for viewers)

A snap turn, a snap crouch or a recentre jumps the game's world under the player in one picture.
The headset must snap: that's the point of it. For a viewer, it's a cut every time.

**Seeing a jump.** The studio watches the mapping each picture was drawn with (`SlotInfo::eye_map`):

- It keeps the world's steady turn and walk from picture to picture.
- A picture that goes further than they would by more than a turn (8 degrees), a step across
  (35 cm) or a rise (12 cm) is a jump.
  - Stopping short isn't one: a landing, or the end of a turn.
  - Nor is a hitch: the game held up, then taking its steps all at once a picture later, the way
    the world was already going, up to 0.3 s of them. (The range does this every time a test
    capture holds the runtime up; Oblivion does it loading a cell.)
- Pictures are timed by the wall's clock, not the runtime's frames: a game walks on in real time
  while the runtime's frame is held up.
- Another place is a cut, not a jump: a move of more than 20 m, a gap of half a second, or a
  change of scale.

**Turning through it as a head would.** The camera on the world is turned back through the jump
about where the camera is. It then turns out of it the way a head turns, not along one even curve
(the first playtest: "smooth but just 1 big smooth jump").

- **A damped spring** (damping 0.7) pulls the camera towards an intent, the turn the head still
  means to make. The intent lets go of the jump over the turn's first moment (an eighth of its
  settling time), so the turn:
  - gathers speed instead of starting at full tilt;
  - is fastest about a third of the way through;
  - eases in over a longer tail;
  - lands with about a degree of overshoot and settles.

  That's the shape of a head's own turn: a quick start and a softer, longer stop.
- **How fast it turns** is `snap_speed`: degrees a second at its fastest, for a 45 degree snap.
  - The spring's shape fixes the fastest turn times the settling time (96 degree seconds for 45),
    so a speed is a settling time.
  - The default is 160 degrees a second: half done in 0.22 s, settled by about 0.7 s.
  - The first playtest's 320 (settled in a third of a second) was quicker than the player liked:
    "not like smoother but slower, like by 50%".
  - Bigger turns take a little longer and peak faster, smaller ones less, as a head's do.
- **A second snap mid-turn** adds to the turn still to come and keeps the speed it has: two quick
  flicks become one continuous turn.
- **The camera leans into the turn**, by its turning speed (`HEAD_LEAN`): about 0.7 degrees at the
  default speed's fastest, a degree and a half at 320.

- The body and what the runtime draws keep the player's own camera, since they turn with the
  player.
- The world turns smoothly round them, as a flat game's camera would.

**Filling in.** Mid-turn, part of the view is past where the new picture reaches. There, the
picture from just before the jump fills in:

- The desktop view's own background is copied every steady frame, so the last one before a jump
  is kept.
- It's looked up by direction, the world's turn since undone.

Between the two pictures, every direction of a snap of up to about 100 degrees is covered.

**Stretching the edges** (`fill_edges`, on by default). The desktop camera can see past both eyes'
pictures in several ways:

- it lags the head on a fast turn (up to `max_lag`);
- the chest mount or a shake turns it;
- a wide lens or the speed widening reaches further.

Past the pictures, the reprojection has nothing, and it was black. There, the edge of the nearer
eye's picture is drawn out over the gap instead:

- each direction is clamped to that picture's border;
- it's softened along the border (seven taps, spread wider the further out it is).

It's a smear, not the world, but it isn't a black band. It's drawn before the body, so the hands
stay over it.

**Moves.** The picture is looked up by direction, so moving the camera wouldn't show a crouch or a
recentre's step. Those dissolve from the picture before the jump over the same time.

The motion blur blurs the eased turn as a camera pan, not the jump.

### 2.2 Speed

- The world's steady walk is the player's speed through it: running, galloping, falling.
  - The room-scale steps don't count: the mapping's origin moves with them.
- The widening is eased in over 0.35 s and out over 0.6 s.
- It never goes past what the game's pictures cover across: about 108 degrees on a Quest 3 with
  no overscan.
  - The looks keep their lens narrower than that (90 in Cinematic, 95 in Action) to leave room.
  - Overscan (on the Display tab, and the Studio tab's Camera section) gives more room, at the
    headset's cost.

### 2.3 Shake

Events add trauma and kicks. A kick is a turn that springs back in about a fifth of a second.

| Event | Trauma | Kick |
|---|---|---|
| Pistol, rifle, shotgun shot (the rig's cues) | 0.12, 0.1 (a burst shakes steadily, not ever harder), 0.4 | the muzzle's climb, 0.8 to 2.5 degrees |
| A bow loosed | 0.06 | a little |
| A blow or a thrown thing landing hard (`Impact`, `Melee` over 0.25) | 0.45 x intensity squared | a jolt away from the hand |
| An explosion (the rig's, or the game's `EV_BLAST`) | up to 0.9, by distance | a jolt |
| The game's rumble (`EV_RUMBLE`: its recoil, its damage) | 0.5 x strength | none |
| The game's own gun firing (`EV_GAME_SHOTS`) | 0.15 a shot | the climb |
| A hard landing (falling past 5 m/s, then stopped) | up to 0.6 | a dip of up to 6 degrees |

The handheld sway is the same noise at 0.2 to 0.9 Hz: half a degree or so.

Oblivion's adapter sends no rumble or damage event yet. Hits taken don't shake its camera, though
the player's own blows landing and hard landings do.

### 2.4 Depth of field

- **The distance** of each pixel is the game's depth as the desktop camera sees it
  (`compositor::reproject_post`), or the body's, where the body is nearer.
  - The body's depth is drawn again from the desktop camera (`PlayerBody::draw_depth`), only while
    the blur or the focus needs it.
- **Where to focus**, in this order:
  1. What a hand holds up close (within 0.7 m and 25 degrees of the view's middle): its distance.
  2. Where a held gun aims: the depth there.
  3. The view's middle, where the head looks.

  The middle of five depths round the point is taken, so a gap between two things doesn't throw
  the focus to the background. It's pulled to a new distance in log space, most of the way in
  `focus_seconds`.
- **The blur** is a lens's:
  - proportional to 1/focus - 1/distance (focused at 2 m, infinity blurs the most), so focusing far
    keeps the background almost sharp, as a real lens would;
  - in front of the focus, its own share (`dof_near`, 0 by default: the hands stay sharp);
  - gathered at half size, 24 samples on a golden-angle disc;
  - a sample counts only where its own blur reaches the pixel, so a sharp thing doesn't smear onto
    the blurred background behind it;
  - a blur in front is looked for round each pixel, so it spreads over what's behind it.

### 2.5 Motion blur

- **Each pixel's motion over the shutter.** For the world, the camera's motion is taken from the
  game's depth and the world's walk and turn this frame, the jump being eased included. For what
  the runtime draws, it comes from the hand that carries it.
  - The hand's motion is rigid (last frame's pose against this frame's).
  - A pixel's share of it grows with its distance from the body's axis: none on the torso and
    legs, all of it half a metre out (a hand, a forearm, a sword held out).
  - The two hands are weighed by distance.
  - This needs no velocity pass in the avatar renderer. A sword's tip, a metre from its hand,
    moves with the hand's turn.
- **The camera's part** is scaled by `camera_blur` and is stronger towards the edges (the middle
  is kept `centre_sharp` sharper). The hands' part is scaled by `object_blur`.
- **The shutter**: `shutter` degrees of a frame of a recording at `recorded_fps`, against this
  frame's time. 180 at 24 is 1/48 s.
- **The reconstruction** is McGuire's:
  - tiles a 32nd of the width, which is also the longest blur;
  - the longest motion per tile (every other pixel), then per 3x3 neighbourhood;
  - 14 jittered samples along it.

### 2.6 Lens and film (one pass)

- **Fisheye**: from the middle out, bent towards an equidistant fisheye, with the corners kept
  where they were. The middle is magnified, the edges are compressed, and nothing past the
  picture is needed.
- **Fringes**: red sampled a little further out and blue a little further in, growing with the
  distance squared.
- **Sharpening**: contrast-adaptive (stronger where there's little detail), and never on what the
  depth of field blurred.
- **Glow**: a quarter-size, separable Gaussian of what's bright, through a soft knee, tinted warm
  for halation.
- **Colour**:
  - exposure (in stops) and a white balance that keeps brightness;
  - contrast about middle grey, in stops;
  - saturation;
  - a soft shoulder past 0.8, so a push doesn't clip.
- **Grades** are made on the picture as seen (its sRGB encoding):
  - teal and orange;
  - bleach bypass;
  - warm print film;
  - a body camera's cheap sensor;
  - noir through a red filter.
- **The LUT** is any `.cube` file in the settings folder's `luts`.
- **Vignette**: a lens's cos^4 fall-off. **Grain**: triangular noise, strongest in the middle tones.

The game's own bloom, fog, lens and grading all stay: the studio works on the picture the game
made.

### 2.7 The flat HUD

The game's own HUD, from its own picture of it (`Slot::hud`, the UI premultiplied over clear),
where a flat player has it. Each of the layout's pieces (`HudLayout`, group 0) is placed this way:

- It's anchored to the side of the screen it's nearest (thirds across and down: the bars at the
  bottom left, the compass at the bottom middle), as far from that side as the game has it.
- It's sized so a line of the game's text (`text_height`) is 3.75 % of the screen's height, which
  is Oblivion's own on a flat screen (36 of 960 menu units).
  - The HUD picture is eye-shaped in VR (Oblivion lays it out 1280 wide for a tall screen), so it
    can't simply be stretched.
- What the headset hides stays hidden: the crosshair, since the hands aim and the middle of the
  view isn't where a gun points.
- A held thing's card stays floating over the thing.
- With the flat HUD on, the same pieces are taken out of the desktop view's world. The headset
  keeps them where the menus feature floats them.
- Menus still float in the world for viewers, where the player works them.
- The HUD's picture is copied before the game's slot is given back: the effects (and the HUD)
  draw after the headset's frame is handed over.

### 2.8 When it runs

- `Stage::target`, at the start of the desktop view: the studio's own picture if it has effects
  to draw, else the window's.
- `Stage::shot`, after the steadied camera: the camera, the world's camera and the field of view.
- `Stage::background`: the world from the world's camera; one pass filling where the eyes'
  pictures don't reach (the history mid-jump, else the stretched edges); and the flat HUD's pieces
  out of the layers.
- `Stage::drawn`, after the picture: the body's depth from the camera, and the HUD's copy.
- `Stage::finish`, after `submit`: the effects and the HUD into the window. The headset's frame
  is never held up by them.

What's off isn't drawn. The clean look draws straight into the window, as before, at the cost of
one copy a frame for smoothed snaps and a pass that leaves every pixel the eyes' pictures reach
alone (no change in the measured times). The shaders compile on a thread of their own at start-up;
until they're ready (a moment), the view shows without effects.

### 2.9 The other eye's fog (the third playtest)

The desktop view looks up the aiming eye's picture and, where that doesn't reach, the other eye's.
With the overscan up and the desktop view wide, the join sits well inside the view. In Oblivion it
showed as a step in the distant fog, with thinner fog past it.

- **Gamebryo's fog isn't by distance.** Oblivion's vertex shaders fog by
  `length(mul(ModelViewProj, v).xyz)` (`SLS2000.vso`, `FogParam` = (far, far - near)): the length
  of the clip-space x, y and depth.
  - That's the depth times `sqrt(1 + ndc.x^2 + ndc.y^2)`, so it grows towards the frustum's edges.
    It only matches the true distance in a symmetric 90 degree frustum.
  - The headset's eyes are off-centre, mirrored, and the overscan widens them. The same direction
    sits at a different `ndc` in each eye.
  - At the playtest's join (the right eye's left edge, 51 degrees out, 11 degrees of overscan),
    the right eye measured 1.43 times the depth and the left eye 1.12: about a fifth less fog.
- **The fix, desktop only** (`refog` in the compositor's shader, `ClipFog`):
  - Where the other eye's picture fills in, each pixel's distance along its ray comes from the
    other eye's depth: followed out through it, or by direction.
  - From that distance it takes the fog amount each eye's frustum gives it, from the game's own
    fog (the adapter's `GameLighting`: colour, near, far).
  - The difference is laid over in the game's display values (or taken off, no more than
    fourfold).
  - It's on only for `LM_GAMEBRYO`, with fog. The headset's background never has a second eye.
    The sky has no depth and is left alone.
  - A GPU unit test checks it with the playtest's frustums
    (`the_other_eyes_picture_takes_the_first_eyes_fog`, in `compositor.rs`): with far hills and
    near ones, over 0.06 off without it, under 0.02 with it.
- **Left as it is:**
  - The edge stretch past both pictures (2.1) smears the other eye's edge as the game fogged it.
    It's a smear for fast turns, and the step there is under it.
- **Then the game's fog went by distance** (at the player's word: it changes the headset's
  picture).
  - The adapter rewrites each vertex shader as the game makes it (Oblivion's `crates/oblivion/src/
    fog.rs`). The fog's distance becomes the view-space length: the clip position's x and y back
    through the frustum, with its depth.
  - The headset's two eyes, any overscan and the water now agree, and the desktop view's two eyes
    meet without a step of their own.
  - The adapter says so in `GameLighting::fog_fade.z`. The desktop view's `refog` stays off then,
    and is the fallback for a game whose shaders keep their own fog.

## 3. With the aim zoom and the view bob

The aim zoom (`spectator::AimZoom`: the field of view narrows while aiming down sights or with a
drawn bow) and the view bob (`view_bob.rs`: off, the runtime's own or the game's own bob while the
stick or the game moves the player) were built on branches of their own beside the studio. Both are
merged with it (2026-10-31, at the player's word). `session.rs` makes the steadied camera, narrows
it for aiming, bobs it, and hands both cameras to the studio's shot (`ShotInput::cam` and
`bobbed`):

- **The field of view** (`field_of_view`): the look's lens (`lens_fov`, else the desktop view's own
  `fov`), narrowed to exactly `aim_fov` as far as the aim zoom has narrowed the desktop view's own
  (a lens of 90 aimed is 65, not 90 x 65/105), plus the speed widening, which fades as the zoom
  goes in and never reaches past what the game's pictures cover.
- **The bob** turns the world's camera (`bob_turn`): the bob's own turn, and its move as the turn
  that moves what's 2.5 m away as much. The body's camera isn't moved.
  - The game's picture is looked up by direction, so moving the camera never moved the world, only
    the body, the wrong way and into the camera. A landing's 20 cm dip put it in the chest.
  - As a turn of the world's camera, the world bobs and the body stays with the camera, as a flat
    game's arms do.
  - The bob is damped as the view narrows for aiming (the bob's own).
- The shake and the chest mount go on top of both cameras.
- **Settings**: the studio's are `[studio]` (`Studio`, `Look`, `Grade`), the aim zoom's are in
  `[spectator]`, the bob's are `[desktop_bob]`.
- **The Studio tab** (`Tab::Studio`, drawn by `spectator_fx::panel`). Its Camera section has the
  desktop field of view (one slider: the look's lens where it has one, else the desktop view's
  own), the aim zoom with its aiming field of view and its times in and out, the overscan, and the
  view bob's mode and strength. The Display tab keeps the desktop view's eye, smoothing, lag,
  level horizon and fullscreen.
- The dominant stick scrolls the RustVR menu's tab wherever the laser points
  (`RuntimeMenu::scroll`, at `ui.menu_scroll`), as it scrolls the game's menus.
- **Oblivion's posture isn't a bob** (the second playtest). Standing still, Oblivion's first-person
  idle holds `Camera01` 9.5 cm up and 3 cm aside of its node.
  - The view bob branch passed that on as its bob: it lifted the desktop camera into what's drawn
    behind the head, and sat the headset 10 cm lower.
  - The adapter now leaves out where the camera has sat lately (`camera::Rest`, following it over
    1.5 s), for the headset and the desktop view both. Steps, dips and floats pass.
- **Still open**: the Hyper-real look's chest-worn camera would want the bob's Generic mode (a
  body camera bounces with the steps). The looks don't set the bob, which is the viewer's own
  setting, like the smoothed snaps.

## 4. Performance

The effects never hold up the headset's frame: they run after `submit`. The timer in
`Stage::finish` logs, every ten seconds:

- the desktop view's picture (the background, the body, the layers: as before, plus a copy for
  smoothed snaps);
- its effects and HUD.

Both are medians, in ms on the GPU. The picture's time includes what the studio adds before the
headset's frame is handed over: the body's depth from the camera, the HUD's copy, and the copy for
smoothed snaps.

On the range at 2227x1252 (the window fitted to a 1440p monitor), RTX 5090, the runtime at 120 Hz,
three runs of ten seconds each:

| Look | Picture (before the headset's frame) | Effects and HUD (after) |
|---|---|---|
| Clean (smoothed snaps on) | 0.16 to 0.18 ms | none |
| Cinematic | 0.15 to 0.17 ms | 0.30 to 0.35 ms |
| Hyper-real | 0.17 to 0.18 ms | 0.30 to 0.34 ms |
| Action | 0.16 to 0.18 ms | 0.28 to 0.33 ms |
| Vintage | 0.09 to 0.10 ms | 0.30 to 0.35 ms |

What the studio adds before the headset's frame doesn't show in the picture's time: it's within
the run-to-run spread. The effects are a third of a millisecond, after it.

The studio's own pictures are half floats. R11G11B10 render targets came out empty on the driver
this was measured on, and the cost of half floats is lost in the numbers above.

## 5. Not done

- **A velocity pass for the body** (per bone). The hands' rigid motion covers what moves fast; the
  elbows blur a little less than they would.
- **Rolling-shutter skew and compression artefacts** for the body-camera look.
- **A frame rate for the desktop view of its own** (drawing it every other headset frame, for
  less GPU). The steadied camera would need its time, and `present` would need a hook.
- **Hits taken in Oblivion.** The adapter would send `EV_RUMBLE` on damage taken.
