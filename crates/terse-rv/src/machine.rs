//! The profile-2 machine: the RV32IMC core, flat memory, the frame loop and
//! the devices reached through `ecall` (RV-PROFILE.md).

use terse_core::memory::Memory;
use terse_core::system::{Event, System};
use terse_core::terminal::{CELL_BYTES, COLS, ROWS, SCROLL_CLEAR_CHARGE};
use terse_core::{FrameInput, FrameResult, Image, Request, Terminal, Vector};

use crate::content::{RvContent, ram_range, readable};
use crate::cpu::{Bus, Code, Cpu, Fault, RAM_BASE, ROM_BASE, Stop};

/// Instructions a program may execute each frame (every instruction costs 1).
pub const INSTRUCTIONS_PER_FRAME: i64 = 500_000;
/// Device calls are charged one instruction per this many bytes they move
/// to or from guest memory, rounded up.
pub const BYTES_PER_CHARGE: usize = 4;
/// Charge for a terminal clear, scroll up or scroll down (SPEC 6.5's 600
/// T-states, scaled by the same 4 as the byte charge).
pub const SCROLL_CLEAR_INSTRUCTIONS: u32 = 150;
/// Exit code reported (`exit_code`, `FrameResult::exit`) after a fault.
pub const FAULT_EXIT: u8 = 0xFF;
/// What an unknown call returns, and a call given a bad buffer.
pub const ERR_UNKNOWN: u32 = 0xFFFF_FFFF;
pub const ERR_BUFFER: u32 = 0xFFFF_FFFE;

/// Instructions charged for moving `bytes` bytes.
pub fn transfer_charge(bytes: usize) -> u32 {
    bytes.div_ceil(BYTES_PER_CHARGE) as u32
}

/// Call numbers: `a7 = (device slot << 8) | function` (RV-PROFILE.md section 5).
pub mod call {
    pub const WAIT_FRAME: u32 = 0x100;
    pub const NEXT_EVENT: u32 = 0x101;
    pub const FRAME: u32 = 0x102;
    pub const TIME: u32 = 0x103;
    pub const RANDOM: u32 = 0x104;
    pub const CONTROLLER: u32 = 0x105;
    pub const EXIT: u32 = 0x106;

    pub const TERM_WRITE: u32 = 0x200;
    pub const TERM_COMMAND: u32 = 0x201;
    pub const TERM_SET_CURSOR: u32 = 0x202;
    pub const TERM_GET_CURSOR: u32 = 0x203;
    pub const TERM_SET_COLOURS: u32 = 0x204;
    pub const TERM_GET_COLOURS: u32 = 0x205;
    pub const TERM_CELLS_READ: u32 = 0x206;
    pub const TERM_CELLS_WRITE: u32 = 0x207;
    pub const TERM_PALETTE_SET: u32 = 0x208;
    pub const TERM_SIZE: u32 = 0x209;

    pub const VEC_SUBMIT: u32 = 0x300;
    pub const VEC_CLEAR: u32 = 0x301;

    pub const CONTENT_SUBMIT: u32 = 0x400;
}

const CELLS: u32 = (COLS * ROWS) as u32;

pub struct RvMachine {
    cpu: Cpu,
    rom: Vec<u8>,
    /// The ROM, decoded once.
    code: Code,
    /// RAM, held in a profile-1 `Memory` so that device 3 reads it unchanged
    /// (its slot registers are never used).
    mem: Memory,
    system: System,
    terminal: Option<Terminal>,
    vector: Option<Vector>,
    content: Option<RvContent>,
    /// Carries a negative remainder (a device charge) into the next frame.
    budget: i64,
    fault: Option<Fault>,
    image_id: [u8; 32],
    /// Instructions executed since reset, device charges not included.
    instructions: u64,
    /// Device calls made since reset.
    calls: u64,
    /// Instructions executed at each pc, when profiling (see `set_profiling`).
    profile: Option<std::collections::HashMap<u32, u64>>,
}

impl RvMachine {
    pub fn new(image: &Image) -> Self {
        let mem = Memory::new(&[], image.ram_pages);
        let ram_end = RAM_BASE + mem.ram().len() as u32;
        RvMachine {
            cpu: Cpu::new(ROM_BASE, ram_end),
            rom: image.rom.clone(),
            code: Code::new(&image.rom),
            mem,
            system: System::default(),
            terminal: image.has_device(2).then(Terminal::default),
            vector: image.has_device(3).then(Vector::default),
            content: image.has_device(4).then(RvContent::default),
            budget: 0,
            fault: None,
            image_id: image.id,
            instructions: 0,
            calls: 0,
            profile: None,
        }
    }

    /// Runs one frame. Does nothing once the machine has exited or faulted.
    pub fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
        let mut result = FrameResult {
            exit: self.exit_code(),
            tstates: 0,
            halted_early: false,
            events_dropped: 0,
            random_used: 0,
            completions_ignored: 0,
        };
        if result.exit.is_some() {
            return result;
        }

        // Inputs, as in profile 1 (SPEC 1.3 step 1, 7.3).
        let mut feeds_updated = false;
        for &e in input.events {
            if e.kind == Event::FEEDS_UPDATED && std::mem::replace(&mut feeds_updated, true) {
                result.events_dropped += 1;
                continue;
            }
            if !self.system.push_event(e) {
                result.events_dropped += 1;
            }
        }
        self.system.latch(input.unix_time, input.random, &input.controllers);
        match &mut self.content {
            Some(content) => {
                let (ignored, dropped) = content.complete(input.completions, self.mem.ram_mut(), &mut self.system);
                result.completions_ignored = ignored;
                result.events_dropped += dropped;
            }
            None => result.completions_ignored = input.completions.len(),
        }

        self.budget += INSTRUCTIONS_PER_FRAME;
        let start = self.budget;
        let mut charged = 0;
        while self.budget > 0 {
            let stop = {
                let mut bus = Bus { rom: &self.rom, ram: self.mem.ram_mut() };
                match &mut self.profile {
                    None => self.cpu.run_cached(&self.code, &mut bus, &mut self.budget),
                    Some(counts) => {
                        // One instruction at a time, counting each pc.
                        let (pc, mut one) = (self.cpu.pc, 1);
                        let stop = self.cpu.run(&mut bus, &mut one);
                        if one == 0 {
                            *counts.entry(pc).or_default() += 1;
                            self.budget -= 1;
                        }
                        match stop {
                            Stop::Budget if self.budget > 0 => continue,
                            s => s,
                        }
                    }
                }
            };
            match stop {
                Stop::Budget => break,
                Stop::Fault(f) => {
                    self.fault = Some(f);
                    break;
                }
                Stop::Ecall => {
                    self.calls += 1;
                    let (charge, wait) = self.ecall();
                    self.budget -= charge as i64;
                    charged += charge as u64;
                    if wait {
                        result.halted_early = true;
                        break;
                    }
                    if self.system.exit_code().is_some() {
                        break;
                    }
                }
            }
        }
        result.tstates = (start - self.budget) as u64;
        self.instructions += result.tstates - charged;
        // Unused budget never carries forward; a negative remainder does.
        self.budget = self.budget.min(0);

        self.system.end_frame();
        result.exit = self.exit_code();
        result.random_used = self.system.random_used();
        result
    }

    /// Carries out the call in a7; returns (charge beyond the `ecall`
    /// itself, whether the program waits for the next frame).
    fn ecall(&mut self) -> (u32, bool) {
        let x = &self.cpu.x;
        let (n, a0, a1, a2) = (x[17], x[10], x[11], x[12]);
        let mut r1 = None;
        let mut charge = 0;
        let mut wait = false;
        let ram = self.mem.ram_mut();
        let r0 = match n {
            call::WAIT_FRAME => {
                // Returns the number of the frame it returns in.
                wait = true;
                self.system.frame().wrapping_add(1)
            }
            call::NEXT_EVENT => match ram_range(ram.len(), a0, 4) {
                None => ERR_BUFFER,
                Some(at) => {
                    charge = transfer_charge(4);
                    let kind = self.system.port_in(0);
                    if kind == 0 {
                        0
                    } else {
                        let ev = [kind, self.system.port_in(1), self.system.port_in(2), self.system.port_in(3)];
                        ram[at..at + 4].copy_from_slice(&ev);
                        self.system.port_out(0, 0);
                        1
                    }
                }
            },
            call::FRAME => u32::from_le_bytes(std::array::from_fn(|i| self.system.port_in(4 + i as u8))),
            call::TIME => u32::from_le_bytes(std::array::from_fn(|i| self.system.port_in(8 + i as u8))),
            call::RANDOM => match ram_range(ram.len(), a0, a1 as usize) {
                None => ERR_BUFFER,
                Some(at) => {
                    for b in &mut ram[at..at + a1 as usize] {
                        *b = self.system.port_in(0xC);
                    }
                    charge = transfer_charge(a1 as usize);
                    a1
                }
            },
            call::CONTROLLER => match ram_range(ram.len(), a1, 8) {
                None => ERR_BUFFER,
                Some(at) => {
                    self.system.port_out(0xD, if a0 < 4 { a0 as u8 } else { 0xFF });
                    for b in &mut ram[at..at + 8] {
                        *b = self.system.port_in(0xE);
                    }
                    charge = transfer_charge(8);
                    ram[at] as u32
                }
            },
            call::EXIT => {
                self.system.port_out(0xF, a0 as u8);
                0
            }
            0x200..=0x2FF if self.terminal.is_some() => {
                let t = self.terminal.as_mut().unwrap();
                let scaled = |c: u32| c / SCROLL_CLEAR_CHARGE * SCROLL_CLEAR_INSTRUCTIONS;
                match n {
                    call::TERM_WRITE => match readable(&self.rom, ram, a0, a1 as usize) {
                        None => ERR_BUFFER,
                        Some(bytes) => {
                            charge = transfer_charge(bytes.len());
                            for &b in bytes {
                                charge += scaled(t.port_out(0, b));
                            }
                            a1
                        }
                    },
                    call::TERM_COMMAND => {
                        if a0 < 0x100 {
                            charge = scaled(t.port_out(5, a0 as u8));
                        }
                        0
                    }
                    call::TERM_SET_CURSOR => {
                        t.port_out(1, a0.min(COLS as u32 - 1) as u8);
                        t.port_out(2, a1.min(ROWS as u32 - 1) as u8);
                        0
                    }
                    call::TERM_GET_CURSOR => {
                        r1 = Some(t.port_in(2) as u32);
                        t.port_in(1) as u32
                    }
                    call::TERM_SET_COLOURS => {
                        // A value above 255 leaves that colour as it is.
                        if a0 < 0x100 {
                            t.port_out(3, a0 as u8);
                        }
                        if a1 < 0x100 {
                            t.port_out(4, a1 as u8);
                        }
                        0
                    }
                    call::TERM_GET_COLOURS => {
                        r1 = Some(t.port_in(4) as u32);
                        t.port_in(3) as u32
                    }
                    call::TERM_CELLS_READ | call::TERM_CELLS_WRITE => {
                        let len = a2 as usize * CELL_BYTES;
                        let buf = if a0 < CELLS && a2 <= CELLS {
                            if n == call::TERM_CELLS_READ {
                                ram_range(ram.len(), a1, len).map(|at| &ram[at..at + len])
                            } else {
                                readable(&self.rom, ram, a1, len)
                            }
                        } else {
                            None
                        };
                        match buf {
                            None => ERR_BUFFER,
                            Some(_) => {
                                let p = (a0 as usize * CELL_BYTES) as u16;
                                t.port_out(6, p as u8);
                                t.port_out(7, (p >> 8) as u8);
                                if n == call::TERM_CELLS_READ {
                                    let at = ram_range(ram.len(), a1, len).unwrap();
                                    for b in &mut ram[at..at + len] {
                                        *b = t.port_in(8);
                                    }
                                } else {
                                    for &b in readable(&self.rom, ram, a1, len).unwrap() {
                                        t.port_out(8, b);
                                    }
                                }
                                charge = transfer_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_PALETTE_SET => {
                        let len = a2 as usize * 3;
                        match readable(&self.rom, ram, a1, len).filter(|_| a0 < 256 && a2 <= 256) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                t.port_out(9, a0 as u8);
                                for &b in bytes {
                                    t.port_out(0xA, b);
                                }
                                charge = transfer_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_SIZE => {
                        r1 = Some(ROWS as u32);
                        COLS as u32
                    }
                    _ => ERR_UNKNOWN,
                }
            }
            call::VEC_SUBMIT if self.vector.is_some() => {
                let v = self.vector.as_mut().unwrap();
                // SPEC 8.2's rules: a list not wholly in RAM is rejected like a bad length.
                let (lin, len) = match ram_range(ram.len(), a0, a1 as usize) {
                    Some(at) if a1 <= 0xFFFF => (at as u32, a1),
                    _ => (0, 0),
                };
                for (reg, b) in [(0, lin), (1, lin >> 8), (2, lin >> 16), (3, len), (4, len >> 8)] {
                    v.port_out(reg, b as u8, &self.mem);
                }
                if v.port_out(5, terse_core::vector::CMD_SUBMIT, &self.mem) > 0 {
                    charge = transfer_charge(len as usize);
                }
                v.port_in(5) as u32
            }
            call::VEC_CLEAR if self.vector.is_some() => {
                let v = self.vector.as_mut().unwrap();
                v.port_out(5, terse_core::vector::CMD_CLEAR, &self.mem);
                v.port_in(5) as u32
            }
            call::CONTENT_SUBMIT if self.content.is_some() => {
                let (handle, c) = self.content.as_mut().unwrap().submit(a0, &self.rom, ram);
                charge = c;
                handle
            }
            _ => ERR_UNKNOWN,
        };
        self.cpu.x[10] = r0;
        if let Some(r1) = r1 {
            self.cpu.x[11] = r1;
        }
        (charge, wait)
    }

    /// The exit code once the program has exited, or [`FAULT_EXIT`] after a fault.
    pub fn exit_code(&self) -> Option<u8> {
        self.system.exit_code().or(self.fault.map(|_| FAULT_EXIT))
    }

    /// The fault that stopped the machine, if any.
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }

    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminal.as_ref()
    }

    pub fn vector(&self) -> Option<&Vector> {
        self.vector.as_ref()
    }

    pub fn vector_list(&self) -> &[u8] {
        self.vector.as_ref().map_or(&[], Vector::list)
    }

    pub fn vector_generation(&self) -> u64 {
        self.vector.as_ref().map_or(0, Vector::generation)
    }

    pub fn content(&self) -> Option<&RvContent> {
        self.content.as_ref()
    }

    pub fn take_content_requests(&mut self) -> Vec<Request> {
        self.content.as_mut().map_or_else(Vec::new, RvContent::take_requests)
    }

    pub fn system(&self) -> &System {
        &self.system
    }

    pub fn frame(&self) -> u32 {
        self.system.frame()
    }

    pub fn cpu(&self) -> &Cpu {
        &self.cpu
    }

    pub fn ram(&self) -> &[u8] {
        self.mem.ram()
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// The carried budget (zero or negative between frames).
    pub fn budget(&self) -> i64 {
        self.budget
    }

    pub fn image_id(&self) -> &[u8; 32] {
        &self.image_id
    }

    /// Instructions executed since reset (device charges not included).
    pub fn instructions(&self) -> u64 {
        self.instructions
    }

    /// Turns on (or off, discarding it) a count of the instructions executed
    /// at each address. Slow; for finding where a program's time goes.
    pub fn set_profiling(&mut self, on: bool) {
        self.profile = on.then(Default::default);
    }

    /// Instructions executed at each address since profiling was turned on.
    pub fn profile(&self) -> Option<&std::collections::HashMap<u32, u64>> {
        self.profile.as_ref()
    }

    /// Device calls (`ecall`s) since reset.
    pub fn calls(&self) -> u64 {
        self.calls
    }
}
