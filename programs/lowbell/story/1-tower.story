# The tower: the bell chamber (where the story starts), the clock room
# with its old striking train, and the tower foot.

room bell_chamber "The Bell Chamber"
  description:
    The top of the tower, open to the cold on all four sides. The bell
    hangs in its oak frame, older than anything else in Low Bell, and its
    rope drops through a hole in the boards to the clock room below.

    The east window looks over the lake. Cut into its sill is the sighting
    notch the ringers use for the nine o'clock look. Through it, on the far
    shore, there should be a light.
  note if far_lamp_shows: Through the notch, on the far shore, the Far Lamp burns.
  note if not far_lamp_shows: Through the notch the far shore is one long black line.
  exit down clock_room
  exit east "It is a long way down to the churchyard."
  spot 250 238
  spot 520 238
  art
    layer frame colour plank
      line 16 22 214 22
      line 28 22 28 250
      line 200 22 200 250
      line 28 64 70 22
      line 200 64 158 22
      line 60 44 168 44
    layer bell colour bronze
      line 66 132 74 120 80 86 92 64 136 64 148 86 154 120 162 132 66 132
      line 76 124 152 124
      line 104 64 104 52 124 52 124 64
      line 114 44 114 52
    layer clapper colour bronze
      line 114 120 114 138
      dots 114 140
    layer rope colour plank
      line 184 44 184 250
      line 176 250 192 250
    layer window colour stone
      line 250 214 250 62 290 36 590 36 630 62 630 214
      line 232 214 640 214
      line 232 226 640 226
    layer notch colour stone
      line 434 214 440 206 446 214
    layer sky colour white
      dots 300 60 342 48 390 66 452 52 510 70 560 50 604 74 270 92 480 90
    layer far_shore colour dim
      line 250 140 290 130 330 134 370 122 410 128 446 118 480 126 530 116 580 124 630 120
    layer lake colour lake
      line 270 158 320 158
      line 352 170 420 170
      line 448 152 508 152
      line 540 176 604 176
      line 290 190 350 190
      line 470 196 540 196
    layer the_lamp colour flame flicker if far_lamp_shows
      dots 440 116
    layer boards colour dim
      line 0 250 640 250
    layer tally colour chalk reveal tally_seen "The beam rakes across the sill and the scratches come up out of the wood: tally marks, hundreds of them, in groups of five, and beside the notch a straight line cut toward the far shore. Notch and line together make two marks; lay your eye along both and they find the Lamp." at 440 220
      line 330 218 330 224
      line 336 218 336 224
      line 342 218 342 224
      line 348 218 348 224
      line 328 224 350 218
      line 360 218 360 224
      line 366 218 366 224
      line 372 218 372 224
      line 378 218 378 224
      line 358 224 380 218
      line 392 218 392 224
      line 398 218 398 224
      line 404 218 404 224
      line 440 222 470 222
    layer diagram colour chalk reveal frame_seen "Low on the frame post, in the slant of the beam, scratches stand out of the grain. Someone scratched a drawing here long ago: a toothed wheel, a weight on a line, a lever over the wheel, and under the lever a little upright candle. Beside it, eleven short strokes." at 110 196
      circle 70 196 16
      dots 70 176 86 188 86 206 70 216 54 206 54 188
      line 100 168 100 220
      rect 94 220 12 14
      line 60 172 130 160
      line 120 162 120 176
      rect 116 176 8 18
      line 140 180 140 186
      line 146 180 146 186
      line 152 180 152 186
      line 158 180 158 186
      line 164 180 164 186
      line 140 196 140 202
      line 146 196 146 202
      line 152 196 152 202
      line 158 196 158 202
      line 164 196 164 202
      line 170 196 170 202

object lantern "storm lantern"
  nouns lantern
  adjectives storm hand my
  in player
  light
  switchable
  beam
  description:
    Your own storm lantern, the one that came with the tower, with a
    polished reflector behind the flame. Lit, it throws a low, narrow beam
    that rakes across a surface and shows every scratch in it.
  at 320 230
  figure
    layer body colour amber
      rect -5 -14 10 12
      line -5 -2 5 -2
      arc 0 -14 5 from 0 to 180
    layer flame colour flame flicker if on lantern
      line 0 -6 -2 -9 0 -13 2 -9 0 -6

object matches "box of matches"
  nouns matches box match
  adjectives safety
  in player
  description: A box of matches, about half full.

object bell "bell"
  nouns bell
  adjectives great bronze town
  in bell_chamber
  scenery
  at 114 98
  description:
    Bronze, green at the lip, cast before anyone now living was born. Its
    voice carries to every house in Low Bell and, on a still night, right
    across the lake.

object rope "bell rope"
  nouns rope
  adjectives bell
  in bell_chamber
  scenery
  at 184 150
  description: A thick rope, polished by eleven years of your hands and a great many before that.

object frame "bell frame"
  nouns frame post posts beam beams oak
  adjectives bell oak old
  in bell_chamber
  scenery
  at 110 196
  description:
    Oak beams pegged together with oak, black with age. The posts are
    scored all over: rope burns, initials, dates.

object notch "sighting notch"
  nouns notch sill window
  adjectives sighting east
  in bell_chamber
  scenery
  at 440 214
  description:
    A V cut into the sill. Put your eye to it and it points straight at
    the Far Lamp. Tonight it points at nothing.

object distant_shore "far shore"
  nouns shore lamp light lake water
  adjectives far east distant
  in bell_chamber
  scenery
  at 440 130
  description:
    A mile of black water, then the far shore, a low line against the
    stars. Where the Lamp should be there is nothing at all.

object peal "dark peal"
  nouns peal
  adjectives dark
  in bell_chamber
  scenery
  at 184 150
  description: Three falling notes, over and over. You have never had to ring it.

object oil_can "oil can"
  nouns can oil dreg
  adjectives oil ringers ringer's
  in bell_chamber
  at 520 238
  initial: The ringers' oil can stands by the window.
  description: A battered can of lamp oil. Shaken, it has a dreg left in it, enough for one filling.
  figure
    layer can colour stone
      rect -8 -16 16 16
      line 4 -16 12 -24

on fill lantern if lantern_filled
  say: It is full.

on fill lantern if not here oil_can and not carrying oil_can
  say: You have no oil to fill it with.

on fill lantern
  say: You tip the can into the lantern's well until the last of the dreg is gone. It will burn the night.
  set lantern_filled

on put_in oil_can lantern
  try fill lantern

on fill oil_can
  say: There is no more oil in the tower.

on turn_on lantern if not lantern_filled
  say: You strike a match and hold it to the wick. The wick is a black stub; there's no oil to draw.

on turn_on lantern if on lantern
  say: It is already lit.

on turn_on lantern
  say: You strike a match and light the lantern. The flame steadies behind its glass, and the beam lays a bright oval on whatever it is pointed at.
  make lantern on

on take oil_can if lantern_filled
  say: It is empty now. You leave it where the ringers keep it.

on ring peal
  sound peal
  lose dark_shore:
    You take the rope and ring the dark peal: three falling notes, over and
    over, until doors open along the lane. Harbourmaster Oake comes up the
    tower in his shirt. He looks east, says nothing for a while, and sends a
    boy down to tell Tolly the ferry stays.

    The town's winter waits for spring. Nobody learns why the Lamp went
    out, and nobody goes onto the Teeth.

on ring bell if not in bell_chamber
  say: The bell is at the top of the tower.

on ring bell if time >= 22:00 and time < 22:10 and not rang_ten
  say:
    You take the rope and lean into it. The bell swings up, comes down, and
    speaks over the roofs and across the water.
    |
    |Ten.
    |
    Down in the lane, doors close.
  set rang_ten
  achieve ten
  sound toll
  loud

on ring bell if time >= 23:00 and time < 23:10 and not rang_eleven and not rig_armed
  say: You ring eleven. The town settles. The far shore stays dark.
  set rang_eleven
  sound toll
  loud

on ring bell if rig_armed and rang_ten
  say: Leave it. The candle has the next hour in hand.

on ring bell
  say: You put your hands on the rope and stop. Between hours the bell stays still. One stray stroke and every window on the lane would open.

on pull rope
  try ring bell

on climb rope
  say: It goes down to the clock room, and so does the ladder, which is quicker.

on sight notch
  say: You put your eye to the notch. The line runs over the black water to the far shore and finds nothing there.

on take bell
  say: It weighs more than the tower does, you sometimes think.

# --- The clock room ----------------------------------------------------------

room clock_room "The Clock Room"
  dark
  description:
    A cramped wooden room halfway up the tower, full of a clock nobody has
    wound in fifty years. Its striking train fills one wall: a great
    toothed wheel, a barrel, and a crank, with an iron weight hanging from
    the barrel on a chain. Over the wheel a release lever juts out, and
    under the lever is a small tin case with two sockets, the kind a candle
    stands in. The bell rope passes through, and the ladder goes up and
    down.
  note if not cord_tied: A length of old grey string hangs from the lever's end.
  note if cord_tied: New cord runs from the lever down past the candle case.
  note if wound: The weight hangs high, held by the pawl.
  note if candle_set and not candles_lit: An hour candle stands in the tin case, unlit.
  note if candles_lit and not candle2_lit: An hour candle burns in the tin case, its flame a finger below the cord.
  note if candle2_lit: Two hour candles burn in the tin case, one a band lower than the other.
  exit up bell_chamber
  exit down tower_foot
  spot 300 238
  spot 420 238
  art
    layer walls colour plank
      line 0 30 640 30
      line 0 246 640 246
      line 40 30 40 246
      line 600 30 600 246
    layer wheel colour bronze
      circle 220 130 64
      circle 220 130 10
      dots 220 62 268 82 288 130 268 178 220 198 172 178 152 130 172 82
      line 220 130 260 98
      line 220 130 180 162
    layer barrel colour plank
      rect 300 104 70 40
      line 300 124 370 124
    layer crank colour stone
      line 370 124 392 124 392 150
      circle 392 154 4
    layer chain colour stone if not wound
      line 336 144 336 196
    layer weight colour stone if not wound
      rect 320 196 32 44
    layer chain_up colour stone if wound
      line 336 144 336 152
    layer weight_high colour stone if wound
      rect 320 152 32 44
    layer lever colour stone
      line 210 66 300 52 310 54
      circle 210 66 3
    layer case colour stone
      rect 290 76 34 20
    layer old_cord colour dim if not cord_tied
      line 304 54 300 70 310 90 304 110
    layer new_cord colour plank if cord_tied
      line 306 54 306 96
    layer candle colour white if candle_set
      rect 298 64 6 14
    layer candle2 colour white if candle2_set
      rect 312 70 6 10
    layer flame colour flame flicker if candles_lit
      line 301 64 299 60 301 56 303 60 301 64
    layer flame2 colour flame flicker if candle2_lit
      line 315 70 313 66 315 62 317 66 315 70
    layer ladder colour plank
      line 500 30 500 246
      line 540 30 540 246
      line 500 60 540 60
      line 500 100 540 100
      line 500 140 540 140
      line 500 180 540 180
      line 500 220 540 220
    layer rope colour plank
      line 460 30 460 246

object train "striking train"
  nouns train clock wheel wheels barrel mechanism teeth
  adjectives striking great toothed old
  in clock_room
  scenery
  at 220 130
  description:
    The clock's striking side, without the clock: the hands and dial went
    long ago. When the lever lifts, the weight falls, the wheel turns, and
    the hammer arm is lifted and dropped on the bell once for each tooth
    until the count is done. It was built to strike the hours by itself.

object weight "weight"
  nouns weight chain
  adjectives iron
  in clock_room
  scenery
  at 336 190
  description: A block of iron the size of a loaf, on a chain round the barrel.

object crank "crank"
  nouns crank handle pawl
  in clock_room
  scenery
  at 392 150
  description: An iron crank on the barrel's end, and a pawl that clicks into the teeth to hold the weight up.

object lever "release lever"
  nouns lever arm
  adjectives release
  in clock_room
  scenery
  at 260 60
  description:
    An iron arm over the wheel. Pull it down and the train runs.

object old_cord "old string"
  nouns string threads
  adjectives old rotten grey
  in clock_room
  scenery
  at 304 80
  description: The string that hangs from the lever has been there fifty years. It is grey and comes apart at a touch.

object case "candle case"
  nouns case tin sockets socket
  adjectives candle tin small
  in clock_room
  scenery
  container
  open
  at 306 86
  description:
    A small tin case fixed under the lever, with two sockets for candles
    and a slot cut in its lid.

on take old_cord
  say: It comes apart in your fingers.

on tie cord to lever if cord_tied
  say: It is tied, and the knot is good.

on tie cord to lever
  say:
    You pick the old string off the lever. It falls apart in grey threads.
    You tie the new cord to the lever's end, lift the arm, and run the cord
    down through the slot in the candle case to a staple on the wall, where
    you make it fast. A second turn goes round the catch lower down, through
    the slot again, so the lever will be held a second time after it has
    dropped once. The lever stays up, held by the cord.
  set cord_tied
  move cord to nowhere
  minutes 2

on tie cord if in clock_room
  try tie cord lever

on tie cord to case
  try tie cord lever

on put_in cord case
  try tie cord lever

on put_on cord lever
  try tie cord lever

on turn crank if wound
  say: The weight is as high as it goes.

on turn crank
  say: You lean on the crank. The barrel turns, the chain winds, and the weight climbs, click by click, until the pawl holds it high under the barrel.
  set wound
  minutes 2

on turn weight
  try turn crank

on pull weight
  try turn crank

on push weight
  try turn crank

on turn train
  try turn crank

on pull lever
  say: If you pull it now the bell strikes. That is what the cord is for.

on put_in candles case if candle_set and candle2_set
  say: Both sockets are full.

on put_in candles case if candle_set
  say: You stand the second hour candle in the other socket. Its top stands a band lower, under the second turn of cord: it will reach it an hour later.
  set candle2_set

on put_in candles case
  say: You stand an hour candle in a socket of the tin case, set so the cord crosses it where the flame will be at eleven.
  set candle_set

on turn_on candles if not candle_set
  say: Set it in the case first. A candle burning in your hand strikes nothing.

on turn_on candles if candles_lit and candle2_set and not candle2_lit
  say: You strike a match and light the second candle.
  set candle2_lit

on turn_on candles if candles_lit
  say: It is burning.

on turn_on candles if candle2_set
  say: You strike a match and light both candles. Two small flames stand under the cord, one a band lower than the other.
  set candles_lit
  set candle2_lit

on turn_on candles
  say: You strike a match and light the candle. The flame stands up straight in the still air, a finger below the cord.
  set candles_lit

on turn_off candles if candles_lit
  say: You pinch it out. The cord hangs unburnt.
  clear candles_lit
  clear candle2_lit
  clear rig_armed

event armed when cord_tied and wound and candle_set and candles_lit
  say:
    You check it all once more: weight up, lever held by the cord, the cord
    across the candle where the flame will be at eleven. When it burns
    through, the lever drops and the train strikes. It is set.
  set rig_armed
  achieve rig
  sound chime

event armed12 when rig_armed and candle2_set and candle2_lit
  say: The second candle stands a band lower, under the second turn of cord. When it burns through, the train will strike twelve.
  set midnight_armed

# --- The tower foot ----------------------------------------------------------

room tower_foot "The Tower Foot"
  description:
    The ground floor of the tower: flagstones, a coil of spare rope, a
    ladder going up into the dark. The door stands open on Church Lane, and
    the lane's lamps throw a yellow wedge across the floor.
  exit up clock_room
  exit out church_lane
  exit south church_lane
  spot 260 236
  spot 360 240
  art
    layer walls colour stone
      line 60 20 60 246
      line 580 20 580 246
      line 0 246 640 246
    layer door colour stone
      line 380 246 380 100 420 80 460 100 460 246
    layer doorlight colour amber faint
      line 380 246 300 256
      line 460 246 520 256
    layer lane colour dim
      line 400 160 440 160
      line 392 190 448 190
    layer ladder colour plank
      line 150 20 150 246
      line 190 20 190 246
      line 150 50 190 50
      line 150 90 190 90
      line 150 130 190 130
      line 150 170 190 170
      line 150 210 190 210
    layer coil colour plank
      ellipse 270 236 30 8
      ellipse 270 230 24 6
    layer flags colour dim
      line 60 246 0 256
      line 200 246 180 256
      line 320 246 330 256

on examine frame if frame_seen
  say: Oak beams pegged together with oak, black with age. Low on one post is the scratched drawing of a wheel, a weight and a candle, with eleven strokes beside it.

on examine weight if wound
  say: A block of iron the size of a loaf. It hangs high under the barrel now, held by the pawl.

event tally_found when tally_seen
  achieve tally
