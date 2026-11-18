//! `terse-relay`: serves blobs and feeds over WebSocket (NETWORK.md sections 3 and 4.3).

use std::net::TcpListener;
use std::process::ExitCode;
use std::sync::Arc;

use terse_net::BlobStore;
use terse_relay::{DEFAULT_FEED_CAP, MIN_FEED_CAP, Relay};

const USAGE: &str = "usage: terse-relay [--listen <addr:port>] --store <dir> [--max-store-bytes N] [--feed-entries N]
  --listen defaults to 127.0.0.1:7878 (loopback only)
  --feed-entries: entries kept per feed, at least 256 (default 1024)
  Feed entries are kept in <dir>/feeds.";


fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("terse-relay: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let default_listen = terse_net::local::local_addr(terse_net::local::DEFAULT_PORT).to_string();
    let (mut listen, mut store, mut limit, mut cap) = (default_listen, None, None, DEFAULT_FEED_CAP);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value\n{USAGE}"));
        match a.as_str() {
            "--listen" => listen = value()?,
            "--store" => store = Some(value()?),
            "--max-store-bytes" => {
                let n = value()?;
                limit = Some(n.parse::<u64>().map_err(|_| format!("bad byte count {n}"))?);
            }
            "--feed-entries" => {
                let n = value()?;
                cap = n.parse().ok().filter(|&c| c >= MIN_FEED_CAP).ok_or(format!("bad entry count {n} (at least 256)"))?;
            }
            _ => return Err(USAGE.into()),
        }
    }
    let dir = store.ok_or(USAGE)?;
    let store = BlobStore::open(&dir, limit).map_err(|e| format!("{dir}: {e}"))?;
    let listener = TcpListener::bind(&listen).map_err(|e| format!("{listen}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    // Printed on stdout so scripts and tests can find an ephemeral port.
    println!("listening on ws://{addr}");
    eprintln!("terse-relay: store {dir} ({} bytes used)", store.used_bytes());
    terse_relay::serve(listener, Arc::new(Relay::new(store, cap))).map_err(|e| e.to_string())
}
