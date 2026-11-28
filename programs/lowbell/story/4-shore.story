# The far shore: the landing, the lamp hut and its lamp room, the hill
# behind it, and the old survey cairn. Every place but the cairn's open
# crest is dark: the lantern shows it where the beam falls.

room landing "The Landing"
  dark
  description:
    A shingle beach under a low black bank. Ines Marl's skiff lies drawn up
    where you left it. A path climbs from the beach toward a squat shape on
    the slope above: the lamp hut. The lake is west.
  exit west east_water
  exit north hut_outside
  exit up hut_outside
  spot 300 236
  art
    layer bank colour stone
      line 0 120 80 110 160 118 240 100 330 108 420 96 520 110 640 104
    layer hut colour stone
      line 420 96 420 60 460 40 500 60 500 96
      rect 448 24 24 16
    layer path colour dim
      line 300 256 330 200 380 150 430 100
      line 360 256 380 200 410 150 450 100
    layer shingle colour dim
      dots 60 220 90 234 130 214 170 240 220 226 420 236 470 220 520 242 580 228
    layer skiff colour plank
      line 120 210 140 222 240 222 262 206
      line 120 210 262 206
    layer water colour lake
      line 0 250 120 246
      line 520 252 640 248

on go west if in landing
  say: You push the skiff off the shingle and row out into the dark.
  minutes 8
  sound oar
  continue

room hut_outside "Outside the Lamp Hut"
  dark
  description:
    A squat stone hut with the lamp room on its roof, a round glass lantern
    house as big as a cart. The door is oak bound with iron, and locked.
    The path goes down to the landing; behind the hut a sheep track climbs
    the hill.
  note if hatch_open: The oil hatch low in the wall stands open.
  note if far_lamp_shows: Light stands in the glass on the roof.
  exit down landing
  exit south landing
  exit up hillside
  exit east hillside
  exit in lamp_room if hatch_open otherwise "The door is locked. The boy from Kell has the key, and he is over the hill."
  spot 300 240
  art
    layer hut colour stone
      line 140 246 140 120 500 120 500 246
      line 120 120 520 120
    layer lantern_house colour stone
      rect 240 40 160 80
      line 240 40 320 14 400 40
      line 280 40 280 120
      line 360 40 360 120
    layer door colour plank
      rect 420 160 50 86
      line 420 200 470 200
    layer padlock colour stone
      rect 440 196 10 12
    layer lens colour dim
      circle 320 80 26
    layer lens_lit colour flame flicker if far_lamp_shows
      circle 320 80 26
      line 346 80 400 80
      line 240 80 294 80
    layer ground colour dim
      line 0 246 640 246
    layer stains colour chalk reveal hatch_seen "The beam runs low along the wall and finds dark stains on the stone, years of spilt oil, running down to a square lip set into the masonry: an oil hatch, the size of a barrel's end, where the keeper's oil was passed in." at 220 216
      line 190 180 196 210 192 232
      line 214 176 218 204 214 226
      rect 200 222 44 24
      line 200 222 244 222

object hut_door "door"
  nouns door padlock lock
  adjectives oak iron hut
  in hut_outside
  scenery
  at 444 200
  description: Oak, bound with iron, with a padlock as big as your fist. The keeper's key fits it, and the keeper is over the hill.

object hut "lamp hut"
  nouns hut house glass wall walls stone masonry stains
  adjectives lamp squat stone round
  in hut_outside
  scenery
  at 220 216
  description: Dry stone, mortared, older than the town's quay. The lantern house on its roof is dark glass.

object hatch "oil hatch"
  nouns hatch lip
  adjectives oil square
  in nowhere
  scenery
  at 222 234
  description: A square wooden hatch set low in the wall, for passing in oil. It has a ring to pull and no lock.
  figure
    layer hatch colour plank
      rect -22 -12 44 24
      circle 0 0 3

event hatch_found when hatch_seen
  move hatch to hut_outside

on unlock hut_door
  say: You have no key. The boy from Kell has it.

on open hut_door
  say: It is locked.

on attack hut_door
  say: Oak and iron. You'd break your shoulder first.

on open hatch if hatch_open
  say: It is open.

on open hatch
  say: You pull the ring. The hatch swings out on a leather hinge, and the smell of lamp oil comes with it. It is a tight fit, but you could crawl in.
  set hatch_open

on pull hatch
  try open hatch

on climb hatch if hatch_open
  go lamp_room

on climb hatch
  try open hatch

# --- The lamp room ---------------------------------------------------------------

room lamp_room "The Lamp Room"
  dark
  description:
    You come up through the floor into the lamp room, a ring of glass with
    the lamp in the middle of it. The lamp is whole: brass, polished, its
    reservoir full. On a shelf by the stair lies the keeper's log.
  note if not wick_fitted: The wick holder is empty.
  note if not cover_off: Someone has tied a canvas cover over the great lens with a careful knot.
  note if on far_lamp and not cover_off: The flame burns behind the canvas. From the water nothing shows.
  note if far_lamp_shows: The lamp burns, and the lens throws its light out across the lake.
  note if rings_seen: Oil-can rings cover the boards by the lamp, one inside another.
  exit out hut_outside
  exit down hut_outside
  spot 160 238
  spot 480 238
  art
    layer glass colour stone
      line 40 40 600 40
      line 40 230 600 230
      line 40 40 40 230
      line 600 40 600 230
      line 180 40 180 230
      line 320 40 320 60
      line 460 40 460 230
    layer lamp colour bronze
      rect 280 150 80 50
      line 290 200 280 230
      line 350 200 360 230
      rect 304 120 32 30
    layer lens colour stone
      circle 320 100 36
      circle 320 100 20
    layer cover colour sail if not cover_off
      line 284 70 356 70 362 130 278 130 284 70
      line 284 100 356 100
    layer knot colour plank if not cover_off
      circle 320 100 4
      line 316 104 310 112
      line 324 104 330 112
    layer flame colour flame flicker if on far_lamp
      line 320 132 316 124 320 112 324 124 320 132
    layer beam colour amber if far_lamp_shows
      line 356 100 600 80
      line 356 100 600 120
      line 284 100 40 80
      line 284 100 40 120
    layer shelf colour plank
      line 500 150 580 150
    layer log colour sail
      rect 520 138 34 12
    layer rings colour chalk reveal rings_seen "On the boards beside the lamp, rings worn into the wood one inside another, each the size of an oil can's foot." at 230 220
      circle 230 220 10
      circle 230 220 14
      circle 232 221 18
      circle 228 219 22

object far_lamp "Far Lamp"
  nouns lamp holder reservoir brass light
  adjectives far great brass wick
  proper
  article the
  in lamp_room
  scenery
  light
  switchable
  at 320 150
  description:
    The lamp that every ferryman on the lake steers by: a brass oil lamp
    with a reservoir that holds a night's oil, standing in a ring of glass
    behind a lens as wide as a cartwheel. It has been cleaned and filled.
    Only the wick is gone.

object cover "canvas cover"
  nouns cover canvas knot sack
  adjectives canvas careful
  in lamp_room
  scenery
  at 320 100
  description:
    Sailcloth, tied over the lens with a knot you have seen tonight: two
    loops, crossed and tucked under. The same knot was on Tolly's parcel.

object log "keeper's log"
  nouns log book logbook entries soundings
  adjectives keeper keeper's lamp
  in lamp_room
  scenery
  at 537 144
  description: A long ledger bound in black cloth. The last few pages are in a small square hand you know from the chandlery's slate.
  readable:
    The last entries, in Ines Marl's hand:
    |
    |Apr 3. Slide on the south shore after the rains.
    |Jun 10. Teeth further out than they were. Sounded the line: 4 fm.
    |Aug 22. Line at 3 fm. Water still high.
    |Sep 14. Line at 2 fm. Told Oake.
    |Oct 9. Line at 1 fm 1 ft. Told Oake again. He says the lamp has
    |       stood two hundred years.
    |Nov 20. Line at 5 ft. Ferry draws 6.
    |Nov 26. Covered the glass. Oake has wicks.

on read log
  continue

after read log
  achieve log

on take log
  say: It belongs here, on its shelf, with the lamp.

on put_in wick far_lamp if wick_fitted
  say: There is a wick in it already.

on put_in wick far_lamp
  say: You feed the new wick into the holder and turn the screw until it stands clear by a thumb's width. It fits as if it were cut for this lamp, which it was.
  set wick_fitted
  move wick to nowhere
  minutes 2

on fit wick
  try put_in wick far_lamp

on put_on wick far_lamp
  try put_in wick far_lamp

on turn_on far_lamp if on far_lamp
  say: It is burning.

on turn_on far_lamp if not wick_fitted
  say: There is no wick in the holder. The lamp would only burn its oil off the brass.

on turn_on far_lamp if cover_off
  say: You strike a match and cup it to the new wick. It catches, blue, then yellow, then the whole lens fills with light and throws it out over the lake.
  make far_lamp on
  set far_lamp_shows
  achieve lamp_relit
  sound chime

on turn_on far_lamp
  say: You strike a match and cup it to the new wick. It catches and burns up bright behind the canvas. From the water, nothing would show.
  make far_lamp on

on untie cover if cover_off
  say: It is off.

on untie cover if on far_lamp
  say: You pick at the knot until it gives, and pull the canvas off the lens. Light pours out of the glass across the water.
  set cover_off
  set far_lamp_shows
  move cover to nowhere
  achieve lamp_relit

on untie cover
  say: You pick at the knot until it gives, and pull the canvas off the lens.
  set cover_off
  move cover to nowhere

on take cover
  try untie cover

on remove cover
  try untie cover

on open cover
  try untie cover

on pull cover
  try untie cover

on attack cover
  try untie cover

on turn_off far_lamp if on far_lamp
  say: You turn the wick down until the flame goes out.
  make far_lamp off
  clear far_lamp_shows

on take far_lamp
  say: It is bolted to its stand, and it weighs as much as you do.

# --- The hillside ------------------------------------------------------------------

room hillside "The Hillside"
  dark
  description:
    A steep sheep track between boulders, climbing behind the lamp hut. The
    roof of the lantern house is below you now. The track goes on up to the
    crest.
  exit down hut_outside
  exit up cairn
  exit north cairn
  spot 300 240
  art
    layer slope colour stone
      line 0 246 200 190 360 120 520 60 640 30
    layer boulders colour stone
      circle 120 210 26
      circle 260 170 20
      circle 420 110 24
      circle 560 64 16
    layer track colour dim
      line 300 256 260 210 340 150 460 90 560 40
    layer roof colour stone
      line 40 246 80 226 140 246
    layer arrows colour chalk reveal arrows_seen "Fresh chalk on the boulders, an arrow on each, pointing up the track." at 260 170
      line 104 210 132 210
      line 124 202 132 210 124 218
      line 248 168 272 162
      line 262 156 272 162 266 172
      line 410 108 432 100
      line 422 94 432 100 428 110

object boulders "boulders"
  nouns boulders boulder rocks rock chalk arrows arrow
  plural
  in hillside
  scenery
  at 260 170
  description: Grey boulders, furred with lichen.

# --- The cairn ---------------------------------------------------------------------

room cairn "The Cairn"
  description:
    The crest of the hill, and on it a cairn of piled stones taller than
    you, built by surveyors long ago. On its west face is a niche, a hollow
    the size of a bread oven. In front of the cairn a flat stone lies like
    a table. Below, the lamp hut's roof; beyond it, a mile of black water,
    and the lights of Low Bell.
  exit down hillside
  exit south hillside
  spot 240 240
  spot 420 240
  art
    layer cairn colour stone
      line 440 246 470 130 500 100 540 104 570 140 600 246
      line 470 170 590 170
      line 456 210 598 210
    layer niche colour stone
      rect 494 140 30 24
    layer soot colour dim
      line 498 142 520 142
      line 500 146 518 146
    layer lit_niche colour flame flicker if lantern_in_niche
      line 509 160 506 154 509 146 512 154 509 160
    layer stars colour white
      dots 40 30 110 18 190 40 260 22 330 36 400 16 600 30 620 70 220 70
    layer table colour stone
      line 80 230 120 200 330 200 370 230 80 230
    layer town colour stone
      line 40 96 80 92 120 94 160 90 200 94
      line 118 92 118 80 122 76 126 80 126 92
    layer town_lights colour flame flicker
      dots 60 94 100 93 150 91 180 93
    layer lake colour lake
      line 0 104 300 110
      line 40 130 160 128
      line 180 150 300 148
    layer roof colour stone
      line 260 150 300 140 340 150
    layer roof_light colour flame flicker if far_lamp_shows
      circle 300 140 3
      line 290 140 270 138
      line 310 140 330 138
    layer bearing colour chalk reveal bearing_seen "Chalk on the flat stone, sharp and recent: a single straight line, ruled with care, and at the end of it a small drawn tower. You sight along it. It runs over the lamp hut's roof, over the Lamp, across the black water, and finishes on the tower of Low Bell." at 220 214
      line 340 216 148 210
      line 140 214 140 200 144 194 148 200 148 214 140 214

object niche "niche"
  nouns niche hollow oven hole soot
  adjectives sooty soot black
  in cairn
  scenery
  container
  open
  at 509 152
  description:
    A hollow in the cairn's west face, roofed with a slab, the inside black
    with old soot. Something burned in here once, many nights over.

object flat_stone "flat stone"
  nouns stone table slab chalk line
  adjectives flat
  in cairn
  scenery
  at 220 214
  description: A flat slab of grey rock, waist high.

object cairn_stones "cairn"
  nouns cairn stones pile
  adjectives survey
  in cairn
  scenery
  at 520 180
  description: Piled stones, fitted without mortar. Surveyors built it as a mark.

on sight flat_stone if bearing_seen
  say: Your eye runs along the chalk: over the hut roof, over the Lamp, a mile across the water, to the tower of Low Bell. From here the three are in one line.

on sight flat_stone
  say: There is nothing on it to sight along, that you can see.

on climb cairn_stones
  say: It would come down under you. Surveyors were not builders.

on put_in lantern niche if not on lantern
  say: You set the cold lantern in the niche.
  set lantern_in_niche
  move lantern to niche

on put_in lantern niche if far_lamp_shows and here ines_cairn
  say:
    You set your lantern in the niche. The beam lies out over the water.
    You step down to the flat stone and look back along the chalk line:
    the lantern above, the Lamp below it, and a mile off, the tower.
    |
    Ines Marl looks from one light to the other. "Two," she says.
  set lantern_in_niche
  set said_two
  move lantern to niche

on put_in lantern niche if far_lamp_shows
  say:
    You set your lantern in the niche. Its light goes out over the water.
    You step down to the flat stone and look back along the chalk line:
    the lantern above, the Lamp below it, and a mile off, the tower.
  set lantern_in_niche
  move lantern to niche

on put_in lantern niche
  say: You set your lantern in the niche. From the water it would be one small light on a dark hill.
  set lantern_in_niche
  move lantern to niche

after take lantern if lantern_in_niche
  clear lantern_in_niche

on drop lantern if in cairn
  say: (in the niche, where it can be seen)
  try put_in lantern niche

npc ines_cairn "Ines Marl"
  nouns ines marl woman chandler
  proper
  in nowhere
  at 360 196
  here: Ines Marl is sitting on the edge of the flat stone, getting her breath.
  arrives:
    Stones shift on the track below. Ines Marl comes up over the crest with
    a hooded lantern, breathing hard, and sits down on the edge of the flat
    stone. She came the long way round, by the deep water. She looks at the
    chalk line, and then at you. "It's your bell they believe," she says.
  description: She has a hooded lantern and her jacket buttoned to the throat. She looks older out here.
  talk: "You found the line, then," she says.
  topic lamp, why, how, out, cover, wick, reason:
    She looks at the lamp hut below. "I told Oake in September, and again
    in October. He has a contract and a chart. A dark lamp was the last
    thing I had to say it with."
  topic you, me, ringer, bell, tower:
    "They'll believe what it says at eleven," she says. "They always have."
  topic oake, harbourmaster:
    "I said he could count. I didn't say he could hear."
  topic teeth, gravel, bar:
    "Six fathoms on the chart. Two foot under your oar."
  topic tolly, ferry, midnight:
    "He'll come round the point at midnight and steer for whatever he sees."
  topic two, lights, range, kell, niche:
    She looks along the chalk line, down at the hut, then at your lantern. She doesn't say anything.
  topic chalk, line, survey, bearing:
    "That's the bearing off the church tower. The survey line. It clears the Teeth with room to spare." She shrugs. "No good to a ferryman in the dark."
  topic relight, choice, decide, what:
    "That's yours to say," she says. "I've said mine."
  unknown: She shakes her head.
  figure
    layer ines colour white
      circle 0 -34 6
      line 0 -28 0 -8
      line -12 -16 0 -24 12 -16
      line -12 0 0 -8 12 0

event ines_comes when in cairn and bearing_seen and time >= 23:00 and not ines_here
  move ines_cairn to cairn
  set ines_here

event ines_comes_late when in cairn and time >= 23:20 and not ines_here
  move ines_cairn to cairn
  set ines_here

on examine far_lamp if wick_fitted
  say: The lamp every ferryman on the lake steers by, cleaned and filled. A new wick stands in the holder.

on examine boulders if arrows_seen
  say: Grey boulders furred with lichen, each with a chalk arrow pointing up the track.

on examine flat_stone if bearing_seen
  say: A flat slab of grey rock, waist high. A chalk line runs across it from the cairn to a little drawn tower.


event bearing_found when bearing_seen
  achieve bearing

event two_late when ines_here and lantern_in_niche and far_lamp_shows and on lantern and not said_two
  say: Ines Marl looks from the Lamp below to the lantern in the cairn, and back. "Two," she says.
  set said_two
