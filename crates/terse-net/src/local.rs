//! Finding relays when none is named: the config directory's `relays` file
//! (one URL per line; the older single-URL `relay` file if `relays` does
//! not exist), else a relay listening on this machine's default port.
//! `terse-host` adds a third choice, a relay of its own (README, "Getting
//! started").

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The port a local relay listens on by default (`terse-relay`, and the
/// relay `terse-host` starts).
pub const DEFAULT_PORT: u16 = 7878;
/// Overrides [`DEFAULT_PORT`] for finding and starting a local relay (so
/// tests never touch the real one).
pub const PORT_ENV: &str = "TERSE_LOCAL_RELAY_PORT";
/// The file in the config directory that lists the relays, one per line.
pub const RELAYS_FILE: &str = "relays";
/// The older file naming one relay, read when `relays` does not exist.
pub const RELAY_FILE: &str = "relay";
/// Most relays a host connects to from its list (NETWORK 5).
pub const MAX_RELAYS: usize = 8;
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

/// Whether `url` is a relay URL this host accepts in its relay list.
/// It is also the test of NETWORK 4.6 for a profile's `relay` value:
/// `ws://` or `wss://`, then something that does not start with `/`, and
/// no control or whitespace character anywhere.
pub fn valid_url(url: &str) -> bool {
    url.strip_prefix("ws://").or_else(|| url.strip_prefix("wss://")).is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A relay as a person types it (NETWORK 3, "Encrypted by default"): a
/// `ws://` or `wss://` URL as it is, or a host and maybe a port without a
/// scheme, taken as `wss://` (`relay.tersenet.org` is
/// `wss://relay.tersenet.org`; a port is only a port), except that
/// `localhost` or a loopback address is taken as `ws://` (this machine:
/// `127.0.0.1:7878` is `ws://127.0.0.1:7878`). `None` if it is neither.
pub fn typed_url(text: &str) -> Option<String> {
    let t = text.trim();
    if valid_url(t) {
        return Some(t.to_string());
    }
    if t.is_empty() || t.contains("://") || t.contains('/') {
        return None;
    }
    let host = crate::relay::Endpoint::parse(&format!("wss://{t}")).ok()?.host;
    let scheme = if terse_core::hints::loopback_text(&host) { "ws" } else { "wss" };
    let url = format!("{scheme}://{t}");
    (valid_url(&url) && crate::relay::Endpoint::parse(&url).is_ok()).then_some(url)
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// A relay line (of the list, or a `--relay` value): its relay as a URL
/// and the line with that URL, then its options ([`crate::trust`]).
/// `Ok(None)` for a blank line or a `#` comment.
pub fn relay_line(line: &str) -> Result<Option<(String, String)>, String> {
    let Some(l) = crate::trust::split_line(line) else { return Ok(None) };
    let url = typed_url(l.relay).ok_or(format!("not a relay (a wss:// or ws:// URL, or a host): {}", l.relay))?;
    let mut text = url.clone();
    for o in &l.options {
        text.push(' ');
        text.push_str(o);
    }
    Ok(Some((url, text)))
}

/// The configured relay list as written, a line per relay with its
/// options, and the file it came from: `<config>/relays` (possibly empty),
/// else `<config>/relay`, else `None`. Blank lines, `#` comments and
/// repeated relays are skipped.
pub fn read_relay_lines(config: &Path) -> Result<Option<(Vec<(String, String)>, PathBuf)>, String> {
    for name in [RELAYS_FILE, RELAY_FILE] {
        let path = config.join(name);
        let Some(text) = read_optional(&path)? else { continue };
        let mut lines: Vec<(String, String)> = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let Some((url, line)) = relay_line(line).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))? else { continue };
            if !lines.iter().any(|(u, _)| *u == url) {
                lines.push((url, line));
            }
        }
        return Ok(Some((lines, path)));
    }
    Ok(None)
}

/// The configured relay list, as URLs ([`read_relay_lines`]).
pub fn read_relays(config: &Path) -> Result<Option<(Vec<String>, PathBuf)>, String> {
    Ok(read_relay_lines(config)?.map(|(l, p)| (l.into_iter().map(|(u, _)| u).collect(), p)))
}

/// Writes the relay list to `<config>/relays`: relay lines, each a relay
/// and maybe its options (checked: [`crate::trust::line_options`]).
pub fn write_relays(config: &Path, lines: &[String]) -> Result<PathBuf, String> {
    let mut text = String::new();
    for line in lines {
        let (url, line) = relay_line(line)?.ok_or("an empty relay line")?;
        let l = crate::trust::split_line(&line).unwrap();
        crate::trust::line_options(&url, &l.options, Some(config)).map_err(|e| e.to_string())?;
        text.push_str(&line);
        text.push('\n');
    }
    std::fs::create_dir_all(config).map_err(|e| format!("{}: {e}", config.display()))?;
    let path = config.join(RELAYS_FILE);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Authorities given on the command line (`--relay-authority`), trusted
/// for every relay by [`find_relays`].
static RELAY_AUTHORITIES: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Trusts the authorities of a PEM file for every relay
/// (`--relay-authority`), from the next [`find_relays`] on.
pub fn add_relay_authority(path: &Path) {
    RELAY_AUTHORITIES.lock().unwrap().push(path.to_path_buf());
}

/// Trusts what the user's settings say for `wss://` relays (README
/// "Private authorities and pins"): `config`'s authorities folder and the
/// options of its relay list, those of `given`, `--relay-authority` and
/// `TERSE_RELAY_CA`. Never anything from the network.
pub fn apply_trust(given: &[String], config: Option<&Path>) -> Result<(), String> {
    let authorities = RELAY_AUTHORITIES.lock().unwrap().clone();
    let trust = crate::trust::Trust::load(config, given, &authorities).map_err(|e| e.to_string())?;
    crate::tls::set_trust(trust);
    Ok(())
}

/// Whether something accepts a TCP connection on 127.0.0.1:`port`.
pub fn accepts(port: u16) -> bool {
    TcpStream::connect_timeout(&local_addr(port), PROBE_TIMEOUT).is_ok()
}

/// Which relays to use, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Named with `--relay` (one or more).
    Given(Vec<String>),
    /// `--relay none`: no network.
    Disabled,
    /// From the config directory's `relays` (or `relay`) file.
    ConfigFile(Vec<String>, PathBuf),
    /// Something accepts connections on the local port.
    Local(String),
    /// None of these.
    Nothing,
}

impl Found {
    /// The relays, at most [`MAX_RELAYS`].
    pub fn urls(&self) -> Vec<String> {
        let all = match self {
            Found::Given(u) | Found::ConfigFile(u, _) => u.clone(),
            Found::Local(u) => vec![u.clone()],
            Found::Disabled | Found::Nothing => Vec::new(),
        };
        all.into_iter().take(MAX_RELAYS).collect()
    }

    /// One line saying which relays are used and why.
    pub fn describe(&self) -> String {
        let list = |u: &[String]| {
            let more = if u.len() > MAX_RELAYS { format!(" (only the first {MAX_RELAYS} are used)") } else { String::new() };
            format!("relay{} {}{more}", if u.len() == 1 { "" } else { "s" }, u.join(" "))
        };
        match self {
            Found::Given(u) => format!("{} (--relay)", list(u)),
            Found::Disabled => "no relay (--relay none)".into(),
            Found::ConfigFile(u, p) => format!("{} (from {})", list(u), p.display()),
            Found::Local(u) => format!("relay {u} (already running on this machine)"),
            Found::Nothing => "no relay".into(),
        }
    }
}

/// The relays to use: `given` (`--relay`, repeatable; `none` alone disables
/// the network), else the list in `config` if it is not empty, else a relay
/// accepting connections on 127.0.0.1:`port`.
/// The settings of `config` and `given` for `wss://` relays are applied
/// first ([`apply_trust`]).
pub fn find_relays(given: &[String], config: Option<&Path>, port: u16) -> Result<Found, String> {
    let lines: Vec<String> = given.iter().filter(|g| *g != "none").cloned().collect();
    apply_trust(&lines, config)?;
    match given {
        [] => {}
        [none] if none == "none" => return Ok(Found::Disabled),
        _ if given.iter().any(|u| u == "none") => return Err("--relay none cannot be combined with relays".into()),
        _ => {
            let mut urls = Vec::new();
            for text in given {
                let (url, _) = relay_line(text)?.ok_or(format!("--relay {text}: not a relay (a wss:// or ws:// URL, or a host)"))?;
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
            return Ok(Found::Given(urls));
        }
    }
    if let Some(config) = config
        && let Some((urls, path)) = read_relays(config)?
        && !urls.is_empty()
    {
        return Ok(Found::ConfigFile(urls, path));
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

    fn list(u: &[&str]) -> Vec<String> {
        u.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn config_file_then_local_relay() {
        let config = tempfile::tempdir().unwrap();
        let port = closed_port();
        assert_eq!(find_relays(&[], Some(config.path()), port).unwrap(), Found::Nothing);
        assert_eq!(find_relays(&list(&["none"]), Some(config.path()), port).unwrap(), Found::Disabled);
        assert!(find_relays(&list(&["none", "ws://x:1"]), Some(config.path()), port).is_err());
        assert!(find_relays(&list(&["http://x"]), Some(config.path()), port).is_err());
        // Typed without a scheme: encrypted, whatever the port.
        assert_eq!(typed_url("relay.tersenet.org").as_deref(), Some("wss://relay.tersenet.org"));
        assert_eq!(typed_url(" relay.example:8443 ").as_deref(), Some("wss://relay.example:8443"));
        assert_eq!(typed_url("relay.example:7878").as_deref(), Some("wss://relay.example:7878"));
        assert_eq!(typed_url("relay.example:80").as_deref(), Some("wss://relay.example:80"));
        assert_eq!(typed_url("203.0.113.5:7878").as_deref(), Some("wss://203.0.113.5:7878"));
        assert_eq!(typed_url("192.168.1.5:7878").as_deref(), Some("wss://192.168.1.5:7878"));
        assert_eq!(typed_url("a.localhost:7878").as_deref(), Some("wss://a.localhost:7878"));
        // Written with a scheme: as written.
        assert_eq!(typed_url("ws://relay.example:7878").as_deref(), Some("ws://relay.example:7878"));
        assert_eq!(typed_url("wss://127.0.0.1:7878").as_deref(), Some("wss://127.0.0.1:7878"));
        // Loopback without a scheme: plain (this machine).
        assert_eq!(typed_url("127.0.0.1:7878").as_deref(), Some("ws://127.0.0.1:7878"));
        assert_eq!(typed_url("127.1.2.3:9").as_deref(), Some("ws://127.1.2.3:9"));
        assert_eq!(typed_url("localhost:7878").as_deref(), Some("ws://localhost:7878"));
        assert_eq!(typed_url("LocalHost:7878").as_deref(), Some("ws://LocalHost:7878"));
        assert_eq!(typed_url("[::1]:7878").as_deref(), Some("ws://[::1]:7878"));
        for bad in ["http://x", "", "a b", "relay.example/x", "[::1"] {
            assert_eq!(typed_url(bad), None, "{bad}");
        }
        let listener = std::net::TcpListener::bind(local_addr(0)).unwrap();
        let open = listener.local_addr().unwrap().port();
        assert_eq!(find_relays(&[], Some(config.path()), open).unwrap(), Found::Local(local_url(open)));
        // The old single-relay file is read when there is no list.
        std::fs::write(config.path().join(RELAY_FILE), "ws://old.example:7878\n").unwrap();
        let found = find_relays(&[], Some(config.path()), open).unwrap();
        assert_eq!(found, Found::ConfigFile(list(&["ws://old.example:7878"]), config.path().join(RELAY_FILE)));
        // The list wins over it.
        let path = write_relays(config.path(), &list(&["ws://a.example:1", "ws://b.example:2"])).unwrap();
        let found = find_relays(&[], Some(config.path()), open).unwrap();
        assert_eq!(found, Found::ConfigFile(list(&["ws://a.example:1", "ws://b.example:2"]), path));
        assert_eq!(found.urls(), list(&["ws://a.example:1", "ws://b.example:2"]));
        let two = list(&["ws://x:1", "ws://y:2"]);
        assert_eq!(find_relays(&two, Some(config.path()), open).unwrap(), Found::Given(two.clone()));
        // --relay and the list take hosts without a scheme as people type them.
        let typed = list(&["relay.example", "127.0.0.1:7878"]);
        let found = find_relays(&typed, Some(config.path()), open).unwrap();
        assert_eq!(found, Found::Given(list(&["wss://relay.example", "ws://127.0.0.1:7878"])));
        std::fs::write(config.path().join(RELAYS_FILE), "relay.example:9000\nlocalhost:7878\n").unwrap();
        let found = find_relays(&[], Some(config.path()), open).unwrap();
        assert_eq!(found.urls(), list(&["wss://relay.example:9000", "ws://localhost:7878"]));
        // An empty list falls through to the local relay.
        write_relays(config.path(), &[]).unwrap();
        assert_eq!(find_relays(&[], Some(config.path()), open).unwrap(), Found::Local(local_url(open)));
        assert!(write_relays(config.path(), &list(&["http://x"])).is_err());
        std::fs::write(config.path().join(RELAYS_FILE), "garbage/x\n").unwrap();
        assert!(find_relays(&[], Some(config.path()), open).is_err());
        let many: Vec<String> = (0..10).map(|i| format!("ws://r{i}.example")).collect();
        assert_eq!(Found::Given(many).urls().len(), MAX_RELAYS);
    }
}
