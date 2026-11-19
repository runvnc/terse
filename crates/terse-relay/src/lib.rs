//! The relay (NETWORK 3, 4.3, 4.7, 5): a blob store and feed entry store
//! reachable over WebSocket. One thread per connection.
//!
//! Every message for a connection (live entries, and requests this relay
//! sends to a peer) goes through a bounded queue per connection, which its
//! own thread drains between reads (reads wait at most [`POLL`]), so a slow
//! or dead client never blocks a publisher or other clients. A client whose
//! queue overflows is disconnected; it resubscribes after the highest `seq`
//! it holds when it reconnects.
//!
//! Subscribing and publishing take one lock, so a subscriber sees the
//! backlog and then live entries in order, each exactly once.
//!
//! Search (NETWORK 4.7): entries are found in the order this relay stored
//! them, newest first, by an arrival log beside the entries (the host's
//! [`terse_net::timeline::Arrivals`]); entries stored before the log
//! existed are added to it at start, by feed key and then `seq`. A
//! connection may search at most [`FINDS_PER_SECOND`] times in any second;
//! further finds are refused. Lookups (NETWORK 4.8) count as finds.
//!
//! Peers (NETWORK 5): a connection on which a Peer message arrived, or one
//! this relay opened to a `--peer` URL. Both sides may send requests on it.
//! Every entry and every blob up to 16,384 bytes this relay newly stores is
//! sent to every peer but the one it came from; a client's subscription is
//! repeated on the peers; a get this relay cannot serve for a client is
//! asked of the peers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use terse_net::blob::{BlobId, blob_id, valid_size};
use terse_net::entry::{self, KIND_ARTICLE, KIND_DOCUMENT, KIND_PROFILE, KIND_PROGRAM};
use terse_net::feeds::Insert;
use terse_net::identity::valid_feed_key;
use terse_net::profile::{self, Document};
use terse_net::protocol::{self, MAX_FIND_RESULTS, Malformed, Reply, Request, SCOPE_ENTRIES, SCOPE_FEEDS, SCOPE_PROGRAMS, status};
use terse_net::relay::websocket_config;
use terse_net::timeline::Arrivals;
use terse_net::{BlobStore, FeedKey, FeedStore};
use tungstenite::Message;

/// Entries kept per feed unless configured otherwise.
pub const DEFAULT_FEED_CAP: usize = 1024;
/// The least a relay may keep (NETWORK 4.3).
pub const MIN_FEED_CAP: usize = 256;
/// How long a connection thread waits for a message before sending queued
/// entries; bounds the extra latency of a live entry.
pub const POLL: Duration = Duration::from_millis(10);
/// Blobs up to this size are pushed to peers (NETWORK 5).
pub const PUSH_LIMIT: usize = 16_384;
/// How long a get waits for the peers (NETWORK 5).
pub const PULL_TIMEOUT: Duration = Duration::from_secs(3);
/// Finds a connection may make in any one second (NETWORK 4.7: a relay may
/// refuse more).
pub const FINDS_PER_SECOND: usize = 4;
/// Queued pushes beyond the feed cap a connection may fall behind by.
const QUEUE_SLACK: usize = 1024;
/// A client that cannot take a message within this time is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

struct Conn {
    queue: SyncSender<Vec<u8>>,
    /// Feeds this connection subscribed to.
    feeds: HashSet<FeedKey>,
    /// Set when the connection is to be closed (its queue overflowed).
    dropped: Arc<AtomicBool>,
    peer: Arc<AtomicBool>,
    /// For a peer: the last `req` this relay used on it, the gets it sent
    /// (by `req`, the pull they serve) and the feeds it subscribed to.
    next_req: u32,
    pulls_sent: HashMap<u32, u64>,
    peer_feeds: HashSet<FeedKey>,
}

/// A client's get that the peers were asked for.
struct Pull {
    client: u64,
    req: u32,
    id: BlobId,
    waiting: usize,
    deadline: Instant,
}

#[derive(Default)]
struct State {
    next: u64,
    conns: HashMap<u64, Conn>,
    pulls: HashMap<u64, Pull>,
    next_pull: u64,
}

/// The latest profile entry of a feed (NETWORK 4.6, 4.7).
struct Profile {
    seq: u32,
    bytes: Vec<u8>,
    blob: BlobId,
    name: String,
    /// `None` until the profile document is in the store.
    listed: Option<bool>,
}

pub struct Relay {
    blobs: BlobStore,
    feeds: FeedStore,
    /// Entries kept per feed.
    cap: usize,
    state: Mutex<State>,
    profiles: Mutex<HashMap<FeedKey, Profile>>,
    /// The order in which entries were stored.
    arrivals: Mutex<Arrivals>,
    /// Messages written to peer connections, by type.
    peer_messages: [AtomicUsize; 256],
}

/// One connection's share of the relay.
pub struct Connection {
    id: u64,
    queue: Receiver<Vec<u8>>,
    dropped: Arc<AtomicBool>,
    peer: Arc<AtomicBool>,
    /// When this connection's recent finds were answered.
    finds: Mutex<VecDeque<Instant>>,
}

/// What to send back for a message.
pub enum Answer {
    Reply(Vec<u8>),
    /// Nothing now (a pushed entry, a Peer message, an unknown type, or a
    /// get the peers were asked for).
    Nothing,
    Close,
}

impl Relay {
    /// A relay on a blob store, with its feed entries beside the blobs.
    pub fn new(blobs: BlobStore, cap: usize) -> Self {
        let feeds = FeedStore::beside_blobs(blobs.dir());
        let mut profiles = HashMap::new();
        for feed in feeds.feeds().unwrap_or_default() {
            if let Ok(Some(p)) = feeds.latest_profile(&feed) {
                profiles.insert(feed, profile_of(&p));
            }
        }
        // Entries stored without an arrival record (by an older relay) are
        // recorded now. A failure only leaves them out of searches.
        let mut arrivals = Arrivals::beside_blobs(blobs.dir());
        let logged: HashSet<(FeedKey, u32)> = arrivals.order().map(|o| o.iter().copied().collect()).unwrap_or_default();
        let mut stored = feeds.feeds().unwrap_or_default();
        stored.sort();
        for feed in stored {
            for seq in feeds.seqs(&feed).unwrap_or_default() {
                if !logged.contains(&(feed, seq)) {
                    let _ = arrivals.record(&feed, seq);
                }
            }
        }
        Relay {
            arrivals: Mutex::new(arrivals),
            blobs,
            feeds,
            cap: cap.max(1),
            state: Mutex::default(),
            profiles: Mutex::new(profiles),
            peer_messages: std::array::from_fn(|_| AtomicUsize::new(0)),
        }
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub fn feeds(&self) -> &FeedStore {
        &self.feeds
    }

    /// How many messages of type `kind` this relay has written to peers.
    pub fn peer_messages(&self, kind: u8) -> usize {
        self.peer_messages[kind as usize].load(Ordering::Relaxed)
    }

    /// The number of connections marked as peers.
    pub fn peer_count(&self) -> usize {
        self.state.lock().unwrap().conns.values().filter(|c| c.peer.load(Ordering::Relaxed)).count()
    }

    pub fn connect(&self) -> Connection {
        let (queue, rx) = sync_channel(self.cap + QUEUE_SLACK);
        let dropped = Arc::new(AtomicBool::new(false));
        let peer = Arc::new(AtomicBool::new(false));
        let mut state = self.state.lock().unwrap();
        state.next += 1;
        let id = state.next;
        let conn = Conn {
            queue,
            feeds: HashSet::new(),
            dropped: dropped.clone(),
            peer: peer.clone(),
            next_req: 0,
            pulls_sent: HashMap::new(),
            peer_feeds: HashSet::new(),
        };
        state.conns.insert(id, conn);
        Connection { id, queue: rx, dropped, peer, finds: Mutex::default() }
    }

    pub fn disconnect(&self, conn: &Connection) {
        self.state.lock().unwrap().conns.remove(&conn.id);
    }

    /// Marks a connection as a peer (NETWORK 5) and repeats the clients'
    /// subscriptions on it.
    pub fn mark_peer(&self, conn: &Connection) {
        let mut state = self.state.lock().unwrap();
        conn.peer.store(true, Ordering::Relaxed);
        let wanted: HashSet<FeedKey> =
            state.conns.values().filter(|c| !c.peer.load(Ordering::Relaxed)).flat_map(|c| c.feeds.iter().copied()).collect();
        let Some(c) = state.conns.get_mut(&conn.id) else { return };
        for feed in wanted {
            let after = self.feeds.highest(&feed).unwrap_or(0);
            peer_subscribe(c, feed, after);
        }
    }

    /// The answer to one binary message. Entries for subscriptions and
    /// replies to gets asked of peers are queued on `conn`.
    pub fn respond(&self, conn: &Connection, message: &[u8]) -> Answer {
        let peer = conn.peer.load(Ordering::Relaxed);
        if message.first().is_some_and(|&k| k >= 0x80) {
            if peer {
                self.from_peer(conn, message);
            }
            return Answer::Nothing;
        }
        Answer::Reply(match protocol::decode_request(message) {
            Ok(Request::Get { req, id }) => match self.blobs.read(&id) {
                Ok(Some(data)) => protocol::encode_get_reply(req, status::DONE, &data),
                // NETWORK 5: a client's get is asked of the peers; a peer's
                // is answered from this store only.
                _ if !peer && self.pull(conn, req, id) => return Answer::Nothing,
                _ => protocol::encode_get_reply(req, status::NOT_FOUND, &[]),
            },
            // Empty, oversize and over-limit blobs are all refused by the store.
            Ok(Request::Put { req, data }) => match self.blobs.put_new(data) {
                Ok((id, new)) => {
                    if new && data.len() <= PUSH_LIMIT {
                        self.push_to_peers(Some(conn.id), |r| protocol::encode_put(r, data));
                    }
                    protocol::encode_put_reply(req, status::DONE, &id)
                }
                Err(_) => protocol::encode_put_reply(req, status::REFUSED, &[0; 32]),
            },
            Ok(Request::Publish { req, entry }) => {
                protocol::encode_status_reply(protocol::PUBLISH_REPLY, req, self.publish_from(entry, Some(conn.id)))
            }
            Ok(Request::Subscribe { req, feed, after }) => match self.subscribe(conn, feed, after) {
                Some(highest) => protocol::encode_subscribe_reply(req, status::DONE, highest),
                None => protocol::encode_subscribe_reply(req, status::REFUSED, 0),
            },
            Ok(Request::Unsubscribe { req, feed }) => {
                if let Some(c) = self.state.lock().unwrap().conns.get_mut(&conn.id) {
                    c.feeds.remove(&feed);
                }
                protocol::encode_status_reply(protocol::UNSUBSCRIBE_REPLY, req, status::DONE)
            }
            Ok(Request::Find { req, scope, query }) => match (valid_find(scope, query) && self.allow_find(conn)).then(|| self.find(scope, query)).flatten() {
                Some(found) => protocol::encode_find_reply(req, status::DONE, &found),
                None => protocol::encode_find_reply::<&[u8]>(req, status::REFUSED, &[]),
            },
            // NETWORK 4.8: answered for any feed, listed or not; counted
            // with the finds.
            Ok(Request::Lookup { req, feed, kind, name }) => {
                if kind != KIND_PROGRAM || !std::str::from_utf8(name).is_ok_and(profile::valid_name) || !self.allow_find(conn) {
                    protocol::encode_lookup_reply(req, status::REFUSED, &[])
                } else {
                    match self.lookup(&feed, name) {
                        Some(bytes) => protocol::encode_lookup_reply(req, status::DONE, &bytes),
                        None => protocol::encode_lookup_reply(req, status::NOT_FOUND, &[]),
                    }
                }
            }
            Ok(Request::Peer) => {
                self.mark_peer(conn);
                return Answer::Nothing;
            }
            Err(Malformed::Unknown) => return Answer::Nothing,
            Err(Malformed::Refuse { reply: protocol::GET_REPLY, req }) => {
                protocol::encode_get_reply(req, status::REFUSED, &[])
            }
            Err(Malformed::Refuse { reply: protocol::PUT_REPLY, req }) => {
                protocol::encode_put_reply(req, status::REFUSED, &[0; 32])
            }
            Err(Malformed::Refuse { reply: protocol::LOOKUP_REPLY, req }) => {
                protocol::encode_lookup_reply(req, status::REFUSED, &[])
            }
            Err(Malformed::Refuse { reply: protocol::FIND_REPLY, req }) => {
                protocol::encode_find_reply::<&[u8]>(req, status::REFUSED, &[])
            }
            Err(Malformed::Refuse { reply, req }) => protocol::encode_status_reply(reply, req, status::REFUSED),
            Err(Malformed::Close) => return Answer::Close,
        })
    }

    /// Whether `conn` may search now: fewer than [`FINDS_PER_SECOND`] finds
    /// answered in the last second. An allowed find is counted; refused and
    /// malformed ones are not.
    fn allow_find(&self, conn: &Connection) -> bool {
        let now = Instant::now();
        let mut finds = conn.finds.lock().unwrap();
        while finds.front().is_some_and(|&t| now.duration_since(t) >= Duration::from_secs(1)) {
            finds.pop_front();
        }
        if finds.len() >= FINDS_PER_SECOND {
            return false;
        }
        finds.push_back(now);
        true
    }

    /// A reply or pushed entry on a peer connection.
    fn from_peer(&self, conn: &Connection, message: &[u8]) {
        match protocol::decode_reply(message) {
            Some(Reply::Entry(bytes)) => {
                self.publish_from(&bytes, Some(conn.id));
            }
            Some(Reply::Get { req, status, data }) => {
                let pull = self.state.lock().unwrap().conns.get_mut(&conn.id).and_then(|c| c.pulls_sent.remove(&req));
                if let Some(pull) = pull {
                    self.pulled(pull, conn.id, (status == status::DONE).then_some(data));
                }
            }
            // Replies to what this relay sent (publish, put, subscribe) need
            // nothing more.
            _ => {}
        }
    }

    /// Asks every peer for a blob a client wants; false if there are none.
    fn pull(&self, conn: &Connection, req: u32, id: BlobId) -> bool {
        let mut state = self.state.lock().unwrap();
        state.next_pull += 1;
        let pull_id = state.next_pull;
        let mut asked = 0;
        for (&cid, c) in state.conns.iter_mut() {
            if cid == conn.id || !c.peer.load(Ordering::Relaxed) {
                continue;
            }
            c.next_req = c.next_req.wrapping_add(1);
            let r = c.next_req;
            if push(c, protocol::encode_get(r, &id)) {
                c.pulls_sent.insert(r, pull_id);
                asked += 1;
            }
        }
        if asked > 0 {
            let deadline = Instant::now() + PULL_TIMEOUT;
            state.pulls.insert(pull_id, Pull { client: conn.id, req, id, waiting: asked, deadline });
        }
        asked > 0
    }

    /// A peer's answer to a pull: the first valid blob is stored (if there
    /// is room) and sent to the client; when every peer has failed, the
    /// client is told "not found".
    fn pulled(&self, pull_id: u64, from: u64, data: Option<Vec<u8>>) {
        let mut state = self.state.lock().unwrap();
        let Some(pull) = state.pulls.get_mut(&pull_id) else { return };
        let good = data.filter(|d| valid_size(d.len()) && blob_id(d) == pull.id);
        pull.waiting -= 1;
        let reply = match &good {
            Some(d) => protocol::encode_get_reply(pull.req, status::DONE, d),
            None if pull.waiting == 0 => protocol::encode_get_reply(pull.req, status::NOT_FOUND, &[]),
            None => return,
        };
        let pull = state.pulls.remove(&pull_id).unwrap();
        if let Some(c) = state.conns.get(&pull.client) {
            push(c, reply);
        }
        drop(state);
        if let Some(d) = good
            && let Ok((_, true)) = self.blobs.put_new(&d)
            && d.len() <= PUSH_LIMIT
        {
            self.push_to_peers(Some(from), |r| protocol::encode_put(r, &d));
        }
    }

    /// Ends pulls whose peers did not answer in time.
    pub fn expire_pulls(&self) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        let late: Vec<u64> = state.pulls.iter().filter(|(_, p)| p.deadline <= now).map(|(&id, _)| id).collect();
        for id in late {
            let pull = state.pulls.remove(&id).unwrap();
            if let Some(c) = state.conns.get(&pull.client) {
                push(c, protocol::encode_get_reply(pull.req, status::NOT_FOUND, &[]));
            }
        }
    }

    /// Queues a request (numbered per peer) on every peer but `except`.
    fn push_to_peers(&self, except: Option<u64>, message: impl Fn(u32) -> Vec<u8>) {
        let mut state = self.state.lock().unwrap();
        for (&cid, c) in state.conns.iter_mut() {
            if Some(cid) != except && c.peer.load(Ordering::Relaxed) {
                c.next_req = c.next_req.wrapping_add(1);
                push(c, message(c.next_req));
            }
        }
    }

    /// Verifies, stores and pushes an entry (NETWORK 4.3); returns the status.
    pub fn publish(&self, bytes: &[u8]) -> u8 {
        self.publish_from(bytes, None)
    }

    /// [`Self::publish`] for an entry that came from connection `source`: a
    /// new entry goes to the subscribers and to every peer but the source.
    fn publish_from(&self, bytes: &[u8], source: Option<u64>) -> u8 {
        let Ok(signed) = entry::decode(bytes) else { return status::REFUSED };
        let feed = signed.entry.feed;
        let mut state = self.state.lock().unwrap();
        let Ok(seqs) = self.feeds.seqs(&feed) else { return status::REFUSED };
        // An entry older than everything kept in a full feed would be pruned
        // at once; storing it could let it replace a pruned entry.
        if seqs.len() >= self.cap && seqs.first().is_some_and(|&low| signed.entry.seq < low) {
            return status::REFUSED;
        }
        match self.feeds.insert(&signed) {
            // A failed record only leaves the entry out of searches.
            Ok(Insert::New) => drop(self.arrivals.lock().unwrap().record(&feed, signed.entry.seq)),
            Ok(Insert::Duplicate) => return status::DONE,
            Ok(Insert::Conflict) | Err(_) => return status::REFUSED,
        }
        let keep = {
            let mut profiles = self.profiles.lock().unwrap();
            if signed.entry.kind == KIND_PROFILE && profiles.get(&feed).is_none_or(|p| p.seq < signed.entry.seq) {
                profiles.insert(feed, profile_of(&signed));
            }
            profiles.get(&feed).map(|p| p.seq)
        };
        // A failed prune only means keeping more than the cap. The latest
        // profile is always kept (NETWORK 4.6).
        let _ = self.feeds.prune_keeping(&feed, self.cap, keep);
        let message = protocol::encode_entry(&signed.bytes);
        // Peers get the entry as a publish below, not as a pushed entry.
        state.conns.retain(|_, c| c.peer.load(Ordering::Relaxed) || !c.feeds.contains(&feed) || push(c, message.clone()));
        for (&cid, c) in state.conns.iter_mut() {
            if Some(cid) != source && c.peer.load(Ordering::Relaxed) {
                c.next_req = c.next_req.wrapping_add(1);
                push(c, protocol::encode_publish(c.next_req, &signed.bytes));
            }
        }
        status::DONE
    }

    /// Subscribes `conn` and queues the backlog; returns the highest `seq`
    /// held (NETWORK 4.3), or `None` if refused. The reply is sent before
    /// the queue is drained, so it precedes the backlog. A client's
    /// subscription is repeated on the peers (NETWORK 5).
    fn subscribe(&self, conn: &Connection, feed: FeedKey, after: u32) -> Option<u32> {
        if !valid_feed_key(&feed) {
            return None;
        }
        // Under the lock, so `highest` and the backlog agree and no publish
        // slips between them.
        let mut state = self.state.lock().unwrap();
        let seqs = self.feeds.seqs(&feed).ok()?;
        let highest = seqs.last().copied().unwrap_or(0);
        let c = state.conns.get_mut(&conn.id)?;
        c.feeds.insert(feed);
        for seq in seqs.iter().copied().filter(|&seq| seq > after) {
            // A file pruned meanwhile is skipped.
            let Ok(Some(bytes)) = self.feeds.get(&feed, seq) else { continue };
            if !push(c, protocol::encode_entry(&bytes)) {
                state.conns.remove(&conn.id);
                return Some(highest);
            }
        }
        if !conn.peer.load(Ordering::Relaxed) {
            for c in state.conns.values_mut().filter(|c| c.peer.load(Ordering::Relaxed)) {
                peer_subscribe(c, feed, highest);
            }
        }
        Some(highest)
    }

    /// Answers a find from this relay's own index (NETWORK 4.7); `None`
    /// refuses it.
    pub fn find(&self, scope: u8, query: &[u8]) -> Option<Vec<Vec<u8>>> {
        if !valid_find(scope, query) {
            return None;
        }
        let mut profiles = self.profiles.lock().unwrap();
        // A profile whose document is not held (or does not parse) is
        // unlisted, until the document arrives.
        for p in profiles.values_mut().filter(|p| p.listed.is_none()) {
            p.listed = match self.blobs.read(&p.blob) {
                Ok(Some(b)) if blob_id(&b) == p.blob => Some(Document::parse(&b).is_some_and(|d| d.listed)),
                _ => None,
            };
        }
        let listed = profiles.iter().filter(|(_, p)| p.listed == Some(true));
        if scope == SCOPE_FEEDS {
            let mut hits: Vec<(bool, FeedKey, &Vec<u8>)> = listed
                .filter(|(_, p)| profile::matches(p.name.as_bytes(), query))
                .map(|(k, p)| (!profile::starts_with(p.name.as_bytes(), query), *k, &p.bytes))
                .collect();
            hits.sort();
            return Some(hits.into_iter().take(MAX_FIND_RESULTS).map(|h| h.2.clone()).collect());
        }
        let listed: HashSet<FeedKey> = listed.map(|(k, _)| *k).collect();
        drop(profiles);
        // Newest stored first (NETWORK 4.7); pruned entries are skipped.
        let order: Vec<(FeedKey, u32)> = match self.arrivals.lock().unwrap().order() {
            Ok(o) => o.iter().rev().filter(|(f, _)| listed.contains(f)).copied().collect(),
            Err(_) => Vec::new(),
        };
        // Scope 2: articles and documents.
        let want: &[u8] = if scope == SCOPE_ENTRIES { &[KIND_ARTICLE, KIND_DOCUMENT] } else { &[KIND_PROGRAM] };
        let mut hits = Vec::new();
        for (feed, seq) in order {
            if hits.len() == MAX_FIND_RESULTS {
                break;
            }
            // Stored entries were verified when they arrived.
            let Ok(Some(bytes)) = self.feeds.get(&feed, seq) else { continue };
            let Some((kind, title)) = kind_and_title(&bytes) else { continue };
            if !want.contains(&kind) || !profile::matches(title, query) {
                continue;
            }
            // Programs: current versions only (no entry of the same name
            // with a higher seq is held).
            if kind == KIND_PROGRAM {
                let newer = self.feeds.seqs(&feed).unwrap_or_default().into_iter().filter(|&s| s > seq).any(|s| {
                    let Ok(Some(b)) = self.feeds.get(&feed, s) else { return false };
                    kind_and_title(&b) == Some((KIND_PROGRAM, title))
                });
                if newer {
                    continue;
                }
            }
            hits.push(bytes.clone());
        }
        Some(hits)
    }
}

impl Relay {
    /// The current version of program `name` of `feed` that this relay
    /// holds: its program entry of that name with the highest `seq`.
    pub fn lookup(&self, feed: &FeedKey, name: &[u8]) -> Option<Vec<u8>> {
        let seqs = self.feeds.seqs(feed).ok()?;
        seqs.into_iter().rev().find_map(|seq| {
            let bytes = self.feeds.get(feed, seq).ok()??;
            (kind_and_title(&bytes) == Some((KIND_PROGRAM, name))).then_some(bytes)
        })
    }
}

/// A stored entry's kind and title.
fn kind_and_title(bytes: &[u8]) -> Option<(u8, &[u8])> {
    let (&kind, &n) = (bytes.get(40)?, bytes.get(108)?);
    Some((kind, bytes.get(109..109 + n as usize)?))
}

/// Whether a find's scope and query are well formed (NETWORK 4.7).
fn valid_find(scope: u8, query: &[u8]) -> bool {
    (SCOPE_FEEDS..=SCOPE_PROGRAMS).contains(&scope)
        && (1..=profile::MAX_NAME).contains(&query.len())
        && std::str::from_utf8(query).is_ok()
}

fn profile_of(p: &entry::SignedEntry) -> Profile {
    Profile { seq: p.entry.seq, bytes: p.bytes.clone(), blob: p.entry.blob, name: p.entry.title.clone(), listed: None }
}

/// Subscribes to `feed` on a peer, once per connection.
fn peer_subscribe(c: &mut Conn, feed: FeedKey, after: u32) {
    if c.peer_feeds.insert(feed) {
        c.next_req = c.next_req.wrapping_add(1);
        push(c, protocol::encode_subscribe(c.next_req, &feed, after));
    }
}

/// Queues a message without waiting; false (and the connection marked for
/// closing) if it cannot take it.
fn push(c: &Conn, message: Vec<u8>) -> bool {
    match c.queue.try_send(message) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            c.dropped.store(true, Ordering::Relaxed);
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

/// Keeps a peer link to each URL (NETWORK 5): connects, announces itself
/// with a Peer message, and reconnects after 1 s, doubling to 30 s, when
/// the link fails or drops.
pub fn connect_peers(relay: Arc<Relay>, urls: Vec<String>) {
    for url in urls {
        let relay = relay.clone();
        thread::spawn(move || {
            let mut retry = FIRST_RETRY;
            loop {
                let started = Instant::now();
                match peer_link(&url, &relay) {
                    Ok(()) => eprintln!("terse-relay: peer {url}: closed"),
                    Err(e) => eprintln!("terse-relay: peer {url}: {e}"),
                }
                if started.elapsed() > MAX_RETRY {
                    retry = FIRST_RETRY;
                }
                thread::sleep(retry);
                retry = (retry * 2).min(MAX_RETRY);
            }
        });
    }
}

fn peer_link(url: &str, relay: &Relay) -> Result<(), String> {
    let transport = terse_net::relay::WsTransport::connect(url, WRITE_TIMEOUT).map_err(|e| e.to_string())?;
    let mut ws = transport.into_inner();
    ws.get_ref().set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
    ws.get_ref().set_write_timeout(Some(WRITE_TIMEOUT)).map_err(|e| e.to_string())?;
    ws.send(Message::binary(protocol::encode_peer())).map_err(|e| e.to_string())?;
    eprintln!("terse-relay: peer {url}: connected");
    let conn = relay.connect();
    relay.mark_peer(&conn);
    let result = serve_connection(&mut ws, relay, &conn);
    relay.disconnect(&conn);
    result.map_err(|e| e.to_string())
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
    let send = |ws: &mut tungstenite::WebSocket<TcpStream>, m: Vec<u8>| {
        if conn.peer.load(Ordering::Relaxed)
            && let Some(&kind) = m.first()
        {
            relay.peer_messages[kind as usize].fetch_add(1, Ordering::Relaxed);
        }
        ws.send(Message::binary(m))
    };
    loop {
        if conn.dropped.load(Ordering::Relaxed) {
            break;
        }
        relay.expire_pulls();
        for message in conn.queue.try_iter() {
            send(ws, message)?;
        }
        let answer = match ws.read() {
            Ok(Message::Binary(m)) => relay.respond(conn, &m),
            // NETWORK 3: all messages are binary.
            Ok(Message::Text(_)) => Answer::Close,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if is_timeout(&e) => continue,
            Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(e),
        };
        match answer {
            Answer::Reply(r) => send(ws, r)?,
            Answer::Nothing => {}
            Answer::Close => break,
        }
    }
    ws.close(None)?;
    // Drain until the peer acknowledges the close.
    while ws.read().is_ok() {}
    Ok(())
}
