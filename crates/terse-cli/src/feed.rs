//! `terse key` and `terse feed` (NETWORK 4).
//!
//! The identity (`identity.key`) and the subscription list (`subscriptions`)
//! live in the config directory, `--config` (default `$XDG_CONFIG_HOME/terse`,
//! else `~/.config/terse`). Feed entries are kept with the blobs, in
//! `<store>/feeds`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terse_net::entry::{self, MAX_TITLE};
use terse_net::identity::valid_feed_key;
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{
    Article, ContentService, Done, Draft, Entry, FeedEvent, FeedKey, Identity, Node, Subscriptions,
    default_config_dir, parse_hex, to_hex,
};

use crate::{Args, USAGE, read};

/// `feed sync` without `--watch` gives up if not caught up (NETWORK 4.3)
/// after this long.
pub const SYNC_DEADLINE: Duration = Duration::from_secs(60);

/// Every `key` and `feed` command accepts these, used or not.
const OPTIONS: [&str; 3] = ["--config", "--relay", "--store"];

fn known(args: &Args, extra: &[&str]) -> Result<(), String> {
    args.check_known(&[&OPTIONS[..], extra].concat())
}

fn config_dir(args: &Args) -> Result<PathBuf, String> {
    args.get("--config").map(PathBuf::from).or_else(default_config_dir).ok_or("no config directory (use --config)".into())
}

fn identity_path(args: &Args) -> Result<PathBuf, String> {
    Ok(config_dir(args)?.join("identity.key"))
}

fn load_identity(args: &Args) -> Result<Identity, String> {
    let path = identity_path(args)?;
    if !path.exists() {
        return Err(format!("no identity in {} (create one with `terse key new`)", path.display()));
    }
    Identity::load(&path)
}

pub fn load_subscriptions(args: &Args) -> Result<Subscriptions, String> {
    Subscriptions::load(config_dir(args)?.join("subscriptions"))
}

/// A node on `--store` and `--relay` with the saved subscriptions.
fn open_node(args: &Args) -> Result<Node, String> {
    let store = args.get("--store").map(PathBuf::from);
    let mut node = Node::open(store, args.get("--relay"), DEFAULT_TIMEOUT)?;
    node.set_subscriptions(load_subscriptions(args)?);
    if let Some(n) = args.get("--auto-download") {
        node.set_auto_download_limit(n.parse().map_err(|_| format!("bad byte count {n}"))?);
    }
    Ok(node)
}

fn parse_feed_key(s: &str) -> Result<FeedKey, String> {
    parse_hex(s).filter(valid_feed_key).ok_or(format!("{s} is not a feed key (64 hex digits)"))
}

pub fn key(args: &[String]) -> Result<ExitCode, String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let args = Args::parse(rest, &[])?;
    known(&args, &[])?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    let identity = match command.as_str() {
        "new" => {
            let path = identity_path(&args)?;
            let identity = Identity::create(&path)?;
            eprintln!("terse: new identity saved in {}", path.display());
            identity
        }
        "show" => load_identity(&args)?,
        _ => return Err(USAGE.into()),
    };
    println!("{}", to_hex(&identity.feed_key()));
    Ok(ExitCode::SUCCESS)
}

pub fn feed(args: &[String]) -> Result<ExitCode, String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    match command.as_str() {
        "publish" => publish(rest),
        "subscribe" | "unsubscribe" => subscribe(rest, command == "subscribe"),
        "subscriptions" => subscriptions(rest),
        "sync" => sync(rest),
        "list" => list(rest),
        "read" => read_article(rest),
        _ => Err(USAGE.into()),
    }
}

fn now() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

fn publish(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &["--title", "--reply-to"])?;
    let data = read(args.one_positional()?)?;
    if !terse_net::blob::valid_size(data.len()) {
        return Err(format!("an article is 1 to {} bytes; this is {}", terse_net::blob::MAX_BLOB, data.len()));
    }
    let title = args.require("--title")?.to_string();
    if title.len() > MAX_TITLE {
        return Err(format!("title is {} bytes, at most {MAX_TITLE} allowed", title.len()));
    }
    let reply_to = match args.get("--reply-to") {
        Some(id) => Some(parse_hex(id).ok_or(format!("{id} is not a blob ID (64 hex digits)"))?),
        None => None,
    };
    let identity = load_identity(&args)?;
    let mut node = open_node(&args)?;
    node.set_identity(identity);
    match node.publish_article(&Draft { data, title, reply_to, time: now() }) {
        Done::Published { seq, blob } => {
            if args.get("--relay").is_none() {
                eprintln!("terse: no --relay: stored locally only");
            }
            println!("seq {seq} blob {}", to_hex(&blob));
            Ok(ExitCode::SUCCESS)
        }
        Done::Refused => Err("the relay refused the entry (does it already hold a different entry with this seq?)".into()),
        _ => Err("publish failed (store error, or the relay did not answer)".into()),
    }
}

fn subscribe(args: &[String], add: bool) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let key = parse_feed_key(args.one_positional()?)?;
    let mut subs = load_subscriptions(&args)?;
    let changed = if add { subs.add(key) } else { subs.remove(&key) }.map_err(|e| e.to_string())?;
    match (add, changed) {
        (true, true) => println!("subscribed to {}", to_hex(&key)),
        (true, false) => println!("already subscribed to {}", to_hex(&key)),
        (false, true) => println!("unsubscribed from {}", to_hex(&key)),
        (false, false) => return Err(format!("not subscribed to {}", to_hex(&key))),
    }
    Ok(ExitCode::SUCCESS)
}

fn subscriptions(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    let node = open_node(&args)?;
    for key in node.subscriptions().feeds() {
        let seqs = node.feed_store().seqs(key).map_err(|e| e.to_string())?;
        let latest = seqs.last().map_or("-".into(), |s| format!("#{s}"));
        println!("{}  {} entries held, latest {latest}", to_hex(key), seqs.len());
    }
    Ok(ExitCode::SUCCESS)
}

fn article_label(article: Option<Article>) -> &'static str {
    match article {
        Some(Article::Local) => "local",
        Some(Article::NotFetched) | None => "remote",
        Some(Article::Failed) => "failed",
        Some(Article::SizeMismatch) => "size-mismatch",
    }
}

/// Text from the network with control characters (other than newline and
/// tab) replaced, so it cannot drive the terminal.
fn printable(text: &str, keep_lines: bool) -> String {
    text.chars()
        .map(|c| if c.is_control() && !(keep_lines && (c == '\n' || c == '\t')) { '\u{FFFD}' } else { c })
        .collect()
}

/// `YYYY-MM-DD HH:MM` in UTC.
fn utc(time: u32) -> String {
    let days = (time / 86_400) as i64;
    let secs = time % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", secs / 3600, secs / 60 % 60)
}

/// One line per entry: time (UTC), feed key prefix, seq, size, article
/// state, title.
fn entry_line(entry: &Entry, article: Option<Article>) -> String {
    format!(
        "{}  {}  #{:<4} {:>8} B  {:<13} {}",
        utc(entry.time),
        &to_hex(&entry.feed)[..8],
        entry.seq,
        entry.size,
        article_label(article),
        printable(&entry.title, false)
    )
}

fn sync(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--watch"])?;
    known(&args, &["--watch", "--auto-download"])?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    let watch = args.has("--watch");
    let relay = args.require("--relay")?.to_string();
    let node = open_node(&args)?;
    if node.subscriptions().is_empty() {
        return Err("no subscriptions (add one with `terse feed subscribe <key>`)".into());
    }
    let mut service = ContentService::start(node);
    let deadline = Instant::now() + SYNC_DEADLINE;
    let mut lost = false;
    loop {
        let wait = if watch { Duration::from_secs(3600) } else { deadline.saturating_duration_since(Instant::now()) };
        let Some(event) = service.wait_feed_event(wait) else {
            if !watch && Instant::now() >= deadline {
                return Err(format!("{relay}: not caught up after {} s", SYNC_DEADLINE.as_secs()));
            }
            continue;
        };
        match event {
            FeedEvent::Entry { entry, article } => println!("{}", entry_line(&entry, Some(article))),
            // Every feed caught up and every download done.
            FeedEvent::Settled if !watch => return Ok(ExitCode::SUCCESS),
            FeedEvent::Settled | FeedEvent::Busy => {}
            FeedEvent::Connected => {
                if lost {
                    eprintln!("terse: reconnected to {relay}");
                }
            }
            FeedEvent::Disconnected { retry_in } => {
                if !watch {
                    return Err(format!("{relay}: cannot reach the relay, or the connection was lost"));
                }
                lost = true;
                eprintln!("terse: {relay}: not connected; retrying in {} s", retry_in.as_secs());
            }
            FeedEvent::SubscribeRefused(key) => eprintln!("terse: the relay refused a subscription to {}", to_hex(&key)),
            FeedEvent::Subscribed(_) | FeedEvent::Published { .. } | FeedEvent::PublishFailed(_) => {}
        }
    }
}

fn list(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let node = Node::open(args.get("--store").map(PathBuf::from), None, DEFAULT_TIMEOUT)?;
    let feeds = match args.positional.as_slice() {
        [] => node.feed_store().feeds().map_err(|e| e.to_string())?,
        [key] => vec![parse_feed_key(key)?],
        _ => return Err(USAGE.into()),
    };
    let mut entries = Vec::new();
    for feed in &feeds {
        entries.extend(node.feed_store().entries(feed).map_err(|e| e.to_string())?.into_iter().map(|e| e.entry));
    }
    // Newest first: by the authors' claimed times, then by seq.
    entries.sort_by(|a, b| (b.time, b.seq, b.feed).cmp(&(a.time, a.seq, a.feed)));
    for entry in &entries {
        println!("{}", entry_line(entry, node.held_article(entry)));
    }
    Ok(ExitCode::SUCCESS)
}

fn read_article(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let [key, seq] = args.positional.as_slice() else { return Err(USAGE.into()) };
    let key = parse_feed_key(key)?;
    let seq: u32 = seq.trim_start_matches('#').parse().map_err(|_| format!("bad seq {seq}"))?;
    let mut node = Node::open(args.get("--store").map(PathBuf::from), args.get("--relay"), DEFAULT_TIMEOUT)?;
    let bytes = node.feed_store().get(&key, seq).map_err(|e| e.to_string())?;
    let entry = bytes
        .and_then(|b| entry::decode(&b).ok())
        .filter(|e| e.entry.feed == key && e.entry.seq == seq)
        .ok_or(format!("no entry #{seq} of feed {} held locally (run `terse feed sync`)", to_hex(&key)))?
        .entry;
    // Fetched on demand if it was not downloaded automatically.
    let article = node.fetch_bytes(entry.blob)?;
    if article.len() != entry.size as usize {
        return Err(format!(
            "article {} is {} bytes but the entry says {}; it is not this entry's article",
            to_hex(&entry.blob),
            article.len(),
            entry.size
        ));
    }
    let mut text = printable(&String::from_utf8_lossy(&article), true);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    print!("{text}");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00");
        assert_eq!(utc(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00");
        assert_eq!(utc(u32::MAX), "2106-02-07 06:28");
    }

    #[test]
    fn control_characters_are_replaced() {
        assert_eq!(printable("a\x1b[2Jb\nc\t", false), "a\u{FFFD}[2Jb\u{FFFD}c\u{FFFD}");
        assert_eq!(printable("a\x1b\nc\t", true), "a\u{FFFD}\nc\t");
    }
}
