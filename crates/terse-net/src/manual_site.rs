//! The manual as a static website: one HTML page per article of the built
//! manual ([`crate::manual`]), parsed by the reference article parser
//! ([`crate::article_parse`]) and written as plain HTML, with figures as
//! inline SVG and one small stylesheet. No scripts, no external requests:
//! every link is to another page of the site.
//!
//! Pages: the manual's `index` article is `index.html`, the landing page:
//! the article alone, with no navigation and no list of articles (it
//! links to the guide's first page and to the contents itself). The
//! `contents` article, which lists every other article, is `contents.html`;
//! every other article is `<name>.html`, with the navigation to the landing
//! page and the contents above it and the previous and next articles of the
//! manifest below it. A link to an
//! article of the manual leads to its page; any other `terse:` link
//! cannot be followed on the web and is shown as its text and address.
//! The output depends only on the articles, so the same sources give the
//! same bytes.
//!
//! Besides the pages, [`site_dir`] copies the files of the manual's
//! `site-files/` directory (for example `install.sh`, the install script
//! the guide names) to the site as they are: text files only, no
//! subdirectories.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::BlobId;
use crate::article_parse::{Parsed, Target, parse};
use crate::manual::Built;
use crate::site_render::{Ctx, article, esc};

/// The stylesheet, `style.css`.
pub const STYLE: &str = include_str!("manual_site.css");

/// The file of an article's page.
pub fn page_of(name: &str) -> String {
    match name {
        "index" => "index.html".into(),
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

/// The directory of a manual's extra files, copied to its site as they are.
pub const EXTRA_DIR: &str = "site-files";

/// The extra files of the manual in `dir` (its `site-files/`, if any):
/// (file name, contents), sorted by name.
pub fn extra_files(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let extra = dir.join(EXTRA_DIR);
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(&extra) else { return Ok(files) };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let data = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            files.push((entry.file_name().to_string_lossy().into_owned(), data));
        }
    }
    files.sort();
    Ok(files)
}

/// Builds the manual in `dir` and writes its site to `out`, with the extra
/// files of [`extra_files`]; `.html` files of articles no longer listed are
/// removed. Returns the files written.
pub fn site_dir(dir: &Path, out: &Path) -> Result<Vec<(String, String)>, String> {
    let built = crate::manual::build(&crate::manual::read_sources(dir)?)?;
    let mut files = site(&built);
    files.extend(extra_files(dir)?);
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
    let landing = b.name == "index";
    let title = if landing { b.title.clone() } else { format!("{} - the Tersenet manual", b.title) };
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<link rel=\"stylesheet\" href=\"style.css\">\n</head>\n<body>\n",
        esc(&title)
    );
    if landing {
        out.push_str("<main class=\"landing\">\n<article>\n");
    } else {
        out.push_str(
            "<header><nav><a href=\"index.html\" class=\"home\">Tersenet</a> <a href=\"contents.html\">Contents</a></nav></header>\n\
             <main>\n<article>\n",
        );
    }
    out.push_str(&article(&ctx, &blocks, &mut ids));
    out.push_str("</article>\n</main>\n");
    if landing {
        out.push_str("</body>\n</html>\n");
        return out;
    }
    out.push_str("<footer><nav class=\"pn\">");
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
            ("index", "# What <it> is\n\nSee terse:{b} & https://x.org.\n\n[Contents](terse:{contents})\n"),
            ("contents", "# Contents\n\n- [B](terse:{b})\n"),
            ("b", "# B\n\n[F](terse:feed/0000000000000000000000000000000000000000000000000000000000000000)\n\n| a | b |\n|---|--:|\n| `x\\|y` | 2 |\n\n```path 30 15\nfill #ff000080 stroke fg 1.5 M0 0 L30 15 Z\n```\n"),
        ]))
        .unwrap();
        let files = site(&built);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["style.css", "index.html", "contents.html", "b.html"]);
        // The landing page is the index article alone: no navigation, no
        // list of articles, no previous and next.
        let index = &files[1].1;
        assert!(index.contains("<title>What &lt;it&gt; is</title>"), "{index}");
        assert!(index.contains("<a href=\"b.html\" title=\"B\">terse:"), "{index}");
        assert!(index.contains("&amp; https&#58;//x.org."), "{index}");
        assert!(index.contains("<a href=\"contents.html\">Contents</a>"), "{index}");
        assert!(!index.contains("<header>") && !index.contains("<footer>") && !index.contains("<li>"), "{index}");
        let contents = &files[2].1;
        assert!(contents.contains("<title>Contents - the Tersenet manual</title>"), "{contents}");
        assert!(contents.contains("<li><a href=\"b.html\">B</a></li>"), "{contents}");
        assert!(contents.contains("<a href=\"index.html\" class=\"home\">Tersenet</a> <a href=\"contents.html\">Contents</a>"));
        assert!(contents.contains("<a rel=\"prev\" href=\"index.html\">") && contents.contains("<a rel=\"next\" href=\"b.html\">"));
        let b = &files[3].1;
        assert!(b.contains("F (<code class=\"addr\">terse:feed/0000"), "{b}");
        assert!(b.contains("<th class=\"ae\">b</th>") && b.contains("<code>x|y</code>"), "{b}");
        assert!(b.contains("<path d=\"M0 0 L30 15 Z\" fill=\"#ff0000\" fill-opacity=\"0.502\" stroke=\"currentColor\" stroke-width=\"1.5\""), "{b}");
        assert!(b.contains("style=\"width:2em\""), "{b}");
        assert!(b.contains("<a rel=\"prev\" href=\"contents.html\">") && !b.contains("rel=\"next\""));
        assert_eq!(site(&built), files, "deterministic");
    }
}
