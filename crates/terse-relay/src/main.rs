//! `terse-relay`: serves blobs and feeds over WebSocket (NETWORK.md sections 3 and 4.3).

use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::BlobStore;
use terse_relay::limits::{Cidr, Rate};
use terse_relay::{GIB, KIB, MIB, MIN_FEED_CAP, Policy, Relay};

const USAGE: &str = "usage: terse-relay [--listen <addr:port>] --store <dir> [options]
       terse-relay deny <file> [feed <key> | blob <id> | address <addr[/bits]>] [reason...]
       terse-relay allow <file> feed <key> | blob <id> | address <addr[/bits]>
       terse-relay feeds <store dir> [--by size|arrival] [--limit <n>]
       terse-relay feed <store dir> <key>
  (deny: adds a line to the deny list, or lists it; allow: removes the item's lines. A
  running relay reads the file again within 2 s. feeds, feed: what the store holds, read
  from its files: no network, safe beside a running relay. README \"Running a public relay\")
  --listen <addr:port>      default 127.0.0.1:7878 (loopback only)
  --peer <wss://host[:port]>  another relay to mirror (NETWORK.md section 5); repeatable;
                            ws:// for a plain one; options may follow in the same argument: \"wss://h pin=sha256:...\" (authority=,
                            only-authority=); TERSE_RELAY_CA is trusted too
  --tls-cert <pem> --tls-key <pem>
                            serve wss:// (TLS) with this certificate chain and key;
                            read again on SIGHUP (renewals). Without them: plain ws://
  --max-store-bytes <size>  all blobs together, at most (sizes: 1048576, 512K, 16M, 20G)
  --feed-entries <n>        entries kept per feed, at least 256 (default 1024)
  Feed entries are kept in <dir>/feeds.

Limits for a public relay (README \"Running a public relay\"); all off unless given:
  --public                  the proposed defaults: --feed-bytes 16M --unreferenced-time 1h
                            --uploads-per-minute 120 --upload-bytes-per-hour 32M
                            --new-feeds-per-hour 3 --new-feeds-per-hour-total 60
                            --finds-per-minute 60 --connections-per-address 32
                            --max-connections 150 --subscriptions-per-connection 1024
                            --status-every 10m (memory at worst about 514 MiB: the
                            status line says)
  --feed-bytes <size|none>  bytes of the blobs one feed's entries name, at most;
                            older entries are dropped to make room
  --unreferenced-time <time|forever>
                            how long a blob no entry names is kept (and it is not
                            served); e.g. 3600, 90s, 30m, 1h
  --unreferenced-bytes <size>              such blobs together (default 256M)
  --unreferenced-bytes-per-address <size>  per address (default 16M)
  --uploads-per-minute <n>  puts and publishes per address
  --upload-bytes-per-hour <size>           bytes put per address
  --new-feeds-per-hour <n>  entries of feeds new to this relay, per address
  --new-feeds-per-hour-total <n|none>      the same from all addresses together
  --finds-per-minute <n>    finds and lookups per address
  --no-rate-limits          none of the five above
  --connections-per-address <n|none>, --max-connections <n|none>
  --subscriptions-per-connection <n|none>  feeds one connection may subscribe to
  --memory <size>           the most memory to take at worst: sets --max-connections
                            (3 MiB each, 64 MiB besides, and the index of what is held)
  --deny <file>             the operator's deny list (feed <key>, blob <id>,
                            address <addr[/bits]>, each with a reason; # comments):
                            refused and removed; read again when it changes or on SIGHUP
  --writers <file>          take entries only from the feeds listed (one key per
                            line, # comments); read again when it changes or on SIGHUP
  --trust-proxy <addr[/bits]>[,...]   connections from these addresses are a proxy's:
                            the client is the one CF-Connecting-IP or X-Forwarded-For
                            names (repeatable; @<file>: the ranges in a file, one a line)
  --ping-every <time|never> a ping on a silent connection (default 30s)
  --status-every <time|never>  a status line on stderr; --status-file <path>: kept
                            up to date in a file (every 5 s), with a line for each
                            table or log that could grow and its bound
  An IPv6 client is counted per /64.";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("terse-relay: {e}");
            ExitCode::from(2)
        }
    }
}

/// `1048576`, `512K`, `16M`, `20G` (also `KiB`, `MiB`, `GiB`, `KB`...: all
/// powers of 1024).
fn parse_size(s: &str) -> Option<u64> {
    let digits = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (n, unit) = s.split_at(digits);
    let n: u64 = n.parse().ok()?;
    let unit = unit.to_ascii_lowercase();
    let unit = unit.trim_end_matches("ib").trim_end_matches('b');
    let factor = match unit {
        "" => 1,
        "k" => KIB,
        "m" => MIB,
        "g" => GIB,
        "t" => 1024 * GIB,
        _ => return None,
    };
    n.checked_mul(factor)
}

/// `3600`, `90s`, `30m`, `1h`, `1d`.
fn parse_time(s: &str) -> Option<Duration> {
    let digits = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (n, unit) = s.split_at(digits);
    let n: u64 = n.parse().ok()?;
    let factor = match unit {
        "" | "s" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    Some(Duration::from_secs(n.checked_mul(factor)?))
}

fn optional<T>(s: &str, words: &[&str], parse: impl Fn(&str) -> Option<T>) -> Option<Option<T>> {
    if words.contains(&s) { Some(None) } else { parse(s).map(Some) }
}

/// `terse-relay deny|allow|feeds|feed ...`: the operator's commands.
fn admin(command: &str, args: &[String]) -> Result<(), String> {
    use terse_relay::{admin, deny};
    match (command, args) {
        ("deny", [file]) => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            for l in deny::parse(&text).map_err(|e| format!("{file}: {e}"))? {
                println!("{}{}{}", l.item.text(), if l.reason.is_empty() { "" } else { "  " }, l.reason);
            }
        }
        ("deny", [file, kind, value, reason @ ..]) => {
            let item = deny::Item::parse(kind, value)?;
            if deny::add(std::path::Path::new(file), item, &reason.join(" "))? {
                eprintln!("terse-relay: {file}: {} denied (a running relay applies it within 2 s)", item.text());
            } else {
                eprintln!("terse-relay: {file}: {} was denied already", item.text());
            }
        }
        ("allow", [file, kind, value]) => {
            let item = deny::Item::parse(kind, value)?;
            match deny::remove(std::path::Path::new(file), item)? {
                0 => return Err(format!("{file}: {} is not denied there", item.text())),
                _ => eprintln!("terse-relay: {file}: {} allowed again (what was removed is not restored)", item.text()),
            }
        }
        ("feeds", [dir, rest @ ..]) => {
            let (mut order, mut limit) = (admin::Order::Size, usize::MAX);
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                let v = it.next().ok_or(format!("{a} needs a value"))?;
                match a.as_str() {
                    "--by" if v == "size" => order = admin::Order::Size,
                    "--by" if v == "arrival" => order = admin::Order::Arrival,
                    "--limit" => limit = v.parse().map_err(|_| format!("--limit {v}: not a number"))?,
                    _ => return Err(USAGE.into()),
                }
            }
            println!("feed key                                                          entries      bytes     stored  name");
            for f in admin::feeds(std::path::Path::new(dir), order)?.into_iter().take(limit) {
                println!(
                    "{} {:>7} {:>10} {:>10}  {}",
                    terse_net::blob::to_hex(&f.key),
                    f.entries,
                    f.bytes,
                    f.stored,
                    admin::printable(f.name.as_deref().unwrap_or("-"))
                );
            }
        }
        ("feed", [dir, key]) => {
            let key = terse_net::blob::parse_hex(&key.to_ascii_lowercase()).ok_or(format!("not a feed key: {key}"))?;
            println!("seq        time (UTC, as claimed)  kind        size  stored  blob / title");
            for e in admin::entries(std::path::Path::new(dir), &key)? {
                let when = admin::utc(e.time);
                println!(
                    "{:<10} {:<23} {:<8} {:>8}  {:<6}  {}\n{:>60}{}",
                    e.seq,
                    when,
                    e.kind,
                    e.size,
                    if e.stored { "yes" } else { "no" },
                    e.blob,
                    "",
                    admin::printable(&e.title)
                );
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn run(args: Vec<String>) -> Result<(), String> {
    if let Some(command) = args.first().filter(|a| ["deny", "allow", "feeds", "feed"].contains(&a.as_str())) {
        return admin(command, &args[1..]);
    }
    let default_listen = terse_net::local::local_addr(terse_net::local::DEFAULT_PORT).to_string();
    let (mut listen, mut store, mut limit) = (default_listen, None, None);
    let mut peers = Vec::new();
    let mut peer_lines = Vec::new();
    let mut memory = None;
    let (mut tls_cert, mut tls_key) = (None, None);
    // Options are applied in order over the base policy, so `--public`
    // comes first whatever its place.
    let public = args.iter().any(|a| a == "--public");
    let mut policy = if public { Policy::public() } else { Policy::unlimited(terse_relay::DEFAULT_FEED_CAP) };
    let base_rates = Policy::public().rates.unwrap();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value\n{USAGE}"));
        let bad = |v: &str| format!("{a}: bad value {v}");
        macro_rules! rates {
            () => {
                policy.rates.get_or_insert_with(|| base_rates.clone())
            };
        }
        match a.as_str() {
            "--public" => {}
            "--listen" => listen = value()?,
            "--store" => store = Some(value()?),
            "--peer" => {
                // A relay line: wss://, or ws:// on this machine only
                // (NETWORK 3), maybe with options (authority=, pin=).
                let line = value()?;
                let (url, _) = terse_net::local::relay_line(&line)
                    .map_err(|e| format!("--peer {line}: {e}"))?
                    .ok_or(format!("--peer {line}: not a relay"))?;
                peer_lines.push(line);
                peers.push(url);
            }
            "--tls-cert" => tls_cert = Some(std::path::PathBuf::from(value()?)),
            "--tls-key" => tls_key = Some(std::path::PathBuf::from(value()?)),
            "--max-store-bytes" => {
                let n = value()?;
                limit = Some(parse_size(&n).ok_or(format!("bad byte count {n}"))?);
            }
            "--feed-entries" => {
                let n = value()?;
                policy.feed_entries =
                    n.parse().ok().filter(|&c| c >= MIN_FEED_CAP).ok_or(format!("bad entry count {n} (at least 256)"))?;
            }
            "--feed-bytes" => {
                let v = value()?;
                policy.feed_bytes = optional(&v, &["none", "0"], parse_size).ok_or(bad(&v))?;
            }
            "--unreferenced-time" => {
                let v = value()?;
                policy.unreferenced_ttl = optional(&v, &["forever", "0"], parse_time).ok_or(bad(&v))?;
                // NETWORK 5.3: at least 10 minutes, for a publisher to
                // put a blob and then publish its entry.
                if policy.unreferenced_ttl.is_some_and(|t| t < Duration::from_secs(600)) {
                    return Err(format!("{a}: at least 10m"));
                }
            }
            "--unreferenced-bytes" => {
                let v = value()?;
                policy.unreferenced_bytes = parse_size(&v).ok_or(bad(&v))?;
            }
            "--unreferenced-bytes-per-address" => {
                let v = value()?;
                policy.unreferenced_bytes_per_address = parse_size(&v).ok_or(bad(&v))?;
            }
            "--uploads-per-minute" => {
                let v = value()?;
                let n = v.parse::<u32>().ok().filter(|&n| n > 0).ok_or(bad(&v))?;
                rates!().uploads = Rate::new(f64::from(n), Duration::from_secs(60));
            }
            "--upload-bytes-per-hour" => {
                let v = value()?;
                let n = parse_size(&v).filter(|&n| n >= terse_net::blob::MAX_BLOB as u64).ok_or(format!("{a}: at least 1M"))?;
                rates!().upload_bytes = Rate::new(n as f64, Duration::from_secs(3600));
            }
            "--new-feeds-per-hour" => {
                let v = value()?;
                let n = v.parse::<u32>().ok().filter(|&n| n > 0).ok_or(bad(&v))?;
                rates!().new_feeds = Rate::new(f64::from(n), Duration::from_secs(3600));
            }
            "--new-feeds-per-hour-total" => {
                let v = value()?;
                let n = optional(&v, &["none"], |s| s.parse::<u32>().ok().filter(|&n| n > 0)).ok_or(bad(&v))?;
                policy.new_feeds_total = n.map(|n| Rate::new(f64::from(n), Duration::from_secs(3600)));
            }
            "--finds-per-minute" => {
                let v = value()?;
                let n = v.parse::<u32>().ok().filter(|&n| n > 0).ok_or(bad(&v))?;
                rates!().finds = Rate::new(f64::from(n), Duration::from_secs(60));
            }
            "--no-rate-limits" => {
                policy.rates = None;
                policy.new_feeds_total = None;
            }
            "--connections-per-address" => {
                let v = value()?;
                policy.connections_per_address = optional(&v, &["none"], |s| s.parse().ok().filter(|&n| n > 0)).ok_or(bad(&v))?;
            }
            "--max-connections" => {
                let v = value()?;
                policy.max_connections = optional(&v, &["none"], |s| s.parse().ok().filter(|&n| n > 0)).ok_or(bad(&v))?;
            }
            "--subscriptions-per-connection" => {
                let v = value()?;
                policy.subscriptions_per_connection =
                    optional(&v, &["none"], |s| s.parse().ok().filter(|&n| n > 0)).ok_or(bad(&v))?;
            }
            "--writers" => policy.writers = Some(value()?.into()),
            "--deny" => policy.deny = Some(value()?.into()),
            "--memory" => {
                let v = value()?;
                memory = Some(parse_size(&v).ok_or(bad(&v))?);
            }
            "--trust-proxy" => {
                // `@<file>`: the ranges in a file, one per line (# comments).
                let v = value()?;
                let text = match v.strip_prefix('@') {
                    Some(path) => std::fs::read_to_string(path).map_err(|e| format!("--trust-proxy {path}: {e}"))?,
                    None => v,
                };
                let items = text.lines().map(|l| l.split('#').next().unwrap_or_default()).flat_map(|l| l.split([',', ' ', '\t']));
                for item in items.map(str::trim).filter(|s| !s.is_empty()) {
                    policy.trust_proxy.push(Cidr::parse(item).ok_or(format!("--trust-proxy: not an address or range: {item}"))?);
                }
            }
            "--ping-every" => {
                let v = value()?;
                policy.ping_every = optional(&v, &["never", "0"], parse_time).ok_or(bad(&v))?;
            }
            "--status-every" => {
                let v = value()?;
                policy.status_every = optional(&v, &["never", "0"], parse_time).ok_or(bad(&v))?;
            }
            "--status-file" => policy.status_file = Some(value()?.into()),
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(USAGE.into()),
        }
    }
    let dir = store.ok_or(USAGE)?;
    let tls = match (tls_cert, tls_key) {
        (Some(cert), Some(key)) => Some((cert, key)),
        (None, None) => None,
        _ => return Err("--tls-cert and --tls-key go together".into()),
    };
    let load_tls = |(cert, key): &(std::path::PathBuf, std::path::PathBuf)| terse_net::tls::server_config(cert, key).map_err(|e| e.to_string());
    let tls_config = tls.as_ref().map(load_tls).transpose()?;
    let store = BlobStore::open(&dir, limit).map_err(|e| format!("{dir}: {e}"))?;
    let listener = TcpListener::bind(&listen).map_err(|e| format!("{listen}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    if (policy.rates.is_some() || policy.connections_per_address.is_some()) && policy.trust_proxy.is_empty() {
        eprintln!("terse-relay: limits per address count each TCP peer; behind a proxy, give --trust-proxy");
    }
    let started = Instant::now();
    if let Some(m) = memory {
        // The index is not known yet: the connections get what the
        // program and its buffers leave; the status line then says the
        // worst case with the index.
        let n = Policy::connections_for_memory(m, 0);
        if n == 0 {
            return Err(format!("--memory {m}: too little for one connection"));
        }
        policy.max_connections = Some(policy.max_connections.map_or(n, |c| c.min(n)));
    }
    let relay = Arc::new(Relay::with_policy(store, policy)?);
    let scheme = if tls_config.is_some() { "wss" } else { "ws" };
    relay.set_tls(tls_config);
    // Printed on stdout so scripts and tests can find an ephemeral port.
    println!("listening on {scheme}://{addr}");
    eprintln!("terse-relay: store {dir}: {} (ready in {} ms)", relay.status(), started.elapsed().as_millis());
    signals::install();
    // What a peer link trusts: TERSE_RELAY_CA and the --peer lines' options.
    terse_net::local::apply_trust(&peer_lines, None)?;
    terse_relay::connect_peers(relay.clone(), peers);
    let served = relay.clone();
    let server = std::thread::spawn(move || terse_relay::serve(listener, served));
    loop {
        std::thread::sleep(Duration::from_millis(100));
        if server.is_finished() {
            return server.join().map_err(|_| "the server stopped".to_string())?.map_err(|e| e.to_string());
        }
        if signals::take_reload() {
            relay.reload_writers(true);
            relay.reload_deny(true);
            // A renewed certificate: a file that cannot be read now
            // leaves the one in use.
            if let Some(files) = &tls {
                match load_tls(files) {
                    Ok(config) => {
                        relay.set_tls(Some(config));
                        eprintln!("terse-relay: certificate read again from {}", files.0.display());
                    }
                    Err(e) => eprintln!("terse-relay: certificate not read again, the old one stays: {e}"),
                }
            }
        }
        if signals::stopping() {
            break;
        }
    }
    // A graceful stop: connections are closed with a close frame (they
    // see it within a poll), then the process ends. What is stored is on
    // disk already: every blob and entry is written before it is
    // acknowledged.
    eprintln!("terse-relay: stopping: {}", relay.status());
    relay.close();
    let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(200));
    let deadline = Instant::now() + Duration::from_secs(3);
    while relay.connection_count() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(unix)]
mod signals {
    use std::sync::atomic::{AtomicBool, Ordering};

    static STOP: AtomicBool = AtomicBool::new(false);
    static RELOAD: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_stop(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }

    extern "C" fn on_reload(_: libc::c_int) {
        RELOAD.store(true, Ordering::Relaxed);
    }

    /// SIGTERM and SIGINT stop the relay gracefully; SIGHUP reads the
    /// write list and the certificate again.
    pub fn install() {
        // SAFETY: the handlers only store to atomics, which is
        // async-signal-safe.
        unsafe {
            libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, on_stop as *const () as libc::sighandler_t);
            libc::signal(libc::SIGHUP, on_reload as *const () as libc::sighandler_t);
        }
    }

    pub fn stopping() -> bool {
        STOP.load(Ordering::Relaxed)
    }

    pub fn take_reload() -> bool {
        RELOAD.swap(false, Ordering::Relaxed)
    }
}

#[cfg(not(unix))]
mod signals {
    pub fn install() {}
    pub fn stopping() -> bool {
        false
    }
    pub fn take_reload() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times() {
        assert_eq!(parse_size("1048576"), Some(MIB));
        assert_eq!(parse_size("16M"), Some(16 * MIB));
        assert_eq!(parse_size("16MiB"), Some(16 * MIB));
        assert_eq!(parse_size("20g"), Some(20 * GIB));
        assert_eq!(parse_size("512KB"), Some(512 * KIB));
        assert_eq!(parse_size("M"), None);
        assert_eq!(parse_size("1X"), None);
        assert_eq!(parse_time("3600"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_time("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_time("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_time("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_time("1y"), None);
    }
}
