//! Relay hints of a feed address (NETWORK.md, Feed addresses): the text
//! after the `@` of `<key>@<hint>[,<hint>]`, which says where a feed may
//! be found. Only the grammar lives here (the content device checks the
//! hints a program gives to subscribe, SPEC §content.feeds); using a hint to reach a
//! relay is the network's business (terse-net).
//!
//! ```text
//! hint list = hint [ "," hint ]                    at most 161 bytes
//! hint      = host [ ":" port ]                    at most 80 bytes
//! host      = name / IPv4 / "[" IPv6 "]"
//! name      = label *( "." label ), the last label holding a letter
//! label     = 1 to 63 of A-Z a-z 0-9 "-", not starting or ending with "-"
//! IPv4      = four of 0-255 separated by ".", no leading zeros
//! IPv6      = 1 to 4 hex digits (either case) per group, 8 groups
//!             separated by ":", or fewer with one "::" for one or more
//!             zero groups; no dotted IPv4 part, no zone
//! port      = 1 to 65535, decimal, no leading zero
//! ```
//!
//! A hint names an encrypted relay, `wss://host[:port]` (443 when it names
//! no port); nothing that travels on the network names a plain relay
//! (NETWORK.md 3, 4.9). Clients ignore hints whose host is `localhost` or
//! a loopback address (NETWORK.md 5.1); the reference's tests, which follow
//! such hints, reach them plainly, as an address typed without a scheme
//! on this machine is (NETWORK.md 3).

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// The longest hint, in bytes.
pub const MAX_HINT: usize = 80;
/// The most hints an address has.
pub const MAX_HINTS: usize = 2;
/// The longest hint list: two hints and the comma.
pub const MAX_HINT_LIST: usize = MAX_HINTS * MAX_HINT + 1;
/// The default port of `wss://`: an encrypted hint that names no port.
pub const WSS_PORT: u16 = 443;
/// The default port of `ws://`, for a loopback hint that names no port.
pub const WS_PORT: u16 = 80;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Host {
    /// A DNS name, as written (either case).
    Name(String),
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

impl Host {
    /// `localhost` or a loopback IP address: a host whose relay is on this
    /// machine, reached plainly when no scheme is written (NETWORK.md 3).
    /// Names under `.localhost` are not, since a resolver may send them on.
    pub fn is_loopback(&self) -> bool {
        match self {
            Host::Name(n) => n.eq_ignore_ascii_case("localhost"),
            Host::V4(a) => a.is_loopback(),
            Host::V6(a) => a.is_loopback(),
        }
    }
}

/// Whether a host as a URL writes it (an IPv6 address in brackets) is
/// `localhost` or a loopback IP address ([`Host::is_loopback`]).
pub fn loopback_text(host: &str) -> bool {
    let inner = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host.eq_ignore_ascii_case("localhost") || inner.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// One hint: a relay's host and port (443 when the hint names none).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hint {
    pub host: Host,
    pub port: u16,
}

impl Hint {
    /// Whether the relay is reached over TLS: always, but on a loopback
    /// host (which clients ignore in hints, NETWORK.md 5.1).
    pub fn encrypted(&self) -> bool {
        !self.host.is_loopback()
    }

    /// The default port of the hint's scheme.
    pub fn default_port(&self) -> u16 {
        if self.encrypted() { WSS_PORT } else { WS_PORT }
    }

    /// The relay's URL: `wss://host` (with `:port` when it is not 443), or
    /// `ws://host:port`.
    pub fn url(&self) -> String {
        if !self.encrypted() {
            format!("ws://{}:{}", self.host_text(), self.port)
        } else if self.port == WSS_PORT {
            format!("wss://{}", self.host_text())
        } else {
            format!("wss://{}:{}", self.host_text(), self.port)
        }
    }

    /// The host as a URL writes it (an IPv6 address in brackets).
    pub fn host_text(&self) -> String {
        match &self.host {
            Host::Name(n) => n.clone(),
            Host::V4(a) => a.to_string(),
            Host::V6(a) => format!("[{a}]"),
        }
    }
}

/// The hint in its shortest text form: the port left out when it is the
/// default of the scheme.
impl fmt::Display for Hint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.port == self.default_port() { write!(f, "{}", self.host_text()) } else { write!(f, "{}:{}", self.host_text(), self.port) }
    }
}

/// Why a hint or hint list is malformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// An empty hint (an empty list, `a,` or `,a`).
    Empty,
    TooLong,
    TooMany,
    /// A name with an empty or over-long label, a label starting or ending
    /// with `-`, or a byte that is not a letter, digit, `-` or `.`.
    Name,
    /// The last label holds no letter, and it is not an IPv4 address.
    Address,
    /// An IPv6 address in brackets that is malformed (or unclosed).
    Ipv6,
    /// Nothing after the `:`, a leading zero, not digits, or out of range.
    Port,
}

impl Malformed {
    pub fn reason(self) -> &'static str {
        match self {
            Malformed::Empty => "an empty hint",
            Malformed::TooLong => "a hint longer than 80 bytes",
            Malformed::TooMany => "more than two hints",
            Malformed::Name => "a host name that is malformed",
            Malformed::Address => "an IPv4 address that is malformed (or a name whose last label has no letter)",
            Malformed::Ipv6 => "an IPv6 address that is malformed",
            Malformed::Port => "a port that is not 1 to 65535 (without leading zeros)",
        }
    }

    /// A short name for test vectors.
    pub fn code(self) -> &'static str {
        match self {
            Malformed::Empty => "empty-hint",
            Malformed::TooLong => "hint-too-long",
            Malformed::TooMany => "too-many-hints",
            Malformed::Name => "bad-name",
            Malformed::Address => "bad-ipv4",
            Malformed::Ipv6 => "bad-ipv6",
            Malformed::Port => "bad-port",
        }
    }
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

fn port(s: &[u8]) -> Result<u16, Malformed> {
    if s.is_empty() || s.len() > 5 || s[0] == b'0' || !s.iter().all(u8::is_ascii_digit) {
        return Err(Malformed::Port);
    }
    let v: u32 = s.iter().fold(0, |v, &d| v * 10 + (d - b'0') as u32);
    if v > 65535 { Err(Malformed::Port) } else { Ok(v as u16) }
}

fn ipv4(s: &[u8]) -> Option<Ipv4Addr> {
    let parts: Vec<&[u8]> = s.split(|&b| b == b'.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut o = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 3 || (p.len() > 1 && p[0] == b'0') || !p.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let v: u32 = p.iter().fold(0, |v, &d| v * 10 + (d - b'0') as u32);
        o[i] = u8::try_from(v).ok()?;
    }
    Some(Ipv4Addr::from(o))
}

fn hex_group(g: &[u8]) -> Option<u16> {
    if g.is_empty() || g.len() > 4 || !g.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    Some(g.iter().fold(0u16, |v, &d| v << 4 | (d as char).to_digit(16).unwrap() as u16))
}

fn ipv6(s: &[u8]) -> Option<Ipv6Addr> {
    let groups = |part: &[u8]| -> Option<Vec<u16>> {
        if part.is_empty() {
            return Some(Vec::new());
        }
        part.split(|&b| b == b':').map(hex_group).collect()
    };
    let mut out = [0u16; 8];
    match s.windows(2).position(|w| w == b"::") {
        Some(at) => {
            let (head, tail) = (groups(&s[..at])?, groups(&s[at + 2..])?);
            // One "::" only, standing for at least one group.
            if head.len() + tail.len() > 7 {
                return None;
            }
            out[..head.len()].copy_from_slice(&head);
            out[8 - tail.len()..].copy_from_slice(&tail);
        }
        None => {
            let g = groups(s)?;
            if g.len() != 8 {
                return None;
            }
            out.copy_from_slice(&g);
        }
    }
    Some(Ipv6Addr::from(out))
}

fn name(s: &[u8]) -> Result<Host, Malformed> {
    let labels: Vec<&[u8]> = s.split(|&b| b == b'.').collect();
    for l in &labels {
        if l.is_empty() || l.len() > 63 || l[0] == b'-' || l[l.len() - 1] == b'-' || !l.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-') {
            return Err(Malformed::Name);
        }
    }
    if !labels.last().unwrap().iter().any(u8::is_ascii_alphabetic) {
        return ipv4(s).map(Host::V4).ok_or(Malformed::Address);
    }
    Ok(Host::Name(String::from_utf8(s.to_vec()).unwrap()))
}

/// One hint (NETWORK.md, Feed addresses).
pub fn parse_hint(s: &[u8]) -> Result<Hint, Malformed> {
    if s.is_empty() {
        return Err(Malformed::Empty);
    }
    if s.len() > MAX_HINT {
        return Err(Malformed::TooLong);
    }
    let (host, rest) = if s[0] == b'[' {
        let close = s.iter().position(|&b| b == b']').ok_or(Malformed::Ipv6)?;
        (Host::V6(ipv6(&s[1..close]).ok_or(Malformed::Ipv6)?), &s[close + 1..])
    } else {
        let colon = s.iter().position(|&b| b == b':').unwrap_or(s.len());
        (name(&s[..colon])?, &s[colon..])
    };
    let port = match rest {
        [] => None,
        [b':', p @ ..] => Some(port(p)?),
        _ => return Err(if matches!(host, Host::V6(_)) { Malformed::Ipv6 } else { Malformed::Port }),
    };
    let mut hint = Hint { host, port: 0 };
    hint.port = port.unwrap_or(hint.default_port());
    Ok(hint)
}

/// A hint list: one or two hints separated by a comma.
pub fn parse_hints(s: &[u8]) -> Result<Vec<Hint>, Malformed> {
    if s.is_empty() {
        return Err(Malformed::Empty);
    }
    let parts: Vec<&[u8]> = s.split(|&b| b == b',').collect();
    if parts.len() > MAX_HINTS {
        return Err(Malformed::TooMany);
    }
    parts.into_iter().map(parse_hint).collect()
}

/// The hints of a feed link in an article (ARTICLE.md 4.1): `after` is
/// the text just after the `@` that follows the key. The candidate is the
/// longest run of letters, digits, `.`, `-`, `:`, `,`, `[` and `]` (a `]`
/// only while a `[` of the run is open), less every `.`, `,` and `:` at
/// its end. Returns the candidate's length if it is a valid hint list,
/// else 0 (the link is then the key alone, and the `@` is text).
pub fn article_hints(after: &[u8]) -> usize {
    let mut n = 0;
    let mut open = false;
    while let Some(&b) = after.get(n) {
        match b {
            b'[' => open = true,
            b']' if open => open = false,
            b if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b',') => {}
            _ => break,
        }
        n += 1;
    }
    while n > 0 && matches!(after[n - 1], b'.' | b',' | b':') {
        n -= 1;
    }
    if n <= MAX_HINT_LIST && parse_hints(&after[..n]).is_ok() { n } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Vec<Hint> {
        parse_hints(s.as_bytes()).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn bad(s: &str) -> Malformed {
        parse_hints(s.as_bytes()).err().unwrap_or_else(|| panic!("{s} should be malformed"))
    }

    #[test]
    fn forms() {
        // A bare host: encrypted, on the default port of wss://.
        assert_eq!(ok("relay.example.org"), [Hint { host: Host::Name("relay.example.org".into()), port: 443 }]);
        assert_eq!(ok("a")[0].url(), "wss://a");
        // A port is only a port: still encrypted.
        assert_eq!(ok("Relay.Example:443")[0].url(), "wss://Relay.Example");
        assert_eq!(ok("Relay.Example:443")[0].to_string(), "Relay.Example");
        assert_eq!(ok("relay.example:7878")[0].url(), "wss://relay.example:7878");
        assert_eq!(ok("a:80")[0].url(), "wss://a:80");
        assert_eq!(ok("[2001:db8::1]:8080")[0].url(), "wss://[2001:db8::1]:8080");
        assert_eq!(ok("[2001:db8::1]")[0].url(), "wss://[2001:db8::1]");
        // Loopback without a scheme: plain (this machine).
        assert_eq!(ok("127.0.0.1:7878")[0].url(), "ws://127.0.0.1:7878");
        assert_eq!(ok("127.0.0.1:7878")[0].to_string(), "127.0.0.1:7878");
        assert_eq!(ok("localhost:7878")[0].url(), "ws://localhost:7878");
        assert_eq!(ok("[::1]:9")[0].url(), "ws://[::1]:9");
        // A name under .localhost is not loopback here.
        assert_eq!(ok("a.localhost")[0].url(), "wss://a.localhost");
        assert_eq!(ok("203.0.113.5:65535")[0], Hint { host: Host::V4(Ipv4Addr::new(203, 0, 113, 5)), port: 65535 });
        assert_eq!(ok("0.0.0.0")[0].host, Host::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(ok("[::]")[0].host, Host::V6(Ipv6Addr::UNSPECIFIED));
        assert_eq!(ok("[1:2:3:4:5:6:7:8]")[0].to_string(), "[1:2:3:4:5:6:7:8]");
        assert_eq!(ok("[1:2:3:4:5:6::8]")[0].to_string(), "[1:2:3:4:5:6:0:8]");
        assert_eq!(ok("[ABCD::]")[0].to_string(), "[abcd::]");
        assert_eq!(ok("a.example,b.example:1").len(), 2);
        assert_eq!(ok("x-1.example")[0].to_string(), "x-1.example");
        assert_eq!(ok("1.example")[0].to_string(), "1.example");
        assert_eq!(ok(&format!("{}.e", "a".repeat(63))).len(), 1);
        assert_eq!(ok(&format!("{}.e", "a".repeat(63)))[0].to_string().len(), 65);
        assert_eq!(ok(&format!("{}.{}", "a".repeat(39), "b".repeat(40))).len(), 1);
        assert_eq!(bad(&format!("{}.{}", "a".repeat(40), "b".repeat(40))), Malformed::TooLong);
    }

    #[test]
    fn near_misses() {
        assert_eq!(bad(""), Malformed::Empty);
        assert_eq!(bad("a,"), Malformed::Empty);
        assert_eq!(bad(",a"), Malformed::Empty);
        assert_eq!(bad("a,b,c"), Malformed::TooMany);
        assert_eq!(bad(&"a".repeat(81)), Malformed::TooLong);
        assert_eq!(bad("a:"), Malformed::Port);
        assert_eq!(bad("a:0"), Malformed::Port);
        assert_eq!(bad("a:07878"), Malformed::Port);
        assert_eq!(bad("a:65536"), Malformed::Port);
        assert_eq!(bad("a:1x"), Malformed::Port);
        assert_eq!(bad("a:1:2"), Malformed::Port);
        assert_eq!(bad("-a.example"), Malformed::Name);
        assert_eq!(bad("a-.example"), Malformed::Name);
        assert_eq!(bad("a..example"), Malformed::Name);
        assert_eq!(bad("a.example."), Malformed::Name);
        assert_eq!(bad(".a"), Malformed::Name);
        assert_eq!(bad("a_b.example"), Malformed::Name);
        assert_eq!(bad(&format!("{}.e", "a".repeat(64))), Malformed::Name);
        // No scheme in a hint: a plain relay cannot be named, nor is a
        // scheme read as a host.
        assert_eq!(bad("ws://a.example"), Malformed::Port);
        assert_eq!(bad("wss://a.example"), Malformed::Port);
        assert_eq!(bad("ws:a.example:7878"), Malformed::Port);
        assert_eq!(bad("ws:127.0.0.1:7878"), Malformed::Port);
        assert_eq!(bad("ws:[::1]:9"), Malformed::Port);
        assert_eq!(bad("ws:"), Malformed::Port);
        assert_eq!(bad("a.example,ws:b.example:1"), Malformed::Port);
        assert_eq!(bad("a.example/x"), Malformed::Name);
        assert_eq!(bad("1.2.3"), Malformed::Address);
        assert_eq!(bad("1.2.3.256"), Malformed::Address);
        assert_eq!(bad("1.2.3.04"), Malformed::Address);
        assert_eq!(bad("1234"), Malformed::Address);
        assert_eq!(bad("2001:db8::1"), Malformed::Address);
        assert_eq!(bad("[2001:db8::1"), Malformed::Ipv6);
        assert_eq!(bad("[1::2::3]"), Malformed::Ipv6);
        assert_eq!(bad("[1:2:3:4:5:6:7:8:9]"), Malformed::Ipv6);
        assert_eq!(bad("[1:2:3:4:5:6:7::8]"), Malformed::Ipv6);
        assert_eq!(bad("[12345::]"), Malformed::Ipv6);
        assert_eq!(bad("[::ffff:1.2.3.4]"), Malformed::Ipv6);
        assert_eq!(bad("[fe80::1%eth0]"), Malformed::Ipv6);
        assert_eq!(bad("[]"), Malformed::Ipv6);
        assert_eq!(bad("[::1]x"), Malformed::Ipv6);
        assert_eq!(bad("[::1]:0"), Malformed::Port);
        assert_eq!(bad("a b"), Malformed::Name);
    }

    #[test]
    fn in_articles() {
        let h = |s: &str| article_hints(s.as_bytes());
        assert_eq!(h("relay.example."), 13);
        assert_eq!(h("relay.example, and"), 13);
        assert_eq!(h("relay.example:7000."), 18);
        assert_eq!(h("relay.example:"), 13);
        assert_eq!(h("a.example,b.example."), 19);
        assert_eq!(h("a.example,b.example,c.example"), 0);
        assert_eq!(h("[::1]:9)"), 7);
        assert_eq!(h("[::1]]"), 5);
        assert_eq!(h("a.example]"), 9);
        assert_eq!(h("a.example/x"), 9);
        assert_eq!(h("relay_x"), 5);
        assert_eq!(h("-x"), 0);
        assert_eq!(h(" relay.example"), 0);
        assert_eq!(h(""), 0);
        assert_eq!(h("..."), 0);
        assert_eq!(h("1.2.3"), 0);
        assert_eq!(h(&"a".repeat(81)), 0);
        // A scheme in an article is not a hint: the link is the key alone.
        assert_eq!(h("ws:relay.example:7878."), 0);
    }
}
