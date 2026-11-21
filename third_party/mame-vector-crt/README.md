# MAME's BGFX vector CRT renderer (third-party originals)

Unmodified copies of the files of MAME's BGFX vector CRT renderer that
`crates/terse-mame-crt` translates for Tersenet (to WGSL and Rust, for
wgpu). They are kept here, as they were upstream, so that the translation
can be checked line by line and so that their copyright notices and
licence travel with the derived files.

- Upstream: https://github.com/mamedev/mame
- Commit: `6f9a2f023e1a951ba34f289d6a4d214a9578676b` (master, fetched
  2026-11-21)
- Licence of the copied files: BSD-3-Clause, text in `LICENSE` (a copy of
  upstream `bgfx/chains/LICENSE`, identical to `bgfx/effects/LICENSE`).
  MAME as a whole is GPL-2.0-or-later with per-file licences; only files
  whose own header says BSD-3-Clause (or, for the effect JSON files, that
  live under `bgfx/effects/`, whose `LICENSE` is BSD-3-Clause) were copied
  or translated. Nothing GPL was copied, translated or read for this work.

## Files

| Here | Upstream path | Header licence | Copyright holders | Use |
|---|---|---|---|---|
| `shaders/vs_beam.sc` | `src/osd/modules/render/bgfx/shaders/chains/vector-crt/vs_beam.sc` | BSD-3-Clause | Hans Andersson | translated: `crates/terse-mame-crt/shaders/vs_beam.wgsl` |
| `shaders/fs_beam.sc` | same directory | BSD-3-Clause | Hans Andersson | translated: `shaders/fs_beam.wgsl` |
| `shaders/vs_post.sc` | same directory | BSD-3-Clause | Hans Andersson | translated: `shaders/vs_post.wgsl` |
| `shaders/fs_decay.sc` | same directory | BSD-3-Clause | Hans Andersson | translated: `shaders/fs_decay.wgsl` |
| `shaders/fs_downsample.sc` | same directory | BSD-3-Clause | Hans Andersson | translated: `shaders/fs_downsample.wgsl` |
| `shaders/fs_blur.sc` | same directory | BSD-3-Clause | Hans Andersson | translated: `shaders/fs_blur.wgsl` |
| `shaders/fs_composite.sc` | same directory | BSD-3-Clause | Hans Andersson | translated: `shaders/fs_composite.wgsl` |
| `effects/beam.json`, `blur.json`, `composite.json`, `decay.json`, `downsample.json` | `bgfx/effects/vector-crt/` | no header; directory `bgfx/effects/LICENSE`: BSD-3-Clause | Ryan Holtz and MAME Development Team (the directory's notice) | translated: the blend states and default uniforms in `crates/terse-mame-crt/src/pipeline.rs` |
| `cpp/vectorrenderer.cpp`, `cpp/vectorrenderer.h` | `src/osd/modules/render/bgfx/` | BSD-3-Clause | Hans Andersson | translated: `crates/terse-mame-crt/src/pipeline.rs` (targets, passes, decay, bloom radii, sliders' defaults) and `src/beams.rs` (beam instances and scan timing) |
| `cpp/vector.cpp` | `src/emu/vector.cpp` | BSD-3-Clause | Brad Oliver, Aaron Giles, Bernd Wiebelt, Allard van der Bas | translated (a few lines): the beam width model in `crates/terse-mame-crt/src/beams.rs` |
| `docs/bgfx_vector_crt.rst` | `docs/source/techspecs/bgfx_vector_crt.rst` | CC0-1.0 (`docs/LICENSE`, copied as `docs/LICENSE-CC0`) | MAMEdev | read as documentation; not translated |

**Not copied:** `varying.def.sc` from the same shader directory. It has no
licence header, so by MAME's `COPYING` it falls under the GPL; it only
declares the varyings and vertex attributes, and the WGSL interface structs
in `crates/terse-mame-crt/shaders/` were written independently from the
shaders' own uses of those names.

The shaders include bgfx's `common.sh` (bgfx's shader helpers, BSD-2-Clause,
from the bgfx project); it was not copied: the WGSL uses WGSL's own
built-ins (`dpdx`, `textureSample`, ...) in place of its macros.
