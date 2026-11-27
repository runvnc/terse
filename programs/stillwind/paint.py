#!/usr/bin/env python3
"""Paints a Stillwind scene: compiles a scene written in the article path
syntax (ARTICLE.md section 6: M L H V C S Q T Z, lowercase relative,
`fill <colour>`, `stroke <colour> [<width>]`, `layer <n>`) into a page
document (spec/page.md §page.document) of figures stacked one under the
other. story.py calls it for every room's `scene`; on its own it prints a
scene's use of the caps:

    paint.py <scene.paths> [--actor <actor.paths>]

Conveniences on top of the article syntax, expanded here:
  ; ...                     a comment, to the end of the line
  @circle cx cy r           a circle as four cubic segments (a subpath)
  @ellipse cx cy rx ry      an ellipse likewise
  @include <file>           the lines of another file (beside this one)
  @copy <name> ... @end     remembers the lines between (and keeps them)
  @paste <name>             those lines again
  @actor                    the player character's poses (layers 1 to
                            ACTOR_LAYERS, made by actor.py)
  layer <name>              a named layer: story.py gives it a number
                            after the actor's, in order of first use
The first line `size W H` gives the scene's size in units; `split Y ...`
(optional) and `background #rrggbb` (the page's colour behind the
figures, which costs nothing against the caps: the colour most of the
lowest figure would otherwise fill) cuts it into figures at those heights: a path goes
into every figure its box reaches, so each figure has caps of its own
(spec/page.md §page.document), and a figure ends where the next begins
(no space between them); a path in a layer goes only into the lowest
figure it reaches (a layer is clipped to its figure, so a moving layer,
the actor, must be painted where its figure is). The file starts as an
article figure does: no fill, stroke `fg` 1 wide; the page's `fg` is the
document's text colour, so a scene says `stroke none` first.
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


ACTOR_LAYERS = 11   # actor.py's poses: layers 1 to 11


class PaintError(Exception):
    pass


def lines_of(path, copies, actor):
    out = []
    recording = None
    for n, line in enumerate(open(path, encoding='utf-8').read().splitlines(), 1):
        line = line.split(";", 1)[0].strip()
        if not line:
            continue
        f = line.split()
        if f[0] == "@include":
            out.extend(lines_of(os.path.join(os.path.dirname(path), f[1]), copies, actor))
        elif f[0] == "@actor":
            if not actor:
                raise PaintError(f"{path}:{n}: @actor, but no actor's poses were given")
            out.extend(lines_of(actor, copies, None))
            out.append("stroke none")
        elif f[0] == "@copy":
            recording = f[1]
            copies[recording] = []
        elif f[0] == "@end":
            recording = None
        elif f[0] == "@paste":
            if f[1] not in copies:
                raise PaintError(f"{path}:{n}: @paste {f[1]}: nothing was copied under that name")
            out.extend(copies[f[1]])
        else:
            out.append(line)
            if recording:
                copies[recording].append(line)
    return out


def expand(path, actor=None, names=None):
    """The scene's size, cuts and path text; named layers are given numbers
    in `names` (name -> number), after the actor's."""
    w = h = None
    split = []
    out = []
    expand.background = 0x26386C
    names = {} if names is None else names
    for line in lines_of(path, {}, actor):
        f = line.split()
        if f[0] == "size":
            w, h = int(f[1]), int(f[2])
        elif f[0] == "split":
            split = [int(v) for v in f[1:]]
        elif f[0] == "background":
            # The page's colour behind the figures: free of the caps.
            if len(f) != 2 or not re.match(r"^#[0-9a-fA-F]{6}$", f[1]):
                raise PaintError(f"{path}: background #rrggbb")
            expand.background = int(f[1][1:], 16)
        elif f[0] == "@circle":
            cx, cy, r = map(float, f[1:4])
            out.append(circle(cx, cy, r, r))
        elif f[0] == "@ellipse":
            cx, cy, rx, ry = map(float, f[1:5])
            out.append(circle(cx, cy, rx, ry))
        elif f[0] == "layer" and len(f) == 2 and re.match(r"^[a-z][a-z0-9_]*$", f[1]):
            if f[1] not in names:
                names[f[1]] = ACTOR_LAYERS + 1 + len(names)
            out.append(f"layer {names[f[1]]}")
        else:
            out.append(line)
    if w is None:
        raise PaintError(f"{path}: the first line must be `size W H`")
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
        raise PaintError(f"bad colour #{c}")
    return tuple(int(c[i:i + 2], 16) for i in range(0, 8, 2))


def six(v):
    """Units to sixteenths, rounded half away from zero."""
    s = v * 16
    r = int(abs(s) + 0.5)
    r = r if s >= 0 else -r
    if not -32768 <= r <= 32767:
        raise PaintError(f"{v} out of range")
    return r


def tokens(s):
    pos = 0
    while pos < len(s):
        m = TOKEN.match(s, pos)
        if not m or m.end() == pos:
            if s[pos:].strip() == "":
                return
            raise PaintError(f"cannot read {s[pos:pos + 30]!r}")
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
                    raise PaintError("layer wants a number 0 to 63")
                layer = int(toks[i + 1][1])
                i += 2
                cmd = None
                continue
            if i + 1 >= len(toks) or toks[i + 1][0] != "colour":
                raise PaintError("a word without a colour")
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
            raise PaintError(f"unexpected {v!r}")
        up = cmd.upper()
        n = ARGS[up]
        a = [t[1] for t in toks[i:i + n]]
        if len(a) < n or any(t[0] != "num" for t in toks[i:i + n]):
            raise PaintError(f"{cmd} without its numbers")
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


def paint(path, actor=None, names=None):
    """The scene at `path` as a page document: (bytes, notes, over, caps),
    notes being each figure's use of the caps and the document's layers,
    over whether a cap is passed (the host would stop drawing there), caps
    a list of (paths, area, area cap, length) per figure. Named layers are
    numbered in `names`."""
    w, h, split, s = expand(path, actor, names)
    paths = compile_paths(s)
    cuts = [0] + split + [h]
    doc = bytearray()
    notes = []
    figs = []
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
        figs.append((n, area, area_cap, length))
        layers = sorted({p.layer for p in mine if p.layer is not None})
        doc += bytes([8]) + w.to_bytes(2, "little") + fh.to_bytes(2, "little") + bytes([0]) + len(body).to_bytes(2, "little") + body
        note = f"figure {w} x {fh} at {top}: {len(body)} bytes, {n}/256 paths, area {area}/{area_cap}, length {length}/65536"
        notes.append(note + (f", layers {layers[0]}-{layers[-1]}" if layers else ""))
        over |= n > 256 or area > area_cap or length > 65536
    doc += bytes([0])
    layer_cmds = count_layer_commands(doc)
    notes.append(f"layers: {layer_cmds}/64 Layer commands, layered area {budget['area']}/614400")
    over |= layer_cmds > 64 or budget["area"] > 614400
    return bytes(doc), notes, over, dict(figures=figs, layer_cmds=layer_cmds, layered=budget["area"], w=w, h=h, cuts=cuts,
                                         background=expand.background)


def preview(path, out, actor=None, show=(), scale=2, place=None):
    """Draws the scene to a PNG (needs Pillow) for painting: the base and
    the layers in `show` (numbers or names), each where it is painted, or
    for a layer in `place` (number -> (x, y, scale 256ths, mirrored)) moved
    as the page's `layer` call moves it. Curves are flattened; a quick
    look, not the host's drawing (that is `terse-host --drive` and `shot`)."""
    from PIL import Image, ImageDraw
    names = {}
    w, h, split, s = expand(path, actor, names)
    paths = compile_paths(s)
    want = set()
    for v in show:
        want.add(int(v) if str(v).isdigit() else names.get(v, -1))
    place = place or {}
    bg = expand.background
    img = Image.new("RGBA", (w * scale, h * scale), (bg >> 16, bg >> 8 & 255, bg & 255, 255))

    def flat(ops):
        subs, cur, pos = [], [], (0, 0)
        for op, *c in ops:
            if op == 3:
                if len(cur) > 1:
                    subs.append((cur, False))
                cur = [(c[0], c[1])]
                pos = (c[0], c[1])
            elif op == 4:
                cur.append((c[0], c[1]))
                pos = (c[0], c[1])
            elif op in (5, 6):
                pts = [pos] + [(c[k], c[k + 1]) for k in range(0, len(c), 2)]
                for i in range(1, 13):
                    t = i / 12
                    q = pts
                    while len(q) > 1:
                        q = [(a[0] + (b2[0] - a[0]) * t, a[1] + (b2[1] - a[1]) * t) for a, b2 in zip(q, q[1:])]
                    cur.append(q[0])
                pos = (c[-2], c[-1])
            elif op == 7:
                if len(cur) > 1:
                    subs.append((cur, True))
                pos = cur[0] if cur else pos
                cur = [pos]
        if len(cur) > 1:
            subs.append((cur, False))
        return subs

    for p in paths:
        if p.layer is not None and p.layer not in want:
            continue
        tx = lambda x, y: (x / 16 * scale, y / 16 * scale)
        if p.layer in place:
            ox, oy, sc, mir = place[p.layer]
            tx = lambda x, y, ox=ox, oy=oy, sc=sc, mir=mir: (((ox - sc * x / 256) if mir else (ox + sc * x / 256)) / 16 * scale,
                                                         (oy + sc * y / 256) / 16 * scale)
        layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
        d = ImageDraw.Draw(layer)
        subs = [([tx(x, y) for x, y in pts], closed) for pts, closed in flat(p.ops)]
        if p.fill:
            for pts, closed in subs:
                if len(pts) > 2:
                    d.polygon(pts, fill=tuple(p.fill))
        if p.stroke:
            sw = max(1, round(p.stroke[4] / 16 * scale * (place[p.layer][2] / 256 if p.layer in place else 1)))
            col = tuple(p.stroke[:4])
            for pts, closed in subs:
                q = pts + [pts[0]] if closed else pts
                d.line(q, fill=col, width=sw, joint="curve")
                r = sw / 2
                for x, y in (q[0], q[-1]):
                    d.ellipse((x - r, y - r, x + r, y + r), fill=col)
        img = Image.alpha_composite(img, layer)
    img.convert("RGB").save(out)
    return names


def costs(path, actor=None, top=0, n=20):
    """The paths of the figure that starts at `top`, dearest first by the
    length cap (spec/page.md §page.document): (count, path number, layer,
    fill, top y, bottom y), and the totals. For a painter looking for what
    to simplify."""
    w, h, split, s = expand(path, actor, {})
    paths = compile_paths(s)
    cuts = [0] + split + [h]
    bottom = cuts[cuts.index(top) + 1]
    rows = []
    for i, p in enumerate(paths):
        b = p.box()
        if not (b[1] < bottom * 16 and b[3] > top * 16):
            continue
        if p.layer is not None and not (b[3] <= bottom * 16 or bottom == h):
            continue
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
        cnt = (2 * length + 64 * segs if p.fill else 0) + (2 * length + 4 * p.stroke[4] * segs if p.stroke else 0)
        rows.append((-(-cnt // 16), i, -1 if p.layer is None else p.layer, p.fill, b[1] // 16, b[3] // 16))
    rows.sort(reverse=True)
    print(f"figure at {top}: {len(rows)} paths, length {sum(r[0] for r in rows)}, "
          f"of which layers 1-{ACTOR_LAYERS} (the actor) {sum(r[0] for r in rows if 1 <= r[2] <= ACTOR_LAYERS)}")
    for r in rows[:n]:
        print("  ", r)


def main():
    import argparse
    ap = argparse.ArgumentParser(description="Compiles a scene and prints its use of the page's caps; --png draws it.")
    ap.add_argument("scene")
    ap.add_argument("--actor", help="the actor's poses (actor.paths) for @actor")
    ap.add_argument("--png", help="draw the scene to this PNG (a quick look)")
    ap.add_argument("--show", default="", help="with --png: layers to show, numbers or names, comma-separated")
    ap.add_argument("--costs", type=int, metavar="TOP", help="list the dearest paths of the figure starting at TOP")
    ap.add_argument("--at", action="append", default=[], help="with --png: LAYER,X,Y,SCALE[,m] places a layer as `layer` would (units)")
    a = ap.parse_args()
    try:
        doc, notes, over, _ = paint(a.scene, a.actor)
        if a.costs is not None:
            costs(a.scene, a.actor, a.costs)
        if a.png:
            place = {}
            show = [v for v in a.show.split(",") if v]
            for s in a.at:
                f = s.split(",")
                place[int(f[0])] = (float(f[1]) * 16, float(f[2]) * 16, int(f[3]), len(f) > 4)
                show.append(f[0])
            preview(a.scene, a.png, a.actor, show, place=place)
    except PaintError as e:
        sys.exit(f"paint.py: {e}")
    print(f"{a.scene}: {len(doc)} bytes")
    for n in notes:
        print("  " + n)
    if over:
        sys.exit("paint.py: a figure passes a cap: the host stops drawing it there")


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


if __name__ == "__main__":
    main()
