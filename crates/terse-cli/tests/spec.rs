//! The machine specification's documents: SPEC.md (the core machine) and
//! one document per further device in spec/. Their section ids are unique
//! and live where the naming rule says, every `§id` cited anywhere in the
//! repository names a section, no citation uses the old section numbers,
//! and the call and event tables of the documents are exactly the rows of
//! SPEC.md's index (§index), so the two cannot drift apart.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The device documents, by the id prefix that names them.
const DEVICES: [&str; 5] = ["content", "sound", "page", "machines", "vector3d"];

fn documents() -> Vec<(String, String)> {
    let root = common::workspace();
    let mut docs = vec![("SPEC.md".to_string(), std::fs::read_to_string(root.join("SPEC.md")).unwrap())];
    for d in DEVICES {
        let name = format!("spec/{d}.md");
        docs.push((name.clone(), std::fs::read_to_string(root.join(&name)).unwrap()));
    }
    docs
}

/// The document an id belongs in: `spec/<prefix>.md` for a device prefix,
/// else SPEC.md.
fn home(id: &str) -> String {
    let first = id.split('.').next().unwrap();
    if DEVICES.contains(&first) { format!("spec/{first}.md") } else { "SPEC.md".to_string() }
}

/// The heading ids of a document: `# §id Title`, `## §id …`, `### §id …`,
/// with the heading's byte offset.
fn headings(text: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start_matches('#');
        if line.starts_with('#') && t.starts_with(" §") {
            let id = t[" §".len()..].split_whitespace().next().unwrap().to_string();
            out.push((id, at));
        }
        at += line.len();
    }
    out
}

/// Every id of every document, with the document it is in.
fn ids() -> BTreeMap<String, String> {
    let mut all = BTreeMap::new();
    for (name, text) in documents() {
        for (id, _) in headings(&text) {
            assert_eq!(home(&id), name, "§{id} is in {name}, but its name says {}", home(&id));
            assert!(all.insert(id.clone(), name.clone()).is_none(), "§{id} is defined twice");
        }
    }
    all
}

#[test]
fn ids_are_unique_and_where_they_belong() {
    let all = ids();
    for d in DEVICES {
        assert!(all.contains_key(d), "spec/{d}.md has its device section §{d}");
    }
    for id in ["minimal", "profiles", "calls.table", "events", "terminal.put", "index.calls", "index.events"] {
        assert!(all.contains_key(id), "§{id}");
    }
}

/// The repository's text files, skipping build output and hidden
/// directories.
fn text_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if e.file_type().unwrap().is_dir() {
            if !name.starts_with('.') && name != "target" && name != "__pycache__" && name != "node_modules" {
                text_files(&path, out);
            }
        } else if e.metadata().unwrap().len() < 4 << 20 {
            out.push(path);
        }
    }
}

/// `§name` citations in `text`: a section sign followed by a lowercase
/// letter, then letters, digits, dots and hyphens (not ending in one).
fn citations(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices('§') {
        let rest = &text[i + '§'.len_utf8()..];
        if !rest.starts_with(|c: char| c.is_ascii_lowercase()) {
            continue;
        }
        let end = rest.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
        out.push(rest[..end].trim_end_matches(['.', '-']).to_string());
    }
    out
}

/// Whether `text` cites the specification by an old section number:
/// `SPEC 12.2`, `SPEC.md section 9`, `` `SPEC.md` 4 ``.
fn old_citation(text: &str) -> Option<String> {
    for (i, _) in text.match_indices("SPEC") {
        let mut rest = &text[i + 4..];
        rest = rest.strip_prefix(".md").unwrap_or(rest);
        rest = rest.strip_prefix('`').unwrap_or(rest);
        let r = rest.trim_start_matches([' ', '\n', '\t', '/', '*', '!', '#']);
        let r = r.strip_prefix("sections ").or_else(|| r.strip_prefix("section ")).unwrap_or(r);
        if rest.len() != r.len() && r.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(text[i..].chars().take(24).collect());
        }
    }
    None
}

#[test]
fn every_citation_resolves() {
    let all = ids();
    let root = common::workspace();
    let mut files = Vec::new();
    text_files(&root, &mut files);
    let mut bad = Vec::new();
    let mut count = 0;
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        // With `/` between the parts on every system.
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if rel.ends_with("tests/spec.rs") {
            continue;
        }
        for c in citations(&text) {
            count += 1;
            if !all.contains_key(&c) {
                bad.push(format!("{rel}: §{c}"));
            }
        }
        if rel != "spec/RENUMBERING.md" {
            if let Some(s) = old_citation(&text) {
                bad.push(format!("{rel}: an old section number: {s:?} (spec/RENUMBERING.md maps them)"));
            }
        }
    }
    assert!(count > 900, "{count} citations");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The rows of every table in `text` whose header is `header`, outside
/// (`inside` false) or inside (`inside` true) the section `§index`.
fn rows(text: &str, header: &str, inside: bool) -> Vec<String> {
    let index = text.find("\n## §index ").unwrap_or(text.len());
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find(header) {
        let start = at + i;
        let body = &text[start + header.len()..];
        let end = body.find("\n\n").unwrap_or(body.len());
        if (start > index) == inside {
            out.extend(body[..end].lines().filter(|l| l.starts_with("| `")).map(str::to_string));
        }
        at = start + header.len() + end;
    }
    out
}

/// `owners`: the documents that must have a table of their own.
fn check_table(header: &str, what: &str, expect: usize, owners: &[&str]) {
    let docs = documents();
    let index = rows(&docs[0].1, header, true);
    assert_eq!(index.len(), expect, "{what}: rows of §index");
    let mut owned = Vec::new();
    for (name, text) in &docs {
        let r = rows(text, header, false);
        assert_eq!(!r.is_empty(), owners.contains(&name.as_str()), "{name}: a table of {what}");
        owned.extend(r);
    }
    let a: BTreeSet<&String> = owned.iter().collect();
    let b: BTreeSet<&String> = index.iter().collect();
    assert_eq!(a.len(), owned.len(), "{what}: a row is in two documents");
    let missing: Vec<_> = b.difference(&a).collect();
    let extra: Vec<_> = a.difference(&b).collect();
    assert!(missing.is_empty() && extra.is_empty(), "{what}: in §index only: {missing:#?}\nin a document only: {extra:#?}");
}

#[test]
fn call_tables_equal_the_index() {
    check_table("| `a7` | Call | Arguments | Result | Class |\n|---|---|---|---|---|\n", "calls", 57, &["SPEC.md", "spec/content.md", "spec/sound.md", "spec/page.md", "spec/machines.md", "spec/vector3d.md"]);
}

#[test]
fn event_tables_equal_the_index() {
    check_table("| Type | Name | `a` | `b` | `c` |\n|---|---|---|---|---|\n", "events", 16, &["SPEC.md", "spec/content.md", "spec/page.md", "spec/machines.md"]);
}

/// The indented block that first follows `heading` in `text`, each line
/// with its indent removed.
fn list_after(text: &str, heading: &str) -> Vec<String> {
    let at = text.find(heading).unwrap_or_else(|| panic!("no heading {heading:?}"));
    let lines: Vec<&str> = text[at..].lines().collect();
    let start = lines.iter().position(|l| l.trim_start().starts_with("U+")).expect("a list");
    let indent = lines[start].len() - lines[start].trim_start().len();
    lines[start..]
        .iter()
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.get(indent..).unwrap_or_else(|| panic!("{l:?}: indented less")).to_string())
        .collect()
}

/// The basic text repertoire is listed in the same words in ARTICLE.md 1.1
/// and SPEC §terminal.text (so that a reader's and a machine's implementer
/// each read only their own document), and the code points that list names
/// are those of `vectors/basic-text.txt` and of the code.
#[test]
fn basic_text_lists_agree() {
    let root = common::workspace();
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
    let article = list_after(&read("ARTICLE.md"), "### 1.1 The basic text repertoire");
    let spec = list_after(&read("SPEC.md"), "### §terminal.text ");
    assert_eq!(article, spec, "ARTICLE.md 1.1 and SPEC.md §terminal.text list the same characters");
    let hex = |s: &str| u16::from_str_radix(s.trim_start_matches("U+"), 16).unwrap_or_else(|e| panic!("{s:?}: {e}"));
    let mut listed = Vec::new();
    let mut total = 0;
    for line in &article {
        let Some(first) = line.split_whitespace().next().filter(|w| w.starts_with("U+")) else { continue };
        let (lo, hi) = first.split_once('-').map_or_else(|| (hex(first), hex(first)), |(a, b)| (hex(a), hex(b)));
        let n: u32 = line.split_whitespace().last().unwrap().parse().unwrap_or_else(|e| panic!("{line:?}: {e}"));
        assert_eq!(n, (hi - lo + 1) as u32, "{line}");
        total += n;
        listed.push((lo, hi));
    }
    assert_eq!(listed, terse_core::text::BASIC);
    assert_eq!(total, terse_core::text::BASIC_COUNT);
    let vectors = read("vectors/basic-text.txt");
    let ranges: Vec<(u16, u16)> = vectors
        .lines()
        .filter_map(|l| l.trim().strip_prefix("range "))
        .map(|r| {
            let (a, b) = r.split_once(' ').unwrap();
            (u16::from_str_radix(a, 16).unwrap(), u16::from_str_radix(b, 16).unwrap())
        })
        .collect();
    assert_eq!(ranges, listed);
}

/// SPEC §terminal.text counts the full repertoire block by block; the
/// counts are those of `terse_core::text::FULL`, which
/// `vectors/full-text.txt` lists.
#[test]
fn full_text_counts_agree() {
    let root = common::workspace();
    let spec = std::fs::read_to_string(root.join("SPEC.md")).unwrap();
    let blocks = list_after(&spec, "lists them exactly; by block:");
    let hex = |s: &str| u16::from_str_radix(s.trim_start_matches("U+").trim_end_matches(','), 16).unwrap();
    let mut total = 0;
    for line in &blocks {
        let n: u32 = line.split_whitespace().last().unwrap().parse().unwrap();
        let mut have = 0;
        for w in line.split_whitespace().filter(|w| w.starts_with("U+")) {
            let w = w.trim_end_matches(',');
            let (lo, hi) = w.split_once('-').map_or_else(|| (hex(w), hex(w)), |(a, b)| (hex(a), hex(b)));
            have += (lo..=hi).filter(|&c| terse_core::text::is_full(c)).count() as u32;
        }
        assert_eq!(have, n, "{line}");
        total += n;
    }
    assert_eq!(total, terse_core::text::FULL_COUNT);
    let vectors = std::fs::read_to_string(root.join("vectors/full-text.txt")).unwrap();
    let ranges: Vec<(u16, u16)> = vectors
        .lines()
        .filter_map(|l| l.trim().strip_prefix("range "))
        .map(|r| {
            let (a, b) = r.split_once(' ').unwrap();
            (u16::from_str_radix(a, 16).unwrap(), u16::from_str_radix(b, 16).unwrap())
        })
        .collect();
    assert_eq!(ranges, terse_core::text::FULL);
}
