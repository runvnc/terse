//! Feed entries (NETWORK 4.2): encoding, signing and the validity rules.

use ed25519_dalek::{Signature, VerifyingKey};

use crate::blob::{BlobId, MAX_BLOB};
use crate::identity::{FeedKey, Identity};

/// Prefixed to the signed bytes so a signature over an entry cannot be
/// mistaken for one over anything else.
pub const DOMAIN: &[u8] = b"terse-entry-0";
pub const KIND_ARTICLE: u8 = 1;
pub const MAX_TITLE: usize = 120;
/// Bytes before the title.
const HEAD: usize = 109;
const SIG: usize = 64;
/// Length of the signature at the end of an entry.
pub const SIGNATURE_LEN: usize = SIG;
pub const MAX_ENTRY: usize = HEAD + MAX_TITLE + SIG;

/// An entry's fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub feed: FeedKey,
    pub seq: u32,
    /// Unix seconds, as claimed by the author.
    pub time: u32,
    pub kind: u8,
    /// Size of the blob in bytes.
    pub size: u32,
    pub blob: BlobId,
    /// The blob this entry responds to, or all zeros.
    pub reply_to: BlobId,
    pub title: String,
}

/// A valid entry and its exact bytes (as signed and as sent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedEntry {
    pub entry: Entry,
    pub bytes: Vec<u8>,
}

/// Why an entry is invalid (NETWORK 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// Shorter than the fixed part, or the length does not match `n`.
    Length,
    /// `n` is over 120.
    TitleLength,
    TitleUtf8,
    Seq,
    Kind,
    Size,
    /// The feed key is not a usable Ed25519 public key.
    Key,
    Signature,
    /// Signing with an identity whose key is not the entry's feed key.
    WrongIdentity,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Length => "length does not match the title length",
            Self::TitleLength => "title longer than 120 bytes",
            Self::TitleUtf8 => "title is not UTF-8",
            Self::Seq => "seq is 0",
            Self::Kind => "unknown kind",
            Self::Size => "size is not 1 to 1,048,576",
            Self::Key => "feed key is not a usable Ed25519 public key",
            Self::Signature => "signature does not verify",
            Self::WrongIdentity => "identity does not own this feed",
        })
    }
}

impl std::error::Error for Invalid {}

impl Entry {
    /// An article entry for `data`.
    pub fn article(feed: FeedKey, seq: u32, time: u32, data: &[u8], reply_to: Option<BlobId>, title: &str) -> Self {
        Entry {
            feed,
            seq,
            time,
            kind: KIND_ARTICLE,
            size: data.len() as u32,
            blob: crate::blob::blob_id(data),
            reply_to: reply_to.unwrap_or([0; 32]),
            title: title.to_string(),
        }
    }

    /// The validity rules that do not involve the signature.
    fn check(&self) -> Result<(), Invalid> {
        if self.title.len() > MAX_TITLE {
            return Err(Invalid::TitleLength);
        }
        if self.seq == 0 {
            return Err(Invalid::Seq);
        }
        if self.kind != KIND_ARTICLE {
            return Err(Invalid::Kind);
        }
        if !(1..=MAX_BLOB as u32).contains(&self.size) {
            return Err(Invalid::Size);
        }
        Ok(())
    }

    /// Bytes 0 to 108 + `n`: everything but the signature. The title must
    /// already be at most 120 bytes and the size below 2^24.
    fn unsigned_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(HEAD + self.title.len() + SIG);
        b.extend_from_slice(&self.feed);
        b.extend_from_slice(&self.seq.to_le_bytes());
        b.extend_from_slice(&self.time.to_le_bytes());
        b.push(self.kind);
        b.extend_from_slice(&self.size.to_le_bytes()[..3]);
        b.extend_from_slice(&self.blob);
        b.extend_from_slice(&self.reply_to);
        b.push(self.title.len() as u8);
        b.extend_from_slice(self.title.as_bytes());
        b
    }

    pub fn sign(self, identity: &Identity) -> Result<SignedEntry, Invalid> {
        self.check()?;
        if identity.feed_key() != self.feed {
            return Err(Invalid::WrongIdentity);
        }
        let mut bytes = self.unsigned_bytes();
        let signature = identity.sign(&signed_message(&bytes));
        bytes.extend_from_slice(&signature);
        Ok(SignedEntry { entry: self, bytes })
    }
}

fn signed_message(unsigned: &[u8]) -> Vec<u8> {
    [DOMAIN, unsigned].concat()
}

/// Decodes and validates an entry (NETWORK 4.2).
pub fn decode(bytes: &[u8]) -> Result<SignedEntry, Invalid> {
    if bytes.len() < HEAD + SIG {
        return Err(Invalid::Length);
    }
    let n = bytes[HEAD - 1] as usize;
    if n > MAX_TITLE {
        return Err(Invalid::TitleLength);
    }
    if bytes.len() != HEAD + n + SIG {
        return Err(Invalid::Length);
    }
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let id_at = |at: usize| -> [u8; 32] { bytes[at..at + 32].try_into().unwrap() };
    let title = std::str::from_utf8(&bytes[HEAD..HEAD + n]).map_err(|_| Invalid::TitleUtf8)?;
    let entry = Entry {
        feed: id_at(0),
        seq: u32_at(32),
        time: u32_at(36),
        kind: bytes[40],
        size: u32::from_le_bytes([bytes[41], bytes[42], bytes[43], 0]),
        blob: id_at(44),
        reply_to: id_at(76),
        title: title.to_string(),
    };
    entry.check()?;
    let key = VerifyingKey::from_bytes(&entry.feed).map_err(|_| Invalid::Key)?;
    let signature = Signature::from_bytes(bytes[HEAD + n..].try_into().unwrap());
    // Strict verification: rejects small-order keys and non-canonical
    // signatures, so each valid entry has exactly one valid encoding.
    key.verify_strict(&signed_message(&bytes[..HEAD + n]), &signature).map_err(|_| Invalid::Signature)?;
    Ok(SignedEntry { entry, bytes: bytes.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{blob_id, to_hex};

    /// Seed 00 01 02 … 1f.
    fn test_identity() -> Identity {
        Identity::from_seed(std::array::from_fn(|i| i as u8))
    }

    fn sample(identity: &Identity) -> Entry {
        Entry::article(identity.feed_key(), 1, 1_700_000_000, b"Hello, Terse!", None, "Hello")
    }

    /// A fixed vector so other implementations can check their encoding and
    /// signing: secret key seed 000102…1f, seq 1, time 1,700,000,000, kind 1,
    /// the blob "Hello, Terse!" (13 bytes), no reply-to, title "Hello".
    #[test]
    fn test_vector() {
        let id = test_identity();
        let signed = sample(&id).sign(&id).unwrap();
        assert_eq!(to_hex(&id.feed_key()), "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8");
        assert_eq!(to_hex(&blob_id(b"Hello, Terse!")), "f47ef1568e6932e823c48e1f5700153ea5c3b96a0cd5b346f60e1a70b3588fe8");
        let hex: String = signed.bytes.iter().map(|b| format!("{b:02x}")).collect();
        let expected = concat!(
            "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8", // feed key
            "01000000",                                                         // seq 1
            "00f15365",                                                         // time
            "01",                                                               // kind
            "0d0000",                                                           // size 13
            "f47ef1568e6932e823c48e1f5700153ea5c3b96a0cd5b346f60e1a70b3588fe8", // blob ID
            "0000000000000000000000000000000000000000000000000000000000000000", // reply-to
            "05",                                                               // n
            "48656c6c6f",                                                       // "Hello"
            "fad3de68a1e3ee879d622f7efefc267d0e1778db22ae4fef89f2070b50c89272", // signature
            "3189587ecfa3cc3dba997633b35eb5793a310578e40d1a4500cceea6ad9f860c",
        );
        assert_eq!(hex, expected);
        assert_eq!(decode(&signed.bytes).unwrap(), signed);
    }

    #[test]
    fn round_trip() {
        let id = test_identity();
        let mut e = sample(&id);
        e.reply_to = [5; 32];
        e.title = "日本語のタイトル".into();
        let signed = e.clone().sign(&id).unwrap();
        assert_eq!(signed.bytes.len(), HEAD + e.title.len() + SIG);
        assert_eq!(decode(&signed.bytes), Ok(signed.clone()));
        assert_eq!(decode(&signed.bytes).unwrap().entry, e);

        e.title = "x".repeat(MAX_TITLE);
        let longest = e.sign(&id).unwrap();
        assert_eq!(longest.bytes.len(), MAX_ENTRY);
        assert!(decode(&longest.bytes).is_ok());
    }

    /// Re-signs `bytes` after an edit, so only the rule under test fails.
    fn resign(id: &Identity, mut bytes: Vec<u8>) -> Vec<u8> {
        let end = bytes.len() - SIG;
        let sig = id.sign(&signed_message(&bytes[..end]));
        bytes[end..].copy_from_slice(&sig);
        bytes
    }

    #[test]
    fn validity_rules() {
        let id = test_identity();
        let good = sample(&id).sign(&id).unwrap().bytes;
        let edited = |at: usize, value: u8| {
            let mut b = good.clone();
            b[at] = value;
            resign(&id, b)
        };
        assert_eq!(decode(&good[..good.len() - 1]), Err(Invalid::Length));
        assert_eq!(decode(&[good.clone(), vec![0]].concat()), Err(Invalid::Length));
        assert_eq!(decode(&good[..HEAD + SIG - 1]), Err(Invalid::Length));
        assert_eq!(decode(&edited(HEAD - 1, 6)), Err(Invalid::Length), "n says 6, title is 5");
        assert_eq!(decode(&edited(32, 0)), Err(Invalid::Seq));
        assert_eq!(decode(&edited(40, 2)), Err(Invalid::Kind));
        assert_eq!(decode(&edited(40, 0)), Err(Invalid::Kind));
        let mut zero_size = good.clone();
        zero_size[41..44].fill(0);
        assert_eq!(decode(&resign(&id, zero_size)), Err(Invalid::Size));
        let mut too_big = good.clone();
        too_big[41..44].copy_from_slice(&(MAX_BLOB as u32 + 1).to_le_bytes()[..3]);
        assert_eq!(decode(&resign(&id, too_big)), Err(Invalid::Size));
        let mut max = good.clone();
        max[41..44].copy_from_slice(&(MAX_BLOB as u32).to_le_bytes()[..3]);
        assert!(decode(&resign(&id, max)).is_ok());
        assert_eq!(decode(&edited(HEAD, 0xFF)), Err(Invalid::TitleUtf8));

        // n over 120, with a matching length.
        let mut long = good[..HEAD].to_vec();
        long[HEAD - 1] = 121;
        long.extend(std::iter::repeat_n(b'x', 121));
        long.extend_from_slice(&[0; SIG]);
        assert_eq!(decode(&resign(&id, long)), Err(Invalid::TitleLength));

        // Any change to the signed bytes, the signature, or the key.
        assert_eq!(decode(&{ let mut b = good.clone(); b[36] ^= 1; b }), Err(Invalid::Signature));
        assert_eq!(decode(&{ let mut b = good.clone(); b[HEAD + 5] ^= 1; b }), Err(Invalid::Signature));
        let other = Identity::from_seed([9; 32]);
        let mut b = good.clone();
        b[..32].copy_from_slice(&other.feed_key());
        assert_eq!(decode(&b), Err(Invalid::Signature));
        let mut b = good.clone();
        b[..32].fill(0);
        assert!(matches!(decode(&b), Err(Invalid::Signature | Invalid::Key)));
    }

    /// The signature covers exactly `terse-entry-0` followed by bytes 0 to
    /// 108 + n.
    #[test]
    fn signature_covers_the_specified_bytes() {
        let id = test_identity();
        let signed = sample(&id).sign(&id).unwrap();
        let n = signed.entry.title.len();
        let mut message = b"terse-entry-0".to_vec();
        message.extend_from_slice(&signed.bytes[..109 + n]);
        let key = VerifyingKey::from_bytes(&id.feed_key()).unwrap();
        let sig = Signature::from_bytes(signed.bytes[109 + n..].try_into().unwrap());
        assert!(key.verify_strict(&message, &sig).is_ok());
        assert!(key.verify_strict(&signed.bytes[..109 + n], &sig).is_err(), "domain prefix is required");
    }

    #[test]
    fn signing_checks_fields() {
        let id = test_identity();
        let with = |f: fn(&mut Entry)| {
            let mut e = sample(&id);
            f(&mut e);
            e.sign(&id).map(|_| ())
        };
        assert_eq!(with(|e| e.seq = 0), Err(Invalid::Seq));
        assert_eq!(with(|e| e.size = 0), Err(Invalid::Size));
        assert_eq!(with(|e| e.kind = 3), Err(Invalid::Kind));
        assert_eq!(with(|e| e.title = "x".repeat(121)), Err(Invalid::TitleLength));
        assert_eq!(sample(&id).sign(&Identity::from_seed([1; 32])), Err(Invalid::WrongIdentity));
    }
}
