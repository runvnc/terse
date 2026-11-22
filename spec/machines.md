# §machines Device 8 — Machines (version 0)

Part of the Terse machine specification, v0 (draft): `SPEC.md` is the core
machine and says how these documents fit together; this one is complete
for its device, given the core sections it builds on (below).

A host may run several machines as one **system**. One of them, the
**shell**, starts the others, decides where their screens are shown and
which of them receives the keyboard, through device 8. The host provides
the mechanism (running the machines, drawing their screens, routing input);
how screens are laid out and what the user is offered is the shell's
business.

**Builds on** all of `SPEC.md`, and on the other device documents where a
system touches them: images are fetched as content's get does
(`spec/content.md`, §content.submit), programs are named by feed entries
(§content.feeds), the shell lays out its machines' screens as panels with
the 3D display (`spec/vector3d.md`, §vector3d.list), and rectangles are
clamped as the page's are (`spec/page.md`, §page.calls).

## §machines.shell The shell and its machines

A host starts a system with one image, the shell. The shell has device 8 if
its image lists (8, 0). Every other machine has device 8 absent (its calls
return `FFFF_FFFFh`, §calls.convention), also if its image lists it; so does an
image a host runs on its own, outside a system. How the user names the shell
is up to the host.

The shell starts machines under **handles** 1 to 8. Each is a complete,
separate machine: its own ROM, RAM, devices, event queue, content request
handles (§content), budget and frame counter. Machines share nothing but
what the host keeps behind the content device (its store, feeds, identity).
Each machine's screen is the whole 640 × 480 units (§screen).

A handle is **in use** from the `start` that returns it until a `stop` of it,
and is then in one of these states:

| Value | State | Meaning |
|---|---|---|
| 0 | none | not in use |
| 1 | loading | started; the host does not have the image yet |
| 2 | running | runs a frame in every system frame |
| 3 | paused | runs no frames and receives no input |
| 4 | exited | stopped by `exit` (§stopping) |
| 5 | faulted | stopped by a fault (§stopping) |
| 6 | failed | the image could not be had, is not an image the host can run, the host has no room for another machine, or a snapshot could not be restored; `info` gives the reason |
| 7 | halted | stopped by the host: a device it uses did not keep up (SPEC §stopping, `PERFORMANCE.md` R15); its snapshot from just before is in its automatic slot (§machines.snapshots) |

Each handle also has a **run flag**, a **rectangle** (x, y, width,
height in the shell's screen units) and possibly an **identity**. The
system has a **focus**: a handle, or 0 for the shell; it is 0 at reset;
and a **view** of the shell's 3D scene (`face`, below): none at reset.

A program's **identity** is its publisher's feed key and its name: the
(feed key, title) of a program entry (`NETWORK.md` section 4.2). The
host's built-in programs have the identity (32 zero bytes, image title). A
machine started with `start_program` has that identity for its whole life,
whatever version of the program it runs; one started with `start` (by blob
ID), and an image a host runs from a file, have none; a restored machine
has the identity of the machine its snapshot was taken of. A shell the host
starts as its own built-in shell has the identity of a built-in. §content.records says what the identity is for.

## §machines.calls Calls

| `a7` | Call | Arguments | Result | Class |
|---|---|---|---|---|
| `800h` | `start` | `a0` = address of an image ID (32 bytes, read) | a handle 1–8, or 0 if none is free | core |
| `801h` | `stop` | `a0` = handle | 1, or 0 if the handle is not in use | core |
| `802h` | `place` | `a0` = handle, `a1` = x, `a2` = y, `a3` = width, `a4` = height | 1, or 0 if the handle is not in use | core |
| `803h` | `run` | `a0` = handle, `a1` = 1 run, 0 pause | 1, or 0 if refused | core |
| `804h` | `focus` | `a0` = handle, or 0 | 1, or 0 if refused | core |
| `805h` | `info` | `a0` = handle, `a1` = buffer (48 bytes, written) | 1, or 0 if the handle is not 1–8 | core |
| `806h` | `builtin` | `a0` = index, `a1` = buffer (32 bytes, written) | 1, or 0 past the last | core |
| `807h` | `start_program` | `a0` = address of a program record (read) | a handle 1–8, or 0 if none is free | core |
| `808h` | `identity` | `a0` = handle, `a1` = buffer (81 bytes, written) | 1, or 0 if it has none | core |
| `809h` | `request` | `a0` = handle, `a1` = buffer (82 bytes, written) | 1, or 0 if no request waits | core |
| `80Ah` | `argument` | `a0` = handle, `a1` = address of a blob ID (32 bytes, read) | 1, or 0 if the handle is not loading | core |
| `80Bh` | `snapshot` | `a0` = handle, `a1` = slot | 1 accepted, 0 refused | core |
| `80Ch` | `restore` | `a0` = slot | a handle 1–8, or 0 if none is free or the slot is out of range | core |
| `80Dh` | `snapshot_info` | `a0` = slot, `a1` = buffer (122 bytes, written) | 1 if the slot holds a snapshot, else 0 | core |
| `80Eh` | `snapshot_delete` | `a0` = slot | 1, or 0 if the slot is out of range | core |
| `80Fh` | `face` | `a0` = handle, `a1` = frames, `a2` = x, `a3` = y, `a4` = width, `a5` = height | 1, or 0 if refused | core |
| `810h` | `unface` | `a0` = frames | 1, or 0 if refused | core |
| `811h` | `experimental` | `a0` = handle, `a1` = index, `a2` = buffer (32 bytes, written) | 2 granted, 1 not granted, or 0 if there is no such entry | core |

- **`start`** reads the 32-byte image ID at `a0`, which must lie wholly in
  ROM or wholly in RAM (else `FFFF_FFFEh`). If every handle is in use it
  returns 0. Otherwise the lowest handle not in use becomes loading, with the
  run flag set, the rectangle (0, 0, 0, 0) and that image ID, and the call
  returns it. The host then looks for the blob with that ID, in its store
  first and then as §content does for a get, and checks it as an image
  (§image); a later frame takes the result into account (§machines.frames):
  the handle becomes running (paused if the run flag is clear) with a
  machine reset with that image (§reset), or failed. The host may take
  any number of frames to get the image.
- **`start_program`** reads a program record at `a0`: the publisher's feed
  key (32 bytes; all zero for a built-in program), the name's length n (1
  byte) and the name (n bytes, a valid display name, §display-names, so n is
  1 to 48). The 33 +
  n bytes must lie wholly in ROM or wholly in RAM and the name must be
  valid, else the call returns `FFFF_FFFEh` and does nothing else. If every
  handle is in use it returns 0. Otherwise the lowest handle not in use
  becomes loading, with the run flag set, the rectangle (0, 0, 0, 0), image
  ID zero and that identity, and the call returns it. The host then finds
  the current version of the program that it holds (`NETWORK.md` section
  4.2; for the zero key, the first built-in program whose image title is
  the name), gets its image as for `start` and checks it; a later frame
  takes the result into account: the handle becomes running or paused,
  with the record's image ID that of the version started, or failed, with
  reason 1 if the host holds no such program or cannot get its image. The
  host may take any number of frames.
- **`identity`** writes the identity of handle `a0` to the buffer, 81
  bytes wholly in RAM (else `FFFF_FFFEh`): the feed key (32), the name's
  length (1) and the name, padded with `00h` to 48 bytes; it returns 1. For
  a handle not in use or with no identity it writes nothing and returns 0.
- **`stop`** with a handle in use: the machine ceases to exist and the
  handle is not in use (state none, run flag clear, rectangle and record
  zero); if it had the focus, the focus becomes 0. Its pending content
  requests are never completed, and the host withdraws its consent prompts
  for them. Returns 1, or 0 for any other `a0`.
- **`place`** sets the rectangle of a handle in use, clamped to the screen as
  the page's `region` is (§page.calls). Returns 1, or 0 (and nothing
  changes) for a handle not in use.
- **`run`** with a handle that is loading, running or paused: `a1` = 1 sets
  the run flag, 0 clears it; a running handle becomes paused, a paused one
  running, at once; a loading one keeps its state. Returns 1; returns 0 and
  changes nothing for any other handle or any other `a1`.
- **`focus`** with `a0` = 0 gives the focus to the shell; with a handle that
  is loading, running or paused, to that handle. Returns 1; returns 0 and
  changes nothing for any other `a0`.
- **`info`** writes a 48-byte record of handle `a0` to the buffer, which must
  lie wholly in RAM (else `FFFF_FFFEh`), and returns 1; for `a0` outside 1–8
  it writes nothing and returns 0.

  | Offset | Size | Field |
  |---|---|---|
  | 0 | 1 | State |
  | 1 | 1 | Exit code if exited; the device's slot if halted; else 0 |
  | 2 | 1 | Fault kind if faulted, numbered as in `STATE.md` 1.3 (1 illegal instruction … 6 misaligned), else 0 |
  | 3 | 1 | Why the start failed, if failed: 1 not found, 2 not a valid image, 3 the image needs a device (slot, version) the host does not provide, 4 no room for another machine, 5 no snapshot in the slot, or one that cannot be read or does not fit its image (`restore`); else 0 |
  | 4 | 4 | Units the machine used in its last frame (§frames, delivered content included), 0 before its first |
  | 8 | 4 | The machine's frame counter |
  | 12 | 4 | Fault `pc` if faulted, else 0 |
  | 16 | 32 | The image ID given to `start`; for `start_program` and `restore`, zeros while loading, then the ID of the image the host got (also when it failed as not an image the host can run, or as not fitting the snapshot); zeros for a handle not in use |

  The record is as the host latched it at the start of the frame (§machines.frames), with the changes the shell's own calls made since.
- **`experimental`** tells the shell of the experimental devices
  (`spec/experimental.md`) of machine `a0`, so that it can tell the user
  that the program is, or may be elsewhere, outside the standard. It writes
  entry `a1` (0 for the first) of those its image lists, in the order of
  the image's header, to the buffer, 32 bytes wholly in RAM (else
  `FFFF_FFFEh`): the entry as the image's experimental device table holds
  it (the slot, the name's length, the name, `00h` up to 32 bytes,
  §experimental.image). It returns 2 if the machine was granted that
  device (§experimental.grants), and 1 if it was not: it runs without it,
  as on any host. It writes nothing and returns 0 for `a1` past the last
  entry, for `a0` outside 1–8, for a handle with no machine (none,
  loading, or failed), and for an image whose header lists no
  experimental device or whose table is not valid (no host grants one of
  those). The entries are the machine's from when it was made (reset, or
  restored with its snapshot's grants) until its handle is stopped, also
  once it has exited, faulted or halted. A host that grants no
  experimental device (§experimental.grants) returns 1 for every entry.
  The entries are not part of device 8's state (`STATE.md` 1.10): they
  follow from the machine's image and grants.
- **`builtin`** writes the image ID of the host's built-in program number
  `a0` to the buffer, 32 bytes wholly in RAM (else `FFFF_FFFEh`), and
  returns 1; past the last program it writes nothing and returns 0. The
  built-in programs are images the host has in its store, in an order it
  chooses, the same for the whole run. A shell learns a program's title by
  getting the first 32 bytes of its image with the content device.
- **`request`** reads the open request (§system, `open`) machine `a0`
  made: if one waits and the shell has been told of it (an open requested
  event, §machines.input), it writes 82 bytes to the buffer, which must lie
  wholly in RAM (else `FFFF_FFFEh`): the kind (1), then for kind 1 the
  blob ID (32) and zeros, for kind 2 the program record as `identity`
  writes it (feed key, name length, name padded with `00h` to 48 bytes).
  The request no longer waits (the machine may make another) and the call
  returns 1. Otherwise, and for `a0` outside 1–8, it writes nothing and
  returns 0. A `stop` of the handle, and a `start` that reuses it, drop
  its request. What the shell does next is its own policy.
- **`argument`** reads a blob ID, 32 bytes wholly in ROM or wholly in RAM
  at `a1` (else `FFFF_FFFEh`). If handle `a0` is loading (so the machine
  has not run its first frame), the ID becomes the machine's argument
  (§system, `argument`), replacing one given before, and the call
  returns 1; otherwise it returns 0 and changes nothing. The argument is
  given to the machine when it becomes running or paused (§machines.frames).
  A handle being restored (`restore`) takes no argument: the call returns
  0 and changes nothing, and the machine keeps the argument in its
  snapshot. The device tells such a handle from its record: loading, with
  image ID zero and no identity; a `start` of the all-zero image ID, which
  can never be found, looks the same and is treated the same.
- **`snapshot`** with a handle that is running or paused and a slot 0–15:
  the host keeps the machine's snapshot (§machines.snapshots) in that slot,
  replacing what was there, taken at the end of this system frame (step
  6 of §machines.frames, after the machine's own frame if it runs one). Returns
  1; returns 0 and does nothing for any other handle or slot. The result
  arrives as a snapshot done event (§machines.input): status 1 stored, 3
  failed (the machine was stopped in the same frame, or the host could not
  store it); the values are content's done and failed (§content.status). A
  snapshot the shell deletes with `snapshot_delete` later in the same
  frame is stored and then deleted: its event says 1, and the slot is
  empty.
- **`restore`** with a slot 0–23 (16–23: the automatic slots, §machines.snapshots): if every handle is in use it returns 0.
  Otherwise the lowest handle not in use becomes loading, with the run
  flag set, the rectangle (0, 0, 0, 0), image ID zero and no identity,
  and the call returns it. The host reads the slot's snapshot and gets the
  image it names, in its store first and then as §content does for a get;
  a later frame takes the result into account (§machines.frames): the handle
  becomes running (paused if the run flag is clear) with the machine as
  it was when the snapshot was taken, its record's image ID that of the
  snapshot and its identity that of the snapshot, or failed: reason 5 if
  the slot holds no snapshot, one the host cannot read, or one that does
  not fit its image (`STATE.md` 4), reasons 1–3 as for `start` if the
  image cannot be had or run, reason 3 also if the snapshot's machine was
  granted an optional device (§image) this host cannot give it. A
  machine restored where the host has an optional device the snapshot's
  machine was not granted runs without it. The host may take any number
  of frames. A slot out of range returns 0.
- **`snapshot_info`** writes a 122-byte record of slot `a0` (0–23), as the host's
  snapshots are at the start of the frame with the changes the shell's
  own `snapshot_delete` calls made since, to the buffer, which must lie
  wholly in RAM (else `FFFF_FFFEh`), and returns 1 if the slot holds a
  snapshot; for an empty slot the record is all zero and it returns 0;
  for a slot out of range it writes nothing and returns 0.

  | Offset | Size | Field |
  |---|---|---|
  | 0 | 1 | Present: 1, or 0 for an empty slot |
  | 1 | 32 | The image ID |
  | 33 | 81 | The identity, as `identity` writes it; all zero for none |
  | 114 | 4 | The machine's frame counter when it was taken |
  | 118 | 4 | When it was taken: the Unix time latched for the machine's last frame, as its state holds it (`STATE.md` 1.5: 0 if it never ran; for a restored machine that has not run since, the time of the snapshot it was restored from) |

- **`snapshot_delete`** with a slot 0–23: the slot is empty from now on
  (for `snapshot_info` at once); the host removes its snapshot at the end
  of the frame, in order with the snapshots asked for in the same frame.
  Returns 1, or 0 for a slot out of range.
- **`face`** moves the host's view of the shell's 3D scene (§machines.screens)
  so that machine `a0`'s panel comes to lie flat on the glass, over `a1`
  frames (0–600), and shows it there as a flat picture in the rectangle
  `a2`–`a5` (x, y, width, height, clamped as `place` clamps). It is
  refused (returns 0, nothing changes) unless `a0` is 1–8, its machine
  has a picture (running, paused, exited, faulted or halted), its own rectangle
  shows no picture (§machines.screens), the shell's 3D list as it holds it now
  has a Screen of `a0` (§vector3d.list; decoded only, the caps not counted),
  `a1` is at most 600 and the rectangle shows a picture. Otherwise the view
  becomes: handle `a0`, towards the glass, that rectangle, start frame
  the shell's frame counter in this frame, `a1` frames, and as its start
  progress the view's progress now if it already moved `a0`'s panel,
  else 0. Returns 1.
- **`unface`** moves the view back to the scene as listed over `a0`
  frames (0–600), from its progress now: the direction becomes back, the
  start progress the progress now, the start frame this frame's, the
  duration `a0`. Refused (0) if `a0` is above 600 or there is no view.
  Returns 1.

  The view's **progress** is a number from 0 (the scene as listed) to
  4,096 (the panel flat on the glass). Shown after the shell's frame with
  counter f, with start progress p0, start frame s and duration d, it is
  p0 + (e − p0) × k ÷ d, rounded towards zero, for k = f − s < d (as
  32-bit unsigned numbers), and e from k = d on (also for d = 0), where e
  is 4,096 towards the glass and 0 back. The view **moves** while the
  progress is not e. At the end of each of the shell's frames (§machines.frames, step 3), if the view's machine is not in use, has no picture, has
  a rectangle that shows a picture, or has no Screen in the shell's 3D
  list, the view becomes none: the scene is shown as listed at once.

## §machines.screens Screens

A machine's **picture** is its screen scaled uniformly into its rectangle
(x, y, w, h) and centred. If w × 480 ≤ h × 640, the picture is
pw = w wide and ph = ⌊w × 480 / 640⌋ high, otherwise pw = ⌊h × 640 / 480⌋
and ph = h; it starts at px = x + ⌊(w − pw) / 2⌋, py = y + ⌊(h − ph) / 2⌋.
If pw or ph is 0 nothing is shown. A machine that is running, paused,
exited, faulted or halted has a picture; one that is loading or failed has none.
Exited, faulted and halted machines show the screen as it was at their last frame.
A machine's screen here is its complete picture, its own Surfaces (§vector3d.presentation) included.

A machine is shown **flat** in its picture: the picture of its own
rectangle, or, once the view (§machines.calls `face`) of it has finished
moving towards the glass, the picture of the view's rectangle. A machine
is shown as a **panel** where the shell's 3D list has a Screen of it in
effect (§vector3d.list), unless it is shown flat by the view.

If the shell's 3D list has no Screen in effect, the host shows, from the
bottom:

1. the shell's screen, over the whole of the host's screen as a single
   machine's would be, combined as §vector.layers says;
2. each flat picture, in increasing handle order, each a complete
   screen (its own layers, style, any afterglow and page, §vector.layers, §vector.phosphor,
   §page) scaled into its picture and covering what is below it; outside the
   picture, the rest of the rectangle shows what is below;
3. the host's own prompts (§content.feeds), over everything; a consent prompt
   names the image title of the machine whose request it is.

If it has at least one, the host shows the shell's **scene**, from the
bottom:

1. the shell's screen as above, without the lines, points and circles of
   its 3D list (its glass, §vector3d.presentation, if it has Surfaces in effect);
2. the panels, and the shell's own Surfaces (§vector3d.list), from the
   farthest to the nearest by the distance from the eye in use (§vector3d.space) to the panel's centre (the mean of its corners), the lower handle
   first where two are as far (a Surface before a Screen, two Surfaces in
   list order); each covers what is below it;
3. the lines, points and circles of the shell's 3D list as §vector3d
   draws them, each Screen and Surface in effect hiding what is behind it
   as a face of its four corners does;
4. each flat picture, in increasing handle order, as step 2 above;
5. the host's own prompts, as step 3 above.

A **panel** shows its machine's complete screen (as a flat picture does)
on its front, seen in perspective from the eye in use: the point of the
panel a fraction u along its top edge from its top left corner and v
down its left edge shows the point (640u, 480v) of the machine's screen.
Seen from behind (§vector3d.list) it is an opaque plate of a dark colour
the host chooses. Panels are cut at the near plane (z = −800) and at the
edges of the shell's screen. A host that cannot draw a picture in
perspective may draw each panel's picture flat instead: scaled uniformly
into the bounding rectangle of the panel's projection (its part in front
of the near plane), as a picture into a rectangle, and centred; the panel
still hides what it would hide.

**The view.** While there is a view whose machine has a Screen in
effect, everything in steps 2 and 3 of the scene (panels, lines, points,
circles and the faces that hide them) is moved by the view's transform
before it is seen from the eye; the caps of §vector3d.caps are counted on
the list as given. Let the view's machine's panel as listed have the
centre c₀, the width w₀ and the orientation R₀ (the rotation whose
columns are the unit vectors along its top edge, down its left edge, and
their cross product), and let the picture of the view's rectangle be
(px, py, pw, ph), with centre c₁ = (px + pw/2, py + ph/2, 0). At
progress p (§machines.calls) let t = p/4,096 and e = t² (3 − 2t). Then
c = c₀ + (c₁ − c₀) e, w = w₀ (pw/w₀)^e, R the rotation the spherical
interpolation of the quaternions of R₀ and the identity by e gives, by
the shorter arc, and the transform takes a point q to
c + (w/w₀) R R₀ᵀ (q − c₀). So at progress 0 the scene is as listed, and at
4,096 the view's panel lies on the glass over the picture, upright,
centred on it and as wide; the eye stays where it is, so what is not on
the glass still moves with it. When the view has finished moving towards
the glass its machine is shown flat (step 4) and not as a panel. A host
that cannot animate may show every view as at its end (progress 0 or
4,096) at once; the input is routed by the view as §machines.input says.

How the host scales a screen (fonts rasterised for the picture's size,
smoothing) is presentation (§terminal.presentation), and so are the resolution a
panel's picture is drawn at, how it is filtered, and the smoothing of a
panel's edges.

## §machines.ink The system ink

Each machine's lists are capped on their own (§vector.ink-cap,
§vector3d.caps). In a system the machines the shell shows also share one
budget, the **system ink**: 691,200 square units of the shell's screen,
what one machine shown over the whole screen may count (its picture,
640 × 480, and both its lists at their caps). So a system can ask of a
host about what two machines can: the shell's, and the machines' together.
The shell's own screen is not counted: it is drawn as a single machine's,
within its own caps, whatever the machines ask, so no machine can take
from the picture the user needs to regain control.

The host counts, for what it shows after each of the shell's frames
(§machines.screens), from the machines' lists and device 8's records
alone:

1. **The pictures**, in increasing handle order. A machine shown flat
   counts its picture's area, pw × ph. A machine shown as a panel counts
   the area of the box its four corners make on the glass, moved by the
   view where it is (§machines.calls `face`, at its progress), seen from
   the reference eye and clipped to the screen, rounded up; 640 × 480 if
   a corner is not in front of the near plane. A picture whose area would
   take the total above the system ink is a **plate**: nothing of its
   machine's screen is drawn. A flat plate shows a plain rectangle of a
   colour the host chooses; a panel that is a plate is not drawn (the
   shell's screen shows there, and it still hides the shell's lines as a
   face does). The count goes on with the next picture.
2. **The lists**, in the same order, of each machine whose picture is not
   a plate, each at its picture's **scale**, n 640ths: n = pw for a flat
   picture; for a panel, the larger of its box's width and 4/3 of its
   height on the glass before clipping, rounded up, at most 640 (640 if a
   corner is not in front of the near plane). With R the system ink not
   yet counted, its 2D list is drawn as §vector.ink-cap says with the cap
   the smaller of 230,400 and ⌊R × 640 / n⌋, and counts ⌈k × n / 640⌉,
   k being the ink of what it drew. Then its 3D list is drawn as
   §vector3d.caps says with its ink cap the smaller of 153,600 and
   ⌊R × 640 / n⌋ (R now less what the 2D list counted), and counts the
   lesser of ⌈k × n / 640⌉ and R.

A list counts its ink times the scale, not times the scale squared: a
shape drawn smaller covers fewer pixels, but the edge a host smooths
stays as wide, so the work of a thin line shrinks with its length only.
A machine with no picture (none, or a rectangle that shows none and no
Screen in effect) is not drawn and counts nothing.

What the system ink cuts is not machine state: no machine can observe
it, the machine runs exactly as if it were shown alone, and input is
routed by the pictures and panels as §machines.input says, plates
included. The counts of a panel and of a 3D list are evaluated after a
floating-point projection, so hosts may differ at the boundary by
rounding, as §vector3d.caps says. Measured on the reference host
(`PERFORMANCE.md` R12), the bundled shell's layouts with the bundled
programs count at most about 60% of the system ink.

## §machines.input Input

The host's input events (§events) are in the shell's screen units. In each
frame, before the shell runs, they are routed in order by the focus, the
flat pictures and the panels as they are at that moment (§machines.screens, as
shown after the shell's last frame, seen from the eye the host draws
from):

| Input | Goes to |
|---|---|
| Key down, key up, text, wheel | the machine with the focus if it is running; the shell if the focus is 0; otherwise nobody |
| Pointer, over a flat picture | the machine whose picture it is (the highest handle where pictures overlap), mapped into its screen units, if it is running; otherwise nobody |
| Pointer, over the front of a panel and no flat picture | while the view moves (§machines.calls), nobody; otherwise the machine whose panel it is, mapped into its screen units, if it is running; otherwise nobody |
| Pointer, over nothing else | the shell, unchanged (but for the shell's own Surfaces, below) |
| Controllers (§controllers) | the machine with the focus (the shell if the focus is 0); every other machine reads every controller as not connected, so nobody reads them while the machine with the focus does not run |
| Content done, feeds updated, page events | each machine its own (§content, §page.events) |
| Any other event in the host's input | the shell, unchanged |

The focus that routes the input of a frame, the controllers included, is
the one at this step: the home key changes it here (below), for the
events after it; a `focus` call takes effect at the next frame's routing
(§machines.frames).

A position (x, y) over a picture (px, py, pw, ph) is mapped to
x′ = ⌊(2(x − px) + 1) × 640 / (2pw)⌋ and y′ = ⌊(2(y − py) + 1) × 480 / (2ph)⌋.
The pointer at (x, y) is over a panel if the ray from the eye in use
(§vector3d.space) through the point (x + 1/2, y + 1/2, 0) meets the panel in
front of the near plane (z > −800), at a fraction u along its top edge
(0 ≤ u < 1) and v down its left edge (0 ≤ v < 1); of several, the one it
meets nearest the eye counts. If that panel is seen from behind (§vector3d.list), the pointer is over nothing; otherwise it is mapped to x′ = ⌊640u⌋,
y′ = ⌊480v⌋. A pointer event that presses a button (sets a button bit that
was clear in the previous pointer event, wherever that went) over a flat
picture, or over a panel while the view does not move, also gives the
shell a machine pressed event, in the place among the shell's events that
the pointer event had in the input, whatever the machine's state (also
when the pointer event itself goes to nobody).

**Surfaces.** A machine's own Surfaces (§vector3d.list) map the pointer once
more: a pointer event going to a machine, at (x′, y′) in its screen units
as above, is delivered at the point of the Surface it is over, if any, as
§vector3d.list says, seen from the same eye. The shell's own Surfaces, when
its scene has panels, are among the panels (§machines.screens): a pointer over
the front of one goes to the shell at that point (while the view moves, to
nobody), and gives no machine pressed event; when its scene has no
panels, the pointer events that go to the shell are mapped through its
Surfaces as any machine's are. A host running a single machine maps its
pointer events through its Surfaces in the same way.

**The home key.** A host reserves one key (or other input) as its home key;
which one is up to the host. It is never delivered to any machine as a key
event. When it is pressed, the focus becomes 0 and a home event is queued
for the shell. A host running a single machine, not a system, has no
shell to tell: the home key does nothing there (the reference host then
treats that key as an ordinary key).

**Events for the shell** (§events):

| Type | Name | `a` | `b` | `c` |
|---|---|---|---|---|
| `0Ah` | Machine changed | handle | the new state | 0 |
| `0Bh` | Home | 0 | 0 | 0 |
| `0Ch` | Machine pressed | handle | buttons | 0 |
| `0Fh` | Open requested | handle | 0 | 0 |
| `10h` | Snapshot done | slot | 1 stored, 3 failed | 0 |

Open requested says that machine `a` made an open request the shell can
read with `request`; there is one such event per request. Snapshot done
answers a `snapshot` call; there is one per accepted call, in the order of
the calls.

Machine changed is sent for every change of state the shell did not make
with a call: loading to running, paused or failed; running to exited or
faulted; running or paused to halted. When the machine with the focus
becomes exited, faulted, failed or halted, the focus becomes 0 (and no
home event is sent).

## §machines.frames Frames

For each system frame the host does the following, in order:

1. **Latch.** For each handle in use, in increasing order: a loading handle
   whose image the host now has, or has failed to get, becomes running
   (paused if the run flag is clear) or failed; a running handle whose
   machine stopped in an earlier frame becomes exited or faulted, with its
   exit code or fault kind and `pc`; a running or paused handle whose
   machine the host stopped becomes halted, with the device's slot. Each of these changes queues a machine
   changed event for the shell, ahead of the frame's input. Each record then
   takes the machine's units and frame counter (§machines.calls). Then, in
   increasing handle order, each machine that made an open request in its
   last frame (§system) has it waiting for `request`, and an open
   requested event is queued for the shell, after the machine changed
   events and ahead of the frame's input. A machine that becomes running
   or paused here is reset with the argument its handle was given, if
   any, or, for `restore`, is the machine of the snapshot. Then each
   snapshot the host has stored, or failed to store, since the last
   frame queues a snapshot done event, in the order of the `snapshot`
   calls, and a slot whose snapshot was stored holds it from now on for
   `snapshot_info` (unless the shell deleted it after asking for it).
2. **Route** the frame's input (§machines.input).
3. **The shell** runs its frame (§frames). Then, if the view's
   machine is no longer shown as a panel, the view becomes none (§machines.calls `face`).
4. **Starts and stops** the shell made in its frame are carried out in the
   order it made them, with the arguments it gave. A machine started in
   this frame is loading: its first frame is at the earliest in the next
   system frame, the one in whose step 1 it becomes running. A request
   the shell read with `request` in its frame no longer waits, so the
   machine's next `open` (in step 5) can make another.
5. **The machines** that are running now, in increasing handle order, each
   run one frame (§frames) with a full budget and their own carry. So a
   `run`, `stop` or `place` made by the shell takes effect in the same
   frame; a `focus` takes effect at the next frame's routing, for every
   kind of input, the controllers included (§machines.input).
6. **Snapshots and deletes** the shell asked for in its frame are carried
   out in the order it asked for them: each snapshot is taken of the
   machine as it is now (failed if it no longer exists), each delete
   removes a slot's snapshot.

Paused, loading, exited, faulted, failed and halted machines run no frames, and
their frame counters do not advance. When the shell stops (§stopping), the
system stops: no machine runs any more frames.

## §machines.determinism Determinism

Each machine is deterministic given its own inputs (§system and §content.determinism). For
the shell they include the machine changed, home, machine pressed, open
requested and snapshot done events and what device 8 returns that the
shell did not set itself: the latched records (states, exit codes, faults,
failure reasons, the image IDs and identities of programs started by
identity or restored, units, frame counters), the open requests `request`
delivers, the built-in programs and what `snapshot_info` tells, and the entries
`experimental` reports; the view
is the shell's own state, and the eye the host routes the pointer by is
part of the input. For a machine
the shell started, what `open` returns (whether a request of it still
waits for the shell) is an input like a completion. The identities the shell gave are its own state. Device 8's state is part of the shell's state (`STATE.md`
1.10); a started machine's state is its own and holds nothing of the
system.

## §machines.snapshots Snapshots

A **snapshot** is a machine's complete state, its state serialization
(`STATE.md` 1), with what is needed to recreate it: the image ID and the
argument (both in the serialization) and the program's identity. A machine
restored from a snapshot has the state hash the machine had when the
snapshot was taken and, given the same later inputs, runs exactly as that
machine would have. Its frame counter goes on from the snapshot's; the
time, random bytes and every other input are what the host supplies after
the restore.

- The shell has 16 **slots**, 0 to 15, and the host has 8 **automatic
  slots**, 16 to 23: slot 15 + h holds the snapshot the host took of
  machine h when a device it used was slow (`PERFORMANCE.md` R15: at the
  warning, and again just before it halted the machine). Only the host
  writes them (`snapshot` with slot 16–23 is refused); `restore`,
  `snapshot_info` and `snapshot_delete` take slots 0 to 23, so a shell
  offers them like its own. A host that never halts a machine leaves them
  empty. All 24 are the host's local data,
  kept per shell as named records are (by the shell's identity, else its
  image ID, §content.records), durably (written to a temporary file, synced,
  renamed into place), and never sent anywhere. `STATE.md` 4 gives a
  host's file format; a snapshot may be over 1 MiB.
- A snapshot belongs to its exact image. After a program is updated, a
  snapshot of the old version still restores the old version, by its
  image ID, as long as the host can get that image; it never restores
  into another image.
- Named records (§content.records) are not part of a snapshot: a restored
  machine reads its owner's records as they are now.
- Requests the machine had pending complete as failed in its first frame
  (§content.submit). An open request that waited for the shell (§system
  `open`) is dropped. Locates the machine made in its last frame before
  the snapshot (§page.calls) are not answered. A host lays the page out
  and reports its height (§page.events) as for a new document, and starts
  any afterglow it draws empty (§vector.phosphor).
- The shell itself is never snapshotted: a state with its `STATE.md` section 1.10
  present cannot be restored.

## §machines.charges Charges and limits

Every call is the machine's own (**core**), with the base charge of 4
(§charges), and:

| What | Charge |
|---|---|
| Bytes copied between memory and the device: device 8's `start` (32, when the ID is read, also when no handle is free), `start_program` (33 + n, when the record is read), `info` (48, when written), `builtin` (32, when written), `identity` (81, when written), `request` (82, when written), `argument` (32, when the ID is read, also when it returns 0), `snapshot_info` (122, when written) and `experimental` (32, when written) | 1 per 4 bytes, rounded up, per call |

Provisional values (§provisional):

- the number of machines a shell can start, 8, and of its snapshot slots,
  16 (§machines.shell, §machines.snapshots); program names of 48 bytes (§machines.calls); the
  longest view motion, 600 frames, and the view's progress steps, 4,096
  (§machines.calls `face`);
- the system ink, 691,200 square units, and how it is counted: pictures'
  areas first, then lists' ink times their scale (§machines.ink).

## §machines.unspecified Not yet specified

- In a system (§machines): communication between machines (an open
  request carries one blob ID or program identity, and its machine is not
  told what became of it); arguments other than one blob ID; budgets
  other than one full budget per machine per frame; giving a machine less
  than the whole screen; a volume for each machine's sound, and sound placed
  with a panel in the scene; what a host shows for a machine with no
  picture; telling the shell when a view has finished moving.
- Snapshots (§machines.snapshots): saving them without the shell asking (a host
  has no shutdown notification yet), snapshots of the shell itself, moving
  a snapshot to another host, and a machine asking to be snapshotted.
