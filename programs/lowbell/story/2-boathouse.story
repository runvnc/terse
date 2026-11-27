# PLACEHOLDER STORY: the boathouse, a dark room with a hidden clue.

room boathouse "Inside the Boathouse"
  dark
  description:
    The boathouse smells of tar and old rope. An upturned rowing boat rests
    on trestles; a workbench stands against the back wall, and a coat hangs
    from a hook by the window. The door is west.
  note if here floorboard: One floorboard near the bench sits a little proud of the others.
  exit west jetty through boathouse_door
  exit out jetty through boathouse_door
  spot 200 236
  spot 260 240
  spot 330 238
  art
    layer room colour grey
      rect 140 40 360 150
      line 140 190 40 256
      line 500 190 600 256
      line 140 40 40 0
      line 500 40 600 0
    layer window colour moon
      rect 380 66 60 40
      line 410 66 410 106
      line 380 86 440 86
    layer moonlight colour dim faint
      line 380 106 340 190
      line 440 106 430 190
    layer door colour grey
      line 60 244 60 120 100 104 100 216
    layer boat colour plank
      line 330 168 350 150 470 150 490 168
      line 340 162 480 162
      line 345 168 345 190
      line 475 168 475 190
      line 340 190 350 190
      line 470 190 480 190
    layer hook colour grey
      line 460 110 460 116 456 120
    layer chalk colour chalk reveal chalk_seen "Chalk marks leap out of the dark where the beam falls: three strokes, a bell, and an arrow pointing down at the floor." at 260 96
      line 200 70 200 96
      line 210 70 210 96
      line 220 70 220 96
      line 240 72 246 72 252 92 234 92 240 72
      line 243 92 243 96
      line 280 70 280 120
      line 270 108 280 120 290 108
      line 262 130 300 130

object wall "back wall"
  nouns wall walls planks
  adjectives back
  in boathouse
  scenery
  at 260 96
  description: Rough boards, black with age.

object bench "workbench"
  nouns workbench bench table
  adjectives work wooden
  in boathouse
  scenery
  supporter
  at 236 188
  top 0 -26
  description: A scarred workbench. Something was spilled on it long ago.
  figure
    layer bench colour plank
      line -56 -26 56 -26
      line -50 -26 -50 2
      line 50 -26 50 2
      line -56 -22 56 -22

object flask "flask of oil"
  nouns flask oil bottle
  adjectives tin lamp
  in bench
  description: A tin flask of lamp oil, nearly full.
  at 236 162
  figure
    layer flask colour cyan
      rect -6 -16 12 16
      line -3 -16 -3 -20 3 -20 3 -16

object coat "oilskin coat"
  nouns coat oilskin jacket
  adjectives oilskin yellow
  in boathouse
  wearable
  at 460 120
  initial: An oilskin coat hangs from a hook by the window.
  description: A long oilskin coat, stiff with salt. It would keep the night out.
  figure
    layer coat colour yellow
      line -2 0 -14 10 -16 50 14 50 12 10 2 0
      line 0 2 0 48

object iron_key "iron key"
  nouns key
  adjectives iron black heavy
  in nowhere
  at 300 226
  description: A heavy black iron key. Its bow is shaped like a little flame.
  figure
    layer key colour grey
      circle -7 -3 4
      line -3 -3 10 -3 10 1

object floorboard "loose floorboard"
  nouns floorboard board boards floor
  adjectives loose
  in nowhere
  fixed
  at 300 226
  description: One board is shorter than the rest, and not nailed down.
  figure
    layer board colour plank
      rect -30 -4 60 6

object boat "rowing boat"
  nouns boat rowboat hull
  adjectives rowing upturned old
  in boathouse
  scenery
  at 410 165
  description: An old rowing boat, upside down on two trestles. It has not been on the water for years.

on take boat
  say: It is far too heavy, and full of spiders.

on turn boat
  say: Righting it would take three people and a good reason.

event chalk_found when chalk_seen
  move floorboard to boathouse
  achieve chalk
  sound chime

on lift floorboard if iron_key in nowhere
  say: You work your fingers under the loose board and lift it. Beneath it, in the dust, lies an iron key.
  move iron_key to boathouse
  sound thud

on lift floorboard
  say: You lift the board again. There is nothing else under it.

on pull floorboard
  say: You'd do better to LIFT it.

on take floorboard
  say: You'd do better to LIFT it.

on search floorboard if iron_key in nowhere
  say: The board is loose. You could lift it.

after wear coat
  achieve coat

on take bench
  say: It is bolted to the wall.

on smell if in boathouse
  say: Tar, rope and lamp oil.
