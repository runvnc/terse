# Releasing

The downloads are built by `.github/workflows/release.yml`, which runs
`scripts/release/build.sh` once per system:

| File | Built on | Contents |
| --- | --- | --- |
| `terse-<v>-linux-x86_64.tar.gz` | ubuntu-22.04 | `terse-host` (glibc 2.35+), `terse` and `terse-relay` (static, musl); `bus/`: `terse-bus` and its drivers (glibc) |
| `terse-<v>-linux-aarch64.tar.gz` | ubuntu-22.04-arm | the same for 64-bit ARM |
| `terse-<v>-macos.zip` | macos-15 | `Terse.app`, `terse-host`, `terse`, `terse-relay`, `bus/`: universal (Apple silicon and Intel), macOS 11+ |
| `terse-<v>-windows-x86_64.zip` | windows-latest | `terse-host.exe` (no console window), `terse.exe`, `terse-relay.exe`, `bus\` (Windows 10 1803+), C runtime linked in |
| `tersereader-<v>-{linux,macos}-{amd64,arm64}.tar.gz` | ubuntu-24.04 | the Go terminal reader (`native/reader-go`), static |
| `SHA256SUMS.txt` | | the SHA-256 of every file above |

Each archive also has `LICENSE`, `NOTICE.txt`, `TLS-LICENSES.txt` (the
licences of the TLS libraries and root certificates built into the Rust
programs, from `third_party/tls/`) and a `README.txt` from `packaging/<system>/`;
the `terse-host` archives also have `FONTS-LICENSE.txt`, the licence of the
DejaVu fonts built into `terse-host` (`crates/terse-host/fonts/`).
`terse-host` is built with the GPU renderer (the software one is the
fallback, `--renderer auto`) and with sound (`--features audio`).

**The device bus** (`bus/`: `terse-bus` and the drivers `terse-dev-terminal`,
`-vector`, `-content`, `-sound` and `-page`, for people who build or swap a
device, `DEVICES.md`) is in every download, in a folder of its own, since
`terse-bus` finds its drivers beside itself. It is run and tested on Linux
only. **Untested on macOS and Windows**: there it is checked to compile
(`DEVICES.md` 4.3 says what differs: AF_UNIX sockets through Winsock and a
socket byte for waking on Windows, ulock waits on macOS), and the READMEs say
"not yet tried". Remove that wording here and in `packaging/macos/README.txt`
and `packaging/windows/README.txt` once `.github/workflows/test.yml` (by hand:
Actions, "test") has passed on the macOS and Windows runners and someone has
run a shell with it there. `terse-dev-crash` (a test driver) is left out.

`tersereader` has no Windows build: its terminal code is Unix-only.

## Testing on every system

`.github/workflows/test.yml` runs the workspace's tests on ubuntu-24.04,
macos-15 and windows-latest, the bus tests also with every end sleeping on the
socket and no process timing and with the socket transport only, then the C
core, the example driver and the Go reader where they build (Linux, macOS). It
runs only by hand (Actions, "test", "Run workflow"; or `gh workflow run
test.yml --ref <branch>`), never on a push. Each step ends with the list of
skipped tests (`scripts/test-skips.py`). On Linux, where every tool is
installed, the main test step runs with `TERSE_TEST_NO_SKIP=1`: a skip there
is a failure, except a missing graphics adapter (`TERSE_TEST_ALLOW_SKIP=gpu`;
the runner has no GPU, and lavapipe is installed so that wgpu may find a
software one). On macOS and Windows skips stay skips; the legitimate ones
there are listed in the run's skip list (no `/proc/self/schedstat`, no
graphics adapter, no CJK font, and on Windows no `AF_UNIX` in Python and no
C compiler, clang, Go or the RISC-V tools unless installed).

Locally, before a release: `scripts/cargo test --workspace` (it builds every
program the tests run, from the current sources; no build is needed first),
then `scripts/cargo build --release --workspace --bins` and the tests outside
cargo, which use `target/release` and refuse programs older than their
sources: `native/core-c` (`make && ./test.sh`), `native/driver-example/test.sh`,
`native/driver-experimental/test.sh` and `go test ./...` in
`native/reader-go`.

## Cutting a release

1. Set the version in `Cargo.toml` (`[workspace.package] version`; every crate
   takes it from there), run `scripts/cargo build` so `Cargo.lock` follows,
   and commit. Refresh the trusted roots built in for `wss://` relays
   (Mozilla's list, `webpki-roots`) in the same commit:
   `scripts/cargo update -p webpki-roots` (a newer 1.x; check its licence
   is still CDLA-Permissive-2.0, and copy its `LICENSE` over
   `third_party/tls/webpki-roots-LICENSE` if it changed). The workflow refuses a tag that differs from this version.
2. Tag and push:

       git tag v0.1.0
       git push origin v0.1.0

3. Wait for the workflow (Actions tab, "release"; about 20 to 30 minutes). It
   ends by making a **draft** release named "Tersenet 0.1.0" with every file
   attached and the notes from `packaging/release-notes.md`.
4. Check the draft: download the file for your system, run it, open the
   manual. Edit the notes if you like (say what changed).
5. Publish it. The README's download links point at
   `https://github.com/runvnc/terse/releases/latest`, which is the newest
   published, non-pre-release release; so does the install script (below).
6. Try the install script against it, into a scratch folder:

       curl -fsSL https://tersenet.org/install.sh | TERSE_PREFIX=/tmp/terse-try sh
       /tmp/terse-try/bin/terse --version

To try the workflow without releasing: Actions, "release", "Run workflow"
(or `gh workflow run release.yml --ref <branch>`). That run makes no release;
its downloads are attached to the run as the artifact `terse-downloads`,
named `dev-<commit>`.

If a tag run fails, fix it on `main`, then move the tag
(`git tag -f v0.1.0 && git push -f origin v0.1.0`) or, simpler, release the
next patch version. Rerunning the workflow on the same tag replaces the
files of the draft.

To build one download locally, as the workflow does:

    TERSE_CARGO="flock /run/user/$(id -u)/terse-build.lock scripts/cargo" scripts/release/build.sh linux-x86_64

(it needs the musl target, `rustup target add x86_64-unknown-linux-musl`, and
`musl-gcc`, or `CC_x86_64_unknown_linux_musl=clang`). The files go to `dist/`.

## The install script

`curl -fsSL https://tersenet.org/install.sh | sh` (the manual's "Install
Tersenet" and the README) runs `docs/manual/site-files/install.sh`, which
`terse manual site --out site` copies into `site/`; the site is then deployed
as usual. After changing the script, regenerate `site/` and deploy it.

It relies on what this page describes, so keep them in step:

- `SHA256SUMS.txt` among the release's files, in `sha256sum` format
  (`<hash>  <file>`), naming `terse-<v>-linux-x86_64.tar.gz`,
  `terse-<v>-linux-aarch64.tar.gz` and `terse-<v>-macos.zip`;
- each archive holding one folder named like it (without `.tar.gz` or `.zip`)
  with `terse-host`, `terse` and `terse-relay` at its top;
- the files at `https://github.com/runvnc/terse/releases/latest/download/<file>`
  (the newest published release), or `releases/download/v<version>/<file>`
  with `TERSE_VERSION`.

It installs the three programs into `~/.local/bin` (`$TERSE_PREFIX/bin`),
never with sudo, and edits no shell profile. `bus/` and `Terse.app` are not
installed (the download has them). To test it without a release, serve a
folder holding `dist/`'s files and their `SHA256SUMS.txt` over HTTP and point
the script at it:

    (cd dist && sha256sum -- terse-*.tar.gz > SHA256SUMS.txt && python3 -m http.server 8765 --bind 127.0.0.1) &
    TERSE_RELEASE_URL=http://127.0.0.1:8765 TERSE_PREFIX=/tmp/terse-try sh docs/manual/site-files/install.sh

There is no script for Windows: the zip already runs where it is unpacked,
and a PowerShell script would add only a Start menu entry and a `PATH`
change, untested on Windows so far. The guide says: download, Extract All,
double-click `terse-host.exe`.

## What is not signed

Nothing is signed by a certificate authority:

- **macOS**: the binaries carry only ad-hoc signatures (no identity), which
  Apple silicon needs to run code at all. Gatekeeper blocks a downloaded,
  unnotarised app; `README.txt` and the release notes tell users how to open
  it (`xattr -dr com.apple.quarantine <folder>`, or Open Anyway). Signing and
  notarising need a paid Apple Developer Program membership (99 USD a year), a
  "Developer ID Application" certificate and an app-specific password or API
  key as repository secrets; then, at the place marked `SIGNING` in
  `scripts/release/build.sh`: import the certificate into a temporary
  keychain, `codesign --force --options runtime --timestamp --sign "Developer
  ID Application: …"` each binary and `Terse.app`, `xcrun notarytool submit
  <zip> --wait`, `xcrun stapler staple Terse.app`, and zip again.
- **Windows**: unsigned `.exe` files make SmartScreen warn ("More info", "Run
  anyway") until the files gain reputation. Signing needs a code-signing
  certificate (an OV or EV certificate from a CA, or Azure Trusted Signing);
  sign the three `.exe` files with `signtool` at the place marked `SIGNING`.
- **Linux**: nothing is expected; the checksums file is the check.

The checksums prove a download is the file the workflow built, not who built
it. Anyone who trusts GitHub's account security can compare them with the
draft's.

## The public relay

A public relay runs the Linux `terse-relay` from a release (static, so it
runs on any Linux server) with the limits of README "Running a public
relay". `packaging/linux/terse-relay.service` is its systemd unit; it is in
the source, not in the archives. To update the relay: copy the new
`terse-relay` over `/usr/local/bin/terse-relay` and `systemctl restart
terse-relay` (SIGTERM closes connections cleanly; clients reconnect within
seconds, and the index of what the store holds is rebuilt at start). A
release that changes how the relay limits or refuses says so in its notes,
since it changes what publishers on the public relay see.
