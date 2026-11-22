//! Relay client (NETWORK 3, 4.3).
//!
//! [`RelayClient`] is a poll-driven state machine over a [`Transport`], so it
//! can run on a background thread here and, later, on a browser event loop
//! with a browser WebSocket transport. Replies are matched to requests by
//! `req`; a request with no reply within the timeout fails. Entries the relay
//! pushes are collected unverified for the caller ([`RelayClient::take_pushed`]).
//!
//! A WebSocket client connects on a thread of its own ([`RelayClient::start_connect`]),
//! so a relay that does not answer never holds up the thread that polls the
//! client; `poll` takes in the outcome of the attempt.

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

use crate::blob::{BlobId, MAX_BLOB};
use crate::identity::FeedKey;
use crate::protocol::{self, Refusal, Reply, status};
use crate::tls::{self, Stream};

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

    /// The next received message if one is already here, without waiting
    /// (for the messages after the first in one poll).
    fn try_recv_now(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.try_recv()
    }
}

pub fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE))
}

/// A `ws://` or `wss://` WebSocket over a blocking TCP stream.
pub struct WsTransport {
    ws: WebSocket<Stream>,
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

/// Where a relay URL points: whether over TLS, the host as the URL writes
/// it (an IPv6 address in brackets) and the port (80 for `ws://` and 443
/// for `wss://` when it gives none, NETWORK 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub tls: bool,
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn parse(url: &str) -> io::Result<Self> {
        let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidInput, format!("{url}: {why}"));
        let (tls, rest) = match (url.strip_prefix("ws://"), url.strip_prefix("wss://")) {
            (Some(r), _) => (false, r),
            (_, Some(r)) => (true, r),
            _ => return Err(bad("not a ws:// or wss:// URL")),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let authority = authority.rsplit('@').next().unwrap_or_default();
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (inside, after) = v6.split_once(']').ok_or_else(|| bad("no ] after the IPv6 address"))?;
            (format!("[{inside}]"), after.strip_prefix(':'))
        } else {
            match authority.split_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty() || host == "[]" {
            return Err(bad("no host"));
        }
        let port = match port {
            None | Some("") if tls => 443,
            None | Some("") => 80,
            Some(p) => p.parse().map_err(|_| bad("bad port"))?,
        };
        Ok(Endpoint { tls, host, port })
    }

    /// `host:port`, for resolving.
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// The words for a plain client that met a relay requiring TLS.
const REQUIRES_TLS: &str = "this relay requires TLS (try wss://)";

/// Why a WebSocket handshake failed, in words: a TLS answer to a plain
/// request or a redirect to `https://` (the relay requires TLS), or an
/// HTTP refusal.
fn handshake_failure(url: &str, e: tungstenite::Error, sniffed: Option<u8>, tls: bool) -> io::Error {
    use tungstenite::Error as E;
    if !tls && sniffed.is_some_and(tls::is_tls_record) {
        return io::Error::new(io::ErrorKind::InvalidData, format!("{url}: {REQUIRES_TLS}"));
    }
    match e {
        E::Http(r) => {
            let location = r.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or_default();
            if !tls && r.status().is_redirection() && (location.starts_with("https://") || location.starts_with("wss://")) {
                return io::Error::new(io::ErrorKind::InvalidData, format!("{url}: {REQUIRES_TLS}"));
            }
            // A TLS server's refusal of plain HTTP (nginx's and Cloudflare's
            // "The plain HTTP request was sent to HTTPS port", Go's "Client
            // sent an HTTP request to an HTTPS server").
            let body = String::from_utf8_lossy(r.body().as_deref().unwrap_or_default()).to_ascii_lowercase();
            if !tls && r.status() == 400 && ["https", "tls", "ssl"].iter().any(|w| body.contains(w)) {
                return io::Error::new(io::ErrorKind::InvalidData, format!("{url}: {REQUIRES_TLS}"));
            }
            // NETWORK 3: 429 and 503 are a relay refusing for its limits.
            let words = match r.status().as_u16() {
                429 => "too many connections from this address; try again later".to_string(),
                503 => "the relay has too many connections; try again later".into(),
                _ => format!("WebSocket handshake refused (HTTP {})", r.status()),
            };
            io::Error::new(io::ErrorKind::ConnectionRefused, format!("{url}: {words}"))
        }
        E::Io(e) => io::Error::new(e.kind(), format!("{url}: WebSocket handshake failed: {e}")),
        e => io::Error::other(format!("{url}: WebSocket handshake failed: {e}")),
    }
}

impl WsTransport {
    pub fn connect(url: &str, timeout: Duration) -> io::Result<Self> {
        Self::connect_checked(url, timeout, false)
    }

    /// Connects; with `public_only`, refuses an address that is not public
    /// ([`crate::profile::public_ip`]) once the host name is resolved (for
    /// relays named in profiles, NETWORK 5). A `wss://` relay's
    /// certificate must be trusted and be for its host ([`crate::tls`]).
    /// Each step, the TLS handshake included, waits at most `timeout`;
    /// [`RelayClient`] runs all of it on a thread of its own.
    pub fn connect_checked(url: &str, timeout: Duration, public_only: bool) -> io::Result<Self> {
        let end = Endpoint::parse(url)?;
        let addrs: Vec<SocketAddr> =
            end.authority().to_socket_addrs().map_err(|e| io::Error::new(e.kind(), format!("{url}: {e}")))?.collect();
        check_addrs(&addrs, public_only).map_err(|e| io::Error::new(e.kind(), format!("{url}: {e}")))?;
        // Only the addresses checked above are used; the name is not
        // resolved again.
        let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{url}: no address"));
        let mut tcp = None;
        for addr in &addrs {
            match TcpStream::connect_timeout(addr, timeout) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) => last = io::Error::new(e.kind(), format!("{url}: {e}")),
            }
        }
        let tcp = tcp.ok_or(last)?;
        tcp.set_read_timeout(Some(timeout))?;
        tcp.set_write_timeout(Some(timeout))?;
        tcp.set_nodelay(true)?;
        let stream = if end.tls {
            tls::connect(&end.host, tcp).map_err(|e| io::Error::new(e.kind(), format!("{url}: {e}")))?
        } else {
            tls::take_sniffed();
            Stream::Plain { tcp, sniff: true }
        };
        let (mut ws, _) = tungstenite::client::client_with_config(url, stream, Some(websocket_config())).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(e) => handshake_failure(url, e, tls::take_sniffed(), end.tls),
            tungstenite::HandshakeError::Interrupted(_) => {
                io::Error::new(io::ErrorKind::TimedOut, format!("{url}: WebSocket handshake timed out"))
            }
        })?;
        ws.get_mut().stop_sniffing();
        ws.get_ref().tcp().set_read_timeout(Some(RECV_WAIT))?;
        Ok(WsTransport { ws })
    }

    /// The WebSocket (a relay's link to a peer uses it directly).
    pub fn into_inner(self) -> WebSocket<Stream> {
        self.ws
    }
}

/// Checks the resolved addresses of a relay: there must be one and, with
/// `public_only`, every one must be public ([`crate::profile::public_ip`]),
/// so a name that resolves to a private address among public ones is
/// refused (NETWORK 5).
pub fn check_addrs(addrs: &[SocketAddr], public_only: bool) -> io::Result<()> {
    if addrs.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no address"));
    }
    if public_only && addrs.iter().any(|a| !crate::profile::public_ip(a.ip())) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not a public address"));
    }
    Ok(())
}

impl WsTransport {
    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
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

impl Transport for WsTransport {
    fn send(&mut self, message: Vec<u8>) -> io::Result<()> {
        self.ws.send(Message::binary(message)).map_err(io_error)
    }

    fn try_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.read()
    }

    /// Without the read timeout's wait: after one message, `poll` takes the
    /// others already here and returns (waiting `RECV_WAIT` once more after
    /// every reply added that much to every request).
    fn try_recv_now(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.ws.get_ref().tcp().set_nonblocking(true)?;
        let got = self.read();
        self.ws.get_ref().tcp().set_nonblocking(false)?;
        got
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
    /// A lookup found an entry, not yet verified.
    LookedUp(Vec<u8>),
    NotFound,
    /// The relay refused, and why (NETWORK 3.1).
    Refused(Refusal),
    /// Connection error, protocol error or timeout.
    Failed,
}

type Connector = Box<dyn FnMut() -> io::Result<Box<dyn Transport>> + Send>;
type Attempt = (Connector, io::Result<Box<dyn Transport>>);

pub struct RelayClient {
    /// `None` while a background attempt has it.
    connect: Option<Connector>,
    /// Connect on a thread of its own ([`Self::start_connect`]).
    background: bool,
    /// The background attempt in progress.
    attempt: Option<mpsc::Receiver<Attempt>>,
    /// Why the last attempt failed, not yet taken.
    failure: Option<io::Error>,
    /// Why the last attempt failed, kept until one succeeds.
    last_failure: Option<String>,
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
            connect: Some(connect),
            background: false,
            attempt: None,
            failure: None,
            last_failure: None,
            transport: None,
            timeout,
            next_req: 0,
            waiting: HashMap::new(),
            ready: Vec::new(),
            pushed: Vec::new(),
            generation: 0,
        }
    }

    /// A client for a `ws://` relay URL. It connects in the background
    /// ([`Self::start_connect`]).
    pub fn websocket(url: &str, timeout: Duration) -> Self {
        Self::websocket_checked(url, timeout, false)
    }

    /// A client for a relay named in a profile: with `public_only`, it only
    /// connects to public addresses.
    pub fn websocket_checked(url: &str, timeout: Duration, public_only: bool) -> Self {
        let url = url.to_string();
        let mut client = Self::new(
            Box::new(move || Ok(Box::new(WsTransport::connect_checked(&url, timeout, public_only)?) as Box<dyn Transport>)),
            timeout,
        );
        client.background = true;
        client
    }

    /// Searches the relay's index (NETWORK 4.7).
    pub fn find(&mut self, scope: u8, query: &[u8]) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_find(req, scope, query));
        req
    }

    /// Asks for a feed's current entry of a kind and name (NETWORK 4.8).
    pub fn lookup(&mut self, feed: &FeedKey, kind: u8, name: &[u8]) -> u32 {
        let req = self.next_req();
        self.send(req, protocol::encode_lookup(req, feed, kind, name));
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

    /// Connects now if not connected, waiting for an attempt in progress.
    pub fn connect(&mut self) -> io::Result<()> {
        if let Some(rx) = self.attempt.take() {
            let (connect, result) = rx.recv().map_err(|_| io::Error::other("connect thread lost"))?;
            self.connect = Some(connect);
            return result.map(|t| {
                self.transport = Some(t);
                self.generation += 1;
            });
        }
        self.failure = None;
        if self.transport.is_none() {
            let connect = self.connect.as_mut().ok_or_else(|| io::Error::other("no connector"))?;
            self.transport = Some(connect()?);
            self.generation += 1;
        }
        Ok(())
    }

    /// Starts connecting if not connected and no attempt is in progress: on
    /// a thread of its own for a WebSocket client (the outcome is taken in
    /// by `poll`), else at once. A failure is kept for
    /// [`Self::take_connect_failure`].
    pub fn start_connect(&mut self) {
        if self.transport.is_some() || self.attempt.is_some() {
            return;
        }
        let Some(mut connect) = self.connect.take() else { return };
        if !self.background {
            let result = connect();
            self.connect = Some(connect);
            return self.finish(result);
        }
        let (tx, rx) = mpsc::channel();
        self.attempt = Some(rx);
        std::thread::spawn(move || {
            let result = connect();
            let _ = tx.send((connect, result));
        });
    }

    /// Whether a background attempt to connect is in progress.
    pub fn connecting(&self) -> bool {
        self.attempt.is_some()
    }

    /// Why the last attempt to connect failed, once.
    pub fn take_connect_failure(&mut self) -> Option<io::Error> {
        self.failure.take()
    }

    fn finish(&mut self, result: io::Result<Box<dyn Transport>>) {
        match result {
            Ok(t) => {
                self.transport = Some(t);
                self.generation += 1;
                self.last_failure = None;
            }
            Err(e) => {
                self.last_failure = Some(e.to_string());
                self.failure = Some(e);
            }
        }
    }

    /// Why the last attempt to connect failed, until one succeeds (an
    /// untrusted certificate, say: README "Relays over TLS").
    pub fn last_failure(&self) -> Option<&str> {
        self.last_failure.as_deref()
    }

    /// Takes in the outcome of a background attempt, waiting at most `wait`.
    fn check_attempt(&mut self, wait: Duration) {
        let Some(rx) = &self.attempt else { return };
        match rx.recv_timeout(wait) {
            Ok((connect, result)) => {
                self.attempt = None;
                self.connect = Some(connect);
                self.finish(result);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.attempt = None;
                self.failure = Some(io::Error::other("connect thread lost"));
            }
        }
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
        // A background client is connected by `start_connect` only, so a
        // request never waits for a connection.
        let connected = if self.background { self.transport.is_some() } else { self.connect().is_ok() };
        if !connected {
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
        self.check_attempt(RECV_WAIT);
        // Read while connected, not only while busy: entries may be pushed.
        let mut first = true;
        loop {
            let Some(transport) = &mut self.transport else { break };
            let next = if first { transport.try_recv() } else { transport.try_recv_now() };
            first = false;
            let message = match next {
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
                Some(Reply::Found { req, status, .. }) => (req, RelayReply::Refused(Refusal::of_status(status))),
                Some(Reply::LookedUp { req, entry: Some(entry), .. }) => (req, RelayReply::LookedUp(entry)),
                Some(Reply::LookedUp { req, status: status::NOT_FOUND, .. }) => (req, RelayReply::NotFound),
                Some(Reply::LookedUp { req, status, .. }) => (req, RelayReply::Refused(Refusal::of_status(status))),
                Some(Reply::Get { req, status: status::NOT_FOUND, .. }) => (req, RelayReply::NotFound),
                Some(Reply::Get { req, status, .. } | Reply::Put { req, status, .. } | Reply::Status { req, status, .. }) => {
                    (req, RelayReply::Refused(Refusal::of_status(status)))
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
        assert_eq!(
            c.poll(),
            [(s, RelayReply::Subscribed { highest: 12 }), (p, RelayReply::Refused(Refusal::Unspecified)), (u, RelayReply::Done)]
        );
        // The reason of a refusal is kept (NETWORK 3.1).
        let p = c.publish(b"entry");
        let q = c.put(b"blob");
        relay.send(protocol::encode_status_reply(protocol::PUBLISH_REPLY, p, status::NOT_WRITER)).unwrap();
        relay.send(protocol::encode_put_reply(q, status::retry(Duration::from_secs(30)), &[0; 32])).unwrap();
        assert_eq!(
            c.poll(),
            [(p, RelayReply::Refused(Refusal::NotWriter)), (q, RelayReply::Refused(Refusal::TryLater(Duration::from_secs(30))))]
        );
        sent.try_iter().count();
        assert_eq!(c.take_pushed(), [b"pushed".to_vec()]);
        // Entries arrive while nothing is waiting, too.
        relay.send(protocol::encode_entry(b"later")).unwrap();
        assert!(c.poll().is_empty());
        assert_eq!(c.take_pushed(), [b"later".to_vec()]);
        drop(relay);
        assert!(c.poll().is_empty());
        assert!(!c.connected());
    }

    /// NETWORK 5: a feed relay is refused if any of its addresses is not
    /// public.
    #[test]
    fn every_resolved_address_is_checked() {
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        let public = [addr("93.184.216.34:80"), addr("[2606:2800:220:1::1]:80")];
        assert!(check_addrs(&public, true).is_ok());
        for bad in ["127.0.0.1:80", "10.1.2.3:80", "192.168.0.1:80", "169.254.1.1:80", "[::1]:80", "[fe80::1]:80", "[::ffff:10.0.0.1]:80"] {
            let mixed = [public[0], addr(bad), public[1]];
            assert_eq!(check_addrs(&mixed, true).unwrap_err().kind(), io::ErrorKind::PermissionDenied, "{bad}");
            assert!(check_addrs(&mixed, false).is_ok(), "own relays may be anywhere");
        }
        assert_eq!(check_addrs(&[], false).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    /// A background client connects on its own thread; requests made
    /// meanwhile fail at once instead of waiting.
    #[test]
    fn background_connect() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let mut c = RelayClient::websocket(&url, Duration::from_millis(200));
        c.start_connect();
        assert!(c.connecting() && !c.connected());
        let start = Instant::now();
        let a = c.get(&[1; 32]);
        assert_eq!(c.poll(), [(a, RelayReply::Failed)]);
        assert!(start.elapsed() < Duration::from_millis(100));
        // Nobody answers the handshake: the attempt fails at the timeout.
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.connecting() {
            assert!(Instant::now() < deadline);
            c.poll();
        }
        assert!(!c.connected());
        assert!(c.take_connect_failure().is_some());
        assert!(c.take_connect_failure().is_none());
        drop(listener);
    }
}
