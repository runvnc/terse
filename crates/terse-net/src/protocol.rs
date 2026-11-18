//! Relay messages (NETWORK 3, 4.3). All are binary WebSocket messages.

use crate::blob::BlobId;
use crate::identity::FeedKey;

pub const GET: u8 = 0x01;
pub const PUT: u8 = 0x02;
pub const PUBLISH: u8 = 0x03;
pub const SUBSCRIBE: u8 = 0x04;
pub const UNSUBSCRIBE: u8 = 0x05;
pub const GET_REPLY: u8 = 0x81;
pub const PUT_REPLY: u8 = 0x82;
pub const PUBLISH_REPLY: u8 = 0x83;
pub const SUBSCRIBE_REPLY: u8 = 0x84;
pub const UNSUBSCRIBE_REPLY: u8 = 0x85;
/// An entry pushed by the relay; it has no `req`.
pub const ENTRY: u8 = 0x90;

pub mod status {
    pub const DONE: u8 = 1;
    pub const NOT_FOUND: u8 = 2;
    /// Too large, empty, store full, or malformed request.
    pub const REFUSED: u8 = 3;
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
}

/// Why a client message could not be decoded.
#[derive(Debug, PartialEq, Eq)]
pub enum Malformed {
    /// The type and `req` are readable but the rest is wrong: reply "refused"
    /// with this reply type.
    Refuse { reply: u8, req: u32 },
    /// Unparseable: the relay closes the connection.
    Close,
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

pub fn encode_entry(entry: &[u8]) -> Vec<u8> {
    [&[ENTRY], entry].concat()
}

fn split(msg: &[u8]) -> Option<(u8, u32, &[u8])> {
    let (&kind, rest) = msg.split_first()?;
    let req = u32::from_le_bytes(rest.get(..4)?.try_into().unwrap());
    Some((kind, req, &rest[4..]))
}

pub fn decode_request(msg: &[u8]) -> Result<Request<'_>, Malformed> {
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
        _ => Err(Malformed::Close),
    }
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
        assert_eq!(decode_request(&[0x7F, 1, 2, 3, 4]), Err(Malformed::Close));
        assert_eq!(decode_request(&[]), Err(Malformed::Close));

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
}
