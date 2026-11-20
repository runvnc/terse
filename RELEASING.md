# Releasing

The downloads are built by `.github/workflows/release.yml`, which runs
`scripts/release/build.sh` once per system:

| File | Built on | Contents |
| --- | --- | --- |
| `terse-<v>-linux-x86_64.tar.gz` | ubuntu-22.04 | `terse-host` (glibc 2.35+), `terse` and `terse-relay` (static, musl) |
| `terse-<v>-linux-aarch64.tar.gz` | ubuntu-22.04-arm | the same for 64-bit ARM |
| `terse-<v>-macos.zip` | macos-15 | `Terse.app`, `terse-host`, `terse`, `terse-relay`: universal (Apple silicon and Intel), macOS 11+ |
| `terse-<v>-windows-x86_64.zip` | windows-latest | `terse-host.exe` (no console window), `terse.exe`, `terse-relay.exe`, C runtime linked in |
| `tersereader-<v>-{linux,macos}-{amd64,arm64}.tar.gz` | ubuntu-24.04 | the Go terminal reader (`native/reader-go`), static |
| `SHA256SUMS.txt` | | the SHA-256 of every file above |

Each archive also has `LICENSE` and a `README.txt` from `packaging/<system>/`.
`terse-host` is built with the GPU renderer (the software one is the
fallback, `--renderer auto`) and with sound (`--features audio`).

Left out on purpose: the device bus (`terse-bus`, `terse-dev-*`), which uses
Unix sockets and so does not build for Windows; it is for developers, who build
from source. `tersereader` has no Windows build: its terminal code is Unix-only.

## Cutting a release

1. Set the version in `Cargo.toml` (`[workspace.package] version`; every crate
   takes it from there), run `scripts/cargo build` so `Cargo.lock` follows,
   and commit. The workflow refuses a tag that differs from this version.
2. Tag and push:

       git tag v0.1.0
       git push origin v0.1.0

3. Wait for the workflow (Actions tab, "release"; about 20 to 30 minutes). It
   ends by making a **draft** release named "Terse 0.1.0" with every file
   attached and the notes from `packaging/release-notes.md`.
4. Check the draft: download the file for your system, run it, open the
   manual. Edit the notes if you like (say what changed).
5. Publish it. The README's download links point at
   `https://github.com/runvnc/terse/releases/latest`, which is the newest
   published, non-pre-release release.

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
