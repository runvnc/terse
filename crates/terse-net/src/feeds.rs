//! Feed storage (NETWORK 4.2, 4.4), used by hosts and relays.
//!
//! On disk, next to the blobs (`<blob store>/feeds/`):
//!
//! ```text
//! feeds/<feed key hex>/<seq, 10 decimal digits>   one entry's exact bytes
//! ```
//!
//! Nothing is kept in memory, so several processes may share a store. An entry
//! is written to a temporary file and hard-linked to its name, which fails if
//! the name exists: the first entry stored for a `seq` wins (NETWORK 4.2),
//! even between processes. Entries are stored only after validation; readers
//! that do not trust the disk decode (and so verify) them again.
//!
//! A host's subscription list is a separate text file, one feed key in hex
//! per line (see [`Subscriptions`]); the hints a subscription was made with
//! (NETWORK 4.9) are in another, `subscription-hints` beside it, one
//! `<key hex>@<hint list>` per line, so that older versions still read the
//! list.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::blob::{parse_hex, to_hex};
use crate::entry::{self, SignedEntry};
use crate::identity::FeedKey;

/// What storing an entry did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insert {
    New,
    /// The same entry was already stored.
    Duplicate,
    /// A different entry with this `seq` was stored first; it is kept.
    Conflict,
}

pub struct FeedStore {
    dir: PathBuf,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn seq_name(seq: u32) -> String {
    format!("{seq:010}")
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

impl FeedStore {
    /// The store in `dir`; directories are created when the first entry is
    /// stored.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        FeedStore { dir: dir.into() }
    }

    /// The feed store kept with a blob store in `blob_dir`.
    pub fn beside_blobs(blob_dir: &Path) -> Self {
        Self::open(blob_dir.join("feeds"))
    }

    fn feed_dir(&self, feed: &FeedKey) -> PathBuf {
        self.dir.join(to_hex(feed))
    }

    /// Stores a validated entry unless one with its `seq` is already stored.
    pub fn insert(&self, e: &SignedEntry) -> io::Result<Insert> {
        let dir = self.feed_dir(&e.entry.feed);
        let path = dir.join(seq_name(e.entry.seq));
        if let Some(held) = read_optional(&path)? {
            return Ok(if held == e.bytes { Insert::Duplicate } else { Insert::Conflict });
        }
        fs::create_dir_all(&dir)?;
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(".{}.{}.{n}.tmp", e.entry.seq, std::process::id()));
        let linked = (|| {
            let mut f = fs::File::create(&temp)?;
            f.write_all(&e.bytes)?;
            f.sync_all()?;
            fs::hard_link(&temp, &path)
        })();
        let _ = fs::remove_file(&temp);
        match linked {
            Ok(()) => Ok(Insert::New),
            // Another writer stored this seq in the meantime.
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                let held = fs::read(&path)?;
                Ok(if held == e.bytes { Insert::Duplicate } else { Insert::Conflict })
            }
            Err(err) => Err(err),
        }
    }

    /// The stored bytes of one entry, not re-verified.
    pub fn get(&self, feed: &FeedKey, seq: u32) -> io::Result<Option<Vec<u8>>> {
        read_optional(&self.feed_dir(feed).join(seq_name(seq)))
    }

    /// The `seq` numbers stored for a feed, in increasing order.
    pub fn seqs(&self, feed: &FeedKey) -> io::Result<Vec<u32>> {
        let dir = match fs::read_dir(self.feed_dir(feed)) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut seqs = Vec::new();
        for item in dir {
            let name = item?.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.len() == 10
                && name.bytes().all(|b| b.is_ascii_digit())
                && let Ok(seq) = name.parse::<u32>()
            {
                seqs.push(seq);
            }
        }
        seqs.sort_unstable();
        Ok(seqs)
    }

    /// The highest `seq` stored for a feed, or 0.
    pub fn highest(&self, feed: &FeedKey) -> io::Result<u32> {
        Ok(self.seqs(feed)?.last().copied().unwrap_or(0))
    }

    /// The stored bytes of every entry with `seq` greater than `after`, in
    /// increasing order; not re-verified.
    pub fn after(&self, feed: &FeedKey, after: u32) -> io::Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for seq in self.seqs(feed)?.into_iter().filter(|&s| s > after) {
            // A file pruned meanwhile is skipped.
            out.extend(self.get(feed, seq)?);
        }
        Ok(out)
    }

    /// The valid entries stored for a feed, in increasing `seq` order.
    /// Files that fail validation are skipped.
    pub fn entries(&self, feed: &FeedKey) -> io::Result<Vec<SignedEntry>> {
        let held = self.after(feed, 0)?;
        Ok(held.iter().filter_map(|b| entry::decode(b).ok()).filter(|e| e.entry.feed == *feed).collect())
    }

    /// The feeds with a directory in the store.
    pub fn feeds(&self) -> io::Result<Vec<FeedKey>> {
        let dir = match fs::read_dir(&self.dir) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut feeds = Vec::new();
        for item in dir {
            if let Some(key) = item?.file_name().to_str().and_then(parse_hex) {
                feeds.push(key);
            }
        }
        feeds.sort_unstable();
        Ok(feeds)
    }

    /// Deletes all but the `keep` highest entries of a feed.
    pub fn prune(&self, feed: &FeedKey, keep: usize) -> io::Result<()> {
        self.prune_keeping(feed, keep, None)
    }

    /// Deletes all but the `keep` highest entries of a feed, and never the
    /// entry with `seq` `also` (a relay keeps the latest profile, NETWORK
    /// 4.6).
    pub fn prune_keeping(&self, feed: &FeedKey, keep: usize, also: Option<u32>) -> io::Result<()> {
        let seqs = self.seqs(feed)?;
        let excess = seqs.len().saturating_sub(keep);
        for seq in seqs[..excess].iter().filter(|&&s| Some(s) != also) {
            fs::remove_file(self.feed_dir(feed).join(seq_name(*seq)))?;
        }
        Ok(())
    }

    /// Deletes one entry; deleting one that is not stored succeeds.
    pub fn remove(&self, feed: &FeedKey, seq: u32) -> io::Result<()> {
        match fs::remove_file(self.feed_dir(feed).join(seq_name(seq))) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The profile in force for a feed (NETWORK 4.6): its valid kind-2
    /// entry with the highest `seq`.
    pub fn latest_profile(&self, feed: &FeedKey) -> io::Result<Option<SignedEntry>> {
        for seq in self.seqs(feed)?.into_iter().rev() {
            let Some(bytes) = self.get(feed, seq)? else { continue };
            if bytes.get(40) != Some(&entry::KIND_PROFILE) {
                continue;
            }
            if let Ok(e) = entry::decode(&bytes)
                && e.entry.feed == *feed
                && e.entry.seq == seq
            {
                return Ok(Some(e));
            }
        }
        Ok(None)
    }
}

/// A host's subscribed feeds (NETWORK 4.4), optionally saved to a file of
/// hex keys, one per line, rewritten atomically on every change, and the
/// hint list each was subscribed with, if any (NETWORK 4.9).
#[derive(Debug, Default)]
pub struct Subscriptions {
    path: Option<PathBuf>,
    feeds: BTreeSet<FeedKey>,
    /// Valid hint lists, as written; only for feeds in `feeds`.
    hints: BTreeMap<FeedKey, String>,
}

/// The file of hints beside the subscription list at `path`.
fn hints_path(path: &Path) -> PathBuf {
    path.with_file_name("subscription-hints")
}

/// Writes `text` to `path` atomically.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    crate::identity::private_dir(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut f = fs::File::create(&temp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

impl Subscriptions {
    /// An unsaved list.
    pub fn in_memory(feeds: impl IntoIterator<Item = FeedKey>) -> Self {
        Subscriptions { path: None, feeds: feeds.into_iter().collect(), hints: BTreeMap::new() }
    }

    /// Loads the list in `path` (empty if the file does not exist).
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut feeds = BTreeSet::new();
        for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let key = parse_hex(line.trim()).ok_or(format!("{}:{}: not a feed key", path.display(), i + 1))?;
            feeds.insert(key);
        }
        // Hints: lines that are not a subscribed key and a valid hint list
        // are skipped (the file is only an aid).
        let mut hints = BTreeMap::new();
        if let Ok(text) = fs::read_to_string(hints_path(&path)) {
            for line in text.lines() {
                let Some((key, list)) = line.trim().split_once('@') else { continue };
                if let Some(key) = parse_hex(key).filter(|k| feeds.contains(k))
                    && list.len() <= terse_core::hints::MAX_HINT_LIST
                    && terse_core::hints::parse_hints(list.as_bytes()).is_ok()
                {
                    hints.insert(key, list.to_string());
                }
            }
        }
        Ok(Subscriptions { path: Some(path), feeds, hints })
    }

    /// The hint list `feed` was subscribed with, if any.
    pub fn hints(&self, feed: &FeedKey) -> Option<&str> {
        self.hints.get(feed).map(String::as_str)
    }

    /// The feeds subscribed with hints.
    pub fn hinted(&self) -> impl Iterator<Item = (&FeedKey, &str)> {
        self.hints.iter().map(|(k, h)| (k, h.as_str()))
    }

    /// Remembers (or with `None` forgets) the hint list of a subscribed
    /// feed; `hints` must be a valid hint list. False if nothing changed.
    pub fn set_hints(&mut self, feed: &FeedKey, hints: Option<&str>) -> io::Result<bool> {
        if !self.feeds.contains(feed) {
            return Ok(false);
        }
        let changed = match hints {
            Some(h) => self.hints.insert(*feed, h.to_string()).as_deref() != Some(h),
            None => self.hints.remove(feed).is_some(),
        };
        if changed {
            self.save_hints()?;
        }
        Ok(changed)
    }

    pub fn feeds(&self) -> impl Iterator<Item = &FeedKey> {
        self.feeds.iter()
    }

    pub fn contains(&self, feed: &FeedKey) -> bool {
        self.feeds.contains(feed)
    }

    pub fn is_empty(&self) -> bool {
        self.feeds.is_empty()
    }

    /// Adds a feed; false if it was already in the list.
    pub fn add(&mut self, feed: FeedKey) -> io::Result<bool> {
        let added = self.feeds.insert(feed);
        if added {
            self.save()?;
        }
        Ok(added)
    }

    /// Removes a feed; false if it was not in the list.
    pub fn remove(&mut self, feed: &FeedKey) -> io::Result<bool> {
        let removed = self.feeds.remove(feed);
        if removed {
            self.save()?;
            if self.hints.remove(feed).is_some() {
                self.save_hints()?;
            }
        }
        Ok(removed)
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let text: String = self.feeds.iter().map(|k| format!("{}\n", to_hex(k))).collect();
        write_atomic(path, &text)
    }

    fn save_hints(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let text: String = self.hints.iter().map(|(k, h)| format!("{}@{h}\n", to_hex(k))).collect();
        write_atomic(&hints_path(path), &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Entry;
    use crate::identity::Identity;

    fn signed(id: &Identity, seq: u32, title: &str) -> SignedEntry {
        Entry::article(id.feed_key(), seq, 1000 + seq, title.as_bytes(), None, title).sign(id).unwrap()
    }

    #[test]
    fn first_received_wins_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::from_seed([3; 32]);
        let feed = id.feed_key();
        let store = FeedStore::beside_blobs(dir.path());
        assert_eq!(store.highest(&feed).unwrap(), 0);
        assert!(store.feeds().unwrap().is_empty());

        let (one, two, three) = (signed(&id, 1, "one"), signed(&id, 2, "two"), signed(&id, 3, "three"));
        assert_eq!(store.insert(&three).unwrap(), Insert::New);
        assert_eq!(store.insert(&one).unwrap(), Insert::New);
        assert_eq!(store.insert(&one).unwrap(), Insert::Duplicate);
        let rival = signed(&id, 1, "another one");
        assert_eq!(store.insert(&rival).unwrap(), Insert::Conflict);
        assert_eq!(store.get(&feed, 1).unwrap(), Some(one.bytes.clone()));
        assert_eq!(store.highest(&feed).unwrap(), 3);
        assert_eq!(store.insert(&two).unwrap(), Insert::New);

        // A new store on the same directory sees everything (a restart).
        let reopened = FeedStore::beside_blobs(dir.path());
        assert_eq!(reopened.feeds().unwrap(), [feed]);
        assert_eq!(reopened.seqs(&feed).unwrap(), [1, 2, 3]);
        assert_eq!(reopened.after(&feed, 1).unwrap(), [two.bytes.clone(), three.bytes.clone()]);
        assert_eq!(reopened.entries(&feed).unwrap(), [one.clone(), two.clone(), three.clone()]);

        // A damaged file is skipped by `entries`.
        fs::write(dir.path().join("feeds").join(to_hex(&feed)).join(seq_name(2)), b"junk").unwrap();
        assert_eq!(reopened.entries(&feed).unwrap(), [one, three.clone()]);

        reopened.prune(&feed, 1).unwrap();
        assert_eq!(reopened.seqs(&feed).unwrap(), [3]);
        assert_eq!(reopened.latest_profile(&feed).unwrap(), None);
        // No temporary files left behind.
        let names: Vec<_> = fs::read_dir(dir.path().join("feeds").join(to_hex(&feed))).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    /// The latest profile wins, and pruning can spare it.
    #[test]
    fn latest_profile_and_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::from_seed([4; 32]);
        let feed = id.feed_key();
        let store = FeedStore::beside_blobs(dir.path());
        let profile = |seq: u32, name: &str| Entry::profile(feed, seq, seq, b"listed: yes\n", name).sign(&id).unwrap();
        for e in [signed(&id, 1, "a"), profile(2, "Old"), profile(3, "New"), signed(&id, 4, "b"), signed(&id, 5, "c")] {
            store.insert(&e).unwrap();
        }
        assert_eq!(store.latest_profile(&feed).unwrap().unwrap().entry.title, "New");
        store.prune_keeping(&feed, 1, Some(3)).unwrap();
        assert_eq!(store.seqs(&feed).unwrap(), [3, 5]);
        assert_eq!(store.latest_profile(&feed).unwrap().unwrap().entry.seq, 3);
    }

    #[test]
    fn subscriptions_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/subscriptions");
        let mut subs = Subscriptions::load(&path).unwrap();
        assert!(subs.is_empty());
        assert!(subs.add([1; 32]).unwrap());
        assert!(subs.add([2; 32]).unwrap());
        assert!(!subs.add([1; 32]).unwrap());
        assert!(subs.remove(&[2; 32]).unwrap());
        assert!(!subs.remove(&[2; 32]).unwrap());
        let reloaded = Subscriptions::load(&path).unwrap();
        assert_eq!(reloaded.feeds().collect::<Vec<_>>(), [&[1; 32]]);
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{}\n", to_hex(&[1; 32])));
        // Hints are kept beside the list, for subscribed feeds only.
        assert!(!subs.set_hints(&[2; 32], Some("b.example")).unwrap());
        assert!(subs.set_hints(&[1; 32], Some("a.example,b.example:1")).unwrap());
        assert!(!subs.set_hints(&[1; 32], Some("a.example,b.example:1")).unwrap());
        let reloaded = Subscriptions::load(&path).unwrap();
        assert_eq!(reloaded.hints(&[1; 32]), Some("a.example,b.example:1"));
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{}\n", to_hex(&[1; 32])), "the list is unchanged");
        let hints = dir.path().join("config/subscription-hints");
        fs::write(&hints, format!("{}@a,b,c\n{}@ok.example\njunk\n", to_hex(&[1; 32]), to_hex(&[3; 32]))).unwrap();
        assert_eq!(Subscriptions::load(&path).unwrap().hinted().count(), 0, "bad and unsubscribed lines skipped");
        assert!(subs.set_hints(&[1; 32], Some("a.example")).unwrap());
        assert!(subs.remove(&[1; 32]).unwrap());
        assert_eq!(fs::read_to_string(&hints).unwrap(), "");
        fs::write(&path, "nonsense\n").unwrap();
        assert!(Subscriptions::load(&path).is_err());
    }
}
