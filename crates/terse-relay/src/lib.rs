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
//!
//! Limits (NETWORK 3.1, 5.3), all set by a [`Policy`] and off in
//! [`Policy::unlimited`] (the relay a host starts for itself):
//!
//! - **Per feed**: at most `feed_entries` entries and `feed_bytes` bytes of
//!   the blobs they name (each entry's size; a blob named by two feeds
//!   counts for each). A new entry that goes over drops the feed's oldest
//!   entries, never the latest profile; one that would itself be dropped
//!   is refused (status 4).
//! - **Blobs no entry names** (with `unreferenced_ttl`): a put of one is
//!   accepted into a waiting room, limited in all and per address, and
//!   removed after the time unless an entry names it by then; a blob whose
//!   last entry is dropped waits the same way. They are not served to
//!   gets: only blobs that entries name are. See [`holdings`].
//! - **Per address** ([`limits`]): puts and publishes, bytes put, entries
//!   of new feeds, finds and lookups, and connections. The address is the
//!   TCP peer's, or a trusted proxy's header. A refusal says when to try
//!   again (status 64 and up). Links this relay opened to its `--peer`s
//!   and connections made in the process ([`Relay::connect`]) are not
//!   limited.
//! - **Write list** ([`writers`]): entries only of the listed feeds.

pub mod holdings;
pub mod limits;
pub mod writers;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terse_net::blob::{BlobId, blob_id, parse_hex, valid_size};
use terse_net::store::PutError;
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
/// How long a client may take over the WebSocket handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Most waiting blobs removed in one hold of the lock.
const COLLECT_BATCH: usize = 64;
/// How often the maintenance thread runs.
const TICK: Duration = Duration::from_millis(100);
/// How often the write list's file is checked for changes.
const WRITERS_CHECK: Duration = Duration::from_secs(2);

/// What a relay limits (module documentation; NETWORK 5.3). Every limit
/// is off in [`Policy::unlimited`]; [`Policy::public`] has the defaults
/// proposed for a public relay.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Entries kept per feed (at least [`MIN_FEED_CAP`] for a relay that
    /// keeps to NETWORK 4.3).
    pub feed_entries: usize,
    /// Bytes of the blobs a feed's entries name, at most.
    pub feed_bytes: Option<u64>,
    /// How long a blob no entry names is kept; `None`: for ever (and such
    /// blobs are served).
    pub unreferenced_ttl: Option<Duration>,
    /// The waiting room's size, in all and per address (each blob counts
    /// at least [`holdings::MIN_CHARGE`] bytes).
    pub unreferenced_bytes: u64,
    pub unreferenced_bytes_per_address: u64,
    /// With a write list: per address not known as a writer's (none of
    /// the listed feeds published from it lately); all of them together
    /// have at most half the room.
    pub stranger_bytes_per_address: u64,
    /// Rates per address; `None`: none.
    pub rates: Option<limits::Rates>,
    /// Entries of new feeds from all addresses together.
    pub new_feeds_total: Option<limits::Rate>,
    pub connections_per_address: Option<usize>,
    pub max_connections: Option<usize>,
    /// Addresses whose rates are kept at once; more share one.
    pub table_size: usize,
    /// The write list's file.
    pub writers: Option<PathBuf>,
    /// How long an address counts as a writer's after a listed feed
    /// published from it.
    pub writer_memory: Duration,
    /// Proxies whose `CF-Connecting-IP` / `X-Forwarded-For` is believed.
    pub trust_proxy: Vec<limits::Cidr>,
    /// A ping on a connection that has sent nothing for this long.
    pub ping_every: Option<Duration>,
    /// A status line on stderr this often.
    pub status_every: Option<Duration>,
    /// A file rewritten with the status every few seconds.
    pub status_file: Option<PathBuf>,
}

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;
pub const GIB: u64 = 1024 * MIB;

impl Policy {
    /// No limits but the entries per feed: the relay a host starts for
    /// itself, and `terse-relay` without `--public`.
    pub fn unlimited(feed_entries: usize) -> Self {
        Policy {
            feed_entries: feed_entries.max(1),
            feed_bytes: None,
            unreferenced_ttl: None,
            unreferenced_bytes: 256 * MIB,
            unreferenced_bytes_per_address: 16 * MIB,
            stranger_bytes_per_address: MIB + 64 * KIB,
            rates: None,
            new_feeds_total: None,
            connections_per_address: None,
            max_connections: None,
            table_size: 8192,
            writers: None,
            writer_memory: Duration::from_secs(24 * 3600),
            trust_proxy: Vec::new(),
            ping_every: Some(Duration::from_secs(30)),
            status_every: None,
            status_file: None,
        }
    }

    /// The defaults proposed for a public relay (README "Running a public
    /// relay"): 16 MiB per feed, blobs no entry names kept an hour, rates
    /// per address.
    pub fn public() -> Self {
        use limits::{Rate, Rates};
        let minute = Duration::from_secs(60);
        let hour = Duration::from_secs(3600);
        Policy {
            feed_bytes: Some(16 * MIB),
            unreferenced_ttl: Some(hour),
            rates: Some(Rates {
                uploads: Rate::new(120.0, minute),
                upload_bytes: Rate::new((32 * MIB) as f64, hour),
                new_feeds: Rate::new(3.0, hour),
                finds: Rate::new(60.0, minute),
            }),
            new_feeds_total: Some(Rate::new(60.0, hour)),
            connections_per_address: Some(32),
            max_connections: Some(1000),
            status_every: Some(Duration::from_secs(600)),
            ..Policy::unlimited(DEFAULT_FEED_CAP)
        }
    }

    fn indexed(&self) -> bool {
        self.feed_bytes.is_some() || self.unreferenced_ttl.is_some()
    }
}

/// Why something was refused, counted for the status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    FeedFull,
    RelayFull,
    NotWriter,
    Uploads,
    UploadBytes,
    NewFeeds,
    Finds,
    WaitingRoom,
    Connections,
}

const REASONS: [(Reason, &str); 9] = [
    (Reason::FeedFull, "feed-full"),
    (Reason::RelayFull, "relay-full"),
    (Reason::NotWriter, "not-writer"),
    (Reason::Uploads, "uploads"),
    (Reason::UploadBytes, "upload-bytes"),
    (Reason::NewFeeds, "new-feeds"),
    (Reason::Finds, "finds"),
    (Reason::WaitingRoom, "waiting-room"),
    (Reason::Connections, "connections"),
];

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
    policy: Policy,
    /// The index of [`holdings`], kept when the policy needs it (a limit
    /// on feed bytes, or blobs no entry names expiring).
    holdings: Option<Mutex<holdings::Holdings>>,
    rates: Mutex<limits::RateTable>,
    new_feeds_total: Mutex<Option<limits::Bucket>>,
    writers: Option<Mutex<writers::Writers>>,
    /// Added to the clock (tests).
    clock_offset: AtomicU64,
    refused: [AtomicUsize; 9],
    /// Waiting blobs removed, and their bytes; entries dropped for a
    /// feed's limits.
    collected: AtomicU64,
    collected_bytes: AtomicU64,
    dropped_entries: AtomicU64,
    /// Connections served now (not counting in-process ones).
    connections: AtomicUsize,
    /// Set to stop: connections close, peer links stop.
    closing: AtomicBool,
    maintenance: AtomicBool,
    state: Mutex<State>,
    profiles: Mutex<HashMap<FeedKey, Profile>>,
    /// The order in which entries were stored.
    arrivals: Mutex<Arrivals>,
    /// Messages written to peer connections, by type.
    peer_messages: [AtomicUsize; 256],
    /// Messages received on every connection, by type (for counting round
    /// trips: `terse perf`).
    received: [AtomicUsize; 256],
}

/// One connection's share of the relay.
pub struct Connection {
    id: u64,
    /// The address it is limited as; `None`: not limited.
    group: Option<limits::Group>,
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
    /// A relay on a blob store, with its feed entries beside the blobs,
    /// keeping `cap` entries per feed and with no other limit.
    pub fn new(blobs: BlobStore, cap: usize) -> Self {
        Self::with_policy(blobs, Policy::unlimited(cap)).expect("no write list to read")
    }

    /// A relay with the limits of `policy`; fails if its write list cannot
    /// be read.
    pub fn with_policy(blobs: BlobStore, policy: Policy) -> Result<Self, String> {
        let writers = match &policy.writers {
            Some(path) => Some(Mutex::new(writers::Writers::load(path)?)),
            None => None,
        };
        let cap = policy.feed_entries.max(1);
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
        let mut relay = Relay {
            arrivals: Mutex::new(arrivals),
            blobs,
            feeds,
            cap,
            holdings: None,
            rates: Mutex::new(limits::RateTable::new(policy.rates.clone(), policy.table_size)),
            new_feeds_total: Mutex::new(None),
            writers,
            clock_offset: AtomicU64::new(0),
            refused: std::array::from_fn(|_| AtomicUsize::new(0)),
            collected: AtomicU64::new(0),
            collected_bytes: AtomicU64::new(0),
            dropped_entries: AtomicU64::new(0),
            connections: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            maintenance: AtomicBool::new(false),
            policy,
            state: Mutex::default(),
            profiles: Mutex::new(profiles),
            peer_messages: std::array::from_fn(|_| AtomicUsize::new(0)),
            received: std::array::from_fn(|_| AtomicUsize::new(0)),
        };
        if relay.policy.indexed() {
            relay.holdings = Some(Mutex::new(relay.build_index()));
        }
        Ok(relay)
    }

    /// The index of what is on disk: every entry's blob and size, and the
    /// blobs no entry names, waiting until their file's time plus the
    /// waiting time. Temporary files a crash left are removed.
    fn build_index(&self) -> holdings::Holdings {
        let mut h = holdings::Holdings::default();
        for feed in self.feeds.feeds().unwrap_or_default() {
            for seq in self.feeds.seqs(&feed).unwrap_or_default() {
                if let Ok(Some(bytes)) = self.feeds.get(&feed, seq)
                    && let Some((blob, size)) = blob_and_size(&bytes)
                {
                    h.add_entry(feed, seq, blob, size);
                }
            }
        }
        let Ok(dir) = std::fs::read_dir(self.blobs.dir()) else { return h };
        for item in dir.flatten() {
            let name = item.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') && name.ends_with(".tmp") {
                let _ = std::fs::remove_file(item.path());
                continue;
            }
            let (Some(id), Ok(meta)) = (parse_hex(name), item.metadata()) else { continue };
            let len = meta.len() as u32;
            if let Some(ttl) = self.policy.unreferenced_ttl
                && !h.held(&id, len)
            {
                let since = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
                let charge = u64::from(len).max(holdings::MIN_CHARGE);
                h.wait(id, holdings::Wait { len, deadline: since + ttl.as_millis() as u64, charge, group: None, stranger: false });
            }
        }
        h
    }

    /// The relay's clock, in ms since 1970 (moved by [`Self::advance_clock`]).
    pub fn now_ms(&self) -> u64 {
        let real = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        real + self.clock_offset.load(Ordering::Relaxed)
    }

    /// Moves the relay's clock on (tests: rates and the waiting room).
    pub fn advance_clock(&self, by: Duration) {
        self.clock_offset.fetch_add(by.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// How many requests were refused for `reason`.
    pub fn refused(&self, reason: Reason) -> usize {
        self.refused[reason as usize].load(Ordering::Relaxed)
    }

    fn count(&self, reason: Reason) {
        self.refused[reason as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Bytes and number of the blobs no entry names that are waiting.
    pub fn waiting(&self) -> (u64, usize) {
        match &self.holdings {
            Some(h) => {
                let h = h.lock().unwrap();
                (h.waiting.values().map(|w| u64::from(w.len)).sum(), h.waiting.len())
            }
            None => (0, 0),
        }
    }

    /// The bytes of blobs a feed's entries name, if the relay counts them.
    pub fn feed_bytes(&self, feed: &FeedKey) -> Option<u64> {
        let h = self.holdings.as_ref()?.lock().unwrap();
        Some(h.feeds.get(feed).map_or(0, |f| f.bytes))
    }

    /// Asks connections and peer links to close (a graceful stop).
    pub fn close(&self) {
        self.closing.store(true, Ordering::Relaxed);
    }

    pub fn closing(&self) -> bool {
        self.closing.load(Ordering::Relaxed)
    }

    /// Connections being served (from [`serve`]).
    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    /// One line on what the relay holds and has refused.
    pub fn status(&self) -> String {
        let mut line = format!("store {} bytes", self.blobs.used_bytes());
        match &self.holdings {
            Some(h) => {
                let h = h.lock().unwrap();
                let waiting: u64 = h.waiting.values().map(|w| u64::from(w.len)).sum();
                line += &format!(
                    ", feeds {}, entries {}, unreferenced {} bytes in {} blobs",
                    h.feeds.len(),
                    h.entries(),
                    waiting,
                    h.waiting.len()
                );
            }
            None => line += &format!(", feeds {}", self.feeds.feeds().map_or(0, |f| f.len())),
        }
        line += &format!(
            ", connections {} from {} addresses, collected {} blobs ({} bytes), dropped {} entries",
            self.connection_count(),
            self.rates.lock().unwrap().len(),
            self.collected.load(Ordering::Relaxed),
            self.collected_bytes.load(Ordering::Relaxed),
            self.dropped_entries.load(Ordering::Relaxed)
        );
        if let Some(w) = &self.writers {
            line += &format!(", writers {}", w.lock().unwrap().len());
        }
        let refused: Vec<String> =
            REASONS.iter().filter(|(r, _)| self.refused(*r) > 0).map(|(r, name)| format!("{name} {}", self.refused(*r))).collect();
        line += &format!("; refused: {}", if refused.is_empty() { "none".into() } else { refused.join(", ") });
        line
    }

    /// Removes waiting blobs whose time is up, a batch at a time (the
    /// lock is let go between batches, so connections are not held up);
    /// returns how many.
    pub fn collect(&self) -> usize {
        let Some(h) = &self.holdings else { return 0 };
        let mut total = 0;
        loop {
            let mut h = h.lock().unwrap();
            let now = self.now_ms();
            let ids = h.expired(now, COLLECT_BATCH);
            for id in &ids {
                // Unlinked under the lock: a put or an entry naming it
                // waits, then finds it gone.
                if let Some(w) = h.unwait(id) {
                    match self.blobs.remove(id) {
                        Ok(()) => {
                            self.collected.fetch_add(1, Ordering::Relaxed);
                            self.collected_bytes.fetch_add(u64::from(w.len), Ordering::Relaxed);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => eprintln!("terse-relay: removing a waiting blob: {e}"),
                    }
                }
            }
            drop(h);
            total += ids.len();
            if ids.len() < COLLECT_BATCH {
                return total;
            }
            // The lock is not fair: give the connections waiting for it
            // their turn between batches.
            thread::sleep(Duration::from_micros(200));
        }
    }

    /// Reads the write list again (if its file changed, unless `force`);
    /// says so on stderr.
    pub fn reload_writers(&self, force: bool) {
        let Some(w) = &self.writers else { return };
        let mut w = w.lock().unwrap();
        let result = if force { Some(w.reload()) } else { w.reload_if_changed() };
        match result {
            Some(Ok(n)) => eprintln!("terse-relay: write list read: {n} feeds"),
            Some(Err(e)) => eprintln!("terse-relay: write list not read, the previous one stays: {e}"),
            None => {}
        }
    }

    /// Starts the relay's periodic work on a thread of its own (once):
    /// late pulls, the waiting room, the write list, the status. [`serve`]
    /// starts it.
    pub fn maintain(self: &Arc<Self>) {
        if self.maintenance.swap(true, Ordering::Relaxed) {
            return;
        }
        let relay = self.clone();
        thread::spawn(move || {
            let mut writers_at = Instant::now();
            let mut status_at = Instant::now();
            let mut file_at: Option<Instant> = None;
            loop {
                thread::sleep(TICK);
                relay.expire_pulls();
                relay.collect();
                if writers_at.elapsed() >= WRITERS_CHECK {
                    writers_at = Instant::now();
                    relay.reload_writers(false);
                }
                if let Some(every) = relay.policy.status_every
                    && status_at.elapsed() >= every
                {
                    status_at = Instant::now();
                    eprintln!("terse-relay: status: {}", relay.status());
                }
                if let Some(path) = &relay.policy.status_file
                    && file_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
                {
                    file_at = Some(Instant::now());
                    let temp = path.with_extension("tmp");
                    if std::fs::write(&temp, relay.status() + "\n").and_then(|()| std::fs::rename(&temp, path)).is_err() {
                        let _ = std::fs::remove_file(&temp);
                    }
                }
            }
        });
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

    /// How many messages of type `kind` this relay has received, from
    /// clients and peers.
    pub fn received(&self, kind: u8) -> usize {
        self.received[kind as usize].load(Ordering::Relaxed)
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
        Connection { id, group: None, queue: rx, dropped, peer, finds: Mutex::default() }
    }

    /// A connection from a client counted as `group`, limited by the
    /// policy's rates.
    pub fn connect_from(&self, group: limits::Group) -> Connection {
        Connection { group: Some(group), ..self.connect() }
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
        if let Some(&kind) = message.first() {
            self.received[kind as usize].fetch_add(1, Ordering::Relaxed);
        }
        let peer = conn.peer.load(Ordering::Relaxed);
        if message.first().is_some_and(|&k| k >= 0x80) {
            if peer {
                self.from_peer(conn, message);
            }
            return Answer::Nothing;
        }
        Answer::Reply(match protocol::decode_request(message) {
            Ok(Request::Get { req, id }) => match self.served(&id) {
                Some(data) => protocol::encode_get_reply(req, status::DONE, &data),
                // NETWORK 5: a client's get is asked of the peers; a peer's
                // is answered from this store only.
                _ if !peer && self.pull(conn, req, id) => return Answer::Nothing,
                _ => protocol::encode_get_reply(req, status::NOT_FOUND, &[]),
            },
            Ok(Request::Put { req, data }) => match self.put_blob(data, conn.group) {
                Ok((id, new)) => {
                    if new && data.len() <= PUSH_LIMIT {
                        self.push_to_peers(Some(conn.id), |r| protocol::encode_put(r, data));
                    }
                    protocol::encode_put_reply(req, status::DONE, &id)
                }
                Err(s) => protocol::encode_put_reply(req, s, &[0; 32]),
            },
            Ok(Request::Publish { req, entry }) => {
                protocol::encode_status_reply(protocol::PUBLISH_REPLY, req, self.publish_from(entry, Some(conn.id), conn.group))
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
            Ok(Request::Find { req, scope, query }) => {
                let found = if valid_find(scope, query) { self.allow_find(conn).map(|()| self.find(scope, query)) } else { Ok(None) };
                match found {
                    Ok(Some(found)) => protocol::encode_find_reply(req, status::DONE, &found),
                    Ok(None) => protocol::encode_find_reply::<&[u8]>(req, status::REFUSED, &[]),
                    Err(s) => protocol::encode_find_reply::<&[u8]>(req, s, &[]),
                }
            }
            // NETWORK 4.8: answered for any feed, listed or not; counted
            // with the finds.
            Ok(Request::Lookup { req, feed, kind, name }) => {
                if kind != KIND_PROGRAM || !std::str::from_utf8(name).is_ok_and(profile::valid_name) {
                    protocol::encode_lookup_reply(req, status::REFUSED, &[])
                } else if let Err(s) = self.allow_find(conn) {
                    protocol::encode_lookup_reply(req, s, &[])
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
    /// answered in the last second, and its address's allowance (the
    /// policy's rates) not used up; else the "try again" status. An
    /// allowed find is counted; refused and malformed ones are not.
    fn allow_find(&self, conn: &Connection) -> Result<(), u8> {
        let now = Instant::now();
        let mut finds = conn.finds.lock().unwrap();
        while finds.front().is_some_and(|&t| now.duration_since(t) >= Duration::from_secs(1)) {
            finds.pop_front();
        }
        if finds.len() >= FINDS_PER_SECOND {
            self.count(Reason::Finds);
            let wait = Duration::from_secs(1).saturating_sub(now.duration_since(finds[0]));
            return Err(status::retry(wait));
        }
        if let Some(g) = conn.group
            && let Err(wait) = self.rates.lock().unwrap().take(g, limits::Kind::Find, 1.0, self.now_ms())
        {
            self.count(Reason::Finds);
            return Err(status::retry(wait));
        }
        finds.push_back(now);
        Ok(())
    }

    /// A blob to serve to a get: one this relay holds and, if blobs no
    /// entry names expire here, that an entry names (NETWORK 5.3).
    fn served(&self, id: &BlobId) -> Option<Vec<u8>> {
        let data = self.blobs.read(id).ok()??;
        if self.policy.unreferenced_ttl.is_some()
            && let Some(h) = &self.holdings
            && !h.lock().unwrap().held(id, data.len() as u32)
        {
            return None;
        }
        Some(data)
    }

    /// Stores a blob put by a client counted as `group` (`None`: a peer
    /// this relay linked to, or the process itself): its ID and whether it
    /// is new, or the status refusing it (NETWORK 3.1, 5.3).
    pub fn put_blob(&self, data: &[u8], group: Option<limits::Group>) -> Result<(BlobId, bool), u8> {
        // Empty and oversize blobs are refused without a reason.
        if data.is_empty() || !valid_size(data.len()) {
            return Err(status::REFUSED);
        }
        if let Some(g) = group {
            let now = self.now_ms();
            let mut rates = self.rates.lock().unwrap();
            if let Err(wait) = rates.take(g, limits::Kind::Upload, 1.0, now) {
                self.count(Reason::Uploads);
                return Err(status::retry(wait));
            }
            if let Err(wait) = rates.take(g, limits::Kind::UploadBytes, data.len() as f64, now) {
                self.count(Reason::UploadBytes);
                return Err(status::retry(wait));
            }
        }
        let (Some(ttl), Some(holdings)) = (self.policy.unreferenced_ttl, &self.holdings) else {
            return self.blobs.put_new(data).map_err(|e| self.store_refusal(e));
        };
        let (id, len, now) = (blob_id(data), data.len() as u32, self.now_ms());
        let ttl = ttl.as_millis() as u64;
        let deadline = now + ttl;
        // With a write list, an address none of the listed feeds published
        // from lately has a smaller share of the room.
        let stranger = self.writers.is_some() && group.is_some_and(|g| !self.rates.lock().unwrap().is_writer(g, now));
        let charge = u64::from(len).max(holdings::MIN_CHARGE);
        {
            let mut h = holdings.lock().unwrap();
            if h.waiting.contains_key(&id) {
                h.extend(&id, deadline);
                return Ok((id, false));
            }
            if h.writing.contains(&id) {
                return Ok((id, false));
            }
            if !h.held(&id, len) {
                let wait = |deadline: Option<u64>| Duration::from_millis(deadline.map_or(ttl, |d| d.saturating_sub(now)).max(1000));
                let room = self.policy.unreferenced_bytes;
                let mut full = (h.waiting_bytes + charge > room).then(|| h.next_deadline(None));
                if let Some(g) = group {
                    let allowed = if stranger { self.policy.stranger_bytes_per_address } else { self.policy.unreferenced_bytes_per_address };
                    if full.is_none() && h.group_bytes(&g) + charge > allowed {
                        full = Some(h.next_deadline(Some(g)));
                    }
                }
                if full.is_none() && stranger && h.stranger_bytes + charge > room / 2 {
                    full = Some(h.next_deadline(None));
                }
                if let Some(deadline) = full {
                    self.count(Reason::WaitingRoom);
                    return Err(status::retry(wait(deadline)));
                }
            }
            h.writing.insert(id);
        }
        // Written without the lock; the collector leaves it alone.
        let result = self.blobs.put_new(data);
        let mut h = holdings.lock().unwrap();
        h.writing.remove(&id);
        let (id, new) = result.map_err(|e| self.store_refusal(e))?;
        if !h.held(&id, len) {
            h.wait(id, holdings::Wait { len, deadline, charge, group, stranger });
        }
        Ok((id, new))
    }

    fn store_refusal(&self, e: PutError) -> u8 {
        match e {
            PutError::Full => {
                self.count(Reason::RelayFull);
                status::RELAY_FULL
            }
            _ => status::REFUSED,
        }
    }

    /// A blob whose last entry was dropped waits like an unnamed one, from
    /// now (its file's time is set to now, for a restart).
    fn start_waiting(&self, h: &mut holdings::Holdings, blob: BlobId, size: u32) {
        let Some(ttl) = self.policy.unreferenced_ttl else { return };
        let path = self.blobs.path(&blob);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == u64::from(size)) {
            let now = self.now_ms();
            if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                let _ = f.set_modified(UNIX_EPOCH + Duration::from_millis(now));
            }
            let charge = u64::from(size).max(holdings::MIN_CHARGE);
            h.wait(blob, holdings::Wait { len: size, deadline: now + ttl.as_millis() as u64, charge, group: None, stranger: false });
        }
    }

    /// A reply or pushed entry on a peer connection.
    fn from_peer(&self, conn: &Connection, message: &[u8]) {
        match protocol::decode_reply(message) {
            Some(Reply::Entry(bytes)) => {
                self.publish_from(&bytes, Some(conn.id), conn.group);
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
            && let Ok((_, true)) = self.put_blob(&d, None)
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
        self.publish_from(bytes, None, None)
    }

    /// [`Self::publish`] for an entry that came from connection `source`,
    /// a client counted as `group` (`None`: not limited): a new entry goes
    /// to the subscribers and to every peer but the source.
    fn publish_from(&self, bytes: &[u8], source: Option<u64>, group: Option<limits::Group>) -> u8 {
        if let Some(g) = group
            && let Err(wait) = self.rates.lock().unwrap().take(g, limits::Kind::Upload, 1.0, self.now_ms())
        {
            self.count(Reason::Uploads);
            return status::retry(wait);
        }
        let Ok(signed) = entry::decode(bytes) else { return status::REFUSED };
        let (feed, seq, size) = (signed.entry.feed, signed.entry.seq, signed.entry.size);
        if let Some(w) = &self.writers
            && !w.lock().unwrap().contains(&feed)
        {
            self.count(Reason::NotWriter);
            return status::NOT_WRITER;
        }
        if self.policy.feed_bytes.is_some_and(|q| u64::from(size) > q) {
            self.count(Reason::FeedFull);
            return status::FEED_FULL;
        }
        let mut state = self.state.lock().unwrap();
        let Ok(seqs) = self.feeds.seqs(&feed) else { return status::REFUSED };
        if seqs.is_empty()
            && let Some(g) = group
            && let Err(wait) = self.allow_new_feed(g)
        {
            self.count(Reason::NewFeeds);
            return status::retry(wait);
        }
        // The latest profile once this entry is in: never dropped.
        let profile = {
            let held = self.profiles.lock().unwrap().get(&feed).map(|p| p.seq);
            if signed.entry.kind == KIND_PROFILE && held.is_none_or(|p| p < seq) { Some(seq) } else { held }
        };
        let drops = match &self.holdings {
            Some(h) => {
                let h = h.lock().unwrap();
                let empty = BTreeMap::new();
                let entries = h.feeds.get(&feed).map_or(&empty, |f| &f.entries);
                if entries.contains_key(&seq) {
                    // A duplicate or a conflict: the store says which.
                    Vec::new()
                } else {
                    match plan_drops(entries, seq, size, profile, self.cap, self.policy.feed_bytes) {
                        Some(drops) => drops,
                        None => {
                            self.count(Reason::FeedFull);
                            return status::FEED_FULL;
                        }
                    }
                }
            }
            // An entry older than everything kept in a full feed would be
            // pruned at once; storing it could let it replace a pruned
            // entry.
            None if seqs.len() >= self.cap && seqs.first().is_some_and(|&low| seq < low) => {
                self.count(Reason::FeedFull);
                return status::FEED_FULL;
            }
            None => Vec::new(),
        };
        match self.feeds.insert(&signed) {
            // A failed record only leaves the entry out of searches.
            Ok(Insert::New) => drop(self.arrivals.lock().unwrap().record(&feed, seq)),
            Ok(Insert::Duplicate) => return status::DONE,
            Ok(Insert::Conflict) | Err(_) => return status::REFUSED,
        }
        let keep = {
            let mut profiles = self.profiles.lock().unwrap();
            if signed.entry.kind == KIND_PROFILE && profiles.get(&feed).is_none_or(|p| p.seq < seq) {
                profiles.insert(feed, profile_of(&signed));
            }
            profiles.get(&feed).map(|p| p.seq)
        };
        match &self.holdings {
            Some(h) => {
                let mut h = h.lock().unwrap();
                h.add_entry(feed, seq, signed.entry.blob, size);
                for s in drops {
                    // A failed removal keeps the entry, and its blob held.
                    if self.feeds.remove(&feed, s).is_ok() {
                        self.dropped_entries.fetch_add(1, Ordering::Relaxed);
                        if let Some((blob, size)) = h.remove_entry(&feed, s) {
                            self.start_waiting(&mut h, blob, size);
                        }
                    }
                }
            }
            // A failed prune only means keeping more than the cap. The
            // latest profile is always kept (NETWORK 4.6).
            None => drop(self.feeds.prune_keeping(&feed, self.cap, keep)),
        }
        if let Some(g) = group
            && self.writers.is_some()
        {
            let now = self.now_ms();
            self.rates.lock().unwrap().mark_writer(g, now + self.policy.writer_memory.as_millis() as u64, now);
        }
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

    /// Whether `group` may start a new feed now (the policy's rates, per
    /// address and for all together); else the wait.
    fn allow_new_feed(&self, group: limits::Group) -> Result<(), Duration> {
        let now = self.now_ms();
        self.rates.lock().unwrap().take(group, limits::Kind::NewFeed, 1.0, now)?;
        if let Some(rate) = &self.policy.new_feeds_total {
            let mut total = self.new_feeds_total.lock().unwrap();
            total.get_or_insert_with(|| limits::Bucket::full(now, rate)).take(1.0, now, rate)?;
        }
        Ok(())
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

/// The entries to drop from a feed (`entries`: `seq` to blob and size) so
/// that with a new entry (`seq`, `size`) it keeps to `cap` entries and
/// `quota` bytes: the oldest first, never `profile` (the latest profile).
/// `None` if the new entry would itself be dropped, or the feed cannot
/// get under the limits.
pub fn plan_drops(
    entries: &BTreeMap<u32, (BlobId, u32)>,
    seq: u32,
    size: u32,
    profile: Option<u32>,
    cap: usize,
    quota: Option<u64>,
) -> Option<Vec<u32>> {
    let mut count = entries.len() + 1;
    let mut bytes = entries.values().map(|&(_, s)| u64::from(s)).sum::<u64>() + u64::from(size);
    let over = |count: usize, bytes: u64| count > cap || quota.is_some_and(|q| bytes > q);
    let mut drops = Vec::new();
    let mut merged: Vec<(u32, u32)> = entries.iter().map(|(&s, &(_, z))| (s, z)).collect();
    let at = merged.partition_point(|&(s, _)| s < seq);
    merged.insert(at, (seq, size));
    for (s, z) in merged {
        if !over(count, bytes) {
            break;
        }
        if Some(s) == profile {
            continue;
        }
        if s == seq {
            return None;
        }
        drops.push(s);
        count -= 1;
        bytes -= u64::from(z);
    }
    (!over(count, bytes)).then_some(drops)
}

/// A stored entry's blob and size (NETWORK 4.2), unverified.
fn blob_and_size(bytes: &[u8]) -> Option<(BlobId, u32)> {
    let size = bytes.get(41..44)?;
    let size = u32::from(size[0]) | u32::from(size[1]) << 8 | u32::from(size[2]) << 16;
    Some((bytes.get(44..76)?.try_into().ok()?, size))
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

/// Accepts connections until the relay is closed ([`Relay::close`]; the
/// next connection then ends the loop), and starts the relay's periodic
/// work ([`Relay::maintain`]). A failure to accept (too many open files,
/// say) is reported and waited out.
pub fn serve(listener: TcpListener, relay: Arc<Relay>) -> std::io::Result<()> {
    relay.maintain();
    for stream in listener.incoming() {
        if relay.closing() {
            break;
        }
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("terse-relay: accepting a connection: {e}");
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let relay = relay.clone();
        thread::spawn(move || {
            if let Err((who, e)) = handle(stream, &relay) {
                eprintln!("terse-relay: {who}: {e}");
            }
        });
    }
    Ok(())
}

/// Keeps a peer link to each URL (NETWORK 5): connects, announces itself
/// with a Peer message, and reconnects after 1 s, doubling to 30 s, when
/// the link fails or drops.
pub fn connect_peers(relay: Arc<Relay>, urls: Vec<String>) {
    relay.maintain();
    for url in urls {
        let relay = relay.clone();
        thread::spawn(move || {
            let mut retry = FIRST_RETRY;
            while !relay.closing() {
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

/// A connection counted against its address and the relay's total until
/// it is dropped.
struct Counted<'a> {
    relay: &'a Relay,
    group: limits::Group,
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.relay.rates.lock().unwrap().disconnect(self.group);
        self.relay.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The HTTP answer refusing a WebSocket handshake.
fn refusal(code: u16, text: &str) -> tungstenite::handshake::server::ErrorResponse {
    let mut r = tungstenite::http::Response::new(Some(text.to_string()));
    *r.status_mut() = tungstenite::http::StatusCode::from_u16(code).unwrap();
    r
}

/// Serves one connection; on an error, the client's address as it may be
/// logged (truncated, [`limits::Group::for_log`]) and the error.
fn handle(stream: TcpStream, relay: &Relay) -> Result<(), (String, tungstenite::Error)> {
    let peer = stream.peer_addr().map(|a| a.ip()).unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let mut who = limits::Group::of(peer).for_log();
    let setup = |s: &TcpStream| -> std::io::Result<()> {
        s.set_nodelay(true)?;
        // A client that never finishes its handshake does not keep a thread.
        s.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        s.set_write_timeout(Some(WRITE_TIMEOUT))
    };
    setup(&stream).map_err(|e| (who.clone(), e.into()))?;
    let counted = std::cell::Cell::new(None);
    let check = |request: &tungstenite::handshake::server::Request, response| {
        let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(String::from);
        let group = limits::Group::of(limits::client_ip(peer, &relay.policy.trust_proxy, header));
        if relay.policy.max_connections.is_some_and(|m| relay.connections.load(Ordering::Relaxed) >= m) {
            relay.count(Reason::Connections);
            return Err(refusal(503, "this relay has too many connections; try again later"));
        }
        if !relay.rates.lock().unwrap().connect(group, relay.policy.connections_per_address, relay.now_ms()) {
            relay.count(Reason::Connections);
            return Err(refusal(429, "too many connections from this address"));
        }
        relay.connections.fetch_add(1, Ordering::Relaxed);
        counted.set(Some(group));
        Ok(response)
    };
    let accepted = tungstenite::accept_hdr_with_config(stream, check, Some(websocket_config()));
    let counted = counted.get().map(|group| Counted { relay, group });
    if let Some(c) = &counted {
        who = c.group.for_log();
    }
    let mut ws = accepted.map_err(|e| {
        let e = match e {
            tungstenite::HandshakeError::Failure(e) => e,
            tungstenite::HandshakeError::Interrupted(_) => tungstenite::Error::ConnectionClosed,
        };
        (who.clone(), e)
    })?;
    let ready = (|| {
        ws.get_ref().set_read_timeout(Some(POLL))?;
        ws.get_ref().set_write_timeout(Some(WRITE_TIMEOUT))
    })();
    ready.map_err(|e| (who.clone(), e.into()))?;
    let conn = match &counted {
        Some(c) => relay.connect_from(c.group),
        None => relay.connect(),
    };
    let result = serve_connection(&mut ws, relay, &conn);
    relay.disconnect(&conn);
    drop(counted);
    result.map_err(|e| (who, e))
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
    let mut sent_at = Instant::now();
    loop {
        if conn.dropped.load(Ordering::Relaxed) || relay.closing() {
            break;
        }
        for message in conn.queue.try_iter() {
            send(ws, message)?;
            sent_at = Instant::now();
        }
        // A silent connection is kept alive through proxies that close
        // idle ones (NETWORK 3: the other side answers pings).
        if relay.policy.ping_every.is_some_and(|every| sent_at.elapsed() >= every) {
            ws.send(Message::Ping(Default::default()))?;
            sent_at = Instant::now();
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
            Answer::Reply(r) => {
                send(ws, r)?;
                sent_at = Instant::now();
            }
            Answer::Nothing => {}
            Answer::Close => break,
        }
    }
    ws.close(None)?;
    // Drain until the peer acknowledges the close.
    while ws.read().is_ok() {}
    Ok(())
}
