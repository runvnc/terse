//! What a client trusts for `wss://` relays besides the public certificate
//! authorities (NETWORK 3, "Overrides come only from the user"): the
//! reference's settings, from the user's own config folder and command
//! line only (README, "Private authorities and pins").
//!
//! - every `*.pem` file in `<config>/authorities/` is an authority trusted
//!   for every relay, besides the public ones;
//! - a line of the relay list (`<config>/relays`), or a `--relay` value, is
//!   a relay and, after blanks, options for its host:
//!   `authority=<file>` trusts that authority for this host too;
//!   `only-authority=<file>` trusts only that authority for this host;
//!   `pin=sha256:<64 hex>` accepts for this host only the certificate, or
//!   the public key, with that SHA-256 fingerprint. With `only-authority`
//!   or `pin`, nothing else is accepted for the host, a public authority's
//!   certificate included. A file is relative to the config folder;
//! - `TERSE_RELAY_CA` names a PEM file of authorities for every relay, and
//!   `--relay-authority <file>` does the same on a command line.
//!
//! Nothing here turns verification off, and an option applies only to the
//! host it is written with. Nothing received from the network (a hint, a
//! profile's relay line) is read here.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use rustls::pki_types::CertificateDer;
use sha2::{Digest, Sha256};

use crate::tls::read_certs;

/// The folder of the config folder whose PEM files are trusted for every
/// relay.
pub const AUTHORITIES_DIR: &str = "authorities";
/// The environment variable naming a PEM file of authorities trusted for
/// every relay.
pub const CA_ENV: &str = "TERSE_RELAY_CA";

/// A SHA-256 fingerprint, of a certificate or of its public key (the
/// DER of its SubjectPublicKeyInfo).
pub type Fingerprint = [u8; 32];

/// What is trusted for one host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostTrust {
    /// `authority=`: trusted for this host besides the others.
    pub also: Vec<CertificateDer<'static>>,
    /// `only-authority=`: the only authorities accepted for this host.
    pub only: Vec<CertificateDer<'static>>,
    /// `pin=`: the only certificates or public keys accepted for this host
    /// (besides those of `only`).
    pub pins: Vec<Fingerprint>,
}

impl HostTrust {
    /// Whether nothing but the pinned authorities and keys is accepted.
    pub fn pinned(&self) -> bool {
        !self.only.is_empty() || !self.pins.is_empty()
    }

    fn merge(&mut self, other: HostTrust) {
        self.also.extend(other.also);
        self.only.extend(other.only);
        self.pins.extend(other.pins);
    }
}

/// Everything trusted besides the public authorities.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trust {
    /// Authorities for every relay.
    pub all: Vec<CertificateDer<'static>>,
    /// By host ([`host_key`]).
    pub hosts: BTreeMap<String, HostTrust>,
}

/// A host as the trust settings key it: lowercase, an IPv6 address
/// without brackets in its shortest form.
pub fn host_key(host: &str) -> String {
    let inner = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    match inner.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => inner.to_ascii_lowercase(),
    }
}

/// The SHA-256 fingerprints of a certificate: of the certificate itself,
/// and of its public key (`None` if the certificate cannot be read).
pub fn fingerprints(cert: &CertificateDer<'_>) -> (Fingerprint, Option<Fingerprint>) {
    let whole: Fingerprint = Sha256::digest(cert.as_ref()).into();
    let key = webpki::EndEntityCert::try_from(cert).ok().map(|c| Sha256::digest(c.subject_public_key_info().as_ref()).into());
    (whole, key)
}

/// `sha256:` and 64 lowercase hex digits.
pub fn fingerprint_text(f: &Fingerprint) -> String {
    format!("sha256:{}", crate::blob::to_hex(f))
}

fn parse_fingerprint(text: &str) -> Option<Fingerprint> {
    let hex = text.strip_prefix("sha256:")?;
    let hex: String = hex.chars().filter(|c| *c != ':').collect::<String>().to_ascii_lowercase();
    if hex.len() != 64 {
        return None;
    }
    crate::blob::parse_hex(&hex)
}

/// A relay line: the relay as typed, and its options.
#[derive(Debug, PartialEq)]
pub struct RelayLine<'a> {
    pub relay: &'a str,
    pub options: Vec<&'a str>,
}

/// Splits a line of the relay list (or a `--relay` value) at blanks: the
/// relay, then its options. `None` for an empty line or a `#` comment.
pub fn split_line(line: &str) -> Option<RelayLine<'_>> {
    let mut words = line.split_whitespace();
    let relay = words.next().filter(|w| !w.starts_with('#'))?;
    Some(RelayLine { relay, options: words.collect() })
}

/// Why a setting cannot be taken.
#[derive(Debug, PartialEq, Eq)]
pub struct Bad(pub String);

impl fmt::Display for Bad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, Bad> {
    read_certs(path).map_err(|e| Bad(e.to_string()))
}

/// The trust of one relay line's options for the relay `url` (already a
/// URL, `wss://` for any option), files relative to `base`.
pub fn line_options(url: &str, options: &[&str], base: Option<&Path>) -> Result<Option<(String, HostTrust)>, Bad> {
    if options.is_empty() {
        return Ok(None);
    }
    if !url.starts_with("wss://") {
        return Err(Bad(format!("{url}: authority= and pin= apply only to wss:// (encrypted) relays")));
    }
    let end = crate::relay::Endpoint::parse(url).map_err(|e| Bad(e.to_string()))?;
    let file = |name: &str| -> PathBuf {
        let p = PathBuf::from(name);
        match base {
            Some(b) if p.is_relative() => b.join(p),
            _ => p,
        }
    };
    let mut t = HostTrust::default();
    for opt in options {
        let (name, value) = opt.split_once('=').ok_or(Bad(format!("{url}: {opt}: not an option (authority=, only-authority= or pin=)")))?;
        if value.is_empty() {
            return Err(Bad(format!("{url}: {name}= without a value")));
        }
        match name {
            "authority" => t.also.extend(certs(&file(value))?),
            "only-authority" => t.only.extend(certs(&file(value))?),
            "pin" => t.pins.push(
                parse_fingerprint(value)
                    .ok_or(Bad(format!("{url}: pin={value}: not a fingerprint (sha256: and 64 hex digits; `terse relay fingerprint <cert.pem>` prints it)")))?,
            ),
            _ => return Err(Bad(format!("{url}: {opt}: not an option (authority=, only-authority= or pin=)"))),
        }
    }
    Ok(Some((host_key(&end.host), t)))
}

impl Trust {
    /// Adds the authorities of a PEM file for every relay.
    pub fn add_all(&mut self, path: &Path) -> Result<(), Bad> {
        self.all.extend(certs(path)?);
        Ok(())
    }

    /// Adds what a relay line's options say for its host.
    pub fn add_host(&mut self, host: String, t: HostTrust) {
        self.hosts.entry(host).or_default().merge(t);
    }

    /// What applies to `host` (as a URL writes it), if anything was set
    /// for it.
    pub fn host(&self, host: &str) -> Option<&HostTrust> {
        self.hosts.get(&host_key(host))
    }

    /// The authorities of `TERSE_RELAY_CA` (all relays).
    pub fn from_env() -> Result<Trust, Bad> {
        let mut t = Trust::default();
        if let Some(path) = std::env::var_os(CA_ENV).filter(|p| !p.is_empty()) {
            t.add_all(Path::new(&path)).map_err(|e| Bad(format!("{CA_ENV}: {e}")))?;
        }
        Ok(t)
    }

    /// Everything the user's settings say: `TERSE_RELAY_CA`, the PEM files
    /// of `<config>/authorities/` (in name order), the options of the
    /// relay list's lines, then those of `given` (`--relay` values) and
    /// the files of `authorities` (`--relay-authority`).
    pub fn load(config: Option<&Path>, given: &[String], authorities: &[PathBuf]) -> Result<Trust, Bad> {
        let mut t = Trust::from_env()?;
        if let Some(config) = config {
            let dir = config.join(AUTHORITIES_DIR);
            match std::fs::read_dir(&dir) {
                Ok(entries) => {
                    let mut pems: Vec<PathBuf> = entries
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().is_some_and(|x| x == "pem"))
                        .collect();
                    pems.sort();
                    for p in pems {
                        t.add_all(&p)?;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Bad(format!("{}: {e}", dir.display()))),
            }
            let path = config.join(crate::local::RELAYS_FILE);
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    for (i, line) in text.lines().enumerate() {
                        let Some(l) = split_line(line) else { continue };
                        let at = |e: Bad| Bad(format!("{}:{}: {e}", path.display(), i + 1));
                        let url = crate::local::typed_url(l.relay).ok_or_else(|| at(Bad(format!("not a relay: {}", l.relay))))?;
                        if let Some((host, h)) = line_options(&url, &l.options, Some(config)).map_err(at)? {
                            t.add_host(host, h);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Bad(format!("{}: {e}", path.display()))),
            }
        }
        for g in given {
            let Some(l) = split_line(g) else { continue };
            let url = crate::local::typed_url(l.relay).ok_or_else(|| Bad(format!("--relay {}: not a relay", l.relay)))?;
            if let Some((host, h)) = line_options(&url, &l.options, None).map_err(|e| Bad(format!("--relay: {e}")))? {
                t.add_host(host, h);
            }
        }
        for a in authorities {
            t.add_all(a).map_err(|e| Bad(format!("--relay-authority: {e}")))?;
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn certs_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../terse-testkit/tls")
    }

    #[test]
    fn lines() {
        assert_eq!(split_line("  # a comment"), None);
        assert_eq!(split_line(""), None);
        assert_eq!(split_line(" wss://a.example  pin=x\tauthority=y "), Some(RelayLine { relay: "wss://a.example", options: vec!["pin=x", "authority=y"] }));
        assert_eq!(host_key("[2001:DB8::1]"), "2001:db8::1");
        assert_eq!(host_key("Relay.Example"), "relay.example");
    }

    #[test]
    fn options() {
        let dir = certs_dir();
        let pin = format!("pin=sha256:{}", "ab".repeat(32));
        let (host, t) = line_options("wss://Relay.Example:9000", &["authority=ca.pem", &pin], Some(&dir)).unwrap().unwrap();
        assert_eq!(host, "relay.example");
        assert_eq!((t.also.len(), t.only.len(), t.pins.clone()), (1, 0, vec![[0xab; 32]]));
        assert!(t.pinned());
        let (_, t) = line_options("wss://a.example", &["authority=ca.pem"], Some(&dir)).unwrap().unwrap();
        assert!(!t.pinned());
        let (_, t) = line_options("wss://a.example", &["only-authority=ca.pem"], Some(&dir)).unwrap().unwrap();
        assert!(t.pinned());
        // Colons between the bytes and capitals are taken too.
        let colons = format!("pin=sha256:{}", vec!["AB"; 32].join(":"));
        assert_eq!(line_options("wss://a.example", &[&colons], None).unwrap().unwrap().1.pins, [[0xab; 32]]);
        assert_eq!(line_options("wss://a.example", &[], None).unwrap(), None);
        for (url, opt) in [
            // No override for a plain relay: it has no certificate.
            ("ws://a.example:7878", "authority=ca.pem"),
            ("wss://a.example", "pin=sha256:abcd"),
            ("wss://a.example", "pin=md5:00"),
            ("wss://a.example", "trust-anything=yes"),
            ("wss://a.example", "insecure"),
            ("wss://a.example", "authority="),
            ("wss://a.example", "authority=missing.pem"),
            ("wss://a.example", "authority=localhost.key"),
        ] {
            assert!(line_options(url, &[opt], Some(&dir)).is_err(), "{url} {opt}");
        }
    }

    #[test]
    fn fingerprints_of_a_certificate() {
        let cert = read_certs(&certs_dir().join("localhost.pem")).unwrap().remove(0);
        let (whole, key) = fingerprints(&cert);
        assert_ne!(Some(whole), key);
        assert!(key.is_some());
        assert_eq!(parse_fingerprint(&fingerprint_text(&whole)), Some(whole));
    }

    #[test]
    fn load_from_the_config_folder() {
        let config = tempfile::tempdir().unwrap();
        let ca = std::fs::read(certs_dir().join("ca.pem")).unwrap();
        std::fs::create_dir(config.path().join(AUTHORITIES_DIR)).unwrap();
        std::fs::write(config.path().join(AUTHORITIES_DIR).join("org.pem"), &ca).unwrap();
        // Not a .pem: not read.
        std::fs::write(config.path().join(AUTHORITIES_DIR).join("notes.txt"), "x").unwrap();
        std::fs::write(config.path().join("corp-ca.pem"), &ca).unwrap();
        let pin = format!("sha256:{}", "01".repeat(32));
        std::fs::write(
            config.path().join(crate::local::RELAYS_FILE),
            format!("# mine\nwss://a.example only-authority=corp-ca.pem\nb.example pin={pin}\n127.0.0.1:7878\n"),
        )
        .unwrap();
        let given = vec![format!("wss://c.example authority={}", config.path().join("corp-ca.pem").display()), "d.example".to_string()];
        let t = Trust::load(Some(config.path()), &given, &[]).unwrap();
        assert_eq!(t.all.len(), 1);
        assert_eq!(t.hosts.len(), 3);
        assert!(t.host("A.example").unwrap().pinned());
        assert_eq!(t.host("b.example").unwrap().pins, [[1; 32]]);
        assert!(!t.host("c.example").unwrap().pinned());
        assert!(t.host("d.example").is_none());
        // A bad line names its file and line.
        std::fs::write(config.path().join(crate::local::RELAYS_FILE), "wss://a.example pin=nope\n").unwrap();
        let e = Trust::load(Some(config.path()), &[], &[]).unwrap_err();
        assert!(e.0.contains("relays:1:") && e.0.contains("not a fingerprint"), "{e}");
    }
}
