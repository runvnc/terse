//! The relay over real loopback WebSockets (NETWORK 3).

use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use terse_net::blob::MAX_BLOB;
use terse_net::protocol::{self, GET_REPLY, PUT_REPLY, status};
use terse_net::{BlobStore, blob_id};
use terse_relay::{DEFAULT_FEED_CAP, Relay};
use tungstenite::{Message, WebSocket};

/// Starts a relay on an ephemeral loopback port; it lives until the test
/// process exits.
fn start(limit: Option<u64>) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(Relay::new(BlobStore::open(dir.path(), limit).unwrap(), DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    (url, dir)
}

fn connect(url: &str) -> WebSocket<TcpStream> {
    let stream = TcpStream::connect(url.strip_prefix("ws://").unwrap()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    tungstenite::client::client_with_config(url, stream, Some(terse_net::relay::websocket_config())).unwrap().0
}

fn ask(ws: &mut WebSocket<TcpStream>, message: Vec<u8>) -> Vec<u8> {
    ws.send(Message::binary(message)).unwrap();
    loop {
        match ws.read().unwrap() {
            Message::Binary(b) => return b.to_vec(),
            Message::Ping(_) | Message::Pong(_) => {}
            m => panic!("unexpected {m:?}"),
        }
    }
}

/// True if the relay closes the connection after `message`.
fn closes_after(url: &str, message: Message) -> bool {
    let mut ws = connect(url);
    ws.send(message).unwrap();
    loop {
        match ws.read() {
            Ok(Message::Close(_)) => {}
            Ok(m) => panic!("unexpected {m:?}"),
            Err(tungstenite::Error::ConnectionClosed) => return true,
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
fn put_then_get_from_another_client() {
    let (url, dir) = start(None);
    let mut a = connect(&url);
    let id = blob_id(b"shared blob");
    assert_eq!(ask(&mut a, protocol::encode_put(7, b"shared blob")), protocol::encode_put_reply(7, status::DONE, &id));
    assert!(dir.path().join(terse_net::to_hex(&id)).exists());
    let mut b = connect(&url);
    assert_eq!(ask(&mut b, protocol::encode_get(9, &id)), protocol::encode_get_reply(9, status::DONE, b"shared blob"));
    assert_eq!(ask(&mut b, protocol::encode_get(10, &[0; 32])), [GET_REPLY, 10, 0, 0, 0, status::NOT_FOUND]);
}

#[test]
fn refuses_empty_oversize_and_over_limit_blobs() {
    let (url, _dir) = start(Some(MAX_BLOB as u64 + 10));
    let mut ws = connect(&url);
    let refused = |req: u8| vec![PUT_REPLY, req, 0, 0, 0, status::REFUSED];
    assert_eq!(ask(&mut ws, protocol::encode_put(1, b"")), refused(1));
    assert_eq!(ask(&mut ws, protocol::encode_put(2, &vec![1; MAX_BLOB + 1])), refused(2));
    let full = vec![2; MAX_BLOB];
    assert_eq!(ask(&mut ws, protocol::encode_put(3, &full)), protocol::encode_put_reply(3, status::DONE, &blob_id(&full)));
    assert_eq!(ask(&mut ws, protocol::encode_put(4, &[3; 11])), vec![PUT_REPLY, 4, 0, 0, 0, status::RELAY_FULL], "store limit (NETWORK 3.1)");
    assert_eq!(ask(&mut ws, protocol::encode_put(5, &[3; 10]))[5], status::DONE, "exactly at the limit");
    assert_eq!(ask(&mut ws, protocol::encode_put(6, &full))[5], status::DONE, "already stored");
}

#[test]
fn malformed_requests() {
    let (url, _dir) = start(None);
    let mut ws = connect(&url);
    // Readable type and req, wrong length: refused.
    let short_get = protocol::encode_get(5, &[0; 32])[..30].to_vec();
    assert_eq!(ask(&mut ws, short_get), [GET_REPLY, 5, 0, 0, 0, status::REFUSED]);
    let mut long_get = protocol::encode_get(6, &[0; 32]);
    long_get.push(0);
    assert_eq!(ask(&mut ws, long_get), [GET_REPLY, 6, 0, 0, 0, status::REFUSED]);
    // Unparseable: the connection is closed.
    assert!(closes_after(&url, Message::binary(vec![0x01, 0, 0])));
    assert!(closes_after(&url, Message::text("get")));
    // A type the relay does not know is ignored (NETWORK 3), so is a reply
    // type from a client that is not a peer; the connection stays usable.
    ws.send(Message::binary(vec![0x55, 0, 0, 0, 0, 0])).unwrap();
    ws.send(Message::binary(vec![0x09])).unwrap();
    ws.send(Message::binary(protocol::encode_get_reply(1, status::DONE, b"x"))).unwrap();
    assert_eq!(ask(&mut ws, protocol::encode_get(7, &[0; 32])), [GET_REPLY, 7, 0, 0, 0, status::NOT_FOUND]);
    // Find: a bad scope, an empty or long query, a cut message: refused.
    let refused = |req: u8| vec![protocol::FIND_REPLY, req, 0, 0, 0, status::REFUSED, 0];
    assert_eq!(ask(&mut ws, protocol::encode_find(1, 4, b"a", None)), refused(1));
    assert_eq!(ask(&mut ws, protocol::encode_find(2, 1, b"", None)), refused(2));
    assert_eq!(ask(&mut ws, protocol::encode_find(3, 2, &[b'a'; 49], None)), refused(3));
    assert_eq!(ask(&mut ws, protocol::encode_find(4, 1, &[0xFF], None)), refused(4), "not UTF-8");
    assert_eq!(ask(&mut ws, protocol::encode_find(5, 1, b"ab", None)[..7].to_vec()), refused(5));
    // A find carried out is done (status 1) also when nothing matches,
    // with no results (NETWORK 4.7).
    assert_eq!(ask(&mut ws, protocol::encode_find(6, 1, b"ab", None)), [protocol::FIND_REPLY, 6, 0, 0, 0, status::DONE, 0]);
    // Paging (NETWORK 4.7): a find with `c` 0 is answered with `c`; a
    // cursor the relay did not make, or cut short, is refused with c 0.
    assert_eq!(ask(&mut ws, protocol::encode_find(7, 1, b"ab", Some(&[]))), [protocol::FIND_REPLY, 7, 0, 0, 0, status::DONE, 0, 0]);
    assert_eq!(
        ask(&mut ws, protocol::encode_find(8, 1, b"ab", Some(b"cursor"))),
        [protocol::FIND_REPLY, 8, 0, 0, 0, status::REFUSED, 0, 0]
    );
    assert_eq!(ask(&mut ws, protocol::encode_find(9, 1, b"ab", Some(b"cursor"))[..13].to_vec()), refused(9), "cut short");
}
