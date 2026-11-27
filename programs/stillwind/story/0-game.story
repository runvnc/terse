# The Stillwind Spire: the game, the player, the things she carries and
# the verbs and hints for the whole story. The format is Low Bell's
# (programs/lowbell/STORY.md) with painted scenes (SCENES.md).

game
  title The Stillwind Spire
  author for Terse
  start meadow
  carry 8
  # The clock is stopped: it has been 7:12 in the evening since spring.
  clock 19:12
  minutes 0
  status: {room}|| {time}, as ever     score {score}/{max}
  intro:
    The wind stopped over Hallow Mere one afternoon in spring, and the
    evening stopped with it: the sun has sat in the same notch of the
    mountains ever since. Ysolde Fenn, who surveyed the county's weather
    for forty years, knows that winds do not stop by themselves. They are
    kept, and in this valley they are kept in the old tower up the meadow.
    (Arrows walk, or click. Type what to do below. HELP, HINT.)

achievement apple "The apple comes down" 1
achievement trade "A fair trade with Clementine" 2
achievement door "Into the Spire" 1
achievement dandelion "A clock that will not tell" 1
achievement tube "A voice in the tube" 1
achievement mended "Bellows mended" 2
achievement gate "The wind-gate turns" 2

# --- Verbs this story adds ---------------------------------------------------

verb use = use, apply
  default: You can't think how to use {the} on its own. Try saying what you mean to do with {it}.
verb knock = knock, rap, bang, tap
  default: You knock on {the}. {The} {is} unmoved.
verb tie = tie, knot, fasten, bind, mend, patch, repair, fix
  default: There's nothing to tie {the} to, and nothing about {it} that wants mending.
verb untie = untie, unknot
  default: There is no knot in {the}.
verb pump = pump, work, squeeze
  default: {The} {is} not the pumping kind.
verb shake = shake, rattle, jiggle
  default: You shake {the}. Nothing falls out but a little of your dignity.
verb wave = wave, brandish, swing, flourish
  default: You wave {the} about. The air, such as it is, takes no notice.
verb feed = feed
  default: {The} {is} not hungry, or at least not for you.
verb pet = pet, scratch, cuddle, hug
  default: {The} {is} not that sort of thing.
verb sit = sit
  alone
  default: You would rather stand. Sitting down in a stopped evening feels like giving in.
verb whistle = whistle, hum, sing
  alone
  far
  default: You whistle a few bars of "The Wind's in the Willow". It goes nowhere: there's no wind to carry it.
verb think = think, ponder, consider
  alone
  far
  default: You think about winds. Kept winds, bound winds, winds in bottles. It is a good deal more than most people think about them.
verb lick = lick
  default: You are sixty-three years old.
verb measure = measure, survey, gauge
  far
  default: You can't measure {the} with anything you have; you note in your head that {it} {is} about the size {it} looks.

# Smaller verbs that mean ones the story has.
verb take = pluck, harvest

message examine: Nothing about {the} strikes you as remarkable, and you have been looking at things professionally for forty years.
message attack: You were a surveyor, not a brawler. Besides, it never helps.
message jump: You hop on the spot. Your knees file a short report.
message sleep: Sleep, in an evening that never ends? You'd never wake up.
message listen: Nothing. Not a leaf, not a creak, not a far-off dog. The quiet has the thickness of felt.
message smell: Warm grass, dust, and that sweetish smell of air that has not moved in months.
message wait: You wait. The sun does not move. It never does.
message climb: There's no sensible way up {the}.
message eat: You've had stranger lunches, but not on purpose.
message blow: You puff at {the}. Your breath is the only wind for miles, and it is not much of one.
message no_exit: There's no way to go in that direction from here.
message cant_see: You see no {word} here.

# --- The player and her things ---------------------------------------------------

object staff "vane-staff"
  nouns staff stick vane weathervane cane
  adjectives walking brass vane my
  in player
  description:
    Your walking stick: ash, a brass ferrule, and a little brass
    weathervane on top, an arrow with a feathered tail, which used to
    swing round to every breeze. It has pointed east since spring. It is
    the most professionally offensive thing you own.

object notebook "notebook"
  nouns notebook book notes journal
  adjectives field survey my little
  in player
  description: A field notebook with a pencil in its spine, swollen with forty years of other people's weather.
  readable:
    The newest page, in your hand:
    |  Hallow Mere. Wind: none. Since: April. Sun: fixed, WSW, 4 deg.
    |  Mill: stopped. Washing: damp. Dandelion clocks: unblown.
    |  Keeper: W. Hale, not seen. Tower: go and LOOK.

object satchel "satchel"
  nouns satchel bag
  adjectives leather my
  in player
  wearable
  container
  open
  description: A leather satchel that has been rained on in four counties. It used to be rained on, anyway.

object sweets "bag of humbugs"
  nouns humbugs humbug sweets sweet bag
  adjectives paper striped
  in satchel
  plural
  article some
  description: Striped mint humbugs in a paper twist. Emergency rations, in the surveying sense.

# --- Things in the whole story -----------------------------------------------------

object sun "sun"
  nouns sun sunset evening light
  adjectives low setting gold golden
  in meadow
  also in foot
  scenery
  description: The sun sits in a notch of the mountains, an inch above setting, exactly where it sat in April. You have checked with a pencil held at arm's length. Twice.

on take sun
  say: It is a little further off than it looks, and it looks quite far.

on wave staff
  say: You wave the vane-staff at the sky like a conductor whose orchestra has gone home. The vane stays pointing east, out of loyalty or rust.

on measure sun
  say: Four degrees above the ridge. It was four degrees in April. In your professional opinion, that is not how suns work.

on lick staff
  say: Brass, ash, and a faint taste of 1987.

on eat sweets
  say: You have a humbug. It is very minty. The valley remains exactly as still as before, but you feel slightly more able to deal with it.

on read staff
  say: Stamped on the vane's tail, tiny: MADE BY HALE, STILLWIND. A keeper's vane. Wenna gave it to you when you retired.

# --- Hints ---------------------------------------------------------------------------

hint apple
  while not carrying apple and not traded
  - Clementine has the key, and Clementine is a goat. What do goats like better than ribbons?
  - There is one apple left on the tree by the Spire's door, out of reach.
  - Your vane-staff is long. KNOCK APPLE WITH STAFF, then TAKE APPLE.

hint trade
  while carrying apple and not traded
  - Clementine wears the door key on a red ribbon. You have something she would like more.
  - GIVE APPLE TO GOAT, in the meadow.

hint door
  while traded and not open tower_door
  - The key opens the Spire's door.
  - UNLOCK DOOR WITH KEY, then OPEN DOOR, then IN.

hint bellows
  while in hall and not mended
  - The wind-gate wants a draught. Something by the hearth makes draughts, or would if it were whole.
  - The bellows' leather is split along a seam. Something you carry would bind it.
  - TIE RIBBON TO BELLOWS.

hint gate
  while mended and not gate_open
  - Mended bellows make a fine draught. Point it at the gate.
  - TAKE BELLOWS, then PUMP BELLOWS AT GATE (or BLOW GATE WITH BELLOWS).

hint stair
  while gate_open
  - The way up is open. UP.
