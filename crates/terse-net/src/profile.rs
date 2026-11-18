//! Profiles, display names, nicknames and search matching (NETWORK 4.6,
//! 4.7).
//!
//! A profile entry (kind 2) names a feed: its title is the display name,
//! its blob the profile document:
//!
//! ```text
//! listed: yes
//! relay: ws://relay.example:7878
//!
//! Free text, the description.
//! ```
//!
//! Nicknames are the host's own names for feeds, kept in the config
//! directory's `nicknames` file (one `<feed key hex> <name>` per line) and
//! never sent anywhere.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use crate::blob::{parse_hex, to_hex};
use crate::identity::FeedKey;

/// Longest display name or nickname, in bytes.
pub const MAX_NAME: usize = 48;
/// Longest profile document, in bytes.
pub const MAX_DOCUMENT: usize = 4096;
/// Most `relay` lines a profile document's reader takes.
pub const MAX_PROFILE_RELAYS: usize = 4;
/// The nickname file in the config directory.
pub const NICKNAMES_FILE: &str = "nicknames";

/// A display name or nickname: 1 to 48 bytes of UTF-8, no control
/// characters (U+0000–U+001F, U+007F–U+009F), no leading or trailing
/// whitespace (the Unicode White_Space property).
pub fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && !name.chars().any(char::is_control)
        && !name.starts_with(char::is_whitespace)
        && !name.ends_with(char::is_whitespace)
}

/// A parsed profile document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// `listed: yes`: the name may be found by search.
    pub listed: bool,
    /// The `relay` lines that are relay URLs, at most four, in order.
    pub relays: Vec<String>,
    /// The text after the first empty line, if any.
    pub description: String,
}

impl Document {
    /// Parses a profile document; `None` if it is not one (not UTF-8, over
    /// 4,096 bytes, or a header line without `: `). Unknown fields are
    /// ignored; `relay` values that are not `ws://` or `wss://` URLs are
    /// skipped.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_DOCUMENT {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        let mut doc = Document::default();
        let mut rest = text;
        while !rest.is_empty() {
            // An empty line ends the header; the rest is the description.
            if let Some(description) = rest.strip_prefix('\n') {
                doc.description = description.to_string();
                break;
            }
            let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
            rest = after;
            let (field, value) = line.split_once(": ")?;
            match field {
                "listed" => doc.listed = value == "yes",
                "relay" if crate::local::valid_url(value) && doc.relays.len() < MAX_PROFILE_RELAYS => {
                    doc.relays.push(value.to_string());
                }
                _ => {}
            }
        }
        Some(doc)
    }

    /// The document's bytes: the `listed` line, the `relay` lines, then a
    /// blank line and the description if there is one. A description that
    /// does not fit in 4,096 bytes is cut at a character boundary.
    pub fn build(&self) -> Vec<u8> {
        let mut text = format!("listed: {}\n", if self.listed { "yes" } else { "no" });
        for relay in self.relays.iter().take(MAX_PROFILE_RELAYS) {
            text.push_str(&format!("relay: {relay}\n"));
        }
        if !self.description.is_empty() {
            text.push('\n');
            let room = MAX_DOCUMENT.saturating_sub(text.len());
            let mut cut = self.description.len().min(room);
            while !self.description.is_char_boundary(cut) {
                cut -= 1;
            }
            text.push_str(&self.description[..cut]);
        }
        text.into_bytes()
    }
}

/// ASCII letters A–Z folded to a–z; nothing else changes (NETWORK 4.7).
pub fn fold(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(u8::to_ascii_lowercase).collect()
}

/// Whether `text` contains `query`, both folded.
pub fn matches(text: &[u8], query: &[u8]) -> bool {
    let (text, query) = (fold(text), fold(query));
    !query.is_empty() && text.windows(query.len()).any(|w| w == query)
}

/// Whether `text` starts with `query`, both folded.
pub fn starts_with(text: &[u8], query: &[u8]) -> bool {
    fold(text).starts_with(&fold(query))
}

/// Whether an address is one a profile may send a host to: not loopback,
/// unspecified, private (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7)
/// or link-local (169.254/16, fe80::/10). IPv4-mapped IPv6 addresses are
/// judged as IPv4.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || a == 0
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_ip(IpAddr::V4(v4)),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback() || v6.is_unspecified() || first & 0xFE00 == 0xFC00 || first & 0xFFC0 == 0xFE80)
            }
        },
    }
}

/// The host part of a `ws://` or `wss://` URL (without brackets for IPv6).
pub fn url_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("ws://").or_else(|| url.strip_prefix("wss://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split_once(']').map(|(h, _)| h);
    }
    Some(authority.rsplit_once(':').map_or(authority, |(h, _)| h))
}

/// Whether a relay URL from a profile may be used: its host is not
/// `localhost` (or under `.localhost`) and, if an IP address, a public one.
/// A host name is checked again once resolved ([`public_ip`]).
pub fn usable_profile_relay(url: &str) -> bool {
    let Some(host) = url_host(url) else { return false };
    let lower = host.to_ascii_lowercase();
    if host.is_empty() || lower == "localhost" || lower.ends_with(".localhost") {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => public_ip(ip),
        Err(_) => true,
    }
}

/// Whether a URL names this machine (a loopback address or `localhost`):
/// such relays are left out of the relay lines of a profile the host
/// writes.
pub fn loopback_url(url: &str) -> bool {
    match url_host(url) {
        Some(host) => {
            let lower = host.to_ascii_lowercase();
            lower == "localhost" || lower.ends_with(".localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        }
        None => false,
    }
}

/// The host's nicknames for feeds, optionally saved to a file.
#[derive(Debug, Default)]
pub struct Nicknames {
    path: Option<PathBuf>,
    names: BTreeMap<FeedKey, String>,
}

impl Nicknames {
    pub fn in_memory() -> Self {
        Nicknames::default()
    }

    /// Loads `path` (empty if the file does not exist).
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut names = BTreeMap::new();
        for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
            let bad = || format!("{}:{}: not `<feed key> <nickname>`", path.display(), i + 1);
            let (key, name) = line.split_once(' ').ok_or_else(bad)?;
            let key = parse_hex(key).ok_or_else(bad)?;
            if !valid_name(name) {
                return Err(bad());
            }
            names.insert(key, name.to_string());
        }
        Ok(Nicknames { path: Some(path), names })
    }

    /// The file in a config directory.
    pub fn load_from_config(config: &Path) -> Result<Self, String> {
        Self::load(config.join(NICKNAMES_FILE))
    }

    pub fn get(&self, feed: &FeedKey) -> Option<&str> {
        self.names.get(feed).map(String::as_str)
    }

    /// Sets a nickname (`None` or an empty name removes it); false if it
    /// already was as asked. The name must be valid.
    pub fn set(&mut self, feed: FeedKey, name: Option<&str>) -> io::Result<bool> {
        let name = name.filter(|n| !n.is_empty());
        if self.get(&feed) == name {
            return Ok(false);
        }
        match name {
            Some(n) if !valid_name(n) => return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a valid nickname")),
            Some(n) => {
                self.names.insert(feed, n.to_string());
            }
            None => {
                self.names.remove(&feed);
            }
        }
        self.save()?;
        Ok(true)
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        crate::identity::private_dir(dir)?;
        let text: String = self.names.iter().map(|(k, n)| format!("{} {n}\n", to_hex(k))).collect();
        let temp = dir.join(format!(".nicknames.{}.tmp", std::process::id()));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("Ana"));
        assert!(valid_name("日本語 の 名前"));
        assert!(valid_name(&"é".repeat(24)));
        assert!(!valid_name(&"é".repeat(25)), "50 bytes");
        assert!(!valid_name(""));
        assert!(!valid_name(" Ana"));
        assert!(!valid_name("Ana\u{3000}"), "ideographic space is whitespace");
        assert!(!valid_name("A\u{7}na"));
        assert!(!valid_name("A\u{85}na"), "C1 control");
        assert!(valid_name("A na"));
    }

    #[test]
    fn documents() {
        let doc = Document::parse(b"listed: yes\nrelay: ws://a.example:1\nrelay: http://no\ncolour: blue\n\nHello.\n\nMore.").unwrap();
        assert_eq!(
            doc,
            Document { listed: true, relays: vec!["ws://a.example:1".into()], description: "Hello.\n\nMore.".into() }
        );
        assert_eq!(Document::parse(&doc.build()), Some(Document { ..doc.clone() }));
        assert_eq!(Document::parse(b"listed: no\n").unwrap(), Document::default());
        assert_eq!(Document::parse(b"listed: YES").unwrap().listed, false, "only `yes` lists");
        assert_eq!(Document::parse(b"\nJust text.").unwrap().description, "Just text.");
        assert_eq!(Document::parse(b"listed yes\n"), None, "a header line without `: `");
        assert_eq!(Document::parse(&[0xFF]), None);
        assert_eq!(Document::parse(&vec![b'\n'; 4097]), None);
        let many: String = (0..6).map(|i| format!("relay: ws://r{i}.example\n")).collect();
        assert_eq!(Document::parse(many.as_bytes()).unwrap().relays.len(), 4);
        let long = Document { description: "ü".repeat(3000), ..Default::default() };
        let built = long.build();
        assert!(built.len() <= MAX_DOCUMENT && Document::parse(&built).is_some());
    }

    #[test]
    fn matching() {
        assert!(matches(b"Ana Lopez", b"lop"));
        assert!(matches("ÄNA".as_bytes(), "Äna".as_bytes()), "bytes other than A-Z compare as they are");
        assert!(!matches("äna".as_bytes(), "Äna".as_bytes()), "no Unicode folding");
        assert!(starts_with(b"Ana", b"aN"));
        assert!(!matches(b"Ana", b""));
    }

    #[test]
    fn addresses() {
        for url in ["ws://relay.example:7878", "ws://8.8.8.8:1", "ws://[2001:db8::1]:7", "wss://x.example/path"] {
            assert!(usable_profile_relay(url), "{url}");
        }
        for url in [
            "ws://127.0.0.1:7878",
            "ws://localhost:1",
            "ws://a.localhost",
            "ws://10.1.2.3",
            "ws://172.20.0.1:5",
            "ws://192.168.1.1",
            "ws://169.254.0.1",
            "ws://100.64.0.1",
            "ws://0.0.0.0:1",
            "ws://[::1]:1",
            "ws://[fd00::1]",
            "ws://[fe80::1]",
            "ws://[::ffff:127.0.0.1]",
            "http://relay.example",
        ] {
            assert!(!usable_profile_relay(url), "{url}");
        }
        assert!(loopback_url("ws://127.0.0.1:7878") && loopback_url("ws://localhost:1") && !loopback_url("ws://10.0.0.1"));
    }

    #[test]
    fn nicknames_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut n = Nicknames::load_from_config(dir.path()).unwrap();
        assert!(n.set([1; 32], Some("Ana")).unwrap());
        assert!(!n.set([1; 32], Some("Ana")).unwrap());
        assert!(n.set([2; 32], Some("Bo b")).unwrap());
        assert!(n.set([2; 32], None).unwrap());
        assert!(!n.set([2; 32], Some("")).unwrap());
        assert!(n.set([3; 32], Some(" bad")).is_err());
        let again = Nicknames::load_from_config(dir.path()).unwrap();
        assert_eq!((again.get(&[1; 32]), again.get(&[2; 32])), (Some("Ana"), None));
        fs::write(dir.path().join(NICKNAMES_FILE), "zz Ana\n").unwrap();
        assert!(Nicknames::load_from_config(dir.path()).is_err());
    }
}
