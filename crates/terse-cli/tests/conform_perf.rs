//! programs/conform-perf: the machine host's responsiveness requirements a
//! program can see (SPEC §responsiveness, PERFORMANCE.md). Headless (`terse
//! run`), where every request completes in the next frame, its screen and
//! its state hashes are fixed (`expected-screen.txt`, `expected-hashes.txt`;
//! `UPDATE_GOLDEN=1` rewrites them). The content service it relies on
//! answers held blobs without waiting on the network, checked here with a
//! relay that never answers.

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use terse_core::{Outcome, Request, RequestKind};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{BlobStore, ContentService, Node};

const FRAMES: &str = "140";

fn dir() -> PathBuf {
    common::programs().join("conform-perf")
}

fn path(name: &str) -> String {
    dir().join(name).to_str().unwrap().to_string()
}

fn run(extra: &[&str]) -> String {
    let (store, config, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let out = common::terse_command()
        .args(["run", &path("conform-perf.terse"), "--frames", FRAMES])
        .args(["--store", store.path().to_str().unwrap(), "--config", config.path().to_str().unwrap()])
        .args(["--data", data.path().to_str().unwrap()])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn conform_perf_image_is_up_to_date() {
    let image = std::fs::read(dir().join("conform-perf.terse")).unwrap();
    let parsed = terse_core::Image::parse(&image).unwrap();
    assert_eq!(parsed.title, b"conform-perf");
    let devices: Vec<(u8, u8, bool)> = parsed.devices.iter().map(|d| (d.slot, d.version, d.optional)).collect();
    assert_eq!(devices, [(2, 0, false), (4, 0, false)]);
    if !common::have_tools() {
        return;
    }
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let out = Command::new("sh").arg(path("build.sh")).arg(tmp.path()).env("TERSE", env!("CARGO_BIN_EXE_terse")).output().unwrap();
    assert!(out.status.success(), "build.sh: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let built = std::fs::read(tmp.path().join("conform-perf.terse")).unwrap();
    assert!(built == image, "programs/conform-perf/conform-perf.terse is stale; run programs/conform-perf/build.sh");
}

#[test]
fn headless_screen_and_hashes() {
    let screen = run(&["--dump"]);
    common::golden("conform-perf/expected-screen.txt", &screen);
    assert!(screen.contains("held get (the blob just put): done in 1 frame (at most 2)  PASS"), "{screen}");
    assert!(screen.contains("timeline: done in 1 frame (at most 2)  PASS"), "{screen}");
    assert!(screen.contains("Every requirement met."), "{screen}");
    let hashes = run(&["--hash-every", "10"]);
    assert_eq!(hashes.lines().count(), 14);
    common::golden("conform-perf/expected-hashes.txt", &hashes);
}

/// On the reading profile the put is refused: the held get is not checked,
/// the timeline still is.
#[test]
fn reading_profile() {
    let screen = run(&["--dump", "--profile", "reading"]);
    assert!(screen.contains("held get: not checked"), "{screen}");
    assert!(screen.contains("timeline: done in 1 frame (at most 2)  PASS"), "{screen}");
}

/// spec/content.md §content.submit: a get of a held blob and a timeline
/// request complete without waiting on the network. The only relay
/// accepts the TCP connection and never answers the WebSocket handshake,
/// so anything that waited for it would wait for the connect timeout
/// (10 s); both requests finish long before.
#[test]
fn held_content_does_not_wait_for_a_silent_relay() {
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", silent.local_addr().unwrap());
    let store = tempfile::tempdir().unwrap();
    let id = BlobStore::open(store.path(), None).unwrap().put(b"held here").unwrap();
    let node = Node::open(Some(store.path().to_path_buf()), &[url], DEFAULT_TIMEOUT).unwrap();
    let mut service = ContentService::start(node);
    let start = Instant::now();
    service.submit(vec![
        Request { handle: 1, kind: RequestKind::Get { id, offset: 0, length: 64 } },
        Request { handle: 2, kind: RequestKind::Timeline { offset: 0, length: 64 } },
    ]);
    let mut done = service.wait_finished();
    let took = start.elapsed();
    done.sort_by_key(|c| c.handle);
    assert!(matches!(&done[0].outcome, Outcome::Got { data, .. } if data == b"held here"), "{done:?}");
    assert!(matches!(done[1].outcome, Outcome::Records { .. }), "{done:?}");
    assert!(took < Duration::from_secs(5), "held content waited {took:?} for a relay that never answers");
}
