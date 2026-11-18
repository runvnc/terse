//! Blob IDs (NETWORK 1).

pub use terse_core::content::MAX_BLOB;

/// BLAKE3-256 hash of a blob's bytes.
pub type BlobId = [u8; 32];

pub fn blob_id(bytes: &[u8]) -> BlobId {
    *blake3::hash(bytes).as_bytes()
}

/// A blob is 1 to 1,048,576 bytes.
pub fn valid_size(len: usize) -> bool {
    (1..=MAX_BLOB).contains(&len)
}

/// The ID as 64 lowercase hexadecimal digits.
pub fn to_hex(id: &BlobId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses exactly 64 hexadecimal digits (either case).
pub fn parse_hex(s: &str) -> Option<BlobId> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut id = [0u8; 32];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let id = blob_id(b"abc");
        let hex = to_hex(&id);
        assert_eq!(hex, "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
        assert_eq!(parse_hex(&hex), Some(id));
        assert_eq!(parse_hex(&hex.to_uppercase()), Some(id));
        assert_eq!(parse_hex(&hex[1..]), None);
        assert_eq!(parse_hex(&format!("{}g", &hex[1..])), None);
        assert_eq!(parse_hex(&format!("{hex}0")), None);
    }

    #[test]
    fn sizes() {
        assert!(!valid_size(0));
        assert!(valid_size(1) && valid_size(MAX_BLOB));
        assert!(!valid_size(MAX_BLOB + 1));
    }
}
