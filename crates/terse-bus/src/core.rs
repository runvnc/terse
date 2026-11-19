//! The core host's machine: CPU, ROM and RAM, the budget and frame loop
//! (SPEC 1), the System device (SPEC 4, 5) and the event queue, and the
//! state serialization's frame of sections (STATE.md 1). Every other
//! device is behind the [`Devices`] trait: the device bus (`bus.rs`) or the
//! same devices compiled in (`inproc.rs`).

use terse_core::content::{ram_range, readable};
use terse_core::cpu::{Bus, Code, Stop};
use terse_core::machine::{call, charge, copy_charge, fault_number};
use terse_core::system::QUEUE_CAPACITY;
use terse_core::{BUDGET_PER_FRAME, CONTROLLERS, Controller, Cpu, ERR_BUFFER, ERR_UNKNOWN, Event, FaultKind, Image, RAM_BASE, ROM_BASE, STATE_DOMAIN, Stopped};

use crate::proto::{EVENT_PAIR, PosEvent, position};

/// The guest's memory as a device sees it during a call or at a frame's
/// start.
pub struct GuestMem<'a> {
    pub rom: &'a [u8],
    pub ram: &'a mut [u8],
}

/// What a call did (SPEC 3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallOutcome {
    pub a0: u32,
    pub a1: Option<u32>,
    /// Units beyond the `ecall` instruction and the call's 4.
    pub charge: u32,
}

/// What the devices give at the start of a frame.
#[derive(Default)]
pub struct FrameInputs {
    pub events: Vec<PosEvent>,
    /// Units subtracted from the frame's budget.
    pub charge: u32,
    /// Per slot, the input record of this frame (for recording).
    pub records: Vec<(u8, Vec<u8>)>,
}

/// The devices other than the System device. The machine's own semantics
/// (budget, charges' application, the queue) stay in [`Core`].
pub trait Devices {
    /// Step 1 of a frame: the devices' inputs (they have already written
    /// guest memory where they deliver data). `replay`: each slot's
    /// recorded input, to be used instead of the live sources.
    fn begin_frame(&mut self, frame: u32, time: u32, host: &[Event], replay: Option<&[(u8, Vec<u8>)]>, mem: GuestMem) -> Result<FrameInputs, String>;
    /// A call to `slot` (not 1, listed by the image). `budget` is what is
    /// left after the `ecall` instruction and the call's 4.
    fn call(&mut self, slot: u8, function: u8, args: [u32; 6], budget: i64, mem: GuestMem) -> Result<CallOutcome, String>;
    /// Step 3: the frame has ended; `frame` is the counter after it.
    fn end_frame(&mut self, frame: u32, stopped: bool, mem: GuestMem) -> Result<(), String>;
    /// The slot's section of the state serialization, without its present
    /// flag (STATE.md 1.6–1.9).
    fn state(&mut self, slot: u8) -> Result<Vec<u8>, String>;
}

/// The state sections the core writes for these slots, in this order, each
/// with a present flag (STATE.md 1.6–1.9). Slot 3's section is present
/// for an image that lists 3 or 9.
pub const STATE_SLOTS: [u8; 4] = [2, 3, 4, 7];

/// Per frame: what happened.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub used: u64,
    pub calls: u64,
    pub waited: bool,
    pub events_dropped: usize,
    pub random_used: usize,
}

/// The guest's memory: the core's own, or the shared memory file.
pub enum Memory {
    Owned { rom: Vec<u8>, ram: Vec<u8> },
    Shared { map: crate::transport::Mapping, rom_len: usize, ram_len: usize },
}

impl Memory {
    pub fn split(&mut self) -> (&[u8], &mut [u8]) {
        match self {
            Memory::Owned { rom, ram } => (rom, ram),
            Memory::Shared { map, rom_len, ram_len } => {
                let (rom_len, ram_len) = (*rom_len, *ram_len);
                let b = map.bytes_mut();
                let (rom, rest) = b.split_at_mut(crate::proto::MEMORY_RAM_OFFSET);
                (&rom[..rom_len], &mut rest[..ram_len])
            }
        }
    }

    pub fn ram(&self) -> &[u8] {
        match self {
            Memory::Owned { ram, .. } => ram,
            Memory::Shared { map, ram_len, .. } => &map.bytes()[crate::proto::MEMORY_RAM_OFFSET..][..*ram_len],
        }
    }
}

pub struct Core<D: Devices> {
    pub cpu: Cpu,
    code: Code,
    pub memory: Memory,
    pub system: terse_core::system::System,
    budget: i64,
    stopped: Option<Stopped>,
    pub image: Image,
    pub devices: D,
    /// Calls since reset.
    pub calls: u64,
}

impl<D: Devices> Core<D> {
    pub fn new(image: Image, memory: Memory, devices: D) -> Self {
        let ram_len = image.ram_pages as usize * 8192;
        Core {
            cpu: Cpu::new(ROM_BASE, RAM_BASE + ram_len as u32),
            code: Code::new(&image.rom),
            memory,
            system: Default::default(),
            budget: 0,
            stopped: None,
            image,
            devices,
            calls: 0,
        }
    }

    pub fn stopped(&self) -> Option<Stopped> {
        self.stopped
    }

    pub fn frame(&self) -> u32 {
        self.system.frame()
    }

    /// Runs one frame (SPEC 1.4) with the host's input.
    pub fn run_frame(
        &mut self,
        time: u32,
        host: &[Event],
        random: &[u8],
        controllers: &[Controller; CONTROLLERS],
        replay: Option<&[(u8, Vec<u8>)]>,
    ) -> Result<(FrameStats, Vec<(u8, Vec<u8>)>), String> {
        let mut stats = FrameStats::default();
        if self.stopped.is_some() {
            return Ok((stats, Vec::new()));
        }
        // Step 1: the devices' inputs.
        let frame = self.frame();
        let inputs = {
            let (rom, ram) = self.memory.split();
            self.devices.begin_frame(frame, time, host, replay, GuestMem { rom, ram })?
        };
        let FrameInputs { events, charge: delivered, records } = inputs;
        let mut input: Vec<(Event, u8)> = Vec::with_capacity(host.len() + events.len());
        input.extend(events.iter().filter(|e| e.position == position::BEFORE).map(|e| (e.event, e.flags)));
        for (i, &h) in host.iter().enumerate() {
            input.push((h, 0));
            input.extend(events.iter().filter(|e| e.position == position::AFTER_HOST && e.index as usize == i).map(|e| (e.event, e.flags)));
        }
        input.extend(events.iter().filter(|e| e.position == position::AFTER_INPUT).map(|e| (e.event, e.flags)));
        // SPEC 5: at most one feeds-updated event a frame; a pair is queued
        // whole or not at all.
        let mut feeds_updated = false;
        let mut skip_next = false;
        for (e, flags) in input {
            if std::mem::take(&mut skip_next) {
                stats.events_dropped += 1;
                continue;
            }
            if e.kind == Event::FEEDS_UPDATED && std::mem::replace(&mut feeds_updated, true) {
                stats.events_dropped += 1;
                continue;
            }
            if flags & EVENT_PAIR != 0 && self.system.queue().len() + 2 > QUEUE_CAPACITY {
                stats.events_dropped += 1;
                skip_next = true;
                continue;
            }
            if !self.system.push_event(e) {
                stats.events_dropped += 1;
            }
        }
        self.system.latch(time, random, controllers);
        for e in events.iter().filter(|e| e.position == position::COMPLETION) {
            if !self.system.push_event(e.event) {
                stats.events_dropped += 1;
            }
        }

        // Step 2.
        self.budget += BUDGET_PER_FRAME - delivered as i64;
        let start = self.budget;
        while self.budget > 0 {
            let stop = {
                let (rom, ram) = self.memory.split();
                self.cpu.run_cached(&self.code, &mut Bus { rom, ram }, &mut self.budget)
            };
            match stop {
                Stop::Budget => break,
                Stop::Fault(f) => {
                    self.stopped = Some(Stopped::Fault(f));
                    break;
                }
                Stop::Ecall => {
                    self.calls += 1;
                    stats.calls += 1;
                    let (units, wait) = self.ecall()?;
                    self.budget -= units as i64;
                    if wait {
                        stats.waited = true;
                        break;
                    }
                    if self.stopped.is_some() {
                        break;
                    }
                }
            }
        }
        stats.used = (start - self.budget + delivered as i64) as u64;
        self.budget = self.budget.min(0);

        // Step 3.
        self.system.end_frame();
        stats.random_used = self.system.random_used();
        let frame = self.frame();
        let stopped = self.stopped.is_some();
        let (rom, ram) = self.memory.split();
        self.devices.end_frame(frame, stopped, GuestMem { rom, ram })?;
        Ok((stats, records))
    }

    /// The call in a7 (SPEC 3): the System device here, every other slot
    /// the image lists to the devices. Returns (units beyond the `ecall`
    /// instruction, whether the program waits for the next frame).
    fn ecall(&mut self) -> Result<(u32, bool), String> {
        let x = self.cpu.x;
        let (n, a0, a1) = (x[17], x[10], x[11]);
        let mut units = charge::CALL;
        let mut wait = false;
        let mut r1 = None;
        let slot = n >> 8;
        let r0 = if slot == 1 {
            let (rom, ram) = self.memory.split();
            match n {
                call::WAIT_FRAME => {
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
                call::RANDOM => match ram_range(ram.len(), a0, a1 as usize).filter(|_| a1 <= 256) {
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
                // A machine that runs on its own: `open` passes nothing on
                // (SPEC 4), but a valid request is still read and charged.
                call::OPEN => match terse_core::machines::read_open_request(rom, ram, a0) {
                    None => ERR_BUFFER,
                    Some((_, read)) => {
                        units += copy_charge(read);
                        0
                    }
                },
                // It has no argument.
                call::ARGUMENT => match ram_range(ram.len(), a0, 32) {
                    None => ERR_BUFFER,
                    Some(_) => 0,
                },
                _ => ERR_UNKNOWN,
            }
        } else if slot < 256 && self.image.has_device(slot as u8) {
            let budget = self.budget - units as i64;
            let args = [x[10], x[11], x[12], x[13], x[14], x[15]];
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
    pub fn state(&mut self) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(64 * 1024 + self.memory.ram().len());
        out.extend_from_slice(STATE_DOMAIN);
        out.extend_from_slice(&self.image.id);
        out.extend_from_slice(&self.cpu.pc.to_le_bytes());
        for r in &self.cpu.x[1..] {
            out.extend_from_slice(&r.to_le_bytes());
        }
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
        let ram = self.memory.ram();
        out.extend_from_slice(&(ram.len() as u32).to_le_bytes());
        out.extend_from_slice(ram);
        self.system.write_state(&mut out);
        for slot in STATE_SLOTS {
            let present = self.image.has_device(slot) || (slot == 3 && self.image.has_device(9));
            out.push(present as u8);
            if present {
                out.extend_from_slice(&self.devices.state(slot)?);
            }
        }
        Ok(out)
    }

    pub fn state_hash(&mut self) -> Result<[u8; 32], String> {
        Ok(*blake3::hash(&self.state()?).as_bytes())
    }

    /// The bytes `[addr, addr + len)` of ROM or RAM.
    pub fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        let (rom, ram) = self.memory.split();
        readable(rom, ram, addr, len).map(<[u8]>::to_vec)
    }
}
