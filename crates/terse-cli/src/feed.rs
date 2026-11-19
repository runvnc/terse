//! `terse key`, `terse feed`, `terse profile`, `terse find` and `terse
//! relay` (NETWORK 4, 5).
//!
//! The identity (`identity.key`), the subscription list (`subscriptions`),
//! the nicknames (`nicknames`) and the relay list (`relays`) live in the
//! config directory, `--config` (default `$XDG_CONFIG_HOME/terse`,
//! else `~/.config/terse`). Feed entries are kept with the blobs, in
//! `<store>/feeds`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terse_net::entry::{self, MAX_TITLE};
use terse_net::identity::valid_feed_key;
use terse_net::address::parse_address;
use terse_net::profile::{self, Nicknames};
use terse_net::protocol::{SCOPE_ENTRIES, SCOPE_FEEDS, SCOPE_PROGRAMS};
use terse_net::{
    Article, ContentService, Done, Draft, Entry, FeedEvent, FeedKey, Identity, Node, Subscriptions,
    default_config_dir, parse_hex, to_hex,
};

use crate::{Args, USAGE, read, relay_urls};

/// `feed sync` without `--watch` gives up if not caught up (NETWORK 4.3)
/// after this long.
pub const SYNC_DEADLINE: Duration = Duration::from_secs(60);

/// Every `key` and `feed` command accepts these, used or not.
const OPTIONS: [&str; 4] = ["--config", "--relay", "--store", "--no-feed-relays"];

fn known(args: &Args, extra: &[&str]) -> Result<(), String> {
    args.check_known(&[&OPTIONS[..], extra].concat())
}

fn config_dir(args: &Args) -> Result<PathBuf, String> {
    args.get("--config").map(PathBuf::from).or_else(default_config_dir).ok_or("no config directory (use --config)".into())
}

pub fn identity_path(args: &Args) -> Result<PathBuf, String> {
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

pub fn load_nicknames(args: &Args) -> Result<Nicknames, String> {
    Nicknames::load_from_config(&config_dir(args)?)
}

/// A node on `--store` and `relays` with the saved subscriptions and
/// nicknames.
fn open_node(args: &Args, relays: &[String]) -> Result<Node, String> {
    let mut node = crate::open_node(args, relays)?;
    node.set_subscriptions(load_subscriptions(args)?);
    node.set_nicknames(load_nicknames(args)?);
    if let Some(n) = args.get("--auto-download") {
        node.set_auto_download_limit(n.parse().map_err(|_| format!("bad byte count {n}"))?);
    }
    node.set_prefetch(!args.has("--no-prefetch"));
    node.set_program_download(!args.has("--no-program-download"));
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
        "address" => address(rest),
        "sync" => sync(rest),
        "list" => list(rest),
        "programs" => programs(rest),
        "read" => read_article(rest),
        "nickname" => nickname(rest),
        _ => Err(USAGE.into()),
    }
}

/// `terse relay add|remove|set <url>`, `terse relay list|show`: the
/// relay list in the config directory's `relays` file (NETWORK 5).
/// `terse manual build|check [--dir <d>]`: resolves the manual's links and
/// writes (or checks) `<d>/build`; `terse manual publish`: publishes the
/// manual built into terse as article entries of the user's feed, the
/// articles linked to before those that link to them, so the index last.
pub fn manual(args: &[String]) -> Result<ExitCode, String> {
    use terse_net::manual;
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let args = Args::parse(rest, &[])?;
    match command.as_str() {
        "build" | "check" => {
            args.check_known(&["--dir"])?;
            if !args.positional.is_empty() {
                return Err(USAGE.into());
            }
            let dir = PathBuf::from(args.get("--dir").unwrap_or("docs/manual"));
            if command == "build" {
                for b in manual::build_dir(&dir)? {
                    println!("{} {:6} {}", to_hex(&b.id), b.text.len(), b.name);
                }
                return Ok(ExitCode::SUCCESS);
            }
            let built = manual::build(&manual::read_sources(&dir)?)?;
            let out = dir.join("build");
            let mut stale = Vec::new();
            for b in &built {
                if std::fs::read(out.join(format!("{}.txt", b.name))).ok().as_deref() != Some(b.text.as_bytes()) {
                    stale.push(b.name.clone());
                }
            }
            if std::fs::read_to_string(out.join("manifest")).ok() != Some(manual::build_manifest(&built)) {
                stale.push("manifest".into());
            }
            if stale.is_empty() {
                println!("the built manual is up to date ({} articles)", built.len());
                Ok(ExitCode::SUCCESS)
            } else {
                Err(format!("out of date in {}: {} (run `terse manual build`)", out.display(), stale.join(", ")))
            }
        }
        "publish" => {
            known(&args, &[])?;
            if !args.positional.is_empty() {
                return Err(USAGE.into());
            }
            let identity = load_identity(&args)?;
            let relays = relay_urls(&args, true)?;
            let mut node = open_node(&args, &relays)?;
            node.set_identity(identity);
            if relays.is_empty() {
                eprintln!("terse: no relay: stored locally only");
            }
            for (name, title, text) in terse_net::builtin::MANUAL.iter().rev() {
                let draft = Draft { data: text.to_vec(), title: title.to_string(), reply_to: None, time: now() };
                match node.publish_article(&draft) {
                    Done::Published { seq, blob } => println!("seq {seq} blob {} {name}", to_hex(&blob)),
                    Done::Refused => return Err(format!("{name}: the relays refused the entry")),
                    _ => return Err(format!("{name}: publish failed (store error, or no relay accepted it)")),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(USAGE.into()),
    }
}

pub fn relay(args: &[String]) -> Result<ExitCode, String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let args = Args::parse(rest, &[])?;
    known(&args, &[])?;
    let config = config_dir(&args)?;
    let mut list = terse_net::local::read_relays(&config)?.map(|(l, _)| l).unwrap_or_default();
    let save = |list: &[String]| -> Result<(), String> {
        let path = terse_net::local::write_relays(&config, list)?;
        eprintln!("terse: relays saved in {}", path.display());
        Ok(())
    };
    match (command.as_str(), args.positional.as_slice()) {
        ("set", [url]) => {
            save(std::slice::from_ref(url))?;
            println!("{url}");
        }
        ("add", [url]) => {
            if list.contains(url) {
                return Err(format!("{url} is already in the list"));
            }
            list.push(url.clone());
            save(&list)?;
        }
        ("remove", [url]) => {
            let before = list.len();
            list.retain(|u| u != url);
            if list.len() == before {
                return Err(format!("{url} is not in the list"));
            }
            save(&list)?;
        }
        ("list" | "show", []) if list.is_empty() => {
            let port = terse_net::local::local_port();
            eprintln!(
                "terse: no relays listed in {}; commands use {} if a relay runs there",
                config.join(terse_net::local::RELAYS_FILE).display(),
                terse_net::local::local_url(port)
            );
            return Ok(ExitCode::from(1));
        }
        ("list" | "show", []) => {
            for url in &list {
                println!("{url}");
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(ExitCode::SUCCESS)
}

/// The name to show beside a feed key: the nickname, else the declared
/// name (NETWORK 4.6), with control characters replaced.
fn name_of(node: &Node, key: &FeedKey) -> Option<String> {
    node.display_name(key).map(|(n, _)| printable(&n, false))
}

fn now() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

fn publish(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &["--title", "--reply-to", "--program", "--name"])?;
    if args.has("--program") {
        return publish_program(&args);
    }
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
    let relays = relay_urls(&args, true)?;
    let mut node = open_node(&args, &relays)?;
    node.set_identity(identity);
    match node.publish_article(&Draft { data, title, reply_to, time: now() }) {
        Done::Published { seq, blob } => {
            if relays.is_empty() {
                eprintln!("terse: no relay: stored locally only");
            }
            println!("seq {seq} blob {}", to_hex(&blob));
            Ok(ExitCode::SUCCESS)
        }
        Done::Refused => Err("the relays refused the entry (do they already hold a different entry with this seq?)".into()),
        _ => Err("publish failed (store error, or no relay accepted it)".into()),
    }
}

/// `terse feed publish --program <image> [--name <n>]`: a program entry
/// (NETWORK 4.2) for a valid image, named by default by its title.
fn publish_program(args: &Args) -> Result<ExitCode, String> {
    if !args.positional.is_empty() || args.has("--title") || args.has("--reply-to") {
        return Err(USAGE.into());
    }
    let path = args.require("--program")?;
    let image = read(path)?;
    let parsed = terse_core::Image::parse(&image).map_err(|e| format!("{path}: not a Terse image: {e}"))?;
    let name = match args.get("--name") {
        Some(n) => n.to_string(),
        None => String::from_utf8(parsed.title.clone()).map_err(|_| format!("{path}: the title is not UTF-8; give --name"))?,
    };
    if !profile::valid_name(&name) {
        return Err(format!("\"{}\" {BAD_NAME}", printable(&name, false)));
    }
    let identity = load_identity(args)?;
    let relays = relay_urls(args, true)?;
    let mut node = open_node(args, &relays)?;
    node.set_identity(identity);
    match node.publish_program(&image, &name, now()) {
        Done::Published { seq, blob } => {
            if relays.is_empty() {
                eprintln!("terse: no relay: stored locally only");
            }
            println!("seq {seq} program {name} image {}", to_hex(&blob));
            Ok(ExitCode::SUCCESS)
        }
        Done::Refused => Err("the relays refused the entry (do they already hold a different entry with this seq?)".into()),
        _ => Err("publish failed (store error, or no relay accepted it)".into()),
    }
}

/// `terse feed programs [<key>]`: the current version of each program held
/// (NETWORK 4.2), of one feed or of every feed held.
fn programs(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let node = open_node(&args, &[])?;
    let feeds = match args.positional.as_slice() {
        [] => node.feed_store().feeds().map_err(|e| e.to_string())?,
        [key] => vec![parse_feed_key(key)?],
        _ => return Err(USAGE.into()),
    };
    for feed in &feeds {
        let name = name_of(&node, feed);
        for s in node.current_programs(feed) {
            println!("{}", entry_line(&s.entry, node.held_article(&s.entry), name.as_deref()));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `terse feed subscribe|unsubscribe <address>`: a feed key, or an address
/// with hints (NETWORK 4.9), also as a `terse:feed/` link. The hints are
/// remembered with the subscription (replacing earlier ones).
fn subscribe(args: &[String], add: bool) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let text = args.one_positional()?;
    let address = parse_address(text).map_err(|e| format!("{}: {e}", printable(text.trim(), false)))?;
    let key = address.key;
    let mut subs = load_subscriptions(&args)?;
    let changed = if add { subs.add(key) } else { subs.remove(&key) }.map_err(|e| e.to_string())?;
    let hinted = add && !address.hints.is_empty();
    let new_hints = hinted && subs.set_hints(&key, Some(&address.hints)).map_err(|e| e.to_string())?;
    match (add, changed) {
        (true, true) => println!("subscribed to {}", to_hex(&key)),
        (true, false) if new_hints => println!("already subscribed to {}; its hints are now {}", to_hex(&key), address.hints),
        (true, false) => println!("already subscribed to {}", to_hex(&key)),
        (false, true) => println!("unsubscribed from {}", to_hex(&key)),
        (false, false) => return Err(format!("not subscribed to {}", to_hex(&key))),
    }
    if hinted {
        let hints = address.hint_list();
        let shown: Vec<String> = hints.iter().map(|h| h.to_string()).collect();
        println!("if your relays do not hold it, it is looked for on {}", shown.join(" and "));
        for h in &hints {
            if !profile::usable_profile_relay(&h.url()) {
                eprintln!("terse: {h} is a loopback, private or link-local address: that hint will not be used (NETWORK 5.1)");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `terse feed address`: the user's own feed address (NETWORK 4.9), with
/// hints from the profile's relay lines, then the relays in use (the
/// --relay options, else the relay list), public ones only.
fn address(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    let identity = load_identity(&args)?;
    let relays = relay_urls(&args, false)?;
    let relays = if relays.is_empty() {
        terse_net::local::read_relays(&config_dir(&args)?)?.map(|(l, _)| l).unwrap_or_default()
    } else {
        relays
    };
    let mut node = open_node(&args, &relays)?;
    node.set_identity(identity);
    let hints = terse_net::address::hint_text(&node.own_hints());
    let key = to_hex(&node.identity_key().unwrap());
    if hints.is_empty() {
        println!("{key}");
        eprintln!(
            "terse: no public relay in your profile or relay list, so your address has no hints: only people who use the same relay as you will find your feed"
        );
    } else {
        println!("{key}@{hints}");
    }
    Ok(ExitCode::SUCCESS)
}

fn subscriptions(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    let node = open_node(&args, &[])?;
    for key in node.subscriptions().feeds() {
        let seqs = node.feed_store().seqs(key).map_err(|e| e.to_string())?;
        let latest = seqs.last().map_or("-".into(), |s| format!("#{s}"));
        let name = name_of(&node, key).map_or(String::new(), |n| format!("{n}  "));
        // NETWORK 4.9: the hints, unless the profile's relay lines replace them.
        let hints = match node.subscriptions().hints(key) {
            None => String::new(),
            Some(h) if node.profile(key).and_then(|(_, d)| d).is_some_and(|d| !d.relays.is_empty()) => {
                format!("  hints {h} (not used: the profile names relays)")
            }
            Some(h) => format!("  hints {h}"),
        };
        println!("{}  {name}{} entries held, latest {latest}{hints}", to_hex(key), seqs.len());
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

/// One line per entry: time (UTC), feed key prefix (and the feed's name,
/// if known), seq, size, article state, title (an article's as it is, the
/// others' marked with their kind: `[profile: …]`, `[program: …]`,
/// `[document: …]`, `[kind 9: …]` for a kind this version does not define).
fn entry_line(entry: &Entry, article: Option<Article>, name: Option<&str>) -> String {
    let title = printable(&entry.title, false);
    format!(
        "{}  {}{}  #{:<4} {:>8} B  {:<13} {}",
        utc(entry.time),
        &to_hex(&entry.feed)[..8],
        name.map_or(String::new(), |n| format!(" ({n})")),
        entry.seq,
        entry.size,
        article_label(article),
        match entry.kind {
            terse_net::entry::KIND_PROFILE => format!("[profile: {title}]"),
            terse_net::entry::KIND_PROGRAM => format!("[program: {title}]"),
            terse_net::entry::KIND_DOCUMENT => format!("[document: {title}]"),
            terse_net::entry::KIND_ARTICLE => title,
            // A kind this revision does not define (NETWORK 6): listed, not
            // opened.
            kind => format!("[kind {kind}: {title}]"),
        }
    )
}

fn sync(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--watch", "--no-prefetch", "--no-program-download"])?;
    known(&args, &["--watch", "--auto-download", "--no-prefetch", "--no-program-download"])?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    let watch = args.has("--watch");
    let relays = relay_urls(&args, true)?;
    if relays.is_empty() {
        return Err("no relay: give --relay, add one with `terse relay add <url>`, or start terse-relay on this machine".into());
    }
    let relay = relays.join(" ");
    let node = open_node(&args, &relays)?;
    if node.subscriptions().is_empty() {
        return Err("no subscriptions (add one with `terse feed subscribe <key>`)".into());
    }
    let subscribed: Vec<FeedKey> = node.subscriptions().feeds().copied().collect();
    let hinted: Vec<FeedKey> = node.subscriptions().hinted().map(|(k, _)| *k).collect();
    let feed_relays = !args.has("--no-feed-relays");
    let store_dir = node.store().dir().to_path_buf();
    let mut service = ContentService::start(node);
    // NETWORK 4.9: say which feeds no relay held, after a sync.
    let report = || {
        let feeds = terse_net::FeedStore::beside_blobs(&store_dir);
        for key in &subscribed {
            if feeds.highest(key).unwrap_or(0) > 0 {
                continue;
            }
            if hinted.contains(key) && feed_relays {
                eprintln!("terse: {}: not found on your relays nor on the relays its address names", to_hex(key));
            } else if hinted.contains(key) {
                eprintln!("terse: {}: not found on your relays (its hints are not used with --no-feed-relays)", to_hex(key));
            } else {
                eprintln!(
                    "terse: {}: not found on your relays; if its author publishes elsewhere, subscribe with the feed's address (<key>@<relay>)",
                    to_hex(key)
                );
            }
        }
    };
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
            FeedEvent::Entry { entry, article } => println!("{}", entry_line(&entry, Some(article), None)),
            FeedEvent::ProgramDownloaded { entry, article } => {
                println!("{}  (downloaded)", entry_line(&entry, Some(article), None));
            }
            // Every feed caught up and every download done.
            FeedEvent::Settled if !watch => {
                report();
                return Ok(ExitCode::SUCCESS);
            }
            FeedEvent::Settled | FeedEvent::Busy => {}
            FeedEvent::Connected(url) => {
                if lost {
                    eprintln!("terse: connected to {url}");
                }
            }
            FeedEvent::Unreachable if !watch => {
                return Err(format!("{relay}: cannot reach any relay, or the connections were lost"));
            }
            FeedEvent::Unreachable => {}
            FeedEvent::Disconnected { url, retry_in } => {
                lost = true;
                eprintln!("terse: {url}: not connected; retrying in {} s", retry_in.as_secs());
            }
            FeedEvent::SubscribeRefused(key) => eprintln!("terse: the relay refused a subscription to {}", to_hex(&key)),
            FeedEvent::Subscribed(_) | FeedEvent::Published { .. } | FeedEvent::PublishFailed(_) => {}
        }
    }
}

fn list(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let node = open_node(&args, &[])?;
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
    let names: std::collections::HashMap<FeedKey, Option<String>> = feeds.iter().map(|f| (*f, name_of(&node, f))).collect();
    for entry in &entries {
        println!("{}", entry_line(entry, node.held_article(entry), names[&entry.feed].as_deref()));
    }
    Ok(ExitCode::SUCCESS)
}

fn read_article(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let [key, seq] = args.positional.as_slice() else { return Err(USAGE.into()) };
    let key = parse_feed_key(key)?;
    let seq: u32 = seq.trim_start_matches('#').parse().map_err(|_| format!("bad seq {seq}"))?;
    let relays = relay_urls(&args, true)?;
    let mut node = open_node(&args, &relays)?;
    let bytes = node.feed_store().get(&key, seq).map_err(|e| e.to_string())?;
    let entry = bytes
        .and_then(|b| entry::decode(&b).ok())
        .filter(|e| e.entry.feed == key && e.entry.seq == seq)
        .ok_or(format!("no entry #{seq} of feed {} held locally (run `terse feed sync`)", to_hex(&key)))?
        .entry;
    if !terse_net::entry::known_kind(entry.kind) {
        return Err(format!("entry #{seq} is of kind {}, which this version cannot open", entry.kind));
    }
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

const BAD_NAME: &str = "is not a valid name (1 to 48 bytes, no control characters, no space at either end)";

/// `terse feed nickname <key> [<name>]`: sets or (without a name) removes
/// the local nickname of a feed (NETWORK 4.6).
fn nickname(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let (key, name) = match args.positional.as_slice() {
        [key] => (key, None),
        [key, name] => (key, Some(name.as_str())),
        _ => return Err(USAGE.into()),
    };
    let key = parse_feed_key(key)?;
    if let Some(n) = name.filter(|n| !profile::valid_name(n)) {
        return Err(format!("\"{}\" {BAD_NAME}", printable(n, false)));
    }
    let mut nicknames = load_nicknames(&args)?;
    let changed = nicknames.set(key, name).map_err(|e| e.to_string())?;
    match (name, changed) {
        (Some(n), _) => println!("{}  {n}", to_hex(&key)),
        (None, true) => println!("{}  nickname removed", to_hex(&key)),
        (None, false) => return Err(format!("no nickname for {}", to_hex(&key))),
    }
    Ok(ExitCode::SUCCESS)
}

/// `terse profile set|show` (NETWORK 4.6).
pub fn profile(args: &[String]) -> Result<ExitCode, String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    match command.as_str() {
        "set" => profile_set(rest),
        "show" => profile_show(rest),
        _ => Err(USAGE.into()),
    }
}

fn profile_set(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--listed", "--unlisted"])?;
    known(&args, &["--name", "--listed", "--unlisted", "--description"])?;
    if !args.positional.is_empty() || (args.has("--listed") && args.has("--unlisted")) {
        return Err(USAGE.into());
    }
    let name = args.require("--name")?;
    if !profile::valid_name(name) {
        return Err(format!("\"{}\" {BAD_NAME}", printable(name, false)));
    }
    let description = match args.get("--description") {
        Some(path) => Some(String::from_utf8(read(path)?).map_err(|_| format!("{path}: not UTF-8"))?),
        None => None,
    };
    let identity = load_identity(&args)?;
    let relays = relay_urls(&args, true)?;
    let mut node = open_node(&args, &relays)?;
    let current = node.profile(&identity.feed_key()).and_then(|(_, d)| d);
    let listed = args.has("--listed") || (!args.has("--unlisted") && current.as_ref().is_some_and(|d| d.listed));
    node.set_identity(identity);
    match node.publish_profile(name, listed, description, now()) {
        Done::Published { seq, blob } => {
            if relays.is_empty() {
                eprintln!("terse: no relay: stored locally only");
            }
            println!("seq {seq} blob {} ({})", to_hex(&blob), if listed { "listed" } else { "unlisted" });
            Ok(ExitCode::SUCCESS)
        }
        Done::Refused => Err("the relays refused the profile entry".into()),
        _ => Err("publishing the profile failed (store error, or no relay accepted it)".into()),
    }
}

fn profile_show(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    known(&args, &[])?;
    let node = open_node(&args, &[])?;
    let key = match args.positional.as_slice() {
        [] => load_identity(&args)?.feed_key(),
        [key] => parse_feed_key(key)?,
        _ => return Err(USAGE.into()),
    };
    println!("feed:        {}", to_hex(&key));
    if let Some(n) = node.nicknames().get(&key) {
        println!("nickname:    {}", printable(n, false));
    }
    let Some((entry, doc)) = node.profile(&key) else {
        println!("profile:     none held (subscribe to the feed and run `terse feed sync`)");
        return Ok(ExitCode::SUCCESS);
    };
    println!("name:        {}", printable(&entry.title, false));
    println!("profile:     #{} of {}, blob {}", entry.seq, utc(entry.time), to_hex(&entry.blob));
    match doc {
        None => println!("document:    not held, or not a profile document"),
        Some(d) => {
            println!("listed:      {}", if d.listed { "yes" } else { "no" });
            for r in &d.relays {
                println!("relay:       {}", printable(r, false));
            }
            if !d.description.is_empty() {
                println!("\n{}", printable(&d.description, true).trim_end());
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `terse find [--entries|--programs] <text>`: asks every relay (NETWORK
/// 4.7).
pub fn find(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--entries", "--programs"])?;
    known(&args, &["--entries", "--programs"])?;
    if args.has("--entries") && args.has("--programs") {
        return Err(USAGE.into());
    }
    let text = args.one_positional()?;
    if !(1..=profile::MAX_NAME).contains(&text.len()) {
        return Err("the text to find is 1 to 48 bytes".into());
    }
    let relays = relay_urls(&args, true)?;
    if relays.is_empty() {
        return Err("no relay to ask: give --relay, or add one with `terse relay add <url>`".into());
    }
    let mut node = open_node(&args, &relays)?;
    let scope = match (args.has("--entries"), args.has("--programs")) {
        (true, _) => SCOPE_ENTRIES,
        (_, true) => SCOPE_PROGRAMS,
        _ => SCOPE_FEEDS,
    };
    let Done::Found(found) = node.find(scope, text.as_bytes()) else {
        return Err("no relay answered".into());
    };
    for s in &found {
        if scope == SCOPE_FEEDS {
            let nick = node.nicknames().get(&s.entry.feed);
            let nick = nick.map_or(String::new(), |n| format!("  (your nickname: {})", printable(n, false)));
            println!("{}  {}{nick}", to_hex(&s.entry.feed), printable(&s.entry.title, false));
        } else {
            let name = name_of(&node, &s.entry.feed);
            println!("{}", entry_line(&s.entry, node.held_article(&s.entry), name.as_deref()));
        }
    }
    if found.is_empty() {
        eprintln!("terse: nothing found");
    }
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
