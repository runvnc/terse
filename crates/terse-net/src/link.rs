//! A `terse:` link given to a program by hand (typed, pasted or on its
//! command line): the forms of ARTICLE.md's links, whole, with blanks at
//! either end ignored and hex digits in either case.

use crate::address::{AddressError, FeedAddress, parse_address};
use crate::blob::{BlobId, parse_hex};
use crate::identity::FeedKey;

/// What a link names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// `terse:` and a blob ID: an article, a program image or a document.
    Blob(BlobId),
    /// `terse:feed/` and a feed address (NETWORK 4.9).
    Feed(FeedAddress),
    /// `terse:program/`, the publisher's feed key (all zero for a built-in
    /// program), `/` and the program's name, percent-decoded.
    Program { publisher: FeedKey, name: Vec<u8> },
}

/// Parses a link. `None` if `text` does not begin with `terse:`; otherwise
/// the link, or what is wrong with it, in words.
pub fn parse(text: &str) -> Option<Result<Link, String>> {
    let rest = text.trim().strip_prefix("terse:")?;
    Some(parse_rest(rest))
}

fn parse_rest(rest: &str) -> Result<Link, String> {
    if let Some(address) = rest.strip_prefix("feed/") {
        return parse_address(address).map(Link::Feed).map_err(|e| match e {
            AddressError::Key => "not a feed link: terse:feed/ is followed by a key, 64 hex digits".to_string(),
            e => format!("not a feed link: {e}"),
        });
    }
    if let Some(program) = rest.strip_prefix("program/") {
        let bad = || "not a program link: terse:program/ is followed by a key (64 hex digits), / and the program's name".to_string();
        let (key, name) = program.split_once('/').ok_or_else(bad)?;
        let publisher = parse_hex(key).ok_or_else(bad)?;
        let name = decode_name(name).ok_or_else(|| format!("not a program link: {name:?} is not a program's name"))?;
        return Ok(Link::Program { publisher, name });
    }
    parse_hex(rest).map(Link::Blob).ok_or_else(|| {
        "not a link: terse: is followed by an article's ID (64 hex digits), feed/ and a key, or program/, a key, / and a name"
            .to_string()
    })
}

/// A program link's name: RFC 3986 unreserved characters and %XX triples,
/// decoded to 1 to 48 bytes of UTF-8 without control characters.
fn decode_name(text: &str) -> Option<Vec<u8>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else if b[i].is_ascii_alphanumeric() || b"-._~".contains(&b[i]) {
            out.push(b[i]);
            i += 1;
        } else {
            return None;
        }
    }
    let s = std::str::from_utf8(&out).ok()?;
    (!out.is_empty() && out.len() <= 48 && !s.chars().any(char::is_control)).then_some(out)
}

/// A blob ID given on a command line: 64 hex digits, or the link
/// `terse:` and 64 hex digits.
pub fn blob_arg(text: &str) -> Option<BlobId> {
    match parse(text) {
        Some(Ok(Link::Blob(id))) => Some(id),
        Some(_) => None,
        None => parse_hex(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        let id = "3607930000000000000000000000000000000000000000000000000000dec189";
        assert_eq!(parse(&format!(" terse:{id}\n")), Some(Ok(Link::Blob(parse_hex(id).unwrap()))));
        assert_eq!(parse(&format!("terse:{}", id.to_uppercase())), Some(Ok(Link::Blob(parse_hex(id).unwrap()))));
        assert_eq!(parse(id), None, "not a link");
        assert_eq!(blob_arg(id), parse_hex(id));
        assert_eq!(blob_arg(&format!("terse:{id}")), parse_hex(id));
        assert!(matches!(parse("terse:1234"), Some(Err(e)) if e.starts_with("not a link")));
        let zero = "0".repeat(64);
        assert_eq!(
            parse(&format!("terse:program/{zero}/sheet")),
            Some(Ok(Link::Program { publisher: [0; 32], name: b"sheet".to_vec() }))
        );
        assert_eq!(
            parse(&format!("terse:program/{zero}/Lumen%20Reef")),
            Some(Ok(Link::Program { publisher: [0; 32], name: b"Lumen Reef".to_vec() }))
        );
        assert!(matches!(parse(&format!("terse:program/{zero}/a b")), Some(Err(_))));
        assert!(matches!(parse(&format!("terse:program/{zero}/")), Some(Err(_))));
        assert!(matches!(parse(&format!("terse:program/{zero}/%4")), Some(Err(_))));
        assert!(matches!(parse("terse:feed/abc"), Some(Err(e)) if e.contains("64 hex digits")));
    }
}
