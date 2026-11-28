//! The manual (docs/manual): its build is up to date and deterministic,
//! its links resolve to blobs every host carries, it covers every call,
//! event, content operation and status of the machine specification
//! (SPEC.md and spec/), and `terse manual
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

/// The rows of the Markdown table of the section `heading` (up to the
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

/// Every call (number and name) of SPEC §index.calls, every event type of SPEC §index.events,
/// every content operation (SPEC §content.request, §content.feeds, §content.records) and status (SPEC §content.status) is
/// on one line of the manual with its name, so the manual cannot fall
/// behind the spec without this failing.
#[test]
fn covers_the_spec() {
    let spec = std::fs::read_to_string(common::workspace().join("SPEC.md")).unwrap();
    let content = std::fs::read_to_string(common::workspace().join("spec/content.md")).unwrap();
    let lines: Vec<String> = builtin::MANUAL
        .iter()
        .flat_map(|(_, _, t)| std::str::from_utf8(t).unwrap().lines().map(str::to_lowercase).collect::<Vec<_>>())
        .collect();
    let find = |what: &str, number: &str, name: &str| {
        let (n, m) = (number.to_lowercase(), name.to_lowercase());
        assert!(lines.iter().any(|l| l.contains(&n) && l.contains(&m)), "{what} {number} {name} is not in the manual");
    };
    let calls = spec_rows(&spec, "### §index.calls Calls");
    assert!(calls.len() >= 47, "{}", calls.len());
    for (n, name) in &calls {
        find("call", n, name);
    }
    let events = spec_rows(&spec, "### §index.events Events");
    assert_eq!(events.len(), 16);
    for (n, name) in &events {
        find("event", n, name);
    }
    let mut ops = vec![("01h".to_string(), "get".to_string()), ("02h".to_string(), "put".to_string())];
    ops.extend(spec_rows(&content, "## §content.feeds Feeds, names and search"));
    ops.extend(spec_rows(&content, "## §content.records Named records"));
    assert_eq!(ops.len(), 18);
    for (n, name) in &ops {
        find("content op", n, name);
    }
    let statuses = spec_rows(&content, "## §content.status Status values");
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

// --- The website (site/) ------------------------------------------------------

fn site_dir() -> PathBuf {
    common::workspace().join("site")
}

/// The checked-in site is what the sources make, file for file, and
/// `terse manual site` writes the same bytes.
#[test]
fn site_is_up_to_date() {
    let built = manual::build(&manual::read_sources(&dir()).unwrap()).unwrap();
    let mut files: Vec<(String, Vec<u8>)> =
        terse_net::manual_site::site(&built).into_iter().map(|(n, d)| (n, d.into_bytes())).collect();
    // The extra files (docs/manual/site-files: the install script, the
    // front page's picture) are copied as they are.
    files.extend(terse_net::manual_site::extra_files(&dir()).unwrap());
    let stale = "site/ is stale: run `terse manual site --out site`";
    let mut on_disk: Vec<String> =
        std::fs::read_dir(site_dir()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    on_disk.sort();
    let mut names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    assert_eq!(on_disk, names, "{stale}");
    for (name, data) in &files {
        assert!(std::fs::read(site_dir().join(name)).unwrap() == *data, "{name}: {stale}");
    }
    // Every manifest article has a page.
    for b in &built {
        assert!(names.contains(&terse_net::manual_site::page_of(&b.name)), "{}", b.name);
    }
    let tmp = tempfile::tempdir().unwrap();
    let out = terse(&["manual", "site", "--dir", dir().to_str().unwrap(), "--out", tmp.path().to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for (name, data) in &files {
        assert!(std::fs::read(tmp.path().join(name)).unwrap() == *data, "{name}");
    }
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

/// Whether `xml` is well formed, as far as a figure needs: every tag is
/// closed in order, attributes are quoted, no stray `<`, `>` or `&`.
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
        // name="value" pairs.
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

#[test]
fn well_formed_catches_mistakes() {
    assert!(well_formed("<svg a=\"1\"><path d=\"M0 0\"/></svg>").is_ok());
    assert!(well_formed("<svg><path d=\"M0 0\"></svg>").is_err());
    assert!(well_formed("<svg a=1></svg>").is_err());
    assert!(well_formed("<svg>a > b</svg>").is_err());
}

/// The site's pages and stylesheet (its text files), by name; the
/// pictures are left out.
fn site_pages() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = std::fs::read_dir(site_dir())
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.ends_with(".html") || n.ends_with(".css")
        })
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap()))
        .collect();
    files.sort();
    files
}

/// The site is self-contained: every link leads to a page of it (and a
/// fragment to an id there) or to the project on GitHub, nothing is
/// loaded from elsewhere, the only script is the front page's copy
/// button, and every figure is well-formed SVG.
#[test]
fn site_is_self_contained() {
    use terse_net::manual_site::{HOME_PAGE, MANUAL_PAGE, REPO, TRY_PAGE};
    let files = site_pages();
    let page = |name: &str| files.iter().find(|(n, _)| n == name).map(|(_, d)| d.as_str());
    let exists = |name: &str| site_dir().join(name).is_file();
    let css = page("style.css").unwrap();
    assert!(css.len() < 8192, "a small stylesheet");
    let mut figures = 0;
    for (name, html) in &files {
        let lower = html.to_lowercase();
        for bad in ["javascript:", "http://", "@import", "url(", "<iframe", "<object", "<embed", "<link rel=\"pre"] {
            assert!(!lower.contains(bad), "{name} contains {bad}");
        }
        if !name.ends_with(".html") {
            continue;
        }
        // Scripts: only the front page's, inline.
        let scripts = lower.matches("<script").count();
        if name == HOME_PAGE {
            assert_eq!(scripts, 1, "{name}: the copy button's script");
            assert!(lower.contains("<script>"), "{name}: inline");
        } else {
            assert_eq!(scripts, 0, "{name} has a script");
        }
        // Web addresses only in links to the project.
        let repo_links = attrs(html, "href").iter().filter(|h| h.starts_with(REPO)).count();
        assert_eq!(lower.matches("https://").count(), repo_links, "{name}: a web address other than {REPO}");
        // Pictures only from the site.
        for src in attrs(html, "src") {
            assert!(!src.contains(':') && !src.contains('/') && exists(src), "{name}: {src}");
        }
        assert!(html.starts_with("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">"), "{name}");
        assert!(html.contains("<meta name=\"viewport\"") && html.contains("<title>"), "{name}");
        assert_eq!(attrs(html, "href").iter().filter(|h| h.ends_with(".css")).count(), 1, "{name}: one stylesheet");
        for href in attrs(html, "href") {
            if href.starts_with(REPO) {
                continue;
            }
            assert!(!href.contains(':') && !href.starts_with('/') && !href.contains("//"), "{name}: {href} is not relative");
            let (file, fragment) = href.split_once('#').unwrap_or((href, ""));
            let file = if file.is_empty() { name.as_str() } else { file };
            if file == TRY_PAGE && page(file).is_none() {
                continue; // made in another branch; linked from every header already
            }
            let target = page(file).unwrap_or_else(|| panic!("{name}: {href} leads nowhere"));
            if !fragment.is_empty() {
                assert!(target.contains(&format!(" id=\"{fragment}\"")), "{name}: {href}: no such id");
            }
        }
        let ids = attrs(html, "id");
        assert_eq!(ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len(), "{name}: ids are unique");
        let mut at = 0;
        while let Some(i) = html[at..].find("<svg") {
            let start = at + i;
            let end = start + html[start..].find("</svg>").unwrap() + 6;
            well_formed(&html[start..end]).unwrap_or_else(|e| panic!("{name}: figure: {e}"));
            figures += 1;
            at = end;
        }
    }
    // Every figure of the manual is drawn (the indented sources of
    // figures in writing-articles and device-page-figures are not figures).
    let sources: usize =
        builtin::MANUAL.iter().map(|(_, _, t)| std::str::from_utf8(t).unwrap().matches("\n```path ").count()).sum();
    assert!(sources >= 4);
    assert_eq!(figures, sources, "every figure is drawn");
    // The manual's first page is the index article alone: its heading, the
    // paragraph and three links (the user guide's first page, the
    // implementation guide's, then the contents); no list of articles.
    let first = page(MANUAL_PAGE).unwrap();
    let (name, title, _) = builtin::MANUAL[0];
    assert_eq!(name, "index");
    let article = &first[first.find("<article>").unwrap()..first.find("</article>").unwrap()];
    assert!(article.contains(&format!(">{title}</h1>")), "{article}");
    let links: Vec<&str> = attrs(article, "href").into_iter().collect();
    assert_eq!(
        links,
        ["guide-install.html", "guide-reader.html", "contents.html"],
        "the manual's first page links to the two guides and the contents only"
    );
    assert!(!article.contains("<li>") && !first.contains("rel=\"next\""), "{first}");
    // The front page opens with the same paragraph.
    let index = std::str::from_utf8(builtin::MANUAL[0].2).unwrap();
    let paragraph = index.split("\n\n").nth(1).unwrap().replace('\n', " ");
    assert!(page(HOME_PAGE).unwrap().contains(&format!("<p class=\"lede\">{}</p>", paragraph.trim())), "the project's paragraph");
    // The contents page lists every other article, the guide first.
    let contents = page("contents.html").unwrap();
    let listed: Vec<&str> = attrs(contents, "href").into_iter().filter(|h| h.ends_with(".html")).collect();
    for (name, _, _) in &builtin::MANUAL[1..] {
        if *name != "contents" {
            assert!(listed.contains(&terse_net::manual_site::page_of(name).as_str()), "{name} is listed on contents.html");
        }
    }
    let at = |h: &str| contents.find(&format!("href=\"{h}\">")).unwrap();
    assert!(at("guide-install.html") < at("guide-reader.html"), "the user guide comes before the implementation guide");
    assert!(at("guide-reader.html") < at("how-it-works.html"), "the guides come before the reference");
}

/// Every page has the site's header (its navigation) and footer.
#[test]
fn every_page_has_the_header() {
    use terse_net::manual_site::{HOME_PAGE, MANUAL_PAGE, REPO, Section, TRY_PAGE, footer, header};
    let nav = header(Section::Other);
    let pages: Vec<(String, String)> = site_pages().into_iter().filter(|(n, _)| n.ends_with(".html")).collect();
    assert!(pages.len() > 50);
    for (name, html) in &pages {
        let plain = html.replace(" aria-current=\"page\"", "");
        assert!(plain.contains(&nav), "{name}: the header");
        assert!(html.contains(&footer()), "{name}: the footer");
        let hrefs = attrs(html, "href");
        for want in [HOME_PAGE.to_string(), format!("{HOME_PAGE}#download"), MANUAL_PAGE.into(), TRY_PAGE.into(), REPO.into()] {
            assert!(hrefs.contains(&want.as_str()), "{name}: no link to {want}");
        }
    }
}

/// The front page's downloads are this version's files, at the release's
/// address on GitHub; the install command and the picture are there.
#[test]
fn front_page_downloads_this_version() {
    use terse_net::manual_site::{HOME_PAGE, REPO, SHOT, VERSION};
    // The workspace's version, read from Cargo.toml itself.
    let cargo = std::fs::read_to_string(common::workspace().join("Cargo.toml")).unwrap();
    let v = cargo
        .lines()
        .skip_while(|l| l.trim() != "[workspace.package]")
        .find_map(|l| l.strip_prefix("version = \""))
        .and_then(|l| l.strip_suffix('"'))
        .unwrap();
    assert_eq!(v, VERSION);
    let html = std::fs::read_to_string(site_dir().join(HOME_PAGE)).unwrap();
    let base = format!("{REPO}/releases/download/v{v}/");
    for file in [
        format!("terse-{v}-windows-x86_64.zip"),
        format!("terse-{v}-macos.zip"),
        format!("terse-{v}-linux-x86_64.tar.gz"),
        format!("terse-{v}-linux-aarch64.tar.gz"),
        format!("tersereader-{v}-linux-amd64.tar.gz"),
        format!("tersereader-{v}-linux-arm64.tar.gz"),
        format!("tersereader-{v}-macos-amd64.tar.gz"),
        format!("tersereader-{v}-macos-arm64.tar.gz"),
        "SHA256SUMS.txt".to_string(),
    ] {
        assert!(html.contains(&format!("href=\"{base}{file}\">{file}</a>")), "no download of {file}");
    }
    // Every download is of this version.
    for href in attrs(&html, "href").into_iter().filter(|h| h.contains("/releases/download/")) {
        assert!(href.starts_with(&base), "{href} is not of {v}");
        let file = &href[base.len()..];
        assert!(file == "SHA256SUMS.txt" || file.contains(&format!("-{v}-")), "{href}");
    }
    assert!(html.contains(&format!("href=\"{REPO}/releases\">All releases</a>")));
    assert!(html.contains("<code id=\"install-cmd\">curl -fsSL https&#58;//tersenet.org/install.sh | sh</code>"));
    assert!(html.contains("<button type=\"button\" data-copy=\"install-cmd\" hidden>"), "the copy button is hidden without scripts");
    assert!(site_dir().join(SHOT.0).is_file() && html.contains(&format!("<img src=\"{}\"", SHOT.0)));
    // Small: the page, the stylesheet and the picture.
    let weight = html.len()
        + std::fs::metadata(site_dir().join("style.css")).unwrap().len() as usize
        + std::fs::metadata(site_dir().join(SHOT.0)).unwrap().len() as usize;
    assert!(weight < 120_000, "the front page weighs {weight} bytes");
}
