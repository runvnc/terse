//! A feed as a static website: one HTML page per article entry (kind 1),
//! drawn by the manual site's renderer ([`crate::site_render`]), an index
//! newest first in pages, a page listing the feed's programs, the shared
//! stylesheet and, given the site's own address, an Atom feed with each
//! article's whole text. No scripts, no external requests, and no web
//! address in any page; the Atom feed holds the site's address (its links
//! must be absolute) and the Atom namespace name.
//!
//! **Incremental.** The site is regenerated on every run, but a run writes
//! only what changed, and reads only the blobs of the pages it writes. A
//! state file in the output directory ([`STATE_FILE`]) records, for each
//! article, its entry (seq, time, title, blob), its page, the articles its
//! links lead to and the hash of the inputs its page was made from; and
//! for every other file, the hash of its contents.
//!
//! - **Article pages** are named by the first 16 hex digits of the blob ID
//!   (`<hex16>.html`; the whole ID if another article already has that
//!   name), so a link, which names a blob, finds its page without a table,
//!   and an article published twice has one page. The name is kept in the
//!   state for good. A page is a function of the blob, the entry's title
//!   and time, the site options (generator version, title, base URL, feed
//!   address) and **the links of the article that resolve within the
//!   site**, with the page and title they lead to. No page lists other
//!   articles or links to its neighbours, so a new article changes no
//!   old page, except:
//! - **Links.** A link to an article of this feed leads to its page if
//!   the site has that article, whenever it was published; otherwise it is
//!   shown as its address. An article linking to one that arrives later is
//!   therefore rewritten once, when that article arrives: its page's
//!   inputs hash covers its resolved links, worked out from the link list
//!   kept in the state, without reading its blob.
//! - **The index** is numbered from the oldest: `page-1.html` holds the
//!   first [`PER_PAGE`] articles in seq order (shown newest first) and
//!   never changes once full; `index.html` shows the newest [`PER_PAGE`]
//!   and lists the pages. A new article changes `index.html` and the last
//!   page (or starts a new one).
//! - **feed.xml** holds the newest [`FEED_ITEMS`] articles, their HTML
//!   taken from their pages, so writing it reads no blob.
//! - Every file whose inputs are unchanged and that exists is left alone;
//!   a new generator version, other options or `force` rewrite them all.
//! - The network cannot withdraw an entry (NETWORK 4.2), so no page is
//!   ever removed: an article the store or relays no longer hold (a relay
//!   keeps only the newest entries of a feed) stays on the site, from the
//!   state file.
//!
//! The same inputs give the same bytes: nothing depends on when the site
//! is made.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::article_parse::{Parsed, Target, parse};
use crate::profile::Document;
use crate::site_render::{Ctx, address, article, article_links, esc};
use crate::{BlobId, parse_hex, to_hex};

/// The generator version: a change rewrites every page.
pub const GENERATOR: &str = "terse-feed-site-1";
/// The state file in the output directory.
pub const STATE_FILE: &str = ".terse-site";
/// Articles on each page of the index.
pub const PER_PAGE: usize = 20;
/// Articles in the Atom feed.
pub const FEED_ITEMS: usize = 20;
/// The rules added to the manual's stylesheet.
pub const STYLE_EXTRA: &str = include_str!("feed_site.css");

/// The stylesheet, `style.css`: the manual's and a few rules more.
pub fn style() -> String {
    format!("{}{STYLE_EXTRA}", crate::manual_site::STYLE)
}

/// An article entry (kind 1) of the feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedArticle {
    pub seq: u32,
    pub time: u32,
    pub title: String,
    pub blob: BlobId,
    pub size: u32,
}

/// What the site is made from.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub feed: [u8; 32],
    /// The feed's address as shown (`key[@hints]`, NETWORK 4.9).
    pub address: String,
    /// The article entries held, in any order.
    pub articles: Vec<FeedArticle>,
    /// The names of the feed's programs held.
    pub programs: Vec<String>,
    /// The profile in force: its blob and the display name (the entry's
    /// title).
    pub profile: Option<(BlobId, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The site's title (default: the profile's name).
    pub title: Option<String>,
    /// The site's own address, `http://` or `https://`; without it there
    /// is no Atom feed.
    pub base_url: Option<String>,
    /// Rewrite every file.
    pub force: bool,
}

/// What a run did.
#[derive(Debug, Default)]
pub struct Summary {
    /// Articles on the site.
    pub articles: usize,
    /// The files written, in the order written.
    pub written: Vec<String>,
    /// Files left as they were.
    pub unchanged: usize,
    /// Blobs read (from the store or a relay).
    pub blobs_read: usize,
    /// Why every file was rewritten, if it was (not on a first run).
    pub full: Option<String>,
    /// Articles that could not be read: (blob, why).
    pub missing: Vec<(BlobId, String)>,
}

impl Summary {
    /// The one-line summary.
    pub fn line(&self) -> String {
        format!(
            "{} articles, {} written, {} unchanged, {} blob{} read",
            self.articles,
            self.written.len(),
            self.unchanged,
            self.blobs_read,
            if self.blobs_read == 1 { "" } else { "s" }
        )
    }
}

/// Checks a base URL and gives it with a final `/`.
pub fn base_url(url: &str) -> Result<String, String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    let ok = rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/'))
        && url.bytes().all(|b| b.is_ascii_graphic() && !b"\"'<>&\\`{}|^#?".contains(&b));
    if !ok {
        return Err(format!("{url}: the base URL is the site's address, http:// or https://, without a query or fragment"));
    }
    Ok(if url.ends_with('/') { url.to_string() } else { format!("{url}/") })
}

// --- State ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rec {
    seq: u32,
    time: u32,
    title: String,
    blob: BlobId,
    page: String,
    /// The articles its links lead to.
    links: Vec<BlobId>,
    /// The inputs hash of the page as written, `None` if it must be.
    hash: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State {
    generator: String,
    options: String,
    /// The profile blob, name and description last used.
    profile: Option<(BlobId, String, String)>,
    programs: BTreeSet<String>,
    articles: BTreeMap<BlobId, Rec>,
    /// Other files: name to contents (or inputs) hash.
    files: BTreeMap<String, String>,
}

/// Text as one token of the state file: `%`, blanks and control
/// characters percent-encoded; an empty text is `%`.
fn enc(s: &str) -> String {
    if s.is_empty() {
        return "%".into();
    }
    let mut out = String::new();
    for c in s.chars() {
        if c == '%' || c == ' ' || c.is_control() {
            let mut b = [0; 4];
            for x in c.encode_utf8(&mut b).bytes() {
                let _ = write!(out, "%{x:02X}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn dec(s: &str) -> Option<String> {
    if s == "%" {
        return Some(String::new());
    }
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

impl State {
    fn to_text(&self) -> String {
        let mut out = String::from("terse-site-state 1\n");
        let _ = writeln!(out, "generator {}", enc(&self.generator));
        let _ = writeln!(out, "options {}", self.options);
        if let Some((blob, name, description)) = &self.profile {
            let _ = writeln!(out, "profile {} {} {}", to_hex(blob), enc(name), enc(description));
        }
        for p in &self.programs {
            let _ = writeln!(out, "program {}", enc(p));
        }
        let mut recs: Vec<&Rec> = self.articles.values().collect();
        recs.sort_by_key(|r| (r.seq, r.blob));
        for r in recs {
            let links: Vec<String> = r.links.iter().map(to_hex).collect();
            let _ = writeln!(
                out,
                "article {} {} {} {} {} {} {}",
                r.seq,
                r.time,
                to_hex(&r.blob),
                r.page,
                r.hash.as_deref().unwrap_or("-"),
                if links.is_empty() { "-".into() } else { links.join(",") },
                enc(&r.title)
            );
        }
        for (name, hash) in &self.files {
            let _ = writeln!(out, "file {name} {hash}");
        }
        out
    }

    fn parse(text: &str) -> Option<State> {
        let mut lines = text.lines();
        if lines.next()? != "terse-site-state 1" {
            return None;
        }
        let mut s = State::default();
        for line in lines {
            let w: Vec<&str> = line.split(' ').collect();
            match w.as_slice() {
                ["generator", g] => s.generator = dec(g)?,
                ["options", o] => s.options = o.to_string(),
                ["profile", b, n, d] => s.profile = Some((parse_hex(b)?, dec(n)?, dec(d)?)),
                ["program", n] => {
                    s.programs.insert(dec(n)?);
                }
                ["article", seq, time, blob, page, hash, links, title] => {
                    let blob = parse_hex(blob)?;
                    let links = if *links == "-" { Vec::new() } else { links.split(',').map(parse_hex).collect::<Option<Vec<_>>>()? };
                    if !safe_name(page) {
                        return None;
                    }
                    let rec = Rec {
                        seq: seq.parse().ok()?,
                        time: time.parse().ok()?,
                        title: dec(title)?,
                        blob,
                        page: page.to_string(),
                        links,
                        hash: (*hash != "-").then(|| hash.to_string()),
                    };
                    s.articles.insert(blob, rec);
                }
                ["file", name, hash] if safe_name(name) => {
                    s.files.insert(name.to_string(), hash.to_string());
                }
                _ => return None,
            }
        }
        Some(s)
    }
}

/// A file name the generator may write: no directory, nothing hidden.
fn safe_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
}

/// A hash of the parts, as 32 hex digits.
fn hash(parts: &[&[u8]]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p);
    }
    h.finalize().as_bytes()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// The page of a new article: the first 16 hex digits of its blob ID,
/// or the whole ID if that name is taken.
fn page_name(blob: &BlobId, taken: &HashSet<String>) -> String {
    let hex = to_hex(blob);
    let short = format!("{}.html", &hex[..16]);
    if taken.contains(&short) { format!("{hex}.html") } else { short }
}

// --- Dates -----------------------------------------------------------------------

/// (year, month, day, seconds of the day) in UTC.
fn civil(time: u32) -> (i64, i64, i64, u32) {
    let days = (time / 86_400) as i64;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day, time % 86_400)
}

/// `YYYY-MM-DD`, or `None` for time 0 (no time given).
fn date(time: u32) -> Option<String> {
    (time != 0).then(|| {
        let (y, m, d, _) = civil(time);
        format!("{y:04}-{m:02}-{d:02}")
    })
}

/// RFC 3339 in UTC.
fn rfc3339(time: u32) -> String {
    let (y, m, d, s) = civil(time);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

// --- Text ------------------------------------------------------------------------

/// Text from the network with control characters (other than newline and
/// tab) replaced.
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() && c != '\n' && c != '\t' { '\u{FFFD}' } else { c }).collect()
}

/// An entry's title for showing.
fn title_of(r: &Rec) -> String {
    let t = clean(&r.title);
    if t.trim().is_empty() { format!("Untitled (#{})", r.seq) } else { t }
}

/// Text escaped for XML.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// --- Pages -----------------------------------------------------------------------

struct Site<'a> {
    title: String,
    description: String,
    address: String,
    base: Option<&'a str>,
}

impl Site<'_> {
    fn head(&self, title: &str) -> String {
        let mut out = format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
             <title>{}</title>\n<link rel=\"stylesheet\" href=\"style.css\">\n",
            esc(title)
        );
        if self.base.is_some() {
            let _ = writeln!(out, "<link rel=\"alternate\" type=\"application/atom+xml\" href=\"feed.xml\" title=\"{}\">", esc(&self.title));
        }
        out.push_str("</head>\n<body>\n");
        out
    }

    fn header(&self, programs: bool) -> String {
        let mut out = format!("<header><nav><a href=\"index.html\" class=\"home\">{}</a>", esc(&self.title));
        if programs {
            out.push_str(" <a href=\"programs.html\">Programs</a>");
        }
        out.push_str("</nav></header>\n");
        out
    }

    /// The footer: `nav` (pager links, or empty), then the feed's address.
    fn footer(&self, nav: &str) -> String {
        let mut out = String::from("<footer>");
        if !nav.is_empty() {
            let _ = write!(out, "<nav class=\"pn\">{nav}</nav>");
        }
        let _ = write!(
            out,
            "<p class=\"feed\">This site is the feed <code class=\"addr\">terse:feed/{}</code>. \
             Read this without a browser: follow this feed in a Terse reader.</p></footer>\n</body>\n</html>\n",
            esc(&self.address)
        );
        out
    }

    fn list(&self, recs: &[&Rec]) -> String {
        let mut out = String::from("<ul class=\"posts\">\n");
        for r in recs.iter().rev() {
            let _ = write!(out, "<li><a href=\"{}\">{}</a>", r.page, esc(&title_of(r)));
            if let Some(d) = date(r.time) {
                let _ = write!(out, " <span class=\"date\">{d}</span>");
            }
            out.push_str("</li>\n");
        }
        out.push_str("</ul>\n");
        out
    }

    fn article_page(&self, r: &Rec, body: &str) -> String {
        let mut out = self.head(&format!("{} - {}", title_of(r), self.title));
        out.push_str(&self.header(false));
        out.push_str("<main>\n");
        if let Some(d) = date(r.time) {
            let _ = writeln!(out, "<p class=\"date\">{d}</p>");
        }
        out.push_str("<article>\n");
        out.push_str(body);
        out.push_str("</article>\n</main>\n");
        out.push_str(&self.footer("<a rel=\"index\" href=\"index.html\">All articles</a>"));
        out
    }

    /// Page `n` (from 1) of the index: articles `recs` (seq order).
    fn index_page(&self, n: usize, recs: &[&Rec]) -> String {
        let first = (n - 1) * PER_PAGE + 1;
        let mut out = self.head(&format!("{}: articles {first} to {}", self.title, first + recs.len() - 1));
        out.push_str(&self.header(false));
        let _ = write!(out, "<main>\n<h1>Articles {first} to {}</h1>\n", first + recs.len() - 1);
        out.push_str(&self.list(recs));
        out.push_str("</main>\n");
        let mut nav = String::new();
        if n > 1 {
            let _ = write!(nav, "<a rel=\"prev\" href=\"page-{}.html\">← Older</a>", n - 1);
        }
        nav.push_str("<a rel=\"next\" href=\"index.html\">Newest →</a>");
        out.push_str(&self.footer(&nav));
        out
    }

    fn index(&self, recs: &[&Rec], programs: bool) -> String {
        let mut out = self.head(&self.title);
        out.push_str(&self.header(programs));
        let _ = writeln!(out, "<main>\n<h1>{}</h1>", esc(&self.title));
        for para in self.description.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
            let _ = writeln!(out, "<p class=\"lede\">{}</p>", esc(&clean(para)));
        }
        if recs.is_empty() {
            out.push_str("<p>No articles yet.</p>\n");
        }
        out.push_str(&self.list(&recs[recs.len().saturating_sub(PER_PAGE)..]));
        let pages = recs.len().div_ceil(PER_PAGE);
        if pages > 1 {
            out.push_str("<p class=\"pages\">All articles, oldest first:");
            for n in 1..=pages {
                let _ = write!(out, " <a href=\"page-{n}.html\">{}</a>", (n - 1) * PER_PAGE + 1);
            }
            out.push_str("</p>\n");
        }
        out.push_str("</main>\n");
        out.push_str(&self.footer(""));
        out
    }

    fn programs(&self, feed: &[u8; 32], names: &BTreeSet<String>) -> String {
        let mut out = self.head(&format!("Programs - {}", self.title));
        out.push_str(&self.header(true));
        out.push_str(
            "<main>\n<h1>Programs</h1>\n<p>These programs run on a Terse machine, not in a web browser. \
             Open one in a Terse reader by its address.</p>\n<ul>\n",
        );
        for n in names {
            let a = address(&Target::Program(*feed, n.clone()));
            let _ = writeln!(out, "<li>{} <code class=\"addr\">{}</code></li>", esc(&clean(n)), esc(&a));
        }
        out.push_str("</ul>\n</main>\n");
        out.push_str(&self.footer(""));
        out
    }
}

/// The body of an article page as written: between `<article>` and
/// `</article>`.
fn body_of(page: &str) -> Option<&str> {
    let start = page.find("<article>\n")? + "<article>\n".len();
    let end = page.rfind("</article>")?;
    page.get(start..end)
}

/// The Atom feed: the newest articles with their HTML, links absolute.
fn atom(site: &Site, base: &str, feed: &[u8; 32], items: &[(&Rec, String)], author: &str) -> String {
    let updated = items.iter().map(|(r, _)| r.time).max().unwrap_or(0);
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    let _ = writeln!(out, "<title>{}</title>", xml(&site.title));
    if !site.description.trim().is_empty() {
        let _ = writeln!(out, "<subtitle>{}</subtitle>", xml(&clean(site.description.trim())));
    }
    let _ = writeln!(out, "<id>terse:feed/{}</id>", to_hex(feed));
    let _ = writeln!(out, "<updated>{}</updated>", rfc3339(updated));
    let _ = writeln!(out, "<author><name>{}</name></author>", xml(author));
    let _ = writeln!(out, "<link rel=\"self\" type=\"application/atom+xml\" href=\"{base}feed.xml\"/>");
    let _ = writeln!(out, "<link rel=\"alternate\" type=\"text/html\" href=\"{base}\"/>");
    let _ = writeln!(out, "<generator>Terse</generator>");
    for (r, body) in items.iter().rev() {
        let html = body.replace("<a href=\"", &format!("<a href=\"{base}"));
        let _ = write!(
            out,
            "<entry>\n<title>{}</title>\n<id>terse:{}</id>\n<updated>{}</updated>\n\
             <link rel=\"alternate\" type=\"text/html\" href=\"{base}{}\"/>\n<content type=\"html\">{}</content>\n</entry>\n",
            xml(&title_of(r)),
            to_hex(&r.blob),
            rfc3339(r.time),
            r.page,
            xml(&html)
        );
    }
    out.push_str("</feed>\n");
    out
}

// --- Generation --------------------------------------------------------------------

fn write_file(out: &Path, name: &str, data: &str) -> Result<(), String> {
    let path = out.join(name);
    let temp = out.join(format!(".{name}.tmp"));
    std::fs::write(&temp, data).and_then(|_| std::fs::rename(&temp, &path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes (or updates) the site of a feed in `out`. `fetch` reads a blob
/// (verified: its ID is its hash) from the store or a relay; it is called
/// only for the pages that must be written, and for a profile not seen
/// before.
pub fn generate(
    out: &Path,
    input: &Input,
    opts: &Options,
    fetch: &mut dyn FnMut(&BlobId) -> Result<Vec<u8>, String>,
) -> Result<Summary, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let state_path = out.join(STATE_FILE);
    let old_text = std::fs::read_to_string(&state_path).ok();
    let old = old_text.as_deref().and_then(State::parse);
    let mut sum = Summary::default();
    let base = opts.base_url.as_deref().map(base_url).transpose()?;

    // The profile: read only when it is not the one used last time (and
    // kept only once read).
    let mut profile_read = true;
    let profile = match &input.profile {
        None => None,
        Some((blob, name)) => match old.as_ref().and_then(|s| s.profile.clone()).filter(|(b, n, _)| b == blob && n == name) {
            Some(p) => Some(p),
            None => {
                sum.blobs_read += 1;
                let doc = fetch(blob).ok().map(|b| Document::parse(&b));
                profile_read = doc.is_some();
                let description = doc.flatten().map(|d| d.description).unwrap_or_default();
                Some((*blob, name.clone(), description))
            }
        },
    };
    let title = opts
        .title
        .clone()
        .or_else(|| profile.as_ref().map(|(_, n, _)| clean(n)))
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| format!("Feed {}", &to_hex(&input.feed)[..8]));
    let site = Site {
        title: title.clone(),
        description: profile.as_ref().map(|(_, _, d)| d.clone()).unwrap_or_default(),
        address: input.address.clone(),
        base: base.as_deref(),
    };
    let options = hash(&[GENERATOR.as_bytes(), title.as_bytes(), base.as_deref().unwrap_or("").as_bytes(), input.address.as_bytes(), &input.feed]);

    let mut state = old.clone().unwrap_or_default();
    sum.full = match &old {
        _ if opts.force => Some("--force".into()),
        None => None,
        Some(s) if s.generator != GENERATOR => Some("the generator changed".into()),
        Some(s) if s.options != options => Some("the site options changed (title, base URL or feed address)".into()),
        Some(_) => None,
    };
    if sum.full.is_some() || old.is_none() {
        for r in state.articles.values_mut() {
            r.hash = None;
        }
        state.files.clear();
    }
    state.generator = GENERATOR.into();
    state.options = options.clone();
    state.profile = profile.clone().filter(|_| profile_read);
    state.programs.extend(input.programs.iter().cloned());

    // New articles: read now (their links are needed to resolve the
    // others'); one that cannot be read is left out until it can.
    let mut fresh: HashMap<BlobId, Vec<u8>> = HashMap::new();
    let mut taken: HashSet<String> = state.articles.values().map(|r| r.page.clone()).collect();
    let mut new: Vec<&FeedArticle> = input.articles.iter().filter(|a| !state.articles.contains_key(&a.blob)).collect();
    new.sort_by_key(|a| (a.seq, a.blob));
    for a in new {
        if state.articles.contains_key(&a.blob) {
            continue; // published twice: the first entry counts
        }
        sum.blobs_read += 1;
        let bytes = match fetch(&a.blob) {
            Ok(b) if b.len() == a.size as usize => b,
            Ok(b) => {
                sum.missing.push((a.blob, format!("{} bytes, the entry says {}", b.len(), a.size)));
                continue;
            }
            Err(e) => {
                sum.missing.push((a.blob, e));
                continue;
            }
        };
        let links = match parse(&bytes) {
            Parsed::Article(blocks) => article_links(&blocks),
            _ => Vec::new(),
        };
        let page = page_name(&a.blob, &taken);
        taken.insert(page.clone());
        state.articles.insert(a.blob, Rec { seq: a.seq, time: a.time, title: a.title.clone(), blob: a.blob, page, links, hash: None });
        fresh.insert(a.blob, bytes);
    }

    // The pages links lead to.
    let pages: HashMap<BlobId, (String, String)> =
        state.articles.values().map(|r| (r.blob, (r.page.clone(), title_of(r)))).collect();
    let page_hash = |r: &Rec, links: &[BlobId]| {
        let mut parts: Vec<Vec<u8>> =
            vec![options.as_bytes().to_vec(), r.blob.to_vec(), r.page.as_bytes().to_vec(), r.title.as_bytes().to_vec(), r.time.to_le_bytes().to_vec()];
        for l in links {
            if let Some((p, t)) = pages.get(l) {
                parts.extend([l.to_vec(), p.as_bytes().to_vec(), t.as_bytes().to_vec()]);
            }
        }
        hash(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>())
    };
    let feed_key = input.feed;
    let resolve = |t: &Target| match t {
        Target::Article(id) => pages.get(id).cloned(),
        Target::Feed(k, _) if *k == feed_key => Some(("index.html".to_string(), title.clone())),
        _ => None,
    };

    let mut recs: Vec<Rec> = state.articles.values().cloned().collect();
    recs.sort_by_key(|r| (r.seq, r.blob));
    for r in &mut recs {
        let want = page_hash(r, &r.links);
        if r.hash.as_deref() == Some(want.as_str()) && out.join(&r.page).is_file() {
            sum.unchanged += 1;
            continue;
        }
        let bytes = match fresh.remove(&r.blob) {
            Some(b) => b,
            None => {
                sum.blobs_read += 1;
                match fetch(&r.blob) {
                    Ok(b) => b,
                    Err(e) => {
                        // Kept as it was; tried again next time.
                        sum.missing.push((r.blob, e));
                        continue;
                    }
                }
            }
        };
        let body = match parse(&bytes) {
            Parsed::Article(blocks) => {
                r.links = article_links(&blocks);
                article(&Ctx { resolve: &resolve }, &blocks, &mut HashSet::new())
            }
            _ => "<p>This entry is not an article that a web page can show.</p>\n".into(),
        };
        write_file(out, &r.page, &site.article_page(r, &body))?;
        sum.written.push(r.page.clone());
        r.hash = Some(page_hash(r, &r.links));
    }
    for r in &recs {
        state.articles.insert(r.blob, r.clone());
    }
    sum.articles = recs.len();

    // Every other file, written when its contents (or for feed.xml, its
    // inputs) changed.
    let shown: Vec<&Rec> = recs.iter().filter(|r| r.hash.is_some() || out.join(&r.page).is_file()).collect();
    let mut others: Vec<(String, String, Option<String>)> = Vec::new(); // (name, hash, contents)
    let style = style();
    others.push(("style.css".into(), hash(&[style.as_bytes()]), Some(style)));
    for (i, chunk) in shown.chunks(PER_PAGE).enumerate() {
        let page = site.index_page(i + 1, chunk);
        others.push((format!("page-{}.html", i + 1), hash(&[page.as_bytes()]), Some(page)));
    }
    let index = site.index(&shown, !state.programs.is_empty());
    others.push(("index.html".into(), hash(&[index.as_bytes()]), Some(index)));
    if !state.programs.is_empty() {
        let page = site.programs(&input.feed, &state.programs);
        others.push(("programs.html".into(), hash(&[page.as_bytes()]), Some(page)));
    }
    if base.is_some() {
        let items = &shown[shown.len().saturating_sub(FEED_ITEMS)..];
        let author = profile.as_ref().map(|(_, n, _)| clean(n)).unwrap_or_else(|| title.clone());
        let mut parts: Vec<Vec<u8>> = vec![options.as_bytes().to_vec(), site.description.as_bytes().to_vec(), author.as_bytes().to_vec()];
        parts.extend(items.iter().map(|r| r.hash.clone().unwrap_or_default().into_bytes()));
        let h = hash(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>());
        others.push(("feed.xml".into(), h, None));
    } else if state.files.remove("feed.xml").is_some() {
        // Without a base URL there is no Atom feed.
        let _ = std::fs::remove_file(out.join("feed.xml"));
    }
    for (name, h, contents) in others {
        if state.files.get(&name) == Some(&h) && out.join(&name).is_file() {
            sum.unchanged += 1;
            continue;
        }
        let data = match contents {
            Some(d) => d,
            None => {
                // feed.xml: the bodies of the newest pages, as written.
                let items = &shown[shown.len().saturating_sub(FEED_ITEMS)..];
                let mut bodies = Vec::new();
                for r in items {
                    let page = std::fs::read_to_string(out.join(&r.page)).map_err(|e| format!("{}: {e}", r.page))?;
                    bodies.push((*r, body_of(&page).unwrap_or_default().to_string()));
                }
                let author = profile.as_ref().map(|(_, n, _)| clean(n)).unwrap_or_else(|| title.clone());
                atom(&site, base.as_deref().unwrap(), &input.feed, &bodies, &author)
            }
        };
        write_file(out, &name, &data)?;
        sum.written.push(name.clone());
        state.files.insert(name, h);
    }

    let text = state.to_text();
    if old_text.as_deref() != Some(text.as_str()) {
        write_file(out, STATE_FILE, &text)?;
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip() {
        let mut s = State { generator: GENERATOR.into(), options: "ab".into(), ..Default::default() };
        s.profile = Some(([1; 32], "A name".into(), "Two\n\nlines 100%".into()));
        s.programs.insert("my prog".into());
        s.articles.insert(
            [2; 32],
            Rec { seq: 3, time: 9, title: "A title with spaces".into(), blob: [2; 32], page: "0202.html".into(), links: vec![[1; 32], [3; 32]], hash: Some("cd".into()) },
        );
        s.articles.insert([4; 32], Rec { seq: 4, time: 0, title: String::new(), blob: [4; 32], page: "0404.html".into(), links: vec![], hash: None });
        s.files.insert("index.html".into(), "ef".into());
        let text = s.to_text();
        assert_eq!(State::parse(&text), Some(s));
        assert_eq!(State::parse("nonsense"), None);
    }

    #[test]
    fn names_and_dates() {
        let blob = [0xab; 32];
        let mut taken = HashSet::new();
        assert_eq!(page_name(&blob, &taken), "abababababababab.html");
        taken.insert("abababababababab.html".to_string());
        assert_eq!(page_name(&blob, &taken), format!("{}.html", to_hex(&blob)));
        assert_eq!(rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(date(951_782_400).as_deref(), Some("2000-02-29"));
        assert_eq!(date(0), None);
        assert_eq!(base_url("https://example.org/blog").unwrap(), "https://example.org/blog/");
        assert!(base_url("ftp://x").is_err() && base_url("https://x/?a").is_err() && base_url("https://").is_err());
        assert_eq!(enc("a b%\n"), "a%20b%25%0A");
        assert_eq!(dec(&enc("a b%\n")).as_deref(), Some("a b%\n"));
    }
}
