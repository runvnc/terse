# LOW BELL
#
# One night on a lake. The bellringer of Low Bell finds the Far Lamp dark
# at the nine o'clock look, and decides to relight it before the last
# ferry sails at midnight. The design is in the owner's notes (draft 2);
# STORY.md describes this format.
#
# Files: 0-game (this: settings, sounds, verbs, hints, the clock, the
# endings), 1-tower, 2-town, 3-lake, 4-shore.

game
  title Low Bell
  author Tersenet
  start bell_chamber
  carry 8
  clock 21:00
  minutes 1
  beam 44
  step step
  score 10
  status: {room}|| {time}   next bell {bell}   score {score}/{max}
  intro:
    Nine o'clock. You climb the last ladder into the bell chamber of Low
    Bell, as you have every night for eleven years, and put your eye to
    the notch in the east sill.

    The Far Lamp is out.

    Custom is plain about this. If the Lamp is dark at the nine o'clock
    look, the bellringer rings the dark peal and the ferry stays at the
    quay. Tonight the last ferry before the freeze sails at midnight with
    the town's winter in its hold: flour, salt, lamp oil, the doctor's
    crate. Ring the peal and all of it waits for spring.

    You leave the rope where it hangs. If the hours ring as usual, nobody
    down there will think to look east. The Lamp is a mile off across the
    water, and you have three hours.

    (Type HELP for how to play and HINT when you are stuck. Looking and
    examining cost no time; most other things cost a minute or two.)
  help:
    Type what you want to do in a few words: LOOK, EXAMINE NOTCH, TAKE
    CAN, GO DOWN (or D), ASK INES ABOUT WICK, PUT WICK IN LAMP. INVENTORY
    (I) lists what you carry. The status line shows the time and the
    minutes to the next bell.

    Lit, your lantern throws a beam. Move it with the arrow keys or the
    pointer, or aim it with SHINE LANTERN AT something. The beam shows
    marks the picture does not, and what it has shown stays drawn, faint.

    Looking, examining, reading and the inventory take no time. Walking
    takes a minute, rowing about eight. UNDO takes a move back, SAVE and
    RESTORE keep a game, HINT helps a little more each time you ask.

message end_won: *** The end ***
message end_lost: *** The end ***
message dark_room: It is black here. Without a light you have only what you remember.
message wait: Time passes.

colour plank 170 130 80
colour chalk 235 235 220
colour flame 255 190 60
colour bronze 196 146 72
colour lake 60 120 210
colour reed 120 150 90
colour tar 120 110 100
colour stone 150 150 150
colour sail 210 200 180

achievement tally "The ringers' marks" 1
achievement ten "Ten, on the hour" 1
achievement rig "The bell that rings itself" 2
achievement gravel "Gravel where the chart says deep" 1
achievement log "The keeper's log" 1
achievement bearing "Her bearing" 1
achievement lamp_relit "The Far Lamp, burning" 1
achievement two_lights "Two lights" 2

counter rows 0

# --- Sounds ----------------------------------------------------------------

sound toll bell C3 70, G2 90
sound peal bell E3 20, C3 20, A2 20, E3 20, C3 20, A2 60
sound step tick C6 3
sound squeal swish B5 10, rest 4, B5 14
sound oar swish C3 30
sound grind thud C2 10, rest 3, C2 8
sound chime chime E6 6, B6 14
sound horn horn D2 50, rest 6, D2 70
sound thud thud C2 12
sound water swish C3 40

# --- Time: looking, examining and reading take none; the rest a minute;
# rowing between the lake's places eight (the rules in 3-lake).

minutes go 1
minutes ask 1
minutes talk 1
minutes tell 1
minutes wait 10

# --- Verbs this story adds ---------------------------------------------------

verb ring = ring, toll, peal, sound
  minutes 1
  default: There is nothing here to ring.
verb fill = fill, refill, refuel, top
  default: That holds nothing worth filling.
verb buy = buy, purchase
  default: Nobody here is selling anything.
verb tie = tie, fasten, knot, attach, bind
  default: There is nothing here to tie that to.
verb untie = untie, unknot, loosen, unwrap, uncover
  default: It isn't tied.
verb muffle = muffle, wrap, pad, quiet, quieten, silence
  default: It makes no noise to speak of.
verb row = row, scull, paddle, launch
  alone
  default: You need to be in a boat to row.
verb sight = sight
  default: You can't sight along that.
verb fit = fit, mount, fix
  default: That fits nothing here.

# --- Hints, in the order they are tried ------------------------------------

hint lantern
  while not on lantern and in bell_chamber
  - Your lantern is dry. Look around the bell chamber.
  - The oil can on the floor has a dreg left in it. FILL LANTERN.
  - FILL LANTERN, then LIGHT LANTERN.

hint tally
  while in bell_chamber and on lantern and not tally_seen
  - Your lantern's beam shows things the picture does not. Try the sill.
  - SHINE LANTERN AT NOTCH, or move the beam with the arrow keys.

hint wick
  while not carrying wick and not wick_fitted and not launched
  - The Lamp will need a wick if the old one is gone. Who in town sells lamp gear?
  - Ines Marl keeps the chandlery, east off Church Lane. ASK INES ABOUT WICK.
  - Ines wants a favour first. TAKE PARCEL. While you are there, BUY CORD and BUY CANDLES.

hint parcel
  while carrying parcel
  - The parcel is for Tolly, the ferryman.
  - The ferry is moored at the quay, south of Church Lane. GIVE PARCEL TO TOLLY.

hint boat
  while not skiff_found
  - You need a boat. The town skiffs are chained up for the winter.
  - Ask Ines about a boat, or look along the reeds west of the net sheds with your lantern.
  - In the reed bank, SHINE LANTERN AT REEDS.

hint rig_early
  while refused_eleven and not rig_armed
  - Something has to ring eleven while you are on the water. The clock room is built to strike.
  - Shine the lantern at the bell frame.

hint oars
  while skiff_found and not muffled
  - Those oarlocks will squeal, and the harbourmaster's window looks over the reeds.
  - The net sheds have wool. TAKE WOOL, then MUFFLE OARLOCKS.

hint ten
  while not rang_ten and time >= 21:40
  - Ten has to be rung from the bell chamber, on the hour.
  - Be at the top of the tower by ten. RING BELL between ten and ten past.

hint frame
  while rang_ten and not frame_seen and not rig_armed
  - You cannot be across the lake and in the tower at eleven. The old bell frame might help.
  - SHINE LANTERN AT FRAME in the bell chamber.

hint rig
  while frame_seen and not rig_armed
  - The drawing shows the clock room below: a weight, a lever, and a candle.
  - You need the cord and the hour candles from the chandlery. In the clock room: TIE CORD TO LEVER, WIND WEIGHT, PUT CANDLE IN CASE.
  - Then LIGHT CANDLE.

hint launch
  while rig_armed and in reed_bank and not launched
  - The skiff is ready. Row east.
  - EAST.

hint hatch
  while in hut_outside and not hatch_seen
  - The door is locked and you have no key. The beam might find another way in.
  - SHINE LANTERN AT WALL.

hint lamp
  while visited lamp_room and not far_lamp_shows
  - The lamp is whole. It is missing its wick, and something covers the lens.
  - PUT WICK IN LAMP, UNTIE COVER, then LIGHT LAMP.

hint hill
  while visited lamp_room and not bearing_seen
  - Read the log. Then there is the hill behind the hut.
  - Go UP from outside the hut, and use the lantern on the stones.
  - At the cairn, SHINE LANTERN AT STONE.

hint two
  while bearing_seen and tolly_spoke and not lantern_in_niche
  - Remember what Tolly said about Kell.
  - One light shows where a thing is. Two lights, one above the other, show a line to steer down.
  - Light the Far Lamp. Then PUT LANTERN IN NICHE here at the cairn.

hint two_later
  while bearing_seen and not tolly_spoke and not lantern_in_niche
  - The chalk line runs from the cairn, over the Lamp, to the tower. A ferry on that line misses the gravel.
  - A ferryman can hold a line if he has two lights to keep one above the other.
  - Light the Far Lamp. Then PUT LANTERN IN NICHE here at the cairn.

# --- The clock -----------------------------------------------------------------

event ten_warn at 21:50
  if not rang_ten
  say: Down the lane a door bangs and someone calls a child in. Ten minutes to ten, and ten is rung from the bell chamber, on the hour.

event ten_warn2 at 22:05
  if not rang_ten
  say: Five past ten, and the bell has not spoken. Somewhere a window opens.

event ten_missed at 22:10
  if not rang_ten
  lose missed_ten:
    Ten does not ring. In a town that sets its clocks by the bell, that
    brings people out. Lanterns come up Church
    Lane, someone climbs the tower, and from the bell chamber anyone can
    see the far shore is dark. Harbourmaster Oake keeps the ferry at the
    quay and asks you, in front of half the town, where you were.

event eleven at 23:00
  if rig_armed
  say:
    Across the water, faint and certain, the bell of Low Bell begins to
    strike. You count. It stops at eleven. Nobody's hand was on the rope.
  sound toll

event eleven_missed at 23:10
  if not rig_armed and not rang_eleven
  lose missed_ten:
    Eleven does not ring. People come up the lane with lanterns, and the
    first of them to reach the bell chamber looks east and sees that the
    Lamp is dark.

event horn_warn at 23:45
  say: From behind the point, faint, Tolly's horn: once, long. A quarter of an hour.
  sound horn

# --- Midnight: the ferry sails, and the night is decided by lights --------------

event twelve at 0:00
  if midnight_armed
  say: Across the water the bell strikes twelve, from no hand.
  sound toll

event no_twelve at 0:00
  if rig_armed and not midnight_armed
  say: Midnight comes, and for the first time anyone can remember, the bell of Low Bell says nothing.

event end_two at 0:00
  if far_lamp_shows and lantern_in_niche and on lantern
  achieve two_lights
  win two_lights:
    You see none of it from the hill. At the quay, Tolly casts off and the
    ferry slides out from behind the point. He looks for the Lamp and finds two lights, one low
    on the shore and one above it on the hill, apart. He swings north
    until the upper one sits on the lower, and holds it there.

    The ferry runs down the line, past the Teeth in the dark, close enough
    that the deckhand hears water breaking on gravel. Then it is past, and
    turning down the lake for Kell.

    "Somebody set me a range," Tolly says to nobody.

event end_grounded at 0:00
  if far_lamp_shows and not lantern_in_niche
  lose grounded:
    The ferry comes out from behind the point and steers for the Lamp, as
    it always has. Halfway over there is a sound
    like a cart tipping out stones. The ferry stops, leaning, with its
    light held still over the water.

    Nobody is drowned. The hold takes water, the flour and salt are lost,
    and the ice comes in before anything else can be fetched across.

    Nobody ever learned the lamp had gone out.

event end_lantern_only at 0:00
  if not far_lamp_shows and lantern_in_niche and on lantern
  lose one_small_light:
    Tolly brings the ferry out from behind the point and looks for the
    Lamp. There is a small yellow light on the far hill, too high and too
    faint to steer by. He watches it a while, then puts about for the
    quay.

    In the morning Oake comes up the tower. "You knew at nine," he says.
    You tell him about the log and the gravel and the chalk. He listens to
    all of it, which is more than he did for Ines.

event end_dark at 0:00
  if not far_lamp_shows and not lantern_in_niche and visited lamp_room
  lose dark_shore:
    Tolly takes the ferry out from behind the point, finds no Lamp, and
    will not sail blind. The ferry goes back to the quay and stays there.

    In the morning Oake climbs the tower. "You knew at nine," he says. You
    tell him what the log says, and what you saw under the boat, and he
    goes down to the quay to look at the water marks for himself. The
    town's winter waits for spring. The ferry is whole.

event end_dark_town at 0:00
  if not far_lamp_shows and not lantern_in_niche and not visited lamp_room
  lose dark_shore:
    Tolly takes the ferry out from behind the point, finds no Lamp, and
    will not sail blind. The ferry goes back to the quay and stays there.

    In the morning Oake climbs the tower. "You knew at nine," he says. You
    have nothing to tell him but that you saw it dark and rang ten anyway.
    The town's winter waits for spring.

event end_lantern_dark at 0:00
  if lantern_in_niche and not on lantern and not far_lamp_shows
  lose dark_shore:
    The ferry comes out from behind the point, finds no light on the far
    shore, and goes back to the quay. Your lantern sits cold in the
    cairn where you left it.

event end_lantern_dark_lit at 0:00
  if lantern_in_niche and not on lantern and far_lamp_shows
  lose grounded:
    The ferry steers for the Lamp. Your lantern sits cold in the
    cairn, so there is only the one light, and the one light leads onto
    the Teeth. The ferry stops, leaning, with its light held still over
    the water. Nobody is drowned. The winter's goods are lost.

# --- Pictures for the endings ------------------------------------------------------

picture two_lights
  layer window colour stone
    line 140 220 140 60 180 36 460 36 500 60 500 220
    line 120 220 520 220
  layer notch colour stone
    line 314 220 320 212 326 220
  layer shore colour dim
    line 140 150 180 140 230 144 280 130 330 136 380 124 430 134 470 128 500 136
  layer lake colour lake drift 6 0 300
    line 160 176 200 176
    line 230 190 290 190
    line 360 172 420 172
    line 430 196 480 196
  layer lights colour flame flicker
    circle 320 132 3
    circle 320 112 3
    line 310 132 300 131
    line 330 132 340 131
    line 312 112 304 111
    line 328 112 336 111
  layer line_of colour chalk faint
    line 320 136 320 208
  layer ferry colour sail drift 40 0 900
    poly 210 168 250 168 245 174 215 174
    line 222 168 223 160 236 160 237 168
  layer ferry_light colour amber drift 40 0 900
    dots 230 156
  layer teeth colour chalk faint
    line 240 184 270 178 300 186
    line 250 192 280 188

picture grounded
  layer window colour stone
    line 140 220 140 60 180 36 460 36 500 60 500 220
    line 120 220 520 220
  layer shore colour dim
    line 140 150 180 140 230 144 280 130 330 136 380 124 430 134 470 128 500 136
  layer lamp colour flame flicker
    circle 320 132 3
    line 310 132 300 131
    line 330 132 340 131
  layer ferry colour sail
    poly 254 176 296 170 292 178 258 182
    line 268 174 270 166 282 165 283 172
  layer ferry_light colour amber
    dots 276 160
  layer teeth colour chalk
    line 230 184 262 178 300 182 330 190
    line 240 192 280 186 320 194
    line 250 198 300 196

picture dark_shore
  layer window colour stone
    line 140 220 140 60 180 36 460 36 500 60 500 220
    line 120 220 520 220
  layer shore colour dim
    line 140 150 180 140 230 144 280 130 330 136 380 124 430 134 470 128 500 136
  layer teeth colour chalk faint
    line 230 184 262 178 300 182 330 190
    line 240 192 280 186 320 194
  layer stars colour white
    dots 170 60 220 48 290 70 350 52 410 66 470 58

picture one_small_light
  layer window colour stone
    line 140 220 140 60 180 36 460 36 500 60 500 220
    line 120 220 520 220
  layer shore colour dim
    line 140 150 180 140 230 144 280 130 330 136 380 124 430 134 470 128 500 136
  layer lantern colour amber blink 40 10
    circle 320 112 2

picture missed_ten
  layer tower colour stone
    line 260 256 260 70 320 30 380 70 380 256
    rect 300 80 40 50
  layer lane colour dim
    line 200 256 250 200
    line 440 256 390 200
  layer people colour flame blink 30 10
    dots 230 236 260 226 410 240 380 224 300 250

picture caught
  layer reeds colour reed
    line 40 256 46 180
    line 70 256 64 170
    line 100 256 108 186
    line 520 256 512 176
    line 560 256 566 184
  layer oake colour white
    circle 320 120 8
    line 320 128 320 180
    line 304 150 320 136 336 150
    line 308 210 320 180 332 210
  layer lamp colour flame flicker
    dots 344 146
