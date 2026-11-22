#!/usr/bin/env python3
"""Copies source files into the manual's articles, so that a listing is
always the file it names.

A fence whose opening line is three backquotes, a language word and a
path from the repository's top is a listing of that file:

    ```c examples/program/c/hello.c

and one with two more words, each between slashes, is the part of the
file between the first line holding the first text and the next line
holding the second (both left out, and blank lines at either end):

    ```c native/driver-experimental/dice.c /the device ===/ /the bus ===/

The reader ignores the words after the backquotes (ARTICLE.md 2.4). This
script replaces each listing's lines with the file's; with --check it
changes nothing and lists the listings that differ (status 1).
`cargo test -p terse-cli --test examples` checks the same.

    scripts/manual-listings.py            (from the repository's top)
    scripts/manual-listings.py --check
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OPENING = re.compile(r"^```(\w+) (\S+/\S+)(?: /([^/]+)/ /([^/]+)/)?\s*$")


def excerpt(path, start, end):
    lines = (ROOT / path).read_text().splitlines()
    if start:
        first = next(i for i, l in enumerate(lines) if start in l)
        last = next(i for i in range(first + 1, len(lines)) if end in lines[i])
        lines = lines[first + 1:last]
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    return lines


def listings(text):
    """Yields (index of the opening line, index of the closing line, the
    file's lines) for each listing of an article's lines."""
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        m = OPENING.match(lines[i])
        if m:
            close = next(j for j in range(i + 1, len(lines)) if lines[j].rstrip() == "```")
            yield i, close, excerpt(m.group(2), m.group(3), m.group(4))
            i = close
        elif lines[i].startswith("```"):
            i = next(j for j in range(i + 1, len(lines)) if lines[j].rstrip() == "```")
        i += 1


def main():
    check = "--check" in sys.argv[1:]
    stale = []
    for article in sorted((ROOT / "docs/manual").glob("*.txt")):
        text = article.read_text()
        lines = text.split("\n")
        changed = False
        for opening, close, want in reversed(list(listings(text))):
            if lines[opening + 1:close] != want:
                stale.append(f"{article.relative_to(ROOT)}:{opening + 1}")
                lines[opening + 1:close] = want
                changed = True
        if changed and not check:
            article.write_text("\n".join(lines))
    for s in stale:
        print(("differs: " if check else "updated: ") + s)
    return 1 if check and stale else 0


if __name__ == "__main__":
    sys.exit(main())
