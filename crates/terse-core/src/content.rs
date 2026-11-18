//! Device 4, Content (SPEC 9).
//!
//! The core only validates requests, copies bytes and writes request blocks.
//! The host reads submitted requests with [`Content::take_requests`] and
//! passes completions back through `FrameInput`.

use crate::cpu::{RAM_BASE, ROM_BASE};
use crate::system::{Event, System};

/// Most requests pending at once; handles are 1 to 8 (SPEC 9.2).
pub const MAX_PENDING: usize = 8;
/// Size of a request block (SPEC 9.1).
pub const BLOCK_LEN: usize = 52;
/// Largest blob (SPEC 9).
pub const MAX_BLOB: usize = 1_048_576;
/// Largest `length` of a get or a records request (SPEC 9.1).
pub const MAX_GET_LENGTH: usize = 16_384;

pub const OP_GET: u8 = 0x01;
pub const OP_PUT: u8 = 0x02;
pub const OP_SUBSCRIPTIONS: u8 = 0x03;
pub const OP_TIMELINE: u8 = 0x04;

/// A subscriptions record: feed key and highest `seq` (SPEC 9.5).
pub const SUBSCRIPTION_RECORD: usize = 36;
/// Bytes of an entry before its title (NETWORK 4.2).
pub const ENTRY_HEAD: usize = 109;
/// Longest entry title (NETWORK 4.2).
pub const MAX_TITLE: usize = 120;
/// Longest timeline record: `L`, flags and an entry with a 120-byte title,
/// without its signature (SPEC 9.5).
pub const MAX_TIMELINE_RECORD: usize = 2 + ENTRY_HEAD + MAX_TITLE;
/// Timeline record flag: the article blob is in the host's store.
pub const FLAG_LOCAL: u8 = 0x01;

/// Status values (SPEC 9.3).
pub mod status {
    pub const PENDING: u8 = 0x00;
    pub const DONE: u8 = 0x01;
    pub const NOT_FOUND: u8 = 0x02;
    pub const FAILED: u8 = 0x03;
    pub const BAD_REQUEST: u8 = 0x04;
}

// Request block field offsets (SPEC 9.1).
pub const OP: usize = 0;
pub const STATUS: usize = 1;
pub const ID: usize = 4;
pub const OFFSET: usize = 36;
pub const LENGTH: usize = 40;
pub const ADDR: usize = 44;
pub const SIZE: usize = 48;

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
    /// Subscription records from index `offset`, at most `length` bytes (SPEC 9.5).
    Subscriptions { offset: u32, length: u16 },
    /// Timeline records from index `offset`, at most `length` bytes (SPEC 9.5).
    Timeline { offset: u32, length: u16 },
}

/// The host's answer to a pending request (SPEC 9.2), an input to the machine
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
    /// as many as fit in `length` (SPEC 9.5).
    Records { total: u32, data: Vec<u8> },
    NotFound,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    /// `addr` is the guest address of the data.
    Get { id: [u8; 32], offset: u32, length: u16, addr: u32 },
    Put { id: [u8; 32], size: u32 },
    Records { op: u8, offset: u32, length: u16, addr: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    /// Guest address of the request block, fixed at submit.
    block: u32,
    pending: Pending,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Content {
    slots: [Option<Slot>; MAX_PENDING],
    submitted: Vec<Request>,
}

/// Whether `data` can be the answer to a subscriptions or timeline request
/// (SPEC 9.5): whole, well-formed records, no more than fit in `length` or
/// exist after `offset`, and as many as fit. Subscription records have a
/// fixed size, so their count is exact. A timeline record's size is only
/// known from its bytes: the records must tile `data` exactly, each `L`
/// must match its entry's title length, and space left over must be too
/// small for any record when records remain.
pub fn records_fit(op: u8, offset: u32, length: u16, total: u32, data: &[u8]) -> bool {
    let (length, remaining) = (length as usize, total.saturating_sub(offset) as usize);
    if data.len() > length {
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
    /// Submits the block at guest address `block_addr` (SPEC 9.2 steps
    /// 1-4). Returns the handle (1-8, or 0 if rejected) and the number of
    /// bytes a put copied (for the charge).
    pub fn submit(&mut self, block_addr: u32, rom: &[u8], ram: &mut [u8]) -> (u32, usize) {
        // Step 1: the block lies wholly in RAM, else nothing is written.
        let Some(block) = ram_range(ram.len(), block_addr, BLOCK_LEN) else { return (0, 0) };
        let b: [u8; BLOCK_LEN] = ram[block..block + BLOCK_LEN].try_into().unwrap();
        let (offset, length, addr, size) = (u32_at(&b, OFFSET), u32_at(&b, LENGTH), u32_at(&b, ADDR), u32_at(&b, SIZE));
        let in_ram = |len: usize| ram_range(ram.len(), addr, len).is_some();

        // Step 2.
        let request = match b[OP] {
            OP_GET | OP_SUBSCRIPTIONS | OP_TIMELINE => {
                ((1..=MAX_GET_LENGTH as u32).contains(&length) && in_ram(length as usize)).then(|| {
                    let length = length as u16;
                    match b[OP] {
                        OP_GET => {
                            let id = b[ID..ID + 32].try_into().unwrap();
                            (Pending::Get { id, offset, length, addr }, RequestKind::Get { id, offset, length })
                        }
                        op => {
                            let kind = if op == OP_SUBSCRIPTIONS {
                                RequestKind::Subscriptions { offset, length }
                            } else {
                                RequestKind::Timeline { offset, length }
                            };
                            (Pending::Records { op, offset, length, addr }, kind)
                        }
                    }
                })
            }
            // The bytes are copied (and hashed) only once the request is
            // accepted, after step 3.
            OP_PUT => ((1..=MAX_BLOB as u32).contains(&size) && readable(rom, ram, addr, size as usize).is_some())
                .then(|| (Pending::Put { id: [0; 32], size }, RequestKind::Put { data: Vec::new() })),
            _ => None,
        };
        let Some((mut pending, mut kind)) = request else {
            ram[block + STATUS] = status::BAD_REQUEST;
            return (0, 0);
        };

        // Step 3.
        let Some(index) = self.slots.iter().position(Option::is_none) else { return (0, 0) };
        if let (Pending::Put { id, .. }, RequestKind::Put { data }) = (&mut pending, &mut kind) {
            *data = readable(rom, ram, addr, size as usize).unwrap().to_vec();
            *id = *blake3::hash(data).as_bytes();
        }

        // Step 4.
        ram[block + STATUS] = status::PENDING;
        let handle = index as u8 + 1;
        self.slots[index] = Some(Slot { block: block_addr, pending });
        let taken = match &kind {
            RequestKind::Put { data } => data.len(),
            _ => 0,
        };
        self.submitted.push(Request { handle, kind });
        (handle as u32, taken)
    }

    /// Applies completions in increasing handle order (SPEC 9.2). A
    /// completion that does not fit a pending request (unknown or already
    /// completed handle, wrong kind, wrong byte count, size out of range,
    /// wrong put ID) is ignored and the request stays pending. Returns
    /// (ignored, events dropped because the queue was full, bytes delivered
    /// to RAM).
    pub fn complete(&mut self, completions: &[Completion], ram: &mut [u8], system: &mut System) -> (usize, usize, usize) {
        let mut ordered: Vec<&Completion> = completions.iter().collect();
        ordered.sort_by_key(|c| c.handle);
        let (mut ignored, mut dropped, mut delivered) = (0, 0, 0);
        for c in ordered {
            match self.apply(c, ram) {
                Some((status, bytes)) => {
                    delivered += bytes;
                    if !system.push_event(Event::content_done(c.handle, status)) {
                        dropped += 1;
                    }
                }
                None => ignored += 1,
            }
        }
        (ignored, dropped, delivered)
    }

    /// Completes one request; returns its final status and the bytes
    /// delivered, or `None` if ignored.
    fn apply(&mut self, c: &Completion, ram: &mut [u8]) -> Option<(u8, usize)> {
        let index = (c.handle as usize).checked_sub(1).filter(|&i| i < MAX_PENDING)?;
        let slot = self.slots[index].as_ref()?;
        // Submit checked that the block and the data range lie in RAM.
        let block = ram_range(ram.len(), slot.block, BLOCK_LEN)?;
        let put32 = |ram: &mut [u8], at: usize, v: u32| ram[block + at..block + at + 4].copy_from_slice(&v.to_le_bytes());
        let mut delivered = 0;
        let status = match (&slot.pending, &c.outcome) {
            (&Pending::Get { offset, length, addr, .. }, Outcome::Got { size, data }) => {
                let (size, offset) = (*size as usize, offset as usize);
                let expected = (length as usize).min(size.saturating_sub(offset));
                if !(1..=MAX_BLOB).contains(&size) || data.len() != expected {
                    return None;
                }
                let at = ram_range(ram.len(), addr, data.len())?;
                ram[at..at + data.len()].copy_from_slice(data);
                put32(ram, LENGTH, data.len() as u32);
                put32(ram, SIZE, size as u32);
                delivered = data.len();
                status::DONE
            }
            (&Pending::Records { op, offset, length, addr }, Outcome::Records { total, data }) => {
                if !records_fit(op, offset, length, *total, data) {
                    return None;
                }
                let at = ram_range(ram.len(), addr, data.len())?;
                ram[at..at + data.len()].copy_from_slice(data);
                put32(ram, LENGTH, data.len() as u32);
                put32(ram, SIZE, *total);
                delivered = data.len();
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
        Some((status, delivered))
    }

    /// The eight request slots (STATE.md, content section). Requests not
    /// yet taken by the host are not included: they are also in the slots.
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        for slot in &self.slots {
            // op, block, id, offset, length, addr, size: 53 bytes.
            let (op, block, id, offset, length, addr, size) = match slot {
                None => (0, 0, [0; 32], 0, 0, 0, 0),
                Some(Slot { block, pending }) => match *pending {
                    Pending::Get { id, offset, length, addr } => (OP_GET, *block, id, offset, length as u32, addr, 0),
                    Pending::Put { id, size } => (OP_PUT, *block, id, 0, 0, 0, size),
                    Pending::Records { op, offset, length, addr } => (op, *block, [0; 32], offset, length as u32, addr, 0),
                },
            };
            out.push(op);
            out.extend_from_slice(&block.to_le_bytes());
            out.extend_from_slice(&id);
            for v in [offset, length, addr, size] {
                out.extend_from_slice(&v.to_le_bytes());
            }
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

    const BLOCK: u32 = RAM_BASE;
    const DATA: u32 = RAM_BASE + 0x1000;
    const RAM_LEN: usize = 4 * 0x2000;
    const ROM: &[u8] = b"rom bytes";

    struct Rig {
        content: Content,
        ram: Vec<u8>,
        system: System,
    }

    impl Rig {
        fn new() -> Self {
            Rig { content: Content::default(), ram: vec![0; RAM_LEN], system: System::default() }
        }

        fn write(&mut self, addr: u32, bytes: &[u8]) {
            let at = (addr - RAM_BASE) as usize;
            self.ram[at..at + bytes.len()].copy_from_slice(bytes);
        }

        fn read(&self, addr: u32, len: usize) -> Vec<u8> {
            let at = (addr - RAM_BASE) as usize;
            self.ram[at..at + len].to_vec()
        }

        /// Writes `block` at `addr` (if any) and submits it; returns
        /// (handle, bytes a put copied).
        fn submit(&mut self, addr: u32, block: Option<&[u8; BLOCK_LEN]>) -> (u8, usize) {
            if let Some(b) = block {
                self.write(addr, b);
            }
            let (h, taken) = self.content.submit(addr, ROM, &mut self.ram);
            (h as u8, taken)
        }

        fn complete(&mut self, completions: &[Completion]) -> (usize, usize) {
            let (ignored, dropped, _) = self.content.complete(completions, &mut self.ram, &mut self.system);
            (ignored, dropped)
        }

        fn events(&self) -> Vec<(u8, u8, u8, u8)> {
            self.system.queue().map(|e| (e.kind, e.a, e.b, e.c)).collect()
        }
    }

    /// A block with status AAh, so writes by the device are visible.
    fn block(op: u8, id: [u8; 32], offset: u32, length: u32, addr: u32, size: u32) -> [u8; BLOCK_LEN] {
        let mut b = [0u8; BLOCK_LEN];
        b[0] = op;
        b[STATUS] = 0xAA;
        b[2] = 0x55;
        b[ID..ID + 32].copy_from_slice(&id);
        b[OFFSET..OFFSET + 4].copy_from_slice(&offset.to_le_bytes());
        b[LENGTH..LENGTH + 4].copy_from_slice(&length.to_le_bytes());
        b[ADDR..ADDR + 4].copy_from_slice(&addr.to_le_bytes());
        b[SIZE..SIZE + 4].copy_from_slice(&size.to_le_bytes());
        b
    }

    fn get(offset: u32, length: u32) -> [u8; BLOCK_LEN] {
        block(OP_GET, [7; 32], offset, length, DATA, 0)
    }

    fn feed(op: u8, offset: u32, length: u32) -> [u8; BLOCK_LEN] {
        block(op, [7; 32], offset, length, DATA, 0)
    }

    fn records(handle: u8, total: u32, data: &[u8]) -> Completion {
        Completion { handle, outcome: Outcome::Records { total, data: data.to_vec() } }
    }

    /// A timeline record (SPEC 9.5) whose entry has an `n`-byte title.
    fn timeline_record(n: usize, flags: u8, fill: u8) -> Vec<u8> {
        let mut r = vec![(1 + ENTRY_HEAD + n) as u8, flags];
        r.extend(std::iter::repeat_n(fill, ENTRY_HEAD - 1));
        r.push(n as u8);
        r.extend(std::iter::repeat_n(b't', n));
        r
    }

    fn put(size: u32) -> [u8; BLOCK_LEN] {
        block(OP_PUT, [0; 32], 0, 0, DATA, size)
    }

    fn got(handle: u8, size: u32, data: &[u8]) -> Completion {
        Completion { handle, outcome: Outcome::Got { size, data: data.to_vec() } }
    }

    fn status(rig: &Rig, addr: u32) -> u8 {
        rig.read(addr + STATUS as u32, 1)[0]
    }

    #[test]
    fn submit_and_take() {
        let mut rig = Rig::new();
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
        let end = RAM_BASE + RAM_LEN as u32;
        // Straddles the end of RAM.
        rig.write(end - 32, &get(0, 1)[..32]);
        assert_eq!(rig.submit(end - 32, None).0, 0);
        assert_eq!(rig.read(end - 31, 1), [0xAA], "nothing written");
        // Ends exactly at the end of RAM: accepted, at any alignment.
        assert_eq!(rig.submit(end - BLOCK_LEN as u32, Some(&get(0, 1))).0, 1);
        assert_eq!(rig.submit(BLOCK + 0x203, Some(&get(0, 1))).0, 2);
        // In ROM, and nowhere.
        for addr in [ROM_BASE, 0, RAM_BASE - 1, u32::MAX] {
            assert_eq!(rig.submit(addr, None).0, 0);
        }
        assert_eq!(rig.content.pending_handles(), [1, 2]);
    }

    #[test]
    fn step2_invalid_requests_get_bad_request() {
        let ram_end = RAM_BASE + RAM_LEN as u32;
        let bad = [
            block(0, [0; 32], 0, 1, 0x1000, 1),
            block(5, [0; 32], 0, 1, 0x1000, 1),
            block(0xFF, [0; 32], 0, 1, 0x1000, 1),
            get(0, 0),
            feed(OP_SUBSCRIPTIONS, 0, 0),
            feed(OP_TIMELINE, 0, MAX_GET_LENGTH as u32 + 1),
            block(OP_TIMELINE, [0; 32], 0, 2, ram_end - 1, 0),
            get(0, MAX_GET_LENGTH as u32 + 1),
            get(0, u32::MAX),
            block(OP_GET, [0; 32], 0, 1, ROM_BASE, 0),
            block(OP_GET, [0; 32], 0, 2, ram_end - 1, 0),
            block(OP_GET, [0; 32], 0, 1, ram_end, 0),
            block(OP_GET, [0; 32], 0, 1, 0, 0),
            block(OP_GET, [0; 32], 0, 1, u32::MAX, 0),
            put(0),
            put(MAX_BLOB as u32 + 1),
            block(OP_PUT, [0; 32], 0, 0, ram_end - 10, 11),
            block(OP_PUT, [0; 32], 0, 0, ROM_BASE + 2, ROM.len() as u32 - 1),
        ];
        let mut rig = Rig::new();
        for b in &bad {
            assert_eq!(rig.submit(BLOCK, Some(b)), (0, 0), "{b:02X?}");
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST, "{b:02X?}");
            assert_eq!(rig.read(BLOCK + 2, 50), b[2..], "only the status is written");
        }
        assert!(rig.content.pending_handles().is_empty());
        // The boundaries themselves are valid.
        let good = [
            block(OP_GET, [0; 32], 0, MAX_GET_LENGTH as u32, ram_end - MAX_GET_LENGTH as u32, 0),
            block(OP_PUT, [0; 32], 0, 0, ram_end - 10, 10),
            block(OP_PUT, [0; 32], 0, 0, ROM_BASE, ROM.len() as u32),
            block(OP_GET, [0; 32], u32::MAX, 1, DATA, 0),
        ];
        for b in &good {
            assert_ne!(rig.submit(BLOCK, Some(b)).0, 0, "{b:02X?}");
        }
    }

    #[test]
    fn step3_nine_pending_requests_are_rejected() {
        let mut rig = Rig::new();
        for h in 1..=8u32 {
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
        for h in 1..=4u32 {
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
        for h in 1..=3u32 {
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
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [10, 0, 0, 0]);
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [100, 0, 0, 0]);
        assert_eq!(rig.read(BLOCK + ID as u32, 32), [7; 32], "id untouched");
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
            assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [0, 0, 0, 0]);
            assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [100, 0, 0, 0]);
            assert_eq!(rig.read(DATA, 1), [0xEE]);
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
            assert_eq!(rig.read(BLOCK + 2, 50), b[2..]);
            assert_eq!(rig.events(), [(5, 1, want, 0)]);
        }
    }

    #[test]
    fn put_copies_data_at_submit() {
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
        assert_eq!(rig.read(BLOCK + ID as u32, 32), id);
        assert_eq!(status(&rig, BLOCK), status::DONE);
        // Rejected puts copy nothing.
        assert_eq!(rig.submit(BLOCK, Some(&put(RAM_LEN as u32 + 1))), (0, 0));
        // A put may take its bytes from ROM.
        assert_eq!(rig.submit(BLOCK, Some(&block(OP_PUT, [0; 32], 0, 0, ROM_BASE + 4, 5))), (1, 5));
        assert_eq!(rig.content.take_requests()[0].kind, RequestKind::Put { data: b"bytes".to_vec() });
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
        assert_eq!(rig.read(DATA, 1), [0x42]);
        assert!(rig.events().iter().all(|e| e.0 == Event::KEY_DOWN));
        assert!(rig.content.pending_handles().is_empty());
    }

    #[test]
    fn feed_requests_are_submitted_like_get() {
        let mut rig = Rig::new();
        assert_eq!(rig.submit(BLOCK, Some(&feed(OP_SUBSCRIPTIONS, 3, 72))), (1, 0));
        assert_eq!(rig.submit(BLOCK + 64, Some(&feed(OP_TIMELINE, 0x1234_5678, MAX_GET_LENGTH as u32))), (2, 0));
        assert_eq!(
            rig.content.take_requests(),
            [
                Request { handle: 1, kind: RequestKind::Subscriptions { offset: 3, length: 72 } },
                Request { handle: 2, kind: RequestKind::Timeline { offset: 0x1234_5678, length: MAX_GET_LENGTH as u16 } },
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
        assert_eq!(rig.complete(&[records(1, 4, &two)]), (0, 0));
        assert_eq!(rig.read(DATA, 73), [&two[..], &[0xEE]].concat());
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [72, 0, 0, 0]);
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [4, 0, 0, 0]);
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
            assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [0, 0, 0, 0]);
            assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [4, 0, 0, 0]);
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
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), (abc.len() as u32).to_le_bytes());
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [3, 0, 0, 0]);

        // Room for a and b (342 bytes) but not c: the rest of the buffer is
        // under the largest record, so stopping there is allowed.
        rig.submit(BLOCK, Some(&feed(OP_TIMELINE, 0, 400)));
        assert_eq!(rig.complete(&[records(1, 3, &a)]), (1, 0), "b would have fit");
        assert_eq!(rig.complete(&[records(1, 3, &abc[..342])]), (0, 0));
        // Buffer too small for one record; offsets at and beyond the end.
        for (offset, length) in [(0, 100), (3, 1000), (0x00FF_FFFF, 1000)] {
            rig.submit(BLOCK, Some(&feed(OP_TIMELINE, offset, length)));
            assert_eq!(rig.complete(&[records(1, 3, &[])]), (0, 0), "{offset} {length}");
            assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [0, 0, 0, 0]);
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
