//! `terse`: headless runner, image tools and feeds.

mod asm;
mod feed;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use terse_core::{DeviceReq, FrameMachine, Script};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::service::Headless;
use terse_net::{ContentService, Done, Node, blob_id, parse_hex, to_hex};
use terse_rv::{AnyMachine, Program};

/// Frames `run` executes when neither `--frames` nor an exit stops it (10 minutes).
const DEFAULT_FRAME_CAP: u64 = 60 * 60 * 10;

const USAGE: &str = "usage:
  terse run <image|id> [--frames N] [--dump [--dump-vectors]] [--events <file>] [--relay ws://...]
            [--store <dir>] [--config <dir>] [--hash] [--hash-every N] [--bench]
  terse info <image|id> [--relay ws://...] [--store <dir>]
  terse put <file> [--relay ws://...] [--store <dir>]
  terse get <id> [-o <file>] [--relay ws://...] [--store <dir>]
  terse hash <file>
  terse pack --rom <raw.bin> --title <t> --ram-pages <n> [--device <slot:version>]... [--profile 1|2]
             -o <out.terse>
  terse asm <source.asm> -o <out.bin>
  terse key new|show
  terse feed publish <file> --title <t> [--reply-to <id>]
  terse feed subscribe|unsubscribe <key>
  terse feed subscriptions
  terse feed sync [--watch] [--auto-download <bytes>]
  terse feed list [<key>]
  terse feed read <key> <seq>
  (key and feed commands all take [--relay ws://...] [--store <dir>] [--config <dir>];
  sync needs --relay)

<id> is a 64-hex-digit blob ID, looked up in the store, then the relay.
<key> is a feed key (64 hex digits).
--store defaults to $XDG_CACHE_HOME/terse/blobs (or ~/.cache/terse/blobs); feed
entries are kept in <store>/feeds.
--config holds the identity and subscriptions; it defaults to $XDG_CONFIG_HOME/terse
(or ~/.config/terse).
--auto-download: articles up to this size are fetched as entries arrive (default 16384).
--events: scripted input, one event per line (see README).
--hash prints the state hash (STATE.md) after the last frame; --hash-every N prints
`<frame> <hash>` after every Nth frame (the frame counter, so after frames N, 2N, ...);
--bench prints the emulation time per frame (p50, p99, max) and the multiple of real time.";

/// Options for commands that use the blob store and relay.
const NET_OPTIONS: [&str; 2] = ["--relay", "--store"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("info") => info(&args[1..]),
        Some("put") => put(&args[1..]),
        Some("get") => get(&args[1..]),
        Some("hash") => hash(&args[1..]),
        Some("pack") => pack(&args[1..]),
        Some("asm") => assemble(&args[1..]),
        Some("key") => feed::key(&args[1..]),
        Some("feed") => feed::feed(&args[1..]),
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
                let value = if switches.contains(&a.as_str()) {
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

/// The blob store and relay from `--store` and `--relay`, opened on first use.
struct Net<'a> {
    args: &'a Args,
    node: Option<Node>,
}

impl Net<'_> {
    fn node(&mut self) -> Result<&mut Node, String> {
        if self.node.is_none() {
            let store = self.args.get("--store").map(PathBuf::from);
            self.node = Some(Node::open(store, self.args.get("--relay"), DEFAULT_TIMEOUT)?);
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

/// Loads an image of either profile from a file, or by image ID (NETWORK 2).
fn load_image(arg: &str, net: &mut Net) -> Result<Program, String> {
    let bytes = match parse_hex(arg) {
        Some(id) if !Path::new(arg).exists() => net.fetch(id)?,
        _ => read(arg)?,
    };
    terse_rv::parse_image(&bytes).map_err(|e| format!("{arg}: {e}"))
}

/// Reads an `--events` script (README, "Scripted input").
pub fn load_script(path: Option<&str>) -> Result<Script, String> {
    let Some(path) = path else { return Ok(Script::default()) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Script::parse(&text).map_err(|e| format!("{path}: {e}"))
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--dump", "--dump-vectors", "--hash", "--bench"])?;
    args.check_known(&[
        "--frames",
        "--dump",
        "--dump-vectors",
        "--events",
        "--relay",
        "--store",
        "--config",
        "--hash",
        "--hash-every",
        "--bench",
    ])?;
    if args.has("--dump-vectors") && !args.has("--dump") {
        return Err("--dump-vectors needs --dump".into());
    }
    let script = load_script(args.get("--events"))?;
    let mut net = Net { args: &args, node: None };
    let image = load_image(args.one_positional()?, &mut net)?;
    let frames: Option<u64> = match args.get("--frames") {
        Some(n) => Some(n.parse().map_err(|_| format!("bad frame count {n}"))?),
        None => None,
    };
    let hash_every: Option<u32> = match args.get("--hash-every") {
        Some(n) => Some(n.parse().ok().filter(|&n| n > 0).ok_or(format!("bad --hash-every {n}"))?),
        None => None,
    };

    let mut machine = AnyMachine::new(&image);
    let content = if image.has_device(4) {
        // SPEC 7.7: the feeds are those of the --config subscriptions.
        let mut node = net.take()?;
        node.set_subscriptions(feed::load_subscriptions(&args)?);
        let mut content = ContentService::start(node);
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
    let mut exit = None;
    let mut times = Vec::new();
    for _ in 0..frames.unwrap_or(DEFAULT_FRAME_CAP) {
        let start = std::time::Instant::now();
        exit = headless.frame(&mut machine).exit;
        if args.has("--bench") {
            times.push(start.elapsed());
        }
        if let Some(n) = hash_every
            && machine.frame() % n == 0
        {
            let hash = machine.state_hash().ok_or("--hash-every: no state hash for this profile")?;
            println!("{} {}", machine.frame(), to_hex(&hash));
        }
        if exit.is_some() {
            break;
        }
    }
    if let Some(fault) = machine.fault() {
        eprintln!("terse: fault: {fault}");
    }
    if exit.is_none() && frames.is_none() {
        eprintln!("terse: stopped after {DEFAULT_FRAME_CAP} frames without an exit");
    }

    if args.has("--bench") {
        bench_report(&mut times);
    }
    if args.has("--hash") {
        let hash = machine.state_hash().ok_or("--hash: no state hash for this profile")?;
        println!("{}", to_hex(&hash));
    }
    if args.has("--dump") {
        match machine.terminal() {
            Some(t) => print!("{}", t.text()),
            None => eprintln!("terse: image has no terminal device"),
        }
        // SPEC 8: the list device 3 holds at the end of the last frame, one
        // primitive per line.
        if args.has("--dump-vectors") {
            if let Some(line) = machine.vector().and_then(|v| v.style_line()) {
                println!("{line}");
            }
            let list = machine.vector_list();
            println!("-- vectors: {} list bytes --", list.len());
            print!("{}", terse_core::vector::dump(list));
        }
    }
    Ok(ExitCode::from(exit.unwrap_or(0)))
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
    let image = load_image(path, &mut Net { args: &args, node: None })?;
    let devices: Vec<String> =
        image.devices.iter().map(|d| format!("{}:{}", d.slot, d.version)).collect();
    println!("file:      {path}");
    println!("title:     {}", image.title_lossy());
    println!("format:    {}", terse_core::image::FORMAT_VERSION);
    println!("profile:   {}", image.profile);
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
    let mut net = Net { args: &args, node: None };
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
    args.check_known(&["-o", "--relay", "--store"])?;
    let arg = args.one_positional()?;
    let id = parse_hex(arg).ok_or(format!("{arg} is not a blob ID (64 hex digits)"))?;
    let bytes = Net { args: &args, node: None }.fetch(id)?;
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
    args.check_known(&["--rom", "--title", "--ram-pages", "--device", "--profile", "-o"])?;
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
    let profile = match args.get("--profile").unwrap_or("1") {
        "1" => terse_core::image::PROFILE,
        "2" => terse_rv::PROFILE_RV32,
        p => return Err(format!("unknown profile {p} (1 or 2)")),
    };
    let file = terse_rv::build_image(profile, title, ram_pages, &devices, &rom);
    // Validate exactly as a host would.
    let image = terse_rv::parse_image(&file).map_err(|e| e.to_string())?;
    let out = args.require("-o")?;
    write(out, &file)?;
    println!("{out}: {} bytes, id {}", file.len(), image.id_hex());
    Ok(ExitCode::SUCCESS)
}

fn assemble(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&["-o"])?;
    let src_path = args.one_positional()?;
    let src = std::fs::read_to_string(src_path).map_err(|e| format!("{src_path}: {e}"))?;
    let bin = asm::assemble(src_path, &src)?;
    let out = args.require("-o")?;
    write(out, &bin)?;
    println!("{out}: {} bytes", bin.len());
    Ok(ExitCode::SUCCESS)
}
