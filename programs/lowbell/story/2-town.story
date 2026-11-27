# The town: Church Lane, the chandlery, the quay and the ferry, under the
# harbour office window, the net sheds and the reed bank.

room church_lane "Church Lane"
  description:
    Church Lane runs downhill from the tower to the quay between houses
    with their shutters closed for the night. Oil lamps on brackets light
    it in pools. Halfway down on the east side, a lit window and a painted
    board: the chandlery. The tower door is at the top of the lane, north;
    the quay is at the bottom, south.
  exit north tower_foot
  exit in "Which way? The tower is north, the chandlery east."
  exit east chandlery
  exit south quay
  exit down quay
  exit west "Only the churchyard that way, and nobody in it who can help."
  spot 300 240
  spot 360 236
  art
    layer houses_w colour stone
      line 0 60 60 40 120 60 120 200
      line 120 200 0 256
      line 120 90 180 80 210 100 210 180
      line 210 180 120 200
    layer houses_e colour stone
      line 640 50 570 36 500 56 500 200
      line 500 200 640 256
      line 500 96 440 84 410 104 410 176
      line 410 176 500 200
    layer shutters colour dim
      rect 30 90 20 30
      rect 140 110 16 24
      rect 590 80 20 30
    layer chandlery_window colour amber flicker
      rect 436 120 26 30
    layer board colour plank
      rect 424 100 50 12
    layer lamps colour flame flicker
      dots 120 70 500 66
    layer lamp_brackets colour stone
      line 120 74 132 74
      line 500 70 488 70
    layer cobbles colour dim
      line 210 180 230 140 250 110 270 100
      line 410 176 390 140 370 110 350 100
      line 250 220 390 220
      line 270 196 370 196
    layer tower colour stone
      line 280 100 280 30 310 10 340 30 340 100
    layer quay_glow colour lake faint
      line 230 252 410 252

room chandlery "Marl's Chandlery"
  description:
    A narrow shop that smells of tar and paraffin wax. Coils of cord hang
    from pegs and lamp glasses stand in rows on the shelves. Over the
    counter a board reads M. MARL, CHANDLER. The door is west.
  note if not candles_bought: A box of hour candles sits on the counter, each candle ringed with painted bands.
  note if board_seen: Under the paint of the board, the old words show faintly: & LAMP KEEPER.
  note if chandlery_shut: The lamp on the counter is turned low. Nobody minds the shop.
  exit west church_lane
  exit out church_lane
  spot 260 240
  spot 400 240
  art
    layer walls colour plank
      line 0 40 640 40
      line 0 246 640 246
    layer shelves colour plank
      line 60 80 300 80
      line 60 130 300 130
      line 380 80 600 80
      line 380 130 600 130
    layer glasses colour sail
      ellipse 90 70 8 10
      ellipse 120 70 8 10
      ellipse 150 70 8 10
      ellipse 180 70 8 10
      rect 420 112 14 18
      rect 450 112 14 18
      rect 480 112 14 18
    layer cords colour reed
      circle 230 106 14
      circle 262 106 14
      circle 540 60 14
    layer counter colour plank
      rect 120 170 420 20
      line 140 190 140 246
      line 520 190 520 246
    layer board colour plank
      rect 230 50 180 22
    layer board_words colour dim
      line 244 60 300 60
      line 318 60 396 60
    layer painted_out colour chalk reveal board_seen "Under the paint of the board, in the sidelong light, the old letters show through: M. MARL, CHANDLER & LAMP KEEPER. The last three words were painted over, not quite thickly enough." at 320 64
      line 244 66 270 66
      line 280 66 400 66
    layer candles colour white
      rect 420 156 40 14
      line 428 156 428 148
      line 440 156 440 148
      line 452 156 452 148
    layer counter_lamp colour flame flicker
      dots 180 160

object shop_board "board"
  nouns board sign letters paint
  adjectives painted
  in chandlery
  scenery
  at 320 62
  description: M. MARL, CHANDLER, in black on cream. The paint after CHANDLER is fresher than the rest.

object shelf_cord "coils of cord"
  nouns cord coils coil string line
  adjectives hemp tarred
  plural
  in chandlery
  scenery
  at 246 106
  description: Hemp cord, tarred and untarred, a fathom to a hank, on pegs behind the counter.

object shelf_candles "box of hour candles"
  nouns candles candle box
  adjectives hour
  in chandlery
  scenery
  at 440 156
  description: Fat yellow candles, each painted with twelve bands. One band burns in an hour, more or less.

object cord "length of cord"
  nouns cord string line length
  adjectives tarred new
  in nowhere
  description: A fathom of new tarred cord. It will take a knot and keep it, and it will burn through in a flame.

object candles "hour candles"
  nouns candles candle
  adjectives hour two
  plural
  in nowhere
  description: Two hour candles, ringed with twelve painted bands from the top down. One band burns in an hour.

object wick "lamp wick"
  nouns wick
  adjectives lamp big wide new
  in nowhere
  description: A wide flat wick, as long as your forearm and stiff with newness. It is made for a big lamp, bigger than any in town.

object parcel "parcel"
  nouns parcel package bundle knot
  adjectives small brown
  in nowhere
  description:
    Something heavy in brown paper, tied with a knot you don't know: two
    loops, crossed and tucked under. It has TOLLY written on it.

npc ines "Ines Marl"
  nouns ines marl chandler woman
  adjectives old
  proper
  in chandlery
  at 330 168
  here: Ines Marl is behind the counter, winding cord onto a card.
  description:
    A small woman with big hands, past sixty, in a man's jacket with the
    sleeves turned back. She kept the Far Lamp for thirty years before the
    harbour board gave the job to a boy from Kell. She does not look up
    when you come in, and she seems to have expected you.
  talk: "Ringer," she says. "You're out late for the rope."
  topic lamp, light, far, shore, out:
    "Is it." She goes on winding.
  topic wick, wicks:
    "One that fits," she says. "That's all I'll say about wicks here."
  topic why, how, cause, reason, happened:
    "Ask me that at the lamp."
  topic boat, skiff, skiffs, row, rowing, crossing, across:
    "Chained till spring. Oake has a key." She goes back to her cord.
    "There's a boat in the reeds. Mine."
  topic candle, candles, hour:
    "Hour candles. Twelve bands, a band an hour."
  topic cord, string:
    "Tarred hemp. It burns clean and it doesn't stretch."
  topic oake, harbourmaster, harbour, board:
    "Oake has a contract with Kell and ninety tons to move before the ice." She ties off the card. "He's not a bad man. He can count, is all."
  topic tolly, ferry, ferryman:
    "Tolly steers by what he sees. Best ferryman on the lake."
  topic teeth, gravel, bar, slide:
    "The Teeth have been there since before the town." She looks at you a moment. "Things move, though."
  topic keeper, kell, job, lampkeeper:
    "Thirty years. Then the board wanted somebody younger." She shrugs. "The boy lights it at dusk and walks back over the hill to his supper. He doesn't sit up with it."
  topic parcel, favour, price, pay:
    "For Tolly. On your way down. That's the price."
  topic bell, ten, eleven, tower:
    "You'll have to be in two places at once, then." She says it as if it were a thing people do.
  topic ines, herself, marl:
    "I sell rope."
  unknown: Ines goes on winding.
  figure
    layer ines colour white
      circle 0 -50 6
      line 0 -44 0 -4
      line -14 -22 0 -34 14 -22

event ines_gone when launched
  move ines to nowhere
  set chandlery_shut

object counter "counter"
  nouns counter wood
  in chandlery
  scenery
  supporter
  at 330 170
  top 0 -8
  description: A long counter, scarred by forty years of knives cutting cord.

event wick_ready enter chandlery
  if not gave_wick
  say:
    Ines doesn't look up. "Lamp's out, is it," she says, which is not quite
    a question. She reaches under the counter and puts a wide flat wick on
    the wood, then a parcel in brown paper beside it. "That's a wick that
    fits. And you'll take that to Tolly on your way down. He's at the ferry."
  move wick to counter
  move parcel to counter
  set gave_wick

on take wick if parcel in counter
  say: You take the wick. "And the parcel," Ines says, still not looking up. You take that too.
  move wick to player
  move parcel to player

on buy wick
  try take wick

on buy shelf_cord if not here ines
  say: There's nobody to pay.

on buy shelf_cord
  say: Ines measures off a fathom against her arm, cuts it, and coils it into your hand. "On the tower's slate," she says, and writes it down.
  move cord to player
  move shelf_cord to nowhere

on buy shelf_candles if not here ines
  say: There's nobody to pay.

on buy shelf_candles
  say: You ask for one. Ines puts two in your hand. "A long night for one pair of hands," she says, and writes it on the tower's slate.
  move candles to player
  move shelf_candles to nowhere
  set candles_bought

on take shelf_cord
  try buy shelf_cord

on take shelf_candles
  try buy shelf_candles

on give parcel to ines
  say: "Tolly," she says, tapping the name on it. "At the ferry."

on show parcel to ines
  try give parcel ines

# --- The quay --------------------------------------------------------------------

room quay "The Quay"
  description:
    The stone quay at the foot of Church Lane. The ferry is moored along it,
    broad and low, lit fore and aft, and men are loading the last of the
    winter goods. On the landward side the harbour office has one window
    lit, over the water. The town skiffs lie hauled up and chained in a
    row. To the east, a long point of land shuts off the view across the
    lake: from down here you could not see the Far Lamp even on a good
    night.
  note if not marks_seen: The quay wall drops into the black water.
  exit north church_lane
  exit up church_lane
  exit east ferry
  exit in ferry
  exit west under_window
  exit south net_sheds
  ambient water every 480
  spot 250 240
  spot 330 244
  art
    layer quay colour stone
      line 0 190 640 190
      line 0 202 640 202
      line 60 190 60 202
      line 180 190 180 202
      line 300 190 300 202
      line 420 190 420 202
    layer water colour lake drift 6 0 300
      line 20 222 80 222
      line 140 234 200 234
      line 260 216 330 216
      line 380 240 450 240
    layer point colour dim
      line 300 120 360 110 430 104 500 112 560 120 640 118
    layer office colour stone
      line 20 190 20 80 100 50 180 80 180 190
    layer office_window colour amber flicker
      rect 70 100 30 26
    layer ferry colour plank
      line 372 146 640 146
      line 372 146 390 186 640 186
      line 380 166 640 166
    layer wheelhouse colour plank
      rect 560 106 56 40
      line 556 106 588 94 620 106
    layer ferry_lights colour flame flicker
      dots 380 140 630 96
    layer casks colour plank
      circle 262 178 12
      circle 288 178 12
      circle 275 156 12
    layer skiffs colour dim
      line 100 186 130 176 170 176 176 186
      line 140 186 170 178 200 178 206 186
    layer chain colour stone
      line 110 184 220 186
    layer water_marks colour chalk reveal marks_seen "Held low, the beam picks out the old water marks scratched into it, years of them, each with a date. Tonight's water is far below the lowest." at 330 196
      line 310 192 350 192
      line 316 196 344 196
      line 320 200 340 200

object quay_wall "quay wall"
  nouns wall marks quay stones stone
  adjectives quay water
  in quay
  scenery
  at 330 196
  description: Squared stone, green below the waterline. The water is a long way down tonight.

object town_skiffs "town skiffs"
  nouns skiffs skiff boats boat chain chains padlock
  adjectives town
  plural
  in quay
  scenery
  at 150 182
  description: Six skiffs, hauled up and upturned, chained through their bow rings to a ring in the quay. The padlock is the harbour board's.

object casks "casks"
  nouns casks cask barrels barrel
  in quay
  scenery
  at 275 170
  description: Beer for Kell, going on the ferry. Every one Mott rolls sets the planks booming.

object point "point"
  nouns point headland land east
  adjectives long
  in quay
  scenery
  at 460 110
  description: A long low point of land, black against the sky. It hides the far shore from the quay and from the office window both. Only the tower sees over it.

on take town_skiffs
  say: They are chained, and the padlock is the harbour board's.

on attack town_skiffs
  say: You'd wake the town and still not break the chain.

npc publican "publican"
  nouns publican landlord man mott
  in quay
  at 226 188
  here: Mott the publican is rolling casks down to the ferry, one at a time.
  description: A big man in a leather apron, red to the elbows.
  talk: "Evening, Ringer. Last of the beer for Kell, and then they can drink water till spring."
  topic casks, beer, barrels:
    "Twelve. Don't stand where they're going."
  topic lamp, light:
    "Can't see it from down here. Never could, for the point. That's your job, isn't it?"
  topic ferry, tolly, midnight:
    "Midnight, Tolly says. He's never been late in his life."
  topic oake, harbourmaster:
    "He's been at his desk since six with a pen in his hand. Contracts."
  unknown: "Couldn't tell you."
  idle every 4:
    A cask goes rumbling down the planks onto the ferry.
    Mott leans on a cask and gets his breath.
  figure
    layer mott colour white
      circle 0 -46 7
      line 0 -39 0 -12
      line -14 -26 0 -34 14 -26
      line -10 0 0 -12 10 0

# --- The ferry ---------------------------------------------------------------

room ferry "The Ferry"
  description:
    The ferry's deck is stacked to the rails with sacks, crates and casks,
    lashed down under canvas. Aft, the wheelhouse has a chart pinned flat
    on its table and a lamp swinging over it. A deckhand hangs over the
    rail, pale. The quay is west.
  exit west quay
  exit out quay
  spot 240 236
  spot 300 240
  art
    layer deck colour plank
      line 0 200 640 200
      line 0 140 0 200
      line 0 140 520 140
    layer rail colour plank
      line 0 120 520 120
      line 40 120 40 140
      line 140 120 140 140
      line 240 120 240 140
      line 340 120 340 140
      line 440 120 440 140
    layer cargo colour sail
      line 40 196 60 160 160 156 190 196
      line 200 196 210 166 300 164 312 196
      line 60 160 120 150 160 156
    layer wheelhouse colour plank
      rect 470 60 150 140
      line 470 60 545 40 620 60
      rect 500 90 40 30
    layer wheel colour bronze
      circle 590 130 16
      line 574 130 606 130
      line 590 114 590 146
    layer chart colour sail
      rect 490 150 60 20
    layer lamp colour flame flicker
      dots 520 76
    layer water colour lake drift 6 0 300
      line 60 230 120 230
      line 200 244 280 244
      line 380 228 450 228

object chart "chart"
  nouns chart map table soundings figures
  adjectives harbour
  in ferry
  scenery
  readable:
    The harbour board's chart of the lake, dated thirty years back. The
    crossing is ruled from the quay to the Far Lamp. A gravel bar, THE
    TEETH, is drawn south of the line, well clear of it. Along the line
    itself the soundings run deep: four fathoms, five, six in the middle.
  at 520 160
  description: A chart of the lake, pinned flat. You could read it.

object wheel "wheel"
  nouns wheel helm
  in ferry
  scenery
  at 590 130
  description: The ferry's wheel, its spokes worn pale where Tolly holds them.

npc tolly "Tolly"
  nouns tolly ferryman man skipper
  proper
  in ferry
  at 420 196
  here: Tolly the ferryman is checking the lashings, whistling.
  description: A broad cheerful man in a knitted cap. He has run the ferry for twenty years and never once lost a cargo.
  talk: "Ringer! Come to see us off? Midnight, sharp."
  topic lights, light, kell, two, range, channel:
    "Told you. Two lights, stack 'em, and you're in. Kell's spoiled."
  topic steer, steering, steers, course:
    "I steer by what I see. Always have."
  topic water, low, level, depth, deep:
    "Water's low. She'll float. There's six fathom on the line."
  topic chart, map:
    "Chart's older than my time on her. Never been wrong yet."
  topic lamp, far:
    "I'll pick it up as soon as I'm round the point. Always do."
  topic teeth, gravel, bar:
    "South of the line, well clear. You'd have to try."
  topic ines, marl:
    "Ines kept that lamp thirty years. Never a night out."
  topic oake, harbourmaster:
    "Oake wants us gone at midnight and not a minute after."
  topic parcel:
    "For me? Hand it over, then."
  topic ferry, boat, cargo, winter:
    "Ninety tons for Kell. Flour, salt, oil, beer. The doctor's crate. All of it, or Kell goes short."
  unknown: Tolly scratches his head under his cap. "Couldn't say."
  figure
    layer tolly colour white
      circle 0 -48 7
      line -7 -52 7 -52
      line 0 -41 0 -14
      line -14 -28 0 -36 14 -28
      line -10 0 0 -14 10 0

npc deckhand "deckhand"
  nouns deckhand boy lad hand sailor
  adjectives pale seasick young
  in ferry
  at 120 118
  here: The deckhand is draped over the rail, watching the water in case it starts moving.
  description: A boy of sixteen or so, from Kell by his voice, green about the mouth though the ferry is still tied up.
  talk: "Is it moving? It feels like it's moving."
  topic kell, lights, light, two, home, range:
    "Kell's easy coming in. Two lights, one on top of the other, like
    plates stacked on a shelf. If they slide apart you've gone wrong."
  topic sick, seasick, water, stomach:
    "We're not even off the quay." He swallows. "I'll be fine."
  topic tolly:
    "He whistles the whole way. The whole way."
  unknown: The deckhand only groans.
  figure
    layer boy colour white
      circle 0 -10 5
      line 0 -5 12 4
      line 4 0 12 -6

event tolly_kell enter ferry
  if not tolly_spoke
  say:
    Tolly sees you and straightens up. "Ringer! Look at that, they've sent
    the bell to see us off." He jerks his head at the deckhand. "This one's
    from Kell. Kell's spoiled, mind. Two lights there, one on the jetty and
    one on the church. Keep the one sat on top of the other and you're in
    the channel. Here we've only the one." He shrugs. "I point her at it."
  set tolly_spoke

on give parcel to tolly
  say:
    Tolly weighs it in his hand and looks at the knot. "That's her knot.
    Nobody else ties it like that." He grins. "Lamp oil for my own lantern.
    She does it every year." He stows it by the wheel.
  move parcel to nowhere

on show parcel to tolly
  try give parcel tolly

# --- Under the office window ---------------------------------------------------

room under_window "Under the Office Window"
  description:
    The back of the harbour office, where the quay runs out into mud and
    reeds. The lit window is above your head. A shadow crosses the blind,
    turns, crosses back. Through the glass comes a voice reading figures
    aloud to nobody. The quay is east; the reed bank is south.
  exit east quay
  exit south reed_bank
  spot 380 240
  art
    layer wall colour stone
      line 0 30 380 30
      line 380 30 380 220
      line 0 220 380 220
    layer window colour amber flicker
      rect 160 70 80 60
    layer shadow colour dim drift 40 0 360
      circle 190 92 10
      line 190 102 190 130
    layer pipe colour stone
      line 340 30 340 220
    layer butt colour plank
      rect 280 170 40 50
    layer mud colour dim
      line 380 230 640 236
      line 0 240 380 236
    layer reeds colour reed
      line 520 236 516 190
      line 540 236 546 196
      line 580 236 574 186
      line 610 236 618 200

object office_window "office window"
  nouns window blind shadow office light
  adjectives lit office harbour
  in under_window
  scenery
  at 200 100
  description: A square of yellow blind with Oake's shadow moving on it.

npc oake "Harbourmaster Oake"
  nouns oake harbourmaster man voice
  adjectives harbour
  proper
  in under_window
  at 200 100
  here: Above you, Oake's voice goes on: "Ninety tons at four shillings, against the penalty clause..."
  description: You can only see his shadow on the blind, square-shouldered, with a pen in its hand.
  talk: You'd have to knock, and then you'd have to explain why you're out here and not up the tower.
  topic lamp, light, teeth, ferry:
    You'd have to knock, and then you'd have to explain.
  unknown: You'd have to knock, and then you'd have to explain.

on listen if in under_window
  say: "Ninety tons at four shillings. Penalty for late delivery after the first of the month. Ice by the tenth." The pen scratches. "Two hundred years that lamp's stood."

# --- The net sheds ---------------------------------------------------------------

room net_sheds "The Net Sheds"
  description:
    Three tarred sheds below the quay, their doors propped open, nets
    hanging inside like grey curtains. Bales of oily fleece are stacked
    under the eaves for packing the winter's crockery. The quay is north;
    the reeds are down the bank to the southwest.
  exit north quay
  exit up quay
  exit southwest reed_bank
  exit west reed_bank
  exit down reed_bank
  spot 300 240
  art
    layer sheds colour tar
      line 20 220 20 90 90 60 160 90 160 220
      line 200 220 200 80 280 50 360 80 360 220
      line 400 220 400 90 470 60 540 90 540 220
    layer doors colour dim
      rect 60 130 50 90
      rect 250 120 60 100
      rect 440 130 60 90
    layer nets colour stone
      line 64 134 100 214
      line 104 134 70 214
      line 254 124 300 214
      line 304 124 260 214
      line 444 134 490 214
    layer fleece colour sail
      ellipse 580 210 30 12
      ellipse 590 188 26 10
    layer ground colour dim
      line 0 222 640 222

object fleece "fleece"
  nouns fleece wool bales bale
  adjectives oily
  in net_sheds
  scenery
  at 584 200
  description: Bales of raw fleece, oily with lanolin. Handfuls pull out easily.

object wool "handful of wool"
  nouns wool handful fleece
  adjectives oily raw
  in nowhere
  description: A greasy handful of raw wool. It would pad anything that rattles or squeaks.

object nets "nets"
  nouns nets net
  plural
  in net_sheds
  scenery
  at 280 170
  description: Herring nets, hung up to dry and mend. They smell of the summer.

on take fleece if carrying wool
  say: One handful is plenty.

on take fleece
  say: You pull a handful of wool from the nearest bale.
  move wool to player

on take nets
  say: They are hung up for mending, and a net is no use to you tonight.

# --- The reed bank -----------------------------------------------------------------

room reed_bank "The Reed Bank"
  description:
    Reeds higher than your head, standing in black water at the edge of the
    lake. The mud sucks at your boots. Back up the bank the harbour office
    window throws a square of yellow on the reeds. The net sheds are up
    the bank to the east, the office wall north. Open water is east.
  note if skiff_found and not launched: Ines Marl's skiff lies hidden in the reeds, its bow on the mud.
  exit north under_window
  exit east west_water
  exit northeast net_sheds
  exit up net_sheds
  exit west "The reeds go on, and the mud gets deeper."
  spot 300 238
  art
    layer reeds colour reed
      line 20 250 26 120
      line 40 250 36 110
      line 60 250 70 130
      line 90 250 84 116
      line 110 250 118 126
      line 470 250 464 120
      line 490 250 498 112
      line 520 250 512 130
      line 550 250 556 116
      line 580 250 574 124
      line 610 250 618 110
      line 630 250 626 130
    layer heads colour tar
      ellipse 26 116 3 8
      ellipse 84 112 3 8
      ellipse 498 108 3 8
      ellipse 556 112 3 8
      ellipse 618 106 3 8
    layer water colour lake drift 5 0 300
      line 150 180 230 180
      line 260 196 360 196
      line 380 176 440 176
      line 180 216 300 216
    layer far colour dim
      line 130 140 210 134 300 138 380 130 460 136
    layer office_light colour amber faint
      line 200 252 160 230
      line 300 252 330 230
    layer drag colour chalk reveal skiff_found "The beam lies low across the mud and finds a groove, fresh and wet, where a boat was dragged into the reeds, and beside it the prints of somebody's boots, small and deep. At the end of the groove, pushed in among the stems, a skiff." at 300 232
      line 240 250 262 236 292 228 340 222 380 216
      line 250 244 256 240
      line 270 238 276 234
      line 300 232 306 228
      line 330 226 336 222

object reeds "reeds"
  nouns reeds reed stems mud
  in reed_bank
  scenery
  plural
  at 300 232
  description: Reeds, and the mud they grow in. A boat could be pushed in among them and lost.

object skiff "Ines's skiff"
  nouns skiff boat
  adjectives ines ines's marl's small
  proper
  in nowhere
  scenery
  at 400 220
  description: A small clinker skiff, tarred, with her oars shipped inside and the thole oarlocks bare. The bottom boards are still wet.
  figure
    layer hull colour plank
      line -50 -6 -40 6 40 6 54 -8
      line -50 -6 54 -8
    layer oars colour plank
      line -30 -10 30 -16

object oarlocks "oarlocks"
  nouns oarlocks oarlock rowlocks rowlock tholes thole oars oar
  adjectives iron dry
  plural
  in nowhere
  scenery
  at 400 216
  description: Iron oarlocks, dry and rusty. They will squeal at every stroke.

event skiff_appears when skiff_found
  move skiff to reed_bank
  move oarlocks to reed_bank

on muffle oarlocks if muffled
  say: They are packed tight with wool.

on muffle oarlocks if not carrying wool
  say: You'd need something soft to pack them with.

on muffle oarlocks
  say: You pack the greasy wool into each oarlock and work the oars in it. They turn without a sound.
  set muffled
  move wool to nowhere

on put_in wool oarlocks
  try muffle oarlocks

on put_on wool oarlocks
  try muffle oarlocks

on tie wool to oarlocks
  try muffle oarlocks

on muffle skiff
  try muffle oarlocks

on take skiff
  say: You'll row it, not carry it.

on push skiff
  say: Go EAST to push off.

on row if in reed_bank
  say: Go EAST to push off.

on go east if in reed_bank and not skiff_found
  say: The water past the reeds is black and cold, and you have no boat.

on go east if in reed_bank and not rang_ten
  say: You look back up at the tower. Ten hasn't been rung. If you go now, nobody rings it.

on go east if in reed_bank and not rig_armed
  say: You look back up at the tower. If you go now, nobody rings eleven, and the whole town comes up the lane to see why.
  set refused_eleven

on go east if in reed_bank and not muffled and squealed
  lose caught:
    You push off and pull. The oarlocks shriek. The office door bangs open
    and Oake comes down through the reeds with a lamp, and sees you, and
    sees the skiff. "Ringer," he says. "Out of that boat." You tell him the
    Lamp is out. He goes up the tower to look, and keeps the ferry at the
    quay, and does not ask you anything else.

on go east if in reed_bank and not muffled
  say:
    You push the skiff out and pull once. The oarlocks shriek like a gate.
    Up the bank the office door opens and light falls across the reeds.
    "Who's that?" You hold still in the stems. The door stays open a while,
    then shuts. He'll be listening now. You let the skiff drift back onto
    the mud.
  set squealed
  sound squeal

on go east if in reed_bank
  say:
    You push the skiff off the mud and step in. The muffled oars dip
    without a sound. You row the way every rower does, facing backwards,
    keeping the quay lights over the stern, and the reeds close behind
    you.
  set launched
  sound oar
  minutes 8
  continue

on examine oarlocks if muffled
  say: Packed tight with greasy wool. They turn without a sound.
