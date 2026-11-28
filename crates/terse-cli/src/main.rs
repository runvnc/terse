//! `terse`: headless runner, image tools and feeds.

mod build;
mod drivers;
mod feed;
mod node;
mod perf;
mod perflog;
mod report;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use terse_core::{DeviceReq, Image, Machine, Script, Stopped, System};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{Consent, ContentService, Done, HeadlessSystem, Node, blob_id, builtin, local, to_hex};

/// Frames `run` executes when neither `--frames` nor an exit stops it (10 minutes).
const DEFAULT_FRAME_CAP: u64 = 60 * 60 * 10;

const USAGE: &str = "usage:
  terse run <image|id> [--frames N] [--dump [--dump-vectors] [--dump-page]] [--events <file>] [--relay ws://...]
            [--store <dir>] [--config <dir>] [--data <dir>] [--consent allow|deny] [--hash] [--hash-every N] [--bench]
            [--save-state <file>] [--load-state <file>] [--record <file>] [--profile reading|program] [--with <slot>]...
            [--without <slot>]... [--audio-out <file.wav>] [--work]
  terse run <image> --replay <file> [--frames N] [--dump ...] [--hash] [--hash-every N] [--bench] [--save-state <file>]
            [--audio-out <file.wav>]
  terse run --shell [<image|id>] [--builtin <image|id>]... [--hash-all] [the options of run]
  terse info <image|id> [--relay ws://...|none] [--store <dir>] [--config <dir>]
  terse put <file> [--relay ws://...|none] [--store <dir>] [--config <dir>]
  terse get <id> [-o <file>] [--relay ws://...|none] [--store <dir>] [--config <dir>]
  terse hash <file>
  terse build <source>... [-o <out.terse>] [--title <t>] [--ram-pages <n>] [--device <slot:version>]...
              [--optional <slot:version>]... [--cflag <flag>]... [--clang <path>] [--lld <path>] [--rustc <path>]
              [--show] [--run [the options of run]...]   (C, assembly or Rust to an image; `terse build` alone: details)
  terse build --write-lib <dir>   (the C library, linker script and Rust runtime built into terse)
  terse new <file.c|file.rs|file.S>   (a starter program)
  terse pack --rom <raw.bin> --title <t> --ram-pages <n> [--device <slot:version>]... [--optional <slot:version>[=<name>]]...
             -o <out.terse>
  terse key new|show|import <file>
  terse feed publish <file> --title <t> [--reply-to <id>] [--dry-run] [--force]
  terse feed publish --program <image.terse> [--name <n>]
  terse feed programs [<key>]
  terse feed subscribe|unsubscribe <address>
  terse feed address
  terse feed subscriptions
  terse feed sync [--watch] [--auto-download <bytes>] [--no-prefetch] [--no-program-download]
  terse feed list [<key>]
  terse feed read <key> <seq>
  terse feed nickname <key> [<name>]
  terse feed site [<address>] --out <dir> [--title <t>] [--base-url <https://...>] [--force]
  terse profile set --name <n> [--listed|--unlisted] [--description <file>]
  terse profile show [<key>]
  terse find [--entries|--programs] <text>
  terse records list [<owner>] | show <owner> <name> | delete <owner> <name>   [--data <dir>]
  terse manual build|check [--dir <docs/manual>]
  terse manual site --out <dir> [--dir <docs/manual>]   (the manual as a static website)
  terse manual publish   (the manual built into terse, as articles of your feed)
  terse vectors [--dir <vectors>]   (the core test vectors: blobs, entries, profiles, relay messages, articles)
  terse perf [--only machine|client|host] [--runs <n>] [--programs <dir>] [--host <terse-host>] [--reader <tersereader>]
             (the responsiveness scenarios of PERFORMANCE.md, headless, with relays started on ephemeral ports)
  terse perf log [<dir or file>]   (the performance records of RECORD.md, summarised; default: the data
             directory's perf/: late frames by component and program, worst frames, memory, start-ups)
  terse perf report [--days <n>] [--to <date>] [--host <name>] [--run] [--programs <n>] [--out <file>] [--publish [--yes]]
             (a performance report from the record, RECORD.md 5, printed in full; published only with --publish,
             after you confirm: it is public, signed with your feed key, and cannot be deleted)
  terse perf report --check <file>   (whether a file is a valid report)
  terse perf aggregate [--follow <keys file>] <report file or dir>...   (verdicts per host and device, RECORD.md 5.5)
  terse node [--socket <path>] [--relay ws://...|none]... [--store <dir>] [--config <dir>] [--no-feed-relays]
             [--auto-download <bytes>] [--no-prefetch] [--no-program-download]
             (a node for readers on a local socket, LOCAL.md: it follows your subscriptions on your relays
             and serves them until stopped; Unix only)
  terse relay add|remove|set <wss://host | ws://host:port | host>   (a host alone: wss://host; localhost: ws://)
        add and set take options for the relay: authority=<pem> only-authority=<pem> pin=sha256:<hex>
  terse relay list|show
  terse relay fingerprint <cert.pem>   (the pin= values of a certificate and of its key)
  terse fonts list | add <file> | remove <file> | set mono|sans|bold <file>|default
  terse drivers list | add <file> [--slot <n>] [--name <name>] | remove <name> | use <slot> <name>|default
  terse --version
  (key, feed, profile, find and relay commands all take [--relay ws://...|none]... [--store <dir>]
  [--config <dir>] [--no-feed-relays])

node listens on --socket, else $TERSE_NODE_SOCKET, else $XDG_RUNTIME_DIR/terse/node.sock (or
/tmp/terse-<uid>/node.sock); it refuses to start if a node already answers there, and removes the
socket when it stops (Ctrl-C, SIGTERM). Readers' changes (subscriptions, nicknames, relays) are saved
in --config as the other commands save them.

Relays: info, put, get, feed publish, feed sync, feed read, profile set, find and node use the
--relay options if given (several may be; `none`: no network), else the relays listed in
<config>/relays (`terse relay add` and `set` write it; an older <config>/relay file is read
if there is no list), else a relay running on this machine (ws://127.0.0.1:7878), else none
(sync then fails). A relay written without a scheme is wss:// (encrypted) whatever its port, but
localhost or a loopback address is ws://; ws://host:port names a plain relay (yours only: hints, profiles
and your own address never name one). A wss:// relay's certificate must be trusted: the usual authorities are
built in; a private authority is trusted from <config>/authorities/*.pem, TERSE_RELAY_CA or
--relay-authority <pem> (every relay), or by a relay's options in <config>/relays or in a --relay
value (authority=, only-authority=, pin=; README \"Private authorities and pins\"). Puts and publishes go to every relay; gets ask them in turn. Feeds
whose profile names relays are also followed there, unless --no-feed-relays. `run` uses
relays only with --relay.

<id> is a 64-hex-digit blob ID, looked up in the store, then the relay; the link terse:<id> is taken too.
<key> is a feed key (64 hex digits). An <address> is a feed key, optionally followed by `@` and
one or two relay hints separated by `,` (host or host:port, port 7878 if left out; NETWORK 4.9),
also written as the link terse:feed/<address>: if your relays do not hold the feed, sync looks
for it on the hinted relays (public addresses only; not with --no-feed-relays) until the feed's
profile names relays. `feed address` prints your own, with hints from your profile's relay
lines, then your relays.
--store defaults to $XDG_CACHE_HOME/terse/blobs (or ~/.cache/terse/blobs; ~/Library/Caches/terse/blobs on macOS; %LOCALAPPDATA%\\terse\\blobs
on Windows); feed
entries are kept in <store>/feeds.
fonts: the fonts terse-host draws with, besides its built-in ones (<config>/fonts, read when it
starts; README, Fonts): add lists an extra font for characters the built-in fonts lack, set
replaces one of them (mono must be monospaced). Each file is checked as it is given.
drivers: device drivers terse-bus uses instead of the ones shipped with it (<config>/drivers, read
when it starts; DEVICES.md 2.2). add runs the driver's handshake once (with terse-bus, next to
terse), shows its name, version, build and slots, copies it into the folder and uses it for its
slots; use chooses an installed driver, or default for the shipped one; list shows which driver
each slot gets and why. A driver is a native program with your full rights: add only ones you
trust. Nothing installs a driver but this command.
--config holds the identity, subscriptions, nicknames, relay list, fonts and drivers; it defaults to
$XDG_CONFIG_HOME/terse (or ~/.config/terse; ~/Library/Application Support/terse on macOS; %APPDATA%\\terse on Windows).
profile set publishes your profile: the display name (1-48 bytes), --listed to let search
find it (default: as in your current profile, else unlisted), the description from a file
(default: kept), and relay lines for your relays that are not loopback addresses (the
--relay options, else your relay list). Names are not unique: listings show the key too.
--auto-download: articles up to this size are fetched as entries arrive (default 16384).
--no-prefetch: do not also fetch the terse: links of those articles (NETWORK 4.4).
--no-program-download: do not fetch the current version of each program of the subscribed feeds
(NETWORK 4.4; by default they are fetched whatever their size, up to the largest image).
feed site writes a feed (default: your own) as a static website to --out: a page per article,
an index newest first, the stylesheet, a page of the feed's programs, and with --base-url (the
site's address) an Atom feed, feed.xml. It fetches the feed's entries first, and on later runs
writes only what changed (a state file, .terse-site, remembers what was written; --force rewrites
everything).
feed publish lists each terse: link of the article first: whether this host holds what it leads
to, whether it was uploaded with the article (what an entry of your feed names, and what no entry
names), and whether relays keep it (an entry of your feed names it) or it is link-only (another
feed's, or unknown here). An entry names one blob, so a relay that drops blobs no entry names
(NETWORK 5.3) keeps a linked article only while some entry names it: a link to a blob held here
that no entry names is warned about (publish that blob as an article of its own to keep it). An
article that links to a blob of your feed this host does not hold is refused unless --force.
--dry-run lists the links and publishes nothing.
feed publish --program publishes an image as a program (NETWORK 4.2) under --name (default: the
image's title); feed programs lists the current version of each program held (of one feed, or of
all the feeds held), find --programs searches programs by name.
Named records (SPEC §content.records), the data programs keep on this machine, are in <data>/records; --data
defaults to $XDG_DATA_HOME/terse (or ~/.local/share/terse; ~/Library/Application Support/terse/data on macOS; %LOCALAPPDATA%\\terse on Windows). `run` keeps them there too. `records`
shows them for inspection: an <owner> is builtin:<title>, program:<feed key>:<name> or
image:<image id>; without one, `records list` lists every owner.
--events: scripted input, one event per line (see README).
--hash prints the state hash (STATE.md) after the last frame; --hash-every N prints
`<counter> <hash>` after every frame that leaves the frame counter at a multiple of N (from
reset: after the first N frames, 2N frames, ...; a frame that stops the machine counts);
--bench prints the emulation time per frame (p50, p99, max) and the multiple of real time.
--work prints, for each machine, the most its displays asked of the host in any frame (PERFORMANCE.md R10):
the 2D list's filled area and stroke ink, the 3D list's ink, length, segments and faces, before the caps.
--save-state writes the machine's snapshot (STATE.md 4) to <file> after the last frame; --load-state
starts the machine from the snapshot in <file> instead of from reset (its image: <image|id> if given, else
the one the snapshot names, by ID); its hash is then the hash at the moment of the snapshot. Not with
--shell. --record writes every input of every frame and the hash after it to <file> (STATE.md 5);
--replay runs such a recording again from <image> (a file) alone: no network, store, data
directory or events, every frame's hash checked (it stops at the first that differs), and it
refuses a recording of another image. --profile reading provides only the machine's reading
profile: devices 1, 2, 4, 7 and 10 (full text), and the content operations get, subscriptions,
timeline, names, identity, subscribe and unsubscribe; every other content request completes with
status 05h (refused) without asking. --profile program provides only the program profile: devices
1, 2 and 3, the terminal with the basic text repertoire only (no device 10, SPEC §terminal.text).
--with <slot> adds a device to a profile (--profile program --with 10: with full text);
--without <slot> withholds one. An image that requires a device the run does not provide is
refused; an optional device it does not provide is absent (SPEC §image).
pack: --device lists a required device, --optional one the program can run without (--device 10:0
or --optional 10:0: full text, SPEC §terminal.text); `info` shows both. An experimental device (slots
240-255, spec/experimental.md) is only optional and named: --optional 240:0=example.dice.
A shell keeps the snapshots its machines' `snapshot` calls make in <data>/snapshots (SPEC §machines.snapshots).
--consent: how `run` answers a program's requests that need consent (subscribe, unsubscribe, publish,
set profile, set nickname; SPEC §content.feeds), since nobody can be asked (default deny).
--shell runs a system (SPEC §machines): the image (default: the built-in shell, programs/shell) is the shell,
with device 8; the built-in programs (reader, sparks, fetch, hello, sketch, holo, reports, reef, sheet), and each --builtin image after
them, are put into the store and offered to it. Scripted events are the host's input: `home` is the
home key, pointer positions are in the whole screen's units, and the system routes them. --dump
prints the shell's screen, then for each machine whose rectangle is not empty a line
`-- machine <handle> <state> <x> <y> <w> <h> <title> --` and its screen; --hash prints the shell's
hash, and --hash-all after it `<handle> <hash>` for each machine the shell has after the last
frame, in handle order (one it stopped is not listed; `-`: no machine, as its image or snapshot is
still being fetched or its start failed).
--audio-out writes what the sound device (device 6, SPEC §sound) of every machine played, mixed, as a
WAV file (48,000 Hz, stereo, 16 bits): 800 sample pairs a frame run, silence for a frame in which
no machine gave any. Nothing is ever played: `run` has no audio output.
--dump-vectors prints the 2D list (device 3) as shapes and the 3D list (device 9) as commands, then
`-- 3d: <n> segments, length <l> units --`: what it draws seen from the reference eye (SPEC §vector3d.caps).
--dump-page prints the page (device 7) as the reference layout lays it out: a header line with
its height and scroll, then the lines in view (y, height, x, the first span's font, the text).
`run` exits with the program's exit code (0-255). If the machine stops with a fault
(SPEC §stopping), it prints the fault and ends by signal SIGTERM, so a parent process can
tell a fault from any exit code (a shell's $status shows 143).";

/// Options for commands that use the blob store and relay.
const NET_OPTIONS: [&str; 5] = ["--relay", "--relay-authority", "--store", "--config", "--no-feed-relays"];

/// The command line as UTF-8. An argument that is not is refused with the
/// option it belongs to (an image title, SPEC §image, is UTF-8).
fn utf8_args() -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for a in std::env::args_os().skip(1) {
        match a.into_string() {
            Ok(s) => out.push(s),
            Err(a) => {
                let shown = a.to_string_lossy();
                return Err(match out.last().map(String::as_str) {
                    Some(opt) if opt.starts_with('-') => format!("{opt} {shown}: not valid UTF-8"),
                    _ => format!("{shown}: not valid UTF-8"),
                });
            }
        }
    }
    Ok(out)
}

fn main() -> ExitCode {
    let args = match utf8_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("terse: {e}");
            return ExitCode::from(2);
        }
    };
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("info") => info(&args[1..]),
        Some("put") => put(&args[1..]),
        Some("get") => get(&args[1..]),
        Some("hash") => hash(&args[1..]),
        Some("pack") => pack(&args[1..]),
        Some("build") => build::build(&args[1..]),
        Some("new") => build::new(&args[1..]),
        Some("key") => feed::key(&args[1..]),
        Some("feed") => feed::feed(&args[1..]),
        Some("relay") => feed::relay(&args[1..]),
        Some("fonts") => feed::fonts(&args[1..]),
        Some("drivers") => drivers::drivers(&args[1..]),
        Some("profile") => feed::profile(&args[1..]),
        Some("find") => feed::find(&args[1..]),
        Some("records") => records(&args[1..]),
        Some("manual") => feed::manual(&args[1..]),
        Some("node") => node::node(&args[1..]),
        Some("vectors") => vectors(&args[1..]),
        Some("perf") if args.get(1).is_some_and(|a| a == "log") => perflog::log(&args[2..]),
        Some("perf") if args.get(1).is_some_and(|a| a == "report") => report::report(&args[2..]),
        Some("perf") if args.get(1).is_some_and(|a| a == "aggregate") => report::aggregate(&args[2..]),
        Some("perf") => perf::perf(&args[1..]),
        Some("--version") => {
            println!("{}", terse_perflog::Identity::this("terse").line());
            return ExitCode::SUCCESS;
        }
        _ => Err(USAGE.to_string()),
    };
    result.unwrap_or_else(|e| {
        eprintln!("terse: {e}");
        ExitCode::from(2)
    })
}

/// Splits arguments into positionals and `--flag value` / `-o value` options.
struct Args {
    positional: Vec<String>,
    options: Vec<(String, Option<String>)>,
}

impl Args {
    /// `switches` are flags without a value.
    fn parse(args: &[String], switches: &[&str]) -> Result<Self, String> {
        let mut out = Args { positional: Vec::new(), options: Vec::new() };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a.starts_with('-') {
                let value = if switches.contains(&a.as_str()) || a == "--no-feed-relays" || a == "--no-prefetch" || a == "--no-program-download"
                {
                    None
                } else {
                    Some(it.next().ok_or(format!("{a} needs a value"))?.clone())
                };
                out.options.push((a.clone(), value));
            } else {
                out.positional.push(a.clone());
            }
        }
        Ok(out)
    }

    fn has(&self, name: &str) -> bool {
        self.options.iter().any(|(n, _)| n == name)
    }

    fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.options.iter().filter(move |(n, _)| n == name).filter_map(|(_, v)| v.as_deref())
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.all(name).last()
    }

    fn require(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or(format!("missing {name}\n{USAGE}"))
    }

    fn check_known(&self, known: &[&str]) -> Result<(), String> {
        match self.options.iter().find(|(n, _)| !known.contains(&n.as_str())) {
            Some((n, _)) => Err(format!("unknown option {n}\n{USAGE}")),
            None => Ok(()),
        }
    }

    fn one_positional(&self) -> Result<&str, String> {
        match self.positional.as_slice() {
            [p] => Ok(p),
            _ => Err(USAGE.to_string()),
        }
    }
}

/// `--data`, else the default data directory (SPEC §content.records).
pub(crate) fn data_dir(args: &Args) -> Result<PathBuf, String> {
    args.get("--data").map(PathBuf::from).or_else(terse_net::default_data_dir).ok_or("no data directory (use --data)".into())
}

/// `terse records list [<owner>]`, `show <owner> <name>`, `delete <owner>
/// <name>`: the named records programs keep on this machine (SPEC §content.records).
fn records(args: &[String]) -> Result<ExitCode, String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let args = Args::parse(rest, &[])?;
    args.check_known(&["--data"])?;
    let records = terse_net::Records::in_data_dir(&data_dir(&args)?);
    let owner = |s: &str| {
        terse_net::Owner::parse(s).ok_or(format!("{s}: not an owner (builtin:<title>, program:<key>:<name> or image:<id>)"))
    };
    let shown = |name: &[u8]| String::from_utf8_lossy(name).chars().map(|c| if c.is_control() { '\u{FFFD}' } else { c }).collect::<String>();
    let io = |e: std::io::Error| format!("{}: {e}", records.root().display());
    match (command.as_str(), args.positional.as_slice()) {
        ("list", []) => {
            for o in records.owners().map_err(io)? {
                let list = records.list(&o).map_err(io)?;
                let bytes: u64 = list.iter().map(|(_, s)| *s as u64).sum();
                println!("{}  {} records, {bytes} bytes", o.describe(), list.len());
            }
        }
        ("list", [o]) => {
            for (name, size) in records.list(&owner(o)?).map_err(io)? {
                println!("{size:>8} B  {}", shown(&name));
            }
        }
        ("show", [o, name]) => {
            let data = records.load(&owner(o)?, name.as_bytes()).map_err(io)?.ok_or(format!("no record {name}"))?;
            std::io::stdout().write_all(&data).map_err(|e| e.to_string())?;
        }
        ("delete", [o, name]) => {
            let o = owner(o)?;
            if records.load(&o, name.as_bytes()).map_err(io)?.is_none() {
                return Err(format!("no record {name}"));
            }
            records.delete(&o, name.as_bytes()).map_err(io)?;
            eprintln!("terse: deleted {name}");
        }
        _ => return Err(USAGE.into()),
    }
    Ok(ExitCode::SUCCESS)
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{path}: {e}"))
}

/// The relays for a command: the `--relay` options (`none`: no network);
/// with `find`, when there are none, the config directory's relay list,
/// else one running on this machine (README, "Getting started").
pub(crate) fn relay_urls(args: &Args, find: bool) -> Result<Vec<String>, String> {
    let given: Vec<String> = args.all("--relay").map(str::to_string).collect();
    for a in args.all("--relay-authority") {
        local::add_relay_authority(std::path::Path::new(a));
    }
    let config = args.get("--config").map(PathBuf::from).or_else(terse_net::default_config_dir);
    if !find {
        // Without a list to fall back on, no --relay means no relay; the
        // settings for wss:// relays still apply.
        if given.is_empty() {
            local::apply_trust(&[], config.as_deref())?;
            return Ok(Vec::new());
        }
        return Ok(local::find_relays(&given, config.as_deref(), 0)?.urls());
    }
    let found = local::find_relays(&given, config.as_deref(), local::local_port())?;
    if matches!(found, local::Found::ConfigFile(..) | local::Found::Local(_)) {
        eprintln!("terse: {}", found.describe());
    }
    Ok(found.urls())
}

/// A node on `--store` and `relays`, with feed relays unless
/// `--no-feed-relays`.
/// After a put or publish: which relays took it, and a warning for each
/// that did not (it succeeds when one does, NETWORK 5.1; README: puts and
/// publishes go to each relay).
pub(crate) fn say_delivery(node: &mut Node) {
    terse_net::account::delivery(node, &mut feed::say);
}

pub(crate) fn open_node(args: &Args, relays: &[String]) -> Result<Node, String> {
    let mut node = Node::open(args.get("--store").map(PathBuf::from), relays, DEFAULT_TIMEOUT)?;
    node.set_feed_relays(!args.has("--no-feed-relays"));
    // A command exits after its put or publish: every relay gets it first.
    node.set_deliver_to_all(true);
    Ok(node)
}

/// The blob store and relay from `--store` and `--relay`, opened on first
/// use; `find`: look for a relay when `--relay` is not given.
struct Net<'a> {
    args: &'a Args,
    node: Option<Node>,
    find: bool,
}

impl Net<'_> {
    fn node(&mut self) -> Result<&mut Node, String> {
        if self.node.is_none() {
            let relays = relay_urls(self.args, self.find)?;
            self.node = Some(open_node(self.args, &relays)?);
        }
        Ok(self.node.as_mut().unwrap())
    }

    fn take(mut self) -> Result<Node, String> {
        self.node()?;
        Ok(self.node.take().unwrap())
    }

    /// Fetches a whole, verified blob (NETWORK 2).
    fn fetch(&mut self, id: [u8; 32]) -> Result<Vec<u8>, String> {
        self.node()?.fetch_bytes(id)
    }
}

/// Loads an image from a file, or by image ID (NETWORK 2, 7).
fn load_image(arg: &str, net: &mut Net) -> Result<Image, String> {
    let bytes = match terse_net::link::blob_arg(arg) {
        Some(id) if !Path::new(arg).exists() => net.fetch(id)?,
        _ => read(arg)?,
    };
    Image::parse(&bytes).map_err(|e| format!("{arg}: {e}"))
}

/// Reads an `--events` script (README, "Scripted input").
pub fn load_script(path: Option<&str>) -> Result<Script, String> {
    let Some(path) = path else { return Ok(Script::default()) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let script = Script::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    // STATE.md 6.5: the lines for a window (pictures, dumps, its size) are
    // terse-host --drive's; a headless run passes over them.
    if let Some((frame, line)) = script.host_commands().first() {
        let n = script.host_commands().len();
        eprintln!("terse: {path}: {n} line{} for terse-host --drive skipped (the first, frame {frame}: {line})", if n == 1 { "" } else { "s" });
    }
    Ok(script)
}

/// The devices the machine's reading profile provides (SPEC §profiles): the system (1), the terminal (2), content (4), the page (7)
/// and full text (10).
const READING_DEVICES: [u8; 5] = [1, 2, 4, 7, 10];
/// The devices the machine's program profile provides (SPEC §profiles): the system (1), the terminal (2) and the 2D vector display (3).
const PROGRAM_DEVICES: [u8; 3] = [1, 2, 3];

/// `--with <slot>` and `--without <slot>`: a device slot, 2 to 255.
fn parse_slot(s: &str) -> Result<u8, String> {
    s.parse().ok().filter(|&n: &u8| n >= 2).ok_or(format!("bad device slot {s} (2 to 255)"))
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--dump", "--dump-vectors", "--dump-page", "--hash", "--hash-all", "--bench", "--shell", "--work"])?;
    if args.has("--replay") {
        return replay(&args);
    }
    args.check_known(&[
        "--frames",
        "--dump",
        "--dump-vectors",
        "--dump-page",
        "--events",
        "--relay",
        "--relay-authority",
        "--store",
        "--config",
        "--consent",
        "--no-feed-relays",
        "--hash",
        "--hash-all",
        "--hash-every",
        "--bench",
        "--shell",
        "--builtin",
        "--data",
        "--no-program-download",
        "--save-state",
        "--load-state",
        "--record",
        "--profile",
        "--with",
        "--without",
        "--audio-out",
        "--work",
    ])?;
    if (args.has("--dump-vectors") || args.has("--dump-page")) && !args.has("--dump") {
        return Err("--dump-vectors and --dump-page need --dump".into());
    }
    let shell = args.has("--shell");
    if !shell && (args.has("--builtin") || args.has("--hash-all")) {
        return Err("--builtin and --hash-all need --shell".into());
    }
    if shell && (args.has("--save-state") || args.has("--load-state") || args.has("--record")) {
        return Err("--save-state, --load-state and --record are for a single machine, not --shell".into());
    }
    // SPEC §profiles: the devices this run provides.
    let profile: Option<(&str, &[u8])> = match args.get("--profile") {
        None => None,
        Some("reading") => Some(("reading", &READING_DEVICES)),
        Some("program") => Some(("program", &PROGRAM_DEVICES)),
        Some(p) => return Err(format!("unknown profile {p} (reading or program)")),
    };
    let reading = profile.is_some_and(|p| p.0 == "reading");
    if let Some((name, _)) = profile
        && shell
    {
        return Err(format!("the {name} profile has no device 8: it runs a single machine, not --shell"));
    }
    let without = args.all("--without").map(parse_slot).collect::<Result<Vec<u8>, _>>()?;
    let with = args.all("--with").map(parse_slot).collect::<Result<Vec<u8>, _>>()?;
    if shell && !without.is_empty() {
        return Err("--without is for a single machine, not --shell".into());
    }
    if profile.is_none() && !with.is_empty() {
        return Err("--with adds a device to a --profile".into());
    }
    let provides =
        |slot: u8| (profile.is_none_or(|(_, p)| p.contains(&slot)) || with.contains(&slot)) && !without.contains(&slot);
    let script = load_script(args.get("--events"))?;
    let mut net = Net { args: &args, node: None, find: false };
    // STATE.md 4: a machine started from a snapshot.
    let snapshot_file = args.get("--load-state").map(read).transpose()?;
    let snapshot = match &snapshot_file {
        Some(file) => {
            let path = args.get("--load-state").unwrap_or_default();
            Some(terse_core::Snapshot::decode(file).map_err(|e| format!("{path}: {e}"))?)
        }
        None => None,
    };
    let image = match (args.positional.as_slice(), &snapshot) {
        ([], _) if shell => Image::parse(builtin::SHELL).map_err(|e| format!("built-in shell: {e}"))?,
        ([], Some(s)) => load_image(&to_hex(&s.image_id()), &mut net)?,
        _ => load_image(args.one_positional()?, &mut net)?,
    };
    // SPEC §image: a host refuses an image that requires a device it lacks; an
    // optional device it lacks is not granted.
    if let Some(d) = image.devices.iter().find(|d| !d.optional && !provides(d.slot)) {
        return Err(match profile {
            Some((name, p)) if !p.contains(&d.slot) && !with.contains(&d.slot) => format!(
                "the image requires device {}, which the {name} profile does not provide (it has devices {})",
                d.slot,
                p.iter().map(u8::to_string).collect::<Vec<_>>().join(", ")
            ),
            _ => format!("the image requires device {}, which this run does not provide (--without {})", d.slot, d.slot),
        });
    }
    let grants = match &snapshot {
        // STATE.md 4: a restored machine has the snapshot's devices.
        Some(s) => s.grants(&image).map_err(|e| format!("{}: {e}", args.get("--load-state").unwrap_or_default()))?,
        None => image.grants(|d| provides(d.slot)),
    };
    let frames = frame_count(&args)?;
    let hash_every = hash_every(&args)?;

    let consent = match args.get("--consent") {
        None => Consent::Deny,
        Some(c) => Consent::parse(c).filter(|&c| c != Consent::Ask).ok_or(format!("bad --consent {c} (allow or deny)"))?,
    };

    // A shell needs the store for the images it starts (SPEC §machines.calls).
    let mut builtins = Vec::new();
    let content = if image.device_set(&grants).has(4) || shell {
        // SPEC §content.feeds: the feeds are those of the --config subscriptions, and
        // the identity is the one kept there.
        let mut node = net.take()?;
        if shell {
            let extra = args.all("--builtin").map(|a| image_bytes(a, &mut node)).collect::<Result<Vec<_>, _>>()?;
            builtins = builtin::install(node.store(), &extra)?;
        } else {
            // The manual, which every host carries.
            builtin::install_manual(node.store())?;
        }
        node.set_subscriptions(feed::load_subscriptions(&args)?);
        node.set_identity_path(feed::identity_path(&args)?)?;
        node.set_nicknames(feed::load_nicknames(&args)?);
        node.set_records(terse_net::Records::in_data_dir(&data_dir(&args)?));
        if shell {
            node.set_snapshots(terse_net::Snapshots::in_data_dir(&data_dir(&args)?));
        }
        node.set_program_download(!args.has("--no-program-download"));
        let mut content = ContentService::start_with_consent(node, consent);
        content.set_reading_profile(reading);
        // Start from a caught-up store, so that a run sees what the relay
        // holds when it starts; later entries arrive while it runs.
        if content.following()
            && let Err(e) = content.wait_caught_up(feed::SYNC_DEADLINE)
        {
            eprintln!("terse: feeds: {e}; running with the entries held locally");
        }
        Some(content)
    } else {
        None
    };
    let mut system = match &snapshot {
        Some(s) => {
            let path = args.get("--load-state").unwrap_or_default();
            System::with_machine(s.restore_where(&image, |d| provides(d.slot)).map_err(|e| format!("{path}: {e}"))?)
        }
        None if shell => System::new(&image, shell, builtins),
        None => System::with_machine(Machine::with_grants(&image, grants.clone())),
    };
    // The built-in shell has the identity of a built-in (SPEC §machines.calls).
    if shell && args.positional.is_empty() {
        system.set_shell_identity(Some(terse_core::ProgramId { publisher: [0; 32], name: image.title.clone() }));
    }
    // SPEC §machines.snapshots: the shell's snapshots, as this host has them.
    if system.is_system() {
        terse_net::snapshots::load_table(&mut system, &data_dir(&args)?);
    }
    // Headless runs are reproducible (see `HeadlessSystem`).
    let mut headless = HeadlessSystem::new(content, script);
    // STATE.md 5: every input of every frame, and the hash after it.
    let mut recording = match args.get("--record") {
        Some(path) => {
            headless.record();
            let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
            let mut out = std::io::BufWriter::new(file);
            out.write_all(&terse_core::record::encode_header(&image.id, snapshot_file.as_deref(), &grants))
                .map_err(|e| format!("{path}: {e}"))?;
            Some((path, out))
        }
        None => None,
    };
    let mut stopped = None;
    let mut times = Vec::new();
    let mut audio = AudioOut::new(&args);
    // --work: the most each machine's displays asked of the host in a frame.
    let mut work: std::collections::BTreeMap<String, terse_core::worst::Work> = Default::default();
    let (mut system_ink, mut system_cut, mut frame_no) = ((0u64, 0u32, 0usize), 0u32, 0u32);
    for _ in 0..frames.unwrap_or(DEFAULT_FRAME_CAP) {
        let start = std::time::Instant::now();
        headless.frame(&mut system);
        audio.frame(&mut system);
        if args.has("--work") {
            let screens = system.scene((0.0, 0.0)).map_or(0, |s| s.view.screens);
            let mut note = |name: String, m: &Machine, screens: u8| {
                let w = terse_core::worst::Work::of(m, screens);
                let e = work.entry(name).or_default();
                *e = e.max(w);
            };
            note("0 shell".into(), system.shell(), screens);
            for (h, c) in system.children() {
                if let Some(m) = &c.machine {
                    note(format!("{h} {}", c.title), m, 0);
                }
            }
            // SPEC §machines.ink: the system ink the machines used.
            if system.is_system() {
                let s = system.shares();
                frame_no += 1;
                if s.used > system_ink.0 {
                    system_ink = (s.used, frame_no, s.shares.len());
                }
                system_cut += s.cut() as u32;
            }
        }
        stopped = system.stopped();
        if args.has("--bench") {
            times.push(start.elapsed());
        }
        if let Some((path, out)) = &mut recording {
            let hash = system.shell().state_hash();
            for (_, frame) in headless.take_recorded() {
                out.write_all(&frame.encode(&hash)).map_err(|e| format!("{path}: {e}"))?;
            }
        }
        print_hash_every(system.shell(), hash_every);
        if stopped.is_some() {
            break;
        }
    }
    if let Some((path, mut out)) = recording {
        out.flush().map_err(|e| format!("{path}: {e}"))?;
    }
    if stopped.is_none() && frames.is_none() {
        eprintln!("terse: stopped after {DEFAULT_FRAME_CAP} frames without an exit");
    }
    audio.finish()?;
    if args.has("--work") && system.is_system() {
        let (used, at, n) = system_ink;
        eprintln!(
            "terse: work: system ink: at most {used} of {} (frame {at}, {n} machines shown); frames with a picture or list cut: {system_cut}",
            terse_core::system_ink::SYSTEM_INK
        );
    }
    for (name, w) in &work {
        eprintln!("terse: work: {name}: {w}");
    }
    finish(&args, &system, stopped, &mut times)
}

/// `--frames`.
fn frame_count(args: &Args) -> Result<Option<u64>, String> {
    args.get("--frames").map(|n| n.parse().map_err(|_| format!("bad frame count {n}"))).transpose()
}

/// `--hash-every`.
fn hash_every(args: &Args) -> Result<Option<u32>, String> {
    args.get("--hash-every").map(|n| n.parse().ok().filter(|&n| n > 0).ok_or(format!("bad --hash-every {n}"))).transpose()
}

/// `--hash-every N`: `<counter> <hash>` after a frame that leaves the frame
/// counter at a multiple of N.
fn print_hash_every(machine: &Machine, every: Option<u32>) {
    if let Some(n) = every
        && machine.frame() % n == 0
    {
        println!("{} {}", machine.frame(), to_hex(&machine.state_hash()));
    }
}

/// `terse run <image> --replay <file>`: the recorded run again, from the
/// recording alone (STATE.md 5): no network, store, data directory or
/// script. Every frame's state hash is checked against the recording.
fn replay(args: &Args) -> Result<ExitCode, String> {
    args.check_known(&["--replay", "--frames", "--dump", "--dump-vectors", "--dump-page", "--hash", "--hash-every", "--bench", "--save-state", "--audio-out"])
        .map_err(|e| format!("{e}\n(--replay takes only --frames, --dump, --hash, --hash-every, --bench, --save-state and --audio-out)"))?;
    if (args.has("--dump-vectors") || args.has("--dump-page")) && !args.has("--dump") {
        return Err("--dump-vectors and --dump-page need --dump".into());
    }
    let path = args.require("--replay")?;
    let bytes = read(path)?;
    let mut recording = terse_core::record::Recording::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    // The image, from its file: a recording needs no store.
    let image_path = args.one_positional()?;
    let image = Image::parse(&read(image_path)?).map_err(|e| format!("{image_path}: {e}"))?;
    if image.id != recording.image_id {
        return Err(format!("{path}: a recording of image {}, not {image_path} ({})", to_hex(&recording.image_id), image.id_hex()));
    }
    // STATE.md 5: the grants follow the snapshot.
    let grants = recording.grants(image.optional().count()).map_err(|e| format!("{path}: {e}"))?;
    let machine = match recording.snapshot {
        Some(file) => terse_core::Snapshot::decode(file)
            .and_then(|s| s.restore(&image))
            .map_err(|e| format!("{path}: its snapshot: {e}"))?,
        None => match image.check_grants(&grants) {
            Ok(()) => Machine::with_grants(&image, grants.clone()),
            Err(d) => return Err(format!("{path}: {}", terse_core::machines::needs_device(&image, d).message)),
        },
    };
    if machine.grants() != &grants {
        return Err(format!("{path}: the grants differ from its snapshot's"));
    }
    let mut system = System::with_machine(machine);
    let frames = frame_count(args)?;
    let hash_every = hash_every(args)?;
    let mut stopped = None;
    let mut times = Vec::new();
    let mut run = 0;
    let mut audio = AudioOut::new(args);
    while frames.is_none_or(|n| run < n) {
        let Some(next) = recording.next_frame() else { break };
        let (frame, hash) = next.map_err(|e| format!("{path}: {e}"))?;
        let start = std::time::Instant::now();
        let machine = system.shell_mut();
        if frame.frame != machine.frame() || machine.stopped().is_some() {
            return Err(format!("{path}: a record of frame {} where the machine is at frame {}", frame.frame, machine.frame()));
        }
        let controllers = frame.controllers();
        machine.run_frame(&frame.input(&controllers));
        // Requests go nowhere: their completions are in the recording.
        machine.take_content_requests();
        if args.has("--bench") {
            times.push(start.elapsed());
        }
        run += 1;
        stopped = machine.stopped();
        audio.frame(&mut system);
        let machine = system.shell_mut();
        if machine.state_hash() != hash {
            return Err(format!("{path}: the replay differs from the recording after frame {}", frame.frame));
        }
        print_hash_every(system.shell(), hash_every);
    }
    audio.finish()?;
    finish(args, &system, stopped, &mut times)
}

/// `--audio-out`: the system's samples, frame by frame, written as a WAV
/// file at the end (SPEC §sound.presentation).
struct AudioOut {
    path: Option<String>,
    samples: Vec<i16>,
}

impl AudioOut {
    fn new(args: &Args) -> Self {
        AudioOut { path: args.get("--audio-out").map(str::to_string), samples: Vec::new() }
    }

    /// The frame just run: every machine's samples mixed, or silence.
    fn frame(&mut self, system: &mut System) {
        if self.path.is_some() {
            let mut frame = [0i16; 2 * terse_core::sound::FRAME_SAMPLES];
            system.take_audio(&mut frame);
            self.samples.extend_from_slice(&frame);
        }
    }

    fn finish(self) -> Result<(), String> {
        match self.path {
            Some(path) => write(&path, &terse_core::sound::wav(&self.samples)),
            None => Ok(()),
        }
    }
}

/// What `run` prints after the last frame, and its exit status.
fn finish(args: &Args, system: &System, stopped: Option<Stopped>, times: &mut [std::time::Duration]) -> Result<ExitCode, String> {
    if let Some(fault) = system.shell().fault() {
        eprintln!("terse: fault: {fault}");
    }
    if args.has("--bench") {
        bench_report(times);
    }
    if let Some(path) = args.get("--save-state") {
        write(path, &terse_core::Snapshot::of(system.shell()).encode())?;
    }
    if args.has("--hash") || args.has("--hash-all") {
        println!("{}", to_hex(&system.shell().state_hash()));
    }
    // Every machine the shell has (started and not stopped), in handle
    // order; `-` while it has no machine: its image is being fetched, or
    // its start failed.
    if args.has("--hash-all") {
        for (h, child) in system.children() {
            match &child.machine {
                Some(m) => println!("{h} {}", to_hex(&m.state_hash())),
                None => println!("{h} -"),
            }
        }
    }
    if args.has("--dump") {
        dump(system.shell(), args);
        if let Some(dev) = system.shell().machines() {
            for (h, child) in system.children() {
                let slot = dev.slot(h);
                let r = slot.rect;
                if r.w == 0 || r.h == 0 {
                    continue;
                }
                let state = terse_core::machines::state::name(slot.state);
                println!("-- machine {h} {state} {} {} {} {} {} --", r.x, r.y, r.w, r.h, child.title);
                if let Some(m) = &child.machine {
                    dump(m, args);
                }
            }
        }
    }
    match stopped {
        Some(Stopped::Fault(_)) => Ok(fault_exit()),
        Some(Stopped::Exit(code)) => Ok(ExitCode::from(code)),
        // A restored state the host had stopped (PERFORMANCE.md R15).
        Some(Stopped::Halted(slot)) => {
            eprintln!("terse: stopped by the host: device {slot} did not keep up");
            Ok(ExitCode::from(4))
        }
        None => Ok(ExitCode::SUCCESS),
    }
}

/// An image's bytes from a file, or by ID from the node (NETWORK 2, 7).
fn image_bytes(arg: &str, node: &mut Node) -> Result<Vec<u8>, String> {
    let bytes = match terse_net::link::blob_arg(arg) {
        Some(id) if !Path::new(arg).exists() => node.fetch_bytes(id)?,
        _ => read(arg)?,
    };
    Image::parse(&bytes).map_err(|e| format!("{arg}: {e}"))?;
    Ok(bytes)
}

/// `--dump`: one machine's terminal, and its vectors and page if asked.
fn dump(machine: &Machine, args: &Args) {
    match machine.terminal() {
        Some(t) => print!("{}", t.text()),
        None => eprintln!("terse: image has no terminal device"),
    }
    // SPEC §vector: the list device 3 holds at the end of the last frame, one
    // shape per line, as the host draws it.
    if args.has("--dump-vectors") {
        if let Some(line) = machine.vector().and_then(|v| v.style_line()) {
            println!("{line}");
        }
        if machine.has_vector() {
            println!("-- vectors: {} list bytes --", machine.vector_list().len());
            if let Some(v) = machine.vector() {
                print!("{}", v.dump());
            }
        }
        // SPEC §vector3d: the 3D list's commands, and what the reference eye sees.
        if let Some(v) = machine.vector3() {
            print!("{}", v.dump());
        }
    }
    // SPEC §page.reference: the page as the reference layout shows it.
    if args.has("--dump-page") {
        match machine.page() {
            Some(page) => {
                let width = page.region().w;
                let layout =
                    page.document().map(|doc| terse_core::page::reference_layout(doc, width)).unwrap_or_default();
                print!("{}", layout.dump(page));
            }
            None => println!("-- page: image has no page device --"),
        }
    }
}

/// Ends the process after a fault in a way no exit code can be mistaken
/// for: by signal SIGTERM (a shell shows status 143, which a program could
/// also return; a parent that waits for the process sees the signal).
fn fault_exit() -> ExitCode {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn raise(sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        // SAFETY: raise only sends a signal to this process.
        unsafe {
            raise(SIGTERM);
        }
    }
    ExitCode::from(255)
}

/// `--bench`: emulation time per frame (the machine and, for device 4, the
/// headless content service), on stderr.
fn bench_report(times: &mut [std::time::Duration]) {
    if times.is_empty() {
        return;
    }
    let total: std::time::Duration = times.iter().sum();
    times.sort();
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    let real = times.len() as f64 / 60.0;
    eprintln!(
        "bench: {} frames, emulation per frame p50 {:.3} ms, p99 {:.3} ms, max {:.3} ms; {:.1}x real time",
        times.len(),
        ms(at(0.5)),
        ms(at(0.99)),
        ms(times[times.len() - 1]),
        real / total.as_secs_f64(),
    );
}

fn info(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&NET_OPTIONS)?;
    let path = args.one_positional()?;
    let image = load_image(path, &mut Net { args: &args, node: None, find: true })?;
    // SPEC §image: required devices, then the optional ones.
    let list = |optional: bool| -> String {
        let v: Vec<String> = image.devices.iter().filter(|d| d.optional == optional).map(|d| format!("{}:{}", d.slot, d.version)).collect();
        if v.is_empty() { "-".into() } else { v.join(" ") }
    };
    println!("file:      {path}");
    println!("title:     {}", image.title_lossy());
    println!("format:    {}", terse_core::image::FORMAT_VERSION);
    println!("profile:   {}", terse_core::image::PROFILE);
    println!("rom_len:   {} bytes", image.rom.len());
    println!("ram_pages: {} ({} KB)", image.ram_pages, image.ram_pages as usize * 8);
    println!("devices:   {}", list(false));
    println!("optional:  {}", list(true));
    // spec/experimental.md: their names, which only the table gives.
    if image.devices.iter().any(|d| d.experimental()) {
        let names = match image.experimental() {
            Some(v) => v.iter().map(|(s, n)| format!("{s}={n}")).collect::<Vec<_>>().join(" "),
            None => "(the table of names is not valid: none is granted)".into(),
        };
        println!("experimental: {names} (not covered by conformance)");
    }
    // SPEC §terminal.text: the characters the terminal is drawn with.
    let text = match image.devices.iter().find(|d| d.slot == terse_core::text::FULL_TEXT) {
        Some(d) if !d.optional => "full (device 10 required)",
        Some(_) => "full where the host has it (device 10 optional), else basic",
        None => "basic (no device 10)",
    };
    println!("text:      {text}");
    println!("id:        {}", image.id_hex());
    Ok(ExitCode::SUCCESS)
}

fn put(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&NET_OPTIONS)?;
    let data = read(args.one_positional()?)?;
    if !terse_net::blob::valid_size(data.len()) {
        return Err(format!("a blob is 1 to {} bytes; this is {}", terse_net::blob::MAX_BLOB, data.len()));
    }
    let mut net = Net { args: &args, node: None, find: true };
    let node = net.node()?;
    let done = node.publish(&data);
    say_delivery(node);
    match done {
        Done::Put(id) => {
            println!("{}", to_hex(&id));
            Ok(ExitCode::SUCCESS)
        }
        _ => Err("put failed (store error, or the relay refused or did not answer)".into()),
    }
}

fn get(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&["-o", "--relay", "--relay-authority", "--store", "--config", "--no-feed-relays"])?;
    let arg = args.one_positional()?;
    let id = terse_net::link::blob_arg(arg).ok_or(format!("{arg} is not a blob ID (64 hex digits, or terse: and them)"))?;
    let bytes = Net { args: &args, node: None, find: true }.fetch(id)?;
    match args.get("-o") {
        Some(out) => write(out, &bytes)?,
        None => std::io::stdout().write_all(&bytes).map_err(|e| e.to_string())?,
    }
    Ok(ExitCode::SUCCESS)
}

/// `terse vectors [--dir <dir>]`: writes the core test vectors (vectors/README).
fn vectors(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&["--dir"])?;
    if !args.positional.is_empty() {
        return Err(USAGE.to_string());
    }
    let dir = PathBuf::from(args.get("--dir").unwrap_or("vectors"));
    terse_net::vectors::write(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    println!("wrote {} files to {}", terse_net::vectors::generate().len(), dir.display());
    Ok(ExitCode::SUCCESS)
}

fn hash(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&[])?;
    println!("{}", to_hex(&blob_id(&read(args.one_positional()?)?)));
    Ok(ExitCode::SUCCESS)
}

/// `slot:version`, a version 0 to 127 (SPEC §image); for an experimental
/// slot (240-255, spec/experimental.md) `slot:version=name`, which only
/// `--optional` takes ([`experimental_names`] reads the names).
fn parse_device(s: &str, optional: bool) -> Result<DeviceReq, String> {
    let bad = || format!("bad device {s} (want slot:version, a version 0 to 127; slot:version=name for an experimental device)");
    let (s, name) = match s.split_once('=') {
        Some((s, n)) => (s, Some(n)),
        None => (s, None),
    };
    let (slot, version) = s.split_once(':').ok_or_else(bad)?;
    let version: u8 = version.parse().ok().filter(|&v: &u8| v < terse_core::image::OPTIONAL).ok_or_else(bad)?;
    let d = DeviceReq { slot: slot.parse().map_err(|_| bad())?, version, optional };
    match (d.experimental(), name) {
        (true, _) if !optional => Err(format!(
            "device {}: slots 240-255 are experimental devices, which a program may only list as optional (--optional {}:{}=<name>; spec/experimental.md)",
            d.slot, d.slot, d.version
        )),
        (true, None) => Err(format!("--optional {s}: an experimental device needs its name: --optional {s}=<name> (spec/experimental.md)")),
        (true, Some(n)) if !terse_core::image::valid_experimental_name(n) => {
            Err(format!("--optional {s}={n}: a name is 1 to 30 of a-z, 0-9, - and ., starting with a letter"))
        }
        (false, Some(_)) => Err(format!("--optional {s}=...: only an experimental device (slots 240-255) has a name")),
        _ => Ok(d),
    }
}

/// The names of the experimental devices given with `--optional
/// <slot>:<version>=<name>` (spec/experimental.md), in the order given.
pub(crate) fn experimental_names(args: &Args) -> Vec<(u8, String)> {
    args.all("--optional")
        .filter_map(|o| {
            let (d, n) = o.split_once('=')?;
            Some((d.split_once(':')?.0.parse().ok()?, n.to_string()))
        })
        .collect()
}

/// `rom` with the experimental device table at its end, if `devices`
/// lists experimental devices (spec/experimental.md §experimental.image):
/// refuses two with one name, and an image that a host would not read the
/// table of.
pub(crate) fn with_experimental_table(args: &Args, devices: &[DeviceReq], mut rom: Vec<u8>) -> Result<Vec<u8>, String> {
    let names = experimental_names(args);
    if names.is_empty() {
        return Ok(rom);
    }
    for (i, (_, n)) in names.iter().enumerate() {
        if names[..i].iter().any(|(_, m)| m == n) {
            return Err(format!("the experimental device {n} is listed twice"));
        }
    }
    rom.extend(terse_core::image::experimental_table(&names));
    let image = Image::parse(&Image::build("", 1, devices, &rom)).map_err(|e| e.to_string())?;
    if image.experimental().is_none_or(|v| v.len() != names.len()) {
        return Err("the experimental device table does not match the devices listed".into());
    }
    Ok(rom)
}

fn pack(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&["--rom", "--title", "--ram-pages", "--device", "--optional", "-o"])?;
    if !args.positional.is_empty() {
        return Err(USAGE.to_string());
    }
    let rom = read(args.require("--rom")?)?;
    let title = args.require("--title")?;
    if title.len() > terse_core::image::TITLE_LEN {
        return Err(format!("title is {} bytes, at most 16 allowed", title.len()));
    }
    let ram_pages = args.require("--ram-pages")?;
    let ram_pages: u8 = ram_pages.parse().map_err(|_| format!("bad RAM page count {ram_pages}"))?;
    // Required devices first, then the optional ones, each in the order given.
    let devices = args
        .all("--device")
        .map(|d| parse_device(d, false))
        .chain(args.all("--optional").map(|d| parse_device(d, true)))
        .collect::<Result<Vec<_>, _>>()?;
    if devices.len() > terse_core::image::MAX_DEVICES {
        return Err("more than 16 devices".into());
    }
    let rom = with_experimental_table(&args, &devices, rom)?;
    let file = Image::build(title, ram_pages, &devices, &rom);
    // Validate exactly as a host would.
    let image = Image::parse(&file).map_err(|e| e.to_string())?;
    let out = args.require("-o")?;
    write(out, &file)?;
    println!("{out}: {} bytes, id {}", file.len(), image.id_hex());
    Ok(ExitCode::SUCCESS)
}
