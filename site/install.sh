#!/bin/sh
# Installs Tersenet on Linux or macOS, for you alone (never with sudo):
#
#     curl -fsSL https://tersenet.org/install.sh | sh
#
# It downloads the archive of the latest release for this system from
# GitHub, checks it against the release's SHA256SUMS.txt, and copies
# terse-host, terse and terse-relay into ~/.local/bin. It changes nothing
# else: no shell profile is edited (it prints the line to add instead).
#
#     curl -fsSL https://tersenet.org/install.sh | sh -s uninstall
#
# removes the three programs again (so does `sh install.sh uninstall`).
#
# TERSE_PREFIX       install into $TERSE_PREFIX/bin (default ~/.local)
# TERSE_VERSION      a release such as 0.9.0 (default: the latest)
# TERSE_RELEASE_URL  download from here instead of GitHub (for testing)
#
# The source of this script: docs/manual/site-files/install.sh in
# https://github.com/runvnc/terse
set -eu

repo=https://github.com/runvnc/terse
if [ -n "${TERSE_VERSION:-}" ]; then
    base=${TERSE_RELEASE_URL:-$repo/releases/download/v$TERSE_VERSION}
else
    base=${TERSE_RELEASE_URL:-$repo/releases/latest/download}
fi
bin=${TERSE_PREFIX:-$HOME/.local}/bin
programs="terse-host terse terse-relay"

say() { printf 'tersenet: %s\n' "$*"; }
die() {
    printf 'tersenet: %s\n' "$*" >&2
    exit 1
}

if [ "${1:-}" = uninstall ] || [ "${1:-}" = --uninstall ]; then
    for p in $programs; do
        if [ -e "$bin/$p" ]; then
            rm -f "$bin/$p"
            say "removed $bin/$p"
        fi
    done
    say "your key, subscriptions and downloads are kept; to remove them too,"
    if [ "$(uname -s)" = Darwin ]; then
        say "delete ~/Library/Application Support/terse and ~/Library/Caches/terse"
    else
        say "delete ~/.config/terse, ~/.local/share/terse and ~/.cache/terse"
    fi
    exit 0
fi
[ $# -eq 0 ] || die "usage: install.sh [uninstall]"

# Which download: the system and the processor.
case $(uname -s) in
Linux) os=linux ;;
Darwin) os=macos ;;
*) die "this script is for Linux and macOS; for Windows, download the zip from $repo/releases" ;;
esac
case $(uname -m) in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*) die "no download for this processor ($(uname -m)); build from source: $repo" ;;
esac
if [ "$os" = macos ]; then
    suffix=-macos.zip # one download for Apple silicon and Intel
else
    suffix=-linux-$arch.tar.gz
fi

fetch() { # <url> <file>
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        die "neither curl nor wget is installed"
    fi
}

sha256() { # <file>: its SHA-256 in hexadecimal
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        die "neither sha256sum nor shasum is installed, so the download cannot be checked"
    fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The checksums file names every download of the release.
say "looking for a release at $base"
fetch "$base/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt" ||
    die "no release found there: Tersenet may not have published one yet (see $repo/releases), or the network is down"
line=$(grep -E "^[0-9a-f]{64} [ *]terse-[^ /]*$suffix\$" "$tmp/SHA256SUMS.txt" | head -n 1)
[ -n "$line" ] || die "the release has no download for $os $arch"
want=${line%% *}
file=${line##*[ *]}

say "downloading $file"
fetch "$base/$file" "$tmp/$file" || die "could not download $base/$file"
[ "$(sha256 "$tmp/$file")" = "$want" ] || die "$file does not match its SHA-256 in SHA256SUMS.txt: not installed"
say "checked its SHA-256"

case $file in
*.tar.gz)
    tar -xzf "$tmp/$file" -C "$tmp"
    dir=$tmp/${file%.tar.gz}
    ;;
*.zip)
    command -v unzip >/dev/null 2>&1 || die "unzip is not installed"
    unzip -q "$tmp/$file" -d "$tmp"
    dir=$tmp/${file%.zip}
    ;;
esac

mkdir -p "$bin"
for p in $programs; do
    [ -f "$dir/$p" ] || die "$file has no $p"
    # Copied beside, then renamed: replaces a program even while it runs.
    cp "$dir/$p" "$bin/.$p.new"
    chmod 755 "$bin/.$p.new"
    mv -f "$bin/.$p.new" "$bin/$p"
    say "installed $bin/$p"
done
say "$("$bin/terse" --version)"

case ":$PATH:" in
*":$bin:"*) ;;
*)
    say "$bin is not on your PATH. Add it with the line for your shell:"
    say "  bash: echo 'export PATH=\"$bin:\$PATH\"' >> ~/.bashrc"
    say "  zsh:  echo 'export PATH=\"$bin:\$PATH\"' >> ~/.zshrc"
    say "  fish: fish_add_path $bin"
    say "then open a new terminal."
    ;;
esac
say "start it with: terse-host"
say "to remove it: the same command, ending sh -s uninstall"
