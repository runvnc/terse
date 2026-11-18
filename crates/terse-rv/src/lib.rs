//! Experimental machine profile 2 for Terse: an RV32IMC CPU, flat memory and
//! devices reached through `ecall` (RV-PROFILE.md). Profile 1 (the Z80,
//! `terse_core::Machine`) is unchanged; [`AnyMachine`] runs either, chosen by
//! the image header.

pub mod content;
pub mod cpu;
pub mod machine;

use std::ops::Deref;

use terse_core::image::{HEADER_LEN, PROFILE};
use terse_core::{DeviceReq, FrameInput, FrameMachine, FrameResult, Image, ImageError, Machine, Request, Terminal};

pub use cpu::{Cpu, Fault, FaultKind, RAM_BASE, ROM_BASE};
pub use machine::{INSTRUCTIONS_PER_FRAME, RvMachine, call};

/// The profile byte of a profile-2 image.
pub const PROFILE_RV32: u8 = 2;
/// Offset of the profile byte in the header (SPEC 2).
const PROFILE_OFFSET: usize = 5;

/// A parsed image and its machine profile. Derefs to the [`Image`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub profile: u8,
    pub image: Image,
}

impl Deref for Program {
    type Target = Image;
    fn deref(&self) -> &Image {
        &self.image
    }
}

/// Parses an image of profile 1 or 2. The header rules are SPEC 2's for
/// both; the image ID is the hash of the file as given.
pub fn parse_image(file: &[u8]) -> Result<Program, ImageError> {
    if file.len() >= HEADER_LEN && file[PROFILE_OFFSET] == PROFILE_RV32 {
        // Validate with the profile-1 rules, then restore what differs.
        let mut copy = file.to_vec();
        copy[PROFILE_OFFSET] = PROFILE;
        let mut image = Image::parse(&copy)?;
        image.id = *blake3::hash(file).as_bytes();
        return Ok(Program { profile: PROFILE_RV32, image });
    }
    Ok(Program { profile: PROFILE, image: Image::parse(file)? })
}

/// Builds the file bytes of an image of either profile.
pub fn build_image(profile: u8, title: &str, ram_pages: u8, devices: &[DeviceReq], rom: &[u8]) -> Vec<u8> {
    let mut file = Image::build(title, ram_pages, devices, rom);
    file[PROFILE_OFFSET] = profile;
    file
}

/// A machine of either profile.
#[allow(clippy::large_enum_variant)]
pub enum AnyMachine {
    Z80(Box<Machine>),
    Rv(Box<RvMachine>),
}

impl AnyMachine {
    pub fn new(program: &Program) -> Self {
        match program.profile {
            PROFILE_RV32 => AnyMachine::Rv(Box::new(RvMachine::new(&program.image))),
            _ => AnyMachine::Z80(Box::new(Machine::new(&program.image))),
        }
    }

    pub fn profile(&self) -> u8 {
        match self {
            AnyMachine::Z80(_) => PROFILE,
            AnyMachine::Rv(_) => PROFILE_RV32,
        }
    }

    /// The fault that stopped a profile-2 machine, if any.
    pub fn fault(&self) -> Option<Fault> {
        match self {
            AnyMachine::Z80(_) => None,
            AnyMachine::Rv(m) => m.fault(),
        }
    }
}

impl FrameMachine for AnyMachine {
    fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
        match self {
            AnyMachine::Z80(m) => m.run_frame(input),
            AnyMachine::Rv(m) => m.run_frame(input),
        }
    }
    fn frame(&self) -> u32 {
        match self {
            AnyMachine::Z80(m) => m.frame(),
            AnyMachine::Rv(m) => m.frame(),
        }
    }
    fn exit_code(&self) -> Option<u8> {
        match self {
            AnyMachine::Z80(m) => m.exit_code(),
            AnyMachine::Rv(m) => m.exit_code(),
        }
    }
    fn terminal(&self) -> Option<&Terminal> {
        match self {
            AnyMachine::Z80(m) => m.terminal(),
            AnyMachine::Rv(m) => m.terminal(),
        }
    }
    fn vector_list(&self) -> &[u8] {
        match self {
            AnyMachine::Z80(m) => m.vector_list(),
            AnyMachine::Rv(m) => m.vector_list(),
        }
    }
    fn vector_generation(&self) -> u64 {
        match self {
            AnyMachine::Z80(m) => m.vector_generation(),
            AnyMachine::Rv(m) => m.vector_generation(),
        }
    }
    fn take_content_requests(&mut self) -> Vec<Request> {
        match self {
            AnyMachine::Z80(m) => m.take_content_requests(),
            AnyMachine::Rv(m) => m.take_content_requests(),
        }
    }
}

impl FrameMachine for RvMachine {
    fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
        RvMachine::run_frame(self, input)
    }
    fn frame(&self) -> u32 {
        RvMachine::frame(self)
    }
    fn exit_code(&self) -> Option<u8> {
        RvMachine::exit_code(self)
    }
    fn terminal(&self) -> Option<&Terminal> {
        RvMachine::terminal(self)
    }
    fn vector_list(&self) -> &[u8] {
        RvMachine::vector_list(self)
    }
    fn vector_generation(&self) -> u64 {
        RvMachine::vector_generation(self)
    }
    fn take_content_requests(&mut self) -> Vec<Request> {
        RvMachine::take_content_requests(self)
    }
}
