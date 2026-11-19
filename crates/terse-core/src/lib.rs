//! Terse machine core (SPEC.md, machine profile 1): the RV32IMC CPU, the
//! machine and its devices.
//!
//! Pure and deterministic: no threads, no clock, no OS randomness. The host
//! passes events, the Unix time, random bytes, controller state and content
//! completions in through [`FrameInput`].

pub mod content;
pub mod cpu;
pub mod image;
pub mod machine;
pub mod machines;
pub mod page;
pub mod script;
pub mod shell;
pub mod system;
pub mod terminal;
pub mod vector;
pub mod wide;

pub use content::{Completion, Outcome, Request, RequestKind};
pub use cpu::{Cpu, Fault, FaultKind, RAM_BASE, RAM_INSTRUCTION_COST, ROM_BASE};
pub use image::{DeviceReq, Image, ImageError};
pub use machines::{ProgramId, StartFailure, load_image};
pub use machine::{
    BUDGET_PER_FRAME, ERR_BUFFER, ERR_UNKNOWN, FrameInput, FrameResult, MAX_RANDOM_PER_FRAME, Machine, STATE_DOMAIN,
    Stopped, call, charge,
};
pub use page::{Layout, Layouter, Page, PageInput, PageWatch, Region};
pub use script::Script;
pub use shell::{Child, Driver, System, SystemFrame, Who};
pub use system::{CONTROLLERS, Controller, Event, SCREEN_HEIGHT, SCREEN_WIDTH, buttons, modifiers, pad};
pub use terminal::{CONTINUATION, Cell, COLS, ROWS, Terminal};
pub use vector::{Primitive, Vector};
