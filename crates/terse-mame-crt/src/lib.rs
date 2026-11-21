//! MAME's vector CRT renderer for Tersenet's vector displays and terminal.
//!
//! - [`pipeline`]: MAME's BGFX vector CRT pipeline (`vectorrenderer.cpp`
//!   and its shaders, BSD-3-Clause), translated to wgpu and WGSL.
//! - [`beams`]: Tersenet's display lists as MAME's beam instances.
//! - [`stroke`]: a stroke font (Hershey Roman Simplex and strokes of our
//!   own) so that the terminal's text is drawn by the beam.
//!
//! The driver is `src/bin/terse-dev-mame.rs`; see the crate's README.

pub mod beams;
pub mod layers;
pub mod pipeline;
pub mod stroke;
pub mod text;
