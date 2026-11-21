# Tersenet performance record — v1 (draft)

Every machine host keeps a **record** of what made its frames late: which
of its own components (a device driver, its renderer, its core), how long,
whether the computer was merely busy, and what the host did about it
(`PERFORMANCE.md` R13–R16). This document is the record's format, so that
any tool can read any host's record and a Terse32 program could parse it
within its limits.

**Privacy.** The record stays on the computer that made it. Nothing in it
is ever sent anywhere by a host; a later, voluntary report is the user's
own act and is not specified here. It holds no host name, user name,
serial number, network address or path, and the computer only coarsely
(the CPU's model string, its core count, the RAM rounded to whole GiB,
the operating system and its release, the renderer). It does name, by the
first 16 hex digits of their image IDs, the programs that ran when a frame
was late, and it carries dates (days, UTC) and times since a run began.

## 1. Where it lives

In the host's data directory: `$XDG_DATA_HOME/terse/perf/` (else
`~/.local/share/terse/perf/`; the platform's equivalent elsewhere). One
file per **host identity**: `<name>-<build hex>.log` (the host's name and
the hex digits of its build identifier, R13; characters other than
letters, digits, `-` and `.` as `_`). A host may write elsewhere when told
to (the reference: `terse-bus --perf-log <dir>`, `terse-host --perf-log
<dir>`); the reference hosts write the user's record only for runs in a
window or in real time.

## 2. Format

UTF-8 text, one record per line, lines ended by LF, each at most 255
bytes. A line is a **type** word and then **fields in a fixed order**,
separated by single spaces. No field is ever empty and none contains a
space:

- numbers are decimal; times in milliseconds have one decimal (`52.3`);
- a string (a name, the CPU model) is written with every byte outside
  `!` to `~`, and `%`, as `%` and two uppercase hex digits (`Intel(R)%20Core(TM)`),
  and an empty string as `-`;
- an image is the first 16 lowercase hex digits of its image ID, or `-`;
- a **component** is `core` (the machines' emulation: the CPU, the System
  and Machines devices, and in a host with built-in devices their calls),
  `compose` (putting the picture together), `present` (handing it to the
  display), `outside` (the frame loop's time in none of these: an event
  loop held elsewhere, a host service waited on), or `dev<n>`: the device
  driver serving slot n (its lowest slot);
- a **verdict** is `blamed`, `starved`, `memory` or `spread` (section 4).

A reader skips blank lines, lines whose first word starts with `#`, and
lines of a known type it cannot parse; it keeps lines of an unknown type
as unknown (a later version may add types; it adds fields only at the end
of a line). The first line of every file is `terse-perf 1`.

| Type | Fields | Meaning |
|---|---|---|
| `terse-perf` | version | The format's version: `1` |
| `run` | date, mode | A run of the host begins: its UTC date (`2026-11-21`); `window`, `realtime`, `headless` or `selftest` |
| `host` | name, version, build | The host's identity (R13) |
| `env` | os, os-version, arch, cores, ram-gib, renderer, cpu | The computer: `Linux`, the kernel's release, `x86_64`, logical CPUs, GiB, the renderer (`software`, `gpu`, `auto`, `drivers`: the drivers draw, `none`), the CPU's model string |
| `device` | component, slots, name, version, build | A device implementation: its slots (comma-separated) and identity. A host whose devices are built in writes one line for `core` with its own identity |
| `start` | ms, cold | From the process's start to its first frame presented. Cold: `build` (the first run of this build: no record of it yet), `boot` (the first since the computer or the user's session started: no marker in the runtime directory), `warm`, or `-` (unknown) |
| `late` | frame, ms, verdict, component, own-ms, cpu-ms, blocked-ms, starved-ms, image | A frame that overran its deadline (section 4): the host's frame number, its time, the verdict, the component with the most own time, that time split into on a CPU and blocked, its starvation, and the program it worked for most |
| `block` | frame, ms, component, what | One wait of 100 ms or more on a component: `frame` work, `start` (a machine's first frames), `open` (starting a machine), `service` (a host service, an image fetched), `state` (a snapshot the rule took) |
| `shed` | frame, level, why | The host shed optional work (the phosphor look, `SPEC.md` §vector.phosphor) to `level` (0 none … 3 plain lines): `late` (frames ran long), `recover`, `check` (the hardware check, R16) |
| `warn` | frame, component, count | The host warned the user (R15): `count` late frames within the warning window |
| `stop` | frame, component, machine, image, saved | The host stopped a machine (R15): its handle (0: the shell or a machine alone), its program, where its snapshot went: `slot<n>` (an automatic slot, `spec/machines.md` §machines.snapshots), `file`, `none` |
| `mem` | seconds, component, rss-kib | A component's resident memory, every 600 frames (10 s) from the run's start: the host's process is `core` |
| `check` | component, ms, result | The start-up hardware check (R16): a device's worst legal frame took this long; `ok`, `shed` (the look is not offered), `slow` (the device's worst frame takes more than a frame), `absent` (not provided) |
| `end` | frames, late, seconds | The run ended cleanly: its frames, how many were late (also those past the late lines' cap), how long it ran |
| `drop` | lines | Lines the writer lost (its queue was full) |

`vectors/perf-record.txt` is an example of every type; `cargo test -p
terse-perflog` parses it, writes it back byte for byte and summarises it.

## 3. Bounds

A record must never grow without limit nor become a cost itself:

- **Size.** A file is renamed `<file>.1` (replacing an older one) when the
  next line would take it past 256 KiB, and a new one begins with
  `terse-perf 1` and the run's `host`, `run`, `env`, `device` and `start`
  lines again, so that every file can be read alone. A directory keeps at
  most 16 files: the oldest go first.
- **Rate.** A run writes at most 2,000 `late` lines; later late frames are
  only counted (`end`). Memory is sampled every 600 frames.
- **The frame loop never waits on it.** Lines go to a queue of 1,024
  that a thread of the host's own writes to the file; a line that finds
  the queue full is dropped and counted (`drop`). The writer works out
  the host's identity itself (hashing the program file takes
  milliseconds). No line is synced to the disk.

## 4. What a reading means

A **late** frame is one whose work (from its start to its picture
presented) took longer than its deadline, one frame (16.7 ms at 60
frames a second). The host attributes the frame's time to components
(`PERFORMANCE.md` R14) and asks the operating system for each one's
starvation (time its threads were runnable but not running: Linux
`/proc/<pid>/task/<tid>/schedstat`) and for the system's memory stall
(PSI, `/proc/pressure/memory`). The verdict:

1. `starved`: less every component's starvation and the frame loop's own,
   the frame would have been on time. The computer was busy.
2. `memory`: the system was stalled on memory for a quarter of a frame or
   more (pages read back from swap). Nobody's fault.
3. `blamed`: the component with the most own time (its time less its
   starvation: on a CPU, or blocked on a disk, a lock, a sleep) had more
   than its allowance, half a frame. Its doing.
4. `spread`: no component's own time was over its allowance.

Own time is split by the component's waited-on thread's time on a CPU
(`cpu-ms`); the rest is `blocked-ms`. Where the operating system does not
say (no schedstat, another system), starvation is unknown and counted as
none: the record then blames by the waits alone, and `env` says which
system it was. A driver stopped by a signal (a debugger, job control)
counts as blocked.
