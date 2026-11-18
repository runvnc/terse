//! End to end over a relay on an ephemeral loopback port (NETWORK 2, 3):
//! `terse put` from one store, `terse get` and `terse run` by ID from others.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use terse_net::BlobStore;

const TERSE: &str = env!("CARGO_BIN_EXE_terse");

fn programs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs")
}

/// Starts an in-process relay; it lives until the test process exits.
fn start_relay(store: &Path) -> String {
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(store, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

fn terse(args: &[&str]) -> Output {
    Command::new(TERSE).args(args).output().unwrap()
}

fn terse_ok(args: &[&str]) -> String {
    let out = terse(args);
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn relay_round_trip_tampering_and_fetch_by_image_id() {
    let dirs: Vec<tempfile::TempDir> = (0..5).map(|_| tempfile::tempdir().unwrap()).collect();
    let [relay_dir, a, b, c, d] = [0, 1, 2, 3, 4].map(|i| dirs[i].path().to_str().unwrap());
    let url = start_relay(Path::new(relay_dir));

    // Put from store A; get into the empty store B.
    let message = programs().join("fetch/message.txt");
    let id = terse_ok(&["put", message.to_str().unwrap(), "--relay", &url, "--store", a]).trim().to_string();
    assert_eq!(id.len(), 64);
    let out = Path::new(b).join("message.out");
    terse_ok(&["get", &id, "-o", out.to_str().unwrap(), "--relay", &url, "--store", b]);
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&message).unwrap());
    assert!(Path::new(b).join(&id).exists(), "verified blob is cached");

    // A blob altered on the relay's disk is rejected by the client.
    let tampered_src = Path::new(a).join("tampered.txt");
    std::fs::write(&tampered_src, b"original bytes").unwrap();
    let tampered = terse_ok(&["put", tampered_src.to_str().unwrap(), "--relay", &url, "--store", a]).trim().to_string();
    std::fs::write(Path::new(relay_dir).join(&tampered), b"altered bytes").unwrap();
    let res = terse(&["get", &tampered, "--relay", &url, "--store", c]);
    assert!(!res.status.success());
    assert!(String::from_utf8_lossy(&res.stderr).contains("not found"), "{}", String::from_utf8_lossy(&res.stderr));
    assert!(!Path::new(c).join(&tampered).exists(), "nothing stored");

    // programs/fetch by image ID from an empty store: the image and its
    // message both come from the relay. The screen matches the golden one.
    let image = programs().join("fetch/fetch.terse");
    let image_id = terse_ok(&["put", image.to_str().unwrap(), "--relay", &url, "--store", a]).trim().to_string();
    let screen = terse_ok(&["run", &image_id, "--relay", &url, "--store", d, "--frames", "10", "--dump"]);
    let expected = std::fs::read_to_string(programs().join("fetch/expected-screen.txt")).unwrap();
    assert_eq!(screen, expected);
    // The program's put reached the relay.
    let put_data: Vec<u8> = (0..=255).collect();
    assert!(Path::new(relay_dir).join(terse_net::to_hex(&terse_net::blob_id(&put_data))).exists());

    // Unreachable relay: the content request fails rather than hanging.
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("ws://{}", l.local_addr().unwrap())
    };
    let e = tempfile::tempdir().unwrap();
    let screen = terse_ok(&[
        "run", image.to_str().unwrap(), "--relay", &dead, "--store", e.path().to_str().unwrap(), "--frames", "10", "--dump",
    ]);
    assert!(screen.contains("fetching… status 03 failed"), "{screen}");
    assert!(screen.contains("Putting 256 bytes built in RAM… status 03 failed"), "{screen}");
}
