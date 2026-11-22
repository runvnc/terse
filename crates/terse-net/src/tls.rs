//! TLS for relay connections (`wss://`, NETWORK 3), on both sides: a
//! client of a `wss://` relay, and a relay given a certificate
//! (`terse-relay --tls-cert`).
//!
//! The stack is rustls with the ring crypto provider. The roots a client
//! trusts are compiled in (`webpki-roots`, Mozilla's list), so every
//! system, a static Linux binary on a bare server included, trusts the same
//! certificates; a release refreshes them (RELEASING.md). A self-hosted
//! relay with a certificate from a private CA is trusted by naming the
//! CA's PEM file in [`CA_ENV`] (or with [`trust_ca_file`]), on top of the
//! built-in roots. Certificates are always verified, the relay's name
//! included; there is no way to turn that off.
//!
//! A [`Stream`] is what a relay WebSocket runs over: plain TCP or TLS over
//! TCP. Read and write timeouts are those of the TCP stream, so the read
//! loops that poll with a short timeout behave the same over TLS.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
pub use rustls::ServerConfig;
use rustls::{CertificateError, ClientConfig, ClientConnection, RootCertStore, ServerConnection, StreamOwned};

/// The environment variable naming a PEM file of CA certificates that a
/// client trusts for `wss://` relays besides the built-in roots: for a
/// self-hosted relay whose certificate comes from a private CA (and for
/// the tests).
pub const CA_ENV: &str = "TERSE_RELAY_CA";

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

/// Extra trusted CAs: from [`trust_ca_file`], else from [`CA_ENV`] the
/// first time a client config is needed.
static CLIENT: Mutex<Option<Arc<ClientConfig>>> = Mutex::new(None);

fn build_client(extra: &[CertificateDer<'static>]) -> io::Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    for cert in extra {
        roots.add(cert.clone()).map_err(|e| invalid(format!("a trusted CA certificate: {e}")))?;
    }
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Trusts the CA certificates in a PEM file for `wss://` relays, besides
/// the built-in roots, from now on in this process (in place of
/// [`CA_ENV`]).
pub fn trust_ca_file(path: &Path) -> io::Result<()> {
    let config = build_client(&read_certs(path)?)?;
    *CLIENT.lock().unwrap() = Some(config);
    Ok(())
}

/// The client configuration: the built-in roots, and the CAs of
/// [`trust_ca_file`] or [`CA_ENV`].
pub fn client_config() -> io::Result<Arc<ClientConfig>> {
    let mut held = CLIENT.lock().unwrap();
    if let Some(c) = held.as_ref() {
        return Ok(c.clone());
    }
    let extra = match std::env::var_os(CA_ENV).filter(|p| !p.is_empty()) {
        Some(path) => read_certs(Path::new(&path)).map_err(|e| io::Error::new(e.kind(), format!("{CA_ENV}: {e}")))?,
        None => Vec::new(),
    };
    let config = build_client(&extra)?;
    *held = Some(config.clone());
    Ok(config)
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
            CertificateError::UnknownIssuer => {
                format!("the relay's certificate is not trusted (it is not from a known authority; for a private CA, see {CA_ENV})")
            }
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
    let mut conn = ClientConnection::new(client_config()?, name).map_err(io::Error::other)?;
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
