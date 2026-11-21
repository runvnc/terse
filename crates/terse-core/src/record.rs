//! Recordings of a single machine's run (STATE.md 5): every input of every
//! frame (SPEC §system, §content.determinism, §page.determinism), so that the run can be replayed exactly, with
//! no network, store or data directory, and its state hash checked after
//! every frame.
//!
//! A recording is a header (the image ID and the start: reset, or a
//! snapshot file) followed by one record per frame: the frame counter, the
//! Unix time, the four controllers as the machine latched them, the events
//! the host appended, the random bytes the program consumed, the content
//! completions with every field they deliver, and the state hash after the
//! frame.

use std::fmt;

use crate::content::{Completion, Outcome};
use crate::machine::{FrameInput, FrameResult};
use crate::system::{CONTROLLERS, Controller, Event};

/// `TREC`.
pub const MAGIC: [u8; 4] = *b"TREC";
pub const VERSION: u8 = 0;
/// Magic, version, image ID, snapshot length.
pub const HEADER_LEN: usize = 4 + 1 + 32 + 4;

/// The completion kinds of a frame record (STATE.md 5).
mod kind {
    pub const GOT: u8 = 0x01;
    pub const PUT: u8 = 0x02;
    pub const RECORDS: u8 = 0x03;
    pub const DONE: u8 = 0x04;
    pub const PUBLISHED: u8 = 0x05;
    pub const IDENTITY: u8 = 0x06;
    pub const NOT_FOUND: u8 = 0x07;
    pub const FAILED: u8 = 0x08;
    pub const REFUSED: u8 = 0x09;
    pub const NO_ROOM: u8 = 0x0A;
    pub const BAD_REQUEST: u8 = 0x0B;
    pub const IDENTITY_HINTS: u8 = 0x0C;
}

/// One frame's inputs, as the machine received them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// The frame counter as the frame starts.
    pub frame: u32,
    pub unix_time: u32,
    /// As the `controller` call writes them (all zero: not connected).
    pub controllers: [[u8; 8]; CONTROLLERS],
    /// The events the host appended, in order (before the machine drops
    /// any for a full queue).
    pub events: Vec<Event>,
    /// The random bytes the program consumed.
    pub random: Vec<u8>,
    /// The completions the host applied, in the order given.
    pub completions: Vec<Completion>,
}

impl Frame {
    /// The inputs of a frame just run: `frame` is the counter before it.
    pub fn of(frame: u32, input: &FrameInput, result: &FrameResult) -> Frame {
        Frame {
            frame,
            unix_time: input.unix_time,
            controllers: input.controllers.map(|c| c.bytes()),
            events: input.events.to_vec(),
            random: input.random[..result.random_used.min(input.random.len())].to_vec(),
            completions: input.completions.to_vec(),
        }
    }

    /// The controllers as a host passes them (the bytes say everything the
    /// machine sees).
    pub fn controllers(&self) -> [Controller; CONTROLLERS] {
        self.controllers.map(|b| {
            if b[0] == 0 {
                return Controller::default();
            }
            Controller {
                connected: true,
                buttons: u16::from_le_bytes([b[1], b[2]]),
                left: (b[3] as i8, b[4] as i8),
                right: (b[5] as i8, b[6] as i8),
            }
        })
    }

    /// The frame's input, with the recorded controllers (from
    /// [`Self::controllers`]).
    pub fn input<'a>(&'a self, controllers: &[Controller; CONTROLLERS]) -> FrameInput<'a> {
        FrameInput {
            events: &self.events,
            unix_time: self.unix_time,
            random: &self.random,
            completions: &self.completions,
            controllers: *controllers,
        }
    }

    /// The frame record, ending with `hash`, the state hash after the frame.
    pub fn encode(&self, hash: &[u8; 32]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.frame.to_le_bytes());
        out.extend_from_slice(&self.unix_time.to_le_bytes());
        for c in &self.controllers {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(&(self.events.len() as u32).to_le_bytes());
        for e in &self.events {
            out.extend_from_slice(&[e.kind, e.a, e.b, e.c]);
        }
        out.extend_from_slice(&(self.random.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.random);
        out.extend_from_slice(&(self.completions.len() as u32).to_le_bytes());
        for c in &self.completions {
            encode_completion(c, &mut out);
        }
        out.extend_from_slice(hash);
        out
    }
}

pub fn encode_completion(c: &Completion, out: &mut Vec<u8>) {
    out.push(c.handle);
    let bytes = |out: &mut Vec<u8>, k: u8, n: u32, data: &[u8]| {
        out.push(k);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    };
    match &c.outcome {
        Outcome::Got { size, data } => bytes(out, kind::GOT, *size, data),
        Outcome::Records { total, data } => bytes(out, kind::RECORDS, *total, data),
        Outcome::Put { id } => {
            out.push(kind::PUT);
            out.extend_from_slice(id);
        }
        Outcome::Identity { key } => {
            out.push(kind::IDENTITY);
            out.extend_from_slice(key);
        }
        Outcome::IdentityHints { key, hints } => {
            out.push(kind::IDENTITY_HINTS);
            out.extend_from_slice(key);
            out.extend_from_slice(&(hints.len() as u32).to_le_bytes());
            out.extend_from_slice(hints);
        }
        Outcome::Published { seq } => {
            out.push(kind::PUBLISHED);
            out.extend_from_slice(&seq.to_le_bytes());
        }
        Outcome::Done => out.push(kind::DONE),
        Outcome::NotFound => out.push(kind::NOT_FOUND),
        Outcome::Failed => out.push(kind::FAILED),
        Outcome::Refused => out.push(kind::REFUSED),
        Outcome::NoRoom => out.push(kind::NO_ROOM),
        Outcome::BadRequest => out.push(kind::BAD_REQUEST),
    }
}

/// The header: the image's ID, the snapshot file (STATE.md 4) the
/// machine starts from, if it does not start from reset, and the machine's
/// grants (none for an image without optional devices).
pub fn encode_header(image_id: &[u8; 32], snapshot: Option<&[u8]>, grants: &crate::image::Grants) -> Vec<u8> {
    let snapshot = snapshot.unwrap_or_default();
    let mut out = Vec::with_capacity(HEADER_LEN + snapshot.len() + grants.0.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.extend_from_slice(image_id);
    out.extend_from_slice(&(snapshot.len() as u32).to_le_bytes());
    out.extend_from_slice(snapshot);
    out.extend(grants.0.iter().map(|&g| g as u8));
    out
}

/// Why a recording cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    NotARecording,
    Version(u8),
    /// Cut short, or a field out of range, at this byte offset.
    Malformed(usize),
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::NotARecording => write!(f, "not a Terse32 recording"),
            RecordError::Version(v) => write!(f, "recording version {v} (this host reads version {VERSION})"),
            RecordError::Malformed(at) => write!(f, "malformed recording at byte {at}"),
        }
    }
}

impl std::error::Error for RecordError {}

/// A recording read from its bytes: the header, then frame by frame.
pub struct Recording<'a> {
    pub image_id: [u8; 32],
    /// The snapshot file the run starts from; `None`: from reset.
    pub snapshot: Option<&'a [u8]>,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Recording<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, RecordError> {
        if bytes.len() < 5 || bytes[..4] != MAGIC {
            return Err(RecordError::NotARecording);
        }
        if bytes[4] != VERSION {
            return Err(RecordError::Version(bytes[4]));
        }
        let mut r = Recording { image_id: [0; 32], snapshot: None, bytes, at: 5 };
        r.image_id = r.take(32)?.try_into().unwrap();
        let n = r.u32()? as usize;
        let snapshot = r.take(n)?;
        r.snapshot = (n > 0).then_some(snapshot);
        Ok(r)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], RecordError> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or(RecordError::Malformed(self.at))?;
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, RecordError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u8(&mut self) -> Result<u8, RecordError> {
        Ok(self.take(1)?[0])
    }

    fn id(&mut self) -> Result<[u8; 32], RecordError> {
        Ok(self.take(32)?.try_into().unwrap())
    }

    /// The grants of the recorded machine, `k` flags after the snapshot
    /// (STATE.md 5): `k` is the number of optional devices the image
    /// lists. Read before the first frame.
    pub fn grants(&mut self, k: usize) -> Result<crate::image::Grants, RecordError> {
        let mut out = Vec::with_capacity(k);
        for _ in 0..k {
            let at = self.at;
            out.push(match self.u8()? {
                0 => false,
                1 => true,
                _ => return Err(RecordError::Malformed(at)),
            });
        }
        Ok(crate::image::Grants(out))
    }

    /// The next frame and the state hash after it; `None` at the end.
    pub fn next_frame(&mut self) -> Option<Result<(Frame, [u8; 32]), RecordError>> {
        (self.at < self.bytes.len()).then(|| self.frame())
    }

    fn frame(&mut self) -> Result<(Frame, [u8; 32]), RecordError> {
        let mut f = Frame { frame: self.u32()?, unix_time: self.u32()?, ..Default::default() };
        for c in &mut f.controllers {
            let at = self.at;
            *c = self.take(8)?.try_into().unwrap();
            // Exactly what `controller` can write.
            let valid = match c[0] {
                0 => c.iter().all(|&b| b == 0),
                1 => c[7] == 0 && c[3..7].iter().all(|&b| b != 0x80),
                _ => false,
            };
            if !valid {
                return Err(RecordError::Malformed(at));
            }
        }
        let n = self.u32()? as usize;
        let events = self.take(n.checked_mul(4).ok_or(RecordError::Malformed(self.at))?)?;
        f.events = events.chunks(4).map(|e| Event { kind: e[0], a: e[1], b: e[2], c: e[3] }).collect();
        let n = self.u32()? as usize;
        if n > crate::machine::MAX_RANDOM_PER_FRAME {
            return Err(RecordError::Malformed(self.at - 4));
        }
        f.random = self.take(n)?.to_vec();
        let n = self.u32()?;
        for _ in 0..n {
            let handle = self.u8()?;
            let at = self.at;
            let outcome = match self.u8()? {
                kind::GOT | kind::RECORDS => {
                    let k = self.bytes[at];
                    let size = self.u32()?;
                    let len = self.u32()? as usize;
                    let data = self.take(len)?.to_vec();
                    if k == kind::GOT { Outcome::Got { size, data } } else { Outcome::Records { total: size, data } }
                }
                kind::PUT => Outcome::Put { id: self.id()? },
                kind::IDENTITY => Outcome::Identity { key: self.id()? },
                kind::IDENTITY_HINTS => {
                    let key = self.id()?;
                    let len = self.u32()? as usize;
                    Outcome::IdentityHints { key, hints: self.take(len)?.to_vec() }
                }
                kind::PUBLISHED => Outcome::Published { seq: self.u32()? },
                kind::DONE => Outcome::Done,
                kind::NOT_FOUND => Outcome::NotFound,
                kind::FAILED => Outcome::Failed,
                kind::REFUSED => Outcome::Refused,
                kind::NO_ROOM => Outcome::NoRoom,
                kind::BAD_REQUEST => Outcome::BadRequest,
                _ => return Err(RecordError::Malformed(at)),
            };
            f.completions.push(Completion { handle, outcome });
        }
        let hash = self.id()?;
        Ok((f, hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let frame = Frame {
            frame: 7,
            unix_time: 1_800_000_000,
            controllers: [[1, 0x13, 0x20, 0x81, 0x7F, 0, 5, 0], [0; 8], [0; 8], [0; 8]],
            events: vec![Event::key_down(4, 1), Event::feeds_updated()],
            random: vec![9, 8, 7],
            completions: [
                Outcome::Got { size: 10, data: b"abc".to_vec() },
                Outcome::Put { id: [3; 32] },
                Outcome::Records { total: 2, data: vec![1, 2] },
                Outcome::Done,
                Outcome::Published { seq: 4 },
                Outcome::Identity { key: [5; 32] },
                Outcome::IdentityHints { key: [7; 32], hints: b"relay.example".to_vec() },
                Outcome::NotFound,
                Outcome::Failed,
                Outcome::Refused,
                Outcome::NoRoom,
                Outcome::BadRequest,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, outcome)| Completion { handle: i as u8 + 1, outcome })
            .collect(),
        };
        let mut bytes = encode_header(&[1; 32], Some(b"snap"), &crate::image::Grants::default());
        bytes.extend(frame.encode(&[6; 32]));
        let mut r = Recording::parse(&bytes).unwrap();
        assert_eq!((r.image_id, r.snapshot), ([1; 32], Some(&b"snap"[..])));
        assert_eq!(r.grants(0), Ok(crate::image::Grants::default()));
        assert_eq!(r.next_frame(), Some(Ok((frame.clone(), [6; 32]))));
        assert_eq!(r.next_frame(), None);
        // The controllers come back as the machine saw them.
        assert_eq!(frame.controllers().map(|c| c.bytes()), frame.controllers);
        // Cut short anywhere in the frame: malformed.
        for cut in HEADER_LEN + 5..bytes.len() {
            let mut r = Recording::parse(&bytes[..cut]).unwrap();
            assert!(matches!(r.next_frame(), Some(Err(RecordError::Malformed(_)))), "cut at {cut}");
        }
        assert_eq!(Recording::parse(b"TSNP\0").err(), Some(RecordError::NotARecording));
        let mut v1 = bytes.clone();
        v1[4] = 1;
        assert_eq!(Recording::parse(&v1).err(), Some(RecordError::Version(1)));
        // The grants follow the snapshot: one flag per optional device.
        let grants = crate::image::Grants(vec![true, false]);
        let mut bytes = encode_header(&[1; 32], None, &grants);
        assert_eq!(&bytes[HEADER_LEN..], &[1, 0]);
        bytes.extend(frame.encode(&[6; 32]));
        let mut r = Recording::parse(&bytes).unwrap();
        assert_eq!(r.grants(2), Ok(grants));
        assert_eq!(r.next_frame(), Some(Ok((frame.clone(), [6; 32]))));
        bytes[HEADER_LEN] = 2;
        assert_eq!(Recording::parse(&bytes).unwrap().grants(2), Err(RecordError::Malformed(HEADER_LEN)));
    }
}
