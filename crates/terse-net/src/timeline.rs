//! The order in which a host received feed entries, for the timeline of
//! SPEC 7.7, and the records the content device delivers for it.
//!
//! On disk, beside the entries: `<blob store>/feeds/arrivals`, an append-only
//! file of 36-byte records (feed key, `seq` little-endian), oldest first.
//! A record is appended when a node stores a new entry, in one `write` on a
//! file opened for appending, so processes sharing a store (say `terse feed
//! sync` and a running host) interleave whole records. The order is
//! therefore the same after a restart. Readers keep the first record for an
//! entry and ignore a torn record at the end.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::identity::FeedKey;

const RECORD: usize = 36;

pub struct Arrivals {
    path: PathBuf,
    /// Oldest first, each entry once.
    order: Vec<(FeedKey, u32)>,
    seen: HashSet<(FeedKey, u32)>,
    /// Bytes of the file already read.
    read: u64,
}

impl Arrivals {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Arrivals { path: path.into(), order: Vec::new(), seen: HashSet::new(), read: 0 }
    }

    /// The log kept with a blob store in `blob_dir`.
    pub fn beside_blobs(blob_dir: &Path) -> Self {
        Self::open(blob_dir.join("feeds").join("arrivals"))
    }

    /// Records that a new entry was stored now.
    pub fn record(&mut self, feed: &FeedKey, seq: u32) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut record = [0u8; RECORD];
        record[..32].copy_from_slice(feed);
        record[32..].copy_from_slice(&seq.to_le_bytes());
        fs::OpenOptions::new().create(true).append(true).open(&self.path)?.write_all(&record)
    }

    /// Every recorded entry, oldest first, including records appended by
    /// other processes since the last call.
    pub fn order(&mut self) -> io::Result<&[(FeedKey, u32)]> {
        let mut file = match fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(&self.order),
            Err(e) => return Err(e),
        };
        file.seek(SeekFrom::Start(self.read))?;
        let mut fresh = Vec::new();
        file.read_to_end(&mut fresh)?;
        for record in fresh.chunks_exact(RECORD) {
            let key = (record[..32].try_into().unwrap(), u32::from_le_bytes(record[32..].try_into().unwrap()));
            if self.seen.insert(key) {
                self.order.push(key);
            }
        }
        self.read += (fresh.len() - fresh.len() % RECORD) as u64;
        Ok(&self.order)
    }
}

/// Whole records from `records`, appended while they fit in `length` bytes
/// (SPEC 7.7: as many whole records as fit).
pub fn fill(records: impl IntoIterator<Item = Vec<u8>>, length: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        if out.len() + r.len() > length {
            break;
        }
        out.extend_from_slice(&r);
    }
    out
}

/// A subscriptions record: the feed key and the highest `seq` held.
pub fn subscription_record(feed: &FeedKey, highest: u32) -> Vec<u8> {
    [&feed[..], &highest.to_le_bytes()].concat()
}

/// A timeline record: `L`, the flags (bit 0: the article is in the store)
/// and the entry without its signature.
pub fn timeline_record(unsigned_entry: &[u8], local: bool) -> Vec<u8> {
    let mut r = Vec::with_capacity(2 + unsigned_entry.len());
    r.push((1 + unsigned_entry.len()) as u8);
    r.push(local as u8);
    r.extend_from_slice(unsigned_entry);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_persists_and_follows_other_writers() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Arrivals::beside_blobs(dir.path());
        assert!(a.order().unwrap().is_empty());
        a.record(&[1; 32], 5).unwrap();
        a.record(&[2; 32], 1).unwrap();
        // Another process appends; a duplicate record is ignored.
        let mut b = Arrivals::beside_blobs(dir.path());
        b.record(&[1; 32], 6).unwrap();
        b.record(&[2; 32], 1).unwrap();
        assert_eq!(a.order().unwrap(), [([1; 32], 5), ([2; 32], 1), ([1; 32], 6)]);
        // A torn record at the end is not read until it is whole.
        let path = dir.path().join("feeds/arrivals");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&[3; 20]).unwrap();
        assert_eq!(a.order().unwrap().len(), 3);
        f.write_all(&[3; 12]).unwrap();
        f.write_all(&7u32.to_le_bytes()).unwrap();
        assert_eq!(a.order().unwrap()[3], ([3; 32], 7));
        // A restart reads the same order.
        assert_eq!(Arrivals::beside_blobs(dir.path()).order().unwrap(), a.order().unwrap());
    }

    #[test]
    fn records_and_filling() {
        assert_eq!(subscription_record(&[9; 32], 0x0102_0304)[32..], [4, 3, 2, 1]);
        let r = timeline_record(&[7; 109], true);
        assert_eq!((r.len(), r[0], r[1]), (111, 110, 1));
        let records = vec![vec![1; 10], vec![2; 10], vec![3; 10]];
        assert_eq!(fill(records.clone(), 25), [vec![1; 10], vec![2; 10]].concat());
        assert_eq!(fill(records.clone(), 9), Vec::<u8>::new());
        assert_eq!(fill(records, 30).len(), 30);
    }
}
