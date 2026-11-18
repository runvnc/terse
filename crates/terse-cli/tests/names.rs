//! Relay lists, profiles, nicknames and search from the command line
//! (NETWORK 4.6, 4.7, 5), over in-process relays on ephemeral loopback
//! ports.

mod common;

use std::net::TcpListener;
use std::path::Path;
use std::process::Output;
use std::sync::Arc;

use terse_net::{BlobStore, FeedStore, Identity, blob_id, to_hex};

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

struct User {
    store: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl User {
    fn new() -> Self {
        User { store: tempfile::tempdir().unwrap(), config: tempfile::tempdir().unwrap() }
    }

    fn args<'a>(&'a self, args: &[&'a str]) -> Vec<&'a str> {
        let mut all = args.to_vec();
        all.extend(["--store", self.store.path().to_str().unwrap(), "--config", self.config.path().to_str().unwrap()]);
        all
    }

    fn ok(&self, args: &[&str]) -> String {
        terse_ok(&self.args(args))
    }

    fn fails(&self, args: &[&str]) -> String {
        let out = terse(&self.args(args));
        assert!(!out.status.success(), "terse {args:?} succeeded");
        String::from_utf8(out.stderr).unwrap()
    }
}

#[test]
fn relay_list_commands() {
    let u = User::new();
    let relays = u.config.path().join("relays");
    assert!(!terse(&u.args(&["relay", "list"])).status.success(), "nothing listed: status 1");
    // The older single-relay file is read while there is no list.
    std::fs::write(u.config.path().join("relay"), "ws://old.example:7878\n").unwrap();
    assert_eq!(u.ok(&["relay", "list"]), "ws://old.example:7878\n");
    assert_eq!(u.ok(&["relay", "show"]), "ws://old.example:7878\n");
    // add starts from it and writes the list.
    u.ok(&["relay", "add", "ws://a.example:1"]);
    assert_eq!(std::fs::read_to_string(&relays).unwrap(), "ws://old.example:7878\nws://a.example:1\n");
    u.fails(&["relay", "add", "ws://a.example:1"]);
    u.fails(&["relay", "add", "http://a.example"]);
    u.ok(&["relay", "remove", "ws://old.example:7878"]);
    u.fails(&["relay", "remove", "ws://old.example:7878"]);
    assert_eq!(u.ok(&["relay", "list"]), "ws://a.example:1\n");
    assert_eq!(u.ok(&["relay", "set", "ws://b.example:2"]), "ws://b.example:2\n");
    assert_eq!(u.ok(&["relay", "list"]), "ws://b.example:2\n");
    u.ok(&["relay", "remove", "ws://b.example:2"]);
    assert!(!terse(&u.args(&["relay", "list"])).status.success(), "an empty list");
}

#[test]
fn old_relay_file_is_used() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let u = User::new();
    std::fs::write(u.config.path().join("relay"), format!("{url}\n")).unwrap();
    let file = u.store.path().join("x.txt");
    std::fs::write(&file, b"via the old file").unwrap();
    let out = u.ok(&["put", file.to_str().unwrap()]);
    assert_eq!(out.trim(), to_hex(&blob_id(b"via the old file")));
    assert!(relay_dir.path().join(out.trim()).exists());
}

#[test]
fn profiles_nicknames_and_search() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (start_relay(da.path()), start_relay(db.path()));
    let (author, quiet, reader) = (User::new(), User::new(), User::new());
    let key = author.ok(&["key", "new"]).trim().to_string();
    let quiet_key = quiet.ok(&["key", "new"]).trim().to_string();

    // The author lists two relays; the profile goes to both.
    author.ok(&["relay", "add", &a]);
    author.ok(&["relay", "add", &b]);
    let about = author.store.path().join("about.txt");
    std::fs::write(&about, "I write about relays.\n").unwrap();
    let out = author.ok(&["profile", "set", "--name", "Ana Writer", "--listed", "--description", about.to_str().unwrap()]);
    assert!(out.starts_with("seq 1 blob ") && out.ends_with("(listed)\n"), "{out}");
    let feed = Identity::load(&author.config.path().join("identity.key")).unwrap().feed_key();
    for d in [&da, &db] {
        assert_eq!(FeedStore::beside_blobs(d.path()).seqs(&feed).unwrap(), [1]);
    }
    let shown = author.ok(&["profile", "show"]);
    assert!(shown.contains("name:        Ana Writer") && shown.contains("listed:      yes"), "{shown}");
    assert!(shown.contains("I write about relays."), "{shown}");
    assert!(!shown.contains("relay:"), "loopback relays are not written into a profile: {shown}");
    author.fails(&["profile", "set", "--name", " Ana"]);
    let file = author.store.path().join("post.txt");
    std::fs::write(&file, "Relays, mirrored.\n").unwrap();
    author.ok(&["feed", "publish", file.to_str().unwrap(), "--title", "On mirrored relays"]);
    // Renaming keeps the description and the listing.
    assert!(author.ok(&["profile", "set", "--name", "Ana W."]).ends_with("(listed)\n"));
    assert!(author.ok(&["profile", "show"]).contains("I write about relays."));
    // An unlisted feed with a matching name.
    quiet.ok(&["profile", "set", "--name", "Anastasia", "--relay", &a]);

    // Search on A: only the listed feed; its newest name.
    reader.ok(&["relay", "add", &a]);
    let found = reader.ok(&["find", "ana"]);
    assert_eq!(found, format!("{key}  Ana W.\n"));
    let found = reader.ok(&["find", "--entries", "MIRRORED"]);
    assert!(found.contains("On mirrored relays") && found.contains(&key[..8]), "{found}");
    assert!(reader.ok(&["find", &quiet_key[..6]]).is_empty());

    // Names in listings: the declared name, then a nickname over it.
    reader.ok(&["feed", "subscribe", &key]);
    reader.ok(&["feed", "subscribe", &quiet_key]);
    reader.ok(&["feed", "sync", "--no-feed-relays"]);
    let subs = reader.ok(&["feed", "subscriptions"]);
    assert!(subs.contains(&format!("{key}  Ana W.  3 entries")), "{subs}");
    assert!(subs.contains(&format!("{quiet_key}  Anastasia  1 entries")), "{subs}");
    let listed = reader.ok(&["feed", "list", &key]);
    assert!(listed.lines().all(|l| l.contains(&format!("{} (Ana W.)", &key[..8]))), "{listed}");
    assert!(listed.contains("[profile: Ana W.]"), "{listed}");
    assert_eq!(reader.ok(&["feed", "nickname", &key, "Ana from work"]), format!("{key}  Ana from work\n"));
    assert!(reader.ok(&["feed", "subscriptions"]).contains(&format!("{key}  Ana from work  3 entries")));
    assert!(reader.ok(&["find", "ana"]).contains("(your nickname: Ana from work)"));
    let shown = reader.ok(&["profile", "show", &key]);
    assert!(shown.contains("nickname:    Ana from work") && shown.contains("name:        Ana W."), "{shown}");
    reader.fails(&["feed", "nickname", &key, "bad\tname"]);
    assert!(reader.ok(&["feed", "nickname", &key]).contains("nickname removed"));
    reader.fails(&["feed", "nickname", &key]);
    assert!(reader.ok(&["feed", "subscriptions"]).contains(&format!("{key}  Ana W.  3 entries")));
}
