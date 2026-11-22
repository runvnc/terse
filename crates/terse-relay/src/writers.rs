//! The write list (`--writers`, NETWORK 5.3): the feeds a relay takes
//! entries from. One feed key (64 hex digits) per line; `#` starts a
//! comment; blank lines are skipped; anything after the key and a blank is
//! a comment too (a name, say). The file is read again when it changes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use terse_net::FeedKey;
use terse_net::blob::parse_hex;
use terse_net::identity::valid_feed_key;

/// Parses a write list; the first bad line is an error.
pub fn parse(text: &str) -> Result<HashSet<FeedKey>, String> {
    let mut keys = HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some(word) = line.split_whitespace().next() else { continue };
        let key = parse_hex(&word.to_ascii_lowercase())
            .filter(valid_feed_key)
            .ok_or(format!("line {}: not a feed key: {word}", i + 1))?;
        keys.insert(key);
    }
    Ok(keys)
}

/// The list and the file it came from.
pub struct Writers {
    path: PathBuf,
    keys: HashSet<FeedKey>,
    /// The file's modification time and length when last read.
    seen: Option<(SystemTime, u64)>,
}

impl Writers {
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut w = Writers { path: path.to_path_buf(), keys: HashSet::new(), seen: None };
        w.reload()?;
        Ok(w)
    }

    pub fn contains(&self, key: &FeedKey) -> bool {
        self.keys.contains(key)
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn stamp(&self) -> Option<(SystemTime, u64)> {
        let m = std::fs::metadata(&self.path).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    /// Reads the file again; on an error the list in force is kept.
    pub fn reload(&mut self) -> Result<usize, String> {
        let stamp = self.stamp();
        let text = std::fs::read_to_string(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.keys = parse(&text).map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.seen = stamp;
        Ok(self.keys.len())
    }

    /// Reads the file again if it changed since it was read; `None` if it
    /// did not.
    pub fn reload_if_changed(&mut self) -> Option<Result<usize, String>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use terse_net::Identity;
    use terse_net::blob::to_hex;

    #[test]
    fn parsing() {
        let a = Identity::from_seed([1; 32]).feed_key();
        let b = Identity::from_seed([2; 32]).feed_key();
        let text = format!("# writers\n\n{}  ana\n  {} # bo\n", to_hex(&a), to_hex(&b).to_uppercase());
        assert_eq!(parse(&text).unwrap(), HashSet::from([a, b]));
        assert!(parse("abc\n").unwrap_err().starts_with("line 1"));
        // A key that is not a valid feed key (NETWORK 4.1) is refused.
        assert!(parse(&format!("\n{}\n", "00".repeat(32))).unwrap_err().starts_with("line 2"));
    }
}
