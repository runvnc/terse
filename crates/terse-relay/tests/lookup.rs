//! Lookup (NETWORK 4.8): a feed's current program of a name, for any feed,
//! listed or not; rate-limited with the finds; and a host that starts a
//! program it does not hold (SPEC §machines.calls `start_program`) looks it up first.
//! Documents (kind 4) are found by entry search (scope 2).

use std::net::TcpListener;
use std::sync::Arc;

use terse_core::{Outcome, ProgramId};
use terse_net::protocol::SCOPE_ENTRIES;
use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient, RelayReply};
use terse_net::{BlobStore, ContentService, Entry, Identity, Node, blob_id, entry};
use terse_relay::Relay;

fn relay(dir: &std::path::Path) -> Relay {
    Relay::new(BlobStore::open(dir, None).unwrap(), 1024)
}

fn serve(r: Arc<Relay>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, r));
    url
}

fn program(r: &Relay, id: &Identity, seq: u32, image: &[u8], name: &str) {
    r.blobs().put(image).unwrap();
    let e = Entry::program(id.feed_key(), seq, seq, image, name).sign(id).unwrap();
    assert_eq!(r.publish(&e.bytes), 1);
}

fn ask(client: &mut RelayClient, reqs: Vec<u32>) -> Vec<RelayReply> {
    let mut replies = Vec::new();
    while replies.len() < reqs.len() {
        replies.extend(client.poll());
    }
    replies.sort_by_key(|r| reqs.iter().position(|q| *q == r.0));
    replies.into_iter().map(|r| r.1).collect()
}

/// Found (the highest seq of that name), not found, a feed with no
/// profile at all (unlisted) still answered, kinds other than 3 and bad
/// names refused, and more than 4 lookups and finds a second refused.
#[test]
fn lookup_answers_and_rate_limit() {
    let dir = tempfile::tempdir().unwrap();
    let r = Arc::new(relay(dir.path()));
    let author = Identity::from_seed([1; 32]);
    program(&r, &author, 1, b"v1", "tool");
    program(&r, &author, 2, b"other", "other");
    program(&r, &author, 3, b"v2", "tool");
    let url = serve(r.clone());
    let mut client = RelayClient::websocket(&url, DEFAULT_TIMEOUT);
    client.connect().unwrap();
    let key = author.feed_key();
    let reqs = vec![client.lookup(&key, 3, b"tool"), client.lookup(&key, 3, b"nope"), client.lookup(&[9; 32], 3, b"tool")];
    let got = ask(&mut client, reqs);
    let RelayReply::LookedUp(bytes) = &got[0] else { panic!("{got:?}") };
    let e = entry::decode(bytes).unwrap().entry;
    assert_eq!((e.seq, e.blob), (3, blob_id(b"v2")));
    assert_eq!(got[1..], [RelayReply::NotFound, RelayReply::NotFound]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let reqs = vec![client.lookup(&key, 1, b"tool"), client.lookup(&key, 3, b" tool")];
    let refused = RelayReply::Refused(terse_net::protocol::Refusal::Unspecified);
    assert_eq!(ask(&mut client, reqs), [refused.clone(), refused]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut reqs: Vec<u32> = (0..3).map(|_| client.lookup(&key, 3, b"other")).collect();
    reqs.push(client.find(SCOPE_ENTRIES, b"x"));
    reqs.push(client.lookup(&key, 3, b"other"));
    let got = ask(&mut client, reqs);
    assert!(matches!(got[..3], [RelayReply::LookedUp(_), RelayReply::LookedUp(_), RelayReply::LookedUp(_)]), "{got:?}");
    assert_eq!(got[3], RelayReply::Found(Vec::new()));
    // The fifth in a second: refused, try again within a second (NETWORK 3.1).
    let RelayReply::Refused(terse_net::protocol::Refusal::TryLater(wait)) = got[4] else { panic!("{got:?}") };
    assert!(wait <= std::time::Duration::from_secs(1));
}

/// SPEC §machines.calls: `start_program` for a feed the host does not follow and a
/// program it does not hold: looked up on the relay, then its image
/// fetched; not found after that if the relay has none.
#[test]
fn start_program_looks_up() {
    let (dr, dh) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let r = Arc::new(relay(dr.path()));
    let author = Identity::from_seed([2; 32]);
    let image = terse_core::Image::build("tool", 1, &[], &[0x6F, 0, 0, 0]);
    program(&r, &author, 1, &image, "Tool");
    let url = serve(r);
    let mut node = Node::new(BlobStore::open(dh.path(), None).unwrap(), None);
    node.add_relay(&url, RelayClient::websocket(&url, DEFAULT_TIMEOUT));
    let mut service = ContentService::start(node);
    let mut fetch = |name: &str| {
        service.fetch_program(5, &ProgramId { publisher: author.feed_key(), name: name.into() }, &[]);
        let done = service.wait_finished_tagged();
        assert_eq!((done.len(), done[0].0, done[0].1.handle), (1, 5, 0));
        done[0].1.outcome.clone()
    };
    assert_eq!(fetch("Tool"), Outcome::Got { size: image.len() as u32, data: image.clone() });
    assert_eq!(fetch("Missing"), Outcome::NotFound);
    drop(service);
    // The entry and the image are now held.
    let node = Node::new(BlobStore::open(dh.path(), None).unwrap(), None);
    assert_eq!(node.current_program(&author.feed_key(), b"Tool").map(|e| e.blob), Some(blob_id(&image)));
}

/// Documents (kind 4) are found with the articles (scope 2).
#[test]
fn documents_are_found_with_articles() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path());
    let author = Identity::from_seed([3; 32]);
    let doc = b"listed: yes\n";
    r.blobs().put(doc).unwrap();
    let p = Entry::profile(author.feed_key(), 1, 1, doc, "Drawer").sign(&author).unwrap();
    assert_eq!(r.publish(&p.bytes), 1);
    let d = Entry::document(author.feed_key(), 2, 2, b"TDOC...", "Sketch of a cat").sign(&author).unwrap();
    assert_eq!(r.publish(&d.bytes), 1);
    let a = Entry::article(author.feed_key(), 3, 3, b"cat", None, "A cat").sign(&author).unwrap();
    assert_eq!(r.publish(&a.bytes), 1);
    let found: Vec<(u8, String)> =
        r.find(SCOPE_ENTRIES, b"cat").unwrap().iter().map(|b| entry::decode(b).unwrap().entry).map(|e| (e.kind, e.title)).collect();
    assert_eq!(found, [(1, "A cat".to_string()), (4, "Sketch of a cat".to_string())]);
}
