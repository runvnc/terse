Tersenet @VERSION@ for macOS (Apple silicon and Intel, macOS 11 or newer)
=======================================================================

Tersenet is a way to share formatted text, with diagrams where they help,
without web browsers, ads or tracking. https://github.com/runvnc/terse

In this folder
--------------
  Terse.app     the Tersenet window: the reader, the manual, the built-in programs
  terse-host    the same program, for starting from Terminal with options
  terse         the command-line tool (publish articles, manage feeds and relays)
  terse-relay   a relay, to share with other people (see the manual)
  bus/          for developers: terse-bus, the host that runs each device as
                a program of its own, and the device programs it starts
                (terse-dev-*), to build or swap a device (DEVICES.md in the
                source). Not yet tried on macOS: please report what happens
  LICENSE       Apache License 2.0
  FONTS-LICENSE.txt  the licence of the DejaVu fonts built into terse-host

First: let macOS open it
------------------------
This download is not signed by an Apple developer account, so macOS blocks
it the first time ("cannot be opened because Apple cannot check it for
malicious software" or "is damaged"). Either:

  In Terminal (the simplest; it covers all four programs):
    xattr -dr com.apple.quarantine ~/Downloads/terse-@VERSION@-macos
  (use the folder's real location if you moved it), or

  In Finder: Control-click (right-click) Terse.app, choose Open, then Open
  again. On macOS 15 or newer: double-click Terse.app, close the warning,
  open System Settings > Privacy & Security, scroll down and click
  "Open Anyway" next to the message about Terse.

Start
-----
Double-click Terse.app (you may move it to Applications), or in Terminal:
  ./terse-host

The window opens on the launcher. Up and Down choose a program, Enter
starts it. Choose "reader" and press M: that is the Tersenet manual. F12
(fn F12 on most Mac keyboards) always brings you back to the launcher. In
the reader, ? shows the keys.

Optionally, `./terse-host --demo` first publishes four sample articles to
your own feed, so the reader has something to show.

Where Tersenet keeps things
---------------------------
  ~/.config/terse         your feed key (identity.key) and subscriptions
  ~/.local/share/terse    programs' records, and the store of the relay
                          terse-host starts when it finds none
  ~/.cache/terse/blobs    downloaded articles and programs
Keep identity.key safe: it is your feed. To remove Tersenet, delete this
folder and the three above.

Without a relay of its own, terse-host starts one on 127.0.0.1:7878 (this
computer only). To read other people's feeds, add their relay:
  ./terse relay add ws://their-host:7878
