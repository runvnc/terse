//! The order in which a host received feed entries, for the timeline of
//! SPEC §content.feeds, and the records the content device delivers for it.
//!
//! On disk, beside the entries: `<blob store>/feeds/arrivals`, an append-only
//! file of 36-byte records (feed key, `seq` little-endian), oldest first.
//! A record is appended when a node stores a new entry, in one `write` on a
//! file opened for appending, so processes sharing a store (say `terse feed
//! sync` and a running host) interleave whole records. The order is
//! therefore the same after a restart. Readers keep the first record for an
//! entry and ignore a torn record at the end.
//!
//! A relay, which drops entries (NETWORK 5.3), rewrites the log with only
//! the entries it still keeps ([`Compaction`]), a batch at a time: the
//! records are copied to `arrivals.compact` beside it, the copy is synced,
//! and it is renamed over the log. A crash before the rename leaves the log
//! as it was (the copy is removed at the next start); one after it leaves
//! the copy, of which at most the records copied after the sync may be
//! missing or torn, and a relay records every entry it holds that the log
//! lacks when it starts. A host never compacts: its log is also the
//! record of entries of feeds it unsubscribed from (NETWORK 4.4).

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
        // Windows: the log may be in use for a moment (`store::in_use`).
        crate::store::retry_in_use(|| fs::OpenOptions::new().create(true).append(true).open(&self.path))?.write_all(&record)
    }

    /// The log's file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The bytes of the log's file (0 if there is none).
    pub fn file_bytes(&self) -> u64 {
        fs::metadata(&self.path).map_or(0, |m| m.len())
    }

    /// Where a compaction writes its copy.
    pub fn compaction_path(&self) -> PathBuf {
        self.path.with_file_name("arrivals.compact")
    }

    /// Removes the copy a compaction stopped by a crash left (the log
    /// itself is whole: the copy replaces it only once complete).
    pub fn remove_leftover(&self) -> io::Result<()> {
        match fs::remove_file(self.compaction_path()) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Cuts a torn record off the end of the file (one a crash left), so
    /// that the records appended after it are whole; true if there was
    /// one. Only for a log no other process appends to (a relay's, when
    /// it starts).
    pub fn cut_torn(&mut self) -> io::Result<bool> {
        let len = self.file_bytes();
        let whole = len - len % RECORD as u64;
        if whole == len {
            return Ok(false);
        }
        let file = fs::OpenOptions::new().write(true).open(&self.path)?;
        file.set_len(whole)?;
        file.sync_all()?;
        Ok(true)
    }

    /// Starts rewriting the log with only the entries still kept.
    pub fn start_compaction(&mut self) -> io::Result<Compaction> {
        let before = self.order()?.len();
        let temp = self.compaction_path();
        if let Some(dir) = temp.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = fs::OpenOptions::new().create(true).write(true).truncate(true).open(&temp)?;
        Ok(Compaction { temp, file, cursor: 0, kept: Vec::new(), before, synced_at: None })
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

/// A rewriting of an [`Arrivals`] log that keeps only some of its records,
/// in their order, done a batch at a time ([`Self::copy`]) so that whoever
/// shares the log is not held up for long; the log takes records meanwhile.
pub struct Compaction {
    temp: PathBuf,
    file: fs::File,
    /// Records of the log looked at so far.
    cursor: usize,
    /// The records copied, in order.
    kept: Vec<(FeedKey, u32)>,
    /// Records in the log when it began.
    pub before: usize,
    /// The cursor when the copy was last synced.
    synced_at: Option<usize>,
}

impl Compaction {
    /// Looks at up to `max` records after the cursor (fewer if `stop` says
    /// so after one of them), copying those that `keep` says are kept.
    /// Returns how many it looked at, and whether the cursor is at the end
    /// of the log.
    pub fn copy(
        &mut self,
        log: &mut Arrivals,
        max: usize,
        mut keep: impl FnMut(&FeedKey, u32) -> bool,
        mut stop: impl FnMut() -> bool,
    ) -> io::Result<(usize, bool)> {
        let order = log.order()?;
        let mut bytes = Vec::new();
        let mut looked = 0;
        for &(feed, seq) in order.iter().skip(self.cursor).take(max) {
            if looked > 0 && stop() {
                break;
            }
            looked += 1;
            if keep(&feed, seq) {
                bytes.extend_from_slice(&feed);
                bytes.extend_from_slice(&seq.to_le_bytes());
                self.kept.push((feed, seq));
            }
        }
        self.file.write_all(&bytes)?;
        self.cursor += looked;
        Ok((looked, self.cursor == order.len()))
    }

    /// Whether every record up to the cursor is copied and synced.
    pub fn synced(&self) -> bool {
        self.synced_at == Some(self.cursor)
    }

    /// Records copied so far.
    pub fn kept(&self) -> usize {
        self.kept.len()
    }

    /// Makes the copy durable so far (an fsync: done without holding up
    /// the log's other users).
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()?;
        self.synced_at = Some(self.cursor);
        Ok(())
    }

    /// Replaces the log with the copy, which must have reached its end (a
    /// [`Self::copy`] that said so, with the log not touched since).
    /// Returns the copy's file and its directory, to be synced again with
    /// [`sync_finished`] (without holding up the log's other users), which
    /// makes the records copied since [`Self::sync`] durable too.
    pub fn finish(self, log: &mut Arrivals) -> io::Result<(fs::File, PathBuf)> {
        // Windows: renaming over a log something has open, unshared, fails
        // for a moment (`store::in_use`).
        crate::store::retry_in_use(|| fs::rename(&self.temp, &log.path))?;
        log.read = (self.kept.len() * RECORD) as u64;
        log.seen = self.kept.iter().copied().collect();
        log.order = self.kept;
        let dir = log.path.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok((self.file, dir))
    }

    /// Gives up: the log stays as it is and the copy is removed.
    pub fn abandon(self) {
        let _ = fs::remove_file(&self.temp);
    }
}

/// Syncs what [`Compaction::finish`] returned: the file, and the directory
/// that holds the rename.
pub fn sync_finished(file: &fs::File, dir: &Path) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// Whole records from `records`, appended while they fit in `length` bytes
/// (SPEC §content.feeds: as many whole records as fit).
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

/// A names record (SPEC §content.feeds): `L`, the flags (bit 0: a nickname; bit 1: a
/// profile is in force), the feed key and the name.
pub fn name_record(feed: &FeedKey, flags: u8, name: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(34 + name.len());
    r.push((33 + name.len()) as u8);
    r.push(flags);
    r.extend_from_slice(feed);
    r.extend_from_slice(name);
    r
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

    /// A compaction keeps the records `keep` allows, in order, takes the
    /// records appended while it runs, and replaces the file only at the
    /// end; stopped before that, the log is as it was.
    #[test]
    fn compaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Arrivals::beside_blobs(dir.path());
        for seq in 1..=10 {
            a.record(&[1; 32], seq).unwrap();
        }
        let path = dir.path().join("feeds/arrivals");
        let odd = |_: &FeedKey, seq: u32| seq % 2 == 1;
        // Stopped after copying and syncing: the log is untouched.
        let mut c = a.start_compaction().unwrap();
        assert_eq!(c.copy(&mut a, 100, odd, || false).unwrap(), (10, true));
        c.sync().unwrap();
        drop(c);
        assert_eq!(fs::metadata(&path).unwrap().len(), 10 * RECORD as u64);
        assert_eq!(Arrivals::beside_blobs(dir.path()).order().unwrap().len(), 10);
        assert!(a.compaction_path().exists());
        a.remove_leftover().unwrap();
        assert!(!a.compaction_path().exists());
        // In batches of 4, the second stopped after one record; records
        // appended meanwhile are taken.
        let mut c = a.start_compaction().unwrap();
        assert_eq!(c.before, 10);
        assert_eq!(c.copy(&mut a, 4, odd, || false).unwrap(), (4, false));
        assert_eq!(c.copy(&mut a, 4, odd, || true).unwrap(), (1, false));
        a.record(&[2; 32], 1).unwrap();
        assert_eq!(c.copy(&mut a, 100, odd, || false).unwrap(), (6, true));
        assert!(!c.synced());
        c.sync().unwrap();
        assert!(c.synced());
        assert_eq!(c.kept(), 6);
        let (file, dir_path) = c.finish(&mut a).unwrap();
        sync_finished(&file, &dir_path).unwrap();
        let kept = [([1; 32], 1), ([1; 32], 3), ([1; 32], 5), ([1; 32], 7), ([1; 32], 9), ([2; 32], 1)];
        assert_eq!(a.order().unwrap(), kept);
        assert_eq!(fs::metadata(&path).unwrap().len(), 6 * RECORD as u64);
        assert!(!a.compaction_path().exists());
        // Appending goes on in the new file; a restart reads the same.
        a.record(&[3; 32], 1).unwrap();
        assert_eq!(a.order().unwrap().len(), 7);
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
