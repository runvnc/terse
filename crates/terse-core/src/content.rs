//! Device 4, Content (SPEC 7).
//!
//! The core only validates requests, copies bytes and writes request blocks.
//! The host reads submitted requests with [`Content::take_requests`] and
//! passes completions back through `FrameInput`.

use crate::memory::Memory;
use crate::system::{Event, System};

/// Most requests pending at once; handles are 1 to 8 (SPEC 7.3).
pub const MAX_PENDING: usize = 8;
/// Size of a request block (SPEC 7.2).
pub const BLOCK_LEN: usize = 45;
/// Largest blob (SPEC 7).
pub const MAX_BLOB: usize = 1_048_576;
/// Largest `length` of a get (SPEC 7.2).
pub const MAX_GET_LENGTH: usize = 16_384;

pub const OP_GET: u8 = 0x01;
pub const OP_PUT: u8 = 0x02;
pub const OP_SUBSCRIPTIONS: u8 = 0x03;
pub const OP_TIMELINE: u8 = 0x04;

/// A subscriptions record: feed key and highest `seq` (SPEC 7.7).
pub const SUBSCRIPTION_RECORD: usize = 36;
/// Bytes of an entry before its title (NETWORK 4.2).
pub const ENTRY_HEAD: usize = 109;
/// Longest entry title (NETWORK 4.2).
pub const MAX_TITLE: usize = 120;
/// Longest timeline record: `L`, flags and an entry with a 120-byte title,
/// without its signature (SPEC 7.7).
pub const MAX_TIMELINE_RECORD: usize = 2 + ENTRY_HEAD + MAX_TITLE;
/// Timeline record flag: the article blob is in the host's store.
pub const FLAG_LOCAL: u8 = 0x01;
/// Largest record count `size` can hold.
const MAX_RECORDS: u32 = 0xFF_FFFF;

/// Status values (SPEC 7.4).
pub mod status {
    pub const PENDING: u8 = 0x00;
    pub const DONE: u8 = 0x01;
    pub const NOT_FOUND: u8 = 0x02;
    pub const FAILED: u8 = 0x03;
    pub const BAD_REQUEST: u8 = 0x04;
}

// Request block field offsets (SPEC 7.2).
const STATUS: usize = 1;
const ID: usize = 2;
const OFFSET: usize = 34;
const LENGTH: usize = 37;
const ADDR: usize = 39;
const SIZE: usize = 42;

/// A request accepted by a submit, for the host to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub handle: u8,
    pub kind: RequestKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestKind {
    /// Up to `length` bytes of blob `id` starting at `offset`.
    Get { id: [u8; 32], offset: u32, length: u16 },
    /// The bytes to publish, copied at submit time.
    Put { data: Vec<u8> },
    /// Subscription records from index `offset`, at most `length` bytes (SPEC 7.7).
    Subscriptions { offset: u32, length: u16 },
    /// Timeline records from index `offset`, at most `length` bytes (SPEC 7.7).
    Timeline { offset: u32, length: u16 },
}

/// The host's answer to a pending request (SPEC 7.3), an input to the machine
/// (SPEC 7.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub handle: u8,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A get succeeded: the blob's total size and the bytes delivered from the
    /// requested offset, which must be exactly `min(length, size - offset)`
    /// bytes (none if the offset is at or beyond the end).
    Got { size: u32, data: Vec<u8> },
    /// A put succeeded; `id` must be the BLAKE3-256 hash of the submitted bytes.
    Put { id: [u8; 32] },
    /// A subscriptions or timeline request succeeded: the number of records
    /// that exist and the whole records delivered from the requested index,
    /// as many as fit in `length` (SPEC 7.7).
    Records { total: u32, data: Vec<u8> },
    NotFound,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Get { id: [u8; 32], offset: u32, length: u16, addr: usize },
    Put { id: [u8; 32], size: u32 },
    Records { op: u8, offset: u32, length: u16, addr: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    /// Linear RAM address of the request block, fixed at submit.
    block: usize,
    pending: Pending,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Content {
    block_addr: u16,
    last_submit: u8,
    slots: [Option<Slot>; MAX_PENDING],
    submitted: Vec<Request>,
}

fn u24(b: &[u8]) -> usize {
    b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16
}

/// Whether `data` can be the answer to a subscriptions or timeline request
/// (SPEC 7.7): whole, well-formed records, no more than fit in `length` or
/// exist after `offset`, and as many as fit. Subscription records have a
/// fixed size, so their count is exact. A timeline record's size is only
/// known from its bytes: the records must tile `data` exactly, each `L`
/// must match its entry's title length, and space left over must be too
/// small for any record when records remain.
fn records_fit(op: u8, offset: u32, length: u16, total: u32, data: &[u8]) -> bool {
    let (length, remaining) = (length as usize, total.saturating_sub(offset) as usize);
    if total > MAX_RECORDS || data.len() > length {
        return false;
    }
    if op == OP_SUBSCRIPTIONS {
        return data.len() == remaining.min(length / SUBSCRIPTION_RECORD) * SUBSCRIPTION_RECORD;
    }
    let (mut at, mut count) = (0, 0);
    while at < data.len() {
        let l = data[at] as usize;
        // L counts the flags byte and the entry; byte 108 of the entry is `n`.
        let Some(&n) = data.get(at + 2 + ENTRY_HEAD - 1) else { return false };
        if n as usize > MAX_TITLE || l != 1 + ENTRY_HEAD + n as usize || data[at + 1] & !FLAG_LOCAL != 0 {
            return false;
        }
        at += 1 + l;
        count += 1;
    }
    at == data.len() && count <= remaining && (count == remaining || length - data.len() < MAX_TIMELINE_RECORD)
}

impl Content {
    /// Returns the charge in T-states.
    pub fn port_out(&mut self, reg: u8, value: u8, memory: &mut Memory) -> u32 {
        match reg {
            0x0 => self.block_addr = (self.block_addr & 0xFF00) | value as u16,
            0x1 => self.block_addr = (self.block_addr & 0x00FF) | (value as u16) << 8,
            0x2 => return self.submit(memory),
            _ => {}
        }
        0
    }

    pub fn port_in(&self, reg: u8) -> u8 {
        match reg {
            0x2 => self.last_submit,
            _ => 0xFF,
        }
    }

    /// SPEC 7.3 steps 1-4; returns the charge (SPEC 7.5).
    fn submit(&mut self, memory: &mut Memory) -> u32 {
        self.last_submit = 0;
        let ram_len = memory.ram().len();
        // Step 1: the block is the 45 bytes of linear RAM from the translated address.
        let Some(block) = memory.linear(self.block_addr).filter(|&b| b + BLOCK_LEN <= ram_len) else {
            return 0;
        };
        let ram = memory.ram_mut();
        let b: [u8; BLOCK_LEN] = ram[block..block + BLOCK_LEN].try_into().unwrap();
        let addr = u24(&b[ADDR..]);
        let fits = |len: usize| len >= 1 && addr + len <= ram_len;

        // Step 2.
        let request = match b[0] {
            OP_GET => {
                let length = u16::from_le_bytes([b[LENGTH], b[LENGTH + 1]]);
                (length as usize <= MAX_GET_LENGTH && fits(length as usize)).then(|| {
                    let id = b[ID..ID + 32].try_into().unwrap();
                    let offset = u24(&b[OFFSET..]) as u32;
                    (Pending::Get { id, offset, length, addr }, RequestKind::Get { id, offset, length })
                })
            }
            OP_SUBSCRIPTIONS | OP_TIMELINE => {
                let length = u16::from_le_bytes([b[LENGTH], b[LENGTH + 1]]);
                (length as usize <= MAX_GET_LENGTH && fits(length as usize)).then(|| {
                    let offset = u24(&b[OFFSET..]) as u32;
                    let kind = if b[0] == OP_SUBSCRIPTIONS {
                        RequestKind::Subscriptions { offset, length }
                    } else {
                        RequestKind::Timeline { offset, length }
                    };
                    (Pending::Records { op: b[0], offset, length, addr }, kind)
                })
            }
            OP_PUT => {
                let size = u24(&b[SIZE..]);
                (size <= MAX_BLOB && fits(size)).then(|| {
                    let data = ram[addr..addr + size].to_vec();
                    let id = *blake3::hash(&data).as_bytes();
                    (Pending::Put { id, size: size as u32 }, RequestKind::Put { data })
                })
            }
            _ => None,
        };
        let Some((pending, kind)) = request else {
            ram[block + STATUS] = status::BAD_REQUEST;
            return 0;
        };

        // Step 3.
        let Some(index) = self.slots.iter().position(Option::is_none) else { return 0 };

        // Step 4.
        ram[block + STATUS] = status::PENDING;
        let handle = index as u8 + 1;
        self.slots[index] = Some(Slot { block, pending });
        self.last_submit = handle;
        let charge = match &kind {
            RequestKind::Put { data } => data.len() as u32,
            _ => 0,
        };
        self.submitted.push(Request { handle, kind });
        charge
    }

    /// Applies completions in increasing handle order (SPEC 7.3). A completion
    /// that does not fit a pending request (unknown or already completed
    /// handle, wrong kind, wrong byte count, size out of range, wrong put ID)
    /// is ignored and the request stays pending. Returns (ignored, events
    /// dropped because the queue was full).
    pub fn complete(&mut self, completions: &[Completion], memory: &mut Memory, system: &mut System) -> (usize, usize) {
        let mut ordered: Vec<&Completion> = completions.iter().collect();
        ordered.sort_by_key(|c| c.handle);
        let (mut ignored, mut dropped) = (0, 0);
        for c in ordered {
            match self.apply(c, memory.ram_mut()) {
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

    /// Completes one request; returns its final status, or `None` if ignored.
    fn apply(&mut self, c: &Completion, ram: &mut [u8]) -> Option<u8> {
        let index = (c.handle as usize).checked_sub(1).filter(|&i| i < MAX_PENDING)?;
        let slot = self.slots[index].as_ref()?;
        let block = slot.block;
        let status = match (&slot.pending, &c.outcome) {
            (&Pending::Get { offset, length, addr, .. }, Outcome::Got { size, data }) => {
                let (size, offset) = (*size as usize, offset as usize);
                let expected = (length as usize).min(size.saturating_sub(offset));
                if !(1..=MAX_BLOB).contains(&size) || data.len() != expected {
                    return None;
                }
                ram[addr..addr + data.len()].copy_from_slice(data);
                ram[block + LENGTH..block + LENGTH + 2].copy_from_slice(&(data.len() as u16).to_le_bytes());
                ram[block + SIZE..block + SIZE + 3].copy_from_slice(&(size as u32).to_le_bytes()[..3]);
                status::DONE
            }
            (&Pending::Records { op, offset, length, addr }, Outcome::Records { total, data }) => {
                if !records_fit(op, offset, length, *total, data) {
                    return None;
                }
                ram[addr..addr + data.len()].copy_from_slice(data);
                ram[block + LENGTH..block + LENGTH + 2].copy_from_slice(&(data.len() as u16).to_le_bytes());
                ram[block + SIZE..block + SIZE + 3].copy_from_slice(&total.to_le_bytes()[..3]);
                status::DONE
            }
            (Pending::Put { id: want, .. }, Outcome::Put { id }) => {
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

    /// Block address, last result and the eight request slots (STATE.md,
    /// content section). Requests not yet taken by the host are not
    /// included: they are also in the slots.
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.block_addr.to_le_bytes());
        out.push(self.last_submit);
        for slot in &self.slots {
            // op, block, id, offset, length, addr, size: 51 bytes.
            let (op, block, id, offset, length, addr, size) = match slot {
                None => (0, 0, [0; 32], 0, 0, 0, 0),
                Some(Slot { block, pending }) => match *pending {
                    Pending::Get { id, offset, length, addr } => (OP_GET, *block, id, offset, length, addr, 0),
                    Pending::Put { id, size } => (OP_PUT, *block, id, 0, 0, 0, size),
                    Pending::Records { op, offset, length, addr } => (op, *block, [0; 32], offset, length, addr, 0),
                },
            };
            out.push(op);
            out.extend_from_slice(&(block as u32).to_le_bytes());
            out.extend_from_slice(&id);
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(&(addr as u32).to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: u16 = 0x8000; // linear 0000h
    const DATA: u16 = 0x9000; // linear 1000h
    const RAM_LEN: usize = 4 * 0x2000;

    struct Rig {
        content: Content,
        memory: Memory,
        system: System,
    }

    impl Rig {
        fn new() -> Self {
            Rig { content: Content::default(), memory: Memory::new(&[0], 4), system: System::default() }
        }

        fn write(&mut self, addr: u16, bytes: &[u8]) {
            for (i, &b) in bytes.iter().enumerate() {
                self.memory.write(addr.wrapping_add(i as u16), b);
            }
        }

        fn read(&self, addr: u16, len: usize) -> Vec<u8> {
            (0..len).map(|i| self.memory.read(addr.wrapping_add(i as u16))).collect()
        }

        /// Writes `block` at `addr` (if any) and submits it; returns (handle, charge).
        fn submit(&mut self, addr: u16, block: Option<&[u8; BLOCK_LEN]>) -> (u8, u32) {
            if let Some(b) = block {
                self.write(addr, b);
            }
            self.content.port_out(0, addr as u8, &mut self.memory);
            self.content.port_out(1, (addr >> 8) as u8, &mut self.memory);
            let charge = self.content.port_out(2, 0, &mut self.memory);
            (self.content.port_in(2), charge)
        }

        fn complete(&mut self, completions: &[Completion]) -> (usize, usize) {
            self.content.complete(completions, &mut self.memory, &mut self.system)
        }

        fn events(&self) -> Vec<(u8, u8, u8, u8)> {
            self.system.queue().map(|e| (e.kind, e.a, e.b, e.c)).collect()
        }
    }

    /// A block with status AAh, so writes by the device are visible.
    fn block(op: u8, id: [u8; 32], offset: u32, length: u16, addr: u32, size: u32) -> [u8; BLOCK_LEN] {
        let mut b = [0u8; BLOCK_LEN];
        b[0] = op;
        b[STATUS] = 0xAA;
        b[ID..ID + 32].copy_from_slice(&id);
        b[OFFSET..OFFSET + 3].copy_from_slice(&offset.to_le_bytes()[..3]);
        b[LENGTH..LENGTH + 2].copy_from_slice(&length.to_le_bytes());
        b[ADDR..ADDR + 3].copy_from_slice(&addr.to_le_bytes()[..3]);
        b[SIZE..SIZE + 3].copy_from_slice(&size.to_le_bytes()[..3]);
        b
    }

    fn get(offset: u32, length: u16) -> [u8; BLOCK_LEN] {
        block(OP_GET, [7; 32], offset, length, 0x1000, 0)
    }

    fn feed(op: u8, offset: u32, length: u16) -> [u8; BLOCK_LEN] {
        block(op, [7; 32], offset, length, 0x1000, 0)
    }

    fn records(handle: u8, total: u32, data: &[u8]) -> Completion {
        Completion { handle, outcome: Outcome::Records { total, data: data.to_vec() } }
    }

    /// A timeline record (SPEC 7.7) whose entry has an `n`-byte title.
    fn timeline_record(n: usize, flags: u8, fill: u8) -> Vec<u8> {
        let mut r = vec![(1 + ENTRY_HEAD + n) as u8, flags];
        r.extend(std::iter::repeat_n(fill, ENTRY_HEAD - 1));
        r.push(n as u8);
        r.extend(std::iter::repeat_n(b't', n));
        r
    }

    fn put(size: u32) -> [u8; BLOCK_LEN] {
        block(OP_PUT, [0; 32], 0, 0, 0x1000, size)
    }

    fn got(handle: u8, size: u32, data: &[u8]) -> Completion {
        Completion { handle, outcome: Outcome::Got { size, data: data.to_vec() } }
    }

    fn status(rig: &Rig, addr: u16) -> u8 {
        rig.memory.read(addr + STATUS as u16)
    }

    #[test]
    fn reset_state_and_ports() {
        let mut rig = Rig::new();
        assert_eq!(rig.content.port_in(2), 0);
        assert_eq!((rig.content.port_in(0), rig.content.port_in(1), rig.content.port_in(3)), (0xFF, 0xFF, 0xFF));
        let (handle, _) = rig.submit(BLOCK, Some(&get(0, 1)));
        assert_eq!(handle, 1);
        assert_eq!(status(&rig, BLOCK), status::PENDING);
        assert_eq!(rig.content.pending_handles(), [1]);
        let reqs = rig.content.take_requests();
        assert_eq!(reqs, [Request { handle: 1, kind: RequestKind::Get { id: [7; 32], offset: 0, length: 1 } }]);
        assert!(rig.content.take_requests().is_empty());
    }

    #[test]
    fn step1_block_not_in_ram_is_rejected_untouched() {
        let mut rig = Rig::new();
        // Straddles the end of the declared RAM (FFE0h is linear 7FE0h).
        let (handle, _) = rig.submit(0xFFE0, Some(&get(0, 1)));
        assert_eq!(handle, 0);
        assert_eq!(rig.memory.read(0xFFE1), 0xAA, "nothing written");
        // Ends exactly at the end of RAM: accepted.
        assert_eq!(rig.submit(0x10000u32.wrapping_sub(BLOCK_LEN as u32) as u16, Some(&get(0, 1))).0, 1);
        // In ROM, and in an unmapped page.
        assert_eq!(rig.submit(0x0000, None).0, 0);
        rig.memory.port_out(5, 0x84);
        assert_eq!(rig.submit(0xA000, None).0, 0);
        rig.memory.port_out(5, 0x10);
        assert_eq!(rig.submit(0xA000, None).0, 0);
        assert_eq!(rig.content.pending_handles(), [1]);
    }

    #[test]
    fn block_follows_linear_ram_across_slots() {
        let mut rig = Rig::new();
        // Slot 2 shows RAM page 81h; the block runs on into linear page 82h,
        // whatever slot 3 shows.
        rig.memory.port_out(2, 0x81);
        let b = get(0, 1);
        rig.write(0x5FF0, &b[..16]);
        rig.memory.port_out(2, 0x82);
        rig.write(0x4000, &b[16..]);
        rig.memory.port_out(2, 0x81);
        assert_eq!(rig.submit(0x5FF0, None).0, 1);
        assert_eq!(rig.content.take_requests()[0].kind, RequestKind::Get { id: [7; 32], offset: 0, length: 1 });
    }

    #[test]
    fn step2_invalid_requests_get_bad_request() {
        let ram_end = RAM_LEN as u32;
        let bad = [
            block(0, [0; 32], 0, 1, 0x1000, 1),
            block(5, [0; 32], 0, 1, 0x1000, 1),
            block(0xFF, [0; 32], 0, 1, 0x1000, 1),
            get(0, 0),
            feed(OP_SUBSCRIPTIONS, 0, 0),
            feed(OP_TIMELINE, 0, MAX_GET_LENGTH as u16 + 1),
            block(OP_TIMELINE, [0; 32], 0, 2, ram_end - 1, 0),
            get(0, MAX_GET_LENGTH as u16 + 1),
            block(OP_GET, [0; 32], 0, 2, ram_end - 1, 0),
            block(OP_GET, [0; 32], 0, 1, ram_end, 0),
            block(OP_GET, [0; 32], 0, 1, 0xFF_FFFF, 0),
            put(0),
            put(MAX_BLOB as u32 + 1),
            block(OP_PUT, [0; 32], 0, 0, ram_end - 10, 11),
        ];
        let mut rig = Rig::new();
        for b in &bad {
            assert_eq!(rig.submit(BLOCK, Some(b)), (0, 0), "{b:02X?}");
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST, "{b:02X?}");
            assert_eq!(rig.read(BLOCK + 2, 43), b[2..], "only the status is written");
        }
        assert!(rig.content.pending_handles().is_empty());
        // The boundaries themselves are valid.
        let good = [
            block(OP_GET, [0; 32], 0, MAX_GET_LENGTH as u16, ram_end - MAX_GET_LENGTH as u32, 0),
            block(OP_PUT, [0; 32], 0, 0, ram_end - 10, 10),
            block(OP_GET, [0; 32], 0xFF_FFFF, 1, 0, 0),
        ];
        for b in &good {
            assert_ne!(rig.submit(BLOCK, Some(b)).0, 0, "{b:02X?}");
        }
    }

    #[test]
    fn step3_nine_pending_requests_are_rejected() {
        let mut rig = Rig::new();
        for h in 1..=8u16 {
            assert_eq!(rig.submit(BLOCK + h * 64, Some(&get(0, 1))).0, h as u8);
        }
        assert_eq!(rig.submit(BLOCK, Some(&get(0, 1))).0, 0);
        assert_eq!(status(&rig, BLOCK), 0xAA, "block not changed");
        // Step 2 comes first: a bad request is still marked as such.
        assert_eq!(rig.submit(BLOCK, Some(&get(0, 0))).0, 0);
        assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
        assert_eq!(rig.content.take_requests().len(), 8);
    }

    #[test]
    fn lowest_free_handle_is_assigned() {
        let mut rig = Rig::new();
        for h in 1..=4u16 {
            rig.submit(BLOCK + h * 64, Some(&get(0, 1)));
        }
        rig.complete(&[Completion { handle: 3, outcome: Outcome::NotFound }]);
        rig.complete(&[Completion { handle: 2, outcome: Outcome::Failed }]);
        assert_eq!(rig.content.pending_handles(), [1, 4]);
        assert_eq!(rig.submit(BLOCK, Some(&get(0, 1))).0, 2);
        assert_eq!(rig.submit(BLOCK + 0x400, Some(&get(0, 1))).0, 3);
        assert_eq!(rig.submit(BLOCK + 0x440, Some(&get(0, 1))).0, 5);
    }

    #[test]
    fn completions_apply_in_handle_order_and_duplicates_are_ignored() {
        let mut rig = Rig::new();
        for h in 1..=3u16 {
            rig.submit(BLOCK + h * 64, Some(&get(0, 1)));
        }
        let c = |handle, outcome| Completion { handle, outcome };
        let (ignored, dropped) = rig.complete(&[
            c(3, Outcome::Failed),
            c(1, Outcome::NotFound),
            c(2, Outcome::Failed),
            c(1, Outcome::Failed),
            c(0, Outcome::Failed),
            c(9, Outcome::Failed),
            c(5, Outcome::Failed),
        ]);
        assert_eq!((ignored, dropped), (4, 0));
        assert_eq!(rig.events(), [(5, 1, 2, 0), (5, 2, 3, 0), (5, 3, 3, 0)]);
        assert_eq!(status(&rig, BLOCK + 64), status::NOT_FOUND, "the first completion for a handle wins");
    }

    #[test]
    fn get_delivers_bytes_length_and_size() {
        let mut rig = Rig::new();
        rig.write(DATA, &[0xEE; 32]);
        rig.submit(BLOCK, Some(&get(90, 20)));
        // The blob ends first: 10 bytes delivered.
        let data: Vec<u8> = (1..=10).collect();
        assert_eq!(rig.complete(&[got(1, 100, &data[..9])]), (1, 0), "wrong count is ignored");
        assert_eq!(rig.complete(&[got(1, 100, &[0; 11])]), (1, 0));
        assert_eq!(rig.content.pending_handles(), [1]);
        assert_eq!(rig.complete(&[got(1, 100, &data)]), (0, 0));
        assert_eq!(rig.read(DATA, 11), [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0xEE]);
        assert_eq!(status(&rig, BLOCK), status::DONE);
        assert_eq!(rig.read(BLOCK + LENGTH as u16, 2), [10, 0]);
        assert_eq!(rig.read(BLOCK + SIZE as u16, 3), [100, 0, 0]);
        assert_eq!(rig.read(BLOCK + ID as u16, 32), [7; 32], "id untouched");
        assert_eq!(rig.events(), [(5, 1, 1, 0)]);
        assert!(rig.content.pending_handles().is_empty());
    }

    #[test]
    fn get_at_or_beyond_the_end_delivers_nothing() {
        for offset in [100, 5000] {
            let mut rig = Rig::new();
            rig.write(DATA, &[0xEE]);
            rig.submit(BLOCK, Some(&get(offset, 20)));
            assert_eq!(rig.complete(&[got(1, 100, &[1])]), (1, 0));
            assert_eq!(rig.complete(&[got(1, 100, &[])]), (0, 0));
            assert_eq!(status(&rig, BLOCK), status::DONE);
            assert_eq!(rig.read(BLOCK + LENGTH as u16, 2), [0, 0]);
            assert_eq!(rig.read(BLOCK + SIZE as u16, 3), [100, 0, 0]);
            assert_eq!(rig.memory.read(DATA), 0xEE);
        }
    }

    #[test]
    fn get_size_must_be_a_blob_size() {
        let mut rig = Rig::new();
        rig.submit(BLOCK, Some(&get(0, 1)));
        assert_eq!(rig.complete(&[got(1, 0, &[])]), (1, 0));
        assert_eq!(rig.complete(&[got(1, MAX_BLOB as u32 + 1, &[0])]), (1, 0));
        assert_eq!(rig.complete(&[got(1, MAX_BLOB as u32, &[0])]), (0, 0));
    }

    #[test]
    fn failures_write_only_the_status() {
        for (outcome, want) in [(Outcome::NotFound, status::NOT_FOUND), (Outcome::Failed, status::FAILED)] {
            let mut rig = Rig::new();
            let b = get(3, 9);
            rig.submit(BLOCK, Some(&b));
            rig.complete(&[Completion { handle: 1, outcome }]);
            assert_eq!(status(&rig, BLOCK), want);
            assert_eq!(rig.read(BLOCK + 2, 43), b[2..]);
            assert_eq!(rig.events(), [(5, 1, want, 0)]);
        }
    }

    #[test]
    fn put_copies_data_at_submit_and_charges_per_byte() {
        let mut rig = Rig::new();
        rig.write(DATA, b"hello");
        assert_eq!(rig.submit(BLOCK, Some(&put(5))), (1, 5));
        rig.write(DATA, b"HELLO");
        let reqs = rig.content.take_requests();
        assert_eq!(reqs, [Request { handle: 1, kind: RequestKind::Put { data: b"hello".to_vec() } }]);
        let id = *blake3::hash(b"hello").as_bytes();
        let wrong = *blake3::hash(b"HELLO").as_bytes();
        assert_eq!(rig.complete(&[Completion { handle: 1, outcome: Outcome::Put { id: wrong } }]), (1, 0));
        assert_eq!(rig.complete(&[got(1, 5, &[])]), (1, 0), "a get result does not complete a put");
        assert_eq!(rig.complete(&[Completion { handle: 1, outcome: Outcome::Put { id } }]), (0, 0));
        assert_eq!(rig.read(BLOCK + ID as u16, 32), id);
        assert_eq!(status(&rig, BLOCK), status::DONE);
        // Rejected puts cost nothing.
        assert_eq!(rig.submit(BLOCK, Some(&put(RAM_LEN as u32 + 1))), (0, 0));
    }

    #[test]
    fn put_result_does_not_complete_a_get() {
        let mut rig = Rig::new();
        rig.submit(BLOCK, Some(&get(0, 1)));
        assert_eq!(rig.complete(&[Completion { handle: 1, outcome: Outcome::Put { id: [0; 32] } }]), (1, 0));
        assert_eq!(rig.content.pending_handles(), [1]);
    }

    #[test]
    fn event_dropped_when_queue_full_but_block_updated() {
        let mut rig = Rig::new();
        rig.submit(BLOCK, Some(&get(0, 1)));
        for _ in 0..crate::system::QUEUE_CAPACITY {
            rig.system.push_event(Event::key_down(4, 0));
        }
        assert_eq!(rig.complete(&[got(1, 1, &[0x42])]), (0, 1));
        assert_eq!(status(&rig, BLOCK), status::DONE);
        assert_eq!(rig.memory.read(DATA), 0x42);
        assert!(rig.events().iter().all(|e| e.0 == Event::KEY_DOWN));
        assert!(rig.content.pending_handles().is_empty());
    }

    #[test]
    fn feed_requests_are_submitted_like_get() {
        let mut rig = Rig::new();
        assert_eq!(rig.submit(BLOCK, Some(&feed(OP_SUBSCRIPTIONS, 3, 72))), (1, 0));
        assert_eq!(rig.submit(BLOCK + 64, Some(&feed(OP_TIMELINE, 0x12_3456, MAX_GET_LENGTH as u16))), (2, 0));
        assert_eq!(
            rig.content.take_requests(),
            [
                Request { handle: 1, kind: RequestKind::Subscriptions { offset: 3, length: 72 } },
                Request { handle: 2, kind: RequestKind::Timeline { offset: 0x12_3456, length: MAX_GET_LENGTH as u16 } },
            ]
        );
        // A get result does not complete a feed request, nor records a get.
        assert_eq!(rig.complete(&[got(1, 1, &[])]), (1, 0));
        rig.submit(BLOCK + 128, Some(&get(0, 1)));
        assert_eq!(rig.complete(&[records(3, 0, &[])]), (1, 0));
    }

    #[test]
    fn subscriptions_deliver_whole_records() {
        let sub = |i: u8| [vec![i; 32], vec![i, 0, 0, 0]].concat();
        // 100 bytes hold two 36-byte records; four exist.
        let mut rig = Rig::new();
        rig.write(DATA, &[0xEE; 110]);
        rig.submit(BLOCK, Some(&feed(OP_SUBSCRIPTIONS, 0, 100)));
        let three = [sub(1), sub(2), sub(3)].concat();
        let two = [sub(1), sub(2)].concat();
        assert_eq!(rig.complete(&[records(1, 4, &three)]), (1, 0), "more than fit");
        assert_eq!(rig.complete(&[records(1, 4, &two[..71])]), (1, 0), "not whole records");
        assert_eq!(rig.complete(&[records(1, 4, &two[..36])]), (1, 0), "fewer than fit");
        assert_eq!(rig.complete(&[records(1, 0x100_0000, &two)]), (1, 0), "total beyond 24 bits");
        assert_eq!(rig.complete(&[records(1, 4, &two)]), (0, 0));
        assert_eq!(rig.read(DATA, 73), [&two[..], &[0xEE]].concat());
        assert_eq!(rig.read(BLOCK + LENGTH as u16, 2), [72, 0]);
        assert_eq!(rig.read(BLOCK + SIZE as u16, 3), [4, 0, 0]);
        assert_eq!(status(&rig, BLOCK), status::DONE);
        assert_eq!(rig.events(), [(5, 1, 1, 0)]);

        // Offset 3 of 4: one record, though two would fit.
        rig.submit(BLOCK, Some(&feed(OP_SUBSCRIPTIONS, 3, 100)));
        assert_eq!(rig.complete(&[records(1, 4, &two)]), (1, 0));
        assert_eq!(rig.complete(&[records(1, 4, &sub(4))]), (0, 0));
        // A buffer too small for one record, and an offset at or beyond the end.
        for (offset, length) in [(0, 35), (4, 100), (5000, 100)] {
            rig.submit(BLOCK, Some(&feed(OP_SUBSCRIPTIONS, offset, length)));
            assert_eq!(rig.complete(&[records(1, 4, &sub(1))]), (1, 0), "{offset} {length}");
            assert_eq!(rig.complete(&[records(1, 4, &[])]), (0, 0), "{offset} {length}");
            assert_eq!(rig.read(BLOCK + LENGTH as u16, 2), [0, 0]);
            assert_eq!(rig.read(BLOCK + SIZE as u16, 3), [4, 0, 0]);
        }
        // No subscriptions at all.
        rig.submit(BLOCK, Some(&feed(OP_SUBSCRIPTIONS, 0, 36)));
        assert_eq!(rig.complete(&[records(1, 0, &[])]), (0, 0));
    }

    #[test]
    fn timeline_records_must_tile_the_delivered_bytes() {
        let (a, b, c) = (timeline_record(0, 1, 0xA1), timeline_record(120, 0, 0xB2), timeline_record(7, 1, 0xC3));
        assert_eq!((a.len(), b.len(), c.len()), (111, MAX_TIMELINE_RECORD, 118));
        let abc = [a.clone(), b.clone(), c.clone()].concat();
        let mut rig = Rig::new();
        rig.submit(BLOCK, Some(&feed(OP_TIMELINE, 0, 1000)));
        let mut bad_l = abc.clone();
        bad_l[0] += 1;
        let mut bad_flags = abc.clone();
        bad_flags[1] = 0x02;
        let mut long_title = timeline_record(121, 0, 0);
        long_title[0] = 0xE7; // 1 + 109 + 121 does not fit in L; any value fails
        let cut = abc[..abc.len() - 1].to_vec();
        let extra = [abc.clone(), vec![0]].concat();
        for (data, why) in [
            (&bad_l[..], "L disagrees with n"),
            (&bad_flags, "unknown flag"),
            (&long_title, "title over 120 bytes"),
            (&cut, "last record cut short"),
            (&extra, "trailing byte"),
            (&abc[..111 + 231], "fewer than fit"),
        ] {
            assert_eq!(rig.complete(&[records(1, 3, data)]), (1, 0), "{why}");
        }
        assert_eq!(rig.complete(&[records(1, 2, &abc)]), (1, 0), "more records than exist");
        assert_eq!(rig.complete(&[records(1, 3, &abc)]), (0, 0));
        assert_eq!(rig.read(DATA, abc.len()), abc);
        assert_eq!(rig.read(BLOCK + LENGTH as u16, 2), (abc.len() as u16).to_le_bytes());
        assert_eq!(rig.read(BLOCK + SIZE as u16, 3), [3, 0, 0]);

        // Room for a and b (342 bytes) but not c: the rest of the buffer is
        // under the largest record, so stopping there is allowed.
        rig.submit(BLOCK, Some(&feed(OP_TIMELINE, 0, 400)));
        assert_eq!(rig.complete(&[records(1, 3, &a)]), (1, 0), "b would have fit");
        assert_eq!(rig.complete(&[records(1, 3, &abc[..342])]), (0, 0));
        // Buffer too small for one record; offsets at and beyond the end.
        for (offset, length) in [(0, 100), (3, 1000), (0x00FF_FFFF, 1000)] {
            rig.submit(BLOCK, Some(&feed(OP_TIMELINE, offset, length)));
            assert_eq!(rig.complete(&[records(1, 3, &[])]), (0, 0), "{offset} {length}");
            assert_eq!(rig.read(BLOCK + LENGTH as u16, 2), [0, 0]);
        }
        rig.submit(BLOCK, Some(&feed(OP_TIMELINE, 0, 1000)));
        assert_eq!(rig.complete(&[records(1, 3, &[])]), (1, 0), "nothing delivered though records fit");
        // An empty timeline; a failure.
        assert_eq!(rig.complete(&[records(1, 0, &[])]), (0, 0));
        rig.submit(BLOCK, Some(&feed(OP_TIMELINE, 0, 1000)));
        rig.complete(&[Completion { handle: 1, outcome: Outcome::Failed }]);
        assert_eq!(status(&rig, BLOCK), status::FAILED);
    }
}
