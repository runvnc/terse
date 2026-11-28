//! The website (tersenet.org): the manual as static HTML, one page per
//! article of the built manual ([`crate::manual`]), parsed by the reference
//! article parser ([`crate::article_parse`]) and written as plain HTML, with
//! figures as inline SVG and one small stylesheet; and a front page.
//!
//! **Every page** comes from one template, [`page_shell`]: the same header
//! (the name, then Download, Manual, Try in browser and Source on GitHub)
//! and footer (licence, source, the public relay). Links within the site
//! are relative; the only addresses elsewhere are the project's on GitHub
//! (source, licence, downloads). Nothing is loaded from elsewhere: system
//! fonts, no tracking. The only scripts are the copy buttons (a few lines
//! inline; a page is the same without them, less the button) and the try
//! page's `try.js`, which loads the machine (`terse-web.wasm`) on Start.
//!
//! Pages:
//! - `index.html`, the front page ([`front_page`]): the project's paragraph
//!   (the first paragraph of the manual's `index` article), two pictures,
//!   the install command and the downloads of this version ([`VERSION`],
//!   the workspace's, so the links follow releases once the site is
//!   regenerated).
//! - `manual.html`, the manual's first page: the `index` article alone.
//! - `contents.html`, which lists every other article, and `<name>.html`
//!   for every other article, with the previous and next articles of the
//!   manifest below it. A link to an article of the manual leads to its
//!   page; any other `terse:` link cannot be followed on the web and is
//!   shown as its text and address.
//! - [`TRY_PAGE`] is linked from the header and made elsewhere.
//!
//! The output depends only on the articles and the version, so the same
//! sources give the same bytes.
//!
//! Besides the pages, [`site_dir`] copies the files of the manual's
//! `site-files/` directory (the install script the guide names, the front
//! page's pictures) to the site as they are, with one exception: an `.html`
//! file there that does not start with `<!doctype` is the inside of a page
//! (what goes in its `<main>`), written inside the template and titled by
//! its `<h1>` (a `<style>` at its start goes in the page's head, the
//! `<script>`s at its end after the footer). No subdirectories. When the
//! files include [`TRY_PAGE`] (the "try it in your browser" page, with its
//! script and `terse-web.wasm`), the site also has that page's content,
//! `try-content.bin` ([`crate::try_page`]).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::BlobId;
use crate::article_parse::{Block, Parsed, Target, parse};
use crate::manual::Built;
use crate::site_render::{Ctx, article, esc};

/// The stylesheet, `style.css`.
pub const STYLE: &str = include_str!("manual_site.css");

/// The version the downloads are of: the workspace's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The project's source.
pub const REPO: &str = "https://github.com/runvnc/terse";
/// The public relay, as typed.
pub const RELAY: &str = "relay.tersenet.org";
/// The front page.
pub const HOME_PAGE: &str = "index.html";
/// The manual's first page (the `index` article).
pub const MANUAL_PAGE: &str = "manual.html";
/// The page that runs Tersenet in the browser (made elsewhere; linked from
/// every page's header).
pub const TRY_PAGE: &str = "try.html";

/// The file of an article's page.
pub fn page_of(name: &str) -> String {
    match name {
        "index" => MANUAL_PAGE.into(),
        n => format!("{n}.html"),
    }
}

/// The site of a built manual: (file name, contents), the stylesheet
/// first, then the front page, then one page per article in manifest
/// order.
pub fn site(built: &[Built]) -> Vec<(String, String)> {
    let pages: HashMap<BlobId, usize> = built.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut out = vec![("style.css".to_string(), STYLE.to_string())];
    out.push((HOME_PAGE.to_string(), front_page(built)));
    for (i, b) in built.iter().enumerate() {
        out.push((page_of(&b.name), page(built, &pages, i)));
    }
    out
}

/// The directory of a manual's extra files, copied to its site.
pub const EXTRA_DIR: &str = "site-files";

/// The extra files of the manual in `dir` (its `site-files/`, if any):
/// (file name, contents), sorted by name; an `.html` fragment is written
/// inside the template (see the module's documentation).
pub fn extra_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let extra = dir.join(EXTRA_DIR);
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(&extra) else { return Ok(files) };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let mut data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if name.ends_with(".html") && !data.to_ascii_lowercase().starts_with(b"<!doctype") {
                let text = String::from_utf8(data).map_err(|e| format!("{}: {e}", path.display()))?;
                data = fragment_page(&name, &text).into_bytes();
            }
            files.push((name, data));
        }
    }
    files.sort();
    Ok(files)
}

/// A page made of a fragment from `site-files/`, titled by its `<h1>`: a
/// `<style>` at the fragment's start goes in the head, the `<script>`s at
/// its end after the footer.
fn fragment_page(name: &str, fragment: &str) -> String {
    let mut main = fragment.trim();
    let mut style = "";
    if main.starts_with("<style>") {
        if let Some(e) = main.find("</style>") {
            style = &main[..e + "</style>".len()];
            main = main[style.len()..].trim_start();
        }
    }
    let mut scripts = "";
    if let Some(i) = main.find("<script") {
        if main[i..].trim_end().ends_with("</script>") {
            scripts = &main[i..];
            main = main[..i].trim_end();
        }
    }
    let h1 = main
        .find("<h1")
        .and_then(|i| main[i..].find('>').map(|j| i + j + 1))
        .and_then(|s| main[s..].find("</h1>").map(|e| &main[s..s + e]))
        .unwrap_or("");
    // The heading's text, without its tags.
    let mut title = String::new();
    let mut in_tag = false;
    for c in h1.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => title.push(c),
            _ => {}
        }
    }
    let title = if title.is_empty() || title == "Tersenet" { "Tersenet".to_string() } else { format!("{title} - Tersenet") };
    let section = if name == TRY_PAGE { Section::Try } else { Section::Other };
    let class = if name == TRY_PAGE { "try" } else { "" };
    let after = if scripts.is_empty() { String::new() } else { format!("{scripts}\n") };
    let page = page_shell(&title, "", section, class, main, &after);
    if style.is_empty() { page } else { page.replacen("</head>", &format!("{style}\n</head>"), 1) }
}

/// The files besides the pages: the extra files of [`extra_files`], and
/// the try page's content if they have the page.
pub fn other_files(dir: &Path, built: &[Built]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = extra_files(dir)?;
    if files.iter().any(|(n, _)| n == TRY_PAGE) {
        files.push((crate::try_page::CONTENT.to_string(), crate::try_page::content(built)));
    }
    Ok(files)
}

/// Builds the manual in `dir` and writes its site to `out`, with the files
/// of [`other_files`]; `.html` files of articles no longer listed are
/// removed. Returns the files written.
pub fn site_dir(dir: &Path, out: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let built = crate::manual::build(&crate::manual::read_sources(dir)?)?;
    let mut files: Vec<(String, Vec<u8>)> = site(&built).into_iter().map(|(n, d)| (n, d.into_bytes())).collect();
    files.extend(other_files(dir, &built)?);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for entry in std::fs::read_dir(out).map_err(|e| format!("{}: {e}", out.display()))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".html") && !files.iter().any(|(f, _)| *f == name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    for (name, data) in &files {
        let path = out.join(name);
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(files)
}

// --- The template --------------------------------------------------------------

/// Which part of the site a page belongs to (its link in the header is
/// marked as the current page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Home,
    Manual,
    Try,
    Other,
}

/// The header of every page: the name (a link to the front page), then
/// the sections.
pub fn header(current: Section) -> String {
    let mark = |s: Section| if s == current { " aria-current=\"page\"" } else { "" };
    format!(
        "<header class=\"site\"><nav>\
         <a href=\"{HOME_PAGE}\" class=\"home\"{}>Tersenet</a> \
         <a href=\"{HOME_PAGE}#download\">Download</a> \
         <a href=\"{MANUAL_PAGE}\"{}>Manual</a> \
         <a href=\"{TRY_PAGE}\"{}>Try in browser</a> \
         <a href=\"{REPO}\">Source on GitHub</a>\
         </nav></header>\n",
        mark(Section::Home),
        mark(Section::Manual),
        mark(Section::Try),
    )
}

/// The footer of every page.
pub fn footer() -> String {
    format!(
        "<footer class=\"site\"><p>Tersenet is free software under the \
         <a href=\"{REPO}/blob/main/LICENSE\">Apache License 2.0</a>. \
         <a href=\"{REPO}\">Source on GitHub</a>. \
         Public relay: <code>{RELAY}</code>. No cookies, no tracking.</p></footer>\n"
    )
}

/// A whole page: the head (title, the description unless empty,
/// stylesheet), [`header`], `<main>` (of the class `main_class` unless
/// empty) holding `main`, [`footer`], then `after` (a page's script, if
/// any).
pub fn page_shell(title: &str, description: &str, current: Section, main_class: &str, main: &str, after: &str) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n",
        esc(title)
    );
    if !description.is_empty() {
        let _ = writeln!(out, "<meta name=\"description\" content=\"{}\">", esc(description));
    }
    out.push_str("<link rel=\"stylesheet\" href=\"style.css\">\n</head>\n<body>\n");
    out.push_str(&header(current));
    if main_class.is_empty() {
        out.push_str("<main>\n");
    } else {
        let _ = writeln!(out, "<main class=\"{main_class}\">");
    }
    out.push_str(main);
    out.push_str("\n</main>\n");
    out.push_str(&footer());
    out.push_str(after);
    out.push_str("</body>\n</html>\n");
    out
}

// --- The front page --------------------------------------------------------------

/// A download of this version: its address.
pub fn download_url(file: &str) -> String {
    format!("{REPO}/releases/download/v{VERSION}/{file}")
}

/// The downloads of this version, per system: (system, [(what, file)], a
/// note as HTML).
pub fn downloads() -> Vec<(&'static str, Vec<(&'static str, String)>, String)> {
    let v = VERSION;
    vec![
        (
            "Windows",
            vec![("Windows 10 or newer, 64-bit", format!("terse-{v}-windows-x86_64.zip"))],
            "Extract all, then double-click <code>terse-host.exe</code>. It is not signed, so SmartScreen \
             may say &#8220;Windows protected your PC&#8221;: click <b>More info</b>, then <b>Run anyway</b>."
                .into(),
        ),
        (
            "macOS",
            vec![("macOS 11 or newer, Apple silicon and Intel", format!("terse-{v}-macos.zip"))],
            format!(
                "It is not notarised by Apple, so macOS blocks it the first time. Unzip it, run \
                 <code>xattr -dr com.apple.quarantine ~/Downloads/terse-{v}-macos</code> in Terminal, then \
                 double-click <code>Terse.app</code>. Or Control-click <code>Terse.app</code> and choose Open \
                 (on macOS 15: try once, then System Settings, Privacy &amp; Security, Open Anyway)."
            ),
        ),
        (
            "Linux",
            vec![
                ("x86_64", format!("terse-{v}-linux-x86_64.tar.gz")),
                ("aarch64 (64-bit ARM)", format!("terse-{v}-linux-aarch64.tar.gz")),
            ],
            "Ubuntu 22.04, Debian 12, Fedora 36 or newer (glibc 2.35), on Wayland or X11. Unpack it with \
             <code>tar xzf</code> and run <code>terse-host</code>, or use the install command above."
                .into(),
        ),
        (
            "Terminal reader",
            vec![
                ("Linux x86_64", format!("tersereader-{v}-linux-amd64.tar.gz")),
                ("Linux ARM 64-bit", format!("tersereader-{v}-linux-arm64.tar.gz")),
                ("macOS Intel", format!("tersereader-{v}-macos-amd64.tar.gz")),
                ("macOS Apple silicon", format!("tersereader-{v}-macos-arm64.tar.gz")),
            ],
            "<code>tersereader</code> reads Tersenet in a terminal: one static program, nothing else to \
             install. There is no Windows build."
                .into(),
        ),
        (
            "Checksums",
            vec![("the SHA-256 of every file above", "SHA256SUMS.txt".to_string())],
            "Check a download with <code>sha256sum -c --ignore-missing SHA256SUMS.txt</code> (Linux), \
             <code>shasum -a 256 -c --ignore-missing SHA256SUMS.txt</code> (macOS) or \
             <code>Get-FileHash</code> in PowerShell (Windows)."
                .into(),
        ),
    ]
}

/// The picture on the front page: (file in `site-files/`, width, height,
/// what it shows).
pub const SHOT: (&str, u32, u32, &str) =
    ("shot-reader.webp", 800, 600, "A Tersenet article in the reader: a paragraph, a line diagram and a table.");

/// The front page's title (the browser's tab, search results).
pub const FRONT_TITLE: &str = "Tersenet: signed plain-text articles, read without a browser";
/// The front page's description (search results).
pub const FRONT_DESCRIPTION: &str = "Tersenet is a small network of signed plain-text articles, with tables and \
     diagrams, that you read in a program that runs no scripts and cannot load anything from the web.";

/// Where the release stands: the version it is about (`major.minor`, which
/// [`VERSION`] must start with: a new minor version needs a new line) and
/// the line, as HTML.
pub const STATUS: (&str, &str) =
    ("0.9", "0.9 is the first public release. It works and is tested; formats may still change before 1.0.");

/// The install command (Linux, macOS).
pub const INSTALL: &str = "curl -fsSL https://tersenet.org/install.sh | sh";

/// The copy button's script: it shows the buttons (hidden without
/// scripts) and copies their command.
const COPY_SCRIPT: &str = "<script>\
for(const b of document.querySelectorAll(\"button[data-copy]\")){b.hidden=false;\
b.onclick=()=>navigator.clipboard.writeText(document.getElementById(b.dataset.copy).textContent)\
.then(()=>{b.textContent=\"Copied\";setTimeout(()=>b.textContent=\"Copy\",1500)})}\
</script>\n";

/// The front page, `index.html`.
pub fn front_page(built: &[Built]) -> String {
    let resolve = |_: &Target| None;
    let ctx = Ctx { resolve: &resolve };
    // The project's paragraph: the first paragraph of the manual's first page.
    let blocks = match built.iter().find(|b| b.name == "index").map(|b| parse(b.text.as_bytes())) {
        Some(Parsed::Article(blocks)) => blocks,
        _ => Vec::new(),
    };
    let lede: Vec<Block> = blocks.into_iter().filter(|b| matches!(b, Block::Paragraph(_))).take(1).collect();
    let lede = article(&ctx, &lede, &mut HashSet::new()).trim_end().replacen("<p>", "<p class=\"lede\">", 1);
    let v = VERSION;
    let all = downloads();
    let file_of = |system: &str, i: usize| download_url(&all.iter().find(|d| d.0 == system).unwrap().1[i].1);
    let (shot, w, h, alt) = SHOT;
    let mut m = String::new();
    let _ = write!(
        m,
        "<h1>Tersenet</h1>\n{lede}\n\
         <p><a href=\"{TRY_PAGE}\">Try it in your browser</a>: the reader and a few articles, nothing to install.</p>\n\
         <p id=\"install\">Install on Linux or macOS:</p>\n\
         <div class=\"cmd\"><pre class=\"sh\"><code id=\"install-cmd\">{install}</code></pre>\
         <button type=\"button\" data-copy=\"install-cmd\" hidden>Copy</button></div>\n\
         <p>Download: <a href=\"{win}\">Windows (zip)</a> &#183; <a href=\"{mac}\">macOS (zip)</a> \
         &#183; <a href=\"{lx}\">Linux x86_64</a> &#183; <a href=\"{la}\">Linux ARM64</a> \
         &#183; <a href=\"#dl-reader\">terminal reader</a></p>\n\
         <p class=\"small\">Version {v}. The <a href=\"guide-install.html\">install guide</a> says what the \
         script does and how to open the unsigned files.</p>\n\
         <figure class=\"shot\"><img src=\"{shot}\" width=\"{w}\" height=\"{h}\" alt=\"{alt}\"></figure>\n\
         <p>Something to read now: <a href=\"{MANUAL_PAGE}\">the manual</a>, itself written as Tersenet \
         articles, the same pages the reader shows.</p>\n\
         <h2 id=\"download\">Download {v}</h2>\n<div class=\"downloads\">\n",
        install = esc(INSTALL),
        win = file_of("Windows", 0),
        mac = file_of("macOS", 0),
        lx = file_of("Linux", 0),
        la = file_of("Linux", 1),
        alt = esc(alt),
    );
    for (system, files, note) in &all {
        let id = match *system {
            "Terminal reader" => "dl-reader".to_string(),
            s => format!("dl-{}", s.to_lowercase()),
        };
        let _ = write!(m, "<section id=\"{id}\">\n<h3>{system}</h3>\n<ul>\n");
        for (what, file) in files {
            let _ = writeln!(m, "<li><a href=\"{}\">{file}</a><br><span>{what}</span></li>", download_url(file));
        }
        let _ = write!(m, "</ul>\n<p>{note}</p>\n</section>\n");
    }
    let _ = write!(
        m,
        "</div>\n<p>{} <a href=\"{REPO}/releases/tag/v{v}#known-limits\">Known limits</a>.</p>\n\
         <p><a href=\"{REPO}/releases/tag/v{v}\">What is new in {v}</a> \
         &#183; <a href=\"{REPO}/releases\">All releases</a></p>",
        STATUS.1
    );
    page_shell(FRONT_TITLE, FRONT_DESCRIPTION, Section::Home, "front", &m, COPY_SCRIPT)
}

// --- Pages ---------------------------------------------------------------------

fn page(built: &[Built], pages: &HashMap<BlobId, usize>, i: usize) -> String {
    // A link to an article of the manual leads to its page.
    let resolve = |t: &Target| match t {
        Target::Article(id) => pages.get(id).map(|&i| (page_of(&built[i].name), built[i].title.clone())),
        _ => None,
    };
    let ctx = Ctx { resolve: &resolve };
    let b = &built[i];
    let blocks = match parse(b.text.as_bytes()) {
        Parsed::Article(blocks) => blocks,
        _ => Vec::new(),
    };
    let mut ids = HashSet::new();
    let first = b.name == "index";
    let title = if first { "The Tersenet manual".to_string() } else { format!("{} - the Tersenet manual", b.title) };
    let mut m = String::from("<article>\n");
    m.push_str(&article(&ctx, &blocks, &mut ids));
    m.push_str("</article>");
    if !first {
        m.push_str("\n<nav class=\"pn\">");
        if i > 0 {
            let p = &built[i - 1];
            let _ = write!(m, "<a rel=\"prev\" href=\"{}\">← {}</a>", page_of(&p.name), esc(&p.title));
        }
        if let Some(n) = built.get(i + 1) {
            let _ = write!(m, "<a rel=\"next\" href=\"{}\">{} →</a>", page_of(&n.name), esc(&n.title));
        }
        m.push_str("</nav>");
    }
    page_shell(&title, "", Section::Manual, if first { "landing" } else { "" }, &m, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::build;

    fn src(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect()
    }

    #[test]
    fn pages_links_and_escaping() {
        let built = build(&src(&[
            ("index", "# What <it> is\n\nSee terse:{b} & https://x.org.\n\n[Contents](terse:{contents})\n"),
            ("contents", "# Contents\n\n- [B](terse:{b})\n"),
            ("b", "# B\n\n[F](terse:feed/0000000000000000000000000000000000000000000000000000000000000000)\n\n| a | b |\n|---|--:|\n| `x\\|y` | 2 |\n\n```path 30 15\nfill #ff000080 stroke fg 1.5 M0 0 L30 15 Z\n```\n"),
        ]))
        .unwrap();
        let files = site(&built);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["style.css", "index.html", "manual.html", "contents.html", "b.html"]);
        let nav = header(Section::Other);
        // The front page: the index article's first paragraph, the
        // downloads of this version, the same header and footer.
        let front = &files[1].1;
        assert!(front.contains(&format!("<title>{FRONT_TITLE}</title>")), "{front}");
        assert!(front.contains("<meta name=\"description\" content=\"Tersenet is a small network"), "{front}");
        assert!(VERSION.starts_with(&format!("{}.", STATUS.0)), "STATUS is about {}, the version is {VERSION}", STATUS.0);
        assert!(front.contains(STATUS.1), "{front}");
        assert!(front.contains("<p class=\"lede\">See <code class=\"addr\">terse:"), "{front}");
        assert!(front.contains(&format!("{REPO}/releases/download/v{VERSION}/terse-{VERSION}-windows-x86_64.zip")));
        assert!(front.contains("<a href=\"index.html\" class=\"home\" aria-current=\"page\">Tersenet</a>"), "{front}");
        assert!(front.contains(&footer()));
        // The manual's first page is the index article alone, below the header.
        let first = &files[2].1;
        assert!(first.contains("<title>The Tersenet manual</title>"), "{first}");
        assert!(first.contains("<a href=\"b.html\" title=\"B\">terse:"), "{first}");
        assert!(first.contains("&amp; https&#58;//x.org."), "{first}");
        assert!(first.contains("<a href=\"contents.html\">Contents</a>"), "{first}");
        assert!(first.contains("<a href=\"manual.html\" aria-current=\"page\">Manual</a>"), "{first}");
        assert!(!first.contains("<li>") && !first.contains("rel=\"next\""), "{first}");
        let contents = &files[3].1;
        assert!(contents.contains("<title>Contents - the Tersenet manual</title>"), "{contents}");
        assert!(contents.contains("<li><a href=\"b.html\">B</a></li>"), "{contents}");
        assert!(contents.contains("<a rel=\"prev\" href=\"manual.html\">") && contents.contains("<a rel=\"next\" href=\"b.html\">"));
        let b = &files[4].1;
        assert!(b.contains("F (<code class=\"addr\">terse:feed/0000"), "{b}");
        assert!(b.contains("<th class=\"ae\">b</th>") && b.contains("<code>x|y</code>"), "{b}");
        assert!(b.contains("<path d=\"M0 0 L30 15 Z\" fill=\"#ff0000\" fill-opacity=\"0.502\" stroke=\"currentColor\" stroke-width=\"1.5\""), "{b}");
        assert!(b.contains("style=\"width:2em\""), "{b}");
        assert!(b.contains("<a rel=\"prev\" href=\"contents.html\">") && !b.contains("rel=\"next\""));
        for (name, html) in &files[1..] {
            let plain = nav.replace(" aria-current=\"page\"", "");
            assert!(html.replace(" aria-current=\"page\"", "").contains(&plain), "{name}: the header");
            assert!(html.contains(&footer()), "{name}: the footer");
        }
        assert_eq!(site(&built), files, "deterministic");
    }

    #[test]
    fn fragments_are_written_in_the_template() {
        let page = fragment_page("try.html", "<h1>Try <i>it</i></h1>\n<p>Hi</p>\n");
        assert!(page.starts_with("<!doctype html>"), "{page}");
        assert!(page.contains("<title>Try it - Tersenet</title>"), "{page}");
        assert!(page.contains("<a href=\"try.html\" aria-current=\"page\">Try in browser</a>"), "{page}");
        assert!(page.contains("<main>\n<h1>Try <i>it</i></h1>\n<p>Hi</p>\n</main>\n"), "{page}");
        assert!(page.contains(&footer()));
    }
}
