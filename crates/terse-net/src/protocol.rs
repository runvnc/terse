//! Relay messages (NETWORK 3, 4.3, 4.7, 5). All are binary WebSocket
//! messages.

use crate::blob::BlobId;
use crate::identity::FeedKey;

pub const GET: u8 = 0x01;
pub const PUT: u8 = 0x02;
pub const PUBLISH: u8 = 0x03;
pub const SUBSCRIBE: u8 = 0x04;
pub const UNSUBSCRIBE: u8 = 0x05;
pub const FIND: u8 = 0x06;
/// A relay announcing itself to another (NETWORK 5); no body, no reply.
pub const PEER: u8 = 0x07;
/// A feed's current entry of a kind and name (NETWORK 4.8).
pub const LOOKUP: u8 = 0x08;
pub const GET_REPLY: u8 = 0x81;
pub const PUT_REPLY: u8 = 0x82;
pub const PUBLISH_REPLY: u8 = 0x83;
pub const SUBSCRIBE_REPLY: u8 = 0x84;
pub const UNSUBSCRIBE_REPLY: u8 = 0x85;
pub const FIND_REPLY: u8 = 0x86;
pub const LOOKUP_REPLY: u8 = 0x88;
/// An entry pushed by the relay; it has no `req`.
pub const ENTRY: u8 = 0x90;

pub mod status {
    use std::time::Duration;

    pub const DONE: u8 = 1;
    pub const NOT_FOUND: u8 = 2;
    /// Refused without a reason given (too large, empty, malformed, an
    /// invalid entry, a different entry with the same `seq`...).
    pub const REFUSED: u8 = 3;
    /// Refused: the feed has reached this relay's limit (NETWORK 3.1).
    pub const FEED_FULL: u8 = 4;
    /// Refused: the relay is full (NETWORK 3.1).
    pub const RELAY_FULL: u8 = 5;
    /// Refused: the feed is not on this relay's list of writers (NETWORK
    /// 3.1).
    pub const NOT_WRITER: u8 = 6;
    /// The first "try again later" status (NETWORK 3.1).
    pub const RETRY_FIRST: u8 = 64;
    /// The first "try again later" status counted in minutes.
    const RETRY_MINUTES: u8 = 184;

    /// The "try again later" status for a wait of `wait` (rounded up to
    /// what the status can say, at most 74 minutes).
    pub fn retry(wait: Duration) -> u8 {
        let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        match secs {
            0..=120 => RETRY_FIRST + secs.max(1) as u8 - 1,
            _ => RETRY_MINUTES + (secs.div_ceil(60).min(74) as u8) - 3,
        }
    }

    /// The wait a "try again later" status asks for, or `None` for any
    /// other status.
    pub fn retry_after(status: u8) -> Option<Duration> {
        match status {
            RETRY_FIRST..RETRY_MINUTES => Some(Duration::from_secs(u64::from(status - RETRY_FIRST) + 1)),
            RETRY_MINUTES.. => Some(Duration::from_secs((u64::from(status - RETRY_MINUTES) + 3) * 60)),
            _ => None,
        }
    }
}

/// Why a relay refused a request (NETWORK 3.1), read from the status of a
/// reply that is not "done" or "not found".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Status 3, or a status this revision does not define.
    Unspecified,
    /// The feed has reached the relay's limit (status 4).
    FeedFull,
    /// The relay is full (status 5).
    RelayFull,
    /// The feed is not on the relay's list of writers (status 6).
    NotWriter,
    /// Too many requests from this address: try again after this wait
    /// (statuses 64 to 255).
    TryLater(std::time::Duration),
}

impl Refusal {
    pub fn of_status(s: u8) -> Self {
        match s {
            status::FEED_FULL => Refusal::FeedFull,
            status::RELAY_FULL => Refusal::RelayFull,
            status::NOT_WRITER => Refusal::NotWriter,
            _ => status::retry_after(s).map_or(Refusal::Unspecified, Refusal::TryLater),
        }
    }

    pub fn status(self) -> u8 {
        match self {
            Refusal::Unspecified => status::REFUSED,
            Refusal::FeedFull => status::FEED_FULL,
            Refusal::RelayFull => status::RELAY_FULL,
            Refusal::NotWriter => status::NOT_WRITER,
            Refusal::TryLater(wait) => status::retry(wait),
        }
    }

    /// The reason in plain words, to follow "it refused it: ".
    pub fn describe(self) -> String {
        match self {
            Refusal::Unspecified => "no reason given".into(),
            Refusal::FeedFull => "this feed is over the relay's limit".into(),
            Refusal::RelayFull => "the relay is full".into(),
            Refusal::NotWriter => "this feed is not on the relay's list of writers".into(),
            Refusal::TryLater(wait) => {
                let s = wait.as_secs();
                let when = if s < 120 { format!("{s} s") } else { format!("{} min", s.div_ceil(60)) };
                format!("too many requests from this address; try again in {when}")
            }
        }
    }
}

/// A client message as the relay sees it.
#[derive(Debug, PartialEq, Eq)]
pub enum Request<'a> {
    Get { req: u32, id: BlobId },
    Put { req: u32, data: &'a [u8] },
    /// The entry is not yet validated.
    Publish { req: u32, entry: &'a [u8] },
    Subscribe { req: u32, feed: FeedKey, after: u32 },
    Unsubscribe { req: u32, feed: FeedKey },
    /// `scope` and `query` are not yet checked.
    Find { req: u32, scope: u8, query: &'a [u8] },
    Peer,
    /// `kind` and `name` are not yet checked.
    Lookup { req: u32, feed: FeedKey, kind: u8, name: &'a [u8] },
}

/// Find scopes (NETWORK 4.7).
pub const SCOPE_FEEDS: u8 = 1;
pub const SCOPE_ENTRIES: u8 = 2;
/// Find programs by name (NETWORK 4.7).
pub const SCOPE_PROGRAMS: u8 = 3;
/// Most results in a find reply.
pub const MAX_FIND_RESULTS: usize = 16;

/// Why a client message could not be decoded.
#[derive(Debug, PartialEq, Eq)]
pub enum Malformed {
    /// The type and `req` are readable but the rest is wrong: reply "refused"
    /// with this reply type.
    Refuse { reply: u8, req: u32 },
    /// Unparseable: the relay closes the connection.
    Close,
    /// A type the relay does not know: ignored (NETWORK 3).
    Unknown,
}

/// A relay message as the client sees it.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    Get { req: u32, status: u8, data: Vec<u8> },
    Put { req: u32, status: u8, id: Option<BlobId> },
    /// A publish or unsubscribe reply, or a subscribe reply that is not
    /// "done"; `kind` is its type byte.
    Status { kind: u8, req: u32, status: u8 },
    /// A successful subscribe: the highest `seq` the relay holds for the
    /// feed, or 0 (NETWORK 4.3).
    Subscribed { req: u32, highest: u32 },
    /// A pushed entry, not yet validated.
    Entry(Vec<u8>),
    /// A find reply: the entries, not yet validated (empty unless done).
    Found { req: u32, status: u8, entries: Vec<Vec<u8>> },
    /// A lookup reply: the entry, not yet validated, if found.
    LookedUp { req: u32, status: u8, entry: Option<Vec<u8>> },
}

fn header(kind: u8, req: u32, extra: usize) -> Vec<u8> {
    let mut m = Vec::with_capacity(5 + extra);
    m.push(kind);
    m.extend_from_slice(&req.to_le_bytes());
    m
}

pub fn encode_get(req: u32, id: &BlobId) -> Vec<u8> {
    let mut m = header(GET, req, 32);
    m.extend_from_slice(id);
    m
}

pub fn encode_put(req: u32, data: &[u8]) -> Vec<u8> {
    let mut m = header(PUT, req, data.len());
    m.extend_from_slice(data);
    m
}

/// Get reply; `data` is sent only with status 1.
pub fn encode_get_reply(req: u32, status: u8, data: &[u8]) -> Vec<u8> {
    let data = if status == status::DONE { data } else { &[] };
    let mut m = header(GET_REPLY, req, 1 + data.len());
    m.push(status);
    m.extend_from_slice(data);
    m
}

/// Put reply; `id` is sent only with status 1.
pub fn encode_put_reply(req: u32, status: u8, id: &BlobId) -> Vec<u8> {
    let mut m = header(PUT_REPLY, req, 33);
    m.push(status);
    if status == status::DONE {
        m.extend_from_slice(id);
    }
    m
}

pub fn encode_publish(req: u32, entry: &[u8]) -> Vec<u8> {
    let mut m = header(PUBLISH, req, entry.len());
    m.extend_from_slice(entry);
    m
}

pub fn encode_subscribe(req: u32, feed: &FeedKey, after: u32) -> Vec<u8> {
    let mut m = header(SUBSCRIBE, req, 36);
    m.extend_from_slice(feed);
    m.extend_from_slice(&after.to_le_bytes());
    m
}

pub fn encode_unsubscribe(req: u32, feed: &FeedKey) -> Vec<u8> {
    let mut m = header(UNSUBSCRIBE, req, 32);
    m.extend_from_slice(feed);
    m
}

/// Subscribe reply; `highest` is sent only with status 1 (NETWORK 4.3).
pub fn encode_subscribe_reply(req: u32, status: u8, highest: u32) -> Vec<u8> {
    let mut m = encode_status_reply(SUBSCRIBE_REPLY, req, status);
    if status == status::DONE {
        m.extend_from_slice(&highest.to_le_bytes());
    }
    m
}

/// A publish or unsubscribe reply, or a failed subscribe (`kind` is the
/// reply type).
pub fn encode_status_reply(kind: u8, req: u32, status: u8) -> Vec<u8> {
    let mut m = header(kind, req, 1);
    m.push(status);
    m
}

pub fn encode_find(req: u32, scope: u8, query: &[u8]) -> Vec<u8> {
    let mut m = header(FIND, req, 2 + query.len());
    m.push(scope);
    m.push(query.len() as u8);
    m.extend_from_slice(query);
    m
}

/// Find reply: `status`, the count, then each entry with its length.
pub fn encode_find_reply<E: AsRef<[u8]>>(req: u32, status: u8, entries: &[E]) -> Vec<u8> {
    let entries = if status == status::DONE { entries } else { &[] };
    let mut m = header(FIND_REPLY, req, 2 + entries.iter().map(|e| 2 + e.as_ref().len()).sum::<usize>());
    m.push(status);
    m.push(entries.len() as u8);
    for e in entries {
        m.extend_from_slice(&(e.as_ref().len() as u16).to_le_bytes());
        m.extend_from_slice(e.as_ref());
    }
    m
}

pub fn encode_lookup(req: u32, feed: &FeedKey, kind: u8, name: &[u8]) -> Vec<u8> {
    let mut m = header(LOOKUP, req, 34 + name.len());
    m.extend_from_slice(feed);
    m.push(kind);
    m.push(name.len() as u8);
    m.extend_from_slice(name);
    m
}

/// Lookup reply; `entry` is sent only with status 1.
pub fn encode_lookup_reply(req: u32, status: u8, entry: &[u8]) -> Vec<u8> {
    let entry = if status == status::DONE { entry } else { &[] };
    let mut m = header(LOOKUP_REPLY, req, 1 + entry.len());
    m.push(status);
    m.extend_from_slice(entry);
    m
}

pub fn encode_peer() -> Vec<u8> {
    vec![PEER]
}

pub fn encode_entry(entry: &[u8]) -> Vec<u8> {
    [&[ENTRY], entry].concat()
}

fn split(msg: &[u8]) -> Option<(u8, u32, &[u8])> {
    let (&kind, rest) = msg.split_first()?;
    let req = u32::from_le_bytes(rest.get(..4)?.try_into().unwrap());
    Some((kind, req, &rest[4..]))
}

/// Whether a message type is one this relay knows as a request.
fn known_request(kind: u8) -> bool {
    (GET..=LOOKUP).contains(&kind)
}

pub fn decode_request(msg: &[u8]) -> Result<Request<'_>, Malformed> {
    match msg.first() {
        Some(&PEER) => return Ok(Request::Peer),
        Some(&kind) if !known_request(kind) => return Err(Malformed::Unknown),
        _ => {}
    }
    match split(msg).ok_or(Malformed::Close)? {
        (GET, req, rest) => match <BlobId>::try_from(rest) {
            Ok(id) => Ok(Request::Get { req, id }),
            Err(_) => Err(Malformed::Refuse { reply: GET_REPLY, req }),
        },
        (PUT, req, data) => Ok(Request::Put { req, data }),
        (PUBLISH, req, entry) => Ok(Request::Publish { req, entry }),
        (SUBSCRIBE, req, rest) if rest.len() == 36 => Ok(Request::Subscribe {
            req,
            feed: rest[..32].try_into().unwrap(),
            after: u32::from_le_bytes(rest[32..].try_into().unwrap()),
        }),
        (SUBSCRIBE, req, _) => Err(Malformed::Refuse { reply: SUBSCRIBE_REPLY, req }),
        (UNSUBSCRIBE, req, rest) => match <FeedKey>::try_from(rest) {
            Ok(feed) => Ok(Request::Unsubscribe { req, feed }),
            Err(_) => Err(Malformed::Refuse { reply: UNSUBSCRIBE_REPLY, req }),
        },
        (FIND, req, [scope, n, query @ ..]) if query.len() == *n as usize => {
            Ok(Request::Find { req, scope: *scope, query })
        }
        (FIND, req, _) => Err(Malformed::Refuse { reply: FIND_REPLY, req }),
        (LOOKUP, req, rest) if rest.len() >= 34 && rest.len() == 34 + rest[33] as usize => Ok(Request::Lookup {
            req,
            feed: rest[..32].try_into().unwrap(),
            kind: rest[32],
            name: &rest[34..],
        }),
        (LOOKUP, req, _) => Err(Malformed::Refuse { reply: LOOKUP_REPLY, req }),
        _ => Err(Malformed::Close),
    }
}

/// The entries of a done find reply's body (after the status).
fn decode_found(mut body: &[u8]) -> Option<Vec<Vec<u8>>> {
    let (&count, rest) = body.split_first()?;
    body = rest;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let len = u16::from_le_bytes(body.get(..2)?.try_into().unwrap()) as usize;
        entries.push(body.get(2..2 + len)?.to_vec());
        body = &body[2 + len..];
    }
    body.is_empty().then_some(entries)
}

/// `None` for anything that is not a well-formed reply.
pub fn decode_reply(msg: &[u8]) -> Option<Reply> {
    if let [ENTRY, entry @ ..] = msg {
        return Some(Reply::Entry(entry.to_vec()));
    }
    let (kind, req, rest) = split(msg)?;
    let (&status, body) = rest.split_first()?;
    let done = status == status::DONE;
    match kind {
        GET_REPLY if done || body.is_empty() => Some(Reply::Get { req, status, data: body.to_vec() }),
        PUT_REPLY if done => Some(Reply::Put { req, status, id: Some(body.try_into().ok()?) }),
        PUT_REPLY if body.is_empty() => Some(Reply::Put { req, status, id: None }),
        SUBSCRIBE_REPLY if done => Some(Reply::Subscribed { req, highest: u32::from_le_bytes(body.try_into().ok()?) }),
        PUBLISH_REPLY | SUBSCRIBE_REPLY | UNSUBSCRIBE_REPLY if body.is_empty() => Some(Reply::Status { kind, req, status }),
        FIND_REPLY if done => Some(Reply::Found { req, status, entries: decode_found(body)? }),
        FIND_REPLY if body == [0] => Some(Reply::Found { req, status, entries: Vec::new() }),
        LOOKUP_REPLY if done && !body.is_empty() => Some(Reply::LookedUp { req, status, entry: Some(body.to_vec()) }),
        LOOKUP_REPLY if !done && body.is_empty() => Some(Reply::LookedUp { req, status, entry: None }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        let id = [9; 32];
        assert_eq!(encode_get(0x0403_0201, &id)[..5], [GET, 1, 2, 3, 4]);
        assert_eq!(encode_get(1, &id).len(), 37);
        assert_eq!(encode_put(1, b"ab"), [PUT, 1, 0, 0, 0, b'a', b'b']);
        assert_eq!(encode_get_reply(7, status::NOT_FOUND, b"ignored"), [GET_REPLY, 7, 0, 0, 0, 2]);
        assert_eq!(encode_put_reply(7, status::REFUSED, &id), [PUT_REPLY, 7, 0, 0, 0, 3]);
        assert_eq!(encode_put_reply(7, status::DONE, &id).len(), 38);
    }

    #[test]
    fn decoding() {
        let id = [9; 32];
        assert_eq!(decode_request(&encode_get(5, &id)), Ok(Request::Get { req: 5, id }));
        assert_eq!(decode_request(&encode_put(5, b"x")), Ok(Request::Put { req: 5, data: b"x" }));
        assert_eq!(decode_request(&encode_put(5, b"")), Ok(Request::Put { req: 5, data: b"" }));
        assert_eq!(decode_request(&encode_get(5, &id)[..36]), Err(Malformed::Refuse { reply: GET_REPLY, req: 5 }));
        assert_eq!(decode_request(&[GET, 1, 2, 3]), Err(Malformed::Close));
        assert_eq!(decode_request(&[0x7F, 1, 2, 3, 4]), Err(Malformed::Unknown));
        assert_eq!(decode_request(&[0x09]), Err(Malformed::Unknown));
        assert_eq!(decode_request(&[0x86, 1, 2, 3, 4, 1, 0]), Err(Malformed::Unknown));
        assert_eq!(decode_request(&[]), Err(Malformed::Close));
        assert_eq!(decode_request(&[PEER]), Ok(Request::Peer));

        let reply = encode_get_reply(3, status::DONE, b"blob");
        assert_eq!(decode_reply(&reply), Some(Reply::Get { req: 3, status: 1, data: b"blob".to_vec() }));
        assert_eq!(decode_reply(&encode_put_reply(3, status::DONE, &id)), Some(Reply::Put { req: 3, status: 1, id: Some(id) }));
        assert_eq!(decode_reply(&encode_put_reply(3, status::REFUSED, &id)), Some(Reply::Put { req: 3, status: 3, id: None }));
        assert_eq!(decode_reply(&[GET_REPLY, 3, 0, 0, 0, 2, 0]), None, "data with a failure status");
        assert_eq!(decode_reply(&[PUT_REPLY, 3, 0, 0, 0, 1, 0]), None, "short id");
        assert_eq!(decode_reply(&encode_get(1, &id)), None);
    }

    #[test]
    fn feed_messages() {
        let key = [4; 32];
        let sub = encode_subscribe(0x0403_0201, &key, 0x0807_0605);
        assert_eq!(sub.len(), 41);
        assert_eq!(sub[..5], [SUBSCRIBE, 1, 2, 3, 4]);
        assert_eq!(sub[37..], [5, 6, 7, 8]);
        assert_eq!(decode_request(&sub), Ok(Request::Subscribe { req: 0x0403_0201, feed: key, after: 0x0807_0605 }));
        assert_eq!(decode_request(&sub[..40]), Err(Malformed::Refuse { reply: SUBSCRIBE_REPLY, req: 0x0403_0201 }));
        let unsub = encode_unsubscribe(9, &key);
        assert_eq!(unsub.len(), 37);
        assert_eq!(decode_request(&unsub), Ok(Request::Unsubscribe { req: 9, feed: key }));
        assert_eq!(decode_request(&unsub[..36]), Err(Malformed::Refuse { reply: UNSUBSCRIBE_REPLY, req: 9 }));
        assert_eq!(encode_publish(2, b"entry"), [PUBLISH, 2, 0, 0, 0, b'e', b'n', b't', b'r', b'y']);
        assert_eq!(decode_request(&encode_publish(2, b"entry")), Ok(Request::Publish { req: 2, entry: b"entry" }));

        assert_eq!(encode_status_reply(PUBLISH_REPLY, 3, status::DONE), [PUBLISH_REPLY, 3, 0, 0, 0, 1]);
        let sub = encode_subscribe_reply(3, status::DONE, 0x0102_0304);
        assert_eq!(sub, [SUBSCRIBE_REPLY, 3, 0, 0, 0, 1, 4, 3, 2, 1]);
        assert_eq!(decode_reply(&sub), Some(Reply::Subscribed { req: 3, highest: 0x0102_0304 }));
        assert_eq!(decode_reply(&sub[..9]), None, "short highest");
        assert_eq!(decode_reply(&sub[..6]), None, "done without highest");
        assert_eq!(encode_subscribe_reply(3, status::REFUSED, 7), [SUBSCRIBE_REPLY, 3, 0, 0, 0, 3]);
        for kind in [PUBLISH_REPLY, SUBSCRIBE_REPLY, UNSUBSCRIBE_REPLY] {
            let reply = encode_status_reply(kind, 3, status::REFUSED);
            assert_eq!(decode_reply(&reply), Some(Reply::Status { kind, req: 3, status: 3 }));
            assert_eq!(decode_reply(&[reply, vec![0]].concat()), None);
        }
        assert_eq!(encode_entry(b"abc"), [ENTRY, b'a', b'b', b'c']);
        assert_eq!(decode_reply(&encode_entry(b"abc")), Some(Reply::Entry(b"abc".to_vec())));
        assert_eq!(decode_reply(&[ENTRY]), Some(Reply::Entry(Vec::new())));
    }

    #[test]
    fn find_messages() {
        let find = encode_find(0x0403_0201, SCOPE_FEEDS, b"ana");
        assert_eq!(find, [FIND, 1, 2, 3, 4, 1, 3, b'a', b'n', b'a']);
        assert_eq!(decode_request(&find), Ok(Request::Find { req: 0x0403_0201, scope: 1, query: b"ana" }));
        assert_eq!(decode_request(&find[..9]), Err(Malformed::Refuse { reply: FIND_REPLY, req: 0x0403_0201 }));
        assert_eq!(decode_request(&find[..6]), Err(Malformed::Refuse { reply: FIND_REPLY, req: 0x0403_0201 }));
        let reply = encode_find_reply(7, status::DONE, &[b"ab".to_vec(), b"c".to_vec()]);
        assert_eq!(reply, [FIND_REPLY, 7, 0, 0, 0, 1, 2, 2, 0, b'a', b'b', 1, 0, b'c']);
        assert_eq!(
            decode_reply(&reply),
            Some(Reply::Found { req: 7, status: 1, entries: vec![b"ab".to_vec(), b"c".to_vec()] })
        );
        assert_eq!(decode_reply(&reply[..reply.len() - 1]), None, "cut short");
        assert_eq!(decode_reply(&[reply.clone(), vec![0]].concat()), None, "trailing byte");
        let refused = encode_find_reply(7, status::REFUSED, &[b"x"]);
        assert_eq!(refused, [FIND_REPLY, 7, 0, 0, 0, 3, 0]);
        assert_eq!(decode_reply(&refused), Some(Reply::Found { req: 7, status: 3, entries: Vec::new() }));
        assert_eq!(encode_peer(), [PEER]);
    }

    /// NETWORK 3.1: the refusal statuses and the waits they carry.
    #[test]
    fn refusals() {
        use std::time::Duration;
        let secs = Duration::from_secs;
        assert_eq!(status::retry(Duration::ZERO), 64);
        assert_eq!(status::retry(Duration::from_millis(1)), 64, "rounded up to 1 s");
        assert_eq!(status::retry(secs(1)), 64);
        assert_eq!(status::retry(Duration::from_millis(1500)), 65);
        assert_eq!(status::retry(secs(120)), 183);
        assert_eq!(status::retry(secs(121)), 184, "3 minutes");
        assert_eq!(status::retry(secs(180)), 184);
        assert_eq!(status::retry(secs(181)), 185);
        assert_eq!(status::retry(secs(74 * 60)), 255);
        assert_eq!(status::retry(secs(86_400)), 255, "at most 74 minutes");
        for s in 0..=255u8 {
            let r = Refusal::of_status(s);
            match s {
                0..=3 | 7..=63 => assert_eq!(r, Refusal::Unspecified, "{s}"),
                4..=6 => assert_eq!(r.status(), s),
                _ => {
                    let Refusal::TryLater(wait) = r else { panic!("{s}") };
                    assert_eq!(status::retry(wait), s, "the wait of {s} gives it back");
                }
            }
        }
        assert_eq!(status::retry_after(64), Some(secs(1)));
        assert_eq!(status::retry_after(183), Some(secs(120)));
        assert_eq!(status::retry_after(184), Some(secs(180)));
        assert_eq!(status::retry_after(255), Some(secs(74 * 60)));
        assert_eq!(Refusal::TryLater(secs(30)).describe(), "too many requests from this address; try again in 30 s");
        assert_eq!(Refusal::TryLater(secs(600)).describe(), "too many requests from this address; try again in 10 min");
        // Older clients read any refusal as one: the layouts do not change.
        assert_eq!(decode_reply(&encode_put_reply(1, status::FEED_FULL, &[0; 32])), Some(Reply::Put { req: 1, status: 4, id: None }));
        assert_eq!(decode_reply(&encode_find_reply::<&[u8]>(1, 70, &[])), Some(Reply::Found { req: 1, status: 70, entries: Vec::new() }));
    }

    #[test]
    fn lookup_messages() {
        let key = [4; 32];
        let m = encode_lookup(0x0403_0201, &key, 3, b"tool");
        assert_eq!(m.len(), 5 + 32 + 2 + 4);
        assert_eq!((m[0], m[37], m[38]), (LOOKUP, 3, 4));
        assert_eq!(decode_request(&m), Ok(Request::Lookup { req: 0x0403_0201, feed: key, kind: 3, name: b"tool" }));
        assert_eq!(decode_request(&m[..m.len() - 1]), Err(Malformed::Refuse { reply: LOOKUP_REPLY, req: 0x0403_0201 }));
        let reply = encode_lookup_reply(7, status::DONE, b"entry");
        assert_eq!(reply, [LOOKUP_REPLY, 7, 0, 0, 0, 1, b'e', b'n', b't', b'r', b'y']);
        assert_eq!(decode_reply(&reply), Some(Reply::LookedUp { req: 7, status: 1, entry: Some(b"entry".to_vec()) }));
        let none = encode_lookup_reply(7, status::NOT_FOUND, b"x");
        assert_eq!(none, [LOOKUP_REPLY, 7, 0, 0, 0, 2]);
        assert_eq!(decode_reply(&none), Some(Reply::LookedUp { req: 7, status: 2, entry: None }));
        assert_eq!(decode_reply(&[LOOKUP_REPLY, 7, 0, 0, 0, 1]), None, "done without an entry");
    }
}
