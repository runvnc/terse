#!/usr/bin/env python3
"""Compiles a scene painted in the article path syntax (ARTICLE.md section 6:
M L H V C S Q T Z, lowercase relative, `fill <colour>`, `stroke <colour>
[<width>]`) into a page document (spec/page.md §page.document) that holds
one figure, as a C header the program submits straight from ROM.

    scene.py <scene.paths> -o <scene_data.h>

Two conveniences on top of the article syntax, expanded here:
  ; ...                     a comment, to the end of the line
  @circle cx cy r           a circle as four cubic segments (a subpath)
  @ellipse cx cy rx ry      an ellipse likewise
The first line `size W H` gives the figure's size in units. The file
starts as an article figure does: no fill, stroke `fg` 1 wide; the page's
`fg` is the document's text colour, so a scene says `stroke none` first.

It prints the figure's use of the page device's caps (paths, area,
length), so a painter sees how much room is left.
"""
import re
import sys

K = 0.5522847498


def circle(cx, cy, rx, ry):
    return (f"M{cx + rx} {cy} C{cx + rx} {cy + ry * K} {cx + rx * K} {cy + ry} {cx} {cy + ry} "
            f"C{cx - rx * K} {cy + ry} {cx - rx} {cy + ry * K} {cx - rx} {cy} "
            f"C{cx - rx} {cy - ry * K} {cx - rx * K} {cy - ry} {cx} {cy - ry} "
            f"C{cx + rx * K} {cy - ry} {cx + rx} {cy - ry * K} {cx + rx} {cy} Z")


def expand(text):
    w = h = None
    out = []
    for line in text.splitlines():
        line = line.split(";", 1)[0].strip()
        if not line:
            continue
        f = line.split()
        if f[0] == "size":
            w, h = int(f[1]), int(f[2])
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
    return w, h, " ".join(out)


TOKEN = re.compile(r"\s*(?:(fill|stroke)\b|([MLHVCSQTZmlhvcsqtz])|(#[0-9a-fA-F]+|none)|([+-]?(?:\d+\.?\d*|\.\d+))|,)")
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


def compile_path(s):
    """Returns the path command bytes and the caps' counts."""
    out = bytearray()
    toks = list(tokens(s))
    i = 0
    cur = (0.0, 0.0)
    start = (0.0, 0.0)
    last_c = last_q = None
    # Caps (spec/page.md §page.document): per path its box and length.
    paths = []
    fill = (0, 0, 0, 0)
    stroke = (0, 0, 0, 0, 0)
    pts = []
    length = 0
    segs = 0

    def end_path():
        nonlocal pts, length, segs
        if segs and (fill[3] or (stroke[3] and stroke[4])):
            paths.append((fill[3] > 0, stroke[3] > 0 and stroke[4] > 0, stroke[4], pts, length, segs))
        pts, length, segs = [], 0, 0

    def emit(op, *coords):
        nonlocal length, segs
        out.append(op)
        q = [six(v) for v in coords]
        for v in q:
            out.extend((v & 0xFFFF).to_bytes(2, "little"))
        return q

    cmd = None
    while i < len(toks):
        kind, v = toks[i]
        if kind == "word":
            end_path()
            if i + 1 >= len(toks) or toks[i + 1][0] != "colour":
                sys.exit("scene.py: a word without a colour")
            c = colour(toks[i + 1][1])
            i += 2
            if v == "fill":
                fill = c
                out.append(1)
                out.extend(c)
            else:
                width = 1.0
                if i < len(toks) and toks[i][0] == "num":
                    width = toks[i][1]
                    i += 1
                stroke = c + (six(width),)
                out.append(2)
                out.extend(c)
                out.extend(six(width).to_bytes(2, "little"))
            cmd = None
            last_c = last_q = None
            continue
        if kind == "cmd":
            cmd = v
            i += 1
            if cmd in "Zz":
                if segs or pts:
                    out.append(7)
                    cur = start
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
        x0, y0 = cur
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
        prev = (six(x0), six(y0))
        if up == "M":
            emit(3, *a)
            start = (a[0], a[1])
            pts.append((six(a[0]), six(a[1])))
            cmd = "l" if rel else "L"
            last_c = last_q = None
        else:
            op = {"L": 4, "Q": 5, "C": 6}[up]
            q = emit(op, *a)
            p = prev
            for k in range(0, len(q), 2):
                length += abs(q[k] - p[0]) + abs(q[k + 1] - p[1])
                p = (q[k], q[k + 1])
                pts.append(p)
            pts.append(prev)
            segs += 1
            last_c = (a[2], a[3]) if up == "C" else None
            last_q = (a[0], a[1]) if up == "Q" else None
        cur = (a[-2], a[-1])
    end_path()
    return bytes(out), paths


def caps(paths, w, h):
    area = 0
    count = 0
    W, H = w * 16, h * 16
    for filled, stroked, sw, pts, length, segs in paths:
        xs = [p[0] for p in pts]
        ys = [p[1] for p in pts]

        def box(g):
            x0, x1 = max(0, min(xs) - g), min(W, max(xs) + g)
            y0, y1 = max(0, min(ys) - g), min(H, max(ys) + g)
            return max(0, x1 - x0) * max(0, y1 - y0)

        if filled:
            area += -(-box(0) // 256)
        if stroked:
            area += -(-box(-(-sw // 2)) // 256)
        c = 0
        if filled:
            c += 2 * length + 4 * 16 * segs
        if stroked:
            c += 2 * length + 4 * sw * segs
        count += -(-c // 16)
    return len(paths), area, min(8 * w * h, 4 * 640 * 480), count


def main():
    args = sys.argv[1:]
    if len(args) != 3 or args[1] != "-o":
        sys.exit(__doc__)
    w, h, s = expand(open(args[0]).read())
    path, paths = compile_path(s)
    n, area, area_cap, length = caps(paths, w, h)
    doc = bytes([8]) + w.to_bytes(2, "little") + h.to_bytes(2, "little") + bytes([0]) + len(path).to_bytes(2, "little") + path + bytes([0])
    lines = [
        "/* Made by scene.py from scene.paths: do not edit. The page document:",
        f" * one figure {w} x {h} units, {len(path)} bytes of path commands.",
        f" * Caps (spec/page.md §page.document): {n} of 256 paths, area {area} of {area_cap},",
        f" * length {length} of 65536. */",
        f"#define SCENE_W {w}",
        f"#define SCENE_H {h}",
        f"static const unsigned char scene_doc[{len(doc)}] = {{",
    ]
    for k in range(0, len(doc), 16):
        lines.append("    " + ", ".join(str(b) for b in doc[k:k + 16]) + ",")
    lines.append("};")
    open(args[2], "w").write("\n".join(lines) + "\n")
    print(f"scene: {len(doc)} bytes, {n}/256 paths, area {area}/{area_cap}, length {length}/65536")
    if n > 256 or area > area_cap or length > 65536:
        sys.exit("scene.py: the figure passes a cap: the host stops drawing it there")


main()
