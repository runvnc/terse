//! The host's own screens for the user's side of Tersenet ("You", F10 in
//! a window with the shell): the signature (the identity key), the
//! profile, publishing an article, the feeds followed and the relays,
//! without the command line.
//!
//! They are the host's, never a program's: programs are sandboxed and must
//! never come near the secret key. Like the consent prompt they are drawn
//! by the host over the screen in the overlay's own colours, here from row
//! 1 down (the shell's bar, row 0, stays); while they are open no program
//! gets a key, a character or the pointer. Machines go on running beneath.
//!
//! Everything they do goes through `terse_net::account`, as the `terse`
//! commands do, on a thread of their own (a relay may be slow): creating or
//! importing the key, publishing the profile and articles, following,
//! search, the relay list. The secret key itself is never shown.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use terse_core::{COLS, ROWS, Terminal};
use terse_net::account::{self, Said};
use terse_net::article_parse::{self, Block, Inline, Parsed};
use terse_net::profile::{self, Nicknames};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{FeedKey, Node, to_hex};

use crate::consent::{BG_BAND, BG_BASE, BG_FIELD, BG_KEY, FG_DIM, FG_INK, FG_KEY, FG_TEXT, fit, short_key, size_text, width};

/// Where the screens find the user's things.
#[derive(Clone, Debug)]
pub struct Setup {
    /// The config directory: identity, subscriptions, relay list.
    pub config: PathBuf,
    /// The blob store (feeds are kept beside it).
    pub store: PathBuf,
    /// The relays this run uses.
    pub relays: Vec<String>,
    pub feed_relays: bool,
    /// Where the file chooser starts for an article.
    pub documents: PathBuf,
    /// Where it starts for a key file.
    pub home: PathBuf,
}

/// The user's documents folder: `XDG_DOCUMENTS_DIR`, else the one
/// `user-dirs.dirs` names, else `Documents` in the home folder if there is
/// one, else the home folder.
pub fn documents_dir(home: &Path) -> PathBuf {
    let var = |v: &str| std::env::var_os(v).map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(d) = var("XDG_DOCUMENTS_DIR") {
        return d;
    }
    let config = var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    if let Ok(text) = std::fs::read_to_string(config.join("user-dirs.dirs")) {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("XDG_DOCUMENTS_DIR=") {
                let v = v.trim_matches('"');
                let p = match v.strip_prefix("$HOME") {
                    Some(rest) => home.join(rest.trim_start_matches('/')),
                    None => PathBuf::from(v),
                };
                if p.is_dir() {
                    return p;
                }
            }
        }
    }
    let d = home.join("Documents");
    if d.is_dir() { d } else { home.to_path_buf() }
}

/// The home folder (`HOME`, `USERPROFILE`), else the working directory.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

// ---------------------------------------------------------------- work

/// What the screens ask their thread to do.
#[derive(Clone, Debug, PartialEq)]
enum Job {
    Load,
    CreateKey,
    ImportKey(PathBuf),
    SaveProfile { name: String, about: String, listed: bool },
    Preview { path: PathBuf, title: String },
    Publish { data: Vec<u8>, title: String },
    Find(String),
    Follow { text: String, add: bool },
    AddRelay(Vec<String>),
    RemoveRelay(String),
}

impl Job {
    /// What is being done, for the message line.
    fn doing(&self) -> &'static str {
        match self {
            Job::Load => "reading your settings",
            Job::CreateKey => "making your signature",
            Job::ImportKey(_) => "reading the key file",
            Job::SaveProfile { .. } => "publishing your profile",
            Job::Preview { .. } => "reading the file",
            Job::Publish { .. } => "publishing the article",
            Job::Find(_) => "searching the relays",
            Job::Follow { add: true, .. } => "following the feed",
            Job::Follow { add: false, .. } => "unfollowing the feed",
            Job::AddRelay(_) | Job::RemoveRelay(_) => "saving your relay list",
        }
    }
}

/// What the user has, as last read.
#[derive(Clone, Debug, Default, PartialEq)]
struct State {
    key: Option<FeedKey>,
    /// Why the key file could not be read.
    key_problem: Option<String>,
    key_path: PathBuf,
    address: Option<(String, bool)>,
    /// The profile in force: its name, entry number, description, listed.
    profile: Option<(String, u32, String, bool)>,
    /// Followed feeds: key and name to show.
    follows: Vec<(FeedKey, Option<String>)>,
    relay_list: Vec<(String, String)>,
    relay_problem: Option<String>,
    in_use: Vec<String>,
}

/// An article read and checked, before it is published.
#[derive(Clone, Debug, PartialEq)]
struct Preview {
    path: PathBuf,
    title: String,
    data: Vec<u8>,
    /// The article as the reader lays it out, roughly: (colour, line).
    lines: Vec<(u8, String)>,
    /// What publishing would do with each link, or why it would not.
    links: Vec<String>,
    refusal: Option<String>,
}

enum Reply {
    State(Box<State>),
    /// A job finished: the lines to show (the first is the summary).
    Done(Vec<String>),
    Failed(String),
    Preview(Box<Preview>),
    Found(Vec<(FeedKey, String)>),
}

fn open_node(setup: &Setup, relays: &[String]) -> Result<Node, String> {
    let mut node = Node::open(Some(setup.store.clone()), relays, DEFAULT_TIMEOUT)?;
    node.set_feed_relays(setup.feed_relays);
    // As a command does: every relay gets what is published.
    node.set_deliver_to_all(true);
    node.set_subscriptions(account::load_subscriptions(&setup.config)?);
    node.set_nicknames(Nicknames::load_from_config(&setup.config)?);
    Ok(node)
}

fn load(setup: &Setup) -> Result<State, String> {
    let mut s = State { key_path: account::identity_path(&setup.config), in_use: setup.relays.clone(), ..State::default() };
    let mut node = open_node(setup, &setup.relays)?;
    if s.key_path.exists() {
        match account::load_identity(&setup.config) {
            Ok(identity) => {
                let key = identity.feed_key();
                node.set_identity(identity);
                s.key = Some(key);
                s.address = account::own_address(&node);
                if let Some((entry, doc)) = node.profile(&key) {
                    let (about, listed) = doc.map_or((String::new(), false), |d| (d.description, d.listed));
                    s.profile = Some((account::printable(&entry.title, false), entry.seq, about, listed));
                }
            }
            Err(e) => s.key_problem = Some(e),
        }
    }
    s.follows = node.subscriptions().feeds().map(|k| (*k, account::name_of(&node, k))).collect();
    match account::relay_list(&setup.config) {
        Ok(l) => s.relay_list = l,
        Err(e) => s.relay_problem = Some(e),
    }
    Ok(s)
}

/// The notes and results an operation said, as lines for the screen.
fn heard(said: Vec<Said>) -> Vec<String> {
    said.into_iter().map(|s| match s {
        Said::Out(l) | Said::Note(l) => l,
    }).collect()
}

fn work(setup: &Setup, job: Job) -> Vec<Reply> {
    let mut said = Vec::new();
    let mut say = |s: Said| said.push(s);
    let reload = |r: Reply| match load(setup) {
        Ok(s) => vec![r, Reply::State(Box::new(s))],
        Err(e) => vec![r, Reply::Failed(e)],
    };
    match job {
        Job::Load => match load(setup) {
            Ok(s) => vec![Reply::State(Box::new(s))],
            Err(e) => vec![Reply::Failed(e)],
        },
        Job::CreateKey => match account::create_identity(&setup.config) {
            Ok(id) => reload(Reply::Done(vec![format!("Your signature is made: {}. Keep a copy of its file (below).", short_key(&id.feed_key()))])),
            Err(e) => vec![Reply::Failed(e)],
        },
        Job::ImportKey(path) => match account::import_identity(&setup.config, &path) {
            Ok(id) => reload(Reply::Done(vec![format!("The key in {} is now your signature: {}.", path.display(), short_key(&id.feed_key()))])),
            Err(e) => vec![Reply::Failed(e)],
        },
        Job::SaveProfile { name, about, listed } => {
            let r = account::load_identity(&setup.config).and_then(|identity| {
                let mut node = open_node(setup, &setup.relays)?;
                account::publish_profile(&mut node, identity, &name, Some(listed), Some(about), &mut say)
            });
            match r {
                Ok((p, listed)) => {
                    let mut lines = vec![format!(
                        "Your profile is published as entry #{}{}.",
                        p.seq,
                        if listed { ", and search can find your name" } else { "; search will not find your name" }
                    )];
                    lines.extend(heard(said).into_iter().filter(|l| !l.starts_with("seq ")));
                    reload(Reply::Done(lines))
                }
                Err(e) => vec![Reply::Failed(with_notes(e, said))],
            }
        }
        Job::Preview { path, title } => match preview(setup, &path, title) {
            Ok(p) => vec![Reply::Preview(Box::new(p))],
            Err(e) => vec![Reply::Failed(e)],
        },
        Job::Publish { data, title } => {
            let shown = title.clone();
            let r = account::load_identity(&setup.config).and_then(|identity| {
                let mut node = open_node(setup, &setup.relays)?;
                let a = account::Article { data, title, reply_to: None, dry_run: false, force: false };
                account::publish_article(&mut node, Some(identity), a, &mut say)
            });
            match r {
                Ok(Some(p)) => {
                    let mut lines = vec![format!("“{}” is published as entry #{} of your feed.", shown, p.seq)];
                    lines.extend(heard(said).into_iter().filter(|l| !l.starts_with("seq ")));
                    reload(Reply::Done(lines))
                }
                Ok(None) => vec![Reply::Failed("nothing was published".into())],
                Err(e) => vec![Reply::Failed(with_notes(e, said))],
            }
        }
        Job::Find(_) if setup.relays.is_empty() => vec![Reply::Failed("there is no relay to ask: add one on the Relays page".into())],
        Job::Find(text) => {
            let r = open_node(setup, &setup.relays).and_then(|mut node| {
                let found = account::find(&mut node, &setup.relays, terse_net::protocol::SCOPE_FEEDS, &text)?;
                Ok(found.into_iter().map(|s| (s.entry.feed, account::printable(&s.entry.title, false))).collect::<Vec<_>>())
            });
            match r {
                Ok(found) => vec![Reply::Found(found)],
                Err(e) => vec![Reply::Failed(e)],
            }
        }
        Job::Follow { text, add } => match account::subscribe(&setup.config, &text, add, &mut say) {
            Ok(key) => {
                let first = if add { format!("You follow {} now: its articles come to your timeline.", short_key(&key)) } else { format!("You no longer follow {}.", short_key(&key)) };
                let mut lines = vec![first];
                lines.extend(heard(said).into_iter().filter(|l| !l.starts_with("subscribed to") && !l.starts_with("unsubscribed from")));
                reload(Reply::Done(lines))
            }
            Err(e) => vec![Reply::Failed(e)],
        },
        Job::AddRelay(words) => {
            let was_empty = account::relay_list(&setup.config).map(|l| l.is_empty()).unwrap_or(false);
            match account::change_relays(&setup.config, account::RelayChange::Add(&words), &mut say) {
                Ok(list) => {
                    let mut lines = vec![format!("Added {}: Tersenet uses it from its next start.", list.last().map_or("", |(u, _)| u.as_str()))];
                    if was_empty {
                        lines.push(format!(
                            "Your list now replaces the relay on this computer ({}): add it too to keep using it.",
                            terse_net::local::local_url(terse_net::local::local_port())
                        ));
                    }
                    lines.extend(heard(said));
                    reload(Reply::Done(lines))
                }
                Err(e) => vec![Reply::Failed(with_notes(e, said))],
            }
        }
        Job::RemoveRelay(url) => match account::change_relays(&setup.config, account::RelayChange::Remove(&url), &mut say) {
            Ok(_) => {
                let mut lines = vec![format!("{url} is out of your relay list; Tersenet stops using it from its next start.")];
                lines.extend(heard(said));
                reload(Reply::Done(lines))
            }
            Err(e) => vec![Reply::Failed(e)],
        },
    }
}

/// An error, after what was said on the way to it.
fn with_notes(e: String, said: Vec<Said>) -> String {
    let notes: Vec<String> = heard(said).into_iter().filter(|l| l.starts_with("warning") || l.starts_with("not in")).collect();
    if notes.is_empty() { e } else { format!("{e} ({})", notes.join("; ")) }
}

/// Reads and checks an article, and says what publishing it would do,
/// as `terse feed publish --dry-run` does.
fn preview(setup: &Setup, path: &Path, title: String) -> Result<Preview, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if len > terse_net::blob::MAX_BLOB as u64 {
        return Err(format!("{} is {} bytes; an article is at most {}", path.display(), len, terse_net::blob::MAX_BLOB));
    }
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = std::str::from_utf8(&data).map_err(|_| format!("{} is not a text file (UTF-8): an article is plain text", path.display()))?;
    let fallback = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = if title.trim().is_empty() { account::title_of(&data, &fallback) } else { title.trim().to_string() };
    account::check_article(&data, &title)?;
    let identity = account::load_identity(&setup.config).ok();
    // No relay: a dry run asks none.
    let mut node = open_node(setup, &[])?;
    let mut said = Vec::new();
    let a = account::Article { data: data.clone(), title: title.clone(), reply_to: None, dry_run: true, force: false };
    let refusal = account::publish_article(&mut node, identity, a, &mut |s| said.push(s)).err();
    let links = heard(said).into_iter().filter(|l| l.starts_with("link ")).collect();
    Ok(Preview { path: path.to_path_buf(), title, lines: render_article(text, BODY_WIDTH), data, links, refusal })
}

/// An article as plain lines `cols` wide: headings in the key colour,
/// items with bullets, code and tables as written.
fn render_article(text: &str, cols: usize) -> Vec<(u8, String)> {
    let Parsed::Article(blocks) = article_parse::parse(text.as_bytes()) else {
        return wrap(text, cols).into_iter().map(|l| (FG_TEXT, l)).collect();
    };
    let words = |xs: &[Inline]| -> String {
        xs.iter()
            .map(|x| match x {
                Inline::Text(t) | Inline::Code(t) => t.clone(),
                Inline::Link { text: Some(t), .. } => t.clone(),
                Inline::Link { target, .. } => match target {
                    article_parse::Target::Article(id) => format!("terse:{}…", &to_hex(id)[..8]),
                    article_parse::Target::Feed(k, _) => format!("feed {}", short_key(k)),
                    article_parse::Target::Program(_, n) => n.clone(),
                },
            })
            .collect()
    };
    let mut out = Vec::new();
    let mut in_list = false;
    for b in &blocks {
        let item = matches!(b, Block::Item(_));
        // A blank line between blocks; a list's items go together.
        if !out.is_empty() && !(item && in_list) {
            out.push((FG_TEXT, String::new()));
        }
        in_list = item;
        match b {
            Block::Heading(x) => out.extend(wrap(&words(x).to_uppercase(), cols).into_iter().map(|l| (FG_KEY, l))),
            Block::Subheading(x) => out.extend(wrap(&words(x), cols).into_iter().map(|l| (FG_KEY, l))),
            Block::Paragraph(x) => out.extend(wrap(&words(x), cols).into_iter().map(|l| (FG_TEXT, l))),
            Block::Item(x) => {
                for (i, l) in wrap(&words(x), cols - 2).into_iter().enumerate() {
                    out.push((FG_TEXT, format!("{} {l}", if i == 0 { '•' } else { ' ' })));
                }
            }
            Block::Pre(lines, _) => out.extend(lines.iter().map(|l| (FG_DIM, fit(&words(l), cols)))),
            Block::Table(t) => {
                for (i, row) in t.rows.iter().enumerate() {
                    let cells: Vec<String> = row.iter().map(|c| words(c)).collect();
                    out.push((if i == 0 && t.header { FG_KEY } else { FG_TEXT }, fit(&cells.join(" │ "), cols)));
                }
            }
            Block::Figure(_) => out.push((FG_DIM, "[a figure: the reader draws it]".into())),
        }
    }
    out
}

/// `text` broken into lines of at most `cols` columns at spaces (a word
/// longer than a line is cut); newlines kept.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut w = 0;
        for word in para.split(' ') {
            let ww: usize = word.chars().map(width).sum();
            if w > 0 && w + 1 + ww > cols {
                out.push(std::mem::take(&mut line));
                w = 0;
            }
            if w > 0 {
                line.push(' ');
                w += 1;
            }
            if ww > cols {
                let mut piece = String::new();
                let mut pw = 0;
                for c in word.chars() {
                    if pw + width(c) > cols {
                        out.push(std::mem::take(&mut piece));
                        pw = 0;
                    }
                    piece.push(c);
                    pw += width(c);
                }
                line.push_str(&piece);
                w = pw;
            } else {
                line.push_str(word);
                w += ww;
            }
        }
        out.push(line);
    }
    out
}

// ---------------------------------------------------------------- the screens

/// A key the screens take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Tab,
    BackTab,
    Backspace,
    Delete,
}

/// What the window does after an input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Close,
    /// Put this text on the clipboard.
    Copy(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Signature,
    Profile,
    Publish,
    Follow,
    Relays,
}

const TABS: [(Tab, &str); 5] =
    [(Tab::Signature, "Signature"), (Tab::Profile, "Profile"), (Tab::Publish, "Publish"), (Tab::Follow, "Follow"), (Tab::Relays, "Relays")];

/// What can have the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Tabs,
    Create,
    Import,
    CopyAddress,
    GoSignature,
    Name,
    About,
    Listed,
    Save,
    File,
    Choose,
    Title,
    Preview,
    Query,
    Search,
    FollowOwn,
    Found(usize),
    Followed(usize),
    RelayField,
    Options,
    Add,
    Relay(usize),
}

impl Item {
    fn is_field(self) -> bool {
        matches!(self, Item::Name | Item::About | Item::File | Item::Title | Item::Query | Item::RelayField | Item::Options)
    }
}

/// A line of text being edited.
#[derive(Clone, Debug, Default, PartialEq)]
struct Field {
    text: String,
    /// The caret, in characters.
    caret: usize,
    /// At most this many bytes.
    max: usize,
}

impl Field {
    fn new(max: usize) -> Self {
        Field { max, ..Field::default() }
    }

    fn set(&mut self, text: &str) {
        self.text.clear();
        self.caret = 0;
        for c in text.chars() {
            self.insert(c);
        }
    }

    fn byte(&self, at: usize) -> usize {
        self.text.char_indices().nth(at).map_or(self.text.len(), |(i, _)| i)
    }

    fn insert(&mut self, c: char) {
        if c.is_control() && c != '\n' || self.text.len() + c.len_utf8() > self.max {
            return;
        }
        let at = self.byte(self.caret);
        self.text.insert(at, c);
        self.caret += 1;
    }

    fn key(&mut self, k: Key) -> bool {
        let n = self.text.chars().count();
        match k {
            Key::Left => self.caret = self.caret.saturating_sub(1),
            Key::Right => self.caret = (self.caret + 1).min(n),
            Key::Home => self.caret = 0,
            Key::End => self.caret = n,
            Key::Backspace if self.caret > 0 => {
                self.caret -= 1;
                let at = self.byte(self.caret);
                self.text.remove(at);
            }
            Key::Delete if self.caret < n => {
                let at = self.byte(self.caret);
                self.text.remove(at);
            }
            Key::Backspace | Key::Delete => {}
            _ => return false,
        }
        true
    }
}

/// A coloured run of text on a row.
#[derive(Clone, Debug)]
struct Span {
    fg: u8,
    bg: u8,
    text: String,
}

fn span(fg: u8, bg: u8, text: impl Into<String>) -> Span {
    Span { fg, bg, text: text.into() }
}

/// A row of the body, and what it lets the user act on.
struct Line {
    spans: Vec<Span>,
    item: Option<Item>,
}

/// A question before something is done that cannot be undone, or that
/// goes out to the network, as the consent prompt asks it.
#[derive(Clone, Debug, PartialEq)]
struct Confirm {
    what: String,
    then: String,
    job: Job,
}

/// What the file chooser is choosing for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    Article,
    KeyFile,
}

#[derive(Clone, Debug, PartialEq)]
struct Chooser {
    purpose: Purpose,
    dir: PathBuf,
    /// Name, whether a folder, size.
    entries: Vec<(String, bool, u64)>,
    selected: usize,
    problem: Option<String>,
}

impl Chooser {
    fn open(purpose: Purpose, dir: PathBuf) -> Self {
        let mut c = Chooser { purpose, dir, entries: Vec::new(), selected: 0, problem: None };
        c.read();
        c
    }

    /// The folder's entries: `..` first, folders, then files, by name;
    /// hidden ones left out. At most 2,000.
    fn read(&mut self) {
        self.entries.clear();
        self.selected = 0;
        self.problem = None;
        if self.dir.parent().is_some() {
            self.entries.push(("..".into(), true, 0));
        }
        let mut found = Vec::new();
        match std::fs::read_dir(&self.dir) {
            Ok(rd) => {
                for e in rd.flatten().take(2000) {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    let meta = std::fs::metadata(e.path());
                    let dir = meta.as_ref().is_ok_and(|m| m.is_dir());
                    found.push((name, dir, meta.map_or(0, |m| m.len())));
                }
            }
            Err(e) => self.problem = Some(format!("{}: {e}", self.dir.display())),
        }
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        self.entries.extend(found);
    }
}

/// What covers the pages for a while.
#[derive(Clone, Debug, PartialEq)]
enum Modal {
    Confirm(Confirm),
    Chooser(Chooser),
    Preview(Preview, usize),
}

/// Rows of the screen: the bar with the pages, the body, the message and
/// the keys.
const BAR_ROW: usize = 1;
const BODY_TOP: usize = 3;
const BODY_ROWS: usize = 22;
const MESSAGE_ROW: usize = BODY_TOP + BODY_ROWS;
const MESSAGE_ROWS: usize = 3;
const KEYS_ROW: usize = ROWS - 1;
/// The confirmation's rows (as the consent prompt's).
const CONFIRM_ROW: usize = ROWS - 4;
/// The body's text, inside its margin.
const BODY_WIDTH: usize = COLS - 4;

/// The host's screens: open or not, what each page holds, the thread
/// doing their work.
pub struct You {
    setup: Setup,
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
    tab: Tab,
    focus: Item,
    state: Option<State>,
    name: Field,
    about: Field,
    listed: bool,
    file: Field,
    title: Field,
    query: Field,
    relay: Field,
    options: Field,
    found: Option<Vec<(FeedKey, String)>>,
    modal: Option<Modal>,
    /// The job being done.
    busy: Option<&'static str>,
    /// The last result: whether it went well, and its lines.
    message: Option<(bool, Vec<String>)>,
    /// Something changed since the last `render`.
    changed: bool,
}

impl You {
    /// The screens, with a thread for their work, reading the user's
    /// things at once.
    pub fn new(setup: Setup) -> Self {
        let (jobs, rx) = channel::<Job>();
        let (tx, replies) = channel();
        let s = setup.clone();
        std::thread::Builder::new()
            .name("terse-you".into())
            .spawn(move || {
                for job in rx {
                    for r in work(&s, job) {
                        if tx.send(r).is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("a thread for the host's screens");
        let mut you = You {
            setup,
            jobs,
            replies,
            tab: Tab::Signature,
            focus: Item::Tabs,
            state: None,
            name: Field::new(profile::MAX_NAME),
            about: Field::new(3000),
            listed: false,
            file: Field::new(4096),
            title: Field::new(terse_net::entry::MAX_TITLE),
            query: Field::new(400),
            relay: Field::new(300),
            options: Field::new(600),
            found: None,
            modal: None,
            busy: None,
            message: None,
            changed: true,
        };
        you.start(Job::Load);
        you
    }

    /// Opened again: the user's things are read again, the page kept.
    pub fn reopen(&mut self) {
        self.modal = None;
        self.message = None;
        if self.busy.is_none() {
            self.start(Job::Load);
        }
        self.changed = true;
    }

    fn start(&mut self, job: Job) {
        self.busy = Some(job.doing());
        if job != Job::Load {
            self.message = None;
        }
        let _ = self.jobs.send(job);
        self.changed = true;
    }

    /// Takes in what the thread finished; `true` if the picture changed.
    pub fn poll(&mut self) -> bool {
        loop {
            match self.replies.try_recv() {
                Ok(r) => self.reply(r),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        std::mem::take(&mut self.changed)
    }

    /// Whether a job is being done (tests wait for it).
    pub fn busy(&self) -> bool {
        self.busy.is_some()
    }

    fn reply(&mut self, r: Reply) {
        self.changed = true;
        match r {
            Reply::State(s) => {
                let first = self.state.is_none();
                let profile_changed = self.state.as_ref().map(|o| &o.profile) != Some(&s.profile);
                if first || profile_changed {
                    // The profile's fields show what is in force.
                    let (name, about, listed) = s.profile.clone().map_or((String::new(), String::new(), false), |(n, _, a, l)| (n, a, l));
                    self.name.set(&name);
                    self.about.set(&about);
                    self.listed = listed;
                }
                if first {
                    // A first run: the signature page; else where the user left off.
                    self.tab = if s.key.is_none() { Tab::Signature } else { self.tab };
                }
                self.state = Some(*s);
                self.busy = None;
            }
            Reply::Done(lines) => {
                self.message = Some((true, lines));
                self.busy = None;
                self.after_done();
            }
            Reply::Failed(e) => {
                self.message = Some((false, vec![plain_error(&e)]));
                self.busy = None;
            }
            Reply::Preview(p) => {
                if self.title.text.trim().is_empty() {
                    self.title.set(&p.title);
                }
                self.modal = Some(Modal::Preview(*p, 0));
                self.busy = None;
            }
            Reply::Found(found) => {
                self.message = Some((true, vec![match found.len() {
                    0 => format!("No listed feed has a name with “{}” in it.", self.query.text.trim()),
                    1 => "1 feed found: Enter on it follows it.".into(),
                    n => format!("{n} feeds found: Enter on one follows it."),
                }]));
                self.focus = if found.is_empty() { Item::Query } else { Item::Found(0) };
                self.found = Some(found);
                self.busy = None;
            }
        }
    }

    /// After a job went well: fields that did their work are emptied.
    fn after_done(&mut self) {
        match self.tab {
            Tab::Follow => {
                self.query.set("");
                self.found = None;
                if matches!(self.focus, Item::Found(_)) {
                    self.focus = Item::Query;
                }
            }
            Tab::Relays => {
                self.relay.set("");
                self.options.set("");
            }
            Tab::Publish => {
                self.file.set("");
                self.title.set("");
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ input

    /// A key pressed (or repeated).
    pub fn key(&mut self, k: Key) -> Effect {
        self.changed = true;
        if let Some(m) = self.modal.take() {
            return self.modal_key(m, k);
        }
        let items = self.items();
        let at = items.iter().position(|&i| i == self.focus).unwrap_or(0);
        if self.focus.is_field() {
            let f = self.field_mut(self.focus).expect("a field");
            if f.key(k) {
                return Effect::None;
            }
        }
        match k {
            Key::Escape => return Effect::Close,
            Key::Up | Key::BackTab => self.focus = items[at.saturating_sub(1)],
            Key::Down | Key::Tab => self.focus = items[(at + 1).min(items.len() - 1)],
            Key::PageUp | Key::Home => self.focus = items[0],
            Key::PageDown | Key::End => self.focus = *items.last().unwrap(),
            Key::Left | Key::Right if self.focus == Item::Tabs => {
                let i = TABS.iter().position(|(t, _)| *t == self.tab).unwrap();
                let i = if k == Key::Left { (i + TABS.len() - 1) % TABS.len() } else { (i + 1) % TABS.len() };
                self.show_tab(TABS[i].0, false);
            }
            Key::Enter => return self.activate(self.focus),
            Key::Delete => match self.focus {
                Item::Followed(_) | Item::Relay(_) => return self.activate(self.focus),
                _ => {}
            },
            _ => {}
        }
        Effect::None
    }

    /// A character typed (or pasted).
    pub fn text(&mut self, c: char) -> Effect {
        self.changed = true;
        match self.modal {
            Some(Modal::Confirm(_)) => {
                return match c.to_ascii_lowercase() {
                    'y' => self.answer(true),
                    'n' => self.answer(false),
                    _ => Effect::None,
                };
            }
            Some(Modal::Preview(..)) => {
                if c.eq_ignore_ascii_case(&'p') {
                    self.ask_publish();
                }
                return Effect::None;
            }
            Some(Modal::Chooser(_)) => return Effect::None,
            None => {}
        }
        // A newline only in the description; elsewhere a space.
        let c = if c == '\n' && self.focus != Item::About { ' ' } else { c };
        if let Some(f) = self.field_mut(self.focus) {
            if c == '\t' {
                return Effect::None;
            }
            f.insert(c);
            return Effect::None;
        }
        if c == ' ' {
            return self.activate(self.focus);
        }
        // On anything but a field, 1 to 5 choose a page.
        if let Some(d) = c.to_digit(10).filter(|d| (1..=5).contains(d)) {
            self.show_tab(TABS[d as usize - 1].0, false);
        }
        Effect::None
    }

    /// The primary button pressed at (x, y) in screen units.
    pub fn click(&mut self, x: u16, y: u16) -> Effect {
        self.changed = true;
        let (col, row) = (x as usize * COLS / terse_core::SCREEN_WIDTH as usize, y as usize * ROWS / terse_core::SCREEN_HEIGHT as usize);
        match self.modal.clone() {
            Some(Modal::Confirm(_)) => {
                if row == ROWS - 1 {
                    let (yes, no) = confirm_keys();
                    if yes.contains(&col) {
                        return self.answer(true);
                    }
                    if no.contains(&col) {
                        return self.answer(false);
                    }
                }
                return Effect::None;
            }
            Some(Modal::Chooser(c)) => {
                if (BODY_TOP..BODY_TOP + BODY_ROWS).contains(&row) {
                    let i = chooser_scroll(&c) + row - BODY_TOP;
                    if i < c.entries.len() {
                        if c.selected == i {
                            return self.key(Key::Enter);
                        }
                        if let Some(Modal::Chooser(c)) = &mut self.modal {
                            c.selected = i;
                        }
                    }
                }
                return Effect::None;
            }
            Some(Modal::Preview(..)) => {
                if row == KEYS_ROW && col < 40 {
                    self.ask_publish();
                }
                return Effect::None;
            }
            None => {}
        }
        if row == BAR_ROW {
            for (tab, range) in tab_ranges() {
                if range.contains(&col) {
                    self.show_tab(tab, true);
                }
            }
            return Effect::None;
        }
        let lines = self.body();
        let scroll = self.scroll(&lines);
        if (BODY_TOP..BODY_TOP + BODY_ROWS).contains(&row)
            && let Some(item) = lines.get(scroll + row - BODY_TOP).and_then(|l| l.item)
        {
            self.focus = item;
            if let Some(f) = self.field_mut(item) {
                f.caret = f.text.chars().count();
                return Effect::None;
            }
            return self.activate(item);
        }
        Effect::None
    }

    /// Wheel steps (positive: down).
    pub fn wheel(&mut self, steps: i32) {
        self.changed = true;
        if self.modal.is_none() {
            let k = if steps > 0 { Key::Down } else { Key::Up };
            for _ in 0..steps.unsigned_abs() {
                if !self.focus.is_field() {
                    self.key(k);
                }
            }
            return;
        }
        match &mut self.modal {
            Some(Modal::Preview(p, at)) => {
                let total = preview_lines(p).len();
                *at = (*at as i64 + steps as i64 * 3).clamp(0, total.saturating_sub(BODY_ROWS) as i64) as usize;
            }
            Some(Modal::Chooser(c)) => c.selected = (c.selected as i64 + steps as i64).clamp(0, c.entries.len().saturating_sub(1) as i64) as usize,
            _ => {}
        }
    }

    fn show_tab(&mut self, tab: Tab, enter: bool) {
        self.tab = tab;
        self.message = None;
        self.focus = Item::Tabs;
        if enter {
            let items = self.items();
            self.focus = items.get(1).copied().unwrap_or(Item::Tabs);
        }
    }

    fn field_mut(&mut self, item: Item) -> Option<&mut Field> {
        Some(match item {
            Item::Name => &mut self.name,
            Item::About => &mut self.about,
            Item::File => &mut self.file,
            Item::Title => &mut self.title,
            Item::Query => &mut self.query,
            Item::RelayField => &mut self.relay,
            Item::Options => &mut self.options,
            _ => return None,
        })
    }

    fn field(&self, item: Item) -> &Field {
        match item {
            Item::Name => &self.name,
            Item::About => &self.about,
            Item::File => &self.file,
            Item::Title => &self.title,
            Item::Query => &self.query,
            Item::RelayField => &self.relay,
            _ => &self.options,
        }
    }

    fn say(&mut self, ok: bool, line: impl Into<String>) {
        self.message = Some((ok, vec![line.into()]));
    }

    /// Enter (or a click, or Space) on an item.
    fn activate(&mut self, item: Item) -> Effect {
        if self.busy.is_some() && item != Item::Tabs {
            return Effect::None;
        }
        let key = self.state.as_ref().and_then(|s| s.key);
        match item {
            Item::Tabs => {
                let items = self.items();
                self.focus = items.get(1).copied().unwrap_or(Item::Tabs);
            }
            Item::Create => self.start(Job::CreateKey),
            Item::Import => self.modal = Some(Modal::Chooser(Chooser::open(Purpose::KeyFile, self.setup.home.clone()))),
            Item::CopyAddress => {
                if let Some((a, _)) = self.state.as_ref().and_then(|s| s.address.clone()) {
                    self.say(true, "Your feed address is on the clipboard: paste it wherever you give it to people.");
                    return Effect::Copy(a);
                }
            }
            Item::GoSignature => self.show_tab(Tab::Signature, true),
            Item::Name | Item::About => self.focus = if item == Item::Name { Item::About } else { Item::Listed },
            Item::Listed => self.listed = !self.listed,
            Item::Save => {
                let name = self.name.text.trim().to_string();
                if !profile::valid_name(&name) {
                    self.say(false, "A name is 1 to 48 bytes (letters, digits, spaces inside), with no space at either end.");
                    self.focus = Item::Name;
                    return Effect::None;
                }
                let then = if self.listed {
                    "Anyone can see it beside your key, and search can find your feed by it."
                } else {
                    "Anyone can see it beside your feed key; search will not find it."
                };
                self.modal = Some(Modal::Confirm(Confirm {
                    what: format!("Publish your profile with the name “{}”?", fit(&name, 40)),
                    then: then.into(),
                    job: Job::SaveProfile { name, about: self.about.text.clone(), listed: self.listed },
                }));
            }
            Item::File | Item::Title | Item::Preview => {
                if self.file.text.trim().is_empty() {
                    self.say(false, "Choose a file first, or type its path.");
                    self.focus = Item::Choose;
                    return Effect::None;
                }
                let path = self.typed_path(&self.file.text);
                self.start(Job::Preview { path, title: self.title.text.clone() });
            }
            Item::Choose => {
                let typed = self.typed_path(&self.file.text);
                let dir = if typed.is_dir() {
                    typed
                } else {
                    typed.parent().filter(|p| p.is_dir() && !self.file.text.trim().is_empty()).map(Path::to_path_buf).unwrap_or(self.setup.documents.clone())
                };
                self.modal = Some(Modal::Chooser(Chooser::open(Purpose::Article, dir)));
            }
            Item::Query | Item::Search => {
                let text = self.query.text.trim().to_string();
                if text.is_empty() {
                    self.say(false, "Paste a feed's address, or type a name to search for.");
                    self.focus = Item::Query;
                } else if terse_net::address::parse_address(&text).is_ok() {
                    self.start(Job::Follow { text, add: true });
                } else if text.len() > profile::MAX_NAME || text.chars().all(|c| c.is_ascii_hexdigit() || c == '@') && text.len() > 40 {
                    self.say(false, "That is not a feed address (64 hexadecimal digits, maybe an @ and relays after them), and too long for a name.");
                } else {
                    self.start(Job::Find(text));
                }
            }
            Item::FollowOwn => {
                if let Some(k) = key {
                    self.start(Job::Follow { text: to_hex(&k), add: true });
                }
            }
            Item::Found(i) => {
                if let Some((k, _)) = self.found.as_ref().and_then(|f| f.get(i)) {
                    let text = to_hex(k);
                    self.start(Job::Follow { text, add: true });
                }
            }
            Item::Followed(i) => {
                if let Some((k, name)) = self.state.as_ref().and_then(|s| s.follows.get(i)).cloned() {
                    let who = match name {
                        Some(n) => format!("“{}” ({})", fit(&n, 30), short_key(&k)),
                        None => format!("the feed {}", short_key(&k)),
                    };
                    self.modal = Some(Modal::Confirm(Confirm {
                        what: format!("Stop following {who}?"),
                        then: "Its articles will no longer be shown in your timeline.".into(),
                        job: Job::Follow { text: to_hex(&k), add: false },
                    }));
                }
            }
            Item::RelayField | Item::Options | Item::Add => {
                let relay = self.relay.text.trim().to_string();
                if relay.is_empty() {
                    self.say(false, "Type a relay first: a name such as relay.example.org, or ws://… for a plain one.");
                    self.focus = Item::RelayField;
                    return Effect::None;
                }
                let mut words = vec![relay];
                words.extend(self.options.text.split_whitespace().map(str::to_string));
                self.start(Job::AddRelay(words));
            }
            Item::Relay(i) => {
                if let Some((url, _)) = self.state.as_ref().and_then(|s| s.relay_list.get(i)).cloned() {
                    self.modal = Some(Modal::Confirm(Confirm {
                        what: format!("Take {} out of your relay list?", fit(&url, 50)),
                        then: "From Tersenet's next start it is no longer asked for feeds or sent what you publish.".into(),
                        job: Job::RemoveRelay(url),
                    }));
                }
            }
        }
        Effect::None
    }

    /// A path as typed: `~` is the home folder, a relative one is in the
    /// documents folder.
    fn typed_path(&self, text: &str) -> PathBuf {
        let t = text.trim();
        if let Some(rest) = t.strip_prefix("~/") {
            return self.setup.home.join(rest);
        }
        let p = PathBuf::from(t);
        if p.is_absolute() { p } else { self.setup.documents.join(p) }
    }

    fn answer(&mut self, yes: bool) -> Effect {
        if let Some(Modal::Confirm(c)) = self.modal.take() {
            if yes {
                self.start(c.job);
            } else {
                self.say(false, "Nothing was done.");
            }
        }
        Effect::None
    }

    fn ask_publish(&mut self) {
        let Some(Modal::Preview(p, _)) = self.modal.clone() else { return };
        if let Some(r) = &p.refusal {
            self.say(false, plain_error(r));
            return;
        }
        if self.state.as_ref().is_none_or(|s| s.key.is_none()) {
            self.say(false, "You need a signature to publish: make one on the Signature page first.");
            return;
        }
        let size = size_text(p.data.len() as u32);
        self.modal = Some(Modal::Confirm(Confirm {
            what: format!("Publish “{}” ({size})?", fit(&p.title, 50)),
            then: "It will be signed with your key and sent to the relay for anyone to read.".into(),
            job: Job::Publish { data: p.data, title: p.title },
        }));
    }

    fn modal_key(&mut self, m: Modal, k: Key) -> Effect {
        match m {
            Modal::Confirm(c) => {
                self.modal = Some(Modal::Confirm(c));
                if k == Key::Escape {
                    return self.answer(false);
                }
            }
            Modal::Preview(p, at) => {
                let total = preview_lines(&p).len();
                let max = total.saturating_sub(BODY_ROWS);
                let at = match k {
                    Key::Escape => return Effect::None,
                    Key::Up => at.saturating_sub(1),
                    Key::Down => (at + 1).min(max),
                    Key::PageUp => at.saturating_sub(BODY_ROWS - 1),
                    Key::PageDown => (at + BODY_ROWS - 1).min(max),
                    Key::Home => 0,
                    Key::End => max,
                    Key::Enter => {
                        self.modal = Some(Modal::Preview(p, at));
                        self.ask_publish();
                        return Effect::None;
                    }
                    _ => at,
                };
                self.modal = Some(Modal::Preview(p, at));
            }
            Modal::Chooser(mut c) => {
                let last = c.entries.len().saturating_sub(1);
                match k {
                    Key::Escape => return Effect::None,
                    Key::Up => c.selected = c.selected.saturating_sub(1),
                    Key::Down => c.selected = (c.selected + 1).min(last),
                    Key::PageUp => c.selected = c.selected.saturating_sub(BODY_ROWS - 1),
                    Key::PageDown => c.selected = (c.selected + BODY_ROWS - 1).min(last),
                    Key::Home => c.selected = 0,
                    Key::End => c.selected = last,
                    Key::Backspace | Key::Left => {
                        if let Some(p) = c.dir.parent() {
                            let from = c.dir.file_name().map(|n| n.to_string_lossy().into_owned());
                            c.dir = p.to_path_buf();
                            c.read();
                            if let Some(i) = from.and_then(|f| c.entries.iter().position(|e| e.0 == f)) {
                                c.selected = i;
                            }
                        }
                    }
                    Key::Enter | Key::Right => {
                        if let Some((name, dir, _)) = c.entries.get(c.selected).cloned() {
                            if name == ".." {
                                return self.modal_key(Modal::Chooser(c), Key::Backspace);
                            }
                            let path = c.dir.join(&name);
                            if dir {
                                c.dir = path;
                                c.read();
                            } else if k == Key::Enter {
                                return self.chosen(c.purpose, path);
                            }
                        }
                    }
                    _ => {}
                }
                self.modal = Some(Modal::Chooser(c));
            }
        }
        Effect::None
    }

    /// A file chosen.
    fn chosen(&mut self, purpose: Purpose, path: PathBuf) -> Effect {
        match purpose {
            Purpose::Article => {
                self.file.set(&path.to_string_lossy());
                self.title.set("");
                self.focus = Item::Preview;
                self.start(Job::Preview { path, title: String::new() });
            }
            Purpose::KeyFile => {
                self.modal = Some(Modal::Confirm(Confirm {
                    what: format!("Use the key in {} as your signature?", fit(&path.to_string_lossy(), 44)),
                    then: "It is copied into your settings; the file you chose is left as it is.".into(),
                    job: Job::ImportKey(path),
                }));
            }
        }
        Effect::None
    }

    // ------------------------------------------------------------ pages

    /// What can have the focus on this page, in order.
    fn items(&self) -> Vec<Item> {
        let mut v = vec![Item::Tabs];
        v.extend(self.body().iter().filter_map(|l| l.item));
        v.dedup();
        v
    }

    /// The body's first line shown: the focus stays in view.
    fn scroll(&self, lines: &[Line]) -> usize {
        let at = lines.iter().position(|l| l.item == Some(self.focus)).unwrap_or(0);
        let last = lines.iter().rposition(|l| l.item == Some(self.focus)).unwrap_or(at);
        (last + 2).saturating_sub(BODY_ROWS).min(at).min(lines.len().saturating_sub(BODY_ROWS))
    }

    fn body(&self) -> Vec<Line> {
        let mut b = Body { lines: Vec::new(), focus: self.focus };
        let Some(s) = &self.state else {
            b.text(FG_DIM, "Reading your settings…");
            return b.lines;
        };
        match self.tab {
            Tab::Signature => self.signature(&mut b, s),
            Tab::Profile => self.profile(&mut b, s),
            Tab::Publish => self.publish(&mut b, s),
            Tab::Follow => self.follow(&mut b, s),
            Tab::Relays => self.relays(&mut b, s),
        }
        b.lines
    }

    fn signature(&self, b: &mut Body, s: &State) {
        if let Some(p) = &s.key_problem {
            b.para(FG_KEY, "Your key file cannot be read:");
            b.para(FG_TEXT, &plain_error(p));
            b.blank();
            b.para(FG_DIM, &format!("Tersenet will not replace it. Put a good copy at {} and open this page again.", s.key_path.display()));
            return;
        }
        let Some(key) = s.key else {
            b.para(FG_KEY, "You have no signature yet.");
            b.blank();
            b.para(
                FG_TEXT,
                "Your signature is a key this computer makes. Everything you publish is signed with it, so anyone can check that it came from you. Nobody issues it and nobody can take it away: there is no account and no password.",
            );
            b.blank();
            b.button(Item::Create, "Create my signature");
            b.button(Item::Import, "Use a key file I already have…");
            b.blank();
            b.para(FG_DIM, &format!("It will be kept in {}.", s.key_path.display()));
            return;
        };
        b.text(FG_DIM, "Your signature (public key):");
        b.text(FG_KEY, &format!("  {}", to_hex(&key)));
        b.blank();
        b.text(FG_DIM, "Your feed address, to give to people:");
        if let Some((a, hinted)) = &s.address {
            for l in wrap(a, BODY_WIDTH - 2) {
                b.text(FG_TEXT, &format!("  {l}"));
            }
            b.button(Item::CopyAddress, "Copy the address");
            if !hinted {
                b.para(FG_DIM, "Only people who use the same relay as you will find it: no public encrypted (wss://) relay is in your profile or relay list.");
            }
        }
        b.blank();
        b.text(FG_DIM, "Its secret half is kept in:");
        for l in wrap(&s.key_path.display().to_string(), BODY_WIDTH - 2) {
            b.text(FG_TEXT, &format!("  {l}"));
        }
        b.blank();
        b.para(
            FG_KEY,
            "Keep a copy of this file somewhere safe, such as a USB stick. Whoever has it can publish as you, and if you lose it you can never add to this feed again. Tersenet never shows the secret key itself.",
        );
    }

    fn profile(&self, b: &mut Body, s: &State) {
        if s.key.is_none() {
            b.para(FG_TEXT, "A profile needs a signature: make yours first.");
            b.blank();
            b.button(Item::GoSignature, "Go to the Signature page");
            return;
        }
        b.para(FG_DIM, "Your profile is the first entry of your feed: the name readers show beside your key, and a few words about you.");
        b.blank();
        b.field(self, Item::Name, "Name ");
        b.field(self, Item::About, "About");
        b.check(Item::Listed, self.listed, "Let search find my name");
        b.blank();
        let listed = account::profile_relays(&s.in_use);
        b.text(FG_DIM, "Relays your profile names (encrypted ones only):");
        if listed.is_empty() {
            b.text(FG_TEXT, "  none");
        }
        for r in &listed {
            b.text(FG_TEXT, &format!("  {}", fit(r, BODY_WIDTH - 2)));
        }
        let plain = account::plain_left_out(&s.in_use);
        if !plain.is_empty() {
            b.para(FG_DIM, &format!("Not named: {}: a profile lists only encrypted (wss://) relays; plain ones stay in your own relay list.", plain.join(", ")));
        }
        b.blank();
        b.button(Item::Save, "Save and publish");
        b.blank();
        match &s.profile {
            Some((name, seq, _, listed)) => b.text(
                FG_DIM,
                &format!("Published now: “{}”, entry #{seq} of your feed, {}.", fit(name, 40), if *listed { "listed" } else { "not listed" }),
            ),
            None => b.text(FG_DIM, "No profile published yet."),
        }
    }

    fn publish(&self, b: &mut Body, s: &State) {
        b.para(FG_DIM, "An article is a plain text file: # starts a heading, a blank line ends a paragraph. The manual's page “Write and publish an article” shows the rest.");
        b.blank();
        b.field(self, Item::File, "File ");
        b.button(Item::Choose, "Choose a file…");
        b.field(self, Item::Title, "Title");
        b.para(FG_DIM, "Left empty, the title is the file's first heading.");
        b.blank();
        b.button(Item::Preview, "Preview");
        if s.key.is_none() {
            b.blank();
            b.para(FG_DIM, "You have no signature yet: you can preview an article, but publishing needs one (the Signature page).");
        } else if !s.follows.iter().any(|(k, _)| Some(*k) == s.key) {
            b.blank();
            b.para(FG_DIM, "Your own feed is not in your timeline: follow it on the Follow page to see what you publish.");
        }
    }

    fn follow(&self, b: &mut Body, s: &State) {
        b.para(FG_DIM, "Paste a feed's address (Ctrl+V) to follow it, or type a name to search for feeds that let search find them.");
        b.blank();
        b.field(self, Item::Query, "Address or name");
        b.button(Item::Search, "Follow or search");
        if let Some(found) = &self.found {
            b.blank();
            b.text(FG_DIM, &format!("Found ({}): Enter follows", found.len()));
            for (i, (k, name)) in found.iter().enumerate() {
                let have = s.follows.iter().any(|(f, _)| f == k);
                b.row(Item::Found(i), &format!("{}  {}{}", fit(name, 40), short_key(k), if have { "  (followed)" } else { "" }));
            }
        }
        b.blank();
        b.text(FG_DIM, &format!("Feeds you follow ({}): Enter or Delete unfollows", s.follows.len()));
        if s.follows.is_empty() {
            b.text(FG_TEXT, "  none yet");
        }
        for (i, (k, name)) in s.follows.iter().enumerate() {
            let you = if Some(*k) == s.key { "  (you)" } else { "" };
            let name = name.as_deref().map_or(String::new(), |n| format!("{}  ", fit(n, 40)));
            b.row(Item::Followed(i), &format!("{name}{}{you}", short_key(k)));
        }
        if let Some(k) = s.key
            && !s.follows.iter().any(|(f, _)| *f == k)
        {
            b.blank();
            b.button(Item::FollowOwn, "Follow my own feed");
        }
    }

    fn relays(&self, b: &mut Body, s: &State) {
        b.para(FG_DIM, "Relays carry your feed to others and theirs to you. Changes here take effect the next time Tersenet starts.");
        b.blank();
        b.text(FG_DIM, "This window uses:");
        if s.in_use.is_empty() {
            b.text(FG_TEXT, "  no relay");
        }
        for r in &s.in_use {
            b.text(FG_TEXT, &format!("  {}", fit(r, BODY_WIDTH - 2)));
        }
        b.blank();
        b.text(FG_DIM, "Your relay list: Enter or Delete takes one out");
        if let Some(p) = &s.relay_problem {
            b.para(FG_TEXT, &plain_error(p));
        } else if s.relay_list.is_empty() {
            let local = terse_net::local::local_url(terse_net::local::local_port());
            b.para(FG_TEXT, &format!("  empty: Tersenet uses the relay on this computer, {local}, and starts one if none runs (the default)."));
        }
        for (i, (_, line)) in s.relay_list.iter().enumerate() {
            b.row(Item::Relay(i), &fit(line, BODY_WIDTH - 2));
        }
        b.blank();
        b.field(self, Item::RelayField, "Add a relay");
        b.para(FG_DIM, "A name such as relay.example.org is encrypted (wss://); a plain one is written ws://host:port.");
        b.blank();
        b.field(self, Item::Options, "Options    ");
        b.para(FG_DIM, "For advanced use, as a line of the relay list: pin=sha256:…, authority=<file> or only-authority=<file>.");
        b.blank();
        b.button(Item::Add, "Add");
    }

    // ------------------------------------------------------------ drawing

    /// The screens as the host draws them: rows 1 to 29 (row 0 is left
    /// as it is: the shell's bar).
    pub fn render(&self) -> Terminal {
        let mut t = Terminal::default();
        t.set_full_text(true);
        // The bar: the pages, the one shown marked.
        let mut bar = vec![span(FG_KEY, BG_BAND, " You ")];
        bar.push(span(FG_DIM, BG_BAND, "· "));
        for (tab, name) in TABS {
            let (fg, bg) = match (tab == self.tab, self.focus == Item::Tabs && self.modal.is_none()) {
                (true, true) => (FG_INK, BG_KEY),
                (true, false) => (FG_KEY, BG_BASE),
                _ => (FG_TEXT, BG_BAND),
            };
            bar.push(span(fg, bg, format!(" {name} ")));
            bar.push(span(FG_TEXT, BG_BAND, " "));
        }
        draw(&mut t, BAR_ROW, BG_BAND, &bar);
        draw(&mut t, BAR_ROW + 1, BG_BASE, &[]);
        let keys: &str;
        match &self.modal {
            Some(Modal::Chooser(c)) => {
                let what = if c.purpose == Purpose::Article { "Choose the article's file" } else { "Choose your key file" };
                draw(&mut t, BAR_ROW + 1, BG_BASE, &[span(FG_KEY, BG_BASE, format!("  {what}: ")), span(FG_TEXT, BG_BASE, fit_left(&c.dir.display().to_string(), BODY_WIDTH - what.len() - 2))]);
                let scroll = chooser_scroll(c);
                for r in 0..BODY_ROWS {
                    let row = BODY_TOP + r;
                    match c.entries.get(scroll + r) {
                        Some((name, dir, size)) => {
                            let sel = scroll + r == c.selected;
                            let bg = if sel { BG_KEY } else { BG_BASE };
                            let fg = if sel { FG_INK } else if *dir { FG_KEY } else { FG_TEXT };
                            let label = if *dir && name != ".." { format!("{name}/") } else { name.clone() };
                            let right = if *dir { String::new() } else { size_text((*size).min(u32::MAX as u64) as u32) };
                            let room = BODY_WIDTH - 12;
                            let label = fit(&label, room);
                            let pad = room.saturating_sub(label.chars().map(width).sum());
                            draw(&mut t, row, BG_BASE, &[span(FG_TEXT, BG_BASE, "  "), span(fg, bg, format!(" {label}{} {right:>9} ", " ".repeat(pad)))]);
                        }
                        None if r == 0 => draw(&mut t, row, BG_BASE, &[span(FG_DIM, BG_BASE, "  This folder is empty.")]),
                        None => draw(&mut t, row, BG_BASE, &[]),
                    }
                }
                if let Some(p) = &c.problem {
                    self.message_rows(&mut t, Some(&(false, vec![plain_error(p)])));
                } else {
                    self.message_rows(&mut t, None);
                }
                keys = " ↑↓ choose   Enter open   Backspace the folder above   Esc back";
            }
            Some(Modal::Preview(p, at)) => {
                let head = format!("  Preview: “{}”, {}", fit(&p.title, 50), size_text(p.data.len() as u32));
                draw(&mut t, BAR_ROW + 1, BG_BASE, &[span(FG_KEY, BG_BASE, head)]);
                let lines = preview_lines(p);
                for r in 0..BODY_ROWS {
                    match lines.get(at + r) {
                        Some((fg, l)) => draw(&mut t, BODY_TOP + r, BG_BASE, &[span(*fg, BG_BASE, format!("  {}", fit(l, BODY_WIDTH)))]),
                        None => draw(&mut t, BODY_TOP + r, BG_BASE, &[]),
                    }
                }
                let refused = p.refusal.as_ref().map(|r| (false, vec![plain_error(r)]));
                self.message_rows(&mut t, refused.as_ref().or(self.message.as_ref()));
                keys = " P or Enter publish…   ↑↓ PgUp PgDn scroll   Esc back";
            }
            _ => {
                let lines = self.body();
                let scroll = self.scroll(&lines);
                for r in 0..BODY_ROWS {
                    match lines.get(scroll + r) {
                        Some(l) => {
                            let mut spans = vec![span(FG_TEXT, BG_BASE, "  ")];
                            spans.extend(l.spans.iter().cloned());
                            draw(&mut t, BODY_TOP + r, BG_BASE, &spans);
                        }
                        None => draw(&mut t, BODY_TOP + r, BG_BASE, &[]),
                    }
                }
                self.message_rows(&mut t, self.message.as_ref());
                keys = if self.focus == Item::Tabs {
                    " ←→ or 1-5 choose a page   ↓ Enter go in   Esc or F10 closes"
                } else if self.focus.is_field() {
                    " Type, Ctrl+V pastes   Enter does it   ↑↓ Tab move   Esc or F10 closes"
                } else {
                    " ↑↓ Tab move   Enter or Space does it   Esc or F10 closes"
                };
            }
        }
        draw(&mut t, KEYS_ROW, BG_BAND, &[span(FG_DIM, BG_BAND, keys)]);
        if let Some(Modal::Confirm(c)) = &self.modal {
            draw(&mut t, CONFIRM_ROW, BG_BAND, &[span(FG_KEY, BG_BAND, " Tersenet · please confirm")]);
            draw(&mut t, CONFIRM_ROW + 1, BG_BASE, &[span(FG_TEXT, BG_BASE, format!(" {}", fit(&c.what, COLS - 2)))]);
            draw(&mut t, CONFIRM_ROW + 2, BG_BASE, &[span(FG_DIM, BG_BASE, format!(" {}", fit(&c.then, COLS - 2)))]);
            draw(
                &mut t,
                CONFIRM_ROW + 3,
                BG_BASE,
                &[
                    span(FG_TEXT, BG_BASE, " Go ahead?   "),
                    span(FG_KEY, BG_BASE, "Y"),
                    span(FG_TEXT, BG_BASE, " yes     "),
                    span(FG_KEY, BG_BASE, "N"),
                    span(FG_TEXT, BG_BASE, " or "),
                    span(FG_KEY, BG_BASE, "Esc"),
                    span(FG_TEXT, BG_BASE, " no"),
                ],
            );
        }
        t.command(0x06); // no cursor
        t
    }

    /// The message rows: what is being done, or the last result.
    fn message_rows(&self, t: &mut Terminal, message: Option<&(bool, Vec<String>)>) {
        let mut rows: Vec<Vec<Span>> = Vec::new();
        if let Some(doing) = self.busy {
            rows.push(vec![span(FG_KEY, BG_BASE, format!("  Working: {doing}…"))]);
        } else if let Some((ok, lines)) = message {
            for (i, l) in lines.iter().enumerate() {
                let (fg, text) = match (i, ok) {
                    (0, true) => (FG_KEY, l.clone()),
                    (0, false) => (FG_TEXT, format!("Not done: {l}")),
                    _ => (FG_DIM, capital(l)),
                };
                rows.extend(wrap(&text, BODY_WIDTH).into_iter().map(|w| vec![span(fg, BG_BASE, format!("  {w}"))]));
            }
        }
        for r in 0..MESSAGE_ROWS {
            draw(t, MESSAGE_ROW + r, BG_BASE, rows.get(r).map_or(&[][..], |v| v.as_slice()));
        }
    }

    /// The screens' text (rows 1 to 29), for `--drive`'s `dump host`.
    pub fn screen_text(&self) -> String {
        self.render().text().lines().skip(1).collect::<Vec<_>>().join("\n") + "\n"
    }
}

/// The pages' columns on the bar.
fn tab_ranges() -> Vec<(Tab, std::ops::Range<usize>)> {
    let mut col = " You ".len() + "· ".chars().count();
    TABS.iter()
        .map(|(tab, name)| {
            let w = name.len() + 2;
            let r = col..col + w;
            col += w + 1;
            (*tab, r)
        })
        .collect()
}

/// The columns of "Y yes" and "N or Esc no" on the confirmation's last row.
fn confirm_keys() -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    (13..18, 23..34)
}

fn chooser_scroll(c: &Chooser) -> usize {
    (c.selected + 1).saturating_sub(BODY_ROWS)
}

/// The preview's lines: the article, then what publishing does with its links.
fn preview_lines(p: &Preview) -> Vec<(u8, String)> {
    let mut v = vec![(FG_TEXT, String::new())];
    v.extend(p.lines.iter().cloned());
    if !p.links.is_empty() {
        v.push((FG_TEXT, String::new()));
        v.push((FG_DIM, "── its links, and what publishing does with each ──".into()));
        for l in &p.links {
            v.extend(wrap(l, BODY_WIDTH).into_iter().map(|l| (FG_DIM, l)));
        }
    }
    v
}

/// `s` with its first letter up.
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// `s` cut from the left with an ellipsis to `cols` (a path: its end matters).
fn fit_left(s: &str, cols: usize) -> String {
    let total: usize = s.chars().map(width).sum();
    if total <= cols {
        return s.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut w = 1;
    for c in s.chars().rev() {
        if w + width(c) > cols {
            break;
        }
        w += width(c);
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// An error in plain words: the first letter up, a full stop at the end.
fn plain_error(e: &str) -> String {
    let mut s: String = e.trim().to_string();
    if let Some(c) = s.chars().next() {
        s.replace_range(..c.len_utf8(), &c.to_uppercase().to_string());
    }
    if !s.ends_with(['.', ')', '?']) {
        s.push('.');
    }
    s
}

/// The body as it is built: lines, and the item each lets the user act on.
struct Body {
    lines: Vec<Line>,
    focus: Item,
}

impl Body {
    fn text(&mut self, fg: u8, text: &str) {
        self.lines.push(Line { spans: vec![span(fg, BG_BASE, fit(text, BODY_WIDTH))], item: None });
    }

    fn para(&mut self, fg: u8, text: &str) {
        for l in wrap(text, BODY_WIDTH) {
            self.text(fg, &l);
        }
    }

    fn blank(&mut self) {
        self.lines.push(Line { spans: Vec::new(), item: None });
    }

    fn button(&mut self, item: Item, label: &str) {
        let s = if self.focus == item { span(FG_INK, BG_KEY, format!(" {label} ")) } else { span(FG_KEY, BG_BAND, format!(" {label} ")) };
        self.lines.push(Line { spans: vec![s], item: Some(item) });
    }

    fn check(&mut self, item: Item, on: bool, label: &str) {
        let (fg, bg) = if self.focus == item { (FG_INK, BG_KEY) } else { (FG_TEXT, BG_BASE) };
        let mark = if on { "[x]" } else { "[ ]" };
        self.lines.push(Line { spans: vec![span(fg, bg, format!("{mark} {label}")), span(FG_DIM, BG_BASE, "   Space switches it")], item: Some(item) });
    }

    /// A list row.
    fn row(&mut self, item: Item, text: &str) {
        let (fg, bg) = if self.focus == item { (FG_INK, BG_KEY) } else { (FG_TEXT, BG_BASE) };
        self.lines.push(Line { spans: vec![span(fg, bg, format!(" {} ", fit(text, BODY_WIDTH - 2)))], item: Some(item) });
    }

    /// A labelled field, its text scrolled to keep the caret in view.
    fn field(&mut self, you: &You, item: Item, label: &str) {
        let f = you.field(item);
        let focused = self.focus == item;
        let room = BODY_WIDTH - label.chars().count() - 3;
        let shown: Vec<char> = f.text.chars().map(|c| if c == '\n' { '¶' } else { c }).collect();
        // The first character shown: the caret stays inside.
        let mut first = 0;
        let w = |a: usize, b: usize| shown[a..b].iter().map(|&c| width(c)).sum::<usize>();
        while w(first, f.caret.min(shown.len())) + 1 > room {
            first += 1;
        }
        let mut before = String::new();
        let mut used = 0;
        for &c in &shown[first..f.caret.min(shown.len())] {
            before.push(c);
            used += width(c);
        }
        let mut spans = vec![span(if focused { FG_KEY } else { FG_DIM }, BG_BASE, format!("{label} ")), span(FG_TEXT, BG_FIELD, before)];
        let mut after = String::new();
        let mut rest = shown.get(f.caret..).unwrap_or(&[]).iter();
        if focused {
            let c = rest.next().copied().unwrap_or(' ');
            spans.push(span(FG_INK, BG_KEY, c.to_string()));
            used += width(c);
        }
        for &c in rest {
            if used + width(c) > room {
                break;
            }
            after.push(c);
            used += width(c);
        }
        spans.push(span(FG_TEXT, BG_FIELD, after));
        spans.push(span(FG_TEXT, BG_FIELD, " ".repeat(room.saturating_sub(used))));
        self.lines.push(Line { spans, item: Some(item) });
    }
}

/// Draws a row: `bg` across it, then the spans from the left, cut at the
/// right edge.
fn draw(t: &mut Terminal, row: usize, bg: u8, spans: &[Span]) {
    t.set_colours(FG_TEXT, bg);
    t.set_cursor(0, row as u32);
    t.command(0x04); // clear to the end of the row
    let mut col = 0;
    for s in spans {
        let room = COLS.saturating_sub(col);
        if room == 0 {
            break;
        }
        let text: String = if s.text.chars().map(width).sum::<usize>() > room { fit(&s.text, room) } else { s.text.clone() };
        let w: usize = text.chars().map(width).sum();
        t.set_cursor(col as u32, row as u32);
        t.set_colours(s.fg, s.bg);
        t.write(text.as_bytes());
        col += w;
    }
}

/// `shown` (the shell's screen) with the host's screens over rows 1 on.
pub fn over(shown: &Terminal, screen: &Terminal) -> Terminal {
    let mut t = shown.clone();
    t.set_full_text(true);
    let mut cells = vec![0u8; (ROWS - 1) * COLS * 4];
    screen.cells_read(COLS, &mut cells);
    t.cells_write(COLS, &cells);
    t.command(0x06);
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn setup(dir: &Path) -> Setup {
        let docs = dir.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        Setup {
            config: dir.join("config"),
            store: dir.join("store"),
            relays: Vec::new(),
            feed_relays: false,
            documents: docs,
            home: dir.to_path_buf(),
        }
    }

    /// Waits for the thread's work and takes it in.
    fn settle(y: &mut You) {
        let end = Instant::now() + Duration::from_secs(20);
        y.poll();
        while y.busy() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
            y.poll();
        }
        assert!(!y.busy(), "the screens' work did not finish");
    }

    fn typed(y: &mut You, s: &str) {
        for c in s.chars() {
            y.text(c);
        }
    }

    #[test]
    fn fields_edit_and_wrap() {
        let mut f = Field::new(5);
        f.set("héllo!");
        assert_eq!((f.text.as_str(), f.caret), ("héll", 4));
        f.key(Key::Left);
        f.key(Key::Backspace);
        assert_eq!(f.text, "hél");
        f.key(Key::Home);
        f.key(Key::Delete);
        f.insert('\u{7}');
        assert_eq!((f.text.as_str(), f.caret), ("él", 0));
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\n\nb", 4), ["a", "", "b"]);
        assert_eq!(plain_error("no relay answered"), "No relay answered.");
        assert_eq!((capital("taken by 1"), fit_left("/a/long/path", 7)), ("Taken by 1".to_string(), "…g/path".to_string()));
        let lines = render_article("# Hi there\n\nSome words here.\n\n- one\n- two\n", 20);
        let text: Vec<&str> = lines.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(text, ["HI THERE", "", "Some words here.", "", "• one", "• two"]);
        assert_eq!(lines[0].0, FG_KEY);
    }

    #[test]
    fn documents_folder() {
        let home = tempfile::tempdir().unwrap();
        // Without Documents (and with no user-dirs file in this home), the home.
        if std::env::var_os("XDG_DOCUMENTS_DIR").is_none() && std::env::var_os("XDG_CONFIG_HOME").is_none() {
            assert_eq!(documents_dir(home.path()), home.path());
            std::fs::create_dir(home.path().join("Documents")).unwrap();
            assert_eq!(documents_dir(home.path()), home.path().join("Documents"));
            std::fs::create_dir_all(home.path().join(".config")).unwrap();
            std::fs::create_dir(home.path().join("Papers")).unwrap();
            std::fs::write(home.path().join(".config/user-dirs.dirs"), "XDG_DOCUMENTS_DIR=\"$HOME/Papers\"\n").unwrap();
            assert_eq!(documents_dir(home.path()), home.path().join("Papers"));
        }
    }

    /// A first run: the signature page; one action makes the key, which
    /// is never shown; the address can be copied; the profile, an article
    /// (chosen, previewed, confirmed), following and the relay list work
    /// through the same code as `terse`, without a relay here.
    #[test]
    fn walk_every_page_without_a_relay() {
        let dir = tempfile::tempdir().unwrap();
        let s = setup(dir.path());
        std::fs::write(s.documents.join("hello.txt"), "# Hello, Tersenet\n\nMy first article.\n").unwrap();
        std::fs::create_dir(s.documents.join("old")).unwrap();
        let mut y = You::new(s.clone());
        settle(&mut y);
        let t = y.screen_text();
        assert!(t.contains("You have no signature yet") && t.contains("Create my signature"), "{t}");
        // Down to the first button, Enter.
        y.key(Key::Down);
        assert_eq!(y.focus, Item::Create);
        y.key(Key::Enter);
        settle(&mut y);
        let key = account::load_identity(&s.config).unwrap().feed_key();
        let secret = std::fs::read_to_string(account::identity_path(&s.config)).unwrap();
        let t = y.screen_text();
        assert!(t.contains(&to_hex(&key)) && t.contains("identity.key") && t.contains("Keep a copy"), "{t}");
        assert!(!t.contains(secret.trim()), "the secret key is never shown");
        assert!(t.contains("Your signature is made"), "{t}");
        y.focus = Item::CopyAddress;
        assert_eq!(y.key(Key::Enter), Effect::Copy(to_hex(&key)));
        // Profile: name, about, listed, confirm.
        y.text('2');
        assert_eq!(y.tab, Tab::Profile);
        y.focus = Item::Name;
        typed(&mut y, "Sam");
        y.key(Key::Down);
        typed(&mut y, "I write about birds.");
        y.focus = Item::Listed;
        y.text(' ');
        assert!(y.listed);
        y.focus = Item::Save;
        y.key(Key::Enter);
        assert!(y.screen_text().contains("Publish your profile with the name “Sam”?"), "{}", y.screen_text());
        y.text('y');
        settle(&mut y);
        let t = y.screen_text();
        assert!(t.contains("Your profile is published as entry #1"), "{t}");
        assert!(t.contains("Published now: “Sam”, entry #1 of your feed, listed"), "{t}");
        // Publish: choose the file, preview, publish.
        y.text('3');
        y.focus = Item::Choose;
        y.key(Key::Enter);
        let t = y.screen_text();
        assert!(t.contains("old/") && t.contains("hello.txt"), "{t}");
        y.key(Key::Down); // .. is first
        y.key(Key::Down);
        y.key(Key::Enter);
        settle(&mut y);
        let t = y.screen_text();
        assert!(t.contains("Preview: “Hello, Tersenet”") && t.contains("HELLO, TERSENET") && t.contains("My first article."), "{t}");
        y.text('p');
        assert!(y.screen_text().contains("Publish “Hello, Tersenet”"), "{}", y.screen_text());
        y.key(Key::Escape);
        assert!(y.screen_text().contains("Nothing was done"));
        y.modal = None;
        y.focus = Item::Preview;
        y.key(Key::Enter);
        settle(&mut y);
        y.key(Key::Enter);
        y.text('y');
        settle(&mut y);
        let t = y.screen_text();
        assert!(t.contains("“Hello, Tersenet” is published as entry #2 of your feed"), "{t}");
        assert!(t.contains("follow it on the Follow page"), "{t}");
        let feeds = terse_net::FeedStore::beside_blobs(&s.store);
        assert_eq!(feeds.seqs(&key).unwrap(), [1, 2]);
        // Follow: the own feed by its button, another by a pasted address.
        y.text('4');
        y.focus = Item::FollowOwn;
        y.key(Key::Enter);
        settle(&mut y);
        let other = to_hex(&terse_net::Identity::from_seed([9; 32]).feed_key());
        y.focus = Item::Query;
        typed(&mut y, &other);
        y.key(Key::Enter);
        settle(&mut y);
        let t = y.screen_text();
        assert!(t.contains("Feeds you follow (2)") && t.contains("(you)"), "{t}");
        let subs = account::load_subscriptions(&s.config).unwrap();
        assert_eq!(subs.feeds().count(), 2);
        // Unfollow asks first.
        y.focus = Item::Followed(1);
        y.key(Key::Delete);
        assert!(y.screen_text().contains("Stop following"), "{}", y.screen_text());
        y.text('y');
        settle(&mut y);
        assert_eq!(account::load_subscriptions(&s.config).unwrap().feeds().count(), 1);
        // A name with no relay to ask: said plainly.
        y.focus = Item::Query;
        typed(&mut y, "Ana");
        y.key(Key::Enter);
        settle(&mut y);
        assert!(y.screen_text().contains("Not done: There is no relay to ask"), "{}", y.screen_text());
        // Relays: add (taken as wss://), a bad one, remove with a confirmation.
        y.text('5');
        assert_eq!(y.query.text, "Ana5", "a digit typed in a field is text");
        y.key(Key::Up);
        y.key(Key::Up);
        assert_eq!(y.focus, Item::Tabs);
        y.text('5');
        assert_eq!(y.tab, Tab::Relays);
        assert!(y.screen_text().contains("empty: Tersenet uses the relay on this computer"), "{}", y.screen_text());
        y.focus = Item::RelayField;
        typed(&mut y, "relay.example.org");
        y.key(Key::Enter);
        settle(&mut y);
        let t = y.screen_text();
        assert!(t.contains("Added wss://relay.example.org") && t.contains("replaces the relay on this computer"), "{t}");
        y.focus = Item::RelayField;
        typed(&mut y, "not a relay");
        y.key(Key::Enter);
        settle(&mut y);
        assert!(y.screen_text().contains("Not done: Not a relay"), "{}", y.screen_text());
        y.focus = Item::Relay(0);
        y.key(Key::Enter);
        y.text('n');
        assert_eq!(account::relay_list(&s.config).unwrap().len(), 1);
        y.key(Key::Enter);
        y.text('y');
        settle(&mut y);
        assert!(account::relay_list(&s.config).unwrap().is_empty());
        assert_eq!(y.key(Key::Escape), Effect::Close);
    }

    /// Importing a key file, by the chooser, with a confirmation; a second
    /// key is never put over the first.
    #[test]
    fn import_a_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = setup(dir.path());
        let other = tempfile::tempdir().unwrap();
        let made = account::create_identity(other.path()).unwrap();
        std::fs::copy(account::identity_path(other.path()), dir.path().join("saved.key")).unwrap();
        let mut y = You::new(s.clone());
        settle(&mut y);
        y.focus = Item::Import;
        y.key(Key::Enter);
        let Some(Modal::Chooser(c)) = &y.modal else { panic!("no chooser") };
        let i = c.entries.iter().position(|e| e.0 == "saved.key").unwrap();
        for _ in 0..i {
            y.key(Key::Down);
        }
        y.key(Key::Enter);
        assert!(y.screen_text().contains("Use the key in"), "{}", y.screen_text());
        y.text('y');
        settle(&mut y);
        assert_eq!(account::load_identity(&s.config).unwrap().feed_key(), made.feed_key());
        assert!(y.screen_text().contains(&to_hex(&made.feed_key())));
        // Mouse: a click on a page's name shows it.
        let (tab, range) = tab_ranges()[3].clone();
        y.click((range.start as u16 + 1) * 8, BAR_ROW as u16 * 16 + 4);
        assert_eq!(y.tab, tab);
    }

    #[test]
    fn drawn_over_the_shell_from_row_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut y = You::new(setup(dir.path()));
        settle(&mut y);
        let mut shell = Terminal::default();
        shell.write(b"the shell's bar");
        let t = over(&shell, &y.render());
        let text = t.text();
        let rows: Vec<&str> = text.lines().collect();
        assert!(rows[0].starts_with("the shell's bar"), "{text}");
        assert!(rows[1].contains("Signature") && rows[1].contains("Relays"), "{text}");
        assert!(rows[ROWS - 1].contains("Esc"), "{text}");
        for r in rows {
            assert!(r.chars().map(width).sum::<usize>() <= COLS);
        }
    }
}
