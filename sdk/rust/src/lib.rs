//! Checks Tersenet feed entries: every validity rule of NETWORK.md 4.2,
//! with the strict Ed25519 verification the network asks for (ordinary
//! Ed25519 libraries accept signatures it refuses). It has no options,
//! no state, does no I/O and needs no allocator.
#![no_std]
#![forbid(unsafe_code)]

use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha512};

/// A valid entry's fields (NETWORK.md 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The feed key.
    pub feed: [u8; 32],
    /// 1 for the feed's first entry.
    pub seq: u32,
    /// Unix seconds, as claimed by the author.
    pub time: u32,
    /// 1 article, 2 profile, 3 program, 4 document.
    pub kind: u8,
    /// The blob's size in bytes.
    pub size: u32,
    /// The blob's ID.
    pub blob: [u8; 32],
    /// The blob replied to, or all zeros.
    pub reply_to: [u8; 32],
    pub title: &'a str,
}

/// The rule an entry breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Length,
    TitleLength,
    TitleUtf8,
    Seq,
    Kind,
    Size,
    Name,
    Key,
    SignatureS,
    SignatureR,
    Signature,
}

impl core::fmt::Display for Invalid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Length => "the entry's length does not match its title length",
            Self::TitleLength => "the title is longer than 120 bytes",
            Self::TitleUtf8 => "the title is not valid UTF-8",
            Self::Seq => "seq is 0",
            Self::Kind => "the kind is reserved (0 or 255)",
            Self::Size => "the size is not 1 to 1,048,576 bytes (4,096 for a profile)",
            Self::Name => "a profile's or a program's title is not a display name",
            Self::Key => "the feed key is not the canonical encoding of a point of large order",
            Self::SignatureS => "the signature's S is not below the group order",
            Self::SignatureR => "the signature's R is not a point of large order",
            Self::Signature => "the signature does not verify",
        })
    }
}

impl core::error::Error for Invalid {}

/// Checks an entry, as received, by every rule of NETWORK.md 4.2 and
/// returns its fields, or the first rule it breaks.
pub fn verify_entry(bytes: &[u8]) -> Result<Entry<'_>, Invalid> {
    if bytes.len() < 173 {
        return Err(Invalid::Length);
    }
    let n = bytes[108] as usize;
    if n > 120 {
        return Err(Invalid::TitleLength);
    }
    if bytes.len() != 173 + n {
        return Err(Invalid::Length);
    }
    let (signed, signature) = bytes.split_at(109 + n);
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let title = core::str::from_utf8(&signed[109..]).map_err(|_| Invalid::TitleUtf8)?;
    let e = Entry {
        feed: array(&bytes[0..32]),
        seq: u32_at(32),
        time: u32_at(36),
        kind: bytes[40],
        size: u32::from_le_bytes([bytes[41], bytes[42], bytes[43], 0]),
        blob: array(&bytes[44..76]),
        reply_to: array(&bytes[76..108]),
        title,
    };
    let max_size = if e.kind == 2 { 4096 } else { 1 << 20 };
    if e.seq == 0 {
        return Err(Invalid::Seq);
    } else if e.kind == 0 || e.kind == 255 {
        return Err(Invalid::Kind);
    } else if !(1..=max_size).contains(&e.size) {
        return Err(Invalid::Size);
    } else if (e.kind == 2 || e.kind == 3) && !display_name(title) {
        return Err(Invalid::Name);
    }
    check(&e.feed, &[b"terse-entry-0", signed], &array(signature))?;
    Ok(e)
}

/// Whether `signature` is a valid Ed25519 signature of `message` by
/// `key` under Tersenet's strict rules (NETWORK.md 4.2).
pub fn verify(key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    check(key, &[message], signature).is_ok()
}

/// Whether `key` is a valid feed key (NETWORK.md 4.1): the canonical
/// encoding of a point not of small order.
pub fn valid_feed_key(key: &[u8; 32]) -> bool {
    feed_point(key).is_some()
}

fn feed_point(key: &[u8; 32]) -> Option<EdwardsPoint> {
    let a = CompressedEdwardsY(*key).decompress()?;
    (a.compress().0 == *key && !a.is_small_order()).then_some(a)
}

/// The four checks of NETWORK.md 4.2, in order; the message is the
/// concatenation of `message`.
fn check(key: &[u8; 32], message: &[&[u8]], signature: &[u8; 64]) -> Result<(), Invalid> {
    let a = feed_point(key).ok_or(Invalid::Key)?;
    let r: [u8; 32] = array(&signature[..32]);
    let s = Option::from(Scalar::from_canonical_bytes(array(&signature[32..])));
    let s: Scalar = s.ok_or(Invalid::SignatureS)?;
    match CompressedEdwardsY(r).decompress() {
        Some(point) if !point.is_small_order() => {}
        _ => return Err(Invalid::SignatureR),
    }
    let mut h = Sha512::new();
    h.update(r);
    h.update(key);
    message.iter().for_each(|part| h.update(part));
    let k = Scalar::from_bytes_mod_order_wide(&array(&h.finalize()));
    // [S]B - [k]A, without the factor of 8, compared byte for byte.
    let p = EdwardsPoint::vartime_double_scalar_mul_basepoint(&k, &-a, &s);
    if p.compress().0 == r { Ok(()) } else { Err(Invalid::Signature) }
}

/// A display name (NETWORK.md 4.6): 1 to 48 bytes, no control
/// character, no white space first or last.
fn display_name(s: &str) -> bool {
    let control = |c: char| c < '\u{20}' || ('\u{7f}'..='\u{9f}').contains(&c);
    let ends = [s.chars().next(), s.chars().next_back()];
    (1..=48).contains(&s.len()) && !s.chars().any(control) && !ends.into_iter().flatten().any(space)
}

/// The Unicode White_Space property.
fn space(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}')
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

fn array<const N: usize>(b: &[u8]) -> [u8; N] {
    let mut a = [0; N];
    a.copy_from_slice(b);
    a
}
