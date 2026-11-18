//! Finding a relay when none is named: the config directory's `relay` file,
//! else a relay listening on this machine's default port. `terse-host` adds
//! a third choice, a relay of its own (README, "Getting started").

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The port a local relay listens on by default (`terse-relay`, and the
/// relay `terse-host` starts).
pub const DEFAULT_PORT: u16 = 7878;
/// Overrides [`DEFAULT_PORT`] for finding and starting a local relay (so
/// tests never touch the real one).
pub const PORT_ENV: &str = "TERSE_LOCAL_RELAY_PORT";
/// The file in the config directory that names the relay.
pub const RELAY_FILE: &str = "relay";
/// How long to wait for a local relay to accept a connection.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// [`DEFAULT_PORT`], or the port in `TERSE_LOCAL_RELAY_PORT`.
pub fn local_port() -> u16 {
    std::env::var(PORT_ENV).ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

/// The loopback address of the local relay on `port`.
pub fn local_addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

pub fn local_url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}")
}

/// Whether `url` is a relay URL this host accepts in the `relay` file.
pub fn valid_url(url: &str) -> bool {
    url.strip_prefix("ws://").or_else(|| url.strip_prefix("wss://")).is_some_and(|rest| !rest.is_empty())
        && !url.chars().any(char::is_whitespace)
}

/// The relay named in `<config>/relay`, if the file exists.
pub fn read_relay_file(config: &Path) -> Result<Option<String>, String> {
    let path = config.join(RELAY_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let url = text.trim();
    if !valid_url(url) {
        return Err(format!("{}: not a ws:// or wss:// URL: {url}", path.display()));
    }
    Ok(Some(url.to_string()))
}

/// Writes `url` to `<config>/relay`.
pub fn write_relay_file(config: &Path, url: &str) -> Result<PathBuf, String> {
    if !valid_url(url) {
        return Err(format!("not a ws:// or wss:// URL: {url}"));
    }
    std::fs::create_dir_all(config).map_err(|e| format!("{}: {e}", config.display()))?;
    let path = config.join(RELAY_FILE);
    std::fs::write(&path, format!("{url}\n")).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Whether something accepts a TCP connection on 127.0.0.1:`port`.
pub fn accepts(port: u16) -> bool {
    TcpStream::connect_timeout(&local_addr(port), PROBE_TIMEOUT).is_ok()
}

/// Which relay to use, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Named with `--relay`.
    Given(String),
    /// `--relay none`: no network.
    Disabled,
    /// From the config directory's `relay` file.
    ConfigFile(String, PathBuf),
    /// Something accepts connections on the local port.
    Local(String),
    /// None of these.
    Nothing,
}

impl Found {
    pub fn url(&self) -> Option<&str> {
        match self {
            Found::Given(u) | Found::ConfigFile(u, _) | Found::Local(u) => Some(u),
            Found::Disabled | Found::Nothing => None,
        }
    }

    /// One line saying which relay is used and why.
    pub fn describe(&self) -> String {
        match self {
            Found::Given(u) => format!("relay {u} (--relay)"),
            Found::Disabled => "no relay (--relay none)".into(),
            Found::ConfigFile(u, p) => format!("relay {u} (from {})", p.display()),
            Found::Local(u) => format!("relay {u} (already running on this machine)"),
            Found::Nothing => "no relay".into(),
        }
    }
}

/// The relay to use: `given` (`--relay`; `none` disables the network), else
/// the `relay` file in `config`, else a relay accepting connections on
/// 127.0.0.1:`port`.
pub fn find_relay(given: Option<&str>, config: Option<&Path>, port: u16) -> Result<Found, String> {
    match given {
        Some("none") => return Ok(Found::Disabled),
        Some(url) => return Ok(Found::Given(url.to_string())),
        None => {}
    }
    if let Some(config) = config
        && let Some(url) = read_relay_file(config)?
    {
        return Ok(Found::ConfigFile(url, config.join(RELAY_FILE)));
    }
    Ok(if accepts(port) { Found::Local(local_url(port)) } else { Found::Nothing })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port nothing listens on (bound, then released).
    fn closed_port() -> u16 {
        std::net::TcpListener::bind(local_addr(0)).unwrap().local_addr().unwrap().port()
    }

    #[test]
    fn config_file_then_local_relay() {
        let config = tempfile::tempdir().unwrap();
        let port = closed_port();
        assert_eq!(find_relay(None, Some(config.path()), port).unwrap(), Found::Nothing);
        assert_eq!(find_relay(Some("none"), Some(config.path()), port).unwrap(), Found::Disabled);
        let listener = std::net::TcpListener::bind(local_addr(0)).unwrap();
        let open = listener.local_addr().unwrap().port();
        assert_eq!(find_relay(None, Some(config.path()), open).unwrap(), Found::Local(local_url(open)));
        let path = write_relay_file(config.path(), "ws://relay.example:7878").unwrap();
        let found = find_relay(None, Some(config.path()), open).unwrap();
        assert_eq!(found, Found::ConfigFile("ws://relay.example:7878".into(), path));
        assert_eq!(found.url(), Some("ws://relay.example:7878"));
        assert_eq!(find_relay(Some("ws://x:1"), Some(config.path()), open).unwrap(), Found::Given("ws://x:1".into()));
        assert!(write_relay_file(config.path(), "http://x").is_err());
        std::fs::write(config.path().join(RELAY_FILE), "garbage\n").unwrap();
        assert!(find_relay(None, Some(config.path()), open).is_err());
    }
}
