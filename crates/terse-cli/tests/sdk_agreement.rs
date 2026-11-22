//! Every implementation of the entry rules (NETWORK.md 4.2) gives the same
//! verdict on the same entries: the reference (`terse_net::entry::decode`),
//! the Rust SDK (`tersenet`, sdk/rust), the reference's check before it
//! used the SDK (a canonical-key check and ed25519-dalek's
//! `verify_strict`), the Go SDK (sdk/go) and tersereader, the Go reader
//! written from the specifications alone (native/reader-go).
//!
//! The entries: a few thousand made from the valid entries of
//! vectors/entries.txt by a fixed sequence of changes (seed below): bits
//! flipped, fields set to the edges of their ranges and signed again,
//! titles that are and are not display names, lengths that do not match,
//! keys and R of small order or not canonical with signatures that satisfy
//! the equation where one can be made, S + L, R with a component of order
//! 8, and keys with a component of small order signed by their owner (an
//! eighth of them valid). The Go programs run as `go test -run TestCorpus`
//! with the corpus in a scratch file; without Go, that part skips.

use std::process::Command;

use curve25519_dalek::constants::ED25519_BASEPOINT_POINT as B;
use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::IsIdentity;
use sha2::{Digest, Sha512};
use terse_testkit::{skip, workspace};

const COUNT: usize = 4000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn h(parts: &[&[u8]]) -> Scalar {
    let mut d = Sha512::new();
    parts.iter().for_each(|p| d.update(p));
    Scalar::from_bytes_mod_order_wide(&d.finalize().into())
}

/// The valid entries of vectors/entries.txt and the test key's seed.
fn vectors() -> (Vec<Vec<u8>>, [u8; 32]) {
    let text = std::fs::read_to_string(workspace().join("vectors/entries.txt")).unwrap();
    let mut valid = Vec::new();
    let mut seed = None;
    for record in text.split("\n\n") {
        let field = |name: &str| record.lines().find_map(|l| l.strip_prefix(&format!("  {name} ")));
        if record.starts_with("key test") {
            seed = Some(hex(field("secret-seed").unwrap()).try_into().unwrap());
        }
        if record.starts_with("entry ") && field("valid") == Some("yes") {
            valid.push(hex(field("bytes").unwrap()));
        }
    }
    (valid, seed.unwrap())
}

/// The eight points of small order.
fn torsion() -> Vec<EdwardsPoint> {
    let inv8 = Scalar::from(8u8).invert();
    let t = (2u8..)
        .filter_map(|y| CompressedEdwardsY([[y].as_slice(), &[0; 31]].concat().try_into().unwrap()).decompress())
        .map(|p| p - p.mul_by_cofactor() * inv8)
        .find(|t| !(t + t + t + t).is_identity())
        .unwrap();
    (0..8u8).map(|i| t * Scalar::from(i)).collect()
}

struct Maker {
    a: Scalar,
    key: [u8; 32],
    torsion: Vec<EdwardsPoint>,
    nonce: u64,
}

impl Maker {
    /// Signs `entry` (its last 64 bytes replaced) as the owner of `key`
    /// with secret scalar `a` would, writing `r_extra` + [r]B as R.
    fn sign_as(&mut self, entry: &mut [u8], a: Scalar, r_extra: EdwardsPoint) {
        self.nonce += 1;
        let r = h(&[b"agreement nonce", &self.nonce.to_le_bytes()]);
        let r_bytes = (B * r + r_extra).compress().0;
        let end = entry.len() - 64;
        let k = h(&[&r_bytes, &entry[..32], b"terse-entry-0", &entry[..end]]);
        entry[end..end + 32].copy_from_slice(&r_bytes);
        entry[end + 32..].copy_from_slice(&(r + k * a).to_bytes());
    }

    fn sign(&mut self, entry: &mut [u8]) {
        let (a, zero) = (self.a, self.torsion[0]);
        self.sign_as(entry, a, zero);
    }

    /// A small-order or non-canonical key encoding.
    fn odd_key(&self, rng: &mut Rng) -> [u8; 32] {
        let mut p_plus = |add: u8| {
            let mut b = [0xff; 32];
            b[31] = 0x7f | ((rng.next() as u8) & 0x80);
            b[0] = 0xed + add;
            b
        };
        match rng.below(4) {
            0 | 1 => self.torsion[rng.below(8)].compress().0,
            2 => p_plus(rng.below(19) as u8),
            _ => {
                let mut b = self.torsion[rng.below(8)].compress().0;
                b[31] |= 0x80;
                b
            }
        }
    }
}

fn entry_with_title(base: &[u8], title: &[u8]) -> Vec<u8> {
    let mut e = base[..108].to_vec();
    e.push(title.len() as u8);
    e.extend_from_slice(title);
    e.extend([0; 64]);
    e
}

fn corpus() -> Vec<Vec<u8>> {
    let (valid, seed) = vectors();
    let digest = Sha512::digest(seed);
    let mut clamped: [u8; 32] = digest[..32].try_into().unwrap();
    clamped[0] &= 248;
    clamped[31] &= 127;
    clamped[31] |= 64;
    let a = Scalar::from_bytes_mod_order(clamped);
    let mut m = Maker { a, key: (B * a).compress().0, torsion: torsion(), nonce: 0 };
    assert_eq!(m.key[..], valid[0][..32], "the vectors' key");
    let titles: Vec<Vec<u8>> = [
        "", "Ana", " Ana", "Ana ", "\u{a0}x", "x\u{3000}", "x\u{85}", "a\u{7}b", "\u{2028}", "名前", "\u{200b}x",
        "x\u{180e}", "\u{feff}x", "x\u{9f}", "x\u{a0}y", "\u{1680}x", "x\u{205f}", "e\u{301}",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .chain([
        vec![0xff],
        vec![0xc0, 0x80],
        vec![0xed, 0xa0, 0x80],
        vec![0xf4, 0x90, 0x80, 0x80],
        vec![b'x'; 48],
        vec![b'x'; 49],
        "é".repeat(24).into_bytes(),
        "é".repeat(25).into_bytes(),
        vec![b'x'; 120],
    ])
    .collect();
    let s_l = hex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");
    let mut rng = Rng(0x7e25_e5d4_a64e_e3e1);
    let mut out = Vec::new();
    while out.len() < COUNT {
        let mut e = valid[rng.below(valid.len())].clone();
        match rng.below(10) {
            0 => {
                let i = rng.below(e.len());
                e[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = 32 + rng.below(e.len() - 96);
                e[i] ^= 1 << rng.below(8);
                if i != 108 {
                    m.sign(&mut e);
                }
            }
            2 => {
                match rng.below(4) {
                    0 => e[32..36].copy_from_slice(&[0, 1, u32::MAX][rng.below(3)].to_le_bytes()),
                    1 => e[40] = [0, 1, 2, 3, 4, 5, 9, 254, 255][rng.below(9)],
                    2 => {
                        let size: u32 = [0, 1, 4096, 4097, 1 << 20, (1 << 20) + 1, 0xff_ffff][rng.below(7)];
                        e[41..44].copy_from_slice(&size.to_le_bytes()[..3]);
                    }
                    _ => e[36..40].copy_from_slice(&(rng.next() as u32).to_le_bytes()),
                }
                m.sign(&mut e);
            }
            3 => {
                e = entry_with_title(&e, &titles[rng.below(titles.len())]);
                e[40] = [1, 2, 3, 4, 9][rng.below(5)];
                if e[40] == 2 {
                    e[41..44].copy_from_slice(&[0x10, 0, 0]);
                }
                m.sign(&mut e);
            }
            4 => {
                if rng.below(2) == 0 {
                    e[108] = rng.below(131) as u8;
                } else {
                    let len = 1 + rng.below(300);
                    e.resize(len, rng.next() as u8);
                }
            }
            5 => {
                // A key no one owns or that anyone can sign for: R = [r]B,
                // S = r satisfies the equation if [k]A is neutral.
                let key = m.odd_key(&mut rng);
                e[..32].copy_from_slice(&key);
                let zero = m.torsion[0];
                m.sign_as(&mut e, Scalar::ZERO, zero);
            }
            6 => {
                // R of small order (with S = [k]a: the cofactored equation
                // holds) or with a small-order component.
                let t = m.torsion[1 + rng.below(7)];
                if rng.below(2) == 0 {
                    let end = e.len() - 64;
                    let k = h(&[&t.compress().0, &e[..32], b"terse-entry-0", &e[..end]]);
                    e[end..end + 32].copy_from_slice(&t.compress().0);
                    e[end + 32..].copy_from_slice(&(k * m.a).to_bytes());
                } else {
                    let a = m.a;
                    m.sign_as(&mut e, a, t);
                }
            }
            7 => {
                let end = e.len() - 32;
                if rng.below(2) == 0 {
                    let mut carry = 0u16;
                    for i in 0..32 {
                        let v = e[end + i] as u16 + s_l[i] as u16 + carry;
                        e[end + i] = v as u8;
                        carry = v >> 8;
                    }
                } else {
                    e[end + 31] |= 0x80 >> rng.below(3);
                }
            }
            8 => {
                // A key with a small-order component, signed by its owner.
                let t = m.torsion[1 + rng.below(7)];
                let key = (B * m.a + t).compress().0;
                e[..32].copy_from_slice(&key);
                let (a, zero) = (m.a, m.torsion[0]);
                m.sign_as(&mut e, a, zero);
            }
            _ => {
                if rng.below(2) == 0 {
                    e = (0..1 + rng.below(300)).map(|_| rng.next() as u8).collect();
                }
            }
        }
        out.push(e);
    }
    out
}

fn rule(r: Result<tersenet::Entry<'_>, tersenet::Invalid>) -> String {
    match r {
        Ok(_) => "valid".into(),
        Err(e) => format!("{e:?}"),
    }
}

fn old_reference(e: &[u8]) -> bool {
    use ed25519_dalek::{Signature, VerifyingKey};
    use tersenet::Invalid::{Key, Signature as Sig, SignatureR, SignatureS};
    // The rules before the signature are the SDK's: the reference made
    // them the same way before it used the SDK.
    if let Err(r) = tersenet::verify_entry(e) {
        if !matches!(r, Key | SignatureS | SignatureR | Sig) {
            return false;
        }
    }
    let key: [u8; 32] = e[..32].try_into().unwrap();
    let end = e.len() - 64;
    let canonical =
        CompressedEdwardsY(key).decompress().is_some_and(|p| p.compress().0 == key && !p.is_small_order());
    let sig = Signature::from_bytes(e[end..].try_into().unwrap());
    let message = [b"terse-entry-0".as_slice(), &e[..end]].concat();
    canonical && VerifyingKey::from_bytes(&key).is_ok_and(|k| k.verify_strict(&message, &sig).is_ok())
}

/// The verdicts of a Go package's TestCorpus, or None (skipped) without Go.
fn go_verdicts(dir: &str, package: &str, corpus: &std::path::Path) -> Option<Vec<String>> {
    if !Command::new("go").arg("version").output().is_ok_and(|o| o.status.success()) {
        skip!("go", "needs Go (`go` not found) to run {dir}");
        return None;
    }
    let out = corpus.with_extension(format!("{}.verdicts", dir.replace('/', "-")));
    let run = Command::new("go")
        .args(["test", "-count=1", "-run", "^TestCorpus$", package])
        .current_dir(workspace().join(dir))
        .env("GOFLAGS", "-mod=readonly")
        .env("CGO_ENABLED", "0")
        .env("TERSENET_CORPUS", corpus)
        .env("TERSENET_VERDICTS", &out)
        .output()
        .unwrap();
    if !run.status.success() {
        let why = String::from_utf8_lossy(&run.stdout) + String::from_utf8_lossy(&run.stderr);
        if why.contains("missing go.sum entry") || why.contains("dial tcp") || why.contains("module lookup disabled") {
            skip!("go-modules", "cannot get the Go modules of {dir} (offline?): {}", why.trim());
            return None;
        }
        panic!("go test in {dir}: {why}");
    }
    Some(std::fs::read_to_string(out).unwrap().lines().map(str::to_string).collect())
}

#[test]
fn every_implementation_agrees() {
    let corpus = corpus();
    let sdk: Vec<String> = corpus.iter().map(|e| rule(tersenet::verify_entry(e))).collect();
    let mut counts = std::collections::BTreeMap::new();
    for v in &sdk {
        *counts.entry(v.as_str()).or_insert(0) += 1;
    }
    eprintln!("verdicts of {} entries: {counts:?}", corpus.len());
    let enough = [("valid", 300), ("Key", 100), ("SignatureR", 100), ("SignatureS", 100), ("Signature", 300)];
    for (rule, at_least) in enough {
        assert!(counts.get(rule).copied().unwrap_or(0) >= at_least, "too few {rule}: {counts:?}");
    }
    for rule in ["Length", "TitleLength", "TitleUtf8", "Seq", "Kind", "Size", "Name"] {
        assert!(counts.contains_key(rule), "no {rule}: {counts:?}");
    }
    let test_key = vectors().0[0][..32].to_vec();
    let mixed_valid = corpus.iter().zip(&sdk).filter(|(e, v)| *v == "valid" && e[..32] != test_key[..]).count();
    eprintln!("valid entries under keys with a small-order component: {mixed_valid}");
    assert!(mixed_valid >= 10, "{mixed_valid} valid entries under keys with a small-order component");

    let mut disagreements = Vec::new();
    for (i, e) in corpus.iter().enumerate() {
        let reference = terse_net::entry::decode(e);
        if reference.is_ok() != (sdk[i] == "valid") {
            disagreements.push(format!("{i}: reference {:?}, SDK {}: {}", reference.err(), sdk[i], to_hex(e)));
        }
        if old_reference(e) != (sdk[i] == "valid") {
            let old = !(sdk[i] == "valid");
            disagreements.push(format!("{i}: verify_strict gives {old}, SDK {}: {}", sdk[i], to_hex(e)));
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("corpus.hex");
    std::fs::write(&file, corpus.iter().map(|e| to_hex(e) + "\n").collect::<String>()).unwrap();
    if let Some(go) = go_verdicts("sdk/go", ".", &file) {
        assert_eq!(go.len(), corpus.len());
        for (i, v) in go.iter().enumerate() {
            if *v != sdk[i] {
                disagreements.push(format!("{i}: Go SDK {v}, Rust SDK {}: {}", sdk[i], to_hex(&corpus[i])));
            }
        }
    }
    if let Some(reader) = go_verdicts("native/reader-go", "./internal/terse", &file) {
        assert_eq!(reader.len(), corpus.len());
        for (i, v) in reader.iter().enumerate() {
            if (v == "valid") != (sdk[i] == "valid") {
                disagreements.push(format!("{i}: tersereader {v}, SDK {}: {}", sdk[i], to_hex(&corpus[i])));
            }
        }
    }
    assert!(disagreements.is_empty(), "{} disagreements:\n{}", disagreements.len(), disagreements.join("\n"));
}
