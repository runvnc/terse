Tersenet @VERSION@ for Windows (64-bit, Windows 10 or newer)
============================================================

Tersenet is a way to share formatted text, with diagrams where they help,
without web browsers, ads or tracking. https://github.com/runvnc/terse

In this folder
--------------
  terse-host.exe    the Tersenet window: the reader, the manual, the built-in programs
  terse.exe         the command-line tool (publish articles, manage feeds and relays)
  terse-relay.exe   a relay, to share with other people (see the manual)
  bus\              for developers: terse-bus.exe, the host that runs each
                    device as a program of its own, and the device programs
                    it starts (terse-dev-*.exe), to build or swap a device
                    (DEVICES.md in the source; Windows 10 version 1803 or
                    newer). Not yet tried on Windows: please report what happens
  LICENSE.txt       Apache License 2.0
  FONTS-LICENSE.txt the licence of the DejaVu fonts built into terse-host.exe

Start
-----
Extract the whole zip first (right-click it, "Extract All..."), then
double-click terse-host.exe.

This download is not signed, so Windows SmartScreen may say "Windows
protected your PC". Click "More info", then "Run anyway". It asks only once.

The window opens on the launcher. Up and Down choose a program, Enter
starts it. Choose "reader" and press M: that is the Tersenet manual. F12
always brings you back to the launcher. In the reader, ? shows the keys.

From a terminal (PowerShell or Command Prompt) in this folder:
  .\terse-host.exe --demo     publishes four sample articles to your own
                              feed first, so the reader has something to show
  .\terse.exe                 lists the commands
terse-host prints what it is doing to the terminal it was started from.

Where Tersenet keeps things
---------------------------
  %APPDATA%\terse         your feed key (identity.key) and subscriptions
  %LOCALAPPDATA%\terse    programs' records, downloaded articles and
                          programs (blobs), and the store of the relay
                          terse-host starts when it finds none
Keep identity.key safe: it is your feed. To remove Tersenet, delete this
folder and the two above.

Without a relay of its own, terse-host starts one on 127.0.0.1:7878 (this
computer only). To read other people's feeds, add their relay:
  .\terse.exe relay add ws://their-host:7878
