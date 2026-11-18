//! What `terse-host` sets up before it runs a program: the relay (README,
//! "Getting started") and, with `--demo`, the user's own feed with four
//! sample articles.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terse_net::local::{self, Found};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{BlobStore, ContentService, Draft, FeedCommand, FeedEvent, FeedStore, Node, Subscriptions, to_hex};

/// The relays the host uses, and why.
#[derive(Debug, PartialEq, Eq)]
pub enum Relay {
    /// `--relay`, the config file, an already running local relay, or none.
    Found(Found),
    /// A relay started inside this process, with its store.
    Started { url: String, store: PathBuf },
}

impl Relay {
    pub fn urls(&self) -> Vec<String> {
        match self {
            Relay::Found(f) => f.urls(),
            Relay::Started { url, .. } => vec![url.clone()],
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Relay::Found(f) => f.describe(),
            Relay::Started { url, store } => format!("relay {url} (started here, store {})", store.display()),
        }
    }
}

/// The relays for a windowed run: `--relay` (repeatable; `none`: no
/// network), else the config directory's relay list, else a relay
/// accepting connections on
/// 127.0.0.1:`port`, else one started in this process on that port, with
/// its store in `data/relay` (no relay if there is no `data` directory or
/// the port cannot be bound).
pub fn choose_relay(given: &[String], config: &Path, port: u16, data: Option<&Path>) -> Result<Relay, String> {
    let found = local::find_relays(given, Some(config), port)?;
    if found != Found::Nothing {
        return Ok(Relay::Found(found));
    }
    let Some(data) = data else { return Ok(Relay::Found(Found::Nothing)) };
    let store_dir = data.join("relay");
    let store = BlobStore::open(&store_dir, None).map_err(|e| format!("{}: {e}", store_dir.display()))?;
    let listener = match TcpListener::bind(local::local_addr(port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("terse-host: cannot start a relay on port {port}: {e}");
            return Ok(Relay::Found(Found::Nothing));
        }
    };
    let relay = Arc::new(terse_relay::Relay::new(store, terse_relay::DEFAULT_FEED_CAP));
    std::thread::Builder::new()
        .name("terse-relay".into())
        .spawn(move || {
            if let Err(e) = terse_relay::serve(listener, relay) {
                eprintln!("terse-host: relay stopped: {e}");
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Relay::Started { url: local::local_url(port), store: store_dir })
}

/// A sample article published by `--demo`.
struct Sample {
    title: &'static str,
    text: &'static str,
}

/// Oldest first: the timeline shows the last one at the top. Only a feed
/// with no entries gets them (`prepare_demo`), so a sample added here
/// reaches new users only; README says how others publish it themselves.
const SAMPLES: [Sample; 4] = [
    Sample { title: "Welcome to Terse", text: include_str!("../demo/welcome.txt") },
    Sample { title: "How Terse works: a longer read", text: include_str!("../demo/long.txt") },
    Sample { title: "Hello in three languages: こんにちは, مرحبا", text: include_str!("../demo/languages.txt") },
    Sample { title: "Pictures in articles", text: include_str!("../demo/pictures.txt") },
];

/// How long `--demo` waits for the relay (catching up, publishing).
const DEMO_DEADLINE: Duration = Duration::from_secs(60);

fn now() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

/// `--demo`: makes sure the identity exists, subscribes to the own feed and,
/// if that feed has no entries (after catching up with the relay, if there
/// is one), publishes the sample articles. Returns how many it
/// published.
pub fn prepare_demo(store_dir: &Path, config: &Path, relays: &[String]) -> Result<usize, String> {
    let mut node = Node::open(Some(store_dir.to_path_buf()), relays, DEFAULT_TIMEOUT)?;
    node.set_identity_path(config.join("identity.key"))?;
    let key = node.ensure_identity()?;
    let mut subs = Subscriptions::load(config.join("subscriptions"))?;
    subs.add(key).map_err(|e| format!("{}: {e}", config.join("subscriptions").display()))?;
    node.set_subscriptions(subs);
    eprintln!("terse-host: demo: your feed key is {}", to_hex(&key));
    let mut service = ContentService::start(node);
    if service.following()
        && let Err(e) = service.wait_caught_up(DEMO_DEADLINE)
    {
        return Err(format!("demo: relay {}: {e}", relays.join(" ")));
    }
    let held = FeedStore::beside_blobs(store_dir).seqs(&key).map_err(|e| e.to_string())?.len();
    if held > 0 {
        eprintln!("terse-host: demo: your feed already has {held} entries; nothing published");
        return Ok(0);
    }
    for s in &SAMPLES {
        let draft = Draft { data: s.text.as_bytes().to_vec(), title: s.title.into(), reply_to: None, time: now() };
        service.feed(FeedCommand::Publish(draft));
        let deadline = Instant::now() + DEMO_DEADLINE;
        loop {
            match service.wait_feed_event(deadline.saturating_duration_since(Instant::now())) {
                Some(FeedEvent::Published { seq, .. }) => {
                    eprintln!("terse-host: demo: published \"{}\" as #{seq}", s.title);
                    break;
                }
                Some(FeedEvent::PublishFailed(done)) => return Err(format!("demo: publishing failed ({done:?})")),
                Some(_) => {}
                None => return Err("demo: the relay did not answer".into()),
            }
        }
    }
    Ok(SAMPLES.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free_port() -> u16 {
        TcpListener::bind(local::local_addr(0)).unwrap().local_addr().unwrap().port()
    }

    /// Config file, then a relay already running, then one started here;
    /// the port is chosen by the test (never 7878).
    #[test]
    fn relay_choice_order() {
        let config = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let port = free_port();
        // Nothing running, no file: started here.
        let started = choose_relay(&[], config.path(), port, Some(data.path())).unwrap();
        assert_eq!(started, Relay::Started { url: local::local_url(port), store: data.path().join("relay") });
        // Now something listens on the port: it is used.
        let again = choose_relay(&[], config.path(), port, Some(data.path())).unwrap();
        assert_eq!(again, Relay::Found(Found::Local(local::local_url(port))));
        // The started relay answers the protocol.
        let mut client = terse_net::relay::RelayClient::websocket(&local::local_url(port), DEFAULT_TIMEOUT);
        client.connect().unwrap();
        // A config file wins over the running relay.
        local::write_relays(config.path(), &["ws://elsewhere.example:7878".into(), "ws://b.example:1".into()]).unwrap();
        let file = choose_relay(&[], config.path(), port, Some(data.path())).unwrap();
        assert_eq!(file.urls(), ["ws://elsewhere.example:7878", "ws://b.example:1"]);
        assert!(matches!(file, Relay::Found(Found::ConfigFile(..))));
        // --relay wins over everything; none disables the network.
        assert!(choose_relay(&["none".into()], config.path(), port, None).unwrap().urls().is_empty());
        assert_eq!(choose_relay(&["ws://x:1".into()], config.path(), port, None).unwrap().urls(), ["ws://x:1"]);
        // No data directory and nothing running: no relay.
        let empty = tempfile::tempdir().unwrap();
        let none = choose_relay(&[], empty.path(), free_port(), None).unwrap();
        assert_eq!(none, Relay::Found(Found::Nothing));
    }

    /// `--demo` twice: four entries, not eight; with a relay, a second host
    /// with a fresh store finds them there and publishes nothing.
    #[test]
    fn demo_publishes_once() {
        let (store, config, relay_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let listener = TcpListener::bind(local::local_addr(0)).unwrap();
        let url = local::local_url(listener.local_addr().unwrap().port());
        let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(relay_dir.path(), None).unwrap(), 1024));
        std::thread::spawn(move || terse_relay::serve(listener, relay));
        let url = [url];
        assert_eq!(prepare_demo(store.path(), config.path(), &url).unwrap(), 4);
        assert_eq!(prepare_demo(store.path(), config.path(), &url).unwrap(), 0);
        let key = terse_net::Identity::load(&config.path().join("identity.key")).unwrap().feed_key();
        let feeds = FeedStore::beside_blobs(store.path());
        assert_eq!(feeds.seqs(&key).unwrap(), [1, 2, 3, 4]);
        let subs = Subscriptions::load(config.path().join("subscriptions")).unwrap();
        assert_eq!(subs.feeds().collect::<Vec<_>>(), [&key]);
        // Same config, empty store: caught up from the relay, nothing new.
        let fresh = tempfile::tempdir().unwrap();
        assert_eq!(prepare_demo(fresh.path(), config.path(), &url).unwrap(), 0);
        assert_eq!(FeedStore::beside_blobs(fresh.path()).seqs(&key).unwrap(), [1, 2, 3, 4]);
        assert_eq!(FeedStore::beside_blobs(relay_dir.path()).seqs(&key).unwrap(), [1, 2, 3, 4]);
        // Without a relay too.
        let (store, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert_eq!(prepare_demo(store.path(), config.path(), &[]).unwrap(), 4);
        assert_eq!(prepare_demo(store.path(), config.path(), &[]).unwrap(), 0);
        let key = terse_net::Identity::load(&config.path().join("identity.key")).unwrap().feed_key();
        let entries = FeedStore::beside_blobs(store.path()).entries(&key).unwrap();
        let titles: Vec<&str> = entries.iter().map(|e| e.entry.title.as_str()).collect();
        assert_eq!(titles, SAMPLES.map(|s| s.title));
        for s in &SAMPLES {
            assert!(s.title.len() <= 120 && s.text.len() <= terse_net::node::AUTO_DOWNLOAD_LIMIT as usize);
        }
    }
}
