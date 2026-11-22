#!/usr/bin/env python3
"""List the tests that skipped themselves, from the skip log.

    scripts/test-skips.py            # print the skips recorded since --clear
    scripts/test-skips.py --clear    # empty the log (before a test run)

A test that cannot run here (no GPU, no Go, no C compiler, ...) records a
line in the skip log (terse-testkit's skip!, and the Go reader's tests)
instead of passing silently. The log is $TERSE_TEST_SKIPS, else
test-skips.log in the target directory ($CARGO_TARGET_DIR, else target/).
scripts/cargo clears it before `test` and prints it after; the workflow
does the same. TERSE_TEST_NO_SKIP=1 makes every skip a failure instead
(except the needs listed in TERSE_TEST_ALLOW_SKIP, comma-separated).
"""

import os
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def log_path():
    if os.environ.get("TERSE_TEST_SKIPS"):
        return Path(os.environ["TERSE_TEST_SKIPS"])
    target = Path(os.environ.get("CARGO_TARGET_DIR") or "target")
    return (target if target.is_absolute() else ROOT / target) / "test-skips.log"


def main():
    log = log_path()
    if sys.argv[1:] == ["--clear"]:
        log.unlink(missing_ok=True)
        return 0
    if sys.argv[1:]:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    try:
        lines = log.read_text(errors="replace").splitlines()
    except FileNotFoundError:
        lines = []
    skips = sorted({tuple((l.split("\t") + ["", "", "", ""])[:4]) for l in lines if l.strip()})
    if not skips:
        print("skips: none")
        return 0
    needs = Counter(need for _, _, need, _ in skips)
    print(f"skips: {len(skips)} ({', '.join(f'{n} {k}' for k, n in sorted(needs.items()))}); "
          f"TERSE_TEST_NO_SKIP=1 makes them failures")
    for package, test, need, why in skips:
        print(f"  SKIPPED [{need}] {package} {test}: {why}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
