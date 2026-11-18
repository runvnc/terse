//! The relay (NETWORK 3, 4.3): a blob store and feed entry store reachable
//! over WebSocket. One thread per connection.
//!
//! Live entries are fanned out through a bounded queue per connection, which
//! its own thread drains between reads (reads wait at most [`POLL`]), so a
//! slow or dead client never blocks a publisher or other clients. A client
//! whose queue overflows is disconnected; it resubscribes after the highest
//! `seq` it holds when it reconnects.
//!
//! Subscribing and publishing take one lock, so a subscriber sees the
//! backlog and then live entries in order, each exactly once.

use std::collections::{HashMap, HashSet};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use terse_net::entry;
use terse_net::feeds::Insert;
use terse_net::identity::valid_feed_key;
use terse_net::protocol::{self, Malformed, Request, status};
use terse_net::relay::websocket_config;
use terse_net::{BlobStore, FeedKey, FeedStore};
use tungstenite::Message;

/// Entries kept per feed unless configured otherwise.
pub const DEFAULT_FEED_CAP: usize = 1024;
/// The least a relay may keep (NETWORK 4.3).
pub const MIN_FEED_CAP: usize = 256;
/// How long a connection thread waits for a message before sending queued
/// entries; bounds the extra latency of a live entry.
pub const POLL: Duration = Duration::from_millis(10);
/// Queued pushes beyond the feed cap a connection may fall behind by.
const QUEUE_SLACK: usize = 1024;
/// A client that cannot take a message within this time is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

struct Subscriber {
    queue: SyncSender<Vec<u8>>,
    feeds: HashSet<FeedKey>,
    /// Set when the connection is to be closed (its queue overflowed).
    dropped: Arc<AtomicBool>,
}

pub struct Relay {
    blobs: BlobStore,
    feeds: FeedStore,
    /// Entries kept per feed.
    cap: usize,
    subscribers: Mutex<Subscribers>,
}

#[derive(Default)]
struct Subscribers {
    next: u64,
    by_id: HashMap<u64, Subscriber>,
}

/// One connection's share of the relay.
pub struct Connection {
    id: u64,
    queue: Receiver<Vec<u8>>,
    dropped: Arc<AtomicBool>,
}

impl Relay {
    /// A relay on a blob store, with its feed entries beside the blobs.
    pub fn new(blobs: BlobStore, cap: usize) -> Self {
        let feeds = FeedStore::beside_blobs(blobs.dir());
        Relay { blobs, feeds, cap: cap.max(1), subscribers: Mutex::default() }
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub fn feeds(&self) -> &FeedStore {
        &self.feeds
    }

    pub fn connect(&self) -> Connection {
        let (queue, rx) = sync_channel(self.cap + QUEUE_SLACK);
        let dropped = Arc::new(AtomicBool::new(false));
        let mut subs = self.subscribers.lock().unwrap();
        subs.next += 1;
        let id = subs.next;
        subs.by_id.insert(id, Subscriber { queue, feeds: HashSet::new(), dropped: dropped.clone() });
        Connection { id, queue: rx, dropped }
    }

    pub fn disconnect(&self, conn: &Connection) {
        self.subscribers.lock().unwrap().by_id.remove(&conn.id);
    }

    /// The reply to one binary message, or `None` to close the connection.
    /// Entries for subscriptions are queued on `conn`.
    pub fn respond(&self, conn: &Connection, message: &[u8]) -> Option<Vec<u8>> {
        Some(match protocol::decode_request(message) {
            Ok(Request::Get { req, id }) => match self.blobs.read(&id) {
                Ok(Some(data)) => protocol::encode_get_reply(req, status::DONE, &data),
                _ => protocol::encode_get_reply(req, status::NOT_FOUND, &[]),
            },
            // Empty, oversize and over-limit blobs are all refused by the store.
            Ok(Request::Put { req, data }) => match self.blobs.put(data) {
                Ok(id) => protocol::encode_put_reply(req, status::DONE, &id),
                Err(_) => protocol::encode_put_reply(req, status::REFUSED, &[0; 32]),
            },
            Ok(Request::Publish { req, entry }) => {
                protocol::encode_status_reply(protocol::PUBLISH_REPLY, req, self.publish(entry))
            }
            Ok(Request::Subscribe { req, feed, after }) => {
                protocol::encode_status_reply(protocol::SUBSCRIBE_REPLY, req, self.subscribe(conn, feed, after))
            }
            Ok(Request::Unsubscribe { req, feed }) => {
                if let Some(s) = self.subscribers.lock().unwrap().by_id.get_mut(&conn.id) {
                    s.feeds.remove(&feed);
                }
                protocol::encode_status_reply(protocol::UNSUBSCRIBE_REPLY, req, status::DONE)
            }
            Err(Malformed::Refuse { reply: protocol::GET_REPLY, req }) => {
                protocol::encode_get_reply(req, status::REFUSED, &[])
            }
            Err(Malformed::Refuse { reply: protocol::PUT_REPLY, req }) => {
                protocol::encode_put_reply(req, status::REFUSED, &[0; 32])
            }
            Err(Malformed::Refuse { reply, req }) => protocol::encode_status_reply(reply, req, status::REFUSED),
            Err(Malformed::Close) => return None,
        })
    }

    /// Verifies, stores and pushes an entry (NETWORK 4.3); returns the status.
    pub fn publish(&self, bytes: &[u8]) -> u8 {
        let Ok(signed) = entry::decode(bytes) else { return status::REFUSED };
        let feed = signed.entry.feed;
        let mut subs = self.subscribers.lock().unwrap();
        let Ok(seqs) = self.feeds.seqs(&feed) else { return status::REFUSED };
        // An entry older than everything kept in a full feed would be pruned
        // at once; storing it could let it replace a pruned entry.
        if seqs.len() >= self.cap && seqs.first().is_some_and(|&low| signed.entry.seq < low) {
            return status::REFUSED;
        }
        match self.feeds.insert(&signed) {
            Ok(Insert::New) => {}
            Ok(Insert::Duplicate) => return status::DONE,
            Ok(Insert::Conflict) | Err(_) => return status::REFUSED,
        }
        // A failed prune only means keeping more than the cap.
        let _ = self.feeds.prune(&feed, self.cap);
        let message = protocol::encode_entry(&signed.bytes);
        subs.by_id.retain(|_, s| !s.feeds.contains(&feed) || push(s, message.clone()));
        status::DONE
    }

    fn subscribe(&self, conn: &Connection, feed: FeedKey, after: u32) -> u8 {
        if !valid_feed_key(&feed) {
            return status::REFUSED;
        }
        let mut subs = self.subscribers.lock().unwrap();
        let Ok(backlog) = self.feeds.after(&feed, after) else { return status::REFUSED };
        let Some(s) = subs.by_id.get_mut(&conn.id) else { return status::REFUSED };
        s.feeds.insert(feed);
        for bytes in backlog {
            if !push(s, protocol::encode_entry(&bytes)) {
                subs.by_id.remove(&conn.id);
                break;
            }
        }
        status::DONE
    }
}

/// Queues a message without waiting; false (and the connection marked for
/// closing) if the subscriber cannot take it.
fn push(s: &Subscriber, message: Vec<u8>) -> bool {
    match s.queue.try_send(message) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            s.dropped.store(true, Ordering::Relaxed);
            false
        }
    }
}

/// Accepts connections until the listener fails.
pub fn serve(listener: TcpListener, relay: Arc<Relay>) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        let relay = relay.clone();
        thread::spawn(move || {
            let peer = stream.peer_addr().map_or("?".into(), |a| a.to_string());
            if let Err(e) = handle(stream, &relay) {
                eprintln!("terse-relay: {peer}: {e}");
            }
        });
    }
    Ok(())
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

fn handle(stream: TcpStream, relay: &Relay) -> Result<(), tungstenite::Error> {
    stream.set_nodelay(true)?;
    let mut ws = tungstenite::accept_with_config(stream, Some(websocket_config())).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => e,
        tungstenite::HandshakeError::Interrupted(_) => tungstenite::Error::ConnectionClosed,
    })?;
    ws.get_ref().set_read_timeout(Some(POLL))?;
    ws.get_ref().set_write_timeout(Some(WRITE_TIMEOUT))?;
    let conn = relay.connect();
    let result = serve_connection(&mut ws, relay, &conn);
    relay.disconnect(&conn);
    result
}

fn serve_connection(
    ws: &mut tungstenite::WebSocket<TcpStream>,
    relay: &Relay,
    conn: &Connection,
) -> Result<(), tungstenite::Error> {
    loop {
        if conn.dropped.load(Ordering::Relaxed) {
            break;
        }
        for message in conn.queue.try_iter() {
            ws.send(Message::binary(message))?;
        }
        let reply = match ws.read() {
            Ok(Message::Binary(m)) => relay.respond(conn, &m),
            // NETWORK 3: all messages are binary.
            Ok(Message::Text(_)) => None,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if is_timeout(&e) => continue,
            Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(e),
        };
        match reply {
            Some(r) => ws.send(Message::binary(r))?,
            None => break,
        }
    }
    ws.close(None)?;
    // Drain until the peer acknowledges the close.
    while ws.read().is_ok() {}
    Ok(())
}
