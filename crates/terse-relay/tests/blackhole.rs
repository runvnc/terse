//! A relay that does not answer never holds up the others (NETWORK 5):
//! connects happen in the background, and requests go to the relays that
//! are connected.

use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terse_net::protocol::SCOPE_ENTRIES;
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{BlobStore, Done, Entry, Identity, Node, blob_id};
use terse_relay::Relay;

/// A listener that accepts TCP connections and never says anything; the
/// accepted streams are kept open.
fn black_hole() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let held: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            held.lock().unwrap().push(stream);
        }
    });
    url
}

fn good_relay(dir: &std::path::Path) -> (String, Arc<Relay>) {
    let relay = Arc::new(Relay::new(BlobStore::open(dir, None).unwrap(), 1024));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let r = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, r));
    (url, relay)
}

/// Get, put and find with the silent relay listed first: each is answered
/// by the good relay well within a second, while the other is still
/// connecting.
fn served_beside(silent: &str) {
    let (dr, dn) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (good, relay) = good_relay(dr.path());
    relay.blobs().put(b"on the good relay").unwrap();
    let author = Identity::from_seed([9; 32]);
    let doc = b"listed: yes\n";
    relay.blobs().put(doc).unwrap();
    relay.publish(&Entry::profile(author.feed_key(), 1, 1, doc, "Author").sign(&author).unwrap().bytes);
    relay.publish(&Entry::article(author.feed_key(), 2, 2, b"x", None, "Some news").sign(&author).unwrap().bytes);

    let mut node = Node::open(Some(dn.path().to_path_buf()), &[silent.to_string(), good.clone()], DEFAULT_TIMEOUT).unwrap();
    let start = Instant::now();
    assert_eq!(node.fetch(blob_id(b"on the good relay")), Done::Got(b"on the good relay".to_vec()));
    assert!(start.elapsed() < Duration::from_millis(900), "get took {:?}", start.elapsed());
    assert_eq!(node.connected_relays(), [good.clone()]);

    let start = Instant::now();
    assert_eq!(node.publish(b"put while one relay hangs"), Done::Put(blob_id(b"put while one relay hangs")));
    assert!(start.elapsed() < Duration::from_millis(500), "put took {:?}", start.elapsed());
    assert!(relay.blobs().read(&blob_id(b"put while one relay hangs")).unwrap().is_some());

    let start = Instant::now();
    let Done::Found(found) = node.find(SCOPE_ENTRIES, b"news") else { panic!("find failed") };
    assert_eq!(found.len(), 1);
    assert!(start.elapsed() < Duration::from_millis(500), "find took {:?}", start.elapsed());
}

#[test]
fn a_relay_that_accepts_and_never_answers() {
    let silent = black_hole();
    served_beside(&silent);
}

/// An address that is not routed (TEST-NET-1): the connect either hangs
/// until its timeout or fails at once, depending on the network; either
/// way the good relay serves.
#[test]
fn an_unroutable_relay() {
    served_beside("ws://192.0.2.1:7");
}

/// With only the silent relay, a request waits for its connect, which
/// fails at the timeout; the request then fails.
#[test]
fn only_a_silent_relay() {
    let dn = tempfile::tempdir().unwrap();
    let silent = black_hole();
    let mut node = Node::open(Some(dn.path().to_path_buf()), &[silent], Duration::from_millis(300)).unwrap();
    let start = Instant::now();
    assert_eq!(node.fetch(blob_id(b"anything")), Done::Failed);
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(250) && took < Duration::from_secs(3), "{took:?}");
}
