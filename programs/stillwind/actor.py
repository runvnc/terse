#!/usr/bin/env python3
"""Paints Ysolde Fenn, the player character, in the article path syntax
(ARTICLE.md section 6): one layer for each pose, which the engine shows
one at a time, moved, scaled by depth and mirrored with the page's
`layer` call (spec/page.md §page.layers). Every scene takes them in with
`@actor` (paint.py).

    actor.py -o scenes/actor.paths

Each pose is painted at full size, the size she is at the front of a
scene, her feet at (FEET_X, FEET_Y) units, facing right (the side poses),
toward the viewer (front) or away (back). The engine (stillwind.c) knows
the same numbers: FEET_X, FEET_Y and the layer of each pose.

The layers:
  1-5    side, the stride 0, 4, 7, -4, -7 (the front foot's x)
  6-8    toward the viewer, stride 0, 4, -4
  9-11   away, stride 0, 4, -4
Light from the left, as in every scene: the shaded side of the coat is
her back when she faces right; mirrored, the light is then from the
right, which nobody minds in a walk.
"""
import sys

FEET_X, FEET_Y = 24, 314
SIDE = [0, 4, 7, -4, -7]
FRONT = [0, 4, -4]

COAT = "#2f7f7b"
COAT_DARK = "#1f5c5c"
COAT_LINE = "#173f40"
SCARF = "#f1e4c2"
SKIN = "#eec6a2"
HAIR = "#d4d6dc"
HAIR_DARK = "#a9adb8"
BOOTS = "#3d2b22"
SATCHEL = "#b07a3e"
STAFF = "#7a5230"
BRASS = "#e2b048"
EYE = "#2b2230"


def n(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


class Pose:
    def __init__(self):
        self.out = []
        self.dy = 0

    def P(self, x, y):
        return f"{n(FEET_X + x)} {n(FEET_Y + y + self.dy)}"

    def poly(self, colour, *polys, line=None):
        """Filled polygons (lists of (x, y)), outlined in `line` if given."""
        d = " ".join("M" + " L".join(self.P(x, y) for x, y in pts) + " Z" for pts in polys)
        stroke = "stroke none"  # outlines would double the length cap's count (paint.py --costs)
        self.out.append(f"fill {colour} {stroke} {d}")

    def lines(self, colour, width, *segs, ground=False):
        """Round-capped strokes, each a list of points."""
        dy, self.dy = self.dy, 0 if ground else self.dy
        d = " ".join("M" + " L".join(self.P(x, y) for x, y in s) for s in segs)
        self.dy = dy
        self.out.append(f"fill none stroke {colour} {n(width)} {d}")

    def shape(self, colour, polys=(), discs=(), line=None):
        """Polygons and circles in one colour: one path."""
        self.poly(colour, *polys, line=line) if polys else self.out.append("fill none stroke none")
        if discs:
            self.discs(colour, *discs)
            d = self.out.pop()
            self.out[-1] += " " + d.split(" ", 4)[4]

    def discs(self, colour, *discs):
        """Filled circles (x, y, r); one path."""
        parts = []
        K = 0.5523
        for x, y, r in discs:
            X, Y = FEET_X + x, FEET_Y + y + self.dy
            parts.append(f"M{n(X + r)} {n(Y)} C{n(X + r)} {n(Y + r * K)} {n(X + r * K)} {n(Y + r)} {n(X)} {n(Y + r)} "
                         f"C{n(X - r * K)} {n(Y + r)} {n(X - r)} {n(Y + r * K)} {n(X - r)} {n(Y)} "
                         f"C{n(X - r)} {n(Y - r * K)} {n(X - r * K)} {n(Y - r)} {n(X)} {n(Y - r)} "
                         f"C{n(X + r * K)} {n(Y - r)} {n(X + r)} {n(Y - r * K)} {n(X + r)} {n(Y)} Z")
        self.out.append(f"fill {colour} stroke none " + " ".join(parts))


def vane(p, sx, top):
    """The brass weathervane on the staff's top: an arrow and a ball."""
    p.shape(BRASS,
            [[(sx - 6, top - 1), (sx + 3.5, top - 1), (sx + 3.5, top - 2.6), (sx + 7, top), (sx + 3.5, top + 2.6), (sx + 3.5, top + 1),
              (sx - 6, top + 1), (sx - 8, top + 3), (sx - 8, top - 3)]], [(sx, top + 3.6, 1.5)], line=COAT_LINE)


def side(a):
    p = Pose()
    bob = -1 if a in (4, -4) else 0
    p.lines("#00000040", 7, [(-9, 0), (9, 0)], ground=True)
    # Legs: the back one first, then the front one; a lifted foot is higher.
    lift = lambda f: -2.5 if f < 0 and a else 0
    fb, ff = (lift(-a) if a < 0 else 0), (lift(a) if a > 0 else 0)
    p.lines(BOOTS, 4.5, [(-1, -26 + bob), (-a, fb), (-a + 2.5, fb)], [(1, -26 + bob), (a, ff), (a + 2.5, ff)], ground=True)
    p.dy = bob
    flare = a * 0.35
    # The coat: long, A-line, its hem swinging with the stride.
    p.poly(COAT, [(-6.5, -57), (5, -58.5), (9, -50), (11.5 + flare, -13), (2, -11), (-13 + flare, -12.5), (-10, -40)], line=COAT_LINE)
    p.poly(COAT_DARK, [(-6.5, -57), (-1, -58), (-4, -36), (-6 + flare, -12.2), (-13 + flare, -12.5), (-10, -40)])
    # The satchel at her back hip, its strap over the front shoulder.
    p.poly(SATCHEL, [(-13, -39), (-4, -39), (-4.5, -29), (-12.5, -29)], [(3.5, -57.5), (5.5, -56), (-6, -38), (-8.5, -38.5)], line=COAT_LINE)
    # The scarf: round the neck, its end hanging straight down (no wind).
    p.poly(SCARF, [(-5, -60), (6, -60.5), (6.5, -56.5), (-5, -56)], [(3.5, -57.5), (6.5, -57.5), (7, -45), (4.5, -45.5)])
    # The arm and the staff, swinging with the stride.
    hx = 9 - a * 0.45
    sx = hx + 1.2
    p.lines(STAFF, 1.8, [(sx + 1.5, -82), (sx - 1, -1 - bob)])
    vane(p, sx + 1.5, -84)
    p.lines(COAT_DARK, 4.2, [(1.5, -54), (hx - 0.5, -41)])
    # Face and hand (one colour, one path), the hair and its bun, an eye.
    p.shape(SKIN, [[(7.5, -67), (10, -64.5), (7.5, -63.5)]], [(2.5, -66, 6), (hx + 0.5, -40, 1.9)])
    p.shape(HAIR, [[(-4.5, -66), (-3, -71.5), (2.5, -73), (8, -70), (8.6, -68), (3, -69.5), (0.5, -66), (-1, -61), (-4, -61.5)]],
            [(-4.5, -71, 3.2)], line=HAIR_DARK)
    p.discs(EYE, (5.6, -66.2, 0.75))
    return p.out


def front(a, back=False):
    p = Pose()
    bob = -1 if a else 0
    p.lines("#00000040", 7, [(-9, 0), (9, 0)], ground=True)
    lift = lambda side: -2.5 if a and (a > 0) == (side > 0) else 0
    p.lines(BOOTS, 4.5, [(-3, -26 + bob), (-3.5, lift(-1)), (-4, lift(-1))], [(3, -26 + bob), (3.5, lift(1)), (4, lift(1))], ground=True)
    p.dy = bob
    sway = a * 0.25
    coat = COAT_DARK if back else COAT
    p.poly(coat, [(-6.5, -58), (6.5, -58), (9, -50), (12 + sway, -12.5), (-12 + sway, -12.5), (-9, -50)], line=COAT_LINE)
    p.poly(COAT_DARK if not back else COAT_LINE, [(-6.5, -58), (-3, -58), (-6 + sway, -12.5), (-12 + sway, -12.5), (-9, -50)])
    # Satchel: at her left hip, strap across; from behind, the other way.
    s = -1 if not back else 1
    p.poly(SATCHEL, [(s * 13.5, -40), (s * 6, -40), (s * 6.5, -30), (s * 13, -30)], [(-s * 6, -58), (-s * 3.5, -58), (s * 9, -40), (s * 6.5, -40)],
           line=COAT_LINE)
    p.poly(SCARF, [(-6, -61), (6, -61), (6.5, -56.5), (-6.5, -56.5)], *([] if back else [[(0.5, -57), (3.5, -57), (3.5, -45), (0.8, -45.3)]]))
    hx = 11 if not back else -11
    p.lines(STAFF, 1.8, [(hx + (1.5 if not back else -1.5), -82), (hx + (1 if not back else -1), -1 - bob)])
    vane(p, hx + (1.5 if not back else -1.5), -84)
    p.lines(COAT_DARK if not back else COAT_LINE, 4.2, [(7 * (1 if not back else -1), -55), (hx, -42 + sway)])
    if back:
        p.discs(SKIN, (hx, -41 + sway, 1.9))
        p.shape(HAIR, [[(-2.5, -70.2), (0, -68.3), (2.5, -70.2), (0, -69.2)]], [(0, -66, 6.6), (0, -72.5, 3.4)], line=HAIR_DARK)
    else:
        p.discs(SKIN, (0, -66, 6.2), (hx, -41 + sway, 1.9))
        p.shape(HAIR, [[(-6.6, -65), (-5, -71), (0, -73), (5, -71), (6.6, -65), (4.5, -68.5), (0, -70), (-4.5, -68.5)]], [(0, -74.5, 2.6)],
                line=HAIR_DARK)
        p.discs(EYE, (-2.4, -66, 0.75), (2.4, -66, 0.75))
    return p.out


def main():
    if len(sys.argv) != 3 or sys.argv[1] != "-o":
        sys.exit(__doc__)
    lines = ["; Made by actor.py: do not edit. Ysolde's poses, a layer each,",
             f"; her feet at ({FEET_X}, {FEET_Y}) units, at full size."]
    k = 1
    for kind, strides in (("side", SIDE), ("front", FRONT), ("back", FRONT)):
        for a in strides:
            lines.append(f"; layer {k}: {kind}, stride {a}")
            lines.append(f"layer {k}")
            lines.extend(side(a) if kind == "side" else front(a, kind == "back"))
            k += 1
    with open(sys.argv[2], "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


main()
