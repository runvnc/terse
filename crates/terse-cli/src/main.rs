//! `terse`: headless runner, image tools and feeds.

mod feed;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use terse_core::{DeviceReq, Image, Machine, Script, Stopped};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::service::Headless;
use terse_net::{Consent, ContentService, Done, Node, blob_id, local, parse_hex, to_hex};

/// Frames `run` executes when neither `--frames` nor an exit stops it (10 minutes).
const DEFAULT_FRAME_CAP: u64 = 60 * 60 * 10;

const USAGE: &str = "usage:
  terse run <image|id> [--frames N] [--dump [--dump-vectors] [--dump-page]] [--events <file>] [--relay ws://...]
            [--store <dir>] [--config <dir>] [--consent allow|deny] [--hash] [--hash-every N] [--bench]
  terse info <image|id> [--relay ws://...|none] [--store <dir>] [--config <dir>]
  terse put <file> [--relay ws://...|none] [--store <dir>] [--config <dir>]
  terse get <id> [-o <file>] [--relay ws://...|none] [--store <dir>] [--config <dir>]
  terse hash <file>
  terse pack --rom <raw.bin> --title <t> --ram-pages <n> [--device <slot:version>]... -o <out.terse>
  terse key new|show
  terse feed publish <file> --title <t> [--reply-to <id>]
  terse feed subscribe|unsubscribe <key>
  terse feed subscriptions
  terse feed sync [--watch] [--auto-download <bytes>]
  terse feed list [<key>]
  terse feed read <key> <seq>
  terse feed nickname <key> [<name>]
  terse profile set --name <n> [--listed|--unlisted] [--description <file>]
  terse profile show [<key>]
  terse find [--entries] <text>
  terse relay add|remove|set <ws://host:port>
  terse relay list|show
  (key, feed, profile, find and relay commands all take [--relay ws://...|none]... [--store <dir>]
  [--config <dir>] [--no-feed-relays])

Relays: info, put, get, feed publish, feed sync, feed read, profile set and find use the
--relay options if given (several may be; `none`: no network), else the relays listed in
<config>/relays (`terse relay add` and `set` write it; an older <config>/relay file is read
if there is no list), else a relay running on this machine (ws://127.0.0.1:7878), else none
(sync then fails). Puts and publishes go to every relay; gets ask them in turn. Feeds
whose profile names relays are also followed there, unless --no-feed-relays. `run` uses
relays only with --relay.

<id> is a 64-hex-digit blob ID, looked up in the store, then the relay.
<key> is a feed key (64 hex digits).
--store defaults to $XDG_CACHE_HOME/terse/blobs (or ~/.cache/terse/blobs); feed
entries are kept in <store>/feeds.
--config holds the identity, subscriptions, nicknames and relay list; it defaults to
$XDG_CONFIG_HOME/terse (or ~/.config/terse).
profile set publishes your profile: the display name (1-48 bytes), --listed to let search
find it (default: as in your current profile, else unlisted), the description from a file
(default: kept), and relay lines for your relays that are not loopback addresses (the
--relay options, else your relay list). Names are not unique: listings show the key too.
--auto-download: articles up to this size are fetched as entries arrive (default 16384).
--events: scripted input, one event per line (see README).
--hash prints the state hash (STATE.md) after the last frame; --hash-every N prints
`<frame> <hash>` after every Nth frame (the frame counter, so after frames N, 2N, ...);
--bench prints the emulation time per frame (p50, p99, max) and the multiple of real time.
--consent: how `run` answers a program's requests that need consent (subscribe, unsubscribe, publish,
set profile, set nickname; SPEC 9.5), since nobody can be asked (default deny).
--dump-page prints the page (device 7) as the reference layout lays it out: a header line with
its height and scroll, then the lines in view (y, height, x, the first span's font, the text).
`run` exits with the program's exit code (0-255). If the machine stops with a fault
(SPEC 1.5), it prints the fault and ends by signal SIGTERM, so a parent process can
tell a fault from any exit code (a shell's $status shows 143).";

/// Options for commands that use the blob store and relay.
const NET_OPTIONS: [&str; 4] = ["--relay", "--store", "--config", "--no-feed-relays"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("info") => info(&args[1..]),
        Some("put") => put(&args[1..]),
        Some("get") => get(&args[1..]),
        Some("hash") => hash(&args[1..]),
        Some("pack") => pack(&args[1..]),
        Some("key") => feed::key(&args[1..]),
        Some("feed") => feed::feed(&args[1..]),
        Some("relay") => feed::relay(&args[1..]),
        Some("profile") => feed::profile(&args[1..]),
        Some("find") => feed::find(&args[1..]),
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
                let value = if switches.contains(&a.as_str()) || a == "--no-feed-relays" {
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
    if !find {
        // Without a list to fall back on, no --relay means no relay.
        return if given.is_empty() { Ok(Vec::new()) } else { Ok(local::find_relays(&given, None, 0)?.urls()) };
    }
    let config = args.get("--config").map(PathBuf::from).or_else(terse_net::default_config_dir);
    let found = local::find_relays(&given, config.as_deref(), local::local_port())?;
    if matches!(found, local::Found::ConfigFile(..) | local::Found::Local(_)) {
        eprintln!("terse: {}", found.describe());
    }
    Ok(found.urls())
}

/// A node on `--store` and `relays`, with feed relays unless
/// `--no-feed-relays`.
pub(crate) fn open_node(args: &Args, relays: &[String]) -> Result<Node, String> {
    let mut node = Node::open(args.get("--store").map(PathBuf::from), relays, DEFAULT_TIMEOUT)?;
    node.set_feed_relays(!args.has("--no-feed-relays"));
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

/// Loads an image from a file, or by image ID (NETWORK 2).
fn load_image(arg: &str, net: &mut Net) -> Result<Image, String> {
    let bytes = match parse_hex(arg) {
        Some(id) if !Path::new(arg).exists() => net.fetch(id)?,
        _ => read(arg)?,
    };
    Image::parse(&bytes).map_err(|e| format!("{arg}: {e}"))
}

/// Reads an `--events` script (README, "Scripted input").
pub fn load_script(path: Option<&str>) -> Result<Script, String> {
    let Some(path) = path else { return Ok(Script::default()) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Script::parse(&text).map_err(|e| format!("{path}: {e}"))
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--dump", "--dump-vectors", "--dump-page", "--hash", "--bench"])?;
    args.check_known(&[
        "--frames",
        "--dump",
        "--dump-vectors",
        "--dump-page",
        "--events",
        "--relay",
        "--store",
        "--config",
        "--consent",
        "--no-feed-relays",
        "--hash",
        "--hash-every",
        "--bench",
    ])?;
    if (args.has("--dump-vectors") || args.has("--dump-page")) && !args.has("--dump") {
        return Err("--dump-vectors and --dump-page need --dump".into());
    }
    let script = load_script(args.get("--events"))?;
    let mut net = Net { args: &args, node: None, find: false };
    let image = load_image(args.one_positional()?, &mut net)?;
    let frames: Option<u64> = match args.get("--frames") {
        Some(n) => Some(n.parse().map_err(|_| format!("bad frame count {n}"))?),
        None => None,
    };
    let hash_every: Option<u32> = match args.get("--hash-every") {
        Some(n) => Some(n.parse().ok().filter(|&n| n > 0).ok_or(format!("bad --hash-every {n}"))?),
        None => None,
    };

    let consent = match args.get("--consent") {
        None => Consent::Deny,
        Some(c) => Consent::parse(c).filter(|&c| c != Consent::Ask).ok_or(format!("bad --consent {c} (allow or deny)"))?,
    };

    let mut machine = Machine::new(&image);
    let content = if image.has_device(4) {
        // SPEC 9.5: the feeds are those of the --config subscriptions, and
        // the identity is the one kept there.
        let mut node = net.take()?;
        node.set_subscriptions(feed::load_subscriptions(&args)?);
        node.set_identity_path(feed::identity_path(&args)?)?;
        node.set_nicknames(feed::load_nicknames(&args)?);
        let mut content = ContentService::start_with_consent(node, consent);
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
    // Headless runs are reproducible (see `Headless`).
    let mut headless = Headless::new(content, script);
    let mut stopped = None;
    let mut times = Vec::new();
    for _ in 0..frames.unwrap_or(DEFAULT_FRAME_CAP) {
        let start = std::time::Instant::now();
        stopped = headless.frame(&mut machine).stopped;
        if args.has("--bench") {
            times.push(start.elapsed());
        }
        if let Some(n) = hash_every
            && machine.frame() % n == 0
        {
            println!("{} {}", machine.frame(), to_hex(&machine.state_hash()));
        }
        if stopped.is_some() {
            break;
        }
    }
    if let Some(fault) = machine.fault() {
        eprintln!("terse: fault: {fault}");
    }
    if stopped.is_none() && frames.is_none() {
        eprintln!("terse: stopped after {DEFAULT_FRAME_CAP} frames without an exit");
    }

    if args.has("--bench") {
        bench_report(&mut times);
    }
    if args.has("--hash") {
        println!("{}", to_hex(&machine.state_hash()));
    }
    if args.has("--dump") {
        match machine.terminal() {
            Some(t) => print!("{}", t.text()),
            None => eprintln!("terse: image has no terminal device"),
        }
        // SPEC 10: the list device 3 holds at the end of the last frame, one
        // shape per line, as the host draws it.
        if args.has("--dump-vectors") {
            if let Some(line) = machine.vector().and_then(|v| v.style_line()) {
                println!("{line}");
            }
            println!("-- vectors: {} list bytes --", machine.vector_list().len());
            if let Some(v) = machine.vector() {
                print!("{}", v.dump());
            }
        }
        // SPEC 11.6: the page as the reference layout shows it.
        if args.has("--dump-page") {
            match machine.page() {
                Some(page) => {
                    let width = page.region().w;
                    let layout = page
                        .document()
                        .map(|doc| terse_core::page::reference_layout(doc, width))
                        .unwrap_or_default();
                    print!("{}", layout.dump(page));
                }
                None => println!("-- page: image has no page device --"),
            }
        }
    }
    match stopped {
        Some(Stopped::Fault(_)) => Ok(fault_exit()),
        Some(Stopped::Exit(code)) => Ok(ExitCode::from(code)),
        None => Ok(ExitCode::SUCCESS),
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
    let devices: Vec<String> =
        image.devices.iter().map(|d| format!("{}:{}", d.slot, d.version)).collect();
    println!("file:      {path}");
    println!("title:     {}", image.title_lossy());
    println!("format:    {}", terse_core::image::FORMAT_VERSION);
    println!("profile:   {}", terse_core::image::PROFILE);
    println!("rom_len:   {} bytes", image.rom.len());
    println!("ram_pages: {} ({} KB)", image.ram_pages, image.ram_pages as usize * 8);
    println!("devices:   {}", if devices.is_empty() { "-".into() } else { devices.join(" ") });
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
    match net.node()?.publish(&data) {
        Done::Put(id) => {
            println!("{}", to_hex(&id));
            Ok(ExitCode::SUCCESS)
        }
        _ => Err("put failed (store error, or the relay refused or did not answer)".into()),
    }
}

fn get(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&["-o", "--relay", "--store", "--config", "--no-feed-relays"])?;
    let arg = args.one_positional()?;
    let id = parse_hex(arg).ok_or(format!("{arg} is not a blob ID (64 hex digits)"))?;
    let bytes = Net { args: &args, node: None, find: true }.fetch(id)?;
    match args.get("-o") {
        Some(out) => write(out, &bytes)?,
        None => std::io::stdout().write_all(&bytes).map_err(|e| e.to_string())?,
    }
    Ok(ExitCode::SUCCESS)
}

fn hash(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&[])?;
    println!("{}", to_hex(&blob_id(&read(args.one_positional()?)?)));
    Ok(ExitCode::SUCCESS)
}

fn parse_device(s: &str) -> Result<DeviceReq, String> {
    let bad = || format!("bad device {s} (want slot:version)");
    let (slot, version) = s.split_once(':').ok_or_else(bad)?;
    Ok(DeviceReq {
        slot: slot.parse().map_err(|_| bad())?,
        version: version.parse().map_err(|_| bad())?,
    })
}

fn pack(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&["--rom", "--title", "--ram-pages", "--device", "-o"])?;
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
    let devices = args.all("--device").map(parse_device).collect::<Result<Vec<_>, _>>()?;
    if devices.len() > terse_core::image::MAX_DEVICES {
        return Err("more than 16 devices".into());
    }
    let file = Image::build(title, ram_pages, &devices, &rom);
    // Validate exactly as a host would.
    let image = Image::parse(&file).map_err(|e| e.to_string())?;
    let out = args.require("-o")?;
    write(out, &file)?;
    println!("{out}: {} bytes, id {}", file.len(), image.id_hex());
    Ok(ExitCode::SUCCESS)
}
