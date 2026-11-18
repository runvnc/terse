//! Device 4, content, for profile 2 (RV-PROFILE.md section 5.4).
//!
//! The operations, handles, statuses, completion rules and record formats
//! are those of SPEC 7; only the request block differs: 32-bit fields at
//! aligned offsets, and ordinary guest addresses instead of linear RAM
//! addresses. The host side is unchanged: requests and completions are
//! `terse_core`'s [`Request`] and [`Completion`].

use terse_core::content::{
    MAX_BLOB, MAX_GET_LENGTH, MAX_PENDING, OP_GET, OP_PUT, OP_SUBSCRIPTIONS, OP_TIMELINE, records_fit, status,
};
use terse_core::system::{Event, System};
use terse_core::{Completion, Outcome, Request, RequestKind};

use crate::cpu::{RAM_BASE, ROM_BASE};

/// Size of a profile-2 request block.
pub const BLOCK_LEN: usize = 52;
// Field offsets.
pub const OP: usize = 0;
pub const STATUS: usize = 1;
pub const ID: usize = 4;
pub const OFFSET: usize = 36;
pub const LENGTH: usize = 40;
pub const ADDR: usize = 44;
pub const SIZE: usize = 48;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    /// `addr` is an offset into RAM.
    Get { offset: u32, length: u16, addr: usize },
    Put { id: [u8; 32] },
    Records { op: u8, offset: u32, length: u16, addr: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    /// Offset of the request block in RAM, fixed at submit.
    block: usize,
    pending: Pending,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RvContent {
    slots: [Option<Slot>; MAX_PENDING],
    submitted: Vec<Request>,
}

/// The RAM offset of `[addr, addr + len)` if it lies wholly in RAM.
pub fn ram_range(ram_len: usize, addr: u32, len: usize) -> Option<usize> {
    let off = addr.wrapping_sub(RAM_BASE) as usize;
    (off <= ram_len && ram_len - off >= len).then_some(off)
}

/// The bytes `[addr, addr + len)` if they lie wholly in ROM or wholly in RAM.
pub fn readable<'a>(rom: &'a [u8], ram: &'a [u8], addr: u32, len: usize) -> Option<&'a [u8]> {
    if let Some(off) = ram_range(ram.len(), addr, len) {
        return Some(&ram[off..off + len]);
    }
    let off = addr.wrapping_sub(ROM_BASE) as usize;
    (off <= rom.len() && rom.len() - off >= len).then(|| &rom[off..off + len])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

impl RvContent {
    /// Submits the block at guest address `block_addr`. Returns the handle
    /// (1-8, or 0 if rejected) and the charge in instructions (a put: one
    /// per 4 bytes copied).
    pub fn submit(&mut self, block_addr: u32, rom: &[u8], ram: &mut [u8]) -> (u32, u32) {
        // Step 1: the block lies wholly in RAM, else nothing is written.
        let Some(block) = ram_range(ram.len(), block_addr, BLOCK_LEN) else { return (0, 0) };
        let b: [u8; BLOCK_LEN] = ram[block..block + BLOCK_LEN].try_into().unwrap();
        let (offset, length, addr, size) = (u32_at(&b, OFFSET), u32_at(&b, LENGTH), u32_at(&b, ADDR), u32_at(&b, SIZE));
        let in_ram = |len: usize| ram_range(ram.len(), addr, len);

        // Step 2.
        let request = match b[OP] {
            OP_GET | OP_SUBSCRIPTIONS | OP_TIMELINE => (1..=MAX_GET_LENGTH as u32)
                .contains(&length)
                .then(|| in_ram(length as usize))
                .flatten()
                .map(|at| {
                    let length = length as u16;
                    match b[OP] {
                        OP_GET => {
                            let id = b[ID..ID + 32].try_into().unwrap();
                            (Pending::Get { offset, length, addr: at }, RequestKind::Get { id, offset, length })
                        }
                        op => {
                            let kind = if op == OP_SUBSCRIPTIONS {
                                RequestKind::Subscriptions { offset, length }
                            } else {
                                RequestKind::Timeline { offset, length }
                            };
                            (Pending::Records { op, offset, length, addr: at }, kind)
                        }
                    }
                }),
            OP_PUT => (1..=MAX_BLOB as u32)
                .contains(&size)
                .then(|| readable(rom, ram, addr, size as usize))
                .flatten()
                .map(|data| {
                    let data = data.to_vec();
                    (Pending::Put { id: blake3_id(&data) }, RequestKind::Put { data })
                }),
            _ => None,
        };
        let Some((pending, kind)) = request else {
            ram[block + STATUS] = status::BAD_REQUEST;
            return (0, 0);
        };

        // Step 3.
        let Some(index) = self.slots.iter().position(Option::is_none) else { return (0, 0) };

        // Step 4.
        ram[block + STATUS] = status::PENDING;
        let handle = index as u8 + 1;
        self.slots[index] = Some(Slot { block, pending });
        let charge = match &kind {
            RequestKind::Put { data } => crate::machine::transfer_charge(data.len()),
            _ => 0,
        };
        self.submitted.push(Request { handle, kind });
        (handle as u32, charge)
    }

    /// Applies completions in increasing handle order, exactly as
    /// `terse_core::Content::complete`. Returns (ignored, events dropped).
    pub fn complete(&mut self, completions: &[Completion], ram: &mut [u8], system: &mut System) -> (usize, usize) {
        let mut ordered: Vec<&Completion> = completions.iter().collect();
        ordered.sort_by_key(|c| c.handle);
        let (mut ignored, mut dropped) = (0, 0);
        for c in ordered {
            match self.apply(c, ram) {
                Some(status) => {
                    if !system.push_event(Event::content_done(c.handle, status)) {
                        dropped += 1;
                    }
                }
                None => ignored += 1,
            }
        }
        (ignored, dropped)
    }

    fn apply(&mut self, c: &Completion, ram: &mut [u8]) -> Option<u8> {
        let index = (c.handle as usize).checked_sub(1).filter(|&i| i < MAX_PENDING)?;
        let slot = self.slots[index].as_ref()?;
        let block = slot.block;
        let put32 = |ram: &mut [u8], at: usize, v: u32| ram[block + at..block + at + 4].copy_from_slice(&v.to_le_bytes());
        let status = match (&slot.pending, &c.outcome) {
            (&Pending::Get { offset, length, addr }, Outcome::Got { size, data }) => {
                let (size, offset) = (*size as usize, offset as usize);
                let expected = (length as usize).min(size.saturating_sub(offset));
                if !(1..=MAX_BLOB).contains(&size) || data.len() != expected {
                    return None;
                }
                ram[addr..addr + data.len()].copy_from_slice(data);
                put32(ram, LENGTH, data.len() as u32);
                put32(ram, SIZE, size as u32);
                status::DONE
            }
            (&Pending::Records { op, offset, length, addr }, Outcome::Records { total, data }) => {
                if !records_fit(op, offset, length, *total, data) {
                    return None;
                }
                ram[addr..addr + data.len()].copy_from_slice(data);
                put32(ram, LENGTH, data.len() as u32);
                put32(ram, SIZE, *total);
                status::DONE
            }
            (Pending::Put { id: want }, Outcome::Put { id }) => {
                if id != want {
                    return None;
                }
                ram[block + ID..block + ID + 32].copy_from_slice(id);
                status::DONE
            }
            (_, Outcome::NotFound) => status::NOT_FOUND,
            (_, Outcome::Failed) => status::FAILED,
            _ => return None,
        };
        ram[block + STATUS] = status;
        self.slots[index] = None;
        Some(status)
    }

    /// Requests accepted since the last call, in submit order.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.submitted)
    }

    /// Handles of the pending requests, in increasing order.
    pub fn pending_handles(&self) -> Vec<u8> {
        (1..=MAX_PENDING as u8).filter(|&h| self.slots[h as usize - 1].is_some()).collect()
    }
}

fn blake3_id(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}
