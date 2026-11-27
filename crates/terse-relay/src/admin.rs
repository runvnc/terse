//! What the operator sees of a store before deciding (README "Running a
//! public relay", "Removing what does not belong"): the feeds by size or
//! by arrival, and one feed's entries. Read-only, from the files on disk,
//! so it needs no network and works beside a running relay.

use std::collections::HashMap;
use std::path::Path;

use terse_net::entry::{KIND_ARTICLE, KIND_DOCUMENT, KIND_PROFILE, KIND_PROGRAM};
use terse_net::timeline::Arrivals;
use terse_net::{BlobStore, FeedKey, FeedStore};

/// One feed held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedSummary {
    pub key: FeedKey,
    pub entries: usize,
    /// The sizes its entries claim, together.
    pub bytes: u64,
    /// The bytes of the blobs its entries name that are in the store.
    pub stored: u64,
    /// The name of its latest profile held, if any.
    pub name: Option<String>,
    /// Where its newest entry is in the order of arrival (larger is
    /// later); `None` if the arrival log does not have it.
    pub last_arrival: Option<usize>,
}

/// Which order [`feeds`] lists in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// The most bytes first.
    Size,
    /// The feed whose newest entry arrived last first.
    Arrival,
}

/// The feeds of the store in `dir`, in `order`.
pub fn feeds(dir: &Path, order: Order) -> Result<Vec<FeedSummary>, String> {
    let blobs = BlobStore::open(dir, None).map_err(|e| format!("{}: {e}", dir.display()))?;
    let store = FeedStore::beside_blobs(blobs.dir());
    let mut arrivals = Arrivals::beside_blobs(blobs.dir());
    let mut last: HashMap<FeedKey, usize> = HashMap::new();
    if let Ok(order) = arrivals.order() {
        for (i, (feed, _)) in order.iter().enumerate() {
            last.insert(*feed, i);
        }
    }
    let mut out = Vec::new();
    for key in store.feeds().map_err(|e| e.to_string())? {
        let entries = store.entries(&key).map_err(|e| e.to_string())?;
        if entries.is_empty() {
            continue;
        }
        let mut bytes = 0;
        let mut stored = 0;
        let mut name = None;
        for e in &entries {
            bytes += u64::from(e.entry.size);
            if std::fs::metadata(blobs.path(&e.entry.blob)).is_ok_and(|m| m.len() == u64::from(e.entry.size)) {
                stored += u64::from(e.entry.size);
            }
            if e.entry.kind == KIND_PROFILE {
                name = Some(e.entry.title.clone());
            }
        }
        out.push(FeedSummary { key, entries: entries.len(), bytes, stored, name, last_arrival: last.get(&key).copied() });
    }
    match order {
        Order::Size => out.sort_by(|a, b| b.stored.cmp(&a.stored).then(b.bytes.cmp(&a.bytes)).then(a.key.cmp(&b.key))),
        Order::Arrival => out.sort_by(|a, b| b.last_arrival.cmp(&a.last_arrival).then(a.key.cmp(&b.key))),
    }
    Ok(out)
}

/// One entry of a feed, for [`entries`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntrySummary {
    pub seq: u32,
    /// Unix seconds, as the author claims.
    pub time: u32,
    pub kind: &'static str,
    pub size: u32,
    pub blob: String,
    /// Whether the blob is in the store.
    pub stored: bool,
    pub title: String,
}

/// The entries of `feed` in the store in `dir`, by `seq`.
pub fn entries(dir: &Path, feed: &FeedKey) -> Result<Vec<EntrySummary>, String> {
    let blobs = BlobStore::open(dir, None).map_err(|e| format!("{}: {e}", dir.display()))?;
    let store = FeedStore::beside_blobs(blobs.dir());
    let held = store.entries(feed).map_err(|e| e.to_string())?;
    Ok(held
        .into_iter()
        .map(|e| {
            let e = e.entry;
            EntrySummary {
                seq: e.seq,
                time: e.time,
                kind: match e.kind {
                    KIND_ARTICLE => "article",
                    KIND_PROFILE => "profile",
                    KIND_PROGRAM => "program",
                    KIND_DOCUMENT => "document",
                    _ => "other",
                },
                size: e.size,
                blob: terse_net::blob::to_hex(&e.blob),
                stored: std::fs::metadata(blobs.path(&e.blob)).is_ok_and(|m| m.len() == u64::from(e.size)),
                title: e.title,
            }
        })
        .collect())
}

/// Text that may be printed on an operator's terminal: control
/// characters replaced (titles and names are anyone's).
pub fn printable(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { '\u{fffd}' } else { c }).collect()
}

/// `YYYY-MM-DD HH:MM` in UTC.
pub fn utc(time: u32) -> String {
    let days = (time / 86_400) as i64;
    let secs = time % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", secs / 3600, secs / 60 % 60)
}
