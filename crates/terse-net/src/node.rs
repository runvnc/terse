//! Host behaviour (NETWORK 2, 4.4, 4.6, 4.7, 5): gets are answered from the
//! local store, else fetched whole from the relays, verified and stored;
//! puts are stored locally and sent to every connected relay. With
//! subscriptions, the node subscribes on each relay after the highest `seq`
//! it holds, verifies and stores pushed entries (the first received of a
//! `seq` wins, whichever relay it came from), and downloads small articles
//! and profiles.
//!
//! Relays: the host's own (at most eight, [`crate::local::MAX_RELAYS`]) and,
//! unless turned off, feed relays: for each subscribed feed whose profile in
//! force names relays, up to two of them, used for that feed's subscription
//! and for gets (at most [`MAX_LINKS`] connections in all).
//!
//! Reconnecting: a relay whose connection is lost (or cannot be made) is
//! tried again after 1 s, doubling up to 30 s, and the subscriptions are
//! renewed on every new connection. WebSocket relays connect in the
//! background; an operation that needs a relay while none is connected but
//! some are connecting waits until one is connected or none is connecting,
//! and is otherwise sent to the connected relays only, so a relay that does
//! not answer never delays the others.
//!
//! Hint relays (NETWORK 4.9): a feed subscribed with the hints of its
//! address, whose profile in force names no relays, is also followed on up
//! to two hinted relays once none of the host's own relays holds an entry
//! of it (every connected one answered its subscribe with `highest` 0, or
//! there is none to ask). They are feed relays: the same address rule, the
//! same switch, the same connection cap.
//!
//! Prefetch (NETWORK 4.4): when an automatic download stores a kind-1
//! article, the first [`PREFETCH_LINKS`] distinct `terse:` article links in
//! it whose blobs are not held or being fetched are fetched too, unless an
//! entry shows the blob is larger than the automatic download limit; a
//! fetched blob larger than the limit is discarded. Prefetched blobs are
//! not scanned.
//!
//! Caught up (NETWORK 4.3): a feed is caught up on a connection once the
//! entry whose `seq` equals the `highest` of the subscribe reply has arrived
//! (valid or not: an invalid entry is still the relay's last), or at once if
//! `highest` is not above `after`. The node is settled when every feed is
//! caught up on every connected relay and no automatic download is running.
//!
//! The node also answers the content device's feed operations (SPEC §content.feeds):
//! every new entry it stores is appended to the arrival log
//! ([`crate::timeline::Arrivals`]); the timeline is that log, newest first,
//! restricted to the article entries of the feeds subscribed to now.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::article;
use crate::blob::{BlobId, blob_id, to_hex, valid_size};
use crate::entry::{self, Entry, KIND_ARTICLE, KIND_DOCUMENT, KIND_PROFILE, KIND_PROGRAM, MAX_IMAGE, MAX_TITLE, SignedEntry};
use crate::feeds::{FeedStore, Insert, Subscriptions};
use crate::identity::{FeedKey, Identity};
use crate::profile::{self, Document, Nicknames};
use crate::protocol::{MAX_FIND_RESULTS, Refusal, SCOPE_ENTRIES, SCOPE_FEEDS, SCOPE_PROGRAMS};
use crate::relay::{DEFAULT_TIMEOUT, RelayClient, RelayReply};
use crate::store::{BlobStore, default_store_dir};
use crate::timeline::{self, Arrivals};

/// Identifies an operation started on a [`Node`]; chosen by the caller.
pub type Token = u64;

/// Articles (and profiles) up to this size are downloaded as soon as their
/// entry arrives (NETWORK 4.4).
pub const AUTO_DOWNLOAD_LIMIT: u32 = 16_384;
/// Most relay connections: the host's relays and feed relays (NETWORK 5).
pub const MAX_LINKS: usize = 16;
/// Feed relays taken from one profile.
pub const FEED_RELAYS_PER_FEED: usize = 2;
/// Links of an automatically downloaded article that are fetched with it.
pub const PREFETCH_LINKS: usize = 8;
/// How long an operation waiting for relays to connect waits for the rest
/// once one is connected.
pub const CONNECT_GRACE: Duration = Duration::from_millis(200);
/// Relays asked at once for a blob.
const GET_FANOUT: usize = 2;

const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// How an operation ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// The whole blob, verified against its ID.
    Got(Vec<u8>),
    /// The blob is stored (and accepted by a relay, if there is one).
    Put(BlobId),
    /// An article (or profile) and its entry are stored (and accepted by a
    /// relay, if there is one).
    Published { seq: u32, blob: BlobId },
    /// A search's merged, verified results (NETWORK 4.7).
    Found(Vec<SignedEntry>),
    NotFound,
    /// Every relay refused an entry (for example, a different entry already
    /// has its `seq`), or no relay answered a search and one refused it.
    Refused,
    Failed,
}

/// What one relay did with a put or an entry: see [`Node::take_delivery`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Accepted,
    /// The relay refused, and why (NETWORK 3.1).
    Refused(Refusal),
    /// The relay took the entry but refused its blob (an article's,
    /// profile's, program's or document's), and why.
    BlobRefused(Refusal),
    /// No answer, a connection error or an unexpected reply.
    Failed,
    /// Not connected when it was sent, so not sent there.
    NotConnected,
}

impl Delivery {
    fn of(accepted: bool, reply: &RelayReply) -> Self {
        match reply {
            _ if accepted => Delivery::Accepted,
            RelayReply::Refused(why) => Delivery::Refused(*why),
            _ => Delivery::Failed,
        }
    }
}

/// An article to publish.
#[derive(Clone, Debug)]
pub struct Draft {
    pub data: Vec<u8>,
    pub title: String,
    pub reply_to: Option<BlobId>,
    /// Unix seconds.
    pub time: u32,
}

/// Where an entry's article stands in the local store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Article {
    /// In the store, with the entry's size.
    Local,
    /// Not in the store: larger than the automatic download limit.
    NotFetched,
    /// The automatic download failed (not found, or a network error).
    Failed,
    /// The blob's length differs from the entry's size, so it is not
    /// associated with the entry (NETWORK 4.4).
    SizeMismatch,
}

/// What happened to the node's feeds and relays; see
/// [`Node::take_feed_events`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedEvent {
    /// A new valid entry for a subscribed feed is stored. Reported once any
    /// automatic download has finished (a program entry's at once: its image
    /// is downloaded once the feed is caught up, see `ProgramDownloaded`).
    Entry { entry: Entry, article: Article },
    /// The automatic download of the current version of a program
    /// (NETWORK 4.4) finished or failed.
    ProgramDownloaded { entry: Entry, article: Article },
    /// A relay accepted a subscription; entries it holds follow.
    Subscribed(FeedKey),
    SubscribeRefused(FeedKey),
    /// Connected to this relay; subscriptions are being renewed.
    Connected(String),
    /// Could not connect to this relay, or the connection was lost; the
    /// next attempt is in `retry_in`.
    Disconnected { url: String, retry_in: Duration },
    /// None of the host's own relays is connected.
    Unreachable,
    /// Every feed is caught up on every connected relay (NETWORK 4.3) and
    /// no automatic download is in progress.
    Settled,
    /// A subscription or download started after `Settled`; `Settled` follows
    /// when it is done.
    Busy,
    /// The result of `ContentService::publish`.
    Published { seq: u32, blob: BlobId },
    PublishFailed(Done),
}

/// What a relay request belongs to.
enum Waiting {
    Op(u64),
    /// A subscribe sent with this `after`.
    Subscribe(FeedKey, u32),
    Unsubscribe,
}

/// Why a blob is fetched.
enum GetFor {
    Token(Token),
    /// The automatic download of a new entry's blob.
    Auto(Entry),
    /// A blob linked from an automatically downloaded article.
    Prefetch,
    /// The image of a program's current version (NETWORK 4.4).
    Program(Entry),
}

/// An operation that may involve several relays.
enum Op {
    /// Relays still to ask, in order; at most [`GET_FANOUT`] asked at once.
    Get { purpose: GetFor, id: BlobId, queue: VecDeque<u64>, in_flight: usize, not_found: bool },
    /// Done at the first relay that accepts; kept until every relay asked
    /// has answered (`reported`: the token has had its answer), for
    /// [`Node::take_delivery`].
    Put { token: Token, id: BlobId, in_flight: usize, reported: bool, outcomes: Vec<(u64, Delivery)> },
    /// An article's (or profile's) blob on its way; its entry (`seq` not yet
    /// assigned) is published once a relay has it.
    /// `published`: a relay has taken the blob and the entry is on its
    /// way; the op is kept until every relay has answered, so that a
    /// relay's refusal of the blob can be told ([`Node::take_delivery`]).
    PublishBlob { token: Token, entry: Entry, in_flight: usize, published: bool, outcomes: Vec<(u64, Delivery)> },
    /// Done at the first relay that accepts, kept like `Put`.
    Entry { token: Token, signed: SignedEntry, in_flight: usize, refused: bool, reported: bool, outcomes: Vec<(u64, Delivery)> },
    /// The verified results of each relay that answered, by link.
    Find { token: Token, scope: u8, query: Vec<u8>, in_flight: usize, refused: bool, found: BTreeMap<u64, Vec<SignedEntry>> },
    /// The highest `seq` of the valid answers so far.
    Lookup { token: Token, feed: FeedKey, name: Vec<u8>, in_flight: usize, refused: bool, not_found: bool, best: Option<SignedEntry> },
}

/// One relay connection.
struct Link {
    url: String,
    client: RelayClient,
    /// `None`: one of the host's own relays, used for every subscribed feed.
    /// `Some`: a feed relay (NETWORK 5), used for these feeds.
    feeds: Option<BTreeSet<FeedKey>>,
    /// The connection generation the subscriptions were sent on (0: none).
    generation: u64,
    was_connected: bool,
    retry_at: Option<Instant>,
    retry_in: Duration,
    /// Feeds subscribed on this connection and not yet caught up, with the
    /// `highest` their subscribe reply gave.
    catching_up: HashMap<FeedKey, u32>,
}

impl Link {
    fn new(url: String, client: RelayClient, feeds: Option<BTreeSet<FeedKey>>) -> Self {
        Link {
            url,
            client,
            feeds,
            generation: 0,
            was_connected: false,
            retry_at: None,
            retry_in: FIRST_RETRY,
            catching_up: HashMap::new(),
        }
    }

    fn serves(&self, feed: &FeedKey) -> bool {
        self.feeds.as_ref().is_none_or(|f| f.contains(feed))
    }

    fn ready(&self) -> bool {
        self.client.connected() && self.client.generation() == self.generation
    }
}

type FeedConnector = Box<dyn Fn(&str) -> RelayClient + Send>;
/// An operation started again once a relay is connected or none is
/// connecting.
type Deferred = Box<dyn FnOnce(&mut Node) + Send>;

pub struct Node {
    store: BlobStore,
    links: BTreeMap<u64, Link>,
    next_link: u64,
    /// Relay requests by link and request number.
    waiting: HashMap<(u64, u32), Waiting>,
    ops: HashMap<u64, Op>,
    next_op: u64,
    ready: Vec<(Token, Done)>,
    feeds: FeedStore,
    subscriptions: Subscriptions,
    nicknames: Nicknames,
    identity: Option<Identity>,
    /// Where the identity is kept, so one can be created when first needed.
    identity_path: Option<PathBuf>,
    auto_limit: u32,
    /// The last `seq` given to an own entry, so publishes in flight do not
    /// reuse one.
    last_seq: u32,
    settled: bool,
    /// No own relay is connected (and `Unreachable` was reported).
    unreachable: bool,
    events: Vec<FeedEvent>,
    arrivals: Arrivals,
    /// Verified stored entries (`None`: invalid on disk), by feed and `seq`;
    /// stored entries never change.
    verified: HashMap<(FeedKey, u32), Option<(Entry, Vec<u8>)>>,
    /// A feed with an entry pointing at a blob, for choosing relays.
    blob_feeds: HashMap<BlobId, FeedKey>,
    /// The size an entry gives a blob, for prefetch (filled from the
    /// subscribed feeds when first needed, then as entries are stored).
    blob_sizes: HashMap<BlobId, u32>,
    blob_sizes_loaded: bool,
    /// Blobs a program entry (kind 3) names: images, not prefetched.
    program_blobs: BTreeSet<BlobId>,
    prefetch: bool,
    /// Download the current version of each program of the subscribed
    /// feeds (NETWORK 4.4).
    program_download: bool,
    /// Named records (SPEC §content.records), if the host keeps them.
    records: Option<crate::records::Records>,
    /// Snapshots of a shell's machines (SPEC §machines.snapshots), if the host keeps them.
    snapshots: Option<crate::snapshots::Snapshots>,
    /// What each relay did with the last put or entry every relay has
    /// answered.
    delivery: Vec<(String, Delivery)>,
    /// The relays that refused the blob of a publish in progress, by its
    /// token, and why.
    blob_refusals: HashMap<Token, Vec<(u64, Refusal)>>,
    /// See [`Node::set_deliver_to_all`].
    deliver_to_all: bool,
    /// Operations waiting for a relay to be connected, and since when.
    deferred: Vec<Deferred>,
    deferred_since: Option<Instant>,
    /// Feed relays (NETWORK 5) in use.
    feed_relays: bool,
    /// The feed relays must be worked out again (a profile or the
    /// subscriptions changed).
    feed_relays_dirty: bool,
    /// Also take loopback and private addresses from profiles (tests).
    feed_relays_unrestricted: bool,
    connect_feed_relay: FeedConnector,
    /// Feeds an own relay said it holds entries of (`highest` > 0).
    held_on_own: BTreeSet<FeedKey>,
    /// The own relays that said they hold no entry of a feed.
    empty_on_own: HashMap<FeedKey, BTreeSet<u64>>,
    /// Feeds subscribed with hints that none of the own relays holds: their
    /// hint relays are used (unless the profile names relays).
    hints_wanted: BTreeSet<FeedKey>,
    /// Feeds followed through a hint relay now.
    hinted: BTreeSet<FeedKey>,
    /// The hint relays in use, with the feeds followed on each because of
    /// a hint.
    hint_relays: BTreeMap<String, BTreeSet<FeedKey>>,
    /// The timeout of the own relays' connections ([`Node::set_relays`]).
    relay_timeout: Duration,
}

/// One relay connection, for [`Node::relay_links`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayLink {
    pub url: String,
    /// One of the host's own relays (not a feed relay).
    pub own: bool,
    /// A feed relay used because of a hint (NETWORK 4.9).
    pub hint: bool,
    pub connected: bool,
}

impl Node {
    /// A node on `store` (feed entries are kept beside its blobs) with no
    /// subscriptions and no identity, and at most one relay.
    pub fn new(store: BlobStore, relay: Option<RelayClient>) -> Self {
        let mut node = Node {
            feeds: FeedStore::beside_blobs(store.dir()),
            arrivals: Arrivals::beside_blobs(store.dir()),
            store,
            links: BTreeMap::new(),
            next_link: 0,
            waiting: HashMap::new(),
            ops: HashMap::new(),
            delivery: Vec::new(),
            blob_refusals: HashMap::new(),
            deliver_to_all: false,
            next_op: 0,
            ready: Vec::new(),
            subscriptions: Subscriptions::default(),
            nicknames: Nicknames::in_memory(),
            identity: None,
            identity_path: None,
            auto_limit: AUTO_DOWNLOAD_LIMIT,
            last_seq: 0,
            settled: false,
            unreachable: false,
            events: Vec::new(),
            verified: HashMap::new(),
            blob_feeds: HashMap::new(),
            blob_sizes: HashMap::new(),
            blob_sizes_loaded: false,
            program_blobs: BTreeSet::new(),
            prefetch: true,
            program_download: true,
            records: None,
            snapshots: None,
            deferred: Vec::new(),
            deferred_since: None,
            feed_relays: true,
            feed_relays_dirty: true,
            feed_relays_unrestricted: false,
            connect_feed_relay: Box::new(|url| RelayClient::websocket_checked(url, DEFAULT_TIMEOUT, true)),
            held_on_own: BTreeSet::new(),
            empty_on_own: HashMap::new(),
            hints_wanted: BTreeSet::new(),
            hinted: BTreeSet::new(),
            hint_relays: BTreeMap::new(),
            relay_timeout: DEFAULT_TIMEOUT,
        };
        if let Some(client) = relay {
            node.add_relay("relay", client);
        }
        node
    }

    /// A node on `store_dir` (default: [`default_store_dir`]) and `ws://`
    /// relays.
    pub fn open(store_dir: Option<PathBuf>, relays: &[String], timeout: Duration) -> Result<Self, String> {
        let dir = store_dir.or_else(default_store_dir).ok_or("no blob store directory (use --store)")?;
        let store = BlobStore::open(&dir, None).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut node = Node::new(store, None);
        for url in relays.iter().take(crate::local::MAX_RELAYS) {
            node.add_relay(url, RelayClient::websocket(url, timeout));
        }
        node.connect_feed_relay = Box::new(move |url| RelayClient::websocket_checked(url, timeout, true));
        node.relay_timeout = timeout;
        Ok(node)
    }

    /// Adds one of the host's own relays.
    pub fn add_relay(&mut self, url: &str, client: RelayClient) {
        self.add_link(Link::new(url.to_string(), client, None));
    }

    fn add_link(&mut self, link: Link) -> u64 {
        self.next_link += 1;
        self.links.insert(self.next_link, link);
        self.next_link
    }

    /// Replaces the host's own relays with `urls` (`ws://` relays, at most
    /// [`crate::local::MAX_RELAYS`]): connections to relays still listed
    /// are kept, the others are closed (their requests fail), and the new
    /// ones are connected on the next poll.
    pub fn set_relays(&mut self, urls: &[String]) {
        let urls: Vec<&String> = urls.iter().take(crate::local::MAX_RELAYS).collect();
        let drop: Vec<u64> =
            self.links.iter().filter(|(_, l)| l.feeds.is_none() && !urls.contains(&&l.url)).map(|(&id, _)| id).collect();
        for lid in drop {
            self.remove_link(lid);
            for set in self.empty_on_own.values_mut() {
                set.remove(&lid);
            }
        }
        let have = self.relay_urls();
        for url in urls.into_iter().filter(|u| !have.contains(u)) {
            // A feed relay at the same URL becomes an own relay.
            let feed_link: Vec<u64> = self.links.iter().filter(|(_, l)| &l.url == url).map(|(&id, _)| id).collect();
            for lid in feed_link {
                self.remove_link(lid);
            }
            self.add_relay(url, RelayClient::websocket(url, self.relay_timeout));
        }
        self.feed_relays_dirty = true;
    }

    /// Every relay connection: the own relays first, in the order they
    /// were added, then the feed relays.
    pub fn relay_links(&self) -> Vec<RelayLink> {
        let link = |l: &Link| RelayLink {
            url: l.url.clone(),
            own: l.feeds.is_none(),
            hint: l.feeds.is_some() && self.hint_relays.contains_key(&l.url),
            connected: l.client.connected(),
        };
        let own = self.links.values().filter(|l| l.feeds.is_none()).map(link);
        own.chain(self.links.values().filter(|l| l.feeds.is_some()).map(link)).collect()
    }

    /// The hint relays `feed` is followed on now (NETWORK 4.9).
    pub fn hint_relays(&self, feed: &FeedKey) -> Vec<String> {
        self.hint_relays.iter().filter(|(_, f)| f.contains(feed)).map(|(u, _)| u.clone()).collect()
    }

    /// Whether every subscribed feed is caught up (NETWORK 4.3) on every
    /// connected relay and no automatic download is running, with at least
    /// one relay connected: the state [`FeedEvent::Settled`] reports, but
    /// also true with no subscriptions.
    pub fn caught_up(&self) -> bool {
        !self.settle_pending() && self.connections_current()
    }

    fn settle_pending(&self) -> bool {
        self.links.values().any(|l| !l.catching_up.is_empty())
            || self.waiting.values().any(|w| matches!(w, Waiting::Subscribe(..)))
            || self.ops.values().any(|o| matches!(o, Op::Get { purpose: GetFor::Auto(_) | GetFor::Prefetch | GetFor::Program(_), .. }))
    }

    /// Some relay is connected, and every connected one has had the
    /// subscriptions sent on its current connection.
    fn connections_current(&self) -> bool {
        let mut connected = self.links.values().filter(|l| l.client.connected()).peekable();
        connected.peek().is_some() && connected.all(|l| l.ready())
    }

    /// Whether to use feed relays (NETWORK 5); on by default.
    pub fn set_feed_relays(&mut self, on: bool) {
        self.feed_relays = on;
        self.feed_relays_dirty = true;
    }

    /// Takes feed relays at loopback and private addresses too, connecting
    /// with `connect` (for tests, where every relay is on loopback).
    pub fn set_feed_relays_unrestricted(&mut self, connect: FeedConnector) {
        self.feed_relays_unrestricted = true;
        self.connect_feed_relay = connect;
        self.feed_relays_dirty = true;
    }

    /// The URLs of the connected relays, own and feed relays.
    pub fn connected_relays(&self) -> Vec<String> {
        self.links.values().filter(|l| l.client.connected()).map(|l| l.url.clone()).collect()
    }

    /// The URLs of the host's own relays.
    pub fn relay_urls(&self) -> Vec<String> {
        self.links.values().filter(|l| l.feeds.is_none()).map(|l| l.url.clone()).collect()
    }

    pub fn store(&self) -> &BlobStore {
        &self.store
    }

    pub fn feed_store(&self) -> &FeedStore {
        &self.feeds
    }

    pub fn subscriptions(&self) -> &Subscriptions {
        &self.subscriptions
    }

    pub fn set_subscriptions(&mut self, subscriptions: Subscriptions) {
        self.subscriptions = subscriptions;
        // Subscribe to the new list on the next poll.
        for link in self.links.values_mut() {
            link.generation = 0;
        }
        self.feed_relays_dirty = true;
    }

    pub fn nicknames(&self) -> &Nicknames {
        &self.nicknames
    }

    pub fn set_nicknames(&mut self, nicknames: Nicknames) {
        self.nicknames = nicknames;
    }

    /// Sets or (with `None` or an empty name) removes a nickname; false if
    /// it already was as asked.
    pub fn set_nickname(&mut self, feed: FeedKey, name: Option<&str>) -> std::io::Result<bool> {
        self.nicknames.set(feed, name)
    }

    /// The identity whose feed `publish_article` writes to.
    pub fn set_identity(&mut self, identity: Identity) {
        self.identity = Some(identity);
    }

    /// Keeps the identity in `path` (`identity.key` in the config
    /// directory): loads it if the file exists; otherwise
    /// [`Node::ensure_identity`] creates it there when first needed.
    pub fn set_identity_path(&mut self, path: PathBuf) -> Result<(), String> {
        if path.exists() {
            self.identity = Some(Identity::load(&path)?);
        }
        self.identity_path = Some(path);
        Ok(())
    }

    /// The identity's feed key, if there is an identity.
    pub fn identity_key(&self) -> Option<FeedKey> {
        self.identity.as_ref().map(Identity::feed_key)
    }

    /// The identity, created (and saved) if there is none yet and the node
    /// has an identity path.
    pub fn ensure_identity(&mut self) -> Result<FeedKey, String> {
        if self.identity.is_none() {
            let path = self.identity_path.as_ref().ok_or("no identity and nowhere to keep one")?;
            // Another process may have created it meanwhile.
            self.identity = Some(if path.exists() { Identity::load(path)? } else { Identity::create(path)? });
        }
        Ok(self.identity_key().unwrap())
    }

    pub fn set_auto_download_limit(&mut self, bytes: u32) {
        self.auto_limit = bytes;
    }

    /// Whether to fetch the `terse:` links of automatically downloaded
    /// articles (NETWORK 4.4); on by default.
    pub fn set_prefetch(&mut self, on: bool) {
        self.prefetch = on;
    }

    /// Whether to download the current versions of programs automatically
    /// (NETWORK 4.4); on by default.
    pub fn set_program_download(&mut self, on: bool) {
        self.program_download = on;
    }

    /// Keeps named records (SPEC §content.records) in `records`.
    pub fn set_records(&mut self, records: crate::records::Records) {
        self.records = Some(records);
    }

    pub fn records(&self) -> Option<&crate::records::Records> {
        self.records.as_ref()
    }

    /// Keeps the snapshots a shell asks for (SPEC §machines.snapshots) in `snapshots`.
    pub fn set_snapshots(&mut self, snapshots: crate::snapshots::Snapshots) {
        self.snapshots = Some(snapshots);
    }

    pub fn snapshots(&self) -> Option<&crate::snapshots::Snapshots> {
        self.snapshots.as_ref()
    }

    /// The current version of each program of a feed (NETWORK 4.2: its
    /// valid kind-3 entry with that name and the highest `seq`), in
    /// increasing order of name (byte order).
    pub fn current_programs(&self, feed: &FeedKey) -> Vec<SignedEntry> {
        let mut current: BTreeMap<Vec<u8>, SignedEntry> = BTreeMap::new();
        for s in self.feeds.entries(feed).unwrap_or_default().into_iter().filter(|s| s.entry.kind == KIND_PROGRAM) {
            // Entries come in increasing seq order.
            current.insert(s.entry.title.as_bytes().to_vec(), s);
        }
        current.into_values().collect()
    }

    /// The current version of the program `name` of `feed`, if the host
    /// holds one (SPEC §machines.calls `start_program`).
    pub fn current_program(&self, feed: &FeedKey, name: &[u8]) -> Option<Entry> {
        self.current_programs(feed).into_iter().find(|s| s.entry.title.as_bytes() == name).map(|s| s.entry)
    }

    /// The profile in force for a feed (NETWORK 4.6) and its document, if
    /// the document is in the store and parses.
    pub fn profile(&self, feed: &FeedKey) -> Option<(Entry, Option<Document>)> {
        let signed = self.feeds.latest_profile(feed).ok()??;
        let doc = match self.store.read(&signed.entry.blob) {
            Ok(Some(bytes)) if blob_id(&bytes) == signed.entry.blob => Document::parse(&bytes),
            _ => None,
        };
        Some((signed.entry, doc))
    }

    /// The name to show for a feed: its nickname (`true`), else the display
    /// name of its profile in force (`false`).
    pub fn display_name(&self, feed: &FeedKey) -> Option<(String, bool)> {
        if let Some(n) = self.nicknames.get(feed) {
            return Some((n.to_string(), true));
        }
        self.profile(feed).map(|(e, _)| (e.title, false))
    }

    fn new_op(&mut self, op: Op) -> u64 {
        self.next_op += 1;
        self.ops.insert(self.next_op, op);
        self.next_op
    }

    /// The connected relays, in link order.
    fn connected_links(&self) -> Vec<u64> {
        self.links.iter().filter(|(_, l)| l.client.connected()).map(|(&id, _)| id).collect()
    }

    /// Whether some relay is being connected in the background.
    pub fn connecting(&self) -> bool {
        self.links.values().any(|l| l.client.connecting())
    }

    /// Keeps `start` for later if relays are being connected and none is
    /// connected yet; returns whether it did. It is started once no relay
    /// is being connected, or [`CONNECT_GRACE`] after one is connected.
    fn defer(&mut self, start: impl FnOnce(&mut Node) + Send + 'static) -> bool {
        if !self.connecting() || !self.connected_links().is_empty() {
            return false;
        }
        self.deferred.push(Box::new(start));
        self.deferred_since.get_or_insert_with(Instant::now);
        true
    }

    /// Starts fetching the whole blob `id`.
    pub fn start_get(&mut self, token: Token, id: BlobId) {
        match self.store.read(&id) {
            Ok(Some(bytes)) if blob_id(&bytes) == id => return self.ready.push((token, Done::Got(bytes))),
            // A damaged local copy is discarded and fetched again.
            Ok(Some(_)) => {
                let _ = self.store.remove(&id);
            }
            Ok(None) => {}
            Err(_) => return self.ready.push((token, Done::Failed)),
        }
        if self.links.is_empty() {
            return self.ready.push((token, Done::NotFound));
        }
        self.maintain();
        if self.defer(move |n| n.start_get(token, id)) {
            return;
        }
        self.fetch_from_relays(GetFor::Token(token), id);
    }

    /// Asks the connected relays for `id`: first the relays named in the
    /// profile of a feed with an entry pointing at it, then the rest, two
    /// at a time (NETWORK 5).
    fn fetch_from_relays(&mut self, purpose: GetFor, id: BlobId) {
        let preferred: Vec<String> = self
            .blob_feeds
            .get(&id)
            .and_then(|feed| self.profile(feed))
            .and_then(|(_, doc)| doc)
            .map(|d| d.relays)
            .unwrap_or_default();
        let mut queue: Vec<u64> = self.connected_links();
        queue.sort_by_key(|lid| preferred.iter().position(|u| *u == self.links[lid].url).unwrap_or(usize::MAX));
        let op = self.new_op(Op::Get { purpose, id, queue: queue.into(), in_flight: 0, not_found: false });
        self.advance_get(op);
    }

    /// Sends a get's next requests, or finishes it.
    fn advance_get(&mut self, op_id: u64) {
        let Some(Op::Get { id, queue, in_flight, .. }) = self.ops.get_mut(&op_id) else { return };
        let id = *id;
        while *in_flight < GET_FANOUT {
            let Some(lid) = queue.pop_front() else { break };
            let Some(link) = self.links.get_mut(&lid).filter(|l| l.client.connected()) else { continue };
            let req = link.client.get(&id);
            self.waiting.insert((lid, req), Waiting::Op(op_id));
            *in_flight += 1;
        }
        if *in_flight == 0 {
            let Some(Op::Get { purpose, not_found, .. }) = self.ops.remove(&op_id) else { unreachable!() };
            match purpose {
                GetFor::Token(token) => self.ready.push((token, if not_found { Done::NotFound } else { Done::Failed })),
                GetFor::Auto(entry) => self.downloaded(entry, None),
                GetFor::Program(entry) => self.events.push(FeedEvent::ProgramDownloaded { entry, article: Article::Failed }),
                GetFor::Prefetch => {}
            }
        }
    }

    /// Starts publishing `data`.
    pub fn start_put(&mut self, token: Token, data: &[u8]) {
        let id = match self.store.put(data) {
            Ok(id) => id,
            Err(_) => return self.ready.push((token, Done::Failed)),
        };
        if self.links.is_empty() {
            return self.ready.push((token, Done::Put(id)));
        }
        self.maintain();
        let copy = data.to_vec();
        if self.defer(move |n| n.start_put(token, &copy)) {
            return;
        }
        let op = self.new_op(Op::Put { token, id, in_flight: 0, reported: false, outcomes: Vec::new() });
        self.send_put(op, data);
    }

    /// Sends a put to every connected relay for `op`, or fails it.
    fn send_put(&mut self, op_id: u64, data: &[u8]) {
        let mut sent = 0;
        for lid in self.connected_links() {
            let req = self.links.get_mut(&lid).unwrap().client.put(data);
            self.waiting.insert((lid, req), Waiting::Op(op_id));
            sent += 1;
        }
        match self.ops.get_mut(&op_id) {
            Some(Op::Put { in_flight, .. } | Op::PublishBlob { in_flight, .. }) => *in_flight = sent,
            _ => {}
        }
        if sent == 0 {
            match self.ops.remove(&op_id) {
                Some(Op::Put { token, .. } | Op::PublishBlob { token, .. }) => self.ready.push((token, Done::Failed)),
                _ => {}
            }
        }
    }

    /// Starts publishing an article (NETWORK 4.4): puts the blob, then signs
    /// and publishes an entry with the next `seq` of the identity's feed.
    pub fn start_publish_article(&mut self, token: Token, draft: &Draft) {
        let Some(identity) = &self.identity else { return self.ready.push((token, Done::Failed)) };
        if draft.title.len() > MAX_TITLE {
            return self.ready.push((token, Done::Failed));
        }
        let entry = Entry::article(identity.feed_key(), 0, draft.time, &draft.data, draft.reply_to, &draft.title);
        self.start_publish(token, &draft.data, entry);
    }

    /// Starts publishing a profile (NETWORK 4.6) for the identity's feed
    /// (created if need be): display name `name`, `listed`, the description
    /// of the profile in force if there is one, and the host's relays that
    /// are not loopback addresses.
    pub fn start_publish_profile(&mut self, token: Token, name: &str, listed: bool, time: u32) {
        let description = self.ensure_identity().ok().map(|key| {
            self.profile(&key).and_then(|(_, doc)| doc).map(|d| d.description).unwrap_or_default()
        });
        self.start_publish_profile_with(token, name, listed, description, time);
    }

    /// [`Self::start_publish_profile`] with a given description (`None`: no
    /// identity could be had, which fails).
    pub fn start_publish_profile_with(&mut self, token: Token, name: &str, listed: bool, description: Option<String>, time: u32) {
        let (Some(description), Some(identity)) = (description, &self.identity) else {
            return self.ready.push((token, Done::Failed));
        };
        if !profile::valid_name(name) {
            return self.ready.push((token, Done::Failed));
        }
        let relays = self.relay_urls().into_iter().filter(|u| !profile::loopback_url(u)).collect();
        let data = Document { listed, relays, description }.build();
        let entry = Entry::profile(identity.feed_key(), 0, time, &data, name);
        self.start_publish(token, &data, entry);
    }

    /// Starts publishing a program (NETWORK 4.2): `image` under the name
    /// `name` (a display name), as the next entry of the identity's feed.
    /// Whether the blob is a valid image is the caller's business.
    pub fn start_publish_program(&mut self, token: Token, image: &[u8], name: &str, time: u32) {
        let Some(identity) = &self.identity else { return self.ready.push((token, Done::Failed)) };
        if !profile::valid_name(name) || !valid_size(image.len()) {
            return self.ready.push((token, Done::Failed));
        }
        let entry = Entry::program(identity.feed_key(), 0, time, image, name);
        self.start_publish(token, image, entry);
    }

    /// Puts `data` and then publishes `entry` (its `seq` still to assign).
    fn start_publish(&mut self, token: Token, data: &[u8], entry: Entry) {
        if self.store.put(data).is_err() {
            return self.ready.push((token, Done::Failed));
        }
        if self.links.is_empty() {
            return self.publish_entry(token, entry);
        }
        self.maintain();
        let (copy, later) = (data.to_vec(), entry.clone());
        if self.defer(move |n| n.start_publish(token, &copy, later)) {
            return;
        }
        let op = self.new_op(Op::PublishBlob { token, entry, in_flight: 0, published: false, outcomes: Vec::new() });
        self.send_put(op, data);
    }

    /// Starts publishing blob `id`, which must be in the local store, as the
    /// next entry of the identity's feed (creating the identity if needed):
    /// `NotFound` if the store does not hold it.
    /// `kind` is [`KIND_ARTICLE`] or [`KIND_DOCUMENT`].
    pub fn start_publish_blob(&mut self, token: Token, id: BlobId, title: &str, kind: u8, time: u32) {
        let data = match self.store.read(&id) {
            Ok(Some(bytes)) if blob_id(&bytes) == id => bytes,
            Ok(_) => return self.ready.push((token, Done::NotFound)),
            Err(_) => return self.ready.push((token, Done::Failed)),
        };
        if self.ensure_identity().is_err() {
            return self.ready.push((token, Done::Failed));
        }
        if kind == KIND_DOCUMENT {
            return self.start_publish_document(token, &data, title, time);
        }
        self.start_publish_article(token, &Draft { data, title: title.into(), reply_to: None, time });
    }

    /// Starts publishing a document (NETWORK 4.2, kind 4) titled `title`;
    /// whether `data` has a document header is the caller's business.
    pub fn start_publish_document(&mut self, token: Token, data: &[u8], title: &str, time: u32) {
        let Some(identity) = &self.identity else { return self.ready.push((token, Done::Failed)) };
        if title.len() > MAX_TITLE || !valid_size(data.len()) {
            return self.ready.push((token, Done::Failed));
        }
        let entry = Entry::document(identity.feed_key(), 0, time, data, title);
        self.start_publish(token, data, entry);
    }

    fn publish_entry(&mut self, token: Token, mut entry: Entry) {
        let Some(identity) = &self.identity else { return self.ready.push((token, Done::Failed)) };
        let Ok(highest) = self.feeds.highest(&entry.feed) else { return self.ready.push((token, Done::Failed)) };
        entry.seq = highest.max(self.last_seq) + 1;
        let Ok(signed) = entry.sign(identity) else { return self.ready.push((token, Done::Failed)) };
        self.last_seq = signed.entry.seq;
        if self.links.is_empty() {
            return self.store_own_entry(token, &signed);
        }
        let targets = self.connected_links();
        if targets.is_empty() {
            return self.ready.push((token, Done::Failed));
        }
        let op = self.new_op(Op::Entry {
            token,
            signed: signed.clone(),
            in_flight: targets.len(),
            refused: false,
            reported: false,
            outcomes: Vec::new(),
        });
        for lid in targets {
            let req = self.links.get_mut(&lid).unwrap().client.publish(&signed.bytes);
            self.waiting.insert((lid, req), Waiting::Op(op));
        }
    }

    fn store_own_entry(&mut self, token: Token, signed: &SignedEntry) {
        let done = match self.insert(signed) {
            Ok(insert @ (Insert::New | Insert::Duplicate)) => {
                // A new entry of a subscribed feed, like one from a relay
                // (its blob is in the store).
                if insert == Insert::New && self.subscriptions.contains(&signed.entry.feed) {
                    self.events.push(FeedEvent::Entry { entry: signed.entry.clone(), article: Article::Local });
                }
                Done::Published { seq: signed.entry.seq, blob: signed.entry.blob }
            }
            _ => Done::Failed,
        };
        self.ready.push((token, done));
    }

    /// Stores an entry and records the arrival of a new one.
    fn insert(&mut self, signed: &SignedEntry) -> std::io::Result<Insert> {
        let insert = self.feeds.insert(signed)?;
        if insert == Insert::New {
            // A failure only leaves the entry at the old end of the timeline.
            let _ = self.arrivals.record(&signed.entry.feed, signed.entry.seq);
            self.blob_feeds.insert(signed.entry.blob, signed.entry.feed);
            self.blob_sizes.insert(signed.entry.blob, signed.entry.size);
            if signed.entry.kind == KIND_PROGRAM {
                self.program_blobs.insert(signed.entry.blob);
            }
            if signed.entry.kind == KIND_PROFILE {
                self.feed_relays_dirty = true;
            }
        }
        Ok(insert)
    }

    /// Starts a search (NETWORK 4.7) on every connected relay: `scope` 1
    /// (feeds) or 2 (entries). Fails if no relay could be asked.
    pub fn start_find(&mut self, token: Token, scope: u8, query: &[u8]) {
        if !(SCOPE_FEEDS..=SCOPE_PROGRAMS).contains(&scope) || !(1..=profile::MAX_NAME).contains(&query.len()) {
            return self.ready.push((token, Done::Failed));
        }
        self.maintain();
        let copy = query.to_vec();
        if self.defer(move |n| n.start_find(token, scope, &copy)) {
            return;
        }
        let targets = self.connected_links();
        if targets.is_empty() {
            return self.ready.push((token, Done::Failed));
        }
        let op = self.new_op(Op::Find {
            token,
            scope,
            query: query.to_vec(),
            in_flight: targets.len(),
            refused: false,
            found: BTreeMap::new(),
        });
        for lid in targets {
            let req = self.links.get_mut(&lid).unwrap().client.find(scope, query);
            self.waiting.insert((lid, req), Waiting::Op(op));
        }
    }

    /// Asks every connected relay for the current version of the program
    /// `name` of `feed` (NETWORK 4.8); a valid answer is stored like a
    /// received entry and finishes as `Done::Found` with that entry (the
    /// highest `seq` of the answers); `NotFound` if a relay said so and
    /// none had it, `Refused` if they refused, else `Failed`.
    pub fn start_lookup(&mut self, token: Token, feed: FeedKey, name: &[u8]) {
        if !std::str::from_utf8(name).is_ok_and(profile::valid_name) {
            return self.ready.push((token, Done::Failed));
        }
        self.maintain();
        let copy = name.to_vec();
        if self.defer(move |n| n.start_lookup(token, feed, &copy)) {
            return;
        }
        let targets = self.connected_links();
        if targets.is_empty() {
            return self.ready.push((token, Done::NotFound));
        }
        let op = self.new_op(Op::Lookup {
            token,
            feed,
            name: name.to_vec(),
            in_flight: targets.len(),
            refused: false,
            not_found: false,
            best: None,
        });
        for lid in targets {
            let req = self.links.get_mut(&lid).unwrap().client.lookup(&feed, KIND_PROGRAM, name);
            self.waiting.insert((lid, req), Waiting::Op(op));
        }
    }

    /// Adds a feed to the subscriptions (saved if they are backed by a file)
    /// and subscribes on the current connections.
    pub fn subscribe(&mut self, feed: FeedKey) -> std::io::Result<()> {
        self.subscribe_with_hints(feed, None)
    }

    /// Subscribes, remembering the hint list of the feed's address
    /// (NETWORK 4.9; valid, else ignored). For a feed already subscribed,
    /// hints given replace those remembered.
    pub fn subscribe_with_hints(&mut self, feed: FeedKey, hints: Option<&str>) -> std::io::Result<()> {
        let hints = hints.filter(|h| !h.is_empty() && h.len() <= crate::address::MAX_HINT_LIST && crate::address::parse_hints(h.as_bytes()).is_ok());
        let added = self.subscriptions.add(feed)?;
        if hints.is_some() && self.subscriptions.set_hints(&feed, hints)? {
            self.feed_relays_dirty = true;
            self.check_hints(&feed);
        }
        if added {
            let after = self.feeds.highest(&feed).unwrap_or(0);
            for (&lid, link) in self.links.iter_mut().filter(|(_, l)| l.ready() && l.serves(&feed)) {
                let req = link.client.subscribe(&feed, after);
                self.waiting.insert((lid, req), Waiting::Subscribe(feed, after));
            }
            self.feed_relays_dirty = true;
        }
        Ok(())
    }

    pub fn unsubscribe(&mut self, feed: &FeedKey) -> std::io::Result<()> {
        if self.subscriptions.remove(feed)? {
            self.held_on_own.remove(feed);
            self.empty_on_own.remove(feed);
            self.hints_wanted.remove(feed);
            for (&lid, link) in self.links.iter_mut() {
                link.catching_up.remove(feed);
                if link.client.connected() && link.serves(feed) {
                    let req = link.client.unsubscribe(feed);
                    self.waiting.insert((lid, req), Waiting::Unsubscribe);
                }
            }
            self.feed_relays_dirty = true;
        }
        Ok(())
    }

    /// Whether `feed` is followed through a relay named by a hint of its
    /// address (NETWORK 4.9) now.
    pub fn followed_by_hint(&self, feed: &FeedKey) -> bool {
        self.hinted.contains(feed)
    }

    /// Whether an own relay said it holds entries of `feed`; `None` if
    /// none has answered for it yet (or the feed is not subscribed).
    pub fn held_on_own_relays(&self, feed: &FeedKey) -> Option<bool> {
        if self.held_on_own.contains(feed) {
            Some(true)
        } else {
            self.empty_on_own.get(feed).is_some_and(|s| !s.is_empty()).then_some(false)
        }
    }

    /// The hints of the host's own feed address (NETWORK 4.9): from its
    /// profile's relay lines, then its own relays, public ones only, at
    /// most two.
    pub fn own_hints(&self) -> Vec<crate::address::Hint> {
        let profile_relays = self
            .identity_key()
            .and_then(|k| self.profile(&k))
            .and_then(|(_, d)| d)
            .map(|d| d.relays)
            .unwrap_or_default();
        crate::address::own_hints(&profile_relays, &self.relay_urls())
    }

    /// Works out whether the hint relays of `feed` are wanted: it was
    /// subscribed with hints and none of the own relays holds it (each
    /// connected one said so, or none is configured or reachable).
    fn check_hints(&mut self, feed: &FeedKey) {
        if self.subscriptions.hints(feed).is_none() || self.held_on_own.contains(feed) || self.hints_wanted.contains(feed) {
            return;
        }
        let own: Vec<(u64, bool, bool)> = self
            .links
            .iter()
            .filter(|(_, l)| l.feeds.is_none())
            .map(|(&id, l)| (id, l.client.connected(), !l.client.connected() && !l.client.connecting() && l.retry_at.is_some()))
            .collect();
        let answered = self.empty_on_own.get(feed);
        let connected: Vec<u64> = own.iter().filter(|o| o.1).map(|o| o.0).collect();
        // Every own relay was tried and could not be reached.
        let none_reachable = own.iter().all(|o| o.2);
        let all_empty = !connected.is_empty() && connected.iter().all(|id| answered.is_some_and(|a| a.contains(id)));
        if own.is_empty() || none_reachable || all_empty {
            self.hints_wanted.insert(*feed);
            self.feed_relays_dirty = true;
        }
    }

    /// Whether any operation is waiting for a relay.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty() || !self.deferred.is_empty() || !self.ready.is_empty()
    }

    /// Whether the node has subscriptions and a relay, so it follows feeds.
    pub fn following(&self) -> bool {
        !self.links.is_empty() && !self.subscriptions.is_empty()
    }

    /// Whether the node has relays, so it must be polled (to reconnect)
    /// even when not busy.
    pub fn has_relays(&self) -> bool {
        !self.links.is_empty()
    }

    pub fn connected(&self) -> bool {
        self.links.values().any(|l| l.client.connected())
    }

    /// Feed events since the last call.
    pub fn take_feed_events(&mut self) -> Vec<FeedEvent> {
        std::mem::take(&mut self.events)
    }

    /// Where an entry's article stands in the local store (by length; the
    /// hash is checked when the article is read).
    pub fn held_article(&self, entry: &Entry) -> Option<Article> {
        let len = std::fs::metadata(self.store.path(&entry.blob)).ok()?.len();
        Some(if len == entry.size as u64 { Article::Local } else { Article::SizeMismatch })
    }

    /// The subscriptions records from index `offset` that fit in `length`
    /// bytes, and how many exist (SPEC §content.feeds). The order is the key order.
    pub fn subscription_records(&self, offset: u32, length: u16) -> (u32, Vec<u8>) {
        let feeds: Vec<&FeedKey> = self.subscriptions.feeds().collect();
        let records = feeds
            .iter()
            .skip(offset as usize)
            .map(|feed| timeline::subscription_record(feed, self.feeds.highest(feed).unwrap_or(0)));
        (feeds.len() as u32, timeline::fill(records, length as usize))
    }

    /// The names records from index `offset` that fit in `length` bytes,
    /// and how many exist (SPEC §content.feeds): one per subscribed feed in key order.
    pub fn name_records(&self, offset: u32, length: u16) -> (u32, Vec<u8>) {
        let subscribed: BTreeSet<&FeedKey> = self.subscriptions.feeds().collect();
        let feeds: BTreeSet<&FeedKey> = subscribed.iter().copied().chain(self.nicknames.feeds()).collect();
        let records = feeds.iter().skip(offset as usize).map(|feed| {
            let declared = self.feeds.latest_profile(feed).ok().flatten().map(|p| p.entry.title);
            let nickname = self.nicknames.get(feed);
            let flags = nickname.is_some() as u8
                | (declared.is_some() as u8) << 1
                | (!subscribed.contains(feed) as u8) << 2
                | (self.hinted.contains(*feed) as u8) << 3;
            let name = nickname.map(str::to_string).or(declared).unwrap_or_default();
            timeline::name_record(feed, flags, name.as_bytes())
        });
        (feeds.len() as u32, timeline::fill(records, length as usize))
    }

    /// The stored article entries of the subscribed feeds, newest arrival
    /// first (SPEC §content.feeds). Entries of feeds no longer subscribed are left out
    /// (and come back in place if the feed is subscribed again); entries
    /// stored without an arrival record (by an older version) come last,
    /// highest `seq` first. Entries that fail verification and profile
    /// entries are left out.
    pub fn timeline(&mut self) -> Vec<(FeedKey, u32)> {
        let mut held: HashMap<FeedKey, BTreeSet<u32>> = self
            .subscriptions
            .feeds()
            .map(|feed| (*feed, self.feeds.seqs(feed).unwrap_or_default().into_iter().collect()))
            .collect();
        let order = self.arrivals.order().map(<[_]>::to_vec).unwrap_or_default();
        let mut out: Vec<(FeedKey, u32)> = order
            .into_iter()
            .rev()
            .filter(|(feed, seq)| held.get_mut(feed).is_some_and(|seqs| seqs.remove(seq)))
            .collect();
        let mut rest: Vec<(FeedKey, u32)> =
            held.into_iter().flat_map(|(feed, seqs)| seqs.into_iter().map(move |seq| (feed, seq))).collect();
        rest.sort_by(|a, b| (b.1, b.0).cmp(&(a.1, a.0)));
        out.extend(rest);
        out.retain(|&(feed, seq)| self.verified_entry(feed, seq).is_some_and(|(e, _)| e.kind == KIND_ARTICLE || e.kind == KIND_DOCUMENT));
        out
    }

    /// A stored entry and its bytes without the signature, verified once.
    fn verified_entry(&mut self, feed: FeedKey, seq: u32) -> Option<&(Entry, Vec<u8>)> {
        let feeds = &self.feeds;
        let found = self
            .verified
            .entry((feed, seq))
            .or_insert_with(|| {
                let signed = entry::decode(&feeds.get(&feed, seq).ok()??).ok()?;
                let unsigned = signed.bytes[..signed.bytes.len() - entry::SIGNATURE_LEN].to_vec();
                (signed.entry.feed == feed && signed.entry.seq == seq).then_some((signed.entry, unsigned))
            })
            .as_ref();
        if let Some((e, _)) = found {
            self.blob_feeds.entry(e.blob).or_insert(feed);
        }
        found
    }

    /// The timeline records from index `offset` that fit in `length` bytes,
    /// and how many exist (SPEC §content.feeds).
    pub fn timeline_records(&mut self, offset: u32, length: u16) -> (u32, Vec<u8>) {
        let list = self.timeline();
        let mut records = Vec::new();
        let mut used = 0;
        for &(feed, seq) in list.iter().skip(offset as usize) {
            let Some((entry, unsigned)) = self.verified_entry(feed, seq).cloned() else { continue };
            if used + 2 + unsigned.len() > length as usize {
                break;
            }
            let local = self.held_article(&entry) == Some(Article::Local);
            used += 2 + unsigned.len();
            records.push(timeline::timeline_record(&unsigned, local));
        }
        (list.len() as u32, records.concat())
    }

    /// The programs records from index `offset` that fit in `length`
    /// bytes, and how many exist (SPEC §content.feeds): the current version of each
    /// program of the subscribed feeds, by feed key, then name.
    pub fn program_records(&self, offset: u32, length: u16) -> (u32, Vec<u8>) {
        let list: Vec<SignedEntry> = self.subscriptions.feeds().flat_map(|f| self.current_programs(f)).collect();
        let records = list.iter().skip(offset as usize).map(|s| {
            let unsigned = &s.bytes[..s.bytes.len() - entry::SIGNATURE_LEN];
            timeline::timeline_record(unsigned, self.held_article(&s.entry) == Some(Article::Local))
        });
        (list.len() as u32, timeline::fill(records, length as usize))
    }

    /// Search results as records (SPEC §content.feeds): the timeline record of each,
    /// as many as fit in `length` bytes, and how many there are.
    pub fn find_records(&self, found: &[SignedEntry], length: u16) -> (u32, Vec<u8>) {
        let records = found.iter().map(|s| {
            let unsigned = &s.bytes[..s.bytes.len() - entry::SIGNATURE_LEN];
            timeline::timeline_record(unsigned, self.held_article(&s.entry) == Some(Article::Local))
        });
        (found.len() as u32, timeline::fill(records, length as usize))
    }

    /// Connects relays and renews subscriptions when needed (NETWORK 4.4),
    /// and works out the feed relays when a profile or the subscriptions
    /// changed (NETWORK 5).
    fn maintain(&mut self) {
        let undecided: Vec<FeedKey> =
            self.subscriptions.hinted().map(|(f, _)| *f).filter(|f| !self.hints_wanted.contains(f) && !self.held_on_own.contains(f)).collect();
        for feed in undecided {
            self.check_hints(&feed);
        }
        if self.feed_relays_dirty {
            self.feed_relays_dirty = false;
            self.update_feed_relays();
        }
        let now = Instant::now();
        let ids: Vec<u64> = self.links.keys().copied().collect();
        for lid in ids {
            let link = self.links.get_mut(&lid).unwrap();
            if !link.client.connected() {
                if std::mem::take(&mut link.was_connected) {
                    link.retry_at = Some(now + link.retry_in);
                    link.catching_up.clear();
                    self.events.push(FeedEvent::Disconnected { url: link.url.clone(), retry_in: link.retry_in });
                }
                if link.client.connecting() {
                    continue;
                }
                let mut failed = link.client.take_connect_failure().is_some();
                if !failed {
                    if link.retry_at.is_some_and(|t| now < t) {
                        continue;
                    }
                    link.client.start_connect();
                    failed = link.client.take_connect_failure().is_some();
                }
                if failed {
                    self.events.push(FeedEvent::Disconnected { url: link.url.clone(), retry_in: link.retry_in });
                    link.retry_at = Some(now + link.retry_in);
                    link.retry_in = (link.retry_in * 2).min(MAX_RETRY);
                    continue;
                }
                if !link.client.connected() {
                    continue;
                }
            }
            if link.client.generation() != link.generation {
                link.generation = link.client.generation();
                link.was_connected = true;
                link.retry_at = None;
                link.retry_in = FIRST_RETRY;
                link.catching_up.clear();
                self.settled = false;
                self.events.push(FeedEvent::Connected(link.url.clone()));
                let feeds: Vec<FeedKey> = self.subscriptions.feeds().filter(|f| link.serves(f)).copied().collect();
                for feed in feeds {
                    let after = self.feeds.highest(&feed).unwrap_or(0);
                    let req = link.client.subscribe(&feed, after);
                    self.waiting.insert((lid, req), Waiting::Subscribe(feed, after));
                }
            }
        }
        let own: Vec<&Link> = self.links.values().filter(|l| l.feeds.is_none()).collect();
        let down = !own.is_empty() && own.iter().all(|l| !l.client.connected() && !l.client.connecting());
        if down && !self.unreachable {
            self.events.push(FeedEvent::Unreachable);
        }
        self.unreachable = down;
    }

    /// Adds, changes and removes feed relays to match the profiles in force
    /// of the subscribed feeds (NETWORK 5).
    fn update_feed_relays(&mut self) {
        let own: BTreeSet<String> = self.relay_urls().into_iter().collect();
        let mut wanted: BTreeMap<String, BTreeSet<FeedKey>> = BTreeMap::new();
        // Feeds whose relays come from hints, for `hinted`.
        let mut by_hint: BTreeMap<String, BTreeSet<FeedKey>> = BTreeMap::new();
        let feeds: Vec<FeedKey> = self.subscriptions.feeds().copied().collect();
        // With no own relay at all, nothing could hold a hinted feed.
        if self.links.values().all(|l| l.feeds.is_some()) {
            for feed in &feeds {
                if self.subscriptions.hints(feed).is_some() {
                    self.hints_wanted.insert(*feed);
                }
            }
        }
        if self.feed_relays {
            for feed in &feeds {
                let named = self.profile(feed).and_then(|(_, d)| d).map(|d| d.relays).unwrap_or_default();
                // NETWORK 4.9: the profile's relay lines supersede the hints.
                if !named.is_empty() {
                    named
                        .iter()
                        .filter(|u| self.feed_relays_unrestricted || profile::usable_profile_relay(u))
                        .filter(|u| !own.contains(*u))
                        .take(FEED_RELAYS_PER_FEED)
                        .for_each(|u| {
                            wanted.entry(u.clone()).or_default().insert(*feed);
                        });
                    continue;
                }
                if !self.hints_wanted.contains(feed) {
                    continue;
                }
                let Some(list) = self.subscriptions.hints(feed) else { continue };
                crate::address::parse_hints(list.as_bytes())
                    .unwrap_or_default()
                    .iter()
                    .filter(|h| !own.iter().any(|u| crate::address::url_is_hint(u, h)))
                    .map(crate::address::Hint::url)
                    .filter(|u| self.feed_relays_unrestricted || profile::usable_profile_relay(u))
                    .take(FEED_RELAYS_PER_FEED)
                    .for_each(|u| {
                        wanted.entry(u.clone()).or_default().insert(*feed);
                        by_hint.entry(u).or_default().insert(*feed);
                    });
            }
        }
        let room = MAX_LINKS.saturating_sub(own.len());
        let wanted: BTreeMap<String, BTreeSet<FeedKey>> = wanted.into_iter().take(room).collect();
        self.hinted = by_hint.iter().filter(|(u, _)| wanted.contains_key(*u)).flat_map(|(_, f)| f.iter().copied()).collect();
        self.hint_relays = by_hint.into_iter().filter(|(u, _)| wanted.contains_key(u)).collect();
        // Remove or update the feed relays there are.
        let current: Vec<(u64, String)> =
            self.links.iter().filter(|(_, l)| l.feeds.is_some()).map(|(&id, l)| (id, l.url.clone())).collect();
        for (lid, url) in current {
            match wanted.get(&url) {
                None => self.remove_link(lid),
                Some(feeds) => {
                    let link = self.links.get_mut(&lid).unwrap();
                    let old = link.feeds.replace(feeds.clone()).unwrap_or_default();
                    if link.ready() {
                        for feed in feeds.difference(&old) {
                            let after = self.feeds.highest(feed).unwrap_or(0);
                            let req = link.client.subscribe(feed, after);
                            self.waiting.insert((lid, req), Waiting::Subscribe(*feed, after));
                        }
                        for feed in old.difference(feeds) {
                            link.catching_up.remove(feed);
                            let req = link.client.unsubscribe(feed);
                            self.waiting.insert((lid, req), Waiting::Unsubscribe);
                        }
                    }
                }
            }
        }
        let have: BTreeSet<String> = self.links.values().map(|l| l.url.clone()).collect();
        for (url, feeds) in wanted.into_iter().filter(|(u, _)| !have.contains(u)) {
            let client = (self.connect_feed_relay)(&url);
            self.add_link(Link::new(url, client, Some(feeds)));
        }
    }

    /// Drops a relay; its requests fail.
    fn remove_link(&mut self, lid: u64) {
        let Some(mut link) = self.links.remove(&lid) else { return };
        link.client.close();
        let mut failed: Vec<u32> = link.client.poll().into_iter().map(|(req, _)| req).collect();
        failed.extend(self.waiting.keys().filter(|(l, _)| *l == lid).map(|&(_, req)| req));
        for req in failed {
            if let Some(waiting) = self.waiting.remove(&(lid, req)) {
                self.reply(lid, waiting, RelayReply::Failed);
            }
        }
    }

    /// Verifies and stores an entry pushed by a relay, and starts its
    /// automatic download (NETWORK 4.4).
    fn receive(&mut self, lid: u64, bytes: &[u8]) {
        // Caught up when the relay's last entry for the feed arrives, whether
        // or not it turns out to be valid (NETWORK 4.3).
        let mut caught_up = None;
        if bytes.len() >= 36
            && let Some(link) = self.links.get_mut(&lid)
        {
            let feed: FeedKey = bytes[..32].try_into().unwrap();
            let seq = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
            if link.catching_up.get(&feed).is_some_and(|&highest| seq >= highest) {
                link.catching_up.remove(&feed);
                caught_up = Some(feed);
            }
        }
        self.receive_entry(bytes);
        // NETWORK 4.4: programs are downloaded once the feed is caught up, so
        // that only current versions are.
        if let Some(feed) = caught_up {
            self.program_downloads(&feed);
        }
    }

    /// Whether some connection is still catching up with `feed`.
    fn catching_up(&self, feed: &FeedKey) -> bool {
        self.links.values().any(|l| l.catching_up.contains_key(feed))
    }

    /// Starts the automatic download of the current version of each
    /// program of `feed` whose image the store does not hold and that is
    /// not being fetched, up to the largest image (NETWORK 4.4); not while
    /// a connection is catching up with the feed.
    fn program_downloads(&mut self, feed: &FeedKey) {
        if !self.program_download || !self.subscriptions.contains(feed) || self.catching_up(feed) {
            return;
        }
        let fetching: BTreeSet<BlobId> =
            self.ops.values().filter_map(|o| if let Op::Get { id, .. } = o { Some(*id) } else { None }).collect();
        for s in self.current_programs(feed) {
            let e = s.entry;
            if e.size > MAX_IMAGE || fetching.contains(&e.blob) || self.held_article(&e).is_some() {
                continue;
            }
            if self.connected_links().is_empty() {
                return;
            }
            self.fetch_from_relays(GetFor::Program(e.clone()), e.blob);
        }
    }

    fn receive_entry(&mut self, bytes: &[u8]) {
        // Invalid entries, entries for feeds not subscribed and entries
        // already received (from this relay or another) are discarded.
        let Ok(signed) = entry::decode(bytes) else { return };
        if !self.subscriptions.contains(&signed.entry.feed) || self.insert(&signed).ok() != Some(Insert::New) {
            return;
        }
        let entry = signed.entry;
        if entry.kind == KIND_PROGRAM {
            let (feed, article) = (entry.feed, self.held_article(&entry).unwrap_or(Article::NotFetched));
            self.events.push(FeedEvent::Entry { entry, article });
            return self.program_downloads(&feed);
        }
        let article = match self.held_article(&entry) {
            Some(held) => held,
            // A kind this revision does not define is kept but its blob is
            // not downloaded: nothing here could open it (NETWORK 4.4).
            None if !entry::known_kind(entry.kind) => Article::NotFetched,
            None if entry.size <= self.auto_limit => {
                if self.connected_links().is_empty() {
                    Article::Failed
                } else {
                    return self.fetch_from_relays(GetFor::Auto(entry.clone()), entry.blob);
                }
            }
            None => Article::NotFetched,
        };
        self.events.push(FeedEvent::Entry { entry, article });
    }

    /// The result of an automatic download (`None`: not found anywhere).
    fn downloaded(&mut self, entry: Entry, bytes: Option<Vec<u8>>) {
        let article = match bytes {
            Some(bytes) if bytes.len() != entry.size as usize => Article::SizeMismatch,
            Some(bytes) if self.store.put(&bytes).is_ok() => {
                if entry.kind == KIND_PROFILE {
                    self.feed_relays_dirty = true;
                }
                if entry.kind == KIND_ARTICLE && self.prefetch && self.subscriptions.contains(&entry.feed) {
                    self.prefetch_links(&bytes);
                }
                Article::Local
            }
            _ => Article::Failed,
        };
        self.events.push(FeedEvent::Entry { entry, article });
    }

    /// Fetches the first [`PREFETCH_LINKS`] distinct `terse:` article links
    /// of `text` whose blobs are not held or being fetched, except those an
    /// entry shows to be larger than the automatic download limit
    /// (NETWORK 4.4).
    fn prefetch_links(&mut self, text: &[u8]) {
        if !self.blob_sizes_loaded {
            self.blob_sizes_loaded = true;
            let feeds: Vec<FeedKey> = self.subscriptions.feeds().copied().collect();
            for feed in feeds {
                for s in self.feeds.entries(&feed).unwrap_or_default() {
                    self.blob_sizes.entry(s.entry.blob).or_insert(s.entry.size);
                    if s.entry.kind == KIND_PROGRAM {
                        self.program_blobs.insert(s.entry.blob);
                    }
                }
            }
        }
        let fetching: BTreeSet<BlobId> =
            self.ops.values().filter_map(|o| if let Op::Get { id, .. } = o { Some(*id) } else { None }).collect();
        let mut chosen: Vec<BlobId> = Vec::new();
        for id in article::terse_links(text) {
            if chosen.len() == PREFETCH_LINKS {
                break;
            }
            if chosen.contains(&id) || fetching.contains(&id) || self.store.path(&id).exists() {
                continue;
            }
            chosen.push(id);
        }
        for id in chosen {
            // An image a program entry names is not prefetched (NETWORK 4.4).
            if self.blob_sizes.get(&id).is_none_or(|&size| size <= self.auto_limit) && !self.program_blobs.contains(&id) {
                self.fetch_from_relays(GetFor::Prefetch, id);
            }
        }
    }

    fn reply(&mut self, lid: u64, waiting: Waiting, reply: RelayReply) {
        match (waiting, reply) {
            (Waiting::Op(op), reply) => self.op_reply(lid, op, reply),
            (Waiting::Subscribe(feed, after), RelayReply::Subscribed { highest }) => {
                if highest > after
                    && self.subscriptions.contains(&feed)
                    && let Some(link) = self.links.get_mut(&lid)
                {
                    link.catching_up.insert(feed, highest);
                }
                // NETWORK 4.9: what the own relays hold decides on hints.
                if self.subscriptions.contains(&feed) && self.links.get(&lid).is_some_and(|l| l.feeds.is_none()) {
                    if highest > 0 {
                        self.empty_on_own.remove(&feed);
                        if self.held_on_own.insert(feed) && self.hints_wanted.remove(&feed) {
                            self.feed_relays_dirty = true;
                        }
                    } else if !self.held_on_own.contains(&feed) {
                        self.empty_on_own.entry(feed).or_default().insert(lid);
                        self.check_hints(&feed);
                    }
                }
                self.events.push(FeedEvent::Subscribed(feed));
                self.program_downloads(&feed);
            }
            (Waiting::Subscribe(feed, _), RelayReply::Refused(_)) => {
                // NETWORK 4.9: an own relay that refuses the feed holds none
                // of it for this client.
                if self.subscriptions.contains(&feed)
                    && self.links.get(&lid).is_some_and(|l| l.feeds.is_none())
                    && !self.held_on_own.contains(&feed)
                {
                    self.empty_on_own.entry(feed).or_default().insert(lid);
                    self.check_hints(&feed);
                }
                self.events.push(FeedEvent::SubscribeRefused(feed));
            }
            // No answer: subscribe again on the next poll.
            (Waiting::Subscribe(..), _) => {
                if let Some(link) = self.links.get_mut(&lid) {
                    link.generation = 0;
                }
            }
            (Waiting::Unsubscribe, _) => {}
        }
    }

    fn op_reply(&mut self, lid: u64, op_id: u64, reply: RelayReply) {
        let Some(op) = self.ops.get_mut(&op_id) else { return };
        match op {
            Op::Get { id, in_flight, not_found, .. } => {
                *in_flight -= 1;
                let id = *id;
                match reply {
                    // NETWORK 2: verify before storing or delivering; a blob
                    // that fails the check counts as not found on this relay.
                    RelayReply::Got(bytes) if valid_size(bytes.len()) && blob_id(&bytes) == id => {
                        let Some(Op::Get { purpose, .. }) = self.ops.remove(&op_id) else { unreachable!() };
                        match purpose {
                            GetFor::Token(token) => {
                                // A full local store does not stop delivery.
                                let _ = self.store.put(&bytes);
                                self.ready.push((token, Done::Got(bytes)));
                            }
                            GetFor::Auto(entry) => self.downloaded(entry, Some(bytes)),
                            GetFor::Program(entry) => {
                                let article = if bytes.len() != entry.size as usize {
                                    Article::SizeMismatch
                                } else if self.store.put(&bytes).is_ok() {
                                    Article::Local
                                } else {
                                    Article::Failed
                                };
                                self.events.push(FeedEvent::ProgramDownloaded { entry, article });
                            }
                            // Not scanned; discarded if over the limit.
                            GetFor::Prefetch => {
                                if bytes.len() <= self.auto_limit as usize {
                                    let _ = self.store.put(&bytes);
                                }
                            }
                        }
                        return;
                    }
                    RelayReply::Got(_) | RelayReply::NotFound => *not_found = true,
                    _ => {}
                }
                self.advance_get(op_id);
            }
            Op::Put { token, id, in_flight, reported, outcomes } => {
                *in_flight -= 1;
                let accepted = reply == RelayReply::Put(*id);
                outcomes.push((lid, Delivery::of(accepted, &reply)));
                if accepted && !*reported {
                    *reported = true;
                    self.ready.push((*token, Done::Put(*id)));
                }
                if *in_flight == 0 {
                    let Some(Op::Put { token, reported, outcomes, .. }) = self.ops.remove(&op_id) else { unreachable!() };
                    if !reported {
                        self.ready.push((token, Done::Failed));
                    }
                    self.delivered(outcomes);
                }
            }
            Op::PublishBlob { token, entry, in_flight, published, outcomes } => {
                *in_flight -= 1;
                let accepted = reply == RelayReply::Put(entry.blob);
                outcomes.push((lid, Delivery::of(accepted, &reply)));
                if let RelayReply::Refused(why) = reply {
                    self.blob_refusals.entry(*token).or_default().push((lid, why));
                }
                let (token, publish) = (*token, accepted && !*published);
                let entry = publish.then(|| entry.clone());
                *published |= publish;
                if let Some(Op::PublishBlob { in_flight: 0, .. }) = self.ops.get(&op_id) {
                    let Some(Op::PublishBlob { published, outcomes, .. }) = self.ops.remove(&op_id) else { unreachable!() };
                    if !published {
                        // No relay took the blob: what each said is the
                        // delivery of this publish.
                        self.blob_refusals.remove(&token);
                        self.ready.push((token, Done::Failed));
                        self.delivered(outcomes);
                    }
                }
                if let Some(entry) = entry {
                    self.publish_entry(token, entry);
                }
            }
            Op::Entry { token, signed, in_flight, refused, reported, outcomes } => {
                *in_flight -= 1;
                *refused |= matches!(reply, RelayReply::Refused(_));
                let accepted = reply == RelayReply::Done;
                outcomes.push((lid, Delivery::of(accepted, &reply)));
                if accepted && !*reported {
                    *reported = true;
                    let (token, signed) = (*token, signed.clone());
                    self.store_own_entry(token, &signed);
                }
                if let Some(Op::Entry { in_flight: 0, .. }) = self.ops.get(&op_id) {
                    let Some(Op::Entry { token, refused, reported, mut outcomes, .. }) = self.ops.remove(&op_id) else {
                        unreachable!()
                    };
                    if !reported {
                        self.ready.push((token, if refused { Done::Refused } else { Done::Failed }));
                    }
                    // A relay that took the entry but not its blob.
                    for (lid, why) in self.blob_refusals.remove(&token).unwrap_or_default() {
                        if let Some(d) = outcomes.iter_mut().find(|(l, _)| *l == lid).map(|(_, d)| d)
                            && *d == Delivery::Accepted
                        {
                            *d = Delivery::BlobRefused(why);
                        }
                    }
                    self.delivered(outcomes);
                }
            }
            Op::Lookup { feed, name, in_flight, refused, not_found, best, .. } => {
                *in_flight -= 1;
                match reply {
                    // Verified: the feed, kind and name asked for.
                    RelayReply::LookedUp(bytes) => match entry::decode(&bytes) {
                        Ok(s) if s.entry.feed == *feed && s.entry.kind == KIND_PROGRAM && s.entry.title.as_bytes() == &name[..] => {
                            if best.as_ref().is_none_or(|b| b.entry.seq < s.entry.seq) {
                                *best = Some(s);
                            }
                        }
                        _ => {}
                    },
                    RelayReply::NotFound => *not_found = true,
                    RelayReply::Refused(_) => *refused = true,
                    _ => {}
                }
                if *in_flight == 0 {
                    let Some(Op::Lookup { token, best, refused, not_found, .. }) = self.ops.remove(&op_id) else {
                        unreachable!()
                    };
                    let done = match best {
                        // The first received of a seq wins (NETWORK 4.2):
                        // a different one already held is the one used.
                        Some(s) => match self.insert(&s) {
                            Ok(Insert::New | Insert::Duplicate) => Done::Found(vec![s]),
                            Ok(Insert::Conflict) => match self.current_program(&s.entry.feed, s.entry.title.as_bytes()) {
                                Some(_) => Done::Found(Vec::new()),
                                None => Done::NotFound,
                            },
                            Err(_) => Done::Failed,
                        },
                        None if not_found => Done::NotFound,
                        None if refused => Done::Refused,
                        None => Done::Failed,
                    };
                    self.ready.push((token, done));
                }
            }
            Op::Find { scope, query, in_flight, refused, found, .. } => {
                *in_flight -= 1;
                *refused |= matches!(reply, RelayReply::Refused(_));
                if let RelayReply::Found(entries) = reply {
                    let kinds: &[u8] = match *scope {
                        SCOPE_FEEDS => &[KIND_PROFILE],
                        SCOPE_ENTRIES => &[KIND_ARTICLE, KIND_DOCUMENT],
                        _ => &[KIND_PROGRAM],
                    };
                    // NETWORK 4.7: every result is verified; forged ones and
                    // ones that do not answer the query are dropped.
                    found.entry(lid).or_default().extend(
                        entries
                            .iter()
                            .filter_map(|b| entry::decode(b).ok())
                            .filter(|s| kinds.contains(&s.entry.kind) && profile::matches(s.entry.title.as_bytes(), query)),
                    );
                }
                if *in_flight == 0 {
                    let Some(Op::Find { token, scope, query, refused, found, .. }) = self.ops.remove(&op_id) else {
                        unreachable!()
                    };
                    let done = if !found.is_empty() {
                        Done::Found(merge_found(scope, &query, found.into_values().collect()))
                    } else if refused {
                        Done::Refused
                    } else {
                        Done::Failed
                    };
                    self.ready.push((token, done));
                }
            }
        }
    }

    /// Reports `Settled` when nothing is outstanding for the feeds.
    fn check_settled(&mut self) {
        let pending = self.settle_pending();
        let current = self.connections_current();
        if pending {
            if std::mem::take(&mut self.settled) {
                self.events.push(FeedEvent::Busy);
            }
        } else if current && !self.settled && !self.subscriptions.is_empty() {
            self.settled = true;
            self.events.push(FeedEvent::Settled);
        }
    }

    /// Returns finished operations and keeps subscriptions going; waits at
    /// most briefly. Feed events are collected for `take_feed_events`.
    pub fn poll(&mut self) -> Vec<(Token, Done)> {
        self.maintain();
        let ids: Vec<u64> = self.links.keys().copied().collect();
        for lid in ids {
            let Some(link) = self.links.get_mut(&lid) else { continue };
            let (replies, pushed) = (link.client.poll(), link.client.take_pushed());
            for (req, reply) in replies {
                if let Some(waiting) = self.waiting.remove(&(lid, req)) {
                    self.reply(lid, waiting, reply);
                }
            }
            for bytes in pushed {
                self.receive(lid, &bytes);
            }
        }
        let graced = self.deferred_since.is_some_and(|t| t.elapsed() >= CONNECT_GRACE) && !self.connected_links().is_empty();
        if !self.deferred.is_empty() && (!self.connecting() || graced) {
            self.deferred_since = None;
            for start in std::mem::take(&mut self.deferred) {
                start(self);
            }
        }
        self.check_settled();
        std::mem::take(&mut self.ready)
    }

    /// The operations that finished without the network (from the store)
    /// since the last call or [`Self::poll`], without polling the relays.
    pub fn take_ready(&mut self) -> Vec<(Token, Done)> {
        std::mem::take(&mut self.ready)
    }

    fn wait_for(&mut self, token: Token) -> Done {
        loop {
            if let Some((_, done)) = self.poll().into_iter().find(|(t, _)| *t == token) {
                return done;
            }
        }
    }

    /// Records what each relay did with a put or entry once all have
    /// answered: those sent to, then the others as not connected.
    fn delivered(&mut self, outcomes: Vec<(u64, Delivery)>) {
        let mut delivery: Vec<(String, Delivery)> =
            outcomes.iter().filter_map(|(lid, d)| Some((self.links.get(lid)?.url.clone(), *d))).collect();
        for (lid, link) in &self.links {
            if !outcomes.iter().any(|(l, _)| l == lid) {
                delivery.push((link.url.clone(), Delivery::NotConnected));
            }
        }
        self.delivery = delivery;
    }

    /// What each relay did with the last put or entry sent to the relays,
    /// once every relay it was sent to has answered (or failed): for a
    /// blocking publish, which waits for that, the relays that took it and
    /// those that did not (NETWORK 5.1: a publish goes to every connected
    /// relay and succeeds when one accepts). Empty without relays.
    pub fn take_delivery(&mut self) -> Vec<(String, Delivery)> {
        std::mem::take(&mut self.delivery)
    }

    /// Makes the blocking puts and publishes deliver to every relay: for a
    /// program that exits after one (the `terse` commands). They then wait
    /// until no relay is still being connected, and after the first
    /// acceptance until every relay sent to has answered or failed (at
    /// most the relay timeout), so that what is sent goes to every relay
    /// that can be reached and none is cut off by the exit; see
    /// [`Self::take_delivery`]. Off by default: a host that keeps running
    /// is not held up by a relay that does not answer (NETWORK 5).
    pub fn set_deliver_to_all(&mut self, on: bool) {
        self.deliver_to_all = on;
    }

    /// With [`Self::set_deliver_to_all`], before a blocking call sends:
    /// waits until no relay is still being connected (each is connected,
    /// or has failed to connect).
    fn connect_all(&mut self) {
        if !self.deliver_to_all {
            return;
        }
        loop {
            let done = self.poll();
            self.ready.extend(done);
            if !self.connecting() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// `wait_for`; then, with [`Self::set_deliver_to_all`], waits until
    /// every relay the put or entry went to has answered (or failed).
    fn wait_for_all(&mut self, token: Token) -> Done {
        let done = self.wait_for(token);
        while self.deliver_to_all
            && self.ops.values().any(|op| matches!(op, Op::Put { .. } | Op::PublishBlob { .. } | Op::Entry { .. })) {
            let more = self.poll();
            self.ready.extend(more);
        }
        done
    }

    /// Fetches a whole blob, blocking.
    pub fn fetch(&mut self, id: BlobId) -> Done {
        self.start_get(0, id);
        self.wait_for(0)
    }

    /// `fetch`, with failures as messages.
    pub fn fetch_bytes(&mut self, id: BlobId) -> Result<Vec<u8>, String> {
        match self.fetch(id) {
            Done::Got(bytes) => Ok(bytes),
            Done::NotFound => Err(format!("blob {} not found", to_hex(&id))),
            _ => Err(format!("fetching blob {} failed", to_hex(&id))),
        }
    }

    /// Publishes a blob, blocking, to every relay that can be reached; see
    /// [`Self::take_delivery`].
    pub fn publish(&mut self, data: &[u8]) -> Done {
        self.connect_all();
        self.start_put(0, data);
        self.wait_for_all(0)
    }

    /// Publishes an article, blocking, to every relay that can be reached;
    /// see [`Self::take_delivery`].
    pub fn publish_article(&mut self, draft: &Draft) -> Done {
        self.connect_all();
        self.start_publish_article(0, draft);
        self.wait_for_all(0)
    }

    /// Publishes a profile, blocking, to every relay that can be reached
    /// (see [`Self::take_delivery`]); `description` `None` keeps that of
    /// the profile in force.
    pub fn publish_profile(&mut self, name: &str, listed: bool, description: Option<String>, time: u32) -> Done {
        self.connect_all();
        match description {
            None => self.start_publish_profile(0, name, listed, time),
            Some(d) => {
                let d = self.ensure_identity().ok().map(|_| d);
                self.start_publish_profile_with(0, name, listed, d, time);
            }
        }
        self.wait_for_all(0)
    }

    /// Publishes a document (kind 4), blocking, like `publish_article`.
    pub fn publish_document(&mut self, data: &[u8], title: &str, time: u32) -> Done {
        self.connect_all();
        self.start_publish_document(0, data, title, time);
        self.wait_for_all(0)
    }

    /// Publishes a program, blocking, like `publish_article`.
    pub fn publish_program(&mut self, image: &[u8], name: &str, time: u32) -> Done {
        self.connect_all();
        self.start_publish_program(0, image, name, time);
        self.wait_for_all(0)
    }

    /// Searches, blocking.
    pub fn find(&mut self, scope: u8, query: &[u8]) -> Done {
        self.start_find(0, scope, query);
        self.wait_for(0)
    }

    /// Lets every relay be tried again at once (tests).
    #[cfg(test)]
    fn skip_backoff(&mut self) {
        for link in self.links.values_mut() {
            link.retry_at = Some(Instant::now());
        }
    }
}

/// Merges the results of several relays, one list per relay in relay
/// order (NETWORK 4.7): one entry per feed and `seq`, at most 16. Feeds:
/// only the highest `seq` of each feed, names starting with the query
/// first, then by feed key. Entries: by rank, the first result of each
/// relay, then the second of each, and so on.
pub fn merge_found(scope: u8, query: &[u8], found: Vec<Vec<SignedEntry>>) -> Vec<SignedEntry> {
    let mut list: Vec<SignedEntry> = if scope == SCOPE_FEEDS {
        let mut latest: BTreeMap<FeedKey, SignedEntry> = BTreeMap::new();
        for s in found.into_iter().flatten() {
            if latest.get(&s.entry.feed).is_none_or(|l| l.entry.seq < s.entry.seq) {
                latest.insert(s.entry.feed, s);
            }
        }
        let mut list: Vec<SignedEntry> = latest.into_values().collect();
        list.sort_by_key(|s| (!profile::starts_with(s.entry.title.as_bytes(), query), s.entry.feed));
        list
    } else {
        let mut seen: BTreeSet<(FeedKey, u32)> = BTreeSet::new();
        let mut list = Vec::new();
        let longest = found.iter().map(Vec::len).max().unwrap_or(0);
        for rank in 0..longest {
            for s in found.iter().filter_map(|l| l.get(rank)) {
                if seen.insert((s.entry.feed, s.entry.seq)) {
                    list.push(s.clone());
                }
            }
        }
        if scope == SCOPE_PROGRAMS {
            // Current versions only: a program is dropped if another result
            // has the same feed and name and a higher seq.
            let highest: BTreeMap<(FeedKey, String), u32> = list.iter().fold(BTreeMap::new(), |mut m, s| {
                let h = m.entry((s.entry.feed, s.entry.title.clone())).or_insert(0);
                *h = (*h).max(s.entry.seq);
                m
            });
            list.retain(|s| highest[&(s.entry.feed, s.entry.title.clone())] == s.entry.seq);
        }
        list
    };
    list.truncate(MAX_FIND_RESULTS);
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{self, Request, status};
    use crate::relay::{DEFAULT_TIMEOUT, Transport};
    use std::sync::{Arc, Mutex};

    #[test]
    fn without_a_relay() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let id = blob_id(b"blob");
        assert_eq!(node.fetch(id), Done::NotFound);
        assert_eq!(node.publish(b"blob"), Done::Put(id));
        assert_eq!(node.fetch(id), Done::Got(b"blob".to_vec()));
        assert_eq!(node.publish(b""), Done::Failed);
        // A damaged local copy is not delivered.
        std::fs::write(node.store().path(&id), b"blub").unwrap();
        assert_eq!(node.fetch(id), Done::NotFound);
        assert!(!node.store().path(&id).exists());
        assert_eq!(node.find(SCOPE_FEEDS, b"x"), Done::Failed, "no relay to ask");
    }

    #[test]
    fn publishing_without_a_relay_numbers_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let draft = |title: &str| Draft { data: title.as_bytes().to_vec(), title: title.into(), reply_to: None, time: 5 };
        assert_eq!(node.publish_article(&draft("a")), Done::Failed, "no identity");
        let id = Identity::from_seed([1; 32]);
        node.set_identity(id.clone());
        assert_eq!(node.publish_article(&draft("a")), Done::Published { seq: 1, blob: blob_id(b"a") });
        assert_eq!(node.publish_article(&draft("b")), Done::Published { seq: 2, blob: blob_id(b"b") });
        assert_eq!(node.publish_article(&draft(&"x".repeat(121))), Done::Failed);
        let held = node.feed_store().entries(&id.feed_key()).unwrap();
        assert_eq!(held.iter().map(|e| e.entry.title.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    /// Profiles: the latest wins; the description is kept; the timeline
    /// has only articles; names records.
    #[test]
    fn profiles_and_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let me = Identity::from_seed([1; 32]);
        node.set_identity(me.clone());
        let key = me.feed_key();
        assert_eq!(node.publish_profile(" bad", true, None, 1), Done::Failed);
        assert!(matches!(node.publish_profile("Ana", false, Some("About me.".into()), 1), Done::Published { seq: 1, .. }));
        let draft = Draft { data: b"text".to_vec(), title: "Post".into(), reply_to: None, time: 2 };
        assert!(matches!(node.publish_article(&draft), Done::Published { seq: 2, .. }));
        assert!(matches!(node.publish_profile("Ana B", true, None, 3), Done::Published { seq: 3, .. }));
        let (entry, doc) = node.profile(&key).unwrap();
        assert_eq!((entry.title.as_str(), entry.seq, entry.kind), ("Ana B", 3, KIND_PROFILE));
        assert_eq!(doc.unwrap(), Document { listed: true, relays: vec![], description: "About me.".into() });
        node.set_subscriptions(Subscriptions::in_memory([key]));
        assert_eq!(node.timeline(), [(key, 2)], "profile entries are not in the timeline");
        assert_eq!(node.display_name(&key), Some(("Ana B".into(), false)));

        let other = Identity::from_seed([2; 32]).feed_key();
        node.set_subscriptions(Subscriptions::in_memory([key, other]));
        node.set_nickname(other, Some("Bo")).unwrap();
        let (total, data) = node.name_records(0, 1000);
        let rec = |k: &FeedKey, flags: u8, name: &str| [&[33 + name.len() as u8, flags][..], k, name.as_bytes()].concat();
        let mut want = [(key, rec(&key, 2, "Ana B")), (other, rec(&other, 1, "Bo"))];
        want.sort_by_key(|w| w.0);
        assert_eq!((total, data), (2, [want[0].1.clone(), want[1].1.clone()].concat()));
        node.set_nickname(key, Some("Me")).unwrap();
        node.set_nickname(other, None).unwrap();
        let (_, data) = node.name_records(0, 1000);
        assert!(data.windows(rec(&key, 3, "Me").len()).any(|w| w == rec(&key, 3, "Me")));
        assert!(data.windows(rec(&other, 0, "").len()).any(|w| w == rec(&other, 0, "")));
        assert_eq!(node.name_records(0, 33), (2, Vec::new()), "the shortest record is 34 bytes");
        // A feed that is not subscribed is listed for its nickname, flagged.
        let third = Identity::from_seed([3; 32]).feed_key();
        node.set_nickname(third, Some("Cy")).unwrap();
        let (total, data) = node.name_records(0, 1000);
        assert_eq!(total, 3);
        assert!(data.windows(rec(&third, 5, "Cy").len()).any(|w| w == rec(&third, 5, "Cy")));
        node.set_subscriptions(Subscriptions::in_memory([key, other, third]));
        assert!(node.name_records(0, 1000).1.windows(rec(&third, 1, "Cy").len()).any(|w| w == rec(&third, 1, "Cy")));
    }

    /// A scripted relay: answers requests from in-memory state.
    #[derive(Default)]
    struct Script {
        blobs: HashMap<BlobId, Vec<u8>>,
        /// Entries sent after a subscribe for their feed with seq > after,
        /// whatever their validity.
        held: Vec<Vec<u8>>,
        subscribes: Vec<(FeedKey, u32)>,
        connections: usize,
        /// Messages to deliver on the next receive.
        live: Vec<Vec<u8>>,
        /// Drop the connection on the next receive.
        drop: bool,
        /// Refuse connections.
        down: bool,
        /// Report this `highest` in subscribe replies instead of the truth.
        claim_highest: Option<u32>,
        /// Hold back messages after the subscribe reply until cleared.
        stall: bool,
        /// Entries a find returns, whatever the query.
        find: Vec<Vec<u8>>,
        /// Refuse finds.
        find_refused: bool,
        gets: usize,
        /// The blobs asked for, in order.
        asked: Vec<BlobId>,
        puts: usize,
        /// Entry every lookup returns, if set.
        lookup_reply: Option<Vec<u8>>,
    }

    struct Fake {
        script: Arc<Mutex<Script>>,
        inbox: VecDeque<Vec<u8>>,
    }

    impl Transport for Fake {
        fn send(&mut self, message: Vec<u8>) -> std::io::Result<()> {
            let mut s = self.script.lock().unwrap();
            let reply = match protocol::decode_request(&message).unwrap() {
                Request::Get { req, id } => {
                    s.gets += 1;
                    s.asked.push(id);
                    match s.blobs.get(&id) {
                        Some(b) => protocol::encode_get_reply(req, status::DONE, b),
                        None => protocol::encode_get_reply(req, status::NOT_FOUND, &[]),
                    }
                }
                Request::Put { req, data } => {
                    s.puts += 1;
                    s.blobs.insert(blob_id(data), data.to_vec());
                    protocol::encode_put_reply(req, status::DONE, &blob_id(data))
                }
                Request::Publish { req, entry } => {
                    s.held.push(entry.to_vec());
                    protocol::encode_status_reply(protocol::PUBLISH_REPLY, req, status::DONE)
                }
                Request::Subscribe { req, feed, after } => {
                    s.subscribes.push((feed, after));
                    let seq = |e: &Vec<u8>| u32::from_le_bytes(e[32..36].try_into().unwrap());
                    let highest = s.held.iter().filter(|e| e[..32] == feed).map(seq).max().unwrap_or(0);
                    let highest = s.claim_highest.unwrap_or(highest);
                    self.inbox.push_back(protocol::encode_subscribe_reply(req, status::DONE, highest));
                    for e in s.held.iter().filter(|e| e[..32] == feed && seq(e) > after) {
                        self.inbox.push_back(protocol::encode_entry(e));
                    }
                    return Ok(());
                }
                Request::Unsubscribe { req, .. } => {
                    protocol::encode_status_reply(protocol::UNSUBSCRIBE_REPLY, req, status::DONE)
                }
                Request::Find { req, .. } if s.find_refused => {
                    protocol::encode_find_reply::<&[u8]>(req, status::REFUSED, &[])
                }
                Request::Find { req, .. } => protocol::encode_find_reply(req, status::DONE, &s.find),
                Request::Peer => return Ok(()),
                // The held program entry of that feed and name with the
                // highest seq, or the forged reply if one is scripted.
                Request::Lookup { req, feed, name, .. } => {
                    let hit = s.lookup_reply.clone().or_else(|| {
                        let seq = |e: &Vec<u8>| u32::from_le_bytes(e[32..36].try_into().unwrap());
                        s.held
                            .iter()
                            .filter(|e| e[..32] == feed && e[40] == KIND_PROGRAM && e.get(109..109 + e[108] as usize) == Some(name))
                            .max_by_key(|e| seq(e))
                            .cloned()
                    });
                    match hit {
                        Some(e) => protocol::encode_lookup_reply(req, status::DONE, &e),
                        None => protocol::encode_lookup_reply(req, status::NOT_FOUND, &[]),
                    }
                }
            };
            self.inbox.push_back(reply);
            Ok(())
        }

        fn try_recv(&mut self) -> std::io::Result<Option<Vec<u8>>> {
            let mut s = self.script.lock().unwrap();
            if std::mem::take(&mut s.drop) {
                return Err(std::io::ErrorKind::ConnectionReset.into());
            }
            self.inbox.extend(s.live.drain(..));
            if s.stall && self.inbox.front().is_some_and(|m| m[0] == protocol::ENTRY) {
                return Ok(None);
            }
            Ok(self.inbox.pop_front())
        }
    }

    fn fake_relay() -> (RelayClient, Arc<Mutex<Script>>) {
        let script = Arc::new(Mutex::new(Script::default()));
        let s = script.clone();
        let connect = Box::new(move || {
            let mut locked = s.lock().unwrap();
            if locked.down {
                return Err(std::io::ErrorKind::ConnectionRefused.into());
            }
            locked.connections += 1;
            Ok(Box::new(Fake { script: s.clone(), inbox: VecDeque::new() }) as Box<dyn Transport>)
        });
        (RelayClient::new(connect, DEFAULT_TIMEOUT), script)
    }

    fn node_with_fake(dir: &std::path::Path) -> (Node, Arc<Mutex<Script>>) {
        let (relay, script) = fake_relay();
        (Node::new(BlobStore::open(dir, None).unwrap(), Some(relay)), script)
    }

    /// NETWORK 2, spec/content.md §content.submit: a get for a blob the node
    /// holds is answered from the store at once, whatever the relays do. A
    /// connected relay is not asked (no Get is sent), and a relay that is
    /// still connecting, one that accepts the TCP connection and never
    /// answers the WebSocket handshake, does not hold it up: the first poll
    /// after the get has the blob. (A get for a blob not held does wait
    /// for that relay, NETWORK 5.1.)
    #[test]
    fn a_held_blob_never_waits_on_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, script) = node_with_fake(dir.path());
        let id = node.store().put(b"held").unwrap();
        node.poll();
        assert!(node.connected());
        node.start_get(1, id);
        assert_eq!(node.poll(), [(1, Done::Got(b"held".to_vec()))]);
        assert_eq!(script.lock().unwrap().gets, 0, "a held blob is not asked of the relay");

        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}", silent.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        node.add_relay(&url, RelayClient::websocket(&url, DEFAULT_TIMEOUT));
        let id = node.store().put(b"held").unwrap();
        node.poll();
        assert!(node.connecting(), "the handshake is never answered");
        node.start_get(2, id);
        assert_eq!(node.poll(), [(2, Done::Got(b"held".to_vec()))]);
        node.start_get(3, blob_id(b"not held"));
        assert_eq!(node.poll(), [], "a blob not held waits for the relay being connected");
        assert!(node.connecting());
    }

    /// Polls until `Settled` and returns the events.
    fn settle(node: &mut Node) -> Vec<FeedEvent> {
        let mut events = Vec::new();
        for _ in 0..100 {
            node.poll();
            events.extend(node.take_feed_events());
            if events.last() == Some(&FeedEvent::Settled) {
                return events;
            }
        }
        panic!("not settled: {events:?}");
    }

    fn entries(events: &[FeedEvent]) -> Vec<(u32, Article)> {
        events
            .iter()
            .filter_map(|e| match e {
                FeedEvent::Entry { entry, article } => Some((entry.seq, *article)),
                _ => None,
            })
            .collect()
    }

    /// NETWORK 4.2, 4.4: a feed's current version of a program is its
    /// program entry with that name and the highest seq; the host downloads
    /// the current versions (not older ones), whatever the article limit,
    /// once the feed is caught up, up to the largest image; programs are not
    /// in the timeline; the programs records (SPEC §content.feeds).
    /// NETWORK 4.8: a lookup asks the relays for the current version of a
    /// program of a feed the host does not follow; the answer is verified
    /// (forged or mismatched ones are dropped) and stored.
    #[test]
    fn lookup_programs() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, script) = node_with_fake(dir.path());
        let author = Identity::from_seed([2; 32]);
        let feed = author.feed_key();
        let program = |seq: u32, data: &[u8], name: &str| Entry::program(feed, seq, 100, data, name).sign(&author).unwrap().bytes;
        script.lock().unwrap().held = vec![program(1, b"v1", "tool"), program(2, b"v2", "tool"), program(3, b"x", "other")];
        let run = |node: &mut Node, token: Token, name: &[u8]| {
            node.start_lookup(token, feed, name);
            node.wait_for(token)
        };
        let Done::Found(found) = run(&mut node, 1, b"tool") else { panic!() };
        assert_eq!((found.len(), found[0].entry.seq), (1, 2));
        assert_eq!(node.current_program(&feed, b"tool").map(|e| e.blob), Some(blob_id(b"v2")), "stored");
        assert!(!node.subscriptions().contains(&feed), "without subscribing");
        assert_eq!(run(&mut node, 2, b"nope"), Done::NotFound);
        assert_eq!(run(&mut node, 3, b" bad name"), Done::Failed);
        // Forged: an entry of another feed, or of another name.
        let other = Identity::from_seed([3; 32]);
        script.lock().unwrap().lookup_reply = Some(Entry::program(other.feed_key(), 1, 0, b"evil", "fresh").sign(&other).unwrap().bytes);
        assert_eq!(run(&mut node, 4, b"fresh"), Done::Failed);
        script.lock().unwrap().lookup_reply = Some(program(9, b"evil", "tool"));
        assert_eq!(run(&mut node, 5, b"fresh"), Done::Failed);
        assert!(node.current_program(&feed, b"fresh").is_none());
    }

    /// Documents (kind 4) are in the timeline with the articles.
    #[test]
    fn documents_in_the_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let author = Identity::from_seed([6; 32]);
        node.set_identity(author.clone());
        assert!(matches!(node.publish_article(&Draft { data: b"text".to_vec(), title: "A".into(), reply_to: None, time: 0 }), Done::Published { .. }));
        let doc = [&b"TDOC\0"[..], &[0; 32], &[6], b"sketch", b"data"].concat();
        node.start_publish_document(9, &doc, "Sketch 1", 0);
        assert!(matches!(node.wait_for(9), Done::Published { seq: 2, .. }));
        assert!(matches!(node.publish_program(b"image", "tool", 0), Done::Published { seq: 3, .. }));
        node.set_subscriptions(Subscriptions::in_memory([author.feed_key()]));
        let (total, data) = node.timeline_records(0, 4096);
        assert_eq!(total, 2, "the article and the document, not the program");
        assert_eq!((data[2 + 40], &data[2 + 109..2 + 117]), (KIND_DOCUMENT, &b"Sketch 1"[..]));
    }

    #[test]
    fn programs_current_versions_and_their_download() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, script) = node_with_fake(dir.path());
        let author = Identity::from_seed([2; 32]);
        let feed = author.feed_key();
        let program = |seq: u32, data: &[u8], name: &str| Entry::program(feed, seq, 100, data, name).sign(&author).unwrap().bytes;
        let big = vec![9u8; MAX_IMAGE as usize + 1];
        let (v1, v2, tool, other) = (vec![1u8; 40_000], vec![2u8; 50_000], b"tool image".to_vec(), b"other".to_vec());
        {
            let mut s = script.lock().unwrap();
            for data in [&v1, &v2, &tool, &big, &other] {
                s.blobs.insert(blob_id(data), data.clone());
            }
            s.held = vec![
                program(1, &v1, "sparks"),
                program(2, &tool, "tool"),
                signed(&author, 3, b"an article", "Post"),
                program(4, &v2, "sparks"),
                program(5, &big, "huge"),
            ];
        }
        node.set_auto_download_limit(16);
        node.set_subscriptions(Subscriptions::in_memory([feed]));
        let events = settle(&mut node);
        let downloaded: Vec<(u32, Article)> = events
            .iter()
            .filter_map(|e| match e {
                FeedEvent::ProgramDownloaded { entry, article } => Some((entry.seq, *article)),
                _ => None,
            })
            .collect();
        let mut downloaded = downloaded;
        downloaded.sort_by_key(|d| d.0);
        assert_eq!(downloaded, [(2, Article::Local), (4, Article::Local)]);
        assert!(node.store().read(&blob_id(&v2)).unwrap().is_some());
        assert!(node.store().read(&blob_id(&v1)).unwrap().is_none(), "an older version is not downloaded");
        assert!(node.store().read(&blob_id(&big)).unwrap().is_none(), "larger than any image");
        assert!(!script.lock().unwrap().asked.contains(&blob_id(&v1)));
        // Every entry is reported; program entries at once, as not fetched.
        let mut got = entries(&events);
        got.sort_by_key(|e| e.0);
        assert_eq!(got.iter().map(|e| e.0).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);

        // The current versions, by name; the records, by feed then name,
        // with the local flag.
        let names: Vec<(String, u32)> = node.current_programs(&feed).into_iter().map(|s| (s.entry.title, s.entry.seq)).collect();
        assert_eq!(names, [("huge".into(), 5), ("sparks".into(), 4), ("tool".into(), 2)]);
        assert_eq!(node.current_program(&feed, b"sparks").map(|e| e.blob), Some(blob_id(&v2)));
        assert_eq!(node.current_program(&feed, b"nope"), None);
        assert_eq!(node.timeline(), [(feed, 3)], "programs are not in the timeline");
        let (total, data) = node.program_records(0, 10_000);
        assert_eq!(total, 3);
        let mut at = 0;
        let mut seen = Vec::new();
        while at < data.len() {
            let (l, flags) = (data[at] as usize, data[at + 1]);
            let e = &data[at + 2..at + 1 + l];
            seen.push((String::from_utf8(e[109..].to_vec()).unwrap(), e[40], flags));
            at += 1 + l;
        }
        assert_eq!(seen, [("huge".into(), KIND_PROGRAM, 0), ("sparks".into(), KIND_PROGRAM, 1), ("tool".into(), KIND_PROGRAM, 1)]);
        assert_eq!(node.program_records(1, 10_000).1.len(), data.len() - (2 + 109 + 4));

        // A new version arriving live is downloaded at once; with the
        // download turned off, the next is not.
        let v3 = vec![3u8; 30_000];
        script.lock().unwrap().blobs.insert(blob_id(&v3), v3.clone());
        let live = protocol::encode_entry(&program(6, &v3, "sparks"));
        script.lock().unwrap().live.push(live);
        let events = settle(&mut node);
        assert!(events.contains(&FeedEvent::ProgramDownloaded {
            entry: entry::decode(&program(6, &v3, "sparks")).unwrap().entry,
            article: Article::Local
        }));
        node.set_program_download(false);
        let v4 = vec![4u8; 30_000];
        script.lock().unwrap().blobs.insert(blob_id(&v4), v4.clone());
        let live = protocol::encode_entry(&program(7, &v4, "sparks"));
        script.lock().unwrap().live.push(live);
        for _ in 0..20 {
            node.poll();
        }
        assert!(node.store().read(&blob_id(&v4)).unwrap().is_none());
        assert_eq!(node.current_program(&feed, b"sparks").map(|e| e.seq), Some(7));
        // Unsubscribed: no programs.
        node.set_subscriptions(Subscriptions::in_memory([]));
        assert_eq!(node.program_records(0, 10_000), (0, Vec::new()));
    }

    /// NETWORK 4.7: programs found are kept only in their current versions.
    #[test]
    fn find_programs_keeps_current_versions() {
        let author = Identity::from_seed([2; 32]);
        let p = |seq: u32, name: &str| entry::decode(&Entry::program(author.feed_key(), seq, 0, b"img", name).sign(&author).unwrap().bytes).unwrap();
        let merged = merge_found(SCOPE_PROGRAMS, b"s", vec![vec![p(1, "sparks"), p(2, "stars")], vec![p(3, "sparks")]]);
        let got: Vec<(u32, &str)> = merged.iter().map(|s| (s.entry.seq, s.entry.title.as_str())).collect();
        assert_eq!(got, [(3, "sparks"), (2, "stars")], "by rank: the first of each relay, then the second");
    }

    #[test]
    fn follows_feeds_verifies_and_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, script) = node_with_fake(dir.path());
        let author = Identity::from_seed([2; 32]);
        let feed = author.feed_key();
        let entry = |seq: u32, data: &[u8], size: u32| {
            let mut e = Entry::article(feed, seq, 100, data, None, "t");
            e.size = size;
            e.sign(&author).unwrap().bytes
        };
        {
            let mut s = script.lock().unwrap();
            for data in [&b"small"[..], &[7; 20], b"liar"] {
                s.blobs.insert(blob_id(data), data.to_vec());
            }
            s.held = vec![entry(1, b"small", 5), entry(2, &[7; 20], 20), entry(3, b"liar", 3), entry(4, b"gone", 4)];
            // A forged entry: valid layout, bad signature.
            let mut forged = entry(5, b"small", 5);
            forged[40 + 30] ^= 1;
            s.held.push(forged);
            // A valid entry for a feed not subscribed.
            let other = Identity::from_seed([3; 32]);
            s.held.push(Entry::article(other.feed_key(), 1, 1, b"small", None, "x").sign(&other).unwrap().bytes);
            let other_entry = protocol::encode_entry(&s.held[5]);
            s.live.push(other_entry);
        }
        node.set_auto_download_limit(10);
        node.set_subscriptions(Subscriptions::in_memory([feed]));
        let events = settle(&mut node);
        assert_eq!(events[0], FeedEvent::Connected("relay".into()));
        assert!(events.contains(&FeedEvent::Subscribed(feed)));
        let mut got = entries(&events);
        got.sort_by_key(|e| e.0);
        assert_eq!(
            got,
            [(1, Article::Local), (2, Article::NotFetched), (3, Article::SizeMismatch), (4, Article::Failed)]
        );
        assert!(node.store().read(&blob_id(b"small")).unwrap().is_some());
        assert!(node.store().read(&blob_id(b"liar")).unwrap().is_none(), "mismatched blob not stored");
        assert_eq!(node.feed_store().seqs(&feed).unwrap(), [1, 2, 3, 4]);
        assert!(node.feed_store().feeds().unwrap() == [feed], "nothing stored for other feeds");

        // A live entry arrives; then the connection drops and the node
        // resubscribes after the highest seq it holds.
        let data = b"fresh";
        script.lock().unwrap().blobs.insert(blob_id(data), data.to_vec());
        let fresh = entry(6, data, 5);
        script.lock().unwrap().live.push(protocol::encode_entry(&fresh));
        assert_eq!(entries(&settle(&mut node)), [(6, Article::Local)]);
        script.lock().unwrap().drop = true;
        node.poll();
        node.poll();
        let events = node.take_feed_events();
        assert!(matches!(events[..], [FeedEvent::Disconnected { .. }, FeedEvent::Unreachable]), "{events:?}");
        node.skip_backoff();
        let events = settle(&mut node);
        assert_eq!(entries(&events), [], "nothing new: {events:?}");
        let s = script.lock().unwrap();
        assert_eq!(s.connections, 2);
        assert_eq!(s.subscribes, [(feed, 0), (feed, 6)]);
    }

    /// Prefetch (NETWORK 4.4): the first 8 distinct links not held (also
    /// the target of a link with text), one level, none known or found to
    /// be over the limit; off with `set_prefetch(false)`.
    #[test]
    fn prefetches_links_of_downloaded_articles() {
        let author = Identity::from_seed([6; 32]);
        let feed = author.feed_key();
        let link = |data: &[u8]| format!("terse:{}", to_hex(&blob_id(data)));
        let held = b"held already".to_vec();
        let deeper = b"one level down".to_vec();
        let nested = format!("Nested: {}", link(&deeper)).into_bytes();
        let small: Vec<Vec<u8>> = (1..=7).map(|i| format!("small {i}").into_bytes()).collect();
        let big_known = vec![b'y'; 2000];
        let big_unknown = vec![b'x'; 2000];
        // One through a link with text (ARTICLE.md).
        let mut text =
            format!("{} {} {} [Nested]({}) {}", link(&held), link(&big_known), link(&big_unknown), link(&nested), link(&small[0]));
        text += &format!(" again {} feed terse:feed/{}", link(&small[0]), to_hex(&blob_id(&small[6])));
        for s in &small[1..] {
            text += &format!(" {}", link(s));
        }
        let main = text.into_bytes();
        assert!(main.len() <= 1500);
        let run = |prefetch: bool| {
            let dir = tempfile::tempdir().unwrap();
            let (mut node, script) = node_with_fake(dir.path());
            node.store().put(&held).unwrap();
            {
                let mut s = script.lock().unwrap();
                for b in [&big_known, &big_unknown, &deeper, &nested, &main].into_iter().chain(&small) {
                    s.blobs.insert(blob_id(b), b.clone());
                }
                s.held = vec![
                    Entry::article(feed, 1, 1, &big_known, None, "big").sign(&author).unwrap().bytes,
                    Entry::article(feed, 2, 2, &main, None, "main").sign(&author).unwrap().bytes,
                ];
            }
            node.set_auto_download_limit(1500);
            node.set_prefetch(prefetch);
            node.set_subscriptions(Subscriptions::in_memory([feed]));
            settle(&mut node);
            let asked = script.lock().unwrap().asked.clone();
            (dir, node, asked)
        };
        let (_dir, node, asked) = run(true);
        let mut want = vec![blob_id(&main), blob_id(&big_unknown), blob_id(&nested)];
        want.extend(small[..5].iter().map(|s| blob_id(s)));
        assert_eq!(asked, want, "8 links: held and known-big skipped, not counted beyond the 8, nested not followed");
        let has = |b: &[u8]| node.store().read(&blob_id(b)).unwrap().is_some();
        assert!(has(&nested) && small[..5].iter().all(|s| has(s)));
        assert!(!has(&big_unknown), "fetched, over the limit, discarded");
        assert!(!has(&big_known) && !has(&deeper) && !has(&small[5]));
        let (_dir, _node, asked) = run(false);
        assert_eq!(asked, [blob_id(&main)]);
    }

    #[test]
    fn publishing_through_a_relay() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, script) = node_with_fake(dir.path());
        let author = Identity::from_seed([4; 32]);
        node.set_identity(author.clone());
        let draft = Draft { data: b"article".to_vec(), title: "T".into(), reply_to: Some([9; 32]), time: 77 };
        assert_eq!(node.publish_article(&draft), Done::Published { seq: 1, blob: blob_id(b"article") });
        assert_eq!(node.publish_article(&draft), Done::Published { seq: 2, blob: blob_id(b"article") });
        let s = script.lock().unwrap();
        assert!(s.blobs.contains_key(&blob_id(b"article")));
        let sent = entry::decode(&s.held[1]).unwrap().entry;
        assert_eq!((sent.seq, sent.time, sent.reply_to, sent.size), (2, 77, [9; 32], 7));
        assert_eq!(node.feed_store().highest(&author.feed_key()).unwrap(), 2);
    }

    /// Two relays: puts and publishes go to both; a get asks the other when
    /// one does not have the blob; with one down, everything still works
    /// through the other.
    #[test]
    fn several_relays() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let (a, sa) = fake_relay();
        let (b, sb) = fake_relay();
        node.add_relay("ws://a", a);
        node.add_relay("ws://b", b);
        node.set_identity(Identity::from_seed([5; 32]));
        assert_eq!(node.publish(b"both"), Done::Put(blob_id(b"both")));
        let draft = Draft { data: b"art".to_vec(), title: "T".into(), reply_to: None, time: 1 };
        assert!(matches!(node.publish_article(&draft), Done::Published { seq: 1, .. }));
        for s in [&sa, &sb] {
            let s = s.lock().unwrap();
            assert!(s.blobs.contains_key(&blob_id(b"both")) && s.blobs.contains_key(&blob_id(b"art")));
            assert_eq!(s.held.len(), 1);
        }
        // Only B holds this blob.
        sb.lock().unwrap().blobs.insert(blob_id(b"only b"), b"only b".to_vec());
        assert_eq!(node.fetch(blob_id(b"only b")), Done::Got(b"only b".to_vec()));
        assert_eq!(node.fetch(blob_id(b"nowhere")), Done::NotFound);
        // A goes down: publishing and getting still work through B.
        {
            let mut s = sa.lock().unwrap();
            s.drop = true;
            s.down = true;
        }
        node.poll();
        assert_eq!(node.connected_relays(), ["ws://b"]);
        assert!(matches!(node.publish_article(&draft), Done::Published { seq: 2, .. }));
        sb.lock().unwrap().blobs.insert(blob_id(b"later"), b"later".to_vec());
        assert_eq!(node.fetch(blob_id(b"later")), Done::Got(b"later".to_vec()));
        // A comes back: the node reconnects.
        sa.lock().unwrap().down = false;
        node.skip_backoff();
        node.poll();
        assert_eq!(node.connected_relays(), ["ws://a", "ws://b"]);
        assert_eq!(sa.lock().unwrap().connections, 2);
    }

    /// Search: results from two relays are verified, merged and ordered.
    #[test]
    fn find_merges_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let (a, sa) = fake_relay();
        let (b, sb) = fake_relay();
        node.add_relay("ws://a", a);
        node.add_relay("ws://b", b);
        let ids: Vec<Identity> = (10..14).map(|i| Identity::from_seed([i; 32])).collect();
        let profile = |id: &Identity, seq: u32, name: &str| {
            Entry::profile(id.feed_key(), seq, seq, b"listed: yes\n", name).sign(id).unwrap().bytes
        };
        let mut forged = profile(&ids[3], 1, "Ana forged");
        forged[50] ^= 1;
        sa.lock().unwrap().find = vec![profile(&ids[0], 1, "Mariana"), profile(&ids[1], 2, "Ana"), forged];
        sb.lock().unwrap().find = vec![profile(&ids[1], 5, "Anabel"), profile(&ids[2], 1, "ana"), profile(&ids[0], 1, "Mariana")];
        let Done::Found(found) = node.find(SCOPE_FEEDS, b"ANA") else { panic!() };
        let got: Vec<(&str, u32)> = found.iter().map(|s| (s.entry.title.as_str(), s.entry.seq)).collect();
        // Prefix matches first (by feed key), then the rest; the newer
        // profile of ids[1] wins; the forged entry is gone.
        let mut prefix = vec![(ids[1].feed_key(), ("Anabel", 5)), (ids[2].feed_key(), ("ana", 1))];
        prefix.sort();
        let want: Vec<(&str, u32)> = prefix.into_iter().map(|p| p.1).chain([("Mariana", 1)]).collect();
        assert_eq!(got, want);
        // Entries scope: articles only, merged by rank in relay order (each
        // relay's own order is kept; claimed times do not matter), each once.
        let art = |id: &Identity, seq: u32, time: u32, title: &str| {
            Entry::article(id.feed_key(), seq, time, b"x", None, title).sign(id).unwrap().bytes
        };
        sa.lock().unwrap().find = vec![
            art(&ids[0], 1, 50, "a1 news"),
            profile(&ids[0], 2, "news"),
            art(&ids[0], 3, 99, "a2 news"),
            art(&ids[0], 4, 10, "a3 news"),
        ];
        sb.lock().unwrap().find =
            vec![art(&ids[1], 1, 90, "b1 news"), art(&ids[0], 1, 50, "a1 news"), art(&ids[1], 2, 1, "b2 news")];
        let Done::Found(found) = node.find(SCOPE_ENTRIES, b"news") else { panic!() };
        let titles: Vec<&str> = found.iter().map(|s| s.entry.title.as_str()).collect();
        assert_eq!(titles, ["a1 news", "b1 news", "a2 news", "a3 news", "b2 news"]);
        // A refusal counts only when no relay answered.
        sa.lock().unwrap().find_refused = true;
        assert!(matches!(node.find(SCOPE_ENTRIES, b"news"), Done::Found(f) if f.len() == 3));
        sb.lock().unwrap().find_refused = true;
        assert_eq!(node.find(SCOPE_ENTRIES, b"news"), Done::Refused);
    }

    fn signed(author: &Identity, seq: u32, data: &[u8], title: &str) -> Vec<u8> {
        Entry::article(author.feed_key(), seq, 1000 + seq, data, None, title).sign(author).unwrap().bytes
    }

    /// Feed relays: a profile held on the host's relay names another relay,
    /// which the node then also uses for the feed and for gets.
    #[test]
    fn feed_relays_from_profiles() {
        for on in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let (mut node, sa) = node_with_fake(dir.path());
            let (b, sb) = fake_relay();
            let b = Arc::new(Mutex::new(Some(b)));
            node.set_feed_relays_unrestricted(Box::new(move |url| {
                assert_eq!(url, "ws://feed-relay");
                b.lock().unwrap().take().unwrap()
            }));
            node.set_feed_relays(on);
            let author = Identity::from_seed([30; 32]);
            let doc = b"listed: no\nrelay: ws://feed-relay\n";
            {
                let mut s = sa.lock().unwrap();
                s.blobs.insert(blob_id(doc), doc.to_vec());
                s.held.push(Entry::profile(author.feed_key(), 1, 1, doc, "Author").sign(&author).unwrap().bytes);
            }
            sb.lock().unwrap().blobs.insert(blob_id(b"on b"), b"on b".to_vec());
            sb.lock().unwrap().held.push(signed(&author, 2, b"on b", "B only"));
            node.set_subscriptions(Subscriptions::in_memory([author.feed_key()]));
            let events = poll_n(&mut node, 50);
            let seqs = node.feed_store().seqs(&author.feed_key()).unwrap();
            if on {
                assert_eq!(node.connected_relays(), ["relay", "ws://feed-relay"], "{events:?}");
                assert_eq!(seqs, [1, 2]);
                assert!(node.store().read(&blob_id(b"on b")).unwrap().is_some(), "downloaded from B");
            } else {
                assert_eq!(node.connected_relays(), ["relay"]);
                assert_eq!(seqs, [1]);
                assert_eq!(node.fetch(blob_id(b"on b")), Done::NotFound);
            }
        }
    }

    /// Polls `n` times and returns the events.
    fn poll_n(node: &mut Node, n: usize) -> Vec<FeedEvent> {
        (0..n).flat_map(|_| {
            node.poll();
            node.take_feed_events()
        })
        .collect()
    }

    /// NETWORK 4.3: caught up when the entry with `seq` = `highest` arrives,
    /// or at once if `highest` is not above `after`.
    #[test]
    fn caught_up_rule() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, script) = node_with_fake(dir.path());
        let author = Identity::from_seed([6; 32]);
        let feed = author.feed_key();
        {
            let mut s = script.lock().unwrap();
            s.held = vec![signed(&author, 1, b"a", "a"), signed(&author, 3, b"b", "b")];
            s.stall = true;
        }
        node.set_auto_download_limit(0);
        node.set_subscriptions(Subscriptions::in_memory([feed]));
        // Subscribed, but the backlog has not arrived: not settled.
        let events = poll_n(&mut node, 5);
        assert!(events.contains(&FeedEvent::Subscribed(feed)), "{events:?}");
        assert!(!events.contains(&FeedEvent::Settled), "{events:?}");
        script.lock().unwrap().stall = false;
        let events = settle(&mut node);
        assert_eq!(entries(&events), [(1, Article::NotFetched), (3, Article::NotFetched)]);

        // Reconnect: highest (3) equals after (3): settled with nothing new.
        script.lock().unwrap().drop = true;
        poll_n(&mut node, 2);
        node.skip_backoff();
        assert_eq!(entries(&settle(&mut node)), []);
        // A relay that holds less than we do (highest < after) is caught up at once.
        script.lock().unwrap().claim_highest = Some(1);
        script.lock().unwrap().drop = true;
        poll_n(&mut node, 2);
        node.skip_backoff();
        assert_eq!(entries(&settle(&mut node)), []);
        // An invalid last entry still ends the catch-up.
        {
            let mut s = script.lock().unwrap();
            s.claim_highest = None;
            let mut forged = signed(&author, 4, b"c", "c");
            forged[50] ^= 1;
            s.held.push(forged);
            s.drop = true;
        }
        poll_n(&mut node, 2);
        node.skip_backoff();
        assert_eq!(entries(&settle(&mut node)), []);
        assert_eq!(script.lock().unwrap().subscribes, [(feed, 0), (feed, 3), (feed, 3), (feed, 3)]);
    }

    /// An empty relay: subscribed with `highest` 0, settled at once.
    #[test]
    fn caught_up_with_a_relay_with_no_entries() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, _script) = node_with_fake(dir.path());
        node.set_subscriptions(Subscriptions::in_memory([Identity::from_seed([7; 32]).feed_key()]));
        let events = settle(&mut node);
        assert_eq!(entries(&events), []);
        assert_eq!(node.timeline_records(0, 16384), (0, Vec::new()));
    }

    fn timeline_titles(node: &mut Node) -> Vec<String> {
        let (total, data) = node.timeline_records(0, 16384);
        let mut titles = Vec::new();
        let mut at = 0;
        while at < data.len() {
            let n = data[at + 2 + 108] as usize;
            titles.push(String::from_utf8(data[at + 2 + 109..at + 2 + 109 + n].to_vec()).unwrap());
            at += 1 + data[at] as usize;
        }
        assert_eq!(titles.len(), total as usize);
        titles
    }

    /// SPEC §content.feeds: newest arrival first across feeds, the same after a
    /// restart; unsubscribed feeds drop out.
    #[test]
    fn timeline_order_across_feeds_and_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (Identity::from_seed([8; 32]), Identity::from_seed([9; 32]));
        let (mut node, script) = node_with_fake(dir.path());
        {
            let mut s = script.lock().unwrap();
            s.blobs.insert(blob_id(b"a1"), b"a1".to_vec());
            s.held = vec![signed(&a, 1, b"a1", "a1"), signed(&a, 2, b"a2", "a2")];
        }
        node.set_subscriptions(Subscriptions::in_memory([a.feed_key(), b.feed_key()]));
        settle(&mut node);
        for (author, seq, title) in [(&b, 1, "b1"), (&a, 3, "a3"), (&b, 2, "b2")] {
            let e = signed(author, seq, title.as_bytes(), title);
            script.lock().unwrap().live.push(protocol::encode_entry(&e));
            settle(&mut node);
        }
        assert_eq!(timeline_titles(&mut node), ["b2", "a3", "b1", "a2", "a1"]);

        // Records: L, flags (a1 was downloaded) and the unsigned entry.
        let (total, data) = node.timeline_records(4, 16384);
        assert_eq!(total, 5);
        let want = &signed(&a, 1, b"a1", "a1")[..111];
        assert_eq!(data, [&[112u8, 1][..], want].concat());
        // Whole records only; an offset past the end; a buffer too small.
        assert_eq!(node.timeline_records(0, 113 + 112).1.len(), 113);
        assert_eq!(node.timeline_records(5, 1000), (5, Vec::new()));
        assert_eq!(node.timeline_records(0, 100), (5, Vec::new()));

        // Subscriptions: key order, highest seq held.
        let (total, data) = node.subscription_records(0, 100);
        let mut keys = [a.feed_key(), b.feed_key()];
        keys.sort();
        let highest = |k: &FeedKey| if *k == a.feed_key() { 3u32 } else { 2 };
        let want: Vec<u8> = keys.iter().flat_map(|k| [&k[..], &highest(k).to_le_bytes()].concat()).collect();
        assert_eq!((total, data), (2, want.clone()));
        assert_eq!(node.subscription_records(1, 36), (2, want[36..].to_vec()));
        assert_eq!(node.subscription_records(0, 35), (2, Vec::new()));

        // A restart: the same order.
        drop(node);
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        node.set_subscriptions(Subscriptions::in_memory([a.feed_key(), b.feed_key()]));
        assert_eq!(timeline_titles(&mut node), ["b2", "a3", "b1", "a2", "a1"]);
        // Unsubscribed feeds drop out, and return in place.
        node.set_subscriptions(Subscriptions::in_memory([a.feed_key()]));
        assert_eq!(timeline_titles(&mut node), ["a3", "a2", "a1"]);
        node.set_subscriptions(Subscriptions::in_memory([a.feed_key(), b.feed_key()]));
        assert_eq!(timeline_titles(&mut node), ["b2", "a3", "b1", "a2", "a1"]);
        // A damaged entry file is left out.
        std::fs::write(dir.path().join("feeds").join(to_hex(&a.feed_key())).join("0000000003"), b"junk").unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        node.set_subscriptions(Subscriptions::in_memory([a.feed_key(), b.feed_key()]));
        assert_eq!(timeline_titles(&mut node), ["b2", "b1", "a2", "a1"]);
    }
}
