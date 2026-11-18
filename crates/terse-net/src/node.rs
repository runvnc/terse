//! Host behaviour (NETWORK 2, 4.4): gets are answered from the local store,
//! else fetched whole from the relay, verified and stored; puts are stored
//! locally and sent to the relay. With subscriptions, the node keeps a relay
//! connection, subscribes to each feed after the highest `seq` it holds,
//! verifies and stores pushed entries, and downloads small articles.
//!
//! Reconnecting: when the connection is lost (or cannot be made) while there
//! are subscriptions, the node retries after 1 s, doubling up to 30 s, and
//! subscribes again on every new connection.
//!
//! Caught up (NETWORK 4.3): a feed is caught up once the entry whose `seq`
//! equals the `highest` of the subscribe reply has arrived (valid or not:
//! an invalid entry is still the relay's last), or at once if `highest` is
//! not above `after`. The node is settled when every feed is caught up and
//! no automatic download is running.
//!
//! The node also answers the content device's feed operations (SPEC 9.5):
//! every new entry it stores is appended to the arrival log
//! ([`crate::timeline::Arrivals`]); the timeline is that log, newest first,
//! restricted to the feeds subscribed to now.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::blob::{BlobId, blob_id, to_hex, valid_size};
use crate::entry::{self, Entry, MAX_TITLE, SignedEntry};
use crate::feeds::{FeedStore, Insert, Subscriptions};
use crate::identity::{FeedKey, Identity};
use crate::relay::{RelayClient, RelayReply};
use crate::store::{BlobStore, default_store_dir};
use crate::timeline::{self, Arrivals};

/// Identifies an operation started on a [`Node`]; chosen by the caller.
pub type Token = u64;

/// Articles up to this size are downloaded as soon as their entry arrives
/// (NETWORK 4.4).
pub const AUTO_DOWNLOAD_LIMIT: u32 = 16_384;

const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// How an operation ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// The whole blob, verified against its ID.
    Got(Vec<u8>),
    /// The blob is stored (and accepted by the relay, if there is one).
    Put(BlobId),
    /// An article and its entry are stored (and accepted by the relay, if
    /// there is one).
    Published { seq: u32, blob: BlobId },
    NotFound,
    /// The relay refused an entry (for example, a different entry already
    /// has its `seq`).
    Refused,
    Failed,
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

/// What happened to the node's feeds; see [`Node::take_feed_events`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedEvent {
    /// A new valid entry for a subscribed feed is stored. Reported once any
    /// automatic download has finished.
    Entry { entry: Entry, article: Article },
    /// The relay accepted a subscription; entries it holds follow.
    Subscribed(FeedKey),
    SubscribeRefused(FeedKey),
    /// Connected to the relay; subscriptions are being renewed.
    Connected,
    /// Could not connect, or the connection was lost; the next attempt is
    /// in `retry_in`.
    Disconnected { retry_in: Duration },
    /// Every feed is caught up on this connection (NETWORK 4.3) and no
    /// automatic download is in progress.
    Settled,
    /// A subscription or download started after `Settled`; `Settled` follows
    /// when it is done.
    Busy,
    /// The result of `ContentService::publish`.
    Published { seq: u32, blob: BlobId },
    PublishFailed(Done),
}

enum Waiting {
    Get(Token, BlobId),
    Put(Token, BlobId),
    /// An article's blob is on its way to the relay; its entry (`seq` not
    /// yet assigned) is published next.
    ArticleBlob(Token, Entry),
    Entry(Token, SignedEntry),
    /// A subscribe sent with this `after`.
    Subscribe(FeedKey, u32),
    Unsubscribe,
    /// The automatic download of a new entry's article.
    AutoGet(Entry),
}

/// Relay connection state for subscriptions.
struct Follow {
    /// The relay connection on which every subscription was sent (0: none).
    generation: u64,
    connected: bool,
    retry_at: Option<Instant>,
    retry_in: Duration,
    settled: bool,
}

pub struct Node {
    store: BlobStore,
    relay: Option<RelayClient>,
    /// Operations waiting for the relay, by relay request number.
    waiting: HashMap<u32, Waiting>,
    ready: Vec<(Token, Done)>,
    feeds: FeedStore,
    subscriptions: Subscriptions,
    identity: Option<Identity>,
    auto_limit: u32,
    /// The last `seq` given to an own entry, so publishes in flight do not
    /// reuse one.
    last_seq: u32,
    follow: Follow,
    events: Vec<FeedEvent>,
    /// Feeds subscribed on this connection and not yet caught up, with the
    /// `highest` their subscribe reply gave.
    catching_up: HashMap<FeedKey, u32>,
    arrivals: Arrivals,
    /// Verified stored entries (`None`: invalid on disk), by feed and `seq`;
    /// stored entries never change.
    verified: HashMap<(FeedKey, u32), Option<(Entry, Vec<u8>)>>,
}

impl Node {
    /// A node on `store` (feed entries are kept beside its blobs) with no
    /// subscriptions and no identity.
    pub fn new(store: BlobStore, relay: Option<RelayClient>) -> Self {
        Node {
            feeds: FeedStore::beside_blobs(store.dir()),
            arrivals: Arrivals::beside_blobs(store.dir()),
            store,
            relay,
            waiting: HashMap::new(),
            ready: Vec::new(),
            subscriptions: Subscriptions::default(),
            identity: None,
            auto_limit: AUTO_DOWNLOAD_LIMIT,
            last_seq: 0,
            follow: Follow { generation: 0, connected: false, retry_at: None, retry_in: FIRST_RETRY, settled: false },
            events: Vec::new(),
            catching_up: HashMap::new(),
            verified: HashMap::new(),
        }
    }

    /// A node on `store_dir` (default: [`default_store_dir`]) and an optional
    /// `ws://` relay.
    pub fn open(store_dir: Option<PathBuf>, relay_url: Option<&str>, timeout: Duration) -> Result<Self, String> {
        let dir = store_dir.or_else(default_store_dir).ok_or("no blob store directory (use --store)")?;
        let store = BlobStore::open(&dir, None).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Node::new(store, relay_url.map(|url| RelayClient::websocket(url, timeout))))
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
        self.follow.generation = 0;
    }

    /// The identity whose feed `publish_article` writes to.
    pub fn set_identity(&mut self, identity: Identity) {
        self.identity = Some(identity);
    }

    pub fn set_auto_download_limit(&mut self, bytes: u32) {
        self.auto_limit = bytes;
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
        match &mut self.relay {
            Some(relay) => {
                let req = relay.get(&id);
                self.waiting.insert(req, Waiting::Get(token, id));
            }
            None => self.ready.push((token, Done::NotFound)),
        }
    }

    /// Starts publishing `data`.
    pub fn start_put(&mut self, token: Token, data: &[u8]) {
        let id = match self.store.put(data) {
            Ok(id) => id,
            Err(_) => return self.ready.push((token, Done::Failed)),
        };
        match &mut self.relay {
            Some(relay) => {
                let req = relay.put(data);
                self.waiting.insert(req, Waiting::Put(token, id));
            }
            None => self.ready.push((token, Done::Put(id))),
        }
    }

    /// Starts publishing an article (NETWORK 4.4): puts the blob, then signs
    /// and publishes an entry with the next `seq` of the identity's feed.
    pub fn start_publish_article(&mut self, token: Token, draft: &Draft) {
        let Some(identity) = &self.identity else { return self.ready.push((token, Done::Failed)) };
        if draft.title.len() > MAX_TITLE || self.store.put(&draft.data).is_err() {
            return self.ready.push((token, Done::Failed));
        }
        let entry = Entry::article(identity.feed_key(), 0, draft.time, &draft.data, draft.reply_to, &draft.title);
        match &mut self.relay {
            Some(relay) => {
                let req = relay.put(&draft.data);
                self.waiting.insert(req, Waiting::ArticleBlob(token, entry));
            }
            None => self.publish_entry(token, entry),
        }
    }

    fn publish_entry(&mut self, token: Token, mut entry: Entry) {
        let Some(identity) = &self.identity else { return self.ready.push((token, Done::Failed)) };
        let Ok(highest) = self.feeds.highest(&entry.feed) else { return self.ready.push((token, Done::Failed)) };
        entry.seq = highest.max(self.last_seq) + 1;
        let Ok(signed) = entry.sign(identity) else { return self.ready.push((token, Done::Failed)) };
        self.last_seq = signed.entry.seq;
        match &mut self.relay {
            Some(relay) => {
                let req = relay.publish(&signed.bytes);
                self.waiting.insert(req, Waiting::Entry(token, signed));
            }
            None => self.store_own_entry(token, &signed),
        }
    }

    fn store_own_entry(&mut self, token: Token, signed: &SignedEntry) {
        let done = match self.insert(signed) {
            Ok(Insert::New | Insert::Duplicate) => Done::Published { seq: signed.entry.seq, blob: signed.entry.blob },
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
        }
        Ok(insert)
    }

    /// Adds a feed to the subscriptions (saved if they are backed by a file)
    /// and subscribes on the current connection.
    pub fn subscribe(&mut self, feed: FeedKey) -> std::io::Result<()> {
        if self.subscriptions.add(feed)? {
            if let Some(relay) = self.relay.as_mut().filter(|r| r.connected())
                && relay.generation() == self.follow.generation
            {
                let after = self.feeds.highest(&feed).unwrap_or(0);
                let req = relay.subscribe(&feed, after);
                self.waiting.insert(req, Waiting::Subscribe(feed, after));
            }
        }
        Ok(())
    }

    pub fn unsubscribe(&mut self, feed: &FeedKey) -> std::io::Result<()> {
        self.catching_up.remove(feed);
        if self.subscriptions.remove(feed)?
            && let Some(relay) = self.relay.as_mut().filter(|r| r.connected())
        {
            let req = relay.unsubscribe(feed);
            self.waiting.insert(req, Waiting::Unsubscribe);
        }
        Ok(())
    }

    /// Whether any operation is waiting for the relay.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Whether the node has subscriptions and a relay, so it must be polled
    /// even when not busy.
    pub fn following(&self) -> bool {
        self.relay.is_some() && !self.subscriptions.is_empty()
    }

    pub fn connected(&self) -> bool {
        self.relay.as_ref().is_some_and(RelayClient::connected)
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
    /// bytes, and how many exist (SPEC 9.5). The order is the key order.
    pub fn subscription_records(&self, offset: u32, length: u16) -> (u32, Vec<u8>) {
        let feeds: Vec<&FeedKey> = self.subscriptions.feeds().collect();
        let records = feeds
            .iter()
            .skip(offset as usize)
            .map(|feed| timeline::subscription_record(feed, self.feeds.highest(feed).unwrap_or(0)));
        (feeds.len() as u32, timeline::fill(records, length as usize))
    }

    /// The stored entries of the subscribed feeds, newest arrival first
    /// (SPEC 9.5). Entries of feeds no longer subscribed are left out (and
    /// come back in place if the feed is subscribed again); entries stored
    /// without an arrival record (by an older version) come last, highest
    /// `seq` first. Entries that fail verification are left out.
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
        out.retain(|&(feed, seq)| self.verified_entry(feed, seq).is_some());
        out
    }

    /// A stored entry and its bytes without the signature, verified once.
    fn verified_entry(&mut self, feed: FeedKey, seq: u32) -> Option<&(Entry, Vec<u8>)> {
        let feeds = &self.feeds;
        self.verified
            .entry((feed, seq))
            .or_insert_with(|| {
                let signed = entry::decode(&feeds.get(&feed, seq).ok()??).ok()?;
                let unsigned = signed.bytes[..signed.bytes.len() - entry::SIGNATURE_LEN].to_vec();
                (signed.entry.feed == feed && signed.entry.seq == seq).then_some((signed.entry, unsigned))
            })
            .as_ref()
    }

    /// The timeline records from index `offset` that fit in `length` bytes,
    /// and how many exist (SPEC 9.5).
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

    /// Connects and renews subscriptions when needed (NETWORK 4.4).
    fn maintain(&mut self) {
        let Some(relay) = self.relay.as_mut() else { return };
        if self.subscriptions.is_empty() {
            return;
        }
        let now = Instant::now();
        let follow = &mut self.follow;
        if !relay.connected() {
            if follow.connected {
                follow.connected = false;
                follow.retry_at = Some(now + follow.retry_in);
                self.events.push(FeedEvent::Disconnected { retry_in: follow.retry_in });
            }
            if follow.retry_at.is_some_and(|t| now < t) {
                return;
            }
            if relay.connect().is_err() {
                self.events.push(FeedEvent::Disconnected { retry_in: follow.retry_in });
                follow.retry_at = Some(now + follow.retry_in);
                follow.retry_in = (follow.retry_in * 2).min(MAX_RETRY);
                return;
            }
        }
        if relay.generation() != follow.generation {
            *follow = Follow {
                generation: relay.generation(),
                connected: true,
                retry_at: None,
                retry_in: FIRST_RETRY,
                settled: false,
            };
            self.events.push(FeedEvent::Connected);
            self.catching_up.clear();
            for feed in self.subscriptions.feeds() {
                let after = self.feeds.highest(feed).unwrap_or(0);
                let req = relay.subscribe(feed, after);
                self.waiting.insert(req, Waiting::Subscribe(*feed, after));
            }
        }
    }

    /// Verifies and stores an entry pushed by the relay, and starts its
    /// automatic download (NETWORK 4.4).
    fn receive(&mut self, bytes: &[u8]) {
        // Caught up when the relay's last entry for the feed arrives, whether
        // or not it turns out to be valid (NETWORK 4.3).
        if bytes.len() >= 36 {
            let feed: FeedKey = bytes[..32].try_into().unwrap();
            let seq = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
            if self.catching_up.get(&feed).is_some_and(|&highest| seq >= highest) {
                self.catching_up.remove(&feed);
            }
        }
        // Invalid entries and entries for feeds not subscribed are discarded.
        let Ok(signed) = entry::decode(bytes) else { return };
        if !self.subscriptions.contains(&signed.entry.feed) || self.insert(&signed).ok() != Some(Insert::New) {
            return;
        }
        let entry = signed.entry;
        let article = match self.held_article(&entry) {
            Some(held) => held,
            None if entry.size <= self.auto_limit => {
                if let Some(relay) = &mut self.relay {
                    let req = relay.get(&entry.blob);
                    self.waiting.insert(req, Waiting::AutoGet(entry));
                    return;
                }
                Article::Failed
            }
            None => Article::NotFetched,
        };
        self.events.push(FeedEvent::Entry { entry, article });
    }

    /// The result of an automatic download.
    fn downloaded(&mut self, entry: Entry, reply: RelayReply) {
        let article = match reply {
            RelayReply::Got(bytes) if valid_size(bytes.len()) && blob_id(&bytes) == entry.blob => {
                if bytes.len() != entry.size as usize {
                    Article::SizeMismatch
                } else if self.store.put(&bytes).is_ok() {
                    Article::Local
                } else {
                    Article::Failed
                }
            }
            _ => Article::Failed,
        };
        self.events.push(FeedEvent::Entry { entry, article });
    }

    fn reply(&mut self, waiting: Waiting, reply: RelayReply) {
        let finished = match (waiting, reply) {
            (Waiting::Get(token, id), RelayReply::Got(bytes)) => {
                // NETWORK 2: verify before storing or delivering; a blob
                // that fails the check counts as not found on this relay.
                if valid_size(bytes.len()) && blob_id(&bytes) == id {
                    // A full local store does not stop delivery.
                    let _ = self.store.put(&bytes);
                    (token, Done::Got(bytes))
                } else {
                    (token, Done::NotFound)
                }
            }
            (Waiting::Get(token, _), RelayReply::NotFound) => (token, Done::NotFound),
            (Waiting::Put(token, id), RelayReply::Put(relay_id)) if relay_id == id => (token, Done::Put(id)),
            (Waiting::ArticleBlob(token, entry), RelayReply::Put(relay_id)) if relay_id == entry.blob => {
                return self.publish_entry(token, entry);
            }
            (Waiting::Entry(token, signed), RelayReply::Done) => return self.store_own_entry(token, &signed),
            (Waiting::Entry(token, _), RelayReply::Refused) => (token, Done::Refused),
            (Waiting::Get(token, _) | Waiting::Put(token, _), _)
            | (Waiting::ArticleBlob(token, _) | Waiting::Entry(token, _), _) => (token, Done::Failed),
            (Waiting::Subscribe(feed, after), RelayReply::Subscribed { highest }) => {
                if highest > after && self.subscriptions.contains(&feed) {
                    self.catching_up.insert(feed, highest);
                }
                return self.events.push(FeedEvent::Subscribed(feed));
            }
            (Waiting::Subscribe(feed, _), RelayReply::Refused) => {
                return self.events.push(FeedEvent::SubscribeRefused(feed));
            }
            // No answer: subscribe again on the next poll.
            (Waiting::Subscribe(..), _) => {
                self.follow.generation = 0;
                return;
            }
            (Waiting::Unsubscribe, _) => return,
            (Waiting::AutoGet(entry), reply) => return self.downloaded(entry, reply),
        };
        self.ready.push(finished);
    }

    /// Reports `Settled` when nothing is outstanding for the feeds.
    fn check_settled(&mut self) {
        let pending = !self.catching_up.is_empty()
            || self.waiting.values().any(|w| matches!(w, Waiting::Subscribe(..) | Waiting::AutoGet(_)));
        let current = self.relay.as_ref().is_some_and(|r| r.connected() && r.generation() == self.follow.generation);
        if pending {
            if std::mem::take(&mut self.follow.settled) {
                self.events.push(FeedEvent::Busy);
            }
        } else if current && !self.follow.settled && !self.subscriptions.is_empty() {
            self.follow.settled = true;
            self.events.push(FeedEvent::Settled);
        }
    }

    /// Returns finished operations and keeps subscriptions going; waits at
    /// most briefly. Feed events are collected for `take_feed_events`.
    pub fn poll(&mut self) -> Vec<(Token, Done)> {
        self.maintain();
        let (replies, pushed) = match &mut self.relay {
            Some(relay) => (relay.poll(), relay.take_pushed()),
            None => Default::default(),
        };
        for (req, reply) in replies {
            if let Some(waiting) = self.waiting.remove(&req) {
                self.reply(waiting, reply);
            }
        }
        for bytes in pushed {
            self.receive(&bytes);
        }
        self.check_settled();
        std::mem::take(&mut self.ready)
    }

    fn wait_for(&mut self, token: Token) -> Done {
        loop {
            if let Some((_, done)) = self.poll().into_iter().find(|(t, _)| *t == token) {
                return done;
            }
        }
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

    /// Publishes a blob, blocking.
    pub fn publish(&mut self, data: &[u8]) -> Done {
        self.start_put(0, data);
        self.wait_for(0)
    }

    /// Publishes an article, blocking.
    pub fn publish_article(&mut self, draft: &Draft) -> Done {
        self.start_publish_article(0, draft);
        self.wait_for(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{self, Request, status};
    use crate::relay::{DEFAULT_TIMEOUT, Transport};
    use std::collections::VecDeque;
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
        /// Report this `highest` in subscribe replies instead of the truth.
        claim_highest: Option<u32>,
        /// Hold back messages after the subscribe reply until cleared.
        stall: bool,
    }

    struct Fake {
        script: Arc<Mutex<Script>>,
        inbox: VecDeque<Vec<u8>>,
    }

    impl Transport for Fake {
        fn send(&mut self, message: Vec<u8>) -> std::io::Result<()> {
            let mut s = self.script.lock().unwrap();
            let reply = match protocol::decode_request(&message).unwrap() {
                Request::Get { req, id } => match s.blobs.get(&id) {
                    Some(b) => protocol::encode_get_reply(req, status::DONE, b),
                    None => protocol::encode_get_reply(req, status::NOT_FOUND, &[]),
                },
                Request::Put { req, data } => {
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

    fn node_with_fake(dir: &std::path::Path) -> (Node, Arc<Mutex<Script>>) {
        let script = Arc::new(Mutex::new(Script::default()));
        let s = script.clone();
        let connect = Box::new(move || {
            s.lock().unwrap().connections += 1;
            Ok(Box::new(Fake { script: s.clone(), inbox: VecDeque::new() }) as Box<dyn Transport>)
        });
        let relay = RelayClient::new(connect, DEFAULT_TIMEOUT);
        (Node::new(BlobStore::open(dir, None).unwrap(), Some(relay)), script)
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
        assert_eq!(events[0], FeedEvent::Connected);
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
        assert!(matches!(node.take_feed_events()[..], [FeedEvent::Disconnected { .. }]));
        node.follow.retry_at = Some(Instant::now()); // skip the backoff
        let events = settle(&mut node);
        assert_eq!(entries(&events), [], "nothing new: {events:?}");
        let s = script.lock().unwrap();
        assert_eq!(s.connections, 2);
        assert_eq!(s.subscribes, [(feed, 0), (feed, 6)]);
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

    fn signed(author: &Identity, seq: u32, data: &[u8], title: &str) -> Vec<u8> {
        Entry::article(author.feed_key(), seq, 1000 + seq, data, None, title).sign(author).unwrap().bytes
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
        node.follow.retry_at = Some(Instant::now());
        assert_eq!(entries(&settle(&mut node)), []);
        // A relay that holds less than we do (highest < after) is caught up at once.
        script.lock().unwrap().claim_highest = Some(1);
        script.lock().unwrap().drop = true;
        poll_n(&mut node, 2);
        node.follow.retry_at = Some(Instant::now());
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
        node.follow.retry_at = Some(Instant::now());
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

    /// SPEC 9.5: newest arrival first across feeds, the same after a
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
