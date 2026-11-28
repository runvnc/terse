//! The host's own screens (F10, `src/you.rs`) in the window, driven as a
//! user drives them (`--drive`, in a headless compositor the host starts;
//! skipped without weston), with scratch directories and a relay this test
//! starts on a free port: a first run makes the signature with one key,
//! then the profile is published (after a confirmation), an article is
//! chosen in the documents folder, previewed and published, the user's
//! own feed is found by name and followed, and a relay is added to the
//! list. What the screens did is then checked where `terse` would leave
//! it: the key file, the relay's store, the subscription list, the relay
//! list. With the software renderer, and the GPU's where there is one.

#![cfg(target_os = "linux")]

use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use terse_net::{BlobStore, FeedStore, Identity, to_hex};

static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

fn text(o: &Output) -> String {
    format!("status {:?}\n--- stdout\n{}--- stderr\n{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// A relay on a free port of this machine, its store in `dir`.
fn relay(dir: &Path) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(dir, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

/// Every page, as a user goes through them. Each job of the screens'
/// thread is given time (`wait`) before what it did is expected.
const SCRIPT: &str = "\
settle max 300
press f10
wait 30
expect host contains You have no signature yet
shot 01-first-run.png
press down
press enter
wait 60
expect host contains Keep a copy of this file
expect host lacks Working
shot 02-signature.png
press 2
wait 5
press down
type Sam Tester
press down
type I test Tersenet.
press down
press space
press down
press enter
wait 10
expect host contains please confirm
shot 03-profile-confirm.png
press y
wait 150
expect host contains Your profile is published as entry #1
shot 04-profile.png
press 3
press down
press down
press enter
wait 10
expect host contains hello.txt
shot 05-chooser.png
press down
press enter
wait 60
expect host contains Preview: “Hello from the window”
shot 06-preview.png
press p
wait 5
expect host contains Publish “Hello from the window”
press y
wait 150
expect host contains is published as entry #2
shot 07-published.png
press 4
press down
type Sam
press enter
wait 150
expect host contains 1 feed found
shot 08-found.png
press enter
wait 60
expect host contains Feeds you follow (1)
shot 09-follow.png
press up
press 5
press down
type relay.example.org
press enter
wait 60
expect host contains Added wss://relay.example.org
shot 10-relays.png
press escape
wait 10
expect host contains closed
quit
";

fn walk(renderer: &str) {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let docs = d.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("hello.txt"), "# Hello from the window\n\nPublished without a terminal.\n\n- one\n- two\n").unwrap();
    let url = relay(&d.join("relay-store"));
    let script = d.join("script.txt");
    std::fs::write(&script, SCRIPT).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg("--drive")
        .arg(&script)
        .args(["--renderer", renderer, "--size", "1280x960", "--consent", "deny", "--relay", &url])
        .args(if renderer == "gpu" { &["--perf-rule", "off", "--no-shed"][..] } else { &[] })
        .arg("--config")
        .arg(d.join("config"))
        .arg("--data")
        .arg(d.join("data"))
        .arg("--store")
        .arg(d.join("store"))
        .env("XDG_CACHE_HOME", d.join("cache"))
        .env("XDG_CONFIG_HOME", d.join("xdg-config"))
        .env("XDG_DATA_HOME", d.join("xdg-data"))
        .env("XDG_DOCUMENTS_DIR", &docs)
        .env_remove("TERSE_DRIVE_DISPLAY")
        .env_remove("TERSE_DRIVE_NESTED")
        .current_dir(d)
        .output()
        .unwrap();
    if renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter") {
        terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
        return;
    }
    assert!(o.status.success(), "{renderer}: {}", text(&o));
    for n in ["01-first-run", "04-profile", "06-preview", "10-relays"] {
        assert!(d.join(format!("{n}.png")).exists(), "{renderer}: {n}.png: {}", text(&o));
    }
    // The key, made by the screens: where `terse key show` finds it.
    let key = Identity::load(&d.join("config/identity.key")).unwrap().feed_key();
    // The profile and the article reached the relay, signed with it.
    let feeds = FeedStore::beside_blobs(&d.join("relay-store"));
    assert_eq!(feeds.seqs(&key).unwrap(), [1, 2], "{renderer}: {}", text(&o));
    let profile = feeds.latest_profile(&key).unwrap().unwrap();
    assert_eq!(profile.entry.title, "Sam Tester");
    // Followed, and the relay list written.
    let subs = std::fs::read_to_string(d.join("config/subscriptions")).unwrap();
    assert!(subs.contains(&to_hex(&key)), "{subs}");
    let relays = std::fs::read_to_string(d.join("config/relays")).unwrap();
    assert!(relays.contains("wss://relay.example.org"), "{relays}");
    // The secret key was never on the screen.
    let secret = std::fs::read_to_string(d.join("config/identity.key")).unwrap();
    assert!(!String::from_utf8_lossy(&o.stdout).contains(secret.trim()));
}

#[test]
fn every_page_without_the_command_line_software() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if have_weston() {
        walk("software");
    }
}

#[test]
fn every_page_without_the_command_line_gpu() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if have_weston() {
        walk("gpu");
    }
}
