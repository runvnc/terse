//! The machine: CPU, bus and the frame loop (SPEC 1.3).

use rustzx_z80::{Z80, Z80Bus};

use crate::content::{Completion, Content, Request};
use crate::image::Image;
use crate::memory::Memory;
use crate::system::{Event, System};
use crate::terminal::Terminal;

/// T-states added to the budget each frame (SPEC 1).
pub const TSTATES_PER_FRAME: i64 = 300_000;
/// Value on the data bus during interrupt acknowledge (SPEC 1.3).
const INTERRUPT_BUS_BYTE: u8 = 0xFF;
/// T-states of an I/O cycle. rustzx-z80 leaves this to the bus.
const IO_CYCLE: u64 = 4;
/// Upper bound on port 1Ch reads in one frame: every instruction that reads a
/// port takes at least 11 T-states and must start with a positive budget.
pub const MAX_RANDOM_READS_PER_FRAME: usize = (TSTATES_PER_FRAME as usize).div_ceil(11);

/// Inputs for one frame (SPEC 5.2).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameInput<'a> {
    pub events: &'a [Event],
    pub unix_time: u32,
    /// Bytes returned by successive reads of port 1Ch in this frame. Reads
    /// beyond the end return 00h; supply `MAX_RANDOM_READS_PER_FRAME` bytes to
    /// never run out.
    pub random: &'a [u8],
    /// Content requests the host finished (SPEC 7.3), applied before the CPU runs.
    pub completions: &'a [Completion],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameResult {
    /// Exit code, if the machine has stopped (in this frame or earlier).
    pub exit: Option<u8>,
    /// T-states executed this frame, including device charges.
    pub tstates: u64,
    /// The CPU halted with no interrupt able to wake it and the rest of the
    /// budget was discarded.
    pub halted_early: bool,
    /// Events dropped because the queue was full.
    pub events_dropped: usize,
    /// How many bytes of `FrameInput::random` were consumed (for recording).
    pub random_used: usize,
    /// Completions ignored because they matched no pending request (see
    /// `Content::complete`).
    pub completions_ignored: usize,
}

/// Everything the CPU sees through its bus.
struct Hardware {
    memory: Memory,
    system: System,
    terminal: Option<Terminal>,
    content: Option<Content>,
    interrupt_request: bool,
    /// T-states of the current step, including device charges.
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

    // SPEC 1 and 3: the upper address byte is ignored; the high nibble selects the device.
    fn read_io(&mut self, port: u16) -> u8 {
        self.step_tstates += IO_CYCLE;
        let (slot, reg) = ((port as u8) >> 4, port as u8 & 0x0F);
        match slot {
            0 => self.memory.port_in(reg),
            1 => self.system.port_in(reg),
            2 => self.terminal.as_mut().map_or(0xFF, |t| t.port_in(reg)),
            4 => self.content.as_ref().map_or(0xFF, |c| c.port_in(reg)),
            _ => 0xFF,
        }
    }

    fn write_io(&mut self, port: u16, data: u8) {
        self.step_tstates += IO_CYCLE;
        let (slot, reg) = ((port as u8) >> 4, port as u8 & 0x0F);
        match slot {
            0 => self.memory.port_out(reg, data),
            1 => self.system.port_out(reg, data),
            2 => {
                if let Some(t) = &mut self.terminal {
                    // SPEC 1.3: charges count as extra T-states of the I/O instruction.
                    self.step_tstates += t.port_out(reg, data) as u64;
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
        INTERRUPT_BUS_BYTE
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

/// A snapshot of the CPU registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registers {
    pub af: u16,
    pub bc: u16,
    pub de: u16,
    pub hl: u16,
    pub af_alt: u16,
    pub bc_alt: u16,
    pub de_alt: u16,
    pub hl_alt: u16,
    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,
    pub i: u8,
    pub r: u8,
    pub iff1: bool,
    pub iff2: bool,
    pub im: u8,
    pub halted: bool,
}

pub struct Machine {
    cpu: Z80,
    hw: Hardware,
    /// Carries a negative remainder between frames (SPEC 1.3).
    budget: i64,
    image_id: [u8; 32],
}

impl Machine {
    pub fn new(image: &Image) -> Self {
        // SPEC 1.2: rustzx-z80's default state is all registers zero,
        // IFF1 = IFF2 = 0, interrupt mode 0.
        Machine {
            cpu: Z80::default(),
            hw: Hardware {
                memory: Memory::new(&image.rom, image.ram_pages),
                system: System::default(),
                terminal: image.has_device(2).then(Terminal::default),
                content: image.has_device(4).then(Content::default),
                interrupt_request: false,
                step_tstates: 0,
            },
            budget: 0,
            image_id: image.id,
        }
    }

    /// Runs one frame (SPEC 1.3). Does nothing once the machine has exited.
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
        let hw = &mut self.hw;

        // Step 1: events and clock.
        for &e in input.events {
            if !hw.system.push_event(e) {
                result.events_dropped += 1;
            }
        }
        hw.system.latch(input.unix_time, input.random);

        // SPEC 7.3: completions, after this frame's input events.
        match &mut hw.content {
            Some(content) => {
                let (ignored, dropped) = content.complete(input.completions, &mut hw.memory, &mut hw.system);
                result.completions_ignored = ignored;
                result.events_dropped += dropped;
            }
            None => result.completions_ignored = input.completions.len(),
        }

        // Step 2: the request is a latch; setting it again is not a second interrupt.
        hw.interrupt_request = true;

        // Step 3.
        self.budget += TSTATES_PER_FRAME;
        while self.budget > 0 {
            let wakeable = hw.interrupt_request && self.cpu.regs.get_iff1();
            if self.cpu.is_halted() && !wakeable {
                result.halted_early = true;
                break;
            }
            hw.step_tstates = 0;
            if self.cpu.emulate(hw) {
                hw.interrupt_request = false;
            }
            self.budget -= hw.step_tstates as i64;
            result.tstates += hw.step_tstates;
            if hw.system.exit_code().is_some() {
                break;
            }
        }
        // Unused budget never carries forward; a negative remainder does.
        self.budget = self.budget.min(0);

        // Step 4 needs no work here: the terminal state is the presented state.
        hw.system.end_frame();
        result.exit = hw.system.exit_code();
        result.random_used = hw.system.random_used();
        result
    }

    pub fn exit_code(&self) -> Option<u8> {
        self.hw.system.exit_code()
    }

    /// The terminal, or `None` if the image does not list device 2.
    pub fn terminal(&self) -> Option<&Terminal> {
        self.hw.terminal.as_ref()
    }

    /// The content device, or `None` if the image does not list device 4.
    pub fn content(&self) -> Option<&Content> {
        self.hw.content.as_ref()
    }

    /// Content requests submitted since the last call (SPEC 7.3), for the host
    /// to carry out.
    pub fn take_content_requests(&mut self) -> Vec<Request> {
        self.hw.content.as_mut().map_or_else(Vec::new, Content::take_requests)
    }

    pub fn system(&self) -> &System {
        &self.hw.system
    }

    /// Number of frames run so far (the frame counter, port 14h).
    pub fn frame(&self) -> u32 {
        self.hw.system.frame()
    }

    /// The carried T-state budget (zero or negative between frames).
    pub fn budget(&self) -> i64 {
        self.budget
    }

    pub fn image_id(&self) -> &[u8; 32] {
        &self.image_id
    }

    pub fn interrupt_requested(&self) -> bool {
        self.hw.interrupt_request
    }

    pub fn slots(&self) -> &[u8; 8] {
        self.hw.memory.slots()
    }

    pub fn ram(&self) -> &[u8] {
        self.hw.memory.ram()
    }

    /// Reads the address space as the CPU sees it (no side effects).
    pub fn peek(&self, addr: u16) -> u8 {
        self.hw.memory.read(addr)
    }

    pub fn registers(&self) -> Registers {
        let r = &self.cpu.regs;
        Registers {
            af: r.get_af(),
            bc: r.get_bc(),
            de: r.get_de(),
            hl: r.get_hl(),
            af_alt: u16::from_be_bytes([r.get_acc_alt(), r.get_flags_alt()]),
            bc_alt: u16::from_be_bytes([r.get_b_alt(), r.get_c_alt()]),
            de_alt: u16::from_be_bytes([r.get_d_alt(), r.get_e_alt()]),
            hl_alt: u16::from_be_bytes([r.get_h_alt(), r.get_l_alt()]),
            ix: r.get_ix(),
            iy: r.get_iy(),
            sp: r.get_sp(),
            pc: r.get_pc(),
            i: r.get_i(),
            r: r.get_r(),
            iff1: r.get_iff1(),
            iff2: r.get_iff2(),
            im: self.cpu.get_im().into(),
            halted: self.cpu.is_halted(),
        }
    }
}
