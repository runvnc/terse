//! Feeds end to end (NETWORK 4) over an in-process relay on an ephemeral
//! loopback port, with `terse` run as separate author and reader processes.

mod common;

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use terse_net::{BlobStore, Entry, Identity, blob_id, protocol, to_hex};


fn start_relay(store: &Path) -> String {
    start_relay_kept(store).0
}

/// A relay and its URL, for a test that looks at what the relay sees.
fn start_relay_kept(store: &Path) -> (String, Arc<terse_relay::Relay>) {
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(store, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let served = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, served));
    (url, relay)
}

fn terse(args: &[&str]) -> Output {
    common::terse_command().args(args).output().unwrap()
}

fn terse_ok(args: &[&str]) -> String {
    let out = terse(args);
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// Publishes raw entry bytes as a client that is not a Terse host would.
fn raw_publish(url: &str, entry: &[u8]) -> u8 {
    let stream = TcpStream::connect(url.strip_prefix("ws://").unwrap()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut ws = tungstenite::client::client(url, stream).unwrap().0;
    ws.send(tungstenite::Message::binary(protocol::encode_publish(1, entry))).unwrap();
    loop {
        if let tungstenite::Message::Binary(b) = ws.read().unwrap() {
            return b[5];
        }
    }
}

struct User {
    store: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl User {
    fn new() -> Self {
        User { store: tempfile::tempdir().unwrap(), config: tempfile::tempdir().unwrap() }
    }

    fn store(&self) -> &str {
        self.store.path().to_str().unwrap()
    }

    /// `terse feed <args> --store … --config …` (and `--relay` if given).
    fn feed_args<'a>(&'a self, args: &[&'a str], relay: Option<&'a str>) -> Vec<&'a str> {
        let mut all = vec!["feed"];
        all.extend_from_slice(args);
        all.extend(["--store", self.store(), "--config", self.config.path().to_str().unwrap()]);
        if let Some(url) = relay {
            all.extend(["--relay", url]);
        }
        all
    }

    fn feed(&self, args: &[&str], relay: Option<&str>) -> String {
        terse_ok(&self.feed_args(args, relay))
    }

    fn has_blob(&self, data: &[u8]) -> bool {
        self.store.path().join(to_hex(&blob_id(data))).exists()
    }
}

#[test]
fn author_and_reader() {
    let relay_dir = tempfile::tempdir().unwrap();
    let (url, relay) = start_relay_kept(relay_dir.path());
    let (author, reader) = (User::new(), User::new());

    let config = author.config.path().to_str().unwrap();
    let key = terse_ok(&["key", "new", "--config", config]).trim().to_string();
    assert_eq!(key.len(), 64);
    assert_eq!(terse_ok(&["key", "show", "--config", config]).trim(), key);
    assert!(!terse(&["key", "new", "--config", config]).status.success(), "refuses to overwrite");

    // Three small articles and one over the 16,384-byte automatic download limit.
    let articles: Vec<Vec<u8>> = vec![
        b"First article.\n".to_vec(),
        "二つ目の記事です。\n".as_bytes().to_vec(),
        b"A reply.\n".to_vec(),
        "long line\n".repeat(2000).into_bytes(),
    ];
    for (i, data) in articles.iter().enumerate() {
        let file = author.store.path().join(format!("article{i}.txt"));
        std::fs::write(&file, data).unwrap();
        let first = to_hex(&blob_id(&articles[0]));
        let mut args = vec!["publish", file.to_str().unwrap(), "--title", ["one", "二", "re: one", "big"][i]];
        if i == 2 {
            args.extend(["--reply-to", &first]);
        }
        let out = author.feed(&args, Some(&url));
        assert_eq!(out, format!("seq {} blob {}\n", i + 1, to_hex(&blob_id(data))));
    }

    // The reader subscribes and syncs from an empty store.
    assert!(reader.feed(&["subscribe", &key], None).starts_with("subscribed"));
    assert_eq!(reader.feed(&["subscriptions"], None), format!("{key}  0 entries held, latest -\n"));
    let synced = reader.feed(&["sync"], Some(&url));
    let lines: Vec<&str> = synced.lines().collect();
    assert_eq!(lines.len(), 4, "{synced}");
    assert!(lines.iter().all(|l| l.contains(&key[..8])));
    let state = |seq: u32| lines.iter().find(|l| l.contains(&format!("#{seq} "))).unwrap().to_string();
    for seq in 1..=3 {
        assert!(state(seq).contains(" local "), "{}", state(seq));
    }
    assert!(state(4).contains(" remote ") && state(4).contains("20000 B"), "{}", state(4));
    assert!(state(2).ends_with("二"));
    assert!((0..3).all(|i| reader.has_blob(&articles[i])));
    assert!(!reader.has_blob(&articles[3]));

    // Listing: newest first, articles held locally marked.
    let listed = reader.feed(&["list"], None);
    let seqs: Vec<&str> = listed.lines().map(|l| l.split_whitespace().nth(3).unwrap()).collect();
    assert_eq!(seqs, ["#4", "#3", "#2", "#1"], "{listed}");
    assert!(listed.lines().next().unwrap().contains(" remote "));
    assert_eq!(reader.feed(&["list", &key], None), listed);

    // Reading: from the store, or fetched on demand.
    assert_eq!(reader.feed(&["read", &key, "2"], None).as_bytes(), articles[1]);
    assert!(!terse(&reader.feed_args(&["read", &key, "4"], None)).status.success(), "no relay, not local");
    assert_eq!(reader.feed(&["read", &key, "4"], Some(&url)).as_bytes(), articles[3]);
    assert!(reader.has_blob(&articles[3]));
    assert!(reader.feed(&["list"], None).lines().all(|l| l.contains(" local ")));

    // Syncing again finds nothing new.
    assert_eq!(reader.feed(&["sync"], Some(&url)), "");

    // Watch mode: a new article arrives by push and is downloaded.
    // Killed when dropped, also when an assertion below fails, so that it
    // never outlives the test.
    struct Watcher(std::process::Child);
    impl Drop for Watcher {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut watcher = Watcher(
        common::terse_command()
            .args(reader.feed_args(&["sync", "--watch"], Some(&url)))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (tx, rx) = mpsc::channel();
    let stdout = watcher.0.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let _ = tx.send((Instant::now(), line.unwrap()));
        }
    });
    // Wait for it to be subscribed on the relay, so that the entry is
    // pushed live rather than sent as backlog.
    let feed_key = terse_net::parse_hex(&key).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while relay.subscribers(&feed_key) == 0 {
        assert!(Instant::now() < deadline, "the watcher never subscribed");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(rx.try_recv().is_err());
    let subscribes = relay.received(protocol::SUBSCRIBE);
    let fresh = author.store.path().join("fresh.txt");
    std::fs::write(&fresh, b"Breaking news.\n").unwrap();
    author.feed(&["publish", fresh.to_str().unwrap(), "--title", "fresh"], Some(&url));
    let line = rx.recv_timeout(Duration::from_secs(60)).expect("pushed entry").1;
    drop(watcher);
    assert!(line.contains("#5 ") && line.contains(" local ") && line.ends_with("fresh"), "{line}");
    // Pushed on the subscription already in place: nobody subscribed again
    // (which would have fetched it as backlog).
    assert_eq!(relay.received(protocol::SUBSCRIBE), subscribes, "the entry came by a new subscription, not by push");
    assert!(reader.has_blob(b"Breaking news.\n"));

    // A forged entry is refused by the relay; one planted in the relay's
    // storage is sent, but rejected by the reader.
    let identity = Identity::load(&author.config.path().join("identity.key")).unwrap();
    let mut forged = Entry::article(identity.feed_key(), 6, 1, b"forged", None, "forged").sign(&identity).unwrap().bytes;
    forged[44] ^= 1; // the blob ID, under the signature
    assert_eq!(raw_publish(&url, &forged), protocol::status::REFUSED);
    let planted = relay_dir.path().join("feeds").join(&key).join("0000000006");
    std::fs::write(&planted, &forged).unwrap();
    assert_eq!(reader.feed(&["sync"], Some(&url)), "");
    assert!(reader.feed(&["list"], None).lines().all(|l| !l.contains("#6 ")));
    assert!(!reader.store.path().join("feeds").join(&key).join("0000000006").exists());

    // Size mismatch: an entry whose blob is not the size it claims. The blob
    // is not downloaded or associated with the entry.
    let data = b"nine byte";
    let liar = author.store.path().join("liar.txt");
    std::fs::write(&liar, data).unwrap();
    terse_ok(&["put", liar.to_str().unwrap(), "--relay", &url, "--store", author.store()]);
    let mut e = Entry::article(identity.feed_key(), 7, 2, data, None, "liar");
    e.size = 4;
    assert_eq!(raw_publish(&url, &e.sign(&identity).unwrap().bytes), protocol::status::DONE);
    let synced = reader.feed(&["sync"], Some(&url));
    assert!(synced.contains("#7 ") && synced.contains(" size-mismatch "), "{synced}");
    assert!(!reader.has_blob(data));
    let out = terse(&reader.feed_args(&["read", &key, "7"], Some(&url)));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not this entry's article"));
}

fn reader_image() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/reader/reader.terse")
}

/// `terse run` with a relay: the feeds are synced (NETWORK 4.3 caught up)
/// before frame 0, so the reader's first timeline already holds them; an
/// entry published while the machine runs reaches it as a feeds-updated
/// event (SPEC §events) and the list refreshes by itself.
#[test]
fn reader_follows_feeds_while_running() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let (author, reader) = (User::new(), User::new());
    let config = author.config.path().to_str().unwrap();
    let key = terse_ok(&["key", "new", "--config", config]).trim().to_string();
    let publish = |name: &str, title: &str| {
        let file = author.store.path().join(name);
        std::fs::write(&file, format!("{title}\n")).unwrap();
        author.feed(&["publish", file.to_str().unwrap(), "--title", title], Some(&url));
    };
    publish("one.txt", "First from the relay");
    reader.feed(&["subscribe", &key], None);

    // From an empty store: synced before the first frame.
    let image = reader_image();
    let screen = terse_ok(&[
        "run", image.to_str().unwrap(), "--frames", "10", "--dump", "--dump-page", "--relay", &url, "--store",
        reader.store(), "--config", reader.config.path().to_str().unwrap(),
    ]);
    assert!(screen.contains("|1 article  ·  1 feed\n"), "{screen}");
    assert!(screen.contains("|[First from the relay]{1}"), "{screen}");
    assert!(screen.contains(&format!("|[{}…{}  ·  ", &key[..6], &key[58..])), "{screen}");

    // While running: a live entry, without any key press.
    let store = BlobStore::open(reader.store.path(), None).unwrap();
    let mut node = terse_net::Node::new(store, Some(terse_net::relay::RelayClient::websocket(&url, Duration::from_secs(10))));
    node.set_subscriptions(terse_net::Subscriptions::load(reader.config.path().join("subscriptions")).unwrap());
    let mut content = terse_net::ContentService::start(node);
    content.wait_caught_up(Duration::from_secs(10)).unwrap();
    let image = terse_core::Image::parse(&std::fs::read(&image).unwrap()).unwrap();
    let mut machine = terse_core::Machine::new(&image);
    let mut headless = terse_net::service::Headless::new(Some(content), terse_core::Script::default());
    for _ in 0..10 {
        headless.frame(&mut machine);
    }
    publish("two.txt", "Second, while running");
    let deadline = Instant::now() + Duration::from_secs(10);
    let page = |machine: &terse_core::Machine| {
        let p = machine.page().unwrap();
        let layout = p.document().map(|d| terse_core::page::reference_layout(d, p.region().w)).unwrap_or_default();
        layout.dump(p)
    };
    while !page(&machine).contains("Second, while running") && Instant::now() < deadline {
        headless.frame(&mut machine);
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..10 {
        headless.frame(&mut machine);
    }
    let screen = page(&machine);
    assert!(screen.contains("|2 articles  ·  1 feed\n"), "{screen}");
    let (second, first) = (screen.find("[Second, while running]{1}"), screen.find("[First from the relay]{2}"));
    assert!(second.is_some() && first.is_some() && second < first, "newest first: {screen}");
}

/// Subscribing with the CLI while the reader runs (NETWORK 4.4, SPEC
/// §content.events): the running host takes in the changed list, follows
/// the feed on its relay and tells the reader, whose timeline then shows
/// the feed's entries, also those published later; unsubscribing takes
/// them out again. No restart.
#[test]
fn subscribing_while_running_takes_effect() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let (author, reader) = (User::new(), User::new());
    let config = author.config.path().to_str().unwrap();
    let key = terse_ok(&["key", "new", "--config", config]).trim().to_string();
    let publish = |name: &str, title: &str| {
        let file = author.store.path().join(name);
        std::fs::write(&file, format!("{title}\n")).unwrap();
        author.feed(&["publish", file.to_str().unwrap(), "--title", title], Some(&url));
    };
    publish("one.txt", "Published before subscribing");

    let store = BlobStore::open(reader.store.path(), None).unwrap();
    let mut node = terse_net::Node::new(store, Some(terse_net::relay::RelayClient::websocket(&url, Duration::from_secs(10))));
    node.set_subscriptions(terse_net::Subscriptions::load(reader.config.path().join("subscriptions")).unwrap());
    let content = terse_net::ContentService::start(node);
    let image = terse_core::Image::parse(&std::fs::read(reader_image()).unwrap()).unwrap();
    let mut machine = terse_core::Machine::new(&image);
    let mut headless = terse_net::service::Headless::new(Some(content), terse_core::Script::default());
    let page = |machine: &terse_core::Machine| {
        let p = machine.page().unwrap();
        let layout = p.document().map(|d| terse_core::page::reference_layout(d, p.region().w)).unwrap_or_default();
        layout.dump(p)
    };
    // Frames until the timeline shows what `done` looks for (failing after
    // a generous bound, which only a broken host reaches).
    let mut until = |machine: &mut terse_core::Machine, what: &str, done: &dyn Fn(&str) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(&page(machine)) {
            assert!(Instant::now() < deadline, "{what}: {}", page(machine));
            headless.frame(machine);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    until(&mut machine, "the empty timeline", &|p| p.contains("No articles yet"));

    reader.feed(&["subscribe", &key], None);
    until(&mut machine, "the feed's entry after subscribing", &|p| p.contains("[Published before subscribing]"));
    publish("two.txt", "Published while subscribed");
    until(&mut machine, "a live entry", &|p| p.contains("[Published while subscribed]"));

    reader.feed(&["unsubscribe", &key], None);
    until(&mut machine, "the timeline without the feed", &|p| !p.contains("Published") && p.contains("No articles yet"));
    let subs = std::fs::read_to_string(reader.config.path().join("subscriptions")).unwrap();
    assert!(!subs.contains(&key), "the host keeps the list as the CLI left it: {subs}");
}

/// `feed sync` also fetches the `terse:` links of the articles it downloads
/// (NETWORK 4.4), unless `--no-prefetch`.
#[test]
fn sync_prefetches_links() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let linked = b"An article only linked to.\n".to_vec();
    BlobStore::open(relay_dir.path(), None).unwrap().put(&linked).unwrap();
    let author = User::new();
    let config = author.config.path().to_str().unwrap();
    let key = terse_ok(&["key", "new", "--config", config]).trim().to_string();
    let file = author.store.path().join("linking.txt");
    std::fs::write(&file, format!("See terse:{}.\n", to_hex(&blob_id(&linked)))).unwrap();
    author.feed(&["publish", file.to_str().unwrap(), "--title", "Linking"], Some(&url));
    for prefetch in [true, false] {
        let reader = User::new();
        reader.feed(&["subscribe", &key], None);
        let mut args = vec!["sync"];
        if !prefetch {
            args.push("--no-prefetch");
        }
        assert_eq!(reader.feed(&args, Some(&url)).lines().count(), 1);
        assert_eq!(reader.has_blob(&linked), prefetch);
    }
}

/// NETWORK 4.2, 6: an entry of a kind this version does not define (9) is
/// valid: the relay stores it and passes it on, and a client keeps it,
/// lists it as `[kind 9: …]`, does not download its blob or open it, and
/// leaves it out of the timeline (which holds kinds 1 and 4 only). Kinds 0
/// and 255 are reserved: the relay refuses them.
#[test]
fn unknown_kind_entries() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let (author, reader) = (User::new(), User::new());
    let config = author.config.path().to_str().unwrap();
    let key = terse_ok(&["key", "new", "--config", config]).trim().to_string();
    let file = author.store.path().join("one.txt");
    std::fs::write(&file, b"An article.\n").unwrap();
    author.feed(&["publish", file.to_str().unwrap(), "--title", "An article"], Some(&url));
    let identity = Identity::load(&author.config.path().join("identity.key")).unwrap();
    let data = b"Something a later revision defines.\n";
    let with_kind = |seq: u32, kind: u8| {
        let mut e = Entry::article(identity.feed_key(), seq, 1_800_000_000, data, None, "Later");
        e.kind = kind;
        e
    };
    for kind in [0, 255] {
        assert!(with_kind(2, kind).sign(&identity).is_err(), "kind {kind} is reserved");
        // Signed as it is, so that only the kind is wrong.
        let mut bytes = with_kind(2, 1).sign(&identity).unwrap().bytes;
        bytes[40] = kind;
        let n = bytes.len() - 64;
        let sig = identity.sign(&[terse_net::entry::DOMAIN, &bytes[..n]].concat());
        bytes[n..].copy_from_slice(&sig);
        assert_eq!(raw_publish(&url, &bytes), protocol::status::REFUSED, "kind {kind}");
    }
    let nine = with_kind(2, 9).sign(&identity).unwrap();
    assert_eq!(raw_publish(&url, &nine.bytes), protocol::status::DONE);
    // Its blob is on the relay too, but a client does not fetch it.
    let blob_file = author.store.path().join("later.bin");
    std::fs::write(&blob_file, data).unwrap();
    terse_ok(&["put", blob_file.to_str().unwrap(), "--relay", &url, "--store", author.store()]);

    reader.feed(&["subscribe", &key], None);
    let synced = reader.feed(&["sync"], Some(&url));
    assert_eq!(synced.lines().count(), 2, "{synced}");
    let line = synced.lines().find(|l| l.contains("#2 ")).unwrap();
    assert!(line.ends_with("[kind 9: Later]") && line.contains(" remote "), "{line}");
    assert!(!reader.has_blob(data), "not downloaded");
    assert!(reader.store.path().join("feeds").join(&key).join("0000000002").exists(), "kept");
    assert!(reader.feed(&["list"], None).contains("[kind 9: Later]"));
    let out = terse(&reader.feed_args(&["read", &key, "2"], Some(&url)));
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("kind 9"));

    // The timeline holds the article only, for a client and for the machine.
    let store = BlobStore::open(reader.store.path(), None).unwrap();
    let mut node = terse_net::Node::new(store, None);
    node.set_subscriptions(terse_net::Subscriptions::load(reader.config.path().join("subscriptions")).unwrap());
    assert_eq!(node.timeline(), [(identity.feed_key(), 1)]);
    let image = reader_image();
    let screen = terse_ok(&[
        "run", image.to_str().unwrap(), "--frames", "10", "--dump", "--dump-page", "--store", reader.store(), "--config",
        reader.config.path().to_str().unwrap(),
    ]);
    assert!(screen.contains("|1 article  ·  1 feed\n"), "{screen}");
}

/// Feed addresses (NETWORK 4.9) on the command line: `feed subscribe`
/// takes a key, an address or a `terse:feed/` link and keeps the hints,
/// `feed subscriptions` shows them, `feed address` prints the user's own,
/// a loopback hint is refused, and `feed sync` says which feeds no relay
/// held.
#[test]
fn feed_addresses() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let reader = User::new();
    let config = reader.config.path().to_str().unwrap();
    let key = terse_ok(&["key", "new", "--config", config]).trim().to_string();
    let other = to_hex(&Identity::from_seed([71; 32]).feed_key());
    let third = to_hex(&Identity::from_seed([72; 32]).feed_key());

    // Malformed addresses are refused whole.
    for bad in [format!("{other}@"), format!("{other}@a,b,c"), format!("{other}@a.example:0"), other.to_uppercase()] {
        let out = terse(&reader.feed_args(&["subscribe", &bad], None));
        assert!(!out.status.success(), "{bad}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("not a feed address"), "{}", String::from_utf8_lossy(&out.stderr));
    }
    assert_eq!(reader.feed(&["subscriptions"], None), "");

    let out = reader.feed(&["subscribe", &format!(" terse:feed/{other}@relay.example:9000,[2001:db8::7] ")], None);
    assert!(out.starts_with(&format!("subscribed to {other}\n")), "{out}");
    assert!(out.contains("looked for on relay.example:9000 and [2001:db8::7]"), "{out}");
    assert_eq!(
        reader.feed(&["subscriptions"], None),
        format!("{other}  0 entries held, latest -  hints relay.example:9000,[2001:db8::7]\n")
    );
    // A loopback hint is accepted but will not be used; it replaces the old ones.
    let out = terse(&reader.feed_args(&["subscribe", &format!("{other}@127.0.0.1:7")], None));
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("its hints are now 127.0.0.1:7"), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("127.0.0.1:7 is a loopback, private or link-local address"), "{out:?}");
    assert!(reader.config.path().join("subscription-hints").exists());

    // The own address: hints from public relays only.
    let out = terse(&reader.feed_args(&["address"], Some(&url)));
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{key}\n"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("only people who use the same relay as you"), "{out:?}");
    let out = reader.feed(&["address", "--relay", "ws://relay.example:7878", "--relay", "ws://10.0.0.1:7878", "--relay", "ws://b.example:9000"], None);
    assert_eq!(out, format!("{key}@relay.example,b.example:9000\n"));

    // Sync: a bare key no relay holds, and the hinted feed (its hint is
    // loopback, so not used), are reported.
    reader.feed(&["subscribe", &third], None);
    let out = terse(&reader.feed_args(&["sync"], Some(&url)));
    assert!(out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("{third}: not found on your relays; if its author publishes elsewhere")), "{err}");
    assert!(err.contains(&format!("{other}: not found on your relays nor on the relays its address names")), "{err}");
    let out = terse(&reader.feed_args(&["sync", "--no-feed-relays"], Some(&url)));
    assert!(String::from_utf8_lossy(&out.stderr).contains("its hints are not used with --no-feed-relays"), "{out:?}");
    // Unsubscribing by address forgets the hints.
    assert!(reader.feed(&["unsubscribe", &format!("{other}@relay.example")], None).starts_with("unsubscribed"));
    assert_eq!(std::fs::read_to_string(reader.config.path().join("subscription-hints")).unwrap(), "");
}
