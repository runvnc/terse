//! The content a web page gives the machine: `try-content.bin` of the site,
//! which `terse manual site` writes (terse_net::try_page) from the programs
//! and manual of the `terse` that made the site.
//!
//! Format (little-endian): `TWB1`, a u32 count, then `count` entries, each
//! a kind byte, a u32 length and that many bytes. Kinds: 0 the shell's
//! image, 1 a built-in program's image (in the order `builtin` lists them),
//! 2 a blob (an article, a program's data), 3 the manual's index article
//! (a blob that the reader opens when it runs alone).

pub const MAGIC: &[u8; 4] = b"TWB1";

pub const SHELL: u8 = 0;
pub const PROGRAM: u8 = 1;
pub const BLOB: u8 = 2;
pub const MANUAL_INDEX: u8 = 3;

/// Encodes `entries` (kind, bytes) as a bundle.
pub fn encode(entries: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend((entries.len() as u32).to_le_bytes());
    for (kind, data) in entries {
        out.push(*kind);
        out.extend((data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

/// Decodes a bundle into (kind, bytes) entries.
pub fn decode(bytes: &[u8]) -> Result<Vec<(u8, Vec<u8>)>, String> {
    let rest = bytes.strip_prefix(&MAGIC[..]).ok_or("not a content bundle (no TWB1)")?;
    let take = |at: &mut usize, n: usize| -> Result<&[u8], String> {
        let s = rest.get(*at..*at + n).ok_or("content bundle cut short")?;
        *at += n;
        Ok(s)
    };
    let mut at = 0;
    let count = u32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap());
    let mut out = Vec::new();
    for _ in 0..count {
        let kind = take(&mut at, 1)?[0];
        let len = u32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap()) as usize;
        out.push((kind, take(&mut at, len)?.to_vec()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let b = encode(&[(SHELL, b"shell"), (PROGRAM, b""), (BLOB, b"text")]);
        assert_eq!(decode(&b).unwrap(), [(SHELL, b"shell".to_vec()), (PROGRAM, vec![]), (BLOB, b"text".to_vec())]);
        assert!(decode(&b[..b.len() - 1]).is_err());
        assert!(decode(b"nope").is_err());
    }
}
