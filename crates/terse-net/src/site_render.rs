//! Articles as plain HTML, shared by the static websites: the manual's
//! ([`crate::manual_site`]) and a feed's ([`crate::feed_site`]). An article
//! is parsed by the reference article parser ([`crate::article_parse`]) and
//! written as HTML with figures as inline SVG. No scripts, no external
//! requests: a link leads to a page only if the front end says which
//! ([`Ctx::resolve`]); any other `terse:` link cannot be followed on the
//! web and is shown as its text and address.

use std::collections::HashSet;
use std::fmt::Write as _;

use crate::article_parse::{Align, Block, Colour, Figure, Inline, Table, Target};
use crate::{BlobId, highlight, to_hex};

// --- Escaping ----------------------------------------------------------------

/// Text escaped for HTML content and attribute values. A `://` in the
/// text is written with its colon as a character reference, so that the
/// page holds no web address in its bytes (it reads the same).
pub fn esc(s: &str) -> String {
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
pub fn address(t: &Target) -> String {
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

/// How links resolve on a site: a target to the page that shows it and
/// that page's title, or `None` if it is not a page of the site.
pub struct Ctx<'a> {
    pub resolve: &'a dyn Fn(&Target) -> Option<(String, String)>,
}

impl Ctx<'_> {
    /// Inline text as HTML. `pre`: inside a preformatted block (links count,
    /// code spans do not, and the text is already monospaced).
    pub fn inlines(&self, xs: &[Inline], pre: bool) -> String {
        let mut out = String::new();
        for x in xs {
            match x {
                Inline::Text(t) => out.push_str(&esc(t)),
                Inline::Code(t) => {
                    let _ = write!(out, "<code>{}</code>", esc(t));
                }
                Inline::Link { target, text } => match ((self.resolve)(target), text) {
                    (Some((href, _)), Some(text)) => {
                        let _ = write!(out, "<a href=\"{href}\">{}</a>", esc(text));
                    }
                    (Some((href, title)), None) => {
                        // Bare: shortened as the reference reader shows it.
                        let hex = &address(target)[6..];
                        let _ = write!(
                            out,
                            "<a href=\"{href}\" title=\"{}\">terse:{}…{}</a>",
                            esc(&title),
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
pub fn slug(text: &str, used: &mut HashSet<String>) -> String {
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
pub fn units(v: i32) -> String {
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
    use crate::article_parse::Seg;
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

/// `s[r]` escaped as [`esc`] escapes it within the whole of `s` (so that
/// a `://` cut by a run's edge is still escaped).
fn esc_in(s: &str, r: std::ops::Range<usize>) -> String {
    let mut out = String::with_capacity(r.len());
    for (i, c) in s[r.clone()].char_indices() {
        match c {
            ':' if s[r.start + i..].starts_with("://") => out.push_str("&#58;"),
            c => out.push_str(&esc(c.encode_utf8(&mut [0; 4]))),
        }
    }
    out
}

/// A preformatted block or fence. A fence whose info string names a
/// language ([`highlight::lang`]) is coloured: each run of a class is a
/// `<span>` of its CSS class; command lines are `<pre class="sh">`. Links
/// are shown as everywhere else, the tokenizer seeing each as its address.
fn pre(ctx: &Ctx, lines: &[Vec<Inline>], info: &str, out: &mut String) {
    let lang = highlight::lang(info);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| {
            l.iter()
                .map(|x| match x {
                    Inline::Text(t) | Inline::Code(t) => t.clone(),
                    Inline::Link { target, .. } => address(target),
                })
                .collect()
        })
        .collect();
    let runs = match lang {
        Some(lang) => highlight::highlight(lang, &texts.iter().map(String::as_str).collect::<Vec<_>>()),
        None => vec![Vec::new(); lines.len()],
    };
    out.push_str(if lang == Some(highlight::Lang::Shell) { "<pre class=\"sh\">" } else { "<pre>" });
    for (i, (l, text)) in lines.iter().zip(&texts).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if runs[i].is_empty() {
            out.push_str(&ctx.inlines(l, true));
            continue;
        }
        let mut at = 0;
        for x in l {
            let len = match x {
                Inline::Text(t) | Inline::Code(t) => t.len(),
                Inline::Link { target, .. } => {
                    out.push_str(&ctx.inlines(std::slice::from_ref(x), true));
                    at += address(target).len();
                    continue;
                }
            };
            let (from, to) = (at, at + len);
            let mut k = from;
            for (class, r) in runs[i].iter().filter(|(_, r)| r.end > from && r.start < to) {
                let (a, b) = (r.start.max(from), r.end.min(to));
                if a > k {
                    out.push_str(&esc_in(text, k..a));
                }
                let _ = write!(out, "<span class=\"{}\">{}</span>", class.css().unwrap_or(""), esc_in(text, a..b));
                k = b;
            }
            if to > k {
                out.push_str(&esc_in(text, k..to));
            }
            at = to;
        }
    }
    out.push_str("</pre>\n");
}

/// An article's blocks as HTML; `ids` are the element ids already used on
/// the page (headings get unique ones).
pub fn article(ctx: &Ctx, blocks: &[Block], ids: &mut HashSet<String>) -> String {
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
            Block::Pre(lines, info) => pre(ctx, lines, info, &mut out),
            Block::Table(t) => table(ctx, t, &mut out),
            Block::Figure(f) => figure(f, &mut out),
        }
    }
    if in_list {
        out.push_str("</ul>\n");
    }
    out
}

/// Every article a link of `blocks` leads to (the targets [`Ctx::resolve`]
/// is asked about), without repeats, in increasing order.
pub fn article_links(blocks: &[Block]) -> Vec<BlobId> {
    let mut out = std::collections::BTreeSet::new();
    let mut add = |xs: &[Inline]| {
        for x in xs {
            if let Inline::Link { target: Target::Article(id), .. } = x {
                out.insert(*id);
            }
        }
    };
    for b in blocks {
        match b {
            Block::Heading(x) | Block::Subheading(x) | Block::Paragraph(x) | Block::Item(x) => add(x),
            Block::Pre(lines, _) => lines.iter().for_each(|l| add(l)),
            Block::Table(t) => t.rows.iter().flatten().for_each(|c| add(c)),
            Block::Figure(_) => {}
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn fences_coloured() {
        let a = [7u8; 32];
        let text = format!(
            "```c x.c\nint x = 1; // terse:{} see\nchar *u = \"http://q\"; y = http://z;\n```\n\n```sh\nterse run a.terse\n```\n\n```\nint x;\n```\n\n    int y;\n",
            to_hex(&a)
        );
        let crate::article_parse::Parsed::Article(blocks) = crate::article_parse::parse(text.as_bytes()) else { panic!() };
        let resolve = |_: &Target| Some(("a.html".to_string(), "A".to_string()));
        let html = article(&Ctx { resolve: &resolve }, &blocks, &mut HashSet::new());
        assert!(html.contains("<pre><span class=\"t\">int</span> x = <span class=\"n\">1</span>; <span class=\"c\">// </span><a href=\"a.html\" title=\"A\">terse:070707…070707</a><span class=\"c\"> see</span>\n"), "{html}");
        assert!(html.contains("<span class=\"s\">&quot;http&#58;//q&quot;</span>; y = http<span class=\"c\">&#58;//z;</span>") || html.contains("http&#58;<span class=\"c\">//z;</span>"), "{html}");
        assert!(!html.contains("://"), "{html}");
        assert!(html.contains("<pre class=\"sh\"><span class=\"k\">terse</span> run a.terse</pre>"), "{html}");
        assert!(html.contains("<pre>int x;</pre>\n<pre>    int y;</pre>"), "{html}");
    }

    #[test]
    fn links_found() {
        let a = [7u8; 32];
        let b = [3u8; 32];
        let text = format!("# T\n\nterse:{} and [x](terse:{})\n\n    terse:{}\n", to_hex(&a), to_hex(&b), to_hex(&a));
        let crate::article_parse::Parsed::Article(blocks) = crate::article_parse::parse(text.as_bytes()) else { panic!() };
        assert_eq!(article_links(&blocks), vec![b, a]);
    }
}
