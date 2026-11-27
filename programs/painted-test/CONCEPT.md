# The Stillwind Spire: concept and technique test

## The game

A walk-around graphic adventure: a painted scene above, a traveller you
steer with the arrows, and typed commands below. In the valley of Hallow
Mere the wind stopped in spring, and with it the evening: the sun has sat
in the same notch of the mountains for months, the mills are still and the
crops will not ripen. Ysolde Fenn, a retired surveyor of weather, walks up
to the old tower at the head of the valley, where the winds were once
kept. Each floor of the tower holds one bound wind (the dry east wind, the
rain-bringer, the night breeze), each stopped by a different small wrong
that she has to find and put right. Every wind she frees changes the
scenes below it: the pennant lifts, clouds move, the light goes on toward
night, and paths open that the still air had closed. The last floor holds
whoever stopped them, and why.

## How a scene is made

- **Background: one page figure.** The scene is painted in the article
  path syntax (`ARTICLE.md` section 6: `M L H V C S Q T Z`, `fill`,
  `stroke`) in `scene.paths`, which `scene.py` compiles into a page
  document of one 640 x 352 figure (`scene_data.h`). The program submits
  it once, from ROM, and never again: the page is laid out once, outside
  the machine's frames, and stays put.
- **Actors and occluders: the 2D vector display**, which lies over the
  page (`SPEC.md` §vector.layers). The traveller is about 30 shapes
  (discs, triangles, round-capped lines) in quarter units, scaled by the
  height of the feet (full size at the bottom, 0.39 at the far edge of the
  meadow), mirrored by facing, with an 8-step walk cycle. The shadow is a
  fat translucent line: its round caps are the nearest thing to an
  ellipse.
- **Depth by drawing order.** Anything whose base lies in front of the feet
  is drawn after the traveller. The boulder lives only in the vector list
  (low-poly triangles about one point), drawn before or after him. The
  tower is page art; while he is behind it, its foot is drawn again in the
  vector list (rectangles, discs and lines in the page's colours, window
  and door included), so he disappears behind the wall.
- **Walkable ground** is a box less the footprints of the tower and the
  boulder; scene changes that are small (the open door and its light) are
  vector shapes too, so the page is never submitted twice.
- **Text** is the terminal's bottom 8 rows: a status line, 6 lines of
  replies wrapped between words, and a prompt.

## What the test showed, and its cost

Measured with `terse-host --drive` at 1280 x 960 (software renderer, and
the GPU renderer, which in the headless compositor is llvmpipe):

- **Machine budget:** walking costs about 2.9 to 3.0 % of a frame
  (about 3,700 of 125,000 units): reading keys, building and submitting a
  list of 470 to 550 bytes. Standing still costs nothing (no list is
  submitted when nothing changed; the picture stays still, `expect idle`).
  The worst frame is a typed command at about 8 % (the reply area is
  rewritten). The start, with the page submitted, 7.2 %.
- **Ink:** 6,500 to 9,800 square units a frame of the cap's 230,400 (3 to
  4 %), as the `stats` command counts it; outlined, 7,700 at the open
  door.
- **Page figure:** 3,234 bytes, 53 of 256 paths, area 739,785 of
  1,228,800, length 47,065 of 65,536. Rasterised once off the frames in
  2.9 ms (outlines 8.9 ms) at 880 x 484 pixels.
- `expect no-slow` holds with the software renderer for walking, typing,
  the door and the occlusion.

## Limits found

- **The figure's length cap binds first.** Every full-width shape (a sky
  band, a mountain range, the meadow) costs about 2,700 of the 65,536,
  whatever its detail: of the 47,065 used, about 35,000 went on 13 such
  shapes. A richer scene would
  use two figures (the caps are per figure: a sky figure over a ground
  figure) or let the page's background be the top band, as here.
- **Never re-lay the page per frame.** A submit is laid out on the host's
  thread over some frames; an unchanged figure is shown again at once,
  but anything that changes often belongs in the vector list.
- **Occluders are hand copies.** The vector display has no paths, so a
  page shape that must hide the actor is repeated as rectangles, discs and
  triangles in the same colours; curves (the tower's rounded base) are
  approximated, which works only where the actor never reaches.
- **Phosphor and fill mode 1 outline everything**, page figures included,
  so nothing hides anything: occlusion is lost (the program then skips the
  tower's copy, which would only add lines), sky bands become horizontal
  lines across the scene, and the boulder's facets show as spokes. It
  reads as a line drawing; a game meant for both styles should paint
  outlines with `stroke` where they matter and keep fills few.
- **Host problems seen (not the program's):** the software renderer kept
  showing the filled figure after a switch to phosphor, for good (an
  arrival of the outlined figure was counted in the cached light's key
  before the quads used it); fixed in `crates/terse-host/src/soft/page.rs`.
  The switch of style itself sometimes makes one or two frames late while
  the host sets the phosphor up. llvmpipe composes a 1280 x 960 frame in
  about 33 ms, so the "GPU" in a headless compositor is late whatever the
  program does.
