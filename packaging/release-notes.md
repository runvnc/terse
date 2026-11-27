# Tersenet 0.9

Tersenet is a way to share information: formatted text, with diagrams where
they help, without a web browser's scripts, ads or tracking, and without one
website in control. Writers publish signed feeds of articles to relays;
readers fetch and check them. Optionally it also shares very small programs
(at most 1 MiB of memory each) that run on an isolated virtual machine,
Terse32.

This is the first public release. It works and is tested, but formats and
commands may still change before 1.0.

## What is in it

- **The reader**: articles with headings, lists, tables, links and figures;
  subscribing to feeds; the manual built in (press M). Code blocks that name
  their language are coloured, in the reader, the terminal reader
  (`tersereader`) and the website.
- **The shell**: the window you start in, with the reader, the manual and
  the built-in programs side by side in tiles.
- **The public relay** at `wss://relay.tersenet.org`, open to reading and
  publishing: add it with `terse relay add relay.tersenet.org`, and
  `terse feed publish` puts your articles there. `terse-relay` runs your
  own.
- **The machine** (Terse32, machine profile 1): 32-bit RISC-V, ROM up to
  256 KiB, RAM up to 768 KiB, a fixed budget of work per frame, and devices
  for a terminal, vector graphics, a page, sound and content.
- **The sheet**: a spreadsheet program with menus, paste, sort, number
  formats and the system clipboard.
- **Low Bell**: a text adventure engine with pictures, and the start of a
  game written for it. The game is in progress, not finished.
- **The performance rule**: every program works within a fixed budget per
  frame, and the host keeps a record of which of its own parts (a device
  driver, the renderer, its core) made a frame late, warning about or
  halting a part that keeps doing so. `terse perf report` sums that record
  up in a short report you may choose to publish. Nothing is sent
  automatically.

## Known limits

- The device bus (`bus/`, for people who build or swap a device) is tested on
  Linux only. On macOS and Windows it builds but has not yet been tried.
- Paging through the results of a find is specified (NETWORK.md) but not
  built yet.
- Nothing is signed by Apple or Microsoft; see below for opening the files.

## Install

On Linux or macOS, in a terminal:

    curl -fsSL https://tersenet.org/install.sh | sh

installs `terse-host`, `terse` and `terse-relay` of the latest release into
`~/.local/bin`, after checking them against `SHA256SUMS.txt`
(`TERSE_VERSION=@VERSION@` before `sh` for this one). Or download:

## Download

| System | File |
| --- | --- |
| Windows 10 or newer, 64-bit | `terse-@VERSION@-windows-x86_64.zip` |
| macOS 11 or newer (Apple silicon and Intel) | `terse-@VERSION@-macos.zip` |
| Linux, x86_64 (glibc 2.35 or newer: Ubuntu 22.04, Debian 12, Fedora 36 or newer) | `terse-@VERSION@-linux-x86_64.tar.gz` |
| Linux, ARM 64-bit (same) | `terse-@VERSION@-linux-aarch64.tar.gz` |
| The terminal reader, Linux and macOS | `tersereader-@VERSION@-<system>.tar.gz` |

Each archive holds `terse-host` (the window: reader, manual and built-in
programs), `terse` (the command-line tool) and `terse-relay`, with a
`README.txt`. Run `terse-host`, choose **reader** in the launcher, press
**Enter**, then **M**: that is the manual.

**Windows:** extract the zip, double-click `terse-host.exe`. The files are not
signed, so SmartScreen may say "Windows protected your PC": click **More
info**, then **Run anyway**.

**macOS:** the files are not signed or notarised by Apple, so macOS blocks them
the first time. After unzipping, run in Terminal

    xattr -dr com.apple.quarantine ~/Downloads/terse-@VERSION@-macos

then double-click `Terse.app`. (Or Control-click `Terse.app` and choose
Open; on macOS 15 or newer, try to open it once, then click **Open Anyway**
in System Settings > Privacy & Security.)

**Linux:** `tar xzf terse-@VERSION@-linux-x86_64.tar.gz`, then
`./terse-@VERSION@-linux-x86_64/terse-host`. It needs Wayland or X11; it draws
with Vulkan or OpenGL if it can and with the CPU otherwise. `terse` and
`terse-relay` are static and run on any Linux.

## Check the download

`SHA256SUMS.txt` lists the SHA-256 of every file:

    sha256sum -c --ignore-missing SHA256SUMS.txt              # Linux
    shasum -a 256 -c --ignore-missing SHA256SUMS.txt          # macOS
    Get-FileHash terse-@VERSION@-windows-x86_64.zip           # Windows (PowerShell): compare by eye

Built from this tag by the [release workflow](https://github.com/@REPO@/blob/v@VERSION@/.github/workflows/release.yml).
