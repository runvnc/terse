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

## Running the tests

`scripts/cargo test --workspace` alone runs the whole suite correctly: no
build is needed before it.

- **Programs are never stale.** A test that runs another package's program
  (`terse-bus`, `terse`, the drivers) gets it from
  `terse_testkit::program(name)`, which runs `cargo build --workspace --bin
  <name>` first (once per test binary; nothing happens when it is up to
  date, and `terse-testkit: built ...` is printed when it was not). Never
  find such a program by path (`target/debug/...`, beside
  `CARGO_BIN_EXE_*`): cargo builds only the programs of the package being
  tested. A program of the test's own package comes from
  `env!("CARGO_BIN_EXE_<name>")`.
- **Skips are seen.** A test that cannot run here calls
  `terse_testkit::skip!("<need>", "why")` and returns; never a bare
  `eprintln!("skipped ...")`, which cargo hides behind `ok`. The run ends
  with the list of skips. `TERSE_TEST_NO_SKIP=1` turns every skip into a
  failure (`TERSE_TEST_ALLOW_SKIP=gpu,...` exempts needs).
- The tests outside cargo (`native/*/test.sh`, `go test` in
  `native/reader-go`) use `target/release`: run
  `scripts/cargo build --release --workspace --bins` before them. They
  refuse a program older than its sources (`scripts/check-built.py`).
