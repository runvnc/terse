//! The Terse manual (docs/manual): articles whose links to each other are
//! written as placeholders, `terse:{name}`, which [`build`] replaces with
//! the target's blob ID. IDs are content hashes, so an article's ID
//! depends on the IDs it links to: the links must form no cycle, and the
//! articles are resolved in dependency order.
//!
//! Sources: `docs/manual/manifest` lists the article names in reading order
//! (blank lines and `#` comments ignored); each article is
//! `docs/manual/<name>.txt` and starts with a line `# <title>`. The build
//! output, checked in, is `docs/manual/build/<name>.txt` and
//! `docs/manual/build/manifest`, one line per article in reading order:
//! `<blob ID> <name> <title>`.

use std::collections::HashMap;
use std::path::Path;

use crate::{BlobId, blob_id, to_hex};

/// One built article.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub name: String,
    pub title: String,
    pub text: String,
    pub id: BlobId,
}

/// The names in a manifest's text, in order.
pub fn manifest_names(manifest: &str) -> Vec<String> {
    manifest.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect()
}

/// The placeholders of `text`: each `terse:{name}`, as (byte range, name).
fn placeholders(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find("terse:{") {
        let start = at + i;
        let open = start + "terse:{".len();
        match text[open..].find('}') {
            Some(j) if text[open..open + j].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') => {
                out.push((start..open + j + 1, &text[open..open + j]));
                at = open + j + 1;
            }
            _ => at = open,
        }
    }
    out
}

/// The title of an article: its first line, `# <title>`.
pub fn title_of(text: &str) -> Option<&str> {
    let first = text.lines().next()?;
    let t = first.strip_prefix("# ")?.trim();
    (!t.is_empty() && t.len() <= 120).then_some(t)
}

/// Resolves the articles `sources` (name, text) in manifest order: every
/// placeholder is replaced with `terse:` and the target's blob ID. Errors
/// name an unknown target, a cycle of links, or a missing title.
pub fn build(sources: &[(String, String)]) -> Result<Vec<Built>, String> {
    let index: HashMap<&str, usize> = sources.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    if index.len() != sources.len() {
        return Err("an article is listed twice in the manifest".into());
    }
    for (name, text) in sources {
        if title_of(text).is_none() {
            return Err(format!("{name}: the first line must be `# <title>` (1 to 120 bytes)"));
        }
        for (_, target) in placeholders(text) {
            if !index.contains_key(target) {
                return Err(format!("{name}: link to an unknown article terse:{{{target}}}"));
            }
        }
    }
    // Depth-first, in manifest order: 0 unvisited, 1 on the path, 2 done.
    let mut state = vec![0u8; sources.len()];
    let mut ids: Vec<Option<BlobId>> = vec![None; sources.len()];
    let mut texts: Vec<String> = vec![String::new(); sources.len()];
    fn visit(
        i: usize,
        sources: &[(String, String)],
        index: &HashMap<&str, usize>,
        state: &mut [u8],
        ids: &mut [Option<BlobId>],
        texts: &mut [String],
        path: &mut Vec<usize>,
    ) -> Result<(), String> {
        match state[i] {
            2 => return Ok(()),
            1 => {
                let from = path.iter().position(|&p| p == i).unwrap();
                let cycle: Vec<&str> = path[from..].iter().chain([&i]).map(|&p| sources[p].0.as_str()).collect();
                return Err(format!("the links form a cycle: {}", cycle.join(" -> ")));
            }
            _ => {}
        }
        state[i] = 1;
        path.push(i);
        let text = &sources[i].1;
        for (_, target) in placeholders(text) {
            visit(index[target], sources, index, state, ids, texts, path)?;
        }
        let mut out = String::with_capacity(text.len() + 64);
        let mut at = 0;
        for (range, target) in placeholders(text) {
            out.push_str(&text[at..range.start]);
            out.push_str("terse:");
            out.push_str(&to_hex(&ids[index[target]].unwrap()));
            at = range.end;
        }
        out.push_str(&text[at..]);
        ids[i] = Some(blob_id(out.as_bytes()));
        texts[i] = out;
        path.pop();
        state[i] = 2;
        Ok(())
    }
    for i in 0..sources.len() {
        visit(i, sources, &index, &mut state, &mut ids, &mut texts, &mut Vec::new())?;
    }
    Ok(sources
        .iter()
        .enumerate()
        .map(|(i, (name, src))| Built {
            name: name.clone(),
            title: title_of(src).unwrap().to_string(),
            text: std::mem::take(&mut texts[i]),
            id: ids[i].unwrap(),
        })
        .collect())
}

/// Reads the sources of the manual in `dir` (its manifest and articles).
pub fn read_sources(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let manifest = std::fs::read_to_string(dir.join("manifest")).map_err(|e| format!("{}: {e}", dir.join("manifest").display()))?;
    manifest_names(&manifest)
        .into_iter()
        .map(|name| {
            let path = dir.join(format!("{name}.txt"));
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((name, text))
        })
        .collect()
}

/// The build manifest: `<blob ID> <name> <title>` per article.
pub fn build_manifest(built: &[Built]) -> String {
    built.iter().map(|b| format!("{} {} {}\n", to_hex(&b.id), b.name, b.title)).collect()
}

/// Builds the manual in `dir` and writes `dir/build`: the articles and the
/// build manifest; files of articles no longer listed are removed.
pub fn build_dir(dir: &Path) -> Result<Vec<Built>, String> {
    let built = build(&read_sources(dir)?)?;
    let out = dir.join("build");
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let write = |path: &Path, data: &str| std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()));
    for entry in std::fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".txt") && !built.iter().any(|b| format!("{}.txt", b.name) == name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    for b in &built {
        write(&out.join(format!("{}.txt", b.name)), &b.text)?;
    }
    write(&out.join("manifest"), &build_manifest(&built))?;
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect()
    }

    #[test]
    fn links_are_resolved_in_dependency_order() {
        let s = src(&[
            ("index", "# Index\n\n[A](terse:{a}) and terse:{b}, not terse:{x y} or terse:{\n"),
            ("a", "# A\n\nsee [B](terse:{b})\n"),
            ("b", "# B\n\nleaf\n"),
        ]);
        let built = build(&s).unwrap();
        let b = blob_id(b"# B\n\nleaf\n");
        assert_eq!(built[2].id, b);
        assert_eq!(built[1].text, format!("# A\n\nsee [B](terse:{})\n", to_hex(&b)));
        assert_eq!(built[1].id, blob_id(built[1].text.as_bytes()));
        assert!(built[0].text.contains(&format!("[A](terse:{})", to_hex(&built[1].id))));
        assert!(built[0].text.contains("not terse:{x y} or terse:{\n"), "not placeholders: left as they are");
        assert_eq!(built[0].title, "Index");
        // Deterministic.
        assert_eq!(build(&s).unwrap(), built);
        assert!(build_manifest(&built).starts_with(&format!("{} index Index\n", to_hex(&built[0].id))));
    }

    #[test]
    fn cycles_unknown_names_and_titles_are_errors() {
        let e = build(&src(&[("a", "# A\nterse:{b}"), ("b", "# B\nterse:{c}"), ("c", "# C\nterse:{a}")])).unwrap_err();
        assert_eq!(e, "the links form a cycle: a -> b -> c -> a");
        let e = build(&src(&[("a", "# A\nterse:{a}")])).unwrap_err();
        assert_eq!(e, "the links form a cycle: a -> a");
        let e = build(&src(&[("a", "# A\n[x](terse:{nope})")])).unwrap_err();
        assert!(e.contains("unknown article terse:{nope}"), "{e}");
        assert!(build(&src(&[("a", "no title\n")])).unwrap_err().contains("first line"));
        assert!(build(&src(&[("a", "# A\n"), ("a", "# A\n")])).is_err());
    }
}
