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
