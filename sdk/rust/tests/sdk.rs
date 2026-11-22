//! The SDK against the project's vectors (vectors/entries.txt, with the
//! rule each invalid one breaks from sdk/testdata/reasons.txt), against
//! the classic Ed25519 strictness cases (generated here into
//! sdk/testdata/strict.txt, which the Go SDK's tests read too; set
//! UPDATE_GOLDEN=1 to rewrite it), against the reference's check before
//! it used this crate (ed25519-dalek's `verify_strict` with a
//! canonical-key check), and on arbitrary bytes (no panics).

use std::path::PathBuf;

use curve25519_dalek::constants::ED25519_BASEPOINT_POINT as B;
use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::IsIdentity;
use ed25519_dalek::Verifier;
use sha2::{Digest, Sha512};
use tersenet::{Invalid, valid_feed_key, verify, verify_entry};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn hex(text: &str) -> Vec<u8> {
    if text == "-" {
        return Vec::new();
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn rule(r: Result<(), Invalid>) -> String {
    match r {
        Ok(()) => "valid".into(),
        Err(e) => format!("{e:?}"),
    }
}

/// The records of a vectors-style file: (first line, fields).
fn records(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    text.split("\n\n")
        .filter_map(|r| {
            let mut lines = r.lines().filter(|l| !l.starts_with('#'));
            let head = lines.next()?.to_string();
            let fields = lines
                .filter_map(|l| l.strip_prefix("  ")?.split_once(' '))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            Some((head, fields))
        })
        .collect()
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> &'a str {
    fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).unwrap_or_else(|| panic!("no {name}"))
}

fn unquote(q: &str) -> String {
    let mut out = String::new();
    let mut chars = q.strip_prefix('"').unwrap().strip_suffix('"').unwrap().chars();
    while let Some(c) = chars.next() {
        out.push(if c != '\\' {
            c
        } else {
            match chars.next().unwrap() {
                't' => '\t',
                'n' => '\n',
                'r' => '\r',
                o => o,
            }
        });
    }
    out
}

fn entry_vectors() -> Vec<(String, Vec<(String, String)>)> {
    records(&std::fs::read_to_string(repo().join("vectors/entries.txt")).unwrap())
}

fn test_seed() -> [u8; 32] {
    let key = entry_vectors().into_iter().find(|(h, _)| h == "key test").unwrap();
    hex(field(&key.1, "secret-seed")).try_into().unwrap()
}

/// The reference's check before it used this crate: a canonical key,
/// then ed25519-dalek's `verify_strict`.
fn dalek_strict(key: &[u8; 32], message: &[u8], sig: &[u8; 64]) -> bool {
    let canonical = CompressedEdwardsY(*key).decompress().is_some_and(|p| p.compress().0 == *key);
    let sig = ed25519_dalek::Signature::from_bytes(sig);
    canonical
        && ed25519_dalek::VerifyingKey::from_bytes(key).is_ok_and(|k| k.verify_strict(message, &sig).is_ok())
}

#[test]
fn entry_vectors_accepted_or_refused_for_their_rule() {
    let reasons = std::fs::read_to_string(repo().join("sdk/testdata/reasons.txt")).unwrap();
    let reasons: Vec<(&str, &str)> =
        reasons.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once(' ')).collect();
    let (mut valid, mut invalid) = (0, 0);
    for (head, f) in entry_vectors() {
        let Some(name) = head.strip_prefix("entry ") else { continue };
        let bytes = hex(field(&f, "bytes"));
        let got = verify_entry(&bytes);
        if field(&f, "valid") == "yes" {
            let e = got.unwrap_or_else(|e| panic!("{name}: refused: {e}"));
            assert_eq!(to_hex(&e.feed), field(&f, "feed-key"), "{name}");
            assert_eq!(e.seq.to_string(), field(&f, "seq"), "{name}");
            assert_eq!(e.time.to_string(), field(&f, "time"), "{name}");
            assert_eq!(e.kind.to_string(), field(&f, "kind"), "{name}");
            assert_eq!(e.size.to_string(), field(&f, "size"), "{name}");
            assert_eq!(to_hex(&e.blob), field(&f, "blob-id"), "{name}");
            assert_eq!(to_hex(&e.reply_to), field(&f, "reply-to"), "{name}");
            assert_eq!(e.title, unquote(field(&f, "title")), "{name}");
            valid += 1;
        } else {
            let want = reasons.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("{name}: no reason")).1;
            assert_eq!(rule(got.map(|_| ())), want, "{name}");
            invalid += 1;
        }
    }
    assert_eq!(reasons.len(), invalid, "every reason is a vector");
    assert!(valid >= 10 && invalid >= 24, "{valid} valid, {invalid} invalid");
}

/// The eight points of small order: [i]T for a point T of order 8.
fn torsion() -> Vec<EdwardsPoint> {
    let inv8 = Scalar::from(8u8).invert();
    let t = (2u8..)
        .filter_map(|y| CompressedEdwardsY([[y].as_slice(), &[0; 31]].concat().try_into().unwrap()).decompress())
        .map(|p| p - p.mul_by_cofactor() * inv8)
        .find(|t| !(t + t + t + t).is_identity())
        .unwrap();
    (0..8u8).map(|i| t * Scalar::from(i)).collect()
}

/// A field element as 32 bytes, little-endian: p + `add` (p = 2^255 - 19).
fn p_plus(add: u8, sign: bool) -> [u8; 32] {
    let mut b = [0xff; 32];
    b[31] = 0x7f;
    b[0] = 0xed + add;
    if sign {
        b[31] |= 0x80;
    }
    b
}

fn h(parts: &[&[u8]]) -> Scalar {
    let mut d = Sha512::new();
    parts.iter().for_each(|p| d.update(p));
    Scalar::from_bytes_mod_order_wide(&d.finalize().into())
}

/// A strictness case: key, message, signature and the rule it breaks.
struct Case {
    name: String,
    key: [u8; 32],
    message: Vec<u8>,
    sig: [u8; 64],
    expect: &'static str,
}

/// The test key's secret scalar a (RFC 8032 5.1.5) and A = [a]B.
fn secret() -> (Scalar, EdwardsPoint) {
    let digest = Sha512::digest(test_seed());
    let mut a: [u8; 32] = digest[..32].try_into().unwrap();
    a[0] &= 248;
    a[31] &= 127;
    a[31] |= 64;
    let a = Scalar::from_bytes_mod_order(a);
    (a, B * a)
}

/// A signature (R's bytes, S) for `key` (bytes as hashed), the secret
/// scalar `a` and the nonce `r`, with `r_bytes` written as R.
fn sign(key: &[u8; 32], a: Scalar, r: Scalar, r_bytes: [u8; 32], message: &[u8]) -> [u8; 64] {
    let s = r + h(&[&r_bytes, key, message]) * a;
    [r_bytes, s.to_bytes()].concat().try_into().unwrap()
}

fn add_l(s: &[u8], times: u8) -> [u8; 32] {
    // L = 2^252 + 27742317777372353535851937790883648493, little-endian.
    let l = hex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in 0..32 {
        let v = s[i] as u16 + l[i] as u16 * times as u16 + carry;
        out[i] = v as u8;
        carry = v >> 8;
    }
    out
}

fn cases() -> Vec<Case> {
    let (a, big_a) = secret();
    let key = big_a.compress().0;
    let t = torsion();
    let mut out = Vec::new();
    let mut add = |name: String, key: [u8; 32], message: Vec<u8>, sig: [u8; 64], expect: &'static str| {
        out.push(Case { name, key, message, sig, expect })
    };
    let msg = |name: &str| format!("strict {name}").into_bytes();
    let nonce = |name: &str| h(&[b"nonce", name.as_bytes()]);
    let honest = |name: &str| {
        let r = nonce(name);
        sign(&key, a, r, (B * r).compress().0, &msg(name))
    };

    add("valid".into(), key, msg("valid"), honest("valid"), "valid");
    let mut wrong = msg("valid");
    wrong.push(b'!');
    add("other-message".into(), key, wrong, honest("valid"), "Signature");

    // S: S + L and S + 2L verify if S is reduced first; L - 1 is canonical.
    for (name, times) in [("s-plus-l", 1), ("s-plus-2l", 2)] {
        let mut sig = honest(name);
        let s = add_l(&sig[32..], times);
        sig[32..].copy_from_slice(&s);
        add(name.into(), key, msg(name), sig, "SignatureS");
    }
    let l = add_l(&[0; 32], 1);
    let mut sig = honest("s-equals-l");
    sig[32..].copy_from_slice(&l);
    add("s-equals-l".into(), key, msg("s-equals-l"), sig, "SignatureS");
    let mut sig = honest("s-top-bit");
    sig[63] |= 0x80;
    add("s-top-bit".into(), key, msg("s-top-bit"), sig, "SignatureS");
    let mut sig = honest("s-l-minus-1");
    sig[32..].copy_from_slice(&(Scalar::ZERO - Scalar::ONE).to_bytes());
    add("s-l-minus-1".into(), key, msg("s-l-minus-1"), sig, "Signature");

    // A of small order, each of the eight: R = [r]B and S = r satisfy
    // [S]B = R + [k]A whenever [k]A is the neutral point.
    let small_keys: Vec<(String, [u8; 32])> = t
        .iter()
        .enumerate()
        .map(|(i, p)| (format!("key-small-order-{i}"), p.compress().0))
        .chain([
            ("key-small-order-y-p".into(), p_plus(0, false)),
            ("key-small-order-y-p-signed".into(), p_plus(0, true)),
            ("key-small-order-y-p-plus-1".into(), p_plus(1, false)),
            ("key-small-order-y-p-plus-1-signed".into(), p_plus(1, true)),
            ("key-small-order-minus-zero-y-1".into(), {
                let mut b = [0; 32];
                b[0] = 1;
                b[31] = 0x80;
                b
            }),
            ("key-small-order-minus-zero-y-p-1".into(), {
                let mut b = p_plus(0, true);
                b[0] = 0xec;
                b
            }),
        ])
        .collect();
    for (name, k) in small_keys {
        let r = nonce(&name);
        add(name.clone(), k, msg(&name), sign(&k, Scalar::ZERO, r, (B * r).compress().0, &msg(&name)), "Key");
    }
    // A not canonical though of large order (y = p + y0), and A off the curve.
    let mut found = 0;
    for y0 in 2..=18u8 {
        let mut canon = [0; 32];
        canon[0] = y0;
        match CompressedEdwardsY(canon).decompress() {
            Some(p) if !p.is_small_order() && found < 2 => {
                found += 1;
                let name = format!("key-not-canonical-y-p-plus-{y0}");
                add(name.clone(), p_plus(y0, false), msg(&name), honest(&name), "Key");
            }
            None => {
                let name = format!("key-not-on-curve-y-{y0}");
                add(name.clone(), canon, msg(&name), honest(&name), "Key");
            }
            _ => {}
        }
    }

    // R of small order with S = [k]a: [S]B - [k]A is the neutral point,
    // so the cofactored equation holds for every one, the cofactorless
    // one for the neutral point only.
    let small_r: Vec<(String, [u8; 32])> = t
        .iter()
        .enumerate()
        .map(|(i, p)| (format!("r-small-order-{i}"), p.compress().0))
        .chain([
            ("r-small-order-y-p-plus-1".into(), p_plus(1, false)),
            ("r-small-order-y-p".into(), p_plus(0, false)),
            ("r-small-order-minus-zero".into(), {
                let mut b = [0; 32];
                b[0] = 1;
                b[31] = 0x80;
                b
            }),
        ])
        .collect();
    for (name, rb) in small_r {
        add(name.clone(), key, msg(&name), sign(&key, a, Scalar::ZERO, rb, &msg(&name)), "SignatureR");
    }
    let mut off = [0; 32];
    off[0] = (2..=18u8).find(|&y| CompressedEdwardsY([[y].as_slice(), &[0; 31]].concat().try_into().unwrap()).decompress().is_none()).unwrap();
    let mut sig = honest("r-not-on-curve");
    sig[..32].copy_from_slice(&off);
    add("r-not-on-curve".into(), key, msg("r-not-on-curve"), sig, "SignatureR");
    // R written y + p, of large order: the canonical encoding computed differs.
    let y0 = (2..=18u8)
        .find(|&y| CompressedEdwardsY([[y].as_slice(), &[0; 31]].concat().try_into().unwrap()).decompress().is_some_and(|p| !p.is_small_order()))
        .unwrap();
    let mut sig = honest("r-not-canonical");
    sig[..32].copy_from_slice(&p_plus(y0, false));
    add("r-not-canonical".into(), key, msg("r-not-canonical"), sig, "Signature");

    // Mixed order R = [r]B + T: only the cofactored equation holds.
    for (i, ti) in t.iter().enumerate().skip(1) {
        let name = format!("r-mixed-order-{i}");
        let r = nonce(&name);
        add(name.clone(), key, msg(&name), sign(&key, a, r, (B * r + ti).compress().0, &msg(&name)), "Signature");
    }

    // Mixed-order A = [a]B + T, a valid key: S = r + [k]a satisfies the
    // cofactorless equation if and only if [k]T is the neutral point.
    for (i, ti) in t.iter().enumerate().skip(1) {
        let mixed = (big_a + ti).compress().0;
        let mut have = [false; 2];
        for n in 0.. {
            let name = format!("key-mixed-order-{i}-{n}");
            let r = nonce(&name);
            let rb = (B * r).compress().0;
            let holds = (ti * h(&[&rb, &mixed, &msg(&name)])).is_identity();
            if have[holds as usize] {
                continue;
            }
            have[holds as usize] = true;
            add(name.clone(), mixed, msg(&name), sign(&mixed, a, r, rb, &msg(&name)), if holds { "valid" } else { "Signature" });
            if have == [true, true] {
                break;
            }
        }
    }
    out
}

fn write_cases(cases: &[Case]) -> String {
    let mut s = String::from(
        "# Ed25519 strictness cases for NETWORK.md 4.2, generated by\n\
         # sdk/rust/tests/sdk.rs (UPDATE_GOLDEN=1 rewrites it). Each gives a\n\
         # key, a message and a signature, and `expect`: valid, or the rule\n\
         # of strict verification broken (Key, SignatureS, SignatureR,\n\
         # Signature). The signatures are made with the PUBLIC TEST key of\n\
         # vectors/entries.txt.\n\n",
    );
    for c in cases {
        s += &format!(
            "case {}\n  key {}\n  message {}\n  signature {}\n  expect {}\n\n",
            c.name,
            to_hex(&c.key),
            to_hex(&c.message),
            to_hex(&c.sig),
            c.expect
        );
    }
    s
}

#[test]
fn strictness_cases() {
    let cases = cases();
    let path = repo().join("sdk/testdata/strict.txt");
    let text = write_cases(&cases);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "sdk/testdata/strict.txt is out of date (UPDATE_GOLDEN=1)");
    let mut valid = 0;
    for c in &cases {
        // The check verify() makes, through an entry-shaped door: the
        // reasons are those of verify_entry.
        let got = verify(&c.key, &c.message, &c.sig);
        assert_eq!(got, c.expect == "valid", "{}", c.name);
        assert_eq!(dalek_strict(&c.key, &c.message, &c.sig), got, "{}: ed25519-dalek verify_strict disagrees", c.name);
        valid += got as usize;
        if c.expect == "Key" {
            assert!(!valid_feed_key(&c.key), "{}", c.name);
        }
    }
    assert_eq!(valid, 1 + 7, "the honest signature and one per mixed-order key");
    assert!(cases.len() > 50, "{} cases", cases.len());
}

/// Each case's key and signature in an entry: verify_entry names the
/// same rule for a key, an S or an R it refuses (they are checked before
/// the message matters), and refuses the rest, signed over another
/// message, as Signature.
#[test]
fn strictness_reasons() {
    for c in cases() {
        let mut bytes = c.key.to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend([1, 1, 0, 0]);
        bytes.extend([7; 32]);
        bytes.extend([0; 32]);
        bytes.push(0);
        bytes.extend(c.sig);
        let got = verify_entry(&bytes).map(|_| ()).map_err(|e| format!("{e:?}"));
        let want = if matches!(c.expect, "valid" | "Signature") { "Signature" } else { c.expect };
        assert_eq!(got, Err(want.to_string()), "{}", c.name);
    }
}

/// The claims the reference article makes about ed25519-dalek: its
/// `verify` accepts small-order keys and R; `verify_strict` refuses them
/// but accepts a non-canonical key (`VerifyingKey::from_bytes` decodes
/// y >= p).
#[test]
fn what_ed25519_dalek_does() {
    let cases = cases();
    let get = |n: &str| cases.iter().find(|c| c.name == n).unwrap();
    let plain = |c: &Case| {
        ed25519_dalek::VerifyingKey::from_bytes(&c.key)
            .is_ok_and(|k| k.verify(&c.message, &ed25519_dalek::Signature::from_bytes(&c.sig)).is_ok())
    };
    let strict = |c: &Case| {
        ed25519_dalek::VerifyingKey::from_bytes(&c.key)
            .is_ok_and(|k| k.verify_strict(&c.message, &ed25519_dalek::Signature::from_bytes(&c.sig)).is_ok())
    };
    assert!(plain(get("key-small-order-0")), "verify: a forgery for the neutral key");
    assert!(plain(get("key-small-order-y-p-plus-1")), "verify: the neutral key written y = p + 1");
    assert!(plain(get("r-small-order-0")), "verify: R the neutral point");
    assert!(!plain(get("s-plus-l")) && !plain(get("r-mixed-order-1")));
    assert!(!strict(get("key-small-order-0")) && !strict(get("r-small-order-0")));
    assert!(ed25519_dalek::VerifyingKey::from_bytes(&p_plus(3, false)).is_ok(), "from_bytes: y = p + 3");
    assert!(!valid_feed_key(&p_plus(3, false)));
}

/// Arbitrary bytes and every vector cut, extended and flipped: no panic,
/// and only the valid vectors accepted.
#[test]
fn no_panics() {
    let mut x = 0x5eed_u64;
    let mut next = || {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    for len in 0..400 {
        let b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let _ = verify_entry(&b);
    }
    for (head, f) in entry_vectors() {
        if !head.starts_with("entry ") {
            continue;
        }
        let bytes = hex(field(&f, "bytes"));
        for cut in 0..=bytes.len() {
            let _ = verify_entry(&bytes[..cut]);
        }
        let valid: Vec<Vec<u8>> = entry_vectors()
            .iter()
            .filter(|(h, f)| h.starts_with("entry ") && field(f, "valid") == "yes")
            .map(|(_, f)| hex(field(f, "bytes")))
            .collect();
        for _ in 0..200 {
            let mut b = bytes.clone();
            if b.is_empty() {
                break;
            }
            let i = next() as usize % b.len();
            b[i] ^= 1 << (next() % 8);
            if next() % 4 == 0 {
                let at = 108.min(b.len() - 1);
                b[at] = next() as u8;
            }
            assert!(verify_entry(&b).is_err() || valid.contains(&b), "accepted a changed entry");
        }
    }
}
