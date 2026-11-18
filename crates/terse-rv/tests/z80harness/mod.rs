//! A profile-1 machine for measurement: the frame loop of
//! `terse_core::Machine` (SPEC 1.3) rebuilt from the same public parts
//! (rustzx-z80, `Memory`, `System`, `Terminal`, `Vector`, `Content`), with
//! counters the real machine does not keep: instructions executed and the
//! lowest stack pointer. `compare.rs` checks that it gives exactly the real
//! machine's results frame by frame before it uses its counts.
#![allow(dead_code)]

use rustzx_z80::{Z80, Z80Bus};
use terse_core::content::Content;
use terse_core::memory::Memory;
use terse_core::system::System;
use terse_core::{Event, FrameInput, FrameMachine, FrameResult, Image, Request, TSTATES_PER_FRAME, Terminal, Vector};

struct Hardware {
    memory: Memory,
    system: System,
    terminal: Option<Terminal>,
    vector: Option<Vector>,
    content: Option<Content>,
    interrupt_request: bool,
    step_tstates: u64,
}

impl Z80Bus for Hardware {
    fn read_internal(&mut self, addr: u16) -> u8 {
        self.memory.read(addr)
    }
    fn write_internal(&mut self, addr: u16, data: u8) {
        self.memory.write(addr, data);
    }
    fn wait_mreq(&mut self, _addr: u16, clk: usize) {
        self.step_tstates += clk as u64;
    }
    fn wait_no_mreq(&mut self, _addr: u16, clk: usize) {
        self.step_tstates += clk as u64;
    }
    fn wait_internal(&mut self, clk: usize) {
        self.step_tstates += clk as u64;
    }
    fn read_io(&mut self, port: u16) -> u8 {
        self.step_tstates += 4;
        let (slot, reg) = ((port as u8) >> 4, port as u8 & 0x0F);
        match slot {
            0 => self.memory.port_in(reg),
            1 => self.system.port_in(reg),
            2 => self.terminal.as_mut().map_or(0xFF, |t| t.port_in(reg)),
            3 => self.vector.as_ref().map_or(0xFF, |v| v.port_in(reg)),
            4 => self.content.as_ref().map_or(0xFF, |c| c.port_in(reg)),
            _ => 0xFF,
        }
    }
    fn write_io(&mut self, port: u16, data: u8) {
        self.step_tstates += 4;
        let (slot, reg) = ((port as u8) >> 4, port as u8 & 0x0F);
        match slot {
            0 => self.memory.port_out(reg, data),
            1 => self.system.port_out(reg, data),
            2 => {
                if let Some(t) = &mut self.terminal {
                    self.step_tstates += t.port_out(reg, data) as u64;
                }
            }
            3 => {
                if let Some(v) = &mut self.vector {
                    self.step_tstates += v.port_out(reg, data, &self.memory) as u64;
                }
            }
            4 => {
                if let Some(c) = &mut self.content {
                    self.step_tstates += c.port_out(reg, data, &mut self.memory) as u64;
                }
            }
            _ => {}
        }
    }
    fn read_interrupt(&mut self) -> u8 {
        0xFF
    }
    fn reti(&mut self) {}
    fn halt(&mut self, _halted: bool) {}
    fn int_active(&self) -> bool {
        self.interrupt_request
    }
    fn nmi_active(&self) -> bool {
        false
    }
    fn pc_callback(&mut self, _addr: u16) {}
}

pub struct Z80Counter {
    cpu: Z80,
    hw: Hardware,
    budget: i64,
    /// Instructions executed since reset (interrupt acceptances not counted).
    pub instructions: u64,
    /// Instructions in the last frame.
    pub frame_instructions: u64,
    /// The lowest SP seen before an instruction (SP starts at 0000h, i.e. 10000h).
    pub min_sp: u32,
}

impl Z80Counter {
    pub fn new(image: &Image) -> Self {
        Z80Counter {
            cpu: Z80::default(),
            hw: Hardware {
                memory: Memory::new(&image.rom, image.ram_pages),
                system: System::default(),
                terminal: image.has_device(2).then(Terminal::default),
                vector: image.has_device(3).then(Vector::default),
                content: image.has_device(4).then(Content::default),
                interrupt_request: false,
                step_tstates: 0,
            },
            budget: 0,
            instructions: 0,
            frame_instructions: 0,
            min_sp: 0x1_0000,
        }
    }

    pub fn ram(&self) -> &[u8] {
        self.hw.memory.ram()
    }
}

impl FrameMachine for Z80Counter {
    fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
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
        let hw = &mut self.hw;
        let mut feeds_updated = false;
        for &e in input.events {
            if e.kind == Event::FEEDS_UPDATED && std::mem::replace(&mut feeds_updated, true) {
                result.events_dropped += 1;
                continue;
            }
            if !hw.system.push_event(e) {
                result.events_dropped += 1;
            }
        }
        hw.system.latch(input.unix_time, input.random, &input.controllers);
        match &mut hw.content {
            Some(content) => {
                let (ignored, dropped) = content.complete(input.completions, &mut hw.memory, &mut hw.system);
                result.completions_ignored = ignored;
                result.events_dropped += dropped;
            }
            None => result.completions_ignored = input.completions.len(),
        }
        hw.interrupt_request = true;
        self.budget += TSTATES_PER_FRAME;
        self.frame_instructions = 0;
        while self.budget > 0 {
            let wakeable = hw.interrupt_request && self.cpu.regs.get_iff1();
            if self.cpu.is_halted() && !wakeable {
                result.halted_early = true;
                break;
            }
            let halted = self.cpu.is_halted();
            let sp = self.cpu.regs.get_sp() as u32;
            self.min_sp = self.min_sp.min(if sp == 0 { 0x1_0000 } else { sp });
            hw.step_tstates = 0;
            if self.cpu.emulate(hw) {
                hw.interrupt_request = false;
            } else if !halted {
                self.frame_instructions += 1;
            }
            self.budget -= hw.step_tstates as i64;
            result.tstates += hw.step_tstates;
            if hw.system.exit_code().is_some() {
                break;
            }
        }
        self.instructions += self.frame_instructions;
        self.budget = self.budget.min(0);
        hw.system.end_frame();
        result.exit = hw.system.exit_code();
        result.random_used = hw.system.random_used();
        result
    }
    fn frame(&self) -> u32 {
        self.hw.system.frame()
    }
    fn exit_code(&self) -> Option<u8> {
        self.hw.system.exit_code()
    }
    fn terminal(&self) -> Option<&Terminal> {
        self.hw.terminal.as_ref()
    }
    fn vector_list(&self) -> &[u8] {
        self.hw.vector.as_ref().map_or(&[], Vector::list)
    }
    fn vector_generation(&self) -> u64 {
        self.hw.vector.as_ref().map_or(0, Vector::generation)
    }
    fn take_content_requests(&mut self) -> Vec<Request> {
        self.hw.content.as_mut().map_or_else(Vec::new, Content::take_requests)
    }
}
