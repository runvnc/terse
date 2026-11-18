//! Relay client (NETWORK 3, 4.3).
//!
//! [`RelayClient`] is a poll-driven state machine over a [`Transport`], so it
//! can run on a background thread here and, later, on a browser event loop
//! with a browser WebSocket transport. Replies are matched to requests by
//! `req`; a request with no reply within the timeout fails. Entries the relay
//! pushes are collected unverified for the caller ([`RelayClient::take_pushed`]).

use std::collections::HashMap;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

use crate::blob::{BlobId, MAX_BLOB};
use crate::identity::FeedKey;
use crate::protocol::{self, Reply, status};

/// Largest WebSocket message either side accepts: a full blob plus headers,
/// with room so that an oversize put still reaches the relay to be refused.
pub const MAX_MESSAGE: usize = 2 * MAX_BLOB;

/// How long a relay request may take before it fails.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A message channel to a relay.
pub trait Transport: Send {
    fn send(&mut self, message: Vec<u8>) -> io::Result<()>;
    /// The next received message, or `None` if none is ready. May wait briefly.
    fn try_recv(&mut self) -> io::Result<Option<Vec<u8>>>;
}

pub fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE))
}

/// A `ws://` WebSocket over a blocking TCP stream (no TLS).
pub struct WsTransport {
    ws: WebSocket<TcpStream>,
}

/// How long `try_recv` waits for a message.
const RECV_WAIT: Duration = Duration::from_millis(5);

fn io_error(e: tungstenite::Error) -> io::Error {
    match e {
        tungstenite::Error::Io(e) => e,
        e => io::Error::other(e),
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

impl WsTransport {
    pub fn connect(url: &str, timeout: Duration) -> io::Result<Self> {
        Self::connect_checked(url, timeout, false)
    }

    /// Connects; with `public_only`, refuses an address that is not public
    /// ([`crate::profile::public_ip`]) once the host name is resolved (for
    /// relays named in profiles, NETWORK 5).
    pub fn connect_checked(url: &str, timeout: Duration, public_only: bool) -> io::Result<Self> {
        let rest = url
            .strip_prefix("ws://")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{url}: only ws:// relays are supported")))?;
        let authority = rest.split('/').next().unwrap_or_default();
        let addr = authority
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{url}: no address")))?;
        if public_only && !crate::profile::public_ip(addr.ip()) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{url}: not a public address")));
        }
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        let (ws, _) = tungstenite::client::client_with_config(url, stream, Some(websocket_config()))
            .map_err(|e| io::Error::other(format!("{url}: WebSocket handshake failed: {e}")))?;
        ws.get_ref().set_read_timeout(Some(RECV_WAIT))?;
        Ok(WsTransport { ws })
    }

    /// The WebSocket (a relay's link to a peer uses it directly).
    pub fn into_inner(self) -> WebSocket<TcpStream> {
        self.ws
    }
}

impl Transport for WsTransport {
    fn send(&mut self, message: Vec<u8>) -> io::Result<()> {
        self.ws.send(Message::binary(message)).map_err(io_error)
    }

    fn try_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        match self.ws.read() {
            Ok(Message::Binary(b)) => Ok(Some(b.to_vec())),
            Ok(Message::Close(_)) => Err(io::ErrorKind::ConnectionAborted.into()),
            Ok(Message::Text(_)) => Err(io::Error::new(io::ErrorKind::InvalidData, "text message from relay")),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if is_timeout(&e) => Ok(None),
            Err(e) => Err(io_error(e)),
        }
    }
}

/// The outcome of one relay request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayReply {
    /// The relay's bytes for a get, not yet verified.
    Got(Vec<u8>),
    /// The ID the relay computed for a put.
    Put(BlobId),
    /// A publish or unsubscribe succeeded.
    Done,
    /// A subscribe succeeded; the relay holds entries up to `seq` `highest`
    /// (0: none).
    Subscribed { highest: u32 },
    /// A find succeeded: the entries, not yet verified.
    Found(Vec<Vec<u8>>),
    NotFound,
    Refused,
    /// Connection error, protocol error or timeout.
    Failed,
}

type Connector = Box<dyn FnMut() -> io::Result<Box<dyn Transport>> + Send>;

pub struct RelayClient {
    connect: Connector,
    transport: Option<Box<dyn Transport>>,
    timeout: Duration,
    next_req: u32,
    /// Requests sent and not yet answered, with their deadlines.
    waiting: HashMap<u32, Instant>,
    /// Outcomes not yet returned by `poll`.
    ready: Vec<(u32, RelayReply)>,
    /// Entries pushed by the relay, not yet taken.
    pushed: Vec<Vec<u8>>,
    /// Counts successful connections, so callers can tell a new one (and
    /// renew subscriptions, which a relay forgets on disconnect).
    generation: u64,
}

impl RelayClient {
    /// `connect` opens a new transport; it is called lazily and again after a
    /// connection fails.
    pub fn new(connect: Connector, timeout: Duration) -> Self {
        RelayClient {
            connect,
            transport: None,
            timeout,
            next_req: 0,
            waiting: HashMap::new(),
            ready: Vec::new(),
            pushed: Vec::new(),
            generation: 0,
        }
    }

    /// A client for a `ws://` relay URL.
    pub fn websocket(url: &str, timeout: Duration) -> Self {
        Self::websocket_checked(url, timeout, false)
    }

    /// A client for a relay named in a profile: with `public_only`, it only
    /// connects to public addresses.
    pub fn websocket_checked(url: &str, timeout: Duration, public_only: bool) -> Self {
        let url = url.to_string();
        Self::new(
            Box::new(move || Ok(Box::new(WsTransport::connect_checked(&url, timeout, public_only)?) as Box<dyn Transport>)),
            timeout,
        )
    }

    /// Searches the relay's index (NETWORK 4.7).
    pub fn find(&mut self, scope: u8, query: &[u8]) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_find(req, scope, query));
        req
    }

    /// Drops the connection (and fails every waiting request).
    pub fn close(&mut self) {
        self.disconnect();
    }

    /// Asks for a whole blob; returns the request number `poll` reports it under.
    pub fn get(&mut self, id: &BlobId) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_get(req, id));
        req
    }

    pub fn put(&mut self, data: &[u8]) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_put(req, data));
        req
    }

    /// Publishes a signed entry (NETWORK 4.3).
    pub fn publish(&mut self, entry: &[u8]) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_publish(req, entry));
        req
    }

    pub fn subscribe(&mut self, feed: &FeedKey, after: u32) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_subscribe(req, feed, after));
        req
    }

    pub fn unsubscribe(&mut self, feed: &FeedKey) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_unsubscribe(req, feed));
        req
    }

    /// Connects now if not connected.
    pub fn connect(&mut self) -> io::Result<()> {
        if self.transport.is_none() {
            self.transport = Some((self.connect)()?);
            self.generation += 1;
        }
        Ok(())
    }

    pub fn connected(&self) -> bool {
        self.transport.is_some()
    }

    /// The number of connections made so far.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Entries the relay has pushed since the last call, unverified.
    pub fn take_pushed(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pushed)
    }

    /// Whether any request is waiting for the relay.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty()
    }

    fn next_req(&mut self) -> u32 {
        self.next_req = self.next_req.wrapping_add(1);
        self.next_req
    }

    fn send(&mut self, req: u32, message: Vec<u8>) {
        if self.connect().is_err() {
            return self.ready.push((req, RelayReply::Failed));
        }
        match self.transport.as_mut().map(|t| t.send(message)) {
            Some(Ok(())) => {
                self.waiting.insert(req, Instant::now() + self.timeout);
            }
            _ => {
                self.ready.push((req, RelayReply::Failed));
                self.disconnect();
            }
        }
    }

    /// Fails every waiting request and drops the connection.
    fn disconnect(&mut self) {
        self.transport = None;
        self.ready.extend(self.waiting.drain().map(|(req, _)| (req, RelayReply::Failed)));
    }

    /// Receives what has arrived and expires late requests; returns finished
    /// requests. Waits at most briefly.
    pub fn poll(&mut self) -> Vec<(u32, RelayReply)> {
        // Read while connected, not only while busy: entries may be pushed.
        loop {
            let Some(transport) = &mut self.transport else { break };
            let message = match transport.try_recv() {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(_) => {
                    self.disconnect();
                    break;
                }
            };
            let (req, reply) = match protocol::decode_reply(&message) {
                Some(Reply::Get { req, status: status::DONE, data }) => (req, RelayReply::Got(data)),
                Some(Reply::Put { req, status: status::DONE, id: Some(id) }) => (req, RelayReply::Put(id)),
                Some(Reply::Status { req, status: status::DONE, .. }) => (req, RelayReply::Done),
                Some(Reply::Subscribed { req, highest }) => (req, RelayReply::Subscribed { highest }),
                Some(Reply::Found { req, status: status::DONE, entries }) => (req, RelayReply::Found(entries)),
                Some(Reply::Found { req, .. }) => (req, RelayReply::Refused),
                Some(Reply::Get { req, status: status::NOT_FOUND, .. }) => (req, RelayReply::NotFound),
                Some(Reply::Get { req, .. } | Reply::Put { req, .. } | Reply::Status { req, .. }) => {
                    (req, RelayReply::Refused)
                }
                Some(Reply::Entry(entry)) => {
                    self.pushed.push(entry);
                    continue;
                }
                None => {
                    self.disconnect();
                    break;
                }
            };
            // A reply to an expired request is dropped.
            if self.waiting.remove(&req).is_some() {
                self.ready.push((req, reply));
            }
        }
        let now = Instant::now();
        let late: Vec<u32> = self.waiting.iter().filter(|&(_, &d)| d <= now).map(|(&r, _)| r).collect();
        for req in late {
            self.waiting.remove(&req);
            self.ready.push((req, RelayReply::Failed));
        }
        std::mem::take(&mut self.ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A transport fed from a channel; records what was sent.
    struct Fake {
        incoming: mpsc::Receiver<Vec<u8>>,
        sent: mpsc::Sender<Vec<u8>>,
    }

    impl Transport for Fake {
        fn send(&mut self, message: Vec<u8>) -> io::Result<()> {
            self.sent.send(message).map_err(|_| io::ErrorKind::BrokenPipe.into())
        }

        fn try_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
            match self.incoming.try_recv() {
                Ok(m) => Ok(Some(m)),
                Err(mpsc::TryRecvError::Empty) => Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => Err(io::ErrorKind::ConnectionAborted.into()),
            }
        }
    }

    fn client(timeout: Duration) -> (RelayClient, mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
        let (to_client, incoming) = mpsc::channel();
        let (sent, from_client) = mpsc::channel();
        let mut fake = Some(Fake { incoming, sent });
        let connect: Connector = Box::new(move || {
            fake.take().map(|f| Box::new(f) as Box<dyn Transport>).ok_or(io::ErrorKind::ConnectionRefused.into())
        });
        (RelayClient::new(connect, timeout), to_client, from_client)
    }

    #[test]
    fn replies_in_any_order() {
        let (mut c, relay, sent) = client(DEFAULT_TIMEOUT);
        let a = c.get(&[1; 32]);
        let b = c.put(b"data");
        assert_eq!(sent.recv().unwrap(), protocol::encode_get(a, &[1; 32]));
        assert_eq!(sent.recv().unwrap(), protocol::encode_put(b, b"data"));
        assert!(c.poll().is_empty());
        relay.send(protocol::encode_put_reply(b, status::DONE, &[2; 32])).unwrap();
        relay.send(protocol::encode_get_reply(a, status::NOT_FOUND, &[])).unwrap();
        relay.send(protocol::encode_get_reply(99, status::DONE, b"stray")).unwrap();
        assert_eq!(c.poll(), [(b, RelayReply::Put([2; 32])), (a, RelayReply::NotFound)]);
        assert!(!c.busy());
    }

    #[test]
    fn timeout_and_connection_loss_fail_requests() {
        let (mut c, relay, _sent) = client(Duration::ZERO);
        let a = c.get(&[1; 32]);
        assert_eq!(c.poll(), [(a, RelayReply::Failed)]);

        let (mut c, relay2, _sent2) = client(DEFAULT_TIMEOUT);
        let a = c.get(&[1; 32]);
        relay2.send(vec![0xFF]).unwrap(); // malformed reply
        assert_eq!(c.poll(), [(a, RelayReply::Failed)]);
        // The connector has nothing left to offer: the next request fails at once.
        let b = c.get(&[1; 32]);
        assert_eq!(c.poll(), [(b, RelayReply::Failed)]);
        drop(relay);
    }

    #[test]
    fn feed_requests_and_pushed_entries() {
        let (mut c, relay, sent) = client(DEFAULT_TIMEOUT);
        assert!(!c.connected());
        c.connect().unwrap();
        assert_eq!(c.generation(), 1);
        let s = c.subscribe(&[3; 32], 7);
        let p = c.publish(b"entry");
        let u = c.unsubscribe(&[3; 32]);
        assert_eq!(sent.recv().unwrap(), protocol::encode_subscribe(s, &[3; 32], 7));
        assert_eq!(sent.recv().unwrap(), protocol::encode_publish(p, b"entry"));
        assert_eq!(sent.recv().unwrap(), protocol::encode_unsubscribe(u, &[3; 32]));
        relay.send(protocol::encode_subscribe_reply(s, status::DONE, 12)).unwrap();
        relay.send(protocol::encode_entry(b"pushed")).unwrap();
        relay.send(protocol::encode_status_reply(protocol::PUBLISH_REPLY, p, status::REFUSED)).unwrap();
        relay.send(protocol::encode_status_reply(protocol::UNSUBSCRIBE_REPLY, u, status::DONE)).unwrap();
        assert_eq!(c.poll(), [(s, RelayReply::Subscribed { highest: 12 }), (p, RelayReply::Refused), (u, RelayReply::Done)]);
        assert_eq!(c.take_pushed(), [b"pushed".to_vec()]);
        // Entries arrive while nothing is waiting, too.
        relay.send(protocol::encode_entry(b"later")).unwrap();
        assert!(c.poll().is_empty());
        assert_eq!(c.take_pushed(), [b"later".to_vec()]);
        drop(relay);
        assert!(c.poll().is_empty());
        assert!(!c.connected());
    }
}
