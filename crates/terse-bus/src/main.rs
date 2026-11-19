//! `terse-bus`: runs one Terse machine with its devices as separate driver
//! processes (DEVICES.md).

#[cfg(feature = "window")]
mod window;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use terse_bus::bus::{Bus, BusConfig, split_command};
use terse_bus::compose::{Compositor, screen_rect, write_png};
use terse_bus::core::{Core, Devices, Memory};
use terse_bus::inproc::{ContentOptions, InProcess};
use terse_bus::proto::MEMORY_RAM_OFFSET;
use terse_bus::record::{self, FrameRecord};
use terse_bus::stats::spread;
use terse_bus::transport::{self, Mapping};
use terse_core::{Image, Script, Stopped};

const USAGE: &str = "usage:
  terse-bus <image> [--frames N] [--events <file>] [--hash] [--hash-every N] [--dump]
            [--driver <slot>=<command line>]... [--absent <slot>]... [--transport shm|socket] [--no-batch] [--in-process]
            [--screenshot <out.png>] [--size <W>x<H>] [--present-every-frame] [--window]
            [--bench] [--realtime] [--frame-times <file>] [--pin] [--spin-us N] [--record <file>] [--replay <file>]
            [--store <dir>] [--config <dir>] [--data <dir>] [--relay ws://...] [--consent allow|deny]
            [--profile reading] [--no-feed-relays] [--no-program-download]

Runs the image's devices as driver processes: by default terse-dev-terminal (slot 2),
terse-dev-vector (3), terse-dev-content (4, given --store, --config, --data, --relay, --consent,
--profile) and terse-dev-page (7), from this program's directory; --driver replaces one (the
command line is split at spaces). --absent runs an image although no driver gives that
device (against SPEC 2: its calls return FFFF_FFFFh; for measurements only). --transport socket offers drivers only the socket (default:
shared memory is offered and the Rust drivers take it). --no-batch sends every call at once.
--in-process runs the same core with the devices compiled in, for comparison.
Inputs are those of `terse run`: the --events script, time 0, no random bytes, content completed
at the next frame. --hash, --hash-every and --dump as in `terse run`. --screenshot composes the
drivers' layers after the last frame (--size, default the terminal's native size);
--present-every-frame presents every frame (as a window would) and --bench then times it too.
--realtime runs at 60 frames a second, as a window would. --frame-times writes each frame's emulation time (ms), one a line. --bench prints the frame times (emulation including the bus) and per-device call latencies.
--pin puts the core on CPU 1 and the drivers on CPUs 2, 3, 4, ... --record writes the bus's
recording; --replay runs one again, checking every frame's hash.";

struct Opts {
    image: Option<String>,
    frames: Option<u64>,
    events: Option<String>,
    hash: bool,
    hash_every: Option<u32>,
    dump: bool,
    drivers: Vec<(u8, String)>,
    absent: Vec<u8>,
    socket_only: bool,
    batching: bool,
    in_process: bool,
    screenshot: Option<PathBuf>,
    size: Option<(u32, u32)>,
    present_every: bool,
    window: bool,
    bench: bool,
    realtime: bool,
    frame_times: Option<PathBuf>,
    pin: bool,
    spin_us: u64,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
    content: ContentOptions,
}

fn parse(args: Vec<String>) -> Result<Opts, String> {
    let mut o = Opts {
        image: None,
        frames: None,
        events: None,
        hash: false,
        hash_every: None,
        dump: false,
        drivers: Vec::new(),
        absent: Vec::new(),
        socket_only: false,
        batching: true,
        in_process: false,
        screenshot: None,
        size: None,
        present_every: false,
        window: false,
        bench: false,
        realtime: false,
        frame_times: None,
        pin: false,
        spin_us: 100,
        record: None,
        replay: None,
        content: ContentOptions::default(),
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        if o.content.take(&a, &mut value)? {
            continue;
        }
        match a.as_str() {
            "--frames" => o.frames = Some(value()?.parse().map_err(|_| "bad --frames")?),
            "--events" => o.events = Some(value()?),
            "--hash" => o.hash = true,
            "--hash-every" => o.hash_every = Some(value()?.parse().ok().filter(|&n| n > 0).ok_or("bad --hash-every")?),
            "--dump" => o.dump = true,
            "--driver" => {
                let v = value()?;
                let (slot, cmd) = v.split_once('=').ok_or("--driver <slot>=<command line>")?;
                o.drivers.push((slot.parse().map_err(|_| format!("bad slot {slot}"))?, cmd.to_string()));
            }
            "--absent" => o.absent.push(value()?.parse().map_err(|_| "bad --absent")?),
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
            "--present-every-frame" => o.present_every = true,
            "--window" => o.window = true,
            "--bench" => o.bench = true,
            "--realtime" => o.realtime = true,
            "--frame-times" => {
                o.frame_times = Some(value()?.into());
                o.bench = true;
            }
            "--pin" => o.pin = true,
            "--spin-us" => o.spin_us = value()?.parse().map_err(|_| "bad --spin-us")?,
            "--record" => o.record = Some(value()?.into()),
            "--replay" => o.replay = Some(value()?.into()),
            "--help" | "-h" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            _ if o.image.is_none() => o.image = Some(a),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(o)
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("terse-bus: {e}");
            ExitCode::from(2)
        }
    }
}

/// The default driver for a slot: a program next to this one.
fn default_driver(slot: u8, opts: &Opts) -> Option<Vec<String>> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = |name: &str| dir.join(name).display().to_string();
    match slot {
        2 => Some(vec![exe("terse-dev-terminal")]),
        3 => Some(vec![exe("terse-dev-vector")]),
        4 => {
            let mut v = vec![exe("terse-dev-content")];
            v.extend(opts.content.args());
            Some(v)
        }
        7 => Some(vec![exe("terse-dev-page")]),
        _ => None,
    }
}

/// Removes the session directory when dropped.
struct SessionDir(PathBuf);

impl Drop for SessionDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let opts = parse(args)?;
    let started = Instant::now();
    let path = opts.image.clone().ok_or(USAGE)?;
    let image = Image::parse(&std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?).map_err(|e| format!("{path}: {e}"))?;
    let script = match &opts.events {
        Some(p) => Script::parse(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?).map_err(|e| format!("{p}: {e}"))?,
        None => Script::default(),
    };
    let replay = match &opts.replay {
        Some(p) => {
            let (id, frames) = record::parse(&std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)?;
            if id != image.id {
                return Err(format!("{}: a recording of another image", p.display()));
            }
            Some(frames)
        }
        None => None,
    };
    if opts.pin {
        transport::pin_to(1);
    }
    if opts.in_process {
        let service = if image.has_device(4) { Some(opts.content.start()?) } else { None };
        let devices = InProcess::new(&image, service);
        let memory = Memory::Owned { rom: image.rom.clone(), ram: vec![0; image.ram_pages as usize * 8192] };
        let mut core = Core::new(image, memory, devices);
        let startup = started.elapsed();
        return frames(&opts, &mut core, &script, replay.as_deref(), None, startup);
    }

    // The drivers, in slot order.
    let mut wanted = Vec::new();
    for d in &image.devices {
        // --absent: a test aid that breaks SPEC 2 on purpose: the device's
        // calls return FFFF_FFFFh and its state section is left out.
        if d.slot == 1 || opts.absent.contains(&d.slot) {
            continue;
        }
        let command = match opts.drivers.iter().rev().find(|(s, _)| *s == d.slot) {
            Some((_, c)) => split_command(c),
            None => default_driver(d.slot, &opts).ok_or(format!("the image lists device {}, for which there is no driver (SPEC 2)", d.slot))?,
        };
        wanted.push((d.slot, d.version, command));
    }
    wanted.sort_by_key(|w| w.0);
    let session = SessionDir(transport::session_dir()?);
    let ram_len = image.ram_pages as usize * 8192;
    let memory_path = session.0.join("memory");
    let mut map = Mapping::create(&memory_path, MEMORY_RAM_OFFSET + ram_len)?;
    map.bytes_mut()[..image.rom.len()].copy_from_slice(&image.rom);
    let presents = opts.screenshot.is_some() || opts.present_every || opts.window;
    let config = BusConfig {
        session: session.0.clone(),
        shm: !opts.socket_only,
        presents,
        replay: replay.is_some(),
        batching: opts.batching,
        spin: Duration::from_micros(opts.spin_us),
        driver_cpus: if opts.pin { (2..2 + wanted.len()).collect() } else { Vec::new() },
    };
    let bus = Bus::start(&config, &wanted, image.id, &image.rom, ram_len as u32, Some(&memory_path))?;
    let memory = Memory::Shared { map, rom_len: image.rom.len(), ram_len };
    let mut core = Core::new(image, memory, bus);
    let startup = started.elapsed();
    if opts.bench {
        for d in &core.devices.drivers {
            eprintln!("bus: slot {} {} ({}), transport {}", d.slot, d.info.name, d.command, if d.transport == 1 { "shm" } else { "socket" });
        }
    }
    let native = core.devices.native().unwrap_or((640, 480));
    let picture = opts.size.unwrap_or(native);
    let layers: Vec<(u8, u8)> = core.devices.layers().into_iter().map(|(_, slot, _, l)| (slot, l.id)).collect();
    let compositor = presents.then(|| Compositor::new(picture, layers));
    #[cfg(feature = "window")]
    if opts.window {
        let code = window::run(core, script, native, compositor.unwrap())?;
        return Ok(code);
    }
    #[cfg(not(feature = "window"))]
    if opts.window {
        return Err("this terse-bus was built without the window (feature \"window\")".into());
    }
    let code = frames(&opts, &mut core, &script, replay.as_deref(), compositor.map(|c| (c, native)), startup);
    core.devices.shutdown();
    drop(session);
    code
}

/// Runs the frames and prints what was asked for.
fn frames<D: Devices + MaybeBus>(
    opts: &Opts,
    core: &mut Core<D>,
    script: &Script,
    replay: Option<&[FrameRecord]>,
    mut comp: Option<(Compositor, (u32, u32))>,
    startup: Duration,
) -> Result<ExitCode, String> {
    let mut times = Vec::new();
    let mut present_times = Vec::new();
    let mut recording = opts.record.as_ref().map(|_| record::header(&core.image.id));
    let total = match replay {
        Some(r) => opts.frames.map_or(r.len() as u64, |n| n.min(r.len() as u64)),
        None => opts.frames.unwrap_or(60 * 60 * 10),
    };
    let mut used = Vec::new();
    let mut tick = Instant::now();
    for i in 0..total {
        if opts.realtime {
            tick += Duration::from_micros(16_667);
            if let Some(d) = tick.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
        let frame = core.frame();
        let (events, controllers, rec) = match replay {
            Some(r) => {
                let f = &r[i as usize];
                if f.frame != frame {
                    return Err(format!("the recording has frame {} where the machine is at {frame}", f.frame));
                }
                (f.events.clone(), f.controllers(), Some(&f.drivers[..]))
            }
            None => (script.events(frame).to_vec(), script.controllers(frame), None),
        };
        let random: &[u8] = match replay {
            Some(r) => &r[i as usize].random,
            None => &[],
        };
        let start = Instant::now();
        let (stats, records) = core.run_frame(0, &events, random, &controllers, rec)?;
        let took = start.elapsed();
        if opts.bench {
            times.push(took.as_secs_f64() * 1000.0);
            used.push(stats.used);
        }
        if opts.present_every
            && let Some((c, native)) = &mut comp
        {
            let t = Instant::now();
            core.devices.present_if_bus(c, *native)?;
            present_times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        if recording.is_some() || replay.is_some() {
            let hash = core.state_hash()?;
            if let Some(out) = &mut recording {
                let f = FrameRecord {
                    frame,
                    time: 0,
                    controllers: controllers.map(|c| c.bytes()),
                    events: events.clone(),
                    random: random[..stats.random_used.min(random.len())].to_vec(),
                    drivers: records,
                    hash,
                };
                f.encode(out);
            }
            if let Some(r) = replay
                && r[i as usize].hash != hash
            {
                return Err(format!("the replay differs from the recording after frame {frame}"));
            }
        }
        if let Some(n) = opts.hash_every
            && core.frame() % n == 0
        {
            println!("{} {}", core.frame(), hex(&core.state_hash()?));
        }
        if core.stopped().is_some() {
            break;
        }
    }
    if let (Some(path), Some(bytes)) = (&opts.record, &recording) {
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(Stopped::Fault(f)) = core.stopped() {
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
            "bench: {} frames, emulation per frame p50 {p50:.3} ms, p99 {p99:.3} ms, max {max:.3} ms, mean {mean:.3} ms, over 16.7 ms: {over}; calls {}; start-up {:.1} ms",
            times.len(),
            core.calls,
            startup.as_secs_f64() * 1000.0
        );
        if !present_times.is_empty() {
            let (p50, p99, max) = spread(&mut present_times);
            eprintln!("bench: present and compose per frame p50 {p50:.3} ms, p99 {p99:.3} ms, max {max:.3} ms");
        }
        core.devices.report();
    }
    if let Some(path) = &opts.screenshot
        && let Some((c, native)) = &mut comp
    {
        core.devices.present_if_bus(c, *native)?;
        write_png(path, &c.rgba(), c.size)?;
    }
    if opts.hash {
        println!("{}", hex(&core.state_hash()?));
    }
    if opts.dump && core.image.has_device(2) {
        let state = core.devices.state(2)?;
        let mut t = terse_core::Terminal::default();
        t.restore(&mut terse_core::snapshot::Reader::new(&state)).map_err(|e| format!("terminal state: {e}"))?;
        print!("{}", t.text());
    }
    Ok(match core.stopped() {
        Some(Stopped::Exit(c)) => ExitCode::from(c),
        Some(Stopped::Fault(_)) => ExitCode::from(255),
        None => ExitCode::SUCCESS,
    })
}

/// What only the bus does: presentation and its report.
trait MaybeBus {
    fn present_if_bus(&mut self, comp: &mut Compositor, native: (u32, u32)) -> Result<(), String>;
    fn report(&self);
}

impl MaybeBus for InProcess {
    fn present_if_bus(&mut self, _: &mut Compositor, _: (u32, u32)) -> Result<(), String> {
        Err("--in-process does not present".into())
    }

    fn report(&self) {}
}

impl MaybeBus for Bus {
    fn present_if_bus(&mut self, comp: &mut Compositor, native: (u32, u32)) -> Result<(), String> {
        self.flush()?;
        let rect = screen_rect(native, comp.size);
        let updates = self.present(comp.size, rect)?;
        comp.update(&updates)
    }

    fn report(&self) {
        for d in &self.drivers {
            eprintln!(
                "bus: slot {} {}: calls {}; batched {} in {} batches; refused by the core's checks {}; BEGIN {}",
                d.slot,
                d.info.name,
                d.stats.calls.summary_us(),
                d.stats.batched,
                d.stats.batches,
                d.stats.checked_out,
                d.stats.begin.summary_us()
            );
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
