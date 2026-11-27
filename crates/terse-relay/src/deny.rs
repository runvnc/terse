//! The deny list (`--deny <file>`): what this relay's operator keeps out,
//! by the relay's own policy (NETWORK 5.3). One item per line:
//!
//! ```text
//! feed <64 hex digits> [reason]      its entries are refused and removed
//! blob <64 hex digits> [reason]      never stored or served; entries naming it refused
//! address <addr>[/<bits>] [reason]   connections from it are refused
//! ```
//!
//! `#` starts a comment; blank lines are skipped. The reason is the rest of
//! the line, kept only here (the relay never sends it). The file is read
//! again when it changes or on SIGHUP; a file with a bad line is not taken
//! (the list in force stays). [`add`] and [`remove`] edit it safely (the
//! operator's `terse-relay deny` and `allow`): the new file is checked, then
//! written beside the old one and renamed over it.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use terse_net::FeedKey;
use terse_net::blob::{BlobId, parse_hex};
use terse_net::identity::valid_feed_key;

use crate::limits::Cidr;

/// One thing denied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Feed(FeedKey),
    Blob(BlobId),
    Address(Cidr),
}

impl Item {
    /// `feed <hex>`, `blob <hex>` or `address <range>`, as typed (the
    /// hex in either case).
    pub fn parse(kind: &str, value: &str) -> Result<Item, String> {
        let hex = || parse_hex(&value.to_ascii_lowercase());
        match kind {
            "feed" => hex().filter(valid_feed_key).map(Item::Feed).ok_or(format!("not a feed key: {value}")),
            "blob" => hex().map(Item::Blob).ok_or(format!("not a blob id (64 hex digits): {value}")),
            "address" => Cidr::parse(value).map(Item::Address).ok_or(format!("not an address or range: {value}")),
            _ => Err(format!("{kind}: not feed, blob or address")),
        }
    }

    /// The item as a line of the file writes it.
    pub fn text(&self) -> String {
        match self {
            Item::Feed(k) => format!("feed {}", terse_net::blob::to_hex(k)),
            Item::Blob(b) => format!("blob {}", terse_net::blob::to_hex(b)),
            Item::Address(c) => format!("address {c}"),
        }
    }
}

/// A line: the item and its reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub item: Item,
    pub reason: String,
}

/// Parses a deny list; the first bad line is an error.
pub fn parse(text: &str) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let mut words = line.splitn(3, char::is_whitespace);
        let Some(kind) = words.next().filter(|w| !w.is_empty()) else { continue };
        let value = words.next().unwrap_or_default().trim();
        let item = Item::parse(kind, value).map_err(|e| format!("line {}: {e}", i + 1))?;
        out.push(Line { item, reason: words.next().unwrap_or_default().trim().to_string() });
    }
    Ok(out)
}

/// The list in force and the file it came from.
pub struct DenyList {
    path: PathBuf,
    pub feeds: HashMap<FeedKey, String>,
    pub blobs: HashMap<BlobId, String>,
    pub addresses: Vec<(Cidr, String)>,
    seen: Option<(SystemTime, u64)>,
}

impl DenyList {
    /// Reads the file; one that does not exist is an empty list (so that
    /// `terse-relay deny` can create it).
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut d =
            DenyList { path: path.to_path_buf(), feeds: HashMap::new(), blobs: HashMap::new(), addresses: Vec::new(), seen: None };
        d.reload()?;
        Ok(d)
    }

    pub fn denies_feed(&self, key: &FeedKey) -> bool {
        self.feeds.contains_key(key)
    }

    pub fn denies_blob(&self, id: &BlobId) -> bool {
        self.blobs.contains_key(id)
    }

    pub fn denies_address(&self, ip: IpAddr) -> bool {
        self.addresses.iter().any(|(c, _)| c.contains(ip))
    }

    /// Feeds, blobs and address ranges listed.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.feeds.len(), self.blobs.len(), self.addresses.len())
    }

    fn stamp(&self) -> Option<(SystemTime, u64)> {
        let m = std::fs::metadata(&self.path).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    /// Reads the file again; on an error the list in force is kept.
    pub fn reload(&mut self) -> Result<(), String> {
        let stamp = self.stamp();
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let lines = parse(&text).map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.feeds.clear();
        self.blobs.clear();
        self.addresses.clear();
        for l in lines {
            match l.item {
                Item::Feed(k) => drop(self.feeds.insert(k, l.reason)),
                Item::Blob(b) => drop(self.blobs.insert(b, l.reason)),
                Item::Address(c) => self.addresses.push((c, l.reason)),
            }
        }
        self.seen = stamp;
        Ok(())
    }

    /// Reads the file again if it changed since it was read; `None` if it
    /// did not.
    pub fn reload_if_changed(&mut self) -> Option<Result<(), String>> {
        let stamp = self.stamp();
        if stamp == self.seen {
            return None;
        }
        let result = self.reload();
        // A bad file is not reported again until it changes.
        self.seen = stamp;
        Some(result)
    }
}

/// Writes `text` to `path` through a file beside it, renamed over it.
fn replace(path: &Path, text: &str) -> Result<(), String> {
    parse(text).map_err(|e| format!("{}: not written: {e}", path.display()))?;
    let temp = path.with_extension("new");
    std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("{}: {e}", path.display())
    })
}

fn read(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Adds a line denying `item` (with `reason`) unless the file has one;
/// returns whether it added it. The file must be valid as it is.
pub fn add(path: &Path, item: Item, reason: &str) -> Result<bool, String> {
    let text = read(path)?;
    let lines = parse(&text).map_err(|e| format!("{}: {e} (fix it first)", path.display()))?;
    if lines.iter().any(|l| l.item == item) {
        return Ok(false);
    }
    let reason: String = reason.chars().map(|c| if c.is_control() || c == '#' { ' ' } else { c }).collect();
    let mut text = text;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text += &item.text();
    if !reason.trim().is_empty() {
        text.push(' ');
        text += reason.trim();
    }
    text.push('\n');
    replace(path, &text)?;
    Ok(true)
}

/// Removes the lines denying `item`; returns how many. Comments and other
/// lines are kept as they are.
pub fn remove(path: &Path, item: Item) -> Result<usize, String> {
    let text = read(path)?;
    parse(&text).map_err(|e| format!("{}: {e} (fix it first)", path.display()))?;
    let mut removed = 0;
    let mut out = String::new();
    for line in text.lines() {
        if parse(line).is_ok_and(|l| l.first().is_some_and(|l| l.item == item)) {
            removed += 1;
            continue;
        }
        out += line;
        out.push('\n');
    }
    if removed > 0 {
        replace(path, &out)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use terse_net::Identity;
    use terse_net::blob::to_hex;

    #[test]
    fn parsing() {
        let a = Identity::from_seed([1; 32]).feed_key();
        let text = format!(
            "# deny\n\nfeed {}  spam, 2026-11-22\nblob {} # copyright notice\naddress 203.0.113.0/24 flood\naddress 2001:db8::1\n",
            to_hex(&a).to_uppercase(),
            "ab".repeat(32)
        );
        let lines = parse(&text).unwrap();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], Line { item: Item::Feed(a), reason: "spam, 2026-11-22".into() });
        assert_eq!(lines[1].reason, "");
        assert_eq!(lines[2].item, Item::Address(Cidr::parse("203.0.113.0/24").unwrap()));
        for bad in ["feed abc", &format!("feed {}", "00".repeat(32)), "blob xyz", "address 1.2.3.4/40", "key 00", "feed"] {
            assert!(parse(&format!("# ok\n{bad}\n")).unwrap_err().starts_with("line 2"), "{bad}");
        }
    }

    #[test]
    fn editing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deny");
        let a = Identity::from_seed([2; 32]).feed_key();
        assert!(add(&path, Item::Feed(a), "spam # not a comment\n").unwrap());
        assert!(!add(&path, Item::Feed(a), "again").unwrap());
        assert!(add(&path, Item::parse("address", "198.51.100.7").unwrap(), "").unwrap());
        let d = DenyList::load(&path).unwrap();
        assert!(d.denies_feed(&a) && d.denies_address("198.51.100.7".parse().unwrap()));
        assert_eq!(d.feeds[&a], "spam   not a comment");
        assert_eq!(remove(&path, Item::Feed(a)).unwrap(), 1);
        assert_eq!(remove(&path, Item::Feed(a)).unwrap(), 0);
        assert_eq!(DenyList::load(&path).unwrap().counts(), (0, 0, 1));
        // A bad file is not edited.
        std::fs::write(&path, "nonsense\n").unwrap();
        assert!(add(&path, Item::Feed(a), "").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nonsense\n");
        // A missing file is an empty list.
        assert_eq!(DenyList::load(&dir.path().join("none")).unwrap().counts(), (0, 0, 0));
    }
}
