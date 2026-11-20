//! `terse run --record` and `--replay` (STATE.md 5): a recorded run replays
//! to the same state hash after every frame from the recording and the
//! image alone, with no network, store or data directory: the reader
//! fetching an article over a relay while it runs, and sparks played with
//! a controller. A replay refuses a recording of another image and one that
//! does not match what the machine does.

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use terse_core::record::Recording;
use terse_core::{Event, Outcome};
use terse_net::{BlobStore, blob_id, to_hex};

fn programs() -> PathBuf {
    common::programs()
}

fn image(name: &str) -> String {
    programs().join(name).join(format!("{name}.terse")).to_str().unwrap().to_string()
}

/// An in-process relay on an ephemeral port; it lives until the test
/// process exits.
fn start_relay(store: &Path) -> String {
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(store, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

fn terse(args: &[&str]) -> Output {
    common::terse_command().args(args).output().unwrap()
}

fn terse_ok(args: &[&str]) -> String {
    let out = terse(args);
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// `terse run <image> --replay <rec> <extra>` with nothing else: the
/// default config, cache and data directories are in a new, empty
/// directory, which must still be empty afterwards, and no relay.
fn replay(image: &str, rec: &Path, extra: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = common::terse_command();
    for var in ["XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"] {
        cmd.env(var, home.path().join(var));
    }
    let out = cmd.current_dir(home.path()).args(["run", image, "--replay", rec.to_str().unwrap()]).args(extra).output().unwrap();
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0, "a replay writes no directory");
    out
}

fn replay_ok(image: &str, rec: &Path, extra: &[&str]) -> String {
    let out = replay(image, rec, extra);
    assert!(out.status.success(), "replay: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// Every frame of a recording of `image` (after its grants, one flag per
/// optional device of the image).
fn frames(rec: &[u8], image: &str) -> Vec<terse_core::record::Frame> {
    let image = terse_core::Image::parse(&std::fs::read(image).unwrap()).unwrap();
    let mut r = Recording::parse(rec).unwrap();
    r.grants(image.optional().count()).unwrap();
    std::iter::from_fn(|| r.next_frame()).map(|f| f.unwrap().0).collect()
}

/// An article over the automatic download size (16 KiB), so that the
/// reader's gets go to the relay while it runs.
fn big_article() -> String {
    let mut s = String::from("A long article from the relay\n\n");
    let mut i = 0;
    while s.len() < 30_000 {
        s.push_str(&format!("Paragraph {i}: fetched while the reader runs, recorded, and replayed without a network.\n\n"));
        i += 1;
    }
    s
}

#[test]
fn reader_with_a_relay_replays_without_one() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let dirs: Vec<tempfile::TempDir> = (0..6).map(|_| tempfile::tempdir().unwrap()).collect();
    let [author_store, author_config, store, config, data, work] = [0, 1, 2, 3, 4, 5].map(|i| dirs[i].path().to_str().unwrap());

    // An author publishes a short article and then a long one to the relay.
    let key = terse_ok(&["key", "new", "--config", author_config]).trim().to_string();
    let big = big_article();
    for (name, title, text) in [("short.txt", "A short one", "Short.\n".to_string()), ("big.txt", "A long one", big.clone())] {
        let file = Path::new(work).join(name);
        std::fs::write(&file, text).unwrap();
        terse_ok(&[
            "feed", "publish", file.to_str().unwrap(), "--title", title, "--store", author_store, "--config", author_config, "--relay", &url,
        ]);
    }
    terse_ok(&["feed", "subscribe", &key, "--store", store, "--config", config]);

    // The reader opens the newest entry: its article is not held, so it is
    // fetched from the relay during the run.
    let events = Path::new(work).join("events");
    std::fs::write(&events, "20 press enter\n80 press down\n90 press down\n").unwrap();
    let rec = Path::new(work).join("reader.rec");
    let reader = image("reader");
    let recorded = terse_ok(&[
        "run", &reader, "--relay", &url, "--store", store, "--config", config, "--data", data, "--events",
        events.to_str().unwrap(), "--frames", "120", "--record", rec.to_str().unwrap(), "--hash-every", "1", "--dump", "--dump-page",
    ]);
    assert!(recorded.contains("Paragraph 0: fetched while the reader runs"), "{recorded}");
    assert!(Path::new(store).join(to_hex(&blob_id(big.as_bytes()))).exists(), "fetched from the relay");

    // The recording holds the article's bytes as the gets delivered them,
    // the key presses, and the page's events.
    let bytes = std::fs::read(&rec).unwrap();
    let all = frames(&bytes, &reader);
    assert_eq!(all.len(), 120);
    assert!(all.iter().enumerate().all(|(i, f)| f.frame == i as u32));
    let got: Vec<&[u8]> = all
        .iter()
        .flat_map(|f| &f.completions)
        .filter_map(|c| match &c.outcome {
            Outcome::Got { size, data } if *size as usize == big.len() => Some(&data[..]),
            _ => None,
        })
        .collect();
    assert!(!got.is_empty() && big.as_bytes().starts_with(got[0]), "the article's first bytes are recorded");
    let events_of = |kind: u8| all.iter().flat_map(|f| &f.events).filter(|e| e.kind == kind).count();
    assert!(events_of(Event::KEY_DOWN) == 3 && events_of(Event::PAGE_LAID_OUT) > 0);

    // The replay: the same hash after every frame, the same screen.
    let replayed = replay_ok(&reader, &rec, &["--hash-every", "1", "--dump", "--dump-page"]);
    assert_eq!(replayed, recorded);
    // Fewer frames: a prefix.
    let first = replay_ok(&reader, &rec, &["--hash-every", "1", "--frames", "50"]);
    assert_eq!(first.lines().count(), 50);
    assert!(recorded.starts_with(&first));

    // A recording whose delivered bytes were changed differs from the
    // machine's run at the frame they reach it.
    let at = bytes.windows(9).position(|w| w == b"Paragraph").unwrap();
    let mut changed = bytes.clone();
    changed[at] = b'p';
    let changed_rec = Path::new(work).join("changed.rec");
    std::fs::write(&changed_rec, &changed).unwrap();
    let out = replay(&reader, &changed_rec, &["--hash-every", "1"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("differs from the recording after frame"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(recorded.starts_with(&String::from_utf8(out.stdout).unwrap()), "identical until then");

    // Another image's recording is refused, and so are options that would
    // need a network, a store or a script.
    let out = replay(&image("hello"), &rec, &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("a recording of image"), "{}", String::from_utf8_lossy(&out.stderr));
    for extra in [&["--relay", url.as_str()][..], &["--store", store], &["--events", events.to_str().unwrap()], &["--data", data]] {
        let out = replay(&reader, &rec, extra);
        assert!(!out.status.success(), "{extra:?}");
    }
}

/// Sparks with its recorded controller session (programs/sparks/sessions):
/// controllers, events and named records, replayed frame for frame.
#[test]
fn sparks_session_replays() {
    let dirs: Vec<tempfile::TempDir> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let [store, config, data, work] = [0, 1, 2, 3].map(|i| dirs[i].path().to_str().unwrap());
    let rec = Path::new(work).join("sparks.rec");
    let sparks = image("sparks");
    let session = programs().join("sparks/sessions/pad.txt");
    let recorded = terse_ok(&[
        "run", &sparks, "--events", session.to_str().unwrap(), "--frames", "3600", "--store", store, "--config", config, "--data",
        data, "--record", rec.to_str().unwrap(), "--hash-every", "1", "--dump", "--dump-vectors",
    ]);
    let all = frames(&std::fs::read(&rec).unwrap(), &sparks);
    assert_eq!(all.len(), 3600);
    assert!(all.iter().any(|f| f.controllers[0][0] == 1 && f.controllers[0][1] != 0), "controller 0 recorded");
    assert!(all.iter().any(|f| !f.completions.is_empty()), "named records completed");
    assert_eq!(replay_ok(&sparks, &rec, &["--hash-every", "1", "--dump", "--dump-vectors"]), recorded);
}

/// SPEC 2, STATE.md 5: the grants are in the recording, after the
/// snapshot: sparks without content (`--without 4`, or the program
/// profile) records 00h and replays without content, on a host that has
/// it; with content, 01h. Each replays only as it was recorded.
#[test]
fn recordings_carry_the_grants() {
    let dir = tempfile::tempdir().unwrap();
    let path = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
    let sparks = image("sparks");
    let session = programs().join("sparks/sessions/keys.txt");
    let session = session.to_str().unwrap();
    let args = ["--events", session, "--frames", "300", "--hash-every", "1", "--store", &path("st"), "--data", &path("d"), "--config", &path("c")];
    let with = terse_ok(&[&["run", &sparks, "--record", &path("with")][..], &args].concat());
    let without = terse_ok(&[&["run", &sparks, "--without", "4", "--record", &path("without")][..], &args].concat());
    let program = terse_ok(&[&["run", &sparks, "--profile", "program", "--record", &path("program")][..], &args].concat());
    assert_ne!(with, without, "the grants are in the state");
    assert_eq!(without, program, "--without 4 is the program profile for sparks");
    for (name, flag) in [("with", 1u8), ("without", 0), ("program", 0)] {
        let bytes = std::fs::read(path(name)).unwrap();
        assert_eq!(bytes[terse_core::record::HEADER_LEN], flag, "{name}");
        assert_eq!(frames(&bytes, &sparks).len(), 300);
    }
    assert_eq!(replay_ok(&sparks, Path::new(&path("with")), &["--hash-every", "1"]), with);
    assert_eq!(replay_ok(&sparks, Path::new(&path("without")), &["--hash-every", "1"]), without);
    // A grant byte that is not a flag is refused.
    let mut bad = std::fs::read(path("without")).unwrap();
    bad[terse_core::record::HEADER_LEN] = 2;
    std::fs::write(path("bad"), bad).unwrap();
    assert!(!replay(&sparks, Path::new(&path("bad")), &[]).status.success());
}

/// A run started from a snapshot (`--load-state`) records the snapshot
/// file, and its replay starts from it.
#[test]
fn recording_from_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
    let hello = image("hello");
    terse_ok(&["run", &hello, "--frames", "10", "--save-state", &path("s")]);
    let recorded = terse_ok(&["run", &hello, "--load-state", &path("s"), "--frames", "20", "--record", &path("r"), "--hash-every", "1"]);
    assert_eq!(recorded.lines().next().unwrap().split(' ').next(), Some("11"));
    assert_eq!(replay_ok(&hello, Path::new(&path("r")), &["--hash-every", "1"]), recorded);
    // Not for a system.
    assert!(!terse(&["run", "--shell", "--record", &path("x"), "--frames", "1", "--store", &path("st"), "--data", &path("d")]).status.success());
    assert!(!Path::new(&path("x")).exists());
}
