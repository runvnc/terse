//! A host with several relays (NETWORK 5), over real loopback WebSockets:
//! two relays that do not know each other, one of them stopped and started
//! again mid-session; and feed relays named in a profile.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient};
use terse_net::{BlobStore, Done, Draft, Entry, Identity, Node, Subscriptions, blob_id};
use terse_relay::Relay;

/// A `terse-relay` process, killed when dropped: also when an assertion
/// fails, so that no relay outlives the test (a leftover one keeps the
/// ports and every file descriptor it inherited, a build lock included).
struct RelayProcess(Child);

impl RelayProcess {
    fn stop(mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for RelayProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `terse-relay` as a process on `listen` (port 0: any); returns it and its URL.
fn spawn_relay(store: &std::path::Path, listen: &str) -> (RelayProcess, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_terse-relay"))
        .args(["--listen", listen, "--store", store.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let child = RelayProcess(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    let url = line.trim().strip_prefix("listening on ").unwrap().to_string();
    (child, url)
}

fn in_process(dir: &std::path::Path) -> (String, Arc<Relay>) {
    let relay = Arc::new(Relay::new(BlobStore::open(dir, None).unwrap(), 1024));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let r = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, r));
    (url, relay)
}

/// A relay serving wss:// with the test certificate, trusted in this
/// process through the test authority: a profile names only encrypted
/// relays (NETWORK 4.6).
fn in_process_tls(dir: &std::path::Path) -> (String, Arc<Relay>) {
    let certs = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../terse-testkit/tls");
    terse_net::tls::trust_ca_file(&certs.join("ca.pem")).unwrap();
    let (url, relay) = in_process(dir);
    relay.set_tls(Some(terse_net::tls::server_config(&certs.join("localhost.pem"), &certs.join("localhost.key")).unwrap()));
    (url.replace("ws://", "wss://"), relay)
}

fn poll_until(node: &mut Node, what: &str, mut ok: impl FnMut(&Node) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok(node) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        node.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn two_relays_one_stopped_and_restarted() {
    let (da, db, dn) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (child, url_a) = spawn_relay(da.path(), "127.0.0.1:0");
    let (url_b, b) = in_process(db.path());
    let mut node = Node::open(Some(dn.path().to_path_buf()), &[url_a.clone(), url_b.clone()], DEFAULT_TIMEOUT).unwrap();
    node.set_identity(Identity::from_seed([50; 32]));

    // A blob only B holds is found.
    b.blobs().put(b"only on B").unwrap();
    assert_eq!(node.fetch(blob_id(b"only on B")), Done::Got(b"only on B".to_vec()));
    assert_eq!(node.connected_relays(), [url_a.clone(), url_b.clone()]);
    // Puts and publishes reach both.
    let draft = Draft { data: b"to both".to_vec(), title: "Both".into(), reply_to: None, time: 1 };
    assert!(matches!(node.publish_article(&draft), Done::Published { seq: 1, .. }));
    let at_a = da.path().join(terse_net::to_hex(&blob_id(b"to both")));
    poll_until(&mut node, "A to take the article", |_| std::fs::metadata(&at_a).is_ok());
    assert!(b.blobs().read(&blob_id(b"to both")).unwrap().is_some());

    // A stops: publishing and getting still work, through B.
    child.stop();
    let draft = Draft { data: b"only B now".to_vec(), title: "B".into(), reply_to: None, time: 2 };
    assert!(matches!(node.publish_article(&draft), Done::Published { seq: 2, .. }));
    assert_eq!(b.feeds().seqs(&Identity::from_seed([50; 32]).feed_key()).unwrap(), [1, 2]);
    b.blobs().put(b"later on B").unwrap();
    assert_eq!(node.fetch(blob_id(b"later on B")), Done::Got(b"later on B".to_vec()));
    poll_until(&mut node, "A to be seen as down", |n| n.connected_relays() == [url_b.clone()]);

    // A starts again on the same port: the node reconnects.
    let listen = url_a.strip_prefix("ws://").unwrap();
    let (child, again) = spawn_relay(da.path(), listen);
    assert_eq!(again, url_a);
    poll_until(&mut node, "A to be reconnected", |n| n.connected_relays().len() == 2);
    assert_eq!(node.publish(b"after restart"), Done::Put(blob_id(b"after restart")));
    // A put is done when the first relay takes it (B, in this process,
    // is usually first): A's copy follows.
    let at_a = da.path().join(terse_net::to_hex(&blob_id(b"after restart")));
    poll_until(&mut node, "A to take the put", |_| std::fs::metadata(&at_a).is_ok());
    child.stop();
}

#[test]
fn feed_relays_named_in_a_profile() {
    for on in [true, false] {
        let (da, db, dn) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (url_a, a) = in_process(da.path());
        let (url_b, b) = in_process_tls(db.path());
        // The author's profile (on A) names B; the article is only on B.
        let author = Identity::from_seed([51; 32]);
        let doc = format!("listed: no\nrelay: {url_b}\n").into_bytes();
        a.blobs().put(&doc).unwrap();
        assert_eq!(a.publish(&Entry::profile(author.feed_key(), 1, 1, &doc, "Author").sign(&author).unwrap().bytes), 1);
        let article = b"Held on B only.".to_vec();
        b.blobs().put(&article).unwrap();
        let entry = Entry::article(author.feed_key(), 2, 2, &article, None, "On B").sign(&author).unwrap();
        assert_eq!(b.publish(&entry.bytes), 1);

        let mut node = Node::open(Some(dn.path().to_path_buf()), &[url_a.clone()], DEFAULT_TIMEOUT).unwrap();
        // Loopback relays are ignored in profiles; allowed here for the test.
        node.set_feed_relays_unrestricted(Box::new(|url| RelayClient::websocket(url, DEFAULT_TIMEOUT)));
        node.set_feed_relays(on);
        node.set_subscriptions(Subscriptions::in_memory([author.feed_key()]));
        let want = if on { vec![url_a.clone(), url_b.clone()] } else { vec![url_a.clone()] };
        poll_until(&mut node, "the relays", |n| n.connected_relays() == want);
        if on {
            poll_until(&mut node, "the entry from B", |n| n.feed_store().seqs(&author.feed_key()).unwrap() == [1, 2]);
            poll_until(&mut node, "the article from B", |n| n.store().read(&blob_id(&article)).unwrap().is_some());
        } else {
            for _ in 0..50 {
                node.poll();
            }
            assert_eq!(node.feed_store().seqs(&author.feed_key()).unwrap(), [1]);
            assert_eq!(node.fetch(blob_id(&article)), Done::NotFound);
        }
    }
}
