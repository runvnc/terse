//! `terse:` links in article text (ARTICLE.md, Links), as prefetch finds
//! them (NETWORK 4.4).
//!
//! A link is `terse:` followed by 64 lowercase hex digits (an article, by
//! blob ID) or `terse:feed/` followed by 64 lowercase hex digits (a feed
//! key). It is not preceded by an ASCII letter or digit, and the 64 digits
//! are not followed by one. This is a plain scan of the bytes: it does not
//! parse the article's blocks, so it also finds the link forms in code
//! spans, which ARTICLE.md does not count as links (NETWORK 4.4 allows it:
//! prefetch is only a hint).

use crate::blob::BlobId;
use crate::identity::FeedKey;

const PREFIX: &[u8] = b"terse:";
const FEED: &[u8] = b"feed/";

/// A `terse:` link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Article(BlobId),
    Feed(FeedKey),
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// 32 bytes from exactly 64 lowercase hex digits at the start of `s` that
/// are not followed by a letter or digit.
fn key_at(s: &[u8]) -> Option<[u8; 32]> {
    let digits = s.get(..64)?;
    if s.get(64).is_some_and(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in digits.chunks_exact(2).enumerate() {
        out[i] = hex_digit(pair[0])? << 4 | hex_digit(pair[1])?;
    }
    Some(out)
}

/// Every `terse:` link in `text`, in order (repeats included).
pub fn links(text: &[u8]) -> Vec<Link> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + PREFIX.len() <= text.len() {
        if &text[i..i + PREFIX.len()] != PREFIX || (i > 0 && text[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let rest = &text[i + PREFIX.len()..];
        if let Some(id) = key_at(rest) {
            out.push(Link::Article(id));
            i += PREFIX.len() + 64;
        } else if let Some(key) = rest.strip_prefix(FEED).and_then(key_at) {
            out.push(Link::Feed(key));
            i += PREFIX.len() + FEED.len() + 64;
        } else {
            i += PREFIX.len();
        }
    }
    out
}

/// The article links (`terse:<id>`) in `text`, in order (repeats included).
pub fn terse_links(text: &[u8]) -> Vec<BlobId> {
    links(text)
        .into_iter()
        .filter_map(|l| match l {
            Link::Article(id) => Some(id),
            Link::Feed(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn id(s: &str) -> BlobId {
        crate::blob::parse_hex(s).unwrap()
    }

    #[test]
    fn article_and_feed_links() {
        let b = "f".repeat(64);
        let text = format!("See terse:{A}, then (terse:feed/{b}) and terse:{b}.\nterse:{A}");
        assert_eq!(links(text.as_bytes()), [Link::Article(id(A)), Link::Feed(id(&b)), Link::Article(id(&b)), Link::Article(id(A))]);
        assert_eq!(terse_links(text.as_bytes()), [id(A), id(&b), id(A)]);
    }

    #[test]
    fn boundaries() {
        let upper = A.to_uppercase();
        let not = [
            format!("xterse:{A}"),
            format!("9terse:{A}"),
            format!("terse:{}", &A[..63]),
            format!("terse:{A}0"),
            format!("terse:{A}g"),
            format!("terse:{upper}"),
            format!("terse:feed/{}", &A[..63]),
            format!("terse: {A}"),
            format!("TERSE:{A}"),
        ];
        for t in &not {
            assert!(links(t.as_bytes()).is_empty(), "{t}");
        }
        for t in [format!("-terse:{A}"), format!("é terse:{A}/"), format!("terse:{A}_"), format!("terse:terse:{A}")] {
            assert_eq!(terse_links(t.as_bytes()), [id(A)], "{t}");
        }
        // A feed link is not an article link.
        assert!(terse_links(format!("terse:feed/{A}").as_bytes()).is_empty());
    }

    /// Links with text (ARTICLE.md): the target in the parentheses is
    /// found like a bare link, also by prefetch.
    #[test]
    fn links_with_text() {
        let b = "f".repeat(64);
        let text = format!("- [How Terse works](terse:{A}), and [a feed](terse:feed/{b}).\n[x]( terse:{b})");
        assert_eq!(links(text.as_bytes()), [Link::Article(id(A)), Link::Feed(id(&b)), Link::Article(id(&b))]);
        assert_eq!(terse_links(text.as_bytes()), [id(A), id(&b)]);
    }
}
