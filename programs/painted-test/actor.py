#!/usr/bin/env python3
"""Paints the traveller's poses in the article path syntax (ARTICLE.md
section 6), each in a layer of its own, for scene.py to put in the scene's
figure (spec/page.md §page.layers): the program shows one pose at a time,
moved, scaled and mirrored with the page's `layer` call.

    actor.py -o actor.paths

Every pose is painted at full size facing right (or toward the viewer, or
away), its feet at (FEET_X, FEET_Y) units; painted-test.c places them.
The drawing is the one the vector version drew (quarter units, feet at
0, 0, y up negative): shadow, legs, cloak, belt, staff and its light,
arm, head, beard, eyes, hat.

The layers (painted-test.c LAYER_*):
  1-5    side poses, stride 0, 12, 20, -12, -20
  6-8    toward the viewer, stride 0, 12, -12
  9-11   away, stride 0, 12, -12
"""
import sys

FEET_X, FEET_Y = 30, 344
SIDE = [0, 12, 20, -12, -20]
FRONT = [0, 12, -12]

CLOAK = "#2e7a78"
CLOAK_DARK = "#215c5a"
HAT = "#3b2f6e"
GOLD = "#e0b84a"
SKIN = "#eac6a0"
BEARD = "#e6e2da"
BOOTS = "#3a2a20"
STAFF = "#8a5a2c"


def u(v):
    """Quarter units to units, as text."""
    s = f"{v / 4:.4f}".rstrip("0").rstrip(".")
    return s if s != "-0" else "0"


class Pose:
    def __init__(self):
        self.out = []
        self.fy = 0  # the body's offset (bob), quarter units

    def X(self, lx):
        return FEET_X * 4 + lx

    def Y(self, ly):
        return FEET_Y * 4 + ly + self.fy

    def stroke(self, colour, width_q, segs):
        """Round-capped lines of width `width_q` quarter units."""
        d = " ".join(f"M{u(self.X(a))} {u(self.Y(b))} L{u(self.X(c))} {u(self.Y(e))}" for a, b, c, e in segs)
        self.out.append(f"fill none stroke {colour} {u(width_q)} {d}")

    def fill(self, colour, shapes):
        """Filled shapes: ('tri', x0, y0, x1, y1, x2, y2), ('box', x0, y0,
        x1, y1) or ('disc', x, y, r), quarter units."""
        parts = []
        for s in shapes:
            if s[0] == "tri":
                _, a, b, c, d, e, f = s
                parts.append(f"M{u(self.X(a))} {u(self.Y(b))} L{u(self.X(c))} {u(self.Y(d))} L{u(self.X(e))} {u(self.Y(f))} Z")
            elif s[0] == "box":
                _, a, b, c, d = s
                parts.append(f"M{u(self.X(a))} {u(self.Y(b))} H{u(self.X(c))} V{u(self.Y(d))} H{u(self.X(a))} Z")
            else:
                _, x, y, r = s
                parts.append(f"@circle {u(self.X(x))} {u(self.Y(y))} {u(r)}")
        self.out.append(f"stroke none fill {colour}")
        self.out.extend(parts)


def pose(facing, a):
    """facing: 'side', 'front' (toward the viewer) or 'back'."""
    p = Pose()
    side = facing == "side"
    bob = 0 if a in (20, -20, 0) else -4
    # Shadow: a fat translucent line, its round caps the ends of an ellipse.
    p.stroke("#00000046", 26, [(-36, 0, 36, 0)])
    # Legs.
    if side:
        p.stroke(BOOTS, 15, [(0, -88 + bob, -a, -6 if a < 0 else 0), (0, -88 + bob, a, -6 if a > 0 else 0)])
    else:
        p.stroke(BOOTS, 15, [(-10, -88 + bob, -12, -10 if a > 0 else 0), (10, -88 + bob, 12, -10 if a < 0 else 0)])
    p.fy = bob
    # Cloak and its shaded side.
    cloak = CLOAK_DARK if facing == "back" else CLOAK
    p.fill(cloak, [("tri", 0, -188, -50, -56, 48, -56)])
    shade = ("tri", 0, -188, -50, -56, -18, -56) if side else ("tri", 0, -188, 22, -56, 48, -56)
    p.fill(CLOAK_DARK, [shade])
    p.fill(GOLD, [("box", -24, -117, 24, -109)])
    # Arm and staff, swinging a little with the stride.
    sx = 44 - a // 2 if side else 58
    p.stroke(STAFF, 9, [(sx - 2, -214, sx + 2, -2)])
    p.fill("#ffe58a3c", [("disc", sx - 2, -218, 26)])
    p.fill("#fff2b8", [("disc", sx - 2, -218, 10)])
    p.stroke(CLOAK_DARK, 15, [(6 if side else 26, -160, sx - 6, -124)])
    # Head: the face from the front or the side, the hair from behind; the
    # hand with it (one colour, one path).
    if facing == "back":
        p.fill(SKIN, [("disc", sx - 2, -124, 8)])
        p.fill("#b9b4aa", [("disc", 0, -202, 22)])
    else:
        p.fill(SKIN, [("disc", 4 if side else 0, -204, 22), ("disc", sx - 2, -124, 8)])
        p.fill(BEARD, [("tri", -6, -198, 24, -198, 12, -156) if side else ("tri", -18, -198, 18, -198, 0, -150)])
        p.fill("#2a2030", [("disc", 17, -210, 3)] if side else [("disc", -8, -210, 3), ("disc", 8, -210, 3)])
    # Hat: the brim, the cone, the gold band.
    p.stroke(HAT, 12, [(-42, -220, 44, -220)])
    p.fill(HAT, [("tri", -26, -220, 30, -220, -16, -294) if side else ("tri", -27, -220, 27, -220, 6, -298)])
    p.stroke(GOLD, 6, [(-23, -228, 24, -228)])
    return p.out


def main():
    if len(sys.argv) != 3 or sys.argv[1] != "-o":
        sys.exit(__doc__)
    lines = ["; Made by actor.py: do not edit. The traveller's poses, a layer each.",
             f"; Feet at ({FEET_X}, {FEET_Y}) units, full size, facing right (side poses)."]
    n = 1
    for facing, strides in (("side", SIDE), ("front", FRONT), ("back", FRONT)):
        for a in strides:
            lines.append(f"; layer {n}: {facing}, stride {a}")
            lines.append(f"layer {n}")
            lines.extend(pose(facing, a))
            n += 1
    open(sys.argv[2], "w").write("\n".join(lines) + "\n")


main()
