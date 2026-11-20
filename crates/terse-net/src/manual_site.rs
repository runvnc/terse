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

use crate::BlobId;
use crate::article_parse::{Parsed, Target, parse};
use crate::manual::Built;
use crate::site_render::{Ctx, article, esc, slug};

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
}
