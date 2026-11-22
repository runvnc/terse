# §sound Device 6 — Sound (version 0)

Part of the Terse machine specification, v0 (draft): `SPEC.md` is the core
machine and says how these documents fit together; this one is complete
for its device, given the core sections it builds on (below).

A small retro synthesizer in the spirit of the MSX SCC: 16 voices, each
playing a 64-sample wavetable, a noise generator or a sample, with an
envelope, a volume and a pan, and three cheap extras taken from FM chips:
**pairing** (one voice's output moves another's phase), **feedback** (a
voice's own output moves its phase) and **ring modulation** (one voice's
output multiplies another's). It is not a mixer and not a full FM
synthesizer.

**Builds on** `SPEC.md`: the call convention (§calls.convention), the
frame loop, whose step 3 renders the samples (§frames), charges
(§charges) and optional devices (§image, §profiles).

Sound is optional (§profiles): neither profile has it, and nothing a
reader shows depends on it. A program that needs it requires device 6 in
its header like any other device; a game whose sound is an extra lists
it as optional and plays only if `device` (§system) says it has it.

Every step of the synthesis below is integer arithmetic on the values the
program set, specified to the bit, so two implementations render the same
samples from the same state, and the samples of a run are a function of
the image and its inputs like everything else (§sound.determinism).

Arithmetic: values are integers; `×` and `+` are exact (the stated ranges
show where they fit in 32 bits); `a >> k` is an arithmetic shift, the
floor of a ÷ 2^k also for a negative a; "mod 2^32" is the low 32 bits, as
an unsigned number.

## §sound.output Output

The device renders **48,000 sample pairs a second**: stereo, each sample
a signed 16-bit number. One frame is **800 pairs** (48,000 ÷ 60). The
device renders a frame's 800 pairs at the end of the frame (§frames
step 3), from its state as the frame's calls left it, so what a program
sets in frame N is heard from the first pair of frame N's samples; the
host plays them after the frame. A frame that does not run (a stopped or
paused machine) renders nothing.

## §sound.calls Calls

| `a7` | Call | Arguments | Result | Class |
|---|---|---|---|---|
| `600h` | `submit` | `a0` = list address, `a1` = length | 0 done, 1 rejected | deferred |
| `601h` | `wave` | `a0` = slot, `a1` = buffer (64 bytes, read) | 0 | deferred |
| `602h` | `sample` | `a0` = slot, `a1` = buffer (read), `a2` = length, `a3` = loop start | 0 | deferred |
| `603h` | `info` | — | `a0` = 16 voices, `a1` = capabilities | deferred |

- **`submit`** carries out the command list (§sound.list) of `a1` bytes
  at `a0` at once, and returns 0. It is rejected, returning 1 and doing
  nothing, if the length is 0 or more than 4,096, or if the list does not
  lie wholly in ROM or wholly in RAM. It costs 2 units per byte of an
  accepted list, like `write` (§terminal.calls), whose bound on how far one
  call can take the budget below zero it shares.
- **`wave`** copies the 64 bytes at `a1` (in ROM or RAM) into wavetable
  `a0` (0–31), each a signed sample (−128 to 127), and returns 0. A slot
  above 31, or a buffer not wholly in ROM or RAM, is a bad buffer
  (`FFFF_FFFEh`, §calls.convention).
- **`sample`** copies the `a2` bytes at `a1` (0 to 65,536, in ROM or RAM)
  into sample slot `a0` (0–3), replacing what it held, and returns 0. Each
  byte is a signed sample. `a3` is the **loop start**: a voice that plays
  past the last byte goes on from byte `a3`; if `a3` is at least the
  length, the sample plays once (§sound.sources). A length of 0 empties the
  slot. A slot above 3, a length above 65,536 or a buffer not wholly in
  ROM or RAM is a bad buffer.
- **`info`** returns 16 (the voices) in `a0` and the **capabilities** in
  `a1`: bit 0 set, the device plays samples. In version 0 every host sets
  bit 0: a host of version 0 must play samples, so that a program's
  samples are the same on every host (§sound.determinism). The bit is there for
  a later version that may leave samples out; a program that wants to run
  on one checks it before it uses `sample` or the Sample command.

Every call is deferred (§charges). After reset every voice is as
§sound.voices says, every wavetable is all zero and every sample slot is
empty (length 0, loop start 0).

## §sound.list Command list

A list is a sequence of commands, each an opcode byte followed by its
operands; multi-byte operands are little-endian. The commands set the
registers of the **current voice**, which is voice 0 at the start of each
list. They take effect at once, in order: the next samples rendered use
them (§sound.output).

| Opcode | Command | Operands | Effect on the current voice |
|---|---|---|---|
| `00h` | End | — | Ends the list |
| `01h` | Voice | v (1 byte, 0–15) | Makes voice v the current voice |
| `02h` | Wave | w (1 byte, 0–31) | Source: wavetable w |
| `03h` | Noise | — | Source: the noise generator |
| `04h` | Sample | s (1 byte, 0–3) | Source: sample slot s |
| `05h` | Frequency | increment (4 bytes) | Sets the phase increment (§sound.voices) |
| `06h` | Volume | volume (1 byte) | 0–255 |
| `07h` | Pan | left, right (1 byte each) | The levels in the left and right outputs, 0–255 each |
| `08h` | Envelope | attack (2), decay (2), sustain (1), release (2) | Sets the envelope (§sound.render) |
| `09h` | Key on | — | Stage attack; the phase becomes 0 and Ended is cleared (§sound.sources). The level is unchanged |
| `0Ah` | Key off | — | Stage release, if the stage is attack, decay or sustain |
| `0Bh` | Modulator | m (1 byte, 0–15 or `FFh`), depth (1 byte) | Pairing: voice m's output moves this voice's phase; `FFh` none |
| `0Ch` | Feedback | depth (1 byte) | This voice's own output moves its phase; 0 none |
| `0Dh` | Ring | q (1 byte, 0–15 or `FFh`) | This voice's output is multiplied by voice q's; `FFh` none |
| `0Eh` | Mix | on (1 byte, 0 or 1) | 1: heard; 0: muted (it is still rendered, so it can modulate or ring another voice) |

- The list ends at an End command, at the end of the submitted bytes, at
  an unknown opcode, at a command whose operands are cut off by the end of
  the list, or at a command with an operand out of range (a voice above
  15, a wavetable above 31, a sample slot above 3, a modulator or ring
  voice above 15 that is not `FFh`, a mix value above 1). The commands
  before it have taken effect; the rest are ignored. `submit` still
  returns 0: like a vector display (§vector.list), the device does not
  report where a list ended, since that would make `submit` an answered
  call (§charges).
- A voice may name itself as its modulator or ring voice; it then reads
  its own output of the previous pair (§sound.render).
- Changing a voice's source does not change its phase, its level or its
  Ended flag.

## §sound.voices Voices

Each voice has these registers, with these values after reset:

| Register | Range | After reset |
|---|---|---|
| Source | wavetable w, noise, or sample slot s | wavetable 0 |
| Phase | 32 bits, unsigned | 0 |
| Increment | 32 bits, unsigned | 0 |
| Volume | 0–255 | 255 |
| Pan | left, right: 0–255 each | 255, 255 |
| Attack, decay, release | 0–65,535 each | 0, 0, 0 |
| Sustain | 0–255 | 255 |
| Stage | off, attack, decay, sustain, release | off |
| Level | 0 to `FF_FFFFh` | 0 |
| Modulator, depth | voice 0–15 or none; 0–255 | none, 0 |
| Feedback | 0–255 | 0 |
| Ring | voice 0–15 or none | none |
| Mix | 0 or 1 | 1 |
| Noise register | 15 bits, never 0 | 1 |
| Ended | flag | clear |
| Output | the voice's output in the last pair rendered, −32,768 to 32,767 | 0 |
| Previous output | its output in the pair before that, likewise | 0 |

The **increment** is how far the phase moves each sample. For a
wavetable voice one turn of the phase (2^32) is one pass through the
table, so a tone of f Hz has the increment round(f × 2^32 ÷ 48,000) (440
Hz: 39,370,534). For a noise voice it sets how often the noise changes
(§sound.sources). For a sample voice it is the playback rate in samples of
the sample per output sample, with 16 fraction bits: 65,536 plays the
sample at 48,000 samples a second, and a sample recorded at r samples a
second plays at its pitch with round(r × 65,536 ÷ 48,000).

## §sound.render Rendering a pair

For each of the frame's 800 pairs, the device computes each voice's
output in turn, voice 0 to voice 15, then mixes them. Let **cur(q)** be
voice q's output in this pair if q is below the voice being computed (it
was computed already), else voice q's Output register: its output in the
previous pair. So a voice paired with a higher-numbered modulator (or
ringed by one) hears it one pair late, and one paired with itself hears
its own previous output. For voice v, in this order:

1. **Envelope.** With T = sustain × 65,793 (so that 255 gives
   `FF_FFFFh`):
   - attack: if attack is 0 or level + attack ≥ `FF_FFFFh`, the level
     becomes `FF_FFFFh` and the stage decay; else the level increases by
     attack;
   - decay: if decay is 0 or level ≤ T + decay, the level becomes T and the
     stage sustain; else the level decreases by decay;
   - sustain: the level becomes T;
   - release: if release is 0 or level ≤ release, the level becomes 0 and
     the stage off; else the level decreases by release;
   - off: the level becomes 0.

   So the rates are steps per pair on a 24-bit level, and 0 is at once:
   the reset envelope (0, 0, 255, 0) is a gate, full from the first pair
   after a key on and silent from the first pair after a key off.
2. **Modulation.** M = cur(m) × depth if the voice has a modulator m,
   else 0, which is less than 2^23 in size; F = (Output(v) + Previous
   output(v)) × feedback, the sum of its own outputs in the last two
   pairs, which is less than 2^24 in size.
3. **Raw sample** r, −128 to 127, from the source (§sound.sources), which
   also advances the phase.
4. **Output.** o = (r × (level >> 8) × volume) >> 16, which lies in
   −32,640 … 32,384. If the voice has a ring voice q, o becomes (o ×
   cur(q)) >> 15.

Then the pair: L is the sum, over the voices whose mix is 1, of o × left;
R likewise with right. The pair's left sample is L >> 10 and its right
sample R >> 10, each clamped to −32,768 … 32,767. (One voice at full
volume and pan reaches a quarter of the range, so four can sound at full
volume before the sum is clamped.) Last, every voice's Previous output
register becomes its Output, and its Output becomes its o.

## §sound.sources Sources

**Wavetable w.** The index is ((phase + M × 1,024 + F × 128) mod 2^32)
>> 26, a number 0–63, and r is wavetable w's sample at that index. Then
the phase becomes (phase + increment) mod 2^32. M and F move only the
index, never the phase: with depth 255 and a modulator at full scale the
index moves by about two passes through the table; with feedback 255 and
the voice at full scale, by about half a pass.

Feedback is the average of the last two outputs (F × 128 is that average
× feedback × 256, with no rounding), as on FM chips: a voice moved by its
last output alone settles into an oscillation from one pair to the next
from a swing of about 1 radian (a sixth of a pass) and is chaotic, heard
as static, soon after; the average cancels that oscillation. It goes
unstable in turn, at a 3-pair period, from a swing of about 2 radians
(about a third of a pass): feedback 165 for a voice at full scale, about
210 at volume 200. Up to there a sine grows from pure through brassy to
nearly a sawtooth as the feedback rises; above it the tone gains a shrill
edge, at most half a pass at 255. Since F follows the output, a voice's
tone also mellows as its envelope falls. A voice that names itself as its
own modulator (§sound.list) is moved by its last output alone, at up to
four times the swing: chaotic from small depths.

**Noise.** r is 127 if bit 0 of the noise register is 1, else −127. Then
the phase becomes (phase + increment) mod 2^32, and if that addition
carried (the sum was 2^32 or more), the noise register n becomes (n >> 1)
| (((n XOR (n >> 1)) AND 1) << 14): a 15-bit linear feedback shift
register of period 32,767. So the noise changes increment × 48,000 ÷ 2^32
times a second. Modulation and feedback do not apply.

**Sample slot s**, of length len (0–65,536) and loop start ls. The phase
is a position in the sample with 16 fraction bits. A position p is
**brought into** the sample: if p < len × 65,536 it stays; else, if ls <
len, it becomes ls × 65,536 + ((p − ls × 65,536) mod ((len − ls) ×
65,536)); else the sample has **ended**. Then:

- if the voice's Ended flag is set, r is 0;
- else the phase is brought into the sample. If the sample has ended, the
  flag is set, the phase becomes 0 and r is 0. Otherwise r is the
  sample's byte at (phase >> 16), and the phase becomes phase + increment
  (computed without a 32-bit limit) brought into the sample; if that has
  ended, the flag is set and the phase becomes 0.

Key on clears Ended (§sound.list). Modulation and feedback do not apply.
A slot loaded again while a voice plays it is read as it now is.

## §sound.presentation Presentation

The 800 pairs of each frame are what the host plays; they are not machine
state and the program cannot read them. In a system (§machines) each
machine has its own sound device; the host adds the pairs of every
machine whose frame ran in a system frame, sample by sample, and clamps
the sums to −32,768 … 32,767. A host may buffer, delay, resample, drop or
not play the samples, as long as nothing in the machines changes: a host
with no audio output has the device all the same (it is present, its
state advances as above, and nothing is heard), as a headless host has a
terminal nobody sees. `terse run --audio-out` and `terse-bus --audio-out`
write the mixed samples to a WAV file instead of playing them.

Whether a host has the device is a property of the host, never of the
audio hardware it finds: an image that lists device 6 is granted it (or,
requiring it, run) the same on a computer with no speakers, no sound
server or a failed output, so `device` (§system), the state and its hash
never depend on them. A program cannot tell whether it is heard.

The user can. A host that plays sound and cannot (it was built without
an output, finds none, cannot open it, or the output stops) tells the
user so, briefly and where they are looking, with the reason: when a
machine first gives a frame that is not silent (a stopped output: when
it stops), once, and again only when the reason changes. The reference
hosts (`terse-host`, `terse-bus` in a window) show a notice and write
one line on standard error; `terse-host --hardware-check` says what the
output is, or why there is none. A muted or silent system is not the
host's to report. A volume and a mute of the host's own apply to the
mixed samples on their way out and to nothing else. A host plays a
frame's samples promptly (`PERFORMANCE.md` R18).

## §sound.determinism Determinism

The device's state is every voice's registers, the wavetables and the
sample slots (`STATE.md` 1.12). It takes no input from the host. The
samples a frame renders are a function of that state, so they are the
same on every host and after a restore; a test compares implementations
by hashing them (`programs/tune/expected-audio.txt`).

## §sound.charges Charges and limits

Every call is deferred, with the base charge of 4 (§charges), and:

| What | Charge |
|---|---|
| Sound `submit`: each byte of an accepted list (§sound.calls) | 2 |
| Bytes copied between memory and the device: sound `wave` (64) and `sample` (its length), each when it is carried out | 1 per 4 bytes, rounded up, per call |

Provisional values (§provisional): sound command lists of 1 to 4,096
bytes and samples of up to 65,536 bytes (§sound.calls). The sound
device's synthesis (48,000 Hz, 800 sample pairs a frame, 16 voices, 32
wavetables of 64 samples, 4 sample slots, every formula) is not a
placeholder: it is the device's version 0.

## §sound.unspecified Not yet specified

- On the sound device (§sound): a volume per machine in a system,
  filters, and samples longer than 65,536 bytes or of more than 8 bits.
