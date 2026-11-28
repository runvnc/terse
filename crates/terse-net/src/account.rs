//! What a person does with their own side of Tersenet: their identity
//! (NETWORK 4.1), profile (4.6), feed (4.2), the feeds they follow (4.4,
//! 4.9), search (4.7) and their relay list (5). `terse key`, `terse
//! profile`, `terse feed`, `terse find` and `terse relay` and terse-host's
//! own screens both do these things through this module, so the two say
//! and do the same.
//!
//! What an operation reports as it goes is handed to a `say` function as
//! [`Said`]: a result line (the command line prints it on standard
//! output) or a note (standard error, after `terse: `).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::address::parse_address;
use crate::entry::MAX_TITLE;
use crate::links::{self, Keep};
use crate::profile;
use crate::{BlobId, Delivery, Done, Draft, FeedKey, Identity, Node, SignedEntry, Subscriptions, to_hex};

/// The identity's file in the config directory.
pub const IDENTITY_FILE: &str = "identity.key";
/// The subscription list's file in the config directory.
pub const SUBSCRIPTIONS_FILE: &str = "subscriptions";

/// Why a name is refused (NETWORK 4.6).
pub const BAD_NAME: &str = "is not a valid name (1 to 48 bytes, no control characters, no space at either end)";

/// What an address without hints means (NETWORK 4.9).
pub const NO_HINTS: &str = "no public encrypted (wss://) relay in your profile or relay list, so your address has no hints (plain ws:// relays are never given out): only people who use the same relay as you will find your feed";

/// A line an operation reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// A result (standard output on the command line).
    Out(String),
    /// A note or a warning (standard error, after `terse: `).
    Note(String),
}

/// Seconds since 1970, as entries are dated.
pub fn now() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

/// `text` with control characters replaced by U+FFFD (newlines and tabs
/// kept if `keep_lines`): text from the network, made safe to print.
pub fn printable(text: &str, keep_lines: bool) -> String {
    text.chars().map(|c| if c.is_control() && !(keep_lines && (c == '\n' || c == '\t')) { '\u{FFFD}' } else { c }).collect()
}

/// A feed key shortened as readers show it: its first and last six hex digits.
pub fn short_key(key: &FeedKey) -> String {
    let hex = to_hex(key);
    format!("{}…{}", &hex[..6], &hex[58..])
}

pub fn identity_path(config: &Path) -> PathBuf {
    config.join(IDENTITY_FILE)
}

/// The identity in the config directory.
pub fn load_identity(config: &Path) -> Result<Identity, String> {
    let path = identity_path(config);
    if !path.exists() {
        return Err(format!("no identity in {} (create one with `terse key new`)", path.display()));
    }
    Identity::load(&path)
}

/// Makes a new identity in the config directory; never replaces one.
pub fn create_identity(config: &Path) -> Result<Identity, String> {
    Identity::create(&identity_path(config))
}

/// Takes the secret key in `from` (a key file as `create_identity`
/// writes it) as the identity in the config directory: checked, then
/// written as a new file with the owner's permissions only. Never
/// replaces an identity.
pub fn import_identity(config: &Path, from: &Path) -> Result<Identity, String> {
    let path = identity_path(config);
    if path.exists() {
        return Err(format!("{}: an identity already exists; not overwriting it", path.display()));
    }
    let given = Identity::load(from)?;
    given.save_new(&path)?;
    Ok(given)
}

/// The subscription list in the config directory.
pub fn load_subscriptions(config: &Path) -> Result<Subscriptions, String> {
    Subscriptions::load(config.join(SUBSCRIPTIONS_FILE))
}

/// After a put or publish: which relays took it, and a warning for each
/// that did not (it succeeds when one does, NETWORK 5.1).
pub fn delivery(node: &mut Node, say: &mut dyn FnMut(Said)) {
    let delivery = node.take_delivery();
    let taken: Vec<&str> = delivery.iter().filter(|(_, d)| *d == Delivery::Accepted).map(|(u, _)| u.as_str()).collect();
    if !taken.is_empty() {
        say(Said::Note(format!("taken by {} of {} relays: {}", taken.len(), delivery.len(), taken.join(", "))));
    }
    for (url, d) in &delivery {
        let why = match d {
            Delivery::Accepted => continue,
            // NETWORK 3.1: the relay's reason, in words.
            Delivery::Refused(why) => format!("refused it: {}", why.describe()),
            Delivery::BlobRefused(why) => {
                say(Said::Note(format!("warning: {url} took the entry but refused its blob: {}", why.describe())));
                continue;
            }
            Delivery::Failed => "did not answer (connection lost or timed out)".into(),
            Delivery::NotConnected => match node.relay_failure(url) {
                Some(why) => format!("could not be connected: {why}"),
                None => "could not be connected".into(),
            },
        };
        say(Said::Note(format!("warning: not on {url}: it {why}")));
    }
}

/// The relays a profile published through `node` lists (NETWORK 4.6):
/// its encrypted relays that are not this machine's.
pub fn profile_relays(relays: &[String]) -> Vec<String> {
    relays.iter().filter(|u| profile::valid_relay_line(u) && !profile::loopback_url(u)).cloned().collect()
}

/// The plain relays a profile leaves out that are worth naming (this
/// machine's go without a word).
pub fn plain_left_out(relays: &[String]) -> Vec<String> {
    relays.iter().filter(|u| u.starts_with("ws://") && !profile::loopback_url(u)).cloned().collect()
}

/// What a published entry was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Published {
    pub seq: u32,
    pub blob: BlobId,
}

/// Publishes the profile of `identity` through `node` (NETWORK 4.6):
/// `listed` `None` keeps what the profile in force says (not listed if
/// there is none); `description` `None` keeps its description.
pub fn publish_profile(
    node: &mut Node,
    identity: Identity,
    name: &str,
    listed: Option<bool>,
    description: Option<String>,
    say: &mut dyn FnMut(Said),
) -> Result<(Published, bool), String> {
    if !profile::valid_name(name) {
        return Err(format!("\"{}\" {BAD_NAME}", printable(name, false)));
    }
    let current = node.profile(&identity.feed_key()).and_then(|(_, d)| d);
    let listed = listed.unwrap_or(current.as_ref().is_some_and(|d| d.listed));
    node.set_identity(identity);
    let relays = node.relay_urls();
    // Plain relays are not listed (NETWORK 4.6); this machine's go
    // without a word, others are named.
    let plain = plain_left_out(&relays);
    if !plain.is_empty() {
        say(Said::Note(format!("not in the profile: {}: {}", plain.join(", "), profile::PLAIN_NOT_LISTED)));
    }
    let done = node.publish_profile(name, listed, description, now());
    delivery(node, say);
    match done {
        Done::Published { seq, blob } => {
            if relays.is_empty() {
                say(Said::Note("no relay: stored locally only".into()));
            }
            say(Said::Out(format!("seq {seq} blob {} ({})", to_hex(&blob), if listed { "listed" } else { "unlisted" })));
            Ok((Published { seq, blob }, listed))
        }
        Done::Refused => Err("every relay refused the profile entry (each one's reason is above)".into()),
        _ => Err("publishing the profile failed (store error, or no relay accepted it)".into()),
    }
}

/// The name to show beside a feed key: the nickname, else the declared
/// name (NETWORK 4.6), with control characters replaced.
pub fn name_of(node: &Node, key: &FeedKey) -> Option<String> {
    node.display_name(key).map(|(n, _)| printable(&n, false))
}

/// What publishing an article asks for.
pub struct Article {
    pub data: Vec<u8>,
    pub title: String,
    pub reply_to: Option<BlobId>,
    /// List the links and publish nothing.
    pub dry_run: bool,
    /// Publish even if the article links to a part of the user's feed
    /// this host does not hold.
    pub force: bool,
}

/// Checks an article before anything is done with it: its size and its
/// title's.
pub fn check_article(data: &[u8], title: &str) -> Result<(), String> {
    if !crate::blob::valid_size(data.len()) {
        return Err(format!("an article is 1 to {} bytes; this is {}", crate::blob::MAX_BLOB, data.len()));
    }
    if title.is_empty() {
        return Err("an article needs a title".into());
    }
    if title.len() > MAX_TITLE {
        return Err(format!("title is {} bytes, at most {MAX_TITLE} allowed", title.len()));
    }
    Ok(())
}

/// Publishes an article entry (NETWORK 4.2) through `node`, which has
/// the relays (none for a dry run). Each `terse:` link of the article is
/// said first, with what this host holds, what is uploaded with it and
/// what a relay keeps ([`crate::links`]); an article that links to a blob
/// of the user's feed that this host does not hold is refused unless
/// `force`. `identity` may be `None` only for a dry run (nothing then
/// counts as the user's own). `Ok(None)`: a dry run.
pub fn publish_article(node: &mut Node, identity: Option<Identity>, a: Article, say: &mut dyn FnMut(Said)) -> Result<Option<Published>, String> {
    check_article(&a.data, &a.title)?;
    let own = identity.as_ref().map(Identity::feed_key);
    let linked = links::survey(&a.data, node, own.as_ref());
    let names: std::collections::HashMap<FeedKey, String> = linked
        .iter()
        .filter_map(|l| match l.keep {
            Keep::Others { feed, .. } => Some((feed, name_of(node, &feed).unwrap_or_else(|| format!("feed {}", short_key(&feed))))),
            _ => None,
        })
        .collect();
    let name = |key: &FeedKey| names.get(key).cloned().unwrap_or_default();
    // The user's own parts (named by an entry of their feed) that this host
    // does not hold cannot go up with the article.
    let missing: Vec<String> = linked.iter().filter(|l| matches!(l.keep, Keep::Own { .. }) && !l.held).map(|l| l.link()).collect();
    let refusal = (!missing.is_empty() && !a.force).then(|| {
        let (what, them, it) = if missing.len() == 1 { ("an article", "it", "it is") } else { ("articles", "them", "they are") };
        format!(
            "not published: the article links to {what} of your feed that this host does not hold: {}, so {it} not \
             uploaded with it. Get {them} first (terse get <id>), or give --force if your relays hold {them}",
            missing.join(", ")
        )
    });
    if a.dry_run || refusal.is_some() {
        let would = if a.dry_run { "would be uploaded" } else { "not uploaded" };
        for l in &linked {
            say(Said::Out(l.line(l.upload.map(|_| would), name)));
        }
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        say(Said::Out("dry run: nothing uploaded or published".into()));
        return Ok(None);
    }
    let identity = identity.ok_or("no identity to publish with")?;
    // What is the user's own (or named by nothing) goes up first, so that
    // the article's links lead somewhere from the moment it is published.
    let mut lines = Vec::new();
    for l in &linked {
        let uploaded = match l.upload.map(|id| node.store().read(&id)) {
            None => None,
            Some(Ok(Some(bytes))) => Some(match (node.publish(&bytes), node.take_delivery()) {
                (Done::Put(_), d) if d.is_empty() => "stored here only (no relay)".to_string(),
                (Done::Put(_), d) => {
                    let taken = d.iter().filter(|(_, d)| *d == Delivery::Accepted).count();
                    format!("uploaded now (taken by {taken} of {} relays)", d.len())
                }
                _ => "upload failed (no relay took it)".to_string(),
            }),
            Some(_) => Some("not uploaded (it could not be read here)".to_string()),
        };
        lines.push(l.line(uploaded.as_deref(), name));
    }
    for line in lines {
        say(Said::Out(line));
    }
    node.set_identity(identity);
    let relays = node.relay_urls();
    let done = node.publish_article(&Draft { data: a.data, title: a.title, reply_to: a.reply_to, time: now() });
    delivery(node, say);
    match done {
        Done::Published { seq, blob } => {
            if relays.is_empty() {
                say(Said::Note("no relay: stored locally only".into()));
            }
            say(Said::Out(format!("seq {seq} blob {}", to_hex(&blob))));
            // The article's own parts that no entry names: an attachment
            // entry each (NETWORK 4.2), so relays keep them with it.
            for l in linked.iter().filter(|l| l.keep == Keep::Unnamed) {
                let Some(Ok(Some(bytes))) = l.upload.map(|id| node.store().read(&id)) else { continue };
                let done = node.publish_attachment(&bytes, blob, now());
                delivery(node, say);
                match done {
                    Done::Published { seq, .. } => say(Said::Out(format!("seq {seq} attachment {}", l.link()))),
                    _ => say(Said::Note(format!(
                        "warning: no attachment entry for {}: a relay that drops blobs no entry names (NETWORK.md 5.3) will drop it",
                        l.link()
                    ))),
                }
            }
            Ok(Some(Published { seq, blob }))
        }
        Done::Refused => Err("every relay refused the entry (each one's reason is above)".into()),
        _ => Err("publish failed (store error, or no relay accepted it)".into()),
    }
}

/// A title for an article: its first heading (`# `), else `fallback`,
/// cut to what a title may hold.
pub fn title_of(data: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(data);
    let heading = text.lines().find_map(|l| l.strip_prefix("# ").map(str::trim)).filter(|h| !h.is_empty());
    let mut t = heading.unwrap_or(fallback).to_string();
    while t.len() > MAX_TITLE {
        t.pop();
    }
    t
}

/// Follows (`add`) or stops following the feed of an address (NETWORK
/// 4.9: a key, maybe with hints, or a `terse:feed/` link), in the config
/// directory's subscription list, which a running host takes in. The
/// hints are remembered with the subscription (replacing earlier ones).
/// Returns the feed's key.
pub fn subscribe(config: &Path, text: &str, add: bool, say: &mut dyn FnMut(Said)) -> Result<FeedKey, String> {
    let address = parse_address(text).map_err(|e| format!("{}: {e}", printable(text.trim(), false)))?;
    let key = address.key;
    let mut subs = load_subscriptions(config)?;
    let changed = if add { subs.add(key) } else { subs.remove(&key) }.map_err(|e| e.to_string())?;
    let hinted = add && !address.hints.is_empty();
    let new_hints = hinted && subs.set_hints(&key, Some(&address.hints)).map_err(|e| e.to_string())?;
    match (add, changed) {
        (true, true) => say(Said::Out(format!("subscribed to {}", to_hex(&key)))),
        (true, false) if new_hints => say(Said::Out(format!("already subscribed to {}; its hints are now {}", to_hex(&key), address.hints))),
        (true, false) => say(Said::Out(format!("already subscribed to {}", to_hex(&key)))),
        (false, true) => say(Said::Out(format!("unsubscribed from {}", to_hex(&key)))),
        (false, false) => return Err(format!("not subscribed to {}", to_hex(&key))),
    }
    if hinted {
        let hints = address.hint_list();
        let shown: Vec<String> = hints.iter().map(|h| h.to_string()).collect();
        say(Said::Out(format!("if your relays do not hold it, it is looked for on {}", shown.join(" and "))));
        for h in &hints {
            if !profile::usable_profile_relay(&h.url()) {
                say(Said::Note(format!("{h} is a loopback, private or link-local address: that hint will not be used (NETWORK 5.1)")));
            }
        }
    }
    Ok(key)
}

/// The user's own feed address (NETWORK 4.9): the key, then `@` and the
/// hints from the profile's relay lines and the node's relays, public
/// ones only; and whether it has hints.
pub fn own_address(node: &Node) -> Option<(String, bool)> {
    let key = to_hex(&node.identity_key()?);
    let hints = crate::address::hint_text(&node.own_hints());
    Some(if hints.is_empty() { (key, false) } else { (format!("{key}@{hints}"), true) })
}

/// Search (NETWORK 4.7) of every relay of `node` (`relays`, the URLs it
/// was opened with), in `scope`.
pub fn find(node: &mut Node, relays: &[String], scope: u8, text: &str) -> Result<Vec<SignedEntry>, String> {
    if !(1..=profile::MAX_NAME).contains(&text.len()) {
        return Err("the text to find is 1 to 48 bytes".into());
    }
    if relays.is_empty() {
        return Err("no relay to ask: give --relay, or add one with `terse relay add <url>`".into());
    }
    match node.find(scope, text.as_bytes()) {
        Done::Found(found) => Ok(found),
        _ => Err("no relay answered".into()),
    }
}

/// A relay as a person types it: a URL, or a host name taken as `wss://`
/// (a loopback address as `ws://`), said when the scheme was left out.
pub fn typed_relay(text: &str, say: &mut dyn FnMut(Said)) -> Result<String, String> {
    let url = crate::local::typed_url(text).ok_or(format!("not a relay: {text} (a ws:// or wss:// URL, or a host name)"))?;
    if !text.contains("://") {
        if url.starts_with("ws://") {
            say(Said::Note(format!("taken as {url} (plain: a relay on this machine; for TLS give wss://{text})")));
        } else {
            say(Said::Note(format!("taken as {url} (encrypted; for a plain relay give ws://{text})")));
        }
    }
    Ok(url)
}

/// The relay list as written in the config directory: each relay's URL
/// and its line (with its options).
pub fn relay_list(config: &Path) -> Result<Vec<(String, String)>, String> {
    Ok(crate::local::read_relay_lines(config)?.map(|(l, _)| l).unwrap_or_default())
}

/// A change to the relay list.
pub enum RelayChange<'a> {
    /// The list becomes this one relay (typed, then its options).
    Set(&'a [String]),
    /// The relay (typed, then its options) is added at the end.
    Add(&'a [String]),
    Remove(&'a str),
}

/// Changes the relay list in the config directory (its options,
/// `authority=`, `only-authority=`, `pin=`, are checked when it is
/// saved) and returns the new list.
pub fn change_relays(config: &Path, change: RelayChange, say: &mut dyn FnMut(Said)) -> Result<Vec<(String, String)>, String> {
    let mut list = relay_list(config)?;
    let line = |given: &[String], say: &mut dyn FnMut(Said)| -> Result<(String, String), String> {
        let (relay, options) = given.split_first().ok_or("no relay given")?;
        let url = typed_relay(relay, say)?;
        let mut words = vec![url.clone()];
        words.extend(options.iter().filter(|o| !o.is_empty()).cloned());
        Ok((url, words.join(" ")))
    };
    match change {
        RelayChange::Set(given) => list = vec![line(given, say)?],
        RelayChange::Add(given) => {
            let (url, l) = line(given, say)?;
            if list.iter().any(|(u, _)| *u == url) {
                return Err(format!("{url} is already in the list (remove it first to change its options)"));
            }
            list.push((url, l));
        }
        RelayChange::Remove(given) => {
            let url = typed_relay(given, say)?;
            let before = list.len();
            list.retain(|(u, _)| *u != url);
            if list.len() == before {
                return Err(format!("{url} is not in the list"));
            }
        }
    }
    let lines: Vec<String> = list.iter().map(|(_, l)| l.clone()).collect();
    let path = crate::local::write_relays(config, &lines)?;
    say(Said::Note(format!("relays saved in {}", path.display())));
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_created_imported_never_replaced() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert!(load_identity(a.path()).unwrap_err().contains("terse key new"));
        let made = create_identity(a.path()).unwrap();
        assert!(create_identity(a.path()).unwrap_err().contains("already exists"));
        let imported = import_identity(b.path(), &identity_path(a.path())).unwrap();
        assert_eq!(imported.feed_key(), made.feed_key());
        assert_eq!(load_identity(b.path()).unwrap().feed_key(), made.feed_key());
        assert!(import_identity(b.path(), &identity_path(a.path())).unwrap_err().contains("already exists"));
        // Not a key file: refused, nothing written.
        let c = tempfile::tempdir().unwrap();
        std::fs::write(c.path().join("note.txt"), "hello").unwrap();
        assert!(import_identity(c.path(), &c.path().join("note.txt")).unwrap_err().contains("not a Tersenet secret key file"));
        assert!(!identity_path(c.path()).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(identity_path(b.path())).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
    }

    #[test]
    fn titles_and_checks() {
        assert_eq!(title_of(b"intro\n# Hello there \n\ntext", "file"), "Hello there");
        assert_eq!(title_of(b"no heading", "notes"), "notes");
        assert_eq!(title_of("# ".to_string().as_bytes(), "x"), "x");
        assert!(title_of(format!("# {}", "é".repeat(100)).as_bytes(), "x").len() <= MAX_TITLE);
        assert!(check_article(b"", "t").unwrap_err().contains("1 to"));
        assert!(check_article(b"x", "").is_err());
        assert!(check_article(b"x", &"t".repeat(MAX_TITLE + 1)).is_err());
        check_article(b"x", "t").unwrap();
    }

    #[test]
    fn subscriptions_and_relays() {
        let config = tempfile::tempdir().unwrap();
        let mut said = Vec::new();
        let mut say = |s: Said| said.push(s);
        let key = to_hex(&Identity::from_seed([7; 32]).feed_key());
        let k = subscribe(config.path(), &format!("{key}@relay.example"), true, &mut say).unwrap();
        assert!(load_subscriptions(config.path()).unwrap().contains(&k));
        assert!(subscribe(config.path(), "nonsense", true, &mut say).is_err());
        subscribe(config.path(), &key, false, &mut say).unwrap();
        assert!(subscribe(config.path(), &key, false, &mut say).unwrap_err().starts_with("not subscribed"));
        assert!(said.contains(&Said::Out(format!("subscribed to {key}"))), "{said:?}");
        let mut said = Vec::new();
        let mut say = |s: Said| said.push(s);
        let list = change_relays(config.path(), RelayChange::Add(&["relay.example".into()]), &mut say).unwrap();
        assert_eq!(list, [("wss://relay.example".to_string(), "wss://relay.example".to_string())]);
        let list = change_relays(config.path(), RelayChange::Add(&["ws://192.168.1.2:7878".into()]), &mut say).unwrap();
        assert_eq!(list.len(), 2);
        assert!(change_relays(config.path(), RelayChange::Add(&["wss://relay.example".into()]), &mut say).unwrap_err().contains("already"));
        assert!(change_relays(config.path(), RelayChange::Add(&["wss://b.example".into(), "pin=nonsense".into()]), &mut say).is_err());
        let list = change_relays(config.path(), RelayChange::Remove("relay.example"), &mut say).unwrap();
        assert_eq!(list[0].0, "ws://192.168.1.2:7878");
        assert!(change_relays(config.path(), RelayChange::Remove("relay.example"), &mut say).is_err());
        assert_eq!(relay_list(config.path()).unwrap().len(), 1);
        assert!(said.contains(&Said::Note("taken as wss://relay.example (encrypted; for a plain relay give ws://relay.example)".into())), "{said:?}");
        assert_eq!(profile_relays(&["wss://a.example".into(), "ws://b.example".into(), "wss://127.0.0.1:9".into()]), ["wss://a.example"]);
        assert_eq!(plain_left_out(&["ws://127.0.0.1:7878".into(), "ws://b.example".into()]), ["ws://b.example"]);
    }

    #[test]
    fn profile_and_article_without_a_relay() {
        let (config, store) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let identity = create_identity(config.path()).unwrap();
        let mut node = Node::open(Some(store.path().to_path_buf()), &[], crate::relay::DEFAULT_TIMEOUT).unwrap();
        let mut said = Vec::new();
        let mut say = |s: Said| said.push(s);
        assert!(publish_profile(&mut node, load_identity(config.path()).unwrap(), " Sam", None, None, &mut say).unwrap_err().contains(BAD_NAME));
        let (p, listed) = publish_profile(&mut node, load_identity(config.path()).unwrap(), "Sam", Some(true), Some("About me".into()), &mut say).unwrap();
        assert_eq!((p.seq, listed), (1, true));
        // Listed stays as it was when not given.
        let (p, listed) = publish_profile(&mut node, load_identity(config.path()).unwrap(), "Sam B", None, None, &mut say).unwrap();
        assert_eq!((p.seq, listed), (2, true));
        let (_, doc) = node.profile(&identity.feed_key()).unwrap();
        assert_eq!(doc.unwrap().description, "About me");
        let a = || Article { data: b"# Hi\n\ntext\n".to_vec(), title: "Hi".into(), reply_to: None, dry_run: true, force: false };
        assert_eq!(publish_article(&mut node, None, a(), &mut say).unwrap(), None);
        let done = publish_article(&mut node, Some(identity.clone()), Article { dry_run: false, ..a() }, &mut say).unwrap().unwrap();
        assert_eq!(done.seq, 3);
        assert!(said.contains(&Said::Note("no relay: stored locally only".into())), "{said:?}");
        assert_eq!(own_address(&node), Some((to_hex(&identity.feed_key()), false)));
    }
}
