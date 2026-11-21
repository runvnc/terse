# §vector3d Device 9 — 3D vector display (version 0)

Part of the Terse machine specification, v0 (draft): `SPEC.md` is the core
machine and says how these documents fit together; this one is complete
for its device, given the core sections it builds on (below).

Draws lines, points and circles placed in a box around the screen, from a
display list like device 3's (§vector); faces, which draw nothing, hide
what is behind them; Surfaces show parts of the machine's own 2D layers
as panels in the box; in the shell's list, Screens place the screens of the
machines it started as panels in the box (§machines.screens). The program gives geometry, not
a picture: the host projects it onto the screen from an eye it chooses
(§vector3d.space), so it may let the viewer look around the models, and the
program cannot observe that choice. As with device 3, nothing drawn can be
read back.

**Builds on** `SPEC.md`: screen units (§screen), the call convention
(§calls.convention), charges (§charges), and the 2D vector display
(§vector), whose style and persistence it shares and whose layers and
phosphor style it is drawn in (§vector.layers, §vector.phosphor). The
shell's Screens are defined with the system of machines
(`spec/machines.md`, §machines.screens).

## §vector3d.space Space and view

Coordinates are signed 16-bit numbers in **sixteenths of a screen unit**
(§screen), little-endian. x and y are as on the screen (x right, y down);
z is depth in the same units: 0 is the **glass** (the plane of the screen),
positive is away from the viewer. The axes are right-handed.

The **reference eye** is at (320, 240, −960) in screen units. Seen from an
eye at (ex, ey, −960), a point (x, y, z) with z > −800 is drawn on the glass
at

  x′ = ex + (x − ex) × 960 / (960 + z),  y′ = ey + (y − ey) × 960 / (960 + z).

So a point on the glass is drawn exactly where the 2D layers would put it,
from any eye; a point behind the glass moves with the eye, one in front of
it against the eye. A host may move the eye sideways and up or down by up
to 160 units on each axis from the reference eye, at its own discretion
(for example following the viewer's pointer or head); the eye's distance
from the glass, and the glass, stay fixed. The program is not told where
the eye is.

## §vector3d.calls Calls

| `a7` | Call | Arguments | Result | Class |
|---|---|---|---|---|
| `900h` | `submit` | `a0` = list address, `a1` = length | 0 done, 1 rejected | deferred |
| `901h` | `clear` | — | 0 | deferred |
| `902h` | `style` | `a0` = style | 0 done, 1 rejected | deferred |
| `903h` | `persistence` | `a0` = persistence | 0 done, 1 rejected | deferred |

- **`submit`** copies `a1` bytes from address `a0` into the device,
  replacing the previous list, and returns 0. It is rejected, returning 1
  and keeping the previous list, if the length is 0 or more than 16,384 or
  any part of the range is outside RAM.
- **`clear`** empties the list.
- **`style`** and **`persistence`** set the display style and the
  persistence of §vector.phosphor, with the results of device 3's calls of the
  same names. They are one setting with device 3's: whichever device sets
  them, both displays (and the whole screen) are shown that way. A machine
  whose image lists device 9 and not device 3 has the setting too (device
  3's calls stay absent).

After reset the list is empty (and the style and persistence are as §vector.calls says). The list the device holds at the end of a frame is the one the
host shows for that frame. Nothing of device 9 is filled, so the fill mode
does not apply.

## §vector3d.list Display list

A list is a sequence of commands, each one opcode byte followed by its
operands. Drawing state at the start of a list: colour opaque white (255,
255, 255, 255), width 16 (one screen unit), the identity transform, no
saved transforms, current point (0, 0, 0).

| Opcode | Command | Operands | Effect |
|---|---|---|---|
| `00h` | End | — | Ends the list |
| `01h` | Colour | r, g, b, a (1 byte each) | Sets the colour; `a` is opacity, 255 opaque |
| `02h` | Width | w (2 bytes, unsigned) | Sets the line width in sixteenths, the same on the screen at any depth; 0 counts as 1, more than 255 as 255 |
| `03h` | Move | x, y, z | Sets the current point |
| `04h` | Line | x, y, z | Draws a line from the current point to (x, y, z), which becomes the current point |
| `05h` | Point | — | Draws a dot at the current point |
| `06h` | Circle | radius (2 bytes, unsigned) | Draws a circle about the current point in the transform's xy plane |
| `07h` | Push | — | Saves the transform |
| `08h` | Pop | — | Restores the transform saved last |
| `09h` | Translate | x, y, z | Moves the transform's origin to (x, y, z) |
| `0Ah` | Rotate | axis (1 byte: 0 x, 1 y, 2 z), angle (2 bytes, unsigned, in 1/65,536 of a turn) | Turns the transform about its axis |
| `0Bh` | Scale | s (2 bytes, unsigned, 8.8 fixed point: 256 is 1) | Scales the transform uniformly; s = 0 puts every point given after it at the transform's origin |
| `0Ch` | Face | n (1 byte: 3 or 4), then n corners x, y, z | Hides what is behind it (below); draws nothing |
| `0Dh` | Screen | handle (1 byte, 1–8), width w (2 bytes, unsigned) | Shows machine `handle`'s screen as a panel (below and §machines.screens) |
| `0Eh` | Surface | x, y, width, height (2 bytes each, unsigned, screen units), panel width w (2 bytes, unsigned) | Shows that rectangle of the machine's own 2D layers as a panel (below and §vector3d.presentation) |

- **The transform.** A point p given to Move or Line is drawn at M(p),
  where M is the transform in force, with p and every translation in
  screen units (sixteenths divided by 16). Translate t, Rotate R and Scale
  s replace M by M ∘ T(t), M ∘ R and M ∘ S(s): each applies to coordinates
  given after it, before the transforms given earlier, as in ordinary
  matrix-stack graphics. So "Translate (100, 0, 0), Rotate z a quarter
  turn, Move (10, 0, 0)" puts the current point at (100, 10, 0).
- **Rotation.** A positive angle θ about x turns y towards z, about y turns
  z towards x, and about z turns x towards y (clockwise as seen on the
  screen): about x, (x, y, z) goes to (x, y cos θ − z sin θ, y sin θ +
  z cos θ); about y, to (x cos θ + z sin θ, y, −x sin θ + z cos θ); about z,
  to (x cos θ − y sin θ, x sin θ + y cos θ, z).
- **The current point** is kept as Move or Line placed it, in the box (M
  applied), so a later transform does not move it, as a later Origin does
  not move the 2D display's (§vector.list). A Line after a Push, a
  Translate and a Pop starts where the Move put it. To draw a circle about
  a transform's origin, Move to (0, 0, 0) first.
- **A circle** of radius r (in sixteenths) about the current point c is
  the closed curve c + L(r cos φ, r sin φ, 0) / 16, where L is M without its
  translation (so it lies in the plane of M's x and y axes and is scaled
  with it). It is drawn as the 64 chords between the points at φ = k/64 of
  a turn, k = 0 to 63, which join without ends of their own: one curve.
  A circle of radius 0 is 64 chords of length 0: it counts 64 segments
  and no length (§vector3d.caps) and draws nothing.
- Lines and points use the current width and have round ends, as the 2D
  display's lines do (a Point is the dot a line of length 0 draws). Circle,
  Point and the transform commands leave the current point unchanged.
- **The stack** holds up to 8 transforms. Push saves only the transform
  (not the colour, the width or the current point). A Push with 8 saved,
  and a Pop with none saved, end the list.
- The list ends at an End command, at the end of the submitted bytes, at
  an unknown opcode, at a command whose operands are cut off by the end of
  the list, at a Rotate whose axis is above 2, at a Face whose n is not 3
  or 4, at a Screen whose handle is 0 or above 8, or as the stack rule
  says. Commands before that point are drawn.
- Lines, points and circles are drawn in list order, each over the earlier
  ones, blended by opacity as in §vector.list. Only faces hide: a near line
  does not cover a far one any more than a later one covers an earlier one.
- **A face** is a flat polygon that gives off no light but hides the
  lines, points and circles behind it. Its corners are transformed as
  Move's point is (M applied); the current point is not changed. A face of
  3 corners is the triangle they make; one of 4 corners is the two
  triangles of corners 0, 1, 2 and 0, 2, 3 (so it need not be flat or
  convex). Both sides of a face hide alike. A list without faces draws
  exactly as it would if faces did not exist.
- **What a face hides.** Seen from the eye in use (§vector3d.space), a point
  of a line, point or circle is hidden if the straight segment from the eye
  to it passes through one of a face's triangles and the point lies more
  than **2 sixteenths** (1/8 screen unit) beyond the triangle's plane,
  measured perpendicular to the plane, on the side away from the eye. So a
  line in a face's plane, along its edge or ending on it is not hidden by
  that face, and a solid's edges given with the same corners as its faces
  are drawn. Only a face's part with z > −800 hides (a face is cut at that
  plane as lines are). A face hides whatever is behind it wherever the
  face is in the list, before or after it. A triangle with no area, or
  seen edge on, hides nothing.
- **What is drawn of a hidden line.** A line, and each chord of a circle,
  is drawn as its pieces that are not hidden. Each piece has, along it,
  the depth cue that part of the whole line has (§vector3d.presentation). An end
  where a piece was cut is a butt end, as where a circle's chords join
  (and has no dwell where a host draws one, §vector3d.presentation); the line's own ends keep their own. A piece with a
  cut end that is shorter than 1/2 screen unit on the glass is not drawn,
  unless the whole line is that short. The whole line's length is that of
  its part in front of the near plane, on the glass, seen from the eye in
  use, before it is clipped to the screen. A point is drawn or hidden whole.
- **Faces that cross** need no rule of their own: each point of a line is
  hidden or not by each face. Faces do not hide one another and draw
  nothing, so no line is drawn where two faces cross unless the list draws
  one.
- **A Screen** is a flat panel w/16 units wide and 3/4 of that high,
  centred on the transform's origin in its xy plane: its corners are
  M(−w/32, −3w/128, 0) (top left), M(w/32, −3w/128, 0) (top right),
  M(w/32, 3w/128, 0) (bottom right) and M(−w/32, 3w/128, 0) (bottom left),
  with w in sixteenths and M the transform in force (so it is scaled,
  turned and moved with it). With the identity transform it lies like the
  glass: its top edge along x, its left edge down along y, its front
  towards the viewer. It does not change the current point. With a = top
  right − top left and b = bottom left − top left, the panel is **seen
  from behind** from an eye on the side of its plane that a × b points to
  (or in its plane), else its front is seen.
- **Which Screens are in effect.** A Screen is in effect only in the list
  of the shell of a system (the machine that has device 8, §machines.shell),
  only if it is the first Screen of its handle in the list, the handle's
  machine has a picture (§machines.screens) and the handle's rectangle shows no
  picture. Any other Screen is decoded and does nothing: it draws nothing,
  hides nothing and is not counted. A Screen in effect is shown as §machines.screens says, and hides lines, points and circles exactly as a Face of its
  four corners does; one of width 0 has no area and hides nothing.
- **A Surface** shows a rectangle of the machine's own screen: its
  **source**, x, y, width and height in screen units, clamped so it lies
  on the screen as the page's rectangle is (§page.calls: x to at most
  640, y to at most 480, the width to at most 640 − x, the height to at
  most 480 − y). What it shows is the machine's 2D layers there (§vector.layers: the terminal's cell backgrounds, the page, device 3's shapes, the
  terminal's characters and cursor, in that order and in the machine's
  style), never its 3D list. It is a flat panel w/16 units wide and as
  high as the source's shape makes it, h′ = w × height / width (in
  sixteenths), centred on the transform's origin in its xy plane: its
  corners are M(−w/32, −h′/32, 0) (top left), M(w/32, −h′/32, 0) (top
  right), M(w/32, h′/32, 0) (bottom right) and M(−w/32, h′/32, 0) (bottom
  left), lying as a Screen's do; the point of the panel a fraction u along
  its top edge and v down its left edge shows the point (x + u × width,
  y + v × height) of the screen. It does not change the current point. It
  is seen from behind as a Screen is.
- **Which Surfaces are in effect.** Any machine's list may have them (a
  machine needs no device but 9 for them; they show what its other devices
  draw). A Surface is in effect if its source, after clamping, has a width
  and a height, its w is not 0 and fewer than **4** Surfaces before it in
  the list are in effect; any other Surface is decoded and does nothing: it
  draws nothing, hides nothing, is not counted and leaves the glass as it
  is. A Surface in effect counts as one face for the caps (§vector3d.caps);
  if a cap stops it, it is not in effect either. It is shown as §vector3d.presentation says: it is opaque, hides lines, points and circles exactly as a
  Face of its four corners does (from either side), and its source is not
  drawn on the glass.
- **The pointer over a Surface.** A pointer position (x, y) on the glass
  is over a Surface in effect if the ray from the eye in use (§vector3d.space) through (x + 1/2, y + 1/2, 0) meets it in front of the near plane
  (z > −800) at a fraction u along its top edge (0 ≤ u < 1) and v down its
  left edge (0 ≤ v < 1); of several, the one it meets nearest the eye
  counts. If that Surface's front is seen, the machine receives the
  pointer event at x′ = sx + ⌊u × sw⌋, y′ = sy + ⌊v × sh⌋, where (sx, sy,
  sw, sh) is its source, with the same buttons; otherwise (seen from
  behind, or over no Surface) at (x, y) as on the glass. The list and the
  eye are those the machine's picture was last shown with (§machines.input);
  a page anchor press (§page.events) is found at the position delivered,
  so text and anchors shown on a Surface can be pressed there.

## §vector3d.caps Clipping and caps

A line is cut where it crosses the plane z = −800: only its part with z >
−800 is drawn; a point (or a dot) at z ≤ −800 is not drawn. What is left is
projected (§vector3d.space) and clipped to the screen grown by 32 units on
every side; what lies beyond that margin is not drawn, and the host clips
what it draws to the screen.

The amount one list draws is limited. In list order, each Line, Point and
Circle counts a **length**, a number of **segments** and its **ink**, and
each face, Screen and Surface in effect counts as a face:

| Command | Length (screen units) | Segments | Faces | Ink (square screen units) |
|---|---|---|---|---|
| Line | its length on the glass, seen from the reference eye, of its part with z > −800 that lies on the screen (0 to 640 by 0 to 480) | 1 | 0 | a segment's (below) |
| Point | 0 | 1 | 0 | a segment's, of length 0 |
| Circle | the sum of its 64 chords' lengths, each measured as a line's | 64 | 0 | the sum of its 64 chords' |
| Face | 0 | 0 | 1 | 0 |
| Screen in effect | 0 | 0 | 1 | 0 |
| Surface in effect | 0 | 0 | 1 | the area of the box its four corners make on the glass, seen from the reference eye, clipped to the screen; 640 × 480 if a corner is not in front of the near plane |

A **segment's ink** is counted as a 2D line's is (§vector.ink-cap), in
screen units: with its part in front of the near plane seen on the glass
from the reference eye as (x0, y0)–(x1, y1) and w its width in screen
units (the width in effect, 1 to 255 sixteenths, divided by 16), the
smaller of (|x1 − x0| + |y1 − y0| + w) × (w + 3) and the area of the box
of its two ends grown by (w + 3) / 2 on every side and clipped to the
screen; 0 if no part is in front of the near plane.

A command that would take the list's total length above 65,536, its
number of segments above 16,384, its number of faces above 256, its
number of faces times its number of segments above 131,072, or its ink
above ½ × 640 × 480 = 153,600 is not drawn, and neither is any command
after it; a face not drawn hides nothing. A command that is not drawn
does not count. The caps are counted from the reference eye whatever eye
the host draws from, and as if faces hid nothing, so the same commands
are drawn from every eye. The ink cap bounds the pixels a host draws (a
Surface's are those of its panel), the faces × segments cap the work of
hiding; together, measured on the reference host, a list at its caps
costs about a quarter of a frame to draw (`PERFORMANCE.md` R10). Screens
are the shell's view of other machines, each of which has its own caps,
and are not counted as ink (§machines.screens).

| Faces | Segments at most |
|---|---|
| 0 to 8 | 16,384 |
| 16 | 8,192 |
| 64 | 2,048 |
| 256 | 512 |

After the hidden parts are cut out (§vector3d.list), at most 16,384 pieces
are drawn, in list order; pieces beyond that are not drawn. Only pieces
within the screen grown by 32 units count. Which pieces those are depends
on the eye.

The caps are evaluated by the host, after the floating-point projection
of §vector3d.space, so hosts may differ at the boundary by rounding: a list
whose totals come within rounding of a cap may be cut one command
earlier on one host than on another. This affects only what is drawn,
never the machine's state, which holds the list as submitted (§vector3d.determinism).

## §vector3d.presentation Presentation

**What a host must draw**, from the eye it uses (§vector3d.space):

- the lines, points and circles that §vector3d.list draws, placed by the
  transform and projected as §vector3d.space says, with the width in
  effect (the same on the screen at any depth) and round ends, cut,
  clipped and capped as §vector3d.caps says;
- each in the colour in effect at its command, its opacity multiplied by
  the depth cue (below);
- without what faces, and Screens and Surfaces in effect, hide
  (§vector3d.list). Hiding is part of the picture, not of its look: a
  program draws a solid as its edges and faces and relies on the edges
  behind it not showing. What is hidden depends on the eye, which is the
  host's, so it is not machine state (§vector3d.determinism);
- in the clean style, in list order, each over the earlier ones, in the
  vector layer (below); Screens and Surfaces in effect as below;
- in the phosphor style, what §vector.phosphor requires: nothing of
  device 9 is filled, so for it that is light in its colours on the black
  screen.

**The host's choice**: where the eye is, within §vector3d.space's range;
the trigonometry and the projection, in which hosts may differ in the last
pixel; smoothing and the exact colour of blended pixels; and in the
phosphor style all of the look that §vector.phosphor leaves to the host
(glow, afterglow and smear, the beam's profile, dwell). *Note, not a
requirement:* `terse-host` draws the 3D lines, points and circles in the
phosphor style as it draws the 2D display's lines, with extra light (dwell)
at line ends and points but not where a circle's chords join or where a
face cut a line.

- **Depth cue.** Light dims with depth. Each end of a line (after the cut
  at z = −800) has the factor min(1, 960 / (960 + z)); along the line on
  the glass the factor goes linearly from one end's to the other's, which
  is its value at every point of the line, as it is the projection's own
  scale. The opacity is multiplied by the factor: 0.5 at z = 960, and never
  more than the colour's own in front of the glass. A point has its own,
  each chord of a circle those of its ends.
- **Layers.** Device 9 is drawn in the vector layer of §vector.layers, after
  device 3's shapes: over the page and the 2D shapes, under the terminal's
  characters, so text on the glass labels the models.
- **Screens** in effect are drawn as panels, in the shell's scene (§machines.screens), not in the vector layer: the shell's lines, points and circles
  are then drawn over the panels, and in the phosphor style how they are
  lit there is up to the host.
- **Surfaces.** A machine whose list has no Surface in effect is drawn
  exactly as without this rule (§vector.layers). One that has Surfaces in
  effect is drawn, from the bottom:
  1. the **glass**: its 2D layers as §vector.layers combines them, without
     the lines, points and circles of its 3D list, and where a pixel lies
     in the source of a Surface in effect, black instead (the 2D layers
     are not drawn there: what a Surface shows is not also on the glass);
  2. its Surfaces, from the farthest to the nearest by the distance from
     the eye in use to their centres (the mean of their corners), the
     earlier in the list first where two are as far; each shows its source
     (§vector3d.list) in perspective, or from behind the dark plate of a
     Screen, and covers what is below it;
  3. the lines, points and circles of its 3D list as this section draws
     them, each Surface hiding what is behind it as a face of its corners
     does.

  So the program's text on the glass outside the sources stays where it
  is, as a heads-up line, and its 3D lines are drawn over it. The 2D
  layers a Surface shows are those of the same frame, in the machine's
  style; in the phosphor style any afterglow a host draws is that of the
  2D layers on the screen, so it moves with the panel (a moving Surface
  leaves no trail), and how the 3D lines are lit over the Surfaces is up to the host,
  as over a shell's panels. Surfaces are cut at the near plane and at the
  edges of the screen as panels are. A host that cannot draw a picture in
  perspective may draw each Surface's source flat instead, scaled
  uniformly into the bounding rectangle of its projection (its part in
  front of the near plane) and centred, as §machines.screens allows for panels;
  it still hides what it would hide. The resolution the source is drawn at
  for a Surface, its filtering and the smoothing of its edges are
  presentation.

  The shell's own Surfaces, when its scene has panels (§machines.screens), are
  among them: step 1 there is the shell's glass, and its Surfaces are
  drawn with the panels, far to near, before the shell's lines. Wherever a
  machine's screen is shown (on its own, as a flat picture or as a panel)
  it is its complete picture, its Surfaces included; a Surface shows only
  2D layers, so no picture shows itself.

## §vector3d.determinism Determinism

The device's state is the copy of the list (STATE.md 1.11); the style and
the persistence are device 3's (§vector.determinism, STATE.md 1.7), held once for
both devices. It takes no input from the host: the eye is presentation,
and so is what faces hide from it. Which Surfaces are in effect depends on
the list alone; where a pointer event over a Surface is delivered depends
on the eye, which is part of the host's input (§machines.determinism). Which
Screens are in effect depends on
the shell's device 8 records, and only for what is shown; a shell's
`face` reads its list as decoded (§machines.calls).

## §vector3d.charges Charges and limits

Every call is deferred, with the base charge of 4 (§charges), and:

| What | Charge |
|---|---|
| Bytes copied between memory and the device: an accepted `submit`, when it is carried out | 1 per 4 bytes, rounded up, per call |

Provisional values (§provisional):

- the 3D display's caps (§vector3d.caps): 65,536 units of length, 16,384
  segments, 256 faces, 131,072 faces × segments, 153,600 square units of
  ink (a segment's width plus 3 units, a Surface's box) and 16,384 pieces
  drawn after hiding; its 64 chords to a circle, its transform stack of 8, its
  widest line, 255 sixteenths, and its 4 Surfaces in effect in a list
  (§vector3d.list); a face hides only what lies
  more than 2 sixteenths beyond its plane, and a cut piece shorter than
  1/2 unit is dropped (§vector3d.list); the margin of 32 units around the
  screen within which pieces are drawn (§vector3d.caps); and its eye: 960
  units from the glass, at most 160 from the reference eye on each axis,
  and the near plane at z = −800 (§vector3d.space);
- display lists of 1 to 16,384 bytes (§vector3d.calls).

## §vector3d.unspecified Not yet specified

- On the 3D display (§vector3d): filled or shaded faces; the exact
  intersection of faces (a line drawn where two faces cross); more than 4
  Surfaces in a list, Surfaces that show the 3D layer, and curved or
  non-rectangular Surfaces and screens; panels that cross one another (they are drawn
  far to near by their centres, and one does not cut another); a stereo
  picture or another display of the same geometry (the eye
  rule allows one; how a host shows it is not specified).
