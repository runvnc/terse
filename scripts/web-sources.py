#!/usr/bin/env python3
"""Prints the hash of the sources docs/manual/site-files/terse-web.wasm is
built from (scripts/build-web writes it to crates/terse-web/wasm.sources;
crates/terse-web/tests/wasm.rs computes the same to tell a stale build):
SHA-256 over, in path order, each file's path relative to the repository,
a zero byte, its bytes and a zero byte, for the Cargo.toml and every .rs
file under src/ of crates/terse-core and crates/terse-web."""

import hashlib
import os

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
files = []
for crate in ("crates/terse-core", "crates/terse-web"):
    files.append(f"{crate}/Cargo.toml")
    for d, _, names in os.walk(os.path.join(root, crate, "src")):
        for n in names:
            if n.endswith(".rs"):
                files.append(os.path.relpath(os.path.join(d, n), root).replace(os.sep, "/"))
h = hashlib.sha256()
for f in sorted(files):
    h.update(f.encode() + b"\0")
    with open(os.path.join(root, f), "rb") as fh:
        h.update(fh.read() + b"\0")
print("sha256", h.hexdigest())
