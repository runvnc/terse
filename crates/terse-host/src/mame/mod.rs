//! The phosphor style's default look (SPEC §vector.phosphor leaves the look
//! to the host): MAME's vector CRT renderer, its beams, its phosphor and
//! bloom and its tone map, and the terminal's text drawn by the beam as
//! strokes.
//!
//! - [`params`]: MAME's sliders and the shaders' sources (BSD-3-Clause,
//!   translated from MAME).
//! - [`beams`]: Tersenet's display lists as MAME's beam instances
//!   (BSD-3-Clause in part).
//! - [`gpu`]: MAME's pipeline on wgpu (BSD-3-Clause, translated), used by
//!   the GPU renderer and by the `terse-dev-mame` driver.
//! - `soft::mame`: the same pipeline computed on the CPU for the software
//!   renderer (BSD-3-Clause: the shaders' arithmetic, ported).
//! - [`stroke`], [`arcade`], [`text`]: the stroke fonts and the terminal's
//!   screen as strokes.

pub mod arcade;
pub mod beams;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod params;
pub mod stroke;
pub mod text;

pub use params::{BEAM_SIGMA_SCALE, BLOOM_PASSES, Params};
