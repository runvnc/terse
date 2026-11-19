//! The machine: the RV32IMC core, ROM and RAM, the frame loop and the
//! devices reached through `ecall` (SPEC 1, 3, 11).

use std::fmt;

use crate::content::{Completion, Content, Request, ram_range, readable};
use crate::cpu::{Bus, Code, Cpu, Fault, FaultKind, RAM_BASE, ROM_BASE, Stop};
use crate::image::Image;
use crate::machines::{Machines, ProgramId};
use crate::page::Page;
use crate::system::{CONTROLLERS, Controller, Event, System};
use crate::terminal::{CELL_BYTES, CELLS, COLS, ROWS, Terminal};
use crate::vector::Vector;

/// Leading domain string of the state serialization (STATE.md).
pub const STATE_DOMAIN: &[u8] = b"terse-state-1";

/// Budget units added each frame (SPEC 1.4). Provisional.
pub const BUDGET_PER_FRAME: i64 = 125_000;

/// The charges of SPEC 13, in budget units. Provisional.
pub mod charge {
    /// Every call, on top of the `ecall` instruction itself.
    pub const CALL: u32 = 4;
    /// Terminal `write`, per byte.
    pub const WRITE_BYTE: u32 = 1;
    /// Raw copies between guest memory and a device: one unit per this many
    /// bytes, rounded up per call.
    pub const COPY_BYTES_PER_UNIT: usize = 4;
    /// A terminal clear, scroll up or scroll down, also when caused by `write`.
    pub const SCROLL_CLEAR: u32 = 150;
}

/// The longest `random` request (SPEC 4).
pub const MAX_RANDOM_PER_CALL: u32 = 256;

/// The most random bytes a program can take in one frame: each `random`
/// call that starts with a positive budget takes at most 256 bytes and
/// costs at least its instruction, the call charge and the copy.
pub const MAX_RANDOM_PER_FRAME: usize = (BUDGET_PER_FRAME as usize)
    .div_ceil(1 + charge::CALL as usize + MAX_RANDOM_PER_CALL as usize / charge::COPY_BYTES_PER_UNIT)
    * MAX_RANDOM_PER_CALL as usize;

/// What an unknown call, or a call to an absent device, returns.
pub const ERR_UNKNOWN: u32 = 0xFFFF_FFFF;
/// What a call given a bad buffer returns.
pub const ERR_BUFFER: u32 = 0xFFFF_FFFE;

/// Units charged for copying `bytes` bytes.
pub fn copy_charge(bytes: usize) -> u32 {
    bytes.div_ceil(charge::COPY_BYTES_PER_UNIT) as u32
}

/// The fault kind's number (STATE.md 1.3; SPEC 12.2 `info`).
pub fn fault_number(f: &Fault) -> u8 {
    match f.kind {
        FaultKind::Illegal(_) => 1,
        FaultKind::Ebreak => 2,
        FaultKind::Fetch => 3,
        FaultKind::Load => 4,
        FaultKind::Store => 5,
        FaultKind::Misaligned => 6,
    }
}

/// Call numbers: `a7 = (device slot << 8) | function` (SPEC 3.2).
pub mod call {
    pub const WAIT_FRAME: u32 = 0x100;
    pub const NEXT_EVENT: u32 = 0x101;
    pub const FRAME: u32 = 0x102;
    pub const TIME: u32 = 0x103;
    pub const RANDOM: u32 = 0x104;
    pub const CONTROLLER: u32 = 0x105;
    pub const EXIT: u32 = 0x106;
    pub const OPEN: u32 = 0x107;
    pub const ARGUMENT: u32 = 0x108;

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
    pub const VEC_STYLE: u32 = 0x302;
    pub const VEC_PERSISTENCE: u32 = 0x303;
    pub const VEC_FILL: u32 = 0x304;

    pub const CONTENT_SUBMIT: u32 = 0x400;

    pub const PAGE_SUBMIT: u32 = 0x700;
    pub const PAGE_CLEAR: u32 = 0x701;
    pub const PAGE_REGION: u32 = 0x702;
    pub const PAGE_SCROLL: u32 = 0x703;
    pub const PAGE_BACKGROUND: u32 = 0x704;
    pub const PAGE_LOCATE: u32 = 0x705;
}

/// Why the machine stopped (SPEC 1.5): the program exited, or a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    Exit(u8),
    Fault(Fault),
}

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stopped::Exit(code) => write!(f, "exit {code}"),
            Stopped::Fault(fault) => write!(f, "fault: {fault}"),
        }
    }
}

/// Inputs for one frame (SPEC 4.2).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameInput<'a> {
    pub events: &'a [Event],
    pub unix_time: u32,
    /// Bytes for the `random` calls of this frame, taken in order. Calls
    /// beyond the end get zeros; supply `MAX_RANDOM_PER_FRAME` bytes to never
    /// run out (`FrameResult::random_used` says how many were taken).
    pub random: &'a [u8],
    /// Content requests the host finished (SPEC 9.2), applied before the CPU runs.
    pub completions: &'a [Completion],
    /// The four controllers (SPEC 6), latched for the frame.
    pub controllers: [Controller; CONTROLLERS],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameResult {
    /// Set once the machine has stopped (in this frame or earlier).
    pub stopped: Option<Stopped>,
    /// Budget units used this frame: instructions, call charges and the
    /// charge for content delivered at its start.
    pub used: u64,
    /// The program called `wait_frame` and the rest of the budget was
    /// discarded.
    pub waited: bool,
    /// Events dropped because the queue was full, and feeds-updated events
    /// beyond the first in this frame's input (SPEC 5: at most one per frame).
    pub events_dropped: usize,
    /// How many bytes of `FrameInput::random` were consumed (for recording).
    pub random_used: usize,
    /// Completions ignored because they matched no pending request (see
    /// `Content::complete`).
    pub completions_ignored: usize,
}

impl FrameResult {
    /// The exit code, if the program has exited.
    pub fn exit(&self) -> Option<u8> {
        match self.stopped {
            Some(Stopped::Exit(code)) => Some(code),
            _ => None,
        }
    }
}

pub struct Machine {
    cpu: Cpu,
    rom: Vec<u8>,
    /// The ROM, decoded once.
    code: Code,
    ram: Vec<u8>,
    system: System,
    terminal: Option<Terminal>,
    vector: Option<Vector>,
    content: Option<Content>,
    page: Option<Page>,
    /// Device 8, only for the shell (SPEC 12.1).
    machines: Option<Machines>,
    /// The image lists device 8 (STATE.md 1.10 is then present).
    lists_machines: bool,
    /// The budget; zero or negative between frames (a negative remainder
    /// carries into the next frame).
    budget: i64,
    stopped: Option<Stopped>,
    image_id: [u8; 32],
    /// Instructions executed since reset (not machine state).
    instructions: u64,
    /// Device calls made since reset (not machine state).
    calls: u64,
    /// The program's identity (SPEC 12.2), if it was started as one; whose
    /// named records it reads (SPEC 9.6). Not machine state.
    identity: Option<ProgramId>,
    /// The machine was started by a shell (SPEC 12.1), so `open` can pass
    /// a request to it.
    in_system: bool,
    /// The request `open` passed that the shell has not read yet, and
    /// whether the shell has been told of it (SPEC 4). Not machine state.
    open_request: Option<([u8; crate::machines::REQUEST_BYTES], bool)>,
    /// The blob ID the shell gave the machine before its first frame
    /// (STATE.md 1.11).
    argument: Option<[u8; 32]>,
}

impl Machine {
    /// The machine at reset (SPEC 1.3).
    pub fn new(image: &Image) -> Self {
        let ram = vec![0; image.ram_pages as usize * 8192];
        let ram_end = RAM_BASE + ram.len() as u32;
        Machine {
            cpu: Cpu::new(ROM_BASE, ram_end),
            rom: image.rom.clone(),
            code: Code::new(&image.rom),
            ram,
            system: System::default(),
            terminal: image.has_device(2).then(Terminal::default),
            vector: image.has_device(3).then(Vector::default),
            content: image.has_device(4).then(Content::default),
            page: image.has_device(7).then(Page::default),
            machines: None,
            lists_machines: image.has_device(crate::machines::SLOT),
            budget: 0,
            stopped: None,
            image_id: image.id,
            instructions: 0,
            calls: 0,
            identity: None,
            in_system: false,
            open_request: None,
            argument: None,
        }
    }

    /// The machine is one a shell started (SPEC 12.1): `open` passes its
    /// requests to the shell.
    pub fn set_in_system(&mut self, on: bool) {
        self.in_system = on;
    }

    /// Its argument (SPEC 4, 12.2), set before its first frame.
    pub fn set_argument(&mut self, argument: Option<[u8; 32]>) {
        self.argument = argument;
    }

    pub fn argument(&self) -> Option<&[u8; 32]> {
        self.argument.as_ref()
    }

    /// The open request waiting for the shell (SPEC 4), if any, and whether
    /// the shell has been told of it.
    pub fn open_request(&self) -> Option<&([u8; crate::machines::REQUEST_BYTES], bool)> {
        self.open_request.as_ref()
    }

    /// The shell has been told of the waiting request (SPEC 12.5).
    pub fn announce_open_request(&mut self) -> Option<[u8; crate::machines::REQUEST_BYTES]> {
        let (bytes, told) = self.open_request.as_mut()?;
        (!std::mem::replace(told, true)).then_some(*bytes)
    }

    /// The shell read the request: another may be made.
    pub fn clear_open_request(&mut self) {
        self.open_request = None;
    }

    /// The identity the host started the machine with (SPEC 12.2).
    pub fn set_identity(&mut self, identity: Option<ProgramId>) {
        self.identity = identity;
    }

    pub fn identity(&self) -> Option<&ProgramId> {
        self.identity.as_ref()
    }

    /// The machine at reset as the shell (SPEC 12.1): device 8 is granted
    /// if the image lists it; `builtins` are the IDs `builtin` returns.
    pub fn new_shell(image: &Image, builtins: Vec<[u8; 32]>) -> Self {
        let mut m = Machine::new(image);
        if m.lists_machines {
            m.machines = Some(Machines::new(builtins));
        }
        m
    }

    /// Runs one frame (SPEC 1.4). Does nothing once the machine has stopped.
    pub fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
        let mut result = FrameResult {
            stopped: self.stopped,
            used: 0,
            waited: false,
            events_dropped: 0,
            random_used: 0,
            completions_ignored: 0,
        };
        // The last frame's locates are the host's now.
        if let Some(page) = &mut self.page {
            page.begin_frame();
        }
        if self.stopped.is_some() {
            return result;
        }

        // Step 1: inputs.
        let mut feeds_updated = false;
        let mut skip_y = false;
        for &e in input.events {
            if e.kind == Event::FEEDS_UPDATED && std::mem::replace(&mut feeds_updated, true) {
                result.events_dropped += 1;
                continue;
            }
            // SPEC 11.5: the two events of a located anchor are queued
            // together or not at all.
            if e.kind == Event::PAGE_LOCATED_Y && std::mem::take(&mut skip_y) {
                result.events_dropped += 1;
                continue;
            }
            if e.kind == Event::PAGE_LOCATED && self.system.queue().len() + 2 > crate::system::QUEUE_CAPACITY {
                result.events_dropped += 1;
                skip_y = true;
                continue;
            }
            skip_y = false;
            if !self.system.push_event(e) {
                result.events_dropped += 1;
            }
        }
        self.system.latch(input.unix_time, input.random, &input.controllers);
        let mut delivered_charge = 0;
        match &mut self.content {
            Some(content) => {
                // After a restore, the requests pending in the snapshot fail
                // first (SPEC 9.2).
                let orphans = content.take_orphans();
                let joined: Vec<Completion>;
                let completions = if orphans.is_empty() {
                    input.completions
                } else {
                    joined = orphans.into_iter().chain(input.completions.iter().cloned()).collect();
                    &joined
                };
                let (ignored, dropped, delivered) = content.complete(completions, &mut self.ram, &mut self.system);
                result.completions_ignored = ignored;
                result.events_dropped += dropped;
                delivered_charge = copy_charge(delivered) as i64;
            }
            None => result.completions_ignored = input.completions.len(),
        }

        // Step 2: run.
        self.budget += BUDGET_PER_FRAME - delivered_charge;
        let start = self.budget;
        while self.budget > 0 {
            let before = self.budget;
            let stop = self.cpu.run_cached(&self.code, &mut Bus { rom: &self.rom, ram: &mut self.ram }, &mut self.budget);
            self.instructions += (before - self.budget).max(0) as u64;
            match stop {
                Stop::Budget => break,
                Stop::Fault(f) => {
                    self.stopped = Some(Stopped::Fault(f));
                    break;
                }
                Stop::Ecall => {
                    self.calls += 1;
                    let (units, wait) = self.ecall();
                    self.budget -= units as i64;
                    if wait {
                        result.waited = true;
                        break;
                    }
                    if self.stopped.is_some() {
                        break;
                    }
                }
            }
        }
        result.used = (start - self.budget + delivered_charge) as u64;
        // Unused budget never carries forward; a negative remainder does.
        self.budget = self.budget.min(0);

        // Step 3: the frame ends.
        self.system.end_frame();
        result.stopped = self.stopped;
        result.random_used = self.system.random_used();
        result
    }

    /// Carries out the call in a7 (SPEC 3); returns (units charged beyond
    /// the `ecall` instruction, whether the program waits for the next frame).
    fn ecall(&mut self) -> (u32, bool) {
        let x = &self.cpu.x;
        let (n, a0, a1, a2, a3) = (x[17], x[10], x[11], x[12], x[13]);
        let mut r1 = None;
        let mut units = charge::CALL;
        let mut wait = false;
        let ram = &mut self.ram[..];
        let rom = &self.rom[..];
        let r0 = match n {
            call::WAIT_FRAME => {
                // Returns the number of the frame it returns in.
                wait = true;
                self.system.frame().wrapping_add(1)
            }
            call::NEXT_EVENT => match ram_range(ram.len(), a0, 4) {
                None => ERR_BUFFER,
                Some(at) => match self.system.pop_event() {
                    None => 0,
                    Some(e) => {
                        ram[at..at + 4].copy_from_slice(&[e.kind, e.a, e.b, e.c]);
                        units += copy_charge(4);
                        1
                    }
                },
            },
            call::FRAME => self.system.frame(),
            call::TIME => self.system.time(),
            call::RANDOM => match ram_range(ram.len(), a0, a1 as usize).filter(|_| a1 <= MAX_RANDOM_PER_CALL) {
                None => ERR_BUFFER,
                Some(at) => {
                    self.system.random(&mut ram[at..at + a1 as usize]);
                    units += copy_charge(a1 as usize);
                    a1
                }
            },
            call::CONTROLLER => match ram_range(ram.len(), a1, 8) {
                None => ERR_BUFFER,
                Some(at) => {
                    let bytes = self.system.controller(a0);
                    ram[at..at + 8].copy_from_slice(&bytes);
                    units += copy_charge(8);
                    bytes[0] as u32
                }
            },
            call::EXIT => {
                self.stopped = Some(Stopped::Exit(a0 as u8));
                0
            }
            call::OPEN => match crate::machines::read_open_request(rom, ram, a0) {
                None => ERR_BUFFER,
                Some((request, read)) => {
                    units += copy_charge(read);
                    if !self.in_system || self.open_request.is_some() {
                        0
                    } else {
                        self.open_request = Some((request, false));
                        1
                    }
                }
            },
            call::ARGUMENT => match ram_range(ram.len(), a0, 32) {
                None => ERR_BUFFER,
                Some(at) => match &self.argument {
                    None => 0,
                    Some(id) => {
                        ram[at..at + 32].copy_from_slice(id);
                        units += copy_charge(32);
                        1
                    }
                },
            },
            0x200..=0x2FF if self.terminal.is_some() => {
                let t = self.terminal.as_mut().unwrap();
                match n {
                    call::TERM_WRITE => match readable(rom, ram, a0, a1 as usize) {
                        None => ERR_BUFFER,
                        Some(bytes) => {
                            // Stops after the byte that uses up the budget.
                            let left = self.budget - units as i64;
                            let (done, charged) = t.write_limited(bytes, left, charge::WRITE_BYTE, charge::SCROLL_CLEAR);
                            units += charged;
                            done as u32
                        }
                    },
                    call::TERM_COMMAND => {
                        if a0 < 0x100 {
                            units += t.command(a0 as u8) * charge::SCROLL_CLEAR;
                        }
                        0
                    }
                    call::TERM_SET_CURSOR => {
                        t.set_cursor(a0, a1);
                        0
                    }
                    call::TERM_GET_CURSOR => {
                        let (col, row) = t.cursor();
                        r1 = Some(row as u32);
                        col as u32
                    }
                    call::TERM_SET_COLOURS => {
                        // A value above 255 leaves that colour as it is.
                        let (fg, bg) = t.colors();
                        t.set_colours(u8::try_from(a0).unwrap_or(fg), u8::try_from(a1).unwrap_or(bg));
                        0
                    }
                    call::TERM_GET_COLOURS => {
                        let (fg, bg) = t.colors();
                        r1 = Some(bg as u32);
                        fg as u32
                    }
                    call::TERM_CELLS_READ => {
                        // A count over 2,400 is refused before it is multiplied.
                        let len = if a2 as usize <= CELLS { a2 as usize * CELL_BYTES } else { usize::MAX };
                        match ram_range(ram.len(), a1, len).filter(|_| (a0 as usize) < CELLS && a2 as usize <= CELLS) {
                            None => ERR_BUFFER,
                            Some(at) => {
                                t.cells_read(a0 as usize, &mut ram[at..at + len]);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_CELLS_WRITE => {
                        let len = if a2 as usize <= CELLS { a2 as usize * CELL_BYTES } else { usize::MAX };
                        match readable(rom, ram, a1, len).filter(|_| (a0 as usize) < CELLS && a2 as usize <= CELLS) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                t.cells_write(a0 as usize, bytes);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_PALETTE_SET => {
                        let len = if a2 <= 256 { a2 as usize * 3 } else { usize::MAX };
                        match readable(rom, ram, a1, len).filter(|_| a0 < 256 && a2 <= 256) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                t.palette_set(a0 as usize, bytes);
                                units += copy_charge(len);
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
            0x300..=0x3FF if self.vector.is_some() => {
                let v = self.vector.as_mut().unwrap();
                match n {
                    call::VEC_SUBMIT => {
                        // A list not wholly in RAM is rejected like a bad length.
                        match ram_range(ram.len(), a0, a1 as usize) {
                            Some(at) if v.submit(&ram[at..at + a1 as usize]) == crate::vector::RESULT_DONE => {
                                units += copy_charge(a1 as usize);
                                crate::vector::RESULT_DONE
                            }
                            _ => crate::vector::RESULT_REJECTED,
                        }
                    }
                    call::VEC_CLEAR => {
                        v.clear();
                        0
                    }
                    call::VEC_STYLE => v.set_style(a0),
                    call::VEC_PERSISTENCE => v.set_persistence(a0),
                    call::VEC_FILL => v.set_fill(a0),
                    _ => ERR_UNKNOWN,
                }
            }
            call::CONTENT_SUBMIT if self.content.is_some() => {
                let (handle, taken) = self.content.as_mut().unwrap().submit(a0, rom, ram);
                units += copy_charge(taken);
                handle
            }
            0x800..=0x8FF if self.machines.is_some() => {
                let (r, extra) = self.machines.as_mut().unwrap().call(n, [a0, a1, a2, a3, x[14]], rom, ram);
                units += extra;
                r
            }
            0x700..=0x7FF if self.page.is_some() => {
                let page = self.page.as_mut().unwrap();
                match n {
                    call::PAGE_SUBMIT => {
                        // The length is checked first; then the buffer.
                        if a1 == 0 || a1 as usize > crate::page::MAX_DOCUMENT {
                            crate::page::RESULT_REJECTED
                        } else {
                            match readable(rom, ram, a0, a1 as usize) {
                                None => ERR_BUFFER,
                                Some(bytes) => {
                                    units += copy_charge(bytes.len());
                                    page.submit(bytes)
                                }
                            }
                        }
                    }
                    call::PAGE_CLEAR => {
                        page.clear();
                        0
                    }
                    call::PAGE_REGION => {
                        page.set_region(a0, a1, a2, a3);
                        0
                    }
                    call::PAGE_SCROLL => {
                        page.set_scroll(a0);
                        0
                    }
                    call::PAGE_BACKGROUND => {
                        page.set_background(a0);
                        0
                    }
                    call::PAGE_LOCATE => page.locate(a0),
                    _ => ERR_UNKNOWN,
                }
            }
            _ => ERR_UNKNOWN,
        };
        self.cpu.x[10] = r0;
        if let Some(r1) = r1 {
            self.cpu.x[11] = r1;
        }
        (units, wait)
    }

    /// Why the machine stopped, once it has.
    pub fn stopped(&self) -> Option<Stopped> {
        self.stopped
    }

    /// The exit code, once the program has exited (not after a fault).
    pub fn exit_code(&self) -> Option<u8> {
        match self.stopped {
            Some(Stopped::Exit(code)) => Some(code),
            _ => None,
        }
    }

    /// The fault that stopped the machine, if any.
    pub fn fault(&self) -> Option<Fault> {
        match self.stopped {
            Some(Stopped::Fault(f)) => Some(f),
            _ => None,
        }
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

    /// Changes whenever `vector_list` changes.
    pub fn vector_generation(&self) -> u64 {
        self.vector.as_ref().map_or(0, Vector::generation)
    }

    /// Device 7, if the image lists it.
    pub fn page(&self) -> Option<&Page> {
        self.page.as_ref()
    }

    /// Device 8, if this machine is the shell and the image lists it.
    pub fn machines(&self) -> Option<&Machines> {
        self.machines.as_ref()
    }

    pub fn machines_mut(&mut self) -> Option<&mut Machines> {
        self.machines.as_mut()
    }

    pub fn content(&self) -> Option<&Content> {
        self.content.as_ref()
    }

    /// Content requests submitted since the last call.
    pub fn take_content_requests(&mut self) -> Vec<Request> {
        self.content.as_mut().map_or_else(Vec::new, Content::take_requests)
    }

    pub fn system(&self) -> &System {
        &self.system
    }

    /// The frame counter: frames run since reset.
    pub fn frame(&self) -> u32 {
        self.system.frame()
    }

    pub fn cpu(&self) -> &Cpu {
        &self.cpu
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram
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

    /// Budget units spent on instructions since reset (an instruction from
    /// RAM counts 4), call charges not included. Not machine state.
    pub fn instructions(&self) -> u64 {
        self.instructions
    }

    /// Device calls (`ecall`s) since reset. Not machine state.
    pub fn calls(&self) -> u64 {
        self.calls
    }

    /// The state serialization of STATE.md. Defined between frames.
    pub fn state(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 * 1024 + self.ram.len());
        out.extend_from_slice(STATE_DOMAIN);
        out.extend_from_slice(&self.image_id);
        // CPU.
        out.extend_from_slice(&self.cpu.pc.to_le_bytes());
        for r in &self.cpu.x[1..] {
            out.extend_from_slice(&r.to_le_bytes());
        }
        // Frame loop.
        out.extend_from_slice(&self.system.frame().to_le_bytes());
        let (kind, code, fault) = match self.stopped {
            None => (0u8, 0u8, None),
            Some(Stopped::Exit(c)) => (1, c, None),
            Some(Stopped::Fault(f)) => (2, 0, Some(f)),
        };
        out.extend_from_slice(&[kind, code]);
        let (fkind, detail, pc, addr) = match fault {
            None => (0u8, 0u32, 0u32, 0u32),
            Some(f) => {
                let d = if let FaultKind::Illegal(enc) = f.kind { enc } else { 0 };
                (fault_number(&f), d, f.pc, f.addr)
            }
        };
        out.push(fkind);
        for v in [detail, pc, addr] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.budget as i32).to_le_bytes());
        // RAM.
        out.extend_from_slice(&(self.ram.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.ram);
        // Devices.
        self.system.write_state(&mut out);
        out.push(self.terminal.is_some() as u8);
        if let Some(t) = &self.terminal {
            t.write_state(&mut out);
        }
        out.push(self.vector.is_some() as u8);
        if let Some(v) = &self.vector {
            v.write_state(&mut out);
        }
        out.push(self.content.is_some() as u8);
        if let Some(c) = &self.content {
            c.write_state(&mut out);
        }
        out.push(self.page.is_some() as u8);
        if let Some(p) = &self.page {
            p.write_state(&mut out);
        }
        // Only for an image that lists device 8 (STATE.md 1.10).
        if self.lists_machines {
            out.push(self.machines.is_some() as u8);
            if let Some(m) = &self.machines {
                m.write_state(&mut out);
            }
        }
        // Only for a machine given an argument (STATE.md 1.11).
        if let Some(id) = &self.argument {
            out.extend_from_slice(id);
        }
        out
    }

    /// The state hash: BLAKE3-256 of [`Machine::state`].
    pub fn state_hash(&self) -> [u8; 32] {
        *blake3::hash(&self.state()).as_bytes()
    }

    /// The machine whose state serialization is `state`, for `image`
    /// (STATE.md 4): `state()` of the result is `state`, byte for byte, and
    /// given the same inputs it runs as the machine it was taken of. A
    /// shell's state (section 1.10 present) is not restorable. Content
    /// requests that were pending complete with status 03h (failed) in its
    /// first frame (SPEC 9.2). The identity and whether it is in a system
    /// are the caller's to set.
    pub fn restore(image: &Image, state: &[u8]) -> Result<Machine, crate::snapshot::RestoreError> {
        use crate::snapshot::{RestoreError, Reader, bad};
        let mut r = Reader::new(state);
        if r.take(STATE_DOMAIN.len())? != STATE_DOMAIN {
            return Err(bad("not a terse-state-1 serialization"));
        }
        if r.array::<32>()? != image.id {
            return Err(RestoreError::WrongImage);
        }
        let mut m = Machine::new(image);
        m.cpu.pc = r.u32()?;
        for i in 1..32 {
            m.cpu.x[i] = r.u32()?;
        }
        m.cpu.min_sp = m.cpu.x[2];
        let frame = r.u32()?;
        let (stopped, code, kind, detail, pc, addr) = (r.u8()?, r.u8()?, r.u8()?, r.u32()?, r.u32()?, r.u32()?);
        m.stopped = match stopped {
            0 => None,
            1 => Some(Stopped::Exit(code)),
            2 => {
                let kind = match kind {
                    1 => FaultKind::Illegal(detail),
                    2 => FaultKind::Ebreak,
                    3 => FaultKind::Fetch,
                    4 => FaultKind::Load,
                    5 => FaultKind::Store,
                    6 => FaultKind::Misaligned,
                    _ => return Err(bad("fault kind")),
                };
                Some(Stopped::Fault(Fault { kind, pc, addr }))
            }
            _ => return Err(bad("stopped")),
        };
        let budget = r.u32()? as i32;
        if budget > 0 {
            return Err(bad("a positive budget carried"));
        }
        m.budget = budget as i64;
        if r.u32()? as usize != m.ram.len() {
            return Err(bad("RAM size"));
        }
        let n = m.ram.len();
        m.ram.copy_from_slice(r.take(n)?);
        m.system.restore(frame, &mut r)?;
        if r.flag()? != m.terminal.is_some() {
            return Err(bad("terminal present"));
        }
        if let Some(t) = &mut m.terminal {
            t.restore(&mut r)?;
        }
        if r.flag()? != m.vector.is_some() {
            return Err(bad("vector display present"));
        }
        if let Some(v) = &mut m.vector {
            v.restore(&mut r)?;
        }
        if r.flag()? != m.content.is_some() {
            return Err(bad("content present"));
        }
        if let Some(c) = &mut m.content {
            c.restore(&mut r, m.ram.len())?;
        }
        if r.flag()? != m.page.is_some() {
            return Err(bad("page present"));
        }
        if let Some(p) = &mut m.page {
            p.restore(&mut r)?;
        }
        if m.lists_machines && r.u8()? != 0 {
            return Err(bad("the state of a shell cannot be restored"));
        }
        m.argument = match r.remaining() {
            0 => None,
            32 => Some(r.array()?),
            _ => return Err(bad("length")),
        };
        // Every field as `state` writes it, so nothing was out of range.
        if m.state() != state {
            return Err(bad("not as this machine would write it"));
        }
        Ok(m)
    }
}
