//! Host behaviour (NETWORK 2, 4.4): gets are answered from the local store,
//! else fetched whole from the relay, verified and stored; puts are stored
//! locally and sent to the relay. With subscriptions, the node keeps a relay
//! connection, subscribes to each feed after the highest `seq` it holds,
//! verifies and stores pushed entries, and downloads small articles.
//!
//! Reconnecting: when the connection is lost (or cannot be made) while there
//! are subscriptions, the node retries after 1 s, doubling up to 30 s, and
//! subscribes again on every new connection.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::blob::{BlobId, blob_id, to_hex, valid_size};
use crate::entry::{self, Entry, MAX_TITLE, SignedEntry};
use crate::feeds::{FeedStore, Insert, Subscriptions};
use crate::identity::{FeedKey, Identity};
use crate::relay::{RelayClient, RelayReply};
use crate::store::{BlobStore, default_store_dir};

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
    /// Every subscription on this connection is answered and no automatic
    /// download is in progress.
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
    Subscribe(FeedKey),
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
}

impl Node {
    /// A node on `store` (feed entries are kept beside its blobs) with no
    /// subscriptions and no identity.
    pub fn new(store: BlobStore, relay: Option<RelayClient>) -> Self {
        Node {
            feeds: FeedStore::beside_blobs(store.dir()),
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
        let done = match self.feeds.insert(signed) {
            Ok(Insert::New | Insert::Duplicate) => Done::Published { seq: signed.entry.seq, blob: signed.entry.blob },
            _ => Done::Failed,
        };
        self.ready.push((token, done));
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
                self.waiting.insert(req, Waiting::Subscribe(feed));
            }
        }
        Ok(())
    }

    pub fn unsubscribe(&mut self, feed: &FeedKey) -> std::io::Result<()> {
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
            for feed in self.subscriptions.feeds() {
                let after = self.feeds.highest(feed).unwrap_or(0);
                let req = relay.subscribe(feed, after);
                self.waiting.insert(req, Waiting::Subscribe(*feed));
            }
        }
    }

    /// Verifies and stores an entry pushed by the relay, and starts its
    /// automatic download (NETWORK 4.4).
    fn receive(&mut self, bytes: &[u8]) {
        // Invalid entries and entries for feeds not subscribed are discarded.
        let Ok(signed) = entry::decode(bytes) else { return };
        if !self.subscriptions.contains(&signed.entry.feed) || self.feeds.insert(&signed).ok() != Some(Insert::New) {
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
            (Waiting::Subscribe(feed), RelayReply::Done) => return self.events.push(FeedEvent::Subscribed(feed)),
            (Waiting::Subscribe(feed), RelayReply::Refused) => {
                return self.events.push(FeedEvent::SubscribeRefused(feed));
            }
            // No answer: subscribe again on the next poll.
            (Waiting::Subscribe(_), _) => {
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
        let pending = self.waiting.values().any(|w| matches!(w, Waiting::Subscribe(_) | Waiting::AutoGet(_)));
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
                    self.inbox.push_back(protocol::encode_status_reply(protocol::SUBSCRIBE_REPLY, req, status::DONE));
                    let seq = |e: &Vec<u8>| u32::from_le_bytes(e[32..36].try_into().unwrap());
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
}
