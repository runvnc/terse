# The lake: three stretches of water between the reeds and the far shore.
# You row facing astern, so each picture looks back at the town.

room west_water "West Water"
  description:
    Open water, black and still. You pull, and the town slides away over
    the stern: the quay lights, the yellow window of the office, the tower
    standing up dark above the roofs. The reeds are west. Ahead of you,
    behind your back, is the far shore.
  exit west reed_bank
  exit east the_teeth
  spot 320 236
  art
    layer town colour stone
      line 120 120 180 112 220 116 260 106 330 110 380 104 440 112 520 116
      line 300 106 300 60 316 46 332 60 332 106
    layer town_lights colour flame flicker
      dots 150 114 210 112 350 108 420 108 470 112
    layer office colour amber
      dots 260 112
    layer water colour lake drift 8 0 300
      line 40 150 120 150
      line 180 166 260 166
      line 330 150 420 150
      line 470 170 560 170
      line 90 190 170 190
      line 400 196 500 196
    layer stern colour plank
      line 200 256 250 214 390 214 440 256
      line 250 214 320 200 390 214
    layer oars colour plank
      line 120 230 250 226
      line 390 226 520 230

room the_teeth "Over the Teeth"
  description:
    Halfway over. The town is small behind the stern now, and the far shore
    is a black wall over your shoulder. The water here moves oddly, folding
    over itself as if something lay just under it.
  exit west west_water
  exit east east_water
  spot 320 236
  art
    layer town colour stone
      line 200 122 240 118 280 120 320 114 360 118 400 116 440 120
      line 316 114 316 90 324 82 332 90 332 114
    layer town_lights colour flame flicker
      dots 230 118 300 117 380 116
    layer water colour lake drift 8 0 240
      line 40 150 100 150
      line 160 160 240 158
      line 330 152 410 154
      line 470 164 560 162
    layer folds colour lake drift 4 0 120
      line 100 184 130 178 160 184
      line 420 188 450 182 480 188
    layer stern colour plank
      line 200 256 250 214 390 214 440 256
      line 250 214 320 200 390 214
    layer oars colour plank
      line 120 230 250 226
      line 390 226 520 230
    layer gravel colour chalk reveal gravel_seen "You hold the lantern low over the side. Under two feet of water, pale and close enough to touch, the gravel goes by: stones the size of hens' eggs, piled in ridges, where the chart says six fathoms." at 560 216
      line 470 206 500 200 530 208 560 202 600 210
      line 480 222 510 216 540 224 570 218 610 224
      line 490 238 520 232 550 240 590 234
      dots 486 212 516 210 548 214 580 210 506 228 538 230 570 228 600 230

event oar_strikes enter the_teeth
  if not oar_struck
  say: Your oar comes down on something that grates. Stone. You lift the blade and the skiff slides on, scraping once under the keel, and floats free.
  set oar_struck
  sound grind

object teeth_water "water"
  nouns water side gravel stones teeth bar lake
  in the_teeth
  scenery
  at 560 216
  description: Black, moving oddly, folding over itself.

event gravel_found when gravel_seen
  achieve gravel

room east_water "East Water"
  description:
    Deep again. The town is a scatter of sparks behind the stern, the tower
    a dark peg among them. The far shore is close now behind you: you hear
    small waves on shingle.
  exit west the_teeth
  exit east landing
  spot 320 236
  art
    layer town colour stone
      line 240 124 280 122 320 120 360 122 400 124
      line 318 120 318 106 322 102 326 106 326 120
    layer town_lights colour flame flicker
      dots 260 123 300 121 350 121 390 123
    layer water colour lake drift 8 0 300
      line 40 154 120 154
      line 200 164 280 164
      line 360 152 440 152
      line 500 168 600 168
      line 80 196 160 196
    layer stern colour plank
      line 200 256 250 214 390 214 440 256
      line 250 214 320 200 390 214
    layer oars colour plank
      line 120 230 250 226
      line 390 226 520 230

on go east if in west_water
  say: You pull on into the dark.
  minutes 8
  sound oar
  continue

on go west if in west_water
  say: You turn the skiff and row back into the reeds.
  minutes 8
  sound oar
  continue

on go east if in the_teeth
  say: You pull clear of the folding water.
  minutes 8
  sound oar
  continue

on go west if in the_teeth
  minutes 8
  sound oar
  continue

on go east if in east_water
  say: The skiff's bow runs up on shingle with a long hiss. You step out into ankle-deep water and drag it up the beach.
  minutes 8
  sound oar
  continue

on go west if in east_water
  minutes 8
  sound oar
  continue

on row if in west_water
  say: East, to the far shore, or west, back to the reeds.

on row if in the_teeth
  say: East, to the far shore, or west, back to the town.

on row if in east_water
  say: East, to the shore, or west, back over the Teeth.

on jump if in west_water
  say: You have the oars and the night is short. Stay in the boat.

on jump if in the_teeth
  say: You have the oars and the night is short. Stay in the boat.

on jump if in east_water
  say: You have the oars and the night is short. Stay in the boat.
