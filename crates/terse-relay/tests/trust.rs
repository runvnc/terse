//! Private authorities and pins (NETWORK 3, "Overrides come only from the
//! user"; README "Private authorities and pins"): relays in this process
//! with certificates of the private test authority (crates/terse-testkit/tls:
//! ca.pem signed localhost.pem; untrusted.pem is self-signed), trusted
//! through the user's settings only: the authorities folder, one host's
//! authority, a pinned key or authority. What this process trusts is one
//! setting, so every case runs in turn in one test.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use terse_net::relay::WsTransport;
use terse_net::{BlobStore, local, tls, trust};
use terse_relay::{DEFAULT_FEED_CAP, Policy, Relay};

fn certs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../terse-testkit/tls")
}

/// A relay serving wss:// with a test certificate, at 127.0.0.1.
fn start(dir: &Path, cert: &str) -> String {
    let relay = Arc::new(Relay::with_policy(BlobStore::open(dir, None).unwrap(), Policy::unlimited(DEFAULT_FEED_CAP)).unwrap());
    let config = tls::server_config(&certs().join(format!("{cert}.pem")), &certs().join(format!("{cert}.key"))).unwrap();
    relay.set_tls(Some(config));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("wss://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

fn connect(url: &str) -> Result<(), String> {
    WsTransport::connect(url, Duration::from_secs(5)).map(drop).map_err(|e| e.to_string())
}

fn pin(cert: &str, key: bool) -> String {
    let c = tls::read_certs(&certs().join(format!("{cert}.pem"))).unwrap().remove(0);
    let (whole, k) = trust::fingerprints(&c);
    format!("pin={}", trust::fingerprint_text(&if key { k.unwrap() } else { whole }))
}

#[test]
fn private_authorities_and_pins() {
    // SAFETY: no other thread of this test binary reads the environment.
    unsafe { std::env::remove_var(tls::CA_ENV) };
    let (rd, sd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let secure = start(rd.path(), "localhost");
    let self_signed = start(sd.path(), "untrusted");
    let other_name = secure.replace("127.0.0.1", "localhost");
    let ca = certs().join("ca.pem");
    let config = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    // The settings of `config` and of `--relay` values, as a program
    // applies them before it connects.
    let apply = |dir: &Path, given: &[String]| local::find_relays(given, Some(dir), 0).map(drop).unwrap();
    let expect = |what: &str, url: &str, want: Option<&str>| match (connect(url), want) {
        (Ok(()), None) => {}
        (Err(e), Some(w)) if e.contains(w) => {}
        (got, _) => panic!("{what}: {url}: want {want:?}, got {got:?}"),
    };

    // Nothing set: the private authority is not trusted.
    apply(empty.path(), &[]);
    expect("no settings", &secure, Some("certificate is not trusted"));

    // The folder: every relay, by any name.
    std::fs::create_dir(config.path().join(trust::AUTHORITIES_DIR)).unwrap();
    std::fs::copy(&ca, config.path().join(trust::AUTHORITIES_DIR).join("org.pem")).unwrap();
    apply(config.path(), &[]);
    expect("folder", &secure, None);
    expect("folder", &other_name, None);
    expect("folder, another authority", &self_signed, Some("certificate is not trusted"));

    // One host's authority, in the relay list: not another name of it.
    std::fs::copy(&ca, empty.path().join("corp-ca.pem")).unwrap();
    std::fs::write(empty.path().join(local::RELAYS_FILE), format!("{secure} authority=corp-ca.pem\n")).unwrap();
    apply(empty.path(), &[]);
    expect("per host", &secure, None);
    expect("per host, other name", &other_name, Some("certificate is not trusted"));
    std::fs::remove_file(empty.path().join(local::RELAYS_FILE)).unwrap();

    // The same as a --relay value.
    apply(empty.path(), &[format!("{secure} authority={}", ca.display())]);
    expect("per host, --relay", &secure, None);
    expect("per host, --relay, other name", &other_name, Some("certificate is not trusted"));

    // A pinned key: that key only, without any authority.
    apply(empty.path(), &[format!("{self_signed} {}", pin("untrusted", true))]);
    expect("pinned key", &self_signed, None);
    apply(empty.path(), &[format!("{secure} {}", pin("localhost", false))]);
    expect("pinned certificate", &secure, None);

    // Pinned to another key: refused, though an authority trusted for
    // every relay (as a public one is) vouches for the certificate.
    apply(config.path(), &[format!("{secure} {}", pin("untrusted", true))]);
    expect("pinned key mismatch", &secure, Some("does not match the key pinned for it"));
    expect("a pin is for its host only", &other_name, None);

    // A pinned authority: that one only.
    apply(empty.path(), &[format!("{self_signed} only-authority={}", ca.display())]);
    expect("pinned authority mismatch", &self_signed, Some("not from the authority pinned for it"));
    apply(config.path(), &[format!("{secure} only-authority={}", certs().join("untrusted.pem").display())]);
    expect("pinned authority, the folder's does not count", &secure, Some("not from the authority pinned for it"));
    apply(empty.path(), &[format!("{secure} only-authority={}", ca.display())]);
    expect("pinned authority", &secure, None);

    // No setting accepts anything: options that would are refused.
    for bad in ["insecure", "accept-any=yes", "pin=", "pin=sha256:00"] {
        assert!(local::find_relays(&[format!("{secure} {bad}")], Some(empty.path()), 0).is_err(), "{bad}");
    }
    // An override on a plain relay is refused: it has no certificate.
    assert!(local::find_relays(&[format!("ws://127.0.0.1:1 authority={}", ca.display())], Some(empty.path()), 0).is_err());
}

/// A hint received from the network carries no override and no downgrade:
/// it cannot name options (it is then malformed), and an encrypted hint
/// to a relay is reached over TLS with what the user trusts, never plain.
#[test]
fn hints_carry_no_override() {
    use terse_net::address::{parse_address, parse_hints};
    for hints in ["relay.example pin=sha256:00", "relay.example;insecure", "ws:relay.example:7878 authority=x"] {
        assert!(parse_hints(hints.as_bytes()).is_err(), "{hints}");
    }
    let key = terse_net::Identity::from_seed([5; 32]).feed_key();
    assert!(parse_address(&format!("{}@relay.example?pin=00", terse_net::blob::to_hex(&key))).is_err());
    // An encrypted hint names wss://; trust settings for its host are the
    // user's (none here), so a host the user pinned stays pinned.
    let h = parse_hints(b"127.0.0.2:9000,relay.example:9000").unwrap();
    assert_eq!(h[1].url(), "wss://relay.example:9000");
    assert!(h[1].encrypted());
}
