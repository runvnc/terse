# terse-mame-crt: MAME's vector CRT renderer as a Tersenet driver

`terse-dev-mame` is a device driver (DEVICES.md) for the vector group
(slots 3 and 9) **or** the terminal (slot 2), whichever the core asks it
for. The devices are terse-core's, exactly as in the reference drivers, so
a machine runs identically with it (its tests compare state hashes frame by
frame). Only the picture differs: in the phosphor style
(SPEC §vector.phosphor) it draws the screen with **MAME's BGFX vector CRT
renderer**, translated from MAME's BSD-3-Clause sources to WGSL and wgpu,
and it draws the terminal's text as **strokes of the beam** (the Hershey
Roman Simplex font and strokes of its own), like a vector monitor's
character generator.

## See it

Built in release mode:

    scripts/cargo build --release -p terse-bus -p terse-devices -p terse-mame-crt

then, from the repository's top (fish or any shell):

    target/release/terse-bus programs/sparks/sparks.terse --window \
        --driver 3,9=target/release/terse-dev-mame --driver 2=target/release/terse-dev-mame

Press P in sparks for the phosphor style. The test pattern and a full
screen of text are `crates/terse-mame-crt/pattern/pattern.terse` and
`text.terse`; any program can be forced into the style with
`--force-phosphor`. Give `--store`, `--config` and `--data` to keep a run
away from your own directories.

Installing it for every run (each group once; it is one program file):

    terse drivers add target/release/terse-dev-mame --slot 3
    terse drivers add target/release/terse-dev-mame --slot 2

`terse drivers use 3 default` and `terse drivers use 2 default` go back.

MAME's sliders are environment variables, clamped to the sliders' ranges:
`TERSE_MAME_PERSISTENCE` (ms, 1–100, MAME's default 2),
`TERSE_MAME_BEAM_WIDTH` (0.30–4.00, 0.75), `TERSE_MAME_BEAM_INTENSITY`
(0.10–5.00, 4.00), `TERSE_MAME_HALO` (0–1, 0.04),
`TERSE_MAME_BLOOM_STRENGTH` (0–3, 0.20), `TERSE_MAME_BLOOM_RADIUS`
(0.50–2.60, 2.12), `TERSE_MAME_EXPOSURE` (0.10–4.00, 0.78).
`TERSE_MAME_TIMES=1` prints each pipeline's frame times when the run ends (submitting, waiting for a picture, the picture into the layer, the whole PRESENT; frames drawn and not drawn). `TERSE_MAME_MAX_LINES` and `TERSE_MAME_LATENCY`: below.

## How it is put together

- `shaders/`: MAME's seven shaders (`vs_beam`, `fs_beam`, `vs_post`,
  `fs_decay`, `fs_downsample`, `fs_blur`, `fs_composite`) translated line
  for line, each keeping MAME's licence and copyright lines, with a comment
  at each construct that had to change (`dFdx` → `dpdx`, `?:` → `select`,
  `texture2D` → `textureSample`, `vec3_splat`, `mul`). `beam_common.wgsl`
  and `post_common.wgsl` declare the interfaces (written for this; MAME's
  `varying.def.sc` has no licence header and was not copied).
- `src/pipeline.rs`: `vectorrenderer.cpp`: two RGBA16F phosphor targets
  ping-ponged and decayed by exp(−dt/τ), the beams added as instanced
  quads, a half-size box downsample, two horizontal and vertical blur
  iterations, the composite; the effects' blend states and the sliders'
  defaults.
- `src/beams.rs`: `draw_beams`' instances (scan timing from accumulated
  length, σ = width × beam width × 0.085) and the width model of MAME's
  `vector.cpp`; Tersenet's lists as MAME's line primitives.
- `src/stroke.rs`, `src/text.rs`: the stroke font and the terminal's
  screen as strokes.
- `src/bin/terse-dev-mame.rs`: the driver. `pattern/`: a test pattern and
  a full screen of text (`build.sh` builds the `.terse` images).
- The originals, unmodified, with their licence: `third_party/mame-vector-crt/`.
  The Hershey font and its terms: `third_party/hershey/`.

## The bus

In the phosphor style the core (terse-bus) normally draws the beam
itself and the vector driver leaves its layers empty (DEVICES.md 7.1). A
driver with its own tube publishes `vector.tube` = 1 (DEVICES.md 7.1,
7.2): its layers are then light like the text's, and the core adds the
light of every light layer as it is, without a tube of its own. Each
group's driver keeps its own phosphor, decayed by the frames it is told of
(END), so the vector picture and the text are two MAME pipelines whose
pictures the core adds.

## Where it is not MAME

- **What drives it.** MAME's games hand it the vector generator's moves
  in drawing order. A Tersenet list is a display list of shapes, so the
  beam's order is the list's (the 2D list, then the 3D list's segments;
  the terminal's text from the top left, in its own process), and the
  timing is MAME's own approximation from that order: constant deflection
  speed, scan time proportional to length.
- **Width.** MAME's default beam is 1/512 of the screen's height; a
  Tersenet line states its width in sixteenths of a unit (1/480 of the
  height for width 16), which is kept: a width-16 line is 512/480 of
  MAME's default beam, and wider lines are wider beams (MAME's calibration
  then makes them brighter more than wider).
- **Colour and intensity.** Colour bytes over 255 (as MAME passes an
  `rgb_t`), opacity as intensity.
- **Shapes MAME does not have.** Filled shapes are outlines, as the
  phosphor style requires. Circles are polygons whose sides stray at most
  a quarter of a pixel from the circle. A 3D segment that fades from one
  end to the other has one intensity, the mean.
- **Targets.** The phosphor targets are the screen's size (MAME's: the
  vector screen's item) up to **960 lines** (`TERSE_MAME_MAX_LINES`,
  480–4320): a larger screen (the 640 × 480 screen three or more times
  over, 1920 × 1440 in a 2560 × 1440 window) has its phosphor and bloom
  drawn at 1280 × 960 and the composite samples them (bilinear, before
  the tone map) up to the screen's size. Widths, σ and the bloom radius
  are MAME's for the phosphor's own height, as MAME would draw at that
  resolution; the picture is that one enlarged, a little softer than one
  drawn at full size (`~/terse-builds/mame-vector-crt/perf/` has both).
  The composite goes to a BGRA8 target cleared to opaque black and read
  back into the light layer (MAME's: added into its view).
- **Latency.** The picture is read back **a frame later**: each PRESENT
  submits this frame's passes and takes the picture of the frame before,
  which the GPU finished meanwhile, through a ring of three buffers, and
  writes it into the layer only where rows changed (the damage is those
  rows). The driver never waits for the GPU (it waited 2–4 ms a frame,
  often more on a busy GPU); the vector picture is one frame behind the
  machine, like a presentation with one frame queued. The first picture
  of a tube, a new layer and a machine that stopped running are not kept
  waiting. `TERSE_MAME_LATENCY=0` waits for each picture instead.
- **Frames not drawn.** When a frame's beams are those of the frames
  before (the same lines, the same time) and the phosphor has reached its
  fixed point (MAME's decay^n ≤ 2^-16: 3 frames at the default 2 ms, 68 at
  100 ms), the frame is not drawn: the picture would be the same. A
  terminal whose text does not change costs nothing.
- **Shedding.** The host's shed level (`core.shed`, DEVICES.md 7.1) gives
  up resolution then bloom: 1 draws the phosphor at most 720 lines high,
  2 at most 480, 3 also without the bloom (MAME's bloom strength 0).
- **Time.** A machine frame is 1/60 s of emulated time; a picture drawn
  when the machine has not run does not excite the phosphor again
  (MAME's paused emulation); the first frame after a size change starts a
  new phosphor.
- **The program's persistence** (0–255) is not used: MAME's persistence
  is its slider (2 ms by default).
- **Text** is not MAME's (MAME has no terminal): it is drawn by the same
  pipeline as strokes, in the terminal's driver, from the stroke font.
- **The clean style** is drawn as the reference drivers draw it (the
  terminal's characters as plain strokes).
- **Without a graphics adapter** the driver says so, publishes
  `vector.tube` = 0, and the core's own tube draws the phosphor style.
- Scenes with Screens or Surfaces are drawn with the core's tube.
