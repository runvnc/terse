//! Snapshots (STATE.md 4, SPEC 12.7): a machine's state serialization and
//! what is needed to recreate it, and restoring a machine from them.
//!
//! Restoring reads the `terse-state-1` serialization back
//! ([`crate::Machine::restore`]); a snapshot file adds the program's
//! identity and the time it was taken, stores RAM pages that are all zero
//! compactly and ends with the state hash.

use std::fmt;

use crate::machine::{Machine, STATE_DOMAIN};
use crate::machines::{IDENTITY_BYTES, MAX_PROGRAM_NAME, ProgramId};

/// Snapshot slots a shell has (SPEC 12.2 `snapshot`): 0 to 15.
pub const SLOTS: usize = 16;
/// The record `snapshot_info` writes (SPEC 12.2).
pub const INFO_BYTES: usize = 1 + 32 + IDENTITY_BYTES + 4 + 4;
/// A snapshot file starts with these bytes (STATE.md 4).
pub const MAGIC: &[u8; 4] = b"TSNP";
pub const VERSION: u8 = 0;
/// Bytes of a snapshot file before the serialization's first 200 bytes.
pub const FILE_HEAD: usize = 4 + 1 + IDENTITY_BYTES + 4 + 4;
/// Offsets in the serialization (STATE.md 1).
const IMAGE_AT: usize = STATE_DOMAIN.len();
const FRAME_AT: usize = 173;
/// The serialization up to and including the RAM size.
const BEFORE_RAM: usize = 200;
const PAGE: usize = 8192;
/// Bytes of the page map: one bit per RAM page, 128 pages at most.
const PAGE_MAP: usize = 16;
/// The longest serialization: header, CPU, frame loop, 1 MiB of RAM, a
/// full queue, every device at its largest, and an argument.
const MAX_STATE: usize =
    200 + 1_048_576 + crate::image::MAX_DEVICES + 37 + 4 * 64 + 10_379 + 8 + 16_384 + 425 + 21 + 65_536 + 1 + 4 + 16_384 + crate::sound::MAX_STATE + 32;

/// Why a snapshot or a serialization could not be restored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// The bytes are not a snapshot (or serialization) this implementation
    /// can read: cut short, a field out of range, a wrong hash.
    Malformed(&'static str),
    /// It is the state of another image.
    WrongImage,
    /// The machine had an optional device (STATE.md 1.4) this host does
    /// not provide (SPEC 12.2 `restore`, reason 3); a message for people.
    NeedsDevice(String),
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RestoreError::Malformed(why) => write!(f, "not a valid snapshot: {why}"),
            RestoreError::WrongImage => write!(f, "the snapshot is of another image"),
            RestoreError::NeedsDevice(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for RestoreError {}

pub fn bad(why: &'static str) -> RestoreError {
    RestoreError::Malformed(why)
}

/// The grants a serialization of a machine of `image` holds (STATE.md
/// 1.4): one flag per optional device of the image, after the RAM.
pub fn grants_of(image: &crate::Image, state: &[u8]) -> Result<crate::image::Grants, RestoreError> {
    let mut r = Reader::new(state);
    r.take(IMAGE_AT)?;
    if r.array::<32>()? != image.id {
        return Err(RestoreError::WrongImage);
    }
    r.take(BEFORE_RAM - IMAGE_AT - 32 - 4)?;
    let ram_len = r.u32()? as usize;
    r.take(ram_len)?;
    let k = image.optional().count();
    Ok(crate::image::Grants((0..k).map(|_| r.flag()).collect::<Result<_, _>>()?))
}

/// Reads a serialization field by field.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], RestoreError> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or(bad("cut short"))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], RestoreError> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8, RestoreError> {
        Ok(self.take(1)?[0])
    }

    pub fn flag(&mut self) -> Result<bool, RestoreError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(bad("a flag is not 0 or 1")),
        }
    }

    pub fn u16(&mut self) -> Result<u16, RestoreError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, RestoreError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    /// Bytes read so far.
    pub fn position(&self) -> usize {
        self.at
    }

    /// The bytes not read yet.
    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.at..]
    }
}

/// A program identity from the 81-byte record `identity` writes; a name
/// length of 0 (the record all zero) is no identity.
pub fn identity_from_record(r: &[u8; IDENTITY_BYTES]) -> Result<Option<ProgramId>, RestoreError> {
    let n = r[32] as usize;
    if n == 0 {
        return if r.iter().all(|&b| b == 0) { Ok(None) } else { Err(bad("identity")) };
    }
    if n > MAX_PROGRAM_NAME || !crate::content::valid_display_name(&r[33..33 + n]) || r[33 + n..].iter().any(|&b| b != 0) {
        return Err(bad("identity"));
    }
    Ok(Some(ProgramId { publisher: r[..32].try_into().unwrap(), name: r[33..33 + n].to_vec() }))
}

/// What `snapshot_info` tells of a snapshot (SPEC 12.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub image: [u8; 32],
    pub identity: Option<ProgramId>,
    /// The machine's frame counter when it was taken: frames run.
    pub frames: u32,
    /// The Unix time it was taken: the time latched for the machine's last
    /// frame.
    pub time: u32,
}

impl SnapshotInfo {
    /// The 122-byte record of `snapshot_info` for a slot holding this
    /// snapshot.
    pub fn record(&self) -> [u8; INFO_BYTES] {
        let mut out = [0; INFO_BYTES];
        out[0] = 1;
        out[1..33].copy_from_slice(&self.image);
        out[33..33 + IDENTITY_BYTES].copy_from_slice(&self.identity.as_ref().map_or([0; IDENTITY_BYTES], ProgramId::record));
        out[114..118].copy_from_slice(&self.frames.to_le_bytes());
        out[118..122].copy_from_slice(&self.time.to_le_bytes());
        out
    }

    /// The information in the first [`FILE_HEAD`] + 200 bytes of a
    /// snapshot file (the rest is not checked).
    pub fn from_file_head(file: &[u8]) -> Option<SnapshotInfo> {
        let head = file.get(..FILE_HEAD + BEFORE_RAM)?;
        if &head[..4] != MAGIC || head[4] != VERSION {
            return None;
        }
        let identity = identity_from_record(head[5..5 + IDENTITY_BYTES].try_into().unwrap()).ok()?;
        let time = u32::from_le_bytes(head[86..90].try_into().unwrap());
        let state = &head[FILE_HEAD..];
        (&state[..IMAGE_AT] == STATE_DOMAIN).then(|| SnapshotInfo {
            image: state[IMAGE_AT..IMAGE_AT + 32].try_into().unwrap(),
            identity,
            frames: u32::from_le_bytes(state[FRAME_AT..FRAME_AT + 4].try_into().unwrap()),
            time,
        })
    }
}

/// A machine's complete state and what is needed to recreate it: the
/// serialization (which holds the image ID and the argument), the
/// program's identity and when it was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The `terse-state-1` serialization (STATE.md 1).
    pub state: Vec<u8>,
    pub identity: Option<ProgramId>,
    pub time: u32,
}

impl Snapshot {
    /// The snapshot of `machine` now (between frames).
    pub fn of(machine: &Machine) -> Snapshot {
        Snapshot { state: machine.state(), identity: machine.identity().cloned(), time: machine.system().time() }
    }

    /// The snapshot of a machine with devices of any kind (its devices
    /// asked for their sections); an error is the devices'.
    pub fn try_of<D: crate::devices::Devices>(machine: &mut Machine<D>) -> Result<Snapshot, String> {
        Ok(Snapshot { state: machine.try_state()?, identity: machine.identity().cloned(), time: machine.system().time() })
    }

    pub fn image_id(&self) -> [u8; 32] {
        self.state[IMAGE_AT..IMAGE_AT + 32].try_into().unwrap()
    }

    /// The state hash at the moment it was taken.
    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.state).as_bytes()
    }

    pub fn info(&self) -> SnapshotInfo {
        SnapshotInfo {
            image: self.image_id(),
            identity: self.identity.clone(),
            frames: u32::from_le_bytes(self.state[FRAME_AT..FRAME_AT + 4].try_into().unwrap()),
            time: self.time,
        }
    }

    /// The machine it was taken of, with its identity: same state hash.
    pub fn restore(&self, image: &crate::Image) -> Result<Machine, RestoreError> {
        self.restore_where(image, |_| true)
    }

    /// The same on a host that provides the optional devices `provides`
    /// accepts ([`Machine::restore_where`]).
    pub fn restore_where(&self, image: &crate::Image, provides: impl Fn(&crate::DeviceReq) -> bool) -> Result<Machine, RestoreError> {
        let mut m = Machine::restore_where(image, &self.state, provides)?;
        m.set_identity(self.identity.clone());
        Ok(m)
    }

    /// The grants of the machine it was taken of (STATE.md 1.4).
    pub fn grants(&self, image: &crate::Image) -> Result<crate::image::Grants, RestoreError> {
        grants_of(image, &self.state)
    }

    /// The same with devices like `like`'s (a sibling of the machine whose
    /// devices they are: a shell's), as machine `serial`.
    pub fn restore_with<D: crate::devices::Devices>(&self, like: &D, serial: u32, image: &crate::Image) -> Result<Machine<D>, RestoreError> {
        if self.image_id() != image.id {
            return Err(RestoreError::WrongImage);
        }
        let grants = grants_of(image, &self.state)?;
        let mut m = like.new_machine(serial, image, Some(&grants), self.identity.as_ref()).map_err(|f| {
            if f.reason == crate::machines::failure::NEEDS_DEVICE { RestoreError::NeedsDevice(f.message) } else { bad("the devices could not be started") }
        })?;
        m.load_state(&self.state)?;
        m.set_identity(self.identity.clone());
        Ok(m)
    }

    /// The snapshot file (STATE.md 4).
    pub fn encode(&self) -> Vec<u8> {
        let s = &self.state;
        let ram_len = u32::from_le_bytes(s[196..200].try_into().unwrap()) as usize;
        let ram = &s[BEFORE_RAM..BEFORE_RAM + ram_len];
        let mut out = Vec::with_capacity(FILE_HEAD + s.len() + PAGE_MAP + 32);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.identity.as_ref().map_or([0; IDENTITY_BYTES], ProgramId::record));
        out.extend_from_slice(&self.time.to_le_bytes());
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(&s[..BEFORE_RAM]);
        let mut map = [0u8; PAGE_MAP];
        for (k, page) in ram.chunks(PAGE).enumerate() {
            if page.iter().any(|&b| b != 0) {
                map[k / 8] |= 1 << (k % 8);
            }
        }
        out.extend_from_slice(&map);
        for (k, page) in ram.chunks(PAGE).enumerate() {
            if map[k / 8] >> (k % 8) & 1 != 0 {
                out.extend_from_slice(page);
            }
        }
        out.extend_from_slice(&s[BEFORE_RAM + ram_len..]);
        out.extend_from_slice(&self.hash());
        out
    }

    /// Reads a snapshot file; checks its layout and the state hash, not
    /// yet whether the state fits an image (that is [`Snapshot::restore`]).
    pub fn decode(file: &[u8]) -> Result<Snapshot, RestoreError> {
        let mut r = Reader::new(file);
        if r.take(4)? != MAGIC {
            return Err(bad("not a snapshot file"));
        }
        if r.u8()? != VERSION {
            return Err(bad("unknown snapshot version"));
        }
        let identity = identity_from_record(&r.array()?)?;
        let time = r.u32()?;
        let n = r.u32()? as usize;
        if !(BEFORE_RAM..=MAX_STATE).contains(&n) {
            return Err(bad("state length"));
        }
        let mut state = Vec::with_capacity(n);
        state.extend_from_slice(r.take(BEFORE_RAM)?);
        let ram_len = u32::from_le_bytes(state[196..200].try_into().unwrap()) as usize;
        if ram_len == 0 || ram_len % PAGE != 0 || ram_len > 128 * PAGE || BEFORE_RAM + ram_len > n {
            return Err(bad("RAM size"));
        }
        let map: [u8; PAGE_MAP] = r.array()?;
        let pages = ram_len / PAGE;
        if (pages..PAGE_MAP * 8).any(|k| map[k / 8] >> (k % 8) & 1 != 0) {
            return Err(bad("page map"));
        }
        for k in 0..pages {
            if map[k / 8] >> (k % 8) & 1 != 0 {
                state.extend_from_slice(r.take(PAGE)?);
            } else {
                state.resize(state.len() + PAGE, 0);
            }
        }
        state.extend_from_slice(r.take(n - BEFORE_RAM - ram_len)?);
        let hash: [u8; 32] = r.array()?;
        if r.remaining() != 0 {
            return Err(bad("bytes after the hash"));
        }
        if &state[..IMAGE_AT] != STATE_DOMAIN {
            return Err(bad("not a terse-state-1 serialization"));
        }
        let snapshot = Snapshot { state, identity, time };
        if snapshot.hash() != hash {
            return Err(bad("the state hash does not match"));
        }
        Ok(snapshot)
    }
}
