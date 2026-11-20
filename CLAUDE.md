# Notes for agents

## Builds and disk space

**Use `scripts/cargo` instead of `cargo` for anything that compiles**
(`scripts/cargo build`, `scripts/cargo test --workspace`, and so on). It takes
the same arguments and does three things plain `cargo` does not: it caps how
fast the build may write to disk, runs at low priority with 4 jobs, and prunes
old builds afterwards. An uncapped test run filled the disk to 93% and froze
the desktop, so do not bypass it. Run one heavy build at a time.

- **Only the last 3 builds are kept.** After each run the wrapper calls
  `scripts/sweep-target.py`, which deletes all but the newest 3 copies of each
  library, binary and test in `target/`.
- **A build worth keeping is stored deliberately.** A release, or any build
  someone will want later, is tagged and copied out by hand: `git tag <name>`,
  then copy the binaries from `target/release/` to `~/terse-builds/<name>/`.
  Never rely on finding an old build in `target/`.
- **Debug info is off** (`[profile.dev] debug = false` in `Cargo.toml`). Leave
  it off. If one build needs a debugger or full symbols, enable it for that
  build only:
  `CARGO_PROFILE_DEV_DEBUG=true scripts/cargo build ...`
  That produces a second, much larger set of artifacts.
