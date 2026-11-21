//! `terse feed site`: a feed as a static website, fetched from a relay,
//! brought up to date incrementally (one new article: four files written,
//! one blob read), self-contained, deterministic.

mod common;

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::Path;
use std::process::Output;
use std::sync::Arc;
use std::time::SystemTime;

use terse_net::{BlobStore, blob_id, to_hex};

fn start_relay(store: &Path) -> String {
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(store, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

/// A user of terse: a store and a config directory of their own.
struct User {
    store: tempfile::TempDir,
    config: tempfile::TempDir,
    url: String,
}

impl User {
    fn new(url: &str) -> Self {
        User { store: tempfile::tempdir().unwrap(), config: tempfile::tempdir().unwrap(), url: url.to_string() }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut all: Vec<&str> = args.to_vec();
        all.extend(["--relay", &self.url, "--store", self.store.path().to_str().unwrap(), "--config", self.config.path().to_str().unwrap()]);
        common::terse_command().args(&all).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> (String, String) {
        let out = self.run(args);
        let (o, e) = (String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap());
        assert!(out.status.success(), "{args:?}: {o}{e}");
        (o, e)
    }
}

/// The text of article `i`. Article 1 has a figure and a table; article 5
/// links to article 3 (published before it), article 10 to article 32
/// (published after it).
fn text(i: usize) -> String {
    let mut t = format!("# Article {i}\n\nThe body of article number {i}. A web address in the text, https://example.com, stays text.\n");
    match i {
        1 => t.push_str(
            "\n## A figure and a table\n\n```path 60 30\nfill #3060c0 stroke fg 1 M0 0 L60 0 L30 30 Z\n```\n\n| Name | Count |\n|---|--:|\n| one | 1 |\n| `two` | 22 |\n",
        ),
        5 => t.push_str(&format!("\nSee [article 3](terse:{}).\n", to_hex(&blob_id(text(3).as_bytes())))),
        10 => t.push_str(&format!("\nLater: terse:{}\n", to_hex(&blob_id(text(32).as_bytes())))),
        _ => {}
    }
    t
}

fn publish(author: &User, dir: &Path, i: usize) {
    let file = dir.join(format!("{i}.txt"));
    std::fs::write(&file, text(i)).unwrap();
    author.ok(&["feed", "publish", file.to_str().unwrap(), "--title", &format!("Article {i}")]);
}

/// Every file of a directory but the state file: name to (bytes, mtime).
fn snapshot(dir: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), (std::fs::read(e.path()).unwrap(), e.metadata().unwrap().modified().unwrap())))
        .collect()
}

/// The files whose bytes or mtime differ, or that are new.
fn changed(before: &BTreeMap<String, (Vec<u8>, SystemTime)>, after: &BTreeMap<String, (Vec<u8>, SystemTime)>) -> Vec<String> {
    assert!(before.keys().all(|k| after.contains_key(k)), "no file is removed");
    after.iter().filter(|(k, v)| before.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect()
}

fn page_of(i: usize) -> String {
    format!("{}.html", &to_hex(&blob_id(text(i).as_bytes()))[..16])
}

const BASE: &str = "https://example.org/notes/";

#[test]
fn feed_site_incremental() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = start_relay(relay_dir.path());
    let author = User::new(&url);
    let work = tempfile::tempdir().unwrap();
    let key = author.ok(&["key", "new"]).0.trim().to_string();
    let description = work.path().join("description.txt");
    std::fs::write(&description, "Notes on small machines.\n").unwrap();
    author.ok(&["profile", "set", "--name", "Ann Example", "--description", description.to_str().unwrap()]);
    for i in 1..=30 {
        publish(&author, work.path(), i);
    }
    author.ok(&["feed", "publish", "--program", common::programs().join("hello/hello.terse").to_str().unwrap(), "--name", "Hello"]);

    // A reader who holds nothing of the feed makes its site.
    let reader = User::new(&url);
    let out = tempfile::tempdir().unwrap();
    let site = out.path().join("site");
    let s = site.to_str().unwrap();
    let make = |extra: &[&str]| reader.ok(&[&["feed", "site", &key, "--out", s, "--base-url", BASE][..], extra].concat());
    let (o, e) = make(&[]);
    // 30 pages, style.css, page-1, page-2, index.html, programs.html, feed.xml.
    assert!(o.starts_with("30 articles, 36 written, 0 unchanged, 31 blobs read in "), "{o}{e}");
    assert!(!e.contains("rewritten"), "{e}");
    let first = snapshot(&site);
    assert_eq!(first.len(), 36);
    eprintln!("the site: {} files, {} bytes", first.len(), first.values().map(|(b, _)| b.len()).sum::<usize>());
    check_site(&site, &key);
    let html = |name: &str| String::from_utf8(std::fs::read(site.join(name)).unwrap()).unwrap();
    // The profile gives the title and description; the links resolve.
    let index = html("index.html");
    assert!(index.contains("<h1>Ann Example</h1>") && index.contains("Notes on small machines."), "{index}");
    assert!(index.contains("<a href=\"programs.html\">Programs</a>"));
    assert!(html("programs.html").contains(&format!("Hello <code class=\"addr\">terse:program/{key}/Hello</code>")));
    assert!(html(&page_of(5)).contains(&format!("<a href=\"{}\">article 3</a>", page_of(3))));
    let ten = html(&page_of(10));
    assert!(ten.contains(&format!("<code class=\"addr\">terse:{}</code>", to_hex(&blob_id(text(32).as_bytes())))), "{ten}");
    let one = html(&page_of(1));
    assert!(one.contains("<svg viewBox=\"0 0 60 30\"") && one.contains("<th class=\"ae\">Count</th>"), "{one}");
    assert!(one.contains("https&#58;//example.com"));
    // Newest first, numbered from the oldest.
    assert!(index.find("Article 30<").unwrap() < index.find("Article 11<").unwrap() && !index.contains("Article 10<"));
    assert!(html("page-1.html").contains("Article 1<") && html("page-1.html").contains("Article 20<"));
    assert!(html("page-2.html").contains("Article 21<") && !html("page-2.html").contains("Article 20<"));

    // Nothing new: nothing written, nothing read.
    let (o, _) = make(&[]);
    assert!(o.starts_with("30 articles, 0 written, 36 unchanged, 0 blobs read"), "{o}");
    assert!(changed(&first, &snapshot(&site)).is_empty());

    // One new article: its page, the last index page, index.html and
    // feed.xml are written, and only its blob is read.
    publish(&author, work.path(), 31);
    let (o, _) = make(&[]);
    assert!(o.starts_with("31 articles, 4 written, 33 unchanged, 1 blob read"), "{o}");
    let second = snapshot(&site);
    let mut expected = vec![page_of(31), "feed.xml".into(), "index.html".into(), "page-2.html".into()];
    expected.sort();
    assert_eq!(changed(&first, &second), expected);
    assert!(String::from_utf8(second["feed.xml"].0.clone()).unwrap().contains("Article 31"));

    // An article an old one links to: that old page is written again too
    // (its link now resolves), and its blob read.
    publish(&author, work.path(), 32);
    let (o, _) = make(&[]);
    assert!(o.starts_with("32 articles, 5 written, 33 unchanged, 2 blobs read"), "{o}");
    let third = snapshot(&site);
    let mut expected = vec![page_of(32), page_of(10), "feed.xml".into(), "index.html".into(), "page-2.html".into()];
    expected.sort();
    assert_eq!(changed(&second, &third), expected);
    assert!(html(&page_of(10)).contains(&format!("<a href=\"{}\" title=\"Article 32\">terse:", page_of(32))));
    check_site(&site, &key);

    // The same inputs give the same bytes, in a new directory too.
    let again = out.path().join("again");
    let (o, _) = reader.ok(&["feed", "site", &key, "--out", again.to_str().unwrap(), "--base-url", BASE]);
    assert!(o.starts_with("32 articles, 38 written"), "{o}");
    let a = snapshot(&again);
    assert_eq!(a.keys().collect::<Vec<_>>(), third.keys().collect::<Vec<_>>());
    for (k, (bytes, _)) in &a {
        assert!(*bytes == third[k].0, "{k} differs");
    }
    assert_eq!(std::fs::read(again.join(".terse-site")).unwrap(), std::fs::read(site.join(".terse-site")).unwrap());

    // Other options rewrite everything; so does --force.
    let (o, e) = make(&["--title", "Ann's notes"]);
    assert!(o.starts_with("32 articles, 38 written, 0 unchanged, 32 blobs read"), "{o}");
    assert!(e.contains("every file rewritten: the site options changed"), "{e}");
    assert!(html(&page_of(1)).contains("<title>Article 1 - Ann&#39;s notes</title>"));
    let (o, e) = make(&["--title", "Ann's notes", "--force"]);
    assert!(o.starts_with("32 articles, 38 written, 0 unchanged, 32 blobs read"), "{o}");
    assert!(e.contains("every file rewritten: --force"), "{e}");
    let (o, _) = make(&["--title", "Ann's notes"]);
    assert!(o.starts_with("32 articles, 0 written, 38 unchanged, 0 blobs read"), "{o}");

    // Without a base URL there is no Atom feed, and it says so.
    let plain = out.path().join("plain");
    let (o, e) = reader.ok(&["feed", "site", &key, "--out", plain.to_str().unwrap()]);
    assert!(o.starts_with("32 articles, 37 written"), "{o}");
    assert!(e.contains("no Atom feed"), "{e}");
    assert!(!plain.join("feed.xml").exists());
    check_site(&plain, &key);

    // The author's own feed, the default, from the author's store.
    let own = out.path().join("own");
    let (o, _) = author.ok(&["feed", "site", "--out", own.to_str().unwrap()]);
    assert!(o.starts_with("32 articles, 37 written"), "{o}");
    assert_eq!(std::fs::read(own.join(page_of(7))).unwrap(), std::fs::read(plain.join(page_of(7))).unwrap());
}

/// The values of every `name="..."` attribute in `html`.
fn attrs<'a>(html: &'a str, name: &str) -> Vec<&'a str> {
    let key = format!(" {name}=\"");
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = html[at..].find(&key) {
        let start = at + i + key.len();
        let end = start + html[start..].find('"').unwrap();
        out.push(&html[start..end]);
        at = end;
    }
    out
}

/// Whether `xml` is well formed: every tag is closed in order, attributes
/// are quoted, no stray `<`, `>` or `&`.
fn well_formed(xml: &str) -> Result<(), String> {
    let mut stack: Vec<&str> = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(['<', '&']) {
        if rest[..i].contains('>') {
            return Err(format!("stray > in {:?}", &rest[..i]));
        }
        rest = &rest[i..];
        if rest.starts_with('&') {
            let end = rest.find(';').ok_or("unterminated reference")?;
            let name = &rest[1..end];
            if !(name.starts_with('#') || ["amp", "lt", "gt", "quot", "apos"].contains(&name)) {
                return Err(format!("unknown entity &{name};"));
            }
            rest = &rest[end + 1..];
            continue;
        }
        let end = rest.find('>').ok_or("unterminated tag")?;
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            if stack.pop() != Some(name) {
                return Err(format!("</{name}> closes nothing open"));
            }
            continue;
        }
        let empty = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, mut a) = tag.split_once(' ').unwrap_or((tag, ""));
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(format!("bad tag name {name:?}"));
        }
        loop {
            a = a.trim_start();
            if a.is_empty() {
                break;
            }
            let eq = a.find("=\"").ok_or(format!("unquoted attribute in <{tag}>"))?;
            let key = &a[..eq];
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b':') {
                return Err(format!("bad attribute {key:?}"));
            }
            let v = &a[eq + 2..];
            let close = v.find('"').ok_or("unterminated attribute")?;
            if v[..close].contains(['<', '>']) {
                return Err("< or > in an attribute".into());
            }
            a = &v[close + 1..];
        }
        if !empty {
            stack.push(name);
        }
    }
    if rest.contains('>') || !stack.is_empty() {
        return Err(format!("unclosed: {stack:?}"));
    }
    Ok(())
}

/// The site is self-contained: every link leads to a page of it (and a
/// fragment to an id there), nothing is loaded from elsewhere, no script,
/// no web address but the site's own in feed.xml, well-formed figures,
/// the feed's address on every page; feed.xml is well-formed Atom.
fn check_site(dir: &Path, key: &str) {
    let files: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap())
        })
        .filter(|(n, _)| !n.starts_with('.'))
        .collect();
    let page = |name: &str| files.iter().find(|(n, _)| n == name).map(|(_, d)| d.as_str());
    assert!(page("style.css").unwrap().starts_with(terse_net::manual_site::STYLE), "the manual's stylesheet, shared");
    for (name, data) in &files {
        let lower = data.to_lowercase();
        let checked = if name == "feed.xml" {
            lower.replace(BASE, "").replace("xmlns=\"http://www.w3.org/2005/atom\"", "")
        } else {
            lower.clone()
        };
        for bad in ["<script", "javascript:", "http://", "https://", "@import", "url(", " src=", "<iframe", "<object", "<embed", "<img", "<link rel=\"pre"] {
            assert!(!checked.contains(bad), "{name} contains {bad}");
        }
        if name == "feed.xml" {
            let body = data.strip_prefix("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n").expect("an XML declaration");
            well_formed(body).unwrap_or_else(|e| panic!("feed.xml: {e}"));
            assert!(body.starts_with("<feed xmlns=\"http://www.w3.org/2005/Atom\">"));
            assert!(body.contains(&format!("<id>terse:feed/{key}</id>")) && body.contains("<author><name>"));
            let entries = body.matches("<entry>").count();
            assert_eq!(entries, body.matches("<updated>").count() - 1, "every entry is dated");
            assert_eq!(entries, body.matches("<id>terse:").count() - 1, "every entry has an id");
            for href in attrs(body, "href") {
                assert!(href.starts_with(BASE), "{href}");
                assert!(page(&href[BASE.len()..]).is_some() || href == BASE, "{href} leads nowhere");
            }
            // The articles' HTML, escaped, links absolute.
            assert!(body.contains("&lt;a href=&quot;https://example.org/notes/") || !body.contains("&lt;a href"), "absolute links");
            continue;
        }
        if !name.ends_with(".html") {
            continue;
        }
        assert!(data.starts_with("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">"), "{name}");
        assert!(data.contains("<meta name=\"viewport\"") && data.contains("<title>"), "{name}");
        assert!(data.contains(&format!("terse:feed/{key}")) && data.contains("follow this feed in a Tersenet reader"), "{name}: the footer");
        assert_eq!(attrs(data, "href").iter().filter(|h| h.ends_with(".css")).count(), 1, "{name}: one stylesheet");
        for href in attrs(data, "href") {
            assert!(!href.contains(':') && !href.starts_with('/') && !href.contains("//"), "{name}: {href} is not relative");
            let (file, fragment) = href.split_once('#').unwrap_or((href, ""));
            let file = if file.is_empty() { name.as_str() } else { file };
            let target = page(file).unwrap_or_else(|| panic!("{name}: {href} leads nowhere"));
            if !fragment.is_empty() {
                assert!(target.contains(&format!(" id=\"{fragment}\"")), "{name}: {href}: no such id");
            }
        }
        let ids = attrs(data, "id");
        assert_eq!(ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len(), "{name}: ids are unique");
        let mut at = 0;
        while let Some(i) = data[at..].find("<svg") {
            let start = at + i;
            let end = start + data[start..].find("</svg>").unwrap() + 6;
            well_formed(&data[start..end]).unwrap_or_else(|e| panic!("{name}: figure: {e}"));
            at = end;
        }
    }
}
