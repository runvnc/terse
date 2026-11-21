//! Programs published in feeds (NETWORK 4.2, 4.4, 4.7) and named records
//! (SPEC §content.records) from the command line, over an in-process relay on an
//! ephemeral loopback port, with temporary stores, configs and data
//! directories.

mod common;

use std::net::TcpListener;
use std::path::Path;
use std::process::Output;
use std::sync::Arc;

use terse_core::Image;
use terse_net::{BlobStore, blob_id, to_hex};

const SPARKS: &[u8] = include_bytes!("../../../programs/sparks/sparks.terse");
const HELLO: &[u8] = include_bytes!("../../../programs/hello/hello.terse");

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

struct User {
    store: tempfile::TempDir,
    config: tempfile::TempDir,
    data: tempfile::TempDir,
}

impl User {
    fn new() -> Self {
        User { store: tempfile::tempdir().unwrap(), config: tempfile::tempdir().unwrap(), data: tempfile::tempdir().unwrap() }
    }

    fn args<'a>(&'a self, args: &[&'a str]) -> Vec<&'a str> {
        let mut all = args.to_vec();
        all.extend(["--store", self.store.path().to_str().unwrap(), "--config", self.config.path().to_str().unwrap()]);
        all
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = terse(&self.args(args));
        assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    fn fails(&self, args: &[&str]) -> String {
        let out = terse(&self.args(args));
        assert!(!out.status.success(), "terse {args:?} succeeded");
        String::from_utf8(out.stderr).unwrap()
    }

    fn records(&self, args: &[&str]) -> Output {
        let mut all = vec!["records"];
        all.extend(args);
        all.extend(["--data", self.data.path().to_str().unwrap()]);
        terse(&all)
    }
}

#[test]
fn publish_follow_list_and_find_programs() {
    let relay_store = tempfile::tempdir().unwrap();
    let url = start_relay(relay_store.path());
    let dir = tempfile::tempdir().unwrap();
    let sparks = dir.path().join("sparks.terse");
    let hello = dir.path().join("hello.terse");
    let junk = dir.path().join("junk.terse");
    std::fs::write(&sparks, SPARKS).unwrap();
    std::fs::write(&hello, HELLO).unwrap();
    std::fs::write(&junk, b"not an image").unwrap();

    let author = User::new();
    let key = author.ok(&["key", "new"]).trim().to_string();
    author.ok(&["profile", "set", "--name", "Ann", "--listed", "--relay", &url]);
    // The name is the image's title unless given; the image is checked.
    let out = author.ok(&["feed", "publish", "--program", sparks.to_str().unwrap(), "--relay", &url]);
    assert_eq!(out, format!("seq 2 program sparks image {}\n", to_hex(&blob_id(SPARKS))));
    let out = author.ok(&["feed", "publish", "--program", hello.to_str().unwrap(), "--name", "Hello, world", "--relay", &url]);
    assert!(out.starts_with("seq 3 program Hello, world image "), "{out}");
    assert!(author.fails(&["feed", "publish", "--program", junk.to_str().unwrap(), "--relay", &url]).contains("not a Terse32 image"));
    assert!(author.fails(&["feed", "publish", "--program", hello.to_str().unwrap(), "--name", " x", "--relay", &url]).contains("not a valid name"));
    // A new version of sparks (another image under the same name).
    let v2 = Image::build("sparks", 1, &[terse_core::DeviceReq::new(2, 0)], &[0x6F, 0, 0, 0]);
    let v2_path = dir.path().join("v2.terse");
    std::fs::write(&v2_path, &v2).unwrap();
    author.ok(&["feed", "publish", "--program", v2_path.to_str().unwrap(), "--relay", &url]);

    // A follower: sync downloads the current versions only, whatever the
    // article limit.
    let reader = User::new();
    reader.ok(&["feed", "subscribe", &key]);
    let out = reader.ok(&["feed", "sync", "--relay", &url, "--auto-download", "10"]);
    assert!(out.contains("[program: sparks]"), "{out}");
    assert!(out.contains("(downloaded)"), "{out}");
    let store = BlobStore::open(reader.store.path(), None).unwrap();
    assert!(store.read(&blob_id(&v2)).unwrap().is_some(), "the current version");
    assert!(store.read(&blob_id(HELLO)).unwrap().is_some());
    assert!(store.read(&blob_id(SPARKS)).unwrap().is_none(), "not the older version");
    let programs = reader.ok(&["feed", "programs"]);
    let lines: Vec<&str> = programs.lines().collect();
    assert_eq!(lines.len(), 2, "{programs}");
    assert!(lines[0].contains("(Ann)") && lines[0].contains("#3") && lines[0].contains("local") && lines[0].ends_with("[program: Hello, world]"), "{programs}");
    assert!(lines[1].contains("#4") && lines[1].ends_with("[program: sparks]"), "{programs}");
    assert_eq!(reader.ok(&["feed", "programs", &key]), programs);
    let list = reader.ok(&["feed", "list"]);
    assert_eq!(list.matches("[program: sparks]").count(), 2, "{list}");
    // Search: current versions only, and only programs.
    let found = reader.ok(&["find", "--programs", "SPARK", "--relay", &url]);
    assert_eq!(found.lines().count(), 1, "{found}");
    assert!(found.contains("#4") && found.contains("[program: sparks]"), "{found}");
    assert!(reader.ok(&["find", "--entries", "spark", "--relay", &url]).is_empty());
    assert!(!terse(&reader.args(&["find", "--programs", "--entries", "x", "--relay", &url])).status.success());
}

/// SPEC §content.records: `terse run` keeps named records in --data; `terse records`
/// lists, shows and deletes them. The built-in shell keeps its layout
/// under its identity (builtin:shell); a file run keeps records under its
/// image ID.
#[test]
fn records_from_runs() {
    let u = User::new();
    let events = u.data.path().join("l.events");
    std::fs::write(&events, "2 press l\n").unwrap();
    let data = u.data.path().to_str().unwrap();
    u.ok(&["run", "--shell", "--frames", "5", "--events", events.to_str().unwrap(), "--data", data]);
    let list = u.records(&["list"]);
    assert_eq!(String::from_utf8_lossy(&list.stdout), "builtin:shell  1 records, 2 bytes\n");
    let one = u.records(&["list", "builtin:shell"]);
    assert_eq!(String::from_utf8_lossy(&one.stdout), "       2 B  layout\n");
    let show = u.records(&["show", "builtin:shell", "layout"]);
    assert_eq!(show.stdout, [2, 2], "the grid, chosen with L");
    // The same image run from a file is another owner.
    let shell = u.data.path().join("shell.terse");
    std::fs::write(&shell, terse_net::builtin::SHELL).unwrap();
    u.ok(&["run", "--shell", shell.to_str().unwrap(), "--frames", "5", "--events", events.to_str().unwrap(), "--data", data]);
    let list = String::from_utf8(u.records(&["list"]).stdout).unwrap();
    let image = format!("image:{}", to_hex(&blob_id(terse_net::builtin::SHELL)));
    assert_eq!(list, format!("builtin:shell  1 records, 2 bytes\n{image}  1 records, 2 bytes\n"));
    // Delete; errors.
    assert!(u.records(&["delete", &image, "layout"]).status.success());
    assert!(!u.records(&["delete", &image, "layout"]).status.success());
    assert!(!u.records(&["show", "builtin:shell", "nope"]).status.success());
    assert!(!u.records(&["list", "someone"]).status.success());
    assert_eq!(String::from_utf8(u.records(&["list"]).stdout).unwrap(), "builtin:shell  1 records, 2 bytes\n");
}
