//! MAME's vector CRT renderer for Tersenet's vector displays and terminal,
//! as a device driver.
//!
//! The pipeline, the beams and the stroke fonts are terse-host's
//! (`terse_host::mame`), where they are the phosphor style's built-in look
//! in both renderers; they are re-exported here under their old names:
//!
//! - [`pipeline`]: MAME's BGFX vector CRT pipeline (`vectorrenderer.cpp`
//!   and its shaders, BSD-3-Clause), translated to wgpu and WGSL.
//! - [`beams`]: Tersenet's display lists as MAME's beam instances.
//! - [`stroke`]: the stroke fonts (an arcade vector font of our own, the
//!   default; Hershey Roman Simplex; a drafting style) so that the
//!   terminal's text is drawn by the beam. [`arcade`]: the arcade font.
//!
//! The driver is `src/bin/terse-dev-mame.rs`; see the crate's README.

pub use terse_host::mame::gpu as pipeline;
pub use terse_host::mame::{arcade, beams, stroke, text};

pub mod layers;
