//! Feed addresses (NETWORK 4.9): a feed key and up to two relay hints,
//! `<key>@<hint>[,<hint>]`, also written as the link
//! `terse:feed/<key>@<hint>[,<hint>]`. The hint grammar is in
//! [`terse_core::hints`]; this module reads whole addresses as people
//! type or paste them, turns hints into relay URLs and back, and works out
//! a host's own address.

use std::fmt;

use crate::blob::to_hex;
use crate::identity::{FeedKey, valid_feed_key};
use crate::profile;
pub use terse_core::hints::{DEFAULT_PORT, Hint, Host, MAX_HINT, MAX_HINT_LIST, MAX_HINTS, Malformed, TLS_PORT, parse_hint, parse_hints};

/// A feed key and the hints of its address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedAddress {
    pub key: FeedKey,
    /// The hint list as written (valid), or empty.
    pub hints: String,
}

impl FeedAddress {
    pub fn hint_list(&self) -> Vec<Hint> {
        parse_hints(self.hints.as_bytes()).unwrap_or_default()
    }
}

/// `<key>` or `<key>@<hints>`.
impl fmt::Display for FeedAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.hints.is_empty() { write!(f, "{}", to_hex(&self.key)) } else { write!(f, "{}@{}", to_hex(&self.key), self.hints) }
    }
}

/// Why typed text is not a feed address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// The key is not 64 lowercase hex digits.
    Key,
    /// 64 hex digits that are not a valid feed key (NETWORK 4.1).
    InvalidKey,
    /// An `@` with a malformed hint list after it.
    Hints(Malformed),
}

impl AddressError {
    /// A short name for test vectors.
    pub fn code(self) -> &'static str {
        match self {
            AddressError::Key => "bad-key",
            AddressError::InvalidKey => "invalid-key",
            AddressError::Hints(m) => m.code(),
        }
    }
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::Key => write!(f, "not a feed address: a feed key is 64 lowercase hex digits"),
            AddressError::InvalidKey => write!(f, "not a valid feed key (NETWORK.md 4.1)"),
            AddressError::Hints(m) => write!(f, "not a feed address: {m} after the @"),
        }
    }
}

/// The address in `text` exactly as NETWORK 4.9 writes it: 64 lowercase
/// hex digits, then optionally `@` and a valid hint list. The key need
/// not be a valid feed key (see [`parse_address`]).
pub fn parse_exact(text: &str) -> Result<FeedAddress, AddressError> {
    let (key, hints) = match text.split_once('@') {
        Some((k, h)) => (k, Some(h)),
        None => (text, None),
    };
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(AddressError::Key);
    }
    let key = crate::blob::parse_hex(key).ok_or(AddressError::Key)?;
    let hints = match hints {
        None => String::new(),
        Some(h) if h.len() > MAX_HINT_LIST => {
            // Report the more telling reason when there is one.
            return Err(AddressError::Hints(parse_hints(h.as_bytes()).err().unwrap_or(Malformed::TooLong)));
        }
        Some(h) => {
            parse_hints(h.as_bytes()).map_err(AddressError::Hints)?;
            h.to_string()
        }
    };
    Ok(FeedAddress { key, hints })
}

/// An address as a person types or pastes it: blanks at either end are
/// ignored, and the link form `terse:feed/<address>` is taken too. The key
/// must be a valid feed key. A malformed hint list makes the whole address
/// invalid (in an article, by contrast, it only ends the link: ARTICLE.md
/// 4.1).
pub fn parse_address(text: &str) -> Result<FeedAddress, AddressError> {
    let t = text.trim();
    let t = t.strip_prefix("terse:feed/").unwrap_or(t);
    let a = parse_exact(t)?;
    if !valid_feed_key(&a.key) {
        return Err(AddressError::InvalidKey);
    }
    Ok(a)
}

/// The hint for a relay URL, if it can be written as one (NETWORK 4.9): a
/// host a hint can name and no path but `/`, with `ws://` and a port that
/// is not 443 (7878 when none is given), or `wss://` and port 443 (given or
/// not).
pub fn hint_of_url(url: &str) -> Option<Hint> {
    let (tls, rest) = match (url.strip_prefix("ws://"), url.strip_prefix("wss://")) {
        (Some(r), _) => (false, r),
        (_, Some(r)) => (true, r),
        _ => return None,
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    let mut hint = parse_hint(rest.as_bytes()).ok()?;
    // A URL without a port means 80 or 443 (NETWORK 3), not the hint's 7878.
    let port_given = rest.rsplit_once(']').map_or(rest, |(_, p)| p).contains(':');
    if !port_given {
        hint.port = if tls { TLS_PORT } else { 80 };
    }
    (tls == (hint.port == TLS_PORT)).then_some(hint)
}

/// The hints a host gives for its own address (NETWORK 4.9): from the
/// relay lines of its profile in force, then its configured relays, those
/// that can be written as hints and are not loopback, private or link-local
/// addresses (NETWORK 5.1), without repeats, at most two.
pub fn own_hints(profile_relays: &[String], configured: &[String]) -> Vec<Hint> {
    let mut out: Vec<Hint> = Vec::new();
    for url in profile_relays.iter().chain(configured) {
        if out.len() == MAX_HINTS {
            break;
        }
        if !profile::usable_profile_relay(url) {
            continue;
        }
        if let Some(h) = hint_of_url(url).filter(|h| !out.iter().any(|o| same_relay(o, h))) {
            out.push(h);
        }
    }
    out
}

/// The hint list of `hints`, in their shortest forms.
pub fn hint_text(hints: &[Hint]) -> String {
    hints.iter().map(Hint::to_string).collect::<Vec<_>>().join(",")
}

/// Whether two hints name the same relay (names compared without regard
/// to case).
pub fn same_relay(a: &Hint, b: &Hint) -> bool {
    a.port == b.port
        && match (&a.host, &b.host) {
            (Host::Name(x), Host::Name(y)) => x.eq_ignore_ascii_case(y),
            (x, y) => x == y,
        }
}

/// Whether a relay URL names the relay of `hint`.
pub fn url_is_hint(url: &str, hint: &Hint) -> bool {
    hint_of_url(url).is_some_and(|h| same_relay(&h, hint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn typed_addresses() {
        let key = Identity::from_seed([1; 32]).feed_key();
        let hex = to_hex(&key);
        for text in [hex.clone(), format!("  {hex}\n"), format!("terse:feed/{hex}")] {
            assert_eq!(parse_address(&text), Ok(FeedAddress { key, hints: String::new() }), "{text}");
        }
        let a = parse_address(&format!(" terse:feed/{hex}@relay.example,[::1]:9 ")).unwrap();
        assert_eq!(a.hints, "relay.example,[::1]:9");
        assert_eq!(a.to_string(), format!("{hex}@relay.example,[::1]:9"));
        assert_eq!(a.hint_list().len(), 2);
        assert_eq!(parse_address(&hex.to_uppercase()), Err(AddressError::Key));
        assert_eq!(parse_address(&hex[..63]), Err(AddressError::Key));
        assert_eq!(parse_address(&format!("{hex}@")), Err(AddressError::Hints(Malformed::Empty)));
        assert_eq!(parse_address(&format!("{hex}@a,b,c")), Err(AddressError::Hints(Malformed::TooMany)));
        assert_eq!(parse_address(&format!("{hex}@a.example.")), Err(AddressError::Hints(Malformed::Name)));
        assert_eq!(parse_address(&format!("{hex} @a.example")), Err(AddressError::Key));
        assert_eq!(parse_address(&"0".repeat(64)), Err(AddressError::InvalidKey));
        assert_eq!(parse_exact(&"0".repeat(64)).unwrap().key, [0; 32]);
    }

    #[test]
    fn urls_and_own_hints() {
        assert_eq!(hint_of_url("ws://relay.example:7878/").unwrap().to_string(), "relay.example");
        // A URL without a port is port 80 (NETWORK 3), not the hint default.
        assert_eq!(hint_of_url("ws://relay.example").unwrap().to_string(), "relay.example:80");
        assert_eq!(hint_of_url("ws://[2001:db8::1]:9").unwrap().to_string(), "[2001:db8::1]:9");
        // Over TLS: port 443, given or not.
        assert_eq!(hint_of_url("wss://relay.example").unwrap().to_string(), "relay.example:443");
        assert_eq!(hint_of_url("wss://relay.example:443/").unwrap().url(), "wss://relay.example");
        assert_eq!(hint_of_url("wss://[2001:db8::1]").unwrap().to_string(), "[2001:db8::1]:443");
        let urls = ["wss://relay.example:8443", "ws://relay.example:443", "ws://relay.example/x", "wss://a/x", "ws://", "wss://"];
        for url in urls.into_iter().chain(["http://relay.example", "ws://a_b"]) {
            assert!(hint_of_url(url).is_none(), "{url}");
        }
        let s = |v: &[&str]| v.iter().map(|u| u.to_string()).collect::<Vec<_>>();
        let own = own_hints(
            &s(&["ws://127.0.0.1:7878", "wss://secure.example:8443", "ws://A.example:7878"]),
            &s(&["ws://a.example:7878", "ws://192.168.1.2:7878", "ws://b.example:9000", "ws://c.example"]),
        );
        assert_eq!(hint_text(&own), "A.example,b.example:9000");
        let own = own_hints(&s(&["wss://relay.tersenet.org"]), &s(&["ws://b.example:9000"]));
        assert_eq!(hint_text(&own), "relay.tersenet.org:443,b.example:9000");
        assert!(url_is_hint("wss://relay.tersenet.org", &parse_hint(b"relay.tersenet.org:443").unwrap()));
        assert!(own_hints(&[], &s(&["ws://127.0.0.1:7878", "ws://10.0.0.1"])).is_empty());
        assert!(url_is_hint("ws://relay.example:7878", &parse_hint(b"RELAY.example").unwrap()));
    }
}
