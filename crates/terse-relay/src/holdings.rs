//! What the relay holds for whom (NETWORK 5.3): the blob bytes each feed's
//! entries name (for the per-feed limit), the blobs some entry names
//! (held), and the **waiting room**: blobs no entry names, each kept until
//! a deadline and then removed.
//!
//! A blob is held for an entry only if its length is the entry's size
//! (NETWORK 4.4: a blob of another length is not associated with the
//! entry), so a feed is charged exactly the bytes it is held for.
//!
//! Nothing here is written to disk: the index is rebuilt at start from the
//! entries and blobs on disk, and a waiting blob's deadline from its file's
//! modification time (set when it starts waiting). So a relay stopped at
//! any point, even in the middle of a collection, starts again with an
//! index that matches its disk: a removed blob was unlinked (it cannot come
//! back), and a blob is unlinked only while the index says no entry names
//! it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use terse_net::FeedKey;
use terse_net::blob::BlobId;

use crate::limits::Group;

/// The least a waiting blob is charged: a disk block.
pub const MIN_CHARGE: u64 = 4096;

/// One feed's entries: `seq` to the blob and size it names.
#[derive(Default)]
pub struct Feed {
    pub entries: BTreeMap<u32, (BlobId, u32)>,
    /// The sum of the sizes.
    pub bytes: u64,
}

pub struct Wait {
    pub len: u32,
    /// Removed after this (ms).
    pub deadline: u64,
    /// What it counts against the room's limits, and whose upload it was.
    pub charge: u64,
    pub group: Option<Group>,
    /// Counted in the share of addresses not known as writers'.
    pub stranger: bool,
}

#[derive(Default)]
pub struct Holdings {
    pub feeds: HashMap<FeedKey, Feed>,
    /// For each blob some entry names: (size, how many entries).
    refs: HashMap<BlobId, Vec<(u32, u32)>>,
    pub waiting: HashMap<BlobId, Wait>,
    by_deadline: BTreeSet<(u64, BlobId)>,
    /// Charges of the waiting blobs: in all, of strangers, per address.
    pub waiting_bytes: u64,
    pub stranger_bytes: u64,
    group_bytes: HashMap<Group, u64>,
    /// Blobs being written by a put; not collected.
    pub writing: HashSet<BlobId>,
}

impl Holdings {
    /// Whether an entry held names `id` with size `len`.
    pub fn held(&self, id: &BlobId, len: u32) -> bool {
        self.refs.get(id).is_some_and(|v| v.iter().any(|&(s, n)| s == len && n > 0))
    }

    pub fn entries(&self) -> usize {
        self.feeds.values().map(|f| f.entries.len()).sum()
    }

    pub fn group_bytes(&self, g: &Group) -> u64 {
        self.group_bytes.get(g).copied().unwrap_or(0)
    }

    /// Records an entry; true if it now holds a blob that was waiting.
    pub fn add_entry(&mut self, feed: FeedKey, seq: u32, blob: BlobId, size: u32) -> bool {
        let f = self.feeds.entry(feed).or_default();
        if f.entries.insert(seq, (blob, size)).is_some() {
            return false;
        }
        f.bytes += u64::from(size);
        let v = self.refs.entry(blob).or_default();
        match v.iter_mut().find(|(s, _)| *s == size) {
            Some((_, n)) => *n += 1,
            None => v.push((size, 1)),
        }
        // A waiting blob of this length is held from now on.
        if self.waiting.get(&blob).is_some_and(|w| w.len == size) {
            self.unwait(&blob);
            return true;
        }
        false
    }

    /// Forgets an entry; returns the blob and size it named if no entry
    /// names that blob with that size any more.
    pub fn remove_entry(&mut self, feed: &FeedKey, seq: u32) -> Option<(BlobId, u32)> {
        let f = self.feeds.get_mut(feed)?;
        let (blob, size) = f.entries.remove(&seq)?;
        f.bytes -= u64::from(size);
        if f.entries.is_empty() {
            self.feeds.remove(feed);
        }
        let v = self.refs.get_mut(&blob)?;
        let i = v.iter().position(|(s, _)| *s == size)?;
        v[i].1 -= 1;
        if v[i].1 > 0 {
            return None;
        }
        v.swap_remove(i);
        if v.is_empty() {
            self.refs.remove(&blob);
        }
        Some((blob, size))
    }

    /// Puts a blob in the waiting room (or moves its deadline later).
    pub fn wait(&mut self, id: BlobId, w: Wait) {
        if let Some(old) = self.waiting.get(&id) {
            let deadline = old.deadline.max(w.deadline);
            return self.extend(&id, deadline);
        }
        self.waiting_bytes += w.charge;
        if w.stranger {
            self.stranger_bytes += w.charge;
        }
        if let Some(g) = w.group {
            *self.group_bytes.entry(g).or_default() += w.charge;
        }
        self.by_deadline.insert((w.deadline, id));
        self.waiting.insert(id, w);
    }

    /// Moves a waiting blob's deadline to `deadline` if that is later.
    pub fn extend(&mut self, id: &BlobId, deadline: u64) {
        let Some(w) = self.waiting.get_mut(id) else { return };
        if deadline > w.deadline {
            self.by_deadline.remove(&(w.deadline, *id));
            w.deadline = deadline;
            self.by_deadline.insert((deadline, *id));
        }
    }

    /// Takes a blob out of the waiting room (it is held, or removed).
    pub fn unwait(&mut self, id: &BlobId) -> Option<Wait> {
        let w = self.waiting.remove(id)?;
        self.by_deadline.remove(&(w.deadline, *id));
        self.waiting_bytes -= w.charge;
        if w.stranger {
            self.stranger_bytes -= w.charge;
        }
        if let Some(g) = w.group
            && let Some(b) = self.group_bytes.get_mut(&g)
        {
            *b -= w.charge;
            if *b == 0 {
                self.group_bytes.remove(&g);
            }
        }
        Some(w)
    }

    /// Waiting blobs whose deadline has passed, at most `max`, not those
    /// being written.
    pub fn expired(&self, now: u64, max: usize) -> Vec<BlobId> {
        self.by_deadline
            .iter()
            .take_while(|(d, _)| *d <= now)
            .filter(|(_, id)| !self.writing.contains(id))
            .take(max)
            .map(|(_, id)| *id)
            .collect()
    }

    /// The earliest deadline of the waiting room, or of one address's
    /// blobs in it.
    pub fn next_deadline(&self, group: Option<Group>) -> Option<u64> {
        match group {
            None => self.by_deadline.first().map(|(d, _)| *d),
            Some(g) => self.waiting.values().filter(|w| w.group == Some(g)).map(|w| w.deadline).min(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(len: u32, deadline: u64, group: Option<Group>) -> Wait {
        Wait { len, deadline, charge: u64::from(len).max(MIN_CHARGE), group, stranger: false }
    }

    #[test]
    fn references_by_size() {
        let mut h = Holdings::default();
        let (a, b) = ([1; 32], [2; 32]);
        let blob = [9; 32];
        h.wait(blob, wait(10, 100, Some(Group::V4([1, 2, 3, 4]))));
        assert_eq!((h.waiting_bytes, h.group_bytes(&Group::V4([1, 2, 3, 4]))), (MIN_CHARGE, MIN_CHARGE));
        // An entry with the wrong size does not hold it.
        assert!(!h.add_entry(a, 1, blob, 11));
        assert!(!h.held(&blob, 10) && h.held(&blob, 11));
        assert!(h.add_entry(b, 1, blob, 10), "it leaves the waiting room");
        assert!(h.waiting.is_empty() && h.waiting_bytes == 0 && h.group_bytes.is_empty());
        // Shared by two feeds: held until both entries are gone, counted
        // for each.
        assert!(!h.add_entry(a, 2, blob, 10));
        assert_eq!((h.feeds[&a].bytes, h.feeds[&b].bytes), (21, 10));
        assert_eq!(h.remove_entry(&b, 1), None);
        assert!(h.held(&blob, 10));
        assert_eq!(h.remove_entry(&a, 2), Some((blob, 10)));
        assert!(!h.held(&blob, 10));
        assert_eq!(h.remove_entry(&a, 1), Some((blob, 11)));
        assert!(h.feeds.is_empty());
        assert_eq!(h.remove_entry(&a, 1), None);
    }

    #[test]
    fn deadlines() {
        let mut h = Holdings::default();
        h.wait([1; 32], wait(1, 50, None));
        h.wait([2; 32], wait(1, 10, None));
        h.wait([3; 32], wait(1, 30, None));
        assert_eq!(h.expired(30, 10), [[2; 32], [3; 32]]);
        assert_eq!(h.expired(30, 1), [[2; 32]]);
        h.extend(&[2; 32], 60);
        h.wait([3; 32], wait(1, 5, None));
        assert_eq!(h.expired(30, 10), [[3; 32]], "an earlier deadline does not shorten the wait");
        h.writing.insert([3; 32]);
        assert!(h.expired(30, 10).is_empty());
        assert_eq!(h.next_deadline(None), Some(30));
        assert_eq!(h.unwait(&[3; 32]).unwrap().deadline, 30);
        assert_eq!(h.waiting_bytes, 2 * MIN_CHARGE);
    }
}
