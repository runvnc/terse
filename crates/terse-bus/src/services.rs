//! The bytes of the host services and of the consent properties
//! (DEVICES.md 7.2, 9), written and read by the core and the content
//! driver alike.

use terse_core::machines::{ProgramId, StartFailure, failure};
use terse_core::snapshot::{INFO_BYTES, SLOTS, SnapshotInfo, identity_from_record};
use terse_core::{Image, Snapshot};
use terse_net::{Asked, Prompt};

use crate::proto::{Reader, Short};

/// What a fetch gave (the first byte of `image.fetch`'s and
/// `program.fetch`'s answers).
pub mod fetched {
    pub const GOT: u8 = 1;
    pub const NOT_FOUND: u8 = 2;
    pub const FAILED: u8 = 3;
}

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s);
}

fn put_blob(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s);
}

/// `image.fetch`: the machine (tag) and the image ID.
pub fn fetch_image(tag: u32, id: &[u8; 32]) -> Vec<u8> {
    [&tag.to_le_bytes()[..], id].concat()
}

pub fn read_fetch_image(b: &[u8]) -> Result<(u32, [u8; 32]), Short> {
    let mut r = Reader::new(b);
    Ok((r.u32()?, r.take(32)?.try_into().unwrap()))
}

/// `program.fetch`: the machine, the program and the built-ins.
pub fn fetch_program(tag: u32, program: &ProgramId, builtins: &[[u8; 32]]) -> Vec<u8> {
    let mut out = tag.to_le_bytes().to_vec();
    out.extend_from_slice(&program.publisher);
    put_str(&mut out, &program.name);
    out.push(builtins.len() as u8);
    for b in builtins {
        out.extend_from_slice(b);
    }
    out
}

pub fn read_fetch_program(b: &[u8]) -> Result<(u32, ProgramId, Vec<[u8; 32]>), Short> {
    let mut r = Reader::new(b);
    let tag = r.u32()?;
    let publisher = r.take(32)?.try_into().unwrap();
    let name = r.str()?.to_vec();
    let n = r.u8()?;
    let builtins = (0..n).map(|_| r.take(32).map(|b| b.try_into().unwrap())).collect::<Result<_, _>>()?;
    Ok((tag, ProgramId { publisher, name }, builtins))
}

/// A fetch's answer as the image to start, or why not (SPEC 12.2: a
/// missing blob, or one that is not an image this host can run, fails the
/// start).
pub fn fetched_image(answer: &[u8]) -> Result<Image, StartFailure> {
    match answer.split_first() {
        Some((&fetched::GOT, data)) => terse_core::load_image(data),
        Some((&fetched::NOT_FOUND, _)) => Err(StartFailure::not_found("image not found")),
        _ => Err(StartFailure::not_found("image fetch failed")),
    }
}

/// An image's file again (an image is its header and ROM).
pub fn image_file(image: &Image) -> Vec<u8> {
    Image::build(&String::from_utf8_lossy(&image.title), image.ram_pages, &image.devices, &image.rom)
}

/// `snapshot.load`'s answer: 1, the image file, the snapshot file; or 0,
/// the failure's reason, its image ID if known, and the message.
pub fn restored(result: &Result<(Image, Snapshot), StartFailure>) -> Vec<u8> {
    let mut out = Vec::new();
    match result {
        Ok((image, snapshot)) => {
            out.push(1);
            put_blob(&mut out, &image_file(image));
            put_blob(&mut out, &snapshot.encode());
        }
        Err(f) => {
            out.push(0);
            out.push(f.reason);
            match f.image {
                Some(id) => {
                    out.push(1);
                    out.extend_from_slice(&id);
                }
                None => out.push(0),
            }
            put_str(&mut out, f.message.as_bytes());
        }
    }
    out
}

pub fn read_restored(b: &[u8]) -> Result<(Image, Snapshot), StartFailure> {
    let fail = |message: &str| StartFailure { reason: failure::SNAPSHOT, image: None, message: message.into() };
    let mut r = Reader::new(b);
    match r.u8().map_err(|_| fail("a short answer"))? {
        1 => {
            let file = r.blob().map_err(|_| fail("a short answer"))?;
            let image = terse_core::load_image(file)?;
            let snapshot = Snapshot::decode(r.blob().map_err(|_| fail("a short answer"))?).map_err(|e| fail(&e.to_string()))?;
            Ok((image, snapshot))
        }
        _ => {
            let mut read = || -> Result<StartFailure, Short> {
                let reason = r.u8()?;
                let image = match r.u8()? {
                    0 => None,
                    _ => Some(r.take(32)?.try_into().unwrap()),
                };
                Ok(StartFailure { reason, image, message: r.string()? })
            };
            Err(read().unwrap_or_else(|_| fail("a short answer")))
        }
    }
}

/// `snapshot.table`'s answer: each slot's `snapshot_info` record (SPEC
/// 12.2: all zero for an empty slot).
pub fn table(t: &[Option<SnapshotInfo>; SLOTS]) -> Vec<u8> {
    t.iter().flat_map(|i| i.as_ref().map_or([0; INFO_BYTES], SnapshotInfo::record)).collect()
}

pub fn read_table(b: &[u8]) -> [Option<SnapshotInfo>; SLOTS] {
    let mut out: [Option<SnapshotInfo>; SLOTS] = Default::default();
    for (slot, rec) in b.chunks_exact(INFO_BYTES).take(SLOTS).enumerate() {
        if rec[0] != 1 {
            continue;
        }
        let Ok(identity) = identity_from_record(rec[33..114].try_into().unwrap()) else { continue };
        let u = |at: usize| u32::from_le_bytes(rec[at..at + 4].try_into().unwrap());
        out[slot] = Some(SnapshotInfo { image: rec[1..33].try_into().unwrap(), identity, frames: u(114), time: u(118) });
    }
    out
}

/// The `content.prompt` property: the question shown (its machine's tag,
/// the request's handle, what is asked) and how many wait, itself
/// included; empty if none.
pub fn prompt(p: Option<(&Prompt, usize)>) -> Vec<u8> {
    let Some((p, waiting)) = p else { return Vec::new() };
    let mut out = p.tag.to_le_bytes().to_vec();
    out.push(p.handle);
    out.extend_from_slice(&(waiting as u32).to_le_bytes());
    match &p.asked {
        Asked::Subscribe { feed, hints } => {
            out.push(1);
            out.extend_from_slice(feed);
            put_str(&mut out, hints.as_bytes());
        }
        Asked::Unsubscribe(k) => {
            out.push(2);
            out.extend_from_slice(k);
        }
        Asked::Publish { title, size, document } => {
            out.push(3);
            put_str(&mut out, title.as_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.push(*document as u8);
        }
        Asked::SetProfile { name, listed } => {
            out.push(4);
            put_str(&mut out, name.as_bytes());
            out.push(*listed as u8);
        }
        Asked::SetNickname { feed, name } => {
            out.push(5);
            out.extend_from_slice(feed);
            put_str(&mut out, name.as_bytes());
        }
    }
    out
}

pub fn read_prompt(b: &[u8]) -> Option<(Prompt, usize)> {
    let mut r = Reader::new(b);
    let tag = r.u32().ok()?;
    let handle = r.u8().ok()?;
    let waiting = r.u32().ok()? as usize;
    let key = |r: &mut Reader| r.take(32).ok().map(|k| k.try_into().unwrap());
    let asked = match r.u8().ok()? {
        1 => Asked::Subscribe { feed: key(&mut r)?, hints: r.string().ok()? },
        2 => Asked::Unsubscribe(key(&mut r)?),
        3 => Asked::Publish { title: r.string().ok()?, size: r.u32().ok()?, document: r.u8().ok()? != 0 },
        4 => Asked::SetProfile { name: r.string().ok()?, listed: r.u8().ok()? != 0 },
        5 => Asked::SetNickname { feed: key(&mut r)?, name: r.string().ok()? },
        _ => return None,
    };
    Some((Prompt { tag, handle, asked }, waiting))
}

/// The `core.answer` property: the question's tag and handle, and 1 to
/// allow or 0 to refuse.
pub fn answer(tag: u32, handle: u8, allow: bool) -> Vec<u8> {
    let mut out = tag.to_le_bytes().to_vec();
    out.extend_from_slice(&[handle, allow as u8]);
    out
}

pub fn read_answer(b: &[u8]) -> Option<(u32, u8, bool)> {
    (b.len() == 6).then(|| (u32::from_le_bytes(b[..4].try_into().unwrap()), b[4], b[5] != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_and_tables_round_trip() {
        let p = Prompt { tag: 3, handle: 7, asked: Asked::Publish { title: "Hi".into(), size: 812, document: true } };
        let (q, n) = read_prompt(&prompt(Some((&p, 2)))).unwrap();
        assert_eq!((q, n), (p, 2));
        let p = Prompt { tag: 1, handle: 2, asked: Asked::Subscribe { feed: [9; 32], hints: "relay.example".into() } };
        assert_eq!(read_prompt(&prompt(Some((&p, 1)))).unwrap(), (p, 1));
        assert!(read_prompt(&prompt(None)).is_none());
        let mut t: [Option<SnapshotInfo>; SLOTS] = Default::default();
        t[3] = Some(SnapshotInfo { image: [5; 32], identity: Some(ProgramId { publisher: [0; 32], name: b"holo".to_vec() }), frames: 9, time: 77 });
        assert_eq!(read_table(&table(&t)), t);
        let image = Image::parse(&Image::build("t", 1, &[], &[0x6f, 0, 0, 0])).unwrap();
        assert_eq!(Image::parse(&image_file(&image)).unwrap().id, image.id);
    }
}
