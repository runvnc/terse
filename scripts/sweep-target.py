#!/usr/bin/env python3
"""Delete old build copies from target/, keeping the newest 3 of each.

    scripts/sweep-target.py              # keep 3
    scripts/sweep-target.py --keep 2
    scripts/sweep-target.py --dry-run    # report only

Cargo never deletes old builds: each change of flags, features or profile
leaves another hashed copy of a library, binary or test behind. cargo-sweep
0.8 does not understand the target/<profile>/build/<crate>/<hash>/ layout,
hence this script. Anything deleted is rebuilt by cargo when next needed.
"""

import argparse
import os
import shutil
import sys
from collections import defaultdict
from pathlib import Path

TARGET = Path(__file__).resolve().parent.parent / "target"


def newest(path):
    """Newest file modification time under path."""
    latest = 0.0
    for dirpath, _, files in os.walk(path):
        for name in files:
            try:
                latest = max(latest, os.lstat(os.path.join(dirpath, name)).st_mtime)
            except OSError:
                pass
    return latest


def copies():
    """Yield (what it is a copy of, path) for every build copy."""
    for profile in TARGET.iterdir():
        for crate in (profile / "build").glob("*/"):
            for copy in crate.glob("*/"):
                units = sorted((copy / "fingerprint").glob("*.json"))
                unit = units[0].stem if units else "?"
                yield (profile.name, crate.name, unit), copy
        for copy in (profile / "incremental").glob("*/"):
            yield (profile.name, "incremental", copy.name.rsplit("-", 1)[0]), copy


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--keep", type=int, default=3)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    if args.keep < 1:
        parser.error("--keep must be at least 1")
    if not TARGET.is_dir():
        return 0

    groups = defaultdict(list)
    for key, path in copies():
        groups[key].append((newest(path), path))

    old = [path for group in groups.values() for _, path in sorted(group, reverse=True)[args.keep:]]
    if not args.dry_run:
        for path in old:
            shutil.rmtree(path, ignore_errors=True)

    total = sum(len(group) for group in groups.values())
    verb = "would remove" if args.dry_run else "removed"
    print(f"{verb} {len(old)} of {total} build copies, keeping the newest {args.keep} of each")
    return 0


if __name__ == "__main__":
    sys.exit(main())
