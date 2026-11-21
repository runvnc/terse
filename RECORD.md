# Tersenet performance record and reports — v1 (draft)

Every machine host keeps a **record** of what made its frames late: which
of its own components (a device driver, its renderer, its core), how long,
whether the computer was merely busy, and what the host did about it
(`PERFORMANCE.md` R13–R16). This document is the record's format, so that
any tool can read any host's record and a Terse32 program could parse it
within its limits.

**Privacy.** The record stays on the computer that made it. Nothing in it
is ever sent anywhere by a host; a performance report made from it is
the user's own, voluntary act (section 5). It holds no host name, user name,
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

## 5. Performance reports

The record never leaves the computer. A **performance report** is a short
summary of it over a period, plus the results of a `terse perf` run if
the user made one, that the user may choose to **publish** in their own
feed, signed with their feed key. Anyone can then **aggregate** the
reports they can see with nothing but Tersenet itself: signed feeds,
relays and a Terse32 program. There is no central collector and nothing
is sent automatically, ever. The purpose: hosts and devices that perform
badly on many people's computers should be seen to, so that they are not
distributed.

**Privacy.** A report is voluntary and public. It is made only when the
user runs `terse perf report`, published only when they confirm it, and
anyone who follows their feed or finds it on a relay can read it. It holds
what 5.2 lists: which host and device builds they run, how those
performed over a period (dates, no times), and the computer coarsely (the
`env` line). It holds no host name, user name, path, address or serial
number, and by default no program IDs (they would say what the user
runs). Tersenet cannot retract an entry (`NETWORK.md`): a published
report cannot be deleted once a relay holds it. To stop, a user publishes
no more.

### 5.1 The carrier: a document of the `reports` format

A report is published as a **document** entry (kind 4, `NETWORK.md` 4.2)
whose blob is a document (`spec/content.md` §content.documents) of the
format `reports` owned by the all-zero key: the format of a built-in
program named `reports`. Bytes 0–44 are the header (`TDOC`, `00h`, 32
zero bytes, `07h`, `reports`), then the report's text (5.2).

Why a document and not an article: documents are typed (the header names
the format, so a program finds reports by their first 45 bytes and
ignores every other document); readers list them as documents, not as
articles to read; they are still found by search (scope 2 searches
article and document titles, `NETWORK.md` 4.7) and downloaded
automatically from followed feeds (a report is at most 8,192 bytes,
below the 16,384 of `NETWORK.md` 4.4). After its 45-byte header the blob
is plain text, so `terse get <id>` shows a person what was published, and
`terse perf report --check <file>` checks and prints it.

The entry's title is `Performance report: <host name> <version>, <first
day> to <last day>`. Titles are for search and people; nothing reads
them as data.

The `reports` program (5.4) is not yet one of the host's built-in
programs (5.7): until it is, a shell asked to open a report document says
it could not start `reports`, and the program runs as an image of its
own.

### 5.2 The format

Printable ASCII (`20h`–`7Eh`) and LF only; every line ends with LF and is
at most 255 bytes. Lines use the record's syntax (section 2): a type word,
then fields separated by single spaces, none empty; strings escaped with
`%` and uppercase hex, the empty string `-`; times in ms with one
decimal. A whole document is at most **8,192 bytes** (a hard cap); a
report should stay within **4,096** (a target: the example below is
906 bytes).

The lines come in this order (no type may follow one later in the table).
`1`: exactly once; `?`: at most once; a number: at most that many.

| Type | # | Fields | Meaning |
|---|---|---|---|
| `terse-report` | 1 | version | `1`; the first line |
| `period` | 1 | first, last | UTC dates of the first and last day covered (first ≤ last) |
| `host` | 1 | name, version, build | The host's identity (R13): name 1–64 bytes; version and build at most 32 bytes without `%`, or `-` |
| `env` | 1 | as the record's | Cores 1–4,096, RAM at most 65,536 GiB |
| `device` | 16 | component, slots, name, version, build | As the record's: `dev<n>` (or `core`), slots 1–255 comma-separated or `-`; components distinct |
| `runs` | 1 | runs, ended, frames, late | Runs begun in the period; of them ended cleanly; their frames and late frames (ended ≤ runs, late ≤ frames) |
| `start` | ? | count, median-ms, worst-ms, cold, cold-worst-ms | Start-ups: how many (≥ 1), median ≤ worst; how many were cold (`build`, `boot`) and the worst of those, `-` exactly when cold is 0 |
| `late` | 20 | component, blamed, starved, memory, spread | Late frames recorded, by verdict (section 4) |
| `warn` | 20 | component, count | R15 warnings about it (≥ 1) |
| `halt` | 20 | component, count | R15 stops: machines halted for it (≥ 1) |
| `shed` | ? | late, check | Times the look was shed for late frames, by the hardware check |
| `mem` | 20 | component, peak-mib, growth | Most resident memory (MiB, one decimal); fastest growth over a run of a minute or more (MiB a minute, two decimals, may be negative) or `-` |
| `check` | 20 | component, ms, result | The latest hardware check: `ok`, `shed`, `slow`, `absent` |
| `perf` | ? | name, version, build, date | The `terse perf` that ran, and when |
| `bench` | 64 | id, value, unit, requirement, ceiling, target | A `terse perf` row: id (≤ 24 bytes, no `%`, distinct); value (an integer or up to 3 decimals) or `-`; unit or `-`; `pass`/`fail`/`-`; `pass`/`fail`/`-`; `met`/`missed`/`-`. Only after `perf` |
| `program` | 5 | image, blamed | Only if asked (`--programs`): 16 hex digits of an image ID, late frames blamed while it ran |
| `end` | 1 | lines | The number of lines before it; the last line |

A component in `late`, `warn`, `halt`, `mem` and `check` is `core`,
`compose`, `present`, `outside` or one a `device` line declared, written
exactly (`dev7`, not `dev07`), at most once per type. Numbers are decimal
without leading zeros, at most 4,294,967,295.

A reader skips lines of a type it does not know (a later version adds
types, and fields only at the end of a line; extra fields are ignored),
except on the first line. A report that breaks any other rule is
**malformed** and ignored whole. The reference parser
(`terse_perflog::report`) names the first rule broken with one of these
codes, used by `vectors/perf-reports.txt`: `not-a-document`,
`other-format`, `too-big`, `not-text`, `no-final-newline`, `first-line`,
`version`, `long-line`, `bad-spacing` (an empty line, a leading or
trailing space, two spaces), `order`, `duplicate`, `fields`, `bad-field`,
`bad-period`, `undeclared-component`, `too-many`, `bench-without-perf`,
`end-count`, `after-end`, `missing-period`, `missing-host`,
`missing-env`, `missing-runs`, `missing-end`.

The `full` vector:

```
terse-report 1
period 2026-11-01 2026-11-21
host terse-bus 0.1.0 b3:5faccb21bc9113fb
env Linux 7.3.1-1-cachyos x86_64 12 15 drivers Intel(R)%20Core(TM)%20i7-10750H%20CPU%20@%202.60GHz
device dev2 2,10 terse-dev-terminal%20(Rust) 0.1.0 b3:3c1f0a9e77d2b410
device dev3 3,9 terse-dev-vector%20(Rust) 0.1.0 b3:9a0b44c1e5f60d27
device dev4 4 terse-dev-content%20(Rust) 0.1.0 b3:97f8a7cc7bf5e08c
device dev7 7 terse-dev-page%20(Rust) 0.1.0 b3:f8f5a88bb33ad6eb
runs 14 12 2592000 61
start 14 131.4 412.0 2 412.0
late outside 0 0 0 1
late core 0 0 1 0
late dev7 41 12 0 0
warn dev7 2
halt dev7 1
shed 1 0
mem core 50.2 0.13
mem dev7 80.0 6.00
check dev3 6.5 ok
perf terse 0.1.0 b3:0f1e2d3c4b5a6978 2026-11-21
bench M1a 1 frames pass - -
bench M2 1 trips pass - -
bench H1 22 ms - - met
bench H6 22.4 ms - - missed
bench W - - pass - -
program abababababababab 3
end 26
```

### 5.3 Making and publishing one

`terse perf report` makes a report from the record of one host identity
(by default the one that ran last; `--host <name>` chooses) over the last
`--days` days (default 30) up to `--to` (default today, UTC) and **prints
it in full**; the period starts on its first day with a run. `--run` also
runs `terse perf` (a minute or two) and adds its rows as `bench` lines;
`--programs <n>` (at most 5) names the programs most often working when a
frame was blamed; `--out <file>` writes the document. Nothing leaves the
computer.

`terse perf report --publish` prints the report, then this, and waits for
an answer on the terminal:

```
This report is about to be published as a document of your feed,
titled "Performance report: terse-bus 0.1.0, 2026-11-21 to 2026-11-21",
signed with your feed key 05a2cf…4ce5cd.

- It is PUBLIC: anyone who follows your feed or finds it on a relay can
  read every line above: which host and devices you run, how they
  performed, and the kind of computer (no name, address or path).
- It CANNOT BE DELETED: once a relay holds it, there is no way to take it
  back. Tersenet has no retraction.
- It is voluntary: nothing is published unless you say yes here, and
  nothing is ever sent automatically. To stop, do not publish again.
- Aggregators count your newest report per host build; one report per
  period is enough.

Publish it? Type yes to publish, anything else cancels:
```

Only `yes` publishes. `--yes` answers for scripts after printing the same
text: the report is still public, signed with the user's feed key and
permanent. It is published as the next entry of the user's feed through
the configured relays (`--relay`, `--store`, `--config`, as for `terse
feed publish`).

**Newer and older reports.** One report per period is enough. For each
feed key and each identity a report speaks about, aggregators count only
the report with the highest `seq`: a newer report replaces the older for
every host, device and `terse perf` build it names; older reports about
other builds still count for those.

**Programs do not make reports.** No machine can read the record, and
this does not change: only the `terse` command makes and publishes
reports. The `reports` program reads published ones, and publishes only
its aggregate, an ordinary article, with the host's consent prompt.

### 5.4 Gathering: programs/reports

`programs/reports/reports.terse` requires devices 2 (Terminal) and 4
(Content) and lists 10 (Full text, for names in any script) as optional;
it runs on a host of the reading profile, which may refuse its searches
and its publish (it then says so). Run it alone (`terse-host
programs/reports/reports.terse`), or publish it in your feed (`terse feed
publish --program programs/reports/reports.terse`) and start it from the
shell's launcher.

- **Feeds you follow.** It reads the names of the feeds the host knows
  (op 09h: whom the user follows, and their names), then the timeline
  (op 04h), again after every feeds-updated event. Every document entry
  of at most 8,192 bytes not taken before (by feed key and `seq`) is
  fetched, four at a time; a `reports` document is parsed by every rule
  of 5.2; other documents are ignored.
- **All found** (F, optional). It searches the relays (find, scope 2)
  for `performance report`, then `performance report: <host name>` for
  up to three host names it has seen, one search every 75 frames (relays
  limit how often a client searches), and takes the results like the
  timeline's. Search finds only feeds with a listed profile, at most 16
  entries a search (newest first): "all found" is a sample, not a census.
  The host offers no lookup of a profile by key, so a reporter found this
  way shows only a short key.
- **Budget.** A frame parses at most 1,536 bytes of reports, line by line
  (a quarter of that in a frame that may draw), scans at most 64
  timeline records and, only in a frame that parsed little, computes one
  identity's numbers; the screen is drawn at most every 6 frames while
  reports arrive. It keeps a small fact per reporter and identity, never
  the reports (at most 256 reporters, 48 identities, 1,024 facts, 1,536
  entries taken). Tested: about 70 reports a second from followed feeds;
  the busiest frame under 70 % of the budget, also with 250 reporters.

The list gives each identity three rows: its kind and identity, then the
verdict among the feeds the user follows and among all found, under one
line that says `Trust "feeds you follow": anyone can make keys, so "all
found" may hold fakes.` Enter shows an identity's numbers in both samples
and its reporters (name or nickname, short key, followed ones first and
marked, the computer's class, what failed). P publishes the aggregate as
an article (a table of every identity's two verdicts, the counts, the
rule, the date), which the host asks the user about. It is never a global
ruling, and the article says so.

### 5.5 The verdict

For one identity (a host, a device, a `terse perf` build) in one sample
(the feeds you follow, or all found):

1. **One report per key.** For each feed key only its report with the
   highest `seq` among those that speak about the identity counts. A
   report speaks about its host, each device identity of its `device`
   lines other than the host's own (a driver serving several components
   counts once), and its `perf` line's build. Malformed reports count for
   nothing; no number in a report counts for more than its one reporter.
2. **The computer's class**, from `env`: **small** with fewer than 4
   logical CPUs or less than 4 GiB; **large** with at least 8 and 16 GiB;
   **medium** otherwise. Small computers are below the hardware floor:
   listed, not judged.
3. A judged reporter **fails** the identity if R15 halted a machine for
   it (a `halt` line for one of its components; for a host, `core`,
   `compose`, `present` or `outside`), or, for a `terse perf` build, a
   `bench` requirement or ceiling failed. Late frames judged `starved` or
   `memory` never count against anything; blamed and warned are shown,
   not failing.
4. Fewer than **5** judged reporters: `TOO FEW REPORTS (n)`. At least
   **1 in 3** failing: `NON-CONFORMANT IN SAMPLE (f of n fail: halted a,
   requirement b, ceiling c)` (the reasons that occur). Otherwise
   `CONFORMANT IN SAMPLE (n reporters)`.

Why 5 and 1 in 3: below five computers one odd machine decides; a third
is far more than chance for a healthy build (a halted device made 10 late
frames of its own within five seconds) yet lets a few honest bad
experiences pass. Each identity also shows a median: a host's start-up, a
device's blamed frames per 10,000, a `terse perf` build's targets missed.

`terse perf aggregate [--follow <keys file>] <file or dir>...` is the
reference: it reads report files (whole documents or the text), takes a
file named `<feed key hex>-<seq>...` as that reporter's entry (any other
name is a reporter of its own), lists the malformed ones and prints every
identity's verdict, in two samples when `--follow` lists the followed
keys. The tests check the program's published aggregate against it.

### 5.6 Test vectors

`vectors/perf-reports.txt`: valid reports (every type of line; the
minimum; a later version's lines) and malformed ones with the code of the
rule each breaks (vectors/README). `cargo test -p terse-perflog` checks
the parser against them; `cargo test -p terse-cli --test reports`
publishes them in a followed feed for the program.

### 5.7 Open questions

- **A built-in.** Making `reports` a built-in program lets a shell open
  report documents and list it in the launcher; it moves the launcher's
  entries after the built-ins, which the shell's scripted tests and
  reference pictures navigate by.
- **Search.** At most 16 results a search, listed feeds only, no profile
  lookup by key: an aggregate of strangers is a small, self-selected
  sample.
- **Defences against faked reports** (not built). Feed keys are free, so
  "all found" can be flooded. Later signals the owner discussed:
  - *vouching*: a feed publishes a signed list of feeds it vouches for,
    and aggregators weight reporters by their distance from the user in
    that graph;
  - *key age and history*: when relays first saw a feed, and how much
    else it has published;
  - optional profile fields (a stated kind of computer, say) that a
    reporter keeps the same over time;
  - *relay-attested distinct networks*: on request, per report, a relay
    adds a keyed, rotating hash of the network prefix the report came
    from, so aggregators can count distinct networks without anyone
    learning addresses (opt-in per report).
