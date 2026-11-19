//! The manual (docs/manual): its build is up to date and deterministic,
//! its links resolve to blobs every host carries, it covers every call,
//! event, content operation and status of SPEC.md, and `terse manual
//! publish` puts it on the user's feed.

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use terse_net::{BlobStore, blob_id, builtin, manual, parse_hex, to_hex};

fn dir() -> PathBuf {
    common::workspace().join("docs/manual")
}

fn terse(args: &[&str]) -> Output {
    common::terse_command().args(args).output().unwrap()
}

/// The checked-in build is what the sources build to, and what `terse`
/// carries.
#[test]
fn build_is_up_to_date() {
    let built = manual::build(&manual::read_sources(&dir()).unwrap()).unwrap();
    let out = dir().join("build");
    for b in &built {
        let file = std::fs::read_to_string(out.join(format!("{}.txt", b.name))).unwrap_or_default();
        assert!(file == b.text, "docs/manual/build/{}.txt is stale: run `terse manual build`", b.name);
    }
    let manifest = std::fs::read_to_string(out.join("manifest")).unwrap();
    assert_eq!(manifest, manual::build_manifest(&built), "docs/manual/build/manifest is stale: run `terse manual build`");
    assert_eq!(built.len(), builtin::MANUAL.len());
    for (b, (name, title, text)) in built.iter().zip(builtin::MANUAL) {
        assert_eq!((b.name.as_str(), b.title.as_str(), b.text.as_bytes()), (*name, *title, *text));
    }
    assert_eq!(built[0].name, "index", "the index comes first");
    // `terse manual check` agrees.
    let out = terse(&["manual", "check", "--dir", dir().to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// `terse manual build` on a copy reports cycles and unknown names.
#[test]
fn build_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    std::fs::write(d.join("manifest"), "a\nb\n").unwrap();
    std::fs::write(d.join("a.txt"), "# A\n\n[B](terse:{b})\n").unwrap();
    std::fs::write(d.join("b.txt"), "# B\n\n[A](terse:{a})\n").unwrap();
    let out = terse(&["manual", "build", "--dir", d.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("the links form a cycle: a -> b -> a"), "{out:?}");
    std::fs::write(d.join("b.txt"), "# B\n\nterse:{c}\n").unwrap();
    let out = terse(&["manual", "build", "--dir", d.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown article terse:{c}"), "{out:?}");
    std::fs::write(d.join("b.txt"), "# B\n\nleaf\n").unwrap();
    let out = terse(&["manual", "build", "--dir", d.to_str().unwrap()]);
    assert!(out.status.success(), "{out:?}");
    let first = std::fs::read(d.join("build/a.txt")).unwrap();
    terse(&["manual", "build", "--dir", d.to_str().unwrap()]);
    assert_eq!(std::fs::read(d.join("build/a.txt")).unwrap(), first, "deterministic");
    std::fs::write(d.join("b.txt"), "# B\n\nchanged\n").unwrap();
    let out = terse(&["manual", "check", "--dir", d.to_str().unwrap()]);
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("a, b, manifest"), "{out:?}");
}

/// Every `terse:` article link of the built manual names a blob every host
/// puts in its store, and every manual article but the index is linked to.
#[test]
fn links_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let store = BlobStore::open(tmp.path(), None).unwrap();
    builtin::install(&store, &[]).unwrap();
    let mut linked = std::collections::HashSet::new();
    for (name, _, text) in builtin::MANUAL {
        let text = std::str::from_utf8(text).unwrap();
        assert!(!text.contains("terse:{"), "{name}: an unresolved placeholder");
        let mut at = 0;
        while let Some(i) = text[at..].find("terse:") {
            let start = at + i + 6;
            at = start;
            let Some(id) = text.get(start..start + 64).and_then(parse_hex) else { continue };
            assert!(store.read(&id).unwrap().is_some(), "{name}: terse:{} is not a built-in blob", to_hex(&id));
            linked.insert(id);
        }
    }
    for (name, _, text) in &builtin::MANUAL[1..] {
        assert!(linked.contains(&blob_id(text)), "{name} is linked to");
    }
}

/// The rows of the Markdown table of SPEC.md section `heading` (up to the
/// next heading): the first two cells of each, without backquotes.
fn spec_rows(spec: &str, heading: &str) -> Vec<(String, String)> {
    let start = spec.find(heading).unwrap_or_else(|| panic!("{heading}"));
    let body = &spec[start + heading.len()..];
    let end = body.find("\n#").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            let cells: Vec<&str> = l.split('|').map(|c| c.trim().trim_matches('`')).collect();
            (cells[1].to_string(), cells[2].to_string())
        })
        .filter(|(n, _)| n.ends_with('h') && n[..n.len() - 1].bytes().all(|b| b.is_ascii_hexdigit()))
        .collect()
}

/// Every call (number and name) of SPEC 3.2, every event type of SPEC 5,
/// every content operation (SPEC 9.1, 9.5, 9.6) and status (SPEC 9.3) is
/// on one line of the manual with its name, so the manual cannot fall
/// behind the spec without this failing.
#[test]
fn covers_the_spec() {
    let spec = std::fs::read_to_string(common::workspace().join("SPEC.md")).unwrap();
    let lines: Vec<String> = builtin::MANUAL
        .iter()
        .flat_map(|(_, _, t)| std::str::from_utf8(t).unwrap().lines().map(str::to_lowercase).collect::<Vec<_>>())
        .collect();
    let find = |what: &str, number: &str, name: &str| {
        let (n, m) = (number.to_lowercase(), name.to_lowercase());
        assert!(lines.iter().any(|l| l.contains(&n) && l.contains(&m)), "{what} {number} {name} is not in the manual");
    };
    let calls = spec_rows(&spec, "### 3.2 Call table");
    assert!(calls.len() >= 47, "{}", calls.len());
    for (n, name) in &calls {
        find("call", n, name);
    }
    let events = spec_rows(&spec, "## 5. Events");
    assert_eq!(events.len(), 16);
    for (n, name) in &events {
        find("event", n, name);
    }
    let mut ops = vec![("01h".to_string(), "get".to_string()), ("02h".to_string(), "put".to_string())];
    ops.extend(spec_rows(&spec, "### 9.5 Feeds, names and search"));
    ops.extend(spec_rows(&spec, "### 9.6 Named records"));
    assert_eq!(ops.len(), 17);
    for (n, name) in &ops {
        find("content op", n, name);
    }
    let statuses = spec_rows(&spec, "### 9.3 Status values");
    assert_eq!(statuses.len(), 7);
    for (n, meaning) in &statuses {
        // The first word of the meaning ("Not found", "No room", ...).
        let word = meaning.split([' ', ':', '(']).next().unwrap();
        find("status", n, word);
    }
}

fn start_relay(store: &Path) -> String {
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(store, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

/// `terse manual publish` publishes every article as an entry of the
/// user's feed, the index last, with the articles' titles; the relay then
/// holds every article.
#[test]
fn publish_to_a_relay() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let (store, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let opts = ["--relay", url.as_str(), "--store", store.path().to_str().unwrap(), "--config", config.path().to_str().unwrap()];
    let key = terse(&[&["key", "new"][..], &opts].concat());
    assert!(key.status.success());
    let out = terse(&[&["manual", "publish"][..], &opts].concat());
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), builtin::MANUAL.len());
    assert!(lines.last().unwrap().ends_with(" index"), "{stdout}");
    let relay_store = BlobStore::open(relay_dir.path(), None).unwrap();
    for (name, _, text) in builtin::MANUAL {
        assert!(relay_store.read(&blob_id(text)).unwrap().is_some(), "{name} is on the relay");
    }
    // The entries carry the titles.
    let list = terse(&[&["feed", "list"][..], &opts].concat());
    let list = String::from_utf8(list.stdout).unwrap();
    for (_, title, _) in builtin::MANUAL {
        assert!(list.contains(title), "{title}: {list}");
    }
}
