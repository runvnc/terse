# PLACEHOLDER STORY: the square, with the bell, the dead lamp and the warden.

room square "The Square"
  description:
    A small cobbled square at the top of the bank. Tall houses lean over it
    with their shutters closed. On the west side stands the bell tower,
    its rope hanging to the ground; in the middle, an iron lamp post holds
    up a lamp that has gone out. The path back down to the jetty is south.
  note if on lamp: The lamp burns bright and steady over the square.
  exit south jetty
  exit down jetty
  exit north "The lanes beyond the square are for another story."
  exit east "The lanes beyond the square are for another story."
  exit west "The tower door is bolted from inside. The rope will have to do."
  spot 250 236
  spot 330 240
  spot 470 236
  art
    layer houses colour grey
      line 160 200 160 90 200 60 240 90 240 200
      line 240 200 240 110 280 80 320 110 320 200
      line 470 200 470 100 520 70 570 100 570 200
      line 570 200 570 124 640 124
    layer windows colour dim
      rect 180 110 16 20
      rect 212 110 16 20
      rect 260 125 14 18
      rect 530 115 18 22
    layer window_lit colour amber flicker
      rect 490 115 18 22
    layer tower colour grey
      line 40 210 40 50 80 18 120 50 120 210
      line 56 100 56 62 80 54 104 62 104 100 56 100
    layer rope colour plank
      line 80 96 80 196
    layer post colour grey
      line 400 210 400 104
      line 392 210 408 210
    layer ground colour dim
      line 0 210 640 210
      line 280 256 300 210
      line 360 256 340 210
      line 120 256 200 210
      line 520 256 450 210

object bell "bell"
  nouns bell
  adjectives bronze great town
  in square
  scenery
  at 80 90
  description: A bronze bell in the open belfry, green with age. A rope hangs from it to the cobbles.
  figure
    layer bell colour orange
      line -12 0 -9 -6 -8 -16 -4 -20 4 -20 8 -16 9 -6 12 0 -12 0
      dots 0 3

object rope "bell rope"
  nouns rope
  adjectives bell
  in square
  scenery
  at 80 150
  description: A thick rope, polished by hands, running up to the bell.

object lamp "street lamp"
  nouns lamp light lantern hatch
  adjectives street iron dead
  in square
  scenery
  container
  openable
  closed
  locked
  key iron_key
  light
  switchable
  at 400 104
  description: An iron street lamp on a tall post. A little hatch in its side gives on to the reservoir; it has a keyhole.
  figure
    layer housing colour grey
      line -10 0 10 0 6 -18 -6 -18 -10 0
      line -8 -18 0 -25 8 -18
    layer hatch_open colour grey if open lamp
      line 10 0 18 -4 14 -16 6 -18
    layer flame colour flame flicker if on lamp
      line 0 -3 -3 -8 0 -15 3 -8 0 -3
      line -16 -9 -24 -9
      line 16 -9 24 -9

npc warden "warden"
  nouns warden woman keeper
  adjectives old night
  in square
  at 300 206
  here: The warden is here, stamping her feet against the cold.
  description: A small, square woman in a long coat, with a ring of keys at her belt and the look of someone who has rung a great many bells.
  talk: "Evening," says the warden. "If you've come about the lamp, you'd best be quick. The ferry won't come in to a dark shore."
  topic lamp, light, oil, wick:
    "Went out an hour ago," she says. "The hatch is locked, and I've lost the
    key. Oil's in the boathouse, if you can get in."
  topic bell, hour, rope, clock:
    "On the hour, mind. Not before, not after. The town sets its clocks by
    it, and it notices."
  topic ferry, boat:
    "Last one tonight. Comes in at midnight if it sees the lamp. If it doesn't,
    it turns round."
  topic key, keys, hatch:
    "My brother kept the spare. He hid everything under the boathouse floor,
    the old fool, and drew chalk marks so he'd remember where."
  topic boathouse, chalk, brother:
    "Dark as a pocket in there. You'll want your lantern lit."
  topic warden, herself, name:
    "Forty years I've rung that bell. Never once late."
  unknown: The warden shrugs. "Can't help you there."
  idle every 3:
    The warden blows on her fingers.
    The warden glances up at the dead lamp and shakes her head.
    The warden checks the clock on the tower, then her own watch.
  route
    22:15 jetty
    22:40 square
    23:20 jetty
  figure
    layer warden colour white
      circle 0 -46 5
      line -7 -52 7 -52
      line -4 -55 4 -55 4 -52
      line 0 -41 0 -18
      line -8 -28 0 -38 8 -28
      line -6 0 0 -18 6 0
      line -8 -14 0 -40 8 -14

on ring bell if not in square
  say: The bell is in the square.

on ring bell if bell == 0
  say: You haul on the rope. The bell swings, and its voice rolls out over the town and the water: the hour, on the hour.
  set rang
  achieve on_hour
  sound toll
  loud

on ring bell
  say: You haul on the rope. The bell booms out over the dark town, at no particular hour.
  sound toll
  set rang_wrong
  add wrong_rings 1
  loud

on pull rope
  say: (ringing the bell)
  continue

on climb rope
  say: The rope would only ring the bell, and the tower door is bolted.

event too_many_rings noise
  if rang_wrong and wrong_rings >= 2
  lose:
    Shutters bang open all round the square. "Twice!" somebody shouts. "Who
    is ringing the bell at all hours?" By the time the fuss dies down, the
    ferry has turned away from the dark shore.

event scolded noise
  if rang_wrong and hears warden
  say: The warden comes hurrying, red in the face. "On the HOUR," she hisses. "Do that again and the whole town will be out."
  clear rang_wrong

event unheard noise
  if rang_wrong
  clear rang_wrong

on take lamp
  say: It is at the top of a tall iron post.

on put_in flask lamp if closed lamp
  say: The lamp's hatch is shut.

on put_in flask lamp if lamp_oiled
  say: The reservoir is already full.

on put_in flask lamp
  say: You tip the flask into the reservoir until oil shows at the brim.
  set lamp_oiled
  score 1

on turn_on lamp if not lamp_oiled
  say: The wick is dry. It would only smoke and sputter.

on turn_on lamp if off lantern
  say: You have no flame to light it with. Your lantern would do, lit.

after turn_on lamp
  set lamp_lit
  sound chime
  score 2
  win lamp_lit:
    You hold your lantern's flame to the wick. It catches, blue, then gold,
    and the lamp throws its light across the square and down the bank to the
    water. Far out on the lake the ferry sounds its horn, and turns for the
    shore.

on give coin to warden
  say: The warden bites the coin, then pockets it. "My brother," she says, "drew chalk marks in the boathouse to show where he hid things. Shine a light on the back wall."
  move coin to warden
  score 1

on show note to warden
  say: "I wrote that," she says. "Not that anyone reads anything."

on attack warden
  say: You think better of it. She has rung bells for forty years; she has arms like a docker.

picture lamp_lit
  layer post colour grey
    line 320 250 320 120
  layer lamp colour flame flicker
    line 300 120 340 120 332 90 308 90 300 120
    line 320 112 314 102 320 94 326 102 320 112
  layer rays colour amber
    line 270 105 230 100
    line 370 105 410 100
    line 290 80 260 50
    line 350 80 380 50
    line 320 74 320 34
