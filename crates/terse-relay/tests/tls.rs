//! Relays over TLS (`wss://`, NETWORK 3): a relay given a certificate
//! (`--tls-cert`) from a private test CA (crates/terse-testkit/tls), which
//! the clients here trust; certificates that must be refused, and a client
//! speaking the other way from the relay, each with its own words.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient, RelayReply, WsTransport};
use terse_net::{BlobStore, Delivery, Done, Draft, FeedEvent, Identity, Node, blob_id, tls};
use terse_relay::{DEFAULT_FEED_CAP, Policy, Relay};

fn certs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../terse-testkit/tls")
}

/// Trusts the test CA in this process (every test here does).
fn trust_test_ca() {
    tls::trust_ca_file(&certs().join("ca.pem")).unwrap();
}

fn server(name: &str) -> Arc<tls::ServerConfig> {
    tls::server_config(&certs().join(format!("{name}.pem")), &certs().join(format!("{name}.key"))).unwrap()
}

/// A relay in this process; `cert`: serves wss:// with that test
/// certificate. Its URL names 127.0.0.1 (the test certificate is for it).
fn start(dir: &std::path::Path, policy: Policy, cert: Option<&str>) -> (String, Arc<Relay>) {
    let relay = Arc::new(Relay::with_policy(BlobStore::open(dir, None).unwrap(), policy).unwrap());
    relay.set_tls(cert.map(server));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let scheme = if cert.is_some() { "wss" } else { "ws" };
    let url = format!("{scheme}://{}", listener.local_addr().unwrap());
    let served = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, served));
    (url, relay)
}

fn node(dir: &std::path::Path, urls: &[&str]) -> Node {
    let mut node = Node::new(BlobStore::open(dir, None).unwrap(), None);
    for url in urls {
        node.add_relay(url, RelayClient::websocket(url, DEFAULT_TIMEOUT));
    }
    node
}

fn connect_error(url: &str) -> String {
    match WsTransport::connect(url, Duration::from_secs(5)) {
        Ok(_) => panic!("{url}: connected"),
        Err(e) => e.to_string(),
    }
}

/// Put, get, publish, subscribe with entries pushed live, over wss://,
/// by IP address and by name.
#[test]
fn everything_works_over_tls() {
    trust_test_ca();
    let (rd, ad, bd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (url, relay) = start(rd.path(), Policy::unlimited(DEFAULT_FEED_CAP), Some("localhost"));
    let by_name = url.replace("127.0.0.1", "localhost");

    let author = Identity::from_seed([31; 32]);
    let mut a = node(ad.path(), &[&url]);
    a.set_identity(author.clone());
    let blob = b"a blob carried over TLS".to_vec();
    assert_eq!(a.publish(&blob), Done::Put(blob_id(&blob)));
    let draft = Draft { data: b"Over TLS.\n\nHello.\n".to_vec(), title: "Over TLS".into(), reply_to: None, time: 1_800_000_000 };
    assert!(matches!(a.publish_article(&draft), Done::Published { seq: 1, .. }));
    assert!(relay.blobs().read(&blob_id(&blob)).unwrap().is_some());

    // Another node, through the name: the blob, and the feed live.
    let mut b = node(bd.path(), &[&by_name]);
    assert_eq!(b.fetch_bytes(blob_id(&blob)).unwrap(), blob);
    b.subscribe(author.feed_key()).unwrap();
    let draft2 = Draft { data: b"Second.\n".to_vec(), title: "Second".into(), reply_to: None, time: 1_800_000_001 };
    let mut seqs = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut published = false;
    while seqs.len() < 2 {
        assert!(Instant::now() < deadline, "entries over TLS: {seqs:?}");
        b.poll();
        for e in b.take_feed_events() {
            if let FeedEvent::Entry { entry, .. } = e {
                seqs.push(entry.seq);
            }
        }
        // The second is published once the first has arrived: pushed live.
        if seqs == [1] && !published {
            assert!(matches!(a.publish_article(&draft2), Done::Published { seq: 2, .. }));
            published = true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(seqs, [1, 2]);
}

/// The relay's keepalive pings (NETWORK 3) are answered over TLS: a
/// connection silent for many ping periods still works.
#[test]
fn pings_over_tls() {
    trust_test_ca();
    let rd = tempfile::tempdir().unwrap();
    let mut policy = Policy::unlimited(DEFAULT_FEED_CAP);
    policy.ping_every = Some(Duration::from_millis(30));
    let (url, relay) = start(rd.path(), policy, Some("localhost"));
    relay.blobs().put(b"still here").unwrap();
    let mut client = RelayClient::websocket(&url, DEFAULT_TIMEOUT);
    client.connect().unwrap();
    let generation = client.generation();
    let quiet = Instant::now();
    while quiet.elapsed() < Duration::from_millis(600) {
        assert!(client.poll().is_empty());
        std::thread::sleep(Duration::from_millis(10));
    }
    let req = client.get(&blob_id(b"still here"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let reply = loop {
        if let Some((_, r)) = client.poll().into_iter().find(|(r, _)| *r == req) {
            break r;
        }
        assert!(Instant::now() < deadline);
    };
    assert_eq!(reply, RelayReply::Got(b"still here".to_vec()));
    assert_eq!(client.generation(), generation, "the connection was not lost");
}

/// A put waits for every relay (`set_deliver_to_all`), one over TLS and
/// one plain, and one that cannot be reached is named with the reason.
#[test]
fn delivery_to_a_tls_and_a_plain_relay() {
    trust_test_ca();
    let (d1, d2, d3, nd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (secure, r1) = start(d1.path(), Policy::unlimited(DEFAULT_FEED_CAP), Some("localhost"));
    let (plain, r2) = start(d2.path(), Policy::unlimited(DEFAULT_FEED_CAP), None);
    let (wrong, _r3) = start(d3.path(), Policy::unlimited(DEFAULT_FEED_CAP), Some("wrong"));
    let mut n = node(nd.path(), &[&secure, &plain, &wrong]);
    n.set_deliver_to_all(true);
    n.set_identity(Identity::from_seed([32; 32]));
    let draft = Draft { data: b"To all.\n".to_vec(), title: "All".into(), reply_to: None, time: 1_800_000_000 };
    assert!(matches!(n.publish_article(&draft), Done::Published { seq: 1, .. }));
    let delivery: std::collections::HashMap<String, Delivery> = n.take_delivery().into_iter().collect();
    assert_eq!(delivery[&secure], Delivery::Accepted);
    assert_eq!(delivery[&plain], Delivery::Accepted);
    assert_eq!(delivery[&wrong], Delivery::NotConnected);
    let why = n.relay_failure(&wrong).unwrap();
    assert!(why.contains("certificate is for another name"), "{why}");
    let feed = Identity::from_seed([32; 32]).feed_key();
    assert_eq!(r1.feeds().seqs(&feed).unwrap(), [1]);
    assert_eq!(r2.feeds().seqs(&feed).unwrap(), [1]);
}

/// Refusals in words: a certificate for another name, one from no trusted
/// authority, a TLS client at a plain relay and a plain one at a TLS relay.
#[test]
fn refusals_say_why() {
    trust_test_ca();
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let p = || Policy::unlimited(DEFAULT_FEED_CAP);
    let (wrong, _a) = start(dirs[0].path(), p(), Some("wrong"));
    let (untrusted, _b) = start(dirs[1].path(), p(), Some("untrusted"));
    let (plain, _c) = start(dirs[2].path(), p(), None);
    let (secure, _d) = start(dirs[3].path(), p(), Some("localhost"));
    let e = connect_error(&wrong);
    assert!(e.contains("the relay's certificate is for another name"), "{e}");
    let e = connect_error(&untrusted);
    assert!(e.contains("the relay's certificate is not trusted"), "{e}");
    let e = connect_error(&plain.replace("ws://", "wss://"));
    assert!(e.contains("this relay does not speak TLS (try ws://)"), "{e}");
    let e = connect_error(&secure.replace("wss://", "ws://"));
    assert!(e.contains("this relay requires TLS (try wss://)"), "{e}");
    // The right way round, each works.
    assert!(WsTransport::connect(&plain, DEFAULT_TIMEOUT).is_ok());
    assert!(WsTransport::connect(&secure, DEFAULT_TIMEOUT).is_ok());
}

/// A relay mirrors a peer over wss:// (`--peer wss://...`).
#[test]
fn peer_link_over_tls() {
    trust_test_ca();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a_url, a) = start(da.path(), Policy::unlimited(DEFAULT_FEED_CAP), Some("localhost"));
    let (_b_url, b) = start(db.path(), Policy::unlimited(DEFAULT_FEED_CAP), None);
    terse_relay::connect_peers(b.clone(), vec![a_url.clone()]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.peer_count() == 0 {
        assert!(Instant::now() < deadline, "no peer link over TLS");
        std::thread::sleep(Duration::from_millis(10));
    }
    // A blob put on A reaches B.
    let mut client = RelayClient::websocket(&a_url, DEFAULT_TIMEOUT);
    client.connect().unwrap();
    client.put(b"mirrored over TLS");
    while Instant::now() < deadline && b.blobs().read(&blob_id(b"mirrored over TLS")).unwrap().is_none() {
        client.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(b.blobs().read(&blob_id(b"mirrored over TLS")).unwrap().is_some());
}

/// The program: `--tls-cert`/`--tls-key` print a wss:// address, and
/// SIGHUP reads the certificate again (a renewal): an untrusted one is
/// replaced by a trusted one without a restart.
#[cfg(unix)]
#[test]
fn tls_options_and_reload() {
    trust_test_ca();
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    std::fs::copy(certs().join("untrusted.pem"), &cert).unwrap();
    std::fs::copy(certs().join("untrusted.key"), &key).unwrap();
    let only_cert = Command::new(env!("CARGO_BIN_EXE_terse-relay"))
        .args(["--listen", "127.0.0.1:0", "--store"])
        .arg(dir.path().join("store"))
        .arg("--tls-cert")
        .arg(&cert)
        .output()
        .unwrap();
    assert_eq!(only_cert.status.code(), Some(2));
    let mut child = Command::new(env!("CARGO_BIN_EXE_terse-relay"))
        .args(["--listen", "127.0.0.1:0", "--store"])
        .arg(dir.path().join("store"))
        .arg("--tls-cert")
        .arg(&cert)
        .arg("--tls-key")
        .arg(&key)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = line.trim().strip_prefix("listening on ").unwrap().to_string();
    assert!(url.starts_with("wss://127.0.0.1:"), "{url}");
    let e = connect_error(&url);
    assert!(e.contains("not trusted"), "{e}");
    std::fs::copy(certs().join("localhost.pem"), &cert).unwrap();
    std::fs::copy(certs().join("localhost.key"), &key).unwrap();
    // SAFETY: a plain kill(2) on our own child.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGHUP) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while WsTransport::connect(&url, DEFAULT_TIMEOUT).is_err() {
        assert!(Instant::now() < deadline, "the renewed certificate was not taken");
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    child.wait().unwrap();
}
