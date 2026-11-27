# The Spire's Foot: the great door, Wenna's note, the apple tree, the
# rain barrel.

room foot "The Spire's Foot"
  scene foot
  depth 254 196 314 256
  start 330 300
  walk 16 280 624 314
  walk 146 254 480 314
  walk 16 270 480 314
  walk 16 254 110 272
  walk 540 254 624 282
  walk 16 254 624 260
  description:
    The Spire rises straight out of the turf here, grey stone furred with
    ivy, so tall that looking up at it is mostly looking at sky. Its great
    arched door is shut, with a note pinned to it. An old apple tree leans
    on the tower's shoulder to the west; a rain barrel stands by the wall
    to the east, bone dry.
  note if apple in tree: One apple, the last of last year's, hangs high in the tree.
  note if apple in foot: An apple lies in the grass under the tree.
  note if open tower_door: The door stands open on a warm, lamplit hall.
  exit south meadow at 330 310 zone 250 306 420 314
  exit in hall through tower_door at 330 257 zone 312 254 348 259
  show apple_tree if apple in tree
  show apple_ground if apple in foot
  show door_open if open tower_door
  front trunk_front 110 146 266
  front barrel_front 480 540 274
  front door_open_front 290 370 256 if open tower_door

object tower_door "great door"
  nouns door doors
  adjectives great arched oak tower front
  in foot
  also in hall
  door
  locked
  key key
  area 296 150 364 252
  stand 330 263
  description:
    A great arched door of black oak, strapped with iron, with a ring for
    a handle and a keyhole you could post a letter through. A brass hare
    is let into the wood at eye height, ears back, as if bracing for a
    gale that has not come.

object note "note"
  nouns note paper notice letter
  adjectives pinned wenna's
  in foot
  scenery
  area 318 186 342 206
  stand 330 263
  description: A sheet of good paper pinned to the door with a hatpin, written in two hands.
  readable:
    In a firm, old hand:
    |  Gone down the mere to fetch the new rain. Back by evening.
    |  Tamsin minds the Spire. Key's with Clementine.   - W. Hale
    And under it, in pencil, round and fierce:
    |  NOBODY IS TO COME IN UNTIL EVENING IS OVER.   - T.

object tree "apple tree"
  nouns tree branches branch leaves trunk
  adjectives apple old gnarled
  in foot
  scenery
  container
  transparent
  area 40 30 230 268
  stand 168 272
  description: An old apple tree, crooked as a question, leaning on the tower as if it had been told a secret. Its leaves are summer-thick and perfectly still.

object apple "apple"
  nouns apple fruit
  adjectives green last red
  in tree
  area 172 100 196 124
  stand 168 272
  description: A small hard apple, green going red on the sunny side. In any ordinary autumn the wind would have brought it down. This one has been hanging on since last year out of sheer lack of encouragement.

object barrel "rain barrel"
  nouns barrel butt water float bung
  adjectives rain water dry wooden
  in foot
  scenery
  container
  open
  area 480 214 540 280
  stand 510 286
  description: An oak rain barrel under a spout from the roof, staved and hooped. It is dry to the bottom, where a wooden float lies on its side like a dead fish.

object scraper "boot-scraper"
  nouns scraper bootscraper
  adjectives boot iron
  in foot
  scenery
  area 372 236 392 252
  stand 382 262
  description: An iron boot-scraper in the shape of a hare. Everything here is in the shape of a hare.

object ivy "ivy"
  nouns ivy leaves stone stones wall walls window windows
  adjectives dark green grey
  in foot
  scenery
  area 214 0 446 250
  description: Ivy climbs the stone in dark ropes. High up, one small window shows a lamp lit inside, which means somebody has been lighting lamps in an evening that never ends. Somebody stubborn.

object spout "spout"
  nouns spout gutter pipe downpipe
  adjectives rain lead
  in foot
  scenery
  area 492 60 512 214
  description: A lead spout comes down the wall from somewhere high up and ends over the barrel. Nothing has come out of it since spring.

# --- The apple -------------------------------------------------------------------

on take apple if apple in tree
  say: You stretch up on your toes. The apple is at least a foot beyond your fingertips, and smugly so. Something longer would reach it.

on knock apple with staff if apple in tree
  say:
    You reach up with the vane-staff and tap the apple's stalk. It gives
    at once, with the sigh of a thing that has been waiting to be asked,
    and drops into the grass at your feet.
  move apple to foot
  achieve apple

on knock apple if apple in tree and carrying staff
  try knock apple staff

on attack apple with staff if apple in tree
  try knock apple staff

on use staff on apple if apple in tree
  try knock apple staff

on use staff on tree if apple in tree
  try knock apple staff

on knock tree with staff if apple in tree
  try knock apple staff

on wave staff if in foot and apple in tree
  say: You wave the staff about under the tree. It would work better aimed: KNOCK APPLE WITH STAFF.

on shake tree if apple in tree
  say: You shake the trunk. It's like shaking a house. A single leaf considers falling, and doesn't.

on climb tree
  say: You find a foothold, then another, and then you find out that you are sixty-three, which you knew. Down you come.

on eat apple
  say: You raise the apple to your mouth, remember the look Clementine gave it, and think better of it. Some trades are more important than lunch.

on knock apple if not apple in tree
  say: It's already down.

# --- The door --------------------------------------------------------------------

on knock tower_door
  say:
    You knock. The sound goes up the inside of the tower like a stone
    down a well. After a long while a voice calls down from very high
    up, small with distance: "NOBODY'S IN!" Then, after a pause: "COME
    BACK AFTER EVENING!"

on open tower_door if locked tower_door
  say: You push. The door has the patient weight of a door that is locked and knows it. The keyhole could take a big key.

after open tower_door if not door_done
  say: The great door swings inward without a sound. Lamplight, and the smell of old stone and older paper.
  set door_done
  achieve door

on pull tower_door
  try open tower_door

on take note
  say: You leave it where it is: it's the keeper's door, and the keeper's note.

on knock barrel
  say: It booms, hollow as a drum. Dry right down.

on search barrel
  say: Nothing in it but the wooden float on its side, and a spider who has given up.

on take barrel
  say: The barrel is full of nothing and still too heavy to lift; and the float is down at the bottom of a barrel taller than your arm is long, and anyway it's doing a very good job of being nothing.

on climb barrel
  say: Up on a dry barrel, to reach nothing in particular? No.

on smell tree
  say: Apple-leaf and bark. It smells like an autumn that hasn't been allowed.

on listen if in foot
  say: From far up inside the tower, faintly: a thump, and footsteps running on a stair. Then nothing.
