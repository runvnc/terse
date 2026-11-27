# PLACEHOLDER STORY. Three rooms that exercise every feature of the
# Lamplight engine; the real story replaces these files. STORY.md
# describes the format.

game
  title Lamplight (placeholder story)
  author the engine's test story
  start jetty
  carry 5
  clock 21:30
  minutes 5
  beam 44
  step step
  score 10
  status: {room}|| {time}   bell {bell}   score {score}/{max}
  intro:
    [This is a placeholder story, written to test the Lamplight engine. The
    real story will replace it.]

    Night has fallen on the lake. Somewhere up the bank a street lamp has
    gone out, and the last ferry will not come in to a dark shore. You
    have a lantern, a little time, and the feeling that the town expects
    its bell on the hour.

    (Type HELP for how to play, HINT if you are stuck.)

colour plank 170 130 80
colour chalk 235 235 220
colour flame 255 190 60
colour lake 60 120 210

achievement chalk "You found the chalk marks" 2
achievement coat "Dressed for the weather" 1
achievement on_hour "You rang the bell on the hour" 2

counter wrong_rings 0
counter missed 0

# --- Sounds ----------------------------------------------------------------

sound toll bell C3 70, G2 90
sound step tick C6 3
sound chime chime E6 6, B6 14
sound creak swish A3 18
sound water swish C3 40
sound horn horn D2 50, rest 6, D2 70
sound thud thud C2 12

# --- Time: looking, examining and reading take none; the rest 5 minutes
# (`minutes` above), walking between places 6, ringing the bell 2.

minutes go 6

# --- Verbs this story adds ----------------------------------------------------

verb ring = ring, toll, "bang on"
  minutes 2
  default: There is nothing here to ring.
verb lift = lift, raise, pry, prise

# --- Hints, in the order they are tried -----------------------------------------

hint box
  while not carrying brass_key and locked boathouse_door
  - The boathouse door is locked. Where on the jetty might a key be kept?
  - The box on the jetty is closed. OPEN BOX.
  - TAKE BRASS KEY, then UNLOCK DOOR WITH BRASS KEY and OPEN DOOR.

hint door
  while locked boathouse_door
  - You have the brass key. The boathouse door is east of the jetty.
  - UNLOCK DOOR WITH BRASS KEY, then OPEN DOOR.

hint dark
  while in boathouse and dark
  - It is too dark to see in here. You carry a lantern.
  - LIGHT LANTERN.

hint chalk
  while visited boathouse and not chalk_seen
  - The boathouse holds more than it shows. Your lantern's beam reveals things.
  - Move the beam over the back wall with the arrow keys or the pointer, or SHINE LANTERN AT WALL.

hint board
  while chalk_seen and iron_key in nowhere
  - The chalk arrow points at the floor.
  - LIFT BOARD.

hint lamp
  while not lamp_lit
  - The lamp in the square needs oil, and its hatch is locked.
  - The iron key opens the lamp's hatch. The flask on the workbench holds oil.
  - UNLOCK LAMP WITH IRON KEY, OPEN LAMP, PUT FLASK IN LAMP, LIGHT LAMP.

# --- The clock ------------------------------------------------------------------

event hour every 60 minutes from 22:00
  say: Far off, a clock begins to strike the hour. The town will be listening for its bell.
  clear rang

event silent_hour every 60 minutes from 22:10
  if not rang
  say: The hour has passed in silence. Somewhere up the bank a shutter bangs open.
  add missed 1

event warning at 23:45
  say: Out on the lake the ferry sounds its horn: it is turning for the shore.
  sound horn

event deadline at 0:15 warn 20 5
  warning: The ferry's horn sounds again, nearer. It will reach the shore in about {left} minutes.
  lose ferry_gone:
    The ferry's light swings past the dark shore, hesitates, and turns
    away across the lake. Nobody relit the lamp in time.

picture ferry_gone
  layer lake colour lake
    line 0 150 640 150
    line 40 180 140 180
    line 260 200 380 200
    line 470 175 600 175
  layer ferry colour dim drift 60 0 600
    poly 420 140 500 140 490 148 430 148
    line 450 140 452 128 470 128 472 140
  layer light colour amber blink 20 20
    dots 461 124
