# Spec findings from a clean-room core

`tcore` was written from `SPEC.md`, `STATE.md`, `DEVICES.md`, the manual's
`machine.txt`, `state.txt` and `writing-programs.txt`, the program images
and their expected files, and the RISC-V unprivileged specification,
without reading the reference implementation (`crates/`, `programs/**/*.c`,
`programs/lib/`) or the device documents of `spec/`. The reference binaries
were used only as black boxes: the drivers, `terse run` (hashes, `--dump`,
`--save-state` to compare serializations byte by byte) and their `--help`.

This file lists every place where those documents did not settle a
question: the document and section, the question, what was assumed, and,
checked at the end against the reference, whether the assumption was right.

**Summary.** 31 findings: 2 blocking (the run failed until the answer was
found by experiment), 14 guesses, 15 about wording or missing pointers.
Three guesses were wrong (1, 2, 3 below); every other guess the tests
could reach was right. Once the three were fixed, every comparison matched:
the `hello` vectors, all 20 hashes of `conform-program`, `conform` and
`conform-reading`, five other programs, 42 hand-made images and thousands of
random CPU images. The CPU, the System device, the budget, the charges,
`BLAKE3` and the serialization were right on the first run.

## Resolved

Every finding below has since been settled in the documents, and both
cores (the Rust reference and `tcore`) follow the written rule. "test.sh"
is this directory's; Rust tests are named by file.

| # | Decision | Where | Tests and vectors | Code changed |
|---|---|---|---|---|
| 1 | BEGIN's replay field is always present: a u32 length, 0 unless flag bit 0 | DEVICES 1.5 | every run of test.sh; `native/driver-example` parses it | none |
| 2 | Slots that share state (3 and 9) are asked for together whenever a machine lists either; OPEN lists only the slots the machine has; STATE of a machine with only 9 asks slot 3's section | DEVICES 1.1, 1.7 (table), 12 | test.sh: `holo`, `conform-program` | C core: asks for [3, 9] in both cases (it asked [9, 3] for holo but [3] for an image with 3 only) |
| 3 | `exit` writes 0 to `a0`, leaves `pc` after the `ecall`, is charged 4 | SPEC §system, STATE 1.3 | STATE 3 `exit` vector (a 42-byte image, two hashes); `terse-core/tests/state.rs` `exit_test_vector`; test.sh "exit vector" | none (C core fixed while it was written) |
| 4 | After `exit` the budget is min(budget, 0): a negative remainder is kept | SPEC §frames | `tests/exitneg0`–`4` | none |
| 5 | Alignment is checked first: misaligned and outside is a misaligned fault | SPEC §stopping | `terse-core/tests/edge.rs` `misaligned_before_outside_and_long_encodings`; `tests/misout` | none |
| 6 | Low bits `11`: both parcels are fetched; a 48-bit or longer encoding is an illegal instruction with its 32 bits, or a fetch fault at the second parcel | SPEC §cpu | `edge.rs` (as 5); `tests/wide` | none |
| 7 | An unknown call, a call to a device the machine lacks, and `a7` above `FFFFh` cost 4 | SPEC §charges, §calls.convention | `tests/sysonly` | none |
| 8 | At most one feeds-updated event a frame, on every machine, whoever sent it | SPEC §events | `conform-program.events` frame 5; `tests/script.events` | none |
| 9 | Keys are case-insensitive; one character is a name (`5` is the key 5), two or more digits a usage ID, `0x` hex | STATE 6.2 | `tests/script.events`; `script.rs` `key_names` | C core: case, `escape`, `control`, `05` (was octal-capable `strtol`) |
| 10 | `press` sends no text; `click` is buttons 1 then 0 in the frame; `text` is the rest after one whitespace character, `#` included; positions are clamped, negatives refused; lines in any frame order | STATE 6.2 | `tests/script.events` (and as CR LF) | C core: a tab after `text`, negative positions refused, lines sorted by frame |
| 11 | Out-of-range values refuse the file (status 2): sticks −128, buttons above `FFFF`, pointer buttons above 7, wheel −128. The clamping guess was wrong | STATE 6.2 | test.sh "refused:" (14 lines, both cores); `script.rs` `errors_name_the_line` | Rust: wheel −128 and pointer buttons above 7 refused (were accepted). C core: refuses instead of clamping |
| 12 | CALL's budget: after the `ecall`'s own cost, less 4, for every call | DEVICES 1.4 | — (no driver reads it) | none |
| 13 | The checked argument is checked for every call that has one, with or without a buffer | DEVICES 5 | — | none |
| 14 | A driver answers for exactly the slots asked, or all but 10; else the run does not start (status 2). The guess (unserved slots) was wrong | DEVICES 1.1 | test.sh "wrong slots" (both cores) | C core: refuses a driver that leaves a slot out |
| 15 | Every reference driver's options and environment, with the content driver's defaults (the user's own directories) | DEVICES 2.1 | — | test.sh gives the content driver by path |
| 16 | `--dump`: a continuation after a non-continuation is skipped, any other is a space, control codes as they are, surrogates as U+FFFD | STATE 6.3 | `tests/dump.py` (both cores and terse-bus) | C core: control codes were spaces, lone continuations skipped, surrogates written raw |
| 17 | A bus core needs no Unicode tables or palette: UTF-8 and White_Space only | SPEC §minimal | — | none |
| 18 | The minimal core's and minimal driver's message lists | DEVICES 12, 13 | `native/driver-example` | none |
| 19 | One drivers directory (by default the core's own; never PATH for the defaults); `--driver` a path or a name on PATH; optional device without its default driver is absent; a `--driver` that cannot start refuses the run; slots that are no device are never asked | DEVICES 2.1 | test.sh: no content driver, empty directory, `--driver` to nothing (both cores) | C core: default directory (was PATH), `--driver` lookup, refusal; Rust: `terse-bus` never asks for a slot that is no device |
| 20 | The session directory is always given and exists | DEVICES 1.1 | — | none |
| 21 | The core's one device-specific table: sections' order and presence | DEVICES 1.7, STATE 1, DEVICES 14 (open question) | — | none |
| 22 | Device 8 on a machine run alone: not refused, absent, section `00h` | SPEC §image, DEVICES 1.7 | test.sh `shell run alone` | none |
| 23 | Calls to 8 (not a shell), 10 and slots that are no device return `FFFF_FFFFh` | SPEC §calls.convention | `tests/sysonly` | none |
| 24 | `open`'s checks in order, each reading only what the last showed | SPEC §system | `tests/sysonly` | none |
| 25 | Every driver gets every host event of the frame | DEVICES 1.5 | — | none |
| 26 | A position-1 event whose index is no host event's is dropped | DEVICES 1.5 | — | none |
| 27 | Status 2: the run did not start (image, events file, driver missing or failing its handshake); 3: a driver failed while running; a fault ends the process by SIGTERM | STATE 6.4, DEVICES 2.1, 10 | test.sh status checks; "a fault ends terse-bus and terse run by SIGTERM" | Rust: `terse-bus` ends by SIGTERM after a fault and prints it (exited 255, an exit code). C core: handshake failures 2 (were 3) |
| 28 | The stopping frame is printed only if its counter is a multiple of N | STATE 3 | — | none |
| 29 | Already clear | — | — | none |
| 30 | The core reads only the fields an entry uses; three argument numbers are `FFh` when unused | DEVICES 5 | — | none |
| 31 | Headless: time 0, no random bytes, controllers from the script | SPEC §minimal, STATE 6.1 | — | none |

## Blocking (found only by running the reference drivers)

1. **DEVICES 1.5, BEGIN: is the replay field always there?** The table
   lists "Replay: the driver's recorded input for this frame (u32 length,
   bytes)" after the events, and flag bit 0 says "a recorded input follows
   (replay)". I read that as: the field is present only with bit 0. The
   terminal driver answered the first BEGIN with "message cut short" and
   closed. **The field is always present** (a u32 0 when not replaying).
   Wrong first guess. Suggested wording: "Replay: the driver's recorded input
   for this frame (u32 length, then bytes; length 0 unless flag bit 0)".

2. **DEVICES 1.1 / 1.2 / 1.7, device 9 without device 3.** `programs/holo`
   requires device 9 and not 3. I asked the vector driver for slot 9 only;
   OPEN was refused: "device 9 needs this driver to serve device 3 too
   (SPEC §vector3d.calls)". DEVICES says a core "starts one driver for both"
   and that STATE uses "slot 3's section for an image that lists only device
   9", but not that the **HELLO must ask for slot 3 although the image does
   not list it**, nor whether OPEN then lists slot 3 (it must not: the
   machine does not have it; with HELLO [9, 3] and OPEN [9] the hashes
   match). Wrong first guess.

## Had to guess

3. **SPEC §system `exit`: the value of `a0` afterwards.** "does not return",
   and §calls.convention says a call changes only `a0` (and `a1`). The
   state holds every register, so it matters. I left `a0` unchanged; **the
   reference writes 0 to `a0`** (found by comparing `--save-state` bytes:
   offset 85). Wrong first guess. SPEC should say "`a0` becomes 0".

4. **SPEC §frames, budget after `exit`.** "The rest of that frame's budget
   is dropped": is a negative budget (the `ecall` and its 4 units taking it
   below zero) kept or set to 0? Assumed: a positive remainder becomes 0, a
   negative one is kept. Right (tests/exitneg0–4).

5. **SPEC §cpu / §stopping: a load or store that is both misaligned and
   outside memory.** Which fault? Assumed misaligned (checked first).
   Right (tests/misout).

6. **SPEC §cpu: 48-bit and longer encodings.** Is the first parcel enough
   to fault (and which encoding is recorded, 16 or 32 bits), or must the
   second parcel be fetched first? Assumed: fetched as any instruction with
   low bits `11` (a fetch fault if the second half is missing), recorded as
   the 32 bits. Right for an encoding inside ROM (tests/wide); the fetch
   case was not tested.

7. **SPEC §charges: an unknown call, a call to an absent device, or `a7`
   above `FFFFh`.** "Any call, including one that fails: its base charge",
   but such a call has no class. DEVICES 5 says 4 for an unknown function
   of a driver's device; nothing says it for an absent device or a slot
   above 255. Assumed 4 in every case. Right (tests/sysonly).

8. **SPEC §events / DEVICES 1.5: feeds updated on a machine without
   content.** The conformance script sends two feeds-updated events in frame
   5 to `conform-program`, which has no content device. SPEC.md does not
   say whether a host queues it at all, and the "one feeds-updated event a
   frame" rule is in §content.events, a document a program-profile
   implementer is told it need not read (DEVICES 1.5 repeats it in a
   parenthesis). Assumed: queued, at most one a frame. Right.

9. **writing-programs.txt, the events file: keys.** "A key is a USB HID usage
   ID (`40` or `0x28`) or a name: `a`-`z`, `0`-`9` …": is `5` the key 5
   (`22h`) or usage 5? Assumed a single digit is the name. Untested (no
   script uses one).

10. **The events file: `press`, `click`, `text`.** Does `press` send a text
    event too? (Assumed not.) Is `click` a pointer event with button 1 then
    one with button 0, in the same frame? (Assumed yes.) Does `text` take
    the rest of the line after exactly one space, `#` included? (Assumed
    yes.) Are `pointer` positions clamped to the screen? (Assumed yes.)
    Right as far as `conform*` and `hello` exercise them.

11. **The events file: `pad` stick values of −128 and buttons above
    `FFFFh`.** Assumed clamped to −127 and truncated to 16 bits. Untested.

12. **DEVICES 1.4, CALL's budget field: "what is left after the `ecall`
    instruction and the call's 4".** For an answered call the base is 64.
    Assumed the budget after the instruction, less 4. No reference driver
    reads it.

13. **DEVICES 5, the checked argument of a call with no buffer.** "For a
    call with a buffer, in this order: … 3. The checked argument" — is it
    checked for a call without one? Assumed yes. No reference table has
    such a call.

14. **DEVICES 1.1, a driver that answers for fewer slots than asked** (other
    than 10). Assumed: those slots are not served (an optional one not
    granted, a required one refused). Untested.

15. **The content driver's command line.** DEVICES 2 says terse-bus starts
    `terse-dev-content` "given --store, --config, --data, --relay,
    --consent, --profile", not how. Assumed command-line flags of the same
    names (`--driver "4=terse-dev-content --store … --config … --data …"`).
    `conform` and `conform-reading` matched, but they start from an empty
    store, which a driver ignoring the flags would also see (with HOME in a
    temporary directory). Probably right; not proven.

16. **`--dump` (writing-programs.txt "Running it headless").** The format
    is shown only by the expected files: 30 lines, trailing spaces dropped,
    continuation cells skipped. What of control codes in cells? Assumed a
    space. Right for every screen compared.

## Wording and missing pointers

17. **SPEC §minimal is written for a host with its devices built in.** For
    a bus core the list is different, and shorter: none of the Unicode
    tables, the palette or the 25 device calls' semantics stay in the core
    (they are the drivers'); what stays is UTF-8 validation and the
    White_Space property for `open`'s display names. A paragraph "A core
    on the device bus implements …" pointing at DEVICES would help.

18. **DEVICES never lists the minimal message set.** A headless single
    machine needs HELLO, OPEN, CALL/RESULT, READ/DATA (content's data
    ranges), BEGIN/INPUTS, END, STATE and SHUTDOWN; BATCH only for speed;
    not RESET, CLOSE, RESTORE, GET/SET, SERVICE or PRESENT. That GET/SET
    are needed only before PRESENT (7.2: "for a subscriber, before it
    presents") and for device 8 is there, but has to be worked out.

19. **DEVICES 2: where a core finds drivers.** The start protocol
    (`TERSE_BUS_SOCKET`, connect, HELLO within 10 s) is precise; which
    program serves which slot is described only as what terse-bus does
    ("from its own directory"). Fine for a reference, but say that a core
    chooses, and give the default names as a table.

20. **DEVICES 1.1, HELLO's session directory and bus file** when the core
    neither presents nor offers shared memory: required? Assumed a session
    directory is created anyway (cheap) and the bus file is an empty string.

21. **STATE.md 1.6–1.12 and DEVICES 1.7: the core must know every device's
    section rules.** DEVICES says the core "knows nothing of what any other
    device means", but to serialize it must know the order of the sections,
    which have a present flag (2, 3/9, 4, 7), which are absent unless the
    device is had (9, 6) and the special case of 8. A new device would need
    a core change for the state hash. Worth stating as the one place the
    core is device-aware (or adding a "state order" byte to the HELLO reply).

22. **STATE.md 1.10 for a machine run alone whose image lists device 8.**
    SPEC §image says device 8 is "absent elsewhere whatever the header says";
    STATE 1.10 says the section is a `00h` flag then. Consistent, but the
    two are far apart. (`programs/shell` run alone matched.)

23. **SPEC §calls.convention, "a call to a device the machine does not
    have".** Calls to slot 8 on a machine run alone and to slot 10 (a
    device with no calls) both return `FFFF_FFFFh`; stated for 10, implied
    for 8. Fine; listed because both needed a second look.

24. **SPEC §system `open`: how a request is checked.** The request's length
    depends on bytes inside it (kind, then n). The order of the checks
    (byte 0 readable, kind, bytes 0–33 readable, n in 1–48, the whole
    request readable, the name) is implied; the charge "whenever it is
    valid" is clear. Right as implemented (tests/sysonly).

25. **DEVICES 1.5: BEGIN to every driver with all host events.** "as the
    system routed them to this machine" — every driver of the machine gets
    every event, not only those it might want. Assumed so. Right.

26. **DEVICES 1.5: where host events go relative to drivers' position-1
    events whose index is past the last host event.** Assumed dropped.
    Untested (the page driver never sends one).

27. **DEVICES 10: the exit status for a refused image or a missing
    required driver.** Only the status for a failed driver (3) is given.
    `tcore` uses 2, as `terse run` does for a refused image (found by
    running it).

28. **STATE.md 3, the `--hash-every` rule when the machine stops** between
    multiples: is the stopping frame printed? Assumed not, unless its
    counter is a multiple. Checked only with `--hash-every 1`.

29. **SPEC §frames: "A negative remainder … may be larger than a frame's
    budget".** Clear; only noting that `write`'s 8,199 bound and the content
    charge are the only ways, both computable by the core.

30. **DEVICES 5: "Fields an entry does not use are zero, but argument
    numbers, which are `FFh`"** — the buffer address argument (offset 2) of
    a call with no buffer: `FFh` or 0? Irrelevant (flag bit 2 says whether
    there is a buffer) but the sentence reads as if every argument field
    were `FFh`.

31. **The manual's state.txt says the replay events file gives "time stays
    0 and there are no random bytes"**, STATE.md 3 says the same of "no
    input". A core run headless therefore needs no clock or randomness at
    all; worth saying in SPEC §minimal for testers.

## Was every charge computable by the core?

Yes. The System calls are the core's (4 plus the bytes copied, all in SPEC
§charges). A driver's deferred calls are charged from its call table
alone (DEVICES 5): `--no-batch` sends them as ordinary calls and checks the
driver's RESULT against the table's result and charge; every call of
`conform-program` agreed. Answered calls are 64 plus the RESULT's charge,
and content delivered at a frame's start is INPUTS's charge. The only
charge not stated in SPEC is that of a call to an absent device (7).
