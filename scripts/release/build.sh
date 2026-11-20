#!/usr/bin/env bash
# Builds and packages one release download (RELEASING.md). The release
# workflow (.github/workflows/release.yml) runs exactly this; so can you:
#
#     scripts/release/build.sh linux-x86_64     # or linux-aarch64, macos, windows-x86_64
#     scripts/release/build.sh reader            # tersereader for Linux and macOS (Go)
#
# The archives go to dist/ (OUT changes that). VERSION names them (default:
# the tag at HEAD without its v, else dev-<short hash>). TERSE_CARGO is the
# cargo command (default cargo); on the owner's machine, use the throttled
# wrapper, as CLAUDE.md asks:
#
#     TERSE_CARGO="flock /run/user/$(id -u)/terse-build.lock scripts/cargo" scripts/release/build.sh linux-x86_64
set -euo pipefail

platform=${1:?usage: scripts/release/build.sh linux-x86_64|linux-aarch64|macos|windows-x86_64|reader}
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
read -r -a cargo <<<"${TERSE_CARGO:-cargo}"
out=${OUT:-$root/dist}
if [ -z "${VERSION:-}" ]; then
    tag=$(git describe --tags --exact-match --match 'v*' 2>/dev/null || true)
    VERSION=${tag#v}
    [ -n "$VERSION" ] || VERSION="dev-$(git rev-parse --short HEAD)"
fi
mkdir -p "$out"
stage_root=$(mktemp -d)
trap 'rm -rf "$stage_root"' EXIT

# Release binaries without symbols: a third of the size.
export CARGO_PROFILE_RELEASE_STRIP=${CARGO_PROFILE_RELEASE_STRIP:-symbols}

# The windowed host: GPU renderer with the software one as fallback
# (--renderer auto, the default) and sound.
host_features=(--features terse-host/audio)

build() { # <target> <cargo args...>
    local target=$1
    shift
    "${cargo[@]}" build --release --locked --target "$target" "$@"
}

stage() { # <name>: a fresh directory named for the download
    local dir="$stage_root/$1"
    mkdir -p "$dir"
    printf '%s\n' "$dir"
}

case "$platform" in
linux-x86_64 | linux-aarch64)
    arch=${platform#linux-}
    gnu="$arch-unknown-linux-gnu"
    musl="$arch-unknown-linux-musl"
    # The host links the system's graphics, sound and input libraries:
    # glibc, on the oldest runner image the workflow can use.
    build "$gnu" -p terse-host "${host_features[@]}"
    # The command-line tools need nothing from the system: static (musl),
    # so they run on any Linux, servers included.
    musl_cc="CC_${musl//-/_}"
    export "$musl_cc=${!musl_cc:-musl-gcc}"
    build "$musl" -p terse-cli -p terse-relay
    name="terse-$VERSION-$platform"
    dir=$(stage "$name")
    cp "target/$gnu/release/terse-host" "target/$musl/release/terse" "target/$musl/release/terse-relay" "$dir/"
    cp LICENSE "$dir/LICENSE"
    sed "s/@VERSION@/$VERSION/g" packaging/linux/README.txt >"$dir/README.txt"
    tar -C "$stage_root" -czf "$out/$name.tar.gz" --owner=0 --group=0 "$name"
    ;;
macos)
    # Both Apple silicon and Intel, joined into universal binaries.
    export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
        build "$t" -p terse-host -p terse-cli -p terse-relay "${host_features[@]}"
    done
    name="terse-$VERSION-macos"
    dir=$(stage "$name")
    for bin in terse-host terse terse-relay; do
        lipo -create -output "$dir/$bin" "target/aarch64-apple-darwin/release/$bin" "target/x86_64-apple-darwin/release/$bin"
    done
    # A minimal app bundle, so the host opens with a double click.
    app="$dir/Terse.app"
    mkdir -p "$app/Contents/MacOS"
    cp "$dir/terse-host" "$app/Contents/MacOS/terse-host"
    sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
    # Ad-hoc signatures (no Apple account needed): Apple silicon runs no
    # unsigned code at all. They do not satisfy Gatekeeper; README.txt says
    # how to open the download anyway.
    # SIGNING: with a Developer ID, sign here instead with
    #   codesign --force --options runtime --timestamp --sign "Developer ID Application: …" <each binary> "$app"
    # and notarise the zip below (xcrun notarytool submit … --wait; xcrun stapler staple "$app").
    for f in "$dir/terse-host" "$dir/terse" "$dir/terse-relay"; do
        codesign --force --sign - "$f"
    done
    codesign --force --sign - "$app"
    cp LICENSE "$dir/LICENSE"
    sed "s/@VERSION@/$VERSION/g" packaging/macos/README.txt >"$dir/README.txt"
    rm -f "$out/$name.zip"
    (cd "$stage_root" && ditto -c -k --keepParent "$name" "$out/$name.zip")
    ;;
windows-x86_64)
    t=x86_64-pc-windows-msvc
    # The C runtime linked in, so no Visual C++ redistributable is needed.
    RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" build "$t" -p terse-host -p terse-cli -p terse-relay "${host_features[@]}"
    name="terse-$VERSION-windows-x86_64"
    dir=$(stage "$name")
    # SIGNING: with a code-signing certificate, sign the three .exe files
    # here (signtool sign /fd SHA256 /tr <timestamp URL> /td SHA256 …).
    cp "target/$t/release/terse-host.exe" "target/$t/release/terse.exe" "target/$t/release/terse-relay.exe" "$dir/"
    cp LICENSE "$dir/LICENSE.txt"
    sed "s/@VERSION@/$VERSION/g" packaging/windows/README.txt | sed 's/$/\r/' >"$dir/README.txt"
    rm -f "$out/$name.zip"
    (cd "$stage_root" && 7z a -tzip -bd "$out/$name.zip" "$name" >/dev/null)
    ;;
reader)
    # The terminal reader (native/reader-go): pure Go, cross-compiled from
    # one machine. No Windows build: its terminal code is Unix-only.
    for os_arch in linux/amd64 linux/arm64 darwin/amd64 darwin/arm64; do
        goos=${os_arch%/*}
        goarch=${os_arch#*/}
        label="$goos-$goarch"
        [ "$goos" = darwin ] && label="macos-$goarch"
        name="tersereader-$VERSION-$label"
        dir=$(stage "$name")
        (cd native/reader-go && CGO_ENABLED=0 GOOS=$goos GOARCH=$goarch GOMAXPROCS=${GOMAXPROCS:-4} \
            nice -n 15 go build -p 4 -trimpath -ldflags='-s -w' -o "$dir/tersereader" .)
        cp LICENSE "$dir/LICENSE"
        cp native/reader-go/README.md "$dir/README.md"
        tar -C "$stage_root" -czf "$out/$name.tar.gz" --owner=0 --group=0 "$name"
    done
    ;;
*)
    echo "unknown platform $platform" >&2
    exit 2
    ;;
esac
ls -l "$out"
