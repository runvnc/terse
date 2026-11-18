//! Terse machine core (SPEC.md, profile 1).
//!
//! Pure and deterministic: no threads, no clock, no OS randomness. The host
//! passes events, the Unix time, random bytes, controller state and content
//! completions in through [`FrameInput`].

pub mod content;
pub mod image;
pub mod machine;
pub mod memory;
pub mod script;
pub mod system;
pub mod terminal;
pub mod vector;
pub mod wide;

pub use content::{Completion, Outcome, Request, RequestKind};
pub use image::{DeviceReq, Image, ImageError};
pub use machine::{
    FrameInput, FrameResult, Machine, Registers, MAX_RANDOM_READS_PER_FRAME, STATE_DOMAIN, TSTATES_PER_FRAME,
};
pub use script::Script;
pub use system::{CONTROLLERS, Controller, Event, SCREEN_HEIGHT, SCREEN_WIDTH, buttons, modifiers, pad};
pub use terminal::{CONTINUATION, Cell, COLS, ROWS, Terminal};
pub use vector::{Primitive, Vector};
