//! What a host needs from a machine, whatever its profile.
//!
//! [`crate::Machine`] (profile 1) implements it; so does the experimental
//! profile-2 machine in the `terse-rv` crate. Hosts that run either drive
//! them through this trait.

use crate::content::Request;
use crate::machine::{FrameInput, FrameResult, Machine};
use crate::terminal::Terminal;
use crate::vector::Vector;

pub trait FrameMachine {
    /// Runs one frame with these inputs.
    fn run_frame(&mut self, input: &FrameInput) -> FrameResult;
    /// Frames run so far (the frame counter).
    fn frame(&self) -> u32;
    /// The exit code, once the machine has stopped.
    fn exit_code(&self) -> Option<u8>;
    /// The terminal, or `None` without device 2.
    fn terminal(&self) -> Option<&Terminal>;
    /// Device 3, or `None` without it.
    fn vector(&self) -> Option<&Vector>;
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
    fn vector(&self) -> Option<&Vector> {
        Machine::vector(self)
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

impl<M: FrameMachine + ?Sized> FrameMachine for Box<M> {
    fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
        (**self).run_frame(input)
    }
    fn frame(&self) -> u32 {
        (**self).frame()
    }
    fn exit_code(&self) -> Option<u8> {
        (**self).exit_code()
    }
    fn terminal(&self) -> Option<&Terminal> {
        (**self).terminal()
    }
    fn vector(&self) -> Option<&Vector> {
        (**self).vector()
    }
    fn vector_list(&self) -> &[u8] {
        (**self).vector_list()
    }
    fn vector_generation(&self) -> u64 {
        (**self).vector_generation()
    }
    fn take_content_requests(&mut self) -> Vec<Request> {
        (**self).take_content_requests()
    }
}
