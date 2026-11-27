# The Meadow Path: the head of the valley, the Spire up the path,
# Clementine with the key.

room meadow "The Meadow Path"
  scene meadow
  depth 230 118 312 256
  start 330 300
  # The floor: the meadow less the boulder, Clementine's knoll and the fence.
  walk 14 274 470 312
  walk 14 248 104 312
  walk 196 248 626 296
  walk 14 248 626 256
  walk 14 230 290 250
  walk 362 230 626 250
  walk 436 222 476 236
  description:
    The path from the village ends here, in a meadow gone gold in the
    long light. Up the rise, the Stillwind Spire stands against the
    mountains, its pennant hanging like washing. Dandelion clocks stand
    everywhere, unblown. A track runs west toward the mill and east
    toward the mere.
  note if not traded: Something red hangs round Clementine's neck.
  exit north foot at 456 226 zone 440 222 472 230
  exit west "The track to the mill runs west along the ridge. (That part of the valley is not painted yet: the mill is for another day.)" zone 14 230 20 312
  exit east "The track east goes down to the mere. (That part of the valley is not painted yet.)" zone 620 230 626 296
  exit south "The path runs back down to Hallow Mere. You have only just come up it, and you're not going home without an answer."
  show goat_key if goat in meadow and not traded
  show goat_plain if goat in meadow and traded
  front boulder_front 104 196 262
  front fence_front 476 626 300

npc goat "Clementine"
  nouns goat clementine nanny
  adjectives white brown keeper's
  in meadow
  proper
  area 300 214 356 250
  stand 330 256
  here: Clementine the goat is grazing on her knoll, chewing with her whole face.
  description:
    A brown-and-white nanny goat with a gold eye, a sardonic beard and a
    red ribbon round her neck. Something heavy and iron hangs from the
    ribbon and swings when she chews.
  talk: Clementine looks at you with one gold eye, then the other, as if checking you in stereo. "Meh," she says. It sounds like a reasonable position.
  topic key, ribbon:
    You ask about the key. Clementine lowers her head, so that the key
    swings out of your reach, and holds your gaze. It is a negotiation,
    and you are losing it.
  topic wenna, keeper, hale, gran:
    At the keeper's name, Clementine looks up the path toward the Spire
    and down the valley toward the mere, and bleats once, quite softly.
  topic tamsin, girl, apprentice:
    Clementine's ears go flat, then flick up again. Whoever Tamsin is,
    the goat has opinions, and some of them involve being fed late.
  topic apple, apples, food, grass:
    Clementine's nostrils widen. She looks, very deliberately, toward the
    tower door, where an apple tree grows.
  topic wind, weather, evening, sun:
    Clementine has been eating the same evening since April. She chews it
    thoughtfully.
  unknown: Clementine regards you as one regards a gate that has started talking.
  idle every 4:
    Clementine pulls up a dandelion clock and eats it, seeds and all. Not one seed escapes.
    Clementine scratches her chin on the boulder, and the key on its ribbon clanks.
    Clementine looks at the Spire, then at you, then at your satchel.

object key "iron key"
  nouns key
  adjectives iron big heavy door
  in goat
  description: A big iron door key, warm from the goat, with a hare worked into its bow.

object ribbon "red ribbon"
  nouns ribbon
  adjectives red silk
  in goat
  description: A long red silk ribbon, the kind a girl would tie in her hair if she could ever find it. Strong stuff.

object boulder "boulder"
  nouns boulder rock stone arrow scratch scratches
  adjectives big grey scratched
  in meadow
  scenery
  area 104 214 196 270
  stand 150 282
  description:
    A boulder the size of a haycart, split into flat grey faces. Someone
    has scratched an arrow on it, pointing at the Spire, and under it, in
    a careful round hand: THIS WAY FOR WEATHER.
  readable: THIS WAY FOR WEATHER. And smaller, in a different, younger hand: NOT TODAY.

object dandelions "dandelion clocks"
  nouns dandelion dandelions clock clocks seeds puff puffs flowers flower
  adjectives dandelion white
  in meadow
  scenery
  plural
  area 20 276 120 312
  stand 70 300
  description: Hundreds of dandelion clocks, every one of them perfect. In any other year the wind would have spent them by May. These have been waiting since spring for somebody to tell the time.

object fence "fence"
  nouns fence rail rails post posts
  adjectives old wooden split
  in meadow
  scenery
  area 476 252 626 312
  stand 470 290
  description: An old split-rail fence leaning toward the mere, as fences do when the wind always comes from one side. This year it has been leaning out of habit.

object spire_far "Spire"
  nouns spire tower stillwind pennant flag roof
  adjectives stillwind old stone tall
  in meadow
  scenery
  area 430 30 500 222
  description:
    The Stillwind Spire: a tall stone tower with a pointed slate roof, a
    window for each floor and, on the very top, a brass hare where most
    towers would keep a cockerel. The pennant under the hare hangs
    straight down. It has the air of a building waiting for something.

object mountains "mountains"
  nouns mountains mountain range peaks ridge notch snow
  adjectives blue far
  in meadow
  scenery
  description: The Coldbacks, blue with distance, snow in their creases. The sun sits in the notch between the two tallest, where it sat in April.

object mill_far "mill"
  nouns mill windmill sails
  adjectives distant still
  in meadow
  scenery
  area 30 120 80 175
  description: Far off to the west on the ridge, Hob Tarrant's windmill, its sails stopped in a cross like a sign that says closed.

object flowers "wildflowers"
  nouns wildflowers buttercups daisies grass meadow
  adjectives wild yellow
  in meadow
  scenery
  plural
  description: Buttercups and clover and long gold grass, every blade upright. A meadow with no wind in it looks like a painting of a meadow.

# --- Clementine and her key ------------------------------------------------------

on examine goat if traded
  say: A brown-and-white nanny goat with a gold eye and a sardonic beard, lighter by one key and one ribbon and heavier by one apple. She looks pleased with the deal.

on take key if key in goat
  say: You reach for the key. Clementine steps neatly sideways, and the key swings away with her. She is not going to give it up for nothing.

on take ribbon if ribbon in goat
  try take key

on untie ribbon if ribbon in goat
  say: You reach for the knot. Clementine reaches for your sleeve with her teeth. You both withdraw with honour.

on take goat
  say: She weighs as much as you do and has firmer views. No.

on pet goat
  say: You scratch Clementine behind the horns. She leans into it, eyes half shut, and for a moment the two of you are friends. Then she checks your satchel for snacks.

on give sweets to goat
  say: Clementine sniffs the humbugs, sneezes violently, and looks at you as if you'd offered her a lit firework. Not a mint goat, then.

on feed goat if carrying apple
  try give apple goat

on feed goat
  say: With what? Clementine would like to know too.

on give apple to goat
  say:
    Clementine takes the apple with great delicacy, crunches it in three
    bites, and lowers her head for you. You lift the ribbon over her
    horns: the iron key, and the red ribbon it hangs on, are yours.
    Clementine burps, and returns to the dandelions.
  move apple to nowhere
  move key to player
  move ribbon to player
  set traded
  achieve trade

on give key to goat if traded
  say: Clementine sniffs the key and declines it. She has made her trade, and she is a goat of her word.

on give any to goat
  say: Clementine looks at {the} and then at you, the way a shopkeeper looks at a button offered for a loaf. No.

on blow dandelions
  say:
    You pick a clock and blow. The seeds lift off, hang in the air beside
    your face, and drift straight back down to the grass, settling almost
    exactly where they were. There is no wind to take them anywhere.
  achieve dandelion

on take dandelions
  say: You pick a dandelion clock. It doesn't shed a single seed. You put it back, feeling that it was not yours to keep.

on smell dandelions
  say: They smell faintly green, and of nothing happening.

on climb boulder
  say: You get one boot on the lowest face, consider the drop, and remember that you're sixty-three. The view from the top is probably of more meadow.

on push boulder
  say: You lean on the boulder. It does not register the fact.

on touch boulder
  say: The stone is warm from months of evening sun. The scratched arrow is sharp-edged and new: cut this year.

on climb fence
  say: You could, but on the other side is more meadow, and the fence would tell everyone.

on listen if in meadow
  say: You listen. No birdsong: the birds gave up singing an evening song in May, as there's never a morning to sing to. Clementine chews. Somewhere a bee bumbles, sounding lost.

on knock boulder
  say: Knock knock. The boulder doesn't answer. Boulders rarely do.

on sit if in meadow
  say: You sit on the grass for a while and look at the Spire. It looks back. Then you get up again, because nothing is going to happen if you don't.
