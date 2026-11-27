# The Hall of Vanes: the ground floor, the bellows, the speaking-tube and
# the wind-gate before the stair.

room hall "The Hall of Vanes"
  scene hall
  depth 248 190 314 256
  start 300 300
  walk 30 248 600 286
  walk 150 248 600 314
  walk 30 302 600 314
  description:
    A round hall, lamplit and cool, its walls hung with old weathervanes:
    a fish, a ship, a running hare, a crescent moon, all pointing their own
    ways like guests at a party nobody introduced. A cold hearth yawns on
    the left; a stair curls up the right-hand wall behind a little
    turnstile of canvas sails. A brass speaking-tube opens beside it.
  note if gate_open: The wind-gate's sails stand edge-on, and the stair is open.
  exit out foot through tower_door at 300 310 zone 230 306 380 314
  exit south foot through tower_door at 300 310
  exit up "You can't: the wind-gate stands across the stair." at 528 252 zone 512 248 544 256
  show bellows_hook if bellows in hall
  show ribbon_on if mended and bellows in hall
  show gate_shut if not gate_open
  show gate_open if gate_open
  front chair_front 60 150 300

object bellows "pair of bellows"
  nouns bellows pair
  adjectives old leather hearth split mended
  in hall
  area 176 180 206 236
  stand 190 254
  description: A pair of hearth bellows, older than you, with brass nozzle and carved handles. The leather is split along one seam, so that they wheeze rather than blow.

object hearth "hearth"
  nouns hearth fireplace fire grate mantel mantelpiece ash ashes
  adjectives cold stone big
  in hall
  scenery
  area 40 120 170 250
  stand 110 254
  description: A great stone hearth, swept and cold. On the mantel, a row of small brass hares sit like books. There's been no fire since spring: a fire wants a draught.

object vanes "weathervanes"
  nouns vanes weathervanes fish ship hare moon rack
  adjectives old wall
  in hall
  scenery
  plural
  area 220 70 420 160
  description: Retired weathervanes, each with a label in a firm old hand. THE FISH: SOUTH WIND, 1932-1961. THE SHIP: SOUTHWEST, BLEW DOWN 1977. THE HARE: ALL WINDS, ON LOAN TO THE ROOF. THE MOON: NIGHT BREEZES (RETIRED, SULKS).
  readable: THE FISH: SOUTH WIND, 1932-1961. THE SHIP: SOUTHWEST, BLEW DOWN 1977. THE HARE: ALL WINDS, ON LOAN TO THE ROOF. THE MOON: NIGHT BREEZES (RETIRED, SULKS).

object tube "speaking-tube"
  nouns tube horn mouthpiece pipe speaking-tube
  adjectives speaking brass
  in hall
  scenery
  area 424 136 452 176
  stand 438 254
  description: A brass mouthpiece like a small gramophone horn, at the end of a pipe that climbs the wall and disappears up the stair-well. A tag says: SPEAK UP.

npc tamsin "voice"
  nouns voice girl tamsin someone somebody apprentice child
  adjectives small fierce
  in nowhere
  proper
  article the
  description: You can't see her: only a voice in the tube, from a long way up.
  arrives: The tube, it seems, has somebody at the other end.
  topic wind, winds, weather:
    "They're resting," says the voice, too quickly. "Winds need a rest. Everybody knows that."
  topic gran, wenna, keeper, grandmother, hale:
    "She's coming back by evening," says the voice. "She said. So it's
    staying evening till she does. That's just sense." The tube goes
    quiet, the way a person does.
  topic evening, sun, time:
    "It's a very good evening," says the voice. "I'm keeping it."
  topic gate, stair, sails:
    "The gate only lets the wind up," says the voice, smugly, "and there isn't any. So."
  topic tamsin, name, herself, apprentice:
    "Tamsin Hale, apprentice keeper, third class," says the voice. "Second class when Gran gets back."
  topic clementine, goat, key, apple:
    "Clementine was supposed to keep that key." A deep, put-upon sigh
    comes down the pipe. "You can't trust anyone with an apple tree."
  unknown: "Don't know," says the voice in the tube. "Don't care. Go away. Please."

object gate "wind-gate"
  nouns gate turnstile sails sail canvas windgate wind-gate
  adjectives wind little canvas
  in hall
  scenery
  area 496 186 566 252
  stand 516 258
  description:
    A turnstile of four little canvas sails on an iron post, set across
    the foot of the stair. Painted on the post: THE WIND LETS YOU UP.
    The sails are broadside on, politely and completely in the way.
  readable: THE WIND LETS YOU UP.

object chair "armchair"
  nouns chair armchair
  adjectives arm big old wing
  in hall
  scenery
  area 60 230 150 306
  stand 156 296
  description: A wing armchair, sagging in the shape of somebody small and old and very comfortable. A shawl hangs over the back. It is Wenna's chair, or you're no judge of chairs.

object clock "tall clock"
  nouns clock hands face pendulum
  adjectives tall long-case
  in hall
  scenery
  area 364 116 406 250
  stand 386 256
  description: A tall clock in dark oak, its brass pendulum hanging dead straight. The hands stand at twelve minutes past seven, which, to be fair to the clock, is the time.
  readable: XII past VII. As ever.

object map "weather map"
  nouns map chart arrows valley
  adjectives weather pinned old
  in hall
  scenery
  area 150 58 216 106
  description: A hand-drawn map of the valley, pinned at the corners: the mere, the mill, the Spire, and red arrows for the winds, each labelled in the keeper's firm hand. Someone has drawn a small, defiant cross through every arrow in pencil.

object books "books"
  nouns books shelf shelves book
  adjectives weather old
  in hall
  scenery
  plural
  area 20 58 130 92
  description: Weather books: A KEEPER'S ALMANAC, ON THE BINDING OF BREEZES, WINDS I HAVE KNOWN (vols. 1-4), and a thin new one in a child's hand: HOW TO STOP EVENING.
  readable: HOW TO STOP EVENING, by T. Hale, aged 11. Page one: "Step 1. Stop the winds." Page two is blank.

on take books
  say: You slide out HOW TO STOP EVENING, read the first page, and put it back exactly where it was. Some books are evidence.

on push clock
  say: You give the pendulum a nudge. It swings twice, thinks better of it, and hangs straight again. Clocks know.

on listen clock
  say: Not a tick. A clock with nothing to count.

object stair "stair"
  nouns stair stairs staircase steps stairwell
  adjectives stone winding spiral
  in hall
  scenery
  area 470 40 630 250
  description: Stone steps curl up the wall and out of sight. From somewhere far above comes the creak of a floorboard under a very light foot.

# --- The tube and Tamsin -------------------------------------------------------------

on talk tube
  say:
    You put your mouth to the brass horn. "Hello?" A pause, then a voice
    rattles down the pipe, small and fierce: "Who's that? Gran?" Another
    pause. "You're not Gran. Nobody's to come up till evening's over.
    It's the rule. I made it."
  move tamsin to hall
  achieve tube

on talk tamsin
  say: "I'm not talking to you," says the voice in the tube. "I'm guarding." After a moment: "What do you want?"

on tell tamsin about any
  say: "La la la," says the tube.

on blow tube
  say: You blow down the tube. From far above comes a yelp, and then, with dignity: "That was very rude."

on listen tube
  say: Far up the pipe, someone small is humming, and stops the moment she thinks you might be listening.

# --- The bellows and the gate -----------------------------------------------------------

on pump bellows if not mended
  say: You squeeze the bellows. They wheeze out of their split seam like an old dog sighing, and the nozzle gives no more than a whisper.

on blow bellows
  try pump bellows

on tie ribbon to bellows if not mended
  say:
    You wind the red ribbon tight round the split seam, three turns and a
    surveyor's hitch, and give the bellows an experimental squeeze. FWOOF.
    A good honest gust from the nozzle lifts the ash in the grate for the
    first time since spring.
  move ribbon to nowhere
  set mended
  achieve mended

on tie bellows with ribbon
  try tie ribbon bellows

on put_on ribbon bellows
  try tie ribbon bellows

on use ribbon on bellows
  try tie ribbon bellows

on tie ribbon to any
  say: The ribbon would hold {the2}, but {the2} doesn't need holding.

on untie ribbon if mended
  say: And split the bellows again? No: the ribbon is doing honest work.

on pump bellows at gate if mended and not gate_open
  say:
    You aim the bellows at the wind-gate and pump. FWOOF. The little
    canvas sails fill, swing, and spin round with a clatter, and stop
    edge-on to the stair. Far above, a small voice says, "Hey!"
  set gate_open
  achieve gate

on blow gate with bellows
  try pump bellows gate

on use bellows on gate
  try pump bellows gate

on pump bellows if mended and not gate_open
  say: FWOOF. A fine gust. You could aim it at something.

on pump bellows if mended and gate_open
  say: FWOOF. The wind-gate's sails rattle happily. They're open already.

on push gate if not gate_open
  say: You push one of the sails. It doesn't give at all: the turnstile is built to turn for wind, not for people.

on blow gate if not gate_open
  say: You puff at the sails until you see spots. One of them quivers, as if amused.

on knock vanes
  say: You tap the fish. It swings round to point at you, considers, and swings back.

on take vanes
  say: They're the keeper's, and fixed to the wall, and you would look ridiculous.

on sit if in hall
  say: You sit in Wenna's chair. It is as comfortable as it looks, which is dangerous. You get up before the evening can have you.

on climb stair if not gate_open
  say: The wind-gate stands in the way.

on climb stair if gate_open
  say: The gate is open: just go UP.

on go up if gate_open
  say:
    You step through the wind-gate and set your boot on the first stair.
    From somewhere very high, a door slams, and small feet run.

    Above you, the Spire waits: the bellows room, the rain loft, the
    gallery of stars, and whoever has stopped the winds.
  win:
    (That is the end of the part of The Stillwind Spire that is built so
    far: three scenes and the way in. The rest of the tower is still being
    painted.)

on smell hearth
  say: Cold soot and old applewood. Somebody here burns apple logs.

on search hearth
  say: Ash, swept tidy. And a paper twist of something that smells like sherbet, which you leave alone. Someone small has been sitting in this hearth.

on listen if in hall
  say: The tower ticks and settles as stone does. High up, a floorboard creaks under a light foot and stops, guiltily.
