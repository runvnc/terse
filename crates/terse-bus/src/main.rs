//! `terse-bus`: runs a Terse machine, or a shell and the machines it
//! starts, with their devices as separate driver processes (DEVICES.md).

#[cfg(feature = "window")]
mod window;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use terse_bus::bus::BusConfig;
use terse_bus::compose::{Compositor, write_png};
use terse_bus::host::{self, BusSystem, Start};
use terse_bus::inproc::{ContentOptions, InProcessHost};
use terse_bus::record::{self, FrameRecord};
use terse_bus::signals;
use terse_bus::stats::spread;
use terse_bus::transport::{self, Spin};
use terse_core::devices::Memory;
use terse_core::{Event, FrameInput, Image, Machine, Script, Snapshot, Stopped, Terminal};
use terse_net::{BlobStore, builtin};

const USAGE: &str = "usage:
  terse-bus [<image> | --shell [<image>]] [--builtin <image>]... [--frames N] [--events <file>]
            [--hash] [--hash-all] [--hash-every N] [--dump] [--save-state <file>] [--load-state <file>]
            [--driver <slot>[,<slot>...]=<command line>]... [--transport shm|socket] [--no-batch] [--in-process]
            [--screenshot <out.png>] [--size <W>x<H>] [--eye <dx>,<dy>] [--panels perspective|flat]
            [--present-every-frame] [--window] [--selftest] [--parallax off|pointer] [--threads N]
            [--bench] [--realtime] [--frame-times <file>] [--pin] [--spin-us N] [--spin fixed|adaptive]
            [--driver-timeout S] [--start-timeout S] [--record <file>] [--replay <file>]
            [--store <dir>] [--config <dir>] [--data <dir>] [--relay ws://...] [--consent allow|deny|ask]
            [--profile reading] [--no-feed-relays] [--no-program-download] [--audio-out <file.wav>]
            [--without <slot>]... [--perf-log <dir>|off] [--perf-rule on|off]
  terse-bus --probe-driver <program> [--slot <n>]
  terse-bus --version

Runs the devices as driver processes: by default terse-dev-terminal (slot 2), terse-dev-vector
(slots 3 and 9, one process), terse-dev-content (4, given --store, --config, --data, --relay,
--consent, --profile), terse-dev-sound (6) and terse-dev-page (7), from this program's directory; --driver replaces the
driver of the slots it names (the command line is split at spaces). Between the two come the drivers the user installed
(terse drivers; <config>/drivers, DEVICES.md 2.2): one that cannot be used gives way to the shipped driver, with a
message. --probe-driver makes the handshake terse drivers add makes and prints what the program answered. --shell runs the image (default:
the shell built into terse) as the shell (SPEC §machines): one driver process per device serves every
machine. --transport socket offers drivers only the socket (default: shared memory is offered and
the Rust drivers take it). --no-batch sends every call at once. --in-process runs a single machine
with the devices compiled in, for comparison.
Inputs are those of `terse run`: the --events script, time 0, no random bytes, content completed at
the next frame. --hash, --hash-all, --hash-every, --dump, --save-state and --load-state as in `terse
run` (snapshot files are the same). --screenshot composes the picture after the last frame (--size,
default the terminal's native size; --eye, the eye for 3D lists); --present-every-frame composes
every frame (as a window does) and --bench then times it too. --window shows the picture in a window
(F12 is the home key, Ctrl+V or Shift+Insert pastes; with --parallax pointer the eye follows the
pointer); --selftest runs --frames frames (default 300) in the window with the scripted input only and
prints the emulation, bus, composition and present-interval timings. --realtime runs at 60 frames a
second. --frame-times writes each frame's emulation time (ms). --bench prints the frame times
(emulation including the bus) and per-driver latencies. --pin puts the core on CPU 1 and the drivers
on CPUs 2, 3, ... --spin-us (default 100) is the core's spin for a reply; drivers spin adaptively
(TERSE_BUS_SPIN=fixed and TERSE_BUS_SPIN_US for theirs). --driver-timeout (default 30 s): a driver
that does not answer in time is given up as hung; --start-timeout (default 10 s): for it to start.
A driver that fails ends the run (exit status 3); a fault ends it by signal SIGTERM, as terse run does
(STATE.md 6.4). --record writes the bus's recording of a single
machine; --replay runs one again, checking every frame's hash. --audio-out writes what the sound
devices played (SPEC §sound.presentation: every machine's frame, mixed) as a WAV file, as `terse run` does; nothing
is ever played. --without withholds an optional device (SPEC §image: the machine runs without it); a
device only optional devices use whose driver is not installed is left out the same way, while a
required device without a driver stops the run.
--perf-log: where the performance record goes (RECORD.md; default: none headless, and
$XDG_DATA_HOME/terse/perf with --window or --realtime; it never leaves this computer). --perf-rule
(default: on with --window or --realtime, off headless): the rule of PERFORMANCE.md R15: a driver
that makes 3 frames late within 60 is warned about (its machines are saved at once), one that makes
10 late within 300 has its machines saved and stopped (a machine alone: exit status 4, its snapshot
in the record's directory). --version prints this program's name, version and build.";

struct Opts {
    image: Option<String>,
    shell: bool,
    builtins: Vec<String>,
    frames: Option<u64>,
    events: Option<String>,
    hash: bool,
    hash_all: bool,
    hash_every: Option<u32>,
    dump: bool,
    save_state: Option<PathBuf>,
    load_state: Option<PathBuf>,
    drivers: Vec<(Vec<u8>, String)>,
    socket_only: bool,
    batching: bool,
    in_process: bool,
    screenshot: Option<PathBuf>,
    size: Option<(u32, u32)>,
    eye: (f32, f32),
    panels_flat: bool,
    present_every: bool,
    window: bool,
    selftest: bool,
    parallax: bool,
    threads: usize,
    bench: bool,
    realtime: bool,
    frame_times: Option<PathBuf>,
    pin: bool,
    spin_us: u64,
    spin_adaptive: bool,
    driver_timeout: f64,
    start_timeout: f64,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
    audio_out: Option<PathBuf>,
    content: ContentOptions,
    /// Optional devices withheld (SPEC §image).
    without: Vec<u8>,
    /// The record's directory: `Some(None)` off.
    perf_log: Option<Option<PathBuf>>,
    perf_rule: Option<bool>,
}

fn parse(args: Vec<String>) -> Result<Opts, String> {
    let mut o = Opts {
        image: None,
        shell: false,
        builtins: Vec::new(),
        frames: None,
        events: None,
        hash: false,
        hash_all: false,
        hash_every: None,
        dump: false,
        save_state: None,
        load_state: None,
        drivers: Vec::new(),
        socket_only: false,
        batching: true,
        in_process: false,
        screenshot: None,
        size: None,
        eye: (0.0, 0.0),
        panels_flat: false,
        present_every: false,
        window: false,
        selftest: false,
        parallax: true,
        threads: 0,
        bench: false,
        realtime: false,
        frame_times: None,
        pin: false,
        spin_us: 100,
        spin_adaptive: false,
        driver_timeout: 30.0,
        start_timeout: 10.0,
        record: None,
        replay: None,
        audio_out: None,
        content: ContentOptions::default(),
        without: Vec::new(),
        perf_log: None,
        perf_rule: None,
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        if o.content.take(&a, &mut value)? {
            continue;
        }
        match a.as_str() {
            "--shell" => o.shell = true,
            "--builtin" => o.builtins.push(value()?),
            "--frames" => o.frames = Some(value()?.parse().map_err(|_| "bad --frames")?),
            "--events" => o.events = Some(value()?),
            "--hash" => o.hash = true,
            "--hash-all" => o.hash_all = true,
            "--hash-every" => o.hash_every = Some(value()?.parse().ok().filter(|&n| n > 0).ok_or("bad --hash-every")?),
            "--dump" => o.dump = true,
            "--save-state" => o.save_state = Some(value()?.into()),
            "--load-state" => o.load_state = Some(value()?.into()),
            "--driver" => {
                let v = value()?;
                let (slots, cmd) = v.split_once('=').ok_or("--driver <slot>[,<slot>...]=<command line>")?;
                let slots = slots.split(',').map(|s| s.parse().map_err(|_| format!("bad slot {s}"))).collect::<Result<Vec<u8>, String>>()?;
                o.drivers.push((slots, cmd.to_string()));
            }
            "--transport" => {
                o.socket_only = match value()?.as_str() {
                    "socket" => true,
                    "shm" => false,
                    t => return Err(format!("bad --transport {t} (shm or socket)")),
                }
            }
            "--no-batch" => o.batching = false,
            "--in-process" => o.in_process = true,
            "--screenshot" => o.screenshot = Some(value()?.into()),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or("--size <W>x<H>")?;
                o.size = Some((w.parse().map_err(|_| "bad --size")?, h.parse().map_err(|_| "bad --size")?));
            }
            "--eye" => {
                let v = value()?;
                let m = terse_core::vector3::MAX_EYE_OFFSET;
                let parsed = v.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)));
                o.eye = parsed.filter(|&(x, y)| x.abs() <= m && y.abs() <= m).ok_or(format!("bad --eye {v}"))?;
            }
            "--panels" => {
                o.panels_flat = match value()?.as_str() {
                    "flat" => true,
                    "perspective" | "auto" => false,
                    v => return Err(format!("bad --panels {v}")),
                }
            }
            "--present-every-frame" => o.present_every = true,
            "--window" => o.window = true,
            "--selftest" => {
                o.selftest = true;
                o.window = true;
            }
            "--parallax" => {
                o.parallax = match value()?.as_str() {
                    "pointer" => true,
                    "off" => false,
                    v => return Err(format!("bad --parallax {v}")),
                }
            }
            "--threads" => o.threads = value()?.parse().map_err(|_| "bad --threads")?,
            "--bench" => o.bench = true,
            "--realtime" => o.realtime = true,
            "--audio-out" => o.audio_out = Some(value()?.into()),
            "--frame-times" => {
                o.frame_times = Some(value()?.into());
                o.bench = true;
            }
            "--pin" => o.pin = true,
            "--spin-us" => o.spin_us = value()?.parse().map_err(|_| "bad --spin-us")?,
            "--spin" => {
                o.spin_adaptive = match value()?.as_str() {
                    "adaptive" => true,
                    "fixed" => false,
                    v => return Err(format!("bad --spin {v} (fixed or adaptive)")),
                }
            }
            "--driver-timeout" => o.driver_timeout = value()?.parse().map_err(|_| "bad --driver-timeout")?,
            "--start-timeout" => o.start_timeout = value()?.parse().map_err(|_| "bad --start-timeout")?,
            "--without" => o.without.push(value()?.parse().ok().filter(|&s: &u8| s >= 2).ok_or("bad --without (a device slot, 2 to 255)")?),
            "--record" => o.record = Some(value()?.into()),
            "--perf-log" => {
                let v = value()?;
                o.perf_log = Some(if v == "off" { None } else { Some(v.into()) });
            }
            "--perf-rule" => {
                o.perf_rule = Some(match value()?.as_str() {
                    "on" => true,
                    "off" => false,
                    v => return Err(format!("bad --perf-rule {v} (on or off)")),
                })
            }
            "--replay" => o.replay = Some(value()?.into()),
            "--help" | "-h" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            _ if o.image.is_none() => o.image = Some(a),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(o)
}

/// Set when the machine faulted: the process then ends by SIGTERM, as
/// `terse run` does, once every driver is reaped (STATE.md 6.4).
static FAULTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// SPEC §stopping: a fault is never shown as an exit code. Prints it and
/// notes it for `main`.
fn faulted(stopped: Stopped) -> ExitCode {
    eprintln!("terse-bus: {stopped}");
    FAULTED.store(true, std::sync::atomic::Ordering::SeqCst);
    ExitCode::from(255)
}

/// Ends the process by SIGTERM (restoring its default action first: the
/// run's handler only notes a signal).
fn fault_exit() -> ExitCode {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: resets one signal's action and sends it to this process.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        libc::raise(libc::SIGTERM);
    }
    ExitCode::from(255)
}

fn main() -> ExitCode {
    if std::env::args().nth(1).is_some_and(|a| a == "--probe-driver") {
        return probe(std::env::args().skip(2).collect());
    }
    if std::env::args().skip(1).any(|a| a == "--version") {
        println!("{}", terse_perflog::Identity::this("terse-bus").line());
        return ExitCode::SUCCESS;
    }
    signals::install();
    match run(std::env::args().skip(1).collect()) {
        Ok(_) if FAULTED.load(std::sync::atomic::Ordering::SeqCst) => fault_exit(),
        Ok(code) => code,
        Err(e) => {
            eprintln!("terse-bus: {e}");
            ExitCode::from(if e.starts_with("driver ") { 3 } else { 2 })
        }
    }
}

/// `--probe-driver <program> [--slot n]`: the handshake `terse drivers add`
/// makes (DEVICES.md 2.2). Starts the program as this core would, with no
/// options and on the socket, once for each group of slots (or the group of
/// `--slot`), and prints, for each group it answers for, its identity and
/// slots: lines `group`, `name`, `version`, `build` and `slot <s> <v>`, then
/// a blank line. The groups it refuses go to standard error. Status 0 if it
/// answered for one or more groups, else 2.
fn probe(args: Vec<String>) -> ExitCode {
    let (program, slot) = match args.as_slice() {
        [p] => (p.clone(), None),
        [p, f, s] if f == "--slot" => match s.parse::<u8>().ok().and_then(terse_net::driverlist::group_of) {
            Some(g) => (p.clone(), Some(g)),
            None => {
                eprintln!("terse-bus: --slot {s}: not a slot a driver serves (2, 3, 4, 6, 7, 9 or 10)");
                return ExitCode::from(2);
            }
        },
        _ => {
            eprintln!("usage: terse-bus --probe-driver <program> [--slot <n>]");
            return ExitCode::from(2);
        }
    };
    let groups: Vec<&[u8]> = match slot {
        Some(g) => vec![g],
        None => terse_net::driverlist::GROUPS.to_vec(),
    };
    let mut answered = 0;
    for g in groups {
        let session = match transport::session_dir() {
            Ok(d) => host::SessionDir(d),
            Err(e) => {
                eprintln!("terse-bus: {e}");
                return ExitCode::from(2);
            }
        };
        let mut config = BusConfig::new(session.0.clone());
        config.shm = false;
        let want = terse_bus::bus::Wanted { slots: g.to_vec(), command: vec![program.clone()], installed: None };
        match terse_bus::bus::Bus::start(&config, &[want]) {
            Ok(bus) => {
                let d = &bus.procs[0].info;
                println!("group {}", g[0]);
                println!("name {}", d.name.replace('\n', " "));
                println!("version {}", if d.version.is_empty() { "-" } else { &d.version });
                println!("build {}", if d.build.is_empty() { "-" } else { &d.build });
                for s in &d.slots {
                    println!("slot {} {}", s.slot, s.version);
                }
                println!();
                answered += 1;
            }
            Err(e) => eprintln!("terse-bus: {e}"),
        }
    }
    if answered > 0 { ExitCode::SUCCESS } else { ExitCode::from(2) }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What was started, and how.
pub struct Run {
    pub opts_frames: Option<u64>,
    pub script: Script,
    pub native: (u32, u32),
    pub startup: Duration,
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let opts = parse(args)?;
    let started = Instant::now();
    let script = match &opts.events {
        Some(p) => Script::parse(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?).map_err(|e| format!("{p}: {e}"))?,
        None => Script::default(),
    };
    let snapshot = match &opts.load_state {
        Some(p) => Some(Snapshot::decode(&read(p)?).map_err(|e| format!("{}: {e}", p.display()))?),
        None => None,
    };
    // The image: a file; the built-in shell; or a snapshot's, from the store.
    let store_dir = opts.content.store.clone().or_else(terse_net::default_store_dir);
    let image_bytes = match (&opts.image, &snapshot) {
        (Some(p), _) => read(Path::new(p))?,
        (None, _) if opts.shell => builtin::SHELL.to_vec(),
        (None, Some(s)) => {
            let store = BlobStore::open(store_dir.as_ref().ok_or("no store directory (use --store)")?, None).map_err(|e| e.to_string())?;
            store.read(&s.image_id()).map_err(|e| e.to_string())?.ok_or("the snapshot's image is not in the store")?
        }
        (None, None) => return Err(USAGE.into()),
    };
    let path = opts.image.clone().unwrap_or_else(|| "shell".into());
    let image = Image::parse(&image_bytes).map_err(|e| format!("{path}: {e}"))?;
    if let Some(s) = &snapshot
        && s.image_id() != image.id
    {
        return Err("the snapshot is of another image".into());
    }
    if opts.pin {
        transport::pin_to(1);
    }
    let (replay, replay_grants) = match &opts.replay {
        Some(p) => {
            let (_, grants, frames) = record::parse(&read(p)?, &image).map_err(|e| format!("{}: {e}", p.display()))?;
            (Some(frames), Some(grants))
        }
        None => (None, None),
    };
    if opts.in_process {
        // SPEC §image: every optional device compiled in, less those withheld.
        let grants = image.grants(|d| !opts.without.contains(&d.slot));
        let service = if image.device_set(&grants).has(4) { Some(opts.content.start()?) } else { None };
        let memory = Memory::Owned { rom: image.rom.clone(), ram: vec![0; image.ram_pages as usize * 8192] };
        let mut core = Machine::with_devices(&image, grants.clone(), memory, InProcessHost::new(&image, &grants, service));
        let startup = started.elapsed();
        return in_process(&opts, &mut core, &script, startup);
    }

    // SPEC §machines.calls: the built-in programs, in the store for the shell.
    let builtins = if opts.shell {
        let store = BlobStore::open(store_dir.as_ref().ok_or("no store directory (use --store)")?, None).map_err(|e| e.to_string())?;
        let extra = opts.builtins.iter().map(|p| read(Path::new(p))).collect::<Result<Vec<_>, _>>()?;
        Some(builtin::install(&store, &extra)?)
    } else {
        None
    };
    // The built-in shell has the identity of a built-in (SPEC §machines.calls).
    let identity = (opts.shell && opts.image.is_none()).then(|| host::shell_identity(&image));
    let dir = std::env::current_exe().map_err(|e| e.to_string())?.parent().ok_or("no directory")?.to_path_buf();
    let overrides: Vec<(Vec<u8>, Vec<String>)> = opts.drivers.iter().map(|(s, c)| (s.clone(), terse_bus::bus::split_command(c))).collect();
    if opts.shell && !opts.without.is_empty() {
        return Err("--without is for a single machine, not --shell".into());
    }
    // The user's drivers folder (DEVICES.md 2.2), after --driver and before
    // this program's directory.
    let config_dir = opts.content.config.clone().or_else(terse_net::default_config_dir);
    let (installed, mut unused, bad) = config_dir.as_deref().map(host::installed_drivers).unwrap_or_default();
    for b in bad {
        eprintln!("terse-bus: {b}");
    }
    let slots = host::installed_slots_with(&dir, &image, opts.shell, &opts.without, &overrides, &installed);
    let drivers = host::drivers_with(&dir, &slots, &opts.content.args(), &overrides, &installed)?;
    let presents = opts.screenshot.is_some() || opts.present_every || opts.window;
    let mut config = BusConfig::new(PathBuf::new());
    config.shm = !opts.socket_only;
    config.presents = presents;
    config.replay = replay.is_some();
    config.batching = opts.batching;
    config.spin = Spin { max: Duration::from_micros(opts.spin_us), adaptive: opts.spin_adaptive };
    config.driver_cpus = if opts.pin { (2..2 + drivers.len()).collect() } else { Vec::new() };
    config.reply_timeout = Duration::from_secs_f64(opts.driver_timeout);
    config.start_timeout = Duration::from_secs_f64(opts.start_timeout);
    let start = Start {
        config,
        drivers,
        image: &image,
        shell: builtins,
        identity,
        snapshot: snapshot.as_ref(),
        grants: replay_grants,
        without: opts.without.clone(),
    };
    let mut bs = start.start()?;
    let startup = started.elapsed();
    // Installed drivers that were not used: said on standard error, and
    // in a window as a notice (DEVICES.md 2.2).
    unused.extend(bs.bus.borrow().notes.iter().cloned());
    for u in &unused {
        eprintln!("terse-bus: {}", u.message());
    }
    if let Some(u) = unused.first() {
        bs.notice = Some((
            host::Notice {
                heading: "an installed driver was not used".into(),
                what: format!("The installed driver {} (slot {}) could not be used.", u.name, u.slot),
                then: format!("Using the shipped driver. To keep it: terse drivers use {} default", u.slot),
            },
            host::NOTICE_FRAMES,
        ));
    }
    // The watch (PERFORMANCE.md R14, R15; RECORD.md).
    let live = opts.window || opts.realtime;
    let (record, default_dir) = match &opts.perf_log {
        Some(Some(d)) => (Some(d.clone()), false),
        Some(None) => (None, false),
        None if live => (terse_perflog::record::default_dir(), true),
        None => (None, false),
    };
    let act = opts.perf_rule.unwrap_or(live);
    if (record.is_some() || act) && replay.is_none() {
        bs.watch(&terse_bus::monitor::WatchConfig {
            record,
            act,
            mode: if opts.window { "window" } else if opts.realtime { "realtime" } else { "headless" },
            renderer: if presents { "drivers" } else { "none" },
            started,
            default_dir,
            rule: Default::default(),
        });
    }
    if opts.bench {
        for d in &bs.bus.borrow().procs {
            eprintln!("bus: slots {:?} {} ({}), transport {}", d.slots, d.info.name, d.command, if d.transport == 1 { "shm" } else { "socket" });
        }
    }
    let native = bs.bus.borrow().native().unwrap_or((640, 480));
    #[cfg(feature = "window")]
    if opts.window {
        let w = window::Options {
            size: opts.size,
            eye: opts.eye,
            parallax: opts.parallax,
            selftest: opts.selftest.then(|| opts.frames.unwrap_or(300)),
            threads: opts.threads,
            flat: opts.panels_flat,
            startup,
        };
        return window::run(bs, script, native, w);
    }
    #[cfg(not(feature = "window"))]
    if opts.window {
        return Err("this terse-bus was built without the window (feature \"window\")".into());
    }
    headless(&opts, &mut bs, &script, replay.as_deref(), native, startup)
}

/// Runs the frames headless and prints what was asked for.
fn headless(opts: &Opts, bs: &mut BusSystem, script: &Script, replay: Option<&[FrameRecord]>, native: (u32, u32), startup: Duration) -> Result<ExitCode, String> {
    let presents = opts.screenshot.is_some() || opts.present_every;
    let mut comp = presents.then(|| Compositor::new(bs.system.shell().devices(), native, opts.threads));
    if let Some(c) = &mut comp {
        c.set_eye(opts.eye);
        if opts.panels_flat {
            c.panel_mode = terse_host::soft::panel::PanelMode::Flat;
        }
    }
    bs.system.set_eye(opts.eye);
    let picture = opts.size.unwrap_or(native);
    let mut times = Vec::new();
    let mut present_times = Vec::new();
    let mut recording = opts.record.as_ref().map(|_| record::header(bs.system.shell().image_id(), bs.system.shell().grants()));
    if (opts.record.is_some() || replay.is_some()) && bs.system.is_system() {
        return Err("--record and --replay are for a single machine".into());
    }
    let total = match replay {
        Some(r) => opts.frames.map_or(r.len() as u64, |n| n.min(r.len() as u64)),
        None => opts.frames.unwrap_or(60 * 60 * 10),
    };
    let mut tick = Instant::now();
    let mut audio = Vec::new();
    for i in 0..total {
        if signals::stop_requested() {
            eprintln!("terse-bus: stopped by a signal");
            break;
        }
        if opts.realtime {
            tick += Duration::from_micros(16_667);
            if let Some(d) = tick.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
        let frame = bs.system.shell().frame();
        let (events, controllers) = match replay {
            Some(r) => {
                let f = &r[i as usize];
                if f.frame != frame {
                    return Err(format!("the recording has frame {} where the machine is at {frame}", f.frame));
                }
                bs.system.shell_mut().devices_mut().replay = Some(f.drivers.clone());
                (f.events.clone(), f.controllers())
            }
            // SPEC §content.feeds: while a question is asked the keyboard is the host's.
            None => (bs.input(script.events(frame).to_vec())?, script.controllers(frame)),
        };
        let start = Instant::now();
        bs.frame(&events, controllers)?;
        let took = start.elapsed();
        if opts.audio_out.is_some() {
            let mut frame = [0i16; 2 * terse_core::sound::FRAME_SAMPLES];
            bs.system.take_audio(&mut frame);
            audio.extend_from_slice(&frame);
        }
        if opts.bench {
            times.push(took.as_secs_f64() * 1000.0);
        }
        if opts.present_every
            && let Some(c) = &mut comp
        {
            let t = Instant::now();
            let prompt = bs.prompt()?;
            c.draw(&mut bs.system, picture, prompt.as_ref().map(|(p, n, t)| (p, *n, t.clone())))?;
            present_times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        // The host's own work after the frame (hashes) comes first: it
        // asks the drivers for their state, which waits for their END.
        let hashed = if recording.is_some() || replay.is_some() {
            let records = bs.system.shell_mut().devices_mut().take_records();
            Some((records, bs.system.shell_mut().try_state_hash()?))
        } else {
            None
        };
        if let Some(n) = opts.hash_every
            && bs.system.shell().frame() % n == 0
        {
            let f = bs.system.shell().frame();
            println!("{f} {}", hex(&bs.system.shell_mut().try_state_hash()?));
        }
        // The watch judges the frame; the rule may stop the machine (R15).
        for line in bs.end_frame(Duration::ZERO)? {
            eprintln!("{line}");
        }
        // A replay stops the machine where the recording says the host did.
        if let Some(r) = replay
            && r[i as usize].halted != 0
        {
            bs.system.shell_mut().halt(r[i as usize].halted);
        }
        if let Some((records, mut hash)) = hashed {
            // The frame's record holds the stop and the hash after it.
            let halted = match bs.system.shell().stopped() {
                Some(Stopped::Halted(s)) => s,
                _ => 0,
            };
            if halted != 0 {
                hash = bs.system.shell_mut().try_state_hash()?;
            }
            if let Some(out) = &mut recording {
                let f = FrameRecord { frame, time: 0, controllers: controllers.map(|c| c.bytes()), events: events.clone(), random: Vec::new(), drivers: records, halted, hash };
                f.encode(out);
            }
            if let Some(r) = replay
                && r[i as usize].hash != hash
            {
                return Err(format!("the replay differs from the recording after frame {frame}"));
            }
        }
        if bs.system.stopped().is_some() {
            break;
        }
    }
    if let Some(path) = &opts.audio_out {
        std::fs::write(path, terse_core::sound::wav(&audio)).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let (Some(path), Some(bytes)) = (&opts.record, &recording) {
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(Stopped::Fault(f)) = bs.system.stopped() {
        eprintln!("terse-bus: fault: {f}");
    }
    if let Some(path) = &opts.frame_times {
        let text: String = times.iter().map(|t| format!("{t:.4}\n")).collect();
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if opts.bench {
        let (p50, p99, max) = spread(&mut times.clone());
        let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
        let over = times.iter().filter(|&&t| t > 1000.0 / 60.0).count();
        eprintln!(
            "bench: {} frames, emulation per frame p50 {p50:.3} ms, p99 {p99:.3} ms, max {max:.3} ms, mean {mean:.3} ms, over 16.7 ms: {over}; start-up {:.1} ms",
            times.len(),
            startup.as_secs_f64() * 1000.0
        );
        if !present_times.is_empty() {
            let (p50, p99, max) = spread(&mut present_times);
            eprintln!("bench: present and compose per frame p50 {p50:.3} ms, p99 {p99:.3} ms, max {max:.3} ms");
        }
        report(bs);
        if let Some(m) = &bs.monitor {
            let o = &m.overhead;
            eprintln!("bench: the watch's own cost per frame (two calls): {}", o.summary_us());
        }
    }
    if let Some(path) = &opts.screenshot
        && let Some(c) = &mut comp
    {
        let prompt = bs.prompt()?;
        let frame = c.draw(&mut bs.system, picture, prompt.as_ref().map(|(p, n, t)| (p, *n, t.clone())))?;
        write_png(path, &frame.to_rgba(), picture)?;
    }
    if let Some(path) = &opts.save_state {
        let s = Snapshot::try_of(bs.system.shell_mut())?;
        std::fs::write(path, s.encode()).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if opts.hash || opts.hash_all {
        println!("{}", hex(&bs.system.shell_mut().try_state_hash()?));
    }
    if opts.hash_all {
        let (_, children) = bs.hashes()?;
        for (h, hash) in children {
            match hash {
                Some(x) => println!("{h} {}", hex(&x)),
                None => println!("{h} -"),
            }
        }
    }
    if opts.dump {
        dump(bs)?;
    }
    Ok(match bs.system.stopped() {
        Some(Stopped::Exit(c)) => ExitCode::from(c),
        Some(stopped @ Stopped::Fault(_)) => faulted(stopped),
        Some(Stopped::Halted(slot)) => halted(slot, bs),
        None => ExitCode::SUCCESS,
    })
}

/// The host stopped the shell or the machine because device `slot` did not
/// keep up (PERFORMANCE.md R15): exit status 4 (STATE.md 6.4).
pub fn halted(slot: u8, bs: &BusSystem) -> ExitCode {
    let saved = bs.halted.as_ref().map_or(String::new(), |(_, d)| format!("; its state before the stop: {d}"));
    eprintln!("terse-bus: stopped by the host: device {slot} did not keep up{saved}");
    ExitCode::from(4)
}

/// A machine's terminal text, from its driver's state section.
fn screen(m: &mut Machine<terse_bus::bus::BusDevices>) -> Result<String, String> {
    if !m.has_device(2) {
        return Ok(String::new());
    }
    let section = m.devices_mut().state(2)?;
    let mut t = Terminal::default();
    t.restore(&mut terse_core::snapshot::Reader::new(&section)).map_err(|e| format!("terminal state: {e}"))?;
    Ok(t.text())
}

/// `--dump` as `terse run --shell --dump` prints it.
fn dump(bs: &mut BusSystem) -> Result<(), String> {
    print!("{}", screen(bs.system.shell_mut())?);
    let Some(dev) = bs.system.shell().machines() else { return Ok(()) };
    let slots: Vec<(u8, terse_core::machines::Slot)> = bs.system.children().map(|(h, _)| (h, dev.slot(h).clone())).collect();
    for (h, slot) in slots {
        let r = slot.rect;
        if r.w == 0 || r.h == 0 {
            continue;
        }
        let child = bs.system.child_mut(h).unwrap();
        let state = terse_core::machines::state::name(slot.state);
        println!("-- machine {h} {state} {} {} {} {} {} --", r.x, r.y, r.w, r.h, child.title);
        if let Some(m) = &mut child.machine {
            print!("{}", screen(m)?);
        }
    }
    Ok(())
}

fn report(bs: &BusSystem) {
    for d in &bs.bus.borrow().procs {
        eprintln!(
            "bus: slots {:?} {}: calls {}; batched {} in {} batches; refused by the core's checks {}; BEGIN {}; PRESENT {}; GET {}",
            d.slots,
            d.info.name,
            d.stats.calls.summary_us(),
            d.stats.batched,
            d.stats.batches,
            d.stats.checked_out,
            d.stats.begin.summary_us(),
            d.stats.present.summary_us(),
            d.stats.gets
        );
    }
}

/// `--in-process`: a single machine with the devices compiled in.
fn in_process(opts: &Opts, core: &mut Machine<InProcessHost>, script: &Script, startup: Duration) -> Result<ExitCode, String> {
    let mut times = Vec::new();
    let mut audio = Vec::new();
    for _ in 0..opts.frames.unwrap_or(60 * 60 * 10) {
        let frame = core.frame();
        let events: Vec<Event> = script.events(frame).to_vec();
        let start = Instant::now();
        core.try_run_frame(&FrameInput { events: &events, controllers: script.controllers(frame), ..Default::default() })?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        if opts.audio_out.is_some() {
            audio.extend(core.take_audio().unwrap_or_else(|| vec![0; 2 * terse_core::sound::FRAME_SAMPLES]));
        }
        if let Some(n) = opts.hash_every
            && core.frame() % n == 0
        {
            println!("{} {}", core.frame(), hex(&core.try_state_hash()?));
        }
        if core.stopped().is_some() {
            break;
        }
    }
    if opts.bench {
        let (p50, p99, max) = spread(&mut times);
        eprintln!("bench: {} frames, emulation per frame p50 {p50:.3} ms, p99 {p99:.3} ms, max {max:.3} ms; start-up {:.1} ms", times.len(), startup.as_secs_f64() * 1000.0);
    }
    if let Some(path) = &opts.audio_out {
        std::fs::write(path, terse_core::sound::wav(&audio)).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if opts.hash {
        println!("{}", hex(&core.try_state_hash()?));
    }
    Ok(match core.stopped() {
        Some(Stopped::Exit(c)) => ExitCode::from(c),
        Some(stopped @ Stopped::Fault(_)) => faulted(stopped),
        Some(Stopped::Halted(_)) => ExitCode::from(4),
        None => ExitCode::SUCCESS,
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
