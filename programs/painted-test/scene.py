#!/usr/bin/env python3
"""Compiles a scene painted in the article path syntax (ARTICLE.md section 6:
M L H V C S Q T Z, lowercase relative, `fill <colour>`, `stroke <colour>
[<width>]`, `layer <n>`) into a page document (spec/page.md §page.document)
of figures stacked one under the other, as a C header the program submits
straight from ROM.

    scene.py <scene.paths> -o <scene_data.h>

Conveniences on top of the article syntax, expanded here:
  ; ...                     a comment, to the end of the line
  @circle cx cy r           a circle as four cubic segments (a subpath)
  @ellipse cx cy rx ry      an ellipse likewise
  @include <file>           the lines of another file (beside this one)
  @copy <name> ... @end     remembers the lines between (and keeps them)
  @paste <name>             those lines again
The first line `size W H` gives the scene's size in units; `split Y ...`
(optional, second) cuts it into figures at those heights: a path goes
into every figure its box reaches, so each figure has caps of its own
(spec/page.md §page.document), and a figure ends where the next begins
(no space between them); a path in a layer goes only into the lowest
figure it reaches. The file starts as an article figure does: no
fill, stroke `fg` 1 wide; the page's `fg` is the document's text colour,
so a scene says `stroke none` first.

`layer <n>` puts the paths after it in layer n (spec/page.md §page.layers):
in the page device the program shows, hides and moves them; in an
article they are drawn where they are. Each figure gets the Layer
commands of the layers it holds paths of.

It prints each figure's use of the caps (paths, area, length; and the
document's Layer commands and layered area), so a painter sees how much
room is left.
"""
import os
import re
import sys

K = 0.5522847498


def circle(cx, cy, rx, ry):
    return (f"M{cx + rx} {cy} C{cx + rx} {cy + ry * K} {cx + rx * K} {cy + ry} {cx} {cy + ry} "
            f"C{cx - rx * K} {cy + ry} {cx - rx} {cy + ry * K} {cx - rx} {cy} "
            f"C{cx - rx} {cy - ry * K} {cx - rx * K} {cy - ry} {cx} {cy - ry} "
            f"C{cx + rx * K} {cy - ry} {cx + rx} {cy - ry * K} {cx + rx} {cy} Z")


def lines_of(path, copies):
    out = []
    recording = None
    for line in open(path).read().splitlines():
        line = line.split(";", 1)[0].strip()
        if not line:
            continue
        f = line.split()
        if f[0] == "@include":
            out.extend(lines_of(os.path.join(os.path.dirname(path), f[1]), copies))
        elif f[0] == "@copy":
            recording = f[1]
            copies[recording] = []
        elif f[0] == "@end":
            recording = None
        elif f[0] == "@paste":
            out.extend(copies[f[1]])
        else:
            out.append(line)
            if recording:
                copies[recording].append(line)
    return out


def expand(path):
    w = h = None
    split = []
    out = []
    for line in lines_of(path, {}):
        f = line.split()
        if f[0] == "size":
            w, h = int(f[1]), int(f[2])
        elif f[0] == "split":
            split = [int(v) for v in f[1:]]
        elif f[0] == "@circle":
            cx, cy, r = map(float, f[1:4])
            out.append(circle(cx, cy, r, r))
        elif f[0] == "@ellipse":
            cx, cy, rx, ry = map(float, f[1:5])
            out.append(circle(cx, cy, rx, ry))
        else:
            out.append(line)
    if w is None:
        sys.exit("scene.py: the first line must be `size W H`")
    return w, h, split, " ".join(out)


TOKEN = re.compile(r"\s*(?:(fill|stroke|layer)\b|([MLHVCSQTZmlhvcsqtz])|(#[0-9a-fA-F]+|none)|([+-]?(?:\d+\.?\d*|\.\d+))|,)")
ARGS = {"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "T": 2, "Z": 0}


def colour(c):
    if c == "none":
        return (0, 0, 0, 0)
    c = c[1:]
    if len(c) == 3:
        c = "".join(ch * 2 for ch in c)
    if len(c) == 6:
        c += "ff"
    if len(c) != 8:
        sys.exit(f"scene.py: bad colour #{c}")
    return tuple(int(c[i:i + 2], 16) for i in range(0, 8, 2))


def six(v):
    """Units to sixteenths, rounded half away from zero."""
    s = v * 16
    r = int(abs(s) + 0.5)
    r = r if s >= 0 else -r
    if not -32768 <= r <= 32767:
        sys.exit(f"scene.py: {v} out of range")
    return r


def tokens(s):
    pos = 0
    while pos < len(s):
        m = TOKEN.match(s, pos)
        if not m or m.end() == pos:
            if s[pos:].strip() == "":
                return
            sys.exit(f"scene.py: cannot read {s[pos:pos + 30]!r}")
        pos = m.end()
        word, cmd, col, num = m.groups()
        if word:
            yield ("word", word)
        elif cmd:
            yield ("cmd", cmd)
        elif col:
            yield ("colour", col)
        elif num is not None:
            yield ("num", float(num))


class Path:
    """A path that draws: its fill (r, g, b, a) or None, stroke (r, g, b, a,
    width in sixteenths) or None, layer (None: the base) and commands
    (opcode, sixteenths...), absolute."""

    def __init__(self, fill, stroke, layer):
        self.fill, self.stroke, self.layer = fill, stroke, layer
        self.ops = []

    def points(self):
        pts, prev = [], None
        for op, *c in self.ops:
            if op == 7:
                continue
            for k in range(0, len(c), 2):
                pts.append((c[k], c[k + 1]))
        return pts

    def box(self):
        """The box of its points, grown by half the stroke width if it is
        stroked (sixteenths)."""
        pts = self.points()
        g = -(-self.stroke[4] // 2) if self.stroke else 0
        return (min(p[0] for p in pts) - g, min(p[1] for p in pts) - g, max(p[0] for p in pts) + g, max(p[1] for p in pts) + g)


def compile_paths(s):
    """The paths that draw, in order."""
    toks = list(tokens(s))
    paths = []
    fill = None
    stroke = None  # set by the scene first (`stroke none`); fg is not known here
    layer = None
    cur = None
    i = 0
    pos = (0.0, 0.0)
    start = (0.0, 0.0)
    last_c = last_q = None
    cmd = None

    def end():
        nonlocal cur
        if cur and any(op != 3 for op, *_ in cur.ops) and (cur.fill or cur.stroke):
            paths.append(cur)
        cur = None

    def path():
        nonlocal cur
        if cur is None:
            cur = Path(fill, stroke, layer)
        return cur

    while i < len(toks):
        kind, v = toks[i]
        if kind == "word":
            end()
            if v == "layer":
                if i + 1 >= len(toks) or toks[i + 1][0] != "num" or not (0 <= toks[i + 1][1] <= 63) or toks[i + 1][1] != int(toks[i + 1][1]):
                    sys.exit("scene.py: layer wants a number 0 to 63")
                layer = int(toks[i + 1][1])
                i += 2
                cmd = None
                continue
            if i + 1 >= len(toks) or toks[i + 1][0] != "colour":
                sys.exit("scene.py: a word without a colour")
            c = colour(toks[i + 1][1])
            i += 2
            if v == "fill":
                fill = c if c[3] else None
            else:
                width = 1.0
                if i < len(toks) and toks[i][0] == "num":
                    width = toks[i][1]
                    i += 1
                stroke = c + (six(width),) if c[3] and six(width) else None
            cmd = None
            last_c = last_q = None
            continue
        if kind == "cmd":
            cmd = v
            i += 1
            if cmd in "Zz":
                if cur and cur.ops and cur.ops[-1][0] != 7:
                    cur.ops.append((7,))
                    pos = start
                cmd = None
                last_c = last_q = None
                continue
        elif kind != "num" or cmd is None:
            sys.exit(f"scene.py: unexpected {v!r}")
        up = cmd.upper()
        n = ARGS[up]
        a = [t[1] for t in toks[i:i + n]]
        if len(a) < n or any(t[0] != "num" for t in toks[i:i + n]):
            sys.exit(f"scene.py: {cmd} without its numbers")
        i += n
        rel = cmd.islower()
        x0, y0 = pos
        if up == "H":
            a = [a[0] + (x0 if rel else 0), y0]
            up, rel = "L", False
        elif up == "V":
            a = [x0, a[0] + (y0 if rel else 0)]
            up, rel = "L", False
        if rel:
            a = [v + (x0 if k % 2 == 0 else y0) for k, v in enumerate(a)]
        if up == "S":
            r = (2 * x0 - last_c[0], 2 * y0 - last_c[1]) if last_c else (x0, y0)
            a = [r[0], r[1]] + a
            up = "C"
        if up == "T":
            r = (2 * x0 - last_q[0], 2 * y0 - last_q[1]) if last_q else (x0, y0)
            a = [r[0], r[1]] + a
            up = "Q"
        p = path()
        if up == "M":
            p.ops.append((3, six(a[0]), six(a[1])))
            start = (a[0], a[1])
            cmd = "l" if rel else "L"
            last_c = last_q = None
        else:
            if not p.ops or p.ops[-1][0] == 7:
                p.ops.append((3, six(x0), six(y0)))
                start = (x0, y0)
            p.ops.append(({"L": 4, "Q": 5, "C": 6}[up], *[six(v) for v in a]))
            last_c = (a[2], a[3]) if up == "C" else None
            last_q = (a[0], a[1]) if up == "Q" else None
        pos = (a[-2], a[-1])
    end()
    return paths


def path_bytes(p, dy, state):
    """The path's commands moved up by dy sixteenths: its Layer, Fill and
    Stroke commands where they change (`state` is the figure's so far)."""
    out = bytearray()
    if p.layer != state.get("layer"):
        out += bytes([8, p.layer])
        state["layer"] = p.layer
    f = p.fill or (0, 0, 0, 0)
    if f != state.get("fill"):
        out += bytes([1, *f])
        state["fill"] = f
    s = p.stroke or (0, 0, 0, 0, 0)
    if s != state.get("stroke"):
        out += bytes([2, *s[:4]]) + s[4].to_bytes(2, "little")
        state["stroke"] = s
    else:
        # Fill and Stroke end a path; consecutive paths in the same colours
        # are kept apart by the same Fill again.
        if not out:
            out += bytes([1, *f])
    for op, *c in p.ops:
        out.append(op)
        for k, v in enumerate(c):
            v = v - dy if k % 2 else v
            out += (v & 0xFFFF).to_bytes(2, "little")
    return bytes(out)


def caps(paths, dy, w, h, budget):
    """A figure's caps (spec/page.md §page.document) and what it adds to the
    document's layer limits (§page.layers)."""
    W, H = w * 16, h * 16
    area = count = 0
    groups = [[]]
    last = None
    layered = any(p.layer is not None for p in paths)
    for p in paths:
        pts = [(x, y - dy) for x, y in p.points()]
        xs = [q[0] for q in pts]
        ys = [q[1] for q in pts]

        def box(g):
            return (max(0, min(xs) - g), max(0, min(ys) - g), min(W, max(xs) + g), min(H, max(ys) + g))

        def a(b):
            return -(-max(0, b[2] - b[0]) * max(0, b[3] - b[1]) // 256) if b[2] > b[0] and b[3] > b[1] else 0

        boxes = []
        if p.fill:
            boxes.append(box(0))
        if p.stroke:
            boxes.append(box(-(-p.stroke[4] // 2)))
        area += sum(a(b) for b in boxes)
        length = segs = 0
        prev = None
        for op, *c in p.ops:
            if op == 3:
                prev = (c[0], c[1])
            elif op != 7:
                for k in range(0, len(c), 2):
                    length += abs(c[k] - prev[0]) + abs(c[k + 1] - prev[1])
                    prev = (c[k], c[k + 1])
                segs += 1
        c = 0
        if p.fill:
            c += 2 * length + 4 * 16 * segs
        if p.stroke:
            c += 2 * length + 4 * p.stroke[4] * segs
        count += -(-c // 16)
        if p.layer != last:
            groups.append([])
            last = p.layer
        groups[-1].extend(b for b in boxes if b[2] > b[0] and b[3] > b[1])
    if layered:
        for bs in groups:
            if bs:
                b = (min(q[0] for q in bs), min(q[1] for q in bs), max(q[2] for q in bs), max(q[3] for q in bs))
                budget["area"] += -(-(b[2] - b[0]) * (b[3] - b[1]) // 256)
    return len(paths), area, min(8 * w * h, 4 * 640 * 480), count


def main():
    args = sys.argv[1:]
    if len(args) != 3 or args[1] != "-o":
        sys.exit(__doc__)
    w, h, split, s = expand(args[0])
    paths = compile_paths(s)
    cuts = [0] + split + [h]
    doc = bytearray()
    notes = []
    budget = {"area": 0}
    over = False
    for top, bottom in zip(cuts, cuts[1:]):
        fh = bottom - top
        # A path in a layer goes into the lowest figure it reaches only (a
        # copy of scenery in front of an actor must not paint over what is
        # above it in a figure the actor never enters).
        reach = lambda b: b[1] < bottom * 16 and b[3] > top * 16
        lowest = lambda b: b[3] <= bottom * 16 or bottom == h
        mine = [p for p in paths if reach(p.box()) and (p.layer is None or lowest(p.box()))]
        state = {"layer": None}
        body = b"".join(path_bytes(p, top * 16, state) for p in mine)
        n, area, area_cap, length = caps(mine, top * 16, w, fh, budget)
        layers = sorted({p.layer for p in mine if p.layer is not None})
        doc += bytes([8]) + w.to_bytes(2, "little") + fh.to_bytes(2, "little") + bytes([0]) + len(body).to_bytes(2, "little") + body
        notes.append(f"figure {w} x {fh} at {top}: {len(body)} bytes, {n}/256 paths, area {area}/{area_cap}, length {length}/65536, layers {layers or 'none'}")
        over |= n > 256 or area > area_cap or length > 65536
    doc += bytes([0])
    # Layer commands: counted by walking the documents' path bytes.
    layer_cmds = count_layer_commands(doc)
    notes.append(f"layers: {layer_cmds}/64 Layer commands, layered area {budget['area']}/614400")
    over |= layer_cmds > 64 or budget["area"] > 614400
    lines = [
        "/* Made by scene.py from scene.paths: do not edit. The page document:",
        *[f" * {n}" for n in notes],
        " * (Caps: spec/page.md §page.document, §page.layers.) */",
        f"#define SCENE_W {w}",
        f"#define SCENE_H {h}",
        f"static const unsigned char scene_doc[{len(doc)}] = {{",
    ]
    for k in range(0, len(doc), 16):
        lines.append("    " + ", ".join(str(b) for b in doc[k:k + 16]) + ",")
    lines.append("};")
    open(args[2], "w").write("\n".join(lines) + "\n")
    for n in notes:
        print("scene: " + n)
    if over:
        sys.exit("scene.py: a figure passes a cap: the host stops drawing it there")


def count_layer_commands(doc):
    """Layer commands in the document's figures."""
    n, at = 0, 0
    sizes = {1: 4, 2: 6, 3: 4, 4: 4, 5: 8, 6: 12, 7: 0, 8: 1}
    while at < len(doc) and doc[at] == 8:
        ln = int.from_bytes(doc[at + 6:at + 8], "little")
        p, end = at + 8, at + 8 + ln
        while p < end:
            op = doc[p]
            n += op == 8
            p += 1 + sizes[op]
        at = end
    return n


main()
