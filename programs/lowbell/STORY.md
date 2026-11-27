# Writing a story for Low Bell

Low Bell runs on an engine for illustrated text adventures: the top of the
screen is a vector drawing of the place, the bottom is the text. The
story is plain text in `story/*.story`, read in name order (one file or
many: splitting by place is convenient). `story.py` compiles it into
`story_data.h`, which `build.sh` builds into the program. Nobody needs to
touch C to write or change a story.

    python3 story.py --check            check the story, print its sizes
    ./build.sh                          compile the story and build lowbell.terse
    terse-host lowbell.terse          play it
    python3 story.py --draw jetty -o jetty.png          draw a place's picture
    python3 story.py --draw boathouse --hidden --beam 260,96 -o b.svg
    python3 story.py --import-svg sketch.svg --layer roofs --colour grey
    python3 story.py --events transcripts/walkthrough.txt -o walk.events

The story in `story/` now is a three-room placeholder that exercises
every feature; the real story replaces it.

## The shape of a file

One declaration a line at the left margin; its settings indented under
it, more indented than it (spaces; a tab counts as four). `#` starts a
comment (at the start of a line, or after a space outside quotes).

Text is either on the line, after a colon or in quotes:

    description: A small square at the top of the bank.
    say "Taken."

or as a block under a line that ends with a colon, as long as you like,
with no quoting:

    description:
      Tall houses lean over the square with their shutters closed.
      The lines are joined and wrapped to the screen.

      A blank line starts a new paragraph.
      |A line starting with | keeps its own line (signs, verse).

Lines inside a block starting with `#` are comments. Text may use ASCII,
Latin-1 letters (é, ö), dashes – —, curly quotes, • and …. These
placeholders fill themselves in:

| Placeholder | Becomes |
|---|---|
| `{the}`, `{The}`, `{a}`, `{A}` | the object of the command, with its article |
| `{the2}`, `{The2}`, `{a2}` | the second object (put X in **Y**) |
| `{it}`, `{It}`, `{is}`, `{Is}` | it / them, is / are for the object |
| `{word}` | the word the player typed (in messages about words) |
| `{verb}`, `{prep}` | the verb and preposition typed |
| `{list}` | a list of objects ("a brass key and a coin") |
| `{choices}` | the choices of a "which" question |
| `{time}`, `{bell}` | the clock ("9:45 pm"), minutes to the hour ("in 15 min" or "now") |
| `{score}`, `{max}`, `{moves}`, `{room}`, `{name}` | as named; `{name}` is an achievement's title |

A text that needs more lines than the text area (12) is shown with a
`-- more --` prompt; `story.py` notes each such text, and refuses one over
60 lines.

## game

    game
      title Low Bell
      author A. Writer
      start jetty                  # the first place
      clock 21:30                  # the time when the story starts (add 24h for a 24-hour clock)
      minutes 5                    # what an action takes, unless told otherwise
      carry 5                      # most things held at once (0: no limit)
      beam 44                      # the lantern beam's radius, screen units
      score 10                     # the most points (default: the achievements' sum)
      step step                    # a sound for each walk between places
      status: {room}|| {time}   bell {bell}   score {score}/{max}
      intro:
        The text before the first place.
      help: Overrides the HELP text.

`status` is the line between the picture and the text: what is before
`||` at the left, the rest at the right.

## Time

The clock moves by what the player does. Looking, examining, reading,
the inventory, searching and the commands about the game (UNDO, SAVE,
HINT...) take no time; other actions take the game's `minutes`. Change
an action's time with a top-level line, or in a verb:

    minutes go 6                 # walking from place to place
    minutes take 1

    verb row = row, paddle
      minutes 8

A rule can set the time of the move it handles: `minutes 15`.

Nothing happens in the world on a move that takes no time: no events, no
one comes or goes. When time passes, the events whose time has come run
(see events), people speak their idle lines and move on their routes.

## Places

    room square "The Square"
      dark                          # too dark to see without a light
      description:
        A small cobbled square ...
      note if on lamp: The lamp burns bright over the square.
      exit south jetty
      exit down jetty
      exit east boathouse through boathouse_door
      exit up loft if ladder_down otherwise "The hatch is out of reach."
      exit north "The lanes beyond are for another story."
      ambient water every 420       # a sound every 420 frames while here
      spot 250 236                  # where dropped things are drawn
      spot 330 240
      art
        layer ...

- Directions: north south east west northeast northwest southeast
  southwest up down in out (and n, s, e, w, ne, nw, se, sw, u, d).
- An exit `through` a door is closed while the door is; one with `if` is
  refused with its `otherwise` text while the condition is false; one with
  only a text is no way at all.
- `note if <condition>: text` adds a sentence to the description.
- Things the player drops are drawn at the spots, in turn.

## Objects

    object brass_key "brass key"
      nouns key
      adjectives brass small yellow
      in box                       # a place, a container, player, worn, nowhere
      description: A small brass key, worn smooth.
      initial: A brass key glints between the planks.   # in the place's text until moved
      readable: The tag says BOATHOUSE.
      at 320 230                   # where it is drawn (and the beam aims)
      figure
        layer key colour yellow
          circle -6 -3 3
          line -3 -3 8 -3 8 0

Kinds, one a line: `portable` (the default), `fixed`, `scenery` (fixed
and not listed: described in the place's text), `container`, `supporter`,
`openable`, `open`, `closed`, `locked` with `key <object>`, `wearable`,
`light` (gives light when on), `switchable`, `on`, `beam` (the lantern:
its beam is drawn and reveals hidden lines), `plural`, `proper` (no
article), `transparent`, `door` (scenery and openable). `also in <room>`
puts a door in a second place. `article a|an|some|the|none` overrides
the article. `top <dx> <dy>` is where things on a supporter are drawn.

A figure's points are relative to its `at`. Until the player moves a
thing it is drawn at `at`; afterwards at the place's spots, or on its
supporter.

## People

    npc warden "warden"
      nouns warden woman
      in square
      at 300 206
      here: The warden is here, stamping her feet.
      description: A small, square woman in a long coat.
      talk: "Evening," says the warden.
      topic lamp, light, oil:
        "Went out an hour ago," she says.
      topic bell, hour: "On the hour, mind."
      unknown: The warden shrugs.
      arrives: The warden comes stamping up the path.
      leaves: The warden goes off down the path.
      idle every 3:
        The warden blows on her fingers.
        The warden glances up at the dead lamp.
      route
        22:15 jetty                # at a time (or a move number)
        22:40 square
      figure
        layer ...

TALK TO, ASK ... ABOUT and TELL ... ABOUT use the topics (any of the
words); GIVE and SHOW are for rules. People are heard (for loud actions)
in their place and the places next to it.

## Verbs

The usual verbs are built in (below). A story adds words to them or new
verbs:

    verb ring = ring, toll, "bang on"
      default: There is nothing here to ring.
      minutes 2
    verb lift = lift, raise, prise
    verb take = snatch               # one more word for a built-in verb

A new verb needs an object unless it has `alone` on a line.

Built in: look (l), examine (x, look at), inventory (i), take (get, pick
up), drop (put down), put in, put on, open, close, lock, unlock, wear
(put on), remove (take off), turn on (light, switch on), turn off (put
out, blow out), read, go, wait (z), talk to, ask about, tell about,
give to, show to, shine (point, aim: SHINE LANTERN AT WALL), search
(look in, look under), listen, smell, touch, push, pull, turn, eat,
drink, climb, jump, sleep, attack, blow, and again (g), undo, save,
restore, restart, quit, help, hint, score. Each has a default reply,
overridden with `message`.

## Rules

    on ring bell if minute < 10
      say: The bell swings; the hour, on the hour.
      set rang
      achieve on_hour
      sound toll
      loud

    on put_in flask lamp if closed lamp
      say: The lamp's hatch is shut.

    after open box if not box_scored
      score 1
      set box_scored

`on <verb> [<object> [<preposition> <object>]]` takes the place of the
built-in action; `after` runs once the action has been done. Objects are
named by their names in the story (`brass_key`); `any`, or nothing, is
any object; `on go north` handles a direction. Conditions follow `if`
on the header or on `if` lines (all must hold). The first `on` rule whose
conditions hold is used; `continue` in it lets the built-in action go on
too. Every `after` rule that holds runs.

**Conditions**, joined by `and`, each may start with `not`: a flag's name;
`in ROOM`; `carrying OBJ`; `here OBJ` (can be seen); `OBJ in PLACE`;
`open`, `closed`, `locked`, `unlocked`, `on`, `off`, `worn OBJ`;
`visited ROOM`; `dark`; `lit`; `beam` (the lantern beam is shining);
`hears NPC`; `random PERCENT`; `COUNTER op NUMBER` with `== != < <= > >=`
(counters: yours, and `moves`, `score`, `time` (as 22:30), `bell`
(minutes to the hour), `minute` (minutes past the hour)).

**Effects**, one a line: `say`; `set`, `clear`, `toggle FLAG`;
`move OBJ to PLACE`; `go ROOM`; `make OBJ open|closed|locked|unlocked|on|off`;
`exit ROOM DIR to ROOM2` / `exit ROOM DIR none`; `sound NAME`;
`score N`; `add COUNTER N`; `let COUNTER N`; `schedule EVENT in N`
(moves) and `cancel EVENT`; `win [PICTURE]: text`; `lose [PICTURE]: text`;
`loud` (runs the noise events); `continue`; `look`; `achieve NAME`;
`aim OBJ` (the beam); `minutes N`.

Flags need no declaring (`flag name set` starts one set); counters do:
`counter missed 0`. story.py warns of a flag tested but never set, or set
but never tested.

## Events

    event hour every 60 minutes from 22:00
      say: Far off, a clock strikes the hour.
    event deadline at 0:15 warn 20 5
      warning: The ferry will reach the shore in about {left} minutes.
      lose ferry_gone: The ferry turns away from the dark shore.
    event chalk_found when chalk_seen
      move floorboard to boathouse
    event drip whenever in boathouse and random 30
      say: Water drips somewhere.
    event echo timer               # started by `schedule echo in 2`
    event scolded noise            # after a loud action
      if hears warden
      say: The warden comes hurrying.
    event arrive enter square      # each time the player arrives

`at` takes a time or a move number; `every N [from M]` counts moves,
`every N minutes [from TIME]` the clock. `warn` gives warnings that many
minutes before an `at` time, with the `warning:` text (`{left}` is the
minutes). `when` runs once, as soon as its condition holds (also between
moves, as when the beam reveals something); `whenever` after every move
it holds. Other events take `if` lines.

## Pictures

The picture is 640 by 256 units, (0, 0) at the top left; points may be
halves (12.5). Each place's `art` (and each figure, and each `picture`
for an ending) is a list of layers, drawn in order:

    art
      layer roofs colour grey
        line 160 200 160 90 200 60 240 90 240 200      # a polyline
        poly 10 10 40 10 25 30                          # closed
        rect 180 110 16 20                              # x y width height
        circle 560 40 14
        ellipse 300 100 40 12
        arc 566 36 10 from 100 to 260                   # degrees, 0 east, 90 up
        dots 20 20 64 44 110 14                         # points (stars)
      layer door_open colour grey if open boathouse_door
        line 548 152 534 158 534 210 548 205
      layer flame colour flame flicker if on lamp
      layer ripples colour lake drift 8 0 240           # sways 8 units over 240 frames
      layer buoy colour red blink 20 70                 # on 20 frames, off 70
      layer chalk colour chalk reveal chalk_seen "Chalk marks leap out." at 260 96
      layer wreck hidden colour water
      layer moonlight colour dim faint

- `colour` is a name (white grey dim red green blue cyan yellow amber
  orange brown violet pink moon water, and those declared with
  `colour name r g b`) or `#rrggbb`.
- `if <condition>` shows the layer only while it holds (always last on
  the line).
- One animation each: `flicker` (a flame's brightness), `drift DX DY FRAMES`,
  `blink ON OFF`.
- `faint` draws it at a third of its colour.

**Hidden lines.** A `hidden` layer is drawn only where the lantern's beam
falls, dashed (5 units on, 3 off) and wider and brighter than the rest of
the picture. Every line of it the beam has touched is remembered (in the
game's state: saved, undone, restored with it) and drawn faint and dashed
from then on. `reveal FLAG "text"`
makes the layer hidden and also sets the flag, and says the text, when the
beam covers its point (`at X Y`, else its middle): rules and `when`
events then follow from the discovery. In a `dark` place lit only by the
lantern every layer is seen only in the beam.

The player moves the beam with the arrow keys or the pointer, or aims it
with SHINE LANTERN AT something (at the thing's `at`).

story.py checks that every point is inside the picture (drift included)
and that no place can draw more than half the display's ink cap. Draw a
place to look at it while writing:

    python3 story.py --draw jetty -o jetty.png --flag "open boathouse_door"
    python3 story.py --draw boathouse --hidden -o b.svg      # hidden dashed
    python3 story.py --draw boathouse --beam 260,96 -o b.png # as the beam shows it

`--flag` takes a flag or a condition as written (`"on lamp"`). The SVG has
a grid with the coordinates. To sketch in a drawing program, draw a
640 x 256 SVG of lines, polylines, polygons, rects, circles and paths
(straight segments, or curves which are flattened) and print it as a
layer:

    python3 story.py --import-svg sketch.svg --layer roofs --colour grey

## Other declarations

    hint lamp
      while not lamp_lit and visited square      # the puzzle is open
      - The lamp needs oil, and its hatch is locked.
      - The iron key opens the hatch.
      - UNLOCK LAMP WITH IRON KEY, OPEN LAMP, PUT FLASK IN LAMP.

HINT shows the first hint whose `while` holds, one line more each time.

    achievement chalk "You found the chalk marks" 2     # achieve chalk: points and a line
    sound toll bell C3 70, G2 90                        # instrument, note frames, ...
    picture ferry_gone                                  # art for an ending
      layer ...
    message cant_see: You see no {word} hereabouts.     # any default reply
    colour plank 170 130 80
    flag box_open set
    counter missed 0

Instruments: bell, chime, tick, swish, horn, thud, tone; notes as C4,
F#3, Bb2, or `rest`. Without the sound device the game is silent and
otherwise the same. `story.py` lists the message keys in `MESSAGES` (the
default replies, all in one voice).

## Transcripts

`transcripts/*.txt` are played by `cargo test -p terse-cli --test
lowbell`; the format is at the top of `transcripts/walkthrough.txt`:
`> command`, `< text the reply holds`, `! text it does not`,
`status text`, `vectors text`, `@ press right`. The walkthrough is the
story's solution; give each puzzle its own transcript.

## Errors

story.py names the file and line of each mistake: unknown names (with
the nearest known one), places no exit or rule leads to, objects never
placed, rules about missing things, points outside the picture, too much
ink, texts too long. The exit status is 1 on any error.
