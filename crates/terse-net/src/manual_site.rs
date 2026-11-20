//! The manual as a static website: one HTML page per article of the built
//! manual ([`crate::manual`]), parsed by the reference article parser
//! ([`crate::article_parse`]) and written as plain HTML, with figures as
//! inline SVG and one small stylesheet. No scripts, no external requests:
//! every link is to another page of the site.
//!
//! Pages: the `overview` article is `index.html`, the landing page, with a
//! list of every article under it; the manual's own `index` article is
//! `contents.html`; every other article is `<name>.html`. A link to an
//! article of the manual leads to its page; any other `terse:` link
//! cannot be followed on the web and is shown as its text and address.
//! The output depends only on the articles, so the same sources give the
//! same bytes.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::article_parse::{Align, Block, Colour, Figure, Inline, Parsed, Seg, Table, Target, parse};
use crate::manual::Built;
use crate::{BlobId, to_hex};

/// The stylesheet, `style.css`.
pub const STYLE: &str = include_str!("manual_site.css");

/// The file of an article's page.
pub fn page_of(name: &str) -> String {
    match name {
        "overview" => "index.html".into(),
        "index" => "contents.html".into(),
        n => format!("{n}.html"),
    }
}

/// The site of a built manual: (file name, contents), the stylesheet
/// first, then one page per article in manifest order.
pub fn site(built: &[Built]) -> Vec<(String, String)> {
    let pages: HashMap<BlobId, usize> = built.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut out = vec![("style.css".to_string(), STYLE.to_string())];
    for (i, b) in built.iter().enumerate() {
        out.push((page_of(&b.name), page(built, &pages, i)));
    }
    out
}

/// Builds the manual in `dir` and writes its site to `out`; `.html` files
/// of articles no longer listed are removed. Returns the files written.
pub fn site_dir(dir: &Path, out: &Path) -> Result<Vec<(String, String)>, String> {
    let built = crate::manual::build(&crate::manual::read_sources(dir)?)?;
    let files = site(&built);
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

// --- Escaping ----------------------------------------------------------------

/// Text escaped for HTML content and attribute values. A `://` in the
/// text is written with its colon as a character reference, so that the
/// page holds no web address in its bytes (it reads the same).
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            ':' if s[i..].starts_with("://") => out.push_str("&#58;"),
            c => out.push(c),
        }
    }
    out
}

// --- Inline text ---------------------------------------------------------------

/// A link's address as written: `terse:` and the target.
fn address(t: &Target) -> String {
    match t {
        Target::Article(id) => format!("terse:{}", to_hex(id)),
        Target::Feed(k, h) if h.is_empty() => format!("terse:feed/{}", to_hex(k)),
        Target::Feed(k, h) => format!("terse:feed/{}@{h}", to_hex(k)),
        Target::Program(k, name) => {
            let mut n = String::new();
            for &b in name.as_bytes() {
                if b.is_ascii_alphanumeric() || b"-_~".contains(&b) {
                    n.push(b as char);
                } else {
                    let _ = write!(n, "%{b:02X}");
                }
            }
            format!("terse:program/{}/{n}", to_hex(k))
        }
    }
}

struct Ctx<'a> {
    built: &'a [Built],
    pages: &'a HashMap<BlobId, usize>,
}

impl Ctx<'_> {
    /// The page of an article link to the manual.
    fn href(&self, t: &Target) -> Option<(String, &str)> {
        match t {
            Target::Article(id) => self.pages.get(id).map(|&i| (page_of(&self.built[i].name), self.built[i].title.as_str())),
            _ => None,
        }
    }

    /// Inline text as HTML. `pre`: inside a preformatted block (links count,
    /// code spans do not, and the text is already monospaced).
    fn inlines(&self, xs: &[Inline], pre: bool) -> String {
        let mut out = String::new();
        for x in xs {
            match x {
                Inline::Text(t) => out.push_str(&esc(t)),
                Inline::Code(t) => {
                    let _ = write!(out, "<code>{}</code>", esc(t));
                }
                Inline::Link { target, text } => match (self.href(target), text) {
                    (Some((href, _)), Some(text)) => {
                        let _ = write!(out, "<a href=\"{href}\">{}</a>", esc(text));
                    }
                    (Some((href, title)), None) => {
                        // Bare: shortened as the reference reader shows it.
                        let hex = &address(target)[6..];
                        let _ = write!(
                            out,
                            "<a href=\"{href}\" title=\"{}\">terse:{}…{}</a>",
                            esc(title),
                            &hex[..6],
                            &hex[hex.len() - 6..]
                        );
                    }
                    // Not a page of this site: the address as text.
                    (None, _) if pre => out.push_str(&esc(&address(target))),
                    (None, Some(text)) => {
                        let _ = write!(out, "{} (<code class=\"addr\">{}</code>)", esc(text), esc(&address(target)));
                    }
                    (None, None) => {
                        let _ = write!(out, "<code class=\"addr\">{}</code>", esc(&address(target)));
                    }
                },
            }
        }
        out
    }
}

/// The plain text of inline content (for heading ids).
fn plain(xs: &[Inline]) -> String {
    xs.iter()
        .map(|x| match x {
            Inline::Text(t) | Inline::Code(t) | Inline::Link { text: Some(t), .. } => t.clone(),
            Inline::Link { target, text: None } => address(target),
        })
        .collect()
}

/// A unique id for a heading: its words, lowercase, joined by `-`.
fn slug(text: &str, used: &mut HashSet<String>) -> String {
    let mut s = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            s.push(c);
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() || !s.starts_with(|c: char| c.is_ascii_alphabetic()) {
        s.insert_str(0, "s-");
    }
    let mut id = s.clone();
    let mut n = 2;
    while !used.insert(id.clone()) {
        id = format!("{s}-{n}");
        n += 1;
    }
    id
}

// --- Blocks --------------------------------------------------------------------

fn table(ctx: &Ctx, t: &Table, out: &mut String) {
    out.push_str("<div class=\"table\"><table>\n");
    let class = |c: usize| match t.align[c] {
        Align::Start => "",
        Align::Centre => " class=\"ac\"",
        Align::End => " class=\"ae\"",
    };
    for (r, row) in t.rows.iter().enumerate() {
        let head = t.header && r == 0;
        if head {
            out.push_str("<thead>");
        } else if r == t.header as usize {
            out.push_str("<tbody>\n");
        }
        out.push_str("<tr>");
        for (c, cell) in row.iter().enumerate() {
            let tag = if head { "th" } else { "td" };
            let _ = write!(out, "<{tag}{}>{}</{tag}>", class(c), ctx.inlines(cell, false));
        }
        out.push_str("</tr>");
        if head {
            out.push_str("</thead>");
        }
        out.push('\n');
    }
    if t.rows.len() > t.header as usize {
        out.push_str("</tbody>");
    }
    out.push_str("</table></div>\n");
}

/// Sixteenths of a unit as a decimal number of units.
fn units(v: i32) -> String {
    let (sign, a) = if v < 0 { ("-", -(v as i64)) } else { ("", v as i64) };
    let (int, frac) = (a / 16, a % 16);
    if frac == 0 {
        return format!("{sign}{int}");
    }
    let f = format!("{:04}", frac * 625);
    format!("{sign}{int}.{}", f.trim_end_matches('0'))
}

fn colour(c: Colour) -> (String, Option<String>) {
    match c {
        Colour::Fg => ("currentColor".into(), None),
        Colour::Rgba([r, g, b, a]) => {
            let op = (a != 255).then(|| {
                let v = format!("{:.3}", a as f64 / 255.0);
                v.trim_end_matches('0').trim_end_matches('.').to_string()
            });
            (format!("#{r:02x}{g:02x}{b:02x}"), op)
        }
    }
}

/// A figure (ARTICLE.md 6) as inline SVG: each path filled (non-zero) then
/// stroked with round joins and caps; `fg` is the text colour. A unit is
/// a fifteenth of the body text's size, as in the reference reader (body
/// text 15 units in a 512-unit column), and the figure is never wider
/// than the column.
fn figure(f: &Figure, out: &mut String) {
    let w = f.width as f64 / 15.0;
    let _ = write!(
        out,
        "<figure><svg viewBox=\"0 0 {0} {1}\" width=\"{0}\" height=\"{1}\" style=\"width:{2}em\" role=\"img\" aria-label=\"Figure, {0} by {1} units\">",
        f.width,
        f.height,
        format!("{w:.3}").trim_end_matches('0').trim_end_matches('.')
    );
    for p in &f.paths {
        let mut d = String::new();
        for s in &p.segs {
            if !d.is_empty() {
                d.push(' ');
            }
            let u = units;
            let _ = match *s {
                Seg::Move(x, y) => write!(d, "M{} {}", u(x), u(y)),
                Seg::Line(x, y) => write!(d, "L{} {}", u(x), u(y)),
                Seg::Quad(a, b, x, y) => write!(d, "Q{} {} {} {}", u(a), u(b), u(x), u(y)),
                Seg::Cubic(a, b, c, e, x, y) => write!(d, "C{} {} {} {} {} {}", u(a), u(b), u(c), u(e), u(x), u(y)),
                Seg::Close => write!(d, "Z"),
            };
        }
        let _ = write!(out, "<path d=\"{d}\"");
        match p.fill {
            None => out.push_str(" fill=\"none\""),
            Some(c) => {
                let (c, op) = colour(c);
                let _ = write!(out, " fill=\"{c}\"");
                if let Some(op) = op {
                    let _ = write!(out, " fill-opacity=\"{op}\"");
                }
            }
        }
        if let Some((c, width)) = p.stroke {
            let (c, op) = colour(c);
            let _ = write!(out, " stroke=\"{c}\" stroke-width=\"{}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"", units(width as i32));
            if let Some(op) = op {
                let _ = write!(out, " stroke-opacity=\"{op}\"");
            }
        }
        out.push_str("/>");
    }
    out.push_str("</svg></figure>\n");
}

/// An article's blocks as HTML.
fn article(ctx: &Ctx, blocks: &[Block], ids: &mut HashSet<String>) -> String {
    let mut out = String::new();
    let mut in_list = false;
    for b in blocks {
        let item = matches!(b, Block::Item(_));
        if in_list && !item {
            out.push_str("</ul>\n");
        } else if !in_list && item {
            out.push_str("<ul>\n");
        }
        in_list = item;
        match b {
            Block::Heading(x) | Block::Subheading(x) => {
                let h = if matches!(b, Block::Heading(_)) { "h1" } else { "h2" };
                let id = slug(&plain(x), ids);
                let _ = writeln!(out, "<{h} id=\"{id}\">{}</{h}>", ctx.inlines(x, false));
            }
            Block::Paragraph(x) => {
                let _ = writeln!(out, "<p>{}</p>", ctx.inlines(x, false));
            }
            Block::Item(x) => {
                let _ = writeln!(out, "<li>{}</li>", ctx.inlines(x, false));
            }
            Block::Pre(lines) => {
                out.push_str("<pre>");
                for (i, l) in lines.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    out.push_str(&ctx.inlines(l, true));
                }
                out.push_str("</pre>\n");
            }
            Block::Table(t) => table(ctx, t, &mut out),
            Block::Figure(f) => figure(f, &mut out),
        }
    }
    if in_list {
        out.push_str("</ul>\n");
    }
    out
}

// --- Pages ---------------------------------------------------------------------

fn page(built: &[Built], pages: &HashMap<BlobId, usize>, i: usize) -> String {
    let ctx = Ctx { built, pages };
    let b = &built[i];
    let blocks = match parse(b.text.as_bytes()) {
        Parsed::Article(blocks) => blocks,
        _ => Vec::new(),
    };
    let mut ids = HashSet::new();
    let landing = b.name == "overview";
    let title = if landing { format!("Terse: {}", b.title) } else { format!("{} - the Terse manual", b.title) };
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<link rel=\"stylesheet\" href=\"style.css\">\n</head>\n<body>\n\
         <header><nav><a href=\"index.html\" class=\"home\">Terse</a> <a href=\"contents.html\">Manual</a></nav></header>\n\
         <main>\n<article>\n",
        esc(&title)
    );
    out.push_str(&article(&ctx, &blocks, &mut ids));
    out.push_str("</article>\n");
    if landing {
        let id = slug("The manual", &mut ids);
        let _ = writeln!(out, "<hr>\n<section class=\"toc\">\n<h2 id=\"{id}\">The manual</h2>\n<ul>");
        for a in built {
            let _ = writeln!(out, "<li><a href=\"{}\">{}</a></li>", page_of(&a.name), esc(&a.title));
        }
        out.push_str("</ul>\n</section>\n");
    }
    out.push_str("</main>\n<footer><nav class=\"pn\">");
    if i > 0 {
        let p = &built[i - 1];
        let _ = write!(out, "<a rel=\"prev\" href=\"{}\">← {}</a>", page_of(&p.name), esc(&p.title));
    }
    if let Some(n) = built.get(i + 1) {
        let _ = write!(out, "<a rel=\"next\" href=\"{}\">{} →</a>", page_of(&n.name), esc(&n.title));
    }
    out.push_str("</nav></footer>\n</body>\n</html>\n");
    out
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
            ("index", "# Contents\n\n- [What](terse:{overview})\n- [B](terse:{b})\n"),
            ("overview", "# What <it> is\n\nSee terse:{b} & https://x.org.\n"),
            ("b", "# B\n\n[F](terse:feed/0000000000000000000000000000000000000000000000000000000000000000)\n\n| a | b |\n|---|--:|\n| `x\\|y` | 2 |\n\n```path 30 15\nfill #ff000080 stroke fg 1.5 M0 0 L30 15 Z\n```\n"),
        ]))
        .unwrap();
        let files = site(&built);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["style.css", "contents.html", "index.html", "b.html"]);
        let index = &files[2].1;
        assert!(index.contains("<title>Terse: What &lt;it&gt; is</title>"), "{index}");
        assert!(index.contains("<a href=\"b.html\" title=\"B\">terse:"), "{index}");
        assert!(index.contains("&amp; https&#58;//x.org."), "{index}");
        assert!(index.contains("<li><a href=\"contents.html\">Contents</a></li>"), "{index}");
        assert!(files[1].1.contains("<a href=\"index.html\">What</a>"));
        let b = &files[3].1;
        assert!(b.contains("F (<code class=\"addr\">terse:feed/0000"), "{b}");
        assert!(b.contains("<th class=\"ae\">b</th>") && b.contains("<code>x|y</code>"), "{b}");
        assert!(b.contains("<path d=\"M0 0 L30 15 Z\" fill=\"#ff0000\" fill-opacity=\"0.502\" stroke=\"currentColor\" stroke-width=\"1.5\""), "{b}");
        assert!(b.contains("style=\"width:2em\""), "{b}");
        assert!(b.contains("<a rel=\"prev\" href=\"index.html\">") && !b.contains("rel=\"next\""));
        assert_eq!(site(&built), files, "deterministic");
    }

    #[test]
    fn numbers_and_ids() {
        assert_eq!(units(16), "1");
        assert_eq!(units(-8), "-0.5");
        assert_eq!(units(1), "0.0625");
        assert_eq!(units(-3), "-0.1875");
        let mut used = HashSet::new();
        assert_eq!(slug("2. Blocks", &mut used), "s-2-blocks");
        assert_eq!(slug("A `b` c!", &mut used), "a-b-c");
        assert_eq!(slug("A b c", &mut used), "a-b-c-2");
    }
}
