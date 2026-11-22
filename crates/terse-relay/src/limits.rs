//! Limits per network address (NETWORK 3.1, 5.3): token buckets kept in a
//! table of fixed size, the address a client is counted under (an IPv4
//! address, or an IPv6 /64), and the proxies whose header naming the client
//! is trusted.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

/// What a client is counted as: one IPv4 address, or one IPv6 /64 (an
/// IPv4-mapped IPv6 address counts as the IPv4 address).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    V4([u8; 4]),
    V6([u8; 8]),
    /// Every new address while the table is full of busy ones.
    Overflow,
}

impl Group {
    pub fn of(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => Group::V4(v4.octets()),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => Group::V4(v4.octets()),
                None => Group::V6(v6.octets()[..8].try_into().unwrap()),
            },
        }
    }

    /// The group in a log: truncated, so a log names no single client
    /// (IPv4 to its /24, IPv6 to its /48).
    pub fn for_log(&self) -> String {
        match self {
            Group::V4([a, b, c, _]) => format!("{a}.{b}.{c}.x"),
            Group::V6(p) => format!(
                "{:x}:{:x}:{:x}::/48",
                u16::from_be_bytes([p[0], p[1]]),
                u16::from_be_bytes([p[2], p[3]]),
                u16::from_be_bytes([p[4], p[5]])
            ),
            Group::Overflow => "(overflow)".into(),
        }
    }
}

/// An address range: `addr/bits`, or one address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    bits: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Self> {
        let (a, bits) = match s.split_once('/') {
            Some((a, b)) => (a, Some(b.parse::<u8>().ok()?)),
            None => (s, None),
        };
        let addr: IpAddr = a.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let bits = bits.unwrap_or(max);
        (bits <= max).then_some(Cidr { addr, bits })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) if self.addr.is_ipv4() => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            _ => ip,
        };
        let (a, b, len) = match (self.addr, ip) {
            (IpAddr::V4(a), IpAddr::V4(b)) => (u128::from(u32::from(a)) << 96, u128::from(u32::from(b)) << 96, self.bits),
            (IpAddr::V6(a), IpAddr::V6(b)) => (u128::from(a), u128::from(b), self.bits),
            _ => return false,
        };
        let mask = if len == 0 { 0 } else { u128::MAX << (128 - u32::from(len)) };
        a & mask == b & mask
    }
}

/// `addr/bits`, or the address alone for a single one.
impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let max = if self.addr.is_ipv4() { 32 } else { 128 };
        if self.bits == max { write!(f, "{}", self.addr) } else { write!(f, "{}/{}", self.addr, self.bits) }
    }
}

/// The client's address for a connection from `peer`: the proxy's header
/// if `peer` is a trusted proxy and the header names one, else `peer`.
/// `CF-Connecting-IP` is preferred; else the right-most address of
/// `X-Forwarded-For` that is not itself a trusted proxy.
pub fn client_ip(peer: IpAddr, trusted: &[Cidr], header: impl Fn(&str) -> Option<String>) -> IpAddr {
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    if !is_trusted(peer) {
        return peer;
    }
    if let Some(ip) = header("cf-connecting-ip").and_then(|v| v.trim().parse().ok()) {
        return ip;
    }
    if let Some(list) = header("x-forwarded-for") {
        for item in list.rsplit(',') {
            match item.trim().parse::<IpAddr>() {
                Ok(ip) if is_trusted(ip) => continue,
                Ok(ip) => return ip,
                Err(_) => break,
            }
        }
    }
    peer
}

/// `amount` per `period`, at most `amount` at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub amount: f64,
    pub period: Duration,
}

impl Rate {
    pub fn new(amount: f64, period: Duration) -> Self {
        Rate { amount, period }
    }

    fn per_ms(&self) -> f64 {
        self.amount / (self.period.as_millis().max(1) as f64)
    }
}

/// A token bucket: full when created.
#[derive(Clone, Copy, Debug)]
pub struct Bucket {
    tokens: f64,
    at: u64,
}

impl Bucket {
    pub fn full(now: u64, rate: &Rate) -> Self {
        Bucket { tokens: rate.amount, at: now }
    }

    fn refill(&mut self, now: u64, rate: &Rate) {
        let elapsed = now.saturating_sub(self.at) as f64;
        self.tokens = (self.tokens + elapsed * rate.per_ms()).min(rate.amount);
        self.at = self.at.max(now);
    }

    /// Takes `cost` (at most the whole bucket); else the wait until it
    /// could.
    pub fn take(&mut self, cost: f64, now: u64, rate: &Rate) -> Result<(), Duration> {
        self.refill(now, rate);
        let cost = cost.min(rate.amount);
        if self.tokens >= cost {
            self.tokens -= cost;
            return Ok(());
        }
        let ms = ((cost - self.tokens) / rate.per_ms()).ceil() as u64;
        Err(Duration::from_millis(ms.max(1)))
    }

    /// Whether the bucket is full at `now` (it can be forgotten).
    pub fn is_full(&self, now: u64, rate: &Rate) -> bool {
        let mut b = *self;
        b.refill(now, rate);
        b.tokens >= rate.amount
    }
}

/// The rates a relay limits each address to.
#[derive(Clone, Debug, PartialEq)]
pub struct Rates {
    /// Puts and publishes.
    pub uploads: Rate,
    /// Bytes of puts.
    pub upload_bytes: Rate,
    /// Entries of feeds the relay holds nothing of.
    pub new_feeds: Rate,
    /// Finds and lookups.
    pub finds: Rate,
}

/// What a request is counted as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Upload,
    UploadBytes,
    NewFeed,
    Find,
}

#[derive(Clone, Debug)]
struct Slot {
    /// Uploads, upload bytes, new feeds, finds: with rates only.
    buckets: Option<[Bucket; 4]>,
    connections: usize,
    /// Until when the address counts as a writer's (it published an
    /// entry the write list allows), in ms.
    writer_until: u64,
}

impl Rates {
    fn of(&self, kind: Kind) -> &Rate {
        match kind {
            Kind::Upload => &self.uploads,
            Kind::UploadBytes => &self.upload_bytes,
            Kind::NewFeed => &self.new_feeds,
            Kind::Find => &self.finds,
        }
    }
}

const KINDS: [Kind; 4] = [Kind::Upload, Kind::UploadBytes, Kind::NewFeed, Kind::Find];

impl Slot {
    fn new(now: u64, r: Option<&Rates>) -> Self {
        Slot { buckets: r.map(|r| KINDS.map(|k| Bucket::full(now, r.of(k)))), connections: 0, writer_until: 0 }
    }

    /// Nothing would be lost by forgetting it: no connection, not a
    /// writer's, every bucket full again.
    fn idle(&self, now: u64, r: Option<&Rates>) -> bool {
        self.connections == 0
            && self.writer_until <= now
            && match (&self.buckets, r) {
                (Some(b), Some(r)) => KINDS.iter().zip(b).all(|(k, b)| b.is_full(now, r.of(*k))),
                _ => true,
            }
    }
}

/// The buckets of each address, in a table of at most `capacity` slots.
/// When it is full, idle slots are forgotten (no connection, not a
/// writer's, buckets full again: a new slot starts the same); if none can
/// be, a new address shares the one overflow slot, whose limits are as
/// strict as any address's.
pub struct RateTable {
    rates: Option<Rates>,
    capacity: usize,
    slots: HashMap<Group, Slot>,
    /// When the table was last swept for idle slots (ms).
    swept: Option<u64>,
}

impl RateTable {
    pub fn new(rates: Option<Rates>, capacity: usize) -> Self {
        RateTable { rates, capacity: capacity.max(1), slots: HashMap::new(), swept: None }
    }

    pub fn rates(&self) -> Option<&Rates> {
        self.rates.as_ref()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The slot of `group`, made if need be (or the overflow slot).
    fn slot(&mut self, group: Group, now: u64) -> &mut Slot {
        let rates = self.rates.as_ref();
        let mut group = group;
        if !self.slots.contains_key(&group) && self.slots.len() >= self.capacity {
            // At most one sweep a second: a full table of busy addresses
            // does not cost a scan per request.
            if self.swept.is_none_or(|t| now.saturating_sub(t) >= 1000) {
                self.swept = Some(now);
                self.slots.retain(|g, s| *g == Group::Overflow || !s.idle(now, rates));
            }
            if self.slots.len() >= self.capacity {
                group = Group::Overflow;
            }
        }
        self.slots.entry(group).or_insert_with(|| Slot::new(now, rates))
    }

    /// Takes `cost` from `group`'s bucket for `kind`; the wait if it is
    /// empty. Without rates everything is allowed.
    pub fn take(&mut self, group: Group, kind: Kind, cost: f64, now: u64) -> Result<(), Duration> {
        let Some(rates) = self.rates.clone() else { return Ok(()) };
        let i = KINDS.iter().position(|k| *k == kind).unwrap();
        let slot = self.slot(group, now);
        let buckets = slot.buckets.get_or_insert_with(|| KINDS.map(|k| Bucket::full(now, rates.of(k))));
        buckets[i].take(cost, now, rates.of(kind))
    }

    /// Counts a connection of `group` if it has fewer than `max`; false
    /// if not.
    pub fn connect(&mut self, group: Group, max: Option<usize>, now: u64) -> bool {
        let Some(max) = max else { return true };
        let slot = self.slot(group, now);
        if slot.connections >= max {
            return false;
        }
        slot.connections += 1;
        true
    }

    /// Uncounts a connection counted by [`Self::connect`] (in the
    /// address's slot, or the overflow slot it was put in).
    pub fn disconnect(&mut self, group: Group) {
        let g = if self.slots.contains_key(&group) { group } else { Group::Overflow };
        if let Some(s) = self.slots.get_mut(&g) {
            s.connections = s.connections.saturating_sub(1);
        }
    }

    /// Marks `group` as a writer's until `until` (ms).
    pub fn mark_writer(&mut self, group: Group, until: u64, now: u64) {
        let slot = self.slot(group, now);
        slot.writer_until = slot.writer_until.max(until);
    }

    pub fn is_writer(&self, group: Group, now: u64) -> bool {
        self.slots.get(&group).is_some_and(|s| s.writer_until > now)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn groups() {
        assert_eq!(Group::of(ip("203.0.113.7")), Group::V4([203, 0, 113, 7]));
        assert_eq!(Group::of(ip("::ffff:203.0.113.7")), Group::of(ip("203.0.113.7")));
        // One /64 is one client.
        assert_eq!(Group::of(ip("2001:db8:1:2:aaaa::1")), Group::of(ip("2001:db8:1:2:ffff:1:2:3")));
        assert_ne!(Group::of(ip("2001:db8:1:2::1")), Group::of(ip("2001:db8:1:3::1")));
        assert_eq!(Group::of(ip("203.0.113.7")).for_log(), "203.0.113.x");
        assert_eq!(Group::of(ip("2001:db8:1:2::1")).for_log(), "2001:db8:1::/48");
    }

    #[test]
    fn ranges() {
        let c = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains(ip("10.200.1.1")) && !c.contains(ip("11.0.0.1")));
        assert!(c.contains(ip("::ffff:10.1.1.1")));
        assert!(Cidr::parse("127.0.0.1").unwrap().contains(ip("127.0.0.1")));
        assert!(!Cidr::parse("127.0.0.1").unwrap().contains(ip("127.0.0.2")));
        let v6 = Cidr::parse("2400:cb00::/32").unwrap();
        assert!(v6.contains(ip("2400:cb00:1::5")) && !v6.contains(ip("2400:cb01::5")));
        assert!(!v6.contains(ip("10.0.0.1")));
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        for bad in ["10.0.0.0/33", "::/129", "nonsense", "10.0.0.0/x", ""] {
            assert_eq!(Cidr::parse(bad), None, "{bad}");
        }
    }

    /// The proxy's header is used only from a trusted proxy.
    #[test]
    fn proxy_header() {
        let trusted = [Cidr::parse("127.0.0.1").unwrap(), Cidr::parse("173.245.48.0/20").unwrap()];
        let headers = |cf: Option<&'static str>, xff: Option<&'static str>| {
            move |name: &str| match name {
                "cf-connecting-ip" => cf.map(String::from),
                "x-forwarded-for" => xff.map(String::from),
                _ => None,
            }
        };
        let proxy = ip("127.0.0.1");
        assert_eq!(client_ip(proxy, &trusted, headers(Some("198.51.100.4"), None)), ip("198.51.100.4"));
        assert_eq!(client_ip(proxy, &trusted, headers(Some(" 2001:db8::9 "), Some("1.2.3.4"))), ip("2001:db8::9"));
        // X-Forwarded-For: the right-most address that is not a proxy.
        assert_eq!(client_ip(proxy, &trusted, headers(None, Some("6.6.6.6, 198.51.100.4, 173.245.48.9"))), ip("198.51.100.4"));
        // No usable header: the proxy's own address.
        assert_eq!(client_ip(proxy, &trusted, headers(Some("junk"), None)), proxy);
        assert_eq!(client_ip(proxy, &trusted, headers(None, Some("junk, 1.2.3.4"))), ip("1.2.3.4"));
        assert_eq!(client_ip(proxy, &trusted, headers(None, Some("1.2.3.4, junk"))), proxy);
        // Not trusted: the header is ignored (anyone could send it).
        let stranger = ip("198.51.100.77");
        assert_eq!(client_ip(stranger, &trusted, headers(Some("1.1.1.1"), Some("2.2.2.2"))), stranger);
        assert_eq!(client_ip(proxy, &[], headers(Some("1.1.1.1"), None)), proxy);
    }

    /// Buckets with a fake clock (milliseconds).
    #[test]
    fn buckets() {
        let rate = Rate::new(4.0, Duration::from_secs(4));
        let mut b = Bucket::full(0, &rate);
        for _ in 0..4 {
            assert_eq!(b.take(1.0, 0, &rate), Ok(()));
        }
        assert_eq!(b.take(1.0, 0, &rate), Err(Duration::from_secs(1)));
        assert_eq!(b.take(1.0, 500, &rate), Err(Duration::from_millis(500)));
        assert_eq!(b.take(1.0, 1000, &rate), Ok(()));
        assert!(!b.is_full(1000, &rate));
        assert!(b.is_full(5000, &rate));
        // A cost over the whole bucket costs the whole bucket.
        assert_eq!(b.take(100.0, 5000, &rate), Ok(()));
        assert_eq!(b.take(1.0, 5000, &rate), Err(Duration::from_secs(1)));
        // A clock that goes back does not add tokens.
        assert!(b.take(1.0, 10, &rate).is_err());
    }

    fn rates() -> Rates {
        let r = Rate::new(2.0, Duration::from_secs(10));
        Rates { uploads: r, upload_bytes: Rate::new(100.0, Duration::from_secs(10)), new_feeds: r, finds: r }
    }

    #[test]
    fn table_is_bounded() {
        let mut t = RateTable::new(Some(rates()), 3);
        let g = |n: u8| Group::V4([10, 0, 0, n]);
        for n in 1..=3 {
            assert!(t.take(g(n), Kind::Upload, 2.0, 0).is_ok());
        }
        // Full of busy addresses: a fourth shares the overflow slot.
        assert!(t.take(g(4), Kind::Upload, 1.0, 0).is_ok());
        assert!(t.take(g(5), Kind::Upload, 1.0, 0).is_ok());
        assert!(t.take(g(6), Kind::Upload, 1.0, 0).is_err(), "the overflow slot is one address");
        assert_eq!(t.len(), 4);
        // Once the first three are full again they are forgotten.
        assert!(t.take(g(7), Kind::Upload, 1.0, 20_000).is_ok());
        assert!(t.len() <= 4);
        assert!(t.slots.contains_key(&g(7)));
        // Connections keep a slot.
        let mut t = RateTable::new(Some(rates()), 1);
        assert!(t.connect(g(1), Some(2), 0));
        assert!(t.connect(g(1), Some(2), 0));
        assert!(!t.connect(g(1), Some(2), 0));
        assert!(t.take(g(2), Kind::Find, 1.0, 50_000).is_ok());
        assert!(t.slots.contains_key(&g(1)), "a connected address is not forgotten");
        t.disconnect(g(1));
        assert!(t.connect(g(1), Some(2), 50_000));
        // Without rates, connections and writers are counted.
        let mut t = RateTable::new(None, 1);
        assert!(t.take(g(1), Kind::Upload, 1e9, 0).is_ok());
        assert!(t.connect(g(1), Some(1), 0));
        assert!(!t.connect(g(1), Some(1), 0));
        t.disconnect(g(1));
        t.mark_writer(g(2), 5000, 2000);
        assert!(t.is_writer(g(2), 4999) && !t.is_writer(g(2), 5000));
        assert!(!t.is_writer(g(1), 0));
    }
}
