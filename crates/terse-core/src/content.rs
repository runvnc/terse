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
pub const OP_SUBSCRIBE: u8 = 0x05;
pub const OP_UNSUBSCRIBE: u8 = 0x06;
pub const OP_PUBLISH: u8 = 0x07;
pub const OP_IDENTITY: u8 = 0x08;
pub const OP_NAMES: u8 = 0x09;
pub const OP_FIND: u8 = 0x0A;
pub const OP_SET_PROFILE: u8 = 0x0B;
pub const OP_SET_NICKNAME: u8 = 0x0C;
pub const OP_PROGRAMS: u8 = 0x0D;
pub const OP_SAVE: u8 = 0x0E;
pub const OP_LOAD: u8 = 0x0F;
pub const OP_DELETE: u8 = 0x10;
pub const OP_RECORDS: u8 = 0x11;

/// Entry kinds a program may publish (SPEC 9.5, NETWORK 4.2).
pub const ENTRY_ARTICLE: u8 = 1;
pub const ENTRY_DOCUMENT: u8 = 4;
/// A document starts with these bytes (SPEC 9.7).
pub const DOCUMENT_MAGIC: &[u8; 4] = b"TDOC";
/// Bytes of a document header before the format name: magic, version,
/// owner key, name length.
pub const DOCUMENT_HEAD: usize = 38;

/// The format of a document (SPEC 9.7): the owner's key and the format
/// name, and where the data starts; `None` if `blob` does not start with a
/// valid document header.
pub fn document_format(blob: &[u8]) -> Option<([u8; 32], &[u8], usize)> {
    if blob.len() < DOCUMENT_HEAD || &blob[..4] != DOCUMENT_MAGIC || blob[4] != 0 {
        return None;
    }
    let n = blob[37] as usize;
    let name = blob.get(DOCUMENT_HEAD..DOCUMENT_HEAD + n)?;
    valid_display_name(name).then(|| (blob[5..37].try_into().unwrap(), name, DOCUMENT_HEAD + n))
}

/// Largest named record (SPEC 9.6).
pub const MAX_RECORD: usize = 65_536;
/// Longest record name, in bytes (SPEC 9.6).
pub const MAX_RECORD_NAME: usize = 32;
/// A records record: the name, zero padded, and the size (SPEC 9.6).
pub const RECORDS_RECORD: usize = 36;

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
/// Longest display name, nickname or search query, in bytes (SPEC 9.5).
pub const MAX_NAME: usize = 48;
/// Bytes of a names record after `L` and flags: the feed key (SPEC 9.5).
pub const NAMES_HEAD: usize = 32;
/// Longest names record: `L`, flags, key and a 48-byte name.
pub const MAX_NAMES_RECORD: usize = 2 + NAMES_HEAD + MAX_NAME;
/// Names record flag: the name is a local nickname.
pub const NAME_NICKNAME: u8 = 0x01;
/// Names record flag: the feed has a profile in force.
pub const NAME_PROFILE: u8 = 0x02;
/// Names record flag: the feed is not subscribed (listed for its nickname).
pub const NAME_UNSUBSCRIBED: u8 = 0x04;
/// Names record flag: the host follows the feed through a relay named by
/// a hint of its address (NETWORK.md 4.9), since none of its own relays
/// holds it.
pub const NAME_HINTED: u8 = 0x08;
/// Find scopes (`offset` of a find request).
pub const FIND_FEEDS: u8 = 1;
pub const FIND_ENTRIES: u8 = 2;
pub const FIND_PROGRAMS: u8 = 3;
/// Most results a find completes with.
pub const MAX_FIND_RESULTS: u32 = 16;

/// Status values (SPEC 9.3).
pub mod status {
    pub const PENDING: u8 = 0x00;
    pub const DONE: u8 = 0x01;
    pub const NOT_FOUND: u8 = 0x02;
    pub const FAILED: u8 = 0x03;
    pub const BAD_REQUEST: u8 = 0x04;
    pub const REFUSED: u8 = 0x05;
    pub const NO_ROOM: u8 = 0x06;
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
///
/// A range of length 0 is valid at any address (SPEC 3.1); its offset is
/// then 0, and only ever used for an empty slice.
pub fn ram_range(ram_len: usize, addr: u32, len: usize) -> Option<usize> {
    if len == 0 {
        return Some(0);
    }
    let off = addr.wrapping_sub(RAM_BASE) as usize;
    (off <= ram_len && ram_len - off >= len).then_some(off)
}

/// The bytes `[addr, addr + len)` if they lie wholly in ROM or wholly in
/// RAM; empty for a length of 0, at any address (SPEC 3.1).
pub fn readable<'a>(rom: &'a [u8], ram: &'a [u8], addr: u32, len: usize) -> Option<&'a [u8]> {
    if let Some(off) = ram_range(ram.len(), addr, len) {
        return Some(&ram[off..off + len]);
    }
    let off = addr.wrapping_sub(ROM_BASE) as usize;
    (off <= rom.len() && rom.len() - off >= len).then(|| &rom[off..off + len])
}

/// Whether `b` is a valid display name or nickname (SPEC 9.5): UTF-8, 1 to
/// 48 bytes, no control characters, no leading or trailing whitespace.
pub fn valid_display_name(b: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(b) else { return false };
    (1..=MAX_NAME).contains(&b.len())
        && !s.chars().any(char::is_control)
        && !s.starts_with(char::is_whitespace)
        && !s.ends_with(char::is_whitespace)
}

/// The length of the record name in `id` (SPEC 9.6), if it is one: 1 to 32
/// bytes of UTF-8 without control characters, then zeros to the end.
pub fn record_name_len(id: &[u8; 32]) -> Option<usize> {
    let len = id.iter().position(|&b| b == 0).unwrap_or(MAX_RECORD_NAME);
    let name = std::str::from_utf8(&id[..len]).ok()?;
    (len > 0 && id[len..].iter().all(|&b| b == 0) && !name.chars().any(char::is_control)).then_some(len)
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
    /// Subscribe to the feed `feed`; needs the user's consent (SPEC 9.5).
    /// `hints`: the hint list of its address (NETWORK.md 4.9), copied at
    /// submit and valid; empty if none was given.
    Subscribe { feed: [u8; 32], hints: Vec<u8> },
    /// Unsubscribe from the feed `feed`; needs the user's consent.
    Unsubscribe { feed: [u8; 32] },
    /// Publish blob `blob` as the next entry of the host's own feed, with
    /// `title` (valid UTF-8, at most 120 bytes, copied at submit); needs the
    /// user's consent.
    /// `kind` is the entry's kind: [`ENTRY_ARTICLE`] or [`ENTRY_DOCUMENT`]
    /// (SPEC 9.5).
    Publish { blob: [u8; 32], title: Vec<u8>, kind: u8 },
    /// The host's own feed key.
    Identity,
    /// Names records from index `offset`, at most `length` bytes (SPEC 9.5).
    Names { offset: u32, length: u16 },
    /// Search the relays: `scope` 1 (feeds), 2 (articles and documents) or
    /// 3 (programs), the query
    /// (1 to 48 bytes of UTF-8, copied at submit), results in at most
    /// `length` bytes (SPEC 9.5).
    Find { scope: u8, query: Vec<u8>, length: u16 },
    /// Publish a new profile for the host's feed with display name `name`
    /// (copied at submit); needs the user's consent.
    SetProfile { name: Vec<u8>, listed: bool },
    /// Set (or, with an empty name, remove) the local nickname of `feed`;
    /// needs the user's consent.
    SetNickname { feed: [u8; 32], name: Vec<u8> },
    /// The current programs of the subscribed feeds: records from index
    /// `offset`, at most `length` bytes (SPEC 9.5).
    Programs { offset: u32, length: u16 },
    /// Save the named record `name` (1 to 32 bytes) with `data` (copied at
    /// submit), replacing it (SPEC 9.6).
    Save { name: Vec<u8>, data: Vec<u8> },
    /// Up to `length` bytes of the named record from `offset`.
    Load { name: Vec<u8>, offset: u32, length: u16 },
    /// Delete the named record.
    Delete { name: Vec<u8> },
    /// The list of named records: records from index `offset`, at most
    /// `length` bytes.
    Records { offset: u32, length: u16 },
}

/// The host's answer to a pending request (SPEC 9.2), an input to the machine
/// (SPEC 9.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub handle: u8,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A get succeeded: the blob's total size and the bytes delivered from the
    /// requested offset, which must be exactly `min(length, size - offset)`
    /// bytes (none if the offset is at or beyond the end). A load: the same,
    /// with the record's size (0 to 65,536).
    Got { size: u32, data: Vec<u8> },
    /// A put succeeded; `id` must be the BLAKE3-256 hash of the submitted bytes.
    Put { id: [u8; 32] },
    /// A subscriptions, timeline, names, programs or records request
    /// succeeded: the number of records that exist and the whole records
    /// delivered from the requested index, as many as fit in `length` (SPEC
    /// 9.5, 9.6). A find: the number of
    /// results (at most 16) and as many whole records as fit.
    Records { total: u32, data: Vec<u8> },
    /// A subscribe, unsubscribe, set nickname, save or delete succeeded.
    Done,
    /// A publish or set profile succeeded: the new entry's `seq`, written to
    /// `size`.
    Published { seq: u32 },
    /// An identity request succeeded: the host's feed key, written to `id`.
    /// The host has no hints for its own address (or gives none).
    Identity { key: [u8; 32] },
    /// An identity request succeeded: the host's feed key and the hint
    /// list of its own feed address (NETWORK.md 4.9; 1 to 161 bytes, valid),
    /// of which the device writes the whole hints that fit in the
    /// program's buffer, if it gave one (SPEC 9.5).
    IdentityHints { key: [u8; 32], hints: Vec<u8> },
    NotFound,
    Failed,
    /// The user (or the host) did not allow a subscribe, unsubscribe,
    /// publish, set profile or set nickname (SPEC 9.5).
    Refused,
    /// A save would exceed the owner's limits (SPEC 9.6).
    NoRoom,
    /// A document publish whose blob has no valid document header (SPEC
    /// 9.5): status 04h.
    BadRequest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    /// `addr` is the guest address of the data.
    Get { id: [u8; 32], offset: u32, length: u16, addr: u32 },
    Put { id: [u8; 32], size: u32 },
    /// Subscriptions, timeline, names, programs and records.
    Records { op: u8, offset: u32, length: u16, addr: u32 },
    /// Save, load and delete: the name as read at submit (zero padded), a
    /// save's size, a load's range and address.
    Named { op: u8, name: [u8; 32], offset: u32, length: u16, addr: u32, size: u32 },
    /// The query's length is kept for the state record only.
    Find { scope: u8, length: u16, addr: u32, query_len: u8 },
    /// Subscribe, unsubscribe, publish, set profile and set nickname: the
    /// `id` read at submit (zero for set profile), set profile's listed bit
    /// or a publish's kind as given (`offset`) and the length of a
    /// publish's title, the name or a subscribe's hints.
    Feed { op: u8, id: [u8; 32], offset: u8, len: u8 },
    /// Identity: the buffer for the own address's hints (`length` 0: none).
    Identity { length: u16, addr: u32 },
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
    /// Restored with pending requests that no host will answer: they fail
    /// in the next frame (not machine state).
    orphaned: bool,
}

/// How many bytes of a hint list are its whole hints that fit in `room`
/// bytes (SPEC 9.5, identity): all of it, or the first hint, or none.
pub fn hints_that_fit(hints: &[u8], room: usize) -> usize {
    if hints.len() <= room {
        return hints.len();
    }
    match hints.iter().position(|&b| b == b',') {
        Some(first) if first <= room => first,
        _ => 0,
    }
}

/// Whether `data` can be the answer to a subscriptions, timeline, names or
/// find request (SPEC 9.5): whole, well-formed records, no more than fit in
/// `length` or exist after `offset`, and as many as fit. Subscription
/// records have a fixed size, so their count is exact. Other records' sizes
/// are only known from their bytes: the records must tile `data` exactly,
/// each `L` must be in range (for timeline and find records, match its
/// entry's title length), and space left over must be too small for any
/// record when records remain. A find has `offset` 0 and at most 16 results.
pub fn records_fit(op: u8, offset: u32, length: u16, total: u32, data: &[u8]) -> bool {
    let (length, remaining) = (length as usize, total.saturating_sub(offset) as usize);
    if data.len() > length || (op == OP_FIND && total > MAX_FIND_RESULTS) {
        return false;
    }
    if op == OP_SUBSCRIPTIONS || op == OP_RECORDS {
        return data.len() == remaining.min(length / SUBSCRIPTION_RECORD) * SUBSCRIPTION_RECORD;
    }
    let (mut at, mut count) = (0, 0);
    while at < data.len() {
        let l = data[at] as usize;
        let Some(&flags) = data.get(at + 1) else { return false };
        if op == OP_NAMES {
            // L counts the flags byte, the key and the name.
            if !(1 + NAMES_HEAD..=1 + NAMES_HEAD + MAX_NAME).contains(&l)
                || flags & !(NAME_NICKNAME | NAME_PROFILE | NAME_UNSUBSCRIBED | NAME_HINTED) != 0
            {
                return false;
            }
        } else {
            // L counts the flags byte and the entry; byte 108 of the entry is `n`.
            let Some(&n) = data.get(at + 2 + ENTRY_HEAD - 1) else { return false };
            if n as usize > MAX_TITLE || l != 1 + ENTRY_HEAD + n as usize || flags & !FLAG_LOCAL != 0 {
                return false;
            }
        }
        at += 1 + l;
        count += 1;
    }
    let largest = if op == OP_NAMES { MAX_NAMES_RECORD } else { MAX_TIMELINE_RECORD };
    at == data.len() && count <= remaining && (count == remaining || length - data.len() < largest)
}

impl Content {
    /// Submits the block at guest address `block_addr` (SPEC 9.2 steps
    /// 1-4). Returns the handle (1-8, or 0 if rejected) and the number of
    /// bytes copied (a put's data, a publish title, a find query, a name),
    /// for the charge.
    pub fn submit(&mut self, block_addr: u32, rom: &[u8], ram: &mut [u8]) -> (u32, usize) {
        // Step 1: the block lies wholly in RAM, else nothing is written.
        let Some(block) = ram_range(ram.len(), block_addr, BLOCK_LEN) else { return (0, 0) };
        let b: [u8; BLOCK_LEN] = ram[block..block + BLOCK_LEN].try_into().unwrap();
        let (offset, length, addr, size) = (u32_at(&b, OFFSET), u32_at(&b, LENGTH), u32_at(&b, ADDR), u32_at(&b, SIZE));
        let in_ram = |len: usize| ram_range(ram.len(), addr, len).is_some();

        // Step 2.
        let request = match b[OP] {
            OP_GET | OP_SUBSCRIPTIONS | OP_TIMELINE | OP_NAMES | OP_PROGRAMS | OP_RECORDS => {
                ((1..=MAX_GET_LENGTH as u32).contains(&length) && in_ram(length as usize)).then(|| {
                    let length = length as u16;
                    match b[OP] {
                        OP_GET => {
                            let id = b[ID..ID + 32].try_into().unwrap();
                            (Pending::Get { id, offset, length, addr }, RequestKind::Get { id, offset, length })
                        }
                        op => {
                            let kind = match op {
                                OP_SUBSCRIPTIONS => RequestKind::Subscriptions { offset, length },
                                OP_TIMELINE => RequestKind::Timeline { offset, length },
                                OP_PROGRAMS => RequestKind::Programs { offset, length },
                                OP_RECORDS => RequestKind::Records { offset, length },
                                _ => RequestKind::Names { offset, length },
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
            OP_UNSUBSCRIBE => {
                let id: [u8; 32] = b[ID..ID + 32].try_into().unwrap();
                Some((Pending::Feed { op: OP_UNSUBSCRIBE, id, offset: 0, len: 0 }, RequestKind::Unsubscribe { feed: id }))
            }
            // SPEC 9.5: the hints, if any, are a valid hint list of at most
            // 161 bytes in ROM or RAM, copied after step 3.
            OP_SUBSCRIBE => (length == 0
                || (length as usize <= crate::hints::MAX_HINT_LIST
                    && readable(rom, ram, addr, length as usize).is_some_and(|h| crate::hints::parse_hints(h).is_ok())))
            .then(|| {
                let id: [u8; 32] = b[ID..ID + 32].try_into().unwrap();
                (Pending::Feed { op: OP_SUBSCRIBE, id, offset: 0, len: length as u8 }, RequestKind::Subscribe { feed: id, hints: Vec::new() })
            }),
            // SPEC 9.5: a buffer for the own address's hints, if `length`
            // is not 0.
            OP_IDENTITY => (length == 0 || ((1..=MAX_GET_LENGTH as u32).contains(&length) && in_ram(length as usize)))
                .then(|| (Pending::Identity { length: length as u16, addr: if length == 0 { 0 } else { addr } }, RequestKind::Identity)),
            // The title is copied (and charged) only once the request is
            // accepted, after step 3.
            // SPEC 9.5: the kind is 0 or 1 (article) or 4 (document).
            OP_PUBLISH => (length as usize <= MAX_TITLE
                && matches!(offset, 0 | 1 | 4)
                && readable(rom, ram, addr, length as usize).is_some_and(|t| std::str::from_utf8(t).is_ok()))
            .then(|| {
                let id: [u8; 32] = b[ID..ID + 32].try_into().unwrap();
                (
                    Pending::Feed { op: OP_PUBLISH, id, offset: offset as u8, len: length as u8 },
                    RequestKind::Publish { blob: id, title: Vec::new(), kind: if offset == 4 { ENTRY_DOCUMENT } else { ENTRY_ARTICLE } },
                )
            }),
            // The query and the names are copied (and charged) after step 3,
            // like a publish title. A find's query and results share `addr`.
            // SPEC 9.6: the name is checked first; the data (a save's, in ROM
            // or RAM; a load's buffer, in RAM) is copied after step 3.
            OP_SAVE | OP_LOAD | OP_DELETE => {
                let name: [u8; 32] = b[ID..ID + 32].try_into().unwrap();
                record_name_len(&name).and_then(|n| {
                    let named = |offset, length: u32, addr, size| Pending::Named { op: b[OP], name, offset, length: length as u16, addr, size };
                    let text = name[..n].to_vec();
                    match b[OP] {
                        OP_SAVE => (size as usize <= MAX_RECORD && readable(rom, ram, addr, size as usize).is_some())
                            .then(|| (named(0, 0, 0, size), RequestKind::Save { name: text, data: Vec::new() })),
                        OP_LOAD => ((1..=MAX_GET_LENGTH as u32).contains(&length) && in_ram(length as usize)).then(|| {
                            (named(offset, length, addr, 0), RequestKind::Load { name: text, offset, length: length as u16 })
                        }),
                        _ => Some((named(0, 0, 0, 0), RequestKind::Delete { name: text })),
                    }
                })
            }
            OP_FIND => ((FIND_FEEDS as u32..=FIND_PROGRAMS as u32).contains(&offset)
                && (1..=MAX_NAME as u32).contains(&size)
                && (1..=MAX_GET_LENGTH as u32).contains(&length)
                && in_ram(length as usize)
                && ram_range(ram.len(), addr, size as usize).is_some_and(|q| std::str::from_utf8(&ram[q..q + size as usize]).is_ok()))
            .then(|| {
                let (scope, length) = (offset as u8, length as u16);
                (Pending::Find { scope, length, addr, query_len: size as u8 }, RequestKind::Find { scope, query: Vec::new(), length })
            }),
            OP_SET_PROFILE => readable(rom, ram, addr, length as usize).is_some_and(valid_display_name).then(|| {
                let listed = offset & 1 != 0;
                (Pending::Feed { op: OP_SET_PROFILE, id: [0; 32], offset: listed as u8, len: length as u8 }, RequestKind::SetProfile { name: Vec::new(), listed })
            }),
            OP_SET_NICKNAME => (length as usize <= MAX_NAME
                && readable(rom, ram, addr, length as usize).is_some_and(|n| n.is_empty() || valid_display_name(n)))
            .then(|| {
                let id: [u8; 32] = b[ID..ID + 32].try_into().unwrap();
                (Pending::Feed { op: OP_SET_NICKNAME, id, offset: 0, len: length as u8 }, RequestKind::SetNickname { feed: id, name: Vec::new() })
            }),
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
        match &mut kind {
            RequestKind::Publish { title: bytes, .. }
            | RequestKind::SetProfile { name: bytes, .. }
            | RequestKind::SetNickname { name: bytes, .. }
            | RequestKind::Subscribe { hints: bytes, .. } => {
                *bytes = readable(rom, ram, addr, length as usize).unwrap().to_vec();
            }
            RequestKind::Find { query, .. } => *query = readable(rom, ram, addr, size as usize).unwrap().to_vec(),
            RequestKind::Save { data, .. } => *data = readable(rom, ram, addr, size as usize).unwrap().to_vec(),
            _ => {}
        }

        // Step 4.
        ram[block + STATUS] = status::PENDING;
        let handle = index as u8 + 1;
        self.slots[index] = Some(Slot { block: block_addr, pending });
        let taken = match &kind {
            RequestKind::Put { data } | RequestKind::Save { data, .. } => data.len(),
            RequestKind::Publish { title: bytes, .. }
            | RequestKind::SetProfile { name: bytes, .. }
            | RequestKind::SetNickname { name: bytes, .. }
            | RequestKind::Subscribe { hints: bytes, .. }
            | RequestKind::Find { query: bytes, .. } => bytes.len(),
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
            (&Pending::Find { length, addr, .. }, Outcome::Records { total, data }) => {
                if !records_fit(OP_FIND, 0, length, *total, data) {
                    return None;
                }
                let at = ram_range(ram.len(), addr, data.len())?;
                ram[at..at + data.len()].copy_from_slice(data);
                put32(ram, LENGTH, data.len() as u32);
                put32(ram, SIZE, *total);
                delivered = data.len();
                status::DONE
            }
            (&Pending::Named { op: OP_LOAD, offset, length, addr, .. }, Outcome::Got { size, data }) => {
                let (size, offset) = (*size as usize, offset as usize);
                let expected = (length as usize).min(size.saturating_sub(offset));
                if size > MAX_RECORD || data.len() != expected {
                    return None;
                }
                let at = ram_range(ram.len(), addr, data.len())?;
                ram[at..at + data.len()].copy_from_slice(data);
                put32(ram, LENGTH, data.len() as u32);
                put32(ram, SIZE, size as u32);
                delivered = data.len();
                status::DONE
            }
            (&Pending::Named { op: OP_SAVE | OP_DELETE, .. }, Outcome::Done) => status::DONE,
            (&Pending::Named { op: OP_SAVE, .. }, Outcome::NoRoom) => status::NO_ROOM,
            (&Pending::Feed { op: OP_PUBLISH, .. }, Outcome::BadRequest) => status::BAD_REQUEST,
            (Pending::Put { id: want, .. }, Outcome::Put { id }) => {
                if id != want {
                    return None;
                }
                ram[block + ID..block + ID + 32].copy_from_slice(id);
                status::DONE
            }
            (&Pending::Feed { op: OP_SUBSCRIBE | OP_UNSUBSCRIBE | OP_SET_NICKNAME, .. }, Outcome::Done) => status::DONE,
            (&Pending::Feed { op: OP_PUBLISH | OP_SET_PROFILE, .. }, &Outcome::Published { seq }) => {
                put32(ram, SIZE, seq);
                status::DONE
            }
            (&Pending::Identity { length, .. }, Outcome::Identity { key }) => {
                ram[block + ID..block + ID + 32].copy_from_slice(key);
                if length > 0 {
                    put32(ram, LENGTH, 0);
                }
                status::DONE
            }
            // SPEC 9.5: the whole hints that fit in the buffer.
            (&Pending::Identity { length, addr }, Outcome::IdentityHints { key, hints }) => {
                if hints.len() > crate::hints::MAX_HINT_LIST || crate::hints::parse_hints(hints).is_err() {
                    return None;
                }
                ram[block + ID..block + ID + 32].copy_from_slice(key);
                if length > 0 {
                    let fit = hints_that_fit(hints, length as usize);
                    let at = ram_range(ram.len(), addr, fit)?;
                    ram[at..at + fit].copy_from_slice(&hints[..fit]);
                    put32(ram, LENGTH, fit as u32);
                    delivered = fit;
                }
                status::DONE
            }
            (
                &Pending::Feed { op: OP_SUBSCRIBE | OP_UNSUBSCRIBE | OP_PUBLISH | OP_SET_PROFILE | OP_SET_NICKNAME, .. },
                Outcome::Refused,
            ) => status::REFUSED,
            // NETWORK 4.7: the relays refused the search.
            (Pending::Find { .. }, Outcome::Refused) => status::REFUSED,
            // SPEC 2.1: operations outside the machine's reading profile,
            // which such a host may refuse without carrying them out.
            (Pending::Put { .. } | Pending::Named { .. } | Pending::Records { op: OP_PROGRAMS | OP_RECORDS, .. }, Outcome::Refused) => {
                status::REFUSED
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
    pub fn write_state(&self, out: &mut Vec<u8>) {
        for slot in &self.slots {
            // op, block, id, offset, length, addr, size: 53 bytes.
            let (op, block, id, offset, length, addr, size) = match slot {
                None => (0, 0, [0; 32], 0, 0, 0, 0),
                Some(Slot { block, pending }) => match *pending {
                    Pending::Get { id, offset, length, addr } => (OP_GET, *block, id, offset, length as u32, addr, 0),
                    Pending::Put { id, size } => (OP_PUT, *block, id, 0, 0, 0, size),
                    Pending::Records { op, offset, length, addr } => (op, *block, [0; 32], offset, length as u32, addr, 0),
                    Pending::Find { scope, length, addr, query_len } => {
                        (OP_FIND, *block, [0; 32], scope as u32, length as u32, addr, query_len as u32)
                    }
                    Pending::Feed { op, id, offset, len } => (op, *block, id, offset as u32, len as u32, 0, 0),
                    Pending::Identity { length, addr } => (OP_IDENTITY, *block, [0; 32], 0, length as u32, addr, 0),
                    Pending::Named { op, name, offset, length, addr, size } => {
                        (op, *block, name, offset, length as u32, addr, size)
                    }
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

    /// The content section as `write_state` wrote it (STATE.md 4). Every
    /// request restored pending is to fail in the machine's next frame
    /// ([`Content::take_orphans`]); the host never had it.
    pub fn restore(&mut self, r: &mut crate::snapshot::Reader, ram_len: usize) -> Result<(), crate::snapshot::RestoreError> {
        use crate::snapshot::bad;
        for slot in &mut self.slots {
            let op = r.u8()?;
            let block = r.u32()?;
            let id: [u8; 32] = r.array()?;
            let (offset, length, addr, size) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            if op == 0 {
                // A free handle: the rest is zero (checked with the whole).
                continue;
            }
            if ram_range(ram_len, block, BLOCK_LEN).is_none() {
                return Err(bad("request block"));
            }
            let length16 = u16::try_from(length).map_err(|_| bad("request length"))?;
            let small = |v: u32| u8::try_from(v).map_err(|_| bad("request field"));
            let pending = match op {
                OP_GET => Pending::Get { id, offset, length: length16, addr },
                OP_PUT => Pending::Put { id, size },
                OP_SUBSCRIPTIONS | OP_TIMELINE | OP_NAMES | OP_PROGRAMS | OP_RECORDS => {
                    Pending::Records { op, offset, length: length16, addr }
                }
                OP_FIND => Pending::Find { scope: small(offset)?, length: length16, addr, query_len: small(size)? },
                OP_SUBSCRIBE | OP_UNSUBSCRIBE | OP_PUBLISH | OP_SET_PROFILE | OP_SET_NICKNAME => {
                    Pending::Feed { op, id, offset: small(offset)?, len: small(length)? }
                }
                OP_IDENTITY => {
                    if length > MAX_GET_LENGTH as u32 || ram_range(ram_len, addr, length as usize).is_none() {
                        return Err(bad("identity buffer"));
                    }
                    Pending::Identity { length: length16, addr }
                }
                OP_SAVE | OP_LOAD | OP_DELETE => {
                    if record_name_len(&id).is_none() {
                        return Err(bad("record name"));
                    }
                    Pending::Named { op, name: id, offset, length: length16, addr, size }
                }
                _ => return Err(bad("request op")),
            };
            *slot = Some(Slot { block, pending });
        }
        self.orphaned = self.slots.iter().any(Option::is_some);
        Ok(())
    }

    /// After a restore: a failed completion for each request that was
    /// pending in the snapshot, once (SPEC 9.2).
    pub fn take_orphans(&mut self) -> Vec<Completion> {
        if !std::mem::take(&mut self.orphaned) {
            return Vec::new();
        }
        self.pending_handles().into_iter().map(|handle| Completion { handle, outcome: Outcome::Failed }).collect()
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

    /// SPEC 9.2: a block that is already pending may be submitted again:
    /// two requests, each writing the same block when it completes (in
    /// handle order within a frame). A get at or beyond the end of the blob
    /// completes with 01h, length 0 and the blob's size. Bytes delivered in
    /// one frame are added up, then charged (rounded up) once.
    #[test]
    fn same_block_twice_get_past_the_end_and_delivered_bytes() {
        let mut rig = Rig::new();
        assert_eq!(rig.submit(BLOCK, Some(&get(5, 3))).0, 1);
        assert_eq!(rig.submit(BLOCK, None).0, 2, "the same block again");
        assert_eq!(rig.content.pending_handles(), [1, 2]);
        let (_, _, delivered) = rig.content.complete(
            &[done(2, Outcome::Got { size: 6, data: vec![9] }), done(1, Outcome::Got { size: 5, data: vec![] })],
            &mut rig.ram,
            &mut rig.system,
        );
        assert_eq!(delivered, 1);
        assert_eq!(rig.events(), [(5, 1, 1, 0), (5, 2, 1, 0)]);
        // Handle 2's write is the last: length 1, size 6.
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), 1u32.to_le_bytes());
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), 6u32.to_le_bytes());
        // Alone, handle 1's past-the-end completion: length 0, the size.
        let mut rig = Rig::new();
        rig.submit(BLOCK, Some(&get(5, 3)));
        rig.complete(&[done(1, Outcome::Got { size: 5, data: vec![] })]);
        assert_eq!((status(&rig, BLOCK), rig.read(BLOCK + LENGTH as u32, 4), rig.read(BLOCK + SIZE as u32, 4)), (status::DONE, 0u32.to_le_bytes().to_vec(), 5u32.to_le_bytes().to_vec()));
        // Three gets of 1 byte each in one frame: 3 bytes delivered, which
        // the machine charges as 1 unit (not 3).
        let mut rig = Rig::new();
        for i in 0..3 {
            rig.submit(BLOCK + 64 * i, Some(&get(0, 1)));
        }
        let got = |h| done(h, Outcome::Got { size: 1, data: vec![1] });
        let (_, _, delivered) = rig.content.complete(&[got(1), got(2), got(3)], &mut rig.ram, &mut rig.system);
        assert_eq!(crate::machine::copy_charge(delivered), 1);
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
            block(0x12, [0; 32], 0, 1, 0x1000, 1),
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

    fn feed_op(op: u8, id: [u8; 32], length: u32, addr: u32) -> [u8; BLOCK_LEN] {
        block(op, id, 0, length, addr, 0)
    }

    fn done(handle: u8, outcome: Outcome) -> Completion {
        Completion { handle, outcome }
    }

    /// SPEC 9.5: a publish's `offset` is the kind (0 or 1 article, 4
    /// document; others are bad requests); a document whose blob has no
    /// header completes with 04h. SPEC 9.7: the document header.
    #[test]
    fn publish_kinds_and_documents() {
        let mut rig = Rig::new();
        rig.write(DATA, b"T");
        for (kind, want) in [(0, Some(ENTRY_ARTICLE)), (1, Some(ENTRY_ARTICLE)), (4, Some(ENTRY_DOCUMENT)), (2, None), (3, None), (5, None)] {
            let (handle, _) = rig.submit(BLOCK, Some(&block(OP_PUBLISH, [5; 32], kind, 1, DATA, 0)));
            match want {
                None => assert_eq!((handle, status(&rig, BLOCK)), (0, status::BAD_REQUEST), "kind {kind}"),
                Some(k) => {
                    assert_eq!(rig.content.take_requests(), [Request { handle, kind: RequestKind::Publish { blob: [5; 32], title: b"T".to_vec(), kind: k } }]);
                    assert_eq!(rig.complete(&[done(handle, Outcome::BadRequest)]), (0, 0));
                    assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
                }
            }
        }
        let doc = |name: &[u8]| [&b"TDOC\0"[..], &[7; 32], &[name.len() as u8], name, b"data"].concat();
        assert_eq!(document_format(&doc(b"sketch")), Some(([7; 32], &b"sketch"[..], 44)));
        assert_eq!(document_format(&doc(b"sketch")[..44]).map(|f| f.2), Some(44), "no data");
        assert_eq!(document_format(&doc(b"")), None);
        assert_eq!(document_format(&doc(b" x")), None);
        assert_eq!(document_format(&doc(&[b'x'; 49])), None);
        assert_eq!(document_format(&doc(b"sketch")[..40]), None, "cut short");
        let mut v1 = doc(b"sketch");
        v1[4] = 1;
        assert_eq!(document_format(&v1), None, "version 1");
        assert_eq!(document_format(b"TERS\0\x01"), None);
    }

    /// SPEC 9.5: subscribe, unsubscribe, publish and identity are submitted
    /// like the other requests and complete with their own outcomes.
    #[test]
    fn subscribe_unsubscribe_publish_identity() {
        let mut rig = Rig::new();
        rig.write(DATA, "Hello, ünïcode".as_bytes());
        let title_len = "Hello, ünïcode".len() as u32;
        assert_eq!(rig.submit(BLOCK, Some(&feed_op(OP_SUBSCRIBE, [3; 32], 0, 0))), (1, 0));
        assert_eq!(rig.submit(BLOCK + 64, Some(&feed_op(OP_UNSUBSCRIBE, [4; 32], 0, 0))), (2, 0));
        assert_eq!(rig.submit(BLOCK + 128, Some(&feed_op(OP_PUBLISH, [5; 32], title_len, DATA))), (3, title_len as usize));
        // The title is copied at submit.
        rig.write(DATA, b"XXXXX");
        assert_eq!(rig.submit(BLOCK + 192, Some(&feed_op(OP_IDENTITY, [6; 32], 0, 0))), (4, 0));
        // A publish with no title, from ROM, and at the end of ROM (empty).
        assert_eq!(rig.submit(BLOCK + 256, Some(&feed_op(OP_PUBLISH, [7; 32], 3, ROM_BASE))).0, 5);
        assert_eq!(rig.submit(BLOCK + 320, Some(&feed_op(OP_PUBLISH, [8; 32], 0, ROM_BASE + ROM.len() as u32))).0, 6);
        assert_eq!(
            rig.content.take_requests(),
            [
                Request { handle: 1, kind: RequestKind::Subscribe { feed: [3; 32], hints: Vec::new() } },
                Request { handle: 2, kind: RequestKind::Unsubscribe { feed: [4; 32] } },
                Request { handle: 3, kind: RequestKind::Publish { blob: [5; 32], title: "Hello, ünïcode".into(), kind: 1 } },
                Request { handle: 4, kind: RequestKind::Identity },
                Request { handle: 5, kind: RequestKind::Publish { blob: [7; 32], title: b"rom".to_vec(), kind: 1 } },
                Request { handle: 6, kind: RequestKind::Publish { blob: [8; 32], title: Vec::new(), kind: 1 } },
            ]
        );
        // Outcomes that do not fit the request are ignored.
        assert_eq!(rig.complete(&[done(1, Outcome::Published { seq: 1 })]), (1, 0));
        assert_eq!(rig.complete(&[done(3, Outcome::Done)]), (1, 0));
        assert_eq!(rig.complete(&[done(4, Outcome::Refused)]), (1, 0), "identity needs no consent");
        assert_eq!(rig.complete(&[done(4, Outcome::Done)]), (1, 0));
        assert_eq!(rig.complete(&[got(2, 1, &[0])]), (1, 0));
        assert_eq!(rig.content.pending_handles(), [1, 2, 3, 4, 5, 6]);

        let (ignored, _) = rig.complete(&[
            done(1, Outcome::Done),
            done(2, Outcome::Done),
            done(3, Outcome::Published { seq: 0x0102_0304 }),
            done(4, Outcome::Identity { key: [0xAB; 32] }),
        ]);
        assert_eq!(ignored, 0);
        assert_eq!(rig.events(), [(5, 1, 1, 0), (5, 2, 1, 0), (5, 3, 1, 0), (5, 4, 1, 0)]);
        assert_eq!(rig.read(BLOCK + 128 + SIZE as u32, 4), [4, 3, 2, 1], "size is the new seq");
        assert_eq!(rig.read(BLOCK + 128 + ID as u32, 32), [5; 32], "id untouched");
        assert_eq!(rig.read(BLOCK + 192 + ID as u32, 32), [0xAB; 32], "id is the host's key");
        assert_eq!(rig.read(BLOCK + 2, 50), feed_op(OP_SUBSCRIBE, [3; 32], 0, 0)[2..], "only the status written");
        assert_eq!(rig.content.pending_handles(), [5, 6]);
    }

    #[test]
    fn refused_and_not_found() {
        let mut rig = Rig::new();
        for (op, h) in [(OP_SUBSCRIBE, 1u32), (OP_UNSUBSCRIBE, 2), (OP_PUBLISH, 3), (OP_IDENTITY, 4)] {
            assert_eq!(rig.submit(BLOCK + h * 64, Some(&feed_op(op, [1; 32], 0, DATA))).0, h as u8);
        }
        rig.complete(&[
            done(1, Outcome::Refused),
            done(2, Outcome::Refused),
            done(3, Outcome::NotFound),
            done(4, Outcome::NotFound),
        ]);
        assert_eq!(rig.events(), [(5, 1, 5, 0), (5, 2, 5, 0), (5, 3, 2, 0), (5, 4, 2, 0)]);
        assert_eq!(status(&rig, BLOCK + 64), status::REFUSED);
        assert_eq!(status(&rig, BLOCK + 192), status::NOT_FOUND);
        rig.submit(BLOCK, Some(&feed_op(OP_PUBLISH, [1; 32], 0, DATA)));
        rig.complete(&[done(1, Outcome::Refused)]);
        assert_eq!(status(&rig, BLOCK), status::REFUSED);
        // Refused is not an answer to a get.
        rig.submit(BLOCK, Some(&get(0, 1)));
        assert_eq!(rig.complete(&[done(1, Outcome::Refused)]), (1, 0));
    }

    /// SPEC 2.1, 9.3: every operation outside the reading profile (put,
    /// publish, find, set profile, set nickname, programs, save, load,
    /// delete, records) may complete with 05h; those inside it that need
    /// no consent (get, subscriptions, timeline, names, identity) may not.
    #[test]
    fn refused_outside_the_reading_profile() {
        let mut rig = Rig::new();
        rig.write(DATA, b"Ann");
        let blocks = [
            block(OP_PUT, [0; 32], 0, 0, DATA, 3),
            feed_op(OP_PUBLISH, [1; 32], 0, DATA),
            find(1, 3, 100, DATA),
            set_profile(3, DATA, 0),
            feed(OP_PROGRAMS, 0, 100),
            named(OP_SAVE, "k", 0, 0, DATA, 3),
            named(OP_LOAD, "k", 0, 4, DATA, 0),
            named(OP_DELETE, "k", 0, 0, 0, 0),
        ];
        for (i, b) in blocks.iter().enumerate() {
            assert_eq!(rig.submit(BLOCK + 64 * i as u32, Some(b)).0, i as u8 + 1, "{b:02X?}");
        }
        let refused: Vec<Completion> = (1..=8).map(|h| done(h, Outcome::Refused)).collect();
        assert_eq!(rig.complete(&refused), (0, 0));
        assert!(rig.content.pending_handles().is_empty());
        for i in 0..8u32 {
            assert_eq!(status(&rig, BLOCK + 64 * i), status::REFUSED);
        }
        let mut rig = Rig::new();
        rig.submit(BLOCK, Some(&named(OP_RECORDS, "", 0, 72, DATA, 0)));
        rig.submit(BLOCK + 64, Some(&feed_op(OP_SET_NICKNAME, [1; 32], 0, DATA)));
        assert_eq!(rig.complete(&[done(1, Outcome::Refused), done(2, Outcome::Refused)]), (0, 0));
        for b in [feed(OP_SUBSCRIPTIONS, 0, 100), feed(OP_TIMELINE, 0, 300), feed(OP_NAMES, 0, 100), feed_op(OP_IDENTITY, [0; 32], 0, 0)] {
            let mut rig = Rig::new();
            rig.submit(BLOCK, Some(&b));
            assert_eq!(rig.complete(&[done(1, Outcome::Refused)]), (1, 0), "{:02X}h", b[0]);
        }
    }

    #[test]
    fn bad_publish_requests() {
        let ram_end = RAM_BASE + RAM_LEN as u32;
        let mut rig = Rig::new();
        rig.write(DATA, &[0xC3, 0x28]); // malformed UTF-8
        rig.write(DATA + 16, &[b'a'; 121]);
        for b in [
            feed_op(OP_PUBLISH, [1; 32], 2, DATA),               // not UTF-8
            feed_op(OP_PUBLISH, [1; 32], 121, DATA + 16),        // longer than 120 bytes
            feed_op(OP_PUBLISH, [1; 32], 2, ram_end - 1),        // past the end of RAM
            feed_op(OP_PUBLISH, [1; 32], 4, ROM_BASE + ROM.len() as u32 - 3), // past the end of ROM
            feed_op(OP_PUBLISH, [1; 32], 1, 0),                  // nowhere
            feed_op(OP_PUBLISH, [1; 32], 1, u32::MAX),
        ] {
            assert_eq!(rig.submit(BLOCK, Some(&b)), (0, 0), "{b:02X?}");
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
        }
        assert!(rig.content.pending_handles().is_empty());
        assert_eq!(rig.submit(BLOCK, Some(&feed_op(OP_PUBLISH, [1; 32], 120, DATA + 16))), (1, 120));
        // SPEC 3.1: an empty title is valid at any address.
        for addr in [0, ram_end + 1, u32::MAX] {
            assert_ne!(rig.submit(BLOCK + 64, Some(&feed_op(OP_PUBLISH, [1; 32], 0, addr))).0, 0, "{addr:#x}");
        }
    }

    /// SPEC 9.5: a subscribe may carry the hints of the feed's address, a
    /// valid hint list of at most 161 bytes in ROM or RAM, copied at
    /// submit; anything else makes the request invalid.
    #[test]
    fn subscribe_with_hints() {
        let mut rig = Rig::new();
        rig.write(DATA, b"relay.example:7000,[2001:db8::1]");
        assert_eq!(rig.submit(BLOCK, Some(&feed_op(OP_SUBSCRIBE, [3; 32], 32, DATA))), (1, 32));
        rig.write(DATA, b"XXXX");
        assert_eq!(rig.submit(BLOCK + 64, Some(&feed_op(OP_SUBSCRIBE, [4; 32], 3, ROM_BASE))), (2, 3), "rom is a name");
        assert_eq!(
            rig.content.take_requests(),
            [
                Request { handle: 1, kind: RequestKind::Subscribe { feed: [3; 32], hints: b"relay.example:7000,[2001:db8::1]".to_vec() } },
                Request { handle: 2, kind: RequestKind::Subscribe { feed: [4; 32], hints: b"rom".to_vec() } },
            ]
        );
        let long = format!("{0}.{1}.e,{0}.{1}.e", "a".repeat(40), "b".repeat(37));
        assert_eq!(long.len(), 161);
        rig.write(DATA, long.as_bytes());
        assert_eq!(rig.submit(BLOCK + 128, Some(&feed_op(OP_SUBSCRIBE, [5; 32], 161, DATA))).0, 3);
        for (hints, len) in [
            ("a.example,b.example,c.example", 0),
            ("a.example:0", 0),
            ("a.example,", 0),
            ("a b", 0),
            ("[::1", 0),
            (long.as_str(), 162),
            ("a.example", 300),
        ] {
            rig.write(DATA, &[0; 400]);
            rig.write(DATA, hints.as_bytes());
            let len = if len == 0 { hints.len() as u32 } else { len };
            assert_eq!(rig.submit(BLOCK + 192, Some(&feed_op(OP_SUBSCRIBE, [6; 32], len, DATA))), (0, 0), "{hints}");
            assert_eq!(status(&rig, BLOCK + 192), status::BAD_REQUEST, "{hints}");
        }
        // Not in ROM or RAM.
        assert_eq!(rig.submit(BLOCK + 192, Some(&feed_op(OP_SUBSCRIBE, [6; 32], 5, 0x10))), (0, 0));
        // Unsubscribe ignores length and addr, as before.
        assert_eq!(rig.submit(BLOCK + 256, Some(&feed_op(OP_UNSUBSCRIBE, [6; 32], 999, 0x10))).0, 4);
    }

    /// SPEC 9.5: identity with a buffer (`length` > 0, in RAM) also gets
    /// the whole hints of the host's own address that fit, and `length`
    /// says how many bytes were written.
    #[test]
    fn identity_with_hints() {
        let mut rig = Rig::new();
        let hints = b"relay.example,203.0.113.9:7000".to_vec();
        let key = Outcome::IdentityHints { key: [0xAB; 32], hints: hints.clone() };
        for (length, written) in [(0u32, None), (100, Some(30usize)), (20, Some(13)), (13, Some(13)), (5, Some(0))] {
            rig.write(DATA, &[0xEE; 100]);
            let h = rig.submit(BLOCK, Some(&feed_op(OP_IDENTITY, [6; 32], length, DATA))).0 as u8;
            assert!(h > 0);
            assert_eq!(rig.complete(&[done(h, key.clone())]), (0, 0));
            assert_eq!(rig.read(BLOCK + ID as u32, 32), [0xAB; 32]);
            let got = u32::from_le_bytes(rig.read(BLOCK + LENGTH as u32, 4).try_into().unwrap());
            match written {
                None => {
                    assert_eq!(got, 0);
                    assert_eq!(rig.read(DATA, 1), [0xEE]);
                }
                Some(n) => {
                    assert_eq!(got as usize, n, "length {length}");
                    assert_eq!(rig.read(DATA, n), hints[..n]);
                    assert_eq!(rig.read(DATA + n as u32, 1), [0xEE], "nothing past the hints");
                }
            }
        }
        // Without hints, a buffer gets length 0.
        let h = rig.submit(BLOCK, Some(&feed_op(OP_IDENTITY, [6; 32], 50, DATA))).0 as u8;
        assert_eq!(rig.complete(&[done(h, Outcome::Identity { key: [1; 32] })]), (0, 0));
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [0; 4]);
        // Malformed hints from a host are ignored (the request stays pending).
        let h = rig.submit(BLOCK, Some(&feed_op(OP_IDENTITY, [6; 32], 50, DATA))).0 as u8;
        assert_eq!(rig.complete(&[done(h, Outcome::IdentityHints { key: [1; 32], hints: b"a,b,c".to_vec() })]), (1, 0));
        assert_eq!(rig.complete(&[done(h, Outcome::IdentityHints { key: [1; 32], hints: Vec::new() })]), (1, 0));
        assert_eq!(rig.complete(&[done(h, Outcome::NotFound)]), (0, 0));
        // A buffer out of RAM, or too long, makes the request invalid.
        for (length, addr) in [(5, ROM_BASE), (16_385, DATA), (10, 0x10)] {
            assert_eq!(rig.submit(BLOCK, Some(&feed_op(OP_IDENTITY, [6; 32], length, addr))), (0, 0));
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
        }
        assert_eq!(hints_that_fit(b"a,b", 1), 1);
        assert_eq!(hints_that_fit(b"ab,c", 1), 0);
    }

    /// STATE.md 1.8: the pending records of the new operations.
    #[test]
    fn state_records_of_feed_operations() {
        let mut rig = Rig::new();
        rig.write(DATA, b"title");
        rig.write(DATA + 16, b"a.example");
        rig.submit(BLOCK, Some(&feed_op(OP_SUBSCRIBE, [3; 32], 9, DATA + 16)));
        rig.submit(BLOCK + 64, Some(&feed_op(OP_PUBLISH, [5; 32], 5, DATA)));
        rig.submit(BLOCK + 128, Some(&feed_op(OP_IDENTITY, [6; 32], 0, DATA)));
        rig.submit(BLOCK + 192, Some(&feed_op(OP_IDENTITY, [6; 32], 40, DATA)));
        let mut out = Vec::new();
        rig.content.write_state(&mut out);
        assert_eq!(out.len(), 8 * 53);
        let rec = |i: usize| &out[i * 53..(i + 1) * 53];
        let want = |op: u8, block: u32, id: [u8; 32], length: u32, addr: u32| {
            let mut r = vec![op];
            r.extend_from_slice(&block.to_le_bytes());
            r.extend_from_slice(&id);
            r.extend_from_slice(&[0; 4]);
            r.extend_from_slice(&length.to_le_bytes());
            r.extend_from_slice(&addr.to_le_bytes());
            r.extend_from_slice(&[0; 4]);
            r
        };
        // A subscribe keeps the length of its hints (copied at submit); an
        // identity its buffer for the own hints, if it gave one.
        assert_eq!(rec(0), want(OP_SUBSCRIBE, BLOCK, [3; 32], 9, 0));
        assert_eq!(rec(1), want(OP_PUBLISH, BLOCK + 64, [5; 32], 5, 0));
        assert_eq!(rec(2), want(OP_IDENTITY, BLOCK + 128, [0; 32], 0, 0));
        assert_eq!(rec(3), want(OP_IDENTITY, BLOCK + 192, [0; 32], 40, DATA));
        assert!(rec(4).iter().all(|&b| b == 0));
        // They restore as they were.
        let mut restored = Content::default();
        restored.restore(&mut crate::snapshot::Reader::new(&out), RAM_LEN).unwrap();
        let mut again = Vec::new();
        restored.write_state(&mut again);
        assert_eq!(again, out);
    }

    /// A names record (SPEC 9.5) with an `n`-byte name.
    fn names_record(n: usize, flags: u8, key: u8) -> Vec<u8> {
        let mut r = vec![(1 + NAMES_HEAD + n) as u8, flags];
        r.extend([key; 32]);
        r.extend(std::iter::repeat_n(b'n', n));
        r
    }

    #[test]
    fn valid_display_names() {
        for good in ["a", "Ann Example", "ünïcode", "a b", "x\u{200B}", &"a".repeat(48)] {
            assert!(valid_display_name(good.as_bytes()), "{good:?}");
        }
        for bad in [
            "",
            " a",
            "a ",
            "\ta",
            "a\u{3000}",
            "\u{A0}a",
            "a\nb",
            "a\u{0}b",
            "a\u{7F}",
            "a\u{9F}",
            "a\u{85}",
            &"a".repeat(49),
        ] {
            assert!(!valid_display_name(bad.as_bytes()), "{bad:?}");
        }
        assert!(!valid_display_name(&[0xC3, 0x28]));
        // 48 bytes of two-byte characters is fine; 49 bytes is not.
        assert!(valid_display_name("é".repeat(24).as_bytes()));
        assert!(!valid_display_name(["é".repeat(24), "a".into()].concat().as_bytes()));
    }

    #[test]
    fn names_are_submitted_like_subscriptions() {
        let ram_end = RAM_BASE + RAM_LEN as u32;
        let mut rig = Rig::new();
        for b in [
            feed(OP_NAMES, 0, 0),
            feed(OP_NAMES, 0, MAX_GET_LENGTH as u32 + 1),
            block(OP_NAMES, [0; 32], 0, 2, ram_end - 1, 0),
            block(OP_NAMES, [0; 32], 0, 1, ROM_BASE, 0),
        ] {
            assert_eq!(rig.submit(BLOCK, Some(&b)), (0, 0), "{b:02X?}");
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
        }
        assert_eq!(rig.submit(BLOCK, Some(&block(OP_NAMES, [9; 32], 7, MAX_GET_LENGTH as u32, ram_end - MAX_GET_LENGTH as u32, 0))), (1, 0));
        assert_eq!(rig.content.take_requests(), [Request { handle: 1, kind: RequestKind::Names { offset: 7, length: MAX_GET_LENGTH as u16 } }]);
    }

    #[test]
    fn names_records_must_tile_the_delivered_bytes() {
        let (a, b, c) = (names_record(0, 0, 1), names_record(48, 3, 2), names_record(5, NAME_NICKNAME | NAME_UNSUBSCRIBED, 3));
        assert_eq!((a.len(), b.len(), c.len()), (34, MAX_NAMES_RECORD, 39));
        let abc = [a.clone(), b.clone(), c.clone()].concat();
        let mut rig = Rig::new();
        rig.write(DATA, &[0xEE; 200]);
        rig.submit(BLOCK, Some(&feed(OP_NAMES, 0, 1000)));
        let mut short_l = a.clone();
        short_l[0] = 32;
        let mut long_name = names_record(49, 0, 1);
        long_name[0] = 82;
        let mut bad_flags = abc.clone();
        bad_flags[1] = 0x10;
        for (data, why) in [
            (&short_l[..], "L under 33"),
            (&long_name, "name over 48 bytes"),
            (&bad_flags, "unknown flag"),
            (&abc[..abc.len() - 1], "last record cut short"),
            (&[abc.clone(), vec![0]].concat(), "trailing byte"),
            (&[abc.clone(), vec![33]].concat(), "trailing L"),
            (&abc[..34 + 82], "fewer than fit"),
        ] {
            assert_eq!(rig.complete(&[records(1, 3, data)]), (1, 0), "{why}");
        }
        assert_eq!(rig.complete(&[records(1, 2, &abc)]), (1, 0), "more records than exist");
        assert_eq!(rig.complete(&[got(1, 1, &[])]), (1, 0));
        assert_eq!(rig.complete(&[records(1, 3, &abc)]), (0, 0));
        assert_eq!(rig.read(DATA, abc.len() + 1), [&abc[..], &[0xEE]].concat());
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), (abc.len() as u32).to_le_bytes());
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [3, 0, 0, 0]);
        assert_eq!(status(&rig, BLOCK), status::DONE);

        // Room for a and b (116 bytes) but not c: what is left (81) is under
        // the largest record. With 82 bytes left, c must be delivered.
        rig.submit(BLOCK, Some(&feed(OP_NAMES, 0, 116 + 81)));
        assert_eq!(rig.complete(&[records(1, 3, &abc[..116])]), (0, 0));
        rig.submit(BLOCK, Some(&feed(OP_NAMES, 0, 116 + 82)));
        assert_eq!(rig.complete(&[records(1, 3, &abc[..116])]), (1, 0));
        assert_eq!(rig.complete(&[records(1, 3, &abc)]), (0, 0));
        // From index 2: only c.
        rig.submit(BLOCK, Some(&feed(OP_NAMES, 2, 1000)));
        assert_eq!(rig.complete(&[records(1, 3, &abc[34..])]), (1, 0));
        assert_eq!(rig.complete(&[records(1, 3, &c)]), (0, 0));
        // Buffer too small for one record; offset at the end; none at all.
        for (offset, length, total) in [(0, 33, 3), (3, 1000, 3), (0, 1000, 0)] {
            rig.submit(BLOCK, Some(&feed(OP_NAMES, offset, length)));
            assert_eq!(rig.complete(&[records(1, total, &[])]), (0, 0), "{offset} {length}");
            assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [0, 0, 0, 0]);
        }
        // A buffer of 33 bytes still takes no record of 34.
        rig.submit(BLOCK, Some(&feed(OP_NAMES, 0, 33)));
        assert_eq!(rig.complete(&[records(1, 1, &a)]), (1, 0));
    }

    fn find(scope: u32, query_len: u32, length: u32, addr: u32) -> [u8; BLOCK_LEN] {
        block(OP_FIND, [7; 32], scope, length, addr, query_len)
    }

    #[test]
    fn find_copies_the_query_at_submit() {
        let mut rig = Rig::new();
        rig.write(DATA, "café".as_bytes());
        assert_eq!(rig.submit(BLOCK, Some(&find(1, 5, 1000, DATA))), (1, 5));
        rig.write(DATA, b"XXXXX");
        assert_eq!(rig.submit(BLOCK + 64, Some(&find(2, 1, 1, DATA))), (2, 1));
        assert_eq!(
            rig.content.take_requests(),
            [
                Request { handle: 1, kind: RequestKind::Find { scope: FIND_FEEDS, query: "café".into(), length: 1000 } },
                Request { handle: 2, kind: RequestKind::Find { scope: FIND_ENTRIES, query: b"X".to_vec(), length: 1 } },
            ]
        );
        // A find the relays refused (NETWORK 4.7) completes with status 05h;
        // one no relay could answer with 03h.
        rig.complete(&[done(1, Outcome::Refused), done(2, Outcome::Failed)]);
        assert_eq!(status(&rig, BLOCK), status::REFUSED);
        assert_eq!(status(&rig, BLOCK + 64), status::FAILED);
    }

    #[test]
    fn bad_find_requests() {
        let ram_end = RAM_BASE + RAM_LEN as u32;
        let mut rig = Rig::new();
        rig.write(DATA, &[b'q'; 64]);
        rig.write(DATA + 100, &[0xC3, 0x28]);
        rig.write(ram_end - 4, b"tail");
        for b in [
            find(0, 4, 100, DATA),                    // no scope
            find(4, 4, 100, DATA),                    // unknown scope
            find(0x101, 4, 100, DATA),                // only scope 1, 2 or 3, not low bits
            find(1, 0, 100, DATA),                    // empty query
            find(1, 49, 100, DATA),                   // query over 48 bytes
            find(1, 2, 100, DATA + 100),              // not UTF-8
            find(1, 4, 0, DATA),                      // empty buffer
            find(1, 4, MAX_GET_LENGTH as u32 + 1, DATA),
            find(1, 4, 5, ram_end - 4),               // buffer past the end of RAM
            find(1, 5, 5, ram_end - 4),               // query and buffer past the end
            find(1, 3, 4, ROM_BASE),                  // in ROM
            find(1, 1, 1, 0),
            find(1, 1, 1, u32::MAX),
        ] {
            assert_eq!(rig.submit(BLOCK, Some(&b)), (0, 0), "{b:02X?}");
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
            assert_eq!(rig.read(BLOCK + 2, 50), b[2..], "only the status is written");
        }
        assert!(rig.content.pending_handles().is_empty());
        // The boundaries: a 48-byte query, a buffer at the end of RAM, a
        // buffer shorter than the query (the query is read from the same
        // address), a 16,384-byte buffer.
        for b in [
            find(1, 48, 100, DATA),
            find(2, 4, 4, ram_end - 4),
            find(1, 48, 1, DATA),
            find(2, 1, MAX_GET_LENGTH as u32, ram_end - MAX_GET_LENGTH as u32),
        ] {
            assert_ne!(rig.submit(BLOCK, Some(&b)).0, 0, "{b:02X?}");
        }
        // A buffer shorter than a 4-byte query that ends at the end of RAM.
        assert_eq!(rig.submit(BLOCK, Some(&find(1, 4, 2, ram_end - 2))), (0, 0));
    }

    #[test]
    fn find_delivers_whole_timeline_records() {
        let (a, b) = (timeline_record(3, 1, 0xA1), timeline_record(120, 0, 0xB2));
        let ab = [a.clone(), b.clone()].concat();
        let mut rig = Rig::new();
        rig.write(DATA, b"query");
        rig.write(DATA + 400, &[0xEE]);
        rig.submit(BLOCK, Some(&find(2, 5, 400, DATA)));
        let mut bad_flags = a.clone();
        bad_flags[1] = 0x02;
        for (data, total, why) in [
            (&bad_flags[..], 1, "unknown flag"),
            (&a[..], 2, "fewer than fit"),
            (&ab, 1, "more records than results"),
            (&ab[..ab.len() - 1], 2, "cut short"),
            (&[ab.clone(), vec![0]].concat()[..], 2, "trailing byte"),
            (&ab, 17, "more than 16 results"),
        ] {
            assert_eq!(rig.complete(&[records(1, total, data)]), (1, 0), "{why}");
        }
        assert_eq!(rig.complete(&[got(1, 1, &[])]), (1, 0), "a get result does not complete a find");
        assert_eq!(rig.complete(&[records(1, 2, &ab)]), (0, 0));
        assert_eq!(rig.read(DATA, ab.len()), ab, "the results overwrite the query");
        assert_eq!(rig.read(DATA + 400, 1), [0xEE]);
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), (ab.len() as u32).to_le_bytes());
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [2, 0, 0, 0]);
        assert_eq!(status(&rig, BLOCK), status::DONE);

        // Sixteen results, of which only those that fit are delivered.
        rig.write(DATA, b"query");
        rig.submit(BLOCK, Some(&find(1, 5, 400, DATA)));
        assert_eq!(rig.complete(&[records(1, 16, &ab)]), (0, 0));
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [16, 0, 0, 0]);
        // No results; a buffer too small for any record; no relay asked.
        rig.write(DATA, b"query");
        for (length, total) in [(400, 0), (110, 3)] {
            assert_eq!(rig.submit(BLOCK, Some(&find(1, 5, length, DATA))).0, 1);
            assert_eq!(rig.complete(&[records(1, total, &[])]), (0, 0), "{length} {total}");
            assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [0, 0, 0, 0]);
        }
        assert_eq!(rig.submit(BLOCK, Some(&find(1, 5, 400, DATA))).0, 1);
        assert_eq!(rig.complete(&[records(1, 1, &[])]), (1, 0), "nothing delivered though a record fits");
        rig.complete(&[done(1, Outcome::Failed)]);
        assert_eq!(status(&rig, BLOCK), status::FAILED);
    }

    fn set_profile(name_len: u32, addr: u32, offset: u32) -> [u8; BLOCK_LEN] {
        block(OP_SET_PROFILE, [7; 32], offset, name_len, addr, 0)
    }

    fn set_nickname(key: [u8; 32], name_len: u32, addr: u32) -> [u8; BLOCK_LEN] {
        block(OP_SET_NICKNAME, key, 0, name_len, addr, 0)
    }

    #[test]
    fn set_profile_and_set_nickname() {
        let mut rig = Rig::new();
        rig.write(DATA, "Zoë Example".as_bytes());
        let n = "Zoë Example".len() as u32;
        assert_eq!(rig.submit(BLOCK, Some(&set_profile(n, DATA, 0xFFFF_FFFF))), (1, n as usize));
        assert_eq!(rig.submit(BLOCK + 64, Some(&set_profile(3, ROM_BASE, 2))), (2, 3));
        assert_eq!(rig.submit(BLOCK + 128, Some(&set_nickname([4; 32], 3, DATA))), (0, 0), "\"Zo\" and half of \"ë\"");
        assert_eq!(rig.submit(BLOCK + 128, Some(&set_nickname([4; 32], 4, DATA))), (3, 4));
        rig.write(DATA, b"XXXX");
        // An empty nickname removes it; it may start just past the end of RAM or ROM.
        assert_eq!(rig.submit(BLOCK + 192, Some(&set_nickname([5; 32], 0, RAM_BASE + RAM_LEN as u32))), (4, 0));
        assert_eq!(rig.submit(BLOCK + 256, Some(&set_nickname([6; 32], 0, ROM_BASE + ROM.len() as u32))), (5, 0));
        assert_eq!(rig.submit(BLOCK + 320, Some(&set_nickname([7; 32], 5, ROM_BASE + 4))), (6, 5));
        assert_eq!(
            rig.content.take_requests(),
            [
                Request { handle: 1, kind: RequestKind::SetProfile { name: "Zoë Example".into(), listed: true } },
                Request { handle: 2, kind: RequestKind::SetProfile { name: b"rom".to_vec(), listed: false } },
                Request { handle: 3, kind: RequestKind::SetNickname { feed: [4; 32], name: "Zoë".into() } },
                Request { handle: 4, kind: RequestKind::SetNickname { feed: [5; 32], name: Vec::new() } },
                Request { handle: 5, kind: RequestKind::SetNickname { feed: [6; 32], name: Vec::new() } },
                Request { handle: 6, kind: RequestKind::SetNickname { feed: [7; 32], name: b"bytes".to_vec() } },
            ]
        );
        // Outcomes that do not fit the request are ignored.
        assert_eq!(rig.complete(&[done(1, Outcome::Done)]), (1, 0));
        assert_eq!(rig.complete(&[done(3, Outcome::Published { seq: 1 })]), (1, 0));
        assert_eq!(rig.complete(&[records(4, 0, &[])]), (1, 0));
        assert_eq!(rig.complete(&[done(4, Outcome::Identity { key: [0; 32] })]), (1, 0));
        let (ignored, _) = rig.complete(&[
            done(1, Outcome::Published { seq: 0x0A0B_0C0D }),
            done(2, Outcome::Refused),
            done(3, Outcome::Done),
            done(4, Outcome::Refused),
            done(5, Outcome::NotFound),
            done(6, Outcome::Failed),
        ]);
        assert_eq!(ignored, 0);
        assert_eq!(rig.events(), [(5, 1, 1, 0), (5, 2, 5, 0), (5, 3, 1, 0), (5, 4, 5, 0), (5, 5, 2, 0), (5, 6, 3, 0)]);
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [0x0D, 0x0C, 0x0B, 0x0A], "size is the new seq");
        assert_eq!(rig.read(BLOCK + ID as u32, 32), [7; 32], "id untouched");
        assert_eq!(rig.read(BLOCK + 128 + 2, 50), set_nickname([4; 32], 4, DATA)[2..], "only the status written");
        assert!(rig.content.pending_handles().is_empty());
    }

    #[test]
    fn bad_set_profile_and_set_nickname_requests() {
        let ram_end = RAM_BASE + RAM_LEN as u32;
        let mut rig = Rig::new();
        rig.write(DATA, b" lead");
        rig.write(DATA + 16, b"trail ");
        rig.write(DATA + 32, b"a\x07b");
        rig.write(DATA + 48, &[0xC3, 0x28]);
        rig.write(DATA + 64, &[b'a'; 49]);
        rig.write(ram_end - 2, b"ok");
        let mut bad = vec![
            set_profile(0, DATA + 64, 1), // empty
            set_profile(0, ram_end, 1),
            set_profile(3, ram_end - 2, 0), // past the end of RAM
            set_profile(4, ROM_BASE + ROM.len() as u32 - 3, 0),
            set_profile(1, 0, 0),
            set_profile(1, u32::MAX, 0),
            set_profile(49, DATA + 64, 0),
            set_nickname([1; 32], 1, 0),
            set_nickname([1; 32], 49, DATA + 64),
            set_nickname([1; 32], 3, ram_end - 2),
        ];
        for (addr, len) in [(DATA, 5), (DATA + 16, 6), (DATA + 32, 3), (DATA + 48, 2)] {
            bad.push(set_profile(len, addr, 0));
            bad.push(set_nickname([1; 32], len, addr));
        }
        for b in &bad {
            assert_eq!(rig.submit(BLOCK, Some(b)), (0, 0), "{b:02X?}");
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
            assert_eq!(rig.read(BLOCK + 2, 50), b[2..], "only the status is written");
        }
        assert!(rig.content.pending_handles().is_empty());
        for b in [
            set_profile(48, DATA + 64, 0),
            set_profile(2, ram_end - 2, 0),
            set_nickname([1; 32], 48, DATA + 64),
            set_nickname([1; 32], 2, ram_end - 2),
            set_nickname([1; 32], 4, DATA + 1), // "lead"
            // SPEC 3.1: an empty name (removing a nickname) is valid at
            // any address.
            set_nickname([1; 32], 0, 0),
            set_nickname([1; 32], 0, ram_end + 1),
        ] {
            assert_ne!(rig.submit(BLOCK, Some(&b)).0, 0, "{b:02X?}");
        }
    }

    /// STATE.md 1.8: the pending records of names, find, set profile and
    /// set nickname.
    #[test]
    fn state_records_of_names_find_and_profiles() {
        let mut rig = Rig::new();
        rig.write(DATA, b"Ann");
        rig.submit(BLOCK, Some(&block(OP_NAMES, [3; 32], 5, 300, DATA, 77)));
        rig.submit(BLOCK + 64, Some(&find(2, 3, 900, DATA)));
        rig.submit(BLOCK + 128, Some(&set_profile(3, DATA, 0xFFFF_FFFF)));
        rig.submit(BLOCK + 192, Some(&set_profile(3, DATA, 2)));
        rig.submit(BLOCK + 256, Some(&block(OP_SET_NICKNAME, [9; 32], 1, 3, DATA, 8)));
        let mut out = Vec::new();
        rig.content.write_state(&mut out);
        assert_eq!(out.len(), 8 * 53);
        let rec = |i: usize| &out[i * 53..(i + 1) * 53];
        let want = |op: u8, block: u32, id: [u8; 32], fields: [u32; 4]| {
            let mut r = vec![op];
            r.extend_from_slice(&block.to_le_bytes());
            r.extend_from_slice(&id);
            for v in fields {
                r.extend_from_slice(&v.to_le_bytes());
            }
            r
        };
        assert_eq!(rec(0), want(OP_NAMES, BLOCK, [0; 32], [5, 300, DATA, 0]));
        assert_eq!(rec(1), want(OP_FIND, BLOCK + 64, [0; 32], [2, 900, DATA, 3]));
        assert_eq!(rec(2), want(OP_SET_PROFILE, BLOCK + 128, [0; 32], [1, 3, 0, 0]));
        assert_eq!(rec(3), want(OP_SET_PROFILE, BLOCK + 192, [0; 32], [0, 3, 0, 0]));
        assert_eq!(rec(4), want(OP_SET_NICKNAME, BLOCK + 256, [9; 32], [0, 3, 0, 0]));
        assert!(rec(5).iter().all(|&b| b == 0));
    }

    fn named(op: u8, name: &str, offset: u32, length: u32, addr: u32, size: u32) -> [u8; BLOCK_LEN] {
        let mut id = [0u8; 32];
        id[..name.len()].copy_from_slice(name.as_bytes());
        block(op, id, offset, length, addr, size)
    }

    #[test]
    fn record_names() {
        let id = |b: &[u8]| {
            let mut id = [0u8; 32];
            id[..b.len()].copy_from_slice(b);
            id
        };
        assert_eq!(record_name_len(&id(b"high-score")), Some(10));
        assert_eq!(record_name_len(&id("ünï code/x".as_bytes())), Some(12));
        assert_eq!(record_name_len(&[b'a'; 32]), Some(32));
        assert_eq!(record_name_len(&id(b"")), None, "empty");
        assert_eq!(record_name_len(&id(b"a\nb")), None, "a control character");
        assert_eq!(record_name_len(&id(&[0xC3, 0x28])), None, "not UTF-8");
        let mut gap = id(b"ab");
        gap[5] = b'c';
        assert_eq!(record_name_len(&gap), None, "not zero padded");
    }

    /// SPEC 9.6: save copies its data at submit (ROM or RAM, 0 to 65,536
    /// bytes); load, delete and records are submitted like get; bad names
    /// and ranges are bad requests.
    #[test]
    fn named_records_are_submitted() {
        let ram_end = RAM_BASE + RAM_LEN as u32;
        let mut rig = Rig::new();
        rig.write(DATA, b"0042");
        assert_eq!(rig.submit(BLOCK, Some(&named(OP_SAVE, "high-score", 0, 0, DATA, 4))), (1, 4));
        rig.write(DATA, b"XXXX");
        assert_eq!(rig.submit(BLOCK + 64, Some(&named(OP_SAVE, "empty", 0, 0, ram_end, 0))), (2, 0));
        assert_eq!(rig.submit(BLOCK + 128, Some(&named(OP_SAVE, "rom", 0, 0, ROM_BASE, 3))), (3, 3));
        assert_eq!(rig.submit(BLOCK + 192, Some(&named(OP_LOAD, "high-score", 2, 100, DATA + 16, 0))), (4, 0));
        assert_eq!(rig.submit(BLOCK + 256, Some(&named(OP_DELETE, "x", 9, 9, 9, 9))), (5, 0));
        assert_eq!(rig.submit(BLOCK + 320, Some(&named(OP_RECORDS, "", 1, 72, DATA + 200, 0))), (6, 0));
        assert_eq!(rig.submit(BLOCK + 384, Some(&named(OP_PROGRAMS, "", 0, 500, DATA + 300, 0))), (7, 0));
        assert_eq!(
            rig.content.take_requests(),
            [
                Request { handle: 1, kind: RequestKind::Save { name: b"high-score".to_vec(), data: b"0042".to_vec() } },
                Request { handle: 2, kind: RequestKind::Save { name: b"empty".to_vec(), data: Vec::new() } },
                Request { handle: 3, kind: RequestKind::Save { name: b"rom".to_vec(), data: b"rom".to_vec() } },
                Request { handle: 4, kind: RequestKind::Load { name: b"high-score".to_vec(), offset: 2, length: 100 } },
                Request { handle: 5, kind: RequestKind::Delete { name: b"x".to_vec() } },
                Request { handle: 6, kind: RequestKind::Records { offset: 1, length: 72 } },
                Request { handle: 7, kind: RequestKind::Programs { offset: 0, length: 500 } },
            ]
        );
        let mut rig = Rig::new();
        rig.write(DATA, &[0xC3, 0x28]);
        let mut bad_name = named(OP_LOAD, "ok", 0, 1, DATA, 0);
        bad_name[ID + 1] = 0x07;
        let mut gap = named(OP_DELETE, "ok", 0, 0, 0, 0);
        gap[ID + 5] = b'x';
        for b in [
            named(OP_SAVE, "", 0, 0, DATA, 1),
            named(OP_LOAD, "", 0, 1, DATA, 0),
            named(OP_DELETE, "", 0, 0, 0, 0),
            bad_name,
            gap,
            named(OP_SAVE, "k", 0, 0, DATA, MAX_RECORD as u32 + 1),
            named(OP_SAVE, "k", 0, 0, ram_end - 1, 2),
            named(OP_SAVE, "k", 0, 0, 0, 1),
            named(OP_LOAD, "k", 0, 0, DATA, 0),
            named(OP_LOAD, "k", 0, MAX_GET_LENGTH as u32 + 1, DATA, 0),
            named(OP_LOAD, "k", 0, 4, ROM_BASE, 0),
            named(OP_LOAD, "k", 0, 2, ram_end - 1, 0),
            named(OP_RECORDS, "", 0, 0, DATA, 0),
            named(OP_PROGRAMS, "", 0, 1, ROM_BASE, 0),
        ] {
            assert_eq!(rig.submit(BLOCK, Some(&b)), (0, 0), "{b:02X?}");
            assert_eq!(status(&rig, BLOCK), status::BAD_REQUEST);
            assert_eq!(rig.read(BLOCK + 2, 50), b[2..], "only the status is written");
        }
        // The longest record, a name of 32 bytes.
        let mut long = named(OP_SAVE, "k", 0, 0, RAM_BASE, MAX_RECORD as u32);
        long[ID..ID + 32].fill(b'n');
        let mut rig = Rig { ram: vec![0; MAX_RECORD + 64], ..Rig::new() };
        assert_eq!(rig.submit(RAM_BASE + MAX_RECORD as u32, Some(&long)), (1, MAX_RECORD));
        // SPEC 3.1: an empty record is valid at any address.
        for addr in [0, u32::MAX] {
            assert_eq!(rig.submit(RAM_BASE + MAX_RECORD as u32, Some(&named(OP_SAVE, "e", 0, 0, addr, 0))).1, 0);
        }
        assert_eq!(rig.content.pending_handles(), [1, 2, 3]);
    }

    #[test]
    fn named_records_complete() {
        let mut rig = Rig::new();
        rig.write(DATA, &[0xEE; 16]);
        rig.submit(BLOCK, Some(&named(OP_LOAD, "k", 2, 4, DATA, 0)));
        rig.submit(BLOCK + 64, Some(&named(OP_SAVE, "k", 0, 0, DATA, 1)));
        rig.submit(BLOCK + 128, Some(&named(OP_DELETE, "k", 0, 0, 0, 0)));
        rig.submit(BLOCK + 192, Some(&named(OP_RECORDS, "", 0, 72, DATA + 100, 0)));
        rig.submit(BLOCK + 256, Some(&named(OP_LOAD, "gone", 0, 4, DATA, 0)));
        rig.submit(BLOCK + 320, Some(&named(OP_SAVE, "big", 0, 0, DATA, 1)));
        // Answers that do not fit are ignored.
        for c in [
            got(1, 10, b"abc"),                    // 4 bytes from offset 2 of 10
            got(1, MAX_RECORD as u32 + 1, b"abcd"), // over the largest record
            done(1, Outcome::Done),
            done(2, Outcome::Got { size: 1, data: vec![1] }),
            done(3, Outcome::NoRoom),
            records(4, 3, &[0; 36]),                // two fit
            done(4, Outcome::Done),
        ] {
            assert_eq!(rig.complete(&[c.clone()]), (1, 0), "{c:?}");
        }
        let two = [[b'a'; 36], [b'b'; 36]].concat();
        let (ignored, _, delivered) = rig.content.complete(
            &[
                got(1, 4, b"cd"),
                done(2, Outcome::Done),
                done(3, Outcome::Done),
                records(4, 3, &two),
                done(5, Outcome::NotFound),
                done(6, Outcome::NoRoom),
            ],
            &mut rig.ram,
            &mut rig.system,
        );
        assert_eq!((ignored, delivered), (0, 2 + 72));
        assert_eq!(rig.events(), [(5, 1, 1, 0), (5, 2, 1, 0), (5, 3, 1, 0), (5, 4, 1, 0), (5, 5, 2, 0), (5, 6, 6, 0)]);
        assert_eq!(rig.read(DATA, 3), [b'c', b'd', 0xEE]);
        assert_eq!(rig.read(BLOCK + LENGTH as u32, 4), [2, 0, 0, 0]);
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [4, 0, 0, 0], "the record's size");
        assert_eq!(rig.read(DATA + 100, 72), two);
        assert_eq!(status(&rig, BLOCK + 320), status::NO_ROOM);
        // An empty record; a load at its end.
        rig.submit(BLOCK, Some(&named(OP_LOAD, "k", 0, 4, DATA, 0)));
        assert_eq!(rig.complete(&[got(1, 0, b"")]), (0, 0));
        assert_eq!(rig.read(BLOCK + SIZE as u32, 4), [0, 0, 0, 0]);
    }

    /// STATE.md 1.8: the pending records of save, load, delete, records
    /// and programs.
    #[test]
    fn state_records_of_named_records() {
        let mut rig = Rig::new();
        rig.write(DATA, b"0042");
        rig.submit(BLOCK, Some(&named(OP_SAVE, "high-score", 7, 7, DATA, 4)));
        rig.submit(BLOCK + 64, Some(&named(OP_LOAD, "k", 2, 100, DATA, 9)));
        rig.submit(BLOCK + 128, Some(&named(OP_DELETE, "k", 1, 2, 3, 4)));
        rig.submit(BLOCK + 192, Some(&named(OP_RECORDS, "zz", 1, 72, DATA, 5)));
        rig.submit(BLOCK + 256, Some(&named(OP_PROGRAMS, "zz", 2, 300, DATA, 5)));
        let mut out = Vec::new();
        rig.content.write_state(&mut out);
        let rec = |i: usize| &out[i * 53..(i + 1) * 53];
        let want = |op: u8, block: u32, name: &str, fields: [u32; 4]| {
            let mut id = [0u8; 32];
            id[..name.len()].copy_from_slice(name.as_bytes());
            let mut r = vec![op];
            r.extend_from_slice(&block.to_le_bytes());
            r.extend_from_slice(&id);
            for v in fields {
                r.extend_from_slice(&v.to_le_bytes());
            }
            r
        };
        assert_eq!(rec(0), want(OP_SAVE, BLOCK, "high-score", [0, 0, 0, 4]));
        assert_eq!(rec(1), want(OP_LOAD, BLOCK + 64, "k", [2, 100, DATA, 0]));
        assert_eq!(rec(2), want(OP_DELETE, BLOCK + 128, "k", [0, 0, 0, 0]));
        assert_eq!(rec(3), want(OP_RECORDS, BLOCK + 192, "", [1, 72, DATA, 0]));
        assert_eq!(rec(4), want(OP_PROGRAMS, BLOCK + 256, "", [2, 300, DATA, 0]));
    }
}
