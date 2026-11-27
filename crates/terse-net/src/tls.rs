//! TLS for relay connections (`wss://`, NETWORK 3), on both sides: a
//! client of a `wss://` relay, and a relay given a certificate
//! (`terse-relay --tls-cert`).
//!
//! The stack is rustls with the ring crypto provider. The roots a client
//! trusts are compiled in (`webpki-roots`, Mozilla's list), so every
//! system, a static Linux binary on a bare server included, trusts the same
//! certificates; a release refreshes them (RELEASING.md). The user's own
//! settings add authorities, for every relay or for one host, or pin a
//! host to an authority or a key ([`crate::trust`], set with
//! [`set_trust`]; [`CA_ENV`] and [`trust_ca_file`] add an authority for
//! every relay). Certificates are always verified, the relay's name
//! included (a pinned key stands for both); there is no way to turn that
//! off.
//!
//! A [`Stream`] is what a relay WebSocket runs over: plain TCP or TLS over
//! TCP. Read and write timeouts are those of the TCP stream, so the read
//! loops that poll with a short timeout behave the same over TLS.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
pub use rustls::ServerConfig;
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, OtherError, RootCertStore, ServerConnection, SignatureScheme,
    StreamOwned,
};

use crate::trust::{Fingerprint, HostTrust, Trust};

/// The environment variable naming a PEM file of CA certificates that a
/// client trusts for `wss://` relays besides the built-in roots: for a
/// self-hosted relay whose certificate comes from a private CA (and for
/// the tests).
pub const CA_ENV: &str = crate::trust::CA_ENV;

/// The first byte of every TLS record a server sends first (a handshake
/// record), and of an alert.
const TLS_HANDSHAKE: u8 = 0x16;
const TLS_ALERT: u8 = 0x15;

/// A relay connection's bytes: plain TCP, or TLS over it.
pub enum Stream {
    /// `sniff`: during a client's handshake, the first byte received is
    /// noted, so that a TLS answer to a plain request can be told
    /// ([`take_sniffed`]).
    Plain { tcp: TcpStream, sniff: bool },
    Client(Box<StreamOwned<ClientConnection, TcpStream>>),
    Server(Box<StreamOwned<ServerConnection, TcpStream>>),
}

thread_local! {
    static SNIFFED: std::cell::Cell<Option<u8>> = const { std::cell::Cell::new(None) };
}

/// The first byte a sniffing stream read on this thread, once.
pub fn take_sniffed() -> Option<u8> {
    SNIFFED.with(|s| s.take())
}

impl Stream {
    pub fn plain(tcp: TcpStream) -> Self {
        Stream::Plain { tcp, sniff: false }
    }

    /// The TCP stream underneath (for its timeouts and blocking mode).
    pub fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain { tcp, .. } => tcp,
            Stream::Client(s) => s.get_ref(),
            Stream::Server(s) => s.get_ref(),
        }
    }

    pub fn is_tls(&self) -> bool {
        !matches!(self, Stream::Plain { .. })
    }

    /// Stops noting the first byte (after a handshake).
    pub fn stop_sniffing(&mut self) {
        if let Stream::Plain { sniff, .. } = self {
            *sniff = false;
        }
    }
}

/// A TLS stream that ended without a close_notify reads as the end of the
/// stream, as plain TCP does: the WebSocket's own closing handshake says
/// whether the connection was closed properly.
fn eof_as_end(r: io::Result<usize>) -> io::Result<usize> {
    match r {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
        r => r,
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain { tcp, sniff } => {
                let n = tcp.read(buf)?;
                if *sniff && n > 0 {
                    SNIFFED.with(|s| s.set(Some(buf[0])));
                    *sniff = false;
                }
                Ok(n)
            }
            Stream::Client(s) => eof_as_end(s.read(buf)),
            Stream::Server(s) => eof_as_end(s.read(buf)),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain { tcp, .. } => tcp.write(buf),
            Stream::Client(s) => s.write(buf),
            Stream::Server(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain { tcp, .. } => tcp.flush(),
            Stream::Client(s) => s.flush(),
            Stream::Server(s) => s.flush(),
        }
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn invalid(what: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_string())
}

/// The certificates in a PEM file.
pub fn read_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let bytes = std::fs::read(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    let certs = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| invalid(format!("{}: not a PEM certificate file: {e}", path.display())))?;
    if certs.is_empty() {
        return Err(invalid(format!("{}: no certificate in it (PEM, BEGIN CERTIFICATE)", path.display())));
    }
    Ok(certs)
}

/// What this process trusts ([`set_trust`]; [`CA_ENV`] alone until then),
/// and the client configurations made from it: for every host without
/// settings of its own (`""`), and per host that has some.
struct Held {
    trust: Arc<Trust>,
    configs: HashMap<String, Arc<ClientConfig>>,
}

static HELD: Mutex<Option<Held>> = Mutex::new(None);

fn roots(certs: impl IntoIterator<Item = CertificateDer<'static>>, public: bool) -> io::Result<RootCertStore> {
    let mut roots = RootCertStore { roots: if public { webpki_roots::TLS_SERVER_ROOTS.to_vec() } else { Vec::new() } };
    for cert in certs {
        roots.add(cert).map_err(|e| invalid(format!("a trusted authority's certificate: {e}")))?;
    }
    Ok(roots)
}

fn with_roots(roots: RootCertStore) -> io::Result<Arc<ClientConfig>> {
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Why a pinned host's certificate was refused.
#[derive(Debug)]
pub enum PinRefused {
    /// It is not the pinned certificate or key (`pin=`).
    Key,
    /// It is not from the pinned authority (`only-authority=`).
    Authority,
    /// Neither (both were given).
    KeyOrAuthority,
}

impl std::fmt::Display for PinRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PinRefused::Key => "the relay's certificate does not match the key pinned for it (pin= in your relay list): it is not trusted",
            PinRefused::Authority => {
                "the relay's certificate is not from the authority pinned for it (only-authority= in your relay list): it is not trusted"
            }
            PinRefused::KeyOrAuthority => {
                "the relay's certificate matches neither the key nor the authority pinned for it (your relay list): it is not trusted"
            }
        })
    }
}

impl std::error::Error for PinRefused {}

fn pin_refused(why: PinRefused) -> rustls::Error {
    rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(why))))
}

/// A pinned host's verifier: the certificate or its public key is one of
/// `pins`, or it chains to one of the pinned authorities and is for the
/// host. Nothing else is accepted, a public authority's certificate
/// included. A matching pin stands for the chain and the name.
#[derive(Debug)]
struct Pinned {
    pins: Vec<Fingerprint>,
    authority: Option<Arc<WebPkiServerVerifier>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if !self.pins.is_empty() {
            let (whole, key) = crate::trust::fingerprints(end_entity);
            if self.pins.iter().any(|p| *p == whole || Some(*p) == key) {
                return Ok(ServerCertVerified::assertion());
            }
        }
        match &self.authority {
            None => Err(pin_refused(PinRefused::Key)),
            Some(v) => v.verify_server_cert(end_entity, intermediates, server_name, ocsp, now).map_err(|e| match e {
                rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
                    pin_refused(if self.pins.is_empty() { PinRefused::Authority } else { PinRefused::KeyOrAuthority })
                }
                e => e,
            }),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// The configuration for a host with settings of its own: pinned, only
/// what it pins; else the public authorities, those for every relay and
/// its own.
fn host_config(trust: &Trust, host: &HostTrust) -> io::Result<Arc<ClientConfig>> {
    if !host.pinned() {
        return with_roots(roots(trust.all.iter().chain(&host.also).cloned(), true)?);
    }
    let provider = provider();
    let authority = if host.only.is_empty() {
        None
    } else {
        let roots = Arc::new(roots(host.only.iter().cloned(), false)?);
        Some(WebPkiServerVerifier::builder_with_provider(roots, provider.clone()).build().map_err(io::Error::other)?)
    };
    let verifier = Pinned { pins: host.pins.clone(), authority, algorithms: provider.signature_verification_algorithms };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Trusts what `trust` says, from now on in this process, in place of
/// what was trusted before ([`CA_ENV`] alone until the first call): the
/// user's own settings, never anything from the network.
pub fn set_trust(trust: Trust) {
    *HELD.lock().unwrap() = Some(Held { trust: Arc::new(trust), configs: HashMap::new() });
}

/// Trusts the CA certificates in a PEM file for every `wss://` relay,
/// besides the built-in roots and what is trusted already.
pub fn trust_ca_file(path: &Path) -> io::Result<()> {
    let certs = read_certs(path)?;
    let mut held = HELD.lock().unwrap();
    let mut trust = match held.take() {
        Some(h) => (*h.trust).clone(),
        None => Trust::from_env().map_err(invalid)?,
    };
    trust.all.extend(certs);
    *held = Some(Held { trust: Arc::new(trust), configs: HashMap::new() });
    Ok(())
}

/// The client configuration for a relay named `host` (as its URL writes
/// it): the built-in roots and the authorities trusted for every relay,
/// or what the user's settings say for that host.
pub fn client_config_for(host: &str) -> io::Result<Arc<ClientConfig>> {
    let mut guard = HELD.lock().unwrap();
    if guard.is_none() {
        let trust = Trust::from_env().map_err(invalid)?;
        *guard = Some(Held { trust: Arc::new(trust), configs: HashMap::new() });
    }
    let held = guard.as_mut().unwrap();
    let own = held.trust.host(host).cloned();
    let key = if own.is_some() { crate::trust::host_key(host) } else { String::new() };
    if let Some(c) = held.configs.get(&key) {
        return Ok(c.clone());
    }
    let config = match &own {
        Some(h) => host_config(&held.trust, h)?,
        None => with_roots(roots(held.trust.all.iter().cloned(), true)?)?,
    };
    held.configs.insert(key, config.clone());
    Ok(config)
}

/// The client configuration for a relay without settings of its own.
pub fn client_config() -> io::Result<Arc<ClientConfig>> {
    client_config_for("")
}

/// Counts the bytes read through it and notes the first (for the
/// handshake's error messages).
struct Counting<'a> {
    tcp: &'a mut TcpStream,
    read: usize,
    first: Option<u8>,
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.tcp.read(buf)?;
        if self.read == 0 && n > 0 {
            self.first = Some(buf[0]);
        }
        self.read += n;
        Ok(n)
    }
}

impl Write for Counting<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tcp.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

/// The rustls error inside an I/O error, if it is one.
fn rustls_error(e: &io::Error) -> Option<&rustls::Error> {
    e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>())
}

/// Why a client's TLS handshake failed, in words (README "Relays over
/// TLS").
fn client_failure(e: io::Error, first: Option<u8>) -> io::Error {
    use io::ErrorKind::*;
    // Something answered, but not with TLS: an HTTP server, most likely.
    if first.is_some_and(|b| !(0x14..=0x17).contains(&b)) {
        return io::Error::new(InvalidData, "this relay does not speak TLS (try ws://)");
    }
    if first.is_none() && matches!(e.kind(), UnexpectedEof | ConnectionReset | ConnectionAborted) {
        return io::Error::new(e.kind(), "the relay closed the connection at the TLS handshake: it may not speak TLS (try ws://)");
    }
    let Some(r) = rustls_error(&e) else { return e };
    let words = match r {
        rustls::Error::InvalidCertificate(c) => match c {
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                "the relay's certificate is for another name".to_string()
            }
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => "the relay's certificate has expired".into(),
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                "the relay's certificate is not valid yet (is this computer's clock right?)".into()
            }
            CertificateError::Revoked => "the relay's certificate has been revoked".into(),
            CertificateError::UnknownIssuer => "the relay's certificate is not trusted (it is not from a known authority; \
                 for a private one, add it to your settings: README \"Private authorities and pins\")"
                .to_string(),
            CertificateError::Other(o) if o.0.downcast_ref::<PinRefused>().is_some() => o.0.to_string(),
            other => format!("the relay's certificate is not trusted ({other:?})"),
        },
        rustls::Error::InvalidMessage(_) => "this relay does not speak TLS (try ws://)".into(),
        rustls::Error::AlertReceived(a) => format!("the relay refused the TLS handshake ({a:?})"),
        rustls::Error::PeerIncompatible(p) => format!("the relay's TLS is not compatible ({p:?})"),
        other => format!("TLS handshake failed: {other}"),
    };
    io::Error::new(InvalidData, words)
}

/// Opens TLS over `tcp` to the relay named `host` (a name or an IP
/// address; the certificate must be for it). The handshake waits as long
/// as the stream's read and write timeouts allow.
pub fn connect(host: &str, mut tcp: TcpStream) -> io::Result<Stream> {
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let name = ServerName::try_from(host.to_string()).map_err(|_| invalid(format!("{host}: not a host name for TLS")))?;
    let mut conn = ClientConnection::new(client_config_for(host)?, name).map_err(io::Error::other)?;
    let mut counting = Counting { tcp: &mut tcp, read: 0, first: None };
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut counting) {
            return Err(client_failure(e, counting.first));
        }
    }
    Ok(Stream::Client(Box::new(StreamOwned::new(conn, tcp))))
}

/// A relay's certificate chain and key, from PEM files.
pub fn server_config(cert: &Path, key: &Path) -> io::Result<Arc<ServerConfig>> {
    let certs = read_certs(cert)?;
    let key_bytes = std::fs::read(key).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", key.display())))?;
    let key = PrivateKeyDer::from_pem_slice(&key_bytes)
        .map_err(|e| invalid(format!("{}: not a PEM private key: {e}", key.display())))?;
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| invalid(format!("the certificate and key: {e}")))?;
    Ok(Arc::new(config))
}

/// Whether the first byte a client sent starts a TLS handshake.
pub fn is_tls_hello(first: u8) -> bool {
    first == TLS_HANDSHAKE
}

/// Whether the first byte of an answer is a TLS record: a TLS server
/// answering a plain request (with an alert, usually).
pub fn is_tls_record(first: u8) -> bool {
    first == TLS_ALERT || first == TLS_HANDSHAKE
}

/// Reads and drops what the client has sent (up to 16 KiB, waiting at most
/// 100 ms), so that closing the connection does not reset it before the
/// client has read an answer.
fn drain(tcp: &mut TcpStream) {
    let _ = tcp.set_read_timeout(Some(std::time::Duration::from_millis(100)));
    let mut buf = [0; 4096];
    for _ in 0..4 {
        if !matches!(tcp.read(&mut buf), Ok(n) if n > 0) {
            break;
        }
    }
}

/// Sends a last answer and closes the connection gently ([`drain`]).
pub fn answer_and_close(tcp: &mut TcpStream, answer: &[u8]) {
    drain(tcp);
    let _ = tcp.write_all(answer);
    let _ = tcp.shutdown(std::net::Shutdown::Write);
    drain(tcp);
}

/// What a relay without TLS answers a client that starts a TLS handshake:
/// an HTTP refusal, which the client sees is not TLS.
pub const NOT_TLS_ANSWER: &[u8] = b"HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: 46\r\nConnection: close\r\n\r\nThis relay does not speak TLS: use ws:// URLs\n";

/// Accepts TLS over `tcp` on a relay. On a failure the alert rustls
/// queued (if any) is sent, so a plain client learns the relay wants TLS.
pub fn accept(config: Arc<ServerConfig>, mut tcp: TcpStream) -> io::Result<Stream> {
    let mut conn = ServerConnection::new(config).map_err(io::Error::other)?;
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut tcp) {
            let mut alert = Vec::new();
            while conn.wants_write() {
                if conn.write_tls(&mut alert).is_err() {
                    break;
                }
            }
            answer_and_close(&mut tcp, &alert);
            return Err(e);
        }
    }
    Ok(Stream::Server(Box::new(StreamOwned::new(conn, tcp))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_files() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../terse-testkit/tls");
        assert_eq!(read_certs(&dir.join("ca.pem")).unwrap().len(), 1);
        assert!(server_config(&dir.join("localhost.pem"), &dir.join("localhost.key")).is_ok());
        // A key that is not the certificate's.
        assert!(server_config(&dir.join("localhost.pem"), &dir.join("wrong.key")).is_err());
        assert!(read_certs(&dir.join("localhost.key")).is_err());
        assert!(read_certs(&dir.join("missing.pem")).is_err());
        assert!(client_config().is_ok());
    }
}
