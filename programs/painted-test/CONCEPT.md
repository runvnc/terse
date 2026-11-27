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

Since the page device has layers (`spec/page.md` §page.layers), the whole
picture is the page's; the first version, with the traveller in the 2D
vector display, is described after it (its numbers are kept for
comparison).

- **One document, two figures.** The scene is painted in the article path
  syntax (`ARTICLE.md` section 6: `M L H V C S Q T Z`, `fill`, `stroke`,
  `layer`) in `scene.paths`, which `scene.py` compiles into a page document
  (`scene_data.h`) that the program submits once, from ROM, and never
  again. It is cut at y 176 into two figures 640 x 176 stacked with no
  gap (`split 176`): each figure has caps of its own, and a path across
  the cut is in both. The top one is sky, mountains and the tower's top;
  the lower one holds the ground and every layer.
- **Layers in the lower figure**, in this order after the scenery: the
  open door (12), the traveller's poses (1-11), then copies of what stands
  in front of him: the boulder (13), the foot of the tower (14) and the
  open door again (15). A copy is the same paths as the scenery's
  (`@copy` / `@paste` in scene.py), so when it is shown nothing changes
  but that the traveller is hidden where they cover him: occlusion comes
  from the order of the paths alone, curves included.
- **The traveller** is 11 poses painted by `actor.py` at full size, feet
  at (30, 344): side poses for the strides 0, 12, 20, -12, -20 (the walk
  cycle's 8 steps), and 3 each toward the viewer and away. The program
  shows one at a time and hides the rest; `layer` moves it so the feet are
  where he stands, scales it by the feet's height (256/256 at the bottom,
  99/256 at the far edge of the meadow) and mirrors the side poses for
  walking left. The shadow is a fat translucent line in each pose.
- **Depth**: while his feet are above the boulder's or the tower's base
  and he is across it, its copy is shown. In the outline styles nothing
  is filled, so nothing could hide him: the copies stay hidden there.
- **The 2D vector display draws nothing**; the program keeps it only to
  choose the style and fill mode (device 3's `style` and `fill`).
- **Text** is the terminal's bottom 8 rows: a status line, 6 lines of
  replies wrapped between words, and a prompt.

### The first version: the traveller in the vector display

- The background was one page figure 640 x 352; the traveller about 30
  vector shapes in quarter units, scaled by the feet's height, mirrored by
  facing, with an 8-step walk cycle; the boulder low-poly triangles in the
  vector list, drawn before or after him.
- The tower is page art, so while he was behind it its foot was drawn
  again in the vector list (rectangles, discs and lines in the page's
  colours, window and door included): hand copies, curves approximated.
- Walkable ground is a box less the footprints of the tower and the
  boulder (unchanged); the open door was vector shapes too.

## What the test showed, and its cost

Measured with `terse-host --drive` at 1280 x 960 (software renderer; the
GPU renderer, which in the headless compositor is llvmpipe, shows the
same pictures), the same walk as `crates/terse-host/tests/painted_test_window.rs`.

| | First version (vector traveller) | Page layers |
|---|---|---|
| Machine budget while walking | 2.9-3.0 % of a frame (about 3,700 of 125,000 units: reading keys, building and submitting a list of 470-550 bytes) | 0.5-0.6 % (about 700 units: reading keys and 1 to 6 `layer` calls of 4 units, only for what changed) |
| Standing still | nothing (no list submitted) | nothing (no call) |
| The start | 7.2 % (the page submitted) | 6.3 % (a bigger document: 11,707 bytes, 1 unit per 4) |
| A typed command | about 8 % | 4-5 % (the reply area rewritten) |
| 2D ink | 6,500-9,800 square units a frame of 230,400 (3-4 %); 7,700 outlined | 0: no vector list |
| Page figures | one, 3,234 bytes: 53/256 paths, area 739,785/1,228,800, length 47,065/65,536 | top: 1,771 bytes, 35/256 paths, area 237,216/901,120, length 29,031/65,536; lower: 9,919 bytes, 232/256 paths, area 575,569/901,120, length 54,109/65,536 |
| Layers | none | 15/64 Layer commands, layered area 156,444/614,400 |
| Rasterised (off the frames) | 2.9 ms (outlines 8.9 ms), 880 x 484 px | the two figures' bases and 15 group images, about 50 ms in all in a debug build (each pose about 2.5 ms, 40 x 108 px) |

`expect no-slow` holds with the software renderer for walking, typing,
the door and the occlusion; `expect idle` holds while he stands still.
The shots in `shots/` are the layered version: `rock.png` (behind the
boulder), `tower.png` (half behind the tower), `door.png` (the door open,
lamplight on the path), `phosphor.png` (the phosphor style: outlines, so
the copies are hidden and nothing is occluded).

## Limits found

- **The figure's caps bind first.** Every full-width shape (a sky band,
  a mountain range, the meadow) costs about 2,700 of the length cap's
  65,536, whatever its detail. The single figure of the first version
  used 47,065; with the traveller's 11 poses (about 160 paths) the scene
  no longer fits one figure's 256 paths, hence two figures (the lower
  one at 232 of 256). More poses (a full 8-step cycle for every facing)
  would want a third figure or fewer paths a pose; the layer limits
  themselves (64 Layer commands, 614,400 square units) are far off.
- **Never re-lay the page per frame.** A submit is laid out on the host's
  thread over some frames; an unchanged figure is shown again at once.
  Anything that changes often is a layer (shown, hidden, moved, scaled:
  no layout) or, failing that, the vector list.
- **Occluders were hand copies** in the first version (the vector display
  has no paths); with layers they are the scenery's own paths again, in
  a layer after the actor's, exact. A scaled pose is the full-size
  picture drawn smaller, so the far traveller is a little softer than
  paths drawn at that size would be (the host's choice: it may redraw).
- **Phosphor and fill mode 1 outline everything**, page figures and their
  layers included (order is still order), so nothing hides anything:
  occlusion is lost (the program then hides the copies, which would only
  add lines), sky bands become horizontal
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
