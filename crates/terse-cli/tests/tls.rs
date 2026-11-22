//! The `terse` command with relays over TLS (`wss://`): a relay typed as a
//! bare host name is saved as `wss://`, publishing reaches a TLS and a
//! plain relay, sync and get work over TLS, and a relay whose certificate
//! cannot be trusted is named with the reason. The relays run in this
//! process with the test certificates (crates/terse-testkit/tls), whose CA
//! the commands trust through TERSE_RELAY_CA.

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use terse_net::{BlobStore, FeedStore, Identity, tls};

fn certs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../terse-testkit/tls")
}

/// A relay in this process: over TLS with the test certificate `cert`, or
/// plain.
fn start_relay(store: &Path, cert: Option<&str>) -> String {
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(store, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let config = cert.map(|c| tls::server_config(&certs().join(format!("{c}.pem")), &certs().join(format!("{c}.key"))).unwrap());
    let scheme = if config.is_some() { "wss" } else { "ws" };
    relay.set_tls(config);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("{scheme}://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

struct User {
    store: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl User {
    fn new() -> Self {
        User { store: tempfile::tempdir().unwrap(), config: tempfile::tempdir().unwrap() }
    }

    fn run(&self, args: &[&str]) -> Output {
        common::terse_command()
            .args(args)
            .args(["--store", self.store.path().to_str().unwrap(), "--config", self.config.path().to_str().unwrap()])
            .env(tls::CA_ENV, certs().join("ca.pem"))
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> (String, String) {
        let out = self.run(args);
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "terse {args:?}: {err}");
        (String::from_utf8(out.stdout).unwrap(), err)
    }
}

#[test]
fn a_bare_host_is_wss() {
    let u = User::new();
    let (_, err) = u.ok(&["relay", "add", "relay.tersenet.org"]);
    assert!(err.contains("taken as wss://relay.tersenet.org"), "{err}");
    u.ok(&["relay", "add", "ws://127.0.0.1:7979"]);
    assert_eq!(u.ok(&["relay", "list"]).0, "wss://relay.tersenet.org\nws://127.0.0.1:7979\n");
    u.ok(&["relay", "remove", "relay.tersenet.org"]);
    assert_eq!(u.ok(&["relay", "list"]).0, "ws://127.0.0.1:7979\n");
    assert!(!u.run(&["relay", "add", "http://relay.example"]).status.success());
}

#[test]
fn publish_sync_and_get_over_tls() {
    let (ds, dp, dw) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let secure = start_relay(ds.path(), Some("localhost"));
    let plain = start_relay(dp.path(), None);
    let wrong = start_relay(dw.path(), Some("wrong"));
    let author = User::new();
    author.ok(&["key", "new"]);
    author.ok(&["relay", "add", &secure]);
    author.ok(&["relay", "add", &plain]);
    let feed = Identity::load(&author.config.path().join("identity.key")).unwrap().feed_key();
    let held = |d: &tempfile::TempDir| FeedStore::beside_blobs(d.path()).seqs(&feed).unwrap();

    let file = author.store.path().join("post.txt");
    std::fs::write(&file, "Over TLS and not.\n").unwrap();
    let (_, err) = author.ok(&["feed", "publish", file.to_str().unwrap(), "--title", "Both"]);
    // In the order they answered.
    let both = [format!("taken by 2 of 2 relays: {secure}, {plain}"), format!("taken by 2 of 2 relays: {plain}, {secure}")];
    assert!(both.iter().any(|b| err.contains(b)), "{err}");
    assert_eq!((held(&ds), held(&dp)), (vec![1], vec![1]));

    // A relay with a certificate for another name: named, with the reason.
    author.ok(&["relay", "add", &wrong]);
    let (_, err) = author.ok(&["feed", "publish", file.to_str().unwrap(), "--title", "Again"]);
    assert!(err.contains("taken by 2 of 3 relays"), "{err}");
    assert!(err.contains(&format!("not on {wrong}: it could not be connected: the relay's certificate is for another name")), "{err}");

    // A reader with only the TLS relay: sync and get.
    let reader = User::new();
    reader.ok(&["relay", "add", &secure]);
    reader.ok(&["feed", "subscribe", &terse_net::to_hex(&feed)]);
    reader.ok(&["feed", "sync"]);
    let (list, _) = reader.ok(&["feed", "list"]);
    assert!(list.contains("Both") && list.contains("Again"), "{list}");
    let id = terse_net::blob_id(b"Over TLS and not.\n");
    let out = reader.store.path().join("got.txt");
    reader.ok(&["get", &terse_net::to_hex(&id), "-o", out.to_str().unwrap()]);
    assert_eq!(std::fs::read(&out).unwrap(), b"Over TLS and not.\n");

    // Without the CA, the same relay is not trusted, and says so.
    let stranger = User::new();
    let out = common::terse_command()
        .args(["get", &terse_net::to_hex(&id), "--relay", &secure, "--store", stranger.store.path().to_str().unwrap()])
        .env_remove(tls::CA_ENV)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("certificate is not trusted"), "{err}");
}
