//! What a host needs from a machine, whatever its profile.
//!
//! [`crate::Machine`] (profile 1) implements it; so does the experimental
//! profile-2 machine in the `terse-rv` crate. Hosts that run either drive
//! them through this trait.

use crate::content::Request;
use crate::machine::{FrameInput, FrameResult, Machine};
use crate::terminal::Terminal;

pub trait FrameMachine {
    /// Runs one frame with these inputs.
    fn run_frame(&mut self, input: &FrameInput) -> FrameResult;
    /// Frames run so far (the frame counter).
    fn frame(&self) -> u32;
    /// The exit code, once the machine has stopped.
    fn exit_code(&self) -> Option<u8>;
    /// The terminal, or `None` without device 2.
    fn terminal(&self) -> Option<&Terminal>;
    /// The display list device 3 holds (empty without it).
    fn vector_list(&self) -> &[u8];
    /// Changes whenever `vector_list` changes.
    fn vector_generation(&self) -> u64;
    /// Content requests submitted since the last call.
    fn take_content_requests(&mut self) -> Vec<Request>;
}

impl FrameMachine for Machine {
    fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
        Machine::run_frame(self, input)
    }
    fn frame(&self) -> u32 {
        Machine::frame(self)
    }
    fn exit_code(&self) -> Option<u8> {
        Machine::exit_code(self)
    }
    fn terminal(&self) -> Option<&Terminal> {
        Machine::terminal(self)
    }
    fn vector_list(&self) -> &[u8] {
        Machine::vector_list(self)
    }
    fn vector_generation(&self) -> u64 {
        Machine::vector_generation(self)
    }
    fn take_content_requests(&mut self) -> Vec<Request> {
        Machine::take_content_requests(self)
    }
}
