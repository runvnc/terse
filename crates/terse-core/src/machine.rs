//! The machine: the RV32IMC core, ROM and RAM, the frame loop and the
//! devices reached through `ecall` (SPEC 1, 3, 14).

use std::fmt;

use crate::content::{Completion, Content, Request, ram_range};
use crate::devices::{Devices, EVENT_PAIR, FrameInputs, GuestMem, InProcess, Memory, position};
use crate::cpu::{Bus, Code, Cpu, Fault, FaultKind, RAM_BASE, ROM_BASE, Stop};
use crate::image::Image;
use crate::machines::{Machines, ProgramId};
use crate::page::Page;
use crate::system::{CONTROLLERS, Controller, Event, System};
use crate::terminal::Terminal;
use crate::vector::Vector;
use crate::vector3::Vector3;

/// Leading domain string of the state serialization (STATE.md).
pub const STATE_DOMAIN: &[u8] = b"terse-state-1";

/// Budget units added each frame (SPEC 1.4). Provisional.
pub const BUDGET_PER_FRAME: i64 = 125_000;

/// The charges of SPEC 14, in budget units. Provisional.
pub mod charge {
    /// Every call, on top of the `ecall` instruction itself: the base
    /// charge of a deferred call, and of every System and Machines call.
    pub const CALL: u32 = 4;
    /// The base charge of an answered call (SPEC 14: [`super::call::answered`]).
    pub const ANSWERED: u32 = 64;
    /// Terminal `write`, per byte.
    pub const WRITE_BYTE: u32 = 2;
    /// The longest terminal `write`, in bytes.
    pub const MAX_WRITE: u32 = 4096;
    /// Raw copies between guest memory and a device: one unit per this many
    /// bytes, rounded up per call.
    pub const COPY_BYTES_PER_UNIT: usize = 4;
    /// A terminal command's clear, scroll up or scroll down (inside a
    /// `write` they are not charged).
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

    pub const V3_SUBMIT: u32 = 0x900;
    pub const V3_CLEAR: u32 = 0x901;
    pub const V3_STYLE: u32 = 0x902;
    pub const V3_PERSISTENCE: u32 = 0x903;

    /// The answered calls (SPEC 14): a device's calls whose result or
    /// charge depends on the device's state. Every other call of devices
    /// 2–7 and 9 is deferred; System and Machines calls are the machine's
    /// own, never carried to a device elsewhere, and charged as deferred.
    pub fn answered(n: u32) -> bool {
        matches!(n, TERM_GET_CURSOR | TERM_GET_COLOURS | TERM_CELLS_READ | CONTENT_SUBMIT | PAGE_LOCATE)
    }
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

/// Inputs for one frame (SPEC 1.4 step 1, 4, 6, 9.4).
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
    /// Device calls made in the frame.
    pub calls: u64,
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

/// A machine (SPEC 1): the CPU, ROM and RAM, the budget and the frame loop,
/// the System device and, for a shell, the Machines device. Every other
/// device is behind [`Devices`]: compiled in ([`InProcess`], the default)
/// or elsewhere (terse-bus's driver processes).
pub struct Machine<D: Devices = InProcess> {
    cpu: Cpu,
    /// The ROM, decoded once.
    code: Code,
    memory: Memory,
    system: System,
    devices: D,
    /// Which slots the image lists.
    listed: [bool; 256],
    /// Device 8, only for the shell (SPEC 12.1).
    machines: Option<Machines>,
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
    /// (STATE.md 1.12).
    argument: Option<[u8; 32]>,
}

impl Machine {
    /// The machine at reset (SPEC 1.3), with the devices compiled in.
    pub fn new(image: &Image) -> Self {
        let memory = Memory::Owned { rom: image.rom.clone(), ram: vec![0; image.ram_pages as usize * 8192] };
        Machine::with_devices(image, memory, InProcess::new(image))
    }

    /// The machine at reset as the shell (SPEC 12.1): device 8 is granted
    /// if the image lists it; `builtins` are the IDs `builtin` returns.
    pub fn new_shell(image: &Image, builtins: Vec<[u8; 32]>) -> Self {
        let mut m = Machine::new(image);
        if m.listed[crate::machines::SLOT as usize] {
            m.machines = Some(Machines::new(builtins));
        }
        m
    }

    /// Runs one frame (SPEC 1.4). Does nothing once the machine has stopped.
    pub fn run_frame(&mut self, input: &FrameInput) -> FrameResult {
        self.try_run_frame(input).expect("the devices compiled in do not fail")
    }

    /// The state serialization of STATE.md. Defined between frames.
    pub fn state(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 * 1024 + self.memory.ram().len());
        self.write_core_state(&mut out);
        let d = &self.devices;
        for (slot, present) in [(2, d.terminal.is_some()), (3, d.vector.is_some()), (4, d.content.is_some()), (7, d.page.is_some())] {
            out.push(present as u8);
            if present {
                match slot {
                    2 => d.terminal.as_ref().unwrap().write_state(&mut out),
                    3 => d.vector.as_ref().unwrap().write_state(&mut out),
                    4 => d.content.as_ref().unwrap().write_state(&mut out),
                    _ => d.page.as_ref().unwrap().write_state(&mut out),
                }
            }
        }
        self.write_machines_state(&mut out);
        if let Some(v) = &d.vector3 {
            v.write_state(&mut out);
        }
        self.write_argument(&mut out);
        out
    }

    /// The state hash: BLAKE3-256 of [`Machine::state`].
    pub fn state_hash(&self) -> [u8; 32] {
        *blake3::hash(&self.state()).as_bytes()
    }

    pub fn terminal(&self) -> Option<&Terminal> {
        self.devices.terminal()
    }

    pub fn vector(&self) -> Option<&Vector> {
        self.devices.vector()
    }

    /// Whether the image lists device 3 (a machine with only device 9 has
    /// a [`Vector`] for the shared style, but no 2D list).
    pub fn lists_vector(&self) -> bool {
        self.listed[3]
    }

    /// Device 9, if the image lists it.
    pub fn vector3(&self) -> Option<&Vector3> {
        self.devices.vector3()
    }

    /// The 3D display, to change its list from outside a frame (hosts'
    /// tests).
    pub fn vector3_mut(&mut self) -> Option<&mut Vector3> {
        self.devices.vector3_mut()
    }

    pub fn vector_list(&self) -> &[u8] {
        self.devices.vector().map_or(&[], Vector::list)
    }

    /// Changes whenever `vector_list` changes.
    pub fn vector_generation(&self) -> u64 {
        self.devices.vector().map_or(0, Vector::generation)
    }

    /// Device 7, if the image lists it.
    pub fn page(&self) -> Option<&Page> {
        self.devices.page()
    }

    pub fn content(&self) -> Option<&Content> {
        self.devices.content()
    }

    /// Content requests submitted since the last call.
    pub fn take_content_requests(&mut self) -> Vec<Request> {
        self.devices.take_content_requests()
    }
}

impl<D: Devices> Machine<D> {
    /// The machine at reset (SPEC 1.3) with these devices and memory
    /// (`memory` holds the image's ROM and a zeroed RAM of its size).
    pub fn with_devices(image: &Image, memory: Memory, devices: D) -> Self {
        let ram_end = RAM_BASE + image.ram_pages as u32 * 8192;
        let mut listed = [false; 256];
        for d in &image.devices {
            listed[d.slot as usize] = true;
        }
        listed[1] = true;
        Machine {
            cpu: Cpu::new(ROM_BASE, ram_end),
            code: Code::new(&image.rom),
            memory,
            system: System::default(),
            devices,
            listed,
            machines: None,
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

    /// The devices.
    pub fn devices(&self) -> &D {
        &self.devices
    }

    pub fn devices_mut(&mut self) -> &mut D {
        &mut self.devices
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

    /// Runs one frame (SPEC 1.4). Does nothing once the machine has
    /// stopped. An error is the devices' (a driver that failed).
    pub fn try_run_frame(&mut self, input: &FrameInput) -> Result<FrameResult, String> {
        let mut result = FrameResult {
            stopped: self.stopped,
            used: 0,
            waited: false,
            events_dropped: 0,
            random_used: 0,
            completions_ignored: 0,
            calls: 0,
        };
        if self.stopped.is_some() {
            self.devices.idle_frame();
            return Ok(result);
        }

        // Step 1: inputs. The devices' events go among the host's as their
        // positions say (DEVICES.md 1.4).
        let inputs = {
            let (rom, ram) = self.memory.split();
            self.devices.begin_frame(self.system.frame(), input, GuestMem { rom, ram })?
        };
        let FrameInputs { events, charge: delivered, completions_ignored } = inputs;
        result.completions_ignored = completions_ignored;
        // (event, flags, from the host)
        let mut merged: Vec<(Event, u8, bool)> = Vec::with_capacity(input.events.len() + events.len());
        let placed = |p: u8| events.iter().filter(move |e| e.position == p);
        merged.extend(placed(position::BEFORE).map(|e| (e.event, e.flags, false)));
        for (i, &h) in input.events.iter().enumerate() {
            merged.push((h, 0, true));
            merged.extend(placed(position::AFTER_HOST).filter(|e| e.index as usize == i).map(|e| (e.event, e.flags, false)));
        }
        merged.extend(placed(position::AFTER_INPUT).map(|e| (e.event, e.flags, false)));
        // SPEC 5: at most one feeds-updated event a frame. SPEC 11.5: the
        // two events of a located anchor are queued together or not at
        // all (a host's pair by kind, a device's by its flag).
        let mut feeds_updated = false;
        let mut skip_y = false;
        let mut skip_next = false;
        for (e, flags, host) in merged {
            if std::mem::take(&mut skip_next) {
                result.events_dropped += 1;
                continue;
            }
            if e.kind == Event::FEEDS_UPDATED && std::mem::replace(&mut feeds_updated, true) {
                result.events_dropped += 1;
                continue;
            }
            if host && e.kind == Event::PAGE_LOCATED_Y && std::mem::take(&mut skip_y) {
                result.events_dropped += 1;
                continue;
            }
            let pair = if host { e.kind == Event::PAGE_LOCATED } else { flags & EVENT_PAIR != 0 };
            if pair && self.system.queue().len() + 2 > crate::system::QUEUE_CAPACITY {
                result.events_dropped += 1;
                if host {
                    skip_y = true;
                } else {
                    skip_next = true;
                }
                continue;
            }
            if host {
                skip_y = false;
            }
            if !self.system.push_event(e) {
                result.events_dropped += 1;
            }
        }
        self.system.latch(input.unix_time, input.random, &input.controllers);
        // The content-done events of the completions, after the input.
        for e in placed(position::COMPLETION) {
            if !self.system.push_event(e.event) {
                result.events_dropped += 1;
            }
        }

        // Step 2: run.
        let delivered_charge = delivered as i64;
        self.budget += BUDGET_PER_FRAME - delivered_charge;
        let start = self.budget;
        while self.budget > 0 {
            let before = self.budget;
            let stop = {
                let (rom, ram) = self.memory.split();
                self.cpu.run_cached(&self.code, &mut Bus { rom, ram }, &mut self.budget)
            };
            self.instructions += (before - self.budget).max(0) as u64;
            match stop {
                Stop::Budget => break,
                Stop::Fault(f) => {
                    self.stopped = Some(Stopped::Fault(f));
                    break;
                }
                Stop::Ecall => {
                    self.calls += 1;
                    result.calls += 1;
                    let (units, wait) = self.ecall()?;
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
        let (frame, stopped) = (self.system.frame(), self.stopped.is_some());
        let (rom, ram) = self.memory.split();
        self.devices.end_frame(frame, stopped, GuestMem { rom, ram })?;
        Ok(result)
    }

    /// Carries out the call in a7 (SPEC 3); returns (units charged beyond
    /// the `ecall` instruction, whether the program waits for the next
    /// frame). System and Machines calls are the machine's own; the rest go
    /// to the devices.
    fn ecall(&mut self) -> Result<(u32, bool), String> {
        let x = self.cpu.x;
        let (n, a0, a1) = (x[17], x[10], x[11]);
        let args = [x[10], x[11], x[12], x[13], x[14], x[15]];
        let mut r1 = None;
        let mut units = charge::CALL;
        let mut wait = false;
        let slot = n >> 8;
        let r0 = if slot == 1 {
            let (rom, ram) = self.memory.split();
            match n {
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
                _ => ERR_UNKNOWN,
            }
        } else if slot == crate::machines::SLOT as u32 {
            match &mut self.machines {
                Some(machines) => {
                    let (rom, ram) = self.memory.split();
                    let ctx = crate::machines::CallContext { frame: self.system.frame(), list: self.devices.list3() };
                    let (r, extra) = machines.call(n, args, rom, ram, ctx);
                    units += extra;
                    r
                }
                None => ERR_UNKNOWN,
            }
        } else if slot < 256 && self.listed[slot as usize] {
            let budget = self.budget - units as i64;
            let (rom, ram) = self.memory.split();
            let out = self.devices.call(slot as u8, n as u8, args, budget, GuestMem { rom, ram })?;
            units += out.charge;
            r1 = out.a1;
            out.a0
        } else {
            ERR_UNKNOWN
        };
        self.cpu.x[10] = r0;
        if let Some(r1) = r1 {
            self.cpu.x[11] = r1;
        }
        Ok((units, wait))
    }

    /// The state serialization (STATE.md 1), the devices' sections asked
    /// of them.
    pub fn try_state(&mut self) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(64 * 1024 + self.memory.ram().len());
        self.write_core_state(&mut out);
        for slot in [2u8, 3, 4, 7] {
            let present = self.listed[slot as usize] || (slot == 3 && self.listed[crate::vector3::SLOT as usize]);
            out.push(present as u8);
            if present {
                self.devices.write_state(slot, &mut out)?;
            }
        }
        self.write_machines_state(&mut out);
        if self.listed[crate::vector3::SLOT as usize] {
            self.devices.write_state(crate::vector3::SLOT, &mut out)?;
        }
        self.write_argument(&mut out);
        Ok(out)
    }

    pub fn try_state_hash(&mut self) -> Result<[u8; 32], String> {
        Ok(*blake3::hash(&self.try_state()?).as_bytes())
    }

    /// STATE.md 1.1–1.5.
    fn write_core_state(&self, out: &mut Vec<u8>) {
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
        let ram = self.memory.ram();
        out.extend_from_slice(&(ram.len() as u32).to_le_bytes());
        out.extend_from_slice(ram);
        self.system.write_state(out);
    }

    /// STATE.md 1.10: only for an image that lists device 8.
    fn write_machines_state(&self, out: &mut Vec<u8>) {
        if self.listed[crate::machines::SLOT as usize] {
            out.push(self.machines.is_some() as u8);
            if let Some(m) = &self.machines {
                m.write_state(out);
            }
        }
    }

    /// STATE.md 1.12: only for a machine given an argument.
    fn write_argument(&self, out: &mut Vec<u8>) {
        if let Some(id) = &self.argument {
            out.extend_from_slice(id);
        }
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

    /// Device 8, if this machine is the shell and the image lists it.
    pub fn machines(&self) -> Option<&Machines> {
        self.machines.as_ref()
    }

    pub fn machines_mut(&mut self) -> Option<&mut Machines> {
        self.machines.as_mut()
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
        self.memory.ram()
    }

    pub fn rom(&self) -> &[u8] {
        self.memory.rom()
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

}

impl Machine {
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
        let ram_len = m.memory.ram().len();
        if r.u32()? as usize != ram_len {
            return Err(bad("RAM size"));
        }
        m.memory.split().1.copy_from_slice(r.take(ram_len)?);
        m.system.restore(frame, &mut r)?;
        if r.flag()? != m.devices.terminal.is_some() {
            return Err(bad("terminal present"));
        }
        if let Some(t) = &mut m.devices.terminal {
            t.restore(&mut r)?;
        }
        if r.flag()? != m.devices.vector.is_some() {
            return Err(bad("vector display present"));
        }
        if let Some(v) = &mut m.devices.vector {
            v.restore(&mut r)?;
            // Without device 3 the fill mode and the 2D list cannot be set.
            if !m.listed[3] && (!v.fill() || !v.list().is_empty()) {
                return Err(bad("a fill mode or 2D list without device 3"));
            }
        }
        if r.flag()? != m.devices.content.is_some() {
            return Err(bad("content present"));
        }
        if let Some(c) = &mut m.devices.content {
            c.restore(&mut r, ram_len)?;
        }
        if r.flag()? != m.devices.page.is_some() {
            return Err(bad("page present"));
        }
        if let Some(p) = &mut m.devices.page {
            p.restore(&mut r)?;
        }
        if m.listed[crate::machines::SLOT as usize] && r.u8()? != 0 {
            return Err(bad("the state of a shell cannot be restored"));
        }
        if let Some(v) = &mut m.devices.vector3 {
            v.restore(&mut r)?;
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
