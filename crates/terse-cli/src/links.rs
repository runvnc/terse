//! What `terse feed publish` says about the `terse:` links of an article
//! (ARTICLE.md 4): for each, whether this host holds what it leads to,
//! whether it is uploaded with the article, and whether a relay keeps it.
//!
//! An entry names one blob (NETWORK 4.2), so the article's entry keeps only
//! the article: a relay that drops blobs no entry names (NETWORK 5.3) keeps
//! what a link leads to only while an entry it keeps names that blob. A
//! linked blob named by an entry of the user's own feed is the user's own
//! and is uploaded again; one named by another feed's entry is someone
//! else's (**link-only**: kept where that feed is kept); one held here that
//! no entry names would be dropped by such a relay, so publishing refuses
//! it unless `--force` (publish it as an article of its own first). Figures
//! are written in the article's text (ARTICLE.md 6), so they always go with
//! it.

use std::collections::HashMap;

use terse_net::article_parse::{self, Block, Inline, Parsed, Target};
use terse_net::blob::{BlobId, to_hex};
use terse_net::entry::KIND_PROGRAM;
use terse_net::{FeedKey, Node};

/// What a link leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An article (or another blob), by ID.
    Blob(BlobId),
    Feed(FeedKey),
    Program(FeedKey, String),
}

/// Whether a relay keeps what a link leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Keep {
    /// An entry of the user's own feed names it: kept while that entry is.
    Own { seq: u32 },
    /// An entry of another feed names it (link-only).
    Others { feed: FeedKey, seq: u32 },
    /// Held here and named by no entry held here: a relay that drops blobs
    /// no entry names drops it.
    Unnamed,
    /// Not held here, and no entry held here names it (link-only).
    Unknown,
    /// A feed, or a program built into machine hosts: nothing to keep.
    Nothing,
}

/// One distinct link of an article.
pub struct Linked {
    pub kind: Kind,
    /// Whether this host holds the blob (for a program: its current
    /// version's image).
    pub held: bool,
    pub keep: Keep,
    /// The blob to upload with the article, if it is the user's own (or
    /// named by nothing) and held here.
    pub upload: Option<BlobId>,
}

impl Linked {
    /// The link as written in an article.
    pub fn link(&self) -> String {
        match &self.kind {
            Kind::Blob(id) => format!("terse:{}", to_hex(id)),
            Kind::Feed(key) => format!("terse:feed/{}", to_hex(key)),
            Kind::Program(key, name) => format!("terse:program/{}/{}", to_hex(key), encode_name(name)),
        }
    }

    /// One line: the link, whether it is held here, uploaded (`uploaded`:
    /// what the upload did, or `None` if there was none), and kept.
    pub fn line(&self, uploaded: Option<&str>, name: impl Fn(&FeedKey) -> String) -> String {
        let held = match (&self.kind, self.held) {
            (Kind::Feed(_), _) => "a feed, nothing to upload".to_string(),
            (Kind::Program(key, _), _) if *key == [0; 32] => "a program built into machine hosts, nothing to upload".to_string(),
            (_, held) => {
                let held = if held { "held here" } else { "not held here" };
                format!("{held}, {}", uploaded.unwrap_or("not uploaded"))
            }
        };
        let what = if matches!(self.kind, Kind::Program(..)) { "program entry" } else { "entry" };
        let keep = match &self.keep {
            Keep::Own { seq } => format!("kept: your feed's {what} #{seq} names it, and relays keep it while they keep that entry"),
            Keep::Others { feed, seq } => {
                format!("link-only: {}'s {what} #{seq} names it; kept where that feed is kept", name(feed))
            }
            Keep::Unnamed => "NOT KEPT: no entry names it, and a relay that drops blobs no entry names (NETWORK.md 5.3) drops it".into(),
            Keep::Unknown => "link-only: no entry held here names it; kept only where its own feed is kept".into(),
            Keep::Nothing => return format!("link {}: {held}", self.link()),
        };
        format!("link {}: {held}; {keep}", self.link())
    }
}

/// A program's name as a program link writes it (ARTICLE.md 4.1).
fn encode_name(name: &str) -> String {
    name.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Every distinct link of `text` that ARTICLE.md counts (not those in code
/// spans), in the order of the text.
pub fn links(text: &[u8]) -> Vec<Kind> {
    let Parsed::Article(blocks) = article_parse::parse(text) else { return Vec::new() };
    let mut out: Vec<Kind> = Vec::new();
    let mut add = |xs: &[Inline]| {
        for x in xs {
            let Inline::Link { target, .. } = x else { continue };
            let kind = match target {
                Target::Article(id) => Kind::Blob(*id),
                Target::Feed(key, _) => Kind::Feed(*key),
                Target::Program(key, name) => Kind::Program(*key, name.clone()),
            };
            if !out.contains(&kind) {
                out.push(kind);
            }
        }
    };
    for b in &blocks {
        match b {
            Block::Heading(x) | Block::Subheading(x) | Block::Paragraph(x) | Block::Item(x) => add(x),
            Block::Pre(lines) => lines.iter().for_each(|l| add(l)),
            Block::Table(t) => t.rows.iter().flatten().for_each(|c| add(c)),
            Block::Figure(_) => {}
        }
    }
    out
}

/// What each link of `text` leads to, as this host knows it; `own` is the
/// user's feed, if there is an identity.
pub fn survey(text: &[u8], node: &Node, own: Option<&FeedKey>) -> Vec<Linked> {
    let kinds = links(text);
    if kinds.is_empty() {
        return Vec::new();
    }
    // Every entry held, by the blob it names: the user's own first, then
    // the newest.
    let mut named: HashMap<BlobId, Vec<(FeedKey, u32, u32)>> = HashMap::new();
    let mut programs: HashMap<(FeedKey, String), (u32, BlobId)> = HashMap::new();
    for feed in node.feed_store().feeds().unwrap_or_default() {
        for s in node.feed_store().entries(&feed).unwrap_or_default() {
            let e = s.entry;
            named.entry(e.blob).or_default().push((feed, e.seq, e.size));
            if e.kind == KIND_PROGRAM {
                let p = programs.entry((feed, e.title.clone())).or_insert((e.seq, e.blob));
                if e.seq >= p.0 {
                    *p = (e.seq, e.blob);
                }
            }
        }
    }
    for v in named.values_mut() {
        v.sort_by_key(|&(feed, seq, _)| (Some(&feed) != own, std::cmp::Reverse(seq)));
    }
    let len_of = |id: &BlobId| std::fs::metadata(node.store().path(id)).ok().map(|m| m.len());
    kinds
        .into_iter()
        .map(|kind| match &kind {
            Kind::Feed(_) => Linked { kind, held: false, keep: Keep::Nothing, upload: None },
            Kind::Program(key, _) if *key == [0; 32] => Linked { kind, held: false, keep: Keep::Nothing, upload: None },
            Kind::Program(key, name) => {
                let current = programs.get(&(*key, name.clone())).copied();
                let held = current.is_some_and(|(_, blob)| len_of(&blob).is_some());
                let (keep, upload) = match current {
                    Some((seq, blob)) if Some(key) == own => (Keep::Own { seq }, held.then_some(blob)),
                    Some((seq, _)) => (Keep::Others { feed: *key, seq }, None),
                    None => (Keep::Unknown, None),
                };
                Linked { kind, held, keep, upload }
            }
            Kind::Blob(id) => {
                let len = len_of(id);
                // An entry names a blob only with its length as its size
                // (NETWORK 5.3).
                let entry = named.get(id).and_then(|v| v.iter().find(|&&(_, _, size)| len.is_none_or(|l| l == u64::from(size))));
                let (keep, upload) = match entry {
                    Some(&(feed, seq, _)) if Some(&feed) == own => (Keep::Own { seq }, len.map(|_| *id)),
                    Some(&(feed, seq, _)) => (Keep::Others { feed, seq }, None),
                    None if len.is_some() => (Keep::Unnamed, Some(*id)),
                    None => (Keep::Unknown, None),
                };
                Linked { kind, held: len.is_some(), keep, upload }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_in_order_without_repeats() {
        let a = "ab".repeat(32);
        let k = "cd".repeat(32);
        let text = format!(
            "# Title\n\nSee terse:{a} and [again](terse:{a}).\n\n- terse:feed/{k}@relay.example\n\n`terse:{k}` is code.\n\n| x |\n|---|\n| terse:program/{k}/Hello%20there |\n"
        );
        let found = links(text.as_bytes());
        assert_eq!(found, [Kind::Blob([0xab; 32]), Kind::Feed([0xcd; 32]), Kind::Program([0xcd; 32], "Hello there".into())]);
        let l = Linked { kind: found[2].clone(), held: false, keep: Keep::Unknown, upload: None };
        assert_eq!(l.link(), format!("terse:program/{k}/Hello%20there"));
    }
}
