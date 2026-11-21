Tersenet @VERSION@ for Linux
===========================

Tersenet is a way to share formatted text, with diagrams where they help,
without web browsers, ads or tracking. https://github.com/runvnc/terse

In this folder
--------------
  terse-host    the Tersenet window: the reader, the manual, the built-in programs
  terse         the command-line tool (publish articles, manage feeds and relays)
  terse-relay   a relay, to share with other people (see the manual)
  bus/          for developers: terse-bus, the host that runs each device as
                a program of its own, and the device programs it starts
                (terse-dev-*), to build or swap a device (DEVICES.md in the
                source); bus/terse-bus with no arguments lists its options
  LICENSE       Apache License 2.0
  FONTS-LICENSE.txt  the licence of the DejaVu fonts built into terse-host

Start
-----
  ./terse-host

The window opens on the launcher. Up and Down choose a program, Enter
starts it. Choose "reader" and press M: that is the Tersenet manual. F12
always brings you back to the launcher. In the reader, ? shows the keys.

Optionally, `./terse-host --demo` first publishes four sample articles to
your own feed, so the reader has something to show.

To run Tersenet from anywhere, copy the three programs to a folder on your
PATH, for example ~/.local/bin.

What it needs
-------------
- A desktop session: Wayland or X11.
- glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or newer).
- For drawing with the GPU: Vulkan or OpenGL drivers (Mesa has them). Without
  them terse-host draws with the CPU by itself; `--renderer software` forces that.
- libasound.so.2 (ALSA, for sound) and libudev.so.1 (for game controllers):
  terse-host does not start without them. Desktop distributions have both;
  PipeWire and PulseAudio play the sound.
- No fonts: terse-host has its own (DejaVu, built in; FONTS-LICENSE.txt).
  For Japanese, Chinese, Korean and other scripts it lacks, add a font:
  terse fonts add <font file> (the manual article "Fonts").
terse and terse-relay are static programs and need nothing at all.

Where Tersenet keeps things
---------------------------
  ~/.config/terse         your feed key (identity.key) and subscriptions
  ~/.local/share/terse    programs' records, and the store of the relay
                          terse-host starts when it finds none
  ~/.cache/terse/blobs    downloaded articles and programs
($XDG_CONFIG_HOME, $XDG_DATA_HOME and $XDG_CACHE_HOME are used if set.)
Keep identity.key safe: it is your feed. To remove Tersenet, delete this
folder and the three above.

Without a relay of its own, terse-host starts one on 127.0.0.1:7878 (this
computer only). To read other people's feeds, add their relay:
  ./terse relay add ws://their-host:7878
