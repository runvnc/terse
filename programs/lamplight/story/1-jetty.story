# PLACEHOLDER STORY: the jetty, where the story starts.

room jetty "The Jetty"
  description:
    You stand on an old wooden jetty that runs out into the black water of
    the lake. A boathouse crouches on the shore to the east; a path climbs
    the bank to the north, towards the town. Far out, the lights of the
    ferry crawl along the horizon.
  note if open boathouse_door: The boathouse door stands open.
  exit north square
  exit east boathouse through boathouse_door
  exit south "The jetty ends in cold, deep water."
  exit west "The water is cold and deep."
  ambient water every 420
  spot 260 232
  spot 300 216
  spot 380 236
  art
    layer stars colour white
      dots 20 20  64 44  110 14  152 36  205 22  248 50  292 12  340 30  388 18  430 46  476 16  512 64  604 22  622 60  86 70  178 64  402 68
    layer moon colour moon
      circle 560 40 14
      arc 566 36 10 from 100 to 260
    layer shore colour dim
      line 0 104 50 96 110 100 170 86 230 98 300 92 360 99 420 90 480 97 540 88 600 96 640 92
    layer bank colour grey
      line 0 140 60 150 120 176 170 212 205 256
      line 62 256 30 152
      line 112 256 48 158
    layer ripples colour lake drift 8 0 240
      line 20 120 60 120
      line 80 132 130 132
      line 30 150 66 150
      line 160 124 210 124
      line 230 112 270 112
      line 400 116 450 116
      line 140 160 180 160
      line 455 150 490 150
      line 470 230 520 230
      line 560 244 610 244
    layer ferry colour dim drift 30 0 900
      poly 100 104 140 104 134 109 106 109
      line 112 104 113 97 126 97 127 104
    layer ferry_light colour amber drift 30 0 900
      dots 120 93
    layer buoy colour red blink 20 70
      dots 470 132
      line 466 140 474 140
    layer jetty colour plank
      line 210 256 300 140 360 140 450 256
      line 222 240 438 240
      line 236 222 424 222
      line 249 206 411 206
      line 260 192 400 192
      line 269 180 391 180
      line 278 169 382 169
      line 285 159 375 159
      line 292 150 368 150
      line 300 140 300 122
      line 360 140 360 122
      line 296 122 304 122
      line 356 122 364 122
    layer mooring_post colour plank
      line 440 250 440 196
      line 432 196 448 196
    layer carving colour chalk reveal carving_seen "Cut into the post, low down where the rope has worn it smooth: a lamp, and the words NOT TOO LATE." at 440 236
      line 428 242 452 242
      line 431 236 434 230 437 236
      line 443 236 443 230 449 230 449 236
      line 432 248 448 248
    layer wreck hidden colour water
      line 150 190 180 178 205 184
      line 160 196 170 186
      line 186 196 192 184
    layer boardwalk colour plank
      line 446 236 545 204
      line 450 252 575 205
    layer boathouse colour grey
      line 500 205 500 130 565 95 630 130 630 205
      line 492 205 640 205
      line 520 205 520 220
      line 610 205 610 222
    layer door_shut colour grey if closed boathouse_door
      rect 548 152 28 53
      dots 570 180
    layer door_open colour grey if open boathouse_door
      line 548 152 534 158 534 210 548 205
      line 548 152 576 152 576 205
    layer path_sign colour grey
      line 150 256 150 214
      line 136 214 172 214 178 220 172 226 136 226 136 214

object lantern "storm lantern"
  nouns lantern lamp
  adjectives old storm my
  in player
  light
  switchable
  beam
  description:
    A storm lantern with a polished reflector behind the flame. Lit, it
    throws a narrow beam that shows things the dark would rather keep.
  at 320 230
  figure
    layer body colour amber
      rect -5 -14 10 12
      line -5 -2 5 -2
      arc 0 -14 5 from 0 to 180
    layer flame colour flame flicker if on lantern
      line 0 -6 -2 -9 0 -13 2 -9 0 -6

object box "wooden box"
  nouns box crate chest
  adjectives wooden small
  in jetty
  fixed
  container
  openable
  closed
  description: A small wooden box nailed to the planks, the kind fishermen keep their odds and ends in.
  at 330 206
  figure
    layer crate colour plank
      rect -18 -14 36 14
      line -18 -7 18 -7
    layer lid_shut colour plank if closed box
      line -19 -14 19 -14
    layer lid_open colour plank if open box
      line -18 -14 -24 -30 12 -30 18 -14

object brass_key "brass key"
  nouns key
  adjectives brass small yellow
  in box
  description: A small brass key, worn smooth. A paper tag says BOATHOUSE.
  readable: The tag says BOATHOUSE.
  at 320 230
  figure
    layer key colour yellow
      circle -6 -3 3
      line -3 -3 8 -3 8 0

object coin "copper coin"
  nouns coin penny
  adjectives copper old
  in box
  description: An old copper coin, green at the edges.
  at 300 230
  figure
    layer coin colour orange
      circle 0 -3 3

object note "note"
  nouns note paper notice
  adjectives pinned
  in jetty
  fixed
  at 440 214
  description: A square of paper pinned to the mooring post.
  readable:
    |PLACEHOLDER STORY
    |
    |The lamp in the square has gone out. Oil is kept in the boathouse.
    |Ring the bell on the hour. -- the Warden
  figure
    layer paper colour white
      rect -6 -8 12 12
      line -3 -4 3 -4
      line -3 -1 3 -1

object post "mooring post"
  nouns post bollard
  adjectives mooring wooden
  in jetty
  scenery
  at 440 220
  description: A thick post, rope-worn, with a note pinned to it.

object lake "lake"
  nouns lake water waves ripples
  adjectives black cold deep
  in jetty
  scenery
  at 100 140
  description: The water is black and still, except where the ripples catch the moon.

object ferry "ferry"
  nouns ferry boat lights
  adjectives distant far night
  in jetty
  scenery
  at 120 100
  description: The night ferry, a string of small lights far out on the lake. It will not come in to a dark shore.

object boathouse_door "boathouse door"
  nouns door
  adjectives boathouse wooden heavy
  in jetty
  also in boathouse
  door
  locked
  key brass_key
  at 562 178
  description: A heavy door of tarred planks with a brass lock.

on take post
  say: It is sunk deep in the lake bed. It is not going anywhere.

on open boathouse_door if locked boathouse_door
  say: The door is locked fast. The lock is brass.

after open boathouse_door
  sound creak

after open box if not box_scored
  score 1
  set box_scored
  sound creak

on examine post if carving_seen
  say: A thick post, rope-worn. Cut into it, low down: a lamp, and NOT TOO LATE.

on climb post
  say: You shin a little way up the post and see nothing but more dark. You slide back down.

on drink lake
  say: It is very cold and tastes of weed.
