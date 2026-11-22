#!/usr/bin/env python3
"""Fail if a program cargo built is missing or older than its sources.

    scripts/check-built.py <dir> <program>...

For the tests outside cargo (native/*/test.sh) that run the workspace's
programs from a directory (target/release by default). Cargo writes beside
each program a dependency file, <program>.d ("<program>: <source> ..."); a
source changed after the program was built means the program no longer
corresponds to the sources being tested. A program without that file (a
copy installed elsewhere) cannot be checked, and is said to be so.
"""

import os
import sys
from pathlib import Path


def sources(dep):
    line = dep.split("\n", 1)[0]
    _, sep, rest = line.partition(": ")
    if not sep:
        return []
    files, cur, i = [], "", 0
    while i < len(rest):
        if rest[i] == "\\" and rest[i + 1:i + 2] == " ":
            cur += " "
            i += 2
            continue
        if rest[i] == " ":
            if cur:
                files.append(cur)
            cur = ""
        else:
            cur += rest[i]
        i += 1
    if cur:
        files.append(cur)
    return files


def check(path):
    """Why `path` is not usable, or None."""
    if not path.exists():
        return "not built"
    dep = path.with_name(path.name.removesuffix(".exe") + ".d")
    if not dep.exists():
        print(f"check-built: {path}: no build record beside it, so not checked against the sources", file=sys.stderr)
        return None
    built = path.stat().st_mtime
    for src in sources(dep.read_text(errors="replace")):
        try:
            if os.stat(src).st_mtime > built:
                return f"older than its sources ({src} changed after it was built)"
        except FileNotFoundError:
            return f"older than its sources ({src} is gone)"
    return None


def main():
    if len(sys.argv) < 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    d = Path(sys.argv[1])
    bad = [(n, why) for n in sys.argv[2:] if (why := check(d / n))]
    for n, why in bad:
        print(f"check-built: {d / n}: {why}", file=sys.stderr)
    if bad:
        print("check-built: run scripts/cargo build --release --workspace --bins (or set the directory to programs built from these sources)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
