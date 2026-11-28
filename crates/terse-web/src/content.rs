//! The content device's requests (SPEC §content) carried out in the page,
//! in memory: the blobs of the bundle (the manual, the programs' data) and
//! any the programs put, named records kept for this visit, and no
//! network. The page has no feed key and follows no feeds: the feed
//! lists are empty, and every request that would need consent (subscribe,
//! publish, a profile, a nickname) is refused, as a host refuses when
//! nobody can answer. Nothing leaves the page and nothing is kept after it.

use std::collections::{BTreeMap, HashMap};

use terse_core::content::{MAX_RECORD, RequestKind};
use terse_core::{Machine, Outcome, ProgramId, Request};

/// Most records one owner keeps, and their bytes (as terse-net's records).
const MAX_RECORDS: usize = 256;
const MAX_TOTAL: usize = 1_048_576;

/// Whose records: a program's identity, or an image with none (SPEC
/// §content.records).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Owner {
    Program(ProgramId),
    Image([u8; 32]),
}

impl Owner {
    fn of(machine: &Machine) -> Owner {
        match machine.identity() {
            Some(id) => Owner::Program(id.clone()),
            None => Owner::Image(*machine.image_id()),
        }
    }
}

#[derive(Default)]
pub struct Content {
    blobs: HashMap<[u8; 32], Vec<u8>>,
    records: HashMap<Owner, BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl Content {
    /// Stores `data`; returns its ID (BLAKE3-256, NETWORK.md).
    pub fn put(&mut self, data: Vec<u8>) -> [u8; 32] {
        let id = *blake3::hash(&data).as_bytes();
        self.blobs.entry(id).or_insert(data);
        id
    }

    pub fn get(&self, id: &[u8; 32]) -> Option<&[u8]> {
        self.blobs.get(id).map(Vec::as_slice)
    }

    /// The built-in program `name` among `builtins` (SPEC §machines.calls
    /// `start_program`: a built-in by its image title).
    pub fn find_builtin(&self, name: &[u8], builtins: &[[u8; 32]]) -> Option<[u8; 32]> {
        builtins.iter().copied().find(|id| {
            let Some(bytes) = self.get(id) else { return false };
            let title = bytes.get(16..32).unwrap_or_default();
            let len = title.iter().position(|&b| b == 0).unwrap_or(title.len());
            title[..len] == name[..]
        })
    }

    /// The answer to `request` of `machine`.
    pub fn answer(&mut self, machine: &Machine, request: Request) -> Outcome {
        match request.kind {
            RequestKind::Get { id, offset, length } => match self.get(&id) {
                Some(blob) => {
                    let start = (offset as usize).min(blob.len());
                    let end = (start + length as usize).min(blob.len());
                    Outcome::Got { size: blob.len() as u32, data: blob[start..end].to_vec() }
                }
                None => Outcome::NotFound,
            },
            RequestKind::Put { data } => Outcome::Put { id: self.put(data) },
            RequestKind::Subscriptions { .. }
            | RequestKind::Timeline { .. }
            | RequestKind::Names { .. }
            | RequestKind::Programs { .. } => Outcome::Records { total: 0, data: Vec::new() },
            // No feed key in the page, and no relay to search.
            RequestKind::Identity | RequestKind::Find { .. } | RequestKind::FindMore { .. } => Outcome::Failed,
            RequestKind::Subscribe { .. }
            | RequestKind::Unsubscribe { .. }
            | RequestKind::Publish { .. }
            | RequestKind::SetProfile { .. }
            | RequestKind::SetNickname { .. } => Outcome::Refused,
            RequestKind::Save { name, data } => {
                let records = self.records.entry(Owner::of(machine)).or_default();
                let old = records.get(&name).map_or(0, Vec::len);
                let count = records.len() + !records.contains_key(&name) as usize;
                let total = records.values().map(Vec::len).sum::<usize>() - old + data.len();
                if data.len() > MAX_RECORD || count > MAX_RECORDS || total > MAX_TOTAL {
                    return Outcome::NoRoom;
                }
                records.insert(name, data);
                Outcome::Done
            }
            RequestKind::Load { name, offset, length } => {
                match self.records.get(&Owner::of(machine)).and_then(|r| r.get(&name)) {
                    Some(data) => {
                        let start = (offset as usize).min(data.len());
                        let end = (start + length as usize).min(data.len());
                        Outcome::Got { size: data.len() as u32, data: data[start..end].to_vec() }
                    }
                    None => Outcome::NotFound,
                }
            }
            RequestKind::Delete { name } => {
                if let Some(r) = self.records.get_mut(&Owner::of(machine)) {
                    r.remove(&name);
                }
                Outcome::Done
            }
            RequestKind::Records { offset, length } => {
                let empty = BTreeMap::new();
                let records = self.records.get(&Owner::of(machine)).unwrap_or(&empty);
                // (name, size) records of 36 bytes, as many as fit.
                let mut data = Vec::new();
                for (name, value) in records.iter().skip(offset as usize) {
                    if data.len() + 36 > length as usize {
                        break;
                    }
                    let mut r = [0u8; 36];
                    r[..name.len()].copy_from_slice(name);
                    r[32..].copy_from_slice(&(value.len() as u32).to_le_bytes());
                    data.extend_from_slice(&r);
                }
                Outcome::Records { total: records.len() as u32, data }
            }
        }
    }
}
