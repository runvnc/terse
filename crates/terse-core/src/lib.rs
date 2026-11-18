//! Terse machine core (SPEC.md, profile 1).
//!
//! Pure and deterministic: no threads, no clock, no OS randomness. The host
//! passes events, the Unix time and random bytes in through [`FrameInput`].

pub mod content;
pub mod image;
pub mod machine;
pub mod memory;
pub mod system;
pub mod terminal;
pub mod wide;

pub use content::{Completion, Outcome, Request, RequestKind};
pub use image::{DeviceReq, Image, ImageError};
pub use machine::{FrameInput, FrameResult, Machine, Registers, MAX_RANDOM_READS_PER_FRAME, TSTATES_PER_FRAME};
pub use system::{Event, buttons, modifiers};
pub use terminal::{CONTINUATION, Cell, COLS, ROWS, Terminal};
