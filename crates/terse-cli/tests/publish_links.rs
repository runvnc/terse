//! `terse feed publish` and the `terse:` links of an article: each is
//! listed with whether this host holds what it leads to, whether it was
//! uploaded with the article, and whether a relay keeps it (an entry of the
//! user's own feed names it), or it is link-only (another feed's, or
//! unknown here). A link to a blob held here that no entry names is
//! uploaded with a warning, since a relay that drops such blobs (NETWORK
//! 5.3) drops it after a while; a link to a blob of the user's own feed
//! that this host does not hold is refused unless `--force`; `--dry-run`
//! lists and publishes nothing. On a
//! scratch relay in this process, on an ephemeral port, whose clock the
//! test moves on to show what it keeps.

mod common;

use std::net::TcpListener;
use std::path::Path;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use terse_net::{BlobStore, blob_id, to_hex};
use terse_relay::{Policy, Relay};

fn terse(args: &[&str]) -> Output {
    common::terse_command().args(args).output().unwrap()
}

fn ok(args: &[&str]) -> String {
    let out = terse(args);
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// A relay that keeps a blob no entry names for an hour (NETWORK 5.3).
fn start_relay(store: &Path) -> (String, Arc<Relay>) {
    let policy = Policy { unreferenced_ttl: Some(Duration::from_secs(3600)), ..Policy::unlimited(1024) };
    let relay = Arc::new(Relay::with_policy(BlobStore::open(store, None).unwrap(), policy).unwrap());
    relay.freeze_clock();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let served = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, served));
    (url, relay)
}

struct User {
    store: tempfile::TempDir,
    config: tempfile::TempDir,
    key: String,
}

impl User {
    fn new() -> Self {
        let config = tempfile::tempdir().unwrap();
        let key = ok(&["key", "new", "--config", config.path().to_str().unwrap()]).trim().to_string();
        User { store: tempfile::tempdir().unwrap(), config, key }
    }

    fn args<'a>(&'a self, args: &[&'a str], relay: &'a str) -> Vec<&'a str> {
        let mut all = vec!["feed"];
        all.extend_from_slice(args);
        all.extend(["--store", self.store.path().to_str().unwrap(), "--config", self.config.path().to_str().unwrap(), "--relay", relay]);
        all
    }

    fn file(&self, name: &str, text: &[u8]) -> String {
        let path = self.store.path().join(name);
        std::fs::write(&path, text).unwrap();
        path.to_str().unwrap().to_string()
    }
}

#[test]
fn publish_names_linked_articles() {
    let relay_dir = tempfile::tempdir().unwrap();
    let (url, relay) = start_relay(relay_dir.path());
    let (author, other) = (User::new(), User::new());

    // The author's own part, published as an article of its own (#1).
    let part = b"Part two: the details.\n".to_vec();
    let part_file = author.file("part.txt", &part);
    ok(&author.args(&["publish", &part_file, "--title", "Part two"], &url));
    // A picture written here and never published: held, named by nothing.
    let picture = b"```path 10 10\nM 0 0 L 10 10\n```\n".to_vec();
    BlobStore::open(author.store.path(), None).unwrap().put(&picture).unwrap();
    // Someone else's article, which the author follows and so holds.
    let theirs = b"Someone else's article.\n".to_vec();
    let theirs_file = other.file("theirs.txt", &theirs);
    ok(&other.args(&["publish", &theirs_file, "--title", "Theirs"], &url));
    ok(&author.args(&["subscribe", &other.key], &url));
    ok(&author.args(&["sync"], &url));
    // And an article nobody here knows of.
    let unknown = [0x5a; 32];

    let (part_id, picture_id, theirs_id) = (to_hex(&blob_id(&part)), to_hex(&blob_id(&picture)), to_hex(&blob_id(&theirs)));
    let zero = "0".repeat(64);
    let main = format!(
        "# Main\n\nSee terse:{part_id} and the picture terse:{picture_id}.\n\n\
         [Their article](terse:{theirs_id}), terse:{}, by terse:feed/{}.\n\n\
         The shell: terse:program/{zero}/Shell. Code is not a link: `terse:{}`.\n",
        to_hex(&unknown),
        other.key,
        "ab".repeat(32),
    );
    let main_file = author.file("main.txt", main.as_bytes());
    let short = format!("feed {}…{}", &other.key[..6], &other.key[58..]);
    let list = |part_up: &str, picture_up: &str| {
        vec![
            format!("link terse:{part_id}: held here, {part_up}; kept: your feed's entry #1 names it, and relays keep it while they keep that entry"),
            format!(
                "link terse:{picture_id}: held here, {picture_up}; NOT KEPT: no entry names it, and a relay that drops blobs no entry names (NETWORK.md 5.3) drops it"
            ),
            format!("link terse:{theirs_id}: held here, not uploaded; link-only: {short}'s entry #1 names it; kept where that feed is kept"),
            format!(
                "link terse:{}: not held here, not uploaded; link-only: no entry held here names it; kept only where its own feed is kept",
                to_hex(&unknown)
            ),
            format!("link terse:feed/{}: a feed, nothing to upload", other.key),
            format!("link terse:program/{zero}/Shell: a program built into machine hosts, nothing to upload"),
        ]
    };
    let highest = || relay.feeds().highest(&terse_net::parse_hex(&author.key).unwrap()).unwrap();

    let warning = format!("terse: warning: terse:{picture_id} is named by no entry");
    // A dry run lists and publishes nothing.
    let out = terse(&author.args(&["publish", &main_file, "--title", "Main", "--dry-run"], &url));
    assert!(out.status.success());
    let mut expected = list("would be uploaded", "would be uploaded");
    expected.push("dry run: nothing uploaded or published".into());
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().collect::<Vec<_>>(), expected);
    assert!(String::from_utf8_lossy(&out.stderr).contains(&warning));
    assert_eq!(highest(), 1);
    assert!(!relay.blobs().path(&blob_id(&picture)).exists());

    // Published: the author's own blobs, and the one no entry names, go up
    // with it; the latter with a warning.
    let up = "uploaded now (taken by 1 of 1 relays)";
    let out = terse(&author.args(&["publish", &main_file, "--title", "Main"], &url));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[..6], list(up, up));
    assert!(lines[6].starts_with("seq 2 blob "), "{stdout}");
    assert!(String::from_utf8_lossy(&out.stderr).contains(&warning));
    assert_eq!(highest(), 2);
    assert!(relay.blobs().path(&blob_id(&picture)).exists());

    // A part of the author's own feed that this host no longer holds
    // cannot go up with an article: refused, unless --force.
    let gone = b"Part three, published and then lost here.\n".to_vec();
    let gone_file = author.file("gone.txt", &gone);
    ok(&author.args(&["publish", &gone_file, "--title", "Part three"], &url));
    std::fs::remove_file(author.store.path().join(to_hex(&blob_id(&gone)))).unwrap();
    let gone_id = to_hex(&blob_id(&gone));
    let more = author.file("more.txt", format!("# More\n\nSee terse:{gone_id}.\n").as_bytes());
    let line = |up: &str| {
        format!("link terse:{gone_id}: not held here, {up}; kept: your feed's entry #3 names it, and relays keep it while they keep that entry")
    };
    let out = terse(&author.args(&["publish", &more, "--title", "More"], &url));
    assert!(!out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), line("not uploaded") + "\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("not published: the article links to an article of your feed that this host does not hold: terse:{gone_id}")),
        "{stderr}"
    );
    assert!(stderr.contains("give --force"), "{stderr}");
    let dry = terse(&author.args(&["publish", &more, "--title", "More", "--dry-run"], &url));
    assert!(!dry.status.success(), "a dry run says it would be refused");
    assert_eq!(highest(), 3);
    let out = ok(&author.args(&["publish", &more, "--title", "More", "--force"], &url));
    assert!(out.starts_with(&(line("not uploaded") + "\nseq 4 blob ")), "{out}");

    // An hour on, the relay has dropped the picture no entry names and
    // kept what entries name.
    relay.advance_clock(Duration::from_secs(3601));
    relay.collect();
    assert!(!relay.blobs().path(&blob_id(&picture)).exists(), "dropped");
    for kept in [&part, &theirs, main.as_bytes()] {
        assert!(relay.blobs().path(&blob_id(kept)).exists());
    }

    // An article without links publishes as before, with nothing listed.
    let plain = author.file("plain.txt", b"No links here.\n");
    let out = ok(&author.args(&["publish", &plain, "--title", "Plain"], &url));
    assert!(out.starts_with("seq 5 blob "), "{out}");
}
