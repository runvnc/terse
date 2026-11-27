# Writing scenes for Stillwind

The story format is Low Bell's (`programs/lowbell/STORY.md`: rooms,
objects, people, verbs, rules, events, hints, achievements, the clock).
What is different here: each room is a **painted scene** in which the
player walks Ysolde about, and the vector art, the lantern beam and the
sounds of Low Bell are gone. Nobody needs to touch C.

    python3 story.py --check                 check the story and the scenes, print their sizes
    ./build.sh                               compile, and build stillwind.terse
    python3 paint.py scenes/meadow.paths --actor scenes/actor.paths --png m.png --show goat_key
    python3 paint.py scenes/meadow.paths --actor scenes/actor.paths --costs 186
    python3 actor.py -o scenes/actor.paths   Ysolde's poses (build.sh does it)

To play, the scenes' blobs must be where the host can get them (below):

    for f in blobs/*.scene; do terse put "$f" --store <store> --relay none; done
    terse-host stillwind.terse --store <store>

## A room

    room meadow "The Meadow Path"
      scene meadow                   # scenes/meadow.paths
      depth 230 118 312 256          # her scale (256ths) at y 230 and at y 312
      start 330 300                  # where she stands when no exit says
      walk 14 274 470 312            # the floor: boxes she may stand in
      walk 14 248 104 312
      exit north foot at 456 226 zone 440 222 472 230
      exit west "The track to the mill ... not painted yet." zone 14 230 20 312
      exit in hall through tower_door at 330 257
      show goat_key if goat in meadow and not traded
      front boulder_front 104 196 262
      front door_open_front 290 370 256 if open tower_door

- **The picture** is 640 x 320 units, (0, 0) at the top left; her feet
  are a point on it.
- **`walk`** boxes (left top right bottom) are the floor; she walks in
  their union, around what is not floor (the boulder's foot, a fence).
  Boxes that overlap are joined: she finds her way from one to another
  when she walks by herself. story.py checks that every box can be
  reached from the first, and that `start`, every exit's `at` and every
  object's `stand` are on the floor.
- **`depth`** is how big she is: 256 is the size actor.py paints her (the
  front of a scene), smaller toward the back. story.py checks that at the
  top of the floor her head stays inside the figure that holds her.
- **Exits.** `at X Y` is where she walks to take the exit when its
  direction is typed; `zone x0 y0 x1 y1` is the part of the floor that
  takes it when she walks into it (by the arrows, or walking to a point
  in it). An exit with `at` and no zone gets a zone round the point. She
  comes into a room at the `at` of the exit that leads back where she
  came from, facing into the room (or at `start`). An exit with only a
  text, and a zone, says its text when she walks into it.
- **`show LAYER [if CONDITION]`** shows a named layer of the scene while
  the condition holds (an open door, the apple on the grass, the goat
  with and without her key). Every layer the room does not show is
  hidden.
- **`front LAYER X0 X1 BASE [if CONDITION]`** shows a layer, a copy of
  scenery painted after her, while her feet are above (behind) `BASE`
  and she overlaps `X0`..`X1`: the scenery then hides her, curves and
  all, by the order of the paths. In the outline looks (PHOSPHOR,
  OUTLINE) nothing is filled, so nothing could hide her: front layers are
  not shown there.

## Objects

    object tower_door "great door"
      ...
      area 296 150 364 252           # where it is in the picture
      stand 330 263                  # where she stands to handle it

- **`area`** is the object's hotspot: a click there looks at it
  (EXAMINE). People have one too.
- **`stand`**: the actions done with the hands (take, open, unlock, give,
  push, knock... and the story's own verbs, unless `far`) make her walk
  there first, then the command is carried out. Looking, reading,
  talking and asking are done from where she is. Without `stand`, she
  handles the thing from anywhere.
- `verb NAME = words` takes `far` (done from anywhere) and `alone` (no
  object needed), as well as `default:` and `minutes`.

## A scene: scenes/NAME.paths

The article path syntax (`ARTICLE.md` section 6: `M L H V C S Q T Z`,
lowercase relative, `fill <colour>`, `stroke <colour> [<width>]`,
`layer`), compiled by paint.py into a page document (`spec/page.md`).

    size 640 320
    split 96 186                     # three figures: 0-96, 96-186, 186-320
    background #94ad5a               # the page's colour behind the figures
    stroke none
    fill #2c4277 M0 34 H640 V63 H0 Z
    @circle 196 150 13               # also @ellipse cx cy rx ry
    @copy boulder ... @end           # remember lines (and keep them)
    layer goat_key                   # a named layer; story.py numbers it
    @actor                           # Ysolde's poses: layers 1 to 11
    layer boulder_front
    @paste boulder                   # the same paths again, in front of her

- **Order is depth.** Paint back to front: scenery (the figures' base),
  then the named layers that sit behind her, then `@actor`, then the
  copies of what stands in front of her. Everything before the first
  `layer` is the base, always drawn.
- **Figures.** Each figure has caps of its own (`spec/page.md`
  §page.document): 256 paths, an area and a length. Cut the scene with
  `split` so that each figure has room; a path is in every figure it
  reaches, so keep full-width shapes inside one band. A layer is clipped
  to its figure, so the lowest figure must hold her, from the feet at the
  front to the head at the back of the floor (story.py checks).
- **The lowest figure binds.** It holds her 11 poses (about 129 paths
  and 19,000 of the length cap's 65,536), the ground and every copy in
  front of her. `paint.py --costs TOP` lists its dearest paths. What
  helps: the `background` colour instead of the biggest fill; mountains
  and sky ending above the cut; outlines (a `stroke` on a filled path
  counts its length twice) only where they carry the shape; detail in
  the upper figures, which have room to spare.
- **Light.** The sun is low in the west: light from the left, long
  shadows to the right (translucent dark fills). One palette for every
  scene.

## The scenes as blobs

story.py writes each room's document to `blobs/ROOM-HASH.scene` and puts
only its BLAKE3 hash and length in `story_data.h`: the image holds the
engine and an index of scenes, not the scenes. The engine fetches them
through the content device (`spec/content.md`, get, 16 KiB a request)
into four slots in RAM: the room she is in first, then the rooms its
exits lead to, one scene at a time; a slot not wanted any more is reused.
A scene change then submits a scene already in RAM, the frame after the
room's text. A scene that has not come yet is replaced by a short notice
("Fetching the scene...") until it comes; one that cannot be fetched (no
copy on this computer, no relay with it) says so, and the story goes on
in words; it is tried again when she comes back. Without the content
device there are no scenes and no saving.

## Transcripts

`transcripts/*.txt` are played by `scripts/cargo test -p terse-cli
--test stillwind` (format at the top of `transcripts/walkthrough.txt`).
Besides Low Bell's lines: `scene ROOM` (the page shows that room's
scene), `shown ROOM_LAYER` / `hidden ROOM_LAYER` (a layer, by its name in
story_data.h), `near X Y [tolerance]` (her feet), `settle` (wait until
she stops), and `@ click X Y` / `@ down left` ... `@ up left` for the
pointer and the arrows. A command waits for her to walk where it takes
her before its reply is checked.
